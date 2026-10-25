# Existing checks: kernel.commit disposition path

System: `/local/home/ahrav/scratch/eidnara`, HEAD `3a3b916b7` (2026-10-25).
Every check below is `unaudited`: adequacy verdicts belong to
`/testing:invariant-test-review` (tests) and
`/low-level-systems:defensive-assertions-and-invariant-guards` (production
guards). An existing check never removes a property from the catalog.

## Route integration tests (`crates/daemon/tests/kernel_routes.rs`)

Driven through a test `Daemon` with `source_kind: "assistant"` unless the
test seeds through the store API.

| Line | Test | What it asserts | Records | Status |
|---|---|---|---|---|
| 931 | `replayed_intents_return_one_receipt_and_projects_never_collide` | one receipt per identity; projects do not share receipts | replay | unaudited |
| 976 | `a_token_conflicts_when_its_object_advanced_was_retracted_or_was_superseded` | token conflict reasons | (context) | unaudited |
| 3153 | `another_projects_objects_are_not_found_through_commit_targets_or_tokens` | foreign objects are `not_found` as targets and tokens | subject-in-project | unaudited |
| 4392 | `disposition_events_apply_the_fixed_table_and_report_outcome_and_disposition` | each of five events yields the table's `(outcome, disposition)` from Active; `denied: false`; stored disposition matches; object becomes a token; quarantine fixture | classes (partial), restrictiveness, maturity (none: no maturity column read) | unaudited |
| 4452 | `a_relaxation_is_denied_without_a_valid_approval_and_permitted_with_one` | Quarantined to Stale denied, one new row, disposition unchanged; unknown approval `not_found`; scoped approval permits; Stale to Stale admitted | restrictiveness | unaudited |
| 4562, 4629 | `disposition_approval_chains_of_every_kind_are_project_scoped`, `assert_disposition_approval_scope` | direct, own-admission, and lineage approval chains out of scope answer `not_found`, no receipt | subject-in-project | unaudited |
| 4573 | `approval_chain_reader_sees_envelope_writes_and_refuses_truncation` | chain reader sees in-envelope writes; over-depth refused | subject-in-project | unaudited |
| 4882 | `a_disposition_needs_a_live_admitted_decision` | retired subject `not_found`; unadmitted subject `admission_policy`; no row written | subject-in-project | unaudited |
| 4941 | `a_disposition_leaves_the_object_token_valid` | a token read before a disposition still authorizes a retire | (context) | unaudited |
| 4976 | `disposition_operations_share_the_envelope_replay_and_token_rules` | replay returns equal `dispositions` and `tokens`, no new rows; different digest `operation_key_reused`; token abort drops the whole envelope; missing id `not_found` | replay, subject-in-project | unaudited |
| 5114 | `a_disposition_preview_reports_visibility_per_surface_and_writes_nothing` | preview leaves log, ledger, receipts untouched; per-surface verdicts | preview | unaudited |
| 5271 | `a_disposition_preview_judges_later_operations_after_earlier_ones` | in-request prior carried forward; verdicts equal the commit's | preview | unaudited |
| 5349 | `a_disposition_preview_reports_denials_and_refusals_like_the_commit` | denial and out-of-scope refusals match the commit | preview, restrictiveness | unaudited |
| 5487 | `a_disposition_preview_refuses_an_operation_whose_authority_an_earlier_one_changes` | quarantine of an approval plus dependent relaxation in one preview is `invalid_params`; separately previewed they judge; the commit denies the relaxation after the quarantine | preview, authority-bearer cascade (partial) | unaudited |

Fixtures: `commit-available-disposition-quarantine.json`,
`commit-available-disposition-denied.json` (asserted at `:4442`, `:4493`).

## Kernel evaluator unit tests (`crates/kernel/src/admission.rs`)

| Line | Test | What it asserts | Records | Status |
|---|---|---|---|---|
| 4839 | `restrictive_disposition_and_origin_classification_are_sticky` | a Rejected prior stays Rejected under `CodeObserved`; a request with other classes than the prior is `AdmissionPolicy` | classes | unaudited |
| 5135 | `leaving_a_restricted_disposition_requires_authority` | Rejected, Contradicted, Quarantined to Stale or Disputed: denied without approval, moves with one | restrictiveness | unaudited |
| 5240 | `an_event_that_names_a_disposition_keeps_its_own_outcome` | each disposition event keeps its table outcome, not `DemoteSupport` | maturity | unaudited |
| 5334 | `relaxing_a_restrictive_disposition_requires_approval` | relaxation needs approval | restrictiveness | unaudited |
| 5400 | `prior_with_effective_above_historical_is_rejected` | inconsistent prior is `AdmissionPolicy` | classes (refusal branch) | unaudited |
| 5659 | `non_active_dispositions_withdraw_effective_support` | an `Enforce` on a Rejected prior raises `historical` to Enforced and keeps `effective` at Candidate | maturity | unaudited |
| 4663 | `an_authority_event_requires_approval_even_at_an_unchanged_rung` | authority events need approval | (context) | unaudited |

Compile-time guard: `admission.rs:4312-4319` asserts the maturity rank
order. `commit.rs:1151` (`every_disposition_event_names_a_disposition_
transition`) asserts each `DispositionEvent` maps to a kind with
`requested_disposition().is_some()` and that `"approve"` does not parse.
`commit.rs:1171` asserts the receipt payload round trip and tolerates a
receipt without `dispositions`.

## Kernel integration tests (`crates/kernel/tests/kernel_admission.rs`)

| Line | Test | What it asserts | Records | Status |
|---|---|---|---|---|
| 883-904 | (ordinal test body) | `MarkStale`/`MarkDisputed` recorded in one commit take distinct ordinals | replay (ordinal), classes | unaudited |
| 1503 | `quarantining_an_approval_demotes_its_dependents_and_ends_its_authority` | Quarantine on an approval through the store API demotes dependents | authority-bearer cascade | unaudited |
| 1676 | `a_revocation_reason_survives_without_dependents` | a revocation's reason is stored | reason | unaudited |
| 2269 | `both_latest_decision_formulations_choose_the_same_row` | two dispositions in one commit tie-break by `admission_decision_id` | replay (ordering) | unaudited |
| 2735 | `dispositions_and_sensitivity_only_reduce_visibility` | `MarkStale` on a `CodeObserved` subject reduces visibility | maturity | unaudited |
| 2894 | `every_non_active_disposition_has_the_contracted_surface_row` | each non-Active disposition maps to its surface row | maturity (visibility) | unaudited |
| 4211 | `a_trigger_whose_evidence_is_gone_cannot_admit` | a `CodeObserved` trigger whose evidence is retired cannot admit | slice liveness (trigger branch) | unaudited |

## TypeScript tests (`packages/opencode-plugin/src/shared/memory-mark-command.test.ts`)

| Line | Test | What it asserts | Records | Status |
|---|---|---|---|---|
| 68, 79 | `parseMemoryMarkArgs` | event spelling, flag position, refusals of a target disposition, missing id, flag-shaped id | command | unaudited |
| 93 | tightening on a labeled object applies without confirmation | no `confirm` call; `applied` | command | unaudited |
| 115 | the same command on a verified object asks first; no dialog writes nothing | `needs_confirmation` | command | unaudited |
| 136 | declining writes nothing; accepting applies | `declined`, `applied` | command | unaudited |
| 155 | a relaxation without an approval is denied before anything is written | `denied` from preview, no commit | command, restrictiveness | unaudited |
| 172 | repeating a command with the same identity replays the receipt | `replayed: true` | replay | unaudited |
| 183 | a repeat after the object moved on still replays | replay with stale verdict | replay | unaudited |
| 196 | a denial recorded by the commit itself is reported as denied | `denied` from commit | command | unaudited |
| 214 | a session that ends after the preview commits nothing | `isCancelled` honored | command | unaudited |
| 245 | an actor carrying the key separator is refused before any call | `RangeError` path | command | unaudited |
| 261 | a refused preview names the state and writes nothing | `refused/preview` | command | unaudited |
| 275 | a verdict for another object or event never decides the one the user named | `verdictFor` null paths | command | unaudited |

## Production guards

| Location | Guard | Records | Status |
|---|---|---|---|
| `admission.rs:265-281` | `evaluate_admission` refuses illegal class pairs, prior mismatch, inconsistent prior, `Enforce` without evidence, succession without predecessor | classes, maturity | unaudited |
| `admission.rs:1560-1562` | blank reason is `AdmissionPolicy` | reason | unaudited |
| `redaction.rs:72-77` | reason over 512 KiB is `InvalidInput` | reason | unaudited |
| `admission.rs:2559-2567` | `refuse_succession_events` on `record_admission` and `preview_admission` | (context) | unaudited |
| `schema.rs:162` | FK `evidence_id REFERENCES evidence_meta ON DELETE RESTRICT`; `foreign_keys=ON` at `schema.rs:234`, `open.rs:1035` | evidence existence | unaudited |
| `schema.rs:25` | `commit_log` append-only triggers | commit-log row | unaudited |
| `commit.rs:82-83`, `:54-55` | `deny_unknown_fields` on `Operation` and `CommitRequest` | no-evidence field | unaudited |
| `commit.rs:1001-1016` | preview refuses tokens and non-disposition operations | preview | unaudited |
| `state.rs:191-250` | compile-time guard that every `InvalidReason` has a slice position | refusal mapping | unaudited |

## None found

- No test reads `source_class`, `taint_class`, `maturity`,
  `effective_maturity`, `evidence_id`, `trigger_object_id`, or `reason` of
  a route-written disposition row.
- No test sends a disposition operation with an unknown key.
- No test writes an admission row citing a retired evidence id.
- No test names an evidence object id as a disposition subject, approval,
  or token.
- No test reads the `commit_log` row of a disposition commit.
- No test drives `mark_disputed` or `mark_stale` on an approval object
  through the route and reads the dependents' rows.
- No test drives Stale to Disputed or Disputed to Stale.
- No TypeScript test asserts the commit body equals the preview body.
- No liveness checks exist on this surface (none claimed, none found).

## Suspiciously quiet areas

- The disposition row's class and maturity columns: every route test
  reads `disposition` only (`stored_disposition`, `kernel_routes.rs:4342`).
- The admission path's `evidence_id` handling: two production writers set
  it (`harness_sources.rs:976`, `memory_reviewer/lifecycle.rs:644`) and no
  test exercises a retired or dangling id on that path.
- The lateral disposition moves at equal restrictiveness rank.
- `derive_classes` is unit-tested (`commit.rs:1071-1149`) but its output
  is irrelevant to the disposition path, which no test demonstrates.
- The TypeScript `invalid_params` transport error path for a body the
  daemon refuses as malformed.
