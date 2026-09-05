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
  reaches U+FEFF; the `nel-after-space` and `nel-runs` golden cases
  (`gen/gen-token-golden.ts:99-100`) reach U+0085.
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
- Findings: the macro expands to 25 code points (U+0009 through U+000D, U+0020,
  U+00A0, U+1680, U+2000 through U+200A, U+2028, U+2029, U+202F, U+205F, U+3000,
  U+FEFF); the test enumerates 16 positives and four negatives, so U+2001
  through U+2009 are asserted by nothing. The derivation test expands the same
  macro on both sides (`lib.rs:181` and `:65-75`), so it is invariant under
  edits to the macro body.
- Missing evidence: a literal-set assertion over all 25 members, queued in
  `fault-map.md`.
- Conclusion: Exercised is partial; the Check names the full set as a literal.

### Q: Would parity catch a drift on U+0085?

- Sources examined: `tests/token_golden.rs:101`, `gen/gen-token-golden.ts:97-100`,
  and `testdata/token-golden.json`.
- Findings: U+FEFF is in the corpus through the BOM test and the `zero-width`,
  `bom-leading`, and `bom-between-punct` golden cases (`gen-token-golden.ts:76`,
  `:101-102`); U+0085 through `nel-after-space` and `nel-runs`. U+1680 and
  U+2000 through U+200A appear in no golden case.
- Missing evidence: golden cases carrying the Zs-block members.
- Conclusion: a drift on U+FEFF or U+0085 fails parity; a drift on the twelve
  Zs-block members fails parity nowhere, and on the nine interior members fails
  nothing in the tree.
