# p6-new-verifier-input-anchor-and-document-citation

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N1. A proposed obligation that [#1171](https://github.com/ahrav/eidnara/issues/1171) converts to active.

## Evidence trail

`class_of` (`crates/daemon/src/attestation_verifier/classify.rs:266`) decides one class per decision, in this order:

1. The standing read (`crates/daemon/src/attestation_verifier/classify.rs:276`). An absent object, an object that is not a decision, a decision not live at the sequence, or a latest own admission missing or with a disposition other than `Active` (`crates/daemon/src/attestation_verifier/classify.rs:286`): `NotInput`. A `verifier` decision marked `Stale` is `NotInput`, since a stale attestation is outside the input set.
2. `source_kind == "verifier"` (`crates/daemon/src/attestation_verifier/classify.rs:292`): `self_source`. Both checks use the standing read alone, so they precede every anchor and citation read.
3. The anchor. No `anchor_id`, or no anchor live at the sequence: `no_anchor`. The decoded condition from `anchor_condition_at` (`crates/daemon/src/attestation_verifier/classify.rs:299`; `crates/kernel/src/anchor_write.rs:468`) other than `GitCondition::ReachableFrom`, so `Exact`, `ReachableBetween`, and every non-git kind: `anchor_not_git`. A row `validate_anchor_row` refuses is `CorruptCanonicalRow`, an error and no class.
4. The citations from `citations` (`crates/daemon/src/attestation_verifier/classify.rs:351`): the decision row's own `evidence_id` first, with its liveness from `evidence_liveness_at_within_budget` (`:361`), then the event-level citations of other evidence from `linked_decision_evidence_within_budget` (`:376`), newest first, at most `MAX_EVENT_CITATIONS` (`crates/daemon/src/attestation_verifier/classify.rs:46`, 32). An event citation never displaces the row-level one. None: `no_citation`.
5. Each cited row in that order through `resolve_citation` (`crates/daemon/src/attestation_verifier/classify.rs:400`): an invalidated row is `evidence_retired`; otherwise `source_descriptor_citing_evidence` (`crates/daemon/src/attestation_verifier/classify.rs:409`; `crates/kernel/src/source_descriptor.rs:1213`) for `repository_documents` returns `Live`, `Retired` (`descriptor_retired`), or `Uncited` (no document descriptor). A live descriptor whose `artifact_digest` is not 64 lowercase hex digits is `digest_missing`; one with a digest returns `Verifiable` at once.
6. No cited row verifiable: `ClassifyError::InvalidDocumentPath` when a cited live document descriptor's path is outside the document path rule, ahead of every reason; otherwise the first cited row's reason other than "no document descriptor", else `citation_not_document`.

`descriptor_retired` covers a cited descriptor revision that is no longer live at the sequence: retired or superseded by a newer revision of its lineage. Invalidated evidence is `evidence_retired` before the descriptor read. A newer live revision of the lineage cites other evidence and does not resolve the decision's citation, because C8 requires a live descriptor citing the cited evidence.

`VerifierInput::new` (`crates/daemon/src/attestation_verifier/classify.rs:428`) builds the `Claim` from the descriptor identity's `relative_path`, the parsed digest, the evidence row's `byte_length`, and the anchor's oid, and maps the identity's `repository_id` to the first bound root with it (`crates/daemon/src/attestation_verifier/classify.rs:449`), or carries `Err(Reason::RepositoryUnbound)`. A stored `relative_path` that passes the descriptor row checks and that `Claim::new` refuses makes that cited row `ClassifyError::InvalidDocumentPath` (`crates/daemon/src/attestation_verifier/classify.rs:448`); the loop remembers the first such refusal and continues, so a later cited row that resolves to a verifiable document still makes the decision verifiable, and the refusal is returned only when none does. The result is independent of where the refused row sits in citation order. The refusal is distinct from the Kernel's `CorruptCanonicalRow`, so the pass can count it against that decision alone. The input names the evidence by id and holds no evidence bytes.

Checks in `crates/daemon/tests/attestation_verifier_classify.rs`: `each_unverifiable_decision_classifies_under_exactly_one_reason_and_nothing_is_written` (`:355`) constructs `citation_not_document` (`messages` only), `no_anchor`, `anchor_not_git` for `Exact`, `ReachableBetween`, and `deployment_revision`, `evidence_retired` through `delete_artifact` after a `messages` citation and through a deleted row-level `evidence_id`, `descriptor_retired` through a retired descriptor, `no_citation`, and `descriptor_retired` for a retired descriptor cited before a digest-less document, where the first reason in citation order wins; it shows a deleted evidence row cited before a document and a document cited only by `decisions.evidence_id` both verifiable through the document, asserts the tip unchanged across every classification, and keeps a verifiable control decision citing the same document. `a_descriptor_without_a_digest_is_digest_missing` (`:547`); `a_stored_path_outside_the_document_path_rule_is_refused_for_that_decision` (`:639`) refuses a decision citing only a path outside the rule and one citing it before a digest-less document, and finds a decision citing it before a valid document verifiable; `the_row_level_citation_is_read_before_the_capped_event_citations` (`:697`) shows a row-level document citation verifiable behind 33 `messages` event citations that precede it in the linked-evidence order, a document cited as the 32nd event citation verifiable, a document cited as the 33rd event citation outside the cap, and a document cited as the 33rd event citation verifiable when the row's own evidence is also among the event citations, which are read once; `a_decision_outside_the_input_set_counts_under_no_reason` (`:755`); `a_decision_citing_a_message_and_a_document_is_verifiable_through_the_document` (`:806`) compares the claim's path, digest, byte length, and oid with the descriptor, evidence, and anchor rows. The Kernel reads are checked by `the_evidence_keyed_read_returns_the_live_document_descriptor_citing_the_evidence` (`crates/kernel/tests/kernel_source_descriptors.rs:3252`) and `the_anchor_read_returns_the_decoded_condition_and_refuses_an_undecodable_row` (`crates/kernel/tests/kernel_anchor_write.rs:869`), which also invalidates an anchor by SQL and finds it live before that sequence and absent from it on.

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

### Q: Should `descriptor_retired` stay the reason when a newer revision of the cited lineage is live?

- Sources examined: the #1171 ticket's Scope, which defines `descriptor_retired` as "a document lineage with no live revision"; C8 of the specification; `resolve_citation` and `source_descriptor_citing_evidence`.
- Findings: The code returns `descriptor_retired` when the cited revision is no longer live, including when a newer revision of its lineage is live and cites other evidence. The ticket's wording names a lineage with no live revision. C8 requires a live descriptor citing the cited evidence, which the newer revision is not.
- Missing evidence: an owner decision on whether the reason name should cover a superseded revision or a separate reason should name it.
- Conclusion: unresolved (needs human input).

### Q: Which descriptor resolves when several live document descriptors cite one evidence row?

- Sources examined: `source_descriptor_citing_evidence`; `publish_batch` in `crates/daemon/src/harness_sources.rs`, whose units of equal class and text cite one evidence object.
- Findings: The read returns the first live one in object id order, so the claim's path is that descriptor's; the digest and byte length are equal for every descriptor citing the row.
- Missing evidence: none.
- Conclusion: resolved with answer.
