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
records, and the RP2.6.U2 ticket
([#610](https://github.com/ahrav/eidnara/issues/610)) lands the candidate-pool
records.

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
| [dense-pool-is-the-top-r-of-the-eligible-resolved-set](#dense-pool-is-the-top-r-of-the-eligible-resolved-set) | safety | test-only | always | active | high |
| [dense-rejected-leaders-never-starve-eligible-rows](#dense-rejected-leaders-never-starve-eligible-rows) | liveness | test-only | always | active | high |
| [dense-pool-never-mixes-authority-states](#dense-pool-never-mixes-authority-states) | safety | test-only | always | active | high |
| [dense-scan-bounds-end-with-no-candidate](#dense-scan-bounds-end-with-no-candidate) | safety | test-only | always | active | high |

## Records

### dense-quantized-score-matches-weighted-reference

Type: safety
Reachability: test-only - `QuantizedQuery` (`crates/retrieval/src/dense/scalar.rs:448`)
and `TermTable::score_rows` (`scalar.rs:565`) are called from
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
`weighted_dot` (`scalar.rs:628`) and `QuantizedQuery::term_table`
(`scalar.rs:501`) were read against the formula and the tests run
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
(`crates/daemon/src/query_route.rs:503`) ranks through the oracle walk, which
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
`CandidateCapacity::new` has no production caller at this base, so the clause
that a pool is sized from the checked capacity before any R-sized state exists
waits for #610's scan witness.
Guarantee: `R = ceil(alpha * K)` is computed exactly; alpha that is not finite
or is below one refuses first, a zero `K` then yields no capacity, an
unrepresentable product refuses next, and a pool above the cap refuses last.
Every outcome is decided from three scalars, before any R-sized state exists;
the private fields make `CandidateCapacity` the only source of a checked pool
size, and the candidate scan sizes its pool from it
(`crates/retrieval/src/dense/candidates.rs:321`).
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
from tests only; `rescore` (`score.rs:194`) is also test-only at this base and
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
(`crates/retrieval/src/dense/candidates.rs:260`) has no production caller at
this base; #613 connects it to pinned generations.
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
(`crates/retrieval/src/dense/candidates.rs:260`) is called only from
`crates/retrieval/tests/dense_candidates.rs` at this base; no producer calls
it until #613 connects it to pinned generations.
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
Reachability: test-only - `select_candidates`
(`crates/retrieval/src/dense/candidates.rs:260`) and
`select_candidates_with_hook_for_test` (`candidates.rs:271`, behind the
`test-support` feature) are called only from
`crates/retrieval/tests/dense_candidates.rs` at this base; no producer calls
either until #613 connects the scan to pinned generations.
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
Reachability: test-only - `select_candidates`
(`crates/retrieval/src/dense/candidates.rs:260`) and
`select_candidates_with_hook_for_test` (`candidates.rs:271`) are called only
from `crates/retrieval/tests/dense_candidates.rs` at this base; the slot
reservation test runs inside `crates/retrieval/src/dense/oracle.rs`. No
producer calls the scan until #613 connects it to pinned generations.
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
allocated (`Progress::read_candidate`, `crates/retrieval/src/dense/oracle.rs:1090`),
and a batch reserves only the candidate and score slots its batch bytes can
fill (`Stored::batch_slots`, `oracle.rs:641`); heap bytes are checked before an
eligible row moves into the set (`Progress::hold`,
`oracle.rs:1215`). The heap bound covers the set while the scan runs; the
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
