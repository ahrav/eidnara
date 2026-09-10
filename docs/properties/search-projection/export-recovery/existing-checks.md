# Existing checks: RP2.1 export and recovery

Repository: `/local/home/ahrav/scratch/eidnara`.
Verified HEAD: `913234433ae36a80a6e22c6aac14c7f9aab74386`.
Date: 2026-09-10. External scope is supplied by the user: plan, linked
parent/index/research, and local repo; no incident logs are supplied.
[Sources and why consulted](catalog.md#sources) apply to this inventory.

Every check below has status **unaudited**. Inspection locates a guard or a
claim-bearing assertion; it does not establish adequacy or a passing run.
No tests ran. Return errors are guard behavior, not assertion messages.
Test assertions use standard mismatch diagnostics unless a message is quoted.
Tests from removed mirror/effect modules are not counted as existing checks.
Independent central review is complete under
`ses_f7623dcccffe3Y09nW2wVoABif`; [dispositions](portfolio-evaluation.md) separate
catalog corrections from remaining implementation and observation gaps.

## Scope survey

Sandbox counts are physical lines and literal assertion-bearing lines, not
coverage or assertion-quality scores.

| Focal file | Lines | Assertion-bearing lines | `#[test]` attributes |
| --- | ---: | ---: | ---: |
| `crates/kernel/src/slice/read.rs` | 355 | 0 | 0 |
| `crates/kernel/tests/kernel_slice.rs` | 1,627 | 112 | 20 |
| `crates/kernel/tests/kernel_outbox.rs` | 698 | 56 | 14 |
| `crates/daemon/src/kernel_routes/serving.rs` | 225 | 16 | 5 |
| `crates/host-runtime/src/generation.rs` | 2,327 | 104 | 28 |

Kernel source has 32 Rust files and 28,239 lines; daemon source has 56 Rust
files and 115,249 lines. Discovery follows the focal call boundaries rather
than claiming an exhaustive audit of both crates. All 14 outbox tests are
inventoried below; other tables include checks bearing directly on this domain.

## Production guards and invariants

| Location | Check and observable behavior | Relevance | Status |
| --- | --- | --- | --- |
| `crates/kernel/src/outbox.rs:79-112` | Registration validates identity/time; derives initial checkpoint from oldest retained commit or pre-operation tip; duplicate insert is a conflict. | Retention starts from existing rows, not a caller-invented S. | unaudited |
| `crates/kernel/src/outbox.rs:133-156` | Checkpoint below pre-operation tip returns `ConsumerPending`; otherwise completes satisfied barriers before deletion. | Disable and old-consumer release. | unaudited |
| `crates/kernel/src/outbox.rs:177-205` | Empty operator/reason is `InvalidInput`; a named barrier must record the consumer. | Explicit abandonment authorization facts. | unaudited |
| `crates/kernel/src/outbox.rs:215-283` | Writes abandonment records for blocked barriers before deleting consumer and completing barriers. | Auditable lifecycle transition. | unaudited |
| `crates/kernel/src/outbox.rs:303-355` | Explicit empty-consumer barrier abandonment rejects barriers with recorded consumers. | Last-consumer and deletion semantics. | unaudited |
| `crates/kernel/src/outbox.rs:432-477` | Zero limit is `InvalidInput`; query marks true commit boundary independently of page limit. | Publisher paging, not consumer-complete replay. | unaudited |
| `crates/kernel/src/outbox.rs:493-517` | New publication positions must be commit boundaries; repeated positions at/below persisted watermark are idempotent. | Separate publication-position semantics. | unaudited |
| `crates/kernel/src/outbox.rs:536-570` | Ack validates identity/time, writer fence, registration, monotonicity, and existing commit; failure is typed. | Does not check local search durability or applied ordinals. | unaudited |
| `crates/kernel/src/outbox.rs:578-601` | Inclusive minimum checkpoint horizon; empty set returns `NoRequiredConsumers`. | Retention and disabled last consumer. | unaudited |
| `crates/kernel/src/outbox.rs:621-655` | Persist satisfied consumer acknowledgement; complete barriers only after required checkpoints or qualifying abandonment. | Replacement cannot erase old barrier obligations. | unaudited |
| `crates/kernel/src/envelope.rs:395-438` | `remediate_text_inner` validates a domain target, updates `domains.name` in place, and appends `operator_remediation` with the existing object/revision. | Mutable canonical bytes can invalidate required S input; projected dependency is conditional on source mapping. | unaudited |
| `crates/kernel/src/slice/read.rs:177-191` | Reject negative or future snapshot. | Does not check an export retention lease. | unaudited |
| `crates/kernel/src/slice/read.rs:245-295` | Size query reads stored lengths; row decoder classifies invalid JSON. | Reusable metadata read, not page memory admission. | unaudited |
| `crates/kernel/src/slice/read.rs:344-354` | JSON conversion error becomes `CorruptCanonicalRow`; other row errors use SQLite classification. | Failed decode must not look like empty export. | unaudited |
| `crates/kernel/src/open.rs:410-418` | Refuse poisoned restore before and after writer lock acquisition. | Ack locking and recovery authority. | unaudited |
| `crates/kernel/src/cas/gc.rs:437-534` | Separate artifact reclaim checks for purge, references, pins, reservations, and grace periods. | No evidence this is an export retention fence. | unaudited |
| `crates/daemon/src/kernel_routes/serving.rs:19-63` | Inclusive lag thresholds; zero consumers unavailable for gated reads but available for canonical tip reads. | Pause/deregister surface distinction. | unaudited |
| `crates/daemon/src/kernel_routes/serving.rs:68-100` | Lag is stale on explicit search and abstained on automatic surfaces. | Existing withheld semantics are reused. | unaudited |
| `crates/daemon/src/canonical_memory.rs:147-173` | Store, tip, lag, or visible-read failures return a withheld canonical read. | Authority unavailability is not emptiness. | unaudited |
| `crates/daemon/src/kernel_routes/eligibility.rs:132-173` | Checks retraction, supersession, revision, scope, sensitivity, visibility, and artifact eligibility. | Existing authority policy, not proposed kernel eligibility API. | unaudited |
| `crates/host-runtime/src/generation.rs:438-546` | Profile decoding and generation validation reject malformed/missing/inconsistent files; unknown schema quarantines. | Generic publication check reused by link. | unaudited |
| `crates/host-runtime/src/generation.rs:568-628` | Stages/promotes before profile replacement and checks named directory identity. | Payload-generation precedent only. | unaudited |
| `crates/host-runtime/src/generation.rs:848-911` | Prune protects selected/supplied digests and preserves unknown schemas. | Generic pruning; no search consumer handoff. | unaudited |

## Kernel outbox claim-bearing tests

All locations in this table are in `crates/kernel/tests/kernel_outbox.rs`.
The file is gated on `test-support` at line 5. The property catalog does not
infer test execution merely from a workspace test command in a plan.

| Location | Test and asserted contract | Status |
| --- | --- | --- |
| `crates/kernel/tests/kernel_outbox.rs:50-83` | `consumer_insert_maps_only_constraint_failures_to_conflict`: duplicate is `Conflict`, missing table is `Io`. | unaudited |
| `crates/kernel/tests/kernel_outbox.rs:86-153` | `acknowledgements_use_commit_boundaries_through_commit_log_tip`: repeated ack, empty commit, rejection of regression/beyond-tip; direct SQLite checkpoint read. | unaudited |
| `crates/kernel/tests/kernel_outbox.rs:156-224` | `slow_consumer_sets_commit_prune_horizon_and_registration_sees_oldest_retained_commit`: exact horizon, surviving rows, redaction cleanup, preserved receipt, late registration start. | unaudited |
| `crates/kernel/tests/kernel_outbox.rs:227-244` | `empty_required_set_refuses_prune_and_empty_outbox_registration_uses_pre_registration_tip`: typed refusal and initial checkpoint. | unaudited |
| `crates/kernel/tests/kernel_outbox.rs:247-330` | `deregistration_uses_commit_tip_without_publication_and_abandonment_records_four_facts`: pending refusal, successful caught-up removal, durable audit facts/payload. | unaudited |
| `crates/kernel/tests/kernel_outbox.rs:333-414` | `derived_projection_discard_preserves_exact_checkpoints_and_receipts`: clear internal alignment rows, preserve checkpoints/receipts; not deletion of `search.sqlite`. | unaudited |
| `crates/kernel/tests/kernel_outbox.rs:417-434` | `publication_boundary_and_fence_checks_remain_enforced`: mid-commit publication is `InvalidCheckpoint`, invalidated writer is `FenceLost`. | unaudited |
| `crates/kernel/tests/kernel_outbox.rs:437-481` | `publication_marks_rows_through_the_position_and_leaves_later_rows_unpublished`: exact positions/timestamps and monotonic watermark. | unaudited |
| `crates/kernel/tests/kernel_outbox.rs:484-516` | `publication_stays_idempotent_after_the_rows_are_pruned`: retry publication after deletion preserves surviving row boundary. | unaudited |
| `crates/kernel/tests/kernel_outbox.rs:519-531` | `a_consumer_id_that_redaction_rewrites_is_rejected`: `InvalidInput` instead of consumer identity rewrite. | unaudited |
| `crates/kernel/tests/kernel_outbox.rs:534-550` | `argument_validation_is_separate_from_checkpoint_rejection`: invalid arguments and absent consumer remain distinct errors. | unaudited |
| `crates/kernel/tests/kernel_outbox.rs:553-575` | `every_outbox_and_retention_entry_point_checks_the_writer_fence`: each invoked write entry returns `FenceLost`. | unaudited |
| `crates/kernel/tests/kernel_outbox.rs:578-619` | `a_lookup_id_that_redaction_rewrites_cannot_alias_onto_another_consumer`: ack/deregister refuse alias; stored checkpoint stays zero. | unaudited |
| `crates/kernel/tests/kernel_outbox.rs:622-698` | `pending_outbox_reads_unpublished_rows_in_order_with_commit_boundaries`: ordered shape, payload identity, limit cut and publication filtering; message: `position 1 is mid-commit`. | unaudited |

## Snapshot and source-retention checks

| Location | Check and semantics | Status |
| --- | --- | --- |
| `crates/kernel/tests/kernel_envelope.rs:246-289` | Historical lookup masks future invalidation and exposes known correction/deletion metadata at later S. | unaudited |
| `crates/kernel/tests/kernel_slice.rs:466-559` | Requested live IDs at two snapshots; empty/large ID sets and negative/future snapshot rejection. | unaudited |
| `crates/kernel/tests/kernel_slice.rs:562-609` | Live decision size equals serialized stored payload size; empty IDs and future snapshot cases. | unaudited |
| `crates/kernel/tests/kernel_slice.rs:612-735` | Corrections preserve old rows and update dependency/invalidation facts; reject dead or unbumped replacements. | unaudited |
| `crates/kernel/tests/kernel_alignment.rs:621-642` | Corrupt decision JSON returns `CorruptCanonicalRow`; message: `a corrupt decision payload was reported as a transient storage fault`. | unaudited |
| `crates/kernel/tests/kernel_alignment.rs:645-672` | Corrupt observation JSON is classified consistently by slice/alignment; message: `slice and alignment reads disagree about canonical corruption`. | unaudited |
| `crates/kernel/tests/kernel_retention.rs:316-391` | Staging cleanup completes with halted consumer while exact outbox bytes/checkpoint remain unchanged. | unaudited |
| `crates/kernel/tests/kernel_retention.rs:395-466` | Two live domain names become the placeholder; audit fields and caller receipt are retained. Does not prove RP2.1 projects those names. | unaudited |
| `crates/kernel/tests/kernel_retention.rs:743-814` | Retired domain names can be remediated; repeat remediation appends another audit event. | unaudited |
| `crates/kernel/tests/kernel_retention.rs:817-850` | Missing target returns `NotFound`; empty operator returns `InvalidInput`. | unaudited |
| `crates/kernel/tests/kernel_deletion.rs:212-292` | A deletion captures its consumer set; later registration is not retroactive; empty recorded set needs explicit abandonment. | unaudited |
| `crates/kernel/tests/kernel_deletion.rs:296-350` | Barrier-specific abandonment preserves operator, barrier, and time. | unaudited |
| `crates/kernel/tests/kernel_deletion.rs:802-856` | All barriers blocked by an abandoned consumer clear; message: `barrier {barrier} stayed blocked`. | unaudited |
| `crates/kernel/tests/kernel_deletion.rs:860-897` | Caught-up deregistration leaves its deletion barrier completed. | unaudited |
| `crates/kernel/tests/kernel_deletion.rs:1130-1166` | Cleared barrier retains satisfied consumer acknowledgement after removal; message: `a cleared barrier reported its acknowledged consumer as unsatisfied`. | unaudited |

## Serving and publication checks reused from existing catalogs

| Location | Check and semantics | Status |
| --- | --- | --- |
| `crates/daemon/src/kernel_routes/serving.rs:116-121` | Thresholds are inclusive; absent lag does not trip them. | unaudited |
| `crates/daemon/src/kernel_routes/serving.rs:124-160` | No consumers takes precedence over thresholds, including missing age. | unaudited |
| `crates/daemon/src/kernel_routes/serving.rs:163-176` | Tip and gated policies diverge only without a consumer. | unaudited |
| `crates/daemon/src/kernel_routes/serving.rs:179-206` | Explicit stale versus automatic abstention; available maps identically. | unaudited |
| `crates/daemon/src/kernel_routes/serving.rs:209-224` | Unavailability names its reason only on explicit search. | unaudited |
| `crates/daemon/tests/transform_canonical_memory.rs:230-302` | Real daemon/kernel fixture withholds memory under lag and restores it after ack. | unaudited |
| `crates/host-runtime/src/generation.rs:1241-1272` | Stage, validate, read selected digest, and repeat same-digest stage. | unaudited |
| `crates/host-runtime/src/generation.rs:1430-1556` | Tampered generation shapes are rejected. | unaudited |
| `crates/host-runtime/src/generation.rs:1559-1611` | Unknown manifest/profile schema is quarantined; mutation is refused; message: `quarantined bytes must be preserved exactly`. | unaudited |
| `crates/host-runtime/src/generation.rs:1801-1854` | Current and caller-protected digests survive prune; unprotected old generation is removed. | unaudited |
| `crates/host-runtime/src/generation.rs:1857-1896` | Protected corrupt target refuses repair; unprotected same-digest target can be repaired. | unaudited |
| `crates/host-runtime/src/generation.rs:1899-1926` | Manually created partial staging/profile temp leaves old profile selected. This constructs residue, not actual crash execution. | unaudited |

## Existing model and canonical-digest seams

The kernel proof suite supplies reusable fixture/model infrastructure. It is
not a search recovery harness. Its own fault-domain statement explicitly
limits restart to a clean close after transaction rollback
(`crates/kernel/tests/kernel_proofs/harness.rs:1-13`).

| Location | Check and semantics | Status |
| --- | --- | --- |
| `crates/kernel/tests/kernel_proofs/harness.rs:118-143` | Replay closure panics if re-executed; fault hook checks `KernelError::Fault` before rollback/digest comparison. Message: `replayed intent re-executed its operation`. | unaudited |
| `crates/kernel/tests/kernel_proofs/harness.rs:175-183` | Canonical digest equality before and after clean reopen; messages: `fault left canonical state changed`, `rolled-back state did not survive restart`. | unaudited |
| `crates/kernel/tests/kernel_proofs/model.rs:183-245` | Clean and perturbed roots agree; duplicate replay preserves digest; both run the object reference model. Message: `duplicate changed canonical state`. | unaudited |
| `crates/kernel/tests/kernel_proofs/obligations/o10_idempotency.rs:16-34` | Runs 32 fixed-seed generated histories with 8-24 steps through that model. This sampling is not an RP2.9 bound. | unaudited |
| `crates/kernel/tests/kernel_proofs/obligations/o5_correction.rs:234-278` | Earlier live/history/decision snapshots and typed content remain equal after corrections, with a positive successor-content control. Message: `correction rewrote the typed content of {id}`. | unaudited |
| `crates/kernel/tests/kernel_proofs/obligations/o2_atomic_repair.rs:178-209` | Two observations share one commit, retain duplicate-sensitive vector equality, are absent at the earlier snapshot, and survive clean reopen. | unaudited |
| `crates/kernel/tests/kernel_proofs/obligations/o6_deletion.rs:230-324` | Deletion invalidates exact reference IDs, emits every propagation target with barrier/digest/affected IDs, and leaves the recorded consumer barrier unsatisfied. Message: `{kind} payload lost the affected object ids`. | unaudited |
| `crates/kernel/tests/kernel_proofs/obligations/o8_restart_backup.rs:323-378` | Prune actually removes rows; new positions exceed prior high water after clean reopen, and writer epoch advances. Message: `positive control: prune removed rows`. | unaudited |
| `crates/kernel/tests/support/canonical_state.rs:1-20` | Existing table-wise digest provides SameRoot and CrossRoot comparison profiles; normalization is part of the oracle contract. | unaudited |

The model tracks only domain, decision, and observation object sets
(`crates/kernel/tests/kernel_proofs/model.rs:95-120`); it is not the complete
RP2.1 source/revision/tombstone oracle. Its operation vocabulary has no export
page, search database, or selector (`:37-55`). Reuse/extend these fixtures and
digest utilities where their comparison semantics fit. Do not silently use
CrossRoot normalization to erase differences a fixed-S exactness check needs.

## Reusable process-crash and allocation-observation patterns

| Location | Existing pattern and evidence limit | Status |
| --- | --- | --- |
| `crates/kernel/tests/cas_fault_injection.rs:1-6` | Declares process-crash and injected-error scope on the developer filesystem, not power-loss/torn-write/cold-device persistence. | unaudited |
| `crates/kernel/tests/cas_fault_injection.rs:1046-1094` | Child flushes a barrier and parks; parent launches the exact child test, waits for the barrier, then `ChildGuard` kills and reaps it. Existing pattern for search-specific hooks, not an absent broad harness. | unaudited |
| `crates/host-runtime/examples/perf_host.rs:5-33` | Unsafe `GlobalAlloc` wrapper counts allocation requests and cumulative requested bytes; deallocation does not subtract bytes. It is neither per-export live heap nor decoded-memory high water. | unaudited |
| `crates/kernel/src/lib.rs:5` | `forbid(unsafe_code)` is the kernel crate boundary. Importing the perf allocator into that crate would conflict with it. No export observation boundary is supplied by this lint. | unaudited |

The crash pattern's bounded barrier wait is a test-control mechanism, not an
RP2.9 recovery bound. Concrete search COMMIT/ack/selector hooks and reopened
search-state oracles are missing. Reuse the existing process pattern rather
than create a second broad harness.

No permitted live decoded-heap observer is identified for export. A logical
admission charge could help implement a budget, but passing that charge check
does not prove physical heap bounded. The appropriate observer and any separate
test-crate/lint boundary need review; this catalog chooses neither an allocator
nor a new unsafe dependency.

## Durable reuse corrections

These corrections qualify reuse at this HEAD and remain here independently of
working lens notes. The older catalogs are not edited.

| Existing claim or source | Verified correction | Reuse disposition |
| --- | --- | --- |
| [Host selector validity](../../host-runtime/catalog.md#current-profile-never-names-an-unvalidatable-generation) says generation staging has no production caller. | `crates/daemon/src/bin/eidnara-host.rs:964-993` calls staging on the supplied-payload path. | Reuse the generic invariant, not its stale reachability rationale or a claim of search integration. |
| [Invalidated facade effect record](../../daemon/facade/catalog.md#facade-a-claim-effects-ack-and-producer-checkpoint-advance-are-never-composed) says store-side mirror source remains. | `crates/memory-store/src/claim_mirror.rs` is absent; [memory-store invalidation](../../memory-store/catalog.md#mirror-reset-cycle-requires-a-rebuild-grant) records removal. | Historical lead only; do not harden or recreate the removed mechanism. |
| Generic interruption fixture at `crates/host-runtime/src/generation.rs:1899-1926` is offered as recovery evidence. | It writes residue manually; the separate CAS suite supplies the actual child/barrier/kill pattern. | Keep both unaudited and distinguish residue, process crash, and power loss. |
| Remediation is described as a known projected-byte dependency. | `crates/kernel/src/envelope.rs:240-242` has only the domain-name target; `:395-438` changes its bytes without a source-revision change. RP2.1 source mapping is undefined. | Link [projection-remediation-invalidates-derived-bytes](../projection-coverage/catalog.md#projection-remediation-invalidates-derived-bytes) only if approved mapping consumes that field; do not mandate a new occurrence generation. |

The [relationship map](catalog.md#relationship-and-handoff-map) also reuses the
existing withheld/empty distinction and directory-identity property. Ack and
lock ordering belong solely to this part; projection references its record and
the canonical lowercase markers in [fault-map.md](fault-map.md#independent-situation-markers).

## Empty categories and suspiciously quiet areas

| Proposed obligation | Existing full-path check |
| --- | --- |
| Fixed-S multi-page canonical export with concurrent mutations | None found. |
| Retention registration before S, including source-byte coverage and expiry | None found. |
| Pre-decode page memory admission and explicit oversized-row failure | None found. |
| Consumer replay of all retained events through complete commits | None found. |
| Search COMMIT/release before kernel ack, including lost response | None found. |
| Search selector linked to complete caught-up coverage and compatibility | None found. |
| Search database deletion after outbox pruning followed by bounded rebuild | None found. |
| Retrieval disable/pause/deregister/abandon/re-enable orchestration | None found. |
| Ordinary finite-backlog CatchingUp-to-Current and authorized Disabled recovery within approved bounds | None found. |
| Search recovery preserves canonical authority through corruption/mismatch | None found. |

The largest quiet area is the composition, not missing assertion density in
kernel primitives. `pending_outbox` is not a consumer cursor. An outbox fence
does not pin artifact bytes. Generic selector validity does not verify search
coverage. `acknowledge_outbox` cannot observe local work. These distinctions
remain explicit test handoffs rather than assumptions in fixture setup.
Source-sweep coverage, including message cleanup and git sweeps, stays with
[projection-source-inventory-complete](../projection-coverage/catalog.md#projection-source-inventory-complete).
The at-rest sensitivity policy for staged and retained derived bytes remains
an owner question in the authority record, not an inferred policy.
