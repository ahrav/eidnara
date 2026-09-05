# addon-scheduling-reaches-peer-eof-and-interrupted-wait

## Discovery trigger

Split from `addon-scheduling-wakes-only-on-acknowledged-readiness` so the situation-coverage half carries its own `sometimes` semantics instead of riding inside an `always` record, where a campaign that never reached EOF or `EINTR` would pass vacuously.

## Evidence trail

- `register_setup_socket` (`packages/shm-native/src/scheduling.rs:63`) adds the setup socket to the reactor with the channel id as event data, so peer closure surfaces as `IN`, `HUP`, or `RDHUP` readiness on that id.
- `retry_interrupted` (`scheduling.rs:16`) treats `EINTR` as a retry and the closing flag as the only other exit.
- `setup_socket_eof_is_reactor_readiness` (`scheduling.rs:432`) registers one end of a socket pair, drops the other, and asserts exactly one event for the registered id with a readiness or hang-up flag.
- `interrupted_wait_retries_until_success_or_close` (`scheduling.rs:473`) constructs the `EINTR` retry and the close exit directly.

## Failure scenario

A campaign that never reaches either situation passes while a real peer death or signal leaves the JavaScript side waiting forever.

## Timing windows and dependencies

Peer exit or signal delivery while a wait is pending.

## What a test must construct

- Present: both situations at unit level against scheduler internals.
- Missing: either situation reached through the public `watch` or `poll` surface, and a campaign marker at each situation so the `sometimes` check is observable.

## Investigation log

### Q: Are both situations reachable through the public surface?

- Sources examined: `scheduling.rs` unit tests, `packages/shm-native/tests/mechanism.ts`.
- Findings: Only the internal tests construct them; `mechanism.ts` does not kill a peer or interrupt a wait.
- Missing evidence: A public-surface test for each.
- Conclusion: unresolved, needs public-surface coverage.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 9, `packages/shm-native/src/scheduling.rs:31` now `packages/shm-native/src/scheduling.rs:63`: The registration takes an event_data: u64 that callers derive with setup_event(channel_id) (`:40-42`) rather than the raw channel id, and it adds EventFlags::ONESHOT beside IN, HUP, ERR, and RDHUP (`:71-81`).
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
