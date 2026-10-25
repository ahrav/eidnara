# p2-new-head-unavailable-keeps-decision-admitted

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), proposed obligation. OQ2-11 resolved: a detached HEAD anchors at its commit. Owner: the U2 ticket [#1134](https://github.com/ahrav/eidnara/issues/1134) to `partial` and the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135) to `active`.

## Evidence trail

`snapshot_checkout` (`crates/kernel/src/applicability/checkout.rs:690`) shows `head_id()` refusing an unborn branch as `NoHead`; U2 uses the daemon open instead, which avoids the status scan, and maps unborn, unreadable, unsupported-format, and non-repository roots to `head_unavailable`.

## Failure scenario

A submission at an unborn repository that fails its jobs.

## Timing windows and dependencies

None.

## What a test must construct

Fixtures for an unborn HEAD, a plain directory, and an unsupported object format, asserting admission is unchanged and the reason is recorded.

## Investigation log

### Q: Does code for this obligation exist at the branch tip?

- Sources examined: the surfaces named in the trail, read at the branch that adds this part.
- Findings: No. The record stays `proposed` until its owner lands the behavior and the check.
- Missing evidence: the implementation.
- Conclusion: unresolved, needs the U2 ticket [#1134](https://github.com/ahrav/eidnara/issues/1134) to `partial` and the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135) to `active`.
