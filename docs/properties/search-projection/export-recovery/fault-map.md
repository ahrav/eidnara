# Fault map: RP2.1 export and recovery

Repository: `/local/home/ahrav/scratch/eidnara`.
Verified HEAD: `913234433ae36a80a6e22c6aac14c7f9aab74386`.
Date: 2026-09-10. External scope is supplied by the user: plan, linked
parent/index/research, and local repo; no incident logs are supplied.
[Sources and why consulted](catalog.md#sources) apply here.

This is a proposed fault/observation contract. No campaign ran, and no test
form is chosen. All new catalog records remain `Exercised: not yet`.

## Fault availability at HEAD

| Fault or situation | Available substrate | Missing export/recovery capability |
| --- | --- | --- |
| Concurrent inserts, corrections, deletions | Real kernel commits and historical reads. | Hooks between proposed pages at one S. |
| In-place domain-name remediation | `operator_remediation` changes `domains.name` without changing source revision. | Approved source dependency and valid-S/abort observation for mutable bytes. |
| Outbox prune while consumer is held | Registration, ack, and `prune_outbox` exist. | Export fence lifecycle and validity observation. |
| Source reclamation or forced purge | Artifact GC/deletion and capture-pin logic exist. | Export-source pin/abort contract across every source class. |
| Oversize row and page budget boundary | Decision payload-size lookup; separate perf example counts cumulative allocation requests. | Pre-materialization admission and a permitted live decoded-heap high-water observer. |
| Split, published, duplicate, or empty commits | Kernel outbox fixtures expose these lower mechanisms. | Bounded consumer-specific replay and complete-prefix observer. |
| Blocked writer acquisition | Kernel mutex and explicit ack operation exist. | Search lock ownership and release/ack timing hooks. |
| Process death after local COMMIT or ack | Existing CAS child/barrier/kill/reap pattern at `crates/kernel/tests/cas_fault_injection.rs:1046-1094`, status unaudited. | Concrete search COMMIT/ack/selector hooks and reopened search-state oracle. |
| Selector I/O failure and crash residue | Host generation fixtures and filesystem operations exist. | Search publication unit, durability fault model, and full boundary hooks. |
| Missing database after pruning | Kernel pruning and canonical source are available. | Actual search database and rebuild supervisor. |
| Pending disable and recorded deletion barrier | Kernel lifecycle methods and serving decisions exist. | Retrieval disable-state persistence and operator recovery protocol. |
| Ordinary backlog and authorized recovery progress | Manual ack/serving fixture exists. | CatchingUp/Disabled-to-Current controller, authorization/gate observations, RP2.9-approved per-mode bounds. |
| Mismatch or unavailable canonical authority | Existing verdict/read failure surfaces exist. | Search compatibility identity and recovery-to-egress integration. |

Artifact-GC test hooks are specific to GC. They are not export-fence hooks.
Manually created temporary files are not process termination. Process death
does not demonstrate power-loss durability; any later power-loss claim needs
its own storage model and evidence.

The existing CAS suite explicitly limits its evidence to process crash and
injected errors (`crates/kernel/tests/cas_fault_injection.rs:1-6`). Reuse its
bounded child/barrier/kill/reap pattern; the missing work is search-specific
hooks and oracles, not a new broad crash harness. Its child-barrier timeout is
not a recovery acceptance bound.

The allocator pattern at `crates/host-runtime/examples/perf_host.rs:5-33`
uses unsafe `GlobalAlloc` and cumulative requested-byte counters, not live
decoded-heap accounting. Kernel forbids unsafe code
(`crates/kernel/src/lib.rs:5`). No permitted export decoder/high-water observer
is identified. Logical admission charges are a possible implementation
approach, not proof that physical heap is bounded. Observation-boundary choice
stays open; this catalog prescribes no allocator or unsafe dependency.

Reuse the existing `Proof` clean-restart/fault/replay fixture and kernel
operation model before creating fixture machinery. Their limits are explicit:
`crates/kernel/tests/kernel_proofs/harness.rs:1-13` rules out crash-durability
evidence, and `crates/kernel/tests/kernel_proofs/model.rs:95-120` tracks only
three object kinds. The table-wise digest at
`crates/kernel/tests/support/canonical_state.rs:1-20` is an authority-comparison
seam, with normalization that must be reviewed for each oracle.

## Per-property prerequisites and observations

| Property | Required situation/fault | Oracle and required markers |
| --- | --- | --- |
| [rp21-export-fixed-s-exactly-once](catalog.md#rp21-export-fixed-s-exactly-once) | Multiple pages, mutations between pages, class boundary, historical invalidation, in-place domain-name remediation. | Fixture ledger `E(S)`, exact key/byte comparison or abort when required S bytes are unavailable; `rp21_export_writes_between_pages`, `rp21_export_operator_remediation_during_snapshot`. |
| [rp21-export-retention-fence-covers-read](catalog.md#rp21-export-retention-fence-covers-read) | Prune under valid fence and lost source/history coverage during build. | Durable consumer/fence witness, required-source availability, selector exclusion, cleanup outcome; `rp21_export_prune_with_fence`, `rp21_export_retention_lost_mid_build`. |
| [rp21-export-predecode-bounds](catalog.md#rp21-export-predecode-bounds) | Over-cap row, residual page-byte edge, exact boundary, JSON expansion. | Size admission before materialization, decoder entries, separately observed live decoded heap; `rp21_export_oversize_row_present`, `rp21_export_page_budget_edge`. |
| [rp21-catchup-complete-commit-prefix](catalog.md#rp21-catchup-complete-commit-prefix) | Split commit, published retained rows, duplicate delivery, empty commit, `operator_remediation`. | Complete canonical transaction ledger, per-ordinal application, durable prefix; `rp21_catchup_split_commit`, `rp21_catchup_published_retained`, `rp21_catchup_empty_commit`, `rp21_export_operator_remediation_during_snapshot`. |
| [rp21-ack-follows-local-release](catalog.md#rp21-ack-follows-local-release) | Crash after local commit but before ack, contended kernel writer, lost ack response. | Ownership event order and independent durable reads; `rp21_ack_local_commit_interrupted`, `rp21_ack_writer_contended`, `rp21_ack_response_lost`. |
| [rp21-replacement-selects-complete-compatible-state](catalog.md#rp21-replacement-selects-complete-compatible-state) | Reader during old/new switch, partial staging, selector failure/restart. | One selected identity per read, `O(T)`, compatible tuple, consumer-release order; `rp21_replacement_switch_interrupted`. |
| [rp21-rebuild-after-pruning-converges](catalog.md#rp21-rebuild-after-pruning-converges) | Search deletion after confirmed historical pruning, finite concurrent work then quiet window. | Independent canonical `O(T)`, stable T, approved recovery/work bounds; `rp21_rebuild_missing_db_after_prune`, `rp21_rebuild_quiet_window_admitted`. |
| [rp21-catchup-and-authorized-recovery-converge](catalog.md#rp21-catchup-and-authorized-recovery-converge) | Healthy finite normal backlog plus a separate explicitly authorized Disabled recovery with every gate accepted. | Per-admitted-episode Current/coverage/checkpoint observation within RP2.9 bounds; `rp21_healthy_lagged_target_admitted`, `rp21_authorized_recovery_current_observed`. |
| [rp21-disable-preserves-consumer-obligations](catalog.md#rp21-disable-preserves-consumer-obligations) | Lagging disable, last/non-last removal, outstanding deletion barrier, restart. | Consumer rows, audit records, serving outcomes per surface; `rp21_disable_pending_consumer`, `rp21_disable_last_consumer`, `rp21_abandon_request_with_barrier`. |
| [rp21-recovery-preserves-canonical-authority](catalog.md#rp21-recovery-preserves-canonical-authority) | Stale valid projection, each identity mismatch, canonical unavailability, authorized remediation. | Independent canonical mutation ledger and final verdicts; `rp21_recovery_stale_authority`, `rp21_recovery_contract_mismatch`, `rp21_recovery_authority_unavailable`. |

## Independent situation markers

Each marker below has `sometimes` semantics per declared campaign. Names are
constant, lowercase, and unique. They record preconditions, injected boundaries,
and an authorized recovery outcome, not violations or negations of safety
checks. A marker can fire on a correct
implementation. Cases with multiple markers require every listed marker in
their campaign contract; one does not substitute for another.

| Marker | Exact witness condition |
| --- | --- |
| `rp21_export_writes_between_pages` | One export has returned a nonfinal page, and a canonical mutation affecting its source domain commits before a later page request at the same S. |
| `rp21_export_operator_remediation_during_snapshot` | An export is active at S when a domain-name `operator_remediation` commits after S with unchanged source revision, before its catch-up target T is fixed. The fixture records whether approved export mapping consumes that field. |
| `rp21_export_prune_with_fence` | A durable bootstrap registration exists, unread export input remains, and an outbox prune is attempted before export completion. |
| `rp21_export_retention_lost_mid_build` | An incomplete candidate exists and the harness invalidates its retention witness or removes required source bytes before the next export/catch-up/publication decision. |
| `rp21_export_oversize_row_present` | The fixture ledger has a row larger than the declared standalone cap and export requests the page whose next key is that row. |
| `rp21_export_page_budget_edge` | A page has admitted input, and its next row fits alone but exceeds remaining row/byte capacity. |
| `rp21_catchup_split_commit` | A commit with multiple ordinals after S exceeds the declared batch cut; the first partial batch is delivered and remaining ordinals are available. |
| `rp21_catchup_published_retained` | An event after S is marked published, remains stored above the consumer checkpoint, and the consumer requests catch-up through its commit. |
| `rp21_catchup_empty_commit` | A commit-log entry with zero outbox rows lies after S and at or before requested T. |
| `rp21_ack_local_commit_interrupted` | The observer records a batch's successful local COMMIT, then the process is terminated before its kernel ack is attempted. |
| `rp21_ack_writer_contended` | Another operation holds the kernel writer when a locally committed batch attempts its ack. |
| `rp21_ack_response_lost` | Durable kernel ack COMMIT is observed and the harness loses its response before the consumer observes completion. |
| `rp21_replacement_switch_interrupted` | A selected old projection and a staged candidate exist, a read is requested during switching, and execution is interrupted at a declared selector boundary. |
| `rp21_rebuild_missing_db_after_prune` | Independent reads prove pre-S outbox history was pruned, all local search state is removed, and retained canonical source remains nonempty when recovery starts. |
| `rp21_rebuild_quiet_window_admitted` | After a fault/load phase, an episode starts with a fixed finite T, retained inputs within approved caps, available dependencies, and injection/new-write pressure stopped for the approved window. |
| `rp21_healthy_lagged_target_admitted` | An ordinary CatchingUp episode is admitted with local complete prefix below finite T, healthy stores/workers, retained inputs, all prerequisite/gate acceptances, approved RP2.9 caps/bound, and injection/new writes stopped for the interval. |
| `rp21_authorized_recovery_current_observed` | A previously Disabled controller has recorded explicit recovery authorization and all prerequisite/gate acceptances, admitted a healthy finite-target episode, and is subsequently observed Current through that episode's T. This positive outcome is observed independently of the deadline assertion. |
| `rp21_disable_pending_consumer` | Disable is requested with retrieval's checkpoint below the kernel pre-operation tip. |
| `rp21_disable_last_consumer` | A safely caught-up retrieval consumer is the only registered consumer when its deregistration is requested. |
| `rp21_abandon_request_with_barrier` | An authorized abandonment request names a consumer recorded by an incomplete deletion barrier. |
| `rp21_recovery_stale_authority` | A valid old projection names an occurrence subsequently invalidated or revised canonically before a recovery/egress attempt. |
| `rp21_recovery_contract_mismatch` | A persisted projection tuple differs from the active tuple on a selected schema, tokenizer, model, policy, or identity dimension when opened. |
| `rp21_recovery_authority_unavailable` | Recovery or egress is attempted while the canonical authority read is deliberately failed. |

This table is the single definition site for all 23 markers, including
`rp21_ack_local_commit_interrupted` and `rp21_ack_response_lost`. The projection
part references these ack markers without defining alternatives. Its first-class
[projection-acceptance-situations-witnessed](../projection-coverage/catalog.md#projection-acceptance-situations-witnessed)
record requires every declared marker. Preserve these witness conditions when
wiring that campaign; an authorized Current outcome never excuses missing
ordinary healthy-backlog admission or a missed per-episode deadline.

Domain-name remediation is an actual control event, not proof that RP2.1
projects domain names. If the approved mapping consumes that field, the export
must supply its required S bytes or abort and the projection owner applies
[projection-remediation-invalidates-derived-bytes](../projection-coverage/catalog.md#projection-remediation-invalidates-derived-bytes).
If it does not, account for the event without inventing a byte dependency,
source revision change, or new occurrence generation.

For contract mismatch, report each dimension separately under this constant
marker with a bounded enum value. Do not generate marker names dynamically.
For switch/crash coverage, retain the declared boundary as bounded scenario
metadata and require the campaign's whole boundary list, not one lucky hit.

## Recovery bound and non-vacuity

RP2.9 must approve `B_recovery_ms`, finite rows/bytes/commits, attempt/work caps,
the retained-source envelope, and fence lifetime before the positive recovery
episode is executable. These are oracle parameters, not existing fields.
The interval starts after the fault/load phase ends and includes the declared
reopen/bootstrap/catch-up/select work. Whether ack reconciliation uses that
same interval remains an explicit decision in the liveness record.

Ordinary catch-up and authorized Disabled recovery use their separately
approved `B_catchup_ms` and `B_authorized_recovery_ms` episode envelopes. Both
are RP2.9-blocked. Admission observes authorization where required and every
prerequisite/gate independently of progress. Failed or absent admission leaves
the feature unavailable; this liveness property does not authorize enabling it.
Source inventory and sweep completion, including message cleanup and git work,
remain dependencies of
[projection-source-inventory-complete](../projection-coverage/catalog.md#projection-source-inventory-complete).

Do not reset t0 after each retry or advance T indefinitely. Inspect safety
during the fault phase; inspect convergence by the fixed bound. A source-loss
episode must exercise abort safety, not be counted as a successful rebuild.
Disabled retrieval is not convergence in an admitted healthy episode.

An unfired marker first triggers precondition review. Then distinguish a
workload/injection gap from a genuinely unreachable required situation. It is
not evidence that the associated safety property holds, and a finite missing
witness is not a proof about unbounded formal liveness.

Ack outcomes use a per-consumer, per-candidate ledger: attempted values,
observed responses, and durable kernel/local checkpoints. Repeated ack calls
collapse into one monotonic checkpoint; exact request/effect counts are wrong.
Similarly, replay of one canonical commit can be attempted repeatedly while
its logical application remains singular.

## Leverage ranking by cheapest valid oracle

| Rank | Seam | Why start here | Routing |
| ---: | --- | --- | --- |
| 1 | Kernel fixed-S reader plus independent small canonical ledger | Extend existing `kernel_proofs` fixtures/model where applicable; add the missing all-class identity/byte oracle. Direct equality can discriminate omissions before a full daemon exists. | `/testing:test-strategy` chooses the form. |
| 2 | Page admission before materialization | Decoder-entry observation separates pre-decode refusal from late truncation; a permitted physical decoded-heap observer remains missing. Logical charge alone cannot clear the heap claim. | `/testing:test-strategy`; RP2.9 supplies units and observation approval. |
| 3 | Canonical transaction ledger and consumer cursor | Published retained rows, empty commits, and split commits expose API gaps cheaply. | `/testing:test-strategy`; row owner supplies atomic apply contract. |
| 4 | Local transaction release versus kernel writer acquisition | Deterministic ownership events expose prohibited overlap without flaky wall-clock deadlock detection. | `/testing:test-strategy`, then `/testing:deterministic-simulation-testing`. |
| 5 | Real-kernel lifecycle and serving adapter | Existing fixtures already distinguish lag/no-consumer behavior and auditable removal. | `/testing:invariant-test-review` for reuse, then `/testing:test-strategy`. |
| 6 | Selector/reopen and deletion-after-pruning process boundary | Reuse the existing CAS child/barrier/kill/reap pattern. Cost lies in concrete search hooks, publication unit, reopened-state oracle, and approved window, not a broad new harness. | `/testing:test-strategy`, then `/testing:deterministic-simulation-testing`. |
| 7 | Normal and authorized recovery progress plus canonical egress | Reuse lower oracles and existing process-control patterns; add per-episode target/authorization/gate observations. | `/testing:test-strategy`; RP2.9 owns numeric bounds. |

No numeric ranking of performance or claimed coverage percentage is implied.
Independent central review is complete under analyst session
`ses_f7623dcccffe3Y09nW2wVoABif`. See [dispositions](portfolio-evaluation.md)
for applied corrections and remaining owner/implementation questions.
