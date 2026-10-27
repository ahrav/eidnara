# disposition-admission-copies-the-subjects-prior-classes

Catalog record: [catalog.md](../catalog.md#disposition-admission-copies-the-subjects-prior-classes).
Authored at `3a3b916b7`, 2026-10-25; references re-verified at the PR A
tip of #1160 (code commit `6c4d445e3`), with PR A changes noted inline.

## Discovery trigger

Plan P4 Problem Frame: "the event is recorded under the subject's prior
source class (`:587-588`)". Lens: claimed safety guarantees, then data
integrity. The `commit.rs` header (`:7-10`) says every written row is
"admitted under the derived classes", which disagrees with the plan's
reading; code resolves which describes the disposition path.

## Evidence trail

- `disposition_request` builds the request with `source_class:
  Some(prior.source_class)` and `taint_class: Some(prior.taint_class)`
  (`crates/daemon/src/kernel_routes/commit.rs:725-726`). `prior` is the
  `PriorDecision` from `envelope.subject_admission(object_id)`
  (`:710-712`), which reads `load_prior_for_key(AdmissionKey::Object)`
  (`crates/kernel/src/admission.rs:1006-1013`).
- `CommitPlan.classes` (the caller's derived pair from `resolve_classes`,
  `commit.rs:411-444`) is read only by `admit` (`:579-598`) for inserts
  and supersedes. The `Operation::Disposition` arm in `apply`
  (`:998-1004`) never touches it.
- `evaluate_admission` refuses when the request's classes differ from the
  prior's (`admission.rs:268-273`), so the copy is also what makes the
  route's dispositions pass that gate for every subject class.
- `write_admission` binds `prepared.source_class.as_str()` and
  `prepared.taint_class.as_str()` into the row (`admission.rs:1767-1768`);
  `prepared.source_class` is the request's (`prepare_with_facts`,
  `:1563-1564`, `:1666-1667`).
- Route test `:5493` quarantines `approval-1`, an `ExplicitUser`/
  `UserExplicit` subject seeded by `seed_approval`
  (`kernel_routes.rs:4362-4393`), through a route whose caller derives
  (`ModelInference`, `AssistantInference`); the commit is `available`, so
  the copy is observable at the route today for one event.

## Failure scenario

A regression writes `plan.classes` into the disposition request. For a
subject whose prior is `ExplicitUser`/`UserExplicit`, the evaluator
refuses (`AdmissionPolicy`) and no plugin can dispute a user-authored
decision. If the evaluator check were also loosened, the row would
relabel the subject's lineage as model inference, and the subject's next
`automatic_ceiling` would drop to `Candidate`.

## Timing windows and dependencies

None in time. Depends on `subject_admission` returning the object-scoped
latest row, not the lineage row (`admission.rs:2172-2181` doc).

## What a test must construct

- A live admitted decision in the bound project whose prior classes are
  not (`ModelInference`, `AssistantInference`) or (`ModelInference`,
  `UserInferred`): `seed_approval` gives (`ExplicitUser`, `UserExplicit`);
  a `CodeObserved` admission gives (`TrustedLocalCode`, `CurrentCode`)
  (`kernel_routes.rs:5136-5146`).
- A `kernel.commit` disposition under `source_kind: "assistant"` and
  under `"user"`.
- A read of the written row's `source_class`, `taint_class`, and
  `event_kind` by `commit_seq`; compare with the prior row.
- Coverage marker `disposition_subject_classes_differ_from_caller`.

## Investigation log

### Q: Which wording is the contract, the header's "derived classes" or the plan's "prior source class"?

- Sources examined: `commit.rs:1-10`, `:579-598`, `:694-736`,
  `admission.rs:264-273`, plan P4 Problem Frame and R4.
- Findings: the header describes inserts; the disposition arm copies the
  prior. Both statements are true of their own paths; the header's
  "every written row" is over-broad for admission rows written by a
  disposition.
- Missing evidence: an owner statement of which classes a disposition
  row should carry once P4 adds a reporter class (OQ4-1 picks a
  structured `reason`, leaving the columns as the subject's).
- Conclusion: needs human input.
