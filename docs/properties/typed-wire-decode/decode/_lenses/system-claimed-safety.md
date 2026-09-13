# System lens: claimed safety guarantees

Provenance: HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.

- Settled plan R1-R3 and KTD1 require derived wire structs, discarded unknown
  envelope fields, and fallback-preserved acceptance and outcomes.
- HEAD comments promise lossless unknown-field replay at
  `crates/memory-store/src/lib.rs:108-111,238-240` and
  `crates/daemon/src/wire.rs:17-19`. This is an explicit contract transition,
  not evidence that removal already works.
- HEAD tests pin retained sibling fields at
  `crates/memory-store/src/lib.rs:16096-16112` and
  `crates/daemon/src/wire.rs:1708-1745`. Accepted R3 supersedes those retention
  expectations; payload preservation and sibling isolation still apply.
- `docs/host-wire-protocol.md:291-292,337-354` separates opaque routed
  application bodies from strict channel-0 controls. Its duplicate-field
  rejection rule is not a transform-body rejection rule.

Candidates: envelope shape, unknown/payload separation, and mutation records.
Do not count a statement in the plan as observed satisfaction.
