# p6-new-attestation-sibling-model-stays-candidate

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N3. Converted to active by [#1172](https://github.com/ahrav/eidnara/issues/1172), which lands `crates/daemon/src/attestation_verifier/attest.rs` and the fixture-store tests in `crates/daemon/tests/attestation_verifier_attest.rs`.

## Evidence trail

The `Match` envelope writes rows for the attestation only; its one event on the model decision's lineage is none. `recheck` (`crates/daemon/src/attestation_verifier/attest.rs:513`) runs first in the envelope: it compares the store's incarnation with the classification's target (`:523`) while the envelope holds the writer, then requires the model decision's registry row to be live with no successor, a decision with the same creating commit, domain, and source triple, and not `verifier`; its latest admission's classes to equal the classification's; its anchor to be live as `reachable_from` the claim's oid (`:560`) at the committed tip the envelope extends, one before its sequence; and the cited descriptor to be the live revision of its lineage, still citing the compared evidence (`:551`). The live-descriptor read joins the cited evidence row and returns a descriptor only while that row is live, so a deleted evidence row reads as `Changed`. A difference stops the envelope with `Outcome::Changed`. The causality record (`:375`) names the model decision at its registered revision, and the Kernel refuses a parent that is not live at that revision.

The tests share one fixture in `crates/daemon/tests/attestation_verifier_attest.rs`: a Kernel store with a `Sensitive` document evidence row, the `repository_documents` descriptor citing it, and a model decision under `(ModelInference, AssistantInference)` with a `reachable_from` anchor at a git checkout's commit, citing the evidence through a `memory_capture_citation` event or its own `evidence_id`. Each test classifies the decision with `classify`, computes the verdict with `verify` against the checkout, and calls `attest`, the order a pass will use. `a_match_commits_the_attestation_with_every_companion_row` (`crates/daemon/tests/attestation_verifier_attest.rs:429`) compares the model decision's registry row and own admission before and after the write and finds them equal, at `Candidate`, and reads the attestation's causality class as `DerivedReinjection` naming `(model object id, 1)`. `a_model_decision_changed_after_classification_aborts_the_set` (`:963`) classifies five decisions, then supersedes one, retires one, quarantines one, marks one `Stale`, and invalidates one's anchor; each returns `Changed`, and a decision whose descriptor is retired after classification returns `Changed` too, with no write-set row. In a second store, a decision whose compared evidence row is deleted with `delete_artifact` after classification returns `Changed` with the tip unchanged and no write-set row. `an_unrelated_attestation_does_not_abort_an_older_classification` (`:1065`) classifies two model decisions at one sequence, attests the first, and then attests the second on its older classification: it commits at `Verified`, so a commit outside the rechecked rows does not abort a set.

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
- Findings: The record names the parent at its revision; a later supersession leaves the read `Unknown(ParentUnavailable)` while the attestation keeps `Verified`, which the ticket accepts.
- Missing evidence: no test reads the causality class after a later supersession.
- Conclusion: resolved with answer.
