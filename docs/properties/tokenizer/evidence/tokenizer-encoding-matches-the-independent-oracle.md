# `tokenizer-encoding-matches-the-independent-oracle`

- **Discovery:** U3.
- **Primary evidence:** `crates/tokenizer/testdata/token-golden.json` is authored in this tree by `gen/gen-token-golden.ts`, which encodes the corpus with `ai-tokenizer@1.0.6` (a root dev dependency) and writes the ids; `tests/token_golden.rs` compares `encode_ordinary` and `estimate_tokens` against it for all 36 cases.
- **Existing evidence:** `encode_ordinary_matches_ai_tokenizer_ids`, `estimate_tokens_matches_golden_counts`, plus the two estimator tests in the same file; pass on Rust 1.98 and stable.
- **Failure scenario:** a vocabulary or pre-tokenizer divergence.
- **Timing window:** none.
- **Instrumentation:** none.
- **Audit verdict (U3):** pass. The oracle is the JavaScript package, not the crate; the corpus covers whitespace runs, digits, punctuation, code, JSON, paths, special-token substrings, several scripts, emoji with ZWJ, combining marks, zero-width characters, control characters, surrogate pairs, long runs, and a 40-line mixed blob.
- **Open-question log:** none.
