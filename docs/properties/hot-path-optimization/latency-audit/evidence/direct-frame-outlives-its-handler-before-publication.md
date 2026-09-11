# direct-frame-outlives-its-handler-before-publication

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

T3's charge clause is about bytes a serializer closure owns after the handler
that built it is gone and before the endpoint thread has committed them.
No test at HEAD queues a direct frame at all, so no test has ever had that
window open. This record asks a campaign to witness the three preconditions,
not the charge accounting.

## Evidence trail

- [`output_from_writer`][from-writer] is the only entry to
  [`reserve_direct`][reserve-direct], and its only production-tree caller is
  the [`direct_fill` fixture arm][fixture-arm]; a repository search finds no
  test that sends `"kind": "direct_fill"`.
- The handler future is spawned at [dispatch.rs:928-934][spawn] with the
  `RequestCtx` moved into `handler.handle(ctx)`. For a unary response the
  dispatcher awaits that future ([`joined`][joined]), maps
  `RequestOutcome::Response { body }` to `Terminal::Response`, and calls
  [`settle`][settle], which runs [`emit_reserved_frame`][emit-reserved] at
  [`:409-420`][emit-call]. So the handler future has returned before the
  frame is queued.
- [`emit_reserved_frame`][emit-reserved] turns `OutputParts::Direct` into a
  [`DirectFrame`][direct-frame] and queues it through
  [`send_before`][send-before], which reserves a channel permit under the
  admission lock and pushes the `OutboundFrame`.
- The endpoint thread takes frames from `queue.recv()` in the
  [`select!`][idle-select] and runs [`publish_one`][publish-one]; the direct
  arm reserves, serializes, and commits in [`publish_direct`][publish-direct].
  Commit is the point at which the closure's captures stop being needed and
  the egress charge is dropped ([`:784`][publish-one]).
- A stream item is different: [`StreamSink::send`][stream-send] runs
  `emit_reserved_frame` while the handler future is still executing, so a
  direct stream item's closure can be serialized before the handler returns.
  The transform response is unary.
- The in-crate test [`a_commit_past_the_write_deadline_is_refused`][t-deadline]
  calls `publish_direct` on a ring directly, with no handler and no queue, so
  it constructs none of the three preconditions.

## Failure scenario

Not a violation; a coverage gap. Without a queued direct frame whose handler
has returned and whose commit has not happened, T3's charge clause is
satisfied vacuously and a design that releases the source bytes' charge at
`handle` return passes the suite.

## Timing windows and dependencies

For a unary response the second precondition follows from dispatch order: the
frame is queued only after the handler future completes. The window is then
the interval between [`send_before`][send-before] pushing the frame and
[`publish_direct`][publish-direct] committing it. With an idle ring and a
free endpoint thread that interval is short, so a marker must be able to fire
inside it; a held ring reservation or a slow earlier frame ahead in the queue
lengthens it. A generation retirement or `discard` during the window drops
the `OutboundFrame` in the queue instead of committing it.

## What a test must construct

A handler that returns `output_from_writer(len, closure)` immediately, sent
through the host with the ring's egress held (an uncommitted reservation on
the host-to-peer ring or a slow frame queued first); observation of handler
completion and of `publish_one` commit as separate events, for example the
handler future's join and the `written` or publish hook. The marker records,
at queue time or from the endpoint thread before commit: a `DirectFrame` is
in the queue, the handler future has completed or been dropped, and no commit
has occurred for it. It asserts the three preconditions, not the charge. The
[ring checks](../existing-checks.md#ring-arena-and-direct-frame) contain no
queued direct frame.

The window opens whenever the frame is queued: a unary response is queued
from `settle` after the joined handler future returns (`dispatch.rs:956-997`,
the `settle` call at `:995`, the emit at `:409-420`). Held egress lengthens
the window so the marker can observe it; it does not open it.

## Investigation log

The catalog record lists no open questions. One question was checked while
tracing the window.

### Q: Can fast egress close the window before the handler future completes?

- Sources examined: The [spawn][spawn] and [`joined`][joined] arms,
  [`settle`][settle], the emit call at [`:409-420`][emit-call],
  [`send_before`][send-before], [`StreamSink::send`][stream-send].
- Findings: For a unary `Response`, the frame is queued from `settle`, which
  runs after the handler future has completed, so the closure always outlives
  the handler once queued; egress speed shortens the window but cannot close
  it before the handler returns. For a stream item sent during the handler,
  the endpoint thread can serialize before the handler returns.
- Missing evidence: None for the order; the campaign has not run.
- Conclusion: resolved with answer - the catalog's fault angle ("serializes
  before the handler future is polled to completion") describes the stream
  case, not the unary response; for the unary response the window always
  opens on queueing, and the construction problem is observability of a short
  window plus the absence of any direct sender at HEAD.

[from-writer]: ../../../../../crates/host-runtime/src/handler.rs#L465-L472
[reserve-direct]: ../../../../../crates/host-runtime/src/dispatch.rs#L517-L554
[emit-reserved]: ../../../../../crates/host-runtime/src/dispatch.rs#L282-L332
[settle]: ../../../../../crates/host-runtime/src/dispatch.rs#L365-L468
[emit-call]: ../../../../../crates/host-runtime/src/dispatch.rs#L409-L420
[stream-send]: ../../../../../crates/host-runtime/src/dispatch.rs#L556-L582
[spawn]: ../../../../../crates/host-runtime/src/dispatch.rs#L928-L934
[joined]: ../../../../../crates/host-runtime/src/dispatch.rs#L956-L997
[direct-frame]: ../../../../../crates/host-runtime/src/frame_channel.rs#L166-L200
[send-before]: ../../../../../crates/host-runtime/src/frame_channel.rs#L248-L275
[idle-select]: ../../../../../crates/host-runtime/src/ring_transport.rs#L582-L617
[publish-one]: ../../../../../crates/host-runtime/src/ring_transport.rs#L749-L786
[publish-direct]: ../../../../../crates/host-runtime/src/ring_transport.rs#L788-L800
[t-deadline]: ../../../../../crates/host-runtime/src/ring_transport.rs#L1857-L1887
[fixture-arm]: ../../../../../crates/host-runtime/tests/support/mod.rs#L441-L455
