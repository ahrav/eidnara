# p2-new-anchor-and-citation-add-no-authority

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), proposed obligation. Constraint C2. Owner: the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135).

## Evidence trail

Admission classes are fixed at `(ModelInference, AssistantInference)` (`crates/daemon/src/memory_capture.rs:1209`); neither the anchor nor the event citation is an admission input.

## Failure scenario

A cited decision with higher effective maturity than an uncited sibling.

## Timing windows and dependencies

None.

## What a test must construct

One cited and one uncited job in one session, comparing admission rows.

## Investigation log

### Q: Does code for this obligation exist at the branch tip?

- Sources examined: the surfaces named in the trail, read at the branch that adds this part.
- Findings: No. The record stays `proposed` until its owner lands the behavior and the check.
- Missing evidence: the implementation.
- Conclusion: unresolved, needs the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135).
