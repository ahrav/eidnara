# Shared-input equivalence surface

This lens records the equivalences that hold today between a cloned input and
the artifact derived from it, at every place the per-turn `transform` pass
copies the request or the projection and then consumes the copy read-only or
with one small mutation. Anchors are checked in
`/local/home/ahrav/scratch/eidnara` at
`913234433ae36a80a6e22c6aac14c7f9aab74386` on 2026-09-10. It is analysis
only; no test ran and nothing outside this file changed. The pass-internal
synthetic shadow is already the subject of
[synthetic-strip-precedes-every-coverage-read][tc-synthetic]; the two tag
numbering authorities are
[speculative-tag-numbering-has-two-authorities][tc-tagnum]. This lens adds
the equivalences those records assume and the observers outside `apply_once`
that they do not cover.

The handler owns the request as `parsed: Arc<TransformRequest>`
([`Arc::new(parsed)`][arc-parsed]). Before that, a delta body is expanded by
[`expand_transform_tail_delta`][expand]: the acknowledged message prefix is
rebuilt from the projection cache by [`reattach_messages_prefix`][reattach],
which clones every `Arc<WireBlock>` payload and rebuilds each message shell
with [`WireMessage::from_parts`][from-parts] (so the shell has no retained
`original`), and the native prefix is deep-copied from `Vec<Arc<Value>>` into
`Vec<Value>` at [`:4262-4265`][native-deep]. Inside
[`apply_once`][apply-head], [`normalize_synthetic_todo_ingress`][normalize]
clones the whole request when any non-synthetic message carries a
[`synthetic_todo_`][todo-prefix] call or result id and sets
`ck.meta.synthetic = true` on the clone only; the shadow
[`let req = rebased_req.as_ref().unwrap_or(ingress_req)`][shadow] makes every
later read inside the pass see the normalized flags. The handler-level
consumers do not: [`prepare_historian_fire`][historian-fire] passes the
un-normalized `parsed` to [`boundary_messages`][boundary-call] and to
[`assemble_historian_firing`][assemble], [`store_projection_cache`][store-pc]
charges `parsed.messages`, and
[`attach_native_messages_incremental`][native-attach] filters
`request.messages` by the un-normalized flag at
[`:13196-13201`][newest-assistant]. With compaction disabled
([default true][compaction-default]) the pass returns through
[`apply_additive_only`][additive] before normalization runs.

The projection is the shared artifact. [`project_messages`][project] and
[`project_messages_incremental`][project-inc] build [`FlatBlock`][flatblock]
values in [`flatten_block`][flatten]: `bytes` is `serde_json::to_string(block)`
behind an `Arc<str>`, `content_hash` is the SHA-256 of those bytes,
`tool_input` is `Arc::new(input.clone())`, `wire` is `Arc::new(block.clone())`,
and `synthetic` copies `msg.ck.meta.synthetic`. Synthetic messages are left
out of `identity_by_mid` ([`:489-494`][identity-skip],
[`:505-507`][identity-insert]) and their `HarnessMeta` is cloned into
`message_meta` at [`:514`][meta-clone]. The incremental path copies the
cached prefix blocks ([`:402`][prefix-copy]) and skips synthetic prefix
messages at [`:393`][inc-skip]. Consumers of the projection copy again:
[`sel_item_from_flat`][sel-item] clones `input` out of `block.wire`,
[`sel_kind_for_flat`][sel-kind] clones `tool_input` for the historian's
[`BoundaryBlock`][boundary-block],
[`tail_for_selection.clone()`][tail-clone] copies the whole `Vec<SelItem>`,
[`taggable_source`][taggable] text becomes `source_bytes: Vec<u8>` on every
minted [`TagRow`][tagrow] ([`:7167-7172`][mint-input]), and
[`measure_tail_hygiene`][hygiene] builds a fresh content `String` per part and
hashes `kind_name`, NUL, `content` in [`part_measurement`][part-measure].
On output, [`ServedMessage::from_message_reusing`][served-reusing] computes
`canonical_bytes = to_vec(to_value(&message))`, which sorts object keys because
the workspace enables only [`raw_value`][serde-features] on `serde_json`, and
the prepared transform output writes exactly those bytes per served segment
([`PreparedSegment::served`][segment-served], [`:14506-14512`][segments]).
`Serialize for` [`WireMessage`][ser-msg] and [`WireBlock`][ser-block] replay
the retained `original` when present, and a `meta` edit does not clear it
([`:210-216`][meta-doc]). The tag baseline is a process cache of
`Arc<Vec<TagRow>>`: [`load_cached_tags`][load-tags] returns the cache's own
`Arc` on a hit, and [`Arc::make_mut(tag_rows)`][make-mut] runs on every pass
whether or not the mint batch is empty, so it copies the whole vector on every
pass whose rows came from a cache hit; only a store re-read refreshes the
entry.

Clone sites in scope, what reads the copy, and whether the copy is mutated:

| Site | Copy | Mutation | Downstream artifact |
| --- | --- | --- | --- |
| [`normalize`][normalize] | whole `TransformRequest` | `meta.synthetic = true` on matched messages | projection `synthetic`, `identity_by_mid`, `message_meta`; [`live`][live]; [ordinal check][ordinal-check]; [tail loop skip][tail-loop]; [served fingerprint ids][served-fps]; boundary, hygiene, tag mint, historian filters inside the pass |
| [`reattach`][reattach] | `WireBlock` per prefix block, new shell | none | `parsed.messages` prefix, then everything above; served bytes of a rebuilt shell use typed fields |
| [`native-deep`][native-deep] | `Value` per native prefix message | none | sidecar decode (which [copies each raw message again][raw-clone] into [`HarnessMessageMeta.raw`][decode-sidecar]), `native_ingress_chunks` equality, retained-bytes accounting |
| [`prefix-copy`][prefix-copy] | `FlatBlock` per cached prefix block | none | the incremental `FlatProjection` |
| [`sel-item`][sel-item], [`sel-kind`][sel-kind], [`tail-clone`][tail-clone] | [`SelKind::ToolCall.input: Value`][sel-kind-enum] per tool call | none (read at [selection.rs:323-331][sel-consume], [boundary.rs:1727-1746][boundary-consume], [injection.rs:400-403][injection-consume]) | reductions, historian tool summaries, todo capture |
| [`mint-input`][mint-input] | text bytes per taggable block | none after mint | `TagRow.source_bytes`, active-tag match at [`:7364`][active-match], caveman source at [`:5702-5705`][caveman-source] |
| [`make-mut`][make-mut] | whole `Vec<TagRow>` | append mint rows | `tag_rows` for overlay, hygiene, commit inputs at [`:4935-4945`][commit-inputs] |
| [`part-measure`][part-measure] | content `String` per part | none | hygiene `content_hash`, token cache key, `T`/`U` |
| [`served-reusing`][served-reusing] | `Value` tree plus bytes per served message | none | wire bytes, `canonical_hash`, `output_identity`, native message keys |

## Candidate properties

### projection-artifacts-are-ownership-independent

Type: safety
Check: `always` - for every message slice reaching the projector, assert
`project_messages(&msgs)` is structurally equal (the derived `PartialEq` on
[`FlatProjection`][flatproj], which covers `blocks` with `bytes`,
`content_hash`, `tool_input`, `wire`, `synthetic`, `arc_id`, and
`identity_by_mid`, `message_meta`, `message_block_ends`,
`states_after_messages`) whether the slice is the fresh request, the
normalized clone, a reattached prefix plus suffix, or a borrowed or
`Arc`-shared view; assert per block `content_hash == sha256(bytes)` and
`bytes == to_string(wire)`; assert `tool_input` equals the `input` inside
`wire.kind()` for every `tool_call` block; and assert
`project_messages_incremental(msgs, cached, k) == project_messages(msgs)`
with equal [`differential_bytes`][diff-bytes]. `always` because every pass
projects and every downstream digest (output identity, served fingerprints,
token caches, tag mint) keys on these fields.
Guarantee: The projection and every field a consumer digests from it depend
only on the message values, never on which allocation holds them.
Fault/timing angle: None in time; the hazard is a projector that reuses an
ingress `Arc<WireBlock>` and then computes `bytes` from a different
serialization than `to_string(block)`, or a consumer that reads `tool_input`
from one source and `wire.kind()` from another after the two are no longer
built from the same block ([`sel_item_from_flat`][sel-item] reads `wire`;
[`sel_kind_for_flat`][sel-kind] reads `tool_input`).
Required faults and enabling state: A session with a projection cache hit
(second pass onward,
[`astro_scale_projection_cache_reuses_on_the_second_pass`][t-astro]), a
delta body so the prefix is reattached, and a message set with
tool calls, tool results in a user message, repeated call ids, and a
replayed synthetic todo pair.
Reachability: default-production - every pass with compaction enabled
projects; the incremental arm needs a cache hit, which the plugin's delta
protocol produces on steady turns.
Existing check: [`assert_prefix_projection_equivalent`][assert-prefix] is
live under [`prefix_projection_differential_enabled`][gate-prefix], which is
`cfg!(test) || EIDNARA_PREFIX_PROJECTION_DIFFERENTIAL == "1"`; it fires only
when a reusable projection exists and compares incremental against full.
[`incremental_projection_reuses_prefix_storage_and_preserves_tool_arc_state`][t-inc]
and
[`projection_differential_catches_corrupt_first_changed_position`][t-projdiff]
exercise it. None found for `tool_input` versus `wire.kind()`
equality, and none found that runs the differential from
`crates/daemon/tests/` or the [benches][bench], where `cfg!(test)` is false
for the library and no test or CI file sets the variable.
Open questions:
- Is the release-build `assert_eq!` panic under the environment variable the
  intended production contract, or a developer switch? The transform catalog
  queued this as its gap G2 and it is still open. (needs human input)

### synthetic-normalization-is-scoped-to-the-pass

Type: safety
Check: `always` - for every pass, assert three observers see the same
synthetic sets as at HEAD: inside `apply_once` a message is synthetic iff
`meta.synthetic || any block id has the synthetic_todo_ prefix`; in
[`cached_boundary_messages`][cached-boundary],
[`assemble_historian_firing`][assemble], [`store_projection_cache`][store-pc],
and [`attach_native_messages_incremental`][native-attach] a message is
synthetic iff `parsed.messages[i].ck.meta.synthetic` as it arrived; and the
served wire bytes of a message whose flag was set by normalization equal the
bytes of the same message without the flag, because
[`Serialize for WireMessage`][ser-msg] replays `original`.
`always` because the sets decide which blocks count for coverage, historian
ordinals, native reasoning clears, and the output; the check is on the
observer, not on a defect.
Guarantee: Replacing the normalization clone with a shared view changes no
observer's synthetic set and no served byte.
Fault/timing angle: A shared-reference design that marks `parsed` in place
before `apply` widens the normalized view to the historian and native
attach, and a design that marks through `mark_modified` or rebuilds the
message drops `original`, so `"synthetic":true` appears on the wire for the
first time. Both are behavior changes relative to HEAD, not preservation.
Required faults and enabling state: A prior bust pass that froze a todo pair
([`tail_reclaim`][tail-reclaim] is true for every shipping profile), then an
array in which the harness replays the pair as ordinary messages without the
`synthetic` marker, as
[`warm_cache_selection_bust_does_not_replay_collapsed_synthetic_todo_as_live`][t-collapsed]
constructs at `:27303-27318`; a historian firing on that pass;
`serve_native` on.
Reachability: default-production - the normalization only runs when
`ctx.compaction_enabled` ([default true][compaction-default]); the replay
itself needs OpenCode to have consumed an injected pair. The
`apply_additive_only` arm is explicit-config-only.
Existing check: [tc-synthetic][tc-synthetic] covers the inside-the-pass
shadow with
[`pending_rewrite_passes_isolate_ingress_meta_usage_and_reconcile`][t-pending];
[`warm_cache_...`][t-collapsed] checks no duplicate tool-use
ids and cache reuse on the replayed pair. None found that compares the
historian's `BoundaryMsg` list or `input_ordinals` between a full-array turn
and a delta turn carrying the same replayed pair; none found that asserts
the served bytes of a normalized message omit the flag.
Open questions:
- Is the historian meant to see the replayed pair as a non-synthetic message
  with zero blocks (full-array lane) or not at all (delta lane)? See the
  disagreement below. (needs human input)
- [`pending_passthrough_messages`][pending-pass] and
  [`lineage_protocol_passthrough`][lineage-pass] serve the normalized clone's
  messages including the synthetic ones; their fingerprints get
  `eidnara_todo:` ids from the typed flag while their bytes carry no flag. Is
  that pairing intended? (needs human input)

### served-canonical-bytes-are-the-value-round-trip-form

Type: safety
Check: `always` - for every `ServedMessage`, assert
`canonical_bytes == serde_json::to_vec(&serde_json::to_value(&message))`,
`canonical_hash == sha256(canonical_bytes)`, `output_identity` is its lowercase
hex before [`with_output_identity`][with-identity] replaces it, and the bytes
the prepared output writes for the segment are `canonical_bytes`; assert
each `block_fingerprints[i]` equals
`(fingerprint(to_string(block)), to_string(block).len())`, and that the
projected `content_hash` is reused
only when [`flat.wire == served`][fp-reuse] under `WireBlock`'s derived
equality, which includes `original`. `always` because the bytes are the
response body on every turn and the hash is the native cache key on every
served message.
Guarantee: A served message's wire bytes, hash, and per-block fingerprints
are the same whether they are computed from a fresh clone, a shared
`Arc<WireMessage>`, or a reused projection digest.
Fault/timing angle: None in time. A direct `to_vec(&message)` on a typed
shell (rebuilt prefix, reduced, overlaid, or synthetic message) emits struct
field order instead of sorted keys, so bytes and `canonical_hash` change
while the JSON stays equal; a message with `original` is unaffected.
[`Serialize for ServedMessage`][ser-served] re-serializes the inner
`WireMessage`, not `canonical_bytes`, so any path that encodes a served
message through serde instead of [`PreparedSegment::served`][segment-served]
emits the other form; the handler avoids this only because it takes
`messages` out of the response before `to_value(response)`
([`:14485-14500`][segments-take]). The same message therefore already has
two canonical forms depending on whether its shell came from the harness or
from [`reattach`][reattach], which the
[`reattach_keeps_block_level_original_but_rebuilds_the_message_shell`][t-reattach]
test documents for unknown fields.
Required faults and enabling state: A response containing a harness message
with `original`, a rebuilt prefix message, a reduced message, a tag-overlaid
message, and the synthetic m0 and m1, compared against
`to_vec(to_value(..))` per message and against the prepared body.
Reachability: default-production - every accepted pass with messages goes
through [`PreparedSegment::served`][segment-served].
Existing check:
[`parked_p2_fingerprint_reuse_and_tag_frontier_match_baseline`][t-parked]
asserts equal `block_fingerprints` and equal `canonical_bytes`
between reuse and rehash;
[`served_fingerprint_block_ids_pin_flat_mid_index_format`][t-fpids] pins
fingerprint ids;
[`transform_segments_preserve_existing_golden_bytes`][t-segments] uses
`Exact` segments only. None found that asserts a `Served` segment writes
`canonical_bytes`, and none found for the sorted-key form itself.
Open questions:
- Is sorted-key canonical JSON a contract with the plugin, or an artifact of
  `preserve_order` being off? No wire document names it. (needs human input)

### native-attachment-output-is-representation-independent

Type: safety
Check: `always` - for every `serve_native` pass, assert
`to_vec(incremental native_messages) == to_vec(encode_full_native_messages(..))`
where the incremental side is `Vec<Arc<Value>>` and the full side is the
`Vec<Value>` from [`encode_full_native_messages`][encode-full] (the
comparison the [differential][native-diff] already makes); assert the sidecar
produced by [`native_sidecar`][native-sidecar]
from `request.native_messages` has the same `order` and `messages` as
`decode_opencode(native_messages).sidecar`, with `order` in first-seen
position ([`remember_message`][remember], [`:277-291`][sidecar-merge]); and
assert [`native_ingress_chunks`][ingress-chunks] shares an output chunk for
index `i` exactly when `chunk.value == native_messages[i]` by value. `always`
because the native attachment is the plugin's replay source on every turn.
Guarantee: Holding the expanded native prefix as `Arc<Value>` instead of
deep-copied `Value` changes no native output byte, sidecar order, or chunk
sharing decision.
Fault/timing angle: None in time; the hazard is a sharing shortcut that
decides chunk reuse by pointer identity and diverges from value equality, or
an incremental sidecar merge that changes first-seen order when a mid
repeats across the prefix and suffix.
Required faults and enabling state: A delta turn whose native prefix comes
from the attachment cache, a suffix that repeats a prefix mid, and a message
whose value is equal but not pointer-equal to a cached chunk.
Reachability: default-production for the output equality (`serve_native` on
every plugin turn); the differential assertion is explicit-config-only in
release under `EIDNARA_NATIVE_ATTACHMENT_DIFFERENTIAL`.
Existing check: [`native_attachment_differential_enabled`][gate-native] is
`cfg!(test) || env`, so every in-crate test that attaches native messages
runs the byte comparison;
[`incremental_native_cache_replays_complex_prefix_and_encodes_only_tail`][t-native-inc],
[`differential_assert_rejects_frontier_inside_mutated_native_region`][t-native-reject],
and [`duplicate_tool_use_assert_covers_incremental_native_suffix`][t-dup]
exercise it;
[`incremental_sidecar_carries_pins_across_three_generations`][t-sidecar]
covers `mid_pins`. None found for sidecar `order` equality between the
incremental and full decode, and none found for the chunk-sharing predicate.
Open questions:
- [`decode_opencode_sidecar_incremental`][sidecar-inc] sets `mid_pins` from
  the suffix decode only; the full decode accumulates pins over the whole
  array. The differential compares output bytes, not sidecars. Is pin
  equality required? Unresolved, needs a two-form sidecar comparison.

### tag-baseline-cache-entry-is-never-mutated-by-a-pass

Type: safety
Check: `always` - after every pass, assert the
[`TagBaselineCacheEntry.tags`][tag-entry] for the session equals
`store.load_tags_for_session(session)` for the
`(store_namespace, generation, count, max_tag_number)` the entry records,
in [`ORDER BY tag_number ASC`][load-order]; assert the pass-local `tag_rows`
after [`append_tag_mint_rows`][append-mint] is the baseline followed by the
mint rows with `tag_number = max + offset + 1`, in projection block order;
and
assert every committed `TagRow.source_bytes` equals the block's
[`taggable_source`][taggable] text bytes exactly. `always` because the
cache entry is read on the next pass of the same session and a stale or
speculative row changes the active-tag match at [`:7364`][active-match].
Guarantee: Pass-local mint rows never become visible through the tag
baseline cache, and every visible row's `source_bytes` is byte-equal to the
projected text it tags.
Fault/timing angle: [`Arc::make_mut`][make-mut] copies because the cache
holds a second reference ([`snapshot`][tag-snapshot] clones the `Arc`). A
design that appends in place, or that stores the pass's `Arc` back into the
cache before commit, exposes rows the store has not numbered; the store
numbers from `MAX(tag_number) + 1` per row and skips existing ids, which is
[tc-tagnum][tc-tagnum]'s subject. The same window exists on the store side:
[`load_cached_tags`][load-tags] re-reads the summary and appends
`load_tags_after` only when the summary is unchanged between two reads.
Required faults and enabling state: Tagging active (a profile with
`tool_present`), a warm baseline entry, a pass that mints, then a second pass
on the same session; for the aliasing arm, a mint batch whose CAS fails so
the speculative rows are never committed.
Reachability: default-production - the baseline cache is a process-wide
`OnceLock` used on every pass that loads tags.
Existing check:
[`tag_baseline_cache_matches_cold_passes_across_drop_reset_and_remint`][t-tagcold]
compares served bytes and durable rows between a cold and a cached store
across five passes;
[`poisoned_tag_baseline_refills_after_direct_sql_update`][t-poison] and
[`tag_baseline_cache_keeps_interleaved_sessions_isolated`][t-interleave]
cover refill and isolation. None found that asserts the cache entry is
unchanged after a mint pass whose commit fails, and none found for
`source_bytes` equality against the projected text outside the mint tests.
Open questions:
- `source_bytes` pass through the store's prepared-field path
  ([`write.bytes("source_bytes", ..)`][mint-prepared]); a detection refuses
  the insert. [R1][r1] owns that policy; this record assumes the bytes that
  land are unchanged. Is that assumption stated anywhere? (needs human input)

### hygiene-digest-is-kind-prefixed-part-content

Type: safety
Check: `always` - for every hygiene part, assert
`content_hash == hex(sha256(kind_name ++ "\0" ++ content))` where `content`
is the derived part string ([caveman-substituted and reminder-stripped
text][hyg-text], [`to_string(input)`][hyg-input], or
[`tool_output_content`][hyg-output]), that excluded parts hash
`"excluded\0" ++ block.bytes`, that the token cache is keyed by that digest
([`count_with_digest`][count-digest]), and that the measurement is identical
with a cold and a warm token cache. `always` because the digest is both the
reported hash and the cache key on every measured pass.
Guarantee: The hygiene digest is a function of the part kind and derived
content and is never the projection `content_hash`.
Fault/timing angle: None in time. The hazard is a shortcut that substitutes
`FlatBlock.content_hash` for the hygiene digest: the two hash different
inputs (full serialized `WireBlock` versus kind-prefixed derived content), so
the substitution changes every reported `content_hash` and, through the
cache key, mixes counts of the serialized block with counts of its text. The
projection digest already keys a different cache, the boundary token cache
([`token_count`][token-count]), which counts `block.bytes`, not part content.
Required faults and enabling state: A tail with text, tool call, tool result
(text and content variants), media, an excluded reduced block, and a
caveman-substituted text block; the same input measured twice.
Reachability: default-production - hygiene runs on every pass that reaches
the measurement.
Existing check:
[`measurement_is_identical_with_cold_and_warm_token_cache`][t-hyg-cold] and
[`parity_golden_matches_ts_reference_across_full_corpus`][t-hyg-golden] (the
golden carries per-part hashes generated by
`nudge-hygiene-ts-v2`). None found that states the digest input explicitly
against the projection digest.
Open questions: None.

## Existing checks

| Check | Source condition | Status |
| --- | --- | --- |
| [`assert_prefix_projection_equivalent`][assert-prefix] | incremental and full projection equal by `differential_bytes` and by value; live under [`gate-prefix`][gate-prefix] | unaudited |
| [`incremental_projection_reuses_prefix_storage_and_preserves_tool_arc_state`][t-inc] | reattached prefix equals `from_parts` inputs; incremental equals full; prefix `Arc`s are pointer-shared | unaudited |
| [`reattach_keeps_block_level_original_but_rebuilds_the_message_shell`][t-reattach] | unknown message-level field dropped, block-level field kept | unaudited |
| [`projection_differential_catches_corrupt_first_changed_position`][t-projdiff] | corrupt frontier is caught by the differential | unaudited |
| [`astro_scale_projection_cache_reuses_on_the_second_pass`][t-astro] | second pass reuses the cached projection | unaudited |
| [`pending_rewrite_passes_isolate_ingress_meta_usage_and_reconcile`][t-pending] | pass reads of ingress meta are isolated | unaudited |
| [`warm_cache_selection_bust_does_not_replay_collapsed_synthetic_todo_as_live`][t-collapsed] | replayed pair without the flag yields no duplicate tool-use id and reuses cache | unaudited |
| [`parked_p2_fingerprint_reuse_and_tag_frontier_match_baseline`][t-parked] | reused `content_hash` fingerprints and `canonical_bytes` equal a full rehash | unaudited |
| [`served_fingerprint_block_ids_pin_flat_mid_index_format`][t-fpids] | fingerprint block ids are `mid#index` and synthetic ids | unaudited |
| [`transform_segments_preserve_existing_golden_bytes`][t-segments] | `Exact` segments concatenate into the golden body | unaudited |
| [`incremental_native_cache_replays_complex_prefix_and_encodes_only_tail`][t-native-inc] | native prefix replayed, tail encoded, differential live | unaudited |
| [`differential_assert_rejects_frontier_inside_mutated_native_region`][t-native-reject] | differential panics on a corrupt native frontier | unaudited |
| [`frontier_vacuity_covers_opaque_repeats_eviction_and_same_length_edits`][t-vacuity] | same-length edits and repeats are not vacuously reused | unaudited |
| [`duplicate_tool_use_assert_covers_incremental_native_suffix`][t-dup] | unique tool-use ids across cached prefix and encoded suffix | unaudited |
| [`incremental_sidecar_carries_pins_across_three_generations`][t-sidecar] | `mid_pins` survive incremental sidecar decode | unaudited |
| [`tag_baseline_cache_matches_cold_passes_across_drop_reset_and_remint`][t-tagcold] | cold and cached passes serve equal bytes and equal durable rows | unaudited |
| [`poisoned_tag_baseline_refills_after_direct_sql_update`][t-poison] | generation change refills the baseline | unaudited |
| [`tag_baseline_cache_keeps_interleaved_sessions_isolated`][t-interleave] | two sessions do not share rows | unaudited |
| [`measurement_is_identical_with_cold_and_warm_token_cache`][t-hyg-cold] | hygiene output independent of token cache state | unaudited |
| [`parity_golden_matches_ts_reference_across_full_corpus`][t-hyg-golden] | per-part hashes and totals match the TypeScript golden | unaudited |
| [selection_differential.rs][t-seldiff] | optimized selection equals the frozen reference over generated `SelItem`s | unaudited |

None found:

- A test that `tool_input` on a `FlatBlock` equals the `input` inside its
  `wire.kind()`.
- A test that a `Served` prepared segment writes `canonical_bytes`, or that
  names the sorted-key form.
- A two-lane (full array versus delta) comparison of the historian's
  `BoundaryMsg` list, `input_ordinals`, or the native attachment for the
  same replayed synthetic pair.
- A test that the tag baseline entry is unchanged after a pass whose mint
  commit fails.
- A test that runs either differential gate from `crates/daemon/tests/` or
  `crates/daemon/benches/`; `cfg!(test)` is false there and no file sets the
  variables.
- A test that compares incremental and full `DecodeSidecar` values rather
  than encoded bytes.

## Contract-versus-code disagreements

No written contract names these equivalences; `docs/host-wire-protocol.md`
leaves routed bodies opaque. Two code-versus-code observations, both cited on
both sides and neither resolved here:

- The normalized synthetic view stops at the pass boundary. Inside
  `apply_once` every read follows the [shadow][shadow]; the handler passes
  the un-normalized `parsed` to [`boundary_messages`][boundary-call] and
  [`assemble_historian_firing`][assemble] together with the normalized
  `result.projection`. In the full-array lane a replayed todo message is a
  `BoundaryMsg` whose every block is filtered out by `!block.synthetic`
  ([`:16646`][boundary-filter]) and its ordinal counts in
  [`build_historian_chunk`][chunk-build]; in the delta lane the same message
  is rebuilt from `message_meta` with `synthetic: true` and is excluded by
  the message filter. The transform catalog's `synthetic-strip` record
  states the inside-the-pass invariant; nothing states the outside one.
- Two canonical forms of one message. A harness message serializes through
  its retained `original`; the same message rebuilt by
  [`reattach_messages_prefix`][reattach] serializes through typed fields and
  drops unknown top-level keys ([doc][reattach-doc], [test][t-reattach]).
  The output cache key ([`message_output_identity`][output-identity]) digests
  typed meta and block fingerprints, not the unknown keys, so a cache hit can
  return bytes computed from the other form. This is HEAD behavior, recorded
  so a sharing change does not silently pick one form.

## Anchors

[tc-synthetic]: ../../../daemon/transform/catalog.md#synthetic-strip-precedes-every-coverage-read
[tc-tagnum]: ../../../daemon/transform/catalog.md#speculative-tag-numbering-has-two-authorities
[r1]: ../../catalog.md#prepared-field-output-and-audit-policy-agree
[arc-parsed]: ../../../../../crates/daemon/src/lib.rs#L8190
[expand]: ../../../../../crates/daemon/src/lib.rs#L4204-L4291
[native-deep]: ../../../../../crates/daemon/src/lib.rs#L4262-L4265
[store-pc]: ../../../../../crates/daemon/src/lib.rs#L4332-L4375
[historian-fire]: ../../../../../crates/daemon/src/lib.rs#L5043
[boundary-call]: ../../../../../crates/daemon/src/lib.rs#L5123
[assemble]: ../../../../../crates/daemon/src/lib.rs#L5283-L5287
[native-sidecar]: ../../../../../crates/daemon/src/lib.rs#L12945-L12965
[encode-full]: ../../../../../crates/daemon/src/lib.rs#L13039-L13082
[ingress-chunks]: ../../../../../crates/daemon/src/lib.rs#L13084-L13128
[gate-native]: ../../../../../crates/daemon/src/lib.rs#L13130-L13137
[native-attach]: ../../../../../crates/daemon/src/lib.rs#L8481-L8510
[newest-assistant]: ../../../../../crates/daemon/src/lib.rs#L13196-L13201
[native-diff]: ../../../../../crates/daemon/src/lib.rs#L13373-L13390
[segments-take]: ../../../../../crates/daemon/src/lib.rs#L14485-L14500
[segments]: ../../../../../crates/daemon/src/lib.rs#L14506-L14512
[cached-boundary]: ../../../../../crates/daemon/src/lib.rs#L16624-L16684
[boundary-filter]: ../../../../../crates/daemon/src/lib.rs#L16646
[sel-kind]: ../../../../../crates/daemon/src/lib.rs#L16686-L16701
[token-count]: ../../../../../crates/daemon/src/lib.rs#L2035-L2057
[t-native-inc]: ../../../../../crates/daemon/src/lib.rs#L21075
[t-astro]: ../../../../../crates/daemon/src/lib.rs#L21755
[t-vacuity]: ../../../../../crates/daemon/src/lib.rs#L22495
[t-native-reject]: ../../../../../crates/daemon/src/lib.rs#L22565
[t-projdiff]: ../../../../../crates/daemon/src/lib.rs#L23038
[t-dup]: ../../../../../crates/daemon/src/lib.rs#L23081
[served-reusing]: ../../../../../crates/daemon/src/transform.rs#L164-L224
[with-identity]: ../../../../../crates/daemon/src/transform.rs#L226-L233
[ser-served]: ../../../../../crates/daemon/src/transform.rs#L301-L308
[gate-prefix]: ../../../../../crates/daemon/src/transform.rs#L2024-L2031
[assert-prefix]: ../../../../../crates/daemon/src/transform.rs#L2033-L2048
[served-fps]: ../../../../../crates/daemon/src/transform.rs#L2050-L2092
[normalize]: ../../../../../crates/daemon/src/transform.rs#L2094-L2111
[lineage-pass]: ../../../../../crates/daemon/src/transform.rs#L2724-L2728
[apply-head]: ../../../../../crates/daemon/src/transform.rs#L2847-L2877
[additive]: ../../../../../crates/daemon/src/transform.rs#L2858-L2860
[shadow]: ../../../../../crates/daemon/src/transform.rs#L2962
[live]: ../../../../../crates/daemon/src/transform.rs#L2976-L2980
[ordinal-check]: ../../../../../crates/daemon/src/transform.rs#L2986-L2997
[tail-clone]: ../../../../../crates/daemon/src/transform.rs#L4005
[commit-inputs]: ../../../../../crates/daemon/src/transform.rs#L4935-L4945
[caveman-source]: ../../../../../crates/daemon/src/transform.rs#L5702-L5705
[sel-item]: ../../../../../crates/daemon/src/transform.rs#L6326-L6355
[pending-pass]: ../../../../../crates/daemon/src/transform.rs#L6637-L6665
[tag-entry]: ../../../../../crates/daemon/src/transform.rs#L6793-L6818
[tag-snapshot]: ../../../../../crates/daemon/src/transform.rs#L6838-L6843
[load-tags]: ../../../../../crates/daemon/src/transform.rs#L6910-L6968
[mint-input]: ../../../../../crates/daemon/src/transform.rs#L7167-L7172
[append-mint]: ../../../../../crates/daemon/src/transform.rs#L7275-L7296
[taggable]: ../../../../../crates/daemon/src/transform.rs#L7300-L7324
[active-match]: ../../../../../crates/daemon/src/transform.rs#L7364
[make-mut]: ../../../../../crates/daemon/src/transform.rs#L7897-L7898
[output-identity]: ../../../../../crates/daemon/src/transform.rs#L10244-L10326
[tail-loop]: ../../../../../crates/daemon/src/transform.rs#L11006-L11010
[t-fpids]: ../../../../../crates/daemon/src/transform.rs#L13604
[t-parked]: ../../../../../crates/daemon/src/transform.rs#L13989
[t-pending]: ../../../../../crates/daemon/src/transform.rs#L19406
[t-tagcold]: ../../../../../crates/daemon/src/transform.rs#L22515
[t-poison]: ../../../../../crates/daemon/src/transform.rs#L22651
[t-interleave]: ../../../../../crates/daemon/src/transform.rs#L22686
[t-collapsed]: ../../../../../crates/daemon/src/transform.rs#L27957
[flatblock]: ../../../../../crates/daemon/src/wire.rs#L36-L64
[flatproj]: ../../../../../crates/daemon/src/wire.rs#L115-L128
[reattach-doc]: ../../../../../crates/daemon/src/wire.rs#L142-L145
[reattach]: ../../../../../crates/daemon/src/wire.rs#L146-L187
[diff-bytes]: ../../../../../crates/daemon/src/wire.rs#L330-L338
[project]: ../../../../../crates/daemon/src/wire.rs#L370-L372
[project-inc]: ../../../../../crates/daemon/src/wire.rs#L378-L412
[inc-skip]: ../../../../../crates/daemon/src/wire.rs#L393
[prefix-copy]: ../../../../../crates/daemon/src/wire.rs#L402
[identity-skip]: ../../../../../crates/daemon/src/wire.rs#L489-L494
[identity-insert]: ../../../../../crates/daemon/src/wire.rs#L505-L507
[meta-clone]: ../../../../../crates/daemon/src/wire.rs#L514
[flatten]: ../../../../../crates/daemon/src/wire.rs#L622-L685
[fp-reuse]: ../../../../../crates/daemon/src/wire.rs#L775-L786
[t-inc]: ../../../../../crates/daemon/src/wire.rs#L1520
[t-reattach]: ../../../../../crates/daemon/src/wire.rs#L1707
[hyg-output]: ../../../../../crates/daemon/src/tail_hygiene.rs#L572-L591
[part-measure]: ../../../../../crates/daemon/src/tail_hygiene.rs#L599-L622
[hygiene]: ../../../../../crates/daemon/src/tail_hygiene.rs#L566-L620
[hyg-text]: ../../../../../crates/daemon/src/tail_hygiene.rs#L630-L648
[hyg-input]: ../../../../../crates/daemon/src/tail_hygiene.rs#L649-L659
[t-hyg-cold]: ../../../../../crates/daemon/src/tail_hygiene.rs#L1147
[t-hyg-golden]: ../../../../../crates/daemon/src/tail_hygiene.rs#L2289
[count-digest]: ../../../../../crates/daemon/src/token_cache.rs#L103-L143
[sidecar-inc]: ../../../../../crates/daemon/src/codec/opencode.rs#L258-L293
[sidecar-merge]: ../../../../../crates/daemon/src/codec/opencode.rs#L277-L291
[raw-clone]: ../../../../../crates/daemon/src/codec/opencode.rs#L244
[t-sidecar]: ../../../../../crates/daemon/src/codec/opencode.rs#L2082
[decode-sidecar]: ../../../../../crates/daemon/src/codec/sidecar.rs#L46-L55
[remember]: ../../../../../crates/daemon/src/codec/sidecar.rs#L67-L73
[sel-kind-enum]: ../../../../../crates/daemon/src/selection.rs#L111-L126
[sel-consume]: ../../../../../crates/daemon/src/selection.rs#L323-L331
[boundary-block]: ../../../../../crates/daemon/src/boundary.rs#L84-L102
[boundary-consume]: ../../../../../crates/daemon/src/boundary.rs#L1727-L1746
[injection-consume]: ../../../../../crates/daemon/src/injection.rs#L400-L403
[todo-prefix]: ../../../../../crates/daemon/src/injection.rs#L187-L189
[chunk-build]: ../../../../../crates/daemon/src/historian_chunk.rs#L350-L381
[segment-served]: ../../../../../crates/daemon/src/dispatch.rs#L50-L72
[compaction-default]: ../../../../../crates/daemon/src/config.rs#L121
[tail-reclaim]: ../../../../../crates/daemon/src/healing.rs#L130-L139
[ser-msg]: ../../../../../crates/memory-store/src/lib.rs#L145-L161
[from-parts]: ../../../../../crates/memory-store/src/lib.rs#L166-L180
[meta-doc]: ../../../../../crates/memory-store/src/lib.rs#L210-L216
[ser-block]: ../../../../../crates/memory-store/src/lib.rs#L266-L280
[tagrow]: ../../../../../crates/memory-store/src/lib.rs#L1798-L1806
[mint-prepared]: ../../../../../crates/memory-store/src/lib.rs#L7758-L7766
[load-order]: ../../../../../crates/memory-store/src/lib.rs#L7847-L7875
[serde-features]: ../../../../../Cargo.toml#L45
[t-segments]: ../../../../../crates/daemon/tests/prepared_output.rs#L32-L52
[t-seldiff]: ../../../../../crates/daemon/tests/selection_differential.rs#L1-L5
[bench]: ../../../../../crates/daemon/benches/hot_path.rs#L68-L126
