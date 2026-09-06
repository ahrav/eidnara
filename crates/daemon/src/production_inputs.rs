//! Inventory of qualified production harness inputs, authored as source.
//!
//! The closure manifests and the lock file are committed under `release/`;
//! each closure digest is what `host_runtime::harness_closure::manifest_digest`
//! computes for its manifest, held together by
//! `tests/release_contract_conformance.rs`.

use std::sync::OnceLock;

use sha2::{Digest, Sha256};

const PRODUCTION_INPUTS_LOCK_JSON: &str =
    include_str!("../../../release/production-inputs.lock.json");

/// SHA-256 hex digest of `release/production-inputs.lock.json`.
pub fn production_inputs_lock_sha256() -> &'static str {
    static DIGEST: OnceLock<String> = OnceLock::new();
    DIGEST.get_or_init(|| {
        format!(
            "{:x}",
            Sha256::digest(PRODUCTION_INPUTS_LOCK_JSON.as_bytes())
        )
    })
}

/// `(harness, canonical manifest digest, manifest JSON)` for every qualified closure.
pub const QUALIFIED_HARNESS_CLOSURES: &[(&str, &str, &str)] = &[
    (
        "opencode",
        "4b61f1d84075351d2195e98148beddd0341a3725ffa870da5ef15b02674abfde",
        include_str!("../../../release/harness-closures/opencode-linux-x64-1.18.22.json"),
    ),
    (
        "pi",
        "23b6a41046463ca1d046e281585b4815814a4490b512c176d93de2c897c4c45c",
        include_str!("../../../release/harness-closures/pi-linux-x64-node-24.18.0.json"),
    ),
];
