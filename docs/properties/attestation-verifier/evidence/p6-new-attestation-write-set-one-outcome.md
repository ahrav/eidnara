# p6-new-attestation-write-set-one-outcome

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N3. Converted to active by [#1172](https://github.com/ahrav/eidnara/issues/1172), which lands `crates/daemon/src/attestation_verifier/attest.rs` and the fixture-store tests in `crates/daemon/tests/attestation_verifier_attest.rs`.

## Evidence trail

`WriteSet::write` inserts the attestation (`crates/daemon/src/attestation_verifier/attest.rs:369`), the witness (`:370`), the witness admission (`:372`), the causality record (`:391`), and the citation event (`:417`), all inside the one closure `run` (`:494`) passes to `KernelStore::commit_within_budget`; the receipt commits with them. Any refusal returns from the closure with an error, so the transaction rolls back. `attest` returns one `Outcome` per call. `WriteSet::new` returns `Changed` when the model decision's row is absent at the classification's sequence; `a_write_set_built_before_the_model_decision_existed_is_changed` (`crates/daemon/tests/attestation_verifier_attest.rs:727`) builds one at a sequence before the decision was written.

The tests share one fixture in `crates/daemon/tests/attestation_verifier_attest.rs`: a Kernel store with a `Sensitive` document evidence row, the `repository_documents` descriptor citing it, and a model decision under `(ModelInference, AssistantInference)` with a `reachable_from` anchor at a git checkout's commit, citing the evidence through a `memory_capture_citation` event or its own `evidence_id`. The end-to-end tests classify the decision with `classify`, compute the verdict with `verify` against the checkout, and call `attest`, the order a pass will use; the write-set check tests build and commit `WriteSet` directly, and the input-guard tests pass hand-built `Evaluation` values to `attest`. `a_rollback_after_the_admission_leaves_no_row_and_a_reopened_store_keeps_every_row` (`crates/daemon/tests/attestation_verifier_attest.rs:1178`) creates a SQLite trigger that aborts the insert of a `verifier_citation` decision event, the last write, after the admission and the causality record. `attest` returns `Refused`, the tip is unchanged, and no verifier object, causality object, citation event, verifier admission, or witness observation exists; the model decision's facts are unchanged. After the trigger is dropped the write commits; the store is closed and its data home opened again; every companion row is present, the attestation is `Verified` with its `DerivedReinjection` record and its witness detail, and a call at a new target replays the receipt at the first commit's sequence.

## Failure scenario

An attestation at `Candidate` with no observation after a crash.

## Timing windows and dependencies

A fault after the admission inside the envelope; a restart after the commit.

## What a test must construct

F11: a fault-injected rollback after the admission; a commit, a close, then a fresh open.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record and this file, read at the #1172 branch tip.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Does the reopen model a crash?

- Sources examined: the test's `Store::reopen`; a trial that abandoned the live store with `std::mem::forget` before the second open.
- Findings: A live store holds its data home's writer lock, so the second open of an abandoned store fails with "kernel store is held by another writer". The test closes the store and opens the data home again; no process is killed mid-write, so the record names this a fresh open after close.
- Missing evidence: a process kill between the commit and its acknowledgment.
- Conclusion: resolved with answer: the test shows the committed rows and receipt are durable across a fresh open.
