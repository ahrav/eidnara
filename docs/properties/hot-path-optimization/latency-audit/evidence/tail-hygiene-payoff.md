# tail-hygiene-payoff

## Discovery trigger

The bounded hygiene memo needs evidence that its warm-call benefit exceeds
local run-to-run variation. The fixed run meets both predeclared ticket-local
conditions: **YES**, with a **73.1659% reduction in measured warm-call time**.
This is supporting evidence for [B4](../catalog.md#hygiene-digest-is-kind-prefixed-part-content),
not a new property or a revival of the retired global M0/W1/W2 campaign.

## Evidence trail

The evidence root is `/tmp/opencode/tail-hygiene-payoff/`. The
[compact manifest](tail-hygiene-payoff.json) retains identities and receipt
paths/hashes. `plan.md` freezes the workload, schedule, analysis, and local
rule. `plan-repaired.md` amends only the baseline identity and blocked-build
status before measurement. `aa-summary.json`, `ab-summary.json`, and
`pr-summary.md` record the completed result.

### Paired result and local rule

Three A/A pairs ran in order AB, BA, AB, using A for both labels. Five A/B
pairs followed in order AB, BA, AB, BA, AB. All 16 fresh processes are valid,
with 20 Criterion samples each and distinct `CRITERION_HOME` directories.
There are no retries, discarded observations, added pairs, or numerical early
stops. A is rerun beside B, rather than compared only with the earlier pilot.

Times below are milliseconds. Each median and p95 summarizes the process's
20 batch-average call times, `times[j] / iters[j]`. The p95 is sorted element
19 of 20, not an individual-call tail-latency percentile. Samples are not
pooled across processes, and process p95s are not averaged.

| Pair | Order | A median | A p95 | B median | B p95 | B/A median ratio |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 1 | AB | 2.425548 | 2.436526 | 0.651831 | 0.653447 | 0.268735537 |
| 2 | BA | 2.434601 | 2.441615 | 0.654061 | 0.659193 | 0.268652269 |
| 3 | AB | 2.431053 | 2.438300 | 0.655358 | 0.656311 | 0.269577933 |
| 4 | BA | 2.419599 | 2.427811 | 0.650700 | 0.651604 | 0.268928949 |
| 5 | AB | 2.439236 | 2.457155 | 0.648409 | 0.649443 | 0.265824697 |

For `g = ln(A median / B median)`, all five gains exceed the A/A guard
`N = 0.1658994523568077`; the smallest is `1.310897753908875`.
The mean `G = 1.3154977945163326` also exceeds twice the sample standard
deviation, `2s = 0.010872346514671818`. Both strict inequalities hold.
The geometric mean B/A ratio is `exp(-G) = 0.2683407114517186`, giving
`100 * (1 - exp(-G)) = 73.16592885482814%` less measured time.

The six A/A medians span 2.416126 to 2.852127 ms, an 18.0454% span relative
to the minimum. Their paired `ln(B/A)` values are -0.009020317, 0.009775619,
and -0.156023472, with mean -0.051756057 and sample SD 0.090785969.
`N = ln(max median / min median)` retains that spread, including the slow
third-pair A process. It is an empirical sensitivity guard, not a calibrated
noise floor or proof of stationarity.

The prespecified paired-t 95% interval is
`[0.26653553754286746, 0.27015811132064715]` for B/A, or
72.9842% to 73.3464% less time. It assumes approximately normal, independent,
stationary pair contrasts for these exact builds and this host window. Three
pilot pairs and five treatment pairs do not establish those assumptions.
Deterministic counterbalancing is not random assignment; unequal order counts
and shared-host drift remain limitations. The interval is reporting evidence,
not an additional acceptance threshold or a CI guarantee.

### Exact build identity

The workspace is `/local/home/ahrav/scratch/eidnara`.
Original baseline `2b83194f0185cdc9b13a8e7a6003b864cb618ce8` failed its
release/bench build with exit 101. It produced no baseline executable or
timings. `summary.json`, `build-before.*`, and `checksums.json` preserve that
failure. Commit `d487b5549458796df1820a3a799ffb4bfb7146fa` is a prerequisite
release-accessor repair only: six additions and six deletions in
`crates/daemon/src/transform.rs`, with no memo or benchmark changes.
`repair.patch` preserves the diff; `repair-release-test.log` records the
passing `duplicate_tool_use_belt_drops_later_owner_and_result_in_release`
test. A reuses the target directory from that failed build and repair test;
it is not an independent clean build.

A is that repaired commit. B is the same source base plus the uncommitted
`candidate-source.patch`, not a memo commit named `d487b554`. Build receipts
exclude the user's `docs/agents/issue-tracker.md` edits from build identity.
The full `candidate.patch` also captures the documentation state at build time;
its hash is historical, not the hash of this later documentation update.

B is the fixed-slot memo revision. The later session-table revision, which
refuses over-budget blocks instead of resetting the memo, fingerprints caveman
units instead of cloning them, and replaces sixteen hashed slots with a
least-recently-used table behind per-session locks, has not been measured
under this schedule. Its warm-path work per call is table lookup plus one
`Arc` clone and per-block validity checks; no new timing is claimed for it.

| Identity | A | B |
| --- | --- | --- |
| Executable relative to workspace | `target/hygiene-before/release/deps/hot_path-c149ac66da8b63fb` | `target/hygiene-after/release/deps/hot_path-c149ac66da8b63fb` |
| Executable SHA-256 | `d214810c2c5e9fc820becd6b740994e436faf8cab781d2315207c8980cfe17e8` | `771a492632fd0fa0ce257e3e68e7d3425d3a8399463606ca10229cd1cb0fa5e0` |
| Source-input manifest SHA-256 | `8a40d4cbe7fea489cb5756845f5b3ffe7ffe339b50745b0ac0f154f49cec32f5` | `85bd52a378388d5a988add5f2ec9e364eaf1f53c806cc1652730c41df33d8ccd` |

B source-patch SHA-256:
`9de449cc1e30e053b7787714861179ba39792415a98fe5244441f41b84303c09`.
The source manifests are the SHA-256 of each build receipt's
`json.dumps(inputs_sha256, sort_keys=True)` encoding. Each lists 476 inputs.

Both builds use Rust/Cargo 1.98.1, Criterion 0.8.2, native
`x86_64-unknown-linux-gnu`, and the default optimized `bench` profile, with
matching feature closure and no supplied `RUSTFLAGS` or profile overrides.
Cargo records optimization level 3, no debuginfo, and disabled debug assertions
and overflow checks. The host is AMD EPYC 9R14 in KVM, Linux
`6.12.103-127.188.amzn2023.x86_64`, on
`dev-dsk-ahrav-2c-a9191cb6.us-west-2.amazon.com`. Both arms use CPU 0.

### Recorded commands

These commands identify the completed builds; this documentation update does
not rebuild or rerun the pinned candidate. Build cwd is the workspace root:

```sh
cargo bench --locked -p daemon --features bench-internals --bench hot_path --no-run --message-format=json --target-dir target/hygiene-before
cargo bench --locked -p daemon --features bench-internals --bench hot_path --no-run --message-format=json --target-dir target/hygiene-after
```

Measurement cwd is `/local/home/ahrav/scratch/eidnara/crates/daemon`.
The first A/B pair expands to the commands below; subsequent attempt receipts
retain every expanded argv and their unique Criterion home. A/A uses the same
A executable at both label positions.

```sh
A=/local/home/ahrav/scratch/eidnara/target/hygiene-before/release/deps/hot_path-c149ac66da8b63fb
B=/local/home/ahrav/scratch/eidnara/target/hygiene-after/release/deps/hot_path-c149ac66da8b63fb
R=/tmp/opencode/tail-hygiene-payoff
CRITERION_HOME="$R/raw/ab-p1-A/criterion" /usr/bin/taskset -c 0 "$A" 'tail_hygiene/measure/1400msgs_2KiB_mixed' --exact --warm-up-time 3 --measurement-time 5 --noplot --bench
CRITERION_HOME="$R/raw/ab-p1-B/criterion" /usr/bin/taskset -c 0 "$B" 'tail_hygiene/measure/1400msgs_2KiB_mixed' --exact --warm-up-time 3 --measurement-time 5 --noplot --bench
```

The frozen schedule waits 30 seconds after build/preflight and allows 900
seconds per process. CPU affinity is not CPU isolation. The receipts retain
affinity observations during warmup, collection, and analysis, plus process
lists and runtime hashes. Private effective environments remain in the raw
build/process receipts; do not publish that bundle without secret redaction.

## Failure scenario

Removing result construction, substituting a precomputed U/T pair, omitting
session lookup/locking, or treating batch p95 as individual latency would claim
a benefit outside the measured boundary. Using the failed original baseline
or naming the repair commit as the memo implementation would misidentify A/B.

## Timing windows and dependencies

The cell is `tail_hygiene/measure/1400msgs_2KiB_mixed`: 1,400 messages,
nominal 2,048-byte text payloads, seed `0x9E3779B97F4A7C15`. Every fourth
message is a tool call with command text capped at 256 bytes. The label does
not mean each serialized message is exactly 2 KiB. Projection construction,
tokenizer initialization, pool construction, and memo priming are outside
timing. A's token cache already warms during Criterion warmup.

The [benchmark](../../../../../crates/daemon/benches/hot_path.rs#L161-L199)
owns and primes B's pool before the callback. Its
[wrapper](../../../../../crates/daemon/src/lib.rs#L154-L180) uses the actual
production memo table with namespace 0 and session ID `benchmark`. Session
lookup, memo locking, memo validity checks, bookkeeping, and full measurement
construction/destruction remain inside each timed call, as does loop overhead.
The fixture has empty core state, no populated tags or caveman units, no
coverage ordinal, empty protected IDs, and `protected_tags = 20`; U is zero.

This result concerns repeated warm in-process hygiene calls at one synthetic
input point. It establishes neither production representativeness nor
concurrent-session, cold-call, delta-ingress, total turn, or session latency.
Correctness tests demonstrate that distinct sessions overlap without blocking
or evicting each other up to the sixteen-session limit, not contention payoff.
The 16 MiB plus fixed-container retention bound
and its shared accounting-model limits are documented in
[B4's retention evidence](hygiene-digest-is-kind-prefixed-part-content.md#q-does-the-bounded-memo-preserve-measurements-and-account-for-retention),
not measured as peak allocation or RSS here.

## What a test must construct

Payoff does not replace B4's cold/warm full-result, digest-domain, poisoned-key,
invalidation, eviction, namespace, reset, and retention checks. The
[existing-check inventory](../existing-checks.md#shared-input-equivalence)
retains their unaudited adequacy status. The pre-memo characterization is
agent-witnessed, transcript-only provenance; no separate characterization
log/binary is preserved, and no artifact-hash verification is claimed for it.
The controller reports all 14 recent local gates passed, with logs at
`/tmp/opencode/hygiene-memo-*.log`; this is not a claim of a remote CI run.

## Investigation log

### Q: Does the retained local evidence support the payoff decision?

- Sources examined: The eight requested scratch documents, raw Criterion
  samples and process receipts, build input maps, retained executables,
  repair diff/test log, local gate logs, and the cited source boundaries.
- Findings: On 2026-09-12, all 280 files named by `ab-checksums.json` match
  their SHA-256 entries. Both binaries match their recorded hashes. All 16
  receipts record valid exit-zero runs and unchanged pre/post identities.
  Recomputing each median/p95 from the raw samples and the paired statistics
  reproduces the summaries and both strict inequalities. All 476 candidate
  source-input hashes still match the working tree during this docs-only pass.
- Missing evidence: Independent reexecution of the pre-memo characterization,
  calibration across builds or hosts, and production/concurrency/cold latency
  remain outside this receipt. Raw evidence lives in local scratch storage;
  the manifest does not archive it or promise CI retention.
- Conclusion: The completed fixed schedule supports retaining the optimization
  for this local warm-call payoff. No test, build, or benchmark is rerun for
  this documentation update.
