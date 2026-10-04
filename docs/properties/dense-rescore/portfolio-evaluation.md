# Dense numerical portfolio evaluation

Discovery seeks properties; evaluation seeks flaws in the set. This pass was
run at RP2.6.U1, during review of the U1 change, by an evaluator that had not
taken part in the discovery: it was given `../METHOD.md`, `catalog.md`,
`existing-checks.md`, `fault-map.md`, `crates/retrieval/src/dense/scalar.rs`,
`score.rs`, `capacity.rs`, and `crates/retrieval/tests/dense_numerics.rs`
(with `dense_scalar.rs` and `dense_properties.rs` for the cited existing
checks), and did not open `evidence/`. It ran
`cargo test --locked -p retrieval --test dense_numerics` (25 passed in the
debug profile; `term_table_refuses_a_reserved_code_under_debug_assertions` is
`cfg(debug_assertions)`, so a release run has 24) and grepped
`crates --include=*.rs` outside `crates/retrieval/tests/` for
`QuantizedQuery`, `TermTable`, `CandidateCapacity`, `weighted_dot`,
`term_table`, `score_rows`, and `rescore`: the only hits are the re-exports in
`crates/retrieval/src/dense/mod.rs`. Production reaches `score_block`
(`crates/retrieval/src/dense/oracle.rs:508`) and `rank_order` (`oracle.rs:653`)
only. Every `file:line` in `catalog.md` and `existing-checks.md` was found at
the U1 head. The disposition is ours.

Four lenses were applied: harness fit, coverage balance, implementability, and
a wildcard pass that questioned the framing itself.

## Disposition summary

| Category | Count | Status |
| --- | --- | --- |
| refinement | 7 | applied to the catalog, existing-checks, and fault-map |
| gap | 2 | queued |
| bias | 3 | require human judgment, listed below |

## Refinements applied

1. **Three `dense_numerics.rs` tests were credited by no record.**
   `scales_from_every_constructor_score_like_the_reference`
   (`crates/retrieval/tests/dense_numerics.rs:541`),
   `term_table_refuses_codes_that_are_not_whole_rows` (`:717`), and
   `term_table_refuses_a_reserved_code_under_debug_assertions` (`:726`) are
   now listed under `dense-quantized-score-matches-weighted-reference`;
   `fault-map.md` already named their seam.
2. **The first record's Check overstated its negative control.**
   `f32_first_quantized` (`dense_numerics.rs:90-98`) squares the scale in f32
   and rounds the product in f32 in one control, and the test asserts only
   that the combined control differs (`:600-607`). The Check now says the two
   together change at least one score's bits.
3. **`dense-original-score-matches-f64-reference` justified `always` with a
   function production does not call.** `rescore` (`score.rs:190`) has no
   caller outside tests; the oracle walk scores through `score_block`
   (`oracle.rs:508`). The Reachability line now names the producer
   (`crates/daemon/src/query_route.rs:467`), the block scorer, and the bridge
   the label rests on, `dense_properties.rs:255` holding `inner_product_block`
   equal to `inner_product` bit for bit; the rationale now reads "the row
   scorer is the contract the block scorer is held to".
4. **`dense-invalid-query-never-enters-scoring` cited `rescore` without its
   own reachability class.** It is test-only at this base; the line says so.
5. **The fault map omitted the strongest unrepresentable-product witness.**
   `a_large_alpha_shifts_exactly_or_refuses` (`dense_numerics.rs:306-314`)
   drives alpha `2^76` with `K = 2^52`, where an unchecked shift wraps `2^128`
   to zero and would fit a `usize`; the row now names it.
6. **`existing-checks.md` under-described `dense_properties.rs:53`.** The test
   also asserts `TopK::admits` against model membership before every offer
   (`:80-85`); the row says so.
7. **The last record's Check read as an independent equality.** The equality
   between an accepted query's codes and the stored-row encoding compares two
   production paths (`QuantizedQuery::new` and `encode`,
   `dense_numerics.rs:436-447`); the literal code witnesses are `[1, 0, 0, 0]`
   (`:432`) and the pinned fixture codes `[127, 1, 32, -10]` (`:491`). The
   Check now names both.

## Gaps queued

1. **The resident charge for decoded scales has no value-level check.** The
   U1 change makes `vector_generation::resident_bytes`
   (`crates/daemon/src/vector_generation.rs:754`) charge the scales file three
   times over, four bytes of scale plus eight bytes of squared weight per
   coordinate (`Scales { scales: Vec<f32>, weights: Vec<f64> }`,
   `crates/retrieval/src/dense/scalar.rs:65-69`). The only checks are the
   overflow saturation (`vector_generation.rs:1249`) and a census equality
   that calls the same function on both sides (`existing-checks.md`, resource
   accounting). Queue a record `dense-resident-charge-covers-decoded-scales`
   (default-production through the ledger at `vector_generation.rs:948` and
   `crates/daemon/src/vector_reader.rs:316`; `always`): for a manifest whose
   scales file is `n` bytes, `resident_bytes` equals the other resident sizes
   plus `3n`, and `3n / 4` equals `n / 4` coordinates times
   `size_of::<f32>() + size_of::<f64>()`. A unit test beside
   `vector_generation.rs:1249` is enough. It belongs in this part because U1
   authored the charge.
2. **The term table's documented size has no record.** `QuantizedQuery::term_table`
   documents a `2048 * dimension` byte table built from `256 * dimension`
   terms (`scalar.rs:340`), and the catalog's scope claims the part owns
   resource obligations, but no record or test asserts either number. Queue
   `dense-term-table-size-is-bounded-by-dimension` (`always`, test-only at this
   base), or narrow the scope sentence (bias 2).

## Biases for a human

1. **The capacity slug claims an ordering nothing here observes.**
   `dense-candidate-capacity-is-checked-before-allocation` tests arithmetic
   and refusal precedence over three scalars (`capacity.rs:38-61`); the
   allocation half is deferred to "#610's scan witness". The record now
   carries `Exercised: partial` naming the covered clauses and the pending
   witness. Whether to also rename the slug to one the U1 tests discharge,
   for example `dense-candidate-capacity-is-exact-and-refuses-in-order`, or
   keep the specification's framing until #610 lands, is the human decision.
2. **The scope sentence outruns the portfolio.** The catalog says the part
   owns "their resource and cancellation obligations", yet every record is
   `always` over a pure function: the semantics distribution is `always` 5,
   `always-or-unreached` 0, `sometimes` 0, `reachable` 0, `unreachable` 0, and
   `dense/{scalar,score,capacity}.rs` has no cancellation seam. Either narrow
   the scope to the numerical contract, candidate pool sizing, and retained-f32
   rescore, or pre-register the resource and cancellation records later
   tickets will discharge.
3. **"Independent reference" is independent by authorship only.**
   `reference_quantized` (`dense_numerics.rs:38-47`) writes the same formula in
   the same order as `weighted_dot` (`scalar.rs:468-490`) and landed in the
   same change. It discriminates the frozen policy choices through the
   negative controls (`:80-110`) and cannot catch a shared misreading of the
   specification, such as the Q2 grouping `(s * s) * product` the first
   record leaves open. The oracle is fit for its purpose; whether the catalog
   should say "written from the formula, same author" is a wording decision.
