# p2-new-miss-reason-matches-outcome

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), proposed obligation. D5 amended by OQ2-12: five citation values and two anchor values. Owner: the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135), with U4 adding `publish_budget_expired`.

## Evidence trail

U3 writes one `citation` and one `anchor` value into the rationale JSON (`crates/daemon/src/memory_capture.rs:1162` builds it today without either).

## Failure scenario

An uncited decision with no reason, or a cited decision carrying a miss reason.

## Timing windows and dependencies

None.

## What a test must construct

Each reason constructed separately.

## Investigation log

### Q: Does code for this obligation exist at the branch tip?

- Sources examined: the surfaces named in the trail, read at the branch that adds this part.
- Findings: No. The record stays `proposed` until its owner lands the behavior and the check.
- Missing evidence: the implementation.
- Conclusion: unresolved, needs the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135), with U4 adding `publish_budget_expired`.
