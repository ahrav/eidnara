# Property lens: version compatibility

Provenance: HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.

- Preserve published names and literals. The host transport remains version 2
  (`docs/host-wire-protocol.md:4`); CK behavior comes from the accepted plan
  and daemon model, not the channel-0 schema.
- R3 explicitly changes retention of unknown CK fields. R5 explicitly accepts
  default-false omissions. Do not classify either as unchanged raw replay.
- Unknown variants and malformed known fields are not unknown fields.
  `crates/memory-store/src/lib.rs:326-356,388-400` still defines closed enums.
- Defaults, optional nulls, explicit false, and omitted provider_executed
  require different witnesses. The producer uses true-only emission at
  `packages/opencode-plugin/src/hooks/context/module-wire.ts:1013-1014`.

Candidate: payload separation with fallback compatibility. Historic storage
decode and fingerprint migration remain with the identity/historian agent.
No cross-version execution evidence is imported from issue descriptions.
