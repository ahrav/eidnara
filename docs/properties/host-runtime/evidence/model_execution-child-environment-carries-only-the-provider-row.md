# model_execution-child-environment-carries-only-the-provider-row

## Discovery trigger

`HOST_LAUNCH_IDENTITY_VARS` at `subprocess.rs:29-34` carries the reason for
this record: a harness child that inherited `EIDNARA_MODULE_ID` and
`EIDNARA_LAUNCH_NONCE` "could reconnect to the daemon as the supervised
module". The second half of the guarantee is about ambient credentials: an
`AWS_ACCESS_KEY_ID` or `LD_PRELOAD` in the daemon's environment must not reach
a harness the user did not choose. The audit traced what the child actually
receives from snapshot capture through `env_clear` at spawn.

## Evidence trail

All references are at the tree this record was last revised with.

Capture. `EnvSnapshot::from_vars` at `subprocess.rs:183-194` filters the
launch-identity names out of every snapshot regardless of construction path.
`capture_from` at `:158-179` wraps it and charges each entry its string bytes
plus two plus `ENV_ENTRY_OVERHEAD_BYTES` (128, `config.rs:120`), rejecting the
snapshot when the sum exceeds `MAX_ENV_SNAPSHOT_BYTES` (1536 KiB,
`config.rs:116`).

Selection. `provider_row` at `subprocess.rs:201-233` maps the canonical
provider to a row specification (`provider_row_spec`, `:118-149`): one
variable for `anthropic`, `google`, and `openai`, and for `amazon-bedrock` the
access key, the secret key, the region, and an optional session token. It
looks up each named variable (`:208-223`), treats an empty value as absent
(`:216-218`), rejects a value over `CREDENTIAL_VALUE_CAP_BYTES` (16 KiB,
`:47`, check at `:219-221`), fails with `CredentialMissing` when a required
variable is absent (`:229`), and returns the row in specification order
(`:224-232`). Nothing else from the snapshot is returned. The doc at `:200`
states the intended effect: no loader, proxy, cloud-chain, `PATH`, `HOME`, or
unrelated provider variable survives.

Composition. Both adapters start `child_env` from the row
`aws_refresh::child_credentials` returns (`aws_refresh.rs:576-614`):
`provider_row`'s row, or, for `amazon-bedrock` under an AWS profile source, the
leased access key, secret key, session token, and region. They append
only adapter-owned variables. OpenCode (`opencode.rs:255-266`, `:312-329`)
adds `OPENCODE_DB`, `OPENCODE_CONFIG_CONTENT`,
`OPENCODE_DISABLE_PROJECT_CONFIG`, `EIDNARA_MODEL_EXECUTION_CHILD`, and `HOME`. Pi
(`pi.rs:390-401`, `:522-539`) adds `EIDNARA_PI_SUBAGENT`,
`EIDNARA_MODEL_EXECUTION_MAX_OUTPUT_TOKENS`, `EIDNARA_MODEL_EXECUTION_TEMPERATURE`, `HOME`, and, for a request that asks for its system text as written, `EIDNARA_MODEL_EXECUTION_PLAIN_SYSTEM` (`crates/host-runtime/src/model_execution/pi.rs:533-538`, at the commit that adds the variable).
The `OPENCODE_CONFIG_CONTENT` value is bounded before spawn by
`MAX_OPENCODE_CONFIG_BYTES` (`opencode.rs:217-229`).

Spawn. `run` calls `.env_clear()` at `subprocess.rs:391` and then sets exactly
the entries of `spec.env` at `:517-519`. The daemon's own environment is never
inherited.

Existing checks, verified. Three are unit-level against `EnvSnapshot`:

- `env_snapshot_strips_launch_identity`
  (`tests/model_execution_subprocess.rs:4069-4082`): both identity names are removed
  and `KEEP_ME` survives.
- `env_snapshot_admission_charges_per_entry_overhead` (`:4085-4108`): 16,384
  one-byte variables are under the cap on string bytes alone (`:4089-4096`)
  yet rejected (`:4097-4101`); 100 ordinary variables admit (`:4103-4107`).
- `provider_rows_exclude_ambient_credentials_and_enforce_caps`
  (`:4110-4164`): from a snapshot holding two credentials plus `AWS`, proxy,
  `PATH`, and `LD_PRELOAD` entries, the OpenCode anthropic row is exactly one
  pair (`:4121-4126`), the Pi `openai-codex` alias selects the OpenAI row
  (`:4127-4132`), `custom` is `provider_unsupported` (`:4133-4139`), a
  16 KiB + 1 value is `credential_value_too_large` (`:4141-4152`), and the
  source claim equals the pinned `anthropic env` vector (`:4154-4163`).

Two more exercise a real spawn. The fixture design is itself evidence: the
fixture binary receives its test controls only through the credential value
(`fixture_snapshot` at `:1125-1139` encodes them as JSON inside all three
credential variables; `install_fixture_controls` at `:433-463` decodes the
first one present and panics if none arrived). Every fixture test therefore
proves the credential row is the only channel from snapshot to child.

- `opencode_argv_env_stdin_contract` (`:1437-1550`) reads the child's
  `env.json` and asserts the adapter variables, `ANTHROPIC_API_KEY`
  equal to the sentinel (`:1516-1520`), and the absence of both identity names
  (`:1521-1522`).
- `pi_argv_privacy_contract` (`:1642-1791`) does the same for Pi at
  `:1755-1774`.

`credential_snapshot_must_match_before_backend_spawn`
(`tests/model_execution_protocol.rs:728-793`) covers the send-time check in
`CredentialVerifier::verify` (`mod.rs:104-125`): a route without a fingerprint
gets `harness_unavailable` and `backend.starts() == 0`
(`tests/model_execution_protocol.rs:744-755`); a route bound with the correct
fingerprint admits and starts one backend (`:757-790`).

## Failure scenario

Two distinct leaks, both closed by `env_clear` plus single-row selection:

1. The daemon runs with `EIDNARA_MODULE_ID` set. A forwarded environment lets
   the harness child open the daemon's socket as the module and issue
   privileged control operations.
2. The daemon runs with both `ANTHROPIC_API_KEY` and `OPENAI_API_KEY`. A
   request names `anthropic`; a forwarded environment hands the OpenAI key to
   the OpenCode child, which could send it to any endpoint it chooses.

## Timing windows and dependencies

None. The environment is fixed before `spawn` at `subprocess.rs:1199`, and
`env_clear` makes the daemon's later environment changes irrelevant.

The record's `Check` line says "no entry over the per-value cap". Only a
credential value is capped per-value (`subprocess.rs:219-221`).
`OPENCODE_CONFIG_CONTENT` has its own cap at `opencode.rs:218`; the other
adapter values are short constants or paths. A row holds at most four
variables (`subprocess.rs:118-149`), each within the 16 KiB value cap.

## What a test must construct

The fixture tests assert presence of the selected credential but not the
absence of the unselected ones in the spawned child. `pi_argv_privacy_contract`
runs with all three credential names in the snapshot and selects OpenAI; an
assertion that `env.json` lacks `ANTHROPIC_API_KEY` and `GEMINI_API_KEY`
would pin "exactly one provider variable" at the spawn boundary rather than at
the `provider_row` unit boundary. `install_fixture_controls` iterates its
credential names in order and returns on the first hit
(`tests/model_execution_subprocess.rs:439-461`), so today a
leaked `ANTHROPIC_API_KEY` in the Pi run would silently become the decoded
control channel and the test would still pass.

## Investigation log

### Q: Does the child receive the snapshot, or only the provider row?

- Sources examined: `subprocess.rs:200-233`, `:391`, `:517-519`;
  `opencode.rs:255-266`, `:312-334`; `pi.rs:390-401`, `:522-544`; the record's
  `Guarantee` line.
- Findings: only the provider row plus adapter-owned variables. The record's
  phrase "the admitted snapshot with the launch identity stripped and exactly
  one provider credential row" reads as though the rest of the snapshot is
  forwarded. It is not; the snapshot exists to be selected from, and
  `from_vars` stripping is defence in depth behind `provider_row`. The
  `Check` line ("exactly the selected provider variable") matches the code.
- Missing evidence: none for the mechanism.
- Conclusion: resolved with answer: the child environment is the provider row
  plus adapter-owned variables, and the record's `Guarantee` now says exactly
  that; the code and the record agree.

### Q: Is there an in-tree production caller that captures the real environment?

- Sources examined: `rg` for `capture_from`, `new_with_credentials`, and
  `EnvSnapshot` outside `src/model_execution` and `tests`, and for `ModelExecutionComponent::new`
  outside tests.
- Findings: none. Every `capture_from` and `new_with_credentials` call is in
  a test. `ModelExecutionComponent::new` at `mod.rs:129-139` sets `credential_verifier`
  to `None`, so a component built that way skips the fingerprint check at
  `mod.rs:357-369`.
- Missing evidence: the daemon wiring that constructs the snapshot from the
  process environment and chooses between `new` and `new_with_credentials`.
- Conclusion: resolved with answer: the record is `test-only`. In this checkout
  the aggregate cap and the verifier are reached only from tests, and the daemon
  wiring that would decide the production path is scheduled for U4.

## Dated note (2026-11-25, #1240)

Dated note, 2026-11-25, #1240: at this head the daemon's host binary captures
its startup environment with `EnvSnapshot::capture_from`
(`crates/daemon/src/bin/eidnara_host/serve.rs:1226`) and constructs the
component with `new_with_credentials` when its startup envelope carries
credentials or an AWS source
(`crates/daemon/src/bin/eidnara_host/serve.rs:1043-1045`, `:1250-1259`). Every
clause in this record that says these calls have no caller outside tests, or
that the daemon is scheduled for U4, is invalidated. The catalog's `test-only`
label stays under the catalog's definition of `default-production` until the
ModelExecution records are reclassified together.
