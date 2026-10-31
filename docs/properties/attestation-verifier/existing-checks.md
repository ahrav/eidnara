# Attestation verifier: existing checks

Every check below has status `unaudited`. A listed check is evidence that a
claim is exercised, not a verdict that it is adequate; adequacy belongs to
`/testing:invariant-test-review` for tests and
`/low-level-systems:defensive-assertions-and-invariant-guards` for
production guards. References were verified at the #1170 branch, and the
rows #1171 added and every row pointing into a file #1171 changed were
verified at the #1171 branch, the rows #1172 added and every row pointing
into a file #1172 changed at the #1172 branch, the rows #1173 added and
every row pointing into a file #1173 changed at the #1173 branch, and the
rows #1174 added and every row pointing into a file #1174 changed at the
#1174 branch.

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
| Evidence-keyed descriptor read | `crates/kernel/src/source_descriptor.rs:1230` (`source_descriptor_citing_evidence`) running the statement of `SnapshotRead::source_descriptors_citing_evidence` (`:1300`) through `checked` (`:795`) | the live descriptor of a class citing an evidence id at a sequence, first in object id order, with the page read's row checks; `Retired` and `Uncited` otherwise; one statement answers several ids in request order, and a live row that fails the checks refuses only its own entry | unaudited |
| Anchor-condition read | `crates/kernel/src/anchor_write.rs:469` (`anchor_condition_as_of`) running `SnapshotRead::anchor_condition` (`:488`) through `validate_anchor_row` (`:288`) | the anchor live at a sequence decodes as the engine decodes it; an undecodable row is `CorruptCanonicalRow` | unaudited |
| Snapshot read | `crates/kernel/src/snapshot_read.rs:34` (`read_at`), incarnation guard at `:44`; `SnapshotRead::decision_standing` at `crates/kernel/src/claim_facts.rs:1005` | every read of one call in one reader transaction at the target's sequence; tip, budget, and incarnation refused before the first read; the standing read's registry, decision, and own-admission rows with its row checks, a supporting approval's validity from the authority-chain check as a second statement, and the row-level evidence's liveness at the sequence | unaudited |
| Classifier self-source check | `crates/daemon/src/attestation_verifier/classify.rs:337` in `class_of` (`:316`) | after the `Active` filter (`:331`), both on the standing read, a live `Active` `verifier` decision is `self_source` before any anchor or citation read | unaudited |
| Classifier reason order | `crates/daemon/src/attestation_verifier/classify.rs:316` (`class_of`) | one class per decision: `NotInput` for a decision not live or not `Active`, then `self_source`, then the anchor reasons, then the citation reasons in citation order | unaudited |
| Classifier citation order and cap | `crates/daemon/src/attestation_verifier/classify.rs:411` (`citations`), `MAX_EVENT_CITATIONS` at `:54` | the row-level `evidence_id` first, then at most 32 event-level citations of other evidence from the newest `LINKED_EVENT_WINDOW` × 33 decision events | unaudited |
| Classifier incarnation recheck | `crates/daemon/src/attestation_verifier/classify.rs:309` in `classify` (`:295`) | after the `read_at` transaction, successful or refused, a store that is not the target's incarnation is `IncarnationMismatch` | unaudited |
| Classifier claim and binding | `crates/daemon/src/attestation_verifier/classify.rs:467` (`VerifierInput::new`), path refusal at `:483`, root match at `:486` | the claim's path and digest from the descriptor, its length from the evidence row, its oid from the anchor; a stored path outside the document path rule is `InvalidDocumentPath` when no cited row resolves to a verifiable document; the first bound root with the repository identity, else `repository_unbound` | unaudited |
| Document kind rule | `crates/daemon/src/attestation_verifier/classify.rs:247` (`DocumentKind::of`), `:505` (`DocumentKind::event_kind`) | configuration extensions are `CurrentConfig` with `config_present` and `ConfigObserved`; every other path is `CurrentCode` with `code_present` and `CodeObserved` | unaudited |
| Attestation identity | `crates/daemon/src/attestation_verifier/attest.rs:66` (`Identity::of`), `:732` (`intent`), `:776` (`ID_PREFIXES`), refused on the plugin route at `crates/daemon/src/kernel_routes/commit.rs:1403` | every id and the `source_id` hash `(model object id, oid)`; the key names the verdict's write, the decision, and the oid | unaudited |
| Write-set check | `crates/daemon/src/attestation_verifier/attest.rs:343` (`WriteSet::check`), called in the `attest_match` envelope at `:180` and in `WriteSet::commit` at `:399` | the compared, detail, and citation evidence ids are one id; the detail is at `WITNESS_DETAIL_VERSION` and names the write set's blob and the claim's commit oid, digest, path, descriptor, lineage, and descriptor revision; the citation is a `verifier_citation` event; every id is the pair's `Identity`; both rows carry the model decision's domain and served sensitivity and one scope and the attestation its anchor; the witness carries the attestation's source triple at revision 1 and the document kind's witness kind; no row cites evidence by FK | unaudited |
| Write-set row comparison | `crates/daemon/src/attestation_verifier/attest.rs:415` in `WriteSet::commit` (`:398`), against `WriteSet::at_tip` (`:423`) | a caller's write set equals, apart from the witness and citation timestamps, the set the model decision's row at the committed tip yields; otherwise `invalid_write_set` | unaudited |
| Envelope recheck | `crates/daemon/src/attestation_verifier/attest.rs:634` (`recheck`): incarnation at `:643`, row anchor at `:671`, anchor at `:681`, descriptor at `:690`, the model decision's citations at `:709`; served sensitivity at `:445` | the model decision, its anchor at the sequence the envelope extends read in the envelope's transaction, its descriptor with its cited evidence row live, the classification's revision, and the claim's path, digest, and byte length, and the class the Kernel serves it at are what the classification read; otherwise `changed` | unaudited |
| Witness oracle | `crates/daemon/src/attestation_verifier/attest.rs:468` | the envelope commits only at effective maturity `Verified` and disposition `Active`; otherwise `witness_degraded` | unaudited |
| Opposite-key refusal | `crates/daemon/src/attestation_verifier/attest.rs:597` in `run` (`:585`) | a recorded key for the other verdict on the same decision and oid is `conflict` | unaudited |
| Stale path | `crates/daemon/src/attestation_verifier/attest.rs:507` (`mark_stale`), through `record_stale` (`:540`), skip at `:548`, admission at `:551` | one `MarkStale` under the latest admission's classes; an already `Stale` decision is skipped; a `MarkStale` returning another disposition is `invalid_write_set` | unaudited |
| Verifier flag | `crates/daemon/src/config.rs:162` (default), `ConfigKey::VerifierEnabled` user-only; `crates/daemon/src/attestation_verifier/bounded_pass.rs:379` in `Verifier::pass` (`:351`) | the flag is off by default, a project tier cannot set it, the pass reads it per project before any repository or Kernel read, and the production reader (`crates/daemon/src/verifier_task.rs:83`) resolves the user tier at the filesystem root, so one memoized merge answers every project | unaudited |
| Pass budgets | `crates/daemon/src/attestation_verifier/bounded_pass.rs:626` in `ProjectRun::decisions` (`:545`); `MAX_VERDICTS` at `:100` | a project stops at N verdicts or N × 256 KiB blob bytes per pass | unaudited |
| Pass cursor | `crates/daemon/src/attestation_verifier/bounded_pass.rs:618` in `ProjectRun::decisions` (`:545`) | the cursor moves only over a resolved prefix; a resolved decision behind an unresolved one is remembered; a project whose pages ran out with every decision resolved records its floor (`:639`) and drops the remembered decisions below it (`:638`), as a visit while the prefix holds drops those before its key (`:592-593`), and the next pass pages from the later of the floor and the cursor (`:547`); a page shorter than the row cap ends the project's pages (`:571-572`) | unaudited |
| Pass skips | `crates/daemon/src/attestation_verifier/bounded_pass.rs:604-604` (`skipped_stale`) in `ProjectRun::decisions` (`:545`); `:683` (`skipped_attested`) in `ProjectRun::attested` (`:670`), which `ProjectRun::decision` (`:691`) calls at the oid of the anchor the page's standing read names before classification (`:701`) and at the classified oid after (`:745`) | a `Stale` decision and one whose attestation id the registry holds under the `verifier` source kind are skipped before any claim verification read or Kernel write; an object another source kind wrote under that id skips nothing; an attested decision whose page standing names a live `reachable_from` anchor that the anchor read returns is skipped before its classification reads | unaudited |
| Pass retry bound | `crates/daemon/src/attestation_verifier/bounded_pass.rs:646` (`retry`), `:661-662` (`transient`), `MAX_ATTEMPTS` at `:102` | a decision resolves on its K-th attempt in total, shared across transient verdicts, deadlines, classification and commit refusals, `changed` outcomes, and degraded witnesses; attempts are keyed by `(created_commit_seq, object_id)`, and a visit while the resolved prefix holds (`:592-593`) or a recorded floor (`:637-637`) drops the attempts behind it | unaudited |
| Pass effect counters | `crates/daemon/src/attestation_verifier/bounded_pass.rs:786` (`ProjectRun::outcome`), after `attest` at `:781` | each outcome increments its effect counter after the commit returns | unaudited |
| Scope decision page | `crates/kernel/src/slice/read.rs:833` (`scope_decisions_after_within_budget`) | a scope's live decisions after a `(created_commit_seq, object_id)` pair in that order, of every source kind but one, at most a row cap written into the statement text (`limited`, `:823`), read by seeking the commit-sequence index `idx_decisions_known_as_of` at the pair's sequence, under the budget's deadline for the wait and the read | unaudited |
| Applicability evaluation | `crates/daemon/src/attestation_verifier/applicability.rs:72` (`evaluate_in`), digest comparison at `:88`, checked descriptor read at `:95`; `:46` (`evaluate`) | the witness detail decodes at `WITNESS_DETAIL_VERSION` and names a lineage whose live document descriptor at the snapshot carries the witnessed evidence digest (`Current`), another digest whose checked row carries another digest (`Republished`), or no live descriptor (`Unavailable`) | unaudited |
| Applicability stale mark | `crates/daemon/src/attestation_verifier/applicability.rs:116` (`mark_republished`), key at `:128`, descriptor recheck at `:169`, `record_stale` at `:172` | one `MarkStale` on a live `Active` `verifier` attestation under its latest admission's classes with no evidence, keyed by the attestation and the descriptor revision; a `Stale` attestation is skipped and any other change is `changed` | unaudited |
| Applicability step | `crates/daemon/src/attestation_verifier/bounded_pass.rs:821-822` (`ProjectRun::applicability`), cycle commit at `:824-826`, page bound at `:828`, page transaction at `:834`, N bound at `:992`, cycle end at `:861`; `:972` (`read_page`); `:881` (`ProjectRun::applicable`), commit poll at `:895` | at most N `Active` attestations evaluated per project per pass from a process-local cursor that returns to the start on a page shorter than `PAGE_ROWS`, each cycle paging through the commit its first pass captured; at most N pages read per pass, each in one read transaction whose page statement stops at the first `Active` attestation past the N bound; the deadline is polled before each page and each commit | unaudited |
| Source-kind decision page | `crates/kernel/src/slice/read.rs:732` (`SnapshotRead::scope_source_decisions_after`), page statement at `:810` | a scope's live decisions of one source kind after a registry `(source_id, object_id)` pair in that order, at most a row cap, read by seeking `idx_objects_source` at the pair's `source_id` and handed to the caller one row at a time | unaudited |
| Lineage descriptor read | `crates/kernel/src/source_descriptor.rs:1252` (`SnapshotRead::live_source_descriptor_of_lineage`) through `Envelope::lineage_descriptor` (`:974`); `:1268` (`SnapshotRead::live_source_descriptor_digest_of_lineage`) | the one live descriptor of a class in a lineage at a sequence, with the page read's row checks; the digest read runs the checked read's statement over the same lineage range and liveness predicate and returns the live row's evidence digest | unaudited |
| Verifier block sanitizer | `crates/host-runtime/src/control.rs:2112` (`VERIFIER_COUNTERS`) through `sanitize_counters` | `metrics.verifier` keeps each allowlisted unsigned integer no greater than 2^53, the three applicability counters included, and drops every other field | unaudited |

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
| `the_evidence_keyed_read_returns_the_live_document_descriptor_citing_the_evidence` | `crates/kernel/tests/kernel_source_descriptors.rs:3339` | document descriptor; verifier input | unaudited |
| `the_anchor_read_returns_the_decoded_condition_and_refuses_an_undecodable_row` | `crates/kernel/tests/kernel_anchor_write.rs:869` | verifier input | unaudited |
| `the_decision_standing_equals_the_standing_read_without_its_serving_facts_at_every_snapshot` | `crates/kernel/tests/kernel_snapshot_read.rs:143` | snapshot purity; verifier input (standing read) | unaudited |
| `one_descriptor_statement_answers_each_cited_row_as_its_single_call_read` | `crates/kernel/tests/kernel_snapshot_read.rs:198` | verifier input (descriptor statement, per-entry refusal) | unaudited |
| `read_at_refuses_before_and_after_its_closure_runs` | `crates/kernel/tests/kernel_snapshot_read.rs:276` | snapshot purity (incarnation, tip, budget) | unaudited |
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
| `a_plugin_route_cannot_write_rows_under_the_verifier_id_prefixes` | `crates/daemon/tests/kernel_routes.rs:6834` | party (AC14, the verifier's id prefixes) | unaudited |
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
| `the_attestation_serves_at_the_model_decisions_folded_class` | `crates/daemon/tests/attestation_verifier_attest.rs:1410` | classes and cited fields (served sensitivity over a `Personal` taint floor, F14) | unaudited |
| `a_served_class_other_than_the_kernels_is_changed` | `crates/daemon/tests/attestation_verifier_attest.rs:1445` | sibling (served-sensitivity recheck) | unaudited |
| `a_write_set_naming_another_document_or_revision_is_refused_before_the_commit` | `crates/daemon/tests/attestation_verifier_attest.rs:1480` | witness evidence equality (AC9: path, descriptor, lineage, revision, scope, citation kind, source revision, source kind) | unaudited |
| `a_classification_from_another_incarnation_or_with_other_classes_is_changed` | `crates/daemon/tests/attestation_verifier_attest.rs:1553` | sibling (incarnation and class recheck, both verdicts) | unaudited |
| `a_mismatch_for_a_decision_re_disposed_after_classification_is_changed` | `crates/daemon/tests/attestation_verifier_attest.rs:1596` | stale path (F2, other disposition) | unaudited |
| `a_descriptor_republished_after_classification_is_changed` | `crates/daemon/tests/attestation_verifier_attest.rs:1624` | sibling (F3, a newer live descriptor revision, both verdicts) | unaudited |
| `a_match_after_a_stale_path_under_another_blob_conflicts` | `crates/daemon/tests/attestation_verifier_attest.rs:1690` | identity (F10, opposite key under another digest) | unaudited |
| `the_attestation_copies_the_model_decisions_kind_scope_and_summary` | `crates/daemon/tests/attestation_verifier_attest.rs:1859` | classes and cited fields (decision kind, scope, and summary copied from the model decision's row) | unaudited |
| `a_write_set_with_content_other_than_the_model_decisions_is_refused_in_the_envelope` | `crates/daemon/tests/attestation_verifier_attest.rs:1938` | classes and cited fields (a caller-edited decision kind, summary, rationale, or payload text is `invalid_write_set`) | unaudited |
| `the_identity_depends_on_the_model_and_the_oid` | `crates/daemon/tests/attestation_verifier_attest.rs:1718` | identity (known answer, oid dependence) | unaudited |
| `a_classification_naming_another_live_anchor_is_changed` | `crates/daemon/tests/attestation_verifier_attest.rs:1996` | sibling (row anchor recheck, both verdicts) | unaudited |
| `a_classification_whose_claim_differs_from_the_cited_descriptor_is_changed` | `crates/daemon/tests/attestation_verifier_attest.rs:2025` | sibling (claim against the cited descriptor, both verdicts) | unaudited |
| `a_classification_citing_a_document_the_model_decision_does_not_is_changed` | `crates/daemon/tests/attestation_verifier_attest.rs:2073` | sibling (compared evidence among the model decision's citations, both verdicts) | unaudited |
| `a_row_level_citation_older_than_the_event_window_still_attests` | `crates/daemon/tests/attestation_verifier_attest.rs:2127` | sibling (row-level citation outside the event window, both verdicts) | unaudited |
| `a_corrupt_cited_descriptor_refuses_only_when_the_citation_walk_reaches_it` | `crates/daemon/tests/attestation_verifier_classify.rs:1165` | verifier input (per-entry refusal in citation order) | unaudited |
| `row_level_evidence_with_no_row_at_the_sequence_refuses_after_the_membership_and_anchor_checks` | `crates/daemon/tests/attestation_verifier_classify.rs:1217` | party, `self_source` half; verifier input (refusal order) | unaudited |
| `a_classification_naming_another_descriptor_revision_is_changed` | `crates/daemon/tests/attestation_verifier_attest.rs:2198` | sibling (descriptor revision recheck, both verdicts) | unaudited |
| `a_match_commits_while_every_pooled_reader_is_held` | `crates/daemon/tests/attestation_verifier_attest.rs:1733` | one outcome (the envelope reads on the writer connection while every pooled reader is held) | unaudited |
| `a_mismatch_for_a_decision_superseded_or_retired_after_classification_is_changed` | `crates/daemon/tests/attestation_verifier_attest.rs:1775` | sibling (F2 on the stale path) | unaudited |
| `a_classification_at_a_sequence_before_the_model_decision_is_changed` | `crates/daemon/tests/attestation_verifier_attest.rs:1827` | sibling; one outcome (a classification before the creating commit, both verdicts) | unaudited |
| `a_mismatch_marks_stale_under_the_model_decisions_own_classes` | `crates/daemon/tests/attestation_verifier_attest.rs:2226` | stale path (AC8, the latest admission's classes under a `Personal` taint) | unaudited |
| `the_committed_rationale_and_detail_name_the_compared_claim` | `crates/daemon/tests/attestation_verifier_attest.rs:2255` | classes and cited fields (the committed rationale and detail) | unaudited |
| `an_occupied_attestation_id_refuses_the_write_set` | `crates/daemon/tests/attestation_verifier_attest.rs:2290` | identity (D1, an occupied id is `refused(conflict)`) | unaudited |
| `the_request_digest_is_pinned` | `crates/daemon/tests/attestation_verifier_attest.rs:2326` | identity (the request digest's tagged layout) | unaudited |
| `an_envelope_reads_the_decisions_live_at_the_tip_it_extends` | `crates/kernel/tests/kernel_slice.rs:2550` | sibling (the envelope's decision-row read at the committed tip) | unaudited |
| `an_envelope_reads_the_decisions_cited_evidence_at_the_tip_it_extends` | `crates/kernel/tests/kernel_slice.rs:2595` | sibling (the envelope's linked-evidence read at the committed tip) | unaudited |
| `an_envelope_reads_the_anchors_live_at_the_tip_it_extends` | `crates/kernel/tests/kernel_anchor_write.rs:971` | sibling (the envelope's anchor read at the committed tip) | unaudited |
| `a_flag_off_pass_runs_nothing_and_a_reload_to_false_stops_the_next_pass` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:442` | flag off (AC10, AC13 reload, F12) | unaudited |
| `each_pass_verifies_at_most_n_per_project_and_no_project_starves` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:486` | pass bounds (AC11, F13); blob read bounded (per pass) | unaudited |
| `a_deadline_between_decisions_leaves_exactly_k_verdicts_and_the_cursor_before_the_next` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:553` | cursor (AC4, F7) | unaudited |
| `a_rolled_back_commit_counts_its_refusal_and_a_replayed_commit_counts_its_replay` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:601` | pass accounting (AC12, F10, F11) | unaudited |
| `a_concurrent_commit_completes_while_a_verifier_read_is_stalled` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:692` | writer never held across a read (AC13) | unaudited |
| `a_transient_head_resolves_after_k_passes_and_a_terminal_reason_is_read_once` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:745` | progress under transient; cursor (F13) | unaudited |
| `a_restart_rescan_writes_nothing_and_counts_skips` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:785` | cursor (restart) | unaudited |
| `the_task_steps_only_under_the_flag_and_stops_on_cancellation` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:825` | flag off (task) | unaudited |
| `the_health_block_names_exactly_the_documented_counters` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:875` | counters block and wire table | unaudited |
| `a_witness_degraded_k_passes_is_skipped_and_the_pass_continues` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:908` | progress under transient; degradation (OQ6-11) | unaudited |
| `a_head_decision_that_meets_the_deadline_k_passes_is_skipped_and_read_no_more` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:972` | progress under transient (deadline at the commit and during a read); cursor | unaudited |
| `a_deadline_at_the_commit_leaves_the_decision_unwritten_and_the_next_pass_commits` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1001` | cursor (F7 at the commit poll); pass accounting | unaudited |
| `unverifiable_and_attested_decisions_count_outside_n` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1030` | pass accounting (unverifiable and skip buckets outside N) | unaudited |
| `a_worktree_root_verifies_under_its_git_identity_and_a_nested_root_binds_none` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1074` | bound roots (worktree `git:` identity, nested root) | unaudited |
| `a_failed_derivation_leaves_the_root_unbound_and_its_decision_is_skipped_after_k_passes` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1117` | progress under transient (an unreadable root binds no repository; its decision resolves after K passes) | unaudited |
| `a_broken_root_leaves_the_projects_healthy_root_bound` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1151` | progress under transient; bound roots (a broken root beside a healthy one) | unaudited |
| `a_deadline_at_the_derivation_passes_over_the_project_and_the_next_pass_verifies` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1182` | progress under transient (a deadline-cut derivation spends no attempt and caches nothing) | unaudited |
| `a_pass_budget_exhausted_during_the_derivation_passes_over_the_project` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1207` | progress under transient (a walk cut by the exhausted pass budget passes over the project and caches nothing) | unaudited |
| `a_classification_deadline_k_passes_is_skipped_and_the_cursor_passes_it` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1247` | progress under transient (a classification deadline spends an attempt; the K-th counts `unavailable_skipped`) | unaudited |
| `a_decision_resolves_only_against_its_own_projects_roots` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1280` | bound roots (per-project resolution) | unaudited |
| `a_terminal_reason_survives_a_project_unbound_and_bound_again` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1305` | cursor (terminal reason across pruning) | unaudited |
| `a_flag_off_project_beside_an_enabled_one_is_left_untouched` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1336` | flag off (per project, no derivation) | unaudited |
| `a_transient_head_ahead_of_a_page_boundary_counts_each_decision_once` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1364` | cursor (page boundary) | unaudited |
| `a_terminal_reason_is_not_read_again_after_an_incarnation_change` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1401` | cursor (terminal reason across an incarnation change) | unaudited |
| `the_task_paces_passes_by_their_progress` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1426` | task intervals | unaudited |
| `cancelling_an_in_flight_pass_returns_before_its_deadline` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1463` | task cancellation | unaudited |
| `a_digest_only_root_derives_again_after_a_decision_names_an_unbound_repository` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:2324` | progress under transient; bound roots (a root derived before its repository's first commit binds the `git:` identity a later publication names, within K passes) | unaudited |
| `a_row_the_standing_read_refuses_is_skipped_after_k_passes` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:2407` | progress under transient (a row the decisions step's batched standing read refuses); pass skips (a restart skips the attested decision behind it at the check after classification) | unaudited |
| `a_refused_anchor_read_leaves_the_decision_to_its_classification` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:2461` | pass skips (a refused pre-check anchor read neither stops the project nor resolves the decision, and the decision behind it verifies); progress under transient | unaudited |
| `a_drained_project_verifies_the_decisions_committed_after_its_last_page` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:2510` | cursor (the floor a drained project records, a decision at the floor's sequence, decisions committed behind another project's and during the project's own pass after its target capture, and a transient decision committed past the floor) | unaudited |
| `an_attested_decision_skips_before_its_classification_after_its_descriptor_retires` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:2619` | pass skips (attested skip before classification) | unaudited |
| `an_object_another_source_wrote_under_the_attestations_id_skips_nothing` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:2698` | pass skips (an `assistant` object under the attestation's id: the mismatch behind it is marked stale, the match behind it verifies and its commit is refused by the occupied id) | unaudited |
| `a_terminal_reason_does_not_resolve_the_same_object_id_citing_another_path` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:2729` | cursor (a terminal reason is keyed by the claim's path and oid, so another store's decision under the same object id citing a present path verifies) | unaudited |
| `a_transient_decision_retired_before_it_resolves_leaves_no_attempt` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:2759` | pass retry bound (a retired transient decision's attempt is dropped by the floor and by a cursor advance) | unaudited |
| `a_resolved_decision_retired_behind_a_transient_head_is_forgotten_at_the_floor` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:2831` | cursor (a remembered resolved decision retired with its transient head is dropped by the floor) | unaudited |
| `a_failed_derivation_without_decisions_fails_the_pass` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:2874` | progress under transient (a failed root derivation fails the pass report with no decision to count it) | unaudited |
| `two_decisions_citing_one_absent_path_are_each_counted` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:3572` | pass accounting (two decisions on one absent path at one oid are each read and counted; the terminal reason is per decision) | unaudited |
| `retired_decisions_behind_a_transient_first_row_are_dropped` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:3600` | cursor (retired attempts and remembered decisions before a transient first row are dropped while the prefix holds) | unaudited |
| `every_reason_and_unverifiable_class_names_a_counter` | `crates/daemon/src/attestation_verifier/bounded_pass.rs:1052` | counters block (every bucket name) | unaudited |
| `bound_projects_follow_bound_root_order` | `crates/daemon/src/verifier_task.rs:176` | project order | unaudited |
| `a_project_bound_through_a_symlink_gets_its_canonical_root` | `crates/daemon/src/verifier_task.rs:224` | bound roots (a project's root is the binding's canonical root, the one its scope and source capture's repository identity derive from) | unaudited |
| `the_pass_reader_follows_the_user_tier_and_drops_a_project_tier_value` | `crates/daemon/src/config.rs:3959` | flag off (reader, reload, project tier) | unaudited |
| `the_pass_reader_reads_the_user_tier_alone` | `crates/daemon/src/config.rs:4001` | flag off (the reader resolves the user tier and leaves a project root's tier file unread) | unaudited |
| `the_page_walks_a_scopes_live_decisions_in_commit_order_after_the_cursor` | `crates/kernel/tests/kernel_scope_decisions.rs:66` | cursor (page order, including a cursor whose object id sorts after later decisions') | unaudited |
| `the_scope_decision_page_seeks_the_commit_sequence_index` | `crates/kernel/src/slice/read.rs:875` | page plan only: the read seeks the commit-sequence index and sorts within one sequence | unaudited |
| `an_interrupted_tip_query_is_a_deadline` | `crates/kernel/src/slice/read.rs:976` | cursor (a tip query the budget's progress handler interrupts is `Deadline`, the page read's documented refusal) | unaudited |
| `every_fixture_row_matches_through_the_configuration_cache` | `crates/daemon/src/config.rs:3919` | flag off (project tier, Rust) | unaudited |
| `verifier parity fixture` | `packages/opencode-plugin/src/config/index.test.ts:1101`, `packages/pi-plugin/src/config/index.test.ts:754` | flag off (project tier, both plugins) | unaudited |
| `a_republished_document_marks_its_attestation_stale_once_and_leaves_the_model_decision_unchanged` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1633` | stale on byte change (AC18); `MarkStale` under the attestation's classes | unaudited |
| `an_equal_republish_a_retired_descriptor_and_an_undecodable_witness_write_nothing` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1706` | stale on byte change (silent cases) | unaudited |
| `an_attestation_retired_before_its_stale_mark_commits_writes_nothing` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1799` | stale on byte change (F2) | unaudited |
| `a_replayed_stale_mark_key_writes_nothing_new` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1840` | stale on byte change (F10) | unaudited |
| `a_commit_deadline_leaves_its_attestation_first_for_the_next_pass` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1915` | stale on byte change (N, commit deadline); pass bounds | unaudited |
| `the_applicability_step_resumes_after_the_last_attestation_it_evaluated` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:1969-1969` | stale on byte change (cursor resume); pass bounds | unaudited |
| `a_budget_cancelled_at_the_stale_mark_commit_counts_a_deadline_refusal` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:2032` | stale on byte change (commit refused at the deadline) | unaudited |
| `a_revision_published_before_the_stale_mark_commits_refuses_it_as_changed` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:2063` | stale on byte change (descriptor recheck) | unaudited |
| `a_plugin_stale_mark_before_the_verifiers_commit_leaves_one_mark` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:2091-2092` | stale on byte change (disposition recheck) | unaudited |
| `a_witness_read_the_kernel_refuses_resolves_its_attestation_and_the_step_continues` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:2139` | stale on byte change (refused read) | unaudited |
| `an_attestation_row_the_standing_read_refuses_resolves_and_the_step_continues` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:2187` | stale on byte change (refused standing row) | unaudited |
| `the_applicability_step_pages_at_most_n_pages_of_rows_per_pass` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:2228` | stale on byte change (page bound); pass bounds | unaudited |
| `the_applicability_step_runs_after_the_decisions_step_reaches_n_verdicts` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:2898` | stale on byte change (after the decisions bound) | unaudited |
| `a_deadline_at_the_applicability_page_poll_leaves_the_attestation_for_the_next_pass` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:2923` | stale on byte change (page poll); pass bounds | unaudited |
| `a_rolled_back_stale_mark_counts_its_refusal_and_a_later_pass_marks` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:2953` | stale on byte change (refused commit) | unaudited |
| `evaluate_reads_the_witness_and_the_descriptor_at_the_targets_commit` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:2998` | stale on byte change (historical snapshot) | unaudited |
| `a_descriptor_retired_before_the_stale_mark_commits_refuses_it_as_changed` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:3050` | stale on byte change (lineage recheck) | unaudited |
| `an_attestation_retired_within_an_applicability_cycle_is_passed_over` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:3093` | stale on byte change (cycle commit) | unaudited |
| `an_applicability_cycle_marks_each_of_its_projects_attestations_once_and_no_other_projects` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:3137` | stale on byte change (scope filter, cycle coverage across pages); pass bounds | unaudited |
| `an_idle_pass_over_a_drained_project_reads_no_decisions_page` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:3189` | cursor (a drained project whose store committed nothing reads only its target and applicability page) | unaudited |
| `the_evaluation_bound_holds_across_the_pages_of_a_pass` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:3307` | pass bounds (N evaluations across two pages of one pass); stale on byte change (cursor at the N-th) | unaudited |
| `a_restore_before_a_pages_stale_marks_stops_the_project_after_one_refusal` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:3382` | stale on byte change (a restore between a page's reads and its marks counts one refusal and stops the project) | unaudited |
| `a_changed_mark_leaves_the_rest_of_its_page_to_be_marked_in_the_same_pass` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:3447` | stale on byte change (a mark refused as changed with the incarnation unchanged resolves only its own attestation) | unaudited |
| `a_rolled_back_stale_mark_leaves_the_rest_of_its_page_to_their_own_marks` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:3514` | stale on byte change (a mark the Kernel rolls back resolves only its own attestation; each mark of the page is attempted and counted) | unaudited |
| `a_writer_reads_the_live_revision_of_one_lineage` | `crates/kernel/tests/kernel_source_descriptors.rs:2965` | stale on byte change (the digest read returns the checked row's evidence digest, or `None`, and the stored digest of a row the checks refuse; both reads refuse a lineage range with two live rows) | unaudited |
| `the_descriptor_row_checks_run_only_for_a_digest_that_differs_and_before_any_mark` | `crates/daemon/tests/attestation_verifier_bounded_pass.rs:3249` | stale on byte change (descriptor row checks before a mark) | unaudited |
| `the_registry_source_page_walks_a_scopes_live_decisions_of_one_source_kind_in_source_order` | `crates/kernel/tests/kernel_scope_decisions.rs:151` | stale on byte change (page read) | unaudited |
| `the_source_decision_page_seeks_the_registry_source_index` | `crates/kernel/src/slice/read.rs:935` | stale on byte change (page read plan) | unaudited |
| `a_page_statement_with_its_bound_in_the_text_is_prepared_once` | `crates/kernel/src/slice/read.rs:908` | pass bounds (a page statement keeps its prepared plan across executions) | unaudited |
| `the_verifier_block_is_sanitized_field_by_field_and_documented` | `crates/host-runtime/src/control.rs:2179` | counters block on the wire | unaudited |

## Categories with no check found

- A transient refusal of an applicability witness or descriptor read (`Busy`, `Io`) failing the pass: no test induces a transient Kernel error at that read; the row-caused witness-read and checked descriptor-read refusals are tested through the step, and the transient half is covered by reading `ProjectRun::applicable`.
- No test exercises a refused applicability page read, a standing read refused for a store-wide reason inside the page's read transaction, a stale-mark conflict, or an incarnation change at the page's `read_at`.
- `p6-plugin-cannot-replace-a-foreign-source-decision`: no test submits a
  plugin replacement of a decision of another source kind.
- `p6-artifact-deletion-invalidates-cited-evidence-without-the-citation-guard`:
  no test follows the deletion of cited evidence with a witness admission.
- The Kernel checkout opener: no test reads an over-budget delta base or a
  replacement ref through it.
- The trigger fold of `Sensitive` document evidence: no test promotes through
  a trigger whose observation cites document evidence.
- `p6-new-verifiability-pure-function-of-snapshot`: no test starts a restore
  while a classification holds its reader; the restore waits for the reader
  guard, and the tests cover a target of another incarnation refused before
  the closure runs and after the transaction.
- `p6-new-attestation-stale-on-byte-change`: no test drives source capture
  itself to republish an attested document; the tests publish the second
  descriptor revision through the Kernel, and
  `p6-document-republishes-only-on-digest-change-and-retires-on-missing`
  covers when source capture publishes one. A deadline that
  interrupts `evaluate_in` refuses the whole page at `read_at`'s closing budget
  check (`crates/kernel/src/snapshot_read.rs:69`), so the step stops at the
  page refusal (`crates/daemon/src/attestation_verifier/bounded_pass.rs:838-839`)
  with no count; no test drives a deadline inside the page's read
  transaction, and `a_replayed_stale_mark_key_writes_nothing_new` asserts only
  `read_at`'s budget check before `evaluate`'s closure runs.
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
