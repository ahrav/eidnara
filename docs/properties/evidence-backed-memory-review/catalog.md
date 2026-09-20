# Evidence-backed memory review: catalog

Method: `../METHOD.md`. Records were verified against the daemon and kernel
crates at the change that introduced `resolve_descriptor` and `proposal_target`
in `crates/daemon/src/memory_reviewer/coordinator.rs`. References name functions and
tests rather than line numbers.

## Scope

The MemoryReviewer review path from production selection through the coordinator,
broker, settlement, Memory Store ledger, Kernel holds, and the protocol 3
review reads, plus the packaged review commands that consume them. This part
holds records only for slugs whose ticket has landed; the index lists every
slug the specification assigns.

## Reachability classes

- `default-production`: reached by the daemon's worker, scheduler, or routed
  request with no configuration beyond an open activation gate.
- `explicit-config-only`: reached only when an operator opens a gate or
  constant that ships closed.
- `test-only`: constructed only by a test.

## Index

Every slug the seven residual tickets own. Slugs the specification assigns to other owners are not indexed here.

| Slug | Ticket | Record |
| --- | --- | --- |
| `production-classes-reach-policy-eligible-proposal` | #725 | yes |
| `canonical-resolution-refuses-changed-owner-and-target` | #725 | yes |
| `private-result-transfer-preserves-queue-expiry` | #726 | not yet |
| `receipt-selection-fences-private-generation-results` | #727 | not yet |
| `durable-private-result-recovers-without-model-refire` | #727 | not yet |
| `unknown-dispatch-does-not-authorize-resend` | #727 | not yet |
| `uncited-owner-lineage-remains-read-authority` | #727 | not yet |
| `observer-route-does-not-change-background-rosters` | #728 | not yet |
| `status-sanitizer-preserves-inclusive-integer-domain` | #729 | not yet |
| `completed-outcome-pages-have-live-keyset-semantics` | #730 | not yet |
| `shared-path-fixture-reaches-selected-readable-proposal` | #730 | not yet |
| `reference-only-cli-outcomes-preserve-meaning` | #730 | not yet |
| `review-cli-owns-one-replay-free-connection` | #730 | not yet |
| `review-cli-validates-byte-exact-inert-payloads` | #730 | not yet |
| `review-cli-preserves-shared-kernel-refusal-shapes` | #730 | not yet |
| `status-freshness-never-defaults-unknown-to-zero` | #730 | not yet |
| `status-counts-preserve-overlapping-ledger-populations` | #730 | not yet |
| `provider-response-budget-is-cumulative-per-job` | #731 | not yet |
| `attempt-ledger-preserves-cross-generation-ceilings` | #731 | not yet |
| `guarded-request-handoff-precedes-network-polling` | #731 | not yet |

## Records

### production-classes-reach-policy-eligible-proposal

Type: reachability
Reachability: explicit-config-only
Status: active
Exercised: not yet - no Remote-eligible representation of a canonical or promoted artifact exists, so no run over a production class can reach a selected proposal; `PRODUCTION_SELECTION_OPEN` is `false`
Guarantee: When production selection is open, a job over `canonical_claims` or `promoted_memory` reaches a receipt-selected, readable, non-abstaining proposal only through a genuinely eligible originating decision under existing policy, with the request ledger, disclosed bytes, and selected receipt inspectable.
Check: `sometimes` - a campaign with production selection open must produce at least one completed receipt whose selected proposal targets a decision reached through `ReferenceExpectation::CanonicalSource`; the situation, not the branch, is what matters, and it cannot occur while the gate is closed
Fault/timing angle: none
Required faults and enabling state: `PRODUCTION_SELECTION_OPEN` set `true`; an owner-approved Remote-eligible decision representation; an open activation gate; a live scoped decision with a canonical descriptor whose artifact egress is `Allowed` for `ArtifactDestination::Remote`
Confidence: high - [evidence](evidence/production-classes-reach-policy-eligible-proposal.md). Verified that `resolve_descriptor` returns `CanonicalSource` for both production classes instead of refusing them as `Unsupported`, that a canonical subject reaches the coordinator and settles `owner_sensitive` with zero requests, and that the Remote broker refuses the artifact with `PolicyBlocked` and zero disclosed bytes
Existing check: `crates/daemon/tests/memory_reviewer_broker.rs::canonical_and_promoted_descriptors_resolve_to_their_originating_decision_and_target_it` covers resolution; `a_resolved_canonical_subject_still_refuses_the_remote_destination` covers the Remote refusal; `crates/daemon/tests/memory_reviewer_coordinator.rs::a_canonical_subject_resolves_through_its_decision_and_abstains_for_a_remote_model` covers the coordinator path; `crates/daemon/tests/memory_reviewer_selection.rs::the_production_classes_are_walked_in_order_and_unproven_canonical_descriptors_are_not_selected` covers the empty production walk; `crates/daemon/src/lib.rs::memory_reviewer_review_selection_is_not_scheduled_while_production_selection_is_closed` covers the scheduler gate
Impact: A positive result claimed without this witness would report scripted Git-only coverage as production eligibility
Open questions:
- Which decision bytes may reach the Remote destination, and which Kernel-owned proof binds them to the decision's id, revision, and scope (needs human input)

### canonical-resolution-refuses-changed-owner-and-target

Type: safety
Reachability: explicit-config-only - the only production producer of a `ReviewTarget::Memory` job over `canonical_claims` or `promoted_memory` is the selection walk in `crates/daemon/src/memory_reviewer/selection.rs`, which `SchedulerBridge::scheduled_projects` schedules only while `PRODUCTION_SELECTION_OPEN` is `true`; the History Summarizer produces staged subjects
Status: active
Exercised: yes - `crates/daemon/tests/memory_reviewer_broker.rs` constructs a superseded owner, a retired owner, a never-registered owner, a stale descriptor revision, a stale bound decision revision, an owner that is not a decision, an owner scoped to another project, and the Remote destination
Guarantee: A proposal over a canonical or promoted descriptor names the originating decision's exact object id, source revision, snapshot, and last-change commit token, never the descriptor; an owner that is missing, retired, superseded, re-revised, wrong-kind, or out-of-scope when the subject is read refuses before any byte is disclosed; an owner that moves after disclosure refuses at target binding, so no proposal is bound to it; and descriptor identity cannot redirect the target.
Check: `always` - every `ProposalTarget::Memory` produced from a `CanonicalSource` subject has `object_id == originating_decision_id` and `source_revision == decision_source_revision`; `resolve_descriptor` refuses an absent (`NotFound`) or invalidated (`OriginRevoked`) decision and otherwise binds the live registry row, whatever its kind; `proposal_target` refuses an invalidated decision (`OriginRevoked`) and a row that is not a decision or is at another revision (`ExpectationChanged`); the broker's `judge_canonical_source` refuses a wrong-kind or out-of-scope owner (`Scope`) and a stale revision (`ExpectationChanged`) before any byte is read; asserted on every evaluation because the target is the mutation token a later application compares against
Fault/timing angle: the decision changes between resolution and proposal binding; bytes disclosed before the change stay disclosed and the refusal lands at binding
Required faults and enabling state: `correct_decision` superseding the bound decision after `resolve_descriptor`; a descriptor whose identity names an unregistered decision; a bound expectation whose `decision_source_revision` disagrees with the live row; a stale descriptor revision; a descriptor whose owner is an evidence object; a descriptor whose owner is a decision in another project's scope
Confidence: high - [evidence](evidence/canonical-resolution-refuses-changed-owner-and-target.md). Verified each refusal code, that two descriptors of one decision produce one equal target whose commit token is the decision's last change rather than its creation, and that resolution and binding leave the tracked registry rows equal
Existing check: `crates/daemon/tests/memory_reviewer_broker.rs::canonical_and_promoted_descriptors_resolve_to_their_originating_decision_and_target_it`, `a_moved_missing_stale_or_wrong_kind_owner_refuses_the_subject_and_the_target`, `canonical_and_promoted_forms_share_an_origin_and_a_revoked_decision_revokes_both`, `a_canonical_owner_must_be_a_live_decision`
Impact: A proposal bound to the descriptor or to a moved decision would carry a mutation token that a later application could apply to the wrong object or revision
Open questions: None.

## Relationship map

`canonical-resolution-refuses-changed-owner-and-target` is the safety half of
`production-classes-reach-policy-eligible-proposal`: the same resolution path
must reach a proposal when eligible and refuse when the owner or target moved.
Both consume the broker's canonical judgement (`judge_canonical_source`) and the
Kernel's egress fold, which `served-sensitivity-and-artifact-policy-govern-egress`
in `../canonical-positive-claim-projection/` records.
