# p6-new-attestation-identity-unique-per-decision-oid

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N3. Converted to active by [#1172](https://github.com/ahrav/eidnara/issues/1172), which lands `crates/daemon/src/attestation_verifier/attest.rs` and the fixture-store tests in `crates/daemon/tests/attestation_verifier_attest.rs`.

## Evidence trail

`Identity::of` (`crates/daemon/src/attestation_verifier/attest.rs:66`) hashes `model object id` and `oid`, joined by a newline, with sha256 and derives the `source_id` (the hex hash), the attestation object id (`verattest:`), its decision id (`verattestdec:`), and the witness observation and object ids (`verwitness:`, `verwitnessobj:`). None of these prefixes is reserved: `insert_observation` refuses the descriptor (`srcocc:`, `srcdesc:`), causality (`claimcause:`, `claimcauseobj:`), and local-file (`localfile:`, `localfileobj:`) namespaces (`crates/kernel/src/slice/write.rs:169`). The causality record's ids are the Kernel's: `claimcauseobj:<subject>:<commit_seq>` (`crates/kernel/src/claim_causality.rs:202`), so they derive from the attestation's object id and the commit.

`intent` (`crates/daemon/src/attestation_verifier/attest.rs:642`) keys the `Match` write `(verifier, attest:<model object id>:<oid>)` and the `Mismatch` write `(verifier, stale:<model object id>:<oid>)`; the request digest is the sha256 of a JSON object holding the model object id and revision, the oid, the path, the cited digest and length, the compared evidence id, the blob id, and the verdict. `KernelStore::commit` replays a recorded key with an equal digest and refuses one with another digest as `Conflict` before it runs the closure; `run` (`crates/daemon/src/attestation_verifier/attest.rs:526`) maps that refusal to `Outcome::Conflict` because the closure never ran, and inside the closure it reads the opposite verdict's key with `stored_receipt` (`:538`) and stops with `Conflict` when that key is recorded under any digest.

The tests share one fixture in `crates/daemon/tests/attestation_verifier_attest.rs`: a Kernel store with a `Sensitive` document evidence row, the `repository_documents` descriptor citing it, and a model decision under `(ModelInference, AssistantInference)` with a `reachable_from` anchor at a git checkout's commit, citing the evidence through a `memory_capture_citation` event or its own `evidence_id`. The end-to-end tests classify the decision with `classify`, compute the verdict with `verify` against the checkout, and call `attest`, the order a pass will use; the write-set check tests build and commit `WriteSet` directly, and the input-guard tests pass hand-built `Evaluation` values to `attest`. `a_replay_returns_the_receipt_and_a_different_verdict_conflicts` (`crates/daemon/tests/attestation_verifier_attest.rs:895`) commits, replays at the same commit sequence with one attestation row, and then gets `Conflict` for a `Mismatch` and for a `Match` naming another blob, with the tip unchanged. `a_match_after_the_stale_path_for_the_same_oid_conflicts` (`:938`) gets `Conflict` for a `Match` after the stale path for the same pair. `one_attestations_witness_cannot_promote_another` (`:1132`) shows two pairs have distinct source ids.

## Failure scenario

Two live attestations for one `(decision, oid)`, or one attestation whose witness another attestation's trigger can borrow.

## Timing windows and dependencies

A retry after a crash or a replayed pass; a descriptor republished with another digest at the same oid, which changes the request digest.

## What a test must construct

F10: the key replayed with the same and with a different verdict; two decisions at one oid.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record and this file, read at the #1174 branch tip.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.
