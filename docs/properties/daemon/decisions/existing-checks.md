# Part 4f existing-check inventory

Source revision: `74044960ee91641dec95c8552f15282844a18b13`. Config checks and
the budget-reader integration check below are inventoried against that source.
The non-config sections from [Integration tests](#integration-tests) onward are
verified against the working tree at `c871eeb3`. Every existing-check and guard
status is `unaudited`: presence, source inspection, and execution do not
establish oracle adequacy.

## File-local census

These counts enumerate `#[test]` and `#[tokio::test]` functions in the listed
files. Locations are the first and last test function declarations, not coverage
measurements. Paths are relative to `crates/daemon/src/`.

| File | Tests | First and last test declarations | Status of each check |
| --- | --- | --- | --- |
| `selection.rs` | 37 | `selection.rs:1462`, `selection.rs:3221` | unaudited |
| `boundary.rs` | 29 | `boundary.rs:1961`, `boundary.rs:2813` | unaudited |
| `config.rs` | 43 | `config.rs:1089`, `config.rs:2185` | unaudited |
| `codec/opencode.rs` | 17 | `codec/opencode.rs:1368`, `codec/opencode.rs:2104` | unaudited |
| `scheduler.rs` | 16 | `scheduler.rs:1011`, `scheduler.rs:1396` | unaudited |
| `codec/pi.rs` | 14 | `codec/pi.rs:1050`, `codec/pi.rs:1457` | unaudited |
| `codec/mod.rs` | 6 | `codec/mod.rs:59`, `codec/mod.rs:296` | unaudited |
| `wire.rs` | 11 | `wire.rs:839`, `wire.rs:1545` | unaudited |
| `caveman.rs` | 3 | `caveman.rs:883`, `caveman.rs:937` | unaudited |
| `session_resolver.rs` | 1 | `session_resolver.rs:61` | unaudited |
| `codec/sidecar.rs` | 3 | `codec/sidecar.rs:488`, `codec/sidecar.rs:520` | unaudited |

Total: 180 file-local tests. This excludes tests in `lib.rs`, `transform.rs`, and
integration binaries; it must not be compared with a transitive reach count as
though the populations were identical. The old 192-test attribution is not a
current census. In particular, `codec/sidecar.rs` does not have zero tests.

## Config checks

Every location in this table names `crates/daemon/src/config.rs`. There are five
tests in `cache_ttl_tests` and 38 in `tests`. Each row names an existing check,
not a new property or an adequacy verdict.

| Check | Function declaration | Status |
| --- | --- | --- |
| `per_model_cache_ttl_object_shape_parses_and_resolves` | `config.rs:1089` | unaudited |
| `provenance_distinguishes_an_explicit_value_equal_to_the_default` | `config.rs:1118` | unaudited |
| `cache_ttl_resolution_matches_shared_typescript_vectors` | `config.rs:1133` | unaudited |
| `string_cache_ttl_shape_still_parses` | `config.rs:1163` | unaudited |
| `project_tier_cannot_set_cache_ttl` | `config.rs:1174` | unaudited |
| `an_oversized_tier_file_is_ignored_with_a_warning` | `config.rs:1190` | unaudited |
| `user_config_path_prefers_xdg_config_home_over_home` | `config.rs:1239` | unaudited |
| `tier_policy_ignores_project_models_and_rejects_project_lowering` | `config.rs:1279` | unaudited |
| `project_threshold_may_only_raise` | `config.rs:1297` | unaudited |
| `default_threshold_matches_typescript_schema` | `config.rs:1305` | unaudited |
| `default_memory_budget_matches_typescript_schema` | `config.rs:1311` | unaudited |
| `memory_injection_budget_uses_standard_key_and_deprecated_user_fallback` | `config.rs:1317` | unaudited |
| `rust_only_budget_leaves_are_user_tier_only_and_warn_when_project_supplies_them` | `config.rs:1353` | unaudited |
| `compaction_enabled_defaults_true_and_is_user_tier_only` | `config.rs:1390` | unaudited |
| `auto_search_and_caveman_config_follow_user_then_project_tiers` | `config.rs:1407` | unaudited |
| `historian_budget_derivation_clamps_at_both_bounds` | `config.rs:1449` | unaudited |
| `docs_injection_is_user_tier_only_and_temporal_flag_follows_project_tier` | `config.rs:1458` | unaudited |
| `guidance_override_accepts_resolved_user_text_and_ignores_project_injection` | `config.rs:1483` | unaudited |
| `inline_guidance_override_requires_exactly_one_marker_like_the_file_form` | `config.rs:1511` | unaudited |
| `guidance_override_path_resolves_relative_to_user_config_directory` | `config.rs:1532` | unaudited |
| `guidance_override_invalid_and_missing_files_warn_and_fall_back` | `config.rs:1558` | unaudited |
| `guidance_marker_validation_matches_the_typescript_line_rule` | `config.rs:1603` | unaudited |
| `historian_gates_follow_tiers_but_context_limit_remains_user_tier_only` | `config.rs:1611` | unaudited |
| `project_tier_cannot_raise_the_user_memory_gate` | `config.rs:1637` | unaudited |
| `privileged_keys_are_the_model_budget_and_schedule_levers` | `config.rs:1671` | unaudited |
| `hostile_project_tier_cannot_change_privileged_values_and_warns_per_key` | `config.rs:1705` | unaudited |
| `every_consumed_pointer_is_a_classified_config_key` | `config.rs:1820` | unaudited |
| `sibling_keys_keep_their_precedence_within_a_tier` | `config.rs:1860` | unaudited |
| `module_model_replaces_plugin_chain_entirely` | `config.rs:1935` | unaudited |
| `module_model_absent_falls_back_to_plugin_keys` | `config.rs:1952` | unaudited |
| `module_model_blank_is_treated_as_absent` | `config.rs:1968` | unaudited |
| `module_model_is_user_tier_only` | `config.rs:1980` | unaudited |
| `jsonc_strip_preserves_comment_like_strings` | `config.rs:2001` | unaudited |
| `jsonc_strip_keeps_tokens_around_block_comments_separate` | `config.rs:2012` | unaudited |
| `jsonc_strip_rejects_unterminated_block_comment` | `config.rs:2021` | unaudited |
| `jsonc_strip_removes_a_leading_byte_order_mark` | `config.rs:2026` | unaudited |
| `jsonc_strip_ends_line_comments_at_carriage_return` | `config.rs:2033` | unaudited |
| `jsonc_strip_keeps_a_comma_that_directly_follows_an_opener` | `config.rs:2042` | unaudited |
| `project_tier_is_read_from_the_eidnara_config_path` | `config.rs:2061` | unaudited |
| `built_in_guidance_assets_carry_one_eidnara_marker_each` | `config.rs:2080` | unaudited |
| `mtime_cache_reuses_unchanged_reads_and_invalidates_on_mtime_change` | `config.rs:2093` | unaudited |
| `unreadable_and_malformed_tiers_warn_while_missing_tiers_stay_silent` | `config.rs:2133` | unaudited |
| `model_chain_drops_repeats_anywhere_and_keeps_first_occurrence_order` | `config.rs:2185` | unaudited |

### Assertion scope

The privilege check (`config.rs:1671-1700`) pins twelve privileged key names,
including all four budget spellings, and rejects `ProjectAllowed` for those
names. The hostile fixture (`config.rs:1705-1815`) supplies all 25 keys and
asserts selected values, including all three effective budgets. Its warning
count derives from `tier_class` (`config.rs:1786-1796`), so it is not an
independent oracle for every permission in the
[tier-policy record](catalog.md#dec-a-project-tier-can-write-leaves-outside-the-documented-allow-list).

The pointer inventory (`config.rs:1820-1855`) checks registration, uniqueness,
and literal-pointer usage. It does not rule out every alternate way to read a
JSON value. The sibling-precedence test (`config.rs:1860-1932`) covers schedule
versus legacy flag, standard versus legacy budget, malformed primary fallback,
and project rejection.

The malformed-tier test (`config.rs:2133-2182`) checks fallback, path-bearing
read and parse warnings, repeated warnings, same-mtime repair, and silence for
missing tiers. The model-chain test (`config.rs:2185-2194`) supplies primary
`a` and fallbacks `[b, a, c, b]` and asserts `[a, b, c]`. These checks belong
to retained regression contracts, not open silent-read or adjacent-only defects.

No dedicated clamp-reporting check is identified. Ordinary auto-search and
caveman overrides (`config.rs:1407-1446`) do not establish range diagnostics.
The upper threshold test (`config.rs:1297-1302`) asserts a value, not a warning.
These are inventory limits, not authorization to change runtime policy.

### Companion scheduler checks

These checks exercise config-derived behavior outside `config.rs`. They are
included in the scheduler's 16-test file-local count, not additional tests.

| Check | Location | Assertion scope | Status |
| --- | --- | --- | --- |
| `scheduler_golden_matches_production_behaviour` | `scheduler.rs:1011-1013`, `scheduler.rs:1082-1113` | Loads frozen parser, threshold, and scheduling cases, including deserialized percentage maps; does not compare the config and scheduler walks on the same wildcard-only input | unaudited |
| `parse_cache_ttl_never_returns_u64_max` | `scheduler.rs:1378-1385` | Pins case-insensitive `never`, one ordinary TTL, and malformed input | unaudited |
| `never_ttl_predicates_are_always_false` | `scheduler.rs:1388-1393` | Pins both expiry predicates at the maximum sentinel | unaudited |
| `never_ttl_scheduler_stays_deferred` | `scheduler.rs:1396-1405` | Pins one no-expiry scheduler outcome | unaudited |

The percentage-map cases are constructed by deserialization from
`crates/daemon/testdata/scheduler-golden.json:87-106`; searching only for Rust
constructor expressions misses them. The config TTL check at
`config.rs:1133-1160` consumes five retained routing vectors. A search of tracked
package source finds no literal reference to `cache-ttl-routing-vectors.json`;
the historical test name alone does not establish a live TypeScript comparison.

## Config-reader integration check

| Check | Location | Assertion scope | Status |
| --- | --- | --- | --- |
| `rows_past_the_configured_budget_are_dropped_by_the_reader` | `crates/daemon/tests/transform_canonical_memory.rs:448-539` | Loads a nondefault 500-token user config through a daemon fixture, excludes an oversized memory row, includes a short row, and retains the composition revision after editing only the excluded row | unaudited |

The parent invocation writes `memory.injection_budget_tokens: 500` into an
isolated user config directory, then re-executes the exact test with
`XDG_CONFIG_HOME` set for the child
(`crates/daemon/tests/transform_canonical_memory.rs:448-477`). The child starts
`KernelDaemon`, commits the two memory rows, and asserts their rendered inclusion
and exclusion (`crates/daemon/tests/transform_canonical_memory.rs:478-512`).
After superseding the excluded row, it asserts no `HARD` action and an unchanged
revision (`crates/daemon/tests/transform_canonical_memory.rs:514-538`).

This is config-reader integration coverage, not merely a `DaemonConfig` literal
or an in-memory merge. It does not test every tier permission or an out-of-range
budget. This bounded inventory does not claim a complete census of every
integration test that transitively reaches a decision unit.

## Production config guards

| Guard | Location | Contract | Status |
| --- | --- | --- | --- |
| Privileged-key assertion | `config.rs:696-708` | At compile time, every registered privileged key must not be `ProjectAllowed`; this is not a complete per-key policy oracle | unaudited |
| Classified project merge | `config.rs:723-747` | Supplied user-only keys warn and are ignored; changed weakening candidates warn; no-op candidates do not emit tier warnings | unaudited |
| File-read diagnostics | `config.rs:362-393`, `config.rs:262-282` | Read and parse failures retain a warning; `NotFound` does not; resolution collects both tier warnings | unaudited |
| Numeric clamps and zero rejection | `config.rs:750-752`, `config.rs:827-869`, `config.rs:955-961` | Clamps and zero rejection do not emit range warnings; legacy-key deprecation is a different diagnostic | unaudited |
| Full model-chain deduplication | `config.rs:950-953` | Keeps only the first occurrence of each model without sorting | unaudited |

## CI execution

The workspace nextest job selects all targets and features and partitions tests
across two shards (`.github/workflows/ci.yml:384-417`). Its default filter excludes
bench targets, not daemon library or integration tests (`.config/nextest.toml:4-7`).
The statement that no config check runs in CI is false.

For pull requests, Rust test execution is path-filtered. The `test` filter includes
Rust inputs, nextest configuration, release data, and the host wire document
(`.github/workflows/ci.yml:70-81`). A property-doc-only change need not rerun the
full integration suite. This is selection behavior, not a waiver of checks for
Rust changes.

## Integration tests

`crates/daemon/tests/` holds 17 integration binaries. Each was scanned for the 4f
module paths (`selection::`, `boundary::`, `scheduler::`, `codec::`,
`caveman::`, `config::`, `wire::`, `session_resolver::`) and for the named codec
and wire items. Four reach a 4f unit directly; the rest are name false positives
or belong to other parts. All 17 are selected by the workspace nextest job
described under [CI execution](#ci-execution). Locations in this section are
relative to `crates/daemon/tests/`.

| Binary | Tests | 4f reach | Assertion scope | Status |
| --- | --- | --- | --- | --- |
| `caveman_differential.rs` | 3 | `use daemon::caveman::{CavemanLevel, compress}` (`caveman_differential.rs:865`) | `optimized_matches_frozen_reference` (`caveman_differential.rs:1004`) runs 512 proptest documents (`caveman_differential.rs:1001-1013`) and `optimized_matches_frozen_reference_on_large_documents` (`caveman_differential.rs:1042`) runs 24 documents over 32 KiB (`caveman_differential.rs:1039-1051`) against a frozen in-file copy of `caveman.rs` (`mod reference`, `caveman_differential.rs:8`); `targeted_case_and_placeholder_edges_match_reference` (`caveman_differential.rs:1016`) pins five hand-picked inputs. The oracle is a frozen copy of the implementation, so it detects drift from that copy, not incorrectness | unaudited |
| `caveman_reference.rs` | 3 | `use daemon::caveman::{CavemanLevel, compress}` (`caveman_reference.rs:681`) | `reference_matches_golden_fixture` (`caveman_reference.rs:699`) replays the 42-case `caveman-golden.json` (`caveman_reference.rs:701`) through a naive position-by-position port of the TypeScript compressor (`mod reference`, `caveman_reference.rs:15`); `production_matches_reference_on_fuzz_corpus` (`caveman_reference.rs:944`) compares 400 seeded documents; `production_matches_reference_on_tiled_golden` (`caveman_reference.rs:969`) tiles `caveman-golden.json` (`caveman_reference.rs:971`) past 32 KiB and compares once | unaudited |
| `selection_differential.rs` | 18 | `use daemon::selection::{PassClass, ..., select_reductions_with_outcome}` (`selection_differential.rs:1442-1445`) | Eight `proptest!` blocks (`selection_differential.rs:2162`, `:2174`, `:2430`, `:2442`, `:2454`, `:2466`, `:2478`, `:2490`) and eight hand-built cases compare `select_reductions_with_outcome` against a frozen in-file copy of `selection.rs` (`mod reference`, `selection_differential.rs:8`); `generators_reach_every_decision_class` (`selection_differential.rs:2585`) and `every_production_variant_is_generated` (`selection_differential.rs:2650`) check the generators, not the selector | unaudited |
| `transform_canonical_memory.rs` | 5 | `memory.injection_budget_tokens` written to a user config (`transform_canonical_memory.rs:456`) | One test reaches the config reader; see [Config-reader integration check](#config-reader-integration-check) | unaudited |

| Binary | 4f hits | Verdict |
| --- | --- | --- |
| `direct_host.rs`, `transform_meta_bound.rs`, `transform_canonical_memory.rs` | JSON string `"render_config"` (`direct_host.rs:96`, `direct_host.rs:162`, `transform_meta_bound.rs:32`, `transform_canonical_memory.rs:127`) | A config-identity field on the request, not `config.rs`. Out of scope |
| `boundary_counter_durability.rs` | 0 | Name false positive. Its one test, `competing_pass_counter_survives_direct_primary_lifecycle_and_reopen` (`boundary_counter_durability.rs:12`), exercises `boundary_divergence_pending_count` across a store reopen (`boundary_counter_durability.rs:20`, `boundary_counter_durability.rs:30`). It never touches `boundary.rs` |
| `broca_roundtrip.rs` | 0 | Name false positive. `real_broca_success_block_release_failure_and_counters` (`broca_roundtrip.rs:60`) and `real_broca_cancel_shutdown_and_full_route_handle_cleanup` (`broca_roundtrip.rs:146`) drive a `host_runtime` request-and-stream round trip, not a codec round trip |
| `release_contract_conformance.rs` | 0 | Five tests, `credential_constants_match_the_release_contract` (`release_contract_conformance.rs:30`), `provider_credential_matrix_matches_the_published_doc` (`release_contract_conformance.rs:56`), `rust_canonical_encoding_reproduces_every_qualified_closure_digest` (`release_contract_conformance.rs:150`), `lock_platform_blocks_match_the_release_contract` (`release_contract_conformance.rs:230`), and `published_closure_manifest_schema_matches_the_runtime_types` (`release_contract_conformance.rs:284`), over `host_runtime` credential and closure-digest constants and `daemon::release_contract` (`release_contract_conformance.rs:19-32`). It runs under the workspace job and reaches no 4f file |
| `base64_differential.rs`, `historian_truncate_differential.rs`, `host_adapter.rs`, `kernel_routes.rs`, `lifecycle_cli.rs`, `prepared_output.rs`, `stage1_eligibility.rs`, `stage1_routes.rs` | 0 | Other parts |

Only `caveman::compress`, `selection::select_reductions_with_outcome`, and the
config reader have integration coverage. `boundary.rs`, `scheduler.rs`, the codec
files, `wire.rs`, and `session_resolver.rs` are reached only from their own
`mod tests` and from `lib.rs` and `transform.rs` unit tests.

## TypeScript-side gates

The TypeScript suites run on pull requests that touch `packages/**` through
`bun run check:repo` (`.github/workflows/ci.yml:281-283`), which root
`package.json` defines as typecheck, lint, test, and build. The `test` script
runs `bun test` in each package, including `packages/opencode-plugin` and
`packages/pi-plugin`. None of those suites reads a 4f fixture or executes 4f Rust
code. Every 4f fixture is one-legged and replayed by Rust only. Fixture paths are
relative to `crates/daemon/testdata/`.

| Fixture | Rust consumer | TypeScript consumer | Tracked generator |
| --- | --- | --- | --- |
| `cache-ttl-routing-vectors.json` (5 cases) | `config.rs:1135` | none in tracked package source | none |
| `scheduler-golden.json` | `scheduler.rs:1012` | none | none |
| `selection-golden.json` (15 cases) | `selection.rs:1617` | none | none |
| `boundary-golden.json` | `boundary.rs:1946` | none | none |
| `caveman-golden.json` (42 cases) | `caveman.rs:903`; also `crates/daemon/tests/caveman_reference.rs:701`, `:971` and `crates/daemon/benches/kernels.rs:30` | none | none; `caveman.rs:3-8` names the TypeScript compressor as the origin |
| `codec/opencode-golden.json` (1 case) | `codec/mod.rs:61` | none | `testdata/codec/gen-opencode-golden.ts` |
| `codec/pi-golden.json` (1 case) | `codec/mod.rs:186` | none | `testdata/codec/gen-pi-golden.ts` |
| `codec/serve-native-golden.json` | `codec/mod.rs:107-109` | none | none |

The parallel TypeScript implementations are tested against their own
expectations, not against Rust. `resolveCacheTtl` is tested at
`packages/opencode-plugin/src/hooks/context/event-resolvers.test.ts:82-116`
without reading `cache-ttl-routing-vectors.json`. `encodeOpenCodeMessagesToCk` is
imported at `packages/opencode-plugin/src/hooks/context/module-wire.test.ts:9` and
tested from `packages/opencode-plugin/src/hooks/context/module-wire.test.ts:17`;
it does not invoke `codec/opencode.rs`. The TypeScript `checkCompartmentTrigger`
and `resolveProtectedTailBoundary` functions that `boundary.rs` was ported from
are not present in tracked package source. `boundary_constants_match_ts_sources`
(`boundary.rs:1961`) and the three goldens `chunk_golden_matches_ts_formatting`
(`boundary.rs:2034`), `boundary_golden_matches_ts_resolution`
(`boundary.rs:2072`), and `trigger_golden_matches_ts_decision_core`
(`boundary.rs:2116`) are therefore the only cross-implementation evidence for
that unit.

No 4f fixture has a provenance guard, and no workflow or package script
regenerates any of them. 4e's `tail_hygiene.rs:1109-1113` asserts
`generator_version` and `input_sha256`, with the mutation test
`provenance_guard_rejects_mutated_fixture_input` at `tail_hygiene.rs:1174`. No
4f file matches `input_sha256` or `generator_version`; the `provenance` matches
in 4f are production types about the origin of a value (`config.rs:136`
`CacheTtlProvenance`, `scheduler.rs:289` `ContextLimitProvenance`), not of a
fixture. Both codec goldens carry `generated_from` and `projection_oracle`
top-level fields and neither is deserialized: `OpenCodeGolden`
(`codec/mod.rs:33-38`) and `PiGolden` (`codec/mod.rs:46-51`) declare only
`coverage`, `missing_capture_classes`, and
`cases`. The one fixture-regeneration script in the repository,
`mutation:goldens` (`packages/e2e-tests/package.json:24`), targets
`differential-golden.json`
(`packages/e2e-tests/scripts/run-goldens-mutation.ts:21`), which is not a 4f
fixture.

One documented Rust-versus-TypeScript divergence is intentional and has no
cross-language check. `config.rs:11` states the module uses stricter
model-selection policy than the TypeScript implementation. Seven doc comments
assert TypeScript agreement (`config.rs:20`, `:22`, `:24`, `:30`, `:33`, `:36`,
`:42`), and two tests check named schema values (`config.rs:1305`,
`config.rs:1311`; see [Config checks](#config-checks)). No TypeScript test
parses `config.rs`, and the divergence itself is asserted by nothing on either
side.

## In-crate tests, clustered with line ranges

Counted directly at HEAD; the per-file totals equal the
[census](#file-local-census). Four test-only regions sit above the main test
module and would derail a mechanical recount keyed on a file's first
`#[cfg(test)]`: `config.rs:245` (indented, inside an `impl`) and
`config.rs:395` precede its modules at `config.rs:1083-1084` and
`config.rs:1182-1183`; `boundary.rs:701` and `boundary.rs:970` (indented)
precede its module at `boundary.rs:1760-1761`. Each row names existing checks,
not adequacy verdicts; every check is `unaudited`.

| File | Test module | Tests | Line range of test `fn`s | What they cover |
| --- | --- | --- | --- | --- |
| `selection.rs` | `mod tests` at `selection.rs:1304` | 37 | `selection.rs:1462`-`selection.rs:3221` | `natural_bust_drains_a_single_command_remainder` (`selection.rs:1462`); `selection_golden_matches_ts_selectors` (`selection.rs:1616`), the TypeScript selector golden; age reclaim, force band, emergency, and supersession batching from `age_reclaim_requires_both_scheduler_pressure_and_a_ride` (`selection.rs:1740`) through `supersession_floor_tracks_active_tag_window_on_thirty_arc_fixture` (`selection.rs:2376`); `edit_marker_region_hint_caps_utf16_and_backs_off_split_surrogate` (`selection.rs:2424`); the `provider_executed_arc_never_targeted` (`selection.rs:2439`), `frozen_arc_blocks_never_re_emitted` (`selection.rs:2469`), and `dynamic_block_protection_filters_automatic_and_agent_drop_decisions` (`selection.rs:2493`) filters; reasoning eligibility, cross-turn call ids, and the skeleton window from `reasoning_only_assistant_makes_the_whole_tool_arc_ineligible` (`selection.rs:2517`) through `skeleton_window_keeps_call_shell_older_full_drops` (`selection.rs:2689`); `drop_wins_over_edit_marker` (`selection.rs:2718`); `payload_purity_independent_of_pressure` (`selection.rs:2764`); agent drops, ride opportunities, and carriers from `agent_drop_ids_reduce_directly` (`selection.rs:2872`) through `pass_through_carriers_are_never_reduction_targets` (`selection.rs:2953`); the duplicate-safe-tool family from `duplicate_safe_tools_keep_the_newest_same_owner_full_drop` (`selection.rs:2975`) through `duplicate_full_drop_skeletonizes_for_reasoning_adjacency` (`selection.rs:3172`); `defer_pass_produces_nothing` (`selection.rs:3221`) |
| `boundary.rs` | `mod tests` at `boundary.rs:1761` | 29 | `boundary.rs:1961`-`boundary.rs:2813` | `boundary_constants_match_ts_sources` (`boundary.rs:1961`); the three goldens `chunk_golden_matches_ts_formatting` (`boundary.rs:2034`), `boundary_golden_matches_ts_resolution` (`boundary.rs:2072`), and `trigger_golden_matches_ts_decision_core` (`boundary.rs:2116`); the backward reasoning fence, completed-arc rule, and fold-only guard from `reasoning_bearing_completed_arc_fences_a_straddling_boundary_backward` (`boundary.rs:2260`) through `fold_only_guard_folds_large_head_before_deep_newest_arc` (`boundary.rs:2553`); wrap-up watermarks from `wrapup_counts_every_role_and_keeps_the_newest_messages` (`boundary.rs:2585`) through `wrapup_user_snap_window_scales_with_session_geometry` (`boundary.rs:2653`); `boundary_determinism_same_tail_same_resolution` (`boundary.rs:2680`) and `adding_newer_items_never_moves_protected_start_below_anchor` (`boundary.rs:2693`); `open_arc_staleness_flips_when_newer_growth_pushes_it_older_than_size_walk` (`boundary.rs:2708`); trigger, chunk saturation, ordinal-zero, and original-bytes edges from `trigger_never_consumes_the_protected_tail` (`boundary.rs:2735`) through `boundary_measures_original_bytes_not_rendered_reduction_placeholders` (`boundary.rs:2813`) |
| `config.rs` | `mod cache_ttl_tests` at `config.rs:1084`; `mod tests` at `config.rs:1183` | 43 (5 + 38) | `config.rs:1089`-`config.rs:2185` | From `per_model_cache_ttl_object_shape_parses_and_resolves` (`config.rs:1089`) through `model_chain_drops_repeats_anywhere_and_keeps_first_occurrence_order` (`config.rs:2185`), enumerated per check under [Config checks](#config-checks) and [Assertion scope](#assertion-scope) |
| `codec/opencode.rs` | `mod tests` at `codec/opencode.rs:1277` | 17 | `codec/opencode.rs:1368`-`codec/opencode.rs:2104` | Fresh-part completeness, adjacency deletion, native-extras survival, polarity round trip, native time, wire reachability, and reasoning exemptions from `every_fresh_tool_part_from_folded_transform_fixture_has_complete_state` (`codec/opencode.rs:1368`) through `hard_epoch_fold_with_head_todo_coalesces_unmatched_reduced_tool_arc` (`codec/opencode.rs:1929`), including `compaction_is_extracted_as_boundary_not_content` (`codec/opencode.rs:1809`); `whole_array_tool_use_guard_rejects_both_id_collision_directions` (`codec/opencode.rs:2028`), debug-only behind `#[cfg(debug_assertions)]` (`codec/opencode.rs:2026`); `incremental_sidecar_carries_pins_across_three_generations` (`codec/opencode.rs:2062`); `text_before_latest_reasoning_uses_typed_wire_projection` (`codec/opencode.rs:2104`) |
| `scheduler.rs` | `mod tests` at `scheduler.rs:880` | 16 | `scheduler.rs:1011`-`scheduler.rs:1396` | `scheduler_golden_matches_production_behaviour` (`scheduler.rs:1011`); band geometry and the durable-overflow arm from `band_boundaries_are_non_vacuous` (`scheduler.rs:1167`) through `durable_overflow_arm_upgrades_only_a_would_be_defer_to_emergency` (`scheduler.rs:1218`); deferral, latch lifecycle, determinism, hard idle TTL, and vocabulary mapping from `boundary_deferral_records_retries_and_respects_bypasses` (`scheduler.rs:1231`) through `pass_decision_maps_to_selection_vocabulary` (`scheduler.rs:1358`); the never-TTL family from `parse_cache_ttl_never_returns_u64_max` (`scheduler.rs:1378`) through `never_ttl_scheduler_stays_deferred` (`scheduler.rs:1396`), see [Companion scheduler checks](#companion-scheduler-checks) |
| `codec/pi.rs` | `mod tests` at `codec/pi.rs:1046` | 14 | `codec/pi.rs:1050`-`codec/pi.rs:1457` | `response_id_arriving_later_does_not_replace_pinned_timestamp_mid` (`codec/pi.rs:1050`); `split_pipe_tool_ids_decode_to_canonical_id_and_round_trip` (`codec/pi.rs:1088`); adjacency deletion and survivor extras from `adjacent_tool_deletion_matches_the_surviving_native_id` (`codec/pi.rs:1120`) through `mutated_text_survivor_keeps_its_own_signature_and_vendor_extras` (`codec/pi.rs:1198`); multi-part, image, opaque, and empty-error tool results from `untouched_multi_text_tool_result_replays_raw_part_boundaries_and_extras` (`codec/pi.rs:1244`) through `empty_error_tool_result_retains_empty_content_and_error_polarity` (`codec/pi.rs:1373`); frozen and untouched replay in `frozen_deletion_replay_is_byte_stable` (`codec/pi.rs:1397`), `untouched_message_replays_the_exact_retained_raw_value` (`codec/pi.rs:1417`), and `deleted_tool_result_does_not_replay_the_retained_raw_entry` (`codec/pi.rs:1440`); `compaction_entry_is_boundary_signal` (`codec/pi.rs:1457`) |
| `codec/mod.rs` | `mod tests` at `codec/mod.rs:18` | 6 | `codec/mod.rs:59`-`codec/mod.rs:296` | The two harness round-trip goldens `opencode_golden_round_trips_wire_projected_parts_and_is_deterministic` (`codec/mod.rs:59`) and `pi_golden_round_trips_non_compaction_entries_and_is_deterministic` (`codec/mod.rs:184`); `serve_native_golden_preserves_ingress_and_pins_synthetic_shapes` (`codec/mod.rs:97`); `fresh_boundary_prefix_does_not_borrow_persisted_synthetic_meta` (`codec/mod.rs:133`); `codec_conformance_removes_leading_native_blocks_without_reindex_drift` (`codec/mod.rs:222`); `fixture_builder_drives_synthetic_todo_wire_shape` (`codec/mod.rs:296`) |
| `wire.rs` | `mod tests` at `wire.rs:797` | 11 | `wire.rs:839`-`wire.rs:1545` | `projection_retained_bytes_counts_wire_and_frontier_allocations_once` (`wire.rs:839`); arc identity in `repeated_call_id_within_owner_message_shares_one_arc_identity` (`wire.rs:1082`) and `reasoning_joins_the_arc_its_adjacent_call_was_assigned` (`wire.rs:1116`); the user-carried tool-result accept and reject cases `user_carried_tool_result_pairs_with_prior_assistant_call` (`wire.rs:1197`) and `user_carried_tool_result_without_prior_call_still_rejects` (`wire.rs:1262`); `opaque_and_media_inside_tool_result_content_are_accepted_and_projected` (`wire.rs:1287`); `incremental_projection_reuses_prefix_storage_and_preserves_tool_arc_state` (`wire.rs:1360`); `empty_and_reserved_message_ids_are_rejected` (`wire.rs:1415`) and `duplicate_message_ids_are_rejected_across_the_incremental_prefix` (`wire.rs:1432`); `reduced_tool_result_keeps_failure_variant_and_output_extras` (`wire.rs:1461`); `reattach_keeps_block_level_original_but_rebuilds_the_message_shell` (`wire.rs:1545`) |
| `caveman.rs` | `mod tests` at `caveman.rs:876` | 3 | `caveman.rs:883`-`caveman.rs:937` | `literal_placeholder_text_survives_compression_unchanged` (`caveman.rs:883`); `differential_golden_matches_typescript_oracle` (`caveman.rs:901`, extent `caveman.rs:901-929`), the 42-case golden; `pattern_set_invariants` (`caveman.rs:937`) for the automaton. Shared with 4e |
| `session_resolver.rs` | `mod tests` at `session_resolver.rs:57` | 1 | `session_resolver.rs:61` `unsupported_mapping_is_local_absence`, with `#[tokio::test]` at `session_resolver.rs:60` | The absence case only. The supported-mapping path has no test |
| `codec/sidecar.rs` | `mod tests` at `codec/sidecar.rs:465` | 3 | `codec/sidecar.rs:488`-`codec/sidecar.rs:520` (extent `codec/sidecar.rs:487-557`) | Alignment pairing only: `oversized_alignment_takes_the_linear_memory_path_and_keeps_positional_pairs` (`codec/sidecar.rs:488-506`) drives `match_block_metas` past `MAX_ALIGNMENT_CELLS`; `greedy_alignment_never_reuses_a_meta_and_preserves_order` (`codec/sidecar.rs:509-517`) and `greedy_alignment_pairs_fingerprinted_metas_through_the_fingerprint_index` (`codec/sidecar.rs:520-557`) drive the private `greedy_block_metas`. See quiet area 1 for what remains untested |

### `#[ignore]`, `should_panic`, and property tooling

`#[ignore]`: none found in any 4f file, matching both `#[ignore]` and
`#[ignore = "..."]`. `transform.rs` carries three ignored manual timing tests,
`apply_once_stage_timings_large_fixture` (`transform.rs:12375-12376`),
`tag_baseline_warm_hydration_50k` (`transform.rs:22539-22540`), and
`full_module_pass_timing_fixture` (`transform.rs:27392-27393`). `lib.rs` carries
one, `historian_trigger_token_reuse_benchmark` (`lib.rs:16761-16762`), which
calls `boundary::check_compartment_trigger_retokenized_reference`.

`should_panic`: none found in any 4f file. One test has a panic oracle written a
way a `should_panic` search misses:
`whole_array_tool_use_guard_rejects_both_id_collision_directions`
(`codec/opencode.rs:2028`) uses `std::panic::catch_unwind` at
`codec/opencode.rs:2055` and asserts `.is_err()`. It is the only `catch_unwind`
in 4f, and it carries `#[cfg(debug_assertions)]` at `codec/opencode.rs:2026`,
which the guards section turns on.

Property tooling: `proptest` is a `daemon` dev-dependency and drives two of the
integration binaries above (`caveman_differential.rs` and
`selection_differential.rs`). No 4f file under `crates/daemon/src/` uses
`proptest`, `quickcheck`, `loom`, `shuttle`, `miri`, or `kani`. No coverage or
mutation configuration exists in the repository, so every placement statement
in this file is structural rather than
measured. `.config/nextest.toml:1-11` defines only `profile.default` and
`profile.ci` and has no `daemon` entry, so no 4f test is serialized, grouped, or
timeout-adjusted.

## Production assertions and guards, clustered

Measured over production halves only, meaning each file up to its test-module
line as listed above, with the four pre-module `#[cfg(test)]` regions
(`config.rs:245`, `config.rs:395`, `boundary.rs:701`, `boundary.rs:970`)
treated as test-only. Config guards are enumerated under
[Production config guards](#production-config-guards) and are not repeated here.

Runtime assertions: three, all `debug_assert!`, all compiled out of release, all
in one file. Matching `debug_assert` across every 4f production half returns
exactly these three sites and nothing in the other ten files.

| Site | Guard | Release behaviour | Test | Status |
| --- | --- | --- | --- | --- |
| `codec/opencode.rs:263` | `debug_assert!(replace_from <= messages.len())` | Still panics, elsewhere, in every profile. The slice `&messages[replace_from..]` at `codec/opencode.rs:270` is an index-out-of-range panic on the same condition. The assertion buys a better message for a failure that is not silent | none | unaudited |
| `codec/opencode.rs:264` | `debug_assert!(replace_from <= prior.order.len())` | Silent. The only consumer of `replace_from` against `prior.order` is `prior.order.iter().take(replace_from)` at `codec/opencode.rs:277`, and `take` saturates. A violation yields a short sidecar with no signal | none | unaudited |
| `codec/opencode.rs:484` | `debug_assert!(duplicates.is_empty(), ...)`, the whole body of `assert_unique_tool_use_ids` (`codec/opencode.rs:480-487`) | No enforcement at all. The function becomes a no-op, though `duplicate_tool_use_locations(messages)` at `codec/opencode.rs:483` still runs and its result is discarded | `whole_array_tool_use_guard_rejects_both_id_collision_directions` (`codec/opencode.rs:2028`), debug-gated at `codec/opencode.rs:2026` | unaudited |

Three separable facts, each verified line by line:

1. `codec/opencode.rs:263` is not the dangerous one. Its condition is re-checked
   by the language at `codec/opencode.rs:270`, so a violation fails loudly in
   every profile.
2. `codec/opencode.rs:264` is the silent one. Same function, adjacent line, no
   release equivalent, and `take` at `codec/opencode.rs:277` converts a violated
   precondition into a silently truncated sidecar. Neither assertion has a test:
   `decode_opencode_sidecar_incremental` (`codec/opencode.rs:258`) has exactly
   one test, `incremental_sidecar_carries_pins_across_three_generations`
   (`codec/opencode.rs:2062`), which calls it three times with `replace_from` of
   1, 2, and 2 (`codec/opencode.rs:2077`, `:2088`, `:2098`), all in range. Its
   single production caller is `lib.rs:12786`, outside 4f.
3. `codec/opencode.rs:484` enforces nothing in release, and its only test is
   debug-gated. 4e's `enforce_unique_tool_use_ids` (`transform.rs:10439`) has a
   repair path with its own tests (`transform.rs:20554`, `transform.rs:20567`).
   The encode-side `assert_unique_tool_use_ids` has one arm, so in release it
   neither enforces nor repairs. Two production call sites depend on it:
   `codec/opencode.rs:387` inside the encode path and `lib.rs:13193`. A third
   caller of `assert_unique_tool_use_ids`, `lib.rs:21405`, is inside `lib.rs`'s
   test module (`mod tests`, `lib.rs:16204`). The doc comment at
   `codec/opencode.rs:298` states the debug-only contract. The only test carries
   `#[cfg(debug_assertions)]` at `codec/opencode.rs:2026`. CI's nextest job
   builds tests in the dev profile, so the test runs there; the guard's
   release-profile behaviour has no test in any profile.

No `cfg(not(debug_assertions))` exists anywhere in 4f. Matched across all codec
files, `config.rs`, `scheduler.rs`, `boundary.rs`, `selection.rs`,
`caveman.rs`, `wire.rs`, and `session_resolver.rs`: zero occurrences. So no
release-arm counterpart exists for any of the three assertions. The divergence
risk in 4f is concentrated in one codec file; `config.rs`, `scheduler.rs`,
`boundary.rs`, `selection.rs`, and `caveman.rs` contain no `debug_assert!` and
no `#[cfg(debug_assertions)]` production code. Their input guards are
unconditional expressions that behave identically in both profiles:
`derive_trigger_budget` gates a non-finite or non-positive context limit and
clamps the result (`boundary.rs:310-318`);
`derive_protected_tail_token_target` substitutes defaults for a non-finite
context limit or threshold, floors `usable` at one, and clamps usage through
`clamp_percentage` (`boundary.rs:333-347`, `boundary.rs:890-895`);
`parse_cache_ttl` saturates a non-finite or oversized TTL at `u64::MAX`
(`scheduler.rs:388-397`).

One qualification, because that guard list reads as complete and is not.
`BoundaryContext::trigger_budget` (`boundary.rs:139`) is read at
`boundary.rs:348-350` and again at `boundary.rs:720-725` through
`unwrap_or_else` with no `is_finite` gate on the `Some` arm, unlike
`context_limit`, `execute_threshold_percentage`, and `usage_percentage`. A
`Some(f64::NAN)` therefore reaches `boundary.rs:766`'s
`tail_size_bar: trigger_budget * TAIL_SIZE_TRIGGER_MULTIPLIER`, a bare multiply,
and lands in `TriggerProgress` (`boundary.rs:295-304`), which the trigger
records whenever it resolves a boundary (`boundary.rs:288-290`). This is a
missing guard rather than a conditional one, which is why it does not appear in
a `debug_assertions` census. Owned by
[dec-a-caller-supplied-trigger-budget-is-the-one-unvalidated-float-and-reaches-a-diagnostic](catalog.md#dec-a-caller-supplied-trigger-budget-is-the-one-unvalidated-float-and-reaches-a-diagnostic).

Unconditional runtime assertions in the 4f production halves: none. Matching
`assert!`, `assert_eq!`, and `assert_ne!` excluding `debug_assert` returns one
site, `config.rs:702`, which is the compile-time privileged-key guard inside a
`const _` block (`config.rs:697-707`) and is listed under
[Production config guards](#production-config-guards).

Panicking sites: two, so 4f is unlike 4e, which had none.

| Site | Form | Reachability | Status |
| --- | --- | --- | --- |
| `selection.rs:1155` | `PassClass::Defer => unreachable!("defer returned early")` | A production `unreachable!` on a closed-enum match arm inside `select_reductions_with_outcome` (`selection.rs:1043`). Its safety rests on the early return `if ctx.pass_class == PassClass::Defer` at `selection.rs:1049-1051`, which is exactly the shape METHOD.md reserves `unreachable` semantics for. `defer_pass_produces_nothing` (`selection.rs:3221`) is the nearest test | unaudited |
| `scheduler.rs:836` | `.unwrap_or_else(\|err\| panic!("invalid regex {source:?}: {err}"))` in `compile_case_insensitive` (`scheduler.rs:832-837`) | Infallible by construction. The parameter is `source: &'static str` (`scheduler.rs:832`) and both callers, `overflow_patterns` (`scheduler.rs:839`) and `limit_patterns` (`scheduler.rs:854`), map over the literal constant tables `OVERFLOW_PATTERN_SOURCES` (`scheduler.rs:36`) and `LIMIT_EXTRACTION_PATTERN_SOURCES` (`scheduler.rs:60`). Not reachable from configuration or provider text | unaudited |

`.expect(`: five, in three files. `scheduler.rs:871` `"valid 413 regex"` on a
literal pattern; `caveman.rs:184` `"static pattern set builds"` on the
Aho-Corasick build over constant tables; `caveman.rs:236`
`"run_start > 0 within text"` on a char walk; `caveman.rs:626`
`"digits are ascii"` on a byte slice already scanned as ASCII digits;
`wire.rs:337` `"flat projection differential bytes must serialize"`. Two name a
contract no test asserts directly: `caveman.rs:236` and `wire.rs:337`.

`.unwrap()`: 13 in production halves, and all but five are regex compilation.
`caveman.rs` carries eight `Regex::new(...)` over literal patterns inside
`get_or_init` (`caveman.rs:480`, `:520`, `:528`, `:551`, `:556`, `:561`, `:566`,
`:574`) and four `next_char(text, ...).unwrap()` calls on its hand-rolled cursor
walk (`caveman.rs:670`, `:703`, `:743`, `:762`). `selection.rs:581` is
`serde_json::to_string(k).unwrap()` inside `canonical_json`
(`selection.rs:571`), on a map key. Zero `.unwrap()` in the production halves of
`boundary.rs`, `scheduler.rs`, `config.rs`, `wire.rs`, `session_resolver.rs`,
and every codec file.

`let _`: three. `boundary.rs:1666`, `wire.rs:759`, and `codec/sidecar.rs:429`
carry one each.

Typed rejection guards. With no unconditional runtime assertion, 4f enforcement
is a returned value or a `Result`. `wire.rs`'s `project_messages`
(`wire.rs:370`) is the only in-scope decoder returning `Result`, and it rejects
five classes of `WireError` (`wire.rs:343-362`): `EmptyMid` (`wire.rs:438`),
`MidContainsReservedHash` (`wire.rs:443`), `DuplicateMid` (`wire.rs:446`),
`UnsupportedBlock` (`wire.rs:629`), and `UnpairedToolResult` (`wire.rs:704`,
`wire.rs:711`). Tests cover four of the five.
`user_carried_tool_result_pairs_with_prior_assistant_call` (`wire.rs:1197`) and
`user_carried_tool_result_without_prior_call_still_rejects` (`wire.rs:1262`)
cover tool-result pairing, and `wire.rs:1258` and `wire.rs:1283` assert
`UnpairedToolResult`. `empty_and_reserved_message_ids_are_rejected`
(`wire.rs:1415`) asserts `EmptyMid` and `MidContainsReservedHash`
(`wire.rs:1417-1425`).
`duplicate_message_ids_are_rejected_across_the_incremental_prefix`
(`wire.rs:1432`) asserts `DuplicateMid` on the flat and incremental paths
(`wire.rs:1438-1441`, `wire.rs:1455-1458`). `UnsupportedBlock` has no test that
reaches `wire.rs:629`.

## Codec golden coverage

Both harness directions have a golden, both goldens are one case each, their
oracle is derived from the test's own input, and each clears one required block
class through a missing-capture-classes mechanism. Both fixtures were read
directly.

| Codec path | Golden | Cases | Oracle | Status |
| --- | --- | --- | --- | --- |
| `decode_opencode` then `encode_opencode` | `codec/opencode-golden.json` at `codec/mod.rs:61`, test `opencode_golden_round_trips_wire_projected_parts_and_is_deterministic` (`codec/mod.rs:59`) | 1 | Decode twice and compare (`codec/mod.rs:83-85`), encode twice and compare (`codec/mod.rs:89-91`), then `encoded == strip_opencode_compaction(case.messages)` (`codec/mod.rs:92`) | unaudited |
| `decode_pi` then `encode_pi` | `codec/pi-golden.json` at `codec/mod.rs:186`, test `pi_golden_round_trips_non_compaction_entries_and_is_deterministic` (`codec/mod.rs:184`) | 1 | Same shape: `codec/mod.rs:208-210`, `codec/mod.rs:214-216`, then `encoded == strip_pi_compaction(case.entries)` (`codec/mod.rs:217`) | unaudited |
| `encode_opencode_with_session`, m0/m1/synthetic prefix | `codec/serve-native-golden.json` at `codec/mod.rs:107-109`, test `serve_native_golden_preserves_ingress_and_pins_synthetic_shapes` (`codec/mod.rs:97`) | 1 | Field by field against `golden.m0`, `golden.m1`, `golden.synthetic_todo`, then `&encoded[3..] == golden.messages` (`codec/mod.rs:126-129`) | unaudited |

The directions with no golden, established from the golden module's `use super`
list (`codec/mod.rs:28-30`, exactly `decode_opencode`, `decode_pi`,
`encode_opencode`, `encode_opencode_with_session`, `encode_pi`) against the
crate's re-export list (`codec/mod.rs:9-15`):

| Entry point or path | Golden | Notes |
| --- | --- | --- |
| `decode_opencode_with_sidecar` | none | Re-exported at `codec/mod.rs:10`, absent from the `use super` list (`codec/mod.rs:28-30`) |
| `decode_opencode_with_sidecar_and_base` | none | Re-exported at `codec/mod.rs:11`. One of the three `decode_*_with_sidecar` forms, not a fourth entry point |
| `decode_pi_with_sidecar` | none | Re-exported at `codec/mod.rs:14`. These three are the complete set of `decode_*_with_sidecar` forms |
| `encode_opencode_with_session_exemptions` | none | Re-exported at `codec/mod.rs:12`, absent from the `use super` list (`codec/mod.rs:28-30`) |
| `decode_opencode_sidecar_incremental` | none | Crate-private (`codec/opencode.rs:258`). Hand-built only, in `incremental_sidecar_carries_pins_across_three_generations` (`codec/opencode.rs:2062`), with in-range arguments 1, 2, 2 |
| `codec/sidecar.rs` identity functions: `decoded_block_fingerprint` (`codec/sidecar.rs:169`), `stamp_block_identity` (`codec/sidecar.rs:177`), `has_stamped_block_identity` (`codec/sidecar.rs:205`), `block_is_unchanged` (`codec/sidecar.rs:209`), `stable_hash` (`codec/sidecar.rs:407`), `stable_hash_prefix` (`codec/sidecar.rs:417`) | none | No direct test asserts their output. The third alignment test calls `decoded_block_fingerprint` as an input generator (`codec/sidecar.rs:529`, `:542`, `:546`), not as a subject. Reached transitively from `codec/opencode.rs:571-572`, `codec/opencode.rs:740`, `codec/opencode.rs:761`, `codec/pi.rs:330-331`, `codec/pi.rs:399`, `codec/pi.rs:487`, and `transform.rs:9977`, so their codec exercise is the two one-case goldens plus the hand-built codec tests |
| `codec/sidecar.rs` alignment: `match_block_metas` (`codec/sidecar.rs:257`) | none | Direct tests at `codec/sidecar.rs:488-557` cover the oversized linear path and the private `greedy_block_metas` walk (`codec/sidecar.rs:282`), not `optimal_block_metas` (`codec/sidecar.rs:353`) under its cell budget. Production callers `codec/opencode.rs:716` and `codec/pi.rs:392` |
| Compaction extraction, both harnesses | excluded by construction | `strip_opencode_compaction` (`codec/mod.rs:279-287`) and `strip_pi_compaction` (`codec/mod.rs:289-294`) remove compaction from the expected value before comparison at `codec/mod.rs:92` and `codec/mod.rs:217`. Covered by hand-built tests only: `compaction_is_extracted_as_boundary_not_content` (`codec/opencode.rs:1809`) and `compaction_entry_is_boundary_signal` (`codec/pi.rs:1457`) |

The gate is designed to pass without a class, and the two cleared classes are
`subtask` and `redacted_thinking`. `assert_coverage_or_recorded_missing`
(`codec/mod.rs:260-277`) fails only on a required class present in neither list,
filtering `required` against `actual` and `recorded_missing`
(`codec/mod.rs:268-276`), so listing a required class in
`missing_capture_classes` clears it. Reading both fixtures:

| Golden | Required classes | Covered | Passing only by being declared missing |
| --- | --- | --- | --- |
| `opencode-golden.json` | 12 (`codec/mod.rs:66-79`) | 11 | `subtask` |
| `pi-golden.json` | 13 (`codec/mod.rs:190-204`) | 12 | `redacted_thinking` |

So two block classes the test itself declares required have no golden coverage
in either direction, and the gate is green. `redacted_thinking` is the more
load-bearing of the two: it is an Anthropic-signed block class, and 4e's records
turn on signed-block handling.

The encode-direction oracle is self-referential, and it belongs here as a
sampling limit rather than as coverage. For both harnesses the expected value is
a transformation of the test's own input, not an independently generated
expected output. That proves round-trip identity over unmodified content. It
cannot detect a decode error the encoder symmetrically reverses, because both
halves are the code under test. Each fixture carries a `projection_oracle` field
naming an intended external oracle with `status: todo`, and nothing
deserializes it.

## Suspiciously quiet areas

Ranked by the gap between what the code decides and what any check proves.

1. `codec/sidecar.rs` owns the block identity everything downstream keys on,
   and its identity functions have no direct test. The file's three tests
   (`codec/sidecar.rs:488-557`) cover alignment pairing only.
   `stamp_block_identity` (`codec/sidecar.rs:177`), `has_stamped_block_identity`
   (`codec/sidecar.rs:205`), `block_is_unchanged` (`codec/sidecar.rs:209`),
   `stable_hash` (`codec/sidecar.rs:407`), and `stable_hash_prefix`
   (`codec/sidecar.rs:417`) have no direct test anywhere in the crate, and
   `decoded_block_fingerprint` (`codec/sidecar.rs:169`) appears in a test only
   as an input generator. Every exercise they get is transitive, through
   `codec/opencode.rs:571-572`, `codec/opencode.rs:740`,
   `codec/opencode.rs:761`, `codec/pi.rs:330-331`, `codec/pi.rs:399`,
   `codec/pi.rs:487`, and `transform.rs:9977`. `block_is_unchanged` decides
   whether a decoded block is treated as unchanged and therefore whether native
   extras replay verbatim, so a fingerprint that silently collides or silently
   differs changes served bytes. This is the quietest area in the sub-part.

2. `caveman.rs` is 975 lines behind one 42-case snapshot, two structural
   tests, and two differential oracles, none of which is the live TypeScript.
   `differential_golden_matches_typescript_oracle` (`caveman.rs:901`, extent
   `caveman.rs:901-929`) replays `caveman-golden.json` (`caveman.rs:903`); the
   fixture has no TypeScript consumer and no tracked generator, so the
   compatibility contract the header names (`caveman.rs:3-8`) is a snapshot.
   The integration oracles (`crates/daemon/tests/caveman_reference.rs`,
   `crates/daemon/tests/caveman_differential.rs`) are Rust ports and a frozen
   copy of the implementation. The file carries 12 of the 13 production
   `.unwrap()` calls in 4f, four of them on a hand-rolled cursor walk. 4e
   records the same fixture as the seam
   (`../rendering/existing-checks.md:725-728`) and adds the part 4f cannot see:
   caveman payloads feed the hygiene metric through
   `tail_hygiene.rs:431-437` and `tail_hygiene.rs:540`, so the compression and
   the metric consuming it are pinned separately and never checked together.

3. The codec goldens are one case each, with a self-referential oracle and a
   gate built to pass without a required class. One case in
   `opencode-golden.json` and one in `pi-golden.json`; `subtask` and
   `redacted_thinking` cleared through `assert_coverage_or_recorded_missing`
   (`codec/mod.rs:260-277`); and the expected value at `codec/mod.rs:92` and
   `codec/mod.rs:217` derived from the input by `strip_opencode_compaction` and
   `strip_pi_compaction`. Four re-exported codec entry points and the
   crate-private incremental decoder have no golden, the identity functions in
   `codec/sidecar.rs` have none, and compaction extraction is excluded from the
   oracle by construction. These two tests carry 3,602 lines of
   `codec/opencode.rs` plus `codec/pi.rs` production code.

4. `codec/opencode.rs:264` is the only guard in 4f whose violation is silent in
   release, and it sits one line from one that is not. The `messages.len()`
   bound at `codec/opencode.rs:263` is re-checked by the slice at
   `codec/opencode.rs:270`; the `prior.order.len()` bound at
   `codec/opencode.rs:264` is consumed by `take` at `codec/opencode.rs:277`,
   which saturates. Neither has a test, and the only test of the function
   passes in-range values.

5. `assert_unique_tool_use_ids` enforces nothing in a release build and has no
   release-arm test. `assert_unique_tool_use_ids` (`codec/opencode.rs:480-487`)
   has two production callers (`codec/opencode.rs:387`, `lib.rs:13193`), one
   test gated `#[cfg(debug_assertions)]` (`codec/opencode.rs:2026`), and no
   `cfg(not(debug_assertions))` anywhere in 4f. Unlike 4e's belt it has no
   repair arm, so the encode-side duplicate-id contract is unenforced in the
   profile a release artifact ships.

6. `boundary.rs` is named by no test in `transform.rs`'s test module
   (`pub(crate) mod tests`, `transform.rs:11754` onward) and by no integration
   binary. Its in-crate evidence is its own 29 file-local tests, three
   Rust-only goldens, and three `lib.rs` tests that call
   `check_compartment_trigger`:
   `historian_trigger_token_reuse_matches_retokenized_production_shape`
   (`lib.rs:16658`),
   `trigger_suppresses_fire_when_projected_drops_hit_relative_target`
   (`lib.rs:16933`), and the ignored `historian_trigger_token_reuse_benchmark`
   (`lib.rs:16762`). No whole-pass test in `transform.rs` asserts anything
   about where the protected-tail split lands.

7. `selection.rs:1155` is a live production `unreachable!` whose safety rests
   on the non-local early return `if ctx.pass_class == PassClass::Defer`
   (`selection.rs:1049-1051`). 4e recorded zero panicking sites; 4f has this
   one plus the infallible-by-construction `scheduler.rs:836`.

8. `session_resolver.rs` is 70 lines with one test covering the absence case
   only. `unsupported_mapping_is_local_absence` (`session_resolver.rs:61`). The
   supported-mapping path has no test in the file.

9. No 4f fixture has a TypeScript leg. `cache-ttl-routing-vectors.json` is read
   by `config.rs:1135` only; the TypeScript `resolveCacheTtl` tests
   (`packages/opencode-plugin/src/hooks/context/event-resolvers.test.ts:82-116`)
   use their own expectations. Every other 4f fixture is Rust-only too, so a
   Rust-versus-TypeScript divergence in any 4f unit is caught by no automated
   check on either side.

10. No 4f fixture has a provenance guard and no workflow regenerates any of
    them. Eight fixtures, zero hash checks, and the two `generated_from` and
    `projection_oracle` fields that exist are not deserialized by
    `OpenCodeGolden` (`codec/mod.rs:33-38`) or `PiGolden`
    (`codec/mod.rs:46-51`).

11. The classified merge lacks a complete independent per-key policy oracle.
    Its checks are enumerated under [Config checks](#config-checks) and the
    limits under [Assertion scope](#assertion-scope): the hostile fixture
    derives its expected ignored-key count from `tier_class`, and the
    privileged-name check only excludes `ProjectAllowed` for those names.

12. No integration coverage of the boundary, the scheduler, the codecs, or the
    wire projector. Of 17 integration binaries, four reach 4f, and they reach
    `caveman::compress`, `selection::select_reductions_with_outcome`, and the
    config reader. Two of the seventeen have names suggesting otherwise and do
    not deliver: `boundary_counter_durability.rs` tests a store counter, and
    `broca_roundtrip.rs` tests an RPC stream.

## Registered claims that no record owns

Two claims were registered in the claims lens with implementing code identified
and a testable property spelled out, and then became no record in `catalog.md`.
Both were written up in full at
[`_lenses/lens-c1-claims-and-config.md:359-389`](_lenses/lens-c1-claims-and-config.md).
Their source document, `CONFIGURATION.md`, is not tracked at HEAD, so the quoted
guarantees survive only in the lens file. They fell through an ownership gap:
each claim straddles 4f and 4b, and 4f takes both because 4f owns the decision
and the other part owns the application.

| # | Claim | Implementing code at HEAD | Existing check and what a record needs |
| --- | --- | --- | --- |
| C1-29 | Caveman tier shifts are path-independent: compressing the original at the final depth gives byte-identical output to shifting through intermediate depths | Real, and outside 4f: `transform.rs:5690` reads `row.source_bytes`, `transform.rs:5705-5707` refuses a non-increasing depth (`target_depth <= existing_depth`), and `transform.rs:5709` calls `caveman::compress(&source, level)` on the pristine text. The claim is load-bearing because `compress` is not idempotent by construction: `apply_ultra_connectives` (`caveman.rs:440`) and `apply_ultra_abbreviations` (`caveman.rs:456`) rewrite words into symbols a second pass would read as different input | `caveman_depth_deepens_from_source_without_regrowth` (`transform.rs:24650-24687`) seeds a Lite unit and asserts the deepened payload equals `compress(&source, Ultra)` (`transform.rs:24683-24686`), which covers the mechanism for one depth pair at 4b's layer. Status `unaudited`. No test asserts the inequality `compress(compress(t, Lite), Ultra) != compress(t, Ultra)` or that the production path never takes the left-hand form for every depth pair |
| C1-30 | With `smart_drops` off, the messages sent to the model are byte-identical to the age-based-only behaviour, so the feature is inert | `NOT FOUND` as a byte-equality check. The flag defaults `false` (`config.rs:128`) and is `ProjectAllowed` (`config.rs:680-681`), so either tier can set it | None. `smart_drops` appears in test fixtures as a fixed flag (`transform.rs:13190`, `transform.rs:13204`) and no test compares the emitted array with the flag off against the flag on over one identical input. A record needs that differential oracle. The [tier-policy record](catalog.md#dec-a-project-tier-can-write-leaves-outside-the-documented-allow-list) covers who may set the flag; this one would cover what the flag does when unset |

Neither is mined here, per METHOD rule 6. Both are queued in
[portfolio-evaluation.md](portfolio-evaluation.md) with the owner recorded.

## Sampling limits on this inventory

Seven limits, stated so a later pass does not read absence as absence of risk.

- Every placement statement in this file is structural. The repository has no
  coverage measurement, so nothing here is coverage instrumentation. Obtained
  directly at HEAD: all eleven per-file test counts, the test-module lines,
  the zero `#[ignore]` and zero `should_panic` results, the three
  `debug_assert` sites and the absence of `cfg(not(debug_assertions))`, the
  two panicking sites, the CI line numbers, both codec goldens' case counts and
  coverage arrays, the five `cache-ttl-routing-vectors.json` cases, and every
  cited line.
- The pre-refresh attribution of `transform.rs` unit tests to 4f (the 39-test
  and 192-test figures) is not re-derived here. `transform.rs` has 286 test
  attributes at HEAD with its test module `pub(crate) mod tests` at
  `transform.rs:11754`; the only `transform.rs` claims this file makes are the
  `boundary::` absence in that module and the named tests cited above.
- Whether the two codec goldens exercise a given production path was inferred
  from the golden module's `use super` list (`codec/mod.rs:28-30`) and the
  crate's `pub use` re-export list (`codec/mod.rs:9-15`), not from execution.
  Four exported entry points, the incremental decoder, and the
  `codec/sidecar.rs` identity functions are recorded as having no golden on
  that basis.
- The differential integration oracles for `caveman.rs` and `selection.rs`
  compare production against a frozen in-file copy of the same implementation
  or against a Rust port. They detect drift from that copy; they do not
  establish agreement with the TypeScript originals.
- `caveman.rs`'s tests are counted by both this file and 4e's, so 4e's total
  and any figure here overlap by exactly three in-crate tests plus 975
  production lines.
- The `.unwrap()`, `.expect(`, and `let _` counts are line matches over
  production halves; a call split across lines or spelled through a helper is
  not counted.
- Whether a `#[cfg(debug_assertions)]`-gated test counts as `Exercised: partial`
  is unresolved. For `codec/opencode.rs:2028` the test compiles and runs under
  CI's dev-profile nextest job, but it cannot compile in a release test build
  and the guard it exercises does nothing in release. It needs a human ruling,
  not a synthesis decision.

## Historical inventory

The [pre-refresh inventory](https://github.com/ahrav/eidnara/blob/74044960ee91641dec95c8552f15282844a18b13/docs/properties/daemon/decisions/existing-checks.md)
preserves the cross-part `transform.rs` attribution analysis (the 192-test and
39-test figures and the three-way reconciliation with 4b and 4e) that this file
no longer restates. Its line coordinates, zero-integration claim,
zero-sidecar-test claim, single-caveman-test claim, and absent-CI conclusions
are not current coverage evidence. The sections above replace them.
