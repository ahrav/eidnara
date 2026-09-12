# paged-body-measure-equals-declared-frame-length-and-fits-host-caps

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.
The discovery evidence and its unresolved conclusion below are historical.
The host probe in the investigation log supplies counterexamples, not a
passing preservation result.

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

### Q: What does the real host admission probe establish?

- Provenance: 2026-09-11, Linux x86-64, source
  `90f75bbe5606c4f6b52ffa3bdc4fe52dcce59253`, Bun 1.3.14, Rust 1.98.1,
  locked `serde_json` 1.0.151. Production source is unchanged.
  The [pre-probe record][prior-record] preserves the earlier coverage and
  open question.
- Construction: `buildPagedModuleTransformPayloads` receives
  `{method:"transform", session_id:"serialized-page-probe", input}` with
  two items: `{text, number, pad:"x".repeat(N)}` and
  `{text:"y".repeat(500000)}`. Starting at `N = 500000`, increase `N` by
  `targetBytes - firstPage.bytes`. Send only that non-final first page;
  each case uses a fresh route, closed after its terminal. The Unicode text
  is `é😀` followed by U+2028, U+0001, newline, quote, and backslash. Other
  texts are empty or the single indicated surrogate; ordinary numbers are 1.
- Execution: `cargo build -p daemon --example direct_host_fixture --features
  direct-host-fixture --locked` succeeds. A temporary Rust client linked
  against that build's dependencies uses `host_runtime::Client` and the
  existing direct-host `identity` and `wait_for_store` helpers. It submits
  the plugin's UTF-8 JSON bytes unchanged over the real shared-memory ring.
  It also measures `serde_json::to_vec` after parsing those exact bytes.

| Case | Pager and submitted bytes | Rust reserialized bytes | Host terminal |
| --- | ---: | ---: | --- |
| Unicode below cap | 524287 | 524287 | `ok:true, staged:true, next_expected_index:1` |
| Unicode at cap | 524288 | 524288 | Same successful stage |
| `2 ** 64`, two bytes below cap | 524286 | 524288 | Same successful stage |
| `2 ** 64`, at cap | 524288 | 524290 | `host.authority_transform_page_buffer_overflow` |
| Lone high surrogate U+D800 | 524288 | No parsed value | `host.unrecognized_request_shape` |
| Lone low surrogate U+DC00 | 524288 | No parsed value | `host.unrecognized_request_shape` |

- The temporary assertion command `bun
  /tmp/opencode/serialized-page-probe.ts` exits 1: three admitted pages stage
  and three are refused. Its Rust sender source is
  `/tmp/opencode/serialized-page-host-probe.rs`. These are local diagnostic
  artifacts, not repository regression tests. The construction above is the
  retained recipe; no benchmark or latency claim follows from this run.
- The numeric wire token is `18446744073709552000`; Rust emits
  `1.8446744073709552e+19`. The [host cap check][current-page-check] rejects
  the two-byte expansion. The tested control escapes do not inflate.
- The locked serde reader requires paired surrogates for UTF-8 strings
  (`serde_json` 1.0.151, `src/read.rs:897-973`). The high-surrogate parse
  reports `unexpected end of hex escape`; the low-surrogate parse reports
  `lone leading surrogate in hex escape`. The [host parse][current-parse]
  substitutes `Value::Null` on failure; [dispatch][current-shape] returns
  `unrecognized_request_shape`. Thus the discovery claim that strings agree
  excludes these JavaScript strings and cannot establish their acceptance.
- Limits: only first-page admission executes, not final assembly, served
  output, the live plugin hook, or the TypeScript encoder/native writer.
  After a successful `build:native`, Bun's capability probe reports
  `runtime_mechanism_unavailable`; Node 24.18.0 reports
  `node_detachment_unavailable`. Neither guard is bypassed. No carrier or
  cache is added, and no transient-body memory or serialize-once claim is
  established. Full repository checks, smoke, and landing reviews are not
  run for this blocked implementation.
- Focused baseline checks: `bun run --cwd packages/opencode-plugin test
  src/hooks/context/module-wire.test.ts src/shared/host-client/frame-channel.test.ts`
  passes 67 tests with 310 assertions. `typecheck` and `lint` pass for both
  `packages/opencode-plugin` and `packages/e2e-tests`; lint reports existing
  warnings. These unchanged-code checks do not make the admission probe pass.
- Conclusion: boundary refusal is demonstrated. Making every admitted
  lone-surrogate page acceptable while preserving its text and the Rust
  representation is contradictory. Whether the acceptance oracle should
  instead preserve that refusal requires human input. P3 remains active
  and partial; production implementation stops at this contract decision.

### Q: What does the scoped carrier campaign prove?

This section records the first working-tree candidate, not the final gate.
Its unpaged admission rule and manual Rust build were rejected during review.
The correction below supersedes those claims without rewriting these recorded
results. Links identify the maintained check families; the intermediate
candidate was not committed and has no immutable source revision.

- Approval disposition, 2026-09-11: the owner scopes host acceptance to
  Rust-valid JSON strings. Lone surrogates must preserve their exact encoded
  bytes and existing host refusal. The failed preflight above remains
  historical evidence; neither text replacement nor a parser, wire, or cap
  change is authorized.
- Implementation: [the carrier factory][carrier] serializes, copies the root
  fields into a shallow view, installs a private non-enumerable symbol
  containing the text, and freezes the root. It does not parse the text. The
  pager measures that text. The existing
  module-call and transport body parameters carry the branded object without
  another argument or wrapper. [The encoder][carrier-encoder] uses its stored
  text; ordinary objects retain the `JSON.stringify` path. Nested inspection
  edits cannot change stored text, and ordinary field names or an unrelated
  symbol with the same description cannot identify a carrier.
- Admission: [the growth bound][growth-bound] reuses `wireIntegerText` for
  i64/u64 tokens. Every other finite numeric token gets
  `max(0, 24 - token.length)` extra bytes for admission only. The locked
  `serde_json` 1.0.151 formatter calls `zmij::Buffer::format_finite`
  (`src/ser.rs:1716-1722`); locked `zmij` 1.0.21 has a 24-byte buffer
  (`src/lib.rs:90`). This bounds parse-rounding differences without adding
  another approximate Rust-number renderer. It charges actual numeric
  tokens in the serialized text, skipping quoted strings. Using the text
  rather than `String(parsedNumber)` preserves the distinction between an
  integer and raw `1e5` or `-0` tokens. This requires a text scan, not a
  blanket page margin. Packing and final-tail checks use
  the bound, while `bytes` and frame lengths remain exact wire counts.
  A quoted token that decodes to an unpaired surrogate makes the growth
  allowance zero: invalid strings retain byte-only packing rather than being
  forced into continuations by numeric growth. The supported runtimes provide
  `String.isWellFormed`; the local structural type supplies its missing
  declaration under the package's ES2022 library selection.
- Serialization: the full source body and each emitted envelope stringify
  once. The unpaged path parses nothing. A body that needs paging parses the
  full text once so every item is plain JSON, and each item's byte bound is
  computed once before the page-count loop rather than once per attempt.
  Envelopes materialize only after the page count stabilizes and are not
  parsed. Digest, scalar-skeleton, item, and continuation packing still
  serialize their own inputs. This is not a claim that the pager invokes
  `JSON.stringify` once in total. Source getters and `toJSON` are evaluated
  before packing.
- Red/green evidence: the [module transport snapshot test][snapshot-test]
  failed before implementation with two stringify calls where one was
  expected. It passes with the carrier, including source mutation, nested
  inspection mutation, a frozen root, and a literal escaped-surrogate wire
  oracle. The [pager spy][pager-spy] counts one full-body call and one result
  matching each emitted envelope. The [live hook][carrier-hook] observes the
  carrier rather than trusting a reported byte counter.
- The [18-case corpus][carrier-corpus] includes Unicode/control escapes,
  exact and refused scalar boundaries, exact intermediate/final pages,
  f64 growth on both paths, continuations, and small/paged lone surrogates.
  [The joint writer test][writer-test] reads the raw header with `DataView`
  and compares the captured `Uint8Array` with the carried UTF-8 text through
  `HostClient`, `ShmFrameChannel`, and `ProducerCursor`. The native endpoint
  is a fake; this does not verify N-API or shared-memory attachment.
- Real-host reproduction from the repository root:
  `bun run --cwd packages/e2e-tests scripts/verify-serialized-transform-pages.ts`.
  The [runner][host-runner] builds the existing fixture with `--locked`,
  compiles [the standalone Rust test][host-test] against the exact emitted
  dependencies, runs it, and removes the temporary executable. The
  [generator][host-generator] and corpus are checked-in test inputs. This
  explicit command keeps Bun out of the default Rust-only CI test lane.
  It does not depend on the TypeScript native capability probe or bypass it.
- Host result: one test passes with no skips. Nine candidate transforms complete,
  eight pages stage, five lone-surrogate requests retain
  `host.unrecognized_request_shape`, and four scalar bodies are refused by
  the pager. Exact-cap unpaged, intermediate, and final pages pass. Numeric
  growth moves an oversized intermediate item into continuations and spills
  a tight final page: its scalar-only tail has 224142 wire bytes and 224144
  Rust bytes. Another numeric tail has 120794 wire bytes and 120796 Rust
  bytes. Every successful final response's served-message bytes equal an
  unpaged control on the same route, for nine additional control calls.
  Staging ACKs are not completion proof.
  Raw `1e5` at the wire cap reserializes to 524293 bytes; raw `-0` to 524290.
  Both scalar bodies are refused before any send.
  The mixed lone-surrogate/numeric-growth case first failed the standalone
  host test because conservative packing moved the invalid item into a
  continuation. Byte-only packing for invalid strings preserves its original
  `host.unrecognized_request_shape` refusal at 524288 wire bytes.
- Receipt limit: Rust verifies byte length and SHA-256 of the exact text
  supplied by TypeScript, then submits those bytes through the Rust managed
  ring client. This is sender receipt plus real host admission/response
  evidence, not a raw-byte capture inside the host. The native-writer fake
  separately captures TypeScript writer bytes. Actual TypeScript native
  attachment remains unavailable for the reasons recorded above.
- Focused gates: 213 TypeScript tests pass across `module-wire`,
  `rust-mode-transform`, `hook`, `module-transport`, `host-client/client`, and
  `frame-channel` (1153 assertions, no skips). Both TypeScript packages pass
  typecheck and lint; existing lint warnings remain. Six `direct_host` Cargo
  tests and `cargo clippy -p daemon --tests --all-features --locked -- -D
  warnings` pass. Rust formatting passes. Full `check:repo`, bundle smoke,
  and independent landing reviews remain outer gates, not claimed here.
  Pi's typecheck and all 379 tests pass (1365 assertions, no skips).
- Memory: no process-local cache is added. On the unpaged path the original
  input coexists with one shallow root copy and the stored text. During paged
  preparation it also coexists with one parsed full-body snapshot and the
  per-page stored texts; page views are shallow copies that share item
  references with the snapshot. For full-text length S and total page-text
  length P, `2 * (S + P)` accounts for the logical UTF-16 payload bytes of
  the texts, excluding engine/object overhead. Token matches, decoded string
  probes, continuation UTF-8 item buffers, and transport frame reservations
  are additional transient storage. Page objects and
  strings survive across the series' awaits; they are not zero-cost memory
  and are not covered by the transport's publication-only byte charge. The
  full-body snapshot is preparation-local. On Bun 1.3.14 the unpaged path
  measures about half the pre-carrier cost of two stringifies across dense,
  long-string, and mixed 500-770 KiB bodies (0.85, 0.54, and 0.21 ms versus
  1.60, 1.03, and 0.43 ms); no RSS or end-to-end latency improvement is
  asserted.
- Conclusion: the approved acceptance/refusal corpus passes at the specified
  seams. P3 stays active and partial for actual TypeScript native attachment.

### Q: What do the unpaged correction and registered Cargo test prove?

- The host's [page-size check][current-page-check] runs only in the page
  handler. Unpaged transform requests use the raw 32 MiB ingress cap. The
  pager therefore retains its original `wireBytes <= 524288` fast path,
  without any Rust-reserialization allowance. The [unpaged regression
  test][unpaged-regression] failed before this correction and passes after it.
  At 524288 wire bytes, `2 ** 64`, raw `1e5`, and raw `-0` now complete
  unpaged even though Rust serializes them to 524290, 524293, and 524290 bytes.
  The 524289-byte scalar-only overflow still refuses before send.
- Paged admission keeps `F64_JSON_MAX_BYTES = 24` and reuses the integer-token
  classifier. Rust [Ryu 1.0.23 `format64`][ryu-bound] guarantees at most 24
  bytes for finite f64 values. The locked serde formatter uses zmij rather
  than Ryu and also has a 24-byte buffer, as recorded above. The Cargo test
  asserts the exact witness `-2.2250738585072014e-308` and its 24-byte length.
  No second exact number renderer is introduced. The private scanner receives
  text produced by `JSON.stringify`, not an externally supplied string body.
- `hasLoneJsonSurrogate` makes the invalid-string exemption explicit. A
  separate-pages case admits a valid numeric page first, then returns the
  existing `host.unrecognized_request_shape` for a later surrogate page.
  It cannot pass by failing an earlier valid page's cap check. Same-item
  numeric/surrogate refusals remain covered too.
- The [Cargo integration test][host-test] is auto-discovered as
  `daemon/serialized_transform_pages`; `autotests` is not disabled, so no
  manifest registration is needed. It compiles in ordinary Cargo test and
  Clippy lanes. Its actionable ignore reason names the wrapper that generates
  `EIDNARA_SERIALIZED_TRANSFORM_CORPUS` and invokes `cargo test --locked -p
  daemon --test serialized_transform_pages --features direct-host-fixture --
  --ignored --exact serialized_transform_corpus_preserves_host_admission_and_completion
  --nocapture`. The wrapper requires that named test to pass and an explicit
  `1 passed; 0 failed; 0 ignored` summary. There is no rustc/rlib discovery.
- The 19-case host run passes: twelve candidate transforms and twelve
  unpaged controls complete with equal served-message bytes; nine pages
  stage; six surrogate requests retain their refusal; one scalar body is
  refused by the pager. The test asserts page counts, exact first/final byte
  boundaries, page metadata, staging ACKs, and final responses independently.
- The [joint writer][writer-test] and [module snapshot][snapshot-test] tests
  live in the hooks family. They use the public `HostClient.connect` factory
  and channel injection, not private transport-field assignment. Generic
  client tests keep plain-object compatibility and carrier encoding. The
  extracted fake uses Node assertions, matching the portable frame fixtures;
  no checker or skip list is changed.
- The carrier has a shallow-readonly inspection view and authoritative text.
  Root fields are frozen for control-flow reads; nested inspection edits do
  not change the wire. Emitted pages are not parsed; only a body that needs
  paging is parsed, once. No unsafe public text/object pairing factory,
  cache, or extra mode is added. Only transport stringify elimination is
  claimed.
- Final gates pass: `check:repo` (5133 passes, five existing skips), bundle
  smoke, 215 focused TypeScript tests (1111 Bun assertions plus fixture Node
  assertions), six direct-host tests, focused all-feature Cargo Clippy, and
  Rustfmt. The no-default-features Cargo target compiles; its ordinary run
  reports one intentional ignored test, not an executed proof. The explicit
  host wrapper runs one test with no skips. Other full Cargo CI lanes remain
  with the outer verification owner.
- Logs are local artifacts under `/tmp/opencode/`: `serialized-pages-unpaged-red.log`,
  `serialized-pages-unpaged-green.log`, `serialized-pages-focused.log`,
  `serialized-pages-host-final.log`, `serialized-pages-check-repo-verified.log`,
  `serialized-pages-smoke.log`, and `serialized-pages-cargo.log`.
  TypeScript native attachment remains unavailable; the writer fake and Rust
  sender/host evidence retain the limits stated above. P3 remains active.

[ryu-bound]: https://docs.rs/ryu/1.0.23/ryu/raw/fn.format64.html
[unpaged-regression]: ../../../../../packages/opencode-plugin/src/hooks/context/module-wire.test.ts#L1509
[carrier]: ../../../../../packages/opencode-plugin/src/shared/host-client/serialized-json-body.ts#L1-L26
[carrier-encoder]: ../../../../../packages/opencode-plugin/src/shared/host-client/client.ts#L1516-L1521
[growth-bound]: ../../../../../packages/opencode-plugin/src/hooks/context/module-wire.ts#L637-L676
[snapshot-test]: ../../../../../packages/opencode-plugin/src/hooks/context/module-wire-frame.test.ts#L97
[pager-spy]: ../../../../../packages/opencode-plugin/src/hooks/context/module-wire.test.ts#L1427
[carrier-hook]: ../../../../../packages/opencode-plugin/src/hooks/context/hook.test.ts#L1435
[carrier-corpus]: ../../../../../packages/opencode-plugin/src/hooks/context/__tests__/serialized-transform-corpus.ts
[writer-test]: ../../../../../packages/opencode-plugin/src/hooks/context/module-wire-frame.test.ts#L55
[host-runner]: ../../../../../packages/e2e-tests/scripts/verify-serialized-transform-pages.ts
[host-generator]: ../../../../../packages/e2e-tests/scripts/serialized-transform-pages.ts
[host-test]: ../../../../../crates/daemon/tests/serialized_transform_pages.rs#L11

[current-page-check]: ../../../../../crates/daemon/src/lib.rs#L9370-L9392
[current-parse]: ../../../../../crates/daemon/src/lib.rs#L11874-L11895
[current-shape]: ../../../../../crates/daemon/src/lib.rs#L12772-L12784
[prior-record]: https://github.com/ahrav/eidnara/blob/90f75bbe5606c4f6b52ffa3bdc4fe52dcce59253/docs/properties/hot-path-optimization/latency-audit/catalog.md#L1024-L1073

[paged]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/module-wire.ts#L635-L640
[pagebytes]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/module-wire.ts#L690-L691
[pagemax]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/module-wire.ts#L9-L10
[pagecontract]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/module-wire.ts#L629-L633
[numbers]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/module-wire.ts#L57-L109
[sendseries]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L1372-L1388
[transportbytes]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L1417
[transportcall]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/module-transport.ts#L824-L836
[bodyvalid]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/module-transport.ts#L494-L501
[request]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/shared/host-client/client.ts#L534-L549
[encodebody]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/shared/host-client/client.ts#L1515-L1520
[utf8len]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/shared/host-client/frame-channel.ts#L184-L193
[utf8body]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/shared/host-client/frame-channel.ts#L195-L229
[bytecap]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/crates/daemon/src/lib.rs#L15472-L15488
[hostpage]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/crates/daemon/src/lib.rs#L735-L736
[hostpagecheck]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/crates/daemon/src/lib.rs#L9310-L9323
[t1308]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/module-wire.test.ts#L1308
[t1371]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/module-wire.test.ts#L1371
[t1391]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/module-wire.test.ts#L1391
[t181]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/shared/host-client/frame-channel.test.ts#L181
[t193]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/shared/host-client/frame-channel.test.ts#L193
