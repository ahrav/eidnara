//! Setup-handshake proof transcript. Both ends of the handshake link this module rather
//! than keeping their own copy, so the MAC inputs cannot drift apart. `compute_proof`
//! produces a proof and `verify_proof` checks a received one; both run the same private
//! transcript, so neither peer compares proof bytes itself. `vectors` pins the output.

use hmac::{Hmac, Mac};
use sha2::Sha256;

/// Wire version stamped into a grant and echoed by the activating peer; mismatch is fatal.
pub const PROTOCOL_VERSION: u8 = 2;

/// Nonce bytes each side contributes.
pub const NONCE_LEN: usize = 32;
/// Proof bytes carried by each auth message.
pub const PROOF_LEN: usize = 32;
/// Daemon identity bytes bound into every proof.
pub const DAEMON_ID_LEN: usize = 16;

/// Upper bound on one authentication message body.
pub const MAX_AUTH_MESSAGE_LEN: usize = 4096;
/// Upper bound on one setup message body.
pub const MAX_SETUP_MESSAGE_LEN: usize = 16 * 1024;
/// File descriptors a grant transfers: the mapping and two doorbells per direction.
pub use crate::descriptor::SETUP_DESCRIPTOR_COUNT as RING_DESCRIPTOR_COUNT;

/// Domain separator for the host's proof. A proof computed under the client domain never
/// verifies here even with the same key and nonces.
pub const SERVER_PROOF_DOMAIN: &str = "eidnara-server-v1";
/// Domain separator for the peer's proof.
pub const CLIENT_AUTH_DOMAIN: &str = "eidnara-client-v1";
/// Role string a connecting peer presents.
pub const DEFAULT_CLIENT_ROLE: &str = "client";
/// Prefix of every `daemon_ver` string the host publishes; the remainder is its version.
pub const DAEMON_VER_PREFIX: &str = "eidnara-host/";

/// HMAC-SHA256 over `domain || client_nonce || server_nonce || len(daemon_ver) as u32 BE
/// || daemon_ver || daemon_id`. Including `daemon_ver` in the MAC prevents a peer without
/// `key` from altering the reported version. `daemon_ver` has a length prefix and
/// `daemon_id` has a fixed length, so distinct input tuples cannot share a MAC input.
pub fn compute_proof(
    key: &[u8],
    domain: &str,
    client_nonce: &[u8; NONCE_LEN],
    server_nonce: &[u8; NONCE_LEN],
    daemon_ver: &str,
    daemon_id: &[u8; DAEMON_ID_LEN],
) -> [u8; PROOF_LEN] {
    transcript_mac(
        key,
        domain,
        client_nonce,
        server_nonce,
        daemon_ver,
        daemon_id,
    )
    .finalize()
    .into_bytes()
    .into()
}

/// `Mac::verify_slice` performs a constant-time comparison. Callers must use this rather
/// than `==` on proof bytes.
pub fn verify_proof(
    key: &[u8],
    domain: &str,
    client_nonce: &[u8; NONCE_LEN],
    server_nonce: &[u8; NONCE_LEN],
    daemon_ver: &str,
    daemon_id: &[u8; DAEMON_ID_LEN],
    proof: &[u8; PROOF_LEN],
) -> Result<(), ProofMismatch> {
    transcript_mac(
        key,
        domain,
        client_nonce,
        server_nonce,
        daemon_ver,
        daemon_id,
    )
    .verify_slice(proof)
    .map_err(|_| ProofMismatch)
}

/// Carries nothing, so it cannot leak which byte differed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("setup proof does not match transcript")]
pub struct ProofMismatch;

fn transcript_mac(
    key: &[u8],
    domain: &str,
    client_nonce: &[u8; NONCE_LEN],
    server_nonce: &[u8; NONCE_LEN],
    daemon_ver: &str,
    daemon_id: &[u8; DAEMON_ID_LEN],
) -> Hmac<Sha256> {
    let daemon_ver_bytes = daemon_ver.as_bytes();
    let daemon_ver_len =
        u32::try_from(daemon_ver_bytes.len()).expect("auth messages bound daemon_ver to u32");
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts keys of any length");
    mac.update(domain.as_bytes());
    mac.update(client_nonce);
    mac.update(server_nonce);
    mac.update(&daemon_ver_len.to_be_bytes());
    mac.update(daemon_ver_bytes);
    mac.update(daemon_id);
    mac
}

/// Committed proof vectors. Public rather than `cfg(test)` so both ends assert against the
/// same literals instead of against their own output, which would pass even if both drifted.
pub mod vectors {
    use super::{DAEMON_ID_LEN, NONCE_LEN, PROOF_LEN};

    /// Daemon version the committed proofs are computed over.
    pub const DAEMON_VER: &str = "eidnara-host/0.1.0";

    /// Host proof over the committed inputs.
    pub const SERVER_PROOF: [u8; PROOF_LEN] = [
        89, 41, 95, 101, 15, 43, 108, 51, 132, 228, 206, 117, 229, 243, 55, 238, 35, 54, 116, 7,
        168, 92, 82, 74, 242, 210, 114, 64, 98, 38, 64, 56,
    ];

    /// Peer proof over the committed inputs.
    pub const CLIENT_AUTH: [u8; PROOF_LEN] = [
        140, 161, 69, 27, 18, 230, 236, 54, 6, 199, 49, 76, 154, 250, 81, 84, 78, 160, 182, 108,
        253, 146, 214, 55, 25, 147, 137, 168, 222, 41, 215, 159,
    ];

    /// Key `00..1f`, client nonce `20..3f`, server nonce `40..5f`, daemon ID `60..6f`.
    pub fn inputs() -> (
        [u8; 32],
        [u8; NONCE_LEN],
        [u8; NONCE_LEN],
        [u8; DAEMON_ID_LEN],
    ) {
        (
            std::array::from_fn(|index| index as u8),
            std::array::from_fn(|index| index as u8 + 0x20),
            std::array::from_fn(|index| index as u8 + 0x40),
            std::array::from_fn(|index| index as u8 + 0x60),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::vectors;
    use super::*;

    #[test]
    fn descriptor_count_matches_setup_contract() {
        // The admission charge and the setup transfer count describe the same descriptors.
        let profile = crate::profile::host_test_ring_profile().unwrap();
        assert_eq!(
            profile.charges().file_descriptors,
            RING_DESCRIPTOR_COUNT as u64
        );
    }

    #[test]
    fn verify_proof_accepts_committed_vectors_and_rejects_every_altered_input() {
        let (key, client_nonce, server_nonce, daemon_id) = vectors::inputs();
        assert_eq!(
            verify_proof(
                &key,
                SERVER_PROOF_DOMAIN,
                &client_nonce,
                &server_nonce,
                vectors::DAEMON_VER,
                &daemon_id,
                &vectors::SERVER_PROOF,
            ),
            Ok(())
        );
        assert_eq!(
            verify_proof(
                &key,
                CLIENT_AUTH_DOMAIN,
                &client_nonce,
                &server_nonce,
                vectors::DAEMON_VER,
                &daemon_id,
                &vectors::CLIENT_AUTH,
            ),
            Ok(())
        );

        let mut wrong_key = key;
        wrong_key[0] ^= 1;
        let mut wrong_client_nonce = client_nonce;
        wrong_client_nonce[NONCE_LEN - 1] ^= 1;
        let mut wrong_server_nonce = server_nonce;
        wrong_server_nonce[0] ^= 1;
        let mut wrong_daemon_id = daemon_id;
        wrong_daemon_id[0] ^= 1;
        let mut wrong_proof = vectors::SERVER_PROOF;
        wrong_proof[PROOF_LEN - 1] ^= 1;

        let rejects = |key: &[u8],
                       domain: &str,
                       client_nonce: &[u8; NONCE_LEN],
                       server_nonce: &[u8; NONCE_LEN],
                       daemon_ver: &str,
                       daemon_id: &[u8; DAEMON_ID_LEN],
                       proof: &[u8; PROOF_LEN],
                       altered: &str| {
            assert_eq!(
                verify_proof(
                    key,
                    domain,
                    client_nonce,
                    server_nonce,
                    daemon_ver,
                    daemon_id,
                    proof
                ),
                Err(ProofMismatch),
                "altered {altered} must not verify"
            );
        };
        let ver = vectors::DAEMON_VER;
        let ok = &vectors::SERVER_PROOF;
        rejects(
            &wrong_key,
            SERVER_PROOF_DOMAIN,
            &client_nonce,
            &server_nonce,
            ver,
            &daemon_id,
            ok,
            "key",
        );
        rejects(
            &key,
            CLIENT_AUTH_DOMAIN,
            &client_nonce,
            &server_nonce,
            ver,
            &daemon_id,
            ok,
            "domain",
        );
        rejects(
            &key,
            SERVER_PROOF_DOMAIN,
            &wrong_client_nonce,
            &server_nonce,
            ver,
            &daemon_id,
            ok,
            "client nonce",
        );
        rejects(
            &key,
            SERVER_PROOF_DOMAIN,
            &client_nonce,
            &wrong_server_nonce,
            ver,
            &daemon_id,
            ok,
            "server nonce",
        );
        rejects(
            &key,
            SERVER_PROOF_DOMAIN,
            &client_nonce,
            &server_nonce,
            "eidnara-host/9.9.9",
            &daemon_id,
            ok,
            "daemon_ver",
        );
        rejects(
            &key,
            SERVER_PROOF_DOMAIN,
            &client_nonce,
            &server_nonce,
            ver,
            &wrong_daemon_id,
            ok,
            "daemon_id",
        );
        rejects(
            &key,
            SERVER_PROOF_DOMAIN,
            &client_nonce,
            &server_nonce,
            ver,
            &daemon_id,
            &wrong_proof,
            "proof",
        );
    }

    #[test]
    fn verify_proof_agrees_with_compute_proof() {
        let key = [7u8; 32];
        let client_nonce = [1u8; NONCE_LEN];
        let server_nonce = [2u8; NONCE_LEN];
        let daemon_id = [3u8; DAEMON_ID_LEN];
        let proof = compute_proof(
            &key,
            CLIENT_AUTH_DOMAIN,
            &client_nonce,
            &server_nonce,
            "ab",
            &daemon_id,
        );
        assert_eq!(
            verify_proof(
                &key,
                CLIENT_AUTH_DOMAIN,
                &client_nonce,
                &server_nonce,
                "ab",
                &daemon_id,
                &proof
            ),
            Ok(())
        );
    }

    #[test]
    fn committed_daemon_ver_carries_the_published_prefix() {
        assert!(vectors::DAEMON_VER.starts_with(DAEMON_VER_PREFIX));
        assert!(!DAEMON_VER_PREFIX.is_empty());
    }

    #[test]
    fn committed_vectors_pin_the_shared_construction() {
        let (key, client_nonce, server_nonce, daemon_id) = vectors::inputs();
        assert_eq!(
            compute_proof(
                &key,
                SERVER_PROOF_DOMAIN,
                &client_nonce,
                &server_nonce,
                vectors::DAEMON_VER,
                &daemon_id,
            ),
            vectors::SERVER_PROOF,
        );
        assert_eq!(
            compute_proof(
                &key,
                CLIENT_AUTH_DOMAIN,
                &client_nonce,
                &server_nonce,
                vectors::DAEMON_VER,
                &daemon_id,
            ),
            vectors::CLIENT_AUTH,
        );
    }

    #[test]
    fn daemon_ver_is_bound_into_the_proof() {
        let (key, client_nonce, server_nonce, daemon_id) = vectors::inputs();
        let baseline = compute_proof(
            &key,
            SERVER_PROOF_DOMAIN,
            &client_nonce,
            &server_nonce,
            vectors::DAEMON_VER,
            &daemon_id,
        );
        let tampered = compute_proof(
            &key,
            SERVER_PROOF_DOMAIN,
            &client_nonce,
            &server_nonce,
            "eidnara-host/9.9.9",
            &daemon_id,
        );
        assert_ne!(baseline, tampered, "daemon_ver must change the proof");
    }

    #[test]
    fn domains_separate_the_two_proofs() {
        let (key, client_nonce, server_nonce, daemon_id) = vectors::inputs();
        assert_ne!(
            compute_proof(
                &key,
                SERVER_PROOF_DOMAIN,
                &client_nonce,
                &server_nonce,
                vectors::DAEMON_VER,
                &daemon_id,
            ),
            compute_proof(
                &key,
                CLIENT_AUTH_DOMAIN,
                &client_nonce,
                &server_nonce,
                vectors::DAEMON_VER,
                &daemon_id,
            ),
        );
    }
}
