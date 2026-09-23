//! Functions take values and return values; the runner shell owns processes,
//! stores, clocks, and paths.

#![forbid(unsafe_code)]

/// An identifier or name that is empty or whitespace names nothing.
pub(crate) fn blank(text: &str) -> bool {
    text.trim().is_empty()
}

/// Variant names and fields are the message; callers match on the variant.
macro_rules! debug_display {
    ($($error:ty),*) => {$(
        impl std::fmt::Display for $error {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                std::fmt::Debug::fmt(self, f)
            }
        }
        impl std::error::Error for $error {}
    )*};
}

mod campaign;
mod cassette;
mod censoring;
mod census;
mod checkpoint;
mod claim;
mod decimal;
mod eligibility;
mod event;
mod failure_class;
mod fault;
mod generator;
mod governance;
mod identity;
mod injection;
mod ledger;
mod manifest;
mod markers;
mod occurrence;
mod pairs;
mod reducer;
mod render;
mod report;
mod residue;
mod statistics;
mod stream;

pub use campaign::{
    Approval, Ceilings, DisabledReason, Envelope, EnvelopeExceeded, ProfileError,
    RUN_PROFILE_DIGEST_PROTOCOL, RUN_PROFILE_SCHEMA, Resource, RunProfile, SampleError,
    SampleLedger, SampleRecord, Scale, SkipReason, TaskBudgets, TaskUsage, Terminal, TerminalRates,
    UnsupportedReason, parse_run_profile,
};
pub use cassette::{
    BACKEND_COVERED_FIELDS, BackendRecord, Boundary, CASSETTE_GENERATOR_VERSION, CASSETTE_SCHEMA,
    COVERED_FIELDS_VERSION, Cassette, CassetteError, CassetteFile, CassetteMiss, Entry, Location,
    Lookup, MissClass, OPENCODE_COVERED_FIELDS, OPENCODE_HEADER_ALLOWLIST, OpenCodeRequest,
    canonical_decimal_f64, request_digest,
};
pub use censoring::{
    Attempt, BoundMethod, Counter, FailureRate, LatencySummary, P99_MIN_RUNS, PassK, PassKBounds,
    Percentile, PercentileBound, pass_k,
};
pub use census::{
    Construction, EvaluatedSurface, HintBounds, Reachability, SURFACE1_HINT_BOUNDS,
    SURFACE1_STAGES, Surface1Stage,
};
pub use checkpoint::{
    AGING_REPORT_SCHEMA, AGING_RESULT_DIGEST_PROTOCOL, AgingReport, AgingReportError,
    CHECKPOINT_DIGEST_PROTOCOL, Checkpoint, CheckpointRefused, ConstructionKind, Death, Descriptor,
    Divergence, GUARD_DIGEST_PROTOCOL, GenerationState, GuardComparison, HistoricalRows,
    LIVE_DIGEST_PROTOCOL, LiveRows, PrefixRefused, ProjectionConstruction, ProjectionRows,
    QuiescenceReceipt, Reopened, RestoreRefused, Segment, StateSnapshot, StoreFamily,
    StoreIntegrity, StoreQuiescence, TombstoneReason, Unenumerated, WalCheckpoint, WindowDeaths,
    WorkCounter, historical_diff, live_digest, parse_aging_report,
};
pub use claim::{
    AnchorRole, AnchorSet, AnchorTask, AnchorVerdict, ClaimClass, ClaimDerivation,
    TransferCriterion, UnmetClause, WorldProvenance, derive_claim_class,
};
pub use eligibility::*;
pub use event::*;
pub use failure_class::{
    Cell, Delivery, DurableState, FAILURE_CLASS_TABLE_DIGEST, FAILURE_CLASS_TABLE_PROTOCOL,
    FailureClass, Outcome, Slice, cells, classify, serialize_table, table_digest,
};
pub use fault::{
    APPLICATION_CRASH, ArtifactDeletionFaultKind, ArtifactIngestFaultKind, BarrierReceipt,
    BarrierRefused, CoverageRefused, CutCoverage, Effect, EffectLedger, EffectOutcome,
    EffectRefused, EffectState, EpisodeRefused, Expected, ExpectedRefusal, FAULT_REPORT_SCHEMA,
    FaultAction, FaultEpisode, FaultReport, FaultReportError, FaultScope, Heal, HealthyCore,
    KillLabel, Lane, LaneProgress, LivenessRefused, LivenessReport, PublicationFaultKind,
    RecordedRefusal, SearchEpisodeFault, TEST_BINARY_CHILD, cut_receipts, parse_fault_report,
    validate_episodes,
};
pub use generator::*;
pub use governance::{ArmError, ArmRecord, GovernanceArms, HistoryPolicy, pair_set_digest};
pub use identity::{
    BUILD_PROTOCOL, BinaryDigest, BuildRecord, IdentityError, RUN_ID_PROTOCOL, RunIdentity,
    eval_run_id, zero_bytes_sha256,
};
pub use injection::{
    AxisValue, Carrier, INJECTION_CANARY_PROTOCOL, InjectionCase, InjectionError,
    InjectionObservation, InjectionScore, LaterSession, SideEffect, StageValue, TaskSet,
    plan_injection_cases, score_injection,
};
pub use ledger::{
    CHAIN_STAGES, ChainStage, Completed, Evidence, Ledger, LedgerError,
    MAX_CANDIDATES_PER_STAGE_OBSERVATION, Observation, Presence, Required, Stage, StageKind,
    StageVerdict,
};
pub use manifest::{
    ArmRates, Attestation, CLAIM_BOUNDARY_EXCLUSIONS, CLAIM_BOUNDARY_SCHEMA, ClaimBoundary,
    ComponentVersions, Cut, CutOutcome, CutReceipt, DROPPED_FIELDS, ExecutionMode, Ingestion,
    MANIFEST_DIGEST_PROTOCOL, MANIFEST_SCHEMA, Manifest, ManifestError, MemoryReviewerModelCalls,
    REQUIRED_FIELDS, RecencyBaseline, ResourceLimits, RunStatus, TokenizerProfile,
    is_canonical_decimal, parse_manifest,
};
pub use markers::*;
pub use occurrence::*;
pub use pairs::{
    ArmKind, Baseline, BaselineContrast, BaselineFailure, BaselineVerdict,
    NATURAL_FRESH_ENTITY_TAG, PAIRING_POLICY_VERSION, Pair, PairError, PairSet, PairSetInput,
    RECENCY_BASELINE_VERSION, StopCondition, Suite, Task, TaskRole, check_recency_baseline,
    compile_pair_set, recency_bound,
};
pub use reducer::*;
pub use render::*;
pub use report::{
    CampaignGates, Claims, Established, GatedBlocks, ReportError, ReportOutcome,
    SUITE_B_REPORT_SCHEMA, SuiteBReport, Suppression, parse_report, reachability_of,
};
pub use residue::{
    CLOCK_FIELD_KEEP_ALLOWLIST, ObservationSchema, ResidueEntry, ResidueError, Rule, SemanticTrace,
    TRACE_DIGEST_PROTOCOL, is_clock_named, is_never_kept,
};
pub use statistics::{
    ANALYSIS_FAMILY_SCHEMA, Analysis, AnalysisFamily, ArmResult, BlockedReason, CampaignProfile,
    CensorReason, ClusterKey, ClusteringUnit, FrozenFamily, GATE_ENDPOINTS, GateVerdict, Gates,
    ICC_THRESHOLD, ITEM_COUNT_THRESHOLD, IccPilot, Interval, IntervalMethod, IntervalOutcome,
    IntervalWithheld, LivenessBounds, MAX_BOOTSTRAP_DRAWS, MAX_BOOTSTRAP_REPLICATES,
    MIN_BOOTSTRAP_REPLICATES, MultiplicityCorrection, PAIR_TABLE_DIGEST_PROTOCOL, PAIRED_ARMS,
    PairCounts, PairOutcome, PairedReport, PilotObservation, ProfileRates, Ratio, StatisticsError,
    StoppingRule, analyze, arm_miss_asymmetry, cluster_bootstrap_interval, intraclass_correlation,
    pair_table_digest, parse_analysis_family, parse_campaign_profile, run_icc_pilot,
};
pub use stream::*;
