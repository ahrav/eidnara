# typed-source-failure-preserves-model-certainty

## Discovery trigger

Specification #860, record N8: `always(unknown model effect => zero fallback
dispatch)`, and a source-scoped failure skips models on the same source and
honors the cooldown without a text policy. The parent's
`ambiguous-replay-keeps-frozen-lineage` record noted that an ordinary untagged
unknown outcome could still pick another model; N8 closes that gap. The M4
ticket (#872) lands the record with its runnable checks.

## Evidence trail

All four checks are in `crates/daemon/src/history_summarizer.rs` and count the
starts a `ScriptedProducer` observes after the injected outcome.

- `:4621` `cross_incarnation_unknown_records_completion_backoff_without_fallback`:
  an unknown outcome recorded under another incarnation leaves one observed
  start, one close, the summarizer idle, and `failure_backoff_at_ms` at
  `10_876` from a completion clock of `10_000`.
- `:4690` `a_start_failure_with_an_unproven_effect_starts_no_second_model`: a
  start failure without proof the model did not run leaves one observed start
  on `prov/model-a`, the summarizer idle, and a backoff recorded.
- `:4919` `a_source_failure_skips_same_provider_models_and_falls_back_to_another_source`:
  a `CredentialSource`-scoped transient failure on `amazon-bedrock/model-a`
  skips `amazon-bedrock/model-b` and publishes through `anthropic/model-c`
  with two observed starts.
- `:5062` `a_source_failure_whose_cancel_is_unproven_starts_no_further_model`:
  the same failure followed by a cancel that times out leaves one observed
  start and records the source's 120-second retry as
  `failure_backoff_at_ms`; a proven cancel permits the fallback with two
  starts.
- The source failure is typed through `ErrorClassification` with
  `scope: ErrorScope::CredentialSource` and `retry_after_secs`; no test
  matches on error text.

## Failure scenario

A start failure or a cancel timeout leaves the first model's effect unknown.
A fallback dispatch bills a second model run for the same chunk, and if the
first run completes after all, two lineages publish for one operation.

## Timing windows and dependencies

The window opens when a producer run's outcome is unknown: a start that fails
without proof the model did not run, a cancel whose completion is unproven, or
a reconnect under another incarnation. It closes when the backoff recorded at
completion time expires. The check depends on the producer reporting a typed
`ErrorClassification`, since an untyped failure cannot name its source scope.

## What a test must construct

A scripted producer whose first run fails with a typed, source-scoped
classification; a cancel result that is a timeout in one arm and a success in
the other; a model list with two models on the failed source and one on
another; an assertion on the observed start count, not only on the published
result; an assertion on the recorded backoff.

## Investigation log

### Q: Does the daemon ever infer source scope from error text?

- Sources examined: the `source_run_failure` helper above `:4919`, the
  `is_chunk_failure` assertions above `:5062`, the four tests.
- Findings: scope comes from `ErrorScope::CredentialSource` on the
  classification; `class_field_present: true` marks the typed path. The tests
  assert behavior from the typed fields alone.
- Missing evidence: none for the four checks read.
- Conclusion: resolved with answer - scope is typed, not parsed.
