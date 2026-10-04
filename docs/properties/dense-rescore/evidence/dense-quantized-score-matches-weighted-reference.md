# dense-quantized-score-matches-weighted-reference

## Discovery trigger

RP2.6 numerical contract: the quantized score is the scale-weighted integer
dot, and an unweighted int8 dot is incompatible.

## Evidence trail

- `weighted_dot` (`crates/retrieval/src/dense/scalar.rs:468`) forms
  `f64(s) * f64(s)` and `i32(q) * i32(d)`, converts the product to f64, and
  adds each term to a `+0.0` accumulator in coordinate order.
- `QuantizedQuery::score` (`scalar.rs`) calls `weighted_dot` with the scales
  the query was encoded under.
- `QuantizedQuery::term_table` (`scalar.rs:341`) forms, for every coordinate
  and every code byte, the weight times the integer product, which is exact in
  f64. Adding `+0.0` turns a `-0.0` zero product into the `+0.0` that
  `weighted_dot` widens, so every entry carries the term's bits.
- `TermTable::score_rows` (`scalar.rs:405`) starts every row at `+0.0` and adds
  that row's entries in increasing coordinate order. It advances 512 rows
  through one 16-coordinate chunk at a time, which changes the order rows are
  visited and leaves each row's own order unchanged.
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
- For the term-table scan, dimensions that leave a partial chunk and row counts
  that leave a partial tile, with rows that start one byte into their buffer.

## Investigation log

### Q: Is the grouping `(s * s) * product` approved?

- Sources examined: #578 numerical contract, Q2.
- Findings: the contract writes `s_j * s_j * i32(c_query_j) * i32(c_doc_j)`
  and leaves grouping to Q2.
- Missing evidence: owner approval.
- Conclusion: unresolved, needs Q2 owner approval.
