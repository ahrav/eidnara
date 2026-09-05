# readiness-redispatch-is-bounded-under-persistent-arm-failure

## Discovery trigger

Review of `addon-scheduling-wakes-only-on-acknowledged-readiness` noted that
sequential callbacks are compatible with an unbounded number of them: a
channel whose re-arm fails on every acknowledgement requeues the dispatcher
forever.

## Evidence trail

- `packages/shm-native/src/lib.rs:1352-1391`: `readiness_handled` walks
  `registry.channels`, skips unregistered ones, and for each registered channel
  sets `redispatch |= channel.from_host.complete_data_wait().is_err()` and then
  matches `arm_data_wait()`: `Ok(true) => {}`, `Ok(false) | Err(_) => redispatch
  = true`. It calls `reactor.handled()` and returns `redispatch`.
- `packages/shm-native/index.ts:699-729`: `dispatchReadiness` runs every
  handler, catching per-handler errors, and in `finally` does
  `if (loaded?.readinessHandled()) queueMicrotask(dispatchReadiness)`.
- `crates/shm-transport/src/backend/ring.rs:1202-1207`: `arm_data_wait` returns
  `Err(RingError::Quarantined)` when `is_quarantined()`; `Ok(false)` when
  `data_available()`.
- Unregistration sites: `lib.rs:1552` inside `close` and `:1585` inside
  `force_close`; `scheduling.rs:299-309` inside `Reactor::register`, which
  unregisters and fails registration when the first `arm_data_wait` errs. So a
  ring quarantined before `watch` never enters the loop; a ring quarantined
  after a successful registration is never unregistered by the acknowledgement
  walk.
- Node semantics: a microtask that requeues itself runs before any pending
  macrotask, so the loop starves timers and I/O for the whole process.

## Failure scenario

The scenario below was derived against the source tree this record was written from; where the investigation log's post-merge entry records a changed mechanism, the sentences marked "At HEAD" above and that entry carry the current behavior, and the scenario reads as the regression this record guards against.

A peer dies; the client's ring quarantines; the handler runs, sees the error,
and does not close the channel in the same tick (or ever). Every
acknowledgement returns true and the dispatcher requeues itself as a microtask
indefinitely.

## Timing windows and dependencies

From the quarantine until the caller's `close`. No race is needed.

## What a test must construct

One registered channel, its ring quarantined, a handler that does not close;
count dispatcher invocations and schedule a timer to prove macrotasks run.

## Investigation log

### Q: Does anything bound the loop today?

- Sources examined: `readiness_handled`, `dispatchReadiness`, the
  unregistration sites.
- Findings: the two return values are conflated into one boolean and only the
  caller unregisters.
- Missing evidence: none for the mechanism; a test to witness the spin.
- Conclusion: the property is stated as a requirement the current code does not
  meet; the open question asks which fix is intended.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 12, `packages/shm-native/src/lib.rs:1191-1214` now `packages/shm-native/src/lib.rs:1352-1391`: At HEAD `readiness_handled` walks reactor.take_ready() rather than every registered channel, and a channel whose complete_data_wait fails or whose arm_data_wait errs is unregistered on the spot (`:1385`); redispatch is set only when a lease advanced while data remains ready (`:1378-1381`), so a persistent arm failure removes the channel instead of requeueing the dispatcher forever.
  - line 17, `packages/shm-native/index.ts:532-556` now `packages/shm-native/index.ts:699-729`: The requeue now goes through `scheduleRedispatch` (`:689-697`), which queues a microtask for at most REDISPATCH_MICROTASK_BUDGET, sixteen, consecutive rounds and then yields with setImmediate, so a self-requeueing chain no longer starves timers and I/O; a handler that throws is also dropped from the map (`:719`).
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
