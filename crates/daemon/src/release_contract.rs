//! Release and compatibility contract, authored as source.
//!
//! `release/host-release.json` is a committed file, not a generated one. The
//! constants here restate the values the daemon compares against at runtime
//! and derive the ones that have another owner (`host_runtime`,
//! `shm_transport`) from that owner; the tests in `release_contract_tests`
//! hold the JSON and the constants together.

use std::sync::OnceLock;

use sha2::{Digest, Sha256};

/// The committed release contract, byte for byte, including its trailing newline.
pub const RELEASE_CONTRACT_JSON: &str = include_str!("../../../release/host-release.json");

/// SHA-256 hex digest of [`RELEASE_CONTRACT_JSON`] without its trailing
/// newline, the form release tooling digests.
pub fn release_contract_sha256() -> &'static str {
    static DIGEST: OnceLock<String> = OnceLock::new();
    DIGEST.get_or_init(|| {
        let trimmed = RELEASE_CONTRACT_JSON
            .strip_suffix('\n')
            .unwrap_or(RELEASE_CONTRACT_JSON);
        format!("{:x}", Sha256::digest(trimmed.as_bytes()))
    })
}

/// Release version, the daemon crate's version.
pub const RELEASE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Bounded daemon version string authenticated by the server proof:
/// `shm_transport::setup_auth::DAEMON_VER_PREFIX` followed by the crate version.
pub const DAEMON_VERSION: &str = "eidnara-host/0.1.0";

/// Version-2 frame protocol.
pub const WIRE_PROTOCOL_VERSION: u8 = 2;

/// Exact five-part epochs.
pub const MEMORY_RENDER_EPOCH: u32 = 2;
pub const COMPARTMENT_RENDER_EPOCH: u32 = 2;
pub const PROFILE_EPOCH_CLAUDE_CODE_ANTHROPIC: u32 = 2;
pub const TAGGER_EPOCH: u32 = 3;
/// Numeric state-sync epoch, advertised alongside the boolean `state_sync_deltas`
/// feature signal in the module status.
pub const STATE_SYNC_EPOCH: u32 = 1;

/// Version-neutral coordination names, owned by `host_runtime`.
pub const COORDINATION_DIRECTORY: &str = host_runtime::COORDINATION_DIR_NAME;
pub const TRANSACTION_LOCK_NAME: &str = host_runtime::TRANSACTION_LOCK_NAME;
pub const LIFETIME_LOCK_NAME: &str = host_runtime::LIFETIME_LOCK_NAME;

/// Managed data-root layout segments, owned by `host_runtime`.
pub const MANAGED_SUBTREE_DIRECTORY: &str = host_runtime::MANAGED_DIR_NAME;
pub const RUNTIME_DIRECTORY_NAME: &str = host_runtime::RUNTIME_DIR_NAME;
pub const CONNECTION_FILE_NAME: &str = host_runtime::CONNECTION_FILE_NAME;
/// The storage segment is the module id.
pub const STORAGE_SUBDIRECTORY: &str = crate::DEFAULT_MODULE_ID;
