# Property lens: protocol contracts

Provenance: HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.

- Extend A2 with message/block duplicate keys, null/default distinctions,
  unknown enum tags, wrong envelope kinds, and malformed ignored fields.
- Retain discriminator precedence and any-page-key presence checks at
  `crates/daemon/src/lib.rs:12971-12975,13061-13064,15965-15976`.
- `crates/daemon/src/lib.rs:9658-9664` verifies page-array digest before
  typed decoding. Discarding an unknown CK field earlier would hash another
  object and violate KTD5 even though R3 later discards that field.
- Non-transform dispatch retains its Value, including explicit echo at
  `crates/daemon/src/lib.rs:13047-13057`.

Candidate: `page-and-other-routes-retain-tree-semantics`. Page digest is not
block identity; preserve it here. The host's strict control protocol is a
different boundary, not a reason to reject CK duplicate keys.
