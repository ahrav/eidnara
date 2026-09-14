# Typed wire decode property portfolio

This portfolio is reusable input to the
[typed wire decode specification](../../specifications/typed-wire-decode.md).
It catalogs obligations for the
[settled plan](../../plans/2026-09-13-0104-perf-typed-wire-decode-plan.md),
not evidence that its replacement has passed them. No implementation tickets,
source changes, benchmarks, or implementation tests are part of this work.

## Publication receipt

The owner approved the complete specification and testing seams. One open
specification was created at
[GitHub issue #556](https://github.com/ahrav/eidnara/issues/556).
Read-back verification confirms the exact 37,630-byte body and SHA-256
`6b581435925b805a30423ebb643dd0f42acdae75fb49acdf7f9c2ce22929f3ba`,
the approved title, and work-item marker
`typed-wire-decode-83443140-3a3e-4e03-a486-ce46b1405d3f`.
No labels, assignees, milestone, or comments were added. No implementation
tickets were created. The local specification is an exact copy of the
published body. Open implementation questions remain open.

## Scope and evidence

The owner supplied the plan as settled material and required discovery for
each nontrivial behavioral surface. The external-evidence scope is that plan,
its repository contracts, existing property catalogs, and linked GitHub
issues/comments. No separate incident reports or external repositories were
supplied. Individual parts record which leads were read and their limits.

Source evidence is pinned to
`2e4433e6b511ae74944df8a9669c428e73915d29`. The comparison baseline is
`e451a2b470ae8663b4613ca04f019a30b6d7df53`. The accepted plan's SHA-256 is
`badf0d718366bd627d453498576935ba2fd3292cfe5701b7020fa09a07c52f32`.
Dirty worktree files and neighboring untracked catalogs are not treated as
committed source. Source references were checked against the pinned HEAD,
not copied from stale plan offsets.

## Parts

| Surface | Catalog | Checks | Fault map | Independent evaluation |
| --- | --- | --- | --- | --- |
| Decode, routing, owned prefixes, typed mutation | [7 active records](decode/catalog.md) | [Inventory](decode/existing-checks.md) | [Faults](decode/fault-map.md) | [Disposition](decode/portfolio-evaluation.md) |
| Canonical bytes, receipts, durable history and policy | [11 active records](identity/catalog.md) | [Inventory](identity/existing-checks.md) | [Faults](identity/fault-map.md) | [Disposition](identity/portfolio-evaluation.md) |
| Admission, peaks, retained ownership and allocation | [6 active and 1 invalidated records](resources/catalog.md) | [Inventory](resources/existing-checks.md) | [Faults](resources/fault-map.md) | [Disposition](resources/portfolio-evaluation.md) |

Resources keeps its [source register](resources/source-register.md) and
[handoffs](resources/handoff.md) in separate files; the other parts keep those
sections in their catalogs. This is an organizational difference, not a
different evidence requirement.

Each part retains all twelve system-model and eleven property lens passes in
`_lenses/`; wildcard passes run last. Identity adds three targeted discovery
passes after independent evaluation. Each property has its own evidence file,
exact check semantics, reachability class, enabling state, confidence, existing
check status, impact, and open questions. Each has a named test/verification
handoff. Same-source rediscovery is not independent corroboration.

The portfolio has **24 active properties**, **one invalidated category record**,
and **one required performance evidence gate**. The invalidated record is
retained rather than deleted. Its obligation lives in
[EG1: decode-projection payoff](resources/evidence-gates.md); this does not
reactivate the separately invalidated latency-audit W1 record.

The typed-wire U1 branch (`perf/typed-wire-u1-owned-decode`) exercised the
portfolio: each catalog's `Exercised:` line names the tests that ran, and the
per-record evidence files carry a "Typed-wire U1 execution, 2026-09-13" section
with the measured values. EG1 holds both legs and a `proceed` verdict. Records
marked partial name what remains unconstructed. Existing checks beyond the
named tests are unaudited.
No new liveness deadline is justified by this synchronous decode and
serialization change. Every fixed situation marker has an independent result;
aggregate reports are summaries only. The evidence receipt for four benchmark
cells is not a runtime reachability property.
Marker names retain their part-specific spelling. Treat each exact name as
an identifier; never infer the portfolio's membership from a naming pattern.
The fault maps enumerate all 26 independent situation markers and the separate
EG1 evidence receipt.

## Specification traceability

| Plan obligation | Property slugs and evidence |
| --- | --- |
| R1, KTD1, U1 | `envelope-decode-has-no-retained-tree`; `typed-mutation-is-visible-and-copy-isolated` in decode |
| R2, KTD5, U1/U2 | `typed-failure-preserves-tree-outcome`; `page-and-other-routes-retain-tree-semantics`; `nested-duplicate-fallback-is-exercised` in decode |
| R3, KTD3, U1 | `unknown-envelope-fields-do-not-erase-payload-values`; `decoded-snapshots-own-and-share-prefixes` in decode |
| R4, KTD2, U2 | `plugin-block-canonical-identity-preserved`; `sibling-mutation-preserves-untouched-bytes` in identity |
| R5, KTD2, U2 | `served-default-omissions-are-bounded`; `block-byte-policy-outcomes-remain-stable` in identity |
| R6, U2/U4 | `historical-chunks-retain-readable-identity`; `durable-identity-domains-survive-cache-reset`; `durable-hygiene-baseline-preserves-content-identity`; `durable-lineage-anchor-preserves-validation` in identity |
| R7, KTD2, U2 | `block-byte-consumers-share-canonical-basis`; `typed-equality-governs-receipt-reuse` in identity |
| R8, KTD4, U1/U4 | `decode-footprint-covers-both-lanes-combined-peak`; `frozen-admission-outcomes-and-boundaries-stay-stable`; `decode-and-projection-stay-within-resident-pool` in resources |
| R9, KTD4, U1 | `retained-accounting-follows-typed-ownership` in resources |
| R10, U4 | `message-decode-allocation-gate-has-isolated-scope` in resources |
| R11, U4 | EG1 in resources; W1 status reconciliation remains an owner question |
| Nonvacuity across units | `nested-duplicate-fallback-is-exercised`, `identity-edge-states-are-exercised`, `resource-witnesses-reach-independent-preconditions` |
| Stop conditions | Plugin byte preservation; original A1-A3 outcomes; EG1's whole-plan stop within noise; unchanged normative wire surface |

## Review and disposition

The domain routes were selected through the installed `ask-skills` contract:
property discovery for all three surfaces, Rust design review for ownership
and compatibility shape, and test strategy for observable seams and oracles.
The fresh portfolio evaluator is independent of the discoverers. It ran
harness fit, risk coverage, implementability, and wildcard lenses.

Its findings produced targeted durable-hygiene, lineage-anchor, and downstream
policy records; independent typed-only shape markers; a partitioned admission
contract; and separation of measurement evidence from runtime properties.
Per-part evaluation files retain findings and dispositions. These are catalog
reviews, not the owner's required five parallel reviews of every future PR.

The specification preserves unresolved owner decisions on golden partitions,
legacy durable state, history_summarizer reachability, raw-token behavior, W1 status,
frozen witness retuning, allocation attribution, pool coverage, and neighboring
exact-ingress expectations. A catalog entry is a claim under test, not a
compatibility waiver. Publication approval does not mark these properties
exercised or authorize implementation tickets.

## Reuse contract

Before implementation, read the specification, the relevant record, its
evidence, and its fault-map row. Resolve the stated owner decisions without
silently weakening the plan. Select a test form through the named handoff.
Use frozen baseline bytes for field-set preservation, independent literal
expectations for receipt selection, and continuous peak measurement for memory.

After execution, update the owning existing property record and this
plan-specific evidence with the exact artifact, commands, result, and witness
state. Preserve invalidated history and failed or unfired cases. Do not inherit
exercise status from a neighboring catalog or a same-helper equality test.
