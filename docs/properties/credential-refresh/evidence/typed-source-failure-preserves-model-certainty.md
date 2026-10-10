# typed-source-failure-preserves-model-certainty

All four checks are in `crates/daemon/src/history_summarizer.rs` and count the
model calls the scripted producer receives after the injected outcome.

- `:4621` `cross_incarnation_unknown_records_completion_backoff_without_fallback`:
  an unknown outcome recorded under another incarnation backs off without a
  fallback dispatch.
- `:4690` `a_start_failure_with_an_unproven_effect_starts_no_second_model`: a
  start failure without proof the model did not run starts no second model.
- `:4919` `a_source_failure_skips_same_provider_models_and_falls_back_to_another_source`:
  a typed source failure skips models on that source.
- `:5062` `a_source_failure_whose_cancel_is_unproven_starts_no_further_model`.
