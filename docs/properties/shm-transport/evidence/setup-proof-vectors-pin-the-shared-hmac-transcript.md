# `setup-proof-vectors-pin-the-shared-hmac-transcript`

- **Discovery:** U3, when the domain separators and daemon version prefix were renamed and the committed vectors stopped matching.
- **Primary evidence:** `compute_proof` in `crates/shm-transport/src/setup_auth.rs` is the one implementation both peers link. The vectors in `setup_auth::vectors` were computed at U3 by a Python `hmac.new(key, digestmod=sha256)` over `domain || client_nonce || server_nonce || be32(len(ver)) || ver || daemon_id` with the committed inputs; the same script reproduced the predecessor vectors from the predecessor domain and version strings, so the transcript layout is unchanged and only the renamed inputs moved the digests. New server proof `59295f65...4038`, new client proof `8ca1451b...d79f`.
- **Existing evidence:** `committed_vectors_pin_the_shared_construction` and `daemon_ver_is_bound_into_the_proof` (`crates/shm-transport/src/setup_auth.rs`); `committed_wire_vectors_pin_the_proof_construction` (`crates/host-runtime/src/auth.rs`); `committed_auth_proof_vectors_pin_the_construction` and `proof_folds_every_input` (`crates/host-runtime/tests/protocol_vectors.rs`, oracle `raw_client::proof`); `auth_proofs_match_committed_wire_vectors` (`packages/shm-native/src/setup.rs`). All pass on Rust 1.98 and stable.
- **Failure scenario:** a symmetric transcript change keeps both peers interoperating while changing what a captured proof commits to; only the externally computed vector catches it.
- **Timing window:** none.
- **Instrumentation:** none needed; the literals are in the tests.
- **Audit verdict (U3):** pass. Independent oracle: the host-runtime vector test uses `raw_client::proof`, a test-local HMAC over the documented layout, not `compute_proof`; the shm-transport and addon tests compare `compute_proof` against literals that were produced outside the crate. `proof_folds_every_input` shows every field, including `daemon_ver`, changes the digest.
- **Open-question log:** none.
