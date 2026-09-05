# broca-payload-hook-owns-the-generation-controls

## Discovery trigger

`crates/host-runtime/src/broca/pi.rs:35` compiles `assets/pi-broca-extension.mjs`
into the runtime with `include_bytes!`, and the hook's own header
(`pi-broca-extension.mjs:1-16`) states the contract: loaded last, it replaces the
provider-native output-token and temperature fields with the values Broca
admitted and rejects payload shapes it does not recognize. The hook is the
only place where the admitted generation controls become provider bytes, and
it is JavaScript inside a Rust crate, so the Rust catalog records did not
reach it. The audit traced the controls from the admitted request to the
payload the hook returns.

## Evidence trail

All references are at the tree this record enters with.

Admission. `GenerationParams` carries `max_output_tokens: u64` and
`temperature: f64` (`broca/protocol.rs:42-43`), and `parse_send` rejects a
zero or over-bound token count (`:229-230`), so the values the hook receives
are already bounded.

Delivery. `run_pi` writes `PI_BROCA_EXTENSION_BYTES` to a 0600 file in the
per-run 0700 directory through `PrivateDir::write_private` (`pi.rs:226`;
`subprocess.rs:772-778`, `create_new` refuses an existing path or symlink), so
no installed hook can be swapped under the daemon. The argv disables
extension discovery with `--no-approve --no-extensions` (`pi.rs:262-263`),
pushes each trusted closure extension (`:296-297`), and pushes the hook last
(`:300-301`). The admitted values reach the child as
`EIDNARA_BROCA_MAX_OUTPUT_TOKENS` and `EIDNARA_BROCA_TEMPERATURE` (`:314-321`).

The hook. `requiredNumber` (`pi-broca-extension.mjs:21-29`) throws when either
variable is absent, empty, or not finite. The handler (`:35-79`) throws on a
non-object payload (`:39-41`), collects every present spelling from
`max_output_tokens`, `max_completion_tokens`, `max_tokens`, and
`maxOutputTokens` (`:50-55`), reads a Gemini-style `generationConfig` object
(`:56-58`), throws when neither is present (`:59-63`), copies the payload
(`:64`), rewrites every collected spelling (`:65-67`), sets `temperature` when
any spelling was present (`:68-70`), and rewrites `generationConfig` with
both values while preserving its other keys (`:71-77`).

Check. `pi_broca_hook_owns_generation_controls`
(`tests/broca_subprocess.rs:1599-1670`) writes the compiled-in bytes to a
scratch file, registers a handler ahead of the hook that sets `temperature: 9`
and adds `providerTouched`, and drives four payloads under Node or Bun with
the two environment variables set to `32000` and `0.25`. It asserts the
OpenAI-style payload carries `max_tokens == 32000`, `temperature == 0.25`, and
its unrelated fields (`:1648-1652`); the Gemini-style payload carries the
rewritten `generationConfig` with `topK` preserved (`:1656-1659`); a payload
with both `max_completion_tokens` and `max_tokens` has both rewritten
(`:1664-1666`); and `{ foo: "bar" }` is rejected (`:1669`). The runner lists
the check in its `main` table (`:100-101`); the binary is `harness = false`.

## Failure scenario

An earlier extension in the load chain, or a provider's default, sets a larger
output-token limit in a spelling the hook does not rewrite, or a new provider
wire family arrives with no recognized field. The request runs with a budget
the caller never admitted, and the byte charge Broca accounted for the run is
wrong.

## Timing windows and dependencies

None. The hook runs synchronously inside Pi's handler chain on every request;
the property depends on Pi invoking `before_provider_request` handlers in
registration order, which the runner's argv order and the test's handler
array both assume.

## What a test must construct

- Present: a tampering handler ahead of the hook; OpenAI-style, Gemini-style,
  and two-spelling payloads; one unrecognized shape.
- Missing: the hook running inside a real Pi process rather than the driver's
  handler array; a missing or non-numeric environment value; a payload whose
  `generationConfig` is present but not an object.

## Investigation log

### Q: Can a project-owned extension run after the hook?

- Sources examined: `pi.rs:258-301`; `tests/broca_subprocess.rs:1566-1583`.
- Findings: `--no-extensions` disables discovery, only closure extensions and
  the hook are passed with `--extension`, and the hook is pushed last;
  `pi_project_pi_resources_ignored` asserts exactly one `--extension` ending in
  `PI_BROCA_EXTENSION_FILE` when no closure extension is configured.
- Missing evidence: none for the argv contract.
- Conclusion: resolved; the hook is the final handler under the runner's argv.

### Q: Does the hook trust the environment values?

- Sources examined: `pi-broca-extension.mjs:21-29`; `pi.rs:314-321`;
  `broca/protocol.rs:229-230`.
- Findings: the values are formatted from the admitted request, and the hook
  fails the request rather than defaulting when they are absent or not finite.
- Missing evidence: no test drives the absent-variable path.
- Conclusion: resolved as a mechanism; the negative path is unexercised.
