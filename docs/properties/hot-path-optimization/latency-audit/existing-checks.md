# Existing checks for the latency audit supplement

The system is `/local/home/ahrav/scratch/eidnara`, at
`913234433ae36a80a6e22c6aac14c7f9aab74386`, checked on 2026-09-10.
The [catalog scope](catalog.md#scope-and-provenance) governs this inventory.
The supplied audit motivates inspection of these checks; external plans and
incidents are not supplied, and final scope confirmation is pending.

This is a focused inventory of checks bearing on the audit surfaces this area
covers, not a replacement for the existing catalogs or for the parent's
[inventory](../existing-checks.md). Every listed check is **unaudited**.
Descriptions identify what the source checks, not whether the oracle is
sufficient. No historical exercise claim is imported and no check runs here.
This is a working-tree inventory against the source baseline. Each section
ends with an explicit "none found" list and the areas the lenses flagged as
suspiciously quiet, so an omitted category is not mistaken for absent tests.

## Ingress admission and decode

| Check | Source condition or assertion | Status |
| --- | --- | --- |
| [`request_byte_cap_widens_for_transform_class_only`][t-cap] | `enforce_request_byte_cap` class set, `kind` beside `method`, non-string fallback, unparseable refusal, structured and 2 MiB `method`, body above the 32 MiB ceiling. | unaudited |
| [`value_footprint_counts_nodes_outside_strings_only`][t-fp] | Separators inside strings and escaped quotes do not count as nodes. | unaudited |
| [`value_footprint_charges_every_retained_copy_of_string_bytes`][t-fp] | The bound is at least three copies of a 1 MiB text block. | unaudited |
| [`scalar_dense_bodies_bound_far_above_their_wire_size`][t-fp] | Node cost dominates for `[1,1,...]`; string bytes are not charged as nodes. | unaudited |
| [`dispatch_routes_each_envelope_class_to_a_distinct_arm`][t-dispatch] | `kind` routing, `facade_envelope_not_supported`, `unrecognized_request_shape` for object and non-object bodies. | unaudited |
| [`management_drop_alias_routes_are_rejected`][t-shape] | Retired aliases return `unrecognized_request_shape`. | unaudited |
| [`indexing_embedding_git_and_mural_are_unreachable_from_every_route_shape`][t-shape2] | Internal names are not routable by `method`, `kind`, or facade. | unaudited |
| [`transform_request_parses_full_flat_wire_envelope`][t-envelope] | `from_value::<TransformRequest>` accepts one full envelope. | unaudited |
| [`first_hard_pass_meta_respects_the_store_durable_text_bound`][t-meta] | `from_value::<TransformRequest>` accepts a `kind`-only body under `bench-internals`; 1_000 messages commit and 1_400 fail with `InputLimit`. | unaudited |
| [`readiness_permissions_catalog_and_real_unary_transform`][directhost] | One small `kind`-only transform is admitted through the real host. | unaudited |
| [`typed_errors_and_stream_markers_have_no_prepared_body`][t-prep] | Error outcomes settle without an output reservation. | unaudited |
| [`capacity_separates_permanent_from_transient_exhaustion`][t-budget] | `try_charge` above capacity is permanent and consumes nothing. | unaudited |
| [`try_charge_is_exact_and_all_or_none`][t-budget] | Over-capacity acquisition leaves the budget unchanged; `u32` overflow refuses. | unaudited |
| [`the_resident_cap_splits_into_three_non_overlapping_pools`][t-pools] | Ingress, egress, and scratch pools sum to the floor. | unaudited |

None found: a handler-level test that drives `Handler::handle` with an
over-cap or over-footprint body and asserts `invalid_params`, or drains the
scratch pool and asserts `queue_full` (`RequestCtx` is transport-private, so
tests enter at [`dispatch_value`][testentry]); a differential of duplicate
keys, `null` fields, or malformed JSON across the unpaged and paged lanes; a
test that a `null` page field selects the page lane; a test at exactly
`MAX_TRANSFORM_FRAME_BYTES`; a test that counts string copies retained by the
typed decode. The `request_too_large` assertion in
[direct_host.rs][fixture] tests the fixture's line-based control channel,
not the handler.

## Shared-input equivalence

| Check | Source condition or assertion | Status |
| --- | --- | --- |
| [`assert_prefix_projection_equivalent`][assert-prefix] | Incremental and full projection equal by `differential_bytes` and by value; live under [`prefix_projection_differential_enabled`][gate-prefix], which is `cfg!(test) || env`. | unaudited |
| [`incremental_projection_reuses_prefix_storage_and_preserves_tool_arc_state`][t-inc] | Reattached prefix equals `from_parts` inputs; incremental equals full; prefix `Arc`s are pointer-shared. | unaudited |
| [`reattach_keeps_block_level_original_but_rebuilds_the_message_shell`][t-reattach] | An unknown message-level field is dropped; a block-level field is kept. | unaudited |
| [`projection_differential_catches_corrupt_first_changed_position`][t-projdiff] | A corrupt frontier is caught by the differential. | unaudited |
| [`astro_scale_projection_cache_reuses_on_the_second_pass`][t-astro] | The second pass reuses the cached projection. | unaudited |
| [`pending_rewrite_passes_isolate_ingress_meta_usage_and_reconcile`][t-pending] | Pass reads of ingress meta are isolated. | unaudited |
| [`warm_cache_selection_bust_does_not_replay_collapsed_synthetic_todo_as_live`][t-collapsed] | A replayed pair without the flag yields no duplicate tool-use id and reuses the cache. | unaudited |
| [`parked_p2_fingerprint_reuse_and_tag_frontier_match_baseline`][t-parked] | Reused `content_hash` fingerprints and `canonical_bytes` equal a full rehash. | unaudited |
| [`served_fingerprint_block_ids_pin_flat_mid_index_format`][t-fpids] | Fingerprint block ids are `mid#index` and synthetic ids. | unaudited |
| [`transform_segments_preserve_existing_golden_bytes`][t-segments] | `Exact` segments concatenate into the golden body. | unaudited |
| [`incremental_native_cache_replays_complex_prefix_and_encodes_only_tail`][t-native-inc] | The native prefix is replayed, the tail encoded, and the differential is live. | unaudited |
| [`differential_assert_rejects_frontier_inside_mutated_native_region`][t-native-reject] | The differential panics on a corrupt native frontier. | unaudited |
| [`frontier_vacuity_covers_opaque_repeats_eviction_and_same_length_edits`][t-vacuity] | Same-length edits and repeats are not vacuously reused. | unaudited |
| [`duplicate_tool_use_assert_covers_incremental_native_suffix`][t-dup] | Tool-use ids are unique across the cached prefix and encoded suffix. | unaudited |
| [`incremental_sidecar_carries_pins_across_three_generations`][t-sidecar] | `mid_pins` survive incremental sidecar decode. | unaudited |
| [`tag_baseline_cache_matches_cold_passes_across_drop_reset_and_remint`][t-tagcold] | Cold and cached passes serve equal bytes and equal durable rows across five passes. | unaudited |
| [`poisoned_tag_baseline_refills_after_direct_sql_update`][t-poison] | A generation change refills the baseline. | unaudited |
| [`tag_baseline_cache_keeps_interleaved_sessions_isolated`][t-interleave] | Two sessions do not share rows. | unaudited |
| [`measurement_is_identical_with_cold_and_warm_token_cache`][t-hyg-cold] | Hygiene output is independent of token cache state. | unaudited |
| [`parity_golden_matches_ts_reference_across_full_corpus`][t-hyg-golden] | Per-part hashes and totals match the TypeScript golden. | unaudited |
| [selection_differential.rs][t-seldiff] | Optimized selection equals the frozen reference over generated `SelItem`s. | unaudited |

None found: a test that `tool_input` on a `FlatBlock` equals the `input`
inside its `wire.kind()`; a test that a `Served` prepared segment writes
`canonical_bytes`, or that names the sorted-key form; a two-lane (full array
versus delta) comparison of the historian's `BoundaryMsg` list,
`input_ordinals`, or the native attachment for one replayed synthetic pair; a
test that the tag baseline entry is unchanged after a pass whose mint commit
fails; a test that runs either differential gate from `crates/daemon/tests/`
or `crates/daemon/benches/` (`cfg!(test)` is false there and no file sets the
variables); a comparison of incremental and full `DecodeSidecar` values
rather than encoded bytes.

## Cache-state load, pass trace, side channel, and meta preparation

| Check | Source condition or assertion | Status |
| --- | --- | --- |
| [`no_fire_reason_is_durable_change_gated_and_cleared_by_fire`][t-no-fire] | The post-commit load feeds the `record_no_fire` CAS; a repeated reason leaves `row_version` unchanged. | unaudited |
| [`handler_emergency_refolds_when_active_run_publishes_before_live_wait_capture`][t-emergency] | A publication between transform and prepare, injected through [`between_transform_and_prepare`][hook], triggers the rerun. | unaudited |
| [`handler_delta_boundary_divergence_recut_retries_cas_without_stale_projection`][t-cas] | A CAS conflict reruns `apply_once` with a fresh load. | unaudited |
| [`transform_snapshot_resists_commit_between_state_and_overlay_reads`][t-snap-resist] | One read transaction pins `cache_state` and the overlays. | unaudited |
| [`transform_snapshot_keeps_row_version_and_overlays_from_one_commit`][t-snap-keeps] | The snapshot `row_version` matches overlays from the same commit. | unaudited |
| [`transform_cas_conflict_leaves_every_overlay_table_empty`][t-cas-empty] | The CAS loser commits no overlay rows. | unaudited |
| [`competing_pass_counter_survives_direct_primary_lifecycle_and_reopen`][t-counter] | The CAS loser is rejected; state survives reopen. | unaudited |
| [`transform_reject_records_trace_without_advancing_row_version`][t-reject] | Reject: `receive_count == 1`, `reject_count == 1`, `row_version` unchanged. | unaudited |
| [`transform_success_records_received_and_completed_trace`][t-success] | Success: received then completed timestamps. | unaudited |
| [`repeated_rejects_increment_trace_and_overwrite_last_error`][t-repeat] | Counters increment per pass; the last error is overwritten. | unaudited |
| [`sequential_failing_passes_trace_every_reject_while_cache_state_stays_frozen`][t-frozen] | Four rejects: `receive_count == 4`, `row_version` frozen. | unaudited |
| [`status_and_health_surface_pass_trace_for_rejected_sessions`][t-status] | Status and health expose `pass_trace` after a reject. | unaudited |
| [`status_distinguishes_current_and_historical_divergence`][t-divergence] | `first_divergence` is NULL after stable; `last_divergence` is retained. | unaudited |
| [`pass_trace_upserts_counts_and_caps_errors`][t-upserts] | Upsert counters, a 256-entry scheduler ring, a 2000-character error cap. | unaudited |
| [`scheduler_trace_records_every_pass_and_preserves_variable_arm_state`][t-sched] | One scheduler observation per accepted pass. | unaudited |
| [`pass_trace_refuses_a_new_secret_session_and_keeps_tracing_a_stored_one`][t-secret] | Trace writes refuse a new secret session and tolerate a stored one. | unaudited |
| [`historian_side_channel_outbox_recovers_after_restart`][t-restart] | A failed row is redelivered once after reopen; the pending count drops to 0. | unaudited |
| [`historian_side_channel_faults_are_isolated_and_retryable_per_kind`][t-faults-sc] | One failed kind leaves other kinds delivered; a retry with `now_ms = i64::MAX` succeeds. | unaudited |
| [`status_diagnostics_surface_pending_historian_side_channel_failure`][t-status-sc] | A failed `event` delivery leaves one pending row visible to `status`; a transform pass 1100 ms later drains it and clears the failure. | unaudited |
| [`publish_historian_chunk_cas_conflict_leaves_no_transcript_row`][t-publish-cas] | The CAS loser enqueues no outbox rows. | unaudited |
| [`truncate_compartments_for_revert_removes_anchored_events_and_crossing_ranges`][t-truncate] | Revert deletes outbox rows for the session. | unaudited |
| [`duplicate_json_object_names_are_refused`][t-dup-json] | Duplicate names are refused at top level and nested; the message omits the value. | unaudited |
| [`a_key_directed_substitution_records_its_own_detection`][t-keydir] | A protected-key substitution records a synthetic detection. | unaudited |
| [`a_protected_key_holding_a_container_is_refused`][t-container] | A protected key with a container value refuses. | unaudited |
| [`preserved_json_identities_do_not_exempt_integrity_fields_credential_names_or_nested_values`][t-preserved] | Identity preservation is limited to structural scalar names. | unaudited |
| [`cache_state_redacts_payloads_preserves_existing_ids_and_rejects_integrity`][t-cache-redact] | Commit redacts the core payload, preserves legacy ids, and refuses an integrity secret in `meta`. | unaudited |
| [`cache_state_identity_decision_comes_from_the_write_transaction`][t-identity-tx] | New-versus-existing session is decided inside the fenced transaction. | unaudited |
| [`open_pins_full_synchronous`][t-sync] | `synchronous=FULL` is pinned on open and re-pinned per fenced write. | unaudited |

None found: equivalence between a narrow `meta` scalar read and
`MemoryStore::load`; `Handler::historian_active` reading the durable phase
(only the in-memory branch is implied by firing tests); `first_divergence`
NULL after a rejected pass; `receive_count` after an Emergency95 rerun that
commits twice; a `pass_trace` write failure beside a successful cache commit;
a crash between the outbox mark commit and the delete commit; two drainers
overlapping on one session; outbox ordering across firings, the per-kind
limit, or the backoff values; byte identity of a clean stored `meta` against
`serde_json::to_string`; a secret in a `BTreeMap` key of `ModuleMeta`.

Suspiciously quiet: the drain result and every trace result are discarded
with `let _ =` in the handler, so a regression in either surfaces only
through `session.status` or `health`, and only if someone reads them. The
[`record_no_fire`][no-fire-doc] doc comment reads `/// Delete`, which
describes nothing the function does.

## Plugin pre-send

The P1/P5 rows include the 2026-09-10 permission-cache implementation
campaign. Its approval provenance and red/green execution history are appended
to the [P1 evidence](evidence/cached-todowrite-verdict-never-lifts-a-deny-or-outlives-its-inputs.md).
The P2 rows include the local differential and native-cache campaign described
in the [P2 evidence](evidence/mid-turn-read-is-invariant-under-query-collapse-and-statement-caching.md).
Other rows retain their discovery scope. `unaudited` describes adequacy review,
not whether the focused tests ran.

| Check | Source condition or assertion | Status |
| --- | --- | --- |
| [rust-mode-transform.test.ts:381][permission-provisional] | Provisional availability sends `tool_present: false` and `todo_tool_present: false`. | unaudited |
| [rust-mode-transform.test.ts:444][permission-agent] | An agent `deny` rule through the SDK yields `todo_tool_present: false`; `app.agents` is called once. | unaudited |
| [rust-mode-transform.test.ts:470][permission-empty-timeout] | A hung `app.agents()` with an empty cache yields `todo_tool_present: false` after about 2 s. | unaudited |
| [hook-handlers.test.ts:126][permission-capture-timeout] | A hung permission read suppresses capture within the 2 s deadline. | unaudited |
| [ctx-reduce-availability.test.ts:241][permission-evaluator] | `permissionDisabled` last-match semantics, session overlay after agent rules, wildcard escaping, and distinct active-agent inputs. | unaudited |
| [hook.test.ts:167][permission-witness] | Rejection and fake-time timeout after a stored deny reach the constant P5 marker for transform and capture; outcome checks require false wire verdicts and no capture. The marker establishes reachability, not a distinct fallback outcome. | unaudited |
| [hook.test.ts:245][permission-hit] | Transform and capture share fresh hits without another SDK permission read; missing-client cases suppress both. | unaudited |
| [hook.test.ts:292][permission-lifecycle] | Session update, native compaction, and flush invalidate every agent for one session; deletion clears entries; another session stays fresh. An overlapping capture allow cannot clear a newer transform deny. | unaudited |
| [hook.test.ts:415][permission-overlap] | Real transform and capture hooks share one fill. Empty host agents normalize to absence: the agent-list API is not called, session allow/deny rules decide, and undefined or empty capture reuses the same key. | unaudited |
| [ctx-reduce-availability.test.ts:347][permission-lifetime] | Identity isolation, shared pending reads and timeout, 30 s read-start expiry at lookup and settlement, empty/allow/deny failures, missing named agents, malformed/error SDK payloads, invalidation fencing, and settled/pending LRU eviction. | unaudited |
| [ctx-reduce-availability.test.ts:29-125][tavaildb] | DB-derived frozen verdicts: fail-open freeze, tie by id, malformed JSON row. | unaudited |
| [Frozen differential and example states][tmidturn] | Every valid example compares against the frozen base before and after rollback-scoped second-session rows. The reference file is hash-pinned; its shared primitives are live on both sides. Malformed/dynamic values and tuple sentinels remain covered. Approved inconsistent associations have both old-false/new-true and old-true/new-false checks. Equality applies to static snapshots only. | unaudited |
| [Native cache contract][session-db-cache] | Both adapters exercise native retirement after eviction, oversized SQL/binds, throwing execution, close, and replacement without GC. Real time/part reads grow from 801 to 870 IDs across 70 remainders with mid-turn reads between them: all five statements stay warm on one connection, with zero closes. Timestamp maps, ordered message/part outputs and frozen input lists are checked. Binding limits, arrays, named binds and partless users remain covered; finalizer failure still closes the database, and 128-query pressure leaves at most 64 live natives. | unaudited |
| [Transform hook witness][session-db-hook] | Four real transform calls send `mid_turn` values false, false, true after a committed edit, then false after same-path replacement. | unaudited |
| [isMidTurn wrapper][tismidturn] | Idle DB, missing DB, unreadable DB returns mid-turn. | unaudited |
| [OPENCODE_DB override][tdbpath] | The override selects the database; an empty override is ignored. | unaudited |
| [read-session-raw.test.ts:173, 249][tordinal] | Ordinal keyset page reads wider than one part chunk; summary rows spend no ordinal. | unaudited |
| [sqlite.test.ts:279-467][tsqlite] | Adapter constructor option mapping, transaction shim, runtime selector errors. | unaudited |
| [sqlite-bind-style.test.ts:32][tbind] | Every `.run/.get/.all` uses spread positional binds. | unaudited |
| [module-wire.test.ts:1308][t1308] | Unpaged `bytes` equals a later `JSON.stringify` length. | unaudited |
| [module-wire.test.ts:1371][t1371] | Each paged `bytes` equals a later `JSON.stringify` length. | unaudited |
| [module-wire.test.ts:1391][t1391] | The pageable array field list matches the daemon's Rust literal. | unaudited |
| [rust-mode-transform.test.ts:537, 575, 614][tpaged] | A paged series re-pages after `need_full_sync`; it restarts on attempt mismatch and reconnect. | unaudited |
| [frame-channel.test.ts:181, 193][t181] | The declared byte length equals written bytes for lone surrogates, including across a segment boundary. | unaudited |
| [Joint pager and native-writer fake][serialized-writer] | Frozen corpus checks carried UTF-8 bytes against the raw header and captured byte array through the real module transport, client encoder, and writer. Exact first/final lengths and page counts are asserted. | unaudited |
| [Module transport snapshot][serialized-transport] | A getter changes on a second read, source and inspection values mutate, and a stringify spy rejects any send-time serialization. The public connection factory and channel injection exercise the transport without private-field assignments. | unaudited |
| [Generic carrier encoding][serialized-client] | Plain Pi-style objects with colliding field names or symbol descriptions remain ordinary objects; generic client snapshot encoding retains authoritative text after nested inspection edits. | unaudited |
| [Unpaged boundary][serialized-unpaged] | `2 ** 64`, raw `1e5`, and raw `-0` remain single unpaged requests at 524288 wire bytes despite larger Rust reserialization. | unaudited |
| [Pager serialization spy][serialized-pager] | The full input and every emitted envelope serialize once. `toJSON` runs once. Digest and packing serializations remain outside that claim. | unaudited |
| [Live transform hook][serialized-hook] | The real hook passes a carrier with the matching transform/session discriminator. | unaudited |
| [Real-host corpus][serialized-host] | A registered Cargo integration test compiles in normal CI and is ignored until a generated corpus is supplied. The one-command wrapper requires one named pass. It verifies byte/hash receipts, twelve complete transforms equal to unpaged controls, nine staged pages, six unchanged surrogate refusals, one pager refusal, exact intermediate/final boundaries, and the 24-byte f64 witness. | unaudited |
| [rust-mode-transform.test.ts:1446, 1582][tinplace] | In-place mutation of an older message forces a full send; recovery after repeated rejection. | unaudited |
| [rust-mode-transform.test.ts:248][t244] | `rust pass:` and `rust module stages:` lines are emitted per pass (spy on `sessionLog`). | unaudited |
| [logger.test.ts:358][t358] | Control characters are removed; entry size is bounded; no forged fourth line. | unaudited |
| [logger.test.ts:381][t381] | The default log is `0600` under `0700` directories; a planted symlink is not followed. | unaudited |
| [logger.test.ts:342, 408][t342] | Swallowed-write counter; exit flush without holding the process. | unaudited |
| [event-handler.test.ts:272-524][tevent] | `message.updated` usage bookkeeping; no assertions on log lines. | unaudited |

None found: a test asserting secret redaction of log lines on this path;
a test asserting log line counts per pass or per
`message.updated` event.

The [2026-09-11 local host probe](evidence/paged-body-measure-equals-declared-frame-length-and-fits-host-caps.md#q-what-does-the-real-host-admission-probe-establish)
constructs the boundary under both measures and observes real host refusals.
It is historical diagnostic execution, not a passing P3 check. The
[scoped carrier campaign][serialized-campaign] records the approved refusal
oracle and executable checks. Native-writer fakes do not prove actual addon
attachment; that mechanism is unavailable on the tested Bun and Node runtimes.

[serialized-writer]: ../../../../packages/opencode-plugin/src/hooks/context/module-wire-frame.test.ts#L55
[serialized-transport]: ../../../../packages/opencode-plugin/src/hooks/context/module-wire-frame.test.ts#L97
[serialized-client]: ../../../../packages/opencode-plugin/src/shared/host-client/client.test.ts#L108-L175
[serialized-unpaged]: ../../../../packages/opencode-plugin/src/hooks/context/module-wire.test.ts#L1456
[serialized-pager]: ../../../../packages/opencode-plugin/src/hooks/context/module-wire.test.ts#L1425
[serialized-hook]: ../../../../packages/opencode-plugin/src/hooks/context/hook.test.ts#L1415
[serialized-host]: ../../../../crates/daemon/tests/serialized_transform_pages.rs#L11
[serialized-campaign]: evidence/paged-body-measure-equals-declared-frame-length-and-fits-host-caps.md#q-what-do-the-unpaged-correction-and-registered-cargo-test-prove

[permission-provisional]: ../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L381
[permission-agent]: ../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L444
[permission-empty-timeout]: ../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L470
[permission-capture-timeout]: ../../../../packages/opencode-plugin/src/hooks/context/hook-handlers.test.ts#L126
[permission-evaluator]: ../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.test.ts#L241
[permission-witness]: ../../../../packages/opencode-plugin/src/hooks/context/hook.test.ts#L167
[permission-hit]: ../../../../packages/opencode-plugin/src/hooks/context/hook.test.ts#L245
[permission-lifecycle]: ../../../../packages/opencode-plugin/src/hooks/context/hook.test.ts#L292
[permission-overlap]: ../../../../packages/opencode-plugin/src/hooks/context/hook.test.ts#L415
[permission-lifetime]: ../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.test.ts#L347

Suspiciously quiet: `transform-stage-logger.ts` has no test file;
`event-handler.test.ts` never asserts on the two `sessionLog` lines per
`message.updated` event. The two empty-cache hung-read tests now assert
fail-closed behavior. Separate P5 hook witnesses seed a successful deny before
forcing and observing a failed live read.

## Ring arena and direct frame

| Check | Source condition or assertion | Status |
| --- | --- | --- |
| [`process_limits_reject_counts_above_the_resident_byte_ceiling`][t-limits] | `affordable = MAX_RING_RESIDENT_BYTES / arena_bytes`; `affordable + 1` is refused; zero rounds to one. | unaudited |
| [`unaligned_batch_boundaries_do_not_strand_pages`][t-batch] | A run of `batch + 100` bytes leaves one boundary page; the next batch removes it. | unaudited |
| [`aborted_reservation_leaves_no_resident_pages`][t-abort] | Two written pages, abort, zero resident, conservation intact. | unaudited |
| [`reclaimed_pages_leave_residency_and_reuse_as_zeroes`][t-reuse] | A full-arena publish and release; the next reserve sees zero resident and reads zero bytes. | unaudited |
| [`subpage_releases_stay_resident_until_trim`][t-subpage] | Sub-page releases keep one page until `trim`. | unaudited |
| [`trim_reclaims_pending_releases_before_punching`][t-trim-order] | `trim` reclaims then punches; conservation intact. | unaudited |
| [`trim_preserves_bytes_of_an_uncommitted_reservation`][t-trim-res] | `reserved_end` protects an uncommitted reservation from `trim`. | unaudited |
| [`page_removal_failure_quarantines_before_capacity_publication`][t-punchfail] | An injected `madvise` failure quarantines; cursors are unchanged. | unaudited |
| [`retained_oldest_lease_enforces_fifo_reclamation`][t-fifo] | Releasing the oldest lease makes completed pages removable. | unaudited |
| [`sealed_sparse_object_repeated_setup_and_stress_conservation`][t-sparse] | A fresh ring has zero resident arena pages. | unaudited |
| [Syscall counter test][t-syscall] | One `trim` is one `page_removals`. | unaudited |
| [`copy_in_then_copy_out_round_trips_at_every_alignment_and_length`][t-roundtrip] | 16 source offsets x 16 shifts x 40 lengths round-trip; bytes before the shift are untouched. | unaudited |
| [`span_reads_tolerate_a_concurrent_writer`][t-concurrent] | A concurrent same-shape `copy_in` never tears a byte through `read_byte`, `copy_to`, or `checksum`. | unaudited |
| [`read_byte_agrees_with_copy_to_at_every_alignment`][t-readbyte] | Per-byte and bulk reads agree. | unaudited |
| [Miri job][ci-miri] | `lease::` and `backend::ring::miri` tests run under Miri; the job fails unless at least one passes. | unaudited |
| [Valgrind job][ci-valgrind] | `shm-transport --test ring` runs under memcheck. | unaudited |
| [`a_commit_past_the_write_deadline_is_refused`][t-deadline] | `publish_direct` with a slow serializer publishes nothing. | unaudited |
| [`commit_after_quarantine_is_refused_and_aborts`][t-commitq] | `commit` on a quarantined ring aborts. | unaudited |
| [`into_parts_returns_the_unused_output_reservation`][t-parts] | The owned charge shrinks to `len + HEADER_LEN`. | unaudited |
| [`into_parts_never_grows_a_charge_below_the_encoded_size`][t-parts2] | A smaller charge stays unchanged. | unaudited |
| [`into_parts_does_not_leave_a_large_allocation_behind_a_small_charge`][t-parts3] | `Vec` capacity is at most the charge. | unaudited |

None found: a test that asserts `arena_reclaimed - punched <
punch_batch_bytes` as an inequality over a long publish and release sequence,
or that measures resident pages on an idle host ring; any production reader
of `resident_arena_pages` or of process RSS; a test of `to_vec` with span
lengths that do not sum to `body_len`; a test of the direct path with
underfill, overflow, a serializer `Err` after partial write, or a serializer
panic (the `direct_fill` fixture arm has no sender); a test of the egress
charge held by a Direct `OutputBuffer`, or of a queued direct frame dropped
by generation retirement.

Suspiciously quiet: the [hardware envelope bench][bench] counts
`page_removal_syscalls` and `body_copies` per operation, so the audit's
per-MiB cost has a measurement channel, but no test consumes those counters
as an oracle; [`reclaimed_pages_leave_residency_and_reuse_as_zeroes`][t-reuse]
pins a side effect of eager punching that no contract requires and deferral
would change.

## CAS usage accounting

| Check | Source condition or assertion | Status |
| --- | --- | --- |
| [`cap_error_reports_usage_and_cap_without_poisoning_reads`][t-cap-cas] | `Capacity` carries `usage` and `cap`; reads still work. | unaudited |
| [`invalidated_retained_object_still_consumes_cap`][t-retained] | Invalidated bytes count against the cap. | unaudited |
| [`reclaim_frees_capacity_for_next_write`][t-reclaim] | GC reclaim lowers usage so the next ingest fits. | unaudited |
| [`payload_limit_is_inclusive_at_the_artifact_cap`][t-payload] | 64 MiB is accepted; 64 MiB + 1 is refused with no reservation or temp. | unaudited |
| [`ingest_route_accepts_a_payload_at_the_artifact_cap`][t-route-cap] | The daemon route at and over `MAX_PAYLOAD_BYTES`. | unaudited |
| [`orphan_mtime_grace_and_budget_facts_are_reconciled_from_objects`][t-orphan] | Usage is read from objects written outside the store; `warn` at 80 percent. | unaudited |
| [`facts_unless_abandons_the_artifact_walk_once_cancelled`][t-cancel] | The walk polls cancellation before every entry. | unaudited |
| [`assert_semantic_oracle`][t-oracle] | `artifact_budget_facts().usage_bytes` equals an independent scan after every fault. | unaudited |
| [`recover_twice`][t-recover-twice] | A second recovery changes nothing. | unaudited |
| [`return_value_fault_table_latches_eio_and_never_publishes_a_reference`][t-faults] | Six ingest fault points. | unaudited |
| [`purge_and_gc_fault_table_preserves_pending_work_and_converges`][t-gcfaults] | A purge unlink fault and three GC fault points. | unaudited |
| [`crash_windows_recover_idempotently_and_match_no_crash_execution`][t-crash] | SIGKILL children at reservation, ingest, and purge barriers. | unaudited |
| [`startup_retires_a_live_reservation_whose_bytes_are_already_gone`][t-gone] | Recovery deletes a reservation with no bytes. | unaudited |

None found: a test that dedup at exactly the cap is admitted, or that a
second ingest before recovery of an orphan publish is refused; a comparison
of two independent usage sources, because only the walk exists; any daemon
caller of `run_staging_maintenance` or `delete_artifact`, so the GC and purge
decrement paths run only in kernel tests and benches; a test that the health
sampler's lock-free walk and `check_budget`'s locked walk agree, or that the
sampler's 300 s staleness bound is reached by a slow walk; a per-path usage
delta in any fault table.

## Wildcard and cross-cutting

| Check | Source condition or assertion | Status |
| --- | --- | --- |
| [`ci.yml` benches in test mode][ci-bench] | Every bench target runs once; no comparison, no threshold. | unaudited |
| [`evidence.rs`][evidence] manifest rules | Host-runtime arm records carry schema, workload, build, host, and arm ids. | unaudited |
| [`pass_timing_line_is_parseable_for_an_empty_session`][t-line] | The timing line's key set and `key=value` shape. | unaudited |
| [`timings_are_present_and_old_responses_deserialize_without_them`][t-timings] | Absent `timings` deserializes to the default. | unaudited |
| [`emits discriminating pass and stage logs from ordinary Rust transforms`][t244] | The plugin logs `rust module stages:` from response timings. | unaudited |
| [`cached_counts_match_the_tokenizer`][t-tc-match] | The cached count equals the tokenizer count on hit and miss for four inputs. | unaudited |
| [`digest_keyed_hits_skip_retokenization`][t-tc-hits] | The second lookup is a hit, not a miss. | unaudited |
| [`insert_current_rotates_at_capacity`][t-tc-rotate] | `current` never exceeds `GENERATION_CAP`. | unaudited |
| [`stats_partition_calls_into_hits_misses_and_bypassed`][t-tc-stats] | `calls == hits + misses + bypassed`. | unaudited |
| [`kind_prefixed_and_raw_content_keys_do_not_alias`][t-tc-alias] | The tail-hygiene and raw key domains are disjoint. | unaudited |
| [`protected_floor_has_no_global_estimator_bypass`][t-bypass] | No direct tokenizer call in one helper's source span. | unaudited |
| [`preselection_never_drops_a_rule_whose_pattern_matches`][t-preselect] | A matching rule is always preselected (16 inputs). | unaudited |
| [`provider_canaries_return_stable_rule_ids_and_value_spans`][t-canaries] | Canary inputs yield stable rule ids and spans. | unaudited |
| [`minimal_fixture_is_truthful_and_executable`][t-qual] | A one-case qualification fixture with `authority_qualified: false`. | unaudited |
| [`evaluator_constants_are_pinned`][t-pinned] | Evaluator tables and constants digest are pinned. | unaudited |
| [`windows_start_on_line_boundaries_and_overlap_when_lines_are_short`][t-windows] | Redaction window placement and overlap. | unaudited |
| [`scanner_is_the_only_redaction_path`][t-only-path] | No redaction path bypasses the scanner. | unaudited |
| [`chunk_fingerprint_uses_id_kind_and_byte_length`][t-chunk-fp] | The fingerprint literal `id:kind:len`. | unaudited |
| [`optimized_matches_frozen_reference_at_production_windows`][diff-prod] | Truncation bytes equal the frozen reference at production windows (24 cases, budget 1..32_001). | unaudited |
| [`exact_token_budget_returns_original_input`][diff-exact] | Input at budget returns unchanged. | unaudited |
| [`optimized_matches_frozen_reference`][diff-small] | Small-window byte equality. | unaudited |
| [`forced_overflow_preserves_existing_truncation_output`][t-golden] | One golden truncation case from `testdata/historian-chunk-golden.json`. | unaudited |
| [`truncation_uses_marker_and_keeps_multibyte_boundaries`][t-marker] | Marker suffix, budget, scalar-boundary prefix. | unaudited |
| [`star_prefixed_day_fields_are_unrestricted_like_vixie_cron`][t-vixie] | Vixie `*`-prefix semantics. | unaudited |
| [`next_occurrence_survives_extreme_instants`][t-extreme] | `None` at `i64` extremes. | unaudited |
| [`smart_note_evaluation_golden_matches_production_behaviour`][t-golden-cron] | Reduction and due-time golden with a fixture timezone. | unaudited |
| [`a_task_runs_only_once_its_cron_instant_has_passed`][t-sched-cron] | A `*/15 * * * *` project runs at its instant and not before; the same instant does not run twice. | unaudited |
| [`mtime_cache_reuses_unchanged_reads_and_invalidates_on_mtime_change`][t-mtime] | A same mtime hides an edit; a new mtime reloads. | unaudited |
| [`project_threshold_may_only_raise`][t-raise] | The `ProjectRaiseOnly` threshold. | unaudited |
| [`project_tier_cannot_raise_the_user_memory_gate`][t-gate] | The `ProjectRaiseOnly` gate. | unaudited |
| [`hostile_project_tier_cannot_change_privileged_values_and_warns_per_key`][t-hostile] | Privileged keys are ignored with a per-key warning. | unaudited |
| [`a_handler_panic_maps_to_one_redacted_internal_error`][t-panic-internal] | A handler panic on the runtime worker settles as one `internal_error` terminal. | unaudited |
| [`handler_panic_payload_is_redacted_from_process_stderr`][t-panic-stderr] | In a child process, stderr carries the fixed redacted diagnostic, not the handler payload, and an unrelated panic still reaches the prior hook. | unaudited |
| [`panic_redaction_subprocess_child`][t-panic-child] | The child role: installs a prior hook, drives a panicking handler, asserts `internal_error`. | unaudited |

None found: a daemon benchmark that records build, host, and workload
identity; any bench reaching `Handler::handle` or a production-sized steady
session; a check that `DECLARED_RETAINED_RESIDENT_BYTES` includes every
cache; token-cache lock contention evidence; a bounded-scan versus
whole-input differential for the secret scanner; a DST or
unsatisfiable-expression case for the cron stepper; guidance override
staleness, or two project roots sharing one `ConfigCache`; any test of the
SOFT pressure-refold predicate's classification; any test that aborts between
commit and bookkeeping and inspects the next pass; any test in
`crates/daemon/tests/` or in `kernel_routes` that panics inside a
`kernel_routes::blocking` closure or any other `spawn_blocking` worker and
reads the process's stderr or the request's terminal (the three redaction
tests above panic on the runtime worker inside the guard).

Suspiciously quiet: the shm bench manifest declares probes the binary cannot
run and the transport bench's record labels itself `BLOCKED`, yet nothing
fails; the scanner's qualification fixture has one case and declares itself
tooling only; the two production-sized timing fixtures are `#[ignore]` and
print to stderr, so no CI job observes their numbers.

## Audit handoff

All tests go to `/testing:invariant-test-review` before an adequacy verdict.
Production guards go to
`/low-level-systems:defensive-assertions-and-invariant-guards`. Empty
categories above mean no check was identified in the stated inspected scope,
not a claim that no related check exists anywhere in the repository.

[testentry]: ../../../../crates/daemon/src/lib.rs#L12484-L12499
[fixture]: ../../../../crates/daemon/tests/direct_host.rs#L285-L290
[t-cap]: ../../../../crates/daemon/src/lib.rs#L18567-L18625
[t-fp]: ../../../../crates/daemon/src/lib.rs#L18627-L18691
[t-dispatch]: ../../../../crates/daemon/src/lib.rs#L26568-L26623
[t-shape]: ../../../../crates/daemon/src/lib.rs#L32419-L32436
[t-shape2]: ../../../../crates/daemon/src/lib.rs#L32439-L32479
[t-envelope]: ../../../../crates/daemon/src/transform.rs#L16159-L16185
[t-meta]: ../../../../crates/daemon/tests/transform_meta_bound.rs#L21-L96
[directhost]: ../../../../crates/daemon/tests/direct_host.rs#L48-L128
[t-prep]: ../../../../crates/daemon/tests/prepared_output.rs#L103-L115
[t-budget]: ../../../../crates/host-runtime/src/wire.rs#L825-L865
[t-pools]: ../../../../crates/host-runtime/src/config.rs#L480-L503

[gate-prefix]: ../../../../crates/daemon/src/transform.rs#L2013-L2020
[assert-prefix]: ../../../../crates/daemon/src/transform.rs#L2022-L2037
[t-inc]: ../../../../crates/daemon/src/wire.rs#L1360
[t-reattach]: ../../../../crates/daemon/src/wire.rs#L1545
[t-projdiff]: ../../../../crates/daemon/src/lib.rs#L22220
[t-astro]: ../../../../crates/daemon/src/lib.rs#L20948
[t-pending]: ../../../../crates/daemon/src/transform.rs#L19129
[t-collapsed]: ../../../../crates/daemon/src/transform.rs#L27270
[t-parked]: ../../../../crates/daemon/src/transform.rs#L13707
[t-fpids]: ../../../../crates/daemon/src/transform.rs#L13577
[t-segments]: ../../../../crates/daemon/tests/prepared_output.rs#L34-L54
[t-native-inc]: ../../../../crates/daemon/src/lib.rs#L20644
[t-native-reject]: ../../../../crates/daemon/src/lib.rs#L21748
[t-vacuity]: ../../../../crates/daemon/src/lib.rs#L21678
[t-dup]: ../../../../crates/daemon/src/lib.rs#L22264
[t-sidecar]: ../../../../crates/daemon/src/codec/opencode.rs#L2062
[t-tagcold]: ../../../../crates/daemon/src/transform.rs#L22418
[t-poison]: ../../../../crates/daemon/src/transform.rs#L22487
[t-interleave]: ../../../../crates/daemon/src/transform.rs#L22520
[t-hyg-cold]: ../../../../crates/daemon/src/tail_hygiene.rs#L795
[t-hyg-golden]: ../../../../crates/daemon/src/tail_hygiene.rs#L1104
[t-seldiff]: ../../../../crates/daemon/tests/selection_differential.rs#L1-L5

[hook]: ../../../../crates/daemon/src/lib.rs#L8224-L8232
[no-fire-doc]: ../../../../crates/daemon/src/lib.rs#L5449
[t-no-fire]: ../../../../crates/daemon/src/lib.rs#L35691
[t-emergency]: ../../../../crates/daemon/src/lib.rs#L34923
[t-cas]: ../../../../crates/daemon/src/lib.rs#L22606
[t-snap-resist]: ../../../../crates/memory-store/src/lib.rs#L16453
[t-snap-keeps]: ../../../../crates/memory-store/src/lib.rs#L16507
[t-cas-empty]: ../../../../crates/memory-store/src/lib.rs#L16576
[t-counter]: ../../../../crates/daemon/tests/boundary_counter_durability.rs#L12
[t-reject]: ../../../../crates/daemon/src/lib.rs#L23449
[t-success]: ../../../../crates/daemon/src/lib.rs#L23479
[t-repeat]: ../../../../crates/daemon/src/lib.rs#L23495
[t-frozen]: ../../../../crates/daemon/src/lib.rs#L23523
[t-status]: ../../../../crates/daemon/src/lib.rs#L23556
[t-divergence]: ../../../../crates/daemon/src/lib.rs#L31641
[t-upserts]: ../../../../crates/memory-store/src/lib.rs#L17545
[t-sched]: ../../../../crates/daemon/src/transform.rs#L13521
[t-secret]: ../../../../crates/memory-store/src/lib.rs#L15376
[t-restart]: ../../../../crates/memory-store/src/lib.rs#L18920
[t-faults-sc]: ../../../../crates/memory-store/src/lib.rs#L18704
[t-status-sc]: ../../../../crates/daemon/src/lib.rs#L35555
[t-publish-cas]: ../../../../crates/memory-store/src/lib.rs#L19067
[t-truncate]: ../../../../crates/memory-store/src/lib.rs#L20529
[t-dup-json]: ../../../../crates/memory-store/src/lib.rs#L15112
[t-keydir]: ../../../../crates/memory-store/src/lib.rs#L15029
[t-container]: ../../../../crates/memory-store/src/lib.rs#L15129
[t-preserved]: ../../../../crates/memory-store/src/lib.rs#L15166
[t-cache-redact]: ../../../../crates/memory-store/tests/production_redaction.rs#L606
[t-identity-tx]: ../../../../crates/memory-store/src/lib.rs#L15309
[t-sync]: ../../../../crates/storage/src/lib.rs#L3397

[tpaged]: ../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L537
[t244]: ../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L248
[tinplace]: ../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L1446
[tavaildb]: ../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.test.ts#L29-L125
[tmidturn]: ../../../../packages/opencode-plugin/src/hooks/context/read-session-db.test.ts#L57-L892
[tismidturn]: ../../../../packages/opencode-plugin/src/hooks/context/read-session-db.test.ts#L1138-L1169
[tdbpath]: ../../../../packages/opencode-plugin/src/hooks/context/read-session-db.test.ts#L1171-L1239
[session-db-cache]: ../../../../packages/opencode-plugin/src/hooks/context/__tests__/session-db-cache-contract.ts#L1-L392
[session-db-hook]: ../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L1613-L1667
[tordinal]: ../../../../packages/opencode-plugin/src/hooks/context/read-session-raw.test.ts#L173
[tsqlite]: ../../../../packages/opencode-plugin/src/shared/sqlite.test.ts#L279
[tbind]: ../../../../packages/opencode-plugin/src/shared/sqlite-bind-style.test.ts#L32
[t1308]: ../../../../packages/opencode-plugin/src/hooks/context/module-wire.test.ts#L1308
[t1371]: ../../../../packages/opencode-plugin/src/hooks/context/module-wire.test.ts#L1371
[t1391]: ../../../../packages/opencode-plugin/src/hooks/context/module-wire.test.ts#L1391
[t181]: ../../../../packages/opencode-plugin/src/shared/host-client/frame-channel.test.ts#L181
[t358]: ../../../../packages/opencode-plugin/src/shared/logger.test.ts#L358
[t381]: ../../../../packages/opencode-plugin/src/shared/logger.test.ts#L381
[t342]: ../../../../packages/opencode-plugin/src/shared/logger.test.ts#L342
[tevent]: ../../../../packages/opencode-plugin/src/hooks/context/event-handler.test.ts#L272

[t-limits]: ../../../../crates/host-runtime/src/ring_transport.rs#L1052-L1074
[t-batch]: ../../../../crates/shm-transport/src/backend/ring.rs#L4016-L4044
[t-abort]: ../../../../crates/shm-transport/src/backend/ring.rs#L3677-L3693
[t-reuse]: ../../../../crates/shm-transport/src/backend/ring.rs#L4073-L4089
[t-subpage]: ../../../../crates/shm-transport/src/backend/ring.rs#L4091-L4116
[t-trim-order]: ../../../../crates/shm-transport/src/backend/ring.rs#L3329-L3344
[t-trim-res]: ../../../../crates/shm-transport/src/backend/ring.rs#L4136-L4161
[t-punchfail]: ../../../../crates/shm-transport/src/backend/ring.rs#L4243-L4258
[t-fifo]: ../../../../crates/shm-transport/tests/ring.rs#L124-L174
[t-sparse]: ../../../../crates/shm-transport/tests/ring.rs#L250-L254
[t-syscall]: ../../../../crates/shm-transport/src/backend/ring.rs#L3086-L3087
[t-roundtrip]: ../../../../crates/shm-transport/src/lease.rs#L475-L506
[t-concurrent]: ../../../../crates/shm-transport/src/lease.rs#L581-L619
[t-readbyte]: ../../../../crates/shm-transport/src/lease.rs#L543
[ci-miri]: ../../../../.github/workflows/ci.yml#L597-L635
[ci-valgrind]: ../../../../.github/workflows/ci.yml#L637-L669
[t-deadline]: ../../../../crates/host-runtime/src/ring_transport.rs#L1857-L1887
[t-commitq]: ../../../../crates/shm-transport/src/backend/ring.rs#L3208
[t-parts]: ../../../../crates/host-runtime/src/handler.rs#L596-L626
[t-parts2]: ../../../../crates/host-runtime/src/handler.rs#L628-L645
[t-parts3]: ../../../../crates/host-runtime/src/handler.rs#L647-L673
[bench]: ../../../../crates/shm-transport/benches/hardware_envelope.rs#L296-L306

[t-cap-cas]: ../../../../crates/kernel/tests/kernel_cas.rs#L418-L433
[t-retained]: ../../../../crates/kernel/tests/kernel_cas.rs#L436-L450
[t-reclaim]: ../../../../crates/kernel/tests/kernel_gc.rs#L543-L562
[t-payload]: ../../../../crates/kernel/tests/kernel_cas.rs#L214-L234
[t-route-cap]: ../../../../crates/daemon/tests/kernel_routes.rs#L3728-L3756
[t-orphan]: ../../../../crates/kernel/tests/kernel_gc.rs#L602-L642
[t-cancel]: ../../../../crates/kernel/tests/kernel_gc.rs#L565-L599
[t-oracle]: ../../../../crates/kernel/tests/cas_fault_injection.rs#L350-L382
[t-recover-twice]: ../../../../crates/kernel/tests/cas_fault_injection.rs#L384-L389
[t-faults]: ../../../../crates/kernel/tests/cas_fault_injection.rs#L426-L494
[t-gcfaults]: ../../../../crates/kernel/tests/cas_fault_injection.rs#L497-L582
[t-crash]: ../../../../crates/kernel/tests/cas_fault_injection.rs#L924-L990
[t-gone]: ../../../../crates/kernel/tests/cas_fault_injection.rs#L860

[ci-bench]: ../../../../.github/workflows/ci.yml#L514-L518
[evidence]: ../../../../crates/host-runtime/benches/support/evidence.rs#L1-L8
[t-line]: ../../../../crates/daemon/src/transform.rs#L12278
[t-timings]: ../../../../crates/daemon/src/transform.rs#L12228
[t-tc-match]: ../../../../crates/daemon/src/token_cache.rs#L188
[t-tc-hits]: ../../../../crates/daemon/src/token_cache.rs#L209
[t-tc-rotate]: ../../../../crates/daemon/src/token_cache.rs#L233
[t-tc-stats]: ../../../../crates/daemon/src/token_cache.rs#L249
[t-tc-alias]: ../../../../crates/daemon/src/token_cache.rs#L266
[t-bypass]: ../../../../crates/daemon/src/transform.rs#L24170-L24181
[t-preselect]: ../../../../crates/secret-scanner/src/rules.rs#L685-L722
[t-canaries]: ../../../../crates/secret-scanner/tests/rule_canaries.rs#L4
[t-qual]: ../../../../crates/secret-scanner/tests/qualification.rs#L49-L63
[t-pinned]: ../../../../crates/secret-scanner/src/evaluator.rs#L1660-L1683
[t-windows]: ../../../../crates/context-core/src/redaction.rs#L827-L856
[t-only-path]: ../../../../crates/context-core/src/redaction.rs#L857
[t-chunk-fp]: ../../../../crates/daemon/src/historian.rs#L4006-L4033
[diff-prod]: ../../../../crates/daemon/tests/historian_truncate_differential.rs#L100-L113
[diff-exact]: ../../../../crates/daemon/tests/historian_truncate_differential.rs#L115-L129
[diff-small]: ../../../../crates/daemon/tests/historian_truncate_differential.rs#L131-L140
[t-golden]: ../../../../crates/daemon/src/historian_chunk.rs#L1749-L1760
[t-marker]: ../../../../crates/daemon/src/historian_chunk.rs#L1762-L1763
[t-vixie]: ../../../../crates/daemon/src/smart_note_evaluation.rs#L1594
[t-extreme]: ../../../../crates/daemon/src/smart_note_evaluation.rs#L1580-L1591
[t-golden-cron]: ../../../../crates/daemon/src/smart_note_evaluation.rs#L1126
[t-sched-cron]: ../../../../crates/daemon/src/dreamer_scheduler.rs#L680
[t-mtime]: ../../../../crates/daemon/src/config.rs#L2165-L2202
[t-raise]: ../../../../crates/daemon/src/config.rs#L1314
[t-gate]: ../../../../crates/daemon/src/config.rs#L1654
[t-hostile]: ../../../../crates/daemon/src/config.rs#L1722
[t-panic-internal]: ../../../../crates/host-runtime/tests/dispatch.rs#L551
[t-panic-stderr]: ../../../../crates/host-runtime/tests/dispatch.rs#L603
[t-panic-child]: ../../../../crates/host-runtime/tests/dispatch.rs#L631-L660
