# Lens records: group g3

Records re-verified against HEAD `5a842d998` on branch
`source-capture/1090-property-catalog`. Source records: #1066 comment 1
(Part A, baseline `08de9051a`). Code is cited by function, constant, and test
name.

### sc-reduction-drops-body-keeps-failure-variant

Type: safety
Reachability: default-production - `apply_once` in `crates/daemon/src/transform.rs` calls `select_reductions_with_outcome` under `producer_gate` on every transform pass, and its rebuild loop applies `wire::reduced_block`; no `source_capture` or other configuration key gates it
Status: active
Exercised: yes - `crates/daemon/src/wire.rs::reduced_tool_result_keeps_failure_variant_and_output_extras` constructs all eight `OutputKind` variants with output and block extras; `crates/daemon/src/selection.rs::frozen_arc_blocks_never_re_emitted` freezes one result id; `crates/daemon/src/transform.rs::agent_drop_ids_freeze_add_only_through_flat_ids`, `coverage_extending_soft_prunes_covered_red_units`, `host_reversibility_true_reduction_stays_canonical_after_false_flip`, and `owned_profile_drains_pending_drops_without_request_surface` assert served `[dropped]` bytes; `reduced_block_renders_placeholder_without_tag_prefix` asserts the tagged `[dropped §2§]` form
Guarantee: When reduction drops a tool arc, every unfrozen result block becomes a `[dropped]` placeholder (`[dropped §N§]` under a tag overlay) with its `OutputKind` class preserved (`Text|Json|Content` to `Text`, `Error*` to `ErrorText`, `ExecutionDenied` kept with the reason replaced), its id, tool name, `provider_executed`, and both extras maps kept, and the native output bytes absent.
Check: `always` - every `ReductionDecision` that `expand_arc` emits for a result id has kind `drop` and payload `"[dropped]"`; `reduced_block` of an `Error*` output has kind `ErrorText { text: <placeholder> }`, of `ExecutionDenied` has `reason: Some(<placeholder>)`, and of the rest has `Text`; no reduced block holds a byte of the native output body; no decision names a frozen id; a pure per-block mapping, so it must hold on every evaluation
Fault/timing angle: a frozen result id is skipped by `expand_arc`; a tag overlay changes the served placeholder text
Required faults and enabling state: F20, an arc selected for `FullDrop` or `DedupFullDrop` with each `OutputKind`; a frozen result id; a tag overlay naming the dropped block
Confidence: high - [evidence](evidence/sc-reduction-drops-body-keeps-failure-variant.md). Read `DROPPED_PLACEHOLDER` and `expand_arc` in `selection.rs`, `reduced_block` in `wire.rs`, the `apply_once` rebuild loop in `transform.rs`, and the OpenCode tool-output decode in `codec/opencode.rs`; read each test named above; extras are copied verbatim and their contents are not audited
Existing check: `wire.rs::reduced_tool_result_keeps_failure_variant_and_output_extras`; `selection.rs::frozen_arc_blocks_never_re_emitted`; the five `transform.rs` tests named in `Exercised`; all unaudited
Impact: Collapsing `Error*` to `Text` tells the model a failed call succeeded; once dropped, the body survives only in a retained `raw_tool_spans` source, which at HEAD exists only when `source_capture.enabled` is set (AE2)
Open questions:
- Can a codec place native output bytes in `provider_extras`, which `reduced_block` copies verbatim? (unresolved, needs an extras-population audit per codec)

### sc-tool-call-line-carries-no-outcome

Type: safety
Reachability: default-production - `build_history_summarizer_chunk` is called from `maybe_spawn_reattach` in `crates/daemon/src/lib.rs` and from the summarizer firing path in `transform.rs`; the boundary chunk runs in `check_history_segment_trigger_with_index` in `crates/daemon/src/boundary.rs`; neither reads `source_capture.enabled`; the summarizer's own firing conditions are not re-verified here
Status: active
Exercised: yes - `crates/daemon/src/history_summarizer_chunk.rs::tool_output_contains_sentinel` places `SENTINEL-7f3a` on a middle output line and asserts it is absent while `head` and `tail` render; `crates/daemon/src/boundary.rs::the_boundary_chunk_renders_digests_for_answered_calls` does the same at the second site with answered and unanswered calls, with and without narrative; `an_error_digest_shows_its_first_line_and_a_forged_marker_targets_nothing` constructs F17; `a_digest_cut_by_the_budget_withdraws_its_alias` constructs F16; `duplicate_tool_call_ids_resolve_results_by_arc_id` and `tool_result_only_message_absorbs_into_preceding_assistant_block` cover arc lookup and tool-only absorption
Guarantee: At both `TC:` construction sites a tool call renders as `TC: {description}`, `TC: {name}({key_arg})`, or `TC: {name}`, and a tool result renders that summary followed by the outcome digest from `tool_outcome::outcome_digest`; no tool output byte outside the `head` and `tail` excerpts enters the chunk; tool-only parts are `transformed: true`; native marker brackets are escaped; an alias cut by truncation is withdrawn.
Check: `always` - every call summary starts with `TC: `; the key argument is the first of `filePath|path|pattern|query` cut at 60 scalars with `…`, else the first of `symbol|module|action` uncut; no sentinel placed on a tool output line other than the first and last nonblank lines appears in the chunk at either site; every tool-only `FrozenAlias.transformed == true`; after `presented_input` every remaining alias's `marker + presented` occurs in order in the kept prefix; asserted on every built chunk because the chunk is a pure function of its blocks and budget
Fault/timing angle: a token budget cutting inside a digest (F16); a forged alias marker in output (F17)
Required faults and enabling state: tool results whose middle lines carry a sentinel, at both sites (`tool_output_contains_sentinel` precondition); an error result; F16; F17; an unanswered call in a message with and without narrative
Confidence: high - [evidence](evidence/sc-tool-call-line-carries-no-outcome.md). Read `format_tool_summary`, `tool_parts`, `ChunkBuilder::part`, `presented_input`, `extract_key_arg`, `tool_outcome::{from_output, outcome_digest, digest, append_digests}`, and the boundary `tool_results`, `tool_call_parts`, `chunk_message_part`; read each named test; verified both sites share one formatter and pass `exit_code: None`
Existing check: the six tests named in `Exercised`; `crates/daemon/tests/history_summarizer_truncate_differential.rs::optimized_matches_frozen_reference_at_production_windows` covers the truncation function the withdrawal relies on; all unaudited
Impact: The full log, or a secret in its middle, reaching the summarizer prompt; a citation naming bytes the model never saw whole
Open questions:
- OQ17, second `TC:` site: resolved in U6 (`ticket-1088.md`): both sites render through `format_tool_summary` and `tool_outcome::digest`; verified in `boundary.rs::tool_call_parts` and its test
- The slug name and the baseline guarantee ("no status, exit code, or error text") no longer describe HEAD, where the line carries status, byte count, and two excerpts; should the lead rename the slug or invalidate it in favor of `sc-digest-bounded-deterministic-char-boundary`? (needs human input)

### sc-project-docs-canonical-hash-over-fixed-list

Type: safety
Reachability: default-production - `inject_docs` defaults `true` in `DaemonConfig::default()`; `ConfigKey::MemoryClassifierInjectDocs` (`/memory_classifier/inject_docs`) is `TierClass::UserOnly`; the m0 composer in `crates/daemon/src/m0_compose.rs` calls `read_project_docs_canonical` whenever `inputs.inject_docs` holds; the raw-byte capture beside it is a separate, `explicit-config-only` record
Status: active
Exercised: yes - `crates/daemon/src/project_docs.rs` tests `project_docs_golden_matches_reference` (six golden cases, case 3 holds a BOM, CRLF, trailing space and tab, and trailing newlines), `symlinked_doc_is_skipped`, `a_doc_that_outgrows_the_cap_under_the_read_is_skipped` (sparse oversize and FIFO), `oversized_doc_is_skipped`, `a_file_is_missing_only_from_a_directory_that_exists`; `crates/daemon/src/source_capture_tests.rs::a_configured_document_is_one_lineage_whose_edits_supersede_and_removal_retires` asserts a whitespace-only edit leaves `canonical_hash` unchanged; `document_capture_leaves_the_response_and_respects_inject_docs` covers `inject_docs: false`
Guarantee: The block covers only `ARCHITECTURE.md` then `STRUCTURE.md`, each read without following a final-component symlink as a regular file of at most 256 KiB of UTF-8 or skipped, and the hash covers a canonical form, so whitespace-only edits change neither hash nor block while raw bytes differ; `M0Composition::docs_hash` has no reader.
Check: `always` - every `crates/daemon/testdata/project-docs-golden.json` case reproduces `rendered_block` and `canonical_hash`; an edit touching only a BOM, CRLF, trailing spaces or tabs, or trailing newlines yields an identical hash and block; a symlink, FIFO, oversize, or non-UTF-8 file is skipped while the other renders; a pure function of two file reads, so it must hold on every evaluation
Fault/timing angle: the open is `NOFOLLOW` and the descriptor type and cap are checked on the opened file, so a swap of the file after the open changes nothing; a parent-directory swap before the open is outside `NOFOLLOW`
Required faults and enabling state: whitespace-only edits; F18 fixtures (symlink, FIFO, sparse oversize, non-UTF-8); `inject_docs: false`
Confidence: high - [evidence](evidence/sc-project-docs-canonical-hash-over-fixed-list.md). Read `PROJECT_DOC_FILES`, `MAX_PROJECT_DOC_BYTES`, `canonicalize_doc_content`, `read_document`, `read_project_docs_canonical`, `leave_reads`, `config::read_bounded_file`, and the composer; read each named test; the golden file's git history shows only its initial import
Existing check: the seven tests named in `Exercised`; all unaudited
Impact: A whitespace-only edit evicting the cached m0 prefix; a symlink or special file exfiltrating bytes into the prompt; the canonical hash is no revision trail, which U10's `RepositoryDocuments` capture supplies (R7)
Open questions:
- `crates/daemon/src/history_segment_coverage.rs` says `docs_hash` is persisted with the rendered m0 bytes: resolved by a grep of `crates/`, `packages/`, SQL, and Markdown at HEAD, which finds no reader or writer beyond the assignment in `m0_compose.rs`; the comment disagrees with the code

### sc-ready-snapshot-drops-native-messages

Type: safety
Reachability: default-production - the transform handler in `crates/daemon/src/lib.rs` builds `Arc::new(parsed.ready_snapshot())` and calls `TransformSnapshotCache::finish_ready` on every pass that reaches ready, with no configuration gate
Status: active
Exercised: yes - `crates/daemon/src/lib.rs::ready_snapshot_shares_messages_and_charges_at_least_their_bytes` sets `native_messages: Some` and asserts `None` on the snapshot, `Arc::ptr_eq` on `messages[0]`, and a charge that grows with message bytes; `ready_snapshot_holds_no_native_payload_and_a_lease_pins_its_generation` sends a native request with a 64 KiB `provider_data` field through the handler and asserts the lease holds no native messages and is charged under 64 KiB
Guarantee: The ready snapshot carries `native_messages: None` and a resolution-only coverage, shares the CK message `Arc`s, and its retained-byte charge excludes native payload; the native window a consented pass hands to source capture lives in the `CaptureHandoff` under its own `RetainedCharge`.
Check: `always` - `request.ready_snapshot().native_messages.is_none()`; `Arc::ptr_eq(&snapshot.messages[i], &request.messages[i])` for every `i`; `snapshot_retained_bytes` excludes native payload; the `debug_assert!` in `finish_ready` holds in debug builds, and release builds enforce nothing at that point; asserted on every snapshot because the cache budgets charge exactly what the snapshot keeps
Fault/timing angle: none
Required faults and enabling state: a request with `native_messages: Some` (`opencode-aisdk` or `pi` profile) and a large native payload; a pass reaching ready
Confidence: high - [evidence](evidence/sc-ready-snapshot-drops-native-messages.md). Read `ready_snapshot`, `snapshot_retained_bytes`, `finish_ready`, the handler's snapshot and `source_capture::Offer` construction, and `SourceCapture::offer`; read both named tests
Existing check: the two `lib.rs` tests named in `Exercised`; the `debug_assert!` in `finish_ready`; `crates/daemon/src/transform/surface_census.rs::auto_search_is_enabled_by_the_wire_default_and_drives_the_hint_query` asserts a CK-profile request has no native window and does not exercise the snapshot; all unaudited
Impact: The ready LRU and active-lease budget under-charging the native bytes a snapshot pins; D1 holds because capture takes its own charged reference instead of the snapshot's
Open questions:
- Should release builds enforce the absence in `finish_ready`? (unresolved, needs `/low-level-systems:defensive-assertions-and-invariant-guards`)

### sc-session-delete-leaves-kernel-descriptors

Type: safety
Reachability: default-production - `handle_session_delete_value` in `crates/daemon/src/lib.rs` calls `retire_captured_sources` on every `session.delete`; it skips the Kernel only when `KernelOpenCoordinator::expected` (`crates/daemon/src/kernel_routes/mod.rs`) is false, which defaults `true`; `source_capture.enabled` does not gate it
Status: invalidated - U8 (#1087) inverted it: `handle_session_delete_value` now calls `retire_captured_sources`, which runs `source_capture::retire_session_pages`, before `store.delete_session`; the obligation is carried by `sc-delete-retires-before-memory-store-delete`, with `sc-delete-retry-recovers-crash-between-stores` and `sc-deleted-session-not-resurrected-by-queued-capture`
Exercised: yes - the baseline guarantee is refuted at HEAD by `crates/daemon/src/source_capture_tests.rs::session_delete_retires_captured_sources_and_a_retry_recovers_a_stop_between_stores`, which captures two sessions, deletes one, and asserts zero live descriptors for it and the other untouched
Guarantee: At baseline `08de9051a`, `session.delete` deleted memory-store rows and purged the memory-capture memo under `capture_commit_gate` and made no Kernel call.
Check: `always` - after a delete, Kernel descriptor inventories are unchanged; false at HEAD for any session with live `messages` or `raw_tool_spans` descriptors
Fault/timing angle: none
Required faults and enabling state: a session with captured descriptors; a delete
Confidence: high - [evidence](evidence/sc-session-delete-leaves-kernel-descriptors.md). Read `handle_session_delete_value` at both commits, `retire_captured_sources`, `KernelOpenCoordinator::expected`, `retire_session_pages`, `session_page_intent`, `session_page`, and the five U8 tests
Existing check: at baseline, none for the Kernel; the inverting tests at HEAD are `session_delete_retires_captured_sources_and_a_retry_recovers_a_stop_between_stores`, `session_delete_with_the_kernel_starting_deletes_nothing`, `a_delete_before_the_first_publication_drops_the_pending_capture`, `session_delete_retires_only_its_own_captures_and_recovers_a_stop_between_pages`, `session_retirement_pages_are_keyed_by_session_and_ids`; memory-store gate tests `lib.rs::a_cancelled_session_delete_still_purges_in_memory_state`, `explicit_capture_checkpoints_serialize_with_session_delete_off_the_worker`, `session_delete_waits_for_the_capture_gate_off_the_runtime_worker`; all unaudited
Impact: Inverted: every `session.delete` on a daemon that expects a Kernel now depends on Kernel readiness, including sessions with no captures
Open questions:
- OQ12, Kernel unavailable during delete: resolved in U8 as fail closed; verified in `retire_captured_sources` and `session_delete_with_the_kernel_starting_deletes_nothing`
- OQ4, post-delete passes: resolved within approved scope in U8 (no tombstone; a running harness session republishes, as the handler doc states); the tombstone stays the product owner's decision (needs human input)

## Existing checks

| Check | Covers | Status |
| --- | --- | --- |
| `crates/daemon/src/wire.rs::reduced_tool_result_keeps_failure_variant_and_output_extras` | eight `OutputKind` mappings, identity, both extras maps | unaudited |
| `crates/daemon/src/selection.rs::frozen_arc_blocks_never_re_emitted` | a frozen result id suppresses every decision of its arc | unaudited |
| `crates/daemon/src/transform.rs::agent_drop_ids_freeze_add_only_through_flat_ids` | served `[dropped]` bytes after agent drops | unaudited |
| `crates/daemon/src/transform.rs::coverage_extending_soft_prunes_covered_red_units` | served `[dropped]` bytes on soft prune | unaudited |
| `crates/daemon/src/transform.rs::host_reversibility_true_reduction_stays_canonical_after_false_flip` | frozen `[dropped]` payload across a flip | unaudited |
| `crates/daemon/src/transform.rs::owned_profile_drains_pending_drops_without_request_surface` | served `[dropped]` bytes on an owned profile | unaudited |
| `crates/daemon/src/transform.rs::reduced_block_renders_placeholder_without_tag_prefix` | tagged `[dropped §2§]` form | unaudited |
| `crates/daemon/src/history_summarizer_chunk.rs::tool_output_contains_sentinel` | no output byte outside the excerpts, chunk builder | unaudited |
| `crates/daemon/src/boundary.rs::the_boundary_chunk_renders_digests_for_answered_calls` | same at the boundary site; answered and unanswered calls | unaudited |
| `crates/daemon/src/history_summarizer_chunk.rs::an_error_digest_shows_its_first_line_and_a_forged_marker_targets_nothing` | F17; error status | unaudited |
| `crates/daemon/src/history_summarizer_chunk.rs::a_digest_cut_by_the_budget_withdraws_its_alias` | F16 alias withdrawal | unaudited |
| `crates/daemon/src/history_summarizer_chunk.rs::duplicate_tool_call_ids_resolve_results_by_arc_id` | call summary lookup by arc | unaudited |
| `crates/daemon/src/history_summarizer_chunk.rs::tool_result_only_message_absorbs_into_preceding_assistant_block` | tool-only absorption | unaudited |
| `crates/daemon/tests/history_summarizer_truncate_differential.rs::optimized_matches_frozen_reference_at_production_windows` | truncation against its reference | unaudited |
| `crates/daemon/src/project_docs.rs::project_docs_golden_matches_reference` | rendered block and canonical hash, six cases | unaudited |
| `crates/daemon/src/project_docs.rs::symlinked_doc_is_skipped` | symlink skipped, other file renders | unaudited |
| `crates/daemon/src/project_docs.rs::a_doc_that_outgrows_the_cap_under_the_read_is_skipped` | sparse oversize; FIFO without blocking | unaudited |
| `crates/daemon/src/project_docs.rs::oversized_doc_is_skipped` | oversize skipped | unaudited |
| `crates/daemon/src/project_docs.rs::a_file_is_missing_only_from_a_directory_that_exists` | `Missing` versus `Unreadable` | unaudited |
| `crates/daemon/src/source_capture_tests.rs::a_configured_document_is_one_lineage_whose_edits_supersede_and_removal_retires` | whitespace-only edit keeps `canonical_hash` | unaudited |
| `crates/daemon/src/source_capture_tests.rs::document_capture_leaves_the_response_and_respects_inject_docs` | `inject_docs: false`; response equality | unaudited |
| `crates/daemon/src/lib.rs::ready_snapshot_shares_messages_and_charges_at_least_their_bytes` | `native_messages: None`, `Arc::ptr_eq`, charge | unaudited |
| `crates/daemon/src/lib.rs::ready_snapshot_holds_no_native_payload_and_a_lease_pins_its_generation` | handler path, 64 KiB native payload uncharged | unaudited |
| `crates/daemon/src/lib.rs::TransformSnapshotCache::finish_ready` `debug_assert!` | absence in debug builds only | unaudited |
| `crates/daemon/src/transform/surface_census.rs::auto_search_is_enabled_by_the_wire_default_and_drives_the_hint_query` | CK profile parses with no native window | unaudited |
| `crates/daemon/src/source_capture_tests.rs::session_delete_retires_captured_sources_and_a_retry_recovers_a_stop_between_stores` | AE5, F23 cut, retry | unaudited |
| `crates/daemon/src/source_capture_tests.rs::session_delete_with_the_kernel_starting_deletes_nothing` | OQ12 fail closed | unaudited |
| `crates/daemon/src/source_capture_tests.rs::a_delete_before_the_first_publication_drops_the_pending_capture` | queued capture after delete | unaudited |
| `crates/daemon/src/source_capture_tests.rs::session_delete_retires_only_its_own_captures_and_recovers_a_stop_between_pages` | scope filter, stop between pages | unaudited |
| `crates/daemon/src/source_capture_tests.rs::session_retirement_pages_are_keyed_by_session_and_ids` | page receipt keys, replay | unaudited |
| `crates/daemon/src/lib.rs::a_cancelled_session_delete_still_purges_in_memory_state`, `explicit_capture_checkpoints_serialize_with_session_delete_off_the_worker`, `session_delete_waits_for_the_capture_gate_off_the_runtime_worker` | memory-store delete under `capture_commit_gate` | unaudited |

## Fault map rows

| fault id | injection at HEAD | availability | records |
| --- | --- | --- | --- |
| F16 budget truncation inside a part | `presented_input(built, budget)` with the budget search in `a_digest_cut_by_the_budget_withdraws_its_alias` | available | sc-tool-call-line-carries-no-outcome |
| F17 forged alias marker in output | U+00AB/U+00BB text in `an_error_digest_shows_its_first_line_and_a_forged_marker_targets_nothing` | available | sc-tool-call-line-carries-no-outcome |
| F18 symlink, FIFO, sparse oversize, swap | `project_docs.rs` test fixtures; non-UTF-8 by golden-style write; parent-directory swap has no fixture | available (unix) except the swap | sc-project-docs-canonical-hash-over-fixed-list |
| F20 drop with each `OutputKind` | the eight cases in `reduced_tool_result_keeps_failure_variant_and_output_extras`; frozen id in `frozen_arc_blocks_never_re_emitted` | available | sc-reduction-drops-body-keeps-failure-variant |
| F23 stop between Kernel commit and memory-store delete | `#[cfg(test)]` `SourceCapture::stop_after_retirement` returning `injected_stop` | available, in-process only (no process kill) | sc-session-delete-leaves-kernel-descriptors (inverting witness) |

Coverage witness cited: `tool_output_contains_sentinel` (test functions in
`crates/daemon/src/history_summarizer_chunk.rs` and the doc comment on
`crates/daemon/src/boundary.rs::the_boundary_chunk_renders_digests_for_answered_calls`).
No new fault ids are needed.

## Notes for the lead

- `sc-tool-call-line-carries-no-outcome`: U6 replaced the baseline behavior.
  The line now carries status, byte count, and `head`/`tail` excerpts, so the
  slug name and the baseline guarantee ("no status, exit code, or error text")
  disagree with HEAD. I kept `Status: active` per the brief, with the guarantee
  restated as U6's test comment does ("no output byte outside the two
  excerpts"). The lead may prefer `invalidated` pointing at
  `sc-digest-bounded-deterministic-char-boundary`. Both production sites pass
  `exit_code: None`, so `exit=` never appears in the summarizer view; R9 and the
  digest docs mention an optional exit code.
- `sc-reduction-drops-body-keeps-failure-variant`: the served placeholder is
  `[dropped §N§]` under a tag overlay; the test named
  `reduced_block_renders_placeholder_without_tag_prefix` asserts the tagged
  form, so its name contradicts its assertion. AE2's "resolves to exact error
  bytes after the window is reduced" is covered by U2's
  `an_opencode_window_publishes_its_native_sources_once_and_replays_after_memo_loss`
  reading the artifact, but that test does not reduce the window; the two
  halves are tested separately.
- `sc-project-docs-canonical-hash-over-fixed-list`: contract-versus-code. The
  doc lines in `crates/daemon/src/history_segment_coverage.rs` say `docs_hash`
  is persisted; no code reads or persists it. The `project_docs.rs` module
  comment says the path-swap window is narrowed, not eliminated, while the
  `read_document` doc says a swap after the open cannot change what is read;
  both hold (final-component `NOFOLLOW` plus descriptor checks; parent-directory
  swaps remain), and the two comments read as conflicting.
- `sc-ready-snapshot-drops-native-messages`: release builds enforce nothing in
  `finish_ready`; route to defensive-assertion review.
- `sc-session-delete-leaves-kernel-descriptors`: invalidated by U8. The
  retirement runs whenever `KernelOpenCoordinator::expected` holds (default `true`),
  independent of `source_capture.enabled`, so the successor record's
  `default-production` label is consistent with the code.
- Moved citations: baseline `transform.rs:17077, 22357, 24934, 25748`,
  `lib.rs:23483, 2135, 9825-9835, 6787-6848`, `history_summarizer_chunk.rs:1637,
  1703`, `selection.rs:529, 659-690`, and `config.rs:682-712` all moved; the
  records cite names. Baseline "gate tests from `lib.rs:24641`" pointed inside
  an unrelated test; the session-delete gate tests are named above.
- Unresolved: whether any codec puts native output bytes in `provider_extras`,
  which `reduced_block` copies verbatim.
