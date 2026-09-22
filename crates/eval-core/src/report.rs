//! The Suite B report: one serializer every campaign publishes through, so
//! the claim boundary is on every report verbatim, a stop condition removes
//! the gates and not the accounting, and no report claims what its class,
//! its samples, its profile, or its exclusions forbid.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;

use context_core::canonical_json::{ContractError, canonical_json_encode, is_lower_hex};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::blank;
use crate::campaign::{
    Ceilings, DisabledReason, Envelope, EnvelopeExceeded, ProfileError, RunProfile, SampleError,
    SampleLedger, SkipReason, Terminal, TerminalRates, UnsupportedReason,
};
use crate::census::{EvaluatedSurface, Reachability};
use crate::claim::{AnchorSet, ClaimDerivation, WorldProvenance};
use crate::injection::{AxisValue, InjectionScore};
use crate::manifest::{ArmRates, ClaimBoundary};
use crate::pairs::{
    ArmKind, BaselineContrast, BaselineFailure, RECENCY_BASELINE_VERSION, StopCondition,
    recency_bound,
};
use crate::statistics::{
    AnalysisFamily, BlockedReason, ClusteringUnit, FrozenFamily, GateVerdict, Gates,
    IntervalOutcome, IntervalWithheld, PairedReport, Ratio, StatisticsError, StoppingRule,
    arm_miss_asymmetry,
};

pub const SUITE_B_REPORT_SCHEMA: &str = "eval-suite-b-report/v1";

/// The query route and the packer carry the label `ChainStage::REACHABILITY`
/// gives them: no production caller.
pub fn reachability_of(surface: EvaluatedSurface) -> Reachability {
    match surface {
        EvaluatedSurface::Surface1 | EvaluatedSurface::Surface3 => Reachability::DefaultProduction,
        EvaluatedSurface::Surface2 => Reachability::ExplicitConfigOnly,
        EvaluatedSurface::QueryRoute | EvaluatedSurface::Packing => Reachability::TestOnly,
    }
}

/// The run-level gates beside the three paired gates, `passed` when the
/// statistic is at most the bound. The indeterminate and censoring statistics
/// are shares of the attempted samples, so a never-attempted sample cannot
/// dilute them; the refusal statistic is a share of every declared sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CampaignGates {
    pub indeterminate: GateVerdict,
    pub censoring: GateVerdict,
    pub redaction_refusals: GateVerdict,
    pub arm_miss_asymmetry: GateVerdict,
}

impl CampaignGates {
    /// Refuses a ledger with no attempted sample: every rate would be zero
    /// and every gate would pass with nothing behind it.
    pub fn of(
        samples: &SampleLedger,
        ceilings: &Ceilings,
        family: &AnalysisFamily,
        arm_rates: &BTreeMap<String, ArmRates>,
    ) -> Result<Self, ReportError> {
        samples.validate().map_err(ReportError::Samples)?;
        let attempted = samples.attempted();
        if attempted == 0 {
            return Err(ReportError::NoAttemptedSamples);
        }
        let share = |hits: usize, over: usize| {
            let to_i128 = |n: usize| {
                i128::try_from(n).map_err(|_| ReportError::Samples(SampleError::Overflow))
            };
            Ratio::try_new(to_i128(hits)?, to_i128(over)?).map_err(ReportError::Statistics)
        };
        let gate = |statistic: Ratio, bound: Ratio| GateVerdict {
            statistic,
            bound,
            passed: statistic <= bound,
        };
        let indeterminate = samples.count(|t| matches!(t, Terminal::Indeterminate));
        let censored = samples.count(|t| matches!(t, Terminal::Censored { .. }));
        let refused =
            samples.count(|t| matches!(t, Terminal::Skipped(SkipReason::RedactionRefused)));
        let bound = family
            .profile
            .rates()
            .map_err(ReportError::Statistics)?
            .miss_asymmetry_bound;
        Ok(Self {
            indeterminate: gate(share(indeterminate, attempted)?, ceilings.indeterminate),
            censoring: gate(share(censored, attempted)?, ceilings.censoring),
            redaction_refusals: gate(
                share(refused, samples.samples.len())?,
                ceilings.redaction_refusals,
            ),
            arm_miss_asymmetry: gate(
                arm_miss_asymmetry(arm_rates).map_err(ReportError::Statistics)?,
                bound,
            ),
        })
    }
}

/// What a pass establishes, and nothing else: the parent's claim, split into
/// the three things a report can show. A claim outside this vocabulary has
/// no wire form, which is how an excluded claim is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Established {
    RequiredEvidencePresentAtEveryLiveStage,
    TaskOraclePasses,
    FirstLossStageNamed,
}

/// Every report carries the pinned claim boundary; `established` names the
/// properties this run demonstrated and is empty when nothing was gated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Claims {
    pub boundary: ClaimBoundary,
    pub established: Vec<Established>,
    pub derivation: ClaimDerivation,
    pub provenance: WorldProvenance,
    pub anchor_set: Option<AnchorSet>,
}

/// The gated blocks: present only when nothing suppressed them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatedBlocks {
    pub analysis: PairedReport,
    pub baseline: BaselineContrast,
    pub gates: CampaignGates,
}

/// Why a report carries no gates. Stop conditions (a), (b), and (c) map to
/// `condition`; an underpowered-table block, an arm-miss asymmetry block, and
/// an envelope stop are blocks without a stop condition: (c) is the pilot's
/// projection, not the completed table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Suppression {
    TapRejected,
    Baseline { failure: BaselineFailure },
    Analysis { reason: BlockedReason },
    Envelope { exceeded: EnvelopeExceeded },
}

impl Suppression {
    pub fn condition(&self) -> Option<StopCondition> {
        match self {
            Self::TapRejected => Some(StopCondition::A),
            Self::Baseline { .. } => Some(StopCondition::B),
            Self::Analysis {
                reason: BlockedReason::InsufficientEffectiveN { .. },
            } => Some(StopCondition::C),
            Self::Analysis {
                reason:
                    BlockedReason::TableUnderpowered { .. } | BlockedReason::ArmMissAsymmetry { .. },
            }
            | Self::Envelope { .. } => None,
        }
    }
}

/// Either the gates or the reason there are none; both at once, or neither,
/// has no wire form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReportOutcome {
    Open { gated: Box<GatedBlocks> },
    Suppressed { by: Suppression },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuiteBReport {
    pub schema: String,
    pub eval_run_id: String,
    /// The approved profile the run was gated on, verbatim: the ceilings,
    /// margins, and baseline bounds are read from it, never restated.
    pub profile: RunProfile,
    /// `RunProfile::digest` of `profile`, the identity the manifest names.
    pub profile_digest: String,
    pub surface: EvaluatedSurface,
    pub family: AnalysisFamily,
    pub claims: Claims,
    pub outcome: ReportOutcome,
    pub samples: SampleLedger,
    pub rates: TerminalRates,
    pub arm_rates: BTreeMap<String, ArmRates>,
    pub injection: Vec<InjectionScore>,
    pub envelope: Envelope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportError {
    Shape(String),
    SchemaMismatch {
        found: String,
    },
    MalformedDigest {
        field: &'static str,
    },
    ProfileDigestMismatch,
    /// The profile's margins are not the family's.
    ProfileDisagreesWithFamily,
    ClaimBoundaryMismatch,
    /// An open report claiming nothing, or a suppressed one claiming
    /// anything.
    ClaimsDisagreeWithOutcome,
    /// The stored derivation is not the one the family and anchor set derive.
    ClaimNotDerived {
        stored: ClaimDerivation,
        derived: ClaimDerivation,
    },
    FamilyDigestMismatch,
    /// The analysis counts a pair table other than the size the plan froze.
    PairCountNotFrozen {
        frozen: u32,
        found: u64,
    },
    /// An open report whose family or arm rates block the analysis.
    OpenWhileBlocked(BlockedReason),
    /// The stored paired gates are not the ones the counts and margins
    /// compute.
    PairedGatesNotDerived,
    /// The stored run gates are not the ones the ceilings, family, samples,
    /// and arm rates compute.
    GatesNotDerived,
    NoAttemptedSamples,
    /// A paired marginal larger than the ledger backs: more pairs with this
    /// arm result (`pass`, `fail`, `censored`, or `any`) than samples on that
    /// arm ended that way.
    PairsExceedSamples {
        arm: ArmKind,
        terminal: &'static str,
        pairs: u64,
        samples: u64,
    },
    /// An interval field the family and the counts do not derive.
    IntervalNotDerived {
        field: &'static str,
    },
    /// The envelope's bounds are not the approved profile's.
    EnvelopeDisagreesWithProfile,
    /// A sample skipped under a stop condition the outcome does not name.
    StopConditionDisagrees {
        sample: String,
    },
    /// A sample whose lineage names this run as an earlier attempt.
    LineageNamesThisRun {
        sample: String,
    },
    /// A sample skipped for an unapproved profile in a report whose profile
    /// is approved.
    SkipDisagreesWithProfile {
        sample: String,
    },
    /// A sample unsupported on another surface or disabled for another scale
    /// than the run's.
    SampleAxisDisagrees {
        sample: String,
    },
    /// An injection score with no case, or a second score for one case.
    InjectionScoreDisagrees {
        case_id: String,
    },
    /// An integer outside the canonical safe range.
    NotCanonical(ContractError),
    ArmRatesDisagree,
    RatesDisagree,
    /// A baseline contrast field this surface and profile do not produce, or
    /// a zero count where a contrast was exercised.
    BaselineDisagrees {
        field: &'static str,
    },
    /// A suppression naming a block the report's own family, arm rates, or
    /// envelope do not show.
    SuppressionNotDerived,
    /// Peaks over the bounds without an envelope suppression naming them.
    EnvelopeNotHonoured(EnvelopeExceeded),
    /// A sample skipped for an envelope reading the run's envelope does not
    /// show.
    SampleEnvelopeDisagrees {
        sample: String,
    },
    /// The parsed value drops a field the input carried.
    Lossy,
    Profile(ProfileError),
    Samples(SampleError),
    Statistics(StatisticsError),
}

debug_display!(ReportError);

impl SuiteBReport {
    pub fn reachability(&self) -> Reachability {
        reachability_of(self.surface)
    }

    fn check_identity(&self) -> Result<(), ReportError> {
        if self.schema != SUITE_B_REPORT_SCHEMA {
            return Err(ReportError::SchemaMismatch {
                found: self.schema.clone(),
            });
        }
        if !is_lower_hex(&self.eval_run_id, 64) {
            return Err(ReportError::MalformedDigest {
                field: "eval_run_id",
            });
        }
        self.profile.approved().map_err(ReportError::Profile)?;
        if self.profile.digest().map_err(ReportError::Profile)? != self.profile_digest {
            return Err(ReportError::ProfileDigestMismatch);
        }
        self.family.validate().map_err(ReportError::Statistics)?;
        if self.profile.statistics != self.family.profile {
            return Err(ReportError::ProfileDisagreesWithFamily);
        }
        if self.envelope.bounds != self.profile.envelope {
            return Err(ReportError::EnvelopeDisagreesWithProfile);
        }
        // The retained arm rates parse whatever the outcome; a suppression
        // that never reads them still publishes them.
        arm_miss_asymmetry(&self.arm_rates).map_err(ReportError::Statistics)?;
        Ok(())
    }

    fn check_claims(&self) -> Result<(), ReportError> {
        if self.claims.boundary != ClaimBoundary::pinned() {
            return Err(ReportError::ClaimBoundaryMismatch);
        }
        let open = matches!(self.outcome, ReportOutcome::Open { .. });
        if open == self.claims.established.is_empty() {
            return Err(ReportError::ClaimsDisagreeWithOutcome);
        }
        // The report carries the family it derives under; an open report is
        // held to the freeze its analysis recorded in `check_gated`
        // (`FamilyDigestMismatch`), and a suppressed one records no freeze.
        let frozen = FrozenFamily::freeze(&self.family).map_err(ReportError::Statistics)?;
        let derived = self
            .family
            .claim_class(
                &frozen,
                self.claims.provenance,
                self.claims.anchor_set.as_ref(),
            )
            .map_err(ReportError::Statistics)?;
        if derived != self.claims.derivation {
            return Err(ReportError::ClaimNotDerived {
                stored: self.claims.derivation.clone(),
                derived,
            });
        }
        Ok(())
    }

    /// The block `analyze` returns for this family and these arm rates, in
    /// its order: the pilot's block, then the arm-miss asymmetry.
    fn derived_block(&self) -> Result<Option<BlockedReason>, ReportError> {
        if let Some(blocked) = self.family.is_blocked() {
            return Ok(Some(blocked));
        }
        let bound = self
            .family
            .profile
            .rates()
            .map_err(ReportError::Statistics)?
            .miss_asymmetry_bound;
        let asymmetry = arm_miss_asymmetry(&self.arm_rates).map_err(ReportError::Statistics)?;
        Ok((asymmetry > bound).then_some(BlockedReason::ArmMissAsymmetry { asymmetry, bound }))
    }

    /// The recency bound the profile resolves for this surface; `None` means
    /// no baseline could have been evaluated under this profile.
    fn resolved_recency_bound(&self) -> Option<u32> {
        self.profile
            .baseline_bounds
            .get(&self.surface)
            .copied()
            .and_then(NonZeroU32::new)
            .and_then(|declared| recency_bound(self.surface, Some(declared)).ok())
    }

    /// The contrast this surface and profile produce: `check_recency_baseline`
    /// delivers at most the window's `recency_bound` ids and counts each
    /// control role over disjoint pairs of a set the campaign ran, so the two
    /// roles together are at most the analyzed pairs, and each is exercised.
    fn check_baseline(&self, baseline: &BaselineContrast, pairs: u64) -> Result<(), ReportError> {
        let disagrees = |field| Err(ReportError::BaselineDisagrees { field });
        if baseline.baseline_version != RECENCY_BASELINE_VERSION {
            return disagrees("baseline_version");
        }
        if baseline.surface != self.surface {
            return disagrees("surface");
        }
        if self.resolved_recency_bound() != Some(baseline.recency_bound) {
            return disagrees("recency_bound");
        }
        let (failed, passed) = (
            u64::from(baseline.falsification_pairs_failed),
            u64::from(baseline.positive_controls_passed),
        );
        for (field, count, most) in [
            (
                "delivered_ids",
                u64::from(baseline.delivered_ids),
                u64::from(baseline.recency_bound),
            ),
            ("falsification_pairs_failed", failed, pairs),
            (
                "positive_controls_passed",
                passed,
                pairs.saturating_sub(failed),
            ),
        ] {
            if count == 0 || count > most {
                return disagrees(field);
            }
        }
        Ok(())
    }

    /// One score per case, each naming its case and carrying only the axis
    /// values `score_injection` produces: a stage axis is a stage value, so
    /// never `not_measurable`; obedience is `not_measurable` exactly when no
    /// boundary observed the run, never `not_reached`; write-back without
    /// that boundary is `not_measurable` or `not_reached` and with it never
    /// `not_measurable`; exposure is `not_reached` without an output, never
    /// `not_measurable`. Binding the scores to the planned `TaskSet` needs
    /// the manifest the runner writes them beside.
    fn check_injection(&self) -> Result<(), ReportError> {
        let mut cases = BTreeSet::new();
        for score in &self.injection {
            let stage = |axis: AxisValue| axis != AxisValue::NotMeasurable;
            let written_back = match score.obeyed {
                AxisValue::NotMeasurable => matches!(
                    score.written_back_cross_session,
                    AxisValue::NotMeasurable | AxisValue::NotReached
                ),
                _ => score.written_back_cross_session != AxisValue::NotMeasurable,
            };
            let produced = stage(score.ingested)
                && stage(score.retrieved)
                && stage(score.packed)
                && score.obeyed != AxisValue::NotReached
                && written_back
                && score.exposure != AxisValue::NotMeasurable;
            if blank(&score.case_id) || !produced || !cases.insert(score.case_id.as_str()) {
                return Err(ReportError::InjectionScoreDisagrees {
                    case_id: score.case_id.clone(),
                });
            }
        }
        Ok(())
    }

    /// What each sample says about the run it sits in: its lineage names
    /// earlier runs, never this one; a stop-condition skip names the condition
    /// the outcome was suppressed under, so an open report carries none; an
    /// approved profile was not skipped for want of approval; and an envelope
    /// skip names this run's bound for that resource and a reading the peaks
    /// reached.
    fn check_samples(&self) -> Result<(), ReportError> {
        let stopped = match &self.outcome {
            ReportOutcome::Open { .. } => None,
            ReportOutcome::Suppressed { by } => by.condition(),
        };
        for (key, record) in &self.samples.samples {
            let sample = || key.clone();
            if record.lineage.contains(&self.eval_run_id) {
                return Err(ReportError::LineageNamesThisRun { sample: sample() });
            }
            match record.terminal {
                Terminal::Skipped(SkipReason::StopCondition { condition })
                    if Some(condition) != stopped =>
                {
                    return Err(ReportError::StopConditionDisagrees { sample: sample() });
                }
                // `check_identity` has already required the approval.
                Terminal::Skipped(SkipReason::ProfileNotApproved) => {
                    return Err(ReportError::SkipDisagreesWithProfile { sample: sample() });
                }
                // A sample not run on this surface, or at this scale, names
                // the run's own axis; a default-production surface is always
                // activated.
                Terminal::Unsupported(UnsupportedReason::SurfaceNotActivated { surface })
                    if surface != self.surface
                        || reachability_of(surface) == Reachability::DefaultProduction =>
                {
                    return Err(ReportError::SampleAxisDisagrees { sample: sample() });
                }
                // Only the packer lacks a caller.
                Terminal::Unsupported(UnsupportedReason::PackingHasNoCaller)
                    if self.surface != EvaluatedSurface::Packing =>
                {
                    return Err(ReportError::SampleAxisDisagrees { sample: sample() });
                }
                // `s0` runs in the default shards; only a scale with a budget
                // variable can be unbudgeted.
                Terminal::Disabled(DisabledReason::ScaleNotBudgeted { scale })
                    if scale != self.profile.scale || scale.budget_env().is_none() =>
                {
                    return Err(ReportError::SampleAxisDisagrees { sample: sample() });
                }
                Terminal::Skipped(SkipReason::EnvelopeExceeded(exceeded)) => {
                    let bound = exceeded.resource.of(&self.envelope.bounds);
                    let peak = exceeded.resource.of(&self.envelope.peaks);
                    if exceeded.bound != bound || exceeded.observed > peak {
                        return Err(ReportError::SampleEnvelopeDisagrees { sample: sample() });
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn check_suppression(&self, by: &Suppression) -> Result<(), ReportError> {
        match (by, self.envelope.check()) {
            (Suppression::Envelope { exceeded }, Err(shown)) if *exceeded == shown => {}
            (Suppression::Envelope { .. }, _) => return Err(ReportError::SuppressionNotDerived),
            (_, Err(exceeded)) => return Err(ReportError::EnvelopeNotHonoured(exceeded)),
            (_, Ok(())) => {}
        }
        if let Suppression::Analysis { reason } = by {
            let derived = self.derived_block()?;
            match reason {
                // `analyze` reaches the table only after the pre-table blocks
                // pass; the table's own power needs the pair table, which the
                // manifest's `result_digest` binds, so the report holds the
                // recorded block to what the family fixes.
                BlockedReason::TableUnderpowered {
                    effective_n,
                    n_clusters,
                    required_n_for_margin,
                } => {
                    let pilot = &self.family.icc_pilot;
                    let StoppingRule::FixedN { pairs } = self.family.stopping_rule;
                    // A positive pair count deflates to a positive effective N
                    // of at most itself, over one cluster per pair at most and
                    // no more than the plan's worlds or families at its unit.
                    let whole =
                        |n: u32| Ratio::try_new(i128::from(n), 1).map_err(ReportError::Statistics);
                    // With no positive ICC nothing deflates: the table's
                    // effective N is its pair count, which the plan already
                    // holds to the floor, so no table blocks.
                    let deflates =
                        pilot.icc_family > Ratio::ZERO || pilot.icc_world_seed > Ratio::ZERO;
                    if derived.is_some()
                        || !deflates
                        || *required_n_for_margin != pilot.required_n_for_margin
                        || !(1..=self.max_clusters()).contains(n_clusters)
                        || *effective_n <= Ratio::ZERO
                        || *effective_n > whole(pairs)?
                        || *effective_n >= whole(pilot.required_n_for_margin)?
                    {
                        return Err(ReportError::SuppressionNotDerived);
                    }
                    // The block follows a completed table of the frozen size,
                    // so the ledger backs one arm result per pair on each arm.
                    for arm in [ArmKind::Aged, ArmKind::Fresh] {
                        let samples = self.arm_results(arm).iter().sum::<u64>();
                        if u64::from(pairs) > samples {
                            return Err(ReportError::PairsExceedSamples {
                                arm,
                                terminal: "any",
                                pairs: u64::from(pairs),
                                samples,
                            });
                        }
                    }
                }
                _ if derived.as_ref() != Some(reason) => {
                    return Err(ReportError::SuppressionNotDerived);
                }
                _ => {}
            }
        }
        // A baseline is judged under a bound; a surface this profile resolves
        // none for was never judged, and a failure names the task it judged.
        if let Suppression::Baseline { failure } = by {
            let task = match failure {
                BaselineFailure::Vacuous => None,
                BaselineFailure::DeliveredFalsifier { task }
                | BaselineFailure::MissedPositiveControl { task } => Some(task),
            };
            if self.resolved_recency_bound().is_none() || task.is_some_and(|task| blank(task)) {
                return Err(ReportError::SuppressionNotDerived);
            }
        }
        Ok(())
    }

    /// The most clusters a table under this plan spans at the pilot's unit:
    /// one per pair at most, no more than the plan's affordable worlds, and
    /// under the family unit no more than its families, since each world lies
    /// in one family.
    fn max_clusters(&self) -> u32 {
        let pilot = &self.family.icc_pilot;
        let StoppingRule::FixedN { pairs } = self.family.stopping_rule;
        let worlds = pilot.max_affordable_worlds.min(pairs);
        match pilot.clustering_unit {
            ClusteringUnit::Family => worlds.min(pilot.n_families),
            ClusteringUnit::WorldSeed => worlds,
        }
    }

    /// Samples on `arm` that ended as an `ArmResult`: a pass, a fail, or a
    /// censored attempt. An indeterminate attempt has no arm result and backs
    /// no pair.
    fn arm_results(&self, arm: ArmKind) -> [u64; 3] {
        let mut ended = [0u64; 3];
        for record in self.samples.samples.values().filter(|r| r.arm == arm) {
            match record.terminal {
                Terminal::Pass => ended[0] += 1,
                Terminal::Fail => ended[1] += 1,
                Terminal::Censored { .. } => ended[2] += 1,
                _ => {}
            }
        }
        ended
    }

    /// What `cluster_bootstrap_interval` derives from the family and the pair
    /// count: the unit, method, replicate count, item count, and whether an
    /// interval is emitted at all. The bounds themselves need the pair table,
    /// which the manifest's `result_digest` binds.
    fn check_interval(&self, analysis: &PairedReport) -> Result<(), ReportError> {
        let disagrees = |field| Err(ReportError::IntervalNotDerived { field });
        let Ok(n_items) = u32::try_from(analysis.counts.n) else {
            return disagrees("n_items");
        };
        let threshold = self.family.item_count_threshold;
        match &analysis.interval {
            IntervalOutcome::Computed(interval) => {
                if interval.n_items != n_items {
                    return disagrees("n_items");
                }
                if n_items < threshold {
                    return disagrees("outcome");
                }
                if interval.unit != self.family.icc_pilot.clustering_unit {
                    return disagrees("unit");
                }
                if interval.method != self.family.interval_method {
                    return disagrees("method");
                }
                if interval.replicates != self.family.bootstrap_replicates {
                    return disagrees("replicates");
                }
                if !(2..=self.max_clusters().min(n_items)).contains(&interval.n_clusters) {
                    return disagrees("n_clusters");
                }
                if interval.lower > interval.upper {
                    return disagrees("bounds");
                }
            }
            IntervalOutcome::Withheld {
                reason:
                    IntervalWithheld::ItemCountBelowThreshold {
                        n_items: withheld,
                        threshold: at,
                    },
            } => {
                if *withheld != n_items {
                    return disagrees("n_items");
                }
                if *at != threshold {
                    return disagrees("threshold");
                }
                if n_items >= threshold {
                    return disagrees("outcome");
                }
            }
            IntervalOutcome::Withheld {
                reason: IntervalWithheld::FewerThanTwoClusters { n_clusters },
            } => {
                // A non-empty table spans at least one cluster, so fewer than
                // two is exactly one.
                if *n_clusters != 1 {
                    return disagrees("n_clusters");
                }
                if n_items < threshold {
                    return disagrees("outcome");
                }
            }
        }
        Ok(())
    }

    /// Every pair's arm result is one sample on that arm that ended the same
    /// way: a pass, a fail, or a censored attempt (an indeterminate attempt has
    /// no arm result and backs no pair). The aged marginals are all counted, so
    /// each is bounded by its terminal; of the fresh arm the table counts only
    /// `b` (a fresh arm that did not fail, so a pass or a censored attempt),
    /// `c` (a fail), and its censored arms, so the rest is bounded by the
    /// arm's total. Marginals the counts cannot express are left to
    /// `Gates::of`.
    fn check_pairs_backed(&self, analysis: &PairedReport) -> Result<(), ReportError> {
        let counts = &analysis.counts;
        let [aged_pass, aged_fail, aged_censored] = self.arm_results(ArmKind::Aged);
        let [fresh_pass, fresh_fail, fresh_censored] = self.arm_results(ArmKind::Fresh);
        let aged_failed = counts
            .n
            .saturating_sub(counts.aged_pass)
            .saturating_sub(counts.aged_censored);
        for (arm, terminal, pairs, samples) in [
            (ArmKind::Aged, "pass", counts.aged_pass, aged_pass),
            (ArmKind::Aged, "fail", aged_failed, aged_fail),
            (
                ArmKind::Aged,
                "censored",
                counts.aged_censored,
                aged_censored,
            ),
            (
                ArmKind::Fresh,
                "pass_or_censored",
                counts.b,
                fresh_pass + fresh_censored,
            ),
            (ArmKind::Fresh, "fail", counts.c, fresh_fail),
            (
                ArmKind::Fresh,
                "censored",
                counts.fresh_censored,
                fresh_censored,
            ),
            (
                ArmKind::Fresh,
                "any",
                counts.n,
                fresh_pass + fresh_fail + fresh_censored,
            ),
        ] {
            if pairs > samples {
                return Err(ReportError::PairsExceedSamples {
                    arm,
                    terminal,
                    pairs,
                    samples,
                });
            }
        }
        Ok(())
    }

    fn check_gated(&self, gated: &GatedBlocks) -> Result<(), ReportError> {
        let digest = self.family.digest().map_err(ReportError::Statistics)?;
        if gated.analysis.analysis_family_digest != digest {
            return Err(ReportError::FamilyDigestMismatch);
        }
        if gated.analysis.arm_rates != self.arm_rates {
            return Err(ReportError::ArmRatesDisagree);
        }
        // `analyze` refuses a table of any size but the frozen one.
        let StoppingRule::FixedN { pairs: frozen } = self.family.stopping_rule;
        if gated.analysis.counts.n != u64::from(frozen) {
            return Err(ReportError::PairCountNotFrozen {
                frozen,
                found: gated.analysis.counts.n,
            });
        }
        if let Some(blocked) = self.derived_block()? {
            return Err(ReportError::OpenWhileBlocked(blocked));
        }
        let margins = self
            .family
            .profile
            .rates()
            .map_err(ReportError::Statistics)?;
        let paired =
            Gates::of(&gated.analysis.counts, &margins).map_err(ReportError::Statistics)?;
        if paired != gated.analysis.gates {
            return Err(ReportError::PairedGatesNotDerived);
        }
        self.check_interval(&gated.analysis)?;
        self.check_pairs_backed(&gated.analysis)?;
        let ceilings = self.profile.ceilings().map_err(ReportError::Profile)?;
        let gates = CampaignGates::of(&self.samples, &ceilings, &self.family, &self.arm_rates)?;
        if gates != gated.gates {
            return Err(ReportError::GatesNotDerived);
        }
        self.check_baseline(&gated.baseline, gated.analysis.counts.n)?;
        self.envelope
            .check()
            .map_err(ReportError::EnvelopeNotHonoured)
    }

    fn check_accounting(&self) -> Result<(), ReportError> {
        let rates = self.samples.rates().map_err(ReportError::Samples)?;
        if rates != self.rates {
            return Err(ReportError::RatesDisagree);
        }
        self.check_samples()?;
        self.check_injection()?;
        match &self.outcome {
            ReportOutcome::Open { gated } => self.check_gated(gated),
            ReportOutcome::Suppressed { by } => self.check_suppression(by),
        }
    }

    pub fn validate(&self) -> Result<(), ReportError> {
        // Digestible on both runtimes: no integer may leave the canonical safe range.
        let value = serde_json::to_value(self).map_err(|e| ReportError::Shape(e.to_string()))?;
        canonical_json_encode(&value).map_err(ReportError::NotCanonical)?;
        self.check_identity()?;
        self.check_claims()?;
        self.check_accounting()
    }

    /// The one serializer: refuses before a byte leaves.
    pub fn serialize(&self) -> Result<Value, ReportError> {
        self.validate()?;
        serde_json::to_value(self).map_err(|error| ReportError::Shape(error.to_string()))
    }
}

/// Refuses a missing block by name, a field the type would drop, then
/// everything `validate` refuses.
pub fn parse_report(value: &Value) -> Result<SuiteBReport, ReportError> {
    let report =
        SuiteBReport::deserialize(value).map_err(|error| ReportError::Shape(error.to_string()))?;
    report.validate()?;
    let again = serde_json::to_value(&report).map_err(|e| ReportError::Shape(e.to_string()))?;
    if again != *value {
        return Err(ReportError::Lossy);
    }
    Ok(report)
}
