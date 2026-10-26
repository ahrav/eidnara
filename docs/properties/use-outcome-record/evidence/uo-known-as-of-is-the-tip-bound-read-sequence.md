# uo-known-as-of-is-the-tip-bound-read-sequence

## Discovery trigger

Spec #1145 C5 and K14: `served_commit_seq = known_as_of`.

Record kind: existing behavior, reconstructed from the specification catalog and
re-verified at this branch tip.

## Evidence trail

References are verified at the #1155 branch tip (the commit that adds this file;
its parent is `dd84f77af`).

- Tip capture and bound read: `read_project_memory`
  (`crates/daemon/src/canonical_memory.rs:167`).
- Snapshot copy into the composition:
  `crates/daemon/src/canonical_memory.rs:34`.
- R8 applied: the read is an as-of read; the Kernel answers rows visible at the
  requested sequence, so a commit after the tip capture is not reflected and
  `known_as_of` stays the captured tip.
- Correction: the spec cited assertion lines 159 and 196 inside the integration
  test; they are assertions, not production code.

- Guarantee under test: The composition's `known_as_of` is the Kernel tip
  captured before the visible-row read, and the read is bound at that tip.
- Existing check:
  `transform_composes_project_memory_from_canonical_rows_and_observes_absence`
  (`crates/daemon/tests/transform_canonical_memory.rs:46`) (assertions
  `crates/daemon/tests/transform_canonical_memory.rs:159` and
  `crates/daemon/tests/transform_canonical_memory.rs:196`). Status: unaudited.

## Failure scenario

A read at the live tip that reported an earlier `known_as_of` would make a
dispute committed between the two count as before the serve.

## Timing windows and dependencies

Between the tip capture and the read transaction.

## What a test must construct

Capture a tip, commit a disposition, then read bound at the captured tip; assert
the disposition is not reflected and `known_as_of` is the captured tip.

Required faults and enabling state: A Canonical read with a concurrent Kernel
commit.

## Investigation log

### Q: Gap G3

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: Gap G3: commit sequences are allocated by `INTEGER PRIMARY KEY
  AUTOINCREMENT` on `commit_log`, so they are strictly increasing and the tip is
  their maximum; no test names that property for this catalog. Conclusion:
  resolved by the schema (`commit_log` DDL in `crates/kernel/src/schema.rs`); no
  new record.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.
