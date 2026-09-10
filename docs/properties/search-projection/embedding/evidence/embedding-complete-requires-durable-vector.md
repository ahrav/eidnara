# embedding-complete-requires-durable-vector

## Discovery trigger

P1 U3 line 149 states: persist vectors before marking the job complete.
That product completion is distinct from the existing process-local Ready
state. The guarantee is cataloged as a claim under test.
Sources, date, and SHA: [source register](../catalog.md#source-register).
Reachability is test-only because no production RP2.1 vector/completion
persistence path exists at the pinned revision.

## Evidence trail

- `crates/host-runtime/src/synapse/jobs.rs:91-106` defines Ready as vectors,
  page boundaries, and a ResultLease. It contains no persistent store handle.
- `jobs.rs:502-557` converts vectors to Arc-backed memory and sets Ready.
- `jobs.rs:623-655` serves pages from that resident allocation.
- `crates/host-runtime/src/synapse/mod.rs:873-899` validates native output and
  publishes it to JobTable. It does not commit a product vector.
- `mod.rs:1148-1155` clears local jobs after shutdown drains tracked work.
- P1 lines 108-110 separate Pending, Embedded, and Published in the intended
  product flow. U2 owns the pending row transaction; U3 owns vector completion.
- The existing host admission/result-lease record is reused for memory
  custody. It is not promoted into a disk durability guarantee.
- `crates/kernel/tests/cas_fault_injection.rs:1046-1094` supplies an existing
  named child barrier and parent kill/join pattern; `:924-989` reopens and
  compares recovered CAS state. This reusable pattern is unaudited here.
- `cas_fault_injection.rs:1-6` gates the suite on `test-support` and explicitly
  limits its evidence to process crash/injected errors, not power loss.

## Failure scenario

The driver receives a ready result and writes Complete(K) before the vector
commit is durable. A crash loses the vector, but recovery sees completion and
skips backfill. The row appears covered while retrieval has no valid vector.
Another variant treats an unknown vector-commit outcome as success and marks
complete without reconciling storage. A successful poll is equally insufficient.
The reverse residue, durable vector without completion, is legal if retry
reconciles by K rather than creating duplicate or cross-identity effects.

## Timing windows and dependencies

Observe inference return, vector write, vector commit, completion write, and
completion commit as separate boundaries until storage design proves otherwise.
One atomic transaction may collapse the two commit boundaries; that is an
implementation choice, not an assumption this evidence imports.
If vectors live elsewhere, the storage owner must define ordering and recovery
across the two stores. The catalog does not claim cross-database atomicity.
Current identity must still be checked at completion under the fencing record.

## What a test must construct

Start with durable Pending(K) and a known validated vector. Retain an external
trace of attempted and confirmed commits. Kill the process at each supported
persistence boundary, reopen without clean shutdown, and read V(K) and Complete(K).
Require exact identity and vector-byte agreement whenever Complete(K) survives.
Inject failed and lost vector-commit responses; reconcile the actual reopened
state instead of assuming what the error means. Include vector-only residue.
`search_projection_embedding_process_killed_at_persistence_boundary` names the independent
crash situation. It never requires a missing vector or surviving false marker.
The storage contract must distinguish process death from power loss before
`/testing:crash-consistency-and-failpoint-testing` chooses physical fault tools.
Reuse the kernel CAS barrier/kill/reopen pattern after vector storage and
commit hooks exist. Its suite-local functions are not a generic exported
embedding API, and its CAS oracle does not check product vector completion.
The handoff needs narrow product hooks and a per-K oracle, not a broad new
crash harness or an assumption that kernel CAS checks cover this path.

## Investigation log

### Q: Can Ready or a retained result page establish product durability?

- Sources examined: `jobs.rs:91-106`, `:502-557`, `:623-655`; P1 line 149.
- Findings: Ready and its lease own resident vectors only. They can disappear
  with the process even when a caller observed the result.
- Missing evidence: None for the difference between these two boundaries.
- Conclusion: Resolved. The oracle must read durable product state after reopen.

### Q: What storage protocol makes completion durable?

- Sources examined: P1 U2/U3, Synapse, and the kernel CAS process-crash pattern.
- Findings: Pending belongs in search.sqlite, but vector storage/completion
  operations and their durability mode do not exist in the current tree.
  Existing child termination and reopen mechanics are reusable, not absent.
- Missing evidence: Transaction boundary, write-failure semantics, and declared
  process-crash versus power-loss contract.
- Conclusion: Needs human input. No fsync order or SQLite mode is invented here.
