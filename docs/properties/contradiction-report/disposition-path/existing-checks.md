# Existing checks: kernel.commit disposition path

System: authored at `3a3b916b7` (2026-10-25); line numbers re-verified at
the PR A tip of #1160 (code commit `6c4d445e3`; references into the files
the evidence-walk commit `f94f3163b` changed re-verified at `f94f3163b`).
The tables list the checks found at authoring; PR A's dispute tests are
listed in the last section below and in full in
[`../dispute-evidence/existing-checks.md`](../dispute-evidence/existing-checks.md).
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
| 4398 | `disposition_events_apply_the_fixed_table_and_report_outcome_and_disposition` | each of five events yields the table's `(outcome, disposition)` from Active; `denied: false`; stored disposition matches; object becomes a token; quarantine fixture | classes (partial), restrictiveness, maturity (none: no maturity column read) | unaudited |
| 4458 | `a_relaxation_is_denied_without_a_valid_approval_and_permitted_with_one` | Quarantined to Stale denied, one new row, disposition unchanged; unknown approval `not_found`; scoped approval permits; Stale to Stale admitted | restrictiveness | unaudited |
| 4568, 4635 | `disposition_approval_chains_of_every_kind_are_project_scoped`, `assert_disposition_approval_scope` | direct, own-admission, and lineage approval chains out of scope answer `not_found`, no receipt | subject-in-project | unaudited |
| 4579 | `approval_chain_reader_sees_envelope_writes_and_refuses_truncation` | chain reader sees in-envelope writes; over-depth refused | subject-in-project | unaudited |
| 4888 | `a_disposition_needs_a_live_admitted_decision` | retired subject `not_found`; unadmitted subject `admission_policy`; no row written | subject-in-project | unaudited |
| 4947 | `a_disposition_leaves_the_object_token_valid` | a token read before a disposition still authorizes a retire | (context) | unaudited |
| 4982 | `disposition_operations_share_the_envelope_replay_and_token_rules` | replay returns equal `dispositions` and `tokens`, no new rows; different digest `operation_key_reused`; token abort drops the whole envelope; missing id `not_found` | replay, subject-in-project | unaudited |
| 5120 | `a_disposition_preview_reports_visibility_per_surface_and_writes_nothing` | preview leaves log, ledger, receipts untouched; per-surface verdicts | preview | unaudited |
| 5277 | `a_disposition_preview_judges_later_operations_after_earlier_ones` | in-request prior carried forward; verdicts equal the commit's | preview | unaudited |
| 5355 | `a_disposition_preview_reports_denials_and_refusals_like_the_commit` | denial and out-of-scope refusals match the commit | preview, restrictiveness | unaudited |
| 5493 | `a_disposition_preview_refuses_an_operation_whose_authority_an_earlier_one_changes` | quarantine of an approval plus dependent relaxation in one preview is `invalid_params`; separately previewed they judge; the commit denies the relaxation after the quarantine | preview, authority-bearer cascade (partial) | unaudited |

Fixtures: `commit-available-disposition-quarantine.json`,
`commit-available-disposition-denied.json` (asserted at `:4448`, `:4499`).

## Kernel evaluator unit tests (`crates/kernel/src/admission.rs`)

| Line | Test | What it asserts | Records | Status |
|---|---|---|---|---|
| 4912 | `restrictive_disposition_and_origin_classification_are_sticky` | a Rejected prior stays Rejected under `CodeObserved`; a request with other classes than the prior is `AdmissionPolicy` | classes | unaudited |
| 5208 | `leaving_a_restricted_disposition_requires_authority` | Rejected, Contradicted, Quarantined to Stale or Disputed: denied without approval, moves with one | restrictiveness | unaudited |
| 5313 | `an_event_that_names_a_disposition_keeps_its_own_outcome` | each disposition event keeps its table outcome, not `DemoteSupport` | maturity | unaudited |
| 5407 | `relaxing_a_restrictive_disposition_requires_approval` | relaxation needs approval | restrictiveness | unaudited |
| 5473 | `prior_with_effective_above_historical_is_rejected` | inconsistent prior is `AdmissionPolicy` | classes (refusal branch) | unaudited |
| 5732 | `non_active_dispositions_withdraw_effective_support` | an `Enforce` on a Rejected prior raises `historical` to Enforced and keeps `effective` at Candidate | maturity | unaudited |
| 4736 | `an_authority_event_requires_approval_even_at_an_unchanged_rung` | authority events need approval | (context) | unaudited |

Compile-time guard: `admission.rs:4317-4324` asserts the maturity rank
order. `commit.rs:1293` (`every_disposition_event_names_a_disposition_
transition`) asserts each `DispositionEvent` maps to a kind with
`requested_disposition().is_some()` and that `"approve"` does not parse.
`commit.rs:1313` asserts the receipt payload round trip and tolerates a
receipt without `dispositions`.

## Kernel integration tests (`crates/kernel/tests/kernel_admission.rs`)

| Line | Test | What it asserts | Records | Status |
|---|---|---|---|---|
| 883-904 | (ordinal test body) | `MarkStale`/`MarkDisputed` recorded in one commit take distinct ordinals | replay (ordinal), classes | unaudited |
| 1503 | `quarantining_an_approval_demotes_its_dependents_and_ends_its_authority` | Quarantine on an approval through the store API demotes dependents | authority-bearer cascade | unaudited |
| 1676 | `a_revocation_reason_survives_without_dependents` | a revocation's reason is stored | reason | unaudited |
| 2252 | `both_latest_decision_formulations_choose_the_same_row` | two dispositions in one commit tie-break by `admission_decision_id` | replay (ordering) | unaudited |
| 2735 | `dispositions_and_sensitivity_only_reduce_visibility` | `MarkStale` on a `CodeObserved` subject reduces visibility | maturity | unaudited |
| 2890 | `every_non_active_disposition_has_the_contracted_surface_row` | each non-Active disposition maps to its surface row | maturity (visibility) | unaudited |
| 4211 | `a_trigger_whose_evidence_is_gone_cannot_admit` | a `CodeObserved` trigger whose evidence is retired cannot admit | slice liveness (trigger branch) | unaudited |

## TypeScript tests (`packages/opencode-plugin/src/shared/memory-mark-command.test.ts`)

| Line | Test | What it asserts | Records | Status |
|---|---|---|---|---|
| 81, 92 | `parseMemoryMarkArgs` | event spelling, flag position, refusals of a target disposition, missing id, flag-shaped id | command | unaudited |
| 303 | tightening on a labeled object applies without confirmation | no `confirm` call; `applied` | command | unaudited |
| 326 | the same command on a verified object asks first; no dialog writes nothing | `needs_confirmation` | command | unaudited |
| 347 | declining writes nothing; accepting applies | `declined`, `applied` | command | unaudited |
| 366 | a relaxation without an approval is denied before anything is written | `denied` from preview, no commit | command, restrictiveness | unaudited |
| 383 | repeating a command with the same identity replays the receipt | `replayed: true` | replay | unaudited |
| 394 | a repeat after the object moved on still replays | replay with stale verdict | replay | unaudited |
| 407 | a denial recorded by the commit itself is reported as denied | `denied` from commit | command | unaudited |
| 425 | a session that ends after the preview commits nothing | `isCancelled` honored | command | unaudited |
| 456 | an actor carrying the key separator is refused before any call | `RangeError` path | command | unaudited |
| 472 | a refused preview names the state and writes nothing | `refused/preview` | command | unaudited |
| 486 | a verdict for another object or event never decides the one the user named | `verdictFor` null paths | command | unaudited |

## Production guards

| Location | Guard | Records | Status |
|---|---|---|---|
| `admission.rs:265-281` | `evaluate_admission` refuses illegal class pairs, prior mismatch, inconsistent prior, `Enforce` without evidence, succession without predecessor | classes, maturity | unaudited |
| `admission.rs:1565-1567` | blank reason is `AdmissionPolicy` | reason | unaudited |
| `redaction.rs:72-77` | reason over 512 KiB is `InvalidInput` | reason | unaudited |
| `admission.rs:2564-2572` | `refuse_succession_events` on `record_admission` and `preview_admission` | (context) | unaudited |
| `schema.rs:163` | FK `evidence_id REFERENCES evidence_meta ON DELETE RESTRICT`; `foreign_keys=ON` at `schema.rs:244`, `open.rs:1035` | evidence existence | unaudited |
| `schema.rs:26` | `commit_log` append-only triggers | commit-log row | unaudited |
| `commit.rs:136-137`, `:108-109` | `deny_unknown_fields` on `Operation` and `CommitRequest` | no-evidence field | unaudited |
| `commit.rs:1143-1158` | preview refuses tokens and non-disposition operations | preview | unaudited |
| `state.rs:191-250` | compile-time guard that every `InvalidReason` has a slice position | refusal mapping | unaudited |

## PR A checks that touch these records

| Location | Test | Records here | Status |
|---|---|---|---|
| `kernel_routes.rs:5723` | `a_dispute_citing_live_project_evidence_lands_with_the_id_and_the_reporter` | classes, maturity, commit-log row, caller-asserted fields | unaudited |
| `kernel_routes.rs:5801` | `a_dispute_citing_unusable_evidence_is_not_found_and_writes_nothing` | existence-only (route gate), evidence scope (re-labelled) | unaudited |
| `kernel_routes.rs:5971` | `a_dispute_needs_evidence_and_other_events_take_none` | invalidated no-evidence record (other events keep `evidence_id IS NULL`) | unaudited |
| `kernel_routes.rs:6080` | `dispute_identities_land_replay_and_refuse_reuse_like_every_envelope` | replay | unaudited |
| `kernel_routes.rs:6146` | `a_dispute_is_denied_against_a_more_restrictive_prior_and_lateral_from_stale` | restrictiveness (Stale to Disputed) | unaudited |
| `kernel_routes.rs:6199` | `a_dispute_on_an_authority_bearer_copies_its_reason_onto_demoted_dependents` | authority-bearer cascade, classes, maturity | unaudited |
| `admission.rs:4532` | `a_dispute_with_evidence_never_raises_standing_across_the_grid` | maturity, restrictiveness (dispute events) | unaudited |
| `memory-mark-command.test.ts:185` | the evidence rides the preview and the commit byte for byte under the user source kind | command (both bodies, `source_kind`) | unaudited |
| `memory-mark-command.test.ts:212` | two disputes with different evidence in one session are two commits; the same evidence replays | replay | unaudited |

## None found

At authoring (`3a3b916b7`), with the PR A state noted:

- No test read the class, maturity, `evidence_id`, or `reason` columns of
  a route-written disposition row. PR A's dispute tests read them for
  `mark_disputed` and `contradict`; no test reads `trigger_object_id`, or
  any of these columns for the other three events.
- No test sends a disposition operation with an unknown key.
- No test writes an admission row citing a retired evidence id through the
  Kernel API. The route now refuses one before the write
  (`kernel_routes.rs:5801`).
- No test names an evidence object id as a disposition subject, approval,
  or token.
- No test read the `commit_log` row of a disposition commit. PR A reads
  `actor` and `cause` (`kernel_routes.rs:5723`); no test reads the column
  set.
- No test drove `mark_disputed` on an approval through the route and read
  the dependents' rows. PR A does (`kernel_routes.rs:6199`); `mark_stale`
  on an approval is still not driven.
- No test drove Stale to Disputed or Disputed to Stale. PR A drives Stale
  to Disputed (`kernel_routes.rs:6146`); Disputed to Stale is not driven.
- No TypeScript test asserted the commit body equals the preview body. PR
  A matches both bodies against one expected operation
  (`memory-mark-command.test.ts:185`).
- No liveness check existed on this surface. PR A adds the dispute
  evidence check (`require_visible_evidence`, `commit.rs:742-757`).

## Suspiciously quiet areas

- The disposition row's class and maturity columns for the three
  non-dispute events: those route tests read `disposition` only
  (`stored_disposition`, `kernel_routes.rs:4348`).
- The admission path's `evidence_id` handling: one production writer sets
  it (`harness_sources.rs:976`); the `memory_reviewer/lifecycle.rs:649`
  citation is an `ObservationSpec` inside `#[cfg(test)]` (module at
  `:500`), so it is test-only and not a writer of this path. No test
  exercises a retired or dangling id on that path.
- The lateral disposition moves at equal restrictiveness rank other than
  Stale to Disputed.
- `derive_classes` is unit-tested (`commit.rs:1213-1291`) but its output
  is irrelevant to the disposition path, which no test demonstrates.
- The TypeScript `invalid_params` transport error path for a body the
  daemon refuses as malformed.
