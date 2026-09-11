//! Semantic kernel: the durable store that owns its own SQLite connections,
//! fences every write against the lease epoch, and exposes commit envelopes,
//! admission, CAS artifacts, scope algebra, and backup as one API.

#![forbid(unsafe_code)]

pub mod sqlite_runtime;

mod admission;
mod anchor;
pub mod applicability;
mod backup;
mod cas;
mod commit_read;
mod durable_fs;
mod eligibility;
mod envelope;
mod facts;
mod object_write;
pub(crate) mod open;
mod outbox;
mod redaction;
mod retention;
pub mod schema;
mod scope;
mod slice;
mod source_descriptor;
mod source_hold;
pub mod source_identity;

pub use admission::{
    AdmissionDecision, AdmissionDomainSpec, AdmissionEvent, AdmissionRequest, Disposition,
    EffectiveMaturity, EgressCandidate, EgressSnapshot, Evaluation, EvaluationInputs, EventKind,
    Maturity, Outcome, POLICY_REVISION, PriorDecision, ScopeTermFilter, ServedClass, ServedRow,
    SourceClass, Surface, SurfaceVisibility, TaintClass, VisibilityRow, VisibleAsOf, VisibleRow,
    evaluate_admission, served_visibility_row, surface_visibility,
};
pub use anchor::{
    ANCHOR_CAPTURE_SCHEMA, AnchorCapture, AnchorCondition, AnchorDecodeError, AnchorEvaluation,
    AnchorKind, AnchorRowSpec, ContextDependency, GitCondition, PatchIdCapture, QueryContext,
    encode_anchor_captures, evaluate_non_git,
};
#[cfg(all(target_os = "linux", feature = "test-support"))]
pub use backup::filesystem_is_unsafe_for_test;
#[cfg(all(target_os = "macos", feature = "test-support"))]
pub use backup::filesystem_name_is_unsafe_for_test;
pub use backup::{BackupManifest, BackupRequest};
#[cfg(feature = "test-support")]
pub use backup::{
    RestoreFault, RestorePhase, owner_is_current_for_test, restore_marker_is_valid_for_test,
    sensitivity_bearing_tables_for_test, verify_backup_with_deadline_for_test,
};
#[cfg(feature = "test-support")]
pub use cas::{
    ArtifactDeletionFault, ArtifactDeletionHook, ArtifactGcFault, ArtifactIngestFault,
    ArtifactIngestHook,
};
pub use cas::{
    ArtifactDeletionIdentity, ArtifactDeletionKind, ArtifactDeletionRequest,
    ArtifactDeletionResult, ArtifactDestination, ArtifactEgressFacts, ArtifactEligibility,
    ArtifactError, ArtifactErrorKind, ArtifactGcResult, ArtifactHandle, ArtifactIngestRequest,
    BarrierConsumerStatus, DeletionBarrierStatus, EligibilityDeniedReason, MAX_PAYLOAD_BYTES,
    ProviderEgress,
};
pub use commit_read::{
    CommitPage, CommitPageBounds, CommitReadError, CommitReadIncarnation, CommitReadRequest,
    CommitReadTarget, CompleteCommit, PageEnd,
};
pub use eligibility::{
    EligibilityBatch, EligibilityCandidate, EligibilityVerdict, MAX_ELIGIBILITY_CANDIDATES,
    MAX_ELIGIBILITY_OBJECT_ID_BYTES, ProjectScope,
};
pub use envelope::{
    AlignmentProjectionSpec, CommitIntent, CommitReceipt, DependentObservationQuery, DomainSpec,
    Envelope, KnownAsOf, OPERATOR_REDACTION_PLACEHOLDER, ObjectRow, ObjectState, Preview,
    RemediationTarget, RepositoryProvenance, Sensitivity, StagingCandidateRow,
    StagingCandidateSpec, TokenCheck, TokenConflict,
};
pub use facts::{ArtifactBudgetFacts, KernelFacts, MAIN_FILE_WARN_BYTES, OutboxLag};
#[cfg(feature = "test-support")]
pub use open::OpenPhase;
pub use open::{KernelError, KernelStore};
pub use outbox::{ConsumerAbandonment, OutboxEntry, OutboxPruneResult};
pub use retention::{STAGING_RETENTION_MS, StagingMaintenanceResult, StagingTerminalState};
pub use scope::{
    CanonicalScope, Dimension, GraphOracle, MatchOutcome, ScopeFormError, ScopeMatchContext,
    ScopeSpec, ScopeTermSpec, ScopeWriteOutcome, TermValue, UnknownGraph, VersionSpec,
    coerce_version, scope_equivalent, scope_matches, scope_overlaps, scope_subsumes,
};
pub use slice::{
    ALIGNMENT_DEPENDENCY_KIND, AlignmentRebuild, AlignmentRow, AlignmentSnapshot,
    DecisionEventOutcome, DecisionEventPayload, DecisionEventSpec, DecisionPayload, DecisionRow,
    DecisionSpec, DecisionWriteOutcome, ObservationDependencySpec, ObservationPayload,
    ObservationRow, ObservationSpec, ObservationWriteOutcome, RetirementOutcome, SliceSnapshot,
};
pub use source_descriptor::{
    MAX_DESCRIPTORS_PER_COMMIT, SOURCE_DESCRIPTOR_DETAIL_VERSION, SOURCE_DESCRIPTOR_KIND,
    SourceDescriptorDetail, SourceDescriptorError, SourceDescriptorOutcome, SourceDescriptorPolicy,
    SourceDescriptorRequest, descriptor_object_id,
};
#[cfg(feature = "test-support")]
pub use source_hold::SourceHoldCheckPhase;
pub use source_hold::{
    HeldCursor, HeldDescriptor, HeldPage, MAX_ACTIVE_SOURCE_HOLDS_PER_CONSUMER,
    MAX_SOURCE_HOLD_LIFETIME_MS, SourceHold, SourceHoldBinding, SourceHoldBounds, SourceHoldError,
    SourceHoldInvalidity,
};

/// `Connection::execute` and `Connection::query_row` prepare their statement
/// on every call; these run the same statement through the connection's
/// prepared-statement cache instead.
pub(crate) trait CachedSql {
    fn execute_cached<P: rusqlite::Params>(&self, sql: &str, params: P) -> rusqlite::Result<usize>;

    fn query_row_cached<T, P, F>(&self, sql: &str, params: P, f: F) -> rusqlite::Result<T>
    where
        P: rusqlite::Params,
        F: FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>;
}

impl CachedSql for rusqlite::Connection {
    fn execute_cached<P: rusqlite::Params>(&self, sql: &str, params: P) -> rusqlite::Result<usize> {
        self.prepare_cached(sql)?.execute(params)
    }

    fn query_row_cached<T, P, F>(&self, sql: &str, params: P, f: F) -> rusqlite::Result<T>
    where
        P: rusqlite::Params,
        F: FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    {
        self.prepare_cached(sql)?.query_row(params, f)
    }
}

/// A constraint violation is permanent and a lock wait is retryable, so collapsing both into `Io` would make either untreatable.
pub(crate) fn map_sqlite(error: rusqlite::Error) -> KernelError {
    let rusqlite::Error::SqliteFailure(failure, _) = &error else {
        return KernelError::Io;
    };
    match failure.code {
        rusqlite::ffi::ErrorCode::ConstraintViolation => KernelError::Conflict,
        rusqlite::ffi::ErrorCode::DatabaseBusy | rusqlite::ffi::ErrorCode::DatabaseLocked => {
            KernelError::Busy
        }
        _ => KernelError::Io,
    }
}

/// Wall-clock milliseconds since the Unix epoch, saturating rather than failing so
/// a clock before the epoch cannot abort a durable write.
fn current_time_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
