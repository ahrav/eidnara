# flt-hard-case-carries-witness-triple

## Discovery trigger
Parent specification C-DST: "Every hard case carries a coverage witness, a
safety check active while faults are armed, and a bounded healthy-progress
check; a missing entry is `IncompleteCoverage`, not `Pass`."

## Evidence trail
- `crates/eval-core/src/fault.rs` `FaultReport::safety_checks_while_armed`;
  `FaultReport::validate` refuses `SafetyNeverChecked` when it is zero.
- `crates/eval-core/src/fault.rs` `LivenessReport` and its `verdict`;
  `FaultReport::validate` evaluates it only `if let Some(liveness)`, and
  `crates/eval-core/tests/fault.rs` validates a report with `liveness: None`,
  so the progress member is asserted by the drive rather than refused by the
  report.
- `crates/eval-core/src/markers.rs` `Coverage::complete` refuses
  `Incomplete { missing }`.
- `crates/eval-core/tests/fault.rs`
  `a_missing_receipt_is_incomplete_coverage_not_pass`.
- `crates/daemon/tests/eval_fault.rs`
  `liveness_bounds_are_met_with_outside_core_faults_armed`.
- `crates/daemon/examples/eval_runner/fault.rs`
  `Witness::safety_check_while_armed` counts checks while armed;
  `Witness::safety_check` after an episode does not.

## Failure scenario
A campaign arms a fault, heals it, and only then checks store integrity; the
report says the safety property held while the fault was armed, which it
never observed.

## Timing windows and dependencies
The armed window of each episode.

## What a test must construct
A report with zero safety checks while armed, expecting refusal; a report
with a declared cut never receipted, expecting `IncompleteCoverage`.

## Investigation log
### Q: Is the triple recorded per hard case?
- Sources examined: `FaultReport` fields; `Witness` in the fault shell.
- Findings: coverage is per cut, safety is a report-wide count, progress is
  per lane; no per-hard-case grouping exists.
- Missing evidence: a maintainer decision on granularity.
- Conclusion: unresolved, needs human input.
