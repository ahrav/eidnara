# packing-optional-bounds-refuse-at-limit-plus-one

## Discovery trigger

RP2.8 acceptance row AC8 requires each bound to saturate at its approved value
and refuse at value plus one with a typed reason; the U3 ticket assigns the
fused-candidates, parents, spans-per-parent, payload-loads, payload-bytes, and
per-item-maximum bounds to the optional phase.

## Evidence trail

- `crates/retrieval/src/packing/scan.rs` `admit_optional_set` checks every
  bound in row order before any byte is loaded and returns `BoundExceeded`
  with the bound and the crossing position.
- `crates/daemon/src/packing/mod.rs` `prepare_optional` calls it after eligibility
  and before the load hold, mapping the refusal to
  `PreparationRefusal::OptionalBound`.
- `crates/retrieval/tests/packing_grouping.rs` admits a four-span set at the
  exact limits and refuses each bound tightened by one; the daemon test shows
  the refusal leaving `payload_loads()` at the required count.

## Failure scenario

An unbounded optional set loads every candidate payload before the budget
scan and exhausts memory or the deadline.

## Timing windows and dependencies

None.

## What a test must construct

- A set sized exactly to every bound, then each bound reduced by one alone.
