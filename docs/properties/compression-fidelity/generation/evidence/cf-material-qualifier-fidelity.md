# cf-material-qualifier-fidelity

## Discovery trigger

Date: 2026-09-19. Inspected HEAD:
`99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`.
[Requirements][contract] R1/R2 (historical plan lines 47-48) distinguish subject,
scope, polarity, temporal state, and evidence status. The tier obligations are
claims under test, not
implemented guarantees. External scope is the supplied plan and local
code/docs/history. No additional incident logs or related repositories are
supplied, and no tests or real-model calls run here.

## Evidence trail

- [System prompt:3-17][system] asks for decisions, constraints, outcomes, and
  follow-ups without assuming original-transcript recovery.
- [System prompt:173-178,197-218][system] requires self-contained tiers and
  permits title-only P4. It also suggests search recovery at line 212.
- [Materiality and minimum meaning by tier][contract] (historical plan lines
  70-86,104-106) makes required meaning tier-specific, includes
  the heading in acceptance, and forbids unsupported recovery credit.
- [Six acceptance cases][contract] (historical plan lines 97-101) supplies C1-C5:
  rejected pooling with material rationale, unconfirmed deployment,
  a task-scoped constraint, repeated misinformation,
  and a superseded plan with partial/cancelled outcome and inferred cause.
- [validate.rs:320-341][validator], `parse_history_segment_output`, fills
  missing P2/P3 from denser tiers and defaults P4 to empty text.
- [validate.rs:1122-1228][validator], `validate_parsed_history_segments`, checks
  nonempty P1 and legal coverage. It does not compare tier prose with source
  decisions, chronology, negation, rationale, or evidence status.
- [prompt.rs:248-312][prompt] re-renders earlier summaries as session
  references. Repetition is therefore possible input, not independent evidence.

Reachability is `explicit-config-only`: generation requires a configured model
chain and eligible firing at [lib.rs:5685-5757][lib]; [config.rs:139-145][config]
defaults to an empty chain. Confidence is medium for the contract-to-code gap,
not a claim about the quality of unseen outputs. Structural tests are unaudited.

## Failure scenario

A short tier says "Pooling redesign implemented" when the native decision is
that pooling-first was rejected and the redesign is only planned. A title can
introduce the same false completion even when the body retains uncertainty.
Another synthetic mutation promotes a task-scoped limit into a universal rule,
or turns an inferred cause into an observed cause. These are not incidents.

A competing explanation is harmless paraphrase or allowed loss of detail.
The human's predeclared follow-up and material obligations discriminate: losing
incidental dialogue can be allowed, but losing rationale needed to choose the
next action is not. Literal matching of the illustrative C1 answer cannot make
that distinction. Uncertainty must not become indiscriminate abstention when
the matched C2 source actually contains a successful deployment receipt.

## Timing windows and dependencies

Compare chronology across native source occurrences, not just the last repeated
summary. Review each title with its effective tier body, using the provenance
record to identify inherited rather than authored text. A missing decisive
input is recorded separately before assigning a generation-fidelity judgment.
For a case-permitted P4 omission, consume the serving owner's observed
complete-invocation disposition. P4 need not repeat meaning that this permitted
route supplies, but cannot borrow hypothetical recovery. Visible/discoverable
credit requires actual evidence; unavailable remains a preservation loss even
if the case permits safe abstention. Losing the sole relevant constraint is
not excused by that abstention. P5 remains an adjacent serving responsibility.

## What a test must construct

For C1-C5, construct the stated forbidden transitions and valid controls,
including paraphrases, explicit success versus missing receipt, and primary
evidence contradicted by repeated summaries. Keep material rationale where its
loss changes the action. Preserve allowed losses independently of candidates.
Include a permitted P4 omission with demonstrated alternate visibility or
recovery and a matched unavailable control that receives no preservation credit.
Humans judge these predicates. Deterministic checks can require a complete,
artifact-bound judgment per required obligation/tier, but cannot establish
entailment. None found for a human-reviewed generation-fidelity harness at HEAD.

## Investigation log

### Q: Does structural validation already enforce these distinctions?
- Sources examined: `parse_history_segment_output` and
  `validate_parsed_history_segments` at the ranges above.
- Findings: They manipulate tier strings and validate presence/ranges, without
  a source-meaning comparison. Correct structure does not settle fidelity.
- Missing evidence: Reviewed real producer outputs for the material cases.
- Conclusion: Resolved for validator scope; semantic behavior remains untested.

### Q: Which losses and P4 titles are safe for these cases?
- Sources examined: Plan:66-86,97-106 and system prompt:197-218.
- Findings: The plan assigns materiality to a human before candidate review.
  Code cannot establish the permitted next action or approve a safe title.
- Missing evidence: Approved tier obligations, allowed losses, and judgments.
- Conclusion: Needs human input.

[contract]: https://github.com/ahrav/eidnara/issues/707
[system]: ../../../../../crates/daemon/testdata/history_summarizer-system-prompt.txt
[validator]: ../../../../../crates/daemon/src/history_summarizer_validate.rs
[prompt]: ../../../../../crates/daemon/src/history_summarizer_prompt.rs
[config]: ../../../../../crates/daemon/src/config.rs
[lib]: ../../../../../crates/daemon/src/lib.rs
