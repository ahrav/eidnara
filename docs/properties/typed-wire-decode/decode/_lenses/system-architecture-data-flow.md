# System lens: architecture and data flow

Provenance: HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
The supplied plan is worktree-only settled material. See the catalog scope.

- `crates/daemon/src/lib.rs:12153-12170` takes private request bytes through
  admission, probe, and `dispatch_body` before settling the response.
- `crates/daemon/src/lib.rs:12921-12951` tries the compatibility gate and
  typed decode, then falls back to a metered `Value` decode on invalid input.
- `crates/memory-store/src/lib.rs:126-143,250-264` builds, clones, and retains
  envelope trees inside that ostensibly direct typed decode. The accepted
  replacement is derived serde on both wire structs, without those trees.
- The page path reaches `from_value` at
  `crates/daemon/src/lib.rs:8107-8126`. It intentionally keeps its outer tree.
- The one-pass requirement concerns wire-envelope construction. It does not
  remove the probe, compatibility walk, enum buffering, or page tree.

Candidate: `envelope-decode-has-no-retained-tree`. Source-shape inspection is
necessary beside behavioral checks. No execution or allocation evidence is
produced here; quantitative accounting belongs to the sibling workstream.
