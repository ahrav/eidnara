# uo-composition-freeze-persists-at-every-hard-fold-site

## Discovery trigger

Spec #1145 K3: the plan's line references named a wire field and a predicate;
the freeze is the assignment at three HARD sites.

Record kind: existing behavior, reconstructed from the specification catalog and
re-verified at this branch tip.

## Evidence trail

References are verified at the #1156 branch tip, the commit that carries this
revision.

- Additive engine: `crates/daemon/src/transform.rs:3122` inside
  `apply_additive_only` (`apply_additive_only`
  (`crates/daemon/src/transform.rs:2854`)).
- Folding engine fold: `crates/daemon/src/transform.rs:4743`; pressure refold:
  `crates/daemon/src/transform.rs:4898` (`MaterializeReason::PressureRefold`).
- All three call `ctx.project_memory_composition()`, which returns the
  composition of the read pinned for that pass.
- Correction: the spec cited `transform.rs` lines 13972, 20836, and 21245; at this tip
  13972 is inside a test, and the production sites are the three lines above.

- Guarantee under test: Every pass whose plan is `Hard` or `MigrateHard` assigns
  the pinned read's composition to session meta at one of three sites, made
  durable by the pass's memory-store commit.
- Existing check:
  `canonical_memory_composes_the_project_memory_block_and_pins_the_snapshot`
  (`crates/daemon/src/transform.rs:13886`);
  `transform_composes_project_memory_from_canonical_rows_and_observes_absence`
  (`crates/daemon/tests/transform_canonical_memory.rs:46`);
  `a_failed_write_changes_nothing_the_harness_receives_and_a_later_fold_repairs_it`
  (`crates/daemon/src/use_outcome_writer_tests.rs:282`);
  `a_pressure_refold_reports_its_frozen_composition`
  (`crates/daemon/src/transform.rs:20895`). Status: unaudited.

## Failure scenario

A pressure refold that rebuilt m0 without assigning the composition would serve
rows the record never names.

## Timing windows and dependencies

The assignment precedes the memory-store CAS; a discarded attempt retries with
the same pinned read.

## What a test must construct

A pressure refold on the folding engine with a Canonical read, asserting
`meta.project_memory` equals the read; a fault point after the memory-store
commit (the #1155 seam). The seam injects a handled Kernel failure in a live
process; a stop and restart between the two stores is not constructed.

Required faults and enabling state: A HARD fold on each engine, a pressure
refold on the folding engine, and a CAS conflict inside one pass.

## Investigation log

### Q: Gap G1

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: Gap G1: does a CAS-discarded attempt leave a use-outcome serve?
  Owner #1155 (AC7); the writer runs only after the attempt's memory-store
  commit succeeded, and
  `a_discarded_attempt_leaves_no_serve_and_the_retry_records_one`
  (`crates/daemon/src/use_outcome_writer_tests.rs:343`) asserts that a rival
  commit lands while no composition is stored and that one record results.
  The test inspects the Memory Store row version, the frozen composition, the
  Kernel served-set count, and the `written` counter; other durable state is
  outside its scope.
- Missing evidence: none for the serve; other durable effects of a discarded
  attempt are not this record's question.
- Conclusion: resolved with answer for the use-outcome serve - a discarded
  attempt writes none.
