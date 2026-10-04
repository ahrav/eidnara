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
records, the RP2.6.U2 ticket
([#610](https://github.com/ahrav/eidnara/issues/610)) lands the candidate-pool
records, the RP2.6.U3 ticket
([#613](https://github.com/ahrav/eidnara/issues/613)) lands the pinned-rescore
records, and the RP2.6.U4 ticket
([#620](https://github.com/ahrav/eidnara/issues/620)) lands the request
lifetime records.

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
candidate scan scores the lanes of one layer through `weighted_dot_block`, an
eight-lane block with its own tail and extrema fixtures: `dense_numerics.rs`
`each_block_lane_scores_its_row_like_the_reference` and `dense_properties.rs`
`a_quantized_block_scores_each_lane_as_the_single_row_score_does` check every
lane against `weighted_dot` and an in-order model. The term-table scan
(`TermTable::score_rows`) is scalar code with its own chunk-tail, tile-boundary,
alignment, and extrema fixture. A vectorized path (U5) needs its own tail,
alignment, and extrema fixtures, which are not measured here.

## Index

| Slug | Type | Reachability | Semantics | Status | Confidence |
| --- | --- | --- | --- | --- | --- |
| [dense-quantized-score-matches-weighted-reference](#dense-quantized-score-matches-weighted-reference) | safety | test-only | always | active | high |
| [dense-original-score-matches-f64-reference](#dense-original-score-matches-f64-reference) | safety | default-production | always | active | high |
| [dense-order-is-total-and-keeps-distinct-identities](#dense-order-is-total-and-keeps-distinct-identities) | safety | default-production | always | active | high |
| [dense-candidate-capacity-is-checked-before-allocation](#dense-candidate-capacity-is-checked-before-allocation) | safety | test-only | always | active | high |
| [dense-invalid-query-never-enters-scoring](#dense-invalid-query-never-enters-scoring) | safety | test-only | always | active | high |
| [dense-pool-is-the-top-r-of-the-eligible-resolved-set](#dense-pool-is-the-top-r-of-the-eligible-resolved-set) | safety | test-only | always | active | high |
| [dense-rejected-leaders-never-starve-eligible-rows](#dense-rejected-leaders-never-starve-eligible-rows) | liveness | test-only | always | active | high |
| [dense-pool-never-mixes-authority-states](#dense-pool-never-mixes-authority-states) | safety | test-only | always | active | high |
| [dense-scan-bounds-end-with-no-candidate](#dense-scan-bounds-end-with-no-candidate) | safety | test-only | always | active | high |
| [dense-rescore-reads-only-accepted-rows-from-their-pinned-layers](#dense-rescore-reads-only-accepted-rows-from-their-pinned-layers) | safety | test-only | always | active | high |
| [dense-rescore-is-the-top-k-of-the-pool](#dense-rescore-is-the-top-k-of-the-pool) | safety | test-only | always | active | high |
| [dense-missing-original-quarantines-without-substitute](#dense-missing-original-quarantines-without-substitute) | safety | test-only | always | active | medium |
| [dense-stage-evidence-keeps-coverage-and-recall-apart](#dense-stage-evidence-keeps-coverage-and-recall-apart) | safety | test-only | always | active | medium |
| [dense-one-request-budget-spans-every-stage](#dense-one-request-budget-spans-every-stage) | safety | test-only | always | active | high |
| [dense-charges-stay-held-through-physical-work](#dense-charges-stay-held-through-physical-work) | safety | test-only | always | active | high |
| [dense-reused-connection-stays-isolated](#dense-reused-connection-stays-isolated) | safety | test-only | always | active | high |
| [dense-producer-limits-are-checked-at-installation](#dense-producer-limits-are-checked-at-installation) | safety | test-only | always | active | high |
| [dense-failures-keep-their-classification-at-the-route](#dense-failures-keep-their-classification-at-the-route) | safety | test-only | always | active | high |

## Records

### dense-quantized-score-matches-weighted-reference

Type: safety
Reachability: test-only - `QuantizedQuery` (`crates/retrieval/src/dense/scalar.rs:448`)
is built by `select_inner` (`crates/retrieval/src/dense/candidates.rs:312`)
under `select_candidates_observed`, which `rank_compressed` calls;
`CompressedProducer::rank` (`crates/daemon/src/query_route.rs:717`) reaches
`rank_compressed` only after `set_dense_vectors` (`query_route.rs:1865`)
installs a composition, and no production caller installs one at this HEAD.
`TermTable::score_rows` (`scalar.rs:565`) is called from
`crates/retrieval/tests/` only.
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
`term_table_refuses_a_reserved_code_under_debug_assertions`; the block
form by `dense_numerics.rs` `each_block_lane_scores_its_row_like_the_reference`,
`crates/retrieval/tests/dense_properties.rs`
`a_quantized_block_scores_each_lane_as_the_single_row_score_does`, and
`crates/retrieval/tests/dense_candidates.rs`
`a_block_of_winners_from_two_layers_scores_each_under_its_own_layer`.
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
`weighted_dot` (`scalar.rs:628`), `weighted_dot_block` (`scalar.rs:655`), and
`QuantizedQuery::term_table` (`scalar.rs:501`) were read against the formula
and the tests run against them.
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
(`crates/daemon/src/query_route.rs:543`) ranks through the oracle walk, which
scores with `score_block` (`crates/retrieval/src/dense/oracle.rs:181`). The
tests exercise `inner_product` (`crates/retrieval/src/dense/score.rs:13`) and
`rescore` (`score.rs:228`), which have no production caller at this base, so
the label rests on `dense_properties.rs:346` holding `inner_product_block`
equal to `inner_product` bit for bit. Corrected from `score.rs:194` and
`dense_properties.rs:258`, their lines at main `f6f42ea4a`.
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
Reachability: default-production - `rank_order` (`score.rs:126`) orders the
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
(`crates/retrieval/src/dense/capacity.rs:38`) is called by
`CompressedLimits::capacity` (`crates/daemon/src/query_route.rs:616`) at
`set_dense_vectors` and per compressed request; no production caller installs
a composition at this HEAD.
Status: active
Exercised: partial - `crates/retrieval/tests/dense_numerics.rs`
`the_capacity_is_ceil_alpha_times_k_and_never_below_k`,
`the_capacity_takes_the_ceiling_of_the_exact_binary_product`,
`a_malformed_alpha_refuses_before_zero_k_is_considered`,
`zero_k_under_a_valid_alpha_is_an_empty_ranking_with_no_capacity`,
`an_unrepresentable_product_refuses_before_the_cap_is_consulted`, and
`a_pool_over_the_cap_refuses_and_the_cap_itself_is_admitted`,
`a_large_alpha_shifts_exactly_or_refuses`, and the seeded property
`the_capacity_equals_the_integer_ceiling_for_integer_and_dyadic_alphas`
exercise the arithmetic and the refusal precedence over three scalars;
`CandidateCapacity::new` reaches production only behind an installed
composition, so the clause that a pool is sized from the checked capacity
before any R-sized state exists waits for #610's scan witness.
Guarantee: `R = ceil(alpha * K)` is computed exactly; alpha that is not finite
or is below one refuses first, a zero `K` then yields no capacity, an
unrepresentable product refuses next, and a pool above the cap refuses last.
Every outcome is decided from three scalars, before any R-sized state exists;
the private fields make `CandidateCapacity` the only source of a checked pool
size, and the candidate scan sizes its pool from it
(`crates/retrieval/src/dense/candidates.rs:324`).
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
Reachability: test-only - `QuantizedQuery::new` (`scalar.rs:464`) is reached
from tests only; `rescore` (`score.rs:228`) is also test-only at this base and
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

### dense-pool-is-the-top-r-of-the-eligible-resolved-set

Type: safety
Reachability: test-only - `select_candidates`
(`crates/retrieval/src/dense/candidates.rs:263`) has no production caller;
`select_candidates_observed` (`candidates.rs:273`) runs the same
`select_inner` from `rank_compressed`, which the route reaches only behind an
installed composition, and no production caller installs one at this HEAD.
Status: active
Exercised: yes - `crates/retrieval/tests/dense_candidates.rs`
`a_stable_scan_returns_exactly_the_top_r_of_the_eligible_resolved_set` (pool
sizes 1, 3, 5, 6, 20 by page sizes 1, 2, 8, plus an underfilled pool),
`only_resolved_winners_are_scored_and_each_scores_under_its_own_layer`,
`a_coverage_shortfall_keeps_the_pool_and_says_so`,
`the_pool_does_not_depend_on_the_order_rows_were_stored_in`,
`a_revoked_winner_is_never_scored`, and `a_row_at_any_rowid_is_visited`.
Guarantee: Under a stable authority with sufficient bounds, the pool is
exactly `Top(R, E, quantized_score)` in the U1 order, where `E` is the set of
resolved winners the projection lists as live and the kernel judges eligible;
each winner scores its own layer's codes under the query encoded with that
layer's scales, and the pool names each entry's winning layer row.
Check: `always` - the pool's identifiers and score bits equal an independent
full sort of `E` under the restated formula; an underfilled pool holds `E` and
nothing else; tombstoned and superseded rows are never scored (one score per
winner); a winner the projection does not list is never scored, even when
its codes hold the reserved `-128`; the same corpus stored in reverse yields
the same pool and judgment counts; a row moved to the smallest rowid is still
visited; ranked, candidate, and winner-row positions name one identity.
`always` because every completed scan must satisfy it.
Fault/timing angle: none for the stable case.
Required faults and enabling state: Unadmitted leaders interleaved with
eligible rows; a delta that supersedes one row, tombstones another, and has its
own calibration; a pool larger than `E`; a winner missing from the layers; a
winner missing from the projection with a reserved code; the corpus stored in
reverse; a row at rowid `i64::MIN`.
Confidence: high - [evidence](evidence/dense-pool-is-the-top-r-of-the-eligible-resolved-set.md).
The tests use the real kernel and the retrieval eligibility adapter.
Existing check: `crates/retrieval/tests/dense_layered.rs` covers the same walk
over f32 rows.
Impact: A pool built from stale, ineligible, or wrong-layer rows sends the
wrong candidates to rescore.
Open questions:
- Parent Q3: the projected predicates and full authority identity the pool
  carries are the existing snapshot and incarnation stamps (needs human
  input).

### dense-rejected-leaders-never-starve-eligible-rows

Type: liveness
Reachability: test-only - `select_candidates`
(`crates/retrieval/src/dense/candidates.rs:263`) is called only from
`crates/retrieval/tests/dense_candidates.rs`; `select_candidates_observed`
(`candidates.rs:273`) runs the same `select_inner` from `rank_compressed`,
which the route reaches only behind an installed composition, and no
production caller installs one at this HEAD.
Status: active
Exercised: yes - `crates/retrieval/tests/dense_candidates.rs`
`a_rejected_prefix_longer_than_the_pool_and_the_batch_does_not_starve_the_eligible_suffix`
and `an_all_eligible_scan_judges_only_the_pool`; `crates/retrieval/src/dense/oracle.rs`
`rows_left_drawn_stay_ahead_of_the_undrawn_rows` for the best-first draw order.
Guarantee: Rows the kernel rejects take no place in the pool and do not end
the scan, so twenty hidden leaders ahead of a pool of four and pages of four
leave the four best eligible rows in the pool; rows are judged best first
after the walk, so a pool that fills was judged over itself, every rejected
row ranked above its last member, and the rest of the batch that filled it,
at most a page of rows.
Check: `always` - the pool equals the reference top four of the eligible
suffix and at least four hidden rows were judged; with pages of four the scan
judges exactly the twenty hidden rows and the pool, 24 rows in six batches;
with one page of forty the first batch is the four hidden leaders and the
second judges the rest, so all twenty hidden rows, two batches, and forty
judged rows are counted, sixteen of them eligible rows below the pool; with
every row eligible the scan judges exactly `R` rows in batches of at most a
page; the unchecked top-R-then-filter over the same rows returns nothing, so
the fixture separates the two. `always` because the bound is the scan's own
population, which it always completes when its bounds allow.
Fault/timing angle: none.
Required faults and enabling state: A rejected score prefix longer than both
the pool and the page.
Confidence: high - [evidence](evidence/dense-rejected-leaders-never-starve-eligible-rows.md).
Existing check: `dense_oracle.rs`
`a_higher_scoring_excluded_row_never_displaces_an_eligible_one_and_stays_a_policy_exclusion`.
Impact: Ineligible near neighbors would hide every eligible result.
Open questions: None.

### dense-pool-never-mixes-authority-states

Type: safety
Reachability: test-only - `select_candidates_observed`
(`crates/retrieval/src/dense/candidates.rs:273`) is called from
`crates/retrieval/tests/dense_candidates.rs` and from `rank_compressed`
(`crates/daemon/src/vector_reader.rs:881`); `select_candidates`
(`candidates.rs:263`) has no caller in the workspace. The route reaches
`rank_compressed` only through `CompressedProducer`, which it builds only after
`set_dense_vectors` (`crates/daemon/src/query_route.rs:1865`) installs a
composition, and no production caller installs one at this HEAD.
Status: active
Exercised: yes - `crates/retrieval/tests/dense_candidates.rs`
`a_kernel_change_between_batches_discards_the_pool`,
`a_change_to_an_excluded_row_discards_the_pool_too`, and
`a_corrupt_identity_field_refuses_the_scan_with_no_candidate`.
Guarantee: Every batch of one scan, including batches whose verdicts only
excluded rows, is judged under one kernel snapshot and incarnation; every
batch runs after the walk's last page, so a change between batches discards
the pool, and a validation failure refuses the scan with no candidate.
Check: `always` - after a retirement, an admission of a row already judged
hidden, or a kernel restore between the first and second batch, the
completion names the change and the pool is empty; a kernel refusal returns an
error, never an empty complete pool; a discarded pool carries no snapshot or
incarnation. `always` because a mixed-stamp pool must never be returned.
Fault/timing angle: The window between two eligibility batches.
Required faults and enabling state: A kernel commit or restore inside the
walk's hook after the first judgment batch, with two-row pages so the batches
hold two rows each; a corrupt identity field the kernel refuses.
Confidence: high - [evidence](evidence/dense-pool-never-mixes-authority-states.md).
`judge_tracked` (`crates/retrieval/src/eligibility.rs:296`) compares each
batch's stamps with the first. The pool carries no re-judgment: a change after
the last batch is outside the scan, as a change after it returns is, and
validation-to-use freshness stays with RP2.7.
Existing check: `dense_oracle.rs` snapshot and restore tests for the f32 walk.
Impact: A pool judged under two authority states could hold a row a newer
policy excludes.
Open questions:
- Parent Q3 approval of snapshot plus incarnation as the usable authority
  identity, and of discard without restart (needs human input).
- A kernel judgment that fails after earlier batches admitted rows propagates
  through `judge_page` as a refusal by reading; no test injects a judgment-time
  kernel fault, only the identity-field refusal the kernel raises before
  reading.

### dense-scan-bounds-end-with-no-candidate

Type: safety
Reachability: test-only - as for `dense-pool-never-mixes-authority-states`:
the scan runs in production only through `select_candidates_observed`
(`crates/retrieval/src/dense/candidates.rs:273`) inside `rank_compressed`
behind an installed composition, and no production caller installs one at
this HEAD. The slot reservation test runs inside
`crates/retrieval/src/dense/oracle.rs`.
Status: active
Exercised: yes - `crates/retrieval/tests/dense_candidates.rs`
`each_storage_and_row_bound_saturates_alone_and_returns_no_candidate`,
`an_ended_budget_returns_no_candidate`,
`an_ended_budget_outranks_a_batch_bound_reached_in_the_same_flush`,
`a_coverage_shortfall_keeps_the_pool_and_says_so`, and
`codes_that_do_not_cover_their_layer_refuse_before_the_projection_is_read`;
`crates/retrieval/src/dense/oracle.rs`
`a_batch_reserves_only_the_slots_its_batch_bytes_can_fill` for the slot
reservation.
Guarantee: The scan-row bound, the batch-byte bound on the rows of one
judgment batch, the heap-byte bound on the accepted set, the preallocated-slot
check, the kernel batch limit on the pool, and the request budget each stop
the scan alone; every stop except a coverage shortfall returns no candidate,
and the slot check and pool limit refuse before any row is read.
Check: `always` - one row short of the population ends `RowBound`; with
one-row pages, which cap each judgment batch at one row, a batch bound equal
to the largest judged row completes and one byte less ends `BatchBytes`, and
two-row batches complete under twice that row, below the scan's total, so the
count restarts every batch; rows enter the set best first, so it only grows:
the heap bound equal to the slots plus the final set's strings completes and
one byte less ends `HeapBytes`; slots one byte over the bound refuse with no
row visited; a pool of 2000 refuses
`BatchOverBound`; a code set shorter than its layer refuses before any row is
visited; cancellation after a page ends `BudgetExhausted`, and wins over a
batch bound reached in the same flush; a row bound reached alongside a
coverage shortfall names the bound; each returns an empty pool. `always`
because a truncated pool must never pass as complete.
Fault/timing angle: Cancellation after a page is visited.
Required faults and enabling state: Each bound set at, and one unit below,
the value the fixture needs.
Confidence: high - [evidence](evidence/dense-scan-bounds-end-with-no-candidate.md).
Batch bytes are checked from the row's strings before a batch candidate is
allocated (`Progress::read_candidate`, `crates/retrieval/src/dense/oracle.rs:1096`),
and a batch reserves only the candidate and score slots its batch bytes can
fill (`Stored::batch_slots`, `oracle.rs:647`); heap bytes are checked before an
eligible row moves into the set (`Progress::hold`,
`oracle.rs:1221`). The heap bound covers the set while the scan runs; the
result moves the same entries through two more vectors of at most `R` slots,
which the bound does not count. The scan also keeps, per resolved winner, a
live bit, a rowid, and a prefix-table slot, and per scored row a score and a
winner index; `max_entries` and `max_rows` bound those stores, not the byte
bounds.
Existing check: `dense_oracle.rs`
`the_row_bound_stops_the_walk_as_incomplete_and_a_bound_at_the_population_stays_complete`
for the f32 walk, which keeps its partial ranking.
Impact: An unbounded batch or set exhausts memory; a truncated pool labeled
complete loses neighbors silently.
Open questions:
- Parent Q6 approval of the production byte bounds (needs human input).

### dense-rescore-reads-only-accepted-rows-from-their-pinned-layers

Type: safety
Reachability: test-only - `rank_compressed`
(`crates/daemon/src/vector_reader.rs:810`) is called by
`CompressedProducer::rank` (`crates/daemon/src/query_route.rs:717`), which the
route builds only after `set_dense_vectors` (`query_route.rs:1865`) installs a
composition; no production caller installs one at this HEAD.
Status: active
Exercised: yes - `crates/daemon/tests/vector_rescore.rs`
`only_pool_entries_are_read_and_each_from_its_winning_pinned_layer`,
`a_promotion_and_prune_after_selection_leave_the_rescore_on_the_pinned_files`,
and `the_ranking_is_the_same_however_the_rows_are_spread_across_layers`.
Physical order here means how rows spread across a composition's layers; the
order of rows within one layer file is fixed by the format, which writes and
verifies identifiers in strictly increasing byte order.
Guarantee: The rescore reads one original row per pool entry, by a positioned
read of the member and row the entry's winner names, through the descriptors
the view verified; it never reads a superseded, masked, or unselected row and
never consults the selector, so a promotion and prune after selection leave it
on the pinned files.
Check: `always` - the observed reads equal the pool's winner rows in pool
order, one each; the base's stale rows are never read; after a replacement is
published and the store pruned mid-ranking, every read names the old member
and the ranking is the old content's. `always` because every rescore must
read only what the scan accepted.
Fault/timing angle: The window between selection and the first original read.
Required faults and enabling state: A delta that supersedes and masks; a
publication and a prune inside `RescoreEvent::AfterSelection`.
Confidence: high - [evidence](evidence/dense-rescore-reads-only-accepted-rows-from-their-pinned-layers.md).
Real lifecycle generations, pins, and positioned reads; no SQLite vectors.
Existing check: `crates/daemon/tests/vector_reader.rs`
`old_readers_keep_their_complete_set_while_a_new_composition_is_published_and_pruned`
for the f32 layered ranking.
Impact: Reading the current selector's files or a stale row rescores a pool
against data the scan did not select.
Open questions: None.

### dense-rescore-is-the-top-k-of-the-pool

Type: safety
Reachability: test-only - as above; `rescore_pool`
(`crates/retrieval/src/dense/candidates.rs:410`) is pure.
Status: active
Exercised: yes - `crates/daemon/tests/vector_rescore.rs`
`only_pool_entries_are_read_and_each_from_its_winning_pinned_layer` and
`negative_scores_ties_and_an_underfilled_pool_keep_the_global_order`;
`crates/retrieval/tests/dense_candidates.rs`
`the_rescore_reads_each_entry_once_in_pool_order_and_ranks_by_original_score_then_identifier`
and
`the_rescore_refuses_a_bad_query_before_any_read_and_stops_at_the_first_failed_or_malformed_row`.
Guarantee: For the accepted pool `A`, the result is exactly
`Top(K, A, f32_score)` under the dense order, with distinct identities for
equal rows and every returned score the retained-f32 score of its row.
Check: `always` - the rescored identifiers and scores equal the shared f64
reference over the pool's own vectors; with negative scores, an underfilled
pool, and two equal rows the order equals the full reference. `always`
because rescore defines the returned dense ranking.
Fault/timing angle: none.
Required faults and enabling state: An opposite-axis query, a pool larger
than the eligible set, two identical rows.
Confidence: high - [evidence](evidence/dense-rescore-is-the-top-k-of-the-pool.md).
Existing check: `crates/retrieval/tests/dense_numerics.rs` for `rescore`.
Impact: A rescore that reorders or drops pool entries returns a ranking the
contract does not define.
Open questions: None.

### dense-missing-original-quarantines-without-substitute

Type: safety
Reachability: test-only - as above.
Status: active
Exercised: yes - `crates/daemon/tests/vector_rescore.rs`
`a_missing_accepted_row_quarantines_the_view_and_recovery_serves_the_prior_set_under_current_eligibility`,
`a_corrupt_accepted_row_is_refused_and_quarantined_without_a_substitute`,
`missing_codes_found_by_the_scan_quarantine_the_view_and_ordinary_refusals_do_not`,
`cancellation_during_the_rescore_is_a_budget_refusal_and_quarantines_nothing`,
`cancellation_after_an_empty_selection_is_a_budget_refusal`, and
`a_corrupt_row_found_by_the_f32_ranking_quarantines_the_view`;
the unit tests
`a_short_read_is_a_missing_row_and_any_other_read_error_is_a_failed_read` and
`only_missing_or_malformed_rows_count_as_walk_corruption` in
`crates/daemon/src/vector_reader.rs`.
Guarantee: An accepted row that is missing (a short read or an index past the
layer) or fails the codec or the layout refuses the whole request as
`Corrupt` naming the member and the occurrence and quarantines the view, so
every later ranking over it refuses; codes the scan finds missing, and rows
the f32 ranking finds missing or malformed, quarantine it the same way; a read error other than a short read is `RowFault::Unavailable`
and refuses as `Io`, or as `OracleRefusal::ReadFailed` in the scan, and
quarantines nothing; cancellation refuses as `Budget`; nothing older,
quantized, or reconstructed stands in, and no shorter ranking is returned.
Check: `always` - a rows file cut to its header after selection refuses with
`Missing` after one read, quarantines the view, and releases the scratch and
row buffers; NaN rows and doubled, finite rows refuse with `Rejected` naming
the member and the pool's best entry; an emptied codes file refuses the scan
and quarantines the view, while a cancelled scan does not; the read
classifier maps only `UnexpectedEof` to `Missing`; a cancelled budget refuses
with no read and no quarantine, and so does one cancelled over a pool with no
entry to read; a
re-acquisition re-verifies, recovery takes the prior verified composition, and
its ranking excludes an occurrence retired in the kernel meanwhile. `always`
because a corrupt generation must never yield a result.
Fault/timing angle: Corruption between selection and the original reads.
Required faults and enabling state: A truncated and a NaN-filled rows file
inside `RescoreEvent::AfterSelection`; a prior composition to recover to.
Confidence: medium - [evidence](evidence/dense-missing-original-quarantines-without-substitute.md).
The quarantine flag lives on the shared view; persistence across a restart is
the verification a new acquisition runs.
Existing check: `crates/daemon/tests/vector_reader.rs` truncated-row
refusal for the f32 layered ranking.
Impact: A substitute row returns scores for bytes the generation never held.
Open questions:
- Parent Q4: persistent quarantine is re-verification on acquisition, which
  refuses a member whose files no longer hash; a transient fault that leaves
  the files intact is served again after re-acquisition (needs human input).
- The I/O classification is checked at the classifier and the
  code-corruption predicate; no test makes a real file return a read error
  other than a short read, so the rescore's `Io` arm and the scan's
  `ReadFailed` arm are reached only by reading.
- Process-restart evidence here is a fresh acquisition in the same process,
  which reads only durable state; it is not a separate process and not
  power-loss evidence.

### dense-stage-evidence-keeps-coverage-and-recall-apart

Type: safety
Reachability: test-only - the stage record is assembled by the test from
`CompressedRanking`, which returns the pool and the rescored ranking apart.
Status: active
Exercised: yes - `crates/daemon/tests/vector_rescore.rs`
`the_alpha_sweep_keeps_candidate_coverage_and_rescored_recall_as_separate_stage_records`.
Guarantee: For alpha in `{1, 2, 5, 10, 20, 50}` the baseline, candidate, and
rescored identity sets are retained with separate candidate-coverage and
rescored Recall@10 fields against one frozen eligible f32 baseline; an empty
baseline yields no value rather than a perfect score.
Check: `always` - six records with the alphas in order, ten baseline
identities each, coverage below one at alpha one, coverage non-decreasing in
alpha, recall at most coverage,
the widest pool covering the baseline with the rescore equal to it, and no
value for an empty baseline. `always` because every sweep must keep the
stages apart.
Fault/timing angle: none.
Required faults and enabling state: Sixty fixed rows, every fifth
unadmitted.
Confidence: medium - [evidence](evidence/dense-stage-evidence-keeps-coverage-and-recall-apart.md).
The corpus clusters sixty rows around the query so that coverage at alpha one
is below one; coverage and recall are equal at each alpha, which the
specification allows; every alpha from five up pools all 48 eligible rows, so
those four records measure one state; the corpus is not the RP2.9 frozen
corpus.
Existing check: none.
Impact: A collapsed field hides whether a miss came from the pool or the
rescore.
Open questions:
- Parent Q6 and RP2.9 own the frozen corpus, the target, and the aggregation;
  this record measures nothing against them (needs human input).

### dense-one-request-budget-spans-every-stage

Type: safety
Reachability: test-only - the route builds `CompressedProducer`
(`crates/daemon/src/query_route.rs:684`) only when `set_dense_vectors`
(`query_route.rs:1865`) has installed a composition, and no production caller
installs one at this base; #897 configures the live producer.
Status: active
Exercised: yes - `crates/daemon/tests/query_route_compressed.rs`
`cancellation_at_each_stage_ends_the_request_on_the_original_budget_and_releases_its_charges`
and `a_deadline_that_lapses_inside_the_rescore_is_the_original_deadline`;
`crates/daemon/tests/dense_request_lifetime.rs`
`client_cancellation_reaches_the_dense_scan_validation_and_original_reads_and_the_work_joins_before_the_request_settles`
through a real host, client, and `RequestCtx`; `vector_rescore.rs`
`cancellation_at_an_original_read_ends_the_request_before_the_row_is_read`.
Guarantee: The request's one `EvalBudget`, derived with its absolute
deadline before the dense unit is submitted, stops the compressed scan, the
canonical eligibility batches, and the original reads; a cancellation at any
of them ends the request as cancelled and a lapse as the original deadline,
and no stage receives a fresh budget.
Check: `always` - a cancellation at the first visited row, after the first
judgment, and at the first original read each end `Terminal::Cancelled` with
the budget's exhaustion `Cancelled` and its deadline unchanged; a 300 ms
budget held 400 ms after selection ends `Terminal::Deadline` with the same
deadline; through the real host, a client cancellation at the first visited
row, after a judgment, and at an original read
answers no value, and no scan, judgment, selection, or read event follows the
release; a request whose answer was returned keeps it when its token is
cancelled afterwards. `always` because every dense request runs under one
budget.
Fault/timing angle: Cancellation inside each stage's window.
Required faults and enabling state: A `CancellationToken` cancelled from the
ranking thread's observer; a client-side cancellation through the host.
Confidence: high - [evidence](evidence/dense-one-request-budget-spans-every-stage.md).
Existing check: `crates/daemon/src/request_budget/host_tests.rs`
`cancelling_a_suspended_handler_interrupts_the_held_read_and_joins_it_before_settling`
for a generic held read.
Impact: A stage with its own budget runs past the caller's deadline or
ignores its cancellation.
Open questions:
- Parent Q5 approval of the bridge and the cancellation checkpoints, which are
  the ranked walk's check every sixteen visited rows (`BUDGET_STRIDE`), its
  batch checks, and the checks around each original read's observer
  (`vector_reader.rs:902`, `:911`) (needs human input).
- The validation stage holds after an eligibility batch; an interrupt inside
  a running kernel statement is witnessed only by the kernel's and storage's
  own progress-handler tests.

### dense-charges-stay-held-through-physical-work

Type: safety
Reachability: test-only - as above.
Status: active
Exercised: yes - `crates/daemon/tests/dense_request_lifetime.rs`
`client_cancellation_reaches_the_dense_scan_validation_and_original_reads_and_the_work_joins_before_the_request_settles`
and `a_request_that_ranks_no_dense_lane_holds_no_view` and
`a_request_ranks_under_the_limits_and_vectors_installed_together`;
`query_route_compressed.rs`
`cancellation_at_each_stage_ends_the_request_on_the_original_budget_and_releases_its_charges`;
`vector_rescore.rs` `the_scan_scratch_charges_the_encoded_query_of_every_layer`
for the payload buffers `Scratch` covers.
Guarantee: The dense unit of a request whose embedding settled as a vector
owns a clone of the view's `Arc`, taken with the route limits under both
setters' locks before the unit is submitted (`query_route.rs:1977`), so the
unit ranks under a pair the setters checked together and the view's pins and the ranking's `Scratch` and
`RowBuffers` reservations stay charged until the blocking work returns; a
cancelled request settles only after that work ends, and every charge is
released when it does. A request whose embedding is undeclared or unavailable
holds no clone, so an uninstall while its unit is pending leaves the retired
view's teardown to the clones that rank. Corrected from `query_route.rs:1962`,
the clone's line before the match on `embedded`.
Check: `always` - with the ranking thread held at each stage after the client
cancelled, no error frame is published for 200 ms; on release the ledger
still holds `Scratch`, `RowBuffers`, and the pinned bytes; the error frame is
published no earlier than the release, and the ledger's census at
publication holds neither reservation; after a handler future is aborted
while its work is held, and the view is uninstalled and every other `Arc`
dropped, the ledger still holds the scratch and the pins until the release,
and both drop to zero after it. `always` because no charge may go before its
work.
Fault/timing angle: The interval between logical cancellation and the
physical return of the blocking work.
Required faults and enabling state: A held observer on the blocking thread,
released by the test after the client cancels.
Confidence: high - [evidence](evidence/dense-charges-stay-held-through-physical-work.md).
Existing check: `vector_reader.rs` view-drop release tests.
Impact: A charge released at logical cancellation lets a later request
exceed the ledger while the old work still runs.
Open questions:
- A permanently blocked read is visible only as unresolved blocking work; no
  test holds one indefinitely.
- The returned hits, at most `k` rows, leave the ledger when
  `rank_compressed` returns; their bound through fusion is RP2.7's fused-union
  cap.
- The view's `Admission` grant is checked when a reservation is taken; a grant
  invalidated mid-query does not cancel the request, which the owner that
  installs the view (#897) decides.

### dense-reused-connection-stays-isolated

Type: safety
Reachability: test-only - as above.
Status: active
Exercised: yes - `query_route_compressed.rs`
`a_late_cancellation_of_one_request_leaves_the_next_on_the_reused_connection_complete`.
Guarantee: The projection's connection carries one request's stop predicate
only for that request's read; after request A ends cancelled, request B on the
same connection completes even when A's token is cancelled again during B's
original reads, and after request C completes uncancelled, request D
completes when C's token is first cancelled during D's original reads.
Check: `always` - A ends `Cancelled`; B and D end with a complete dense lane
and an unexhausted budget; C's answer stands. `always` because a reused
connection must never carry an earlier caller's cancellation.
Fault/timing angle: A late cancellation of A inside B's ranking.
Required faults and enabling state: Two sequential requests on one
`SearchProjection`.
Confidence: high - [evidence](evidence/dense-reused-connection-stays-isolated.md).
`SqliteStore::with_conn_interruptible` removes the progress handler before
the transaction ends.
Existing check: `crates/storage/src/lib.rs`
`an_interruptible_read_stops_a_running_statement_and_a_later_read_is_untouched`
and its leaked-handler negative control.
Impact: A stale handler interrupts an unrelated request.
Open questions: None.

### dense-producer-limits-are-checked-at-installation

Type: safety
Reachability: test-only - as above.
Status: active
Exercised: yes - `query_route_compressed.rs`
`the_pool_the_limits_give_is_checked_before_a_view_is_installed`,
`vectors_install_only_under_declared_dense_limits`, which also refuses a
later route-limit change that the installed vectors cannot serve, and
`vectors_install_only_under_the_tolerance_their_layers_carry`.
Guarantee: A composition installs only under declared dense limits whose
`unit_norm_tolerance` equals the view's layout bit for bit (`check_pair`,
`query_route.rs:741`), and only when the pool `CompressedLimits::capacity`
(`query_route.rs:615`) derives from the lane's `k` and the alpha policy passes
the capacity checks and fits one kernel eligibility batch, with scan pages
within the batch as well; a later route-limit change is checked against the
installed vectors the same way.
Check: `always` - no dense limits refuses `DenseUndeclared`; alpha 0.5
refuses `DenseCapacity(Alpha)`; a pool of 2048 refuses at `candidates`; pages
of 2000 refuse at `scan_page_rows`; alpha 4 with `k` 64 yields 256; with
vectors installed, route limits raising `k` to 1000 refuse `OverCap` and route
limits without dense limits refuse `DenseUndeclared`; a tolerance of 2e-3
against layers carrying 1e-3 refuses `DenseToleranceMismatch` at installation
and at a later route-limit change. `always` because an unapproved pool, or a
tolerance every request would refuse as `identity`, must never reach a
request.
Fault/timing angle: none.
Required faults and enabling state: The listed limits.
Confidence: high - [evidence](evidence/dense-producer-limits-are-checked-at-installation.md).
Existing check: `QueryRouteLimits::validate` for the exhaustive producer.
Impact: An oversized pool allocates or judges past what the kernel accepts.
Open questions:
- Parent Q6 approval of the production values; #825 D23 and D27 name `k` 64
  and a pool of 256 (needs human input).

### dense-failures-keep-their-classification-at-the-route

Type: safety
Reachability: test-only - as above.
Status: active
Exercised: yes - `query_route_compressed.rs`
`view_bounds_degrade_the_lane_and_a_missing_original_ends_the_request_and_quarantines`,
`each_view_and_scan_bound_degrades_the_lane_with_its_own_reason`,
`a_view_of_another_kernel_incarnation_degrades_the_lane`, and
`the_compressed_producer_serves_the_dense_lane_with_exact_original_scores`.
Guarantee: A served compressed ranking reaches fusion with each hit's exact
original score; a view bound degrades the dense lane as `view_bound` while the
other lanes answer; a missing accepted original ends the request as
`dense_corruption` and quarantines the view, which later requests see as
`quarantined`; cancellation and deadlines end the request with their own
terminals (`compressed_refusal`, `query_route.rs:758`; `lane_status`,
`query_route.rs:580`).
Check: `always` - the dense positions and raw score bits equal the f64
reference; the degraded answer marks `degraded`; pinned bytes and read bytes
degrade as `view_bound`, one scan row as `row_bound`, a one-byte heap as
`heap_over_bound`, a one-byte batch as `batch_bytes`, a full ledger as
`reservation`, and a view of another kernel incarnation as `identity`; the
corruption and the quarantine reasons are those strings. `always` because no
failure may pass as a successful dense completion.
Fault/timing angle: Corruption between selection and the reads.
Required faults and enabling state: A one-byte read bound; a rows file cut
after selection.
Confidence: high - [evidence](evidence/dense-failures-keep-their-classification-at-the-route.md).
Existing check: `crates/daemon/tests/query_route_dense.rs` for the exhaustive
producer.
Impact: A corrupt or truncated dense ranking served as complete.
Open questions:
- Parent Q7: both plugins' full-path witnesses through the delivered route
  are outside this test set; the live producer is configured in #897
  (needs human input).
- A quarantined view keeps the lane unavailable until its owner uninstalls or
  replaces it with `set_dense_vectors`; nothing reinstalls a view on its own.
- The handler reads route limits before the embedding wait and clones the
  vectors after it, so a concurrent reinstallation can pair old limits with
  new vectors; `CompressedProducer::rank` re-checks the pool and degrades the
  lane as `capacity`, which no test constructs.
