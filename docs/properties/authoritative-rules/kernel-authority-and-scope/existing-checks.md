# Existing checks: Kernel admission and scope eligibility (S1, S2)

Every check is listed with its location, what it asserts, and the stable
message or assertion shape. Status is `unaudited` for every entry; adequacy
belongs to `/testing:invariant-test-review` (tests) or
`/low-level-systems:defensive-assertions-and-invariant-guards` (production
guards). HEAD `08de9051a`; lines verified.

## Production guards (runtime refusals and invariants)

| Location | Semantics | Message or error | Status |
| --- | --- | --- | --- |
| `crates/kernel/src/admission.rs:265-282` `evaluate_admission` preconditions | illegal class pair, class drift from the prior, effective above historical on the prior, `Enforce` without evidence, `Correct`/`Replace` without predecessor sensitivity | `KernelError::AdmissionPolicy` | unaudited |
| `admission.rs:294-303` remaining-support rule | `ApprovalRevoked` must state support at or below current effective; any other event must state none | `AdmissionPolicy` | unaudited |
| `admission.rs:1339-1349` `refuse_barred_lineage` | predecessor or fold survivor whose latest disposition is `Quarantined`, `Rejected`, `Contradicted` cannot be superseded | `AdmissionPolicy` | unaudited |
| `admission.rs:1364-1366` `revoke_approval_inner` | blank reason refused | `AdmissionPolicy` | unaudited |
| `admission.rs:1368-1382` `revoke_approval_inner` | target must satisfy `APPROVAL_OBJECT_PREDICATE` | `NotFound` | unaudited |
| `admission.rs:1526-1551` `prepare_admission` | exactly one of candidate or subject; a materialized candidate cannot take candidate-keyed decisions | `AdmissionPolicy` | unaudited |
| `admission.rs:1558-1562` `prepare_with_facts` | classes required, reason non-blank | `AdmissionPolicy` | unaudited |
| `admission.rs:2358-2381` `enforce_lineage_bearer_cap` | at most 64 authority bearers per lineage | `AdmissionPolicy` | unaudited |
| `admission.rs:2383-2433` `enforce_approval_dependent_cap` | at most 1,024 dependents per approval | `AdmissionPolicy` | unaudited |
| `admission.rs:2549-2552` `load_approval_dependents` | over-cap dependent set refused | `AdmissionPolicy` | unaudited |
| `admission.rs:2559-2567` `refuse_succession_events` | callers cannot name `Correct`, `Replace`, `ApprovalRevoked` | `AdmissionPolicy` | unaudited |
| `admission.rs:2569-2591` `validate_provenance` | repository provenance forbids explicit-user classes; unclassified forbids trusted classes (candidates only) | `AdmissionPolicy` | unaudited |
| `admission.rs:2726-2795` `validate_trigger` | trigger must be a live observation of the expected kind on the exact lineage with live backing evidence; else the event degrades to `Other` | no error; `None` | unaudited |
| `admission.rs:1012-1034` `approval_chain_members` | chain longer than 64 hops refused | `AdmissionPolicy` | unaudited |
| `crates/kernel/src/slice/write.rs:388-394` `correct_decision_inner` | survivor equal to predecessor, or survivor classified below the correction, refused | `InvalidInput` | unaudited |
| `write.rs:1271-1285` `validate_successor` | domain, source kind, source id must match; revision must increase | `InvalidInput`, `Conflict` | unaudited |
| `write.rs:1287-1292` `require_spec_fields` | negative revision or blank identity field | `InvalidInput` | unaudited |
| `crates/daemon/src/kernel_routes/commit.rs:283-316` `resolve_classes` | unknown `source_kind` or illegal pair is invalid input; asserted class above derived is `ClassOverDeclared` | `InvalidReason::InvalidInput`, `InvalidReason::ClassOverDeclared` | unaudited |
| `commit.rs:479-484` `caller_domain` | route-owned `project-scopes` domain refused | `InvalidInput` | unaudited |
| `commit.rs:811-836` supersede route | replacement live survivor must be in project; survivor sensitivity floor; `successor_revision <= predecessor` refused | `AdmissionPolicy`, `CommitFailure::RevisionNotAdvanced` + `Conflict` | unaudited |
| `commit.rs:566-600` `disposition_request` | subject must be a live decision in the bound project with an admission; approval and every chain member must be in project | `NotFound`, `AdmissionPolicy` | unaudited |
| `crates/kernel/src/eligibility.rs:76-84` `ProjectScope::new` | project digest must be lowercase SHA-256 hex | `InvalidInput` | unaudited |
| `eligibility.rs:86-94` `names_project` | absent scope, no project term, malformed scope, or non-`Matches` outcome is false | boolean | unaudited |
| `crates/kernel/src/scope.rs:628-659` `CanonicalScope::from_term_specs` | unknown dimension, duplicate dimension, undecodable term | `ScopeFormError::*` | unaudited |
| `crates/daemon/src/kernel_routes/read.rs:210-217` `read_visible` | a visible decision row without its typed decision at the snapshot fails closed | `KernelError::CorruptCanonicalRow` | unaudited |

Debug assertions: none found in S1 or S2 code paths (grep `debug_assert`
over `admission.rs`, `write.rs`, `eligibility.rs`, `scope.rs`,
`commit.rs`, `project.rs`, `read.rs` returns none in the functions this
catalog covers).

## Claim-bearing tests

### Evaluator (`crates/kernel/src/admission.rs` test module, from `:4320`)

| Test | Semantics | Status |
| --- | --- | --- |
| `every_legal_source_taint_pair_enforces_its_automatic_ceiling` (`:4523`) | for every legal pair, support-raising events without approval stop at `automatic_ceiling` | unaudited |
| `only_closed_auto_admit_event_set_avoids_approval` (`:4715`) | the auto-admit set is exactly `ExplicitReject`, `Correct`, `Replace`, `AcceptedAdr`, `CodeObserved`, `ConfigObserved` | unaudited |
| `an_accepted_adr_roots_authority_only_from_an_accepted_decision_object` (`:5066`) | `AcceptedAdr` reaches `Approved` only with `subject_is_accepted_decision` for both `Verified`-ceiling pairs | unaudited |
| `a_self_approving_record_decays_once_its_accepted_status_is_gone` (`:5283`) | an `(explicit_user, user_explicit)` `Approved` prior clamps to `Verified` when the subject stops being an accepted decision | unaudited |
| `revoked_authority_cannot_carry_support_above_the_automatic_ceiling` (`:5185`) | `supporting_authority == Some(false)` clamps carried support | unaudited |
| `approval_revocation_requires_and_preserves_remaining_support` (`:5428`) | `ApprovalRevoked` needs a stated remaining support at or below effective | unaudited |
| `relaxing_a_restrictive_disposition_requires_approval` (`:5334`) | moving toward `Active` without a valid approval is denied | unaudited |
| `visibility_matrix_and_sensitivity_ceiling_are_total` (`:4910`) | `surface_visibility` and `visibility_row` over the full product | unaudited |
| `support_loss_demotes_effective_maturity_without_rewriting_history` (`:4781`) | effective falls, historical stays | unaudited |
| `policy_revision_matches_policy_content` (`:5734-5821`) | digest of marked sections equals `REVISION_1_SOURCE_DIGEST`; `POLICY_REVISION == 1` | unaudited |
| `the_served_fold_is_under_the_serving_digest` (`:5703`) | the serving fold sits inside a digest-marked section | unaudited |

### Store (`crates/kernel/tests/kernel_admission.rs`)

| Test | Semantics | Status |
| --- | --- | --- |
| `a_config_observation_authorizes_only_a_config_event` (`:1292`) | `config_present` trigger: `CodeObserved` denied, `ConfigObserved` promotes to `verified` | unaudited |
| `an_observation_at_another_source_revision_cannot_authorize` (`:1323`) | revision skew denies | unaudited |
| `an_invalidated_observation_cannot_authorize` (`:1339`) | invalidated trigger denies | unaudited |
| `observation_from_another_source_cannot_authorize_candidate` (`:677`) | foreign source id denies | unaudited |
| `a_trigger_whose_evidence_is_gone_cannot_admit` (`:4211`) | dead backing evidence denies | unaudited |
| `an_admitted_object_carries_the_class_of_its_trigger_and_the_evidence_behind_it` (`:4150`) | trigger and evidence sensitivity fold into the served class | unaudited |
| `only_an_accepted_decision_object_self_approves` (`:1740`) | `AcceptedAdr` on a domain object is `deny`, not `Approved` | unaudited |
| `a_self_admitting_record_does_not_inherit_a_revoked_approval` (`:4269`) | `AcceptedAdr` does not back-fill a stored approval | unaudited |
| `approval_revocation_fans_out_support_demotion_without_rewriting_history` (`:469`) | `revoke_approval` demotes dependents; history intact | unaudited |
| `revocation_cascades_through_dependent_approvals_and_strips_their_authority` (`:1582`) | transitive cascade | unaudited |
| `retiring_an_approval_demotes_its_dependents` (`:2614`) | `retire_decision` runs the same cascade | unaudited |
| `quarantining_an_approval_demotes_its_dependents_and_ends_its_authority` (`:1503`) | restrictive disposition on an approval cascades | unaudited |
| `revocation_survives_a_dependent_on_a_superseded_policy_revision` (`:2030`) | deferred dependent does not fail the revocation | unaudited |
| `an_unvisited_descendant_of_a_revoked_root_grants_nothing` (`:2081`) | chain validity independent of the walk | unaudited |
| `a_caller_cannot_manufacture_an_approval_revocation` (`:1473`) | `ApprovalRevoked` refused from callers | unaudited |
| `approval_dependent_capacity_blocks_new_grants_and_keeps_revocation_possible` (`:1087`) | 1,024 cap; revocation still works at cap | unaudited |
| `an_authority_chain_past_the_depth_bound_is_refused` (`:2442`) | 64-hop chain bound | unaudited |
| `superseding_a_quarantined_predecessor_is_rejected` (`:991`) | barred lineage refusal | unaudited |
| `a_hidden_approval_is_not_authority` (`:4326`) | hidden approval grants nothing | unaudited |
| `governing_decision_sensitivity_hides_an_object_the_registry_calls_normal` (`:3422`) | served class comes from the decision history | unaudited |
| `dispositions_and_sensitivity_only_reduce_visibility` (`:2735`) | monotone narrowing | unaudited |
| `every_non_active_disposition_has_the_contracted_surface_row` (`:2890`) | `Rejected -> review_only`, `Quarantined | Contradicted -> audit_only`, others `explicit_labeled` | unaudited |
| `an_approval_needs_standing_of_its_own_not_its_lineage` (`:4017`) | approval row fields judged on the own row | unaudited |
| `decisions_bound_to_another_source_lineage_never_serve` (`:3530`) | lineage-bound serving | unaudited |

### Slice (`crates/kernel/tests/kernel_slice.rs`)

| Test | Semantics | Status |
| --- | --- | --- |
| `supersede_decision_replaces_a_live_predecessor_and_folds_merges_into_a_survivor` (`:1095`) | fresh successor; predecessor invalidated and linked; fold into live survivor writes no row | unaudited |
| `supersede_decision_refuses_a_quarantined_predecessor` (`:1186`) | barred predecessor | unaudited |
| `a_fold_refuses_a_quarantined_survivor` (`:1225`) | barred survivor | unaudited |
| `corrections_reject_cross_domain_replacements` (`:1002`) | `validate_successor` domain rule | unaudited |
| `a_correction_cannot_relabel_a_predecessor_below_its_class` (`:1335`) | successor sensitivity floor | unaudited |
| `corrections_preserve_old_rows_and_reauthor_observation_dependencies` (`:606`) | predecessor history kept | unaudited |

### Scope and eligibility

| Test | Semantics | Status |
| --- | --- | --- |
| `crates/kernel/tests/kernel_eligibility.rs::a_scope_names_a_project_only_through_an_exact_project_term` (`:148`) | malformed digest refused; `branch` term alongside `project` is false; foreign project false; `None`, empty, redacted false | unaudited |
| `kernel_eligibility.rs::the_other_project_sees_its_own_rows_and_nothing_of_the_first` (`:120`) | project isolation | unaudited |
| `kernel_eligibility.rs::every_verdict_class_and_precedence_pair_matches_the_expected_table` (`:72`) | verdict precedence including `WrongScope` | unaudited |
| `crates/kernel/tests/kernel_scope_algebra.rs::missing_context_value_for_constrained_dimension_is_uncertain` (`:325`) | no context value yields `Uncertain` | unaudited |
| `kernel_scope_algebra.rs::placeholder_terms_never_match_or_subsume` (`:272`) | redacted term is `Uncertain` | unaudited |
| `kernel_scope_algebra.rs::definitive_mismatch_skips_later_graph_terms` (`:335`) | `DoesNotMatch` short-circuits | unaudited |
| `kernel_scope_algebra.rs::redacted_bounds_and_requirements_decode_uncertain` (`:440`) | redacted range or version terms are `Uncertain` | unaudited |
| `crates/daemon/tests/kernel_routes.rs::rows_serve_only_to_the_project_their_scope_names` (`:1803`) | route read excludes other projects | unaudited |
| `kernel_routes.rs::a_row_under_any_scope_naming_the_project_is_readable_and_mutable` (`:3311`) | any scope with the project term serves | unaudited |
| `kernel_routes.rs::a_foreign_scope_holding_the_reserved_project_id_refuses_every_write` (`:3444`) | reserved scope id protection | unaudited |

### Route classes and dispositions (`crates/daemon`)

| Test | Semantics | Status |
| --- | --- | --- |
| `crates/daemon/src/kernel_routes/commit.rs::a_plugin_write_is_capped_at_inference_classes` (`:1071`) | derived pairs; over-declaration refused; unknown kind invalid | unaudited |
| `commit.rs::a_pair_is_accepted_exactly_when_not_raised_and_legal_for_admission` (`:1117`) | exhaustive over four kinds and every class pair | unaudited |
| `commit.rs::every_disposition_event_names_a_disposition_transition` (`:1151`) | the five events map to disposition events; `approve` rejected at decode | unaudited |
| `crates/daemon/tests/kernel_routes.rs::a_plugin_route_cannot_declare_a_class_above_the_derived_one` (`:1145`) | `class_over_declared`; inference rows labeled on `explicit_search`, absent on automatic surfaces | unaudited |
| `kernel_routes.rs::disposition_events_apply_the_fixed_table_and_report_outcome_and_disposition` (`:4392`) | `explicit_reject -> reject/rejected` and the other four | unaudited |
| `kernel_routes.rs::a_relaxation_is_denied_without_a_valid_approval_and_permitted_with_one` (`:4452`) | denied relaxation records a row; approval permits | unaudited |
| `kernel_routes.rs::disposition_approval_chains_of_every_kind_are_project_scoped` (`:4562`) | approval chain members must be in project | unaudited |
| `kernel_routes.rs::a_disposition_needs_a_live_admitted_decision` (`:4882`) | `NotFound`/`AdmissionPolicy` for missing admission | unaudited |
| `kernel_routes.rs::a_merge_folds_every_predecessor_into_one_survivor_in_one_commit` (`:1066`) | route fold semantics | unaudited |
| `kernel_routes.rs::recorded_supersede_into_a_retired_id_is_already_exists` (`:1426`) | retired replacement id refused | unaudited |
| `transform_canonical_memory.rs::transform_composes_project_memory_from_canonical_rows_and_observes_absence` (`:46`) | a retired and a superseded decision are absent from the served `<project-memory>` block at the next snapshot (`:118-131`, `:146-147`); added for `served-rows-exclude-objects-invalidated-at-the-snapshot` | unaudited |

## Categories with none found

- Tests of `supersede_decision` against a `Rejected` or `Contradicted`
  predecessor: none found (only `Quarantined`).
- Tests that a successor admitted under different classes than its
  predecessor is accepted or refused: none found.
- Tests of the route's `retire_decision`, `supersede_decision`, or
  `disposition` when the subject's latest admission is `explicit_user` and
  the caller is `model_inference`: none found.
- Tests comparing the SQL Project prefilter's kept set to `names_project`:
  none found.
- Tests reading a stored scope with an `entity` term through `read_visible`:
  none found.
- Tests recording `ConfigObserved` on a decision object subject (as opposed
  to a staged candidate): none found.
- Tests reading a `Sensitive` self-approved `adr_accepted` decision on
  `AutoInject`: none found.
- Production assertions (`debug_assert!`, `assert!`) in S1/S2 hot paths:
  none found.

## Suspiciously quiet areas

- `demote_authority_dependents` deferred set: the comment at
  `admission.rs:1424-1446` names the visibility repair as remaining work;
  one test covers the policy-revision deferral, none covers the 4,096 cap
  being hit.
- `with_authority_cascade` for candidate-scoped decisions
  (`lineage_authority_bearers`, `:1084-1101`): covered by
  `a_lineage_rejection_demotes_what_the_rejected_authority_supported`
  (`kernel_admission.rs:4592`); the directive channel uses object subjects
  only, so this path is outside the plan's use.
- `ScopeFilter` memoization per request (`project.rs:198-206`): no test
  asserts the memo is keyed by `scope_id` alone.
