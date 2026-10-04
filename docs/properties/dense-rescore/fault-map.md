# Dense numerical fault and enabling-state map

System: `/local/home/ahrav/scratch/eidnara`. Base: `cc5898c3e`. Every seam
below is a pure function or a value constructor in
`crates/retrieval/src/dense/`; no fault injection framework is needed for the
U1 records. Each row names the fault or state a record's `Required faults and
enabling state` line calls for, the seam that reaches it, and the record.

## Fault classes and their availability

| Fault or state | Available seam | Records |
| --- | --- | --- |
| Scales two orders of magnitude apart across coordinates | `Scales::from_values` with a hand-chosen vector; `calibrate` over rows whose coordinates differ in magnitude. | dense-quantized-score-matches-weighted-reference |
| Codes at `+127` and `-127`; scales at `f32::MAX`, `f32::MIN_POSITIVE`, and the smallest positive subnormal | `Scales::from_values` admits any positive finite f32; codes are plain `i8` slices handed to `weighted_dot` or `TermTable::score_rows`. | dense-quantized-score-matches-weighted-reference |
| A dimension that leaves a partial 16-coordinate chunk; a row count that leaves a partial 512-row tile | `RowLayout { dimension }` for dimensions `1..=40`, `384`, `385`; a codes slice of `0`, `1`, `2`, `17`, `511`, `512`, `513`, or `1100` whole rows handed to `score_rows`. | dense-quantized-score-matches-weighted-reference |
| An unweighted integer dot or an f32-rounded term in place of the contract | Negative controls written in the test (`unweighted`, `f32_first_quantized`, `f32_first_original` in `crates/retrieval/tests/dense_numerics.rs`) compared against production bits over the same fixture. | dense-quantized-score-matches-weighted-reference, dense-original-score-matches-f64-reference |
| A fused multiply-add | `f64::mul_add` over the same terms as the production sum, compared to the production bits. | dense-quantized-score-matches-weighted-reference (existing check `dense_scalar.rs:314`) |
| Rows one or a few ulps apart in two coordinates | Original-f32 rows built with `f32::from_bits` and `next_up`, handed to `rescore` or `inner_product`. | dense-original-score-matches-f64-reference |
| Negative-zero coordinates; a row opposite the query | Original-f32 rows containing `-0.0`; a row equal to the negated query. | dense-original-score-matches-f64-reference, dense-order-is-total-and-keeps-distinct-identities |
| Two identifiers with identical codes or vectors; a cut inside the tie | Fixture rows with equal payload under distinct identifier strings; `k` chosen at every value from one to the fixture size. | dense-order-is-total-and-keeps-distinct-identities |
| A negative score and a zero score at the cut | A document code vector that is all zeros, and one whose weighted products against the fixture query sum negative, scored through `QuantizedQuery::score` and ordered with `rank_order`. | dense-order-is-total-and-keeps-distinct-identities |
| Arbitrary offer order into `TopK` | Seeded proptest over permutations (existing check `dense_properties.rs:53`). | dense-order-is-total-and-keeps-distinct-identities |
| Alpha `NaN`, `+inf`, `-inf`, `0.999`, `-0.0`, `f64::MAX` | `CandidatePolicy { alpha, cap }` handed to `CandidateCapacity::new`. | dense-candidate-capacity-is-checked-before-allocation |
| `K = 0` under a valid alpha; `K = 0` under a malformed alpha | `CandidateCapacity::new(0, policy)`. | dense-candidate-capacity-is-checked-before-allocation |
| `K = usize::MAX`; a product whose shift overflows `u128` | `CandidateCapacity::new(usize::MAX, policy)`; alpha `2^53` with `K = 2^11`, alpha `2^100 (1 + epsilon)` with `K = 2^40`, and alpha `2^76` with `K = 2^52`, whose unchecked shift would wrap `2^128` to zero. | dense-candidate-capacity-is-checked-before-allocation |
| A pool one above the cap; a pool equal to the cap | `CandidatePolicy { cap }` set to `ceil(alpha * K) - 1` and to `ceil(alpha * K)`. | dense-candidate-capacity-is-checked-before-allocation |
| `alpha = 1.1` with `K = 10` (exact binary alpha rounds up to 12); integer and dyadic alphas against an integer ceiling | `CandidateCapacity::new` against `checked_mul` and an integer ceiling written in the test; two seeded ChaCha runners (2048 cases each) over integer alphas in `1..=2^53` and dyadic alphas, with `K` drawn from `1..=4096` or any `usize`. | dense-candidate-capacity-is-checked-before-allocation |
| A query of the wrong dimension, a non-finite coordinate, zero norm, a norm outside tolerance, a layout without a valid tolerance | `QuantizedQuery::new` and `rescore`, each returning a typed `QueryRefusal` or `RowRejection`. | dense-invalid-query-never-enters-scoring |
| Scales of another dimension | `QuantizedQuery::new` with `Scales` built for a different dimension, refused as `QueryRefusal::ScalesDimension`. | dense-invalid-query-never-enters-scoring |
| A query whose every code is zero | Scales of one on every coordinate with a query of four halves under a 4-dimensional layout, refused as `QueryRefusal::ZeroCodes`. | dense-invalid-query-never-enters-scoring |
| A codes slice that is not a whole number of rows; a reserved `-128` code | `TermTable::score_rows` with a slice length that is not a multiple of the dimension; a slice holding `-128` under debug assertions. | dense-quantized-score-matches-weighted-reference |

## Faults with no seam at this base

| Fault or state | Why it is unreachable here | Record that would need it |
| --- | --- | --- |
| A producer that sizes an R-sized pool from anything other than `CandidateCapacity` | No production caller allocates a quantized pool at this base; the private fields of `CandidateCapacity` make it the only checked source, and #610's scan supplies the allocation witness. | dense-candidate-capacity-is-checked-before-allocation |
| A scan that scores codes through a vectorized path | U5 owns the vectorized path; `TermTable::score_rows` is scalar at this base. | dense-quantized-score-matches-weighted-reference (U5 fixtures) |
| A decoded `Scales` whose resident bytes exceed the ledger's `LayerTables` charge | `vector_generation::resident_bytes` is the only charge and the census check compares it to itself (`existing-checks.md`, resource accounting); a charge-versus-allocation witness needs a `Scales` size seam that does not exist. | none yet; see `portfolio-evaluation.md` |

## Ranking by cheapest valid oracle

1. Bit-equality against a reference written from the formula (`always`
   records, pure functions, no fixtures beyond slices): every U1 record uses
   it, and it needs no harness.
2. Negative controls that recompute the fixture under a rejected arithmetic
   policy and require at least one differing bit: cheap, and they discriminate
   the policy choices Q2 freezes.
3. Seeded generators over code and alpha ranges: deterministic, but one seed
   explores one sample per revision of the test; raising the case count or
   adding an unseeded job is a cost decision recorded in
   `portfolio-evaluation.md`.
