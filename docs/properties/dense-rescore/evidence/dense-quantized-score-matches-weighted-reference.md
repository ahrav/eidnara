# dense-quantized-score-matches-weighted-reference

## Discovery trigger

RP2.6 numerical contract: the quantized score is the scale-weighted integer
dot, and an unweighted int8 dot is incompatible.

## Evidence trail

- `weighted_dot` (`crates/retrieval/src/dense/scalar.rs:364`) forms
  `f64(s) * f64(s)` and `i32(q) * i32(d)`, converts the product to f64, and
  adds each term to a `+0.0` accumulator in coordinate order.
- `QuantizedQuery::score` (`scalar.rs`) calls `weighted_dot` with the scales
  the query was encoded under.
- `weighted_dot_block` (`scalar.rs:391`) scores eight rows at once. Each lane
  adds `w_j * (f64(c_query_j) * f64(c_doc_j))` to its own `+0.0` accumulator
  in coordinate order. The product of two codes is an exact integer of at most
  16129 in magnitude, so each term equals `weighted_dot`'s.
- `ResolvedCodes::score` (`crates/retrieval/src/dense/candidates.rs`) scores
  the lanes of one layer in a block through one `QuantizedQuery::score_block`
  call under that layer's query, and a layer with one lane in the block
  through `QuantizedQuery::score`.
- `crates/retrieval/tests/dense_numerics.rs` `reference_quantized` restates
  the formula with its own loop.

## Failure scenario

A scorer that drops the weights or rounds the term to f32 ranks a different
pool; the fixture's `coarse` and `fine` rows swap under the unweighted dot.

## Timing windows and dependencies

None.

## What a test must construct

- Unequal scales, code extrema, extreme positive finite scales, and rows whose
  order differs between weighted and unweighted scoring.

## Investigation log

### Q: Is the grouping `(s * s) * product` approved?

- Sources examined: #578 numerical contract, Q2.
- Findings: the contract writes `s_j * s_j * i32(c_query_j) * i32(c_doc_j)`
  and leaves grouping to Q2.
- Missing evidence: owner approval.
- Conclusion: unresolved, needs Q2 owner approval.
