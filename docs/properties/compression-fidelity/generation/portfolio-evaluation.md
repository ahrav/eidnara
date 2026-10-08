# Generation portfolio evaluation

This file records the independent review agent's findings and their final
disposition after the four discovery lanes. It is not a new research pass or
an execution result. Source revision:
`99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`, inspected 2026-09-19.
See the [catalog index](../README.md#scope-and-provenance) for supplied scope,
older-catalog limitations, and the explicit `_lenses` placement deviation.

## Portfolio disposition

The [catalog](catalog.md) contains eight active records, seven unexercised and
cf-source-obligation-independence partial after U1, and eight
matching evidence files. Seven are safety records with `always`; one is a
reachability record with `sometimes`. Three surfaces are
`explicit-config-only`, and five are `test-only`. All
[existing checks](existing-checks.md) remain unaudited.

The review refinements are represented in the final records and
[fault map](fault-map.md). Harness and human-review prerequisites remain open.
This is catalog readiness for test-strategy handoff, not fidelity acceptance.

## Independent findings and dispositions

### Harness fit

- **Refinement applied: complete producer exposure.**
  [cf-producer-input-exposure](evidence/cf-producer-input-exposure.md) requires
  the actual system text, user prompt, and selected model. The private recorder
  records only the user prompt. Extend that test-owned recorder under the
  existing daemon test ancestry; do not export production internals. Native
  evidence filtered or truncated before generation receives no generation
  credit and must not become a model failure against unseen input.
- **Refinement applied: one byte-identity owner.**
  [cf-source-obligation-independence](catalog.md#cf-source-obligation-independence)
  consumes evaluation's
  [cf-corpus-byte-identity](../evaluation/evidence/cf-corpus-byte-identity.md).
  Generation owns source/span validation and human obligations, not a second
  compiled/runtime/manifest digest check.

### Coverage balance

- **Gap incorporated: five independent situations.** The
  [generation fault map](fault-map.md#coverage-checks-to-add) adds material
  transformation, actual-budget truncation, rejected-primary/consumed-fallback,
  final discard with earlier coverage, and inherited P2/P3. Report each marker
  and required case separately. They augment
  [cf-material-generation-reachability](evidence/cf-material-generation-reachability.md),
  not five extra catalog records. Their predicates can fire on correct code;
  placeholder input and no-model paths cannot satisfy them.
- **Gap incorporated: U5 ownership.**
  [cf-guidance-capability-bound](evidence/cf-guidance-capability-bound.md) owns
  manual review of producer/hint recovery promises against the actual registry.
  [cf-witnessed-corrections](evidence/cf-witnessed-corrections.md) requires a
  named failing witness, same-case rerun, no required-obligation regression,
  fresh real-output review for meaning changes, and disclosed importance and
  serving changes for rubric edits. These records do not perform retrieval or
  create another evaluator.
- **Refinement applied: P4 omission is not automatic credit.** The final
  [qualifier-fidelity record](evidence/cf-material-qualifier-fidelity.md)
  consumes the complete-invocation disposition for a permitted omission.
  Unavailable evidence earns no useful-preservation or recovery credit, even
  when the scenario explicitly allows safe abstention.

### Implementability

- **Refinement applied: preserve U1-U5 dependencies.** Initial fixture
  qualification has one
  [evaluation owner](../evaluation/evidence/cf-fixture-script-qualification.md)
  inside U3, after U1/U2. A cheap qualification probe does not move that milestone
  first. U5 deterministic corrections need U2/U3; generated-meaning changes need
  U4. Broader prompt edits remain conditional on named failures, not cleanup.
- **Scope retained: no added blinding gate.** U4's known-bad and positive
  controls are required through
  [semantic-control discrimination](../evaluation/evidence/cf-semantic-control-discrimination.md).
  Treatment-arm blinding is optional. Neither control checks nor blinding
  replace human semantic authority.

### Wildcard and bias

- **Bias surfaced: structural success can hide semantic failure.**
  [Stage provenance](evidence/cf-generation-stage-provenance.md) separates
  authored, inherited, healed, discarded, extracted, and published artifacts.
  [Semantic evidence separation](evidence/cf-semantic-evidence-separation.md)
  prevents a valid citation, accepted XML, or scripted output from becoming
  real-model fidelity evidence.
- **Bias surfaced: finite synthetic coverage.** C1-C6 are risk-selected
  examples, not incident frequency data or a reliability sample. Existing
  calibration seeds are not held-out cases. Human-approved paraphrases and
  successful-deployment controls reject constant verdicts but do not certify
  universal reviewer accuracy.

## Open prerequisites and test-strategy handoff

Route all eight records with their evidence and fault requirements to
`/testing:test-strategy`:

| Records | Reusable seam and required evidence |
| --- | --- |
| Source obligations and input exposure | U1 human annotations and Rust native-span validation; real prompt/alias assembly with complete private capture. |
| Stage provenance and generation reachability | U2 private producer/validator/publication replay with attempt-bound observations and all six generation situation markers. |
| Qualifier fidelity and semantic evidence separation | U4 actual producer artifacts, independent obligations, and artifact-bound human judgments. |
| Guidance capability and witnessed corrections | U3 registry evidence and U5 named pre/post witnesses; U4 fresh semantic review where meaning changes. |

The corpus, initial two-human approvals, full recorder, digest exchange, and
joined replay are missing prerequisites. Later semantic approval also needs
authorized provider evidence and human reviewers. They are not unresolved
permission to add production APIs or change the importance/packing policy.
Keep existing-check adequacy with `/testing:invariant-test-review` and runtime
guard review with `/low-level-systems:defensive-assertions-and-invariant-guards`.
No test, model evaluation, or correction is claimed as executed.
