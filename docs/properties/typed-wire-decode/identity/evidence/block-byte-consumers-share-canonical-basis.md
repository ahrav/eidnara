# block-byte-consumers-share-canonical-basis

## Discovery trigger

Plan R7/KTD2/U2 assigns canonical block bytes to one `served_json` entry point.
Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
External leads: the settled plan, latency-audit B1, and issue 426.
Lenses: architecture, data integrity, security boundaries, wildcard.

## Evidence trail

1. `crates/daemon/src/served_json.rs:111-142` exposes message serialization
   over private `encode`; the proposed block-specific function is absent.
2. `crates/daemon/src/wire.rs:725-786` serializes blocks separately and stores
   bytes plus SHA-256. It can replay originals or use typed declaration order.
3. `crates/daemon/src/transform.rs:164-200` computes fresh block receipts
   with a separate `to_string`; receipt reuse takes a different path.
4. `crates/daemon/src/codec/sidecar.rs:148-174` strips `_eidnara_codec`,
   clears retained originals, builds a `Value`, and calls `stable_hash`.
5. Lines 407-410 define `stable_hash(&Value)` as hashing serialized JSON.
   `crates/daemon/src/wire.rs:865-868` already hashes raw string bytes.
6. `sidecar.rs:209-241` uses decoded fingerprints to decide unchanged content
   and alignment candidates. Other provider metadata must remain significant.
7. `packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts:745`
   requests native output in the ordinary path, supporting default-production
   reachability of the sidecar consumer.
8. `crates/memory-store/src/lib.rs:340-352` serializes false tool flags on
   typed blocks at HEAD. The old sidecar clears originals before serialization,
   so it inserts false even for normal plugin blocks whose ingress omitted it.
   Proposed skipping changes that normalized basis by 26 bytes per such flag,
   while the corresponding plugin ingress canonical bytes remain unchanged.

## Failure scenario

All block constructors decode correctly, but one consumer uses declaration
order or wraps canonical text in a JSON string. Its hash disagrees with the
projection. Another faulty variant strips every provider namespace and
mistakes a changed provider payload for unchanged native content.

The exact check hashes raw C bytes. The sidecar hashes C after stripping only
its namespace. Equal hashes across all three are required only when stripping
changes nothing. A common producer must still agree with independent R bytes.

## Timing windows and dependencies

No timing injection is needed. The important condition is fresh fallback,
which receipt reuse can otherwise bypass entirely.
The inspected handler keeps a sidecar in its process-local native cache.
That is not proof every stamp is ephemeral. `DecodeSidecar` is serializable
(`crates/daemon/src/codec/sidecar.rs:46-55`), and codec stamps are ordinary
provider extras (`sidecar.rs:177-193`) representable in persisted raw/frozen
wire rows. No supplied artifact proves old stamped rows cannot exist.

## What a test must construct

- A block without codec extras and the same block with two different stamps.
- A changed noncodec namespace and changed retained opaque/input JSON.
- Plugin and typed-only blocks, escaped keys, and non-ASCII content.
- A served message with no reusable projected receipt.
- Independent expected bytes, full lowercase hashes, and UTF-8 lengths.
- Raw canonical text versus `Value::String(text)` as a negative control.
- Record `typed_wire_identity_fresh_and_sidecar` from inputs and selected path.
- Include ordinary plugin absent/true flags and old typed false flags in the
  sidecar comparison; classify an old stamped row separately from a new cache.

## Investigation log

### Q: Can the existing `stable_hash` consume canonical text directly?

- Sources examined: `sidecar.rs:407-410` and `wire.rs:865-868`.
- Findings: it accepts `Value`, not bytes. Converting text to `Value::String`
  adds quotes and escapes. Parsing C into `Value` reintroduces the unwanted tree.
- Missing evidence: the proposed helper's exact signature and error policy.
- Conclusion: resolved on the double-encoding risk; use the existing raw-byte
  hashing seam or its factored owner. No second canonical serializer is needed.

### Q: Should sidecar hash always equal a reused served receipt?

- Sources examined: namespace stripping and signed-zero receipt assertions at
  `crates/daemon/src/transform.rs:13889-13892`.
- Findings: stripping and equality-based reuse are distinct transformations.
- Missing evidence: none for that distinction; the replacement check is absent.
- Conclusion: resolved, compare fresh receipts only and qualify namespace
  equality. `/testing:test-strategy` owns the cross-consumer oracle; existing
  matching tests go to `/testing:invariant-test-review`, still unaudited.

### Q: Is normalized sidecar fingerprint drift confined to daemon-built blocks?

- Sources examined: `sidecar.rs:168-174`, tool serde fields, and plugin
  emission at `module-wire.ts:1012-1014`.
- Findings: no. Clearing originals first makes absent-flag plugin tools use
  explicit false in the old normalized basis. The proposed skip changes it.
- Missing evidence: an owner resolution of the plan's sidecar-preservation
  wording, and compatibility behavior for any old persisted stamped row.
- Conclusion: needs human input before implementation. A process-local cache
  rationale cannot authorize changed native output or assume durable stamps
  away. This is a supplied fresh-review refinement verified against HEAD.

## Typed-wire U1 execution, 2026-09-13

Branch `perf/typed-wire-u1-owned-decode`; replay envelopes removed. `tests/block_bases_agree.rs` and `fresh_block_byte_consumers_call_the_canonical_producer` pass; `decoded_block_fingerprint` hashes the canonical bytes of the typed block with `_eidnara_codec` removed and no envelope step.
