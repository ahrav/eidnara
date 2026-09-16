# packing-adjustment-removes-last-admitted-within-the-cap

## Discovery trigger

RP2.8's packing contract says wrappers, separators, or escapes may push a
fitting selection over a bound; adjustment must remove lowest-priority optional
groups deterministically within the approved pass cap, reclaim their wrappers,
or return one typed failure without emitting over-budget bytes. The Q4 ruling
fixed the pass unit as a group and the order as last-admitted first.

## Evidence trail

- `crates/daemon/src/packing/serialize.rs` `finalize` loops while `serialize`
  reports a bound, pops the last admitted group into `removed`, and rebuilds
  the ledger through `rebuild` from the required render the admission carries,
  under each remaining group's partition index, so a removed group's `GroupOpen` and
  `GroupClose` entries are not re-appended and the remaining labels match the
  admission's.
- The loop returns `PackingFailure::AdjustmentCapExhausted` when the pass count
  reaches the cap or the admitted list is empty, before constructing any
  `PreparedOutput`.
- `crates/daemon/tests/packing_serialize.rs` checks the removed group's
  partition index, compares the repaired body and ledger with a fresh admission
  of the remaining groups, checks the total dropped by the removed group's
  charge, and compares the body after every optional group is removed with the
  required-only render.

## Failure scenario

An adjustment that trims bytes from the end of the render, or drops the group
with the smallest charge, makes the body depend on the shape of the overflow
and can split a wrapper from its content.

## Timing windows and dependencies

None.

## What a test must construct

- A closed render and a limit one below its length, for both the
  serialized-bytes and estimated-tokens bounds.
- A limit below the required render, so every optional group is removed.
- A cap smaller than the number of passes the limit demands.
