# broca-child-environment-carries-only-the-provider-row

## Discovery trigger

`HOST_LAUNCH_IDENTITY_VARS` at `subprocess.rs:30-35` carries the reason for
this record: a harness child that inherited `EIDNARA_MODULE_ID` and
`EIDNARA_LAUNCH_NONCE` "could reconnect to the daemon as the supervised
module". The second half of the guarantee is about ambient credentials: an
`AWS_ACCESS_KEY_ID` or `LD_PRELOAD` in the daemon's environment must not reach
a harness the user did not choose. The audit traced what the child actually
receives from snapshot capture through `env_clear` at spawn.

## Evidence trail

All references are at `e16e39e`.

Capture. `EnvSnapshot::from_vars` at `subprocess.rs:122-133` filters the
launch-identity names out of every snapshot regardless of construction path.
`capture_from` at `:97-118` wraps it and charges each entry its string bytes
plus two plus `ENV_ENTRY_OVERHEAD_BYTES` (128, `config.rs:92`), rejecting the
snapshot when the sum exceeds `MAX_ENV_SNAPSHOT_BYTES` (1536 KiB,
`config.rs:87`).

Selection. `provider_row` at `:140-165` maps the canonical provider to one
variable name (`:145-150`), finds that one variable (`:151-157`), rejects an
empty value (`:158-160`), rejects a value over `CREDENTIAL_VALUE_CAP_BYTES`
(16 KiB, `:48`, check at `:161-163`), and returns a one-element `Vec`
(`:164`). Nothing else from the snapshot is returned. The doc at `:139`
states the intended effect: no loader, proxy, cloud-chain, `PATH`, `HOME`, or
unrelated provider variable survives.

Composition. Both adapters start `child_env` from `provider_row` and append
only adapter-owned variables. OpenCode (`opencode.rs:116-121`, `:164-179`)
adds `OPENCODE_DB`, `OPENCODE_CONFIG_CONTENT`,
`OPENCODE_DISABLE_PROJECT_CONFIG`, `EIDNARA_BROCA_CHILD`, and `HOME`. Pi
(`pi.rs:215-220`, `:313-322`) adds `EIDNARA_PI_SUBAGENT`,
`EIDNARA_BROCA_MAX_OUTPUT_TOKENS`, `EIDNARA_BROCA_TEMPERATURE`, and `HOME`.
The `OPENCODE_CONFIG_CONTENT` value is bounded before spawn by
`MAX_OPENCODE_CONFIG_BYTES` (`opencode.rs:124-135`).

Spawn. `run` calls `.env_clear()` at `subprocess.rs:317` and then sets exactly
the entries of `spec.env` at `:357-359`. The daemon's own environment is never
inherited.

Existing checks, verified. Three are unit-level against `EnvSnapshot`:

- `env_snapshot_strips_launch_identity`
  (`tests/broca_subprocess.rs:2800-2812`): both identity names are removed
  and `KEEP_ME` survives.
- `env_snapshot_admission_charges_per_entry_overhead` (`:2815-2838`): 16,384
  one-byte variables are under the cap on string bytes alone (`:2823-2826`)
  yet rejected (`:2827`); 100 ordinary variables admit (`:2833-2837`).
- `provider_rows_exclude_ambient_credentials_and_enforce_caps`
  (`:2840-2893`): from a snapshot holding two credentials plus `AWS`, proxy,
  `PATH`, and `LD_PRELOAD` entries, the OpenCode anthropic row is exactly one
  pair (`:2851-2856`), the Pi `openai-codex` alias selects the OpenAI row
  (`:2857-2862`), `custom` is `provider_unsupported` (`:2863-2869`), a
  16 KiB + 1 value is `credential_value_too_large` (`:2871-2882`), and the
  fingerprint matches the committed vector (`:2884-2892`).

Two more exercise a real spawn. The fixture design is itself evidence: the
fixture binary receives its test controls only through the credential value
(`fixture_snapshot` at `:856-870` encodes them as JSON inside all three
credential variables; `install_fixture_controls` at `:259-284` decodes the
first one present and panics if none arrived). Every fixture test therefore
proves the credential row is the only channel from snapshot to child.

- `opencode_argv_env_stdin_contract` (`:1164-1268`) reads the child's
  `env.json` and asserts the four adapter variables, `ANTHROPIC_API_KEY`
  equal to the sentinel (`:1240-1244`), and the absence of both identity names
  (`:1245-1246`).
- `pi_argv_privacy_contract` (`:1360-1504`) does the same for Pi at
  `:1473-1490`.

`credential_snapshot_must_match_before_backend_spawn`
(`tests/broca_protocol.rs:434-496`) covers the send-time check in
`CredentialVerifier::verify` (`mod.rs:44-69`): a route without a fingerprint
gets `harness_unavailable` and `backend.starts() == 0` (`:456-458`); a route
bound with the correct fingerprint admits and starts one backend (`:469-493`).

## Failure scenario

Two distinct leaks, both closed by `env_clear` plus single-row selection:

1. The daemon runs with `EIDNARA_MODULE_ID` set. A forwarded environment lets
   the harness child open the daemon's socket as the module and issue
   privileged control operations.
2. The daemon runs with both `ANTHROPIC_API_KEY` and `OPENAI_API_KEY`. A
   request names `anthropic`; a forwarded environment hands the OpenAI key to
   the OpenCode child, which could send it to any endpoint it chooses.

## Timing windows and dependencies

None. The environment is fixed before `spawn` at `subprocess.rs:361`, and
`env_clear` makes the daemon's later environment changes irrelevant.

The record's `Check` line says "no entry over the per-value cap". Only the
credential value is capped per-value (`:161`). `OPENCODE_CONFIG_CONTENT` has
its own cap at `opencode.rs:124`; the other adapter values are short constants
or paths. `CREDENTIAL_ROW_CAP_BYTES` at `subprocess.rs:51` is declared but has
no reader in the crate; with a one-variable row and a 16 KiB value cap it
cannot be reached.

## What a test must construct

The fixture tests assert presence of the selected credential but not the
absence of the unselected ones in the spawned child. `pi_argv_privacy_contract`
runs with all three credential names in the snapshot and selects OpenAI; an
assertion that `env.json` lacks `ANTHROPIC_API_KEY` and `GEMINI_API_KEY`
would pin "exactly one provider variable" at the spawn boundary rather than at
the `provider_row` unit boundary. `install_fixture_controls` iterates the
three names in order and returns on the first hit (`:265-282`), so today a
leaked `ANTHROPIC_API_KEY` in the Pi run would silently become the decoded
control channel and the test would still pass.

## Investigation log

### Q: Does the child receive the snapshot, or only the provider row?

- Sources examined: `subprocess.rs:139-165`, `:317`, `:357-359`;
  `opencode.rs:116-121`, `:164-184`; `pi.rs:215-220`, `:313-327`; the record's
  `Guarantee` line.
- Findings: only the provider row plus adapter-owned variables. The record's
  phrase "the admitted snapshot with the launch identity stripped and exactly
  one provider credential row" reads as though the rest of the snapshot is
  forwarded. It is not; the snapshot exists to be selected from, and
  `from_vars` stripping is defence in depth behind `provider_row`. The
  `Check` line ("exactly the selected provider variable") matches the code.
- Missing evidence: none for the mechanism.
- Conclusion: resolved with answer: the child environment is strictly smaller
  than the record's `Guarantee` sentence implies. The record's wording should
  be tightened; the code is the stricter side.

### Q: Is there an in-tree production caller that captures the real environment?

- Sources examined: `rg` for `capture_from`, `new_with_credentials`, and
  `EnvSnapshot` outside `src/broca` and `tests`, and for `BrocaComponent::new`
  outside tests.
- Findings: none. Every `capture_from` and `new_with_credentials` call is in
  a test. `BrocaComponent::new` at `mod.rs:73-80` sets `credential_verifier`
  to `None`, so a component built that way skips the fingerprint check at
  `mod.rs:223-235`.
- Missing evidence: the daemon wiring that constructs the snapshot from the
  process environment and chooses between `new` and `new_with_credentials`.
- Conclusion: needs human input. The record is labelled `default-production`;
  in this checkout the aggregate cap and the verifier are reached only from
  tests, and the wiring that decides the production path is out of tree.
