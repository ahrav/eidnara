# RP2.1 property passes

System and external-source provenance are in [model.md](model.md).
Revision: `913234433ae36a80a6e22c6aac14c7f9aab74386`; date: 2026-09-10.
These passes precede synthesis. Repeated discovery is not independent evidence.

## Data integrity

RP2.1 lines 137-142 require one local transaction for three effects. Separate
row, checkpoint and job assertions would miss a mixed durable state. Compare
the entire affected state against a complete-commit oracle. Lines 84 and 156
also require occurrence identity independent of shared payload storage.

## Concurrency

Search commit must finish before kernel acknowledgement. Kernel acknowledgement
at `crates/kernel/src/outbox.rs:530-570` cannot inspect search durability.
Use independent snapshots and a transaction/lock event trace. Concurrent
source revision and late replay must not reactivate an older occurrence.

## Failure recovery

Crashes on both sides of COMMIT and lost acknowledgements require reopening,
not graceful shutdown. Pending rows are the durable obligation. This part
does not assert successful inference or selector recovery, which have other
owners. `outbox.rs:551-560` validates a kernel commit, not local application.

## Protocol contracts

N1 lines 156-169 and RP2.1 line 156 require a per-class durable inventory.
`codec/opencode.rs:490-559` maps a completed native part to call and result
blocks, so wire block count is not source occurrence count. Reuse existing
codec round-trip records; add only the projection-to-source contract.

## Resource boundaries

RP2.1 lines 118-121 name outbox batch, local transaction bytes and pending-job
count. `pending_outbox` at `outbox.rs:431-477` has a row limit, not a payload
byte limit. A multirow commit can exceed a batch (`:423-426`). Refusal must
retain work and expose a blocked state instead of skipping the large commit.
No numeric limit or fairness budget is inferred.

## Security boundaries

The shared authority is proposed in RP2.1 line 101 and index lines 40, 52.
The live `judge` remains private to daemon
(`kernel_routes/eligibility.rs:132-174`). Compare adapters over identical
canonical facts and keep authorization and checkout applicability distinct.
Stale projection policy fields cannot grant eligibility.

## Distributed coordination

Not applicable. This scope has one daemon and local stores, not replicas,
quorums or leader election. Local writer fencing and acknowledgement order
are covered by concurrency and recovery. No distributed claim is introduced.

## Lifecycle transitions

N1.3 hooks stay disabled before gates pass. Daemon route rejection is live at
`lib.rs:11805-11826,12558-12650`; the scheduler names only the existing
review-user-memories task (`dreamer_scheduler.rs:25-36`). The new gate contract
must cover startup and configuration changes, not just public route names.
Schema/model/policy mismatch rebuild is handed to the export/rebuild owner.

## Idempotency and replay

Kernel receipts (`envelope.rs:1043-1074`) protect canonical commit retries.
They do not deduplicate local rows or pending jobs. Replay comparison uses
per-occurrence state including revision, tombstone and required job identity.
Attempt totals and acknowledgement totals are not interchangeable with rows.

## Version compatibility

RP2.1 lines 41, 84 and 156 require explicit identity compatibility and vectors
valid for the current revision/model. A tokenizer or model transition can
leave lexical coverage full while valid dense coverage falls. The rebuild
owner handles mismatch recovery; this part checks reporting and stale-work
exclusion rather than re-specifying embedding admission.

## Overlap decisions before additions

The existing catalogs were inspected before new records were written. Exact
codec preservation, raw historian publication, canonical withheld-read
reporting and scheduler lease/receipt guarantees stay in their existing
records. The [overlap register](../existing-checks.md#overlap-register) links
them and records citation drift. Invalidated mirror records are historical
leads only. These discovery notes precede the central independent evaluation.
Its supplied findings and this pass's dispositions are retained in
[portfolio-evaluation.md](../portfolio-evaluation.md), not presented as another
independent review by the discovery author.
