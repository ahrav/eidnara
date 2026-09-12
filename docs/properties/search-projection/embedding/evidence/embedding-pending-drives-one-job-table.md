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
- `crates/retrieval/src/dispatch.rs:174-181` derives the host item identity from
  the current episode or the first episode that admission opens.
- `charge_admission` at `crates/retrieval/src/dispatch.rs:414-469` compares that
  submitted identity with the ledger episode before either charging or returning
  `AlreadyCharged`. `EpisodeChanged` leaves the row unchanged.
- `EmbeddingDispatcher::admit` at
  `crates/daemon/src/embedding_dispatch.rs:576-664` computes one item identity,
  passes it to host submission and `charge_admission`, and defers
  `EpisodeChanged` without a disposition.
- The [existing admission record](../../../host-runtime/catalog.md#synapse-admission-boundaries-are-exact)
  remains canonical for process-local capacities and retention.

## Failure scenario

A product scanner deletes a pending row when it submits a request. The host
crashes before the descriptor returns, losing the only remaining job state.
Another failure admits the same durable work through a second in-memory queue,
whose worker and result leases do not share JobTable's capacity accounting.
In the episode race, host work is submitted for A, recovery opens B before the
charge transaction, and an unfenced charge consumes one of B's attempts while
recording A's host job on B.
Durability does not require exactly one inference over all process lifetimes.
It requires preserving recoverable work and idempotent durable effects by K.

## Timing windows and dependencies

Projection creates Pending in its own atomic row/checkpoint transaction.
This record assumes that commit boundary is observable; it does not duplicate
the projection lane's atomicity or outbox-ack property.
Discovery can overlap submission or lose its response. Local deduplication
only helps while the key is retained in the same process incarnation.
Recovery can also replace the episode after submission but before charge. The
charge transaction serializes the ledger read and state-predicated update, so
the submitted item identity must match the episode read in that transaction.
Regrouping unchanged rows can alter the ordered batch key; the driver needs
an explicit reconstruction rule instead of assuming all scans yield one key.
Pending storage count/bytes and resident JobTable admission are separate bounds.

## What a test must construct

Commit Pending(K), independently retain its identity, and run two discovery
attempts while the first descriptor is suppressed. Observe JobTable outcomes
and worker starts without creating another execution queue in the harness.
Construct a full JobTable and verify refusal leaves durable work discoverable.
Include Existing and retryable-failure replacement as different legal outcomes.
Submit host work for episode A, open recovery episode B before charge, and assert
that A receives `EpisodeChanged` while B remains pending with zero attempts and
no host job identity.
Observe the persistent source before each new worker, and use one component
identity in the trace to detect a bypass or duplicate routing authority.
`search_projection_embedding_pending_rediscovered_after_descriptor_loss` records the durable
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

- Sources examined: P1 lines 120-121; `protocol.rs:923-947`;
  `dispatch.rs:174-181`, `:414-469`; `embedding_dispatch.rs:576-664`.
- Findings: The durable episode is the host item identity. One value reaches
  host submission and the serialized charge transaction. A changed episode
  defers without consuming the replacement grant.
- Missing evidence: Approved durable and resident capacity limits, the
  submission-before-charge crash window, and a full production acceptance trace.
- Conclusion: Resolved for episode identity. Capacity approval and full
  acceptance evidence remain open. The post-charge crash/reopen window is
  covered by `crash_after_charge_reopens_state_and_accounting_together`.
