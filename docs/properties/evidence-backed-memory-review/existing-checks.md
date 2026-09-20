# Existing checks

Every claim-bearing check in the tree that touches this catalog's records.
Status is `unaudited` for all of them: adequacy belongs to a separate review.

## Canonical resolution and target binding

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `canonical_and_promoted_descriptors_resolve_to_their_originating_decision_and_target_it` | `crates/daemon/tests/memory_reviewer_broker.rs` | both production classes resolve to `CanonicalSource` with the decision's id and live revision; the hold protects exactly the descriptor's evidence; two descriptors bind one equal `CanonicalTarget` whose commit token is the decision's last change, not its creation; `MEMORY_CLASSES` excludes `git_commits`; registry rows equal before and after | unaudited |
| `a_moved_missing_stale_or_wrong_kind_owner_refuses_the_subject_and_the_target` | same | supersession refuses `OriginRevoked` from the bound target and from fresh resolution; a stale descriptor revision refuses `ExpectationChanged` at the broker with zero bytes; an unregistered owner refuses `NotFound`; a stale bound decision revision refuses `ExpectationChanged`; an evidence object as owner refuses `ExpectationChanged` at `proposal_target` and `Scope` at the broker with zero bytes; a decision in another project's scope as owner resolves but refuses `Scope` at the broker with zero bytes; tracked rows equal before and after | unaudited |
| `a_resolved_canonical_subject_still_refuses_the_remote_destination` | same | the Remote destination refuses a resolved canonical artifact `PolicyBlocked` with zero bytes | unaudited |
| `canonical_and_promoted_forms_share_an_origin_and_a_revoked_decision_revokes_both` | same | shared origin across the two forms; Remote `PolicyBlocked`; decision retirement revokes both forms; stale native revision refused | unaudited |
| `a_canonical_owner_must_be_a_live_decision` | same | a scoped observation as owner refuses `ExpectationChanged` | unaudited |
| `a_canonical_subject_resolves_through_its_decision_and_abstains_for_a_remote_model` | `crates/daemon/tests/memory_reviewer_coordinator.rs` | a canonical `ReviewTarget::Memory` subject reaches the coordinator, resolves, and settles `owner_sensitive` with zero connections, no attempt, and the decision and descriptor rows live | unaudited |
| `the_production_classes_are_walked_in_order_and_unproven_canonical_descriptors_are_not_selected` | `crates/daemon/tests/memory_reviewer_selection.rs` | the production walk passes Sensitive canonical descriptors without a job, through both classes and from a mid-walk cursor | unaudited |
| `memory_reviewer_review_selection_is_not_scheduled_while_production_selection_is_closed` | `crates/daemon/src/lib.rs` | an open activation gate schedules no `MemoryReviewerReviewSelection` while `PRODUCTION_SELECTION_OPEN` is `false`; a constant assertion forces the test to change when the gate opens | unaudited |
| `the_selected_classes_are_exactly_the_decision_derived_classes` | `crates/daemon/tests/memory_reviewer_broker.rs` | over every `OccurrenceClass`, `decision_derived` and `MEMORY_CLASSES` membership agree, so no class is selected without resolving through its decision or resolves through a decision without being selectable | unaudited |
| `const` assertion after `MEMORY_CLASSES` | `crates/daemon/src/memory_reviewer/selection.rs` | every walked class satisfies `decision_derived` at compile time; `related_memories::CLASSES` is the same constant, so discovery cannot walk a different set | unaudited |
| `a_selected_eligible_memory_becomes_a_published_proposal_through_the_shared_path` | `crates/daemon/tests/memory_reviewer_coordinator.rs` | Git-only shared path from selection to a readable proposal; a native subject's target is the descriptor | unaudited |

## Suspiciously quiet areas

- No test exercises class tightening (a decision reclassified `Sensitive`
  after resolution) at the coordinator seam; the broker's `judge` folds the
  decision's sensitivity into the descriptor's on every read, which the broker
  tests cover for retirement but not reclassification.
- No test re-publishes the descriptor at a new revision after binding to show
  the bound target is unchanged; each revision is its own object, so the bound
  expectation cannot observe the new row, but no witness records that.
