# Assignment false positives: saved work

This branch holds the work that pull request #1239 set aside when it became additive.
It is a holding branch with no pull request. Its tip is commit 7ad6fb586 of
`redaction/flag-and-url-credentials` plus this file.

## What the branch changes in rules that `main` has

`main` reports two classes of text under `magic-keyed-assignment` that are ordinary
program text:

1. A comparison. `key === "Enter"`, `pkg==1.2`, and a `=====` banner each match
   `key = value` with a value that begins with a second `=`. `main` replaces the operator.
2. The word after an empty assignment. `API_TOKEN=` above `DATABASE_URL=...` replaces the
   whole next line; `api_token= next word` replaces `next`.

The branch edits the keyed rule and splits a `magic-detached-assignment` rule from it:

- a value begins with a character other than `=`;
- a value follows `=` directly, or after blanks on the line of the `=` when a separator
  precedes the `=`;
- a value detached from its `=` (blanks alone, or a line break) is reported when it ends its
  line or a delimiter follows it; a word that another word follows, an assignment, a
  comment, a bracketed header, or punctuation stays as written;
- the reported span of an unquoted value runs through `;` to the end of its line
  (`KEY=ab;cd`), read at most 1,024 bytes past the value;
- an assignment below an empty one is read with its own key.

The evaluator carries the line gate (`value_ends_its_line`), the `;` continuation
(`continued_past_semicolons`), the resumed search (`resume_after`,
`follows_an_empty_assignment`), and the structural-word gate (`is_structural_line`,
`is_assignment`). The differential test `crates/secret-scanner/src/evaluator/monotonicity.rs`
names six exemptions from "every span `main` reports is reported here".

## The `==` audit

Corpus: 62,465 deduplicated units of real developer text, 188.9 million characters (tool
outputs, shell commands, person-typed messages). The branch removes 778 of `main`'s 7,942
findings.

- 774 have a value that begins with a second `=`. The other side of the operator is a quoted
  literal in 538 (110 distinct literals) and unquoted in 236 (identifier, keyword, number,
  member path, version pin, banner).
- 4 are empty assignments: `KEY=   # comment` twice, `token= None, ...`, and a wrapped
  expression.

Every quoted literal was read:

| Class | Count | Examples |
|---|---|---|
| real credential shape | 0 | |
| placeholder | 3 | one f-string naming a variable, three occurrences |
| ordinary code | 535 | keyboard keys (`Escape`, `q`), type names (`number`), field and option names, lexer tokens, banner text |

On `main` the finding of `key == "literal"` is the `==` itself: the value class stops at the
quote, so `main` replaces the operator and leaves the literal in place.

## Why it left the pull request

A redactor keeps every finding it had. Each edit to the keyed rule opened a family of inputs
that `main` redacts and the branch leaves in the text. Five review rounds found such inputs
each time. The pull request now adds rules and leaves `main`'s rules as they are.

## Open findings against this branch (review round 5, head 7ad6fb586)

Each was reproduced against builds of `main` and of this branch. `VALUE` stands for a
credential, `sp` for ASCII spaces.

`main` redacts the value and the branch leaves it:

| Id | Input | Cause |
|---|---|---|
| C1 | `db.password` + U+00A0 + `=`, line break, `keycloak.url = https://host/auth?client_secret=VALUE`; also with a bullet, a backslash, or more than 64 bytes before the lower key | `follows_an_empty_assignment` reads ASCII blanks and a 64-byte prefix |
| C2 | `api_token= VALUE` + 1,019 to 1,022 sp + a serialized `\u000a` + `next` | the line gate's 1,024-byte window cuts the escape |
| C5 | `password= VALUE;x9 \u2713 done` | the continuation reads any `\u` escape as a line end; the line gate reads `\u000a` and `\u000d` only |
| C6 | `{"msg":"password= VALUE ","user":"bob"}` | a padded closing quote before more punctuation and text |
| C7 | `/* password= VALUE */`, `<!-- password= VALUE -->`, `[password= VALUE ,]` | a padded punctuation word |
| C17 | `curl -d \"password= VALUE\" https://host/v1` | a value closed by an escaped quote before another word |
| C15 (disputed) | `password= 900s\nnext` in serialized text | a duration before an escaped line break read as a scalar head |

Extent and test findings on the edited rules:

- `password=ab;cd` + 1,020 sp + a serialized `\n` + `next` reports the head only (as on `main`).
- `monotonicity.rs` accepts a mislabelled exemption row (`shows` checks syntax per label, lines 283-294), its oracle cuts at the first backslash (line 349), and no mutant covers an escape pair on the continuation bound.

Suggested fixes recorded with the findings: a character-aware key and `=` read with a
1 KiB bound for C1; blanks within 8 bytes of the gate's bound count as the line end for C2;
the continuation matches `u000a` and `u000d` exactly for C5; a quote after blanks closes
when the next byte is no word byte for C6; a punctuation-only word after blanks is a closer
for C7; a detached run ending in an escaped quote is closed for C17.

## What a later pull request needs

- A differential test with a stated exemption list, generated inputs, and the corpus run,
  as #1239 has for its additive rules.
- A decision that removing findings from a redactor is acceptable for the two classes, with
  the audit above as evidence for the comparison class.
