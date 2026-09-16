# route-cancelled-work-drains-within-approved-envelope

## Discovery trigger

The RP2.7 specification separates logical cancellation from physical
completion and requires a finite drain within an approved envelope, with a
permanently blocked read visible as unresolved work.

Repository: `/local/home/ahrav/scratch/eidnara`; base `rp27/u2-weighted-rrf` at
`6dea07f455d536eec50556116c882c8dc6a00d98`; inspected 2026-09-16.

## Evidence trail

- `crates/daemon/src/query_route.rs` `handle_retrieval_query`: the budget
  guard is alive across `work.await` and dropped after the join; the closure
  owns the `SearchReader` pin for its whole run.
- `crates/daemon/src/request_budget/host_tests.rs`
  `cancelling_a_suspended_handler_interrupts_the_held_read_and_joins_it_before_settling`
  witnesses join-before-settle on the bridge.
- No envelope bound and no holder census exist for the route.

## Failure scenario

The request settles while a lane still holds the connection or the pin, or a
wedged read drains for an unbounded time without being visible.

## Timing windows and dependencies

Cancellation while the lanes hold the connection; cancellation while the
closure waits to pin.

## What a test must construct

- A real host and client, a held read, a pinned family, and an approved
  envelope.
- A census of the guard, the pin, and the connection.

## Investigation log

### Q: Can the envelope be asserted now?

- Sources examined: the RP2.7 specification's RP2.9 predeclared bounds.
- Findings: the envelope number is not approved.
- Missing evidence: the approved envelope.
- Conclusion: unresolved - needs human input.
