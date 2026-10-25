# il-readmission-block-ends-batch-unnamed-at-head

## Discovery trigger

Issue #1105 Part A evaluation (gap): the re-admission block reaches the daemon as an
unnamed batch conflict, which the plan's R16 replaces.

## Evidence trail

- At `41d889ea4` the pre-screen held no re-admission check.
- Batch ingest refuses a reclaiming or blocked digest with
  `KernelError::Conflict` (`crates/kernel/src/cas/ingest.rs:822-827`); the
  batch contract says the error names no request
  (`crates/kernel/src/cas/ingest.rs:693`). The single ingest names
  `ReAdmissionBlocked` (`crates/kernel/src/cas/ingest.rs:461-468`, `:1151-1153`).
- `BatchRefusal::conflicting_unit` (`harness_sources.rs:1483-1485`) is `None`
  when `unit` is `None`.
- `publish_locked` (`crates/daemon/src/source_capture.rs:845-870`) records
  every keyed unit `Pending` and ends with `kernel_end`, which counts
  `kernel_error` (`source_capture.rs:1476-1481`).
- `is_permanent` lists `ReAdmissionBlocked` (`harness_sources.rs:578`), and
  `classify` maps a permanent refusal to `units_refused_terminal`
  (`source_capture.rs:1209-1211`); neither is reached on this path.

## Failure scenario

An operator purges a captured digest. A later window offers the same bytes
under a new identity beside nine siblings. Every pass ends `kernel_error`,
and all ten units stay pending while the tombstone holds.

## Timing windows and dependencies

The purge commits before the pass. The block outlives the pending unlink
because each pending unlink is written with a tombstone.

## What a test must construct

A daemon capture test: capture a unit, purge its digest, offer identical bytes
under a new identity beside a sibling; assert `kernel_error`, both units
pending, nothing retained.

## Investigation log

### Q: Does any HEAD path name the blocked unit?

- Sources examined: the pre-screen, `publish_screened`'s commit path, the
  Kernel batch ingest.
- Findings: no; the unit is named only by the single-artifact ingest, which
  capture does not call for a batch.
- Missing evidence: none.
- Conclusion: resolved with answer; U6 changes it.

### Q: Why does confidence move from the source's `medium` to `high`?

- Sources examined: the source record in #1105; the code and tests cited above.
- Findings: the source left the trace pending; this pass followed the batch from the pre-screen through the Kernel batch ingest to `kernel_end` at `41d889ea4`, every step cited above.
- Missing evidence: none for the confidence level; an `Exercised` gap, where
  the record names one, stays open.
- Conclusion: resolved with answer.

### Q: What does U6 change?

- Sources examined: `crates/daemon/src/harness_sources.rs:1594-1599` and
  `:1160-1172`; `crates/kernel/src/cas/ingest.rs:1470-1475`; the U6 tests.
- Findings: the screening preview refuses a unit with no stored descriptor
  receipt whose payload digest is tombstoned or pending unlink, before the
  commit and without an admission slot. A purge that commits after that
  preview still reaches batch ingest and ends one pass `kernel_error`
  (`crates/daemon/src/source_capture_tests.rs:2568`).
- Missing evidence: none.
- Conclusion: resolved with answer: the record is invalidated.
