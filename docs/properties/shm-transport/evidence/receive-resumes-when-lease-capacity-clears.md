# receive-resumes-when-lease-capacity-clears

## Citation refresh, 2026-08-31 (eventfd rewrite)

PR #131 (merge `5d638e3e8`) replaced the polling wake mechanism with sparse
eventfd doorbells and moved every cited line in
`crates/shm-transport/src/backend/ring.rs`. All citations below were
re-verified against HEAD. The resumption argument changes with it: a blocked
consumer no longer "polls again"; it parks on the `data_ready` doorbell and the
release's signal is what un-parks it.

## Discovery trigger

`try_receive` returns `Ok(None)` for two structurally different reasons and carries no
way to tell them apart. One means "every lease is out, frames are waiting"; the other
means "there is nothing here". The existing lease-limit test asserts only
`is_none()`, so its oracle is satisfied by either. That is the exact shape the
portfolio evaluation flagged: an assertion that cannot distinguish backpressure from
emptiness cannot prove that saturation ends.

## Evidence trail

- `crates/shm-transport/src/backend/ring.rs:1417-1422` — the saturation gate. It
  loads `active_leases` (`:1413-1416`) and returns `Ok(None)` when
  `active >= self.grant.max_leases`, **before** reading `consumed` or `published`.
  The comment at `:1418-1420` states the intent: a full lease set is backpressure,
  not a fault, and published frames stay queued until a lease is released and the
  caller polls again.
- `ring.rs:1423-1426` — the emptiness gate. It loads `consumed` and `published` and
  returns `Ok(None)` when they are equal. Same return value, unrelated cause, and
  reached only when the first gate did not fire.
- `ring.rs:1452-1454` — the lease accounting the first gate reads:
  `state = SLOT_RECEIVER_LEASED`, `consumed = sequence` with `Release`, and
  `active_leases.fetch_add(1, Relaxed)`. The counter lives on the consumer page and is
  touched only by the receiver.
- `ring.rs:1591-1593` — the only decrement: `release` stores `completion_sequence`
  with `Release`, then `active_leases.fetch_sub(1, Relaxed)`. It runs after the
  `SLOT_RECEIVER_LEASED → SLOT_RELEASE_PENDING` compare-exchange at `:1575-1580`, so
  a duplicate release cannot decrement twice.
- `ring.rs:1598-1599` — new with the eventfd mechanism: after the decrement,
  `release` signals both the `capacity_ready` and `data_ready` doorbells through
  `signal_wake` (`:2026-2037`), which bumps the wake generation and writes the
  eventfd only when a waiter was parked. `data_available` (`:1501-1512`) returns
  true only when `published != consumed` **and** `active < max_leases`, so a
  consumer parked in `wait_for_data` (`:1476-1499`) during saturation is exactly
  the waiter this signal exists to wake.
- `crates/shm-transport/src/lease.rs:324-326`, `:350-357`, `:368-369` — release
  reaches the ring through `release_once`, guarded by a local `released` flag, from
  either the explicit `release()` or `Drop`. Either path clears one lease.
- The available witness for "frames were queued": `ring.rs:1675-1745`
  `conservation()` counts each slot by state, including `SLOT_PUBLISHED` into
  `descriptors.published` (`:1696-1705`) and `SLOT_RECEIVER_LEASED` into
  `descriptors.receiver_leased` (`:1716-1725`). So `published >= 1` at the moment of
  the `None` is observable, which is precisely the fact the current test does not
  check. Caveat carried from `reservation-charge-visible-with-non-free-state`:
  `conservation()` may be test-only, and its `bytes.free` is derived rather than
  observed (`:1739-1743`).
- Existing check: `lease_limit_reports_backpressure_then_recovers_after_release`
  (`crates/shm-transport/tests/ring.rs:214-228`), against
  `lease_limited_profile()` with `descriptor_depth: 2` and `max_leases: 1`
  (`tests/ring.rs:18-30`). It publishes two frames, takes one lease, asserts
  `ring.try_receive().unwrap().is_none()` (`:220-223`), releases, and asserts the
  next receive yields the byte `2` (`:225-226`). The recovery half is real. The
  saturation half is not: at that assertion nothing pins that a frame was pending, so
  an implementation that had lost the second publication entirely would satisfy it up
  to the recovery line.
- The shipped host cannot reach saturation at all. `receive_one` acquires at most one
  lease per call and releases it before returning, on every path: the
  oversized-control rejection at `crates/host-runtime/src/ring_transport.rs:685-687`, the
  normal path at `:734-736`, and `Drop` on every error return. `max_leases` is
  `HOST_TEST_RING_DEPTH`, which is 8
  (`crates/shm-transport/src/profile.rs:679`, `:683-697`), so the endpoint loop
  holds at most one of eight. Saturation is reachable only through a synthetic
  profile or a caller that retains leases.
  At HEAD: `release` signals only the `capacity_ready` doorbell; the doc comment at `:1519-1523` states the data doorbell is deliberately left alone, so a consumer parked in `wait_for_data` is not woken by a release.
  At HEAD: the decrement is an `advance_cursor` compare-exchange (`:1592-1593`), not a `Relaxed` `fetch_sub`.
  At HEAD: both cursors move through `advance_cursor` (`:1951-1956`), which compare-exchanges the expected prior value with `AcqRel` instead of a `Relaxed` `fetch_add`.
  At HEAD: `verified_consumer_cursors` reads `consumed` and `active_leases` together (`:1413-1416`) before the gate, so only `published` is read after it (`:1423`).

## Failure scenario

The scenario below was derived against the source tree this record was written from; where the investigation log's post-merge entry records a changed mechanism, the sentences marked "At HEAD" above and that entry carry the current behavior, and the scenario reads as the regression this record guards against.

1. A receiver holds `max_leases` leases while the producer has published further
   frames. Those frames sit in `SLOT_PUBLISHED` with `consumed < published`.
2. `try_receive` takes the gate at `ring.rs:1417` and returns `Ok(None)`. The caller sees
   the same value it would see on an idle channel. A caller that blocks instead
   parks on the `data_ready` doorbell: `arm_data_wait` (`ring.rs:1187-1220`) sees
   `data_available() == false` because `active == max_leases`, records a parked
   epoch, and the consumer sleeps in `wait_until` (`ring.rs:1490-1493`).
3. The receiver releases a lease. `active_leases` drops below `max_leases`,
   `completion_sequence` is published for the released sequence, and both doorbells
   are signalled (`ring.rs:1598-1599`).
4. Two defect families now break resumption. State defect: the decrement missed,
   `release_once`'s flag inverted, or the gate comparison off by one in the closing
   direction leaves `active_leases` at or above `max_leases` permanently, and every
   later `try_receive` returns `Ok(None)`. Wake defect: the state clears but the
   `data_ready` signal is skipped or lost while the consumer is parked, so a
   blocking consumer sleeps to its deadline even though a bare `try_receive` would
   succeed. The pre-eventfd design had no second family, because a polling consumer
   re-evaluated the gates every 50 microseconds regardless.
5. Either way the channel looks dead and its only signal is the value that also
   means "idle". In the host, `receive_one` returns `Ok(false)`
   (`ring_transport.rs:678-679`), the endpoint loop re-arms the data wait and parks
   (`:566`, `:594`), no error is raised, no quarantine occurs, and no counter moves.
   This is the same silent-capacity-loss signature as
   `attach-reconciles-or-refuses-stale-shared-cursors`, reached without any crash.
   At HEAD: only `capacity_ready` is signalled on release, so a release does not wake a parked `wait_for_data`.

## Timing windows and dependencies

There is no race window on the counter: it is incremented and decremented by the
same thread-confined receiver, both `Relaxed`, and read `Relaxed` at `ring.rs:1413-1416`. The
state half of the property is about state, not ordering, and its bounded fault-free
window is short and exactly statable: once a release has returned `Ok`, the **next
single** `try_receive` must deliver, because both gates are pure reads of state the
release already updated. The wake half has a window the polling design did not: a
consumer parks between `arm_data_wait`'s final recheck (`ring.rs:1216-1218`) and the
`wait_until` (`ring.rs:1490-1493`), and correctness depends on `release` observing the parked
flag in `signal_wake`'s swap (`ring.rs:2033`) and writing the eventfd. The arm/recheck
protocol (park epoch, recheck data, undo park on generation change) closes the
race where data arrives between the check and the park. Cross-process changes
nothing about the counter, since `active_leases` is receiver-local. Dependencies:
saturation must be constructed with a profile whose `max_leases` is small enough to
reach, and at least one frame must be published beyond the leased set, or the
second gate fires first and the test proves nothing about the first. The
reachability caveat above is a dependency on the configuration, not on timing: in
the shipped host topology this window never opens.
At HEAD: both cursors are loaded with `Acquire` and checked against the handle's own record in `verified_consumer_cursors` (`:1990-2000`), not read `Relaxed`.

## What a test must construct

Saturation with a witnessed backlog, then a single-call recovery assertion, then a
parked-waiter recovery assertion. Extend the existing test rather than replacing
it. At the moment of the `None`, assert `conservation()` reports
`descriptors.receiver_leased == max_leases` and `descriptors.published >= 1`. That
converts `is_none()` from an ambiguous value into a statement about a specific
state, and it is the assertion that distinguishes saturation from an empty ring.
Then release exactly one lease and assert that the immediately following
`try_receive` returns `Some`, that its identity's sequence equals the
pre-saturation `consumed + 1`, and that its body equals the frame that was pending
— so recovery cannot be satisfied by delivering some other frame or by skipping
one. Assert `active_leases` is back to `max_leases - 1` by way of
`descriptors.receiver_leased`. A wake arm is also owed that the polling design did
not need: park a consumer in `wait_for_data` with a deadline well above the
expected wake latency while the lease set is saturated and a frame is pending,
release one lease from another lease holder, and assert `wait_for_data` returns
`true` strictly before the deadline — a lost `data_ready` signal fails this arm as
a timeout while the bare `try_receive` arm still passes, which is exactly the
separation that localises a lost wake.

**That wake arm cannot be built by extending the existing in-process test, and
needs a harness decision before it is queued as work.** `wait_for_data` blocks
the calling thread, so the release has to come from somewhere else; but both
`Ring` (`ring.rs:1001-1034`) and `ReceiveLease` (`crates/shm-transport/src/lease.rs:256`) carry
`PhantomData<Rc<()>>` and are therefore neither `Send` nor `Sync`, so neither the
ring nor a held lease can be moved to a second thread to release it there. The
arm requires one of: a cross-process harness driving the release over raw
identities (the shape `tests/ring.rs` already uses for cross-process attach), or
a scheduling seam inside the wait that lets a single-threaded test interleave a
release with a parked waiter. Until one is chosen this arm is not implementable
and must not be recorded as merely unwritten.

Add a multi-lease arm, since
`max_leases: 1` makes "below the cap" and "zero" the same number and hides an
off-by-one: with `max_leases: 4`, take four, assert `None`, release one, assert
exactly one further receive succeeds and the next returns `None` again. Coverage
check to emit: `shm_lease_saturation_observed_then_drained`, which is the
situation record `lease-saturation-is-reached-then-drains` and is what keeps this
property from passing vacuously in a configuration that can never saturate.

## Investigation log

### Q: Can the two `Ok(None)` causes be distinguished by any current caller, and is saturation reachable in the shipped topology?

- Sources examined: `ring.rs:1267-1340`, `:1345-1390`, `:1395-1472`; `lease.rs:324-371`;
  `crates/host-runtime/src/ring_transport.rs:32`, `:41-58`, `:664-747`, `:472-632`;
  `tests/ring.rs:18-38`, `:213-228`.
- Findings: no caller can distinguish the two causes from the return value; the only
  distinguishing evidence is `conservation()`, and no existing test uses it at the
  point of a `None`. Saturation is reachable in-crate only through
  `lease_limited_profile()`. In the host, `receive_one` holds at most one of eight
  leases and always releases before returning, so the first gate cannot fire. That
  makes the property conditional in the shipped configuration, and it is why the check
  semantics are `always-or-unreached` rather than `always`: the obligation is real, and
  the situation must be constructed rather than assumed.
- Missing evidence: whether any intended consumer retains leases across calls. The
  addon does exactly that — `poll` forgets the lease and tracks the identity in its own
  table, per the reachability analysis in `release-authority-bound-to-lease-ownership` —
  which suggests the addon path can saturate where the Rust host cannot. That path was
  not traced end to end here and is left as an open question rather than asserted.
- Conclusion: resolved with answer — the gap in the existing oracle is exactly the
  missing "a frame was pending" fact, the fix is a state assertion at the `None` plus a
  single-call recovery assertion, and the reachability finding changes the semantics to
  `always-or-unreached` and makes the coverage marker mandatory.

### 2026-08-31: re-derivation against the eventfd doorbell mechanism

- Sources examined: `crates/shm-transport/src/backend/ring.rs:1187-1220`,
  `:1395-1426`, `:1452-1454`, `:1476-1512`, `:1575-1599`, `:1675-1745`,
  `:2026-2037`; `crates/shm-transport/src/lease.rs:324-371`;
  `crates/host-runtime/src/ring_transport.rs:566`, `:594`, `:674-687`, `:734-736`;
  `crates/shm-transport/tests/ring.rs:18-30`, `:214-228`;
  `crates/shm-transport/src/profile.rs:679-697`.
- Findings: the guarantee, both gates, and the single decrement site survive PR
  #131 with only line moves. What changed is resumption for a blocked consumer:
  the pre-eventfd argument was "the endpoint keeps polling, so the next poll after
  the release delivers"; at HEAD `release` signals `data_ready` and
  `capacity_ready` (`ring.rs:1598-1599`) and a parked `wait_for_data` waiter is woken
  sparsely (`signal_wake` writes the eventfd only when `parked != 0`, `ring.rs:2033`).
  `data_available` includes the lease gate (`active < max_leases`, `ring.rs:1511`), so
  saturation is a parking state, and the release's signal is load-bearing for
  liveness rather than an optimization. This adds a wake-defect family (lost or
  skipped signal) alongside the original state-defect family and adds the
  parked-waiter arm to the test construction.
- Missing evidence: none for the mechanism; the addon lease-retention question in
  the prior section stands unchanged.
- Conclusion: resolved with answer — the check semantics stay
  `always-or-unreached`, the single-call recovery window stands for the state
  half, and the wake half needs its own arm with a deadline strictly separating a
  wake from a timeout.
  At HEAD: `release` signals `capacity_ready` only; `data_ready` is deliberately not touched (`:1519-1523`), so the release does not wake a parked `wait_for_data` waiter.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 23, `crates/shm-transport/src/backend/ring.rs:1063-1068` now `crates/shm-transport/src/backend/ring.rs:1417-1422`: At HEAD `verified_consumer_cursors` reads `consumed` and `active_leases` together (`:1413-1416`) before the gate, so only `published` is read after it (`:1423`).
  - line 32, `ring.rs:1115-1117` now `ring.rs:1452-1454`: At HEAD both cursors move through `advance_cursor` (`:1951-1956`), which compare-exchanges the expected prior value with `AcqRel` instead of a `Relaxed` `fetch_add`.
  - line 36, `ring.rs:1229-1234` now `ring.rs:1591-1593`: At HEAD the decrement is an `advance_cursor` compare-exchange (`:1592-1593`), not a `Relaxed` `fetch_sub`.
  - line 40, `ring.rs:1236-1241` now `ring.rs:1598-1599`: At HEAD `release` signals only the `capacity_ready` doorbell; the doc comment at `:1519-1523` states the data doorbell is deliberately left alone, so a consumer parked in `wait_for_data` is not woken by a release.
  - line 87, `:1236-1241` now `ring.rs:1598-1599`: At HEAD only `capacity_ready` is signalled on release, so a release does not wake a parked `wait_for_data`.
  - line 106, `:1062` now `ring.rs:1413-1416`: At HEAD both cursors are loaded with `Acquire` and checked against the handle's own record in `verified_consumer_cursors` (`:1990-2000`), not read `Relaxed`.
  - line 201, `:1236-1241` now `ring.rs:1598-1599`: At HEAD `release` signals `capacity_ready` only; `data_ready` is deliberately not touched (`:1519-1523`), so the release does not wake a parked `wait_for_data` waiter.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
