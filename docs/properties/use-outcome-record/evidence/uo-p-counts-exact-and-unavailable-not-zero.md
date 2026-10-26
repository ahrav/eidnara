# uo-p-counts-exact-and-unavailable-not-zero

## Discovery trigger

Spec #1145 C17.

Record kind: new obligation activated by #1157.

## Evidence trail

References are verified at the #1157 branch tip, the commit that carries this
revision.

- Count read: `crates/kernel/src/use_outcome.rs:212` counts the object's
  decision rows in one statement and compares each serve with the newest
  qualifying dispute, the rule the by-decision read applies per serve.
- Route: `crates/daemon/src/memory_reviewer/wire.rs:314` binds through
  `kernel_request`, so no Memory Store or MODULE authority is consulted.
- Client: `packages/cli/src/commands/review.ts:470` requests exact integers.

- Guarantee under test: Served and disputed-after-serve counts per decision are
  exact unsigned integers at one Kernel snapshot; a Kernel that is not Ready
  answers unavailable with no integer.
- Existing check:
  `review_use_outcome_counts_serves_and_later_disputes_of_one_decision`
  (`crates/daemon/tests/memory_reviewer_wire.rs:686`);
  `review_use_outcome_answers_an_unready_kernel_with_its_state_and_no_count`
  (`crates/daemon/src/use_outcome_lifecycle_tests.rs:476`);
  `packages/cli/src/commands/review-wire.contract.test.ts:125`;
  `packages/cli/src/commands/review.test.ts:849`; Kernel half:
  `disputed_after_serve_is_judged_against_each_serve`
  (`crates/kernel/tests/kernel_use_outcome.rs:826`) and
  `disputed_after_serve_counts_only_later_committed_disputes_on_the_same_lineage`
  (`crates/kernel/tests/kernel_use_outcome.rs:400`); the read bound:
  `a_held_reader_pool_bounds_the_count_read`
  (`crates/kernel/tests/kernel_use_outcome.rs:1199`) and
  `review_use_outcome_reads_the_kernel_under_a_bounded_budget`
  (`crates/daemon/tests/memory_reviewer_wire.rs:787`). Status: unaudited.

## Failure scenario

A zero emitted for an unready Kernel.

## Timing windows and dependencies

None.

## What a test must construct

The tests above.

Required faults and enabling state: A Kernel starting; one decision served to
two sessions with one later dispute.

## Investigation log

### Q: OQ3-2 and OQ3-15: what does `review.use_outcome` answer when the Kernel is unready?

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: OQ3-2 and OQ3-15 resolved at #1157 ticketing: a `review.use_outcome`
  method answering the Kernel state outcome when unready.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.

### Q: `served_sessions` counts unretired serving records, one per (session, served set), so a session served the decision at two served sets counts twice; the name follows the ticket

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: `served_sessions` counts unretired serving records, one per
  (session, served set), so a session served the decision at two served sets
  counts twice; the name follows the ticket.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.

### Q: The counts cover the bound project only; another project's serves of the same decision are not counted

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: The counts cover the bound project only; another project's serves of
  the same decision are not counted.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.

### Q: A failed read after the Kernel is Ready answers the kernel state its failure maps to

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: A failed read after the Kernel is Ready answers the kernel state its
  failure maps to. The read runs under a 5 s budget; a held reader pool stops the
  Kernel read with `Deadline` (`a_held_reader_pool_bounds_the_count_read`),
  which maps to `unavailable`/`store_busy`; no daemon test drives that
  state answer. AC19 (no identifier in logs or `host.status`) holds by
  inspection: the handler logs nothing and adds no status field.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.
