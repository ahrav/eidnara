# capacity-wait-unparks-on-exit-and-survives-a-stale-token

## Discovery trigger

Round 24 review of the PR: two tests pin the producer side of the wake-marker
protocol, the mirror of the consumer-side record added the round before, and no
record owned them.

## Evidence trail

- `crates/shm-transport/src/backend/ring.rs:1111-1162`: `reserve_until_in`
  loops over `try_reserve_in`; each iteration arms the capacity wake with
  `ParkGuard::arm` (`:1131`), re-checks, compares the wake generation
  (`:1137`), drains the doorbell and re-checks (`:1141-1146`), compares the
  generation again (`:1148`), and only then blocks in `wait_until` (`:1153`),
  draining once more after a wake (`:1159`). At HEAD this path is reached by
  the native addon (`packages/shm-native/src/lib.rs:1046`) and the test-only
  `RingClientEndpoint::send` (`crates/host-runtime/src/ring_transport.rs:1520`).
- `ring.rs:903-920`: `arm_capacity_wait`, the path the Rust host and client
  bridge use, arms (`:908`), re-checks the blocked reservation and the
  generation (`:909`), drains and re-checks again (`:913-915`), and forgets
  its guard on `Ok(true)` (`:918`); `complete_capacity_wait` (`:974`) clears
  `parked`, and every caller runs it on exit: `ring_transport.rs:930`, `:1098`,
  `:1237`; `client.rs:2731`, `:2797`, and `:2803` before the bridge thread ends.
- `ring.rs:108-110`: `ParkGuard`'s `Drop` stores zero into `parked`; the
  comment at `:1130` states it runs on every exit from the iteration.
- `ring.rs:99-105`: `ParkGuard::arm` stores the incremented generation into
  `parked`, so a marker is non-zero rather than one.
- Tests: `arm_capacity_wait_refuses_to_park_over_a_return_that_landed_before_arming`
  (`ring.rs:2921`) and `ring_bridge_blocked_write_expires_at_its_deadline_without_publishing`
  (`client.rs:7728-7826`, host publish after the bridge's exit at `:7824`).

## Failure scenario

A refactor holds the `ParkGuard` across iterations or returns from one without
dropping it. The producer times out with `parked` still set; the next consumer
release swaps the marker and sends a byte nobody waits for; a later genuine park
drains that stale byte, and if the re-check after the drain were also removed,
waits on an empty doorbell for a release that already happened until its
deadline expires.

## Timing windows and dependencies

The stale-token window is between a spurious signal and the next park; the
two-thread test opens it with a 100 ms sleep on the consumer thread and a 10 s
producer deadline bounded to 5 s.

## What a test must construct

A full ring and an expiring deadline, then a publish from the peer that must
not fail: present for the client bridge's exit (`client.rs:7728-7826`). A
return before the arm leaving `parked` zero: present (`ring.rs:2921`). Missing: a
`reserve_until_in` deadline exit asserting `parked == 0` directly, a queued
token plus a release from another thread asserting bounded return, the
error-exit arm of the guard, and an instruction-scale interleaving.

## Investigation log

### Q: Does every exit from a reserve_until iteration clear the marker?

- Sources examined: `reserve_until`, `ParkGuard`.
- Findings: the guard is a local of the loop body, so `Drop` runs on the
  `return`, the `continue`, and every `?`; no path holds it across
  iterations.
- Missing evidence: a test for the `?` exits.
- Conclusion: the guarantee holds by construction for every exit and is tested
  for the deadline exit.

### Q: What did the #552 re-read find at HEAD?

- Sources examined: `ring.rs:99-110`, `:903-920`, `:974`, `:1111-1162`; every
  `complete_capacity_wait` caller in `ring_transport.rs` and `client.rs`.
- Findings: the two tests this record named,
  `reserve_until_deadline_leaves_the_capacity_wake_unparked` and
  `stale_capacity_token_after_a_drain_does_not_deadlock_the_next_park`, do not
  exist in the tree. The Rust host and client no longer block in
  `reserve_until`; they arm through `arm_capacity_wait` and clear `parked` with
  `complete_capacity_wait`, so the guarantee gained a second mechanism whose
  hazard is a caller exit without that call. The bridge clears both `parked`
  markers before its thread ends (`client.rs:2803-2804`).
- Missing evidence: the stale-token half has no test.
- Conclusion: confidence lowered to medium; exercised is partial.
