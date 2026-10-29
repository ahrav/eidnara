# p6-new-attestation-write-set-one-outcome

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N3. Converted to active by [#1172](https://github.com/ahrav/eidnara/issues/1172), which lands `crates/daemon/src/attestation_verifier/attest.rs` and the fixture-store tests in `crates/daemon/tests/attestation_verifier_attest.rs`.

## Evidence trail

`WriteSet::write` inserts the attestation (`crates/daemon/src/attestation_verifier/attest.rs:326`), the witness (`:327`), the witness admission (`:329`), the causality record (`:351`), and the citation event (`:366`), all inside the one closure `run` (`:439`) passes to `KernelStore::commit_within_budget`; the receipt commits with them. Any refusal returns from the closure with an error, so the transaction rolls back. `attest` returns one `Outcome` per call.

The tests share one fixture in `crates/daemon/tests/attestation_verifier_attest.rs`: a Kernel store with a `Sensitive` document evidence row, the `repository_documents` descriptor citing it, and a model decision under `(ModelInference, AssistantInference)` with a `reachable_from` anchor at a git checkout's commit, citing the evidence through a `memory_capture_citation` event or its own `evidence_id`. Each test classifies the decision with `classify`, computes the verdict with `verify` against the checkout, and calls `attest`, the order a pass will use. `a_rollback_after_the_admission_leaves_no_row_and_a_reopened_store_keeps_every_row` (`crates/daemon/tests/attestation_verifier_attest.rs:983`) creates a SQLite trigger that aborts the insert of a `verifier_citation` decision event, the last write, after the admission and the causality record. `attest` returns `Refused`, the tip is unchanged, and no verifier object, causality object, citation event, verifier admission, or witness observation exists; the model decision's facts are unchanged. After the trigger is dropped the write commits; the store is dropped and opened again; every companion row is present, the attestation is `Verified` with its `DerivedReinjection` record and its witness detail, and a call at a new target replays the receipt at the first commit's sequence.

## Failure scenario

An attestation at `Candidate` with no observation after a crash.

## Timing windows and dependencies

A fault after the admission inside the envelope; a restart after the commit.

## What a test must construct

F11: a fault-injected rollback after the admission; a commit then a fresh open.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record and this file, read at the #1172 branch tip.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Does the reopen model a crash?

- Sources examined: the test's `Store::reopen`.
- Findings: The store is dropped and opened on the same data home; no process is killed mid-write.
- Missing evidence: a process kill between the commit and its acknowledgment.
- Conclusion: resolved with answer: the test shows the committed rows and receipt are durable across a fresh open.
