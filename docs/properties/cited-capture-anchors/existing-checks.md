# Cited capture anchors: existing checks

Every check below has status `unaudited`. A listed check is evidence that a
claim is exercised, not a verdict that it is adequate; adequacy belongs to
`/testing:invariant-test-review` for tests and
`/low-level-systems:defensive-assertions-and-invariant-guards` for
production guards. References were verified at the branch that adds this
part.

## Production guards

| Guard | Location | Claim it carries | Status |
| --- | --- | --- | --- |
| Bound check before decode | `crates/kernel/src/applicability/engine.rs:1277` (`anchor_row_exceeds_bounds`), called at `:537` | oversized anchor rows are `Uncertain` without decoding | unaudited |
| Kind-owned columns | `crates/kernel/src/anchor.rs:245` (`unowned_columns`) | a row naming one kind with another kind's value fails decode | unaudited |
| Typed decode | `crates/kernel/src/anchor.rs:272` (`AnchorCondition::decode`) | unknown kinds, missing values, bad oids, bad ranges, and inverted intervals fail decode | unaudited |
| Duplicate captures | `crates/kernel/src/anchor.rs:200` (`decode_captures`) | two captures of one commit fail decode | unaudited |
| Ancestry first, complete-scan negative | `crates/kernel/src/applicability/resolve.rs:230` (`resolve_commit`), `:270` (`window_scan_complete`) | `Holds` on ancestor-or-equal; `DoesNotHold` only on complete evidence | unaudited |
| Anchor cache key | `crates/kernel/src/applicability/engine.rs:205` (`AnchorCacheKey`) | verdict reuse requires equal identity, row, HEAD, state, and algorithm | unaudited |
| Live parents | `crates/kernel/src/slice/write.rs:1095` (`require_parents`) | decision parents are live at insert | unaudited |
| Sensitivity fold | `crates/kernel/src/slice/write.rs:980` (`fold_cited_evidence_class`) | a citing row is no less restrictive than its evidence | unaudited |
| Cited-evidence predicate | `crates/kernel/src/slice/write.rs:42` (`EVIDENCE_CITED_SQL`), `:585` (`retire_evidence`); `crates/kernel/src/local_file.rs:245` (`expire_harness_captures`) | cited evidence is never retired or swept | unaudited |
| Capture receipt key and digest | `crates/daemon/src/memory_capture.rs:1162` (`publish_captured_memories`) | replay by job key and frozen-plan digest | unaudited |
| Quote and source validation | `crates/daemon/src/memory_capture.rs:208` (`parse_capture_batch`), `:270` (`validate_capture_memories`) | quotes are substrings of their own batch source | unaudited |
| Job identity | `crates/memory-store/src/memory_capture.rs:112` (`job_id`) | job id derives from the source tuple and redacted text | unaudited |
| Hashed fragment id | `crates/daemon/src/memory_capture.rs:315` (`native_capture_fragments`) | out-of-grammar or oversized ids become `{sha256}:{offset}` | unaudited |
| Capture commit gate | `crates/daemon/src/memory_capture.rs:983`; `crates/daemon/src/lib.rs:6909` | publish, checkpoint, source publish, and delete serialize | unaudited |
| Message unit identity | `crates/daemon/src/harness_sources.rs:189` (`SessionIdentity::message_unit`) | one unit per text block | unaudited |
| Evidence id from receipt key | `crates/daemon/src/harness_sources.rs:992` (`evidence_request`) | `srcev:` plus the unit key | unaudited |
| Isolated bounded open | `crates/daemon/src/git_sources.rs:128` (`isolated_options`), `:138` (`open`) | isolation, replacement refs ignored, allocation limit | unaudited |
| HEAD observation | `crates/daemon/src/memory_capture/head.rs:21` (`observe_head`), called once per submit at `crates/daemon/src/memory_capture/native.rs:562` | HEAD resolved through the ref store and one object header; unborn, unreadable, unsupported, non-commit, and non-repository roots are `HeadUnavailable` | unaudited |
| Anchor write validation | `crates/kernel/src/anchor_write.rs:263` (`validate_anchor`), `:254` (`validate_anchor_row`) | written anchor rows pass the engine's bound check and decoder and round-trip | unaudited |
| Anchor write order | `crates/kernel/src/anchor_write.rs:320` (`insert_anchor_inner`) | registry row, anchor row, and change record in one envelope | unaudited |

## Claim-bearing tests

| Test | Records | Status |
| --- | --- | --- |
| `crates/kernel/tests/kernel_anchor.rs:252` `unknown_kind_and_missing_values_fail_decode` | `p2-undecodable-or-oversized-anchor-row-is-uncertain` | unaudited |
| `crates/kernel/tests/kernel_anchor.rs:289` `corrupt_capture_payload_only_disables_fallback_rungs` | same | unaudited |
| `crates/kernel/tests/kernel_anchor.rs:304` `rows_populating_another_kinds_column_fail_decode` | same | unaudited |
| `crates/kernel/tests/kernel_applicability_engine.rs:241` `malformed_inputs_stay_uncertain_and_do_not_poison_the_batch` | same | unaudited |
| `crates/kernel/tests/kernel_applicability_engine.rs:2108` `an_oversized_anchor_payload_is_uncertain_without_being_decoded` | same | unaudited |
| `crates/kernel/tests/kernel_applicability_engine.rs:4143` `duplicate_anchor_captures_are_uncertain_rather_than_last_wins` | same | unaudited |
| `crates/kernel/tests/kernel_anchor_resolution.rs:44` `reachable_from_holds_on_descendants_and_fails_on_foreign_branches` | `p2-reachable-from-holds-on-ancestor-of-head` | unaudited |
| `crates/kernel/tests/kernel_anchor_resolution.rs:159` `criss_cross_histories_answer_deterministically` | same | unaudited |
| `crates/kernel/tests/kernel_anchor_resolution.rs:557` `rebase_merged_through_a_second_parent_still_resolves` | same | unaudited |
| `crates/kernel/tests/kernel_applicability_engine.rs:806` `unconstrained_objects_do_not_claim_an_anchor_held` | same | unaudited |
| `crates/kernel/tests/kernel_anchor_resolution.rs:283`, `:325`, `:507`, `:601`, `:990`, `:1130`, `:1254`, `:1289`, `:1345`, `:1406`, `:1455` | `p2-reachable-from-negative-needs-complete-evidence` | unaudited |
| `crates/kernel/tests/kernel_applicability_engine.rs:148`, `:206`, `:518`, `:1029`, `:1064`, `:2621`, `:2659`, `:2881`, `:3080`, `:3434`, `:3676`, `:3943`, `:4024`, `:4095` | `p2-anchor-verdict-cache-key-covers-row-head-and-boundary` | unaudited |
| `crates/kernel/tests/kernel_slice.rs:196` `slice_payloads_redact_before_storage_and_missing_parents_are_typed` | `p2-decision-parents-must-be-live-at-insert` | unaudited |
| `crates/kernel/tests/kernel_envelope.rs:669` `a_constraint_violation_is_a_conflict_rather_than_an_io_failure` | same | unaudited |
| `crates/kernel/tests/kernel_anchor_write.rs:388` `a_decision_inserted_before_its_anchor_is_not_found_and_the_commit_rolls_back` | same (anchor clause); `p2-new-anchor-and-registry-share-one-envelope` | unaudited |
| `crates/kernel/tests/kernel_slice.rs:1453` `a_decision_citing_evidence_is_classified_no_lower_than_that_evidence` | `p2-citing-decision-carries-at-least-its-evidence-class` | unaudited |
| `crates/kernel/tests/kernel_slice.rs:1526` `a_decision_serves_no_lower_than_its_cited_evidence_reads_today` | same | unaudited |
| `crates/kernel/tests/kernel_review_acceptance.rs:988` `an_accepted_decision_serves_no_lower_than_the_support_it_cites_reads_today` | same | unaudited |
| `crates/kernel/tests/kernel_source_descriptors.rs:1895` `retiring_evidence_invalidates_uncited_evidence_and_refuses_cited_evidence` | `p2-cited-evidence-is-never-retired-or-swept` | unaudited |
| `crates/daemon/tests/harness_sources.rs:3482` `harness_capture_expiry_keeps_held_and_cited_evidence_and_drains_in_pages` | same | unaudited |
| `crates/kernel/src/local_file.rs:791` `a_harness_capture_pinned_after_it_was_selected_for_expiry_is_kept` | same | unaudited |
| `crates/kernel/src/local_file.rs:889` `a_refused_harness_capture_rolls_back_alone_inside_its_page_commit` | same | unaudited |
| `crates/daemon/src/memory_capture.rs:1674` `correction_supersedes_and_replay_or_duplicate_creates_no_extra_memory` | `p2-capture-commit-replays-by-job-key-and-frozen-digest`; `p2-capture-job-and-decision-identity-derive-from-the-source-tuple` | unaudited |
| `crates/kernel/tests/kernel_envelope.rs:160` `receipt_replay_is_effect_free_and_digest_conflict_is_typed` | `p2-capture-commit-replays-by-job-key-and-frozen-digest` | unaudited |
| `crates/daemon/src/lib.rs:24489` `automatic_capture_commits_once_and_is_explicitly_retrievable` | same; identity record | unaudited |
| `crates/daemon/src/memory_capture.rs:1285`, `:1318`, `:1350`, `:1391`, `:1398` | `p2-capture-quote-is-an-exact-substring-of-its-own-batch-source` | unaudited |
| `crates/daemon/src/lib.rs:24880` `automatic_capture_rejection_stays_pending_not_successful_empty_memory` | same | unaudited |
| `crates/memory-store/src/memory_capture.rs:755` `job_identity_derives_from_redacted_text_so_a_stored_digest_cannot_confirm_a_secret` | `p2-capture-job-and-decision-identity-derive-from-the-source-tuple` | unaudited |
| `crates/daemon/src/memory_capture.rs:1417`, `:1466`, `:1566`, `:1588`; `crates/daemon/src/lib.rs:25796` | same | unaudited |
| `crates/daemon/src/lib.rs:25028`, `:25366`, `:25571`, `:26119`, `:24777`; `crates/daemon/src/source_capture_tests.rs:1036` | `p2-capture-publish-and-session-delete-are-serialized` | unaudited |
| `crates/daemon/src/source_capture_tests.rs:866`, `:1258`; `crates/kernel/tests/kernel_source_descriptors.rs:494`, `:2484`, `:2593`, `:2717`, `:2795`, `:2871`; `crates/daemon/tests/harness_sources.rs:1706` | `p2-messages-descriptor-identity-is-per-block-and-evidence-id-is-the-receipt-key` | unaudited |
| `crates/daemon/src/source_capture_tests.rs:180`, `:428`, `:464`, `:1072`, `:1233` | `p2-deferred-source-units-publish-on-a-later-pass` | unaudited |
| `crates/daemon/tests/git_sources.rs:549` `refused_selections_publish_nothing_and_encodings_are_dispositions` | `p2-git-open-is-isolated-and-allocation-bounded` | unaudited |
| `crates/daemon/tests/git_sources.rs:827` `delta_packed_commit_against_an_oversized_base_is_refused` | same | unaudited |
| `crates/daemon/src/memory_capture/head.rs:123` `a_branch_head_and_a_detached_head_freeze_their_commit` | `p2-new-head-observed-once-and-frozen` | unaudited |
| `crates/daemon/src/memory_capture/head.rs:139` `unborn_unsupported_non_commit_and_non_repository_roots_are_unavailable` | `p2-new-head-unavailable-keeps-decision-admitted`; `p2-git-open-is-isolated-and-allocation-bounded` | unaudited |
| `crates/daemon/src/memory_capture/head.rs:173` `the_observation_reads_at_most_one_object_and_neither_the_index_nor_the_tree` | `p2-new-head-observed-once-and-frozen` (no status scan, at most one object read) | unaudited |
| `crates/daemon/src/memory_capture/head.rs:204` `frozen_heads_serialize_as_tagged_values_and_round_trip`; `crates/daemon/src/memory_capture.rs:1654` `a_plan_frozen_without_a_head_reads_as_head_unavailable` | same (C13 compatibility) | unaudited |
| `crates/daemon/src/lib.rs:24619` `a_submission_freezes_one_checkout_head_in_every_plan_and_a_retry_keeps_it` | `p2-new-head-observed-once-and-frozen`; `p2-new-head-unavailable-keeps-decision-admitted` | unaudited |
| `crates/daemon/src/lib.rs:24729` `a_plan_frozen_without_a_head_publishes_as_before` | `p2-new-head-observed-once-and-frozen` (C13: a plan without `head` publishes) | unaudited |
| `crates/kernel/tests/kernel_anchor_write.rs:300` `every_anchor_kind_commits_a_row_the_engine_decodes_and_a_decision_can_reference` | `p2-new-anchor-row-decodes-on-write`; `p2-new-anchor-and-registry-share-one-envelope` | unaudited |
| `crates/kernel/tests/kernel_anchor_write.rs:367` `git_anchors_with_an_empty_capture_list_store_a_null_payload` | `p2-new-anchor-row-decodes-on-write` | unaudited |
| `crates/kernel/tests/kernel_anchor_write.rs:443` `invalid_anchors_are_refused_as_invalid_input_with_a_typed_cause` | same | unaudited |
| `crates/kernel/tests/kernel_anchor_write.rs:595` `a_raw_row_with_an_unowned_column_is_conflicting_columns` | same | unaudited |
| `crates/kernel/tests/kernel_anchor_write.rs:418` `a_decision_failure_after_the_anchor_rolls_back_both` | `p2-new-anchor-and-registry-share-one-envelope` | unaudited |
| `crates/kernel/tests/kernel_anchor_write.rs:622` `a_missing_domain_is_not_found` | same | unaudited |
| `crates/kernel/tests/kernel_anchor_write.rs:643` `a_duplicate_anchor_id_is_a_conflict_within_and_across_commits` | same; Kernel half of `p2-new-capture-anchor-identity-is-deterministic` | unaudited |
| `crates/daemon/tests/claim_sources.rs:1508` `anchor_commits_pass_through_the_claim_materializer` | `p2-new-anchor-and-registry-share-one-envelope` (consumer side) | unaudited |

## Empty categories

- Property-based or model-checked tests over anchor rows: none found.
- Production assertions on the anchor writer beyond typed refusals: none
  found; the writer returns `InvalidInput`, `NotFound`, or `Conflict`.
- Checks for the 10 proposed records: none found; their code does not exist.

## Suspiciously quiet areas

- No production path reads `anchors` rows into applicability candidates, so
  every S1 check runs on fixtures (specification K3).
- The capture prompt rule at `crates/daemon/src/memory_capture.rs:130` is the
  only statement of the quote contract the model sees; no test pins the
  prompt text to the validator.
- No test joins a capture fragment id to a `messages` descriptor identity; the
  hashed-id case (specification K6, OQ2-12) has no witness.
