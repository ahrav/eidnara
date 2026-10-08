# cf-native-reopen-bytes

Repository: `/local/home/ahrav/scratch/eidnara`.
Inspected revision: `99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`.
Date: 2026-09-19. Exercise: not yet. Existing checks are unaudited.

## Discovery trigger

R4 and C6 require original native text/tool-output bytes after reopen. KTD4
keeps that internal witness separate from agent recovery. The
[settled contract](https://github.com/ahrav/eidnara/issues/707), "Requirements",
"Six acceptance cases", "Implementation Decisions" KTD4, and "Source and
evidence flow" (historical plan lines 49-50, 102, 156, 211), is the contract
lead, inspected against this revision rather than the original plan's older
`1555f00c` references. No external incident was supplied.

Persistence and integrity lenses found an existing high-level witness to
extend. This property does not create a source ingestion or retention contract.

## Evidence trail

- [Native adapters, lines 172-218](../../../../../crates/daemon/src/harness_sources.rs#L172)
  copy the selected string and retain native identity fields. Messages name
  project, harness, session, message, and block position. Tool output names
  parent message, tool call, result revision, and output block as well.
- [OpenCode selection, lines 254-301](../../../../../crates/daemon/src/harness_sources.rs#L254)
  uses native timestamps as revisions and selects settled output/error
  strings. Bytes mean the decoded string's UTF-8 encoding, not JSON syntax.
- [Publication, lines 641-647, 782-799](../../../../../crates/daemon/src/harness_sources.rs#L641)
  publishes a whole-block occurrence with no span and offers
  `unit.text.as_bytes()` to `ingest_exact_artifact` with the encoded revision.
  [Lines 662-670](../../../../../crates/daemon/src/harness_sources.rs#L662)
  key evidence by occurrence, revision, and representation. Successful
  succession does not enter the [retirement refusal path, lines 609-616](../../../../../crates/daemon/src/harness_sources.rs#L609).
- [Descriptor succession, lines 409-415](../../../../../crates/kernel/src/source_descriptor.rs#L409)
  calls [correct_observation_inner, lines 433-477](../../../../../crates/kernel/src/slice/write.rs#L433).
  It invalidates the predecessor in `observations` and its registry row.
  [invalidate, lines 90-118](../../../../../crates/kernel/src/object_write.rs#L90)
  updates only the named object and typed table, not `evidence_meta`.
- [Historical selection, lines 556-641](../../../../../crates/kernel/src/source_descriptor.rs#L556)
  accepts an explicit requested commit sequence and validates returned detail.
  [LiveAtEnd, lines 226-235](../../../../../crates/kernel/src/source_hold.rs#L226)
  includes an original descriptor invalidated after that requested sequence.
  [Native classes, lines 23-85](../../../../../crates/kernel/src/source_identity.rs#L23)
  include `Messages` and `RawToolSpans`; the selector has no narrower class
  gate. Tool `result_revision` is an identity field, unlike a message revision.
- [Source rows, lines 84-101](../../../../../crates/kernel/src/source_export.rs#L84)
  expose revision, descriptor detail, and optional text. The
  [materializer, lines 372-403](../../../../../crates/kernel/src/source_export.rs#L372)
  verifies the object, length, UTF-8 span, and selected payload identity.
  [Snapshot export, lines 301-305](../../../../../crates/kernel/src/source_export.rs#L301)
  uses the same `LiveAtEnd` predicate at the hold's pinned sequence.
- [Artifact read, lines 50-112](../../../../../crates/kernel/src/cas/read.rs#L50)
  checks live `evidence_meta`, not descriptor liveness, rejects tombstones,
  and verifies SHA-256. Descriptor succession preserves the original evidence's
  eligibility for this read; missing/corrupt objects and retirement/purge can
  still make the read fail.
- [Open, lines 283-287, 354-370](../../../../../crates/kernel/src/open.rs#L283)
  acquires a new lease; [lease acquisition, lines 686-743](../../../../../crates/lease/src/lib.rs#L686)
  increments its persisted epoch. [Hold loading, lines 1045-1078](../../../../../crates/kernel/src/source_hold.rs#L1045)
  rejects the old epoch/binding. [Reconciliation, lines 997-1035](../../../../../crates/kernel/src/source_hold.rs#L997)
  releases earlier-incarnation holds, not resumes their cursors. The minimal
  reopen witness uses historical descriptor selection without an old hold.
- [Daemon fixture, lines 52-53](../../../../../crates/daemon/tests/harness_sources.rs#L52)
  contains CRLF, leading/trailing whitespace, multibyte text, and a tab.
  [Lines 1419-1447](../../../../../crates/daemon/tests/harness_sources.rs#L1419)
  publish tool units, check exact text, drop the corpus, and compare reopened
  inventory. That is a clean reopen, not abrupt termination or power loss.
- [Inventory, lines 731-744](../../../../../crates/daemon/tests/harness_sources.rs#L731)
  compares class, text, and occurrence ID, mapping absent text to empty text.
  C6 must retain the complete binding and distinguish `None` from `Some("")`.

Reachability is `test-only`: the selected highest boundary is the daemon's
`#[test]` witness at lines 1282-1283, whose helper
[exports through kernel source holds, lines 277-322](../../../../../crates/daemon/tests/harness_sources.rs#L277).
This classification does not claim that every underlying kernel routine is
test-only or that source storage is reachable by the consumer.

## Failure scenario

A fixture publishes an original command and later an equal-byte-length
revision. After reopen it reads the latest live source, compares only a
length or normalized rendering, and credits the original evidence. Another
failure returns the right descriptor with altered whitespace or Unicode.
Both can preserve readable meaning while violating the exact-source request.

An unavailable original is a separate outcome, not a successful byte check.
It remains a failed positive C6 row, not a reason to change its expected
target to the successor or a distinct occurrence. Refusal controls belong to
[cf-unavailable-evidence-no-credit](cf-unavailable-evidence-no-credit.md).

## Timing windows and dependencies

Publish the original, save its descriptor commit sequence and binding, publish
the same-length successor, then close and reopen the same store. Select the
original with `live_source_descriptors` at the saved sequence and read its
evidence handle. The selector uses no lease-bound hold. Current-tip inventory
cannot replace it. This ordering is code-supported, not executed here.

Use `Messages` for same-lineage succession. `RawToolSpans` is also selectable,
but changing native `result_revision` changes its identity and lineage. Keep
that distinct-occurrence control separate. No purge, evidence retirement,
crash, or power-loss fault belongs in the positive row.

## What a test must construct

1. Independently annotate native identities and original UTF-8 bytes. Include
   a command and exact numeric value in addition to the existing byte shapes.
2. After original descriptor publication, save the commit sequence and full
   binding. Publish a same-length message successor before closing the store.
3. Reopen, then page `live_source_descriptors(class, saved_sequence, ...)`
   within a fixed budget until the original binding is matched. Compare all
   native fields; neither the first row nor a matching digest selects it.
4. Build the handle from that descriptor's evidence ID/digest and call
   `read_artifact`. A successful UTF-8 decode supplies `Some(text)` to the
   witness; require the annotated span, payload identity, and bytes to match.
   `None` is not `Some("")`. Do not read the CAS file directly or reuse a hold.
5. Keep both publications before reopen and keep the original as the oracle.
   Refusal/purge outcomes earn no credit; no new API or consumer capability
   follows from this privileged witness.

## Investigation log

### Q: How does C6 select the older revision after succession?

Initial investigation, superseded by the verified follow-up below:

- Sources examined: native publication, source export, artifact read, and the
  daemon integration witness cited above.
- Findings: the fixture's fresh inventory selects live descriptors. The
  export row has revision and evidence/digest detail that its inventory tuple
  does not retain separately. The direct artifact reader accepts a live
  evidence/digest pair, not an arbitrary native-revision query.
- Missing evidence: a C6 fixture that retains and revalidates the intended
  association through a pinned snapshot/source hold after publishing the
  equal-length successor and reopening. The main investigation owns this
  path; its result is pending here.
- Conclusion: unresolved, needs that path's evidence and fixture-level
  observation. A claim that no old-revision read is possible cannot be
  established from `read_artifact` alone. Keep the original C6 target; do not
  substitute a successor/distinct occurrence or broaden an API to pass.

### Q: Does succession make original-revision recovery impossible?

- Sources examined: descriptor correction, typed invalidation, publication
  keys/refusal paths, historical selection, artifact reads, and epoch checks
  cited above. The supplied refute-first finding was checked against each.
- Findings: the impossibility claim is rejected. It conflates invalidated
  descriptor observations with still-live evidence metadata. The saved
  original sequence selects that descriptor after both publications and
  reopen; its evidence handle remains subject to the normal read guards.
  Reopen invalidates old holds, not this historical selector.
- Missing evidence: the combined C6 execution with same-length succession,
  bounded historical selection, and exact original bytes after reopen.
- Conclusion: selector constructibility is code-supported; the joined
  execution remains unexercised. No successor substitution or API expansion
  is needed. Explicit evidence retirement or purge can still refuse the read.
