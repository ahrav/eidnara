# Lexical probe cost investigation

## Decision brief

**Pursue local optimization, not a retrieval-policy redesign.** Shortlist by
`rank, occurrence_id` before joining occurrence metadata, but do not ship the
literal rewrite until its valid-projection precondition or an exact fallback is
enforced. The corruption counterexample is real. This PR contains research only.

The opportunity reproduces on bundled SQLite 3.51.3. A common atom visits every
posting; lowering LIMIT does not bound scoring or candidate lookup work. But the
audit's explanation needs two corrections: the outer tree retains top-K rows,
not every matched row, and the occurrence-ID tie-break prevents FTS5's native
rank-order path. The late-join rewrite does **not** remove the temporary tree.

| Experiment | Change | Observed result | Correctness / quality | Decision |
|---|---|---|---|---|
| Baseline reproduction | Real batches, 20,004 common matches, LIMIT 65 | 59.599 ms median; 381,619 VM steps | Existing retrieval contract | Keep evidence |
| Local late join | LIMIT before occurrence join | 49.854 ms; 141,993 steps; same outer sort count | Exact rows on valid fixtures; underfills with stale tombstoned FTS row | Pursue with precondition/fallback |
| Rank-only control | Drop ID tie-break | 17.297 ms | Changes cutoff membership | Reject as replacement |
| Rank-ordered stream | Rank prefix, ID block sort | Diverse: 59.633 → 46.368 ms; all-tied: 30.033 → 89.585 ms | Exact, including raw tombstones | Reject as default |
| Scale sensitivity | 2,004 versus 20,004 common matches | Baseline 5.836 versus about 59 ms at LIMIT 65 | Same valid-fixture checks | Linear work risk supported at tested scales |
| DF-based skipping | Omit atoms above 10% document frequency | 2k corpus: common 0.113 ms; mixed 0.348 ms | Retains 0/1 and 1/2 reference contributions | Reject without explicit quality change |
| Full retrieval, verified ABBA | Baseline versus late-join artifacts, 20k diverse corpus | Common process medians 61.528–62.342 → 51.046–53.555 ms | All normalized results equal, including counters and ordinals | Component win reproduced |

Times are exploratory, single-host release-build observations. Each process has
eight repetitions per case. The ABBA sequence has two processes per treatment,
one binary per treatment, and no fleet, build-layout, or inferential replication.
The candidate/baseline ratio of process mean common-retrieval times is 0.844;
mixed `parse fetch io` is 0.847. Four simultaneous common requests take
243.151–248.142 ms baseline versus 202.632–213.715 ms candidate (process medians
of batch makespan). That is connection serialization evidence, not open-loop
capacity. Small-fixture and rare-query retrieval shows no win: roughly 0.34–0.54
ms, with candidate mean ratios around 1.01–1.05. Those costs must not be hidden
inside a common-query average.

### Mechanism and plausible headroom

In the verified baseline run, common LIMIT-65 SQL costs 58.905 ms. FTS-only with
the same rank-plus-ID comparator costs 49.084 ms: about 83% remains after removing
occurrence joins. Late join takes 49.340 ms, capturing nearly all of that measured
join-elimination opportunity. Expect an order of 15–20% component improvement on
this diverse/common workload, not 3x. The all-tied fixture shows a larger local
benefit, 30.033 → 20.311 ms, but it is a different corpus, not a universal gain.

Common baseline calls average about 10,882 read syscalls and 44.6 MB of logical
read accounting; late join still has about 10,436 and 42.7 MB. Physical read-byte
accounting is zero during these samples. Together with CPU ticks and bytecode,
this attributes the observed cost to posting/scoring, FTS content access for the
unindexed occurrence ID, cached page traffic, comparisons, and lookups, not disk
waits or contention in the single-reader SQL samples. Allocation and hardware
memory-bandwidth fractions were not measured. They are not needed to distinguish
the tested query shapes.

The rank-only control's roughly 17 ms shows potential beyond the join, but it is
**not** an attainable exact-search floor or theoretical maximum. The bundled
`libsqlite3-sys 0.37.0` source sets rank-order consumption only for one ORDER BY
term (`fts5BestIndexMethod`, `sqlite3.c:257485`). Its
`fts5CursorFirstSorted` builds an internal rank-sorting statement without LIMIT
(`sqlite3.c:257861`), so a tiny outer VM-step count does not imply skipped scoring.
Avoiding that work would need another exact index/execution design or an explicit
quality change. No such redesign is justified by these fixtures alone.

### Tradeoffs and strongest contrary evidence

- Late join preserves the analyzed corpus, rank arithmetic, ID ties, and kernel
  authorization on valid projections. It adds no index, persistent bytes, or
  write/recovery path. The 20k diverse database is 68.7 MB logically, of which
  46.0 MB is lexical shadow tables; both SQL treatments use the same layout.
- The shortcut is **not** equivalent over every database accepted by the public
  retrieval function. Raw tombstoned/orphan lexical rows can consume shortlist
  slots. Verification on open plus controlled mutation may establish the needed
  invariant, but retrieval has no typed proof of that property. Keep baseline
  behavior when that guarantee is unavailable. Do not re-run full `verify_rows`
  per query; it reads and analyzes the whole projection.
- The rank stream preserves raw-tombstone semantics, yet loses almost 3x on ties.
  Metadata loads move ahead of its competitive-row check. Its native FTS sorter
  also introduces hidden work not counted in the outer statement's sort metric.
- Skipping common probes drops valid accepted results and changes completion and
  ordinal metadata. Rarest-first **without** skipping cannot remove the scans;
  stopping early has the same unresolved contribution-loss and rank-reduction
  problem. No production qrels or query distribution supports that tradeoff.
- Cache reset requests preserve the local win (baseline 58.536–59.551 ms versus
  late join 48.599–49.563 ms for common LIMIT 65), but all are OS-warm. No claim
  covers disk-cold startup, a writer racing reads, or daemon latency.

### Before production implementation

1. Decide how retrieval enforces a verified live projection, or prototype a
   fallback that restores baseline results on shortlist underfill/corruption.
   Keep the raw-tombstone counterexample; add orphan and duplicate/collision
   witnesses. Authorization must still happen after candidate collection.
2. Add exact result identity across the existing retrieval fixtures, real
   tombstone/rebuild paths, ties at the cutoff, and progress-handler interruption.
   Re-run the component comparison after implementing the chosen fallback or
   precondition; the literal scratch rewrite is not a green production patch.
3. Before claiming product impact, wire the caller and measure an observed query
   mix, positive-admission large corpus, cold startup, and writer/read concurrency.
   A schema/index change or DF-based policy would need separate lifecycle and
   quality evidence and the appropriate contract/identity update.

## Reproduction and retained evidence

From the repository root:

```sh
# Correctness only; the timing experiment is ignored by default.
cargo test --locked -p retrieval --test lexical_retrieval

# Each output directory must be new and outside the repository.
python3 scripts/research-lexical-probe.py --output /tmp/opencode/lexical-new-a \
  --rows 20000 --reps 8
python3 scripts/research-lexical-probe.py --output /tmp/opencode/lexical-new-b \
  --variant late_join --rows 20000 --reps 8

# Adversarial tie corpus and smaller scale.
python3 scripts/research-lexical-probe.py --output /tmp/opencode/lexical-new-ties \
  --corpus tied --rows 20000 --reps 8
python3 scripts/research-lexical-probe.py --output /tmp/opencode/lexical-new-small \
  --rows 2000 --reps 8
```

For the four-process comparison, run A, B, B, A with distinct output directories.
`--variant rank_stream` builds the rejected stream prototype.
`--variant late_join --check` intentionally fails the raw-tombstone witness before
timing; that negative result is expected and must not be interpreted as approval.

The runner snapshots committed production source and overlays only the research
tests. It does not benchmark uncommitted production edits. It records Cargo
commands, source/lock/test hashes, binary hashes, CPU/build settings, SQL source
identity, plans, bytecode, raw timings, and exact results. SQL alternatives and
the DF policy live only in the test module. Candidate production SQL is replaced
only inside a disposable snapshot. Binaries remain in each external run directory.

[Retained evidence](lexical-probe-evidence/evidence.json.gz) contains all 13 run
records, manifests, raw observations, and the intentional failed test output.
[Summary](lexical-probe-evidence/summary.json) marks invalid/excluded experiments
and reports 2,544 exact normalized retrieval comparisons across valid runs.
[Checksums](lexical-probe-evidence/checksums.json) identify both artifacts.
Read the gzip as a JSON array with Python's standard `gzip` and `json` modules.
Use the runner's `--retain RUN... --output DIRECTORY` mode to produce a new bundle;
`--exclude-run NAME` retains but excludes invalid comparison arms. Earlier harness
versions are identified by hashes and their local binaries, not reconstructed
byte-for-byte by the final runner. The final protocol is reproducible from this PR.

The chronological research log below records rejected directions and the cache
contamination incident. No invalid run contributes to the recommendation.

## Scope and status

This investigation tests the claim that joined lexical probes spend work in
proportion to matching postings rather than `scan_rows`. It compares exact SQL
rewrites and explicitly non-equivalent controls. Production code is unchanged.
The decision and all measurements are exploratory, not a release gate.

Source baseline: `8e0491225a7292ef077c675d44b94f94a24041d3`.
Source references below refer to the checkout at investigation start.

## Discovery

- `crates/retrieval/src/lexical/retrieve.rs:142` owns `PROBE_SQL`. It joins
  occurrences and excludes tombstones before ordering by rank and occurrence ID.
  `scan` requests `scan_rows + 1` to detect truncation. All probes run before
  reduction, eligibility admission, and final revalidation.
- No production caller of `lexical::retrieve` was found. The daemon search
  facade calls `memory_tool::search_history_segments_and_notes_for_session`.
  Results here cannot establish a daemon speedup.
- `docs/lexical-analysis-contract.md:281-335` defines the comparator, eligibility,
  completion, budget, and returned-row bounds. It makes no visited-row or hard
  latency promise.
- `lexical/index.rs::verify_rows` checks live-row correspondence and analyzed
  payloads. `tombstone_occurrence` removes lexical rows. The FTS table has no
  foreign key or occurrence-ID uniqueness constraint of its own.
- Existing `lexical_retrieval.rs` tests provide real projection batches and kernel
  admission. Its raw-tombstone test intentionally violates the index invariant.
  Moving LIMIT ahead of the tombstone check can underfill in that state.
- Existing lexical benches measure analysis, indexing, and verification, not
  retrieval. Existing exact-query tests use EQP and statement counters. The
  claim-validation benchmark runner is specialized to another workload and its
  inferential policy; it is not reused as a lexical release gate.
- The supplied `/tmp/opencode/probe_lexical.py` is absent. Supplied Python numbers
  are leads, not reproduced evidence. The linked rusqlite engine is authoritative.

## Measurement contract

Primary outcome: completed probe wall time, lower is better. Boundary: cached
prepare, bind, step, and decode all returned columns. Fixture setup, query
compilation, EQP, printing, and assertions are outside timing. SQL-only samples
use a separate raw connection; its pragmas and guarded connection pragmas are
recorded. The end-to-end boundary includes storage acquisition/read transaction,
all probes, reduction, kernel eligibility, and revalidation, but not payload
loading, fusion, daemon handling, or rendering.

Correctness requires identical ordered six-column SQL rows, including rank bits,
and identical normalized retrieval results. Snapshot/incarnation identifiers are
fixture-local and excluded from cross-process equality. Exact candidates must
preserve occurrence-ID ties, probe ordinals, exclusions, and completion. Rank-only
FTS is a diagnostic control, not an acceptable implementation.

Workload: six existing admitted fixture rows plus synthetic canonical-claim
occurrences projected through `apply_batch`. Synthetic objects have no kernel
admission. Text varies in length, includes a common atom, a 1% atom, one unique
atom, one engine-split two-token phrase, and a tie-heavy group. This is a stress
fixture, not a sampled production query distribution. Limits are 2, 65, and 1025.
The small end-to-end fixture checks positive admission; the large fixture also
exercises rejection of synthetic candidates. Four synchronized closed readers
exercise the shared projection connection, not an open-loop service capacity test.

Warm means one unmeasured execution after the reset-cache diagnostic; raw samples
are retained to inspect drift rather than assuming steady state. Reset-cache means
statement-cache flush and `PRAGMA shrink_memory`, with the OS cache left warm.
It is not a disk-cold measurement. `/proc/self/stat` CPU ticks are coarse,
process-wide diagnostics. Statement VM steps omit work hidden inside FTS calls.

## Predeclared first experiment

Hypothesis: late materialization of occurrence metadata saves per-match joins,
while the occurrence-ID tie-break may still require SQLite sorting. A missing
temporary B-tree is not required for a useful win.

Treatments: actual baseline SQL, exact rank-plus-ID LIMIT subquery followed by
join (`late_join`), FTS-only rank-plus-ID, and non-equivalent FTS-only rank.
One SQL statement execution receives a treatment. Eight warm executions per
case/treatment alternate forward and reverse order. One cache-reset sample per
case/treatment is reported separately. Samples within this one process are not
independent host/build replications; report median, mean, range, and raw values,
without confidence intervals or significance claims.

Start with 20,000 synthetic rows. Success: exact valid-fixture equality and a
repeatable common-term cost reduction consistent with work counters/plans.
Stop after the fixed eight rounds, every case, both end-to-end fixtures, and
concurrent smoke complete; stop immediately on correctness or execution failure.
Any repeat needs a new question. Follow-up scale and separate-artifact runs are
chosen only after this result, with their protocols recorded before execution.

## Research log

### Iteration 0 — discovery and correctness harness

Added a child module under the existing lexical retrieval test, reusing its
private fixture rather than duplicating setup. Five research correctness tests
pass: original fixture equality, diverse projected rows, real tombstone mutation,
raw-tombstone underfill, and rank-only tie-order counterexample. Full target:
21 passed, one ignored. Targeted Clippy, release compilation, formatting, comment
markers, and diff whitespace checks passed. Timing has not run.

Harness: `crates/retrieval/tests/support/lexical_probe_research.rs`.
Command: `cargo test --locked --release -p retrieval --test lexical_retrieval
lexical_probe_research::probe_cost_experiment -- --exact --ignored --nocapture
--test-threads=1`. Defaults: `LEXICAL_RESEARCH_ROWS=20000`,
`LEXICAL_RESEARCH_REPS=8`; label/variant environment variables annotate output.

Open questions: linked engine plan; cost split; whether a valid local candidate
survives full retrieval; whether early probe skipping can preserve enough quality
to justify a contract change. Next: execute the first protocol and inspect plans,
counters, exact results, and timing.

### Iteration 1 — baseline reproduced, initial explanation narrowed

Command: `python3 scripts/research-lexical-probe.py --output
/tmp/opencode/lexical-probe-e1 --rows 20000 --reps 8`.
The runner archives committed source, overlays the two research test files, builds
a release test binary, and retains hashes, plans, raw samples, and summaries.

SQLite is 3.51.3, source ID
`2026-03-13 10:38:09 737ae4a34738ffa0c3ff7f9bb18df914dd1cad163f28fd6b6e114a344fe6d618`.
Host: AMD EPYC 9R14 under KVM. Default SQLite cache is 2,000 KiB, WAL, mmap off.
At 20,004 common matches and LIMIT 65, median baseline is 59.599 ms
[59.262, 59.762], late join 49.854 ms [49.640, 50.161], rank-plus-ID FTS
49.288 ms, and non-equivalent rank-only FTS 17.297 ms. Baseline/late-join VM
steps are 381,619/141,993. Both still sort once. Rank-only uses virtual index
`32:M3`; the exact variants use `0:M3`. The proposed temporary-tree-disappearance
test is therefore refuted as the criterion for the supplied rewrite.

Late join saves 16.4% here, not the supplied 1.6x speedup. Most remaining cost is
already in FTS rank-plus-ID enumeration, not the occurrence join. The larger
text fixture and actual SQLite build differ from the absent Python fixture.
LIMIT 2/65/1025 baseline medians are 59.067/59.599/70.241 ms. Unique and phrase
probes at LIMIT 65 take 0.081/0.112 ms. Exact six-column equality passes on all
valid SQL cases. Rank-only changes selected IDs at LIMIT 65 and is rejected.

Baseline full retrieval: common atom 62.120 ms, mixed three probes 62.967 ms;
four simultaneous common requests take 250.066 ms total through the shared
store. One common-query result is accepted and 63 synthetic candidates excluded.
Small fixture common retrieval is 0.408 ms. These are component boundaries, not
daemon latency measurements.

Next hypothesis: force an FTS rank-ordered stream behind a non-flattenable
subquery, but keep the live filters and final rank-plus-ID LIMIT outside it.
SQLite may exploit the ordered rank prefix and sort only tie groups. This would
retain raw-tombstone behavior and avoid reading metadata for worse-ranked groups.
Before running, add that exact candidate and bytecode evidence. Success requires
all exact rows to match, including tombstones; failure to reduce work rejects it.
Fixed schedule: the same eight counterbalanced rounds at 20,000 rows. This tests
a new execution shape, not a repeat seeking a favorable timing.

### Iteration 2 — rank-prefix execution works, with limits

Command: `python3 scripts/research-lexical-probe.py --output
/tmp/opencode/lexical-probe-e2 --rows 20000 --reps 8`.
The `rank_stream` candidate uses an inner `ORDER BY rank LIMIT -1 OFFSET 0`,
keeps live filters outside, and applies final rank-plus-ID ordering and LIMIT.
EQP confirms `32:M3` and `USE TEMP B-TREE FOR LAST TERM OF ORDER BY`.
Median common LIMIT-65 cost: baseline 59.633 ms, late join 49.763 ms,
rank stream 46.368 ms. All exact SQL results match. The stream also passes the
raw-tombstone limit witness that defeats late join. Rank-only selection changes
membership at cutoff 3 in the valid tie fixture.

Bytecode corrects the audit's full-sort interpretation. Baseline `OpenEphemeral`,
`IfNotZero`, `Last`, `IdxLE`, and `Delete` retain only competitive top-K rows.
All matching rows are visited, but metadata loads follow the competitive-row
check and the temporary tree is bounded by LIMIT. Rank stream uses the rank
prefix to finish ID tie groups before stopping. It still cannot avoid FTS scoring
all matches. Aggregate baseline common-query CPU ticks account for 1.51 s of
1.510 s wall time; this points to CPU work, including cached-I/O kernel work,
rather than waiting for disk or another request. Ticks are too coarse for small
individual statements.

Next protocol: compare the same SQL variants at 2,000 rows to test posting-length
sensitivity; compare at 20,000 rows with identical synthetic text to expose the
all-tied case. Retain eight rounds, without significance testing. Add `/proc`
I/O counter deltas to distinguish cached reads from physical reads. Run baseline
and rank-stream full retrieval in separate isolated artifacts, in baseline,
candidate, candidate, baseline order at 20,000 diverse rows; report process
medians and ranges, not an inferential claim. Run the ordinary lexical tests
against the candidate artifact as well.

Materially different prototype: omit simple one-token probes whose fts5vocab
document count exceeds 10% of the corpus. Measure `parse` and `parse io` against
the unchanged retrieval reference, including DF lookup cost and exact retained
contribution recall. This threshold is a stress-test policy, not a proposed
default. Any missing reference contribution rejects it as a transparent
optimization. It must not change production compilation, index, or retrieval.
These fixed experiments stop on correctness errors. All-tied regressions and
quality losses remain evidence rather than being averaged away.

### Iteration 3 — reject rank stream as a general replacement

Commands: the runner with `--rows 2000 --reps 8 --output
/tmp/opencode/lexical-probe-e3-scale`, then `--rows 20000 --reps 8 --corpus tied
--output /tmp/opencode/lexical-probe-e3-ties`.

The diverse 2,004-match common probe at LIMIT 65 costs 5.836 ms baseline,
4.842 ms late join, 3.986 ms rank stream. Increasing matches roughly tenfold
increases baseline time roughly tenfold. This supports posting-length scaling
within these tested sizes, not an extrapolated universal slope.

The all-tied stress case reverses the rank-stream win: baseline 30.033 ms,
late join 20.311 ms, rank stream 89.585 ms. Rank stream performs 700,611 VM steps
and approximately 17,524 read syscalls per query, versus 381,101 and 1,172 for
baseline. Physical `read_bytes` stays zero. Bytecode places metadata `Column`
loads before rank stream's competitive-row test; baseline defers them until after
that test. Full ties defeat early group termination and expose extra cached reads.
This is contrary evidence strong enough to discard rank stream as the default.
The planned rank-stream end-to-end comparison is replaced by late join; there is
no reason to spend more timing runs validating a candidate already rejected by
its worst case.

The DF-skip prototype removes real accepted contributions. On the 2,000-row
diverse corpus, `parse` falls from one reference contribution to zero (0.113 ms);
`parse io` retains one of two (0.348 ms). On the six-row fixture the same 10%
threshold skips both terms and loses all five `parse io` contributions. Rejected
as a semantics-preserving optimization. It requires a separate quality/contract
decision, not a golden-file refresh used to hide the loss.

Late join remains the simplest surviving valid-projection candidate. Next:
full lexical tests on a scratch late-join artifact (expect the intentional
raw-tombstone witness to expose underfill), followed by the predeclared
baseline/candidate/candidate/baseline component timing comparison. Timing on valid
projections does not waive the corruption-policy decision.

### Iteration 4a — scratch candidate correctness boundary confirmed

Command: runner with `--output /tmp/opencode/lexical-probe-e4-candidate-check
--variant late_join --check --rows 20000 --reps 8`.
The scratch build passes 21 tests and fails exactly the raw-tombstone limit
witness: one row instead of two. The experiment aborts before timing, as designed.
Existing admission, revalidation, interruption, budget, no-payload-read, duplicate,
and completion tests pass. Neither the witness nor a guard is removed or changed.
This is retained negative evidence, not a clean production-candidate verdict.
The following timing-only runs answer the separate valid-projection cost question.

### Iteration 4b — artifact mismatch invalidates the component comparison

The planned `lexical-probe-e4-a1`, `e4-b1`, `e4-b2`, `e4-a2` runs completed,
but baseline records show late-join SQL hash and 141,993 rather than 381,619 VM
steps. Their archived source hashes differ while binaries coincide: Cargo reused
a candidate artifact across relocated scratch trees sharing the same target
directory and preserved source timestamps. Labels alone did not prove treatment
identity. This entire ABBA comparison is invalid; none of its timing supports the
recommendation. Raw artifacts and manifests remain retained.

Repair: use a separate target cache per SQL treatment and assert the intended
source SQL SHA-256 inside the test before fixture creation. Verify the emitted
hash again in the runner. Repeat the same fixed ABBA protocol as iteration 5,
without changing corpus, SQL, or repetitions. Earlier iterations 1–3 emitted the
correct baseline SQL hash and measured SQL alternatives in the same binary;
their within-process comparisons are unaffected. An additional A/A control uses
the two identical SQL strings in a candidate-labeled process to inspect label
asymmetry; it is a mechanical diagnostic, not null calibration or a noise floor.

### Iteration 5 — valid component comparison and stopping decision

Ran the same four commands as iteration 4b with output names `e5-a1`, `e5-b1`,
`e5-b2`, `e5-a2`, after isolating target caches and enabling the SQL-hash assertion.
All emitted SQL hashes match intended treatments. Baseline binary:
`5fab42fd6bd8be00cf771cabca5b240a1f0e67274146a6e09f264e55ef54eac8`.
Candidate binary:
`0efecc1ae16b66c9a4ff1c1a4ccd7f123734cb0cc84803a0cc7c0ba16a0e6875`.
Each treatment's two processes use the same binary. Every normalized retrieval
result matches across the four runs, including ranks, order, ordinals, exclusions,
completion, and consumed-work counters. The local component win survives; numbers
and limitations are in the decision brief.

Within the two candidate processes, SQL labeled `baseline` and `late_join` is
identical. Common LIMIT-65 means are 50.054/50.181 ms and 47.744/47.669 ms.
This A/A label diagnostic shows no material asymmetry at the scale of the observed
join savings; it is not an independently calibrated significance procedure.
The corrected process comparison is one descriptive ABBA block, not release-grade
statistical evidence. It shows about 15–16% lower common/mixed component time,
while tiny/rare queries show slight overhead.

Stopping condition met: a simple, conditional local optimization has reproducible
headroom; the tested alternatives either regress under ties or lose accepted
contributions. Further broad sweeps would not resolve the missing production
invariant/quality decision. No simulation is needed for a directly measurable
local query. No hardware-counter campaign is needed to choose among these query
shapes. Production implementation remains gated on the concrete validation above.

### Final publication checks

- `cargo fmt --all -- --check`: passed.
- `cargo test --locked -p retrieval --all-features`: 272 tests passed, one
  research timing test ignored; doctests passed with no examples.
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`:
  passed.
- `scripts/forbid-comment-markers.sh` and `git diff --check`: passed.
- The retained baseline executable rejects an intentionally wrong SQL hash with
  exit 101 before emitting a protocol record or creating the fixture.
- An independent read-only review recalculated the retained summaries and exact
  comparisons, checked artifact checksums, binary hashes, pinned SQLite source,
  and invalid-run exclusions, and found no research-publication blocker.

Production source, schema, dependencies, and contract epochs are unchanged. The
full workspace test/native-addon/platform matrix is left to CI; local test
execution is scoped to retrieval. Publishing these findings does not approve
the conditional SQL rewrite for production.
