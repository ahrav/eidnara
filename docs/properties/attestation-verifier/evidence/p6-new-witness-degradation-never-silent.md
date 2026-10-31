# p6-new-witness-degradation-never-silent

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N4. Converted to active by [#1172](https://github.com/ahrav/eidnara/issues/1172), which lands `crates/daemon/src/attestation_verifier/attest.rs` and the fixture-store tests in `crates/daemon/tests/attestation_verifier_attest.rs`.

## Evidence trail

After the witness admission at `crates/daemon/src/attestation_verifier/attest.rs:376`, the envelope checks the returned `AdmissionDecision` (`:389`): effective maturity other than `Verified` or disposition other than `Active` stops the envelope with `Outcome::WitnessDegraded`, and `run` (`:494`) returns that outcome after the Kernel rolls the transaction back. The amended oracle applies: an attestation's first admission has no prior, so a validated witness yields `Outcome::Admit` at `Verified`, and the outcome field is not read.

The tests share one fixture in `crates/daemon/tests/attestation_verifier_attest.rs`: a Kernel store with a `Sensitive` document evidence row, the `repository_documents` descriptor citing it, and a model decision under `(ModelInference, AssistantInference)` with a `reachable_from` anchor at a git checkout's commit, citing the evidence through a `memory_capture_citation` event or its own `evidence_id`. The end-to-end tests classify the decision with `classify`, compute the verdict with `verify` against the checkout, and call `attest`, the order a pass will use; the write-set check tests build and commit `WriteSet` directly, and the input-guard tests pass hand-built `Evaluation` values to `attest`. `a_witness_admitted_below_verified_aborts_the_envelope` (`crates/daemon/tests/attestation_verifier_attest.rs:854`) creates a SQLite trigger on the fixture store that rewrites a `verwitnessobj:` observation's kind to `code_absent` as it is inserted, so `validate_trigger` finds no `code_present` observation and admits the event as `Other`. `attest` returns `witness_degraded`, the tip is unchanged, no write-set row exists, and the model decision's facts are unchanged; after the trigger is dropped the same call commits. The fault is test-side SQL against the test store, not a production emitter. `a_match_commits_the_attestation_with_every_companion_row` (`:431`) reads the committed admission: outcome `admit`, effective maturity `verified`, disposition `active`.

## Failure scenario

A committed attestation at `Candidate`.

## Timing windows and dependencies

The witness row changed between its insert and the admission inside one envelope.

## What a test must construct

F1: a witness whose trigger fails validation inside the envelope.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record and this file, read at the #1172 branch tip.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Should repeated degradation stop the task?

- Sources examined: OQ6-11 as the owner confirmed it in [#1173](https://github.com/ahrav/eidnara/issues/1173); `ProjectRun::outcome` (`crates/daemon/src/attestation_verifier/bounded_pass.rs:719`) and `retry` (`:580`); `a_witness_degraded_k_passes_is_skipped_and_the_pass_continues` (`crates/daemon/tests/attestation_verifier_bounded_pass.rs:897`).
- Findings: The write set returns `witness_degraded`; the pass counts it and retries the decision at most K = 3 passes, then counts `witness_degraded_skipped`, moves the cursor past it, and continues with the decisions behind it.
- Missing evidence: none.
- Conclusion: resolved with answer: no; the task continues.
