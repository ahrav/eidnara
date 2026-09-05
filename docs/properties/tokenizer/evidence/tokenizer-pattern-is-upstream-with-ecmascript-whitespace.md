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
- `lib.rs:77-88`: `const CLAUDE_PAT_STR: &str = concat!(...)` built from the
  upstream alternation with `ecmascript_whitespace!()` spliced into each class
  position; both the constant and the macro (`:66-71`) are `#[cfg(test)]`, so
  no regex is compiled into the runtime.
- `src/scan.rs:252-263`: `pieces` is the runtime pre-tokenizer, calling
  `piece_end` (`:202-237`) per span; the whitespace class is a literal in
  `class_from_tables` (`:71`) and `ASCII_CLASS` (`:38`), and the letter and
  number classes come from `src/unicode_tables.rs`.
- `pattern_is_upstream_with_ecmascript_whitespace` (`:168`) derives the pattern
  from the asset and asserts `derived == CLAUDE_PAT_STR` (`:175`), then asserts
  the constant contains neither `\s` nor `\S` (`:176-177`).
- `reference_pattern_equals_upstream_derived_pattern` (`:183`) asserts
  `reference_impl::CLAUDE_PAT_STR` (`src/reference_impl.rs:23-33`) equals the
  derived constant; the reference compiles that constant at `:56`.
- `matches_reference_on_hand_cases` (`src/scan.rs:302`) compares the scanner's
  splits to the reference regex's spans on hand-written cases;
  `ids_match_reference_impl` (`src/parity_tests.rs:63`) compares ids on 2,000
  generated strings per run, whose whitespace arm (`:12`, `:17`) draws from all
  25 class members plus U+0085 and U+200B.
- `unicode_tables_match_regex_syntax` (`src/unicode_gen_tests.rs:68`) pins the
  committed `\p{L}` and `\p{N}` tables to `regex-syntax`.
- `whitespace_class_matches_ecmascript_not_unicode_white_space` (`:188`) builds
  `^[class]# tokenizer-pattern-is-upstream-with-ecmascript-whitespace

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
 and asserts a match for sixteen code points (`\t`, `\n`, U+000B,
  U+000C, `\r`, space, U+00A0, U+1680, U+2000, U+200A, U+2028, U+2029, U+202F,
  U+205F, U+3000, U+FEFF) and no match for U+0085, U+200B, `a`, U+180E
  (`:191-205`).
- `bom_before_newline_is_preserved` (`tests/token_golden.rs:101`) encodes `"x\u{feff}\n"` and so
  reaches U+FEFF; the `nel-after-space` and `nel-runs` golden cases
  (`gen/gen-token-golden.ts:99-100`) reach U+0085.
- No workspace crate depends on `tokenizer`, per the other three records'
  reachability analysis.

## Failure scenario

An edit to `assets/claude.pat`, to either `ecmascript_whitespace!` macro, to
the scanner's class literals or run logic, or a `unicode_tables.rs` regeneration
against a different Unicode version moves piece boundaries. Token counts stay
plausible while ids change on inputs containing the code points where ECMAScript
and Unicode `White_Space` disagree; parity with the oracle passes on every golden
case that lacks them.

## Timing windows and dependencies

None; a static property of a constant.

## What a test must construct

The derivation from the asset, the absence of unexpanded `\s`/`\S`, the
scanner's equivalence to the reference regex, and a literal membership set for
the class independent of the shared macro. Present: everything but the literal
set; the membership test pins sixteen of 25 members.

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

### Q: What does the record describe once the runtime has no pattern?

- Sources examined: `src/lib.rs:60-88`, `src/scan.rs`, `src/reference_impl.rs`,
  `src/parity_tests.rs`, `src/unicode_gen_tests.rs`.
- Findings: `CLAUDE_PAT_STR` is test-only at HEAD. The upstream derivation still
  anchors the chain, but the runtime subject is the scanner, tied to the pattern
  through the reference constant equality and the parity properties. The
  scanner's class literal is independent of the macro, so a macro-only or
  scanner-only drift fails parity; a drift made to the scanner and both macros
  is caught only by the sixteen-member literal test.
- Missing evidence: the 25-member literal set.
- Conclusion: the guarantee is restated over the scanner; confidence stays high
  and Exercised stays partial.
