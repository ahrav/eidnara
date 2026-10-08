# cf-exact-source-binding

Repository: `/local/home/ahrav/scratch/eidnara`.
Inspected revision: `99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`.
Date: 2026-09-19. Exercise: not yet. Existing checks are unaudited.

## Discovery trigger

The [settled contract](https://github.com/ahrav/eidnara/issues/707),
"Requirements" R3/R4, "Six acceptance cases" C6, and "Source and evidence flow"
(historical plan lines 49-50, 102, 211),
rejects wrong native occurrences, wrong revisions, and normalized substitutes
as exact recovery. Integrity, protocol, and wildcard passes identify the
join between a requested source and an otherwise valid artifact. No incident
or executed wrong-source result is claimed.

## Evidence trail

- [SourceUnit, lines 78-87](../../../../../crates/daemon/src/harness_sources.rs#L78)
  separates class, ordered native identity, revision, representation, and text.
  [Message/tool construction, lines 172-218](../../../../../crates/daemon/src/harness_sources.rs#L172)
  preserves native block position and tool-call/result identity.
- [Adapter revision contract, lines 228-230](../../../../../crates/daemon/src/harness_sources.rs#L228)
  distinguishes native timestamps from a separate edit revision. Changing
  bytes at the same identity/timestamp conflicts at publication; C6 must use
  actual distinct accepted revisions for its revision-selection control.
- [Publication key, lines 641-670](../../../../../crates/daemon/src/harness_sources.rs#L641)
  binds occurrence, revision, and representation. A digest does not replace
  these fields. [Lines 531-546 and 782-799](../../../../../crates/daemon/src/harness_sources.rs#L531)
  distinguish descriptor replay from exact artifact ingestion.
- [Daemon checks, lines 1283-1335](../../../../../crates/daemon/tests/harness_sources.rs#L1283)
  keep equal text under three native identities distinct and reject changed
  bytes at a reused identity/revision. The changed text there is not the C6
  same-length revision control.
- [Succession, lines 1359-1387](../../../../../crates/daemon/tests/harness_sources.rs#L1359)
  checks a newer revision and stale-revision refusal. These are mechanism
  checks, not a consumer's original-evidence selection oracle.
- [Export preflight, lines 535-562](../../../../../crates/kernel/src/source_export.rs#L535)
  checks descriptor version, class, revision, evidence/digest, and reencoded
  identity. [Materialization, lines 372-403](../../../../../crates/kernel/src/source_export.rs#L372)
  verifies bytes and spans. A valid row for another requested source can
  satisfy those checks and still be the wrong answer to C6.
- [Chunk assembly, lines 256-274](../../../../../crates/daemon/src/history_summarizer_chunk.rs#L256)
  joins and compacts parts and records whether presentation is transformed.
  [Text extraction, lines 1187-1204](../../../../../crates/daemon/src/history_summarizer_chunk.rs#L1187)
  cleans or trims text and normalizes it. The
  [publication request, lines 712-723](../../../../../crates/daemon/src/history_summarizer.rs#L712)
  stores the supplied chunk transcript, not a native-source certificate.

Reachability is `test-only`: the property is the C6 comparison at the native
daemon witness/replay boundary, not a production source-expansion operation.
The plan's U2 witness is a requested extension. This inspection does not
establish an implemented C6 comparison or exercise it.

## Failure scenario

There are three distinct negative controls. A newer revision changes a
numeric value without changing UTF-8 byte length. A different native message
contains identical bytes. A normalized transcript preserves apparent meaning
but changes spacing, block boundaries, or text. None receives exact-source
credit, even if a digest is valid or a snippet looks sufficient.

The competing explanation is storage corruption. It is excluded from these
controls by using independently valid alternate artifacts. This isolates
selection/binding errors from the CAS reader's corruption checks.

## Timing windows and dependencies

Select the original before presenting alternative evidence. Reopen may sit
between selection and comparison, but a process crash is not required.
The [native witness](cf-native-reopen-bytes.md) retains the original commit
sequence, publishes the successor before reopen, then selects the historical
descriptor and reads its original evidence handle without reusing a hold.
Use whole native blocks for the existing publisher; any annotated selected
span is a validated UTF-8 byte range within those retained bytes, not a
character count copied from normalized presentation.

## What a test must construct

1. Freeze expected native occurrence, revision, representation, and byte span
   outside generated summaries and citation aliases.
2. Confirm two different revisions have equal byte lengths, not merely equal
   string lengths. Include two different occurrences sharing identical text.
3. Supply valid alternate rows/handles and normalized/full-summary candidates
   to the observation comparison, preserving their real provenance.
4. Require no exact-source credit for every mismatch, and credit for a valid
   independently matched original-native control with `text = Some(text)`.
   Do not label alternatives corrupt or substitute a successor to pass C6.
5. Compare binding and bytes independently. Do not allow `match=exact` from
   memory search or an artifact SHA alone to satisfy the binding check.

## Investigation log

### Q: Where does the independent expected binding live?

- Sources examined: the supplied plan's KTD1/KTD4 and the source/summary code
  cited above.
- Findings: source records expose the relevant native fields, while summary
  assembly has transformed presentation. The plan assigns corpus identity
  validation to Rust and separates annotations from candidate outputs.
- Missing evidence: the C6 corpus and its replay observation representation.
  The plan's `compression-fidelity.json` and `compression_fidelity_tests.rs`
  targets are absent in the inspected checkout.
- Conclusion: unresolved, needs the fixture-level representation selected by
  that implementation owner. Reusing a production normalizer to manufacture
  expected bytes would make the oracle circular and is not an acceptable fix.

### Q: Must the expected identity change because the original was superseded?

- Sources examined: the verified selector and succession chain in the
  [native witness investigation](cf-native-reopen-bytes.md#investigation-log).
- Findings: no. Descriptor invalidation does not invalidate its evidence
  metadata. The supplied original-revision impossibility claim was refuted
  against code, so changing the oracle would conceal a selection failure.
- Missing evidence: the combined same-length/reopen substitution execution.
- Conclusion: keep the original binding; constructibility is supported but
  exactness and negative-control execution remain unexercised.
