//! Suite C growth campaign: sustainability under churn on one root that is
//! never restored, judged by `eval_core::growth`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use eval_core::{
    Approval, CampaignResources, ClaimBoundary, Coverage, Cut, CutOutcome, CutReceipt,
    EnvelopeExceeded, ExecutionMode, GROWTH_REPORT_SCHEMA, GrowthBounds, GrowthLedger, GrowthMode,
    GrowthRefused, GrowthReport, GrowthReportError, HeadroomSample, Operation, ProfileError,
    ResourceSample, ReviewerQuota, RunProfile, Scale, SearchEpisodeFault, StoreBytes, StoreFamily,
    SwarmMix, eval_run_id,
};
use memory_store::memory_reviewer_jobs::{
    MAX_MEMORY_REVIEWER_METADATA_BYTES_PER_HOST, MAX_MEMORY_REVIEWER_METADATA_BYTES_PER_PROJECT,
    MEMORY_REVIEWER_JOB_ALLOWANCE_BYTES, MEMORY_REVIEWER_RECEIPT_CHARGE_BYTES,
};
use serde_json::json;

use super::aging::{
    self, Applied, ManifestInputs, Plan, Stores, kernel_file, memory_file, read_only, search_file,
    store_file, suite_c_manifest,
};
use super::campaign::{Charges, identity, prepare_publish, publish_file};
use super::fault::{self, Witness, lost_reply_episode};
use super::support::memory_reviewer_publish::{
    self as reviewer, abstain, activate_module_authority, begin_job, commit_memory_domain,
};

pub const REPORT_FILE: &str = "suite-c-growth-report.json";
pub const MANIFEST_FILE: &str = "manifest.json";
pub const SIMULATOR_VERSION: &str = "eval-growth-shell/v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub scale: Scale,
    pub messages: u32,
    pub elapsed_bound_ms: u64,
    pub approval: Option<Approval>,
    pub publish: PathBuf,
    pub mode: GrowthMode,
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("profile refused: {0}")]
    Profile(#[from] ProfileError),
    #[error("envelope exceeded: {0:?}")]
    Envelope(#[from] EnvelopeExceeded),
    #[error("plan refused: {0}")]
    Plan(#[from] aging::RunError),
    #[error("fault episode refused: {0}")]
    Fault(#[from] fault::RunError),
    #[error("growth refused: {0}")]
    Growth(#[from] GrowthRefused),
    #[error("report refused: {0}")]
    Report(#[from] GrowthReportError),
    #[error("publish {}: {kind}", path.display())]
    Publish {
        path: PathBuf,
        kind: std::io::ErrorKind,
    },
}

fn publish_refused((path, kind): (PathBuf, std::io::ErrorKind)) -> RunError {
    RunError::Publish { path, kind }
}

pub fn profile(
    scale: Scale,
    steps: u32,
    elapsed_ms: u64,
    approval: Option<Approval>,
) -> RunProfile {
    let mut profile = aging::profile(scale, steps, elapsed_ms, approval);
    profile.name = profile.name.replace("suite-c-aging", "suite-c-growth");
    profile
}

/// The constants as the memory store declares them.
pub fn quota() -> ReviewerQuota {
    ReviewerQuota {
        receipt_charge_bytes: MEMORY_REVIEWER_RECEIPT_CHARGE_BYTES,
        job_allowance_bytes: MEMORY_REVIEWER_JOB_ALLOWANCE_BYTES,
        project_metadata_bytes: MAX_MEMORY_REVIEWER_METADATA_BYTES_PER_PROJECT,
        host_metadata_bytes: MAX_MEMORY_REVIEWER_METADATA_BYTES_PER_HOST,
    }
}

/// Bounds for the final sample, scaled from the history: the whole history
/// fits in the envelope's store bytes, and rows grow with the messages.
pub fn bounds(profile: &RunProfile, messages: u32) -> GrowthBounds {
    GrowthBounds {
        store_bytes: profile.envelope.store_bytes,
        artifact_objects: 64 + 8 * u64::from(messages),
        commit_log_rows: 64 + 16 * u64::from(messages),
        projection_rows: 16 * u64::from(messages),
        open_holds: 1,
    }
}

pub struct Run {
    pub report: GrowthReport,
    pub report_bytes: Vec<u8>,
    pub manifest: eval_core::Manifest,
    pub manifest_bytes: Vec<u8>,
    pub resources: CampaignResources,
    pub coverage: Coverage,
}

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn count_entries(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|entries| entries.filter_map(Result::ok).count() as u64)
        .unwrap_or(0)
}

fn walk_objects(dir: &Path) -> (u64, u64) {
    let mut count = 0;
    let mut bytes = 0;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                let (c, b) = walk_objects(&path);
                count += c;
                bytes += b;
            } else if path.is_file() {
                count += 1;
                bytes += file_len(&path);
            }
        }
    }
    (count, bytes)
}

fn count(path: &Path, sql: &str) -> u64 {
    read_only(path)
        .query_row(sql, [], |row| row.get::<_, i64>(0))
        .map(|n| u64::try_from(n).unwrap())
        .unwrap()
}

/// The growth campaign's mutable state: one root, never restored unless the
/// mode says otherwise, sampled at every quiescence.
pub struct Campaign {
    pub plan: Plan,
    pub stores: Stores,
    pub ledger: GrowthLedger,
    pub mix: SwarmMix,
    pub witness: Witness,
    pub admitted_total: u64,
    pub r24_refusals: u64,
    pub generation: u64,
    pub next_step: usize,
    pub published_bytes: u64,
}

impl Campaign {
    pub fn open(root: &Path, plan: Plan, mode: GrowthMode) -> Self {
        let stores = Stores::open(root, plan.rendering.clone());
        let generation = activate_module_authority(&stores.memory, root);
        commit_memory_domain(&stores.corpus.kernel);
        Self {
            plan,
            stores,
            ledger: GrowthLedger::new(mode),
            mix: SwarmMix::default(),
            witness: Witness::new(),
            admitted_total: 0,
            r24_refusals: 0,
            generation,
            next_step: 0,
            published_bytes: 0,
        }
    }

    pub fn root(&self) -> &Path {
        self.stores.root()
    }

    pub fn remaining(&self) -> usize {
        self.plan.steps.len() - self.next_step
    }

    /// One step of the swarm mix: the planned mutation, then a query every
    /// third step, a fault episode every fifth, quota pressure every fourth,
    /// then a drain to quiescence.
    pub fn step(&mut self, charges: &mut Charges) -> Result<(), RunError> {
        let i = self.next_step;
        let planned = self.plan.steps[i].clone();
        let now = planned.now_ms;
        match self.stores.apply(&planned) {
            Applied::Published => self.mix.record(Operation::Publish),
            Applied::Corrected => self.mix.record(Operation::Correct),
            Applied::Retired => self.mix.record(Operation::Retire),
            Applied::RetireSkipped => {}
        }
        self.mix.record(Operation::StoreGrowth);
        if i % 3 == 2 {
            let rows = self.stores.projection_rows();
            assert!(rows.snapshot_commit_seq >= 0);
            self.mix.record(Operation::Query);
        }
        if i % 5 == 4 {
            lost_reply_episode(
                &mut self.stores,
                &mut self.witness,
                &format!("growth-ack-lost-{i}"),
                i as u32,
                now,
                SearchEpisodeFault::LoseAcknowledgementReply,
            )?;
            self.mix.record(Operation::FaultEpisode);
        }
        if i % 4 == 3 {
            // The reviewer queue's deadlines are wall-clock by design, so its
            // admissions are stamped with the wall clock, not the drive's logical time.
            self.quota_pressure(i, reviewer::now_ms());
        }
        self.stores.drain(now);
        self.next_step += 1;
        self.sample(i as u32, charges)
    }

    /// One reviewer job admitted through the real reservation, staging,
    /// claim, and receipt path; every other one is settled by abstention so
    /// the ledger holds both pending and terminal charges.
    fn quota_pressure(&mut self, i: usize, now: i64) {
        let incarnation = reviewer::kernel_incarnation(&self.stores.corpus.kernel);
        let digest = format!("{:064x}", i);
        let begun = begin_job(
            &self.stores.corpus.kernel,
            &self.stores.memory,
            &digest,
            &incarnation,
            self.generation,
            i as u64,
            now,
        );
        self.admitted_total += 1;
        if i % 8 == 3 {
            abstain(&self.stores.memory, &incarnation, &begun, now);
        }
        self.mix.record(Operation::QuotaPressure);
    }

    fn headroom(&self) -> HeadroomSample {
        let headroom = self
            .stores
            .memory
            .memory_reviewer_headroom(reviewer::PROJECT)
            .unwrap();
        let terminal = count(
            &memory_file(self.root()),
            "SELECT COUNT(*) FROM memory_reviewer_jobs WHERE state='terminal'",
        );
        HeadroomSample {
            pending_jobs: headroom.pending_jobs as u64,
            terminal_jobs: terminal,
            page_bytes: 0,
            project_metadata_bytes: headroom.project_metadata_bytes,
            project_metadata_remaining: headroom.project_metadata_remaining,
            admitted_total: self.admitted_total,
            r24_refusals: self.r24_refusals,
        }
    }

    fn sample_at(&self, step: u32, charges: &Charges) -> ResourceSample {
        sample_root(
            self.root(),
            step,
            self.stores.tip(),
            self.published_bytes,
            charges,
            self.headroom(),
        )
    }
}

fn store_bytes(root: &Path) -> BTreeMap<StoreFamily, StoreBytes> {
    StoreFamily::ALL
        .into_iter()
        .map(|family| {
            let file = store_file(root, family);
            (
                family,
                StoreBytes {
                    file: file_len(&file),
                    wal: file_len(&sidecar(&file, "-wal")),
                    shm: file_len(&sidecar(&file, "-shm")),
                },
            )
        })
        .collect()
}

/// Everything the root holds at one quiescent point, read from the files.
fn sample_root(
    root: &Path,
    step: u32,
    commit_seq: i64,
    published_bytes: u64,
    charges: &Charges,
    headroom: HeadroomSample,
) -> ResourceSample {
    let artifacts = root.join("kernel").join("artifacts");
    let (artifact_objects, artifact_bytes) = walk_objects(&artifacts.join("objects"));
    ResourceSample {
        step,
        commit_seq,
        stores: store_bytes(root),
        artifact_objects,
        artifact_tmp_entries: count_entries(&artifacts.join("tmp")),
        artifact_bytes,
        cassette_bytes: 0,
        published_bytes,
        // The campaign's own root is occupied for the whole run; a leak is any root beyond it.
        temp_roots: charges.roots().saturating_sub(1),
        processes: charges.processes(),
        commit_log_rows: count(&kernel_file(root), "SELECT COUNT(*) FROM commit_log"),
        projection_rows: count(&search_file(root), "SELECT COUNT(*) FROM occurrences"),
        open_holds: count(
            &kernel_file(root),
            "SELECT COUNT(*) FROM capture_pins WHERE released_at IS NULL",
        ),
        headroom,
    }
}

/// The headroom of a closed memory store, from its rows.
fn closed_headroom(root: &Path, admitted_total: u64, r24_refusals: u64) -> HeadroomSample {
    let memory = memory_file(root);
    let project_metadata_bytes = count(
        &memory,
        "SELECT COALESCE(SUM(receipt_charge_bytes + CASE WHEN state<>'terminal' THEN allowance_bytes ELSE 0 END),0) FROM memory_reviewer_jobs",
    );
    HeadroomSample {
        pending_jobs: count(
            &memory,
            "SELECT COUNT(*) FROM memory_reviewer_jobs WHERE state<>'terminal'",
        ),
        terminal_jobs: count(
            &memory,
            "SELECT COUNT(*) FROM memory_reviewer_jobs WHERE state='terminal'",
        ),
        page_bytes: 0,
        project_metadata_bytes,
        project_metadata_remaining: quota().project_metadata_bytes - project_metadata_bytes,
        admitted_total,
        r24_refusals,
    }
}

impl Campaign {
    /// Records the sample and charges the envelope with the transient store
    /// pressure it shows.
    pub fn sample(&mut self, step: u32, charges: &mut Charges) -> Result<(), RunError> {
        let sample = self.sample_at(step, charges);
        charges.observe(eval_core::Resource::StoreBytes, sample.store_total())?;
        charges.elapsed()?;
        self.ledger.record(sample)?;
        Ok(())
    }

    /// A restore request: refused under `never_restored` and counted, with
    /// the campaign handed back untouched; otherwise a close and reopen in
    /// place with the projection rebuilt.
    pub fn restore(mut self, now: i64) -> (Self, Result<(), RunError>) {
        if let Err(refused) = self.ledger.restore_attempted() {
            return (self, Err(refused.into()));
        }
        let Campaign {
            plan,
            stores,
            ledger,
            mix,
            witness,
            admitted_total,
            r24_refusals,
            generation,
            next_step,
            published_bytes,
        } = self;
        let tip = stores.tip();
        let stores = stores.close().reopen(now);
        assert_eq!(stores.tip(), tip, "a reopen moves no commit");
        (
            Campaign {
                plan,
                stores,
                ledger,
                mix,
                witness,
                admitted_total,
                r24_refusals,
                generation,
                next_step,
                published_bytes,
            },
            Ok(()),
        )
    }

    /// Closes the stores, which truncates every WAL, and takes the final
    /// sample from the closed files: quiescent, with nothing transient left.
    pub fn finish(
        self,
        charges: &mut Charges,
    ) -> Result<(GrowthLedger, SwarmMix, Witness), RunError> {
        let Campaign {
            plan,
            stores,
            mut ledger,
            mix,
            witness,
            admitted_total,
            r24_refusals,
            published_bytes,
            ..
        } = self;
        let step = ledger.samples.last().map_or(0, |s| s.step + 1);
        let root = stores.root().to_path_buf();
        let _ = plan;
        let tip = stores.tip();
        drop(stores.close());
        let sample = sample_root(
            &root,
            step,
            tip,
            published_bytes,
            charges,
            closed_headroom(&root, admitted_total, r24_refusals),
        );
        charges.observe(eval_core::Resource::StoreBytes, sample.store_total())?;
        ledger.record(sample)?;
        Ok((ledger, mix, witness))
    }
}

pub fn run(config: &Config) -> Result<Run, RunError> {
    let started_at_ms = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    let plan = aging::plan(config.messages)?;
    let steps = plan.steps.len() as u32;
    let profile = profile(
        config.scale,
        steps,
        config.elapsed_bound_ms,
        config.approval.clone(),
    );
    profile.approved()?;
    prepare_publish(&config.publish, &[REPORT_FILE, MANIFEST_FILE]).map_err(publish_refused)?;
    let mut charges = Charges::new(profile.envelope.clone());
    let bounds = bounds(&profile, config.messages);
    let root = charges.occupy()?;
    let root_path = root.path().display().to_string();
    let mut campaign = Campaign::open(root.path(), plan, config.mode);
    while campaign.remaining() > 0 {
        campaign.step(&mut charges)?;
    }
    let (ledger, mix, witness) = campaign.finish(&mut charges)?;
    charges.vacate(root)?;
    let mut coverage = witness.coverage;
    coverage
        .record("flt_leak_ledger_sampled_before_reopen")
        .unwrap();
    coverage
        .record("flt_headroom_accounted_from_store_constants")
        .unwrap();
    mix.complete().map_err(GrowthReportError::Mix)?;
    coverage.record("flt_swarm_mix_complete").unwrap();
    let identity = identity(
        &profile,
        SIMULATOR_VERSION,
        aging::SEED,
        json!({
            "steps": steps,
            "messages": config.messages,
            "mode": config.mode,
        }),
        &std::env::current_exe().unwrap(),
    );
    let mut report = GrowthReport {
        schema: GROWTH_REPORT_SCHEMA.to_string(),
        eval_run_id: eval_run_id(&identity).unwrap(),
        profile_digest: profile.digest().unwrap(),
        claim_boundary: ClaimBoundary::pinned(),
        quota: quota(),
        bounds,
        ledger,
        mix,
        expected_refusals: witness.refusals.clone(),
        fault_episodes: witness.episodes.len() as u64,
        safety_checks_while_armed: witness.safety_checks,
        markers: coverage.fired().iter().map(|m| m.to_string()).collect(),
        envelope: charges.envelope.clone(),
    };
    charges.retain_publish_root()?;
    report.validate()?;
    let bytes = charges.publish_bytes(|envelope| {
        report.envelope = envelope.clone();
        serde_json::to_vec_pretty(&serde_json::to_value(&report).unwrap()).unwrap()
    })?;
    let published: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let manifest = suite_c_manifest(ManifestInputs {
        identity,
        eval_run_id: report.eval_run_id.clone(),
        sample: format!("growth:{steps}"),
        result_digest: GrowthReport::result_digest(&published)?,
        witness_digest: GrowthReport::result_digest(&published)?,
        cut_receipts: [Cut::AtQuiescence, Cut::EndOfRun]
            .into_iter()
            .map(|cut| CutReceipt {
                cut,
                outcome: CutOutcome::Reached,
            })
            .collect(),
        execution_mode: ExecutionMode::Generate,
        envelope: report.envelope.clone(),
        started_at_ms,
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest.to_value()).unwrap();
    publish_file(&config.publish.join(REPORT_FILE), &bytes).map_err(publish_refused)?;
    publish_file(&config.publish.join(MANIFEST_FILE), &manifest_bytes).map_err(publish_refused)?;
    Ok(Run {
        report,
        report_bytes: bytes,
        manifest,
        manifest_bytes,
        resources: CampaignResources {
            roots: [root_path].into_iter().collect(),
            publish_dirs: [config.publish.display().to_string()].into_iter().collect(),
            cassette_namespaces: BTreeSet::new(),
            ports: BTreeSet::new(),
        },
        coverage,
    })
}

pub const USAGE: &str = "growth --scale <s0|s1|s2> --messages <n> --elapsed-bound-ms <n> \
--approved-by <name> --approval-run-id <hex64> --publish <dir> --mode <never_restored|restoring>";

pub fn config_from_args(args: impl IntoIterator<Item = String>) -> Result<Config, String> {
    let mut args: Vec<String> = args.into_iter().collect();
    let mode = match args.iter().position(|a| a == "--mode") {
        Some(i) if i + 1 < args.len() => {
            let value = args.remove(i + 1);
            args.remove(i);
            serde_json::from_value::<GrowthMode>(serde_json::Value::String(value))
                .map_err(|e| format!("--mode: {e}"))?
        }
        _ => return Err(USAGE.to_string()),
    };
    let base = aging::config_from_args(args).map_err(|error| error.replace(aging::USAGE, USAGE))?;
    Ok(Config {
        scale: base.scale,
        messages: base.messages,
        elapsed_bound_ms: base.elapsed_bound_ms,
        approval: base.approval,
        publish: base.publish,
        mode,
    })
}
