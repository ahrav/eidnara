# directive-revoke-cascades-and-stops-serving

## Discovery trigger

Plan requirement R7 (revocation cascades), KTD3 (`revoke_approval` with
`demote_authority_dependents`), acceptance example AE6, U3 step 4 and its
test scenario. Forbidden outcome F3: a revoked directive served.

## Evidence trail

Verified at HEAD:

- `crates/kernel/src/admission.rs:1351-1445` `revoke_approval` ->
  `revoke_approval_inner`: refuses a blank reason; loads the object through
  `APPROVAL_OBJECT_PREDICATE` (live `adr_accepted` decision) else `NotFound`;
  `object_write::invalidate` on `decisions`; pushes a change of kind
  `APPROVAL_REVOKE_KIND` with an audit `{approval_object_id, reason,
  policy_revision}`; runs `demote_authority_dependents` and records
  `demoted` and `deferred` counts in the audit.
- `admission.rs:1447-1486` `demote_authority_dependents`: breadth-first over
  `load_approval_dependents`, transitive through dependents that are
  themselves approvals; dependents past `MAX_AUTHORITY_DEMOTIONS` or on a
  superseded policy revision are deferred, still traversed.
- `admission.rs:1488-1521` `demote_one_dependent`: records `ApprovalRevoked`
  with the dependent's own stored classes; `remaining_support` is
  `min(prior effective, automatic_ceiling)` (`admission.rs:1616-1621`).
- `admission.rs:1406-1430` doc comment: no policy outcome fails the cascade,
  so a widely cited approval is always revocable.
- Serving: the object is invalidated, so `AuthorityAsOf::live` excludes it
  from snapshots at or after the revoke (`admission.rs:739-752`).
- Tests: `crates/kernel/tests/kernel_admission.rs:469`
  `approval_revocation_fans_out_support_demotion_without_rewriting_history`;
  `:2081` `an_unvisited_descendant_of_a_revoked_root_grants_nothing`; `:5988`
  `a_revocation_past_the_demotion_cap_traverses_deferred_approvals_and_stops_serving_them`.

Plan-only: the route body `{object_id, reason, operation_key}`, the `reject`
variant (OQ8).

## Failure scenario

Legal witness: D1 Approved; M cites D1 and is served Verified-or-higher on
AutoInject. `revoke(D1, "obsolete")` commits: D1 invalidated; audit row;
M's latest admission `ApprovalRevoked` at Candidate; the next `kernel.read`
serves neither D1 nor M on AutoInject.

Smallest violating traces:

1. The handler records `ExplicitReject` instead of `revoke_approval` while the
   UI says "revoked": D1 is Rejected (`review_only`), still registered, and
   `visibility_row(Rejected)` hides it, but dependents are demoted only
   through the disposition's `with_authority_cascade` path; both paths demote,
   so the observable difference is reversibility (OQ8), not a served
   directive. Not a violation of this record; recorded for completeness.
2. The handler revokes with an empty reason after trimming: `AdmissionPolicy`;
   the route must report a rejection, not a defect.
3. A read-side cache (N4 snapshot) keyed only by content serves the old block
   after the revoke: the Kernel verdict is correct, the derived snapshot is
   stale. That is an N4 obligation; this record asserts the Kernel verdict.

## Timing windows and dependencies

The cascade runs inside the revoking transaction and holds the single writer
for its duration (`admission.rs:1424-1430`). A transform pass whose read
snapshot predates the commit serves the old row for that pass only.

## What a test must construct

- D1 through `directive.record`; M seeded citing D1.
- `revoke(D1, reason)`; assert `invalidated_commit_seq` set on D1; a
  `changes` entry of kind `approval_revoke` naming D1 with `demoted = 1`;
  M's latest admission `event_kind='approval_revoked'`,
  `effective_maturity='candidate'`.
- `kernel.read` at the new tip on `auto_inject`: D1 and M absent.
- Replay: the same `revoke` request again returns `replayed: true` and the
  audit `demoted` count is unchanged.
- Negative: `revoke` on a `model_inference` row is `NotFound` (not an
  approval object).

## Investigation log

### Q: Can `revoke_approval` target a non-directive `adr_accepted` row?
- Sources examined: `admission.rs:1374-1388`.
- Findings: any live `adr_accepted` decision in scope qualifies; the route
  must add a project scope check (`scoped_object_state`) and may add a
  `source_kind = user_directive` check.
- Missing evidence: the route.
- Conclusion: unresolved, resolvable at implementation.

### Q: Which of revoke or reject is the default "forget"?
- Sources examined: plan OQ8.
- Findings: plan assumes revoke, offers reject separately.
- Missing evidence: user decision.
- Conclusion: needs human input.
