# rid-tokenizer-accounting-profile-bound-per-run

## Discovery trigger
Issue #768 acceptance criteria: "Judge, live provider, or tokenizer identity
changes force re-scoring the frozen anchor set and refuse cross-run
`residual.*` comparison until re-anchored. Record all identities and
tokenizer accounting profiles rather than treating names as interchangeable."

## Evidence trail
- `crates/eval-core/src/anchor.rs` `ProviderProfile`: `provider`, `model`,
  `tokenizer_profile`, all strings; derived `Eq` compares all three.
- `crates/eval-core/src/judge.rs` `ResidualReport`: carries `judge`,
  `calibration_digest`, `live_provider`; `comparable` walks `judge`,
  `live_provider`, `calibration_digest` and returns `ReanchorRequired` naming
  the first that differs; `validate` requires the supplied calibration set's
  judge and digest to match.
- `crates/eval-core/src/judge.rs` `approve`, `live_slice`,
  `LiveSliceReport::validate`: validated `LiveSettings`, a provider among the
  settings' two, the settings' `k`, exactly `repeats` attempts per task, and
  the settings' task set.
- `crates/eval-core/tests/judge.rs`
  `a_changed_judge_provider_or_tokenizer_refuses_cross_run_residual_comparison`:
  a changed judge model, live model, tokenizer profile, or calibration digest
  each refuse with its field.
- `crates/eval-core/tests/judge.rs`
  `live_slice_validation_recomputes_each_task_and_checks_the_schema`: a
  deserialized report with another `k`, a third profile, or unsettled settings
  refuses.

## Failure scenario
A tokenizer change relabelled under the old profile name, or a report scored
under a new judge, would be plotted on the previous run's series.

## Timing windows and dependencies
None.

## What a test must construct
Two reports differing in exactly one identity component; a report and a
settings value that disagree on provider or `k`.

## Investigation log
### Q: Is the tokenizer identity a digest or a name?
- Sources examined: `ProviderProfile` in `anchor.rs`,
  `manifest::TokenizerProfile`.
- Findings: `tokenizer_profile` is a `String`; the manifest type carries
  `name`, `revision`, `digest`. Equality binds the name only.
- Missing evidence: the anchor shell's decision on adopting the manifest type.
- Conclusion: unresolved, needs human input.
