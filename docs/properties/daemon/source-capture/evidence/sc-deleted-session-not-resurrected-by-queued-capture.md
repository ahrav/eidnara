# sc-deleted-session-not-resurrected-by-queued-capture

## Discovery trigger

Part B record on #1066 (comment 2), decision D11 and coordination point
C5: a capture handoff accepted before `session.delete` could still
publish after the delete committed, so a deleted conversation would come
back as live evidence. U2 built the generation ticket (the delete epoch)
and U8 (#1087) bound it to the delete and named this record.

## Evidence trail

`crates/daemon/src/source_capture.rs`:

- `SourceCapture::delete_epoch` (an `AtomicU64`) is bumped by every
  `session.delete` whose memory-store delete succeeds, under the capture
  commit gate; a delete that ends `kernel_unavailable`,
  `source_retirement_failed`, or `store_write_failed` returns before the
  bump.
- `SourceCapture::epoch` is sampled into `Offer::epoch` when the pass
  builds its offer (`lib.rs`, `epoch: self.source_capture.epoch()`),
  before the response and the handoff.
- `SourceCapture::invalidate_sessions` bumps the epoch and empties the
  memo. The caller holds the gate.
- `CaptureJob::publish` takes `commit_gate`, then `publish_locked` checks
  `delete_epoch != self.epoch` first and ends `SessionInvalidated` with
  no Kernel write, before `ensure_scope`, `publish_screened`, or
  `capture_documents`.
- The `#[cfg(test)]` hook `before_publish` runs before the gate is taken.

`crates/daemon/src/lib.rs`: `handle_session_delete_value` calls
`source_capture.invalidate_sessions()` inside the gate, after
`store.delete_session` and `capture_memo.remove_session`.

## Failure scenario

The supervisor publishes outside `capture_commit_gate`, or without
rechecking the epoch under it, so a job admitted before the delete
commits descriptors for the session after the delete. Or a job that
already held the gate when the delete arrived leaves descriptors that
the delete then misses.

## Timing windows and dependencies

- Job parked before the gate when the delete runs: the delete completes,
  bumps the epoch, and the job ends `SessionInvalidated` once it takes
  the gate.
- Job holding the gate when the delete arrives: the delete waits for the
  gate, and its inventory then retires what the job published.
- The epoch is global. A delete invalidates every queued handoff of
  every session whose pass sampled an older epoch, and empties the whole
  memo. The module doc states that the next pass that carries a dropped
  window captures it again, so other sessions lose no units, only a pass.
- A pass that samples the epoch after the delete is new work. A harness
  session that keeps running after the delete republishes on its next
  pass (OQ4).

## What a test must construct

A capture job parked before the gate (the `before_publish` hook), a
`session.delete` that completes while it is parked, then the job
released. Read the `session_invalidated` counter, the Kernel tip, and
live descriptors for the session.

## Investigation log

### Q: Post-delete passes for the same harness session (OQ4)
- Sources examined: `ticket-1087.md` design decisions; the
  `handle_session_delete_value` doc comment; PR #1101 "Route outcomes".
- Findings: U8 resolved OQ4 within approved scope. There is no retirement
  on window shrink or Pi compaction, no native-removal daemon operation,
  and no tombstone. The route doc states that a harness session that
  keeps running after the delete republishes on its next pass: stored
  receipts answer for its earlier units, and new messages publish fresh
  descriptors. `docs/source-capture.md` is meant to carry the same
  statement; it is not at HEAD (an untracked draft is in the worktree,
  not read here) and is a U11 deliverable.
- Missing evidence: the product owner's tombstone decision.
- Conclusion: resolved with answer for the queued-capture case (U8). The
  tombstone that would refuse later captures needs human input.

### Q: Does invalidating every session's handoffs cost any units?
- Sources examined: `invalidate_sessions`; module doc; the memo design.
- Findings: a dropped window retains nothing, and the memo reset makes
  the next pass ask the Kernel. A window refused this way and never
  offered again is lost to capture, the same condition the module doc
  states for a full slot.
- Missing evidence: none.
- Conclusion: resolved with answer.
