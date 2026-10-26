# uo-kernel-opens-only-at-the-compiled-baseline-identity

## Discovery trigger

Spec #1145 C18; disposition B (the rotation breaks and recreates the baseline).

Record kind: existing behavior, reconstructed from the specification catalog and
re-verified at this branch tip.

## Evidence trail

References are verified at the #1154 branch tip (the commit that adds this file;
its parent is `bd1216847`).

- Pinned digest: `crates/kernel/tests/kernel_schema.rs:183`.
- Previous-baseline fixture:
  `crates/kernel/tests/fixtures/prior_kernel_baseline.sql`, digest
  `crates/kernel/tests/kernel_open.rs:577`.
- Correction: `kernel_schema.rs` tests moved by two lines on this branch.

- Guarantee under test: A Kernel family opens only when its header, marker, live
  digest, and object inventory equal the compiled baseline; any other family is
  refused untouched.
- Existing check:
  `every_conclusive_kernel_mismatch_is_refused_and_left_untouched`
  (`crates/kernel/tests/kernel_open.rs:183`);
  `a_family_created_under_the_previous_baseline_is_refused_untouched`
  (`crates/kernel/tests/kernel_open.rs:582`);
  `kernel_schema_digest_is_pinned_to_the_frozen_v1_shape`
  (`crates/kernel/tests/kernel_schema.rs:599`);
  `kernel_schema_object_inventory_is_pinned_by_kind_and_name`
  (`crates/kernel/tests/kernel_schema.rs:1900`). Status: unaudited.

## Failure scenario

An open that accepted the previous digest would serve without the new tables.

## Timing windows and dependencies

None.

## What a test must construct

The #1154 fixture test.

Required faults and enabling state: A family at another baseline, with a valid
marker naming that baseline.

## Investigation log

### Q: Does any open question remain for this record?

- Sources examined: the specification catalog entry, the code and tests cited above.
- Findings: the cited code and tests agree with the guarantee at this tip.
- Missing evidence: none for the guarantee as stated.
- Conclusion: resolved with answer - no open question.
