# uo-sensitive-class-denies-remote-and-unknown-egress-is-local-only

## Discovery trigger

Spec #1145 C15 and K10.

Record kind: existing behavior, reconstructed from the specification catalog and
re-verified at this branch tip.

## Evidence trail

References are verified at the #1157 branch tip (the commit that adds this file;
its parent is `7d2a34fe5`).

- Class rule: `denies_destination` (`crates/kernel/src/envelope.rs:71`).
- Egress read-back: `crates/kernel/src/cas/mod.rs:109`.
- Correction: `kernel_cas.rs:452` tests an unknown artifact, not unknown stored
  egress text; the direct proof is the `cas/mod.rs` unit test.

- Guarantee under test: The class rule denies Remote for `sensitive`, and an
  unrecognized stored egress class reads as `local_only`.
- Existing check: `eligibility_matrix_includes_secret_unknown_and_tombstone`
  (`crates/kernel/tests/kernel_cas.rs:452`);
  `egress_facts_carry_the_eligibility_verdict_and_the_stored_class`
  (`crates/kernel/tests/kernel_cas.rs:545`);
  `unrecognized_stored_classes_read_back_restrictive`
  (`crates/kernel/src/cas/mod.rs:797`);
  `a_serve_creates_no_registry_or_evidence_row_and_its_class_denies_remote`
  (`crates/kernel/tests/kernel_use_outcome.rs:322`). Status: unaudited.

## Failure scenario

A Remote-eligible class on a serving record is a stop condition of the plan.

## Timing windows and dependencies

None.

## What a test must construct

The existing tests.

Required faults and enabling state: A sensitive row presented to the rule; an
out-of-vocabulary egress string by SQL.

## Investigation log

### Q: Does any open question remain for this record?

- Sources examined: the specification catalog entry, the code and tests cited above.
- Findings: the cited code and tests agree with the guarantee at this tip.
- Missing evidence: none for the guarantee as stated.
- Conclusion: resolved with answer - no open question.
