# route-and-typed-decode-are-independent-of-entry-path

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.
The discovery and investigation sections describe that baseline. Their source
links are pinned to it. The implementation evidence below describes the live code.

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


## Direct-decode evidence

Implementation base: `96709d0ef54bcfad2327878ab96e118fb8ba4969` plus the units
that precede it on the branch.
Preservation authority: [implementation ticket](https://github.com/ahrav/eidnara/issues/435)
and [parent specification](https://github.com/ahrav/eidnara/issues/350).

[`Handler::handle`][handle-live] reads one [entry probe][probe-live] from the
body before anything else: the `method` and `kind` discriminators and whether
any [`TRANSFORM_PAGE_FIELDS`][pageconst] key is present. The probe's
[map visitor][probe-visitor] mirrors the tree dispatch rather than a derived
struct: a repeated key keeps its last value, a page key counts when present
whatever its value, `null` included, each key is [classified in place][probe-key]
so no key text is retained however long it is, and every other value is skipped
through [`deserialize_any`][skipped] so every check serde_json applies to a
`Value` parse applies to the probe: the nesting limit, number range, string
escapes, and UTF-8 (`IgnoredAny` skips without those checks and would let a
body probe where the tree refuses it). A probe is therefore proof that the tree
decode parses the body. The probe's only body-proportional cost is serde_json's
scratch buffer for one escaped string at a time, released with the probe; the
probe runs before the resident reservation, as the byte-cap probe always did.

The same probe serves the byte cap: [`enforce_request_byte_cap`][cap-live] takes
it instead of running its own read, so a body over 1 MiB is probed once, and it
keeps the [class read][class-live] (an overlong `method` reads as absent for the
widening while [dispatch's read][route-resolve] takes any string `method` as
the route, so that body is admitted and then refused by shape). A body over
1 MiB that yields no probe is classed by the [lenient read][class-probe], which
skips other fields without the tree's checks as the derive does, so a
transform-class body the tree cannot parse past is still admitted under the
wider cap and refused by shape, as before; its refusal code does not move to the
1 MiB cap.

[`dispatch_body`][dispatch-body] is the one branch. When the probe names the
`transform` route with no page key, the body decodes with
`serde_json::from_slice::<TransformRequest>` and enters the
[direct lane][direct-lane]; on any decode error the body falls through to the
`Value` parse and the tree dispatch, so a refusal is always the tree decode's
refusal with the tree decode's message. Every other body takes the tree path
unchanged. The branch reports the lane it took, which the handler discards and
the tests assert. The [tree-decoded unpaged lane][tree-lane] and the
[page apply][page-apply-live] both decode their `Value` with `from_value` and
enter the same [typed handler][typed-entry] the direct lane enters; the
`request_observed_at_ms` read moved from the raw `Value` to the typed field,
which decodes to the same value wherever the typed decode succeeds. The
`handler_total` timing starts before the typed decode on both lanes; on the
direct lane that decode reads the body bytes, so the direct lane's
`handler_total` includes the byte parse that the tree lane's `Value` parse
precedes. The direct decode retains at most what the tree lane retains
([one node-copy count][copies-live] covers both lanes), and the
[peak test][t-peak] measures it below the tree decode's peak on the dense
native corpus.
[`handle_transform_for_test`][test-entry] serializes its request and enters at
the body branch, so the crate's transform tests run the direct lane.

The [corpus][t-corpus] holds 34 bodies: a valid body under `kind` and under
`method`, an unknown top-level field, `null` on an `Option` and on a defaulted
field, a wrong type, a float, an exponent, a negative and an above-`u64`
integer on integer fields, `-0` on a float field, a repeated top-level key, a
repeated discriminator, a repeated nested key, a missing required field, a
missing and an unknown serializer profile, a `null` and a lone page field, a
non-string and an overlong `method` beside `kind`, another route, trailing
bytes, malformed JSON, array, string and empty bodies, `messages` as an object,
twenty thousand values under an ignored field, an out-of-range number, a lone surrogate and invalid UTF-8 under an ignored
field, and nesting at and one past the tree's depth limit. The
[entry differential][t-entry-diff] runs every body through `dispatch_body` and
through the tree dispatch on two identical handlers and asserts the same
response with the timing block removed, or the same code and message; it pins
the ten bodies that took the direct lane and asserts the valid body was
served. The [decode differential][t-decode-diff] asserts for every body that a
probe implies the tree parses it, that where both decodes accept a body they
produce the same request (pinning the fourteen such bodies, with `-0` compared
by bit pattern), pins the bodies only the tree accepts to the three
repeated-key shapes (the derive refuses a repeated field; the handler's
fallback carries them), pins the bodies only the direct decode accepts to the
four derive-leniency shapes under an ignored field and asserts the probe
refuses each first, and decodes a two-page assembly of the valid body to the
one-slice request. The [probe test][t-probe] checks each page key with `null`,
the discriminator fallbacks, last-wins, the overlong-`method` split between cap
and route, and the depth limit found by probing the tree. The
[cap test][t-cap-live] adds a body of exactly `MAX_TRANSFORM_FRAME_BYTES`
admitted, one byte more refused, and a 2 MiB transform-class body nested past
the depth limit still admitted.

The numeric open question is answered by the run: the tree and the direct
decode share one parser and one set of visitors, so a float or exponent on an
integer field, an integer above `u64::MAX`, a negative on an unsigned field,
and `-0` on a float field give the same accept or refusal on both. The
repeated-key question stays open as a contract question; the behavior is
unchanged, last-wins through the tree.

### Focused execution, 2026-09-12

`cargo test -p daemon --locked` passed 1016 tests including the four above, the
two `dreamer_run_task_bounds_*` tests failing under full-suite load on the base
branch as well and passing in isolation;
`cargo test -p daemon --locked --features test-support --test parse_charge_covers_typed_decode`
passed; `cargo test -p daemon --locked --features direct-host-fixture --test direct_host`
passed 6, including the real unary transform through `Handler::handle`;
`bun scripts/verify-serialized-transform-pages.ts` in `packages/e2e-tests`
generated the 19-case serialized corpus and passed
`serialized_transform_corpus_preserves_host_admission_and_completion` through
the direct-host fixture, which compares the paged final `messages` bytes to the
one-slice control.

[handle-live]: ../../../../../crates/daemon/src/lib.rs#L11899-L11914
[dispatch-body]: ../../../../../crates/daemon/src/lib.rs#L12654-L12692
[probe-live]: ../../../../../crates/daemon/src/lib.rs#L15465-L15469
[probe-visitor]: ../../../../../crates/daemon/src/lib.rs#L15486-L15520
[probe-key]: ../../../../../crates/daemon/src/lib.rs#L15531-L15556
[skipped]: ../../../../../crates/daemon/src/metered_decode.rs#L403-L460
[route-resolve]: ../../../../../crates/daemon/src/lib.rs#L15560-L15565
[class-live]: ../../../../../crates/daemon/src/lib.rs#L15575-L15582
[class-probe]: ../../../../../crates/daemon/src/lib.rs#L15693-L15698
[cap-live]: ../../../../../crates/daemon/src/lib.rs#L15740-L15761
[direct-lane]: ../../../../../crates/daemon/src/lib.rs#L7969-L7981
[tree-lane]: ../../../../../crates/daemon/src/lib.rs#L7985-L8005
[typed-entry]: ../../../../../crates/daemon/src/lib.rs#L8010
[page-apply-live]: ../../../../../crates/daemon/src/lib.rs#L9520
[copies-live]: ../../../../../crates/daemon/src/metered_decode.rs#L38
[test-entry]: ../../../../../crates/daemon/src/lib.rs#L8593-L8605
[t-probe]: ../../../../../crates/daemon/src/lib.rs#L19250-L19295
[t-corpus]: ../../../../../crates/daemon/src/lib.rs#L19298-L19435
[t-decode-diff]: ../../../../../crates/daemon/src/lib.rs#L19455-L19574
[t-entry-diff]: ../../../../../crates/daemon/src/lib.rs#L19597-L19638
[t-cap-live]: ../../../../../crates/daemon/src/lib.rs#L19168-L19247
[t-peak]: ../../../../../crates/daemon/tests/parse_charge_covers_typed_decode.rs#L94-L153

[handle]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L11805-L11827
[bytecap]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L15472-L15488
[probe]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L15311-L15323
[probestr]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L15330-L15393
[class]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L15395-L15406
[dispatch]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L12558-L12652
[pagefields]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L12654-L12658
[unrecognized]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L12674-L12698
[typename]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L12700-L12709
[pageconst]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L745-L752
[tdispatch]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L7887-L7903
[observed]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L7926-L7932
[fromvalue]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L7933-L7941
[pageapply]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L9425-L9433
[testentry]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L12484-L12499
[wirestruct]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L801-L972
[typed-decode]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L909-L972
[wiremsg]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L126-L143
[wireblock]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L250-L264
[wire63]: https://github.com/ahrav/eidnara/blob/9132344/docs/host-wire-protocol.md#L308
[wire751]: https://github.com/ahrav/eidnara/blob/9132344/docs/host-wire-protocol.md#L440
[mapinsert]: https://docs.rs/serde_json/1.0.151/src/serde_json/map.rs.html#127-129
[derivedup]: https://docs.rs/serde_derive/1.0.229/src/serde_derive/de/struct_.rs.html#269
