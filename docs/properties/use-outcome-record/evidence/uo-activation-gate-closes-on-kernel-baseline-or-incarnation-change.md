# uo-activation-gate-closes-on-kernel-baseline-or-incarnation-change

## Discovery trigger

Spec #1145 C18; the operations note names the renewal terms.

Record kind: existing behavior, reconstructed from the specification catalog and
re-verified at this branch tip.

## Evidence trail

References are verified at the #1155 branch tip (the commit that adds this file;
its parent is `dd84f77af`).

- Terms: `identity_terms`
  (`crates/daemon/src/memory_reviewer/activation.rs:174`).
- Live values: `crates/daemon/src/memory_reviewer/worker.rs:166`.
- Reachability: the gate opens only when the owner writes the record, so the
  closing behavior is explicit-config-only.

- Guarantee under test: The MemoryReviewer activation gate closes when the
  record's `kernel_baseline_digest` or `kernel_incarnation` differs from the
  live value.
- Existing check:
  `the_gate_opens_only_for_a_matching_attested_owner_only_record`
  (`crates/daemon/src/memory_reviewer/activation.rs:448`);
  `the_acceptance_gate_opens_only_for_an_owner_only_record_of_the_live_pair`
  (`crates/daemon/src/memory_reviewer/activation.rs:385`). Status: unaudited.

## Failure scenario

An unrenewed record would admit jobs after the rotation.

## Timing windows and dependencies

None.

## What a test must construct

The existing unit tests.

Required faults and enabling state: An activation record written before the
rotation.

## Investigation log

### Q: Does any open question remain for this record?

- Sources examined: the specification catalog entry, the code and tests cited above.
- Findings: the cited code and tests agree with the guarantee at this tip.
- Missing evidence: none for the guarantee as stated.
- Conclusion: resolved with answer - no open question.
