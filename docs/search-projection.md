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
| `Corruption` | The selected family is quarantined, its coverage observation fails with an integrity failure of its own store, connection, or rows, or its reopen fails with an integrity failure of its own store, connection, or rows, or with a stored prefix that contradicts the kernel's census of it (`BuildError::FamilyPrefix`: its checkpoint, its consumer's acknowledgement, or its per-class live inventory). Kernel errors and operator-repair refusals report the slice blocked. A coverage observation that finds a class over the family's coverage bounds (`CoverageUnavailable::OverBound` or `TombstonedOverBound`) also reports the slice blocked and leaves the family unquarantined, so a manifest that raises those bounds serves the same family again. |
| `SchemaMismatch`, `AnalysisMismatch`, `TokenizerMismatch`, `EmbeddingModelMismatch`, `ProjectionPolicyMismatch`, `IdentityContractMismatch`, `LimitProtocolMismatch` | The selected seed was built under an identity that differs from the running identity in that dimension, checked in this order. The embeddings lane supplies the tokenizer, model, dimension, and table epoch; the schema, analysis, policy, and contract versions are constants of the build; the installed manifest supplies the limit protocol version. |
| `KernelRestored` | The selected seed's identity equals the running identity and the kernel's commit-read incarnation differs from the one the family was opened under, as after a kernel restore within the same database lineage. The kernel is judged before the family's coverage, so a selected checkpoint past the restored tip still earns this cause, and admission treats the family as unregistered while it is selected. Its retirement needs no checkpoint for the old consumer: a backup predating that consumer's registration restores a kernel that holds none. A family built over another kernel incarnation earns no rebuild. |

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

Retirement certifies the old consumer through the replacement's recorded
target: it removes the old family, records the retirement receipt, and
acknowledges the old consumer through that target. The deregistering commit
then calls `Envelope::retire_outbox_consumer`, which acknowledges the old
consumer through the tip that commit finds and deregisters it in the same
transaction, recording the certified target in its audit. A commit that lands
between the acknowledgement and that commit therefore completes the rebuild
instead of holding it in its intent record. The replacement's consumer is
acknowledged through the target, so it still reads those commits, and the
Current family catches up to them under its construction hold.

When the completion slice finds the old consumer still registered and the tip
past the recorded target, it recertifies before retiring:

1. The selected family catches up to that tip under its own hold and the
   episode the slice consumed, and the new consumer acknowledges each window.
2. `ProjectionLifecycle::recertify_target` moves the recorded target forward to
   the tip the family applied. The attempt, allowance, and deadline stay as
   recorded, so every recorded target is a commit the selected family holds.
3. Retirement certifies the old consumer's obligations through the new target
   and acknowledges it there. A receipt for the same retirement through an
   earlier commit is replaced by the later one.

A restart ends the selected family's hold, so a family that trails the tip
after a restart cannot catch up under it. The slice then retires the old
consumer through the commits the family applied, the deregistering commit
acknowledges it through the tip, and the Current family catches up or
rebuilds from there. A family that applied the tip before the restart
recertifies and completes. Each slice that stops before the family reaches
the tip spends one episode, so a record whose slices keep stopping short is
blocked once its allowance is spent.

The completion slice validates the selected family's rows against the kernel
inventory before it retires the old consumer. The reopen at the start of the
slice covers a family already at the target, and a family that caught up is
validated again at the new target. The check before Current repeats that
validation unless the family database's commit counters show the retirement
receipt transaction as its only change since, the selected consumer's
acknowledgement lies inside the validated prefix, and no pending job's episode
deadline has passed.

A record whose deadline passes before it completes stays in its intent record
with every hook denied; `request_rebuild` records only over a Current record.

## Rollback

The lifecycle record stores its cause by name, and a binary reads only the
causes it was built with: a record it cannot decode is `Unavailable` and every
hook is denied. A home whose record names `Registration`, `CatchUpHoldLost`,
`LimitProtocolMismatch`, or `KernelRestored` is therefore unreadable to a build that predates
that cause. Rolling such a
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

## Admission status

`session.status` carries a `search_admission` block whenever the daemon runs a
lifecycle owner: `{"state": "admitted"}` when the gate admits the selected
family's reads, or `{"state": "refused", "reason": <code>}` with the
`Denial::code` of the gate's refusal, for example `no_manifest`,
`manifest_identity`, `evidence_identity`, `evidence_failed`, or
`coverage_stale`. Records the last slice refused to read report
`records_unreadable`, `records_refused` (for example a record readable by
other users), or `records_malformed` in place of `no_manifest`; a slice whose
preparation of valid records failed reports `lane_not_ready`,
`limits_unbounded`, or `kernel_unreadable` the same way. A `retrieval.query` whose pin the gate refuses answers
`lane_unavailable` with the same code as its `reason`, `no_family` once
the gate admits but no family is selected, and `family_refused` when the gate
admits a selected family whose own checks refused the pin. Codes name the gate or condition,
never record content. OpenCode's `/eidnara-status` renders the block as
`Search admission`.

Every admission, including the one behind the status block, reads the
lifecycle record fresh. The gate checks the control directory's owner and mode
from its path metadata, opening the directory only when that metadata does not
show the caller's own owner-only directory. On ext4, XFS, Btrfs, and tmpfs the
gate keeps the record open and rereads its bytes while the record path still
names the same inode with the same mode, owner, and change time; any other
metadata, and every other filesystem, opens the record again. A record whose
bytes equal the last bytes the gate decoded reuses that decode's verdict.

## Disable and recovery

`SearchLifecycleOwner::disable` closes admission before any write, persists the
disabled intent, and reconciles the old consumer within the manifest's cleanup
envelope. `SearchLifecycleOwner::request` with `Transition::AuthorizedRecovery`
and an authorization reference reopens a Disabled record. Restart and elapsed
time grant no authorization.

The operator drives these through `eidnara-host search`, which sends the
`search.lifecycle.*` methods of context application protocol 4 (Section 7.8 of
`docs/host-wire-protocol.md`) over a route bound with harness `cli`:

| Command | Effect |
|---|---|
| `eidnara-host search status` | Reports the record and the search admission. |
| `eidnara-host search rebuild` | Records a rebuild of the Current family under `Cause::OperatorRequest`. |
| `eidnara-host search disable` | Closes admission and reconciles the consumer. |
| `eidnara-host search recover --authorization <ref>` | Records an authorized recovery of a Disabled record. |
| `eidnara-host search abandon --operator <id> --reason <text>` | Abandons a Disabled record's consumer through the kernel's audited `ConsumerAbandonment`, then reconciles the disable. |

A consumer a commit left behind its tip stays registered after a disable. The
audited abandonment is the operator action that releases it.

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
