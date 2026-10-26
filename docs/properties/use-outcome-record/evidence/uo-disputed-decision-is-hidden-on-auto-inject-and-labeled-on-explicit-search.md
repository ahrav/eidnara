# uo-disputed-decision-is-hidden-on-auto-inject-and-labeled-on-explicit-search

## Discovery trigger

Spec #1145 C12: events at or below the served sequence were reflected in the
served rows.

Record kind: existing behavior, reconstructed from the specification catalog and
re-verified at this branch tip.

## Evidence trail

References are verified at the #1155 branch tip (the commit that adds this file;
its parent is `62659617c`).

- Disposition mapping: `crates/kernel/src/admission.rs:4161`.
- Row and surface: `visibility_row` (`crates/kernel/src/admission.rs:4248`),
  `crates/kernel/src/admission.rs:4121`.
- Correction: both integration tests cited by the spec exercise `AutoSearch`;
  the `AutoInject` cell is proved by the unit matrix.

- Guarantee under test: A decision whose newest disposition is `disputed` is
  hidden on `auto_inject` and `auto_search` and served labeled on
  `explicit_search`.
- Existing check: `visibility_matrix_and_sensitivity_ceiling_are_total`
  (`crates/kernel/src/admission.rs:4915`);
  `every_non_active_disposition_has_the_contracted_surface_row`
  (`crates/kernel/tests/kernel_admission.rs:2890`);
  `dispositions_and_sensitivity_only_reduce_visibility`
  (`crates/kernel/tests/kernel_admission.rs:2735`). Status: unaudited.

## Failure scenario

A disputed row on auto-inject would make a pre-serve dispute look like a harm
after the serve.

## Timing windows and dependencies

None.

## What a test must construct

An admitted decision, a `mark_disputed`, and an auto-inject read asserting the
row is absent.

Required faults and enabling state: A decision admitted, then a committed
`mark_disputed` on it.

## Investigation log

### Q: Does any open question remain for this record?

- Sources examined: the specification catalog entry, the code and tests cited above.
- Findings: the cited code and tests agree with the guarantee at this tip.
- Missing evidence: none for the guarantee as stated.
- Conclusion: resolved with answer - no open question.
