# flt-hard-case-carries-witness-triple

## Discovery trigger
Parent specification C-DST: "Every hard case carries a coverage witness, a
safety check active while faults are armed, and a bounded healthy-progress
check; a missing entry is `IncompleteCoverage`, not `Pass`."

## Evidence trail
- `crates/eval-core/src/fault.rs:641` `safety_checks_while_armed`; `:683`
  `validate` refuses `SafetyNeverChecked` when it is zero.
- `crates/eval-core/src/fault.rs:549` `LivenessReport` and its `verdict`.
- `crates/eval-core/src/markers.rs:299` `Coverage::complete` refuses
  `Incomplete { missing }`.
- `crates/eval-core/tests/fault.rs:297` a missing receipt is incomplete
  coverage, not a pass.
- `crates/daemon/tests/eval_fault.rs:512` liveness bounds met with outside-core
  faults armed.
- `crates/daemon/examples/eval_runner/fault.rs:154` `Witness::safety_check`
  counts checks while armed.

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
