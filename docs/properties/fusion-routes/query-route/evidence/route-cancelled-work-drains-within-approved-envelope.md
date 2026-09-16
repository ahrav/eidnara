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
- `crates/daemon/src/query_route.rs` `execute` runs only `exact_read` and
  `lexical::scan` inside `read_under`; `admit_exact`, `lexical::admit`, and
  `judge_eligible` take their kernel readers after the closure returns.
  `crates/daemon/tests/query_route.rs`
  `the_projection_connection_is_free_while_the_lanes_are_admitted` reads the
  projection under a fresh budget from the admission, fusion, and revalidation
  hooks.
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

### Q: Why does the lexical lane no longer judge under the projection connection?

- Sources examined: `retrieval::lexical::retrieve`, which took the connection
  and the kernel together; `storage::SqliteStore`, whose one connection is held
  for the whole callback; `kernel::open::acquire_within`, which polls for one
  of two pooled readers with a 1 ms sleep; the daemon's other judging paths,
  which all close the projection read before judging.
- Findings: with `retrieve` inside `read_under`, every projection reader and
  the lifecycle's `apply_batch` waited on one request's kernel reader
  acquisitions. `lexical::scan` and `lexical::admit` split the two halves so
  the connection is released before the first kernel reader is taken;
  `retrieve` composes them for callers that hold nothing else.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Can the envelope be asserted now?

- Sources examined: the RP2.7 specification's RP2.9 predeclared bounds.
- Findings: the envelope number is not approved.
- Missing evidence: the approved envelope.
- Conclusion: unresolved - needs human input.
