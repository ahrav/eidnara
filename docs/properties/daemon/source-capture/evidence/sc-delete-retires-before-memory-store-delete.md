# sc-delete-retires-before-memory-store-delete

## Discovery trigger

Part A record `sc-session-delete-leaves-kernel-descriptors` showed that
`session.delete` removed memory-store rows and left every captured
descriptor live. Part B turned it into this obligation (R6, D11). U8
(#1087) landed the Kernel step and named this record.

## Evidence trail

`crates/daemon/src/lib.rs`:

- `HandlerCore::handle_session_delete_value` runs the whole delete as one
  `kernel_routes::blocking` closure. Under `capture_commit_gate` it calls
  `retire_captured_sources` first, then `store.delete_session`, then
  `capture_memo.remove_session` and `source_capture.invalidate_sessions`.
  Its doc comment lists the two steps and the outcomes `ok`,
  `kernel_unavailable`, `source_retirement_failed`, `store_write_failed`,
  `store_unavailable`.
- `HandlerCore::retire_captured_sources` returns `Ok(())` when
  `kernel.expected()` is false. Otherwise a `kernel_store()` failure is
  `kernel_unavailable`; `Conflict`, `CorruptCanonicalRow`, and
  `InvalidInput` map to `source_retirement_failed`; other errors map to
  `kernel_unavailable`; a page bound that runs out is `kernel_unavailable`.
  Every error returns before the memory-store delete.

`crates/daemon/src/source_capture.rs`:

- `retire_session_pages` loops: one `store.preview` reads a page through
  `session_page`, one `store.commit` retires it with `retire_descriptors`
  under `session_page_intent`. An empty page returns `true`; a replayed
  receipt is `KernelError::Conflict`.
- `session_page` reads both `Messages` and `RawToolSpans` through
  `live_source_descriptors_with(class, "session_id", ...)` and filters
  `project_id` and `harness` in memory.
- `SESSION_RETIREMENT_PAGE` is 256.

`crates/daemon/src/kernel_routes/mod.rs`: `KernelOpenCoordinator`
starts with `expected: true`; `lib.rs` sets it from the storage backend
(`StorageBackend::Sqlite`). So the inventory runs on every delete of a
SQLite deployment, whether or not capture is enabled.

## Failure scenario

The memory-store delete commits first and the Kernel retirement second,
with a crash or error between them, so the deleted conversation's bytes
stay live and queryable. Or a capture publishes a descriptor for the
session between the inventory and the memory delete.

## Timing windows and dependencies

- Inventory to commit: each page is read in a preview and committed in a
  separate transaction. The gate excludes capture publication. The
  maintenance expiry sweep can retire listed rows first;
  `retire_descriptors` skips rows that are no longer live.
- Kernel `Starting` or unavailable: OQ12 fail closed; the memory rows
  stay.
- The guarantee is per bound harness: the same session id under another
  harness or project keeps its descriptors. `source_capture::consents`
  admits capture only when the serializer profile's harness equals the
  route's harness, so the stamped harness is the one the delete uses.
- `repository_documents` descriptors are project-scoped and are not
  retired by a session delete.

## What a test must construct

A session with captured messages and tool results, another session in
the same project, the same session id under another project and under
Pi, a Kernel in `Starting`, and a cut after the Kernel step. At the cut
and after the delete, read live descriptors filtered by
`(project_id, harness, session_id)`.

## Investigation log

### Q: Kernel unavailable during delete: fail closed or best effort (OQ12)
- Sources examined: `ticket-1087.md`; `retire_captured_sources`;
  `session_delete_with_the_kernel_starting_deletes_nothing`.
- Findings: fail closed landed. A Kernel in `Starting` answers
  `kernel_unavailable` and the memory rows stay; the test then opens the
  Kernel and the retry answers `ok` with `deleted_rows > 0`. This applies
  to every session of a SQLite deployment, including sessions with no
  captures and deployments with capture disabled.
- Missing evidence: none.
- Conclusion: resolved with answer (U8, fail closed).

### Q: Who retries a delete that answered `kernel_unavailable`?
- Sources examined: PR #1101 body "Open item for the owner"; route doc.
- Findings: the route doc names the caller's next `session.delete` as the
  recovery. The PR records that the OpenCode plugin sends one
  `deleteSession` per cleared session and logs a failure. This agent did
  not read the plugin code.
- Missing evidence: the plugin's retry behavior at HEAD, and an owner
  decision on a bounded plugin retry or a daemon-side pending delete.
- Conclusion: needs human input.

### Q: How long does the delete hold the commit gate?
- Sources examined: PR #1101 "Recorded and left as written".
- Findings: hold time grows with the session's captured descriptors; D11
  accepts it; nothing measures it.
- Missing evidence: a measurement.
- Conclusion: unresolved, needs a gate-hold measurement at a large
  session.
