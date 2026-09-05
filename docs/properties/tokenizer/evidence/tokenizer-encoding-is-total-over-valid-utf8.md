# tokenizer-encoding-is-total-over-valid-utf8

## Discovery trigger

The tokenizer portfolio evaluation (gap 3) counted six panic sites and singled
out the hot-path `expect` on a `fancy_regex` match error, reachable only for
inputs above the cap; no record owned totality.

## Evidence trail

- `crates/tokenizer/src/lib.rs:132-150`: `encode_bounded` returns
  `bpe.encode_ordinary(text)` when `text.len() <= MAX_PIECE_BYTES`
  (`:133-135`); otherwise it iterates `piece_regex().find_iter(text)` and
  unwraps each match with
  `m.expect("claude pattern hit fancy-regex's backtrack limit")` (`:138-139`)
  before any chunking.
- `lib.rs:80`: `pub const MAX_PIECE_BYTES: usize = 4096`.
- `lib.rs:65-75`: the pattern contains `]+(?![^`...`])`, a negative lookahead,
  which is why `fancy-regex` (`Cargo.toml:15`, version 0.17) is used rather
  than `regex`.
- `crates/tokenizer/tests/token_golden.rs:116`, `:134`, `:150`: the over-long
  tests use letter and CJK runs and assert chunk structure; none is shaped to
  stress backtracking.
- The evaluation records the corpus's longest whitespace run as 50 spaces
  (`gen-token-golden.ts:79`).

## Failure scenario

An input above 4096 bytes whose whitespace and non-whitespace alternation makes
the backtracking engine exceed its limit returns an error from `find_iter`,
which `expect` turns into a panic in the caller's thread.

## Timing windows and dependencies

None; deterministic per input.

## What a test must construct

A fuzz target or adversarial generator over inputs above the cap, asserting no
panic and `estimate_tokens(t) == encode_ordinary(t).len()`.

## Investigation log

### Q: Is the backtrack limit reachable under this pattern?

- Sources examined: the pattern, `encode_bounded`, the over-long tests.
- Findings: the panic site exists and is on the path for every over-cap input;
  no evidence either way on whether the limit is reachable.
- Missing evidence: a targeted search or fuzz run.
- Conclusion: confidence medium; recorded as the open question.
