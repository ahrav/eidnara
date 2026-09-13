# System lens: existing test strategy

Provenance: HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.

- `crates/daemon/src/lib.rs:19986-20117` compares direct and tree decodes,
  pins accepted and divergent names, and assembles one valid two-page input.
- `crates/daemon/src/lib.rs:20140-20181` compares handler outcomes and lane
  names using an unbounded test pool. It is not a ring fixture.
- `crates/daemon/tests/serialized_transform_pages.rs:11-156` is ignored and
  requires generated TypeScript input. It tests real-host page completion and
  served-message parity for its 19-case corpus.
- `crates/daemon/src/wire.rs:1749-1814` supplies sharing, owner-drop,
  copy-on-write, mobility, and malformed-array checks.
- `crates/daemon/src/transform.rs:13714-13755` deliberately pins stale
  serialization of a latent public-meta edit. Its oracle changes under KTD1.

See `../existing-checks.md` for individual checks and unaudited status.
No tests run. A suite containing a fixture is not exercise evidence here.
