# ing-adapter-ingested-no-production-caller

## Discovery trigger
Parent specification Q-ING: "every manifest carries `adapter-ingested,
production caller: none`, the gap is recorded in `docs/properties/`, and
making an adapter production-live is outside this specification."

## Evidence trail
- `crates/eval-core/src/manifest.rs:137` the variant and its wire name.
- `docs/properties/evaluator/README.md` "Gaps recorded here": every
  ingestion entry point lacks a production caller.
- Every shell manifest sets `ingestion:
  Ingestion::AdapterIngestedNoProductionCaller`
  (`crates/daemon/examples/eval_runner/aging.rs` `suite_c_manifest`).

## Failure scenario
A report is cited as evidence that production ingestion preserves knowledge
under aging when no production path was exercised.

## Timing windows and dependencies
None.

## What a test must construct
A manifest; the label is a required field.

## Investigation log
### Q: Does any ingestion path have a production caller at HEAD?
- Sources examined: `SourcePublisher::publish` callers; the harness, claim,
  and git adapters.
- Findings: tests only.
- Missing evidence: a production caller.
- Conclusion: unresolved, needs a production caller outside this specification.
