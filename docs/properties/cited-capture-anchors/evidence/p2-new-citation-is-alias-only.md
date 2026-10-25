# p2-new-citation-is-alias-only

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), proposed obligation. Constraint C9. Owner: the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135).

## Evidence trail

The validated `message_id` is a batch id (`crates/daemon/src/memory_capture.rs:204`), and the evidence id is a receipt key the job cannot compute (`crates/daemon/src/harness_sources.rs:992`). U3 reads descriptors by lineage id inside the publish envelope and writes the read evidence id into a decision event.

## Failure scenario

A citation copied from a model-emitted field.

## Timing windows and dependencies

None.

## What a test must construct

Model output with an `evidence_id` key; a descriptor of another session with the same `message_id`.

## Investigation log

### Q: Does code for this obligation exist at the branch tip?

- Sources examined: the surfaces named in the trail, read at the branch that adds this part.
- Findings: No. The record stays `proposed` until its owner lands the behavior and the check.
- Missing evidence: the implementation.
- Conclusion: unresolved, needs the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135).
