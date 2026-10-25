# p2-new-anchor-and-citation-add-no-authority

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), proposed obligation for N6 and constraint C2, activated by the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135).

## Evidence trail

Admission is `(ModelInference, AssistantInference)` for every capture decision (`crates/daemon/src/memory_capture.rs:1301`). The anchor is a parent row and the citation is a decision event; neither is an input to `record_admission`, and the decision row's class stays its own because `decisions.evidence_id` is empty.

## Failure scenario

A citation that raised maturity would let a model raise its own authority by quoting a captured message.

## Timing windows and dependencies

None.

## What a test must construct

Four cited decisions and one uncited sibling in one session, compared on admission columns and served visibility (`crates/daemon/src/source_capture_tests.rs:3501`).

## Investigation log

### Q: Do the admission rows differ between cited and uncited decisions?

- Sources examined: `admission_decisions` rows in the end-to-end test.
- Findings: The distinct set of (source, taint, maturity, effective maturity, elevated support) has one member.
- Missing evidence: none.
- Conclusion: resolved with answer.
