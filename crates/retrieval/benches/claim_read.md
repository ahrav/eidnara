# Local claim-read workload

For resumable fixed-schedule collection, see
[`scripts/bench_claim_read.md`](../../../scripts/bench_claim_read.md).
The fixture installs the current kernel identity and a projection checkpoint,
and passes an explicit `EvalBudget` through classification.

Run from the repository root:

```sh
# CI-sized fixtures, all 20 benchmark paths, no timing.
cargo test -p retrieval --bench claim_read --locked

# Full fixtures, one correctness-checked operation per benchmark, no timing.
cargo bench -p retrieval --bench claim_read --locked -- --test

# Descriptive baseline, including raw Criterion samples.
CRITERION_HOME=/tmp/claim-read-baseline \
  cargo bench -p retrieval --bench claim_read --locked -- --save-baseline before

# After a candidate change, reuse the same Criterion home to compare.
CRITERION_HOME=/tmp/claim-read-baseline \
  cargo bench -p retrieval --bench claim_read --locked -- --baseline before
```

Keep the baseline directory outside the repository. Preserve its contents before
running a candidate. Do not overwrite a baseline with a different fixture,
bound, compiler, feature set, or warmup configuration.

## Claim and timing contract, version 1

Measure elapsed time for a synchronous claim-read operation on a local SQLite
projection and canonical kernel store. Lower operation time is better. This is a
single-thread, completion-coupled microbenchmark, not a service arrival model or
capacity test. No network, paid resources, production data, or privileged tuning.

Three stages run against each fixture:

- `projection`: `SqliteStore::with_conn` plus `live_claim_candidates`, including
  projection connection acquisition, read transaction, SQL, row decoding, and
  transaction release.
- `facts`: `claim_facts_as_of` for the known distinct objects at a fixed tip,
  including canonical reader acquisition and the complete facts read. The input
  IDs are already deduplicated. This is a diagnostic stage, not the full operation.
- `classify`: `SqliteStore::with_conn` plus `classify_live_claims`, including both
  databases, tip selection, deduplication, and classification. Returned row and
  fact counts are asserted inside each iteration.

Criterion's ordinary `iter` loop includes dropping the returned values. Database
creation, commits, artifact ingestion, descriptor publication, and correctness
canaries run outside timing. Separate stage estimates are not additive and do
not establish causal attribution by subtraction. Stores remain open across
iterations; all data is read during the canary, then warmed for one second.
This measures within-process warmed reads, not startup or cold storage. There is
no concurrent writer, contention, cache eviction, or filesystem reset.

Default settings: Criterion 0.8.2, 20 samples, one-second warmup, two-second
measurement target per benchmark. Default confidence level 95%, significance
0.05, noise threshold 1%, automatic sampling. Criterion may extend collection;
record realized samples and iteration counts. CLI overrides must match between
compared runs. No trimming, timeout replacement, retries-until-favorable, or
selection of only the fastest run.

## Fixture matrix

Every live object has an independent source lineage, one admission unless noted,
a verified DirectObservation record, and three published representations:
`canonical_claims/decision_summary`, `canonical_claims/rationale`, and
`promoted_memory/summary`. Both claim classes are exercised. All live objects are
Current. Artifacts contain repetitive synthetic text; this is not a production
text distribution.

| Fixture | Objects | Live rows | Admissions/object | Retained tombstones | Bytes/representation |
| --- | ---: | ---: | ---: | ---: | ---: |
| empty | 0 | 0 | 0 | 0 | 0 |
| small | 16 | 48 | 1 | 0 | 256 |
| large | 256 | 768 | 1 | 0 | 256 |
| admission_history | 16 | 48 | 8 | 0 | 256 |
| tombstone_history | 16 | 48 | 1 | 4096 | 256 |
| large_payload | 16 | 48 | 1 | 0 | 16384 |

Row and fact bounds equal the expected live cardinalities, or one for empty
fixtures. Two additional benchmarks use the large fixture: `row_overflow` sets
max_rows=1 and requires TooManyRecords(count=2); `object_overflow` sets
max_claims=1 and requires TooManyClaims. Refusal timings stay separate from
successful read timings. Total: 20 benchmark IDs.

`cargo test --bench claim_read` limits each nonempty fixture to two objects and
each retained-history fixture to eight tombstones. It still exercises history,
large payloads, both classes, all representations, and both refusal branches.
`cargo bench --bench claim_read -- --test` runs full cardinalities once without
collecting timings. Fixture metadata is printed to stderr for every run.

The harness reuses `crates/kernel/tests/claim_fixture/mod.rs` for canonical
writes, retained evidence, and descriptor publication. Projection rows use
`persist_occurrences` and the production exact-key extractor; association rows
are inserted directly in the fixture. Canonical descriptor IDs must match
projection occurrence IDs. Canaries require complete returned facts, all three
descriptors, correct candidate-to-fact mapping, Current states, and verified
DirectObservation. A fast empty/missing-facts path cannot pass.

Retained tombstones are synthetic projection-only history with no corresponding
live kernel objects. They isolate projection scan cost at fixed live cardinality.
The fixture does not model retirement publication, replay, checkpointing,
embedding work, crash recovery, or projection completeness certification.
Those mechanisms remain covered by existing daemon tests, not this benchmark.

## Evidence and next comparison

The initial baseline is exploratory and descriptive. It establishes that local
fixtures execute and identifies stage/scale hypotheses. A single Criterion run,
its inner confidence intervals, or its labels cannot justify a merge/release
verdict, production speedup, or independent-run confidence bound. No p99 estimate
is reported from batched Criterion samples.

For every run retain the command, source revision and dirty diff, fixture and
Cargo.lock hashes, toolchain, features/flags, benchmark executable hash, hardware,
CPU affinity and competing-work caveats, stdout/stderr, exit status, and complete
Criterion output tree. Never benchmark a source tree while another writer can
change it: freeze a copy or verify identities before and after collection.

Before keeping a performance change:

1. Select one mechanism based on the baseline plus a discriminating artifact,
   such as bundled-SQLite query plan/work counters or a CPU profile. Preserve
   existing output, error, canonical snapshot, and eligibility contracts.
2. Freeze the exact baseline/candidate artifacts, workload IDs, timing settings,
   and local target population. Run correctness checks on both.
3. Predeclare process/block assignment, A/A control, replication, stopping,
   invalid-attempt treatment, analysis units, uncertainty construction, and
   multiplicity. Inner Criterion samples are not independent process/build/host
   replicates. Use fresh collection, not retrofitted inference on pilot samples.
4. Run the fixed schedule serially with retained failures and full raw output.
   Compare each workload separately; do not add stage times or average tail
   percentiles. Apply repository policy only if a separately authored matching
   policy exists.

This workload does not cover reader/writer contention, scheduler/cgroup stalls,
adversarial association multiplicity, derived causal graphs, lagging/hidden
claims, or production history distributions. Add those cases only for a selected
mechanism that needs them. Schema/index changes also require the existing
projection version/rebuild rules and write-cost measurements.
