# `host-proof-construction-matches-the-committed-vectors`

- **Discovery:** U3, when the domain separators and daemon version prefix were renamed.
- **Primary evidence:** `crates/host-runtime/src/auth.rs` re-uses `shm_transport::setup_auth::compute_proof`; `committed_wire_vectors_pin_the_proof_construction` compares it to hex literals and `crates/host-runtime/tests/protocol_vectors.rs` compares `raw_client::proof`, a test-local HMAC over the documented layout, to byte literals. Both literals were computed at U3 by a Python HMAC over the same layout; the same script reproduced the predecessor values from the predecessor strings.
- **Existing evidence:** `committed_wire_vectors_pin_the_proof_construction`, `committed_auth_proof_vectors_pin_the_construction`, `proof_folds_every_input`; all pass on Rust 1.98 and stable.
- **Failure scenario:** a symmetric transcript change; only an external oracle detects it.
- **Timing window:** none.
- **Instrumentation:** none.
- **Audit verdict (U3):** pass. The integration test's oracle does not call the crate's proof helpers (its module doc forbids it), and `proof_folds_every_input` shows each field, including `daemon_ver`, changes the digest.
- **Open-question log:** none.
