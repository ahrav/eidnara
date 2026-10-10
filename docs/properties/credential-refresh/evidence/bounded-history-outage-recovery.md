# bounded-history-outage-recovery

## Discovery trigger

Specification #860, record N13: a measured qualification, not formal
liveness. Per-operation unique publication and raw preservation, every fixed
RSS, latency, scan, and allocation gate, and the 360-second recovery bound must
pass under the specified load, the 120-second outage, the cooldown, and
restored demand. The M4 ticket (#872) owns the campaign and blocks the
million-message claim until it passes.

## Evidence trail

All in `crates/eval-core/src/qualification.rs` unless noted.

- Constants: `REPETITIONS` 5 (`:18`), `MIN_INTERACTIVE_OPERATIONS` 10,000
  (`:20`), `OUTAGE_SECONDS` 120 (`:28`), `RECOVERY_BOUND_SECONDS` 360
  (`:29`), `MAX_PEAK_RSS_BYTES` 4 GiB (`:38`), `REQUIRED_MEMORY_BYTES` 16 GiB
  (`:43`). The required runner is documented at `:39`.
- `:459` `gate` fails a case on repetition count, failed repetitions,
  operation count, lineage, read and allocation growth, absolute and relative
  p99, and peak RSS (`GateFailure` at `:413`).
- `:731` `OutageRun::judge` requires the outage to raise the backlog above the
  pre-outage band, the frozen arrival rate to match `frozen_arrival` of the
  fold rate, no failed passes, no schedule slip past `MAX_PASS_SLIP_MS`, and a
  sample at or below the band inside the recovery window.
- `:922` `audit_lineage` asserts each lineage publishes once, ranges never
  overlap, raw messages are lost only to oldest-first eviction under
  `raw_cap_bytes`, and coverage is contiguous. Earlier drafts of this file and
  the catalog cited `:910`; the function sits at `:922` on the #872 branch.
- `:994` `QualificationReport::build` sets `qualified` only with no host
  shortfall, no cgroup constraint, a complete catalog, every gate empty, the
  fixed outage schedule with no judge failure, and every witness in
  `WITNESSES` (`:47`: the four fold paths, `rotation_soak`,
  `warm_acquisition`) recorded exit zero at the report's clean source commit.
- Failing controls: `:1185` each gate fires alone on a failing case; `:1309`
  `the_outage_judge_fires_on_each_failure` (earlier drafts cited `:1296`);
  `:1399` `the_lineage_audit_fires_on_duplicates_overlaps_loss_and_gaps`;
  `:1572` `only_a_complete_passing_run_on_the_dedicated_runner_qualifies`.
- Runner: `crates/daemon/examples/eval_runner/qualification.rs`.
- Warm acquisition: `crates/host-runtime/tests/model_execution_supervisor.rs:1813`
  `warm_acquisition_p99_stays_within_one_millisecond`, `#[ignore]`, run in
  release with `--ignored` on the qualified runner.
- No qualifying run exists. The authoring host has 128 logical CPUs and runs
  under a user-slice cgroup, so `Environment::shortfalls` and
  `constrained_by_cgroup` keep any report from it unqualified.

## Failure scenario

A retained history of one million messages makes interactive passes scan or
allocate in proportion to the folded history, p99 crosses 250 ms or 1.10 times
the 10k control, peak RSS exceeds 4 GiB, or the backlog after the outage never
returns to its band inside 360 seconds. A release claims million-message
support on a run that one of these gates would have failed.

## Timing windows and dependencies

The fixed schedule (`qualification.rs:570`): 60 seconds steady, the
120-second outage, the longest source cooldown of 300 seconds, then the
360-second recovery bound.
The campaign depends on the dedicated runner, a release build, the pinned
tokenizer and seed 702, and a clean source commit for every witness run.

## What a test must construct

The retained-history tiers from 10k to 1m messages and 1000 sessions of 1000
messages; the frozen 50% arrival rate against the calibrated fold rate; the
outage and its cooldown; per-operation publication records for
`audit_lineage`; recorded witness runs for all six names at the same clean
commit; a host that meets the required runner with no cgroup constraint.

## Investigation log

### Q: Does a recorded witness run prove its execution context?

- Sources examined: `WitnessRun` (`qualification.rs:64`),
  `check_witness_runs` (`:79`), the `pending_witnesses` computation in
  `QualificationReport::build` (`:1016`), the runner's `--witness-runs` reader
  (`crates/daemon/examples/eval_runner/qualification.rs:834`).
- Findings: a run carries a test string, a source commit, and an exit code.
  `build` requires a known name, a clean commit equal to the report's, and
  exit zero. No field names the host, the build profile, or the cgroup state,
  so a `warm_acquisition` run from the authoring host or a debug build clears
  the pending set as well as one from the dedicated runner in release.
- Missing evidence: a binding between a witness run and the environment the
  report itself records.
- Conclusion: needs human input - extend `WitnessRun` with the environment and
  profile, or run the measurement inside the campaign.

### Q: Has the campaign passed anywhere?

- Sources examined: `QualificationReport::build`, the runner, the host this
  branch was authored on.
- Findings: the gates, judge, oracle, and their failing controls run here; no
  report with `qualified: true` exists.
- Missing evidence: a complete five-repetition run on the dedicated runner.
- Conclusion: needs human input - supply the Linux x64 GNU runner with 4
  logical CPUs, 16 GiB, and local SSD.
