# p2-new-lookup-runs-only-after-settlement

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), proposed obligation. Constraint C7 and C8. Owner: the U4 ticket [#1136](https://github.com/ahrav/eidnara/issues/1136).

## Evidence trail

U4 runs the citation lookup only after the settlement wait returns. `p2-deferred-source-units-publish-on-a-later-pass` shows a completed publish can leave a unit deferred, so settlement is job end, not descriptor presence.

## Failure scenario

A `descriptor_not_found` decision whose descriptor committed from a job still running within its deadline.

## Timing windows and dependencies

A slow source job and a fast submit.

## What a test must construct

A parked job and a submit inside its window, asserting zero lookups while running.

## Investigation log

### Q: Does code for this obligation exist at the branch tip?

- Sources examined: the surfaces named in the trail, read at the branch that adds this part.
- Findings: No. The record stays `proposed` until its owner lands the behavior and the check.
- Missing evidence: the implementation.
- Conclusion: unresolved, needs the U4 ticket [#1136](https://github.com/ahrav/eidnara/issues/1136).
