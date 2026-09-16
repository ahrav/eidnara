# fusion-order-is-deterministic

## Discovery trigger

RP2.7 user story 2 requires the same fused order regardless of which lane
finishes first, and the contract fixes descending score then identifier
bytes with declared results for empty lanes, all-zero weights, and ties.

## Evidence trail

- `fuse` sorts by descending score then ascending occurrence identifier in
  one comparator, so the tie order is stated rather than inherited from the
  union map's iteration order.
- `DeclaredLanes::admit` stores rankings in fixed slots, so admission order
  cannot change the summation order.

## Failure scenario

Two clients issuing the same query against the same projection would see
different orders depending on lane timing.

## Timing windows and dependencies

None. Fusion is a pure function over values.

## What a test must construct

- Shuffled lane lists and doubled hits under a fixed seed; zero weights; an
  exact set; no lanes.

## Investigation log

### Q: What does an all-zero weight set return?

- Sources examined: the RP2.7 contract; `FusionParameters::new`.
- Findings: every score is `+0.0` after negative-zero normalization, so the
  order is identifier order.
- Missing evidence: none.
- Conclusion: resolved with answer - identifier order with zero scores.
