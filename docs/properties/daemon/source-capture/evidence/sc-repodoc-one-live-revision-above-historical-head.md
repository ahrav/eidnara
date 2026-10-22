# sc-repodoc-one-live-revision-above-historical-head

## Discovery trigger

Part B record on #1066 (comment 2), requirement R7, decision D12, and
C9. Part A record `sc-descriptor-revision-exceeds-live-predecessor-only`
showed that a lineage retired at revision 3 accepts revision 2 again. A
path-keyed document lineage needs every revision to be new. U9 (#1081)
landed the Kernel guard and U10 (#1089) the producer; both name this
record.

## Evidence trail

`crates/kernel/src/source_descriptor.rs`:

- `SourceDescriptorRequest::expected_head`: documented as the highest
  revision the caller observed, retired rows included.
- `publish_verified`: for `OccurrenceClass::RepositoryDocuments` only
  (`guarded`), after `live_lineage` (the domain and live-predecessor
  checks), reads `historical_head` and refuses a revision below 1 or at
  or below the head as `RevisionNotAdvanced`, then refuses
  `head != expected_head` as `HeadMismatch`. Any other class with an
  expected head is `InvalidInput`.
- `historical_head`: `MAX(source_revision)` over every descriptor
  observation of the lineage, with no liveness filter.
- `KernelStore::source_lineage_head` and `Envelope::source_lineage_head`
  expose the same query.

`crates/daemon/src/harness_sources.rs`: `Prepared::expected_head`
derives `revision - 1` for a document revision above 1, so the producer
proposes exactly the head plus one; `SourcePublisher::prepare` skips the
clock-lead check for the class.

`crates/daemon/src/source_capture.rs`: `CaptureJob::plan_document` reads
the head in the job's preview (`preview.source_lineage_head`) and
proposes `head + 1`; `live_documents` pages the project's live rows
narrowed on `project_id`. Document capture runs under the commit gate
after the window batch (`capture_documents`).

Document evidence is `canonical` retention (`harness_sources.rs` evidence
ingest: only `Messages` and `RawToolSpans` are `harness_capture`), so
expiry does not remove the rows `historical_head` reads.

## Failure scenario

A counter derived from live rows restarts at 1 after retirement, or an
expected head that ignores retirement lets a stale capture revive a
retired identity at a lower revision. Or two compose reads race and both
publish the same revision number with different bytes.

## Timing windows and dependencies

- Delete then recreate: the head stays at the retired revision, and the
  next publication is above it.
- Two producers reading one head: the second commit sees a moved head and
  refuses. At the daemon only capture publishes the class, under the
  gate; the PR records that the capture-layer moved-head refusal rests on
  the Kernel tests.
- Roll-forward: `docs/search-projection.md` states that U10's producer
  activates only after binaries older than projection schema version 8
  are retired.

## What a test must construct

A lineage published at 1, 2, 3, then retired at 3; proposals at 2, 3, 0,
and an advancing 4 with a stale head; a foreign domain; a fresh head read
and a publication at 4; a second path; two producers reading one head.
Through capture: edits, removal, recreation, and a second removal.

## Investigation log

### Q: Scope confirmation (OQ1)
- Sources examined: `ticket-1081.md` (Owner gate OQ1; Blocked By: none;
  status `ready-for-agent`); `spec.md` OQ1; PR #1094 body "Owner gate";
  `ticket-1089.md`.
- Findings: the class landed in U9, dark, and its producer in U10. The
  ticket and PR name OQ1 as an owner gate, and the ticket lists no
  blocker. This agent found no recorded product-owner confirmation in
  the ticket, PR, or progress files.
- Missing evidence: the product owner's recorded confirmation.
- Conclusion: unresolved, needs the product owner's recorded OQ1
  confirmation. The code landed on the ticket's unblocked status.

### Q: Guard scope across classes (OQ13)
- Sources examined: `ticket-1081.md`; `publish_verified`;
  `retirement_preserves_domain_ownership_without_reusing_old_revision_ids`.
- Findings: `RepositoryDocuments` only. Messages keep the baseline
  live-predecessor rule, and the existing test still records revision 2
  accepted after retiring 3 for messages.
- Missing evidence: none.
- Conclusion: resolved with answer (U9, `RepositoryDocuments` only).

### Q: Does the Kernel enforce contiguous revisions?
- Sources examined: `publish_verified`; PR #1094 "Recorded and left".
- Findings: the Kernel permits a gap above the head while
  `expected_head` matches. Contiguity is the producer's rule, which
  `Prepared::expected_head` enforces by deriving the head from the
  revision.
- Missing evidence: none.
- Conclusion: resolved with answer; the guarantee "strictly above every
  historical revision" holds in the Kernel, and contiguity is a producer
  property.
