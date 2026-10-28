# Fault map: kernel.commit disposition path

System: authored at `3a3b916b7` (2026-10-25); re-verified at the PR A tip
of #1160 (code commit `6c4d445e3`).
Every record is `always`; each needs its enabling state constructed or it
passes vacuously. Coverage checks below assert preconditions, never the
violation, and must fire on a correct implementation. Marker names are
constant.

## Fault classes and availability

| Class | What it is | Available today | How |
|---|---|---|---|
| F1 subject class divergence | subject's prior classes differ from the caller's derived pair | yes, store API | `seed_approval` (`kernel_routes.rs:4362`) writes an `ExplicitUser`/`UserExplicit` `adr_accepted` decision; `store_decision` plus `record_admission` with any pair |
| F2 elevated prior maturity | subject at `Verified`/`Approved`/`Enforced` effective maturity | yes, store API | `CodeObserved` trigger (`kernel_routes.rs:5134-5166`) or `Verify` with a valid approval (`:4747-4757`) |
| F3 non-Active prior disposition | subject already Stale, Disputed, Rejected, Contradicted, Quarantined, Superseded | yes, route | a prior disposition commit; Superseded via `supersede_decision` |
| F4 approval validity | valid, revoked, out-of-scope, non-`adr_accepted`, chain over depth | yes, route and store API | `seed_approval` with `scope_id` None or another project; quarantine the approval; `approval_chain_reader_sees_envelope_writes_and_refuses_truncation` (`:4652`) builds depth |
| F5 retired evidence | `evidence_meta` row with `invalidated_commit_seq` set | yes, store API or sweep | `ingest_evidence` (`kernel_admission.rs:92`) then retire; retention sweep for `harness_capture` |
| F6 dangling evidence id | id with no `evidence_meta` row | yes | any unused string that passes `identity()` |
| F7 secret-like identity | an id or reason the redactor detects | yes | `sk-ant-api03-abcdefghijklmnopqrstuvwxyzABCDEFGH12345678`, which `redact_durable_text` detects (`crates/daemon/src/kernel_routes/dispute.rs:373-380`); 40- and 64-hex object ids and an ordinary path carry no detection (`dispute.rs:385-398`) |
| F8 oversize reason | reason above 512 KiB | yes, store API only | `"a".repeat(MAX_REDACTABLE_BYTES + 1)` |
| F9 unknown body key | extra key on the disposition operation | yes, route | raw JSON with a key outside the six fields; at the PR A tip `evidence_id` and `evidence_ref` are fields |
| F10 identity replay | same `(producer, operation_key)` with equal digest | yes, route | repeat the request |
| F11 identity reuse | same key, different digest | yes, route | `wire_intent("dispose", "other-bytes")` (`kernel_routes.rs:5032`) |
| F12 lost response | transport drops the reply after the commit | partial | `FakeKernelTransport` can simulate `outcome_unknown`; no route-level harness drops a reply |
| F13 interleaved commit | another commit moves the subject between preview and commit | yes, route | two sequential requests from two sessions |
| F14 evidence object as target | an evidence `object_id` in a scoped position | yes, store API | ingest evidence, use its registry `object_id` |
| F15 authority bearer subject | an `adr_accepted` decision with at least one dependent citing it | yes, store API | `seed_approval` then promote a dependent citing it (`kernel_routes.rs:4747-4757`) |
| F16 version skew | new-shape client against HEAD daemon | yes, raw JSON | same as F9 |
| F17 writer contention | `deadline_ms` exceeded waiting for the writer | yes, hard to time | hold the writer in another transaction; answers `unavailable/store_busy` |
| F18 caller-asserted identity | `actor`, `cause`, `producer`, or `source_kind` claiming another surface | yes, route | raw body; the test daemon binds harness `test` (`crates/daemon/tests/kernel_routes.rs:70`) |

Not available: F12 at the route level (no reply-dropping transport in
`crates/daemon/tests`). Records needing it stay `Exercised: partial`.

## Per-property required faults and enabling state

| Record | Required faults | Enabling state | Exercised |
|---|---|---|---|
| `disposition-admission-copies-the-subjects-prior-classes` | F1 | live admitted decision whose classes differ from the caller's; read the written row | partial |
| `disposition-never-raises-maturity-and-withdraws-effective-support` | F2, F3 | subject above Candidate; every event; read `maturity` and `effective_maturity` | partial |
| `disposition-approval-requirement-follows-the-restrictiveness-order` | F3, F4 | subjects in all six non-Active dispositions; every event; approval valid and invalid | partial |
| `disposition-operation-carries-no-evidence-and-refuses-unknown-fields` | F9, F16 | invalidated by PR A; the unknown-key clause needs a raw body with an extra key | not yet |
| `admission-evidence-citation-is-existence-checked-only` | F5, F6, F7 | store-API `record_admission` with `Some(evidence_id)` retired, dangling, and secret-like | not yet |
| `slice-write-refuses-non-live-evidence-citations` | F5, F6 | route `insert_decision` citing a retired or dangling id | partial |
| `evidence-objects-carry-no-scope-and-fail-scoped-lookup` | F14 | live evidence row; its `object_id` as subject, approval, token | not yet |
| `disposition-subject-is-a-live-admitted-decision-in-the-bound-project` | F3, F4, F14 | retired, unadmitted, foreign, observation-kind, evidence-kind subjects; stored approval out of scope | partial |
| `disposition-preview-writes-nothing-and-reports-the-commit-verdict` | F3, F4, F10, F13 | each prior disposition; recorded identity; interleaved move | partial |
| `disposition-envelope-identity-replays-without-a-second-write` | F10, F11, F12, F13 | committed disposition; equal and different digests; move then replay | partial |
| `memory-mark-command-commits-only-the-previewed-operation` | F10, F12, F13 | stub verdicts mismatched, empty, multiple; replayed preview; `visibility_changes` true | partial |
| `disposition-on-an-authority-bearer-demotes-dependents-in-the-same-commit` | F15, F1 | approval with dependents; `mark_disputed` from `assistant` source kind | partial |
| `disposition-commit-log-row-carries-actor-and-cause-only` | F7 | command-shaped `actor`/`cause`; a secret-like `cause` | partial |
| `commit-actor-cause-and-source-kind-are-caller-asserted` | F18 | an `actor` naming another harness; `source_kind: "user"` from a non-command client | partial |
| `admission-reason-is-redacted-bounded-and-non-blank` | F7, F8 | store-API reason at the bound and with a detection | partial |

## Coverage checks to add

Each is a `sometimes` precondition marker with a constant name. None
asserts a violation.

| Marker | Fires when | Guards records |
|---|---|---|
| `disposition_subject_classes_differ_from_caller` | a disposition is judged against a prior whose `(source_class, taint_class)` differs from `plan.classes` | classes |
| `disposition_subject_above_candidate` | a disposition is judged against a prior with `effective_maturity` above Candidate | maturity |
| `disposition_relaxation_attempted` | `disposition_restrictiveness(requested) < disposition_restrictiveness(prior)` is evaluated | restrictiveness |
| `disposition_lateral_move_attempted` | requested and prior restrictiveness are equal and dispositions differ | restrictiveness |
| `disposition_relaxation_with_valid_approval` | a relaxation is evaluated with `approval_valid == true` | restrictiveness |
| `disposition_body_with_unknown_key_received` | `parse_request_body` rejects a `kernel.commit` body whose disposition operation has an extra key | no-evidence field |
| `admission_write_with_evidence_id` | `write_admission` runs with `evidence_id.is_some()` | evidence existence |
| `admission_write_with_retired_evidence_id` | the cited row has `invalidated_commit_seq IS NOT NULL` at write time | evidence existence |
| `slice_insert_cites_retired_evidence` | `require_parents` or `require_live_evidence` is entered with at least one retired id | slice liveness |
| `scoped_lookup_of_evidence_object` | `scoped_object_state` is called with an id whose registry kind is `evidence` | evidence scope |
| `preview_then_commit_same_tip` | a commit's operations equal an earlier preview's and no commit intervened | preview |
| `preview_then_commit_tip_moved` | a commit's operations equal an earlier preview's and a commit intervened | preview, command |
| `disposition_identity_replayed` | `stored_receipt` returns `Some` with equal digest for a disposition intent | replay |
| `disposition_identity_reused` | `stored_receipt` returns `Conflict` for a disposition intent | replay |
| `disposition_replayed_after_subject_moved` | a replay whose stored `dispositions[0].disposition` differs from the subject's current stored disposition | replay |
| `command_received_mismatched_verdict` | `verdictFor` returns null in either step | command |
| `command_preview_visibility_changes` | a preview answer with `visibility_changes == true` and no `receipt`, so the command reaches its confirmation branch | command |
| `disposition_on_authority_bearer` | `subject_grants_authority(subject) == true` before a disposition write | cascade |
| `disposition_cascade_demoted_dependents` | `demote_dependents_if_authority_lost` returns a non-zero demoted count after a disposition | cascade |
| `commit_log_cause_redacted` | `redact(intent.cause)` records at least one detection | commit-log row |
| `admission_reason_at_bound` | a reason of exactly `MAX_REDACTABLE_BYTES` bytes is written | reason |
| `admission_reason_redacted` | `redact(reason)` records at least one detection | reason |

Correlation: `preview_then_commit_tip_moved` and
`command_received_mismatched_verdict` must be witnessed in one history to
cover the command's "commit verdict differs from preview" branch; separate
witnesses do not establish it. `disposition_on_authority_bearer` and
`disposition_cascade_demoted_dependents` must coincide in one commit.

## Owned coverage markers (B2 resolution)

The #1160 tests construct the enabling states of these markers. The
markers themselves are not instrumented; each row names the test that
constructs the state a marker would observe, verified by reading the test.

| Marker | Constructing test | What it constructs |
|---|---|---|
| `disposition_subject_above_candidate` | `a_dispute_with_evidence_never_raises_standing_across_the_grid` (`crates/kernel/src/admission.rs:4608`); `a_dispute_on_an_authority_bearer_copies_its_reason_onto_demoted_dependents` (`crates/daemon/tests/kernel_routes.rs:6199`) | the grid judges disputes against every effective maturity, including above Candidate; the route test disputes an `AcceptedAdr` approval whose historical maturity it asserts is not `candidate` (the prior's effective maturity is not asserted) |
| `disposition_lateral_move_attempted` | `a_dispute_is_denied_against_a_more_restrictive_prior_and_lateral_from_stale` (`kernel_routes.rs:6146`) | `mark_disputed` against a `stale` prior, an equal-rank move that lands `disputed` with `denied: false`; the other equal-rank pairs are not constructed |
| `disposition_relaxation_attempted` | `a_dispute_is_denied_against_a_more_restrictive_prior_and_lateral_from_stale` (`kernel_routes.rs:6146`); the grid (`admission.rs:4608`) | `mark_disputed` against `rejected`, `contradicted`, and `quarantined` priors, recorded `denied: true` |
| `admission_write_with_evidence_id` | `a_dispute_citing_live_project_evidence_lands_with_the_id_and_the_reporter` (`kernel_routes.rs:5723`); `kernel_routes.rs:6146`, `:6199` | route disputes whose admission row carries the cited `evidence_id` |
| `disposition_identity_replayed` | `dispute_identities_land_replay_and_refuse_reuse_like_every_envelope` (`kernel_routes.rs:6080`); `memory-mark-command.test.ts:212` | a byte-identical repeat returns `replayed: true` with no new row |
| `disposition_identity_reused` | `kernel_routes.rs:6080` | the same key with another `request_digest` answers `operation_key_reused` |
| `preview_then_commit_same_tip` | `a_dispute_citing_a_repository_file_records_it_in_the_reason` (`kernel_routes.rs:5880`); `assert_refused_without_writing` (`kernel_routes.rs:5703`) used by `:5801`, `:5880`, `:5971` | a preview and a commit of the same operations with no intervening commit |
| `disposition_on_authority_bearer` | `kernel_routes.rs:6199` | a route dispute on an approval that a verified dependent cites |
| `disposition_cascade_demoted_dependents` | `kernel_routes.rs:6199` | the dependent's `approval_revoked` row at the dispute's `commit_seq`, in the same commit as the bearer's row |

The same tests also construct `disposition_subject_classes_differ_from_caller`
(`kernel_routes.rs:5723` disputes route-written subjects under
`source_kind: "user"`; `:6199` disputes an `ExplicitUser` approval under
`assistant`).

Every other marker in the table above stays a queued coverage check with
no owning ticket: `disposition_relaxation_with_valid_approval`,
`disposition_body_with_unknown_key_received`,
`admission_write_with_retired_evidence_id`,
`slice_insert_cites_retired_evidence`, `scoped_lookup_of_evidence_object`,
`preview_then_commit_tip_moved`, `disposition_replayed_after_subject_moved`,
`command_received_mismatched_verdict`, `commit_log_cause_redacted`,
`admission_reason_at_bound`, `admission_reason_redacted`, and
`command_preview_visibility_changes` (named in the
`memory-mark-command-commits-only-the-previewed-operation` record).

## Ranking by cheapest valid oracle

1. Evaluator property tests over `EvaluationInputs` for every
   `EventKind` in the five, every `Disposition` prior, every legal class
   pair, and `approval_valid` in both values: covers
   `disposition-never-raises-maturity-and-withdraws-effective-support` and
   `disposition-approval-requirement-follows-the-restrictiveness-order` at
   unit cost, with `disposition_lateral_move_attempted` and
   `disposition_relaxation_with_valid_approval` as floors.
2. One route integration test that seeds an `ExplicitUser` approval and a
   `Verified` dependent, files `mark_disputed` on each through the route
   under `source_kind: "assistant"`, and reads every column of the written
   rows: covers `disposition-admission-copies-the-subjects-prior-classes`,
   the row half of the maturity record, `disposition-on-an-authority-
   bearer-demotes-dependents-in-the-same-commit`, and `disposition-commit-
   log-row-carries-actor-and-cause-only` in one fixture.
3. One route test with a raw JSON body carrying `evidence_id` on a
   disposition operation plus a read of `evidence_id`/`trigger_object_id`
   on existing rows: covers `disposition-operation-carries-no-evidence-
   and-refuses-unknown-fields`; this is the PR A regression baseline.
4. One kernel integration test: ingest evidence, retire it, call
   `record_admission` with `Some(evidence_id)` (row written) and with a
   dangling id (`Conflict`): covers `admission-evidence-citation-is-
   existence-checked-only`; one route `insert_decision` citing the retired
   id (`not_found`): covers `slice-write-refuses-non-live-evidence-
   citations`.
5. One route test naming an evidence `object_id` as subject, approval, and
   token (`not_found` each): covers `evidence-objects-carry-no-scope-and-
   fail-scoped-lookup`.
6. A differential preview-then-commit workload across all `(event,
   prior)` pairs with and without an interleaved move: covers the preview
   record and the replay-after-move branch; `/testing:stateful-workload-
   design`.
7. TypeScript: extend the `FakeKernelTransport` harness to capture both
   bodies and assert equality; cheap, closes the command record's gap.
8. Reason bound and redaction unit tests: lowest value until OQ4-1 makes
   `reason` a structured carrier.
