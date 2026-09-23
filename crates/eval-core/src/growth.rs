//! Sustainability under churn: resource samples at every quiescence of a
//! campaign that never restores, reviewer-quota headroom accounted from the
//! store's own constants, the swarm mix a growth campaign must exercise, and
//! the isolation two concurrent campaigns must keep.

use std::collections::{BTreeMap, BTreeSet};

use context_core::canonical_json::protocol_digest;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::checkpoint::StoreFamily;
use crate::fault::RecordedRefusal;

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
    /// per job still open, plus whatever frozen pages are charged.
    pub fn expected_project_bytes(&self, headroom: &HeadroomSample) -> u64 {
        self.receipt_charge_bytes * headroom.terminal_jobs
            + (self.receipt_charge_bytes + self.job_allowance_bytes) * headroom.pending_jobs
            + headroom.page_bytes
    }

    /// How many more admissions the remaining bytes allow, as a report figure
    /// derived from the constants read; not an acceptance count.
    pub fn admissions_remaining(&self, remaining_bytes: u64) -> u64 {
        remaining_bytes
            .checked_div(self.receipt_charge_bytes + self.job_allowance_bytes)
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
    pub fn store_total(&self) -> u64 {
        self.stores.values().map(|b| b.file + b.wal + b.shm).sum()
    }

    /// Main database-file bytes, excluding the `-wal` and `-shm` sidecars.
    pub fn durable_store_bytes(&self) -> u64 {
        self.stores.values().map(|b| b.file).sum()
    }
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
    StepNotMonotonic {
        step: u32,
    },
    CommitSeqNotMonotonic {
        step: u32,
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
        self.check_order()?;
        for sample in &self.samples {
            let expected = quota.expected_project_bytes(&sample.headroom);
            if sample.headroom.project_metadata_bytes != expected {
                return Err(GrowthRefused::HeadroomMismatch {
                    step: sample.step,
                    expected,
                    observed: sample.headroom.project_metadata_bytes,
                });
            }
        }
        if last.artifact_tmp_entries != 0 {
            return Err(GrowthRefused::Leak {
                resource: "artifact_tmp_entries".to_string(),
                step: last.step,
                observed: last.artifact_tmp_entries,
            });
        }
        let wal: u64 = last.stores.values().map(|b| b.wal).sum();
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
        if grown > bounds.store_bytes_per_commit.saturating_mul(commits.max(1)) {
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
    SharedRoot { path: String },
    SharedPublishDir { path: String },
    SharedCassetteNamespace { namespace: String },
    SharedPort { port: u16 },
    DigestDiffersFromSerial { campaign: usize },
}

/// Two campaigns are isolated when they share none of these, and their
/// concurrent result digests equal their serial ones.
pub fn isolated(a: &CampaignResources, b: &CampaignResources) -> Result<(), IsolationRefused> {
    if let Some(path) = a.roots.intersection(&b.roots).next() {
        return Err(IsolationRefused::SharedRoot { path: path.clone() });
    }
    if let Some(path) = a.publish_dirs.intersection(&b.publish_dirs).next() {
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

pub fn digests_match_serial(
    concurrent: &[String],
    serial: &[String],
) -> Result<(), IsolationRefused> {
    if concurrent.len() != serial.len() {
        return Err(IsolationRefused::DigestDiffersFromSerial {
            campaign: concurrent.len().min(serial.len()),
        });
    }
    for (i, (c, s)) in concurrent.iter().zip(serial).enumerate() {
        if c != s {
            return Err(IsolationRefused::DigestDiffersFromSerial { campaign: i });
        }
    }
    Ok(())
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
    SchemaMismatch { found: String },
    Growth(GrowthRefused),
    Mix(MixIncomplete),
    SafetyNeverChecked,
    Shape(String),
    Lossy,
}

impl GrowthReport {
    pub fn validate(&self) -> Result<(), GrowthReportError> {
        if self.schema != GROWTH_REPORT_SCHEMA {
            return Err(GrowthReportError::SchemaMismatch {
                found: self.schema.clone(),
            });
        }
        self.mix.complete().map_err(GrowthReportError::Mix)?;
        let faulted = self.fault_episodes > 0 || self.mix.exercised(Operation::FaultEpisode);
        if faulted && self.safety_checks_while_armed == 0 {
            return Err(GrowthReportError::SafetyNeverChecked);
        }
        match self.ledger.mode {
            GrowthMode::NeverRestored => self
                .ledger
                .verdict(&self.quota, &self.bounds)
                .map_err(GrowthReportError::Growth)?,
            GrowthMode::Restoring => {
                if self.ledger.samples.is_empty() {
                    return Err(GrowthReportError::Growth(GrowthRefused::NoSamples));
                }
                self.ledger
                    .check_order()
                    .map_err(GrowthReportError::Growth)?;
            }
        }
        Ok(())
    }

    pub fn serialize(&self) -> Result<Value, GrowthReportError> {
        self.validate()?;
        serde_json::to_value(self).map_err(|e| GrowthReportError::Shape(e.to_string()))
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

pub fn parse_growth_report(value: &Value) -> Result<GrowthReport, GrowthReportError> {
    let report =
        GrowthReport::deserialize(value).map_err(|e| GrowthReportError::Shape(e.to_string()))?;
    report.validate()?;
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
