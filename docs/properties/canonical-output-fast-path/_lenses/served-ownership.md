# Served ownership and canonical-output fast path

## Scope and provenance

This is read-only discovery and Rust design enrichment for the supplied
[canonical-output plan](../../../plans/2026-09-13-0030-perf-canonical-output-direct-frame-plan.md).
The user supplies the settled evidence scope; no interview or tracker lookup
is needed. That plan exists only in the working tree, not in HEAD. Its older
source anchors are leads, not verified references for this report.

All source and catalog anchors below are checked through `git show HEAD:path`
at `2e4433e6b511ae74944df8a9669c428e73915d29` on 2026-09-13. Unrelated dirty
source is excluded. The property-discovery-and-catalog and rust-design-review
skills supply the analysis lenses; `docs/properties/METHOD.md` governs records.
Model lenses are state/cache/persistence, ownership/concurrency, and product
context. Property lenses are lifecycle, replay/idempotency, and resource bounds.

The selected change transfers serialization buffer A when no field moves and
retains reorder buffer B otherwise. It changes no cache representation, wire
contract, response writer, admission policy, or transport lifecycle. Catalog
guarantees remain claims under test. Every check listed here is **unaudited**;
no Rust tests, benchmarks, or adequacy audit run in this discovery pass.

## Verified system model

### Representation and identity

- `WireMessage` and `WireBlock` retain parsed `Value` objects, not original
  input text. Their serializers replay those values when present, otherwise
  they build typed serialization data with cloned fields
  ([message serialization][message-ser], [block serialization][block-ser]).
  Public metadata edits alone do not clear originals. `content_mut` clears
  only the message original; `kind_mut` clears the edited block original.
  Untouched sibling originals remain available
  ([message edits][message-edit], [block edits][block-edit]). Unknown fields
  therefore follow the existing shell/block retention boundary, not a new
  promise to preserve unknown fields after every edit.
- [Construction][constructor] first canonicalizes the message, computes block
  receipts, then hashes canonical bytes and converts owned data into Arcs.
  The whole-message SHA-256 is not a block fingerprint. Fresh block receipts
  hash `to_string(block)`; projected reuse needs structural equality. An
  unequal positional candidate prevents fallback, while an absent position
  permits the first digest-index candidate followed by the same equality
  guard ([receipt helpers][receipts]). Equal floating zeros can reuse receipts
  with different serialized spellings, as the [receipt witness][receipt-test]
  explicitly distinguishes. `output_identity` starts as canonical hex but can
  be replaced by the renderer identity without changing `canonical_hash`
  ([identity replacement][identity-replace], [tail construction][tail-build]).
- [Encoding][encoder] serializes once into growable A, sorts every object's
  spans, then unconditionally allocates B and reconstructs the output. It
  already skips sorting ordered unescaped keys; escaped-key sorting decodes
  keys. Returning A is the plan's obligation, not implemented behavior at HEAD.

### Cache state and ownership

- The handler supplies its serialized-output cache to the production transform
  ([handler call][handler-cache], [cache entry point][cache-entry]); compaction
  is enabled by default ([configuration][default-config]). The pass clones a
  snapshot under the mutex and builds outside that lock ([snapshot use][snapshot-use]).
  Snapshots clone entry maps and `ServedMessage` handles. A revert-epoch change
  removes the session; replacement enforces the existing retained-byte budget
  ([cache lifecycle][cache-lifecycle]). This is in-memory memoization.
- A clean matching positive entry returns cloned Arcs without constructing a
  message ([cache lookup][cache-lookup]). The live-tail branch also caches
  omitted output as `Some(None)`; the synthetic-message helper flattens that
  into a miss. Cache statistics therefore are not universal constructor-call
  counts ([tail hit][tail-hit], [tail construction][tail-build]). Snapshot
  metadata and response assembly still cost work; bypass does not mean that
  the whole warmed request is allocation-free.
- [ServedMessage][served-owner] owns the typed message, canonical slice,
  identity, and fingerprint array through separate Arcs. Its fields expose no
  mutable shared payload. `into_message` unwraps unique ownership or clones
  the typed value ([owned extraction][owned-extraction]). Evicting a cache
  entry does not invalidate other handles. The [retained estimate][retention]
  counts those final allocations, not constructor scratch or all outstanding
  snapshot/response owners.

### Prepared output and persistence

- [respond_transform][respond] removes messages before converting the envelope
  to `Value`, then moves each served owner into a prepared segment. That avoids
  [ServedMessage's ordinary serializer][served-ser], which serializes the inner
  `WireMessage` rather than emitting its cached canonical slice. [Segments][segments]
  retain the entire served owner and obtain both bytes and length from that
  slice. [Measurement and writing][segment-write] add stored message lengths
  and later write those bytes; they do not canonicalize messages again.
- [Settlement][settlement] retains the prepared source across output admission,
  checks cancellation, and writes into the existing owned destination. Matching
  completed transform-page retries clone `PreparedOutput` after checking attempt
  and digest fields ([page replay][page-replay]). This is separate from an
  ordinary transform's serialized-output cache hit and is not a restart-durable
  replay claim.
- Block receipts populate `served_output_fingerprint` and divergence detection
  ([metadata update][metadata-update]); metadata is passed to the existing
  conditional commit before output-cache replacement ([commit order][commit-order]).
  Neither the canonical byte buffer nor its whole-message hash replaces those
  persisted receipt semantics.

## Existing checks and missing witnesses

The [B1 inventory][b1-checks] already lists the broad identity checks. These
source observations do not import its historical execution results.

| Check | Observed assertions and limit | Status |
| --- | --- | --- |
| [Frozen shells and actual served segments][shell-test] | Literal bytes, canonical hash, latent edits, unknown fields, measured length, and written frame bytes agree. | unaudited |
| [Frozen corpus][corpus-test] and [receipt witness][receipt-test] | Original/typed output matches the value-round-trip reference; fingerprint selection uses a separate structural reference. | unaudited |
| [Single serialization][once-test] and [key/scalar cases][key-test] | Serialize visit counts and byte ordering are checked, not ownership transfer or absence of B. | unaudited |
| [Constructor source guard][source-test] | Lexical checks reject a `to_value` round trip and require the shared fingerprint receipt helper. They do not measure runtime allocation or call counts. | unaudited |
| [Steady replay][cache-test] | Four reused items, zero serialized items, unchanged token-estimator calls, and equal output bytes are asserted. No independent constructor or encoder observer exists here. | unaudited |
| [Overlay][overlay-test], [drop/fold][drop-fold-test], and [epoch][epoch-test] | Changed items are rebuilt or entries invalidated while unaffected replay matches fresh output. | unaudited |
| [Allocation fixture][allocation-test] | Allocation/reallocation events for 1 and 65 unescaped-key blocks are compared, and every declared population's returned buffer is classified by pointer identity and size with the serialization buffer's release checked. Copies are inferred from provenance, not observed; escaped-key identity return is expected only after the canonicalizer changes. | unaudited |
| [Retained-cache estimate][retained-test] | Metadata and omitted rows are charged; the manual estimate permits 5% tolerance. It is not a transient constructor-memory bound. | unaudited |
| [Prepared-output failures][writer-test] | Cap/overflow, positive short writes followed by failure, and length mismatch are checked. These are not direct-ring publication witnesses. | unaudited |

No independent hit-path constructor spy, returned-A ownership witness, or
constructor peak/spare-capacity measurement is found in these scoped checks.

## Narrow candidate for synthesis

This is a candidate, not a second B1 record or a completed catalog addition.
Its local evidence is above; synthesis can attach an evidence file if adopted.

### served-output-cache-hit-skips-construction

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - The independent constructor/encoder observation is absent;
the existing replay test is inspected but not executed here.
Guarantee: Selecting a clean matching cache entry containing a served message
reuses its owned artifacts without constructing or canonicalizing that message.
Check: `always-or-unreached` - For each selected positive hit, constructor and
encoder call deltas for that item are zero, and returned message, canonical
bytes, identity, and fingerprint Arcs are pointer-equal to the cached owners;
the optional hit path must obey this when reached.
Fault/timing angle: None; the relevant transition is miss, insertion, then hit.
Required faults and enabling state: No injected fault is needed. Use the same
session, revert epoch, key, and identity, a clean item, and a retained positive
entry. Exercise synthetic and live-tail callers. The production call and
default configuration are linked in the cache-state model above.
Confidence: high - [Evidence](#cache-state-and-ownership). Both hit branches and
the Arc-backed representation are verified in source, not by a runtime probe.
Existing check: [Steady replay][cache-test] reports reuse and byte equality;
its counters do not independently observe construction. Status: unaudited.
Impact: Rebuilding equivalent bytes can pass B1 while restoring serialization,
hashing, allocation, and copying on warmed requests.
Open questions:
- No design decision is missing. Test-only observation of construction remains
  to be implemented within the existing test seams.

An accompanying `sometimes` marker,
`canonical-output-fast-path-served-cache-hit`, should record the matching
positive-entry preconditions before lookup. It must fire on correct code; it
must not assert that forbidden construction occurred. Miss and invalidation
controls must reach construction, outside the hit observation interval.

## Rust design enrichment

**Verdict: No structural blockers found.** The settled local change keeps the
verified ownership boundaries. This is not implementation approval or a claim
of lower measured residency. The following ten items separate recommendations
from the source facts above.

### Missing Constraints

1. **Recommendation:** Observe A's length and capacity through block-receipt
   construction, hashing, and `Arc<[u8]>` conversion, not only encoder return.
   Those operations follow encoding in the verified constructor. Distinguish
   transient Vec slack from the final Arc slice's retained accounting.

### Risks

2. **Verified fact and recommendation:** The [test-only differential][test-diff]
   explicitly builds fresh output after cached output. A whole-pass encoder
   counter would count that reference work. Scope observation to the selected
   build or use an existing non-`cfg(test)` library test lane; do not disable
   the differential or infer bypass from `serialized_items` alone.

### Durable Decisions

3. **Accepted decision:** Keep one serializer and canonicalizer, the private
   `sort_fields` owner, and the existing `to_vec` signature. Evaluate every
   object without short-circuiting. Preserve escaped-key sorting and detect its
   identity permutation from increasing source offsets. Do not shortcut on
   `original()`, reorder wire fields, or admit raw fragments into this facade.
4. **Accepted decision:** Keep contiguous retained canonical bytes, Arc
   ownership, fingerprint rules, cache budgets, and owned-output settlement.
   Add no cache-accounting design, `shrink_to_fit`, second length pass, writer
   API, direct-output caller, feature flag, adapter, or migration path. Existing
   error, reservation, cancellation, and publication behavior remains binding.

### Testing Guidance

5. **Recommendation:** Extend the existing replay fixture with the candidate's
   independent observer and pointer checks. For B1, add mixed edited/untouched
   siblings and retain literal/structural oracles, including signed-zero receipt
   differences. Hold a prepared served segment after cache eviction and write
   it repeatedly to witness surviving ownership, without imposing immediate
   allocation release when cache accounting drops.
6. **Recommendation:** Extend the existing allocation fixture for canonical
   misses, including escaped keys and a large scalar with small span tables.
   Witness one B allocation and N logical copy bytes removed for N encoded
   bytes, with no replacement full-size allocation. Record allocation events,
   requested bytes, peak live bytes, and capacity/length separately. Reallocation
   events do not count physical copies. This is a resource witness for the
   accepted plan, not a new retained-cache bound.

### Reuse

7. **Recommendation:** Reuse
   [derived-artifacts-are-ownership-independent][b1] for canonical bytes,
   hashes, fingerprints, unknown-field/edit semantics, and served-segment
   identity. Its [canonical evidence][b1-evidence] already separates transient
   scratch from retained ownership. Refine witnesses there rather than copying
   its guarantee into another record.
8. **Boundary:** Leave
   [direct-frame-publishes-declared-length-or-nothing-and-holds-its-charges][t3]
   and [direct-frame-outlives-its-handler-before-publication][t4] with their
   existing owners. This plan does not exercise or deliver them. The retired
   [optimized-stage-is-measured-at-production-shape][w1] remains invalidated;
   the supplied plan's measurement obligation does not reactivate it.

### Open Questions

9. **Unresolved measurement:** What fraction of real misses is canonical?
   Cold construction, typed/edited misses, and warm hits need separate counts.
   Existing [cold/warm benchmark cells][bench] are reusable controls, not proof
   of the population mix or host latency improvement.
10. **Unresolved measurement:** Does longer-lived A slack increase complete
    constructor peak memory despite removing B? No scoped check measures it.
    Retain the plan's investigation gate for an increased peak; do not invent
    a cache charge, numerical bound, or unmeasured speedup to close it.

## Portfolio disposition and handoff

The local comparison identifies one distinct bypass candidate, B1 witness
refinements, and a measurement bias toward warm or typed fixtures. No new
liveness, persistence, or transport property is justified by this local change.
Fresh-context portfolio evaluation is not completed: subagent dispatch is
blocked by the session depth limit. This file remains lens working material.

Route the candidate and witnesses to `test-strategy`, existing-check adequacy
to `invariant-test-review`, and post-implementation correctness to
`rust-code-reviewer`. Those handoffs are recommendations, not executed reviews.

[message-ser]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/memory-store/src/lib.rs#L126-L162
[block-ser]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/memory-store/src/lib.rs#L250-L280
[message-edit]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/memory-store/src/lib.rs#L203-L222
[block-edit]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/memory-store/src/lib.rs#L306-L323
[constructor]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L164-L224
[receipts]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/wire.rs#L865-L902
[identity-replace]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L226-L237
[tail-build]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L11286-L11315
[encoder]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L121-L164
[handler-cache]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/lib.rs#L8492-L8498
[cache-entry]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L1803-L1817
[default-config]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/config.rs#L116-L125
[snapshot-use]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L4789-L4810
[cache-lifecycle]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L406-L463
[cache-lookup]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L10392-L10431
[tail-hit]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L11126-L11135
[served-owner]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L144-L153
[owned-extraction]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L239-L245
[retention]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L252-L279
[respond]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/lib.rs#L14835-L14873
[served-ser]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L301-L307
[segments]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/dispatch.rs#L17-L72
[segment-write]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/dispatch.rs#L325-L370
[settlement]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/lib.rs#L12360-L12429
[page-replay]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/lib.rs#L9680-L9701
[metadata-update]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L4921-L4930
[commit-order]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L4979-L5029
[shell-test]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L13714-L13756
[corpus-test]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L13760-L13794
[receipt-test]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L13797-L13939
[once-test]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L171-L193
[key-test]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L196-L252
[source-test]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L13942-L13969
[cache-test]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L28295-L28320
[overlay-test]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L28323-L28358
[drop-fold-test]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L28361-L28424
[epoch-test]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L28585-L28602
[allocation-test]: https://github.com/ahrav/eidnara/blob/073f578efcb1bb08d54eac3b54c0f837d2f0c857/crates/daemon/tests/served_json_passthrough_allocations.rs#L37-L163
[retained-test]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L28427-L28581
[writer-test]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/tests/prepared_output.rs#L118-L234
[test-diff]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L4862-L4890
[bench]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/benches/hot_path.rs#L320-L397
[b1]: ../../hot-path-optimization/latency-audit/catalog.md#L356-L478
[b1-evidence]: ../../hot-path-optimization/latency-audit/evidence/derived-artifacts-are-ownership-independent.md#L367-L442
[b1-checks]: ../../hot-path-optimization/latency-audit/existing-checks.md#L88-L95
[t3]: ../../hot-path-optimization/latency-audit/catalog.md#L1513-L1537
[t4]: ../../hot-path-optimization/latency-audit/catalog.md#L1581-L1594
[w1]: ../../hot-path-optimization/latency-audit/catalog.md#L1768-L1776
