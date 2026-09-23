//! Residual judging and the held-out live slice. A judge scores only what
//! the deterministic oracles left open, blinded and order-swapped against a
//! frozen human-anchor calibration set, versioned as a dependency; its
//! output is a typed residual that no control, floor, gate, or injection
//! axis can accept. Live-model runs are reported as trials with pass^k
//! intervals and are never replayable.

use std::collections::{BTreeMap, BTreeSet};

use context_core::canonical_json::protocol_digest;
use serde::{Deserialize, Serialize};

use crate::anchor::ProviderProfile;
use crate::censoring::{PassK, PassKBounds, pass_k};
use crate::statistics::{ArmResult, StatisticsError};

pub const JUDGE_SCHEMA: &str = "eval-judge/v1";
pub const RESIDUAL_REPORT_SCHEMA: &str = "eval-residual-report/v1";
pub const LIVE_SLICE_SCHEMA: &str = "eval-live-slice/v1";
pub const CALIBRATION_DIGEST_PROTOCOL: &str = "eval-judge-calibration/v1";
pub const RUBRIC_DIGEST_PROTOCOL: &str = "eval-judge-rubric/v1";
/// Human review covers at least this share of judged pairs, and never fewer
/// pairs than the minimum.
pub const HUMAN_SAMPLE_FLOOR_PERCENT: u32 = 10;
pub const HUMAN_SAMPLE_MIN_PAIRS: u32 = 20;
/// Maximum blinded-trial arm-identification rate, correct or inverted.
pub const ARM_IDENTIFICATION_CEILING_PERCENT: u32 = 60;
/// Live runs are trials of a nondeterministic model; nothing replays them.
pub const LIVE_REPLAYABLE: bool = false;

/// The judge as a versioned dependency: who it is and what it was told.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JudgeIdentity {
    pub provider: ProviderProfile,
    pub prompt_digest: String,
    pub rubric_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rubric {
    pub schema: String,
    pub criteria: Vec<String>,
}

impl Rubric {
    pub fn digest(&self) -> String {
        let value = serde_json::to_value(self).expect("rubric serializes");
        protocol_digest(RUBRIC_DIGEST_PROTOCOL, &value).expect("rubric is canonical")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preference {
    A,
    B,
    Tie,
    /// The two presentation orders disagreed: the judge followed position.
    Inconsistent,
}

/// Human labels over anchor pairs, frozen with the judge and rubric before
/// any judging; re-scored whenever any identity changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalibrationSet {
    pub schema: String,
    pub judge: JudgeIdentity,
    pub human_labels: BTreeMap<String, Preference>,
}

impl CalibrationSet {
    pub fn digest(&self) -> String {
        let value = serde_json::to_value(self).expect("calibration serializes");
        protocol_digest(CALIBRATION_DIGEST_PROTOCOL, &value).expect("calibration is canonical")
    }

    pub fn validate(&self) -> Result<(), CalibrationRefused> {
        if self.human_labels.is_empty() {
            return Err(CalibrationRefused::EmptyCalibrationSet);
        }
        Ok(())
    }
}

/// How many pairs are judged and how many humans review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SamplingPlan {
    pub pairs: u32,
    pub human_sample: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalibrationRefused {
    /// Fewer pairs than the minimum human sample: no calibrated acceptance.
    TooFewPairs {
        pairs: u32,
        minimum: u32,
    },
    HumanSampleBelowFloor {
        required: u32,
        planned: u32,
    },
    EmptyCalibrationSet,
    /// The calibration set was frozen with another judge, so it anchors
    /// nothing this judge scored.
    CalibrationJudgeDiffers,
    /// The recorded digest is not the digest of the calibration set supplied.
    DigestMismatch,
}

debug_display!(CalibrationRefused);

impl SamplingPlan {
    /// The required human sample: ten percent of the pairs, rounded up, and
    /// never fewer than `HUMAN_SAMPLE_MIN_PAIRS`.
    pub fn required_human_sample(pairs: u32) -> u32 {
        pairs
            .div_ceil(100 / HUMAN_SAMPLE_FLOOR_PERCENT)
            .max(HUMAN_SAMPLE_MIN_PAIRS)
    }

    pub fn validate(&self) -> Result<(), CalibrationRefused> {
        if self.pairs < HUMAN_SAMPLE_MIN_PAIRS {
            return Err(CalibrationRefused::TooFewPairs {
                pairs: self.pairs,
                minimum: HUMAN_SAMPLE_MIN_PAIRS,
            });
        }
        let required = Self::required_human_sample(self.pairs);
        if self.human_sample < required {
            return Err(CalibrationRefused::HumanSampleBelowFloor {
                required,
                planned: self.human_sample,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Order {
    AThenB,
    BThenA,
}

impl Order {
    pub const BOTH: [Self; 2] = [Self::AThenB, Self::BThenA];
}

/// The two responses of one pair, with the arm each came from known only to
/// the runner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pair {
    pub id: String,
    pub a: String,
    pub b: String,
}

/// The judge sees only the serialized form, which omits `pair` and `order`;
/// the struct retains them to identify the pair and map positions back to A
/// and B.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BlindedPrompt {
    #[serde(skip)]
    pub pair: String,
    #[serde(skip)]
    pub order: Order,
    pub first: String,
    pub second: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlindingRefused {
    /// A planted canary would reach the judge.
    CanaryInPrompt { pair: String, canary: String },
    /// A response names its own arm.
    ArmIdentifiable { pair: String, token: String },
}

debug_display!(BlindingRefused);

/// Word sequences that name an arm, in the form `fold_words` produces.
pub const ARM_TOKENS: [&str; 6] = [
    "aged arm",
    "fresh arm",
    "arm a",
    "arm b",
    "baseline arm",
    "control arm",
];

/// Lowercase words joined by single spaces, padded with one space at each
/// end. Full-width ASCII folds to ASCII and every non-alphanumeric character
/// separates words, so `fresh-arm` and `fresh\narm` both read `fresh arm`.
fn fold_words(text: &str) -> String {
    let mut folded = String::with_capacity(text.len() + 2);
    folded.push(' ');
    for c in text.chars() {
        let c = match c {
            '\u{ff01}'..='\u{ff5e}' => char::from_u32(u32::from(c) - 0xfee0).unwrap_or(c),
            _ => c,
        };
        if c.is_alphanumeric() {
            folded.extend(c.to_lowercase());
        } else if !folded.ends_with(' ') {
            folded.push(' ');
        }
    }
    if !folded.ends_with(' ') {
        folded.push(' ');
    }
    folded
}

/// Whole-word match: `harm bound` does not contain `arm b`. The leading pad
/// keeps every match index at one or more.
fn contains_words(folded: &str, token: &str) -> bool {
    let bytes = folded.as_bytes();
    folded
        .match_indices(token)
        .any(|(at, _)| bytes[at - 1] == b' ' && bytes.get(at + token.len()) == Some(&b' '))
}

fn screen(pair: &Pair, canaries: &[String]) -> Result<(), BlindingRefused> {
    for text in [&pair.a, &pair.b] {
        if let Some(canary) = canaries
            .iter()
            .find(|c| !c.is_empty() && text.contains(c.as_str()))
        {
            return Err(BlindingRefused::CanaryInPrompt {
                pair: pair.id.clone(),
                canary: canary.clone(),
            });
        }
        let folded = fold_words(text);
        if let Some(token) = ARM_TOKENS.iter().find(|t| contains_words(&folded, t)) {
            return Err(BlindingRefused::ArmIdentifiable {
                pair: pair.id.clone(),
                token: (*token).to_string(),
            });
        }
    }
    Ok(())
}

/// Blinds one pair in one order, refusing a canary or an arm name.
pub fn blind(
    pair: &Pair,
    order: Order,
    canaries: &[String],
) -> Result<BlindedPrompt, BlindingRefused> {
    screen(pair, canaries)?;
    let (first, second) = match order {
        Order::AThenB => (pair.a.clone(), pair.b.clone()),
        Order::BThenA => (pair.b.clone(), pair.a.clone()),
    };
    Ok(BlindedPrompt {
        pair: pair.id.clone(),
        order,
        first,
        second,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawVerdict {
    First,
    Second,
    Tie,
}

/// One judge call as executed: the identity that answered, the order shown,
/// and the raw positional verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JudgeCall {
    pub pair: String,
    pub judge: JudgeIdentity,
    pub order: Order,
    pub verdict: RawVerdict,
}

/// The judgment of one pair after both orders were executed and unswapped,
/// with the per-arm lengths that expose a length leak.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairJudgment {
    pub pair: String,
    pub preference: Preference,
    pub length_a: u64,
    pub length_b: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JudgeRefused {
    /// One order was never executed; a single presentation is not a judgment.
    OrderMissing {
        pair: String,
        order: Order,
    },
    JudgeDiffers {
        pair: String,
    },
    UnknownPair {
        pair: String,
    },
    /// Two pairs share an id, so one pair's calls would judge both.
    DuplicatePair {
        pair: String,
    },
    /// Keeping either of two calls for one pair and order hides the other
    /// verdict.
    DuplicateCall {
        pair: String,
        order: Order,
    },
    Blinding(BlindingRefused),
}

debug_display!(JudgeRefused);

fn unswap(order: Order, verdict: RawVerdict) -> Preference {
    match (order, verdict) {
        (_, RawVerdict::Tie) => Preference::Tie,
        (Order::AThenB, RawVerdict::First) | (Order::BThenA, RawVerdict::Second) => Preference::A,
        (Order::AThenB, RawVerdict::Second) | (Order::BThenA, RawVerdict::First) => Preference::B,
    }
}

/// `JudgeCall` does not retain the `blind` result, so `judge_pairs` screens
/// each pair.
pub fn judge_pairs(
    pairs: &[Pair],
    judge: &JudgeIdentity,
    calls: &[JudgeCall],
    canaries: &[String],
) -> Result<Vec<PairJudgment>, JudgeRefused> {
    let mut known = BTreeSet::new();
    for pair in pairs {
        if !known.insert(pair.id.as_str()) {
            return Err(JudgeRefused::DuplicatePair {
                pair: pair.id.clone(),
            });
        }
        screen(pair, canaries).map_err(JudgeRefused::Blinding)?;
    }
    let mut unswapped = BTreeMap::new();
    for call in calls {
        if !known.contains(call.pair.as_str()) {
            return Err(JudgeRefused::UnknownPair {
                pair: call.pair.clone(),
            });
        }
        if call.judge != *judge {
            return Err(JudgeRefused::JudgeDiffers {
                pair: call.pair.clone(),
            });
        }
        let preference = unswap(call.order, call.verdict);
        if unswapped
            .insert((call.pair.as_str(), call.order), preference)
            .is_some()
        {
            return Err(JudgeRefused::DuplicateCall {
                pair: call.pair.clone(),
                order: call.order,
            });
        }
    }
    pairs
        .iter()
        .map(|pair| {
            let [first, second] = Order::BOTH.map(|order| {
                unswapped
                    .get(&(pair.id.as_str(), order))
                    .copied()
                    .ok_or_else(|| JudgeRefused::OrderMissing {
                        pair: pair.id.clone(),
                        order,
                    })
            });
            let (first, second) = (first?, second?);
            Ok(PairJudgment {
                pair: pair.id.clone(),
                preference: if first == second {
                    first
                } else {
                    Preference::Inconsistent
                },
                length_a: pair.a.len() as u64,
                length_b: pair.b.len() as u64,
            })
        })
        .collect()
}

/// The arm-identification permutation check: blinded presentations where
/// the judge was asked to name the arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermutationCheck {
    pub trials: u32,
    pub correct: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermutationRefused {
    NoTrials,
    CorrectExceedsTrials {
        trials: u32,
        correct: u32,
    },
    /// The judge identified the arms by naming them correctly or
    /// consistently incorrectly.
    ArmsIdentifiable {
        trials: u32,
        correct: u32,
    },
}

debug_display!(PermutationRefused);

impl PermutationCheck {
    pub fn validate(&self) -> Result<(), PermutationRefused> {
        if self.trials == 0 {
            return Err(PermutationRefused::NoTrials);
        }
        if self.correct > self.trials {
            return Err(PermutationRefused::CorrectExceedsTrials {
                trials: self.trials,
                correct: self.correct,
            });
        }
        let identified = self.correct.max(self.trials - self.correct);
        if u64::from(identified) * 100
            > u64::from(self.trials) * u64::from(ARM_IDENTIFICATION_CEILING_PERCENT)
        {
            return Err(PermutationRefused::ArmsIdentifiable {
                trials: self.trials,
                correct: self.correct,
            });
        }
        Ok(())
    }
}

/// The residual report: every identity recorded, never a name alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResidualReport {
    pub schema: String,
    pub judge: JudgeIdentity,
    pub calibration_digest: String,
    pub live_provider: ProviderProfile,
    pub sampling: SamplingPlan,
    pub permutation: PermutationCheck,
    pub judgments: Vec<PairJudgment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResidualRefused {
    SchemaMismatch {
        found: String,
    },
    Calibration(CalibrationRefused),
    Permutation(PermutationRefused),
    JudgmentCountMismatch {
        declared: u32,
        judged: usize,
    },
    DuplicateJudgment {
        pair: String,
    },
    /// The compared run scored under another judge, provider, tokenizer, or
    /// calibration set; `residual.*` metrics do not compare until the anchor
    /// set is re-scored under the new identities.
    ReanchorRequired {
        changed: &'static str,
    },
}

debug_display!(ResidualRefused);

impl ResidualReport {
    pub fn validate(&self, calibration: &CalibrationSet) -> Result<(), ResidualRefused> {
        if self.schema != RESIDUAL_REPORT_SCHEMA {
            return Err(ResidualRefused::SchemaMismatch {
                found: self.schema.clone(),
            });
        }
        self.sampling
            .validate()
            .map_err(ResidualRefused::Calibration)?;
        self.permutation
            .validate()
            .map_err(ResidualRefused::Permutation)?;
        calibration
            .validate()
            .map_err(ResidualRefused::Calibration)?;
        if calibration.judge != self.judge {
            return Err(ResidualRefused::Calibration(
                CalibrationRefused::CalibrationJudgeDiffers,
            ));
        }
        if calibration.digest() != self.calibration_digest {
            return Err(ResidualRefused::Calibration(
                CalibrationRefused::DigestMismatch,
            ));
        }
        let mut judged = BTreeSet::new();
        if let Some(duplicate) = self
            .judgments
            .iter()
            .find(|j| !judged.insert(j.pair.as_str()))
        {
            return Err(ResidualRefused::DuplicateJudgment {
                pair: duplicate.pair.clone(),
            });
        }
        if self.judgments.len() as u64 != u64::from(self.sampling.pairs) {
            return Err(ResidualRefused::JudgmentCountMismatch {
                declared: self.sampling.pairs,
                judged: self.judgments.len(),
            });
        }
        Ok(())
    }

    /// Two runs' `residual.*` metrics compare only under the same judge,
    /// live provider, tokenizer accounting profile, and calibration digest.
    pub fn comparable(&self, other: &ResidualReport) -> Result<(), ResidualRefused> {
        for (field, same) in [
            ("judge", self.judge == other.judge),
            ("live_provider", self.live_provider == other.live_provider),
            (
                "calibration_digest",
                self.calibration_digest == other.calibration_digest,
            ),
        ] {
            if !same {
                return Err(ResidualRefused::ReanchorRequired { changed: field });
            }
        }
        Ok(())
    }
}

/// Repeated live trials of one task under one provider profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveTask {
    pub task: String,
    pub attempts: Vec<ArmResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveTaskReport {
    pub task: String,
    /// The recorded trials, retained so validation recomputes the summary
    /// instead of trusting it.
    pub attempts: Vec<ArmResult>,
    pub pass_k: PassK,
    /// Every attempt censored: nothing is known, and nothing is zero.
    pub indeterminate: bool,
}

/// The held-out live slice for one provider profile: trials, never replays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveSliceReport {
    pub schema: String,
    pub provider: ProviderProfile,
    pub k: u32,
    pub replayable: bool,
    pub tasks: Vec<LiveTaskReport>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveSliceRefused {
    SchemaMismatch {
        found: String,
    },
    /// A recorded live run presented as deterministic evidence.
    RelabelledReplayable,
    NoTasks,
    /// A task's summary is not what its attempts and the slice's `k` give.
    InconsistentTask {
        task: String,
    },
    Statistics(StatisticsError),
}

debug_display!(LiveSliceRefused);

fn summarize(attempts: &[ArmResult], k: u32) -> Result<(PassK, bool), LiveSliceRefused> {
    let pass_k = pass_k(attempts, k).map_err(LiveSliceRefused::Statistics)?;
    let indeterminate = pass_k.pass_k == PassKBounds::Indeterminate;
    Ok((pass_k, indeterminate))
}

/// Summarizes the live slice with the inherited conservative rules: pass@1,
/// the repeat counts, the censoring rate, and pass^k as an interval; all
/// attempts censored is `indeterminate`.
pub fn live_slice(
    provider: &ProviderProfile,
    k: u32,
    tasks: &[LiveTask],
) -> Result<LiveSliceReport, LiveSliceRefused> {
    if tasks.is_empty() {
        return Err(LiveSliceRefused::NoTasks);
    }
    let reports = tasks
        .iter()
        .map(|task| {
            let (pass_k, indeterminate) = summarize(&task.attempts, k)?;
            Ok(LiveTaskReport {
                task: task.task.clone(),
                attempts: task.attempts.clone(),
                pass_k,
                indeterminate,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(LiveSliceReport {
        schema: LIVE_SLICE_SCHEMA.to_string(),
        provider: provider.clone(),
        k,
        replayable: LIVE_REPLAYABLE,
        tasks: reports,
    })
}

impl LiveSliceReport {
    pub fn validate(&self) -> Result<(), LiveSliceRefused> {
        if self.schema != LIVE_SLICE_SCHEMA {
            return Err(LiveSliceRefused::SchemaMismatch {
                found: self.schema.clone(),
            });
        }
        if self.replayable {
            return Err(LiveSliceRefused::RelabelledReplayable);
        }
        if self.tasks.is_empty() {
            return Err(LiveSliceRefused::NoTasks);
        }
        for task in &self.tasks {
            let (pass_k, indeterminate) = summarize(&task.attempts, self.k)?;
            if pass_k != task.pass_k || indeterminate != task.indeterminate {
                return Err(LiveSliceRefused::InconsistentTask {
                    task: task.task.clone(),
                });
            }
        }
        Ok(())
    }
}

/// The pre-registered values a live campaign must hold before it runs; none
/// defaults, and the two provider profiles are distinct.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveSettings {
    pub providers: Vec<ProviderProfile>,
    pub k: u32,
    pub calibration: Option<CalibrationSet>,
    pub sampling: Option<SamplingPlan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveSettingsRefused {
    /// Exactly two distinct approved provider profiles are required.
    ProviderCount {
        found: usize,
    },
    ZeroRepeats,
    NoCalibrationSet,
    NoSamplingPlan,
    Calibration(CalibrationRefused),
}

debug_display!(LiveSettingsRefused);

impl LiveSettings {
    pub fn validate(&self) -> Result<(), LiveSettingsRefused> {
        let distinct: BTreeSet<&ProviderProfile> = self.providers.iter().collect();
        if distinct.len() != 2 || self.providers.len() != 2 {
            return Err(LiveSettingsRefused::ProviderCount {
                found: self.providers.len(),
            });
        }
        if self.k == 0 {
            return Err(LiveSettingsRefused::ZeroRepeats);
        }
        let calibration = self
            .calibration
            .as_ref()
            .ok_or(LiveSettingsRefused::NoCalibrationSet)?;
        calibration
            .validate()
            .map_err(LiveSettingsRefused::Calibration)?;
        self.sampling
            .ok_or(LiveSettingsRefused::NoSamplingPlan)?
            .validate()
            .map_err(LiveSettingsRefused::Calibration)
    }
}
