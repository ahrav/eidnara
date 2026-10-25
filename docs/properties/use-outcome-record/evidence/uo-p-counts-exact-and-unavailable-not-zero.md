# uo-p-counts-exact-and-unavailable-not-zero

## Discovery trigger

Spec #1145 C17.

Record kind: proposed obligation; its owning ticket #1157 activates it.

## Evidence trail

References are verified at the #1154 branch tip (the commit that adds this file;
its parent is `1c56ba033`).

- The counts derive from `served_sessions_for`.

- Guarantee under test: Served and disputed-after-serve counts per decision are
  exact unsigned integers at one Kernel snapshot; a Kernel that is not Ready
  answers unavailable with no integer.
- Existing check: none; proposed.

## Failure scenario

A zero emitted for an unready Kernel.

## Timing windows and dependencies

None.

## What a test must construct

#1157.

Required faults and enabling state: A Kernel starting; one decision served to
two sessions with one later dispute.

## Investigation log

### Q: OQ3-2 and OQ3-15 resolved at #1157 ticketing: a `review.use_outcome` m

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: OQ3-2 and OQ3-15 resolved at #1157 ticketing: a `review.use_outcome`
  method answering the Kernel state outcome when unready.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.
