# sc-descriptor-revision-exceeds-live-predecessor-only

## Discovery trigger

U9's historical-head guard and U7's re-run reconciliation both depend on how
the Kernel orders descriptor revisions in one lineage. Part A recorded that
the revision rule compares against the live descriptor only, and asked
whether a revision below a retired one is accepted. The fault map ranked
that question first (F10).

## Evidence trail

Verified at HEAD `5a842d998`.

- `crates/kernel/src/source_descriptor.rs`, `Envelope::publish_verified`:
  computes `guarded = class == RepositoryDocuments`; a non-document request
  with `expected_head: Some(_)` is `InvalidInput`; reads `live_lineage`; a
  revision at or below the live revision is `RevisionNotAdvanced`. For
  `repository_documents` only, `historical_head` must be below the revision,
  the revision must be at least 1, and the head must equal
  `expected_head`, else `RevisionNotAdvanced` or `HeadMismatch`. With a
  predecessor the commit calls `correct_observation_inner` (invalidates and
  sets `superseded_by`); without one, `insert_observation_inner`.
  `replaced_object_id` is the predecessor's object id.
- `live_lineage`: more than one live row is `CorruptCanonicalRow`; with no
  live row, the highest historical row is read for domain and collision
  consistency only.
- `historical_head` and `KernelStore::source_lineage_head`: the maximum
  `source_revision` over live and retired descriptors of the lineage.
- `descriptor_object_id(lineage, revision)`: a republished revision equal to
  a retired one collides on object id.

Tests read at HEAD (`crates/kernel/tests/kernel_source_descriptors.rs`):

- `a_successor_invalidates_exactly_its_predecessor_in_one_commit_and_replays_from_its_receipt`:
  revision 2 replaces revision 1 in one commit (`invalidated_commit_seq` and
  `superseded_by` set, `created_commit_seq` equal); another lineage is
  untouched; stale revisions are `RevisionNotAdvanced`.
- `representations_and_spans_of_one_source_are_separate_lineages` and
  `concurrent_successors_serialize_into_one_chain`: `RevisionNotAdvanced`
  for non-advancing revisions.
- `retirement_preserves_domain_ownership_without_reusing_old_revision_ids`
  (present at baseline): publish 1 and 3, retire 3; a foreign domain is
  `DomainMismatch` with `tip()` unchanged; "With no live head, revision 2
  starts a fresh chain" with `replaced_object_id: None`; revision 3 again is
  `Kernel(Conflict)` on the object id; revision 2 again is
  `RevisionNotAdvanced`.
- `repository_document_revisions_exceed_the_historical_head_including_retired_rows`:
  the guarded class refuses 2, 3, and 0 after retiring 3; its doc comment
  names coverage witness `retired_lineage_republished` and states messages
  keep the live-predecessor rule.
- `crates/daemon/tests/harness_sources.rs`,
  `equal_text_stays_distinct_and_revisions_replay_or_succeed_atomically`
  and `permanent_refusal_reports_failed_retirement_attempt`: the publisher
  surfaces `RevisionNotAdvanced` as a `Descriptor` refusal.

## Failure scenario

A stale revision replaces a newer live descriptor, or two live rows of one
lineage exist. For non-document classes, a native source that reissues a
lower revision after retirement starts a fresh chain; that is the current
rule, not a defect.

## Timing windows and dependencies

The check and the supersession share one writer transaction.
`concurrent_successors_serialize_into_one_chain` covers racing successors.
U7 retires a re-run's predecessor generation in the publishing commit and
does not change this rule.

## What a test must construct

F9: two revisions in either order with a live predecessor. F10: a retired
lineage followed by a lower revision, per class.

## Investigation log

### Q: Does publishing revision 2 after retiring revision 3 succeed at baseline?
- Sources examined: `git grep` at `08de9051a`; the test above.
- Findings: yes for `messages`. The baseline test
  `retirement_preserves_domain_ownership_without_reusing_old_revision_ids`
  already asserted it; the source's "no test" note was wrong. At HEAD the
  same holds for every class except `repository_documents`, which U9 guards.
- Conclusion: resolved with answer; the retired-lineage acceptance is now a
  tested fact for non-document classes.

### Q: Should the historical-head guard apply to every class (OQ13)?
- Sources examined: ticket-1081 "Resolved OQ13"; `publish_verified`.
- Findings: `repository_documents` only; other classes must pass
  `expected_head: None`.
- Conclusion: resolved with answer in U9.
