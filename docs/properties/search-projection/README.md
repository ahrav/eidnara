# RP2.1 specification companions

This directory holds local evidence for one owner-approved specification:
[RP2.1: Rebuildable search projection and vector coverage](https://github.com/ahrav/eidnara/issues/347).
Fresh verification passed before the owner approved the complete preview and
publication. The published body matches the approved temporary body byte for
byte; the temporary file is removed. No implementation ticket or PR was created.

## Read this directory

| Document | Purpose |
| --- | --- |
| [Source register](source-register.md) | Authority, exact local source locators and hashes, revision, and limits of reused evidence. |
| [Testing strategy](testing-strategy.md) | Independent testing-owner recommendations, nine shared groups, oracles, negative controls, and unresolved seams. |
| [Specification traceability](spec-traceability.md) | R1-R5, KTD1-KTD4, milestone dependencies, gates, exclusions, landing amendment, and all 30 stable property slugs. |
| [Portfolio evaluation](portfolio-evaluation.md) | Central independent evaluation, qualified findings, dispositions, and remaining owner decisions. |
| [Verification receipt](verification.md) | Fresh verifier verdict, corrected findings, static checks, and evidence limits. |
| [Construction contracts](construction-contracts.md) | P1's frozen contracts CC1-CC12: classes, identity, spans, tuple encoding, payload identity, scanning, excluded inputs, capability dispositions, the RP2.9 limit-manifest interface, the hook-to-gate map, residue, and admission dimensions. |
| [Witness matrix](witness-matrix.md) | P1's nonempty required witness matrix over the 65 marker definitions, mapped to AC1-AC12 and T1-T9 with oracles, negative controls, and integration observations. |
| [Prerequisite receipts](prerequisite-receipts.md) | P1's adoption of the parent prerequisite receipts, including the receipt that is absent. |
| [Export and recovery catalog](export-recovery/catalog.md) | Ten properties for export, complete catch-up, ack ordering, selection, disable, and bounded recovery. |
| [Projection and coverage catalog](projection-coverage/catalog.md) | Eleven properties for local atomicity, identity, source inventory, raw fidelity, gates, coverage, conditional remediation, and campaign witnesses. |
| [Embedding catalog](embedding/catalog.md) | Eleven properties for exact preflight, durable pending/completion, bounded dispatch, identity fencing/GC, backfill, and physical ownership. |

Each part owns its `catalog.md`, `existing-checks.md`, `fault-map.md`,
`portfolio-evaluation.md`, and one `evidence/<slug>.md` per record. Stable slugs
identify claims, not implementation tasks. Use the catalogs for exact METHOD
fields and required faults; the root documents coordinate rather than replace
those records.

## Status and authority

Source inspection is pinned to Eidnara HEAD
`913234433ae36a80a6e22c6aac14c7f9aab74386` on 2026-09-10. The companions and
catalogs were authored as uncommitted working-tree artifacts against that
commit. Ticket P1 ([#352](https://github.com/ahrav/eidnara/issues/352)) carries
them into the repository unchanged, together with the three P1 documents above,
the fixtures under `crates/kernel/tests/fixtures/search-projection/`, and the kernel test
that checks them. The hashes in the source register describe the catalog files
as they are. External Commons files have separate content hashes because the
Eidnara revision cannot pin them.

Authority runs from the Stage 2 parent to the settled RP2.1 plan and shared
index, then implementation evidence. The owner's amendment changes only the
one-unit/one-PR assumption: milestones and dependencies remain, later landings
may split for reviewability, and each PR has independent size, review, and
verification obligations. It does not authorize tickets in this task.

There are 30 active proposed records: 25 safety, four bounded liveness, and one
reachability. Each is individually classified `test-only` and unexercised.
`Active` means an obligation belongs in the specification, not that product
behavior exists. Existing checks remain `unaudited`; source inspection does
not upgrade their adequacy or execution status. The three fault maps define
23, 24, and 18 markers respectively. Marker definitions are not witnesses.

The coordinator commissioned the central analyst evaluation and the independent
testing-owner report. These are analyst evidence, not user-supplied runtime
evidence. Older per-part references to findings being supplied through a user
describe the handoff text, not authorship; [the evaluation](portfolio-evaluation.md)
records the controlling attribution and the narrow correction made here.

One specification issue is published. The specification task itself produced
no product code, tests, builds, benchmarks, implementation tickets, commits, or
PRs. P1 adds documentation, test fixtures, one kernel integration test, and two
CI path-filter entries; it changes no product code. The test
`search_projection_construction_inputs` extracts the marker definitions from the three fault
maps and checks the fixtures and the witness matrix against them. Static
artifact checks do not satisfy
implementation acceptance. Numeric RP2.9 approval, source/persistence decisions,
and release-gate reconciliation remain required; the
[prerequisite receipts](prerequisite-receipts.md) record which parent
prerequisite evidence exists.
