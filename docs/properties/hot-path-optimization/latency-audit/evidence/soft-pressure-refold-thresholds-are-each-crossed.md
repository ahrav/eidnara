# soft-pressure-refold-thresholds-are-each-crossed

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

W9 is a differential over a predicate with three disjuncts. Small fixtures
keep m1 short, budgets large, and update counts at zero, so every SOFT pass
takes the ordinary branch and a candidate that mis-classifies at a boundary
is never distinguished from the reference. The parent's
[portfolio evaluation](../../portfolio-evaluation.md#gaps-queued) asked for a
`sometimes` witness per threshold beside the safety record.

## Evidence trail

- The three disjuncts and their inputs are at [`:4321-4327`][soft-predicate]:
  `m1.memory_update_count > 40`; `m1_has_content && m1_tokens as f64 >
  history_budget_tokens * 0.20 && history_budget_tokens > 0.0`;
  `m1_has_content && m0_tokens >= 500 && m1_tokens as f64 > m0_tokens as f64
  * 0.15`.
- `m0_tokens` is the direct count of the frozen unit with key `m0`
  ([`:4309-4314`][m0-count]); the HARD arm writes that unit at
  [`:4228`][m0-unit] and the refold branch rewrites it at
  [`:4418`][m0-unit-refold]. `m1_tokens` is the direct count of
  `m1.body` when it differs from [`M1_PLACEHOLDER`][placeholder]
  ([`:4315-4320`][m1-count]).
- `m1.body` has content when [`compose_m160-237`][m1-compose] renders at least one
  of: new compartments past `folded_compartment_seq`, a changed user profile
  under `memory_enabled`, or newly claimed notes ([`:174-222`][m1-pieces]).
- `history_budget_tokens` is the request value filtered to finite and
  `>= 0.0`, else the binding's frozen budget
  ([`lib.rs:8239-8242`][budget-filter]); a small positive budget is reachable
  from the request.
- `memory_update_count` is `0` on every composition
  ([`m1_compose.rs:231`][m1-count-zero]); no other writer exists in the
  workspace, so the first disjunct is never true at HEAD.
- The SOFT plan is chosen by the classifier at [`:3806-3824`][plan-select];
  a subagent request is forced to SOFT or Defer, and `force_hard` forces HARD.

## Failure scenario

Not a violation; a coverage gap. A campaign of SOFT passes whose m1 stays a
few lines, whose budget is 60_000 tokens, and whose m0 is under 500 tokens
never evaluates a disjunct near its boundary, so W9 passes for a candidate
that rounds `m1_tokens` or reads a stale cached count.

## Timing windows and dependencies

None in time. Each marker depends on session state built by earlier passes:
compartments folded into a frozen m0 of at least 500 tokens, then new
compartments or notes that give m1 content, then a SOFT pass.

## What a test must construct

Four constant markers, each recording the independently measured inputs, not
`pressure_refold`:

- `...-updates`: `m1.memory_update_count > 40` with the other two false. Not
  constructible at HEAD; see the log.
- `...-budget-share`: content, `history_budget_tokens > 0`, and `m1_tokens >
  history_budget_tokens * 0.20`, with `m0_tokens < 500` or the ratio false. A
  request with a small positive `history_budget_tokens` and an m1 body of a
  few hundred tokens reaches it.
- `...-m0-ratio`: content, `m0_tokens >= 500`, and `m1_tokens > m0_tokens *
  0.15`, with the share false. A HARD pass over enough covered messages to
  freeze a 500-token m0, then compartments or notes that render an m1 above
  15 percent of it, under a large budget.
- `...-below`: a SOFT pass with content where all three are false.

Direct `tokenizer::estimate_tokens` counts of the frozen m0 payload and the
m1 body must be recorded before the candidate runs. No existing check records
any threshold crossing.

The `-updates` marker needs a writer for `memory_update_count` that the
specification introduces; at HEAD the field is the constant `0`
(`crates/daemon/src/m1_compose.rs:231`) and has no other writer.

## Investigation log

### Q: Can the `-updates` marker fire at HEAD?

- Sources examined: [`M1Composition`][m1-struct],
  [`compose_m1:231`][m1-count-zero], a workspace search for
  `memory_update_count`.
- Findings: The field is written once, as `0`, and read once, in the
  predicate. The record's required state, "a session with more than 40
  memory updates since the last fold", has no production or test path that
  produces it. The marker can fire only after a specification gives the
  field a writer, or through a fixture that constructs `M1Composition`
  directly, which bypasses the SOFT arm the marker is meant to witness.
- Missing evidence: The intended source of the count.
- Conclusion: needs human input - keep the marker for a specification-era
  writer, or drop the disjunct from W9's reference and this record.

## Implementation evidence and marker retirement

The preceding discovery snapshot is retained at its stated baseline. Current
checks and explicit decision provenance are in [shared selection and pressure
accounting](shared-selection-and-pressure-accounting.md).

The owner decision retires
`soft-pressure-refold-thresholds-are-each-crossed-updates` together with the
dead production disjunct. The marker is not exercised and no new writer is
required. This resolves the discovery question; its historical text above is
not an active requirement.

The other three constant markers are asserted by the 48-case real-store
threshold test. They observe budget-only pressure, ratio-only pressure, and
neither condition from direct counts before the candidate runs. The m0
payload is constructed at 499/500 tokens; m1 is genuinely composed from
stored compartments at 74/75/76 tokens. The test drives a SOFT plan and checks
its SOFT-or-HARD result. This is bounded characterization, not evidence that
these shapes represent production traffic.

[plan-select]: ../../../../../crates/daemon/src/transform.rs#L3806-L3824
[m0-unit]: ../../../../../crates/daemon/src/transform.rs#L4228
[m0-unit-refold]: ../../../../../crates/daemon/src/transform.rs#L4418
[m0-count]: ../../../../../crates/daemon/src/transform.rs#L4309-L4314
[m1-count]: ../../../../../crates/daemon/src/transform.rs#L4315-L4320
[soft-predicate]: ../../../../../crates/daemon/src/transform.rs#L4321-L4327
[m1-struct]: ../../../../../crates/daemon/src/m1_compose.rs#L92-L101
[m1-compose]: ../../../../../crates/daemon/src/m1_compose.rs#L160-L237
[m1-pieces]: ../../../../../crates/daemon/src/m1_compose.rs#L174-L222
[m1-count-zero]: ../../../../../crates/daemon/src/m1_compose.rs#L231
[placeholder]: ../../../../../crates/daemon/src/memory_render.rs#L10-L12
[budget-filter]: ../../../../../crates/daemon/src/lib.rs#L8239-L8242
