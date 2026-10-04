//! `eval-scale-report/v1`: per-pass rows from a scale driver, the session and tier summaries
//! derived from them, and the tier ratio and flatness gates of the scale measurement contract
//! (#825 Verification Strategy). A report is built from its rows and parsed by rebuilding it, so
//! every summary is a function of the rows the report carries.

use std::collections::BTreeMap;

use context_core::canonical_json::{
    ContractError, canonical_json_encode, is_lower_hex, protocol_digest,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::censoring::{
    BoundMethod, P99_MIN_RUNS, Percentile, PercentileBound, censored_percentile, failure_bound,
};
use crate::statistics::{MAX_BOOTSTRAP_DRAWS, Ratio, StatisticsError, bootstrap_draw};

pub const SCALE_REPORT_SCHEMA: &str = "eval-scale-report/v1";
const SCALE_RESULT_DIGEST_PROTOCOL: &str = "eval-scale-report-result/v1";
/// A tier ratio claim needs this many independent sessions in the tier and in its control.
pub const MIN_RATIO_SESSIONS: usize = 3;
/// Bootstrap replicates behind every tier ratio interval.
pub const RATIO_REPLICATES: u32 = 1_000;
/// The tier ratio gate passes when the interval's upper bound is at most `6/5`.
pub const RATIO_GATE: (i64, u64) = (6, 5);
/// Row measurements the result digest excludes.
const ROW_MEASUREMENTS: [&str; 4] = ["response_us", "service_us", "rss_bytes", "ipc_bytes"];
/// Sub-buckets per power of two in a [`Histogram`]; values below this count map to themselves.
const HISTOGRAM_SUB_BUCKETS: u64 = 32;

/// A session tier, named by its message count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ScaleTier {
    /// The 10k-message control every other tier's ratio is taken against.
    #[serde(rename = "10k")]
    Control10k,
    #[serde(rename = "s3_100k")]
    S3_100k,
    #[serde(rename = "s4_1m")]
    S4_1m,
}

impl ScaleTier {
    pub const fn messages(self) -> u64 {
        match self {
            Self::Control10k => 10_000,
            Self::S3_100k => 100_000,
            Self::S4_1m => 1_000_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScaleHarness {
    Opencode,
    Pi,
}

/// Where a pass sits in its session's boundary history. Summaries pool passes of one state
/// only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundaryState {
    /// No rendered boundary yet.
    Cold,
    /// After the first HARD and before steady-state entry.
    Warming,
    /// After the window reached its fold size and the boundary moved three times, confirmed
    /// by the driver's drift check.
    Steady,
    /// A replay of the previous pass's window at the same declared boundary with no fold in
    /// between: #824's fresh-host-array pass measurement.
    Replay,
    /// The first pass after a daemon or plugin restart.
    AfterRestart,
}

/// How one pass ended. A censored pass ran out of its budget; its `response_us` is the
/// censoring point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PassOutcome {
    Completed,
    Censored,
    Refused,
}

/// Why a pass published nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefusalReason {
    /// The plugin declined the pass and served its input or its last applied output.
    Declined,
    /// The daemon answered an application error.
    DaemonError,
    /// The transport failed.
    TransportError,
}

/// One pass: `response_us` runs from the plugin handing the request to IPC until the
/// transformed array is back in the plugin; `service_us` is the daemon's own `total`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PassRow {
    pub harness: ScaleHarness,
    pub tier: ScaleTier,
    pub session: String,
    pub turn: u32,
    pub boundary_state: BoundaryState,
    pub outcome: PassOutcome,
    pub refusal: Option<RefusalReason>,
    pub response_us: u64,
    pub service_us: Option<u64>,
    pub rss_bytes: u64,
    pub ipc_bytes: u64,
}

/// The machine a report was measured on; figures from different CPU models are never compared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostManifest {
    pub cpu_model: String,
    pub core_count: u32,
    pub memory_bytes: u64,
    pub kernel: String,
    pub glibc: String,
    pub disk: String,
}

/// What was measured: the commit, the Bun that ran the plugin, and the daemon build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactIdentity {
    pub commit: String,
    pub bun_version: String,
    pub daemon_build: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DriverIdentity {
    pub name: String,
    pub budget_seconds: u64,
}

/// Open-loop arrival counts: offered by the schedule, sent, and completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenLoopCounts {
    pub offered: u64,
    pub sent: u64,
    pub completed: u64,
}

/// One histogram bucket: values whose [`bucket_index`] is `index`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistogramBucket {
    pub index: u32,
    pub count: u64,
}

/// A mergeable log-linear histogram: values below 32 have exact buckets, and each power of two
/// above splits into 32 buckets, so a bucket's width is at most 1/32 of its lower bound. Merge
/// adds counts bucket by bucket, so it is associative and order independent.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Histogram {
    /// Buckets with a nonzero count, by ascending index.
    pub buckets: Vec<HistogramBucket>,
    /// Censored observations; they are counted, not bucketed.
    pub censored: u64,
}

/// The bucket a value falls in.
pub fn bucket_index(value: u64) -> u32 {
    if value < HISTOGRAM_SUB_BUCKETS {
        return value as u32;
    }
    let exponent = 63 - value.leading_zeros();
    let shift = exponent - HISTOGRAM_SUB_BUCKETS.trailing_zeros();
    let sub = (value >> shift) - HISTOGRAM_SUB_BUCKETS;
    (shift + 1) * HISTOGRAM_SUB_BUCKETS as u32 + sub as u32
}

/// The largest value in bucket `index`.
pub fn bucket_upper_bound(index: u32) -> u64 {
    let width = HISTOGRAM_SUB_BUCKETS as u32;
    if index < width {
        return u64::from(index);
    }
    let shift = index / width - 1;
    let sub = u64::from(index % width);
    ((HISTOGRAM_SUB_BUCKETS + sub) << shift) + (1u64 << shift) - 1
}

impl Histogram {
    pub fn of(observations: &[(u64, bool)]) -> Self {
        let mut counts = BTreeMap::new();
        let mut censored = 0;
        for &(value, is_censored) in observations {
            if is_censored {
                censored += 1;
            } else {
                *counts.entry(bucket_index(value)).or_insert(0u64) += 1;
            }
        }
        Self::from_counts(counts, censored)
    }

    fn from_counts(counts: BTreeMap<u32, u64>, censored: u64) -> Self {
        Self {
            buckets: counts
                .into_iter()
                .filter(|(_, count)| *count > 0)
                .map(|(index, count)| HistogramBucket { index, count })
                .collect(),
            censored,
        }
    }

    pub fn merge(&self, other: &Self) -> Self {
        let mut counts = BTreeMap::new();
        for bucket in self.buckets.iter().chain(&other.buckets) {
            *counts.entry(bucket.index).or_insert(0u64) += bucket.count;
        }
        Self::from_counts(counts, self.censored + other.censored)
    }

    pub fn count(&self) -> u64 {
        self.buckets.iter().map(|bucket| bucket.count).sum::<u64>() + self.censored
    }
}

/// Exact least-squares drift of RSS over a session's steady span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flatness {
    pub steady_rows: u32,
    pub span_turns: u32,
    pub entry_rss_bytes: u64,
    pub peak_rss_bytes: u64,
    /// The OLS slope of RSS against turn times the span, truncated toward zero.
    pub drift_bytes: i64,
    /// The drift is at most a tenth of the RSS at steady-state entry.
    pub passes: bool,
}

/// The passes of one boundary state, refused passes excluded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateSummary {
    pub state: BoundaryState,
    pub passes: u32,
    pub uncensored: u32,
    pub response_us: Histogram,
    pub service_us: Histogram,
    pub response_percentiles: Vec<Percentile>,
    pub service_percentiles: Vec<Percentile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSummary {
    pub harness: ScaleHarness,
    pub tier: ScaleTier,
    pub session: String,
    pub passes: u32,
    pub refused: u32,
    /// One summary per boundary state the session's passes reached, in state order.
    pub states: Vec<StateSummary>,
    /// RSS drift over the steady passes.
    pub flatness: Option<Flatness>,
}

/// Refused passes over all passes of a tier. Zero refusals is a rule-of-three bound, never an
/// observed zero rate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "evidence_kind", deny_unknown_fields)]
pub enum RefusalRate {
    Observed {
        rate: Ratio,
        upper_bound_95: Ratio,
        bound_method: BoundMethod,
        n: u64,
    },
    Bound {
        upper_bound_95: Ratio,
        bound_method: BoundMethod,
        n: u64,
    },
}

impl RefusalRate {
    fn of(n: u64, refused: u64) -> Result<Self, StatisticsError> {
        let (rate, upper_bound_95, bound_method) = failure_bound(n, refused)?;
        Ok(match rate {
            None => Self::Bound {
                upper_bound_95,
                bound_method,
                n,
            },
            Some(rate) => Self::Observed {
                rate,
                upper_bound_95,
                bound_method,
                n,
            },
        })
    }
}

/// Why a tier carries no ratio claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "reason", deny_unknown_fields)]
pub enum RatioWithheld {
    /// Fewer than [`MIN_RATIO_SESSIONS`] sessions, or fewer than [`P99_MIN_RUNS`] uncensored
    /// steady passes in some session, in the tier or its control.
    Underpowered {
        sessions: u32,
        control_sessions: u32,
        min_steady_uncensored: u32,
    },
    /// A pooled p99, of the whole sample or of some replicate, falls on a censored pass, so it
    /// is only a lower bound and the ratio would bound nothing.
    CensoredP99 {
        sessions: u32,
        control_sessions: u32,
    },
}

/// The upper 95% bound of p99(tier) / p99(control) from a session-level block bootstrap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RatioClaim {
    pub sessions: u32,
    pub control_sessions: u32,
    pub min_steady_uncensored: u32,
    pub replicates: u32,
    pub point: Ratio,
    pub lower: Ratio,
    pub upper: Ratio,
    pub gate: Ratio,
    pub passes: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub enum TierRatio {
    /// The tier is the control.
    Control,
    Computed(RatioClaim),
    Withheld(RatioWithheld),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TierSummary {
    pub harness: ScaleHarness,
    pub tier: ScaleTier,
    pub sessions: Vec<String>,
    /// Per boundary state, the passes of every session pooled; a tier percentile is never a
    /// mean of session percentiles.
    pub states: Vec<StateSummary>,
    /// Refused passes over every pass of every state.
    pub refusals: RefusalRate,
    pub ratio: TierRatio,
}

/// What a driver hands the report: identities, the seed every bootstrap draw derives from, and
/// its rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScaleInputs {
    pub driver: DriverIdentity,
    pub artifact: ArtifactIdentity,
    pub host: HostManifest,
    pub open_loop: Option<OpenLoopCounts>,
    pub seed: u64,
    pub rows: Vec<PassRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScaleReport {
    pub schema: String,
    pub driver: DriverIdentity,
    pub artifact: ArtifactIdentity,
    pub host: HostManifest,
    pub open_loop: Option<OpenLoopCounts>,
    pub seed: u64,
    pub rows: Vec<PassRow>,
    pub sessions: Vec<SessionSummary>,
    pub tiers: Vec<TierSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScaleReportError {
    SchemaMismatch {
        found: String,
    },
    /// A required field is absent; `field` names it.
    MissingField {
        field: String,
    },
    /// `artifact.commit` is not 40 lowercase hex digits.
    MalformedCommit,
    EmptyField {
        field: &'static str,
    },
    /// `completed <= sent <= offered` does not hold.
    OpenLoopInconsistent(OpenLoopCounts),
    /// A row's refusal reason disagrees with its outcome.
    RefusalOutcomeMismatch {
        row: usize,
    },
    DuplicateTurn {
        session: String,
        turn: u32,
    },
    /// One session id appears under two harnesses or tiers.
    SessionReused {
        session: String,
    },
    /// A tier's pooled percentiles are not the percentiles of its pooled passes of each state.
    PercentileNotPooled {
        harness: ScaleHarness,
        tier: ScaleTier,
    },
    /// A ratio claim stands on fewer sessions or passes than the contract requires.
    RatioUnderpowered {
        harness: ScaleHarness,
        tier: ScaleTier,
        sessions: u32,
        min_steady_uncensored: u32,
    },
    /// Zero refusals stated other than as a rule-of-three bound.
    ZeroRefusalNotBounded {
        harness: ScaleHarness,
        tier: ScaleTier,
    },
    /// A summary differs from the one its rows derive.
    SummaryMismatch {
        section: &'static str,
    },
    Statistics(StatisticsError),
    NotCanonical(ContractError),
    Shape(String),
    Lossy,
}

impl From<StatisticsError> for ScaleReportError {
    fn from(error: StatisticsError) -> Self {
        Self::Statistics(error)
    }
}

type SessionKey = (ScaleHarness, ScaleTier, String);

/// `(response_us, censored)` of the unrefused passes in `state`.
fn observations(rows: &[&PassRow], state: BoundaryState) -> Vec<(u64, bool)> {
    rows.iter()
        .filter(|row| row.boundary_state == state && row.outcome != PassOutcome::Refused)
        .map(|row| (row.response_us, row.outcome == PassOutcome::Censored))
        .collect()
}

fn service_observations(rows: &[&PassRow], state: BoundaryState) -> Vec<(u64, bool)> {
    rows.iter()
        .filter(|row| row.boundary_state == state && row.outcome != PassOutcome::Refused)
        .filter_map(|row| row.service_us.map(|service| (service, false)))
        .collect()
}

/// One summary per state any of `rows` is in, in state order.
fn state_summaries(rows: &[&PassRow]) -> Vec<StateSummary> {
    let mut states: Vec<BoundaryState> = rows.iter().map(|row| row.boundary_state).collect();
    states.sort();
    states.dedup();
    states
        .into_iter()
        .map(|state| {
            let response = observations(rows, state);
            let service = service_observations(rows, state);
            StateSummary {
                state,
                passes: response.len() as u32,
                uncensored: uncensored(&response),
                response_us: Histogram::of(&response),
                service_us: Histogram::of(&service),
                response_percentiles: percentiles(&response),
                service_percentiles: percentiles(&service),
            }
        })
        .collect()
}

/// p50 and p95 of any non-empty sample, and p99 once it holds [`P99_MIN_RUNS`] observations.
fn percentiles(observations: &[(u64, bool)]) -> Vec<Percentile> {
    if observations.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<Percentile> = [50, 95]
        .into_iter()
        .map(|p| censored_percentile(observations, p))
        .collect();
    if observations.len() >= P99_MIN_RUNS {
        out.push(censored_percentile(observations, 99));
    }
    out
}

fn flatness(rows: &[&PassRow]) -> Result<Option<Flatness>, ScaleReportError> {
    let steady: Vec<&&PassRow> = rows
        .iter()
        .filter(|row| row.boundary_state == BoundaryState::Steady)
        .collect();
    let (Some(first), Some(last)) = (steady.first(), steady.last()) else {
        return Ok(None);
    };
    if steady.len() < 2 {
        return Ok(None);
    }
    let overflow = || ScaleReportError::Shape("flatness sums overflow".to_string());
    let product = |a: i128, b: i128| a.checked_mul(b).ok_or_else(overflow);
    let sum = |a: i128, b: i128| a.checked_add(b).ok_or_else(overflow);
    let n = steady.len() as i128;
    let (mut sx, mut sy, mut sxy, mut sxx) = (0i128, 0i128, 0i128, 0i128);
    for row in &steady {
        let (x, y) = (i128::from(row.turn), i128::from(row.rss_bytes));
        sx = sum(sx, x)?;
        sy = sum(sy, y)?;
        sxy = sum(sxy, product(x, y)?)?;
        sxx = sum(sxx, product(x, x)?)?;
    }
    let numerator = product(n, sxy)?
        .checked_sub(product(sx, sy)?)
        .ok_or_else(overflow)?;
    let denominator = product(n, sxx)?
        .checked_sub(product(sx, sx)?)
        .ok_or_else(overflow)?;
    let span = last.turn.saturating_sub(first.turn);
    let entry = first.rss_bytes;
    // `denominator` is positive whenever two steady passes have different turns.
    let (drift, passes) = if denominator <= 0 {
        (0, true)
    } else {
        let growth = product(numerator, i128::from(span))?;
        // growth / denominator <= entry / 10, compared exactly; falling RSS passes.
        (
            growth / denominator,
            product(growth, 10)? <= product(denominator, i128::from(entry))?,
        )
    };
    Ok(Some(Flatness {
        steady_rows: steady.len() as u32,
        span_turns: span,
        entry_rss_bytes: entry,
        peak_rss_bytes: steady.iter().map(|row| row.rss_bytes).max().unwrap_or(0),
        drift_bytes: i64::try_from(drift).map_err(|_| overflow())?,
        passes,
    }))
}

/// The pooled p99, or `None` when it is only a lower bound.
fn p99(observations: &[(u64, bool)]) -> Option<u64> {
    let percentile = censored_percentile(observations, 99);
    (percentile.bound == PercentileBound::Point).then_some(percentile.value)
}

type Observations = Vec<(u64, bool)>;

/// Session-level block bootstrap of p99(tier) / p99(control): each replicate draws the tier's
/// sessions and the control's sessions with replacement, pools their steady passes, and takes
/// the ratio of the pooled p99s. The interval is the `ceil(B/40)`-th and `B - floor(B/40)`-th
/// smallest of `B` replicate ratios. `None` when some pooled p99 is only a lower bound.
fn ratio_claim(
    seed: u64,
    tier: &[Observations],
    control: &[Observations],
) -> Result<Option<RatioClaim>, ScaleReportError> {
    let pooled = |sessions: &[&Observations]| -> Observations {
        sessions
            .iter()
            .flat_map(|session| session.iter().copied())
            .collect()
    };
    let ratio = |tier: &[(u64, bool)], control: &[(u64, bool)]| match (p99(tier), p99(control)) {
        (Some(tier), Some(control)) => {
            Ratio::try_new(i128::from(tier), i128::from(control)).map(Some)
        }
        _ => Ok(None),
    };
    let (kt, kc) = (tier.len() as u32, control.len() as u32);
    let draws = u64::from(RATIO_REPLICATES) * u64::from(kt + kc);
    if draws > MAX_BOOTSTRAP_DRAWS {
        return Err(StatisticsError::TooManyDraws(draws).into());
    }
    let whole_tier: Vec<&Observations> = tier.iter().collect();
    let whole_control: Vec<&Observations> = control.iter().collect();
    let Some(point) = ratio(&pooled(&whole_tier), &pooled(&whole_control))? else {
        return Ok(None);
    };
    let mut replicates = Vec::with_capacity(RATIO_REPLICATES as usize);
    for replicate in 0..RATIO_REPLICATES {
        let tier_draw: Vec<&Observations> = (0..kt)
            .map(|draw| &tier[bootstrap_draw(seed, replicate, draw, kt)])
            .collect();
        let control_draw: Vec<&Observations> = (0..kc)
            .map(|draw| &control[bootstrap_draw(seed, replicate, kt + draw, kc)])
            .collect();
        let Some(value) = ratio(&pooled(&tier_draw), &pooled(&control_draw))? else {
            return Ok(None);
        };
        replicates.push(value);
    }
    replicates.sort();
    let b = RATIO_REPLICATES as usize;
    let gate = Ratio::try_new(i128::from(RATIO_GATE.0), i128::from(RATIO_GATE.1))?;
    let upper = replicates[b - b / 40 - 1];
    Ok(Some(RatioClaim {
        sessions: kt,
        control_sessions: kc,
        min_steady_uncensored: tier
            .iter()
            .chain(control)
            .map(|session| uncensored(session))
            .min()
            .unwrap_or(0),
        replicates: RATIO_REPLICATES,
        point,
        lower: replicates[b.div_ceil(40) - 1],
        upper,
        gate,
        passes: upper <= gate,
    }))
}

fn uncensored(observations: &[(u64, bool)]) -> u32 {
    observations
        .iter()
        .filter(|(_, censored)| !censored)
        .count() as u32
}

fn check_inputs(inputs: &ScaleInputs) -> Result<(), ScaleReportError> {
    if !is_lower_hex(&inputs.artifact.commit, 40) {
        return Err(ScaleReportError::MalformedCommit);
    }
    for (field, text) in [
        ("artifact.bun_version", &inputs.artifact.bun_version),
        ("artifact.daemon_build", &inputs.artifact.daemon_build),
        ("driver.name", &inputs.driver.name),
        ("host.cpu_model", &inputs.host.cpu_model),
        ("host.kernel", &inputs.host.kernel),
        ("host.glibc", &inputs.host.glibc),
        ("host.disk", &inputs.host.disk),
    ] {
        if crate::blank(text) {
            return Err(ScaleReportError::EmptyField { field });
        }
    }
    if let Some(counts) = inputs.open_loop
        && !(counts.completed <= counts.sent && counts.sent <= counts.offered)
    {
        return Err(ScaleReportError::OpenLoopInconsistent(counts));
    }
    let mut turns = BTreeMap::<&str, (ScaleHarness, ScaleTier, Vec<u32>)>::new();
    for (index, row) in inputs.rows.iter().enumerate() {
        if crate::blank(&row.session) {
            return Err(ScaleReportError::EmptyField {
                field: "rows.session",
            });
        }
        if (row.outcome == PassOutcome::Refused) != row.refusal.is_some() {
            return Err(ScaleReportError::RefusalOutcomeMismatch { row: index });
        }
        let entry =
            turns
                .entry(row.session.as_str())
                .or_insert((row.harness, row.tier, Vec::new()));
        if (entry.0, entry.1) != (row.harness, row.tier) {
            return Err(ScaleReportError::SessionReused {
                session: row.session.clone(),
            });
        }
        if entry.2.contains(&row.turn) {
            return Err(ScaleReportError::DuplicateTurn {
                session: row.session.clone(),
                turn: row.turn,
            });
        }
        entry.2.push(row.turn);
    }
    Ok(())
}

impl ScaleReport {
    /// Derives every summary from `inputs.rows`.
    pub fn build(inputs: ScaleInputs) -> Result<Self, ScaleReportError> {
        check_inputs(&inputs)?;
        let mut by_session: BTreeMap<SessionKey, Vec<&PassRow>> = BTreeMap::new();
        for row in &inputs.rows {
            by_session
                .entry((row.harness, row.tier, row.session.clone()))
                .or_default()
                .push(row);
        }
        for rows in by_session.values_mut() {
            rows.sort_by_key(|row| row.turn);
        }
        let sessions = by_session
            .iter()
            .map(|((harness, tier, session), rows)| {
                Ok(SessionSummary {
                    harness: *harness,
                    tier: *tier,
                    session: session.clone(),
                    passes: rows.len() as u32,
                    refused: rows
                        .iter()
                        .filter(|row| row.outcome == PassOutcome::Refused)
                        .count() as u32,
                    states: state_summaries(rows),
                    flatness: flatness(rows)?,
                })
            })
            .collect::<Result<Vec<_>, ScaleReportError>>()?;

        let mut by_tier: BTreeMap<(ScaleHarness, ScaleTier), Vec<&Vec<&PassRow>>> = BTreeMap::new();
        for ((harness, tier, _), rows) in &by_session {
            by_tier.entry((*harness, *tier)).or_default().push(rows);
        }
        let session_observations = |harness: ScaleHarness, tier: ScaleTier| {
            by_tier
                .get(&(harness, tier))
                .map(|sessions| {
                    sessions
                        .iter()
                        .map(|rows| observations(rows, BoundaryState::Steady))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        let mut tiers = Vec::new();
        for ((harness, tier), session_rows) in &by_tier {
            let all: Vec<&PassRow> = session_rows
                .iter()
                .flat_map(|rows| rows.iter().copied())
                .collect();
            let refused = all
                .iter()
                .filter(|row| row.outcome == PassOutcome::Refused)
                .count() as u64;
            let refusals = RefusalRate::of(all.len() as u64, refused)?;
            let ratio = if *tier == ScaleTier::Control10k {
                TierRatio::Control
            } else {
                let observed = session_observations(*harness, *tier);
                let control = session_observations(*harness, ScaleTier::Control10k);
                let min_steady_uncensored = observed
                    .iter()
                    .chain(&control)
                    .map(|session| uncensored(session))
                    .min()
                    .unwrap_or(0);
                if observed.len() < MIN_RATIO_SESSIONS
                    || control.len() < MIN_RATIO_SESSIONS
                    || (min_steady_uncensored as usize) < P99_MIN_RUNS
                {
                    TierRatio::Withheld(RatioWithheld::Underpowered {
                        sessions: observed.len() as u32,
                        control_sessions: control.len() as u32,
                        min_steady_uncensored,
                    })
                } else {
                    match ratio_claim(inputs.seed, &observed, &control)? {
                        Some(claim) => TierRatio::Computed(claim),
                        None => TierRatio::Withheld(RatioWithheld::CensoredP99 {
                            sessions: observed.len() as u32,
                            control_sessions: control.len() as u32,
                        }),
                    }
                }
            };
            tiers.push(TierSummary {
                harness: *harness,
                tier: *tier,
                sessions: by_session
                    .keys()
                    .filter(|(h, t, _)| h == harness && t == tier)
                    .map(|(_, _, session)| session.clone())
                    .collect(),
                states: state_summaries(&all),
                refusals,
                ratio,
            });
        }
        Ok(Self {
            schema: SCALE_REPORT_SCHEMA.to_string(),
            driver: inputs.driver,
            artifact: inputs.artifact,
            host: inputs.host,
            open_loop: inputs.open_loop,
            seed: inputs.seed,
            rows: inputs.rows,
            sessions,
            tiers,
        })
    }

    fn inputs(&self) -> ScaleInputs {
        ScaleInputs {
            driver: self.driver.clone(),
            artifact: self.artifact.clone(),
            host: self.host.clone(),
            open_loop: self.open_loop,
            seed: self.seed,
            rows: self.rows.clone(),
        }
    }

    /// Refuses a report whose summaries are not the ones its rows derive, naming the first
    /// contract a summary breaks.
    pub fn validate(&self) -> Result<(), ScaleReportError> {
        if self.schema != SCALE_REPORT_SCHEMA {
            return Err(ScaleReportError::SchemaMismatch {
                found: self.schema.clone(),
            });
        }
        let expected = Self::build(self.inputs())?;
        for tier in &self.tiers {
            if let TierRatio::Computed(claim) = &tier.ratio
                && (claim.sessions < MIN_RATIO_SESSIONS as u32
                    || claim.control_sessions < MIN_RATIO_SESSIONS as u32
                    || (claim.min_steady_uncensored as usize) < P99_MIN_RUNS)
            {
                return Err(ScaleReportError::RatioUnderpowered {
                    harness: tier.harness,
                    tier: tier.tier,
                    sessions: claim.sessions,
                    min_steady_uncensored: claim.min_steady_uncensored,
                });
            }
            let zero_not_bounded = match &tier.refusals {
                RefusalRate::Observed { rate, .. } => *rate == Ratio::ZERO,
                RefusalRate::Bound { bound_method, .. } => {
                    *bound_method != BoundMethod::RuleOfThree
                }
            };
            if zero_not_bounded {
                return Err(ScaleReportError::ZeroRefusalNotBounded {
                    harness: tier.harness,
                    tier: tier.tier,
                });
            }
        }
        if self.tiers.len() != expected.tiers.len() {
            return Err(ScaleReportError::SummaryMismatch { section: "tiers" });
        }
        for (tier, derived) in self.tiers.iter().zip(&expected.tiers) {
            let pooled = |summary: &TierSummary| {
                summary
                    .states
                    .iter()
                    .map(|state| {
                        (
                            state.state,
                            state.response_percentiles.clone(),
                            state.service_percentiles.clone(),
                        )
                    })
                    .collect::<Vec<_>>()
            };
            if pooled(tier) != pooled(derived) {
                return Err(ScaleReportError::PercentileNotPooled {
                    harness: tier.harness,
                    tier: tier.tier,
                });
            }
            if tier.ratio != derived.ratio {
                return Err(ScaleReportError::SummaryMismatch {
                    section: "tiers.ratio",
                });
            }
            if tier != derived {
                return Err(ScaleReportError::SummaryMismatch { section: "tiers" });
            }
        }
        if self.sessions != expected.sessions {
            return Err(ScaleReportError::SummaryMismatch {
                section: "sessions",
            });
        }
        Ok(())
    }

    pub fn serialize(&self) -> Result<Value, ScaleReportError> {
        self.validate()?;
        let value =
            serde_json::to_value(self).map_err(|e| ScaleReportError::Shape(e.to_string()))?;
        canonical_json_encode(&value).map_err(ScaleReportError::NotCanonical)?;
        Ok(value)
    }

    /// The digest covers identities, open-loop counts, the seed, and each row's place and
    /// outcome; it excludes every latency, RSS, and byte measurement and every summary derived
    /// from them.
    pub fn result_digest(report: &Value) -> Result<String, ScaleReportError> {
        let object = report
            .as_object()
            .ok_or_else(|| ScaleReportError::Shape("report is not an object".to_string()))?;
        let mut kept = serde_json::Map::new();
        for field in ["schema", "driver", "artifact", "host", "open_loop", "seed"] {
            if let Some(value) = object.get(field) {
                kept.insert(field.to_string(), value.clone());
            }
        }
        let rows: Vec<Value> = object
            .get("rows")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .map(|row| {
                        let mut row = row.clone();
                        if let Some(row) = row.as_object_mut() {
                            for field in ROW_MEASUREMENTS {
                                row.remove(field);
                            }
                        }
                        row
                    })
                    .collect()
            })
            .unwrap_or_default();
        kept.insert("rows".to_string(), Value::Array(rows));
        protocol_digest(SCALE_RESULT_DIGEST_PROTOCOL, &Value::Object(kept))
            .map_err(|e| ScaleReportError::Shape(e.to_string()))
    }
}

/// Parses one row as the TypeScript writer emits it: every field present, nothing unknown, and
/// every value surviving a lossless round trip.
pub fn parse_pass_row(value: &Value) -> Result<PassRow, ScaleReportError> {
    let row = PassRow::deserialize(value).map_err(shape_error)?;
    canonical_json_encode(value).map_err(ScaleReportError::NotCanonical)?;
    round_trips(value, &serde_json::to_value(&row).map_err(shape_error)?)?;
    Ok(row)
}

pub fn parse_scale_report(value: &Value) -> Result<ScaleReport, ScaleReportError> {
    let report = ScaleReport::deserialize(value).map_err(shape_error)?;
    canonical_json_encode(value).map_err(ScaleReportError::NotCanonical)?;
    round_trips(value, &serde_json::to_value(&report).map_err(shape_error)?)?;
    report.validate()?;
    Ok(report)
}

/// `value` equals its re-serialization. An optional field absent from `value` but present in
/// the re-serialization is named, at any depth; any other difference is lossy.
fn round_trips(value: &Value, again: &Value) -> Result<(), ScaleReportError> {
    if value == again {
        return Ok(());
    }
    match absent_field(value, again) {
        Some(field) => Err(ScaleReportError::MissingField { field }),
        None => Err(ScaleReportError::Lossy),
    }
}

fn absent_field(value: &Value, again: &Value) -> Option<String> {
    match (value, again) {
        (Value::Object(value), Value::Object(again)) => {
            again.iter().find_map(|(key, child)| match value.get(key) {
                None => Some(key.clone()),
                Some(original) => absent_field(original, child),
            })
        }
        (Value::Array(value), Value::Array(again)) => value
            .iter()
            .zip(again)
            .find_map(|(original, child)| absent_field(original, child)),
        _ => None,
    }
}

/// `serde`'s missing-field message names the field; every other decode failure keeps its text.
fn shape_error(error: serde_json::Error) -> ScaleReportError {
    let text = error.to_string();
    if let Some(rest) = text.strip_prefix("missing field `")
        && let Some((field, _)) = rest.split_once('`')
    {
        return ScaleReportError::MissingField {
            field: field.to_string(),
        };
    }
    ScaleReportError::Shape(text)
}

debug_display!(ScaleReportError);

#[cfg(test)]
mod tests {
    use super::*;

    /// Sessions whose passes all share one value pool to a p99 equal to the largest value among
    /// the sessions drawn, so the interval is recomputable from the draws alone.
    fn constant(values: &[u64]) -> Vec<Observations> {
        values
            .iter()
            .map(|value| vec![(*value, false); 299])
            .collect()
    }

    #[test]
    fn the_ratio_interval_equals_an_independent_replay_of_the_draws() {
        let (tier_values, control_values) = ([1_100u64, 1_200, 1_300], [900u64, 1_000, 1_100]);
        let seed = 4_242;
        let claim = ratio_claim(seed, &constant(&tier_values), &constant(&control_values))
            .unwrap()
            .unwrap();
        let mut replicates: Vec<Ratio> = (0..RATIO_REPLICATES)
            .map(|replicate| {
                let tier = (0..3)
                    .map(|draw| tier_values[bootstrap_draw(seed, replicate, draw, 3)])
                    .max()
                    .unwrap();
                let control = (0..3)
                    .map(|draw| control_values[bootstrap_draw(seed, replicate, 3 + draw, 3)])
                    .max()
                    .unwrap();
                Ratio::try_new(i128::from(tier), i128::from(control)).unwrap()
            })
            .collect();
        replicates.sort();
        assert_eq!(claim.lower, replicates[24]);
        assert_eq!(claim.upper, replicates[974]);
        assert_eq!(claim.point, Ratio::try_new(1_300, 1_100).unwrap());
        assert!(claim.lower < claim.upper, "{claim:?}");
    }
}
