# uo-p-classification-sensitive-local-only-no-remote-egress

## Discovery trigger

Spec #1145 C15, D8, K10; AC15.

Record kind: new obligation activated by #1154.

## Evidence trail

References are verified at the #1156 branch tip (the commit that adds this file;
its parent is `6d91bfa92`).

- DDL CHECKs: `crates/kernel/src/schema.rs:187`.
- Insert: `insert_use_outcome` (`crates/kernel/src/use_outcome.rs:252`); it
  pushes no pending change, so the commit writes no change event or outbox row.
- Class rule: `denies_destination` (`crates/kernel/src/envelope.rs:71`).

- Guarantee under test: Every serving record carries `sensitive`, `local_only`,
  and `use_outcome_capture`, the DDL refuses any other value, and no serve
  creates a registry object, evidence row, change event, or outbox entry.
- Existing check: `use_outcome_classification_and_identity_shapes_are_checked`
  (`crates/kernel/tests/kernel_schema.rs:2297`);
  `a_serve_creates_no_registry_or_evidence_row_and_its_class_denies_remote`
  (`crates/kernel/tests/kernel_use_outcome.rs:322`). Status: unaudited.

## Failure scenario

A serve that registered an object would enter the egress machinery.

## Timing windows and dependencies

None.

## What a test must construct

The two tests.

Required faults and enabling state: Inserts with `normal` and `remote_allowed`;
a serve through the envelope.

## Investigation log

### Q: Backup classification

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: Backup classification: the backup scan reads every table with a
  `sensitivity_class` column, so a Kernel holding a serving record classifies as
  at least `sensitive`. Conclusion: resolved by the scan in
  `crates/kernel/src/backup.rs`; no new record.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.
