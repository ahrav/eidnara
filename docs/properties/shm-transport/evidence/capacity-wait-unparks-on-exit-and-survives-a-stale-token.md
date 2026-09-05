# capacity-wait-unparks-on-exit-and-survives-a-stale-token

## Discovery trigger

Round 24 review of the PR: two tests pin the producer side of the wake-marker
protocol, the mirror of the consumer-side record added the round before, and no
record owned them.

## Evidence trail

- `crates/shm-transport/src/backend/ring.rs:1345-1390`: `reserve_until` loops
  over `try_reserve`; each iteration arms the capacity wake with
  `ParkGuard::arm` (`:1359`), re-checks, compares the wake generation
  (`:1364-1366`), drains the doorbell and re-checks (`:1368-1375`), compares
  the generation again (`:1376-1378`), and only then blocks in `wait_until`
  (`:1379-1384`), draining once more after a wake (`:1385-1388`).
- `ring.rs:653-657`: `ParkGuard`'s `Drop` stores zero into `parked`; the
  comment at `:1358` states it runs on every exit from the iteration.
- `ring.rs:645-650`: `ParkGuard::arm` stores the incremented generation into
  `parked`, so a marker is non-zero rather than one.
- Tests: `reserve_until_deadline_leaves_the_capacity_wake_unparked`
  (`:4189-4210`) and `stale_capacity_token_after_a_drain_does_not_deadlock_the_next_park`
  (`:4213-4240`), the latter with a spawned consumer thread.

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

A full ring and an expiring deadline, asserting `parked == 0` afterwards:
present. A queued token plus a release from another thread, asserting bounded
return: present at the mid-block scale. Missing: the error-exit arm of the guard
and an instruction-scale interleaving.

## Investigation log

### Q: Does every exit from a reserve_until iteration clear the marker?

- Sources examined: `reserve_until`, `ParkGuard`.
- Findings: the guard is a local of the loop body, so `Drop` runs on the
  `return`, the `continue`, and every `?`; no path holds it across
  iterations.
- Missing evidence: a test for the `?` exits.
- Conclusion: the guarantee holds by construction for every exit and is tested
  for the deadline exit.
