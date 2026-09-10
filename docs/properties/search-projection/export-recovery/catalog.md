# RP2.1 export and recovery properties

## Scope and provenance

- Repository: `/local/home/ahrav/scratch/eidnara`.
- Verified HEAD: `913234433ae36a80a6e22c6aac14c7f9aab74386`.
- Inspection date: 2026-09-10.
- Method: `/property-discovery-and-catalog` and
  [METHOD.md](../../METHOD.md).
- External-evidence scope is **supplied by the user**: the settled RP2.1 plan,
  its linked parent/index/research, and the local repository. No incident logs
  are supplied. No additional interview or independent public research occurred.
- Output covers fixed-S export, retention, paging, complete-commit catch-up,
  local release before ack, replacement selection, rebuild, disable lifecycle,
  and canonical authority. Other agents own projection rows/source coverage
  and embedding. This is input to one later spec, not implementation tickets.
- Source inspection establishes current mechanisms. Source guarantees are
  claims under test. No tests, builds, benchmarks, or runtime experiments ran.
- Independent central portfolio review is complete. Analyst session
  `ses_f7623dcccffe3Y09nW2wVoABif` ran all four evaluation lenses; the user
  supplied its findings. [Disposition](portfolio-evaluation.md) records the
  verified corrections and remaining implementation questions, not another
  independent review by this author.

## Sources

The following aliases identify exact external files throughout the evidence.
Line numbers refer to the files inspected on the date above, not a claimed
Commons commit. Product source line numbers refer to the verified Eidnara HEAD.

| Alias | Source | Why consulted |
| --- | --- | --- |
| P | [RP2.1 projection coverage plan](../../../../../commons/docs/plans/2026-09-10-feat-eidnara-rp2-1-projection-coverage-plan.md) | KTD2, U1/U5, lock order, disable behavior, and authority claims. |
| I | [RP2 index](../../../../../commons/docs/plans/2026-09-10-eidnara-rp2-plan-index.md) | Shared ownership, approval, publication reuse, and rollback constraints. |
| N | [Parent native Rust cutover plan](../../../../../commons/docs/plans/2026-09-08-0523-feat-eidnara-native-rust-cutover-plan.md) | Stage 2 source of authority and acceptance dimensions. |
| R | [Retained recovery research](../../../../../commons/docs/research/2026-09-10-rp2/recovery-research.md) | RP2.1 section, lines 33-37, and recovery limitations; secondary mechanism evidence, not local crash proof. |
| L | [RP2.9 measurement plan](../../../../../commons/docs/plans/2026-09-10-feat-eidnara-rp2-9-measurement-acceptance-plan.md) | Approval format and unresolved numerical limits, lines 79-84 and 131-136. |
| D | [Rust design review](../../../../../commons/docs/research/2026-09-10-rp2/rust-design-review.md) | R6, lines 121-131, as a lead about consumer/serving coupling; reconcile its wording against current code. |

Local focal sources are `crates/kernel/src/{outbox.rs,envelope.rs,facts.rs}`,
`crates/kernel/src/slice/read.rs`, `crates/kernel/src/cas/{gc.rs,deletion.rs}`,
`crates/daemon/src/kernel_routes/serving.rs`, and the existing host generation
store. The [check inventory](existing-checks.md) gives exact checked locations.

## Model and reachability

The proposed state sequence is Unregistered, FencedBootstrap, CatchingUp,
Current, then Rebuilding or Disabled (P, lines 106-116). Kernel owns canonical
facts and outbox state; daemon owns export orchestration and publication.
The `crates/retrieval` directory is absent at HEAD. Daemon source has no search
database or outbox consumer driver. Each new record therefore explains its
own `test-only` classification. This means a proposed test target, not an
existing test implementation. No new record is marked exercised.

Current lower mechanisms include monotonic consumer ack, inclusive minimum
checkpoint pruning, historical reads, payload-size lookup, and a host-payload
generation selector. None by itself establishes the full proposed path.
The existing `kernel_proofs` operation model, clean-restart `Proof` fixture,
and table-wise canonical digest are reusable testing seams; their limits and
claim-bearing checks are in [the inventory](existing-checks.md#existing-model-and-canonical-digest-seams).

Notation below names **oracle concepts, not existing APIs or schema**:

- `S` is the fixed snapshot captured after durable fence registration.
- `T` is a finite complete-commit catch-up target captured for publication.
- `E(S)` is the independently constructed export input at S, keyed by
  `(class, object_id, revision)` with required invalidation facts and bytes.
  A stable key does not prove immutable bytes. Domain-name remediation changes
  bytes without changing source revision; applicability to export depends on
  the approved source mapping.
- `H(S,c]` is every canonical commit after S through c, including all event
  ordinals, deletion/control events including `operator_remediation`, published
  retained rows, and empty commits.
- `O(T)` is the projection/source-coverage owner's canonical oracle at T.
  Its class and tombstone policy must be agreed before this catalog is executable.
- `g` identifies one candidate replacement and its compatibility contract.
- `B_recovery_ms`, `B_catchup_ms`, `B_authorized_recovery_ms`, and their finite
  row/byte/commit/attempt envelopes are unapproved RP2.9 oracle parameters.
  Existing serving thresholds do not supply them.

## Index

| Slug | Type | Reachability | Semantics | Status | Confidence |
| --- | --- | --- | --- | --- | --- |
| [export-fixed-s-exactly-once](#export-fixed-s-exactly-once) | safety | test-only | always | active | medium |
| [export-retention-fence-covers-read](#export-retention-fence-covers-read) | safety | test-only | always | active | medium |
| [export-predecode-bounds](#export-predecode-bounds) | safety | test-only | always | active | medium |
| [catchup-complete-commit-prefix](#catchup-complete-commit-prefix) | safety | test-only | always | active | medium |
| [ack-follows-local-release](#ack-follows-local-release) | safety | test-only | always | active | medium |
| [replacement-selects-complete-compatible-state](#replacement-selects-complete-compatible-state) | safety | test-only | always | active | medium |
| [rebuild-after-pruning-converges](#rebuild-after-pruning-converges) | liveness | test-only | always per admitted episode; RP2.9-blocked | active | medium |
| [catchup-and-authorized-recovery-converge](#catchup-and-authorized-recovery-converge) | liveness | test-only | always per admitted episode; RP2.9-blocked | active | medium |
| [disable-preserves-consumer-obligations](#disable-preserves-consumer-obligations) | safety | test-only | always | active | medium |
| [recovery-preserves-canonical-authority](#recovery-preserves-canonical-authority) | safety | test-only | always | active | medium |

## Records

### export-fixed-s-exactly-once

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - fixed-S paged production export is absent; slice fixtures
exercise only lower-level reads.
Guarantee: A successful paged export returns exactly the canonical export input
at its original S, once per stable key, despite commits between pages.
Check: `always` - every accepted page retains S and strictly advances the
stable cursor; concatenating a completed export equals sorted `E(S)` by key,
revision, invalidation facts, and bytes, with multiplicity one; if required
bytes at S cannot be supplied after an in-place mutation, abort that attempt
rather than mix bytes, invent a revision, or undo canonical remediation. A
failed export is not reported complete. Check every page and completion.
Fault/timing angle: Insert, correction, delete, or domain-name
`operator_remediation` commits between page reads.
Required faults and enabling state: At least two pages; a later commit changes
an already-read key and an unread key; include an empty final page and a class
boundary. Compare with an independent canonical fixture ledger at S. Include
in-place remediation; its effect on export bytes is conditional on the approved
mapping consuming the affected field, not assumed for all RP2.1 sources.
Confidence: medium - [evidence](evidence/export-fixed-s-exactly-once.md).
P establishes the claim; historical predicates exist, but stable export is
proposed and has no production caller, hence the reachability classification.
Existing check: `crates/kernel/tests/kernel_slice.rs:466-559` and
`crates/kernel/tests/kernel_envelope.rs:246-289` check historical visibility;
`crates/kernel/tests/kernel_proofs/obligations/o5_correction.rs:234-278`
checks frozen snapshots across correction chains; status `unaudited`.
None checks a multi-page export.
Impact: Mixed snapshots omit or revive occurrences before catch-up begins.
Open questions:

- Which history and tombstone rows constitute `E(S)` for each source class,
  and how does its ordering encode the declared class key? (needs human input)
- Does approved export consume remediable domain names, and how can required
  S bytes be supplied or their loss detected without reversing remediation?
  (needs human input)

### export-retention-fence-covers-read

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - no production export fence covers both canonical source
bytes and outbox history through replacement publication.
Guarantee: Bootstrap captures S only after a durable retention fence and never
publishes a replacement built across a loss of required source or history.
Check: `always` - `register_committed < capture_S`; every accepted page and
catch-up interval has continuous retention coverage for its required input;
loss of that coverage prevents selection of g, ends that bootstrap attempt,
and schedules partial-state removal with any removal error exposed. These
conditions hold at every use and publication decision.
Fault/timing angle: Prune, source GC/purge, in-place remediation of a required
field, fence invalidation, or restart after S but before completion.
Required faults and enabling state: A registered bootstrap consumer, unread
source bytes, pruning pressure, and a fault that invalidates coverage while
the replacement is incomplete. Also attempt pruning with a valid fence.
Confidence: medium - [evidence](evidence/export-retention-fence-covers-read.md).
Current retention primitives are verified; their export-specific composition
is absent, so this is a proposed-only test target.
Existing check: `crates/kernel/tests/kernel_outbox.rs:156-224` checks the prune
horizon; `crates/kernel/tests/kernel_retention.rs:316-391` separates staging
cleanup from unacked rows; status `unaudited`. No export-fence check exists.
Impact: A nominally complete replacement silently loses retained history or bytes.
Open questions:

- What mechanism, lifetime, and validity witness hold source bytes as well as
  outbox rows, and which forced purges require abort? (needs human input)
- How is partial-state cleanup retried and bounded after a removal failure?
  (needs human input)

### export-predecode-bounds

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - all-class pre-decode paging and decoded-memory accounting
are absent from production export.
Guarantee: Export admits payloads before decoding within approved page limits
and explicitly fails an individually oversized row instead of skipping it.
Check: `always` - each payload materialization/decoder entry has a preceding
size admission for the same S and key; admitted rows and encoded bytes stay
within their page caps, measured live decoded bytes stay within the decoded
cap, and a row over the standalone admission cap causes an explicit failure
without payload materialization or decode. Every page, including error paths,
must satisfy the limits; post-return truncation is not a bound.
Fault/timing angle: A large final row, many small rows, JSON expansion, and
size discovery followed by a changed or missing source.
Required faults and enabling state: Exact-cap and over-cap payloads, a page
with insufficient residual byte budget, and independent decode-entry and
allocation/high-water observations. A row that fits alone may move to the next
page; it may not disappear from the export.
Confidence: medium - [evidence](evidence/export-predecode-bounds.md).
Existing SQL size lookup is reusable; bounded export and its live decoded-heap
observer remain absent. Kernel forbids unsafe code at
`crates/kernel/src/lib.rs:5`; this is a proposed-only test target, not permission
to add an allocator or change that boundary.
Existing check: `crates/kernel/tests/kernel_slice.rs:562-609` compares reported
sizes for live decisions; status `unaudited`. None observes export admission
before allocation or decoded-memory high water.
Impact: Bootstrap can exhaust memory or silently omit its largest records.
Open questions:

- What are approved row, encoded-byte, decoded-byte, and single-row limits,
  and what allocations count toward decoded bytes? (needs human input)
- Which permitted observation boundary measures live decoded heap high water?
  Logical admission charges are not physical-allocation evidence, and the
  existing unsafe perf allocator counts cumulative requested bytes instead.
  (needs human input)

### catchup-complete-commit-prefix

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - there is no production per-consumer catch-up driver from S.
Guarantee: A catch-up checkpoint names a complete ordered canonical commit
prefix, never a partial commit or a prefix with missing retained events.
Check: `always` - when g records durable catch-up checkpoint c, its applied
logical history equals `H(S,c]`; each commit has every required ordinal in
order, retries have one logical application, and partial buffered commits do
not advance c. A publication target T is reached only at such a boundary.
This holds at each checkpoint, not just at the end of replay.
Fault/timing angle: Batch cuts inside a commit, repeated or interrupted
delivery, another publisher marking rows, and empty canonical commits.
Required faults and enabling state: Multi-row commits larger than one batch,
published-but-retained rows after S, duplicate delivery, an empty commit, and
`operator_remediation`. Account for that control event even when approved
mapping has no dependency on its domain-name field. Use the canonical
transaction ledger as oracle, not `pending_outbox` results.
Confidence: medium - [evidence](evidence/catchup-complete-commit-prefix.md).
Existing publisher boundaries are verified; the proposed consumer-specific
reader/driver is absent, so its reachability is `test-only`.
Existing check: `crates/kernel/tests/kernel_outbox.rs:86-153`, `:417-434`, and
`:622-698` cover consumer sequence admission and publisher boundaries;
status `unaudited`. They do not prove consumer work completeness.
Impact: A checkpoint can permanently skip one part of a canonical transaction.
Open questions:

- How does bounded catch-up read published retained history and represent
  commits with no rows? (needs human input)
- How does a commit larger than the local transaction/batch cap complete or
  fail explicitly without unbounded buffering? (needs human input)

### ack-follows-local-release

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - no production search transaction/kernel ack composition
or ownership trace exists.
Guarantee: The daemon attempts kernel acknowledgement only after committing
and releasing the corresponding local projection transaction.
Check: `always` - for every attempted ack of c for g, local durable checkpoint
is at least c and all local transaction/connection locks for that batch are
released before kernel writer acquisition; no path simultaneously holds the
two transactions, and stored kernel ack never exceeds the matching durable
local prefix. Apply the check to retries and errors as well as successful
returns, because a lost ack response does not undo kernel COMMIT.
Fault/timing angle: Crash after local COMMIT, blocked kernel writer, ack error,
and lost response after kernel COMMIT.
Required faults and enabling state: A real local transaction, a contended
kernel writer, and interruption on both sides of local release and ack. Record
attempted acks, returned acks, and durable checkpoints separately.
Confidence: medium - [evidence](evidence/ack-follows-local-release.md).
Kernel writer acquisition is verified; production search transactions are
absent, so the composite obligation is a proposed-only test target.
Existing check: `crates/kernel/tests/kernel_outbox.rs:86-153` checks monotonic
ack admission; status `unaudited`. No cross-database lock or durability check
exists. Row/checkpoint/job atomicity is handed to the projection-row owner.
Impact: Early ack permits loss through pruning; overlapping locks can deadlock
or stall canonical writes.
Open questions:

- Which production boundary exposes transaction release and ack acquisition
  to a deterministic observer without adding a second coordinator?
  (needs human input)

### replacement-selects-complete-compatible-state

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - the search replacement selector and recovery path are
absent; host payload generation fixtures cover a different subject.
Guarantee: Replacement publication exposes only a prior compatible projection
or a fully verified new projection, and releases old consumer state afterward.
Check: `always` - every served read binds to one selected g whose compatibility
tuple matches the active schema, tokenizer, model, projection policy, and
identity contract and whose local state equals `O(T)` at its complete target;
an interrupted switch exposes the prior compatible g, the complete new g, or
explicit unavailability, never mixed/partial state; old consumer release
requires durable verified selection of the replacement. Check every read and
release transition, including restart after selector errors.
Fault/timing angle: Failure before or after selector replacement, restart with
an incomplete candidate, corrupt local files, and compatibility change.
Required faults and enabling state: Distinguishable old/new projections, a
reader during selection, staged catch-up work, and interruption at each
publication boundary. If no compatible prior exists, unavailability is valid.
Confidence: medium - [evidence](evidence/replacement-selects-complete-compatible-state.md).
P U5 establishes the search-specific claim; its production selector does not
exist, which justifies `test-only` independently of host lifecycle reachability.
Existing check: Reuse
[current-profile-never-names-an-unvalidatable-generation](../../host-runtime/catalog.md#current-profile-never-names-an-unvalidatable-generation)
for generic selector validity; its checks remain `unaudited` here. No existing
check binds a selected search database to complete catch-up or its five identities.
Impact: A valid-looking but incomplete or incompatible projection becomes visible.
Open questions:

- What publication unit includes SQLite state and any live journal, and where
  is durable selection recognized after an ambiguous filesystem error?
  (needs human input)
- How does old-consumer release honor already recorded deletion barriers and
  a tip that advances during cutover? (needs human input)

### rebuild-after-pruning-converges

Type: liveness
Reachability: test-only
Status: active
Exercised: not yet - the production rebuild supervisor and approved RP2.9
recovery bound/input envelope are absent; execution is RP2.9-blocked.
Guarantee: Within an approved bounded fault-free recovery window, deleting
the projection after outbox pruning reconstructs complete canonical coverage
through a fixed target T when required sources remain retained.
Check: `always` - per admitted episode, recovery begins at t0 after pressure
and faults stop, with finite target T, available sources, and approved input
caps; by `t0 + B_recovery_ms` it has selected a compatible projection equal
to `O(T)` with complete local checkpoint through T and reconciled kernel ack
through T, without exceeding approved work/attempt caps. Evaluate every
episode at its bound. Disabled, endless restart, or a continually moved T is
not success. The check is blocked until RP2.9 approves these parameters.
Fault/timing angle: Actual projection deletion after prune, concurrent writes
during bootstrap, process termination/restart, then a bounded quiet window.
Required faults and enabling state: Prove older outbox rows are absent, search
state is absent, and retained canonical state is nonempty; include correction
and deletion before pruning. Stop new writes and fault injection for the
admitted window; separately test source loss as the fence safety case.
Confidence: medium - [evidence](evidence/rebuild-after-pruning-converges.md).
P U5 supplies the convergence obligation; no production recovery implementation
or numerical bound exists, hence `test-only` and no exercise claim.
Existing check: None for search reconstruction after pruning.
`crates/kernel/tests/kernel_outbox.rs:333-414` covers clearing an internal
alignment projection only;
`crates/kernel/tests/kernel_proofs/obligations/o8_restart_backup.rs:323-378`
checks position continuity after prune and clean reopen; status `unaudited`.
Impact: Rebuildable state is operationally unrecoverable despite intact authority.
Open questions:

- What are `B_recovery_ms`, the finite row/byte/commit envelope, retry/work
  ceilings, and required fence lifetime for this acceptance window?
  (needs human input)
- Is final ack reconciliation part of the RP2.9 recovery acceptance boundary
  or a separately bounded phase? (needs human input)

### catchup-and-authorized-recovery-converge

Type: liveness
Reachability: test-only
Status: active
Exercised: not yet - production catch-up/recovery orchestration and approved
RP2.9 episode bounds are absent; execution is RP2.9-blocked.
Guarantee: Every admitted healthy finite-target episode reaches Current within
its approved bound, either from ordinary CatchingUp or from Disabled after
explicit recovery authorization and acceptance of all prerequisites and gates.
Check: `always` - per admitted episode, freeze t0, mode, and finite T; ordinary
mode requires an existing local complete prefix below T. Admit only with
retained inputs, healthy workers and storage, compatible contracts, approved
work/resource caps, and accepted authority, capability, and feature gates.
Disabled mode requires a prior Disabled state and recorded explicit recovery
authorization before t0; missing local state requires the accepted rebuild
path rather than an invented checkpoint. By `t0 + B_catchup_ms` for ordinary
catch-up or `t0 + B_authorized_recovery_ms` for authorized recovery, observe
Current, compatible selected coverage equal to `O(T)`, and complete durable
local and reconciled kernel checkpoints through T within the approved work and
attempt ceilings. Check each episode at its fixed bound; retries cannot reset
t0 or move T. Bounds remain RP2.9-blocked. This does not authorize automatic
enablement or recovery when authorization or any prerequisite gate is absent.
Fault/timing angle: A finite backlog after pressure, or a Disabled state after
failure, followed by healthy admitted operation with injection and new writes
stopped for the declared interval.
Required faults and enabling state: Construct ordinary lag and a separate
Disabled-to-authorized-recovery episode; confirm every admission prerequisite
independently of progress. Re-register and bootstrap if the chosen recovery
requires them. Source inventory and bounded source sweeps are dependencies of
`O(T)`, owned by projection-coverage rather than a second implementation here.
Confidence: medium - [evidence](evidence/catchup-and-authorized-recovery-converge.md).
P defines CatchingUp/Current and operator recovery; current kernel primitives
do not implement that production controller, which justifies `test-only`.
Existing check: None for either complete episode. The real-kernel fixture at
`crates/daemon/tests/transform_canonical_memory.rs:230-302` shows withholding
and restoration after a manual ack, not autonomous catch-up or authorization;
status `unaudited`.
Impact: A safe projection can remain stale or Disabled indefinitely despite
healthy conditions and an accepted recovery request.
Open questions:
- What are the RP2.9-approved per-mode interval, finite backlog/input envelope,
  work/attempt ceilings, and endpoint for ack reconciliation? (needs human input)
- Which authority records recovery authorization and all prerequisite/gate
  acceptance across restart? (needs human input)

### disable-preserves-consumer-obligations

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - kernel lifecycle methods exist, but production retrieval
disable/pause/recovery orchestration is absent.
Guarantee: Retrieval disable keeps retrieval unavailable and never disguises
pause, safe deregistration, or audited abandonment as equivalent transitions.
Check: `always` - pause preserves the registered checkpoint and its retention
obligations; deregistration occurs only after satisfying the pre-operation tip;
abandonment records required operator/reason/time/barrier facts before removing
the consumer; `ConsumerPending` never becomes fake progress or silent abandon;
disabled retrieval stays unavailable until explicit recovery. With zero
remaining consumers, gated routes report no required consumer, direct tip
reads retain their existing behavior, and pruning refuses an empty horizon.
These are checked at every transition and retry, not just on successful disable.
Fault/timing angle: Disable while lagging, tip movement before deregistration,
crash during lifecycle change, and a blocked deletion barrier.
Required faults and enabling state: Lagging and caught-up consumers, last and
non-last removal, a recorded barrier, and explicit operator abandonment as a
separate scenario. Observe durable rows and each serving surface independently.
Confidence: medium - [evidence](evidence/disable-preserves-consumer-obligations.md).
The kernel guards are verified; the subject is the unimplemented retrieval
transition, so its classification is `test-only`.
Existing check: `crates/kernel/tests/kernel_outbox.rs:247-330`,
`crates/kernel/tests/kernel_deletion.rs:212-292`, and
`crates/daemon/src/kernel_routes/serving.rs:116-224`; status `unaudited`. Reuse
[canonical-read-staleness-is-distinguishable-from-emptiness](../../daemon/transform/catalog.md#canonical-read-staleness-is-distinguishable-from-emptiness)
for withheld rendering.
Impact: Disable can lose required history or silently degrade unrelated reads.
Open questions:

- How does default disable resolve `ConsumerPending` while remaining disabled,
  without advancing unapplied progress or silently abandoning? (needs human input)
- Who authorizes recovery and how is disabled state preserved across restart?
  (needs human input)

### recovery-preserves-canonical-authority

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - there is no production search recovery/egress path on
which to observe authority preservation.
Guarantee: Recovery changes derived artifacts without rewriting canonical
truth to fit them or treating stale projection state as eligibility authority.
Check: `always` - each recovery-owned mutation leaves canonical source facts,
revisions, dispositions, and tombstones equal to the independent canonical
ledger, except separately recorded legitimate canonical operations; lifecycle
audit/checkpoint writes are accounted separately; each recovered result is
authorized and revalidated against canonical state before use, and unavailable
authority never becomes approval from the projection. Check throughout faults,
rollback, and restart, not only after a successful rebuild.
Fault/timing angle: Corruption, missing state, five kinds of contract mismatch,
canonical unavailability, and canonical invalidation after snapshot S.
Required faults and enabling state: A usable but stale projection that would
disagree with canonical state, one deliberate identity mismatch at a time,
and canonical reads made unavailable. Keep a separate ledger of intended
canonical operations, including authorized domain-name `operator_remediation`,
so in-place legitimate changes are not false positives or undone by recovery.
Confidence: medium - [evidence](evidence/recovery-preserves-canonical-authority.md).
Canonical reader and eligibility policy exist; the recovery-specific production
composition is absent and therefore classified `test-only`.
Existing check: `crates/kernel/tests/kernel_outbox.rs:333-414` checks that an
alignment discard preserves checkpoints and receipts; status `unaudited`.
The existing clean/perturbed model compares canonical digests at
`crates/kernel/tests/kernel_proofs/model.rs:183-245`; status `unaudited`.
Existing withheld-rendering coverage is reused, not copied. None covers the
proposed search recovery mutation set.
Impact: Derived corruption becomes durable truth or admits retracted content.
Open questions:

- Which canonical tables and authorized control mutations define the recovery
  oracle's comparison boundary? (needs human input)
- How will recovery use the planned shared eligibility authority while the
  existing `judge` remains daemon-owned? (needs human input)
- Who owns the at-rest sensitivity policy for exported/staged/retained derived
  bytes, including remediated fields if projected? No such policy is created
  by this catalog. (needs human input)

## Relationship and handoff map

| Property | Shared mechanism and relationship | Handoff |
| --- | --- | --- |
| export-fixed-s-exactly-once | Supplies `E(S)` to catch-up; needs retention and agreed class/tombstone oracle. | `/testing:test-strategy`; source-coverage owner defines input mapping. |
| export-retention-fence-covers-read | Protects exact export and rebuild; outbox retention alone does not imply byte retention. | `/testing:test-strategy`, then `/testing:deterministic-simulation-testing` for prune/GC timing. |
| export-predecode-bounds | Independent of semantic exactness; size queries are a reuse seam. | `/testing:test-strategy`; RP2.9 supplies accounting/limits. |
| catchup-complete-commit-prefix | Completes export at T; relies on row-owner atomic application but is not implied by monotonic ack. | `/testing:test-strategy`, `/testing:deterministic-simulation-testing`; projection-row owner supplies apply oracle. |
| ack-follows-local-release | Sole owner of ack-after-local-durability, lock ownership, and ack-loss checks/markers; consumes row/checkpoint/job atomicity. | `/testing:test-strategy`, `/testing:deterministic-simulation-testing`; projection owner references this record and markers rather than defining them. |
| replacement-selects-complete-compatible-state | Adds search completeness to existing generic selector validity; consumes export and catch-up. | `/testing:test-strategy`; publication owner reuses lifecycle machinery. |
| rebuild-after-pruning-converges | Composes export, complete catch-up, ack, and publication under a finite healthy window. None alone implies convergence. | `/testing:test-strategy`, `/testing:deterministic-simulation-testing`; RP2.9 approves bound. |
| catchup-and-authorized-recovery-converge | Covers ordinary backlog progress and explicitly authorized Disabled recovery; deletion-after-pruning stays the separate rebuild case. | `/testing:test-strategy`; RP2.9 owns per-mode bounds; projection-coverage owns source inventory/sweep obligations. |
| disable-preserves-consumer-obligations | Shares retention/barrier mechanisms; exact existing withheld rendering is linked. | `/testing:test-strategy`; daemon lifecycle owner resolves pending disable. |
| recovery-preserves-canonical-authority | Applies across all recovery paths; compatible selection alone does not prove canonical permission. | `/testing:test-strategy`; canonical/eligibility owner defines authoritative comparison. |

All referenced test checks remain `unaudited`; adequacy goes to
`/testing:invariant-test-review`. Runtime guard enforcement goes to
`/low-level-systems:defensive-assertions-and-invariant-guards`. These are routing
recommendations, not chosen test forms or implementation authorization.

Exact reuse also includes
[validation-and-enumeration-address-one-directory-object](../../host-runtime/catalog.md#validation-and-enumeration-address-one-directory-object).
Removed mirror/effect catalogs are historical leads only, including
[mirror-reset-cycle-requires-a-rebuild-grant](../../memory-store/catalog.md#mirror-reset-cycle-requires-a-rebuild-grant)
and
[facade-a-claim-effects-ack-and-producer-checkpoint-advance-are-never-composed](../../daemon/facade/catalog.md#facade-a-claim-effects-ack-and-producer-checkpoint-advance-are-never-composed).
No new record duplicates their removed subjects.

Source inventory, message cleanup, git sweeps, and their coverage remain with
[projection-source-inventory-complete](../projection-coverage/catalog.md#projection-source-inventory-complete).
If approved projection mapping consumes a remediated domain-name field, reuse
[projection-remediation-invalidates-derived-bytes](../projection-coverage/catalog.md#projection-remediation-invalidates-derived-bytes).
The existing remediation path does not prove that dependency or mandate a new
occurrence generation. The shared campaign record
[projection-acceptance-situations-witnessed](../projection-coverage/catalog.md#projection-acceptance-situations-witnessed)
requires every declared fault-map marker. Definitions stay in this part's
[fault map](fault-map.md#independent-situation-markers), including the two
canonical ack-loss markers; other parts reference them without redefining them.

## Coverage and evaluation status

Ten active records, ten index rows, and ten evidence files. Type distribution:
eight safety, two bounded liveness. All ten use `always`; liveness is evaluated
per admitted episode and remains visibly RP2.9-blocked. Independent
`sometimes` situation markers are specified in [fault-map.md](fault-map.md).
No forbidden-state claim uses `unreachable`. No marker is the negation of an
invariant, and no marker alone proves the associated guarantee.

Model and property lenses, including wildcard last, are preserved under
[_lenses](_lenses/01-system-model.md). Relevant existing catalogs were read
before writing. The original nested assessment attempt hit the depth limit;
its same-context trace remains as provenance. Independent central analyst
`ses_f7623dcccffe3Y09nW2wVoABif` has completed harness fit, coverage balance,
implementability, and wildcard lenses. [Portfolio dispositions](portfolio-evaluation.md)
record the applied corrections and remaining owner/implementation questions.
This author's disposition is not an independent re-review or implementation proof.
