# derived-artifacts-are-ownership-independent

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

The audit proposes sharing ingress `Arc<WireBlock>` payloads into the
projection, holding the expanded native prefix as `Arc<Value>`, and reusing
projected digests for served output. Each of those replaces a clone with a
shared allocation. Three artifact families are digested downstream: the
`FlatProjection`, the native attachment with its sidecar, and the served
message bytes. The obligation is that each is a function of message values,
never of the allocation or the lane that produced them.

## Evidence trail

- [`flatten_block`][flatten] builds every [`FlatBlock`][flatblock]: `bytes` is
  `serde_json::to_string(block)`, `content_hash` is the SHA-256 of those bytes,
  `tool_input` is `Arc::new(input.clone())` for a tool call, `wire` is
  `Arc::new(block.clone())`, and `synthetic` copies `msg.ck.meta.synthetic`.
  [`FlatProjection`][flatproj] derives `PartialEq`, so structural equality is
  a usable oracle; [`differential_bytes`][diff-bytes] serializes blocks,
  identities, and the wires.
- [`sel_item_from_flat`][sel-item] reads `input` from `block.wire.kind()`;
  [`sel_kind_for_flat`][sel-kind] reads `block.tool_input`. Both must be built
  from the same block for their consumers to agree.
- The prefix differential [`assert_message_projection_equivalent`][assert-prefix]
  compares incremental against full by bytes and by value; it runs at
  [`:2919-2921`][prefix-call] when a reusable projection exists and
  [`prefix_projection_differential_enabled`][gate-prefix] is true, which is
  `cfg!(test) || EIDNARA_PREFIX_PROJECTION_DIFFERENTIAL == "1"`.
- [`reattach_messages_prefix`][reattach] rebuilds prefix shells from cached
  blocks with `WireMessage::from_parts`, so a rebuilt shell has no `original`;
  its [doc][reattach-doc] says unknown top-level fields are dropped.
- The native differential at [`:13316-13333`][native-diff] compares
  `to_vec(incremental)` with `to_vec(encode_full_native_messages(..))` under
  [`native_attachment_differential_enabled`][gate-native], the same gate shape.
- [`native_ingress_chunks`][ingress-chunks] shares an output chunk for index
  `i` exactly when [`chunk.value.as_ref() == message`][chunk-eq], a value test.
  [`remember_message`][remember] appends to `order` only on first sight;
  [`decode_opencode_sidecar_incremental`][sidecar-inc] copies the prior order,
  then appends suffix mids not already present ([`:277-291`][sidecar-merge]),
  and takes `mid_pins` from the suffix only.
- [`ServedMessage::from_message_reusing`][served-reusing] computes
  `canonical_bytes = to_vec(to_value(&message))`, `canonical_hash` as its
  SHA-256, and per-block fingerprints as `(fingerprint(to_string(block)),
  len)` unless [`fingerprint_from_projected_wire`][fp-reuse] finds
  `flat.wire == served` under `WireBlock`'s derived equality. The workspace
  `serde_json` enables only [`raw_value`][serde-features], so `to_value`
  produces a map whose keys serialize sorted.
- [`Serialize for ServedMessage`][ser-served] re-serializes the inner
  `WireMessage`, not `canonical_bytes`. The handler avoids that path by taking
  `messages` out of the response before `to_value(response)`
  ([`:14428-14443`][segments-take]) and writing each through
  [`PreparedSegment::served`][segment-served] ([`:14448-14454`][segments]),
  whose `bytes()` returns `canonical_bytes`.

## Failure scenario

A projector that reuses an ingress `Arc<WireBlock>` but computes `bytes` from
a different serialization breaks `bytes == to_string(wire)` and every digest
keyed on it. A chunk-sharing decision by pointer identity diverges from the
value test at [`:13058`][chunk-eq] for a message equal by value but not by
pointer. A sidecar merge that reorders a repeated mid changes `order`. A
direct `to_vec(&message)` on a typed shell emits struct field order where the
`to_value` round trip emits sorted keys, so bytes and `canonical_hash` change
while the JSON is equal.

## Timing windows and dependencies

None in time. The dependencies are the `raw_value`-only feature set (which
decides key order), `WireBlock`'s derived equality including `original`, and
the two release-live differential gates whose environment variables no `docs/`
file names.

## What a test must construct

A second pass with a projection cache hit ([`astro_scale...`][t-astro]); a delta
body so the prefix is reattached and the native prefix comes from the
attachment cache; tool calls, tool results in a user message, repeated call
ids, and a suffix that repeats a prefix mid; a message equal by value but not
by pointer to a cached chunk; a response holding a harness message with
`original`, a rebuilt prefix message, a reduced message, a tag-overlaid
message, and the synthetic m0 and m1. Assert the three families against their
value-only construction, including `tool_input` versus `wire.kind()`, sidecar
`order` and `messages`, the chunk-sharing predicate, and that every `Served`
segment writes `canonical_bytes`. Run both gates from `crates/daemon/tests/`,
where `cfg!(test)` is false for the library. The
[shared-input checks](../existing-checks.md#shared-input-equivalence) include
both differentials and fingerprint reuse; none covers the four added oracles.

## Investigation log

### Q: Is the release-build `assert_eq!` panic the intended contract?

- Sources examined: [`gate-prefix`][gate-prefix], [`gate-native`][gate-native],
  the transform catalog's [portfolio evaluation][tc-g2] gap G2.
- Findings: Both gates are live in release under an environment variable; the
  evaluation queued the decision and it is still open.
- Missing evidence: A statement of the intended production behavior.
- Conclusion: needs human input.

### Q: Is sorted-key canonical JSON a contract with the plugin?

- Sources examined: [`from_message_reusing`][served-reusing],
  [`Cargo.toml`][serde-features], [`Serialize for ServedMessage`][ser-served].
- Findings: The order follows from `preserve_order` being off; no wire document
  names it, and the response path emits the other form for any served message
  encoded through serde.
- Missing evidence: A written contract.
- Conclusion: needs human input.

### Q: Is `mid_pins` equality between the two sidecar forms required?

- Sources examined: [`decode_opencode_sidecar_incremental`][sidecar-inc],
  the [native differential][native-diff].
- Findings: The incremental form takes pins from the suffix decode; the full
  decode accumulates over the whole array. The differential compares output
  bytes, not sidecars.
- Missing evidence: A two-form sidecar comparison.
- Conclusion: unresolved, needs a two-form sidecar comparison.

[tc-g2]: ../../../daemon/transform/portfolio-evaluation.md
[flatblock]: ../../../../../crates/daemon/src/wire.rs#L36-L64
[flatproj]: ../../../../../crates/daemon/src/wire.rs#L114-L127
[reattach-doc]: ../../../../../crates/daemon/src/wire.rs#L141-L144
[reattach]: ../../../../../crates/daemon/src/wire.rs#L145-L186
[diff-bytes]: ../../../../../crates/daemon/src/wire.rs#L329-L337
[flatten]: ../../../../../crates/daemon/src/wire.rs#L673-L736
[fp-reuse]: ../../../../../crates/daemon/src/wire.rs#L826-L835
[served-reusing]: ../../../../../crates/daemon/src/transform.rs#L164-L216
[ser-served]: ../../../../../crates/daemon/src/transform.rs#L293-L300
[gate-prefix]: ../../../../../crates/daemon/src/transform.rs#L2013-L2020
[assert-prefix]: ../../../../../crates/daemon/src/transform.rs#L2030-L2045
[prefix-call]: ../../../../../crates/daemon/src/transform.rs#L2919-L2921
[sel-item]: ../../../../../crates/daemon/src/transform.rs#L6355-L6384
[sel-kind]: ../../../../../crates/daemon/src/lib.rs#L16629-L16644
[ingress-chunks]: ../../../../../crates/daemon/src/lib.rs#L13027-L13071
[chunk-eq]: ../../../../../crates/daemon/src/lib.rs#L13058
[gate-native]: ../../../../../crates/daemon/src/lib.rs#L13073-L13080
[native-diff]: ../../../../../crates/daemon/src/lib.rs#L13316-L13333
[segments-take]: ../../../../../crates/daemon/src/lib.rs#L14428-L14443
[segments]: ../../../../../crates/daemon/src/lib.rs#L14448-L14454
[t-astro]: ../../../../../crates/daemon/src/lib.rs#L20948
[sidecar-inc]: ../../../../../crates/daemon/src/codec/opencode.rs#L258-L293
[sidecar-merge]: ../../../../../crates/daemon/src/codec/opencode.rs#L277-L291
[remember]: ../../../../../crates/daemon/src/codec/sidecar.rs#L67-L73
[segment-served]: ../../../../../crates/daemon/src/dispatch.rs#L50-L72
[serde-features]: ../../../../../Cargo.toml#L47
