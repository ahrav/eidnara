# tokenizer-over-long-pieces-are-chunked-and-bounded

## Discovery trigger

The crate documentation (`crates/tokenizer/src/lib.rs`) states that tiktoken's merge loop is quadratic in the byte length of one pre-token piece and that pieces above `MAX_PIECE_BYTES` are split before merging. That is a production invariant with a permitted divergence from the oracle, and neither of the first two records captured it.

## Evidence trail

- `MAX_PIECE_BYTES` is `4096` (`lib.rs:93`).
- `encode_bounded` (`lib.rs:123-142`) walks `scan::pieces(text)` and hands each span at or below the cap to `Vocab::encode_piece` whole (`:131-132`); an over-long span is encoded as `char_chunks` of at most `MAX_PIECE_BYTES` bytes (`:134-138`). The source tree's early return of the whole text to `CoreBPE` below the cap is gone; every input takes the same path.
- `char_chunks` (`lib.rs:101-115`) steps back to the last `char` boundary at or before the cap, so a chunk never splits a multi-byte character; `char_chunks_respect_boundaries_and_cap` (`lib.rs:209`) asserts that directly.
- `estimate_tokens` (`lib.rs:148-150`) is `encode_bounded(text).len()`.
- `merge_scan` stores piece-relative offsets as `u32` on the strength of the cap (`src/bpe.rs:232-234`).
- `ids_match_reference_impl` (`src/parity_tests.rs:63`) draws letter runs of 3,800 to 4,400 bytes and CJK runs of 3,600 to 4,500 bytes (`:21-22`) and requires equality with `reference_impl`, whose `MAX_PIECE_BYTES`, `char_chunks`, and `encode_bounded` (`src/reference_impl.rs:35`, `:59`, `:75`) implement the same split.
- `over_long_piece_is_chunked_and_bounded` (`tests/token_golden.rs:116`) builds a letter run three caps long inside `prefix` and ` suffix` and asserts the ids equal the concatenation of the prefix, each `MAX_PIECE_BYTES` chunk, and the suffix, and that `estimate_tokens` matches.
- `over_long_cjk_piece_keeps_char_boundaries` (`:134`) encodes a four-byte-character run above the cap; byte-based chunking would slice mid-codepoint and panic on the `&str` index.
- `long_text_without_over_long_piece_is_unaffected_by_bound` (`:150`) repeats a sentence past the cap with no over-long piece and asserts the whole encoding equals the per-sentence concatenation.
- All pass under `cargo test --workspace` on Rust 1.98 (CI) and stable.

## Failure scenario

The cap is removed or raised so a long unpunctuated run takes seconds, or chunking moves to byte offsets and splits a character.

## Timing windows and dependencies

None; the bound is a per-call cost property.

## What a test must construct

- Present: the over-long, multi-byte, and unaffected cases, asserted structurally.
- Missing: a time-bounded assertion or benchmark, so the bound is measured rather than inferred from chunk structure.

## Investigation log

### Q: Does the record conflict with the oracle-parity record?

- Sources examined: The crate docs, both records, the scope note in `catalog.md`.
- Findings: Parity is promised for pieces at or below the cap; above it, ids may differ at chunk seams by design. The scope note lists this as one of three deliberate divergences.
- Missing evidence: None.
- Conclusion: resolved with answer: the two records partition the input space by piece length.

### Q: Where does the cap live at HEAD, and what pins the seam behaviour?

- Sources examined: `src/lib.rs:93-142`, `src/bpe.rs:232-234`,
  `src/parity_tests.rs:21-22`, `src/reference_impl.rs:35-95`.
- Findings: the cap is unchanged and now also bounds the `u32` offsets inside
  `merge_scan`. The seam ids are pinned to the reference chunker by the
  straddling proptest arms; the reference duplicates the constant, so a cap
  change made to both passes.
- Missing evidence: a literal assertion on the cap's value and a time bound.
- Conclusion: Exercised stays partial; the Existing check gains the proptest
  arms.
