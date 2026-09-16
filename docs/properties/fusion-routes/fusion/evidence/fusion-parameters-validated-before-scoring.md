# fusion-parameters-validated-before-scoring

## Discovery trigger

RP2.7 requires finite nonnegative weights and positive finite k validated
before scoring, and parent Q1 asks for negative-zero and infinite-sum
decisions.

## Evidence trail

- `FusionParameters::new` checks each weight and `k`, normalizes `-0.0` with
  `+ 0.0`, and computes the rank-one maximum score to refuse overflow.
- `crates/retrieval/tests/fusion.rs` covers every refusal class.

## Failure scenario

NaN or infinite scores order arbitrarily under `total_cmp`, so an invalid
parameter would silently produce an unreproducible ranking.

## Timing windows and dependencies

None. Fusion is a pure function over values.

## What a test must construct

- NaN, infinite, and negative weights; zero, negative, NaN, infinite `k`;
  `f64::MAX` weights with `k = 0.5`.

## Investigation log

### Q: Why refuse an infinite sum rather than clamp it?

- Sources examined: RP2.7 "invalid parameters fail validation and perform no
  scoring".
- Findings: clamping would be a silent calibration change.
- Missing evidence: none.
- Conclusion: resolved with answer - refuse.
