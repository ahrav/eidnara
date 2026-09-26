# Existing decode, ownership, and mutation checks

System: `/local/home/ahrav/scratch/eidnara`.
HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`, inspected 2026-09-13.
The [catalog scope](catalog.md#scope-and-provenance) records the worktree-only
settled plan, host contract, A1-A3/B1/W1, and issue leads. No separate incident
or external repository evidence is supplied. No tests run in this pass.

Every entry below is **unaudited**. A description reports source assertions
and guards, not an adequacy verdict. Automatic assert_eq diagnostics are
noted as equality checks; notable custom messages are quoted. Current tests
that contradict the accepted prospective contract remain listed.

## Source key

All line numbers are HEAD locations. Daemon lib.rs is read with `git show`
because the worktree file is dirty. Other cited source files match HEAD.

| Key | File |
| --- | --- |
| D | `crates/daemon/src/lib.rs` |
| W | `crates/daemon/src/wire.rs` |
| M | `crates/memory-store/src/lib.rs` |
| T | `crates/daemon/src/transform.rs` |
| O | `crates/daemon/src/codec/opencode.rs` |
| P | `crates/daemon/src/codec/pi.rs` |
| H | `crates/daemon/tests/direct_host.rs` |
| S | `crates/daemon/tests/serialized_transform_pages.rs` |
| TS | `packages/opencode-plugin/src/hooks/context/module-wire.test.ts` |

The closed inspection surface is wire-envelope serde, body routing/fallback,
page digest/assembly, direct shared-prefix ownership, and serializer mutation
authority. Downstream identity/history_summarizer checks and budget/allocation checks
stay in the canonical linked inventories. Codec and producer checks below
cover retained payload shape and mutation effects at this boundary.

## Runtime checks and type constraints

| Check and location | Condition, semantics, and failure message | Status |
| --- | --- | --- |
| M:114-143,243-264,326-447 serde conversion | Typed required fields, scalar types, tagged variants, and defaults are checked after Value construction; serde errors propagate. Unknown fields are ignored by the mirrors but retained in original. | unaudited |
| D:12923-12938 direct attempt | Refused returns its existing refusal; Invalid restarts and falls through. This is a production branch guard, not a test assertion. | unaudited |
| D:12941-12951 tree decode | Invalid tree input becomes Null for dispatch; valid Value goes to the normal route handler. | unaudited |
| D:8115-8123 tree-to-typed conversion | A serde error returns `bad_request` with its diagnostic. | unaudited |
| D:15805-15824,15953-15964 probe | Last discriminator occurrence wins; string method overrides kind; any page key excludes direct transform. | unaudited |
| D:15864-15949 compatibility walk | Recursive serde validation plus raw-token-key refusal with `raw-value token names an object key`. | unaudited |
| D:9575-9578 page completeness | All six page fields must be present, else `transform page envelope must be all-or-none`. | unaudited |
| D:9579-9610 page types | ID bounds and unsigned generation/index/total, boolean complete, and string digest; individual invalid_params diagnostics name the field. | unaudited |
| D:9613-9627 page position | Nonzero total, index below total, and complete iff final; `protocol_mismatch`. | unaudited |
| D:9629-9645 nonfinal keys | Only routing/session/generation/page-envelope/array keys on nonfinal pages; `non-final transform pages may carry only message arrays`. | unaudited |
| D:9647-9653 raw content digest | Compare supplied digest before typed normalization; `digest_mismatch`. | unaudited |
| D:9682-9700 completed replay input | Match generation, total, final array digest, and scalar digest before replay; `attempt_mismatch` or `digest_mismatch`. | unaudited |
| D:15055-15126 continuation assembly | Validate marker field, position, chunk sequence/total, and final JSON; named continuation errors propagate. | unaudited |
| D:15129-15171 page assembly | Final page must be an object; listed fields must be arrays; `transform page field {field} must be an array`. | unaudited |
| D:13036-13046 non-transform dispatch | Explicit echo retains its tree; unknown shapes reject and facade shapes take their distinct arm. | unaudited |
| W:214-240 prefix reconstruction | Bounds, metadata count, block-end range, and matching mid/ordinal/role/index are required, else None; successful result clones Arc handles. | unaudited |
| W:516-517 builder assertions | debug_assert_eq checks message/end/state counts. Release enforcement is not established by this inventory. | unaudited |
| W:528-537 projection input validation | Empty/reserved/duplicate mids reject before block construction. These are downstream guards, not JSON duplicate-key rejection. | unaudited |
| W:540-559 synthetic override | Share only original-free, same-synthetic shells; otherwise build a typed shell. KTD1 intentionally removes the original-free condition. | unaudited |
| D:4328-4365 tail expansion | Malformed/missing delta fields, fingerprint/state absence, or out-of-range prefix return None; no stricter typed tail_delta is introduced. | unaudited |
| T:2016-2045 projection differential | Enabled in unit tests or by EIDNARA_PREFIX_PROJECTION_DIFFERENTIAL=1; asserts `incremental prefix projection byte drift` and `incremental prefix projection state drift`. | deleted by #828 with the mechanism it checked |
| W:1796-1800 compile constraints | Local generic assertions require IngressMessages, FlatProjection, and TransformRequest to be Send + 'static when tests compile. | unaudited |

No production assertion specifically forbids envelope-tree retention or stale
typed replay. Those properties require prospective representation and mutation
witnesses. Admission and pool guards stay in A1/A3 rather than this table.

## Decode, routing, and page checks

| Check and location | Existing assertion surface | Status |
| --- | --- | --- |
| D:19649-19706 `entry_probe_reads_the_route_and_the_page_envelope_as_dispatch_does` | Route precedence, nonstring method, overlong/escaped route, all null page keys, repeated method, malformed/nonobject input, and depth boundaries. | unaudited |
| D:19712-19741 `tree_parse_witness_refuses_what_the_tree_refuses` | Compare tree/witness on malformed shapes; raw-token exceptions; sorted retained Value re-read errors. | unaudited |
| D:19747-19759 `raw_value_token_matches_serde_json` | Document-string token decodes; nonstring or following-key forms reject; later-position token remains an ordinary map key. | unaudited |
| D:19986-20117 `direct_and_tree_transform_decodes_agree_on_the_corpus` | Compare decoded serialization and negative-zero sign; pin accepted/tree-only/direct-only lists; rejected gate for direct-only cases; assembled valid pages equal one-slice request. | unaudited |
| D:20140-20181 `unpaged_transform_bodies_reach_the_same_outcome_through_both_entry_paths` | Compare response minus timings or exact code/message; pin direct-lane names; valid body must produce response. | unaudited |
| T:16470-16497 `transform_request_parses_full_flat_wire_envelope` | Assert typed envelope values and defaults after from_value. | unaudited |
| W:1796-1814 `shared_ingress_is_send_and_preserves_decode_refusals` | Compare owned/shared array serde error text on eight malformed shapes, in addition to mobility constraints. | unaudited |
| D:29330-29384 `dispatch_routes_each_envelope_class_to_a_distinct_arm` | Transform succeeds, echo preserves probe, facade rejects distinctly, unknown keys/nonobject root identify the shape. | unaudited |
| D:30224-30257 `transform_page_scalar_digest_covers_non_array_fields_only` | Arrays and page metadata do not alter scalar digest; changed scalar does. | unaudited |
| D:30381-30389 `assemble_transform_pages_rejects_a_non_array_page_field` | Early and final-only nonarray messages fail; diagnostic contains `messages must be an array`. | unaudited |
| H:49-129 `readiness_permissions_catalog_and_real_unary_transform` | Real routed unary transform completes alongside fixture readiness/permissions checks; not the full decode differential corpus. | unaudited |
| S:11-156 `serialized_transform_corpus_preserves_host_admission_and_completion` | Ignored generated-corpus test; page byte/hash, staged/final outcomes, malformed host refusal, and final messages equal unpaged control. Requires 19 cases and fixed completion counts. | unaudited |

The corpus helper is D:19762-19964. It is not itself an assertion. Its nested
duplicate changes IngressMessage.mid, not CK role or WireBlock.kind. The
two-page unit assembly uses placeholder digests and bypasses page validation.

Adjacent collector checks remain separately named because their semantics
must survive the retained tree path; they do not prove wire serde correctness:

| Check and location | Existing assertion surface | Status |
| --- | --- | --- |
| D:30015-30040 `transform_page_discard_removes_the_session_entry` | Discard removes entry and resets staged counts. | unaudited |
| D:30043-30077 `transform_page_admission_ignores_sessions_without_a_pending_phase` | Refused stage leaves no empty session entry. Budget policy stays in the handler catalog. | unaudited |
| D:30080-30137 `transform_page_completed_responses_share_one_budget_and_evict_the_oldest` | Oldest completed entry evicts; oversize completion is absent. Accounting detail stays elsewhere. | unaudited |
| D:30140-30170 `transform_page_failed_apply_leaves_no_session_entry` | Failed apply removes staged/completed state. | unaudited |
| D:30173-30221 `transform_page_completed_response_is_not_replayable_while_a_collector_is_live` | Live collector suppresses completed replay with a named assertion. | unaudited |
| D:30330-30378 `transform_page_stale_collectors_are_evicted_after_the_ttl` | Stale collection is evicted under controlled collector time. | unaudited |

## Sharing, mutation, and typed equality checks

| Check and location | Existing assertion surface | Status |
| --- | --- | --- |
| M:16096-16112 `a_block_edit_leaves_its_sibling_byte_identical` | Edited first text serializes; second text and unknown sibling field survive. Unknown-field expectation contradicts accepted R3. | unaudited |
| W:1447-1517 `opaque_and_media_inside_tool_result_content_are_accepted_and_projected` | Opaque content is accepted; replacing nested result content with media projects its URL. | unaudited |
| W:1521-1574 `incremental_projection_reuses_prefix_storage_and_preserves_tool_arc_state` | Reattachment equality, full/incremental equality, shared backing/bytes, and pending tool-arc continuation. | deleted by #828 with the mechanism it checked |
| W:1578-1589 `empty_and_reserved_message_ids_are_rejected` | Empty and reserved mids produce their errors. | unaudited |
| W:1595-1620 `duplicate_message_ids_are_rejected_across_the_incremental_prefix` | Duplicate mids reject in full and incremental projection. | unaudited; #828 deletes the incremental arm and renames the test `duplicate_message_ids_are_rejected` |
| W:1624-1704 `reduced_tool_result_keeps_failure_variant_and_output_extras` | Each success/error/denied output keeps its classification and provider extras after reduction. | unaudited |
| W:1708-1745 `reattach_keeps_block_level_original_but_rebuilds_the_message_shell` | Known shell fields survive; unknown message field drops; block unknown/original survives; raw ingress remains unchanged. Old retention assertions conflict with KTD1/R3. | unaudited |
| W:1749-1792 `repeated_prefix_reattachment_shares_canonical_shells` | Repeated sharing, equivalent projections, input preservation, copy-on-write isolation, and block owner after projection drop. Original-presence assertions need explicit revision. | deleted by #828 with the mechanism it checked |
| W:1818-1860 `incremental_projection_checks_effective_synthetic_status` | All four old/new flag pairs compare full/incremental value and pointer reuse exactly when flags match. | deleted by #828 with the mechanism it checked |
| T:13714-13755 `served_canonical_shell_bytes_and_segments_are_frozen` | Literal raw/latent/typed/edited bytes and prepared segments. Latent public-meta mutation is deliberately invisible under the old oracle. | unaudited |
| T:13760-13793 `served_canonical_frozen_corpus_matches_value_reference_for_both_shells` | Original/fully typed shell serialization and pairwise equality/digest relation over the fixture. Digest details go to the identity agent. | unaudited |
| T:13797 `served_fingerprint_fallback_preserves_complete_identity_and_first_match` | Null block rejection and latent extras/original distinctions among candidates; receipt/hash details belong to the identity agent. | unaudited |
| D:20584-20621 `transform_snapshot_cache_is_generation_safe_and_lru_bounded` | Ready/in-flight/missing transitions, stale finish rejection, retained ready entries, and eviction. | unaudited |
| D:22568-22882 `incremental_native_cache_replays_complex_prefix_and_encodes_only_tail` | Fresh/reattached/shared equality, shared ingress/native/sidecar pointers, suffix encoding, and native output copy-on-write isolation. | unaudited |
| D:22886-23004 `native_delta_ingress_core_is_independent_of_changed_output_messages` | Changed output does not replace raw ingress; evicted caches recover CK/native prefix pointers from ready snapshot. | unaudited |
| D:38911-39024 `compaction_mode_projection_cache_reclassifies_synthetic_prefix` | Off/on transition sequence compares full projection, flags, stable prefix reuse, and unchanged request serialization. | deleted by #828 with the mechanism it checked |
| T:2031-2045 `assert_message_projection_equivalent` | Shared runtime/unit differential detailed above; test adapter at T:2024-2028 delegates. | deleted by #828 with the mechanism it checked |
| D:24532 `projection_differential_catches_corrupt_first_changed_position` | Corrupt projection frontier triggers differential rejection. | deleted by #828 with the mechanism it checked |
| D:24059 `differential_assert_rejects_frontier_inside_mutated_native_region` | Corrupt native frontier triggers differential rejection. | unaudited |

## Retained native payload and producer checks

Native sidecar raw replay remains distinct from the envelope original removed
by KTD1. Pi codec tests are secondary type consumers, not proof Pi currently
drives this transform ingress path.

| Check and location | Existing assertion surface | Status |
| --- | --- | --- |
| O:1531-1558 `mutated_survivor_keeps_its_own_native_extras_after_sibling_deletion` | Edited surviving part retains its own vendor data after adjacent deletion. | unaudited |
| O:1741-1780 `mark_modified_tool_mutation_preserves_native_time_verbatim` | Changed tool output is visible while native time remains equal. Marking calls require KTD1 cleanup. | unaudited |
| P:1199-1241 `mutated_text_survivor_keeps_its_own_signature_and_vendor_extras` | Edited text retains the surviving part's signature/vendor extras. | unaudited |
| P:1245-1267 `untouched_multi_text_tool_result_replays_raw_part_boundaries_and_extras` | Content output and re-encoded native value equal the original multi-part input. | unaudited |
| P:1418-1438 `untouched_message_replays_the_exact_retained_raw_value` | Native sidecar envelope equality with unknown native data. This replay is retained by KTD3. | unaudited |
| P:1441-1455 `deleted_tool_result_does_not_replay_the_retained_raw_entry` | Cleared content yields no native output rather than replaying raw entry. | unaudited |
| TS:19 collapsed synthetic pair | Meta identifies synthetic CK ingress. | unaudited |
| TS:45 repeated call part | Later repeated call emits result without a second call. | unaudited |
| TS:69 unfinished tool input | First-seen unfinished tool still emits a tool_call. | unaudited |
| TS:84 nonobject opaque parts | Nonobject payloads preserve their raw opaque values. | unaudited |
| TS:120 creation timestamp | Nested timestamp precedes aliases; absent timestamp is omitted. | unaudited |
| TS:134 empty reasoning signature | Explicit empty signature survives. | unaudited |
| TS:159 empty tool-call ID | Explicit empty ID survives encoding. | unaudited |
| TS:177 zero ordinal | Zero remains zero, including harness metadata. | unaudited |
| TS:190 invalid ordinal | Values outside daemon-readable unsigned range use fallback ordinals. | unaudited |
| TS:202 provider execution | True reaches call/result; false/default field is omitted. | unaudited |
| TS:241 tool-name aliases | Alias/default names match literal expected names. | unaudited |
| TS:301 nested signature | Signature extraction returns the expected three values. | unaudited |
| TS:327 completion output | Completed/error top-level output keeps text/error_text polarity. | unaudited |
| TS:370 result attachments | Attachments retain content blocks and error_content polarity. | unaudited |
| TS:441 step-finish opaque | Raw step-finish data and opaque source shape survive. | unaudited |
| TS:467 metadata extras | Part metadata is carried in provider extras. | unaudited |
| TS:496 redacted reasoning | Redacted blobs and visible reasoning select the expected variants. | unaudited |
| TS:516 opaque approval arc | Recognized approval IDs produce arcs; unrelated parts omit them. | unaudited |
| TS:559 role fallback | Info/raw/default role precedence is explicit. | unaudited |
| TS:572 origin | Provider/model/API origin survives; unavailable origin is omitted. | unaudited |
| TS:597 media | File/image parts match full typed media shapes and sources. | unaudited |
| TS:656 reasoning module golden | Fixed fixture names and encoded CK inputs match the checked-in golden. | unaudited |

## Page-producer checks

| Check and location | Existing assertion surface | Status |
| --- | --- | --- |
| TS:688 canonical numbers | Canonical number and nested JSON strings match literal expected strings. | unaudited |
| TS:717 serde-compatible compact values | Number, escape, Unicode-key, and nested-value encodings match literals. | unaudited |
| TS:786 Unicode key order | Code-point sorting differs from UTF-16 default ordering as expected. | unaudited |
| TS:1277 unpaged body | First stringify size and unchanged page value are returned. | unaudited |
| TS:1289 sparse arrays | Paged/unpaged arrays preserve null slots and positions. | unaudited |
| TS:1309 reserved continuation key | Carrier is encoded as a continuation and reconstructs the original item. | unaudited |
| TS:1340 page sizes | Each returned size matches later JSON.stringify output. | unaudited |
| TS:1360 pageable fields | Rust literal field list matches producer paging behavior; unrelated arrays remain final-page scalars. This reads the worktree Rust file at execution time. | unaudited |
| TS:1394 toJSON snapshot | Source toJSON runs once; final scalar marker matches that snapshot; emitted envelopes serialize once. | unaudited |
| TS:1425 unpaged carrier | No parse call; serialized text and message pointer are preserved. | unaudited |
| TS:1447 convergent paging | Item measurement occurs once across convergence attempts; body snapshot parse occurs once. | unaudited |
| TS:1477 Rust-expanded numbers | Three boundary bodies remain unpaged and retain exact submitted text. | unaudited |

## Linked checks outside this slice

The [canonical latency inventory](../../hot-path-optimization/latency-audit/existing-checks.md)
owns all A1/A3 charge, refusal, heap-peak, and ring-terminal checks, and B1's
broader projection/native-cache suite. Per-check adequacy remains unaudited
here even where historical execution receipts exist there. The identity agent
owns projection goldens, block-basis checks, fingerprints, and history_summarizer decode.
No numerical accounting or identity guarantee is duplicated in this catalog.

## None found and suspiciously quiet areas

- No direct structural check forbids an envelope Value or renamed original.
- No CK role/WireBlock kind duplicate witness observes all gate, Invalid,
  successful tree-conversion, and final Tree preconditions jointly.
- No explicit-null serializer_profile case exists in the inspected A2 corpus.
  It is a boundary-preservation witness, not authorization for the deferred
  wrapper refactor.
- No before/after raw-value-token witness targets a discarded field under ck.
  The parity hypothesis is source-backed but unconfirmed; any observed
  acceptance change requires stop-and-report, with no authorized exception.
- No one mutation matrix covers every public shell field plus no-op mutable
  access and decoded-versus-constructed typed equality under KTD1.
- No scoped test drops the original input byte buffer and then drives both
  projection-prefix and ready-snapshot fallback with the final representation.
- No scoped raw-page digest witness explicitly alters a discarded CK field
  while preserving the supplied digest, then retries with the updated digest.
- No implemented independent per-marker `sometimes` checks are present for
  the two new fallback witnesses; one marker must not mask the other.
- No new assertions, fixtures, tests, fuzz campaigns, allocation probes, or
  benchmark results are created here.

The independent portfolio review by `ses_f6756093fffeVjNp36S3E8pKrM` is
completed and dispositioned on 2026-09-13. That review does not execute or
audit the adequacy of the listed checks; every per-check status stays
unaudited, and every catalog claim stays unexercised. Worktree-only TE08
reconciliation remains open for the specification/integration owners.

Proposed checks are described in each evidence file and fault-map.md. Runtime
guard adequacy routes to defensive-assertions-and-invariant-guards; test
adequacy routes to invariant-test-review. Inventory presence is not coverage.
