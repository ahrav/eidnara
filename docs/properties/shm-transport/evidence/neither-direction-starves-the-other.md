# neither-direction-starves-the-other

## Citation refresh, 2026-08-31 (eventfd rewrite)

PR #131 (merge `5d638e3e8`) replaced the polling wake mechanism with sparse
eventfd doorbells. `POLL_INTERVAL` is gone from production and survives only
in `crates/host-runtime/tests/support/process_resources.rs:75`, a test-support
poll constant. The endpoint loop, `receive_one`, and the peer harness
(`TestShmPeer` is now `RingClientEndpoint`) were rewritten; every line below
was re-verified against HEAD. `POLL_INTERVAL`-based derivations in this file
are replaced; where prior text is kept for history it is explicitly marked
pre-eventfd.

## Citation refresh, 2026-08-30

The ring-transport refactor (`0f336d3c`, `d8bde128`, `793a973e`, `ed487e11`)
renamed `crates/host-runtime/src/shm_provider.rs` to
`crates/host-runtime/src/ring_transport.rs` and deleted `provider_recovery.rs`,
`transport_negotiation.rs`, and `transport_provider.rs`. Host-side citations below
were re-anchored against `ring_transport.rs` at `e447c927`.

Where the cited construct survives, the citation names `ring_transport.rs` and a
line re-verified against that commit. Where it does not, the original reference is
kept and prefixed `former`, so it reads as pre-refactor evidence rather than a
current location. A `former` line number is never a claim about the tree today.
Every `provider_recovery.rs` reference is `former` by definition: that module has
no successor. See the refresh note in [../catalog.md](../catalog.md).

## Discovery trigger

The host drives both directions of a duplex ring pair from one task on one thread. The
code says so, and it says why: a comment at `ring_transport.rs:553-557` claims
"directions alternate under sustained inbound traffic ... so a peer that refills the
inbound ring as slots release cannot starve responses, Pings, and close frames while
host-to-peer capacity is free". That is a liveness claim about a single-threaded loop,
stated in a comment, asserted nowhere. Every shared-memory host test is strict
request-response lockstep, so no test has ever had frames in flight in both directions
at once.

## Evidence trail

- `crates/host-runtime/src/ring_transport.rs:305-310` — the endpoint runs on a dedicated OS
  thread named `host-shm-endpoint` carrying a `new_current_thread` Tokio runtime.
  `:331-341` runs `run_endpoint` under `block_on` inside `catch_unwind`. One task, one
  thread, both directions.
- `:636-894` — `run_endpoint`. Each iteration runs `Publisher::pump` at the top
  (`:696`), at most one `receive_one` (`:713-725`), and `Publisher::pump` again
  after any push (`:882-884`).
- `:552-558` — the alternation the comment claims. When a frame was received, the loop
  takes at most one queued outbound frame with a non-blocking `queue.try_recv().ok()`.
  This is the inbound-cannot-starve-outbound direction, and it holds: a receive is
  always followed by an outbound attempt.
- `:565-617` — when nothing was received, the loop arms the data doorbell with
  `arm_data_wait` (`:566`) and enters a `biased` `select!` (`:582-617`) over discard,
  finish, read-cancel, `queue.recv()`, and `readiness.readable()` on an `AsyncFd`
  wrapping the duplicated `data_ready` eventfd (`:485-488`). There is no idle sleep
  and no poll interval: the loop is woken by the peer's doorbell signal or by its own
  cancellation and queue events.
- **First starvation path, outbound blocks inbound: superseded at HEAD.** The
  host publish is `Publisher::try_publish` (`:1221-1277`), called from
  `Publisher::pump` (`:1159-1217`). It reserves with the nonblocking
  `Ring::try_reserve_in` (`:1240-1244`) and returns `Ok(false)` on `Exhausted`,
  leaving the frame in the pending set with a deadline of
  `now + frame_deadline` (`:1152`). `run_endpoint` arms `Ring::arm_capacity_wait`
  (`:788`) only while frames are pending and selects on the capacity descriptor
  (`:845-866`) beside data readiness (`:823-844`) and
  `sleep_until(publisher.earliest_deadline())` (`:867-877`), so a full outbound
  ring does not stop `try_receive`. The stall that remains is per frame and
  bounded by `frame_deadline`: `pump` returns `Err` when any pending frame passes
  its deadline (`:1168-1171`) and the loop calls `fail` and returns (`:696-704`,
  `:884-892`). `Ring::reserve_until`
  (`crates/shm-transport/src/backend/ring.rs:1025-1086`) still parks the calling
  thread on `capacity_ready` (`:1075-1078`); its remaining Rust caller is the
  test-only `RingClientEndpoint::send` (`ring_transport.rs:1520`) through
  `reserve_until_in`, not the endpoint; the production bridge uses
  `try_send_bounded` (`:1543`) and parks on `arm_capacity_wait` instead.
- **Second starvation path, inbound blocks outbound.** `receive_one` ends with
  `inbound.send(Ok(InboundEvent::Frame(..))).await` (`:737-745`) on the bounded
  channel created at `:283` with `mpsc::channel(queue_frames)`. That await has no
  timeout and is not inside a `select!`, so it parks until the application drains the
  channel or the receiver is dropped. While parked, the endpoint publishes nothing.
  The bound comes from the far side instead: a sender whose queue stays full past its
  deadline retires the generation itself (`crates/host-runtime/src/frame_channel.rs:253-265`,
  the `timeout_at` arm at `:256` and the `retired`/`generation` cancel at `:259-262`),
  where the deadline is `admission_deadline()` (`:243-245`) built from the same
  `frame_deadline` (`ring_transport.rs:278`). So the symptom is a retired generation,
  not a hang.
- **Where the design does defend itself.** The ingress-budget wait inside
  `receive_one` explicitly services outbound frames rather than blocking on the
  budget alone: the select at `:982-1024` includes a `queue.recv()` arm that
  pushes the frame and runs `Publisher::pump` (`:1000-1008`), a capacity
  readiness arm that pumps after a peer return (`:1009-1020`), and a
  `sleep_until(deadline)` arm for the budget itself (`:994-999`).
  That path is the counterexample to the claim that the loop never yields to the
  other direction, and it is why the property is a bounded ratio rather than a flat
  prohibition.
- Lease pressure is not the mechanism. `receive_one` holds at most one lease and
  releases it on every path (`:685-687`, `:734-736`, `Drop` on error), out of
  `max_leases == 8` (`crates/shm-transport/src/profile.rs:679`, `:683-697`).
- Existing check: none. Every host shared-memory test is lockstep. The peer harness
  `RingClientEndpoint` offers `send` that reserves, writes, and commits in one
  blocking call (`ring_transport.rs:885-892`), `recv` that blocks in `wait_for_data`
  to a deadline (`:1015-1032`), and a non-blocking `try_recv` (`:935-938`); nothing in
  the suites drives send and receive concurrently. The transport test
  `two_process_zero_copy_exchange_uses_authenticated_grant`
  (`crates/shm-transport/tests/ring.rs:488-543`) uses a single ring in a single
  direction.
  At HEAD: recv is cfg(test)-only at HEAD, so no shipped host code calls wait_for_data.
  At HEAD: `send` (`:1520`) reserves through `reserve_until_in` and `try_send_bounded` (`:1543`) through `try_reserve_in`; both hand the reservation to `publish` (`:1580-1604`), which writes, rechecks the frame deadline, checks quarantine, then commits, and reports a `SendFailure` stage instead of an opaque error.
  At HEAD: send_ticket_before and the publication ticket are gone; the admission select! lives in send_before and reserves a permit with self.tx.reserve().
  At HEAD: The inbound channel is sized queue_frames plus one and the extra slot is held as an owned permit for the terminal event, so a fault or cancellation is delivered even when the receiver has stopped draining (`:279-291`).
  At HEAD: The handoff goes through `deliver`, a biased select! over inbound.send, queue.discard.cancelled(), and root.cancelled(), so it is cancellable and no longer an unselected untimed await.
  At HEAD: A failed `Publisher::pump` calls `fail` (`:696-704`, `:884-892`), which sends ReadClose::Corrupt("shared-memory publish failed") on the inbound channel before cancelling `retired` and `root` (`:897-908`).
  At HEAD: The data_ready doorbell is one end of a connected AF_UNIX stream socketpair, not an eventfd (`crates/shm-transport/src/backend/ring.rs:710-728`).

## Failure scenario

The scenario below was derived against the source tree this record was written from; where the investigation log's post-merge entry records a changed mechanism, the sentences marked "At HEAD" above and that entry carry the current behavior, and the scenario reads as the regression this record guards against.

1. The peer offers inbound frames continuously and the host has responses queued, so
   both directions have work.
2. The peer stops draining host-to-peer, or drains it slower than the host fills it.
   The outbound ring reaches depth or its arena fills.
3. Superseded at HEAD: the next `Publisher::try_publish` gets `Exhausted` from
   `try_reserve_in` (`ring_transport.rs:1240-1242`) and leaves the frame pending;
   the host keeps calling `try_receive` and parks only in the `select!` over data
   readiness, capacity readiness, and the earliest pending deadline (`:810-879`).
   In the source tree this record was written against, the next publish enters
   `reserve_until` and parks on the `capacity_ready`
   doorbell. For up to `frame_deadline` the host performs no `try_receive`, so
   inbound frames accumulate in `SLOT_PUBLISHED`. Unlike the pre-eventfd design,
   the host is not spinning: it is asleep in `wait_until` (`ring.rs:1381`) and only
   the peer's release (which signals `capacity_ready`, `ring.rs:1598-1599`) or the
   deadline wakes it. A lost or skipped wake therefore presents as the full
   `frame_deadline` stall even if capacity cleared earlier.
4. The peer's own `reserve_until` on its producer ring now parks too: the inbound
   ring is at depth because nobody is consuming it, and the host, being parked on
   the other lane, never leases a frame and never signals the peer's
   `capacity_ready`. The peer sees `Deadline` from a healthy transport whose only
   problem is that the host is parked on the other lane.
5. Symmetrically, if the application stops draining the `inbound` channel, the endpoint
   parks in `inbound.send().await` and publishes nothing until the outbound sender's own
   admission timeout retires the generation. Either way one direction's pressure ends
   the other direction's progress, and both terminate as transport faults.
   At HEAD: Release signals only the capacity doorbell now; the data doorbell is deliberately left alone because the releasing thread is the thread that would poll for data.

## Timing windows and dependencies

Two bounds define the property and both are configuration, not constants in this
file. The outbound stall is bounded by `frame_deadline` per publish attempt
(`ring_transport.rs:768`). The inbound stall is bounded by the sender's admission
timeout, the same `frame_deadline` (`frame_channel.rs:243-245`, wired at
`ring_transport.rs:278`). The former third quantity, the 50-microsecond
`POLL_INTERVAL` idle-poll granularity, no longer exists: waits park on eventfd
doorbells (`ring.rs:1381`, `:1492`) and the corresponding floor is doorbell wake
latency, which the code does not constant-bound. Because the endpoint owns its own
thread and its own current-thread runtime, none of this blocking harms other host
tasks — the damage is confined to the opposite lane of the same endpoint, which is
exactly the scope of this property. Dependency: the situation requires frames
genuinely in flight in both directions at once, which today is never constructed;
that precondition is carried as `duplex-overlap-is-reached`. The property is only
meaningful for a peer that is draining at all. A peer that has stopped draining
permanently is the dead-peer case and belongs to
`dead-peer-charges-are-reclaimed-or-declared`.

## What a test must construct

Simultaneous offered load in both directions, then removal of the pressure, then a
bounded drain assertion. The peer side needs a shape `RingClientEndpoint` does not
have today: independent send and receive threads, or a non-blocking send to pair
with the existing `try_recv`, so the peer can hold frames outstanding in both
directions rather than alternating. The oracle has two parts. Non-starvation under
load: over a measurement window in which both directions have frames offered
continuously, both directions must complete at least one frame — a ratio bound with
an explicit constant, for example neither direction completing fewer than one frame
per K completions of the other, with K pinned from the test's own `frame_deadline`
configuration and recorded in the test; there is no code-derived K because the only
per-lane bound the code enforces is `frame_deadline` itself. Bounded drain after
pressure stops: stop offering on both sides, poll until stable within an explicit
bound, then assert both queues are empty, `conservation()` reports all descriptors
free on both rings, and neither direction reported a close, strictly inside the
bound. Under eventfd the drain arm doubles as a lost-wake detector: a parked
`reserve_until` whose `capacity_ready` signal was skipped converts a sub-deadline
drain into a `frame_deadline`-shaped stall, so the drain bound must be set well
below `frame_deadline` to distinguish a wake from a timeout. A third arm pins the
second starvation path directly: stop draining the `inbound` channel while outbound
frames are queued, and assert the observable outcome is the sender's admission
timeout rather than an unbounded stall. Coverage check to emit:
`shm_both_directions_in_flight`.

## Investigation log

### Q: Can a single-threaded endpoint loop starve one direction, and if so which one?

- Sources examined: `crates/host-runtime/src/ring_transport.rs:33`, `:75-100`, `:287-397`,
  `:459-544`, `:455-534`, `:536-578`, `:665-691`, `:711-777`;
  `crates/host-runtime/src/frame_channel.rs:770-826`, `:838-880`;
  `crates/shm-transport/src/backend/ring.rs:738-759`, `:766-846`;
  `crates/host-runtime/tests/shm_transport.rs:189-271`.
- Findings: yes, and the two directions are not symmetric. Inbound cannot starve
  outbound through the receive path, because every received frame is followed by one
  non-blocking outbound attempt (`ring_transport.rs:552-558`) and the ingress-budget wait also services
  outbound (`:721-728`) — the comment's claim is accurate for the case it describes.
  Outbound *can* starve inbound in the source tree, because its publish blocks the
  single thread inside `reserve_until` with no yield, for up to `frame_deadline`
  per frame; at HEAD `Publisher::try_publish` (`ring_transport.rs:1221`) is
  nonblocking and this arm does not apply. And inbound
  *can* starve outbound through a path the comment does not cover: the unbounded
  `inbound.send().await` at `:737-745`, which is neither timed nor selected against the
  outbound queue. Neither stall is infinite — the first ends in an unclean close, the
  second in the sender's admission timeout — so the accurate statement is bounded
  starvation with a fault-shaped outcome, not a deadlock.
- Missing evidence: the numeric ratio. `frame_deadline` is caller-supplied
  (`ProviderContext`), so the worst-case service ratio between the lanes cannot be
  derived from this crate alone; a test must pin it from the configuration it runs
  under. Also untested rather than unknown: whether the addon endpoint, which drives the
  same rings from JavaScript, has the same shape.
- Conclusion: resolved with answer — one direction can starve the other, in both
  directions, by two distinct mechanisms, and the comment at `:553-557` is true only of
  the receive-then-publish alternation and not of the two blocking paths. The property
  must therefore be a bounded ratio plus a bounded post-pressure drain, and the
  duplex-overlap situation must be constructed before either can be measured.
  At HEAD: The handoff is inside `deliver`, a biased select! against discard and root, so it is bounded by cancellation rather than being neither timed nor selected.

### 2026-08-31: re-derivation against the eventfd doorbell mechanism

- Sources examined: `crates/host-runtime/src/ring_transport.rs:472-632`, `:664-747`,
  `:749-812`, `:884-955`, `:1076-1084`;
  `crates/shm-transport/src/backend/ring.rs:714-842`, `:1187-1220`, `:1345-1390`,
  `:1476-1499`, `:1598-1599`, `:2026-2037`, `:2376`;
  `crates/host-runtime/src/frame_channel.rs:243-245`, `:253-265`.
- Findings: both starvation mechanisms survive PR #131 unchanged in shape.
  the host publish in that tree is still synchronous on the endpoint thread; its
  wait is a parked `capacity_ready.wait_until` instead of a 50-microsecond retry
  sleep, so
  the outbound-blocks-inbound stall is identical in bound (`frame_deadline`) but
  different in failure texture: a lost `capacity_ready` wake presents as the full
  deadline. The untimed `inbound.send().await` is unchanged. The idle loop no
  longer sleeps at all; it arms `arm_data_wait` and parks in a select over the
  `AsyncFd`-wrapped doorbell, so the pre-eventfd `frame_deadline / POLL_INTERVAL`
  ratio derivation has no referent. A repo test pins the removal:
  `shared_memory_workers_have_no_periodic_polling` (`ring_transport.rs:1076-1084`).
  `POLL_INTERVAL` survives only in
  `crates/host-runtime/tests/support/process_resources.rs:75`.
- Missing evidence: unchanged from the prior section — the numeric ratio K remains
  a configuration decision, and doorbell wake latency has no code-stated constant
  bound to substitute for the old poll quantum.
- Conclusion: resolved with answer — the guarantee and both starvation mechanisms
  survive; only the bound derivation changes. K must be pinned by the test from
  `frame_deadline` alone, and the drain arm gains a second job as a lost-wake
  detector.
  At HEAD: The doorbell is a connected AF_UNIX stream socketpair end held in a UnixStream, not an eventfd, and its syscalls go through backend/sys.rs.
  At HEAD: The main impl block holds `send`, `try_send_bounded`, `publish`, `try_recv`, and `try_recv_with`; `recv` lives in a separate `cfg(test)` impl at `:1695-1712`.
  At HEAD: The host publish is `Publisher::try_publish` (`ring_transport.rs:1221`), which is nonblocking; the outbound-blocks-inbound stall this entry describes does not exist, and only the per-frame deadline (`:1168-1171`) survives from it.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 55, `:371-374` now `:485-488`: The data_ready doorbell is one end of a connected AF_UNIX stream socketpair, not an eventfd (`crates/shm-transport/src/backend/ring.rs:710-728`).
  - line 67, `ring_transport.rs:479-483` now `ring_transport.rs:696-704`: A failed `Publisher::pump` calls `fail`, which sends ReadClose::Corrupt("shared-memory publish failed") on the inbound channel before cancelling `retired` and `root`.
  - line 69, `:551-556` now `:737-745`: The handoff goes through `deliver`, a biased select! over inbound.send, queue.discard.cancelled(), and root.cancelled(), so it is cancellable and no longer an unselected untimed await.
  - line 70, `:230` now `:283`: The inbound channel is sized queue_frames plus one and the extra slot is held as an owned permit for the terminal event, so a fault or cancellation is delivered even when the receiver has stopped draining (`:279-291`).
  - line 74, `crates/host-runtime/src/frame_channel.rs:640-652` now `crates/host-runtime/src/frame_channel.rs:253-265`: send_ticket_before and the publication ticket are gone; the admission select! lives in send_before and reserves a permit with self.tx.reserve().
  - line 91, `ring_transport.rs:684-700` now `ring_transport.rs:885-892`: send delegates to send_bounded (`:901-933`), which reserves, writes, rechecks the frame deadline, checks quarantine, then commits, and reports a SendFailure stage instead of an opaque error.
  - line 92, `:702-716` now `:1015-1032`: recv is cfg(test)-only at HEAD, so no shipped host code calls wait_for_data.
  - line 108, `ring.rs:1236-1241` now `ring.rs:1598-1599`: Release signals only the capacity doorbell now; the data doorbell is deliberately left alone because the releasing thread is the thread that would poll for data.
  - line 181, `:612-617` now `:737-745`: The handoff is inside `deliver`, a biased select! against discard and root, so it is bounded by cancellation rather than being neither timed nor selected.
  - line 199, `:684-721` now `:884-955`: The main impl block holds send, send_bounded, try_recv, and try_recv_with; recv moved into a separate cfg(test) impl at `:1013-1032`.
  - line 200, `crates/shm-transport/src/backend/ring.rs:384-467` now `crates/shm-transport/src/backend/ring.rs:714-842`: The doorbell is a connected AF_UNIX stream socketpair end held in a UnixStream, not an eventfd, and its syscalls go through backend/sys.rs.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.

### Q: Does the outbound-blocks-inbound mechanism survive the nonblocking publisher?

- Sources examined: `crates/host-runtime/src/ring_transport.rs:636-894`
  (`run_endpoint`), `:926-1038` (`receive_one`), `:1075-1278` (`Publisher`),
  `:1520-1604` (`RingClientEndpoint::send`, `try_send_bounded`, `publish`);
  `crates/shm-transport/src/backend/ring.rs:887-922` (`arm_capacity_wait`,
  `complete_capacity_wait`, `duplicate_capacity_ready`), `:940-1020`
  (`try_reserve_in`), `:1025-1086` (`reserve_until`), `:1506-1524`
  (`take_reclaimed`).
- Findings: no. The endpoint never enters `Ring::reserve_until`; every publish
  attempt is `Ring::try_reserve_in` from `Publisher::try_publish`, and a frame
  that finds no capacity stays pending while `run_endpoint` and the budget loop
  in `receive_one` keep receiving and park only in a `select!` that includes the
  duplicated capacity descriptor and the earliest pending deadline. `pump`
  first calls `Ring::take_reclaimed` to drop terminal credits at the physical
  block return (`:1161-1167`), then fails the generation if any pending frame is
  past its deadline (`:1168-1171`), then publishes every eligible frame. The
  inbound-blocks-outbound arm is unchanged in shape: `deliver` (`:911-923`)
  parks until the application drains or a lifecycle token cancels.
- Missing evidence: the ratio arm of the catalog record assumed two blocking
  paths; with one gone, whether a bounded ratio is still the right check or the
  record reduces to the inbound stall plus the per-frame deadline is a catalog
  decision.
- Conclusion: resolved with answer - the outbound stall is superseded by a
  per-frame deadline that retires the generation without parking the thread;
  the catalog record's Check and Fault/timing angle carry a mechanism note.
