# p2-new-miss-counters-track-rationale

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), proposed obligation. Constraint C14 and the METHOD effect-accounting rule. Owner: the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135).

## Evidence trail

U3 adds seven counters to the source-capture closed-vocabulary block (`crates/daemon/src/source_capture.rs:61`) and increments them after `commit_before` returns a receipt that was not replayed.

## Failure scenario

A counter incremented before the commit, counting rolled-back publishes.

## Timing windows and dependencies

A rollback after the reason was computed.

## What a test must construct

A rolled-back publish and a replayed receipt, asserting the counters are unchanged.

## Investigation log

### Q: Does code for this obligation exist at the branch tip?

- Sources examined: the surfaces named in the trail, read at the branch that adds this part.
- Findings: No. The record stays `proposed` until its owner lands the behavior and the check.
- Missing evidence: the implementation.
- Conclusion: unresolved, needs the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135).
