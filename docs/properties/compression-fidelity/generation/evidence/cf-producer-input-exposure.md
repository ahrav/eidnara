# cf-producer-input-exposure

## Discovery trigger

Date: 2026-09-19. Inspected HEAD:
`99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`.
The `cf-producer-input-exposure` row in
[Provenance and reusable catalog][contract] (historical plan lines 194-196)
requires decisive source text to reach real prompt assembly or be recorded as
unexercised input. The supplied external scope is
the plan and local code/docs/history; no additional incidents or related repos
are supplied. No tests or external model calls run here.

## Evidence trail

- [chunk.rs:233-274][chunk], `Builder::push_message`, omits system text,
  handles tool-only messages separately, and constructs transformed parts.
- [chunk.rs:1187-1202][chunk], `text_parts`, cleans user text, trims other text,
  normalizes whitespace, and replaces media with a placeholder.
- [chunk_text.rs:147-173][text] removes system-reminder text from user input
  and collapses Unicode whitespace. Native bytes are not presented bytes.
- [chunk.rs:1242-1259][chunk], `extract_tool_result_summaries`, uses the linked
  call summary or `TC: {tool_name}`. It does not read the result payload.
- [chunk.rs:973-1007,1046-1053][chunk] uses the same configured budget for build
  and presentation before `build_history_segment_agent_prompt` is called.
- [chunk.rs:368-395,1309-1340][chunk] admits an oversized first block, then
  withdraws aliases whose whole presented parts do not survive truncation.
- [citations_golden.rs:457-482,509-522][golden] contains same-budget truncation
  and whitespace-transformation checks. Both are unaudited and unrun here.
- [lib.rs:22428-22463][lib], `TestProducer::start`, records each attempt's
  system text, user prompt, and model in `ProducerState::attempts` since U2
  (#719). The private test recorder supplies the complete producer request; no
  production export obtains these observations.
- The inspected `a03f58d2` patch changes the verbatim comparison from trimmed
  equality to exact native equality, retained at [chunk.rs:271-274][chunk].
  This is code-history evidence of offset risk, not a semantic incident.

Reachability is `explicit-config-only`: [config.rs:139-145][config] defaults
to no models, and [lib.rs:5685-5692,5732-5757][lib] requires a configured chain
and firing boundary before assembling production input. Mechanisms are
inspected with high confidence; their fidelity consequences remain untested.

## Failure scenario

C2's only success receipt appears in a tool-result payload. Generation sees a
call summary but not the receipt. Scoring its uncertainty as a model mistake
would confuse input loss with generation loss. A C3 prohibition near the end of
an oversized first block can likewise disappear before generation.

A competing explanation is that an alias or accepted ordinal proves exposure.
It does not: the alias may name transformed tool-call text, and ordinal metadata
can cover omitted text. Conversely, withdrawal of a whole-part alias does not
prove that every material fragment in that part vanished. Inspect the actual
request and the specific obligation, not an alias-presence proxy.

## Timing windows and dependencies

Observation belongs after `presented_input`, not merely after chunk build.
Inspect all request sections to distinguish native `new_messages` evidence
from calibration examples, prior summaries, or project memory. A reference
repeating the expected answer is not the native receipt reaching the producer.
Use the actual selected source identities and recorded budget.

## What a test must construct

Construct a receipt-bearing tool result, a one-off scoped constraint, Unicode
and whitespace changes, and a same-budget oversized first block. Include
surviving controls. Link each material native occurrence/revision/span to a
presented fragment or an explicit omitted/filtered/truncated outcome.
Humans decide whether transformed text exposes the required meaning; machine
checks validate fragments and linkage. An input gap earns no generation credit
and cannot turn an overall preservation failure into success.
None found for this case-linked exposure harness at HEAD.

## Investigation log

### Q: Does the producer receive native tool-output evidence?
- Sources examined: `extract_tool_result_summaries`, chunk.rs:1242-1259.
- Findings: It takes call summaries by arc identity or emits the tool name;
  result payload bytes are not part of this function's output.
- Missing evidence: Which reviewed cases depend solely on omitted payloads.
- Conclusion: Resolved for the mechanism; case exposure needs a replay.

### Q: Which transformed fragments preserve each obligation?
- Sources examined: The input transformations and plan:194-196.
- Findings: Bytes and identity can be checked locally; semantic sufficiency
  cannot be derived from normalization or citation validity alone. The existing
  private helper records only the user prompt, not the system/model arguments.
- Missing evidence: Human annotations and the required private recorder
  extension capturing system, user prompt, and selected model.
- Conclusion: Needs human input for meaning; unresolved for replay capture.

[contract]: https://github.com/ahrav/eidnara/issues/707
[chunk]: ../../../../../crates/daemon/src/history_summarizer_chunk.rs
[text]: ../../../../../crates/daemon/src/chunk_text.rs
[golden]: ../../../../../crates/daemon/src/history_summarizer_citations_golden.rs
[config]: ../../../../../crates/daemon/src/config.rs
[lib]: ../../../../../crates/daemon/src/lib.rs
