# malformed-stored-claims-blob-reads-as-empty

## Discovery trigger

Specification #835 Property Catalog record
`malformed-stored-claims-blob-reads-as-empty`, derived at `265df096`, with
constraint C-5 (a malformed stored blob reads as empty, and no CHECK or JSON
validity constraint guards the column) and D-5 (a malformed cell reads as an
empty vector with one diagnostic line, never a store error). Milestone
ticket #838 names the cells `'not json'`, `'{}'`, `'[{"key":1}]'`, and
`NULL`, and requires a round trip so serializer drift cannot hide as silent
claim loss.

## Evidence trail

`crates/memory-store/baseline.sql` declares `claims TEXT NOT NULL DEFAULT
'[]'` as the last column of `history_segments`, with no CHECK constraint.
The table is not `STRICT`, so a BLOB can still be stored in the column.

`crates/memory-store/src/lib.rs::HISTORY_SEGMENT_SELECT_COLUMNS` ends with
`claims`, so it sits at zero-based index 17 in every segment read that
maps rows through the one mapper.

`crates/memory-store/src/lib.rs::MemoryStore::stored_history_segment_from_row`
reads the cell as `claims_from_cell(r.get_ref(17)?.as_str().ok())`. A text
cell yields `Some(&str)`; a BLOB or NULL makes `as_str` fail, which becomes
`None`. Only the `get_ref` call can raise an error, and it fails only for a
missing column index.

`crates/memory-store/src/lib.rs::claims_from_cell` returns the parsed vector
when `serde_json::from_str::<Vec<Claim>>` succeeds. Otherwise it writes one
`eprintln!` line, `memory-store: history_segment claims unreadable, read as
none: <reason>`, and returns an empty vector. `claims_cell` writes `[]` for
an empty vector.

`crates/memory-store/src/lib.rs::tests::claims_round_trip_and_a_malformed_cell_reads_as_no_claims`
stores two claims, one with an anchor and one retraction without, through
`replace_history_segments`, and asserts they load equal. It then rewrites
the cell to `'not json'`, `'{}'`, `'[{"key":1}]'`, and the BLOB `b"[]"`, and
after each asserts `load_history_segments` returns the row equal field for
field to the loaded row with claims cleared. It asserts
`claims_from_cell(None)` is empty and `claims_cell(&[])` is `"[]"`.

## Failure scenario

A future serializer change, a manual repair, or file corruption leaves one
cell that does not parse. If the mapper returned the parse error,
`load_history_segments` would fail for the whole session, and every m0 and
m1 compose over that session would fail with it.

## Timing windows and dependencies

None. The mapper runs per row inside each segment read and holds no state.

## What a test must construct

A stored row with non-empty claims to prove the round trip, then that row
with each malformed cell. The cell must be rewritten below the store API,
because the API cannot write a malformed cell. NULL cannot be stored, so the
NULL case is the mapper's `None` input.

## Investigation log

### Q: How is the NULL case exercised when the column is NOT NULL?

- Sources examined: `baseline.sql`, `stored_history_segment_from_row`, the
  test.
- Findings: SQLite refuses a NULL write to the column, so the test cannot
  store one. The test calls `claims_from_cell(None)` directly. The mapper
  maps NULL and BLOB to the same `None`, and the BLOB case is exercised
  through a real load.
- Missing evidence: no load of a stored NULL, which the schema forbids.
- Conclusion: resolved with answer; NULL is checked at the mapper.

### Q: Is the "one diagnostic line" checked?

- Sources examined: `claims_from_cell`, the test body.
- Findings: the function body has exactly one `eprintln!` on the failure
  path and none on success. No test captures stderr or asserts the line.
- Missing evidence: a test that captures the diagnostic output.
- Conclusion: resolved by reading `claims_from_cell`, not by a test.

### Q: Does the round trip compare every field?

- Sources examined: the test's `assert_eq!(loaded[0].claims, claims)`.
- Findings: `Claim` derives `PartialEq`, so key, value, ordinal, and anchor
  are compared, including one `None` anchor and one empty value.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Does the test pass at HEAD?

- Sources examined: nextest run at `c38af85a`.
- Findings: `claims_round_trip_and_a_malformed_cell_reads_as_no_claims`
  passes.
- Missing evidence: none.
- Conclusion: resolved with answer.
