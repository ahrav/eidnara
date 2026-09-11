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

## Implementation evidence

The preceding discovery snapshot is retained at its stated baseline. Current
checks and decision provenance are in [shared selection and pressure
accounting](shared-selection-and-pressure-accounting.md).

Both production selection constructors borrow the projected wire input.
The pointer-identity test fails on the clone-based baseline and passes on
the borrowed representation, including a selection clone and historian input.
The unchanged selection reference passes all 18 differential tests.

The sidecar test compares full and incremental order, metadata, and pins over
three generations with repeated IDs. It also checks sparse prefixes: map
membership is valid only when every copied order entry has metadata; otherwise
the original order scan preserves missing-metadata behavior. This resolves
the two-form pin question for the tested cases. Sorted-key protocol intent
and the production differential-panic policy remain outside this change.

### Shared native prefix

Verification date: 2026-09-11. Predecessor: `6bc7524b`. The discovery evidence
above retains its baseline; the links in this subsection identify the shared
native implementation.

- [Tail expansion][shared-expansion] copies `Arc<Value>` handles from native
  ingress chunks or the full request snapshot. The request wire decoder also
  owns `Arc<Value>` values. JSON fields and serialization remain unchanged.
- [Shared decode][shared-decode] borrows parts and retains each envelope in
  `HarnessMessageMeta::raw` through an `Arc` clone. The value-slice decoder
  keeps its entry interface and delegates to that same decoder. Full-native
  encoding reads the shared request values without materializing a value
  array. Pi adapts to the common sidecar field without changing its output.
- [Ingress accounting][shared-ingress] retains the value-equality test when
  sharing an encoded chunk. An unequal encoded output cannot become the raw
  ingress prefix. Request accounting uses request allocation sizes, not the
  sizes of equal encoded values whose capacities can differ. Reattached
  prefix charges are reused; only the suffix needs a retained-size walk.
  Vector capacity, pointed-to values, and strong/weak counters are charged.
  Sidecar estimates include the raw value's Arc header. Independent cache
  owners conservatively charge shared values; cache budgets are unchanged.
  Within a native snapshot, ingress and encoded chunks deduplicate by pointer.
  Sidecar metadata uses a separate serialized-size heuristic, not an
  allocation-based raw-value charge; the accounting policy below covers their
  overlap.
- [Complex replay][shared-replay-check] constructs synthetic TODO and frozen
  tool pairs, signed reasoning, a compaction marker, and an appended tail.
  Fresh input and a real reattached delta produce equal native bytes. Decoded
  values, sidecars, and projected identity bytes agree. Reattachment and
  sidecar metadata retain pointers; the incremental encoder handles at most
  two tail messages. A cloned shared request replays with zero encoded
  messages. The compiled test setting enables the native differential. The
  corrupt-frontier and corrupt-sidecar-key negative controls verify detection.
  The full encoder used by that self-check still traverses the full input.
- [Ingress-core checks][shared-ingress-check] distinguish equal output reuse
  from unequal output, prove snapshot-fallback pointer identity after native
  cache eviction, and compare cached request charges with a full size walk.

Characterization ran before production edits: the complex replay, four
frozen differential tests, and acknowledged-prefix test passed. Extending
complex replay to real expansion and fresh/full comparisons also passed.
The added reattachment pointer assertion then failed on the predecessor's
deep copy and passed with shared values. The request-accounting assertion
also caught the use of equal output allocations' smaller capacities.

Focused checks passed with `cargo test -p daemon --lib --locked`: `native`
(34 tests), `codec::` (40), `differential_goldens` (4), `tail_delta` (7,
including the giant degraded snapshot), `differential_assert` (2 negative
controls), the two extended sharing tests, the snapshot generation/LRU test,
and `snapshot_lease_budget` (2). The frozen
`crates/daemon/src/differential_goldens.rs` remains byte-identical to the
predecessor. Scoped all-target/all-feature Clippy and workspace format checks
pass. Full workspace gates and cross-process campaigns are not claimed here.
No timing comparison is required or claimed.

### Warm charge and sidecar accounting checks

The complex replay checks the real two-value frontier's warm request charge
against vector capacity, Arc headers, and a fresh walk of its own values using
the same size estimator, independently of cached charges. Omitting
the prefix sum produced 772 instead of 2564 bytes; doubling it produced 4356.
Both mutations failed the added assertion and were removed.

[Shared-vector accounting][shared-vector-charge] folds supplied pointee sizes
into the vector and Arc-header charge. Cold requests supply measured sizes;
warm ingress supplies cached prefix sizes and measures each suffix value
while constructing its chunk. The helper adds no allocation, deep traversal,
cache, or optional mode. The sidecar's serialized-size estimate stays separate.

The [raw-allocation regression][raw-allocation-check] constructs 4096 scalar
values in retained provider data. The raw Arc allocation charge is 131,870
bytes on the checked target, larger than the sidecar's entire serialized-size
estimate. The raw pointer is shared by ingress and sidecar. Seeding
`charged_values` from that pointer, even after checking `sidecar_sizes`,
removed all 131,870 bytes of its ingress charge and failed the regression.

The cache therefore keeps the allocation-based ingress/chunk charge alongside
the sidecar estimate, including when they retain the same raw Arc. This is
intentional conservative overlap within one snapshot, not only across cache
budgets. A sidecar size entry proves that an estimate exists; it does not prove
that the estimate covers the raw allocation. Deduplication across these two
accounting domains requires a sidecar estimate that covers raw storage.
The regression also verifies that ingress and encoded output count a shared
pointer once, charge equal-but-distinct allocations separately, and retain
those rules after optional sidecar trees and sizes are cleared. Independent
request charges and sidecar-only Arc header/Value costs remain intact. The
retention guards and cache capacities are unchanged.

Focused reruns passed: `native` (35), `tail_delta` (7), `differential_assert`
(2), `differential_goldens` (4), `codec::` (40), and `retained_size` (2), all
with `cargo test -p daemon --lib --locked`. Scoped all-target/all-feature
Clippy, format, comment-marker, and diff checks passed. Full gates require a
controller rerun after these edits; earlier execution evidence above remains
historical.

[shared-expansion]: ../../../../../crates/daemon/src/lib.rs#L4156
[shared-decode]: ../../../../../crates/daemon/src/codec/opencode.rs#L56
[shared-ingress]: ../../../../../crates/daemon/src/lib.rs#L13027-L13079
[shared-replay-check]: ../../../../../crates/daemon/src/lib.rs#L20653
[shared-ingress-check]: ../../../../../crates/daemon/src/lib.rs#L20872
[shared-vector-charge]: ../../../../../crates/daemon/src/retained_size.rs#L57-L70
[raw-allocation-check]: ../../../../../crates/daemon/src/lib.rs#L20983

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
[gate-prefix]: ../../../../../crates/daemon/src/transform.rs#L2004-L2011
[assert-prefix]: ../../../../../crates/daemon/src/transform.rs#L2021-L2036
[prefix-call]: ../../../../../crates/daemon/src/transform.rs#L2910-L2912
[sel-item]: ../../../../../crates/daemon/src/transform.rs#L6352
[sel-kind]: ../../../../../crates/daemon/src/lib.rs#L16629
[ingress-chunks]: ../../../../../crates/daemon/src/lib.rs#L13027-L13071
[chunk-eq]: ../../../../../crates/daemon/src/lib.rs#L13058
[gate-native]: ../../../../../crates/daemon/src/lib.rs#L13073-L13080
[native-diff]: ../../../../../crates/daemon/src/lib.rs#L13316-L13333
[segments-take]: ../../../../../crates/daemon/src/lib.rs#L14428-L14443
[segments]: ../../../../../crates/daemon/src/lib.rs#L14448-L14454
[t-astro]: ../../../../../crates/daemon/src/lib.rs#L20954
[sidecar-inc]: ../../../../../crates/daemon/src/codec/opencode.rs#L258-L302
[sidecar-merge]: ../../../../../crates/daemon/src/codec/opencode.rs#L278-L300
[remember]: ../../../../../crates/daemon/src/codec/sidecar.rs#L67-L73
[segment-served]: ../../../../../crates/daemon/src/dispatch.rs#L50-L72
[serde-features]: ../../../../../Cargo.toml#L47
