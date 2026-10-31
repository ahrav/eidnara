# p6-new-attestation-sibling-model-stays-candidate

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N3. Converted to active by [#1172](https://github.com/ahrav/eidnara/issues/1172), which lands `crates/daemon/src/attestation_verifier/attest.rs` and the fixture-store tests in `crates/daemon/tests/attestation_verifier_attest.rs`.

## Evidence trail

The `Match` envelope writes rows for the attestation only; its one event on the model decision's lineage is none. `recheck` (`crates/daemon/src/attestation_verifier/attest.rs:602-603`) runs in the envelope after `run` refuses a recorded opposite key and before any write: it compares the store's incarnation with the classification's target (`:611`) while the envelope holds the writer, then requires the model decision's registry row to be live with no successor, a decision with the same creating commit, at or before the classification's sequence (`:622`), domain, and source triple, and not `verifier`; its latest admission's classes to equal the classification's; its anchor to be live as `reachable_from` the claim's oid (`:648`) at the committed tip the envelope extends, one before its sequence, read in the envelope's transaction; and the cited descriptor to be the live revision of its lineage, still citing the compared evidence (`:658`). The live-descriptor read joins the cited evidence row and returns a descriptor only while that row is live, so a deleted evidence row reads as `Changed`. A difference stops the envelope with `Outcome::Changed`. On a `Match`, `attest_match` reads the model decision's row live at the committed tip before `recheck` runs, so a superseded or retired model decision has no row there and `WriteSet::at_tip` returns `Changed` at `:414`; the `recheck` liveness predicates refuse it on the `Mismatch` path. The causality record (`:454`) names the model decision at its registered revision, and the Kernel refuses a parent that is not live at that revision; `attest` maps that refusal to `Changed` and a malformed causality request to `InvalidWriteSet`.

The tests share one fixture in `crates/daemon/tests/attestation_verifier_attest.rs`: a Kernel store with a `Sensitive` document evidence row, the `repository_documents` descriptor citing it, and a model decision under `(ModelInference, AssistantInference)` with a `reachable_from` anchor at a git checkout's commit, citing the evidence through a `memory_capture_citation` event or its own `evidence_id`. The end-to-end tests classify the decision with `classify`, compute the verdict with `verify` against the checkout, and call `attest`, the order a pass will use. `a_match_commits_the_attestation_with_every_companion_row` (`crates/daemon/tests/attestation_verifier_attest.rs:431`) compares the model decision's registry row and own admission before and after the write and finds them equal, at `Candidate`, and reads the attestation's causality class as `DerivedReinjection` naming `(model object id, 1)`. `a_model_decision_changed_after_classification_aborts_the_set` (`:965`) classifies five decisions, then supersedes one, retires one, quarantines one, marks one `Stale`, and invalidates one's anchor; each returns `Changed`, and a decision whose descriptor is retired after classification returns `Changed` too, with no write-set row. In a second store, a decision whose compared evidence row is deleted with `delete_artifact` after classification returns `Changed` with the tip unchanged and no write-set row. `an_unrelated_attestation_does_not_abort_an_older_classification` (`:1067`) classifies two model decisions at one sequence, attests the first, and then attests the second on its older classification: it commits at `Verified`, so a commit outside the rechecked rows does not abort a set. `a_classification_from_another_incarnation_or_with_other_classes_is_changed` (`:1553`) passes a classification carrying another store's incarnation, and one naming another taint class, and both verdicts return `Changed` with the tip and the admissions unchanged. `a_mismatch_for_a_decision_re_disposed_after_classification_is_changed` (`:1596`) quarantines the decision after classification, and the mismatch returns `Changed` with no `MarkStale`. `a_descriptor_republished_after_classification_is_changed` (`:1624`) publishes revision 2 of the cited lineage, once citing the same evidence and once other evidence, and both verdicts return `Changed` with no row. `a_mismatch_for_a_decision_superseded_or_retired_after_classification_is_changed` (`:1775`) supersedes one model decision and retires another after classification, and each `Mismatch` returns `Changed` with its admissions and the tip unchanged. `a_classification_at_a_sequence_before_the_model_decision_is_changed` (`:1827`) passes a classification whose sequence precedes the model decision's creating commit, and both verdicts return `Changed` with no write-set row.

## Failure scenario

A `Replace` on the model decision, an attestation without causality, or an attestation of a superseded decision.

## Timing windows and dependencies

The model decision, its anchor, or its descriptor changed between the classification and the commit.

## What a test must construct

F2: supersede, retire, quarantine, and mark stale; F3: an invalidated anchor and a retired descriptor.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record and this file, read at the #1172 branch tip.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: What does the causality read report after the model decision is superseded?

- Sources examined: OQ6-12 in the #1172 ticket; `parents_existed` in `crates/kernel/src/claim_causality.rs`.
- Findings: The record names the parent at its revision. `parents_existed` checks only that the parent row existed at the requested snapshot with the named revision, and supersession leaves that row and revision in place, so a later supersession leaves the read `DerivedReinjection` while the attestation keeps `Verified`. OQ6-12 expected `Unknown(ParentUnavailable)`, which the Kernel reports only for a missing historical parent row or a revision mismatch.
- Missing evidence: no test reads the causality class after a later supersession.
- Conclusion: resolved with answer.
