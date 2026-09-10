# projection-commit-checkpoint-pending-atomic

## Discovery trigger

The state, concurrency and recovery lenses converge on one proposed local
transaction. RP2.1 lines 32, 40 and 137-142 promise rows, checkpoint and
durable pending jobs before canonical acknowledgement. This record owns only
the local rows/checkpoint/pending transaction. [The ack owner][ack-owner] alone
owns acknowledgement bounds and cross-store lock order. The failure target here
is split durable local state, not a second acknowledgement invariant.

## Evidence trail

Provenance: [source register](../_lenses/model.md#source-register), dated
2026-09-10, Eidnara HEAD `913234433ae36a80a6e22c6aac14c7f9aab74386`.
External sources are the supplied local plan, index, parent and N1 documents.
No incident, test run or crash result is part of the evidence.

- [RP2.1 U2][plan] identifies the local transaction and before/after crash
  falsifier. Line 116 requires releasing search before acquiring kernel writer.
- [Outbox record][entry] carries position, commit sequence, ordinal and a
  complete-commit flag. These are different identities, not interchangeable
  checkpoint counters.
- [Consumer acknowledgement][ack] acquires a fenced kernel writer, accepts
  repeated current checkpoints, rejects backward/nonexistent commits and
  updates the consumer. It cannot inspect another database's durability.
- [Pending read][pending] selects globally unpublished rows, with a row cap.
  A batch can end mid-commit. It is not a consumer-checkpoint query.
- [Workspace members][workspace] and the tracked tree contain no retrieval
  crate. Exact searches find no named search projection/checkpoint mechanism.
- [CAS process driver][crash] has a child barrier, parent wait, kill and reap.
  [Recovery comparisons][reopen] reopen actual storage after termination.
  These checks are unaudited reusable substrate, not RP2.1 product hooks.

Reachability: `test-only`. No production search transaction, pending table or
daemon projection consumer exists at this HEAD. The live kernel API is a
prerequisite, not a production path through the proposed local transaction.

## Failure scenario

A consumer writes rows and advances its checkpoint but loses pending jobs.
It then acknowledges the kernel sequence. Pruning can erase the only source
that would reconstruct the missing work. The equally important inverse is
jobs or rows becoming durable while the checkpoint implies another prefix.

A competing explanation is an unknown COMMIT response: the whole transaction
may have committed despite the caller seeing no success. The oracle must
accept either complete prefix after reopen, not insist on rollback. It must
reject every mixture. Compare affected rows, deletion facts and outstanding
work by identity, rather than just row counts.

## Timing windows and dependencies

Observe before COMMIT, after durable COMMIT, after a lost local COMMIT response
and after reopen. Observe search state in one snapshot so the checker does not
create a race. The [ack owner][ack-owner] supplies its postcommit/lost-response
witnesses for reuse, without another definition or lock-order check here.
The source must be paired with its incarnation and bootstrap baseline.
A newly registered consumer can start above zero (`outbox.rs:89-92`).
The export owner establishes that baseline; this part does not invent it.
Embedding completion/obsolescence is supplied by the embedding owner.
Required work cannot disappear merely because process-local dispatch occurred.

## What a test must construct

1. A complete source commit containing multiple occurrence mutations and at
   least one required embedding job, plus an earlier durable local baseline.
2. An independent prefix oracle, including a legitimate zero-row source commit.
3. Actual abrupt termination and reopen at each boundary, with the controller
   retaining source and boundary witnesses outside the killed process.
4. Lost local COMMIT responses and retry, without treating missing responses
   as proof of rollback. Compare against either permissible complete prefix.
5. A second local reader during commit so mixed local state is observable.
6. Per-identity comparison of rows, tombstones, checkpoint and outstanding work.
7. The local precommit kill marker in [fault-map](../fault-map.md), plus the
   canonical `rp21_ack_local_commit_interrupted` and `rp21_ack_response_lost`
   witnesses from [export/recovery][export-faults] where the episode uses them.

The product construction is proposed. CAS kill/barrier/reopen machinery exists;
search-specific local transaction hooks and its state oracle do not exist.
Physical power-loss durability needs separate storage-contract evidence.

## Investigation log

### Q: Which per-consumer reader includes already-published required rows?

- Sources examined: [pending read][pending], [acknowledgement][ack], RP2.1
  current-code map and U2.
- Findings: `pending_outbox` filters `published_at IS NULL`; acknowledgement
  tracks a separate consumer commit sequence. Neither is a consumer replay read.
- Missing evidence: The daemon adapter's chosen complete-prefix read contract.
- Conclusion: unresolved, owned by the export consumer's complete-prefix
  contract. It is not another obligation in this local atomicity check.

### Q: How are local baseline and source incarnation paired?

- Sources examined: `outbox.rs:89-92`, RP2.1 bootstrap and U2/U5 claims.
- Findings: Registration may inherit a retained-history floor; no search
  metadata format binds that floor to a kernel incarnation.
- Missing evidence: The bootstrap metadata and reset/reopen contract.
- Conclusion: needs human input from projection and export/rebuild owners.

[plan]: ../../../../../../commons/docs/plans/2026-09-10-feat-eidnara-rp2-1-projection-coverage-plan.md#L137-L142
[entry]: ../../../../../crates/kernel/src/outbox.rs#L28-L43
[ack]: ../../../../../crates/kernel/src/outbox.rs#L530-L570
[pending]: ../../../../../crates/kernel/src/outbox.rs#L419-L477
[workspace]: ../../../../../Cargo.toml#L3-L17
[ack-owner]: ../../export-recovery/catalog.md#rp21-ack-follows-local-release
[export-faults]: ../../export-recovery/fault-map.md
[crash]: ../../../../../crates/kernel/tests/cas_fault_injection.rs#L1046-L1094
[reopen]: ../../../../../crates/kernel/tests/cas_fault_injection.rs#L924-L990
