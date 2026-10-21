# Lens: state, persistence, and the policy evaluator (S1)

Pass focus: stored state, how a decision row is computed from prior state,
what persists after each event.

## Tables and keys

- `admission_decisions` holds one row per recorded decision, keyed
  `"{commit_seq}:{ordinal:020}"` (`admission.rs:1677`). The prior for an
  object subject is its latest committed row (`load_prior_for_key`,
  `:2186-2282`), read through the envelope's same-transaction cache first.
- `object_registry.invalidated_commit_seq` and the typed table's
  `superseded_by` carry retirement and succession; no admission row is
  written on the predecessor by `supersede_decision` or `retire_decision`
  (verified: `EventKind::Correct | Replace` are recorded only by
  `supersede_domain`, `admission.rs:1269-1305`).
- `scope_term` rows (`scope.rs:416-461`) hold one term per `(scope_id,
  ordinal)`; `CanonicalScope::from_term_specs` rejects duplicate dimensions
  and unknown vocabulary (`:628-659`).

## Evaluator state transitions (`evaluate_admission`, `:264-392`)

- Class consistency: a prior with different `source_class` or `taint_class`
  than the request is `AdmissionPolicy` (`:268-274`). This is per subject
  key; a new successor object has no prior, so classes across succession are
  unconstrained by the evaluator.
- `automatic_ceiling` (`:4197-4219`): `(ExplicitUser, UserExplicit |
  UserInferred)` and `(TrustedLocalCode | TrustedToolResult, CurrentCode |
  CurrentTest | CurrentConfig)` are `Verified`; every other legal pair is
  `Candidate`.
- `admission_ceiling` (`:4221-4234`): `AcceptedAdr` on a subject that is a
  live `adr_accepted` decision with automatic ceiling at or above `Verified`
  reaches `Approved` with no approval object.
- `event_effect` (`:4140-4161`): `AcceptedAdr | Approve -> Approved`,
  `CodeObserved | ConfigObserved | Verify -> Verified`, `ExplicitReject ->
  Rejected`, `Other -> (current, None, Deny)`.
- `auto_admit_event` (`:4172-4182`) names the events that need no approval
  below the ceiling: `ExplicitReject`, `Correct`, `Replace`, `AcceptedAdr`,
  `CodeObserved`, `ConfigObserved`. `Verify`, `Approve`, `Enforce`,
  `Corroborate` always need approval (`requires_approval`, `:324-330`).
- A non-active disposition withdraws support to `Candidate` while history
  stays monotonic (`:345-351`).
- `visibility_row` (`:4243-4255`): `Active` at `Verified+` is `Automatic`;
  `Active` below `Verified`, `Stale`, `Disputed`, `Superseded` are
  `ExplicitLabeled`; `Rejected` is `ReviewOnly`; `Contradicted |
  Quarantined` are `AuditOnly`.
- `surface_visibility` (`:4116-4138`): `Secret` is `Hidden` everywhere;
  `Sensitive` is `Hidden` on `AutoInject` and `AutoSearch`; `Automatic` is
  `Visible`; `ExplicitLabeled` is `Labeled` only on `ExplicitSearch`.

## Trigger composition (`prepare_with_facts`, `:1553-1668`)

- `validate_trigger` (`:2726-2795`) returns `None` for `CodeObserved |
  ConfigObserved` without a trigger or with a trigger that is not a live
  observation of kind `code_present | config_present` on the exact
  `(source_kind, source_id, source_revision)` of the subject with live
  backing evidence. The caller then evaluates `EventKind::Other` (`:1640-1644`).
- The trigger's sensitivity (stricter of observation and backing evidence)
  is folded into `candidate_sensitivity` (`:1605`, `:1631`).
- `validate_provenance` (`:2569-2591`) sees `provenance_kind: None` for
  every object subject (`load_subject_facts`, `:2156`) and lineage
  subject (`:2120`); it only constrains staged candidates.

## Authority state

- `APPROVAL_OBJECT_PREDICATE` (`:710-714`): live `decision` object with
  `decision_kind='adr_accepted'`; `subject_is_accepted_decision` (`:2436-2458`)
  uses the same predicate.
- `approval_row_fields` (`:843-856`): `taint_class='user_explicit'`,
  `source_class='explicit_user'`, maturity and effective maturity in
  `approved | enforced`, `disposition='active'`, `visibility='automatic'`,
  `sensitivity_class='normal'`, `policy_revision=1`.
- `approval_qualifies_predicate` (`:861-905`) adds `o.sensitivity_class=
  'normal'`, the strictest history sensitivity `normal`, no inconsistent own
  history, live normal cited evidence, and a non-restrictive lineage row.
- Supporting authority for a self-approving record is its own accepted
  status (`authority_holds`, `:308-311`): once the decision is invalidated or
  its kind no longer qualifies, the carried maturity clamps to the ceiling.

## Cascade state (`:1447-1524`)

- `load_approval_dependents` (`:2505-2555`) selects latest rows with
  `approval_object_id = X AND elevated_support = 1`, capped at
  `MAX_APPROVAL_DEPENDENTS + 1` (1,025) and refused above the cap.
- Each dependent is re-admitted with `ApprovalRevoked`; `remaining_support =
  min(prior.effective, automatic_ceiling)` (`:1617-1621`).
- Over `MAX_AUTHORITY_DEMOTIONS` (4,096) or another `policy_revision`, the
  dependent is counted `deferred` and still traversed (`:1468-1476`).

## Findings for the catalog

- Sensitivity does not block self-approval maturity; it blocks AutoInject
  visibility and authority grant. The plan's "sensitivity must be Normal"
  is therefore two guarantees, both real, at different points.
- "Superseded" status for a decision is registry invalidation plus
  `superseded_by`, not a `Disposition::Superseded` row.
