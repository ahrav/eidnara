# sc-session-delete-leaves-kernel-descriptors

## Discovery trigger

Part A record of the source-capture catalog (#1066 comment 1), written at
baseline `08de9051a`: `session.delete` made no Kernel call. The record named
U8 as its inverter. U8 (`ticket-1087.md`, R6, D11) retires captured sources
before the memory-store delete. This file records the inversion at HEAD
`5a842d998`.

## Evidence trail

Baseline `08de9051a`, `crates/daemon/src/lib.rs::handle_session_delete_value`:
under `capture_commit_gate` it called `store.delete_session`, then purged the
memory-capture memo. It held no Kernel step.

HEAD `crates/daemon/src/lib.rs`:

- `handle_session_delete_value` takes `capture_commit_gate`, calls
  `retire_captured_sources(&binding, &session_id)?`, then
  `store.delete_session`, then `capture_memo.remove_session` and
  `source_capture.invalidate_sessions()`. Its doc comment lists the two steps
  and the outcomes `ok`, `kernel_unavailable`, `source_retirement_failed`,
  `store_write_failed`, and `store_unavailable`.
- `retire_captured_sources` returns `Ok` when `self.kernel.expected()` is
  false, answers `kernel_unavailable` when `kernel_store()` fails, and calls
  `source_capture::retire_session_pages` with `SESSION_RETIREMENT_PAGE` rows
  per page.
- A `#[cfg(test)]` `stop_after_retirement` flag returns `injected_stop` between
  the Kernel step and the memory-store delete (F23).

`crates/daemon/src/kernel_routes/mod.rs`: `KernelOpenCoordinator::expected`
defaults `true`.

`crates/daemon/src/source_capture.rs`:

- `retire_session_pages` previews a page, commits it with
  `harness_sources::retire_descriptors`, and refuses a replayed page with
  `KernelError::Conflict`.
- `session_page_intent` keys the receipt by `SESSION_RETIREMENT_FORMAT`, the
  project, harness, and session, and the page's sorted object ids, never the
  clock.
- `session_page` walks `messages` then `raw_tool_spans` with
  `live_source_descriptors_with(class, "session_id", ..)` and keeps rows whose
  `project_id` and `harness` match.

Tests that refute the baseline guarantee at HEAD
(`crates/daemon/src/source_capture_tests.rs`):
`session_delete_retires_captured_sources_and_a_retry_recovers_a_stop_between_stores`,
`session_delete_with_the_kernel_starting_deletes_nothing`,
`a_delete_before_the_first_publication_drops_the_pending_capture`,
`session_delete_retires_only_its_own_captures_and_recovers_a_stop_between_pages`,
`session_retirement_pages_are_keyed_by_session_and_ids`.

## Failure scenario

The baseline state, now forbidden: a deleted session keeps live `messages` and
`raw_tool_spans` descriptors, so its bytes stay queryable.

## Timing windows and dependencies

The retirement runs whenever the Kernel is expected, whether or not
`source_capture.enabled` is set, so every `session.delete` on a default daemon
depends on Kernel readiness (OQ12, fail closed).

## What a test must construct

Carried by `sc-delete-retires-before-memory-store-delete`,
`sc-delete-retry-recovers-crash-between-stores`, and
`sc-deleted-session-not-resurrected-by-queued-capture`.

## Investigation log

### Q: Which code inverted the record, and which record carries it now?

- Sources examined: `handle_session_delete_value` at both commits;
  `retire_captured_sources`; `retire_session_pages`; `ticket-1087.md`.
- Findings: U8 added the Kernel step ahead of the memory-store delete.
- Missing evidence: none.
- Conclusion: resolved with answer: invalidated by U8;
  `sc-delete-retires-before-memory-store-delete` carries the obligation.

### Q: OQ12. Kernel unavailable during delete?

- Sources examined: `ticket-1087.md`; `retire_captured_sources`;
  `session_delete_with_the_kernel_starting_deletes_nothing`.
- Findings: a `Starting` Kernel answers `kernel_unavailable` and leaves the
  memory rows; the retry after open answers `ok`.
- Missing evidence: none.
- Conclusion: resolved with answer in U8: fail closed.

### Q: OQ4. Post-delete passes for the same harness session?

- Sources examined: `ticket-1087.md`; the `handle_session_delete_value` doc.
- Findings: no tombstone; a session that keeps running republishes on its next
  pass, as the route doc states.
- Missing evidence: the product owner's tombstone decision.
- Conclusion: resolved within approved scope in U8; tombstone needs human input.

### Q: Do the baseline citations still hold?

- Findings: baseline `lib.rs:6787-6848` is now the handler at `:6856`, with
  `retire_captured_sources` above it. Baseline "gate tests from `:24641`"
  pointed inside `a_source_swept_during_preparation_leaves_the_rest_of_the_batch_issued`;
  the session-delete gate tests at HEAD are
  `a_cancelled_session_delete_still_purges_in_memory_state`,
  `explicit_capture_checkpoints_serialize_with_session_delete_off_the_worker`,
  and `session_delete_waits_for_the_capture_gate_off_the_runtime_worker`.
- Conclusion: resolved with answer.
