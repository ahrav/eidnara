# Discovered-at-U3 fault-to-property map

For each of the 16 records under `## Discovered at U3` in
[catalog.md](../catalog.md) (`:9875-10120`), what must actually occur for a
test to be non-vacuous, and whether the harness can produce it today.

Same rules as the earlier parts. Safety checks must hold *while* their faults
are active. Liveness checks need a bounded fault-free window, stated in the
units the code bounds; this set has one liveness record and its bound is
stated in its row. Rare implementation branches need deterministic injection
to be reachable at all. Coverage checks assert independent preconditions,
never the violation.

Provenance: branch `u3/16-catalog-host-runtime`, `HEAD` = `572315a`. Every
fixture named below was located and its line printed; the check inventory it
rests on is [existing-checks.md](existing-checks.md).

Four framing points specific to this record set.

**First, seven of the sixteen records need no fault at all, because their
subject is a committed vector.** The proof, header, closure-digest,
credential-fingerprint, and bundle-fingerprint records are each discharged by
comparing a function's output over fixed inputs to a literal, plus a
test-local or out-of-tree oracle. The data-root record is a pure function of
two values. The coordination-lock record needs one filesystem rename. None of
these needs a timing window, a concurrent actor, or a failing dependency, and
every one of them has a fixture in the tree.

**Second, the fixtures that make the other nine non-vacuous already exist, in
three families, and the map below is mostly a matter of naming which family
each record uses.** `ScriptedBackend` (`tests/support/broca.rs:25`) with its
`completing`, `failing`, `gated`, `gated_ignoring_cancel`, and `flooding`
constructors (`:57`, `:71`, `:79`, `:117`, `:136`) is the in-process Broca
backend. The `harness = false` binary re-executes itself as the harness child
(`tests/broca_subprocess.rs:61-72`) with behaviours selected by environment at
`:636-657`, so real processes, real process groups, and real signals are
available. `DeterministicEngine` (`tests/support/synapse.rs:40`) with `calls`
(`:43`), `set_delay` (`:63`), `fail_next` (`:67`), and `block_calls` (`:71`) is
the Synapse engine, and `expect_disabled_with` (`tests/synapse_bundle.rs:105`)
mutates a copy of the `synapse-tiny` fixture before load.

**Third, the one capability that is genuinely absent in CI is an ONNX Runtime
library, and it gates the load-bearing half of two records.** Six tests return
before asserting when `EIDNARA_SYNAPSE_TEST_ORT_LIBRARY` is unset
(`tests/synapse_bundle.rs:573`, `:645`, `:687`, `:699`, `:709`;
`tests/synapse_roundtrip.rs:121`), and `.github/workflows/ci.yml` never sets
it. The sealed-image record's real-library clause and the degrade record's
wrong-ORT-identity clause are constructible only on a developer machine with
the library present. Nothing in this tree ships one.

**Fourth, reachability rests on a caller outside this tree.** All sixteen
records are labelled `default-production`. At this `HEAD` the only non-test
callers of `host_runtime::run` are `examples/synapse_host.rs:137`,
`examples/perf_host.rs:100`, `examples/synapse_perf.rs:385`, and
`benches/ipc_budget.rs:111`; no `daemon` crate is a workspace member
(`Cargo.toml:3-11`), and `migration/waves/U3/property-impact.json`, which the
catalog preamble at `:9877` cites, is absent at this `HEAD`. The example at
`synapse_host.rs:116-123` constructs `SynapseComponent::new(None)` when the
bundle argument is `-`. This map does not relabel any record; it records that
the label cannot be checked against a production caller in this checkout and
raises it in the open questions.

## Fault classes required

`F0` is listed first because it is the cheapest capability and it is not a
fault. `F1` through `F3` are inputs rather than faults; the rows say so.

| Class | Description | Available today |
| --- | --- | --- |
| **F0** test execution in CI | A workflow job that builds and runs the checks a record's oracle lives in | **Yes.** `ci.yml:118` and `:126` run `cargo test --workspace --all-targets --all-features --locked` under two toolchains; `:122` runs `--doc`. Every binary and inline module in scope executes, twice, on `ubuntu-latest` (`:14`). Two `#[ignore]` tests and six ORT-gated early returns are the whole exception list; see `F8` |
| **F1** a committed vector and a test-local oracle | Fixed inputs, a committed output literal, and an implementation of the same derivation that does not call the crate | **Yes, for four of five vector records.** `raw_client::proof` (`tests/support/raw_client.rs:251-269`) is an HMAC transcript written against the protocol text; `raw_client::header` and `decode_header` (`:271-299`) are a hand-written codec; `vector_inputs()` supplies the proof inputs. The closure-digest oracle is a Python `json.dumps(sort_keys=True, indent=2)` run recorded in the evidence file and not in the tree. The bundle-fingerprint oracle is `tests/fixtures/generate-synapse-tiny.py`, in the tree but run by hand. The credential-fingerprint oracle is a Python run recorded in the evidence file only |
| **F2** a single-field perturbation of a vector input | One input byte changed, so the output must change | **Partial.** `proof_folds_every_input` (`tests/protocol_vectors.rs:75`) perturbs key, both nonces, and daemon id, but against `raw_client::proof`, not `compute_proof`. `ordered_extensions_are_part_of_manifest_identity` (`tests/harness_closure.rs:408`) perturbs one manifest field. `one_bit_changes_to_each_artifact_disable_the_lane` (`tests/synapse_bundle.rs:282`) perturbs each of seven artifacts. `credential_fingerprint_matches_the_committed_vector` (`src/broca/subprocess.rs:1660`) perturbs the key only (`:1674-1679`). No perturbation of the host proof, the credential row fields, or the header fields exists |
| **F3** environment value shapes | Relative, empty, and absent `XDG_DATA_HOME` and `HOME` | **Yes, and it is a pure function.** `default_data_root(DataRootEnv)` (`src/instance.rs:155`) takes the two values as arguments; `default_root_follows_xdg_then_home` (`:867`) drives all five shapes (`:878-903`). The production arm that reads the process environment (`:138-141`) is not driven, by design |
| **F4** managed-subtree replacement under a held lock | `<root>/eidnara` renamed away while a coordination lock is held, then a second independent opener | **Yes.** `independent_openers_see_one_stable_coordination_identity` (`src/lifecycle.rs:2001`) renames at `:2018` and reopens at `:2020-2021`; `a_replaced_lifecycle_child_cannot_mint_a_second_transaction_owner` (`:1981`) renames the lifecycle child at `:1988`; `a_replaced_eidnara_subtree_is_not_reported_stopped_while_the_daemon_lives` (`:2032`) renames at `:2043` under a live `InstanceGuard`. All three use `temp_root()` and `std::fs::rename` |
| **F5** a hostile or reordered closure manifest | An unknown field, a reordered array, a hash mismatch, a traversal or symlink source | **Yes.** `setup()` in `tests/harness_closure.rs` builds a candidate; `:418` inserts an unknown field, `:408` reverses `extensions`, `:325` mismatches hashes, `:360` plants traversal and symlink sources. Reordering JSON object keys on input is not built, and cannot reach the digest by construction (`src/harness_closure.rs:254-259`) |
| **F6** an environment with several provider credentials and the launch identity | `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, ambient `AWS_*`, `HTTPS_PROXY`, `PATH`, `LD_PRELOAD`, plus `EIDNARA_MODULE_ID` and `EIDNARA_LAUNCH_NONCE` | **Yes.** `EnvSnapshot::capture_from(vec![...])` (`src/broca/subprocess.rs:97`) takes the environment as a value; `provider_rows_exclude_ambient_credentials_and_enforce_caps` (`tests/broca_subprocess.rs:2840-2848`) builds the six-variable shape and `env_snapshot_strips_launch_identity` (`:2800`) the launch-identity one. The real child's environment is observed by the fixture binary in `opencode_argv_env_stdin_contract` (`:1164`) and `pi_argv_privacy_contract` (`:1360`) |
| **F7** a mutated bundle artifact | A bit flip, a missing file, an unlisted extra, a symlink, a duplicate manifest key, a stale fingerprint, an out-of-bounds field | **Yes.** `expect_disabled_with` (`tests/synapse_bundle.rs:105-115`) copies `synapse-tiny` to a temp dir and applies a closure; `:282`, `:307`, `:316`, `:325`, `:340`, `:358`, `:463`, `:543`, `:559` each supply one. `disabled_reason` reads the component's disabled state. No ORT library is needed for any of these, because the faults are caught at manifest and artifact verification |
| **F8** an ONNX Runtime shared library | A real `libonnxruntime.so` whose digest the bundle certifies, so `verify_ort_library` (`src/synapse/inference.rs:105`) stages and loads it | **No in CI; yes locally.** `ort_library()` (`tests/synapse_bundle.rs:29-42`, `tests/synapse_roundtrip.rs:27-38`) reads `EIDNARA_SYNAPSE_TEST_ORT_LIBRARY` and returns `None` otherwise; six tests return at that point. `ci.yml` does not set the variable (grep `EIDNARA`: zero hits). The `synapse_host` example takes the library path and digest as arguments (`examples/synapse_host.rs:110-114`) |
| **F9** concurrent identical Broca sends; a byte-different resend; a resend after eviction | Two `Supervisor::send` calls with equal bytes released together; a one-space body difference under the same key; a resend after the terminal aged out | **Yes for the first two, state-only for the third.** `racing_identical_sends_converge_on_one_run_and_one_backend_start` (`tests/broca_supervisor.rs:165`) releases through a `std::sync::Barrier` (`:168-175`); `identical_resend_dedups_and_any_byte_difference_conflicts` (`:126`) inserts one space (`:139-145`). `terminal_expiry_and_oldest_eviction_enforce_the_session_caps` (`:857`) produces the retained-then-evicted state under `start_paused`, and no test resends into it |
| **F10** each Broca terminal path | Success, error, cancel, transport detach, shutdown; and a backend that never exits | **Yes in-process; the never-exits case only with real processes.** `ScriptedBackend::completing`, `failing`, `gated`, and `gated_ignoring_cancel` give the first four; `transport_detach_paths_leave_the_run_untouched` (`:1099`) gives request cancel, route close, and connection loss; `host_shutdown_drains_the_supervisor_to_zero_state` (`:1246`) gives shutdown with live work. A backend that never exits is `hang_ignore_term` (`tests/broca_subprocess.rs:653`), used by `sigkill_escalation_when_term_ignored` (`:2553`), which asserts reaping and not permit baseline |
| **F11** a real child that ignores SIGTERM; a forked grandchild; a dead owner with a live group | Process-group reaping with escalation, and the orphan sweep's owner check | **Yes.** `hang_ignore_term` (`:653`, body at `:574`) ignores SIGTERM; `grandchild_hang` (`:654-656`, body at `:594`) forks and hangs; `record_group` (`:297`, body at `:308`) writes a group record and exits so the sweep sees a dead owner, and `group_registry_sweep_kills_only_dead_owner_groups` (`:3045`) also spawns a live recording host to prove the sweep spares it. These are real `fork`, real `kill(-pgid)` (`src/broca/subprocess.rs:579`), and real `PR_SET_PDEATHSIG` (`:344`) |
| **F12** malformed and boundary-sized Broca bodies | Every enumerated malformed shape; a 512 KiB body and one byte more | **Yes.** `every_malformed_shape_is_rejected_with_schema_violation` (`tests/broca_protocol.rs:127`) enumerates shapes over a running host; `the_512kib_boundary_admits_exactly_and_rejects_one_byte_over` (`:323`) does the boundary; `malformed_requests_over_the_host_create_no_run_state` (`:674`) reads supervisor state after rejection |
| **F13** Synapse admission at the count and byte boundary; completion under count pressure; expiry | Boundary and boundary-plus-one requests against a gated engine; retention elapsed | **Yes, with one wall-clock dependency and one ignored shape.** The four named `tests/synapse_jobs.rs` tests use `DeterministicEngine` and small `SynapseLimits`. `expired_jobs_return_module_restarted` (`:239`) sets 100 ms retention (`:242`) and sleeps 250 ms real time (`:266`). The 33-waiter shape is `#[ignore]` (`tests/synapse_protocol.rs:412-415`) because the host admits at most eight rings per process |
| **F14** each Synapse request violation class, and an equal replay | Constraint violation, unknown field, depth nine, oversize body, foreign job, wrong key; then two equal requests | **Yes.** `DeterministicEngine::calls` (`tests/support/synapse.rs:43`, incremented at `:101`) is the oracle every named `tests/synapse_protocol.rs` check reads. The depth preflight has its own inline table at `src/synapse/protocol.rs:1035-1120` |
| **F15** a fault during inference itself | The engine fails one call after the lane is ready, and a context request follows | **Available and unused.** `DeterministicEngine::fail_next` (`tests/support/synapse.rs:67`) exists; `failed_jobs_report_their_stored_error` (`tests/synapse_jobs.rs:417`) uses it for the job-error contract. No test fails an inference and then asserts the context module is still routable, which is the degrade record's stated `Exercised:` gap |
| **F16** a library replaced on disk after certification; a memfd without seals | The source path rewritten after `verify_ort_library` staged it | **Yes with fake bytes; needs `F8` with real ones.** `source_replacement_cannot_change_verified_loader_bytes` (`src/synapse/inference.rs:343`) stages 28 bytes of text (`:347`), asserts the four seals (`:356-361`), rejects a write (`:363`), renames a replacement over the source (`:365-366`), and re-reads through `/proc/self/fd` (`:370-377`). A memfd without seals is not constructible from outside `verify_ort_library`, because the seals are applied inside it (`:154` onward) and the function has one exit |

Three availability caveats cut across the classes.

**`F1`'s oracles are of three different kinds, and only one runs.** The
`raw_client` codec and HMAC are executed by every CI run. The Python
fingerprint generator is in the tree and executed by nobody. The digest and
credential Python oracles exist only as evidence-file transcripts. So "an
oracle outside the crate" means three different things across five records,
and only the proof and header records have an oracle a CI failure can name.

**`F8`'s absence makes six green tests vacuous, and the records that name them
say `partial`.** A reader who counts `wrong_ort_identity_disables_the_lane` as
coverage of the degrade record's wrong-identity clause is counting a `return`
at `tests/synapse_bundle.rs:709`. The same holds for the sealed-image record's
"full load" clause.

**`F9`'s third shape and `F15` are the two places where the state exists, the
fixture exists, and the assertion has not been written.** Both are one test
each.

## Map

All 16 records, in catalog order. **"Non-vacuous today" means a developer can
construct the required state with the current harness.** Under `F0` it also
means the check runs in CI, which is a stronger position than any earlier
part had; the exceptions are marked.

### Committed vectors

| Property | Required faults and enabling state | Non-vacuous today |
| --- | --- | --- |
| host-proof-construction-matches-the-committed-vectors | **`F1` plus `F2`.** The committed inputs (`tests/protocol_vectors.rs` `vector_inputs()`), the test-local HMAC (`raw_client::proof`), and a perturbation of each input applied to **both** `compute_proof` and the oracle | **Partial, and the split is per side.** The literal side is fully built: `src/auth.rs:641` pins `compute_proof`, `protocol_vectors.rs:33` pins `raw_client::proof`, and the two literals agree byte for byte. The equality side exists only through a live handshake at `:221` with no perturbation. `proof_folds_every_input` (`:75`) perturbs the oracle alone, so the record's `Check:` as written, `compute_proof(...) == raw_client::proof(...)` under every single-field perturbation, has no test that calls both functions on the same perturbed input. The fixture cost is one loop over the existing `vector_inputs()` |
| canonical-route-open-declares-its-exact-body-length | **`F1`, no fault.** The 167-byte literal and the committed 21-byte header, decoded by the test-local decoder | **Yes, as the record states it.** `canonical_route_open_body_is_167_bytes` (`:196`) and `committed_header_vectors_decode_to_their_documented_fields` (`:160`) are complete for the check's own text. What is not built is the host-side reading: no test feeds the committed bytes to `wire.rs`'s decoder or sends the 167-byte body through a host. `little_endian_and_frozen_prefix_layout` (`src/wire.rs:670`) pins the host layout by offset, so the two sides agree by inspection, not by a shared assertion |
| harness-closure-manifest-digest-is-canonical | **`F1` plus `F5`.** The committed `pi-valid.json`, its digest literal, and a field change plus a key reordering | **Yes for the literal, partial for the two invariance clauses.** `canonical_manifest_digest_is_pinned` (`tests/harness_closure.rs:429`) is complete. "Changes when any field changes" is tested for `extensions` order only (`:408`). "Unchanged under key reordering" has no test and holds by construction (`src/harness_closure.rs:254-259` sorts the serialized value), so the cheapest honest oracle is a test that reorders input keys and asserts equality, which would pass on the current implementation and fire if canonicalization were removed. The out-of-tree Python oracle cannot fail a CI run |
| credential-fingerprint-derives-from-the-product-domain | **`F1` plus `F6`.** The documented row, the committed literal, and a snapshot with several credentials so the per-value cap and the row selection are exercised | **Yes, with one unresolvable clause.** `credential_fingerprint_matches_the_committed_vector` (`src/broca/subprocess.rs:1660`) pins the literal and one key change; `provider_rows_exclude_ambient_credentials_and_enforce_caps` (`tests/broca_subprocess.rs:2840`) exercises selection and the 16 KiB per-value cap (`subprocess.rs:161`). The record's open question is a code fact: `CREDENTIAL_ROW_CAP_BYTES` (`:51`) has zero readers, so no test can exercise a cap that nothing enforces. That clause is blocked on a product decision, not a fixture |
| synapse-bundle-fingerprint-covers-every-artifact | **`F1` plus `F7`.** The committed `synapse-tiny` manifest and its fingerprint, then one bit flipped in each artifact and a stale fingerprint in the manifest | **Yes, with the independence living outside the run.** `the_committed_fixture_carries_its_canonical_fingerprint` (`tests/synapse_bundle.rs:375`) compares the stored fingerprint with the crate's own `canonical_fingerprint` (`:384-388`); `a_stale_fingerprint_disables_the_lane` (`:358`) and `one_bit_changes_to_each_artifact_disable_the_lane` (`:282`) cover disagreement and artifact change; `fingerprint_binds_initializer_names_to_their_hashes` (`src/synapse/bundle.rs:899`) covers the initializer lines. "Covers every artifact" is asserted by the seven-name list at `:283-291`, not by enumerating the pre-image lines at `bundle.rs:577` onward, so an artifact added to the bundle and omitted from the pre-image is caught only when the generator is re-run by hand |

### Placement and identity on disk

| Property | Required faults and enabling state | Non-vacuous today |
| --- | --- | --- |
| data-root-resolves-under-the-managed-directory | **`F3`, no fault.** Relative, empty, and absent values for both variables, passed as arguments | **Yes, the cheapest record in the set.** `default_root_follows_xdg_then_home` (`src/instance.rs:867`) drives all five shapes; `explicit_override_resolves_canonical_layout` (`:860`) pins `eidnara/run`. The one unbuilt arm is `data_dir_path`'s environment read (`:138-141`), three lines of glue between `var_os` and the pure resolver, which the split was designed to leave untested rather than call `set_var` |
| coordination-locks-live-beside-the-managed-subtree | **`F4`.** A held lock, `<root>/eidnara` renamed away, a second independent opener; then `(dev, ino)` compared | **Yes for `transaction.lock`, behavioural only for `lifetime.lock`.** `independent_openers_see_one_stable_coordination_identity` (`src/lifecycle.rs:2001`) asserts the literal path (`:2004-2007`) and inode identity (`:2013`, `:2026-2030`) for the transaction lock. `a_replaced_eidnara_subtree_is_not_reported_stopped_while_the_daemon_lives` (`:2032`) shows the lifetime fence survives replacement through `InstanceGuard::acquire` failing (`:2056-2062`), which is the guarantee's consequence and not its inode statement. The fixture for the lifetime half is the same `temp_root()` plus rename; the missing piece is a `symlink_metadata` on `lifetime.lock` before and after |

### Broca

| Property | Required faults and enabling state | Non-vacuous today |
| --- | --- | --- |
| broca-identical-resends-converge-on-one-run | **`F9`.** Concurrent identical sends released together; a byte-different resend under the same key; a resend after the run's terminal was retained then evicted | **Yes for two of three shapes.** `:165` and `:126` in `tests/broca_supervisor.rs` build the first two and assert `backend.starts() == 1` (`:160`, `:198-202`). The third is the record's `Exercised:` gap: `:857` produces the evicted state under `start_paused` and stops. One resend after that eviction, asserting either a fresh run or a stated conflict code, is the whole missing oracle. The `always` needs the check stated per key, since `runs_started <= 1` aggregates across keys |
| broca-permits-and-charges-return-to-baseline | **`F10`.** Each terminal path driven to completion, then `assert_baseline`; then shutdown with live work | **Yes in-process; the never-exits case is asserted for reaping, not for baseline.** `every_path_returns_permits_and_charges_to_baseline` (`:973`) asserts baseline at `:1023` and after shutdown at `:1030`; `host_shutdown_drains_the_supervisor_to_zero_state` (`:1246`) and `transport_detach_paths_leave_the_run_untouched` (`:1099`) cover the remaining paths. A backend that never exits reaches the supervisor only through `tests/broca_subprocess.rs:2553`, whose oracle is process death. Composing the two means running `ScriptedBackend::gated_ignoring_cancel` (`tests/support/broca.rs:117`) through cancel and asserting baseline after the escalation timers, which no test does |
| broca-children-are-reaped-as-a-process-group | **`F11`.** A child that ignores SIGTERM; a forked grandchild; a dead owner with a live group and a live owner beside it | **Yes, with real processes.** `cancel_reaps_group_with_sigterm_first` (`:2519`) asserts the grandchild saw SIGTERM (`:2547-2550`) and both pids are gone (`:2544-2545`); `sigkill_escalation_when_term_ignored` (`:2553`) forces the escalation; `supervisor_shutdown_reaps_group` (`:2637`) and `supervisor_delete_reaps_group` (`:2608`) cover the other two terminals; `group_registry_sweep_kills_only_dead_owner_groups` (`:3045`) asserts the sweep kills the orphan (`:3082-3086`) and spares the live host (`:3088-3091`). The second conjunct of the record's `always`, that the sweep never signals a live owner's group, is asserted for one live owner in one run; a campaign that wants it as an invariant needs the sweep run repeatedly beside a long-lived owner |
| broca-child-environment-carries-only-the-provider-row | **`F6`.** An environment carrying several provider credentials, ambient credentials, and the launch identity, then the spawned child's actual environment read back | **Yes, at both layers.** `env_snapshot_strips_launch_identity` (`:2800`), `env_snapshot_admission_charges_per_entry_overhead` (`:2815`), and `:2840` assert on the `EnvSnapshot` value; `opencode_argv_env_stdin_contract` (`:1164`) and `pi_argv_privacy_contract` (`:1360`) have the fixture child capture its own environment (`:348` `capture`) and assert on what a real `exec` received. `credential_snapshot_must_match_before_backend_spawn` (`tests/broca_protocol.rs:435`) covers the fingerprint gate before spawn |
| broca-protocol-shapes-are-closed | **`F12`.** Every enumerated malformed shape; the 512 KiB boundary and one byte over; an unsupported harness name at bind | **Yes.** Five named checks in `tests/broca_protocol.rs` (`:41`, `:127`, `:323`, `:411`, `:674`) plus `bind_requires_absolute_root_nonempty_session_and_supported_harness` (`:372`). "Every malformed shape" is an enumeration inside `:127`; the record's `always` is over the enumerated set, so the oracle is exactly as strong as the list |

### Synapse

| Property | Required faults and enabling state | Non-vacuous today |
| --- | --- | --- |
| synapse-admission-boundaries-are-exact | **`F13`.** Boundary and boundary-plus-one admission against a gated engine; completion under count pressure; retention elapsed | **Yes, with one wall-clock oracle and one ignored shape.** The four named `tests/synapse_jobs.rs` checks (`:41`, `:131`, `:181`, `:239`) are built. `:239`'s expiry is a 250 ms real sleep against 100 ms retention (`:242`, `:266`), so it is the one check on this record a loaded CI runner could make flaky in the passing direction only. The 33-waiter shape (`tests/synapse_protocol.rs:415`) is blocked by the eight-ring cap at the ring layer, not by the job table; the record's open question owns it. Inline charge tests at `src/synapse/jobs.rs:722-1004` cover the permit arithmetic the boundary rests on |
| synapse-degrades-to-disabled-and-keeps-the-context-routable | **`F7` for every artifact fault class, `F8` for wrong ORT identity, `F15` for a fault during inference; then a context request inside the same scenario.** Liveness bound: the context request completes within the `raw_client` frame budget the test supplies | **Yes for artifact faults; vacuous in CI for wrong ORT identity; unbuilt for a fault during inference.** `corrupt_bundle_degrades_synapse_and_keeps_context_routable` (`tests/synapse_roundtrip.rs:57`) is the one scenario that asserts both clauses together (`artifact_invalid` at `:92-93`, context bind and ping at `:97-114`). The other named checks assert the disabled reason only. `wrong_ort_identity_disables_the_lane` (`tests/synapse_bundle.rs:708`) returns at `:709` without the library. The inference-time fault has `fail_next` available and no test. `incoherent_host_serving_limits_fail_startup_before_ort` (`:451`) marks where "never host-fatal" stops applying, and the record's guarantee should be read with that boundary. The bound the liveness clause uses is the test's own frame timeout; no code-level bound is named, and a campaign must state one |
| synapse-requests-are-validated-before-any-inference | **`F14`.** Each violation class sent to a ready lane with a counting engine; two equal requests | **Yes, the strongest position in the set.** `DeterministicEngine::calls` is a direct oracle for "before any inference", and the five named `tests/synapse_protocol.rs` checks (`:641`, `:820`, `:941`, `:1270`, `:1302`) all read it. The depth preflight has an inline table (`src/synapse/protocol.rs:1035-1120`) and the request key has JavaScript golden vectors (`:1003`). Nothing is missing for the check as stated |
| synapse-inference-runs-through-a-sealed-runtime-image | **`F16` for the memfd mechanics, `F8` plus `F16` for a real library.** A source rewritten after staging; a load through `/proc/self/fd`; the digest compared | **Yes for the mechanics, no in CI for the load.** `source_replacement_cannot_change_verified_loader_bytes` (`src/synapse/inference.rs:343`) is complete for seals, rejected write, replacement resistance, and digest, over 28 bytes of text. The memfd name `host-onnxruntime` (`:134`, `:138`) is in the guarantee and asserted nowhere; a `readlink` on the load path would pin it and is one line. The "loaded image" clause, meaning ONNX Runtime actually initialized from the memfd, is `certified_bundle_loads_and_serves_expected_vectors` (`tests/synapse_bundle.rs:572`) and `all_four_operations_serve_certified_vectors_over_the_wire` (`tests/synapse_roundtrip.rs:120`), both of which return without the library |

**Totals: 8 fully non-vacuous today, 7 partial, 1 partial with a clause
blocked on a product decision.** The eight are the header vectors, the data
root, the child environment, the protocol shapes, request validation, and,
counting their named checks as complete for the check text, the bundle
fingerprint, the manifest digest literal, and the credential-fingerprint
literal. The seven partials split three ways: two need one assertion over an
existing fixture (the proof equality, the lifetime-lock inode); three need one
test over an existing fixture (the post-eviction resend, the never-exits
baseline, the inference-time fault); two are gated on `F8` (the wrong-ORT
clause of degrade, the real-load clause of the sealed image). The blocked
clause is `CREDENTIAL_ROW_CAP_BYTES`, which nothing reads.

Note the shape of that eight: none needs a fault in the ordinary sense, and
five need only a value. That is what cataloging vectors, identities, and
closed schemas produces. The Broca and Synapse records that do need faults
have their fixtures; what they lack is one composition each.

## Coverage checks to add

Each asserts a precondition that a **correct** implementation still
satisfies, so it fires without a defect present. Names are constants,
globally unique with the `u3_` prefix (no earlier fault-map uses it; the
existing prefixes are `auth_`, `client_`, `host_`, `instance_`, `managed_`,
`native_`, `req_`, `ring_`, `rt_`, `setup_`), and never constructed
dynamically.

**No record in this set uses `sometimes`, `reachable`, or `unreachable`.**
Fifteen are `always` and one, the degrade record, is `always` with a liveness
type. So no record carries its own marker, and the forbidden
`always(!X)`/`sometimes(X)` pairing cannot arise from the records as written.
It can arise from a campaign author, and the anti-pattern list below names
the five places it would be easiest to write.

| Coverage check | Situation it witnesses | Why it is safe |
| --- | --- | --- |
| `u3_proof_computed_by_host_over_vector_inputs` | `compute_proof` (`src/auth.rs`) ran over `vector_inputs()` with `SERVER_PROOF_DOMAIN` | The ordinary path of `auth.rs:641`. Records that the host side was exercised, independent of what the oracle produced |
| `u3_proof_computed_by_oracle_over_vector_inputs` | `raw_client::proof` ran over the same inputs | The ordinary path of `protocol_vectors.rs:33`. The pair makes the two-sided comparison provable without asserting it matched |
| `u3_proof_input_perturbed_on_both_sides` | One input field was altered and both functions were called on the altered inputs | Legal and currently unbuilt; `:75` perturbs one side. Fires on the first test that closes the proof record's gap |
| `u3_data_root_resolved_from_absolute_xdg` | `default_data_root` returned through the `XDG_DATA_HOME` arm | The ordinary path of `instance.rs:867`'s first assertion (`:878-881`) |
| `u3_data_root_ignored_a_relative_value` | `default_data_root` skipped a non-absolute `XDG_DATA_HOME` or `HOME` | Legal and specified; `:884-890` and `:897-901` produce it. Records the fallback without asserting that a relative path was never joined |
| `u3_data_root_read_from_process_environment` | `data_dir_path` took the `None` arm at `instance.rs:138` | The production path, currently reached by no test. Placing it shows the glue is untested, which is the honest form of that finding |
| `u3_transaction_lock_opened_at_coordination_path` | `LifecycleTransactionLock::acquire_exclusive` created or opened `.eidnara-coordination/transaction.lock` | Every acquisition. Precondition of the inode identity check |
| `u3_lifetime_lock_opened_at_coordination_path` | `LifetimeLock` opened `.eidnara-coordination/lifetime.lock` (`lifecycle.rs:182`) | Every `InstanceGuard::acquire`. The companion marker that makes the lifetime half of the record provable |
| `u3_managed_subtree_renamed_under_a_held_lock` | `<root>/eidnara` was renamed while a coordination lock was held | Legal in a test and produced by `:2001`, `:1981`, `:2032`. Does not assert that the fence held |
| `u3_manifest_digest_computed_over_sorted_json` | `canonical_manifest` (`harness_closure.rs:254`) ran `sort_json` before hashing | Every digest. Records the canonicalization step without asserting two orderings agreed |
| `u3_manifest_digest_matched_committed_fixture` | `manifest_digest(pi-valid.json)` returned the literal | The ordinary path of `tests/harness_closure.rs:429` |
| `u3_credential_row_selected_from_a_multi_provider_snapshot` | `provider_row` chose one row from a snapshot holding at least two provider credentials | Legal, produced by `:2840`. Precondition of "exactly one provider row" without asserting no second row leaked |
| `u3_credential_value_cap_rejected_a_row` | `subprocess.rs:161` returned the over-cap error | The specified rejection path, produced by `:2840`'s cap case. The only observable form of the 16 KiB cap |
| `u3_bundle_fingerprint_recomputed_at_load` | `load_bundle` recomputed `canonical_fingerprint` and compared it with the manifest's | Every load. Precondition of `a_stale_fingerprint_disables_the_lane` without asserting the comparison's outcome |
| `u3_bundle_artifact_digest_verified_at_load` | One artifact's SHA-256 was computed and compared during load | Every load, once per artifact. The precondition `:282` depends on |
| `u3_broca_send_deduplicated_against_a_live_run` | `Supervisor::send` returned an existing run id for an identical key and body | The ordinary dedup path, produced by `:126` and `:165` |
| `u3_broca_terminal_evicted_from_retained_set` | A terminal run was evicted by count or retention pressure | Legal, produced by `:857` under paused time. The precondition of the post-eviction resend gap, without asserting how the resend behaved |
| `u3_broca_permit_released_on_terminal` | A pending or task permit returned to the pool as a run reached a terminal | Every terminal. Independent of whether the totals reached baseline |
| `u3_broca_group_signaled_sigterm` | `kill_process_group(group, TERM)` (`subprocess.rs:579`) ran | Every cancel, delete, or shutdown of a live child. Precondition of the escalation |
| `u3_broca_group_signaled_sigkill_after_grace` | The KILL escalation fired after the TERM grace elapsed | Legal, produced by `:2553`. Does not assert that the group died |
| `u3_broca_sweep_skipped_a_live_owner_group` | `sweep_orphaned_groups` found a group whose owner was alive and left it | Legal, produced by `:3045`'s survivor (`:3088-3091`). The independent half of "kills only dead owners" |
| `u3_broca_child_env_stripped_launch_identity` | `EnvSnapshot` removed `EIDNARA_MODULE_ID` and `EIDNARA_LAUNCH_NONCE` | Every snapshot. Produced by `:2800` |
| `u3_broca_request_rejected_as_schema_violation` | A Broca request was rejected with `schema_violation` before any run entry was created | The specified path, produced by `:127`, `:323`, `:674` |
| `u3_synapse_admission_rejected_at_boundary_plus_one` | A job or waiter was refused because count or bytes were at the bound | The specified path, produced by `tests/synapse_jobs.rs:41`, `:131` and `tests/synapse_protocol.rs:67` |
| `u3_synapse_completed_job_evicted_under_count_pressure` | A completed job left the table because a new admission needed its slot | Legal, produced by `:181`. Independent of "never evicts live work" |
| `u3_synapse_lane_disabled_by_artifact_fault` | `SynapseComponent` entered the disabled state with an artifact reason | Legal and specified, produced by every `expect_disabled_with` call |
| `u3_context_request_served_beside_a_disabled_synapse_lane` | A `context` route bound and answered while the Synapse lane was disabled | Legal, produced by `synapse_roundtrip.rs:57` (`:97-114`). The liveness clause's own witness; it is not the negation of any `always` |
| `u3_synapse_engine_call_counted` | `DeterministicEngine::calls` was incremented (`support/synapse.rs:101`) | Every inference in a test. The pair with `u3_synapse_request_rejected_before_engine` is the validation record's precondition |
| `u3_synapse_request_rejected_before_engine` | A request was rejected with `schema_violation`, `substitution_rejected`, or `module_restarted` | The specified path. Does not assert that `calls` was unchanged |
| `u3_ort_library_staged_into_sealed_memfd` | `verify_ort_library` created the memfd and applied `SHRINK | GROW | WRITE | SEAL` | Every verification, produced by `inference.rs:343`. Independent of whether a replacement was attempted |
| `u3_ort_library_loaded_through_proc_self_fd` | `load_path()` returned a `/proc/self/fd/` path and a `dlopen` used it | The real-library path; fires only with `F8`. Placing it makes the CI vacuity visible as an absent marker |
| `u3_ort_test_skipped_without_library` | `ort_library()` returned `None` and a test returned early | True of six tests in every CI run. This is the honest form of the `F8` finding: a marker that fires on every skip |

**Anti-patterns to avoid in this record set specifically.** Five pairings are
forbidden by METHOD's rule, and each is tempting here because the defect is
easier to name than its precondition.

- Do not pair `always(compute_proof == oracle)` with
  `sometimes(compute_proof != oracle)`. The second can only fire by observing
  the defect. Assert `u3_proof_computed_by_host_over_vector_inputs`,
  `u3_proof_computed_by_oracle_over_vector_inputs`, and
  `u3_proof_input_perturbed_on_both_sides`.
- Do not pair `always(no_second_backend_start)` with
  `sometimes(second_backend_started)`. Assert
  `u3_broca_send_deduplicated_against_a_live_run` and
  `u3_broca_terminal_evicted_from_retained_set`, then write the post-eviction
  resend as an `always` over its own outcome.
- Do not pair `always(permits_at_baseline_after_terminal)` with
  `sometimes(permit_leaked)`. Assert `u3_broca_permit_released_on_terminal`
  per path; the baseline check stays `always` and the marker shows each path
  was walked.
- Do not pair `always(sweep_spares_live_owners)` with
  `sometimes(sweep_killed_a_live_owner)`. Assert
  `u3_broca_sweep_skipped_a_live_owner_group` and
  `u3_broca_group_signaled_sigkill_after_grace` as two legal facts.
- Do not pair `always(engine_calls_unchanged_by_rejection)` with
  `sometimes(engine_called_on_rejected_request)`. Assert
  `u3_synapse_engine_call_counted` and
  `u3_synapse_request_rejected_before_engine`; the `always` compares the
  counter and the markers show both branches were taken.

Two further constraints on marker placement here.

**Place vector markers on the function, not on the test.** A marker inside
`committed_wire_vectors_pin_the_proof_construction` fires when the test runs;
a marker inside `compute_proof` fires on every handshake. The record is about
production behaviour, so the second placement is the one that carries
information across a campaign.

**Do not place any marker in the ORT-gated tests after the early return.** A
marker after `let Some(ort) = ort_library() else { return };` never fires in
CI and reads as "not yet reached" rather than "skipped". Place
`u3_ort_test_skipped_without_library` inside `ort_library()`'s `None` arm
instead, so the skip is counted.

## Ranking, by cheapest valid oracle

Ranked by the cost of the cheapest oracle that yields a valid result, not by
records unblocked per capability.

1. **Two one-line assertions over existing fixtures.** A `readlink` on
   `verified.load_path()` in `src/synapse/inference.rs:343` pins the memfd
   name `host-onnxruntime` the sealed-image record's guarantee states. A
   `symlink_metadata` on `.eidnara-coordination/lifetime.lock` before and
   after the rename in `src/lifecycle.rs:2001` gives the coordination-lock
   record its second inode. Neither needs a new fixture, a new file, or a
   workflow change, and both run in CI on the next push.

2. **One loop over `vector_inputs()` calling both functions.** Extending
   `proof_folds_every_input` (`tests/protocol_vectors.rs:75`) to call
   `compute_proof` beside `raw_client::proof` on each perturbed input turns
   the proof record's `Check:` from a shared literal into the equality it
   states. `compute_proof` is `pub` at `src/auth.rs:119` and re-exported from the
   crate root (`lib.rs:38-42`), so the integration binary calls it directly as
   `host_runtime::compute_proof`; the loop belongs in `protocol_vectors.rs`, and
   no copy of the oracle's HMAC inside `auth.rs` is needed.

3. **Three one-test compositions over existing fixtures.** A resend after
   `terminal_expiry_and_oldest_eviction_enforce_the_session_caps`
   (`tests/broca_supervisor.rs:857`) closes the dedup record's gap. An
   `assert_baseline` after `gated_ignoring_cancel` runs through cancel and
   the escalation timers closes the permit record's never-exits clause.
   `fail_next` followed by a `context` request closes the degrade record's
   inference-time clause. Each is the shape of an existing test with one
   more step.

4. **A key-reordering test for the manifest digest.** Serialize `pi-valid.json`
   with keys reversed, decode, digest, and assert equality with the literal
   at `tests/harness_closure.rs:437`. It passes today and is the only check
   that would fail if `sort_json` were removed from
   `src/harness_closure.rs:257`.

5. **A reader for `CREDENTIAL_ROW_CAP_BYTES`, or its removal.** The
   credential record's open question is now a code fact. Either enforcement
   at `provider_row` with a test beside the per-value cap, or deletion of the
   constant and its comment at `subprocess.rs:49-51`. This is a product
   decision and is ranked here only because the test, once the decision is
   made, is one case in `:2840`.

6. **Feeding the committed header to `wire.rs`.** Decoding the 21 bytes at
   `tests/protocol_vectors.rs:163` with the host decoder, and sending the
   167-byte body through a `raw_client` connection to a running host, joins
   the two sides the route-open record currently pins separately. The
   `raw_client` connection helpers exist; the cost is that `wire.rs`'s decoder
   may not be reachable from an integration binary, which decides whether the
   test lives inline.

7. **`F8` in CI.** Providing an ONNX Runtime library to the workflow un-skips
   six tests and makes the sealed-image and wrong-ORT-identity clauses assert
   in CI. It is ranked low because it is a supply-chain and workflow decision
   with a download or a vendored binary attached, not a test edit. Until it
   is made, `u3_ort_test_skipped_without_library` is the honest oracle: a
   marker that counts the skips.

8. **A production caller in the tree.** All sixteen `default-production`
   labels rest on a `daemon` that is not a workspace member at this `HEAD`.
   Nothing in this list changes that; it is the reason the open question
   below exists.

## Open questions

- Every record in this set is labelled `default-production`, and at this
  `HEAD` the only non-test callers of `host_runtime::run` are examples and a
  bench. Should the labels stand on the strength of the catalog's intended
  caller, or be re-verified when the daemon crate enters the workspace?
  `migration/waves/U3/property-impact.json`, which the catalog preamble
  cites, is absent at this `HEAD`. (needs human input)
- Is `SynapseComponent::new(None)` a production configuration? If a deployed
  host can run without a bundle, the sealed-image and validation records are
  reached only when a bundle is configured, which is the definition of
  `explicit-config-only`. The example at `examples/synapse_host.rs:116-123`
  makes the bundle optional. (needs human input)
- Should the ORT library be supplied to CI, vendored, or left as a
  release-qualification step alongside the two `#[ignore]` tests? Six tests
  and two record clauses turn on the answer. (needs human input)
- Should `CREDENTIAL_ROW_CAP_BYTES` be enforced or removed? (needs human
  input; also raised in `existing-checks.md`)
- What bound does the degrade record's liveness clause use? The test at
  `tests/synapse_roundtrip.rs:57` uses `raw_client`'s frame budget; no code
  constant names a context-request deadline that a disabled Synapse lane
  must respect. (needs human input)
