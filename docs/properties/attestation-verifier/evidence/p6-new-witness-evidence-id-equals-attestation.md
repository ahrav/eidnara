# p6-new-witness-evidence-id-equals-attestation

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N4. Converted to active by [#1172](https://github.com/ahrav/eidnara/issues/1172), which lands `crates/daemon/src/attestation_verifier/attest.rs` and the fixture-store tests in `crates/daemon/tests/attestation_verifier_attest.rs`.

## Evidence trail

`WriteSet::check` (`crates/daemon/src/attestation_verifier/attest.rs:328`) parses the witness detail, a `WitnessDetail` with `deny_unknown_fields` at version 1, and holds only when the detail's evidence id, the citation event's evidence id, and the compared evidence id (`VerifierInput::evidence_id`) are one id, the detail names the write set's blob id and the claim's commit oid, sha256, path, descriptor object id, lineage, and descriptor revision, the citation event's kind is `verifier_citation`, the attestation's object, decision, and source ids and the witness's observation and object ids equal `Identity::of(model object id, oid)`, both specs carry the model decision's domain and served sensitivity and one scope and the attestation its `anchor_id`, the witness's source triple equals the attestation's, the attestation's source kind is `verifier` at revision 1, the witness kind is the document kind's, and neither spec carries an `evidence_id`. `attest_match` (`:142`) returns `Outcome::InvalidWriteSet` from its envelope before any write when the check fails (`:166`), and `WriteSet::commit` (`:381`) returns it before the commit. Both paths build their set with `WriteSet::from_row` (`:234`), which takes every field the check compares from the classification's input, the row's scope, `Identity::of`, and constants, so a set it builds passes the check; the mutation tests below exercise the refusal through `WriteSet::commit`, and their unmutated set, built through `WriteSet::new` and `from_row`, passes. Inside the envelope `recheck` (`:582`) also requires the descriptor the claim came from to still cite the compared evidence.

The tests share one fixture in `crates/daemon/tests/attestation_verifier_attest.rs`: a Kernel store with a `Sensitive` document evidence row, the `repository_documents` descriptor citing it, and a model decision under `(ModelInference, AssistantInference)` with a `reachable_from` anchor at a git checkout's commit, citing the evidence through a `memory_capture_citation` event or its own `evidence_id`. The end-to-end tests classify the decision with `classify`, compute the verdict with `verify` against the checkout, and call `attest`, the order a pass will use; the write-set check tests build and commit `WriteSet` directly, and the input-guard tests pass hand-built `Evaluation` values to `attest`. `a_write_set_citing_other_evidence_or_another_revision_is_refused_before_the_commit` (`crates/daemon/tests/attestation_verifier_attest.rs:749`) builds a write set, confirms it passes, and applies twenty-one mutations: the witness at revision 2, the witness under another source id, the detail naming another live evidence row, the citation naming it, the `config_present` kind, `Sensitive` on the attestation and on the witness, no anchor, the `source` domain on the attestation and on the witness, another attestation object id, decision id, and shared source id, another witness observation id and object id, a row-level `evidence_id` on the attestation and on the witness, a detail at `witness_version` 2, another blob id on the write set, and a detail naming another commit oid or artifact digest. Each fails the check and returns `invalid_write_set` with the tip unchanged and no write-set row; the unmutated set then commits. `a_write_set_naming_another_document_or_revision_is_refused_before_the_commit` (`:1469`) applies nine more: a detail naming another path, descriptor, lineage, or descriptor revision, another witness scope, another citation kind, both rows at source revision 2, both rows under source kind `assistant`, and no detail; each fails the check and returns `invalid_write_set` with nothing written. `an_unavailable_verdict_writes_nothing` (`:685`) passes a `Match` and a `Mismatch` with no blob id, each `invalid_write_set` with nothing written.

## Failure scenario

A `Verified` attestation citing E whose witness detail names E'.

## Timing windows and dependencies

None; the check runs before the commit.

## What a test must construct

F1, F3: a detail or citation naming another live evidence row; a witness at another revision.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record and this file, read at the #1172 branch tip.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.
