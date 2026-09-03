# Tokenizer property catalog

Records for `crates/tokenizer`, discovered at U3; the source had no catalog for this crate. `index.json` is generated
from this file; the record contract is [`../METHOD.md`](../METHOD.md).

## Provenance and scope

- Discovery at U3 against `host@39e8230`. The crate is a port of the `ai-tokenizer` claude encoding; its only
  contract is bit-faithfulness to that oracle.

## Records

### tokenizer-encoding-matches-the-independent-oracle

Type: safety
Reachability: default-production - every budget estimate encodes through this table.
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
Reachability: default-production - the vocabulary is read at build time.
Status: active
Exercised: partial - the generator rejects duplicate ranks and requires all 256 single bytes; no Rust test asserts the embedded asset's completeness.
Guarantee: The embedded `claude.tiktoken` asset has unique ranks and covers every single byte, so every input encodes without a fallback.
Check: `always` - ranks are unique and 256 single-byte tokens exist in the asset.
Fault/timing angle: A truncated or duplicated asset makes some bytes unencodable.
Required faults and enabling state: None; static asset check.
Confidence: medium - `gen/gen-claude-vocab.ts` enforces both conditions when it writes the asset; the Rust side trusts the asset.
Existing check: The generator's checks; no Rust check; unaudited.
Impact: Encoding panics or silently drops bytes.
Open questions: None.
