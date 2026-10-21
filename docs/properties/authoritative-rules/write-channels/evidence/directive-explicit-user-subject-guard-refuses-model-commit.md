# directive-explicit-user-subject-guard-refuses-model-commit

## Discovery trigger

Plan requirement R9 and KTD9: "`kernel.commit` ops `supersede_decision`,
`retire_decision`, and `Disposition` refuse a subject whose latest admission
has `source_class = explicit_user` when the caller's derived class is
`model_inference`. Today `eidnara_memory retire` can retire any scoped
decision." U3 step 6. Forbidden outcome F2.

## Evidence trail

Verified at HEAD (the gap the guard closes):

- `crates/daemon/src/kernel_routes/commit.rs:849-853` `Operation::RetireDecision`
  calls `scoped_object_state` (project scope only) then
  `envelope.retire_decision(object_id)`. No class comparison.
- `commit.rs:787-848` `Operation::SupersedeDecision`: scope check on the
  predecessor, revision check, sensitivity floor, `supersede_decision`, then
  `admit(successor, plan.classes)`. The predecessor is invalidated without any
  admission on it, so the Kernel's class-continuity check never sees the
  explicit_user prior. The successor is a fresh `model_inference` Candidate.
- `commit.rs:566-600` `disposition_request` builds the admission request with
  `prior.source_class` and `prior.taint_class` of the subject, not the
  caller's derived classes. A model-mediated `explicit_reject` on an
  explicit_user subject therefore passes `evaluate_admission`'s prior check
  (`admission.rs:268-274`) and records a Rejected disposition under
  `explicit_user` classes.
- `crates/kernel/src/slice/write.rs:627-665` `retire_slice_object` runs the
  authority cascade: retiring a directive demotes its dependents, so the
  silencing also strips support from rows the user had blessed through it.
- `packages/opencode-plugin/src/tools/eidnara-memory/execute.ts:665-690`: the
  tool reads Visible rows and sends `retire_decision` for any of them; a
  Visible directive is a valid target today.
- `admission.rs:1001-1008` `Envelope::subject_admission` returns the latest
  `PriorDecision` (with `source_class`) through the same-transaction cache:
  the mechanism the guard can use.
- `commit.rs:985-1000`: `plan.classes` is resolved before the envelope, so
  the caller's derived class is available in `apply`.
- Scope interaction: `crates/kernel/src/eligibility.rs:86-93` `names_project`
  evaluates `scope_matches` with a Project-only context;
  `crates/kernel/src/scope.rs:1418-1420` returns `Uncertain` for a term whose
  dimension has no context value; `names_project` requires `Matches`. A
  compound scope (Project plus Entity `dir:`) is therefore `NotFound` to
  `scoped_object_state` at HEAD. The guard is load-bearing for root-scoped
  directives.

## Failure scenario

Legal witness: directive D (root scope) Visible; model tool `retire D`
through `kernel.commit` with `source_kind: assistant`; the route resolves
classes `(ModelInference, AssistantInference)`, reads D's latest admission
`source_class = explicit_user`, refuses with the invalid-input outcome before
any write; D unchanged. The same `retire` on a `model_inference` row M
succeeds.

Smallest violating trace (reproducible at HEAD): the same request lands;
`retire_slice_object` invalidates D; the next pass has no directive and no
record of who removed it beyond the commit log actor.

Second trace: model `revise D` -> `SupersedeDecision{replaced D, spec rev 2}`;
D invalidated with `superseded_by` a Candidate model row; the user's text is
replaced by model text in the lineage's history.

Third trace: model `explicit_reject D` -> D Rejected, `review_only`; Hidden
on AutoInject; `refuse_barred_lineage` then blocks a user `revise` of D.

## Timing windows and dependencies

The guard must read the latest admission inside the envelope, after
`scoped_object_state`, before any mutation. A directive recorded in an
earlier commit is visible; one recorded in the same envelope cannot occur
(different route). A `kernel.commit` with multiple operations must evaluate
the guard per operation before the first write, or rely on the envelope
poisoning to roll the whole commit back when a later operation is refused.

## What a test must construct

- D through the directive route (or the `seed_approval` store pattern until
  U3 exists); M through `kernel.commit`.
- `kernel.commit` with `source_kind: assistant` and each of
  `retire_decision D`, `supersede_decision D`, `Disposition explicit_reject
  D`; assert the invalid-input outcome, D live and unchanged, no new
  `admission_decisions` row for D, no receipt stored for the refused key (so
  a corrected request can reuse it) or a stored refusal receipt, whichever
  the route defines.
- The same three on M; assert success.
- Multi-op commit mixing one refused and one legal op; assert nothing landed.
- Ledger invariant across a campaign: no `admission_decisions` row with
  `source_class='explicit_user'` was invalidated or succeeded by a commit
  whose `commit_log.producer` is the plugin route producer.

## Investigation log

### Q: Does the Kernel already refuse any of the three at HEAD?
- Sources examined: `commit.rs:566-600, 787-853`, `admission.rs:268-274`.
- Findings: no. Disposition passes because it reuses the subject's classes;
  retire and supersede never admit the predecessor.
- Missing evidence: none.
- Conclusion: resolved with answer: all three succeed at HEAD on a root-scoped
  directive; the plan's KTD9 motivation is confirmed.

### Q: Should the guard also protect `(trusted_local_code, current_config)` rows?
- Sources examined: plan KTD9 (names `explicit_user` only).
- Findings: a model `retire` of a repository rule is undone by the next
  reconcile only if the reconcile detects the missing unit; the plan's
  digest matching compares against live rows, so a retired unit looks "new"
  and is re-inserted. Self-healing but noisy.
- Missing evidence: design decision.
- Conclusion: needs human input (recorded in open-questions.md).
