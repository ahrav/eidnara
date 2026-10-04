# RP2.6 dense candidate and rescore properties

## Scope and provenance

System: `/local/home/ahrav/scratch/eidnara`.
Base: `cc5898c3e` (the `m6/897-mapped-int8-dense` commit the U1 change was
authored against). Method: `../METHOD.md` and
`property-discovery-and-catalog`.

Source: the RP2.6 specification
([#578](https://github.com/ahrav/eidnara/issues/578)). Its local companion
bundle of 23 proposed records is unavailable in this repository, so each
implementation ticket reconstructs the records its change makes executable
from the specification's T1 to T7 seams and verifies them against the code
that implements them. The RP2.6.U1 ticket
([#609](https://github.com/ahrav/eidnara/issues/609)) lands the numerical
records below.

This part owns the dense numerical contract, the quantized candidate pool, the
retained-f32 rescore, and their resource and cancellation obligations. Fusion,
final-use authorization, and packing are separate parts.

Parent Q2 decisions recorded here as implemented and pending owner approval:
arithmetic is unfused f64 in increasing coordinate order (Rust never contracts
`a * b + c`); every score starts from `+0.0`, so a sum of zero terms is `+0.0`
and no score is `-0.0`; scores are `f64`; alpha is an `f64` whose exact binary
value is multiplied by `K` and rounded up, so `alpha = 1.1` and `K = 10` pool 12
candidates; a malformed alpha refuses before a zero `K` is considered; a zero
`K` under a valid alpha yields no capacity and no work; a query whose every
code is zero under a layer's scales is refused with `QueryRefusal::ZeroCodes`.

## Observation contract

The observation point is the public API of `crates/retrieval/src/dense/`,
exported at `crates/retrieval/src/lib.rs` and exercised by
`crates/retrieval/tests/dense_numerics.rs`, `dense_scalar.rs`, and
`dense_properties.rs`. The references in `dense_numerics.rs` are written from
the formulas and call no scorer or comparator of `retrieval::dense`. The
fixtures exercise the scalar scorers. The term-table scan
(`TermTable::score_rows`) is scalar code with its own chunk-tail, tile-boundary,
alignment, and extrema fixture; a vectorized path (U5) needs its own fixtures,
which are not measured here.

## Index

| Slug | Type | Reachability | Semantics | Status | Confidence |
| --- | --- | --- | --- | --- | --- |
| [dense-quantized-score-matches-weighted-reference](#dense-quantized-score-matches-weighted-reference) | safety | test-only | always | active | high |
| [dense-original-score-matches-f64-reference](#dense-original-score-matches-f64-reference) | safety | default-production | always | active | high |
| [dense-order-is-total-and-keeps-distinct-identities](#dense-order-is-total-and-keeps-distinct-identities) | safety | default-production | always | active | high |
| [dense-candidate-capacity-is-checked-before-allocation](#dense-candidate-capacity-is-checked-before-allocation) | safety | test-only | always | active | high |
| [dense-invalid-query-never-enters-scoring](#dense-invalid-query-never-enters-scoring) | safety | test-only | always | active | high |

## Records

### dense-quantized-score-matches-weighted-reference

Type: safety
Reachability: test-only - `QuantizedQuery` (`crates/retrieval/src/dense/scalar.rs:293`)
and `TermTable::score_rows` (`scalar.rs:405`) are called from
`crates/retrieval/tests/` only at this base; no producer scans codes yet.
Status: active
Exercised: yes - `crates/retrieval/tests/dense_numerics.rs`
`quantized_scores_and_order_match_the_independent_reference_and_full_sort`,
`extreme_scales_and_codes_score_finite_and_match_the_reference`,
`the_fixture_rejects_unweighted_and_f32_first_quantized_scoring`,
`term_table_scores_match_the_reference_for_every_chunk_and_tile_shape`,
`term_table_scores_the_fixture_like_the_row_scorer`,
`scales_from_every_constructor_score_like_the_reference` (scales from
`calibrate`, `from_values`, `decode`, and the fixture score alike),
`term_table_refuses_codes_that_are_not_whole_rows`, and
`term_table_refuses_a_reserved_code_under_debug_assertions`.
Guarantee: The quantized score of a document is
`sum_j (s_j * s_j) * i32(c_query_j) * i32(c_doc_j)` with the weight and the
integer product widened to f64 before they multiply, accumulated from `+0.0`
in increasing coordinate order, whether `QuantizedQuery::score` forms each term
or `TermTable::score_rows` looks it up.
Check: `always` - for every fixture row, the production score bits equal the
independent reference's bits; the fixture's order under an unweighted integer
dot differs from the weighted order, and squaring the scale and rounding the
product in f32 together change at least one score's bits. The term-table scan's bits equal the
reference's for dimensions 1 to 40 with 0, 1, 2, and 17 rows, and for
dimensions 15, 16, 17, 384, and 385 with 511, 512, 513, and 1100 rows.
`always` because every scored row must carry the contract's exact value.
Fault/timing angle: none; scoring is a pure function.
Required faults and enabling state: Scales that differ by two orders of
magnitude across coordinates, codes at `+127` and `-127`, scales at
`f32::MAX`, `f32::MIN_POSITIVE`, and the smallest subnormal, dimensions that
leave a partial 16-coordinate chunk, and row counts that leave a partial
512-row tile.
Confidence: high - [evidence](evidence/dense-quantized-score-matches-weighted-reference.md).
`weighted_dot` (`scalar.rs:468`) and `QuantizedQuery::term_table`
(`scalar.rs:341`) were read against the formula and the tests run
against both.
Existing check: `crates/retrieval/tests/dense_scalar.rs`
`weighted_scoring_accumulates_in_f64_in_increasing_coordinate_order` and
`nonuniform_scales_rank_differently_from_a_raw_integer_dot`.
Impact: An unweighted or f32-rounded score selects a different candidate pool,
and exact rescore cannot recover an omitted neighbor.
Open questions:
- Parent Q2 approval of the grouping `(s * s) * product` (needs human input).

### dense-original-score-matches-f64-reference

Type: safety
Reachability: default-production - the query route's `ExhaustiveProducer`
(`crates/daemon/src/query_route.rs:467`) ranks through the oracle walk, which
scores with `score_block` (`crates/retrieval/src/dense/oracle.rs:508`). The
tests exercise `inner_product` (`crates/retrieval/src/dense/score.rs:13`) and
`rescore` (`score.rs:190`), which have no production caller at this base, so
the label rests on `dense_properties.rs:255` holding `inner_product_block`
equal to `inner_product` bit for bit.
Status: active
Exercised: yes - `crates/retrieval/tests/dense_numerics.rs`
`rescore_matches_the_independent_reference_and_full_sort_with_negatives_and_ties`,
`original_scores_separate_rows_an_f32_product_would_tie`, and
`signed_zero_coordinates_score_positive_zero`.
Guarantee: The retained-original score multiplies each pair of f32
coordinates in f64 and accumulates from `+0.0` in increasing coordinate order,
so orthogonal rows with negative-zero coordinates score `+0.0`.
Check: `always` - production score bits equal the reference's bits; two rows
whose f32-first products tie are separated by the f64 products. `always`
because the row scorer is the contract the block scorer is held to.
Fault/timing angle: none.
Required faults and enabling state: Rows one or a few ulps apart in two
coordinates, negative-zero coordinates, a row opposite the query.
Confidence: high - [evidence](evidence/dense-original-score-matches-f64-reference.md).
Existing check: `crates/retrieval/tests/dense_properties.rs` block-versus-row
bit equality.
Impact: A rounded product ties distinct rows and lets the identifier, not the
score, order them.
Open questions: None.

### dense-order-is-total-and-keeps-distinct-identities

Type: safety
Reachability: default-production - `rank_order` (`score.rs:92`) orders the
oracle walk's `TopK`, which the query route's producer uses.
Status: active
Exercised: yes - `crates/retrieval/tests/dense_numerics.rs`
`quantized_scores_and_order_match_the_independent_reference_and_full_sort`
(every `k` from one to the fixture size, a cut inside a tie),
`negative_and_zero_quantized_scores_order_numerically_and_zero_is_positive`,
and `the_fixture_order_is_pinned`; `dense_properties.rs`
`top_k_equals_sort_then_truncate_for_every_offer_order`.
Guarantee: Candidates order by score descending, then occurrence identifier
bytes ascending, with no tolerance; rows with equal payload and distinct
identifiers stay two candidates.
Check: `always` - the production top-k equals the reference full sort
truncated to k for every k, including a cut between two equal-code rows.
`always` because selection must be deterministic on every query.
Fault/timing angle: none.
Required faults and enabling state: Two identifiers with identical codes, a
negative score, a zero score, and a cut at the tie.
Confidence: high - [evidence](evidence/dense-order-is-total-and-keeps-distinct-identities.md).
Existing check: `dense_properties.rs` ranking laws over arbitrary offer
orders.
Impact: A nondeterministic or payload-collapsing order changes which
candidates reach rescore.
Open questions: None.

### dense-candidate-capacity-is-checked-before-allocation

Type: safety
Reachability: test-only - `CandidateCapacity::new`
(`crates/retrieval/src/dense/capacity.rs:38`) has no production caller at this
base.
Status: active
Exercised: yes - `crates/retrieval/tests/dense_numerics.rs`
`the_capacity_is_ceil_alpha_times_k_and_never_below_k`,
`the_capacity_takes_the_ceiling_of_the_exact_binary_product`,
`a_malformed_alpha_refuses_before_zero_k_is_considered`,
`zero_k_under_a_valid_alpha_is_an_empty_ranking_with_no_capacity`,
`an_unrepresentable_product_refuses_before_the_cap_is_consulted`, and
`a_pool_over_the_cap_refuses_and_the_cap_itself_is_admitted`,
`a_large_alpha_shifts_exactly_or_refuses`, and the seeded property
`the_capacity_equals_the_integer_ceiling_for_integer_and_dyadic_alphas`.
Guarantee: `R = ceil(alpha * K)` is computed exactly; alpha that is not finite
or is below one refuses first, a zero `K` then yields no capacity, an
unrepresentable product refuses next, and a pool above the cap refuses last.
Every outcome is decided from three scalars, before any R-sized state exists;
the private fields make `CandidateCapacity` the only source of a checked pool
size, and #610's scan witness supplies the evidence that the pool is sized from
it.
Check: `always` - each refusal class is returned for its witness and in the
stated precedence; the capacity for the approved alpha set
`{1, 2, 5, 10, 20, 50}` equals `alpha * K`. `always` because a scan cannot
start without a capacity.
Fault/timing angle: none.
Required faults and enabling state: NaN, infinities, `0.999`, `-0.0`, alpha
`f64::MAX`, `K = usize::MAX`, a cap one below the pool, `alpha = 1.1`.
Confidence: high - [evidence](evidence/dense-candidate-capacity-is-checked-before-allocation.md).
Existing check: none.
Impact: An unchecked product overflows or allocates an unapproved pool.
Open questions:
- Parent Q2 approval of the exact binary alpha semantics and the
  alpha-before-zero-K precedence (needs human input).
- Parent Q6 approval of the production cap; #825 D27 fixes the pool at 256
  for `K = 64` (needs human input).

### dense-invalid-query-never-enters-scoring

Type: safety
Reachability: test-only - `QuantizedQuery::new` (`scalar.rs:309`) is reached
from tests only; `rescore` (`score.rs:190`) is also test-only at this base and
validates its query the same way through `codec::validate`.
Status: active
Exercised: yes - `crates/retrieval/tests/dense_numerics.rs`
`the_query_transform_refuses_invalid_queries_before_any_code_exists`,
`a_query_whose_every_code_is_zero_is_refused`,
`the_query_codes_are_the_stored_row_encoding_under_the_same_scales`, and
`rescore_refuses_an_invalid_query_or_row_before_scoring`.
Guarantee: A query with the wrong dimension, a non-finite coordinate, zero
norm, a norm outside the layout's tolerance, a layout without a valid
tolerance, scales of another dimension (refused as `ScalesDimension`, a generation
wiring fault), or all-zero codes produces a typed
refusal and no quantized query exists to score with.
Check: `always` - each witness returns its refusal; an accepted query's codes
equal the stored-row encoding under the same scales (an equality between two
production paths, `QuantizedQuery::new` and `encode`; the literal code
witnesses are `[1, 0, 0, 0]` and the pinned fixture codes `[127, 1, 32, -10]`).
`always` because scoring consumes only constructed queries.
Fault/timing angle: none.
Required faults and enabling state: The listed malformed queries; scales of
one on every coordinate with a query of four halves.
Confidence: high - [evidence](evidence/dense-invalid-query-never-enters-scoring.md).
Existing check: `dense_scalar.rs`
`encoding_refuses_rows_outside_the_layout_and_scales_of_another_dimension`.
Impact: A malformed query scores every candidate with a meaningless value.
Open questions:
- Parent Q2 approval of refusing an all-zero-code query (needs human input).
