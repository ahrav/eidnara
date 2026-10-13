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

Rows whose check name ends in an issue number, such as `(#616)`, were added
after the base by that issue's follow-up; their lines are read at the
branch head that adds them.

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
| `crates/retrieval/tests/dense_layered.rs:194`, `winners_from_different_layers_with_equal_scores_order_by_identifier_bytes_whichever_layer_holds_the_smaller` (#616) | A base winner and a delta winner with equal scores order by identifier bytes in both identifier and layer orders, match the f64 reference, and `k = 1` keeps the smaller identifier. | unaudited | One tied pair; the tie vector is shared bit for bit, so near-ties are not exercised. |

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
| `crates/daemon/src/vector_generation.rs:1325`, `resident_bytes_saturate_on_declared_sizes_that_overflow` | `resident_bytes` saturates at `u64::MAX` on overflowing declared sizes. | unaudited | Saturation only; the value for an ordinary manifest is not asserted. |
| `crates/daemon/tests/vector_reader.rs:71`, resident census equality | The ledger's `LayerTables` charge equals `resident_bytes` summed over the composition. | unaudited | Both sides call `vector_generation::resident_bytes`, so the check pins consistency, not that the charge covers the bytes a decoded `Scales` holds. |
| `crates/retrieval/tests/dense_resolve_allocations.rs:98`, `retained_memory_follows_winners_and_hidden_rows_add_only_scratch` (#616) | At 10,000 winners, live bytes at close equal the winner list's capacity, match across base-only, all-superseded, and half-masked shapes, add at most four allocator calls for two or three times the entries, and keep peak bytes per entry within twice the base-only figure. | unaudited | Relative bounds on one host shape; allocator rounding is not counted, and no absolute cap is asserted. |
| `crates/retrieval/tests/dense_resolve_allocations.rs:145`, `retained_memory_grows_with_the_winner_count` (#616) | Retained bytes per winner stay between one and two `Winner`s at 10,000 and 40,000 winners. | unaudited | Two sizes only. |
| `crates/daemon/tests/vector_reader.rs:893`, `a_member_corrupted_while_pinned_for_verification_hands_out_no_view_and_recovery_takes_the_older_composition` (#617) | A delta's rows file corrupted at `BeforeVerification` while pinned yields `NoCompatibleTarget { examined: 1 }` with every pin, the transaction lock, and the census released; with an older composition present, acquisition hands out that composition, pins only its record and base, and charges only their bytes. | unaudited | The corruption flips the first byte; a fault inside the row data is caught by the same member hash. |
| `crates/daemon/tests/vector_reader.rs:993`, `a_walk_cancelled_mid_flight_ends_on_its_own_set_while_a_publish_prune_and_new_reader_overlap_it` (#617) | A ranking walk held on its third row keeps its record and base pinned through a publish, a prune whose readback reconciles against the ledger, a new reader's acquisition, and a bounded connection attempt by the new reader, which returns `StoreError::Deadline` because the walk owns the projection's only connection; cancelled, it ends `Incomplete(BudgetExhausted)` over the old set's five winners, the new reader ranks its own set, and the old files are reclaimed only after the walk returns. | unaudited | Drives `rank_layers_with_hook_for_test` over the view's layers rather than `vector_reader::rank`, which has no walk hook; the scratch reservation is not part of the held window. The new reader's ranking call has no hook, so whether it starts before or after the walk is released is not witnessed. |
| `crates/daemon/tests/query_route_compressed.rs:674`, `compressed_vectors_install_and_rank_only_under_a_live_activation_grant` (#618) | Vectors carrying the gate's activation install and rank `Complete`; after the evaluator is reinstalled, the same vectors degrade the lane as `Unavailable("activation_revoked")` with no scratch held and refuse reinstallation with `ActivationRevoked`; the renewed hook admission paired with the withdrawn activation degrades the same way before any scan window; a fresh activation installs again; and a gate closed at `AfterSelection` lets the rescore read at least one row, then discards the ranking as `activation_revoked` with no scratch held. | unaudited | The revocations are a manifest reinstall and a gate close; a durable stop record or a revoked campaign withdraws the grant through the same token and is covered only in `compressed_activation.rs`. |
| `crates/daemon/tests/vector_compaction.rs:946`, `a_withdrawn_activation_grant_publishes_nothing_and_a_live_one_publishes_the_same_compaction` (#618) | A withdrawn activation refuses compaction publication with `ActivationRevoked` before any profile event, leaving the selection and the generation set unchanged; a live activation publishes the same compacted base. | unaudited | Only the check before the readback is exercised here; the composition check inside `vector_composition::publish` is exercised by the row below. |
| `crates/daemon/tests/vector_compaction.rs:1004`, `every_identity_field_and_a_member_cut_after_pinning_refuse_and_write_nothing` (#619) | With the resident limit at one byte, a view refuses compaction as `Identity` under another `embedding_model`, `tokenizer_fingerprint`, `vector_dimension`, `unit_norm_tolerance`, `generation_epoch`, `kernel_incarnation_id`, `generation_id`, or `checkpoint`, naming the field, so the check precedes every reservation; a base whose rows were cut after pinning refuses as `Row` and releases every reservation; each refusal leaves the work directory empty, the generation set unchanged, and the census as before. | unaudited | `metric` and `quantizer_recipe` have one value each, so no mismatch can be built for them; every case drifts the expectation, so the refusal names the base and the per-delta check is not separately witnessed. |
| `crates/daemon/tests/vector_compaction.rs:1126`, `a_tail_that_inserts_a_new_occurrence_is_carried_once_and_consumed_once_by_the_next_cut` (#619) | A tail published after the cut inserts `zeta`, updates `epsilon`, and deletes `beta`; the compaction carries it unchanged and `zeta` ranks once, first; the next cut consumes the tail into a base holding `zeta` once, with the same ranking. | unaudited | Sequential: the tail is published between the compaction and its publication, not by a racing thread. |
| `crates/daemon/tests/vector_compaction.rs:1194`, `a_tail_published_over_an_unreconciled_compaction_is_kept_once_and_the_retry_refuses` (#619) | After an `Unknown` publication outcome, a writer composes a tail over the compacted selection; reconciliation reports `Other` naming the writer's composition, recovery takes it, a retry from the old cut refuses `PrefixMoved`, and the tail stands once above the compacted base with the reference ranking. | unaudited | Reconciliation compares the selector only; it does not report that the compaction was published and then extended, so that is inferred from the members. The lost reply is an observer error at `AfterRename` in one live process, which skips the root sync; no process restart or later cut is exercised. |
| `crates/daemon/tests/vector_compaction.rs:1283`, `layerings_that_resolve_alike_compact_to_one_base_and_keep_equal_payloads_distinct` (#619) | Two layerings that resolve to the same rows, with `beta` and `delta` sharing one payload, compact to one base digest and rank alike, and the equal payloads stay two occurrences with equal score bits. | unaudited | Two layerings; a permutation across more deltas is not built. |
| `crates/daemon/tests/vector_compaction.rs:1348`, `competing_disk_work_that_fills_the_pool_refuses_the_publication_and_holds_nothing` (#619) | A competing staging reservation that fills the disk pool after the compaction refuses publishing the base with `LimitExceeded` on the disk limit, staging nothing, holding nothing beyond the competitor, and keeping the selection and the old view's pins; after the competitor's reservation ends, the same compaction publishes and its charges end with discard. | unaudited | The pool is full; the exact boundary is the next row. The competitor is a direct ledger reservation, not a second stager. |
| `crates/daemon/tests/vector_compaction.rs:1411`, `the_disk_pool_stages_a_compacted_base_at_its_exact_bytes_and_refuses_one_byte_less` (#619) | With one byte less than the base's staged bytes (its inventory files and manifest) free, publication refuses at the base with `LimitExceeded` on the disk limit; with exactly those bytes free the base stages and publication refuses at the composition record; each refusal leaves the disk census at the competitor and the selection unchanged, and the same compaction publishes once the competitor ends. | unaudited | The composition record's own exact boundary is not pinned; the competitor is a direct ledger reservation. |
| `crates/daemon/tests/vector_composition.rs:2054`, `a_new_base_publishes_only_under_a_live_activation_and_a_delta_on_the_selected_base_needs_none` (#618) | With nothing selected, and again with another base selected, a composition on a new base refuses as `BaseUnactivated` at `NotStaged` with no grant and with a withdrawn grant, leaving the generation set and the selection unchanged; a live grant publishes it; a composition that adds a delta to the selected base publishes with no grant. | unaudited | The grant is checked once before delta admission and staging; a withdrawal after that check is not exercised. |

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
- `Ledger::reconcile` (`crates/daemon/src/vector_admission.rs:332`) runs
  only in tests. No production process owns a vector `Ledger` at this base,
  and the production prunes (`crates/daemon/src/bin/eidnara_host/serve.rs:389`,
  `crates/daemon/src/bin/eidnara-host.rs:1255`) run in launcher processes
  that own none, so the prune readback reconcile in #618 is met only in tests until
  the RP2.7/RP2.8 vector-reader owner reconciles in its own process.
