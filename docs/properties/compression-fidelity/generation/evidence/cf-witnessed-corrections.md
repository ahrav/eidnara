# cf-witnessed-corrections

## Discovery trigger

Date: 2026-09-19. Inspected HEAD:
`99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`.
An orchestrator-commissioned independent portfolio review identifies missing
coverage of U5's correction-acceptance rule.
[Milestone boundaries and dependencies][contract] U5 (historical plan lines
247-255) requires a named failing witness, fresh semantic evidence where meaning
changes, and
disclosed importance/serving effects. Supplied evidence is the plan and local
code/docs/history; no other incident logs or related repositories are supplied.
No fix, test, model call, or semantic review is performed here.

## Evidence trail

- [Milestone boundaries and dependencies][contract] (historical plan lines
  185-205,214-242) orders U1 corpus, U2 replay, U3 qualified
  invocation, then U4 real-output capture and review. Qualification is part of
  U3 after U1/U2, not a replacement for those dependencies.
- [Milestone boundaries and dependencies][contract] U5 (historical plan lines
  247-255) requires U2/U3 for deterministic text corrections and
  U4 for generated-meaning changes. It rejects golden-only certification of a
  prompt change and requires before/after importance and serving observations.
- [Evidence and review gate][contract] (historical plan lines 266-271) blocks
  required-obligation regressions and unresolved
  required failures; baseline failures remain failures even if the harness works.
- [System prompt:120-158][system] contains the importance rubric, including
  the unsupported search premise at line 130. Revising that premise touches
  instructions that determine model-produced importance, not only formatting.
- [history_summarizer.rs:48-83][driver], `to_stored_history_segment`, copies
  generated importance with default/clamp handling. The test at
  [2541-2573][driver] checks that numeric conversion, not rubric stability.
- [decay_render.rs:491-531][decay], `compute_tiers`, consumes importance and
  budget pressure. [297-337][decay], `render_decayed_history_segments`, can
  demote further under the hard budget. A raw importance score does not fully
  describe effective serving, so both observations matter after rubric edits.
- [prompt.rs:481-582][prompt] checks assembled prompt goldens. It neither
  evaluates real output meaning nor links a correction to a prior failure.

Reachability is `test-only`: this record governs offline acceptance of a U5
correction, not a production policy or serving-time evaluator. Its rule is a
documented claim under test. High confidence describes the explicit rule and
inspected importance-to-serving dependency, not a successful correction.
U1 (#718) adds the fidelity corpus; the replay module and evaluation command
remain absent.

## Failure scenario

A prompt cleanup is accepted because its golden is updated, without preserving
the failing case or obtaining fresh reviewed outputs. Another change fixes C1
but loses C3's binding constraint. A rubric edit can also alter importance and
therefore which effective tier later appears, while a report silently assumes
unchanged serving. These are synthetic correction-process failures, not incidents.

A competing explanation is an allowed wording-only change or an unrelated
serving-budget difference. A prior named witness, the same case/scenario
identity, and linked before/after importance plus effective-serving observations
discriminate them. Humans judge semantic improvement and required-obligation
regressions. Byte goldens and a shared corpus digest cannot make that judgment.

## Timing windows and dependencies

Retain the named failure before the local fix. Apply only the witnessed
correction at its owner after U2/U3 or U4 prerequisites, as applicable. For
prompt/generated-meaning changes, capture fresh real outputs on the same cases
and obtain the plan-required human review; do not reuse the old approval.
Consume the evaluation owner's [complete comparison][comparison-owner] instead
of creating a second comparator. Missing required evidence withholds acceptance.

## What a test must construct

Create a prior named failure and matched post-fix artifact references. Include
controls with no pre-fix witness, golden-only prompt evidence, reused old review,
a required-obligation regression, and missing rubric/serving observations.
Deterministic checks validate these evidence links and prerequisite records;
humans decide meaning. Rubric changes must disclose shifts rather than require
importance to remain identical. No new blinding, production export, semantic
callback, packing-policy change, or evaluator framework follows from this rule.
None found for the combined U5 correction-acceptance check at HEAD.

## Investigation log

### Q: Can a prompt golden or unchanged importance certify a correction?
- Sources examined: Prompt golden, importance conversion, and renderer above.
- Findings: Golden equality checks bytes; importance is only one serving input.
  Neither proves semantic improvement or absence of required-obligation loss.
- Missing evidence: Named pre-fix failure, fresh same-case review, and actual
  effective-serving observations for a rubric change.
- Conclusion: Resolved: those local checks alone cannot certify the correction.

### Q: Which witnessed correction and reviewers proceed first?
- Sources examined: Plan U1-U5 and acceptance:266-271.
- Findings: The plan supplies dependency order and human review authority;
  existing code cannot select the approved local fix or reviewers.
- Missing evidence: Chosen witness/fix and reviewers for meaning and observed
  importance/serving effects after prerequisites are met.
- Conclusion: Needs human input.

[contract]: https://github.com/ahrav/eidnara/issues/707
[system]: ../../../../../crates/daemon/testdata/history_summarizer-system-prompt.txt
[driver]: ../../../../../crates/daemon/src/history_summarizer.rs
[decay]: ../../../../../crates/daemon/src/decay_render.rs
[prompt]: ../../../../../crates/daemon/src/history_summarizer_prompt.rs
[comparison-owner]: ../../evaluation/catalog.md#cf-complete-independent-comparison
