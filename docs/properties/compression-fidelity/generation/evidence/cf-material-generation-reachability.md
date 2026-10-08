# cf-material-generation-reachability

## Discovery trigger

Date: 2026-09-19. Inspected HEAD:
`99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`.
The wildcard pass asks whether a green generation replay necessarily sends
material native evidence through the real assembler and publishes its output.
[Source and evidence flow][contract] (historical plan lines 175-179,194-196)
requires evidence for those reached stages.
External scope is the supplied plan and local code/docs/history; no additional
incident logs or related repositories are supplied. No tests run here.

## Evidence trail

- [history_summarizer.rs:4154-4204][driver] executes the scripted producer and
  inspects stored P1 and the publication floor, but supplies `placeholder
  prompt`. It is not a witness of material native input reaching generation.
- [lib.rs:18252-18281][lib] nests private test modules using `#[path]`.
  This is the ancestry KTD2 requires for the new replay module.
- [lib.rs:22327-22367][lib], `ProducerState`, has prompt, output, and attempt
  observations and hooks. [lib.rs:22431-22510][lib], `TestProducer::start`,
  records complete attempts and prepares scripted output without a real model call.
- [lib.rs:22651-22677][lib], `handler_with_store`, supplies the handler,
  store, and test route through the existing producer factory.
- [lib.rs:5660-5692,5732-5761][lib] distinguishes no-fire/no-model outcomes
  from real assembly. [chunk.rs:973-1053][chunk] builds the actual request.
- [config.rs:139-145][config] defaults to an empty model chain. A test that
  forgets explicit configuration can return without generation.
- [citations_golden.rs:417-453][golden] has a local situation check requiring
  some budget to retain a strict alias subset. It does not establish this
  record's material input-to-publication situation.

Reachability is `test-only`: the constant campaign marker belongs in the
planned offline replay, not the production transform. Its component seams
exist, but the combined fidelity witness and marker are absent. Confidence is
medium about constructibility from inspected helpers; no witness runs here.

## Failure scenario

A suite calls the validator with authored XML, compares effective strings, and
reports generation coverage without invoking real prompt assembly. Or the
handler returns `no_models`, so no output is published. A conditional safety
check can remain green because its material situation never occurred.
These are synthetic harness failure modes, not production incidents.

A competing explanation is that a failure to fire the marker reveals a real
path regression rather than bad fixture setup. Check explicit model selection,
eligible range, block identities, budget, consumed output, and store observation
before deciding. A finite campaign cannot establish universal liveness.

## Timing windows and dependencies

The source obligation predates output. Observe its decisive fragment after
real prompt assembly, specifically in `new_messages`, not only in calibration
or reference summaries. After the driver consumes the corresponding output,
observe nonempty published history for that source-bound attempt. Do not infer
publication from entry into validation or a backend completion counter.

## What a test must construct

Construct one independently annotated material native source and an eligible
firing with explicit model configuration, valid identities, and enough budget
to expose the decisive fragment. Return source-bound valid XML through the
existing driver and observe matching stored history.
The `cf-material-generation-reachability` marker uses `sometimes` on this
conjunction. It fires with correct output, requires no semantic violation, and
needs no real provider. None found for this marker or complete material witness.
Per-case completeness remains separate: one witness does not cover C1-C6,
every fallback/healing condition, real-model fidelity, or served invocation.
The [fault-map markers](../fault-map.md#coverage-checks-to-add) cover five
specific situations; the [evaluation comparison owner][comparison-owner]
reconciles required case/scenario rows. Placeholder-prompt and `no_models`
controls must not fire this marker. Its condition remains one joined witness.

## Investigation log

### Q: Does the scripted publication test already witness real assembly?
- Sources examined: `wired_history_summarizer_happy_path_sends_validates_and_publishes`.
- Findings: It reads actual store rows but passes a placeholder prompt.
- Missing evidence: A material native source linked through the assembler.
- Conclusion: Resolved: it supplies a publication seam, not this witness.

### Q: Can private orchestration construct the witness without new APIs?
- Sources examined: KTD2 and the `lib.rs` helpers cited above.
- Findings: The child test module can access existing private state and
  handler helpers. Prompt capture and scripted results are available.
- Missing evidence: A completed joined replay and required case/scenario rows.
- Conclusion: Unresolved, needs implementation and execution; no API expansion
  is justified by this discovery.

[contract]: https://github.com/ahrav/eidnara/issues/707
[driver]: ../../../../../crates/daemon/src/history_summarizer.rs
[lib]: ../../../../../crates/daemon/src/lib.rs
[chunk]: ../../../../../crates/daemon/src/history_summarizer_chunk.rs
[config]: ../../../../../crates/daemon/src/config.rs
[golden]: ../../../../../crates/daemon/src/history_summarizer_citations_golden.rs
[comparison-owner]: ../../evaluation/catalog.md#cf-complete-independent-comparison
