//! `eval-qualification/v3`: the fixed retained-history and outage qualification campaign.
//! The case catalog, the required host, the numeric gates, the drain estimate, the outage and
//! virtual rotation schedules, and the per-publication lineage oracle are values; the runner
//! shell measures and this module judges.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::scale::{Histogram, HostManifest};

pub const QUALIFICATION_SCHEMA: &str = "eval-qualification/v3";
/// Fixture seed every generated history uses.
pub const QUALIFICATION_SEED: u64 = 702;
/// Measured lexical tokens in each ordinary message under the pinned tokenizer.
pub const MESSAGE_TOKENS: usize = 256;
/// Repetitions per case; every repetition is kept.
pub const REPETITIONS: u32 = 5;
/// Interactive operations per case, summed over its repetitions.
pub const MIN_INTERACTIVE_OPERATIONS: u64 = 10_000;
/// Messages in the fixed active window every case serves.
pub const ACTIVE_WINDOW: u64 = 300;
/// Older folded history kept behind the window in the windowed cases.
pub const FOLDED_MULTIPLE: u64 = 10;
/// Fake-model latency.
pub const FAKE_MODEL_LATENCY_MS: u64 = 100;
/// Source outage, recovery bound after cooldown, and the virtual auth soak.
pub const OUTAGE_SECONDS: u64 = 120;
pub const RECOVERY_BOUND_SECONDS: u64 = 360;
pub const SOAK_SECONDS: u64 = 24 * 3600;
pub const SOAK_ROTATIONS: u32 = 24;
/// Gate thresholds.
pub const MAX_GROWTH_PERCENT: u64 = 10;
pub const MAX_P99_US: u64 = 250_000;
/// A case's p99 may be at most `11/10` of its control's.
pub const MAX_P99_RATIO_NUM: u64 = 11;
pub const MAX_P99_RATIO_DEN: u64 = 10;
pub const MAX_PEAK_RSS_BYTES: u64 = 4 << 30;
/// The required runner: Linux x86-64 GNU, four logical CPUs, 16 GiB, local SSD, a release
/// fixture, and eight model workers. Memory may sit up to 1 GiB below 16 GiB for kernel
/// reservations.
pub const REQUIRED_CPUS: u32 = 4;
pub const REQUIRED_MEMORY_BYTES: u64 = 16 << 30;
pub const MEMORY_TOLERANCE_BYTES: u64 = 1 << 30;
pub const REQUIRED_MODEL_WORKERS: u32 = 8;
/// A report qualifies only after every witness in `WITNESSES` has a recorded run at the report's source commit that exited zero.
pub const WITNESSES: [&str; 5] = [
    "normal_fold",
    "emergency_fold",
    "wrapup_fold",
    "reattach_fold",
    "rotation_soak",
];

/// One run of the test that witnesses a named behavior.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WitnessRun {
    /// The test as the operator invoked it, for example its cargo target and name.
    pub test: String,
    /// The source commit the test ran at; a run counts only toward a report measured at the same commit.
    pub source_commit: String,
    pub exit_code: i32,
}

/// Checks that every recorded run names a witness in `WITNESSES` and a test.
///
/// # Errors
///
/// Returns a message naming the first unknown witness or empty test.
pub fn check_witness_runs(runs: &BTreeMap<String, WitnessRun>) -> Result<(), String> {
    for (name, run) in runs {
        if !WITNESSES.contains(&name.as_str()) {
            return Err(format!("witness {name:?} is outside {WITNESSES:?}"));
        }
        if run.test.trim().is_empty() {
            return Err(format!("witness {name:?} names no test"));
        }
    }
    Ok(())
}

/// The message mix of one case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    /// Alternating user and assistant text with a tool call and result at every sixteenth ordinal.
    Mixed,
    /// One message at the largest admitted size.
    LargestAdmitted,
    SameRole,
    /// Whitespace, punctuation, and repeated tokens.
    Noise,
    SystemOnly,
    /// Every third ordinal present.
    Sparse,
    /// Each assistant message calls a tool whose result follows.
    ToolArc,
}

/// One qualification case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub name: String,
    /// Retained messages per session.
    pub retained: u64,
    pub sessions: u32,
    pub shape: Shape,
    /// The case whose figures this case's growth and latency ratio gates compare against.
    pub control: Option<String>,
}

fn case(name: &str, retained: u64, sessions: u32, shape: Shape, control: Option<&str>) -> Case {
    Case {
        name: name.into(),
        retained,
        sessions,
        shape,
        control: control.map(Into::into),
    }
}

/// The fixed catalog: retained-history tiers, the 1000-session tier, the windowed
/// 10x-folded case, and the shape cases at the 10k tier.
pub fn catalog() -> Vec<Case> {
    let control = Some("retained_10k");
    let windowed = ACTIVE_WINDOW * FOLDED_MULTIPLE;
    vec![
        case("retained_10k", 10_000, 1, Shape::Mixed, None),
        case("retained_100k", 100_000, 1, Shape::Mixed, control),
        case("retained_1m", 1_000_000, 1, Shape::Mixed, control),
        case("sessions_1000x1000", 1_000, 1_000, Shape::Mixed, control),
        case("window_10x_folded", windowed, 1, Shape::Mixed, control),
        case(
            "largest_admitted",
            10_000,
            1,
            Shape::LargestAdmitted,
            control,
        ),
        case("same_role", 10_000, 1, Shape::SameRole, control),
        case("noise", 10_000, 1, Shape::Noise, control),
        case("system_only", 10_000, 1, Shape::SystemOnly, control),
        case("sparse", 10_000, 1, Shape::Sparse, control),
        case("tool_arc", 10_000, 1, Shape::ToolArc, control),
    ]
}

/// Why a host cannot stand for the required runner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostShortfall {
    Architecture,
    Cpus,
    Memory,
    Disk,
    DebugBuild,
    ModelWorkers,
}

/// What the runner shell observed about the measuring environment, including the
/// effective cgroup limits it ran under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Environment {
    pub host: HostManifest,
    pub target: String,
    /// Logical CPUs the process may run on (affinity and cgroup quota).
    pub effective_cpus: u32,
    /// Memory the process may use (cgroup limit or physical memory).
    pub effective_memory_bytes: u64,
    /// Whether the effective limits come from a cgroup on a larger host.
    pub constrained_by_cgroup: bool,
    /// Filesystem type behind the campaign's state roots, such as `xfs` or `tmpfs`.
    pub state_filesystem: String,
    /// Whether the fixture binary was built with debug assertions.
    pub fixture_debug: bool,
    /// SHA-256 of the fixture binary the campaign measured.
    pub fixture_sha256: String,
    pub model_workers: u32,
}

impl Environment {
    /// Every shortfall from the required runner. A cgroup-constrained host that meets the
    /// limits is still reported as constrained, never as the dedicated runner.
    pub fn shortfalls(&self) -> Vec<HostShortfall> {
        let mut shortfalls = Vec::new();
        if self.target != "x86_64-unknown-linux-gnu" {
            shortfalls.push(HostShortfall::Architecture);
        }
        if self.effective_cpus != REQUIRED_CPUS {
            shortfalls.push(HostShortfall::Cpus);
        }
        let memory = REQUIRED_MEMORY_BYTES - MEMORY_TOLERANCE_BYTES..=REQUIRED_MEMORY_BYTES;
        if !memory.contains(&self.effective_memory_bytes) {
            shortfalls.push(HostShortfall::Memory);
        }
        if self.host.disk != "ssd" || matches!(self.state_filesystem.as_str(), "tmpfs" | "ramfs") {
            shortfalls.push(HostShortfall::Disk);
        }
        if self.fixture_debug {
            shortfalls.push(HostShortfall::DebugBuild);
        }
        if self.model_workers != REQUIRED_MODEL_WORKERS {
            shortfalls.push(HostShortfall::ModelWorkers);
        }
        shortfalls
    }
}

/// The measured figures of one repetition of one case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Repetition {
    pub index: u32,
    /// This repetition's place in its round's case order.
    pub position: u32,
    /// The generator writing the retained history into the store.
    pub seed_us: u64,
    pub ingested_bytes: u64,
    /// The daemon's cold open of that store: every session reports ready, then each
    /// session's first pass replays its retained history once.
    pub cold_open_us: u64,
    /// Bytes the daemon read (`rchar`) from its launch through the cold open, accounted apart
    /// from the interactive phase.
    pub cold_read_bytes: u64,
    pub operations: u64,
    pub failed_operations: u64,
    pub latency_us: Histogram,
    /// Bytes the daemon read (`rchar`) and pages it first touched (minor faults) over the
    /// interactive phase. They are process-level proxies for candidate scans and allocations,
    /// not row or allocator counters.
    pub read_bytes: u64,
    pub minor_faults: u64,
    pub peak_rss_bytes: u64,
    /// The store-backed lineage audit of the repetition's interactive phase.
    pub lineage_violations: Vec<LineageViolation>,
}

/// Every repetition of one case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseResult {
    pub case: Case,
    pub repetitions: Vec<Repetition>,
    /// Every repetition that could not run, with its error.
    pub failed_repetitions: Vec<String>,
}

/// The p99 of a histogram as its bucket's upper bound; `None` when empty. Censored
/// observations rank above every completed one, so a p99 rank among them is `u64::MAX`.
pub fn p99_upper_us(histogram: &Histogram) -> Option<u64> {
    let total = histogram.count();
    if total == 0 {
        return None;
    }
    let rank = (99 * total).div_ceil(100);
    if rank > total - histogram.censored {
        return Some(u64::MAX);
    }
    let mut seen = 0;
    for bucket in &histogram.buckets {
        seen += bucket.count;
        if seen >= rank {
            return Some(crate::scale::bucket_upper_bound(bucket.index));
        }
    }
    None
}

impl CaseResult {
    pub fn operations(&self) -> u64 {
        self.repetitions.iter().map(|r| r.operations).sum()
    }

    /// The worst repetition's p99; each repetition is one independent unit.
    pub fn worst_p99_us(&self) -> u64 {
        self.repetitions
            .iter()
            .map(|r| p99_upper_us(&r.latency_us).unwrap_or(u64::MAX))
            .max()
            .unwrap_or(u64::MAX)
    }

    /// The median repetition's p99, the control side of the ratio gate.
    pub fn median_p99_us(&self) -> u64 {
        median(
            self.repetitions
                .iter()
                .map(|r| p99_upper_us(&r.latency_us).unwrap_or(u64::MAX)),
        )
        .unwrap_or(u64::MAX)
    }

    pub fn peak_rss_bytes(&self) -> u64 {
        self.repetitions
            .iter()
            .map(|r| r.peak_rss_bytes)
            .max()
            .unwrap_or(0)
    }

    fn per_operation_work(&self) -> impl Iterator<Item = Work> + '_ {
        self.repetitions.iter().map(|r| Work {
            read: Rate::new(r.read_bytes, r.operations),
            faults: Rate::new(r.minor_faults, r.operations),
        })
    }

    fn work_against(&self, control: &Self) -> (Work, Work) {
        let worst = Work {
            read: self
                .per_operation_work()
                .map(|w| w.read)
                .max()
                .unwrap_or_default(),
            faults: self
                .per_operation_work()
                .map(|w| w.faults)
                .max()
                .unwrap_or_default(),
        };
        let control_median = Work {
            read: median(control.per_operation_work().map(|w| w.read)).unwrap_or_default(),
            faults: median(control.per_operation_work().map(|w| w.faults)).unwrap_or_default(),
        };
        (worst, control_median)
    }
}

#[derive(Debug, Clone, Copy)]
struct Work {
    read: Rate,
    faults: Rate,
}

/// An exact per-operation rate. Cross-multiplication preserves fractional per-operation
/// differences when comparing rates.
#[derive(Debug, Clone, Copy)]
struct Rate {
    total: u64,
    operations: u64,
}

impl Default for Rate {
    fn default() -> Self {
        Self::new(0, 1)
    }
}

impl Rate {
    fn new(total: u64, operations: u64) -> Self {
        Self {
            total,
            operations: operations.max(1),
        }
    }

    fn per_1k_ops(self) -> u64 {
        let scaled = u128::from(self.total) * 1_000;
        u64::try_from(scaled.div_ceil(u128::from(self.operations))).unwrap_or(u64::MAX)
    }
}

impl Ord for Rate {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (u128::from(self.total) * u128::from(other.operations))
            .cmp(&(u128::from(other.total) * u128::from(self.operations)))
    }
}

impl PartialOrd for Rate {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Rate {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for Rate {}

/// The lower median when the count is even.
fn median<T: Ord>(values: impl Iterator<Item = T>) -> Option<T> {
    let mut values: Vec<T> = values.collect();
    values.sort_unstable();
    let index = values.len().saturating_sub(1) / 2;
    values.into_iter().nth(index)
}

/// One failed gate.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "gate")]
pub enum GateFailure {
    Repetitions {
        kept: u32,
    },
    FailedRepetitions {
        failed: u32,
    },
    Operations {
        measured: u64,
    },
    FailedOperations {
        failed: u64,
    },
    Lineage {
        violations: u64,
    },
    /// Bytes read per thousand operations, rounded up; the gate compares the exact rates.
    ReadGrowth {
        case_per_1k_ops: u64,
        control_per_1k_ops: u64,
    },
    /// Minor faults per thousand operations, rounded up; the gate compares the exact rates.
    AllocationGrowth {
        case_per_1k_ops: u64,
        control_per_1k_ops: u64,
    },
    P99Absolute {
        p99_us: u64,
    },
    P99Ratio {
        p99_us: u64,
        control_p99_us: u64,
    },
    PeakRss {
        bytes: u64,
    },
    MissingControl,
}

fn grew(case: Rate, control: Rate) -> bool {
    let case_scaled = u128::from(case.total) * u128::from(control.operations);
    let control_scaled = u128::from(control.total) * u128::from(case.operations);
    case_scaled.saturating_mul(100)
        > control_scaled.saturating_mul(u128::from(100 + MAX_GROWTH_PERCENT))
}

/// Every gate `result` fails against its control in `results`.
pub fn gate(result: &CaseResult, results: &[CaseResult]) -> Vec<GateFailure> {
    let mut failures = Vec::new();
    let kept = result.repetitions.len() as u32;
    if kept != REPETITIONS {
        failures.push(GateFailure::Repetitions { kept });
    }
    if !result.failed_repetitions.is_empty() {
        failures.push(GateFailure::FailedRepetitions {
            failed: result.failed_repetitions.len() as u32,
        });
    }
    let measured = result.operations();
    if measured < MIN_INTERACTIVE_OPERATIONS {
        failures.push(GateFailure::Operations { measured });
    }
    let failed: u64 = result.repetitions.iter().map(|r| r.failed_operations).sum();
    if failed > 0 {
        failures.push(GateFailure::FailedOperations { failed });
    }
    let violations: usize = result
        .repetitions
        .iter()
        .map(|r| r.lineage_violations.len())
        .sum();
    if violations > 0 {
        failures.push(GateFailure::Lineage {
            violations: violations as u64,
        });
    }
    let p99 = result.worst_p99_us();
    if p99 > MAX_P99_US {
        failures.push(GateFailure::P99Absolute { p99_us: p99 });
    }
    let bytes = result.peak_rss_bytes();
    if bytes > MAX_PEAK_RSS_BYTES {
        failures.push(GateFailure::PeakRss { bytes });
    }
    if let Some(name) = &result.case.control {
        let Some(control) = results.iter().find(|r| &r.case.name == name) else {
            failures.push(GateFailure::MissingControl);
            return failures;
        };
        failures.extend(compare(result, control));
    }
    failures
}

pub fn compare(result: &CaseResult, control: &CaseResult) -> Vec<GateFailure> {
    let mut failures = Vec::new();
    let (work, control_work) = result.work_against(control);
    if grew(work.read, control_work.read) {
        failures.push(GateFailure::ReadGrowth {
            case_per_1k_ops: work.read.per_1k_ops(),
            control_per_1k_ops: control_work.read.per_1k_ops(),
        });
    }
    if grew(work.faults, control_work.faults) {
        failures.push(GateFailure::AllocationGrowth {
            case_per_1k_ops: work.faults.per_1k_ops(),
            control_per_1k_ops: control_work.faults.per_1k_ops(),
        });
    }
    let p99 = result.worst_p99_us();
    let control_p99 = control.median_p99_us();
    if u128::from(p99) * u128::from(MAX_P99_RATIO_DEN)
        > u128::from(control_p99) * u128::from(MAX_P99_RATIO_NUM)
    {
        failures.push(GateFailure::P99Ratio {
            p99_us: p99,
            control_p99_us: control_p99,
        });
    }
    failures
}

/// Seconds to drain `backlog` tokens at `fold` and `arrival` tokens per second, or `None`
/// when folding does not outpace arrival.
pub fn drain_seconds(backlog: u64, fold_per_second: u64, arrival_per_second: u64) -> Option<u64> {
    let margin = fold_per_second
        .checked_sub(arrival_per_second)
        .filter(|m| *m > 0)?;
    Some(backlog.div_ceil(margin))
}

/// The frozen arrival rate: half the calibrated fold rate.
pub fn frozen_arrival(fold_per_second: u64) -> u64 {
    fold_per_second / 2
}

/// One phase of the outage run, in seconds from its start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Steady,
    Outage,
    Cooldown,
    Recovery,
}

/// The outage run: a steady interval, a source outage, the source cooldown, and the bounded
/// recovery window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutageSchedule {
    pub steady_seconds: u64,
    pub outage_seconds: u64,
    pub cooldown_seconds: u64,
    pub recovery_seconds: u64,
}

impl OutageSchedule {
    /// The fixed schedule: 60 s steady, the 120 s outage, the longest source cooldown (300 s),
    /// then the 360 s recovery bound.
    pub fn fixed() -> Self {
        Self {
            steady_seconds: 60,
            outage_seconds: OUTAGE_SECONDS,
            cooldown_seconds: 300,
            recovery_seconds: RECOVERY_BOUND_SECONDS,
        }
    }

    pub fn phase_at(&self, second: u64) -> Option<Phase> {
        let ends = [
            (self.steady_seconds, Phase::Steady),
            (self.outage_seconds, Phase::Outage),
            (self.cooldown_seconds, Phase::Cooldown),
            (self.recovery_seconds, Phase::Recovery),
        ];
        let mut start = 0;
        for (length, phase) in ends {
            if second < start + length {
                return Some(phase);
            }
            start += length;
        }
        None
    }

    pub fn total_seconds(&self) -> u64 {
        self.steady_seconds + self.outage_seconds + self.cooldown_seconds + self.recovery_seconds
    }

    /// Every duration divided by `divisor`, for a short smoke run.
    pub fn scaled(divisor: u64) -> Self {
        let fixed = Self::fixed();
        let divisor = divisor.max(1);
        Self {
            steady_seconds: fixed.steady_seconds / divisor,
            outage_seconds: fixed.outage_seconds / divisor,
            cooldown_seconds: fixed.cooldown_seconds / divisor,
            recovery_seconds: fixed.recovery_seconds / divisor,
        }
    }
}

/// One backlog sample: seconds since the run started and unfolded tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BacklogSample {
    pub second: u64,
    pub elapsed_ms: u64,
    pub tokens: u64,
}

/// One outage run: the calibrated rates, the backlog samples, and the store-backed lineage
/// audit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutageRun {
    pub schedule: OutageSchedule,
    /// Folded tokens per second measured while the backlog stayed above one chunk.
    pub fold_tokens_per_second: u64,
    pub arrival_tokens_per_second: u64,
    /// `(backlog at cooldown end - pre-outage band) / (fold - arrival)`, in seconds.
    pub drain_estimate_seconds: Option<u64>,
    pub samples: Vec<BacklogSample>,
    /// Passes that failed outside the outage phase.
    pub failed_passes: u64,
    pub lineage_violations: Vec<LineageViolation>,
}

/// A pass may finish up to this long after its scheduled second before the run is judged to
/// have slipped its schedule.
pub const MAX_PASS_SLIP_MS: u64 = 2_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "gate")]
pub enum OutageFailure {
    Lineage {
        violations: u64,
    },
    ArrivalNotFrozen {
        arrival: u64,
        fold: u64,
    },
    FailedPasses {
        failed: u64,
    },
    ScheduleSlipped {
        second: u64,
        elapsed_ms: u64,
    },
    NoPreOutageBand,
    /// The backlog never rose above the pre-outage band, so the outage had no effect.
    OutageNotObserved {
        band: u64,
    },
    /// A derived gate beyond the measured recovery gate: the drain estimate from the calibrated
    /// rates exceeds the recovery window.
    DrainExceedsRecovery {
        estimate_seconds: Option<u64>,
    },
    RunFailed {
        error: String,
    },
    NotRecovered {
        last_backlog: u64,
        band: u64,
    },
}

impl OutageRun {
    pub fn new(
        schedule: OutageSchedule,
        fold_tokens_per_second: u64,
        samples: Vec<BacklogSample>,
        failed_passes: u64,
        lineage_violations: Vec<LineageViolation>,
    ) -> Self {
        let mut run = Self {
            schedule,
            fold_tokens_per_second,
            arrival_tokens_per_second: frozen_arrival(fold_tokens_per_second),
            drain_estimate_seconds: None,
            samples,
            failed_passes,
            lineage_violations,
        };
        run.drain_estimate_seconds = run.drain_estimate();
        run
    }

    fn in_phase(&self, phase: Phase) -> impl Iterator<Item = &BacklogSample> {
        self.samples
            .iter()
            .filter(move |s| self.schedule.phase_at(s.second) == Some(phase))
    }

    /// The largest backlog over the second half of the steady phase, after the calibration
    /// backlog has had half the phase to drain.
    pub fn pre_outage_band(&self) -> Option<u64> {
        let settled = self.schedule.steady_seconds / 2;
        self.in_phase(Phase::Steady)
            .filter(|s| s.second >= settled)
            .map(|s| s.tokens)
            .max()
    }

    fn drain_estimate(&self) -> Option<u64> {
        let band = self.pre_outage_band()?;
        let cooldown_end = self.in_phase(Phase::Cooldown).last()?.tokens;
        drain_seconds(
            cooldown_end.saturating_sub(band),
            self.fold_tokens_per_second,
            self.arrival_tokens_per_second,
        )
    }

    /// The outage must raise the backlog above the pre-outage band, the drain estimate must
    /// fit the recovery window, and recovery requires a sample at or below the band inside
    /// that window.
    pub fn judge(&self) -> Vec<OutageFailure> {
        let mut failures = Vec::new();
        if !self.lineage_violations.is_empty() {
            failures.push(OutageFailure::Lineage {
                violations: self.lineage_violations.len() as u64,
            });
        }
        if self.arrival_tokens_per_second != frozen_arrival(self.fold_tokens_per_second)
            || self.fold_tokens_per_second == 0
        {
            failures.push(OutageFailure::ArrivalNotFrozen {
                arrival: self.arrival_tokens_per_second,
                fold: self.fold_tokens_per_second,
            });
        }
        if self.failed_passes > 0 {
            failures.push(OutageFailure::FailedPasses {
                failed: self.failed_passes,
            });
        }
        if let Some(slipped) = self
            .samples
            .iter()
            .find(|s| s.elapsed_ms > s.second * 1_000 + MAX_PASS_SLIP_MS)
        {
            failures.push(OutageFailure::ScheduleSlipped {
                second: slipped.second,
                elapsed_ms: slipped.elapsed_ms,
            });
        }
        let Some(band) = self.pre_outage_band() else {
            failures.push(OutageFailure::NoPreOutageBand);
            return failures;
        };
        let rose = [Phase::Outage, Phase::Cooldown]
            .into_iter()
            .any(|phase| self.in_phase(phase).any(|s| s.tokens > band));
        if !rose {
            failures.push(OutageFailure::OutageNotObserved { band });
        }
        let estimate = self.drain_estimate();
        if estimate.is_none_or(|seconds| seconds > self.schedule.recovery_seconds) {
            failures.push(OutageFailure::DrainExceedsRecovery {
                estimate_seconds: estimate,
            });
        }
        if !self.in_phase(Phase::Recovery).any(|s| s.tokens <= band) {
            let last_backlog = self.samples.last().map_or(u64::MAX, |s| s.tokens);
            failures.push(OutageFailure::NotRecovered { last_backlog, band });
        }
        failures
    }
}

/// Virtual seconds at which the soak rotates the source credential, one per hour.
pub fn rotation_schedule() -> Vec<u64> {
    let every = SOAK_SECONDS / u64::from(SOAK_ROTATIONS);
    (1..=u64::from(SOAK_ROTATIONS)).map(|n| n * every).collect()
}

/// One durable publication read from the store: its lineage (the segment sequence) and the
/// message range it covered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publication {
    pub lineage: String,
    pub start: u64,
    pub end: u64,
}

/// One retained raw transcript: its inclusive ordinal range and compressed size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawRange {
    pub start: u64,
    pub end: u64,
    pub bytes: u64,
}

/// What the store held at one instant: the retained raw transcripts and the covered ordinal
/// the session's state records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreObservation {
    pub raw: Vec<RawRange>,
    pub covered: u64,
}

impl StoreObservation {
    fn ranges(&self) -> Vec<(u64, u64)> {
        self.raw.iter().map(|r| (r.start, r.end)).collect()
    }

    fn bytes(&self) -> u64 {
        self.raw.iter().map(|r| r.bytes).sum()
    }
}

/// `ranges` sorted and merged into disjoint inclusive ranges.
fn merged(ranges: &[(u64, u64)]) -> Vec<(u64, u64)> {
    let mut sorted = ranges.to_vec();
    sorted.sort_unstable();
    let mut out: Vec<(u64, u64)> = Vec::new();
    for (start, end) in sorted {
        match out.last_mut() {
            Some(last) if start <= last.1.saturating_add(1) => last.1 = last.1.max(end),
            _ => out.push((start, end)),
        }
    }
    out
}

/// The parts of `before` that `after` no longer covers.
fn lost(before: &[(u64, u64)], after: &[(u64, u64)]) -> Vec<(u64, u64)> {
    let after = merged(after);
    let mut missing = Vec::new();
    for (start, end) in merged(before) {
        let mut cursor = start;
        for &(a, b) in after.iter().filter(|(a, b)| *b >= start && *a <= end) {
            if a > cursor {
                missing.push((cursor, a - 1));
            }
            cursor = cursor.max(b.saturating_add(1));
        }
        if cursor <= end {
            missing.push((cursor, end));
        }
    }
    missing
}

/// The oldest-first retention model requires every missing range to precede every retained
/// range. When the newest dropped transcript is one `before` observed, keeping it would also
/// have exceeded the cap. Transcripts published and evicted between the observations are
/// judged by order alone.
fn evicted(
    before: &StoreObservation,
    after: &StoreObservation,
    publications: &[Publication],
    missing: &[(u64, u64)],
    cap: u64,
) -> bool {
    let oldest_kept = after.raw.iter().map(|r| r.start).min().unwrap_or(u64::MAX);
    if !missing.iter().all(|(_, end)| *end < oldest_kept) {
        return false;
    }
    let newest_published_dropped = publications
        .iter()
        .filter(|p| {
            !after
                .raw
                .iter()
                .any(|r| r.start == p.start && r.end == p.end)
        })
        .map(|p| p.end)
        .max();
    let newest_observed_dropped = before
        .raw
        .iter()
        .filter(|r| !after.raw.contains(r))
        .max_by_key(|r| r.end);
    match newest_observed_dropped {
        Some(row) if newest_published_dropped.is_none_or(|end| end < row.end) => {
            after.bytes() + row.bytes > cap
        }
        _ => true,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "violation")]
pub enum LineageViolation {
    /// One lineage published the same range twice, or two lineages overlap.
    DuplicatePublication {
        lineage: String,
        start: u64,
        end: u64,
    },
    RawHistoryLost {
        start: u64,
        end: u64,
    },
    /// Coverage advanced past every published range.
    FalseProgress {
        covered: u64,
        published: u64,
    },
}

/// Audits per-publication accounting: each lineage publishes once, ranges never overlap, raw
/// messages are lost only to the store's oldest-first eviction under `raw_cap_bytes`, and
/// coverage is contiguous from the prior coverage through the publications. The audited raw
/// history is the `before` transcripts plus the transcript the store writes with each
/// publication.
pub fn audit_lineage(
    before: &StoreObservation,
    after: &StoreObservation,
    publications: &[Publication],
    raw_cap_bytes: u64,
) -> Vec<LineageViolation> {
    let mut violations = BTreeSet::new();
    let mut sorted: Vec<&Publication> = publications.iter().collect();
    sorted.sort_by_key(|p| (p.start, p.end));
    let mut lineages = BTreeSet::new();
    let mut reach = before.covered;
    let mut contiguous = before.covered;
    for publication in sorted {
        let duplicate = !lineages.insert(publication.lineage.as_str());
        if duplicate || publication.start <= reach {
            violations.insert(LineageViolation::DuplicatePublication {
                lineage: publication.lineage.clone(),
                start: publication.start,
                end: publication.end,
            });
        }
        if publication.start <= contiguous + 1 {
            contiguous = contiguous.max(publication.end);
        }
        reach = reach.max(publication.end);
    }
    let mut expected = before.ranges();
    expected.extend(publications.iter().map(|p| (p.start, p.end)));
    let missing = lost(&expected, &after.ranges());
    if !evicted(before, after, publications, &missing, raw_cap_bytes) {
        for (start, end) in missing {
            violations.insert(LineageViolation::RawHistoryLost { start, end });
        }
    }
    if after.covered > contiguous {
        violations.insert(LineageViolation::FalseProgress {
            covered: after.covered,
            published: contiguous,
        });
    }
    violations.into_iter().collect()
}

/// The whole campaign report. Every repetition is kept, and the environment states how far
/// the measuring host is from the required runner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualificationReport {
    pub schema: String,
    pub source_commit: String,
    pub tokenizer: String,
    pub seed: u64,
    pub environment: Environment,
    pub host_shortfalls: Vec<HostShortfall>,
    pub cases: Vec<CaseResult>,
    pub gates: BTreeMap<String, Vec<GateFailure>>,
    pub outage: Option<OutageRun>,
    pub outage_failures: Vec<OutageFailure>,
    /// The comparison gates measure dispersion within this run by comparing the control's
    /// worst repetition with its median.
    pub control_dispersion: Vec<GateFailure>,
    /// Virtual seconds of the 24-hour auth soak's rotations.
    pub planned_rotations: Vec<u64>,
    /// The witness runs the operator recorded, keyed by witness.
    pub witness_runs: BTreeMap<String, WitnessRun>,
    /// Witnesses with no recorded run that exited zero at the report's source commit; the report cannot qualify until none remain.
    pub pending_witnesses: Vec<String>,
    /// A report from a host with shortfalls is baseline evidence, never qualification.
    pub qualified: bool,
}

impl QualificationReport {
    pub fn build(
        source_commit: String,
        tokenizer: String,
        environment: Environment,
        cases: Vec<CaseResult>,
        outage: Result<OutageRun, String>,
        witness_runs: BTreeMap<String, WitnessRun>,
    ) -> Self {
        let gates: BTreeMap<_, _> = cases
            .iter()
            .map(|result| (result.case.name.clone(), gate(result, &cases)))
            .collect();
        let outage_failures = match &outage {
            Ok(run) => run.judge(),
            Err(error) => vec![OutageFailure::RunFailed {
                error: error.clone(),
            }],
        };
        let outage = outage.ok();
        let host_shortfalls = environment.shortfalls();
        let complete = catalog().iter().all(|c| cases.iter().any(|r| r.case == *c));
        let pending_witnesses: Vec<String> = WITNESSES
            .iter()
            .filter(|w| {
                witness_runs
                    .get(**w)
                    .is_none_or(|run| run.exit_code != 0 || run.source_commit != source_commit)
            })
            .map(|w| (*w).to_owned())
            .collect();
        // `build` accepts runs from any caller, so a run naming no known witness or no test keeps the report unqualified here too.
        let witnesses_known = check_witness_runs(&witness_runs).is_ok();
        let control_dispersion = catalog()
            .iter()
            .find(|c| c.control.is_none())
            .and_then(|control| cases.iter().find(|r| r.case == *control))
            .map_or_else(
                || vec![GateFailure::MissingControl],
                |control| compare(control, control),
            );
        let qualified = host_shortfalls.is_empty()
            && pending_witnesses.is_empty()
            && witnesses_known
            && control_dispersion.is_empty()
            && !environment.constrained_by_cgroup
            && complete
            && gates.values().all(Vec::is_empty)
            && outage
                .as_ref()
                .is_some_and(|run| run.schedule == OutageSchedule::fixed())
            && outage_failures.is_empty();
        Self {
            schema: QUALIFICATION_SCHEMA.into(),
            source_commit,
            tokenizer,
            seed: QUALIFICATION_SEED,
            environment,
            host_shortfalls,
            cases,
            gates,
            outage,
            outage_failures,
            control_dispersion,
            planned_rotations: rotation_schedule(),
            witness_runs,
            pending_witnesses,
            qualified,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn histogram(values: &[u64]) -> Histogram {
        Histogram::of(&values.iter().map(|v| (*v, false)).collect::<Vec<_>>())
    }

    fn repetition(index: u32, latency: u64, read: u64, faults: u64) -> Repetition {
        Repetition {
            index,
            position: 0,
            seed_us: 1,
            ingested_bytes: 1,
            cold_open_us: 1,
            cold_read_bytes: 0,
            operations: 2_000,
            failed_operations: 0,
            latency_us: histogram(&vec![latency; 2_000]),
            read_bytes: read * 2_000,
            minor_faults: faults * 2_000,
            peak_rss_bytes: 1 << 30,
            lineage_violations: Vec::new(),
        }
    }

    fn result_with(name: &str, latency: u64, read: u64, faults: u64) -> CaseResult {
        let case = catalog().into_iter().find(|c| c.name == name).unwrap();
        CaseResult {
            case,
            repetitions: (0..REPETITIONS)
                .map(|i| repetition(i, latency, read, faults))
                .collect(),
            failed_repetitions: Vec::new(),
        }
    }

    fn result(name: &str, latency: u64, read: u64) -> CaseResult {
        result_with(name, latency, read, 10)
    }

    fn environment(cpus: u32, cgroup: bool) -> Environment {
        Environment {
            host: HostManifest {
                cpu_model: "x".into(),
                core_count: cpus,
                memory_bytes: REQUIRED_MEMORY_BYTES,
                kernel: "6".into(),
                glibc: "2".into(),
                disk: "ssd".into(),
            },
            target: "x86_64-unknown-linux-gnu".into(),
            effective_cpus: cpus,
            effective_memory_bytes: REQUIRED_MEMORY_BYTES,
            constrained_by_cgroup: cgroup,
            state_filesystem: "xfs".into(),
            fixture_debug: false,
            fixture_sha256: "f".into(),
            model_workers: REQUIRED_MODEL_WORKERS,
        }
    }

    fn sample(second: u64, tokens: u64) -> BacklogSample {
        BacklogSample {
            second,
            elapsed_ms: second * 1_000,
            tokens,
        }
    }

    fn outage(samples: Vec<BacklogSample>) -> OutageRun {
        OutageRun::new(OutageSchedule::fixed(), 200, samples, 0, Vec::new())
    }

    fn passing_samples() -> Vec<BacklogSample> {
        vec![
            sample(5, 50_000),
            sample(40, 500),
            sample(100, 9_000),
            sample(400, 2_000),
            sample(600, 400),
        ]
    }

    fn passing_outage() -> OutageRun {
        outage(passing_samples())
    }

    #[test]
    fn the_catalog_covers_every_tier_and_shape_against_the_10k_control() {
        let catalog = catalog();
        let shapes: BTreeSet<_> = catalog.iter().map(|c| c.shape).collect();
        assert_eq!(shapes.len(), 7);
        for retained in [10_000, 100_000, 1_000_000] {
            assert!(
                catalog
                    .iter()
                    .any(|c| c.retained == retained && c.sessions == 1)
            );
        }
        assert!(
            catalog
                .iter()
                .any(|c| c.sessions == 1_000 && c.retained == 1_000)
        );
        assert!(
            catalog
                .iter()
                .any(|c| c.retained == ACTIVE_WINDOW * FOLDED_MULTIPLE)
        );
        assert!(
            catalog
                .iter()
                .skip(1)
                .all(|c| c.control.as_deref() == Some("retained_10k"))
        );
    }

    #[test]
    fn passing_results_pass_and_each_gate_fires_alone_on_a_failing_case() {
        let control = result("retained_10k", 10_000, 4_096);
        let good = result("retained_1m", 10_500, 4_200);
        let results = vec![control.clone(), good.clone()];
        assert!(gate(&control, &results).is_empty());
        assert!(gate(&good, &results).is_empty());
        let only = |case: &CaseResult| gate(case, std::slice::from_ref(&control));

        let slow = result("retained_1m", 300_000, 4_096);
        assert!(
            only(&slow)
                .iter()
                .any(|f| matches!(f, GateFailure::P99Absolute { .. }))
        );
        let ratio = result("retained_1m", 20_000, 4_096);
        assert!(matches!(only(&ratio)[..], [GateFailure::P99Ratio { .. }]));
        let scanning = result("retained_1m", 10_000, 4_096 * 2);
        assert!(matches!(
            only(&scanning)[..],
            [GateFailure::ReadGrowth { .. }]
        ));
        let allocating = result_with("retained_1m", 10_000, 4_096, 12);
        assert!(matches!(
            only(&allocating)[..],
            [GateFailure::AllocationGrowth { .. }]
        ));

        let mut short = result("retained_1m", 10_000, 4_096);
        short.repetitions.pop();
        short.failed_repetitions.push("store open failed".into());
        let failures = only(&short);
        assert!(failures.contains(&GateFailure::Repetitions { kept: 4 }));
        assert!(failures.contains(&GateFailure::FailedRepetitions { failed: 1 }));
        assert!(failures.contains(&GateFailure::Operations { measured: 8_000 }));

        let mut audited = result("retained_1m", 10_000, 4_096);
        audited.repetitions[1]
            .lineage_violations
            .push(LineageViolation::RawHistoryLost { start: 1, end: 2 });
        assert_eq!(only(&audited), vec![GateFailure::Lineage { violations: 1 }]);

        let mut heavy = result("retained_1m", 10_000, 4_096);
        heavy.repetitions[3].peak_rss_bytes = MAX_PEAK_RSS_BYTES + 1;
        heavy.repetitions[2].failed_operations = 1;
        let failures = only(&heavy);
        assert!(
            failures
                .iter()
                .any(|f| matches!(f, GateFailure::PeakRss { .. }))
        );
        assert!(failures.contains(&GateFailure::FailedOperations { failed: 1 }));

        let mut one_slow = result("retained_1m", 10_000, 4_096);
        one_slow.repetitions[4].latency_us = histogram(&vec![400_000; 2_000]);
        assert!(
            only(&one_slow)
                .iter()
                .any(|f| matches!(f, GateFailure::P99Absolute { .. })),
            "one slow repetition fails the case"
        );
        assert_eq!(gate(&good, &[]), vec![GateFailure::MissingControl]);
    }

    #[test]
    fn growth_gates_compare_exact_per_operation_rates() {
        let control = result("retained_10k", 10_000, 0);
        let with_totals = |read: u64, faults: u64| {
            let mut case = result("retained_1m", 10_000, 0);
            for repetition in &mut case.repetitions {
                repetition.read_bytes = read;
                repetition.minor_faults = faults;
            }
            case
        };
        let mut sparse_control = control.clone();
        for repetition in &mut sparse_control.repetitions {
            repetition.read_bytes = 1_000;
            repetition.minor_faults = 1_000;
        }
        let doubled = with_totals(2_000, 2_000);
        assert_eq!(
            compare(&doubled, &sparse_control),
            vec![
                GateFailure::ReadGrowth {
                    case_per_1k_ops: 1_000,
                    control_per_1k_ops: 500,
                },
                GateFailure::AllocationGrowth {
                    case_per_1k_ops: 1_000,
                    control_per_1k_ops: 500,
                },
            ],
            "doubling half an event per operation is growth"
        );
        let mut busy_control = control;
        for repetition in &mut busy_control.repetitions {
            repetition.read_bytes = 2_000;
            repetition.minor_faults = 2_000;
        }
        assert!(
            compare(&with_totals(2_001, 2_001), &busy_control).is_empty(),
            "one extra event over 2,000 operations is within the allowance"
        );
    }

    #[test]
    fn drain_arrival_and_schedules_follow_the_fixed_rules() {
        assert_eq!(drain_seconds(36_000, 200, 100), Some(360));
        assert_eq!(drain_seconds(1, 100, 100), None);
        assert_eq!(frozen_arrival(301), 150);
        let schedule = OutageSchedule::fixed();
        assert_eq!(schedule.phase_at(0), Some(Phase::Steady));
        assert_eq!(schedule.phase_at(60), Some(Phase::Outage));
        assert_eq!(schedule.phase_at(180), Some(Phase::Cooldown));
        assert_eq!(schedule.phase_at(480), Some(Phase::Recovery));
        assert_eq!(schedule.phase_at(schedule.total_seconds()), None);
        assert_eq!(OutageSchedule::scaled(1), schedule);
        assert_eq!(OutageSchedule::scaled(60).steady_seconds, 1);
        let rotations = rotation_schedule();
        assert_eq!(rotations.len(), 24);
        assert_eq!(rotations.last(), Some(&SOAK_SECONDS));
    }

    #[test]
    fn the_outage_judge_fires_on_each_failure() {
        let passing = passing_outage();
        assert_eq!(
            passing.pre_outage_band(),
            Some(500),
            "calibration backlog is excluded"
        );
        assert_eq!(passing.arrival_tokens_per_second, 100);
        assert_eq!(passing.drain_estimate_seconds, Some(15));
        assert!(passing.judge().is_empty());
        let with = |change: fn(&mut Vec<BacklogSample>)| {
            let mut samples = passing_samples();
            change(&mut samples);
            outage(samples).judge()
        };
        assert_eq!(
            with(|s| s[4].tokens = 9_000),
            vec![OutageFailure::NotRecovered {
                last_backlog: 9_000,
                band: 500
            }]
        );
        assert_eq!(
            with(|s| {
                s[2].tokens = 100;
                s[3].tokens = 100;
            }),
            vec![OutageFailure::OutageNotObserved { band: 500 }]
        );
        assert_eq!(
            with(|s| s[3].tokens = 100_000),
            vec![OutageFailure::DrainExceedsRecovery {
                estimate_seconds: Some(995)
            }]
        );
        assert_eq!(
            with(|s| s[2].elapsed_ms += MAX_PASS_SLIP_MS + 1),
            vec![OutageFailure::ScheduleSlipped {
                second: 100,
                elapsed_ms: 102_001
            }]
        );
        assert!(
            with(|s| {
                s.remove(1);
            })
            .contains(&OutageFailure::NoPreOutageBand)
        );
        let mut fast = passing_outage();
        fast.arrival_tokens_per_second = 150;
        assert!(matches!(
            fast.judge()[..],
            [OutageFailure::ArrivalNotFrozen { .. }]
        ));
        let failing = OutageRun::new(
            OutageSchedule::fixed(),
            200,
            passing_samples(),
            2,
            Vec::new(),
        );
        assert_eq!(
            failing.judge(),
            vec![OutageFailure::FailedPasses { failed: 2 }]
        );
        let lossy = OutageRun::new(
            OutageSchedule::fixed(),
            200,
            passing_samples(),
            0,
            vec![LineageViolation::RawHistoryLost { start: 1, end: 1 }],
        );
        assert_eq!(
            lossy.judge(),
            vec![OutageFailure::Lineage { violations: 1 }]
        );
    }

    fn raw(ranges: &[(u64, u64)]) -> Vec<RawRange> {
        ranges
            .iter()
            .map(|&(start, end)| RawRange {
                start,
                end,
                bytes: 10,
            })
            .collect()
    }

    #[test]
    fn the_lineage_audit_fires_on_duplicates_overlaps_loss_and_gaps() {
        let uncapped = u64::MAX;
        let before = StoreObservation {
            raw: raw(&[(1, 10)]),
            covered: 0,
        };
        let publish = |lineage: &str, start, end| Publication {
            lineage: lineage.into(),
            start,
            end,
        };
        let after = |covered| StoreObservation {
            raw: raw(&[(1, 6), (7, 12)]),
            covered,
        };
        let honest = vec![publish("1", 1, 5), publish("2", 6, 8)];
        assert!(audit_lineage(&before, &after(8), &honest, uncapped).is_empty());
        let duplicate = |publications: &[Publication]| {
            audit_lineage(&before, &after(10), publications, uncapped)
                .iter()
                .filter(|v| matches!(v, LineageViolation::DuplicatePublication { .. }))
                .count()
        };
        assert_eq!(duplicate(&[publish("1", 1, 5), publish("1", 6, 10)]), 1);
        assert_eq!(
            duplicate(&[publish("1", 1, 10), publish("2", 3, 4), publish("3", 6, 7)]),
            2
        );
        assert_eq!(
            duplicate(&[publish("1", 1, 10), publish("2", 1, 2), publish("3", 5, 6)]),
            2
        );
        let lost = StoreObservation {
            raw: raw(&[(1, 3), (6, 9)]),
            covered: 8,
        };
        assert_eq!(
            audit_lineage(&before, &lost, &honest, uncapped),
            vec![
                LineageViolation::RawHistoryLost { start: 4, end: 5 },
                LineageViolation::RawHistoryLost { start: 10, end: 10 },
            ]
        );
        assert_eq!(
            audit_lineage(&before, &after(9), &honest, uncapped),
            vec![LineageViolation::FalseProgress {
                covered: 9,
                published: 8
            }]
        );
        let gapped = vec![publish("1", 1, 5), publish("2", 8, 10)];
        assert_eq!(
            audit_lineage(&before, &after(10), &gapped, uncapped),
            vec![LineageViolation::FalseProgress {
                covered: 10,
                published: 5
            }]
        );
    }

    #[test]
    fn oldest_first_eviction_under_the_cap_is_not_raw_history_loss() {
        let before = StoreObservation {
            raw: raw(&[(1, 2), (3, 4), (5, 6)]),
            covered: 6,
        };
        let evicted = StoreObservation {
            raw: raw(&[(3, 4), (5, 6), (7, 8)]),
            covered: 8,
        };
        let publication = [Publication {
            lineage: "4".into(),
            start: 7,
            end: 8,
        }];
        assert!(audit_lineage(&before, &evicted, &publication, 30).is_empty());
        let over_evicted = StoreObservation {
            raw: raw(&[(7, 8)]),
            covered: 8,
        };
        assert_eq!(
            audit_lineage(&before, &over_evicted, &publication, 30),
            vec![LineageViolation::RawHistoryLost { start: 1, end: 6 }],
            "keeping the newest dropped transcript would have fit the cap"
        );
        assert_eq!(
            audit_lineage(&before, &evicted, &publication, 100),
            vec![LineageViolation::RawHistoryLost { start: 1, end: 2 }],
            "under the cap nothing needed evicting"
        );
        let hole = StoreObservation {
            raw: raw(&[(1, 2), (5, 6), (7, 8)]),
            covered: 8,
        };
        assert_eq!(
            audit_lineage(&before, &hole, &publication, 0),
            vec![LineageViolation::RawHistoryLost { start: 3, end: 4 }],
            "eviction removes the oldest transcripts first"
        );
    }

    #[test]
    fn eviction_of_transcripts_published_inside_the_window_is_not_raw_history_loss() {
        let sized = |start, bytes| RawRange {
            start,
            end: start,
            bytes,
        };
        let publish = |start: u64| Publication {
            lineage: format!("segment-{start}"),
            start,
            end: start,
        };
        let before = StoreObservation {
            raw: vec![sized(1, 10)],
            covered: 1,
        };
        let cap = 100;
        let segment_2_bytes = 95;
        let segment_3_bytes = 20;
        assert!(10 + segment_2_bytes > cap && segment_2_bytes + segment_3_bytes > cap);
        let after = StoreObservation {
            raw: vec![sized(3, segment_3_bytes)],
            covered: 3,
        };
        let publications = [publish(2), publish(3)];
        assert_eq!(
            audit_lineage(&before, &after, &publications, cap),
            Vec::new(),
            "each publication evicted the transcript before it"
        );
    }

    #[test]
    fn a_transcript_published_inside_the_window_and_lost_is_raw_history_loss() {
        let before = StoreObservation {
            raw: raw(&[(1, 2), (3, 4)]),
            covered: 4,
        };
        let publication = [Publication {
            lineage: "segment-3".into(),
            start: 5,
            end: 6,
        }];
        let after = StoreObservation {
            raw: raw(&[(1, 2), (3, 4)]),
            covered: 6,
        };
        assert_eq!(
            audit_lineage(&before, &after, &publication, u64::MAX),
            vec![LineageViolation::RawHistoryLost { start: 5, end: 6 }],
            "older transcripts survive, so eviction cannot explain the missing one"
        );
        let kept = StoreObservation {
            raw: raw(&[(1, 2), (3, 4), (5, 6)]),
            covered: 6,
        };
        assert!(audit_lineage(&before, &kept, &publication, u64::MAX).is_empty());
    }

    #[test]
    fn a_censored_operation_ranks_above_every_completed_one() {
        let observations = |censored: usize, total: usize| {
            let mut all = vec![(1_000, false); total - censored];
            all.extend(std::iter::repeat_n((1, true), censored));
            Histogram::of(&all)
        };
        assert_eq!(p99_upper_us(&observations(2, 100)), Some(u64::MAX));
        assert!(p99_upper_us(&observations(1, 1_000)).is_some_and(|p| p < u64::MAX));
        assert!(p99_upper_us(&Histogram::default()).is_none());
    }

    #[test]
    fn only_a_complete_passing_run_on_the_dedicated_runner_qualifies() {
        assert!(environment(4, false).shortfalls().is_empty());
        assert_eq!(
            environment(128, false).shortfalls(),
            vec![HostShortfall::Cpus]
        );
        let shortfall = |change: fn(&mut Environment)| {
            let mut env = environment(4, false);
            change(&mut env);
            env.shortfalls()
        };
        assert_eq!(
            shortfall(|e| e.effective_memory_bytes = 8 << 30),
            [HostShortfall::Memory]
        );
        assert_eq!(
            shortfall(|e| e.state_filesystem = "tmpfs".into()),
            [HostShortfall::Disk]
        );
        assert_eq!(
            shortfall(|e| e.fixture_debug = true),
            [HostShortfall::DebugBuild]
        );
        assert_eq!(
            shortfall(|e| e.model_workers = 2),
            [HostShortfall::ModelWorkers]
        );
        assert_eq!(
            shortfall(|e| e.target = "aarch64-unknown-linux-gnu".into()),
            [HostShortfall::Architecture]
        );
        let all = || {
            catalog()
                .iter()
                .map(|c| result(&c.name, 10_000, 4_096))
                .collect::<Vec<_>>()
        };
        let run = |exit_code| WitnessRun {
            test: "daemon::source_recovery_tests".into(),
            source_commit: "c".into(),
            exit_code,
        };
        let passed: BTreeMap<String, WitnessRun> = WITNESSES
            .iter()
            .map(|w| ((*w).to_owned(), run(0)))
            .collect();
        let report = |env, cases, outage, witnesses: &BTreeMap<String, WitnessRun>| {
            QualificationReport::build(
                "c".into(),
                "t".into(),
                env,
                cases,
                outage,
                witnesses.clone(),
            )
        };
        let full = report(environment(4, false), all(), Ok(passing_outage()), &passed);
        assert!(full.host_shortfalls.is_empty());
        assert!(full.gates.values().all(Vec::is_empty));
        assert!(full.outage_failures.is_empty());
        assert!(full.control_dispersion.is_empty());
        assert!(full.qualified);
        assert_eq!(full.planned_rotations.len(), 24);
        assert_eq!(full.witness_runs, passed);
        for missing in WITNESSES {
            let mut runs = passed.clone();
            runs.remove(missing);
            let pending = report(environment(4, false), all(), Ok(passing_outage()), &runs);
            assert_eq!(pending.pending_witnesses, vec![missing.to_owned()]);
            assert!(!pending.qualified);
            runs.insert(missing.to_owned(), run(1));
            let failed = report(environment(4, false), all(), Ok(passing_outage()), &runs);
            assert_eq!(failed.pending_witnesses, vec![missing.to_owned()]);
            assert!(
                !failed.qualified,
                "a failed {missing} run keeps the report pending"
            );
            let mut stale = run(0);
            stale.source_commit = "earlier".into();
            runs.insert(missing.to_owned(), stale);
            let stale = report(environment(4, false), all(), Ok(passing_outage()), &runs);
            assert_eq!(stale.pending_witnesses, vec![missing.to_owned()]);
            assert!(
                !stale.qualified,
                "a {missing} run at another commit keeps the report pending"
            );
        }
        let mut unknown = passed.clone();
        unknown.insert("warm_soak".into(), run(0));
        assert!(check_witness_runs(&unknown).is_err());
        assert!(!report(environment(4, false), all(), Ok(passing_outage()), &unknown).qualified);
        let mut untested = passed.clone();
        untested.get_mut(WITNESSES[0]).unwrap().test = " ".into();
        assert!(check_witness_runs(&untested).is_err());
        assert!(
            !report(
                environment(4, false),
                all(),
                Ok(passing_outage()),
                &untested
            )
            .qualified
        );
        assert!(check_witness_runs(&passed).is_ok());
        assert!(!report(environment(4, true), all(), Ok(passing_outage()), &passed).qualified);
        let scaled = OutageRun::new(
            OutageSchedule::scaled(2),
            200,
            passing_samples(),
            0,
            Vec::new(),
        );
        assert!(!report(environment(4, false), all(), Ok(scaled), &passed).qualified);
        let failed = report(
            environment(4, false),
            all(),
            Err("fixture exited".into()),
            &passed,
        );
        assert_eq!(
            failed.outage_failures,
            vec![OutageFailure::RunFailed {
                error: "fixture exited".into()
            }]
        );
        assert!(failed.outage.is_none() && !failed.qualified);
        let mut noisy = all();
        noisy[0].repetitions[4].latency_us = histogram(&vec![20_000; 2_000]);
        let unstable = report(environment(4, false), noisy, Ok(passing_outage()), &passed);
        assert!(matches!(
            unstable.control_dispersion[..],
            [GateFailure::P99Ratio { .. }]
        ));
        assert!(
            !unstable.qualified,
            "gates that reject identical work cannot qualify"
        );
    }
}
