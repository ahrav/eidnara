# Research log: per-row allocation churn in the dense and lexical walks

Status: concluded for this study; see the decision brief in
`dense-walk-row-allocation.md`. Branch `rp22/row-allocation-reduction`,
worktree `eidnara-wt-4`. This log records every iteration so a later session
can resume without repeating discovery.

Resume notes: the harness is `crates/retrieval/benches/dense_walk.rs` (all
modes listed in its module doc); nothing in production code changed; the
page-cache experiment used a local env-driven `PRAGMA cache_size` patch in
`kernel::open::apply_preclassification_profile` and `storage::open_sqlite`
that was reverted and is described under Experiment 6. Open items are listed
at the end of the brief.

## Audit finding under test (hypothesis, not established truth)

Per-row allocation churn on both walks: 4 to 5 `String`s plus a `Vec<f32>` per
candidate, then cloned again for judgment.

- `oracle.rs:422-434` and `retrieve.rs:285-298` build
  `OccurrenceCandidate::new(String, class, String, i64, String)` for every
  visited row; `retrieve.rs:318-319` and `revalidate` clone every candidate
  again for the kernel batch; `codec::decode` allocates a `Vec<f32>` per row;
  `TopK::offer` carries a clone of `occurrence_id`. For dense this is per row
  of the whole corpus, not per result.
- Audit measurement: decode-with-allocation of 20k rows 12.0 to 14.1 ms vs
  score-only 6.0 ms; string allocations not isolated; SQLite row
  materialization said to dominate at 2 us/row; allocation churn about
  0.3 us/row.
- Proposed local fix: decode into a reused page buffer; borrow `&str` until the
  top-K' is fixed. Proposed redesign: column-store layout with u32 ordinals.
- Proposed cheapest experiment: allocation count on one exhaustive call at
  N = 20k; expect about 6N if the finding holds; then re-time with a page
  buffer.

## Session 1: orientation (2026-09-16)

### Repository facts established by reading code

- `retrieval::dense::oracle::exhaustive` (StoredVectors source) reads
  `occurrence_vectors` blobs through `PAGE_SQL` and decodes each with
  `codec::decode`. `retrieval::dense::layered::rank_layers` shares the same
  `oracle::walk` but its `ResolvedRows` source takes vectors from resolved
  layers (`RowAccess::row` returns an owned `Vec<f32>`; the resident impl
  clones, the daemon's `PinnedLayer` impl reads bytes at an offset and
  `decode_shape`s them).
- The daemon reaches the dense walk only through
  `daemon::vector_reader::rank`, which calls `rank_layers`. `grep` finds no
  daemon route calling `oracle::exhaustive` or `lexical::retrieve`; both are
  library code exercised by `crates/retrieval/tests` and
  `crates/daemon/tests`. There is no live production request path to measure
  end to end; the end-to-end boundary for this study is one library call
  (`exhaustive`, `rank_layers`, or `retrieve`) including SQLite paging and
  kernel judgment.
- Per visited row the dense walk performs, before any eligibility verdict:
  `row.get::<_, String>(1)` for the class code (a `String` dropped at once),
  `String`s for `occurrence_id`, `source_object_id`, `source_artifact_digest`
  (the last wrapped in `Option<String>` inside `kernel::EligibilityCandidate`),
  `Option<Vec<u8>>` for the stored blob, and `Vec<f32>` from `decode`.
  Per judged row: `eligibility::kernel_candidates` clones the
  `EligibilityCandidate` (two more `String`s); `eligibility::report` clones
  `occurrence_id` into `JudgedOccurrence`; the kernel's `egress_candidates_tx`
  serializes the object ids to JSON and builds `HashMap`s keyed by `String`.
  Per eligible row: `Ranked.occurrence_id` is a clone offered to `TopK`.
  A reading of the code therefore predicts well over 6 allocations per row;
  the count is measured below rather than assumed.
- Kernel judgment per batch (`kernel::admission::egress_candidates_tx`): one
  `served_classes` read and one `load_object_states` read for the batch (ids
  passed as one JSON array), one egress-facts read per distinct digest, one
  scope match per distinct scope id. Batch size is `page_rows`, at most
  `MAX_ELIGIBILITY_CANDIDATES = 1024`.
- Existing measurement infrastructure:
  `crates/daemon/tests/support/alloc_recorder.rs` (global recording
  allocator, 65,536-event ledger, exact byte counters past overflow),
  `crates/retrieval/tests/support/dense.rs` (kernel + projection fixture, 9
  rows, dimension 8), Criterion benches `dense_scalar` and `dense_resolve`
  (no walk-level bench). No "probe_score" artifact exists in the tree; the
  audit's numbers came from a scratch measurement that did not land.
- Environment: AMD EPYC 9R14, 128 CPUs, 2 NUMA nodes, 246 GiB, Linux with
  `perf` (paranoid = 2, user-space sampling allowed) and `valgrind` (DHAT)
  available; Rust 1.98.1.

### Plan

1. Build a walk-level harness (`crates/retrieval/benches/dense_walk.rs`,
   research artifact) that seeds a kernel with N admitted objects, projects N
   occurrences with stored vectors of dimension D, and times one
   `exhaustive` call with `page_rows = 1024`, `k = 10`, `max_rows >= N`.
2. Count allocations per call with a counting allocator (no ledger, so no
   overflow at N = 20k).
3. Attribute wall time across: SQLite paging alone, decode alone, judgment
   alone, score + top-K alone.
4. Prototype the allocation-free walk in the harness (not in production code)
   to bound the headroom.

## Session 1: harness and baseline

Harness: `crates/retrieval/benches/dense_walk.rs` (registered as bench
`dense_walk`, `harness = false`). Build and locate the binary with

```sh
cargo build --release -p retrieval --bench dense_walk --locked
BIN=$(ls -t target/release/deps/dense_walk-* | grep -v '\.d$' | head -1)
EIDNARA_WALK_ROWS=20000 EIDNARA_WALK_SAMPLES=5 $BIN            # time mode
EIDNARA_WALK_ROWS=20000 EIDNARA_WALK_MODE=alloc $BIN            # allocation count
EIDNARA_WALK_ROWS=20000 EIDNARA_WALK_SAMPLES=3 EIDNARA_WALK_MODE=stages $BIN
```

Workload: N `CanonicalClaims` occurrences, each with one admitted decision in
the kernel and one stored unit vector of dimension D (fixed LCG stream), one
random unit query, `page_rows = 1024`, `k = 10`, `max_rows = N`, unbounded
budget. The walk's top-10 (ids and f64 scores) must equal the harness's
independent f64 reference before any timing runs; it did in every run below.
Timing boundary: one `store.with_conn(|conn| exhaustive(..))` call. Fixture
seeding, warmup, reference, and printing are outside it. Single thread,
unpinned, host otherwise idle but uncontrolled. Exploratory numbers.

### Baseline (observation)

N = 20,000, D = 384, release build, 5 samples:
`exhaustive` min 488.9 ms, median 489.0 ms, max 490.5 ms; 24.4 us/row.
N = 2,000: median 25.0 ms; 12.5 us/row. Per-row cost roughly doubles from
2k to 20k rows, so at least one stage is superlinear in N.

The audit's implied budget (about 2 us/row SQLite + 0.6 us/row decode + 0.3
us/row allocation, about 3 us/row) is off by about 8x against this
measurement. The discrepancy is explained by the stage attribution below: the
audit never measured kernel judgment or the real page query.

### Stage attribution (observation), N = 20,000, D = 384, medians of 3

| Stage (run alone over the same rows) | ms | us/row | share of call |
|---|---:|---:|---:|
| `page_borrowed`: the oracle's page query, columns read as `&str`/`&[u8]`, checksum only | 100.4 | 5.02 | 20.6% |
| `page_materialized`: the same query materialized as `read_page` does (`String`s, `Vec<u8>`) | 117.4 | 5.87 | 24.1% |
| `decode_alloc`: `codec::decode` per blob (one `Vec<f32>` each) | 15.3 | 0.76 | 3.1% |
| `decode_into_page_buffer`: same decode and checks into one reused page buffer | 11.2 | 0.56 | 2.3% |
| `judge`: `judge_tracked` over the 20 pages of 1024 candidates | 381.1 (min 331.4) | 19.05 | 78% |
| `score_topk`: f64 inner product + `TopK::offer` with `Ranked` clone | 6.5 | 0.32 | 1.3% |
| `clone_candidates`: `Vec<OccurrenceCandidate>::to_vec()` (the kernel-batch clone) | 2.0 | 0.10 | 0.4% |
| `exhaustive` (same session) | 487.7 | 24.4 | 100% |

Stages overlap (materialization is inside the page stage and the walk), so
the shares sum past 100%; the whole-call profile below is the consistent view.
At N = 2,000 the same stages were: page_borrowed 2.25 ms (1.1 us/row),
page_materialized 3.04 ms, decode 1.23 ms, judge 18.5 ms (9.2 us/row),
score 0.64 ms, exhaustive 24.5 ms.

Allocation-attributable deltas at N = 20k: materialized minus borrowed paging
17.0 ms (0.85 us/row), decode with versus without per-row `Vec` 4.1 ms
(0.20 us/row), candidate clone 2.0 ms (0.10 us/row). Together about 23 ms of
488 ms, or 4.7% of the call. That is the whole headroom of the local
optimization the audit proposes, before any implementation cost.

### Whole-call CPU profile (observation)

`perf record -F 2000 -g -D 5000` over 20 timed calls (seeding excluded),
17,655 samples, grouped by symbol class:

| Class | share |
|---|---:|
| SQLite VDBE, b-tree, pager (`sqlite3VdbeExec` 14.8%, `BtreeTableMoveto` 7.1%, `BtreeIndexMoveto` 6.4%, `pcache1Fetch` 4.0%, ...) | 63.0% |
| `malloc`/`free` (SQLite's C allocations and Rust's together) | 7.2% |
| `memcmp` (SQLite key comparison) | 5.6% |
| `memmove`/`memset` | 4.0% |
| `codec::decode` | 3.9% |
| kernel crate Rust (`egress_candidates_tx` closures, `object_row_from`, `text_column`) | 3.4% |
| kernel-mode (page faults, `pread`) | 2.9% |
| `core::str::from_utf8` (rusqlite `String` reads) | 2.1% |
| `score::score` | 2.1% |
| pthread mutex, hashing, other | about 4% |

Artifacts: `/tmp/opencode/walk/perf-exh2.data` (not committed).

### Reading

- The limiting mechanism of one exhaustive call is the kernel eligibility
  judgment: about 19 us per candidate at N = 20k, 78% of the call, executed
  as SQLite b-tree seeks inside `kernel::admission::served_classes` and its
  neighbours (many correlated lookups per candidate). Its per-row cost also
  doubles from N = 2k to N = 20k, so part of it scales with kernel or batch
  size; the mechanism is not yet isolated (next experiment).
- SQLite paging of the projection is second at about 5 us/row and also
  superlinear (1.1 us/row at 2k). The 20k x 1.5 KiB blob set is 30 MiB,
  past the default SQLite page cache, so pager misses through the OS page
  cache are the likely mechanism; not yet isolated.
- The audited allocation churn is real (counted next) but is about 5% of the
  call. It cannot be the limiting mechanism at any N while every visited row
  is judged.

### Open questions and next experiments

1. Allocation count per call (alloc mode), to confirm or refute "about 6N".
2. Does judgment cost depend on kernel size (scan) or only on batch size?
   Experiment: fixed N = 2,000 projected rows, kernel seeded with 2,000 vs
   20,000 admitted objects.
3. Design alternative that removes cost: score first, judge only rows that
   would displace the current top-K (identical result set by the shared
   comparator; changes `Consumed.judged/batches/excluded` and the contract's
   "judged in bounded batches" wording). Prototype in the harness, verify
   against the reference with a partly inadmissible corpus, and time.
4. The daemon's real path is `rank_layers` over pinned layers, not
   `exhaustive`; measure it with resident layers for the second baseline.
5. Sensitivity: D in {128, 1024}, N in {5k, 50k}.

### Allocation count (observation)

`alloc` mode, N = 20,000, D = 384, one `exhaustive` call: 641,132 allocations,
533 reallocations, 641,121 deallocations, 124.98 MB requested, 3.06 MB peak
live above the window start. That is 32.06 allocations and 6,249 bytes per
visited row, not the audit's "about 6N". Per stage (stages mode, one run
each): page materialization 5.00/row (2,167 B), decode 1.00/row (1,560 B),
kernel judgment 25.04/row (2,719 B), score + top-K 1.00/row (64 B), candidate
clone for the batch 3.00/row (227 B, inside the judge count). The walk's own
code accounts for 7 of the 32; the other 25 are inside `kernel::judge_eligibility`
(candidate clones, `JudgedOccurrence` ids, JSON id list, `HashMap<String, _>`
keys, per-candidate facts). The audit's hypothesis about which allocations
exist is right; its count and its location are not.

### Experiment 1: score-first walk with lazy judgment (prototype, `lazy` mode)

Hypothesis: judging only rows that can enter the current top-K removes most
of the dominant cost while returning the same set, because a row in the final
top-K beats the worst held member at every earlier point of the walk, and the
kernel batch per page shrinks from 1024 to the rows that pass that test.

Change: `lazy_walk` in the harness re-implements the walk: the oracle's page
query, columns borrowed as `&str`/`&[u8]`, decode into one reused page
buffer, `codec::validate`, score, select rows that would enter the top-K under
`rank_order`, one `judge_tracked` batch per page over the selected rows,
offer the eligible ones, then one final re-judgment of the held set. The
prototype lives in the harness only; production code is untouched.

Success criteria: identical ids and scores to both the f64 reference and the
`exhaustive` result, with and without inadmissible rows; wall time below the
baseline. Stopping condition: one interleaved run of 5 pairs per corpus.

Result (N = 20,000, D = 384, interleaved candidate/baseline pairs, medians of 5):

| Corpus | lazy ms | exhaustive ms | ratio | rows judged (lazy / base) | kernel batches (lazy / base) |
|---|---:|---:|---:|---:|---:|
| all admitted | 143.9 (143.4 to 145.2) | 490.2 (481.9 to 491.0) | 0.293 | 1,076 / 20,010 | 13 / 21 |
| every 3rd object inadmissible (6,666 Hidden) | 140.5 (140.1 to 161.2) | 420.0 (414.8 to 424.1) | 0.335 | 1,104 / 20,010 | 16 / 21 |

Correctness: top-10 ids and scores equal the reference and the exhaustive
walk in both corpora (asserted before timing). Allocations: 1.74 per visited
row (312 B) versus 32.06 (6,249 B). The first page always judges its 1024 rows
because the heap is empty; after that only 52 to 80 rows over 19 pages needed
a verdict.

Keep (as the leading design direction). What it changes: `Consumed.judged`,
`Consumed.batches`, and `Consumed.excluded` describe the judged subset rather
than the population; the contract text "judged for canonical eligibility in
one kernel batch" per page becomes "scored, then the rows that can enter the
top-K are judged"; a snapshot move is detected on fewer batches, but the final
re-judgment still fences the returned set exactly as today. No daemon code
reads `Consumed.excluded` for the dense ranking (grep). `DenseCoverage` and
`Completion` semantics are unchanged.

Untested: budget and authority-moved handling in the prototype (it asserts no
move), the `rank_layers` source, and page sizes other than 1024.

### Experiment 2: does judgment cost scale with kernel size? (refuted)

Hypothesis: the doubling of per-candidate judgment cost from N = 2k to 20k
comes from a scan of the kernel proportional to the number of admitted
objects. Change: N = 2,000 projected rows held fixed; kernel seeded with
2,000, 20,000, and 100,000 admitted objects (`EIDNARA_WALK_KERNEL_OBJECTS`).
Result (judge stage, medians of 5): 18.36 ms, 19.75 ms, 20.49 ms (9.2, 9.9,
10.2 us/row). A 50x larger kernel costs 11%. Discard the scan hypothesis.
The remaining explanation for the superlinearity (also seen in projection
paging: 1.1 us/row at 2k, 5.0 us/row at 20k) is SQLite page-cache capacity:
both stores run at the default `cache_size` (about 2 MiB per connection),
the 20k x 1.5 KiB blob set alone is 30 MiB, and the whole-call profile shows
`pcache1Fetch`, `getPageNormal`, `readDbPage`, and `pread` in the pager path.
Not isolated further; it is not the audited mechanism either way.

### Experiment 3: 2x2 matrix, judgment policy x row materialization

Hypothesis: the local optimization (borrow columns as `&str`, decode into a
reused page buffer) saves the 17 to 23 ms the stage deltas suggested, on top
of whatever lazy judgment saves.

Change: `proto_walk(lazy, materialize)` in the harness. `materialize` on
reads every column and the blob as owned `String`/`Vec<u8>` and decodes into a
fresh `Vec<f32>` per row exactly as `read_page` + `codec::decode` do; off
borrows columns and decodes into one page buffer. `lazy` off judges every row
of every page as the oracle does. The `judge_all_materialize` cell is a
re-implementation of today's walk and is compared against the production
`exhaustive` call in the same interleaved rounds to check harness fidelity.

N = 20,000, D = 384, 7 interleaved rounds, all cells equal the reference and
the exhaustive result:

| Cell | median ms | min to max | rows judged | allocs/row |
|---|---:|---|---:|---:|
| `exhaustive` (production) | 481.2 | 477.8 to 501.8 | 20,010 | 32.06 |
| `judge_all_materialize` (re-implementation of today) | 478.2 | 475.9 to 489.3 | 20,010 | 35.06 |
| `judge_all_borrow` (local optimization only) | 480.7 | 479.1 to 492.8 | 20,010 | 32.06 |
| `lazy_materialize` (design change only) | 141.5 | 140.9 to 144.5 | 1,076 | 7.58 |
| `lazy_borrow` (both) | 141.8 | 140.4 to 147.2 | 1,076 | 1.74 |

Result: the re-implementation reproduces the production walk within 0.6%.
The materialization axis moves nothing measurable in either judgment policy
(differences of 0.3 to 2.5 ms against a round-to-round spread of about 5 to
15 ms), even though it removes 3 to 6 allocations per row. The stage-level
delta of 17 ms was an artifact of the stage retaining all 20k materialized
rows at once (50 MB live, cold cache); in the walk each page's rows are freed
before the next page and the allocator serves them from warm memory. The
judgment axis moves 3.4x.

Why the borrow cell cannot save more while every row is judged: the kernel's
`judge_eligibility` takes `&[EligibilityCandidate]` with owned `object_id:
String` and `artifact_digest: Option<String>`, and the walk needs the owned
`occurrence_id` to report verdicts and page, so three of the five per-row
strings are required by the batch API whichever way the row is read. Only the
class code string, the blob copy, and the per-row `Vec<f32>` are avoidable,
and those cost about 3 x 1.5 KiB of warm memcpy per row, under the noise.

Discard the local optimization as a latency change. Keep lazy judgment.

Interpretation of the audit's own numbers: "decode-with-allocation 12 to 14
ms vs score-only 6 ms at 20k rows" is consistent with this harness
(decode_alloc 13.6 to 15.3 ms, score 6.4 ms), but both are 1.3% to 3% of a
488 ms call; the audit's framing of SQLite as the 2 us/row denominator missed
the 19 us/row kernel judgment and understated paging at this N by 2.5x.

### Experiment 3b: paired, pinned re-run of the matrix (D = 384, N = 20k)

The first matrix runs happened while the shared host carried a load average of
20 to 48 (other users' jobs); the round-to-round spread reached 15 to 80 ms.
Re-run pinned with `taskset -c 5`, 15 interleaved rounds per configuration,
paired differences per round, Student-t 95% intervals (14 degrees of
freedom; nominal, exploratory). Two decode-scratch shapes for the borrowing
cells: one page buffer (`page_rows x D` f32, 1.5 MiB here, past this core's
1 MiB L2) and one row buffer (`EIDNARA_WALK_ROW_BUFFER=1`). The borrowing
cells' decode loop was also changed from per-word `push` to
`extend(map(from_le_bytes))` so it matches `codec::decode`'s collect; with
the `push` loop the borrowing cells had measured 0.6% to 1.6% slower than
materializing, a harness artifact rather than a property of borrowing.

| Contrast (mean paired difference, 95% CI) | page buffer | row buffer |
|---|---|---|
| `judge_all_borrow` minus `judge_all_materialize` | -4.57 ms (-8.08 to -1.06), ratio 0.991 | -2.91 ms (-5.43 to -0.39), ratio 0.994 |
| `lazy_borrow` minus `lazy_materialize` | -5.75 ms (-6.62 to -4.89), ratio 0.960 | -7.77 ms (-8.86 to -6.68), ratio 0.947 |
| `judge_all_materialize` minus production `exhaustive` | +0.08 ms (-2.75 to +2.90) | -2.98 ms (-4.72 to -1.24) |
| `lazy_borrow` minus production `exhaustive` | -351.0 ms (-352.7 to -349.3), ratio 0.283 | -349.3 ms (-352.3 to -346.2), ratio 0.281 |

Medians (row-buffer run): exhaustive 484.3 ms, judge_all_materialize 481.2,
judge_all_borrow 476.5, lazy_materialize 143.0, lazy_borrow 135.3.

Reading: the local optimization (borrow columns, decode into a reused
scratch, no per-row `Vec<u8>`/`Vec<f32>`/class `String`) is real but worth
0.6% to 0.9% of today's call. Inside the score-first design it is worth 4%
to 5% of a call that is already 3.5x shorter, because the row scratch fits
the score-immediately shape and nothing decoded has to outlive the row.
The judgment axis is worth 3.5x on its own. Harness fidelity holds within
0.6%.

### Experiment 4: sensitivity of the two designs (observation)

Pinned (`taskset -c 5`), row buffer on, N = 20,000 and D = 384 unless stated,
medians of 5 to 9 interleaved rounds. `exhaustive` is production;
`lazy_borrow` is the prototype.

| Variation | exhaustive ms | lazy_borrow ms | ratio | rows judged (lazy) |
|---|---:|---:|---:|---:|
| baseline (k = 10, page_rows = 1024) | 484 | 135 | 0.28 | 1,076 |
| k = 100 | 493 | 151 | 0.31 | 1,471 |
| k = 1024 (the maximum) | 515 | 249 | 0.48 | 5,629 |
| page_rows = 256 | 609 | 128 | 0.21 | 321 |
| page_rows = 128 | 572 | 130 | 0.23 | 207 |
| D = 128 | 471 | 117 | 0.25 | 1,077 |
| D = 1024 | 589 (noisy host) | 224 (noisy host) | 0.38 | 1,067 |
| N = 5,000 | 86 | 34 | 0.39 | 1,047 |
| N = 50,000 | 1,687 | 354 | 0.21 | 1,083 |
| every 3rd object inadmissible | 420 | 140 | 0.33 | 1,104 |

Per-row cost of the production walk grows with N (17.3, 24.2, 33.7 us/row at
5k, 20k, 50k); the prototype stays at 6.8 to 7.1 us/row. The first page always
judges `page_rows` rows because the heap is empty, so a smaller first page
(or a first page sized to k) lowers judged rows further; the production walk
gets slower with smaller pages because each kernel batch carries about 0.6 ms
of fixed cost (158 batches at page_rows = 128).

### Experiment 5: the lexical walk and a surprise (observation)

`lexical` mode runs `lexical::retrieve` with one probe (`text`, matching
every row) over the same 20k-row projection, `scan_rows` 64 or 1024, one or
two kernel batches. Per scanned row the walk makes 61 to 62 Rust allocations
(9.6 KB at 64, 7.0 KB at 1024); almost all of them are the kernel judgment's
(two batches of `scan_rows` rows, 25 per row) plus the `BTreeMap` and
candidate clones. The FTS5 `MATCH` with `ORDER BY rank` over 20k matching
rows dominates the call: scan alone 27 ms of a 30 ms call at `scan_rows = 64`.
So the lexical half of the finding is even less material than the dense half:
its per-row allocation cost is bounded by `scan_rows`, not by the corpus, and
the engine scan is the cost.

Surprise: at `scan_rows = 1024` the same `retrieve` call alternated between
65 ms and 340 ms, and a plain probe scan on the same connection alternated
between 30 ms and 300 ms, depending only on what ran before it. `strace`
showed the slow runs issuing 154,430 `pread64` calls on `search.sqlite`
against 12 in the fast runs, with equal VDBE step counts (395,924) and
`stime` of about 220 ms; a second raw connection to the same file, idle in
between, showed its own page cache shrunk from 9.84 MB to 1.86 MB with
154,441 cache misses after three `retrieve` calls had run. Mechanism:
rusqlite's bundled SQLite is compiled with `SQLITE_ENABLE_MEMORY_MANAGEMENT`
(`libsqlite3-sys` `build.rs`), which puts every connection's page cache in
one process-wide `pcache1` group; a cache that reaches its own `cache_size`
recycles the group-wide least-recently-used page, which belongs to whichever
connection read longest ago. The kernel judgment (kernel connection, 2 MiB
default `cache_size`) therefore evicts the search connection's pages, and
the search paging evicts the kernel's, batch after batch. This is the
mechanism behind the superlinear per-row costs of both paging and judgment
in Experiment 2, and it is invisible to allocation counting.

### Experiment 6: page-cache sizing (uncommitted local patch, reverted)

Hypothesis: raising `PRAGMA cache_size` on both stores removes the cross-
connection eviction and the pager `pread` traffic, cutting both paging and
judgment cost without any walk change. Change: an environment-variable-driven
`cache_size` pragma added locally to `kernel::open::apply_preclassification_profile`
and to `storage::open_sqlite`, used only for these runs and reverted
afterwards (`git checkout` of both files; nothing committed). N = 20,000,
D = 384, pinned, row buffer on.

| `cache_size` per connection | page_borrowed | judge (stage) | exhaustive | lazy_borrow | rank_layers | layered_lazy_borrow |
|---|---:|---:|---:|---:|---:|---:|
| 2,000 KiB (default) | 106.7 | 345.5 | 495 / 484 | 135 | 483 | 100 |
| 8 MiB | | | 440 | 119 | | |
| 16 MiB | | | 386 | 85 | | |
| 64 MiB | 42.3 | 234.7 | 342 | 78.6 | 337 | 75.2 |

At N = 50,000 with 64 MiB: exhaustive 1,058 ms (from 1,687), lazy_borrow
205 ms (from 354), 4.1 us/row and now linear in N. The lexical bistability
disappears at 64 MiB: `retrieve` 58 to 60 ms on every call, scans 30 ms.
Kernel judgment at 64 MiB still costs 11.7 us per candidate (234.7 ms for
20,010), so even fully cached it remains the limiter of the judge-everything
walk (69% of 342 ms); it is the batch's SQL shape, not I/O.

Correctness: every configuration in Experiments 4 to 6 asserted the top-k
against the reference and the production walk before timing.
