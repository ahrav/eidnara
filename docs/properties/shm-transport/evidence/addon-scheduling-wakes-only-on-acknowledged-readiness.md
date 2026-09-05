# `addon-scheduling-wakes-only-on-acknowledged-readiness`

- **Discovery:** U3, when the addon scheduler was catalogued.
- **Primary evidence:** `dispatchReadiness` (`packages/shm-native/index.ts:515-525`) runs every registered handler and then calls `readinessHandled()` in its `finally`, so the acknowledgement follows the callback batch. `wait_until_handled` (`packages/shm-native/src/scheduling.rs`) blocks on the control eventfd and returns only once the pending flag is cleared, so a wake that arrives while a batch is still unacknowledged does not release the waiter and no second dispatch starts ahead of the acknowledgement. `retry_interrupted` loops on `EINTR` and returns `None` once the closing flag is set, so an interrupted wait is never reported as readiness.
- **Existing evidence:** `pending_callback_waits_for_acknowledgement` writes the control eventfd while the pending flag is set and asserts the waiter stays blocked for 25 ms, then clears the flag, writes again, and asserts the waiter returns. `interrupted_wait_retries_until_success_or_close` injects one `EINTR` and asserts two attempts and the completed value, then sets closing and asserts `None`. `packages/shm-native/tests/mechanism.ts` publishes a frame during a callback and asserts it is observed.
- **Failure scenario:** a second dispatch started before the first batch is acknowledged runs two callbacks for one readiness, or an `EINTR` surfaced as a wake makes the JavaScript side poll an empty ring.
- **Timing window:** the interval between a readiness write on the control eventfd and the callback clearing the pending flag.
- **Instrumentation:** none.
- **Audit verdict (U3):** pass for the ordering condition. No test covers a lost eventfd wake with no frame behind it. Reaching the EOF and `EINTR` situations is recorded separately in `addon-scheduling-reaches-peer-eof-and-interrupted-wait`.
- **Open-question log:** none.
