# Evaluator property records

This part holds the long-horizon evaluator's property records. The evaluator
measures whether a coding agent keeps using the right project knowledge as
project history grows. Its records are `test-only`: the evaluator is a new
subsystem with no production caller, so they live here as one part with
cross-links from the stage catalogs rather than inside those catalogs, where
they would distort reachability summaries.

Records enter this directory when their checks are re-verified at the
then-current HEAD, in the METHOD field order from [`../METHOD.md`](../METHOD.md).
Until then, this file lists the executed checks the Phase 0 regression net,
the Phase 1 world model, the Phase 2 stage ledger, and the Phase 3 cassette
provide, so a reader can find them by test name.

## Phase 0 executed checks

Manifest, identity, and residue (`crates/eval-core/tests/manifest.rs`):

- `required_fields_are_sorted_and_equal_the_struct_field_set` pins
  `eval-manifest/v5` to `REQUIRED_FIELDS`; a struct field added without a
  version bump fails here. `fixture_digests_are_frozen` pins the fixture's
  `eval_run_id` and manifest digest so an encoding change is reviewed.
- `every_missing_field_is_refused_by_name_before_digesting`,
  `unknown_field_wrong_schema_and_non_object_are_refused`,
  `manifest_consistency_refusals_name_their_cause`,
  `residue_declarations_are_non_keep_and_one_rule_per_field`,
  `validate_refuses_what_parse_and_digest_refuse`,
  `arm_rates_stay_within_the_unit_interval`,
  `provenance_strings_are_non_empty`, and `attestation_is_a_tagged_value`
  cover the typed refusals and the tagged attestation.
- `every_kept_field_enters_the_digest_and_every_dropped_field_leaves_it`
  mutates each kept field through a valid manifest and expects a new digest,
  and restamps the dropped fields expecting the same digest.
- `run_id_is_the_protocol_digest_of_the_full_tuple` recomputes `eval_run_id`
  with an independent SHA-256 over the nine-component tuple;
  `changing_any_identity_or_build_component_changes_the_run_id` mutates every
  component and every build sub-record field;
  `identity_validate_refuses_what_the_run_id_refuses` and
  `malformed_or_empty_identity_components_are_refused` cover the zero-bytes
  digest, malformed digests, and empty version strings.
- `residue_classification_is_total_over_observation_fields`,
  `clock_named_keep_fields_equal_the_pinned_allowlist`,
  `host_environment_and_incarnation_fields_are_never_kept`,
  `a_field_declared_twice_is_refused_at_schema_construction`,
  `presence_and_relative_rules_hide_incarnation_values_but_not_their_structure`,
  `a_refused_observation_leaves_relative_numbering_unchanged`,
  `a_kept_value_the_digest_cannot_encode_is_refused_at_record_time`,
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
  `suppression_and_the_length_gate_return_before_any_query` and
  `the_token_gate_returns_before_the_candidate_window_reads_the_store` (through
  the store's statement probe). Decision
  freeze is witnessed by `empty_user_hint_decision_skips_future_queries` in
  `crates/daemon/src/transform.rs`; overlay apply by the aged golden below.
  The threshold reads only the top-ranked score and the cap keeps the
  top-ranked result, so threshold-before-cap has no observable effect and is
  pinned by symbol only, like deferral and native attachment, which have no
  per-hint seam until the Phase 2 taps land.
  `the_threshold_empties_a_result_set_the_cap_would_otherwise_trim` witnesses
  that the threshold is all-or-nothing: five below-threshold candidates yield
  no results, not three.
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

- `aged_history_attaches_a_hint_only_when_it_matches_the_prompt` runs the
  same prompt through the transform path in a fresh world and in a hand-built
  aged world, and compares the tail text and wire digest of each against the
  golden. One case's aged history matches the prompt and attaches a hint; the
  other's is irrelevant and attaches none, asserted directly on the hint
  marker and through the golden equality. The aged tail still differs from
  the fresh tail in its `§n§` ordinal, because the prompt is the seventh wire
  item rather than the first; that ordinal is not the attachment.
- `a_dropped_attachment_fails_the_aged_golden` drops the attachment through the
  production switch (`auto_search_enabled: false`) and expects the golden
  comparison to fail while the fresh arm still matches.
- `aged_golden_provenance_covers_every_case_input` recomputes the fixture's
  `input_sha256` over every case input and rejects a one-byte perturbation.

Placement fences (`scripts/forbid-test-support-dependencies.ts`, run in the
`gates` CI job):

- No normal, build, or target-specific dependency table names the dev-only
  `eval-core` package or requests a feature that turns on test-support in the
  target package, directly or through a forwarding alias in the target's
  feature table or in a local package that table forwards to, and no local
  package's `default` feature enables its own
  `test-support` or reaches a `*/*test-support` entry.

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
  events, a reversed edge, swapped or duplicated edges, and a flattened depth
  are refused by name.
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
  a `u32::MAX` message count to refuse in well under a second, including with
  the bound at `u32::MAX` and a tool span on every slot (the count is
  arithmetic, not a per-slot sweep), a bound equal to the declared count to
  pass, the validator
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

The coverage markers the world-model records name
(`wm_generator_two_processes_compared`,
`wm_removal_two_live_streams_after_event`, `wm_tape_each_choice_kind_recorded`,
`wm_log_has_cross_event_reference`,
`wm_log_has_same_millisecond_cross_stream_events`,
`wm_log_has_cross_stream_causal_edge`, `wm_bound_refusal_arm_entered`,
`wm_step_drive_validated_between_steps`) are asserted inline as preconditions
in the tests above. The registry (`eval_core::MARKERS`) holds the ingestion
suite's markers, whose completeness proof runs in one test binary; the
world-model assertions join it when a single run can witness them all.

## Phase 1 executed checks: eligibility spec and reducer

Spec pin and reduction (`crates/eval-core/tests/reducer.rs`):

- `the_in_code_table_digests_to_the_pinned_constant_in_judge_order`
  (`wm-reducer-spec-fixture-digest-pinned`) digests `serialize_spec()` to
  `ELIGIBILITY_SPEC_DIGEST`, compares the predicate order to a list written in
  the test by hand, requires every verdict to have a vector and the
  both-columns-set cell and a remote destination to appear, checks all 22
  vectors against `judge`, requires the serialized spec to carry no numbers
  (so a matching digest is value equality and `check_spec` has no shape
  error), and round-trips the spec through serde.
- `every_predicate_and_the_surface_fold_are_reachable_from_facts`
  (`wm-reducer-agrees-with-judge-fact-tuples`) flips one fact at a time from a
  live labeled tuple and expects each predicate's verdict, checks the served
  versus registry sensitivity derivation and the remote refusal, and checks
  the fold on labeled, automatic, and split served classes.
- `reordered_predicates_an_added_verdict_or_a_changed_cell_are_spec_drift`
  (`wm-reducer-spec-fixture-digest-pinned`) mutates the serialized spec four
  ways and expects `SpecDrift { expected, found }`, expects a fractional cell
  to refuse as `NotCanonical`, shows a whitespace-only re-serialization
  passes, shows `reduce` refuses both and yields no truth, and shows every
  adjacent transposition of the order disagrees with some vector except the
  first pair, which no realizable tuple separates.
- `reduction_agrees_with_an_independent_scan_at_every_cut`
  (`wm-reducer-agrees-with-judge-fact-tuples`) reduces the shared world at
  every distinct valid time times every distinct observation time (plus the
  edges) and compares every unit's facts, verdict, and the required set with
  an independent scan written in the test; `stale` never appears because
  units are judged at their own revision.
- `a_correction_takes_over_when_true_and_known_and_an_invalidation_only_retracts`
  pins a named correction and target, reduces before, at, and with the
  correction true but not yet known, reduces an invalidation in a
  correction-free world, and shows a correction whose target was deleted from
  the log leaves every other unit unchanged.
- `admission_and_destination_travel_with_the_query_and_refusals_are_typed`
  checks out-of-scope, unadmitted, and sensitive-remote worlds, purity, and
  every refusal by name (`NotLinearized`, `SchemaMismatch`, `EventBound`, and
  each `InvalidQuery` field).
- `truth_and_queries_round_trip_through_serde_in_canonical_form` pins
  `Truth::reducer_version` to `REDUCER_VERSION` (`eval-reducer/v1`), the
  decimal cut times, the state field names, the surface wire names, and
  unknown-field refusal on `Truth` and `Query`.
- `an_enumerate_run_records_its_mode_and_the_pinned_spec_digest`
  (`crates/eval-core/tests/manifest.rs`) parses a manifest with
  `execution_mode: enumerate` and the pinned spec digest and shows the mode
  enters the manifest digest.

Kernel differential (`crates/kernel/tests/eligibility_spec.rs`, fixture in
`crates/kernel/tests/support/eligibility_fixture.rs`, spec file at
`crates/kernel/testdata/eligibility-spec-v1.json`):

- `the_kernel_fixture_and_the_evaluator_table_pin_one_digest_in_judge_order`
  (`wm-reducer-spec-fixture-digest-pinned`) reads the kernel-owned file,
  expects its digest to equal `ELIGIBILITY_SPEC_DIGEST` and its value to equal
  `serialize_spec()`, compares the predicate names to the order copied from
  `judge` by hand, accepts a whitespace-mangled copy, and refuses a swapped
  pair, an added verdict, and a changed cell.
- `the_reducer_agrees_with_the_kernel_on_the_hand_authored_fact_tuple_table`
  (`wm-reducer-agrees-with-judge-fact-tuples`) runs a 24-row hand-authored
  table (every verdict class, the precedence pairs, the
  superseded-and-invalidated cell, an automatically served object and labeled
  ones, a sensitive artifact allowed locally and a secret artifact denied
  everywhere, a never-written object) over both destinations and all three
  surfaces in `enumerate` mode: the kernel's `judge_surface_eligibility` and
  the evaluator's `judge_surface` must both return the row's literal verdict
  and visibility and agree on `permits`, and the row's hand-authored facts
  must equal the tuple projected from the store's `egress_candidates`. The
  kernel assertion precedes the reducer assertion, so the kernel breaks first.
  The destinations and surfaces come from the kernel's own
  `ArtifactDestination::ALL` and `Surface::ALL`, every kernel enum is mapped
  onto its mirror by an exhaustive match, and the surfaces seen must equal
  the mirror's `Surface::ALL`, so a kernel variant the mirror lacks fails to
  compile or fails the test rather than going untested.
  The test records that the admission policy gives both automatic surfaces
  one visibility today.
- `every_adjacent_transposition_of_the_order_disagrees_with_the_table` shows
  a wrong precedence is rejected by the table on its own terms for every
  adjacent pair but the first, which no store object can separate.

Fences (`wm-eval-core-dependency-fence`, `xc-core-oracles-take-values-only`,
`mtr-eval-core-dev-only-member-host-graph-unchanged`):
`scripts/forbid-test-support-dependencies.ts` now asserts `eval-core`'s normal
dependency set is exactly `context-core`, `serde`, `serde_json`, `sha2`, and
that no line of `crates/eval-core/src` names a product crate or a `std`
effect module (`fs`, `path`, `process`, `time`, `net`, `env`, `io`), as a full
path or as a member of a brace-grouped `use std::{...}`, with negative cases
in its unit test; an empty scan is itself a finding, so the fence cannot pass
on a wrong working directory or a moved crate; `eval-core` enters the kernel
only under `[dev-dependencies]`; `cargo tree -p daemon -e normal` is
unchanged.

## Phase 1 executed checks: rendering and ingestion

Identity rule (`crates/kernel/tests/eval_identity.rs`):

- `the_core_encoder_reproduces_every_identity_golden_and_the_kernel_encoder`
  (`wm-occurrence-identity-recomputation`) encodes all 23 records of the
  independent identity goldens with the evaluator's copy and the kernel's
  `encode_preserving_span`, expects both to equal the expected occurrence and
  lineage ids, pins `OCCURRENCE_ENCODING_VERSION`, the contract version,
  `MAX_IDENTITY_VALUE_BYTES`, `HARNESSES`, and `OBJECT_FORMATS` to the
  kernel's (the goldens hold no identity value between 65 and 512 bytes, so
  the limit is pinned directly), and compares the refusal names and order on
  the tuple-level invalid records.
- `a_later_message_time_changes_the_occurrence_but_not_the_lineage` is the
  identity-mapping matrix: a message twin with a later completion time is a
  new occurrence of the same lineage; a commit's time is not an input.

Renderer, encoder, accounting, and registry, store-free
(`crates/eval-core/tests/render.rs`):

- `rendered_messages_have_the_shape_the_adapter_reads_with_valid_time_as_revision`
  pins the OpenCode message JSON the renderer emits: a user turn carries
  `time.created` at its valid time, an assistant turn carries
  `time.completed` at its valid time with `created` one millisecond earlier,
  a tool span renders as a settled `tool` part whose `state.time.end` is the
  valid time, and each expected unit's identity equals an independent
  `encode` of the tuple.
- `every_payload_kind_renders_to_units_a_commit_or_a_named_exclusion` renders
  a world with every payload kind: messages and corrections become messages,
  tool spans attach to their own session's message only (a span whose session
  has no message with its `message_id` refuses as `ToolSpanParentMissing`, and
  one observed at another time than its parent as
  `ToolSpanObservationDiffers`), commits become
  `RenderedCommit`s at the event's times, renames and invalidations are
  counted under their rule names, every unit has a distinct identity, and a
  correction reuses its target's `message_id` and lineage at a later
  revision.
- `render_refuses_bad_targets_roles_times_and_unencodable_identities_by_event`:
  `CorrectionTargetIsNotAMessage` for a commit target,
  `CorrectionTargetMissing` for an unknown one,
  `CorrectionTargetInOtherSession` for a target in another session (a
  same-session correction one millisecond later shares the target's lineage),
  `CorrectionDoesNotAdvance` for a correction at or before the target's valid
  time, `MessageIdReused` for two base messages with one `message_id` in one
  session, `OccurrenceReused` for a second correction of one target at one valid
  time and for a second tool span with one `call_id` at one valid time,
  `SecondRepository` for commits from two repository entities under one
  `repository_id`, `UnknownRole` for a role that
  is neither `user` nor `assistant`, `NoEarlierCreated` for an assistant turn
  at valid time zero (valid time one renders `created: 0`), and an identity
  value with a tab refuses as `Encoding { event_id, .. }` naming the event,
  with `MalformedIdentityValue` checked before the oid format.
- `the_identity_flip_matrix_names_what_enters_each_id` is the tuple-level
  matrix: each identity field moves both ids, revision moves the occurrence
  only, representation and span move both, the same tuple is one identity,
  and `UnknownClass`, `MissingIdentityField`, `MalformedRevision`,
  `MissingRevision`, and `UnknownRepresentation` fire for their malformed
  input (the identity goldens in `eval_identity.rs` fire all nine variants
  in kernel order).
- `accounting_names_every_way_a_unit_can_go_missing` and
  `the_marker_registry_is_unique_and_incomplete_until_every_marker_fires`
  cover `check_accounting` and `Coverage` off the store.

Adapters (`crates/daemon/tests/eval_ingestion.rs`; every scenario records its
marker through `eval_core::Coverage` after asserting its preconditions):

- `rendered_worlds_round_trip_through_the_opencode_adapter_with_exact_accounting`
  (`ing-adapter-round-trip-ids-equal-expected`,
  `ing-ingestion-outcome-accounting-no-silent-loss`) publishes a rendered
  world through `opencode_units` and `SourcePublisher::publish`, expects the
  published identity set to equal the renderer's expected set with no
  refusals, expects each correction to replace the latest predecessor of its
  lineage and every first publication to replace nothing, expects a
  republish to replay every receipt, then adds a step marker, an ignored text
  part, and a running tool to one message and a tool without a `callID` to
  another and expects the dropped parts to move no identity, the broken
  message to refuse every unit it carried as `MissingIdentity`, and
  `expected == published + refused` to hold; removing one outcome or
  double-counting one fails `check_accounting` by name.
- `generated_observations_never_lead_and_a_boundary_fixture_refuses`
  checks every rendered revision is at or before its observation time and in
  the valid-time domain, publishes the world without a refusal, and shows a
  hand-built unit one millisecond past `MAX_REVISION_LEAD_MS` refuses as
  `RevisionAhead` with no tip movement while exactly one hour of lead passes.
- `git_units_keep_revision_one_and_take_valid_time_from_the_projection`
  (`ing-git-units-fixed-revision-drop-commit-time`) writes each rendered
  commit plus two commits that differ only in committer time into a real
  repository, reads them with `read_selection`, destructures `SourceUnit`
  (no time field), expects revision `"1"`, the identity field set, and
  `commit_message`, checks the evaluator's `oid -> valid_time_ms` projection
  against the committer time git recorded for each oid, publishes each, and
  expects the published identity to equal `git_identity(oid)`.
- `observation_time_is_inert_for_identity_and_eligibility`
  (`ing-observation-time-inert-for-projection`) ingests one rendering into two
  stores ten years apart in observation time and expects equal occurrence
  ids, object ids, and eligibility verdicts, then republishes at a later
  observation time and expects a replay with the stored `observed_at`
  unchanged. Residue classification from this evidence: `observed_at_ms`
  stays `Keep` as an evaluator-controlled parameter.
- `hold_embedding_commit_correction_release_query_makes_the_predecessor_obsolete`
  (the four-seam witness, marker `ing_four_seam_hold_correct_release_query`)
  publishes a predecessor, bootstraps a projection with its embedding job
  pending, shows a same-time and a backdated correction refuse as broken
  fixtures, commits the correction through the adapter, catches the
  projection up, releases the held embedding into
  `Obsolete(Canonical(Superseded))` while the successor embeds, and runs the
  query route: the successor is returned and the predecessor is not; a
  republish of the successor replays and leaves the result unchanged.
- `each_adapter_identity_field_flips_its_ids_and_payload_fields_flip_none`
  (`ing-adapter-round-trip-ids-equal-expected`) takes a rendered assistant
  message with a text and a tool part through `opencode_units` and flips one
  field at a time: `info.id`, `sessionID`, and `project_id` move both ids;
  `time.completed` and the text part's position move the text unit only;
  `callID` and `state.time.end` move the tool unit only; `text`, `role`, and
  the tool output move neither.
- `coverage_markers_are_unique_and_each_names_a_scenario_here` and
  `every_registered_marker_fires_across_the_scenarios` are the registry's
  uniqueness test and completeness proof: the second runs every scenario once
  and requires `Coverage::complete` to succeed, so it runs in every CI pass.

Feature closure (`crates/daemon/tests/eval_seam_probe.rs`,
`sls-evaluator-target-feature-closure-complete`): names every adapter,
kernel, publication, catch-up, route, and retrieval `test-support` seam the
shell uses, so the `--all-features` daemon build fails when one moves;
retrieval's `test-support` feature is enabled only under the daemon's
`[dev-dependencies]`.

Manifest (`crates/eval-core/tests/manifest.rs`): `ingestion` is a required
field (`ing-adapter-path-no-production-caller-labelled`); its wire value is
`adapter-ingested, production caller: none`, and `direct-database, non-aged`
with a `replay` construction is refused as `DirectDatabaseAged`.

## Phase 2 executed checks: stage ledger on the activated chain

Ledger join (`crates/eval-core/tests/ledger.rs`):

- `the_chain_stages_are_pinned_in_production_order_with_their_kinds` pins
  `CHAIN_STAGES` (exact, lexical, dense, eligibility, fusion, selection,
  packing), their `Source`/`Filter` kinds, their ordinals, and their wire
  names.
- `stage_presence_is_three_valued_and_an_unjoinable_stage_has_none`
  (`ldg-stage-presence-three-valued`): `NotReached`, `ReachedEvidenceAbsent`,
  and `Reached` from one ledger, and `None` for a stage whose only output is
  `Unjoinable`.
- `the_first_loss_is_the_earliest_absent_stage_on_the_entry_path` drops the
  rule from one stage at a time under three entries and expects a loss only on
  the entry's path; `the_earliest_loss_wins_and_later_absences_are_not_a_second_verdict`,
  `an_unreached_entry_is_indeterminate_not_a_loss_downstream`,
  `a_chain_that_never_reaches_the_terminal_stage_is_indeterminate_not_clean`
  (also with nothing required: an unreached or unjoinable terminal, or an
  empty ledger, is never `Clean`),
  `an_unobserved_filter_between_observed_stages_is_transparent`, and
  `a_stage_that_ends_the_request_reports_an_empty_output` fix the edges.
- `an_unjoinable_stage_is_never_evidence_absence`: a rule present after an
  unjoinable stage is `Clean`; a later sighting places a later loss; an
  absence right behind the unjoinable stage, or an unjoinable last stage, is
  `Indeterminate`.
- `every_permutation_of_the_observations_folds_the_same` folds all 720
  orders of six observations (two of them contradicting) and all 120 orders
  of the five distinct ones to one ledger, one verdict, and one presence per
  stage; `the_highest_sequence_is_the_stage_output` and
  `identical_duplicates_are_idempotent_and_contradictions_are_indeterminate`
  (`ldg-duplicate-observations-idempotent-contradictions-indeterminate`) pin
  sequence and duplicate semantics, the latter in both arrival orders.
- `a_fold_accepts_exactly_one_commit_read_incarnation`
  (`ldg-observation-carries-incarnation-cross-incarnation-refuses`): two
  tokens are `Indeterminate`; a return carrying no token asserts nothing.
- `a_delivered_stale_occurrence_names_the_stage_it_entered`: `StaleIngress`
  names the ingress stage only when the stale occurrence reaches the terminal
  stage, and names where the delivered copy entered: a filter that removed an
  earlier copy clears that sighting, so a reintroduced occurrence is attributed
  to the reintroducing stage; loss and ingress order by ordinal with the loss
  winning ties; a stale occurrence seen before an unjoinable terminal, or first
  seen right behind an unjoinable stage, is `Indeterminate`, while an
  unjoinable stage after a known sighting keeps that ingress, a filter's output
  without it closes the opacity, and its removal before the terminal is
  `Clean`.
- `a_stage_observation_over_the_kernel_batch_is_refused` pins
  `MAX_CANDIDATES_PER_STAGE_OBSERVATION` at 1024 with a refusal at 1025;
  `over_bound_stage_returns_are_unjoinable_instead_of_panicking` pins the
  daemon shell's mapping of that refusal to `Unjoinable` at the source and
  combined eligibility stage;
  `a_packing_refusal_is_a_loss_at_a_bound_and_unjoinable_at_a_fault` pins the
  shell's packing refusals: a bound is an empty output, a fault is
  `Unjoinable`.
- `completed_folds_compare_only_within_one_persisted_store`: `agrees_with`
  answers within one `database_incarnation_id`, refuses `CrossStore`, and the
  fold round-trips through JSON.

Production taps and the seven injections (`crates/daemon/tests/eval_ledger.rs`,
`ldg-seven-fault-self-test-classifies-each-injection`,
`ldg-seven-fault-injections-reached`): each scenario asserts its
preconditions from the returned values, records its `ldg_` marker, and
classifies to its stage.

- `exact_page_bound_loses_the_rule_at_the_exact_lane`: `exact_pages` and
  `exact_page_rows` of one leave the lane `Incomplete("page_bound")` with the
  last rule row unread; `FirstLoss(Exact)`.
- `lexical_accepted_bound_loses_the_rule_at_the_lexical_lane`:
  `lexical_accepted` of one leaves the lane `Incomplete("accepted_bound")`
  with one ranked hit; a lexical-only hit is `FirstLoss(Lexical)`.
- `dense_k_bound_loses_the_rule_at_the_dense_lane`: the exhaustive producer
  at `k = 1` ranks the reference's first row only; the second is
  `FirstLoss(Dense)`.
- `a_retired_object_loses_the_rule_at_eligibility`: after `retire("rule")`
  the exact report reads the rule rows and judges them `PolicyExcluded`;
  `FirstLoss(Eligibility)`.
- `fused_union_bound_loses_the_rule_at_fusion`: `fused_union` of one refuses
  the request as `fused_union` after the lanes admitted the rule;
  `FirstLoss(Fusion)`.
- `result_rows_bound_loses_the_rule_at_selection`: `result_rows` of one
  truncates a fused, revalidated-eligible rule out of the body;
  `FirstLoss(Selection)`.
- `optional_budget_loses_the_rule_at_packing`
  (`ldg-packing-attribution-via-charged-range-members-join`): the packer
  reads every fused entry from the same projection file the route read; under
  a wide budget the `Charged::Range` members join equals the fused set, and a
  budget short by the rule's group cost skips that group;
  `FirstLoss(Packing)`.
- `the_clean_chain_keeps_the_rule_at_every_stage_and_folds_compare_within_one_store`:
  every stage except the undeclared dense lane is `Reached`, one incarnation
  token is seen by both the shell and the ledger, and the fold completes under
  `KernelStore::database_incarnation_id_within_budget`.
- `reports_from_two_stores_carry_two_incarnations_and_fold_indeterminate`:
  eligibility reports from two kernel stores intern to two tokens and the
  fold is `Indeterminate`.
- `a_held_classification_window_makes_eligibility_unjoinable_never_absent`
  (`sls-eligibility-report-non-reusable-window-constructible`):
  `hold_classification_change_for_test` from the admission phase leaves the
  exact and lexical lanes `snapshot_changed`, returns a non-reusable report as
  `ExactAdmission::Moved`, folds eligibility as `Unjoinable` (`presence` is
  `None`), and the verdict is `Indeterminate`;
  `a_window_opened_at_fusion_makes_revalidation_unjoinable_not_a_selection_loss`
  does the same from the fusion phase, so a moved revalidation is never a
  selection loss; `an_exact_lane_that_ends_after_reading_leaves_eligibility_unjoinable`
  covers `KernelError` after rows were read and an exact lane that read
  nothing.
- `tap_returns_leave_production_output_byte_equal`: a direct `execute` and
  the observing shell produce byte-equal bodies, equal statuses, fused sets,
  exact reports, and revalidation reports; two packer runs produce equal
  admissions and `read_occurrences` names every request.
- `a_misattributed_observation_and_missing_coverage_fail_the_self_test`: a
  shell that swaps the fusion and selection labels names `FirstLoss(Fusion)`
  where `FirstLoss(Selection)` is expected, and a coverage missing one `ldg_`
  marker is `Incomplete { missing }` naming it.
- `ledger_markers_each_name_a_scenario_here` and
  `every_ledger_marker_fires_across_the_scenarios` are this suite's registry
  check and completeness proof over the `ldg_` prefix
  (`xc-shell-uses-kernel-value-types-directly`: the shell names
  `CommitReadIncarnation`, `EligibilityReport`, `OccurrenceId`, and
  `Disposition` directly, with no wrapper trait or newtype).

## Phase 2 executed checks: default surface and failure classes

Failure-class table (`crates/eval-core/tests/failure_class.rs`,
`xc-failure-class-separation-truth-table`):

- `the_table_has_forty_eight_distinct_cells_in_axis_order` and
  `every_cell_agrees_with_the_hand_authored_rows`: all 48 cells are reached
  and each agrees with a twelve-row hand-authored table over (durable state,
  delivery) with live and cassette columns; passes are never classified.
- `reasoning_is_claimable_only_in_the_live_slice`,
  `the_serialized_table_digests_to_the_pinned_constant` (the manifest's
  `failure_class_table_digest`; `crates/eval-core/tests/manifest.rs` refuses a
  different value as `FailureClassTableMismatch`), and
  `a_ledger_verdict_maps_to_its_delivery_without_the_stage`.
- `evaluator_document_table_agrees_with_classify` reads the twelve-row table
  in `docs/evaluator.md` under `Failure classes` and checks each row's live and
  cassette class against `classify`, so the document is a test input and a
  prose-versus-code disagreement fails here.
  `evaluator_document_agrees_with_the_manifest_constants`
  (`crates/eval-core/tests/manifest.rs`) does the same for the manifest
  section: the schema literal, the digest protocol, the required-field count,
  the version history, and every versioned `eval-manifest` literal must name
  `MANIFEST_SCHEMA`'s version.

Surface 1 through the direct-host fixture
(`crates/daemon/tests/eval_surface_ledger.rs`; `sls-surface1-stage-list-pinned`
through `Surface1Stage: Stage` over `SURFACE1_STAGES`):

- `a_matching_segment_is_delivered_through_every_stage`: one matching segment
  is `Reached` at all thirteen stages, the verdict is `Clean`, and every
  selected segment's stored native identity resolves through the production
  adapter and the evaluator encoder to the renderer's expected id
  (`ldg-terminal-survivor-proof-captured-host-side`: `attached` is the host's
  own check, read back over the fixture's control socket).
- `an_old_segment_outside_the_window_is_lost_at_the_candidate_window`
  (`sls-surface1-candidate-window-injection-forces-first-loss`): 101 segments,
  the oldest matches, the window holds the newest 100; `FirstLoss(CandidateWindow)`,
  distinct from the in-window rejections below.
- `a_short_prompt_is_lost_at_the_length_gate`
  (`sls-surface1-prompt-gate-injection-forces-first-loss`): a prompt under 20
  characters; `FirstLoss(LengthGate)` with the search stages `NotReached`.
- `a_raised_threshold_is_lost_at_the_threshold`
  (`sls-surface1-score-injection-forces-first-loss`): the segment matches and
  the threshold of 1.5 empties the result; `FirstLoss(Threshold)`.
- `a_fourth_match_is_lost_at_the_cap`
  (`sls-surface1-render-cap-injection-forces-first-loss`): four of twelve
  segments match with equal scores; the cap keeps three; `FirstLoss(Cap)` for
  the fourth and `Clean` for the first.
- `a_native_array_without_the_tail_is_lost_at_attachment`
  (`sls-surface1-attach-injection-forces-first-loss`): the served block carries
  the hint and no native message does; `Attachment` is
  `ReachedEvidenceAbsent`; `FirstLoss(Attachment)`.
- `a_false_survivor_from_the_adapter_fails_the_self_test`: an adapter-supplied
  `attached` would fold the same run `Clean`; a mis-mapped identity does not
  reproduce the expected verdict.
- `a_repeated_pass_reports_the_frozen_decision_as_unjoinable`: the second pass
  over the same tail is `Skipped { already_decided }`, folds the tail as
  unjoinable, and the verdict is `Indeterminate`, never a tail loss.
- `the_user_hint_pass_leaves_the_wire_response_bytes_unchanged`: a
  `TransformResponse` serializes to the same bytes with no pass, a skipped
  pass, and a decided pass; `Surface1Stage::REACHABILITY` is
  `default-production` and `ChainStage::REACHABILITY` is `test-only`
  (`crates/eval-core/tests/ledger.rs`), so the two ledgers' verdicts carry
  distinct labels.
- Every injection's verdict classifies as `interference` under a held store
  and a cassette slice; the clean run classifies as `indeterminate` under a
  cassette and `reasoning` live.
- `surface_markers_each_name_a_scenario_here` and
  `every_surface_marker_fires_across_the_scenarios` are this suite's registry
  check and completeness proof over the `sls_` prefix.

## Phase 3 executed checks: strict cassette replay and redaction

Cassette core (`crates/eval-core/tests/cassette.rs`):

- `covered_field_lists_are_sorted_pinned_and_closed_over_the_observed_body`
  (`rid-opencode-provider-cassette-strict-miss`,
  `rid-cassette-strict-miss-typed-error`): both covered-field lists are pinned
  and sorted, the header allowlist is a subset of the covered headers, the raw
  body of a representative 1.18.31 request (plus the optional `temperature`)
  equals the covered body set exactly, the projection keeps only the two
  allowlisted headers, its digest is frozen so an encoding change is reviewed,
  and a body carrying an unlisted field is `UnknownRequestField`.
- `one_byte_in_any_covered_field_misses_at_the_turn_and_terminates_the_cassette`
  (`rid-opencode-provider-cassette-strict-miss`): one byte changed in each of
  the eleven OpenCode covered fields, after one consumed entry, is exactly one
  `CassetteMiss` naming turn 1, `ModelRequestChanged`, and the nearest recorded
  digest; the unchanged request is refused with the same terminal afterwards
  and `misses()` stays at one. `only_a_tool_result_change_is_tool_result_drift`
  names the other class. `a_fractional_temperature_digests_exactly` shows
  `0.7` persisted as `"0.7"`, `0.70` replaying, and `0.8` differing.
- `volatile_and_uncovered_changes_replay`: a moved `cache_control` marker and
  a changed or removed `x-api-key`, `user-agent`, `authorization`, or
  `x-session-id` header replay. `only_a_terminated_nonce_is_normalized` is the
  nonce table: `cch=<nonce>;` forms digest equal across nonces; an
  unterminated `cch=`, a URL query `cch=`, an empty nonce, and a changed tail
  all digest differently, so normalization never drops text.
- `equal_digests_replay_in_recorded_order_and_distinct_ones_in_any_order`:
  the concurrency case, with `unconsumed()` reaching zero and a miss past the
  recording naming the last entry.
  `the_nearest_entry_is_the_first_unconsumed_one_of_the_same_boundary`
  discriminates the nearest rule from `first` and `last`, shows a miss consumes
  nothing, and shows a backend lookup never answers from OpenCode entries.
- `namespaces_bind_the_cassette_and_equal_digests_elsewhere_refuse`
  (`rid-cassette-world-namespaced-no-cross-replay`): replay under another
  namespace is `NamespaceMismatch` before any request; a lookup or record under
  another namespace is `WrongNamespace`; recording into a replay is
  `RecordOnReplay` and a lookup on a recording is `LookupOnRecord`.
- `provenance_schema_and_version_pins_are_recomputed_on_read`
  (`rid-shared-cassette-schema-verified-provenance`): one edited frame or one
  edited declaration is `ProvenanceMismatch`; a re-signed declaration edit loads
  and is visible; a re-signed request edit is `EntryDigestMismatch`; a schema,
  generator, or covered-field version change and an unknown field each refuse.
- `planted_secrets_and_unscannable_frames_are_refused_and_the_cassette_never_persists`
  (`xc-captured-provider-requests-redacted-before-persistence`): after one
  admitted entry, an `sk-ant-` token in a user message, an AWS key in a tool
  result, and a secret in a response frame are `RedactionRefused(location,
  SecretDetected)`, and a body one byte past `MAX_REDACTABLE_BYTES` is
  `RedactionRefused(Request, InputLimit)`; the refused entry never exists and
  `to_file` returns the refusal, so the one admitted entry is not persisted
  either. `a_malformed_body_is_refused_rather_than_digested_as_empty` keeps
  `{}` out of the digest.
- `backend_records_cover_the_pinned_fields_with_exact_temperatures`
  (`rid-cassette-strict-miss-typed-error`): the `BackendRecord` projection's
  field set equals `BACKEND_COVERED_FIELDS`; `0.7` and `0.70` digest equal,
  `0.8` differs; `NaN`, infinity, `-0.0`, a negative value, and a hand-written
  `0.70` refuse. `every_error_names_its_wire_kind` pins sixteen distinct kinds,
  and `no_wire_detail_carries_request_content` pins `CassetteError::detail`:
  a request-shaped variant (`Shape`, `MalformedBody`, `UnknownRequestField`,
  `TemperatureNotDecimal`, `NotCanonical`) built around a canary payload
  renders an empty detail while its `Display` still carries the canary, and
  every oracle-owned variant renders its version, namespace, digest, index, or
  scanner outcome.

Manifest (`crates/eval-core/tests/manifest.rs`,
`sls-memory-reviewer-model-calls-cassette-or-excluded`): the
`memory_reviewer_model_calls` field is required (`MissingField` when absent),
enters the digest, and takes only `cassette` or `excluded`.

Daemon shell (`crates/daemon/tests/eval_cassette.rs`, `--all-features`):

- `replay_preserves_the_transcript_and_the_declarations`
  (`rid-rust-cassette-backend-impl-preserves-declarations`, marker
  `rid_capabilities_read_during_cassette_run`): two recorded requests, one a
  provider error with a retry hint, replay with equal events and terminals
  while the real backend is never called; `BackendDeclarations::new` reads the
  same declaration from the cassette as from the real backend for both
  harnesses; an edited header is `ProvenanceMismatch` until re-signed, and
  re-signed defaults latch differently.
- `one_byte_in_each_covered_field_misses_and_the_dropped_fields_do_not`
  (`rid-cassette-strict-miss-typed-error`, marker
  `rid_rust_cassette_miss_constructed`): after one consumed entry, each of the
  seven covered fields flipped is `Failed` with `provider_code: cassette_miss`,
  a message naming turn 1 and `ModelRequestChanged`, the typed terminal
  carrying the offered digest and the second entry as nearest, and no emitted
  event; the other recorded request is refused afterwards. `run_id`, `session`,
  and `0.7` written as `0.70` each replay alone.
- `a_regenerated_frame_or_another_namespace_refuses_before_any_request`
  (marker `rid_cassette_namespace_refused`): an edited recorded event is
  `ProvenanceMismatch`; another namespace is `NamespaceMismatch` at load; a
  `NaN` temperature is a `cassette_request` terminal and a canary prompt is a
  `redaction_refused` terminal that leaves the recorder with no file.
- `memory_reviewer_replays_through_the_keyed_peer`
  (`sls-memory-reviewer-model-calls-cassette-or-excluded`, marker
  `rid_reviewer_cassette_miss_reached`): the key recovered from one recorded
  request equals the production sender's `provider_identity()` and credential
  id, the request's model, and the SHA-256 of `MessagesRequest::body`'s bytes;
  the same body replays through `serve_keyed`; a changed body, model, or
  credential each miss with a 409 the sender reports as
  `SendError::Status(409)`, after which the recorded body is refused too; an
  entry keyed to another host misses.
- `every_cassette_marker_fires_across_the_scenarios` is the completeness
  proof over this suite's markers.
- `every_host_finish_reason_error_class_and_terminal_round_trips`: both
  `FinishReason` values, all four `ErrorClass` values under `Failed` and
  `FailedUnresolved`, and an absent event finish reason each record and replay
  as an equal transcript, so the wire mirror's decode side covers every host
  variant, not only the two the marker scenarios produce.

## Gaps recorded here

- The OpenCode cassette is bound to the environment that recorded it: the
  system prompt embeds the working directory, today's date, and the user's
  instruction files, so record and replay must share a checkout, a day, and a
  home. The e2e test records and replays on one OpenCode instance.
- OpenCode 1.18.31 emits no `cch=` billing nonce and no `anthropic-beta`
  header against the mock; both rules are pinned from the cache oracle and the
  parent specification rather than from an observed frame.
- Recording is against the mock's scripted responses; no recording against a
  live provider exists, so the redaction gate has been exercised on planted
  canaries only.
- The keyed reviewer peer holds its entries in memory; reviewer traffic is not
  yet persisted in the cassette file, and `memory_reviewer_model_calls` is a
  manifest declaration no runner enforces yet.
- The Rust oracle's stdin protocol has no Rust-side test; its consumer is the
  TypeScript MockProvider cassette mode. The `{kind, detail}` refusal contract
  is pinned in `eval-core` (`no_wire_detail_carries_request_content`), but the
  oracle's own variants (`UnsafePath`, `Io`, `Json`, `LineTooLong`,
  `AlreadyOpen`, `NoOpenCassette`) are exercised only through that consumer.

- Every ingestion entry point lacks a production caller. No world is labelled
  "validated real ingestion" until one exists; every manifest carries
  `adapter-ingested, production caller: none` (a required field), and the
  ingestion suite drives the adapters from tests only.
- The reducer takes the served class as a query input because generated
  worlds carry no admission events; the ingestion ticket decides what
  `SourcePublisher::publish` actually admits and pins the value.
- `MAX_EVENTS_PER_LOG` is a required `WorldConfig` field with no default, not
  a top-level manifest field; it reaches the manifest through
  `run_identity.config`. Promoting it to a named manifest field is a schema
  bump deferred to pre-run profile approval.
- The 800-unit total cap is not reachable through `render_user_hint` under the
  80-unit fragment cap and 3-result cap; the census exercises the cap function
  directly.
- Residue rules apply to top-level observation fields. Nested values under
  `Keep` enter the digest whole; per-tap observation schemas must flatten or
  register nested records.
- A refusal inside `admit_lanes` (a deadline, a cancellation, or `no_lane`)
  returns no `Admitted`, so the lane and eligibility stages of that request
  are unobserved; the ledger reads them as `NotReached` and the fold is
  `Indeterminate` for any required occurrence that entered through them.
- The lexical and dense lanes judge eligibility inside retrieval and return
  only their eligible rankings, so an occurrence those lanes scanned and the
  kernel excluded is absent at the lane stage, not at eligibility. Only the
  exact lane's admission is attributable to eligibility on its own.
- The lexical lane's `Retrieval.incarnation`, the dense producer's, and the
  packer's kernel judgement are not returned by the route or the packer, so
  their observations carry no incarnation token; the fold's incarnation guard
  covers the exact admission and revalidation reports.
- A `select` refusal discards the revalidation report, so every refusal after
  the union bound, including `response_bytes` and `response_measure`, folds
  as `Unjoinable` at fusion from the refusal reason rather than from the
  report itself; a loss at revalidation or at the response cap cannot be
  placed, and a revalidation incarnation the refusal dropped cannot be
  counted (`a_response_refusal_discards_revalidation_so_fusion_is_unjoinable`).
  The shell matches the reason `fused_union` as a wire literal.
- Cross-restart fold comparison is shown against the persisted
  `database_incarnation_id` of one store; no daemon restart runs in the
  suite.
- The query route and the packer have no production caller. Every result in
  `eval_ledger.rs` is activated-component evidence labelled `test-only`; a
  ledger verdict there is not a statement about shipped retrieval or packing.
- The render stage is observed as the capped selection: under the pinned
  80-unit fragment cap and 3-result cap the 800-unit total cap cannot drop a
  selected segment, and `render_user_hint` keeps no per-segment record. Its
  one per-segment drop, a fragment that compresses to under three bytes, is
  not observed; a segment body starts with its title, so no seeded segment
  reaches it.
- The host records one pass per process, the newest native-serving one of any
  session; the shell asserts the recorded `block_id` names its own tail.
  `DurableState::Held` is asserted by the shell from what it seeded, not read
  back from the store.
- The surface-1 suite hand-builds one history segment per rendered message
  with a chosen summary phrase; the segment's native identity is real, its
  text is not summarizer output. Replay-built segments arrive with the aged
  worlds.
- Surface-1 observations carry no incarnation token: the hint scorer reads
  the memory store, which has no `CommitReadIncarnation`.
- The deferral stage is observed but not injected; a deferred pass needs a
  block the host already served, which the five injections do not construct.
  The suppression and token gates are observed as passed in every scenario and
  not injected either.
- `native_carries_user_hint` filters native messages by the block's `mid`; no
  scenario plants the hint text on another message, so the filter is read, not
  exercised.
- The transform's wire response is unchanged by the tap (`user_hint` is a
  `#[serde(skip)]` field); the aged and differential goldens in the daemon
  library tests pin that output, and the surface suite asserts the key is
  absent from the response.
