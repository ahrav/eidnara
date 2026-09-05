# readiness-redispatch-is-bounded-under-persistent-arm-failure

## Discovery trigger

Review of `addon-scheduling-wakes-only-on-acknowledged-readiness` noted that
sequential callbacks are compatible with an unbounded number of them: a
channel whose re-arm fails on every acknowledgement requeues the dispatcher
forever.

## Evidence trail

- `packages/shm-native/src/lib.rs:1143-1166`: `readiness_handled` walks
  `registry.channels`, skips unregistered ones, and for each registered channel
  sets `redispatch |= channel.from_host.complete_data_wait().is_err()` and then
  matches `arm_data_wait()`: `Ok(true) => {}`, `Ok(false) | Err(_) => redispatch
  = true`. It calls `reactor.handled()` and returns `redispatch`.
- `packages/shm-native/index.ts:515-525`: `dispatchReadiness` runs every
  handler, catching per-handler errors, and in `finally` does
  `if (loaded?.readinessHandled()) queueMicrotask(dispatchReadiness)`.
- `crates/shm-transport/src/backend/ring.rs:1066-1071`: `arm_data_wait` returns
  `Err(RingError::Quarantined)` when `is_quarantined()`; `Ok(false)` when
  `data_available()`.
- Unregistration sites: `lib.rs:1323` inside `close` and `:1350` inside
  `force_close`; `scheduling.rs:242-252` inside `Reactor::register`, which
  unregisters and fails registration when the first `arm_data_wait` errs. So a
  ring quarantined before `watch` never enters the loop; a ring quarantined
  after a successful registration is never unregistered by the acknowledgement
  walk.
- Node semantics: a microtask that requeues itself runs before any pending
  macrotask, so the loop starves timers and I/O for the whole process.

## Failure scenario

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
