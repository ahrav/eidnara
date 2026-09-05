# Tokenizer property catalog

Records for `crates/tokenizer`, discovered at U3; the source had no catalog for this crate. `index.json` is generated
from this file; the record contract is [`../METHOD.md`](../METHOD.md).

## Provenance and scope

- Discovery at U3 against `host@39e8230`. The crate is a port of the `ai-tokenizer` claude encoding. Its contract is
  bit-faithfulness to that oracle for every pre-token piece of at most `MAX_PIECE_BYTES`, plus bounded merge work for
  longer pieces, whose ids may differ from the oracle at chunk seams (`crates/tokenizer/src/lib.rs`, crate docs).
- No workspace crate depends on `tokenizer` and nothing outside `crates/tokenizer` calls `encode_ordinary` or
  `estimate_tokens`; the workspace is `publish = false`. Every record is therefore `test-only` at HEAD. Reclassify to
  `default-production` in the wave that adds the first production caller.

## Records

### tokenizer-encoding-matches-the-independent-oracle

Type: safety
Reachability: test-only - no workspace crate depends on `tokenizer`; only `crates/tokenizer/tests/token_golden.rs` and the crate's unit tests call `encode_ordinary` and `estimate_tokens`.
Status: active
Exercised: yes - 36 golden cases regenerated from the `ai-tokenizer` oracle pass, and a token-count estimate is checked against them.
Guarantee: `encode_ordinary` produces the same token ids as `ai-tokenizer`'s claude encoding for every golden case, and `estimate_tokens` returns their count.
Check: `always` - `encode_ordinary(text) == golden.ids` for every case; `estimate_tokens(text) == golden.ids.len()`.
Fault/timing angle: A vocabulary or pre-tokenizer divergence that keeps counts equal but changes ids.
Required faults and enabling state: The golden corpus, produced by the oracle and not by the crate.
Confidence: high - [evidence](evidence/tokenizer-encoding-matches-the-independent-oracle.md). The corpus texts that named the predecessor were replaced at U3 and the golden regenerated once with `gen/gen-token-golden.ts` against the oracle.
Existing check: `encode_ordinary_matches_ai_tokenizer_ids`, `estimate_tokens_matches_golden_counts` (`crates/tokenizer/tests/token_golden.rs`); audited at U3.
Impact: Budget fitting over- or under-counts and a session overflows or truncates the context window.
Open questions: None.

### tokenizer-vocabulary-is-embedded-and-complete

Type: safety
Reachability: test-only - the asset is embedded with `include_str!` and read on first use, but no workspace crate depends on `tokenizer`, so only the crate's tests reach it.
Status: active
Exercised: partial - the generator rejects duplicate ranks and requires all 256 single bytes; no Rust test asserts the embedded asset's completeness.
Guarantee: The embedded `claude.tiktoken` asset has unique ranks and covers every single byte, so every input encodes without a fallback.
Check: `always` - ranks are unique and 256 single-byte tokens exist in the asset.
Fault/timing angle: A truncated or duplicated asset makes some bytes unencodable.
Required faults and enabling state: None; static asset check.
Confidence: medium - [evidence](evidence/tokenizer-vocabulary-is-embedded-and-complete.md). `gen/gen-claude-vocab.ts` enforces both conditions when it writes the asset; the Rust side trusts the asset.
Existing check: The generator's checks; no Rust check; unaudited.
Impact: Encoding panics or silently drops bytes.
Open questions: None.

### tokenizer-over-long-pieces-are-chunked-and-bounded

Type: safety
Reachability: test-only - every call to `encode_ordinary` and `estimate_tokens` goes through `encode_bounded`, but no workspace crate depends on `tokenizer`, so only the crate's tests reach it.
Status: active
Exercised: yes - three tests build inputs above and around `MAX_PIECE_BYTES`: a letter run three caps long, a CJK run that would split mid-codepoint if chunking were byte-based, and a long text with no over-long piece.
Guarantee: A pre-token piece longer than `MAX_PIECE_BYTES` (4096) is split at character boundaries into chunks of at most that size before BPE merging, so merge work is `O(len * MAX_PIECE_BYTES)` rather than quadratic in the piece; spans with no over-long piece encode exactly as the oracle does, and the ids of a chunked piece equal the concatenation of its chunks' ids, which may differ from the oracle only at chunk seams.
Check: `always` - for an over-long piece, `encode_ordinary(text)` equals the concatenation of `encode_ordinary` over the prefix, each chunk of at most `MAX_PIECE_BYTES` bytes, and the suffix; every chunk boundary is a `char` boundary; for text with no over-long piece, `encode_ordinary(text)` equals the piece-by-piece encoding; and `estimate_tokens(text) == encode_ordinary(text).len()` in every case.
Fault/timing angle: A change that removes or raises the cap restores tiktoken's quadratic merge loop, so one long unpunctuated run (37k CJK characters) takes seconds; a change that chunks by byte splits a multi-byte character and panics or corrupts ids.
Required faults and enabling state: An input whose pre-token piece exceeds `MAX_PIECE_BYTES`; the ` ?\p{L}+` pattern makes any long letter run such a piece.
Confidence: high - [evidence](evidence/tokenizer-over-long-pieces-are-chunked-and-bounded.md). `encode_bounded` and `char_chunks` (`crates/tokenizer/src/lib.rs`) were read directly, and the three tests construct the over-long, multi-byte, and unaffected cases.
Existing check: `over_long_piece_is_chunked_and_bounded`, `over_long_cjk_piece_keeps_char_boundaries`, `long_text_without_over_long_piece_is_unaffected_by_bound` (`crates/tokenizer/tests/token_golden.rs`); audited at U3.
Impact: Worst-case encoding latency regresses from linear to seconds, or a campaign demands oracle equality for a long unpunctuated input that the crate deliberately does not promise.
Open questions: None.
