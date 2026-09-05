# released-charges-wake-blocked-readers

## Discovery trigger

Fix commit `d9d3e632b` "Wake blocked ring readers when released byte charges
free capacity". Before it, the bridge's charge wait was a 50-microsecond
sleep loop; the eventfd rewrite made the wait blocking, which turned a missed
wake from wasted CPU into a hang. Lead only; mechanism re-verified at HEAD.

## Evidence trail

- The bridge admits an inbound frame only under a `ByteCounter` charge. The
  `charge` closure (`crates/host-runtime/src/client.rs:2622-2661`) loops: refuse
  frames wider than capacity (`:2623-2625`), try `read_budget.charge(bytes)`
  (`:2626-2628`), then block in `poll` on `worker_wake` plus the setup socket
  (`:2632-2656`) and drain the eventfd on wake (`:2655-2657`).
- Waiting there is deliberate backpressure: `endpoint.try_recv_with` advances
  the consumed cursor, so refusing a charge would discard a valid response
  (comment at `:2616-2621`).
- The wake edge is wired at bridge start: `read_budget.set_wake(&wake_fd)`
  (`:2482`, setter at `:2284-2286`) stores a `Weak<OwnedFd>` in the counter.
- The release side is `ByteCharge::drop` (`:2312-2329`): decrement `used`,
  then, if a wake is registered and still alive, `signal_eventfd(&wake)`
  (`:2326`). Charges are dropped by the downstream consumer of the frame
  queue, a different thread from the bridge.
- The registration is per-counter and last-writer-wins (`set_wake` overwrites
  the `Mutex<Option<Weak<OwnedFd>>>`), and `read_budget` is per-connection
  (`:432-444`), so one bridge per counter holds at HEAD.
  At HEAD: The call is guarded by owner.parked as well as by a live registered wake, and parked is read only after the used lock is released.
  At HEAD: the drop signals the eventfd only when owner.parked is set (`:2321`), so a release with no parked waiter performs no write; a registered live wake alone is no longer sufficient.
  At HEAD: The wait is now bracketed by an explicit parked marker, read_budget.parked set true at `:2631` before the loop and false at `:2659` after it, so there is an armed-marker protocol here and the release side consults it.

## Failure scenario

The scenario below was derived against the source tree this record was written from; where the investigation log's post-merge entry records a changed mechanism, the sentences marked "At HEAD" above and that entry carry the current behavior, and the scenario reads as the regression this record guards against.

The read budget is exhausted by in-flight frames. The bridge parks in the
charge poll. The consumer finishes a frame and drops its `ByteCharge`. If
that drop did not signal, nothing else does on an otherwise idle channel:
`worker_wake`'s other writers are the write queue (`:2400-2418`), so a
read-heavy peer with no outbound writes leaves the bridge parked forever.
Every subsequent inbound frame sits in the ring unread; the producer
eventually exhausts and reports deadline errors. A wedged reader presents as
a slow peer.

## Timing windows and dependencies

Signal-before-park: `ByteCharge::drop` can run between the failed
`read_budget.charge` and the bridge's `poll`. The eventfd absorbs that race —
the write leaves the counter nonzero, so the later `poll` returns
immediately; there is no armed-epoch protocol here and none is needed because
the eventfd itself is level-observable state. Bounded window: one poll wakeup
plus one loop iteration per released charge. Dependency: the drop-side signal
fires only when `set_wake` ran first; the ordering is enforced by
construction (`:2482` precedes thread spawn at `:2486`).

## What a test must construct

Genuine budget exhaustion with the bridge parked in the charge poll, then a
charge drop from another thread, then bounded resumption — receipt of the
next frame within an explicit deadline. Nothing does this today: the
`ByteCounter` tests (`:7004-7091`, `:7159`) are synchronous accounting
checks, and both ring-bridge tests keep the budget at
`CLIENT_INBOUND_FRAME_BYTES` with small frames, so the poll arm never
executes. The situation needs a shrunken budget (a constructor exists,
`ByteCounter::new`) and a frame stream wider than it.

## Investigation log

### Q: can `saturating_sub` in the drop hide a lost-capacity defect?

- Sources examined: `:2316-2318`.
- Findings: under-subtraction is impossible; over-subtraction saturates at
  zero and would over-free capacity, the opposite failure. Out of scope for
  this record; the charge-conservation records own it.
- Missing evidence: none.
- Conclusion: resolved with answer — different property.

### Q: is a poisoned wake mutex a hang path?

- Sources examined: `lock_unpoisoned` usage at `:2316`, `:2322`.
- Findings: poison is bypassed, not propagated; the signal still fires.
- Missing evidence: none.
- Conclusion: resolved with answer — no.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 16, `:1879-1901` now `:2632-2656`: The wait is now bracketed by an explicit parked marker, read_budget.parked set true at `:2631` before the loop and false at `:2659` after it, so there is an armed-marker protocol here and the release side consults it.
  - line 22, `:1711-1725` now `:2312-2329`: At HEAD the drop signals the eventfd only when owner.parked is set (`:2321`), so a release with no parked waiter performs no write; a registered live wake alone is no longer sufficient.
  - line 24, `:1722` now `:2326`: The call is guarded by owner.parked as well as by a live registered wake, and parked is read only after the used lock is released.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
