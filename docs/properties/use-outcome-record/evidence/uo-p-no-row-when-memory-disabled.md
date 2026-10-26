# uo-p-no-row-when-memory-disabled

## Discovery trigger

Spec #1145 C2.

Record kind: proposed obligation; its owning ticket #1155 activates it.

## Evidence trail

References are verified at the #1154 branch tip (the commit that adds this file;
its parent is `bd1216847`).

- The writer reads only the composition the pass froze.

- Guarantee under test: A route bound with `memory.enabled` off produces no
  serving record on any pass.
- Existing check: none; proposed. The writer lands with #1155.

## Failure scenario

A writer keyed off request fields.

## Timing windows and dependencies

None.

## What a test must construct

#1155.

Required faults and enabling state: A route bound with `/memory/enabled: false`,
then HARD folds on both engines.

## Investigation log

### Q: Does any open question remain for this record?

- Sources examined: the specification catalog entry, the code and tests cited above.
- Findings: the cited code and tests agree with the guarantee at this tip.
- Missing evidence: none for the guarantee as stated.
- Conclusion: resolved with answer - no open question.
