# p6-new-mismatch-stale-unavailable-silent

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N5. Converted to active by [#1172](https://github.com/ahrav/eidnara/issues/1172), which lands `crates/daemon/src/attestation_verifier/attest.rs` and the fixture-store tests in `crates/daemon/tests/attestation_verifier_attest.rs`.

## Evidence trail

`attest` (`crates/daemon/src/attestation_verifier/attest.rs:113`) returns `Skipped(Unavailable)` for every `Unavailable` verdict and `Skipped(NotVerifiable)` for a classification that is not verifiable, both before any commit. `mark_stale` (`:373`) commits under `(verifier, stale:<model object id>:<oid>)`: it rechecks the model decision, returns `Skipped(AlreadyStale)` from the envelope when its latest admission is already `Stale` (`:399`), and otherwise records one `MarkStale` admission (`:404`) under the classes of that latest admission with `evidence_id: None` and a reason naming the blob id, the path, the oid, and the cited sha256. A returned disposition other than `Stale` stops the envelope.

The tests share one fixture in `crates/daemon/tests/attestation_verifier_attest.rs`: a Kernel store with a `Sensitive` document evidence row, the `repository_documents` descriptor citing it, and a model decision under `(ModelInference, AssistantInference)` with a `reachable_from` anchor at a git checkout's commit, citing the evidence through a `memory_capture_citation` event or its own `evidence_id`. Each test classifies the decision with `classify`, computes the verdict with `verify` against the checkout, and calls `attest`, the order a pass will use. `a_mismatch_marks_the_model_decision_stale_once` (`crates/daemon/tests/attestation_verifier_attest.rs:582`) commits a checkout whose file differs from the cited document: the call returns `StaleCommitted`, the model decision has exactly one more admission row, its own admission is `mark_stale` at `stale` under `(ModelInference, AssistantInference)` with no evidence, the reason holds the oid, the digest, and the blob id, no verifier object exists, and the receipt's key is `stale:<model>:<oid>`. A second run classifies the decision `NotInput` and writes nothing, and the old classification replays. `a_mismatch_on_a_decision_already_stale_is_skipped` (`:645`) marks the decision `Stale` after classification and gets `AlreadyStale` with the tip unchanged. `an_unavailable_verdict_writes_nothing` (`:670`) passes two `Unavailable` verdicts and the tip is unchanged.

## Failure scenario

An attestation on `Mismatch`, a second `MarkStale`, or a `MarkStale` on `Unavailable` or on the attestation.

## Timing windows and dependencies

The model decision marked `Stale` between the classification and the commit.

## What a test must construct

A commit holding other bytes at the cited path; `Unavailable` verdicts.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record and this file, read at the #1172 branch tip.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Which two digests does the reason name?

- Sources examined: `Evaluation` in `crates/daemon/src/attestation_verifier.rs`.
- Findings: A mismatch the header size decides reads no content and has no observed content digest; every `Match` and `Mismatch` carries the blob id the path resolved to.
- Missing evidence: none.
- Conclusion: resolved with answer: the cited sha256 and the git blob id.
