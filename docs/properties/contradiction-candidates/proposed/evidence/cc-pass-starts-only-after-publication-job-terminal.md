# cc-pass-starts-only-after-publication-job-terminal

Verified at the #1167 branch tip (base `39a2eb3f9`).

## Discovery trigger

The pass must never spend the publication's 5 s budget, hold the capture commit
gate, or hold the publication slot, because session deletion and the next
publication wait on them.

## Evidence trail

- `crates/daemon/src/source_capture.rs:700` `publish_settled` runs inside the
  job's blocking closure; it destructures the job, so the slot permit
  (`ActivePass` of the capture) drops before the closure returns, and the commit
  gate guard was dropped at the end of `publish`.
- `crates/daemon/src/source_capture.rs:675` `run_settled` returns the settled
  publication only after the join, so the pass is offered from the async side
  after physical completion.
- `crates/daemon/src/lib.rs:9246` `capture_after_response` offers the pass from
  the same tracked task after `run_settled` returns, and only when the route's
  frozen flag and a memory store are present.
- `crates/daemon/src/contradiction_candidates/pass.rs:140` `run` fixes the
  pass's deadline from its admission, before `spawn_blocking`, and `finish`
  (`:167`) builds the pass's own `EvalBudget` from that deadline and the
  daemon cancel.

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
