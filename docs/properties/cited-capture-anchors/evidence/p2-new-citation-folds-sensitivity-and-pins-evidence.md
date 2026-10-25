# p2-new-citation-folds-sensitivity-and-pins-evidence

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), proposed obligation. OQ2-10 resolved as decision-event placement, which removes the fold and keeps the pin. Owner: the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135).

## Evidence trail

`EVIDENCE_CITED_SQL` (`crates/kernel/src/slice/write.rs:42`) counts decision events of live decisions, so an event citation pins the evidence through `expire_harness_captures` (`crates/kernel/src/local_file.rs:245`).

## Failure scenario

A sweep that retires a cited capture.

## Timing windows and dependencies

A sweep after `retain_until`.

## What a test must construct

Cited evidence past `retain_until` with a live decision; a sweep; a retirement attempt.

## Investigation log

### Q: Does code for this obligation exist at the branch tip?

- Sources examined: the surfaces named in the trail, read at the branch that adds this part.
- Findings: No. The record stays `proposed` until its owner lands the behavior and the check.
- Missing evidence: the implementation.
- Conclusion: unresolved, needs the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135).
