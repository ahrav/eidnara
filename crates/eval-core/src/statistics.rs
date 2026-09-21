//! Paired history effects with signed, separate gates and honest uncertainty.
//! Every quantity is an exact `Ratio` over oracle verdicts.
//! Censored arms resolve to the verdict least favorable to the aged arm.
//! Inference the evidence cannot support is withheld or `Blocked`.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use context_core::canonical_json::{ContractError, protocol_digest};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::manifest::{ArmRates, Manifest, is_canonical_decimal};

pub const ANALYSIS_FAMILY_SCHEMA: &str = "eval-analysis-family/v1";
const ANALYSIS_FAMILY_DIGEST_PROTOCOL: &str = "eval-analysis-family-digest/v1";
const BOOTSTRAP_PROTOCOL: &str = "eval-cluster-bootstrap/v1";
/// The smallest item count an interval may be computed from.
pub const ITEM_COUNT_THRESHOLD: u32 = 300;
/// The fewest replicates whose `1/40` tails are distinct order statistics.
pub const MIN_BOOTSTRAP_REPLICATES: u32 = 40;
/// The most replicates a family may ask for; each costs one digest per
/// cluster, so the bound keeps the resample work and its buffer finite.
pub const MAX_BOOTSTRAP_REPLICATES: u32 = 10_000;
/// The clustering level above which within-cluster correlation is treated as
/// real, so the pilot picks the highest level whose ICC exceeds `1/20`.
pub const ICC_THRESHOLD: Ratio = Ratio {
    numerator: 1,
    denominator: 20,
};
/// Canonical JSON's largest safe integer; every ratio component stays within it.
const MAX_SAFE: i128 = (1 << 53) - 1;

/// An exact rational in lowest terms with a positive denominator, both
/// components inside the canonical-JSON safe range, so two runtimes that
/// compute the same statistic serialize the same bytes. Construction and
/// deserialization normalize; arithmetic is checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RatioWire", deny_unknown_fields)]
pub struct Ratio {
    pub numerator: i64,
    pub denominator: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RatioWire {
    numerator: i64,
    denominator: u64,
}

impl TryFrom<RatioWire> for Ratio {
    type Error = StatisticsError;

    fn try_from(wire: RatioWire) -> Result<Self, StatisticsError> {
        Ratio::try_new(i128::from(wire.numerator), i128::from(wire.denominator))
    }
}

impl Ratio {
    pub const ZERO: Ratio = Ratio {
        numerator: 0,
        denominator: 1,
    };
    pub const ONE: Ratio = Ratio {
        numerator: 1,
        denominator: 1,
    };

    /// Reduces `numerator / denominator`; a zero denominator or a component
    /// outside the safe range is a typed refusal, never a wrapped value.
    pub fn try_new(numerator: i128, denominator: i128) -> Result<Self, StatisticsError> {
        if denominator == 0 {
            return Err(StatisticsError::ZeroDenominator);
        }
        let (numerator, denominator) = if denominator < 0 {
            (-numerator, -denominator)
        } else {
            (numerator, denominator)
        };
        let divisor = i128::try_from(gcd(numerator.unsigned_abs(), denominator.unsigned_abs()))
            .map_err(|_| StatisticsError::RationalOverflow)?;
        let (numerator, denominator) = (numerator / divisor, denominator / divisor);
        if numerator.abs() > MAX_SAFE || denominator > MAX_SAFE {
            return Err(StatisticsError::RationalOverflow);
        }
        Ok(Self {
            numerator: numerator as i64,
            denominator: denominator as u64,
        })
    }

    /// `new` for small literals that cannot overflow.
    pub fn new(numerator: i64, denominator: u64) -> Self {
        Self::try_new(i128::from(numerator), i128::from(denominator)).expect("small literal")
    }

    /// Parses a canonical decimal (`0`, `12`, `0.25`) exactly.
    pub fn from_decimal(text: &str) -> Option<Self> {
        if !is_canonical_decimal(text) {
            return None;
        }
        let (integer, fraction) = text.split_once('.').unwrap_or((text, ""));
        let denominator = 10i128.checked_pow(u32::try_from(fraction.len()).ok()?)?;
        let numerator = format!("{integer}{fraction}").parse::<i128>().ok()?;
        Self::try_new(numerator, denominator).ok()
    }

    fn parts(self) -> (i128, i128) {
        (i128::from(self.numerator), i128::from(self.denominator))
    }

    pub fn checked_add(self, other: Self) -> Result<Self, StatisticsError> {
        let ((a, b), (c, d)) = (self.parts(), other.parts());
        Self::try_new(a * d + c * b, b * d)
    }

    pub fn checked_sub(self, other: Self) -> Result<Self, StatisticsError> {
        let ((a, b), (c, d)) = (self.parts(), other.parts());
        Self::try_new(a * d - c * b, b * d)
    }

    pub fn checked_mul(self, other: Self) -> Result<Self, StatisticsError> {
        let ((a, b), (c, d)) = (self.parts(), other.parts());
        Self::try_new(a * c, b * d)
    }

    pub fn checked_div(self, other: Self) -> Result<Self, StatisticsError> {
        let ((a, b), (c, d)) = (self.parts(), other.parts());
        Self::try_new(a * d, b * c)
    }
}

impl Ord for Ratio {
    fn cmp(&self, other: &Self) -> Ordering {
        let ((a, b), (c, d)) = (self.parts(), other.parts());
        (a * d).cmp(&(c * b))
    }
}

impl PartialOrd for Ratio {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// The maintainer's pre-registered settings. Every field is required and none
/// is ever derived from the outcomes it gates; the values are experimental
/// margins, not product targets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CampaignProfile {
    pub noninferiority_margin: String,
    pub harm_bound: String,
    pub floor_threshold: String,
    pub miss_asymmetry_bound: String,
    pub liveness_bounds: LivenessBounds,
}

/// Bounded progress per subsystem in its own unit, never wall clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LivenessBounds {
    pub catch_up_episodes: u64,
    pub embedding_passes: u64,
    pub materialization_episodes: u64,
    pub reviewer_coordinator_passes: u64,
}

/// The profile's four rates parsed once. Canonical decimals carry no sign, so
/// `[0, 1]` needs only the upper check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileRates {
    pub noninferiority_margin: Ratio,
    pub harm_bound: Ratio,
    pub floor_threshold: Ratio,
    pub miss_asymmetry_bound: Ratio,
}

impl CampaignProfile {
    pub fn rates(&self) -> Result<ProfileRates, StatisticsError> {
        let rate = |field: &'static str, text: &str| {
            let ratio =
                Ratio::from_decimal(text).ok_or(StatisticsError::MalformedDecimal { field })?;
            if ratio > Ratio::ONE {
                return Err(StatisticsError::RateOutOfRange { field });
            }
            Ok(ratio)
        };
        Ok(ProfileRates {
            noninferiority_margin: rate("noninferiority_margin", &self.noninferiority_margin)?,
            harm_bound: rate("harm_bound", &self.harm_bound)?,
            floor_threshold: rate("floor_threshold", &self.floor_threshold)?,
            miss_asymmetry_bound: rate("miss_asymmetry_bound", &self.miss_asymmetry_bound)?,
        })
    }
}

pub fn parse_campaign_profile(value: &Value) -> Result<CampaignProfile, StatisticsError> {
    let profile: CampaignProfile = serde_json::from_value(value.clone())
        .map_err(|error| StatisticsError::Shape(error.to_string()))?;
    profile.rates()?;
    Ok(profile)
}

/// The cluster a pair or pilot observation belongs to: its task family, or the
/// world (one family and one seed) it was drawn from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClusteringUnit {
    WorldSeed,
    Family,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntervalMethod {
    ClusterBootstrap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MultiplicityCorrection {
    None,
    Holm,
    BenjaminiHochberg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoppingRule {
    /// The pair count is fixed before the first outcome.
    FixedN,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClusterKey {
    pub family: String,
    pub world_seed: u64,
}

impl ClusterKey {
    /// The cluster this key falls in at `unit`. The world-seed unit is the
    /// world itself (family and seed), so the pilot's ICC and the bootstrap
    /// resample the same partition.
    fn at(&self, unit: ClusteringUnit) -> ClusterKey {
        match unit {
            ClusteringUnit::Family => ClusterKey {
                family: self.family.clone(),
                world_seed: 0,
            },
            ClusteringUnit::WorldSeed => self.clone(),
        }
    }
}

/// One pilot observation: a paired score for one task in one world.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PilotObservation {
    pub cluster: ClusterKey,
    pub task: String,
    pub value: i64,
}

/// One-way ANOVA intraclass correlation over groups of equal or unequal size:
/// `(MSB - MSW) / (MSB + (m0 - 1) MSW)` with `m0` the arithmetic mean group
/// size, exact. Fewer than two groups, or no replication anywhere, is
/// `PilotTooSmall`; groups that are each internally constant give exactly one.
pub fn intraclass_correlation(groups: &[Vec<i64>]) -> Result<Ratio, StatisticsError> {
    let k = groups.len() as i128;
    let n: i128 = groups.iter().map(|g| g.len() as i128).sum();
    if k < 2 || n <= k {
        return Err(StatisticsError::PilotTooSmall);
    }
    let mean = |values: &[i64]| {
        Ratio::try_new(
            values.iter().map(|v| i128::from(*v)).sum(),
            values.len() as i128,
        )
    };
    let grand_mean = mean(&groups.iter().flatten().copied().collect::<Vec<_>>())?;
    let (mut ss_between, mut ss_within) = (Ratio::ZERO, Ratio::ZERO);
    for group in groups {
        let group_mean = mean(group)?;
        let between = group_mean.checked_sub(grand_mean)?;
        let weight = Ratio::try_new(group.len() as i128, 1)?;
        ss_between = ss_between.checked_add(weight.checked_mul(between.checked_mul(between)?)?)?;
        for value in group {
            let within = Ratio::try_new(i128::from(*value), 1)?.checked_sub(group_mean)?;
            ss_within = ss_within.checked_add(within.checked_mul(within)?)?;
        }
    }
    let msb = ss_between.checked_div(Ratio::try_new(k - 1, 1)?)?;
    let msw = ss_within.checked_div(Ratio::try_new(n - k, 1)?)?;
    let m0_minus_1 = Ratio::try_new(n - k, k)?;
    let denominator = msb.checked_add(m0_minus_1.checked_mul(msw)?)?;
    if denominator == Ratio::ZERO {
        return Err(StatisticsError::PilotTooSmall);
    }
    msb.checked_sub(msw)?.checked_div(denominator)
}

/// The pilot's evidence: ICC at each candidate clustering level, the level it
/// selects, and whether the affordable world count supports the margin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IccPilot {
    pub pilot_run_id: String,
    pub n_items: u32,
    pub n_families: u32,
    pub n_worlds: u32,
    pub icc_family: Ratio,
    pub icc_world_seed: Ratio,
    pub clustering_unit: ClusteringUnit,
    pub max_affordable_worlds: u32,
    /// Items at the affordable world count deflated by the design effect
    /// `1 + (m - 1) ICC` of the selected unit; the effect is never below one,
    /// so deflation only ever shrinks N.
    pub effective_n_at_max: Ratio,
    pub required_n_for_margin: u32,
}

fn grouped(observations: &[PilotObservation], unit: ClusteringUnit) -> Vec<Vec<i64>> {
    let mut groups: BTreeMap<ClusterKey, Vec<i64>> = BTreeMap::new();
    for observation in observations {
        groups
            .entry(observation.cluster.at(unit))
            .or_default()
            .push(observation.value);
    }
    groups.into_values().collect()
}

/// Runs the pilot. The unit is the highest level whose ICC exceeds
/// [`ICC_THRESHOLD`], with the world as the finest fallback.
pub fn run_icc_pilot(
    pilot_run_id: &str,
    observations: &[PilotObservation],
    max_affordable_worlds: u32,
    required_n_for_margin: u32,
) -> Result<IccPilot, StatisticsError> {
    if max_affordable_worlds == 0 {
        return Err(StatisticsError::NoAffordableWorlds);
    }
    let by_family = grouped(observations, ClusteringUnit::Family);
    let by_world = grouped(observations, ClusteringUnit::WorldSeed);
    let icc_family = intraclass_correlation(&by_family)?;
    let icc_world_seed = intraclass_correlation(&by_world)?;
    let (clustering_unit, icc, clusters_at_max) = if icc_family > ICC_THRESHOLD {
        (ClusteringUnit::Family, icc_family, by_family.len() as i128)
    } else {
        (
            ClusteringUnit::WorldSeed,
            icc_world_seed,
            i128::from(max_affordable_worlds),
        )
    };
    let n_items = observations.len() as i128;
    let items_at_max = Ratio::try_new(
        n_items * i128::from(max_affordable_worlds),
        by_world.len() as i128,
    )?;
    let mean_cluster = items_at_max.checked_div(Ratio::try_new(clusters_at_max, 1)?)?;
    let design_effect = Ratio::ONE
        .checked_add(
            mean_cluster
                .checked_sub(Ratio::ONE)?
                .checked_mul(icc.max(Ratio::ZERO))?,
        )?
        .max(Ratio::ONE);
    Ok(IccPilot {
        pilot_run_id: pilot_run_id.to_string(),
        n_items: n_items as u32,
        n_families: by_family.len() as u32,
        n_worlds: by_world.len() as u32,
        icc_family,
        icc_world_seed,
        clustering_unit,
        max_affordable_worlds,
        effective_n_at_max: items_at_max.checked_div(design_effect)?,
        required_n_for_margin,
    })
}

/// The frozen analysis family: everything a result depends on, fixed before
/// the first outcome and digested so a later edit is detectable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalysisFamily {
    pub schema: String,
    pub endpoints: Vec<String>,
    pub families: Vec<String>,
    pub exclusions: Vec<String>,
    pub stopping_rule: StoppingRule,
    pub multiplicity_correction: MultiplicityCorrection,
    pub profile: CampaignProfile,
    pub interval_method: IntervalMethod,
    pub item_count_threshold: u32,
    pub bootstrap_replicates: u32,
    pub bootstrap_seed: u64,
    /// The repeat count `k` every live trial's pass^k is read at.
    pub trials_k: u32,
    pub icc_pilot: IccPilot,
}

impl AnalysisFamily {
    pub fn validate(&self) -> Result<(), StatisticsError> {
        if self.schema != ANALYSIS_FAMILY_SCHEMA {
            return Err(StatisticsError::SchemaMismatch {
                found: self.schema.clone(),
            });
        }
        if self.item_count_threshold < ITEM_COUNT_THRESHOLD {
            return Err(StatisticsError::ItemCountThresholdBelowFloor(
                self.item_count_threshold,
            ));
        }
        if self.bootstrap_replicates < MIN_BOOTSTRAP_REPLICATES {
            return Err(StatisticsError::TooFewReplicates(self.bootstrap_replicates));
        }
        if self.bootstrap_replicates > MAX_BOOTSTRAP_REPLICATES {
            return Err(StatisticsError::TooManyReplicates(
                self.bootstrap_replicates,
            ));
        }
        if self.endpoints.is_empty() || self.families.is_empty() || self.trials_k == 0 {
            return Err(StatisticsError::EmptyFamilyField);
        }
        self.profile.rates()?;
        Ok(())
    }

    pub fn digest(&self) -> Result<String, StatisticsError> {
        self.validate()?;
        Ok(protocol_digest(
            ANALYSIS_FAMILY_DIGEST_PROTOCOL,
            &serde_json::to_value(self).expect("serializes"),
        )?)
    }

    pub fn is_blocked(&self) -> Option<BlockedReason> {
        let pilot = &self.icc_pilot;
        let required = Ratio::new(i64::from(pilot.required_n_for_margin), 1);
        (pilot.effective_n_at_max < required).then_some(BlockedReason::InsufficientEffectiveN {
            effective_n_at_max: pilot.effective_n_at_max,
            required_n_for_margin: pilot.required_n_for_margin,
        })
    }
}

pub fn parse_analysis_family(value: &Value) -> Result<AnalysisFamily, StatisticsError> {
    let family: AnalysisFamily = serde_json::from_value(value.clone())
        .map_err(|error| StatisticsError::Shape(error.to_string()))?;
    family.validate()?;
    Ok(family)
}

/// The digest the manifest recorded before the first outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenFamily {
    pub analysis_family_digest: String,
}

impl FrozenFamily {
    pub fn freeze(family: &AnalysisFamily) -> Result<Self, StatisticsError> {
        Ok(Self {
            analysis_family_digest: family.digest()?,
        })
    }

    /// The digest a manifest recorded; a manifest without one supports no
    /// paired report.
    pub fn from_manifest(manifest: &Manifest) -> Result<Self, StatisticsError> {
        manifest
            .analysis_family_digest
            .clone()
            .map(|analysis_family_digest| Self {
                analysis_family_digest,
            })
            .ok_or(StatisticsError::FamilyNotRecorded)
    }

    /// Refuses a family whose digest differs from the frozen one.
    pub fn check(&self, family: &AnalysisFamily) -> Result<(), StatisticsError> {
        let found = family.digest()?;
        if found == self.analysis_family_digest {
            Ok(())
        } else {
            Err(StatisticsError::FamilyChangedAfterResults {
                recorded: self.analysis_family_digest.clone(),
                found,
            })
        }
    }
}

/// Why an attempt ended without a verdict: the timeout or one of the six
/// per-task budgets. Every one is a right-censored observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CensorReason {
    Timeout,
    MaxModelCalls,
    MaxToolCalls,
    MaxTokensIn,
    MaxTokensOut,
    HardDeadlineMs,
    MaxNoProgressIterations,
}

/// One arm's oracle verdict. Only an oracle produces one; a model-written
/// assessment never converts into this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArmResult {
    Pass,
    Fail,
    Censored(CensorReason),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairOutcome {
    pub pair_id: String,
    pub cluster: ClusterKey,
    pub fresh: ArmResult,
    pub aged: ArmResult,
}

/// Censored arms count against the aged arm: `b` includes censored fresh
/// arms; `c` and `aged_pass` exclude censored aged arms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairCounts {
    pub n: u64,
    pub b: u64,
    pub c: u64,
    pub aged_pass: u64,
    pub fresh_censored: u64,
    pub aged_censored: u64,
}

impl PairCounts {
    pub fn of(pairs: &[PairOutcome]) -> Self {
        let mut counts = Self::default();
        for pair in pairs {
            counts.n += 1;
            counts.fresh_censored += u64::from(matches!(pair.fresh, ArmResult::Censored(_)));
            counts.aged_censored += u64::from(matches!(pair.aged, ArmResult::Censored(_)));
            counts.aged_pass += u64::from(pair.aged == ArmResult::Pass);
            counts.b += u64::from(pair.fresh != ArmResult::Fail && pair.aged != ArmResult::Pass);
            counts.c += u64::from(pair.fresh == ArmResult::Fail && pair.aged == ArmResult::Pass);
        }
        counts
    }

    fn absorb(&mut self, other: Self) {
        self.n += other.n;
        self.b += other.b;
        self.c += other.c;
        self.aged_pass += other.aged_pass;
        self.fresh_censored += other.fresh_censored;
        self.aged_censored += other.aged_censored;
    }

    fn over_n(&self, numerator: i64) -> Ratio {
        Ratio::new(numerator, self.n.max(1))
    }

    /// `(b - c) / n`, signed: negative means the aged arm did better.
    pub fn quality_loss(&self) -> Ratio {
        self.over_n(self.b as i64 - self.c as i64)
    }

    pub fn harm(&self) -> Ratio {
        self.over_n(self.b as i64)
    }

    pub fn aged_pass_rate(&self) -> Ratio {
        self.over_n(self.aged_pass as i64)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateVerdict {
    pub statistic: Ratio,
    pub bound: Ratio,
    pub passed: bool,
}

/// The three separate gates; no collapsing scalar exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gates {
    pub quality_loss: GateVerdict,
    pub harm: GateVerdict,
    pub floor: GateVerdict,
}

impl Gates {
    pub fn of(counts: &PairCounts, rates: &ProfileRates) -> Result<Self, StatisticsError> {
        if counts.n == 0 {
            return Err(StatisticsError::NoPairs);
        }
        let (quality_loss, harm, floor) = (
            counts.quality_loss(),
            counts.harm(),
            counts.aged_pass_rate(),
        );
        Ok(Self {
            quality_loss: GateVerdict {
                statistic: quality_loss,
                bound: rates.noninferiority_margin,
                passed: quality_loss <= rates.noninferiority_margin,
            },
            harm: GateVerdict {
                statistic: harm,
                bound: rates.harm_bound,
                passed: harm <= rates.harm_bound,
            },
            floor: GateVerdict {
                statistic: floor,
                bound: rates.floor_threshold,
                passed: floor >= rates.floor_threshold,
            },
        })
    }
}

/// A world-clustered interval for `quality_loss`; every field the reader
/// needs to weigh it travels with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Interval {
    pub unit: ClusteringUnit,
    pub method: IntervalMethod,
    pub n_clusters: u32,
    pub n_items: u32,
    pub replicates: u32,
    pub lower: Ratio,
    pub upper: Ratio,
}

/// The interval, or the reason none is emitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome", deny_unknown_fields)]
pub enum IntervalOutcome {
    Computed(Interval),
    Withheld { reason: IntervalWithheld },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub enum IntervalWithheld {
    ItemCountBelowThreshold { n_items: u32, threshold: u32 },
    FewerThanTwoClusters { n_clusters: u32 },
}

/// The bootstrap draw for replicate `replicate`, position `draw`: the first
/// 64 bits of the protocol digest over the key, reduced by the cluster count.
fn bootstrap_draw(seed: u64, replicate: u32, draw: u32, clusters: u32) -> usize {
    let key = json!({"seed": seed.to_string(), "replicate": replicate, "draw": draw});
    let hex = protocol_digest(BOOTSTRAP_PROTOCOL, &key).expect("draw key is canonical");
    let word = u64::from_str_radix(&hex[..16], 16).expect("digest is lowercase hex");
    (word % u64::from(clusters)) as usize
}

/// Percentile cluster bootstrap of `quality_loss`: clusters are resampled with
/// replacement, the statistic is the ratio of resampled sums, and the interval
/// is the `1/40` and `39/40` order statistics of the replicates. The threshold
/// never goes below [`ITEM_COUNT_THRESHOLD`] and the replicate count never
/// below [`MIN_BOOTSTRAP_REPLICATES`], whatever the caller asks.
pub fn cluster_bootstrap_interval(
    pairs: &[PairOutcome],
    unit: ClusteringUnit,
    threshold: u32,
    seed: u64,
    replicates: u32,
) -> Result<IntervalOutcome, StatisticsError> {
    let threshold = threshold.max(ITEM_COUNT_THRESHOLD);
    if replicates < MIN_BOOTSTRAP_REPLICATES {
        return Err(StatisticsError::TooFewReplicates(replicates));
    }
    if replicates > MAX_BOOTSTRAP_REPLICATES {
        return Err(StatisticsError::TooManyReplicates(replicates));
    }
    let n_items = pairs.len() as u32;
    if n_items < threshold {
        return Ok(IntervalOutcome::Withheld {
            reason: IntervalWithheld::ItemCountBelowThreshold { n_items, threshold },
        });
    }
    // Clusters are ordered by key, so the draw index names one cluster across runtimes.
    let mut cells: BTreeMap<ClusterKey, PairCounts> = BTreeMap::new();
    for pair in pairs {
        cells
            .entry(pair.cluster.at(unit))
            .or_default()
            .absorb(PairCounts::of(std::slice::from_ref(pair)));
    }
    let cells: Vec<PairCounts> = cells.into_values().collect();
    let n_clusters = cells.len() as u32;
    if n_clusters < 2 {
        return Ok(IntervalOutcome::Withheld {
            reason: IntervalWithheld::FewerThanTwoClusters { n_clusters },
        });
    }
    let mut statistics = Vec::with_capacity(replicates as usize);
    for replicate in 0..replicates {
        let mut resample = PairCounts::default();
        for draw in 0..n_clusters {
            resample.absorb(cells[bootstrap_draw(seed, replicate, draw, n_clusters)]);
        }
        statistics.push(resample.quality_loss());
    }
    statistics.sort();
    let tail = (replicates / 40) as usize;
    Ok(IntervalOutcome::Computed(Interval {
        unit,
        method: IntervalMethod::ClusterBootstrap,
        n_clusters,
        n_items,
        replicates,
        lower: statistics[tail],
        upper: statistics[replicates as usize - tail - 1],
    }))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "reason", deny_unknown_fields)]
pub enum BlockedReason {
    InsufficientEffectiveN {
        effective_n_at_max: Ratio,
        required_n_for_margin: u32,
    },
    ArmMissAsymmetry {
        asymmetry: Ratio,
        bound: Ratio,
    },
}

/// The paired report: gates and the interval under the frozen family, with the
/// per-arm miss and refusal rates that qualify them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedReport {
    pub analysis_family_digest: String,
    pub counts: PairCounts,
    pub gates: Gates,
    pub interval: IntervalOutcome,
    pub arm_rates: BTreeMap<String, ArmRates>,
}

/// A campaign's analysis: a report, or a typed block with no gates computed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status", deny_unknown_fields)]
pub enum Analysis {
    Report(Box<PairedReport>),
    Blocked(BlockedReason),
}

/// The gap between the highest and lowest arm cassette-miss rate. Fewer than
/// two arms is missing evidence, which never passes a control.
pub fn arm_miss_asymmetry(
    arm_rates: &BTreeMap<String, ArmRates>,
) -> Result<Ratio, StatisticsError> {
    let rates: Vec<Ratio> = arm_rates
        .values()
        .map(|rates| {
            Ratio::from_decimal(&rates.miss_rate).ok_or(StatisticsError::MalformedDecimal {
                field: "arm_rates.miss_rate",
            })
        })
        .collect::<Result<_, _>>()?;
    match (rates.iter().max(), rates.iter().min()) {
        (Some(high), Some(low)) if rates.len() >= 2 => high.checked_sub(*low),
        _ => Err(StatisticsError::TooFewArms(rates.len())),
    }
}

/// Analyzes a completed pair table under the frozen family. The order is the
/// contract: the freeze check, then the pilot's block, then the arm-miss
/// asymmetry block, and only then the gates, so a blocked campaign computes
/// none.
pub fn analyze(
    frozen: &FrozenFamily,
    family: &AnalysisFamily,
    pairs: &[PairOutcome],
    arm_rates: &BTreeMap<String, ArmRates>,
) -> Result<Analysis, StatisticsError> {
    frozen.check(family)?;
    if let Some(blocked) = family.is_blocked() {
        return Ok(Analysis::Blocked(blocked));
    }
    let rates = family.profile.rates()?;
    let asymmetry = arm_miss_asymmetry(arm_rates)?;
    if asymmetry > rates.miss_asymmetry_bound {
        return Ok(Analysis::Blocked(BlockedReason::ArmMissAsymmetry {
            asymmetry,
            bound: rates.miss_asymmetry_bound,
        }));
    }
    let counts = PairCounts::of(pairs);
    Ok(Analysis::Report(Box::new(PairedReport {
        analysis_family_digest: frozen.analysis_family_digest.clone(),
        gates: Gates::of(&counts, &rates)?,
        counts,
        interval: cluster_bootstrap_interval(
            pairs,
            family.icc_pilot.clustering_unit,
            family.item_count_threshold,
            family.bootstrap_seed,
            family.bootstrap_replicates,
        )?,
        arm_rates: arm_rates.clone(),
    })))
}

/// `FamilyChangedAfterResults` is the post-hoc edit refusal;
/// `RationalOverflow` means a statistic left the canonical-JSON safe range and
/// is refused rather than wrapped; `PilotTooSmall` means a clustering level
/// had fewer than two groups or no replication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatisticsError {
    Shape(String),
    SchemaMismatch { found: String },
    MalformedDecimal { field: &'static str },
    RateOutOfRange { field: &'static str },
    ItemCountThresholdBelowFloor(u32),
    TooFewReplicates(u32),
    TooManyReplicates(u32),
    EmptyFamilyField,
    FamilyChangedAfterResults { recorded: String, found: String },
    FamilyNotRecorded,
    PilotTooSmall,
    NoAffordableWorlds,
    NoPairs,
    TooFewArms(usize),
    MalformedCounter { n: u64, failures: u64 },
    MalformedTrials { k: u32, repeats: u32 },
    ZeroDenominator,
    RationalOverflow,
    NotCanonical(ContractError),
}

debug_display!(StatisticsError);

impl From<ContractError> for StatisticsError {
    fn from(error: ContractError) -> Self {
        Self::NotCanonical(error)
    }
}
