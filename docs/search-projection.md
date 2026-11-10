# Search projection lifecycle

The daemon owns one search projection per data home. `SearchLifecycleOwner`
(`crates/daemon/src/search_lifecycle_owner.rs`) holds the admission owner, the
selection manager, and the running identity. It advances one durable lifecycle
record a bounded slice at a time. The kernel stays the canonical authority; the
projection is a rebuildable derived family.

## Source classes

The projection indexes six Kernel source classes: `messages`,
`canonical_claims`, `promoted_memory`, `git_commits`, `raw_tool_spans`, and
`repository_documents`. The retrieval schema's `occurrences.class` CHECK lists
exactly `OccurrenceClass::ALL`, and the per-class identity contract is in
[construction contracts](properties/search-projection/construction-contracts.md).
`repository_documents` joined the set at schema version 8, keyed by
`(project_id, repository_id, relative_path)` with representation `text`. A
projection built at an earlier schema version fails `SchemaMismatch` and
rebuilds from the Kernel; it is never migrated. A binary built before schema
version 8 cannot re-encode a `repository_documents` descriptor and fails its
export, so the producer of the class activates only after every older binary
is retired, and a store holding such rows does not roll back below version 8.

## Records

The lifecycle record lives outside the disposable search database, under the
data home (`crates/daemon/src/projection_lifecycle.rs`). It is in one of these
states:

| State | Meaning |
|---|---|
| Absent | No projection exists. Installed admission records let the slice loop record the first build (`Cause::Registration`). |
| Intent | A rebuild or authorized recovery is in progress. The record fixes the cause, attempt identity, consumer binding, allowance, and deadline. A restart resumes it as recorded. A rebuild that lapses before it stages a seed is renewed by the slice loop (see [Expiry and renewal](#expiry-and-renewal)). |
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
blocked once its allowance is spent. Its staged seed is selected, so the
slice loop leaves it for the operator once its deadline passes (see
[Expiry and renewal](#expiry-and-renewal)).

The completion slice validates the selected family's rows against the kernel
inventory before it retires the old consumer. The reopen at the start of the
slice covers a family already at the target, and a family that caught up is
validated again at the new target. The check before Current repeats that
validation unless the family database's commit counters show the retirement
receipt transaction as its only change since, the selected consumer's
acknowledgement lies inside the validated prefix, and no pending job's episode
deadline has passed.

## Expiry and renewal

A rebuild's deadline is a wall-clock time in the durable record, so a host
suspend, a clock step, or a stall longer than the time left expires it while
the daemon is idle. A renewable record is a Rebuilding record that holds no
staged seed. A stage certificate persisted in its capture names the capture's
hold and snapshot, the seed digest a later pin must match, and the checkpoint
the fixed target equals; renewal keeps the capture and target, and the
renewed attempt's cleanup discards the unpinned stage before it captures
again. Its renewal is due (`LifecycleIntent::renewal_due`) when its
deadline has passed, or when it was recorded more than
`CLOCK_STEP_TOLERANCE_MS` (1 second) after the current wall clock, as after
the clock stepped back past its recording. A step back smaller than the
record's age leaves its recording behind the clock, and the attempt runs
longer by the size of the step.

A slice over an active record whose deadline has passed, or whose renewal is
due, installs the records with no coverage, so every hook is denied and a
cleanup keeps its evidence. When the renewal is due and an embedding
supervisor is still running, that slice reports `RotateMaintenance` and the
loop drains the supervisor; a drain that does not resolve is reported as a
blocked slice and the renewal waits. The next slice reports
`SliceOutcome::Expired`, and the slice loop calls
`SearchLifecycleOwner::renew_expired_rebuild`. That call reads the records and
lane, refreshes admission on the selected family, and has
`ProjectionLifecycle::renew_expired` rewrite the record for another attempt:

- The transition, generations, consumer binding, cause, attempt identity,
  target, and capture stay as recorded.
- The allowance keeps its size with nothing consumed, the deadline becomes one
  `B_recovery_ms` from the renewal, and the renewal is the record's new
  recording time, so the renewed attempt is one `B_recovery_ms` from a
  recording on the current clock.

A manifest that cannot bound the renewed attempt (its replacement
specification, coverage bounds, or `B_recovery_ms`) refuses the renewal and
closes admission, as a slice's preparation would. A `B_recovery_ms` within
the one-second start margin is refused before anything is installed. A
renewal that still finds an embedding supervisor running refuses and wakes
the loop, so the next slice hands it back. A record whose renewal is no longer due at the clock
read just before the write, as when a clock-step lead decays during the
waits, records nothing, leaves the records installed with no coverage, and
counts as an attempt; the loop is woken so the next slice judges the active
record at once. A refused renewal write installs the records with no
coverage again. The attempt identity keys the construction's consumer
registration, so the renewed attempt replays that registration's receipt and
reads through the consumer the expired attempt registered. No consumer is left
registered without a record that names it, and the replaced family's consumer
retires when the renewed rebuild completes.

The loop attempts the first renewal at once, including one the first slice
after a restart finds. Each attempt, whether it records a renewal, is refused,
or finds the record no longer due, spaces the next one
from it by `SLICE_IDLE` doubled per earlier attempt, running alongside the
renewed record's own `B_recovery_ms`. The spacing reaches
`RENEWAL_BACKOFF_CAP` (10 minutes) at the eighth attempt; from then on a
rebuild that keeps expiring for a persistent cause is attempted at most once
per cap interval. A record that reaches Current starts the schedule again.

Two kinds of expired record stay in their intent record with every hook
denied and report the slice blocked on the deadline. An authorized recovery
keeps the bound its operator authorized. A rebuild that holds a staged seed
keeps its accounting by policy: once selection has written the staged
family's certificate, that certificate binds the record's episode accounting
and recording time, and a seed pinned before then is left with the same
rule. An operator's disable reconciles a selected staged family. For an
unselected staged seed the disable records the stop and reports that the
disabled selection differs from the handoff; the stop then waits for an
authorized recovery. `request_rebuild` records only over a
Current record.

An unselected rebuild whose fixed target the kernel tip has reached cannot
count on capturing at that target again, since a capture is taken at the tip
and any commit before it moves the tip past the target. Its builder clears the
capture first and then releases the target through
`ProjectionLifecycle::release_target`; the attempt captures a fresh source
snapshot at the tip and, after building its baseline from it, fixes the target
again at the kernel tip it then reads. A target ahead of the tip, as after a
kernel restore, still refuses the attempt with `TargetBeyondTip`. An
authorized recovery keeps its target and reports `SnapshotBeyondTarget` once
the tip has passed it.

A rebuild's construction is admitted on the loaded predecessor family's
coverage. While the owner holds a predecessor that trails the kernel by more
than `catchup_lag_commits`, construction and renewal are denied `Stale`; a
daemon restart loads no predecessor for an unselected rebuild, and the rebuild
and its renewal are admitted on the unregistered observation.

The daemon logs `search lifecycle rebuild expired: its deadline passed or the
wall clock stepped back past its recording` when a slice first finds a
record's renewal due, and `search lifecycle renewed expired rebuild:
attempt=<id> consumer=<id> cause=<cause> renewal_attempts=<n> deadline=<ms>`
for each renewal.

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

Every kernel eligibility batch that `resolve` issues, and the kernel capture
that `validate_for_use` takes, runs under the request's `EvalBudget`. A
deadline or cancellation that an enumeration batch observes stops the attempt as
`Incomplete(BudgetExhausted)`, keeping the verdicts of earlier pages and
forming no proof. At final use it invalidates the proof as
`BudgetExhausted`. Under `SqliteStore::with_conn_interruptible`, a stop tied
to the request's budget also interrupts a running exact statement. `resolve`
classifies that SQLite interruption by error code as the same budget stop:
`Err(BudgetExhausted)` when it lands in certificate admission or before any
page completes, and `Incomplete(BudgetExhausted)` with the judged pages kept
after one. The interrupted page is dropped unjudged. An interrupted final-use
checkpoint read invalidates the proof as `BudgetExhausted`. Every other engine
error stays an engine failure whatever the budget:
`Lookup(Projection(Sqlite))` from a page and `Projection(Sqlite)` from
admission. A classification change defeats an attempt and a proof with or
without a tip move: a reclassifying artifact replay commits and moves the tip,
and an open classification window leaves the snapshot without a reusable
generation at the same tip.
One kernel batch judges each page's distinct (object, revision, artifact
digest) terms once, and every row carrying those terms takes that verdict.
Final use takes one kernel capture of the database identity, incarnation,
tip, and classification generation. The proof's occurrences were judged
eligible at the snapshot the proof records, under the proof's project and
destination, which final use checks first. A verdict is a function of that
snapshot's egress state, the project, and the destination, so a capture equal
to the snapshot validates every occurrence, and any difference invalidates the
proof as `IncarnationChanged`, `CanonicalChanged`, or `SnapshotChanged`.
An exact proof records egress
eligibility only. Surface
permission and claim state come from claim classification at use, so a
quarantined or stale claim object can hold an exact proof and still be hidden
or stale on every surface.

## Exact routing on `retrieval.query`

The route's handler pins a reader, then calls `ExactCertification::issue`
when the request names an `id:` selector. Only a gate-admitted pin yields a
`SearchReader`, so only such a reader certifies. Issuance reads the projection
under the request's budget, so a cancellation or deadline ends its connection
wait and its statements. With a certification, the
exact lane resolves each `id:` selector through `retrieval::exact::resolve`
under the request's one `SharedBudget`, inside the same interruptible
projection read as the other lanes. Each attempt reads at most
`exact_page_rows` times `exact_pages` rows and retains at most that many
rows, capped by the kernel's batch bound and by `response_bytes`. The
inventory epoch is read from the reader at resolution and again at final use.

Only a whole-request selector whose resolution holds a proof routes directly.
The route then polls the budget, checks that the reader is still usable (its
family available and unquarantined, its admission standing), and runs
`validate_for_use` under that read; the reader is checked once more after
validation. A proof that passes skips admission, fusion, and fused
revalidation, and the lexical and dense lanes, which a whole-request selector
leaves undeclared. `Fused::direct` serves the proof's occurrences in
exact-lane order under the same `fused_union` and response bounds as a fused
answer, and claim validation judges them at the response's snapshot. Every
other resolved request continues through hybrid retrieval: a mention beside
prose or other selectors, a resolution without a proof, and a proof that
final use invalidates. The hybrid exact lane ranks every row the resolver
retained before fusion, the union bound, and revalidation, so a later
omission belongs to revalidation, claim validation, or the response bounds.
A refusal on a later selector ends the lane with the rows earlier selectors
retained. `ExactReport::route` records the route and
`ExactReport::resolutions` the attempts.

A local bound (`page_bound`, `row_bound`, `retention_bound`) marks the exact
lane incomplete and the request continues. A budget that ends during
certification, resolution, final use, or any later phase ends the request as
`deadline` or `cancelled`; no later lane starts. A reader that issues no
certificate, a certificate the projection refuses at the first selector, and
a reader withdrawn before resolution or before final use all route the lane
to uncertified page reads, which the kernel judges at admission; the request
continues through hybrid retrieval and its exact work forms no proof.

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
resumption, catch-up, the renewal of a rebuild a clock step expired
(`a_rebuild_expired_by_a_clock_step_is_renewed_and_its_consumer_reaches_the_tip`),
the slice loop's renewal of an expired rebuild found after a restart
(`the_slice_loop_renews_an_expired_rebuild_found_after_a_restart`), the renewal
of a record stamped ahead of a clock stepped back
(`a_rebuild_recorded_ahead_of_a_clock_stepped_back_is_renewed_from_the_current_clock`),
an expired authorized recovery and an expired rebuild holding an unselected
staged seed left for the operator
(`an_expired_authorized_recovery_is_left_for_the_operator`,
`an_expired_rebuild_holding_an_unselected_staged_seed_is_left_for_the_operator`),
a refused renewal write
(`a_refused_renewal_leaves_the_expired_record_and_admits_no_reader`), a renewal
refused for the start margin
(`a_renewal_with_no_time_past_the_start_margin_is_refused`), the release
of a target the tip passed
(`an_unselected_rebuild_behind_the_tip_releases_its_target_and_completes`), the real daemon loop rebuilding after a post-restart
commit, direct FTS5 probes over a Current family, and exact-proof invalidation
across a daemon restart. Rebuilds driven through the owner reach Current for
`CatchUpHoldLost` (after a restart, after a released hold within one lease, and
with a commit landing between selection and retirement), `Corruption` (damaged
payload bytes, a missing canonical row, and a family quarantined while
selected), `TokenizerMismatch`, and `EmbeddingModelMismatch` (model and table
epoch). The build-constant dimensions are covered by the cause mapping's unit
tests and by `crates/daemon/tests/search_replacement/recovery.rs`, which
rebuilds each mismatch cause through the recovery coordinator.
`crates/daemon/tests/search_replacement.rs` covers the release of a target at
the tip (`a_rebuild_target_at_the_tip_is_released_and_fixed_again_past_it`)
and the refusal of a spent rebuild before its target is released
(`a_spent_rebuild_keeps_its_target`).
`crates/daemon/tests/search_replacement/` covers construction, selection,
retirement, disable, and recovery, including child-process cuts.
`crates/kernel/tests/kernel_outbox.rs` covers consumer retirement past later
commits.
