# uo-composition-revision-digests-exactly-the-rendered-rows

## Discovery trigger

Spec #1145 C3 and OQ3-12: the revision was the proposed identity component.

Record kind: existing behavior, reconstructed from the specification catalog and
re-verified at this branch tip.

## Evidence trail

References are verified at the #1157 branch tip, the commit that carries this
revision, except the last four tests named under Existing check and the
`canonical_memory.rs` line of every reference: later changes added those
tests, the #1157 gate run covers none of them, and they and those lines
were read at the tree that holds
`rows_of_one_id_enter_the_revision_in_category_order`.

- `CanonicalMemorySnapshot::new` seeds the hasher with
  `eidnara-project-memory-revision-v1` and hashes category plus rendered line
  per rendered row, in the digest's own order: the `NAMED_FILES` rows in the
  supplied order, each line up to its last-change clause
  (`named_files::revised_part`), the positive-category rows in object-id
  order across their categories, rows of one id in category order, then the
  `CURRENT_WORK` rows in the supplied order (`crates/daemon/src/canonical_memory.rs:57-86`).
- The rows come from the injectable snapshot, trimmed to the memory budget
  before rendering (`injectable_snapshot`
  (`crates/daemon/src/canonical_memory.rs:821`)).
- The spec cited the `fn` lines of the first four tests named under Existing
  check, and the production code was not cited. The fifth covers the order
  of the `CURRENT_WORK` rows, the sixth the `NAMED_FILES` rows and how
  the two sections interleave, the seventh the last-change clause, and the
  eighth two rows of one id.

- Guarantee under test: The composition revision is a digest over the
  rendered rows after the token-budget trim, in the digest's own order: the
  `NAMED_FILES` rows in the supplied order, each line up to its last-change
  clause, the positive-category rows sorted by object id across their
  categories, rows of one id in category order, then the `CURRENT_WORK` rows
  in the supplied order. The last-change clause of a `NAMED_FILES` line reads
  the work tree when the block is composed and is no input of the digest.
- Existing check: `revision_changes_only_when_the_rendered_inputs_change`
  (`crates/daemon/src/canonical_memory.rs:1349`);
  `rows_the_renderer_drops_do_not_move_the_revision`
  (`crates/daemon/src/canonical_memory.rs:1645`);
  `bytes_past_the_render_cap_do_not_move_the_revision`
  (`crates/daemon/src/canonical_memory.rs:1662`);
  `rows_past_the_memory_budget_are_neither_injected_nor_digested`
  (`crates/daemon/src/canonical_memory.rs:1684`);
  `the_revision_reads_the_current_work_section_in_the_supplied_order`
  (`crates/daemon/src/canonical_memory.rs:1611`);
  `the_revision_reads_the_named_files_in_the_supplied_order`
  (`crates/daemon/src/canonical_memory.rs:1550`);
  `the_revision_leaves_out_a_named_files_last_change`
  (`crates/daemon/src/canonical_memory.rs:1759`);
  `rows_of_one_id_enter_the_revision_in_category_order`
  (`crates/daemon/src/canonical_memory.rs:1806`). Status: unaudited.

## Failure scenario

A renderer change that drops a row without changing the digest inputs would
leave the revision equal while the bytes differ. The last-change clause of a
`NAMED_FILES` line is such a difference by design: two blocks that differ in
it alone share a revision, and a session keeps the block its last
composition rendered.

## Timing windows and dependencies

None.

## What a test must construct

The eight unit tests already construct each input class.

Required faults and enabling state: Snapshots that differ only in rows the
renderer drops, bytes past the render cap, rows past the memory budget, the
order of the `CURRENT_WORK` rows, the presence and order of the
`NAMED_FILES` rows, the last-change clause of a `NAMED_FILES` line, or the
input order of two rows of one id in two categories.

## Investigation log

### Q: Is a composition revision from `DefaultHasher` stable across toolchain releases?

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: The `DefaultHasher` output is unspecified across toolchain releases;
  #1154 resolves OQ3-12 by keying the record on `served_set_digest` and storing
  the revision as an informational column.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.
