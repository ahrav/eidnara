# Property catalog: write-time contradiction candidates (P5)

## Provenance

Plan: P5
`docs/plans/2026-10-24-adaptive-discovery/05-write-time-contradiction-candidates-plan.md`.
Plan baseline: `a0e3f2993`. Authored at HEAD `3a3b916b7`, 2026-10-24.
Tracker spec: `https://github.com/ahrav/eidnara/issues/1163`.
Method: `docs/properties/METHOD.md`.

## Parts

`dependencies/` is a property-discovery pass over the eight implemented
surfaces the P5 mechanism consumes (analyzer, scope-filtered decision read,
budgeted Kernel reads, source-capture publication, memory-store baseline,
harness-capture retention, routed `review.*` reads and `kernel.read`,
`eidnara_memory` action surface). Its records are discovered facts about code
at HEAD with evidence files, existing checks, a fault map, a system model,
and an independent portfolio evaluation whose refinements R1 to R8 are
applied (with one correction, recorded in its section 9). Slug prefix
`p5dep-`.

`proposed/` is the invariant-driven domain-modeling output for the unbuilt
P5 behavior: 15 proposed obligation records (`Status: active - PROPOSED`),
the method's certificate, and the open questions and plan-versus-HEAD
conflicts the specification must resolve. Slug prefix `cc-`. Nothing in this
part is implementation evidence.

External-evidence scope (user-confirmed): the plan set (00-index, P4, P5
plans), tracker specs #1145 and #1159 (local copies), repository code, tests,
`docs/host-wire-protocol.md`, `docs/memory-reviewer-operations.md`,
`docs/search-projection.md`, and `docs/properties/` at HEAD. No other tracker
issues, no incident history, no git-history lens.

`_lenses/` working material is not included: discovery ran as one pass
without separate lens files.

## Dependencies index (`dependencies/catalog.md`)

| Slug | Surface | Type | Semantics | Reachability |
| --- | --- | --- | --- | --- |
| p5dep-analyzer-refuses-oversize-input-before-retention | S1 | safety | always | default-production |
| p5dep-analyzer-keeps-original-bytes-and-is-pure | S1 | safety | always | default-production |
| p5dep-analysis-epoch-and-identity-pin-the-contract | S1 | safety | always | default-production |
| p5dep-domain-decision-read-matches-domain-and-kind-in-sql | S2 | safety | always | default-production |
| p5dep-scope-prefilter-is-a-superset-of-the-scope-verdict | S2 | safety | always | default-production |
| p5dep-daemon-bounds-rows-after-the-scope-filter | S2 | safety | always | default-production |
| p5dep-kernel-domain-read-has-no-row-bound | S2 | reachability | sometimes | default-production |
| p5dep-claim-fact-bounds-are-refused-before-any-row-is-read | S3 | safety | always | default-production |
| p5dep-budget-exhaustion-stops-a-budgeted-kernel-read | S3 | safety | always | default-production |
| p5dep-decision-page-returns-an-oversize-decision-alone | S3 | safety | always | default-production |
| p5dep-response-never-waits-on-the-capture-job | S4 | safety | always | explicit-config-only |
| p5dep-capture-slot-is-single-and-released-on-every-exit | S4 | safety | always | explicit-config-only |
| p5dep-capture-batch-is-bounded-per-batch | S4 | safety | always | explicit-config-only |
| p5dep-capture-job-ends-in-one-closed-vocabulary-count | S4 | safety | always | explicit-config-only |
| p5dep-baseline-digest-change-closes-the-activation-gate | S5 | safety | always | default-production |
| p5dep-harness-sweep-retires-owners-before-evidence | S6 | safety | always | explicit-config-only |
| p5dep-cited-evidence-survives-the-harness-sweep | S6 | safety | always | explicit-config-only |
| p5dep-review-reads-parse-strict-bodies-after-the-envelope | S7 | safety | always | default-production |
| p5dep-observational-routes-read-and-take-no-background-part | S7 | safety | always | default-production |
| p5dep-memory-tool-actions-are-closed-and-allowlisted | S8 | safety | always | default-production |

## Proposed index (`proposed/catalog.md`)

| Slug | Surface | Type | Semantics | Intended reachability |
| --- | --- | --- | --- | --- |
| cc-scorer-output-deterministic-under-contract-version | S-A | safety | always | explicit-config-only |
| cc-oversize-text-refused-whole-never-truncated | S-A | safety | always | explicit-config-only |
| cc-case-fold-applied-identically-to-both-sides | S-A | safety | always | explicit-config-only |
| cc-pass-starts-only-after-publication-job-terminal | S-B | safety | always | explicit-config-only |
| cc-passes-in-flight-bounded-and-overflow-counted | S-B | safety | always | explicit-config-only |
| cc-candidate-set-bounded-with-counted-truncation | S-C | safety | always | explicit-config-only |
| cc-partial-result-is-descriptor-granular-prefix | S-C | safety | always | explicit-config-only |
| cc-row-key-unique-and-caps-enforced | S-D | safety | always | explicit-config-only |
| cc-table-rebuildable-from-evidence-bytes-and-live-decisions | S-D | safety | always | explicit-config-only |
| cc-sweep-drops-only-dead-targets-bounded-idempotent-kernel-untouched | S-E | safety | always | explicit-config-only |
| cc-read-serves-only-live-decision-rows-ids-and-scores-only | S-F | safety | always | explicit-config-only |
| cc-read-distinguishes-disabled-unavailable-and-empty | S-F | safety | always | default-production |
| cc-flag-off-no-pass-no-write-no-table-read | S-G | safety | always(!X) | default-production |
| cc-candidate-carries-dispute-ids-evidence-liveness-checked-at-confirm | S-H | safety | always | explicit-config-only |
| cc-pass-and-sweep-write-no-kernel-event-and-no-signal | R5 | safety | always(!X) | explicit-config-only |

## Files

- `dependencies/catalog.md`, `system-model.md`, `existing-checks.md`,
  `fault-map.md`, `portfolio-evaluation.md`, `evidence/<slug>.md` (20).
- `proposed/catalog.md`, `certificate.md`, `open-questions.md`.
