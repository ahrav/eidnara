# tokenizer-encoding-is-total-over-valid-utf8

## Discovery trigger

The tokenizer portfolio evaluation (gap 3) counted six panic sites and singled
out the hot-path `expect` on a `fancy_regex` match error, reachable only for
inputs above the cap; no record owned totality. The post-merge rewrite removed
the regex from the runtime, so the record was re-derived against the scanner
and the in-crate merge engines that replaced it.

## Evidence trail

At HEAD:

- `crates/tokenizer/src/lib.rs:123-142`: `encode_bounded` iterates
  `scan::pieces(text)` (`:130`) and calls `vocab.encode_piece` on each span,
  chunked through `char_chunks` above `MAX_PIECE_BYTES` (`:131-139`). No
  regex, no `expect`, no `Result` on the path.
- `src/scan.rs:202-237`: `piece_end` indexes `bytes[pos]` (`:205`) under the
  caller's `pos < text.len()` contract and `bytes[pos + 1]`, `bytes[pos + 2]`
  behind `pos + 1 < n` and `pos + 2 < n` tests (`:209-213`);
  `whitespace_piece_end` (`:239-250`) walks back over UTF-8 continuation bytes
  to give the last whitespace character back.
- `src/bpe.rs:198-230`: `encode_piece` guards its span with
  `debug_assert!(start < end && end <= text.len())` (`:206`) and
  `debug_assert!(piece.len() >= 2)` (`:218`), then indexes `piece[0]` and
  `piece[1]` (`:219`); both guards compile out of release builds.
- `src/bpe.rs:116-150`: `from_blob` asserts the blob has no trailing bytes and
  that every byte has a token (`:144-148`); these run once, on first use.
- `src/parity_tests.rs:63`, `:68`: `ids_match_reference_impl` and
  `count_equals_encode_len` run 2,000 generated strings per property, with an
  `any::<String>()` arm (`:20`) and an arm mixing combining marks, emoji, and
  Arabic with whitespace (`:23`); a panic fails the property.
- No `cargo fuzz` target and no adversarial generator exist for the crate.

In the source tree this record was written against:

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
- The corpus's longest whitespace run is 51 spaces: `gen-token-golden.ts:79`
  repeats 50 after a literal space separator.
- `fancy-regex` 0.17 defaults `backtrack_limit` to 1,000,000, and the
  `(?!...)` at `lib.rs:70-72` forces its backtracking engine rather than
  delegation to `regex`, so the `expect` at `:140` is reachable in principle;
  every over-cap input passes through it once per match.

## Failure scenario

At HEAD: a scanner regression yields an empty, reversed, or non-`char`-aligned
span, or a merge engine indexes past a piece-relative offset, and the release
build panics on the slice or index in the caller's thread. In the source tree:
an input above 4096 bytes whose whitespace and non-whitespace alternation makes
the backtracking engine exceed its limit returns an error from `find_iter`,
which `expect` turns into a panic in the caller's thread.

## Timing windows and dependencies

None; deterministic per input.

## What a test must construct

A fuzz target or adversarial generator over arbitrary `&str`, with emphasis on
class boundaries, contraction prefixes, and pieces at exactly the cap, asserting
no panic, that the spans of `scan::pieces(t)` tile `t`, and
`estimate_tokens(t) == encode_ordinary(t).len()`. Present: the proptest arms,
which reach the scanner and both engines on random input up to a few thousand
bytes.

## Investigation log

### Q: Is the backtrack limit reachable under this pattern?

- Sources examined: the pattern, `encode_bounded`, the over-long tests.
- Findings: the panic site exists and is on the path for every over-cap input;
  no evidence either way on whether the limit is reachable.
- Missing evidence: a targeted search or fuzz run.
- Conclusion: confidence medium; recorded as the open question.

### Q: What replaces the `expect` as the panic surface at HEAD?

- Sources examined: `src/lib.rs:123-142`, `src/scan.rs:202-263`,
  `src/bpe.rs:198-352`, `src/parity_tests.rs`.
- Findings: the runtime has no fallible step; the remaining panic surfaces are
  slice and index operations in the scanner and the merge engines, guarded by
  explicit bounds tests in the scanner and by debug-only assertions in
  `encode_piece`. The proptest arms exercise them on random input and would
  fail on a panic, but nothing targets them.
- Missing evidence: a fuzz target; a release-build guard at the seam.
- Conclusion: the record stays active with Exercised partial and confidence
  medium; the open question moves from the backtrack limit to fuzzing and to
  promoting the debug assertions.
