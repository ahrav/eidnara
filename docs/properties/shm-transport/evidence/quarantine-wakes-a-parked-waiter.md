# quarantine-wakes-a-parked-waiter

## Discovery trigger

The doorbell mechanism pass found three in-crate tests
(`crates/shm-transport/src/backend/ring.rs:3116`, `:3294`, `:3314`) that assert a
quarantine raised while a peer is parked is delivered to that peer, and no record
named the liveness property they pin. The quarantine records in Group A cover
terminality and gating, not delivery.

## Evidence trail

- `quarantine_wakes_a_parked_peer` (`ring.rs:3116`): the attached side calls
  `wait_until(now + 5 s)`; the other side calls `ring.enter_quarantine()`
  (`:1889`); the assertion message reads "quarantine must ring the doorbell a
  parked peer waits on"; afterwards `attached.is_quarantined()` holds.
- `armed_wait_recheck_sees_a_quarantine_that_sent_no_token` (`:3294`):
  `ring.arm_data_wait()` (`:1066`) returns true; quarantine is entered without a
  token being sent; `ring.armed_wait_holds(wake, generation)` (`:1097`) returns
  the quarantine outcome; `(*wake).parked` is `0` afterwards.
- `peer_closing_its_doorbell_quarantines_the_waiting_side` (`:3314`): the
  attached peer is dropped; `ring.wait_for_data(now + 5 s)` (`:1409`) returns a
  quarantine result; `ring.is_quarantined()` holds; `parked` is `0`.
- `signal` is at `:596`, `wait_until` at `:650`. These are the socketpair
  doorbell primitives described in the catalog's doorbell mechanism pass.
- Host usage: `crates/host-runtime/src/ring_transport.rs:426` matches on
  `rings.second.arm_data_wait()`; `:713` calls `.wait_for_data(deadline)`. Both
  are on the shipped receive path, so a parked host waiter is a production
  state.

## Failure scenario

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

- Sources examined: `crates/host-runtime/src/ring_transport.rs:426`, `:713`.
- Findings: the host arms and waits on the ring doorbell on its receive path.
- Missing evidence: none for reachability.
- Conclusion: `default-production`.

### Q: Does the capacity doorbell get the same treatment?

- Sources examined: the three tests; `enter_quarantine` at `:1889` was not read
  line by line for a capacity-side ring.
- Findings: all three tests are data-side.
- Missing evidence: a test or read confirming a producer parked on capacity is
  released by quarantine.
- Conclusion: recorded as the open question.
