# cap-capture-on-requires-frozen-family

## Discovery trigger
Parent specification #1119 C7, user story 4; Part B record `cap-capture-on-requires-frozen-family`;
implemented by #1121.

## Evidence trail
- `crates/eval-core/src/manifest.rs:575` `Manifest::capture_declaration`, the
  one reader of `run_identity.config.capture`.
- `crates/eval-core/src/manifest.rs:564` the refusal, after the
  recency-baseline checks.
- `crates/eval-core/tests/manifest.rs::a_capture_on_run_without_a_frozen_family_is_refused_by_name`
  covers the three on-combinations, the all-off and absent cases, and two
  malformed values.
- `crates/eval-core/tests/statistics.rs::a_capture_on_run_without_a_frozen_family_never_reaches_the_family_gate`
  shows the refusal through `analyze` and `FamilyNotRecorded` for the
  undeclared and all-off runs.

## Failure scenario
Before this check every absent digest was `FamilyNotRecorded`, raised only
when paired analysis ran, so a capture-on run that never ran paired analysis
carried no frozen plan.

## Timing windows and dependencies
None.

## What a test must construct
A manifest with `capture {off, on}` and `analysis_family_digest: null`; a
malformed `config.capture` beside a frozen family.

Legal witness: `{on, off}` with a digest and a baseline validates; `{off,
off}` or no key with a null digest validates. Smallest falsifier: `{off, on}`
with a null digest accepted.

## Investigation log
No open questions were raised while verifying this record.
