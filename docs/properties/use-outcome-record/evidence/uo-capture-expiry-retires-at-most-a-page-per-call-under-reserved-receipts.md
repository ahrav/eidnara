# uo-capture-expiry-retires-at-most-a-page-per-call-under-reserved-receipts

## Discovery trigger

Spec #1145 K1: the capture sweeps live in the local-file module.

Record kind: existing behavior, reconstructed from the specification catalog and
re-verified at this branch tip.

## Evidence trail

References are verified at the #1157 branch tip (the commit that adds this file;
its parent is `d85de937b`).

- Page bound and order: `expired_captures_sql`
  (`crates/kernel/src/local_file.rs:549`).
- Reserved receipt: `expiry_intent` (`crates/kernel/src/local_file.rs:433`).
- The use-outcome sweep follows the same shape at `expire_use_outcomes`
  (`crates/kernel/src/use_outcome.rs:563`).
- Correction: the spec cited `harness_sources.rs:3331, 3187`; the tests moved to
  the lines under Existing check.

- Guarantee under test: The capture expiry sweeps retire at most 64 captures per
  call, oldest deadline first, each under a reserved-producer receipt keyed by
  cutoff and id and rechecked under the writer.
- Existing check:
  `harness_capture_evidence_expires_at_its_deadline_and_replay_never_renews_it`
  (`crates/daemon/tests/harness_sources.rs:3338`);
  `harness_capture_expiry_keeps_held_and_cited_evidence_and_drains_in_pages`
  (`crates/daemon/tests/harness_sources.rs:3482`);
  `a_replayed_expiry_receipt_is_not_counted_as_a_retirement`
  (`crates/kernel/src/local_file.rs:728`). Status: unaudited.

## Failure scenario

A sweep that held the writer for an unbounded backlog would stall other commits.

## Timing windows and dependencies

Between selection under the reader and each commit under the writer.

## What a test must construct

The existing page-drain test.

Required faults and enabling state: More than 64 expired captures; a hold
acquired between selection and commit.

## Investigation log

### Q: Does any open question remain for this record?

- Sources examined: the specification catalog entry, the code and tests cited above.
- Findings: the cited code and tests agree with the guarantee at this tip.
- Missing evidence: none for the guarantee as stated.
- Conclusion: resolved with answer - no open question.
