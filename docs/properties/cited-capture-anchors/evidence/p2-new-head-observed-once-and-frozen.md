# p2-new-head-observed-once-and-frozen

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), proposed obligation. Resolved placement: OQ2-4, read at admission and frozen in the plan (owner decision 2026-10-25). Owner: the U2 ticket [#1134](https://github.com/ahrav/eidnara/issues/1134), with U3 completing the anchor clause.

## Evidence trail

The receipt digest already covers every byte of the frozen plan (`crates/daemon/src/memory_capture.rs:1167`), so a `head` field in the plan is covered by the receipt. U2 adds the field with a serde default and reads HEAD once per submit through the isolated open (`crates/daemon/src/git_sources.rs:138`).

## Failure scenario

Two jobs of one submission frozen with different HEADs, or an anchor whose oid differs from its plan.

## Timing windows and dependencies

A checkout between the submit and a retry.

## What a test must construct

A two-job submission asserting identical frozen heads, then a checkout and a retry asserting the frozen value is unchanged.

## Investigation log

### Q: Does code for this obligation exist at the branch tip?

- Sources examined: the surfaces named in the trail, read at the branch that adds this part.
- Findings: No. The record stays `proposed` until its owner lands the behavior and the check.
- Missing evidence: the implementation.
- Conclusion: unresolved, needs the U2 ticket [#1134](https://github.com/ahrav/eidnara/issues/1134), with U3 completing the anchor clause.
