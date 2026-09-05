# quarantine-wakes-a-parked-waiter

## Discovery trigger

The doorbell mechanism pass found three in-crate tests
(`crates/shm-transport/src/backend/ring.rs:3191`, `:3347`, `:3369`) that assert a
quarantine raised while a peer is parked is delivered to that peer, and no record
named the liveness property they pin. The quarantine records in Group A cover
terminality and gating, not delivery.

## Evidence trail

- `quarantine_wakes_a_parked_peer` (`ring.rs:3191`): the attached side calls
  `wait_until(now + 5 s)`; the other side calls `ring.enter_quarantine()`
  (`:1915`); the assertion message reads "quarantine must ring the doorbell a
  parked peer waits on"; afterwards `attached.is_quarantined()` holds.
- `armed_wait_recheck_sees_a_quarantine_that_sent_no_token` (`:3347`):
  `ring.arm_data_wait()` (`:1187`) returns true; quarantine is entered without a
  token being sent; `ring.armed_wait_holds(wake, generation)` (`:1227`) returns
  the quarantine outcome; `(*wake).parked` is `0` afterwards.
- `peer_closing_its_doorbell_quarantines_the_waiting_side` (`:3369`): the
  attached peer is dropped; `ring.wait_for_data(now + 5 s)` (`:1476`) returns a
  quarantine result; `ring.is_quarantined()` holds; `parked` is `0`.
- `signal` is at `:783`, `wait_until` at `:818`. These are the socketpair
  doorbell primitives described in the catalog's doorbell mechanism pass.
- Host usage: `crates/host-runtime/src/ring_transport.rs:566` matches on
  `rings.second.arm_data_wait()`; `:1026` calls `.wait_for_data(deadline)`. Both
  are on the shipped receive path, so a parked host waiter is a production
  state.
  At HEAD: RingClientEndpoint::recv is cfg(test)-only at HEAD, so the shipped host parks through arm_data_wait plus an AsyncFd readiness select! and never calls wait_for_data.
  At HEAD: armed_wait_holds takes a shared WakeEpoch reference and is a pure predicate; it no longer unparks on any path because the caller's ParkGuard owns that.
  At HEAD: arm_data_wait delegates to arm_data_wait_guarded (`:1201`), and the test calls the guarded form directly so the ParkGuard stays alive.
  At HEAD: The test arms through arm_data_wait_guarded, asserts armed_wait_holds leaves parked nonzero, and only the guard's drop clears it to 0.

## Failure scenario

The scenario below was derived against the source tree this record was written from; where the investigation log's post-merge entry records a changed mechanism, the sentences marked "At HEAD" above and that entry carry the current behavior, and the scenario reads as the regression this record guards against.

A quarantine entry point that sets the quarantine flag without ringing the
doorbell, reached after the waiter's armed re-check has passed, leaves the
waiter asleep until its deadline. The host then reports a timeout for what was a
peer death or verification failure, and the parked flag may remain set so a
later wake is swallowed.

## Timing windows and dependencies

The window is between `arm_data_wait` and the blocking wait: a quarantine there
has no token to deliver, and the armed re-check is what closes it. After the
park, delivery depends on `enter_quarantine` ringing the doorbell and on the
kernel reporting the peer's closed socket end to `wait_until`.

## What a test must construct

A peer parked in `wait_until` or `wait_for_data`, then each of: quarantine
entered on the other side; quarantine entered between arm and park with no
token; the peer's doorbell end closed. Assert return before the deadline, the
quarantine result, and `parked == 0`.

## Investigation log

### Q: Is the parked waiter reachable in the shipped host?

- Sources examined: `crates/host-runtime/src/ring_transport.rs:566`, `:1026`.
- Findings: the host arms and waits on the ring doorbell on its receive path.
- Missing evidence: none for reachability.
- Conclusion: `default-production`.
  At HEAD: Only the cfg(test) peer helper calls wait_for_data, so reachability in the shipped host rests on arm_data_wait and the AsyncFd readiness arm alone.

### Q: Does the capacity doorbell get the same treatment?

- Sources examined: the three tests; `enter_quarantine` at `ring.rs:1915` was not read
  line by line for a capacity-side ring.
- Findings: all three tests are data-side.
- Missing evidence: a test or read confirming a producer parked on capacity is
  released by quarantine.
- Conclusion: recorded as the open question.

### Q: Do the three tests exercise what the record first said they did?

- Sources examined: `ring.rs:3191-3205`, `:3347-3366`, `:3369-3379`, `:1915-1922`,
  `:1490-1493`.
- Findings: `quarantine_wakes_a_parked_peer` writes `parked = 1` by hand at
  `:3195`, quarantines at `:3196`, and only then waits (`:3198-3201`), so it
  proves a token is left for a later waiter, not that a sleeping waiter was
  released; it does not assert `parked == 0`. The peer-closing test asserts
  `Err(RingError::DoorbellFailed)` (`:3373-3376`), which `wait_for_data`
  returns through `quarantine_with` (`:1490-1493`) after quarantining.
  `enter_quarantine` rings both doorbells (`:1920-1921`).
- Missing evidence: a test that parks first and quarantines second on the data
  side; a test of a capacity-parked producer released by a peer-side
  `enter_quarantine` (`:3381-3394` covers the peer-drop case only).
- Conclusion: Exercised is partial; the Check names `DoorbellFailed` for the
  third event; the capacity-side question is answered by code and only the test
  is missing.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 17, `:3294` now `:3347`: The test arms through arm_data_wait_guarded, asserts armed_wait_holds leaves parked nonzero, and only the guard's drop clears it to 0.
  - line 18, `:1066` now `:1187`: arm_data_wait delegates to arm_data_wait_guarded (`:1201`), and the test calls the guarded form directly so the ParkGuard stays alive.
  - line 19, `:1097` now `:1227`: armed_wait_holds takes a shared WakeEpoch reference and is a pure predicate; it no longer unparks on any path because the caller's ParkGuard owns that.
  - line 27, `:713` now `:1026`: RingClientEndpoint::recv is cfg(test)-only at HEAD, so the shipped host parks through arm_data_wait plus an AsyncFd readiness select! and never calls wait_for_data.
  - line 57, `:713` now `:1026`: Only the cfg(test) peer helper calls wait_for_data, so reachability in the shipped host rests on arm_data_wait and the AsyncFd readiness arm alone.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
