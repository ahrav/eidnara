# Property catalog: write-time contradiction candidates (P5)

## Provenance

Plan: P5
`docs/plans/2026-10-24-adaptive-discovery/05-write-time-contradiction-candidates-plan.md`.
Plan baseline: `a0e3f2993`. Authored at HEAD `3a3b916b7`, 2026-10-24, and
published on branch `spec/contradiction-candidates-property-catalog`
(commit `6350c4647`; `739697ad5` adds the tracker link below).
#1164 lands the part: every `file:line` reference is re-verified at the
#1164 branch tip (base `979fb814e`, scorer commits `05acd0107` and
`446c7363f`, the review round; references into the scorer module and
its test file are verified at `446c7363f`). References were remapped through the git diff from `3a3b916b7`; references to lines that
changed since then were rewritten by hand. Those changes come from #1161
(the `eidnara_memory` `dispute` action, `EIDNARA_MEMORY_DEFAULT_ACTIONS`,
per-allowlist schemas, and the pi `allowedActions`, which resolves lead D7)
and #1157 (`review.use_outcome`). The design-time companions
(`dependencies/system-model.md`, `dependencies/portfolio-evaluation.md`,
`proposed/certificate.md`, `proposed/open-questions.md`) describe
`3a3b916b7`; their references were remapped and the S7 and S8 passages
updated where the code changed. Records are claims under test, not proof.
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

`proposed/` is the invariant-driven domain-modeling output for the P5
behavior: 15 obligation records, the method's certificate, and the open
questions and plan-versus-HEAD conflicts the specification must resolve.
Slug prefix `cc-`. #1164 builds the scorer and converts the three S-A
records to `Status: active` (oversize `Exercised: partial`, the other two
`yes`), with evidence files, `existing-checks.md`, `fault-map.md`, and
`portfolio-evaluation.md` (the fresh-context evaluation, with refinements
applied, two gaps queued, and two biases for a human). The other 12 records
stay `Status: active - PROPOSED` with no implementation evidence; the store
(#1165), sweep (#1166), pass (#1167), and read (#1168) tickets make them
checkable.

`dependencies/` records re-exercised by #1164:
`p5dep-analyzer-refuses-oversize-input-before-retention`,
`p5dep-analyzer-keeps-original-bytes-and-is-pure`, and
`p5dep-analysis-epoch-and-identity-pin-the-contract` (the matcher preimage
names `ANALYSIS_CONTRACT_EPOCH`; `identity_pins_the_manifest` is unchanged).

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

| Slug | Surface | Type | Semantics | Reachability | Status |
| --- | --- | --- | --- | --- | --- |
| cc-scorer-output-deterministic-under-contract-version | S-A | safety | always | test-only (intended explicit-config-only) | active |
| cc-oversize-text-refused-whole-never-truncated | S-A | safety | always | test-only (intended explicit-config-only) | active |
| cc-case-fold-applied-identically-to-both-sides | S-A | safety | always | test-only (intended explicit-config-only) | active |
| cc-pass-starts-only-after-publication-job-terminal | S-B | safety | always | intended explicit-config-only | proposed |
| cc-passes-in-flight-bounded-and-overflow-counted | S-B | safety | always | intended explicit-config-only | proposed |
| cc-candidate-set-bounded-with-counted-truncation | S-C | safety | always | intended explicit-config-only | proposed |
| cc-partial-result-is-descriptor-granular-prefix | S-C | safety | always | intended explicit-config-only | proposed |
| cc-row-key-unique-and-caps-enforced | S-D | safety | always | intended explicit-config-only | proposed |
| cc-table-rebuildable-from-evidence-bytes-and-live-decisions | S-D | safety | always | intended explicit-config-only | proposed |
| cc-sweep-drops-only-dead-targets-bounded-idempotent-kernel-untouched | S-E | safety | always | intended explicit-config-only | proposed |
| cc-read-serves-only-live-decision-rows-ids-and-scores-only | S-F | safety | always | intended explicit-config-only | proposed |
| cc-read-distinguishes-disabled-unavailable-and-empty | S-F | safety | always | intended default-production | proposed |
| cc-flag-off-no-pass-no-write-no-table-read | S-G | safety | always(!X) | intended default-production | proposed |
| cc-candidate-carries-dispute-ids-evidence-liveness-checked-at-confirm | S-H | safety | always | intended explicit-config-only | proposed |
| cc-pass-and-sweep-write-no-kernel-event-and-no-signal | R5 | safety | always(!X) | intended explicit-config-only | proposed |

## Files

- `dependencies/catalog.md`, `system-model.md`, `existing-checks.md`,
  `fault-map.md`, `portfolio-evaluation.md`, `evidence/<slug>.md` (20).
- `proposed/catalog.md`, `existing-checks.md`, `fault-map.md`,
  `portfolio-evaluation.md` (placeholder), `evidence/<slug>.md` (3, for the
  active records), `certificate.md`, `open-questions.md`.
