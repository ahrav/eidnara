# model_execution-protocol-shapes-are-closed

## Discovery trigger

`protocol.rs:1-2` says ModelExecution requests are "validated with the same
strict-decoding rules as the LocalEmbeddings protocol", and the section comment at
`:72-75` names the mechanism: closed request structs reject unknown and
duplicate fields, and `MapOnly` refuses the positional-sequence form. The
threat is a permissive decoder that lets a harness smuggle a field the host
never validates into the spawn path. The audit checked that every operation
goes through the closed schema, that the size boundary is exact, and that a
rejected request touches no supervisor state.

## Evidence trail

All references are at the tree this record was last revised with.

Preflight. `parse_request` at `protocol.rs:219-222` runs `preflight` before
any decode. `preflight` (`:164-175`) rejects binary bodies (`:165-167`),
bodies over `MAX_SEND_BODY_BYTES` (512 KiB, `config.rs:10`; check at
`protocol.rs:168-170`), and bodies deeper than `MAX_BODY_DEPTH` (8, `:15`; check at
`:171-173`). `depth_exceeds` (`local_embeddings/protocol.rs:611`) skips string bodies,
so braces inside a prompt do not count.

Dispatch. `decode_request` (`protocol.rs:187-217`) first decodes
`MapOnly<MethodEnvelope>` (`:188`), whose struct is `deny_unknown_fields`
(`:178-185`), then re-decodes the body against the
per-method envelope: `RequiredParams<SendParams>` for `session.send` (`:191`),
`RequiredParams<SubscribeParams>` for `session.subscribe` with a literal
`from == "start"` check (`:195-198`), `RequiredParams<RunIdParams>` for
`run.status` and `run.cancel` (`:201-210`), and `OptionalParams<NoParams>` for
`session.delete` (`:211-214`). Any other method is `schema` at `:215`.
`RequiredParams` and `OptionalParams` are both `deny_unknown_fields`
(`local_embeddings/protocol.rs:228-246`), and `MapOnly` rejects sequences (`:196-226`).

Field structs. `SendParams`, `ModelParams`, `GenerationParams`,
`SubscribeParams`, and `RunIdParams` all carry `deny_unknown_fields`
(`protocol.rs:77-128`). `tools` must deserialise as an empty sequence
(`EmptyTools`, `:130-157`). `parse_send` (`:229-278`) then applies value
rules: nonempty NUL-free prompt bounded by the body cap (`:231`), provider
and model bounded by 256 bytes (`:19`, `:233-234`), no `/` in provider
(`:236-238`), no leading `-` in either (`:241-246`), a `system` that, when
present, is nonempty and holds text after a plain-system directive
(`:247-257`), `max_output_tokens` in `1..=1_000_000` (`:259-261`,
bound at `config.rs:174`), and a temperature that, when present, is finite,
in `0.0..=2.0`, and positive-signed (`protocol.rs:263-269`, range at
`config.rs:181`). Run IDs are bounded by 128 bytes
(`protocol.rs:18`, `:224-227`).

Bind. `Harness::parse` accepts only `"opencode"` and `"pi"`
(`backend.rs:19-25`). `ModelExecutionComponent::bind` rejects anything else with
`invalid_identity` (`mod.rs:275-280`).

Handler order. `handle` parses at `mod.rs:320-323` and returns the error
before any `supervisor` call. The only allocation before the parse is a
resident scratch reservation (`:314`), an RAII local that drops on return.

Existing checks, verified, all in `tests/model_execution_protocol.rs`, a default-harness
binary CI runs via `cargo nextest run --profile ci --workspace --all-targets`
(`.github/workflows/ci.yml:733-735`), Linux-gated at
`tests/model_execution_protocol.rs:4`:

- `each_valid_operation_decodes_its_exact_schema` (`:71-166`): the five
  operations decode to their `Request` variants; a system text that opens with
  the plain-system directive is admitted whole (`:84-94`); a multi-slash model
  splits at the first slash (`:96-110`); `session.delete` accepts `{}` or no
  `params` (`:149-165`).
- `every_malformed_shape_is_rejected_with_schema_violation` (`:169-376`):
  thirty-six cases including binary body, non-object root, truncation,
  trailing content, duplicate `method`, `params`, and `prompt`, unknown
  envelope and params fields, unknown method, array params, empty prompt and
  system, a system of a directive alone, flat model string, slash and flag-shaped provider and model,
  nonempty and missing `tools`, out-of-range and string-typed generation
  fields, `from=cursor:3`, missing and oversize `run_id`, junk delete params,
  and an over-depth body. Each is asserted to return `schema_violation`
  (`:372-375`). The `mutate` helper panics if its needle is absent (`:36-42`),
  so a stale case cannot pass by testing an unmutated valid body.
- `the_512kib_boundary_admits_exactly_and_rejects_one_byte_over`
  (`:379-397`): a body of exactly `MAX_SEND_BODY_BYTES` admits (`:382-387`),
  one more byte is `schema_violation` naming "512 KiB" (`:389-396`).
- `malformed_requests_over_the_host_create_no_run_state` (`:1271-1310`): three
  malformed bodies over a real loopback route each return a `TY_ERROR` frame
  with `schema_violation` (`:1304-1305`); afterwards `backend.starts() == 0`
  and `supervisor.metrics()` equals the pre-request snapshot (`:1307-1308`).
- `harness_vocabulary_is_closed` (`:689-725`): `Harness::parse` accepts the
  two names and rejects case variants, a trailing space, `codex`, and the
  empty string (`:690-694`); the `harness_dispatch` and `answering_model`
  units have their exact shapes (`:695-724`).
- `bind_requires_absolute_root_nonempty_session_and_supported_harness`
  (`:647-686`) asserts the `invalid_identity` code for an unsupported harness
  at bind (`:667`, `:675`). The record's `Check` line names this outcome but
  its `Confidence` line does not list this test.

## Failure scenario

1. A harness sends `session.send` with an extra `"env": {...}` inside
   `params`, hoping the adapter forwards it to the child environment.
2. A decoder with `serde`'s default of ignoring unknown fields would accept
   the body and the field would be silently dropped today, but any later code
   reading the raw body could act on it.
3. As written, `deny_unknown_fields` on `SendParams` makes the body a
   `schema_violation` at `protocol.rs:191`, before the supervisor sees it.

The flag-shaped cases are the concrete spawn-path hazard: a provider of
`--config` would otherwise become `--model --config/model-a` in the OpenCode
argv (`opencode.rs:298-302`). `parse_send` rejects it at `protocol.rs:241-243`.

## Timing windows and dependencies

None. Parsing is synchronous and precedes every state change in `handle`. The
one dependency is the shared LocalEmbeddings decoding layer: a loosening of `MapOnly`,
`RequiredParams`, or `depth_exceeds` in `local_embeddings/protocol.rs` loosens ModelExecution
with it. That coupling is deliberate (`protocol.rs:1-2`) and the malformed
suite would detect most loosenings, but a change to `MapOnly`'s sequence
rejection is covered here only by the "array params" and "array root" cases.

## What a test must construct

The suite is already the enumerated form the record asks for. Two additions
would close what remains:

1. A `MapOnly` regression: a positional-sequence `params` for `session.send`
   that happens to have the right arity, so a derive that accepted sequences
   would fill fields positionally. The "array params" case at
   `tests/model_execution_protocol.rs:216-220` uses
   an empty array, which fails for arity reasons under either decoder.
2. A per-case assertion in the host-level test that the resident scratch at
   `mod.rs:314` is released, if a metric for it exists; today the test checks
   only the supervisor's metrics.

## Investigation log

### Q: Does every malformed case return `schema_violation` and no other code?

- Sources examined: `tests/model_execution_protocol.rs:169-376`; `protocol.rs:159-161`,
  `:164-175`, `:187-217`, `:224-278`; `local_embeddings/protocol.rs:43`.
- Findings: yes. Every rejection path in `preflight`, `decode`,
  `decode_request`, `parse_run_id`, and `parse_send` builds its error through
  `schema` at
  `local_embeddings/protocol.rs:43`, and `decode` wraps every `serde_json` error the
  same way (`protocol.rs:160`). The loop at
  `tests/model_execution_protocol.rs:372-375` asserts the code for
  all thirty-six cases with the case name in the failure message.
- Missing evidence: none.
- Conclusion: resolved. There is no path from `parse_request` to a
  non-`schema_violation` error.

### Q: Does a rejected request leave any supervisor state?

- Sources examined: `mod.rs:306-323`; `tests/model_execution_protocol.rs:1271-1310` (`malformed_requests_over_the_host_create_no_run_state`).
- Findings: the parse at `mod.rs:320` precedes every `self.supervisor` call,
  so a `schema_violation` return cannot have admitted, subscribed, or
  cancelled anything. The host-level test confirms `metrics()` is unchanged
  and no backend started.
- Missing evidence: none for the supervisor. The resident scratch at `mod.rs:314` is
  a host-side budget the test does not observe.
- Conclusion: resolved for the supervisor; the scratch release is an RAII
  drop and is not separately asserted.
