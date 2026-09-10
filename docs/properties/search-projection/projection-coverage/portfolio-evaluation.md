# RP2.1 projection-coverage portfolio evaluation and dispositions

## Provenance and authority

System: `/local/home/ahrav/scratch/eidnara`.
Verified source HEAD: `913234433ae36a80a6e22c6aac14c7f9aab74386`.
Date: 2026-09-10. Plan/source provenance is in
[_lenses/model.md](_lenses/model.md#source-register).

The coordinator commissioned the completed whole-portfolio fresh independent
evaluation from analyst `ses_f7623dcccffe3Y09nW2wVoABif` and supplied its findings,
qualifications, and coordinator decisions. This is analyst evaluation, not user
evidence. This document records the findings assigned to projection-coverage
and their local dispositions. The edit pass checks cited source and artifact
consistency; it is not another independent review or acceptance campaign.
No incident, test run, numerical approval or runtime evidence is added.

## Evaluation lenses represented by the supplied findings

| Lens | Finding focus and evidence |
| --- | --- |
| Harness fit | G3 identifies reusable CAS process termination/barrier/reopen machinery. R1 removes a duplicate cross-store ack oracle in favor of its export/recovery owner. |
| Coverage balance | G1 adds conditional remediation-byte currentness; G2 gives finite normal catch-up and authorized recovery one owner; G4 and B1/B2 make completeness and all required scenario witnesses binding. |
| Implementability | R3 distinguishes the proposed RP2.9 gate from live absent-route behavior. R4/R7 fix index and test-location descriptions. G6 retains source-policy/at-rest decisions as blockers. |
| Wildcard | B4 requires durable legacy-catalog currency caveats. G1's qualification rejects an inferred projection dependency and inferred equality of all embedding-key fields. |

These are the supplied issues and dispositions, not a claim that this writer
reran the evaluator's four lenses independently.

## Findings and dispositions

| Finding | Class | Disposition and evidence |
| --- | --- | --- |
| G1: remediation can change canonical bytes without source revision | gap, qualified | Added [projection-remediation-invalidates-derived-bytes][remediation]. `crates/kernel/src/envelope.rs:395-438` rewrites only `domains.name` and emits `operator_remediation`; no RP2.1 input dependence is established. The record requires approved bounded mapping and independently reconstructed current bytes/hash. It does not assume all occurrence or embedding-key fields agree, mandate a generation/revision, or invent an erasure SLA. |
| G2: ordinary catch-up and authorized recovery need finite convergence | gap, cross-part owner | Linked [catchup-and-authorized-recovery-converge][progress]. Export/recovery owns progress and `Current` outcomes; RP2.9 owns numeric bounds. Projection completeness and the root witness matrix consume those outcomes without another liveness algorithm. |
| G3: crash substrate exists even though product hooks do not | refinement | Inventoried `cas_fault_injection.rs:1046-1094` kill/barrier/reap and `:350-388,924-990` reopened-state comparison as unaudited. Search/embedding product hooks and oracles remain missing. No power-loss proof or RP2.9 timeout is inferred from the CAS tests. |
| G4: offered source input does not reach completeness antecedent | gap | Added `search_projection_complete_declared_nonempty_inventory`: a product completeness declaration plus known nonempty independent bounded inventory. Correctness of its contents is a separate safety check. |
| R1: local atomicity duplicated ack bounds and lock order | refinement | Local atomic CHECK now covers rows/checkpoint/pending only. [ack-follows-local-release][ack-owner] is sole ack/order owner. Duplicate local ack markers are removed; references consume canonical lowercase names from [export fault map][export-map]. Local precommit kill remains here. |
| R3: evidence gate classified as live absent-route behavior | refinement | `projection-n13-hooks-stay-gated` is `test-only`, `Exercised: not yet`. Existing route/config tests are adjacent unaudited checks, not gate exercise. Missing, failed and unsupported/inapplicable evidence have separate scenario markers. |
| R4: index omitted confidence | refinement | Index now uses `Slug | Type | Reachability | Semantics | Status | Confidence`, matching each record. |
| R7: test location described as reader | refinement | `canonical_memory.rs:282-312` is explicitly a test; `:141-212` is the reader. Neither is a full source oracle. |
| B1/B2: acceptance could waive or merely count required situations | bias, coordinator decision applied | Added one [projection-acceptance-situations-witnessed][acceptance] reachability record, `sometimes` over a completed approved campaign's whole frozen matrix. Five classes, nonempty completeness declaration, certified inference offered input, actual crash boundaries and required `Current` states are mandatory for the supported scope. Unknown/skipped/unfired required cells fail. |
| B4: legacy-catalog caveats need a durable inventory home | bias, refinement applied | [existing-checks.md#legacy-catalog-currency-and-policy-limits][legacy] records historical citations/exercise claims, unverified storage-side references, invalidated mirror subjects and the difference between codec value equality and raw-byte fidelity. |
| G6: sensitivity/at-rest source policy unresolved | gap, owner decision retained | The catalog, inventory and remediation evidence retain the policy question. No sensitive-source exclusion, persistence permission or universal cleanup deadline is added. Source selection and residue policy need explicit owner approval. |

## G1 factual boundary

The exact affected field witness is `domains.name`; the target enum at
`crates/kernel/src/envelope.rs:240-242` has only `CanonicalDomainName`.
The implementation loads object metadata, rewrites the name, and appends the
loaded object to an `operator_remediation` change without a source-revision
update. Canonical tests at `kernel_retention.rs:395-466,743-814` assert the name
replacement and audit behavior, not retrieval dependence or full key equality.

The embedding catalog includes exact input hash in work identity. A fresh
current-input hash can detect changed bytes; alternatively, the approved source
mapping may not use the affected field at all. Those competing explanations
prevent the analyst's stronger inferred failure from becoming a factual premise.
The new record is conditional on demonstrated mapping dependence, with exact
byte comparison and bounded independently specified input construction.

## Binding scope decision

Required acceptance has no arbitrary optional exemption. The coordinator fixes
the nonempty required marker/scenario matrix before execution using all three
maps. Constant marker names carry bounded scenario identities. The root marker
is not recursively required as a cell of its own matrix.

Clearly unrelated optional checks may be excluded only with declared scope.
Unsupported optional external lanes stay disabled and cannot replace required
supported-path evidence. An unresolved conditional mapping is a blocker, not
an implicit exclusion. A justified false premise is recorded as applicability,
not as a passing witness. This decision creates a reachability obligation,
not a coverage metric or a proof that associated safety checks hold.

## Remaining specification-boundary blockers

- Canonical/source owners must approve bounded field-to-input mapping, including
  whether remediation affects any selected RP2.1 source and how current inputs
  are reconstructed independently of derived state.
- Source-policy owners must define sensitivity admission and at-rest residue
  handling. Exact raw preservation and egress checks do not settle those choices.
- Projection/embedding owners must settle occurrence encoding, payload collision
  response and current-input hash association without assuming new generations.
- The coordinator must freeze hook-to-gate and marker/scenario manifests, then
  provide product observers and completed witness records.
- RP2.9 must approve all numeric resource and finite recovery parameters. The
  progress record's presence is not numerical approval or runtime exercise.

## Verification and evidence status

The catalog has eleven records and eleven matching evidence files: ten safety
records with `always`, one reachability record with `sometimes`. All are
`test-only` and unexercised at the product boundary. Local source citations are
checked against the named HEAD; cross-part catalogs/maps are working-tree
artifacts coordinated with their owners. METHOD fields, index/evidence equality
and links are mechanically checked after edits. No source, tests, builds or
tracker operations are part of this pass.

The independent evaluation is complete as supplied. Local dispositions address
this directory's assigned findings; unresolved design approvals remain explicit
and no completed acceptance campaign is claimed.

[remediation]: catalog.md#projection-remediation-invalidates-derived-bytes
[acceptance]: catalog.md#projection-acceptance-situations-witnessed
[ack-owner]: ../export-recovery/catalog.md#ack-follows-local-release
[progress]: ../export-recovery/catalog.md#catchup-and-authorized-recovery-converge
[export-map]: ../export-recovery/fault-map.md
[legacy]: existing-checks.md#legacy-catalog-currency-and-policy-limits
