# dense-original-score-matches-f64-reference

## Discovery trigger

RP2.6 numerical contract: the oracle and rescore multiply retained f32
coordinates in f64 and accumulate in coordinate order.

## Evidence trail

- `inner_product` (`crates/retrieval/src/dense/score.rs:13`) widens both
  operands before multiplying and adds to a `+0.0` accumulator.
- `rescore` (`score.rs:194`) validates the query and each row, then scores
  with `score`, which matches `Metric::InnerProduct` to `inner_product`.
- `dense_numerics.rs` `reference_f32` restates the arithmetic.

## Failure scenario

Products formed in f32 tie two distinct rows (`0x3f3504f6, 0x3f3504ef` against
`0x3f3504f3` twice), so the identifier instead of the score orders them.

## Timing windows and dependencies

None.

## What a test must construct

- Rows a few ulps apart, negative-zero coordinates, an opposite row.

## Investigation log

### Q: Can any score be `-0.0`?

- Sources examined: `inner_product`, IEEE 754 round-to-nearest addition.
- Findings: the accumulator starts at `+0.0`, and `+0.0 + -0.0` is `+0.0`, so
  a sum of zero terms is `+0.0`; a nonzero sum carries its own sign.
- Missing evidence: none.
- Conclusion: resolved with answer - no.
