# Dedup cleanup (advisory review follow-up)

Base: u3/base-snapshot. Three stacked units, each its own PR.

1. Delete frame_channel twin: ProducerError/ProducerReservation<C>/ProducedBody,
   LeaseTracker/LeaseClose, tracker plumbing in ReceiveLease, matching
   contract_tests. Keep ReceiveLease/CopyCounter (prod users connection.rs:533,
   ring_transport.rs:546).
2. setup_auth dedup: raw_client.rs local HMAC + domain literals ->
   shm_transport::setup_auth; inline compute_proof wrappers in auth.rs and
   shm-native setup.rs.
3. harness_closure.rs: verify_safe_ancestor -> instance::is_safe_ancestor,
   drop duplicate S_ISVTX, doc leaf-vs-ancestor policy.

Validation per unit: cargo fmt --check, clippy -D warnings (workspace,
all-targets, all-features), cargo test workspace, cargo test --doc.
Review per unit: invariant-test-review, rust-code-reviewer, reduce-complexity,
improve-codebase-architecture (parallel). Final: ponytail-review.
