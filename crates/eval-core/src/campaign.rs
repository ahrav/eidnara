//! The approved shape of one campaign run: a named profile that pins every
//! scale, budget, envelope, and bound before the first sample; the closed
//! vocabulary every sample ends in; and the resource envelope a runner holds
//! itself to from launch.

use std::collections::{BTreeMap, BTreeSet};

use context_core::canonical_json::{is_lower_hex, protocol_digest};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::census::EvaluatedSurface;
use crate::governance::HistoryPolicy;
use crate::manifest::{Cut, ResourceLimits};
use crate::pairs::{ArmKind, PairError, StopCondition, recency_bound};
use crate::statistics::{CampaignProfile, CensorReason, Ratio, StatisticsError};

pub const RUN_PROFILE_SCHEMA: &str = "eval-run-profile/v1";
pub const RUN_PROFILE_DIGEST_PROTOCOL: &str = "eval-run-profile-digest/v1";

/// Every scale runs only when the named environment variable grants it a
/// budget, and is ignored otherwise: an S0 campaign driven through the
/// daemon's lifecycle takes longer than the rest of the daemon's suite, so
/// under the nextest regression policy it runs in its own budgeted job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scale {
    S0,
    S1,
    S2,
}

impl Scale {
    pub fn budget_env(self) -> &'static str {
        match self {
            Self::S0 => "EIDNARA_EVAL_S0_BUDGET_MS",
            Self::S1 => "EIDNARA_EVAL_S1_BUDGET_MS",
            Self::S2 => "EIDNARA_EVAL_S2_BUDGET_MS",
        }
    }
}

/// The six per-task budgets. Exhausting one censors the attempt with the
/// matching reason; `CensorReason::Timeout` is the shell's own attempt
/// timeout and belongs to no budget here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskBudgets {
    pub max_model_calls: u32,
    pub max_tool_calls: u32,
    pub max_tokens_in: u64,
    pub max_tokens_out: u64,
    pub hard_deadline_ms: u64,
    pub max_no_progress_iterations: u32,
}

/// What one attempt has consumed so far.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskUsage {
    pub model_calls: u32,
    pub tool_calls: u32,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub elapsed_ms: u64,
    pub no_progress_iterations: u32,
}

impl TaskBudgets {
    /// The first budget the usage has reached, in declaration order.
    pub fn exhausted(&self, usage: &TaskUsage) -> Option<CensorReason> {
        let reached: [(bool, CensorReason); 6] = [
            (
                usage.model_calls >= self.max_model_calls,
                CensorReason::MaxModelCalls,
            ),
            (
                usage.tool_calls >= self.max_tool_calls,
                CensorReason::MaxToolCalls,
            ),
            (
                usage.tokens_in >= self.max_tokens_in,
                CensorReason::MaxTokensIn,
            ),
            (
                usage.tokens_out >= self.max_tokens_out,
                CensorReason::MaxTokensOut,
            ),
            (
                usage.elapsed_ms >= self.hard_deadline_ms,
                CensorReason::HardDeadlineMs,
            ),
            (
                usage.no_progress_iterations >= self.max_no_progress_iterations,
                CensorReason::MaxNoProgressIterations,
            ),
        ];
        reached
            .into_iter()
            .find_map(|(hit, reason)| hit.then_some(reason))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    pub approved_by: String,
    pub approved_at_run_id: String,
}

/// Everything a campaign pins before its first sample. Every numeric field is
/// explicit: the only grounded default in the repository is surface 1's
/// recency window, which `grounded_baseline_bounds` supplies; nothing else
/// has a production constant to borrow, so absence refuses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunProfile {
    pub schema: String,
    pub name: String,
    pub scale: Scale,
    pub worlds: u32,
    pub tasks_per_world: u32,
    pub max_events_per_log: u32,
    pub budgets: TaskBudgets,
    pub envelope: ResourceLimits,
    /// The largest share of samples that may end `indeterminate`, be
    /// censored, or be skipped for a redaction refusal before the run's
    /// gates fail; canonical decimals in `[0, 1]`.
    pub indeterminate_ceiling: String,
    pub censoring_ceiling: String,
    pub redaction_refusal_ceiling: String,
    pub baseline_bounds: BTreeMap<EvaluatedSurface, u32>,
    /// The experimental margins and liveness bounds, maintainer-set.
    pub statistics: CampaignProfile,
    /// Absent until a maintainer approves the profile; a campaign does not
    /// run without one.
    pub approval: Option<Approval>,
}

/// The profile's three rate ceilings as exact ratios.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ceilings {
    pub indeterminate: Ratio,
    pub censoring: Ratio,
    pub redaction_refusals: Ratio,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileError {
    Shape(String),
    SchemaMismatch {
        found: String,
    },
    Empty {
        field: &'static str,
    },
    Zero {
        field: &'static str,
    },
    MalformedDecimal {
        field: &'static str,
    },
    RateOutOfRange {
        field: &'static str,
    },
    /// A zero bound on a surface, which the resolver would read as "use the
    /// pin" and this profile must state outright.
    ZeroBaselineBound {
        surface: EvaluatedSurface,
    },
    Baseline(PairError),
    Statistics(StatisticsError),
    MalformedRunId {
        field: &'static str,
    },
    /// The parsed value drops a field the input carried.
    Lossy,
    NotApproved {
        name: String,
    },
}

debug_display!(ProfileError);

impl RunProfile {
    /// Surface 1's window is the production hint candidate limit; no other
    /// surface has a constant to pin.
    pub fn grounded_baseline_bounds() -> BTreeMap<EvaluatedSurface, u32> {
        let k = recency_bound(EvaluatedSurface::Surface1, None).expect("pinned");
        BTreeMap::from([(EvaluatedSurface::Surface1, k)])
    }

    pub fn ceilings(&self) -> Result<Ceilings, ProfileError> {
        let rate = |field: &'static str, text: &str| {
            let ratio =
                Ratio::from_decimal(text).ok_or(ProfileError::MalformedDecimal { field })?;
            if ratio > Ratio::ONE {
                return Err(ProfileError::RateOutOfRange { field });
            }
            Ok(ratio)
        };
        Ok(Ceilings {
            indeterminate: rate("indeterminate_ceiling", &self.indeterminate_ceiling)?,
            censoring: rate("censoring_ceiling", &self.censoring_ceiling)?,
            redaction_refusals: rate("redaction_refusal_ceiling", &self.redaction_refusal_ceiling)?,
        })
    }

    pub fn validate(&self) -> Result<(), ProfileError> {
        if self.schema != RUN_PROFILE_SCHEMA {
            return Err(ProfileError::SchemaMismatch {
                found: self.schema.clone(),
            });
        }
        if self.name.is_empty() {
            return Err(ProfileError::Empty { field: "name" });
        }
        let b = &self.budgets;
        let e = &self.envelope;
        let l = &self.statistics.liveness_bounds;
        let zero: [(&'static str, bool); 20] = [
            ("worlds", self.worlds == 0),
            ("tasks_per_world", self.tasks_per_world == 0),
            ("max_events_per_log", self.max_events_per_log == 0),
            ("budgets.max_model_calls", b.max_model_calls == 0),
            ("budgets.max_tool_calls", b.max_tool_calls == 0),
            ("budgets.max_tokens_in", b.max_tokens_in == 0),
            ("budgets.max_tokens_out", b.max_tokens_out == 0),
            ("budgets.hard_deadline_ms", b.hard_deadline_ms == 0),
            (
                "budgets.max_no_progress_iterations",
                b.max_no_progress_iterations == 0,
            ),
            ("envelope.elapsed_ms", e.elapsed_ms == 0),
            ("envelope.store_bytes", e.store_bytes == 0),
            ("envelope.cassette_bytes", e.cassette_bytes == 0),
            ("envelope.artifact_bytes", e.artifact_bytes == 0),
            ("envelope.temp_roots", e.temp_roots == 0),
            ("envelope.retained_artifacts", e.retained_artifacts == 0),
            ("envelope.processes", e.processes == 0),
            (
                "statistics.liveness_bounds.catch_up_episodes",
                l.catch_up_episodes == 0,
            ),
            (
                "statistics.liveness_bounds.embedding_passes",
                l.embedding_passes == 0,
            ),
            (
                "statistics.liveness_bounds.materialization_episodes",
                l.materialization_episodes == 0,
            ),
            (
                "statistics.liveness_bounds.reviewer_coordinator_passes",
                l.reviewer_coordinator_passes == 0,
            ),
        ];
        if let Some((field, _)) = zero.into_iter().find(|(_, is_zero)| *is_zero) {
            return Err(ProfileError::Zero { field });
        }
        self.ceilings()?;
        if self.baseline_bounds.is_empty() {
            return Err(ProfileError::Empty {
                field: "baseline_bounds",
            });
        }
        for (surface, bound) in &self.baseline_bounds {
            let Some(declared) = std::num::NonZeroU32::new(*bound) else {
                return Err(ProfileError::ZeroBaselineBound { surface: *surface });
            };
            recency_bound(*surface, Some(declared)).map_err(ProfileError::Baseline)?;
        }
        self.statistics.rates().map_err(ProfileError::Statistics)?;
        if let Some(approval) = &self.approval {
            if approval.approved_by.is_empty() {
                return Err(ProfileError::Empty {
                    field: "approval.approved_by",
                });
            }
            if !is_lower_hex(&approval.approved_at_run_id, 64) {
                return Err(ProfileError::MalformedRunId {
                    field: "approval.approved_at_run_id",
                });
            }
        }
        Ok(())
    }

    /// The identity a report names, so a run can be tied to the exact profile
    /// that gated it.
    pub fn digest(&self) -> Result<String, ProfileError> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(|e| ProfileError::Shape(e.to_string()))?;
        protocol_digest(RUN_PROFILE_DIGEST_PROTOCOL, &value)
            .map_err(|e| ProfileError::Shape(e.to_string()))
    }

    /// A campaign runs only under an approved profile; code and refusal
    /// tests need no approval, an empirical result does.
    pub fn approved(&self) -> Result<&Approval, ProfileError> {
        self.validate()?;
        self.approval
            .as_ref()
            .ok_or_else(|| ProfileError::NotApproved {
                name: self.name.clone(),
            })
    }
}

/// Refuses a missing field by name before anything reads the profile, and a
/// field the type would drop, since a unit variant of a tagged enum accepts
/// and discards extra keys.
pub fn parse_run_profile(value: &Value) -> Result<RunProfile, ProfileError> {
    let profile =
        RunProfile::deserialize(value).map_err(|error| ProfileError::Shape(error.to_string()))?;
    profile.validate()?;
    let again = serde_json::to_value(&profile).map_err(|e| ProfileError::Shape(e.to_string()))?;
    if again != *value {
        return Err(ProfileError::Lossy);
    }
    Ok(profile)
}

/// Why a sample was never attempted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case", deny_unknown_fields)]
pub enum SkipReason {
    ProfileNotApproved,
    StopCondition { condition: StopCondition },
    EnvelopeExceeded(EnvelopeExceeded),
    CassetteMiss,
    RedactionRefused,
}

/// Why a sample cannot be measured on this surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case", deny_unknown_fields)]
pub enum UnsupportedReason {
    SurfaceNotActivated {
        surface: EvaluatedSurface,
    },
    NoMediationBoundary,
    PackingHasNoCaller,
    /// The policy changes state this surface never reads: `pruned` reclaims
    /// projection rows, and surface 1 reads history segments.
    PolicyNotOnSurface {
        policy: HistoryPolicy,
        surface: EvaluatedSurface,
    },
}

/// Why a sample was switched off for this run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case", deny_unknown_fields)]
pub enum DisabledReason {
    ScaleNotBudgeted { scale: Scale },
    FeatureOff,
}

/// How a sample ended. Every sample ends in exactly one of these, and the
/// three non-outcome families each name their reason from a closed set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Terminal {
    Pass,
    Fail,
    Censored { reason: CensorReason },
    Indeterminate,
    Skipped(SkipReason),
    Unsupported(UnsupportedReason),
    Disabled(DisabledReason),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SampleRecord {
    pub id: String,
    pub task: String,
    pub arm: ArmKind,
    pub policy: HistoryPolicy,
    pub cut: Cut,
    /// The `eval_run_id`s of earlier attempts of this sample, oldest first.
    pub lineage: Vec<String>,
    pub terminal: Terminal,
}

/// Every sample a run declared, in the order it ran, under one epoch. A
/// sample without a record, or a record without a declared sample, refuses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SampleLedger {
    pub epoch: u64,
    pub order: Vec<String>,
    pub samples: BTreeMap<String, SampleRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SampleError {
    OrderNotAPermutation,
    IdMismatch {
        key: String,
        id: String,
    },
    MalformedLineage {
        sample: String,
        entry: String,
    },
    /// A sample naming the same earlier attempt twice.
    RepeatedLineage {
        sample: String,
        entry: String,
    },
    /// A sample skipped for an envelope reading that is not over its bound.
    EnvelopeNotExceeded {
        sample: String,
    },
    Overflow,
}

debug_display!(SampleError);

/// Share of samples with each terminal family; the denominator is every
/// declared sample.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalRates {
    pub samples: u32,
    pub passed: Ratio,
    pub failed: Ratio,
    pub censored: Ratio,
    pub indeterminate: Ratio,
    pub skipped: Ratio,
    pub unsupported: Ratio,
    pub disabled: Ratio,
}

impl SampleLedger {
    pub fn validate(&self) -> Result<(), SampleError> {
        let ordered: BTreeSet<&str> = self.order.iter().map(String::as_str).collect();
        let declared: BTreeSet<&str> = self.samples.keys().map(String::as_str).collect();
        if ordered.len() != self.order.len() || ordered != declared {
            return Err(SampleError::OrderNotAPermutation);
        }
        for (key, record) in &self.samples {
            if *key != record.id {
                return Err(SampleError::IdMismatch {
                    key: key.clone(),
                    id: record.id.clone(),
                });
            }
            let mut seen = BTreeSet::new();
            for entry in &record.lineage {
                if !is_lower_hex(entry, 64) {
                    return Err(SampleError::MalformedLineage {
                        sample: key.clone(),
                        entry: entry.clone(),
                    });
                }
                if !seen.insert(entry) {
                    return Err(SampleError::RepeatedLineage {
                        sample: key.clone(),
                        entry: entry.clone(),
                    });
                }
            }
            if let Terminal::Skipped(SkipReason::EnvelopeExceeded(exceeded)) = record.terminal
                && !exceeded.is_breach()
            {
                return Err(SampleError::EnvelopeNotExceeded {
                    sample: key.clone(),
                });
            }
        }
        Ok(())
    }

    pub fn count(&self, pick: impl Fn(&Terminal) -> bool) -> usize {
        self.samples.values().filter(|s| pick(&s.terminal)).count()
    }

    /// Samples that were attempted: a pass, a fail, a censored attempt, or an
    /// indeterminate one. Skipped, unsupported, and disabled samples were not.
    pub fn attempted(&self) -> usize {
        self.count(|t| {
            matches!(
                t,
                Terminal::Pass
                    | Terminal::Fail
                    | Terminal::Censored { .. }
                    | Terminal::Indeterminate
            )
        })
    }

    pub fn rates(&self) -> Result<TerminalRates, SampleError> {
        self.validate()?;
        let n = i128::try_from(self.samples.len()).map_err(|_| SampleError::Overflow)?;
        let count = |pick: fn(&Terminal) -> bool| {
            let hits = self.count(pick);
            if n == 0 {
                Ok(Ratio::ZERO)
            } else {
                Ratio::try_new(i128::try_from(hits).map_err(|_| SampleError::Overflow)?, n)
                    .map_err(|_| SampleError::Overflow)
            }
        };
        Ok(TerminalRates {
            samples: u32::try_from(self.samples.len()).map_err(|_| SampleError::Overflow)?,
            passed: count(|t| matches!(t, Terminal::Pass))?,
            failed: count(|t| matches!(t, Terminal::Fail))?,
            censored: count(|t| matches!(t, Terminal::Censored { .. }))?,
            indeterminate: count(|t| matches!(t, Terminal::Indeterminate))?,
            skipped: count(|t| matches!(t, Terminal::Skipped(_)))?,
            unsupported: count(|t| matches!(t, Terminal::Unsupported(_)))?,
            disabled: count(|t| matches!(t, Terminal::Disabled(_)))?,
        })
    }
}

/// One dimension of the resource envelope. `StoreBytes` counts a store with
/// its WAL and shm sidecars; `TempRoots` and `Processes` count what the run
/// holds at once, the rest what it has accumulated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resource {
    ElapsedMs,
    StoreBytes,
    CassetteBytes,
    ArtifactBytes,
    TempRoots,
    RetainedArtifacts,
    Processes,
}

impl Resource {
    pub const ALL: [Self; 7] = [
        Self::ElapsedMs,
        Self::StoreBytes,
        Self::CassetteBytes,
        Self::ArtifactBytes,
        Self::TempRoots,
        Self::RetainedArtifacts,
        Self::Processes,
    ];

    pub fn of(self, limits: &ResourceLimits) -> u64 {
        match self {
            Self::ElapsedMs => limits.elapsed_ms,
            Self::StoreBytes => limits.store_bytes,
            Self::CassetteBytes => limits.cassette_bytes,
            Self::ArtifactBytes => limits.artifact_bytes,
            Self::TempRoots => limits.temp_roots,
            Self::RetainedArtifacts => limits.retained_artifacts,
            Self::Processes => limits.processes,
        }
    }

    fn of_mut(self, limits: &mut ResourceLimits) -> &mut u64 {
        match self {
            Self::ElapsedMs => &mut limits.elapsed_ms,
            Self::StoreBytes => &mut limits.store_bytes,
            Self::CassetteBytes => &mut limits.cassette_bytes,
            Self::ArtifactBytes => &mut limits.artifact_bytes,
            Self::TempRoots => &mut limits.temp_roots,
            Self::RetainedArtifacts => &mut limits.retained_artifacts,
            Self::Processes => &mut limits.processes,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvelopeExceeded {
    pub resource: Resource,
    pub bound: u64,
    pub observed: u64,
}

debug_display!(EnvelopeExceeded);

impl EnvelopeExceeded {
    pub fn is_breach(&self) -> bool {
        self.observed > self.bound
    }
}

/// The bounds a run holds itself to and the peaks it has seen. `observe`
/// records the peak first and refuses second, so the manifest's
/// `envelope_peaks` shows the reading that crossed the bound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub bounds: ResourceLimits,
    pub peaks: ResourceLimits,
}

impl Envelope {
    pub fn new(bounds: ResourceLimits) -> Self {
        Self {
            bounds,
            peaks: ResourceLimits {
                elapsed_ms: 0,
                store_bytes: 0,
                cassette_bytes: 0,
                artifact_bytes: 0,
                temp_roots: 0,
                retained_artifacts: 0,
                processes: 0,
            },
        }
    }

    pub fn observe(&mut self, resource: Resource, observed: u64) -> Result<(), EnvelopeExceeded> {
        let peak = resource.of_mut(&mut self.peaks);
        *peak = (*peak).max(observed);
        let bound = resource.of(&self.bounds);
        if observed > bound {
            return Err(EnvelopeExceeded {
                resource,
                bound,
                observed,
            });
        }
        Ok(())
    }

    /// The first resource whose peak is over its bound, in declaration order.
    pub fn check(&self) -> Result<(), EnvelopeExceeded> {
        for resource in Resource::ALL {
            let (bound, observed) = (resource.of(&self.bounds), resource.of(&self.peaks));
            if observed > bound {
                return Err(EnvelopeExceeded {
                    resource,
                    bound,
                    observed,
                });
            }
        }
        Ok(())
    }
}
