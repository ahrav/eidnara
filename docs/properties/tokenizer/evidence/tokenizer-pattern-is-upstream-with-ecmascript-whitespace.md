# tokenizer-pattern-is-upstream-with-ecmascript-whitespace

## Discovery trigger

The tokenizer portfolio evaluation found two tests attached to no record:
`pattern_is_upstream_with_ecmascript_whitespace` and
`whitespace_class_matches_ecmascript_not_unicode_white_space`
(`crates/tokenizer/src/lib.rs:179`, `:192`). They pin the pre-tokenizer pattern
every other tokenizer record depends on.

## Evidence trail

- `crates/tokenizer/src/lib.rs:10-13` (crate docs): the pattern rewrites
  upstream `\s` and `\S` as explicit classes because ECMAScript includes U+FEFF
  and excludes U+0085 while the `regex` crate does the reverse; `assets/claude.pat`
  holds the upstream pattern and a unit test derives `CLAUDE_PAT_STR` from it.
- `lib.rs:63-75`: `const CLAUDE_PAT_STR: &str = concat!(...)` built from the
  upstream alternation with `ecmascript_whitespace!()` spliced into each class
  position.
- `pattern_is_upstream_with_ecmascript_whitespace` (`:179`) derives the pattern
  from the asset and asserts `derived == CLAUDE_PAT_STR` (`:186`), then asserts
  the constant contains neither `\s` nor `\S` (`:187-188`).
- `whitespace_class_matches_ecmascript_not_unicode_white_space` (`:192`) builds
  `^[class]$` and asserts a match for sixteen code points (`\t`, `\n`, U+000B,
  U+000C, `\r`, space, U+00A0, U+1680, U+2000, U+200A, U+2028, U+2029, U+202F,
  U+205F, U+3000, U+FEFF) and no match for U+0085, U+200B, `a`, U+180E
  (`:195-210`).
- `bom_before_newline_is_preserved` (`tests/token_golden.rs:101`) encodes `"x\u{feff}\n"` and so
  reaches U+FEFF through the golden corpus; no golden case is known to contain
  U+0085.
- No workspace crate depends on `tokenizer`, per the other three records'
  reachability analysis.

## Failure scenario

An edit to `assets/claude.pat`, to `ecmascript_whitespace!`, or a change in the
`regex` crate's class semantics moves piece boundaries. Token counts stay
plausible while ids change on inputs containing the code points where ECMAScript
and Unicode `White_Space` disagree; parity with the oracle passes on every golden
case that lacks them.

## Timing windows and dependencies

None; a static property of a constant.

## What a test must construct

The derivation from the asset, the absence of unexpanded `\s`/`\S`, and the
membership table for the class; already present as the two tests.

## Investigation log

### Q: Is the class asserted as a complete set or as a sample?

- Sources examined: `lib.rs:192-210`.
- Findings: sixteen positive and four negative code points are enumerated; the
  U+2001 through U+2009 range is covered by construction of the class but only
  its endpoints are asserted.
- Missing evidence: none needed for the guarantee as stated (the record names
  the enumerated points, not the full range).
- Conclusion: the check is written against the enumerated points.

### Q: Would parity catch a drift on U+0085?

- Sources examined: `tests/token_golden.rs:101` and the golden corpus description in
  `existing-checks.md`.
- Findings: U+FEFF is in the corpus; U+0085 is not known to be.
- Missing evidence: a scan of `assets` golden cases for U+0085.
- Conclusion: recorded as the open question.
