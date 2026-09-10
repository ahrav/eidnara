# projection-replay-does-not-resurrect

## Discovery trigger

RP2.1 lines 110 and 142 require replay after local commit and before canonical
acknowledgement. The idempotency lens adds the discriminating history:
apply a revision or deletion, then retry an older already-applied prefix.
Repeating an insert immediately is insufficient to expose resurrection.

## Evidence trail

Provenance: [source register](../_lenses/model.md#source-register), dated
2026-09-10, Eidnara HEAD `913234433ae36a80a6e22c6aac14c7f9aab74386`.
No incidents or executed tests establish a replay defect or its absence.

- [RP2.1 U2][plan] names rows, tombstones, checkpoint and pending jobs in the
  uninterrupted-versus-replayed comparison, with no duplicate job.
- [Kernel receipts][receipt] compare producer/operation identity and request
  digest. They return stored canonical commit results, not local effects.
- [Consumer acknowledgement][ack] is monotone and accepts the current sequence
  again. It does not prevent a local adapter from applying stale payloads.
- [Existing mirror replay record][mirror] is invalidated. Its original module
  and tests do not exist at this HEAD. Similar wording cannot establish local
  search replay coverage.
- [Workspace][workspace] has no retrieval crate, and the named local occurrence
  and pending mechanisms are absent from Rust source searches.

Reachability: `test-only`. No production path applies a source prefix to search
rows or deduplicates its pending jobs. A kernel commit retry reaches an existing
receipt path, which is a different subject and not this record's exercise.

## Failure scenario

Revision 1 creates an occurrence. Revision 2 replaces it, and a later event
deletes the source. A retry of the first prefix upserts its old live flag or
re-enqueues work against revision 1. Search then exposes evidence the canonical
source has removed, or backfill spends work on a revived obsolete identity.

A competing explanation is legitimate retained history: keeping old rows for
audit is not resurrection. The check compares active/current status, tombstone
meaning and outstanding job identities, not the mere existence of a historical
row. Physical compaction layout is outside the equality relation.

The forbidden difference is relative to single application of the same complete
source prefix. Arbitrarily permuting new canonical commits is not required.
Only duplicate and overlapping delivery of already-applied history is added.
Authorized canonical remediation is not divergent replay: it can change mapped
bytes without a revision increment. The [remediation record][remediation] owns
that conditional case and its fresh current-input comparison.

## Timing windows and dependencies

Replay can follow lost local response, lost kernel acknowledgement, process
restart or retry of an overlapping batch. It must not lower local progress.
The test retains source order and source incarnation explicitly.
Occurrence/payload separation is a dependency: another live occurrence may
legitimately share the deleted occurrence's payload bytes.
Embedding execution is outside this record; durable work identity is inside it.
Completion and obsolescence facts must come from the embedding owner's contract.
One response does not imply one row, since revisions replace and commits fan out.

## What a test must construct

1. A source lineage with insert, revise and delete, all in complete commits.
2. Another live lineage sharing payload bytes with the deleted lineage.
3. Identical replay before and after reopen, then an old-prefix replay after
   the deletion has committed.
4. Overlapping batches that include both already-applied and new complete
   commits, preserving the canonical order of the new portion.
5. Per-occurrence current state and tombstone comparison against a fixture-owned
   model, plus one active row per current tuple and at most one outstanding row
   per required job key. A set comparison alone would hide duplicate rows.
6. Unauthorized divergent replay under a repeated mutation identity, kept
   distinct from an authorized canonical remediation event under an unchanged
   occurrence tuple. The latter must not be rejected solely as duplicate input.
7. Independent `old_prefix_after_delete` and `duplicate_after_reopen` markers
   from [fault-map](../fault-map.md), fired by input history, not resurrection.

No test was run. Existing canonical receipt checks remain unaudited and are
reusable prerequisites, not evidence that local idempotency is implemented.

## Investigation log

### Q: What key distinguishes divergent replay from authorized byte changes?

- Sources examined: RP2.1 KTD4 and U2, [kernel receipts][receipt], and the
  invalidated [mirror conflict record][conflict].
- Findings: The plan requires deterministic occurrence identity and duplicate
  suppression, but specifies no implemented mutation key, unique constraint or
  response for divergent content under a repeated identity.
- Missing evidence: Local schema, conflict policy and outstanding job-key
  definition agreed with the embedding owner, including canonical remediation
  and current-input hash changes.
- Conclusion: needs human input. Canonical receipt keys cannot be assumed to
  substitute for a local projection key without an explicit mapping.

[plan]: ../../../../../../commons/docs/plans/2026-09-10-feat-eidnara-rp2-1-projection-coverage-plan.md#L137-L142
[receipt]: ../../../../../crates/kernel/src/envelope.rs#L1043-L1074
[ack]: ../../../../../crates/kernel/src/outbox.rs#L530-L570
[workspace]: ../../../../../Cargo.toml#L3-L17
[mirror]: ../../../memory-store/catalog.md#mirror-receipt-replay-applies-effects-once
[conflict]: ../../../memory-store/catalog.md#mirror-receipt-conflict-rejects-divergent-replay
[remediation]: ../catalog.md#projection-remediation-invalidates-derived-bytes
