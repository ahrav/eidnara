# queued-write-needs-no-second-wake

## Discovery trigger

Fix commit `ad5bef49e` "prevent queued ring writes from waiting for a second
wake". Lead only; the mechanism was re-read at HEAD.

## Evidence trail

- The client ring bridge is one thread multiplexing writes and reads
  (`crates/host-runtime/src/client.rs:2464` `start_ring_bridge`). Each loop pass
  takes at most one queued write via `write_rx.try_recv()` (`:2522-2544`),
  then drains inbound, then decides whether to block.
- Writers signal the bridge's private `worker_wake` eventfd once per enqueue
  (`RingWriteSender::try_send`, `:2400-2404`). Eventfds coalesce: eight
  enqueues before the bridge polls produce one readable edge, and
  `drain_eventfd` (`:2445-2448`) consumes it whole.
- The fix is the `wrote` flag: set after a successful send (`:2612`,
  `:1858` (source tree; not at HEAD)), checked at `:2673-2675` — `if wrote { continue; }` — so a pass
  that completed a write skips `arm_data_wait` and the blocking poll
  (`:2679-2698`) entirely and immediately re-polls the write queue.
- Without it, the bridge would process one write, find the coalesced eventfd
  already drained, arm the data doorbell, and block on
  `[worker_wake, data_ready, setup]` (`:2684-2688`) while seven writes sit in
  the queue with no future edge to deliver them.
- `RingWriteSender::drop` also signals (`:2417-2419`), so channel teardown
  cannot strand the final pass.
  At HEAD: The check is `if wrote || pending_control.is_some() || pending_data.is_some()`, so a pass holding an unfinished write also skips the poll.
  At HEAD: Each pass polls two lanes, control and data, into the `pending_control` and `pending_data` slots, and then sends at most one of them.

## Failure scenario

The scenario below was derived against the source tree this record was written from; where the investigation log's post-merge entry records a changed mechanism, the sentences marked "At HEAD" above and that entry carry the current behavior, and the scenario reads as the regression this record guards against.

A caller enqueues a burst of writes, then goes quiet. One write is sent; the
rest wait for the next unrelated event — an inbound frame, a capacity signal,
or peer death. Writes complete with unbounded latency or time out at their
deadlines (`endpoint.send(header, body, deadline)`, `:2561-2566`), reported
as transport failures on a healthy channel.
At HEAD: The bridge calls `endpoint.send_bounded` with a `BRIDGE_RESERVE_SLICE` reserve deadline and the frame's own commit deadline, so one pass takes one capacity slice.

## Timing windows and dependencies

The window opens when more than one write is queued before the bridge drains
`worker_wake`, and closes only on the next external edge. Bounded liveness
claim at HEAD: k queued writes complete in k loop passes with no signal after
the first, because every pass that writes continues and every continue
re-polls the queue. The bound is in loop passes, not wall time; a pass can
still block inside `endpoint.send`'s own capacity wait, which is
`capacity-recheck-after-a-wake-race`'s territory.

## What a test must construct

Multiple writes enqueued without per-write wakes, at most one edge delivered,
then per-write bounded completion. Exists:
`ring_bridge_drains_inbound_and_queued_writes`
(`crates/host-runtime/src/client.rs:7598-7681`) pushes eight writes directly into
`write.tx` — bypassing `RingWriteSender::try_send`, so zero worker_wake edges
(`:7638`) — publishes one inbound frame and signals one explicit edge
(`:7665-7667`), then bounds every completion at 250 ms (`:7674-7680`). Not
yet constructed: the same starvation with the inbound direction idle (the
existing test's one edge doubles as the wake; a variant with no inbound
frame at all would isolate the `wrote` path).

## Investigation log

### Q: does `continue` after a write starve inbound frames instead?

- Sources examined: loop order `:2520-2675`.
- Findings: the inbound drain (`endpoint.try_recv_with`, `:2662-2671`) runs
  before the `wrote` check, so every pass services at most one write and one
  inbound frame; neither side can monopolize a pass. The pre-fix test name,
  `ring_bridge_drains_inbound_between_sustained_writes`, pinned the inbound
  half; the renamed test pins both.
- Missing evidence: none.
- Conclusion: resolved with answer — no.

### Q: is one write per pass a throughput ceiling worth recording?

- Sources examined: `:2522-2544`; `CLIENT_DATA_QUEUE_FRAMES` queue bound.
- Findings: deliberate shape, bounded queue, no evidence of a measured
  problem; a per-pass batch would change deadline fairness.
- Missing evidence: none.
- Conclusion: resolved with answer — not a property; noted as context only.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 12, `:1847-1861` now `:2522-2544`: Each pass polls two lanes, control and data, into the `pending_control` and `pending_data` slots, and then sends at most one of them.
  - line 19, `:1913-1915` now `:2673-2675`: The check is `if wrote || pending_control.is_some() || pending_data.is_some()`, so a pass holding an unfinished write also skips the poll.
  - line 34, `:1850-1852` now `:2561-2566`: The bridge calls `endpoint.send_bounded` with a `BRIDGE_RESERVE_SLICE` reserve deadline and the frame's own commit deadline, so one pass takes one capacity slice.
  Constructs with no counterpart at HEAD; their citations above are marked "source tree; not at HEAD":
  - line 19, `:1858` (a second wrote = true site): One `wrote = true` at `client.rs:2612` covers both lanes at HEAD; there is no second set site, and `let mut wrote = false;` is at `:2520`.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
