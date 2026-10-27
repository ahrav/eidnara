# uo-p-baseline-rotation-refuses-previous-family-untouched

## Discovery trigger

Spec #1145 C18 and AC1, AC2.

Record kind: new obligation activated by #1154.

## Evidence trail

References were first verified at the #1157 branch tip (the commit that adds
this file; its parent is `d85de937b`) and are re-verified at the #1162 branch
tip.

- Components: `crates/kernel/src/schema.rs:193` and
  `crates/kernel/src/schema.rs:197`, with the `use_outcome_sessions`
  component #1162 adds (`crates/kernel/src/schema.rs:188`); the admission
  citing column and
  `idx_admission_use_outcome_fk` in the `admission_decisions` component
  (`crates/kernel/src/schema.rs:164`).
- Pinned digest: `crates/kernel/tests/kernel_schema.rs:184`
  (`197430a81f05514d0def36631fb5dea80c05938ed60c9747f9c5d805eca85f03`).
- Component-name list: `crates/kernel/tests/kernel_schema.rs:14`; object
  inventory: `kernel_schema_object_inventory_is_pinned_by_kind_and_name`
  (`crates/kernel/tests/kernel_schema.rs:1901`).
- Refusal: `classify_existing_family` (`crates/kernel/src/open.rs:796`).

- Guarantee under test: A Kernel family at the previous baseline opens as
  `IdentityMismatch` with its main file and `-wal` byte-identical; a new family
  contains the use-outcome tables and its digest equals the pinned literal.
- Existing check:
  `a_family_created_under_the_previous_baseline_is_refused_untouched`
  (`crates/kernel/tests/kernel_open.rs:582`);
  `kernel_schema_digest_is_pinned_to_the_frozen_v1_shape`
  (`crates/kernel/tests/kernel_schema.rs:600`);
  `kernel_schema_object_inventory_is_pinned_by_kind_and_name`
  (`crates/kernel/tests/kernel_schema.rs:1901`);
  `kernel_schema_has_one_ordered_full_shape`
  (`crates/kernel/tests/kernel_schema.rs:152`). Status: unaudited.

## Failure scenario

An open that installed the new tables into the old family would rewrite a family
the operator expected untouched.

## Timing windows and dependencies

None.

## What a test must construct

The fixture test; any later K1 change rotates the pinned digest again and
updates this record.

Required faults and enabling state: A fixture database at the `a0e3f2993` digest
(`13437d62f69c77d1f5bdad72195473d1f409c2647efecdcac660554e5c1db07b`) opened by
this binary.

## Investigation log

### Q: Does any open question remain for this record?

- Sources examined: the specification catalog entry, the code and tests cited above.
- Findings: the cited code and tests agree with the guarantee at this tip.
- Missing evidence: none for the guarantee as stated.
- Conclusion: resolved with answer - no open question.
