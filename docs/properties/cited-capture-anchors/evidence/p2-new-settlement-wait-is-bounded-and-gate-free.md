# p2-new-settlement-wait-is-bounded-and-gate-free

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), proposed obligation. Constraint C7, mechanism D4 and OQ2-2. Owner: the U4 ticket [#1136](https://github.com/ahrav/eidnara/issues/1136).

## Evidence trail

`CAPTURE_BUDGET` (`crates/daemon/src/source_capture.rs:35`) bounds the source job from its slot acquisition in `into_job` (`:615`). U4 adds a per-session settlement record and a bounded wait in the submit task before the blocking hop that takes the gate.

## Failure scenario

A wait inside the gate critical section.

## Timing windows and dependencies

A source job stalled past its budget.

## What a test must construct

A parked source job under a paused tokio clock, asserting the wait returns by `accepted_at + 5 s` and the gate is free while parked.

## Investigation log

### Q: Does code for this obligation exist at the branch tip?

- Sources examined: the surfaces named in the trail, read at the branch that adds this part.
- Findings: No. The record stays `proposed` until its owner lands the behavior and the check.
- Missing evidence: the implementation.
- Conclusion: unresolved, needs the U4 ticket [#1136](https://github.com/ahrav/eidnara/issues/1136).
