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
| `private-result-transfer-preserves-queue-expiry` | #726 | yes |
| `receipt-selection-fences-private-generation-results` | #727 | yes |
| `durable-private-result-recovers-without-model-refire` | #727 | yes |
| `unknown-dispatch-does-not-authorize-resend` | #727 | yes |
| `uncited-owner-lineage-remains-read-authority` | #727 | yes |
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

### private-result-transfer-preserves-queue-expiry

Type: safety
Reachability: default-production - the transfer runs inside `Settlement::settle` (`crates/daemon/src/memory_reviewer/settlement.rs::transfer_retention`), which the coordinator calls at the end of every run the production worker starts; `crates/daemon/src/lib.rs` spawns `memory_reviewer::worker::run` once the Kernel is ready and the worker claims Ready jobs from both producers whenever the activation gate is open, independent of `PRODUCTION_SELECTION_OPEN`. The sweep and the reconciler run in `memory_reviewer::lifecycle::run`, spawned by `crates/daemon/src/lib.rs` as soon as the Memory Store is installed, with no gate. The selected read is the routed `review.read` request (`crates/daemon/src/memory_reviewer/wire.rs::handle_review_read`), dispatched from `crates/daemon/src/lib.rs` for any bound route whose memories authority is MODULE
Status: active
Exercised: yes - `crates/kernel/tests/kernel_memory_reviewer_holds.rs` transfers a hold and reads the row live and as selected on either side of the queue deadline; `crates/daemon/tests/memory_reviewer_settlement.rs` closes an in-progress receipt by sweep after transfer, lists and reads it, reconciles the hold, fences a late completion, sweeps inside the settlement window, orphans a losing generation's hold through takeover, and refuses a selected row whose owner or class changed; `crates/memory-store/tests/memory_reviewer_ledger.rs` closes a receipt at a queue deadline earlier than its run deadline and scopes the selection question
Guarantee: A private Kernel result not selected by a completed receipt remains subject to the job's original queue deadline after execution-to-review hold transfer: the candidate and run rows keep that deadline, a live read past it refuses `Expired`, the public read answers `not_selected`, and the sweep closes the receipt at the earlier of run and queue deadlines. A result selected before the queue deadline is read against its selection time and stays readable only while its review hold is live and its inputs pass policy. Selection, rejection, cancellation, expiry, and reconciliation never move a deadline or publish an unselected row.
Check: `always` - after `transfer_execution_to_review`, `candidates.lease_expires_at` and `extraction_runs.lease_expires_at` equal the job's `queue_deadline_ms`; `read_selected_review_input` refuses when `selected_at >= deadline`; `read_selected_proposal` succeeds only for a `complete` receipt whose `completed_at_ms` precedes the deadline and whose hold is live; asserted on every evaluation because a moved deadline would let an unselected result outlive its queue
Fault/timing angle: crash after the Kernel envelope committed and before the Memory Store completion; sweep and selection racing at the deadline; a run deadline later than the queue deadline
Required faults and enabling state: a sealed proposal row and transferred hold with no completed receipt; the sweep at the run deadline or the queue deadline, whichever is earlier; a late completion under the original claim after the terminal
Confidence: high - [evidence](evidence/private-result-transfer-preserves-queue-expiry.md). Verified the removed deadline promotion, the new selected-read predicate, the ledger's completion fence on the queue deadline, and the reconciler's release of an orphaned hold
Existing check: `crates/kernel/tests/kernel_memory_reviewer_holds.rs::review_transfer_acquires_before_releasing_and_moves_only_live_memory_reviewer_references`; `crates/daemon/tests/memory_reviewer_settlement.rs::an_unselected_transferred_result_expires_with_its_queue_and_its_hold_is_reconciled`, `a_completed_receipt_selects_the_staged_proposal_and_reads_pass_the_kernel`, `kernel_results_stay_private_until_the_receipt_selects_them`, `the_reconciler_releases_a_losing_generations_hold_and_keeps_the_winners`, `a_sweep_inside_the_settlement_window_fences_the_selection_and_releases_the_hold`, `a_selected_row_whose_owner_or_class_changed_refuses_the_read`; `crates/memory-store/tests/memory_reviewer_ledger.rs::the_sweep_closes_an_in_progress_receipt_at_the_queue_deadline_before_its_run_deadline`, `an_attempt_never_outlives_the_job_queue_deadline`, `a_selected_result_answers_only_for_its_project_digest_and_generation`
Impact: An unselected result would stay readable by identity, and its evidence held, for up to seven days past the queue that admitted it
Open questions:
- Whether the capture `retain_until` promotion at transfer should also stay at the queue deadline for an unselected result; the retention floor is a resource bound, not a visibility path, and is left in place (needs human input)

### receipt-selection-fences-private-generation-results

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `crates/daemon/tests/memory_reviewer_settlement.rs` stages a generation-1 result, takes the receipt over at generation 2, and adopts and reads under generation 2
Guarantee: A Kernel result stays private to its generation: only the completed receipt of the same generation selects it, a successor generation adopts only a result at its own provisional identity, and the losing generation's row stays sealed, unpublished, and unreadable through the receipt.
Check: `always` - `adopt` and `read_selected_proposal` derive the candidate id from `(causal_identity, receipt.generation)` and refuse any other row; asserted on every evaluation because a row at another generation is another run's result
Fault/timing angle: takeover between a run's Kernel envelope and its Memory Store completion
Required faults and enabling state: a sealed row and transferred hold at generation 1; `take_over_memory_reviewer_receipt` to generation 2; an adopt at generation 2
Confidence: high - [evidence](evidence/receipt-selection-fences-private-generation-results.md). Verified the provisional identity derivation on both the adopt and the read paths and the takeover test's outcomes
Existing check: `crates/daemon/tests/memory_reviewer_settlement.rs::a_resumed_claim_adopts_the_durable_result_without_the_broker_or_a_new_request` (second half), `a_takeover_fences_the_losing_generation_and_selects_only_its_own_result`, `the_reconciler_releases_a_losing_generations_hold_and_keeps_the_winners`
Impact: A successor could publish a result whose lineage and marker belong to a fenced generation
Open questions: None.

### durable-private-result-recovers-without-model-refire

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `crates/daemon/tests/memory_reviewer_settlement.rs` drops the run's broker and adopts under a fresh one with no aliases and no execution hold; `crates/daemon/tests/memory_reviewer_coordinator.rs` runs a resumed generation against a live peer that receives nothing
Guarantee: A valid same-generation claim that finds a sealed result adopts it from the row's dependency record alone, publishing the byte-identical reference with zero model requests; a record that fails revalidation or is absent refuses without content, without a request, and without rewriting the row.
Check: `always` - `Settlement::adopt` never reaches the disclosure path; the coordinator returns from `adopt` before `open`; asserted on every resumed run because a compensating request is the failure this record forbids
Fault/timing angle: process loss after the Kernel envelope and before the Memory Store completion; process loss after the completed marker and before staging
Required faults and enabling state: a completed marker at the generation; a sealed row with a record, or no row; a fresh broker under the transferred review hold or under an execution hold covering nothing; a retired member, an edited record, and a record removed from the witness
Confidence: high - [evidence](evidence/durable-private-result-recovers-without-model-refire.md). Verified the record's contents after staging, the adopt outcomes for each fault, and the zero-connection assertion at the coordinator
Existing check: `crates/daemon/tests/memory_reviewer_settlement.rs::a_resumed_claim_adopts_the_durable_result_without_the_broker_or_a_new_request`, `a_result_sealed_before_its_transfer_is_adopted_under_an_empty_execution_hold`, `a_durable_result_whose_lineage_moved_or_lacks_a_record_is_not_adopted`, `a_proposal_without_a_completed_marker_at_its_generation_is_not_staged`; `crates/daemon/tests/memory_reviewer_coordinator.rs::a_resumed_generation_adopts_the_result_a_lost_run_sealed_without_a_send`, `a_resumed_generation_with_a_cancelled_marker_completes_unknown_without_a_send`; `crates/kernel/tests/kernel_review_staging.rs::schema_illegal_proposals_are_refused_at_decode`; `crates/context-core/src/memory_reviewer_policy_union.rs::decoding_accepts_only_bytes_that_re_encode_to_themselves_and_their_digest`
Impact: A restart would spend a second physical request and a second attempt for a result the store already holds, or publish a result nothing revalidated
Open questions: None.

### unknown-dispatch-does-not-authorize-resend

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `crates/daemon/tests/memory_reviewer_coordinator.rs` plants an unterminated marker, a cancelled marker, and a `not_dispatched` marker before a run against a live peer
Guarantee: A marker at the run's generation other than a proven `not_dispatched`, or an unterminated or `unknown` marker at any generation, ends the resumed run without a request: the receipt completes `unknown` when no sealed row exists, and adopts or abstains on the row when one does. Only `not_dispatched` markers, or none, admit a newly charged attempt, under the original identity, deadlines, and remaining count.
Check: `always` - `resumes_dispatched_work` is evaluated in `prepare` before subject resolution and hold growth; a `true` result skips both and routes to `adopt`; asserted on every run start
Fault/timing angle: crash between marker commit and terminal write; cancellation mid-attempt; a lapsed recheck after commit
Required faults and enabling state: `dispatch_memory_reviewer_attempt` with no terminal; a cancelled first run; a recheck clock past the attempt deadline
Confidence: high - [evidence](evidence/unknown-dispatch-does-not-authorize-resend.md). Verified the three marker classes at the coordinator with connection counts and attempt counts
Existing check: `crates/daemon/tests/memory_reviewer_coordinator.rs::an_unknown_attempt_outcome_completes_unknown_and_cancellation_joins_the_attempt`, `a_resumed_generation_with_a_cancelled_marker_completes_unknown_without_a_send`, `a_not_dispatched_marker_alone_lets_the_run_proceed_with_a_new_attempt`, `a_takeover_past_an_unterminated_marker_completes_unknown_without_a_send`
Impact: A lost answer would be retried with a fresh physical request, exceeding the attempt and byte ceilings the marker already charged
Open questions:
- Whether a `failed` or `cancelled` marker with no sealed row should complete the receipt as `unknown` or under its own terminal; the ledger's sweep maps a cancelled receipt to `cancelled`, while the resumed run maps every non-`not_dispatched` marker to `unknown` (needs human input)

### uncited-owner-lineage-remains-read-authority

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `crates/daemon/tests/memory_reviewer_settlement.rs` retires an uncited source after selection and reads; a fabricated canonical member is refused on read
Guarantee: The selected read revalidates every persisted union member, cited or not, including each canonical member's originating decision and owner revision, and refuses the proposal when any no longer stands, while the receipt continues to select it.
Check: `always` - `read_selected_proposal` calls `dependencies::revalidate` under the review hold for a local reader and maps its verdict to `dependency_refused`; asserted on every read
Fault/timing angle: a source retired or a decision re-revised between selection and read
Required faults and enabling state: a selected proposal over two disclosed sources with one uncited; `retire_observation` on the uncited one
Confidence: high - [evidence](evidence/uncited-owner-lineage-remains-read-authority.md). Verified the refusal after retiring an uncited member, the fabricated-member refusal, and that the receipt is unchanged by a refused read
Existing check: `crates/daemon/tests/memory_reviewer_settlement.rs::a_selected_read_refuses_when_an_uncited_member_no_longer_stands`, `the_staged_dependencies_are_the_brokers_union_including_uncited_inputs_and_ancestry`, `revoked_or_uncited_dependencies_abstain_and_conflicting_content_is_refused`
Impact: A proposal whose uncited context was retired or whose canonical owner moved would still be served as a supported proposal
Open questions: None.

## Relationship map

`canonical-resolution-refuses-changed-owner-and-target` is the safety half of
`production-classes-reach-policy-eligible-proposal`: the same resolution path
must reach a proposal when eligible and refuse when the owner or target moved.
Both consume the broker's canonical judgement (`judge_canonical_source`) and the
Kernel's egress fold, which `served-sensitivity-and-artifact-policy-govern-egress`
in `../canonical-positive-claim-projection/` records.

`private-result-transfer-preserves-queue-expiry` shares the settlement and
selected-read path with the two records above: the proposal a canonical run
publishes is the row whose deadline this record pins.

The four recovery records share one mechanism, `dependencies::revalidate`:
`durable-private-result-recovers-without-model-refire` and
`unknown-dispatch-does-not-authorize-resend` are the two branches of the resumed
run's decision, `receipt-selection-fences-private-generation-results` is the
generation scope both derive their candidate id under, and
`uncited-owner-lineage-remains-read-authority` is the same revalidation run by
the reader instead of the resumer.
