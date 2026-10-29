# p6-new-verifier-input-anchor-and-document-citation

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N1. A proposed obligation that [#1171](https://github.com/ahrav/eidnara/issues/1171) converts to active.

## Evidence trail

`class_of` (`crates/daemon/src/attestation_verifier/classify.rs:199`) decides one class per decision, in this order:

1. The standing read (`crates/daemon/src/attestation_verifier/classify.rs:209`). An absent object, an object that is not a decision, a decision not live at the sequence: `NotInput`.
2. `source_kind == "verifier"` (`crates/daemon/src/attestation_verifier/classify.rs:219`): `self_source`, before any anchor or citation read.
3. A latest own admission missing or with a disposition other than `Active`: `NotInput`.
4. The anchor. No `anchor_id`, or no anchor live at the sequence: `no_anchor`. The decoded condition from `anchor_condition_at` (`crates/daemon/src/attestation_verifier/classify.rs:246`; `crates/kernel/src/anchor_write.rs:468`) other than `GitCondition::ReachableFrom`, so `Exact`, `ReachableBetween`, and every non-git kind: `anchor_not_git`. A row `validate_anchor_row` refuses is `CorruptCanonicalRow`, an error and no class.
5. The citations from `linked_decision_evidence_within_budget` (`crates/daemon/src/attestation_verifier/classify.rs:254`), at most `MAX_CITATIONS` (`crates/daemon/src/attestation_verifier/classify.rs:37`), the decision's own evidence and its newest events' in the read's order. None: `no_citation`.
6. Each cited row in order: an invalidated row is `evidence_retired`; otherwise `source_descriptor_citing_evidence` (`crates/daemon/src/attestation_verifier/classify.rs:266`; `crates/kernel/src/source_descriptor.rs:1213`) for `repository_documents` returns `Live`, `Retired` (`descriptor_retired`), or `Uncited` (no document descriptor). A live descriptor whose `artifact_digest` is not 64 lowercase hex digits is `digest_missing`; one with a digest returns `Verifiable` at once.
7. No cited row verifiable: the first cited row's reason other than "no document descriptor", else `citation_not_document`.

`VerifierInput::new` (`crates/daemon/src/attestation_verifier/classify.rs:303`) builds the `Claim` from the descriptor identity's `relative_path`, the parsed digest, the evidence row's `byte_length`, and the anchor's oid, and maps the identity's `repository_id` to the first bound root with it (`crates/daemon/src/attestation_verifier/classify.rs:326`), or carries `Err(Reason::RepositoryUnbound)`. The input names the evidence by id and holds no evidence bytes.

Checks in `crates/daemon/tests/attestation_verifier_classify.rs`: `each_unverifiable_decision_classifies_under_exactly_one_reason_and_nothing_is_written` (`:326`) constructs `citation_not_document` (`messages` only), `no_anchor`, `anchor_not_git` for `Exact`, `ReachableBetween`, and `deployment_revision`, `evidence_retired` through `delete_artifact` after a `messages` citation, `descriptor_retired` through a retired descriptor, and `no_citation`, asserts the tip unchanged across every classification, and keeps a verifiable control decision citing the same document; `a_descriptor_without_a_digest_is_digest_missing` (`:470`); `a_decision_outside_the_input_set_counts_under_no_reason` (`:550`); `a_decision_citing_a_message_and_a_document_is_verifiable_through_the_document` (`:599`) compares the claim's path, digest, byte length, and oid with the descriptor, evidence, and anchor rows. The Kernel reads are checked by `the_evidence_keyed_read_returns_the_live_document_descriptor_citing_the_evidence` (`crates/kernel/tests/kernel_source_descriptors.rs:3252`) and `the_anchor_read_returns_the_decoded_condition_and_refuses_an_undecodable_row` (`crates/kernel/tests/kernel_anchor_write.rs:869`).

## Failure scenario

A classifier that resolved a `messages` descriptor as a document, or that treated a retired descriptor or invalidated evidence as live, would hand the verifier a claim whose bytes the store no longer stands behind, and a later `Match` would attest it.

## Timing windows and dependencies

F3: evidence, a descriptor, or an anchor invalidated before the classification's sequence. Evidence a live decision cites can be invalidated only by `delete_artifact`, which skips the citation guard (`p6-artifact-deletion-invalidates-cited-evidence-without-the-citation-guard`). F14 precondition: document evidence is `Sensitive`, and the classification carries its id only.

## What a test must construct

Each unverifiable shape in the memory-capture citation placement (event-level citations), a decision citing a `messages` and a `repository_documents` descriptor, and a decision retired or marked `Stale` after an earlier sequence.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the #1171 branch tip.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Is `ReachableBetween.start_oid` a legal input?

- Sources examined: the #1171 ticket's Design Decisions, OQ6-10.
- Findings: The owner decided `ReachableBetween` is not verified in P6 because no producer emits it.
- Missing evidence: none.
- Conclusion: resolved with answer: `anchor_not_git`.

### Q: Which reason names a decision whose cited document descriptor was superseded by a revision citing other evidence?

- Sources examined: `source_descriptor_citing_evidence` and its test.
- Findings: The read finds the superseded descriptor, not live at the sequence, and returns `Retired`; the newer revision cites other evidence and does not resolve the decision's citation.
- Missing evidence: a daemon test of supersession; the Kernel test covers retirement.
- Conclusion: resolved with answer: `descriptor_retired`.

### Q: Which descriptor resolves when several live document descriptors cite one evidence row?

- Sources examined: `source_descriptor_citing_evidence`; `publish_batch` in `crates/daemon/src/harness_sources.rs`, whose units of equal class and text cite one evidence object.
- Findings: The read returns the first live one in object id order, so the claim's path is that descriptor's; the digest and byte length are equal for every descriptor citing the row.
- Missing evidence: none.
- Conclusion: resolved with answer.
