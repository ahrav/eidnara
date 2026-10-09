# Evaluation portfolio evaluation

This file records the independent review agent's findings and final
dispositions after discovery. It is not a new review run, model evaluation, or
acceptance report. Source revision:
`99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`, inspected 2026-09-19.
The [catalog index](../README.md#scope-and-provenance) records supplied scope,
older-catalog limitations, and the explicit `_lenses` placement deviation.

## Portfolio disposition

The [catalog](catalog.md) contains eight active records, seven unexercised and
cf-corpus-byte-identity partial after U1, and eight
matching evidence files. Seven are safety records and one is reachability.
Semantics are six `always`, one `sometimes`, and one `always-or-unreached`.
All eight surfaces are `test-only`: the proposed evaluator is absent and the
forwarding path exists only behind the e2e mock's explicit `forward`
construction argument; neither is a configured production capability. This
classification does
not relabel the production surfaces whose observations they consume. All
[existing checks](existing-checks.md) remain unaudited.

Final records incorporate the review's missing control and cost surfaces and
its ownership refinements. Harness readiness, safe evaluation, human-reviewed
evidence, and fidelity acceptance remain separate claims.

## Independent findings and dispositions

### Harness fit

- **Refinement applied: one initial qualification owner.**
  [cf-fixture-script-qualification](evidence/cf-fixture-script-qualification.md)
  owns bounded case-ID selection, real alias binding, script consumption,
  nonempty accepted publication, and correlated capture in the ungated lane.
  Unknown IDs, mismatches, and exhaustion fail explicitly rather than returning
  sentinel success. Delivery consumes this receipt and independently checks
  every invocation. Recovery also needs its own visible tool evidence.
- **Suggestion rejected: qualify the fixture before U1/U2 as a reordered
  milestone.** Qualification stays inside U3 after U1/U2. A cost-ranked probe
  does not replace corpus and replay prerequisites. Broader runtime work needed
  for the ungated fold blocks U3; gated or skipped coverage cannot close it.
- **Refinement applied: one identity and capture authority.**
  [cf-corpus-byte-identity](evidence/cf-corpus-byte-identity.md) owns exact-file
  SHA-256 across compiled Rust, runtime TypeScript, and the manifest. Generation
  consumes it. [cf-authoritative-evidence-assembly](evidence/cf-authoritative-evidence-assembly.md)
  resolves each result to retained owner captures and verified byte hashes.
  Missing, reset, cross-bound, or contradictory artifacts cannot be repaired by
  a derived table. A parsed object does not prove original transport-byte identity.

### Coverage balance

- **Gap incorporated: semantic-control discrimination.**
  [cf-semantic-control-discrimination](evidence/cf-semantic-control-discrimination.md)
  records the plan's U4 known-bad and positive controls, not a new semantic
  policy. Reversed negation, false completion, false observation, wrong source,
  and lost hint qualifiers must be distinguished from valid paraphrases and
  confirmed deployment. Missing, duplicate, altered-label, constant, or
  mismatched verdicts cannot qualify review.
- **Suggestion rejected: mandatory treatment-arm blinding.** The settled plan
  requires independent source labels, human authority, and control verdicts.
  Blinding is an optional review practice, not an added milestone gate. Control
  discrimination qualifies these controls, not universal human or model accuracy.
- **Gap incorporated: complete cost evidence.**
  [cf-evaluation-cost-completeness](evidence/cf-evaluation-cost-completeness.md)
  owns estimator/model-tagged sizes, generation input/output and attempts,
  transform time, recovery calls/output, cold/warm separation, and raw-source
  leakage. Retain attempted sends separately from acknowledged responses.
  Missing usage or ambiguous cost is not zero; another send requires a
  conservative bound within the frozen call/output/time/spend limits or stops
  as unreviewable. Delivery separately owns serving-work restrictions.

### Implementability

- **Refinement applied: complete real execution, not a model label.**
  [cf-bounded-record-and-forward](evidence/cf-bounded-record-and-forward.md)
  requires selecting the compatible real model before capture, forwarding that
  same request body, returning its response to OpenCode, and capturing subsequent
  registered-tool-result requests through completion. Script and forward modes
  are explicit and exclusive. A real-looking model field, silent scripted
  fallback, or unfinished loop cannot receive real-model evidence credit.
- **Refinement applied: private artifacts with existing bounded diagnostics.**
  The final [assembly evidence](evidence/cf-authoritative-evidence-assembly.md)
  and forwarding evidence require restrictive capture/temp/final permissions,
  explicit JSON publication with `mode: 0o600`, and reuse of `DiagnosticSink`
  where applicable. The sink bounds private diagnostics; it neither redacts
  credentials nor turns truncated data into complete capture. Credentials enter
  only outbound, never capture/config/log artifacts. Temp-plus-rename is not a
  crash-durability claim. Raw semantic data stays outside repository and incident
  publication paths. Keep the live command separate; do not disable or weaken
  the incident runner's loopback, secret, or environment guards.
- **Refinement applied: optional live safety is narrowly optional.** Only the
  enabled live branch uses `always-or-unreached`. The
  [fault map](fault-map.md#named-safety-checks-and-coverage-checks-to-add)
  separately requires `cf-eval-offline-zero-outbound` with `always`, plus default
  branch coverage. Script exhaustion and setup failure cannot send externally.
  Full acceptance still requires
  [cf-reviewed-semantic-batch-reached](evidence/cf-reviewed-semantic-batch-reached.md);
  an offline green run cannot satisfy it.

### Wildcard and bias

- **Bias surfaced: harness self-properties are not semantic success.**
  [cf-complete-independent-comparison](evidence/cf-complete-independent-comparison.md)
  keeps every declared attempt, retry, fallback, and terminal outcome. It
  separates desired outcomes from observed baseline failures, preserves prompt
  differences as treatment, and requires identity-bound human approval. A
  reviewed failing real batch can satisfy reachability while failing fidelity.
- **Bias surfaced: finite and correlated evidence.** Six risk-selected synthetic
  examples supply no reliability estimate. Repeated lens findings do not create
  independent corroboration. Human review and control judgments cannot prove
  universal entailment, and privileged native reads cannot establish agent
  recovery. Savings never offset a required fidelity failure.

## Open prerequisites and test-strategy handoff

Route all eight records with their evidence and [fault map](fault-map.md) to
`/testing:test-strategy`:

| Records | Reusable seam and required evidence |
| --- | --- |
| Corpus identity | U1/U3/U4 byte hashing and compiled/runtime digest exchange using existing test/control boundaries. |
| Fixture qualification | U3 direct-host fixture, separate bounded case-ID control, real producer validation/publication, and ungated provider capture after U1/U2. |
| Assembly, comparison, controls, and costs | U4 offline intact/tampered/missing artifact checks, one manifest, sealed human labels, full attempt/cost accounting, and private publication. |
| Forwarding and reviewed batch | Existing Messages provider and registered tool loop with explicit authorized mode, frozen whole-loop limits, full capture, and required human review. |

Open prerequisites include the reviewed corpus, complete producer capture
(system, user prompt, and model), provider capture, digest exchange, actual C6
and ungated fold execution,
selected provider/model/version/settings, usage/pricing authority, bounded
ambiguous-attempt accounting, and semantic humans. These withhold acceptance;
they are not permission for another adapter, framework, production export, or
scope expansion. Report all four named safety assertions and every declared
situation separately, including the 29 occurrence markers and missing outcomes.
Existing-test adequacy belongs to `/testing:invariant-test-review`; runtime
guard strength belongs to
`/low-level-systems:defensive-assertions-and-invariant-guards`. No live call or
test execution is part of this disposition.
