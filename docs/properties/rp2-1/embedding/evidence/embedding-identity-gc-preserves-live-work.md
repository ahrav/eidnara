# embedding-identity-gc-preserves-live-work

## Discovery trigger

P1 lines 39 and 146 assign embedding identity GC to RP2.1. Its interaction with
delayed completion is distinct from local result retention and merits its own
claim under test. P2 preserves occurrence identity even when payloads deduplicate.
Sources, date, and SHA: [source register](../catalog.md#source-register).
Reachability is test-only because no production RP2.1 identity GC path exists.

## Evidence trail

- `crates/host-runtime/src/synapse/jobs.rs:62-88` keeps result bytes charged
  while either the table or a served page holds a ResultLease.
- `jobs.rs:91-106` stores that lease only on the process-local Ready state.
- `jobs.rs:156-158` ranks retention using last poll time or completion time.
- `jobs.rs:748-886` sweeps and removes local jobs, not durable model identities.
- `jobs.rs:1382` defines the page-leased-result retention test. Its oracle is
  in-memory resource custody, not whether current product vectors may be deleted.
- `crates/daemon/src/dreamer_scheduler.rs:334-371` schedules only the existing
  review-user-memories task; no embedding GC sweep is registered there.
- P1 line 109 makes identity mismatch Obsolete. P1 KTD4 and P2 lines 37-38
  prevent payload deduplication from collapsing distinct occurrences.

## Failure scenario

A sweep selects an old model identity for cleanup. Before deletion, a worker
acquires a reference or a current occurrence shares its payload storage.
Deleting by stale scan results erases live data or retryable work.
Alternatively GC removes an obsolete identity, then a delayed result recreates
it and advertises it as current. Completion fencing must still apply after GC.
No conservation assertion over total vector count detects all these cases:
one wrong deletion and one stale insertion can cancel numerically.

## Timing windows and dependencies

Selection and deletion are separate observations. The live-reference and
obsolete predicates must be current at deletion, under the chosen storage
serialization boundary, rather than inferred from a prior sweep snapshot.
The reference set may include pending work, running inference, staged durable
vectors, served pages, or generation readers. Its actual members remain an
owner decision; this catalog does not invent a pin or lease subsystem.
Backfill/GC work must also obey the shared supervisor slice limits.
Physical reclamation and semantic eligibility are different obligations.

## What a test must construct

Create old and current identities for one source and two distinct occurrences
sharing payload bytes. Select an obsolete GC candidate, then hold a native
result or page while racing the delete phase with dispatch or identity update.
Verify current pending/vector state survives and old completion cannot become
current after deletion. Include an independently obsolete, unreferenced control
so a no-op collector cannot masquerade as meaningful GC coverage.
`rp21_embedding_gc_candidate_has_concurrent_holder` witnesses the race premise.
`rp21_embedding_gc_obsolete_identity_is_unreferenced` witnesses cleanup scope.
Read per-identity durable state and physical holder counts independently of the
collector's selected list; that list is not the oracle for safe deletion.

## Investigation log

### Q: Is existing JobTable sweep the identity GC required by the plan?

- Sources examined: `jobs.rs:62-106`, `:748-886`; P1 lines 39 and 146.
- Findings: Sweep retires ephemeral results under local capacity/retention.
  It cannot decide whether a durable occurrence/model identity is obsolete.
- Missing evidence: None for the distinction between these two GC domains.
- Conclusion: Resolved. Reuse local lifetime rules without cloning that GC.

### Q: Which references and retention policy govern durable deletion?

- Sources examined: P1 U3, P2 identity contract, scheduler task registration.
- Findings: Identity GC ownership is settled; durable reference tracking,
  grace policy, and bounded deletion API are not implemented.
- Missing evidence: Authoritative live-reference set and RP2.9-approved sweep
  and retention limits, including how a selected candidate is revalidated.
- Conclusion: Needs human input. No retention interval or lease plane is added.
