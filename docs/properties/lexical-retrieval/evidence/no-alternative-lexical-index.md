# no-alternative-lexical-index

## Discovery trigger

#391 AC10: no alternative index. The projection holds one word-level FTS5
index and no prefix, shingle, n-gram, trigram, or contentless path. The RP2.3
property bundle is unavailable here, so the catalog reconstructs this record
from the ticket's acceptance criterion and verifies it against the #391
branch. The #967 PR description maps AC10 to this record as met by existing
tests.

## Evidence trail

Production schema and code:

- `crates/retrieval/baseline.sql:94` creates the one virtual table:
  `CREATE VIRTUAL TABLE lexical USING fts5(original, parts, occurrence_id
  UNINDEXED, tokenize = 'unicode61 remove_diacritics 2 tokenchars ''_''',
  detail = full)`. The argument list carries no `prefix=`, no `content=`,
  no `contentless_delete`, and no `trigram` tokenizer. It is the only
  `CREATE VIRTUAL TABLE` statement in the baseline; every other `CREATE
  INDEX` in the file is a B-tree index on a `STRICT` table.
- `fts5_table_args` (`crates/retrieval/src/lexical/mod.rs:43`) formats the
  same argument list from `TOKENIZER` (`:30`, `unicode61 remove_diacritics 2
  tokenchars '_'`) and `DETAIL` (`:33`, `full`), so the projection table and
  every scratch oracle table share one tokenizer and detail mode.
- `BASELINE` (`crates/retrieval/src/lib.rs:46`) is `include_str!` of
  `baseline.sql`, so the file the schema test opens is the file production
  stores open.
- `probe_engine` (`crates/retrieval/src/lexical/index.rs:251`) issues
  `MATCH '"probe"'` against `lexical` at open, so a build without FTS5 or the
  tokenizer is refused as `Unsupported` rather than served by another path.
- The frozen inventory `docs/properties/search-projection/projection-schema.md:175`
  records the same `fts5(...)` definition and names the shadow tables
  (`lexical_data`, `lexical_idx`, `lexical_content`, `lexical_docsize`,
  `lexical_config`) as engine-owned and outside the inventory.

Tests cited by the record, read at the branch:

- `the_baseline_matches_the_frozen_inventory_field_for_field`
  (`crates/retrieval/tests/schema_inventory.rs:312`) parses the inventory
  document through `documented` (`:39`), asserts 13 documented tables
  (`:332`), and asserts `documented["lexical"].constraints` equals
  `fts5(fts5_table_args())`. It opens a store from `retrieval::BASELINE`
  through `stored` (`:136`), which reads `sqlite_schema`, records a virtual
  table's `USING` clause as its whole definition, skips the five shadow
  table suffixes of any virtual table, and reads every other table's
  columns, constraints, and indexes. `compare` (`:292`) reports a documented
  table not stored, a stored table not documented, or any field difference,
  and the test asserts the difference list is empty. A second FTS5 table, a
  changed tokenizer, a `prefix=` argument, or a `content=''` argument each
  change the stored definition or add a stored table and fail the test.
- `no_prefix_expansion_and_the_prefix_control_differs`
  (`crates/retrieval/tests/lexical_engine.rs:248`) uses the scratch table
  from `populated` (`:180`), which holds `foobar` at rowid 5. It asserts
  the compiled probe for `foo` matches nothing, and the control binds the
  raw expression `"foo"*` and matches `[5]`, so a prefix operator would find
  the row and the compiled probe does not. This covers query-side prefix
  expansion on a table built from `fts5_table_args`; the schema test covers
  the stored definition.

## Failure scenario

An added `prefix=` index, a trigram tokenizer, or a second FTS table changes
which occurrences a literal probe recalls and multiplies the write cost of
every projection insert and tombstone, without a protocol or approval change.
A contentless table removes the stored text `verify_rows` compares against
the analyzed payload. Rebuild and incremental application then store
different rows, and the `analysis_identity` recorded in the projection
identity no longer names the contract the rows were built under.

## Timing windows and dependencies

None. The schema is fixed at store open from a static baseline, and the
inventory comparison reads the schema after open with no concurrent writer.

## What a test must construct

- A store opened from `retrieval::BASELINE` in a temporary directory, with
  storage infrastructure tables excluded through
  `storage::INFRASTRUCTURE_TABLES`.
- An independent parse of the frozen inventory document, so the expected
  definition comes from the document and not from the baseline under test.
- For the prefix control, a scratch FTS5 table built from `fts5_table_args`
  holding a term that a prefix of the probe would match, the compiled probe
  for that prefix, and the raw `"foo"*` expression bound as the control.
- No fault injection; the record's enabling state is the baseline itself.

## Investigation log

### Q: Does the schema inventory test detect an added FTS5 table or option, or only a changed `lexical` definition?

- Sources examined: `crates/retrieval/tests/schema_inventory.rs:136` through
  `:170` (the `stored` reader's virtual table and shadow table handling),
  `:292` (the `compare` function), `:312` through `:339`;
  `crates/retrieval/baseline.sql:94`;
  `docs/properties/search-projection/projection-schema.md:175`.
- Findings: `stored` records any table whose SQL starts with `CREATE VIRTUAL
  TABLE` under its own name with the full `USING` clause, and skips only the
  `config`, `content`, `data`, `docsize`, and `idx` shadow tables of a
  virtual table. A second FTS5 table appears as "stored but not documented".
  A `prefix=` or `content=` option on `lexical` changes the normalized
  `USING` clause and fails the equality on `lexical`. A `CREATE INDEX` on a
  `STRICT` table appears in that table's index list and fails its equality.
- Missing evidence: the test does not inspect `lexical_config` for options
  written after creation through the FTS5 configuration insert; whether FTS5
  accepts a post-creation `prefix` change is unverified here. No test
  enumerates the shadow tables to confirm `lexical_content` exists, which a
  contentless table would omit; the `content=` argument check covers that
  path at the definition level.
- Conclusion: resolved; the inventory test detects an added table, an added
  option on `lexical`, and an added B-tree index, at the stored definition
  level.
