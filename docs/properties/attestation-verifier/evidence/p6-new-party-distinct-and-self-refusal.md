# p6-new-party-distinct-and-self-refusal

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N6. A proposed obligation with two halves: the `self_source` half, checkable since [#1171](https://github.com/ahrav/eidnara/issues/1171), and the plugin-route half with the model-client clause, which converts with [#1172](https://github.com/ahrav/eidnara/issues/1172).

## Evidence trail

`self_source` half: `class_of` (`crates/daemon/src/attestation_verifier/classify.rs:266`) reads the registry row, decision row, and latest own admission through the standing read (`:276`), returns `NotInput` for a decision not live or whose latest own admission is not `Active` (`:286`), and then returns `Unverifiable::SelfSource` when `source_kind` is `VERIFIER_SOURCE_KIND` (`:292`), before the anchor read (`:299`) and the citation reads (`:351`). A `verifier` decision marked `Stale` is outside the input set. The test `a_verifier_decision_is_self_source_before_any_anchor_or_citation_read` (`crates/daemon/tests/attestation_verifier_classify.rs:565`) writes a `verifier` decision and two assistant decisions that cite one document, corrupts the `verifier` decision's anchor and one assistant decision's anchor so `validate_anchor_row` refuses them, and overwrites the cited descriptor's payload so the descriptor read refuses it. The assistant decisions fail with `CorruptCanonicalRow`, one at the anchor read and one at the citation read, as positive controls that both reads would fail; the `verifier` decision classifies `self_source`; and a second `verifier` decision marked `Stale` classifies `NotInput`.

Plugin-route half: `derive_classes` (`crates/daemon/src/kernel_routes/commit.rs:370`) has no `verifier` entry (`p6-plugin-source-kind-table-is-closed-and-classes-never-raised`). The refusal of a plugin commit under `verifier` and the absence of a model client in the verifier module are checks #1172 adds.

## Failure scenario

The verifier attests its own attestation: a `verifier` decision classified verifiable would let a later pass write an attestation of an attestation.

## Timing windows and dependencies

None for the `self_source` half: the check reads one registry column from the standing read.

## What a test must construct

A store holding a `verifier` decision whose anchor and cited descriptor the reads refuse, and a `verifier` decision marked `Stale`; a plugin write under `verifier` (#1172).

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the #1171 branch tip.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Does anything beyond the plugin route reserve `verifier`?

- Sources examined: OQ6-15.
- Findings: No; a plugin can still `MarkStale` or retire an attestation (gap G4).
- Missing evidence: see the conclusion.
- Conclusion: needs human input
