# model_execution-payload-hook-owns-the-generation-controls

## Discovery trigger

`crates/host-runtime/src/model_execution/pi.rs:44-45` compiles `assets/pi-model_execution-extension.mjs`
into the runtime with `include_bytes!`, and the hook's own header
(`pi-model_execution-extension.mjs:1-32`) states the contract: loaded last, it replaces the
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
zero or over-bound token count (`:265`), a revision outside `1 | 2` (`:268-269`), and
a revision 1 request with no temperature (`:271-272`), and bounds any present
temperature (`:275-280`). A revision 2 request may omit temperature to keep the
model's native decoding policy, so the hook can receive a bound with no
temperature.

Delivery. `run_pi` writes `PI_MODEL_EXECUTION_EXTENSION_BYTES` to a 0600 file in the
per-run 0700 directory through `PrivateDir::write_private_async` (`pi.rs:430-433`;
`subprocess.rs:1382-1390`, `create_new` refuses an existing path or symlink), so
no installed hook can be swapped under the daemon. The argv disables
extension discovery with `--no-approve --no-extensions` (`pi.rs:502-503`),
pushes each trusted closure extension (`:505-508`), and pushes the hook last
(`:509-510`). The admitted bound reaches the child as
`EIDNARA_MODEL_EXECUTION_MAX_OUTPUT_TOKENS` (`:523-526`); `EIDNARA_MODEL_EXECUTION_TEMPERATURE` is
set only when the request admitted a temperature (`:527-532`).

The hook. `requiredNumber` (`pi-model_execution-extension.mjs:38`) throws when a
variable is present but empty or not finite; the bound is always required
(`:115`), while temperature is read only when its variable exists (`:116-117`) and
is dropped for a model whose registry entry refuses it (`:118-119`).
The handler throws on a non-object payload, collects every present spelling
from `max_output_tokens`, `max_completion_tokens`, `max_tokens`, and
`maxOutputTokens`, reads Gemini-style `generationConfig` and Bedrock-style
`inferenceConfig` objects and rejects either when present but not an object
(`:142-146`), throws when none is present (`:153-157`), rewrites every collected
spelling, sets `temperature` only when a spelling was present and a temperature
was admitted (`:165-167`), and rewrites `generationConfig` and `inferenceConfig`
with the bound and, when admitted, the temperature, preserving their other keys
(`:171-176`, `:181-186`). For a registry-refusing model it also removes any
`temperature` already present in those three places (`:168-170`, `:177-179`,
`:187-189`). With no admitted temperature for any other model the hook neither
writes nor removes a `temperature` field, so an earlier handler's value or the
provider default survives.

Cache markers. A run sends one user message and ends: the argv passes
`--print`, `--no-session`, and `--no-tools` (`pi.rs:492-500`). The handler passes a
payload's `messages` array through `withoutConversationCacheMarkers`
(`pi-model_execution-extension.mjs:159-161`). That function
(`pi-model_execution-extension.mjs:74-92`) removes the Bedrock Converse
`cachePoint` member and the Anthropic `cache_control` member from each content
block of every message whose role is `user`. A block's other members stay, and
a block that held only a `cachePoint` is dropped. The function reads a block's
own members only. A `system` or `developer` message and the payload's
`system` field pass through with their markers, so only the system prompt can
carry cache markers in a run. `withoutConversationCacheMarkers` in
`packages/pi-plugin/src/memory-capture-native.ts:38-58` applies the same rule
to the plugin's capture call through `onPayload` (`:105`).

Check. `pi_model_execution_hook_owns_generation_controls`
(`tests/model_execution_subprocess.rs:2240`) writes the compiled-in bytes to a
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
(`:2265-2266`) to model revision 2: an OpenAI-style payload gains
`max_output_tokens == 32000` and no `temperature` (`:2331-2332`), and a Bedrock-style
payload keeps its own `inferenceConfig.temperature == 1` beside the rewritten
bound (`:2335-2336`). The binary is `harness = false`.

`pi_model_execution_hook_keeps_cache_writes_to_the_system_prompt`
(`tests/model_execution_subprocess.rs:2116`) drives the hook with a Bedrock
Converse payload, an Anthropic Messages payload, and OpenAI-completions
payloads that carry the system prompt as a `system` and as a `developer`
message. It asserts each user message comes back with its other blocks and no
marker, each system prompt and an assistant message keep their markers, and a
user message whose content is a string is unchanged. A last payload carries
user blocks that an earlier handler could leave: a `cachePoint` beside a
`text` member, a block with both markers beside its text, a block with both
markers and nothing else, standalone `cachePoint` blocks whose value is `null`
and a string, a block whose `cachePoint` sits on its prototype, and an array
entry. The test asserts the text members and the array entry come back and no
marker does.

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
wrong. A Pi release that marks a user message in a third spelling leaves that
marker in place, and each run then pays the provider's cache-write premium for
an entry only a Pi retry reads.

## Timing windows and dependencies

None. The hook runs synchronously inside Pi's handler chain on every request;
the property depends on Pi invoking `before_provider_request` handlers in
registration order, which the runner's argv order and the test's handler
array both assume. The cache-marker clause depends on the run staying
single-turn, which the argv fixes. Pi's retry of a failed attempt sends the
same user message again (`auto_retry_start`, up to three retries by default); a
marker on that message would be read only by such a retry, when the provider
had processed the failed attempt, and would cost the write premium on every
run.

## What a test must construct

- Present: a tampering handler ahead of the hook; OpenAI-style, Gemini-style,
  Bedrock-style, and two-spelling payloads; one unrecognized shape; a
  non-object `inferenceConfig`; the revision 2 path with the temperature
  variable absent; refusing contexts with a preexisting `temperature` in each
  owned place; user messages and system prompts that carry a cache marker in
  the Bedrock Converse, Anthropic Messages, and OpenAI-completions shapes; a
  user block that carries a marker beside other members.
- Missing: the hook running inside a real Pi process rather than the driver's
  handler array; a non-numeric environment value; a payload whose
  `generationConfig` is present but not an object.

## Investigation log

### Q: Can a project-owned extension run after the hook?

- Sources examined: `pi.rs:498-510`; `tests/model_execution_subprocess.rs:1913-1963`.
- Findings: `--no-extensions` disables discovery, only closure extensions and
  the hook are passed with `--extension`, and the hook is pushed last;
  `pi_project_pi_resources_ignored` asserts exactly one `--extension` ending in
  `PI_MODEL_EXECUTION_EXTENSION_FILE` when no closure extension is configured.
- Missing evidence: none for the argv contract.
- Conclusion: resolved; the hook is the final handler under the runner's argv.

### Q: Does the hook trust the environment values?

- Sources examined: `pi-model_execution-extension.mjs:38-45`; `pi.rs:523-532`;
  `model_execution/protocol.rs:265-280`.
- Findings: the values are formatted from the admitted request. The hook fails
  the request rather than defaulting when the bound is absent or either value is
  present but not finite. An absent temperature variable is the revision 2
  contract, not a fault: the hook then leaves temperature to the provider.
- Missing evidence: no test drives the absent-bound or non-numeric path.
- Conclusion: resolved as a mechanism; the negative paths are unexercised.

### Q: Does the hook write a temperature for a model whose API refuses it?

- Sources examined: `crates/host-runtime/assets/pi-model_execution-extension.mjs:51-60`, `:104-119`, and `:165-189`;
  Pi 0.80.2 `pi-ai/dist/api/anthropic-messages.js:115` and `:698`, which read
  `model.compat.supportsTemperature` and send no temperature when it is false;
  `pi-coding-agent/dist/core/extensions/runner.js:411-441`, where the handler
  context's `model` getter returns the session model.
- Findings: `modelRefusesTemperature` (`:54`) reads
  `ctx.model.compat.supportsTemperature` and returns true only for `false`; a
  context that throws or carries no model counts as accepting. The handler
  still validates a present temperature variable (`:116-117`), drops it for a
  refusing model (`:118-119`), and deletes a `temperature` an earlier handler left
  on the payload, in `generationConfig`, or in `inferenceConfig` (`:168-170`,
  `:177-179`, `:187-189`), so such a model keeps its native decoding and the
  provider sees no refused field whatever ran before the hook. Every other model keeps the admitted
  temperature, and the host's refusal retry in
  `crates/host-runtime/src/model_execution/pi.rs` still covers models the
  registry does not mark.
  `pi_model_execution_hook_omits_temperature_for_registry_refusing_models`
  (`tests/model_execution_subprocess.rs:2340`) drives refusing, accepting,
  unmarked, throwing, and absent contexts, and refusing contexts whose payloads
  already carry a `temperature` in each of the three places.
- Missing evidence: the driver passes a context object; no test runs the hook
  inside a real Pi process.
- Conclusion: resolved; the guarantee names the registry exception.

## Dated note (2026-10-12, #989)

Dated note, 2026-10-12, #989 (spec #988 amendment A1): the host wire has one
unversioned contract, so every clause here that names a protocol version,
numbered revision, version-skew refusal, or retired-field refusal is
invalidated. At the #989 head, routed envelope, context, memory-capture,
note-evaluation, discovery, and transform bodies carry no `v`; transform
ignores a field named `v` and the former carriers, strict bodies refuse them
as unknown fields, `transform_revision_unsupported` and
`transform_tail_delta_retired` no longer exist, `transform/revision_3.rs` is
`transform/window_admission.rs`, `transform/revision_goldens.rs` is
`transform/recorded_goldens.rs`, and the golden fixture `transform-revision-2-goldens.json` is now
`transform-recorded-goldens.json`; `serve_native` remains until #993; the
`session.send` generation object carries `max_output_tokens` and an optional
`temperature` with no `revision`; absence of `temperature` selects the
model-native decoding policy. Line anchors into files #989 changed are as of
their earlier recorded head.

## Dated note (2026-10-12, #993)

Dated note, 2026-10-12, #993 (spec #988 milestone U3): the transform request
carries no `serve_native`. The `pi` and `opencode-aisdk` profiles send
`native_messages` as their only window, the daemon derives the pass's CK from
it through the profile's codec before any projection and always serves native
recipes, and the `owned-llmrunner` and `claude-code-anthropic` profiles send
`messages`. Every clause here that names `serve_native`, a CK window sent
beside the native window, or the Pi check that aligned rows with `messages` is
invalidated; line anchors into files #993 changed are as of their earlier
recorded head.

## Dated note (2026-11-19, #1223)

Dated note, 2026-11-19, #1223: the daemon's host binary constructs
`PiBackend::new` (`crates/daemon/src/bin/eidnara_host/serve.rs:992`), so Pi runs
and this hook are on a production path at this head. The Reachability section
above and the catalog's `test-only` label describe the tree before that
wiring, and every clause that says the Pi runs are test-only is invalidated.
The label stays `test-only` until the reclassification of all ModelExecution
records. A Pi run starts in the state root's shared `cwd/` directory
(`subprocess.rs:2496-2500`) and keeps its hook file in the per-run private
directory.
