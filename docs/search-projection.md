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
| `Corruption` | The selected family is quarantined, or its reopen fails with an integrity failure of its own store, connection, or rows, or with a stored prefix that contradicts the kernel's census of it (`BuildError::FamilyPrefix`: its checkpoint, its consumer's acknowledgement, or its per-class live inventory). Kernel errors and operator-repair refusals report the slice blocked. |
| `SchemaMismatch`, `AnalysisMismatch`, `TokenizerMismatch`, `EmbeddingModelMismatch`, `ProjectionPolicyMismatch`, `IdentityContractMismatch` | The selected seed was built under an identity that differs from the running identity in that dimension, checked in this order. The embeddings lane supplies the tokenizer, model, dimension, and table epoch; the schema, analysis, policy, and contract versions are constants of the build. |

A family built over another kernel incarnation earns no rebuild; the slice
reports it blocked. A slice that finds the selected family quarantined
reports `Corruption` and keeps the family selected, so every later slice
reports the same cause until the rebuild is recorded. Admission judges a
quarantined selection on the unregistered observation a first build is
admitted on, so the rebuild request is admitted while the quarantine marker
survives a refused or expired request. The slice loop records the replacement through
`SearchLifecycleOwner::request_rebuild` after the slice releases the manager.
The rebuild names the Current seed as the generation it replaces and reads
through the next registered consumer (`search-projection-1`, then
`search-projection-2`, and so on), so a replayed request derives the same
record. The old family keeps serving within the freshness limit until the
replacement is selected, and retirement releases the old consumer.

Kernel source holds belong to one lease, so a daemon restart followed by any
commit rebuilds the projection under a fresh capture.

Retirement certifies the old consumer through the replacement's fixed target:
it removes the old family, records the retirement receipt, and acknowledges the
old consumer through that target. The deregistering commit then calls
`Envelope::retire_outbox_consumer`, which acknowledges the old consumer through
the tip that commit finds and deregisters it in the same transaction. Commits
that land after the target therefore complete the rebuild instead of holding
it in its intent record. The replacement's consumer is acknowledged through
the target, so it still reads those commits, and the Current family catches up
to them under its construction hold.

A record whose deadline passes before it completes stays in its intent record
with every hook denied; `request_rebuild` records only over a Current record.

## Rollback

The lifecycle record stores its cause by name, and a binary reads only the
causes it was built with: a record it cannot decode is `Unavailable` and every
hook is denied. A home whose record names `Registration` or `CatchUpHoldLost`
is therefore unreadable to a build that predates that cause. Rolling such a
home back to an older binary requires removing
`<data_home>/search-lifecycle/intent.json` first. An older binary that
registers projections then records a fresh build from the installed admission
records.

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
lease epoch, the selected seed digest, and the consumer. It also records the
`CommitReadIncarnation` the kernel snapshot reported; `resolve` refuses a
certificate whose incarnation differs from the kernel's current one, so a
restore to the same tip within one lease invalidates certificates issued
before it. A proof minted before a
restart, a replacement, or a later commit fails `validate_for_use` against the
current epoch, checkpoint, or tip.

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
`CatchUpHoldLost` (after a restart, after a released hold within one lease, and
with a commit landing between selection and retirement), `Corruption` (damaged
payload bytes, a missing canonical row, and a family quarantined while
selected), `TokenizerMismatch`, and `EmbeddingModelMismatch` (model and table
epoch). The build-constant dimensions are covered by the cause mapping's unit
tests and by `crates/daemon/tests/search_replacement/recovery.rs`, which
rebuilds each mismatch cause through the recovery coordinator.
`crates/daemon/tests/search_replacement/` covers construction, selection,
retirement, disable, and recovery, including child-process cuts.
`crates/kernel/tests/kernel_outbox.rs` covers consumer retirement past later
commits.
