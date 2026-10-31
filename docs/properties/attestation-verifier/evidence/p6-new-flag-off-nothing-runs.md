# p6-new-flag-off-nothing-runs

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N7. Converted to active by [#1173](https://github.com/ahrav/eidnara/issues/1173), which lands the bounded pass in `crates/daemon/src/attestation_verifier/bounded_pass.rs`, the `verifier.enabled` key in `crates/daemon/src/config.rs`, and the plugin configuration peers.

## Evidence trail

`verifier_enabled` defaults to false (`crates/daemon/src/config.rs:162`). `ConfigKey::VerifierEnabled` is privileged and `TierClass::UserOnly`, so `merge_tiers_with_warnings` drops a project-tier value with the per-key warning "ignoring /verifier/enabled from project tier; setting is user-tier only". `ConfigCache::verifier_enabled` (`crates/daemon/src/config.rs:513-522`) resolves the user tier that `XDG_CONFIG_HOME` and `HOME` name with the filesystem root as the project, as the contradiction-candidates gate does, since the key is user-tier only; the production reader (`crates/daemon/src/verifier_task.rs:67`) calls it for every project root, and one memoized merge serves them all while the user tier keeps its identity, so an edit to the user tier is read by the next call; `the_pass_reader_reads_the_user_tier_alone` (`crates/daemon/src/config.rs:4001`) reads true from the user tier beside a project root whose own tier file holds false and finds the project tier unread; `the_pass_reader_follows_the_user_tier_and_drops_a_project_tier_value` (`:3959`) rewrites the user tier through one cache and reads a project-tier value alone as false with the warning.

The daemon spawns one verifier task per data home after the store opens (`crates/daemon/src/lib.rs:4057`), through `task` (`crates/daemon/src/verifier_task.rs:19`), which reads the flag for each bound project root through `enabled_reader`. `Verifier::pass` (`crates/daemon/src/attestation_verifier/bounded_pass.rs:349`) filters the bound projects by that flag first (`:372`) and returns without counting a pass when none is enabled, before it derives a repository identity, captures a Kernel target, or reads a page. Every repository open in a pass follows that filter: the identity derivation (`:478`) runs only for the enabled projects' roots, and `verify` (`:738`) only inside `ProjectRun::run` (`:519`). `passes` counts only passes whose body ran (`:419`).

The TypeScript peers: `EidnaraConfigSchema` defines `verifier` as a strict object with `enabled` defaulting to false; `stripUnsafeProjectConfigFields` deletes a project-tier `verifier` block and warns "Ignoring verifier from project config" when it sets the switch; the shared fixture `packages/opencode-plugin/src/config/__fixtures__/source-capture-parity.json` runs through the Rust cache, the OpenCode loader, and the Pi loader a second time with its `source_capture` blocks renamed to `verifier`.

`a_flag_off_pass_runs_nothing_and_a_reload_to_false_stops_the_next_pass` (`crates/daemon/tests/attestation_verifier_bounded_pass.rs:442`) holds N + 2 verifiable decisions: with the flag false the pass records no poll, no counter moves, and `COUNT(object_registry WHERE source_kind='verifier')` is 0; the same store with the flag true opens the repository N times and commits N attestations (the positive control); a reload to false then stops the next pass before any poll. `the_task_steps_only_under_the_flag_and_stops_on_cancellation` (`crates/daemon/tests/attestation_verifier_bounded_pass.rs:825`) drives the task's `run` (`crates/daemon/src/verifier_task.rs:87`) with the flag off and then on.

## Failure scenario

A pass that opens a repository or writes before the flag is read, or a verifier row in a default store.

## Timing windows and dependencies

A reload between two passes: the next pass reads the flag again at its start.

## What a test must construct

F12: flag absent, a reload from true to false, a project-tier flag.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record and this file, read at the #1174 branch tip.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Is the project-tier refusal tested in every loader?

- Sources examined: `every_fixture_row_matches_through_the_configuration_cache` in `mod verifier_parity` (`crates/daemon/src/config.rs:3919`), the `verifier parity fixture` blocks in `packages/opencode-plugin/src/config/index.test.ts:1101` and `packages/pi-plugin/src/config/index.test.ts:754`, and `strips verifier from project config in either direction` (`packages/opencode-plugin/src/config/project-security.test.ts:120`).
- Findings: Each loader reads every fixture row; a project-tier `verifier.enabled` never changes the effective value and draws exactly one warning.
- Missing evidence: none.
- Conclusion: resolved with answer.
