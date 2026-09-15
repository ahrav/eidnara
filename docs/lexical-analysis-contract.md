# Lexical analysis and literal MATCH compilation

Status: proposed contract, implemented in `crates/retrieval/src/lexical/`.
Analysis contract epoch: `identifier-analysis.v1`.

This document states the rules the identifier analyzer and the MATCH compiler
apply. Indexing and querying both run the analyzer, and the FTS5 engine
tokenizes the analyzer's output on both sides. That is the parity mechanism:
the same identifier reaches the engine in the same form whether it is being
indexed or probed for. The engine's tokenizer defines the effective term; the
analyzer decides which characters stay adjacent and which conservative parts an
identifier contributes.

Numeric limits are not part of this contract. Callers supply `LexicalBounds`
with no default; a missing bound is a compile error, not an experimental value.

## Tokenizer, detail, and columns

- FTS5 tokenizer: `unicode61 remove_diacritics 2 tokenchars '_'`
  (`lexical::TOKENIZER`).
- Detail mode: `full` (`lexical::DETAIL`). A quoted atom the engine splits
  into more than one token is a phrase, and phrase queries need full detail.
  The `fts5vocab` oracle also needs full detail to report offsets.
- Two indexed columns, in order: `original` and `parts`
  (`lexical::ORIGINAL_COLUMN`, `lexical::PARTS_COLUMN`).
- One unindexed column, `occurrence_id`, so a probe result names its
  occurrence without a join. Unindexed columns take no part in matching.
- `lexical::fts5_table_args()` renders all of the above as the argument list
  of `CREATE VIRTUAL TABLE ... USING fts5(...)`. The `lexical` table in
  `search.sqlite` and every scratch oracle table use it, so neither can drift
  to another tokenizer or detail mode.
- No custom tokenizer, dictionary, prefix index, or n-gram index.

The engine folds case and removes Latin diacritics. Two inputs that fold to the
same term are therefore both candidates for one probe. A folded match is a
candidate only; it is never evidence that two byte strings are equal, and it
never grants access. Canonical validation decides eligibility.

## Atoms

An atom is a maximal run of token characters inside one segment. A character
is a token character when Rust classifies it as alphanumeric, when it is `_`,
or when it is a private-use code point. A combining diacritical mark in
`U+0300..=U+036F` continues a run but cannot start one.

This set is not identical to what `unicode61` treats as a token character.
The engine's Unicode tables are older than Rust's, and the engine treats every
code point its tables do not know, including symbols such as U+1F914, as a
token character, while Rust does not. Neither difference breaks parity,
because the engine only ever sees analyzer output.
The mark and private-use rules exist so that the analyzer never separates
what the engine would fold into one term: `cre\u0301me` and `crème` are one
engine term each, and both stay one atom.

Atoms never contain whitespace, `"`, or NUL, because none of them is a token
character. NUL therefore separates atoms like any other punctuation and is not
refused: a probe and a column text are built only from atoms and parts, so the
C-string reading the engine gives bound MATCH text never sees one.

Examples:

| Input | Atoms |
| --- | --- |
| `snake_case` | `snake_case` |
| `a.b.c` | `a`, `b`, `c` |
| `src/a-b/File.rs` | `src`, `a`, `b`, `File`, `rs` |
| `2024-01-02` | `2024`, `01`, `02` |
| `don't 3.14` | `don`, `t`, `3`, `14` |
| `a OR b` | `a`, `OR`, `b` |
| `foo` U+1F914 `bar` (no spaces) | `foo`, `bar` |
| `!!! ...` | none |

## Parts

Parts are the conservative subwords of one atom. A part boundary falls:

- at every `_`, which is dropped;
- between a lowercase letter and an uppercase letter;
- between a letter and a digit, in either direction;
- before the last uppercase letter of an uppercase run when two or more
  lowercase letters follow it.

The last rule gives `HTTPServer` the parts `HTTP` and `Server`. Its one-letter
tail guard keeps `IDs`, `URLs`, and `ABCd` whole, so plural acronyms do not
produce parts like `I` and `Ds`. Uppercase followed by lowercase never splits,
so `Ωmega` and `Über` stay whole. Letters without case and private-use
characters are letters for these rules and never split on their own, so
`日本語Server` stays whole while `日本語2024` splits at the digit.

A combining mark attaches to the character before it and is invisible to the
boundary rules. It shares that character's class, it never opens a part, and
it is not a letter when the acronym rule counts the lowercase tail. `e\u0301Bar`
and `éBar` therefore split identically, `IDs\u0301` stays whole exactly as `IDś`
does, `aE\u0301cd` gives `a` and `E\u0301cd` exactly as `aÉcd` gives `a` and
`Écd`, and a mark directly after `_` is dropped with the `_`. No part ever
begins with a combining mark.

Parts keep their original bytes. The engine folds them.

Parts are additive. When the parts of an atom are byte-identical to the atom
itself, the atom contributes nothing to the parts column. When the parts differ
from the atom, all of them are emitted, even when there is only one (`_foo_`
contributes `foo`). This is not effective-term deduplication: `x\u0305Y` emits
parts `x\u0305` and `Y`, and the engine indexes `x` and `y` in both columns.
Duplicates are retained; frequencies belong to the engine, and ranking must
not assume each atom contributes an effective term only once.

Parts are a flat sequence with no per-atom alignment. The number of parts never
exceeds the number of scalar values in the atom, so the input byte bound also
bounds the parts.

Examples:

| Atom | Parts |
| --- | --- |
| `HTTPServer` | `HTTP`, `Server` |
| `getHTTPResponse2xx` | `get`, `HTTP`, `Response`, `2`, `xx` |
| `XMLHttpRequest` | `XML`, `Http`, `Request` |
| `iOS` | `i`, `OS` |
| `sha256` | `sha`, `256` |
| `x86_64` | `x`, `86`, `64` |
| `E0308` | `E`, `0308` |
| `TLSv1` | `TLSv`, `1` |
| `ERR_CONN_REFUSED` | `ERR`, `CONN`, `REFUSED` |
| `ÜberServer` | `Über`, `Server` |
| `aE\u0301cd`, `aÉcd` | `a`, `E\u0301cd` and `a`, `Écd` |
| `IDs`, `IDs\u0301`, `IDś`, `ENOENT`, `ab`, `A` | none |

## Short terms

There is no length rule. One- and two-character atoms are complete tokens, and
a probe for `a` matches only the token `a`. Length never routes a request to
exact lookup: `exact::classify` decides `Intent::Direct` from an explicit
selector alone, and a bare `ab12` in prose is a lexical atom.

## Column texts

`Analysis::original_text()` is the atoms joined by single spaces and
`Analysis::parts_text()` is the parts joined by single spaces. Atoms and parts
never contain whitespace, so the join does not change what the engine
tokenizes. The index stores these two strings in the two columns.

## The index row

`search.sqlite` holds one `lexical` row per live occurrence, written in the
same transaction as the occurrence and deleted by
`retrieval::tombstone_occurrence` in the transaction that records its
tombstone; the primitive owns that deletion, so every tombstone path keeps the
invariant. The rowid is one of `lexical::rowids(occurrence_id)`: the four
sixty-four-bit words of the occurrence identifier with the sign bit cleared, in
identifier order. The row sits at the first word no other occurrence holds, so a
rebuild from fenced input and incremental application store equal rows whatever
order they see the occurrences in, unless two live occurrences share a word; then
the one stored second takes its next word, and the two orders may place the pair
differently. Identifiers are SHA-256 digests, so among N live occurrences a
shared first word has probability near N squared over 2 to the 64th, and an
adversary who can choose identifiers can produce a shared word with about 2 to
the 32nd trials but cannot exhaust another occurrence's four words without a
preimage. An occurrence whose four words are all held by other live occurrences
is refused as `ProjectionError::LexicalRowidCollision`, which names every
holder; the batch that would have stored it persists nothing. Within a batch,
tombstones remove their holders before new lexical rows are placed. If all four
holders remain live, an operator must retire a holder or rebuild from canonical
input that excludes one. `lexical::verify_rows` accepts a row at any of its
occurrence's words.

The lexical row follows liveness rather than the batch that stores the
occurrence: a record whose occurrence is live and has no row gains one, and a
record whose occurrence is tombstoned gains none, whether or not the occurrence
row itself was new. `batch_status` reads the same predicate, so a live record
without its row reports the batch as not applied and reapplying the batch
restores the row. A present row with different `original` or `parts` text is
corrupt, not an applied batch or a successful replay. Both paths compare those
columns with the selected payload analyzed under the current contract.

The index analyzes the occurrence's selected payload bytes under the payload
byte bound, which also bounds the atom count because every atom is at least one
byte. NUL is replaced by a space before analysis: the analyzer refuses NUL to
protect a bound probe, and indexed text is never bound as one. Equal-byte
occurrences are distinct rows; revising or deleting one leaves its sibling's row
in place.

The projection identity records `AnalysisIdentity::current()` in
`analysis_identity`. A projection whose stored identity differs from the current
one is incompatible and is rebuilt, exactly as a tokenizer or schema mismatch
is. `lexical::verify_rows` checks that live occurrences and lexical rows
correspond one to one and that both indexed columns equal the analyzed payload.
`PRAGMA integrity_check` verifies the inverted index against its own content.
The row check runs under one snapshot at reopen, construction verification, and
closed-seed certification. It scans every lexical row and reanalyzes each live
payload; `CoverageBounds` does not bound this work. `lexical::probe_engine`
checks FTS5, tokenizer availability, and agreement between the linked engine
and bundled version/source identity at open and closed-seed certification.
It reports the engine as `EngineIdentity`; a missing module, tokenizer, or
mismatched build is `ProjectionError::Unsupported`. `install_identity` and
`require_compatible` pin `analysis_identity` to this build's
`AnalysisIdentity::current()` the way they pin the schema version.

## What a request analyzes

`Intent::lexical_segments(request)` returns the slices of a classified request
that lie outside its selector mentions. Each slice is a separate segment, and a
segment boundary ends an atom, so removing a mention never glues its neighbours
into one atom. A `Direct` request is a single selector and has no lexical text.
`request` must be the string the intent was classified from. The byte bound
applies to the sum of the segments.

`analyze` is the index-side entry point and treats its whole input as one
segment.

## Compilation

`lexical::compile` maps an analysis to `Vec<Probe>`, one probe per atom, in
request order, duplicates retained. A zero-atom analysis yields no probe, so
there is nothing a caller could bind for it. The engine reports `MATCH ''` as an
error and `MATCH '""'` matches nothing, but the contract is that no MATCH is
issued at all.

Each `Probe` is the atom with every internal `"` doubled and the result
enclosed in `"`. Its text is reachable only through `ToSql`, so it is bound as
one SQL value to `... MATCH ?` and cannot be concatenated into SQL.

Inside an FTS5 quoted string, `AND`, `OR`, `NOT`, `NEAR`, `:`, `{`, `}`, `^`,
`*`, `+`, and `-` are text. The compiler generates no bareword, column filter,
connective, prefix operator, or NEAR. Probes compile from atoms only. Parts are
index-side, so a probe for `HTTPServer` matches an occurrence of `HTTPServer`
and not one that says `HTTP Server`, while a probe for `HTTP` matches both.

Duplicate atoms stay duplicate probes. Ranking, not compilation, reduces each
occurrence to its best probe; permuting or repeating probes must leave that
ranking unchanged. A probe's position in the vector is its ordinal.

## Bounds

`LexicalBounds` has two fields, both `NonZeroUsize` with no default:

- `max_input_bytes`: the summed byte length of all segments is compared against
  it before any character is scanned.
- `max_atoms`: atoms are counted as they are found; the atom after the last
  permitted one is refused before it is retained.

These input bounds do not override engine term limits. Bundled SQLite 3.51.3
caps each tokenized term at 32,768 bytes (`FTS5_MAX_TOKEN_SIZE`) on both insertion
and MATCH compilation. Longer terms with the same first 32,768 bytes therefore
collide, including with a term exactly that long, without a prefix operator.
The bound is on engine token bytes after folding, not input scalar values.
`tests/lexical_engine.rs` checks the boundary and the differing-suffix collision.
This is another recall-only equivalence, not byte identity. No extra refusal or
product limit is introduced here; choosing caller limits remains an integration
decision.

Refusals are checked in that order. On the query side the same approved
query-byte limit must feed both `SelectorBounds::max_input_bytes` and
`LexicalBounds::max_input_bytes`; the two fields are one limit applied at two
stages, not two limits.

### The client-side operand pre-check is not this contract

`packages/opencode-plugin/src/tools/eidnara-search/bounds.ts` refuses a search
request before it reaches the daemon when `countQueryAtoms` exceeds
`MAX_QUERY_ATOMS`. That count splits on `/[^\p{L}\p{N}_]+/u`, which is not the
atom rule above: it treats private-use code points, combining marks in
`U+0300..=U+036F`, and marks Rust classifies as alphabetic (an Indic vowel
sign, for example) as separators, while this analyzer keeps them inside an
atom. `cre\u0301me` is two operands there and one atom here; a lone private-use
character between spaces is zero operands there and one atom here. Neither
count bounds the other.

The plugin count is a coarse pre-check, not a second definition of an atom. The
analyzer owns the atom rule; a query that passes the pre-check can still be
refused with `TooManyAtoms`, and the pre-check must never be tightened on the
assumption that it matches this rule. The query lane that binds probes is
responsible for reconciling the two counts, either by dropping the pre-check or
by documenting it as an upper bound with a fixture shared between
`bounds.test.ts` and `tests/lexical_analysis.rs`.

`analyze` and `compile` are pure and perform no I/O, so they take no
`EvalBudget`. The caller checks the shared request budget at entry and never
creates or renews one for analysis.

## Analysis identity

`AnalysisIdentity::current()` is the SHA-256 of a length-delimited manifest:
the contract epoch, the toolchain's `char::UNICODE_VERSION`, the tokenizer
string, the detail mode, the column names in order, the linked SQLite version,
and the bundled SQLite source identity. Changing any of them changes the
identity, even when a given fixture
still analyzes to the same terms. The Unicode version is included because atom
and part boundaries come from the toolchain's character classification tables,
so a toolchain upgrade that changes those tables changes what the analyzer
emits. The SQLite version is included because `unicode61` folds and splits
analyzer output with the engine's own tables, so an engine upgrade can change
the effective terms of rows already indexed.

Every other rule in this document is covered by the epoch. A change to the atom
rule, the part rules, multiplicity, or the grammar requires a new epoch in the
same change as the code, the updated goldens, and the updated pinned digest in
`crates/retrieval/tests/lexical_analysis.rs`.

Because the projection identity pins `analysis_identity` to the running build,
a projection indexed under another SQLite version or source identity is
incompatible and is rebuilt. `probe_engine` checks that the running engine
matches the bundled version and source identity used in the manifest. Rebuild
from canonical input; do not relabel old rows with a new digest. Restoring a
prior binary requires a projection built under its prior identity.

## Known behaviour to keep in mind

- Case and diacritic folding widen recall: `RÉSUMÉ` finds `Résumé`. This is
  intended and is not identity evidence.
- The engine folds only simple single-code-point case mappings and removes
  only its own set of Latin combining marks. `ﬁle` and `file`, `Straße` and
  `STRASSE`, and full-width and ASCII letters are different terms.
- The engine treats some combining marks as separators that the analyzer keeps
  inside an atom (`x\u0305y`). The engine then indexes `x` and `y` as an
  adjacent pair and the probe becomes the same pair, so the query still
  matches. The same probe also matches any occurrence where `x` and `y` are
  adjacent terms, including adjacent parts of two different atoms in the parts
  column (`foo_x y_bar`). This is a false-positive direction, not a miss.
- An atom made only of characters the engine treats as separators (a lone
  Indic vowel sign, for example) produces a probe the engine turns into an
  empty phrase. Such a probe matches nothing; it does not widen the query.
- Plural and inflected acronyms stay whole, so a probe for `ID` does not match
  `IDs` through parts.
