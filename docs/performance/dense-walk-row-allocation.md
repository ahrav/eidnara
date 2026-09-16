# Decision brief: per-row allocation churn in the dense and lexical walks

Status: exploratory evidence, single host, one workload family. Research log
with every run: `dense-walk-row-allocation-log.md`. Harness:
`crates/retrieval/benches/dense_walk.rs`.

## Verdict

Reject the audited local optimization as a latency change. Pursue a
different execution design for the dense walk: score first, judge only the
rows that can enter the top-K. Separately, size the SQLite page caches; that
is a configuration change, not a walk change, and it compounds with the
redesign.

The audit's hypothesis was right about the mechanism's existence and wrong
about its size and location. One `exhaustive` call over 20,000 rows makes 32
Rust allocations per visited row (the audit expected about 6), but 25 of
those 32 happen inside `kernel::judge_eligibility`, not in the walk, and the
walk's own 7 are worth 0.6% to 0.9% of wall time. The call is limited by the
kernel eligibility judgment (about 70% of the time) and by SQLite paging under
a 2 MiB page cache (about 20%), not by allocation.

## Reproduced baseline

Workload: N `CanonicalClaims` occurrences with admitted kernel decisions and
stored unit vectors of dimension D; one random unit query; `k = 10`,
`page_rows = 1024`, `max_rows = N`, unbounded budget; single thread pinned
with `taskset -c 5`; warm OS page cache; AMD EPYC 9R14, Rust 1.98.1, release
build. Timing boundary: one `store.with_conn(|conn| exhaustive(..))`.
Every timed configuration first asserted that the walk's top-k ids and f64
scores equal an independent reference.

| N | D | `exhaustive` median | per row |
|---:|---:|---:|---:|
| 2,000 | 384 | 24.3 ms | 12.2 us |
| 5,000 | 384 | 86 ms | 17.3 us |
| 20,000 | 384 | 484 to 495 ms | 24.4 us |
| 50,000 | 384 | 1,687 ms | 33.7 us |

The audit's implied budget of about 3 us/row (2 us SQLite, 0.6 us decode,
0.3 us allocation) misses the kernel judgment entirely and undercounts paging
at this N by 2.5x. Its own decode and score numbers reproduce (decode 13.6 to
15.3 ms, score 6.4 ms at 20k) and are 1.3% to 3% of the call.

## Limiting mechanism

Stage attribution and a CPU profile agree (N = 20k, D = 384):

- Kernel eligibility judgment: 331 to 381 ms of 488 (68% to 78%); 19 us per
  candidate; 25 allocations per candidate. The cost is the judgment SQL's
  shape (many correlated lookups per candidate in `served_classes` and its
  neighbours) plus page-cache misses. It does not depend on kernel size (a
  50x larger kernel costs 11% more) but does depend on how many rows are
  judged per call.
- SQLite paging of the projection: 100 ms borrowed, 117 ms materialized
  (5 to 6 us/row). Superlinear in N because the 20k x 1.5 KiB blob set is 30
  MiB against a 2 MiB `cache_size`.
- Decode 15 ms, score plus top-K 6.5 ms, candidate clones 2 ms.
- Whole-call profile: 63% SQLite VDBE/b-tree/pager, 7% `malloc`/`free`
  (SQLite's and Rust's together), 5.6% `memcmp`, 3.9% decode, 2% score.

A second mechanism found on the way: rusqlite's bundled SQLite is built with
`SQLITE_ENABLE_MEMORY_MANAGEMENT`, so every connection's page cache shares one
process-wide LRU group. A cache that fills recycles the group-wide
least-recently-used page, which belongs to another connection. The kernel
judgment therefore evicts the search connection's pages after every batch and
vice versa. This is why per-row costs of both paging and judgment double from
2k to 20k rows, and why a lexical `retrieve` at `scan_rows = 1024` alternated
between 65 ms and 340 ms depending only on what ran before it (154,430
`pread64` calls in the slow runs against 12 in the fast ones, equal VDBE step
counts).

## Headroom

| Lever | Measured effect at N = 20k, D = 384 | Nature |
|---|---|---|
| Local optimization alone (borrow `&str`, reused decode scratch) while every row is judged | 481 to 485 ms to 476 to 480 ms; paired mean -2.9 to -4.6 ms, 95% CI excludes zero but stays under 1% | measured |
| Score first, judge only rows that can enter the top-K (prototype) | 484 ms to 135 to 143 ms, ratio 0.28 to 0.30, 1,076 rows judged instead of 20,010 | measured |
| Local optimization inside the score-first walk | 143 ms to 135 ms, ratio 0.95 | measured |
| `cache_size` 64 MiB on both stores, no walk change (local uncommitted patch, reverted) | 495 ms to 342 ms | measured |
| Both: score-first walk with 64 MiB caches | 78.6 ms (ratio 0.16 against today) | measured |
| Same, `rank_layers` over a resident layer (the daemon's path) | 483 ms to 75 ms | measured |
| Column-store layout with u32 ordinals (audit's redesign) | not built; its allocation motive is gone once rows are not materialized, and its paging motive is addressed more cheaply by the cache and by not reading blobs the layered path never reads | untested hypothesis |

The idealized floor for the score-first walk at 20k rows is about 42 ms of
paging plus 10 ms decode plus 6.5 ms score plus about 15 ms for 13 small
kernel batches; the prototype reaches 79 ms with the large cache. The
remaining gap is the borrowed page query itself (2.1 us/row for three b-tree
probes per row: primary key, tombstone, pending job).

## Experiment table

| # | Hypothesis | Change | Result | Correctness | Keep |
|---|---|---|---|---|---|
| 0 | About 6N allocations per call | count with a global allocator | 32.06 per row: 5 page materialization, 1 decode, 25 kernel judgment, 1 `Ranked` clone | n/a | fact recorded |
| 1 | Lazy judgment removes most cost, same result set | prototype `lazy_walk` | 490 to 144 ms; with 1/3 rows inadmissible 420 to 140 ms | top-10 equals reference and production in both corpora | keep |
| 2 | Judgment cost scales with kernel size | N = 2k fixed, kernel 2k/20k/100k objects | 9.2, 9.9, 10.2 us/row | n/a | discard hypothesis |
| 3 | Borrowing saves the 17 to 23 ms the stages suggested | 2x2 matrix, re-implemented production walk as control | control matches production within 0.6%; borrow axis -0.6% to -0.9% (judge all), -4% to -5% (lazy); the stage delta was a retention artifact | all cells equal reference | discard as latency change |
| 3b | Page buffer vs row buffer | `EIDNARA_WALK_ROW_BUFFER` | row buffer 2 to 3 ms better in the lazy cell; a 1.5 to 4 MiB page buffer exceeds L2 | equal | prefer row scratch |
| 4 | Ratio holds across shapes | k 10/100/1024, page_rows 128/256/1024, D 128/384/1024, N 5k/20k/50k, exclusions | ratio 0.21 to 0.48; worst is k = 1024 (5,629 rows judged) | equal in every run | keep |
| 5 | Lexical walk shares the finding | `lexical::retrieve`, one probe over 20k matches | 61 allocations per scanned row, almost all kernel; FTS5 scan is 27 of 30 ms; bounded by `scan_rows` not corpus | contributions bounded as expected | finding immaterial for lexical |
| 5b | (surprise) 65/340 ms bistability | strace, VDBE status, second raw connection | cross-connection page-cache eviction under `SQLITE_ENABLE_MEMORY_MANAGEMENT` | n/a | new finding |
| 6 | Larger page caches remove the eviction and the `pread` traffic | env-driven `cache_size` on both stores (reverted) | 8/16/64 MiB: exhaustive 440/386/342 ms, lazy 119/85/79 ms; N = 50k lazy becomes linear (4.1 us/row); lexical bistability gone | equal | keep as a separate change |

## Recommended approach

1. Change `oracle::walk` to score before judging and judge only rows that
   would enter the current top-K under the existing `rank_order`
   comparator, one kernel batch per page over the selected rows, then the
   existing final re-judgment. Decode into a row scratch and borrow the
   eligibility columns until a row is selected; that is where the audit's
   local optimization belongs and where it is worth 4% to 5%. Size the first
   page's judged set to what the heap can hold rather than `page_rows`.
2. Give the search and kernel connections an explicit `cache_size`
   proportional to the tables they walk, accounted like other resident
   memory. 16 MiB per connection captured most of the gain at 20k rows.
3. Do not build the column-store layout for this finding. Its motive here
   was allocation churn, which the redesign removes without a storage change.

### Tradeoffs

- The walk's contract changes: `Consumed.judged`, `batches`, and `excluded`
  describe the judged subset, not the population; the contract text
  "judged for canonical eligibility in one kernel batch" per page needs the
  matching edit in `docs/dense-vector-contract.md` and the module docs.
  `DenseCoverage`, `Completion`, and the returned set are unchanged. No
  daemon code reads `Consumed.excluded` for the dense ranking.
- Fewer batches means fewer chances to observe a snapshot or incarnation
  move mid-walk; the final re-judgment still fences the returned set exactly
  as today. Whether a move seen only at the end should still end the walk
  `Incomplete` is a design decision to make explicitly.
- Rows that are not judged are still scored, so embedding content of
  ineligible rows is read and multiplied; it is not returned. Today
  ineligible rows are never scored.
- Larger page caches cost resident memory per connection (`READ_POOL_SIZE`
  kernel readers plus the writer plus the search connection) and belong in
  the admission ledger's accounting.
- Worst realistic case for the redesign is `k = MAX_ELIGIBILITY_CANDIDATES`
  (1024): still 2x faster, with 28% of rows judged.

### Strongest contrary evidence

- The local optimization does produce a measurable gain (95% CI excludes
  zero) and removes 3 of the walk's own 7 allocations per visited row; if
  the redesign is rejected, it is still a small hygiene win, not nothing.
- All measurements are warm-cache, single-threaded, one host, one synthetic
  corpus (uniform random unit vectors, identical digests, one scope, one
  class). A corpus whose top scores concentrate in inadmissible rows judges
  more rows under the lazy design; the 1/3-inadmissible run judged 1,104
  rows, but adversarial distributions were not tried.
- The kernel judgment's per-candidate cost (about 12 us fully cached) is a
  kernel-side property this study did not try to reduce; if it were made 10x
  cheaper, the judge-everything walk would be within 2x of the redesign and
  the semantic change might not be worth it.

## Reproduction

```sh
cargo build --release -p retrieval --bench dense_walk --locked
BIN=$(ls -t target/release/deps/dense_walk-* | grep -v '\.d$' | head -1)
# Baseline timing and allocation count at the study's scale.
EIDNARA_WALK_ROWS=20000 EIDNARA_WALK_SAMPLES=5 taskset -c 5 $BIN
EIDNARA_WALK_ROWS=20000 EIDNARA_WALK_MODE=alloc taskset -c 5 $BIN
# Stage attribution, the 2x2 matrix with the prototype, the layered path, the lexical walk.
EIDNARA_WALK_ROWS=20000 EIDNARA_WALK_SAMPLES=3 EIDNARA_WALK_MODE=stages taskset -c 5 $BIN
EIDNARA_WALK_ROWS=20000 EIDNARA_WALK_SAMPLES=15 EIDNARA_WALK_ROW_BUFFER=1 EIDNARA_WALK_MODE=matrix taskset -c 5 $BIN
EIDNARA_WALK_ROWS=20000 EIDNARA_WALK_SAMPLES=5 EIDNARA_WALK_MODE=layered taskset -c 5 $BIN
EIDNARA_WALK_ROWS=20000 EIDNARA_WALK_SCAN_ROWS=1024 EIDNARA_WALK_LEXICAL_ORDER=1 EIDNARA_WALK_MODE=lexical taskset -c 5 $BIN
```

The page-cache runs need the env-driven `cache_size` pragma described in the
log's Experiment 6; that patch is not committed. Without arguments the bench
runs a 2,000-row, two-sample smoke, which is what CI's bench-in-test-mode step
executes. Profiles and traces from this session are under
`/tmp/opencode/walk/` on the study host and are not committed.

## Remaining uncertainty and smallest validation before implementation

- Budget exhaustion and authority-moved handling in the score-first walk were
  not prototyped; the prototype asserts no move. Validation: port the
  existing `dense_oracle.rs` hook tests (budget windows, snapshot change,
  incarnation change) to the new walk and decide the mid-walk-move rule.
- Cold-cache and concurrent behaviour were not measured. The redesign reads
  the same pages in the same order, so cold cost should be unchanged; a
  single cold run of both walks on a freshly copied database would confirm.
- The `PinnedLayer` file path (one `pread` per row) was not timed; the
  resident layer stood in. One `rank` run through
  `crates/daemon/tests/support/vector_reads.rs` at 20k rows would show
  whether per-row `pread` becomes the next limiter after judgment.
- The page-cache change needs a memory budget decision and a check that
  `PRAGMA cache_size` is accepted by the storage authorizer at open.
