# setup-proof-vectors-pin-the-shared-hmac-transcript

## Discovery trigger

U3 renamed the domain separators (`eidnara-server-v1`, `eidnara-client-v1`) and the daemon version prefix (`eidnara-host/0.1.0`), and the committed proof vectors stopped matching. Because both peers link one `compute_proof`, a symmetric transcript change would interoperate while changing what a captured proof commits to; only a vector produced outside the implementation can catch that.

## Evidence trail

- `compute_proof` (`crates/shm-transport/src/setup_auth.rs:40`) is the implementation both peers use; `crates/host-runtime/src/auth.rs:132` re-exports the same construction for the host.
- The vectors in `setup_auth::vectors` were recomputed at U3 with a Python `hmac.new(key, digestmod=sha256)` over `domain || client_nonce || server_nonce || be32(len(ver)) || ver || daemon_id` with the committed inputs (key `00..1f`, nonces `20..3f` and `40..5f`, daemon id `60..6f`). The same script reproduced the predecessor vectors from the predecessor strings, so the layout is unchanged and only the renamed inputs moved the digests. New server proof `59295f65...4038`, new client proof `8ca1451b...d79f`.
- `committed_vectors_pin_the_shared_construction` (`setup_auth.rs:331`) and `daemon_ver_is_bound_into_the_proof` (`:358`) compare `compute_proof` against the literals.
- `committed_wire_vectors_pin_the_proof_construction` (`crates/host-runtime/src/auth.rs:654`) pins the host side.
- `committed_auth_proof_vectors_pin_the_construction` (`crates/host-runtime/tests/protocol_vectors.rs:33`) uses the test-local `raw_client::proof`, an HMAC written over the documented layout rather than a call into `compute_proof`; `proof_folds_every_input` (`:146`) shows every field, including `daemon_ver`, changes the digest.
- `auth_proofs_match_committed_wire_vectors` (`packages/shm-native/src/setup.rs:626`) pins the addon side against the same literals.
- All of the above pass under `cargo test --workspace` on Rust 1.98 (CI) and stable.

## Failure scenario

A field-order, length-prefix, or domain-string change applied to both peers at once. Interoperation continues, every in-crate round-trip test passes, and only the externally computed literal disagrees.

## Timing windows and dependencies

None. The proof is a pure function of the committed inputs.

## What a test must construct

- Present: the committed literals on all three implementations and one independent oracle (`raw_client::proof`).
- Missing: nothing for the transcript itself. A change to the committed inputs must regenerate the vectors with an oracle that is not `compute_proof`, and the regeneration script is not in the tree.

## Investigation log

### Q: Are the U3 vectors the output of the documented layout over the renamed inputs, or were they copied from an implementation run?

- Sources examined: `setup_auth::vectors`, the Python HMAC reproduction of both the predecessor and the U3 vectors, `protocol_vectors.rs`.
- Findings: The Python oracle reproduced the predecessor vectors from the predecessor strings and the U3 vectors from the U3 strings with no other change, and `raw_client::proof` reproduces them in-tree.
- Missing evidence: The regeneration script itself is not committed.
- Conclusion: resolved with answer: the vectors are oracle-produced; commit the regeneration script if the inputs change again.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 9, `crates/host-runtime/src/auth.rs:119` now `crates/host-runtime/src/auth.rs:132`: auth.rs re-exports only the domain and length constants at `:12-14`; compute_proof is a thin wrapper function that forwards to shm_transport::setup_auth::compute_proof.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
