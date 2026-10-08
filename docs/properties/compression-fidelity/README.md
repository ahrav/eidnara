# Compression fidelity property catalog

This index connects the generation, delivery, recovery, and evaluation claims
for compression fidelity. It records ownership and independent-review
disposition, not implementation completion or a semantic baseline.

Inspected source revision: `99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`.
Date: 2026-09-19. U1 ([#718](https://github.com/ahrav/eidnara/issues/718))
lands this part and re-verifies every `file:line` reference against its
branch tip: unchanged lines were renumbered, and moved or
rewritten code was relocated by hand. Two capability facts changed after the
inspected revision and are recorded where they apply: the hint footer no
longer promises full context (#926), and Pi folds history (#850).

All **28 records are active**. U1 partially exercises
[cf-source-obligation-independence](generation/catalog.md#cf-source-obligation-independence)
and [cf-corpus-byte-identity](evaluation/catalog.md#cf-corpus-byte-identity);
the other 26 are unexercised. Every listed existing check remains
**unaudited**. Source inspection, discovery, review, and documentation
validation do not establish that a property holds.

## Scope and provenance

The published specification is
[Compression Fidelity Contract](https://github.com/ahrav/eidnara/issues/707).
It records the settled contract and review resolutions. Publication occurred
once, on 2026-09-19 at 15:46:57 UTC. Readback confirmed the approved body:
52,448 characters, 28 property rows, state `OPEN`, and no labels or comments.
The published body SHA-256 is
`a0d629cd4b29fd3d6efdbd0b4cdac5a6ed03a3df64a49e37fa1c80dc57a52b86`.
The temporary publication preview was removed after issue creation.

The original **Compression Fidelity Contract - Plan**, dated 2026-09-19,
remains historical provenance for the settled input. Its SHA-256 is
`b33aaf8508174d39ea578cb6927d1524bbee1c3f2496c8a22511349beb35a384`; it records
inspection at `1555f00c702296406186f8c859045014e8e8a4d9`. Repository facts here
use the later source revision above. Neither working artifact is copied here.
No local plan or preview is required to read this catalog.

All plan line numbers in this catalog are historical provenance, not line
numbers in the published issue. Contract links open the stable issue; use the
named sections below rather than treating plan offsets as issue anchors.

| Original plan material | Published specification section |
| --- | --- |
| R1-R10 | Constraints and Invariants / Requirements |
| Tier obligations and C1-C6 | Materiality and minimum meaning by tier; Six acceptance cases |
| KTD1-KTD7 and stage observations | Implementation Decisions; Source and evidence flow |
| U1-U5 | Milestone boundaries and dependencies; Milestone completion |
| Acceptance and costs | Evidence and review gate; Resource and security invariants |
| Source provenance and 28 stable properties | Provenance and reusable catalog |

The evidence scope is answered: the supplied plan and local repository,
documentation, and history. No other incident evidence or related repository
was supplied. That describes the inputs; it is not a user prohibition on other
evidence. C1-C6 remain synthetic, risk-selected acceptance examples.

An independent review pre-evaluated the older daemon catalogs as having stale
CI claims and count/link drift. They supply leads and vocabulary only, not
inherited coverage. This catalog makes no broad repairs to them. Four fresh
discovery lanes produced these records. A separate independent review agent
evaluated harness fit, coverage balance, implementability, and wildcard risks.
The four portfolio files record that agent's findings and final dispositions,
not findings attributed to the user or a new review run.

[METHOD.md](../METHOD.md) governs the records. Its preferred in-repository
`_lenses/<lens>.md` retention conflicts with the root rule against committing
research artifacts. This catalog explicitly deviates from that placement:
raw lens paths are non-durable method notes in `/tmp/opencode/`, named
`compression-fidelity-{generation,delivery,recovery,evaluation}-lenses.md`.
The durable system-model/lens summaries, evidence files, and dispositions
retain substantive traceability without copying the research logs. Reading
this catalog does not require those temporary files to exist.

## Parts and stable records

| Part | Records / index rows / evidence files | Supporting artifacts |
| --- | --- | --- |
| [Generation](generation/catalog.md) | 8 / 8 / 8 | [Checks](generation/existing-checks.md), [faults](generation/fault-map.md), [review disposition](generation/portfolio-evaluation.md) |
| [Delivery](delivery/catalog.md) | 7 / 7 / 7 | [Checks](delivery/existing-checks.md), [faults](delivery/fault-map.md), [review disposition](delivery/portfolio-evaluation.md) |
| [Recovery](recovery/catalog.md) | 5 / 5 / 5 | [Checks](recovery/existing-checks.md), [faults](recovery/fault-map.md), [review disposition](recovery/portfolio-evaluation.md) |
| [Evaluation](evaluation/catalog.md) | 8 / 8 / 8 | [Checks](evaluation/existing-checks.md), [faults](evaluation/fault-map.md), [review disposition](evaluation/portfolio-evaluation.md) |

Each linked record defines its predicate, enabling state, evidence, exercise
status, and open prerequisites. These slugs, not table position, identify it.

| Part | Stable property |
| --- | --- |
| Generation | [cf-source-obligation-independence](generation/catalog.md#cf-source-obligation-independence) |
| Generation | [cf-producer-input-exposure](generation/catalog.md#cf-producer-input-exposure) |
| Generation | [cf-material-qualifier-fidelity](generation/catalog.md#cf-material-qualifier-fidelity) |
| Generation | [cf-semantic-evidence-separation](generation/catalog.md#cf-semantic-evidence-separation) |
| Generation | [cf-generation-stage-provenance](generation/catalog.md#cf-generation-stage-provenance) |
| Generation | [cf-material-generation-reachability](generation/catalog.md#cf-material-generation-reachability) |
| Generation | [cf-guidance-capability-bound](generation/catalog.md#cf-guidance-capability-bound) |
| Generation | [cf-witnessed-corrections](generation/catalog.md#cf-witnessed-corrections) |
| Delivery | [cf-tier-transitions-preserve-qualified-meaning](delivery/catalog.md#cf-tier-transitions-preserve-qualified-meaning) |
| Delivery | [cf-pressure-omission-discloses-evidence-loss](delivery/catalog.md#cf-pressure-omission-discloses-evidence-loss) |
| Delivery | [cf-memory-credit-requires-admitted-visible-content](delivery/catalog.md#cf-memory-credit-requires-admitted-visible-content) |
| Delivery | [cf-hints-do-not-strengthen-source-claims](delivery/catalog.md#cf-hints-do-not-strengthen-source-claims) |
| Delivery | [cf-delivery-credit-requires-published-folded-capture](delivery/catalog.md#cf-delivery-credit-requires-published-folded-capture) |
| Delivery | [cf-delivery-scenarios-reach-qualified-invocations](delivery/catalog.md#cf-delivery-scenarios-reach-qualified-invocations) |
| Delivery | [cf-serving-resource-boundaries](delivery/catalog.md#cf-serving-resource-boundaries) |
| Recovery | [cf-native-reopen-bytes](recovery/catalog.md#cf-native-reopen-bytes) |
| Recovery | [cf-exact-source-binding](recovery/catalog.md#cf-exact-source-binding) |
| Recovery | [cf-unavailable-evidence-no-credit](recovery/catalog.md#cf-unavailable-evidence-no-credit) |
| Recovery | [cf-visible-memory-recovery](recovery/catalog.md#cf-visible-memory-recovery) |
| Recovery | [cf-shipped-recovery-capabilities](recovery/catalog.md#cf-shipped-recovery-capabilities) |
| Evaluation | [cf-corpus-byte-identity](evaluation/catalog.md#cf-corpus-byte-identity) |
| Evaluation | [cf-fixture-script-qualification](evaluation/catalog.md#cf-fixture-script-qualification) |
| Evaluation | [cf-authoritative-evidence-assembly](evaluation/catalog.md#cf-authoritative-evidence-assembly) |
| Evaluation | [cf-complete-independent-comparison](evaluation/catalog.md#cf-complete-independent-comparison) |
| Evaluation | [cf-semantic-control-discrimination](evaluation/catalog.md#cf-semantic-control-discrimination) |
| Evaluation | [cf-evaluation-cost-completeness](evaluation/catalog.md#cf-evaluation-cost-completeness) |
| Evaluation | [cf-reviewed-semantic-batch-reached](evaluation/catalog.md#cf-reviewed-semantic-batch-reached) |
| Evaluation | [cf-bounded-record-and-forward](evaluation/catalog.md#cf-bounded-record-and-forward) |

### Type, semantics, and reachability

| Part | Safety / reachability | `always` / `sometimes` / `always-or-unreached` | `default-production` / `explicit-config-only` / `test-only` |
| --- | --- | --- | --- |
| Generation | 7 / 1 | 7 / 1 / 0 | 0 / 3 / 5 |
| Delivery | 6 / 1 | 6 / 1 / 0 | 5 / 0 / 2 |
| Recovery | 4 / 1 | 4 / 1 / 0 | 1 / 0 / 4 |
| Evaluation | 7 / 1 | 6 / 1 / 1 | 0 / 0 / 8 |
| Total | 24 / 4 | 23 / 4 / 1 | 6 / 3 / 19 |

No record claims unbounded liveness. Fault maps also name situation and
location checks; those are not extra catalog records. Report every declared
situation, required case, and subcase separately, including missing ones.
The five added generation markers and delivery's 14-marker set cannot be
replaced by one successful path.

Reachability labels describe each record's surface, not exercise or deployed
assertions. Absent evaluation/forwarding surfaces are `test-only`, even though
future live capture requires explicit configuration. That does not relabel
the underlying production generation, storage, renderer, or registry paths.
Production boundary documentation and the normative
[host wire contract](../../host-wire-protocol.md) remain distinct from these
test and evaluation obligations.

## Requirement, case, and milestone coverage

This is a coverage map, not a replacement specification or execution report.

| Requirement | Owning surface and join |
| --- | --- |
| R1: Qualified meaning | Generation owns input exposure and qualifier fidelity; delivery observes the effective capsule. |
| R2: Tier-safe loss | Generation and delivery cover headings, authored/fallback bodies, and tier loss; recovery owns omission disposition. |
| R3: Invocation-level disposition | Recovery owns exactly-one disposition and no-credit accounting; delivery supplies history, hints, memory, and tail. |
| R4: Exact native recovery | Recovery owns original-revision bytes, complete native binding, and consumer capability limits. |
| R5: Small independent corpus | Generation owns pre-candidate human obligations for C1-C6; evaluation owns shared byte identity. |
| R6: Served invocation | Delivery owns actual publication-to-provider observations and situations; generation supplies stage provenance. |
| R7: Evidence versus judgment | Generation separates scripted/structural evidence from meaning; evaluation binds independent judgments and controls. |
| R8: Complete comparison | Evaluation reconciles scenarios, attempts, outcomes, and human review; generation owns witnessed U5 correction acceptance. |
| R9: Cost and admission boundaries | Delivery owns serving-work restrictions and distinct budget surfaces; evaluation owns complete costs and bounded capture. |
| R10: Existing boundaries | All parts reuse private/test seams; recovery pins registered memory search and unsupported exact expansion; Pi folding shipped after the inspected revision and sits outside the corpus. |

| Case | Required focus and portfolio coverage |
| --- | --- |
| C1: Rejected design returns | Generation and delivery preserve rejection, rationale, and unconfirmed implementation; recovery accounts for omitted rationale. |
| C2: Unconfirmed deployment | Generation and delivery distinguish no receipt from a success receipt; evaluation includes both controls. |
| C3: Constraint mentioned once | Generation preserves scope, polarity, value, and unit; delivery pairs admitted memory with exclusion controls. |
| C4: Repeated misleading summary | Generation preserves primary evidence over repetition; delivery observes selected/truncated hints and recovery limitations. |
| C5: Final state differs from plan | Generation and delivery preserve supersession, partial/cancelled outcome, and inferred cause. |
| C6: Exact source | Recovery binds the original occurrence/revision and bytes after reopen; evaluation keeps internal success separate from unavailable consumer expansion. |

Every case requires P1-P4 serving and at least one pressure/omission scenario.
Focused additional scenarios cover fallback, hints, memory, and recovery. Six
cases require neither a Cartesian expansion nor pairwise machinery.

| Milestone | Dependency and catalog handoff |
| --- | --- |
| U1 | No predecessor. Generation supplies reviewed corpus obligations; evaluation supplies the shared byte-identity contract. |
| U2 | After U1. Generation joins input/parser/publication; delivery observes decay and pressure; recovery constructs original-revision C6. |
| U3 | After U1/U2. Evaluation owns initial ungated fixture qualification; delivery and recovery require their own invocation/tool observations. |
| U4 | After U2 and the qualified U3 publication/provider seam. Evaluation assembles complete deterministic, real semantic, control, and cost evidence. |
| U5 | U2/U3 precede deterministic text corrections; U4 precedes generated-meaning corrections. Generation owns named witnesses, reruns, and disclosed importance/serving shifts. |

## Ownership joins

- [Corpus byte identity](evaluation/catalog.md#cf-corpus-byte-identity) has one
  evaluation owner. Generation consumes it while owning native annotations;
  it does not implement another digest validator.
- [Initial fixture qualification](evaluation/catalog.md#cf-fixture-script-qualification)
  stays in U3 after U1/U2. The
  [delivery-credit record](delivery/catalog.md#cf-delivery-credit-requires-published-folded-capture)
  separately checks each invocation's replacement, application, nonempty
  correlated capture, and raw-tail isolation.
- [Disposition](recovery/catalog.md#cf-unavailable-evidence-no-credit) has one
  recovery owner. Delivery consumes it. Shared memory fixtures support distinct
  checks for admitted visible content and visible-argument tool recovery.
- [Evidence assembly](evaluation/catalog.md#cf-authoritative-evidence-assembly)
  stores each owner observation once. A derived comparison cannot override
  producer, publication, source-read, provider, or tool captures.
- [Serving restrictions](delivery/evidence/cf-serving-resource-boundaries.md)
  and [cost completeness](evaluation/evidence/cf-evaluation-cost-completeness.md)
  are complementary. A lower cost does not excuse forbidden production work.

## Outcome limits and open handoffs

Keep execution, deterministic result, preservation, recovery, consumer safety,
semantic review, and cost separate. A permitted P4 omission or explicitly
allowed abstention cannot earn preservation/recovery credit for unavailable
evidence. Losing the only relevant constraint remains a preservation failure.
Privileged native-byte access is not model-accessible recovery.

The independent review added generation situation markers, guidance/correction
ownership, semantic-control discrimination, serving restrictions, and cost
completeness. It refined hint-drop, C6, budget, and ownership claims without
changing settled scope. The part dispositions explain rejected suggestions to
reorder milestones, mandate blinding, add pairwise machinery, weaken the body
budget, or substitute C6's successor.

Open prerequisites are the reviewed corpus and semantic humans, full private
system/user/model capture and digest exchange, combined C6 execution, ungated
fold qualification, and the selected provider/model with fixed whole-loop
limits and complete cost accounting. They withhold evidence and acceptance;
they do not reopen permission to enlarge scope. No new source-access API,
admission policy, packing policy, production export, or framework follows.

Route all active records and their fault maps to `/testing:test-strategy`.
The part dispositions name the reusable seams. Existing-test adequacy belongs
to `/testing:invariant-test-review`; production guard strength belongs to
`/low-level-systems:defensive-assertions-and-invariant-guards`. No new
simulation framework is required by this catalog.

Six synthetic examples yield no reliability estimate. Harness self-checks do
not establish semantic success. Human control discrimination qualifies those
controls, not universal reviewer accuracy. Clean reopen proves neither crash
nor power-loss durability, and correctness checks prove no speed improvement.
All code-supported combined constructions remain unexecuted.
