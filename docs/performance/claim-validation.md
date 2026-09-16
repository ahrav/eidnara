# Materialized claim-validation workload

The workload runs the real `ClaimMaterializer`, reads its canonical occurrence
inventory, and exercises final validation with same-snapshot claim facts and
incarnation fencing. It needs Linux and Rust 1.98, not an external service.

```sh
# Normal allocator, two-pass timing. Default is a two-sample smoke.
EIDNARA_CLAIM_BENCH_SAMPLES=32 cargo bench -p daemon \
  --bench claim_validation --features test-support --locked

# Requested Rust allocation bytes, not timing.
cargo bench -p daemon --bench claim_validation \
  --features test-support,bench-internals --locked

# Optional single-case selection.
EIDNARA_CLAIM_BENCH_CASE=duplicates1024 cargo bench -p daemon \
  --bench claim_validation --features test-support --locked
```

Each operation validates all rows, copies the first 16 permitted survivors,
revalidates them under the same 30-second budget and drops both outputs.
Timing excludes fixture publication, classification, warmup, assertions and
JSON output. It includes survivor copying and output destruction. Projection
extraction, ranking, transport and provider calls are not measured.

| Case | Objects / rows | Timing workers | Destination |
|---|---:|---:|---|
| single | 1 / 3 | 1 | Local |
| mixed64 | 16 / 64 | 1 | Local |
| duplicates1024 | 16 / 1024 | 1 | Local |
| distinct256 | 256 / 256 | 1 | Local |
| remote64 | 16 / 64 | 1 | Remote |
| concurrent64 | 16 / 64 | 4 | Local |

Fixtures include Automatic and Labeled admissions, foreign scope, quarantine
before and after classification, canonical/promoted representations, duplicate
rows, shared/distinct digests and LocalOnly artifact denial. All lineage is
Unknown. Canaries check actual materialized occurrences, row identity,
permissions, accounting, survivors, returned facts and incarnation. Existing
claim-source tests cover other transitions and split verdicts for one object.
This is a generated integration-shaped workload, not sampled production traffic.

Allocation mode records one owning-thread operation with outputs retained at
window close, even for the `concurrent64` label. It reports Rust allocation and
reallocation requests, not SQLite C allocation, allocator rounding, RSS or other
threads. `distinct256` overflows the shared event inventory: its event count is
`null`, with `ledger_overflow=true`. The recorder continues updating requested
bytes and peak live layout-byte delta; those counters remain available. No
truncated event count is reported as a complete count.

## Verified resource changes

The patch collects compact surface verdicts in an exact-sized vector instead
of retaining full egress facts beside every verdict. Accounting copies an ID
only when its set gains a member. Canonical reads, classification, transaction
boundaries, budgets, order, duplicates and overlapping sets remain intact.

These allocation-contract results were verified against reviewed head
`756785442dab13c5acba10f5c2aa20773ed8e5cb`, including its per-claim cancellation
poll. Four fresh processes per arm produced identical counters. No case
increased requested bytes or peak delta.

| Case | Requested bytes, baseline → candidate | Peak live-byte delta, baseline → candidate |
|---|---:|---:|
| single | 587,986 → 585,794 | 94,367 → 94,367 |
| mixed64 | 3,302,214 → 3,261,006 | 170,712 → 170,712 |
| duplicates1024 | 5,023,274 → 4,455,986 | 730,638 → 497,582 |
| distinct256 | 30,085,582 → 29,944,078 | 973,660 → 973,660 |
| remote64 | 2,102,598 → 2,068,902 | 155,654 → 155,654 |
| concurrent64, serial allocation window | 3,302,214 → 3,261,006 | 170,712 → 170,712 |

For duplicate-heavy validation, requested bytes fell **11.29%**, peak delta
**31.90%**, and allocation events fell from **19,893 to 17,853**. These are
requested Rust layout-byte measurements, not a whole-process memory guarantee.

## Timing remains inconclusive

The frozen timing comparison used reviewed head
`7f5ee7b0ce9090e3f3050f873920ff1a33ff9753`, before the later cancellation-poll
change. Both artifacts implemented that same contract. Do not transfer these
ratios to a different PR revision or reuse older pre-fencing speed claims.

One KVM-virtualized Xeon Platinum 8488C host, CPUs 24-27, optimized Rust 1.98 binaries, 12
balanced ABBA/BAAB blocks, seed 6272026, 48 processes and 32 measured operations
per worker/case. Eight operations warm each worker. Ratios are geometric paired
candidate/baseline contrasts of process arithmetic-mean operation durations.
Student-t intervals use 11 degrees of freedom and Bonferroni allocation across
six cases and two allowed timing comparisons. Their nominal familywise coverage
requires approximately independent normal block contrasts. A/A controls used
distinct paths with identical binaries; they checked mechanics, not calibration.

| Case | Time ratio | Adjusted interval |
|---|---:|---:|
| single | 0.95454 | [0.85335, 1.06772] |
| mixed64 | 0.95984 | [0.87591, 1.05182] |
| duplicates1024 | 0.94882 | [0.86516, 1.04057] |
| distinct256 | 0.98527 | [0.88167, 1.10105] |
| remote64 | 1.00451 | [0.92875, 1.08644] |
| concurrent64 | 0.93319 | [0.77258, 1.12719] |

Every interval includes 1. No speedup or latency-noninferiority claim follows.
Four workers execute finite closed-loop batches; contention falls as workers
finish. This does not measure sustained offered load or service capacity.
Affinity is not isolation: other host activity remained uncontrolled. No
host/build population generalization or repository performance gate is claimed.
ABBA/BAAB controls linear position drift, not arbitrary carryover. The fixed
horizon is a resource budget, not a power guarantee. At block log SD 1%, 3%,
5%, the interval's upper relative half-width is about 1.04%, 3.17%, 5.33%.

Descriptive nearest-rank p50/p95, in milliseconds, pool raw observations within
each arm/case, not per-process percentiles. Serial cases contain 768 observations
per arm; concurrent64 contains 3,072. These are not independent tail-effect tests.

| Case | Baseline p50 / p95 | Candidate p50 / p95 |
|---|---:|---:|
| single | 0.380 / 0.680 | 0.377 / 0.429 |
| mixed64 | 3.229 / 4.845 | 3.210 / 3.302 |
| duplicates1024 | 4.562 / 4.721 | 4.437 / 4.511 |
| distinct256 | 28.823 / 34.861 | 28.760 / 30.106 |
| remote64 | 2.009 / 2.078 | 2.002 / 2.142 |
| concurrent64 | 4.444 / 33.131 | 4.305 / 30.272 |

Concurrent baseline quarter means were 10.669, 8.976, 7.363, 6.826 ms;
candidate quarters were 9.571, 8.184, 7.067, 5.948 ms. This drain-down reinforces
the finite-batch caveat, not a steady-state speed claim.

## Reproduction and moving-head workflow

`scripts/bench-claim-validation.py` runs the native harness against explicit
baseline/candidate executable paths. Build each artifact with Cargo's
`--no-run --message-format=json --locked` and select its unique
`claim_validation` bench executable. Keep timing and allocation binaries
separate. The runner requires Linux CPUs 24-27 within its allowed affinity;
analysis requires Python with SciPy (1.13.1 was used here).

```sh
python3 scripts/bench-claim-validation.py run --output /tmp/claim-run \
  --baseline /absolute/baseline-binary --candidate /absolute/candidate-binary \
  --cwd "$PWD" --blocks 12 --samples 32
python3 scripts/bench-claim-validation.py analyze --output /tmp/claim-run
```

The output directory must not exist. The runner retains raw samples, complete
attempt status, controlled environment, argv, binary/library hashes, worker
affinity, load averages and timestamps. It does not retry failed attempts.
Use `--mode allocations --blocks 2` with allocation binaries for the deterministic
resource check. Output is evidence, not an automatic merge verdict.

During this work the PR moved from `1c0f2c05` to `7f5ee7b0` and then `75678544`.
Only the owned performance/workload patch was replayed. The materialized
workload was updated for the changed API; both timing artifacts were rebuilt
on `7f5ee7b0`. Later cancellation changes were integrated and allocation guards
rerun on `75678544`, without claiming a new timing result. This separates a
verified resource improvement from continually moving latency measurements.

| Round | Disposition |
|---|---|
| 1 | Materialized workload, explicit overflow reporting and A/A setup |
| 2 | Rejected compact-collect prototype: spare Vec capacity increased three peaks by 2–64 bytes |
| 3 | Exact-sized verdict vector removed that overshoot; allocation checks passed |
| 4 | Duplicate accounting-key copies removed; allocation checks passed |
| 5 | Review changes replayed; same-contract timing and latest-head allocation checks completed |

Five source/setup rounds consumed: `ROUND_LIMIT`. The fixed keep rule required
lower requested bytes, no higher recorded peak, passing correctness, and no
statistically supported timing slowdown. Inconclusive timing is not proof of
no slowdown. Minor served-map ID copying and wider claim-read query costs were
not optimized in this pass.

## Checks

CI-profile workspace formatting and all-target/all-feature Clippy passed.
Kernel/retrieval/daemon nextest ran 2,825 tests: all passed, 14 skipped. The
allocation-mode benchmark and comment-marker check passed. Independent code
review found no actionable issue; independent evidence review recomputed the
reported counters, ratios and bounds.

An additional all-target **release-profile** Clippy check failed in pre-existing
`eidnara-host` tests that call debug-only `phase_cap_override`. The same mismatch
exists in baseline `75678544`. CI's dev-profile command passed; release benchmark
builds and targeted release claim/eligibility tests passed. This patch does not
change that unrelated test-profile configuration.

Raw evidence on the measurement host: `/tmp/pr627-current-perf/`. It retains
frozen design and integration manifests, the rejected prototype, exact binaries,
build receipts, A/A records, all attempts and analyses. Temporary storage is not
an archive; copy the bundle when retaining this study long term.

| Verified artifact | SHA-256 |
|---|---|
| Latest baseline allocation binary | `3f165088589fda11909ede9fa097f6ac80b0ccd8556c4e8a6395dbf245432b56` |
| Latest candidate allocation binary | `75263f73b02f0128a3d6898f6a0630bebeed8c340fe91d0012203b34559c3cb4` |
| Frozen timing baseline binary | `968ca8287c8372ec9f2a01ccadf818acc0a358b084da73cc28c845c2da8c50bc` |
| Frozen timing candidate binary | `6b8ac243687d901b28d6ef04fc82d44488790f6e18cb511a9caa9c921083c33c` |
