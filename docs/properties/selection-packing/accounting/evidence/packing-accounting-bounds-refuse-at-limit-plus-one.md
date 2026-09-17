# packing-accounting-bounds-refuse-at-limit-plus-one

## Discovery trigger

RP2.8 acceptance row AC8 requires each bound to saturate at its approved value
and refuse at value plus one; the U4a ticket assigns the rendered-bytes and
estimated-tokens bounds to accounting.

## Evidence trail

- `crates/daemon/src/packing/render.rs` `admit_render` compares the ledger's
  rendered bytes and total tokens to `AccountingBounds` and returns
  `AccountingExceeded` with the bound, value, and limit.
- `crates/daemon/src/packing/mod.rs` calls it after the required reservation
  and after the optional ledger closes, mapping the refusal to
  `PreparationRefusal::Accounting`.
- `crates/daemon/tests/packing_accounting.rs` sets both bounds from a closed
  ledger, then reduces each by one, under each profile and through the required
  entry.

## Failure scenario

A render one byte or one token over the limit is emitted as if it fit.

## Timing windows and dependencies

None.

## What a test must construct

- A closed ledger and bounds derived from its own size.
