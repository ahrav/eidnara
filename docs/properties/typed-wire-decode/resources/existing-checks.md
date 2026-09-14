# Existing resource checks

System: daemon admission, decode, projection, and retained accounting.
HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
[Source register](source-register.md) records scope and consulted leads.
Every entry is **unaudited**. This is a source inventory, not a test run or an
adequacy verdict. Test adequacy belongs to `/testing:invariant-test-review`;
production enforcement belongs to
`/low-level-systems:defensive-assertions-and-invariant-guards`.

## Survey and inventory boundary

The inventory covers all checks in meter/retained-size modules and the two
allocation binaries, the admission/decode test cluster, wire projection
tests, and the resource holder/host-budget seams below. Unrelated daemon
route semantics, storage transactions, and transport protocol suites remain
outside this part. Adjacent projection semantic tests are listed as controls,
not as evidence of allocation or reservation coverage.

Counts below are static macro/attribute counts at HEAD, including tests.
They measure source density, not assertion strength or executed coverage.

| File | Lines | Test attributes | Assertion macro sites |
| --- | --- | --- | --- |
| `crates/daemon/src/metered_decode.rs` | 1155 | 3 | 15 |
| `crates/daemon/src/retained_size.rs` | 336 | 2 | 6 |
| `crates/daemon/src/wire.rs` | 1863 | 14 | 70 |
| `crates/daemon/tests/parse_charge_covers_typed_decode.rs` | 333 | 6 | 19 |
| `crates/daemon/tests/served_json_passthrough_allocations.rs` | 86 | 1 | 4 |
| `crates/daemon/benches/hot_path.rs` | 429 | 0 | 5 |

## Production and compile-time guards

| Location | Exact guard, effect, or diagnostic | Status |
| --- | --- | --- |
| `crates/daemon/src/metered_decode.rs:45-50` | Compile-time assertion: Arc<Value> handle slack plus header and inline Value fit node charge. | unaudited |
| `crates/daemon/src/metered_decode.rs:164-167` | `admit_once` prevents repeated admission work on one meter. | unaudited |
| `crates/daemon/src/metered_decode.rs:201-217` | Restart retains charges; release clears them and sets charged bytes to zero. Lifecycle mechanism, not an allocation oracle. | unaudited |
| `crates/daemon/src/metered_decode.rs:227-244` | Charges only growth of the longest escaped-string scratch, with factor two; raw pre-scan can reserve before deserialization. | unaudited |
| `crates/daemon/src/metered_decode.rs:246-265` | Prior refusal sticks; arithmetic overflow or needed bytes above capacity returns `Permanent`. | unaudited |
| `crates/daemon/src/metered_decode.rs:266-303` | Reserve batches halve to exact shortfall; inability to acquire exact shortfall returns `Transient` and records needed/charged/capacity. | unaudited |
| `crates/daemon/src/metered_decode.rs:329-348` | Meter refusal outranks serde recovery, releases charge, and returns `Refused`; trailing data returns `Invalid`. | unaudited |
| `crates/daemon/src/metered_decode.rs:419-435` | Byte-derived node floor refuses without allocation; malformed suffix bytes can increase the floor, so it is not a valid-JSON outcome oracle. | unaudited |
| `crates/daemon/src/metered_decode.rs:666-675` | Ignored subtrees traverse metered `deserialize_any` rather than bypassing node/string charges. | unaudited |
| `crates/daemon/src/metered_decode.rs:783-905` | Scalar, text, unit, sequence, and map visitors charge before forwarding; text counts node and string bytes; `visit_str` also counts escape scratch. | unaudited |
| `crates/daemon/src/lib.rs:12153-12170` | Byte cap precedes meter; admission precedes sub-cap lane probe; meter lives through settlement. | unaudited |
| `crates/daemon/src/lib.rs:12921-12946,15758-15765` | Gate walk restarts needed count; invalid typed decode falls back; refused decode returns without dispatch. | unaudited |
| `crates/daemon/src/lib.rs:16084-16103` | Footprint floor can refuse before decode; scratch precharge follows; permanent maps to `invalid_params`, transient to `queue_full`. | unaudited |
| `crates/daemon/src/lib.rs:16125-16136` | Error messages: `request body needs more resident bytes than this host can hold`; `resident capacity for request parsing is exhausted`. | unaudited |
| `crates/daemon/src/lib.rs:16144-16161` | Inclusive facade/transform caps; errors say `request body exceeds the 32 MiB transform limit` or `request body exceeds the 1 MiB limit`. | unaudited |
| `crates/daemon/src/wire.rs:218-237` | Reattachment rejects inconsistent frontiers, block bounds, or message/block metadata with `None`. | unaudited |
| `crates/daemon/src/wire.rs:516-537` | Debug assertions align message/frontier/state lengths; invalid or duplicate mids return WireError. | unaudited |
| `crates/daemon/src/retained_size.rs:72-134,224-271` | Recursive retained estimates and conservative per-holder charges; these are accounting mechanisms, not live-heap assertions. | unaudited |
| `crates/daemon/src/lib.rs:2853-2864` | Projection lease uses checked byte addition and count/byte caps before snapshot clone. | unaudited |
| `crates/daemon/src/lib.rs:2871-2899` | Projection cache refuses zero/oversized entries and evicts while over total cap; constructed snapshot already exists. | unaudited |
| `crates/host-runtime/src/handler.rs:549-565` | Resident reserve charges scratch without waiting; returned charge must cover live request allocations. | unaudited |
| `crates/host-runtime/src/config.rs:137-151` | Host limits reject resident budgets below the interoperability minimum or above the semaphore/u32 capacity. | unaudited |
| `crates/host-runtime/src/wire.rs:415-442` | Async charge rejects requests above capacity; try_charge uses exact semaphore acquisition and rejects non-u32 requests. | unaudited |
| `crates/host-runtime/src/wire.rs:449-498` | Drop releases charge; failed split preserves it; shrink releases only excess. | unaudited |

## Meter and admission tests

All locations in this table are `crates/daemon/src/metered_decode.rs` or
`crates/daemon/src/lib.rs`, explicitly named in each row.

| Location and test | Condition or message | Status |
| --- | --- | --- |
| `crates/daemon/src/metered_decode.rs:1066` `a_capacity_bound_count_stops_at_the_value_that_crosses_it` | Permanent refusal stops within one node charge; exact-capacity comparison; undecodable empty input reaches nothing. | unaudited |
| `crates/daemon/src/metered_decode.rs:1097` `footprint_of_counts_what_the_value_decode_charges` | Needed count equals walker footprint for raw-token and escaped cases. | unaudited |
| `crates/daemon/src/metered_decode.rs:1119` `footprint_floor_counts_the_values_the_meter_visits` | Scalar floor equality; strings add coefficient and escape terms; dense floor threshold is inclusive. | unaudited |
| `crates/daemon/src/lib.rs:19569` `request_byte_cap_widens_for_transform_class_only` | Route-sensitive 1/32 MiB caps; exact 32 MiB accepts and next byte refuses. | unaudited |
| `crates/daemon/src/lib.rs:19649` `entry_probe_reads_the_route_and_the_page_envelope_as_dispatch_does` | Method/kind precedence, null page fields, duplicate discriminators, malformed and escaped route names. | unaudited |
| `crates/daemon/src/lib.rs:19712` `tree_parse_witness_refuses_what_the_tree_refuses` | Compatibility walk rejects tree-invalid and reserved raw-token cases before direct decode. | unaudited |
| `crates/daemon/src/lib.rs:19747` `raw_value_token_matches_serde_json` | First-key versus later-key raw-token parse behavior. | unaudited |
| `crates/daemon/src/lib.rs:19986` `direct_and_tree_transform_decodes_agree_on_the_corpus` | Equal typed results when both accept, pinned acceptance classes, page assembly control; no peak measurement. | unaudited |
| `crates/daemon/src/lib.rs:20140` `unpaged_transform_bodies_reach_the_same_outcome_through_both_entry_paths` | Comparable outcomes and expected direct-lane case list. | unaudited |
| `crates/daemon/src/lib.rs:20188` `both_lanes_charge_the_same_footprint_and_refuse_the_same_bodies` | Same needed bytes/outcomes at dynamically derived footprint neighbours. | unaudited |
| `crates/daemon/src/lib.rs:20281` `decode_footprint_counts_values_and_retained_string_copies` | Separators in strings are not nodes; hard-coded three-copy lower bound at `crates/daemon/src/lib.rs:20294-20298`; dense and truncated prefix counts. Linked from R1 for explicit KTD4 disposition. | unaudited |
| `crates/daemon/src/lib.rs:20321` `metered_decode_charges_incrementally_and_refuses_above_capacity` | Needed/charged bounds, immediate refusal release, restart reuses charges. | unaudited |
| `crates/daemon/src/lib.rs:20358` `a_small_body_holds_no_more_than_twice_its_footprint` | Small bodies hold between footprint and twice footprint. | unaudited |
| `crates/daemon/src/lib.rs:20380` `a_nearly_drained_pool_is_charged_in_a_bounded_number_of_acquisitions` | Transient refusal with fewer than 100 reserve attempts. | unaudited |
| `crates/daemon/src/lib.rs:20414` `a_drained_pool_refuses_a_fitting_body_as_transient_and_records_the_shortfall` | Known holder, shortfall fields, zero charge after refusal, success after drop; eventual oversize is transient while held and permanent when free. | unaudited |
| `crates/daemon/src/lib.rs:20476` `a_doomed_body_is_refused_without_touching_the_pool` | Zero reserve calls, `BodyLane::Unread`, zero needed, too-large outcome. | unaudited |
| `crates/daemon/src/lib.rs:20516` `footprint_floor_never_exceeds_the_decoded_footprint` | Floor does not exceed walker count for valid corpus JSON. | unaudited |
| `crates/daemon/src/lib.rs:20533` `a_refused_decode_has_no_dispatch_side_effect` | Expected refusal, no route binding/store row, then success with room. | unaudited |

## Allocation binaries

| Location and test | Condition or message | Status |
| --- | --- | --- |
| `crates/daemon/tests/parse_charge_covers_typed_decode.rs:103` `parse_charge_covers_dense_native_typed_decode_peak` | Combined Value parse plus conversion peak <= footprint; direct <= tree; metered ignored nodes covered. | unaudited |
| `crates/daemon/tests/parse_charge_covers_typed_decode.rs:179` `parse_charge_covers_escaped_text_direct_decode_peak` | Direct plain/escaped text peak <= footprint; no tree text case. | unaudited |
| `crates/daemon/tests/parse_charge_covers_typed_decode.rs:202` `byte_cap_admits_a_facade_sized_body_without_body_proportional_allocation` | Sub-cap 900 KiB escaped key accepted with peak < half key bytes. | unaudited |
| `crates/daemon/tests/parse_charge_covers_typed_decode.rs:238` `a_held_pool_stops_the_direct_lane_walk_before_it_unescapes_a_large_string` | 4 MiB escaped text, drained reserve, peak <64 KiB and `queue_full`. | unaudited |
| `crates/daemon/tests/parse_charge_covers_typed_decode.rs:285` `a_pool_with_room_for_the_prefix_only_refuses_before_the_large_string_is_unescaped` | 64 KiB partial reserve, peak <64 KiB and `queue_full`. | unaudited |
| `crates/daemon/tests/parse_charge_covers_typed_decode.rs:311` `a_held_pool_refuses_before_the_lane_probe_unescapes_a_long_key` | Sub-cap long-key entry, peak <64 KiB and `queue_full`. | unaudited |
| `crates/daemon/tests/served_json_passthrough_allocations.rs:68` `passthrough_shell_canonicalization_allocates_independently_of_key_count` | Canonical bytes equal independent Value serialization; per-block event growth <=8 for serialization. | unaudited |

The two binaries require `test-support`. `Unmetered` and `Granting` in the
peak binary return `ByteCharge::none()`; they do not prove real pool backing.
PeakAlloc records requested layout sizes, excluding allocator rounding.
The served allocation counter is process-global and its per-block division
does not define the new messages allocation threshold.

## Retained accounting and projection controls

| Location and test | Condition or role | Status |
| --- | --- | --- |
| `crates/daemon/src/retained_size.rs:290` `original_json_is_charged_only_while_retained` | Original-tree heap deltas disappear on mutation; this assertion depends on the representation the plan removes. | unaudited |
| `crates/daemon/src/retained_size.rs:322` `message_accounting_charges_content_capacity_not_length` | Seven spare block slots add seven inline block sizes. | unaudited |
| `crates/daemon/src/wire.rs:958` `projection_retained_bytes_counts_wire_and_frontier_allocations_once` | Manual ownership estimate equals retained_bytes and exceeds legacy estimate. | unaudited |
| `crates/daemon/src/wire.rs:1242` `repeated_call_id_within_owner_message_shares_one_arc_identity` | Tool-arc semantic identity control, not heap allocation identity. | unaudited |
| `crates/daemon/src/wire.rs:1276` `reasoning_joins_the_arc_its_adjacent_call_was_assigned` | Adjacent projection semantic control. | unaudited |
| `crates/daemon/src/wire.rs:1357` `user_carried_tool_result_pairs_with_prior_assistant_call` | Valid mixed-payload projection control. | unaudited |
| `crates/daemon/src/wire.rs:1422` `user_carried_tool_result_without_prior_call_still_rejects` | Projection failure control for cleanup-window construction. | unaudited |
| `crates/daemon/src/wire.rs:1447` `opaque_and_media_inside_tool_result_content_are_accepted_and_projected` | Surviving Value-bearing payload shape control. | unaudited |
| `crates/daemon/src/wire.rs:1521` `incremental_projection_reuses_prefix_storage_and_preserves_tool_arc_state` | Prefix storage reuse and semantic equivalence. | unaudited |
| `crates/daemon/src/wire.rs:1578` `empty_and_reserved_message_ids_are_rejected` | Invalid identity projection control. | unaudited |
| `crates/daemon/src/wire.rs:1595` `duplicate_message_ids_are_rejected_across_the_incremental_prefix` | Incremental prefix failure control. | unaudited |
| `crates/daemon/src/wire.rs:1624` `reduced_tool_result_keeps_failure_variant_and_output_extras` | Payload preservation control; no byte accounting assertion. | unaudited |
| `crates/daemon/src/wire.rs:1708` `reattach_keeps_block_level_original_but_rebuilds_the_message_shell` | Message original removed, block original retained, replay preserved; representation-specific clauses need replacement. | unaudited |
| `crates/daemon/src/wire.rs:1749` `repeated_prefix_reattachment_shares_canonical_shells` | Pointer sharing, equivalent projection, copy-on-write, surviving block owner. | unaudited |
| `crates/daemon/src/wire.rs:1796` `shared_ingress_is_send_and_preserves_decode_refusals` | Send + static types and shared/owned decode refusal equivalence. | unaudited |
| `crates/daemon/src/wire.rs:1818` `incremental_projection_checks_effective_synthetic_status` | Sharing occurs only with compatible synthetic status. | unaudited |
| `crates/daemon/src/lib.rs:20584` `transform_snapshot_cache_is_generation_safe_and_lru_bounded` | Snapshot lifecycle/LRU control; not a transient allocator peak. | unaudited |
| `crates/daemon/src/lib.rs:20624` `snapshot_lease_budget_survives_cache_churn_and_releases_exact_charge` | Active owner remains charged across eviction and releases on drop. | unaudited |
| `crates/daemon/src/lib.rs:20679` `snapshot_lease_budget_rejects_second_lease_on_bytes_alone` | Byte cap refuses a second otherwise count-admissible lease. | unaudited |
| `crates/daemon/src/lib.rs:23008` `native_cache_charge_keeps_raw_allocation_floor_beside_sidecar_estimate` | Raw Value allocation floor and shared versus distinct native backing. | unaudited |
| `crates/daemon/src/lib.rs:23311` `projection_cache_clones_are_charged_to_an_active_lease_budget` | Clone admission, cache removal with surviving charge, exact lease release. | unaudited |
| `crates/daemon/src/lib.rs:24143` `projected_prefix_is_charged_to_the_projection_cache_not_native_lru` | Projection and native holder budgets are separate. | unaudited |

## Host pool tests and highest integration seam

| Location and test | Condition or role | Status |
| --- | --- | --- |
| `crates/host-runtime/src/wire.rs:826` `capacity_separates_permanent_from_transient_exhaustion` | Fixed capacity distinguishes impossible size and currently held permits. | unaudited |
| `crates/host-runtime/src/wire.rs:850` `try_charge_is_exact_and_all_or_none` | Failed acquisition changes no permits; drop restores capacity; zero/unconvertible requests. | unaudited |
| `crates/host-runtime/src/wire.rs:868` `split_preserves_total_and_shrink_releases_only_the_delta` | Splits transfer, shrink releases, failed split preserves. | unaudited |
| `crates/host-runtime/src/wire.rs:900` `split_or_take_falls_back_to_the_whole_charge` | Oversized split takes the full charge and restores it on drop. | unaudited |
| `crates/host-runtime/src/wire.rs:916` `body_charge_and_reservation_share_one_ingress_pool` | Two consumers of the same budget observe pressure; this does not merge host ingress and scratch pools. | unaudited |
| `crates/host-runtime/src/wire.rs:936` `charge_above_capacity_fails_instead_of_waiting_forever` | Oversize resolves within a one-second test timeout; exact capacity fits. | unaudited |
| `crates/host-runtime/src/config.rs:481` `the_resident_cap_splits_into_three_non_overlapping_pools` | Minimum equals ingress frame plus reserved egress/scratch. | unaudited |
| `crates/host-runtime/src/config.rs:506` `byte_budget_below_interop_minimum_rejected` | Minimum boundary rejects below and admits at it. | unaudited |
| `crates/host-runtime/src/config.rs:520` `oversize_byte_budget_rejected` | Resident budget above u32 range rejects. | unaudited |
| `crates/daemon/tests/direct_host.rs:49` `readiness_permissions_catalog_and_real_unary_transform` | Unix real-host fixture is available for externally visible admission; no new pressure case is run. | unaudited |

## Bench checks and explicit absences

| Location | Condition or gap | Status |
| --- | --- | --- |
| `crates/daemon/benches/hot_path.rs:69-80` | Decoded bench inputs must carry message originals. | unaudited |
| `crates/daemon/benches/hot_path.rs:118-145` | Reattached shells drop only message originals and share block storage. | unaudited |
| `crates/daemon/Cargo.toml:77-81` | Hot-path bench requires `bench-internals` and disables the libtest harness. | unaudited |
| `docs/properties/hot-path-optimization/latency-audit/catalog.md:125-130,1772-1773` | W1 remains invalidated; retained old handoff text is not active authorization. | unaudited |

R6 is also invalidated, for category mismatch. The exact payoff obligation
is [EG1](evidence-gates.md#eg1-decode-projection-payoff); it is prospective
acceptance evidence rather than another existing runtime check.

None found at HEAD for the proposed isolated 16-events/message and strict
3-times-message-JSON decode gate. None found for a complete text-heavy tree
and failure/fallback peak matrix. None found for a continuous allocation-to-
pool ledger over entry, decode, projection, and retained-owner handoff.
None found for a 40/200 combined decode-plus-projection before/after manifest
or the complete independent resource marker set in this part. A1 explicitly
states that the managed client drops later terminals
(`docs/properties/hot-path-optimization/latency-audit/catalog.md:142-148`);
its settled result does not establish one-terminal emission.

Confirmed discrepancy: above-facade-cap probing precedes meter creation at
`crates/daemon/src/lib.rs:12153-12166,16144-16161`, contrary to A1's unqualified
charge-before-probe claim at
`docs/properties/hot-path-optimization/latency-audit/catalog.md:159-165`.
Allocation size on that path is unmeasured, not a diagnosed budget exceedance.

Suspiciously quiet areas: canonical serializer scratch; error/cleanup peaks;
preconstructed tree baselines;
capacity tests derived from candidate coefficients; and source presence
mistaken for exercised evidence. These are discovery gaps, not adequacy
verdicts on the existing tests.

## Independent-review inventory correction

Finding 4 from `ses_f6756093fffeVjNp36S3E8pKrM` reports omission of the
hard-coded-three test. Local validation finds its row already present in the
pre-disposition inventory, SHA-256
`f06f27cffb73b6ef5b532b03fb24d761e0c4cbb528e6ce1eb3d42eaee7c7db77`.
The omission is in R1's Existing check field and evidence trail, not this table.
Those links are added and the exact three-copy assertion is identified above.
No test is removed or declared adequate. The candidate test disposition stays
open under the accepted KTD4 coefficient decision.

Rename: `crates/daemon/tests/served_json_passthrough_allocations.rs` is
`crates/daemon/tests/served_json_shell_allocations.rs` on the typed-wire U1
branch, and `passthrough_shell_canonicalization_allocates_independently_of_key_count`
is `decoded_shell_canonicalization_allocates_independently_of_key_count`; the
inventory rows above keep their pinned-HEAD names.
