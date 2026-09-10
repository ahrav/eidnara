# RP2.1 system-model passes

## Scope and method

System: `/local/home/ahrav/scratch/eidnara`.
Revision: `913234433ae36a80a6e22c6aac14c7f9aab74386`.
Date: 2026-09-10. Evidence is source inspection, not execution.
The user supplied the evidence scope: the settled RP2.1 plan, its linked
index, parent and N1 plan, and this repository. No incidents were supplied.
No tracker, external web source, or other RP plan was consulted.

The parallel lens dispatch required by METHOD was attempted. The harness
refused all dispatches at its subagent depth limit. These are separate
sequential attention passes, not independent reviewers. `colgrep` was tried
first; its index failed to load. Exact search and file reads supplied the
fallback. Wildcard passes follow in [wildcard.md](wildcard.md).

The central independent analyst's supplied evaluation is recorded in
[portfolio-evaluation.md](../portfolio-evaluation.md). These retained discovery
passes and the later disposition edits are not additional independent reviews.

## Source register

The four Commons files are external working-tree documents, not files at
Eidnara HEAD. Their SHA-256 values pin the bytes inspected here.

| Source | Why consulted | SHA-256 |
| --- | --- | --- |
| [RP2.1][plan] | Settled claims, U2/U4, local commit and coverage boundary. | `1e689c00c8d8a2e0a3acb87964a8b15f868aaa948c1e0b24e8604e2107e4fd2e` |
| [Index][index] | Shared identity, eligibility, and enablement prerequisites. | `95845417eefd6ca6ad4282f511e10d7cbdde75dc0f8778a950f5ccd096eae731` |
| [Parent][parent] | Stage 2 authority and source-class policy distinction. | `a6b787653eb159358784f5c080ee925cf70f195893a1daf5e316d443c8e1b3d5` |
| [N1][n1] | Exact N1.3 disabled-hook ownership inventory. | `ad49c0a9b546e527f4b45c575a7f8bbb8a8783df947ccebe64c2193adbda96e3` |

## Architecture and data flow

`Cargo.toml:3-17` lists no retrieval crate. The tracked tree contains no
`crates/retrieval/`; exact Rust searches find no `search.sqlite`,
`occurrence_id`, `payload_id`, `pending_embedding`, or
`projection_checkpoint`. This supports absence of the named RP2.1 mechanism,
not absence of every index in the product.

`crates/daemon/src/canonical_memory.rs:141-212` reads canonical memory for
rendering. `crates/daemon/src/codec/mod.rs:3-15` exports two harness codecs.
Neither is the proposed durable source projection. RP2.1 lines 81, 101-104
assign codecs and consumption to daemon and indexing to a kernel-dependent
retrieval crate.

## State and persistence

`crates/kernel/src/outbox.rs:28-43` distinguishes outbox position, commit
sequence, ordinal, source revision, and commit boundary. Its acknowledgement
at lines 530-570 updates only kernel state. RP2.1 lines 137-142 require local
rows, checkpoint, and pending jobs in one different database transaction.
The local transaction and its crash observation seam are proposed.

## Concurrency model

`acknowledge_outbox` acquires the kernel writer at `outbox.rs:540-541`.
RP2.1 line 116 requires releasing the search transaction before that lock.
No cross-database atomic commit is promised. A competing reader must observe
one search snapshot, not separate reads that manufacture mixed-state evidence.

## Claimed safety guarantees

RP2.1 lines 37-41, 54-59, 84, 110 and 156 promise source coverage, atomic
progress, replay, identity separation, exact raw bytes and gated enablement.
They remain claims under test. No runtime evidence is inferred from readiness
metadata or the plan's review-disposition table.

## Claimed liveness guarantees

RP2.1 lines 108-120 describe catch-up, retry and bounded supervision, but give
no accepted recovery deadline. RP2.9 owns values. This part checks safety at
declared checkpoints and coverage observations. A stalled consumer cannot be
called healthy merely because its safety checks pass. Progress bounds remain
an explicit handoff, not a fabricated finite-liveness property.

## Bug history and density

Scoped local history includes `f7ccbb6d` (absent-hook checks), `7af2ec9c`
(Dreamer scheduling), and `ffe12796` (retired memory-plane identifiers).
Titles guided inspection of current code; none establishes a defect.
No incident or reproduction was supplied. The dense historical mirror
catalog concerns deleted code and does not establish search replay coverage.

## Existing test strategy

`kernel_outbox.rs:86-153,622-698` tests acknowledgement and batch boundaries.
`codec/mod.rs:59-94,184-219` compares parsed JSON values and determinism.
`config.rs:1881-1929` and daemon `lib.rs:32439-32479` test absent features.
All are inventoried as unaudited in [existing-checks.md](../existing-checks.md).
No test or build ran in this discovery pass.

## Failure and degradation

RP2.1 lines 114-116 keep lexical rows when exact tokenizer identity/count is
unavailable. Pausing, deregistration and abandonment have different retention
effects. This part retains local durable obligations under refusal; consumer
teardown and rebuild protocol belong to the export/rebuild owner.

## Dependencies

SQLite spans two independent stores. `Cargo.toml:43` declares rusqlite with
bundled SQLite. Existing Synapse execution is a downstream dependency, not
durable local job truth. The embedding owner supplies result validity and
token limits. The export owner supplies complete source prefixes. No new
network coordinator or queue is implied.

## Product context

RP2.1 line 37 names messages, canonical claims, promoted memory, git commits
and selected raw tool spans. Parent lines 69-74 separate retention, lexical
indexing and embedding policy and require both harnesses. Aggregate success
can hide a missing class or a broken harness adapter.

## Unproven assumptions

The source inventory, host-record namespace, span byte convention, payload
collision response, gate evidence format and accepted resource limits have
no implemented RP2.1 contract. Existing rendered-memory revision is not a
canonical source revision (`canonical_memory.rs:43-66,84-87`). A proposed
test must not derive its expected source inventory from projection output.

[plan]: ../../../../../../commons/docs/plans/2026-09-10-feat-eidnara-rp2-1-projection-coverage-plan.md
[index]: ../../../../../../commons/docs/plans/2026-09-10-eidnara-rp2-plan-index.md
[parent]: ../../../../../../commons/docs/plans/2026-09-08-0523-feat-eidnara-native-rust-cutover-plan.md
[n1]: ../../../../../../commons/docs/plans/2026-09-08-1614-feat-eidnara-rust-product-state-ownership-plan.md
