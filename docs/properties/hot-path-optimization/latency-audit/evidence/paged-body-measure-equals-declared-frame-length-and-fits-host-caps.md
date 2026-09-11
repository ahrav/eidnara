# paged-body-measure-equals-declared-frame-length-and-fits-host-caps

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

The audit counts two `JSON.stringify` passes over the transform body before
send: one to measure for paging, one to encode the frame. A serialize-once
change must keep the paging decision and the declared frame length in
agreement, and both must stay inside the host's caps, which measure a
different serialization.

## Evidence trail

- [`buildPagedModuleTransformPayloads`][paged] measures
  `Buffer.byteLength(JSON.stringify(body))` and returns the body unpaged when
  the count is at most [`MODULE_PAGE_MAX_BYTES`][pagemax] (512 KiB). Each
  emitted page carries `bytes: Buffer.byteLength(JSON.stringify(page))`
  ([`:690-691`][pagebytes]); [`ModuleTransformWirePage.bytes`][pagecontract]
  is documented as the exact UTF-8 length.
- The pass sends each `page` unchanged through `callModule`
  ([rust-mode-transform.ts:1372-1388][sendseries]) and adds `bytes` to
  `timings.transportBytes` ([`:1417`][transportbytes]).
- [`HostModuleTransport.call`][transportcall] passes the body to
  [`HostClient.request`][request], where [`encodeBody`][encodebody] runs
  `JSON.stringify(body)` again and hands the text to
  [`utf8FrameBody`][utf8body].
- [`utf8ByteLength`][utf8len] replaces lone surrogates with U+FFFD before
  `Buffer.byteLength`; [`writeUtf8`][utf8body] emits U+FFFD for a lone
  surrogate and throws `RangeError("UTF-8 producer length mismatch")` when the
  written count differs from the declared `byteLength`.
- The only body check before send is [`isModuleCallBodyValid`][bodyvalid], a
  field test on `method`.
- The host enforces [`MAX_FACADE_FRAME_BYTES`][bytecap] (1 MiB) and, for the
  transform class, `MAX_TRANSFORM_FRAME_BYTES` (32 MiB) on the raw body
  length, and enforces [`TRANSFORM_PAGE_MAX_BYTES`][hostpage] (512 KiB) on
  `serde_json::to_vec(&request).len()` at [lib.rs:9310-9323][hostpagecheck],
  discarding the series with `buffer_overflow` on a violation.
- [module-wire.ts:57-109][numbers] documents where `JSON.stringify` and
  `serde_json` render numbers differently.
- [module-wire.test.ts:1308][t1308] and [:1371][t1371] assert `bytes` equals a
  later stringify on the unpaged and paged paths;
  [frame-channel.test.ts:181][t181] and [:193][t193] assert the writer emits the
  declared count for lone surrogates; [:1391][t1391] pins the pageable field
  list.

## Failure scenario

A serialize-once change measures text T, then hands the frame channel an
object that re-serializes to T' (a mutated body, a page altered after
measurement, or a different renderer). The header declares `|T|` and the
writer emits `|T'|`, so `writeUtf8` throws and the request fails locally, or
a length mismatch reaches the wire. Separately, a page that measures at
512 KiB under `JSON.stringify` re-serializes longer under `serde_json` on the
host, which refuses it with `buffer_overflow` and fails the whole series.

## Timing windows and dependencies

None in time. The equality depends on the same object reaching both
`JSON.stringify` calls unchanged, and on `JSON.stringify` never emitting a
lone surrogate (it escapes them), so `utf8ByteLength`'s replacement is a
no-op for serialized JSON. The host-cap clause depends on the host measuring a
re-serialization of the parsed page, not the wire bytes.

## What a test must construct

A body with a lone surrogate in a string field, run through
`buildPagedModuleTransformPayloads`, `encodeBody`, and `writeUtf8` in one
path, asserting `bytes === byteLength === written`. A body above 512 KiB to
exercise paging, and an unpaged body at the 512 KiB boundary whose `f64`
fields render longer under `serde_json`, checked against
`TRANSFORM_PAGE_MAX_BYTES` on the host side. The
[plugin checks](../existing-checks.md#plugin-pre-send) cover the two
stringify equalities and the writer separately; none joins them or builds a
boundary body.

## Investigation log

### Q: Can a body pass the plugin's 512 KiB measure and fail the host's?

- Sources examined: [`buildPagedModuleTransformPayloads`][paged], the host
  check at [lib.rs:9310-9323][hostpagecheck], the number-format notes at
  [module-wire.ts:57-109][numbers].
- Findings: The plugin measures `JSON.stringify` output; the host measures
  `serde_json::to_vec` of the parsed page. The two agree on structure and
  strings but can differ on `f64` rendering, so a page within a few bytes of
  the cap can measure differently on each side. Whether any real transform
  body reaches that margin is not shown.
- Missing evidence: A constructed boundary body run through both measures.
- Conclusion: unresolved, needs a boundary construction with `f64` fields.

[paged]: ../../../../../packages/opencode-plugin/src/hooks/context/module-wire.ts#L635-L640
[pagebytes]: ../../../../../packages/opencode-plugin/src/hooks/context/module-wire.ts#L690-L691
[pagemax]: ../../../../../packages/opencode-plugin/src/hooks/context/module-wire.ts#L9-L10
[pagecontract]: ../../../../../packages/opencode-plugin/src/hooks/context/module-wire.ts#L629-L633
[numbers]: ../../../../../packages/opencode-plugin/src/hooks/context/module-wire.ts#L57-L109
[sendseries]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L1372-L1388
[transportbytes]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L1417
[transportcall]: ../../../../../packages/opencode-plugin/src/hooks/context/module-transport.ts#L824-L836
[bodyvalid]: ../../../../../packages/opencode-plugin/src/hooks/context/module-transport.ts#L494-L501
[request]: ../../../../../packages/opencode-plugin/src/shared/host-client/client.ts#L534-L549
[encodebody]: ../../../../../packages/opencode-plugin/src/shared/host-client/client.ts#L1515-L1520
[utf8len]: ../../../../../packages/opencode-plugin/src/shared/host-client/frame-channel.ts#L184-L193
[utf8body]: ../../../../../packages/opencode-plugin/src/shared/host-client/frame-channel.ts#L195-L229
[bytecap]: ../../../../../crates/daemon/src/lib.rs#L15472-L15488
[hostpage]: ../../../../../crates/daemon/src/lib.rs#L735-L736
[hostpagecheck]: ../../../../../crates/daemon/src/lib.rs#L9310-L9323
[t1308]: ../../../../../packages/opencode-plugin/src/hooks/context/module-wire.test.ts#L1308
[t1371]: ../../../../../packages/opencode-plugin/src/hooks/context/module-wire.test.ts#L1371
[t1391]: ../../../../../packages/opencode-plugin/src/hooks/context/module-wire.test.ts#L1391
[t181]: ../../../../../packages/opencode-plugin/src/shared/host-client/frame-channel.test.ts#L181
[t193]: ../../../../../packages/opencode-plugin/src/shared/host-client/frame-channel.test.ts#L193
