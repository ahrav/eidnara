# Source capture: records lens, group g2

Verified against HEAD `5a842d998` (branch
`source-capture/1090-property-catalog`). Records follow `../../../METHOD.md`.
Citations name functions, types, constants, and tests.

### sc-sensitive-denied-remote-hidden-from-auto-surfaces

Type: safety
Reachability: default-production - `Sensitivity::denies_destination`, `eligibility_for`, `surface_visibility`, and `judge` run on every artifact eligibility read, visible-surface read, and `judge_eligibility` call, and `crates/daemon/src/lib.rs` dispatches `"kernel.egress.decide"` to `handle_kernel_egress_decide` with no gate. Captured `Sensitive` objects themselves exist only with `source_capture.enabled` (`DaemonConfig::default` sets `source_capture_enabled: false`).
Status: active
Exercised: yes - `kernel_cas.rs` `unproven_normal_and_non_utf8_are_clamped_sensitive` constructs unprovenanced and non-UTF-8 artifacts and reads both destinations; `admission.rs` unit test `visibility_matrix_and_sensitivity_ceiling_are_total` walks every row and surface; `kernel_admission.rs` `dispositions_and_sensitivity_only_reduce_visibility` reads `AutoSearch` and `ExplicitSearch`; `kernel_eligibility.rs` `every_verdict_class_and_precedence_pair_matches_the_expected_table` judges `secret` and `sensitive` candidates at both destinations; `kernel_routes.rs` `egress_gate_judges_the_owner_by_its_served_class` drives the route for a tainted owner; `source_capture_tests.rs` `captured_units_create_no_decision_and_stay_hidden_from_auto_inject_after_reopen` reads real captures through `AutoInject` after a reopen.
Guarantee: `Sensitive` is denied the `Remote` destination and allowed `Local`, `Secret` is denied both, and `Sensitive` is hidden from `AutoInject` and `AutoSearch` at every policy layer.
Check: `always` - `Sensitive.denies_destination(Remote) == Some(SensitiveRemote)` and `(Local) == None`; `Secret.denies_destination(_) == Some(Secret)`; `surface_visibility(row, AutoInject|AutoSearch, Sensitive) == Hidden` for every row; `judge` with served `Sensitive` at `Remote` returns `ProviderSensitive`; the egress route turns `Allowed` into `Refused(OwnerSensitive)` for a served `Secret` owner, or a non-`Normal` served owner at `Remote`, with zero provider requests. `always` because these are pure policy functions evaluated on every read.
Fault/timing angle: the egress route reads artifact class and owner citation in one `egress_candidates` snapshot, so a reclassifying ingest cannot land between them.
Required faults and enabling state: a `Sensitive` object and artifact; a `Remote` read (F21); `AutoInject` and `AutoSearch` reads.
Confidence: high - [evidence](evidence/sc-sensitive-denied-remote-hidden-from-auto-surfaces.md). Read every function above and the six tests at HEAD, including the eligibility table the source had left unread.
Existing check: `kernel_cas.rs` `unproven_normal_and_non_utf8_are_clamped_sensitive`; `admission.rs` `visibility_matrix_and_sensitivity_ceiling_are_total`; `kernel_admission.rs` `dispositions_and_sensitivity_only_reduce_visibility`; `kernel_eligibility.rs` `every_verdict_class_and_precedence_pair_matches_the_expected_table`; `kernel_routes.rs` `egress_gate_judges_the_owner_by_its_served_class`; `source_capture_tests.rs` `captured_units_create_no_decision_and_stay_hidden_from_auto_inject_after_reopen`; all `unaudited`.
Impact: captured exact bytes reach a remote model or an automatic surface, which violates R13 and the #708 disclosure stop condition.
Open questions:
- May a future read tool disclose captured tool errors to the model, and under which gate (OQ2)? (needs human input)

### sc-retire-evidence-conflicts-while-cited

Type: safety
Reachability: default-production - `memory_reviewer::lifecycle::kernel_slice` calls `expire_local_file_captures` on every lifecycle pass while the Kernel is ready, and its `retire_expired_capture` calls `retire_evidence`; `lifecycle::run` is spawned from `crates/daemon/src/lib.rs` once the Memory Store opens. The harness publisher's compensating `SourcePublisher::retire_evidence` and the harness sweep reach it only with `source_capture.enabled`.
Status: active
Exercised: yes - `kernel_source_descriptors.rs` `retiring_evidence_invalidates_uncited_evidence_and_refuses_cited_evidence` retains an uncited row and a row a live descriptor cites, retires each, and asserts the outcome, tip, and liveness; `harness_sources.rs` `permanent_refusal_reports_failed_retirement_attempt` injects a retirement failure through a trigger.
Guarantee: `Envelope::retire_evidence` refuses with `Conflict` while any live observation, decision, decision event of a live decision, or live asserted edge cites the evidence, the commit rolls back, and capture pins are not consulted.
Check: `always` - retiring cited evidence returns `Err(Conflict)` with `tip()` unchanged, the citing descriptor live, and `invalidated_commit_seq == None`; retiring uncited evidence returns `Ok`, sets `invalidated_commit_seq` to the commit, and records `evidence_retire`; a second retirement is `NotFound`. `always` because the predicate `EVIDENCE_CITED_SQL` is evaluated on every call.
Fault/timing angle: check and invalidation share one writer transaction. `SourcePublisher::retire_evidence` treats `NotFound` on an already invalidated object as retired, so a replayed compensation is idempotent.
Required faults and enabling state: a live citing descriptor, then its retirement (F13); a pinned but uncited evidence row for the pin half.
Confidence: high - [evidence](evidence/sc-retire-evidence-conflicts-while-cited.md). Read `retire_evidence_inner`, `EVIDENCE_CITED_SQL`, the sweep's `cited_elsewhere`, the publisher wrapper, and both tests. The pin half rests on code reading.
Existing check: `kernel_source_descriptors.rs` `retiring_evidence_invalidates_uncited_evidence_and_refuses_cited_evidence`; `harness_sources.rs` `permanent_refusal_reports_failed_retirement_attempt`; `harness_sources.rs` `harness_capture_expiry_keeps_held_and_cited_evidence_and_drains_in_pages` (cited arm); all `unaudited`.
Impact: KTD6's "cites or holds" is citations only; every expiry sweep carries its own pin predicate (D6, C11). A regression would let expiry remove independently cited evidence.
Open questions:
- Should a test retire pinned, uncited evidence directly to pin down that `retire_evidence` ignores holds? (unresolved, needs a Kernel test)

### sc-curator-capture-requires-finite-live-deadline

Type: safety
Reachability: default-production - the checks sit in Kernel ingest (`cas/ingest.rs` request validation and `insert_reference`), reached by every `curator_capture` ingest a MemoryReviewer run issues; the `harness_capture` half is reached only with `source_capture.enabled`.
Status: active
Exercised: yes - `kernel_memory_reviewer_holds.rs` `memory_reviewer_captures_must_carry_a_finite_acquisition_reference` ingests with `None`, `now - 1`, and `now + 1 h`; `a_committed_capture_replays_after_its_retention_deadline` ingests with `now + 200 ms`, sleeps 300 ms, and replays; `local_file.rs` unit test `a_harness_capture_pinned_after_it_was_selected_for_expiry_is_kept` refuses an undated `harness_capture` ingest.
Guarantee: An ingest with `retention_class == "curator_capture"` and `retain_until: None` is refused before any byte is retained, a new reference whose deadline is at or before the store clock is refused at reference commit, and an identical committed request replays after its deadline.
Check: `always` - `ingest_artifact(curator_capture, None)` returns `ArtifactErrorKind::InvalidInput`; `Some(now - 1)` returns `InvalidInput` from `insert_reference`; a committed request replayed after its deadline returns the first handle; `harness_capture` with `None` returns `InvalidInput` with `tip()` unchanged. `always` because validation runs on every ingest.
Fault/timing angle: deadline liveness reads `crate::current_time_ms()` inside the reference commit, so the replay test depends on a 200 ms wall-clock window.
Required faults and enabling state: `None` and past deadlines; a committed request replayed after expiry (F11).
Confidence: high - [evidence](evidence/sc-curator-capture-requires-finite-live-deadline.md). Read both ingest sites and three tests; confirmed with `git grep` that the replay test existed at baseline.
Existing check: `kernel_memory_reviewer_holds.rs` `memory_reviewer_captures_must_carry_a_finite_acquisition_reference`; `kernel_memory_reviewer_holds.rs` `a_committed_capture_replays_after_its_retention_deadline`; `local_file.rs` `a_harness_capture_pinned_after_it_was_selected_for_expiry_is_kept` (undated arm); all `unaudited`.
Impact: a capture without a finite live deadline never enters expiry and holds bytes indefinitely. `harness_capture` mirrors only the finite-deadline half; a past harness deadline commits and is due at the next sweep.
Open questions:
- Should ingest liveness use an injected clock so the replay test needs no sleep? (unresolved, needs a `/testing:test-strategy` decision)

### sc-capture-expiry-class-scoped-paged-citation-safe

Type: safety
Reachability: default-production - `kernel_slice` runs `expire_local_file_captures` then `expire_harness_captures` before `run_staging_maintenance` on every lifecycle pass while the Kernel is ready; `lifecycle::run` is spawned with no gate once the Memory Store opens. Harness rows exist only with `source_capture.enabled`.
Status: active
Exercised: yes - `local_file.rs` unit tests construct a pin taken after selection, a seated receipt under the reserved producer, a replayed expiry receipt, a purge-degraded pin, 512 retired rows of history, and a harness row beside a curator row; `memory_reviewer_project_text.rs` `expiry_retires_the_capture_and_its_detail_but_keeps_independent_support`, `retained_captures_do_not_starve_newer_expired_captures` (65 captures, 64 cited), and `a_capture_the_store_refuses_to_retire_does_not_stall_the_sweep` (64 non-evidence rows ahead of one capture) drive the public sweep.
Guarantee: One `expire_local_file_captures` call retires at most 64 expired, unpinned `curator_capture` captures, oldest `retain_until` first, each in its own reserved-producer commit, keeps evidence another live row cites, and never touches another retention class.
Check: `always` - after one call with N expired unpinned captures, `retired + retained <= min(N, 64)`; every retired row had the class and `retain_until <= now` at its commit; pinned or future-deadline captures are unchanged; evidence cited elsewhere stays live and counts as `retained`, then leaves the candidate set; `canonical` and `harness_capture` rows are unchanged. Across calls, N expired unpinned uncited captures clear within `ceil(N / 64)` calls (the liveness half `sc-expiry-drains-within-page-bound` generalizes). `always` because every sweep call must satisfy it.
Fault/timing angle: a pin acquired between the reader-side `expired_captures` query and the writer-side `EXPIRED_UNPINNED_SQL` recheck keeps the capture as `Conflict`; a refused row is skipped and the page continues.
Required faults and enabling state: more than 64 expired captures (F14); a pin mid-sweep (F12); evidence cited independently (F13); other classes in the same store. Coverage witnesses `expired_capture_pinned_mid_sweep` and `expired_capture_cited_elsewhere` mark the F12 and F13 preconditions.
Confidence: high - [evidence](evidence/sc-capture-expiry-class-scoped-paged-citation-safe.md). Read `expire_captures`, `retire_expired_capture`, `EXPIRED_UNPINNED_SQL`, `expired_captures_sql`, `kernel_slice`, and nine tests.
Existing check: `local_file.rs` `a_caller_cannot_seat_a_receipt_the_expiry_would_replay`, `a_replayed_expiry_receipt_is_not_counted_as_a_retirement`, `a_capture_pinned_after_it_was_selected_for_expiry_is_kept`, `a_harness_capture_pinned_after_it_was_selected_for_expiry_is_kept`, `a_purge_degraded_pin_does_not_keep_a_lapsed_capture`, `expiry_candidate_cost_does_not_grow_with_retired_history`; `memory_reviewer_project_text.rs` `expiry_retires_the_capture_and_its_detail_but_keeps_independent_support`, `retained_captures_do_not_starve_newer_expired_captures`, `a_capture_the_store_refuses_to_retire_does_not_stall_the_sweep`; all `unaudited`.
Impact: U4 reuses this mechanism through `ExpiringClass`, so a defect here reaches both classes. The sweep runs as a sibling of `run_staging_maintenance` in `kernel_slice`; a failing curator sweep skips the harness sweep and staging maintenance for that pass.
Open questions: None.

### sc-live-evidence-reference-blocks-artifact-gc

Type: safety
Reachability: default-production - `run_staging_maintenance` runs `run_artifact_gc` from `kernel_slice` on every lifecycle pass while the Kernel is ready.
Status: active
Exercised: partial - `kernel_gc.rs` covers a live `canonical` row at `now = 13 * DAY_MS`, invalidation grace, the `retain_until` floor, overflow at `i64::MAX`, pin and pin-release grace, pin expiry, and the pending-purge override; no test sweeps a live row at a far-future `now`.
Guarantee: Outside a pending purge, GC never reclaims a digest while any `evidence_meta` row with that digest is live or any unreleased pin reference names it, and after invalidation keeps the bytes until the latest of `MAX(retain_until)`, `invalidated_at + 14 days`, and `pin_released_at + 14 days`.
Check: `always` - for every candidate without a pending unlink, a live evidence row or an unreleased pin reference yields `Withheld::Retained` at any `now`; an invalidated digest is reclaimed only when `now >= deadline`; an overflowing deadline retains. `always` because every GC candidate is judged by `prepare_reclaim`.
Fault/timing angle: a `Reclaiming` reservation carries the decision across a crash; a current-epoch `Live` reservation with an unexpired lease retains; a pending unlink bypasses every retention check by design.
Required faults and enabling state: harness or canonical evidence kept live while `now` advances years (F15); invalidated evidence inside and beyond 14 days; a held and a released pin; a pending unlink over a live row.
Confidence: high - [evidence](evidence/sc-live-evidence-reference-blocks-artifact-gc.md). Read `prepare_reclaim`, `reclaim_candidate`, and seven GC tests; the far-future live-row case rests on code reading because the live check precedes every clock comparison.
Existing check: `kernel_gc.rs` `referenced_and_recently_invalidated_artifacts_survive`, `invalidated_artifact_reclaims_after_grace_but_backward_clock_keeps_it`, `retain_until_is_an_absolute_floor_not_an_extra_grace_start`, `overflowing_grace_deadline_keeps_artifact`, `active_capture_pin_and_pin_release_grace_protect_artifact`, `maintenance_preserves_only_the_grace_remaining_after_pin_expiry`, `pending_purge_unlinks_a_live_reference_and_sweeps_its_digest_temps`; `harness_sources.rs` `harness_capture_evidence_expires_at_its_deadline_and_replay_never_renews_it` (expiry sweep at `NOW + 32 d`, not GC); all `unaudited`.
Impact: a live descriptor loses its exact bytes, or captured bytes never leave the store; physical retention exceeds `retain_until` by up to the 14-day grace (R11, C11).
Open questions:
- Which existing GC test drives a `canonical` row through a far-future sweep? (unresolved, needs one `kernel_gc` test at a far-future `now` over a live row; none exists at HEAD)

### sc-occurrence-class-set-frozen-across-mirrors

Type: safety
Reachability: default-production - `encode_preserving_span` runs on every descriptor publication and projection rebuild, and the retrieval CHECK applies to every `occurrences` insert.
Status: active
Exercised: yes - `eval_identity.rs` `the_core_encoder_reproduces_every_identity_golden_and_the_kernel_encoder` compares the mirrors and encodes 24 fixture records through both encoders; `schema_inventory.rs` `the_check_vocabularies_equal_the_rust_enums` compares the CHECK to `OccurrenceClass::ALL`; `kernel_source_descriptors.rs` `the_kernel_encoder_matches_golden_identifiers_and_string_payload_refusals` covers `unknown_class` and `precedence_class_before_everything`.
Guarantee: The six-class identity contract (codes and order, field orders, representations, harnesses, object formats, the 512-byte cap, and encoding version 2) is identical in the Kernel, eval-core, the retrieval `occurrences.class` CHECK, and the identity fixtures, and any other class code is refused first.
Check: `always` - the eval-core and Kernel constants and per-class fields are equal; the CHECK equals `OccurrenceClass::ALL` codes; all 24 fixture records encode to their pinned occurrence and lineage ids through both encoders; an unknown class returns `Err(UnknownClass)` before any other refusal. `always` because the contract is static.
Fault/timing angle: none.
Required faults and enabling state: the four mirrors; a class name outside the set.
Confidence: high - [evidence](evidence/sc-occurrence-class-set-frozen-across-mirrors.md). Read `source_identity.rs`, `eval-core/src/occurrence.rs`, `baseline.sql`, and the three tests at HEAD.
Existing check: `eval_identity.rs` `the_core_encoder_reproduces_every_identity_golden_and_the_kernel_encoder`; `schema_inventory.rs` `the_check_vocabularies_equal_the_rust_enums`; `kernel_source_descriptors.rs` `the_kernel_encoder_matches_golden_identifiers_and_string_payload_refusals`; all `unaudited`.
Impact: two components derive different occurrence ids for one source. U9 grew the set to six and bumped the projection schema to 8; a binary below 8 cannot re-encode `repository_documents`.
Open questions:
- Is `RepositoryDocuments` in scope (OQ1)? Resolved for landing in U9: ticket-1081 names OQ1 as an owner gate and lists no blocker, and the class landed dark. Neither file records a written owner confirmation.

### sc-descriptor-revision-exceeds-live-predecessor-only

Type: safety
Reachability: default-production - `Envelope::publish_verified` runs for every source descriptor publication through `publish_source_descriptors`, including the git and claim publishers; harness classes reach it only with `source_capture.enabled`.
Status: active
Exercised: yes - `kernel_source_descriptors.rs` `a_successor_invalidates_exactly_its_predecessor_in_one_commit_and_replays_from_its_receipt`, `representations_and_spans_of_one_source_are_separate_lineages`, and `concurrent_successors_serialize_into_one_chain` publish advancing and stale revisions; `retirement_preserves_domain_ownership_without_reusing_old_revision_ids` retires revision 3 and republishes 2 for `messages`.
Guarantee: For every class except `repository_documents`, a revision at or below the live descriptor's in its lineage is refused `RevisionNotAdvanced`, a higher revision invalidates the live row, sets its successor, and returns `replaced_object_id` in one commit, and with no live row a revision below a retired one starts a fresh chain.
Check: `always` - with live revision r: r' <= r returns `Err(RevisionNotAdvanced)` with `tip()` unchanged; r' > r returns `Ok` with `replaced_object_id == Some(live)`, the predecessor's `superseded_by` set, and both changes at one `commit_seq`; with the lineage retired at 3, revision 2 returns `Ok` with `replaced_object_id == None` and revision 3 returns `Kernel(Conflict)`. `always` because every publication passes the check.
Fault/timing angle: two live rows per lineage report `CorruptCanonicalRow`; racing successors serialize into one chain.
Required faults and enabling state: two revisions in either order (F9); a retired lineage followed by a lower revision (F10). Coverage witness `retired_lineage_republished` marks F10 for the guarded class.
Confidence: high - [evidence](evidence/sc-descriptor-revision-exceeds-live-predecessor-only.md). Read `publish_verified`, `live_lineage`, `historical_head`, and five tests; confirmed the retired-lineage test existed at baseline.
Existing check: `kernel_source_descriptors.rs` `a_successor_invalidates_exactly_its_predecessor_in_one_commit_and_replays_from_its_receipt`, `representations_and_spans_of_one_source_are_separate_lineages`, `concurrent_successors_serialize_into_one_chain`, `retirement_preserves_domain_ownership_without_reusing_old_revision_ids`, `repository_document_revisions_exceed_the_historical_head_including_retired_rows` (guarded class); `harness_sources.rs` `equal_text_stays_distinct_and_revisions_replay_or_succeed_atomically`, `permanent_refusal_reports_failed_retirement_attempt`; all `unaudited`.
Impact: a stale revision replaces a newer live one. U9 adds the historical-head guard for `repository_documents` only (`sc-repodoc-one-live-revision-above-historical-head`); every other class keeps this rule.
Open questions:
- Does publishing revision 2 after retiring revision 3 succeed at baseline? Resolved: yes for `messages`, asserted at baseline by `retirement_preserves_domain_ownership_without_reusing_old_revision_ids`.
- Should the historical-head guard apply to every class (OQ13)? Resolved in U9: `repository_documents` only; other classes must pass `expected_head: None`.

### sc-readmission-blocked-after-purge-tombstone

Type: safety
Reachability: default-production - `artifact_is_blocked` runs in every exact and batch ingest; tombstones come only from `delete_artifact` with `ArtifactDeletionKind::Purge`, an operator action. Harness re-observation reaches it only with `source_capture.enabled`.
Status: active
Exercised: yes - `kernel_deletion.rs` `purge_tombstones_unlinks_degrades_pins_and_is_idempotent` purges a digest and re-ingests its bytes under a new key; `purge_intent_failure_has_no_commit_and_commit_before_unlink_recovers_on_reopen` re-ingests inside the pending-unlink window; `kernel_cas.rs` `eligibility_matrix_includes_secret_unknown_and_tombstone` seeds a tombstone and re-ingests. No test combines a purge with a finite retention class, and none re-ingests after ordinary GC.
Guarantee: Exact ingest refuses `ReAdmissionBlocked` for a digest in `artifact_purge_tombstones` (permanent) or `artifact_pending_unlinks`, and `PublishError::is_permanent` returns `true` for that refusal, so the capture memo marks the unit terminal.
Check: `always` - ingest of a tombstoned or pending-unlink digest under a new key returns `ReAdmissionBlocked` and retains nothing; `is_permanent(Artifact(ReAdmissionBlocked)) == true`; `classify` maps it to `Disposition::Terminal`. `always` because the check runs on every new reference.
Fault/timing angle: a purge commits tombstone and pending unlink together, and GC later clears only the pending row. Ordinary GC of expired capture bytes writes neither row, so identical bytes re-observed under a new key after expiry and reclaim are admitted by code reading.
Required faults and enabling state: an operator purge of a captured digest, then identical bytes under a new unit key (F24); an expired capture reclaimed by ordinary GC, then identical bytes under a new key (F27).
Confidence: medium - [evidence](evidence/sc-readmission-blocked-after-purge-tombstone.md). Read `artifact_is_blocked`, both tombstone and pending-unlink insert sites, the immutability triggers, `is_permanent`, `classify`, and four tests; the F27 reading has no test.
Existing check: `kernel_deletion.rs` `purge_tombstones_unlinks_degrades_pins_and_is_idempotent`, `purge_intent_failure_has_no_commit_and_commit_before_unlink_recovers_on_reopen`; `kernel_cas.rs` `eligibility_matrix_includes_secret_unknown_and_tombstone`; `kernel_backup.rs` `a_backup_that_does_not_carry_a_purge_the_target_committed_cannot_be_restored`; all `unaudited`.
Impact: under an operator purge, identical capture bytes stay refused permanently. PR #1084 states expiry-driven reclaim tombstones the digest (the OQ21 loss condition). Code shows only an operator purge does.
Open questions:
- Is `ReAdmissionBlocked` permanent by intent for the pending-unlink arm (OQ20)? Resolved in U2 (ticket-1082): terminal for the process lifetime. The ticket expects a cleared pending unlink to admit after restart; by code reading every pending unlink carries a permanent tombstone, so it does not.
- Should `harness_capture` purges leave the digest re-admissible (OQ21)? (needs human input; the premise in pr-1084 that expiry tombstones disagrees with code, unresolved, needs an F27 test)

## Existing checks

| Check | Covers | Status |
| --- | --- | --- |
| `crates/kernel/tests/kernel_cas.rs` `unproven_normal_and_non_utf8_are_clamped_sensitive` | `Sensitive` artifact denied `Remote`, allowed `Local` | unaudited |
| `crates/kernel/src/admission.rs` `visibility_matrix_and_sensitivity_ceiling_are_total` | `surface_visibility` ceiling for `Secret` and `Sensitive` | unaudited |
| `crates/kernel/tests/kernel_admission.rs` `dispositions_and_sensitivity_only_reduce_visibility` | `AutoSearch` hides `Sensitive`; `ExplicitSearch` shows it | unaudited |
| `crates/kernel/tests/kernel_eligibility.rs` `every_verdict_class_and_precedence_pair_matches_the_expected_table` | `ProviderSensitive` by destination | unaudited |
| `crates/daemon/tests/kernel_routes.rs` `egress_gate_judges_the_owner_by_its_served_class` | `OwnerSensitive` refusal, zero provider requests | unaudited |
| `crates/daemon/src/source_capture_tests.rs` `captured_units_create_no_decision_and_stay_hidden_from_auto_inject_after_reopen` | real captures hidden from `AutoInject` after reopen | unaudited |
| `crates/kernel/tests/kernel_source_descriptors.rs` `retiring_evidence_invalidates_uncited_evidence_and_refuses_cited_evidence` | `retire_evidence` conflict and rollback | unaudited |
| `crates/daemon/tests/harness_sources.rs` `permanent_refusal_reports_failed_retirement_attempt` | failed compensating retirement is reported | unaudited |
| `crates/daemon/tests/harness_sources.rs` `harness_capture_expiry_keeps_held_and_cited_evidence_and_drains_in_pages` | cited evidence kept by the harness sweep | unaudited |
| `crates/daemon/tests/harness_sources.rs` `harness_capture_evidence_expires_at_its_deadline_and_replay_never_renews_it` | canonical rows untouched by the harness sweep at `NOW + 32 d` | unaudited |
| `crates/kernel/tests/kernel_memory_reviewer_holds.rs` `memory_reviewer_captures_must_carry_a_finite_acquisition_reference` | `None` and past deadline refused | unaudited |
| `crates/kernel/tests/kernel_memory_reviewer_holds.rs` `a_committed_capture_replays_after_its_retention_deadline` | replay after deadline (wall clock) | unaudited |
| `crates/kernel/src/local_file.rs` `a_caller_cannot_seat_a_receipt_the_expiry_would_replay` | reserved expiry producer | unaudited |
| `crates/kernel/src/local_file.rs` `a_replayed_expiry_receipt_is_not_counted_as_a_retirement` | replay not counted | unaudited |
| `crates/kernel/src/local_file.rs` `a_capture_pinned_after_it_was_selected_for_expiry_is_kept` | F12 for `curator_capture` | unaudited |
| `crates/kernel/src/local_file.rs` `a_harness_capture_pinned_after_it_was_selected_for_expiry_is_kept` | F12 for `harness_capture`; class scoping; undated refusal | unaudited |
| `crates/kernel/src/local_file.rs` `a_purge_degraded_pin_does_not_keep_a_lapsed_capture` | degraded pin is not a live pin | unaudited |
| `crates/kernel/src/local_file.rs` `expiry_candidate_cost_does_not_grow_with_retired_history` | candidate query cost bound | unaudited |
| `crates/daemon/tests/memory_reviewer_project_text.rs` `expiry_retires_the_capture_and_its_detail_but_keeps_independent_support` | hold, citation, canonical bytes | unaudited |
| `crates/daemon/tests/memory_reviewer_project_text.rs` `retained_captures_do_not_starve_newer_expired_captures` | page composition with 64 retained | unaudited |
| `crates/daemon/tests/memory_reviewer_project_text.rs` `a_capture_the_store_refuses_to_retire_does_not_stall_the_sweep` | non-evidence rows excluded | unaudited |
| `crates/kernel/tests/kernel_gc.rs` `referenced_and_recently_invalidated_artifacts_survive` | live and recently invalidated rows retained | unaudited |
| `crates/kernel/tests/kernel_gc.rs` `invalidated_artifact_reclaims_after_grace_but_backward_clock_keeps_it` | 14-day grace | unaudited |
| `crates/kernel/tests/kernel_gc.rs` `retain_until_is_an_absolute_floor_not_an_extra_grace_start` | `retain_until` floor | unaudited |
| `crates/kernel/tests/kernel_gc.rs` `overflowing_grace_deadline_keeps_artifact` | overflow retains | unaudited |
| `crates/kernel/tests/kernel_gc.rs` `active_capture_pin_and_pin_release_grace_protect_artifact` | pin and release grace | unaudited |
| `crates/kernel/tests/kernel_gc.rs` `maintenance_preserves_only_the_grace_remaining_after_pin_expiry` | expired pin reaped, grace from release | unaudited |
| `crates/kernel/tests/kernel_gc.rs` `pending_purge_unlinks_a_live_reference_and_sweeps_its_digest_temps` | purge override of live reference | unaudited |
| `crates/kernel/tests/eval_identity.rs` `the_core_encoder_reproduces_every_identity_golden_and_the_kernel_encoder` | mirror equality, 24 goldens | unaudited |
| `crates/retrieval/tests/schema_inventory.rs` `the_check_vocabularies_equal_the_rust_enums` | CHECK equals `OccurrenceClass::ALL` | unaudited |
| `crates/kernel/tests/kernel_source_descriptors.rs` `the_kernel_encoder_matches_golden_identifiers_and_string_payload_refusals` | unknown class refused first | unaudited |
| `crates/kernel/tests/kernel_source_descriptors.rs` `a_successor_invalidates_exactly_its_predecessor_in_one_commit_and_replays_from_its_receipt` | one-commit supersession | unaudited |
| `crates/kernel/tests/kernel_source_descriptors.rs` `representations_and_spans_of_one_source_are_separate_lineages` | `RevisionNotAdvanced` | unaudited |
| `crates/kernel/tests/kernel_source_descriptors.rs` `concurrent_successors_serialize_into_one_chain` | racing successors | unaudited |
| `crates/kernel/tests/kernel_source_descriptors.rs` `retirement_preserves_domain_ownership_without_reusing_old_revision_ids` | F10 for `messages` | unaudited |
| `crates/kernel/tests/kernel_source_descriptors.rs` `repository_document_revisions_exceed_the_historical_head_including_retired_rows` | F10 for the guarded class; witness `retired_lineage_republished` | unaudited |
| `crates/daemon/tests/harness_sources.rs` `equal_text_stays_distinct_and_revisions_replay_or_succeed_atomically` | publisher surfaces `RevisionNotAdvanced` | unaudited |
| `crates/kernel/tests/kernel_deletion.rs` `purge_tombstones_unlinks_degrades_pins_and_is_idempotent` | purge blocks re-admission | unaudited |
| `crates/kernel/tests/kernel_deletion.rs` `purge_intent_failure_has_no_commit_and_commit_before_unlink_recovers_on_reopen` | blocked inside the pending-unlink window | unaudited |
| `crates/kernel/tests/kernel_cas.rs` `eligibility_matrix_includes_secret_unknown_and_tombstone` | tombstone blocks ingest and eligibility | unaudited |
| `crates/kernel/tests/kernel_backup.rs` `a_backup_that_does_not_carry_a_purge_the_target_committed_cannot_be_restored` | restore keeps the purge | unaudited |

None found: a GC sweep at a far-future `now` over a live row; a direct
`retire_evidence` on pinned, uncited evidence; re-ingest of identical bytes
after ordinary GC reclaimed an expired capture (F27); a pending unlink with no
tombstone.

## Fault map rows

| fault id | injection at HEAD | availability | records |
| --- | --- | --- | --- |
| F9 revision regression, same lineage | publish two revisions in either order | available | sc-descriptor-revision-exceeds-live-predecessor-only |
| F10 retired lineage then lower revision | `retire_observation`, republish lower (`retirement_preserves_domain_ownership_without_reusing_old_revision_ids`) | available, tested | sc-descriptor-revision-exceeds-live-predecessor-only |
| F11 `retain_until` versus clock | `now_ms() - 1` at ingest; sweep `now` argument | available; ingest reads the wall clock | sc-curator-capture-requires-finite-live-deadline |
| F12 pin acquired between select and commit | `acquire_execution_hold` or raw `capture_pins` insert before `retire_expired_capture` | available, tested; witness `expired_capture_pinned_mid_sweep` | sc-capture-expiry-class-scoped-paged-citation-safe |
| F13 evidence cited by an independent row | `insert_observation` citing the evidence (`note_observation`, `cite`) | available, tested; witness `expired_capture_cited_elsewhere` | sc-retire-evidence-conflicts-while-cited; sc-capture-expiry-class-scoped-paged-citation-safe |
| F14 more than 64 expired captures | 65 captures in `retained_captures_do_not_starve_newer_expired_captures` | available, tested | sc-capture-expiry-class-scoped-paged-citation-safe |
| F15 far-future GC sweep | `run_staging_maintenance(far future)` over a live row | available, no test | sc-live-evidence-reference-blocks-artifact-gc |
| F21 remote disclosure request | `ArtifactDestination::Remote`; route `"remote"` | available, tested | sc-sensitive-denied-remote-hidden-from-auto-surfaces |
| F24 digest purged then re-observed | `delete_artifact(Purge)` then ingest under a new key | available, tested without a finite class | sc-readmission-blocked-after-purge-tombstone |
| F27 expired capture reclaimed by ordinary GC, identical bytes re-observed under a new key | `expire_harness_captures`, `run_staging_maintenance(retain_until + 14 d)`, then publish a new unit with equal bytes | constructible, no test | sc-readmission-blocked-after-purge-tombstone |

F27 is new. F24 conflates operator purge with expiry. The code separates
them: only `ArtifactDeletionKind::Purge` writes a tombstone, so the
expiry-then-reclaim path needs its own fault to test the OQ21 loss
condition.

## Notes for the lead

- Contract versus code, OQ21: pr-1084 "Recorded conditions" and ticket-1084
  state that when an expired capture's artifact is purged, the purge
  tombstones the digest. At HEAD the only tombstone insert is in
  `cas/deletion.rs` under `ArtifactDeletionKind::Purge`. Expiry
  (`local_file.rs` `expire_captures`) and GC (`gc.rs` `reclaim_candidate`)
  write no tombstone. By code reading, identical bytes under a new unit key
  are admitted after ordinary reclaim. `docs/source-capture.md` must not
  state the pr-1084 wording without an F27 test.
- Contract versus code, OQ20: ticket-1082 expects "a cleared pending-unlink
  then admits" after restart. Every pending-unlink row is inserted with a
  tombstone (`deletion.rs`) or re-armed from one (`gc.rs`
  `prepare_startup_cas_recovery`), and tombstones cannot be deleted
  (`artifact_purge_tombstones_no_delete`). The pending-unlink arm therefore
  never clears into admissibility.
- Contract versus code: spec Part A said U4 "mirrors" the curator deadline
  rule. `harness_capture` shares only the `None` refusal; the live-deadline
  check in `insert_reference` names `curator_capture` only. A past harness
  deadline commits (`a_harness_capture_pinned_after_it_was_selected_for_expiry_is_kept`
  ingests `retain_until: Some(10_000)`).
- GC pin predicate differs from the sweep predicate: `prepare_reclaim`
  retains for any unreleased pin reference regardless of `expires_at` or
  `purge_degraded_at`; `EXPIRED_UNPINNED_SQL` ignores expired and degraded
  pins. This is conservative for GC, and the sources do not say whether it
  is intended.
- `prepare_reclaim` checks a pending unlink before live references; the GC
  record's guarantee now carves out purges.
- Source corrections: the replay-after-deadline arm
  (`a_committed_capture_replays_after_its_retention_deadline`) and the F10
  `messages` case (`retirement_preserves_domain_ownership_without_reusing_old_revision_ids`)
  both existed at baseline `08de9051a`. Part A called them untested.
- Citations that moved: `cas/ingest.rs:1887` no longer holds a
  re-admission test (line 1886 is
  `an_oversized_object_is_corruption_on_both_publication_outcomes`); the
  re-admission tests are in `kernel_deletion.rs` and `kernel_cas.rs`.
  `local_file.rs` :600 to :805 are now the named unit tests listed above.
  The identity fixture count is 24, not 23. The class set is six.
- OQ1: ticket-1081 and pr-1081 name OQ1 as an owner gate and record no
  written product-owner confirmation. The brief lists it as resolved in U9.
- OQ2 is attached to the disclosure record from the spec index; Part A
  listed no open question for that record.
- Witnesses cited: `retired_lineage_republished` (present at HEAD),
  `expired_capture_pinned_mid_sweep` and `expired_capture_cited_elsewhere`
  (being named by the lead).
- Unresolved: a far-future GC test over a live row (F15); an F27 test; a
  direct `retire_evidence` on pinned, uncited evidence; an ingest clock seam.
