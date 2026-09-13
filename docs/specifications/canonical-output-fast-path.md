# Canonical Output Buffer Removal

## Problem

Serving an unchanged message serializes it into buffer A, then allocates buffer
B and copies the canonical result into B even when every object's fields are
already ordered. This adds one output-sized allocation and N logical reorder
bytes for an N-byte canonical cache miss. Serialized-output cache hits bypass
the canonicalizer and cannot benefit from removing this copy.

## Outcome

Return A by ownership transfer when no object's field order changes, including
objects with escaped keys. Allocate B only when some object needs reordering.
Preserve served bytes, fingerprints, lengths, errors, cache ownership, and output
publication. Retain one canonicalizer and the existing owned-response writer.

This is an internal optimization, not a wire change or transport migration. It
adds no protocol version. No allocation saving, latency improvement, test result,
implementation completion, or shipment authorization is claimed by this spec.

## User Stories

1. As a caller receiving unchanged messages, I receive identical canonical JSON
   without an unnecessary intermediate allocation and copy on canonical misses.
2. As a caller receiving typed or edited messages, I retain recursive object
   ordering, array order, scalar forms, unknown-field behavior, and fingerprints.
3. As a caller encountering refusal, cancellation, or writer failure, I retain
   the same diagnostics and terminal behavior without partial publication.
4. As a maintainer, I can distinguish a deterministic allocation saving from
   constructor residency, transform cost, and actual host latency, and review
   each landable result independently.

## Constraints and Invariants

### Preserved requirements

- **R1:** Already-canonical messages return A without B allocation or canonical
  reorder copying, including sorted escaped-key objects. Escaped-key decoding
  and sorting workspace are not promised allocation-free.
- **R2:** An unordered object anywhere in the tree requires the existing recursive
  reorder path. Preserve unknown fields according to existing retained-shell and
  edit semantics, array order, omissions, strings, Unicode, numbers, and block
  fingerprints byte for byte. This does not promise retention of fields that the
  existing typed-edit path intentionally drops.
- **R3:** Use one production serialization pass and one canonicalization
  implementation. Add no full serialization to count length, alternate wire
  serializer, compatibility layer, adapter, feature flag, or migration path.
- **R4:** Preserve output-length calculation, reservation order, cancellation,
  writer errors, cache ownership, and transport publication. Add no direct-output
  caller and change no public output API.
- **R5:** Retain before/after evidence for allocation and reallocation events,
  cumulative requested bytes, peak live bytes, explicit logical copy bytes, CPU
  time, and latency. Separate canonicalizer, full message construction, transform,
  and transport costs.

Repository instructions and the normative host wire contract remain authoritative.
Implementation requires an authorized isolated branch containing the plan's
baseline, followed by revalidation of changed surfaces. Do not implement against
the older checkout described in the plan or overwrite unrelated work.

### Property-informed obligations

The companion catalog gives each obligation an exact check, enabling states,
evidence, reachability classification, and test handoff. These are claims under
test, not proof that the optimization exists or passes.

| Property | Exact obligation and specification use |
| --- | --- |
| `served-field-change-flag-matches-stable-permutation` | For every object, the changed flag equals whether stable decoded-key ordering changes the recorded field sequence. Equal keys retain their relative order; increasing post-sort source starts identify the identity permutation because original starts are unique. This constrains branch selection and its oracle. |
| `served-order-decision-visits-every-object` | Every recorded object is processed before deciding whether A can be returned. The aggregate equals the disjunction of all per-object changes without skipping later objects. Late-sibling and descendant witnesses discriminate short-circuit errors. |
| `served-unchanged-span-copy-is-identity` | Applying the existing copier to completed, unchanged formatter tables reproduces the corresponding bytes and length of A. A test-only observation of this production lemma justifies omitting the copier; it is not a second production implementation. |
| `served-serialization-is-single-pass-and-error-terminal` | Controlled success sources are visited once. A failing source returns its original error after the expected visit prefix, with no sorting, identity finalization, B allocation, or reorder output. The injected failure is test-only; the production `WireMessage` constructor still expects serialization success. |
| `served-canonical-return-retains-a-without-b` | On an independently established canonical miss, returned allocation identity, length, and capacity remain A's, with no B, replacement output-sized scratch, or reorder-output bytes. Pointer equality alone is insufficient evidence. |
| `served-output-cache-hit-skips-construction` | Selecting a clean matching positive cache entry reuses its owned artifacts without constructing or canonicalizing that item. Cache-hit statistics alone do not prove constructor bypass; test-reference work must be outside the observed interval. |
| `prepared-output-diagnostics-preserved` | Successful writes match measured lengths. For the same source variant, cap, and write partition, failures retain their variants, codes, length fields, reservation order, and cancellation cuts. A local writer assertion is not evidence of ring publication. |
| `served-canonicalization-campaign-reaches-risk-classes` | The finite campaign must construct its declared ordered/disordered escaped-key, prefix/Unicode, nested/sibling, empty/singleton, and serialization-error preconditions. Coverage observes the enabling cases, never corruption or forbidden allocation. |
| `served-cache-campaign-reaches-miss-and-hit` | The campaign must construct canonical misses, typed/edited disorder, positive warm hits, and distinct completed-output replay. One cache counter cannot stand in for all populations. |

Reuse `derived-artifacts-are-ownership-independent` for broad canonical-byte,
canonical-hash, block-fingerprint, retained-field, and served-segment equivalence.
Do not create a competing byte-identity contract. Existing arena admission and
atomic-copy properties remain unchanged. Direct-frame T3/T4 obligations stay with
HP1; this work neither delivers nor exercises them. Invalidated measurement
records remain invalidated; R5 is the measurement obligation for this scope.

### Ownership and resource boundaries

A remains a growable vector. Span metadata retains object ranges, a nesting
stack, and field ranges with encoded-key ends. Metadata remains proportional to
objects, fields, and depth. Preserve linear sortedness checks and the existing
comparison-based stable sorting policy; do not claim metadata or escaped-key
sorting allocations disappear.

Removing B removes N logical reorder-output bytes, including reconstructed
punctuation. It does not remove A's growth, typed-field cloning, block-receipt
serialization, the later `Arc<[u8]>` allocation/copy, response assembly, or arena
publication copying. These buffers have different lifetimes; they are not four
simultaneously live whole-response buffers. Reallocation events do not measure
physical copying.

The existing host path shifts bodies below 16 KiB in place to prepend the header;
larger bodies use split header/body vectors. Preserve both paths and account for
their copies separately from canonical reordering.

Returning A can retain more spare capacity than B. Measure its capacity/length
ratio and peak live bytes across the full constructor, including receipt
construction, hashing, and Arc conversion. Do not silently add `shrink_to_fit`:
it can restore the reallocation and copy being removed. Existing retained-cache
accounting is not a bound on transient constructor residency or every live owner.

### Stop conditions

Stop implementation on any byte mismatch, changed error behavior, added full
serialization pass, or need for transport redesign. Do not weaken requirements
to declare completion. Stop and reconsider the plan before any unsafe transport
scope expansion. Investigate increased full-constructor peak residency before
landing. Investigate unresolved or adverse CPU/latency evidence before claiming
speedup; no detectable latency gain must be reported as such, not hidden.

## Implementation Decisions

### Canonicality comes from emitted spans

Keep `sort_fields` private and change its result to mean **field order changed**.
Fewer than two fields return false before key decoding. For unescaped keys, reuse
the existing sortedness result; return false when ordered, otherwise use the
existing sort and return true. Compare UTF-8 key bytes without their quotes so
prefixes such as `a` and `a b` retain decoded-string order.

For escaped keys, preserve the existing `sort_by_cached_key` operation. After
sorting, test whether field-start offsets are strictly increasing. Starts are
recorded after commas, are unique, and increase in source order. An increasing
permutation is therefore exactly the identity permutation. Add no re-cloned key
set, permutation vector, replacement sort, or raw-escape comparison.

Call this operation for every object and accumulate changes without
short-circuiting. Do not use `.any(...)` or `changed || sort_fields(...)`. If no
field moved, return A before allocating B. Otherwise retain the existing
`copy_sorted_range` implementation. Preserve `to_vec` and all its callers.

Canonicality is not inferred from `original().is_some()`. Retained originals
replay parsed `Value` objects, not original raw JSON text. Sorted maps explain
the expected miss population under the recorded feature set, but correctness
must not depend on `preserve_order` remaining disabled. Typed message shells and
tagged blocks can emit noncanonical declaration order. Do not reorder type
fields: struct-order serialization also feeds block fingerprints.

Keep the existing accepted input domain. The `raw_value` feature does not make
arbitrary raw fragments canonical, and formatter hooks cannot inspect their
interiors. Do not broaden the facade to arbitrary `RawValue` input. Prepared
Exact receipt bytes remain verbatim and are never recanonicalized.

### Retain contiguous cached bytes and owned publication

Keep canonical message bytes in the retained cache. SHA-256 can consume chunks;
contiguity is chosen for cache replay and prepared byte segments, not required
by hashing. Direct span emission into a final sink is not selected: retaining A
and spans would replace the cache representation and repeat traversal on replay,
while also producing B for the cache would defeat the saving. No production
one-shot consumer earns a new generic writer API.

Reordering preserves encoded tokens and punctuation counts, so message length
remains A's length. Add no counting serialization or separate span-length walk.
Preserve Exact length checks and existing generic/envelope count/write passes.
Transform length includes cached message lengths, envelope bytes, brackets,
keys, and commas; an empty list contributes no message commas. Preserve the
first-over-cap `CountingWriter` diagnostic rather than substituting the eventual
full size or changing write partitions.

Native full/delta values remain in the envelope. `NativeEncodedChunk` retains
`Arc<Value>`, not reusable encoded bytes or a known byte length. Do not assume
the envelope is small or pre-encode payload-sized components before admission.

Owned reservation and direct exact-length publication have different contracts.
Owned output supports bounded writes and resident-capacity charge adjustment;
`output_from_writer(exact_len, ...)` fixes the header and committed length. Never
pass an upper bound as `exact_len`. Exposing lower-level bounded commit would
require coordinated header, cap, charge, and completion redesign outside scope.

Both output reservation APIs charge the body plus the 21-byte header before
returning and wait with cancellation/deadline checks. Owned output allocates
after charge, writes synchronously, and transfers bytes and charge to the queue.
Admission timeout or generation loss can close the connection; preserve that
behavior rather than promising request-only failure.
The direct API instead retains a `Send + 'static` callback that may outlive the
handler and runs synchronously on the endpoint thread while holding a ring
reservation. The reservation itself does not cross threads or awaits. Captured
message trees, metadata, and spare capacity need their own lifetime/admission
solution; body-plus-header charge or decoded-request charge does not establish
that bound. Pre-encoding and charging afterward is not a solution.

Preserve standard `write_all` behavior, bounded-writer accepted-byte accounting,
and atomic-width arena copies, including wraparound. Never expose references
into peer-writable memory or replace those copies with ordinary `memcpy`.
Pre-publication errors, panic, overflow, underfill, and expiry abort direct
reservations; partial bytes are not frames. Cancellation before queueing and
generation retirement can discard handles, but queued winning responses are
not guaranteed retractable. Commit publishes the frame; a later wake failure can
leave it published and quarantine the ring. There is no post-commit rollback
promise. Preserve the distinction between handler-side encoding failure and
direct publication failure that retires a connection; do not blindly re-execute
after an unknown outcome or infer universal recovery from completed-page replay.

The canonicalizer change occurs before output reservation and adds no callback,
lease, cancellation point, `Send` requirement, or transport dependency.

### Milestone boundaries and landing rules

Preserve these settled boundaries and ordering. They are milestone contracts,
not implementation tickets or a requirement for one PR per milestone.

| Milestone | Required result | Dependency |
| --- | --- | --- |
| **U0: Preserve baseline evidence** | Establish the shared harness, fixture populations, allocation/copy instruments, provenance, and baseline measurements before changing production behavior. Capture canonical/miss frequencies. | None within this specification. |
| **U1: Elide the unnecessary canonical copy** | Implement only the local changed-order decision and A transfer, with byte, flag, single-pass, and absolute allocation checks. Keep the reorder path and all output APIs. | U0 baseline captured. |
| **U4: Verify, measure, and remove experiments** | Compare using U0's harness, report actual allocation/residency/CPU/latency results, satisfy R1–R4, preserve run artifacts, and remove abandoned experiments. Update live canonicalization evidence without rewriting historical runs or HP1 direct-frame records. | U0 and U1. |

U2/U3 from the larger draft are not implementation work. No dispatch or host
cutover is selected. Do not restore either as a prerequisite.

The following owner-approved amendment overrides any one-PR-per-unit or
equivalent landing rule:

- Milestones or units may be split into multiple independently landable tickets
  and PRs when needed for reviewability, without changing milestone boundaries
  or dependency ordering. This specification creates no implementation tickets.
- Every PR represents a cohesive, independently verifiable result and passes
  its applicable repository gates independently. Splitting is not permission to
  land a broken intermediate state or postpone its checks to a later PR.
- Target at most **500 changed production-source lines per PR**, with a **hard
  maximum of 1,000**, measured as additions plus deletions. Tests and
  documentation do not count.
- Before opening **each** PR, run independent architecture, complexity, testing,
  over-engineering, and language-design reviews **in parallel**. Consolidate and
  verify findings, fix every applicable finding, rerun affected checks, and
  leave no blocking finding unresolved. This is not limited to blocking fixes.
- Planning and specification publication authorize no implementation, commit,
  push, ticket mutation, or PR. Implementation owners complete these gates before
  requesting shipment.

## Testing Seams

1. **Seam:** Existing real host transform request/terminal boundary.
   **Behavior proved:** End-to-end output and failure preservation; user-visible
   latency only when measured from request send to its matching terminal receipt.
   **Prior art:** The daemon `direct_host` support path and existing transform
   fixtures. **New seam required:** No production seam. A reproducible benchmark
   driver, including `serve_native`, must be added and retained before making a
   host-latency claim. The fixed-body echo benchmark is only a transport control.
2. **Seam:** Existing `ServedMessage` construction, serialized-output cache,
   prepared segments, and `PreparedOutput` measurement/write/settlement tests.
   **Behavior proved:** Frozen bytes, hashes, block receipts, edited/untouched
   siblings, replay ownership, exact lengths, refusal diagnostics, cancellation,
   and partial-writer outcomes. **Prior art:** Frozen-shell and frozen-corpus
   tests, replay/invalidation tests, and prepared-output tests.
   **New seam required:** No. Reuse existing helpers and observe actual served
   segments rather than ordinary serialization of the wrapper.
3. **Seam:** Existing private canonicalizer module and its test-support entry.
   **Behavior proved:** Decoded-key permutation flags, all-object processing,
   unchanged-copy identity, one serialization, error-before-finalization, and A
   allocation transfer. **Prior art:** Counted `Serialize`, reversed-key cases,
   scalar vectors, and the isolated passthrough allocation fixture.
   **New seam required:** No production API. Minimal private/test-only observations
   may expose the actual branch or recorded spans where bytes alone cannot prove
   ownership. Do not duplicate production encoding setup or create a parallel
   fixture family, global production counter, or always-copy production variant.

Use exhaustive enumeration for the six permutations of three distinct keys.
Expected order comes from decoded keys, not the offset predicate being tested.
Keep literal escaped/prefix/Unicode witnesses and the frozen value-reference
corpus. The `to_vec(to_value(message))` reference assumes sorted `Value` maps and
unique keys; it is not the sole oracle if feature resolution changes. Equal-key
stability probes remain private and must not collapse entries through `Value`.

Use absolute per-call allocation/event-size observations for B, not only the
1-versus-65-block slope. Isolate recording from fixture setup, reference work,
other tests, harness threads, and Arc conversion; recording must not allocate
recursively. Preserve isolation under both explicit libtest commands and nextest.
Full-constructor peak observation uses the existing private constructor seam in
an isolated in-crate test process, with test-only allocation recording established
in U0. The public canonicalizer test entry alone cannot measure this lifetime.
Search and reuse existing allocator fixtures before adding recording logic; add
no public constructor API. If compatible observation cannot be established,
stop U0 for a seam decision rather than dropping the peak-residency gate.
Do not infer zero B from cache hits, pointer reuse, or a fabricated cumulative
allocation bound of twice output size. Negative controls must reject always-copy
behavior and short-circuit object processing for the intended reasons.

## Acceptance Criteria

- [ ] **AE1 / R1–R2:** Retained-original canonical misses, including sorted escaped
  keys, return A and match the frozen byte oracle. Absolute evidence shows one B
  allocation and N logical reorder-output bytes removed per N-byte miss, with no
  replacement full-size allocation.
- [ ] **AE2 / R2–R3:** Empty/single-field objects and sorted nested arrays take the
  identity branch. All six key permutations give the correct changed flag.
  Control characters, quotes, backslashes, empty keys, Unicode, and `a` versus
  `a b` are covered. Stable equal-key behavior remains unchanged.
- [ ] Ordered roots with unordered descendants and multiple unordered sibling
  objects still reorder every required object. Independent situation markers
  fire; the short-circuit negative control fails. Unchanged-span copying matches
  A's compact punctuation, ranges, scalar bytes, and length.
- [ ] **AE3 / R2–R4:** Typed, retained-original, partially edited, and cached/replayed
  outputs match frozen served bytes, hashes, output lengths, and existing block
  fingerprint rules. Untouched sibling fields retain existing behavior. Numeric
  cases include `-0.0`, `1.0`, large and small exponents, signed minimum, and
  unsigned maximum. A warm positive hit bypasses construction; completed page
  replay remains a distinct reuse path.
- [ ] **AE4 / R3–R4:** Counted sources retain one serialization. Errors before
  output and after an open-object prefix return without finalization or copying.
  Existing cap-refusal, cancellation, short/error-write, and length-mismatch
  tests retain outcomes and exact path-specific diagnostics. No local writer
  test is reported as exercising direct-ring T3/T4.
- [ ] R5 evidence includes both allocation scales and transform sizes, all source
  representations and cold/warm populations, complete-constructor peak residency,
  and separate copy classes. The same harness revision and the declared process
  schedule are used for baseline and candidate.
- [ ] Increased constructor peak residency is investigated before landing.
  CPU/latency results report their actual scope, including no detectable gain;
  no microbenchmark or echo result is substituted for host latency.
- [ ] All applicable repository gates pass for each PR. Its size and five parallel
  review lanes are recorded, all applicable findings are fixed and rechecked, and
  no blocking finding remains.
- [ ] No second production serde pass, new full-size storage, public output API,
  cache representation, transport change, migration scaffolding, or retained old
  production variant remains. Final handoff includes the response-path matrix
  and states that direct span emission and HP1 M4.3 are not implemented here.

### Measurement and verification contract

Extend existing allocation and hot-path harnesses, corpus, and test-support
entry points after searching for reusable helpers. Use 1 and 65 blocks for
allocation scaling, and 100 and 1,000 messages for transform behavior. Include
retained-original, fully typed, one-edited-block shells; escaped, Unicode, and
prefix keys; many tiny objects versus few large payloads; cold construction and
warmed output-cache hits. Report actual canonical/miss frequencies. These are
declared fixture sizes, not evidence of production representativeness.

Record allocation/reallocation events, cumulative requested bytes, peak live
bytes, live capacity at the return boundary, and A's capacity/length ratio
separately. Attribute canonical reorder bytes separately from Arc conversion,
response assembly, and arena bytes. Include full `from_message_reusing`
construction and cold/warm transform measurements, not only `to_vec`.

Run release baseline/candidate binaries with identical fixtures prepared outside
timed setup, alternating AB/BA across **10 independent process pairs**. Retain
commit, toolchain, resolved feature set, CPU, allocator, sample counts, per-pair
CPU and elapsed time, and p50/p95 latency. Preserve the harness revision, driver,
schedule, and run artifacts. State timing boundaries and results without adding
an unapproved speedup threshold. The existing hot-path benchmark omits host
publication; a real-host driver is required for a user-visible latency claim.

Use `.github/workflows/ci.yml` at implementation time as the authoritative gate
set. Preserve the source plan's commands below; they are required implementation
checks, not a report that this specification task ran them. Cargo uses `--locked`
except formatting, which has no such flag.

```sh
cargo +1.98 fmt --all -- --check
cargo +1.98 clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo +1.98 test -p daemon --all-features --locked --test served_json_passthrough_allocations
cargo +1.98 test -p daemon --all-features --locked served_canonical_frozen_corpus_matches_value_reference_for_both_shells
cargo +1.98 test -p daemon --all-features --locked --test prepared_output
cargo +1.98 nextest run --profile ci --workspace --all-targets --all-features --locked
cargo +1.98 test --workspace --doc --all-features --locked
cargo +1.98 test --workspace --all-features --locked --bench '*'
RUSTDOCFLAGS='-D warnings' cargo +1.98 doc --workspace --no-deps --all-features --locked
cargo +1.98 check --workspace --no-default-features --locked
cargo +1.98 check -p storage --no-default-features --locked
bash scripts/forbid-comment-markers.sh
bun run check:repo
```

Also run checks selected by touched paths, including fuzz-workspace formatting
and compilation when required. Benchmark targets retain CI's no-extra-argument
test-mode behavior. At the inspected revision, additional applicable gates
include repository identifier guards, incident history/verifier checks,
workspace-graph validation, the distinct-stable-toolchain lane, and daemon-triggered
native-addon, payload/tarball, and Rust-mode E2E checks. Miri/Valgrind follow scoped
unsafe/transport rules if a separately reconsidered scope requires them; do not
expand this implementation to justify running them. Never add comment-lint
override markers or weaken guards.

## Domain Skill Inputs

- Routing mode: `/research-planning:ask-skills` invoked. Three compatible installed
  owners were selected for bounded, read-only enrichment, not implementation.
- `/testing:property-discovery-and-catalog` — VERIFIED and invoked for canonical
  span decisions, served identity/cache ownership, and prepared-output/failure
  boundaries. Independent lens discovery, a final wildcard pass, existing-check
  inventory, fault mapping, and a reusable nine-record catalog inform the
  invariants and acceptance criteria. Broad B1 and transport records are reused.
- `/testing:test-strategy` — VERIFIED and invoked. Selected existing public and
  private seams, exhaustive permutation cases, independent literal oracles,
  isolated absolute allocation observations, and negative controls. Separated
  local writer checks from ring evidence and host latency from in-process timing.
- `/design-review:rust-design-review` — VERIFIED and invoked. Found no structural
  blocker in the local ownership transfer; preserved one canonicalizer and the
  owned-output path. Added explicit observation of A's slack through receipts,
  hashing, and Arc conversion, and separation of cache-hit probes from test-only
  reference construction. This is not implementation approval.

## Out of Scope

- Direct span emission, direct canonical serialization without spans, transform
  or Exact direct-frame publication, a bounded direct writer, new source-lifetime
  admission, or HP1 M4.3 delivery/closure. Reconcile actual landing state before
  implementation; do not create a competing transport plan or adapter.
- Removing contiguous cached canonical bytes, changing cache accounting or
  ownership, eliminating the later Arc/assembly/arena copies, or reordering wire
  type declarations to manufacture canonical inputs.
- Arbitrary raw-fragment canonicalization, recanonicalizing persisted receipts,
  payload-sized pre-encoded envelopes, second length serialization, new writer
  traits, borrowed-view types, index-permutation storage, or settlement callbacks.
- Dreamer whole-body materialization cleanup, unrelated serializer cleanup,
  broad cache or transport redesign, feature flags, compatibility layers, and
  transitional scaffolding. Broader draft alternatives were rejected, not made
  prerequisites to this fast path.
- Implementation tickets, source/test/CI implementation, commits, pushes, PRs,
  or modifications to existing tracker items during this specification task.

## Open Questions

- **Implementation owner, before U0 measurements:** Which authorized base and
  resolved feature set will be measured? Revalidate the pinned-plan assumptions,
  current call graph, and HP1 landing state rather than relying on old line
  anchors or unrelated working-tree edits.
- **Measurement owner, during U0 and before U1:** What proportions of the declared
  workloads and any claimed production population are canonical misses, unordered
  misses, and positive hits? Record them; do not assume retained-original traffic
  or benchmark sizes establish production frequency.
- **Measurement owner, before landing U4:** Does A's longer-lived spare capacity
  increase complete-constructor peak residency? Investigate any increase without
  inventing an unapproved shrink or accounting redesign.
- **Measurement owner, before any speedup claim:** What do the 10 paired runs
  establish for CPU and latency? If host latency is claimed, retain the real-host
  driver and its matching-terminal timing evidence. Resolve adverse or inconclusive
  results before making that claim; a local allocation saving is not its answer.

These are evidence obligations, not invitations to reopen the selected design.
The companion catalog records missing observations and test handoffs without
claiming that historical tests exercise the candidate.

## Further Notes

### Response-path matrix

Canonical reorder copying is distinct from ordinary assembly and transport copies.

| Actual path | Canonicalization | Retained output/publication |
| --- | --- | --- |
| CK transform, unchanged retained-original miss | Return A without B, including sorted escaped keys. | Retained canonical Arc, owned response buffer, and arena copy remain. |
| CK transform, typed/synthetic/reduced/edited miss | Reorder A into B if any object changes order. | Retained canonical bytes and ordinary owned publication remain. |
| Serialized-output cache hit | No serialization, key sort, or canonical copy. | Reuse canonical message; assembly and arena copy remain. |
| Native full/delta transform | CK side follows the preceding rows; native Values and envelope serialize through existing sorted-map behavior. | Native payload is not cached raw JSON; envelope count/write and owned output remain. |
| Transform with `messages: None`, including resync/refusal | Generic `PreparedSource::Json`, no served-message canonical copy. | Count, reserve, then write owned output. |
| Facade Applied or unmodified Duplicate | `PreparedSource::Exact`, no canonicalization. | Preserve persisted receipt bytes; existing owned copy, no direct callback. |
| Duplicate with `replayed` inserted or other generic JSON | `PreparedSource::Json`. | Existing count/reserve/write; no assumption that output is small. |
| Completed transform/state-sync page replay | Reuse cached `PreparedOutput` according to its source variant. | No forced recanonicalization or changed lifetime/publication. |
| Dreamer successful task outcome | `respond(Value)`, not `served_json`. | Existing byte materialization and reparse remain outside scope. |
| Host direct-fill fixture | No canonicalizer. | Generated chunks target the arena; no sender invoking that arm was found at the inspected revision. |

No response path newly reorders directly into the final sink. Generic and Exact
paths retain owned output for existing admission and lifetime semantics, not as
a migration fallback. Dreamer and test materializers also rule out claiming that
the system has no whole-response-buffer consumers.

### Provenance and reusable artifacts

Settled source: `docs/plans/2026-09-13-0030-perf-canonical-output-direct-frame-plan.md`,
with baseline `4980f8af3bb90d58b19b80a38a227fb6363a8b33`. This specification incorporates
the owner's review-size and parallel-review amendment without changing its
U0 → U1 → U4 boundaries.

Discovery verifies source against `2e4433e6b511ae74944df8a9669c428e73915d29` on
2026-09-13, excluding unrelated working-tree modifications. The canonicalizer and
prepared-output implementation still have the selected baseline shape; other
surfaces have moved, so old source anchors are not live evidence.

Reusable companion: `docs/properties/canonical-output-fast-path/`, containing
the catalog, per-property evidence, existing-check inventory, fault map,
independent portfolio evaluation, and lens provenance. It supplements the
existing `docs/properties/hot-path-optimization/latency-audit/catalog.md`, which
owns B1, T1–T4, and the invalidated W1/W2 records, rather than replacing historical
evidence. Before updating property documentation, read `docs/properties/AGENTS.md`
and `docs/properties/METHOD.md`. Preserve historical evidence and change only
live canonicalization claims within this scope. These
are local documentation artifacts pending repository publication; this issue
does not imply that uncommitted files are available through GitHub blob links.

[HP1](https://github.com/ahrav/eidnara/issues/350) and
[M4.3](https://github.com/ahrav/eidnara/issues/441) were read on 2026-09-13; both
were open with no comments. Their direct-frame scope remains separate. No
additional external incident reports or related repositories were supplied;
none is assumed. Specification publication leaves both issues unchanged.

Work item: canonical-output-fast-path-21a1865f-ac83-4ae6-941a-b6d726691824
