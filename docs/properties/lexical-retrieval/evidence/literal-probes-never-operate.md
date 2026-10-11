# literal-probes-never-operate

## Discovery trigger

#391 AC1: literal probes. Request text never reaches FTS5 as query syntax;
every compiled probe is one quoted atom. The RP2.3 property bundle is
unavailable here, so the catalog reconstructs this record from the ticket's
acceptance criterion and verifies it against the #391 branch. The #967 PR
description maps AC1 to this record as met by existing tests.

## Evidence trail

Production code:

- `compile` (`crates/retrieval/src/lexical/compile.rs:23`) maps every atom
  of an `Analysis` to one `Probe` through `quote_atom`
  (`crates/retrieval/src/lexical/compile.rs:30`), which wraps the atom in
  double quotes and doubles every internal `"`. FTS5 treats a quoted string
  as a phrase, so `OR`, `NOT`, `NEAR`, `:`, `*`, `^`, and `-` inside it are
  phrase tokens.
- `Probe` (`crates/retrieval/src/lexical/compile.rs:14`) holds the quoted
  text privately and exposes it only through `ToSql`
  (`crates/retrieval/src/lexical/compile.rs:16`), so the text binds as an SQL
  value and never splices into statement text.
- Every engine statement binds the probe as `?1` after `MATCH`: `COUNT_SQL`
  (`crates/retrieval/src/lexical/retrieve.rs:256`), `RANKED_SQL`, `WHOLE_SQL`,
  and `COMMON_SQL` on the following lines, run by `count_probe`
  (`crates/retrieval/src/lexical/retrieve.rs:566`) and the scan functions.
- `analyze_segments` (`crates/retrieval/src/lexical/analysis.rs:78`) keeps
  only maximal runs of token characters, where `starts_atom`
  (`crates/retrieval/src/lexical/analysis.rs:113`) admits alphanumerics, `_`,
  and private-use code points. `"`, `(`, `)`, `*`, `:`, and `^` are
  separators, so no atom carries them and `quote_atom`'s doubling is a second
  guard rather than the first.
- The daemon compiles probes only through this path: `lexical_read`
  (`crates/daemon/src/query_route.rs:1075`) calls `compile`
  (`crates/daemon/src/query_route.rs:1097`) on the analysis of the request's
  prose segments.

Tests cited by the record, read at the branch:

- `literal_operators_are_terms` (`crates/retrieval/tests/lexical_engine.rs:196`)
  builds a scratch FTS5 table from `fts5_table_args` through `engine`
  (`:16`), fills it through `populated` (`:180`) with rows whose text is `a b`,
  `OR`, `NEAR`, `NOT`, `a`, `b`, and others, and asserts `probe_matches`
  (`:80`) for `a OR b`, `a NOT b`, and `NEAR(a b)`. Each request yields three
  probes, and each probe's rowid set equals the hand-written set for its atom:
  `OR` matches only row 2, `NOT` only row 10, `NEAR` only row 7.
- `removing_the_quotes_makes_operators_operate`
  (`crates/retrieval/tests/lexical_engine.rs:213`) is the control. It binds the
  raw request text through `matches` (`:71`) and asserts `a OR b` unions to
  `[1, 8, 9]`, `a NOT b` subtracts to `[8]`, and `NEAR(a b)` constrains to
  `[1]`, so the operator rows are lost and the operators act.
- `a_probe_matches_only_its_term_and_operators_in_text_stay_literal`
  (`crates/retrieval/tests/lexical_retrieval.rs:551`) runs `retrieve` on a
  projection built by `Fixture::all_admitted` (`:317`) over the `corpus`
  (`:228`), whose rows hold `fetch OR fallback parse` and `NEAR miss on a
  fetch path`. It asserts `parse` returns alpha, beta, gamma, and zeta with
  their classes, `OR` returns only beta, `NEAR` only epsilon, `io` only delta,
  `server` only alpha, and `absent` nothing.
- `atoms_never_contain_quotes` (`crates/retrieval/tests/lexical_analysis.rs:236`)
  analyzes `say "OR" ""a""`, asserts the atoms are `say`, `OR`, `a` with no
  `"` in any atom, and reads the bound probe text through `bound` (`:39`) as
  `"say"` and `"OR"`.

Supporting test outside the record's list:

- `syntax_shaped_input_is_ordinary_atoms`
  (`crates/retrieval/tests/lexical_analysis.rs:170`) analyzes
  `{original}: "x" ^y z*` to the atoms `original`, `x`, `y`, `z`, which covers
  the column filter, caret, and prefix star the catalog names.

## Failure scenario

A request such as `alpha OR beta` reaches the engine as the expression
`alpha OR beta` and unions two term sets, or `NEAR(a b)` narrows to adjacent
pairs, or `"foo"*` expands to every term with that prefix. Retrieval widens or
narrows past the request's literal terms, with no refusal and no counter
change, and a crafted request reads occurrences it never named.

## Timing windows and dependencies

None. Compilation is a pure function of the analysis, and the probe binds as
a value on every statement; no interleaving changes the text the engine sees.

## What a test must construct

- An FTS5 table built from `fts5_table_args`, populated with rows whose text
  is itself an operator word (`OR`, `NOT`, `NEAR`) alongside ordinary terms.
- Request text holding `OR`, `NOT`, `NEAR`, parentheses, quotes, a column
  filter, `^`, and `*`.
- A hand-written expected rowid set per atom, never derived from the engine.
- A control that binds the same request text unquoted and shows the operators
  acting, so the quoted assertion is known to be discriminating.
- For the production path, a projection with a kernel whose decisions admit
  the corpus, so `retrieve` returns contributions rather than exclusions.

## Investigation log

### Q: Is the guarantee exercised on the production retrieve path, or only on a fixture engine?

- Sources examined: `crates/retrieval/tests/lexical_engine.rs:16` and `:180`
  (scratch engine); `crates/retrieval/tests/lexical_retrieval.rs:317`, `:164`,
  and `:551` (projection fixture); `crates/daemon/src/query_route.rs:1075`
  and `:1097` (daemon call site).
- Findings: the engine tests use a scratch in-memory table that shares only
  `fts5_table_args` with the projection. The retrieval test runs the real
  `retrieve` over a projection store opened through `open_store` and a kernel
  with admitted decisions, with probes compiled by `compile` after `analyze`,
  the same two functions `lexical_read` calls. The daemon route has no test
  that sends operator-shaped prose through the wire.
- Missing evidence: a daemon-level test with operator-shaped request text.
  The route reaches `compile` only through `lexical_read`, so the gap is
  coverage of the wire path, not a second compilation path.
- Conclusion: resolved; the record is exercised on the retrieval crate's
  production function against a projection, and additionally on a fixture
  engine with the unquoted control.
