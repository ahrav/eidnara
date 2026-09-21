//! The Suite B report: one serializer every campaign publishes through, so
//! the claim boundary is on every report verbatim, a stop condition removes
//! the gates and not the accounting, and no report claims what its class,
//! its samples, its profile, or its exclusions forbid.

use std::collections::BTreeMap;

use context_core::canonical_json::is_lower_hex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::campaign::{
    Ceilings, Envelope, EnvelopeExceeded, SampleError, SampleLedger, SkipReason, Terminal,
    TerminalRates,
};
use crate::census::{EvaluatedSurface, Reachability};
use crate::claim::{AnchorSet, ClaimDerivation, WorldProvenance};
use crate::injection::InjectionScore;
use crate::manifest::{ArmRates, ClaimBoundary};
use crate::pairs::{BaselineContrast, BaselineFailure, StopCondition};
use crate::statistics::{
    AnalysisFamily, BlockedReason, GateVerdict, PairedReport, Ratio, StatisticsError,
    arm_miss_asymmetry,
};

pub const SUITE_B_REPORT_SCHEMA: &str = "eval-suite-b-report/v1";

/// Which evidence a surface's results are: surfaces 1 and 3 are shipped
/// defaults; surface 2, the query route, and packing are activated
/// components, evaluated only with an installed enabling object.
pub fn reachability_of(surface: EvaluatedSurface) -> Reachability {
    match surface {
        EvaluatedSurface::Surface1 | EvaluatedSurface::Surface3 => Reachability::DefaultProduction,
        EvaluatedSurface::Surface2 | EvaluatedSurface::QueryRoute | EvaluatedSurface::Packing => {
            Reachability::ExplicitConfigOnly
        }
    }
}

/// The run-level gates beside the three paired gates: each a rate held under
/// the profile's ceiling or the family's bound, `passed` when the statistic
/// is at most the bound.
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
        if samples.attempted() == 0 {
            return Err(ReportError::NoAttemptedSamples);
        }
        let rates = samples.rates().map_err(ReportError::Samples)?;
        let gate = |statistic: Ratio, bound: Ratio| GateVerdict {
            statistic,
            bound,
            passed: statistic <= bound,
        };
        let refused = samples
            .samples
            .values()
            .filter(|s| matches!(s.terminal, Terminal::Skipped(SkipReason::RedactionRefused)))
            .count();
        let refusal_rate = Ratio::try_new(
            i128::try_from(refused).map_err(|_| ReportError::Samples(SampleError::Overflow))?,
            i128::from(rates.samples),
        )
        .map_err(ReportError::Statistics)?;
        let bound = family
            .profile
            .rates()
            .map_err(ReportError::Statistics)?
            .miss_asymmetry_bound;
        Ok(Self {
            indeterminate: gate(rates.indeterminate, ceilings.indeterminate),
            censoring: gate(rates.censored, ceilings.censoring),
            redaction_refusals: gate(refusal_rate, ceilings.redaction_refusals),
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
/// `condition`; an arm-miss asymmetry block and an envelope stop are blocks
/// without a stop condition.
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
                reason: BlockedReason::ArmMissAsymmetry { .. },
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
    pub profile_name: String,
    /// `RunProfile::digest` of the approved profile, so the ceilings below
    /// are the ones it approved.
    pub profile_digest: String,
    pub ceilings: Ceilings,
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
    /// The stored gates are not the ones the ceilings, family, samples, and
    /// arm rates compute.
    GatesNotDerived,
    NoAttemptedSamples,
    /// More pairs in the analysis than samples were attempted.
    PairsExceedSamples {
        pairs: u64,
        attempted: u64,
    },
    ArmRatesDisagree,
    RatesDisagree,
    /// An open report whose peaks are over its bounds.
    EnvelopeNotHonoured(EnvelopeExceeded),
    /// The parsed value drops a field the input carried.
    Lossy,
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
        for (field, digest) in [
            ("eval_run_id", &self.eval_run_id),
            ("profile_digest", &self.profile_digest),
        ] {
            if !is_lower_hex(digest, 64) {
                return Err(ReportError::MalformedDigest { field });
            }
        }
        self.family.validate().map_err(ReportError::Statistics)
    }

    fn check_claims(&self) -> Result<(), ReportError> {
        if self.claims.boundary != ClaimBoundary::pinned() {
            return Err(ReportError::ClaimBoundaryMismatch);
        }
        let open = matches!(self.outcome, ReportOutcome::Open { .. });
        if open == self.claims.established.is_empty() {
            return Err(ReportError::ClaimsDisagreeWithOutcome);
        }
        let derived = self
            .family
            .claim_class(self.claims.provenance, self.claims.anchor_set.as_ref());
        if derived != self.claims.derivation {
            return Err(ReportError::ClaimNotDerived {
                stored: self.claims.derivation.clone(),
                derived,
            });
        }
        Ok(())
    }

    fn check_accounting(&self) -> Result<(), ReportError> {
        let rates = self.samples.rates().map_err(ReportError::Samples)?;
        if rates != self.rates {
            return Err(ReportError::RatesDisagree);
        }
        let ReportOutcome::Open { gated } = &self.outcome else {
            return Ok(());
        };
        let digest = self.family.digest().map_err(ReportError::Statistics)?;
        if gated.analysis.analysis_family_digest != digest {
            return Err(ReportError::FamilyDigestMismatch);
        }
        if gated.analysis.arm_rates != self.arm_rates {
            return Err(ReportError::ArmRatesDisagree);
        }
        let attempted = u64::try_from(self.samples.attempted()).expect("bounded");
        if gated.analysis.counts.n > attempted {
            return Err(ReportError::PairsExceedSamples {
                pairs: gated.analysis.counts.n,
                attempted,
            });
        }
        let gates =
            CampaignGates::of(&self.samples, &self.ceilings, &self.family, &self.arm_rates)?;
        if gates != gated.gates {
            return Err(ReportError::GatesNotDerived);
        }
        self.envelope
            .check()
            .map_err(ReportError::EnvelopeNotHonoured)
    }

    pub fn validate(&self) -> Result<(), ReportError> {
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
