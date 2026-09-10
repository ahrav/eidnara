# route-and-typed-decode-are-independent-of-entry-path

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

The audit proposes replacing the `Value` parse plus `from_value` pair with a
direct `from_slice::<TransformRequest>` on the unpaged lane. Routing today reads
the raw `Value`, and both lanes reach the same typed decode from a `Value`. A
replacement on one lane can derive the lane from a typed field, treat a `null`
page field as absent, or report duplicate keys and malformed JSON differently,
so the same body decodes two ways depending on which lane carried it.

## Evidence trail

- [`dispatch_value_with_inbound_bytes`][dispatch] reads `method` with
  `Value::as_str`, falls back to `kind` with `as_str`, so a non-string
  discriminator is absent. A body with `name` and `arguments` and no
  discriminator goes to the facade; anything else reaches
  [`unrecognized_request_error`][unrecognized], which reports
  `non-object JSON (<type>)` for a non-object root. A malformed body arrives as
  `Value::Null` from [`handle`][handle], so it reports `non-object JSON (null)`
  ([`json_type_name`][typename]).
- [`handle_transform_dispatch`][tdispatch] splits on
  [`has_transform_page_fields`][pagefields], which is `request.get(field)
  .is_some()` over the six [`TRANSFORM_PAGE_FIELDS`][pageconst]; a `null` value
  is present by key and selects the page lane.
- The unpaged lane reads [`request_observed_at_ms`][observed] from the raw
  `Value`, then runs [`from_value::<TransformRequest>`][fromvalue] with any
  error as `bad_request`. The page lane assembles a `Value` and calls the same
  function through [`handle_transform_unpaged_value`][pageapply].
- [`TransformRequestWire`][wirestruct] has `#[serde(default)] kind: String`,
  no `method` field, and no `deny_unknown_fields`; `session_id` and
  `render_config` are the only fields without a default. The
  [`Deserialize for TransformRequest`][typed-decode] copies `kind` without
  validating it. `Option` fields take `null` as `None`; defaulted non-`Option`
  fields reject `null`.
- [`WireMessage`][wiremsg] and [`WireBlock`][wireblock] each decode a `Value`
  first and retain it, so the typed decode is a `Value` round trip at every
  level, not only at the top.
- Over 1 MiB, [`enforce_request_byte_cap`][bytecap] runs the
  [`RequestMethodProbe`][probe], whose [`ProbeString`][probestr] keeps only a
  string of at most 64 bytes and skips arrays, objects, numbers, booleans, and
  `null`; [`is_transform_class`][class] reads `method` then `kind` and admits
  `transform`, `state_sync`, and `kernel.artifact.ingest.page`. The ceiling
  `body.len() <= MAX_TRANSFORM_FRAME_BYTES` is inclusive at 32 MiB.
- Tests enter at [`dispatch_value_for_test`][testentry] with a built `Value`.
  The `Value` map's last-wins behavior on duplicate keys and serde_derive's
  `duplicate field` error are cited from upstream source in the lens
  ([`Map::insert`][mapinsert], [serde_derive][derivedup]); neither ran here.

## Failure scenario

A direct typed decode derives the lane from `TransformRequest.kind`, so
`{"method":"transform","kind":"x"}` routes differently than the `Value` read.
It treats `{"kind":"transform","transform_page_id":null}` as unpaged where the
key test selects the page lane. It returns `duplicate field` where the round
trip keeps the last value. It reports malformed JSON as `bad_request` where the
`Value::Null` path reports `unrecognized_request_shape`. Each divergence is
silent: the client sees a different code or a different accepted request
depending on lane.

## Timing windows and dependencies

None in time. The dependencies are the two upstream behaviors (map insert
replacement, derived-struct duplicate detection) and the fact that both lanes
share one typed decode function today.

## What a test must construct

A fixed corpus fed through both lanes: `{"method":0,"kind":"transform"}`,
`{"method":"transform","kind":"x"}`,
`{"kind":"transform","transform_page_id":null}`, `{"name":"x","arguments":{}}`,
duplicate `method` and duplicate `session_id` keys, unknown fields, `null` on
`Option` and defaulted fields, missing `session_id`, malformed JSON, numeric
edge cases; bodies over 1 MiB with string, numeric, array, object, and 2 MiB
`method`; bodies of exactly 32 MiB and 32 MiB plus one. Compare route, code,
and the decoded `TransformRequest` between `decode_unpaged`, `decode_via_value`,
and [assembled pages][pageapply]. The
[ingress checks](../existing-checks.md#ingress-admission-and-decode)
cover `kind` routing, the unrecognized shape, the probe class set, and one
full envelope; none covers a `null` page field, duplicates, or malformed JSON
on either lane.

The reference is a frozen test-only copy of HEAD's routing read, probe, and
`Value` decode kept under `crates/daemon/tests/` on the pattern of
`historian_truncate_differential.rs:13-58`, or a recorded corpus of expected
route, code, and decoded request per body. The live reader cannot remain the
oracle once the change replaces it.

## Investigation log

### Q: Is last-wins on duplicate top-level keys a contract or an accident?

- Sources examined: [`from_value`][fromvalue],
  [`TransformRequestWire`][wirestruct],
  [§7.5.1][wire751], upstream [`Map::insert`][mapinsert].
- Findings: Nothing in the context module states a duplicate-key rule; the
  behavior follows from `Value` construction. Synapse rejects duplicates by
  contract. The probe refuses duplicates over 1 MiB, so the two size classes
  already differ.
- Missing evidence: A written rule for routed bodies.
- Conclusion: needs human input.

### Q: Must malformed JSON keep reporting `unrecognized_request_shape`?

- Sources examined: [`handle`][handle] (`unwrap_or(Value::Null)`),
  [`unrecognized_request_error`][unrecognized], [§6.3][wire63].
- Findings: The code is a consequence of mapping parse failure to `Null`; the
  wire contract leaves routed bodies opaque and names no code for this case.
- Missing evidence: A statement of the intended code.
- Conclusion: needs human input.

### Q: Do numeric edge cases decode equivalently on both paths?

- Sources examined: the `u64`, `usize`, and `f64` fields of
  [`TransformRequestWire`][wirestruct].
- Findings: Integers above `u64::MAX`, exponents, and `-0` pass through
  `Value` number normalization before the typed decode today; a direct decode
  would hand serde the token. No differential ran.
- Missing evidence: A differential run over the numeric corpus.
- Conclusion: unresolved, needs a differential run.

[handle]: ../../../../../crates/daemon/src/lib.rs#L11805-L11827
[bytecap]: ../../../../../crates/daemon/src/lib.rs#L15472-L15488
[probe]: ../../../../../crates/daemon/src/lib.rs#L15311-L15323
[probestr]: ../../../../../crates/daemon/src/lib.rs#L15330-L15393
[class]: ../../../../../crates/daemon/src/lib.rs#L15395-L15406
[dispatch]: ../../../../../crates/daemon/src/lib.rs#L12558-L12652
[pagefields]: ../../../../../crates/daemon/src/lib.rs#L12654-L12658
[unrecognized]: ../../../../../crates/daemon/src/lib.rs#L12674-L12698
[typename]: ../../../../../crates/daemon/src/lib.rs#L12700-L12709
[pageconst]: ../../../../../crates/daemon/src/lib.rs#L745-L752
[tdispatch]: ../../../../../crates/daemon/src/lib.rs#L7887-L7903
[observed]: ../../../../../crates/daemon/src/lib.rs#L7926-L7932
[fromvalue]: ../../../../../crates/daemon/src/lib.rs#L7933-L7941
[pageapply]: ../../../../../crates/daemon/src/lib.rs#L9425-L9433
[testentry]: ../../../../../crates/daemon/src/lib.rs#L12484-L12499
[wirestruct]: ../../../../../crates/daemon/src/transform.rs#L801-L972
[typed-decode]: ../../../../../crates/daemon/src/transform.rs#L909-L972
[wiremsg]: ../../../../../crates/memory-store/src/lib.rs#L126-L143
[wireblock]: ../../../../../crates/memory-store/src/lib.rs#L250-L264
[wire63]: ../../../../host-wire-protocol.md#L308
[wire751]: ../../../../host-wire-protocol.md#L440
[mapinsert]: https://docs.rs/serde_json/1.0.151/src/serde_json/map.rs.html#127-129
[derivedup]: https://docs.rs/serde_derive/1.0.229/src/serde_derive/de/struct_.rs.html#269
