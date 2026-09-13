# plugin-block-canonical-identity-preserved

## Discovery trigger

Plan R4 promises identical bytes and hashes for every plugin-emitted block.
This is an accepted prospective contract, not a measured implementation fact.
Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
External leads: the settled plan, latency-audit B1, and issues 350/426.
Discovery lenses: architecture, claimed safety, data integrity, versioning.

## Evidence trail

1. `packages/opencode-plugin/src/hooks/context/module-wire.ts:940-1093`
   emits text, reasoning, redacted reasoning, tools, media, and opaque blocks.
2. Lines 1012-1014 emit the tool flag only when true. Absence is normal.
   Lines 325-367 emit text/error_text or content/error_content output kinds.
3. Lines 184-213,249-266 preserve optional filenames, approval arcs, and
   provider metadata. These require distinct fixture partitions.
4. `crates/memory-store/src/lib.rs:250-279` retains and replays the parsed
   ingress block. Unknown typed-envelope fields can affect baseline bytes.
5. `crates/daemon/src/wire.rs:731-736` hashes `to_string(block)` bytes.
   Lines 599-617 derive the nonsynthetic per-message identity vector.
6. `crates/daemon/src/served_json.rs:121-163` sorts object spans recursively
   while preserving serde scalar forms and sorting escaped keys by decoded text.
7. `crates/daemon/src/transform.rs:14467-14490` compares flat projection
   records with the checked-in projection golden. It does not synthesize an
   exhaustive plugin corpus or separately prove its membership.
8. Static HEAD fixture parsing finds seven messages and sixteen blocks. All
   ten tool blocks explicitly encode false. There are no media blocks.
   This corpus contains nonplugin output variants and misses emitted ones.

## Failure scenario

Deriving serialization adds false flags or uses declaration order. Hashes
change for unchanged plugin input, causing an established covered or frozen
message to fail identity enforcement. Agreement between new consumers does
not catch a common wrong basis.

The independent expected side must be frozen baseline plugin bytes. The typed
value-round-trip reference is a second oracle, not a substitute for that pin.
An old `WireBlock` retaining its original is not a fully typed reference.

## Timing windows and dependencies

The vulnerable boundary is an implementation upgrade, not a thread race.
Capture baseline bytes before changing serde attributes or regeneration code.
The core source at the baseline equals HEAD for the inspected identity files.
Reachability is default-production: the normal emitter supplies these blocks,
and `wire.rs:407-408` projects them without a test-only gate.

## What a test must construct

- Freeze each emitted block kind and each of the four emitted output kinds.
- Cover absent and true flags, empty/present signatures, media source forms,
  missing/present filenames, opaque arcs, and provider extras.
- Retain nested JSON values, escaped keys, non-ASCII strings, and scalar forms.
- Assert exact bytes, SHA-256, and ordered identity vectors with fixed mids.
- Keep explicit-false and unknown-envelope inputs in separate normalization
  fixtures. Their accepted identity change is not a failure of the P contract.
- Record `typed_wire_identity_plugin_shapes` only after all declared input
  partitions exist. Existing catalogs do not supply this marker.

## Investigation log

### Q: Does the existing golden establish the full plugin domain?

- Sources examined: the emitter, `wire-golden.json`, projection golden, and
  `wire_golden_projects_to_flat_blocks` at HEAD.
- Findings: all ten tool flags are false; media and content output are absent.
  `json` and `execution_denied` output are typed-only relative to this emitter.
- Missing evidence: checked-in emitter-derived absent/true and missing-shape
  fixtures, and the raw 53-block Appendix corpus.
- Conclusion: unresolved, needs a classified frozen corpus before replacement.

### Q: Is preservation already proved by `served_json` tests?

- Sources examined: `crates/daemon/src/served_json.rs:171-253`.
- Findings: scalar/order checks exist, but they do not establish new field skips.
- Missing evidence: the replacement decoder and end-to-end projection comparison.
- Conclusion: unresolved, needs `/testing:test-strategy`; existing checks go
  to `/testing:invariant-test-review`. No test ran during discovery.
