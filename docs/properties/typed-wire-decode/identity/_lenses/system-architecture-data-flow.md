# System lens: architecture and data flow

Date: 2026-09-13. Source: HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`.
Comparison baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
External scope: the settled typed-wire-decode plan and normative host protocol.

## Observations

- `packages/opencode-plugin/src/hooks/context/module-wire.ts:940-1093`
  constructs CK blocks inside ingress messages.
  Tool metadata emits `provider_executed` only when true at lines 1012-1014.
- `crates/memory-store/src/lib.rs:126-161,250-279` decodes through retained
  `Value` trees and replays them during serialization at HEAD.
- `crates/daemon/src/wire.rs:540-559` rebuilds message shells but retains
  block originals. Lines 731-736 serialize each block and hash those bytes.
- `crates/daemon/src/transform.rs:164-205` serves canonical message bytes,
  while its block-fingerprint fallback separately calls `to_string(block)`.
- `crates/daemon/src/codec/sidecar.rs:168-174` strips codec metadata, drops
  the original, builds a typed `Value`, and hashes that value.

## Contract and candidate

Plan R4-R7 and KTD2 require a common typed canonical block-byte producer over
`served_json::encode`. That producer is prospective, not present at HEAD.
Candidate: preserve plugin bytes while unifying the three fresh byte bases.

## Narrow nonapplicability and missing evidence

Transport does not parse routed application bodies (`docs/host-wire-protocol.md:763`).
Control-channel duplicate rules do not specify CK envelope identity.
No runtime trace or replacement implementation is supplied.
