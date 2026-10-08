# Delivery portfolio evaluation

This file records the independent review agent's findings and final
dispositions, not a new discovery pass or semantic verdict. Source revision:
`99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`, inspected 2026-09-19.
The [catalog index](../README.md#scope-and-provenance) records supplied scope,
older-catalog limitations, and the explicit `_lenses` placement deviation.

## Portfolio disposition

The [catalog](catalog.md) contains seven active, unexercised records and seven
matching evidence files. Six are `always` safety records; one is a `sometimes`
reachability record. Five surfaces are `default-production`, and two are
`test-only`. The labels do not claim deployed semantic checks. All
[existing checks](existing-checks.md) remain unaudited.

Final records incorporate the review refinements. Actual ungated publication,
provider delivery, and semantic judgments remain prerequisites, not established
results. No smaller request or safe answer substitutes for those observations.

## Independent findings and dispositions

### Harness fit

- **Refinement applied: split initial and per-invocation qualification.**
  Evaluation owns
  [cf-fixture-script-qualification](../evaluation/evidence/cf-fixture-script-qualification.md)
  inside U3 after U1/U2.
  [cf-delivery-credit-requires-published-folded-capture](evidence/cf-delivery-credit-requires-published-folded-capture.md)
  consumes that receipt and checks each invocation's accepted range,
  replacement, recipe application, nonempty correlated capture, and raw-tail
  isolation. Neither backend counters nor an initial receipt grant later
  compression credit. Missing captures and invalid-recipe raw pass-through
  cannot qualify delivery.
- **Gap retained for execution: m1 and ungated folds.** A first fold can go
  straight to m0. The
  [situation evidence](evidence/cf-delivery-scenarios-reach-qualified-invocations.md)
  requires a prior baseline and later publication for m1, followed by m0 and
  warm-repeat observations. A gated, quarantined, or skipped lane cannot close U3.

### Coverage balance

- **Refinement applied: report each situation.** The
  [fault map](fault-map.md#independent-coverage-checks-to-add) requires separate
  outcomes for all 14 markers and every required case/subcase. Coverage includes
  fallback, legacy rows, empty P4, natural/pressure omission, memory exclusions,
  hints, raw fallback, and warm repeats. A single umbrella success is insufficient.
- **Refinement applied: shared disposition, distinct memory claims.** Delivery
  consumes recovery's
  [exactly-one/no-credit predicate](../recovery/evidence/cf-unavailable-evidence-no-credit.md).
  [Memory admission and actual inclusion](evidence/cf-memory-credit-requires-admitted-visible-content.md)
  share a fixture with registered memory recovery, but they remain distinct
  assertions. A permitted P4 omission or explicitly allowed abstention never
  earns credit for unavailable evidence; required-preservation loss stays visible.
- **Suggestion rejected: pairwise machinery.** C1-C6 require focused P1-P4 and
  pressure/omission scenarios. The settled scope needs neither pairwise reduction
  nor a Cartesian expansion. The fault map retains explicit focused variants.

### Implementability

- **Gap incorporated: serving restrictions have an owner.**
  [cf-serving-resource-boundaries](evidence/cf-serving-resource-boundaries.md)
  preserves budgets, admission, cache behavior, and hint gates/limits. It forbids
  feature-added per-transform semantic judging, native rereads, or storage queries.
  Evaluation separately owns cost completeness; cheaper output cannot justify
  forbidden production work.
- **Suggestion rejected: a blanket 105% body budget.** The final resource
  evidence resolves three different surfaces, without weakening the contract:
  1. A positive effective budget is a hard bound on the unwrapped history body,
     which must fit or be empty. Nonpositive budgets do not witness pressure.
  2. The wrapped history slice retries above 105% at most three additional
     times, then returns the last render. This is best effort, not a cap or a
     body-budget allowance. The empty wrapper remains for cache behavior.
  3. Final invocation admission uses a separate heuristic with 25% headroom.
     It declines known-limit, over-limit growth; `limit_unknown` and `shrinks`
     can accept. It does not guarantee an absolute whole-invocation cap.

  Measure these surfaces separately with estimator identities. Do not add Rust
  and OpenCode token estimates. The combined measurements remain unexecuted.

### Wildcard and bias

- **Refinement applied: precise whole-fragment drop.** The final
  [hint evidence](evidence/cf-hints-do-not-strengthen-source-claims.md) checks
  formatted `- ` emptiness: `line.len() <= 2` drops empty normalized content,
  not all nonempty one- or two-character snippets. Long-fragment truncation and
  whole-fragment drop have separate outcomes. Construct real selection with
  distractors and an eligible tail; do not weaken the query gate or demand the
  unreachable total-cap situation under the inspected fixed shape.
- **Bias surfaced: visibility is not meaning.** Distinctive tier bodies prove
  rendering identity, not semantic preservation. Raw-tail leakage, hidden memory,
  or a missing hint can mask loss. Review the complete capsule and invocation
  against independent source obligations. Six synthetic cases yield no rate
  estimate, and a harness pass does not establish fidelity.

## Open prerequisites and test-strategy handoff

Route all seven records and the fault map to `/testing:test-strategy`:

| Record group | Reusable seam and required evidence |
| --- | --- |
| Tier meaning, pressure, and situations | U2 private publication/transform replay plus focused existing renderer checks; U3 provider-visible observations for each required situation. |
| Memory and hints | U3 pinned canonical reads, admission/exclusion pairs, actual hint selection, and full invocation capture; humans judge preserved qualifiers. |
| Delivery credit | U3 evaluation-owned fixture receipt plus each invocation's application, range replacement, capture, and tail evidence. |
| Serving resources | Existing body/wrapper/invocation, hint-query, memory, and cache checks plus a source audit of added production work; evaluation stores costs once. |

Open prerequisites include reviewed obligations and abstention cases, an
independently eligible memory example, full captures, fixed replay bounds, and
actual ungated folds. Broader runtime dependencies block U3 rather than permit
scope expansion. No packing/admission change, automatic memory promotion,
production export, or new retrieval surface is authorized. Existing-test
adequacy belongs to `/testing:invariant-test-review`; guard strength belongs
to `/low-level-systems:defensive-assertions-and-invariant-guards`.
