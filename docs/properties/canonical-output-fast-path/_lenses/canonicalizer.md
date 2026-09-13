# Canonicalizer mechanism properties

## Scope and evidence provenance

This is a documentation-only discovery lane for
`/testing:property-discovery-and-catalog`, dated 2026-09-13. The workspace is
`/local/home/ahrav/scratch/eidnara`. Source evidence is pinned to HEAD
`2e4433e6b511ae74944df8a9669c428e73915d29`. Every numbered source anchor below
is checked against `git show HEAD:<path>`, not the dirty working files.
Search starts with `colgrep`; files without indexed code units are searched
in their pinned HEAD blobs instead.

The user supplies the settled [canonical-output plan][plan] as the evidence
scope. No additional external incidents or repositories are supplied. This
does not record an answer of “none” to an evidence interview. External links
inside the plan and B1 are not independently investigated in this lane.

The plan names baseline `4980f8af3bb90d58b19b80a38a227fb6363a8b33`. The plan
file exists locally but is absent from HEAD, so its headings, not line
numbers, identify its claims. Its source anchors are not copied as HEAD
evidence. For example, the frozen-corpus test is at HEAD
`crates/daemon/src/transform.rs:13760`, not the plan's baseline anchor 13757.
The plan's KTD1, KTD2, U0, and U1 supply obligations under test, not evidence
that the selected optimization is implemented.

Scope covers `encode`, `sort_fields`, formatter/writer span recording,
`copy_sorted_range`, and reachable served-message callers. No source, test,
CI, tracker, transport, or wire-contract change is proposed here. Arbitrary
`RawValue` fragments are outside the input domain. The wildcard pass and
portfolio synthesis belong to the orchestrator.

## Existing catalog and check inventory

[Latency-audit B1][b1] already owns canonical served bytes, SHA-256 identity,
fingerprint reuse, and prepared-segment byte equivalence. Its
[canonicalizer evidence][b1-evidence] and [check inventory][b1-checks] are read
before deriving the candidates below. These candidates refine mechanisms
needed to preserve B1; they do not create another byte/hash record. B1's
historical execution reports do not establish exercise of these candidates
at this HEAD. Every check in this inventory has status `unaudited`.

| Check or guard | Exact condition and message | Status |
| --- | --- | --- |
| [Formatter stack guards][span-hooks] | Object close requires an open stack entry: `serde closes an open object`. Key begin/end require an enclosing object: `serde keys belong to an object`. Value end requires one: `serde values belong to an object`. Key/value completion requires a field entry: `key began`. These are `expect` guards, not explicit span-geometry checks. | unaudited |
| [Decoded-key guard][sort] | Decoding each encoded key as `String` must succeed: `serde emits valid string keys`. The unescaped branch checks nondecreasing raw key order before sorting. Neither branch returns a changed-order flag at HEAD. | unaudited |
| [Single-serialization test][once] | Two nested `Counted` values increment one shared counter; the test asserts `count.get() == 2` and literal output bytes. There is no custom assertion message. It does not count root visits or error-prefix visits separately. | unaudited |
| [Nested-scalar test][scalars] | For each scalar/container fixture, `encode(shell)` equals `to_vec(to_value(shell))`; direct struct serialization must differ. There is no custom assertion message. | unaudited |
| [Prefix and escaped-key test][keys] | Both the `Value` and reversed-map sources equal the `Value` bytes; direct reversed serialization must differ. Assertion context is `{case}`. It does not inspect a permutation flag. | unaudited |
| [Allocation test][alloc-check] | For 1 versus 65 retained-original blocks, `(large_events - small_events) / 64 <= 8`, plus value-reference byte equality. The message is `{per_block} allocation events per passthrough block (small {small_events}, large {large_events})`. The [allocator][alloc-counter] counts allocation/reallocation events, not sizes or allocation identity. | unaudited |
| [Literal shell/segment test][shells] | Original, latent-edited, typed, and block-edited shells match literal bytes, SHA-256, identity text, measured length, and actual prepared-segment writes. There are no custom assertion messages. | unaudited |
| [Frozen-corpus test][corpus] | Retained and fully typed shells match `to_vec(to_value(message))` and prepared-frame bytes. Pairwise block identity digests match structural equality. There are no custom assertion messages. | unaudited |
| [Constructor source guard][source-guard] | The constructor text excludes `serde_json::to_value`; fallback text requires one lazy index initialization and one receipt-helper call and excludes the listed alternate lookup/receipt expressions. There are no custom assertion messages. It does not inspect traversal count inside `encode`. | unaudited |
| [Output-cache replay test][cache-check] | Replay records zero serialized items and four reused items, with canonical output equal to fresh output. There are no custom assertion messages. This is not a canonicalizer fast-path witness. | unaudited |
| [Prepared-output boundary checks][prepared-checks] | Cap-plus-one gives `BodyTooLarge` with the exact length; overflow gives `LengthOverflow`. Partial destination failure gives `Write`, accepts some bytes, and leaves a local terminal unset. Its injected I/O message is `injected serializer failure`. Length mismatch checks both lengths and leaves a local terminal unset. There are no custom assertion messages. These are downstream checks, not an `encode` serialization-error probe. | unaudited |

None found in the scoped HEAD implementation/tests: a changed-permutation
flag check, per-object sort-visit accounting, an unchanged-span identity
check against A, an error-before-finalization witness, an A ownership
identity witness, or an absolute B allocation/copy oracle. No explicit guard
checks complete span geometry. The fixed B allocation in [encode][encode]
can occur in both allocation-test sizes without violating the per-block
slope check.

## Independent attention passes

### Model pass 1: Architecture and data flow

[The host transform caller][host-caller] invokes
`transform_with_projection_cached`; [that entry][transform-entry] is not
test-gated. Default [configuration][defaults] enables compaction. On a
[cache miss][cache-branch], construction reaches
`ServedMessage::from_message`, then [from_message_reusing][constructor],
then `served_json::to_vec`. [Rendered output][rendered-caller] and
[pending passthrough output][pending-caller] also call the same constructor.
A cache hit returns its retained served message instead.

The writer extends A and updates a byte position after each write. Formatter
hooks record objects in source-start order and fields in emission order.
Successful serialization is followed by a loop over every object and an
unconditional B allocation and copy at HEAD. The constructor then hashes the
returned vector and converts it to `Arc<[u8]>`. [Prepared served
segments][segments] read those retained bytes and their length. The proposed
A transfer ends at `to_vec`'s return, not at the later Arc or transport
boundary. This pass yields the all-object and ownership obligations.

### Model pass 2: Safety

[Formatter hooks][span-hooks] record field starts after leading commas and
field ends after values. Key ends include closing quotes. The shared
position follows A's length. The stack guards establish local nesting
expectations but do not check range bounds, disjoint sibling spans, or exact
separator coverage.

[The copier][copy] relies on the object table remaining source-ordered even
when field vectors are permuted. It finds nested objects by `partition_point`,
recurses within a field, and resumes at the enclosing object's end. The
identity fast path needs a separate lemma: copying unchanged span tables is
the identity operation on A. “No keys moved” alone does not establish that
lemma. The `?` at serialization excludes incomplete tables from finalization
at HEAD. This pass yields span-identity and error-terminal obligations.

### Model pass 3: Dependencies and feature assumptions

[The root manifest][serde-feature] requests `serde_json/raw_value`. A scan
of HEAD Cargo manifests finds no `preserve_order` request. This is manifest
evidence, not a captured resolved feature graph. [WireMessage][message-ser]
and [WireBlock][block-ser] replay retained `Value` objects or serialize typed
data. Their reachable payload types use [Value-backed extras][extras] and
[typed/Value-backed block data][block-types], not `RawValue` fragments.

Map ordering explains why retained originals can be canonical under the
plan's feature assumptions; it must not authorize a fast path. KTD2 requires
deciding from emitted spans regardless of map representation or
`original().is_some()`. Requiring `preserve_order` to remain disabled is
rejected. The selected algorithm must work when insertion order changes the
emitted field sequence. Stable equal-key behavior is a private helper
obligation; duplicate-emitting generic sources are test-only and do not
expand the production `WireMessage` facade.

### Property pass 1: Data integrity

The local flag must identify the stable permutation, not escaped byte order
or output-byte equality. Every object must be processed, including objects
after the first changed object. Span geometry must account for every byte
exactly once on the identity path, including braces and commas. These are
distinct failure mechanisms even when B1 catches their final byte effect.

### Property pass 2: Protocol contracts

The plan requires one serialization and unchanged error behavior. An
additional counting serialization violates that requirement even if bytes
match. A partially serialized object cannot be finalized after an error.
The production constructor uses `expect("CK wire message values must always
serialize")`; this lane does not claim that a synthetic generic serialization
failure returns a host `encode_failed` terminal. Direct output, cancellation,
admission, and publication remain outside this canonicalizer mechanism.

### Property pass 3: Version compatibility

The optimization changes ownership, not serde's field omissions, number
forms, escaping, or the wire schema. B1 remains the byte/fingerprint
compatibility owner. Its `to_vec(to_value(message))` reference is conditional
on sorted `Value` map iteration and unique keys. With `preserve_order`, that
expression is not an unconditional canonical oracle. Mechanism checks must
use decoded-string ordering over emitted entries; duplicates must not pass
through `Value`, which can collapse them. Literal B1 witnesses remain useful
without relying on that map-order assumption. No protocol version is added.

## Candidate mechanism records

These are suggested slugs for synthesis, not separate evidence-file writes.
Confidence links point to the inline evidence above and verified source
anchors because this lane is authorized to create only this file. All five
records are safety obligations; situation witnesses below use `sometimes`.
No runtime or test execution is claimed.

### served-field-change-flag-matches-stable-permutation

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - The changed flag and a discriminating flag check are
absent from HEAD.
Guarantee: Sorting reports a change exactly when the stable decoded-key
permutation differs from the recorded field order.
Check: `always` - Save the complete pre-sort field descriptors for one
object as F, with strictly increasing, unique source starts. Decode each
key from A using its recorded key range. Independently order source indices
by `(decoded_key, original_index)` to obtain P. Require the post-sort
descriptors to equal F indexed by P, including their key-end offsets, and
require `changed == (P != [0, ..., F.len() - 1])`. Also require
`changed == !strictly_increasing(post_sort_source_starts)`, treating empty
and singleton sequences as increasing. Every evaluated object owes both
equivalences; decoded-key order, not the production offset predicate,
defines the expected permutation.
Fault/timing angle: There is no temporal fault. Escaped spellings, prefix
keys, equality, or nonunique offsets can make a plausible flag wrong.
Required faults and enabling state: Construct empty/single-field nested
objects, all six permutations of three distinct keys, ordered escaped keys,
and reversed escaped keys. Include the empty key, `a` versus `a b`, quotes,
backslashes, control characters, and Unicode. Test equal decoded keys with
distinct value tags, both already grouped and displaced by another key;
equal-key relative order must remain stable. Production reaches the helper
through [the constructor][constructor] and [encode][encode]; duplicate-key
probes use private generic serialization only and are `test-only` witnesses.
Confidence: high - [The recording and sort mechanism][sort] and
[KTD1 in the supplied plan][plan] establish the obligation. Increasing starts
identify the identity permutation only because the original starts are
unique; satisfaction by a changed implementation remains unverified.
Existing check: [Prefix/escaped tests][keys] compare bytes, and
[nested-scalar tests][scalars] cover values. Both are `unaudited`. No flag,
exhaustive permutation, or stable equal-key witness is found.
Impact: A false negative skips required reordering and threatens B1. A false
positive preserves bytes but violates the no-B requirement for ordered input.
Open questions:
- The helper needs a test-visible changed result and source-order capture.
  Neither observation exists at HEAD.

### served-order-decision-visits-every-object

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - No per-object visitation or aggregate-flag witness exists.
Guarantee: Every recorded object is sorted exactly once before the aggregate
decision can authorize returning A.
Check: `always` - On each successful encode, capture the pre-sort object IDs
as their source starts. Require exactly one `sort_fields` invocation per ID,
no extra ID, unchanged object-table order/ranges, and the expected stable
field permutation for every object. Compute each expected change from its
pre-sort decoded-key sequence, independently of the implementation flag;
require the aggregate to equal the disjunction of all expected changes.
Only an aggregate of false may authorize identity return. Per-ID checks
prevent a duplicated visit from concealing an omitted one in a total count.
Fault/timing angle: Short-circuiting after the first changed object leaves
later descendants or siblings unprocessed; checking only the root misses
deeper disorder.
Required faults and enabling state: Use an unordered typed outer object
followed by multiple unordered nested/sibling objects, with ordered and empty
objects interleaved. Typed block serializers make later disorder reachable
on [production construction][rendered-caller]. Separately construct an
ordered root with only a descendant unordered using a controlled generic
serializer. That exact shape is a `test-only` witness under the pinned
sorted-Value assumption: retained originals serialize as Values, whereas
typed message roots emit `role` before `content`. Do not claim this shape is
default-production merely because the generic helper can encode it.
Confidence: high - [The complete object loop][encode], [object recording][span-hooks],
and [typed serialization][message-ser] establish the mechanisms and
reachability distinctions. KTD1 explicitly rejects short-circuit aggregation.
Existing check: The [single-serialization literal][once] and
[nested-scalar comparisons][scalars] can expose some skipped sorts, but have
no per-object observation. Their status is `unaudited`.
Impact: The fast-path decision or B's copied content can violate B1 despite
correct sorting of the first object.
Open questions:
- Per-object invocation observation is missing. It must observe the real
  loop, not an independently implemented substitute loop.

### served-unchanged-span-copy-is-identity

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - Existing tests compare final bytes to external references,
not the unchanged copier directly to its own serialization buffer.
Guarantee: Copying a completed compact serialization with unchanged field
spans reproduces the selected source range byte for byte.
Check: `always` - Record A and the actual unsorted span tables from one
successful serialization. Check stack completion and range geometry: object
starts strictly increase; object intervals are nested or disjoint and lie
within A; each object's first/last bytes are braces. For a nonempty object O,
require `first_field.start == O.start + 1`,
`last_field.end == O.end - 1`, and
`next_field.start == previous_field.end + 1`, with a comma at every
`previous_field.end`. For an empty object require `O.end == O.start + 2`.
Each field must satisfy `field.start < key_end < field.end`, contain a valid
quoted string in `A[field.start..key_end]`, and have a colon at `key_end`.
Each object nested inside another object's range must lie wholly within one
field's value in its nearest enclosing object; source-order fields must not
overlap. Call the real `copy_sorted_range` with
the unchanged tables for the whole buffer and for complete object/field
ranges. Each result must equal
`A[range]` in bytes and length. Repeat the whole-buffer check after sorting
whenever all permutations are identity. These are invariants of every
completed, hook-visible serialization, not merely of a sampled canonical
output or a particular map feature.
Fault/timing angle: An off-by-one key/field boundary, omitted comma, duplicate
nested traversal, or incorrect resume position can make B differ from A even
when no field moves.
Required faults and enabling state: Include nested empty objects, single
fields, arrays of objects, scalar gaps around objects, and strings containing
braces, commas, colons, quotes, and escapes. Include the existing numeric
forms (`-0.0`, `1.0`, large/small exponents, signed minimum, unsigned maximum).
Nested instances occur in [production payloads][block-types]; scalar-root
and range-isolation probes are test-only. All metadata comes from the real
formatter. Do not inject arbitrary raw JSON that bypasses its object hooks.
Confidence: high - [Span boundaries][span-hooks] and [recursive copying][copy]
support the following proof obligation. For a range without objects, the
copier appends its exact slice. For an object, compact braces/commas plus
the unchanged field ranges cover the original object; recursive identity on
each field gives object identity. Resuming at the object's end skips its
already-emitted descendants. Copying the surrounding gaps then gives range
identity. This reasoning is conditional on the geometry checks above, not
an execution proof that the formatter establishes them for every input.
Existing check: [Formatter guards][span-hooks], [nested scalars][scalars],
and [literal B1 shells][shells] are `unaudited`. No direct unchanged-table
identity assertion is found.
Impact: This is the missing mechanism bridge between the old copy path and
returning A. A correct changed flag alone cannot justify eliding the copier.
Open questions:
- A test-only way to retain the actual formatter tables before sorting is
  required. It must not add a second production serialization or a second
  canonicalization implementation.

### served-serialization-is-single-pass-and-error-terminal

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - The counted success probe exists, but no failing-prefix
probe observes finalization and no execution is performed in this lane.
Guarantee: Each encode call traverses its source once and enters finalization
only after serialization succeeds.
Check: `always` - In controlled generic sources, count the root and each
distinct child emission site separately. Successful calls must match exactly
one expected traversal. Failing calls must match exactly the expected prefix
through the injected failure, with zero visits to later sites, and return
the injected error class/message. Define finalization entry immediately
after successful serialization, before extracting A, sorting, or selecting
a return path. Require zero finalization entries, zero sort visits, zero B
allocation attempts, and zero reorder bytes on error. This uses
`always(!finalization_after_error)`, not `unreachable` on a condition without
a dedicated detection point. Successful canonical and unordered cases must
both retain the one-pass count.
Fault/timing angle: Inject a serializer error before any write and after
writing a key/value prefix with an open object or nested object. Incomplete
metadata must never reach sorting or copying.
Required faults and enabling state: Counted and deliberately failing
`Serialize` sources are accepted only by private `encode` tests, as shown by
[the existing counted fixture][once]. The production facade accepts
`WireMessage`, and [its constructor][constructor] expects serialization to
succeed. The single-pass rule also governs that production call; the
constructed error state is not claimed production-reachable. This does not
model allocator failure, panic recovery, or a transport writer failure.
Confidence: high - [The single serialization followed by `?`][encode]
establishes the control-flow boundary. The error probe and entry counters
remain absent, so this is an obligation rather than tested evidence.
Existing check: [The success counter][once] and [constructor source guard][source-guard]
are `unaudited`. The [prepared-output failure test][prepared-checks] observes
a later I/O boundary and is not evidence for this failure state.
Impact: Extra traversal violates R3 and can observe stateful sources twice.
Finalizing an error prefix can panic on incomplete spans or expose partial
output instead of preserving the error.
Open questions:
- Finalization entry needs an observation that also detects an early identity
  return. A marker only inside `copy_sorted_range` misses that error path.

### served-canonical-return-retains-a-without-b

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - HEAD unconditionally allocates B; there is no fast-return
branch or absolute allocation witness for the selected requirement.
Guarantee: A successful encode whose field permutations are all identity
returns A by ownership transfer without B or replacement output-sized scratch.
Check: `always` - For an independently established identity case, capture A's
allocation identity, pointer, length N, and capacity immediately after
serialization. Before the caller's Arc conversion, require the returned
vector to have the same allocation lifetime, pointer, length, and capacity,
with no intervening deallocation, reallocation, or shrink of A. Require zero
B allocation attempts and zero logical reorder-output bytes over the entire
encode call. Corroborate this with an absolute allocator event/size ledger:
in the isolated large-payload fixture, every output-sized allocation or
reallocation must belong to A's recorded growth chain; none may create
replacement scratch. Pointer equality alone and a B-site counter alone are
insufficient. The invariant applies on every canonical miss, not only as a
difference between workload sizes.
Fault/timing angle: A hidden allocate/copy/discard, returning a cloned vector,
or shrinking A can preserve B1 bytes while defeating R1.
Required faults and enabling state: Use a retained-original `WireMessage`
with a large scalar payload, small span tables, and small keys, constructed
before measurement. Run both unescaped and sorted escaped-key cases; verify
canonicality from emitted entries, not `original().is_some()`. Record actual
sizes so metadata and decoded-key work are distinguishable from an N-byte
output buffer. The existing [test-support entry][entry] invokes the real
production facade. [Production cache misses][cache-branch] and
[pending passthrough construction][pending-caller] provide the default path;
cache hits are a separate bypass, never a witness of this saving.
Confidence: high - [The unconditional B allocation][encode],
[the caller's later Arc conversion][constructor], and U0/U1 in the
[supplied plan][plan] delimit the requirement. No saving is measured here.
Existing check: [The per-block allocation slope][alloc-check] and its
[event-only allocator][alloc-counter] are `unaudited`; neither provides
absolute B attribution, allocation lifetime, sizes, or copy accounting.
Open-coded byte/hash checks remain owned by B1.
Impact: The optimization can pass all byte checks while removing no full-size
buffer or while replacing B with equivalent hidden work.
Open questions:
- Allocation observation must avoid allocating recursively inside the global
  allocator and exclude fixture construction, reference encoding, and later
  Arc conversion. Instrumentation is missing at HEAD.
- A can retain capacity slack. Full-constructor peak residency and timing
  remain separate measurement obligations; this record cannot establish a
  latency gain or a universal reduction in peak memory.

## Fault map and required witness flags

Every name below is a proposed constant, globally unique `sometimes` flag.
Each asserts an independently constructed precondition and must fire on a
correct implementation. None asserts a wrong flag, omitted visit, unwanted
allocation, or corrupted output. No flag is recorded as fired in this lane.

| Required flag | Exact enabling situation | Candidate |
| --- | --- | --- |
| `canonical_output_field_permutations_complete` | The declared three-key permutation cases have all been presented, including identity and nonidentity orders. | `served-field-change-flag-matches-stable-permutation` |
| `canonical_output_escaped_ordered_source` | At least two emitted keys include an escape and their decoded sequence is nondecreasing. | Flag and A-return records |
| `canonical_output_escaped_disordered_source` | Emitted escaped keys contain a decoded-key inversion. | Flag record |
| `canonical_output_equal_key_probe` | A private generic source emits equal decoded keys with distinct value tags and a third key. | Flag record; test-only |
| `canonical_output_late_object_disorder` | An early emitted object and a later sibling/descendant both have decoded-key inversions before sorting. | All-object record |
| `canonical_output_ordered_root_disordered_child` | The controlled generic source emits an ordered root and a disordered descendant. | All-object record; test-only at the pinned map assumption |
| `canonical_output_identity_nested_ranges` | A completed recording contains empty/single-field objects, nested objects in arrays, and scalar gaps, with field descriptors retained in source order. | Span-identity record |
| `canonical_output_serialize_error_open_object` | The controlled source reaches an open nested object and emits a nonempty prefix before raising its planned error. | Single-pass/error record; test-only |
| `canonical_output_serialize_error_before_write` | The controlled source raises its planned error before writing any bytes. | Single-pass/error record; test-only |
| `canonical_output_large_canonical_miss` | A real facade call occurs without output-cache reuse, with independently ordered entries and payload size larger than each metadata/key-work allocation. | A-return record |
| `canonical_output_unordered_success` | A successful controlled serialization has at least one decoded-key inversion before sorting. | All-object and single-pass records |

An absent flag means missing construction or an incorrect reachability
premise, not proof of safety. No asynchronous failure schedule or bounded
recovery window is needed for these synchronous mechanism properties.

## Evaluation refinements, relationships, and handoff

The independent baseline evaluation identifies three gaps. They are dispositioned
without claiming a new independent portfolio evaluation:

1. The missing absolute B oracle becomes the A-return record's per-call
   allocation identity and event/size ledger. A constant allocation can
   disappear from a slope comparison. No fabricated `2 * output_len`
   cumulative allocation limit is introduced.
2. The missing copier identity proof becomes the unchanged-span record,
   including geometry checks and the recursive proof obligation. B1's
   output comparison is linked, not duplicated.
3. The feature-dependent `Value` oracle is explicitly conditional. Decoded
   emitted-key permutations supply the local oracle. Test-only explicit
   recursive ordering or frozen literals can support B1 under a changed map
   feature; production must not gain another serializer. The requirement
   that `preserve_order` stay off is rejected.

The dependency chain is local flag correctness, complete object visitation,
unchanged-copy identity, then A transfer. Single-pass/error-terminal behavior
constrains both finalization branches. Passing one does not subsume the next:
correct bytes cannot detect unnecessary B allocation; an accurate aggregate
cannot prove unchanged-copy identity; a copier check cannot prove that later
objects were visited. B1 remains the end-to-end compatibility record.

The canonical reorder metric counts logical bytes materialized in the
reorder destination, including reconstructed punctuation. A successful
full-buffer reorder materializes N bytes. A counter of slice-copy bytes
alone excludes synthesized braces/commas and must not be mislabeled N.
Neither metric measures physical copies caused by vector growth. A growth,
metadata allocations, escaped-key decoding, stable-sort workspace, Arc
conversion, response assembly, and transport copies remain separate costs.

Route each candidate to `/testing:test-strategy` for observation and oracle
implementation using the existing module and allocation entry point. Route
the listed test adequacy questions to `/testing:invariant-test-review` and
formatter guard strength to
`/low-level-systems:defensive-assertions-and-invariant-guards`. Reuse B1 for
served-byte, hash, fingerprint, and prepared-segment checks. The orchestrator
owns wildcard discovery, evidence-file creation, and fresh portfolio review.
No tests, benchmarks, builds, or implementation verification gates run here.

## Verified HEAD anchors

[plan]: ../../../plans/2026-09-13-0030-perf-canonical-output-direct-frame-plan.md
[b1]: ../../hot-path-optimization/latency-audit/catalog.md#derived-artifacts-are-ownership-independent
[b1-evidence]: ../../hot-path-optimization/latency-audit/evidence/derived-artifacts-are-ownership-independent.md#canonical-served-bytes-and-fingerprint-identity
[b1-checks]: ../../hot-path-optimization/latency-audit/existing-checks.md#shared-input-equivalence
[span-hooks]: ../../../../crates/daemon/src/served_json.rs#L24-L81
[copy]: ../../../../crates/daemon/src/served_json.rs#L84-L109
[entry]: ../../../../crates/daemon/src/served_json.rs#L111-L119
[encode]: ../../../../crates/daemon/src/served_json.rs#L121-L142
[sort]: ../../../../crates/daemon/src/served_json.rs#L144-L164
[once]: ../../../../crates/daemon/src/served_json.rs#L170-L193
[scalars]: ../../../../crates/daemon/src/served_json.rs#L195-L215
[keys]: ../../../../crates/daemon/src/served_json.rs#L217-L252
[alloc-counter]: ../../../../crates/daemon/tests/served_json_passthrough_allocations.rs#L10-L32
[alloc-check]: ../../../../crates/daemon/tests/served_json_passthrough_allocations.rs#L34-L86
[constructor]: ../../../../crates/daemon/src/transform.rs#L155-L224
[transform-entry]: ../../../../crates/daemon/src/transform.rs#L1771-L1819
[pending-caller]: ../../../../crates/daemon/src/transform.rs#L6690-L6719
[cache-branch]: ../../../../crates/daemon/src/transform.rs#L10405-L10432
[rendered-caller]: ../../../../crates/daemon/src/transform.rs#L11263-L11315
[shells]: ../../../../crates/daemon/src/transform.rs#L13713-L13757
[corpus]: ../../../../crates/daemon/src/transform.rs#L13759-L13794
[source-guard]: ../../../../crates/daemon/src/transform.rs#L13942-L13969
[cache-check]: ../../../../crates/daemon/src/transform.rs#L28295-L28320
[host-caller]: ../../../../crates/daemon/src/lib.rs#L8424-L8499
[defaults]: ../../../../crates/daemon/src/config.rs#L116-L125
[segments]: ../../../../crates/daemon/src/dispatch.rs#L41-L72
[prepared-checks]: ../../../../crates/daemon/tests/prepared_output.rs#L131-L234
[message-ser]: ../../../../crates/memory-store/src/lib.rs#L99-L162
[block-ser]: ../../../../crates/memory-store/src/lib.rs#L232-L279
[extras]: ../../../../crates/memory-store/src/lib.rs#L55-L56
[block-types]: ../../../../crates/memory-store/src/lib.rs#L326-L457
[serde-feature]: ../../../../Cargo.toml#L47
