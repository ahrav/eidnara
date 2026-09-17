//! Review inputs: extraction subjects and Curator proposals staged through the stager shared with the public `StagingCandidateSpec` path.
//!
//! `provenance_witness.binding` records the daemon-supplied project digest, domain, owner, and source dependencies. Review rows carry no caller provenance, so `run_sensitivity` classifies them `Sensitive`; a private read cannot upgrade that classification.
//!
//! `lease_expires_at` is an absolute queue deadline no later than [`REVIEW_QUEUE_LIFETIME_MS`] after `recorded_at`; the public one-hour lease cap and renewal path are unchanged and do not apply. Reads require completed run and candidate states, a matching stored digest and binding, and a deadline strictly after both the caller's clock and the store clock. Restaging, reads, expiry, and abandonment never extend a deadline. The existing staging sweep abandons unsealed rows at the deadline; the existing thirty-day terminal cleanup deletes sealed rows. A later owner may move a still-live deadline to a review expiry no later than [`REVIEW_EXPIRY_MAX_MS`] after result creation.

use std::collections::BTreeSet;

use rusqlite::{OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::envelope::{
    MAX_STAGING_CLOCK_SKEW_MS, RedactedCandidate, Sensitivity, StagingReplay, check_fence,
    stage_prepared_candidate,
};
use super::redaction::{
    RedactedField, contains_redaction_placeholder, identity, payload_has_secret,
};
use super::{CachedSql, KernelError, KernelStore, current_time_ms, map_sqlite};

/// Queue deadline bound for a review input, measured from `recorded_at`.
pub const REVIEW_QUEUE_LIFETIME_MS: i64 = 24 * 60 * 60 * 1_000;
/// Review expiry bound for a selected proposal, measured from result creation.
pub const REVIEW_EXPIRY_MAX_MS: i64 = 7 * 24 * 60 * 60 * 1_000;
/// `candidate_kind` literal of a staged extraction subject.
pub const REVIEW_SUBJECT_KIND: &str = "review_subject";
/// `candidate_kind` literal of a staged Curator proposal.
pub const REVIEW_PROPOSAL_KIND: &str = "review_proposal";
/// Encoded payload schema version this build writes and reads.
pub const REVIEW_PAYLOAD_VERSION: u32 = 1;
/// Serialized payload bound, checked before the secret scan runs.
pub const MAX_REVIEW_PAYLOAD_BYTES: usize = 64 * 1024;
pub const MAX_REVIEW_IDENTITY_BYTES: usize = 256;
pub const MAX_REVIEW_TEXT_BYTES: usize = 32 * 1024;
pub const MAX_REVIEW_REFERENCES: usize = 256;
/// The subject plus eight optional starting references.
pub const MAX_REVIEW_SOURCE_DEPENDENCIES: usize = 9;
pub const MAX_REVIEW_FACTS: usize = 64;
pub const MAX_REVIEW_LIMITATIONS: usize = 16;

const WITNESS_KIND: &str = "unclassified";
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
    pub project_digest: String,
    pub domain_id: String,
    pub owner: ReviewOwner,
    /// Sorted and deduplicated so equal bindings encode to equal bytes.
    pub source_dependencies: Vec<SourceDependency>,
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

/// A sealed extraction result that a review job investigates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewSubject {
    pub source: SourceDependency,
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
    pub digest: String,
}

/// Every input the broker disclosed to the model, cited or not, plus the lineage those inputs derive from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyDependencies {
    pub question_template: ReviewQuestionTemplate,
    pub disclosed_inputs: Vec<EvidenceReference>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewPayload {
    Subject(ReviewSubject),
    Proposal(ReviewProposal),
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum PayloadBody {
    ReviewSubject(ReviewSubject),
    ReviewProposal(ReviewProposal),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct VersionedPayload {
    version: u32,
    body: PayloadBody,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewWitness {
    kind: String,
    binding: ReviewBinding,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewStagingSpec {
    pub extraction_run_id: String,
    pub candidate_id: String,
    /// Recorded as the run's `extractor`.
    pub producer: String,
    pub binding: ReviewBinding,
    pub payload: ReviewPayload,
    pub recorded_at: i64,
    /// Absolute; at most [`REVIEW_QUEUE_LIFETIME_MS`] after `recorded_at`.
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
    pub reference: ReviewStagedReference,
    pub binding: ReviewBinding,
    pub payload: ReviewPayload,
    pub sensitivity: Sensitivity,
    pub lifecycle: ReviewLifecycle,
}

/// No variant carries stored content.
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
    #[error(transparent)]
    Store(#[from] KernelError),
}

/// `job_id` and `generation` select one candidate id, so a takeover that advances the generation cannot write into the prior generation's row.
pub fn provisional_result_identity(job_id: &str, generation: u64) -> ProvisionalResultIdentity {
    let mut hasher = Sha256::new();
    hasher.update(job_id.as_bytes());
    hasher.update([RESULT_ID_SEPARATOR]);
    hasher.update(generation.to_string().as_bytes());
    let digest = lower_hex(&hasher.finalize());
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

    /// Struct field order fixes the encoding, so equal payloads encode to equal bytes.
    pub fn encode(&self) -> Result<Vec<u8>, KernelError> {
        self.validate()?;
        let body = match self {
            Self::Subject(subject) => PayloadBody::ReviewSubject(subject.clone()),
            Self::Proposal(proposal) => PayloadBody::ReviewProposal(proposal.clone()),
        };
        let bytes = serde_json::to_vec(&VersionedPayload {
            version: REVIEW_PAYLOAD_VERSION,
            body,
        })
        .map_err(|_| KernelError::InvalidInput)?;
        if bytes.len() > MAX_REVIEW_PAYLOAD_BYTES {
            return Err(KernelError::InvalidInput);
        }
        Ok(bytes)
    }

    /// Unknown fields, another version, or a schema-illegal combination refuse.
    pub fn decode(bytes: &[u8]) -> Result<Self, KernelError> {
        if bytes.len() > MAX_REVIEW_PAYLOAD_BYTES {
            return Err(KernelError::InvalidInput);
        }
        let versioned: VersionedPayload =
            serde_json::from_slice(bytes).map_err(|_| KernelError::InvalidInput)?;
        if versioned.version != REVIEW_PAYLOAD_VERSION {
            return Err(KernelError::InvalidInput);
        }
        let payload = match versioned.body {
            PayloadBody::ReviewSubject(subject) => Self::Subject(subject),
            PayloadBody::ReviewProposal(proposal) => Self::Proposal(proposal),
        };
        payload.validate()?;
        Ok(payload)
    }

    fn validate(&self) -> Result<(), KernelError> {
        match self {
            Self::Subject(subject) => subject.validate(),
            Self::Proposal(proposal) => proposal.validate(),
        }
    }

    /// Lower-hex SHA-256 of [`Self::encode`].
    pub fn digest(&self) -> Result<String, KernelError> {
        Ok(digest_hex(&self.encode()?))
    }
}

impl SourceDependency {
    fn validate(&self) -> Result<(), KernelError> {
        check_identity(&self.source_kind)?;
        check_identity(&self.source_id)?;
        if self.source_revision < 0 {
            return Err(KernelError::InvalidInput);
        }
        Ok(())
    }
}

impl SourceSpan {
    fn validate(&self) -> Result<(), KernelError> {
        check_identity(&self.alias)?;
        if self.end < self.start {
            return Err(KernelError::InvalidInput);
        }
        Ok(())
    }
}

impl ReviewSubject {
    fn validate(&self) -> Result<(), KernelError> {
        self.source.validate()?;
        if self.facts.is_empty() || self.facts.len() > MAX_REVIEW_FACTS {
            return Err(KernelError::InvalidInput);
        }
        for fact in &self.facts {
            check_text(&fact.text)?;
            fact.span.validate()?;
        }
        Ok(())
    }
}

impl EvidenceReference {
    fn validate(&self) -> Result<(), KernelError> {
        check_identity(&self.evidence_id)?;
        if let Some(span) = &self.span {
            span.validate()?;
        }
        Ok(())
    }
}

impl ReviewProposal {
    /// Create targets a staged candidate and needs text; revise targets a memory and needs text; retain and retire target a memory without text; no-change takes either target without text.
    fn validate(&self) -> Result<(), KernelError> {
        let targets_memory = match &self.target {
            ProposalTarget::StagedCandidate { candidate_id } => {
                check_identity(candidate_id)?;
                false
            }
            ProposalTarget::Memory(target) => {
                check_identity(&target.object_id)?;
                if target.source_revision < 0 || target.known_as_of < 0 || target.commit_token < 0 {
                    return Err(KernelError::InvalidInput);
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
            return Err(KernelError::InvalidInput);
        }
        if let Some(text) = &self.new_text {
            check_text(text)?;
        }
        check_references(&self.support)?;
        check_references(&self.contradictions)?;
        if self.limitations.len() > MAX_REVIEW_LIMITATIONS {
            return Err(KernelError::InvalidInput);
        }
        for limitation in &self.limitations {
            check_text(limitation)?;
        }
        check_identity(&self.manifest.manifest_id)?;
        check_identity(&self.manifest.digest)?;
        let deps = &self.policy_dependencies;
        check_references(&deps.disclosed_inputs)?;
        check_references(&deps.uncited_disclosed_inputs)?;
        if deps.ancestry.len() > MAX_REVIEW_REFERENCES {
            return Err(KernelError::InvalidInput);
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
        // Every citation is a disclosed input, and the uncited list is exactly the disclosed inputs no citation names.
        if !cited.is_subset(&disclosed)
            || uncited
                != disclosed
                    .difference(&cited)
                    .copied()
                    .collect::<BTreeSet<_>>()
        {
            return Err(KernelError::InvalidInput);
        }
        Ok(())
    }
}

impl ReviewBinding {
    fn normalized(mut self) -> Result<Self, KernelError> {
        check_identity(&self.project_digest)?;
        check_identity(&self.domain_id)?;
        match &self.owner {
            ReviewOwner::Job { job_id } | ReviewOwner::Proposal { job_id, .. } => {
                check_identity(job_id)?;
            }
        }
        self.source_dependencies.sort();
        self.source_dependencies.dedup();
        if self.source_dependencies.is_empty()
            || self.source_dependencies.len() > MAX_REVIEW_SOURCE_DEPENDENCIES
        {
            return Err(KernelError::InvalidInput);
        }
        for dependency in &self.source_dependencies {
            dependency.validate()?;
        }
        Ok(self)
    }

    fn witness_bytes(&self) -> Result<Vec<u8>, KernelError> {
        serde_json::to_vec(&ReviewWitness {
            kind: WITNESS_KIND.to_string(),
            binding: self.clone(),
        })
        .map_err(|_| KernelError::InvalidInput)
    }

    fn from_witness(bytes: &[u8]) -> Option<Self> {
        let witness: ReviewWitness = serde_json::from_slice(bytes).ok()?;
        (witness.kind == WITNESS_KIND).then_some(witness.binding)
    }
}

impl ReviewStagingSpec {
    fn prepare(self) -> Result<(RedactedCandidate, Vec<u8>), KernelError> {
        let skew_ceiling = current_time_ms()
            .checked_add(MAX_STAGING_CLOCK_SKEW_MS)
            .ok_or(KernelError::InvalidInput)?;
        let deadline_ceiling = self
            .recorded_at
            .checked_add(REVIEW_QUEUE_LIFETIME_MS)
            .ok_or(KernelError::InvalidInput)?;
        if self.recorded_at < 0
            || self.recorded_at > skew_ceiling
            || self.queue_deadline_at <= self.recorded_at
            || self.queue_deadline_at > deadline_ceiling
        {
            return Err(KernelError::InvalidInput);
        }
        check_identity(&self.extraction_run_id)?;
        check_identity(&self.candidate_id)?;
        check_identity(&self.producer)?;
        let binding = self.binding.normalized()?;
        if let ReviewOwner::Proposal { job_id, generation } = &binding.owner {
            let expected = provisional_result_identity(job_id, *generation);
            if !matches!(self.payload, ReviewPayload::Proposal(_))
                || expected.extraction_run_id != self.extraction_run_id
                || expected.candidate_id != self.candidate_id
            {
                return Err(KernelError::InvalidInput);
            }
        } else if !matches!(self.payload, ReviewPayload::Subject(_)) {
            return Err(KernelError::InvalidInput);
        }
        let bytes = self.payload.encode()?;
        let text = String::from_utf8(bytes.clone()).map_err(|_| KernelError::InvalidInput)?;
        // A detected secret refuses the input instead of redacting it: a redacted row is never byte-replayable.
        if payload_has_secret(&bytes).map_err(|_| KernelError::InvalidInput)? {
            return Err(KernelError::InvalidInput);
        }
        let source = binding.source_dependencies[0].clone();
        let candidate = RedactedCandidate {
            extraction_run_id: self.extraction_run_id,
            candidate_id: self.candidate_id,
            extractor: self.producer,
            source_kind: source.source_kind,
            source_id: source.source_id,
            source_revision: source.source_revision,
            candidate_kind: RedactedField {
                text: self.payload.kind().to_string(),
                detections: Vec::new(),
            },
            payload: RedactedField {
                text,
                detections: Vec::new(),
            },
            provenance: None,
            witness: binding.witness_bytes()?,
            replay: StagingReplay::Immutable,
            recorded_at: self.recorded_at,
            lease_expires_at: self.queue_deadline_at,
        };
        Ok((candidate, bytes))
    }
}

impl KernelStore {
    /// A byte-identical restage of a live row returns the same reference without touching stored deadlines; changed bytes, a changed binding, a terminal run, or a lapsed deadline return [`KernelError::Conflict`].
    pub fn stage_review_input(
        &self,
        spec: ReviewStagingSpec,
    ) -> Result<ReviewStagedReference, KernelError> {
        let (candidate, bytes) = spec.prepare()?;
        let mut writer = self.lock_writer()?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite)?;
        check_fence(&tx, self.lease_epoch())?;
        let database_incarnation_id = super::open::database_incarnation_id_via(&tx)?;
        stage_prepared_candidate(&tx, &candidate)?;
        tx.commit().map_err(map_sqlite)?;
        Ok(ReviewStagedReference {
            database_incarnation_id,
            candidate_id: candidate.candidate_id,
            payload_digest: digest_hex(&bytes),
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
            return Err(KernelError::InvalidInput.into());
        }
        let candidate_id = identity(&reference.candidate_id)?;
        let expected = expected.clone().normalized()?;
        let reader = self.lock_reader()?;
        let incarnation = super::open::database_incarnation_id_via(&reader)?;
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
                [candidate_id.as_str()],
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
            .map_err(map_sqlite)?
            .ok_or(ReviewReadRefusal::Missing)?;
        drop(reader);
        let sealed_at = match (
            row.run_terminal.as_deref(),
            row.candidate_terminal.as_deref(),
        ) {
            (Some("completed"), Some("completed")) => {
                row.terminal_at.ok_or(ReviewReadRefusal::Abandoned)?
            }
            (None, None) => return Err(ReviewReadRefusal::Unsealed.into()),
            _ => return Err(ReviewReadRefusal::Abandoned.into()),
        };
        if row.deadline_at <= now || row.deadline_at <= current_time_ms() {
            return Err(ReviewReadRefusal::Expired.into());
        }
        if digest_hex(&row.payload) != reference.payload_digest {
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
            reference: reference.clone(),
            binding,
            payload,
            sensitivity: Sensitivity::from_stored(&row.sensitivity),
            lifecycle: ReviewLifecycle {
                created_at: row.created_at,
                queue_deadline_at: row.deadline_at,
                sealed_at,
            },
        })
    }
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

/// A redaction placeholder is refused rather than stored as identity.
fn check_identity(value: &str) -> Result<(), KernelError> {
    if value.trim().is_empty()
        || value.len() > MAX_REVIEW_IDENTITY_BYTES
        || contains_redaction_placeholder(value)
    {
        return Err(KernelError::InvalidInput);
    }
    identity(value).map(drop)
}

fn check_text(value: &str) -> Result<(), KernelError> {
    if value.len() > MAX_REVIEW_TEXT_BYTES || contains_redaction_placeholder(value) {
        return Err(KernelError::InvalidInput);
    }
    Ok(())
}

fn check_references(references: &[EvidenceReference]) -> Result<(), KernelError> {
    if references.len() > MAX_REVIEW_REFERENCES {
        return Err(KernelError::InvalidInput);
    }
    references.iter().try_for_each(EvidenceReference::validate)
}

fn digest_hex(bytes: &[u8]) -> String {
    lower_hex(&Sha256::digest(bytes))
}

fn lower_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subject_round_trips() {
        let payload = ReviewPayload::Subject(ReviewSubject {
            source: SourceDependency {
                source_kind: "a".into(),
                source_id: "b".into(),
                source_revision: 1,
            },
            facts: vec![ExtractedFact {
                text: "t".into(),
                span: SourceSpan {
                    alias: "s".into(),
                    start: 0,
                    end: 1,
                },
            }],
        });
        let bytes = payload.encode().unwrap();
        println!("{}", String::from_utf8_lossy(&bytes));
        assert_eq!(ReviewPayload::decode(&bytes).unwrap(), payload);
    }
}
