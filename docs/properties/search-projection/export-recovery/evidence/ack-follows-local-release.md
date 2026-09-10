# ack-follows-local-release

Repository: `/local/home/ahrav/scratch/eidnara`.
HEAD: `913234433ae36a80a6e22c6aac14c7f9aab74386`. Date: 2026-09-10.
User-supplied scope: plan, linked parent/index/research, and local repo.
No incident logs or runtime evidence are supplied. Source aliases resolve in
[catalog sources](../catalog.md#sources).

## Discovery trigger

The concurrency pass follows resource ownership across two local databases.
P, line 116, requires local COMMIT/release before acquiring kernel writer for
ack and forbids holding both transactions. P U2, line 142, requires crash
boundaries around both commits and ack not exceeding durable local checkpoint.
The row/checkpoint/job atomicity property belongs to the projection-row owner.
This part exclusively owns ack-after-local-durability, lock ordering, and
ack-loss observations. Projection references the canonical definitions in
[fault-map.md](../fault-map.md#independent-situation-markers).

## Evidence trail

- `crates/kernel/src/outbox.rs:530-541` takes the kernel writer mutex and
  starts a fenced write transaction for each acknowledgement.
- `crates/kernel/src/outbox.rs:551-570` validates monotonic existing sequence,
  writes the checkpoint, completes barriers, and commits locally to kernel.
- `crates/kernel/src/open.rs:410-418` uses blocking writer acquisition.
  `crates/kernel/src/open.rs:421-427` has a separate bounded internal helper;
  ack does not call that helper.
- `crates/kernel/src/retention.rs:203-211` explicitly commits and drops its
  writer before artifact GC acquires it again. This is a reuse precedent for
  ownership discipline, not evidence of the proposed search/kernel composition.
- `crates/kernel/tests/kernel_outbox.rs:86-153` checks repeated/monotonic ack
  and existing sequence admission, without any local search database.
- R, lines 9 and 35, supplies the retained two-WAL-database warning and the
  single-local-transaction/replay proposal. It is not local crash-test proof.
- Daemon source has no search COMMIT or outbox ack call at HEAD. The composite
  record is `test-only` because the production behavior under test is absent.
- `crates/kernel/tests/cas_fault_injection.rs:1046-1094` supplies the existing
  child/barrier/kill/reap pattern. Status: unaudited. Concrete search local
  COMMIT/ack hooks and reopen assertions remain missing; no broad new harness
  is needed merely to terminate a child at a witnessed boundary.

## Failure scenario

A daemon sends ack while the search transaction still owns its connection
guard. A kernel operation can then wait on search-owned work while search
waits on the kernel writer, or canonical writes can stall behind that ordering.
Only checking that ack eventually returns cannot detect prohibited overlap.

If ack precedes local durability, outbox pruning can discard the sole replay
source for local work lost on restart. If ack COMMIT succeeds but its response
is lost, retry is valid; requiring a one-to-one ack-response/effect count is wrong.

## Timing windows and dependencies

Record local transaction begin/COMMIT/rollback/release, kernel writer attempt,
kernel COMMIT, and caller-observed response as distinct events. Compare c to
the matching consumer/candidate's durable prefix, not another generation's.
An old checkpoint after deliberate projection deletion is not evidence that
the new candidate contains those rows; the rebuild path must establish that.
No physical cross-database atomicity is assumed.

## What a test must construct

1. Apply a complete local batch using the row owner's real transaction.
2. Hold the kernel writer in another scheduled operation before ack attempt.
3. Observe that search transaction and batch-owned locks are already released.
4. Observe successful local COMMIT, terminate before kernel ack is attempted,
   then reopen and replay using the existing process-control pattern.
5. Lose an ack response after kernel COMMIT; reconcile the stored checkpoint
   and retry without repeating logical work.
6. Inject local commit failure and require no corresponding advancing ack.
7. Reference `search_projection_ack_local_commit_interrupted`, `search_projection_ack_writer_contended`,
   and `search_projection_ack_response_lost` from commit/fault events, not forbidden overlap.
   Their single definition site is the fault map; no projection-side alias
   or duplicate definition is required.

## Investigation log

### Q: Can kernel acknowledgement enforce the local-durability half itself?

- Sources examined: Entire ack implementation and its checkpoint test.
- Findings: Ack sees kernel consumer state and commit log only. It has no
  search transaction, local checkpoint, or row/job durability evidence.
- Missing evidence: Daemon orchestration and two-store fault observations.
- Conclusion: Resolved with answer: kernel monotonicity is necessary substrate,
  not proof of local durability or release order.

### Q: Where can a deterministic observer see local release before acquisition?

- Sources examined: P lock rule, kernel connection guards, retention precedent.
- Findings: Kernel owns its acquisition internally; no search connection owner
  exists yet. A callback recording only ack return would miss the window.
- Missing evidence: The production transaction-owner seam and a faithful event
  observer that does not introduce another queue/coordinator.
- Conclusion: Needs human input from the daemon/projection implementer. Route
  seam choice to test strategy; do not create a helper during discovery.
