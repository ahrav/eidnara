# capacity-wake-progress

## Discovery trigger

Both capacity transitions, descriptor acknowledgement and final payload return, wake a producer parked on the capacity doorbell within the bounded `reserve_until` deadline, without incoming data or polling (KTD3). The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/shm-transport/src/backend/ring.rs:1024`
- `crates/shm-transport/src/backend/retained.rs:620`
- `crates/shm-transport/src/backend/ring.rs:75`

Witness status: yes - `crates/shm-transport/src/backend/ring.rs:2497`, `crates/shm-transport/src/backend/ring.rs:2537`, and both two-process tests in crates/shm-transport/tests/ring.rs.

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
