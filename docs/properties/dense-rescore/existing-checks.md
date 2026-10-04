# Existing checks and reuse assessment

System: `/local/home/ahrav/scratch/eidnara`. Base: `cc5898c3e` (the
`m6/897-mapped-int8-dense` commit the U1 change was authored against). Every
check below exists at that base and is unchanged by the U1 change; `file:line`
is read at the U1 head and found again with `git grep` at the base. Every
check is `unaudited`: source inspection establishes its presence and
assertions, not adequacy. Adequacy belongs to
`/testing:invariant-test-review`.

The U1 change adds `crates/retrieval/tests/dense_numerics.rs`; its tests are
the records' `Exercised` lists in `catalog.md`, not existing checks.

## Quantized scoring

| Location and check | Asserted behavior | Status | Limitation for this part |
| --- | --- | --- | --- |
| `crates/retrieval/tests/dense_scalar.rs:267`, `weighted_scoring_forms_i32_products_in_range_and_weights_each_by_its_squared_scale` | Every `i32` code product lies in `[-16129, 16129]`; `weighted_dot` equals a hand-summed `(s * s) * product` over eight coordinates. | unaudited | One fixture pair; the expected value is summed in the same order as production, so order is assumed rather than discriminated. |
| `crates/retrieval/tests/dense_scalar.rs:292`, `weighted_scoring_accumulates_in_f64_in_increasing_coordinate_order` | Terms `1e16, 1e-8, -1e16, 1e-8` sum to `1e-8` forward and `0` reversed; production returns the forward value. | unaudited | Discriminates forward from reverse only; a pairwise or blocked sum would also differ and is not pinned. |
| `crates/retrieval/tests/dense_scalar.rs:314`, `weighted_scoring_never_fuses_the_multiply_into_the_add` | The unfused sum `7.450580596923828e-9` differs from `mul_add` of the same terms. | unaudited | One pair; a compiler that fuses only some lanes would be caught only if it fused this one. |
| `crates/retrieval/tests/dense_scalar.rs:329`, `:338`, `:348`, the unequal-length and reserved-code refusals | `weighted_dot` panics on a length mismatch and, under debug assertions, on a `-128` query or document code. | unaudited | The reserved-code guard is `debug_assert!`; release profiles score the code. |
| `crates/retrieval/tests/dense_scalar.rs:355`, `nonuniform_scales_rank_differently_from_a_raw_integer_dot` | Under nonuniform scales the weighted order differs from the unweighted integer order. | unaudited | Negative control for the weights only; the f32-first control is in `dense_numerics.rs`. |
| `crates/retrieval/tests/dense_scalar.rs:376`, `a_query_encoded_under_another_generations_scales_carries_other_codes_and_scores` | The same query encodes to different codes and scores under two generations' scales. | unaudited | Shows the dependence; the `ScalesDimension` refusal for mismatched scales is in `dense_numerics.rs`. |
| `crates/retrieval/tests/dense_scalar.rs:446`, `accumulation_roundoff_can_push_a_computed_score_past_the_real_arithmetic_bound` | A two-coordinate counterexample exceeds the real-arithmetic quantization bound by one ulp of the accumulator and stays within bound plus accumulation roundoff. | unaudited | Bounds the quantization error, not the scorer's bit-exactness. |
| `crates/retrieval/tests/dense_scalar.rs:480`, `weighted_int8_scores_stay_within_the_quantization_bound_and_preserve_separated_orders` | Over a fixed eight-row corpus and three queries, quantized scores stay within the bound and separated exact orders are preserved. | unaudited | Fixed corpus; no extrema scales or codes. |
| `crates/retrieval/tests/dense_properties.rs:163`, `weighted_dot_matches_the_in_order_model_over_the_full_code_range` | Seeded proptest (ChaCha, 512 cases): `weighted_dot` equals an in-order f64 model over codes in `[-127, 127]` and positive scales. | unaudited | The model is written inline in the test; scales are drawn from seven fixed values and dimensions from `1..12`; one fixed seed, so one sample per revision of the test. |

## Original-f32 scoring and order

| Location and check | Asserted behavior | Status | Limitation for this part |
| --- | --- | --- | --- |
| `crates/retrieval/tests/dense_properties.rs:344`, `inner_product_block_matches_the_single_row_functions_bit_for_bit` | Seeded proptest: `inner_product_block` lanes equal `inner_product` per row bit for bit. | unaudited | Block versus row equality; neither side is compared to an independent f64 reference here. |
| `crates/retrieval/tests/dense_properties.rs:413`, `validate_from_sum_agrees_with_validate_on_every_row` | Seeded proptest: the block path's norm validation agrees with `codec::validate` on every row. | unaudited | Validation agreement, not score value. |
| `crates/retrieval/tests/dense_properties.rs:53`, `top_k_equals_sort_then_truncate_for_every_offer_order` | Seeded proptest: `TopK` over any offer order equals the model's full sort truncated to `k`, with ties common at the cut, and `TopK::admits` agrees with model membership before every offer. | unaudited | The model is written inline, not through `rank_order`; scores are drawn from six fixed values and identifiers from eight one-byte strings, so multi-byte identifier comparison is not exercised. |
| `crates/retrieval/tests/dense_oracle.rs:1532`, `rank_order_is_score_descending_then_identifier_bytes_ascending_with_no_epsilon` | One ulp separates scores; equal scores order by identifier bytes; `+0.0` orders before `-0.0` under `total_cmp`. | unaudited | Pairwise cases only. |
| `crates/retrieval/tests/dense_oracle.rs:140`, `tied_scores_order_by_identifier_bytes_ascending_and_k_cuts_the_tie_deterministically` | Two rows with identical vectors stay two candidates, order by identifier bytes, and a `k` inside the tie cuts deterministically. | unaudited | Walk-level; the tie is between two rows only. |
| `crates/retrieval/tests/dense_oracle.rs:1527`, `inner_product_refuses_unequal_lengths_instead_of_truncating` | `inner_product` panics on a length mismatch. | unaudited | Panic, not a typed refusal. |
| `crates/retrieval/tests/dense_oracle.rs:1613`, `tolerated_norm_error_does_not_turn_inner_product_into_cosine` | A row within norm tolerance but longer than the query ranks first with its stored inner product. | unaudited | One pair. |
| `crates/retrieval/tests/dense_oracle.rs:1464`, `rescore_over_retained_rows_agrees_with_the_exhaustive_ranking` | `rescore` over the retained rows returns the same ordered identities and scores as the oracle walk. | unaudited | Production versus production; the independent reference is in `dense_numerics.rs`. |
| `crates/retrieval/tests/dense_oracle.rs:613`, `a_page_that_fills_a_scoring_block_yields_the_reference_prefix` | A page of exactly one scoring block ranks like the test's reference prefix. | unaudited | Walk-level; the reference is the fixture's expected order. |
| `crates/retrieval/tests/dense_layered.rs:127`, `a_base_alone_ranks_like_the_oracle_over_the_same_rows` | The layered ranking over one base equals the oracle ranking. | unaudited | Production versus production. |

## Query and row validation

| Location and check | Asserted behavior | Status | Limitation for this part |
| --- | --- | --- | --- |
| `crates/retrieval/tests/dense_oracle.rs:766`, `an_invalid_query_or_layout_is_refused_before_the_projection_is_read` | Wrong dimension, non-finite, zero-norm, and out-of-tolerance queries and invalid layouts refuse before any page is read. | unaudited | Original-f32 path through the oracle; the quantized `QuantizedQuery::new` path is in `dense_numerics.rs`. |
| `crates/retrieval/tests/dense_oracle.rs:1589`, `codec_entry_points_refuse_invalid_tolerances_before_reading_rows` | Every codec entry point refuses a layout whose tolerance is invalid. | unaudited | Layout validity only. |
| `crates/retrieval/tests/dense_oracle.rs:583`, `an_infinite_stored_coordinate_is_refused` | A stored row with an infinite coordinate refuses the walk. | unaudited | Stored rows, not queries. |
| `crates/retrieval/tests/dense_properties.rs:126`, `the_first_non_finite_coordinate_is_the_one_named` | Seeded proptest: `RowRejection::NonFinite` names the first offending coordinate. | unaudited | Rejection payload only. |
| `crates/retrieval/tests/dense_scalar.rs:235`, `encoding_refuses_rows_outside_the_layout_and_scales_of_another_dimension` | `encode` refuses rows outside the layout and scales of another dimension. | unaudited | Row encoding; the query transform reuses `encode`, so this covers the shared refusal but not the transform's zero-code refusal. |
| `crates/retrieval/tests/dense_scalar.rs:258`, `encoding_with_scales_of_another_dimension_is_a_wiring_error` | Mismatched scales are a distinct, wiring-fault refusal. | unaudited | Encoding path. |
| `crates/retrieval/tests/dense_scalar.rs:184`, `encoding_clips_to_plus_minus_127_counts_every_clip_and_never_yields_minus_128` | Codes clip to `[-127, 127]`, every clip is counted, `-128` is never produced. | unaudited | Guarantees the scorer's reserved-code precondition from the encoder side. |
| `crates/retrieval/tests/dense_scalar.rs:91`, `calibration_refuses_no_rows_invalid_rows_and_a_nonzero_scale_that_underflows` | Calibration refuses an empty corpus, an invalid row, and a scale that underflows to zero. | unaudited | Guarantees `Scales` are positive finite from the calibration side; `Scales::from_values` is exercised in `dense_numerics.rs`. |

## Resource accounting

| Location and check | Asserted behavior | Status | Limitation for this part |
| --- | --- | --- | --- |
| `crates/daemon/src/vector_generation.rs:1249`, `resident_bytes_saturate_on_declared_sizes_that_overflow` | `resident_bytes` saturates at `u64::MAX` on overflowing declared sizes. | unaudited | Saturation only; the value for an ordinary manifest is not asserted. |
| `crates/daemon/tests/vector_reader.rs:71`, resident census equality | The ledger's `LayerTables` charge equals `resident_bytes` summed over the composition. | unaudited | Both sides call `vector_generation::resident_bytes`, so the check pins consistency, not that the charge covers the bytes a decoded `Scales` holds. |

## Capacity

None found. `CandidateCapacity` (`crates/retrieval/src/dense/capacity.rs`) is
new in U1; no check at the base computes `ceil(alpha * K)` or orders the
refusal classes.

## Suspiciously quiet areas

- Before U1 the only formula-level references for a score are the two
  proptest models in `dense_properties.rs` (quantized at `:163`, written
  inline; original-f32 has none, `:255` compares block to row). Every other
  pre-U1 comparison is production versus production or a hand-summed fixture
  in production order. The fixture-based independent references and the
  original-f32 reference now live in `dense_numerics.rs`.
- No check asserts that `resident_bytes` for an ordinary vector manifest
  equals the bytes a decoded `Scales` keeps (four bytes of scale plus eight
  bytes of squared weight per coordinate, `scalar.rs:68`); the U1 change
  doubles the scales file's charge and relies on the census equality above.
- No check scans codes through `TermTable::score_rows` from a producer; the
  scan has no production caller at this base.
