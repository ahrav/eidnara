# Attestation verifier: existing checks

Every check below has status `unaudited`. A listed check is evidence that a
claim is exercised, not a verdict that it is adequate; adequacy belongs to
`/testing:invariant-test-review` for tests and
`/low-level-systems:defensive-assertions-and-invariant-guards` for
production guards. References were verified at the #1170 branch, and the
rows #1171 added and every row pointing into a file #1171 changed were
verified at the #1171 branch.

## Production guards

| Guard | Location | Claim it carries | Status |
| --- | --- | --- | --- |
| Prior-class refusal | `crates/kernel/src/admission.rs:284` in `evaluate_admission` (`:280`) | an admission's classes equal the subject's prior admission's | unaudited |
| Automatic ceiling | `crates/kernel/src/admission.rs:4302` (`automatic_ceiling`) | inference taints cap at `Candidate`; trusted code and config pairs reach `Verified` | unaudited |
| Trigger validation | `crates/kernel/src/admission.rs:2807` (`validate_trigger`), called at `:1662`, degraded to `Other` at `:1699` | live trigger of the event's kind with the subject's source triple and live backing evidence; a failed trigger admits as `Other` | unaudited |
| Event effects | `crates/kernel/src/admission.rs:4245` (`event_effect`) | `MarkStale` sets `Stale` with `Deny`; witness events promote with no disposition | unaudited |
| Provenance | `crates/kernel/src/admission.rs:2650` (`validate_provenance`), called at `:1622` | trusted classes refused only for `unclassified` provenance | unaudited |
| Succession events refused | `crates/kernel/src/admission.rs:2640` (`refuse_succession_events`) | `Correct`, `Replace`, `ApprovalRevoked` cannot be submitted as admission events | unaudited |
| Policy revision | `crates/kernel/src/admission.rs:20` (`POLICY_REVISION`) | admissions and served reads are under one pinned policy | unaudited |
| Successor identity | `crates/kernel/src/slice/write.rs:1271` (`validate_successor`) | same domain and source pair, larger revision | unaudited |
| Barred lineage | `crates/kernel/src/admission.rs:1395` (`refuse_barred_lineage`) | quarantined, rejected, contradicted lineages cannot be superseded | unaudited |
| Cited-evidence retirement | `crates/kernel/src/slice/write.rs:585` (`retire_evidence`), `Conflict` at `:607` | cited evidence is not retired | unaudited |
| Artifact deletion | `crates/kernel/src/cas/deletion.rs:239` (`delete_artifact`), evidence update at `:389` | invalidates every live evidence row of a digest; no citation predicate | unaudited |
| Plugin class table | `crates/daemon/src/kernel_routes/commit.rs:370` (`derive_classes`) | closed `source_kind` table; `verifier` is outside it | unaudited |
| Plugin replacement | `crates/daemon/src/kernel_routes/commit.rs:1002` | a replacement carries the route's `source_kind` | unaudited |
| Daemon isolated open | `crates/daemon/src/git_sources.rs:131` (`isolated_options`), `:138` (`open`), `:148` (`open_with_slots`) | allocation limit after repository configuration; replacement refs ignored | unaudited |
| Header pass before decode | `crates/daemon/src/git_sources.rs:442` and `:456` in `read_selection` (`:390`); `exceeds_allocation` at `:354` | headers judged before any decode; allocation refusals named | unaudited |
| Kernel checkout open | `crates/kernel/src/applicability/checkout.rs:639` (`open_isolated`) | isolated, command configuration stripped; no allocation limit; replacements honored | unaudited |
| Document caps | `crates/daemon/src/project_docs.rs:23` (`MAX_PROJECT_DOC_BYTES`); `crates/daemon/src/harness_sources.rs:642` (`MAX_SOURCE_UNIT_BYTES`); unit cap applied at `crates/daemon/src/source_capture.rs:1297` | 256 KiB read, 64 KiB published | unaudited |
| Document publisher class | `crates/daemon/src/source_capture.rs:922` | documents publish at `Sensitive` | unaudited |
| Causality parents | `crates/kernel/src/claim_causality.rs:218` (`record_claim_causality`), `:38` (`MAX_DERIVATION_PARENTS`) | live exact parents; bounded parent list | unaudited |
| Verifier header gate | `crates/daemon/src/attestation_verifier.rs:298` (`header_verdict`), used at `:433` (`lookup`) and `:314` (`verdict`) | header over 256 KiB is `over_budget`; a claimed length above twice or below half the header size is `Mismatch`; both before any content read | unaudited |
| Verifier object read | `crates/daemon/src/attestation_verifier.rs:399` (`read_object`) | kind and hash checked after decode, decoded length recorded first; allocation refusal is `over_budget` | unaudited |
| Verifier delta-chain walk | `crates/daemon/src/attestation_verifier.rs:385` (`guard_delta_chain`) via `crates/daemon/src/git_sources.rs:205` (`file_object_kind`) | a ref-delta chain that revisits an entry in the repository's own packs is `object_unreadable(class)` before gix reads it | unaudited |
| Verifier deadline polls | `crates/daemon/src/attestation_verifier.rs:372` (`poll`), `:337` (`Deadline`) | polled before the open and before every object read | unaudited |
| Verifier claim path | `crates/daemon/src/attestation_verifier.rs:68` (`Claim::new`) via `crates/daemon/src/confined_path.rs:10` (`MAX_COMPONENTS`) | at most 16 ordinary components | unaudited |
| Evidence-keyed descriptor read | `crates/kernel/src/source_descriptor.rs:1213` (`source_descriptor_citing_evidence`) through `checked` (`:795`) | the live descriptor of a class citing an evidence id at a sequence, first in object id order, with the page read's row checks; `Retired` and `Uncited` otherwise | unaudited |
| Anchor-condition read | `crates/kernel/src/anchor_write.rs:468` (`anchor_condition_at`) through `validate_anchor_row` (`:287`) | the anchor live at a sequence decodes as the engine decodes it; an undecodable row is `CorruptCanonicalRow` | unaudited |
| Classifier self-source check | `crates/daemon/src/attestation_verifier/classify.rs:264` in `class_of` (`:238`) | after the `Active` filter (`:258`), both on the standing read, a live `Active` `verifier` decision is `self_source` before any anchor or citation read | unaudited |
| Classifier reason order | `crates/daemon/src/attestation_verifier/classify.rs:238` (`class_of`) | one class per decision: `NotInput` for a decision not live or not `Active`, then `self_source`, then the anchor reasons, then the citation reasons in citation order | unaudited |
| Classifier citation order and cap | `crates/daemon/src/attestation_verifier/classify.rs:328` (`citations`), `MAX_EVENT_CITATIONS` at `:42` | the row-level `evidence_id` first, then at most 32 event-level citations of other evidence | unaudited |
| Classifier incarnation recheck | `crates/daemon/src/attestation_verifier/classify.rs:233` in `classify` (`:222`) | after the last read, a store that is not the target's incarnation is `IncarnationMismatch` | unaudited |
| Classifier claim and binding | `crates/daemon/src/attestation_verifier/classify.rs:404` (`VerifierInput::new`), path refusal at `:424`, root match at `:425` | the claim's path and digest from the descriptor, its length from the evidence row, its oid from the anchor; a stored path outside the document path rule is `InvalidDocumentPath`; the first bound root with the repository identity, else `repository_unbound` | unaudited |
| Document kind rule | `crates/daemon/src/attestation_verifier/classify.rs:179` (`DocumentKind::of`) | configuration extensions are `CurrentConfig` with `config_present`; every other path is `CurrentCode` with `code_present` | unaudited |

## Tests

| Test | Location | Records | Status |
| --- | --- | --- | --- |
| `unvalidated_observation_denies_without_materializing_candidate` | `crates/kernel/tests/kernel_admission.rs:648` | degraded witness | unaudited |
| `observation_from_another_source_cannot_authorize_candidate` | `crates/kernel/tests/kernel_admission.rs:677` | degraded witness; source triple | unaudited |
| `a_config_observation_authorizes_only_a_config_event` | `crates/kernel/tests/kernel_admission.rs:1292` | source triple | unaudited |
| `an_observation_at_another_source_revision_cannot_authorize` | `crates/kernel/tests/kernel_admission.rs:1323` | degraded witness; source triple | unaudited |
| `an_invalidated_observation_cannot_authorize` | `crates/kernel/tests/kernel_admission.rs:1339` | degraded witness; source triple | unaudited |
| `a_trigger_whose_evidence_is_gone_cannot_admit` | `crates/kernel/tests/kernel_admission.rs:4211` | source triple | unaudited |
| `an_admitted_object_carries_the_class_of_its_trigger_and_the_evidence_behind_it` | `crates/kernel/tests/kernel_admission.rs:4150` | trigger sensitivity | unaudited |
| `an_object_admitted_by_a_trigger_serves_no_lower_than_that_trigger_reads_today` | `crates/kernel/tests/kernel_admission.rs:6378` | trigger sensitivity | unaudited |
| `newer_policy_revision_fails_closed_while_older_still_serves` | `crates/kernel/tests/kernel_admission.rs:3371` | policy digest | unaudited |
| `replacement_preserves_predecessor_history_and_leaves_successor_unadmitted` | `crates/kernel/tests/kernel_admission.rs:393` | replaced decision | unaudited |
| `same_envelope_subject_decision_after_admission_uses_the_recorded_prior` | `crates/kernel/tests/kernel_admission.rs:1047` | mark stale | unaudited |
| `dispositions_and_sensitivity_only_reduce_visibility` | `crates/kernel/tests/kernel_admission.rs:2735` | mark stale | unaudited |
| `inference_ceiling_requires_authority_and_lifts_only_to_event_target` | `crates/kernel/src/admission.rs:4942` | inference class | unaudited |
| `every_legal_source_taint_pair_enforces_its_automatic_ceiling` | `crates/kernel/src/admission.rs:4720` | inference class | unaudited |
| `an_event_that_names_a_disposition_keeps_its_own_outcome` | `crates/kernel/src/admission.rs:5437` | mark stale | unaudited |
| `policy_revision_matches_policy_content` | `crates/kernel/src/admission.rs:5931` | policy digest | unaudited |
| `the_served_fold_is_under_the_serving_digest` | `crates/kernel/src/admission.rs:5900` | policy digest | unaudited |
| `observe_code` (fixture) | `crates/daemon/src/lib.rs:14394` | positive witness | unaudited |
| `reads_at_a_snapshot_return_what_was_visible_then` | `crates/daemon/tests/kernel_routes.rs:1875` | positive witness; object-subject provenance | unaudited |
| `a_disposition_preview_reports_visibility_per_surface_and_writes_nothing` | `crates/daemon/tests/kernel_routes.rs:5120` | positive witness; object-subject provenance | unaudited |
| `a_plugin_route_cannot_declare_a_class_above_the_derived_one` | `crates/daemon/tests/kernel_routes.rs:1145` | plugin table | unaudited |
| `disposition_events_apply_the_fixed_table_and_report_outcome_and_disposition` | `crates/daemon/tests/kernel_routes.rs:4398` | mark stale | unaudited |
| `a_plugin_write_is_capped_at_inference_classes` | `crates/daemon/src/kernel_routes/commit.rs:1249` | plugin table | unaudited |
| `corrections_preserve_old_rows_and_reauthor_observation_dependencies` | `crates/kernel/tests/kernel_slice.rs:607` | successor identity | unaudited |
| `corrections_reject_cross_domain_replacements` | `crates/kernel/tests/kernel_slice.rs:1003` | successor identity | unaudited |
| `supersede_decision_replaces_a_live_predecessor_and_folds_merges_into_a_survivor` | `crates/kernel/tests/kernel_slice.rs:1096` | replaced decision | unaudited |
| `supersede_decision_refuses_a_quarantined_predecessor` | `crates/kernel/tests/kernel_slice.rs:1187` | barred lineage | unaudited |
| `a_decision_citing_evidence_is_classified_no_lower_than_that_evidence` | `crates/kernel/tests/kernel_slice.rs:1454` | document evidence fold | unaudited |
| `a_decision_serves_no_lower_than_its_cited_evidence_reads_today` | `crates/kernel/tests/kernel_slice.rs:1527` | document evidence fold | unaudited |
| `unsupported_symbol_checks_are_uncertain_and_failed_checks_go_stale` | `crates/kernel/tests/kernel_applicability_engine.rs:283` | cheap check | unaudited |
| `unreadable_payloads_are_uncertain_rather_than_current` | `crates/kernel/tests/kernel_applicability_engine.rs:626` | payload fails closed | unaudited |
| `unknown_check_kinds_degrade_without_voiding_the_payload` | `crates/kernel/tests/kernel_applicability_engine.rs:703` | payload fails closed | unaudited |
| `an_oversized_payload_is_uncertain_without_being_decoded` | `crates/kernel/tests/kernel_applicability_engine.rs:1832` | payload fails closed | unaudited |
| `unknown_payload_fields_are_undecodable_at_any_depth_but_an_unknown_kind_is_not` | `crates/kernel/tests/kernel_applicability_engine.rs:2703` | payload fails closed | unaudited |
| `failed_check_appends_observation_event_and_job_in_one_commit` | `crates/kernel/tests/kernel_read_repair.rs:141` | cheap check | unaudited |
| `sha256_hex_is_the_kernel_payload_id` | `crates/daemon/src/project_docs.rs:446` | document descriptor | unaudited |
| `every_request_in_a_batch_proves_its_buffer_against_the_artifact` | `crates/kernel/tests/kernel_source_descriptors.rs:1180` | document descriptor | unaudited |
| `repository_document_revisions_exceed_the_historical_head_including_retired_rows` | `crates/kernel/tests/kernel_source_descriptors.rs:2007` | document descriptor | unaudited |
| `a_configured_document_is_one_lineage_whose_edits_supersede_and_removal_retires` | `crates/daemon/src/source_capture_tests.rs:1992` | document descriptor; republication; evidence fold | unaudited |
| `a_missing_document_retires_only_the_captures_own_lineage` | `crates/daemon/src/source_capture_tests.rs:2172` | republication | unaudited |
| `a_renamed_document_retires_the_old_path_and_publishes_the_new` | `crates/daemon/src/source_capture_tests.rs:2237` | republication | unaudited |
| `unreadable_documents_are_counted_and_retire_nothing` | `crates/daemon/src/source_capture_tests.rs:2350` | republication | unaudited |
| `every_stale_lineage_of_a_path_retires_before_the_current_one_publishes` | `crates/daemon/src/source_capture_tests.rs:3009` | republication | unaudited |
| `direct_observation_rests_on_live_exact_evidence` | `crates/kernel/tests/kernel_claim_causality.rs:42` | direct observation | unaudited |
| `derived_reinjection_rests_on_exact_live_parents` | `crates/kernel/tests/kernel_claim_causality.rs:229` | derived reinjection | unaudited |
| `forged_records_and_copied_strings_grant_nothing` | `crates/kernel/tests/kernel_claim_causality.rs:364` | direct observation | unaudited |
| `detail_operation_and_parent_list_must_agree_with_the_registry` | `crates/kernel/tests/kernel_claim_causality.rs:710` | derived reinjection | unaudited |
| `refused_selections_publish_nothing_and_encodings_are_dispositions` | `crates/daemon/tests/git_sources.rs:549` | header gate; daemon opener | unaudited |
| `delta_packed_commit_against_an_oversized_base_is_refused` | `crates/daemon/tests/git_sources.rs:827` | header gate; daemon opener | unaudited |
| `the_fixture_has_no_worktree` | `crates/daemon/tests/attestation_verifier.rs:202` | verdict deterministic | unaudited |
| `committed_bytes_match_and_a_later_commit_mismatches` | `crates/daemon/tests/attestation_verifier.rs:212` | verdict deterministic; blob read bounded | unaudited |
| `a_nested_path_walks_one_tree_per_component` | `crates/daemon/tests/attestation_verifier.rs:253` | blob read bounded | unaudited |
| `line_endings_normalize_in_either_direction` | `crates/daemon/tests/attestation_verifier.rs:276` | verdict deterministic | unaudited |
| `entries_that_hold_no_blob_are_named_by_kind` | `crates/daemon/tests/attestation_verifier.rs:324` | blob read bounded | unaudited |
| `missing_or_corrupt_objects_are_unreadable_never_absent` | `crates/daemon/tests/attestation_verifier.rs:366` | blob read bounded | unaudited |
| `a_directory_that_is_no_repository_is_unopenable` | `crates/daemon/tests/attestation_verifier.rs:475` | blob read bounded | unaudited |
| `the_driver_writes_nothing` | `crates/daemon/tests/attestation_verifier.rs:485` | blob read bounded | unaudited |
| `replacement_refs_are_ignored` | `crates/daemon/tests/attestation_verifier.rs:505` | verdict deterministic; Kernel checkout open record (daemon-opener half) | unaudited |
| `the_header_gates_every_blob_read` | `crates/daemon/tests/attestation_verifier.rs:541` | blob read bounded | unaudited |
| `a_blob_whose_delta_base_exceeds_the_allocation_limit_is_over_budget` | `crates/daemon/tests/attestation_verifier.rs:594` | blob read bounded; Kernel checkout open record (daemon-opener half) | unaudited |
| `a_passed_deadline_opens_no_repository` | `crates/daemon/tests/attestation_verifier.rs:678` | blob read bounded | unaudited |
| `a_deadline_before_the_header_poll_reads_no_blob` | `crates/daemon/tests/attestation_verifier.rs:704` | blob read bounded | unaudited |
| `verdicts_survive_a_worktree_edit_a_head_move_and_a_repack` | `crates/daemon/tests/attestation_verifier.rs:724` | verdict deterministic | unaudited |
| `claim_paths_follow_the_document_path_rule` | `crates/daemon/tests/attestation_verifier.rs:782` | blob read bounded | unaudited |
| `the_pure_verdict_compares_digests_after_the_header_gate` | `crates/daemon/tests/attestation_verifier.rs:805` | verdict deterministic | unaudited |
| `normalized_digests_exist_only_for_a_reachable_length` | `crates/daemon/tests/attestation_verifier.rs:879` | verdict deterministic | unaudited |
| `reasons_have_stable_names_and_a_terminal_split` | `crates/daemon/tests/attestation_verifier.rs:904` | reason names for #1173 counters | unaudited |
| `a_delta_chain_that_revisits_an_entry_is_unreadable` | `crates/daemon/tests/attestation_verifier.rs:651` | blob read bounded | unaudited |
| `a_raw_digest_match_needs_the_claimed_length` | `crates/daemon/tests/attestation_verifier.rs:951` | verdict deterministic | unaudited |
| `an_empty_committed_file_matches_an_empty_claim` | `crates/daemon/tests/attestation_verifier.rs:970` | verdict deterministic | unaudited |
| `an_unsupported_object_format_is_unopenable` | `crates/daemon/tests/attestation_verifier.rs:993` | blob read bounded | unaudited |
| `a_blob_mode_entry_naming_another_kind_is_unreadable` | `crates/daemon/tests/attestation_verifier.rs:1012` | blob read bounded | unaudited |
| `each_deadline_poll_precedes_its_read` | `crates/daemon/tests/attestation_verifier.rs:1037` | blob read bounded | unaudited |
| `a_non_bare_root_answers_from_the_object_store_not_the_worktree` | `crates/daemon/tests/attestation_verifier.rs:1071` | verdict deterministic | unaudited |
| `evidence_cited_in_refuses_a_deleted_evidence_row_that_live_decisions_still_cite` | `crates/kernel/tests/kernel_slice.rs:2248` | artifact deletion | unaudited |
| `the_evidence_keyed_read_returns_the_live_document_descriptor_citing_the_evidence` | `crates/kernel/tests/kernel_source_descriptors.rs:3252` | document descriptor; verifier input | unaudited |
| `the_anchor_read_returns_the_decoded_condition_and_refuses_an_undecodable_row` | `crates/kernel/tests/kernel_anchor_write.rs:869` | verifier input | unaudited |
| `each_unverifiable_decision_classifies_under_exactly_one_reason_and_nothing_is_written` | `crates/daemon/tests/attestation_verifier_classify.rs:355` | verifier input (AC16); citation order | unaudited |
| `a_descriptor_without_a_digest_is_digest_missing` | `crates/daemon/tests/attestation_verifier_classify.rs:535` | verifier input | unaudited |
| `a_verifier_decision_is_self_source_before_any_anchor_or_citation_read` | `crates/daemon/tests/attestation_verifier_classify.rs:553` | party, `self_source` half | unaudited |
| `a_stored_path_outside_the_document_path_rule_is_refused_for_that_decision` | `crates/daemon/tests/attestation_verifier_classify.rs:627` | verifier input (invalid stored path) | unaudited |
| `a_target_of_another_incarnation_is_refused` | `crates/daemon/tests/attestation_verifier_classify.rs:644` | snapshot purity (incarnation) | unaudited |
| `the_row_level_citation_is_read_before_the_capped_event_citations` | `crates/daemon/tests/attestation_verifier_classify.rs:665` | verifier input (citation cap) | unaudited |
| `a_decision_outside_the_input_set_counts_under_no_reason` | `crates/daemon/tests/attestation_verifier_classify.rs:698` | verifier input; snapshot purity | unaudited |
| `a_decision_citing_a_message_and_a_document_is_verifiable_through_the_document` | `crates/daemon/tests/attestation_verifier_classify.rs:749` | verifier input | unaudited |
| `classification_at_one_sequence_ignores_the_worktree_head_and_later_commits` | `crates/daemon/tests/attestation_verifier_classify.rs:817` | snapshot purity; F3 at the classification step | unaudited |
| `the_repository_binds_to_the_first_matching_root_in_bound_order` | `crates/daemon/tests/attestation_verifier_classify.rs:920` | verifier input (binding) | unaudited |
| `the_document_kind_path_rule_is_a_closed_table` | `crates/daemon/tests/attestation_verifier_classify.rs:959` | document kind for #1172's classes (F15) | unaudited |

## Categories with no check found

- `p6-plugin-cannot-replace-a-foreign-source-decision`: no test submits a
  plugin replacement of a decision of another source kind.
- `p6-artifact-deletion-invalidates-cited-evidence-without-the-citation-guard`:
  no test follows the deletion of cited evidence with a witness admission.
- The Kernel checkout opener: no test reads an over-budget delta base or a
  replacement ref through it.
- The trigger fold of `Sensitive` document evidence: no test promotes through
  a trigger whose observation cites document evidence.
- Every proposed `p6-new-*` record: its code does not exist at the branch tip,
  except the `self_source` half of `p6-new-party-distinct-and-self-refusal`.

## Quiet areas

- A document descriptor superseded by a revision citing other evidence: the
  classifier reads the cited revision, no longer live, as
  `descriptor_retired` even while a newer revision of its lineage is live;
  the Kernel test covers a retired descriptor and no test supersedes one
  under a citing decision. Whether the reason should cover this case is an
  open question for the owner.
- A commit or tree object above 256 KiB, or one on an oversized delta base:
  the verifier maps its allocation refusal to `over_budget`, and no test
  constructs one.
- A delta chain that revisits an entry through an alternate object directory:
  the verifier's walk covers the repository's own packs only.
