mod common;

use common::digest_hex;

const UPSTREAM_DEFAULT_RULES_SHA256: &str =
    "2f1292b50148d38afe3ebdb7c489449d103b75b7df464e06da0d5d7c89ac2820";

#[test]
fn embedded_documents_match_their_pinned_digests() {
    assert_eq!(
        digest_hex(include_bytes!("../default_rules.yaml")),
        secret_scanner::UPSTREAM_CORPUS_SHA256
    );
    assert_eq!(
        digest_hex(include_bytes!("../conservative_overlay.yaml")),
        secret_scanner::CONSERVATIVE_OVERLAY_SHA256
    );
}

/// The embedded corpus is the Gossip-rs corpus with one character class
/// spelled in the other letter order. Restoring the upstream order restores
/// the upstream digest, so no other byte differs.
#[test]
fn corpus_is_upstream_with_one_character_class_reordered() {
    let embedded = include_str!("../default_rules.yaml");
    let reordered = "CM".chars().rev().collect::<String>();
    assert_eq!(embedded.matches("EAA[CM]").count(), 1);
    let upstream = embedded.replacen("EAA[CM]", &format!("EAA[{reordered}]"), 1);
    assert_eq!(
        digest_hex(upstream.as_bytes()),
        UPSTREAM_DEFAULT_RULES_SHA256
    );
}

#[test]
fn notice_names_every_adapted_source_and_digest() {
    let notice = include_str!("../NOTICE");
    for required in [
        "https://github.com/ahrav/gossip-rs",
        "3d2869011138cd7812a12f893dc93635a961b0d7",
        "909e835f6d19a923aefa84484cd7fa215ffad973",
        UPSTREAM_DEFAULT_RULES_SHA256,
        secret_scanner::UPSTREAM_CORPUS_SHA256,
        secret_scanner::CONSERVATIVE_OVERLAY_SHA256,
        "EAA[CM]",
        "Copyright (c) 2026 ahrav",
        "MIT",
    ] {
        assert!(
            notice.contains(required),
            "missing provenance field {required}"
        );
    }
    for path in [
        "crates/scanner-engine/default_rules.yaml",
        "crates/scanner-engine/src/api.rs",
        "crates/scanner-engine/src/rules/yaml.rs",
        "crates/scanner-engine/src/engine/helpers/entropy.rs",
        "crates/scanner-engine/src/engine/offline_validate.rs",
        "crates/scanner-engine/src/engine/safelist.rs",
        "crates/scanner-engine/src/engine/window_validate.rs",
        "LICENSE",
    ] {
        assert!(notice.contains(path), "unattributed source {path}");
    }
}
