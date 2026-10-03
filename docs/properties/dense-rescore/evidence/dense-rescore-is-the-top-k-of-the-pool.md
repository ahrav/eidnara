# dense-rescore-is-the-top-k-of-the-pool

## Discovery trigger

#613 AC2: for accepted set `A`, output is exactly `Top(K_dense, A, f32_score)`.

## Evidence trail

- `rescore_pool` (`crates/retrieval/src/dense/candidates.rs:340`) validates
  the query and each row, scores with `score`, sorts with `rank_order`, and
  truncates to `k`.

## Failure scenario

A rescore that keeps quantized order, or ranks by payload, returns a ranking
other than the exact f32 order of the pool.

## Timing windows and dependencies

None.

## What a test must construct

- Negative scores, ties, and an underfilled pool over real generations.

## Investigation log

### Q: Must the result equal the exhaustive f32 top K?

- Sources examined: #613 AC2, #578 numerical contract.
- Findings: no; rescore cannot recover a neighbor the pool omitted.
- Missing evidence: none.
- Conclusion: resolved with answer - no, only over `A`.
