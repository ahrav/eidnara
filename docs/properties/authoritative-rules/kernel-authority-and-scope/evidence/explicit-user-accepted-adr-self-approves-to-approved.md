# explicit-user-accepted-adr-self-approves-to-approved

## Discovery trigger

Plan KTD1 and R1: the directive channel inserts an `adr_accepted` decision
and records `AcceptedAdr` at `(explicit_user, user_explicit)`, expecting the
policy to self-approve it and make it Visible on AutoInject. The claimed-
guarantees lens listed it as C1; the state lens traced the evaluator.

## Evidence trail

- `crates/kernel/src/admission.rs:4197-4202` `automatic_ceiling`:
  `(ExplicitUser, UserExplicit | UserInferred)` maps to `Verified`.
- `:4221-4234` `admission_ceiling`: `AcceptedAdr` with
  `subject_is_accepted_decision && automatic >= Verified` returns `Approved`.
- `:4148` `event_effect(AcceptedAdr)` is `(Approved, None, Promote)`.
- `:4172-4182` `auto_admit_event` includes `AcceptedAdr`.
- `:324-330` `requires_approval` is false when the event auto-admits and the
  requested rung is at or below `admission_ceiling`; `denied` is false.
- `:318-319` `raises_support` is true (`Approved > Candidate`); `:338-351`
  `supported = requested`, `effective = historical.min(supported) = Approved`.
- `:377` `visibility_row(Approved, Active)` is `Automatic` (`:4243-4246`).
- `:4116-4138` `surface_visibility(Automatic, AutoInject, Normal)` is
  `Visible`.
- `:1569-1580` `prepare_with_facts` skips approval back-fill for
  `AcceptedAdr`; `:1603` `validate_trigger` returns `Some(None)` for this
  event (`:2794`), so the event is evaluated as itself.
- `:376` `support_from_citation` is true (support rose above the ceiling),
  so `:1652-1658` takes `request.event.approval_object_id`, which is `None`;
  the stored `approval_object_id` is therefore `NULL` and the row's
  `elevated_support` is 1 (`:1715-1716`, `Approved > Verified`).
- `:1722-1727` `enforce_lineage_bearer_cap` runs because the row would
  qualify as a bearer; one directive per lineage never reaches the 64 cap.
- Store construction: `crates/daemon/tests/kernel_routes.rs:4356-4387`
  `seed_approval` inserts the decision and records the event in one envelope.
- Evaluator test: `admission.rs:5066` iterates both `Verified`-ceiling pairs.

## Failure scenario

The directive route records `AcceptedAdr`, the evaluator returns `Candidate`
or `Verified` instead of `Approved`, or stores an `approval_object_id`. The
row is `explicit_labeled` or depends on an ancestor approval; the directive
never auto-injects, or a later revocation elsewhere demotes it.

Inverse failure: the evaluator returns `Approved` for a pair whose ceiling is
`Candidate` (a model class), minting authority for model text.

## Timing windows and dependencies

- The subject must be a live `adr_accepted` decision when `prepare_with_facts`
  runs; same-envelope insert then admit is the tested order.
- The decision's sensitivity must be `Normal` for `Visible` on `AutoInject`;
  maturity does not depend on it (see the sensitivity record).
- `policy_revision` is stamped 1; a future revision changes the predicate
  set in `approval_row_fields`.

## What a test must construct

1. A domain, a project scope, and a decision with `decision_kind =
   "adr_accepted"`, `sensitivity = Normal`, in the memory domain.
2. `record_admission(AcceptedAdr, candidate_id: None, subject: the decision,
   (ExplicitUser, UserExplicit), approval_object_id: None)` in the same
   envelope.
3. Assert the returned `AdmissionDecision`: `effective_maturity == Approved`,
   `disposition == Active`, `visibility == Automatic`, `outcome == Admit`.
4. Read `admission_decisions` for the subject: `approval_object_id IS NULL`
   and `elevated_support = 1`.
5. `read_visible(project, Surface::AutoInject, None,
   RowSelection::DomainDecisions("memory"))` returns the object with
   `visibility == Visible` and `labeled == false`.
6. Negative control: the same sequence at `(ModelInference, UserInferred)`
   is a legal pair whose ceiling is `Candidate`, so `admission_ceiling` is
   `Candidate`, `requires_approval` is true, and the row is `deny` at
   `Candidate`.

## Investigation log

### Q: Does `(explicit_user, user_inferred)` also self-approve, and must the route refuse it?

- Sources examined: `automatic_ceiling` (`:4197-4202`), `approval_row_fields`
  (`:843-856`), the plan's KTD1.
- Findings: the pair has a `Verified` ceiling, so an `adr_accepted` decision
  admitted under it reaches `Approved` by the same path; it cannot grant
  authority to others because `approval_row_fields` requires
  `taint_class='user_explicit'`. No writer exists for the pair.
- Missing evidence: a design decision on whether the directive route hard-
  codes `user_explicit` only (the plan says it does) and whether a test must
  pin the refusal.
- Conclusion: needs human input (N1 design); the record's guarantee is
  stated for `user_explicit` only.
