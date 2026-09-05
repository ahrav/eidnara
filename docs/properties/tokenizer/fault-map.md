# Tokenizer fault map

Fault classes the seven tokenizer records need, what is available today, and
which check would make each record non-vacuous. The crate is pure computation
over an embedded asset, so the classes are input shapes and asset corruption,
not timing or process faults.

## Rules applied here

- Safety checks must hold for every input the class names, not only the golden
  cases.
- A check that only re-runs the golden corpus cannot detect a fault the corpus
  does not contain; each row below names the input the corpus lacks.

## Fault classes required

| Class | Description | Available today |
| --- | --- | --- |
| T1 vocabulary or pre-tokenizer drift | A rank, token byte sequence, or pattern change that keeps counts equal but changes ids | **Partial** - the committed golden was produced by the oracle, not the crate, so drift on any of the 606 token ids or pattern branches the 46 cases exercise fails `encode_ordinary_matches_ai_tokenizer_ids`; `ids_match_reference_impl` (`src/parity_tests.rs:63`) covers the other ranks and branches against `src/reference_impl.rs`, which shares the asset and pattern with the live crate, so it sees scanner or BPE drift but not asset or pattern drift |
| T2 corrupted embedded asset | A truncated asset, a duplicated rank, or a duplicated token byte sequence under a new rank | **Yes** - `vocab_blob_matches_claude_tiktoken` (`src/vocab_blob_tests.rs:7`) re-reads the committed asset, asserts rank uniqueness, resolves every row through the live tables, and requires the table sizes to sum to the row count; `from_blob` panics at load on trailing bytes or a missing single-byte token (`src/bpe.rs:144-148`) |
| T3 over-long pre-token piece | A single letter or CJK run longer than `MAX_PIECE_BYTES` | **Yes** - `over_long_piece_is_chunked_and_bounded` and `over_long_cjk_piece_keeps_char_boundaries` construct it |
| T4 oracle defect input | A pre-token equal to an `Object.prototype` member name, or a byte slice starting with a UTF-8 BOM | **Yes** - `valueof-short` and the prototype-name cases are in the golden via the null-prototype encoder; `bom_before_newline_is_preserved` pins the BOM case |
| T5 worst-case latency | A long unpunctuated run that would take seconds under an unbounded merge | **No** - the bound is asserted structurally (chunk count and concatenation equality); `engine_crossover` (`src/bpe.rs:425`) measures time but is ignored and asserts nothing |

## Per-property required faults

| Property | Required faults and enabling state | Non-vacuous today |
| --- | --- | --- |
| tokenizer-encoding-matches-the-independent-oracle | T1, with a golden the crate did not produce | Partial - 46 oracle-produced cases covering 606 distinct ids; the reference parity properties widen rank coverage without adding an oracle |
| tokenizer-vocabulary-is-embedded-and-complete | T2 against the committed asset | Yes - `vocab_blob_matches_claude_tiktoken` plus the `from_blob` load assertion |
| tokenizer-over-long-pieces-are-chunked-and-bounded | T3 and T5 | Partial - T3 yes, including cap-straddling proptest arms pinned to the reference chunker; T5 no |
| tokenizer-pattern-is-upstream-with-ecmascript-whitespace | T1 as pattern drift: an edit to `assets/claude.pat`, to either `ecmascript_whitespace!` macro, to the scanner's class literals or run logic, or to `unicode_tables.rs` | Partial - `pattern_is_upstream_with_ecmascript_whitespace` detects asset and `concat!` edits; `reference_pattern_equals_upstream_derived_pattern`, `matches_reference_on_hand_cases`, and `ids_match_reference_impl` tie the scanner to the reference pattern and see a scanner-only or macro-only drift; a drift made to the scanner and both macros passes everything except `whitespace_class_matches_ecmascript_not_unicode_white_space`, which pins 16 of 25 members and leaves U+2001 through U+2009 to the macro; the literal-set check in item 4 below is the missing case |
| tokenizer-bom-is-its-own-token | T1 or T2 on the `EF BB BF` row: a renumbered or merged BOM rank, or a BOM-stripping lookup | Partial - `bom_before_newline_is_preserved` reaches the input but derives its expectation from the crate; the asset-rank oracle in the record is not yet a test |
| tokenizer-encoding-is-deterministic-across-calls-and-threads | Concurrent first callers racing the `OnceLock` initialisation; repeated calls on one input | Partial - `deterministic_across_calls` constructs the repeated case; `deterministic_across_threads` and `encode_is_pure_across_threads` construct concurrency after initialisation but not a reliable cold start (siblings in the same binary initialise `VOCAB` first); an isolated cold process is the missing case |
| tokenizer-encoding-is-total-over-valid-utf8 | Arbitrary input with emphasis on class boundaries, contraction prefixes, and pieces at the cap; the source tree's backtracking regex is gone | Partial - the proptest arms (`any::<String>()` among them) run the scanner and both merge engines on random input and fail on panic; no fuzz target or adversarial generator exists, and the `encode_piece` guards are debug-only |

## Coverage checks to add

0. A generated corpus that exercises every vocabulary entry at least once, or
   a property test against a live oracle, so T1 covers ranks the hand-written
   corpus never reaches.
1. Done at HEAD: `vocab_blob_matches_claude_tiktoken` parses
   `assets/claude.tiktoken` and asserts unique ranks, per-row resolution through
   the live tables, and the row count; `from_blob` asserts single-byte coverage.
2. A bounded-time assertion or Criterion benchmark for a run of
   `MAX_PIECE_BYTES * 8` letters, so T5 has an oracle beyond structure.
3. An adversarial no-panic oracle for `tokenizer-encoding-is-total-over-valid-utf8`:
   a `cargo fuzz` target over `encode_ordinary` asserting no panic, that the
   spans of `scan::pieces(t)` tile `t`, and
   `estimate_tokens(t) == encode_ordinary(t).len()`, so the scanner's index
   arithmetic and the merge engines have a check beyond the proptest arms.
   Promoting the two `debug_assert!`s in `encode_piece` to release assertions
   would make a scanner regression fail at the seam.
4. A literal-set assertion for the whitespace class: every one of the 25
   ECMAScript code points, written out rather than derived from
   `ecmascript_whitespace!`, so U+2001 through U+2009 stop being guarded by
   nothing.
5. A decode round-trip oracle: for every input, the byte sequences of
   `encode_ordinary(t)` concatenate to `t.as_bytes()`. Input-general, needs no
   fixture, and covers the ranks the golden corpus never reaches.

## Ranking by cheapest valid oracle

1. The asset test (T2): done at HEAD.
2. The whitespace literal set (item 4): one assertion, closes the last hole the
   shared macro leaves in the pattern record.
3. The decode round-trip oracle (item 5): the first input-general behavioural
   check in the crate; a candidate record, not only a test.
4. The adversarial no-panic oracle (item 3): the only action that can make the
   totality record non-vacuous.
2. The latency bound (T5): needs a benchmark harness and a chosen budget.
