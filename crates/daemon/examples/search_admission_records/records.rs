//! Test-only search admission records for a payload's embedding bundle.
//!
//! The records name the lane identity the daemon derives from that bundle. Their capabilities and harness runs are the fixture values the daemon's integration tests install, so the projection admits under every hook, and the campaign is fixture evidence for end-to-end tests on a development payload with no qualification measurement.
//!
//! The repository carries no qualified production manifest: `eidnara-host serve` keeps the projection closed until an operator installs one. These records therefore size their limits from first principles, for a data home as large as the kernel itself admits, so a fixture daemon that runs for days keeps its projection current as the store grows. Limits the sizing leaves alone keep the integration tests' value of 1,000,000.
//!
//! A rebuild of the projection is one slice: it scans every source descriptor, holds the whole decoded corpus as one batch, writes one pending embedding job per dense-eligible row, stages the whole projection file, and censuses every descriptor when it retires its predecessor. Each of those is charged against a manifest limit, so every store-scaled limit below follows from [`DESCRIPTORS`], [`EVIDENCE_BYTES`], [`TEXT_BYTES`], and [`PROJECTION_BYTES`].

use daemon::projection_gates::ProjectionHook;
use retrieval::ProjectionIdentity;
use serde_json::Value;

use crate::projection_gate;

/// Encoded evidence one source hold may pin: the kernel's artifact cap (`DEFAULT_ARTIFACT_CAP` in `crates/kernel/src/cas/mod.rs`), so a hold over every live descriptor is admitted on any store the kernel admits.
const EVIDENCE_BYTES: u64 = 4 << 30;

/// Decoded source text of the whole corpus. A descriptor exports a span of its evidence, and descriptors that share an artifact each export their span, so the text is bounded at twice the evidence.
const TEXT_BYTES: u64 = 2 * EVIDENCE_BYTES;

/// Source descriptor revisions, live and invalidated: one per 4 KiB of evidence at the cap.
const DESCRIPTORS: u64 = EVIDENCE_BYTES / 4096;

/// Bytes of the closed projection file. It stores each occurrence's text twice (payload and lexical content), its lexical index, one 768-float vector, and about 2 KiB of row keys, which at [`DESCRIPTORS`] rows over [`EVIDENCE_BYTES`] is under five times the evidence.
const PROJECTION_BYTES: u64 = 8 * EVIDENCE_BYTES;

/// Episodes a first build or rebuild record carries (`REGISTRATION_ALLOWANCE` in `search_lifecycle_owner.rs`); the seed bound divides half the disk limit across them and two more.
const REBUILD_ALLOWANCE: u64 = 3;

/// One lifecycle record's bytes (`MAX_RECORD_BYTES`), reserved beside each staged seed.
const RECORD_BYTES: u64 = 64 << 10;

/// The largest canonical object payload (`MAX_OBJECT_PAYLOAD_BYTES`), so every source row fits one export page.
const ROW_BYTES: u64 = 1 << 20;

const HOUR_MS: u64 = 60 * 60 * 1000;

/// One slice. A rebuild's capture, export, verification, and staging all run inside one slice, so the bound is the time a whole-store rebuild may take.
const SLICE_MS: u64 = HOUR_MS;

/// One rebuild or authorized recovery record: its three episodes of one slice each, and one more hour for selection and retirement.
const RECOVERY_MS: u64 = (REBUILD_ALLOWANCE + 1) * SLICE_MS;

/// The kernel's longest source hold (`MAX_SOURCE_HOLD_LIFETIME_MS`). A family catches up under its construction hold, and the hold's expiry costs a whole-store rebuild, so the fixture asks for the longest one.
const HOLD_MS: u64 = 30 * 24 * HOUR_MS;

/// Commits a reader's family may trail the kernel by. The slice loop applies new commits once per idle period of five seconds, so a caught-up family trails by the commits of one period; 1,024 admits a reader through a period of 200 commits per second, above what the kernel's single writer sustains. A family that falls further behind is refused to readers until its catch-up episode brings it back, and a rebuild is admitted while fewer commits than this arrive during it.
pub const READER_LAG_COMMITS: u64 = 1024;

/// The limits these records set, by name.
fn limits() -> Vec<(&'static str, u64)> {
    vec![
        // The capture scans `local_transaction_rows / 2` descriptor rows, retirement censuses as many, and each of the six occurrence classes gets one live and one tombstone share of the limit, so twelve times the descriptors admits a store whose descriptors are all of one class.
        ("local_transaction_rows", 12 * DESCRIPTORS),
        // A class's live share also stays within one export page's rows.
        ("export_page_rows", DESCRIPTORS),
        ("capture_reference_count", DESCRIPTORS),
        // A rebuild batch writes one pending embedding job per dense-eligible row, at most 256 bytes each.
        ("pending_count", DESCRIPTORS),
        ("pending_bytes", 256 * DESCRIPTORS),
        // A rebuild holds the corpus as one batch: its text within `catchup_batch_source_bytes`, half of `export_live_decoded_bytes`, and a quarter of `local_transaction_bytes`, and its encoded evidence within half of `catchup_batch_encoded_bytes`. A quarter of `local_transaction_bytes` also carries the retirement census, about 200 bytes per descriptor.
        ("catchup_batch_source_bytes", TEXT_BYTES),
        ("export_live_decoded_bytes", 2 * TEXT_BYTES),
        ("local_transaction_bytes", 4 * TEXT_BYTES),
        ("catchup_batch_encoded_bytes", 2 * EVIDENCE_BYTES),
        // The hold pins evidence within a quarter of the disk limit, and each staged seed fits half of it divided across the allowance and two more, less one record.
        (
            "capture_disk_bytes",
            (4 * EVIDENCE_BYTES)
                .max(2 * (REBUILD_ALLOWANCE + 2) * (PROJECTION_BYTES + RECORD_BYTES)),
        ),
        ("export_page_encoded_bytes", ROW_BYTES),
        ("export_row_standalone_encoded_bytes", ROW_BYTES),
        ("capture_hold_expiry_ms", HOLD_MS),
        ("supervisor_slice_ms", SLICE_MS),
        ("B_recovery_ms", RECOVERY_MS),
        ("B_authorized_recovery_ms", RECOVERY_MS),
        // Retirement charges a record's remaining time against the drain bound, so the drain bound covers a whole record.
        ("physical_drain_ms", RECOVERY_MS),
        ("catchup_lag_commits", READER_LAG_COMMITS),
    ]
}

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
    let manifest = projection_gate::manifest_json_with(&identity, &ProjectionHook::ALL, &limits());
    let campaign = projection_gate::campaign_json(&identity);
    Ok((manifest, campaign))
}
