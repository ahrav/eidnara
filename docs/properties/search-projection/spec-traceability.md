# RP2.1 specification traceability

This map binds settled requirements and stable property slugs to
[specification 347](https://github.com/ahrav/eidnara/issues/347). C1-C7, T1-T9,
and AC1-AC12 name its constraint, testing-seam, and acceptance sections. They
are document locators, not API names or tickets. The approved issue is published;
this companion was authored uncommitted; ticket P1
([#352](https://github.com/ahrav/eidnara/issues/352)) carries it into the
repository. Its property map restates the acceptance criteria and seams of the
[required witness matrix](witness-matrix.md), which is authoritative for both.

## Source requirements and decisions

Source IDs refer to the [source register](source-register.md). Source authority
is S1, then S3/S4, then implementation evidence. The landing amendment changes
granularity only; it does not change source behavior or external ownership.

| Obligation | Specification location | Observable gate or seam | Canonical owner |
| --- | --- | --- | --- |
| R1: five-class rebuildable projection | Outcome; C4; U2/U4 | AC1/AC3; T4/T7 | Projection/source coverage; export supplies fixed-S input. |
| R2: exact raw output, lexical default, exact token limits | C4/C5 | AC3/AC4; T1/T2 | Projection owns raw bytes/policy; embedding owns preflight. RP2.9 owns dense-tool ablation. |
| R3: N1.3 source/control-plane/supervisor slices | Implementation Decisions; U3/U4 | AC3/AC5/AC8/AC9; T3/T7/T8 | Daemon adapters/orchestration; existing JobTable and shared supervisor remain owners of their mechanisms. |
| R4: rows/checkpoint/pending before canonical ack | C3; KTD1; U2 | AC2; T5 | Projection owns local atomicity; export/recovery owns ack/lock order. |
| R5: rebuild on all five contract mismatches | C7; U5 | AC6; T5/T9 | Export/recovery and daemon lifecycle. |
| KTD1: daemon ownership and kernel-only retrieval dependency | C1/C3; Implementation Decisions | AC2/AC6/AC12; T5/T7 and architectural gates | Kernel canonical facts/eligibility; daemon effects; proposed retrieval pure logic. |
| KTD2: register before S, stable cursor, pre-decode caps, full catch-up | C2; U1/U5 | AC1/AC2/AC6; T4/T5/T6/T7 | Kernel export, daemon orchestration. |
| KTD3: verified artifact/fingerprint, untruncated typed count, no new wire method | C5 | AC4; T1/T2 | Embedding owner; provider-accounting vocabulary remains distinct. |
| KTD4: occurrence tuple separate from collision-checked payload bytes | C4 | AC3/AC10; T1/T3/T4 | Projection identity contract. |
| Shared canonical-policy prerequisite | C1; Open Question 8 | AC6/AC9; T7 | Kernel policy move before retrieval implementation; daemon/retrieval adapters share it. |
| Shared async query, original EvalBudget, physical owner | C6; U3; Open Questions 6/8 | AC8; T8 | RP2.1 priority/preflight, RP2.7 query route/budget, RP2.9 envelopes. |
| Stage/verify/select lifecycle reuse | C7; Failure and Rollback | AC6; T5 | Daemon publication owner reuses existing lifecycle; pins drain where applicable. |
| Projection and embedding states, retry/Obsolete | C7; C5 | AC5/AC7/AC9; T3/T8/T9 | Daemon lifecycle and existing JobTable. |
| Exact remediation scope and conditional current-input mapping | C4/C5; Open Question 2 | AC10; T3/T4 | Source mapping owner, with embedding current-input validation. |
| Parent preconditions, not freshly verified success | Milestone Boundaries and Dependencies | AC12; prerequisite evidence review | Migration/Stage 1/host/release/source-freeze owners, not new RP2.1 implementation. |
| Source-plan A1/A2/A3/A4 | Acceptance Criteria | AC2/AC5, AC1/AC6, AC4, AC9/AC12 respectively | The corresponding property owners; RP2.9 acceptance. |
| Bounds approved before capacities | C2/C3/C5/C6; Open Question 6 | AC1/AC4/AC7/AC8/AC12; T2/T6/T8/T9 | RP2.9 owns numeric values; product owners enforce caps. |
| Source-plan Recovery and Harnesses rows | C7; Failure and Rollback; Verification Strategy | AC6/AC7/AC9/AC12 | Daemon recovery, both supported harnesses, RP2.9. |

## Milestones and external dependencies

| Boundary | Preserved goal/dependency | Preserved falsifier and criterion |
| --- | --- | --- |
| U1 | Fixed-S bounded canonical export; no intra-RP dependency. | Concurrent insert/delete across pages, oversized input before decode, expired fence; AC1. |
| U2 | Schema/identity/mutations/checkpoint/durable pending; depends on U1. | Local COMMIT/ack cuts, exact recovered state, no duplicate work or early ack; AC2/AC3. Workspace member and lockfile co-land. |
| U3 | Existing JobTable, preflight, model identity, query priority, backfill, identity GC; depends on U2. | Token boundary/overflow, dispatch/restart, stale identities, vector-before-complete, saturated backfill; AC4/AC5/AC7/AC8. |
| U4 | All N1.3 sources, durable rows/sweeps, raw lexical default; depends on U2/U3. | Per-class ingest/revise/delete/rebuild, byte fidelity, separate coverage, disabled hooks; AC3/AC9. |
| U5 | Stage, verify, catch up, select, then release old consumer; depends on U1-U4. | Search deletion after prune, complete canonical oracle, all selector cuts, slow-consumer disable; AC6/AC7/AC9. |
| RP2.9 first | Limit-manifest schema, protocol, corpus; unapproved placeholders fail closed. | C6 and AC12. Approval precedes capacity, not declaration of placeholder schemas. |
| RP2.4 alongside RP2.1 | Positive-claim projection/policy integration retains its owner. | C1/C4 and AC3/AC6. No parallel claim policy or projection authority. |
| RP2.2/RP2.3/RP2.5.U1 after U2 | Exact/lexical and f32 representation/oracle consume occurrence schema. | Milestone order; these deliverables are not added to RP2.1. |
| RP2.7 identity after U2 | Identity freeze follows schema; RP2.8 consumes, not blocks it. | KTD4; U3 shared query prerequisite; AC8. |
| Thin RP2.7/RP2.8 and RP2.9.U3 before compression/RP2.6 | Both-harness full f32 baseline avoids a calibration/compression cycle. | Milestone order; AC12. |
| RP2.9 final; N3 conditional | Measured triggers are required for conditional work. | C7, Out of Scope, AC12. No conditional algorithm is unconditionally scheduled. |

## Gates, failure rules, exclusions, and landing amendment

| Contract | Specification location and preservation |
| --- | --- |
| Source-plan repository commands | Verification Strategy preserves exactly `cargo fmt --check`; `cargo clippy --workspace --all-targets --locked -- -D warnings`; `cargo test --workspace --locked`; `bun run check:repo`; `bun run release:check`. These are future gates, not executed commands. |
| Current CI additions | Verification Strategy names all-features Clippy/docs, workspace/storage no-default checks, excluded fuzz workspace, two locked nextest slices with all targets/features, bench test mode/doctests, conditional stable, source/metadata/drift guards, native addon/packaged-host/tarball/both-harness E2E, relevant Miri/Valgrind, and scratch-only intentional unlocked dependency checking. |
| Missing release script | Open Question 9 and AC12 retain reconciliation and parent release evidence. No silent waiver or substituted gate. |
| Stop enablement | C7 preserves unmet prerequisite, failed RP2.9 gate, authority mismatch, or unsupported adapter capability. AC9/AC12 require disabled unsupported paths, not simulated capability. |
| Retention and disable failure | C2 and Failure and Rollback preserve source/history fencing, partial cleanup, pause/deregister/audited abandon distinctions, `ConsumerPending`, `NoRequiredConsumer`, no empty prune horizon, and explicit recovery. AC1/AC9. |
| Rollback | C7 and Failure and Rollback permit derived replacement removal/rebuild or prior verified compatible selection with pin drain, never canonical rewriting. AC6. |
| Parent/N1 exclusions | Out of Scope keeps canonical CRUD/maintenance/disposition/transitional removal and general Dreamer elsewhere. TypeScript adapters/glue/configuration/presentation/confirmation/QuickJS/mural stay outside. No migration, predecessor compatibility, or Stage 1 re-verification work is added. |
| Rejected complexity | C1-C6 and Out of Scope exclude cross-database atomicity, post-load-truncated unbounded snapshots, second tokenizer authority, new Synapse wire method, duplicate queue/store/scheduler/lease plane, and deleted-claim hardening. |
| Other RP and N3 boundaries | Milestone order and Out of Scope retain other RPs' deliverables, measured N3 triggers, and deferred inventory/working-set/pin-policy/utility/contradiction/refresh/prefetch/memory/GPU/scope/shadow/hypothesis/transmutation work. |
| Documentation-task boundary | Domain Skill Inputs, Verification Strategy, Out of Scope, and Further Notes distinguish plans/analyst reports from product work; no code, tests, builds, benchmarks, deployment, tickets, commits, or PRs are created here. |
| Owner landing amendment | Milestone section preserves units/dependencies/falsifiers while allowing later multiple cohesive independently verifiable landings. Target <=500, hard <=1,000 changed production-source additions+deletions, tests/docs excluded. Every PR passes applicable gates independently. |
| Owner pre-opening review gate | Milestone section requires independent architecture, complexity, testing, over-engineering, language-design reviews in parallel; consolidate/verify, fix all applicable findings, rerun affected checks, no unresolved blocker. None of these future reviews is represented as complete. |
| One specification, not tickets | Further Notes retains new open specification status, proposed title, local/uncommitted companion location, and exact trailing work marker. Specification approval does not certify implementation, approve numeric limits, or authorize release. No invented issue URL. |

## Property-to-specification map

Each property has one canonical catalog owner. Testing groups can share fixtures,
but they do not redefine another property's oracle. AC11 binds every required
scenario in the three maps, in addition to the specific criteria listed below.
The acceptance criteria and the seams in each row are the union over that
property's cells in the [witness matrix](witness-matrix.md); the kernel test
`search_projection_construction_inputs` checks the two agree.
All records remain proposed, active, `test-only`, and unexercised.

| Canonical property | Specification obligation | Testing seam |
| --- | --- | --- |
| [export-fixed-s-exactly-once](export-recovery/catalog.md#export-fixed-s-exactly-once) | C2/C4; U1; AC1/AC10 | T4 exact S/key/byte ledger; conditional remediation. |
| [export-retention-fence-covers-read](export-recovery/catalog.md#export-retention-fence-covers-read) | C2; U1/U5; AC1 | T7 registration/source-history loss, with T4 oracle. |
| [export-predecode-bounds](export-recovery/catalog.md#export-predecode-bounds) | C2/C6; U1; AC1 | T6 size/decode/high-water observations. |
| [catchup-complete-commit-prefix](export-recovery/catalog.md#catchup-complete-commit-prefix) | C2/C3; U2/U5; AC2 | T4 split/published/empty/control commit ledger. |
| [ack-follows-local-release](export-recovery/catalog.md#ack-follows-local-release) | C3; U2; AC2 | T5 local release/kernel writer/ack and response loss. |
| [replacement-selects-complete-compatible-state](export-recovery/catalog.md#replacement-selects-complete-compatible-state) | C7; U5; AC6 | T5 selector crash/reopen, compatibility and consumer release. |
| [rebuild-after-pruning-converges](export-recovery/catalog.md#rebuild-after-pruning-converges) | C7; U5; AC7 | T9 deletion-after-pruning bounded episode. |
| [catchup-and-authorized-recovery-converge](export-recovery/catalog.md#catchup-and-authorized-recovery-converge) | C7; U5; AC7/AC9 | T9 normal and explicitly authorized recovery modes. |
| [disable-preserves-consumer-obligations](export-recovery/catalog.md#disable-preserves-consumer-obligations) | Failure and Rollback; U5; AC9 | T7 lagging/caught-up and last/non-last/barrier lifecycle matrix. |
| [recovery-preserves-canonical-authority](export-recovery/catalog.md#recovery-preserves-canonical-authority) | C1/C7; Failure and Rollback; AC6 | T7 canonical mutation ledger and final verdicts. |
| [projection-commit-checkpoint-pending-atomic](projection-coverage/catalog.md#projection-commit-checkpoint-pending-atomic) | C3; U2; AC2 | T5 reopened rows/checkpoint/pending prefix. |
| [projection-replay-does-not-resurrect](projection-coverage/catalog.md#projection-replay-does-not-resurrect) | C3; U2; AC2/AC3 | T4 duplicate/overlap/old-prefix replay. |
| [projection-occurrence-payload-separation](projection-coverage/catalog.md#projection-occurrence-payload-separation) | C4; KTD4; U2; AC3 | T1 independent tuples/bytes and collision. |
| [projection-source-inventory-complete](projection-coverage/catalog.md#projection-source-inventory-complete) | C4; R1/R3; U4; AC3/AC11 | T4 all-class inventory, per-class revision and removal ledger, and nonempty declared completeness. |
| [projection-raw-tools-exact-and-lexical](projection-coverage/catalog.md#projection-raw-tools-exact-and-lexical) | C4; R2; U4; AC3 | T1 captured multibyte/CRLF/overlapping-span and multipart error byte buffers. |
| [projection-n13-hooks-stay-gated](projection-coverage/catalog.md#projection-n13-hooks-stay-gated) | C7; U4; AC9/AC12 | T7 frozen hook/gate activation matrix. |
| [projection-canonical-eligibility-authority](projection-coverage/catalog.md#projection-canonical-eligibility-authority) | C1; prerequisite; AC6 | T7 pinned adapter comparison and stale grant. |
| [projection-lexical-dense-coverage-distinct](projection-coverage/catalog.md#projection-lexical-dense-coverage-distinct) | C4/C5; U4; AC3 | T4 independent per-class sets and validity facts. |
| [projection-bounded-admission-preserves-progress](projection-coverage/catalog.md#projection-bounded-admission-preserves-progress) | C3/C6; U2; AC2/AC5/AC7 | T6 whole-commit/pending-cap refusal and unchanged prefix. |
| [projection-remediation-invalidates-derived-bytes](projection-coverage/catalog.md#projection-remediation-invalidates-derived-bytes) | C4/C5; Open Question 2; AC10 | T3 held result/current bytes with the recorded applicability decision and name-only control. |
| [projection-acceptance-situations-witnessed](projection-coverage/catalog.md#projection-acceptance-situations-witnessed) | Verification Strategy; AC11 | All T1-T9; frozen nonempty marker/scenario matrix. |
| [embedding-count-authority-is-untruncated](embedding/catalog.md#embedding-count-authority-is-untruncated) | C5; KTD3; U3; AC4 | T1 independent complete token sequences/artifacts. |
| [embedding-input-is-rejected-before-inference](embedding/catalog.md#embedding-input-is-rejected-before-inference) | C5; U3; AC4 | T2 real preflight plus post-certification attempt counter. |
| [embedding-pending-drives-one-job-table](embedding/catalog.md#embedding-pending-drives-one-job-table) | C5; KTD1/KTD3; U3; AC5 | T8 durable pending/admission/worker trace. |
| [embedding-completion-is-identity-fenced](embedding/catalog.md#embedding-completion-is-identity-fenced) | C5; U3; AC5/AC6 | T3 each identity dimension, current input, tombstone. |
| [embedding-complete-requires-durable-vector](embedding/catalog.md#embedding-complete-requires-durable-vector) | C5; U3; AC2/AC5 | T5 persistence/completion crash and reopened vector. |
| [embedding-restart-retries-durable-pending](embedding/catalog.md#embedding-restart-retries-durable-pending) | C5/C7; U3/U5; AC7 | T9 stable pending and fresh JobTable after actual crash. |
| [embedding-backfill-preserves-query-admission](embedding/catalog.md#embedding-backfill-preserves-query-admission) | C6; shared RP2.7 prerequisite; U3; AC8 | T8 offered-query ledger under bounded saturated backfill. |
| [embedding-identity-gc-preserves-live-work](embedding/catalog.md#embedding-identity-gc-preserves-live-work) | C5; U3; AC3/AC5 | T3 live-reference/obsolete-state matrix and no-op control. |
| [embedding-supervisor-shares-budget-and-joins](embedding/catalog.md#embedding-supervisor-shares-budget-and-joins) | C6; R3; shared RP2.7 prerequisite; AC8 | T8 original deadline/flag and physical owner/permit/charge census. |

## Ownership and completeness checks

The map contains 30 distinct canonical slugs, matching the ten/eleven/nine
catalog split and 30 evidence files. Ack ordering and ack-loss marker definitions
stay with export/recovery. Local atomicity stays with projection; completion
durability and current vector validity stay with embedding. Projection's one
campaign record binds all 63 marker definitions and required dimensions without
copying their definitions into a new map.

Every source requirement, KTD, U dependency/falsifier, acceptance category, gate,
exclusion, and landing amendment has a destination above. This is structural
traceability, not proof that implementation meets the contracts. The
[verification receipt](verification.md) records the fresh check, owner approval,
and byte-for-byte publication readback.
