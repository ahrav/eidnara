# Search projection lifecycle

The daemon owns one search projection per data home. `SearchLifecycleOwner`
(`crates/daemon/src/search_lifecycle_owner.rs`) holds the admission owner, the
selection manager, and the running identity. It advances one durable lifecycle
record a bounded slice at a time. The kernel stays the canonical authority; the
projection is a rebuildable derived family.

## Records

The lifecycle record lives outside the disposable search database, under the
data home (`crates/daemon/src/projection_lifecycle.rs`). It is in one of these
states:

| State | Meaning |
|---|---|
| Absent | No projection exists. Installed admission records let the slice loop record the first build (`Cause::Registration`). |
| Intent | A rebuild or authorized recovery is in progress. The record fixes the cause, attempt identity, consumer binding, allowance, and deadline. A restart resumes it and renews nothing. |
| Current | The selected family reached its fixed target. Later slices judge it on its coverage and catch it up toward the kernel tip. |
| Disabled | Admission is closed. Only an authorized recovery reopens the record. |
| Unavailable | The record cannot be trusted. Every hook is denied and nothing is released. |

## Slices

`run_slices` runs one claim-source slice and then one lifecycle slice per turn.
Each lifecycle slice:

1. Reads the admission records and derives the running `ProjectionIdentity`
   from the ready embeddings lane, the manifest's limit protocol, and the kernel
   incarnation.
2. Refreshes admission from what the daemon has opened.
3. Advances the record: builds or selects a replacement for an Intent, or for a
   Current record reopens the selected family once and then runs one catch-up
   episode toward the tip under the family's source hold.

A slice ends within the manifest's `supervisor_slice_ms`, within the caller's
budget, and for an Intent within the record's own deadline.

## Rebuilds of a Current family

A slice over a Current record reports `SliceOutcome::Rebuild(cause)` when the
selected family needs a replacement:

| Cause | Condition |
|---|---|
| `CatchUpHoldLost` | The catch-up episode cannot extend the family's source hold: the hold is bound to an earlier kernel lease (`SourceHoldError::BindingMismatch`), or it is missing, released, expired, purge-degraded, or missing bytes (`SourceHoldError::Invalid`). Hold errors a retry may clear end the episode without a rebuild. |
| `Corruption` | The selected family is quarantined, or its reopen fails with an integrity failure of its own store, connection, or rows. Kernel errors and operator-repair refusals report the slice blocked. |
| `SchemaMismatch`, `AnalysisMismatch`, `TokenizerMismatch`, `EmbeddingModelMismatch`, `ProjectionPolicyMismatch`, `IdentityContractMismatch` | The selected seed was built under an identity that differs from the running identity in that dimension, checked in this order. The embeddings lane supplies the tokenizer, model, dimension, and table epoch; the schema, analysis, policy, and contract versions are constants of the build. |

A family built over another kernel incarnation earns no rebuild; the slice
reports it blocked. The slice loop records the replacement through
`SearchLifecycleOwner::request_rebuild` after the slice releases the manager.
The rebuild names the Current seed as the generation it replaces and reads
through the next registered consumer (`search-projection-1`, then
`search-projection-2`, and so on), so a replayed request derives the same
record. The old family keeps serving within the freshness limit until the
replacement is selected, and retirement releases the old consumer.

Kernel source holds belong to one lease, so a daemon restart followed by any
commit rebuilds the projection under a fresh capture.

Retirement acknowledges the old consumer through the replacement's certified
target and then deregisters it, and the kernel deregisters only a consumer at
its tip. A commit that lands between the certified target and the
deregistration leaves the rebuild in its intent record, and the slice reports
`outbox consumer has not reached the commit-log tip` while the old family keeps
serving. A rebuild therefore completes in a quiet window after its target.

## Readers and exact-lookup certificates

`SearchLifecycleOwner::pin` returns a `SearchReader` for the selected family
after the owner has reopened and verified it under the running lease. Readers
revalidate canonical authorization at use.

`SearchReader::completeness_certificate` issues the `CompletenessCertificate`
that `retrieval::exact::resolve` requires for an exact proof. It issues one only
when the family's checkpoint equals the kernel tip and the family's consumer has
acknowledged that checkpoint. One kernel statement
(`KernelStore::capture_consumer_tip_within_budget`) reads the tip, the kernel
database identity, and the acknowledgement from one snapshot. The certificate names
`SearchReader::inventory_epoch`, a digest of the kernel incarnation, the kernel
lease epoch, the selected seed digest, and the consumer. A proof minted before a
restart, a replacement, or a later commit fails `validate_for_use` against the
current epoch, checkpoint, or tip.

## Admission status

`session.status` carries a `search_admission` block whenever the daemon runs a
lifecycle owner: `{"state": "admitted"}` when the gate admits the selected
family's reads, or `{"state": "refused", "reason": <code>}` with the
`Denial::code` of the gate's refusal, for example `no_manifest`,
`manifest_identity`, `evidence_identity`, `evidence_failed`, or
`coverage_stale`. Records the last slice refused to read report
`records_unreadable`, `records_refused` (for example a record readable by
other users), or `records_malformed` in place of `no_manifest`. A `retrieval.query` whose pin the gate refuses answers
`lane_unavailable` with the same code as its `reason`, and `no_family` once
the gate admits but no family is selected. Codes name the gate or condition,
never record content. OpenCode's `/eidnara-status` renders the block as
`Search admission`.

## Disable and recovery

`SearchLifecycleOwner::disable` closes admission before any write, persists the
disabled intent, and reconciles the old consumer within the manifest's cleanup
envelope. `SearchLifecycleOwner::request` with `Transition::AuthorizedRecovery`
and an authorization reference reopens a Disabled record. Restart and elapsed
time grant no authorization.

## Witnesses

`crates/daemon/tests/search_lifecycle_owner.rs` covers registration, restart
resumption, catch-up, the real daemon loop rebuilding after a post-restart
commit, direct FTS5 probes over a Current family, and exact-proof invalidation
across a daemon restart. Rebuilds driven through the owner reach Current for
`CatchUpHoldLost` (after a restart and after a released hold within one lease),
`Corruption`, `TokenizerMismatch`, and `EmbeddingModelMismatch` (model and table
epoch). The build-constant dimensions are covered by the cause mapping's unit
tests and by `crates/daemon/tests/search_replacement/recovery.rs`, which
rebuilds each mismatch cause through the recovery coordinator.
`crates/daemon/tests/search_replacement/` covers construction, selection,
retirement, disable, and recovery, including child-process cuts.
