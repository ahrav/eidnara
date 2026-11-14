//! Test-only search admission records for a payload's embedding bundle.
//!
//! The records name the lane identity the daemon derives from that bundle, and their limits,
//! capabilities, and harness runs are the fixture values the daemon's integration tests install,
//! so the projection admits under every hook. The campaign is fixture evidence for end-to-end
//! tests on a development payload and carries no qualification measurement.

use daemon::projection_gates::ProjectionHook;
use retrieval::ProjectionIdentity;
use serde_json::Value;

use crate::projection_gate;

/// Encoded evidence the records admit one source hold to pin: the kernel's artifact cap (`DEFAULT_ARTIFACT_CAP` in `crates/kernel/src/cas/mod.rs`), so a hold over every live descriptor is admitted on any store the kernel admits.
const EVIDENCE_BYTES: u64 = 4 << 30;

/// Decoded source text the records admit. A descriptor exports a span of its evidence, and descriptors that share an artifact each export their span, so the text is bounded at twice the evidence.
const TEXT_BYTES: u64 = 2 * EVIDENCE_BYTES;

/// Source descriptor revisions the records admit.
const DESCRIPTORS: u64 = 1 << 20;

/// Bytes the closed projection file may occupy. The file stores each occurrence's text twice (payload and lexical content), its lexical index, one 768-float vector, and about 2 KiB of row keys.
const PROJECTION_BYTES: u64 = 8 * EVIDENCE_BYTES;

/// Episodes a rebuild record carries; the seed bound divides the disk limit across them and two more.
const REBUILD_ALLOWANCE: u64 = 3;

const HOUR_MS: u64 = 60 * 60 * 1000;

/// The runtime manifest and campaign evidence records for the parsed bundle manifest `bundle`.
pub fn records(bundle: &Value) -> Result<(Value, Value), String> {
    let text = |key: &str| {
        bundle[key]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("bundle manifest names no {key}"))
    };
    let number = |key: &str| {
        bundle[key]
            .as_u64()
            .ok_or_else(|| format!("bundle manifest names no {key}"))
    };
    let identity = ProjectionIdentity {
        schema_version: retrieval::SCHEMA_VERSION,
        // The records' invalidation identity carries every field except the kernel incarnation.
        kernel_incarnation_id: String::new(),
        projection_policy_version: daemon::search_lifecycle_owner::PROJECTION_POLICY_VERSION
            .to_owned(),
        identity_contract_version: daemon::search_lifecycle_owner::IDENTITY_CONTRACT_VERSION
            .to_owned(),
        limit_manifest_protocol_version: projection_gate::LIMITS.to_owned(),
        embedding_model: text("model")?,
        tokenizer_fingerprint: text("fingerprint")?,
        analysis_identity: retrieval::lexical::AnalysisIdentity::current()
            .as_str()
            .to_owned(),
        vector_dimension: u32::try_from(number("dims")?).map_err(|error| error.to_string())?,
        generation_epoch: number("table_epoch")?,
    };
    let mut overrides: Vec<(&str, u64)> = vec![
        ("local_transaction_rows", 12 * DESCRIPTORS),
        ("export_page_rows", DESCRIPTORS),
        ("capture_reference_count", DESCRIPTORS),
        ("pending_count", DESCRIPTORS),
        ("pending_bytes", 256 * DESCRIPTORS),
        ("catchup_batch_source_bytes", TEXT_BYTES),
        ("export_live_decoded_bytes", 2 * TEXT_BYTES),
        ("local_transaction_bytes", 4 * TEXT_BYTES),
        ("catchup_batch_encoded_bytes", 2 * EVIDENCE_BYTES),
        (
            "capture_disk_bytes",
            (4 * EVIDENCE_BYTES).max(2 * (REBUILD_ALLOWANCE + 2) * (PROJECTION_BYTES + (64 << 10))),
        ),
        ("export_page_encoded_bytes", 1 << 20),
        ("export_row_standalone_encoded_bytes", 1 << 20),
        ("capture_hold_expiry_ms", 30 * 24 * HOUR_MS),
        ("supervisor_slice_ms", HOUR_MS),
        ("B_recovery_ms", 4 * HOUR_MS),
        ("B_authorized_recovery_ms", 4 * HOUR_MS),
        ("physical_drain_ms", 4 * HOUR_MS),
        ("catchup_lag_commits", 64),
    ];
    // Probe override, removed before review: `R2_LIMITS` is a JSON object of limit values.
    let probe: std::collections::BTreeMap<String, u64> = std::env::var("R2_LIMITS")
        .ok()
        .map(|text| serde_json::from_str(&text).expect("R2_LIMITS is a JSON object of integers"))
        .unwrap_or_default();
    let names: Vec<&'static str> = daemon::projection_gates::REQUIRED_LIMITS.to_vec();
    for name in names {
        if let Some(value) = probe.get(name) {
            overrides.retain(|(known, _)| *known != name);
            overrides.push((name, *value));
        }
    }
    let manifest = projection_gate::manifest_json_with(&identity, &ProjectionHook::ALL, &overrides);
    let campaign = projection_gate::campaign_json(&identity);
    Ok((manifest, campaign))
}
