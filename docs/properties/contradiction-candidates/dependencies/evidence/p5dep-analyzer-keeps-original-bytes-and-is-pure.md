# p5dep-analyzer-keeps-original-bytes-and-is-pure

HEAD `3a3b916b7`. Surface S1.

## Discovery trigger

Plan R3: "the daemon folds case before comparing atoms (the analyzer keeps
original bytes and leaves folding to FTS5)". The module header at
`crates/retrieval/src/lexical/analysis.rs:6` states "Parts keep their
original bytes; the engine folds case and diacritics."
`crates/retrieval/AGENTS.md` states "Keep mutations pure." P5's determinism
requirement (R3: "ties and ordering are deterministic") depends on the
analyzer being a pure function. Lenses: data integrity, idempotency.

## Evidence trail

- `analysis.rs:1-8` module header: atom definition, part split rules, "Parts
  keep their original bytes".
- `analysis.rs:107-119` `COMBINING_MARKS`, `is_private_use`, `starts_atom`,
  `continues_atom`: `char` predicates only.
- `analysis.rs:122-126` `atoms()`: `text.split(..)` then
  `trim_start_matches` of combining marks; yields `&str` slices of the input.
- `analysis.rs:128-149` `Class` and `class(c)`: `is_numeric`, `is_uppercase`,
  `is_lowercase`; no folding.
- `analysis.rs:152-188` `parts(atom)`: builds `String`s by `push(c)` of the
  atom's own chars in order; the only transformation is splitting. No
  `to_lowercase`, `to_uppercase`, or normalization call exists in the file
  (grep for `lowercase(`/`uppercase(` returns only the `is_*` predicates at
  `:142-144`).
- `analysis.rs:190-199` `splits`: pure over `Class` values.
- No `static`, `thread_local`, `std::env`, `std::time`, or I/O import in the
  file (`:10-11` import only `NonZeroUsize` and `RangeInclusive`).
- `mod.rs:36` `TOKENIZER = "unicode61 remove_diacritics 2 tokenchars '_'"`:
  folding happens in FTS5, consistent with the header.
- Goldens: `tests/lexical_analysis.rs:190-195` `HTTPServer x86_64` ->
  original text `"HTTPServer x86_64"`, parts `"HTTP Server x 86 64"`; `:83`
  `non_ascii_case_and_uncased_letters_follow_the_same_rules`; `:93`, `:108`
  combining-mark cases.

## Failure scenario

If a future change folded case in the analyzer, P5's own fold (R3) would be
redundant but harmless; however the FTS5 index built from the same analyzer
would then probe folded terms against a tokenizer that also folds, changing
recall semantics for the search path. If the analyzer became impure (for
example caching by text in a process-global map with a bug), two passes over
the same descriptor could produce different candidate sets, breaking P5 U3's
"table can be truncated and rebuilt with identical results".

## Timing windows and dependencies

None at runtime. The toolchain's `char` tables are a build-time dependency;
see `p5dep-analysis-epoch-and-identity-pin-the-contract`.

## What a test must construct

- Property: for random `text`, `analyze(text, b)` twice yields equal
  `Analysis`; every atom is a byte-exact substring of `text`; every part is a
  byte-exact substring of some atom; concatenating an atom's parts in order
  (when the atom contributed parts) equals the atom minus `_` separators.
- Inputs with uppercase, lowercase, titlecase (`ǅ`), and diacritics; assert
  the output bytes equal the input bytes for each atom.
- Concurrency: call from several threads on the same inputs; results equal.

## Investigation log

### Q: Do NFC and NFD spellings split identically, as `analysis.rs:5` claims?

- Sources examined: `analysis.rs:152-176` (marks share the base's class and
  are skipped in boundary rules), `tests/lexical_analysis.rs:93-126`.
- Findings: the tests at `:93` and `:108` assert the documented examples
  (`e\u{301}Bar` vs `éBar`, `IDs\u{301}` vs `IDś`). The outputs keep their own
  bytes, so NFC and NFD inputs yield byte-different atoms that split at the
  same positions.
- Missing evidence: a general NFC/NFD property beyond the examples.
- Conclusion: resolved for the documented examples; the general claim is not
  promoted by this record (it is a separate retrieval-contract claim outside
  P5's dependency).
