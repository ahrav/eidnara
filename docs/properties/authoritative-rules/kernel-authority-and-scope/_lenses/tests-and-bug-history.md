# Lens: existing test strategy and bug history (S1, S2)

Pass focus: what the tests already exercise, what they leave quiet, and
what the tracker says.

## Test inventory consulted

- `crates/kernel/src/admission.rs` test module (test module from `:4320`):
  evaluator-level tests over `evaluate_admission` with synthetic
  `EvaluationInputs`; the digest pin `policy_revision_matches_policy_content`
  (`:5734-5821`).
- `crates/kernel/tests/kernel_admission.rs` (6,792 lines): store-level tests
  over `record_admission`, `revoke_approval`, `retire_decision`, triggers,
  approval chains, cascades, and serving.
- `crates/kernel/tests/kernel_slice.rs` (2,133 lines): `supersede_decision`,
  folds, barred predecessors, sensitivity relabeling, cross-domain refusal.
- `crates/kernel/tests/kernel_eligibility.rs` (467 lines):
  `names_project` behavior, verdict precedence table, other-project isolation.
- `crates/kernel/tests/kernel_scope_algebra.rs` (834 lines): `scope_matches`
  outcomes including `Uncertain` for a missing context value and placeholder
  terms; proptest strategies for contexts.
- `crates/daemon/tests/kernel_routes.rs` (5,567 lines): `kernel.commit`
  route behavior, class over-declaration, disposition events, approval
  scoping, project isolation; the `seed_approval` helper (`:4356-4387`)
  constructs the `insert adr_accepted then AcceptedAdr at (explicit_user,
  user_explicit)` sequence the plan reuses.
- `crates/daemon/src/kernel_routes/commit.rs` test module (`:1067-1207`):
  `derive_classes` and `resolve_classes` exhaustively over every pair.

## What the tests exercise (claim-bearing, status unaudited)

Listed with semantics in `existing-checks.md`. Highlights:

- Self-approval: `an_accepted_adr_roots_authority_only_from_an_accepted_decision_object`
  (`admission.rs:5066`), `only_an_accepted_decision_object_self_approves`
  (`kernel_admission.rs:1740`), `a_self_approving_record_decays_once_its_accepted_status_is_gone`
  (`admission.rs:5283`).
- Triggers: `a_config_observation_authorizes_only_a_config_event`
  (`kernel_admission.rs:1292`), `an_observation_at_another_source_revision_cannot_authorize`
  (`:1323`), `an_invalidated_observation_cannot_authorize` (`:1339`),
  `observation_from_another_source_cannot_authorize_candidate` (`:677`),
  `a_trigger_whose_evidence_is_gone_cannot_admit` (`:4211`),
  `an_admitted_object_carries_the_class_of_its_trigger_and_the_evidence_behind_it` (`:4150`).
- Cascade: `approval_revocation_fans_out_support_demotion_without_rewriting_history`
  (`:469`), `revocation_cascades_through_dependent_approvals_and_strips_their_authority`
  (`:1582`), `retiring_an_approval_demotes_its_dependents` (`:2614`),
  `quarantining_an_approval_demotes_its_dependents_and_ends_its_authority`
  (`:1503`), `revocation_survives_a_dependent_on_a_superseded_policy_revision`
  (`:2030`), `a_caller_cannot_manufacture_an_approval_revocation` (`:1473`).
- Succession: `supersede_decision_replaces_a_live_predecessor_and_folds_merges_into_a_survivor`
  (`kernel_slice.rs:1095`), `supersede_decision_refuses_a_quarantined_predecessor`
  (`:1186`), `a_fold_refuses_a_quarantined_survivor` (`:1225`),
  `corrections_reject_cross_domain_replacements` (`:1002`),
  `a_correction_cannot_relabel_a_predecessor_below_its_class` (`:1335`),
  `superseding_a_quarantined_predecessor_is_rejected` (`kernel_admission.rs:991`).
- Scope: `a_scope_names_a_project_only_through_an_exact_project_term`
  (`kernel_eligibility.rs:148`), `missing_context_value_for_constrained_dimension_is_uncertain`
  (`kernel_scope_algebra.rs:325`), `placeholder_terms_never_match_or_subsume`
  (`:272`), `rows_serve_only_to_the_project_their_scope_names`
  (`kernel_routes.rs:1803`), `the_other_project_sees_its_own_rows_and_nothing_of_the_first`
  (`kernel_eligibility.rs:120`).
- Route classes: `a_plugin_route_cannot_declare_a_class_above_the_derived_one`
  (`kernel_routes.rs:1145`), `a_plugin_write_is_capped_at_inference_classes`
  (`commit.rs:1071`), `a_pair_is_accepted_exactly_when_not_raised_and_legal_for_admission`
  (`commit.rs:1117`).
- Dispositions: `disposition_events_apply_the_fixed_table_and_report_outcome_and_disposition`
  (`kernel_routes.rs:4392`), `a_relaxation_is_denied_without_a_valid_approval_and_permitted_with_one`
  (`:4452`), `every_non_active_disposition_has_the_contracted_surface_row`
  (`kernel_admission.rs:2890`).

## Suspiciously quiet areas

- No test exercises `supersede_decision` on a predecessor whose latest
  disposition is `Rejected` (tests cover `Quarantined` only). The code path
  `refuse_barred_lineage` names all three.
- No test asserts that a successor admitted under different classes than its
  predecessor is accepted or refused; the behavior (accepted) is read from
  code only.
- No test asserts that a `kernel.commit` `retire_decision`, `supersede_decision`,
  or `disposition` from a `model_inference` caller succeeds or fails against
  a subject whose latest admission is `explicit_user`. The plan's KTD9 adds
  the guard; the baseline is untested in either direction.
- No test asserts the superset relation between the SQL Project prefilter and
  `names_project` (every row SQL drops would be refused by the filter).
- No production-path test constructs a stored scope with an `entity`,
  `branch`, or `environment` term and reads it through `read_visible`; the
  unresolved-dimension exclusion is tested at the `ProjectScope` and
  `scope_matches` levels only.
- No test records `ConfigObserved` through the daemon; `observe_code` in
  `crates/daemon/src/lib.rs:13853` is test-support only (gate at `:13517`) and uses
  `CodeObserved`.

## Tracker history (gh search, all states)

Searches: "memory injection", "admission", "scope matching", "project
memory", "harness parity", "packing budget", "AcceptedAdr",
"revoke_approval", "supersede", "ScopeFilter". No issue reports a bug or
incident in admission mechanics or scope matching. Related leads:

- #307 (closed): canonical memory injection from `kernel.read` visible rows;
  acceptance criteria name absence of retired, superseded, other-project, and
  non-positive-category decisions. Treated as a documented contract for the
  serve path, not as evidence of a defect.
- #353 (closed): shared kernel eligibility with daemon verdicts; reinforces
  that `ScopeFilter` and `ProjectScope::names_project` are one judgement.
- #311 (closed): privileged-key table closed and build-checked; a precedent
  for the closed-table discipline `derive_classes` follows.
- #579, #657, #408, #709: settled review-path decisions (Candidate ceiling
  for model output, per-surface verdicts, no boost from unknown lineage,
  Kernel-only content authority). None names a defect in S1 or S2.

No incident logs or reproducers exist for S1/S2, so no behavior-level
regression property is promoted from history.
