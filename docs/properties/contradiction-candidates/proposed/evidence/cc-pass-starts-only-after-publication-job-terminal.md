# cc-pass-starts-only-after-publication-job-terminal

Verified at the #1167 branch tip (base `39a2eb3f9`).

## Discovery trigger

The pass must never spend the publication's 5 s budget, hold the capture commit
gate, or hold the publication slot, because session deletion and the next
publication wait on them.

## Evidence trail

- `crates/daemon/src/source_capture.rs:827` `publish_settled` runs inside the
  job's blocking closure; it destructures the job, so the slot permit
  (`source_capture::ActiveSlot`, `:473`) drops before the closure returns, and the commit
  gate guard was dropped at the end of `publish`.
- `crates/daemon/src/source_capture.rs:800` `run_settled` returns the settled
  publication only after the join, so the pass is offered from the async side
  after physical completion; `publish_settled` (`:827-871`) hands out every
  kept unit whatever count ended the job after the window batch's commit,
  since that commit landed and the memo holds the unit.
- `crates/daemon/src/lib.rs:9279` `capture_after_response` offers the pass from
  the same tracked task after `run_settled` returns, and only when the route's
  frozen flag and a memory store are present.
- `crates/daemon/src/contradiction_candidates/pass.rs:139` `run` fixes the
  pass's start at its admission, before `spawn_blocking`, and `finish`
  (`:166`) builds the pass's own `EvalBudget` from that start plus the budget
  (`:187-190`) and the daemon cancel.

## Failure scenario

A pass started inside the closure would hold the slot while it reads the Kernel,
so a busy pass would refuse every later publication `refused_queue_full` and
delay `session.delete` behind the gate.

## Timing windows and dependencies

The window between the job's terminal and the offer is async; a cancel in it
closes admission and counts `refused_shutting_down`. A publication that commits
while a pass scores competes for CPU and Kernel readers only; the latency test
measures that.

## What a test must construct

The `before_publish` hook held, a pass hook that records the slot and gate
state, a memo-hit republication, a replay after the memo is forgotten, and a
publication beside a running pass; all in
`crates/daemon/src/candidate_pass_tests.rs`.

## Investigation log

### Q: Does the pass carry the text or read it back? (OQ-C)

- Sources examined: the evidence trail above.
- Findings: the job keeps each published unit's text and receipt in
  `kept_for_pass` and hands them out; the rebuild test shows the text equals the
  retained bytes.
- Missing evidence: none.
- Conclusion: resolved with answer (carried).
