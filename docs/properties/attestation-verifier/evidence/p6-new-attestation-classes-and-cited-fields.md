# p6-new-attestation-classes-and-cited-fields

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N3. Converted to active by [#1172](https://github.com/ahrav/eidnara/issues/1172), which lands `crates/daemon/src/attestation_verifier/attest.rs` and the fixture-store tests in `crates/daemon/tests/attestation_verifier_attest.rs`.

## Evidence trail

`attest_match` (`crates/daemon/src/attestation_verifier/attest.rs:143`) reads the model decision's row live at the committed tip inside its envelope through `Envelope::decisions_for_objects` (`crates/kernel/src/slice/read.rs:542`), and `WriteSet::new` (`crates/daemon/src/attestation_verifier/attest.rs:204`) reads it at the classification's sequence through a pooled reader; both hand the row to `WriteSet::from_row` (`:229`), which builds three specs: the attestation `DecisionSpec` with the model decision's domain, scope, `anchor_id`, decision kind, summary, and registry sensitivity, `evidence_id: None`, source triple `(verifier, <hash>, 1)`, and a rationale naming the model object id, oid, path, and digest; the witness `ObservationSpec`; and one `DecisionEventSpec` of kind `verifier_citation` citing the compared evidence id. The envelope inserts the decision (`:432`), admits it under `(TrustedLocalCode, DocumentKind::taint_class)` with the event `DocumentKind::event_kind` (`crates/daemon/src/attestation_verifier/classify.rs:530`) (`crates/daemon/src/attestation_verifier/attest.rs:435`), and appends the citation event last (`:480`). `WriteSet::check` (`:323`) requires the attestation's `anchor_id`, its domain and the witness's, and both rows' sensitivity to equal the model decision's before any write: `attest_match` calls it in the envelope before `WriteSet::write` (`:160`), and `WriteSet::commit` (`:378`) calls it before it opens the envelope. Inside the envelope `WriteSet::commit` rebuilds the set with `WriteSet::at_tip` (`:403`) from the model decision's row at the committed tip and returns `invalid_write_set` unless the caller's set equals it apart from the witness and citation timestamps (`:395`), so the decision kind, summary, rationale, and payload text it commits are the ones `attest` derives from the stored model decision.

The tests share one fixture in `crates/daemon/tests/attestation_verifier_attest.rs`: a Kernel store with a `Sensitive` document evidence row, the `repository_documents` descriptor citing it, and a model decision under `(ModelInference, AssistantInference)` with a `reachable_from` anchor at a git checkout's commit, citing the evidence through a `memory_capture_citation` event or its own `evidence_id`. The end-to-end tests classify the decision with `classify`, compute the verdict with `verify` against the checkout, and call `attest`, the order a pass will use; the write-set check tests build and commit `WriteSet` directly, and the input-guard tests pass hand-built `Evaluation` values to `attest`. `a_match_commits_the_attestation_with_every_companion_row` (`crates/daemon/tests/attestation_verifier_attest.rs:431`) reads the attestation's claim facts: its classes are `(TrustedLocalCode, CurrentCode)` for `ARCHITECTURE.md`, its `anchor_id` equals the model decision's, its row and its admission cite no evidence, and `linked_decision_evidence_within_budget` returns exactly the compared evidence through one decision event. The witness stores `evidence_id` NULL. `the_attestation_serves_at_the_model_decisions_sensitivity` (`:572`) repeats the write for a model decision citing the evidence by its row: the model decision serves `Sensitive`, and the attestation, whose rows cite nothing by FK, serves the same class. `a_match_on_a_configuration_document_commits_under_current_config` (`:1092`) attests a model decision citing a `config.toml` descriptor at a commit holding `config.toml`: the admission is `ConfigObserved` under `(TrustedLocalCode, CurrentConfig)`, `admit`, `verified`, and `active`, and the witness is `config_present` with the detail naming `config.toml`. `the_attestation_copies_the_model_decisions_kind_scope_and_summary` (`:1848`) attests a model decision with a scope and a `SECURITY` decision kind and reads the attestation's decision kind, scope, and summary equal to the model decision's row. `a_write_set_with_content_other_than_the_model_decisions_is_refused_in_the_envelope` (`:1927`) builds a write set with `WriteSet::new` and edits, one at a time, its decision kind, summary, rationale, witness summary, witness classification, and citation summary: each edited set passes `WriteSet::check`, `WriteSet::commit` returns `invalid_write_set`, and the tip is unchanged; the unedited set then commits.

## Failure scenario

No citation event, a citation of another evidence row, or classes outside the trusted pairs at `Verified`.

## Timing windows and dependencies

None; the specs are built and checked before any write of the commit.

## What a test must construct

F14: `Sensitive` document evidence; F15: a prose document under `CurrentCode`.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record and this file, read at the #1172 branch tip.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Is a configuration document's attestation exercised?

- Sources examined: `a_match_on_a_configuration_document_commits_under_current_config` in `crates/daemon/tests/attestation_verifier_attest.rs`.
- Findings: The fixture commits a `config.toml` attestation, whose admission is `ConfigObserved` under `(TrustedLocalCode, CurrentConfig)` at `verified` and `active` on a `config_present` witness.
- Missing evidence: none.
- Conclusion: resolved with answer.
