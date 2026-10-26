# uo-p-disputed-after-serve-from-committed-events-only

## Discovery trigger

Spec #1145 C12 and AC12.

Record kind: new obligation activated by #1154.

## Evidence trail

References are verified at the #1154 branch tip (the commit that adds this file;
its parent is `8ace6c9f8`).

- Join: `crates/kernel/src/use_outcome.rs:163`; read: `served_sessions_for`
  (`crates/kernel/src/use_outcome.rs:420`).
- The read takes the newest qualifying commit sequence once for the object and
  compares each serve's `served_commit_seq` against it, so a serve is disputed
  exactly when some qualifying event is later than it.
- The flag is computed in the read and stored nowhere; the read changes no
  visibility.

- Guarantee under test: For a serve at `served_commit_seq = n`,
  `disputed_after_serve` is true exactly when a committed `mark_disputed` or
  `contradict` on the served object and its registry lineage has a commit
  sequence greater than n.
- Existing check:
  `disputed_after_serve_counts_only_later_committed_disputes_on_the_same_lineage`
  (`crates/kernel/tests/kernel_use_outcome.rs:342`);
  `disputed_after_serve_is_judged_against_each_serve`
  (`crates/kernel/tests/kernel_use_outcome.rs:733`). Status: unaudited.

## Failure scenario

A join on `>=` would count the dispute the served rows already reflected.

## Timing windows and dependencies

Events in the serve's own commit are at n and do not count.

## What a test must construct

The join test (marker `uo_two_disposition_events_one_commit`).

Required faults and enabling state: A serve at n; dispositions at n + k, at n,
and before n; a null row and another lineage by SQL.

## Investigation log

### Q: A decision disputed then re-approved still counts: event existence cou

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: A decision disputed then re-approved still counts: event existence
  counts (resolved at #1154 ticketing).
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.
