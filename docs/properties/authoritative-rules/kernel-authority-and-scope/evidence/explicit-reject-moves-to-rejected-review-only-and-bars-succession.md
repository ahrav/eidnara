# explicit-reject-moves-to-rejected-review-only-and-bars-succession

## Discovery trigger

Plan KTD3: promoting a model candidate records a fresh directive and then
`ExplicitReject` on the candidate through the existing `Disposition` op; OQ8
offers `reject` as a `revoke` alternative. The kernel semantics of
`ExplicitReject` are what the plan relies on.

## Evidence trail

- `crates/kernel/src/admission.rs:4145` `event_effect(ExplicitReject)` is
  `(current, Some(Disposition::Rejected), Outcome::Reject)`.
- `:4172-4182` `auto_admit_event` includes `ExplicitReject`; `:4184-4195`
  `event_can_raise_support` excludes it, so `requires_approval` reduces to
  `relaxes_disposition` (`:320-329`), which is false for a move from `Active`
  to `Rejected` (`disposition_restrictiveness`, `:4163-4170`: `Active` 0,
  `Rejected` 2).
- `:331-334` the disposition becomes `Rejected`; `:345-351` a non-active
  disposition sets `supported = Candidate`, so `effective = Candidate`;
  `historical` is unchanged.
- `:4252` `visibility_row(_, Rejected)` is `ReviewOnly`; `:4116-4138`
  `surface_visibility(ReviewOnly, any surface, _)` is `Hidden`.
- `:1339-1349` `refuse_barred_lineage` refuses `supersede_decision` when the
  predecessor's or the survivor's latest disposition is `Rejected`.
- Relaxation: a later `MarkStale` from `Rejected` has restrictiveness 1 < 2,
  so `relaxes_disposition` is true and the event is denied without a valid
  approval (`:324-330`).
- When the rejected subject is an approval, `with_authority_cascade`
  (`:1055-1082`) collects it as a bearer before the write and
  `demote_dependents_if_authority_lost` runs after.
- Route: `crates/daemon/src/kernel_routes/commit.rs:113-131`
  `DispositionEvent::ExplicitReject -> EventKind::ExplicitReject`;
  `:566-600` `disposition_request` builds the request from the subject's
  stored classes and checks the approval chain is in project; `:626-643`
  `record_disposition`; `:602-623` `DispositionResult::denied` is true when
  the resulting disposition differs from the requested one.
- Tests: `crates/daemon/tests/kernel_routes.rs:4392` (`explicit_reject` ->
  outcome `reject`, disposition `rejected`), `:4452` (denied relaxation
  records a row and keeps the disposition; approval permits);
  `crates/kernel/tests/kernel_admission.rs:2890` (`Rejected` ->
  `review_only`); `admission.rs:5334` (relaxation needs approval).

## Failure scenario

The directive route records `ExplicitReject` on the candidate; the candidate
stays `explicit_labeled` and keeps appearing on explicit search, or a later
model `supersede_decision` on the rejected candidate writes a fresh
`Candidate` successor that is served again, undoing the user's refusal.

## Timing windows and dependencies

- `refuse_barred_lineage` reads the latest prior at the time of the
  supersede; a rejection recorded in the same envelope is visible through
  the envelope cache (`load_prior_for_key`, `:2186-2192`).
- The route's `Disposition` uses the subject's stored classes, so the
  evaluator's class check passes for any subject (lead L4).

## What a test must construct

1. A live model-class decision `C` (`Candidate`, `explicit_labeled`).
2. Route `disposition(C, "explicit_reject")`: assert
   `dispositions[0] == {outcome: "reject", previous_disposition: "active",
   disposition: "rejected", denied: false}`; stored row `effective_maturity
   == candidate`, `visibility == review_only`.
3. `read_visible` on each of `AutoInject`, `AutoSearch`, `ExplicitSearch`
   omits `C`.
4. Route `supersede_decision(C, spec at revision + 1)`: assert the commit is
   refused with the kernel's `AdmissionPolicy` mapping (an `invalid` state
   per the route's `CommitFailure` conversion at `commit.rs:328-361`).
5. Route `disposition(C, "mark_stale")` without approval: `denied == true`,
   stored disposition still `rejected`.
6. Approval variant: `C` is itself a seeded approval with a dependent; after
   `explicit_reject` the dependent's latest row is `approval_revoked`.

## Investigation log

### Q: Is a `Rejected` predecessor covered by an existing supersede test?

- Sources examined: `crates/kernel/tests/kernel_slice.rs:1186-1270`,
  `crates/kernel/tests/kernel_admission.rs:991-1046`.
- Findings: both construct `Quarantined`; none constructs `Rejected` or
  `Contradicted`.
- Missing evidence: a test for the other two barred dispositions.
- Conclusion: unresolved, needs a test; listed in `existing-checks.md` under
  "none found".
