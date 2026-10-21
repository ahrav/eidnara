# Lens: security boundaries, unproven assumptions, and wildcard

## Security boundaries (escalation through valid sequences)

- Class forgery through the plugin route: `derive_classes` is closed and
  `resolve_classes` refuses an asserted class ranked above the derived one
  (`commit.rs:283-316`). A plugin cannot name `user_directive` or
  `repository_policy` as `source_kind` today (`None` -> `InvalidInput`).
- Self-approval forgery: `AcceptedAdr` self-approves only when the subject is
  a live `adr_accepted` decision (`admission_ceiling`, `:4221-4234`) and the
  classes have a `Verified` ceiling. The route never emits `AcceptedAdr`, so
  the pair `(explicit_user, user_explicit)` is unreachable from a plugin.
  `(explicit_user, user_inferred)` also has a `Verified` ceiling and would
  self-approve an `adr_accepted` decision; no writer exists for it either.
- Authority grant forgery: an approval must satisfy `approval_row_fields`
  (`:843-856`) and `approval_qualifies_predicate` (`:861-905`), which require
  `user_explicit` taint, `Normal` sensitivity at the object, the history, and
  the cited evidence, and policy revision 1.
- Model-mediated authority change (the plan's KTD9 gap): at HEAD the commit
  route's `RetireDecision` (`commit.rs:851-854`) and `SupersedeDecision`
  (`:787-850`) check scope only; `Disposition` builds the request from the
  subject's stored classes (`:589-590`), so the evaluator's class-consistency
  rule (`admission.rs:268-274`) is satisfied for an `explicit_user` subject.
  A model caller can therefore reject, retire, or supersede any decision in
  its project, and the cascade then demotes that decision's dependents.
- Trigger forgery: a `config_present` observation from another lineage,
  another revision, an invalidated observation, or one with dead backing
  evidence yields `None` from `validate_trigger` and the event becomes
  `Other` (`:1640-1644`). An observation written through the plugin route
  is admitted at `model_inference` classes, but `validate_trigger` does not
  read the observation's admission classes: it reads the observation's
  registry row, kind, lineage, and sensitivity. A plugin-written
  `config_present` observation on the right lineage would satisfy the trigger
  check for a `ConfigObserved` event; the event itself is not reachable from
  the plugin (`admit` uses `Other`). Recorded as an open question on the
  trigger record.
- Scope widening: `names_project` requires a Project term that equals the
  binding digest; a scope with no term, a redacted term, or a term on a
  dimension the context lacks is refused (`eligibility.rs:86-94`).

## Unproven assumptions found in code comments

- `admission.rs:1424-1446`: the cascade accepts holding the single writer for
  up to 4,096 demotions, and "chunking the visibility repair across
  transactions is the remaining work". A deferred dependent keeps its stored
  elevated visibility until a later write; `validate_approval` says it grants
  nothing, but `decided_row` serves the stored row. Recorded on the cascade
  record as a bounded-window open question.
- `admission.rs:4321-4330` (`served_visibility_row` callers): serving folds
  stored and expected visibility; an inconsistent own history fails closed
  (`own_history_inconsistent_sql`, `:639-659`).
- `write.rs:1254`: a live but superseded row decodes with `superseded_by:
  None` in some projections; eligibility reads `superseded_by` from the
  registry state. Noted; S4 owns the read.

## Wildcard

- `supersede_decision` requires `source_revision` strictly greater, but the
  commit route also compares against a live replacement's stored revision
  (`commit.rs:811-832`); a fold into an existing live survivor keeps the
  survivor's row and admits nothing new (`:843-847`). A directive revise that
  accidentally names a live object id would fold rather than create.
- `validate_provenance` constrains staged candidates only; a decision object
  never carries a provenance kind into admission (`:2156`). The plan's
  repository provenance therefore protects the artifact's sensitivity, not
  the decision's admission class.
- `adr_accepted` is absent from `POSITIVE_MEMORY_CATEGORIES`
  (`memory_render.rs:67-80`), so a self-approved directive decision in the
  memory domain is Visible on AutoInject at the kernel yet dropped by the
  renderer at HEAD. Lead for the S4 pass.
- `MAX_LINEAGE_AUTHORITY_BEARERS` (64, `:782`) caps accepted decisions per
  lineage; directives at one lineage per uuid never approach it; a
  repository-policy lineage holds `(trusted_local_code, current_config)`
  decisions that are not approvals, so it is unaffected.
