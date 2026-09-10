# projection-bounded-admission-preserves-progress

## Discovery trigger

The resource lens finds local transaction bytes, outbox batch size and pending
job count in RP2.1's bounds. The recovery lens adds the dangerous response to
capacity: skipping a commit or dropping jobs while still acknowledging it.
This record covers local admission, not JobTable execution capacity.

## Evidence trail

Provenance: [source register](../_lenses/model.md#source-register), dated
2026-09-10, Eidnara HEAD `913234433ae36a80a6e22c6aac14c7f9aab74386`.
No allocation measurement, load test or capacity value is supplied here.

- [RP2.1 bounds][plan] name outbox batch, local transaction bytes and pending
  count, with production values unset and RP2.9 as the approval owner.
- [Index limits contract][index] requires approved limits before capacity;
  declared-but-unapproved values fail closed.
- [Pending outbox read][pending] caps row count but materializes payload bytes
  into `Vec<OutboxEntry>`. A caller-side limit is not a preallocation byte cap.
- [Boundary test][test] constructs a one-row batch ending in the middle of a
  two-row commit and checks that it is not a publication boundary.
- [Acknowledgement][ack] validates a committed sequence, not whether local
  capacity was available to apply that sequence.
- [Workspace][workspace] and named source searches show no local projection
  byte-admission or durable-pending capacity implementation.

Reachability: `test-only`. The kernel row cap is production code, but the
proposed local transaction and durable pending admission path does not exist.
No production request can exercise this record's combined capacity/progress
check before those components are implemented.

## Failure scenario

A pending table is full. The consumer stores the lexical row, drops its required
job and acknowledges the source commit. Alternatively, it increases the batch
size until one huge commit fits, exceeding the byte bound before refusing it.
Both paths can look correct in small row-count tests.

A competing explanation is a legitimate stalled consumer. Refusing a whole
commit is permitted when it cannot fit, provided progress does not advance and
the blocked state is explicit. This safety record does not promise a finite
recovery time without approved limits and a progress contract.

Another competing explanation is an unknown storage outcome. A failed COMMIT
response is not an admission refusal. The atomicity record permits either
complete prefix after reopen; this record's unchanged-state condition applies
to a refusal made before attempting to commit the rejected work.

## Timing windows and dependencies

Measure payload/batch cost before the allocation the limit is intended to bound.
Measure outstanding durable jobs at admission, including the jobs proposed by
the next complete commit. Historical completed rows need a separately defined
retention metric; do not silently equate them with outstanding capacity.
The export/source reader owner supplies pre-materialization limits where needed.
The embedding owner supplies completion/obsolescence to release pending capacity.
No process-local JobTable availability substitutes for durable work accounting.
Cross-store acknowledgement and lock-release order are checked solely by
[the ack owner][ack-owner], which consumes the durable local checkpoint.

## What a test must construct

1. Small approved test limits in declared units, independent of production values.
2. A pending set exactly at capacity, followed by a commit requiring more work.
3. A commit spanning more rows than one batch and a single payload exceeding
   the byte cap, with size observed before materialization.
4. A configuration containing values but no required approval evidence.
5. Explicit refusal observation and before/after rows, checkpoint and jobs for
   the rejected complete commit. Ack trace checking remains with its owner.
6. An accepted small commit to distinguish correct refusal from a dead adapter.
7. The pending-full and over-cap markers in [fault-map](../fault-map.md).

No test was run. Existing row-limit and boundary assertions remain unaudited.
The catalog does not prescribe a second queue, retry plane or arbitrary timeout.

## Investigation log

### Q: Which units, approval artifact and oversized-commit response apply?

- Sources examined: [bounds][plan], [index][index], [pending read][pending],
  [boundary test][test] and [acknowledgement][ack].
- Findings: Limits and approval ordering are contractual. Kernel outbox provides
  a row cap only and permits a batch without any complete commit. No local
  policy states how such a commit is blocked within byte/pending budgets.
- Missing evidence: Approved measurement units, approval identity, outstanding
  job accounting and an explicit oversized-complete-commit response.
- Conclusion: needs human input from projection, export, embedding and RP2.9
  owners. Do not invent capacity values or advance progress to avoid a stall.

[plan]: ../../../../../../commons/docs/plans/2026-09-10-feat-eidnara-rp2-1-projection-coverage-plan.md#L112-L121
[index]: ../../../../../../commons/docs/plans/2026-09-10-eidnara-rp2-plan-index.md#L43-L46
[pending]: ../../../../../crates/kernel/src/outbox.rs#L419-L477
[test]: ../../../../../crates/kernel/tests/kernel_outbox.rs#L622-L698
[ack]: ../../../../../crates/kernel/src/outbox.rs#L530-L570
[workspace]: ../../../../../Cargo.toml#L3-L17
[ack-owner]: ../../export-recovery/catalog.md#ack-follows-local-release
