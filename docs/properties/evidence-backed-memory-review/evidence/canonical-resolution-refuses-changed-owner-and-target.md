# canonical-resolution-refuses-changed-owner-and-target

## Discovery trigger

Specification U6 acceptance: proposal targets contain originating decision
identity, source revision, and commit token; descriptor identity remains a
separate causal input, never the substitute mutation target; missing or
wrong-kind owners, malformed identities, stale owner or descriptor revisions,
wrong scope, owner retraction, and class tightening refuse without disclosure.
KTD7: bind proposal mutation targets to exact originating decision tokens.

## Evidence trail

`crates/daemon/src/memory_reviewer/coordinator.rs` - `resolve_descriptor` refuses
`Unsupported` when the identity field is absent or empty, `NotFound` when the
registry has never seen the decision, and `OriginRevoked` when the decision row
is invalidated; `proposal_target` refuses `OriginRevoked` for an invalidated
decision, `ExpectationChanged` when the live revision differs from the bound
one or the registry row is not a decision, and otherwise returns
`CanonicalTarget` with the decision's object id, revision, the snapshot tip,
and its last change commit sequence.

`crates/daemon/src/memory_reviewer/broker.rs` - `originating_decision` reads the
class's leading identity field; `judge_canonical_source` re-judges the decision
and descriptor on every read and requires a decision row at the snapshot.

`crates/daemon/tests/memory_reviewer_broker.rs` -
`canonical_and_promoted_descriptors_resolve_to_their_originating_decision_and_target_it`
(two descriptors, one target, commit token is the decision's last change, registry
rows equal before and after),
`a_moved_missing_stale_or_wrong_kind_owner_refuses_the_subject_and_the_target`
(supersession, stale descriptor revision, never-registered owner, stale bound
decision revision, evidence object as owner refused by `proposal_target` and by
the broker with zero bytes),
`canonical_and_promoted_forms_share_an_origin_and_a_revoked_decision_revokes_both`,
`a_canonical_owner_must_be_a_live_decision`.

## Failure scenario

A proposal over a promoted memory carries the descriptor's object id as its
target. A later exact-target application revises the descriptor observation
instead of the decision, or the decision is superseded between resolution and
binding and the proposal's commit token names the retired row.

## Timing windows and dependencies

Between `resolve_descriptor` at run preparation and `proposal_target` at
binding, a `correct_decision` or `retire_decision` commit may land. The broker
re-judges the owner on every read in that window, and `proposal_target`
re-reads the registry row at binding.

## What a test must construct

A live scoped decision with two descriptors (canonical and promoted); resolve
both and bind targets; assert equality with the decision's registry state.
Supersede the decision after resolution; assert `OriginRevoked` from both the
bound target and a fresh resolution. Publish a descriptor whose identity names
an unregistered decision; assert `NotFound`. Construct an expectation whose
bound decision revision disagrees with the live row; assert
`ExpectationChanged`. Publish a descriptor whose owner is an evidence object;
assert the broker refuses before disclosure with zero model-visible bytes.

## Investigation log

### Q: Can a descriptor with a malformed or empty identity value reach resolution?

- Sources examined: `crates/kernel/src/source_identity.rs`,
  `crates/kernel/src/source_descriptor.rs`.
- Findings: the Kernel refuses empty, over-long, or control-character identity
  values at publication (`MalformedIdentityValue`) and the broker's
  `descriptor` re-encodes the stored identity before trusting it. A stored
  descriptor cannot carry an empty leading field, so `originating_decision`'s
  empty-value branch is a defense against corruption, not a reachable input.
- Missing evidence: none.
- Conclusion: resolved with answer; the refusal is defensive and the
  publication check is the reachable guard.
