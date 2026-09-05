# Tokenizer property catalog

Records for `crates/tokenizer`, discovered at U3; the source had no catalog for this crate. `index.json` is generated
from this file; the record contract is [`../METHOD.md`](../METHOD.md).

## Provenance and scope

- Discovery at U3 against `host@39e8230`. The crate is a port of the `ai-tokenizer` claude encoding. Its contract is
  bit-faithfulness to that oracle for every pre-token piece of at most `MAX_PIECE_BYTES`, with three deliberate
  exceptions documented in `crates/tokenizer/src/lib.rs` (crate docs): pieces longer than the cap are chunked and may
  differ from the oracle at chunk seams; a pre-token equal to an `Object.prototype` member name that is not a vocabulary
  entry (`valueOf`, `hasOwnProperty`, `isPrototypeOf`, `toLocaleString`, `propertyIsEnumerable`) is encoded as bytes
  where stock `ai-tokenizer@1.0.6` emits a function-valued "token"; and a candidate byte slice starting with a UTF-8
  BOM (`EF BB BF`) is scored with the BOM present where the oracle's `TextDecoder` strips it. The golden corpus pins the
  corrected prototype-name ids by running the oracle with a null-prototype encoder copy (`gen/gen-token-golden.ts`), and
  `bom_before_newline_is_preserved` (`crates/tokenizer/tests/token_golden.rs`) pins the BOM case against the crate's own
  single-character encodings; a campaign must not treat either divergence as a regression.
- No workspace crate depends on `tokenizer` and nothing outside `crates/tokenizer` calls `encode_ordinary` or
  `estimate_tokens`; the workspace is `publish = false`. Every record is therefore `test-only` at HEAD. Reclassify to
  `default-production` in the wave that adds the first production caller.

## Records

### tokenizer-encoding-matches-the-independent-oracle

Type: safety
Reachability: test-only - no workspace crate depends on `tokenizer`; only `crates/tokenizer/tests/token_golden.rs` and the crate's unit tests call `encode_ordinary` and `estimate_tokens`.
Status: active
Exercised: yes - 36 golden cases regenerated from the `ai-tokenizer` oracle pass, and a token-count estimate is checked against them.
Guarantee: `encode_ordinary` produces the same token ids as `ai-tokenizer`'s claude encoding for every golden case, and `estimate_tokens` returns their count. The oracle is the null-prototype-patched encoder described in Provenance and scope, so the two documented oracle defects are excluded from parity by construction.
Check: `always` - `encode_ordinary(text) == golden.ids` for every case; `estimate_tokens(text) == golden.ids.len()`. Parity is asserted against the committed golden, never against a live stock `ai-tokenizer`, so the prototype-name and BOM corrections cannot register as failures.
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
Exercised: partial - the generator rejects duplicate ranks, rejects duplicate token byte sequences, and requires all 256 single bytes; no Rust test asserts the embedded asset's completeness or uniqueness.
Guarantee: The embedded `claude.tiktoken` asset has unique ranks, unique token byte sequences, and covers every single byte, so every input encodes without a fallback and every byte sequence maps to exactly one rank.
Check: `always` - ranks are unique, decoded token byte sequences are unique, and 256 single-byte tokens exist in the asset. Byte-sequence uniqueness is load-bearing on the Rust side: `tokenizer()` (`crates/tokenizer/src/lib.rs`) inserts each decoded token into an `FxHashMap<Vec<u8>, Rank>`, so a duplicated sequence under a new rank would silently replace the earlier rank and change encoded ids while rank uniqueness and single-byte coverage still held.
Fault/timing angle: A truncated asset makes some bytes unencodable; a duplicated token sequence changes ids silently.
Required faults and enabling state: None; static asset check.
Confidence: medium - [evidence](evidence/tokenizer-vocabulary-is-embedded-and-complete.md). `gen/gen-claude-vocab.ts` enforces all three conditions when it writes the asset (duplicate ranks and duplicate token byte sequences each abort the write); the Rust side trusts the asset.
Existing check: The generator's checks; no Rust check; unaudited.
Impact: Encoding panics or silently drops bytes, or a duplicated token silently changes ids.
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
