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

/// Replaces the fixture's four-commit `LAG_LIMIT` with `LIMIT`. Each tool turn commits its transcript to the kernel, so a four-commit lag marks a reader stale between slices, and `LIMIT` lets a search read the family while it catches up.
const LAG_COMMITS: u64 = projection_gate::LIMIT;

/// Catch-up applies each commit within one window of at most `local_transaction_rows / 2` and `local_transaction_bytes / 4 / 4096` rows and `catchup_batch_encoded_bytes / 2` payload bytes, and a commit past that window stops catch-up at that commit. A source publication writes three rows per descriptor, and one capture batch carries up to 32 units of 64 KiB, so the fixture's `LIMIT` of 1,000,000 for all three admits windows of only 61 rows, and a publication of 21 descriptors, which a session of a few dozen tool calls produces, stops catch-up. These values admit windows of 4,096 rows and 8 MiB, four times a whole capture batch's text.
const WINDOW_LIMITS: [(&str, u64); 3] = [
    ("local_transaction_rows", 2 * 4096),
    ("local_transaction_bytes", 4 * 4096 * 4096),
    ("catchup_batch_encoded_bytes", 2 * (8 << 20)),
];

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
    let mut overrides = vec![("catchup_lag_commits", LAG_COMMITS)];
    overrides.extend(WINDOW_LIMITS);
    let manifest = projection_gate::manifest_json_with(&identity, &ProjectionHook::ALL, &overrides);
    let campaign = projection_gate::campaign_json(&identity);
    Ok((manifest, campaign))
}
