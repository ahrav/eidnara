# Existing checks and reuse assessment

System: `/local/home/ahrav/scratch/eidnara`.
HEAD: `913234433ae36a80a6e22c6aac14c7f9aab74386`.
Date: 2026-09-10. External scope and source hashes:
[_lenses/model.md](_lenses/model.md#source-register).
No incidents were supplied. No checks were executed. Every check below is
`unaudited`: source inspection establishes its presence and assertions, not
adequacy or a passing result. The inventory is scoped to the RP2.1 seams in
this part, not every assertion in the workspace.

The independent evaluation supplied by the user adds remediation and the CAS
crash-driver inventory below. Its provenance and dispositions are in
[portfolio-evaluation.md](portfolio-evaluation.md); this edit is not a second
independent adequacy audit.

## Directly relevant checks

Paths in this table are repository-relative. Plain assertion messages below
mean the default Rust assertion diagnostic unless a quoted message is given.

| Location and check | Asserted behavior or rejection | Status | RP2.1 limitation |
| --- | --- | --- | --- |
| `crates/kernel/tests/kernel_outbox.rs:86-153`, `acknowledgements_use_commit_boundaries_through_commit_log_tip` | Repeated acknowledgement succeeds, empty committed sequence is accepted, backward/nonexistent checkpoints return `InvalidCheckpoint`; plain equality assertions. | unaudited | No search database is opened. |
| `crates/kernel/tests/kernel_outbox.rs:156-224`, `slow_consumer_sets_commit_prune_horizon_and_registration_sees_oldest_retained_commit` | Minimum consumer progress controls pruning; receipts survive; new registration sees retained history. | unaudited | Retention mechanism belongs to the export/rebuild owner. |
| `crates/kernel/tests/kernel_outbox.rs:622-698`, `pending_outbox_reads_unpublished_rows_in_order_with_commit_boundaries` | Zero limit rejected; rows ordered; payload equals stored bytes; truncated commit is not a boundary; publication removes rows from the pending set. Message: `position 1 is mid-commit`. | unaudited | Row limit and global publication state do not provide per-consumer byte-bounded replay. |
| `crates/kernel/src/outbox.rs:432-434,493-501,536-561` | Runtime validation rejects zero batch size, invalid publication checkpoint and backward/nonexistent consumer checkpoints. | unaudited | These are `Result` guards, not assertions about local durability. |
| `crates/kernel/src/envelope.rs:1043-1074`, `stored_receipt` | Existing producer/operation key under a different request digest returns `Conflict`; same digest replays the stored receipt. | unaudited | Canonical commit receipt, not local occurrence or job deduplication. |
| `crates/kernel/src/envelope.rs:395-438`, `remediate_text_inner` | Validates operator/time and domain target, rewrites `domains.name`, and emits `operator_remediation` with the loaded object metadata. | unaudited | Only a domain-name target exists. It establishes no approved RP2.1 input dependence or at-rest residue policy. |
| `crates/kernel/tests/kernel_retention.rs:395-466`, `remediation_supports_two_live_domains_and_keeps_receipt_as_caller_result` | Two names become the operator placeholder; audit payloads and caller receipt are checked. Plain equality assertions. | unaudited | Does not assert a projection input change, unchanged occurrence tuple or equality of all embedding-key fields. |
| `crates/kernel/tests/kernel_retention.rs:743-814`, `remediation_reaches_retired_domains_and_repeats_without_failing` | Retired domain name is replaced; repeat remediation creates a second audit event. | unaudited | Canonical field behavior only, not stale payload/vector exclusion. |
| `crates/kernel/tests/cas_fault_injection.rs:1046-1094`, `crash_barrier`, `run_crash_child`, `ChildGuard::drop` | Child flushes a barrier label and parks; parent waits for a barrier line, then kills/reaps the child. Diagnostics include `child barrier timeout` and `child exited before barrier`. | unaudited | Reusable process-control pattern exists; product-specific search/embedding commit hooks are still missing. The test's wait value is not an RP2.9 approval. |
| `crates/kernel/tests/cas_fault_injection.rs:350-388,924-990`, semantic recovery and crash-window checks | Reopens the store, compares successive semantic states, compares crash/no-crash cases, and checks a committed reservation before recovery. Message: `second recovery changed semantic state`. | unaudited | CAS state oracle and its normalization are not a five-class projection oracle. Process termination is not power-loss evidence. |
| `crates/kernel/tests/cas_fault_injection.rs:391-423,981-989`, `assert_drives` and protocol uniqueness | Declared protocol point IDs equal driven IDs for the selected driver/operations; IDs are unique. Message includes `coverage drifted from the declared protocol`. | unaudited | Adjacent declared-versus-witnessed accounting, not an approved three-surface acceptance matrix. |
| `crates/daemon/src/codec/mod.rs:59-94`, OpenCode golden | Decode/encode determinism, boundary presence and value equality after compaction stripping. | unaudited | No persisted raw buffer or search occurrence. |
| `crates/daemon/src/codec/mod.rs:97-130`, native-serving golden | Synthetic output shapes and retained ingress value equality. | unaudited | JSON values, not original serialized envelope bytes. |
| `crates/daemon/src/codec/mod.rs:184-219`, Pi golden | Decode/encode determinism, boundary presence and value equality after compaction stripping. | unaudited | No lexical or dense coverage report. |
| `crates/daemon/src/codec/mod.rs:222-258`, leading-block conformance | Deleting a tool block retains the expected surviving native text part for each harness. | unaudited | Native-part alignment, not durable occurrence identity. |
| `crates/daemon/src/codec/mod.rs:260-277`, capture-class bookkeeping | Every required class is covered or listed missing. Message: `codec golden neither covers nor records missing classes: {unresolved:?}`. | unaudited | Listing a class missing is not a witness that it ran. |
| `crates/daemon/src/canonical_memory.rs:282-312`, test `injectable_rows_are_visible_decisions_in_a_positive_category` | Only the visible positive decision remains among labeled, negative and observation fixtures. | unaudited | This is a test, not the reader. Production reader is at `:141-212`; neither is a full source inventory. |
| `crates/daemon/src/canonical_memory.rs:315-364`, rendered-revision checks | Reordered rows/snapshot changes preserve digest; rendered content changes alter it. | unaudited | That digest is not canonical revision or payload identity. |
| `crates/daemon/src/config.rs:1881-1929`, `no_tier_can_enable_indexing_embedding_git_or_mural` | Config keys omit absent subsystem prefixes; hostile flags leave defaults. Message: `{key:?} would let a tier configure an absent subsystem`. | unaudited | No future gate evidence or supervisor activation matrix. |
| `crates/daemon/src/lib.rs:32439-32479`, absent routes | Flat method requests return `unrecognized_request_shape`; facade requests return `facade_envelope_not_supported`; assertions label `{name}`. | unaudited | Does not observe future hook side effects or acceptance transitions. |
| `crates/daemon/tests/stage1_eligibility.rs:87-134`, two read surfaces | Explicit search labels asserted decisions; auto-inject admits only the verified one; both name the pinned snapshot. | unaudited | Read-surface policy is not retrieval-adapter parity. |
| `crates/daemon/tests/stage1_eligibility.rs:137-183`, batch verdicts | Ordered `ok`, `stale`, `retracted`, `retracted` results; repeats hit cache. | unaudited | Only the daemon adapter exists. |
| `crates/daemon/tests/stage1_eligibility.rs:186-224`, cached retirement | Old snapshot still lists an object while current batch verdict retracts it and misses old cache. | unaudited | Useful canonical authority fixture, not a search projection test. |
| `crates/daemon/src/kernel_routes/eligibility.rs:536-550` | Cache capacity/oldest eviction, replacement and clear. | unaudited | Cache bounds are independent of local projection capacity. |
| `crates/daemon/src/kernel_routes/eligibility.rs:553-571,574-609,612-629` | Only misses read under stable snapshot; moved snapshot discards mixed old hits; all-miss batches avoid a redundant read. | unaudited | Preserves daemon snapshot behavior only. |
| `crates/daemon/src/kernel_routes/eligibility.rs:269-271` | `expect` checks one fact per missing candidate. Message: `egress_candidates returns one entry per named candidate`. | unaudited | Adapter cardinality premise, not a parity oracle. |
| `crates/daemon/src/kernel_routes/eligibility.rs:139-173,387-413` | Runtime verdict ladder and request count/ID/digest validation. | unaudited | The policy is daemon-private; no shared kernel adapter exists. |

## Overlap register

The records below were considered before adding projection records. Reuse
means reuse the claim and investigation trail, not inherit its old exercise
status. The current pass assigns no adequacy verdict to those checks.

| Reusable record | Assessment and disposition |
| --- | --- |
| [Codec round-trip identity][codec] | Reuse the exact decode/encode contract. RP2.1 adds durable raw-byte/source-span fidelity, not a second codec round-trip guarantee. The old citation `codec/mod.rs:78-89` is now the loop at `82-93`; the Pi loop is `207-218`. |
| [Codec identity stamp][stamp] | Reuse native-part alignment investigation. Current fingerprint/stamp code is `codec/sidecar.rs:169-212`; it is not an occurrence/payload allocator. No forged-stamp bug is imported as a demonstrated RP2.1 defect. |
| [Missing capture classes][capture] | Reuse the capture coverage obligation. Current bookkeeping is `codec/mod.rs:260-277`, rather than the old `254-271`. New source-class markers cannot use a missing-class waiver as exercise evidence. |
| [Historian raw publication][raw] | Reuse the historian's atomic raw-copy claim. Its old `historian_chunk.rs:717-727` source construction is now `664-674`. That is serialized selected CK messages, not a five-class search transaction. Storage-side old line citations were not revalidated here and are not imported as current proof. |
| [Canonical withheld-read reporting][withheld] | Reuse the distinction between withheld, empty and disabled reads. Verified current mapping at `canonical_memory.rs:99-127,141-174`. Dense coverage adds a different observation axis. |
| [Scheduled Dreamer slot][scheduler] | Reuse the existing supervisor lease/receipt claim. Current task identity is `dreamer_scheduler.rs:25-36,133-136`, and bridge selection is `lib.rs:13979-14044`. RP2.1 adds message cleanup, git sweeps and embedding work; this pass does not duplicate scheduling semantics. |
| [Mirror replay][mirror] and [mirror conflict][mirror-conflict] | Both records are invalidated. HEAD has neither `crates/memory-store/src/claim_mirror.rs` nor its test file. Their old test claims cannot cover a proposed search database. |

No exact existing catalog guarantee covers RP2.1 local transaction atomicity,
five-class projection coverage, occurrence/payload separation, shared kernel
eligibility adapters, or current-model dense coverage reporting. Similar
mechanisms are linked above instead of being counted as equivalent coverage.

## Legacy catalog currency and policy limits

The overlap register is a durable inventory caveat, not only a lens note.
Legacy records retain historical source references, exercise claims and CI
statements. Those claims do not transfer to this HEAD or to RP2.1 merely because
their Markdown links resolve. Only the current locations explicitly checked
above are treated as current source evidence; test adequacy stays unaudited.
The historian storage-side references remain unverified here. Invalidated
claim-mirror records remain historical and cannot supply executable coverage.
Codec value equality does not certify exact serialized source bytes, and a
missing capture class recorded in a fixture is not a required-scenario witness.

Sensitivity policy for RP2.1 at-rest source retention remains unresolved.
Canonical egress denial does not settle which bytes the projection may retain,
and exact-raw fidelity does not grant new storage permission. This catalog adds
neither a sensitive-source exclusion nor permission to retain remediated bytes.
The source-policy owner must define the admitted boundary and residue policy;
the conditional remediation record only constrains what may be called current.

## Explicit empty categories and quiet areas

- Search rows/checkpoint/pending transaction assertions: none found.
- Retrieval occurrence/payload identity or forced-collision checks: none found.
- Five-class ingestion/revision/deletion coverage checks: none found.
- Stored raw-tool bytes plus default absence of dense work: none found.
- Shared kernel-policy versus retrieval-adapter parity: none found.
- Lexical/current-revision/current-model dense report checks: none found.
- Local projection byte and durable-pending admission guards: none found.
- RP2.9 accepted-gate transitions for N1.3 hooks: none found.
- Production search transaction failpoints or an RP2.1 fault campaign: none found.
- RP2.1 remediation input-hash/currentness checks: none found.
- Binding three-surface acceptance witness checks: none found.

Generic kill/barrier/reopen machinery does exist in the CAS tests listed above.
The missing part is the RP2.1 product boundary and oracle, not all crash tooling.

The source plan marks these mechanisms proposed. These gaps are not reports
that an implemented projection is broken. The existing tests' names can sound
broader than their observation boundaries, especially "both lanes" and
"projection". Inspect the subject before treating them as retrieval evidence.

[codec]: ../../daemon/decisions/catalog.md#codec-b-round-trip-identity-is-claimed-in-one-direction-on-one-case-per-harness
[stamp]: ../../daemon/decisions/catalog.md#codec-b-block-identity-stamp-is-caller-writable-and-the-fingerprint-is-not-an-identity
[capture]: ../../daemon/decisions/catalog.md#codec-b-declared-missing-capture-classes-are-never-decoded
[raw]: ../../daemon/historian/catalog.md#publish-preserves-raw-chunk-messages-atomically
[withheld]: ../../daemon/transform/catalog.md#canonical-read-staleness-is-distinguishable-from-emptiness
[scheduler]: ../../daemon/handlers/catalog.md#scheduled-dreamer-slot-runs-once-through-lease-and-receipt
[mirror]: ../../memory-store/catalog.md#mirror-receipt-replay-applies-effects-once
[mirror-conflict]: ../../memory-store/catalog.md#mirror-receipt-conflict-rejects-divergent-replay
