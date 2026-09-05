# tokenizer-over-long-pieces-are-chunked-and-bounded

## Discovery trigger

The crate documentation (`crates/tokenizer/src/lib.rs`) states that tiktoken's merge loop is quadratic in the byte length of one pre-token piece and that pieces above `MAX_PIECE_BYTES` are split before merging. That is a production invariant with a permitted divergence from the oracle, and neither of the first two records captured it.

## Evidence trail

- `MAX_PIECE_BYTES` is `4096` (`lib.rs:80`).
- `encode_bounded` (`lib.rs:133-152`) returns `CoreBPE::encode_ordinary` for text under the cap; otherwise it walks the pre-tokenizer matches, hands each span between over-long pieces to `CoreBPE` whole, and encodes each over-long piece as `char_chunks` of at most `MAX_PIECE_BYTES` bytes.
- `char_chunks` (`lib.rs:114-128`) steps back to the last `char` boundary at or before the cap, so a chunk never splits a multi-byte character; `char_chunks_respect_boundaries_and_cap` (`lib.rs:213`) asserts that directly.
- `estimate_tokens` (`lib.rs:158`) is `encode_ordinary(text).len()` with an early return for empty input.
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
