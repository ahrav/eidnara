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
  per rendered row: the positive-category rows in object-id order, then the
  `CURRENT_WORK` rows in the supplied order, which is the order the block
  renders them in (`crates/daemon/src/canonical_memory.rs:57-64`).
- The rows come from the injectable snapshot, trimmed to the memory budget
  before rendering (`injectable_snapshot`
  (`crates/daemon/src/canonical_memory.rs:711`)).
- The first four tests named under Existing check are unchanged since
  `a0e3f2993`; the spec cited their `fn` lines, and the production code was not
  cited. The fifth covers the order of the `CURRENT_WORK` rows.

- Guarantee under test: The composition revision is a digest over exactly the
  rendered rows after the token-budget trim: the positive-category rows sorted
  by object id, then the `CURRENT_WORK` rows in the order the block renders
  them.
- Existing check: `revision_changes_only_when_the_rendered_inputs_change`
  (`crates/daemon/src/canonical_memory.rs:1226`);
  `rows_the_renderer_drops_do_not_move_the_revision`
  (`crates/daemon/src/canonical_memory.rs:1370`);
  `bytes_past_the_render_cap_do_not_move_the_revision`
  (`crates/daemon/src/canonical_memory.rs:1387`);
  `rows_past_the_memory_budget_are_neither_injected_nor_digested`
  (`crates/daemon/src/canonical_memory.rs:1409`);
  `the_revision_reads_the_current_work_section_in_the_supplied_order`
  (`crates/daemon/src/canonical_memory.rs:1336`). Status: unaudited.

## Failure scenario

A renderer change that drops a row without changing the digest inputs would
leave the revision equal while the bytes differ.

## Timing windows and dependencies

None.

## What a test must construct

The five unit tests already construct each input class.

Required faults and enabling state: Snapshots that differ only in rows the
renderer drops, bytes past the render cap, rows past the memory budget, or
the order of the `CURRENT_WORK` rows.

## Investigation log

### Q: Is a composition revision from `DefaultHasher` stable across toolchain releases?

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: The `DefaultHasher` output is unspecified across toolchain releases;
  #1154 resolves OQ3-12 by keying the record on `served_set_digest` and storing
  the revision as an informational column.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.
