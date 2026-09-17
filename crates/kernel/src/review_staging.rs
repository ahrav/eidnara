//! Review inputs: extraction subjects and Curator proposals staged through the stager shared with the public `StagingCandidateSpec` path.
//!
//! `provenance_witness` carries `kind: "review"` plus a `binding` object naming the daemon-supplied project digest, domain, owner, subject source, and reference sources. `validate_provenance` rejects the `review` kind, `load_candidate_facts` filters the review `candidate_kind` literals, and the public staging path refuses those literals, so admission never resolves a review row. Review rows carry no caller provenance, so `run_sensitivity` classifies them `Sensitive`; a private read cannot upgrade that classification.
//!
//! `lease_expires_at` is an absolute queue deadline no later than [`REVIEW_QUEUE_LIFETIME_MS`] after `recorded_at`; the public one-hour lease cap and renewal path are unchanged, and `renew_staging_run` refuses review runs. Reads require completed run and candidate states, a matching stored digest and binding, and a deadline strictly after both the caller's clock and the store clock. Restaging, reads, expiry, and abandonment never extend a deadline. The existing staging sweep abandons unsealed rows at the deadline; the existing thirty-day terminal cleanup deletes sealed rows. Moving a still-live deadline to a review expiry belongs to the hold owner, not to this module.

use std::collections::BTreeSet;

use context_core::redaction::redact_durable_text;
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::envelope::{
    MAX_STAGING_CLOCK_SKEW_MS, RedactedCandidate, Sensitivity, StagingReplay, check_fence,
};
use super::redaction::{RedactedField, contains_redaction_placeholder};
use super::source_identity::identity_digest;
use super::{CachedSql, KernelError, KernelStore, current_time_ms, map_sqlite};
use crate::cas::is_artifact_digest;

/// Queue deadline bound for a review input, measured from `recorded_at`.
pub const REVIEW_QUEUE_LIFETIME_MS: i64 = 24 * 60 * 60 * 1_000;
/// `candidate_kind` literal of a staged extraction subject.
pub const REVIEW_SUBJECT_KIND: &str = "review_subject";
/// `candidate_kind` literal of a staged Curator proposal.
pub const REVIEW_PROPOSAL_KIND: &str = "review_proposal";
/// `provenance_witness.kind` literal of every review row; admission rejects it.
pub const REVIEW_WITNESS_KIND: &str = "review";
/// Encoded payload schema version this build writes and reads.
pub const REVIEW_PAYLOAD_VERSION: u32 = 1;
/// Serialized payload bound, checked before per-field validation runs.
pub const MAX_REVIEW_PAYLOAD_BYTES: usize = 64 * 1024;
pub const MAX_REVIEW_IDENTITY_BYTES: usize = 256;
pub const MAX_REVIEW_TEXT_BYTES: usize = 32 * 1024;
pub const MAX_REVIEW_REFERENCES: usize = 256;
/// Optional starting references beside the subject source.
pub const MAX_REVIEW_REFERENCE_SOURCES: usize = 8;
pub const MAX_REVIEW_FACTS: usize = 64;
pub const MAX_REVIEW_LIMITATIONS: usize = 16;
const RESULT_ID_SEPARATOR: u8 = 0x1f;

/// One source lineage a review input depends on.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceDependency {
    pub source_kind: String,
    pub source_id: String,
    pub source_revision: i64,
}

/// The job that reads a review input, or the proposal generation that writes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReviewOwner {
    Job { job_id: String },
    Proposal { job_id: String, generation: u64 },
}

/// Trusted scope a review input is staged under; the daemon supplies it, never a caller of a public route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewBinding {
    /// Lower-hex SHA-256 of the project identity.
    pub project_digest: String,
    pub domain_id: String,
    pub owner: ReviewOwner,
    /// Lineage recorded on the staging run; the subject payload cites this source.
    pub subject_source: SourceDependency,
    /// Sorted and deduplicated so equal bindings encode to equal bytes; never contains `subject_source`.
    pub reference_sources: Vec<SourceDependency>,
}

/// Fixed host-authored review questions. Each variant is content-free; source text never enters a template.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewQuestionTemplate {
    /// Review extracted facts against existing memories for support, contradiction, and supersession.
    ExtractedFacts,
}

/// Byte range inside a frozen source alias.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSpan {
    pub alias: String,
    pub start: u64,
    pub end: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtractedFact {
    pub text: String,
    pub span: SourceSpan,
}

/// A sealed extraction result that a review job investigates; its source is the binding's `subject_source`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewSubject {
    pub facts: Vec<ExtractedFact>,
}

/// Reference to retained evidence; the body stays in its evidence row.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceReference {
    pub evidence_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<SourceSpan>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalAction {
    Create,
    Revise,
    Retain,
    Retire,
    NoChange,
}

/// A canonical memory named by object id, source revision, and the snapshot the proposer read; `commit_token` is the last change commit the proposer observed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalTarget {
    pub object_id: String,
    pub source_revision: i64,
    pub known_as_of: i64,
    pub commit_token: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProposalTarget {
    StagedCandidate { candidate_id: String },
    Memory(CanonicalTarget),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Uncertainty {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestReference {
    pub manifest_id: String,
    /// Lower-hex SHA-256 of the inspection manifest.
    pub digest: String,
}

/// Every input the broker disclosed to the model, cited or not, plus the lineage those inputs derive from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyDependencies {
    pub question_template: ReviewQuestionTemplate,
    pub disclosed_inputs: Vec<EvidenceReference>,
    /// Exactly the disclosed inputs no support or contradiction cites.
    pub uncited_disclosed_inputs: Vec<EvidenceReference>,
    pub ancestry: Vec<String>,
}

/// A Curator proposal as staged; publication and acceptance happen elsewhere.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewProposal {
    pub action: ProposalAction,
    pub target: ProposalTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_text: Option<String>,
    pub support: Vec<EvidenceReference>,
    pub contradictions: Vec<EvidenceReference>,
    pub limitations: Vec<String>,
    pub uncertainty: Uncertainty,
    pub manifest: ManifestReference,
    pub policy_dependencies: PolicyDependencies,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum ReviewPayload {
    #[serde(rename = "review_subject")]
    Subject(ReviewSubject),
    #[serde(rename = "review_proposal")]
    Proposal(Box<ReviewProposal>),
}

#[derive(Serialize)]
struct VersionedPayloadRef<'a> {
    version: u32,
    body: &'a ReviewPayload,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VersionedPayload {
    version: u32,
    body: ReviewPayload,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewWitness {
    kind: String,
    binding: ReviewBinding,
}

/// Lifecycle guards deserialize `WitnessKind` rather than `ReviewWitness` so rows with obsolete bindings remain non-renewable.
#[derive(Deserialize)]
struct WitnessKind {
    kind: String,
}

/// The run and candidate ids of one review input. A proposal's ids are the [`provisional_result_identity`] the caller recorded before dispatch; staging re-derives them from the owner and refuses a mismatch, so a stale recorded identity cannot write into another generation's row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewStagingSpec {
    pub extraction_run_id: String,
    pub candidate_id: String,
    /// Recorded as the run's `extractor`.
    pub producer: String,
    pub binding: ReviewBinding,
    pub payload: ReviewPayload,
    pub recorded_at: i64,
    /// The job's absolute deadline, identical across retries; at most [`REVIEW_QUEUE_LIFETIME_MS`] after `recorded_at`.
    pub queue_deadline_at: i64,
}

/// Immutable reference to a staged review input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewStagedReference {
    pub database_incarnation_id: String,
    pub candidate_id: String,
    /// Lower-hex SHA-256 of the stored payload bytes.
    pub payload_digest: String,
}

/// Staging identity of one proposal generation, recorded before dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionalResultIdentity {
    pub extraction_run_id: String,
    pub candidate_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReviewLifecycle {
    pub created_at: i64,
    pub queue_deadline_at: i64,
    pub sealed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewStagedRow {
    pub binding: ReviewBinding,
    pub payload: ReviewPayload,
    pub sensitivity: Sensitivity,
    pub lifecycle: ReviewLifecycle,
}

/// Why a staging request stored nothing. No variant carries stored content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReviewStageRefusal {
    #[error("an identity, bound, deadline, or payload schema rule failed")]
    Invalid,
    #[error("a text or identity field holds a detected secret or redaction placeholder")]
    SecretDetected,
    #[error("the staging run is sealed, abandoned, or otherwise terminal")]
    Terminal,
    #[error("the queue deadline has passed")]
    Expired,
    #[error("stored bytes, kind, binding, or deadline differ at this identity")]
    Changed,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReviewStageError {
    #[error(transparent)]
    Refused(#[from] ReviewStageRefusal),
    #[error(transparent)]
    Store(KernelError),
}

/// Why a targeted read returned nothing. No variant carries stored content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReviewReadRefusal {
    #[error("reference names another database incarnation")]
    IncarnationMismatch,
    #[error("no staged row has the referenced candidate id")]
    Missing,
    #[error("the extraction is not sealed")]
    Unsealed,
    #[error("the staging run ended without completing")]
    Abandoned,
    #[error("the queue deadline has passed")]
    Expired,
    #[error("stored bytes do not match the referenced digest")]
    Changed,
    #[error("stored binding differs from the expected binding")]
    ScopeMismatch,
    #[error("stored payload does not decode under the current schema")]
    DecodeRefused,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReviewReadError {
    #[error(transparent)]
    Refused(#[from] ReviewReadRefusal),
    #[error("the reference, expected binding, or clock is malformed")]
    Invalid,
    #[error(transparent)]
    Store(KernelError),
}

/// `job_id` and `generation` select one candidate id, so a takeover that advances the generation cannot write into the prior generation's row.
pub fn provisional_result_identity(job_id: &str, generation: u64) -> ProvisionalResultIdentity {
    let mut hasher = Sha256::new();
    hasher.update(job_id.as_bytes());
    hasher.update([RESULT_ID_SEPARATOR]);
    hasher.update(generation.to_string().as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    ProvisionalResultIdentity {
        extraction_run_id: format!("review-result-run:{digest}"),
        candidate_id: format!("review-result:{digest}"),
    }
}

impl ReviewPayload {
    fn kind(&self) -> &'static str {
        match self {
            Self::Subject(_) => REVIEW_SUBJECT_KIND,
            Self::Proposal(_) => REVIEW_PROPOSAL_KIND,
        }
    }

    /// Struct field order fixes the encoding, so equal payloads encode to equal bytes. The size bound is checked before per-field validation so an oversized input is refused without scanning it.
    pub fn encode(&self) -> Result<String, ReviewStageRefusal> {
        let text = serde_json::to_string(&VersionedPayloadRef {
            version: REVIEW_PAYLOAD_VERSION,
            body: self,
        })
        .map_err(|_| ReviewStageRefusal::Invalid)?;
        if text.len() > MAX_REVIEW_PAYLOAD_BYTES {
            return Err(ReviewStageRefusal::Invalid);
        }
        self.validate()?;
        Ok(text)
    }

    /// Unknown fields, another version, or a schema-illegal combination refuse.
    pub fn decode(bytes: &[u8]) -> Result<Self, ReviewStageRefusal> {
        if bytes.len() > MAX_REVIEW_PAYLOAD_BYTES {
            return Err(ReviewStageRefusal::Invalid);
        }
        let versioned: VersionedPayload =
            serde_json::from_slice(bytes).map_err(|_| ReviewStageRefusal::Invalid)?;
        if versioned.version != REVIEW_PAYLOAD_VERSION {
            return Err(ReviewStageRefusal::Invalid);
        }
        versioned.body.validate()?;
        Ok(versioned.body)
    }

    fn validate(&self) -> Result<(), ReviewStageRefusal> {
        match self {
            Self::Subject(subject) => subject.validate(),
            Self::Proposal(proposal) => proposal.validate(),
        }
    }

    /// Lower-hex SHA-256 of [`Self::encode`].
    pub fn digest(&self) -> Result<String, ReviewStageRefusal> {
        Ok(identity_digest(self.encode()?.as_bytes()))
    }
}

impl SourceDependency {
    fn validate(&self) -> Result<(), ReviewStageRefusal> {
        check_identity(&self.source_kind)?;
        check_identity(&self.source_id)?;
        if self.source_revision < 0 {
            return Err(ReviewStageRefusal::Invalid);
        }
        Ok(())
    }
}

impl SourceSpan {
    fn validate(&self) -> Result<(), ReviewStageRefusal> {
        check_identity(&self.alias)?;
        if self.end < self.start {
            return Err(ReviewStageRefusal::Invalid);
        }
        Ok(())
    }
}

impl ReviewSubject {
    fn validate(&self) -> Result<(), ReviewStageRefusal> {
        if self.facts.is_empty() || self.facts.len() > MAX_REVIEW_FACTS {
            return Err(ReviewStageRefusal::Invalid);
        }
        for fact in &self.facts {
            check_text(&fact.text)?;
            fact.span.validate()?;
        }
        Ok(())
    }
}

impl EvidenceReference {
    fn validate(&self) -> Result<(), ReviewStageRefusal> {
        check_identity(&self.evidence_id)?;
        if let Some(span) = &self.span {
            span.validate()?;
        }
        Ok(())
    }
}

impl ReviewProposal {
    /// Create targets a staged candidate and needs text; revise targets a memory and needs text; retain and retire target a memory without text; no-change takes either target without text.
    fn validate(&self) -> Result<(), ReviewStageRefusal> {
        let targets_memory = match &self.target {
            ProposalTarget::StagedCandidate { candidate_id } => {
                check_identity(candidate_id)?;
                false
            }
            ProposalTarget::Memory(target) => {
                check_identity(&target.object_id)?;
                if target.source_revision < 0 || target.known_as_of < 0 || target.commit_token < 0 {
                    return Err(ReviewStageRefusal::Invalid);
                }
                true
            }
        };
        let (needs_memory, needs_text) = match self.action {
            ProposalAction::Create => (Some(false), true),
            ProposalAction::Revise => (Some(true), true),
            ProposalAction::Retain | ProposalAction::Retire => (Some(true), false),
            ProposalAction::NoChange => (None, false),
        };
        if needs_memory.is_some_and(|needs| needs != targets_memory)
            || needs_text != self.new_text.is_some()
        {
            return Err(ReviewStageRefusal::Invalid);
        }
        if let Some(text) = &self.new_text {
            check_text(text)?;
        }
        check_references(&self.support)?;
        check_references(&self.contradictions)?;
        if self.limitations.len() > MAX_REVIEW_LIMITATIONS {
            return Err(ReviewStageRefusal::Invalid);
        }
        for limitation in &self.limitations {
            check_text(limitation)?;
        }
        check_identity(&self.manifest.manifest_id)?;
        check_digest(&self.manifest.digest)?;
        let deps = &self.policy_dependencies;
        check_references(&deps.disclosed_inputs)?;
        check_references(&deps.uncited_disclosed_inputs)?;
        if deps.ancestry.len() > MAX_REVIEW_REFERENCES {
            return Err(ReviewStageRefusal::Invalid);
        }
        for ancestor in &deps.ancestry {
            check_identity(ancestor)?;
        }
        let disclosed: BTreeSet<&str> = deps
            .disclosed_inputs
            .iter()
            .map(|reference| reference.evidence_id.as_str())
            .collect();
        let cited: BTreeSet<&str> = self
            .support
            .iter()
            .chain(&self.contradictions)
            .map(|reference| reference.evidence_id.as_str())
            .collect();
        let uncited: BTreeSet<&str> = deps
            .uncited_disclosed_inputs
            .iter()
            .map(|reference| reference.evidence_id.as_str())
            .collect();
        if !cited.is_subset(&disclosed)
            || uncited
                != disclosed
                    .difference(&cited)
                    .copied()
                    .collect::<BTreeSet<_>>()
        {
            return Err(ReviewStageRefusal::Invalid);
        }
        Ok(())
    }
}

impl ReviewBinding {
    fn normalized(mut self) -> Result<Self, ReviewStageRefusal> {
        check_digest(&self.project_digest)?;
        check_identity(&self.domain_id)?;
        match &self.owner {
            ReviewOwner::Job { job_id } | ReviewOwner::Proposal { job_id, .. } => {
                check_identity(job_id)?;
            }
        }
        self.subject_source.validate()?;
        self.reference_sources.sort();
        self.reference_sources.dedup();
        if self.reference_sources.len() > MAX_REVIEW_REFERENCE_SOURCES
            || self.reference_sources.contains(&self.subject_source)
        {
            return Err(ReviewStageRefusal::Invalid);
        }
        for dependency in &self.reference_sources {
            dependency.validate()?;
        }
        Ok(self)
    }

    fn witness_bytes(&self) -> Result<Vec<u8>, ReviewStageRefusal> {
        serde_json::to_vec(&ReviewWitness {
            kind: REVIEW_WITNESS_KIND.to_string(),
            binding: self.clone(),
        })
        .map_err(|_| ReviewStageRefusal::Invalid)
    }

    fn from_witness(bytes: &[u8]) -> Option<Self> {
        let witness: ReviewWitness = serde_json::from_slice(bytes).ok()?;
        (witness.kind == REVIEW_WITNESS_KIND).then_some(witness.binding)
    }
}

/// Only the two public witness kinds are renewable; decode failures and unknown kinds count as review so that a row the current build cannot classify never becomes renewable.
pub(super) fn is_review_witness(bytes: &[u8]) -> bool {
    serde_json::from_slice::<WitnessKind>(bytes).map_or(true, |witness| {
        !matches!(witness.kind.as_str(), "repository" | "unclassified")
    })
}

/// The public staging path refuses these kinds, so a review `candidate_kind` always pairs with a review witness.
pub(super) fn is_review_kind(candidate_kind: &str) -> bool {
    candidate_kind == REVIEW_SUBJECT_KIND || candidate_kind == REVIEW_PROPOSAL_KIND
}

impl ReviewStagingSpec {
    fn prepare(self) -> Result<(RedactedCandidate, String), ReviewStageRefusal> {
        let store_now = current_time_ms();
        let skew_ceiling = store_now
            .checked_add(MAX_STAGING_CLOCK_SKEW_MS)
            .ok_or(ReviewStageRefusal::Invalid)?;
        let deadline_ceiling = self
            .recorded_at
            .checked_add(REVIEW_QUEUE_LIFETIME_MS)
            .ok_or(ReviewStageRefusal::Invalid)?;
        if self.recorded_at < 0
            || self.recorded_at > skew_ceiling
            || self.queue_deadline_at <= self.recorded_at
            || self.queue_deadline_at > deadline_ceiling
        {
            return Err(ReviewStageRefusal::Invalid);
        }
        if self.queue_deadline_at <= store_now {
            return Err(ReviewStageRefusal::Expired);
        }
        check_identity(&self.extraction_run_id)?;
        check_identity(&self.candidate_id)?;
        check_identity(&self.producer)?;
        let binding = self.binding.normalized()?;
        match (&binding.owner, &self.payload) {
            (ReviewOwner::Job { .. }, ReviewPayload::Subject(_)) => {}
            (ReviewOwner::Proposal { job_id, generation }, ReviewPayload::Proposal(_)) => {
                let expected = provisional_result_identity(job_id, *generation);
                if expected.extraction_run_id != self.extraction_run_id
                    || expected.candidate_id != self.candidate_id
                {
                    return Err(ReviewStageRefusal::Invalid);
                }
            }
            _ => return Err(ReviewStageRefusal::Invalid),
        }
        let text = self.payload.encode()?;
        let candidate = RedactedCandidate {
            extraction_run_id: self.extraction_run_id,
            candidate_id: self.candidate_id,
            extractor: self.producer,
            source_kind: binding.subject_source.source_kind.clone(),
            source_id: binding.subject_source.source_id.clone(),
            source_revision: binding.subject_source.source_revision,
            candidate_kind: RedactedField {
                text: self.payload.kind().to_string(),
                detections: Vec::new(),
            },
            payload: RedactedField {
                text: text.clone(),
                detections: Vec::new(),
            },
            provenance: None,
            witness: binding.witness_bytes()?,
            replay: StagingReplay::Immutable,
            recorded_at: self.recorded_at,
            lease_expires_at: self.queue_deadline_at,
        };
        Ok((candidate, text))
    }
}

impl KernelStore {
    /// A byte-identical restage of a live row returns the same reference without touching stored deadlines. `Terminal`, `Expired`, and `Changed` name the replay conflicts the shared stager would otherwise fold into [`KernelError::Conflict`].
    pub fn stage_review_input(
        &self,
        spec: ReviewStagingSpec,
    ) -> Result<ReviewStagedReference, ReviewStageError> {
        let (candidate, text) = spec.prepare()?;
        let mut writer = self.lock_writer().map_err(ReviewStageError::Store)?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| ReviewStageError::Store(map_sqlite(error)))?;
        check_fence(&tx, self.lease_epoch()).map_err(ReviewStageError::Store)?;
        let database_incarnation_id =
            super::open::database_incarnation_id_via(&tx).map_err(ReviewStageError::Store)?;
        classify_replay(&tx, &candidate)?;
        self.stage_prepared_candidate(&tx, &candidate)
            .map_err(|error| match error {
                KernelError::Conflict => ReviewStageRefusal::Changed.into(),
                other => ReviewStageError::Store(other),
            })?;
        tx.commit()
            .map_err(|error| ReviewStageError::Store(map_sqlite(error)))?;
        Ok(ReviewStagedReference {
            database_incarnation_id,
            candidate_id: candidate.candidate_id,
            payload_digest: identity_digest(text.as_bytes()),
        })
    }

    /// `now` is the caller's clock. The read neither heartbeats, extends, nor publishes.
    pub fn read_review_input(
        &self,
        reference: &ReviewStagedReference,
        expected: &ReviewBinding,
        now: i64,
    ) -> Result<ReviewStagedRow, ReviewReadError> {
        if now < 0 {
            return Err(ReviewReadError::Invalid);
        }
        check_identity(&reference.candidate_id).map_err(|_| ReviewReadError::Invalid)?;
        check_digest(&reference.payload_digest).map_err(|_| ReviewReadError::Invalid)?;
        let expected = expected
            .clone()
            .normalized()
            .map_err(|_| ReviewReadError::Invalid)?;
        let reader = self.lock_reader().map_err(ReviewReadError::Store)?;
        let incarnation =
            super::open::database_incarnation_id_via(&reader).map_err(ReviewReadError::Store)?;
        if incarnation != reference.database_incarnation_id {
            return Err(ReviewReadRefusal::IncarnationMismatch.into());
        }
        let row = reader
            .query_row_cached(
                "SELECT c.payload,c.provenance_witness,c.sensitivity_class,c.candidate_kind,
                        c.created_at,c.lease_expires_at,c.terminal_state,c.terminal_at,
                        r.terminal_state
                 FROM candidates c JOIN extraction_runs r USING(extraction_run_id)
                 WHERE c.candidate_id=?1",
                [reference.candidate_id.as_str()],
                |row| {
                    Ok(StoredReviewRow {
                        payload: row.get(0)?,
                        witness: row.get(1)?,
                        sensitivity: row.get(2)?,
                        candidate_kind: row.get(3)?,
                        created_at: row.get(4)?,
                        deadline_at: row.get(5)?,
                        candidate_terminal: row.get(6)?,
                        terminal_at: row.get(7)?,
                        run_terminal: row.get(8)?,
                    })
                },
            )
            .optional()
            .map_err(|error| ReviewReadError::Store(map_sqlite(error)))?
            .ok_or(ReviewReadRefusal::Missing)?;
        drop(reader);
        // The schema pairs `terminal_state` with `terminal_at`, so a completed row always carries its timestamp.
        let sealed_at = match (
            row.run_terminal.as_deref(),
            row.candidate_terminal.as_deref(),
            row.terminal_at,
        ) {
            (Some("completed"), Some("completed"), Some(sealed_at)) => sealed_at,
            (None, None, _) => return Err(ReviewReadRefusal::Unsealed.into()),
            _ => return Err(ReviewReadRefusal::Abandoned.into()),
        };
        if row.deadline_at <= now || row.deadline_at <= current_time_ms() {
            return Err(ReviewReadRefusal::Expired.into());
        }
        if identity_digest(&row.payload) != reference.payload_digest {
            return Err(ReviewReadRefusal::Changed.into());
        }
        let binding =
            ReviewBinding::from_witness(&row.witness).ok_or(ReviewReadRefusal::ScopeMismatch)?;
        if binding != expected {
            return Err(ReviewReadRefusal::ScopeMismatch.into());
        }
        let payload =
            ReviewPayload::decode(&row.payload).map_err(|_| ReviewReadRefusal::DecodeRefused)?;
        if payload.kind() != row.candidate_kind {
            return Err(ReviewReadRefusal::DecodeRefused.into());
        }
        Ok(ReviewStagedRow {
            binding,
            payload,
            // A review row is never public; a stored class below that floor is not trusted.
            sensitivity: Sensitivity::from_stored(&row.sensitivity)
                .restrictive(Sensitivity::Sensitive),
            lifecycle: ReviewLifecycle {
                created_at: row.created_at,
                queue_deadline_at: row.deadline_at,
                sealed_at,
            },
        })
    }
}

/// Names the replay outcome before the shared stager runs: a terminal run is `Terminal`, a lapsed deadline is `Expired`, and different stored bytes, kind, witness, or deadline are `Changed`. Any remaining stager conflict is also `Changed`.
fn classify_replay(tx: &Transaction<'_>, spec: &RedactedCandidate) -> Result<(), ReviewStageError> {
    let store = |error| ReviewStageError::Store(map_sqlite(error));
    let run = tx
        .query_row_cached(
            "SELECT terminal_state,lease_expires_at,provenance_witness FROM extraction_runs
             WHERE extraction_run_id=?1",
            [spec.extraction_run_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            },
        )
        .optional()
        .map_err(store)?;
    if let Some((terminal_state, lease_expires_at, witness)) = run {
        if terminal_state.is_some() {
            return Err(ReviewStageRefusal::Terminal.into());
        }
        if lease_expires_at <= current_time_ms() {
            return Err(ReviewStageRefusal::Expired.into());
        }
        if witness != spec.witness || lease_expires_at != spec.lease_expires_at {
            return Err(ReviewStageRefusal::Changed.into());
        }
    }
    let candidate = tx
        .query_row_cached(
            "SELECT extraction_run_id,candidate_kind,payload FROM candidates WHERE candidate_id=?1",
            [spec.candidate_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            },
        )
        .optional()
        .map_err(store)?;
    if let Some((run_id, kind, payload)) = candidate
        && (run_id != spec.extraction_run_id
            || kind != spec.candidate_kind.text
            || payload != spec.payload.text.as_bytes())
    {
        return Err(ReviewStageRefusal::Changed.into());
    }
    Ok(())
}

struct StoredReviewRow {
    payload: Vec<u8>,
    witness: Vec<u8>,
    sensitivity: String,
    candidate_kind: String,
    created_at: i64,
    deadline_at: i64,
    candidate_terminal: Option<String>,
    terminal_at: Option<i64>,
    run_terminal: Option<String>,
}

/// Identity fields are bounded and non-empty; a detected secret or redaction placeholder is refused rather than stored as identity.
fn check_identity(value: &str) -> Result<(), ReviewStageRefusal> {
    if value.trim().is_empty() || value.len() > MAX_REVIEW_IDENTITY_BYTES {
        return Err(ReviewStageRefusal::Invalid);
    }
    check_secret_free(value)
}

fn check_digest(value: &str) -> Result<(), ReviewStageRefusal> {
    if is_artifact_digest(value) {
        Ok(())
    } else {
        Err(ReviewStageRefusal::Invalid)
    }
}

fn check_text(value: &str) -> Result<(), ReviewStageRefusal> {
    if value.trim().is_empty() || value.len() > MAX_REVIEW_TEXT_BYTES {
        return Err(ReviewStageRefusal::Invalid);
    }
    check_secret_free(value)
}

/// Fields are scanned individually, so JSON escaping cannot hide a secret from the durable scanner.
fn check_secret_free(value: &str) -> Result<(), ReviewStageRefusal> {
    if contains_redaction_placeholder(value) || !redact_durable_text(value).detections.is_empty() {
        return Err(ReviewStageRefusal::SecretDetected);
    }
    Ok(())
}

fn check_references(references: &[EvidenceReference]) -> Result<(), ReviewStageRefusal> {
    if references.len() > MAX_REVIEW_REFERENCES {
        return Err(ReviewStageRefusal::Invalid);
    }
    references.iter().try_for_each(EvidenceReference::validate)
}
