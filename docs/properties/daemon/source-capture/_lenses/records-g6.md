# Records, group g6

Lens: session deletion, tool re-run generations, repository documents,
and capture resource bounds. Authored at HEAD `5a842d998` (U1 to U10
stacked). Record fields stay one line each to match the sibling catalogs.

### sc-rerun-exactly-one-live-generation

Type: safety
Reachability: explicit-config-only - only `CaptureJob::publish_locked` calls `publish_screened` for tool units, and capture runs only when `source_capture::consents` holds, which requires `DaemonConfig::source_capture_enabled` (user tier, default `false` in `config.rs`) and `memory_enabled`
Status: active
Exercised: yes - `crates/daemon/tests/harness_sources.rs::a_tool_rerun_leaves_exactly_one_live_generation` publishes Pi re-runs in order (error to success with fewer blocks), a distinct call with equal text, a redelivered 1 and duplicate 2, a stale 1 with changed bytes, reversed 2 then never-published 1, published 1 beside new 2 in one window, and two generations in one batch, and reads live generations per call and `created_commit_seq` against the predecessor's retirement; `crates/daemon/src/source_capture_tests.rs::a_rerun_keeps_one_live_generation_across_a_restart` drives an OpenCode call re-run in place through the capture path with the memo emptied between generations (`replay_after_memo_loss`, `reversed_generation_delivery`); `crates/daemon/src/harness_sources.rs::tests::a_rerun_retirement_rechecks_the_call_inside_its_commit` constructs a newer live generation and a missing predecessor inside the commit; `crates/kernel/tests/kernel_source_descriptors.rs::a_writer_reads_the_live_descriptors_of_some_identity_values` covers the narrowed in-transaction read
Guarantee: After every commit made through `publish_screened` under the capture commit gate, each call `(project_id, harness, session_id, parent_message_id, tool_call_id)` has at most one live `raw_tool_spans` generation, and the predecessor's retirement commits in the successor's commit.
Check: `always` - over every commit, live descriptors per call are at most one, and each retired predecessor's `invalidated_commit_seq` equals its successor's `created_commit_seq`; the property is a state invariant evaluated at every commit
Fault/timing angle: reversed and duplicate delivery (`2,1` and `1,2,1`); restart with an empty memo between the two results; a newer generation appearing between the plan preview and the commit
Required faults and enabling state: two results for one `tool_call_id` with different `result_revision` (`state.time.end` or Pi `timestamp`); F3 memo loss; F36 reordered delivery
Confidence: high - [evidence](../evidence/sc-rerun-exactly-one-live-generation.md). Read `publish_screened`, `plan_reruns`, `live_generations`, `predecessors`, `Reruns::intent`, `Reruns::apply`, `retire_descriptors`, and the commit-gate acquisition in `CaptureJob::publish`; read all three daemon tests at HEAD; confirmed no other production caller publishes `raw_tool_spans`
Existing check: `crates/daemon/tests/harness_sources.rs::a_tool_rerun_leaves_exactly_one_live_generation` (unaudited); `crates/daemon/src/source_capture_tests.rs::a_rerun_keeps_one_live_generation_across_a_restart` (unaudited); `crates/daemon/src/harness_sources.rs::tests::a_rerun_retirement_rechecks_the_call_inside_its_commit` (unaudited); `crates/kernel/tests/kernel_source_descriptors.rs::a_writer_reads_the_live_descriptors_of_some_identity_values` (unaudited); `crates/daemon/tests/harness_sources.rs::rerun_lookup_cost_at_ten_thousand_live_tool_descriptors` (ignored measurement, unaudited)
Impact: two live results for one call reach search and recall, or zero live results after reversed delivery
Open questions:
- Source and cost of the predecessor lookup (OQ11): resolved in U7 with the filtered inventory narrowed on `tool_call_id`, no Kernel index; PR #1100 records a 245 ms median at 10 000 live descriptors on a quiet host against the 500 ms criterion
- Does a re-run keep its `callID` (OQ3): resolved for this record in U7; a new `callID` is another call and a kept `callID` with a new `state.time.end` is a newer generation, and both readings are handled
- `SourcePublisher::publish` and `publish_batch` publish tool units without reconciliation, and a store written before U7 converges only on redelivery of the newest generation; no production caller uses the unreconciled path

### sc-delete-retires-before-memory-store-delete

Type: safety
Reachability: default-production - `handle_session_delete_value` calls `retire_captured_sources` on every delete while `KernelOpenCoordinator::expected()` is true, which `lib.rs` sets for `StorageBackend::Sqlite`; the inventory and the fail-closed refusal run whether or not capture is enabled, and a non-empty retirement needs captures, which are `explicit-config-only`
Status: active
Exercised: yes - `crates/daemon/src/source_capture_tests.rs::session_delete_retires_captured_sources_and_a_retry_recovers_a_stop_between_stores` captures a session and a second session of the project and asserts no live descriptor names the session at the F23 cut, the other session untouched, and the retired rows readable; `session_delete_retires_only_its_own_captures_and_recovers_a_stop_between_pages` keeps the same session id under another project and under Pi; `session_delete_with_the_kernel_starting_deletes_nothing` asserts `kernel_unavailable` with the memory rows kept; `session_retirement_pages_are_keyed_by_session_and_ids` pins page keys
Guarantee: The memory-store delete of a session runs only after Kernel commits have retired every live `messages` and `raw_tool_spans` descriptor of the bound project, harness, and session; any inventory, commit, or availability failure returns before the memory-store delete and leaves the session undeleted.
Check: `always` - at the memory-store delete, zero live `messages` or `raw_tool_spans` descriptors name the bound `(project_id, harness, session_id)`; an ordering invariant checked at a fixed code point
Fault/timing angle: a capture committing between inventory and retirement, excluded by `capture_commit_gate`; the expiry sweep retiring listed rows between a page's preview and its commit; a Kernel in `Starting`
Required faults and enabling state: a session with captures; F6 Kernel `Starting` or unavailable during the delete; a second session and a second harness under the same session id
Confidence: high - [evidence](../evidence/sc-delete-retires-before-memory-store-delete.md). Read `handle_session_delete_value`, `retire_captured_sources`, `retire_session_pages`, `session_page`, `session_page_intent`, and `retire_descriptors`; read the four tests; confirmed the ordering inside one gate-held blocking closure
Existing check: `crates/daemon/src/source_capture_tests.rs::session_delete_retires_captured_sources_and_a_retry_recovers_a_stop_between_stores` (unaudited); `session_delete_retires_only_its_own_captures_and_recovers_a_stop_between_pages` (unaudited); `session_delete_with_the_kernel_starting_deletes_nothing` (unaudited); `session_retirement_pages_are_keyed_by_session_and_ids` (unaudited); `a_route_claiming_another_harness_captures_nothing` (unaudited)
Impact: a deleted conversation's bytes stay live and queryable
Open questions:
- Kernel unavailable during delete (OQ12): resolved in U8, fail closed, for every session of a SQLite deployment including sessions with no captures
- No component retries a delete that answered `kernel_unavailable`; PR #1101 records that the OpenCode plugin sends one `deleteSession` and logs a failure (needs human input)
- Gate hold time grows with the session's captured descriptors and is not measured

### sc-delete-retry-recovers-crash-between-stores

Type: liveness
Reachability: default-production - the retry path is the same route; the window holds state only when the session has captures, which are `explicit-config-only`
Status: active
Exercised: partial - `crates/daemon/src/source_capture_tests.rs::session_delete_retires_captured_sources_and_a_retry_recovers_a_stop_between_stores` sets `stop_after_retirement`, gets `injected_stop`, then one retry answers `ok` with `deleted_rows > 0` and an unchanged Kernel tip; `session_delete_retires_only_its_own_captures_and_recovers_a_stop_between_pages` stops after one page of two rows and one delete completes; the cut runs in one process, so process restart and power loss are not exercised
Guarantee: After a stop between the Kernel retirement commit and the memory-store delete, one retry of `session.delete` with a ready Kernel and an open memory store answers `ok`, commits nothing new in the Kernel, and completes the memory-store delete.
Check: `always` - after exactly one retry call, the answer is `ok`, the Kernel tip is unchanged, zero live descriptors name the session, and the memory rows are gone; the bound is one call, the unit the route bounds
Fault/timing angle: the window between the two store commits; a stop between retirement pages
Required faults and enabling state: F23 cut after the Kernel commit; a captured session
Confidence: medium - [evidence](../evidence/sc-delete-retry-recovers-crash-between-stores.md). Read the F23 branch, `retire_session_pages`, `session_page_intent`, and both tests; the cut is in-process, and the memory-store side is read only through the retry's `deleted_rows`
Existing check: `crates/daemon/src/source_capture_tests.rs::session_delete_retires_captured_sources_and_a_retry_recovers_a_stop_between_stores` (unaudited); `session_delete_retires_only_its_own_captures_and_recovers_a_stop_between_pages` (unaudited); `session_retirement_pages_are_keyed_by_session_and_ids` (unaudited)
Impact: a session that can neither be deleted nor recovered
Open questions:
- A process-restart variant of F23 is not landed
- Who issues the retry: the route names the caller's next `session.delete`, and no caller retries today (needs human input)

### sc-deleted-session-not-resurrected-by-queued-capture

Type: safety
Reachability: explicit-config-only - a capture handoff exists only when `source_capture::consents` holds (`source_capture_enabled`, default `false`)
Status: active
Exercised: yes - `crates/daemon/src/source_capture_tests.rs::a_delete_before_the_first_publication_drops_the_pending_capture` parks the job in `before_publish` before it takes the gate, completes `session.delete`, releases the job, and asserts `session_invalidated == 1`, an unchanged tip, and no live descriptor for the session; `a_delete_after_the_handoff_publishes_nothing_for_the_queued_window` (U2) bumps the epoch under the gate and shows the next pass captures the window again
Guarantee: A capture whose pass sampled the delete epoch before a `session.delete` commits nothing once that delete has run, because the job rechecks the epoch under `capture_commit_gate` before any Kernel write.
Check: `always(!X)` - X is a descriptor created after a delete by work whose pass began before it; the state has no detection point of its own, so the check is a forbidden-state invariant, not `unreachable`
Fault/timing angle: handoff accepted, delete runs, job runs; a job already holding the gate when the delete arrives (the delete waits, then retires what it published)
Required faults and enabling state: F37 a capture job parked before the gate while the delete runs
Confidence: high - [evidence](../evidence/sc-deleted-session-not-resurrected-by-queued-capture.md). Read `SourceCapture::epoch`, `invalidate_sessions`, the epoch sample in `lib.rs`, `CaptureJob::publish`, and `publish_locked`'s first check; read both tests
Existing check: `crates/daemon/src/source_capture_tests.rs::a_delete_before_the_first_publication_drops_the_pending_capture` (unaudited); `a_delete_after_the_handoff_publishes_nothing_for_the_queued_window` (unaudited)
Impact: a deleted session reappears as live evidence
Open questions:
- Post-delete passes for the same harness session (OQ4): resolved within scope in U8; a harness session that keeps running republishes on its next pass, stated in the route doc; the tombstone that would refuse later captures is the product owner's decision (needs human input)
- The epoch is global: a delete drops every session's queued handoff and empties the memo; the next pass recaptures

### sc-repodoc-one-live-revision-above-historical-head

Type: safety
Reachability: explicit-config-only - the only producer is `CaptureJob::capture_documents`, reached when `consents` holds and the route's `inject_docs` is set; the Kernel guard applies to any `repository_documents` publication
Status: active
Exercised: yes - `crates/kernel/tests/kernel_source_descriptors.rs::repository_document_revisions_exceed_the_historical_head_including_retired_rows` publishes 1, 2, 3, retires 3, refuses 2, 3, 0, and a stale-head 4, checks domain precedence, reads the head, publishes 4, checks a second path, and runs a two-producer race (`retired_lineage_republished`); `crates/daemon/tests/harness_sources.rs::repository_documents_publish_untrusted_supersede_and_project` refuses 0 and 5 after retirement and republishes at head plus one; `crates/daemon/src/source_capture_tests.rs::a_configured_document_is_one_lineage_whose_edits_supersede_and_removal_retires` drives revisions 1 to 5 through capture with two removals
Guarantee: Each `(project_id, repository_id, relative_path)` lineage has at most one live descriptor, and every published revision is at least 1 and strictly above the highest revision the lineage ever held, retired rows included, with the caller's expected head equal to that head.
Check: `always` - inside the publication transaction and over every store state; a state invariant
Fault/timing angle: delete then recreate; two producers reading one head
Required faults and enabling state: F10 a lineage retired at revision 3, then a publication at 2; F5 two producers proposing the same next revision
Confidence: high - [evidence](../evidence/sc-repodoc-one-live-revision-above-historical-head.md). Read `publish_verified`'s guard, `historical_head`, both `source_lineage_head` readers, `Prepared::expected_head`, `plan_document`, and the three tests; confirmed document evidence is `canonical` retention, so expiry leaves the rows the head reads
Existing check: `crates/kernel/tests/kernel_source_descriptors.rs::repository_document_revisions_exceed_the_historical_head_including_retired_rows` (unaudited); `crates/daemon/tests/harness_sources.rs::repository_documents_publish_untrusted_supersede_and_project` (unaudited); `crates/daemon/src/source_capture_tests.rs::a_configured_document_is_one_lineage_whose_edits_supersede_and_removal_retires` (unaudited); `a_document_is_judged_against_its_own_project_among_many` (unaudited); `crates/kernel/tests/kernel_source_descriptors.rs::retirement_preserves_domain_ownership_without_reusing_old_revision_ids` (baseline for messages, unaudited)
Impact: a stale capture revives a retired identity at a lower revision
Open questions:
- Scope confirmation (OQ1): the class landed in U9 and U10 on a ticket with no blocker; no recorded product-owner confirmation was found (needs human input)
- Guard scope across classes (OQ13): resolved in U9, `RepositoryDocuments` only
- The Kernel permits a gap above the head; contiguity is the producer's rule in `Prepared::expected_head`

### sc-repodoc-raw-bytes-separate-from-prompt-hash

Type: safety
Reachability: explicit-config-only - same producer and gates as the record above
Status: active
Exercised: yes - `crates/daemon/src/source_capture_tests.rs::a_configured_document_is_one_lineage_whose_edits_supersede_and_removal_retires` reads the artifact bytes after the first publication and after a whitespace-only edit, compares `read_project_docs_canonical(...).canonical_hash` before and after that edit, and shows an unchanged file commits nothing and a reuse pass captures nothing; `document_capture_leaves_the_response_and_respects_inject_docs` shows equal response bytes with capture on and off; `crates/daemon/src/project_docs.rs` goldens pin the canonical hash
Guarantee: A captured document's artifact bytes equal the raw bytes the pass read for its prompt; a whitespace-only edit publishes a new revision while the canonical prompt hash is unchanged; an unchanged file publishes nothing, because the skip compares the live artifact digest and never the canonical hash.
Check: `always` - for every document publication the artifact equals the `DocumentRead::Text` buffer, and every skip is an equality of `payload_id(text)` with the live row's `artifact_digest`
Fault/timing angle: a swap or growth after open, closed by the bounded no-follow read; a CAS-retry attempt leaving stale reads, closed by emptying the slot per attempt
Required faults and enabling state: an edit touching only whitespace
Confidence: high - [evidence](../evidence/sc-repodoc-raw-bytes-separate-from-prompt-hash.md). Read `read_document`, `read_project_docs_canonical`, `leave_reads`, `clear_reads`, `SourceCapture::offer`, and `plan_document`; read the tests; the CAS-retry clear is taken from PR #1103 and not re-read at both call sites
Existing check: `crates/daemon/src/source_capture_tests.rs::a_configured_document_is_one_lineage_whose_edits_supersede_and_removal_retires` (unaudited); `document_capture_leaves_the_response_and_respects_inject_docs` (unaudited); `crates/daemon/src/project_docs.rs::tests::project_docs_golden_matches_reference` (unaudited)
Impact: evidence of what the agent read that cannot reproduce the file
Open questions:
- The test compares `read_project_docs_canonical` directly, not the pass's stored `docs_hash`

### sc-repodoc-only-verified-missing-retires

Type: safety
Reachability: explicit-config-only - same producer and gates as the records above
Status: active
Exercised: yes - `crates/daemon/src/source_capture_tests.rs::unreadable_documents_are_counted_and_retire_nothing` arranges a symlink, 70 KiB text, 300 KiB text, non-UTF-8 bytes, and a mode-0 file (skipped as root) and asserts each count rises and the live row is unchanged; `a_renamed_document_retires_the_old_path_and_publishes_the_new`; `a_configured_document_is_one_lineage_whose_edits_supersede_and_removal_retires` covers removal and a missing file with no live row; `crates/daemon/src/project_docs.rs::tests::a_file_is_missing_only_from_a_directory_that_exists` makes a vanished root `Unreadable`
Guarantee: A `repository_documents` descriptor is retired only by a `DocumentRead::Missing` read of a file whose parent directory exists; `NotRegular`, `Oversize`, `NotUtf8`, `Unreadable`, and text above 64 KiB retire nothing; a rename retires the old path and publishes the new one.
Check: `always` - every retirement of a `repository_documents` descriptor is attributed to a `Missing` read through `retire_document`
Fault/timing angle: a transient permission error; an unmounted root; an editor's unlink-then-write save, which reads as a verified removal and republishes above the head on the next pass
Required faults and enabling state: F18 symlink, growth past a cap, non-UTF-8, permission error; F38 a missing parent directory
Confidence: high - [evidence](../evidence/sc-repodoc-only-verified-missing-retires.md). Read `read_document`, `plan_document`, `retire_document`, and the other retirement paths; found no other production writer that retires the class
Existing check: `crates/daemon/src/source_capture_tests.rs::unreadable_documents_are_counted_and_retire_nothing` (unaudited); `a_renamed_document_retires_the_old_path_and_publishes_the_new` (unaudited); `crates/daemon/src/project_docs.rs::tests::a_file_is_missing_only_from_a_directory_that_exists` (unaudited); `symlinked_doc_is_skipped`, `a_doc_that_outgrows_the_cap_under_the_read_is_skipped`, `oversized_doc_is_skipped` (unaudited)
Impact: a transient read failure erases the revision trail
Open questions:
- A document past a cap keeps a live revision that no longer matches the file (needs human input)

### sc-capture-resource-bounds

Type: safety
Reachability: explicit-config-only - the unit, batch, window, memo, and document bounds sit on the capture path behind `source_capture_enabled` (default `false`); the Kernel payload and text-field caps and the digest bound are `default-production` for every artifact ingest and every summarizer chunk, and this record claims them only as the outer limits capture sits under
Status: active
Exercised: partial - `crates/daemon/tests/harness_sources.rs::capture_batch_refuses_an_oversized_unit_whole_and_publishes_its_siblings` publishes a 65 536-byte unit with equal artifact bytes and refuses a 65 537-byte unit with no evidence and no receipt while both siblings publish, then refuses it again; `capture_batch_refusals_take_no_admission_slot` and `capture_batch_defers_units_beyond_its_bounds` cover 32 units and 64 KiB; `crates/daemon/src/source_capture_tests.rs::a_33_unit_window_publishes_32_and_the_next_pass_publishes_the_deferred_one`, `the_capture_report_carries_no_unit_text` (`refused_window_too_large`), `a_window_past_the_remaining_budget_is_refused_resident_short`, `a_documents_text_is_charged_with_its_window`, and `unreadable_documents_are_counted_and_retire_nothing` (70 KiB document); `crates/daemon/src/source_capture.rs::tests::the_memo_stays_within_its_byte_budget_and_keeps_the_first_deferral`; `crates/daemon/tests/harness_sources.rs::an_invalid_outcome_record_is_refused_before_retention` (outcome above 1 KiB); `crates/daemon/src/tool_outcome.rs::tests::every_digest_is_bounded_and_shows_only_its_outer_lines` and `a_digest_clips_on_character_boundaries_within_its_bounds`; `crates/kernel/tests/kernel_cas.rs::payload_limit_is_inclusive_at_the_artifact_cap` and `oversized_text_fields_are_rejected_before_staging`. Not exercised: a `raw_tool_spans` unit above 64 KiB through the capture path, and a window holding a very large tool output beside small siblings
Guarantee: Capture retains a unit only when its text is at most `MAX_SOURCE_UNIT_BYTES` (65 536 bytes) and then retains it whole; a larger unit is `unit_too_large` with nothing retained and no batch slot taken; a batch admits at most 32 units and 64 KiB of text and defers the rest; the supervisor holds at most `MAX_RETAINED_WINDOW_BYTES` (32 MiB) of charged window bytes plus `MEMO_BUDGET_BYTES` (4 MiB) of memo; every outcome record and every summarizer digest is at most 1 KiB; all of these sit under the Kernel's 64 MiB payload and 1 KiB text-field caps.
Check: `always` - every retained capture artifact equals its unit text and is at most 65 536 bytes, every committed batch holds at most 32 units and 65 536 text bytes, the supervisor's `retained_bytes` never exceeds 32 MiB, and every stored outcome record and every built digest encodes to at most 1024 bytes
Fault/timing angle: a window whose charge exceeds the retained budget is refused before the unit screen, so a tool output above about 16 MiB refuses its whole window on every pass that carries it
Required faults and enabling state: F31 a unit above 64 KiB in a batch with siblings, and a window above the retained budget; a 33-unit window; an outcome record above 1 KiB
Confidence: medium - [evidence](../evidence/sc-capture-resource-bounds.md). Read every constant and the code that applies it at HEAD and every test named above; medium because the tool-output case is covered only through message units on a class-agnostic branch and the window-level refusal of siblings is untested
Existing check: `crates/daemon/tests/harness_sources.rs::capture_batch_refuses_an_oversized_unit_whole_and_publishes_its_siblings` (unaudited); `capture_batch_defers_units_beyond_its_bounds` (unaudited); `capture_batch_refusals_take_no_admission_slot` (unaudited); `an_invalid_outcome_record_is_refused_before_retention` (unaudited); `crates/daemon/src/source_capture_tests.rs::a_33_unit_window_publishes_32_and_the_next_pass_publishes_the_deferred_one` (unaudited); `the_capture_report_carries_no_unit_text` (unaudited); `a_window_past_the_remaining_budget_is_refused_resident_short` (unaudited); `a_documents_text_is_charged_with_its_window` (unaudited); `crates/daemon/src/source_capture.rs::tests::the_memo_stays_within_its_byte_budget_and_keeps_the_first_deferral` (unaudited); `crates/daemon/src/tool_outcome.rs::tests::every_digest_is_bounded_and_shows_only_its_outer_lines` (unaudited); `crates/kernel/tests/kernel_cas.rs::payload_limit_is_inclusive_at_the_artifact_cap` (unaudited); `oversized_text_fields_are_rejected_before_staging` (unaudited); `crates/kernel/tests/kernel_source_descriptors.rs::a_tool_descriptor_stores_a_validated_outcome_record` (unaudited)
Impact: a truncated tool output presented as an exact source, unbounded resident memory from large windows, or one oversized unit silencing its window
Open questions:
- A test with a `raw_tool_spans` unit above 64 KiB through `CaptureJob` is not landed
- A tool output above about 16 MiB refuses its whole window, so R10's sibling rule holds only below the retained bound (needs human input)
- The host wire frame bound on the transform request was not located

## Existing checks

All `unaudited`.

| Check | Covers |
| --- | --- |
| `crates/daemon/tests/harness_sources.rs::a_tool_rerun_leaves_exactly_one_live_generation` | Pi re-run orderings, coexisting distinct calls, retirement commit sequence |
| `crates/daemon/src/source_capture_tests.rs::a_rerun_keeps_one_live_generation_across_a_restart` | OpenCode re-run through capture with memo loss; superseded counter |
| `crates/daemon/src/harness_sources.rs::tests::a_rerun_retirement_rechecks_the_call_inside_its_commit` | in-commit `Conflict` on a newer live generation or a changed predecessor set |
| `crates/kernel/tests/kernel_source_descriptors.rs::a_writer_reads_the_live_descriptors_of_some_identity_values` | narrowed in-transaction read, paging, own retirements |
| `crates/daemon/tests/harness_sources.rs::rerun_lookup_cost_at_ten_thousand_live_tool_descriptors` | ignored OQ11 measurement |
| `crates/daemon/src/source_capture_tests.rs::session_delete_retires_captured_sources_and_a_retry_recovers_a_stop_between_stores` | AC5, AC15, F23 cut and one retry |
| `crates/daemon/src/source_capture_tests.rs::session_delete_retires_only_its_own_captures_and_recovers_a_stop_between_pages` | scope filter, stop between pages |
| `crates/daemon/src/source_capture_tests.rs::session_delete_with_the_kernel_starting_deletes_nothing` | OQ12 fail closed |
| `crates/daemon/src/source_capture_tests.rs::session_retirement_pages_are_keyed_by_session_and_ids` | page key determinism and replay |
| `crates/daemon/src/source_capture_tests.rs::a_route_claiming_another_harness_captures_nothing` | stamped harness equals the route harness |
| `crates/daemon/src/source_capture_tests.rs::a_delete_before_the_first_publication_drops_the_pending_capture` | parked job after a delete commits nothing |
| `crates/daemon/src/source_capture_tests.rs::a_delete_after_the_handoff_publishes_nothing_for_the_queued_window` | epoch bump under the gate; next pass recaptures |
| `crates/kernel/tests/kernel_source_descriptors.rs::repository_document_revisions_exceed_the_historical_head_including_retired_rows` | F10, expected head, two-producer race |
| `crates/kernel/tests/kernel_source_descriptors.rs::retirement_preserves_domain_ownership_without_reusing_old_revision_ids` | baseline live-predecessor rule for messages |
| `crates/daemon/tests/harness_sources.rs::repository_documents_publish_untrusted_supersede_and_project` | admission pair, revision refusals, projection |
| `crates/daemon/src/source_capture_tests.rs::a_configured_document_is_one_lineage_whose_edits_supersede_and_removal_retires` | AC4, AC17, raw bytes, whitespace edit, removal and recreation |
| `crates/daemon/src/source_capture_tests.rs::a_renamed_document_retires_the_old_path_and_publishes_the_new` | rename |
| `crates/daemon/src/source_capture_tests.rs::a_document_is_judged_against_its_own_project_among_many` | per-project live lookup |
| `crates/daemon/src/source_capture_tests.rs::unreadable_documents_are_counted_and_retire_nothing` | F18 dispositions; 64 KiB unit cap on documents |
| `crates/daemon/src/source_capture_tests.rs::document_capture_leaves_the_response_and_respects_inject_docs` | response independence; `inject_docs` gate |
| `crates/daemon/src/source_capture_tests.rs::a_documents_text_is_charged_with_its_window` | document bytes charged twice |
| `crates/daemon/src/project_docs.rs::tests::a_file_is_missing_only_from_a_directory_that_exists` | verified-missing rule |
| `crates/daemon/src/project_docs.rs::tests::symlinked_doc_is_skipped`, `a_doc_that_outgrows_the_cap_under_the_read_is_skipped`, `oversized_doc_is_skipped`, `project_docs_golden_matches_reference` | read classification and canonical goldens |
| `crates/daemon/tests/harness_sources.rs::capture_batch_refuses_an_oversized_unit_whole_and_publishes_its_siblings` | 64 KiB inclusive cap, whole refusal, terminal |
| `crates/daemon/tests/harness_sources.rs::capture_batch_defers_units_beyond_its_bounds` | 32 units, 64 KiB aggregate |
| `crates/daemon/tests/harness_sources.rs::capture_batch_refusals_take_no_admission_slot` | refusals take no slot |
| `crates/daemon/src/source_capture_tests.rs::a_33_unit_window_publishes_32_and_the_next_pass_publishes_the_deferred_one` | batch bound through capture |
| `crates/daemon/src/source_capture_tests.rs::the_capture_report_carries_no_unit_text` | `refused_window_too_large` |
| `crates/daemon/src/source_capture_tests.rs::a_window_past_the_remaining_budget_is_refused_resident_short` | retained budget refusal and release |
| `crates/daemon/src/source_capture.rs::tests::the_memo_stays_within_its_byte_budget_and_keeps_the_first_deferral` | 4 MiB memo bound |
| `crates/daemon/tests/harness_sources.rs::an_invalid_outcome_record_is_refused_before_retention` | outcome above 1 KiB refused before retention |
| `crates/kernel/tests/kernel_source_descriptors.rs::a_tool_descriptor_stores_a_validated_outcome_record` | outcome record validation in the commit |
| `crates/daemon/src/tool_outcome.rs::tests::every_digest_is_bounded_and_shows_only_its_outer_lines`, `a_digest_clips_on_character_boundaries_within_its_bounds` | 1 KiB digest, 128-byte excerpts |
| `crates/kernel/tests/kernel_cas.rs::payload_limit_is_inclusive_at_the_artifact_cap` | 64 MiB payload cap |
| `crates/kernel/tests/kernel_cas.rs::oversized_text_fields_are_rejected_before_staging` | 1 KiB text-field cap |

None found: a `raw_tool_spans` unit above 64 KiB through the capture path;
a window holding a large tool output beside small siblings; a
process-restart variant of F23; a gate-hold measurement for large
deletes.

## Fault map rows

| fault id | injection at HEAD | availability | records |
| --- | --- | --- | --- |
| F3 restart / memo loss | `SourceCapture::forget_memo_for_test` | available | sc-rerun-exactly-one-live-generation |
| F5 concurrent duplicate publication | two producers reading one `source_lineage_head` in the Kernel test | available for `repository_documents` at the Kernel; missing elsewhere | sc-repodoc-one-live-revision-above-historical-head |
| F6 Kernel closed or `Starting` | `kernel.set_expected(true)` with the Kernel unopened | available | sc-delete-retires-before-memory-store-delete |
| F10 retired lineage then lower revision | retire revision 3, publish 2 | available | sc-repodoc-one-live-revision-above-historical-head |
| F18 symlink, oversize, non-UTF-8, permission | `unreadable_documents_are_counted_and_retire_nothing` arrangements | available (unix; permission case skipped as root) | sc-repodoc-only-verified-missing-retires |
| F23 stop between Kernel commit and memory-store delete | `SourceCapture::stop_after_retirement` (in-process); `retire_session_pages` with `pages = 1` | available in-process; process restart missing | sc-delete-retires-before-memory-store-delete; sc-delete-retry-recovers-crash-between-stores |
| F36 reordered or duplicate generation delivery (new) | `publish_screened` with windows in order 2,1 and 1,2,1 | available | sc-rerun-exactly-one-live-generation |
| F37 capture job parked before the gate across a delete (new) | `SourceCapture::before_publish` hook | available | sc-deleted-session-not-resurrected-by-queued-capture |
| F38 missing parent directory (new) | `read_document` on `dir/gone/ARCHITECTURE.md` | available | sc-repodoc-only-verified-missing-retires |
| F31 oversized unit or window (new) | `MAX_SOURCE_UNIT_BYTES + 1` unit; a window above the retained budget | available for message units and windows; missing for tool units through capture | sc-capture-resource-bounds |

New ids: F36 names the delivery orders the source record lists without an
id; F37 names the hook that replaced the source's "slow Kernel writer"
fault, which F22 (slot busy) does not cover; F38 separates a vanished
root from F18's file-level faults; F31 has no source id because the
resource record is new.

## Notes for the lead

- Contract versus code, sc-capture-resource-bounds: R10 and AC8 say an
  oversized unit is refused while its siblings publish. That holds at the
  batch screen. `SourceCapture::offer` charges the doubled window first,
  so one tool output above about 16 MiB refuses the whole window as
  `refused_window_too_large` on every pass that carries it. The module
  doc states the refused-window condition; the requirement text does not.
- Contract versus code, sc-rerun-exactly-one-live-generation: "after
  every commit" holds for commits through `publish_screened` under the
  commit gate. `publish` and `publish_batch` bypass reconciliation, and
  the in-crate test builds two live generations that way.
- sc-delete-retires-before-memory-store-delete is `default-production`:
  fail-closed `kernel_unavailable` now applies to every delete of a
  SQLite deployment with capture disabled.
- OQ1: no recorded product-owner confirmation found; U9 and U10 landed
  on a ticket with no blocker. The brief lists OQ1 as resolved in U9. I
  left it open (needs human input).
- No component retries a failed `session.delete` (PR #1101 owner item).
  I did not read the OpenCode plugin to confirm.
- Moved citations: `read_safe_canonical` (`project_docs.rs:45-53`) no
  longer exists; `read_document` replaces it. `lib.rs:6787-6847` and
  `lib.rs:6808-6826` now fall inside `retire_captured_sources` and
  `handle_session_delete_value`, which I cite by name.
- `docs/source-capture.md` is not at HEAD; an untracked draft appeared in
  the worktree while I wrote this. The U8 route doc says it will carry the
  post-delete republication statement; I did not read the draft.
- Unresolved: the host wire frame bound on the transform request; the
  CAS-retry `clear_reads` call sites (taken from PR #1103, not re-read).
