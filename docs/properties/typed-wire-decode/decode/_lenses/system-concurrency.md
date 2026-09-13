# System lens: concurrency model

Provenance: HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.

- `crates/daemon/src/wire.rs:33-36` owns `Vec<Arc<IngressMessage>>`, not
  references into the transport body. SharedWireBlock keeps another owner.
- `crates/daemon/src/lib.rs:2946` stores the snapshot cache behind
  `Arc<Mutex<_>>`; `:4351-4356` obtains a ready request under that lock.
- `crates/daemon/src/wire.rs:1796-1814` has compile-time `Send + 'static`
  assertions for ingress, projection, and TransformRequest, plus malformed
  decode comparison. The assertions are inventoried, not rerun.
- `crates/daemon/src/wire.rs:1785-1792` exercises copy-on-write shell edits
  while another owner and a projected block survive.

The synchronous decode has no independent consensus or scheduling protocol.
Thread mobility and alias isolation apply; blocking-pool join and cancellation
policy belong to issue 438 and are not inferred from `Send`.
