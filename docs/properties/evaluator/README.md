# Evaluator property records

This part holds the long-horizon evaluator's property records. The evaluator
measures whether a coding agent keeps using the right project knowledge as
project history grows. Its records are `test-only`: the evaluator is a new
subsystem with no production caller, so they live here as one part with
cross-links from the stage catalogs rather than inside those catalogs, where
they would distort reachability summaries.

Records enter this directory when their checks are re-verified at the
then-current HEAD, in the METHOD field order from [`../METHOD.md`](../METHOD.md).
Until then, this file lists the executed checks the Phase 0 regression net
provides, so a reader can find them by test name.

## Phase 0 executed checks

Manifest, identity, and residue (`crates/eval-core/tests/manifest.rs`):

- `required_fields_are_sorted_and_equal_the_struct_field_set` pins
  `eval-manifest/v1` to `REQUIRED_FIELDS`; a struct field added without a
  version bump fails here. `fixture_digests_are_frozen` pins the fixture's
  `eval_run_id` and manifest digest so an encoding change is reviewed.
- `every_missing_field_is_refused_by_name_before_digesting`,
  `unknown_field_wrong_schema_and_non_object_are_refused`,
  `manifest_consistency_refusals_name_their_cause`, and
  `attestation_is_a_tagged_value` cover the typed refusals and the tagged
  attestation.
- `every_kept_field_enters_the_digest_and_every_dropped_field_leaves_it`
  mutates each kept field through a valid manifest and expects a new digest,
  and restamps the dropped fields expecting the same digest.
- `run_id_is_the_protocol_digest_of_the_full_tuple` recomputes `eval_run_id`
  with an independent SHA-256 over the nine-component tuple;
  `changing_any_identity_or_build_component_changes_the_run_id` mutates every
  component and every build sub-record field;
  `malformed_or_empty_identity_components_are_refused` covers the zero-bytes
  digest, malformed digests, and empty version strings.
- `residue_classification_is_total_over_observation_fields`,
  `clock_named_keep_fields_equal_the_pinned_allowlist`,
  `host_environment_and_incarnation_fields_are_never_kept`,
  `presence_and_relative_rules_hide_incarnation_values_but_not_their_structure`,
  and `dropped_fields_never_reach_the_trace_digest` cover the residue rules.
- `fractions_travel_as_canonical_decimal_strings` covers the decimal-string
  rule for rates and for `root_seed`.

Cross-process determinism (`crates/eval-core/tests/two_process.rs`):

- `two_process_same_identity_yields_equal_manifest_and_trace_digests` runs two
  child processes of the test binary, compares their digests with each other
  and with the parent's own computation.
- `two_process_planted_map_order_leak_fails_the_equality_test` is the negative
  control: a `Keep` field built from `HashMap` iteration order makes the same
  comparison fail, and a same-process check shows array order enters the
  trace digest.
- `two_process_changed_build_component_fails_the_identity_check` changes the
  code SHA in one child and expects a different `eval_run_id` and manifest
  digest with an unchanged trace digest.

Surface census (`crates/daemon/src/transform/surface_census.rs` and
`crates/daemon/tests/surface_census.rs`):

- `auto_search_is_enabled_by_the_wire_default_and_drives_the_hint_query`
  deserializes a request without the field, then runs the transform with the
  default, with `auto_search_enabled: false`, and as a subagent, counting hint
  queries. The plugin side is pinned by
  `packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts`,
  which asserts the transform body carries `serve_native: true` and
  `auto_search_enabled: true` under default configuration.
- `surface1_stages_are_anchored_to_production_symbols_in_pinned_order` binds
  each of the thirteen `eval_core::Surface1Stage` values to the production
  symbol path that implements it and checks the row order against
  `SURFACE1_STAGES`.
- Order facts witnessed by behavior:
  `suppression_and_the_length_gate_return_before_any_query`,
  `the_token_gate_returns_before_the_candidate_window_reads_the_store` (through
  the store's statement probe), and
  `the_threshold_empties_a_result_set_the_cap_would_otherwise_trim`. Decision
  freeze is witnessed by `empty_user_hint_decision_skips_future_queries` in
  `crates/daemon/src/transform.rs`; overlay apply by the aged golden below.
  Deferral and native attachment have no per-hint seam until the Phase 2 taps
  land and are pinned by symbol only.
- Bounds witnessed by behavior: `hint_bound_constants_equal_the_evaluator_pins`,
  `the_candidate_window_holds_exactly_one_hundred_segments`,
  `the_query_keeps_twenty_four_tokens`, `three_results_survive_the_cap`,
  `two_matched_tokens_gate_the_query_and_filter_candidates`,
  `fragments_are_cut_at_eighty_units`, and
  `the_whole_hint_is_cut_at_eight_hundred_units`.
- `surface_two_and_the_query_route_are_default_closed` sends
  `retrieval.query`, `retrieval.prepare`, `retrieval.apply`, and
  `retrieval.confirm` to a stock daemon and expects `disabled` for each. The
  installed-installer arms that show the same routes answering are
  `scope_harness_and_disable_are_decided_before_any_candidate_read` in
  `crates/daemon/tests/query_route_handler.rs` and
  `the_route_is_disabled_until_an_approved_limit_set_is_installed` in
  `crates/daemon/tests/edit_receipts.rs`.

Fresh versus aged transform goldens
(`crates/daemon/src/transform/aged_goldens.rs`,
`crates/daemon/testdata/aged-transform-golden.json`):

- `aged_history_changes_the_tail_only_when_it_matches_the_prompt` runs the
  same prompt through the transform path in a fresh world and in a hand-built
  aged world, and compares the tail text and wire digest of each against the
  golden. One case's aged history matches the prompt and attaches a hint; the
  other's is irrelevant and attaches none.
- `a_dropped_attachment_fails_the_aged_golden` drops the attachment through the
  production switch (`auto_search_enabled: false`) and expects the golden
  comparison to fail while the fresh arm still matches.
- `aged_golden_provenance_covers_every_case_input` recomputes the fixture's
  `input_sha256` over every case input and rejects a one-byte perturbation.

Placement fences (`scripts/forbid-test-support-dependencies.ts`, run in the
`gates` CI job):

- No normal, build, or target-specific dependency table names a
  `*/test-support` feature or the dev-only `eval-core` package, and no
  `default` feature reaches a `*/test-support` entry through a package's own
  feature table.

## Phase 1 executed checks: generated worlds

World generation, choice replay, and the event log
(`crates/eval-core/tests/world.rs`, fixture in `tests/support/mod.rs`):

- `generation_is_a_pure_function_of_seed_and_config`
  (`wm-generator-pure-function-of-identity`) generates the fixture twice and
  expects equal worlds and digests, then changes the seed and a config field
  and expects different digests; it pins the log header literals to
  `EVENT_SCHEMA_VERSION` and `LINEARIZATION_RULE_VERSION` and the tape
  identity to `tape_identity`, and builds a `RunIdentity` from the generator
  constants to show that the seed, the config, and each constant change
  `eval_run_id`.
- `keyed_draws_are_pinned_and_distinct_per_kind_actor_site_and_occurrence`
  (`wm-independent-rng-streams-per-axis-entity`) freezes one `keyed_draw`
  value so a key-shape or protocol change is reviewed, and shows that
  changing the kind, actor, site, occurrence, or seed changes the draw.
- `every_choice_kind_is_recorded_and_the_tape_replays_the_same_world`
  (`wm-typed-choice-tape-and-replay-refusal`) checks that every `ChoiceKind`
  appears on the tape with an occurrence above zero somewhere, that
  `Mode::ReplayTape` over the unmodified tape reproduces the world through
  `generate_all` and through the step drive, and recomputes the candidate
  digest of every citation and revision-target choice from the log so the
  digest is shown to cover candidate values.
- `replay_refuses_a_missing_changed_or_out_of_range_choice_and_emits_no_log`
  (`wm-typed-choice-tape-and-replay-refusal`) truncates the tape, changes a
  candidates digest, sets an out-of-range index, and moves an entry's actor,
  each at the first, a middle, and the last entry, and expects the typed
  refusal naming that entry; probes the exact index boundary on a choice with
  one candidate; expects a tape replayed under another seed or config to
  refuse with `TapeMismatch`; expects a tape with an entry past the last
  choice to replay the same world and return a tape holding only the consumed
  entries; and after a mid-drive refusal expects `step`, `log`, and `finish`
  to return the same error.
- `removing_an_unrelated_event_changes_no_later_text_or_rename_digest`
  (`wm-independent-rng-streams-per-axis-entity`) shortens one session and,
  separately, the repository, and expects every event on the other streams
  to keep its text, revision target, rename digest, and both times, checking
  that later events on two other streams exist. Adding a repository shows the
  one deliberate coupling: only `cites` fields move, never text. The negative
  control is a hand-built generator with one sequential SHA-256 stream, which
  fails the same comparison.
- `events_are_self_contained_and_any_single_deletion_leaves_a_valid_log`
  (`wm-events-self-contained-under-deletion`) requires all six payload kinds,
  a rename chain, a cross-stream citation, and well-formed unique commit oids
  in the fixture, deletes every event in turn through `EventLog::without`,
  expects the result to validate with every surviving event byte-equal to the
  original, and expects a deletion that leaves edges behind (`DanglingEdge`),
  a positional id (`IdNotDerived`), two events sharing an id (`DuplicateId`),
  and each wrong header literal (`SchemaMismatch`) to be refused. The
  deletion rule is "drop the incident edges, repair nothing"; the record's
  open question is resolved that way because the ticket forbids repairing
  surviving payloads.
- `the_log_is_linearized_by_the_versioned_key_and_equality_checks_the_edges`
  (`wm-linearization-versioned-total-order`) recomputes every `causal_depth`
  as the longest path over the edges, recomputes the order from the key tuple
  with those depths and the stream wire names, pins the wire names and their
  order, checks key uniqueness, checks every edge runs forward, requires
  same-millisecond events on two streams and a cross-stream edge, and expects
  two logs with equal events and different edges to compare unequal; swapped
  events, a reversed edge, and a flattened depth are refused by name.
- `logs_tapes_and_times_round_trip_through_serde_in_canonical_form`
  (`wm-typed-choice-tape-and-replay-refusal`) round-trips the log and the tape
  through JSON, refuses unknown fields on both, and accepts only the
  `i64::to_string` form for times (a number, a leading zero, a sign, a space,
  an empty string, and an exponent are refused).
- `time_domain_and_observation_lead_are_enforced` (C-DST-6; no `wm-` record
  owns the time domain) checks generated worlds never lead observation time,
  and that the validator refuses a lead past `MAX_REVISION_LEAD_MS`, a valid
  time past `MAX_VALID_TIME_MS`, and a negative observation time, while an
  epoch at the domain edge refuses as `TimeOverflow` before generation.
- `the_event_bound_is_refused_before_generation_and_by_the_validator`
  (`wm-log-and-observation-bounds`) expects a bound one below the declared
  count to refuse from `Generator::new` and `generate_all` with `EventBound`,
  a `u32::MAX` message count to refuse in well under a second through the
  slot-count gate, a bound equal to the declared count to pass, the validator
  to refuse `EventBound` at one below, and each `InvalidField` refusal (bound,
  tick, epoch, an entity with no slots, no entities) plus a missing, unknown,
  or fractional field at parse time. The record asks for an exported
  `MAX_EVENTS_PER_LOG` constant; the ticket's profile clarification makes it
  a required config field with no default instead.
- `declared_events_equals_the_emitted_count_across_spec_shapes`
  (`wm-log-and-observation-bounds`) generates 432 worlds over a grid of
  message counts and `*_every` values and expects the declared count to equal
  the emitted count in each, which is what keeps the drive-time bound check
  a consistency assertion rather than a reachable refusal.
- `generate_all_equals_the_step_fold_and_every_partial_log_validates`
  (`wm-generate-all-equals-step-drive`) folds `step` until `Done`, checks
  every batch is one entity at one valid time with exactly the slot's
  independently restated event count, validates the re-linearized snapshot
  after every step, bounds the step count by the slot count, expects `Done`
  to repeat, expects the folded world to equal `generate_all`, expects the
  emission-ordered batches sorted by the key to equal the log, expects
  `finish` after one step to drive the rest, and shows a batch cut before
  quiescence fails the per-slot count.

Cross-process determinism (`crates/eval-core/tests/two_process.rs`):

- `two_process_same_world_inputs_yield_equal_log_and_tape_digests`
  (`wm-generator-pure-function-of-identity`, `wm-no-map-order-reaches-output`)
  runs two child processes that generate the shared fixture (the child asserts
  every choice kind is drawn) and compares their log and tape digests with
  each other and with the parent; a reseeded child differs. The planted-map
  negative control from Phase 0 exercises the trace digest path in the same
  file; the log and tape have no map-backed field, and
  `crates/eval-core/clippy.toml` disallows `HashMap` and `HashSet` in the
  crate.

The coverage markers the records name (`wm_generator_two_processes_compared`,
`wm_removal_two_live_streams_after_event`, `wm_tape_each_choice_kind_recorded`,
`wm_log_has_cross_event_reference`,
`wm_log_has_same_millisecond_cross_stream_events`,
`wm_log_has_cross_stream_causal_edge`, `wm_bound_refusal_arm_entered`,
`wm_step_drive_validated_between_steps`) are asserted inline as preconditions
in the tests above; the evaluator-owned marker registry lands with the
ingestion ticket and these assertions move onto it then.

## Gaps recorded here

- Every ingestion entry point lacks a production caller. No world is labelled
  "validated real ingestion" until one exists; manifests carry
  `adapter-ingested, production caller: none` once ingestion lands.
- `MAX_EVENTS_PER_LOG` is a required `WorldConfig` field with no default, not
  a top-level manifest field; it reaches the manifest through
  `run_identity.config`. Promoting it to a named manifest field is a schema
  bump deferred to pre-run profile approval.
- The 800-unit total cap is not reachable through `render_user_hint` under the
  80-unit fragment cap and 3-result cap; the census exercises the cap function
  directly.
- Residue rules apply to top-level observation fields. Nested values under
  `Keep` enter the digest whole; per-tap observation schemas in Phase 2 must
  flatten or register nested records.
