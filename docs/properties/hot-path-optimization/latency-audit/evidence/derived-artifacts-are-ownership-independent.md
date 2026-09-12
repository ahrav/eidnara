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
  [`:2910-2912`][prefix-call] when a reusable projection exists and
  [`prefix_projection_differential_enabled`][gate-prefix] is true, which is
  `cfg!(test) || EIDNARA_PREFIX_PROJECTION_DIFFERENTIAL == "1"`.
- [`reattach_messages_prefix`][reattach] rebuilds prefix shells from cached
  blocks with `WireMessage::from_parts`, so a rebuilt shell has no `original`;
  its [doc][reattach-doc] says unknown top-level fields are dropped.
- The native differential at [`:13325-13342`][native-diff] compares
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
  ([`:14431-14446`][segments-take]) and writing each through
  [`PreparedSegment::served`][segment-served] ([`:14452-14458`][segments]),
  whose `bytes()` returns `canonical_bytes`.

## Failure scenario

A projector that reuses an ingress `Arc<WireBlock>` but computes `bytes` from
a different serialization breaks `bytes == to_string(wire)` and every digest
keyed on it. A chunk-sharing decision by pointer identity diverges from the
value test at [`:13065`][chunk-eq] for a message equal by value but not by
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
The unchanged selection reference passes all 18 differential tests. The
`tool_input` versus `wire.kind()` question from the discovery snapshot is
resolved by removal: `FlatBlock` no longer carries a separate input copy, so
there is one projected input and no pair of fields to drift apart.

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
  `HarnessMessageMeta::raw` through an `Arc` clone. It is the only compiled
  production decoder; the value-slice adapters that wrap owned fixtures in
  fresh `Arc`s are test-only, so no shipped path can reintroduce that copy.
  Full-native encoding reads the shared request values without materializing
  a value array. Pi adapts to the common sidecar field without changing its
  output.
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

### Shared ingress shells

Verification date: 2026-09-11. Predecessor: `6b2c0c5f`.
Source links for earlier implementation evidence are pinned to that predecessor
where the shared-shell work moves their targets. The discovery narrative and
execution results retain their original scope. Live shared-input anchors in the
catalog and check inventory point to the shared-shell implementation; the two
synthetic delta test links also correct stale locations found during verification.

The [request collection][shell-owner] owns `Arc<IngressMessage>` values and
serializes as the same message array. Its owned `push` preserves the frozen
differential test interface. No memory-store block type or wire field changes.

The [projection builder][shell-build] owns the replay form. It discards the
message's original JSON while preserving each block's original JSON and the
effective synthetic metadata. Already canonical shells are shared directly.
The [reattachment path][shell-reattach] validates the existing identity and
frontier conditions and copies handles. Incremental projection retains the
cached handles; a changed effective synthetic status still forces full
projection rather than accepting the cached prefix.
Each [shared block view][shell-block] holds the canonical shell and its block
index. The private fields prevent callers from changing that pairing. Cloning
the view retains the shell; equality compares only the referenced wire block.

The projection cache and full-request snapshot cache retain shared shells.
[Shell accounting][shell-size] charges Arc counters, the inline ingress shell,
identity string capacity, and wire-message backing through the existing helper.
Each cache holder charges the full allocation conservatively. Within a projection,
flat-block views share the shell's backing, so it is charged once through the
message array, not once per block reference. Derived strings and tool-input
copies remain independently charged. Request prefix size entries retain
their conservative charge even when canonicalization removes original message
JSON; suffix sizes are measured at insertion. No cache or budget is added.

Characterization before production edits passed the unchanged block-original
versus rebuilt-shell expectations and effective-synthetic-status matrix. The
[pointer regression][shell-sharing] then failed on the predecessor because two
reattachments owned different shells. It passes with shared shells, including a
second projection of canonical input. The allocation test initially failed
because its expected sum omitted shell backing; the [independent sum][shell-charge]
now includes that allocation and checks exact equality instead of a tolerance.
The snapshot-fallback assertion initially selected the projection cache; explicit
eviction of both caches now proves the intended fallback path.

The large projection cache regression rejected the first implementation: full
canonical shells beside independent wire-block copies retained 284,143,614
bytes against the unchanged 201,326,592-byte budget. Sharing block backing
through the canonical shell reduces the estimate to 173,793,566 bytes, and the
unchanged cache-fit and second-pass reuse assertions pass. This is allocation
accounting evidence, not a latency comparison.

Passing focused commands use `cargo test -p daemon --lib --all-features --locked`
with filters `wire::tests` (14), `prefix` (21), `differential` (8), `native` (35),
`tail_delta` (7), `synthetic` (24), `lineage` (16), `snapshot` (16), `retained` (8),
`historian_chunk` (20), `tail_hygiene` (9), `golden` (32), `projection` (17),
`expand` (5), and `rejects` (37). All groups pass after the shared-block change.
Filters overlap. Both differential
checks are enabled by the compiled test setting throughout this campaign.
All-target/all-feature daemon Clippy with `-D warnings` and the scoped formatter
pass. The frozen `differential_goldens.rs` is byte-identical to the predecessor;
its SHA-256 is `193bc3918d1b3d06527a4fcd14bfb760b720afd36895313326ddd9964f4a3792`.

No timing baseline or speedup is claimed. The owner retires the global M0/W1/W2
measurement gate for this work. Full workspace tests, cross-process campaigns,
and independent reviews remain controller work. Historical execution evidence
above is retained; this file exceeds the method's length target to preserve it.

The projection bench corpus is built from typed parts, so its shells carry no
retained message JSON. Under shared shells that input takes the `Arc::clone`
branch on every message, a path a decoded request never takes because
`WireMessage::deserialize` always retains its JSON. A guard added to the
[bench corpus helper][bench-ingress] first failed on that shape, then passed
once the helper round-trips the corpus through `serde_json`. `projection/full`
measures the canonical-shell rebuild that a cold request pays. A separate
[`projection/reattached_prefix`][bench-reattached] cell takes the decoded corpus
and clears only each message's retained JSON through `mark_modified`, the shell
shape the projection builder produces on replay. It asserts that every block
keeps its retained JSON and that every projected block points into a corpus
shell, so it measures the share path a reattached prefix takes. A typed corpus
fails the block-retention assertion: `flatten_block` would then clone each typed
payload instead of replaying its `Value`, a cost no reattached block pays. The
`hot_path` target declares `required-features = ["bench-internals"]`, so
`cargo test -p daemon --features bench-internals --locked --bench hot_path`
exercises both cells and passes. These are shape corrections to the bench input,
not measurements.

### Shell metadata preservation and review disposition

The [reattachment test][shell-metadata] includes nonempty `origin` and
`provider_extras`, every non-synthetic `HarnessMeta` field set away from its
default, an unknown message field, and an unknown block field. It reparses the
JSON and checks typed field equality, original block retention, and exact
canonical JSON equality after removing only the unknown message field. Raw
ingress remains unchanged. The original unknown-field assertions are unchanged.

The [exact allocation fixture][shell-charge] includes nonzero origin strings,
provider namespace/tree/value allocations, and harness ID and finish strings.
Its expected sum uses capacities and explicit tree-entry costs, not the retained
charge helpers. Two temporary production mutations were detected: dropping the
canonical origin failed the typed origin assertion, and omitting the provider
extras charge produced 77,861 bytes instead of 78,135. Both mutations were
removed before the final focused runs.

Review dispositions preserve the ownership boundary. The public native Arc
fields and copying legacy decoder are unchanged from the predecessor; they are
not new blockers. The shared block's private index reaches its sole constructor
from the projection builder's `0..msg.ck.content().len()` loop. Its retained Arc
prevents another handle's copy-on-write mutation from shortening the referenced
content, and neither accessor exposes mutation. No checked optional constructor or mutation
API is needed. A short type comment documents the retained-owner mechanism.
The collection remains an owned-insertion adapter over shared-shell storage:
the immutable differential golden requires `push(IngressMessage)`. Deleting
that adapter would break the bound interface. No generic replacement or codec
conversion rewrite is introduced. `flatten_block` reads canonical metadata
without a separate synthetic argument.

The controller reports 14 full gates and six independent reviews before this
test extension. Local reruns use `cargo test -p daemon --lib --all-features
--locked` with filters `wire::tests` (14), `reattach` (16), `projection` (17),
`differential` (8), and `native` (35); all pass, with overlapping coverage.
All-target/all-feature daemon Clippy with `-D warnings` and the scoped formatter
also pass. The extension changes tests and documentation, not served behavior;
the historical evidence and bound golden remain intact.

### Canonical served bytes and fingerprint identity

Verification date: 2026-09-12. Predecessor: `e1a0d06a785763cb848f42bd65f0e95206cb426c`.
The [parent preservation contract](https://github.com/ahrav/eidnara/issues/350)
requires byte-identical served output. This resolves the sorted-key question
for this optimization without adding a wire field or a protocol version.
Earlier discovery and execution evidence above remains historical.

The [served constructor][canonical-constructor] serializes through
[serde's formatter hooks][canonical-encoder]. Those hooks record object and
field byte spans. The copy step orders fields by Rust string key and
copies scalar bytes unchanged. A key without a backslash compares as the raw
bytes between its quotes, which equals its decoded `str` order because serde
writes non-ASCII unescaped; the quotes are excluded because `"` sorts after a
space. Only an object with an escaped key decodes its keys, and it decodes
every key even when it is already in order. An unescaped-key object already
in order is not sorted, so a retained-original shell whose keys carry no
escapes allocates per object, not per key
([allocation witness][canonical-allocations]). It does not build
a `Value` tree, reserialize
values, change numeric forms, or duplicate the wire schema. Existing wire
serializers still decide field omissions and whether to replay retained JSON.
The generic encoder is private; its production entry accepts `WireMessage`,
whose fields contain no `RawValue` fragments that bypass object hooks.
The wire structs use named fields rather than flattened maps; retained JSON
maps have unique keys after parsing. Generic serializers that emit duplicate
keys or raw fragments are outside this facade's input graph.

The [identity digest][canonical-identity] streams the tuple of typed kind,
provider extras, and retained original into SHA-256. It normalizes floating
signed zero only. Integer zero remains distinct from floating zero. These
ephemeral keys are not projection hashes or persisted fingerprints. Projection
hashes and fresh fingerprints still use `to_string(WireBlock)` bytes and full
SHA-256, exposed as 64 lowercase hex characters.
The [optional original][canonical-original] cannot be `Some(Value::Null)`: its field is private,
deserialization validates `WireBlockData` before retaining it, and that decode
rejects null. Constructors and mutation methods set it to `None`. The
theoretical option/null encoding alias is therefore not a constructible block
identity and needs no extra sentinel or encoding change.

The positional map keeps the first entry for each index. Its helper uses the
predecessor's exact structural equality rather than serializing and hashing
both identities. A mismatching positional candidate forces a fresh hash rather
than a fallback search. An absent position initializes the identity index once
per message, preserving
the first candidate for each digest. Repeated served blocks reuse that same
candidate. The digest selects the candidate; the same
[equality helper][canonical-helper] that guards a positional candidate then
compares the served block against it and composes the receipt. The digest's
field list mirrors `WireBlock`'s derived equality by hand across a crate
boundary, so that re-check, not the digest, is the reuse authority, and a
reused receipt has one shape in both paths.

The [receipt witness][canonical-receipts] compares this fallback against an
inline frozen reference from predecessor `e1a0d06a`: positional-first lookup,
first structural match when the position is absent, and the structural reuse
guard. Both algorithms receive the same unpoisoned projection candidates.
The cases reverse signed-zero order, use original and fully typed zeros,
distinguish latent provider extras and original retention, and repeat an equal
block. Reused fingerprints match that reference, including candidate byte
lengths. Opposite floating-zero spellings intentionally produce a different
fresh fingerprint, while canonical served bytes remain identical. This is the
predecessor's behavior, not a new byte-hash equality guarantee. The separate
poisoned-receipt assertions prove candidate choice only.

Scratch lifetime is the constructor call. The positional map holds at most
one `usize`/borrowed-block pair per projected block; the lazy identity map holds
at most one 32-byte digest/borrowed-block pair per distinct identity. The
serializer holds two byte buffers and object/field ranges, plus decoded sort
keys for one escaped-key object at a time. None escapes into `ServedMessage`. Existing request
ownership and [retained served-message accounting][canonical-retention] remain
unchanged; no process-local cache or declared retained budget is added. Peak
scratch usage and latency are not measured by these correctness checks. The
[handler's parse charge][canonical-request-charge] stays held through dispatch
and response settlement; its decoded-tree estimate is not a proof of total
transform peak memory.

Characterization before production edits passed literal bytes, real measured
`Served` frame writes, original and typed frozen-corpus comparisons, latent
metadata edits, unknown fields, duplicate candidates, and floating signed-zero
selection. The [source guard][canonical-source] then failed on the old
`to_value` round trip. A numeric counterexample also rejected a proposed
derived-`Hash` identity: serde JSON hashes integer zero and floating zero with
the same input despite unequal values. Its distinct receipt selected candidate
5 instead of candidate 6. The streamed tuple digest passes this counterexample;
no memory-store derive or serializer change remains.

Focused checks pass with `cargo test -p daemon --lib --all-features --locked`
and filters `served_` (18),
`parked_p2_fingerprint_reuse_and_tag_frontier_match_baseline` (1),
`differential` (8), `wire::tests` (14), `native` (35), and `tag_baseline`
(7 passed, 1 manual timing test ignored). Filters overlap. The
[single-serialization counter][canonical-once] and scalar/key fixtures prove
one serde traversal and unchanged nested scalar bytes. The frozen
`differential_goldens.rs` and all existing fixture bytes remain unchanged.

No benchmarks, environment capture, or measurement-artifact edits run here.
Six independent reviews and full repository gates remain controller work.
`cargo fmt --all --check`, daemon all-target/all-feature Clippy with
`--locked -- -D warnings`, the comment-marker script, and `git diff --check`
pass. `cargo check -p daemon --release --all-features --locked` passes.
The release all-target check exposes an unchanged host-bin test defect:
`phase_cap_override_only_widens` calls a helper gated by `debug_assertions`.
The release production check does not include that test target and passes.

After restoring positional equality, focused reruns use
`cargo test -p daemon --lib --all-features --locked` with `served_` (18),
`fingerprint` (12), `differential` (8),
`parked_p2_fingerprint_reuse_and_tag_frontier_match_baseline` (1), and `native`
(35). All pass; the filters overlap. The controller reports its full 14-gate
pass before this restoration. That result does not stand in for these reruns.

### Parent integration verification

Integration base: `32829851fd2bb69e9390db7dd5be92eef5d8bc40`.
Parent: `dc0cb3bac870058f66dfd681c1d275981e4f5925`.

The merged source preserves canonical serialization, positional equality, and
the lazy absent-index digest lookup alongside the parent's dead-field removal,
shared-input accounting, and test deduplication. It compiles without restoring
removed test helpers or changing production source. The focused daemon command
`cargo test -p daemon --lib --all-features --locked` passes with filters
`served_` (17), `canonical` (21), `fingerprint` (11), `differential` (8), and
`parked_p2_fingerprint_reuse_and_tag_frontier_match_baseline` (1). Filters
overlap; the earlier execution counts remain historical.

Workspace all-target/all-feature Clippy passes with `--locked -- -D warnings`.
The bound differential golden and both historical and integrated payoff
receipts, including their JSON sidecars, are byte-identical to the parent.
No benchmark or environment capture runs during this integration. Full
repository gates remain controller work.

### Review fixes

Base: `c3287f05f98c39ce805d2da8ac204892c654f356`.

Review found two risks in the integrated source. First, the absent-index
fallback reused a receipt on a digest hit alone, and composed that receipt
inline as a second copy of the helper's tuple. The digest's field list is a
hand copy of `WireBlock`'s derived equality from another crate, so a new
`WireBlock` field would widen `==` and `to_string(block)` but not the digest,
and the fallback would reuse an unequal block's receipt. Second, the encoder
decoded every object key into a `String` before sorting, although every object
that a retained-original shell replays is in canonical order before sorting.

Both fixes were written test-first. The [source guard][canonical-source] now
requires one `fingerprint_from_projected_wire(block, Some(flat))` call in the
fallback and forbids `fingerprint_digest(` and `flat.bytes.len()` there; it
failed on the integrated source with count 0. The
[allocation witness][canonical-allocations] counts global allocation and
reallocation events across the served encoder for 1-block and 65-block
retained-original shells with 12 unescaped keys per block and caps the
per-block difference at 8; it failed on the integrated source with 19 events
per block (small shell 32, large shell 1259). After the fixes it measures 4
events per block (small shell 14, large shell 281). Both shells still equal the
`to_vec(to_value(message))` reference. The [key-order witness][canonical-keys]
fixes the raw-byte comparison's two hazards before the encoder changed: quotes
excluded so `"a"` sorts before `"a b"`, and escaped keys decoded so `"\n"`
sorts before `"!"` and `\u0001` before `\t`. It passed before and after.

The fallback now routes the digest-selected candidate through the positional
[equality helper][canonical-helper], which compares `WireBlock` values and
composes the receipt for both paths. The encoder compares unescaped keys as
raw bytes between the quotes and skips the sort when the object is already in
order; only an object with an escaped key decodes its keys. Served bytes,
candidate precedence, and first-duplicate reuse are unchanged; the encoder's
[test-support entry][canonical-test-entry] exists only for the allocation
witness.

Focused checks pass with `cargo test -p daemon --lib --all-features --locked`
and filters `served_` (18), `canonical` (22), `fingerprint` (11),
`differential` (8), `native` (35), and
`parked_p2_fingerprint_reuse_and_tag_frontier_match_baseline` (1). Filters
overlap. `cargo nextest run --profile ci -p daemon --all-targets
--all-features --locked` passes 1216 tests with 6 configuration skips under
local nextest 0.9.140. `cargo fmt --all -- --check`, workspace
all-target/all-feature Clippy with `--locked -- -D warnings`, the
comment-marker script, `git diff --check`, daemon rustdoc with
`-D warnings`, `cargo check -p daemon --release --all-features --locked`, and
`cargo check --workspace --no-default-features --locked` pass. No benchmark
or environment capture runs here; the allocation counts are event counts, not
latency.

[canonical-constructor]: ../../../../../crates/daemon/src/transform.rs#L164-L224
[canonical-encoder]: ../../../../../crates/daemon/src/served_json.rs#L112-L164
[canonical-identity]: ../../../../../crates/daemon/src/wire.rs#L885
[canonical-helper]: ../../../../../crates/daemon/src/wire.rs#L871-L880
[canonical-allocations]: ../../../../../crates/daemon/tests/served_json_passthrough_allocations.rs#L68
[canonical-keys]: ../../../../../crates/daemon/src/served_json.rs#L218
[canonical-test-entry]: ../../../../../crates/daemon/src/served_json.rs#L116-L119
[canonical-original]: ../../../../../crates/memory-store/src/lib.rs#L232-L264
[canonical-receipts]: ../../../../../crates/daemon/src/transform.rs#L13797
[canonical-retention]: ../../../../../crates/daemon/src/transform.rs#L252-L279
[canonical-request-charge]: ../../../../../crates/daemon/src/lib.rs#L11867-L11889
[canonical-source]: ../../../../../crates/daemon/src/transform.rs#L13942
[canonical-once]: ../../../../../crates/daemon/src/served_json.rs#L171

[bench-ingress]: ../../../../../crates/daemon/benches/hot_path.rs#L69-L81
[bench-reattached]: ../../../../../crates/daemon/benches/hot_path.rs#L118-L159
[shell-owner]: ../../../../../crates/daemon/src/wire.rs#L33-L88
[shell-build]: ../../../../../crates/daemon/src/wire.rs#L540-L559
[shell-block]: ../../../../../crates/daemon/src/wire.rs#L89-L116
[shell-reattach]: ../../../../../crates/daemon/src/wire.rs#L212-L241
[shell-size]: ../../../../../crates/daemon/src/retained_size.rs#L263-L273
[shell-sharing]: ../../../../../crates/daemon/src/wire.rs#L1746
[shell-charge]: ../../../../../crates/daemon/src/wire.rs#L955
[shell-metadata]: ../../../../../crates/daemon/src/wire.rs#L1705

[shared-expansion]: https://github.com/ahrav/eidnara/blob/6b2c0c5f/crates/daemon/src/lib.rs#L4157
[shared-decode]: ../../../../../crates/daemon/src/codec/opencode.rs#L61
[shared-ingress]: https://github.com/ahrav/eidnara/blob/6b2c0c5f/crates/daemon/src/lib.rs#L13028-L13080
[shared-replay-check]: https://github.com/ahrav/eidnara/blob/6b2c0c5f/crates/daemon/src/lib.rs#L20654
[shared-ingress-check]: https://github.com/ahrav/eidnara/blob/6b2c0c5f/crates/daemon/src/lib.rs#L20873
[shared-vector-charge]: ../../../../../crates/daemon/src/retained_size.rs#L77-L91
[raw-allocation-check]: https://github.com/ahrav/eidnara/blob/6b2c0c5f/crates/daemon/src/lib.rs#L20984

[tc-g2]: ../../../daemon/transform/portfolio-evaluation.md
[flatblock]: https://github.com/ahrav/eidnara/blob/6b2c0c5f/crates/daemon/src/wire.rs#L36-L64
[flatproj]: https://github.com/ahrav/eidnara/blob/6b2c0c5f/crates/daemon/src/wire.rs#L114-L127
[reattach-doc]: https://github.com/ahrav/eidnara/blob/6b2c0c5f/crates/daemon/src/wire.rs#L141-L144
[reattach]: https://github.com/ahrav/eidnara/blob/6b2c0c5f/crates/daemon/src/wire.rs#L145-L186
[diff-bytes]: https://github.com/ahrav/eidnara/blob/6b2c0c5f/crates/daemon/src/wire.rs#L329-L337
[flatten]: https://github.com/ahrav/eidnara/blob/6b2c0c5f/crates/daemon/src/wire.rs#L680-L743
[fp-reuse]: https://github.com/ahrav/eidnara/blob/6b2c0c5f/crates/daemon/src/wire.rs#L833-L842
[served-reusing]: https://github.com/ahrav/eidnara/blob/e1a0d06a/crates/daemon/src/transform.rs#L164-L216
[ser-served]: https://github.com/ahrav/eidnara/blob/e1a0d06a/crates/daemon/src/transform.rs#L293-L300
[gate-prefix]: ../../../../../crates/daemon/src/transform.rs#L2019
[assert-prefix]: ../../../../../crates/daemon/src/transform.rs#L2034
[prefix-call]: https://github.com/ahrav/eidnara/blob/6b2c0c5f/crates/daemon/src/transform.rs#L2910-L2912
[sel-item]: https://github.com/ahrav/eidnara/blob/6b2c0c5f/crates/daemon/src/transform.rs#L6352
[sel-kind]: https://github.com/ahrav/eidnara/blob/6b2c0c5f/crates/daemon/src/lib.rs#L16632
[ingress-chunks]: https://github.com/ahrav/eidnara/blob/6b2c0c5f/crates/daemon/src/lib.rs#L13028-L13080
[chunk-eq]: https://github.com/ahrav/eidnara/blob/6b2c0c5f/crates/daemon/src/lib.rs#L13065
[gate-native]: https://github.com/ahrav/eidnara/blob/6b2c0c5f/crates/daemon/src/lib.rs#L13084-L13089
[native-diff]: https://github.com/ahrav/eidnara/blob/6b2c0c5f/crates/daemon/src/lib.rs#L13325-L13342
[segments-take]: https://github.com/ahrav/eidnara/blob/6b2c0c5f/crates/daemon/src/lib.rs#L14431-L14446
[segments]: https://github.com/ahrav/eidnara/blob/6b2c0c5f/crates/daemon/src/lib.rs#L14452-L14458
[t-astro]: https://github.com/ahrav/eidnara/blob/6b2c0c5f/crates/daemon/src/lib.rs#L21222
[sidecar-inc]: ../../../../../crates/daemon/src/codec/opencode.rs#L272-L312
[sidecar-merge]: ../../../../../crates/daemon/src/codec/opencode.rs#L288-L310
[remember]: ../../../../../crates/daemon/src/codec/sidecar.rs#L67-L73
[segment-served]: ../../../../../crates/daemon/src/dispatch.rs#L50-L72
[serde-features]: ../../../../../Cargo.toml#L47
