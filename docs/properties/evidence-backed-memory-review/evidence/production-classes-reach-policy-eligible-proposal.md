# production-classes-reach-policy-eligible-proposal

## Discovery trigger

Specification U6 acceptance: production `MEMORY_CLASSES` without overrides
reach a selected non-abstaining proposal with actual artifact eligibility,
disclosed bytes, request ledger, and selected receipt inspected; a nonempty
class set is insufficient. Stop condition: current resolution did not support
the production classes, and canonical artifacts retain Sensitive policy.

## Evidence trail

`crates/daemon/src/memory_reviewer/coordinator.rs` - `resolve_descriptor` builds
`ReferenceExpectation::CanonicalSource` for `decision_derived` classes, binding
the originating decision's live source revision; `proposal_target` names the
decision.

`crates/daemon/src/memory_reviewer/selection.rs` - `MEMORY_CLASSES` and
`PRODUCTION_SELECTION_OPEN`.

`crates/daemon/src/lib.rs` - `scheduled_projects` adds
`MemoryReviewerReviewSelection` only when the activation gate is open and
`PRODUCTION_SELECTION_OPEN` is `true`; `select_review_targets` walks
`MEMORY_CLASSES`.

`crates/kernel/src/cas/ingest.rs` - an artifact without affirmative repository
provenance is stored `Sensitive`; `crates/daemon/src/harness_sources.rs` issues
provenance only for `git_commits`; `crates/kernel/src/envelope.rs` refuses
`Sensitive` for `ArtifactDestination::Remote`.

`crates/daemon/tests/memory_reviewer_broker.rs` -
`canonical_and_promoted_descriptors_resolve_to_their_originating_decision_and_target_it`
and `a_resolved_canonical_or_promoted_subject_still_refuses_the_remote_destination`:
both classes refuse `PolicyBlocked` on the Remote destination with zero
model-visible bytes.

`crates/daemon/tests/memory_reviewer_worker.rs` -
`production_classes_are_refused_the_remote_model_at_selection_and_in_the_coordinator`:
the production `ClaimMaterializer`, run `LocalOnly` as `search_lifecycle_owner`
runs it, publishes two `canonical_claims` forms and one `promoted_memory` form
of one decision. Each names the decision, is permitted to a local reader, and
is denied Remote (`SensitiveRemote`; the selector's surface verdict is
`ProviderSensitive`). `select_review_targets`
over `MEMORY_CLASSES` with no class override returns an empty page with no
cursor. The same causal inputs, enqueued through the selection slot's frozen
page, run in the production worker to `owner_sensitive` receipts with no
attempt row, zero connections, a `not_selected` read, and Kernel object
states equal before and after.

`crates/daemon/tests/memory_reviewer_worker.rs` -
`fixture_canonical_and_promoted_forms_reach_readable_proposals_targeting_their_decision`:
a fixture witness, not production eligibility. Each descriptor's evidence
carries repository provenance the test supplies. For a `canonical_claims` and a
`promoted_memory` form of one decision, the same selector, slot enqueue, and
worker produce `Complete` receipts and readable `retain` proposals whose
`CanonicalTarget` names the decision's object id, source revision, and
last-change commit token, each citing its own form's evidence; the Kernel tip
does not move.

`crates/daemon/tests/memory_reviewer_selection.rs` -
`the_production_classes_are_walked_in_order_and_a_cursor_resumes_in_the_second_class`.

## Failure scenario

The scheduler opens production selection, a job over a canonical descriptor is
claimed, and the coordinator either refuses the class as unsupported (the
pre-change behavior) or discloses a Sensitive artifact to the Remote model.
Either outcome would be reported as production reachability by a test that only
checked the class list.

## Timing windows and dependencies

None. The gate is a compile-time constant and the eligibility judgement is a
single Kernel read per disclosure.

## What a test must construct

With `PRODUCTION_SELECTION_OPEN` true and an owner-approved Remote-eligible
decision representation: a live scoped decision, a canonical descriptor over an
artifact whose Remote egress is `Allowed`, an open activation gate, a real
selection page, a claimed job, and a scripted model response that proposes.
Assert the selected receipt, the readable proposal whose target names the
decision, the request ledger, and canonical before/after equality. Until that
representation exists, the campaign records this situation as not constructed.
The fixture witness above constructs every step except the representation: its
provenance is test-supplied, so it proves the path, not the eligibility.

## Investigation log

### Q: Does any production representation of a decision reach the Remote destination today?

- Sources examined: `crates/kernel/src/cas/ingest.rs`,
  `crates/daemon/src/harness_sources.rs`, `crates/daemon/src/claim_sources.rs`,
  `crates/daemon/tests/memory_reviewer_broker.rs`.
- Findings: canonical materialization publishes decision summary, rationale, and
  promoted summary with `provenance: None`; ingest stores them `Sensitive`;
  Remote egress refuses `Sensitive`. The broker test asserts `PolicyBlocked`
  for the canonical form on the Remote destination.
- Missing evidence: none for the current question. The owner chose that no
  decision bytes go Remote.
- Conclusion: resolved with answer; the gate stays closed and the positive
  witness stays unconstructed while that choice stands.
