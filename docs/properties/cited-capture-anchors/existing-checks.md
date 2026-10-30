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
| Kind-owned columns | `crates/kernel/src/anchor.rs:246` (`unowned_columns`) | a row naming one kind with another kind's value fails decode | unaudited |
| Typed decode | `crates/kernel/src/anchor.rs:273` (`AnchorCondition::decode`) | unknown kinds, missing values, bad oids, bad ranges, and inverted intervals fail decode | unaudited |
| Duplicate captures | `crates/kernel/src/anchor.rs:201` (`decode_captures`) | two captures of one commit fail decode | unaudited |
| Ancestry first, complete-scan negative | `crates/kernel/src/applicability/resolve.rs:230` (`resolve_commit`), `:270` (`window_scan_complete`) | `Holds` on ancestor-or-equal; `DoesNotHold` only on complete evidence | unaudited |
| Anchor cache key | `crates/kernel/src/applicability/engine.rs:205` (`AnchorCacheKey`) | verdict reuse requires equal identity, row, HEAD, state, and algorithm | unaudited |
| Live parents | `crates/kernel/src/slice/write.rs:1095` (`require_parents`) | decision parents are live at insert | unaudited |
| Sensitivity fold | `crates/kernel/src/slice/write.rs:980` (`fold_cited_evidence_class`) | a citing row is no less restrictive than its evidence | unaudited |
| Cited-evidence predicate | `crates/kernel/src/slice/write.rs:42` (`EVIDENCE_CITED_SQL`), `:585` (`retire_evidence`); `crates/kernel/src/local_file.rs:245` (`expire_harness_captures`) | cited evidence is never retired or swept | unaudited |
| Capture receipt key and digest | `crates/daemon/src/memory_capture.rs:1215` (`publish_captured_memories`) | replay by job key and frozen-plan digest | unaudited |
| Quote and source validation | `crates/daemon/src/memory_capture.rs:212` (`parse_capture_batch`), `:274` (`validate_capture_memories`) | quotes are substrings of their own batch source | unaudited |
| Job identity | `crates/memory-store/src/memory_capture.rs:112` (`job_id`) | job id derives from the source tuple and redacted text | unaudited |
| Hashed fragment id | `crates/daemon/src/memory_capture.rs:319` (`native_capture_fragments`) | out-of-grammar or oversized ids become `{sha256}:{offset}` | unaudited |
| Capture commit gate | `crates/daemon/src/memory_capture.rs:1013`; `crates/daemon/src/lib.rs:7030` | publish, checkpoint, source publish, and delete serialize | unaudited |
| Message unit identity | `crates/daemon/src/harness_sources.rs:214` (`SessionIdentity::message_unit`) | one unit per text block | unaudited |
| Evidence id from receipt key | `crates/daemon/src/harness_sources.rs:1020` (`evidence_request`) | `srcev:` plus the unit key | unaudited |
| Isolated bounded open | `crates/daemon/src/git_sources.rs:131` (`isolated_options`), `:138` (`open`), `:148` (`open_with_slots`) | isolation, replacement refs ignored, allocation limit; `open_with_slots` sizes the pack index slot map and, for a given size, refuses the repositories the default sizing refuses | unaudited |
| HEAD observation | `crates/daemon/src/memory_capture/head.rs:32` (`observe_head`), called once per submit at `crates/daemon/src/memory_capture/native.rs:614` | HEAD resolved once through the ref store, following a branch that is itself symbolic to the end of its chain (`head_id`, `crates/daemon/src/memory_capture/head.rs:62`); the header of that one object id read from the loose object or pack files (`crates/daemon/src/git_sources.rs:205`, `file_object_kind`) or the object store (`:490`, `store_object_kind`); unborn, unreadable, unsupported, non-commit, absent-object, and non-repository roots, and a HEAD held by a corrupt pack (an index paired with another pack's data, or a delta chain that revisits an entry), are `HeadUnavailable` | unaudited |
| Capture anchor | `crates/daemon/src/memory_capture/cite.rs:403` (`anchor_spec`), inserted at `crates/daemon/src/memory_capture.rs:1274` | one `reachable_from` anchor per job, ids derived from the job id and the frozen oid, before the job's first decision | unaudited |
| Citation lookup and reasons | `crates/daemon/src/memory_capture/cite.rs:301` (`Citation::lookup`), `:136` (`is_hashed_source_id`) | reads only the job's own lineages inside the envelope; one closed reason per decision | unaudited |
| Block location and target | `crates/daemon/src/memory_capture/cite.rs:198` (`locate`), `:230` (`target`) | digest-located blocks; `block`, `span`, or `first` | unaudited |
| Citation events | `crates/daemon/src/memory_capture/cite.rs:379` (`append_citations`) | one event per cited block, evidence live, decision row uncited | unaudited |
| Capture-decision counters | `crates/daemon/src/source_capture.rs:659` (`record_capture_decisions`); `crates/host-runtime/src/control.rs:709` (`SOURCE_CAPTURE_COUNTERS`) | counts added after a non-replayed receipt; names forwarded by the host | unaudited |
| Settlement record | `crates/daemon/src/source_capture/settlement.rs:91` (`Settlements`), `:272` (`settle`); writers in `crates/daemon/src/source_capture.rs:708` (`into_job`), `:572` (`refuse_handoff`), `:933` (the job's end, after the gate is released), `:470` (`ActiveSlot`) | bounded per-session record; the wait returns by `accepted_at + 5 s`; a free slot leaves no `Running` entry | unaudited |
| Settlement wait | `crates/daemon/src/memory_capture/native.rs:124` (`capture_settlements`, which observes every job session under one lock), awaited at `:394`; `crates/daemon/src/memory_capture/cite.rs:123` (`settlement_key`) | settles each job session's observed entry before the blocking hop that takes the commit gate; skips error submits, consent off, and unjoinable ids; a cancel returns with nothing recorded | unaudited |
| Anchor write validation | `crates/kernel/src/anchor_write.rs:297` (`validate_anchor`), `:288` (`validate_anchor_row`) | written anchor rows pass the engine's bound check and decoder and round-trip | unaudited |
| Anchor write order | `crates/kernel/src/anchor_write.rs:410` (`insert_anchor_inner`) | registry row, anchor row, and change record in one envelope | unaudited |

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
| `crates/kernel/tests/kernel_anchor_write.rs:404` `a_decision_inserted_before_its_anchor_is_not_found_and_the_commit_rolls_back` | same (anchor clause); `p2-new-anchor-and-registry-share-one-envelope` | unaudited |
| `crates/kernel/tests/kernel_slice.rs:1453` `a_decision_citing_evidence_is_classified_no_lower_than_that_evidence` | `p2-citing-decision-carries-at-least-its-evidence-class` | unaudited |
| `crates/kernel/tests/kernel_slice.rs:1526` `a_decision_serves_no_lower_than_its_cited_evidence_reads_today` | same | unaudited |
| `crates/kernel/tests/kernel_review_acceptance.rs:988` `an_accepted_decision_serves_no_lower_than_the_support_it_cites_reads_today` | same | unaudited |
| `crates/kernel/tests/kernel_source_descriptors.rs:1895` `retiring_evidence_invalidates_uncited_evidence_and_refuses_cited_evidence` | `p2-cited-evidence-is-never-retired-or-swept` | unaudited |
| `crates/daemon/tests/harness_sources.rs:3500` `harness_capture_expiry_keeps_held_and_cited_evidence_and_drains_in_pages` | same | unaudited |
| `crates/kernel/src/local_file.rs:791` `a_harness_capture_pinned_after_it_was_selected_for_expiry_is_kept` | same | unaudited |
| `crates/kernel/src/local_file.rs:889` `a_refused_harness_capture_rolls_back_alone_inside_its_page_commit` | same | unaudited |
| `crates/daemon/src/memory_capture.rs:2016` `correction_supersedes_and_replay_or_duplicate_creates_no_extra_memory` | `p2-capture-commit-replays-by-job-key-and-frozen-digest`; `p2-capture-job-and-decision-identity-derive-from-the-source-tuple` | unaudited |
| `crates/kernel/tests/kernel_envelope.rs:160` `receipt_replay_is_effect_free_and_digest_conflict_is_typed` | `p2-capture-commit-replays-by-job-key-and-frozen-digest` | unaudited |
| `crates/daemon/src/lib.rs:24931` `automatic_capture_commits_once_and_is_explicitly_retrievable` | same; identity record | unaudited |
| `crates/daemon/src/memory_capture.rs:1406`, `:1439`, `:1471`, `:1512`, `:1519` | `p2-capture-quote-is-an-exact-substring-of-its-own-batch-source` | unaudited |
| `crates/daemon/src/lib.rs:25326` `automatic_capture_rejection_stays_pending_not_successful_empty_memory` | same | unaudited |
| `crates/memory-store/src/memory_capture.rs:755` `job_identity_derives_from_redacted_text_so_a_stored_digest_cannot_confirm_a_secret` | `p2-capture-job-and-decision-identity-derive-from-the-source-tuple` | unaudited |
| `crates/daemon/src/memory_capture.rs:1538`, `:1587`, `:1900`, `:1922`; `crates/daemon/src/lib.rs:26242` | same | unaudited |
| `crates/daemon/src/lib.rs:25474`, `:25812`, `:26017`, `:26565`, `:25223`; `crates/daemon/src/source_capture_tests.rs:1037` | `p2-capture-publish-and-session-delete-are-serialized` | unaudited |
| `crates/daemon/src/source_capture_tests.rs:867`, `:1259`; `crates/kernel/tests/kernel_source_descriptors.rs:494`, `:2484`, `:2593`, `:2717`, `:2795`, `:2871`; `crates/daemon/tests/harness_sources.rs:1724` | `p2-messages-descriptor-identity-is-per-block-and-evidence-id-is-the-receipt-key` | unaudited |
| `crates/daemon/src/source_capture_tests.rs:181`, `:429`, `:465`, `:1073`, `:1234` | `p2-deferred-source-units-publish-on-a-later-pass` | unaudited |
| `crates/daemon/tests/git_sources.rs:549` `refused_selections_publish_nothing_and_encodings_are_dispositions` | `p2-git-open-is-isolated-and-allocation-bounded` | unaudited |
| `crates/daemon/tests/git_sources.rs:827` `delta_packed_commit_against_an_oversized_base_is_refused` | same | unaudited |
| `crates/daemon/src/memory_capture/head.rs:351` `a_branch_head_and_a_detached_head_freeze_their_commit` | `p2-new-head-observed-once-and-frozen` (loose read) | unaudited |
| `crates/daemon/src/memory_capture/head.rs:373` `a_head_through_a_branch_alias_freezes_the_aliased_commit` | `p2-new-head-observed-once-and-frozen` (branch aliases, loose and packed); `p2-new-head-unavailable-keeps-decision-admitted` (cyclic alias, alias to an unborn branch) | unaudited |
| `crates/daemon/src/memory_capture/head.rs:416` `unborn_unsupported_non_commit_absent_and_non_repository_roots_are_unavailable` | `p2-new-head-unavailable-keeps-decision-admitted`; `p2-git-open-is-isolated-and-allocation-bounded` (refusals of the given slot count) | unaudited |
| `crates/daemon/src/memory_capture/head.rs:486` `the_observation_reads_at_most_one_object_and_neither_the_index_nor_the_tree` | `p2-new-head-observed-once-and-frozen` (no status scan, at most one object read) | unaudited |
| `crates/daemon/src/memory_capture/head.rs:517` `a_packed_head_commit_freezes_whole_or_as_a_delta_from_the_pack_files`; `:534` `a_delta_whose_base_is_in_another_pack_is_read_from_the_store` | `p2-new-head-observed-once-and-frozen` (whole, offset-delta, id-delta, and cross-pack entries) | unaudited |
| `crates/daemon/src/memory_capture/head.rs:556` `a_hinted_pack_index_is_read_before_the_directory_scan`; `:568` `a_pack_index_of_another_repository_never_answers`; `:658` `the_newest_pack_index_answers_and_a_deleted_hint_falls_back_to_the_scan` | `p2-new-head-unavailable-keeps-decision-admitted` (the pack index hint orders reads only) | unaudited |
| `crates/daemon/src/memory_capture/head.rs:595` `a_delta_chain_that_revisits_an_entry_is_unavailable_without_a_store_read`; `:609` `an_index_paired_with_another_pack_is_unavailable_without_a_store_read`; `:631` `slot_exhaustion_while_resolving_a_delta_base_reads_as_exhausted` | `p2-new-head-unavailable-keeps-decision-admitted` (a pack whose delta chain revisits an entry, an index paired with another pack's data); `p2-git-open-is-isolated-and-allocation-bounded` (slot exhaustion reported from inside a delta-base lookup) | unaudited |
| `crates/daemon/src/memory_capture/head.rs:690` `a_head_commit_in_an_alternate_freezes_when_the_store_holds_more_indices_than_its_first_slots` | `p2-git-open-is-isolated-and-allocation-bounded`; `p2-new-head-observed-once-and-frozen` (slot retry, alternates) | unaudited |
| `crates/daemon/src/memory_capture/head.rs:716` `frozen_heads_serialize_as_tagged_values_and_round_trip`; `crates/daemon/src/memory_capture.rs:1996` `a_plan_frozen_without_a_head_reads_as_head_unavailable` | same (C13 compatibility) | unaudited |
| `crates/daemon/src/lib.rs:25061` `a_submission_freezes_one_checkout_head_in_every_plan_and_a_retry_keeps_it` | `p2-new-head-observed-once-and-frozen`; `p2-new-head-unavailable-keeps-decision-admitted` | unaudited |
| `crates/daemon/src/lib.rs:25171` `a_plan_frozen_without_a_head_publishes_as_before` | `p2-new-head-observed-once-and-frozen` (C13: a plan without `head` publishes) | unaudited |
| `crates/kernel/tests/kernel_anchor_write.rs:797` `a_written_reachable_from_anchor_holds_through_descendants_and_not_after_a_rewrite` | `p2-reachable-from-holds-on-ancestor-of-head` (seam 2) | unaudited |
| `crates/daemon/src/source_capture_tests.rs:3496` `captured_memories_cite_the_blocks_holding_their_quotes_and_anchor_at_head` | `p2-new-citation-is-alias-only`; `p2-new-citation-block-target-rule`; `p2-new-anchor-and-citation-add-no-authority`; `p2-new-capture-anchor-identity-is-deterministic`; `p2-new-miss-counters-track-rationale`; `p2-new-cited-decision-pair-observed` | unaudited |
| `crates/daemon/src/source_capture_tests.rs:3646` `misses_carry_one_closed_reason_each_and_the_counters_follow_them` | `p2-new-miss-reason-matches-outcome`; `p2-new-head-unavailable-keeps-decision-admitted`; `p2-new-miss-counters-track-rationale` | unaudited |
| `crates/daemon/src/source_capture_tests.rs:3737` `an_invented_source_id_is_refused_before_any_citation_lookup` | `p2-new-citation-is-alias-only` | unaudited |
| `crates/daemon/src/source_capture_tests.rs:3783` `cited_capture_evidence_survives_expiry_and_refuses_retirement` | `p2-new-citation-folds-sensitivity-and-pins-evidence`; `p2-cited-evidence-is-never-retired-or-swept` | unaudited |
| `crates/daemon/src/source_capture_tests.rs:3822` `a_replayed_publish_adds_no_rows_and_no_counts` | `p2-new-capture-anchor-identity-is-deterministic`; `p2-new-miss-counters-track-rationale` | unaudited |
| `crates/daemon/src/memory_capture.rs:1747` `one_job_and_plan_always_write_the_same_anchor_and_decision_rows` | `p2-new-capture-anchor-identity-is-deterministic` | unaudited |
| `crates/daemon/src/memory_capture.rs:1785` `a_retry_after_a_rolled_back_publish_writes_one_anchor_and_the_same_rows` | `p2-new-capture-anchor-identity-is-deterministic` (rolled-back commit, then retry) | unaudited |
| `crates/daemon/src/memory_capture/cite.rs:472`, `:502`, `:529`, `:918`, `:932` (locate, target, hashed shape, located-at-own-bytes property, quote-join property) | `p2-new-citation-block-target-rule`; `p2-new-miss-reason-matches-outcome` | unaudited |
| `crates/daemon/src/memory_capture/cite.rs:483`, `:896` (location by byte length against a SHA-256 finalized at every `\n`: empty blocks, a block past the text, a malformed digest, random joins and edits) | `p2-new-citation-block-target-rule` | unaudited |
| `crates/daemon/src/memory_capture/cite.rs:828` `stopping_at_the_walk_cites_what_reading_every_index_cites`; `:717` `a_chunked_lookup_cites_what_reading_every_index_one_by_one_cites` | `p2-new-citation-is-alias-only`; `p2-new-citation-block-target-rule` (reading stops at the walk; chunk boundaries at indexes 4, 16, and 64 against one-by-one reads on a Kernel) | unaudited |
| `crates/kernel/tests/kernel_source_descriptors.rs:2965` `a_writer_reads_the_live_revision_of_one_lineage`; `crates/kernel/src/source_identity.rs:1041` `the_occurrence_id_agrees_with_the_full_encoding` | `p2-new-citation-is-alias-only` (the batch read returns the single read's row or error per lineage; the varying-last-field encoder returns the full encoder's lineage ids) | unaudited |
| `crates/daemon/src/source_capture/settlement.rs:312` `a_terminal_or_absent_entry_settles_without_waiting`; `:340` `a_running_entry_waits_for_its_end_or_its_budget_deadline`; `:434` `only_a_deadline_end_or_a_refusal_reads_as_budget_expired` | `p2-new-settlement-wait-is-bounded-and-gate-free`; `p2-new-lookup-runs-only-after-settlement`; `p2-new-miss-reason-matches-outcome` | unaudited |
| `crates/daemon/src/source_capture/settlement.rs:373`, `:385`, `:400`, `:420`, `:461`, `:482`, `:500`, `:518`, `:556`, `:586`, `:598`, `:886`, `:895` (refusal overwrite, released slot, bound and eviction, evicted waiter, newer acceptance, cleared record, current outcome, sessions observed together, shared waiter channel, a waiter reads the write after its observation, misnamed or evicted-acceptance end, and the two properties that check the record against a linear model of #1136's list under one publication slot) | `p2-new-lookup-runs-only-after-settlement` (record writers, bound, replay read, and one-lock observation) | unaudited |
| `crates/daemon/src/source_capture_tests.rs:3978` `a_submit_after_the_publish_ended_arms_no_wait_and_cites`; `:4008` `on_the_paused_clock_the_wait_is_zero_when_settled_and_ends_exactly_at_the_deadline` | `p2-new-settlement-wait-is-bounded-and-gate-free`; `p2-new-lookup-runs-only-after-settlement` (AC8, AC9 on the paused clock) | unaudited |
| `crates/daemon/src/source_capture_tests.rs:4093` `a_submit_during_the_publish_waits_outside_the_gate_and_then_cites`; `:4134` `an_opencode_submit_during_the_publish_waits_and_then_cites`; `:4158` `a_submit_on_another_session_waits_on_each_job_session_it_carries` | `p2-new-settlement-wait-is-bounded-and-gate-free`; `p2-new-lookup-runs-only-after-settlement` | unaudited |
| `crates/daemon/src/source_capture_tests.rs:4205` `a_publish_past_its_budget_releases_the_submit_at_the_deadline` | `p2-new-settlement-wait-is-bounded-and-gate-free`; `p2-new-miss-reason-matches-outcome`; `p2-new-cited-decision-pair-observed` (AC9) | unaudited |
| `crates/daemon/src/source_capture_tests.rs:4259` `a_queue_full_refusal_settles_at_once_and_hashed_ids_skip_the_wait`; `:4315` `consent_off_and_error_submits_arm_no_wait_while_a_publish_runs` | `p2-new-miss-reason-matches-outcome`; `p2-new-cited-decision-pair-observed` (AC10, disabled, hashed) | unaudited |
| `crates/daemon/src/source_capture_tests.rs:4353` `the_settlement_is_written_after_the_response_and_a_shutdown_refusal_records_it`; `:4402` `a_cancel_or_a_dropped_request_during_the_wait_writes_no_decision_and_releases_the_lease`; `:4448` `every_job_end_leaves_an_ended_settlement_and_an_idle_slot` | `p2-new-lookup-runs-only-after-settlement` (AC10, writer coverage) | unaudited |
| `crates/daemon/src/source_capture_tests.rs:4483` `a_completed_publish_with_a_deferred_unit_reports_descriptor_not_found` | `p2-new-lookup-runs-only-after-settlement`; `p2-new-miss-reason-matches-outcome`; `p2-deferred-source-units-publish-on-a-later-pass` | unaudited |
| `crates/host-runtime/src/control.rs:1960` `the_wire_document_names_exactly_the_forwarded_fields` | `p2-new-miss-counters-track-rationale` (wire names) | unaudited |
| `crates/kernel/tests/kernel_anchor_write.rs:311` `every_anchor_kind_commits_a_row_the_engine_decodes_and_a_decision_can_reference` | `p2-new-anchor-row-decodes-on-write`; `p2-new-anchor-and-registry-share-one-envelope` | unaudited |
| `crates/kernel/tests/kernel_anchor_write.rs:383` `git_anchors_with_an_empty_capture_list_store_a_null_payload` | `p2-new-anchor-row-decodes-on-write` | unaudited |
| `crates/kernel/tests/kernel_anchor_write.rs:459` `invalid_anchors_are_refused_as_invalid_input_with_a_typed_cause` | same | unaudited |
| `crates/kernel/tests/kernel_anchor_write.rs:684` `a_raw_row_with_an_unowned_column_is_conflicting_columns` | same | unaudited |
| `crates/kernel/tests/kernel_anchor_write.rs:434` `a_decision_failure_after_the_anchor_rolls_back_both` | `p2-new-anchor-and-registry-share-one-envelope` | unaudited |
| `crates/kernel/tests/kernel_anchor_write.rs:711` `a_missing_domain_is_not_found` | same | unaudited |
| `crates/kernel/tests/kernel_anchor_write.rs:732` `a_duplicate_anchor_id_is_a_conflict_within_and_across_commits` | same; Kernel half of `p2-new-capture-anchor-identity-is-deterministic` | unaudited |
| `crates/daemon/tests/claim_sources.rs:1508` `anchor_commits_pass_through_the_claim_materializer` | `p2-new-anchor-and-registry-share-one-envelope` (consumer side) | unaudited |

## Empty categories

- Property-based or model-checked tests over anchor rows: none found.
- Production assertions on the anchor writer beyond typed refusals: none
  found; the writer returns `InvalidInput`, `NotFound`, or `Conflict`.

## Suspiciously quiet areas

- No production path reads `anchors` rows into applicability candidates, so
  every S1 check runs on fixtures (specification K3).
- The capture prompt rule at `crates/daemon/src/memory_capture.rs:134` is the
  only statement of the quote contract the model sees; no test pins the
  prompt text to the validator.
- No test publishes one message id in two sessions to show the citation
  lookup keeps them apart; the lineage encoding includes the session id.
