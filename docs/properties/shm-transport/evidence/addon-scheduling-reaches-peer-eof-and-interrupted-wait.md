# `addon-scheduling-reaches-peer-eof-and-interrupted-wait`

- **Discovery:** U3, split from `addon-scheduling-wakes-only-on-acknowledged-readiness` so that the situation-coverage half carries its own `sometimes` semantics instead of riding inside an `always` record.
- **Primary evidence:** `register_setup_socket` (`packages/shm-native/src/scheduling.rs`) adds the setup socket to the reactor with the channel id as event data, so peer closure surfaces as `IN`, `HUP`, or `RDHUP` readiness on that id. `retry_interrupted` treats `EINTR` as a retry and the closing flag as the only other exit.
- **Existing evidence:** `setup_socket_eof_is_reactor_readiness` registers one end of a socket pair, drops the other, and asserts exactly one event for the registered id with a readiness or hang-up flag. `interrupted_wait_retries_until_success_or_close` constructs the `EINTR` retry and the close exit directly.
- **Failure scenario:** a campaign that never reaches either situation passes vacuously while a real peer death or signal leaves the JavaScript side waiting forever.
- **Timing window:** peer exit or signal delivery while a wait is pending.
- **Instrumentation:** none; a campaign marker at each situation would make the `sometimes` check observable.
- **Audit verdict (U3):** both situations are constructed by unit tests against scheduler internals; neither is reached through the public `watch` or `poll` surface.
- **Open-question log:** none.
