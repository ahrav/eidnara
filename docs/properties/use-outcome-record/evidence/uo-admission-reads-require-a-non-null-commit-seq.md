# uo-admission-reads-require-a-non-null-commit-seq

## Discovery trigger

Spec #1145 C12.

Record kind: existing behavior, reconstructed from the specification catalog and
re-verified at this branch tip.

## Evidence trail

References are verified at the #1154 branch tip (the commit that adds this file;
its parent is `1c56ba033`).

- Writer binding: `crates/kernel/src/admission.rs:1748`; `commit_seq` is
  `self.commit_seq`.
- Reader filters: `crates/kernel/src/admission.rs:675`; the new reader
  `crates/kernel/src/use_outcome.rs:152`.
- Reachability: a null-commit row exists only after direct SQL, so the label is
  test-only.

- Guarantee under test: Every admission read counts only rows whose `commit_seq`
  is not null.
- Existing check:
  `malformed_stored_policy_and_sensitivity_fail_closed_without_read_error`
  (`crates/kernel/tests/kernel_admission.rs:2935`);
  `disputed_after_serve_counts_only_later_committed_disputes_on_the_same_lineage`
  (`crates/kernel/tests/kernel_use_outcome.rs:288`). Status: unaudited.

## Failure scenario

A reader that omitted the filter would count a row no commit made.

## Timing windows and dependencies

None.

## What a test must construct

The existing test and the #1154 join test.

Required faults and enabling state: A row whose `commit_seq` is nulled by direct
SQL.

## Investigation log

### Q: Does any open question remain for this record?

- Sources examined: the specification catalog entry, the code and tests cited above.
- Findings: the cited code and tests agree with the guarantee at this tip.
- Missing evidence: none for the guarantee as stated.
- Conclusion: resolved with answer - no open question.
