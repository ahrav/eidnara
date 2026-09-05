# broca-protocol-shapes-are-closed

## Discovery trigger

`protocol.rs:1-2` says Broca requests are "validated with the same
strict-decoding rules as the Synapse protocol", and the section comment at
`:70-73` names the mechanism: closed request structs reject unknown and
duplicate fields, and `MapOnly` refuses the positional-sequence form. The
threat is a permissive decoder that lets a harness smuggle a field the host
never validates into the spawn path. The audit checked that every operation
goes through the closed schema, that the size boundary is exact, and that a
rejected request touches no supervisor state.

## Evidence trail

All references are at `e16e39e`.

Preflight. `parse_request` at `protocol.rs:192-195` runs `preflight` before
any decode. `preflight` (`:147-158`) rejects binary bodies (`:148-150`),
bodies over `MAX_SEND_BODY_BYTES` (512 KiB, `config.rs:11`; check at
`:151-153`), and bodies deeper than `MAX_BODY_DEPTH` (8, `:14`; check at
`:154-156`). `depth_exceeds` (`synapse/protocol.rs:492`) skips string bodies,
so braces inside a prompt do not count.

Dispatch. `decode_request` (`:160-190`) first decodes
`MapOnly<MethodEnvelope>` (`:161`), whose struct is `deny_unknown_fields`
(`synapse/protocol.rs:75-82`), then re-decodes the body against the
per-method envelope: `RequiredParams<SendParams>` for `session.send` (`:164`),
`RequiredParams<SubscribeParams>` for `session.subscribe` with a literal
`from == "start"` check (`:168-172`), `RequiredParams<RunIdParams>` for
`run.status` and `run.cancel` (`:175-182`), and `OptionalParams<NoParams>` for
`session.delete` (`:185-186`). Any other method is `schema` at `:188`.
`RequiredParams` and `OptionalParams` are both `deny_unknown_fields`
(`synapse/protocol.rs:117-137`), and `MapOnly` rejects sequences (`:84-87`).

Field structs. `SendParams`, `ModelParams`, `GenerationParams`,
`SubscribeParams`, and `RunIdParams` all carry `deny_unknown_fields`
(`protocol.rs:75-111`). `tools` must deserialise as an empty sequence
(`EmptyTools`, `:113-140`). `parse_send` (`:202-243`) then applies value
rules: nonempty NUL-free prompt bounded by the body cap (`:204`), provider
and model bounded by 256 bytes (`:18`, `:206-207`), no `/` in provider
(`:209-211`), no leading `-` in either (`:214-219`), nonempty `system` when
present (`:220-227`), `max_output_tokens` in `1..=1_000_000` (`:229-231`,
bound at `config.rs:149`), and a finite temperature in `0.0..=2.0`
(`:232-234`, range at `config.rs:153`). Run IDs are bounded by 128 bytes
(`:17`, `:197-200`).

Bind. `Harness::parse` accepts only `"opencode"` and `"pi"`
(`backend.rs:19-25`). `BrocaComponent::bind` rejects anything else with
`invalid_identity` (`mod.rs:171-176`).

Handler order. `handle` parses at `mod.rs:213-216` and returns the error
before any `supervisor` call. The only allocation before the parse is a
resident scratch reservation (`:207`), an RAII local that drops on return.

Existing checks, verified, all in `tests/broca_protocol.rs`, a default-harness
binary CI runs via `cargo test --workspace --all-targets`
(`.github/workflows/ci.yml:118`), Linux-gated at `:4`:

- `each_valid_operation_decodes_its_exact_schema` (`:40-124`): the five
  operations decode to their `Request` variants; a multi-slash model splits at
  the first slash (`:54-68`); `session.delete` accepts `{}` or no `params`
  (`:107-123`).
- `every_malformed_shape_is_rejected_with_schema_violation` (`:126-320`):
  thirty-four cases including binary body, non-object root, truncation,
  trailing content, duplicate `method`, `params`, and `prompt`, unknown
  envelope and params fields, unknown method, array params, empty prompt and
  system, flat model string, slash and flag-shaped provider and model,
  nonempty and missing `tools`, out-of-range and string-typed generation
  fields, `from=cursor:3`, missing and oversize `run_id`, junk delete params,
  and an over-depth body. Each is asserted to return `schema_violation`
  (`:316-319`). The `mutate` helper panics if its needle is absent (`:32-38`),
  so a stale case cannot pass by testing an unmutated valid body.
- `the_512kib_boundary_admits_exactly_and_rejects_one_byte_over`
  (`:322-341`): a body of exactly `MAX_SEND_BODY_BYTES` admits (`:326-331`),
  one more byte is `schema_violation` naming "512 KiB" (`:333-340`).
- `malformed_requests_over_the_host_create_no_run_state` (`:673-710`): three
  malformed bodies over a real loopback route each return a `TY_ERROR` frame
  with `schema_violation` (`:704-705`); afterwards `backend.starts() == 0`
  and `supervisor.metrics()` equals the pre-request snapshot (`:707-708`).
- `harness_vocabulary_is_closed` (`:410-432`): `Harness::parse` accepts the
  two names and rejects case variants, a trailing space, `codex`, and the
  empty string (`:412-416`).
- `bind_requires_absolute_root_nonempty_session_and_supported_harness`
  (`:371-408`) asserts the `invalid_identity` code for an unsupported harness
  at bind (`:389`, `:397`). The record's `Check` line names this outcome but
  its `Confidence` line does not list this test.

## Failure scenario

1. A harness sends `session.send` with an extra `"env": {...}` inside
   `params`, hoping the adapter forwards it to the child environment.
2. A decoder with `serde`'s default of ignoring unknown fields would accept
   the body and the field would be silently dropped today, but any later code
   reading the raw body could act on it.
3. As written, `deny_unknown_fields` on `SendParams` makes the body a
   `schema_violation` at `protocol.rs:164`, before the supervisor sees it.

The flag-shaped cases are the concrete spawn-path hazard: a provider of
`--config` would otherwise become `--model --config/model-a` in the OpenCode
argv (`opencode.rs:141-150`). `parse_send` rejects it at `:214-216`.

## Timing windows and dependencies

None. Parsing is synchronous and precedes every state change in `handle`. The
one dependency is the shared Synapse decoding layer: a loosening of `MapOnly`,
`RequiredParams`, or `depth_exceeds` in `synapse/protocol.rs` loosens Broca
with it. That coupling is deliberate (`protocol.rs:1-2`) and the malformed
suite would detect most loosenings, but a change to `MapOnly`'s sequence
rejection is covered here only by the "array params" and "array root" cases.

## What a test must construct

The suite is already the enumerated form the record asks for. Two additions
would close what remains:

1. A `MapOnly` regression: a positional-sequence `params` for `session.send`
   that happens to have the right arity, so a derive that accepted sequences
   would fill fields positionally. The "array params" case at `:174-178` uses
   an empty array, which fails for arity reasons under either decoder.
2. A per-case assertion in the host-level test that the resident scratch at
   `mod.rs:207` is released, if a metric for it exists; today the test checks
   only the supervisor's metrics.

## Investigation log

### Q: Does every malformed case return `schema_violation` and no other code?

- Sources examined: `tests/broca_protocol.rs:126-320`; `protocol.rs:142-144`,
  `:147-158`, `:160-190`, `:197-243`; `synapse/protocol.rs:27`.
- Findings: yes. Every rejection path in `preflight`, `decode`,
  `decode_request`, `parse_run_id`, and `parse_send` builds its error through
  `schema` at
  `synapse/protocol.rs:27`, and `decode` wraps every `serde_json` error the
  same way (`protocol.rs:143`). The loop at `:316-319` asserts the code for
  all thirty-four cases with the case name in the failure message.
- Missing evidence: none.
- Conclusion: resolved. There is no path from `parse_request` to a
  non-`schema_violation` error.

### Q: Does a rejected request leave any supervisor state?

- Sources examined: `mod.rs:202-216`; `tests/broca_protocol.rs:673-710`.
- Findings: the parse at `mod.rs:213` precedes every `self.supervisor` call,
  so a `schema_violation` return cannot have admitted, subscribed, or
  cancelled anything. The host-level test confirms `metrics()` is unchanged
  and no backend started.
- Missing evidence: none for the supervisor. The resident scratch at `:207` is
  a host-side budget the test does not observe.
- Conclusion: resolved for the supervisor; the scratch release is an RAII
  drop and is not separately asserted.
