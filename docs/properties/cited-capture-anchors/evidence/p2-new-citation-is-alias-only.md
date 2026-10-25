# p2-new-citation-is-alias-only

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), proposed obligation for N3 and constraint C9, activated by the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135).

## Evidence trail

`Citation::lookup` (`crates/daemon/src/memory_capture/cite.rs:245`) runs inside the publish envelope (`crates/daemon/src/memory_capture.rs:1257`). For block indexes `0..256` it derives the `messages` lineage id from the job row's `(project, harness, session_id, message_id, block_index)` with the identity builder source capture publishes under (`harness_sources::message_identity`) and the Kernel's source-identity encoder (`message_lineage`, `:131`), and reads each lineage with `live_source_descriptor_of_lineage`. An identity the encoder refuses ends the probe with no blocks (`an_identity_the_encoder_refuses_reads_no_descriptor_and_reports_a_miss`, `crates/daemon/src/memory_capture/cite.rs:445`). The job row comes from the memory store, not from the model output; the model names sources only as batch aliases (`source_1`, ...), which `parse_capture_batch` maps back and refuses when unknown. The cited evidence id is the descriptor's `detail.evidence_id`, a receipt key (`srcev:` plus the unit key) the job cannot compute. `PreparedCapture` denies unknown fields and has no evidence field.

## Failure scenario

A citation copied from model output would let the extraction cite any evidence id it chose.

## Timing windows and dependencies

None. The read and the writes share one envelope.

## What a test must construct

A published message cited through its own descriptor (`crates/daemon/src/source_capture_tests.rs:3501` compares each event's evidence id with the descriptor of the job's own message and block); an output naming a source outside the batch, refused before publish (`crates/daemon/src/source_capture_tests.rs:3750`).

## Investigation log

### Q: Can a descriptor of another session alias the job?

- Sources examined: `message_lineage` and the lineage encoding.
- Findings: The session id is an identity field of the lineage, so another session yields another lineage id.
- Missing evidence: a test with one message id published in two sessions.
- Conclusion: resolved with answer by construction; the two-session fixture is queued in `fault-map.md`.
