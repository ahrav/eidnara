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
| `observer-route-does-not-change-background-rosters` | #728 | yes |
| `status-sanitizer-preserves-inclusive-integer-domain` | #729 | yes |
| `completed-outcome-pages-have-live-keyset-semantics` | #730 | yes |
| `shared-path-fixture-reaches-selected-readable-proposal` | #730 | yes |
| `reference-only-cli-outcomes-preserve-meaning` | #730 | yes |
| `review-cli-owns-one-replay-free-connection` | #730 | yes |
| `review-cli-validates-byte-exact-inert-payloads` | #730 | yes |
| `review-cli-preserves-shared-kernel-refusal-shapes` | #730 | yes |
| `status-freshness-never-defaults-unknown-to-zero` | #730 | yes |
| `status-counts-preserve-overlapping-ledger-populations` | #730 | yes |
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
Reachability: default-production
Status: active
Exercised: yes - `crates/daemon/tests/memory_reviewer_broker.rs` constructs a superseded owner, a never-registered owner, a stale descriptor revision, a stale bound decision revision, an owner that is not a decision, and the Remote destination
Guarantee: A proposal over a canonical or promoted descriptor names the originating decision's exact object id, source revision, snapshot, and last-change commit token, never the descriptor; a missing, retired, superseded, re-revised, wrong-kind, or out-of-scope owner refuses before any byte is disclosed, and descriptor identity cannot redirect the target.
Check: `always` - every `ProposalTarget::Memory` produced from a `CanonicalSource` subject has `object_id == originating_decision_id` and `source_revision == decision_source_revision`, and `proposal_target` and `resolve_descriptor` return a refusal whenever the decision's registry row is invalidated, absent, not a decision, or at another revision; asserted on every evaluation because the target is the mutation token a later application compares against
Fault/timing angle: the decision changes between resolution and proposal binding
Required faults and enabling state: `correct_decision` superseding the bound decision after `resolve_descriptor`; a descriptor whose identity names an unregistered decision; a bound expectation whose `decision_source_revision` disagrees with the live row; a stale descriptor revision; a descriptor whose owner is an evidence object
Confidence: high - [evidence](evidence/canonical-resolution-refuses-changed-owner-and-target.md). Verified each refusal code, that two descriptors of one decision produce one equal target whose commit token is the decision's last change rather than its creation, and that resolution and binding leave the tracked registry rows equal
Existing check: `crates/daemon/tests/memory_reviewer_broker.rs::canonical_and_promoted_descriptors_resolve_to_their_originating_decision_and_target_it`, `a_moved_missing_stale_or_wrong_kind_owner_refuses_the_subject_and_the_target`, `canonical_and_promoted_forms_share_an_origin_and_a_revoked_decision_revokes_both`, `a_canonical_owner_must_be_a_live_decision`
Impact: A proposal bound to the descriptor or to a moved decision would carry a mutation token that a later application could apply to the wrong object or revision
Open questions: None.

### private-result-transfer-preserves-queue-expiry

Type: safety
Reachability: default-production
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
Guarantee: A marker at the run's generation other than a proven `not_dispatched` ends the resumed run without a request: the receipt completes `unknown` when no sealed row exists, and adopts or abstains on the row when one does. Only `not_dispatched` markers, or none, admit a newly charged attempt, under the original identity, deadlines, and remaining count.
Check: `always` - `resumes_dispatched_work` is evaluated in `prepare` before subject resolution and hold growth; a `true` result skips both and routes to `adopt`; asserted on every run start
Fault/timing angle: crash between marker commit and terminal write; cancellation mid-attempt; a lapsed recheck after commit
Required faults and enabling state: `dispatch_memory_reviewer_attempt` with no terminal; a cancelled first run; a recheck clock past the attempt deadline
Confidence: high - [evidence](evidence/unknown-dispatch-does-not-authorize-resend.md). Verified the three marker classes at the coordinator with connection counts and attempt counts
Existing check: `crates/daemon/tests/memory_reviewer_coordinator.rs::an_unknown_attempt_outcome_completes_unknown_and_cancellation_joins_the_attempt`, `a_resumed_generation_with_a_cancelled_marker_completes_unknown_without_a_send`, `a_not_dispatched_marker_alone_lets_the_run_proceed_with_a_new_attempt`
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

### observer-route-does-not-change-background-rosters

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `crates/daemon/src/lib.rs` tests bind an observer alone on a dormant MODULE project, beside a live scheduled harness, and on a second root, and read the worker, scheduler, and roster views after every open and close in both close orders; `crates/daemon/tests/memory_reviewer_worker.rs` runs a pass over a hand-built empty view against a Ready Sensitive job, so observer to `module_projects` to worker pass is composed from the two tests rather than driven end to end; `crates/daemon/tests/memory_reviewer_wire.rs` reads through a `cli` route
Guarantee: A binding whose harness is `cli` is removed from the participating view before newest-per-root selection, so the MemoryReviewer worker's projects, the scheduler's roots, harnesses, and schedules, and the search maintenance roster are identical whether or not the observer is open; route-local authorization and project lookup for the observer's own route are unchanged.
Check: `always` - `RouteBindings::participating` is the only path into `latest_per_root` and `latest_for_root`, and those are the only sources for `module_projects`, `scheduled_projects`, `binding_for_root`, and `bound_projects`; asserted by the view equality before and after each observer open and close
Fault/timing angle: an observer opened after a live harness on the same root, an observer-only second root of the project, each route closing first
Required faults and enabling state: MODULE authority on the root with the start-up binding closed; a live `pi` binding with a user-tier schedule; an observer binding on the same root and on a second bound root; a Ready job with the gate open and a worker pass over an empty view
Confidence: high - [evidence](evidence/observer-route-does-not-change-background-rosters.md). Verified the four views by equality against the dormant and the scheduled snapshots, the Ready job's state after two passes over an empty view, and byte-equal wire answers through a `cli` route
Existing check: `crates/daemon/src/lib.rs::tests::an_observational_binding_reads_its_project_and_takes_no_part_in_background_work`, `crates/daemon/tests/memory_reviewer_worker.rs::a_pass_over_a_view_without_the_project_leaves_its_ready_job_unclaimed`, `crates/daemon/tests/memory_reviewer_wire.rs::an_observational_route_reads_the_same_outcomes_as_an_ordinary_route`
Impact: Opening the review command on a dormant project would enroll it in the worker and run its Ready jobs, or replace a live harness's schedule with the observer's configuration
Open questions: None.

### status-sanitizer-preserves-inclusive-integer-domain

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `packages/opencode-plugin/src/shared/host-client/client.test.ts` sends raw wire tokens at the signed and unsigned extrema and around 2^53 through a routed response and a `host.status` response over the fake daemon; `exact-json.test.ts` covers the reviver and the domain validators
Guarantee: The host client never presents a rounded integer as exact: a safe integer arrives as a `number`, any other integer lexeme arrives as a `bigint` with its exact value or refuses the body, so adjacent unequal wire integers never compare equal after decoding. The count domain admits `9007199254740992` and refuses `9007199254740993`, negative, fractional, null, and absent values, each without touching its siblings; `u64` and `i64` fields follow their own bounds.
Check: `always` - `consumeJson` is the only JSON decode on the receive path and calls `parseExactJson`; the reviver splits on `Number.isSafeInteger` for every number; asserted on every decoded body
Fault/timing angle: none; a pure decoding property
Required faults and enabling state: response bodies written as raw text with chosen integer tokens; a `host.status` body carrying a counter of 2^53+1 beside a valid counter
Confidence: high - [evidence](evidence/status-sanitizer-preserves-inclusive-integer-domain.md). Verified under Bun 1.3.14 through the test suite and under Node 24.18 by direct import; both report `bigint` for 2^53+1 and `number` for 2^53
Existing check: `client.test.ts::integer lexemes a double cannot reproduce arrive exact through routed and control responses`, `routeOpen omits the ambient consumer identity only when asked, for that bind alone`; `exact-json.test.ts`
Impact: A counter or generation past 2^53 would display a neighboring value as exact, and two different receipts or revisions could render identically
Open questions: None.

### completed-outcome-pages-have-live-keyset-semantics

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `packages/cli/src/commands/review.test.ts` sends an explicit `limit` and `after`, renders a full page's continuation and an empty page's end, and never issues a second request; the daemon's page semantics are exercised in `crates/daemon/tests/memory_reviewer_wire.rs`
Guarantee: `review list` asks for exactly one page with an explicit limit and an optional causal-identity cursor, prints the cursor the daemon returns as the operator's next command, and never walks pages, reads a Kernel payload, or presents the walk as a chronological or stable snapshot.
Check: `always` - one `request` per invocation with `limit` and `after` in the envelope; asserted on every list
Fault/timing angle: an outcome completing behind the cursor during a walk
Required faults and enabling state: a page whose `next` is a cursor, then a follow-up whose `next` is null
Confidence: high - [evidence](evidence/completed-outcome-pages-have-live-keyset-semantics.md). Verified the envelope and the single request at the fake connection and over the real transport
Existing check: `packages/cli/src/commands/review.test.ts`::`review list`; `packages/cli/src/commands/review.wire.test.ts`
Impact: An automatic walk would read every page on each invocation and hide that new identities behind the cursor need a fresh walk
Open questions: None.

### shared-path-fixture-reaches-selected-readable-proposal

Type: reachability
Reachability: test-only
Status: active
Exercised: partially - `crates/daemon/tests/memory_reviewer_wire.rs` reaches a selected proposal over the real handler with a completed receipt through the shared `publish` fixture; `packages/cli/src/commands/review.test.ts` decodes a proposal body with nonempty spans through the command's validator; `packages/e2e-tests/src/rust-runner/review-cli.test.ts` drives status, list, and show against the direct-host fixture, where no MODULE authority is bound and the answers are refusals
Guarantee: The command's decoder accepts exactly the fields the Kernel's staged `ReviewProposal` serializes, span for span, and the real host answers the command's flat envelopes over the installed transport.
Check: `sometimes` - a real-store run must reach a selected proposal the command renders; the situation is the selected read, not a branch
Fault/timing angle: none
Required faults and enabling state: MODULE authority on a root, a completed receipt, a staged proposal with nonempty spans, the command reading it
Confidence: medium - [evidence](evidence/shared-path-fixture-reaches-selected-readable-proposal.md). The positive read through the command against a real host is not constructed; the Rust wire test and the command's decoder are joined by the wire document's shape, not by one process
Existing check: `crates/daemon/tests/memory_reviewer_wire.rs::a_published_proposal_reads_from_every_root_after_a_newer_root_binds`; `packages/cli/src/commands/review.test.ts`::`review show`; `packages/e2e-tests/src/rust-runner/review-cli.test.ts`
Impact: A field the Kernel serializes differently from the decoder's expectation would refuse every real proposal as malformed
Open questions:
- A TypeScript path that activates MODULE authority on the hermetic host and publishes a proposal with nonempty spans is needed to drive `review show` to a rendered proposal in one process (needs human input)

### reference-only-cli-outcomes-preserve-meaning

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `packages/cli/src/commands/review.test.ts` renders each of the six outcomes and each of the nine abstention reasons, maps every read terminal and the `disabled` sentence, and refuses an unknown outcome, an unknown reason, a reason on a non-abstained item, and a terminal outside the operation's vocabulary
Guarantee: Every receipt terminal and abstention reason the protocol names renders as itself, the `disabled` terminal renders as the owner's sentence, `not_selected` never triggers a list walk or a guessed reason, and an unknown variant refuses without success output or a raw dump.
Check: `always` - closed vocabularies at decode; asserted on every item and terminal
Fault/timing angle: none
Required faults and enabling state: bodies with each terminal, an unknown outcome, a reason on a complete item, a `not_selected` terminal on the list operation
Confidence: high - [evidence](evidence/reference-only-cli-outcomes-preserve-meaning.md). Verified each mapping and each refusal
Existing check: `packages/cli/src/commands/review.test.ts`::`review list`, `review list vocabulary`, `review show`
Impact: A guessed reason or a walked list would present an inference as the daemon's outcome
Open questions: None.

### review-cli-owns-one-replay-free-connection

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `packages/cli/src/commands/review.test.ts` counts one `closeAsync` on success, terminal, malformed, thrown request, and incompatible catalog; `packages/cli/src/commands/review.wire.test.ts` asserts `isClosed` after success and after a route refusal; `packages/e2e-tests/src/rust-runner/review-cli.test.ts` asserts it against the real host
Guarantee: One connection is opened per invocation and closed on every path; the catalog probe, the route open, and the request share it; routed requests use `request`, never a managed `call`, so nothing is replayed; and no model call, canonical write, cache, poll, or retry occurs.
Check: `always` - `closeAsync` in `finally`; asserted on every path
Fault/timing angle: a thrown request, an aborted request, a refused route
Required faults and enabling state: a connection whose request throws each error kind
Confidence: high - [evidence](evidence/review-cli-owns-one-replay-free-connection.md). Verified the close count on nine paths and the closed client over the real transport
Existing check: `packages/cli/src/commands/review.test.ts`::`connection lifecycle`; `packages/cli/src/commands/review.wire.test.ts`; `packages/e2e-tests/src/rust-runner/review-cli.test.ts`
Impact: A leaked connection would hold a payload block; a managed call would replay a read after an unknown outcome
Open questions: None.

### review-cli-validates-byte-exact-inert-payloads

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `packages/cli/src/commands/review.test.ts` refuses text past 32 KiB, an identifier past 512 bytes, a span whose end does not follow its start, a page past 64 items, and renders control sequences inert; `packages/cli/src/commands/review.wire.test.ts` decodes 2^53+1 exactly through the real transport
Guarantee: Every field is validated against the Kernel's byte caps and its exact integer domain before rendering; text output strips control and escape sequences and JSON output keeps exact integer tokens; no peer, provider, or error body is echoed and nothing reaches a diagnostic, transcript, file, model context, or remote request.
Check: `always` - the decoder runs before any output; asserted on every body
Fault/timing angle: none
Required faults and enabling state: oversize text, escape sequences in text, unsafe integers in every integer field
Confidence: high - [evidence](evidence/review-cli-validates-byte-exact-inert-payloads.md). Verified the caps, the inert rendering, and the exact tokens
Existing check: `packages/cli/src/commands/review.test.ts`::`review show`, `connection lifecycle`; `packages/cli/src/commands/review.wire.test.ts`
Impact: A crafted proposal could move the cursor, forge output, or round an identifier's revision
Open questions: None.

### review-cli-preserves-shared-kernel-refusal-shapes

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `packages/cli/src/commands/review.test.ts` renders `invalid:project_mismatch`, `unavailable:store_starting`, and an unrecognized state, and the management codes `route_unbound`, `session_mismatch`, `bad_request`, `unrecognized_request_shape`, `invalid_params`, `invalid_response_body`; `packages/cli/src/commands/review.wire.test.ts` drives `session_mismatch` over the real transport; `packages/e2e-tests/src/rust-runner/review-cli.test.ts` drives an unbound root against the real host
Guarantee: A Kernel state answers as the kernel client's state key, a management code as its code, and a closed terminal as the owner's text; none of them prints the daemon's message or body.
Check: `always` - `parseKernelState` and `isHostCallError` on the refusal paths; asserted on every refusal
Fault/timing angle: none
Required faults and enabling state: each state and code body
Confidence: high - [evidence](evidence/review-cli-preserves-shared-kernel-refusal-shapes.md). Verified each rendering
Existing check: `packages/cli/src/commands/review.test.ts`::`review list`, `connection lifecycle`; `packages/cli/src/commands/review.wire.test.ts`; `packages/e2e-tests/src/rust-runner/review-cli.test.ts`
Impact: A wrong-root or unready refusal rendered as raw text would leak the daemon's message and hide the shared state
Open questions: None.

### status-freshness-never-defaults-unknown-to-zero

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `packages/cli/src/commands/review.test.ts` renders a `starting` block with present `swept_*` zeros as all unavailable, an absent block as unavailable, a missing or invalid counter as unavailable beside a valid sibling, and an unknown activation state as unknown
Guarantee: A store that is not `ready` reports every counter unavailable; a counter that is absent, negative, fractional, `null`, or past 2^53 is unavailable, never zero; `sampled_at_ms` and the activation state stay unknown when missing or unrecognized; unknown fields are ignored.
Check: `always` - `decodeReviewStatus` runs the wire document's rules field by field; asserted on every status
Fault/timing angle: a stale or starting sampler
Required faults and enabling state: a `starting` block with zeros, an absent block, out-of-domain counters
Confidence: high - [evidence](evidence/status-freshness-never-defaults-unknown-to-zero.md). Verified each rule
Existing check: `packages/cli/src/commands/review.test.ts`::`review status`; `packages/cli/src/commands/review.wire.test.ts`
Impact: A zero printed for an unsampled counter would read as an idle store
Open questions: None.

### status-counts-preserve-overlapping-ledger-populations

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `packages/cli/src/commands/review.test.ts` asserts every counter prints under its own name with no total, ratio, or success line and that the command binds no route and names no project
Guarantee: Status prints the sampler's counters as the overlapping populations they are, over the whole data home, with no synthetic total, success ratio, or per-project attribution.
Check: `always` - the renderer emits one line per counter and no derived value; asserted on every status
Fault/timing angle: none
Required faults and enabling state: a `ready` block with several populated counters
Confidence: high - [evidence](evidence/status-counts-preserve-overlapping-ledger-populations.md). Verified the output has no derived line and no route open
Existing check: `packages/cli/src/commands/review.test.ts`::`review status`
Impact: A total or ratio over overlapping populations would misstate the store's work
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

The eight review-command records share one consumer: `review-cli-owns-one-replay-free-connection` is the
connection every other record's request travels on, `review-cli-validates-byte-exact-inert-payloads` and
`status-sanitizer-preserves-inclusive-integer-domain` are the same decoding discipline at two layers, and
`observer-route-does-not-change-background-rosters` is what the command's `cli` harness relies on.

`observer-route-does-not-change-background-rosters` sits upstream of every
worker record: the participating view decides which projects the worker sees
at all, and the records above describe what happens to a job the worker did
see.
