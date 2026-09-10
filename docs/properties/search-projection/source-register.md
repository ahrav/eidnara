# RP2.1 source register

## Authority and inspection boundary

Inspection date: 2026-09-10. Product checkout:
`/local/home/ahrav/scratch/eidnara`. Read-only revision inspection confirms HEAD
`913234433ae36a80a6e22c6aac14c7f9aab74386`.

The Stage 2 parent is authoritative, followed by the settled RP2.1 plan and RP2
index, then implementation evidence. Earlier research and reviews explain
decisions but cannot override the settled contract. Historical current-code
claims require reconciliation against this product revision.

The owner amends only landing granularity. U1-U5 remain milestones with their
original dependencies and falsifiers. Later tickets/PRs may split each boundary
into cohesive independently verifiable results. Each PR targets at most 500
production-source changed lines and never exceeds 1,000 additions plus
deletions, excluding tests/docs. Every PR passes applicable gates and independent
parallel architecture, complexity, testing, over-engineering, and language-design
reviews before opening, with all applicable findings fixed, affected checks
rerun, and no unresolved blocker. Workspace member and lockfile co-land even
when U2 is split. This task creates neither tickets nor PRs.

Parent assertions about migration U1-U5, Stage 1 verification, a fresh host
reached by both harnesses, the owner-hosted `1.0.0-rc.1` chain under `rc`, and
frozen source scopes remain preconditions requiring evidence. No runtime,
release, remote tracker, or incident evidence was gathered in this writer pass.
Source beads `3q5.9`, `3q5.22`, `3q5.28`, `pml.4`, `pml.5`, `pml.6`, `3q5.23`,
and `ks4` are provenance only. No bead lookup or mutation occurred.

## Primary sources and review references

The following locators are relative to the exact filesystem base
`/local/home/ahrav/scratch/commons/docs/`. Each SHA-256 hashes the complete
consulted file, not a rendered excerpt. The Eidnara HEAD does not pin Commons
working-tree files. Links are local source locators, not published issue links.

| ID | Title and local locator | Consulted contract | SHA-256 |
| --- | --- | --- | --- |
| S1 | **Eidnara Native Rust and Cutover - Plan**, 2026-09-08. [plans/2026-09-08-0523-feat-eidnara-native-rust-cutover-plan.md](../../../../commons/docs/plans/2026-09-08-0523-feat-eidnara-native-rust-cutover-plan.md) | Preconditions; N2; invocation and Stage 2 acceptance; N3 exclusions; verification. Cutover is not imported as RP2.1 work. | `a6b787653eb159358784f5c080ee925cf70f195893a1daf5e316d443c8e1b3d5` |
| S2 | **Eidnara Rust Product-State Ownership - Plan**, 2026-09-08. [plans/2026-09-08-1614-feat-eidnara-rust-product-state-ownership-plan.md](../../../../commons/docs/plans/2026-09-08-1614-feat-eidnara-rust-product-state-ownership-plan.md) | N1.3 ownership, shared supervisor, disabled hooks, permanent TypeScript boundaries, canonical/transitional exclusions. Its earlier code survey is historical. | `ad49c0a9b546e527f4b45c575a7f8bbb8a8783df947ccebe64c2193adbda96e3` |
| S3 | **Eidnara RP2.1 Rebuildable search.sqlite projection and vector coverage - Plan**, 2026-09-10. [plans/2026-09-10-feat-eidnara-rp2-1-projection-coverage-plan.md](../../../../commons/docs/plans/2026-09-10-feat-eidnara-rp2-1-projection-coverage-plan.md) | Complete document: R1-R5, KTD1-KTD4, states, bounds, U1-U5, acceptance, rollback, gates, and review dispositions. | `1e689c00c8d8a2e0a3acb87964a8b15f868aaa948c1e0b24e8604e2107e4fd2e` |
| S4 | **Eidnara RP2 Standalone Plan Index**, 2026-09-10. [plans/2026-09-10-eidnara-rp2-plan-index.md](../../../../commons/docs/plans/2026-09-10-eidnara-rp2-plan-index.md) | Complete shared dependency order; policy, tokenizer, EvalBudget, lifecycle reuse, capability, and approval contracts. | `95845417eefd6ca6ad4282f511e10d7cbdde75dc0f8778a950f5ccd096eae731` |
| S5 | **Eidnara RP2.9 Measurement and acceptance - Plan**, 2026-09-10. [plans/2026-09-10-feat-eidnara-rp2-9-measurement-acceptance-plan.md](../../../../commons/docs/plans/2026-09-10-feat-eidnara-rp2-9-measurement-acceptance-plan.md) | Schema before capacity approval; corpus; f32 baseline before compression; numeric, class, full-path and transition evidence. No thresholds inferred. | `c8800afdc5009ca984589d3aeb20bf8ac2789c12dc32b4409505634bb82838f5` |
| S6 | **Eidnara RP2.7 One fusion boundary and daemon routes - Plan**, 2026-09-10. [plans/2026-09-10-feat-eidnara-rp2-7-fusion-routes-plan.md](../../../../commons/docs/plans/2026-09-10-feat-eidnara-rp2-7-fusion-routes-plan.md) | KTDs, budget/physical ownership, and U3 query-route ownership; dependency, not scope transfer. | `1f76e2f32dad64b97ddca4bc93b436f02fb5f4f4201dd3bb0b114d8c46d50154` |
| S7 | **Eidnara RP2 Architecture Review**. [research/2026-09-10-rp2/architecture-review.md](../../../../commons/docs/research/2026-09-10-rp2/architecture-review.md) | A1/A2 authority and JobTable reuse, lifecycle reuse, bounded-export qualification, early manifest schema. | `2d2ea000dfb0b7f00a699ed388d3c8e2c8b41bec758e59a7449e1931476f1a25` |
| S8 | **Eidnara RP2 Ponytail Review**. [research/2026-09-10-rp2/ponytail-review.md](../../../../commons/docs/research/2026-09-10-rp2/ponytail-review.md) | Reuse proposals, subsequently qualified by settled preallocation and exact-token requirements. | `cac08ac0d89a5e9a7e73ee802c02eeb50b56ccf1fab5a3d3142f20f5012b0c4b` |
| S9 | **Eidnara RP2 Rust Design Review**. [research/2026-09-10-rp2/rust-design-review.md](../../../../commons/docs/research/2026-09-10-rp2/rust-design-review.md) | Exact tokenizer gap, async query prerequisite, lifecycle/lock order, typed units, lockfile co-landing. Empty-consumer pruning wording is reconciled against current code. | `aa1239eb2644f8adcc83bab13b2bbe9b542776c93134628246bc7ce16975dda2` |
| S10 | **Deeper Research Results: RP2.1-RP2.9 recovery**. [research/2026-09-10-rp2/recovery-research.md](../../../../commons/docs/research/2026-09-10-rp2/recovery-research.md) | S2.F1, source reconciliation, and limits of one-worker recovery synthesis. Mechanism evidence, not local crash proof. | `7735160c219b81450039e6dc81c8fe2ef12ec322e0730a2ce6e0eb22973e3d16` |
| S11 | **P4.a3 Adversarial Assumptions Audit (RP2.1-RP2.9)**. [research/2026-09-10-rp2/initial/adversary-3.md](../../../../commons/docs/research/2026-09-10-rp2/initial/adversary-3.md) | Historical fixed-S/unbounded materialization and consumer/serving risks; reconciled by S3/S4 and current code. | `65074f0e2846f8f691497f25118412895563bc9207a4956f5c014d69ed29d256` |
| S12 | **Research: RP2.1 rebuildable search.sqlite projection and vector coverage**. [research/2026-09-10-rp2/initial/rp2-1-survey.md](../../../../commons/docs/research/2026-09-10-rp2/initial/rp2-1-survey.md) | Initial projection inventory, gaps, and class coverage leads. Later reconciliations take precedence over incomplete findings. | `ce8d4451de0caac86e9e5a517332aa6c89dce8278e62548e0eed8fd7b0d59cf0` |

## Writing and method sources

Read [root AGENTS](../../../AGENTS.md),
[property AGENTS](../AGENTS.md), and [METHOD](../METHOD.md) before editing.
METHOD remains the record-schema authority. The issue body uses every mandatory
spec-template heading, not Rust API documentation scaffolding.

These reference locators are relative to
`/home/ahrav/.claude/plugins/marketplaces/ahrav-claude-plugins/`:

| Title | Exact relative locator | SHA-256 |
| --- | --- | --- |
| Specification Template | `research-planning/skills/to-spec/references/spec-template.md` | `cd6ff8cd3907e61144b04b8118fd62d86fba21ac40780978407b3774d9a328b6` |
| Documentation Style | `docs-writing/skills/doc-rigor/references/documentation-style.md` | `5afdabffbf375acda9734b2ab3fe39eb6d5cbd037d379dfc3f42d94cede5e301` |
| Doc-Rigor Writer Agent Prompt | `docs-writing/skills/doc-rigor/references/writer-prompt.md` | `63f3c449296303926cbd0da1f47d64e4cd71b2f64152b4b7561bc5863140fc32` |

## Discovery inputs

All three complete catalogs, per-part evaluations, fault maps, and inventories
were read. Their exact observation state is hashed below. The projection
evaluation hash includes this writer's authorized attribution correction.
Evidence files are linked by stable slug from each catalog; these root documents
do not claim an independent re-audit of every per-part evidence citation.

| Local input under this directory | SHA-256 |
| --- | --- |
| [export-recovery/catalog.md](export-recovery/catalog.md) | `6868ce20062a2ea2a5201d980f659bb323fa21252b0971d81a1d4eac97c57553` |
| [export-recovery/existing-checks.md](export-recovery/existing-checks.md) | `2d1b69f20d6c6cd816bdc79730fd9c2f62b2eadf16d7daa34ddd8c8f884a199f` |
| [export-recovery/fault-map.md](export-recovery/fault-map.md) | `5d417f090a3c9a3a382439121935a2394a4aadf90cd080ceb1ecb5232bf3d6df` |
| [export-recovery/portfolio-evaluation.md](export-recovery/portfolio-evaluation.md) | `c3aa35f40a22b69c1224672add638e230f718fd40b2f688feea703ae451dff5b` |
| [projection-coverage/catalog.md](projection-coverage/catalog.md) | `a8bfd3459166c095a6b1ce268f43ebced87d7f004cddd4cc0c5771ca5e421db4` |
| [projection-coverage/existing-checks.md](projection-coverage/existing-checks.md) | `bc7db870bb3531908eea15229b564e9f689a0ba5535e0d80af81d5e8d1f413c8` |
| [projection-coverage/fault-map.md](projection-coverage/fault-map.md) | `25d27cca8192b79871f18074cd68bed99fe2953c57f84c953c1188787ae32fa6` |
| [projection-coverage/portfolio-evaluation.md](projection-coverage/portfolio-evaluation.md) | `c7e6236473d011cf65c1e8e881ae413fff81f468fa8ae50da4c7e0a093185e5c` |
| [embedding/catalog.md](embedding/catalog.md) | `93c62e5667e68689b2568c23de896ebc223311c9e7983221e7c959a01b5a9c34` |
| [embedding/existing-checks.md](embedding/existing-checks.md) | `dd5ac07e701a5b4010524457b69aad2a578dc8cd9f71b21ae00284129c5ac6a1` |
| [embedding/fault-map.md](embedding/fault-map.md) | `581a88e60bc14f1a2f544d3a59e693af57f693b7091e5c3c830ef8a331444927` |
| [embedding/portfolio-evaluation.md](embedding/portfolio-evaluation.md) | `18465b4e46b9271d779b1599e751cd68dd6fd2052d64212eb9d37c0e1c60aa3f` |

The root [evaluation](portfolio-evaluation.md) records the controlling
coordinator-commissioned analyst attribution. Per-part references to findings
being supplied through a user are handoff provenance, not evidence of user
authorship, a user-run campaign, or a completed acceptance gate.

## Current implementation evidence and qualifications

Product links below are repository-relative. The named implementation/test
files were compared byte-for-byte with the inspected HEAD where noted in the
writer's static receipt. They establish available mechanisms and gaps, not
passing behavior. Per-part inventories retain detailed check references and
their `unaudited` status.

| Source | Observation and limit |
| --- | --- |
| [Kernel outbox](../../../crates/kernel/src/outbox.rs), `pending_outbox`, `deregister_outbox_consumer`, `acknowledge_outbox`, `prune_outbox` | Publisher reads omit published rows. Deregistration rejects a checkpoint below the pre-operation tip. Ack takes the kernel writer. No consumers means `NoRequiredConsumers`, not permission to prune without a horizon. Complete per-consumer catch-up remains a prerequisite. |
| [Canonical slice reads](../../../crates/kernel/src/slice/read.rs), `slice_as_of`, `decision_payload_sizes_as_of` | Historical reads and size lookup are reusable. Collecting a snapshot before truncating cannot establish a preallocation bound. |
| [Kernel remediation](../../../crates/kernel/src/envelope.rs), `remediate_text_inner` | Only `CanonicalDomainName` is handled; it updates `domains.name` and emits `operator_remediation` without updating source revision. Projected dependence remains unknown. |
| [Serving decisions](../../../crates/daemon/src/kernel_routes/serving.rs), `decide`, `decide_for_tip_read` | Empty consumers mean unavailable gated reads; direct canonical tip reads have a different rule. Removing the last consumer does not imply pruning can resume. |
| [Daemon eligibility](../../../crates/daemon/src/kernel_routes/eligibility.rs), `judge` | Policy is daemon-owned at this HEAD. The shared kernel move is proposed, not an existing authority location. |
| [Inference](../../../crates/host-runtime/src/synapse/inference.rs), `token_count`; [JobTable](../../../crates/host-runtime/src/synapse/jobs.rs) | Existing count uses truncating inference tokenizer. JobTable is process-local, with fresh incarnation/maps on creation and resident Ready results, not durable product completion. |
| [Generation store](../../../crates/host-runtime/src/generation.rs), `stage_and_promote`, `prune` | Reuse lifecycle ownership; host payload validity alone does not prove SQLite/journal replacement completeness. |
| [CAS fault suite](../../../crates/kernel/tests/cas_fault_injection.rs) | Existing child barrier/kill/reap/reopen pattern covers process crash and injected errors, explicitly not power loss. Product hooks remain missing. |
| [Kernel proof harness](../../../crates/kernel/tests/kernel_proofs/harness.rs), [model](../../../crates/kernel/tests/kernel_proofs/model.rs), [canonical digest](../../../crates/kernel/tests/support/canonical_state.rs) | Clean restart and a three-kind model are reusable, not an all-class search oracle. CrossRoot normalization cannot be copied into fixed-S equality without preserving identities and bytes. |
| [Deterministic Synapse support](../../../crates/host-runtime/tests/support/synapse.rs), [scheduler](../../../crates/daemon/src/dreamer_scheduler.rs) | Controlled engine calls and in-crate ManualClock support narrow testing. They do not establish shared RP2.1/RP2.7 ownership. |
| [Kernel lint](../../../crates/kernel/src/lib.rs), [perf allocator](../../../crates/host-runtime/examples/perf_host.rs) | Kernel forbids unsafe code. The perf example uses unsafe cumulative requested-allocation counters, not live decoded-heap high water. No permitted observer is selected. |
| [CI](../../../.github/workflows/ci.yml), [root scripts](../../../package.json) | CI is required-check authority. Root scripts have `check:repo` but no `release:check`. The source-plan release gate remains unresolved, not waived. |

Legacy corrections have durable homes in
[export reuse](export-recovery/existing-checks.md#durable-reuse-corrections),
[projection currency](projection-coverage/existing-checks.md#legacy-catalog-currency-and-policy-limits),
and [embedding corrections](embedding/existing-checks.md#durable-corrections-to-reused-evidence).
Examples include live provider-tokenizer calls, production Synapse composition,
explicit-config certified inference, ignored runtime tests, last-poll-based
retention, a host staging CLI caller, and removed claim-mirror subjects. Reuse
the verified mechanism, not an old exercise label or stale absence assertion.

## Evidence that remains absent

No numeric RP2.9 limits, exact independent embedding-token fixture, full
source/encoding/span mapping, source-byte fence, complete retained-history
consumer, product durability/publication protocol, physical-heap observer,
hook/campaign manifest, or release-gate reconciliation is supplied as completed
implementation evidence. The specification assigns each decision an owner and
latest gate. Catalog presence cannot close it. No source hash is an approval.
