# p6-new-attestation-classes-and-cited-fields

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N3. Converted to active by [#1172](https://github.com/ahrav/eidnara/issues/1172), which lands `crates/daemon/src/attestation_verifier/attest.rs` and the fixture-store tests in `crates/daemon/tests/attestation_verifier_attest.rs`.

## Evidence trail

`WriteSet::new` (`crates/daemon/src/attestation_verifier/attest.rs:175`) reads the model decision's row at the classification's sequence and builds three specs: the attestation `DecisionSpec` with the model decision's domain, scope, `anchor_id`, decision kind, summary, and registry sensitivity, `evidence_id: None`, source triple `(verifier, <hash>, 1)`, and a rationale naming the model object id, oid, path, and digest; the witness `ObservationSpec`; and one `DecisionEventSpec` of kind `verifier_citation` citing the compared evidence id. The envelope inserts the decision (`:353`), admits it under `(TrustedLocalCode, DocumentKind::taint_class)` with the event `DocumentKind::event_kind` (`crates/daemon/src/attestation_verifier/classify.rs:517`) (`crates/daemon/src/attestation_verifier/attest.rs:356`), and appends the citation event last (`:392`). `WriteSet::check` (`:278`) requires the attestation's `anchor_id`, its domain and the witness's, and both rows' sensitivity to equal the model decision's before the commit.

The tests share one fixture in `crates/daemon/tests/attestation_verifier_attest.rs`: a Kernel store with a `Sensitive` document evidence row, the `repository_documents` descriptor citing it, and a model decision under `(ModelInference, AssistantInference)` with a `reachable_from` anchor at a git checkout's commit, citing the evidence through a `memory_capture_citation` event or its own `evidence_id`. Each test classifies the decision with `classify`, computes the verdict with `verify` against the checkout, and calls `attest`, the order a pass will use. `a_match_commits_the_attestation_with_every_companion_row` (`crates/daemon/tests/attestation_verifier_attest.rs:429`) reads the attestation's claim facts: its classes are `(TrustedLocalCode, CurrentCode)` for `ARCHITECTURE.md`, its `anchor_id` equals the model decision's, its row and its admission cite no evidence, and `linked_decision_evidence_within_budget` returns exactly the compared evidence through one decision event. The witness stores `evidence_id` NULL. `the_attestation_serves_at_the_model_decisions_sensitivity` (`:570`) repeats the write for a model decision citing the evidence by its row: the model decision serves `Sensitive`, and the attestation, whose rows cite nothing by FK, serves the same class. `a_match_on_a_configuration_document_commits_under_current_config` (`:1090`) attests a model decision citing a `config.toml` descriptor at a commit holding `config.toml`: the admission is `ConfigObserved` under `(TrustedLocalCode, CurrentConfig)`, `admit`, `verified`, and `active`, and the witness is `config_present` with the detail naming `config.toml`.

## Failure scenario

No citation event, a citation of another evidence row, or classes outside the trusted pairs at `Verified`.

## Timing windows and dependencies

None; the specs are built before the commit.

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
