# Discovered-at-U3 existing-check inventory

Every claim-bearing check for the 16 records under `## Discovered at U3` in
[catalog.md](../catalog.md) (`:9875-10120`). Those records cover code the six
source catalogs did not reach: `crates/host-runtime/src/broca/` (eight files,
4,960 lines), `src/synapse/` (five files, 4,968), `src/harness_closure.rs`
(1,146), the proof vectors in `src/auth.rs` (1,088), the header vectors in
`src/wire.rs` (937), the data-root resolver in `src/instance.rs` (1,349), and
the coordination locks in `src/lifecycle.rs` (2,262).

Provenance: branch `u3/16-catalog-host-runtime`, `HEAD` = `572315a`, working
tree clean outside `docs/`. Every count here was derived at that commit by
listing `#[test]` and `#[tokio::test]` attributes and the `fn` line each
precedes, by reading the `harness = false` runner's name table
(`tests/broca_subprocess.rs:76-218`), and by grepping the production half of
each source file, cut at its `#[cfg(test)] mod tests` line. Every `file:line`
below was printed and confirmed.

**Every status below is `unaudited`.** An existing check never removes a
property from the catalog. Test adequacy belongs to
`/testing:invariant-test-review`; production guard adequacy belongs to
`/low-level-systems:defensive-assertions-and-invariant-guards`. The catalog
records say "audited at U3" for seven of the sixteen; that phrase records that
the check was located and named at U3, and this inventory does not upgrade it
to an adequacy verdict.

## The coverage fact that frames this inventory

**Every check in this inventory runs in CI, twice, except two `#[ignore]`
tests and six that return early without an ONNX Runtime library.** This is the
opposite position from the six earlier inventories, and it must not be copied
from them. `.github/workflows/ci.yml:118` runs
`cargo +1.98 test --workspace --all-targets --all-features --locked`, `:126`
runs the same under `+stable`, and `:122` runs `--workspace --doc`. The root
`Cargo.toml:10` lists `crates/host-runtime` as a workspace member. `--all-
targets` builds the lib test target, every integration binary, and the
`harness = false` binary declared at `crates/host-runtime/Cargo.toml:36-38`.
The job runs on `ubuntu-latest` (`ci.yml:14`), so the two `#[cfg(target_os =
"linux")]` tests in `src/synapse/inference.rs` compile and run.

Three attenuations apply, and they are the whole list.

| Attenuation | Sites | Effect in CI |
| --- | --- | --- |
| `#[ignore]` | `tests/synapse_protocol.rs:412-415` `boundary_waiters_with_maximal_texts_are_all_admitted`; `tests/harness_closure.rs:442-443` `production_closures_from_environment_materialize` | Never executed. `ci.yml` passes no `--include-ignored` (grep: zero hits) |
| Early return when `EIDNARA_SYNAPSE_TEST_ORT_LIBRARY` is unset | `tests/synapse_bundle.rs:29-42` `ort_library()`, called at `:573`, `:645`, `:687`, `:699`, `:709`; `tests/synapse_roundtrip.rs:27-38`, called at `:121` | Six tests **pass without asserting**: `certified_bundle_loads_and_serves_expected_vectors` (`:572`), `production_bundle_from_environment_certifies_offline` (`:639`, which also needs `EIDNARA_SYNAPSE_PRODUCTION_BUNDLE` at `:640`), `wrong_but_dimension_compatible_output_fails_certification` (`:686`), `wrong_pooling_fails_certification` (`:698`), `wrong_ort_identity_disables_the_lane` (`:708`), `all_four_operations_serve_certified_vectors_over_the_wire` (`synapse_roundtrip.rs:120`). `ci.yml` sets neither variable (grep `EIDNARA`: zero hits) |
| Wall-clock timing | `tests/synapse_jobs.rs:242` sets `retention` to 100 ms and `:266` sleeps 250 ms real time; `tests/broca_supervisor.rs` uses `start_paused` at 5 sites and `tests/synapse_protocol.rs` at 6 | Not an execution gap, recorded so a later flake triage starts in the right place |

The six earlier inventories state that no `host-runtime` test runs in CI, for
example `../runtime-config/existing-checks.md:24` and
`../ring-datapath/existing-checks.md:26`. Those statements were true of the
workflow they read and are not true of `ci.yml` at this `HEAD`. This inventory
does not edit them.

## Where the checks live

**237 checks: 165 in nine integration binaries, 72 in nine inline modules.**

### Integration binaries, 165

| Binary | Tests | Lines | Subject | Fixture |
| --- | --- | --- | --- | --- |
| `tests/broca_protocol.rs` | 9 | 710 | Broca request schema, bind, boundary | `ScriptedBackend` (`tests/support/broca.rs:25`), `start_broca_host` (`:170`), `raw_client` |
| `tests/broca_subprocess.rs` | 39 | 3,176 | real harness children, env snapshot, reaping | `harness = false`; the binary re-executes itself as the child under `EIDNARA_BROCA_FIXTURE_MODE` (`:45`, `:61-72`, `:293-300`) with behaviours at `:636-657` |
| `tests/broca_supervisor.rs` | 25 | 1,289 | dedup, permits, terminals, shutdown | `ScriptedBackend`, `std::sync::Barrier` (`:168`), `start_paused` |
| `tests/synapse_bundle.rs` | 24 | 905 | bundle load, fingerprint, degrade | `synapse-tiny` fixture (`:20-22`), `expect_disabled_with` (`:105-115`), `ort_library()` gate |
| `tests/synapse_jobs.rs` | 11 | 611 | job table admission and eviction | `DeterministicEngine` (`tests/support/synapse.rs:40`), `SynapseHost` (`:172`) |
| `tests/synapse_protocol.rs` | 24 | 1,369 | request validation, waiters, replay | `DeterministicEngine` with `calls` counter (`:43`), `block_calls` gate (`:71`) |
| `tests/synapse_roundtrip.rs` | 2 | 251 | degrade over the wire; certified vectors | `synapse-tiny`, `ort_library()` gate |
| `tests/protocol_vectors.rs` | 15 | 765 | committed auth and header vectors | `raw_client::proof` (`tests/support/raw_client.rs:251`), `header` (`:271`), `decode_header` (`:283`) |
| `tests/harness_closure.rs` | 16 | 699 | closure manifest, materialization | `pi-valid.json` (`tests/fixtures/harness-closures/`), `setup()` |

`tests/broca_subprocess.rs` has zero `#[test]` attributes. Its 39 checks are
plain `fn`s named in the table at `:76-218`, run under `catch_unwind` at
`:242`, and listed for nextest at `:219-225`. `cargo test` with no filter runs
all 39. The earlier `../runtime-config/existing-checks.md:162` recorded the zero
attribute count and stopped; this inventory counts the runner's table.

### Inline modules, 72

| File | `mod tests` at | Tests | Selection |
| --- | --- | --- | --- |
| `src/broca/subprocess.rs` | `:1651` | 1 | all |
| `src/broca/{backend,config,mod,opencode,pi,protocol,supervisor}.rs` | none | 0 | seven files, 3,279 lines, no test module |
| `src/synapse/bundle.rs` | `:814` | 11 | all |
| `src/synapse/inference.rs` | `:337` | 2 | all, both `#[cfg(target_os = "linux")]` |
| `src/synapse/jobs.rs` | `:700` | 7 | all |
| `src/synapse/mod.rs` | none | 0 | 1,064 lines, no test module |
| `src/synapse/protocol.rs` | `:975` | 13 | all |
| `src/harness_closure.rs` | none | 0 | 1,146 lines, no test module |
| `src/wire.rs` | `:613` | 14 | all; `:513` is a `#[cfg(test)]` helper `encode_frame`, not a module |
| `src/auth.rs` | `:585` | 12 | all |
| `src/lifecycle.rs` | `:1135` | 10 of 33 | tests that construct `LifecycleTransactionLock`, name `.eidnara-coordination`, or assert the lifetime fence across a subtree replacement |
| `src/instance.rs` | `:834` | 2 of 22 | the data-root resolver tests only |

The `lifecycle.rs` selection is `:1530`, `:1545`, `:1585`, `:1618`, `:1635`,
`:1981`, `:2001`, `:2032`, `:2150`, `:2181`. The other 23 are Part lifecycle
scope. The `instance.rs` selection is `:860` and `:867`; the other 20 are
setup-identity scope. `../setup-identity/existing-checks.md:28-30` places the
`auth.rs` module at `:633` with 11 tests and the `instance.rs` module at
`:889`; at this `HEAD` they are at `:585` with 12 and `:834`. The drift is
recorded, not corrected there.

## Checks by record

One block per record, in catalog order. "Named" is the check the record's
`Existing check:` field cites, located and confirmed. "Also bears" is every
other check in scope whose assertion touches the record's guarantee. Every
named check was found; none is missing.

### host-proof-construction-matches-the-committed-vectors

| Check | Site | Status |
| --- | --- | --- |
| Named `committed_wire_vectors_pin_the_proof_construction` | `src/auth.rs:641` | unaudited |
| Named `committed_auth_proof_vectors_pin_the_construction` | `tests/protocol_vectors.rs:33` | unaudited |
| Named `proof_folds_every_input` | `tests/protocol_vectors.rs:75` | unaudited |
| Also bears `host_authenticates_against_the_independent_oracle` | `tests/protocol_vectors.rs:221` | unaudited |
| Also bears `malformed_and_wrong_proof_handshakes_close_without_envelope_traffic` | `tests/protocol_vectors.rs:308` | unaudited |
| Also bears `wrong_client_proof_is_rejected_and_error_carries_no_secrets` | `src/auth.rs:870` | unaudited |
| Also bears `invalid_server_proof_sends_no_client_auth` | `src/auth.rs:1051` | unaudited |

**Contract-versus-check note.** The record's `Check:` is
`compute_proof(...) == raw_client::proof(...)` over the committed inputs and
every single-field perturbation. No test asserts that equality directly.
`auth.rs:641` pins `compute_proof` to two hex literals (`:648-655`);
`protocol_vectors.rs:33` pins `raw_client::proof` to two decimal byte arrays
(`:36-43`). The arrays and the hex agree byte for byte (`89, 41, 95, 101` is
`59 29 5f 65`), so the two sides meet through a shared literal, not through a
call. `proof_folds_every_input` (`:75-159`) perturbs the **oracle** only;
`compute_proof` is never perturbed. The one place host and oracle are compared
live is `:221`, through a real handshake with no perturbation. Recorded as an
observation about the check's shape, not as an adequacy verdict.

### data-root-resolves-under-the-managed-directory

| Check | Site | Status |
| --- | --- | --- |
| Named `default_root_follows_xdg_then_home` | `src/instance.rs:867` | unaudited |
| Named `explicit_override_resolves_canonical_layout` | `src/instance.rs:860` | unaudited |

**None found** for the environment-reading arm. `data_dir_path`
(`instance.rs:130`) reads `XDG_DATA_HOME` and `HOME` at `:139-140` when the
override is `None`. Both tests call the pure `default_data_root(DataRootEnv)`
(`:155`) or pass `Some(root)`, so the production arm at `:138-141` has no
test. That is the design choice the record's `Confidence:` line describes
(the resolver was split so tests never call `set_var`), and it leaves the
three-line glue untested by construction.

### coordination-locks-live-beside-the-managed-subtree

| Check | Site | Status |
| --- | --- | --- |
| Named `independent_openers_see_one_stable_coordination_identity` | `src/lifecycle.rs:2001` | unaudited |
| Named "replaced-subtree tests": `a_replaced_lifecycle_child_cannot_mint_a_second_transaction_owner` | `src/lifecycle.rs:1981` | unaudited |
| Named "replaced-subtree tests": `a_replaced_eidnara_subtree_is_not_reported_stopped_while_the_daemon_lives` | `src/lifecycle.rs:2032` | unaudited |
| Also bears `transaction_lock_is_exclusive_on_the_stable_coordination_file` | `src/lifecycle.rs:1530` | unaudited |
| Also bears `namespace_drift_fails_the_holder_before_a_named_commit` | `src/lifecycle.rs:1545` | unaudited |
| Also bears `shared_probe_lock_never_creates_and_yields_none_under_a_mutator` | `src/lifecycle.rs:1585` | unaudited |
| Also bears `symlinked_coordination_root_fails_closed_for_probes` | `src/lifecycle.rs:1618` | unaudited |
| Also bears `hostile_shapes_at_the_lock_names_fail_closed` | `src/lifecycle.rs:1635` | unaudited |
| Also bears `lifetime_and_runtime_lock_disagreement_is_wedged` | `src/lifecycle.rs:2150` | unaudited |
| Also bears `a_pre_coordination_incumbent_classifies_by_its_record` | `src/lifecycle.rs:2181` | unaudited |

**Contract-versus-check note.** The guarantee names both locks at
`<root>/.eidnara-coordination/{lifetime,transaction}.lock` and asserts one
inode identity. `:2001` asserts the literal path (`:2004-2007`) and the
`(dev, ino)` identity (`:2013`, `:2026-2030`) for `transaction.lock` only.
`lifetime.lock` appears in no test as a literal path; `:1638` iterates
`[TRANSACTION_LOCK_NAME, LIFETIME_LOCK_NAME]` through the constants. The
lifetime fence's survival of a subtree replacement is asserted behaviourally at
`:2032-2071`, through `InstanceGuard::acquire` returning `AlreadyRunning`
(`:2056-2062`), not through an inode comparison.

### canonical-route-open-declares-its-exact-body-length

| Check | Site | Status |
| --- | --- | --- |
| Named `canonical_route_open_body_is_167_bytes` | `tests/protocol_vectors.rs:196` | unaudited |
| Named `committed_header_vectors_decode_to_their_documented_fields` | `tests/protocol_vectors.rs:160` | unaudited |
| Also bears `little_endian_and_frozen_prefix_layout` | `src/wire.rs:670` | unaudited |
| Also bears `round_trip_request` | `src/wire.rs:647` | unaudited |
| Also bears `reject_truncated_headers_and_unsupported_versions` | `src/wire.rs:689` | unaudited |
| Also bears `reject_pure_header_frame_with_body_len` | `src/wire.rs:744` | unaudited |

**Contract-versus-check note.** The committed 21 bytes at `:163` are decoded by
`raw_client::decode_header` (`tests/support/raw_client.rs:283`) and re-encoded
by `raw_client::header` (`:271`), both test-local. No test feeds those bytes to
the host decoder in `wire.rs`. The host's layout is pinned separately at
`wire.rs:670-686` against field offsets, not against the committed vector. The
167-byte body at `:197-200` is measured and parsed as JSON (`:207-209`); no
test sends it to a host and observes framing. The record's `Check:` says
"decoded by the test-local decoder", so the record and the check agree; the
note is that the host codec and the committed bytes never meet.

### harness-closure-manifest-digest-is-canonical

| Check | Site | Status |
| --- | --- | --- |
| Named `canonical_manifest_digest_is_pinned` | `tests/harness_closure.rs:429` | unaudited |
| Named "strict-decode tests": `strict_manifest_decode_rejects_unknown_fields` | `tests/harness_closure.rs:418` | unaudited |
| Also bears `ordered_extensions_are_part_of_manifest_identity` | `tests/harness_closure.rs:408` | unaudited |
| Also bears `source_and_retained_hash_mismatches_fail_closed` | `tests/harness_closure.rs:325` | unaudited |
| Also bears `retained_closure_survives_source_deletion_and_deduplicates_by_digest` | `tests/harness_closure.rs:208` | unaudited |

**None found** for two clauses of the `Check:`. "Unchanged under key
reordering" has no test; it holds by construction, because
`canonical_manifest` (`src/harness_closure.rs:254-259`) serializes the struct
and sorts the value, so JSON input key order cannot reach the digest. "Changes
when any field changes" is tested for one field, the `extensions` order at
`:408-416`. The "oracle outside the crate" the record names is the Python
`json.dumps(sort_keys=True, indent=2)` run recorded in the evidence file, not
a check in the tree. `production_closures_from_environment_materialize`
(`:443`) is `#[ignore]` and needs three `EIDNARA_*_CLOSURE_*` roots
(`:445-452`).

### credential-fingerprint-derives-from-the-product-domain

| Check | Site | Status |
| --- | --- | --- |
| Named `credential_fingerprint_matches_the_committed_vector` | `src/broca/subprocess.rs:1660` | unaudited |
| Named `provider_rows_exclude_ambient_credentials_and_enforce_caps` | `tests/broca_subprocess.rs:2840` | unaudited |
| Also bears `credential_snapshot_must_match_before_backend_spawn` | `tests/broca_protocol.rs:435` | unaudited |

The named inline test asserts the committed hex at `:1671` and a different key
at `:1674-1679`. The derivation it pins is `subprocess.rs:186-196`. The
record's open question is confirmed at this `HEAD`: `CREDENTIAL_ROW_CAP_BYTES`
(`subprocess.rs:51`) has **zero readers** in `src/` or `tests/` (grep returns
only the definition); the per-value cap `CREDENTIAL_VALUE_CAP_BYTES` (`:48`) is
enforced at `:161`.

### synapse-bundle-fingerprint-covers-every-artifact

| Check | Site | Status |
| --- | --- | --- |
| Named `the_committed_fixture_carries_its_canonical_fingerprint` | `tests/synapse_bundle.rs:375` | unaudited |
| Named `a_bundle_manifest_outside_the_committed_digest_does_not_load` | `tests/synapse_bundle.rs:395` | unaudited |
| Named `one_bit_changes_to_each_artifact_disable_the_lane` | `tests/synapse_bundle.rs:282` | unaudited |
| Also bears `a_stale_fingerprint_disables_the_lane` | `tests/synapse_bundle.rs:358` | unaudited |
| Also bears `unlisted_extra_file_disables_the_lane` | `tests/synapse_bundle.rs:316` | unaudited |
| Also bears `symlinked_artifact_disables_the_lane` | `tests/synapse_bundle.rs:325` | unaudited |
| Also bears `duplicate_manifest_key_disables_the_lane` | `tests/synapse_bundle.rs:340` | unaudited |
| Also bears `fingerprint_binds_initializer_names_to_their_hashes` | `src/synapse/bundle.rs:899` | unaudited |
| Also bears `a_symlinked_artifact_never_opens` | `src/synapse/bundle.rs:1043` | unaudited |

**Contract-versus-check note.** `:375` compares the fixture manifest's stored
`fingerprint` with the crate's own `canonical_fingerprint` (`:384-388`). The
"generator's independent fingerprint function" the record names is
`tests/fixtures/generate-synapse-tiny.py`, which no test executes; the
independence is exercised only when a human regenerates the fixture. `:282`
flips the last byte of seven named artifacts (`:283-291`) and asserts `"hash
mismatch"`; it does not enumerate the pre-image lines at
`src/synapse/bundle.rs:577` onward, so an artifact added to the bundle but
omitted from the pre-image is not what this test detects.

### broca-identical-resends-converge-on-one-run

| Check | Site | Status |
| --- | --- | --- |
| Named `identical_resend_dedups_and_any_byte_difference_conflicts` | `tests/broca_supervisor.rs:126` | unaudited |
| Named `racing_identical_sends_converge_on_one_run_and_one_backend_start` | `tests/broca_supervisor.rs:165` | unaudited |
| Also bears `terminal_expiry_and_oldest_eviction_enforce_the_session_caps` | `tests/broca_supervisor.rs:857` | unaudited |
| Also bears `retained_pressure_sweeps_expired_entries_and_retries_admission_once` | `tests/broca_supervisor.rs:895` | unaudited |
| Also bears `status_and_cancel_are_scoped_to_the_bound_session` | `tests/broca_supervisor.rs:264` | unaudited |

`:857` and `:895` construct the retained-then-evicted state the record's
`Exercised:` gap names; neither issues a resend after it. `:126` asserts
`backend.starts() == 1` (`:160`) and the `idempotency_conflict` code for a
one-space body difference (`:139-145`); `:165` releases two identical sends
through a `Barrier` (`:168-175`).

### broca-permits-and-charges-return-to-baseline

| Check | Site | Status |
| --- | --- | --- |
| Named `every_path_returns_permits_and_charges_to_baseline` | `tests/broca_supervisor.rs:973` | unaudited |
| Named `host_shutdown_drains_the_supervisor_to_zero_state` | `tests/broca_supervisor.rs:1246` | unaudited |
| Named `transport_detach_paths_leave_the_run_untouched` | `tests/broca_supervisor.rs:1099` | unaudited |
| Also bears `subscriber_caps_enforce_per_run_and_total_without_leaking_permits` | `tests/broca_supervisor.rs:385` | unaudited |
| Also bears `thirty_two_blocked_commands_admit_and_command_33_fails_fast` | `tests/broca_supervisor.rs:430` | unaudited |
| Also bears `thirty_two_runs_queue_behind_eight_backends_and_run_33_fails_without_state` | `tests/broca_supervisor.rs:470` | unaudited |
| Also bears `backend_panic_commits_one_failed_terminal` | `tests/broca_supervisor.rs:317` | unaudited |
| Also bears `replay_overflow_commits_one_failed_terminal_and_stops_growth` | `tests/broca_supervisor.rs:928` | unaudited |
| Also bears `shutdown_refuses_new_work_stops_backends_and_wakes_subscribers` | `tests/broca_supervisor.rs:1034` | unaudited |
| Also bears `default_limits_and_resource_declaration_match_the_fixed_caps` | `tests/broca_supervisor.rs:95` | unaudited |

`:973`'s baseline oracle is `assert_baseline(supervisor.metrics(), &limits,
0)` at `:1023` and again after shutdown at `:1030`. The one production guard
on this invariant is `debug_assert!(index.runs.is_empty(), "every run is
session-owned")` at `src/broca/supervisor.rs:641`.

### broca-children-are-reaped-as-a-process-group

| Check | Site | Status |
| --- | --- | --- |
| Named `cancel_reaps_group_with_sigterm_first` | `tests/broca_subprocess.rs:2519` | unaudited |
| Named `sigkill_escalation_when_term_ignored` | `tests/broca_subprocess.rs:2553` | unaudited |
| Named `supervisor_shutdown_reaps_group` | `tests/broca_subprocess.rs:2637` | unaudited |
| Named `group_registry_sweep_kills_only_dead_owner_groups` | `tests/broca_subprocess.rs:3045` | unaudited |
| Also bears `supervisor_delete_reaps_group` | `tests/broca_subprocess.rs:2608` | unaudited |
| Also bears `timeout_reaps_leader_and_grandchild` | `tests/broca_subprocess.rs:2131` | unaudited |
| Also bears `crash_orphaned_run_dirs_swept_only_for_dead_owners` | `tests/broca_subprocess.rs:3003` | unaudited |
| Also bears `pi_lingering_child_drained_after_terminal` | `tests/broca_subprocess.rs:2158` | unaudited |

These run real processes. The fixture child is the test binary re-executed
with `EIDNARA_BROCA_FIXTURE_MODE` (`:45`); `grandchild_hang` (`:654-656`)
forks a grandchild and `hang_ignore_term` (`:653`) ignores SIGTERM. The
production mechanisms are `process_group(0)` (`src/broca/subprocess.rs:324`),
`set_parent_process_death_signal(KILL)` in `pre_exec` (`:344`), and
`kill_process_group` (`:579`, `:1521`). `:2519` asserts the grandchild saw
SIGTERM through a marker file (`:2547-2550`) and that both pids are gone
(`:2544-2545`).

### broca-child-environment-carries-only-the-provider-row

| Check | Site | Status |
| --- | --- | --- |
| Named `env_snapshot_strips_launch_identity` | `tests/broca_subprocess.rs:2800` | unaudited |
| Named `env_snapshot_admission_charges_per_entry_overhead` | `tests/broca_subprocess.rs:2815` | unaudited |
| Named `provider_rows_exclude_ambient_credentials_and_enforce_caps` | `tests/broca_subprocess.rs:2840` | unaudited |
| Named `credential_snapshot_must_match_before_backend_spawn` | `tests/broca_protocol.rs:435` | unaudited |
| Also bears `opencode_argv_env_stdin_contract` | `tests/broca_subprocess.rs:1164` | unaudited |
| Also bears `pi_argv_privacy_contract` | `tests/broca_subprocess.rs:1360` | unaudited |
| Also bears `closed_dispatch_sink_prevents_spawn` | `tests/broca_subprocess.rs:1270` | unaudited |
| Also bears `opencode_oversized_inline_config_rejected_before_spawn` | `tests/broca_subprocess.rs:1918` | unaudited |
| Also bears `output_flood_stopped_and_redacted` | `tests/broca_subprocess.rs:2086` | unaudited |

### broca-protocol-shapes-are-closed

| Check | Site | Status |
| --- | --- | --- |
| Named `each_valid_operation_decodes_its_exact_schema` | `tests/broca_protocol.rs:41` | unaudited |
| Named `every_malformed_shape_is_rejected_with_schema_violation` | `tests/broca_protocol.rs:127` | unaudited |
| Named `the_512kib_boundary_admits_exactly_and_rejects_one_byte_over` | `tests/broca_protocol.rs:323` | unaudited |
| Named `malformed_requests_over_the_host_create_no_run_state` | `tests/broca_protocol.rs:674` | unaudited |
| Named `harness_vocabulary_is_closed` | `tests/broca_protocol.rs:411` | unaudited |
| Also bears `bind_requires_absolute_root_nonempty_session_and_supported_harness` | `tests/broca_protocol.rs:372` | unaudited |
| Also bears `error_unit_stays_within_terminal_headroom_after_json_escaping` | `tests/broca_protocol.rs:344` | unaudited |
| Also bears `five_operation_round_trip_matches_the_consumed_wire_shapes` | `tests/broca_protocol.rs:499` | unaudited |

### synapse-admission-boundaries-are-exact

| Check | Site | Status |
| --- | --- | --- |
| Named `admission_count_boundary_is_exact_and_never_evicts_live_work` | `tests/synapse_jobs.rs:41` | unaudited |
| Named `queued_byte_boundary_is_exact_and_releases_on_completion` | `tests/synapse_jobs.rs:131` | unaudited |
| Named `completed_jobs_evict_oldest_first_under_count_pressure` | `tests/synapse_jobs.rs:181` | unaudited |
| Named `expired_jobs_return_module_restarted` | `tests/synapse_jobs.rs:239` | unaudited |
| Also bears `a_charged_job_transfers_shrinks_and_releases_exact_permits` | `src/synapse/jobs.rs:722` | unaudited |
| Also bears `non_admitted_outcomes_leave_the_candidate_charge_with_the_caller` | `src/synapse/jobs.rs:765` | unaudited |
| Also bears `failure_eviction_and_expiry_release_their_charges` | `src/synapse/jobs.rs:827` | unaudited |
| Also bears `sweep_releases_expired_charges_without_a_request_path` | `src/synapse/jobs.rs:880` | unaudited |
| Also bears `result_byte_boundary_keeps_accepted_job_and_rejects_oversize_before_start` | `src/synapse/jobs.rs:1004` | unaudited |
| Also bears `bounded_query_waiters_are_fifo_and_reject_bound_plus_one` | `tests/synapse_protocol.rs:67` | unaudited |
| Also bears `expired_waiter_releases_its_slot_without_engine_work` | `tests/synapse_protocol.rs:112` | unaudited |
| Also bears `waiter_boundary_is_the_last_feasible_startup_configuration` | `tests/synapse_protocol.rs:388` | unaudited |
| Ignored `boundary_waiters_with_maximal_texts_are_all_admitted` | `tests/synapse_protocol.rs:415` | unaudited, never executed |

`:239` produces expiry with a real 250 ms sleep (`:266`) against a 100 ms
retention (`:242`); it is the one timing-dependent check on this record and
does not use the paused clock. The `#[ignore]` reason at `:412-414` is the
eight-ring admission cap the catalog's open question cites.

### synapse-degrades-to-disabled-and-keeps-the-context-routable

| Check | Site | Status |
| --- | --- | --- |
| Named `unconfigured_component_is_disabled_not_fatal` | `tests/synapse_bundle.rs:226` | unaudited |
| Named `one_bit_changes_to_each_artifact_disable_the_lane` | `tests/synapse_bundle.rs:282` | unaudited |
| Named `missing_artifact_disables_the_lane` | `tests/synapse_bundle.rs:307` | unaudited |
| Named `wrong_ort_identity_disables_the_lane` | `tests/synapse_bundle.rs:708` | unaudited, **returns at `:709` in CI** |
| Named `corrupt_bundle_degrades_synapse_and_keeps_context_routable` | `tests/synapse_roundtrip.rs:57` | unaudited |
| Also bears `unlisted_extra_file_disables_the_lane` | `tests/synapse_bundle.rs:316` | unaudited |
| Also bears `symlinked_artifact_disables_the_lane` | `tests/synapse_bundle.rs:325` | unaudited |
| Also bears `duplicate_manifest_key_disables_the_lane` | `tests/synapse_bundle.rs:340` | unaudited |
| Also bears `a_stale_fingerprint_disables_the_lane` | `tests/synapse_bundle.rs:358` | unaudited |
| Also bears `a_recommended_batch_above_the_admission_cap_disables_the_lane` | `tests/synapse_bundle.rs:420` | unaudited |
| Also bears `retained_result_cap_below_the_manifest_batch_bound_disables_before_ort` | `tests/synapse_bundle.rs:436` | unaudited |
| Also bears `manifest_field_bounds_disable_the_lane` | `tests/synapse_bundle.rs:463` | unaudited |
| Also bears `missing_pad_token_disables_the_lane` | `tests/synapse_bundle.rs:543` | unaudited |
| Also bears `missing_bundle_directory_disables_the_lane` | `tests/synapse_bundle.rs:559` | unaudited |
| Also bears `host_only_platform_reports_exact_synapse_unsupported_state` | `tests/synapse_bundle.rs:45` | unaudited |
| Counter-case `incoherent_host_serving_limits_fail_startup_before_ort` | `tests/synapse_bundle.rs:451` | unaudited |
| Also bears, ORT-gated, `wrong_but_dimension_compatible_output_fails_certification` | `tests/synapse_bundle.rs:686` | unaudited, returns at `:687` in CI |
| Also bears, ORT-gated, `wrong_pooling_fails_certification` | `tests/synapse_bundle.rs:698` | unaudited, returns at `:699` in CI |

`:451` is listed because it is the boundary of the guarantee: infeasible host
serving limits fail startup rather than disable the lane, per the comment at
`:117`. The record's "never host-fatal" clause is scoped to artifact faults,
and the test that shows where the scope ends belongs beside the ones that
show where it holds. `:57` asserts `artifact_invalid` on the Synapse bind
(`synapse_roundtrip.rs:92-93`) and a successful `context` bind and ping
afterwards (`:97-114`), inside one scenario.

### synapse-requests-are-validated-before-any-inference

| Check | Site | Status |
| --- | --- | --- |
| Named `embed_query_rejects_every_constraint_violation` | `tests/synapse_protocol.rs:641` | unaudited |
| Named `embed_batch_validation_creates_no_job_and_no_inference` | `tests/synapse_protocol.rs:820` | unaudited |
| Named `an_unknown_top_level_field_is_rejected_without_reading_its_value` | `tests/synapse_protocol.rs:1270` | unaudited |
| Named `a_routed_depth_nine_request_is_a_schema_violation` | `tests/synapse_protocol.rs:1302` | unaudited |
| Named `equal_replays_reuse_one_job_and_one_inference` | `tests/synapse_protocol.rs:941` | unaudited |
| Also bears `batch_result_over_retention_cap_is_rejected_before_inference` | `tests/synapse_protocol.rs:796` | unaudited |
| Also bears `exact_boundary_batches_are_accepted` | `tests/synapse_protocol.rs:905` | unaudited |
| Also bears `unknown_and_foreign_jobs_are_module_restarted` | `tests/synapse_protocol.rs:1218` | unaudited |
| Also bears `wrong_request_key_for_a_live_job_is_a_schema_violation` | `tests/synapse_protocol.rs:1237` | unaudited |
| Also bears `a_body_above_resident_capacity_is_a_permanent_size_violation` | `tests/synapse_protocol.rs:1345` | unaudited |
| Also bears `request_key_matches_the_javascript_golden_vectors` | `src/synapse/protocol.rs:1003` | unaudited |
| Also bears `depth_eight_passes_and_depth_nine_fails` | `src/synapse/protocol.rs:1035` | unaudited |
| Also bears `depth_counts_params_that_precede_method` | `src/synapse/protocol.rs:1050` | unaudited |
| Also bears `delimiters_inside_strings_never_count_toward_depth` | `src/synapse/protocol.rs:1062` | unaudited |
| Also bears `a_scalar_at_the_container_limit_is_one_level_deeper` | `src/synapse/protocol.rs:1078` | unaudited |
| Also bears `the_item_after_the_bound_is_refused_before_its_fields_are_read` | `src/synapse/protocol.rs:1120` | unaudited |
| Also bears `the_seeded_batch_path_keeps_strict_schema_behavior` | `src/synapse/protocol.rs:1157` | unaudited |
| Also bears `an_identical_retry_replaces_a_failed_job` | `src/synapse/jobs.rs:910` | unaudited |

The oracle for "before any inference" is `DeterministicEngine::calls`
(`tests/support/synapse.rs:43`, incremented at `:101`). Every named check
reads it.

### synapse-inference-runs-through-a-sealed-runtime-image

| Check | Site | Status |
| --- | --- | --- |
| Named `source_replacement_cannot_change_verified_loader_bytes` | `src/synapse/inference.rs:343` | unaudited |
| Also bears `oversized_sparse_ort_library_fails_before_reading_or_allocating_its_length` | `src/synapse/inference.rs:388` | unaudited |
| Also bears, ORT-gated, `certified_bundle_loads_and_serves_expected_vectors` | `tests/synapse_bundle.rs:572` | unaudited, returns at `:573` in CI |
| Also bears, ORT-gated, `all_four_operations_serve_certified_vectors_over_the_wire` | `tests/synapse_roundtrip.rs:120` | unaudited, returns at `:121` in CI |

**Contract-versus-check note.** The guarantee names the memfd
`host-onnxruntime`. Production creates it under that name at
`inference.rs:134` and `:138`. `:343` asserts the four seals (`:356-361`), a
rejected write (`:363`), a `/proc/self/fd` load path (`:370`), and the digest
(`:372-377`); it does not assert the name. The "full load into ONNX Runtime"
the record's `Exercised:` line scopes to "where the runtime library is
present" is exactly the ORT-gated set, which never asserts in CI.

## Checks in scope that bear on no U3 record

Listed so a later pass does not count them as unmapped. Each is owned by an
earlier inventory or is out of this catalog's subject.

| Site | Tests | Owner |
| --- | --- | --- |
| `tests/protocol_vectors.rs:230`, `:256`, `:362`, `:515`, `:568`, `:584`, `:642`, `:712`, `:737` | 9 | setup-identity (auth admission), ring-datapath (frames), request-path (catalog order) |
| `src/wire.rs:641`, `:660`, `:712`, `:762`, `:801`, `:829`, `:853`, `:871`, `:903`, `:919` | 10 | ring-datapath, `../ring-datapath/existing-checks.md:96` |
| `src/auth.rs:590`, `:620`, `:722`, `:770`, `:854`, `:918`, `:964`, `:1059`, `:1067` | 9 | setup-identity, `../setup-identity/existing-checks.md:81` |
| `tests/harness_closure.rs:158`, `:185`, `:258`, `:360`, `:386`, `:499`, `:549`, `:592`, `:633`, `:685` | 10 | runtime-config's closure-store record; not the digest record |
| `tests/harness_closure.rs:443` | 1 | `#[ignore]`, release qualification only |
| `tests/broca_subprocess.rs`: the Pi and OpenCode transcript, alias, retry, flood, private-dir, and cleanup contracts at `:1292`, `:1506`, `:1545`, `:1599`, `:1675`, `:1781`, `:1885`, `:1960`, `:2203`, `:2242`, `:2278`, `:2337`, `:2365`, `:2405`, `:2452`, `:2491`, `:2669`, `:2702`, `:2724`, `:2898`, `:2940`, `:2978`, `:3029` | 23 | no record. Harness-behaviour contracts the U3 records do not state |
| `tests/broca_supervisor.rs:206`, `:338`, `:515`, `:544`, `:604`, `:645`, `:685`, `:730`, `:770`, `:799` | 10 | no record. Status, replay, cancel, delete, and teardown-proof contracts |
| `tests/synapse_bundle.rs:151`, `:639`, `:757`, `:818` | 4 | no record. `:757` and `:818` are activation-drop contracts; `:639` is doubly env-gated |
| `tests/synapse_jobs.rs:92`, `:278`, `:317`, `:370`, `:417`, `:480`, `:540` | 7 | no record. Retry delay, route loss, cancel, deadline, error, shutdown, shape |
| `tests/synapse_protocol.rs:186`, `:236`, `:312`, `:471`, `:512`, `:545`, `:737`, `:999`, `:1112`, `:1192` | 10 | no record. Waiter fairness, shutdown drain, pages, reservations |
| `src/synapse/bundle.rs:819`, `:834`, `:926`, `:948`, `:970`, `:991`, `:1014`, `:1023`, `:1028` | 9 | no record. Limit feasibility and read bounds |
| `src/synapse/jobs.rs:971` | 1 | no record. Allocation sharing |
| `src/synapse/protocol.rs:1214`, `:1247`, `:1263`, `:1287`, `:1306`, `:1327` | 6 | no record. Reservation arithmetic; `:1263` is the `SCRATCH_RESERVED_BYTES` coupling `../runtime-config/existing-checks.md:313-320` describes |

That is 109 of the 237. The remaining 128 appear in a record block above; a
check can appear in more than one block (`one_bit_changes_to_each_artifact_
disable_the_lane`, `provider_rows_exclude_ambient_credentials_and_enforce_
caps`), so the block rows sum to more than 127.

## Doctests

**None found.** The eighteen source files in scope contain one fence,
`src/wire.rs:4-14`, and it is ```` ```text ````, which `ci.yml:122` does not
compile. No `compile_fail`, no runnable example, in any Broca, Synapse,
closure, auth, wire, instance, or lifecycle file.

## Production assertions and guards, clustered

Counts are over the production half of each file. **`assert!`, `assert_eq!`,
`assert_ne!`, `.unwrap()`, `panic!`, `todo!`, `unimplemented!`: none found**
in production code across all eighteen files. Enforcement is by returned
`Result` and typed error, with the exceptions below.

**`.expect(`: 42.** Four clusters.

| Cluster | Sites |
| --- | --- |
| Infallible serialization | `src/broca/protocol.rs:250`, `:255`, `:259`, `:264`; `src/broca/opencode.rs:105`; `src/synapse/jobs.rs:148`, `:151`; `src/synapse/protocol.rs:799`, `:843`, `:960`, `:972`; `src/lifecycle.rs:370`; `src/instance.rs:326` |
| Lock and latch invariants | `src/synapse/mod.rs:301`, `:314`, `:336`, `:344`, `:994`, `:1046` (`"synapse state lock"`); `src/lifecycle.rs:1066`, `:1078`, `:1087` (`"latch lock"`) |
| Validated-above contracts | `src/broca/supervisor.rs:919`; `src/synapse/bundle.rs:231`, `:242`, `:354`, `:356`; `src/synapse/jobs.rs:327`; `src/synapse/mod.rs:280`; `src/harness_closure.rs:276`, `:441`; `src/lifecycle.rs:63`, `:84` |
| OS and library contracts | `src/broca/subprocess.rs:187`, `:191` (`"HMAC accepts any key length"`, inside the credential fingerprint), `:412`, `:413`, `:768`, `:795`; `src/broca/supervisor.rs:259`; `src/synapse/jobs.rs:249`; `src/wire.rs:403` |

`subprocess.rs:187` and `:191` sit on the fingerprint path the credential
record pins; a key-length failure there is a panic, not a rejected row. Status
unaudited.

**`debug_assert!`: 4.** `src/broca/supervisor.rs:641` (`"every run is
session-owned"`), `src/synapse/jobs.rs:571`, `src/synapse/mod.rs:419`,
`src/instance.rs:563`. Whether the release profile enables `debug-assertions`
was not read; it decides whether these four exist in production.

**`unreachable!`: 1.** `src/synapse/mod.rs:327`, `"ready lanes embed"`, in
the disabled-lane bind path the degrade record covers. Status unaudited.

**`unsafe`: 1.** `src/broca/subprocess.rs:341`, the `pre_exec` hook the
reaping record depends on, with its safety comment at `:339`.

**`let _ =` discarded results: 20.** `src/broca/subprocess.rs` 5 (including
`:381`, `:677`, `:682`, each `child.start_kill()`), `src/synapse/mod.rs` 5,
`src/instance.rs` 3, `src/harness_closure.rs` 2, `src/lifecycle.rs` 2,
`src/broca/mod.rs` 1, `src/synapse/protocol.rs` 1, `src/auth.rs` 1. The three
`start_kill` discards are on the reaping path; the comment at `:380` states
the reason. Status unaudited.

**Checked and saturating arithmetic: 31 `checked_`, 53 `saturating_`.**
`src/synapse/bundle.rs` and `src/synapse/jobs.rs` carry 7 and 12 `checked_`;
`src/broca/supervisor.rs` and `src/synapse/bundle.rs` carry 13 `saturating_`
each. No inventory of which saturations are load-bearing was made.

**Typed rejection guards.** `HarnessClosureError` and its caps are described
at `../runtime-config/existing-checks.md:200-206`. The Broca per-value
credential cap is `src/broca/subprocess.rs:161`. The Synapse depth and size
preflight lives in `src/synapse/protocol.rs` and is pinned by the inline tests
at `:1035-1120`.

## Explicit "none found"

- No `should_panic` in any file in scope.
- No `proptest`, `quickcheck`, `arbitrary`, `loom`, `shuttle`, or `miri` in
  any file in scope. `ci.yml:133-137` checks the `shm-transport` fuzz
  workspace, which names no `host-runtime` target.
- No coverage instrumentation, so every placement statement is structural.
- No snapshot or golden fixture other than the three committed vectors
  (`pi-valid.json`, `synapse-tiny`, and the literals in
  `protocol_vectors.rs` and `auth.rs`).
- No differential harness against the TypeScript twin the manifest-digest
  record names; the record says it lands in U7.
- No test in scope drives `data_dir_path`'s environment-reading arm
  (`instance.rs:138-141`).
- No test asserts the memfd name `host-onnxruntime`.
- No test asserts `lifetime.lock`'s literal path or inode identity.

## Suspiciously quiet areas

Six, ranked by the gap between what the code decides and what any check
proves.

1. **The ONNX Runtime path asserts nothing in CI.** Six tests return before
   their first assertion when `EIDNARA_SYNAPSE_TEST_ORT_LIBRARY` is unset
   (`tests/synapse_bundle.rs:573`, `:645`, `:687`, `:699`, `:709`;
   `tests/synapse_roundtrip.rs:121`), and `ci.yml` never sets it. So the full
   certified load, wrong-output and wrong-pooling certification failures,
   wrong-ORT-identity disable, and the over-the-wire vector serve all pass
   green without running. The sealed-image record's one CI-executed check
   (`src/synapse/inference.rs:343`) stages 28 bytes of fake library text
   (`:347`), which proves the memfd mechanics and nothing about loading a
   real library through it. Owned by
   [synapse-inference-runs-through-a-sealed-runtime-image](../catalog.md#synapse-inference-runs-through-a-sealed-runtime-image)
   and the `wrong_ort_identity_disables_the_lane` clause of
   [synapse-degrades-to-disabled-and-keeps-the-context-routable](../catalog.md#synapse-degrades-to-disabled-and-keeps-the-context-routable).

2. **Seven Broca source files, 3,279 lines, have no inline test, and the
   supervisor is one of them.** `src/broca/supervisor.rs` (1,166 lines) holds
   the dedup index, permit accounting, and terminal state the two Broca
   supervisor records assert, and its entire coverage is
   `tests/broca_supervisor.rs`. `src/broca/protocol.rs` (347) is covered only
   by `tests/broca_protocol.rs`. The one inline Broca test is the fingerprint
   vector at `subprocess.rs:1660`. This is a structural fact rather than a
   defect; recorded because an integration-only position means every Broca
   invariant is observed from outside the supervisor's lock.

3. **The proof-construction record's two sides meet through a literal, not a
   call.** `auth.rs:641` and `protocol_vectors.rs:33` each pin their own
   function to a committed value; `proof_folds_every_input` perturbs only the
   oracle. A transcript change applied to `compute_proof` alone fails
   `auth.rs:641`; a transcript change applied to both sides passes both vector
   tests and is caught only if a regenerated vector is not committed. The
   record's `Fault/timing angle:` says exactly this ("only an external oracle
   detects a transcript change both sides apply"), so the quiet area is the
   absence of a check that runs the two functions side by side. Owned by
   [host-proof-construction-matches-the-committed-vectors](../catalog.md#host-proof-construction-matches-the-committed-vectors).

4. **`CREDENTIAL_ROW_CAP_BYTES` is documented, exported, and read by
   nothing.** `src/broca/subprocess.rs:49-51` documents it as the combined
   admitted-set cap and ties it to a contract field; grep across `src/` and
   `tests/` returns only the definition. The record's open question is
   confirmed as a code fact at this `HEAD`. Owned by
   [credential-fingerprint-derives-from-the-product-domain](../catalog.md#credential-fingerprint-derives-from-the-product-domain).

5. **The committed header bytes never reach the host decoder.**
   `protocol_vectors.rs:160-194` decodes and re-encodes the two committed
   headers with the test-local `raw_client` codec. `wire.rs:670` pins the
   host's layout by offset. No check applies `wire.rs`'s decoder to the
   committed vector, and no check sends the 167-byte body to a host. Owned by
   [canonical-route-open-declares-its-exact-body-length](../catalog.md#canonical-route-open-declares-its-exact-body-length).

6. **Three `Exercised: partial` gaps are named by their records and have no
   test.** A resend after a terminal was retained then evicted
   ([broca-identical-resends-converge-on-one-run](../catalog.md#broca-identical-resends-converge-on-one-run));
   a backend that never exits, covered "only through the escalation timers"
   ([broca-permits-and-charges-return-to-baseline](../catalog.md#broca-permits-and-charges-return-to-baseline));
   and a fault during inference itself
   ([synapse-degrades-to-disabled-and-keeps-the-context-routable](../catalog.md#synapse-degrades-to-disabled-and-keeps-the-context-routable)).
   The fixtures for the first two exist (`tests/broca_supervisor.rs:857`,
   `tests/broca_subprocess.rs:2553`); the third has `DeterministicEngine::
   fail_next` (`tests/support/synapse.rs:67`) and no test that fails an
   inference and then asserts context routability.

## Sampling limits on this inventory

- Test counts are attribute counts plus the `harness = false` runner table,
  not execution counts. No test was run for this inventory.
- The record blocks were built by reading each test's name, doc comment, and
  assertion lines. Bodies were read in full for the named checks and for the
  contract-versus-check notes; "also bears" rows rest on names and doc
  comments unless a line is cited.
- The `lifecycle.rs` selection rule (constructs `LifecycleTransactionLock`,
  names `.eidnara-coordination`, or asserts the lifetime fence across a
  replacement) was applied by grepping each test body. A test that reaches a
  coordination lock only through `InstanceGuard::acquire` without naming it
  would be missed.
- Production guard counts are regex counts over the production half of each
  file. `.expect(` labels were printed; `checked_` and `saturating_` sites
  were counted, not read.
- Whether the release profile enables `debug-assertions` was not read.
- `tests/fixtures/generate-synapse-tiny.py` was not read. Its role as the
  fingerprint oracle is taken from the record and from the assertion message
  at `tests/synapse_bundle.rs:387`.

## Open questions

- Is a test that returns early on a missing environment variable
  `Exercised: partial` or `Exercised: not yet` for the clause it would have
  asserted? Six tests here pass in CI without asserting, and the records that
  name them say `partial`. (needs human input)
- Should the two `#[ignore]` tests (`tests/synapse_protocol.rs:415`,
  `tests/harness_closure.rs:443`) count toward a record's `Existing check:`?
  The admission record names the first in an open question only; no record
  names the second. (needs human input)
- Should `CREDENTIAL_ROW_CAP_BYTES` be enforced, deleted, or re-documented as
  a contract-side figure with no host enforcement? (needs human input)
- Should the earlier six inventories' "runs in no CI job" statements be
  corrected in place, or left as a record of the workflow they read? This
  inventory touched none of them. (needs human input)
