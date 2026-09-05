# Tokenizer existing-check inventory

Every claim-bearing check for `crates/tokenizer` at U3, with per-check status.
Every status is `unaudited`, as `../METHOD.md` requires: an existing check never
removes a property from the catalog, and adequacy verdicts belong to a separate
invariant-test review. Where a catalog record cites a test, the row says so; that
is a link, not a verdict.

## Rust integration tests

### `crates/tokenizer/tests/token_golden.rs` - 9 tests

| Test | Claim asserted | Status |
| --- | --- | --- |
| `encode_ordinary_matches_ai_tokenizer_ids` (`:24`) | For all 46 golden cases, `encode_ordinary(text) == golden.ids` | unaudited; cited by `tokenizer-encoding-matches-the-independent-oracle` |
| `estimate_tokens_matches_golden_counts` (`:47`) | For all golden cases, `estimate_tokens(text) == golden.ids.len()` | unaudited; cited by same record |
| `empty_text_is_zero` (`:59`) | `estimate_tokens("") == 0` | unaudited |
| `deterministic_across_calls` (`:64`) | Repeated calls on one input return identical ids | unaudited; cited by `tokenizer-encoding-is-deterministic-across-calls-and-threads` |
| `deterministic_across_threads` (`:73`) | Concurrent callers on one input return identical ids; the `OnceLock` tokenizer is shared safely | unaudited; cited by `tokenizer-encoding-is-deterministic-across-calls-and-threads` |
| `bom_before_newline_is_preserved` (`:101`) | `"x\u{feff}\n"` encodes to `[x, bom, newline]`, diverging from the stock oracle's BOM-stripping lookup | unaudited; cited by `tokenizer-bom-is-its-own-token` and the scope exception in `catalog.md` |
| `over_long_piece_is_chunked_and_bounded` (`:116`) | A letter run above `MAX_PIECE_BYTES` encodes as the concatenation of its chunk encodings and the count matches | unaudited; cited by `tokenizer-over-long-pieces-are-chunked-and-bounded` |
| `over_long_cjk_piece_keeps_char_boundaries` (`:134`) | A multi-byte run above the cap encodes without splitting a codepoint and yields a plausible count | unaudited; cited by same record |
| `long_text_without_over_long_piece_is_unaffected_by_bound` (`:150`) | Text above the cap with no over-long piece encodes identically to its per-sentence encoding | unaudited; cited by same record |

## In-crate unit tests

### `crates/tokenizer/src/lib.rs` - 3 tests

| Test | Claim asserted | Status |
| --- | --- | --- |
| `pattern_is_upstream_with_ecmascript_whitespace` (`:179`) | `CLAUDE_PAT_STR` equals `assets/claude.pat` with `\s`/`\S` rewritten to the ECMAScript whitespace class | unaudited; cited by `tokenizer-pattern-is-upstream-with-ecmascript-whitespace` |
| `whitespace_class_matches_ecmascript_not_unicode_white_space` (`:192`) | The whitespace class includes U+FEFF and excludes U+0085 | unaudited; cited by `tokenizer-pattern-is-upstream-with-ecmascript-whitespace` |
| `char_chunks_respect_boundaries_and_cap` (`:213`) | `char_chunks` never exceeds the cap and never splits a `char` | unaudited |

## Generator-time checks

`crates/tokenizer/gen/gen-claude-vocab.ts` writes `assets/claude.tiktoken` and
aborts on duplicate ranks (`:47`) or duplicate token byte sequences (`:51`), and
requires all 256 single-byte tokens. These run only when the asset is
regenerated; no Rust check re-asserts them against the embedded asset. They are
the only evidence for `tokenizer-vocabulary-is-embedded-and-complete`.

`crates/tokenizer/gen/gen-token-golden.ts` produces `testdata/token-golden.json`
from `ai-tokenizer@1.0.6` with a null-prototype encoder copy, so the golden
pins the corrected prototype-name ids rather than the stock oracle's.

## Production assertions and runtime guards

`tokenizer()` panics with a fixed message if a vocabulary line is malformed, its
token is not valid base64, or its rank is not a `u32`, and if `CoreBPE::new`
rejects the vocabulary or pattern. These fire on first use, not at build time.

## Suspiciously quiet areas

- No Rust test asserts the embedded asset's rank uniqueness, token-byte
  uniqueness, or single-byte coverage; the generator is trusted.
- No test reaches `encode_ordinary` or `estimate_tokens` from outside the crate,
  because no workspace crate depends on `tokenizer` at U3.
- No benchmark bounds the worst-case latency the chunking exists to bound; the
  bound is asserted structurally, not by timing.
- CI (`.github/workflows/ci.yml:118`) runs `cargo test --workspace`, which
  includes these tests on Rust 1.98, but no CI step regenerates the golden or
  the asset, so drift between the committed asset and the generator's checks
  would not be caught.
