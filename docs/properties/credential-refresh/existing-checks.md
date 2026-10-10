# Existing checks: credential refresh qualification

Every claim-bearing check the five records cite, with status `unaudited`.
Adequacy verdicts belong to `/testing:invariant-test-review`; a listed check is
a link, not a proof. Locations are verified against the #872 branch.

## Lifetime predicate (N2)

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `lifetime_must_exceed_the_deadline_plus_skew_strictly` | `crates/host-runtime/src/model_execution/aws_refresh.rs:1105` | threshold minus one, equal, plus one; the row at the skew; cooldown after a refused row | unaudited |
| `clock_rollback_never_extends_life_and_a_forward_jump_shortens_it` | `aws_refresh.rs:1138` | monotonic bound after a wall rollback; a forward wall jump shortens life | unaudited |
| `profile_rows_without_lifetime_start_no_child` | `crates/host-runtime/tests/model_execution_subprocess.rs:4073` | a row five seconds short and a row with no lifetime start no child on either adapter | unaudited |
| `a_wall_jump_after_acquisition_refuses_the_spawn` | `model_execution_subprocess.rs:4140` | a wall jump between acquisition and the spawn recheck | unaudited |

Production guard: `recheck_before_spawn` at `aws_refresh.rs:612` re-applies
`covers` before the child starts.

## Model-effect certainty (N8)

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `cross_incarnation_unknown_records_completion_backoff_without_fallback` | `crates/daemon/src/history_summarizer.rs:4621` | unknown outcome under another incarnation; one start, backoff recorded | unaudited |
| `a_start_failure_with_an_unproven_effect_starts_no_second_model` | `history_summarizer.rs:4690` | start failure without proof the model did not run | unaudited |
| `a_source_failure_skips_same_provider_models_and_falls_back_to_another_source` | `history_summarizer.rs:4926` | source-scoped failure skips the sibling model, falls back across sources, under a source-like and a model-like detail text | unaudited |
| `a_wrapped_source_failure_is_never_a_chunk_failure` | `history_summarizer.rs:5035` | typed scope decides chunk-failure classification, not the detail text | unaudited |
| `a_source_failure_whose_cancel_is_unproven_starts_no_further_model` | `history_summarizer.rs:5077` | timed-out cancel stops the fallback and keeps the source's retry | unaudited |

## Recovery after renewal (N9)

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `the_normal_path_folds_again_after_a_source_failure_and_cooldown` | `crates/daemon/src/source_recovery_tests.rs:68` | normal path; `backoff` refusal; two starts | unaudited |
| `the_emergency_path_folds_again_after_a_source_failure_and_cooldown` | `source_recovery_tests.rs:93` | emergency path; second start; idle at `firing_seq >= 2` | unaudited |
| `the_wrapup_path_folds_again_after_a_source_failure_and_cooldown` | `source_recovery_tests.rs:124` | wrapup `retryable` then `ok` | unaudited |
| `the_reattach_path_folds_again_after_a_source_failure_and_cooldown` | `source_recovery_tests.rs:157` | reattach starts no model on the failure, folds after | unaudited |
| `a_day_of_rotations_and_an_external_login_reuses_one_adapter_and_owner` | `model_execution_subprocess.rs:4266` | 49 runs, 26 refreshes, one external login, exact row sequence | unaudited |

Test seams: `TEST_WAIT_BUDGET` and `TEST_WAIT_POLL` at
`crates/daemon/src/lib.rs:36613`; `fire_and_settle` at `:41620`;
`expire_history_summarizer_backoff` at `:40623`.

## Source health (N10)

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `rust_sanitizer_matches_the_shared_vectors` | `crates/host-runtime/tests/source_health_vectors.rs:5` | 29 shared vectors: every kind and state, bounds, malformed blocks, eight canaries | unaudited |
| `the parser matches the Rust sanitizer on every shared vector` | `packages/opencode-plugin/src/shared/source-health.test.ts:25` | the same vectors in TypeScript | unaudited |
| `a missing, malformed, or extra-key block is unknown and never ready` | `source-health.test.ts:68` | absent or malformed never reads as ready | unaudited |
| `health_reports_the_cached_source_observation_for_each_mode` | `crates/host-runtime/tests/model_execution_protocol.rs:408` | the serialized block for no source, an environment row, and a selected profile | unaudited |

None found: a check that counts credential I/O across a health read.

## Qualification campaign (N13)

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `passing_results_pass_and_each_gate_fires_alone_on_a_failing_case` | `crates/eval-core/src/qualification.rs:1185` | each `GateFailure` variant fires alone | unaudited |
| `the_outage_judge_fires_on_each_failure` | `qualification.rs:1309` | each `OutageFailure` variant | unaudited |
| `the_lineage_audit_fires_on_duplicates_overlaps_loss_and_gaps` | `qualification.rs:1399` | each `LineageViolation` kind | unaudited |
| `only_a_complete_passing_run_on_the_dedicated_runner_qualifies` | `qualification.rs:1572` | host shortfalls, cgroup constraint, pending witnesses | unaudited |
| `warm_acquisition_p99_stays_within_one_millisecond` | `crates/host-runtime/tests/model_execution_supervisor.rs:1813` | warm-path p99, release only, `#[ignore]` | unaudited |

Production oracles: `gate` at `qualification.rs:459`, `OutageRun::judge` at
`:731`, `audit_lineage` at `:922`, `QualificationReport::build` at `:994`.

## Suspiciously quiet areas

- No check records a credential I/O count across `health()`.
- No check binds a recorded `WitnessRun` to the host or build profile it ran
  under; `only_a_complete_passing_run_on_the_dedicated_runner_qualifies`
  covers the campaign's own environment, not the witness runs' environments.
- No qualifying campaign run exists; every N13 check is a failing control or
  a unit of the harness.
- The soak and the path witnesses use scripted producers and dispatches; no
  check drives a real AWS source through a failure and renewal.
