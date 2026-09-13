# sibling-mutation-preserves-untouched-bytes

## Discovery trigger

The plan removes accessor invalidation and promises deterministic typed
serialization keeps unedited siblings byte-identical.
Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
External leads: plan mutation paths and latency-audit B1/B2.
Lenses: concurrency, data integrity, replay, protocol contracts.

## Evidence trail

1. `crates/memory-store/src/lib.rs:203-208` drops a message original on
   mutable content access. Each block retains its own original until edited.
2. Lines 306-317 clear a block original through mutation or `mark_modified`.
   Direct public metadata edits need explicit invalidation at HEAD.
3. `crates/daemon/src/wire.rs:540-559,781-784` shares canonical shells and
   block indexes rather than copying separate blocks into the projection.
4. `crates/memory-store/src/lib.rs:16096-16112` asserts an edited first block
   and an unknown-field sentinel on the untouched second block.
5. `crates/daemon/src/transform.rs:12733-12773` similarly checks a tagged
   block beside an untouched opaque sibling and message provider provenance.
6. `crates/daemon/src/wire.rs:1708-1784` checks original retention and repeated
   prefix reattachment. Its current invariant is representation-specific.
7. `crates/daemon/src/codec/opencode.rs:1492-1560` checks sibling deletion
   and native metadata ownership. These are related, unaudited checks.

## Failure scenario

An edit clears or rebuilds the whole message and changes an unrelated sibling's
serialization, or mutates an alias still retained by another request. Exact
whole-message equality cannot distinguish a correct edit from lost siblings.

The oracle snapshots each normalized typed block separately. It expects the
chosen edit, compares every unchosen block byte-for-byte, and checks the
aliased source remains untouched. For P siblings it also checks old baseline
bytes. Unknown envelopes intentionally discarded at decode are not retained
sentinels under the prospective contract.

## Timing windows and dependencies

The vulnerable sequence is decode, share, clone, mutate, and reserialize.
Thread interleavings are not required to exercise alias noninterference.
Reachability is default-production through overlays, reductions, and shared
projection reattachment; only the proposed observation marker is test-only.

## What a test must construct

- Two or more blocks with an actual edit to exactly one chosen kind.
- Untouched siblings with optional signature, provider extras, or opaque raw
  JSON whose preservation cannot be inferred from a simple text comparison.
- Multiple aliases to the original shell and an independently owned edit.
- Full and reattached/incremental projection of the same effective input.
- A separate unknown-envelope case showing removal occurs at normalization,
  not being mistaken for collateral mutation drift.
- Record `typed_wire_identity_shared_sibling_edit` from alias and edit inputs.

## Investigation log

### Q: Can the old unknown-field sibling tests be carried forward unchanged?

- Sources examined: the memory-store and transform sibling tests and plan R3.
- Findings: those tests promise preservation of fields the plan removes.
  The remaining guarantee is typed sibling isolation, including opaque and
  provider JSON explicitly retained by the model.
- Missing evidence: a replacement sentinel in a retained field and its exact
  byte oracle after the new decoder is implemented.
- Conclusion: resolved as contract conflict. `/testing:invariant-test-review`
  must re-audit the assertions; no automatic exercised status transfers.

### Q: Does deterministic serialization alone prove sharing is safe?

- Sources examined: shared shell construction and accessor mutation.
- Findings: determinism says equal inputs serialize equally; it does not prove
  the input behind an alias was unchanged.
- Missing evidence: explicit source/sibling snapshots across a real edit.
- Conclusion: unresolved, needs `/testing:test-strategy` to observe both
  ownership noninterference and expected edited bytes. No test ran here.
