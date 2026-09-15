# capacity-wake-progress

## Discovery trigger

Both capacity transitions, descriptor acknowledgement and final payload return, wake a producer parked on the capacity doorbell within the bounded `reserve_until` deadline, without incoming data or polling (KTD3). The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/shm-transport/src/backend/ring.rs:1028`
- `crates/shm-transport/src/backend/retained.rs:625`
- `crates/shm-transport/src/backend/ring.rs:75`
- `crates/host-runtime/src/client.rs:2675`
- `packages/shm-native/src/lib.rs:1529`
- `packages/opencode-plugin/src/shared/host-client/shm-frame-channel.ts:379`

Witness status: yes - `crates/shm-transport/src/backend/ring.rs:2525`, `crates/shm-transport/src/backend/ring.rs:2565`, and both two-process tests in crates/shm-transport/tests/ring.rs; at the client, `crates/host-runtime/src/client.rs:7564` parks the managed bridge on the capacity doorbell with ordinary headroom exhausted and shows a host consumption alone, with no inbound data or timer, admits the blocked frame; `shared_memory_workers_have_no_periodic_polling` in crates/host-runtime/src/ring_transport.rs pins that the bridge has no reservation slice. At the native addon, `an armed capacity wait wakes the readiness callback on the peer's consumption or return alone` in packages/shm-native/tests/mechanism.ts arms `arm_capacity` (`packages/shm-native/src/lib.rs:1529`) with ordinary headroom exhausted and shows one peer consumption, and later one lease return, each delivering exactly one readiness wake through the reactor's capacity doorbell registration (`packages/shm-native/src/scheduling.rs:349`), with no replay for a park nobody holds. At the TypeScript channel, `an arm that finds capacity already visible retries the queued head at once` and `a park is rechecked after arming so a return before the arm is not lost` in packages/opencode-plugin/src/shared/host-client/shm-frame-channel.test.ts drive `pumpPending` (`packages/opencode-plugin/src/shared/host-client/shm-frame-channel.ts:333`) through both arm outcomes against a mock addon and show the queued head publishing with no readiness callback; `a full ring queues the frame in order, holds its charge, and publishes on capacity readiness` pins that a frame queued behind a parked head does not re-arm.

## Failure scenario

A lost wake leaves the producer parked to its deadline although capacity exists.

## Timing windows and dependencies

Return before arm, return after arm, and a coalesced token covering both transitions.

## What a test must construct

Producer parked (`parked != 0`) when the transition happens; a transition landing between `try_reserve` and `ParkGuard::arm`.

Situation markers that must fire independently of the safety check:

- `pool.producer_parked_on_capacity`
- `pool.transition_during_arm_window`

Check semantics: `always` - a `reserve_until` parked on exhaustion returns `Ok` before its deadline once either transition happens, with `parks >= 1` in `syscall_counters`; bounded by the test deadline, never an open-ended eventually.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.
