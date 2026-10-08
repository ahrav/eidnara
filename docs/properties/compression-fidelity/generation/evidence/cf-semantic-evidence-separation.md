# cf-semantic-evidence-separation

## Discovery trigger

Date: 2026-09-19. Inspected HEAD:
`99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`.
The [settled contract][contract], "Requirements" R7 and "Evidence and review
gate" (historical plan lines 56,263-271), separates deterministic evidence
from human semantic judgments. External scope is the supplied local plan and
code/docs/history. No additional incident logs or related repositories are
supplied. No model calls, tests, or CI checks run here.

## Evidence trail

- [citations.rs:141-172][citations], `check_fact_set`, checks citation counts,
  alias resolution, nonempty UTF-8 spans, and the accepted ordinal interval.
  It never evaluates whether `fact.content` follows from the cited text.
- [validate.rs:689-709][validator] separates accepted history from extraction
  outcomes. A fact-set rejection can accompany valid history.
- [citations_golden.rs:188-281][golden] tests those outcomes with authored
  strings. Its helper at lines 83-85 only calls validation; the test name's
  "while history publishes" is not proof of a storage write or model quality.
- [history_summarizer.rs:4154-4204][driver] does run scripted publication and
  inspect stored P1. It uses a placeholder prompt and scripted output.
- [Evidence and review gate][contract] (historical plan lines 237-242,270-271)
  requires real-output provenance and human review,
  rejects missing/disputed judgments, and keeps configured attempts visible.
- [CI:557-566][ci] defines workspace tests and doctests. No historical
  `run-history_summarizer-eval.ts` invocation is found at HEAD, and its tracked
  script/corpus paths are absent. This is not a claim that CI ran.
- [Typed mutation script:12-16,40-70][mutation] tests backend-error reporting,
  not semantic recall or authority. Its existence does not restore the old
  evaluator. No compression-fidelity evaluation command exists at HEAD.

Reachability is `test-only`: the record governs the planned offline result
acceptance boundary, not a new production validator. Existing scripted seams
are inspectable, but the semantic gate and baseline are absent. Confidence is
high about validator scope, not about the truth of a future human judgment.

## Failure scenario

A well-formed output reverses C1's rejection or C2's unconfirmed deployment
while citing a valid source span. A report labels the output faithful because
the parser and citation checker accepted it. Alternatively, an authored XML
sample is labeled a real-model result, or a review is attached to different
output bytes. These are synthetic evidence-integrity controls, not incidents.

A competing explanation is that the validator performs semantic comparison
internally. Reading `check_fact_set` and `validate_parsed_history_segments`
discriminates: neither implements that comparison. This establishes the limit
of their evidence, not an actual model reversal. No such reversal is reproduced
or observed during this discovery.

## Timing windows and dependencies

Record provenance when output is captured, before review or comparison can
misattribute it. A semantic label needs the named real producer capture and
human judgment over those same bytes and corpus. A model error, unfinished
capture, or unresolved human dispute remains explicit, not a structural pass.
Later capture work needs separate provider authorization and declared limits.

## What a test must construct

Construct structurally valid good and qualifier-reversing samples with valid
citations. Bind recorded human judgments to their exact input/output artifacts.
Exercise absent judgment, mismatched artifact identity, and scripted-as-real
labels offline. Scripted judgments qualify result accounting, not model quality.
Machine checks can assert complete, correctly linked approvals and reject an
unreviewed result. Humans decide whether meaning is preserved; no text-matching
or optional model judge acquires final authority. None found for this gate.

## Investigation log

### Q: Can schema or citation success establish semantic fidelity?
- Sources examined: Citation checker, validator, and fixture helpers above.
- Findings: They establish shape, source-span legality, or scripted byte
  handling. They do not establish the asserted meaning of real output.
- Missing evidence: Authorized real-output capture and human review.
- Conclusion: Resolved: these successes cannot substitute for semantic review.

### Q: Who reviews which real producer run?
- Sources examined: Plan:134,237-242,270-271.
- Findings: Provider/model selection is explicit; two people approve the first
  baseline, with non-author review and human adjudication for later changes.
- Missing evidence: Named provider/model, declared limits, and human reviewers.
- Conclusion: Needs human input; no external call is authorized here.

[contract]: https://github.com/ahrav/eidnara/issues/707
[citations]: ../../../../../crates/daemon/src/history_summarizer_citations.rs
[validator]: ../../../../../crates/daemon/src/history_summarizer_validate.rs
[golden]: ../../../../../crates/daemon/src/history_summarizer_citations_golden.rs
[driver]: ../../../../../crates/daemon/src/history_summarizer.rs
[ci]: ../../../../../.github/workflows/ci.yml
[mutation]: ../../../../../packages/e2e-tests/scripts/run-rust-history_summarizer-producer-mutation.ts
