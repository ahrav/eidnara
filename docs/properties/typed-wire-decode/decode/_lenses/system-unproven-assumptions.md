# System lens: unproven assumptions

Provenance: HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.

- The plan says A2 covers explicit-null serializer_profile. The inspected
  corpus covers missing and unknown profiles, not that explicit-null case
  (`crates/daemon/src/lib.rs:19819-19828`). Add a witness rather than infer it.
- The plan assumes only the plugin supplies retained-original CK ingress.
  The host wire treats routed bodies as opaque; no runtime sender-shape
  restriction follows from `docs/host-wire-protocol.md:337`.
- R3 is accepted even though HEAD comments and tests promise lossless unknown
  fields. A blanket unchanged-JSON oracle would contradict accepted R3.
- Removing envelope Value re-deserialization can alter the raw-value-token
  boundary under discarded fields. The existing gate remains conservative;
  a baseline/final fixture is needed to settle acceptance, not a guess.
- W1 is invalidated in the canonical catalog at
  `docs/properties/hot-path-optimization/latency-audit/catalog.md:1768-1773`.

Keep these discrepancies in the synthesis. No decision is silently redesigned.

Independent disposition on 2026-09-13, analyst
`ses_f6756093fffeVjNp36S3E8pKrM`: retain raw-token parity as an unrun
baseline/final requirement and stop-and-report any acceptance change. The
null-profile case preserves the existing boundary without authorizing the
deferred wrapper refactor. Worktree-only TE08's exact captured unknown CK
fields conflict with accepted R3; the specification/integration owners must
record reconciliation before integration while preserving R3. No compatibility
exception is authorized, and all claims remain unexercised.
