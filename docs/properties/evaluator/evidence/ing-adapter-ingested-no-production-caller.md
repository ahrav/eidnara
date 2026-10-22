# ing-adapter-ingested-no-production-caller

## Discovery trigger
Parent specification Q-ING: "every manifest carries `adapter-ingested,
production caller: none`, the gap is recorded in `docs/properties/`, and
making an adapter production-live is outside this specification."

## Evidence trail
- `crates/eval-core/src/manifest.rs:137` the variant and its wire name.
- `docs/properties/evaluator/README.md` "Gaps recorded here": every
  ingestion entry point lacks a production caller.
- Every Suite C shell manifest (aging, fault, growth, shrink) sets
  `ingestion: Ingestion::AdapterIngestedNoProductionCaller`
  (`crates/daemon/examples/eval_runner/aging.rs` `suite_c_manifest`). The
  Suite B shell labels its arm `Ingestion::TransformRouteTurnByTurn`
  (`crates/daemon/examples/eval_runner/campaign.rs` `manifest`): the
  daemon's own route, not an adapter, and still no production caller.

## Failure scenario
A report is cited as evidence that production ingestion preserves knowledge
under aging when no production path was exercised.

## Timing windows and dependencies
None.

## What a test must construct
A manifest; the label is a required field.

## Investigation log
### Q: Does any ingestion path have a production caller at HEAD?
- Sources examined: `SourcePublisher::publish`, `publish_batch`, and
  `publish_screened` callers; the harness, claim, and git adapters.
- Findings: the earlier "tests only" answer was out of date for the claim
  adapter. `ClaimMaterializer` (`crates/daemon/src/claim_sources.rs`) runs in
  production from `crates/daemon/src/search_lifecycle_owner.rs`, where the
  search lifecycle owner materializes claim sources through
  `ClaimMaterializer::new`. The source-capture specification
  ([#1066](https://github.com/ahrav/eidnara/issues/1066), unit U2) gave the
  harness adapters a production caller: `CaptureJob::units` in
  `crates/daemon/src/source_capture.rs` calls `opencode_units` and
  `pi_units`, and `CaptureJob::publish_locked` publishes their units through
  `harness_sources::publish_screened`. That path runs only when the user tier
  sets `source_capture.enabled`, which defaults to false.
- Missing evidence: an evaluator world that ingests through either
  production caller.
- Conclusion: resolved with answer. Production callers exist for the claim
  adapter (default production) and the harness adapters (explicit
  configuration). The evaluator's worlds reach neither, so the
  `adapter-ingested, production caller: none` label stays accurate for them.
