# uo-composition-revision-digests-exactly-the-rendered-rows

## Discovery trigger

Spec #1145 C3 and OQ3-12: the revision was the proposed identity component.

Record kind: existing behavior, reconstructed from the specification catalog and
re-verified at this branch tip.

## Evidence trail

References are verified at the #1157 branch tip, the commit that carries this
revision.

- `CanonicalMemorySnapshot::new` seeds the hasher with
  `eidnara-project-memory-revision-v1` and hashes category plus rendered line
  per rendered row, in the digest's own order: the `NAMED_FILES` rows in the
  supplied order, the positive-category rows in object-id order across their
  categories, then the `CURRENT_WORK` rows in the supplied order (`crates/daemon/src/canonical_memory.rs:56-79`).
- The rows come from the injectable snapshot, trimmed to the memory budget
  before rendering (`injectable_snapshot`
  (`crates/daemon/src/canonical_memory.rs:805`)).
- The spec cited the `fn` lines of the first four tests named under Existing
  check, and the production code was not cited. The fifth covers the order
  of the `CURRENT_WORK` rows, and the sixth the `NAMED_FILES` rows and how
  the two sections interleave.

- Guarantee under test: The composition revision is a digest over exactly the
  rendered rows after the token-budget trim, in the digest's own order: the
  `NAMED_FILES` rows in the supplied order, the positive-category rows
  sorted by object id across their categories, then the `CURRENT_WORK` rows
  in the supplied order.
- Existing check: `revision_changes_only_when_the_rendered_inputs_change`
  (`crates/daemon/src/canonical_memory.rs:1333`);
  `rows_the_renderer_drops_do_not_move_the_revision`
  (`crates/daemon/src/canonical_memory.rs:1629`);
  `bytes_past_the_render_cap_do_not_move_the_revision`
  (`crates/daemon/src/canonical_memory.rs:1646`);
  `rows_past_the_memory_budget_are_neither_injected_nor_digested`
  (`crates/daemon/src/canonical_memory.rs:1668`);
  `the_revision_reads_the_current_work_section_in_the_supplied_order`
  (`crates/daemon/src/canonical_memory.rs:1595`);
  `the_revision_reads_the_named_files_in_the_supplied_order`
  (`crates/daemon/src/canonical_memory.rs:1534`). Status: unaudited.

## Failure scenario

A renderer change that drops a row without changing the digest inputs would
leave the revision equal while the bytes differ.

## Timing windows and dependencies

None.

## What a test must construct

The six unit tests already construct each input class.

Required faults and enabling state: Snapshots that differ only in rows the
renderer drops, bytes past the render cap, rows past the memory budget, the
order of the `CURRENT_WORK` rows, or the presence and order of the
`NAMED_FILES` rows.

## Investigation log

### Q: Is a composition revision from `DefaultHasher` stable across toolchain releases?

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: The `DefaultHasher` output is unspecified across toolchain releases;
  #1154 resolves OQ3-12 by keying the record on `served_set_digest` and storing
  the revision as an informational column.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.
