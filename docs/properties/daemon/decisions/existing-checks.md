# Part 4f existing-check inventory

Source revision: `74044960ee91641dec95c8552f15282844a18b13`. Config checks and
the budget-reader integration check below are inventoried against that source.
Every existing-check and guard status is `unaudited`: presence, source inspection,
and execution do not establish oracle adequacy.

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

## Historical inventory

The [pre-refresh inventory](https://github.com/ahrav/eidnara/blob/74044960ee91641dec95c8552f15282844a18b13/docs/properties/daemon/decisions/existing-checks.md)
preserves the discovery-time per-check clusters, codec investigations, and
cross-part attribution analysis. Its line coordinates, 192-test attribution,
zero-integration claim, zero-sidecar-test claim, and absent-CI conclusions are not
current coverage evidence. The counts and config inventory above replace those
claims rather than keeping them active under historical provenance.
