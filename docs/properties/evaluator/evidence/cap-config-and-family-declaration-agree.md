# cap-config-and-family-declaration-agree

## Discovery trigger
Parent specification #1119 C8; Part B record `cap-config-and-family-declaration-agree`; implemented
by #1121.

## Evidence trail
- `crates/eval-core/src/statistics.rs:1188` `frozen.check(family)?`, then
  `:1189` reads `manifest.capture_declaration()` and compares it, with
  `CaptureDeclaration::OFF` for an absent copy.
- `crates/eval-core/tests/statistics.rs::the_run_copy_of_the_capture_declaration_must_equal_the_family_copy`
  runs the five-by-four matrix and the freeze-first case.

## Failure scenario
A run whose config says `memory_capture: on` frozen under a family declaring
off would pass the digest gate, because the family is unchanged, and be
compared with capture-off runs.

## Timing windows and dependencies
None.

## What a test must construct
A manifest declaring `{on, off}` whose frozen family carries `{off, off}`; an
absent key beside a family declaring any `on`.

Legal witness: both copies `{on, on, task_arm}`; analysis proceeds. Smallest
falsifier: config `on`, family `off`, and analysis returns a report.

## Investigation log
No open questions were raised while verifying this record.
