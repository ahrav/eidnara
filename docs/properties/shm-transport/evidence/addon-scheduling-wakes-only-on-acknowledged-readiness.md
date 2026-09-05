# addon-scheduling-wakes-only-on-acknowledged-readiness

## Discovery trigger

The addon's readiness scheduler bridges an epoll thread and the JavaScript main thread. If a second readiness dispatch could start before the previous callback batch was acknowledged, two callbacks could run for one readiness, or an interrupted wait could surface as a wake.

## Evidence trail

- `dispatchReadiness` (`packages/shm-native/index.ts:515-525`) runs every registered handler and then calls `readinessHandled()` in its `finally`, so the acknowledgement follows the batch.
- The reactor thread (`packages/shm-native/src/scheduling.rs:170-190`) sets `pending` with a compare-exchange before calling the callback and then blocks in `wait_until_handled` until the flag is cleared; a wake that arrives while `pending` is set does not release it.
- `retry_interrupted` (`scheduling.rs:16`) loops on `EINTR` and returns `None` once the closing flag is set.
- `pending_callback_waits_for_acknowledgement` (`scheduling.rs:320`) writes the control eventfd while `pending` is set and asserts the waiter stays blocked for 25 ms, then clears the flag, writes again, and asserts it returns.
- `interrupted_wait_retries_until_success_or_close` (`scheduling.rs:374`) injects one `EINTR` and asserts two attempts and the completed value, then sets closing and asserts `None`.
- `packages/shm-native/tests/mechanism.ts` publishes a frame during a callback and asserts it is observed.

## Failure scenario

A second dispatch started before the first batch is acknowledged runs two callbacks for one readiness; an `EINTR` surfaced as a wake makes the JavaScript side poll an empty ring.

## Timing windows and dependencies

The interval between a readiness write on the control eventfd and the callback clearing the pending flag. The `wait_until_handled` error arm (`scheduling.rs:184-189`) fires one final callback without the pending gate and stops the reactor; it is outside this guarantee, as it is for `reactor-callback-is-one-in-flight`.

## What a test must construct

- Present: the two unit tests above against the scheduler internals.
- Missing: a lost eventfd wake with no frame behind it, and the same ordering driven through the public `watch` surface with several registered channels.

## Investigation log

### Q: Which way round is the ordering?

- Sources examined: `dispatchReadiness`, the reactor loop in `scheduling.rs`.
- Findings: The handler runs first and `readinessHandled()` follows in `finally`; the forbidden order is a second dispatch ahead of that acknowledgement, not a handler ahead of its own.
- Missing evidence: None.
- Conclusion: resolved with answer: the record states the one-in-flight form.
