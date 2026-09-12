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
sufficient. The discovery inventory imports no historical exercise claim and
runs no checks. Dated implementation evidence below is separate from that audit.
This is a working-tree inventory against the source baseline. Each section
ends with an explicit "none found" list and the areas the lenses flagged as
suspiciously quiet, so an omitted category is not mistaken for absent tests.

## Ingress admission and decode

| Check | Source condition or assertion | Status |
| --- | --- | --- |
| [`request_byte_cap_widens_for_transform_class_only`][t-cap] | `enforce_request_byte_cap` class set, `kind` beside `method`, non-string fallback, unparseable refusal, structured and 2 MiB `method`, a 2 MiB over-deep transform-class body admitted, a body of exactly 32 MiB admitted and one byte more refused. | unaudited |
| [`entry_probe_reads_the_route_and_the_page_envelope_as_dispatch_does`][t-probe] | The probe resolves `method` then `kind` as dispatch does, counts every page key with `null`, keeps the last repeated key, splits an overlong `method` between cap widening and route, refuses non-object and malformed bodies, probes an over-deep body that `tree_decode_parses` refuses, and keeps an escaped discriminator off the direct lane while `lane_probe` skips a body with no literal `transform`. | unaudited |
| [`tree_parse_witness_refuses_what_the_tree_refuses`][t-witness] | `tree_decode_parses` agrees with a `Value` parse on scalar, array, string, out-of-range, lone-surrogate, trailing, malformed, and empty bodies; refuses an object holding the raw-value token as any key wherever it sits, including the shapes the tree accepts, and shows the tree's `from_value` re-read refusing the token after another key. | unaudited |
| [`raw_value_token_matches_serde_json`][t-token] | The pinned `RAW_VALUE_TOKEN` literal is the key a `Value` parse reads as a boxed raw document: a document string decodes to that document, a non-string or a following key is refused, a later position is an ordinary key. | unaudited |
| [`direct_and_tree_transform_decodes_agree_on_the_corpus`][t-decode-diff] | `tree_decode_parses` implies the tree parses the body; both decodes give one request where both accept (nineteen bodies pinned); only repeated keys are tree-only; only derive-lenient shapes under an ignored field or the discriminator and retained values holding the token after another key, six of them raw-value-token shapes, are direct-only, each refused by `tree_decode_parses`; a two-page assembly decodes to the one-slice request. | unaudited |
| [`unpaged_transform_bodies_reach_the_same_outcome_through_both_entry_paths`][t-entry-diff] | Every corpus body gives the same response or the same code and message through `dispatch_body` and through the tree dispatch; the eleven direct-lane bodies are pinned and the valid body is served. | unaudited |
| [`parse_charge_covers_dense_native_typed_decode_peak`][t-peak] | The metered footprint covers the tree decode's heap peak, the direct decode peaks at or below it, and the direct lane's own metered count covers its peak with the values under an ignored field. | unaudited |
| [`parse_charge_covers_escaped_text_direct_decode_peak`][t-peak-escaped] | The metered footprint covers the direct decode's peak on a 4 MiB and a 64 KiB text block holding one escape, and on the 4 MiB block without one. | unaudited |
| [`byte_cap_admits_a_facade_sized_body_without_body_proportional_allocation`][t-cap-alloc] | A 900 KiB body whose one key holds an escape passes the byte cap allocating less than half its size. | unaudited |
| [`a_held_pool_stops_the_direct_lane_walk_before_it_unescapes_a_large_string`][t-held] | Against a reserve that grants nothing, the metered walk that gates the direct lane is refused as transient at its first value and allocates under 64 KiB for a body holding a 4 MiB escaped text block. | unaudited |
| [`request_observed_to_handler_ends_before_the_typed_decode`][t-observed] | A typed decode begun ten seconds before the handler is not counted in `request_observed_to_handler`. | unaudited |
| [`decode_footprint_counts_values_and_retained_string_copies`][t-footprint] | Separators inside strings are not values; a 1 MiB text block is charged three times; a scalar-dense body's footprint is far above its wire size and string bytes are not charged as values; a cut body counts its decoded part. | unaudited |
| [`metered_decode_charges_incrementally_and_refuses_above_capacity`][t-meter] | A fitting decode holds at least its footprint and under two steps more; a footprint above the capacity is refused as permanent with every byte released; a restarted meter reuses held bytes. | unaudited |
| [`a_small_body_holds_no_more_than_twice_its_footprint`][t-small] | A body under one mebibyte holds at least its footprint and at most twice it. | unaudited |
| [`a_nearly_drained_pool_is_charged_in_a_bounded_number_of_acquisitions`][t-acquire] | A body decoded against a pool with less than a batch free is refused as transient in under a hundred reservation attempts. | unaudited |
| [`a_capacity_bound_count_stops_at_the_value_that_crosses_it`][t-count] | The count-only decode against a capacity stops within one node of it and agrees with the footprint on either side. | unaudited |
| [`footprint_of_counts_what_the_value_decode_charges`][t-footprint-oracle] | The count-only footprint equals the meter's count under a `Value` decode on the raw-value token in first and later positions, with a string, a non-string, and a following key, and on escaped strings. | unaudited |
| [`footprint_floor_counts_the_values_the_meter_visits`][t-floor] | The byte-derived floor equals the footprint on string-free bodies, including nesting to the depth limit, is the footprint less the retained string copies and the longest escaped string's scratch charge on a body with strings, and its refusal agrees with the footprint on either side of the capacity. | unaudited |
| [`footprint_floor_never_exceeds_the_decoded_footprint`][t-floor-corpus] | For every well-formed corpus body the floor is at most the decoded footprint. | unaudited |
| [`a_doomed_body_is_refused_without_touching_the_pool`][t-doomed] | A paged and an unpaged body of two hundred thousand values are refused as too large through `dispatch_body` with the pool never asked and no value counted. | unaudited |
| [`both_lanes_charge_the_same_footprint_and_refuse_the_same_bodies`][t-lanes] | Every corpus body, one with twenty thousand values under an ignored field among them, gives one outcome and one counted footprint through both lanes with a pool one byte short and a pool that just fits; the short pool refuses as too large. | unaudited |
| [`a_drained_pool_refuses_a_fitting_body_as_transient_and_records_the_shortfall`][t-drain] | A held charge makes a fitting body's decode a transient refusal that releases its bytes, records the meter's shortfall marker, and returns `queue_full`; the body decodes once the holder releases; a transient refusal of a body the pool could never hold is too large. | unaudited |
| [`a_refused_decode_has_no_dispatch_side_effect`][t-effect] | A permanent and a transient refusal through `dispatch_body` return the prior codes and leave the handler's route table and the store row untouched, checked directly on the tested route; the body is served with room and only then binds the route. | unaudited |
| [`refused_bodies_emit_one_terminal_and_leave_no_dispatch_state`][t-ring] | Through the direct-host fixture, a body over each byte cap and two dense bodies the byte-derived floor refuses before either decode each return `host.invalid_params` to the managed client, which settles on the first terminal and drops a later one; the session shows no pass trace or row; a transform is then served and counted once. In-decode refusal and charge release are not exercised at ring level. | unaudited |
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

None found: a ring-level test that drains the scratch pool and asserts
`queue_full` (the fixture's pool is the fixed `SCRATCH_RESERVED_BYTES` and the
handler exposes no barrier); a test that counts string copies retained by the
typed decode. The `request_too_large` assertion in
[direct_host.rs][fixture] tests the fixture's line-based control channel,
not the handler.

## Shared-input equivalence

| Check | Source condition or assertion | Status |
| --- | --- | --- |
| [`assert_message_projection_equivalent`][assert-prefix] | Incremental and full projection equal by `differential_bytes` and by value; production calls run under [`prefix_projection_differential_enabled`][gate-prefix], which is enabled in tests or by `EIDNARA_PREFIX_PROJECTION_DIFFERENTIAL=1`. The test-only slice adapter `assert_prefix_projection_equivalent` delegates to this shared check. | unaudited |
| [`incremental_projection_reuses_prefix_storage_and_preserves_tool_arc_state`][t-inc] | Reattached prefix equals `from_parts` inputs; incremental equals full; prefix block backing and byte allocations are pointer-shared. | unaudited |
| [`incremental_projection_checks_effective_synthetic_status`][t-synthetic-status] | Across all four cached/current effective synthetic-flag pairs, incremental and full projections agree; prefix block backing, shell Arcs, and byte allocations are shared exactly when flags agree. | unaudited |
| [`compaction_mode_projection_cache_reclassifies_synthetic_prefix`][t-compaction-cache] | Direct off/on/on/off/on changes to `ProducerContext.compaction_enabled` drive real transforms and the handler's cache lookup/store helpers. Checks full-projection equality, reattached flags, prefix `wire` reuse on the repeated on pass, and unchanged ingress bytes. On enabled passes, `(projection_reused_messages, projection_projected_messages)` is `(0, 4)` after a mode change and `(4, 0)` on stable reuse. No route binding or config reload. | unaudited |
| [`reattach_keeps_block_level_original_but_rebuilds_the_message_shell`][t-reattach] | Reparsed input has nonempty origin and provider extras plus non-default, non-synthetic harness metadata. Replay drops only the unknown message-level field; transport identity, all known shell fields, and block originals remain equal. The original unknown-field expectations are retained. | unaudited |
| [Repeated canonical-shell reattachment][shell-sharing] | Reattachment, cloned requests, full projection of canonical input, and incremental projection preserve pointers, projection bytes, digests, and identities. Unknown message fields are discarded without changing raw ingress; block originals survive. Copy-on-write edits leave the cached shell unchanged. | unaudited |
| [Shared ingress decode and mobility][shell-decode] | Request messages and projections satisfy `Send + 'static`; malformed message arrays return the same errors as owned ingress arrays. | unaudited |
| [Projection shell allocation accounting][shell-charge] | An independent sum covers shell and block backing, retained block JSON, Arc counters, content capacity, identities, and frontiers with exact equality. Nonzero origin, provider namespace/tree/value, and harness metadata heap terms are computed without the cached-charge helper. | unaudited |
| [`projection_differential_catches_corrupt_first_changed_position`][t-projdiff] | A corrupt frontier is caught by the differential. | unaudited |
| [`astro_scale_projection_cache_reuses_on_the_second_pass`][t-astro] | The second pass reuses the cached projection. | unaudited |
| [`pending_rewrite_passes_isolate_ingress_meta_usage_and_reconcile`][t-pending] | Pass reads of ingress meta are isolated. | unaudited |
| [`warm_cache_selection_bust_does_not_replay_collapsed_synthetic_todo_as_live`][t-collapsed] | A replayed pair without the flag yields no duplicate tool-use id and reuses the cache. | unaudited |
| [synthetic_ingress_matches_flagged_reference][synthetic-reference] | Fresh, pending and lineage cases preserve canonical bytes, digests/projection state, native bytes and tag rows against typed-flagged input. Complete boundary diagnostics and chunk inputs agree with the same original handler request. A carrier-targeted overlay cannot mutate the synthetic message; a live control does take its tag. | unaudited |
| [handler_delta_normalization_matches_full_when_reserved_todo_starts_at_frontier][synthetic-delta-parity] | Full and delta requests agree on projection and native bytes after expanding the native response delta. | unaudited |
| [unflagged_synthetic_delta_prepares_historian_and_native_output][synthetic-delta-witness] | A prior HARD pass freezes the pair; an unflagged suffix reaches the situation marker with a prepared firing and native serving. A third delta reuses the pair in its cached prefix. Captured producer prompts and native bytes match a typed-flag baseline reconstruction; boundary/chunk checks retain the difference from full raw ingress. | unaudited |
| [lineage_rebase_preserves_unflagged_synthetic_head][synthetic-lineage-rebase] | A non-subagent descent replay rebases a synthetic head from ordinal 1 to 11 without consuming a live ordinal. Projection marks, served bytes and fingerprints equal the typed-flag reference. | unaudited |
| [tag_overlay_guard_uses_the_pass_local_synthetic_view][synthetic-overlay-guard] | `apply_tag_overlay_to_message` takes its synthetic flag from the pass-local view. A normalized carrier rendered from the raw ingress clone keeps its bytes under a carrier-targeted overlay; a live control takes its tag. | unaudited |
| [`parked_p2_fingerprint_reuse_and_tag_frontier_match_baseline`][t-parked] | Reused `content_hash` fingerprints and `canonical_bytes` equal a full rehash for this text fixture, not universally for equal values with different serialized bytes. | unaudited |
| [Canonical shell and segment witnesses][served-shells] | Literal bytes, SHA-256 identity, measured length, and actual `Served` segment writes agree for original, latent-edited, typed, and block-edited shells. Unknown fields, Unicode keys, a large integer, fractions, and signed zero are preserved. | unaudited |
| [Canonical frozen-corpus witness][served-corpus] | Original and fully typed shells match the independent `to_vec(to_value(message))` reference and prepared-frame bytes. Pairwise block identity digests agree with structural equality across the corpus. | unaudited |
| [Fingerprint candidate witness][served-fallback] | Real projection receipts match the frozen structural reference on the same candidates in both signed-zero directions, with original and typed zeros, latent extras, unknown fields, and first-duplicate reuse. Reused and fresh zero fingerprints intentionally differ while served bytes agree. Separately poisoned receipts expose candidate choice and positional precedence, not byte-hash equality. Null block input is rejected. | unaudited |
| [Single-serialization counter][served-once] | Each nested counted `Serialize` implementation runs once. Nested scalar and decoded-key ordering checks preserve serde bytes and reject unsorted struct output. | unaudited |
| [Prefix and escaped key ordering][served-key-order] | Objects whose quoted or escaped key spellings order differently from their decoded strings (`"a"` against `"a b"`, `"\n"` against `"!"`, `\t` against `\u0001`, non-ASCII against ASCII) encode to the `serde_json::Value` reference from both sorted and reversed input. | unaudited |
| [Passthrough allocation witness][served-allocations] | Under a counting global allocator, a 65-block retained-original shell with 12 keys per block costs at most 8 allocation events per block over a 1-block shell while matching the `to_vec(to_value(message))` bytes. Decoding every key costs 12 per block. | unaudited |
| [Fallback source guard][served-source] | The constructor has no `to_value` round trip. The absent-position fallback has no linear `find`, has one lazy digest-index initializer, routes its selected candidate through `fingerprint_from_projected_wire` exactly once, and composes no receipt inline. | unaudited |
| [`served_fingerprint_block_ids_pin_flat_mid_index_format`][t-fpids] | Fingerprint block ids are `mid#index` and synthetic ids. | unaudited |
| [`transform_segments_preserve_existing_golden_bytes`][t-segments] | `Exact` segments concatenate into the golden body. | unaudited |
| [`incremental_native_cache_replays_complex_prefix_and_encodes_only_tail`][t-native-inc] | Real tail expansion shares native values and sidecar metadata. Fresh, reattached, and shared replay produce equal native bytes and projected identity data. At most two served tail messages are encoded; prefix chunks retain pointer identity. The warm request charge equals a fresh walk using the same size estimator, independently of cached charges. The compiled test setting enables the native differential; the negative controls below verify detection. | unaudited |
| [`native_delta_ingress_core_is_independent_of_changed_output_messages`][t-native-ingress] | Equal but separately allocated ingress/output values share output chunks. Changed output cannot replace raw ingress. Snapshot fallback retains request pointers after native-cache eviction. Cached request charges match capacity-based walks including Arc headers. | unaudited |
| [`native_cache_charge_keeps_raw_allocation_floor_beside_sidecar_estimate`][t-native-charge-floor] | A scalar-dense raw allocation exceeds its sidecar serialized-size estimate, so the shared raw pointer retains its ingress allocation charge. Full and degraded sidecars preserve ingress/output pointer deduplication, while equal values in distinct allocations retain distinct charges. Request charges remain independent. | unaudited |
| [`differential_assert_rejects_frontier_inside_mutated_native_region`][t-native-reject] | The differential panics on a corrupt native frontier. | unaudited |
| [`frontier_vacuity_covers_opaque_repeats_eviction_and_same_length_edits`][t-vacuity] | Same-length edits and repeats are not vacuously reused. | unaudited |
| [`duplicate_tool_use_assert_covers_incremental_native_suffix`][t-dup] | Tool-use ids are unique across the cached prefix and encoded suffix. | unaudited |
| [`incremental_sidecar_carries_pins_across_three_generations`][t-sidecar] | Full and incremental order, metadata, and pins agree across three generations and repeated IDs. Sparse-prefix cases preserve order and missing metadata. | unaudited |
| [`selection_input_shares_projected_wire_value`][selection-sharing] | Selection and historian inputs equal and point to the projected wire input; cloning selection preserves that pointer. | unaudited |
| [`tag_baseline_cache_matches_cold_passes_across_drop_reset_and_remint`][t-tagcold] | Cold and cached passes serve equal bytes and equal durable rows across five passes. | unaudited |
| [`poisoned_tag_baseline_refills_after_direct_sql_update`][t-poison] | A generation change refills the baseline. | unaudited |
| [`tag_baseline_cache_keeps_interleaved_sessions_isolated`][t-interleave] | Two sessions do not share rows. | unaudited |
| [`tag_mint_tail_and_hygiene_share_baseline_rows`][t-tag-sharing] | Baseline and mint sources remain byte-equal and retain row/source pointers in the combined view and hygiene output. | unaudited |
| [`failed_tag_mint_commit_preserves_baseline_and_rolls_back_store`][t-tag-rollback] | The second mint insert aborts. The cache retains its baseline pointer and contents; durable tags, generation, core, meta, row version, and temporal rows roll back. Successful retry does not publish pass rows; explicit refill reads committed sources. | unaudited |
| [`tag_baseline_charge_counts_capacity_and_shared_row_headers`][t-tag-charge] | Spare row capacities, row handles, and row/slice Arc headers are charged. Shared rows are charged in full. | unaudited |
| [`tag_baseline_cache_refuses_an_insert_larger_than_its_budget`][t-tag-refusal] | An oversized insert is refused. A replacement with spare source capacity removes the old entry and charge while loaded rows remain usable; readmission charges once. | unaudited |
| [`claude_code_first_requested_surface_tags_bootstrap_pass_one`][t-tag-bootstrap] | Initial active minting and replay preserve rendered bytes. | unaudited |
| [`newest_tag_block_set_isolates_protected_and_applied_pending_rows`][t-tag-protection] | Bootstrap mints tag 29 without displacing stored tag 5 from protection. Stored rank 21 is dropped, while rank 20 and the second block at the newest stored ordinal stay pending. | unaudited |
| [`measurement_is_identical_with_cold_and_warm_token_cache`][t-hyg-cold] | Cold and warm memo output is identical; warm memo reuse skips token-cache lookup. A fresh memo over the warm token cache preserves the full result with three lookups, three hits, and no tokenization. Clearing both caches preserves output. | unaudited |
| [`parity_golden_matches_ts_reference_across_full_corpus`][t-hyg-golden] | U and T match the TypeScript golden within tokenizer tolerance; the band matches exactly. Cold/warm full results equal the Rust characterization digest. Its pre-memo provenance is agent-witnessed and transcript-only, not independently reexecuted or artifact-hash verified. | unaudited |
| [`hygiene_digest_and_token_key_use_kind_prefixed_content`][t-hyg-key] | A poisoned projection-digest token entry is not reused. The independent text-prefixed digest is the reported hash and token key. | unaudited |
| [`memo_preserves_each_derived_digest_domain`][t-hyg-domains] | Independent text, input, output, file, and excluded digest formulas match cold and warm memo entries and differ from projection hashes. | unaudited |
| [`memo_rechecks_caveman_identity_payload_and_context`][t-hyg-invalidates] | Same-ID content edits, caveman identity/payload and first-duplicate selection, tags, protection, coverage, reduction, role, and synthetic status preserve fresh-measurement equality. | unaudited |
| [`memo_bounds_sessions_bytes_and_refuses_over_budget_blocks`][t-hyg-bounds] | Interleaving, removal, oversize session IDs, over-budget multiblock walks, and empty projections preserve complete results. Prune, reinsert, replacement, and refusal preserve recomputed counters under the production capacity-to-bucket model, not an allocator measurement; refused blocks are counted. | unaudited |
| [`memo_over_budget_keeps_a_warm_prefix_instead_of_resetting`][t-hyg-prefix] | A projection larger than the budget admits the projection prefix; every later walk hits exactly that prefix and retains the same keys under budget. | unaudited |
| [`memo_retention_is_independent_of_caveman_payload_size`][t-hyg-payload] | A 16-byte and a 2 MiB caveman payload memoize with identical retained bytes. | unaudited |
| [`memo_namespaces_are_separate_and_the_least_recently_used_session_is_evicted`][t-hyg-table] | Namespace A/B/A keeps three separate warm memos; namespace and ID-only removal drop only their matches; a seventeenth session evicts the least recently used one while a recently touched one survives. | unaudited |
| [`poisoned_session_memo_recovers_cold_through_every_path`][t-hyg-poison] | A panic with torn accounting is recovered by use, namespace removal, ID-only removal, and reset; results equal uncached measurement and the table stays under its bound. | unaudited |
| [`distinct_sessions_neither_block_nor_evict_each_other_up_to_the_limit`][t-hyg-overlap] | Sixteen distinct sessions reach a channel barrier while all hold their memo locks; each stays resident and warm afterwards. | unaudited |
| [`all_memo_sessions_near_budget_match_independent_retained_accounting`][t-hyg-pool-bound] | All sixteen sessions refuse part of a 4,000-block walk. Recomputed string, bucket, table, and per-session allocation charges match counters and the status metrics using the same capacity-to-bucket model as production. The bound is 16 MiB plus fixed containers after operations, not peak allocation, allocator RSS, or independently verified hashbrown layout. | unaudited |
| [`production_transform_reuses_hygiene_memo_and_recounts_only_edited_block`][t-hyg-production] | An isolated process runs the production transform entry: cold 0/3 hits/misses, unchanged 3/0, single-edit 2/1; the parent asserts the child ran exactly one test. | unaudited |
| [`module_status_memory_metrics_match_budget_accounting_and_falsy_semantics`][t-hyg-status] | After a shared memo clear, `status` reports bounded `tail_hygiene_memo` charged bytes and unsigned session-count and refused-insert fields alongside the other retention classes. Parallel tests may repopulate the table; private-table tests check exact populated counts. | unaudited |
| [`shared_row_iterator_matches_slice_for_protected_legacy_orphan`][t-hyg-iterator] | Arc-row iterator and original slice measurements agree exactly on the frozen orphan fixture with two protected tags; orphan tag 2 has nonzero T and zero U. | unaudited |
| [selection_differential.rs][t-seldiff] | Optimized selection equals the frozen reference over generated `SelItem`s. | unaudited |

None found: a captured production delta body replaying a synthetic pair;
a test that runs either differential gate from `crates/daemon/tests/`
or `crates/daemon/benches/` (`cfg!(test)` is false there and no file sets the
variables).

The [B4 evidence](evidence/hygiene-digest-is-kind-prefixed-part-content.md)
separates the release-accessor prerequisite repair from the transcript-only
pre-memo characterization. The [historical payoff receipt](evidence/tail-hygiene-payoff.md)
records three A/A pairs and five A/B pairs, without retries or discarded runs.
It meets the ticket-local rule with a 73.1659% warm-call time reduction. Median
and p95 describe batch-average call times, not individual-call latency. The
external collector retains build, host, commands, hashes, and raw receipts;
this is not an automated CI performance guarantee. The controller reports all
14 local gates passed, with logs at `/tmp/opencode/hygiene-memo-*.log`.
No gates or benchmarks run during this documentation update.

Merge-integration checks are separate from that historical receipt. The
recorded candidate is `e1a0d06a`, before integration with parent `16542f5e`.
The merged [hygiene cell][hyg-bench-loop] uses [decoded ingress][hyg-bench-input]
with retained original JSON instead of direct typed construction. Memo
construction and priming remain outside the timed callback. The old 73.1659%
result does not establish a gain for the merged workload. No benchmark was
rerun during integration. The focused run passes 15 hygiene tests, four
differential-golden tests, and the real-transform memo-hit test. Workspace
all-target/all-feature clippy and formatting checks pass. The hygiene count
decreases because parent test deduplication removes the standalone fixture
mutation guard; the frozen corpus and digest-domain checks remain.

The separate [integrated payoff receipt](evidence/tail-hygiene-integrated-payoff.md)
resolves the merged-workload timing gap on `05c33bf0`. Archived parent
`16542f5e` plus only the required release-accessor repair supplies A. Both
arms use decoded ingress with retained original JSON. Three new A/A pairs
precede five A/B pairs under the same fixed rule; all 16 processes are valid,
with a 72.5513% warm-call time reduction and a 2.0888% A/A median span.
The conditional paired interval and complete before/after table are in the
receipt. This does not replace or generalize the historical 73.1659% result.
The controller reports 14 gates passed on `05c33bf0`, with outputs at
`/tmp/opencode/memo-integrated-*.log`. Log inspection corroborates recorded
outputs, not an independent source-bound exit receipt for every gate.
No tests, builds, or benchmarks run for this documentation update, and test
adequacy remains unaudited. No production, concurrency, cold-call, or RSS
payoff is established.

The shared-shell campaign extends the complex native replay with request-shell
pointer checks, fresh/reattached/shared projection equality, served-byte equality,
native-output copy-on-write isolation, and independent cold-shell accounting.
The warm charge keeps cached prefix sizes and measures the suffix. The
ingress-core test evicts both projection and native entries to prove that the
snapshot fallback shares raw ingress shells. These are local test observations,
not performance measurements or a full-workspace gate.

## Cache-state load, pass trace, side channel, and meta preparation

| Check | Source condition or assertion | Status |
| --- | --- | --- |
| [`no_fire_reason_is_durable_change_gated_and_cleared_by_fire`][t-no-fire] | The post-commit load feeds the `record_no_fire` CAS; a repeated reason leaves `row_version` unchanged. | unaudited |
| [`handler_emergency_refolds_when_active_run_publishes_before_live_wait_capture`][t-emergency] | A publication between transform and prepare, injected through [`between_transform_and_prepare`][hook], triggers the rerun; the hook records the transform's and the publish's committed row versions and asserts their order. | unaudited |
| [`handler_delta_boundary_divergence_recut_retries_cas_without_stale_projection`][t-cas] | A CAS conflict reruns `apply_once` with a fresh load. | unaudited |
| [`transform_snapshot_resists_commit_between_state_and_overlay_reads`][t-snap-resist] | One read transaction pins `cache_state` and the overlays. | unaudited |
| [`transform_snapshot_keeps_row_version_and_overlays_from_one_commit`][t-snap-keeps] | The snapshot `row_version` matches overlays from the same commit. | unaudited |
| [`transform_cas_conflict_leaves_every_overlay_table_empty`][t-cas-empty] | The CAS loser commits no overlay rows. | unaudited |
| [`competing_pass_counter_survives_direct_primary_lifecycle_and_reopen`][t-counter] | The CAS loser is rejected; state survives reopen. | unaudited |
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
| [`a_read_callback_cannot_lower_fence_durability`][t-sync] | `synchronous=FULL` is pinned on open and re-pinned by the first fenced write after a maintenance callback. | unaudited |
| [`meta_scalar_reads_agree_with_the_full_deserialization`][t-scalar] | Each scalar `meta` read and the `meta`-only load equal the full deserialization where it succeeds and fail where it fails on `meta`; a corrupt `core_state` fails only the full load. | unaudited |
| [`cache_state_full_load_counters_key_on_the_prepared_statement`][t-counters] | The full-select run and eviction counters key on the prepared statement text; a cache of one alternating two statements records one re-creation. | unaudited |
| [`a_steady_pass_loads_meta_once_before_the_transform_and_the_full_row_once_after`][t-load-count] | A steady pass runs the `meta` select once and the full select never before the transform, and the full select once after the commit, split by the interleave hook, on handles that were never evicted. | unaudited |
| [`pass_state_load_has_its_own_timing_bucket`][t-timing] | The pre-transform `meta` load is reported as the `pass_state_load` pass-trace bucket, present and non-zero on a steady pass. | unaudited |
| [`single_pass_preparation_reports_change_and_validates_unwalked_keys`][t-single-pass] | Clean input returns byte-identical with `changed` false; a substitution sets `changed` and records one detection; a secret-bearing key under an integrity- or identity-named container is refused. | unaudited |
| [`a_refusal_after_a_substitution_leaves_its_detection_in_the_callers_vector`][t-refusal-order] | The `keyed` store fixture, serialized and prepared directly, is refused with one detection in the caller's vector, so the store test's receipt count discriminates. | unaudited |
| [`cache_state_meta_is_stored_byte_identical_when_clean_and_scanned_to_every_nested_key`][t-meta-bytes] | Through `commit`: clean `meta` is stored as its serialization; a nested map value secret is substituted and recorded on the `meta` scan; a nested map key secret, preceded in walk order by a substituted value, is refused with no row and no added receipt. | unaudited |
| [`settled_pass_scan_audit_rows_are_retired_while_overlay_scans_remain`][t-retire] | Audit row counts stay flat across six passes, and again across passes after a historian publish bumped the row version; a tag mint's scans are added and kept; the pass owner, the retained owner, and the publish owner each hold exactly their own copies. | unaudited |
| [`a_compartment_generation_conflict_keeps_the_live_pass_scan_audit_rows`][t-seq-conflict] | A pass that loses the compartment-generation check retires nothing; the live pass's rows and owner copies are unchanged. | unaudited |
| [`the_first_receive_for_a_new_session_records_its_identity_receipt`][t-first-receive] | A receive for a session absent from `pass_trace` and `cache_state` records one zero-finding scan and one owner copy; the next receive records nothing. | unaudited |
| [`a_clean_pass_trace_receive_records_no_scan_audit_rows`][t-receive] | A clean receive records no audit row; a detected identity on a known session still does. | unaudited |
| [`retained_pass_fields_keep_their_scan_receipts_across_the_next_pass`][t-retained-pass] | A second pass keeps the first pass's receipts for its root, scheduler observation, interesting observation, and divergence while both roots, both history entries, and the divergence stay stored; the replaced `meta` keeps one live receipt; a third pass adds to the retained receipts. | unaudited |
| [`retained_history_receipts_are_evicted_with_their_ring_entries`][t-ring] | Across 296 passes the retained receipt count for `scheduler_observation` and `scheduler_interesting` equals each ring's length, so the audit rows stay bounded by the two full rings plus the live pass. | unaudited |
| [`re_observed_roots_and_replaced_divergences_keep_one_retained_receipt_each`][t-root] | Five passes repeating one root and one divergence keep one retained root receipt and one divergence receipt; a second root adds one. | unaudited |
| [`a_fingerprint_without_an_interesting_entry_keeps_no_retained_receipt`][t-fingerprint] | Five passes with a fingerprint and no interesting entry hold only the live pass's fingerprint scan; a pass that writes the interesting entry keeps one retained receipt. | unaudited |
| [`a_fingerprint_receipt_is_evicted_with_its_interesting_entry`][t-fingerprint-evict] | One fingerprint-bearing interesting entry followed by 256 without one leaves zero fingerprint receipts once the entry is evicted. | unaudited |
| [`an_oversized_fingerprint_neither_keeps_nor_evicts_a_history_receipt`][t-fingerprint-bound] | A fingerprint over the diagnostic bound leaves the stored fingerprint's receipt in place, holds only the live pass's scan, and is retired by the next pass. | unaudited |
| [`observation_receipts_follow_the_ring_across_both_writers`][t-two-writers] | A commit's observation receipt leaves when 256 stable passes push its entry out; the ring and the receipts across both writers count 256. | unaudited |
| [`a_stale_delivery_does_not_retire_a_re_created_outbox_row`][t-key-reuse] | A handle read before the row was deleted and re-issued under the same key with another payload reports already retired, delivers nothing, and leaves the new row pending. | unaudited |
| [`side_channel_payloads_are_parsed_before_the_fenced_delivery`][t-parse-first] | With another connection holding the write lock, a malformed payload fails as a parse error in under a second rather than waiting on the lock. | unaudited |
| [`reassigning_a_scan_range_leaves_later_scans_under_the_default_owner`][t-reassign] | Reassigning a scan range to another owner leaves the write's default owner list alone, so a scan prepared afterwards carries only the default owner. | unaudited |
| [`a_crash_between_delivery_and_retirement_redelivers_once_under_concurrent_drainers`][t-side-channel-crash] | An injected failure between insert and retirement rolls both back; rows read by a second drainer before the first retired them deliver nothing a second time; the outbox ends empty rather than marked; the C6 marker is recorded per kind. | unaudited |
| [`pass_trace_counts_every_outcome_and_a_failed_receive_does_not_veto_the_commit`][t-outcome] | Rejected, committed, and stable passes count three receives; a receive whose UPSERT fails inside its own transaction leaves the commit intact. | unaudited |
| [`historian_active_reads_the_durable_phase_from_the_pass_state_or_the_store`][t-phase] | `historian_active` reads the phase from the pass load, from the store on a rerun, and treats a failed load as idle. | unaudited |
| [`historian_active_rereads_a_loaded_active_phase_when_no_run_is_live`][t-phase-reread] | A loaded non-idle phase with no live run is re-read from the store; a run that committed idle after the pass load does not report active. | unaudited |

None found: `first_divergence`
NULL after a rejected pass; `receive_count` after an Emergency95 rerun that
commits twice;
outbox ordering across firings, the per-kind limit, or the backoff values.

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
| [rust-mode-transform.test.ts:384][permission-provisional] | Provisional availability sends `tool_present: false` and `todo_tool_present: false`. | unaudited |
| [rust-mode-transform.test.ts:447][permission-agent] | An agent `deny` rule through the SDK yields `todo_tool_present: false`; `app.agents` is called once. | unaudited |
| [rust-mode-transform.test.ts:473][permission-empty-timeout] | A hung `app.agents()` with an empty cache yields `todo_tool_present: false` after about 2 s. | unaudited |
| [hook-handlers.test.ts:126][permission-capture-timeout] | A hung permission read suppresses capture within the 2 s deadline. | unaudited |
| [ctx-reduce-availability.test.ts:241][permission-evaluator] | `permissionDisabled` last-match semantics, session overlay after agent rules, wildcard escaping, and distinct active-agent inputs. | unaudited |
| [hook.test.ts:167][permission-witness] | Rejection and fake-time timeout after a stored deny reach the constant P5 marker for transform and capture; outcome checks require false wire verdicts and no capture. The marker establishes reachability, not a distinct fallback outcome. | unaudited |
| [hook.test.ts:255][permission-hit] | Transform and capture share fresh hits without another SDK permission read; missing-client cases suppress both. | unaudited |
| [hook.test.ts:302][permission-lifecycle] | Session update, native compaction, and flush invalidate every agent for one session; deletion clears entries; another session stays fresh. An overlapping capture allow cannot clear a newer transform deny. | unaudited |
| [hook.test.ts:425][permission-overlap] | Real transform and capture hooks share one fill. Empty host agents normalize to absence: the agent-list API is not called, session allow/deny rules decide, and undefined or empty capture reuses the same key. | unaudited |
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
| [module-wire.test.ts:1310][t1308] | Unpaged `bytes` equals a later `JSON.stringify` length. | unaudited |
| [module-wire.test.ts:1373][t1371] | Each paged `bytes` equals a later `JSON.stringify` length. | unaudited |
| [module-wire.test.ts:1393][t1391] | The pageable array field list matches the daemon's Rust literal. | unaudited |
| [rust-mode-transform.test.ts:540, 578, 617][tpaged] | A paged series re-pages after `need_full_sync`; it restarts on attempt mismatch and reconnect. | unaudited |
| [frame-channel.test.ts:181, 193][t181] | The declared byte length equals written bytes for lone surrogates, including across a segment boundary. | unaudited |
| [Joint pager and native-writer fake][serialized-writer] | Frozen corpus checks carried UTF-8 bytes against the raw header and captured byte array through the real module transport, client encoder, and writer. Exact first/final lengths and page counts are asserted. | unaudited |
| [Module transport snapshot][serialized-transport] | A getter changes on a second read, source and inspection values mutate, and a stringify spy rejects any send-time serialization. The public connection factory and channel injection exercise the transport without private-field assignments. | unaudited |
| [Generic carrier encoding][serialized-client] | Plain Pi-style objects with colliding field names or symbol descriptions remain ordinary objects; generic client snapshot encoding retains authoritative text after nested inspection edits. | unaudited |
| [Unpaged boundary][serialized-unpaged] | `2 ** 64`, raw `1e5`, and raw `-0` remain single unpaged requests at 524288 wire bytes despite larger Rust reserialization. | unaudited |
| [Pager serialization spy][serialized-pager] | The full input and every emitted envelope serialize once. `toJSON` runs once. Digest and packing serializations remain outside that claim. | unaudited |
| [Live transform hook][serialized-hook] | The real hook passes a carrier with the matching transform/session discriminator. | unaudited |
| [Real-host corpus][serialized-host] | A registered Cargo integration test compiles in normal CI and is ignored until a generated corpus is supplied. The one-command wrapper requires one named pass. It verifies byte/hash receipts, twelve complete transforms equal to unpaged controls, nine staged pages, six unchanged surrogate refusals, one pager refusal, exact intermediate/final boundaries, and the 24-byte f64 witness. | unaudited |
| [rust-mode-transform.test.ts:1449, 1585][tinplace] | In-place mutation of an older message forces a full send; recovery after repeated rejection. | unaudited |
| [rust-mode-transform.test.ts:249][t244] | `rust pass:` and `rust module stages:` lines are emitted per pass (spy on `sessionLog.debug`). | unaudited |
| [Baseline logger.test.ts:358][t358] | Control characters are removed; entry size is bounded; no forged fourth line. | unaudited |
| [Baseline logger.test.ts:381][t381] | The default log is `0600` under `0700` directories; a planted symlink is not followed. | unaudited |
| [Baseline logger.test.ts:342][t342], [408][t408] | Swallowed-write counter; exit flush without holding the process. | unaudited |
| [Gate and level ordering][log-level-checks] | Off and below-threshold calls perform no caller inspection, sanitizer iteration, serialization, timestamp, timer, or file work. The level table is frozen and info methods are distinct from plain functions. Every minimum and invalid defaults preserve exact plain/session output. | unaudited |
| [Admitted-entry cleanup][log-flush-checks] | Off preserves explicit and exit flushing; one failed batch counts once, even after a second empty flush. | unaudited |
| [Default and info hardening][log-hardening-checks] | Both minima check exact 2048-character payloads plus ellipses, sanitization, modes, FIFO, serialization markers, swallow counts, and exit behavior. Managed-directory symlink and simulated foreign-uid failures must identify the corresponding refusal, not just increment the counter. | unaudited |
| [Real transform/event logging][log-hook-checks] | Actual handlers run at debug, warn, and off. Exact pass/event line counts, warning-only filtering, no off writes, and byte-identical served/fallback output are asserted without replacing the logger. | unaudited |
| [event-handler.test.ts:272-524][tevent] | `message.updated` usage bookkeeping; no assertions on log lines. | unaudited |

None found: a test asserting secret redaction of log lines on this path.
The logger does not apply secret redaction; P4 preserves that boundary.

The [2026-09-11 local host probe](evidence/paged-body-measure-equals-declared-frame-length-and-fits-host-caps.md#q-what-does-the-real-host-admission-probe-establish)
constructs the boundary under both measures and observes real host refusals.
It is historical diagnostic execution, not a passing P3 check. The
[scoped carrier campaign][serialized-campaign] records the approved refusal
oracle and executable checks. Native-writer fakes do not prove actual addon
attachment; that mechanism is unavailable on the tested Bun and Node runtimes.

[serialized-writer]: ../../../../packages/opencode-plugin/src/hooks/context/module-wire-frame.test.ts#L55
[serialized-transport]: ../../../../packages/opencode-plugin/src/hooks/context/module-wire-frame.test.ts#L97
[serialized-client]: ../../../../packages/opencode-plugin/src/shared/host-client/client.test.ts#L108-L175
[serialized-unpaged]: ../../../../packages/opencode-plugin/src/hooks/context/module-wire.test.ts#L1509
[serialized-pager]: ../../../../packages/opencode-plugin/src/hooks/context/module-wire.test.ts#L1427
[serialized-hook]: ../../../../packages/opencode-plugin/src/hooks/context/hook.test.ts#L1435
[serialized-host]: ../../../../crates/daemon/tests/serialized_transform_pages.rs#L11
[serialized-campaign]: evidence/paged-body-measure-equals-declared-frame-length-and-fits-host-caps.md#q-what-do-the-unpaged-correction-and-registered-cargo-test-prove

[permission-provisional]: ../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L384
[permission-agent]: ../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L447
[permission-empty-timeout]: ../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L473
[permission-capture-timeout]: ../../../../packages/opencode-plugin/src/hooks/context/hook-handlers.test.ts#L126
[permission-evaluator]: ../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.test.ts#L241
[permission-witness]: ../../../../packages/opencode-plugin/src/hooks/context/hook.test.ts#L167
[permission-hit]: ../../../../packages/opencode-plugin/src/hooks/context/hook.test.ts#L255
[permission-lifecycle]: ../../../../packages/opencode-plugin/src/hooks/context/hook.test.ts#L302
[permission-overlap]: ../../../../packages/opencode-plugin/src/hooks/context/hook.test.ts#L425
[permission-lifetime]: ../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.test.ts#L347

Stage logging and event line counts are exercised by the real transform/event
fixture in `logger.test.ts`. The two empty-cache hung-read tests assert
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
| [`production_transform_module_has_no_global_estimator_bypass`][t-bypass] | No direct tokenizer path in production `transform.rs`, including imports, SOFT, serialization, tag minting, and nudge derivation. This lexical scan does not cover the transitive handler call graph. | unaudited |
| [`soft_pressure_classification_matches_frozen_thresholds`][soft-threshold-check] | Forty-eight real SOFT evaluations match a frozen direct-tokenizer predicate; spies observe exact texts and three independent witnesses cross both pressure boundaries and neither. | unaudited |
| [`soft_pressure_absence_and_placeholder_preserve_estimator_gates`][soft-gates-check] | Missing m0 and placeholder m1 skip only their own measurement; cached counts preserve reference classification, and two warm inputs hit the cache. | unaudited |
| [`mature_tag_mint_filter_preserves_rows_and_tokenizes_only_new_sources`][tag-accounting-check] | Only new tags count; stored nudge rows do not recount; derived nudge counts match stored counts; an entirely tagged projection does no work. | unaudited |
| [`serialized_output_cache_reuses_steady_state_and_matches_fresh_bytes`][serialization-gate-check] | Fresh serialization and cached replay produce equal bytes without estimating tokens. | unaudited |
| [`preselection_never_drops_a_rule_whose_pattern_matches`][t-preselect] | A matching rule is always preselected (16 inputs). | unaudited |
| [`provider_canaries_return_stable_rule_ids_and_value_spans`][t-canaries] | Canary inputs yield stable rule ids and spans. | unaudited |
| [`minimal_fixture_is_truthful_and_executable`][t-qual] | A one-case qualification fixture with `authority_qualified: false`. | unaudited |
| [`evaluator_constants_are_pinned`][t-pinned] | Evaluator tables and constants digest are pinned. | unaudited |
| [`windows_start_on_line_boundaries_and_overlap_when_lines_are_short`][t-windows] | Redaction window placement and overlap. | unaudited |
| [`scanner_is_the_only_redaction_path`][t-only-path] | No redaction path bypasses the scanner. | unaudited |
| [`chunk_fingerprint_uses_id_kind_and_byte_length`][t-chunk-fp] | The length-only literal `id:kind:len`, empty input, explicit UTF-8 lengths distinct from UTF-16 units, and unescaped delimiters. | unaudited |
| [`historian_boundary_construction_matches_owned_reference`][t-boundary-construction] | Owned boundary reference versus borrowed IDs/shared bytes; length-only snapshots versus copied strings; exact frozen transcript, prompt, and raw-message bytes; empty, non-ASCII, tool, system, synthetic, excluded-tail, and oversized inputs. Unflagged reserved synthetic messages with no projected identity preserve the no-fire result. A matching frozen-size entry and pending drop check exact percentages against the owned reference, an independent formula, and a no-entry control. | unaudited |
| [`unflagged_synthetic_delta_prepares_historian_and_native_output`][t-firing-capture] | Scripted producer captures actual handler prompts on cached-prefix and reconstructed-prefix lanes. Full prompts compare across lanes and their SHA-256 values are pinned to the construction baseline. | unaudited |
| [`optimized_matches_frozen_reference_at_production_windows`][diff-prod] | Truncation bytes equal the frozen reference at production windows (24 cases, budget 1..32_001). | unaudited |
| [`exact_token_budget_returns_original_input`][diff-exact] | Input at budget returns unchanged. | unaudited |
| [`optimized_matches_frozen_reference`][diff-small] | Small-window byte equality. | unaudited |
| [`historian_chunk_golden_fixture_matches_builder`][t-golden] | Chunk output and every truncation case from `testdata/historian-chunk-golden.json`; each truncation input must exceed its budget before exact output comparison. | unaudited |
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
staleness, or two project roots sharing one `ConfigCache`; any test that aborts between
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

[testentry]: ../../../../crates/daemon/src/lib.rs#L12602-L12609
[fixture]: ../../../../crates/daemon/tests/direct_host.rs#L285-L290
[t-cap]: ../../../../crates/daemon/src/lib.rs#L19301-L19378
[t-dispatch]: ../../../../crates/daemon/src/lib.rs#L29053-L29108
[t-shape]: ../../../../crates/daemon/src/lib.rs#L33072-L33089
[t-shape2]: ../../../../crates/daemon/src/lib.rs#L33092-L33132
[t-envelope]: ../../../../crates/daemon/src/transform.rs#L16470-L16497
[t-probe]: ../../../../crates/daemon/src/lib.rs#L19381-L19439
[t-witness]: ../../../../crates/daemon/src/lib.rs#L19444-L19474
[t-token]: ../../../../crates/daemon/src/lib.rs#L19479-L19491
[t-decode-diff]: ../../../../crates/daemon/src/lib.rs#L19718-L19849
[t-entry-diff]: ../../../../crates/daemon/src/lib.rs#L19871-L19914
[t-peak]: ../../../../crates/daemon/tests/parse_charge_covers_typed_decode.rs#L103-L163
[t-peak-escaped]: ../../../../crates/daemon/tests/parse_charge_covers_typed_decode.rs#L178-L199
[t-held]: ../../../../crates/daemon/tests/parse_charge_covers_typed_decode.rs#L237-L261
[t-cap-alloc]: ../../../../crates/daemon/tests/parse_charge_covers_typed_decode.rs#L201-L222
[t-observed]: ../../../../crates/daemon/src/lib.rs#L21831-L21859
[bodyentry]: ../../../../crates/daemon/src/lib.rs#L12676-L12718
[t-footprint]: ../../../../crates/daemon/src/lib.rs#L20012-L20047
[t-meter]: ../../../../crates/daemon/src/lib.rs#L20053-L20086
[t-small]: ../../../../crates/daemon/src/lib.rs#L20090-L20107
[t-acquire]: ../../../../crates/daemon/src/lib.rs#L20112-L20140
[t-count]: ../../../../crates/daemon/src/metered_decode.rs#L1001-L1029
[t-footprint-oracle]: ../../../../crates/daemon/src/metered_decode.rs#L1031-L1052
[t-floor]: ../../../../crates/daemon/src/metered_decode.rs#L1054-L1092
[t-floor-corpus]: ../../../../crates/daemon/src/lib.rs#L20239-L20251
[t-doomed]: ../../../../crates/daemon/src/lib.rs#L20199-L20234
[t-drain]: ../../../../crates/daemon/src/lib.rs#L20146-L20194
[t-effect]: ../../../../crates/daemon/src/lib.rs#L20256-L20304
[t-lanes]: ../../../../crates/daemon/src/lib.rs#L19919-L19965
[t-ring]: ../../../../crates/daemon/tests/direct_host.rs#L437-L558
[t-meta]: ../../../../crates/daemon/tests/transform_meta_bound.rs#L21-L96
[directhost]: ../../../../crates/daemon/tests/direct_host.rs#L49-L128
[t-prep]: ../../../../crates/daemon/tests/prepared_output.rs#L103-L115
[t-budget]: ../../../../crates/host-runtime/src/wire.rs#L825-L865
[t-pools]: ../../../../crates/host-runtime/src/config.rs#L480-L503

[gate-prefix]: ../../../../crates/daemon/src/transform.rs#L2016-L2021
[assert-prefix]: ../../../../crates/daemon/src/transform.rs#L2031-L2046
[t-inc]: ../../../../crates/daemon/src/wire.rs#L1520
[t-synthetic-status]: ../../../../crates/daemon/src/wire.rs#L1817
[t-compaction-cache]: ../../../../crates/daemon/src/lib.rs#L38576-L38693
[t-reattach]: ../../../../crates/daemon/src/wire.rs#L1707
[shell-sharing]: ../../../../crates/daemon/src/wire.rs#L1749
[shell-decode]: ../../../../crates/daemon/src/wire.rs#L1796
[shell-charge]: ../../../../crates/daemon/src/wire.rs#L958
[t-projdiff]: ../../../../crates/daemon/src/lib.rs#L24253-L24294
[t-astro]: ../../../../crates/daemon/src/lib.rs#L22970-L23031
[t-pending]: ../../../../crates/daemon/src/transform.rs#L19406
[t-collapsed]: ../../../../crates/daemon/src/transform.rs#L27958
[synthetic-reference]: ../../../../crates/daemon/src/transform.rs#L27709
[synthetic-delta-parity]: ../../../../crates/daemon/src/lib.rs#L25102
[synthetic-lineage-rebase]: ../../../../crates/daemon/src/transform.rs#L28976
[synthetic-overlay-guard]: ../../../../crates/daemon/src/transform.rs#L27889
[synthetic-delta-witness]: ../../../../crates/daemon/src/lib.rs#L24821
[t-parked]: ../../../../crates/daemon/src/transform.rs#L13993
[served-shells]: ../../../../crates/daemon/src/transform.rs#L13714
[served-corpus]: ../../../../crates/daemon/src/transform.rs#L13760
[served-fallback]: ../../../../crates/daemon/src/transform.rs#L13797
[served-once]: ../../../../crates/daemon/src/served_json.rs#L171
[served-key-order]: ../../../../crates/daemon/src/served_json.rs#L218
[served-allocations]: ../../../../crates/daemon/tests/served_json_passthrough_allocations.rs#L68
[served-source]: ../../../../crates/daemon/src/transform.rs#L13942
[t-fpids]: ../../../../crates/daemon/src/transform.rs#L13604
[t-segments]: ../../../../crates/daemon/tests/prepared_output.rs#L32-L52
[t-native-inc]: ../../../../crates/daemon/src/lib.rs#L22290-L22606
[t-native-ingress]: ../../../../crates/daemon/src/lib.rs#L22608-L22728
[t-native-charge-floor]: ../../../../crates/daemon/src/lib.rs#L22730-L22801
[t-native-reject]: ../../../../crates/daemon/src/lib.rs#L23780-L23827
[t-vacuity]: ../../../../crates/daemon/src/lib.rs#L23710-L23778
[t-dup]: ../../../../crates/daemon/src/lib.rs#L24296-L24340
[t-sidecar]: ../../../../crates/daemon/src/codec/opencode.rs#L2083
[t-tagcold]: ../../../../crates/daemon/src/transform.rs#L22515
[t-poison]: ../../../../crates/daemon/src/transform.rs#L22651
[t-interleave]: ../../../../crates/daemon/src/transform.rs#L22686
[t-tag-sharing]: ../../../../crates/daemon/src/transform.rs#L22733
[t-tag-rollback]: ../../../../crates/daemon/src/transform.rs#L22770
[t-tag-charge]: ../../../../crates/daemon/src/transform.rs#L11902
[t-tag-refusal]: ../../../../crates/daemon/src/transform.rs#L11865
[t-tag-bootstrap]: ../../../../crates/daemon/src/transform.rs#L21716
[t-tag-protection]: ../../../../crates/daemon/src/transform.rs#L23617
[t-hyg-cold]: ../../../../crates/daemon/src/tail_hygiene.rs#L1148
[t-hyg-golden]: ../../../../crates/daemon/src/tail_hygiene.rs#L2290
[t-hyg-iterator]: ../../../../crates/daemon/src/tail_hygiene.rs#L2386
[t-hyg-key]: ../../../../crates/daemon/src/tail_hygiene.rs#L1217
[t-hyg-domains]: ../../../../crates/daemon/src/tail_hygiene.rs#L1265
[t-hyg-invalidates]: ../../../../crates/daemon/src/tail_hygiene.rs#L1366
[t-hyg-bounds]: ../../../../crates/daemon/src/tail_hygiene.rs#L1506
[t-hyg-prefix]: ../../../../crates/daemon/src/tail_hygiene.rs#L1638
[t-hyg-payload]: ../../../../crates/daemon/src/tail_hygiene.rs#L1696
[t-hyg-table]: ../../../../crates/daemon/src/tail_hygiene.rs#L1766
[t-hyg-poison]: ../../../../crates/daemon/src/tail_hygiene.rs#L1855
[t-hyg-overlap]: ../../../../crates/daemon/src/tail_hygiene.rs#L1905
[t-hyg-pool-bound]: ../../../../crates/daemon/src/tail_hygiene.rs#L1970
[t-hyg-status]: ../../../../crates/daemon/src/lib.rs#L21548-L21687
[t-hyg-production]: ../../../../crates/daemon/src/transform.rs#L22584
[hyg-bench-input]: ../../../../crates/daemon/benches/hot_path.rs#L83-L97
[hyg-bench-loop]: ../../../../crates/daemon/benches/hot_path.rs#L161-L199
[t-seldiff]: ../../../../crates/daemon/tests/selection_differential.rs#L1-L5

[hook]: ../../../../crates/daemon/src/lib.rs#L8277-L8285
[no-fire-doc]: ../../../../crates/daemon/src/lib.rs#L5521-L5535
[t-no-fire]: ../../../../crates/daemon/src/lib.rs#L38152-L38196
[t-emergency]: ../../../../crates/daemon/src/lib.rs#L37359-L37433
[t-cas]: ../../../../crates/daemon/src/lib.rs#L24640-L24711
[t-snap-resist]: ../../../../crates/memory-store/src/lib.rs#L18542
[t-snap-keeps]: ../../../../crates/memory-store/src/lib.rs#L18596
[t-cas-empty]: ../../../../crates/memory-store/src/lib.rs#L18665
[t-counter]: ../../../../crates/daemon/tests/boundary_counter_durability.rs#L12
[t-success]: ../../../../crates/daemon/src/lib.rs#L25777-L25791
[t-repeat]: ../../../../crates/daemon/src/lib.rs#L25793-L25819
[t-frozen]: ../../../../crates/daemon/src/lib.rs#L25821-L25859
[t-status]: ../../../../crates/daemon/src/lib.rs#L26075-L26120
[t-divergence]: ../../../../crates/daemon/src/lib.rs#L34126-L34219
[t-upserts]: ../../../../crates/memory-store/src/lib.rs#L19556
[t-sched]: ../../../../crates/daemon/src/transform.rs#L13549
[t-secret]: ../../../../crates/memory-store/src/lib.rs#L16547
[t-restart]: ../../../../crates/memory-store/src/lib.rs#L21046
[t-faults-sc]: ../../../../crates/memory-store/src/lib.rs#L20670
[t-status-sc]: ../../../../crates/daemon/src/lib.rs#L38016-L38074
[t-publish-cas]: ../../../../crates/memory-store/src/lib.rs#L21164
[t-truncate]: ../../../../crates/memory-store/src/lib.rs#L22626
[t-dup-json]: ../../../../crates/memory-store/src/lib.rs#L16283
[t-keydir]: ../../../../crates/memory-store/src/lib.rs#L16200
[t-container]: ../../../../crates/memory-store/src/lib.rs#L16300
[t-preserved]: ../../../../crates/memory-store/src/lib.rs#L16337
[t-cache-redact]: ../../../../crates/memory-store/tests/production_redaction.rs#L728
[t-identity-tx]: ../../../../crates/memory-store/src/lib.rs#L16480
[t-sync]: ../../../../crates/storage/src/lib.rs#L4286-L4351

[tpaged]: ../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L540
[t244]: ../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L249
[tinplace]: ../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L1449
[tavaildb]: ../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.test.ts#L29-L125
[tmidturn]: ../../../../packages/opencode-plugin/src/hooks/context/read-session-db.test.ts#L57-L892
[tismidturn]: ../../../../packages/opencode-plugin/src/hooks/context/read-session-db.test.ts#L1138-L1169
[tdbpath]: ../../../../packages/opencode-plugin/src/hooks/context/read-session-db.test.ts#L1171-L1239
[session-db-cache]: ../../../../packages/opencode-plugin/src/hooks/context/__tests__/session-db-cache-contract.ts#L1-L392
[session-db-hook]: ../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L1616-L1670
[tordinal]: ../../../../packages/opencode-plugin/src/hooks/context/read-session-raw.test.ts#L173
[tsqlite]: ../../../../packages/opencode-plugin/src/shared/sqlite.test.ts#L279
[tbind]: ../../../../packages/opencode-plugin/src/shared/sqlite-bind-style.test.ts#L32
[t1308]: ../../../../packages/opencode-plugin/src/hooks/context/module-wire.test.ts#L1310
[t1371]: ../../../../packages/opencode-plugin/src/hooks/context/module-wire.test.ts#L1373
[t1391]: ../../../../packages/opencode-plugin/src/hooks/context/module-wire.test.ts#L1393
[t181]: ../../../../packages/opencode-plugin/src/shared/host-client/frame-channel.test.ts#L181
[t358]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/shared/logger.test.ts#L358
[t381]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/shared/logger.test.ts#L381
[t342]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/shared/logger.test.ts#L342
[t408]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/shared/logger.test.ts#L408
[log-level-checks]: ../../../../packages/opencode-plugin/src/shared/logger.test.ts#L377
[log-flush-checks]: ../../../../packages/opencode-plugin/src/shared/logger.test.ts#L492
[log-hardening-checks]: ../../../../packages/opencode-plugin/src/shared/logger.test.ts#L607
[log-hook-checks]: ../../../../packages/opencode-plugin/src/shared/logger.test.ts#L519
[tevent]: ../../../../packages/opencode-plugin/src/hooks/context/event-handler.test.ts#L272

[t-limits]: ../../../../crates/host-runtime/src/ring_transport.rs#L1052-L1074
[t-batch]: ../../../../crates/shm-transport/src/backend/ring.rs#L4018-L4046
[t-abort]: ../../../../crates/shm-transport/src/backend/ring.rs#L3667-L3685
[t-reuse]: ../../../../crates/shm-transport/src/backend/ring.rs#L4075-L4091
[t-subpage]: ../../../../crates/shm-transport/src/backend/ring.rs#L4093-L4118
[t-trim-order]: ../../../../crates/shm-transport/src/backend/ring.rs#L3319-L3335
[t-trim-res]: ../../../../crates/shm-transport/src/backend/ring.rs#L4138-L4164
[t-punchfail]: ../../../../crates/shm-transport/src/backend/ring.rs#L4245-L4261
[t-fifo]: ../../../../crates/shm-transport/tests/ring.rs#L124-L174
[t-sparse]: ../../../../crates/shm-transport/tests/ring.rs#L230-L273
[t-syscall]: ../../../../crates/shm-transport/src/backend/ring.rs#L3086-L3087
[t-roundtrip]: ../../../../crates/shm-transport/src/lease.rs#L475-L506
[t-concurrent]: ../../../../crates/shm-transport/src/lease.rs#L581-L619
[t-readbyte]: ../../../../crates/shm-transport/src/lease.rs#L543
[ci-miri]: ../../../../.github/workflows/ci.yml#L597-L635
[ci-valgrind]: ../../../../.github/workflows/ci.yml#L637-L669
[t-deadline]: ../../../../crates/host-runtime/src/ring_transport.rs#L1849-L1879
[t-commitq]: ../../../../crates/shm-transport/src/backend/ring.rs#L3198
[t-parts]: ../../../../crates/host-runtime/src/handler.rs#L596-L626
[t-parts2]: ../../../../crates/host-runtime/src/handler.rs#L628-L645
[t-parts3]: ../../../../crates/host-runtime/src/handler.rs#L647-L673
[bench]: ../../../../crates/shm-transport/benches/hardware_envelope.rs#L296-L306

[t-cap-cas]: ../../../../crates/kernel/tests/kernel_cas.rs#L418-L433
[t-retained]: ../../../../crates/kernel/tests/kernel_cas.rs#L436-L450
[t-reclaim]: ../../../../crates/kernel/tests/kernel_gc.rs#L535-L554
[t-payload]: ../../../../crates/kernel/tests/kernel_cas.rs#L214-L234
[t-route-cap]: ../../../../crates/daemon/tests/kernel_routes.rs#L3600-L3629
[t-orphan]: ../../../../crates/kernel/tests/kernel_gc.rs#L593-L634
[t-cancel]: ../../../../crates/kernel/tests/kernel_gc.rs#L556-L591
[t-oracle]: ../../../../crates/kernel/tests/cas_fault_injection.rs#L350-L382
[t-recover-twice]: ../../../../crates/kernel/tests/cas_fault_injection.rs#L384-L389
[t-faults]: ../../../../crates/kernel/tests/cas_fault_injection.rs#L426-L494
[t-gcfaults]: ../../../../crates/kernel/tests/cas_fault_injection.rs#L497-L582
[t-crash]: ../../../../crates/kernel/tests/cas_fault_injection.rs#L902-L969
[t-gone]: ../../../../crates/kernel/tests/cas_fault_injection.rs#L838
[ci-bench]: ../../../../.github/workflows/ci.yml#L514-L518
[evidence]: ../../../../crates/host-runtime/benches/support/evidence.rs#L1-L8
[t-line]: ../../../../crates/daemon/src/transform.rs#L12373
[t-timings]: ../../../../crates/daemon/src/transform.rs#L12323
[t-tc-match]: ../../../../crates/daemon/src/token_cache.rs#L188
[t-tc-hits]: ../../../../crates/daemon/src/token_cache.rs#L209
[t-tc-rotate]: ../../../../crates/daemon/src/token_cache.rs#L208
[t-tc-stats]: ../../../../crates/daemon/src/token_cache.rs#L224
[t-tc-alias]: ../../../../crates/daemon/src/token_cache.rs#L241
[t-bypass]: ../../../../crates/daemon/src/transform.rs#L24332
[selection-sharing]: ../../../../crates/daemon/src/transform.rs#L24583
[soft-threshold-check]: ../../../../crates/daemon/src/transform.rs#L24387
[soft-gates-check]: ../../../../crates/daemon/src/transform.rs#L24521
[tag-accounting-check]: ../../../../crates/daemon/src/transform.rs#L21486
[serialization-gate-check]: ../../../../crates/daemon/src/transform.rs#L28295
[t-preselect]: ../../../../crates/secret-scanner/src/rules.rs#L685-L722
[t-canaries]: ../../../../crates/secret-scanner/tests/rule_canaries.rs#L4
[t-qual]: ../../../../crates/secret-scanner/tests/qualification.rs#L49-L63
[t-pinned]: ../../../../crates/secret-scanner/src/evaluator.rs#L1660-L1683
[t-windows]: ../../../../crates/context-core/src/redaction.rs#L827-L856
[t-only-path]: ../../../../crates/context-core/src/redaction.rs#L857
[t-chunk-fp]: ../../../../crates/daemon/src/historian.rs#L3925
[t-boundary-construction]: ../../../../crates/daemon/src/lib.rs#L17913-L18240
[t-firing-capture]: ../../../../crates/daemon/src/lib.rs#L24884-L25165
[diff-prod]: ../../../../crates/daemon/tests/historian_truncate_differential.rs#L102-L112
[diff-exact]: ../../../../crates/daemon/tests/historian_truncate_differential.rs#L117-L128
[diff-small]: ../../../../crates/daemon/tests/historian_truncate_differential.rs#L133-L140
[t-golden]: ../../../../crates/daemon/src/historian_chunk.rs#L1773-L1859
[t-marker]: ../../../../crates/daemon/src/historian_chunk.rs#L1744-L1757
[t-vixie]: ../../../../crates/daemon/src/smart_note_evaluation.rs#L1584
[t-extreme]: ../../../../crates/daemon/src/smart_note_evaluation.rs#L1570-L1582
[t-golden-cron]: ../../../../crates/daemon/src/smart_note_evaluation.rs#L1126
[t-sched-cron]: ../../../../crates/daemon/src/dreamer_scheduler.rs#L680
[t-mtime]: ../../../../crates/daemon/src/config.rs#L2117-L2155
[t-raise]: ../../../../crates/daemon/src/config.rs#L1314
[t-gate]: ../../../../crates/daemon/src/config.rs#L1649
[t-hostile]: ../../../../crates/daemon/src/config.rs#L1715
[t-panic-internal]: ../../../../crates/host-runtime/tests/dispatch.rs#L551
[t-panic-stderr]: ../../../../crates/host-runtime/tests/dispatch.rs#L603
[t-panic-child]: ../../../../crates/host-runtime/tests/dispatch.rs#L631-L660
[t-scalar]: ../../../../crates/memory-store/src/lib.rs#L15763-L15982
[t-counters]: ../../../../crates/memory-store/src/lib.rs#L15989-L16017
[t-load-count]: ../../../../crates/daemon/src/lib.rs#L25908-L25960
[t-timing]: ../../../../crates/daemon/src/lib.rs#L25964-L25980
[t-phase]: ../../../../crates/daemon/src/lib.rs#L25984-L25997
[t-phase-reread]: ../../../../crates/daemon/src/lib.rs#L26052-L26073

The five checks above were added with the single-load pass (implementation
base `96709d0ef54bcfad2327878ab96e118fb8ba4969` plus the preceding storage
units); their links are to the live tree.

The three preparation checks were added with the single-pass `meta`
preparation; their links are to the live tree.

[t-single-pass]: ../../../../crates/memory-store/src/lib.rs#L16108-L16156
[t-refusal-order]: ../../../../crates/memory-store/src/lib.rs#L16161-L16195
[t-meta-bytes]: ../../../../crates/memory-store/tests/production_redaction.rs#L611-L725
[t-retire]: ../../../../crates/memory-store/src/lib.rs#L16659-L16816
[t-seq-conflict]: ../../../../crates/memory-store/src/lib.rs#L16821-L16850
[t-receive]: ../../../../crates/memory-store/src/lib.rs#L16877-L16919
[t-first-receive]: ../../../../crates/memory-store/src/lib.rs#L16855-L16872
[t-retained-pass]: ../../../../crates/memory-store/src/lib.rs#L16986-L17085
[t-ring]: ../../../../crates/memory-store/src/lib.rs#L17471-L17534
[t-root]: ../../../../crates/memory-store/src/lib.rs#L17090-L17135
[t-parse-first]: ../../../../crates/memory-store/src/lib.rs#L17436-L17465
[t-fingerprint]: ../../../../crates/memory-store/src/lib.rs#L17140-L17187
[t-fingerprint-evict]: ../../../../crates/memory-store/src/lib.rs#L17192-L17241
[t-fingerprint-bound]: ../../../../crates/memory-store/src/lib.rs#L17246-L17306
[t-two-writers]: ../../../../crates/memory-store/src/lib.rs#L17311-L17360
[t-key-reuse]: ../../../../crates/memory-store/src/lib.rs#L17365-L17432
[t-reassign]: ../../../../crates/memory-store/src/lib.rs#L24780-L24803
[t-side-channel-crash]: ../../../../crates/memory-store/src/lib.rs#L20883-L21043
[t-outcome]: ../../../../crates/daemon/src/lib.rs#L26006-L26050
