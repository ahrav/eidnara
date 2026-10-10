//! A generator of test-only search admission records for a built payload's embedding bundle.
//!
//! Usage: `search_admission_records <bundle-manifest.json> <out-dir>`. The records name the
//! lane identity the daemon derives from that bundle, and their limits, capabilities, and
//! harness runs are the fixture values the daemon's integration tests install, so
//! `eidnara-host install-search-admission` admits the payload's projection under every hook.
//! The campaign is fixture evidence for end-to-end tests on a development payload and carries
//! no qualification measurement.

#![forbid(unsafe_code)]

#[path = "../tests/support/projection_gate.rs"]
#[allow(dead_code)]
mod projection_gate;

use std::error::Error;
use std::path::PathBuf;

use daemon::projection_gates::ProjectionHook;
use retrieval::ProjectionIdentity;

/// Each tool turn commits its transcript to the kernel, so the fixture's four-commit lag leaves a reader stale between slices.
const LAG_COMMITS: u64 = projection_gate::LIMIT;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let (Some(bundle), Some(out), None) = (args.next(), args.next(), args.next()) else {
        return Err("usage: search_admission_records <bundle-manifest.json> <out-dir>".into());
    };
    let bundle: serde_json::Value = serde_json::from_slice(&std::fs::read(&bundle)?)?;
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
        vector_dimension: u32::try_from(number("dims")?)?,
        generation_epoch: number("table_epoch")?,
    };
    let manifest = projection_gate::manifest_json_with(
        &identity,
        &ProjectionHook::ALL,
        &[("catchup_lag_commits", LAG_COMMITS)],
    );
    let campaign = projection_gate::campaign_json(&identity);
    let out = PathBuf::from(out);
    std::fs::create_dir_all(&out)?;
    std::fs::write(
        out.join("runtime-manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    std::fs::write(
        out.join("campaign-evidence.json"),
        serde_json::to_vec_pretty(&campaign)?,
    )?;
    Ok(())
}
