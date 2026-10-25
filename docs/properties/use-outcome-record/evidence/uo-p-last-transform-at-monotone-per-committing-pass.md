# uo-p-last-transform-at-monotone-per-committing-pass

## Discovery trigger

Spec #1145 C11.

Record kind: proposed obligation; its owning ticket #1156 activates it.

## Evidence trail

References are verified at the #1154 branch tip (the commit that adds this file;
its parent is `5830fe82a`).

- The update is `MAX(last_transform_at, now)` over live records.

- Guarantee under test: `last_transform_at` never decreases on a session's
  serving record, and only a pass whose response committed updates it.
- Existing check: Kernel half only:
  `use_outcome_identity_and_served_set_are_never_rewritten`
  (`crates/kernel/tests/kernel_schema.rs:2174`). The pass hook is proposed.
  Status: unaudited.

## Failure scenario

An earlier clock lowering the value.

## Timing windows and dependencies

None.

## What a test must construct

#1156.

Required faults and enabling state: A committing then a non-committing pass; a
Kernel unavailable at the write.

## Investigation log

### Q: OQ3-6 resolved at #1156 ticketing

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: OQ3-6 resolved at #1156 ticketing: a fenced, receipt-free monotone
  update.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.

### Q: A pass with an earlier clock writes `MAX(last_transform_at, now)`, so 

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: A pass with an earlier clock writes `MAX(last_transform_at, now)`,
  so the trigger never sees a decrease; an abort would mean a writer outside
  that update, and the pass ignores any write failure (C11).
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.
