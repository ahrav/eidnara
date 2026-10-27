# p2-citing-decision-carries-at-least-its-evidence-class

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), existing-behavior record for S2, touchpoints C11 and OQ2-10. Contradiction K4 shows the fold would hide row-cited capture memories from auto-inject.

## Evidence trail

`insert_decision_inner` (`crates/kernel/src/slice/write.rs:146`) replaces the asserted class with `fold_cited_evidence_class` (`:980`), which reads the live evidence row's `sensitivity_class` and returns `asserted.restrictive(stored)`. `insert_observation_inner` applies the same fold. An absent or invalidated evidence row leaves the class as asserted and `require_parents` refuses the insert.

The decision-event path (`append_decision_event`, `:208`) checks evidence liveness and applies no fold, which is why the owner chose event placement for OQ2-10.

## Failure scenario

A `Normal` decision citing `Sensitive` evidence would serve the evidence class through ordinary reads.

## Timing windows and dependencies

None.

## What a test must construct

Evidence stored `Sensitive`, a decision asserted `Normal` citing it, and a read asserting the stored class is `Sensitive` and the served visibility matches.

## Investigation log

### Q: Do the cited checks exist at the branch tip under the cited names?

- Sources examined: `crates/kernel/tests/kernel_slice.rs:1453`, `:1526`; `crates/kernel/tests/kernel_review_acceptance.rs:988`, read at the branch that adds this part (base `3a3b916b7`).
- Findings: Each cited line is the first line of the named test or function.
- Missing evidence: none.
- Conclusion: resolved with answer.
