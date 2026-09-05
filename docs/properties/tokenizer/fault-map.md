# Tokenizer fault map

Fault classes the three tokenizer records need, what is available today, and
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
| T1 vocabulary or pre-tokenizer drift | A rank, token byte sequence, or pattern change that keeps counts equal but changes ids | **Partial** - the committed golden was produced by the oracle, not the crate, so drift on any of the 606 token ids or pattern branches the 46 cases exercise fails `encode_ordinary_matches_ai_tokenizer_ids`; the other 64,389 vocabulary entries and unexercised pattern branches are not covered |
| T2 corrupted embedded asset | A truncated asset, a duplicated rank, or a duplicated token byte sequence under a new rank | **Partial** - `gen-claude-vocab.ts` refuses to write such an asset (`:47`, `:51`); nothing checks the committed asset after the fact, and a byte-sequence duplicate would silently replace a rank in the `FxHashMap` build |
| T3 over-long pre-token piece | A single letter or CJK run longer than `MAX_PIECE_BYTES` | **Yes** - `over_long_piece_is_chunked_and_bounded` and `over_long_cjk_piece_keeps_char_boundaries` construct it |
| T4 oracle defect input | A pre-token equal to an `Object.prototype` member name, or a byte slice starting with a UTF-8 BOM | **Yes** - `valueof-short` and the prototype-name cases are in the golden via the null-prototype encoder; `bom_before_newline_is_preserved` pins the BOM case |
| T5 worst-case latency | A long unpunctuated run that would take seconds under an unbounded merge | **No** - the bound is asserted structurally (chunk count and concatenation equality); no test or benchmark measures time |

## Per-property required faults

| Property | Required faults and enabling state | Non-vacuous today |
| --- | --- | --- |
| tokenizer-encoding-matches-the-independent-oracle | T1, with a golden the crate did not produce | Partial - 46 oracle-produced cases covering 606 distinct ids |
| tokenizer-vocabulary-is-embedded-and-complete | T2 against the committed asset | No - only the generator checks, at generation time |
| tokenizer-over-long-pieces-are-chunked-and-bounded | T3 and T5 | Partial - T3 yes, T5 no |
| tokenizer-pattern-is-upstream-with-ecmascript-whitespace | T1 as pattern drift: an edit to `assets/claude.pat` or to `ecmascript_whitespace!` that moves piece boundaries | Yes - `pattern_is_upstream_with_ecmascript_whitespace` derives the constant from the asset; `whitespace_class_matches_ecmascript_not_unicode_white_space` pins U+FEFF in and U+0085 out |
| tokenizer-bom-is-its-own-token | T1 or T2 on the `EF BB BF` row: a renumbered or merged BOM rank, or a BOM-stripping lookup | Partial - `bom_before_newline_is_preserved` reaches the input but derives its expectation from the crate; the asset-rank oracle in the record is not yet a test |
| tokenizer-encoding-is-deterministic-across-calls-and-threads | Concurrent first callers racing the `OnceLock` initialisation; repeated calls on one input | Yes - `deterministic_across_calls` and `deterministic_across_threads` construct both |
| tokenizer-encoding-is-total-over-valid-utf8 | T5 shaped to backtrack: an input above `MAX_PIECE_BYTES` whose whitespace and non-whitespace alternation stresses the negative lookahead | No - no test or fuzz target reaches the `expect` at `lib.rs:140` |

## Coverage checks to add

0. A generated corpus that exercises every vocabulary entry at least once, or
   a property test against a live oracle, so T1 covers ranks the hand-written
   corpus never reaches.
1. A Rust test that parses `assets/claude.tiktoken` and asserts unique ranks,
   unique decoded token byte sequences, and 256 single-byte tokens, so T2 is
   checked against the committed asset rather than trusted from the generator.
2. A bounded-time assertion or Criterion benchmark for a run of
   `MAX_PIECE_BYTES * 8` letters, so T5 has an oracle beyond structure.

## Ranking by cheapest valid oracle

1. The asset test (T2): cheapest, closes the only record with no Rust check.
2. The latency bound (T5): needs a benchmark harness and a chosen budget.
