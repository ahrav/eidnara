# fusion-rrf-formula-conformance

## Discovery trigger

RP2.7 KTD2 and parent Q1 fix the arithmetic shape: one-based ranks, zero
for absent lanes, f64 term sum in the declared lane order, and an oracle that
sums terms rather than evaluating a closed fraction.

## Evidence trail

- `crates/retrieval/src/fusion/rrf.rs` `FusionParameters::term` computes
  `weight / (k + position)` and `fuse` folds the terms in `Lane::ORDER`.
- `crates/retrieval/tests/fusion.rs` `oracle_score` sums terms independently;
  a grid over `k` in {7, 13, 31, 60, 97}, three weight sets, and four rank
  triples finds fixtures where a closed fraction and another summation order
  differ bitwise from the term sum.

## Failure scenario

A closed-fraction implementation would round differently and produce
rankings that disagree with the baseline at the last bit, breaking
reproducibility across implementations.

## Timing windows and dependencies

None. Fusion is a pure function over values.

## What a test must construct

- Non-calibration weights and `k`; distinct lane positions per fixture.

## Investigation log

### Q: Is the term-sum oracle distinguishable from the alternatives?

- Sources examined: the grid in the negative-control test.
- Findings: both the closed fraction and the reordered sum differ from the
  term sum for at least one fixture; the assertions would fail otherwise.
- Missing evidence: none.
- Conclusion: resolved with answer - yes.
