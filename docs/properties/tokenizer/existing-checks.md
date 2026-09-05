# Tokenizer existing-check inventory

Every claim-bearing check for `crates/tokenizer`, re-inventoried against the post-merge HEAD in which the crate's runtime is a hand-written scanner and an in-crate BPE (`src/scan.rs`, `src/bpe.rs`) and the earlier regex-and-`tiktoken-rs` implementation survives only as the test-side reference (`src/reference_impl.rs`). Per-check status follows.
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
| `deterministic_across_calls` (`:64`) | Repeated calls on one input return identical token counts (`estimate_tokens`) | unaudited; cited by `tokenizer-encoding-is-deterministic-across-calls-and-threads` |
| `deterministic_across_threads` (`:73`) | Concurrent callers on the golden cases return identical ids; the `OnceLock` tokenizer is shared safely after initialisation (the cold-start race is schedule-dependent and no over-cap input is encoded) | unaudited; cited by `tokenizer-encoding-is-deterministic-across-calls-and-threads` |
| `bom_before_newline_is_preserved` (`:101`) | `"x\u{feff}\n"` encodes to `[x, bom, newline]`, diverging from the stock oracle's BOM-stripping lookup | unaudited; cited by `tokenizer-bom-is-its-own-token` and the scope exception in `catalog.md` |
| `over_long_piece_is_chunked_and_bounded` (`:116`) | A letter run above `MAX_PIECE_BYTES` encodes as the concatenation of its chunk encodings and the count matches | unaudited; cited by `tokenizer-over-long-pieces-are-chunked-and-bounded` |
| `over_long_cjk_piece_keeps_char_boundaries` (`:134`) | A multi-byte run above the cap encodes without splitting a codepoint and yields a plausible count | unaudited; cited by same record |
| `long_text_without_over_long_piece_is_unaffected_by_bound` (`:150`) | Text above the cap with no over-long piece encodes identically to its per-sentence encoding | unaudited; cited by same record |

## In-crate unit tests

### `crates/tokenizer/src/lib.rs` - 4 tests

| Test | Claim asserted | Status |
| --- | --- | --- |
| `pattern_is_upstream_with_ecmascript_whitespace` (`:168`) | The test-only `CLAUDE_PAT_STR` equals `assets/claude.pat` with `\s`/`\S` rewritten to the ECMAScript whitespace class | unaudited; cited by `tokenizer-pattern-is-upstream-with-ecmascript-whitespace` |
| `reference_pattern_equals_upstream_derived_pattern` (`:183`) | `reference_impl::CLAUDE_PAT_STR`, the pattern the reference compiles, equals the derived constant | unaudited; cited by `tokenizer-pattern-is-upstream-with-ecmascript-whitespace` |
| `whitespace_class_matches_ecmascript_not_unicode_white_space` (`:188`) | The whitespace class includes U+FEFF and excludes U+0085 | unaudited; cited by `tokenizer-pattern-is-upstream-with-ecmascript-whitespace` |
| `char_chunks_respect_boundaries_and_cap` (`:209`) | `char_chunks` never exceeds the cap and never splits a `char` | unaudited; cited by `tokenizer-over-long-pieces-are-chunked-and-bounded` |

### `crates/tokenizer/src/parity_tests.rs` - 4 tests

Property tests against `src/reference_impl.rs`, 2,000 cases per property by default (`cases()`, `:45-57`; `PROPTEST_CASES` overrides, zero rejected). The reference is built from the same asset and pattern as the live crate, so these detect implementation drift between the two, not asset or pattern drift shared by both.

| Test | Claim asserted | Status |
| --- | --- | --- |
| `ids_match_reference_impl` (`:63`) | `encode_ordinary(text)` equals the reference's ids on generated text from ten strategy arms (`text_strategy`, `:11-25`) | unaudited; cited by `tokenizer-encoding-matches-the-independent-oracle`, `tokenizer-over-long-pieces-are-chunked-and-bounded`, `tokenizer-pattern-is-upstream-with-ecmascript-whitespace`, `tokenizer-encoding-is-total-over-valid-utf8` |
| `count_equals_encode_len` (`:68`) | `estimate_tokens(text) == encode_ordinary(text).len()` and equals the reference's count | unaudited; cited by `tokenizer-encoding-matches-the-independent-oracle`, `tokenizer-encoding-is-deterministic-across-calls-and-threads`, `tokenizer-encoding-is-total-over-valid-utf8` |
| `concat_at_piece_boundary` (`:78`) | Splitting after a non-whitespace piece and encoding the halves separately yields the same ids as the whole | unaudited |
| `encode_is_pure_across_threads` (`:95`) | Eight threads re-encoding 2,000 texts agree with the calling thread's encodings and counts | unaudited; cited by `tokenizer-encoding-is-deterministic-across-calls-and-threads` |

### `crates/tokenizer/src/vocab_blob_tests.rs` - 1 test

| Test | Claim asserted | Status |
| --- | --- | --- |
| `vocab_blob_matches_claude_tiktoken` (`:7`) | Every row of the committed asset has a unique rank below both sentinels and resolves to that rank through the live lookup tables, and the table sizes sum to the row count | unaudited; cited by `tokenizer-vocabulary-is-embedded-and-complete` and `tokenizer-encoding-is-total-over-valid-utf8` |

### `crates/tokenizer/src/scan.rs` - 3 tests

| Test | Claim asserted | Status |
| --- | --- | --- |
| `bmp_table_matches_range_tables` (`:270`) | The precomputed BMP class table agrees with `class_from_tables` for every code point it covers | unaudited |
| `swar_masks_match_ascii_table` (`:279`) | The word-at-a-time ASCII class masks agree with `ASCII_CLASS` byte by byte | unaudited |
| `matches_reference_on_hand_cases` (`:302`) | The scanner's piece splits equal the reference regex's match spans on hand-written cases | unaudited; cited by `tokenizer-pattern-is-upstream-with-ecmascript-whitespace` |

### `crates/tokenizer/src/bpe.rs` - 2 tests

| Test | Claim asserted | Status |
| --- | --- | --- |
| `heap_and_scan_engines_agree` (`:381`) | The rescan and heap merge engines produce the same ids for the same piece | unaudited; cited by `tokenizer-encoding-is-deterministic-across-calls-and-threads` |
| `engine_crossover` (`:425`) | Timing probe for the engine threshold | unaudited; ignored (`timing probe`), not a bound |

### `crates/tokenizer/src/unicode_gen_tests.rs` - 2 tests

| Test | Claim asserted | Status |
| --- | --- | --- |
| `regenerate_unicode_tables` (`:63`) | Rewrites `src/unicode_tables.rs` from `regex-syntax` | ignored (`writes src/unicode_tables.rs`); a generator, not a check |
| `unicode_tables_match_regex_syntax` (`:68`) | The committed `\p{L}` and `\p{N}` tables equal what `regex-syntax` would generate | unaudited; cited by `tokenizer-pattern-is-upstream-with-ecmascript-whitespace` |

## Generator-time and build-time checks

`crates/tokenizer/gen/gen-claude-vocab.ts` writes `assets/claude.tiktoken` and
aborts on duplicate ranks (`:47`) or duplicate token byte sequences (`:51`), and
requires all 256 single-byte tokens. These run only when the asset is
regenerated; `vocab_blob_matches_claude_tiktoken` re-asserts them against the
committed asset and the runtime tables.

`crates/tokenizer/build.rs` decodes the asset into `$OUT_DIR/vocab.bin` and
panics on a malformed line, invalid base64, a rank that is not a `u32`, or a
token longer than `u16::MAX` (`:25-30`); these fire at build time.

`crates/tokenizer/gen/gen-token-golden.ts` produces `testdata/token-golden.json`
from `ai-tokenizer@1.0.6` with a null-prototype encoder copy, so the golden
pins the corrected prototype-name ids rather than the stock oracle's.

## Production assertions and runtime guards

`Vocab::from_blob` (`src/bpe.rs:116-150`) panics on first use if the blob has
trailing bytes or if any byte lacks a single-byte token (`:144-148`).
`encode_piece` carries two `debug_assert!`s, on span validity and on pieces of
at least two bytes (`:206`, `:218`), which are absent from release builds.

## Suspiciously quiet areas

- No test reaches `encode_ordinary` or `estimate_tokens` from outside the crate,
  because no workspace crate depends on `tokenizer` at U3.
- No benchmark bounds the worst-case latency the chunking exists to bound; the
  bound is asserted structurally, not by timing, and `engine_crossover` is an
  ignored probe.
- No fuzz target reaches the scanner or the merge engines; the only random
  inputs are the proptest arms, capped at a few thousand bytes.
- Every parity property compares against a reference that shares the asset and
  the whitespace macro with the live crate, so a drift made to both passes.
- CI (`.github/workflows/ci.yml:118`) runs `cargo test --workspace`, which
  includes these tests on Rust 1.98, but no CI step regenerates the golden or
  the asset, so drift between the committed asset and the generator's checks
  would not be caught.
