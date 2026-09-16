# Dense retrieval investigation

## Scope and resume state

Started 2026-09-16 at source commit
`8e0491225a7292ef077c675d44b94f94a24041d3`.
The deliverable is research evidence and an isolated reproducer, not a production
optimization. No production source changes, paid compute, or rollout are authorized.
The completed research may be committed, pushed, and submitted as a draft PR.

The original checkout contains unrelated untracked `target-fuzz/` and
`tsconfig.tsbuildinfo`; leave both alone. Its branch is
`rp25/dense-path-optimization`, with stale upstream tracking of
`origin/rp25/vector-admission`. Check the PR base before publishing.

## Measurement contract

The primary outcome is elapsed time for one complete, successful dense library
ranking, from entering the projection read through final eligibility revalidation.
Fixture construction, embedding inference, and index construction are separate.
This is a closed, local operation experiment, not an offered-load service test.
Lower elapsed time is better only when the required correctness check passes.

Start with 20,000 deterministic 384-dimensional f32 vectors, 5% projection
tombstones, k=20, and pages of 1,024. Use real kernel admissions and bundled
SQLite. Record cold-process/connection observations separately from repeated
queries; neither implies cold storage. Compare ordered IDs and exact f64 scores
against the exhaustive oracle and an independent reference over the seeded facts.
Keep ordered f64 products and sums, identifier-byte tie order, and complete
coverage. Missing vectors, authority movement, budget exhaustion, and corruption
must not become successful complete results.

Exploratory comparisons report every sample, medians, means, and ranges. They
do not estimate production p99, statistical significance, fleet effects, or a
release gate. Calls within one process are subsamples, not independent hosts or
builds. Candidate schedules alternate/reverse order and retain failures. A fixed
question and sample schedule precede each run; unexpected results get a new
experiment rather than selective retries.

## Iteration 0: source and evidence audit

**Question.** Does the supplied audit describe this source revision and its
user-visible retrieval path?

**Evidence.** Read root/scoped guidance, `.github/workflows/ci.yml`, dense
oracle/scoring/eligibility code, fixtures, and existing benchmark locations.
A read-only investigator traced callers and authority boundaries. `colgrep`
timed out in two read-only searches; exact searches supplied the remaining
source evidence. The named `/tmp/opencode/probe_dense_sql.py` and `probe_score`
were not present in the inspected scratch directory, so the supplied numbers
remain unverified historical evidence.

**Findings.**

- `dense::oracle::walk` visits live dense-required rows, validates present
  vectors, judges pages, scores eligible rows, and rejudges final winners.
- `daemon::vector_reader::rank` calls layered ranking, but its callers at this
  revision are tests/support. Host `eidnara_search` dispatches to history/notes
  substring search, not dense ranking. A dense win cannot be called a measured
  host-search win.
- Kernel judgment already uses one snapshot transaction and named-ID registry
  query per batch, with per-batch digest and scope memoization. The cost of that
  work is not known from query count alone.
- The page query already uses primary-key keyset traversal without a class
  sort. The projection materializes four text values, an integer revision, a
  blob, and a pending flag, not five text values.
- Projection reads share a read transaction; successive kernel batches only
  compare snapshot/incarnation identities. They do not retain one kernel
  transaction for the whole ranking.
- Score-before-judge is not a drop-in oracle replacement: complete coverage,
  first-seen exclusion counts, judged counts, errors on losing corrupt rows,
  and cancellation/authority-change behavior form part of its contract.
- Existing reuse points: `tests/support/dense.rs`, `dense::score::TopK`,
  `dense::codec`, `eligibility::judge_tracked`, `benches/dense_scalar.rs`,
  `benches/dense_resolve.rs`, and daemon vector-reader fixtures.

**Decision.** Keep the O(N) work hypothesis; reject the claim that dense ranking
is the only wired host retrieval path. Do not begin SIMD or ANN work from these
source observations.

**Next experiment.** Build a test-only probe using the existing real fixture
helpers and public hooks. Measure stock ranking and separate page walk, decode,
judgment, scoring, and final revalidation as closely as the hooks permit. Compare
hooked and unhooked totals to quantify instrumentation overhead. Do not add a
production stub seam solely for timing.

### Probe bring-up

The first three compile attempts failed on the JSON macro recursion limit and
SQLite count types (`usize`, then `u64`, neither implements `FromSql`). The probe
uses `i64` counts and a larger macro recursion limit. Attempt four passed at
N=100, d=384, page=17, admission=50%, one repeated pair. The first stock call
took 14.6 ms versus about 1.7 ms after initialization; first-call setup therefore
needs separate reporting. Exact IDs, score bits, coverage, Hidden exclusions,
and visited-once checks passed. Logs: `/tmp/opencode/dense-smoke-{1,2,3,4}`.
The driver now captures a setup JSON record even when libtest prefixes its line.

## Iteration 1: baseline phase attribution (declared before run)

**Hypothesis.** Kernel page judgments cost more than projection SQL and scoring
at N=20,000. Public hook windows suffice to distinguish this large effect without
a production stub seam. The last vector decode in each page falls inside the
judgment interval; report that limitation rather than subtract an estimate.

**Schedule.** One process at N=20,000, d=384, page=1024, k=20, 100% admission,
5% tombstones; first stock/hooked pair plus five repeated pairs in alternating
order. Pin the process to allowed CPU 16. Use release defaults (no native-ISA
override), bundled SQLite, existing APIs, and an unbounded evaluation budget.
Stop after this schedule or a correctness failure. Treat timings as exploratory
subsamples on one shared KVM host, not independent statistical evidence.

**Success criterion.** All reference checks pass and measured phase totals
identify which mechanism warrants the next intervention. If hooked totals differ
materially from stock, use lower-overhead instrumentation before recommending.

**Command.**

```sh
python3 docs/performance/dense-retrieval/run.py \
  --worktree /tmp/opencode/dense-retrieval-lab \
  --output /tmp/opencode/dense-baseline-20k --cpu 16
```

**Result.** Passed all correctness checks. Stock repeated wall times: median
501.155 ms, mean 501.045 ms, range 491.708–508.960 ms. Hooked median 498.880 ms;
the small difference does not suggest a material instrumentation penalty at
this scale. Mean phase times: first-page SQL/validation 6.775 ms, remaining
SQL/framework 119.404 ms, decode except final row 11.853 ms, judgment plus final
decode 349.626 ms, score/select/cleanup 9.437 ms, final revalidation 0.646 ms.
Phase intervals partition the hooked call. Bundled SQLite is 3.51.3, not the
audit's system SQLite 3.40. Kernel setup took 1.762 s and projection setup 1.861 s,
excluded from query time. First stock call was 524.786 ms after database creation.

**Interpretation.** Kernel page judgment is about 70% of the hooked mean, SQL
about 25%, and numerical decoding/scoring a small remainder. Removing page
judgment entirely while holding other work constant gives an idealized 3.36x
ceiling, not a promised speedup. The audit's direction survives; its absolute
SQL estimate does not transfer to this Rust/bundled-SQLite/real-schema fixture.
Kernel judgment is more than a second narrow SELECT: served-class SQL derives
admission history, visibility, sensitivity, and related facts before registry,
artifact, and scope checks. No evidence yet separates storage reads from CPU
page-cache work.

**Decision.** Keep the opportunity. Do not spend effort on SIMD first. Keep
the oracle's full-population diagnostics intact and investigate a separate
serving execution path rather than silently relabel its completion contract.

## Iteration 2: avoid noncompetitive judgments (declared before run)

**Hypotheses.** (1) Scoring each page first and judging only rows that can beat
an already-full eligible top-k reduces kernel work without unbounded refill
state. The first page remains fully judged; an underfilled heap never prunes.
(2) A bounded global score shortlist with exact continuation can reduce
judgments further, but selective queries may pay repeated scans. (3) A resident
contiguous f32 column removes SQL/vector decode work, at a memory and lifecycle
cost. Measure build cost separately and retain per-query kernel authorization.

**Contract.** Exact ordered eligible top-k and full vector validation on a
stable frozen fixture are required. These prototypes do not claim equivalence
for full-population exclusion counters or all concurrent mutation windows.
Any skipped judgment is an explicit accounting difference, not an oracle pass.
Every final winner is rejudged, and snapshot movement is refused. This is not
an authorized production API change.

**Schedule and stop.** First small correctness smoke, then N=20,000, d=384,
k=20, pages=1024, 100% admission, first observation plus five repeated observations
per path with reversed order on alternating repetitions. All paths use the same
frozen fixture and query. Stop at a mismatch or schedule completion. Follow-up
selectivity/size/query/concurrency tests will be selected from these results.

**Success.** Exact reference IDs and score bits; lower total call time supported
by fewer real kernel candidates. For the resident path, disclose complete build
and resident-byte costs and do not label a frozen snapshot as a live index.

**Result.** N=100 smoke passed, then all 20k ranking comparisons passed. In
`/tmp/opencode/dense-candidates-20k`, repeated medians were stock control 523.736 ms,
page-score-first 159.896 ms, SQL shortlist 121.464 ms, and resident shortlist
8.078 ms. Actual judged rows fell from 19,020 to 1,116 (page) or 84 (shortlist),
including 20 final rejudgments. The shortlist needed one pass in this scenario.
The stock-only label measured 526.693 ms; this control checks gross schedule
asymmetry, not statistical null calibration. The source-compatible oracle has
not changed. This process is slower than iteration 1, so compare interleaved
controls within the same run rather than mixing timings across artifacts.

**Resource attribution.** Linux thread accounting reports about 526 ms CPU
for 526 ms stock wall time (10 ms tick resolution), zero storage `read_bytes`,
about 376 MB `rchar`, and 91,717 read calls per stock control query. Page-score-first
reduces that to about 143 MB/34,860 reads; SQL shortlist to 125 MB/30,542 reads;
resident to about 69 KB/27 reads. Thus these are predominantly CPU/SQLite and
page-cache-copy/syscall costs, not measured disk stalls, queueing, or contention.
`rchar` is not physical DRAM traffic; counters include the small resource-probe
reads and do not isolate allocation cost. No hardware-bandwidth claim follows.

**Resident tradeoff.** Build took 140.693 ms and allocated 52,334,080 bytes of
vector/ID capacity (29,184,000 bytes are live vector values). This is an in-memory
column of frozen live rows, not mmap, a durable bitmap, or an incremental index.
It rejects a changed projection or kernel snapshot and must rebuild. Build plus
one query costs about 149 ms: slower than the SQL shortlist's single query.
Amortization requires multiple queries per unchanged snapshot. No production
memory admission or recovery mechanism is implemented.

**Review findings.** An independent read-only pass identified important limits:
skipped judgments can also skip malformed metadata refusals; shortlist
`max_rows` bounds each pass, not aggregate work; 100% admission never exercises
refill; prefix admission changes kernel history as well as eligibility and
usually assigns matching eligibility to tied vector pairs. The runner also
needs tighter build-input provenance. These do not invalidate the measured
frozen-fixture winners but prevent claims of a drop-in oracle optimization.

**Decision.** Keep page-score-first as the simple bounded-memory, single-pass
candidate. Keep resident layout as a counterfactual with a measured lifecycle
tax. Keep shortlist only pending selective-query tests. No ANN experiment is
justified before removing the measured exact-path overhead.

## Iteration 3: stress refill, scale, and query shape (declared before run)

**Questions.** Does fixed-size refill collapse under low eligibility? Does the
page strategy retain an advantage at a larger corpus and larger k? Are the
winner checks sensitive to ties, underfill, zero eligibility, and corruption?

**Schedule.** Run retained untimed guard cases first. Then three separate
same-process interleaved comparisons: (a) N=20k, admission=1%, k=20, query seed 2,
two repeated observations per path; (b) N=100k, admission=100%, k=20, seed 1,
three repeated observations; (c) N=2k, admission=50%, k=256, seed 3, three repeated
observations. Each has its first observation recorded separately, d=384 and
page=1024. These are named workloads, not a controlled selectivity-only sweep:
fewer admissions also mean fewer admission history records. Stop each schedule
on mismatch or completion; do not extrapolate request-tail latency from it.

**Pass criterion.** Every path returns the independent exact ordered top-k;
refill cases assert multiple passes. Record worst-case total visits rather than
calling the per-pass bound a request bound. Record skipped-metadata refusal as
a semantic difference, not a successful equivalence test.

**Results.** All schedules and retained guard cases passed. At 1% prefix
admission, medians were stock 306.547 ms, page-score-first 224.201 ms, SQL
shortlist 3,191.255 ms, resident shortlist 201.588 ms. Both shortlists made 27
passes and scored 513,000 rows; the page strategy made one pass, scoring 19,000
and judging 7,780. Reject fixed-small-shortlist refill as a general SQL serving
strategy: this workload is over ten times slower than stock despite far fewer
judgments. Even resident refill nearly loses its advantage over single-pass SQL.

At 100k, medians were stock 3,943.783 ms, page 802.750 ms, SQL shortlist
700.423 ms, resident 33.017 ms. Page judged 1,147 of 95,000 live rows plus final
revalidation already included in that count. Resident build was 768.772 ms,
allocated capacity 210,552,320 bytes. At 2k/k=256/50% prefix admission, medians
were stock 21.837 ms, page 19.741 ms, SQL shortlist 12.096 ms, resident 7.691 ms.
The page strategy's benefit shrinks when k is large relative to population.

**Guard evidence.** `guards.rs` forces 64 excluded leading ranks and a
mixed-eligibility tie across ranks 64/65, three-pass refill/exhaustion, an
underfilled page heap, zero eligibility, missing vectors, losing-row NaNs,
external projection changes, and a kernel retirement. Expected panics are
caught and checked. A malformed losing-row digest is deliberately reported
as a difference: stock refuses, SQL shortlist still returns the same winners.
Only trusted valid metadata and frozen fully-covered fixtures support the
candidate ranking comparison. Budget/cancellation and full exclusion/error
equivalence remain unimplemented, not verified.

**Surprise.** Stock grows about 7.5x for 5x as many rows, while page grows
about 5x. Linear visit count is not evidence of a fixed per-row kernel cost.
The measured page-cache traffic and two rotating SQLite readers suggest
working-set/cache effects. Investigate before extrapolating the 20k baseline.

## Iteration 4: SQLite residency counterfactual (declared before run)

**Hypothesis.** A larger kernel SQLite page cache removes much of the increasing
per-row judgment cost, without changing eligibility or ranking semantics. If
true, cache residency is an additional local alternative to skipping judgments.

**Intervention.** In disposable fixture files only, close/reopen the kernel
after setting SQLite's persistent default cache to 65,536 pages (256 MiB per
reader at 4 KiB pages). Verify the value on a fresh connection. This uses a
deprecated fixture-only pragma to reach existing private reader connections;
it is not a recommendation to change database headers in production. Two
readers may each grow to that budget; allocated capacity is not measured by the
pragma. Projection cache settings stay unchanged.

**Schedule.** N=100k, d=384, k=20, 100% admission, CPU16, stock/hooks plus
candidate interleaving, first call and three repeated calls per path. Stop on
failure or completion. Compare with iteration 3 as an exploratory cross-process
contrast; no significance claim. Seek a large drop in stock CPU/read calls and
judgment interval. A small change does not justify the memory budget.

**Result.** All exact ranking checks passed. Stock control median fell from
3,943.783 to 2,311.342 ms, page strategy to 475.445 ms, and SQL shortlist to
376.978 ms. Resident remained about 34 ms. This was not an isolated kernel-only
time reduction: projection intervals and read traffic fell too, eventually to
almost no read syscalls despite unchanged projection settings. The retained
result supports a cache-residency counterfactual, not a causal decomposition of
each connection's cache behavior. The initial hooked rounds were still warming
(mean stock 2,443 ms, control 2,373 ms), so steady-state stability is not established.
Do not recommend a 512 MiB aggregate reader-cache allowance from this run.
Kernel and projection main files were about 434 MB and 426 MB, respectively.
Even this generous-cache scenario leaves substantial judgment CPU cost.

**Additional fidelity correction.** The original helper opens projection with
SQLite defaults; daemon `SearchProjection` pins an 8 MiB page cache and
`temp_store=MEMORY` (`crates/daemon/src/search_projection.rs:23–26,224–230`).
Use those settings for the final file-backed validation instead of describing
the helper's SQL timing as a production connection measurement.

## Iteration 5: final-artifact integration validation (declared before run)

**Question.** Does single-pass pruning still win with the daemon's projection
cache settings and a real file-backed `RowAccess` through public `rank_layers`?
Does sharing the projection connection hide queueing under two callers?

**Scope.** One pinned open file with sorted original f32 rows, one base layer,
same checkpoint, same canonical fixture. Baseline invokes public `rank_layers`;
candidate reuses the measured page-pruning loop with that file source. This
does not exercise daemon acquisition, sidecar verification, ledger reservations,
mutable overlays, inference, or host-wire dispatch. Existing correctness tests
cover those separate source contracts; no end-to-end host latency claim follows.

**Schedule.** After the N=100 integration smoke, freeze/retain probe source and
driver hashes. Run N=20k, d=384, k=20, all admitted, query1, projection cache
8192 KiB, first calls plus three alternating repeated observations per path.
Run three barrier-started two-caller pairs for stock/page/resident on CPUs16,17,
sharing one projection connection; report each response time, including its
connection wait. This is a contention witness, not a service capacity test.
Also repeat the 1%-admission query2 workload with this final artifact and cache
setting (two repeats), and run retained guards plus existing dense correctness
tests. Stop on mismatch or completion. These repeats answer artifact/fidelity
changes, not an attempt to select better timings.

**Provenance.** The driver now requires a detached clean base, rejects ambient
Cargo config/build overrides, records the C compiler and Rust compiler, retains
the exact research source files before building, and records failed compile
attempts. Earlier runs remain exploratory pilots with their original hashes;
their historical source versions are not all retained. Final-artifact results
are the reproducible recommendation evidence. Physical cold-storage behavior
is unmeasured; first-call observations use newly created files and cold reader
connections but a warm OS page cache.

**Result.** Final-artifact comparisons passed. With the 8 MiB projection setting,
SQL stock/page medians were 685.652/348.541 ms, file-backed stock/page were
781.579/435.817 ms, and resident shortlist was 9.359 ms. File-backed baseline
and candidate each read 19,000 rows (29,184,000 bytes) from the same already-open
file and returned identical reference IDs/score bits; the local change removed
17,904 kernel judgments, not file I/O. Two callers shared the projection mutex:
stock response pairs were roughly 0.67/1.35 s; page pairs roughly 0.34/0.68 s.
This confirms serialization and lower wait when service work falls, not parallel
query throughput. No errors, missing responses, or timeouts were omitted.

The final selective workload retained the counterexample: stock/page medians
443.201/383.106 ms, SQL shortlist 8,304.680 ms, resident 214.810 ms with 27 passes.
All six guard scenarios passed, including the explicitly demonstrated metadata
refusal difference. Physical cold storage and concurrent mutation safety are
still outside these experiments.

**Surprise to resolve.** Projection SQL grew after changing helper defaults to
the daemon's 8 MiB/MEMORY settings. Stock/page query costs cannot be transferred
between the two configurations. A small cache-setting isolation experiment
will distinguish configuration effects from artifact/host drift before the
brief quotes either number. Linux perf user cycles/instructions are available
(`perf stat -e '{cycles:u,instructions:u}' -- true`), but stage counters and a
configuration intervention are cheaper first evidence than a sampled profile.

## Iteration 6: isolate projection configuration (declared before run)

**Question.** Is the extra SQL work tied to cache allowance, temp storage, or
uncontrolled run drift? Using the unchanged final Rust artifact, run stock/hooks
at N=20k with (a) defaults, (b) 2000 KiB cache plus MEMORY temp storage, (c) 8192
KiB cache plus MEMORY temp storage. CPU16, first pair plus two alternating
repeated pairs each. Record actual pragma values, SQL phase intervals, and read
counts. Stop after these three runs or a reference failure. This is diagnostic
configuration isolation, not a cache-tuning recommendation or exhaustive sweep.

**Result.** All references passed. Default stock median was 508.974 ms;
explicit 2000 KiB/MEMORY was 721.628 ms; 8192 KiB/MEMORY was 701.865 ms.
Default and explicit-2000 both reported `cache_size=-2000` and `mmap_size=0`,
but SQL phase means were about 129 versus 309 ms. Thus the discrepancy follows
explicit connection reconfiguration rather than monotonically increasing with
the cache allowance; it is not explained by comparing nominal cache sizes.
The experiment does not separate resetting the cache from changing temp storage
or establish SQLite's internal eviction mechanism. Retain the observed
configuration sensitivity and decline to recommend cache tuning. The local
pruning win was reproduced under the target settings and does not depend on
resolving that separate internals question.

## Completion audit

- The decision is **pursue local optimization**, specifically single-pass
  score-first page admission, with explicit serving semantics and an unchanged
  exhaustive oracle. No production implementation is included.
- The final brief distinguishes measurements, conditional headroom models,
  unmeasured hypotheses, and runtime/library boundaries.
- Twelve normalized result sets retain commands, source/binary identities,
  raw samples, counts, and correctness outcomes. Final/config source hashes
  match the checked-in Rust artifacts. Pilot source-retention limits are explicit.
- An independent reviewer recomputed medians, ranges, the 3.36050x/2.06230x
  conditional ceilings, 1.79337x file-backed ratio, 17,904 avoided judgments,
  29,184,000 vector bytes, refill counts, and resident break-even. No material
  remaining accuracy or reproducibility finding was reported.
- Existing dense release tests passed: 13 layered, 35 oracle, 5 properties,
  7 resolver, 16 scalar (76 total). Retained prototype guards passed all six
  scenarios, including eight checked expected refusals and one semantic-gap
  demonstration. These are not a claim that prototype budgets or concurrency
  mutation behavior match the oracle.
- Original tracked files remain unchanged. Only this research directory is
  intended for the commit and draft PR. Unrelated untracked build artifacts
  remain untouched.

**Next work requires approval.** Agree on a serving contract, then implement
and verify through the real pinned reader/resource ledger. Preserve input
validation, coverage, cancellation, and authority fencing; keep exhaustive
diagnostics in the oracle. No further experiment is required to choose the
local direction, and no production-readiness claim is made.
