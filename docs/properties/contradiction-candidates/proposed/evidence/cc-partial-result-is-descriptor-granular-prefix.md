# cc-partial-result-is-descriptor-granular-prefix

Verified at the #1167 branch tip (base `39a2eb3f9`).

## Discovery trigger

A pass stopped mid-way must leave rows a reader can trust as a complete top-K
per descriptor.

## Evidence trail

- `crates/daemon/src/contradiction_candidates/pass.rs:248` `score_descriptors`
  scores every descriptor in one scan of the loaded summaries and checks the
  budget before the scan and every 256 summaries in it; a stop there writes no
  row and counts every descriptor `descriptors_unscored`. It then writes the
  descriptors in publish order and checks the budget before each write; a stop
  there returns before that descriptor's write and counts it and every later
  descriptor `descriptors_unscored`.
- `crates/daemon/src/contradiction_candidates/pass.rs:234` `score_publication`
  counts the end once (`deadline` when the deadline passed, `budget_exhausted`
  for the daemon cancel).

## Failure scenario

Writing during scoring would leave a partial top-K that ranks wrong; counting
each stop check would count one exhaustion many times.

## Timing windows and dependencies

The deadline can pass inside the scan; the scan stops within 256 summaries and
writes nothing. A deadline between two writes leaves the earlier descriptors'
rows.

## What a test must construct

A deadline during descriptor 3 of 5 with K overlapping decisions, a zero budget,
and a cancel at descriptor 1 of 3 through the handler; all in
`crates/daemon/src/candidate_pass_tests.rs`.

## Investigation log

### Q: Per descriptor or at the end?

- Sources examined: the evidence trail above.
- Findings: per descriptor, one store write each, in publish order after one
  scan scores every descriptor.
- Missing evidence: none.
- Conclusion: resolved with answer.
