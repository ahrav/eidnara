# packing-charge-equals-rendered-delta

## Discovery trigger

RP2.8's packing and accounting contract charges every admitted item its
rendered delta, including wrappers, separators, and escapes, and forbids a
charge gap; the U4a ticket names the whole-render delta as the oracle.

## Evidence trail

- `crates/daemon/src/packing/render.rs` `Ledger::delta` estimates the render's
  tail from a piece boundary (`tokenizer::suffix_anchor`) with and without the
  fragment and charges the difference for the exact profile, and re-estimates
  the whole render for a heuristic; `append` records the fragment's bytes and
  charge as one entry, so the entries' bytes sum to the render's length.
- `crates/daemon/src/packing/mod.rs` `prepare_required` charges each required
  fragment through the ledger inside `reserve_required`'s cost closure;
  `prepare_optional` prices a group by the delta of its whole fragment and,
  once admitted, appends its open wrapper, ranges, and close wrapper as
  separate entries.
- `crates/daemon/tests/packing_accounting.rs` recomputes each delta over the
  whole prefix with no anchor and compares the sum of charges to the estimate
  of the final text, under the exact tokenizer, a linear byte profile, and a
  non-linear heuristic, with some renders longer than the lookback and with a
  same-class tail run longer than the lookback.

## Failure scenario

A charge that counts payload bytes but not the wrapper around them leaves the
wrapper bytes uncharged; the serialized body then exceeds the budget the
caller trusted by exactly the sum of the wrappers.

## Timing windows and dependencies

None.

## What a test must construct

- Generated fragment sequences long enough to exceed the lookback, including
  non-ASCII and XML-significant bytes, and a same-class run longer than it.
- A whole-prefix oracle independent of the anchored production path.
- A grouped optional admission whose wrapper entries can be found by kind.
