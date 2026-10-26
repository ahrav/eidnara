# uo-p-counts-exact-and-unavailable-not-zero

## Discovery trigger

Spec #1145 C17.

Record kind: new obligation activated by #1157.

## Evidence trail

References are verified at the #1157 branch tip (the commit that adds this file;
its parent is `dc07f442a`).

- Count read: `crates/kernel/src/use_outcome.rs:211` counts the rows of the
  by-decision read in one statement.
- Route: `crates/daemon/src/memory_reviewer/wire.rs:25` binds through
  `kernel_request`, so no Memory Store or MODULE authority is consulted.
- Client: `packages/cli/src/commands/review.ts:420` requests exact integers.

- Guarantee under test: Served and disputed-after-serve counts per decision are
  exact unsigned integers at one Kernel snapshot; a Kernel that is not Ready
  answers unavailable with no integer.
- Existing check:
  `review_use_outcome_counts_serves_and_later_disputes_of_one_decision`
  (`crates/daemon/tests/memory_reviewer_wire.rs:679`);
  `review_use_outcome_answers_an_unready_kernel_with_its_state_and_no_count`
  (`crates/daemon/src/use_outcome_lifecycle_tests.rs:432`);
  `packages/cli/src/commands/review-wire.contract.test.ts:124`;
  `packages/cli/src/commands/review.test.ts:822`; Kernel half:
  `disputed_after_serve_is_judged_against_each_serve`
  (`crates/kernel/tests/kernel_use_outcome.rs:730`). Status: unaudited.

## Failure scenario

A zero emitted for an unready Kernel.

## Timing windows and dependencies

None.

## What a test must construct

The tests above.

Required faults and enabling state: A Kernel starting; one decision served to
two sessions with one later dispute.

## Investigation log

### Q: OQ3-2 and OQ3-15 resolved at #1157 ticketing: a `review.use_outcome` m

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: OQ3-2 and OQ3-15 resolved at #1157 ticketing: a `review.use_outcome`
  method answering the Kernel state outcome when unready.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.

### Q: `served_sessions` counts unretired serving records, one per (session, 

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: `served_sessions` counts unretired serving records, one per
  (session, served set), so a session served the decision at two served sets
  counts twice; the name follows the ticket.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.
