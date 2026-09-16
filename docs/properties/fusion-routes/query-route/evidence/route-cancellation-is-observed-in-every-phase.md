# route-cancellation-is-observed-in-every-phase

## Discovery trigger

The RP2.7 U3b acceptance criteria require the typed terminal for
cancellation and deadline during probe compilation, exact scan, lexical scan,
revalidation, materialization, and response construction.

Repository: `/local/home/ahrav/scratch/eidnara`; base `rp27/u2-weighted-rrf` at
`6dea07f455d536eec50556116c882c8dc6a00d98`; inspected 2026-09-16.

## Evidence trail

- `crates/daemon/src/query_route.rs`: `Phase` enumerates the nine phases;
  `execute` calls `before_phase` then `check(budget)` at each, and
  `judge_eligible` repeats the revalidation gate before every validation slice
  after the first; `check` reads `SharedBudget::is_exhausted` and `exhaustion`
  to pick `Cancelled` or `Deadline`.
- `crates/daemon/tests/query_route.rs`
  `cancellation_and_deadline_are_observed_in_every_phase` and the phase-order
  assertion in `a_healthy_query_completes_fused_in_the_oracles_order`.

## Failure scenario

A phase with no check runs to completion under a dead budget and returns a
ranking or holds the connection past the caller's deadline.

## Timing windows and dependencies

Cancellation between phases; cancellation inside a statement is covered by
the projection read's progress handler.

## What a test must construct

- A populated projection and the `before_phase` hook.
- A token cancelled at the target phase; a 200 ms remaining duration and a
  250 ms sleep at the target phase.

## Investigation log

### Q: Is the hook a production seam?

- Sources examined: `execute` takes `before_phase: impl FnMut(Phase)`; the
  handler passes `|_| {}`.
- Findings: the hook is monomorphized away in production and lets a test
  place a fault exactly before each check.
- Missing evidence: none.
- Conclusion: resolved with answer - the hook stays.
