# directive-confirmed-from-promotes-by-fresh-record

## Discovery trigger

Plan requirement R3: "the only promotion is a fresh user directive recorded
through R1 that cites the candidate in its rationale and rejects the
candidate". KTD3 candidate promotion; U3 step 2 (`confirmed_from`) and the
test scenario "`directive.record` with `confirmed_from` leaves the candidate
Rejected and `review_only`". Forbidden outcome F7 and tracker issues 579 and
709 (acceptance never becomes human knowledge authority).

## Evidence trail

Verified at HEAD:

- `crates/kernel/src/admission.rs:4144` `event_effect(ExplicitReject)` sets
  disposition Rejected with outcome Reject; `admission.rs:4251`
  `visibility_row(_, Rejected) == ReviewOnly`.
- `admission.rs:268-274`: re-admitting a subject under classes different from
  its prior is `AdmissionPolicy`. Promotion in place is therefore impossible
  through `record_admission`; the only way to change class is a new object.
- `crates/daemon/src/kernel_routes/commit.rs:566-600` `disposition_request`
  reads the subject's stored classes through `subject_admission` and records
  the event under them; this is the pattern the directive handler would use
  for the candidate's `ExplicitReject`.
- `admission.rs:1343-1349` `refuse_barred_lineage`: a Rejected predecessor
  cannot be superseded, so after the reject the candidate cannot be revived
  by correction; a new candidate is needed.
- `admission.rs:1055-1103` `with_authority_cascade` is used by disposition
  writes so a rejected row that granted authority demotes dependents; a
  `model_inference` candidate grants none.
- Tests on dispositions through the route: `crates/daemon/tests/kernel_routes.rs:4392`
  `disposition_events_apply_the_fixed_table_and_report_outcome_and_disposition`.

Plan-only: the `confirmed_from` body field, the rationale JSON shape, the
same-envelope pairing.

## Failure scenario

Legal witness: candidate C `(model_inference, user_inferred)`, Candidate,
Labeled on ExplicitSearch. User runs `confirm C`; the route reads C's text,
shows it, and on confirm commits one envelope: `insert_decision(adr_accepted,
user_directive, directive:u, rev 1, rationale {"confirmed_from": C})`,
`record_admission(AcceptedAdr, explicit_user/user_explicit, D)`,
`record_admission(ExplicitReject, C's stored classes, C)`. D is Approved and
Visible; C is Rejected, `review_only`, still registered; C's admission rows
all carry `model_inference`.

Smallest violating traces:

1. The handler calls `record_admission(AcceptedAdr, explicit_user/
   user_explicit, C)`: the Kernel refuses (`admission.rs:268-274`) because
   C's prior is `model_inference`. If the handler instead first supersedes C
   with a spec and admits the successor as `explicit_user`, the successor is
   a new object on C's `model_inference` lineage carrying user authority:
   model text promoted by lineage. The check "no `explicit_user` admission on
   any object whose `source_kind` is a plugin kind" catches it.
2. The handler records D but not the reject: C stays Labeled and D is served;
   two copies of one rule with different authority. Not forbidden by the
   Kernel, forbidden by R3.
3. `confirmed_from` names a `user_directive` object: the handler would reject
   a directive through a model-class-free path; must be refused as invalid.

## Timing windows and dependencies

Same envelope; a concurrent model `revise` of C serializes before or after.
If before, C is invalidated and the route's `scoped_object_state` returns
`NotFound`; the route must report refresh-and-retry.

## What a test must construct

- A `model_inference` candidate C through `kernel.commit`.
- `directive.record` with `confirmed_from = C`.
- Assert: a new live `user_directive` decision D whose rationale JSON has
  `confirmed_from == C`; C's latest admission `disposition='rejected'`,
  `visibility='review_only'`, `source_class='model_inference'`; C's object
  not invalidated; a read on ExplicitSearch no longer labels C (ReviewOnly is
  Hidden there per `surface_visibility`).
- Negative: `confirmed_from` naming D itself or a retired id is refused.

## Investigation log

### Q: Is there any Kernel path that changes a subject's class?
- Sources examined: `admission.rs:264-274`, `load_prior_decision`.
- Findings: none; the prior-class check is in the digest-pinned evaluator.
- Missing evidence: none.
- Conclusion: resolved with answer: no.

### Q: What outcome for an unknown or non-model `confirmed_from`?
- Sources examined: plan U3.
- Findings: not specified.
- Missing evidence: design statement.
- Conclusion: unresolved, resolvable at implementation.
