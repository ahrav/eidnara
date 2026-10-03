# dense-candidate-capacity-is-checked-before-allocation

## Discovery trigger

RP2.6: compute `R_dense = ceil(alpha * K_dense)` with finite alpha of at least
one; check product, ceiling, representation, and cap before allocating
R-sized state; zero K does no work.

## Evidence trail

- `CandidateCapacity::new` (`crates/retrieval/src/dense/capacity.rs:37`)
  checks alpha, then zero `K`, then the exact product, then the cap.
- `ceil_product` (`capacity.rs:69`) decomposes alpha into a 53-bit mantissa
  and a binary exponent, multiplies in `u128`, and shifts with rounding up.
- `CandidateCapacity` has private fields, so a pool size exists only after
  every check passed.

## Failure scenario

An f64 product rounds `1.1 * 10` to `11.0` while the exact product exceeds
eleven, and a large `K` overflows a `usize` conversion.

## Timing windows and dependencies

None.

## What a test must construct

- Each refusal class, the precedence between alpha and zero `K`, and the
  `1.1` exact-binary case.

## Investigation log

### Q: Which wins, a malformed alpha or a zero K?

- Sources examined: #578 numerical contract and Q2; #609 acceptance criteria.
- Findings: both outcomes must perform no work; the order is Q2's.
- Missing evidence: owner approval.
- Conclusion: implemented as alpha first; unresolved, needs Q2 owner
  approval.
