# RP2.1 prerequisite receipts

RP2.1 ([#347](https://github.com/ahrav/eidnara/issues/347)) names five parent
prerequisites and treats each as a precondition requiring evidence, not a
freshly verified claim. Ticket P1
([#352](https://github.com/ahrav/eidnara/issues/352)) adopts the existing
receipts without rerunning Stage 1 or any campaign. This document records what
receipt exists for each prerequisite, where it lives, and what it does not
prove. Inspection date: 2026-09-10, Eidnara HEAD
`913234433ae36a80a6e22c6aac14c7f9aab74386`.

Adoption is a receipt inventory. It grants no approval, publishes nothing, and
does not upgrade a receipt's status. A receipt marked `absent` or `partial`
stays a later gate owned by the named owner; the ticket's closure does not
depend on it.

## Commons sources consulted

Locators are relative to `/local/home/ahrav/scratch/commons/docs/plans/`. Each
SHA-256 covers the complete file. The Eidnara revision does not pin these
files.

| ID | Title and locator | SHA-256 |
| --- | --- | --- |
| R1 | Eidnara Repository Migration - Plan, `2026-09-01-0633-refactor-eidnara-repository-migration-plan.md` | `6267f5e86cc53e850e10547fdd6bd4dc4f2f6e2cae5ab55be8f4597b37cbbe2e` |
| R2 | Eidnara Native Rust and Cutover - Plan, `2026-09-08-0523-feat-eidnara-native-rust-cutover-plan.md` | `a6b787653eb159358784f5c080ee925cf70f195893a1daf5e316d443c8e1b3d5` |

R2 is the RP2.1 source register's S1. Its "Preconditions" section is the
authoritative list adopted here.

## Receipts

| Prerequisite | Receipt | Status | Limit |
| --- | --- | --- | --- |
| Migration U1-U5 complete | R1 change record marks the migration complete at the end of U5 and moves Retrieval Stage 2 to R2. In Eidnara, the U5 port tickets #167-#195 are closed, the five `@eidnara` packages exist under `packages/`, and `bun run check:repo` is the root repository check named by the plan. | adopted | The receipt is the plan's own completion statement plus closed tickets. No comparison against the predecessor repositories exists by design (R1, "No comparison of Eidnara vs sources"). |
| Stage 1 RP1.1 / RP1.2 / RP1.3 verified | R1 records that RP1.1 canonical state, RP1.2 daemon routes, and RP1.3 eligibility were verified in U4. In Eidnara, `crates/kernel/tests/stage1_canonical_state.rs` covers RP1.1, `crates/daemon/src/kernel_routes/` carries the RP1.2 routes, and `crates/daemon/src/kernel_routes/eligibility.rs` carries the RP1.3 `judge` policy. | adopted | Adoption does not rerun Stage 1. The eligibility policy is daemon-owned at this HEAD; its move into the kernel is P2 (#353), not a Stage 1 fact. |
| Fresh host reached by both harnesses | R1 U5 "Done when" requires adapters that load in both harnesses and reach the daemon's ported routes. In Eidnara, the CI job `native addon package (Bun)` step `Rust-mode E2E` runs the retained suite with `EIDNARA_E2E_REQUIRE_PI=1`: seventeen OpenCode Rust-mode tests, including `rust-smoke`, start a real OpenCode against `direct_host_fixture` on a fresh data root; `pi-smoke` loads the built Pi extension for one mock turn. | partial | OpenCode reachability is witnessed in CI. `pi-smoke` starts a Pi RPC process and the mock provider, checks tool registration, and asserts no storage file; it does not connect to `direct_host_fixture` or any daemon route, so Pi reachability has no Eidnara receipt beyond R1's completion statement. Neither test is a CC8 capability witness. |
| `1.0.0-rc.1` passed the owner-hosted release chain under dist-tag `rc` | None in Eidnara. No git tag or GitHub release exists at HEAD. `npm view` for all five `@eidnara` packages returned 404 on 2026-09-10. `bun run release:check` is absent from the root `package.json`. | absent | Owned by the release owner. The specification names release-owner reconciliation and the owner-hosted receipt as later acceptance and enablement gates, not P1 closure prerequisites. Nothing here waives them. |
| Source scopes frozen | R2 lists frozen source scopes as a precondition. For RP2.1 the frozen scopes are the class, identity, span, and capability contracts CC1-CC3 and CC8 in [construction-contracts.md](construction-contracts.md); the kernel test `search_projection_construction_inputs` pins their fixture form and the fixture records that exercise them. | adopted | Freezing is documentary. Source-policy and kernel owners supply the construction admission contracts the later tickets need; readiness is not approval. |

## Both-harness evidence

The both-harness receipt above is the reachability receipt. RP2.1 acceptance
additionally needs, for every required cell in the
[witness matrix](witness-matrix.md) whose scenario names a harness, a witness
from each of `opencode` and `pi`. Those witnesses do not exist yet and are not
P1 work. The capability dispositions in CC8 record what each harness must
support; `evidence_status` is `unwitnessed` for every capability.

## What P1 does not adopt

- No RP2.4 materialization, RP2.7 implementation, RP2.5 generation, or
  full-path RP2.9 result. P1 does not wait for them.
- No numeric limit. Every limit in CC9 is null until RP2.9 approves it.
- No campaign result. Executing the matrix is not a P1 closure prerequisite;
  missing, stale, skipped, unfired, or retroactively changed-limit evidence
  blocks later witness closure, which the coordinator owns through final RP2.9
  acceptance.

## Verification performed for this receipt

- Read R1 and R2 in full and hashed them.
- Confirmed the Eidnara paths named above exist at HEAD.
- Read `.github/workflows/ci.yml` and `packages/e2e-tests/README.md` for the
  retained suite and the Pi smoke's stated limits.
- Ran `git tag -l`, `gh release list`, and `npm view <package> dist-tags` for
  the five packages.

No product code ran. No campaign ran.
