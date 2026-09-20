# Existing checks

Every claim-bearing check in the tree that touches this catalog's records.
Status is `unaudited` for all of them: adequacy belongs to a separate review.

## Canonical resolution and target binding

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `canonical_and_promoted_descriptors_resolve_to_their_originating_decision_and_target_it` | `crates/daemon/tests/memory_reviewer_broker.rs` | both production classes resolve to `CanonicalSource` with the decision's id and live revision; the hold protects exactly the descriptor's evidence; two descriptors bind one equal `CanonicalTarget` whose commit token is the decision's last change, not its creation; `MEMORY_CLASSES` excludes `git_commits`; registry rows equal before and after | unaudited |
| `a_moved_missing_stale_or_wrong_kind_owner_refuses_the_subject_and_the_target` | same | supersession refuses `OriginRevoked` from the bound target and from fresh resolution; a stale descriptor revision refuses `ExpectationChanged` at the broker with zero bytes; an unregistered owner refuses `NotFound`; a stale bound decision revision refuses `ExpectationChanged`; an evidence object as owner refuses `ExpectationChanged` at `proposal_target` and `Scope` at the broker with zero bytes; tracked rows equal before and after | unaudited |
| `a_resolved_canonical_subject_still_refuses_the_remote_destination` | same | the Remote destination refuses a resolved canonical artifact `PolicyBlocked` with zero bytes | unaudited |
| `canonical_and_promoted_forms_share_an_origin_and_a_revoked_decision_revokes_both` | same | shared origin across the two forms; Remote `PolicyBlocked`; decision retirement revokes both forms; stale native revision refused | unaudited |
| `a_canonical_owner_must_be_a_live_decision` | same | a scoped observation as owner refuses `ExpectationChanged` | unaudited |
| `a_canonical_subject_resolves_through_its_decision_and_abstains_for_a_remote_model` | `crates/daemon/tests/memory_reviewer_coordinator.rs` | a canonical `ReviewTarget::Memory` subject reaches the coordinator, resolves, and settles `owner_sensitive` with zero connections, no attempt, and the decision and descriptor rows live | unaudited |
| `the_production_classes_are_walked_in_order_and_unproven_canonical_descriptors_are_not_selected` | `crates/daemon/tests/memory_reviewer_selection.rs` | the production walk passes Sensitive canonical descriptors without a job, through both classes and from a mid-walk cursor | unaudited |
| `memory_reviewer_review_selection_is_not_scheduled_while_production_selection_is_closed` | `crates/daemon/src/lib.rs` | an open activation gate schedules no `MemoryReviewerReviewSelection` while `PRODUCTION_SELECTION_OPEN` is `false`; a constant assertion forces the test to change when the gate opens | unaudited |
| `the_selected_classes_are_exactly_the_decision_derived_classes` | `crates/daemon/tests/memory_reviewer_broker.rs` | over every `OccurrenceClass`, `decision_derived` and `MEMORY_CLASSES` membership agree, so no class is selected without resolving through its decision or resolves through a decision without being selectable | unaudited |
| `const` assertion after `MEMORY_CLASSES` | `crates/daemon/src/memory_reviewer/selection.rs` | every walked class satisfies `decision_derived` at compile time; `related_memories::CLASSES` is the same constant, so discovery cannot walk a different set | unaudited |
| `a_selected_eligible_memory_becomes_a_published_proposal_through_the_shared_path` | `crates/daemon/tests/memory_reviewer_coordinator.rs` | Git-only shared path from selection to a readable proposal; a native subject's target is the descriptor | unaudited |

## Private result expiry and reconciliation

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `review_transfer_acquires_before_releasing_and_moves_only_live_memory_reviewer_references` | `crates/kernel/tests/kernel_memory_reviewer_holds.rs` | candidate and run deadlines equal the queue deadline after transfer; live read at the deadline is `Expired`; selected read with a selection inside the window succeeds past the deadline, and one at the deadline refuses | unaudited |
| `an_unselected_transferred_result_expires_with_its_queue_and_its_hold_is_reconciled` | `crates/daemon/tests/memory_reviewer_settlement.rs` | hold outlives the queue on its own clock; row keeps the queue deadline; reconciler keeps a pending hold; sweep closes the receipt `expired` at the run deadline; list shows the terminal; read is `not_selected`; reconciler releases the orphaned hold once and finds nothing twice; a late completion is fenced | unaudited |
| `a_completed_receipt_selects_the_staged_proposal_and_reads_pass_the_kernel` | same | selection leaves the row deadline; `completed_at_ms` is the settlement clock; a sweep at the queue deadline leaves the complete receipt, its completion time, its selection, and its read untouched; read past the queue deadline succeeds while the hold is live and refuses `review_expired` at its expiry; live read past the deadline is `Expired`; reconciler leaves the selected hold | unaudited |
| `kernel_results_stay_private_until_the_receipt_selects_them` | same | sealed row readable by identity, public read `not_selected`, list empty; same-generation recovery reuses the hold; abstaining recovery releases it | unaudited |
| `the_sweep_closes_an_in_progress_receipt_at_the_queue_deadline_before_its_run_deadline` | `crates/memory-store/tests/memory_reviewer_ledger.rs` | a receipt with a run deadline past the queue deadline closes `expired` at the queue deadline with `completed_at_ms` set and no selection; the job expires with it; the reconciler's two questions answer false and empty | unaudited |
| `an_attempt_never_outlives_the_job_queue_deadline` | same | completion at the queue deadline is stale and writes nothing | unaudited |
| `a_selected_result_answers_only_for_its_project_digest_and_generation` | same | the reconciler's selection listing carries exactly the selection's digest, candidate, and generation, so another digest, another generation, or another candidate does not match | unaudited |
| `the_reconciler_releases_a_losing_generations_hold_and_keeps_the_winners` | `crates/daemon/tests/memory_reviewer_settlement.rs` | after a takeover the losing generation's hold is released as an orphan while the receipt is in progress at the next generation; the winner's hold survives its selection; the losing row stays sealed and unreadable through the receipt | unaudited |
| `a_sweep_inside_the_settlement_window_fences_the_selection_and_releases_the_hold` | same | the sweep closes the receipt `expired` between the Kernel envelope and the completion write; the completion is fenced; the settlement releases the hold; the reconciler finds nothing | unaudited |
| `a_selected_row_whose_owner_or_class_changed_refuses_the_read` | same | a stored witness naming another generation refuses `scope_mismatch`; a row reclassified `secret` refuses `dependency_refused`; the receipt is unchanged | unaudited |

## Suspiciously quiet areas

- No test exercises class tightening (a decision reclassified `Sensitive`
  after resolution) at the coordinator seam; the broker's `judge` folds the
  decision's sensitivity into the descriptor's on every read, which the broker
  tests cover for retirement but not reclassification.
- No test places the originating decision in another project's scope; the
  Kernel's `WrongScope` verdict on the decision candidate maps to `Scope` in
  `judge`, but only the staged-subject scope test constructs it.
- No test re-publishes the descriptor at a new revision after binding to show
  the bound target is unchanged; each revision is its own object, so the bound
  expectation cannot observe the new row, but no witness records that.
- No test drives the daemon lifecycle loop end to end through
  `sweep_and_sample` with a transferred hold; the reconciler is exercised
  directly, and the loop's ordering (sweep, reconcile, Kernel maintenance) is
  read from code.
- No test reopens either store between the Kernel envelope and the sweep; the
  durable rows are read through the same handles that wrote them.
- No test constructs the window after the Kernel result and before the hold
  transfer; the execution hold expires on its own cutoff and the row keeps its
  queue deadline by the same mechanism the post-transfer tests witness.
