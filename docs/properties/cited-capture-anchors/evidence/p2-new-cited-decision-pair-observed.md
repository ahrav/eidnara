# p2-new-cited-decision-pair-observed

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), proposed obligation. Reachability witness for the N2 to N5 safety records. Owner: the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135) to `partial` and the U4 ticket [#1136](https://github.com/ahrav/eidnara/issues/1136) to `active`.

## Evidence trail

No capture decision carries an anchor or a citation at the branch tip.

## Failure scenario

A campaign that ends with a legal state never constructed, leaving the safety records vacuous.

## Timing windows and dependencies

None.

## What a test must construct

A decision with an anchor and a citation, and one decision per miss reason.

## Investigation log

### Q: Does code for this obligation exist at the branch tip?

- Sources examined: the surfaces named in the trail, read at the branch that adds this part.
- Findings: No. The record stays `proposed` until its owner lands the behavior and the check.
- Missing evidence: the implementation.
- Conclusion: unresolved, needs the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135) to `partial` and the U4 ticket [#1136](https://github.com/ahrav/eidnara/issues/1136) to `active`.
