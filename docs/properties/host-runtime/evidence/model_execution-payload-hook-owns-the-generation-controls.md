# model_execution-payload-hook-owns-the-generation-controls

## Discovery trigger

`crates/host-runtime/src/model_execution/pi.rs:41-42` compiles `assets/pi-model_execution-extension.mjs`
into the runtime with `include_bytes!`, and the hook's own header
(`pi-model_execution-extension.mjs:1-23`) states the contract: loaded last, it replaces the
provider-native output-token and temperature fields with the values ModelExecution
admitted, sends no temperature to a model Pi's registry marks as refusing it, and
rejects payload shapes it does not recognize. The hook is the
only place where the admitted generation controls become provider bytes, and
it is JavaScript inside a Rust crate, so the Rust catalog records did not
reach it. The audit traced the controls from the admitted request to the
payload the hook returns.

## Evidence trail

All references are at the tree this record was last revised with.

Admission. `GenerationParams` carries `max_output_tokens: u64` and
`temperature: Option<f64>` (`model_execution/protocol.rs:43-45`). The wire form
carries a generation `revision` defaulting to 1 (`:107-108`); `parse_send` rejects a
zero or over-bound token count (`:262`), a revision outside `1 | 2` (`:265-266`), and
a revision 1 request with no temperature (`:268-269`), and bounds any present
temperature (`:272-277`). A revision 2 request may omit temperature to keep the
model's native decoding policy, so the hook can receive a bound with no
temperature.

Delivery. `run_pi` writes `PI_MODEL_EXECUTION_EXTENSION_BYTES` to a 0600 file in the
per-run 0700 directory through `PrivateDir::write_private_async` (`pi.rs:423-426`;
`subprocess.rs:1313-1326`, `create_new` refuses an existing path or symlink), so
no installed hook can be swapped under the daemon. The argv disables
extension discovery with `--no-approve --no-extensions` (`pi.rs:490-491`),
pushes each trusted closure extension (`:493-496`), and pushes the hook last
(`:497-498`). The admitted bound reaches the child as
`EIDNARA_MODEL_EXECUTION_MAX_OUTPUT_TOKENS` (`:511-514`); `EIDNARA_MODEL_EXECUTION_TEMPERATURE` is
set only when the request admitted a temperature (`:515-520`).

The hook. `requiredNumber` (`pi-model_execution-extension.mjs:28`) throws when a
variable is present but empty or not finite; the bound is always required
(`:54`), while temperature is read only when its variable exists (`:55-56`) and
is dropped for a model whose registry entry refuses it (`:57-58`).
The handler throws on a non-object payload, collects every present spelling
from `max_output_tokens`, `max_completion_tokens`, `max_tokens`, and
`maxOutputTokens`, reads Gemini-style `generationConfig` and Bedrock-style
`inferenceConfig` objects and rejects either when present but not an object
(`:81-85`), throws when none is present (`:92-96`), rewrites every collected
spelling, sets `temperature` only when a spelling was present and a temperature
was admitted (`:101-103`), and rewrites `generationConfig` and `inferenceConfig`
with the bound and, when admitted, the temperature, preserving their other keys
(`:107-112`, `:117-122`). For a registry-refusing model it also removes any
`temperature` already present in those three places (`:104-106`, `:113-115`,
`:123-125`). With no admitted temperature for any other model the hook neither
writes nor removes a `temperature` field, so an earlier handler's value or the
provider default survives.

Check. `pi_model_execution_hook_owns_generation_controls`
(`tests/model_execution_subprocess.rs:1831`) writes the compiled-in bytes to a
scratch file, registers a handler ahead of the hook that sets `temperature: 9`
and adds `providerTouched`, and drives the payloads under Node or Bun with the
two environment variables set to `32000` and `0.25`. It asserts the OpenAI-style
payload carries `max_tokens == 32000`, `temperature == 0.25`, and its unrelated
fields; the Gemini-style payload carries the rewritten `generationConfig` with
`topK` preserved; the Bedrock-style payload carries `inferenceConfig.maxTokens ==
32000`, `temperature == 0.25`, and `topP` preserved, and a non-object
`inferenceConfig` is rejected; a payload with both `max_completion_tokens` and
`max_tokens` has both rewritten; and `{ foo: "bar" }` is rejected. It then
deletes `EIDNARA_MODEL_EXECUTION_TEMPERATURE` and drops the tampering handler
(`:1857`) to model revision 2: an OpenAI-style payload gains
`max_output_tokens == 32000` and no `temperature` (`:1923`), and a Bedrock-style
payload keeps its own `inferenceConfig.temperature == 1` beside the rewritten
bound (`:1927`). The binary is `harness = false`.

## Reachability

The hook is on the path of every Pi run, but the runs themselves are test-only
at this tree: `PiBackend::new`, `PiBackend::with_limits`, and `run_pi` have no
caller outside `tests/model_execution_subprocess.rs`, the same fact
`model_execution-child-environment-carries-only-the-provider-row` records for the spawn
path. The label moves with the other ModelExecution records when the daemon wires a
real backend.

## Failure scenario

An earlier extension in the load chain, or a provider's default, sets a larger
output-token limit in a spelling the hook does not rewrite, or a new provider
wire family arrives with no recognized field. The request runs with a budget
the caller never admitted, and the byte charge ModelExecution accounted for the run is
wrong.

## Timing windows and dependencies

None. The hook runs synchronously inside Pi's handler chain on every request;
the property depends on Pi invoking `before_provider_request` handlers in
registration order, which the runner's argv order and the test's handler
array both assume.

## What a test must construct

- Present: a tampering handler ahead of the hook; OpenAI-style, Gemini-style,
  Bedrock-style, and two-spelling payloads; one unrecognized shape; a
  non-object `inferenceConfig`; the revision 2 path with the temperature
  variable absent; refusing contexts with a preexisting `temperature` in each
  owned place.
- Missing: the hook running inside a real Pi process rather than the driver's
  handler array; a non-numeric environment value; a payload whose
  `generationConfig` is present but not an object.

## Investigation log

### Q: Can a project-owned extension run after the hook?

- Sources examined: `pi.rs:486-498`; `tests/model_execution_subprocess.rs:1777-1827`.
- Findings: `--no-extensions` disables discovery, only closure extensions and
  the hook are passed with `--extension`, and the hook is pushed last;
  `pi_project_pi_resources_ignored` asserts exactly one `--extension` ending in
  `PI_MODEL_EXECUTION_EXTENSION_FILE` when no closure extension is configured.
- Missing evidence: none for the argv contract.
- Conclusion: resolved; the hook is the final handler under the runner's argv.

### Q: Does the hook trust the environment values?

- Sources examined: `pi-model_execution-extension.mjs:28-35`; `pi.rs:511-520`;
  `model_execution/protocol.rs:262-277`.
- Findings: the values are formatted from the admitted request. The hook fails
  the request rather than defaulting when the bound is absent or either value is
  present but not finite. An absent temperature variable is the revision 2
  contract, not a fault: the hook then leaves temperature to the provider.
- Missing evidence: no test drives the absent-bound or non-numeric path.
- Conclusion: resolved as a mechanism; the negative paths are unexercised.

### Q: Does the hook write a temperature for a model whose API refuses it?

- Sources examined: `crates/host-runtime/assets/pi-model_execution-extension.mjs:41-58` and `:101-125`;
  Pi 0.80.2 `pi-ai/dist/api/anthropic-messages.js:115` and `:698`, which read
  `model.compat.supportsTemperature` and send no temperature when it is false;
  `pi-coding-agent/dist/core/extensions/runner.js:411-441`, where the handler
  context's `model` getter returns the session model.
- Findings: `modelRefusesTemperature` (`:44`) reads
  `ctx.model.compat.supportsTemperature` and returns true only for `false`; a
  context that throws or carries no model counts as accepting. The handler
  still validates a present temperature variable (`:55-56`), drops it for a
  refusing model (`:57-58`), and deletes a `temperature` an earlier handler left
  on the payload, in `generationConfig`, or in `inferenceConfig` (`:104-106`,
  `:113-115`, `:123-125`), so such a model keeps its native decoding and the
  provider sees no refused field whatever ran before the hook. Every other model keeps the admitted
  temperature, and the host's refusal retry in
  `crates/host-runtime/src/model_execution/pi.rs` still covers models the
  registry does not mark.
  `pi_model_execution_hook_omits_temperature_for_registry_refusing_models`
  (`tests/model_execution_subprocess.rs:1931`) drives refusing, accepting,
  unmarked, throwing, and absent contexts, and refusing contexts whose payloads
  already carry a `temperature` in each of the three places.
- Missing evidence: the driver passes a context object; no test runs the hook
  inside a real Pi process.
- Conclusion: resolved; the guarantee names the registry exception.
