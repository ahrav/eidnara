# claims-column-is-set-once-at-insert

## Discovery trigger

Specification #835 Property Catalog record
`claims-column-is-set-once-at-insert`, derived at `265df096`, with constraint
C-3 (`history_segments` stays append-only; the `claims` column is set once
at insert and never updated; revert and recomp delete segment rows only).
The spec record names an `unreachable` check for any UPDATE of the column
and the heal record `hv-heal-extends-range-without-revalidating-content` as
the post-publish path that must leave `claims` alone. Milestone ticket #838
requires that no statement updates the column and that publish, fence,
revert, recomp, and heal tests stay green.

## Evidence trail

`crates/memory-store/src/lib.rs::insert_history_segment_tx` is the one
statement that writes caller-supplied claims: an `INSERT INTO
history_segments` with `claims_cell(&c.claims)` as the last value. It serves
`append_history_segments_tx`, `MemoryStore::replace_history_segments`, and
`write_seed_history_segment_tx`.

`crates/memory-store/src/lib.rs::write_seed_history_segment_tx`, called from
`MemoryStore::apply_state_sync`, deletes the row at `(session_id, sequence)`
and re-inserts through `insert_history_segment_tx`. Before commit
`5e2e6466` this function was an upsert with `ON CONFLICT(session_id,
sequence) DO UPDATE SET`.

`crates/memory-store/src/lib.rs::MemoryStore::descend_lineage` holds two
more production inserts. The row copy is an `INSERT ... SELECT` that copies
`claims` when `p1` is NULL or unchanged by the re-scan, else `'[]'`. The
lineage boundary placeholder omits the column and takes the default `'[]'`.
Neither updates an existing row.

`crates/memory-store/src/lib.rs::tests::no_production_statement_updates_a_history_segment_row`
reads `lib.rs` through `include_str!`, cuts it at the `#[cfg(test)] mod
tests {` line, and asserts the production part has no `UPDATE
history_segments`, at least two `INTO history_segments` statements, and no
`DO UPDATE` inside any of them.

`crates/memory-store/src/lib.rs::tests::a_state_sync_overwrite_replaces_the_row_and_its_claims_whole`
seeds a row with a claim, overwrites it through
`write_seed_history_segment_tx` with a new `p1` and no claims, and asserts
the new `p1` and empty claims.

`crates/memory-store/src/lib.rs::tests::truncate_history_segments_for_revert_deletes_suffix_and_bumps_epoch`
truncates rows carrying claims to sequence 1 and asserts s1's claim is the
only one left for its key, then resets for recomp and asserts no rows.

## Failure scenario

A state-sync overwrite rewrites `p1` through an upsert whose `DO UPDATE`
does not list `claims`. The row keeps claims whose anchors name text the
new `p1` no longer holds, so the correction footers forever or corrects the
wrong history.

## Timing windows and dependencies

None at runtime. The property is a source condition, so no runtime point
can observe the absence of a statement.

## What a test must construct

A scan of the production source for `UPDATE history_segments` and upsert
clauses into the table. A state-sync overwrite of a row carrying claims. A
revert truncation and a recomp reset over rows carrying claims.

## Investigation log

### Q: Does the source scan cover every production writer?

- Sources examined: `grep` for `INTO history_segments` and `UPDATE
  history_segments` across `crates/**/*.rs` at `c38af85a`.
- Findings: in memory-store production code there are three inserts, the
  one insert and the two in `descend_lineage`, and no update. Every daemon
  hit is under `#[cfg(test)]`: `transform_read_bound`, the `transform.rs`
  test module, `window_coverage/tests.rs`, and `test_support`. The committed
  scan reads only `crates/memory-store/src/lib.rs`, so a writer added in
  another file or crate would pass it. It matches literal text, so an
  `UPDATE` with other spacing would pass, and an `INSERT OR REPLACE INTO`
  is counted as an insert without being flagged.
- Missing evidence: a scan over every production crate.
- Conclusion: resolved with answer for HEAD by manual grep; the committed
  check is a source scan of one file.

### Q: Does the overwrite test fail against the former upsert?

- Sources examined: `git show 5e2e6466^` for `write_seed_history_segment_tx`.
- Findings: the prior version was an upsert. It predates the `claims`
  column, so the claim that it kept old claims is reasoning about a
  `DO UPDATE` that did not list the column.
- Missing evidence: the test was not run against a reverted upsert here.
- Conclusion: unresolved, needs a mutation run to confirm.

### Q: Do the named tests pass at HEAD?

- Sources examined: nextest run at `c38af85a`.
- Findings: all three tests pass.
- Missing evidence: none.
- Conclusion: resolved with answer.
