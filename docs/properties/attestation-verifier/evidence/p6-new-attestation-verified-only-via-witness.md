# p6-new-attestation-verified-only-via-witness

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N4. Converted to active by [#1172](https://github.com/ahrav/eidnara/issues/1172), which lands `crates/daemon/src/attestation_verifier/attest.rs` and the fixture-store tests in `crates/daemon/tests/attestation_verifier_attest.rs`.

## Evidence trail

The attestation's initial and only promoting admission, before the applicability step can add one `MarkStale` when its document republishes, is the request at `crates/daemon/src/attestation_verifier/attest.rs:396`: a `CodeObserved` or `ConfigObserved` event by document kind whose trigger is the witness observation inserted at `:394` with the attestation's source triple. `validate_trigger` (`crates/kernel/src/admission.rs:2807`) looks the trigger up by kind, liveness, and the subject's exact `(source_kind, source_id, source_revision)`; a failed lookup admits the event as `Other`, below `Verified`.

The tests share one fixture in `crates/daemon/tests/attestation_verifier_attest.rs`: a Kernel store with a `Sensitive` document evidence row, the `repository_documents` descriptor citing it, and a model decision under `(ModelInference, AssistantInference)` with a `reachable_from` anchor at a git checkout's commit, citing the evidence through a `memory_capture_citation` event or its own `evidence_id`. The end-to-end tests classify the decision with `classify`, compute the verdict with `verify` against the checkout, and call `attest`, the order a pass will use; the write-set check tests build and commit `WriteSet` directly, and the input-guard tests pass hand-built `Evaluation` values to `attest`. `a_match_commits_the_attestation_with_every_companion_row` (`crates/daemon/tests/attestation_verifier_attest.rs:431`) reads the attestation's own admission: event `code_observed`, trigger the witness object id, effective maturity `verified`. `a_witness_admitted_below_verified_aborts_the_envelope` (`:854`) installs a SQLite trigger that rewrites the witness's observation kind as it is inserted; the admission falls below `Verified` and the set aborts. `one_attestations_witness_cannot_promote_another` (`:1132`) commits attestation A, then inserts a `verifier` decision under attestation B's identity and admits it under trusted classes with A's witness as trigger; the admission's effective maturity is `candidate`.

## Failure scenario

`Verified` from insert alone, or one attestation's witness promoting another.

## Timing windows and dependencies

None; the witness and the admission share one envelope.

## What a test must construct

F1: a trigger of another kind; a trigger carrying another attestation's source triple.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record and this file, read at the #1174 branch tip.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.
