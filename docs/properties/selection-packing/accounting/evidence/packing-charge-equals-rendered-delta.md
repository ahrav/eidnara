# packing-charge-equals-rendered-delta

## Discovery trigger

RP2.8's packing and accounting contract charges every admitted item its
rendered delta, including wrappers, separators, and escapes, and forbids a
charge gap; the U4a ticket names the whole-render delta as the oracle.

## Evidence trail

- `crates/daemon/src/packing/render.rs` `Ledger::stage` prices fragments
  against the tail from the ledger's anchor, the last trusted piece start
  `tokenizer::suffix_anchor` finds in its window after skipping the two
  pieces a scan from an arbitrary offset needs to resynchronise, keeping the
  count of that tail so each delta
  tokenizes the tail once, and re-estimates the whole render for a heuristic;
  `commit` appends the staged fragments at the charges they were priced at and
  moves the anchor forward once the tail outgrows `ANCHOR_ADVANCE_BYTES`. Tail
  counts bypass the shared token cache through
  `AccountingProfile::charge_uncached`. Each entry records the fragment's
  bytes and charge, so the entries' bytes sum to the render's length.
- `crates/daemon/src/packing/mod.rs` `prepare_required` charges each required
  fragment through the ledger inside `reserve_required`'s cost closure;
  `prepare_optional` reserves the block close from the remaining budget,
  prices each group by staging `render::group_fragments` (open wrapper, ranges,
  close wrapper) and commits that same staging on admission, then closes the
  block and settles the reserve against the close's charge.
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
- A window opening on the apostrophe of an `Other` run, where the scanner
  emits a contraction the full scan does not have.
- A grouped optional admission whose wrapper entries can be found by kind.
- A profile whose headroom rounds per charge, so a single whole-group price
  and per-entry charges would disagree.
