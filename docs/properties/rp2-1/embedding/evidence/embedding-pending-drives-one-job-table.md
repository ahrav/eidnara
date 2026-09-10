# embedding-pending-drives-one-job-table

## Discovery trigger

P1 line 109 and P2 line 54 make durable Pending the crash source and the existing
process-local JobTable the admission/poll/result authority. They reject a
parallel routing or lease plane. This is a claim under test for proposed code.
Sources, date, and SHA: [source register](../catalog.md#source-register).
Reachability is test-only because no production RP2.1 pending driver exists.

## Evidence trail

- `crates/host-runtime/src/synapse/mod.rs:206-225` owns one JobTable, CPU
  semaphore, query admission set, tracker, and closing token per component.
- `crates/host-runtime/src/synapse/jobs.rs:162-196` stores jobs in memory.
- `jobs.rs:387-428` compares retained keys and payload digests under one lock;
  equal nonretryable retained work returns Existing.
- `jobs.rs:430-453` checks queue and result capacity before insertion.
- `jobs.rs:455-488` replaces a retryable failure only after admission succeeds.
- `mod.rs:795-838` spawns a worker only for Admitted, not Existing.
- `crates/host-runtime/src/synapse/protocol.rs:923-947` derives the canonical
  batch key from ordered hashes, IDs, model, epoch, and fingerprint.
- The [existing admission record](../../../host-runtime/catalog.md#synapse-admission-boundaries-are-exact)
  remains canonical for process-local capacities and retention.

## Failure scenario

A product scanner deletes a pending row when it submits a request. The host
crashes before the descriptor returns, losing the only remaining job state.
Another failure admits the same durable work through a second in-memory queue,
whose worker and result leases do not share JobTable's capacity accounting.
Durability does not require exactly one inference over all process lifetimes.
It requires preserving recoverable work and idempotent durable effects by K.

## Timing windows and dependencies

Projection creates Pending in its own atomic row/checkpoint transaction.
This record assumes that commit boundary is observable; it does not duplicate
the projection lane's atomicity or outbox-ack property.
Discovery can overlap submission or lose its response. Local deduplication
only helps while the key is retained in the same process incarnation.
Regrouping unchanged rows can alter the ordered batch key; the driver needs
an explicit reconstruction rule instead of assuming all scans yield one key.
Pending storage count/bytes and resident JobTable admission are separate bounds.

## What a test must construct

Commit Pending(K), independently retain its identity, and run two discovery
attempts while the first descriptor is suppressed. Observe JobTable outcomes
and worker starts without creating another execution queue in the harness.
Construct a full JobTable and verify refusal leaves durable work discoverable.
Include Existing and retryable-failure replacement as different legal outcomes.
Observe the persistent source before each new worker, and use one component
identity in the trace to detect a bypass or duplicate routing authority.
`rp21_embedding_pending_rediscovered_after_descriptor_loss` records the durable
row, attempted admission, suppressed response, and second discovery only.
Per-K reconciliation is the primary oracle; aggregate call counts are secondary.

## Investigation log

### Q: Does a second durable pending table duplicate JobTable?

- Sources examined: P1 lines 75, 109, 144-149; `jobs.rs:162-196`.
- Findings: The plan intentionally separates persistent work discovery from
  process-local scheduling and result leases. They have different lifetimes.
- Missing evidence: None for the intended ownership split.
- Conclusion: Resolved. Reuse JobTable; do not invent a second dispatch plane.

### Q: How does bounded discovery preserve admission identity?

- Sources examined: P1 lines 120-121; `protocol.rs:923-947`; `jobs.rs:387-489`.
- Findings: Current key derivation is ordered and retained-state dependent;
  pending schema, batch reconstruction, and durable caps are not implemented.
- Missing evidence: Projection scan API, grouping/replay rule, and approved L.
- Conclusion: Needs human input. No durable scan, lease, or key schema is added
  by this property catalog.
