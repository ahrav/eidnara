# Attestation verifier: existing checks

Every check below has status `unaudited`. A listed check is evidence that a
claim is exercised, not a verdict that it is adequate; adequacy belongs to
`/testing:invariant-test-review` for tests and
`/low-level-systems:defensive-assertions-and-invariant-guards` for
production guards. References were verified at the #1170 branch, and the
rows #1171 added and every row pointing into a file #1171 changed were
verified at the #1171 branch, the rows #1172 added and every row pointing
into a file #1172 changed at the #1172 branch, and the rows #1173 added and
every row pointing into a file #1173 changed at the #1173 branch.

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
| Header pass before decode | `crates/daemon/src/git_sources.rs:603` and `:617` in `read_selection` (`:551`); `exceeds_allocation` at `:515` | headers judged before any decode; allocation refusals named | unaudited |
| Kernel checkout open | `crates/kernel/src/applicability/checkout.rs:639` (`open_isolated`) | isolated, command configuration stripped; no allocation limit; replacements honored | unaudited |
| Document caps | `crates/daemon/src/project_docs.rs:23` (`MAX_PROJECT_DOC_BYTES`); `crates/daemon/src/harness_sources.rs:642` (`MAX_SOURCE_UNIT_BYTES`); unit cap applied at `crates/daemon/src/source_capture.rs:1297` | 256 KiB read, 64 KiB published | unaudited |
| Document publisher class | `crates/daemon/src/source_capture.rs:922` | documents publish at `Sensitive` | unaudited |
| Causality parents | `crates/kernel/src/claim_causality.rs:218` (`record_claim_causality`), `:38` (`MAX_DERIVATION_PARENTS`) | live exact parents; bounded parent list | unaudited |
| Verifier header gate | `crates/daemon/src/attestation_verifier.rs:313` (`header_verdict`), used at `:538` (`lookup`) and `:329` (`verdict`) | header over 256 KiB is `over_budget`; a claimed length above twice or below half the header size is `Mismatch`; both before any content read | unaudited |
| Verifier object read | `crates/daemon/src/attestation_verifier.rs:485` (`Objects::read`), `:468` (`Objects::blob_header`) | own packs, then the loose store, then the object store for an id no own index holds, each under the allocation limit; a loose read error is `object_unreadable`; kind and hash checked after decode, decoded length recorded first; allocation refusal is `over_budget`; slot exhaustion makes at most one reopen attempt per claim | unaudited |
| Verifier delta-chain walk | `crates/daemon/src/git_sources.rs:314` (`OwnPacks::header`), `:329` (`OwnPacks::decode`), `:445` (`entry_header`) | an own-pack entry whose ofs- and ref-delta walk revisits an entry is refused before any decode; an id whose every holding own pack revisits an entry, names a base outside that pack, or has an undecodable header, or whose located entry fails the decode, reads a valid loose copy or is `object_unreadable(class)`; the object store never reads an id an own index holds | unaudited |
| Verifier deadline polls | `crates/daemon/src/attestation_verifier.rs:393` (`poll`), `:352` (`Deadline`) | polled before the open and before every object read | unaudited |
| Verifier claim path | `crates/daemon/src/attestation_verifier.rs:71` (`Claim::new`) via `crates/daemon/src/confined_path.rs:10` (`MAX_COMPONENTS`) | at most 16 ordinary components | unaudited |
| Evidence-keyed descriptor read | `crates/kernel/src/source_descriptor.rs:1213` (`source_descriptor_citing_evidence`) through `checked` (`:795`) | the live descriptor of a class citing an evidence id at a sequence, first in object id order, with the page read's row checks; `Retired` and `Uncited` otherwise | unaudited |
| Anchor-condition read | `crates/kernel/src/anchor_write.rs:469` (`anchor_condition_as_of`) through `validate_anchor_row` (`:288`) | the anchor live at a sequence decodes as the engine decodes it; an undecodable row is `CorruptCanonicalRow` | unaudited |
| Classifier self-source check | `crates/daemon/src/attestation_verifier/classify.rs:340` in `class_of` (`:314`) | after the `Active` filter (`:334`), both on the standing read, a live `Active` `verifier` decision is `self_source` before any anchor or citation read | unaudited |
| Classifier reason order | `crates/daemon/src/attestation_verifier/classify.rs:314` (`class_of`) | one class per decision: `NotInput` for a decision not live or not `Active`, then `self_source`, then the anchor reasons, then the citation reasons in citation order | unaudited |
| Classifier citation order and cap | `crates/daemon/src/attestation_verifier/classify.rs:401` (`citations`), `MAX_EVENT_CITATIONS` at `:53` | the row-level `evidence_id` first, then at most 32 event-level citations of other evidence from the newest `LINKED_EVENT_WINDOW` × 33 decision events | unaudited |
| Classifier incarnation recheck | `crates/daemon/src/attestation_verifier/classify.rs:306` in `classify` (`:295-296`) | after the last read, successful or refused, a store that is not the target's incarnation is `IncarnationMismatch` | unaudited |
| Classifier claim and binding | `crates/daemon/src/attestation_verifier/classify.rs:492` (`VerifierInput::new`), path refusal at `:508`, root match at `:509` | the claim's path and digest from the descriptor, its length from the evidence row, its oid from the anchor; a stored path outside the document path rule is `InvalidDocumentPath` when no cited row resolves to a verifiable document; the first bound root with the repository identity, else `repository_unbound` | unaudited |
| Document kind rule | `crates/daemon/src/attestation_verifier/classify.rs:246` (`DocumentKind::of`), `:530` (`DocumentKind::event_kind`) | configuration extensions are `CurrentConfig` with `config_present` and `ConfigObserved`; every other path is `CurrentCode` with `code_present` and `CodeObserved` | unaudited |
| Attestation identity | `crates/daemon/src/attestation_verifier/attest.rs:65` (`Identity::of`), `:610` (`intent`) | every id and the `source_id` hash `(model object id, oid)`; the key names the verdict's write, the decision, and the oid | unaudited |
| Write-set check | `crates/daemon/src/attestation_verifier/attest.rs:289` (`WriteSet::check`), called at `:343` | the compared, detail, and citation evidence ids are one id; the detail is at `WITNESS_DETAIL_VERSION` and names the write set's blob and the claim's commit oid, digest, path, descriptor, lineage, and descriptor revision; the citation is a `verifier_citation` event; every id is the pair's `Identity`; both rows carry the model decision's domain and served sensitivity and one scope and the attestation its anchor; the witness carries the attestation's source triple at revision 1 and the document kind's witness kind; no row cites evidence by FK | unaudited |
| Envelope recheck | `crates/daemon/src/attestation_verifier/attest.rs:543` (`recheck`): incarnation at `:552`, anchor at `:580`, descriptor at `:589`; served sensitivity at `:366` | the model decision, its anchor at the sequence the envelope extends read in the envelope's transaction, its descriptor with its cited evidence row live, and the class the Kernel serves it at are what the classification read; otherwise `changed` | unaudited |
| Witness oracle | `crates/daemon/src/attestation_verifier/attest.rs:389` | the envelope commits only at effective maturity `Verified` and disposition `Active`; otherwise `witness_degraded` | unaudited |
| Opposite-key refusal | `crates/daemon/src/attestation_verifier/attest.rs:506` in `run` (`:494`) | a recorded key for the other verdict on the same decision and oid is `conflict` | unaudited |
| Stale path | `crates/daemon/src/attestation_verifier/attest.rs:428` (`mark_stale`), skip at `:454`, admission at `:459` | one `MarkStale` under the latest admission's classes; an already `Stale` decision is skipped; a `MarkStale` returning another disposition is `invalid_write_set` | unaudited |
| Verifier flag | `crates/daemon/src/config.rs:162` (default), `ConfigKey::VerifierEnabled` user-only; `crates/daemon/src/attestation_verifier/bounded_pass.rs:310` in `Verifier::pass` (`:287`) | the flag is off by default, a project tier cannot set it, and the pass reads it per project before any repository or Kernel read | unaudited |
| Pass budgets | `crates/daemon/src/attestation_verifier/bounded_pass.rs:525` in `ProjectRun::run` (`:433`); `MAX_VERDICTS` at `:77` | a project stops at N verdicts or N × 256 KiB blob bytes per pass | unaudited |
| Pass cursor | `crates/daemon/src/attestation_verifier/bounded_pass.rs:517` in `ProjectRun::run` (`:433`) | the cursor moves only over a resolved prefix; a resolved decision behind an unresolved one is remembered; a project whose pages ran out with every decision resolved pages from past the sequence it paged through (`:463`, `:446`) | unaudited |
| Pass skips | `crates/daemon/src/attestation_verifier/bounded_pass.rs:501` (`skipped_stale`), `:566` (`skipped_attested`) in `ProjectRun::attested` (`:559`), which `ProjectRun::decision` (`:574`) calls at the oid of the anchor the page's standing read names before classification (`:578`) and at the classified oid after | a `Stale` decision and one whose attestation id is registered are skipped before any claim verification read or Kernel write; an attested decision is skipped before its classification reads | unaudited |
| Pass retry bound | `crates/daemon/src/attestation_verifier/bounded_pass.rs:535` (`retry`), `:550` (`transient`), `MAX_ATTEMPTS` at `:79` | a decision resolves on its K-th attempt in total, shared across transient verdicts, deadlines, classification and commit refusals, `changed` outcomes, and degraded witnesses | unaudited |
| Pass effect counters | `crates/daemon/src/attestation_verifier/bounded_pass.rs:654` (`ProjectRun::outcome`), after `attest` at `:649` | each outcome increments its effect counter after the commit returns | unaudited |
| Scope decision page | `crates/kernel/src/slice/read.rs:712` (`scope_decisions_after_within_budget`) | a scope's live decisions after a `(created_commit_seq, object_id)` pair in that order, excluding one source kind, at most a row cap, read by seeking the commit-sequence index `idx_decisions_known_as_of` at the pair | unaudited |
| Verifier block sanitizer | `crates/host-runtime/src/control.rs:2112` (`VERIFIER_COUNTERS`) through `sanitize_counters` | `metrics.verifier` keeps each allowlisted unsigned integer no greater than 2^53 and drops every other field | unaudited |

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
| `observe_code` (fixture) | `crates/daemon/src/lib.rs:14407` | positive witness | unaudited |
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
| `a_ref_delta_in_an_own_pack_decodes_against_its_base_in_that_pack` | `crates/daemon/tests/attestation_verifier.rs:1115` | blob read bounded | unaudited |
| `objects_in_an_alternate_with_more_packs_than_index_slots_match` | `crates/daemon/tests/attestation_verifier.rs:1146` | blob read bounded; Kernel checkout open record (daemon-opener half) | unaudited |
| `a_delta_with_an_empty_body_that_names_itself_is_unreadable` | `crates/daemon/tests/attestation_verifier.rs:1197` | blob read bounded | unaudited |
| `an_own_pack_blob_whose_stream_does_not_decode_is_unreadable` | `crates/daemon/tests/attestation_verifier.rs:1224` | blob read bounded | unaudited |
| `a_tree_above_the_allocation_limit_is_over_budget_loose_and_packed` | `crates/daemon/tests/attestation_verifier.rs:1240` | blob read bounded; Kernel checkout open record (daemon-opener half) | unaudited |
| `a_blob_only_in_an_alternate_with_more_packs_than_index_slots_matches` | `crates/daemon/tests/attestation_verifier.rs:1269` | blob read bounded | unaudited |
| `an_own_pack_delta_whose_base_lies_in_another_pack_is_unreadable` | `crates/daemon/tests/attestation_verifier.rs:1307` | blob read bounded | unaudited |
| `a_valid_loose_copy_answers_for_an_own_pack_entry_that_does_not_resolve` | `crates/daemon/tests/attestation_verifier.rs:1358` | blob read bounded | unaudited |
| `a_valid_own_pack_copy_answers_after_a_newer_pack_whose_chain_revisits_an_entry` | `crates/daemon/tests/attestation_verifier.rs:1378` | blob read bounded | unaudited |
| `evidence_cited_in_refuses_a_deleted_evidence_row_that_live_decisions_still_cite` | `crates/kernel/tests/kernel_slice.rs:2248` | artifact deletion | unaudited |
| `the_evidence_keyed_read_returns_the_live_document_descriptor_citing_the_evidence` | `crates/kernel/tests/kernel_source_descriptors.rs:3252` | document descriptor; verifier input | unaudited |
| `the_anchor_read_returns_the_decoded_condition_and_refuses_an_undecodable_row` | `crates/kernel/tests/kernel_anchor_write.rs:869` | verifier input | unaudited |
| `each_unverifiable_decision_classifies_under_exactly_one_reason_and_nothing_is_written` | `crates/daemon/tests/attestation_verifier_classify.rs:355` | verifier input (AC16); citation order | unaudited |
| `a_descriptor_without_a_digest_is_digest_missing` | `crates/daemon/tests/attestation_verifier_classify.rs:572` | verifier input | unaudited |
| `a_verifier_decision_is_self_source_before_any_anchor_or_citation_read` | `crates/daemon/tests/attestation_verifier_classify.rs:590` | party, `self_source` half | unaudited |
| `a_stored_path_outside_the_document_path_rule_is_refused_for_that_decision` | `crates/daemon/tests/attestation_verifier_classify.rs:667` | verifier input (invalid stored path) | unaudited |
| `a_target_of_another_incarnation_is_refused` | `crates/daemon/tests/attestation_verifier_classify.rs:713` | snapshot purity (incarnation) | unaudited |
| `the_row_level_citation_is_read_before_the_capped_event_citations` | `crates/daemon/tests/attestation_verifier_classify.rs:747` | verifier input (citation cap) | unaudited |
| `a_decision_outside_the_input_set_counts_under_no_reason` | `crates/daemon/tests/attestation_verifier_classify.rs:805` | verifier input; snapshot purity | unaudited |
| `a_decision_citing_a_message_and_a_document_is_verifiable_through_the_document` | `crates/daemon/tests/attestation_verifier_classify.rs:872` | verifier input | unaudited |
| `classification_at_one_sequence_ignores_the_worktree_head_and_later_commits` | `crates/daemon/tests/attestation_verifier_classify.rs:950` | snapshot purity; F3 at the classification step | unaudited |
| `the_repository_binds_to_the_first_matching_root_in_bound_order` | `crates/daemon/tests/attestation_verifier_classify.rs:1080` | verifier input (binding) | unaudited |
| `the_document_kind_path_rule_is_a_closed_table` | `crates/daemon/tests/attestation_verifier_classify.rs:1119` | document kind for #1172's classes (F15) | unaudited |
| `the_evaluation_names_the_blob_the_path_resolved_to` | `crates/daemon/tests/attestation_verifier.rs:1418` | the blob id the write set records | unaudited |
| `a_plugin_route_cannot_write_as_the_verifier_or_assert_trusted_local_code` | `crates/daemon/tests/kernel_routes.rs:6778` | plugin table; party (AC14) | unaudited |
| `a_match_commits_the_attestation_with_every_companion_row` | `crates/daemon/tests/attestation_verifier_attest.rs:431` | classes and cited fields; sibling; verified via witness; degradation oracle; party (AC15); validated witness; derived reinjection (AC6, AC7, F14) | unaudited |
| `the_attestation_serves_at_the_model_decisions_sensitivity` | `crates/daemon/tests/attestation_verifier_attest.rs:572` | classes and cited fields (F14); FK fold | unaudited |
| `a_mismatch_marks_the_model_decision_stale_once` | `crates/daemon/tests/attestation_verifier_attest.rs:597` | stale path (AC8); mark stale | unaudited |
| `a_mismatch_on_a_decision_already_stale_is_skipped` | `crates/daemon/tests/attestation_verifier_attest.rs:660` | stale path | unaudited |
| `an_unavailable_verdict_writes_nothing` | `crates/daemon/tests/attestation_verifier_attest.rs:685` | stale path (`Unavailable`, with and without a blob id); witness evidence equality (no blob id) | unaudited |
| `a_write_set_built_before_the_model_decision_existed_is_changed` | `crates/daemon/tests/attestation_verifier_attest.rs:727` | one outcome (`changed`) | unaudited |
| `a_write_set_citing_other_evidence_or_another_revision_is_refused_before_the_commit` | `crates/daemon/tests/attestation_verifier_attest.rs:749` | witness evidence equality (AC9) | unaudited |
| `a_witness_admitted_below_verified_aborts_the_envelope` | `crates/daemon/tests/attestation_verifier_attest.rs:854` | degradation (AC9); degraded witness | unaudited |
| `a_replay_returns_the_receipt_and_a_different_verdict_conflicts` | `crates/daemon/tests/attestation_verifier_attest.rs:895` | identity (F10) | unaudited |
| `a_match_after_the_stale_path_for_the_same_oid_conflicts` | `crates/daemon/tests/attestation_verifier_attest.rs:938` | identity (F10) | unaudited |
| `a_model_decision_changed_after_classification_aborts_the_set` | `crates/daemon/tests/attestation_verifier_attest.rs:965` | sibling (F2, F3, deleted evidence) | unaudited |
| `an_unrelated_attestation_does_not_abort_an_older_classification` | `crates/daemon/tests/attestation_verifier_attest.rs:1067` | sibling (an unrelated attestation committed in the window) | unaudited |
| `a_match_on_a_configuration_document_commits_under_current_config` | `crates/daemon/tests/attestation_verifier_attest.rs:1092` | classes and cited fields (`CurrentConfig`) | unaudited |
| `one_attestations_witness_cannot_promote_another` | `crates/daemon/tests/attestation_verifier_attest.rs:1132` | identity; verified via witness; source triple (F1) | unaudited |
| `a_rollback_after_the_admission_leaves_no_row_and_a_reopened_store_keeps_every_row` | `crates/daemon/tests/attestation_verifier_attest.rs:1178` | one outcome (F11) | unaudited |
| `no_model_client_is_reachable_from_the_verifier_module` | `crates/daemon/tests/attestation_verifier_attest.rs:1246` | party (AC15) | unaudited |
| `the_attestation_serves_at_the_model_decisions_folded_class` | `crates/daemon/tests/attestation_verifier_attest.rs:1400` | classes and cited fields (served sensitivity over a `Personal` taint floor, F14) | unaudited |
| `a_served_class_other_than_the_kernels_is_changed` | `crates/daemon/tests/attestation_verifier_attest.rs:1435` | sibling (served-sensitivity recheck) | unaudited |
| `a_write_set_naming_another_document_or_revision_is_refused_before_the_commit` | `crates/daemon/tests/attestation_verifier_attest.rs:1470` | witness evidence equality (AC9: path, descriptor, lineage, revision, scope, citation kind, source revision, source kind) | unaudited |
| `a_classification_from_another_incarnation_or_with_other_classes_is_changed` | `crates/daemon/tests/attestation_verifier_attest.rs:1543` | sibling (incarnation and class recheck, both verdicts) | unaudited |
| `a_mismatch_for_a_decision_re_disposed_after_classification_is_changed` | `crates/daemon/tests/attestation_verifier_attest.rs:1586` | stale path (F2, other disposition) | unaudited |
| `a_descriptor_republished_after_classification_is_changed` | `crates/daemon/tests/attestation_verifier_attest.rs:1614` | sibling (F3, a newer live descriptor revision, both verdicts) | unaudited |
| `a_match_after_a_stale_path_under_another_blob_conflicts` | `crates/daemon/tests/attestation_verifier_attest.rs:1680` | identity (F10, opposite key under another digest) | unaudited |
| `the_identity_depends_on_the_model_and_the_oid` | `crates/daemon/tests/attestation_verifier_attest.rs:1708` | identity (known answer, oid dependence) | unaudited |
| `a_flag_off_pass_runs_nothing_and_a_reload_to_false_stops_the_next_pass` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:431` | flag off (AC10, AC13 reload, F12) | unaudited |
| `each_pass_verifies_at_most_n_per_project_and_no_project_starves` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:475` | pass bounds (AC11, F13); blob read bounded (per pass) | unaudited |
| `a_deadline_between_decisions_leaves_exactly_k_verdicts_and_the_cursor_before_the_next` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:542` | cursor (AC4, F7) | unaudited |
| `a_rolled_back_commit_counts_its_refusal_and_a_replayed_commit_counts_its_replay` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:590` | pass accounting (AC12, F10, F11) | unaudited |
| `a_concurrent_commit_completes_while_a_verifier_read_is_stalled` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:681` | writer never held across a read (AC13) | unaudited |
| `a_transient_head_resolves_after_k_passes_and_a_terminal_reason_is_read_once` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:734` | progress under transient; cursor (F13) | unaudited |
| `a_restart_rescan_writes_nothing_and_counts_skips` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:774` | cursor (restart) | unaudited |
| `the_task_steps_only_under_the_flag_and_stops_on_cancellation` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:814` | flag off (task) | unaudited |
| `the_health_block_names_exactly_the_documented_counters` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:864` | counters block and wire table | unaudited |
| `a_witness_degraded_k_passes_is_skipped_and_the_pass_continues` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:897` | progress under transient; degradation (OQ6-11) | unaudited |
| `a_head_decision_that_meets_the_deadline_k_passes_is_skipped_and_read_no_more` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:961` | progress under transient (deadline at the commit and during a read); cursor | unaudited |
| `a_deadline_at_the_commit_leaves_the_decision_unwritten_and_the_next_pass_commits` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:990` | cursor (F7 at the commit poll); pass accounting | unaudited |
| `unverifiable_and_attested_decisions_count_outside_n` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1019` | pass accounting (unverifiable and skip buckets outside N) | unaudited |
| `a_worktree_root_verifies_under_its_git_identity_and_a_nested_root_binds_none` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1063` | bound roots (worktree `git:` identity, nested root) | unaudited |
| `a_failed_derivation_leaves_the_root_unbound_and_its_decision_is_skipped_after_k_passes` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1106` | progress under transient (an unreadable root binds no repository; its decision resolves after K passes) | unaudited |
| `a_broken_root_leaves_the_projects_healthy_root_bound` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1140` | progress under transient; bound roots (a broken root beside a healthy one) | unaudited |
| `a_deadline_at_the_derivation_passes_over_the_project_and_the_next_pass_verifies` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1171` | progress under transient (a deadline-cut derivation spends no attempt and caches nothing) | unaudited |
| `a_pass_budget_exhausted_during_the_derivation_passes_over_the_project` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1196` | progress under transient (a walk cut by the exhausted pass budget passes over the project and caches nothing) | unaudited |
| `a_classification_deadline_k_passes_is_skipped_and_the_cursor_passes_it` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1236` | progress under transient (a classification deadline spends an attempt; the K-th counts `unavailable_skipped`) | unaudited |
| `a_decision_resolves_only_against_its_own_projects_roots` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1269` | bound roots (per-project resolution) | unaudited |
| `a_terminal_reason_survives_a_project_unbound_and_bound_again` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1294` | cursor (terminal reason across pruning) | unaudited |
| `a_flag_off_project_beside_an_enabled_one_is_left_untouched` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1325` | flag off (per project, no derivation) | unaudited |
| `a_transient_head_ahead_of_a_page_boundary_counts_each_decision_once` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1353` | cursor (page boundary) | unaudited |
| `a_terminal_reason_is_not_read_again_after_an_incarnation_change` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1390` | cursor (terminal reason across an incarnation change) | unaudited |
| `the_task_paces_passes_by_their_progress` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1414` | task intervals | unaudited |
| `cancelling_an_in_flight_pass_returns_before_its_deadline` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1451` | task cancellation | unaudited |
| `a_drained_project_verifies_the_decisions_committed_after_its_last_page` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1586` | cursor (paging from past a drained project's sequence; a transient decision committed after it) | unaudited |
| `an_attested_decision_skips_before_its_classification_after_its_descriptor_retires` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1642` | pass skips (attested skip before classification) | unaudited |
| `every_reason_and_unverifiable_class_names_a_counter` | `crates/daemon/src/attestation_verifier/bounded_pass.rs:699` | counters block (every bucket name) | unaudited |
| `bound_projects_follow_bound_root_order` | `crates/daemon/src/verifier_task.rs:164` | project order | unaudited |
| `the_pass_reader_follows_the_user_tier_and_drops_a_project_tier_value` | `crates/daemon/src/config.rs:3959` | flag off (reader, reload, project tier) | unaudited |
| `the_page_walks_a_scopes_live_decisions_in_commit_order_after_the_cursor` | `crates/kernel/tests/kernel_scope_decisions.rs:65` | cursor (page order) | unaudited |
| `the_scope_decision_page_seeks_the_commit_sequence_index` | `crates/kernel/src/slice/read.rs:756` | cursor (page plan) | unaudited |
| `every_fixture_row_matches_through_the_configuration_cache` | `crates/daemon/src/config.rs:3919` | flag off (project tier, Rust) | unaudited |
| `verifier parity fixture` | `packages/opencode-plugin/src/config/index.test.ts:1101`, `packages/pi-plugin/src/config/index.test.ts:754` | flag off (project tier, both plugins) | unaudited |
| `the_verifier_block_is_sanitized_field_by_field_and_documented` | `crates/host-runtime/src/control.rs:2176` | counters block on the wire | unaudited |

## Categories with no check found

- `p6-plugin-cannot-replace-a-foreign-source-decision`: no test submits a
  plugin replacement of a decision of another source kind.
- `p6-artifact-deletion-invalidates-cited-evidence-without-the-citation-guard`:
  no test follows the deletion of cited evidence with a witness admission.
- The Kernel checkout opener: no test reads an over-budget delta base or a
  replacement ref through it.
- The trigger fold of `Sensitive` document evidence: no test promotes through
  a trigger whose observation cites document evidence.
- `p6-new-verifiability-pure-function-of-snapshot`: no test restores the store
  after the first read of a classification, so the incarnation recheck after the
  last read has no behavioral test; `classify` has no seam between its reads.
- `p6-new-attestation-stale-on-byte-change`, the one proposed record: its code
  does not exist at the branch tip.
- The write set: no test forces a non-`Stale` result from the `MarkStale`
  admission, a scope mismatch in the envelope, or an exhausted budget in
  `attest`, and no test terminates the process after a commit.

## Quiet areas

- A document descriptor superseded by a revision citing other evidence: the
  classifier reads the cited revision, no longer live, as
  `descriptor_retired` even while a newer revision of its lineage is live;
  the Kernel test covers a retired descriptor and no test supersedes one
  under a citing decision. Whether the reason should cover this case is an
  open question for the owner.
- A commit above 256 KiB, or a commit or tree on an oversized delta base: the
  verifier maps its allocation refusal to `over_budget`, and no test
  constructs one.
- A delta chain that revisits an entry in an alternate object directory's pack
  or in an own pack whose index or pack is not a regular file: the object store
  reads it outside the verifier's walk, and gix bounds only cross-pack
  recursion.
