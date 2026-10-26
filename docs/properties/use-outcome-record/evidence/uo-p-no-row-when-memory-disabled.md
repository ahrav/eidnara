# uo-p-no-row-when-memory-disabled

## Discovery trigger

Spec #1145 C2.

Record kind: proposed obligation; its owning ticket #1155 activates it.

## Evidence trail

References are verified at the #1154 branch tip (the commit that adds this file;
its parent is `8ace6c9f8`).

- Design (proposed, owner #1155): the writer reads only the composition the
  pass froze. At this tip the evidence covers the memory-disabled read path
  (`uo-memory-disabled-route-takes-no-canonical-read`).

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
- Findings: the Kernel half cited above agrees with the guarantee at this tip;
  the writer is proposed and no code at this tip establishes its behavior.
- Missing evidence: the writer path, which lands with the owning ticket.
- Conclusion: unresolved, needs the writer; the Kernel half is settled.
