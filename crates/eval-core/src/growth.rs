//! Sustainability under churn: resource samples at every quiescence of a
//! campaign that never restores, reviewer-quota headroom accounted from the
//! store's own constants, the swarm mix a growth campaign must exercise, and
//! the isolation two concurrent campaigns must keep.

use std::collections::{BTreeMap, BTreeSet};

use context_core::canonical_json::{
    ContractError, canonical_json_encode, is_lower_hex, protocol_digest,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::checkpoint::StoreFamily;
use crate::fault::{ExpectedRefusal, RecordedRefusal};

pub const GROWTH_REPORT_SCHEMA: &str = "eval-suite-c-growth-report/v1";
const GROWTH_RESULT_DIGEST_PROTOCOL: &str = "eval-suite-c-growth-report-result/v1";
const SAMPLE_BYTE_FIELDS: [&str; 3] = ["stores", "artifact_bytes", "cassette_bytes"];

/// The reviewer quota constants as the memory store declares them; the
/// report carries the values it read, never a figure copied from a document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewerQuota {
    pub receipt_charge_bytes: u64,
    pub job_allowance_bytes: u64,
    pub project_metadata_bytes: u64,
    pub host_metadata_bytes: u64,
}

impl ReviewerQuota {
    /// Bytes the quota should hold for the jobs counted: a permanent receipt
    /// charge per terminal job, the receipt charge plus the pending allowance
    /// per job still open, plus whatever frozen pages are charged. `None` when
    /// the counts do not fit in `u64`.
    pub fn expected_project_bytes(&self, headroom: &HeadroomSample) -> Option<u64> {
        self.receipt_charge_bytes
            .checked_mul(headroom.terminal_jobs)?
            .checked_add(
                self.receipt_charge_bytes
                    .checked_add(self.job_allowance_bytes)?
                    .checked_mul(headroom.pending_jobs)?,
            )?
            .checked_add(headroom.page_bytes)
    }

    /// How many more admissions the remaining bytes allow, as a report figure
    /// derived from the constants read; not an acceptance count.
    pub fn admissions_remaining(&self, remaining_bytes: u64) -> u64 {
        self.receipt_charge_bytes
            .checked_add(self.job_allowance_bytes)
            .and_then(|charge| remaining_bytes.checked_div(charge))
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeadroomSample {
    pub pending_jobs: u64,
    pub terminal_jobs: u64,
    pub page_bytes: u64,
    pub project_metadata_bytes: u64,
    pub project_metadata_remaining: u64,
    pub admitted_total: u64,
    pub r24_refusals: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreBytes {
    pub file: u64,
    pub wal: u64,
    pub shm: u64,
}

/// Everything a campaign holds at one quiescent point.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceSample {
    pub step: u32,
    pub commit_seq: i64,
    pub stores: BTreeMap<StoreFamily, StoreBytes>,
    pub artifact_objects: u64,
    pub artifact_tmp_entries: u64,
    pub artifact_bytes: u64,
    pub cassette_bytes: u64,
    pub temp_roots: u64,
    pub processes: u64,
    pub commit_log_rows: u64,
    pub projection_rows: u64,
    pub open_holds: u64,
    pub headroom: HeadroomSample,
}

impl ResourceSample {
    /// Store bytes with their sidecars, saturating so a read past `u64` is a
    /// refusal downstream and not a panic.
    pub fn store_total(&self) -> u64 {
        self.stores
            .values()
            .fold(0u64, |t, b| saturating_sum(t, [b.file, b.wal, b.shm]))
    }

    /// Main database-file bytes, excluding the `-wal` and `-shm` sidecars.
    pub fn durable_store_bytes(&self) -> u64 {
        self.stores
            .values()
            .fold(0u64, |t, b| t.saturating_add(b.file))
    }

    fn wal_bytes(&self) -> u64 {
        self.stores
            .values()
            .fold(0u64, |t, b| t.saturating_add(b.wal))
    }
}

fn saturating_sum(start: u64, terms: impl IntoIterator<Item = u64>) -> u64 {
    terms.into_iter().fold(start, u64::saturating_add)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrowthMode {
    /// No checkpoint restore anywhere in the run; a restore is a refusal.
    NeverRestored,
    /// Restores may happen; the samples cannot support a leak verdict.
    Restoring,
}

/// Per-resource bounds the manifest declares for the final sample.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrowthBounds {
    pub store_bytes: u64,
    pub artifact_objects: u64,
    pub artifact_bytes: u64,
    pub commit_log_rows: u64,
    pub projection_rows: u64,
    pub open_holds: u64,
    /// Store bytes the history may add per commit between the first and last
    /// sample; growth faster than this is a leak proportional to the history.
    pub store_bytes_per_commit: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrowthLedger {
    pub mode: GrowthMode,
    pub samples: Vec<ResourceSample>,
    pub restores_refused: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrowthRefused {
    RestoreUnderNeverRestored,
    NoSamples,
    /// A leak verdict needs a baseline and a final sample.
    NoBaseline,
    StepNotMonotonic {
        step: u32,
    },
    CommitSeqNotMonotonic {
        step: u32,
    },
    R24NotMonotonic {
        step: u32,
    },
    /// Commit sequences come from an append-only log and are never negative;
    /// a negative baseline would buy growth allowance for commits that never
    /// happened.
    CommitSeqNegative {
        step: u32,
    },
    StoreMissing {
        step: u32,
        family: StoreFamily,
    },
    Leak {
        resource: String,
        step: u32,
        observed: u64,
    },
    HeadroomMismatch {
        step: u32,
        expected: u64,
        observed: u64,
    },
    /// The quota constants times the jobs counted do not fit in `u64`.
    HeadroomOverflow {
        step: u32,
    },
    /// The bytes the jobs counted should hold exceed the project quota, which
    /// admission never lets happen.
    HeadroomOverQuota {
        step: u32,
        expected: u64,
        quota: u64,
    },
    RemainingMismatch {
        step: u32,
        expected: u64,
        observed: u64,
    },
    BoundExceeded {
        resource: String,
        step: u32,
        bound: u64,
        observed: u64,
    },
    GrowthRateExceeded {
        bytes_per_commit: u64,
        observed_bytes: u64,
        commits: u64,
    },
    NotALeakVerdict {
        mode: GrowthMode,
    },
}

impl GrowthLedger {
    pub fn new(mode: GrowthMode) -> Self {
        Self {
            mode,
            samples: Vec::new(),
            restores_refused: 0,
        }
    }

    pub fn record(&mut self, sample: ResourceSample) -> Result<(), GrowthRefused> {
        if let Some(last) = self.samples.last() {
            in_order(last, &sample)?;
        }
        self.samples.push(sample);
        Ok(())
    }

    /// The ordering `record` enforces, re-checked for a ledger that was
    /// deserialized or assembled through the public `samples` field.
    pub fn check_order(&self) -> Result<(), GrowthRefused> {
        self.samples
            .windows(2)
            .try_for_each(|pair| in_order(&pair[0], &pair[1]))
    }

    /// The order, and every sample carrying every store family and a headroom
    /// that follows from the constants read. Evidence both modes carry; only
    /// `verdict` judges leaks.
    pub fn check_samples(&self, quota: &ReviewerQuota) -> Result<(), GrowthRefused> {
        if self.samples.is_empty() {
            return Err(GrowthRefused::NoSamples);
        }
        self.check_order()?;
        for sample in &self.samples {
            if sample.commit_seq < 0 {
                return Err(GrowthRefused::CommitSeqNegative { step: sample.step });
            }
            if let Some(family) = StoreFamily::ALL
                .into_iter()
                .find(|f| !sample.stores.contains_key(f))
            {
                return Err(GrowthRefused::StoreMissing {
                    step: sample.step,
                    family,
                });
            }
            let expected = quota
                .expected_project_bytes(&sample.headroom)
                .ok_or(GrowthRefused::HeadroomOverflow { step: sample.step })?;
            if sample.headroom.project_metadata_bytes != expected {
                return Err(GrowthRefused::HeadroomMismatch {
                    step: sample.step,
                    expected,
                    observed: sample.headroom.project_metadata_bytes,
                });
            }
            let remaining = quota.project_metadata_bytes.checked_sub(expected).ok_or(
                GrowthRefused::HeadroomOverQuota {
                    step: sample.step,
                    expected,
                    quota: quota.project_metadata_bytes,
                },
            )?;
            if sample.headroom.project_metadata_remaining != remaining {
                return Err(GrowthRefused::RemainingMismatch {
                    step: sample.step,
                    expected: remaining,
                    observed: sample.headroom.project_metadata_remaining,
                });
            }
        }
        Ok(())
    }

    /// A restore attempted under `never_restored` is refused and counted;
    /// under `restoring` it is permitted and the ledger stays out of the leak verdict.
    pub fn restore_attempted(&mut self) -> Result<(), GrowthRefused> {
        match self.mode {
            GrowthMode::NeverRestored => {
                self.restores_refused += 1;
                Err(GrowthRefused::RestoreUnderNeverRestored)
            }
            GrowthMode::Restoring => Ok(()),
        }
    }

    /// Every sample's headroom matches the constants, and the final sample
    /// shows no temporary object, no WAL bytes, and every counter within its
    /// bound. Only a never-restored ledger can say anything about leaks.
    pub fn verdict(
        &self,
        quota: &ReviewerQuota,
        bounds: &GrowthBounds,
    ) -> Result<(), GrowthRefused> {
        if self.mode != GrowthMode::NeverRestored {
            return Err(GrowthRefused::NotALeakVerdict { mode: self.mode });
        }
        let last = self.samples.last().ok_or(GrowthRefused::NoSamples)?;
        if self.samples.len() < 2 {
            return Err(GrowthRefused::NoBaseline);
        }
        self.check_samples(quota)?;
        if last.artifact_tmp_entries != 0 {
            return Err(GrowthRefused::Leak {
                resource: "artifact_tmp_entries".to_string(),
                step: last.step,
                observed: last.artifact_tmp_entries,
            });
        }
        let wal = last.wal_bytes();
        if wal != 0 {
            return Err(GrowthRefused::Leak {
                resource: "wal_bytes_after_truncate".to_string(),
                step: last.step,
                observed: wal,
            });
        }
        for (resource, observed) in [
            ("temp_roots", last.temp_roots),
            ("processes", last.processes),
        ] {
            if observed != 0 {
                return Err(GrowthRefused::Leak {
                    resource: resource.to_string(),
                    step: last.step,
                    observed,
                });
            }
        }
        let first = &self.samples[0];
        let commits = u64::try_from(last.commit_seq - first.commit_seq).unwrap_or(0);
        let grown = last
            .durable_store_bytes()
            .saturating_sub(first.durable_store_bytes());
        if grown > bounds.store_bytes_per_commit.saturating_mul(commits) {
            return Err(GrowthRefused::GrowthRateExceeded {
                bytes_per_commit: bounds.store_bytes_per_commit,
                observed_bytes: grown,
                commits,
            });
        }
        let checks = [
            ("store_bytes", last.store_total(), bounds.store_bytes),
            (
                "artifact_objects",
                last.artifact_objects,
                bounds.artifact_objects,
            ),
            ("artifact_bytes", last.artifact_bytes, bounds.artifact_bytes),
            (
                "commit_log_rows",
                last.commit_log_rows,
                bounds.commit_log_rows,
            ),
            (
                "projection_rows",
                last.projection_rows,
                bounds.projection_rows,
            ),
            ("open_holds", last.open_holds, bounds.open_holds),
        ];
        for (resource, observed, bound) in checks {
            if observed > bound {
                return Err(GrowthRefused::BoundExceeded {
                    resource: resource.to_string(),
                    step: last.step,
                    bound,
                    observed,
                });
            }
        }
        Ok(())
    }

    /// The largest store total any sample saw: transient pressure, not the
    /// final size.
    pub fn peak_store_bytes(&self) -> u64 {
        self.samples
            .iter()
            .map(ResourceSample::store_total)
            .max()
            .unwrap_or(0)
    }
}

fn in_order(prev: &ResourceSample, next: &ResourceSample) -> Result<(), GrowthRefused> {
    if next.step <= prev.step {
        return Err(GrowthRefused::StepNotMonotonic { step: next.step });
    }
    if next.commit_seq < prev.commit_seq {
        return Err(GrowthRefused::CommitSeqNotMonotonic { step: next.step });
    }
    if next.headroom.r24_refusals < prev.headroom.r24_refusals {
        return Err(GrowthRefused::R24NotMonotonic { step: next.step });
    }
    Ok(())
}

/// The operation kinds a swarm mix must exercise at least once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Publish,
    Correct,
    Retire,
    Query,
    FaultEpisode,
    QuotaPressure,
    StoreGrowth,
}

impl Operation {
    pub const ALL: [Operation; 7] = [
        Operation::Publish,
        Operation::Correct,
        Operation::Retire,
        Operation::Query,
        Operation::FaultEpisode,
        Operation::QuotaPressure,
        Operation::StoreGrowth,
    ];
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SwarmMix {
    pub counts: BTreeMap<Operation, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MixIncomplete {
    pub missing: BTreeSet<Operation>,
}

impl SwarmMix {
    pub fn record(&mut self, operation: Operation) {
        *self.counts.entry(operation).or_insert(0) += 1;
    }

    pub fn complete(&self) -> Result<(), MixIncomplete> {
        let missing: BTreeSet<Operation> = Operation::ALL
            .into_iter()
            .filter(|op| !self.exercised(*op))
            .collect();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(MixIncomplete { missing })
        }
    }

    pub fn exercised(&self, operation: Operation) -> bool {
        self.counts.get(&operation).is_some_and(|n| *n > 0)
    }
}

/// What two campaigns on one checkout must not share.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CampaignResources {
    pub roots: BTreeSet<String>,
    pub publish_dirs: BTreeSet<String>,
    pub cassette_namespaces: BTreeSet<String>,
    pub ports: BTreeSet<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IsolationRefused {
    SharedRoot {
        path: String,
    },
    SharedPublishDir {
        path: String,
    },
    SharedCassetteNamespace {
        namespace: String,
    },
    SharedPort {
        port: u16,
    },
    /// Isolation is a claim about at least two campaigns.
    TooFewCampaigns {
        campaigns: usize,
    },
    DigestDiffersFromSerial {
        campaign: usize,
    },
}

/// Two campaigns are isolated when they share none of these, and their
/// concurrent result digests equal their serial ones. A root and a publish
/// directory are the same filesystem resource, so they are compared across
/// the two kinds, and a path inside another campaign's path writes into it,
/// so ancestors count as shared. Paths are compared as given, not
/// canonicalized: the caller names the directories it created.
pub fn isolated(a: &CampaignResources, b: &CampaignResources) -> Result<(), IsolationRefused> {
    let overlaps = |p: &String| {
        b.roots
            .iter()
            .chain(&b.publish_dirs)
            .any(|q| under(p, q) || under(q, p))
    };
    if let Some(path) = a.roots.iter().find(|p| overlaps(p)) {
        return Err(IsolationRefused::SharedRoot { path: path.clone() });
    }
    if let Some(path) = a.publish_dirs.iter().find(|p| overlaps(p)) {
        return Err(IsolationRefused::SharedPublishDir { path: path.clone() });
    }
    if let Some(namespace) = a
        .cassette_namespaces
        .intersection(&b.cassette_namespaces)
        .next()
    {
        return Err(IsolationRefused::SharedCassetteNamespace {
            namespace: namespace.clone(),
        });
    }
    if let Some(port) = a.ports.intersection(&b.ports).next() {
        return Err(IsolationRefused::SharedPort { port: *port });
    }
    Ok(())
}

/// `path` is `dir` or lies inside it, by `/`-separated components; trailing
/// separators do not count, so `/` (trimmed to nothing) contains every
/// absolute path.
fn under(path: &str, dir: &str) -> bool {
    let (path, dir) = (path.trim_end_matches('/'), dir.trim_end_matches('/'));
    path == dir
        || path
            .strip_prefix(dir)
            .is_some_and(|rest| rest.starts_with('/'))
}

pub fn digests_match_serial(
    concurrent: &[String],
    serial: &[String],
) -> Result<(), IsolationRefused> {
    if concurrent.len() != serial.len() {
        return Err(IsolationRefused::DigestDiffersFromSerial {
            campaign: concurrent.len().min(serial.len()),
        });
    }
    if concurrent.len() < 2 {
        return Err(IsolationRefused::TooFewCampaigns {
            campaigns: concurrent.len(),
        });
    }
    for (i, (c, s)) in concurrent.iter().zip(serial).enumerate() {
        if c != s {
            return Err(IsolationRefused::DigestDiffersFromSerial { campaign: i });
        }
    }
    Ok(())
}

/// What the caller knows independently of the report and holds it to: the
/// quota constants it read from the store, the bounds the manifest declares,
/// and the approved profile's envelope. The report's embedded copies must
/// equal these, so a producer cannot widen what it is judged by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrowthContract {
    pub quota: ReviewerQuota,
    pub bounds: GrowthBounds,
    pub envelope: crate::ResourceLimits,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrowthReport {
    pub schema: String,
    pub eval_run_id: String,
    pub profile_digest: String,
    pub claim_boundary: crate::ClaimBoundary,
    pub quota: ReviewerQuota,
    pub bounds: GrowthBounds,
    pub ledger: GrowthLedger,
    pub mix: SwarmMix,
    pub expected_refusals: Vec<RecordedRefusal>,
    pub fault_episodes: u64,
    pub safety_checks_while_armed: u64,
    pub markers: BTreeSet<String>,
    pub envelope: crate::Envelope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrowthReportError {
    SchemaMismatch {
        found: String,
    },
    /// `eval_run_id` or `profile_digest` is not 64 lowercase hex digits.
    MalformedDigest {
        field: &'static str,
    },
    Growth(GrowthRefused),
    Mix(MixIncomplete),
    SafetyNeverChecked,
    EnvelopeNotHonoured(crate::EnvelopeExceeded),
    /// A sample read more of a resource than the envelope peak admits.
    EnvelopeNotCharged {
        resource: crate::Resource,
        step: u32,
        peak: u64,
        observed: u64,
    },
    /// The embedded quota constants are not the ones the caller read from
    /// the store.
    QuotaMismatch,
    /// The embedded bounds are not the approved ones the caller passed.
    BoundsNotApproved,
    /// The embedded envelope bounds are not the approved profile's limits.
    EnvelopeBoundsNotApproved,
    /// The claim boundary is not the repository's pinned one.
    ClaimBoundaryMismatch,
    /// The final sample's R24 count and the recorded R24 refusals disagree.
    R24Unreconciled {
        counted: u64,
        recorded: u64,
    },
    /// A recorded refusal's production error does not name the variant the
    /// refusal claims.
    RefusalNotEvidenced {
        episode: String,
        refusal: ExpectedRefusal,
    },
    /// A marker no registered suite owns.
    UnregisteredMarker {
        marker: String,
    },
    /// An integer outside the canonical safe range; `result_digest` would
    /// refuse the value `validate` accepted.
    NotCanonical(ContractError),
    Shape(String),
    Lossy,
}

impl GrowthReport {
    pub fn validate(&self, contract: &GrowthContract) -> Result<(), GrowthReportError> {
        if self.schema != GROWTH_REPORT_SCHEMA {
            return Err(GrowthReportError::SchemaMismatch {
                found: self.schema.clone(),
            });
        }
        for (field, digest) in [
            ("eval_run_id", &self.eval_run_id),
            ("profile_digest", &self.profile_digest),
        ] {
            if !is_lower_hex(digest, 64) {
                return Err(GrowthReportError::MalformedDigest { field });
            }
        }
        if self.claim_boundary != crate::ClaimBoundary::pinned() {
            return Err(GrowthReportError::ClaimBoundaryMismatch);
        }
        if self.quota != contract.quota {
            return Err(GrowthReportError::QuotaMismatch);
        }
        if self.bounds != contract.bounds {
            return Err(GrowthReportError::BoundsNotApproved);
        }
        if self.envelope.bounds != contract.envelope {
            return Err(GrowthReportError::EnvelopeBoundsNotApproved);
        }
        self.mix.complete().map_err(GrowthReportError::Mix)?;
        let faulted = self.fault_episodes > 0 || self.mix.exercised(Operation::FaultEpisode);
        if faulted && self.safety_checks_while_armed == 0 {
            return Err(GrowthReportError::SafetyNeverChecked);
        }
        if let Some(marker) = self
            .markers
            .iter()
            .find(|marker| !crate::MARKERS.iter().any(|m| m.name == marker.as_str()))
        {
            return Err(GrowthReportError::UnregisteredMarker {
                marker: marker.clone(),
            });
        }
        self.envelope
            .check()
            .map_err(GrowthReportError::EnvelopeNotHonoured)?;
        for sample in &self.ledger.samples {
            for (resource, observed) in [
                (crate::Resource::StoreBytes, sample.store_total()),
                (crate::Resource::CassetteBytes, sample.cassette_bytes),
                (crate::Resource::TempRoots, sample.temp_roots),
                (crate::Resource::Processes, sample.processes),
            ] {
                let peak = resource.of(&self.envelope.peaks);
                if observed > peak {
                    return Err(GrowthReportError::EnvelopeNotCharged {
                        resource,
                        step: sample.step,
                        peak,
                        observed,
                    });
                }
            }
        }
        for recorded in &self.expected_refusals {
            if !recorded
                .production_error
                .contains(recorded.refusal.production_variant())
            {
                return Err(GrowthReportError::RefusalNotEvidenced {
                    episode: recorded.episode.clone(),
                    refusal: recorded.refusal,
                });
            }
        }
        let recorded = self
            .expected_refusals
            .iter()
            .filter(|r| r.refusal == ExpectedRefusal::R24ReceiptQuotaExhausted)
            .count() as u64;
        let counted = self
            .ledger
            .samples
            .last()
            .map_or(0, |s| s.headroom.r24_refusals);
        if counted != recorded {
            return Err(GrowthReportError::R24Unreconciled { counted, recorded });
        }
        match self.ledger.mode {
            GrowthMode::NeverRestored => self
                .ledger
                .verdict(&self.quota, &self.bounds)
                .map_err(GrowthReportError::Growth)?,
            GrowthMode::Restoring => self
                .ledger
                .check_samples(&self.quota)
                .map_err(GrowthReportError::Growth)?,
        }
        Ok(())
    }

    /// Digestible on both runtimes: no integer may leave the canonical safe
    /// range, or `result_digest` would refuse the value `validate` accepted.
    pub fn serialize(&self, contract: &GrowthContract) -> Result<Value, GrowthReportError> {
        self.validate(contract)?;
        let value =
            serde_json::to_value(self).map_err(|e| GrowthReportError::Shape(e.to_string()))?;
        canonical_json_encode(&value).map_err(GrowthReportError::NotCanonical)?;
        Ok(value)
    }

    /// The digest excludes per-sample byte measurements and envelope peaks.
    /// It retains steps, commit sequence, row and object counts, and headroom,
    /// so commits observed from another campaign change the digest.
    pub fn result_digest(report: &Value) -> Result<String, GrowthReportError> {
        let mut value = report.clone();
        let object = value
            .as_object_mut()
            .ok_or_else(|| GrowthReportError::Shape("report is not an object".to_string()))?;
        if let Some(samples) = object
            .get_mut("ledger")
            .and_then(|ledger| ledger.get_mut("samples"))
            .and_then(Value::as_array_mut)
        {
            for sample in samples.iter_mut().filter_map(Value::as_object_mut) {
                for field in SAMPLE_BYTE_FIELDS {
                    sample.remove(field);
                }
            }
        }
        if let Some(envelope) = object.get_mut("envelope").and_then(Value::as_object_mut) {
            envelope.remove("peaks");
        }
        protocol_digest(GROWTH_RESULT_DIGEST_PROTOCOL, &value)
            .map_err(|e| GrowthReportError::Shape(e.to_string()))
    }
}

pub fn parse_growth_report(
    value: &Value,
    contract: &GrowthContract,
) -> Result<GrowthReport, GrowthReportError> {
    let report =
        GrowthReport::deserialize(value).map_err(|e| GrowthReportError::Shape(e.to_string()))?;
    report.validate(contract)?;
    canonical_json_encode(value).map_err(GrowthReportError::NotCanonical)?;
    let again =
        serde_json::to_value(&report).map_err(|e| GrowthReportError::Shape(e.to_string()))?;
    if again != *value {
        return Err(GrowthReportError::Lossy);
    }
    Ok(report)
}

debug_display!(
    GrowthRefused,
    MixIncomplete,
    IsolationRefused,
    GrowthReportError
);
