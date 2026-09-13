//! Every fresh block-byte consumer hashes the same canonical text: projection
//! identity, the served fingerprint fallback, and the decoded sidecar fingerprint.

#![cfg(feature = "test-support")]

use daemon::codec::sidecar::{
    BLOCK_IDENTITY_NAMESPACE_FOR_TEST, decoded_block_fingerprint_for_test,
};
use daemon::served_json::canonical_block_bytes_for_test;
use daemon::transform::served_message_for_test;
use daemon::wire::{IngressMessage, IngressMessages, project_messages};
use sha2::{Digest, Sha256};

fn corpus() -> Vec<IngressMessage> {
    serde_json::from_str(include_str!("../testdata/ingress-projection-corpus.json"))
        .expect("corpus decodes")
}

fn hex(digest: &[u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn projection_fallback_and_sidecar_hash_the_same_canonical_bytes() {
    let messages = corpus();
    let projection = project_messages(&messages.iter().cloned().collect::<IngressMessages>())
        .expect("corpus projects");
    let mut checked = 0;
    for message in &messages {
        // No projected receipts are supplied, so every fingerprint is a fresh hash.
        let served = served_message_for_test(message.ck.clone());
        let fingerprints = served.block_fingerprints_for_test();
        assert_eq!(
            fingerprints.len(),
            message.ck.content().len(),
            "{}",
            message.mid
        );
        for (index, block) in message.ck.content().iter().enumerate() {
            let flat = projection
                .blocks
                .iter()
                .find(|flat| flat.mid == message.mid && flat.block_index == index)
                .unwrap_or_else(|| panic!("{}#{index} projected", message.mid));
            let canonical = canonical_block_bytes_for_test(block);
            assert_eq!(
                *flat.bytes, *canonical,
                "{}#{index}: projection bytes",
                message.mid
            );
            assert_eq!(
                hex(&flat.content_hash),
                fingerprints[index].0,
                "{}#{index}: projection hash equals fresh fallback fingerprint",
                message.mid
            );
            assert_eq!(
                fingerprints[index].1,
                canonical.len(),
                "{}#{index}",
                message.mid
            );
            assert!(
                !block
                    .provider_extras
                    .contains_key(BLOCK_IDENTITY_NAMESPACE_FOR_TEST),
                "{}#{index}: ingress never carries the codec namespace",
                message.mid
            );
            // Without the codec namespace the sidecar fingerprint is the same digest
            // of the same bytes.
            assert_eq!(
                decoded_block_fingerprint_for_test(block),
                hex(&Sha256::digest(canonical.as_bytes()).into()),
                "{}#{index}: sidecar fingerprint",
                message.mid
            );
            checked += 1;
        }
    }
    assert_eq!(checked, projection.blocks.len());
    assert!(checked >= 27, "the corpus covers every emitted shape");
}
