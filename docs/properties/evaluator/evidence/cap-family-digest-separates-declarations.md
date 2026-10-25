# cap-family-digest-separates-declarations

## Discovery trigger
Parent specification #1119 C4 and D1; Part B record `cap-family-digest-separates-declarations`;
implemented by #1121.

## Evidence trail
- `crates/eval-core/src/statistics.rs:602` `AnalysisFamily.capture`, a
  required field under `eval-analysis-family/v3` (`:20`).
- `crates/eval-core/src/statistics.rs:732` `digest` runs `check_fields` and
  `protocol_digest` over the serialized family.
- `crates/eval-core/tests/statistics.rs::runs_under_different_capture_declarations_never_read_under_each_other`
  builds both manifests and both families and asserts the cross refusals.
- `crates/eval-core/tests/statistics.rs::the_family_is_frozen_before_outcomes_and_any_post_hoc_edit_refuses`
  gains a `capture` row in its edit table.

## Failure scenario
A digest that left a capture switch out of the serialized family would give
`{off, off}` and `{off, on}` one digest, and a family edited from off to on
after results would pass the freeze.

## Timing windows and dependencies
A family edited after results, the window the freeze gate guards.

## What a test must construct
A manifest frozen under one declaration and a family presented with another.

Legal witness: families `{off, off}` and `{off, on}` have distinct digests and
`check` refuses one under the other. Smallest falsifier: `capture` left out of
the digest so the two coincide.

## Investigation log
No open questions were raised while verifying this record.
