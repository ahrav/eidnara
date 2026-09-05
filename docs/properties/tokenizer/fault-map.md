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
