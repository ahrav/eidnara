# Fault map: use-outcome record

The repository has no campaign runtime. A marker here is a `sometimes`
precondition: a named deterministic test constructs the state and asserts it
before its verdict. Marker names are constant and appear only in this file;
the link between a marker and its test is the Observed column.

## Fault classes

| Fault class | Availability at the #1155 tip |
| --- | --- |
| Direct SQL against a closed or idle store (forbidden transitions, null `commit_seq`, out-of-vocabulary classes, citations) | available; `kernel_schema.rs`, `kernel_use_outcome.rs` |
| A citation committed between sweep selection and the retiring commit | available; the in-module hook `expire_use_outcomes_after_selection` in `crates/kernel/src/use_outcome.rs` |
| A family at another baseline | available; `crates/kernel/tests/fixtures/prior_kernel_baseline.sql` |
| Two disposition events in one commit | available by SQL and through the admission API |
| A CAS conflict inside a fold | available; the one-shot per-session attempt hook |
| A fault between the memory-store commit of a fold and the Kernel write | available; the per-handler `use_outcome_fault` injection, taken at the hand-off (`crates/daemon/src/lib.rs:9697`) |
| A stop between the stores on `session.delete` | available; `stop_after_retirement` |
| A Kernel Starting or Unavailable at a read or write | available; the Kernel coordinator test states |
| A config reload between bind and a later pass or wrapup | available |

## Per-property required faults

| Record | Required faults and enabling state |
| --- | --- |
| `uo-served-rows-are-the-frozen-m0-rows-between-hard-folds` | A session with a HARD fold over a Canonical composition, then a Kernel commit that changes the visible rows, then a non-HARD pass. |
| `uo-composition-revision-digests-exactly-the-rendered-rows` | Snapshots that differ only in rows the renderer drops, bytes past the render cap, or rows past the memory budget. |
| `uo-composition-freeze-persists-at-every-hard-fold-site` | A HARD fold on each engine, a pressure refold on the folding engine, and a CAS conflict inside one pass. |
| `uo-subagent-pass-serves-no-composition-under-eidnara-folds` | A route with `eidnara_folds` (compaction on, a model chain) and a subagent request; the same request under native authority. |
| `uo-known-as-of-is-the-tip-bound-read-sequence` | A Canonical read with a concurrent Kernel commit. |
| `uo-memory-disabled-route-takes-no-canonical-read` | A route bound with `/memory/enabled: false`. |
| `uo-consent-gate-source-is-the-route-frozen-config` | A route bound with memory on, then a reload that turns it off (and the reverse), then a pass and a wrapup. |
| `uo-disputed-decision-is-hidden-on-auto-inject-and-labeled-on-explicit-search` | A decision admitted, then a committed `mark_disputed` on it. |
| `uo-admission-reads-require-a-non-null-commit-seq` | A row whose `commit_seq` is nulled by direct SQL. |
| `uo-disposition-events-append-rows-and-newest-by-commit-seq-wins` | A `mark_stale` and a `mark_disputed` committed together. |
| `uo-capture-expiry-retires-at-most-a-page-per-call-under-reserved-receipts` | More than 64 expired captures; a hold acquired between selection and commit. |
| `uo-expiry-keeps-pinned-and-cited-evidence` | An expired capture cited by a live observation; one pinned by a live hold. |
| `uo-session-delete-retires-kernel-sources-before-memory-rows-under-the-capture-gate` | A session with captured sources; the `stop_after_retirement` hook. |
| `uo-route-teardown-purges-process-state-but-deletes-no-durable-row` | A route closed without `session.delete` after a HARD fold. |
| `uo-per-session-pass-timestamps-die-with-the-session` | A session with committed passes, then `session.delete`. |
| `uo-kernel-opens-only-at-the-compiled-baseline-identity` | A family at another baseline, with a valid marker naming that baseline. |
| `uo-activation-gate-closes-on-kernel-baseline-or-incarnation-change` | An activation record written before the rotation. |
| `uo-sensitive-class-denies-remote-and-unknown-egress-is-local-only` | A sensitive row presented to the rule; an out-of-vocabulary egress string by SQL. |
| `uo-status-counters-are-closed-vocabulary-exact-integers-with-unavailable-not-zero` | A `memory_reviewer` block in a non-ready state with zero counters. |
| `uo-review-reads-authorize-by-bearer-and-project-binding-not-owner` | An observational route and an ordinary route bound to one project. |
| `uo-p-baseline-rotation-refuses-previous-family-untouched` | A fixture database at the `a0e3f2993` digest (`13437d62f69c77d1f5bdad72195473d1f409c2647efecdcac660554e5c1db07b`) opened by this binary. |
| `uo-p-served-set-and-identity-never-rewritten` | Direct SQL for each forbidden transition, including REPLACE. |
| `uo-p-classification-sensitive-local-only-no-remote-egress` | Inserts with `normal` and `remote_allowed`; a serve through the envelope. |
| `uo-p-disputed-after-serve-from-committed-events-only` | A serve at n; dispositions at n + k, at n, and before n; a null row and another lineage by SQL. |
| `uo-p-sweep-retires-only-expired-uncited-rows` | An expired record; an expired cited record; 70 expired records; a citation inserted after selection. |
| `uo-p-record-survives-session-delete` | A session with records, then a delete (Kernel arm: direct SQL deletes). |
| `uo-p-served-set-once-per-session-revision` | Two HARD folds with no Kernel change between; a CAS retry. |
| `uo-p-served-ids-equal-frozen-composition` | A read trimmed by the token budget; a CAS retry. |
| `uo-p-no-row-when-memory-disabled` | A route bound with `/memory/enabled: false`, then HARD folds on both engines. |
| `uo-p-no-row-for-withheld-composition` | A Kernel starting or a refusing serving verdict at read time, then a HARD fold. |
| `uo-p-producer-matches-request-subagent-flag` | A subagent request on the additive engine with a Canonical read; a primary on each engine. |
| `uo-p-deleted-at-iff-session-delete-kernel-step` | A delete with the Kernel available; one with it starting; a route close without delete. |
| `uo-p-last-transform-at-monotone-per-committing-pass` | A committing then a non-committing pass; a Kernel unavailable at the write. |
| `uo-p-counts-exact-and-unavailable-not-zero` | A Kernel starting; one decision served to two sessions with one later dispute. |

## Coverage markers

| Marker | Enabling state (assert before the check) | Owner | Observed | Records |
| --- | --- | --- | --- | --- |
| `uo_hard_fold_after_disposition_commit` | A disposition commit lands between a HARD fold and the next pass | #1155 | `a_dispute_between_folds_records_the_next_served_set_without_it` (`crates/daemon/src/use_outcome_writer_tests.rs:468`): a `mark_disputed` commits between the first HARD and the next pass | `uo-served-rows-are-the-frozen-m0-rows-between-hard-folds`, `uo-p-served-ids-equal-frozen-composition` |
| `uo_same_revision_at_later_known_as_of` | A HARD fold at an equal served set reads at a later `known_as_of` | #1155 | `an_equal_served_set_at_a_later_known_as_of_replays` (`crates/daemon/src/use_outcome_writer_tests.rs:432`): the later fold's `known_as_of` is asserted greater before the replay is checked | `uo-p-served-set-once-per-session-revision` |
| `uo_withheld_then_available_same_rows` | A withheld read followed by an Available read of the same rows | #1155 | `a_withheld_read_then_an_available_read_of_the_same_rows_records_once` (`crates/daemon/src/use_outcome_writer_tests.rs:504`) | `uo-p-no-row-for-withheld-composition` |
| `uo_pressure_refold_froze_composition` | A pressure refold on the folding engine assigns a composition | #1155 | `a_pressure_refold_reports_its_frozen_composition` (`crates/daemon/src/transform.rs:20895`): `materialize_reason` is asserted `pressure_refold`, and the reloaded `meta.project_memory` equals the pinned read's composition | `uo-composition-freeze-persists-at-every-hard-fold-site` |
| `uo_subagent_pass_under_native_authority_served_m0` | A subagent pass under native authority is served an m0 composition | #1155 | `the_session_role_follows_the_pass_subagent_flag_on_each_engine` (`crates/daemon/src/use_outcome_writer_tests.rs:369`), native arm | `uo-p-producer-matches-request-subagent-flag`, `uo-subagent-pass-serves-no-composition-under-eidnara-folds` |
| `uo_subagent_pass_under_eidnara_folds_planned_soft_or_defer` | A subagent pass under `eidnara_folds` plans Soft or Defer | #1155 | `the_session_role_follows_the_pass_subagent_flag_on_each_engine` (`crates/daemon/src/use_outcome_writer_tests.rs:369`), folding arm: the action is asserted not `HARD`, the response carries no composition, and its bytes carry no `<project-memory>` block | `uo-subagent-pass-serves-no-composition-under-eidnara-folds` |
| `uo_cas_retry_with_same_pinned_read` | A CAS conflict re-runs the fold attempt with the same pinned read | #1155 | `a_discarded_attempt_leaves_no_serve_and_the_retry_records_one` (`crates/daemon/src/use_outcome_writer_tests.rs:298`): the rival commit is asserted to have run, and the record's ids, `served_commit_seq`, and revision are compared with the retry's persisted composition | `uo-p-served-set-once-per-session-revision`, `uo-composition-freeze-persists-at-every-hard-fold-site` |
| `uo_wrapup_fired_under_reloaded_config` | A wrapup fires after a config reload changed `memory.enabled` | #1155 | partial - `a_wrapup_under_a_config_that_differs_from_the_binding_records_no_serve` (`crates/daemon/src/use_outcome_writer_tests.rs:539`): the route is bound under a config that differs from the live one, so the wrapup's read diverges from the frozen binding; a reload after binding is not performed | `uo-consent-gate-source-is-the-route-frozen-config` |
| `uo_two_disposition_events_one_commit` | Two qualifying disposition events share one commit | #1154 | `disputed_after_serve_counts_only_later_committed_disputes_on_the_same_lineage` (`crates/kernel/tests/kernel_use_outcome.rs:400`): rows `a8` and `a9` share the `after` commit; the test asserts both rows share that sequence before it reads the flag | `uo-p-disputed-after-serve-from-committed-events-only`, `uo-disposition-events-append-rows-and-newest-by-commit-seq-wins` |
| `uo_expiry_page_full` | More expired uncited records than one page holds | #1154 | `an_expiry_backlog_drains_one_page_per_call` (`crates/kernel/tests/kernel_use_outcome.rs:595`): 70 eligible records against a page of 64, asserted before the first sweep | `uo-p-sweep-retires-only-expired-uncited-rows` |
| `uo_expiry_retained_cited_row` | An expired record with a committed citation whose subject is live | #1154 | `an_expired_record_cited_by_a_live_report_is_retained` (`crates/kernel/tests/kernel_use_outcome.rs:665`): record `s1` is expired and cited by a committed row whose subject is live, asserted before the sweep | `uo-p-sweep-retires-only-expired-uncited-rows` |
| `uo_delete_stopped_between_stores` | `session.delete` stops after its Kernel step and before the memory delete | #1156 | not yet | `uo-p-deleted-at-iff-session-delete-kernel-step`, `uo-p-record-survives-session-delete` |
| `uo_route_torn_down_after_hard` | A route is torn down after a HARD fold without `session.delete` | #1156 | not yet | `uo-route-teardown-purges-process-state-but-deletes-no-durable-row` |
| `uo_status_block_non_ready_with_counters_present` | A non-ready status block carries counter values | #1157 | not yet | `uo-status-counters-are-closed-vocabulary-exact-integers-with-unavailable-not-zero` |
| `uo_observational_route_read_review_outcome` | An observational `cli` route reads the use-outcome counts | #1157 | not yet | `uo-review-reads-authorize-by-bearer-and-project-binding-not-owner` |

## Leverage ranking

1. Direct SQL against the trigger and CHECK floor: one connection proves every
   immutability and classification obligation at once
   (`uo-p-served-set-and-identity-never-rewritten`,
   `uo-p-classification-sensitive-local-only-no-remote-egress`).
2. The Kernel store API with rows inserted by SQL: proves the dispute join and
   the sweep, including the cited-row exception, before any production writer
   exists.
3. The previous-baseline fixture: one open proves the rotation.
4. Daemon route tests with real stores (#1155, #1156): the write and lifecycle
   obligations; the post-commit fault hook is the one new seam.
5. The CLI contract test (#1157): the exact-integer boundary.
