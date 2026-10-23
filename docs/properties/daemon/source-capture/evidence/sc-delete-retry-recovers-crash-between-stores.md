# sc-delete-retry-recovers-crash-between-stores

## Discovery trigger

Part B record on #1066 (comment 2), decision D11 and fault F23: the
two-store delete has a window between the Kernel retirement commit and
the memory-store delete. U8 (#1087) added the failpoint and named this
record.

## Evidence trail

`crates/daemon/src/lib.rs`:

- `handle_session_delete_value`: after `retire_captured_sources`, a
  `#[cfg(test)]` branch swaps `source_capture.stop_after_retirement` and
  returns `injected_stop` before `store.delete_session`.
- The route doc: `store_write_failed` means step 1 already committed and
  a retry completes step 2; `store_unavailable` means either step may
  have committed and a retry converges.

`crates/daemon/src/source_capture.rs`:

- `retire_session_pages` re-reads the inventory on every call, so a retry
  after a completed retirement finds an empty page and returns `true`
  without a commit.
- `session_page_intent` binds `source-session-retire.v1`, the session's
  three identity fields, and the page's sorted object ids; it has no
  clock input. A replayed page is `Conflict`, so a page key never repeats
  silently.

`crates/memory-store/src/lib.rs`: `delete_session` (baseline citation
`lib.rs:8968-8976` that it tolerates absent rows; this agent confirmed
the retry's `deleted_rows > 0` through the test, not the function body).

## Failure scenario

The retry writes new receipts and retires nothing while the delete waits
on a count mismatch, for example a page key that includes `now`. Or the
retry fails because the inventory is empty or the pages replay, leaving a
session that can neither be deleted nor recovered.

## Timing windows and dependencies

- The window is between the Kernel page commit and the memory-store
  delete. F23 is an in-process cut, `stop_after_retirement`; the test doc
  states that process restart and power loss are not exercised.
- A stop between pages leaves part of the session live. The next delete
  inventories again and retires the rest.
- With no captures the Kernel step commits nothing, so the window holds
  no state.
- Liveness bound: one retry `session.delete` call, provided the Kernel is
  ready and the memory store is open.

## What a test must construct

A captured session; the F23 cut; a read of live descriptors at the cut;
one retry and its answer; the Kernel tip before and after the retry;
another session's descriptors; the retired rows' state and bytes. Also
a retirement stopped after one page (`retire_session_pages` with
`pages = 1`) followed by one delete.

## Investigation log

### Q: Does the landed cut stand in for a process kill?
- Sources examined: test doc comment of
  `session_delete_retires_captured_sources_and_a_retry_recovers_a_stop_between_stores`;
  PR #1101 review notes (crash-consistency reviewer).
- Findings: the cut returns from the handler in one process. It keeps
  the in-memory memo and epoch state. A real kill would also empty the
  memo and restart the Kernel open sequence. The retry path itself reads
  only durable state (the Kernel inventory and the memory-store rows), so
  the cut covers the decision the retry makes.
- Missing evidence: a process-restart variant.
- Conclusion: unresolved, needs a restart-between-stores test to cover a
  real crash; the in-process cut is landed.

### Q: Who issues the retry?
- Sources examined: PR #1101 "Open item for the owner".
- Findings: the route doc names the caller's next `session.delete`. The
  PR records that the OpenCode plugin does not retry a failed delete.
- Missing evidence: the plugin code at HEAD; an owner decision.
- Conclusion: needs human input.
