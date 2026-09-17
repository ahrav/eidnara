//! One policy-validating broker between a Curator run and the stores. Every model-visible byte passes through it, and the broker never invents support: a reference resolves only through Kernel-owned expectations, a canonical and a promoted form of one decision count once, and an unknown or partial disclosure permits no usable conclusion.
//!
//! Aliases are run-local (Q15): issued for one `(job, receipt generation)`, never persisted, and a fabricated alias resolves to nothing before any effect. Accounting follows the resource table: 32 issued inspections, 8 operations per batch, 128 KiB of rendered model-visible bytes charged on every render (Q22), and one append-only 16 MiB buffer map per run (Q4). Every rendered buffer carries a provenance tag (Q16) and passes the detection-only render check first; a hit refuses with the alias and a bounded host-authored code, never a redacted substitute (Q7).

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use context_core::curator_policy_union::{PolicyUnion, PolicyUnionMember};
use context_core::redaction::{contains_redaction_token, reject_secret_text};
use kernel::source_identity::OccurrenceClass;
use kernel::{
    ArtifactDestination, ArtifactEligibility, ArtifactHandle, CURATOR_CAPTURE_RETENTION_CLASS,
    CuratorHoldBinding, CuratorHoldError, CuratorHoldKind, EligibilityCandidate,
    EligibilityVerdict, HeldEvidence, KernelError, KernelStore, MAX_RUN_BUFFER_BYTES,
    OPERATOR_REDACTION_PLACEHOLDER, ProjectScope, ReviewBinding, ReviewPayload, ReviewReadError,
    ReviewStagedReference, RunBufferMap, RunBufferRefusal, SOURCE_DESCRIPTOR_KIND, Sensitivity,
    SourceDescriptorDetail, Surface, SurfaceVisibility,
};
use sha2::{Digest, Sha256};

pub const MAX_ISSUED_INSPECTIONS: usize = 32;
pub const MAX_OPERATIONS_PER_BATCH: usize = 8;
/// Rendered model-visible evidence bytes per run, charged on every render including resends.
pub const MAX_MODEL_VISIBLE_BYTES: u64 = 128 * 1024;
const ALIAS_PREFIX: &str = "ref-";

/// The fixed host-authored question templates a run may carry (Q24). Source-derived bound questions are not a supported input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestionTemplate {
    ExtractedFacts,
}

impl QuestionTemplate {
    pub const fn text(self) -> &'static str {
        match self {
            Self::ExtractedFacts => {
                "Review the extracted facts against the referenced memories. Report support, contradiction, supersession, and shared origins with citations; abstain when the evidence does not warrant a conclusion."
            }
        }
    }

    pub const fn id(self) -> &'static str {
        match self {
            Self::ExtractedFacts => "extracted_facts",
        }
    }

    /// The template's lineage revision: a digest of its text, so an edited prompt changes every union that carried it.
    pub fn revision(self) -> String {
        format!("{:x}", Sha256::digest(self.text().as_bytes()))
    }

    /// Only the fixed identifiers resolve; any other input, including one carrying question text, is refused.
    pub fn parse(id: &str) -> Option<Self> {
        (id == Self::ExtractedFacts.id()).then_some(Self::ExtractedFacts)
    }
}

/// Origin of one Curator-visible buffer, from a closed set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OriginClass {
    HostAuthored,
    StagedSubject,
    NativeSource,
    CanonicalSource,
    TemporaryCapture,
    SearchResult,
    ModelText,
    Diagnostic,
}

impl OriginClass {
    /// Host-authored template text, alias tokens, and refusal codes are not charged against the model-visible bound (Q22).
    pub const fn charged(self) -> bool {
        !matches!(self, Self::HostAuthored | Self::Diagnostic)
    }
}

/// Run-local alias for one broker-issued reference.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Alias(String);

impl Alias {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Kernel-owned expectations a reference must still meet when it is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReferenceExpectation {
    /// A completed staged extraction: immutable digest, binding, and queue lifetime are checked by the Kernel read.
    StagedSubject {
        reference: ReviewStagedReference,
        binding: ReviewBinding,
    },
    /// A native-source descriptor: its object, revision, digest, class, and identity tuple.
    NativeSource {
        object_id: String,
        class: OccurrenceClass,
        source_revision: i64,
        artifact_digest: String,
        evidence_id: String,
        occurrence_tuple: Vec<u8>,
    },
    /// A canonical-claim or promoted-memory descriptor and the originating decision its eligibility resolves through.
    CanonicalSource {
        object_id: String,
        class: OccurrenceClass,
        source_revision: i64,
        artifact_digest: String,
        evidence_id: String,
        originating_decision_id: String,
        decision_source_revision: i64,
    },
    /// Evidence a Curator run captured for itself, protected by its execution hold until its acquisition reference expires.
    TemporaryCapture {
        evidence_id: String,
        artifact_digest: String,
        byte_length: u64,
        retain_until: i64,
    },
}

impl ReferenceExpectation {
    pub fn origin(&self) -> OriginClass {
        match self {
            Self::StagedSubject { .. } => OriginClass::StagedSubject,
            Self::NativeSource { .. } => OriginClass::NativeSource,
            Self::CanonicalSource { .. } => OriginClass::CanonicalSource,
            Self::TemporaryCapture { .. } => OriginClass::TemporaryCapture,
        }
    }

    /// The identity two forms of one origin share: the originating decision for canonical and promoted forms, the object or row otherwise. Hashes never stand in for this.
    fn origin_key(&self) -> String {
        match self {
            Self::StagedSubject { reference, .. } => format!("staged:{}", reference.candidate_id),
            Self::NativeSource {
                occurrence_tuple, ..
            } => format!("native:{}", hex(occurrence_tuple)),
            Self::CanonicalSource {
                originating_decision_id,
                ..
            } => format!("decision:{originating_decision_id}"),
            Self::TemporaryCapture { evidence_id, .. } => format!("capture:{evidence_id}"),
        }
    }

    fn policy_member(&self, owner_revision: Option<i64>) -> PolicyUnionMember {
        match self {
            Self::StagedSubject { reference, .. } => PolicyUnionMember {
                kind: "staged_subject".to_string(),
                id: reference.candidate_id.clone(),
                revision: reference.payload_digest.clone(),
                owner_id: None,
                owner_revision: None,
            },
            Self::NativeSource {
                object_id,
                source_revision,
                ..
            } => PolicyUnionMember {
                kind: "native_source".to_string(),
                id: object_id.clone(),
                revision: source_revision.to_string(),
                owner_id: None,
                owner_revision: None,
            },
            Self::CanonicalSource {
                object_id,
                source_revision,
                originating_decision_id,
                ..
            } => PolicyUnionMember {
                kind: "canonical_source".to_string(),
                id: object_id.clone(),
                revision: source_revision.to_string(),
                owner_id: Some(originating_decision_id.clone()),
                owner_revision: owner_revision.map(|revision| revision.to_string()),
            },
            Self::TemporaryCapture {
                evidence_id,
                artifact_digest,
                ..
            } => PolicyUnionMember {
                kind: "temporary_capture".to_string(),
                id: evidence_id.clone(),
                revision: artifact_digest.clone(),
                owner_id: None,
                owner_revision: None,
            },
        }
    }
}

/// Bounded host-authored refusal codes; the only thing a refused read discloses beside the alias.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RefusalCode {
    #[error("unknown_alias")]
    UnknownAlias,
    #[error("invalid_range")]
    InvalidRange,
    #[error("inspection_limit")]
    InspectionLimit,
    #[error("batch_limit")]
    BatchLimit,
    #[error("byte_limit")]
    ByteLimit,
    #[error("buffer_limit")]
    BufferLimit,
    #[error("hold_limit")]
    HoldLimit,
    #[error("expectation_changed")]
    ExpectationChanged,
    #[error("scope")]
    Scope,
    #[error("policy_blocked")]
    PolicyBlocked,
    #[error("origin_revoked")]
    OriginRevoked,
    #[error("hold_invalid")]
    HoldInvalid,
    #[error("render_check")]
    RenderCheck,
    #[error("undecodable")]
    Undecodable,
    #[error("invalid_cursor")]
    InvalidCursor,
    #[error("unsupported_question")]
    UnsupportedQuestion,
    #[error("store")]
    Store,
}

impl RefusalCode {
    /// Whether the refusal came from a run or batch ceiling rather than from the reference itself. A capacity refusal after a disclosure leaves the evidence set truncated; any other refusal is a fact about one reference and leaves the set intact.
    pub fn is_capacity(self) -> bool {
        matches!(
            self,
            Self::InspectionLimit
                | Self::BatchLimit
                | Self::ByteLimit
                | Self::BufferLimit
                | Self::HoldLimit
        )
    }
}

/// A refused read: the requesting alias when one exists, and one bounded code. Repeated probing yields nothing more.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code} ({})", alias.as_ref().map_or("-", Alias::as_str))]
pub struct Refusal {
    pub alias: Option<Alias>,
    pub code: RefusalCode,
}

fn refuse(alias: Option<&Alias>, code: RefusalCode) -> Refusal {
    Refusal {
        alias: alias.cloned(),
        code,
    }
}

/// Provenance of one Curator-visible buffer (Q16): kept in memory beside the rendered bytes, never in any schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvenanceTag {
    pub origin: OriginClass,
    pub alias: Option<Alias>,
    /// Byte range the buffer occupies in the assembled prompt; assigned by the prompt assembler.
    pub prompt_range: Range<usize>,
    /// The verdict and visibility the Kernel gave the referenced object at the snapshot tip; host-authored text, staged subjects, and captures carry `None`.
    pub verdict: Option<JudgedAt>,
    pub charged_bytes: u64,
}

/// One Kernel eligibility judgement as recorded on a tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JudgedAt {
    pub verdict: EligibilityVerdict,
    pub visibility: SurfaceVisibility,
    pub tip: i64,
}

/// Rendered bytes with their tag; the bytes are owned by the render and borrowed by the assembler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedBuffer {
    pub bytes: Vec<u8>,
    pub tag: ProvenanceTag,
}

/// Detection-only render check over materialized bytes: a secret or a redaction placeholder refuses disclosure (Q7). Host-authored text passes through the same check so a template can never carry protected text.
pub fn check_render(bytes: &[u8], alias: Option<&Alias>) -> Result<(), Refusal> {
    let text = std::str::from_utf8(bytes).map_err(|_| refuse(alias, RefusalCode::Undecodable))?;
    if contains_redaction_token(text)
        || text.contains(OPERATOR_REDACTION_PLACEHOLDER)
        || reject_secret_text(text).is_err()
    {
        return Err(refuse(alias, RefusalCode::RenderCheck));
    }
    Ok(())
}

/// Run-local alias table for one `(job, receipt generation)`; a takeover starts a new one.
#[derive(Debug, Default)]
pub struct AliasTable {
    next: usize,
    references: BTreeMap<Alias, ReferenceExpectation>,
}

impl AliasTable {
    pub fn issue(&mut self, expectation: ReferenceExpectation) -> Alias {
        self.next += 1;
        let alias = Alias(format!("{ALIAS_PREFIX}{}", self.next));
        self.references.insert(alias.clone(), expectation);
        alias
    }

    /// Resolves an alias the broker issued; any other string, including a well-formed one that was never issued, is unknown.
    pub fn resolve(&self, alias: &str) -> Result<(&Alias, &ReferenceExpectation), Refusal> {
        self.references
            .get_key_value(&Alias(alias.to_string()))
            .ok_or_else(|| refuse(None, RefusalCode::UnknownAlias))
    }

    pub fn len(&self) -> usize {
        self.references.len()
    }

    pub fn is_empty(&self) -> bool {
        self.references.is_empty()
    }
}

/// Inspection, disclosure, uncertain disclosure, and citation are tracked apart; the policy union grows from disclosure alone.
#[derive(Debug, Default)]
pub struct DisclosureLedger {
    inspected: BTreeSet<Alias>,
    disclosed: BTreeSet<Alias>,
    cited: BTreeSet<Alias>,
    /// Set when any attempt's outcome is unknown: bytes may have reached the model without a recorded disclosure.
    uncertain: bool,
    /// Set when a capacity refusal followed a disclosure: the model reasoned over a truncated evidence set.
    partial: bool,
    union: PolicyUnion,
}

impl DisclosureLedger {
    pub fn new(question: QuestionTemplate) -> Self {
        let mut ledger = Self::default();
        ledger.union.insert(PolicyUnionMember {
            kind: "question_template".to_string(),
            id: question.id().to_string(),
            revision: question.revision(),
            owner_id: None,
            owner_revision: None,
        });
        ledger
    }

    pub fn record_inspection(&mut self, alias: &Alias) {
        self.inspected.insert(alias.clone());
    }

    /// A disclosure adds the alias and its lineage member to the union whether or not the model later cites it.
    pub fn record_disclosure(&mut self, alias: &Alias, member: PolicyUnionMember) {
        self.disclosed.insert(alias.clone());
        self.union.insert(member);
    }

    /// The marker-derived scalar (Q20): any unknown attempt terminal makes every later conclusion unusable.
    pub fn record_uncertain_disclosure(&mut self) {
        self.uncertain = true;
    }

    /// A citation must name a disclosed alias; citing something never shown is refused.
    pub fn record_citation(&mut self, alias: &Alias) -> Result<(), Refusal> {
        if !self.disclosed.contains(alias) {
            return Err(refuse(Some(alias), RefusalCode::UnknownAlias));
        }
        self.cited.insert(alias.clone());
        Ok(())
    }

    pub fn uncited_disclosed(&self) -> impl Iterator<Item = &Alias> {
        self.disclosed.difference(&self.cited)
    }

    pub fn disclosed(&self) -> impl Iterator<Item = &Alias> {
        self.disclosed.iter()
    }

    pub fn inspected(&self) -> usize {
        self.inspected.len()
    }

    pub fn union(&self) -> &PolicyUnion {
        &self.union
    }

    /// A capacity refusal after at least one disclosure leaves the evidence set truncated.
    pub fn record_partial_disclosure(&mut self) {
        if !self.disclosed.is_empty() {
            self.partial = true;
        }
    }

    /// Whether a conclusion may be published: nothing is usable after an unknown or partial disclosure.
    pub fn conclusions_usable(&self) -> bool {
        !self.uncertain && !self.partial
    }
}

/// The resource table's per-run counters.
#[derive(Debug)]
pub struct InvestigationAccounting {
    issued_inspections: usize,
    batch_operations: usize,
    model_visible_bytes: u64,
    max_inspections: usize,
}

impl InvestigationAccounting {
    pub fn new() -> Self {
        Self::with_inspection_limit(MAX_ISSUED_INSPECTIONS)
    }

    /// A lower inspection ceiling for tests, per Q23; production uses [`MAX_ISSUED_INSPECTIONS`].
    pub fn with_inspection_limit(max_inspections: usize) -> Self {
        Self {
            issued_inspections: 0,
            batch_operations: 0,
            model_visible_bytes: 0,
            max_inspections,
        }
    }

    /// Admits one operation of the current batch; refused-after-admission operations count, decode-rejected steps do not.
    pub fn admit_operation(&mut self, alias: Option<&Alias>) -> Result<(), Refusal> {
        if self.batch_operations >= MAX_OPERATIONS_PER_BATCH {
            return Err(refuse(alias, RefusalCode::BatchLimit));
        }
        if self.issued_inspections >= self.max_inspections {
            return Err(refuse(alias, RefusalCode::InspectionLimit));
        }
        self.batch_operations += 1;
        self.issued_inspections += 1;
        Ok(())
    }

    pub fn end_batch(&mut self) {
        self.batch_operations = 0;
    }

    /// Operations the current batch can still admit. A caller that stops at zero avoids provoking a `BatchLimit` refusal, which would mark the run's disclosure partial.
    pub fn batch_headroom(&self) -> usize {
        MAX_OPERATIONS_PER_BATCH.saturating_sub(self.batch_operations)
    }

    /// Charges rendered bytes before they are appended; a charge that would exceed the bound is refused whole.
    pub fn charge_render(&mut self, alias: Option<&Alias>, bytes: u64) -> Result<(), Refusal> {
        let total = self.model_visible_bytes.saturating_add(bytes);
        if total > MAX_MODEL_VISIBLE_BYTES {
            return Err(refuse(alias, RefusalCode::ByteLimit));
        }
        self.model_visible_bytes = total;
        Ok(())
    }

    pub fn issued_inspections(&self) -> usize {
        self.issued_inspections
    }

    pub fn model_visible_bytes(&self) -> u64 {
        self.model_visible_bytes
    }
}

impl Default for InvestigationAccounting {
    fn default() -> Self {
        Self::new()
    }
}

/// What a read-only probe of a canonical source produced.
#[derive(Debug)]
pub(crate) enum Probed {
    /// The bytes the descriptor selects from an artifact that was judged, policy-checked, and render-checked whole but not disclosed, and their offset in the artifact.
    Bytes { bytes: Vec<u8>, start: u64 },
    /// The artifact is longer than the caller's bound; nothing was read.
    TooLarge { byte_length: u64 },
    /// The artifact was read but cannot be disclosed: it is unreadable or fails the render check. The bytes it cost are reported so a caller can charge them.
    Refused { byte_length: u64 },
}

/// What one successful read returns to the coordinator: the rendered buffer, the origin key two forms of one decision share, and the lineage member the disclosure added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceRead {
    pub alias: Alias,
    pub buffer: RenderedBuffer,
    pub origin_key: String,
    pub member: PolicyUnionMember,
    pub sensitivity: Sensitivity,
}

/// The run's Kernel-side bindings: the project scope and the execution hold every disclosed byte must be protected by.
#[derive(Debug, Clone)]
pub struct RunBinding {
    pub project: ProjectScope,
    pub hold: CuratorHoldBinding,
    pub hold_id: String,
    /// Where disclosed bytes go. A remote model admits only `Normal`, remote-allowed evidence; unproven sources stay policy-blocked there.
    pub destination: ArtifactDestination,
}

/// The broker: aliases, accounting, buffers, ledger, and the reads that tie them to Kernel expectations.
pub struct EvidenceBroker {
    pub aliases: AliasTable,
    pub accounting: InvestigationAccounting,
    pub buffers: RunBufferMap,
    pub ledger: DisclosureLedger,
    binding: RunBinding,
    /// Origin keys already disclosed, so a second form of one decision is reported as the same origin rather than fresh support.
    origins: BTreeMap<String, Alias>,
    /// Artifacts whose whole buffer passed the render check, so a range split cannot hide a marker or secret across two reads.
    checked_artifacts: BTreeSet<String>,
}

impl EvidenceBroker {
    pub fn new(binding: RunBinding, question: QuestionTemplate) -> Self {
        Self {
            aliases: AliasTable::default(),
            accounting: InvestigationAccounting::new(),
            buffers: RunBufferMap::new(MAX_RUN_BUFFER_BYTES),
            ledger: DisclosureLedger::new(question),
            binding,
            origins: BTreeMap::new(),
            checked_artifacts: BTreeSet::new(),
        }
    }

    /// Lowers the inspection ceiling for tests (Q23); production keeps [`MAX_ISSUED_INSPECTIONS`].
    pub fn with_inspection_limit(mut self, max_inspections: usize) -> Self {
        self.accounting = InvestigationAccounting::with_inspection_limit(max_inspections);
        self
    }

    /// Lowers the run buffer ceiling for tests; production keeps the 16 MiB bound.
    pub fn with_buffer_limit(mut self, capacity: u64) -> Self {
        self.buffers = RunBufferMap::new(capacity);
        self
    }

    /// The execution hold every disclosed byte is protected by.
    pub fn hold_id(&self) -> &str {
        &self.binding.hold_id
    }

    /// Renders host-authored text under the same render check; it is tagged and uncharged (Q22).
    pub fn render_host_text(&self, text: &str) -> Result<RenderedBuffer, Refusal> {
        check_render(text.as_bytes(), None)?;
        Ok(RenderedBuffer {
            bytes: text.as_bytes().to_vec(),
            tag: ProvenanceTag {
                origin: OriginClass::HostAuthored,
                alias: None,
                prompt_range: 0..0,
                verdict: None,
                charged_bytes: 0,
            },
        })
    }

    /// The alias whose origin `alias` shares, when an earlier disclosure already resolved the same decision or row.
    pub fn shared_origin(&self, alias: &str) -> Result<Option<&Alias>, Refusal> {
        let (alias, expectation) = self.aliases.resolve(alias)?;
        Ok(self
            .origins
            .get(&expectation.origin_key())
            .filter(|first| *first != alias))
    }

    /// Reads one reference for disclosure. Any refusal happens before bytes are retained or disclosed; a capacity refusal ([`RefusalCode::is_capacity`]) also records a partial disclosure.
    pub fn read(
        &mut self,
        store: &KernelStore,
        alias: &str,
        range: Option<Range<u64>>,
        now_ms: i64,
    ) -> Result<EvidenceRead, Refusal> {
        let read = self.read_reference(store, alias, range, now_ms);
        if read
            .as_ref()
            .is_err_and(|refusal| refusal.code.is_capacity())
        {
            self.ledger.record_partial_disclosure();
        }
        read
    }

    fn read_reference(
        &mut self,
        store: &KernelStore,
        alias: &str,
        range: Option<Range<u64>>,
        now_ms: i64,
    ) -> Result<EvidenceRead, Refusal> {
        let (alias, expectation) = self
            .aliases
            .resolve(alias)
            .map(|(alias, expectation)| (alias.clone(), expectation.clone()))?;
        self.accounting.admit_operation(Some(&alias))?;
        self.ledger.record_inspection(&alias);
        if range.as_ref().is_some_and(|range| range.end <= range.start) {
            return Err(refuse(Some(&alias), RefusalCode::InvalidRange));
        }
        let (bytes, verdict, sensitivity, member, origin_key) = match &expectation {
            ReferenceExpectation::StagedSubject { reference, binding } => {
                if binding.project_digest != self.binding.hold.project_digest {
                    return Err(refuse(Some(&alias), RefusalCode::Scope));
                }
                let row = store
                    .read_review_input(reference, binding, now_ms)
                    .map_err(|error| refuse(Some(&alias), staged_refusal(error)))?;
                self.gate_destination(&alias, row.sensitivity)?;
                let text = match row.payload {
                    ReviewPayload::Subject(subject) => subject
                        .facts
                        .iter()
                        .map(|fact| fact.text.as_str())
                        .collect::<Vec<_>>()
                        .join("\n"),
                    ReviewPayload::Proposal(_) => {
                        return Err(refuse(Some(&alias), RefusalCode::ExpectationChanged));
                    }
                };
                let bytes = slice_range(text.as_bytes(), range.as_ref())
                    .ok_or_else(|| refuse(Some(&alias), RefusalCode::InvalidRange))?;
                self.charge(&alias, bytes.len())?;
                (
                    bytes,
                    None,
                    row.sensitivity,
                    expectation.policy_member(None),
                    expectation.origin_key(),
                )
            }
            ReferenceExpectation::TemporaryCapture {
                evidence_id,
                artifact_digest,
                byte_length,
                retain_until,
            } => {
                if *retain_until <= now_ms {
                    return Err(refuse(Some(&alias), RefusalCode::ExpectationChanged));
                }
                if range.as_ref().is_some_and(|range| range.end > *byte_length) {
                    return Err(refuse(Some(&alias), RefusalCode::InvalidRange));
                }
                let held = self.hold_evidence(store, &alias, evidence_id, now_ms)?;
                if held.artifact_digest != *artifact_digest
                    || held.byte_length != *byte_length
                    || held.retention_class != CURATOR_CAPTURE_RETENTION_CLASS
                    || held.retain_until != Some(*retain_until)
                {
                    return Err(refuse(Some(&alias), RefusalCode::ExpectationChanged));
                }
                let bytes = self.load_range(store, &alias, &held, range.clone())?;
                (
                    bytes,
                    None,
                    held.sensitivity,
                    expectation.policy_member(None),
                    expectation.origin_key(),
                )
            }
            ReferenceExpectation::NativeSource {
                object_id,
                class,
                source_revision,
                artifact_digest,
                evidence_id,
                occurrence_tuple,
            } => {
                let judged = self.judge(
                    store,
                    Some(&alias),
                    object_id,
                    *source_revision,
                    artifact_digest,
                    None,
                )?;
                let detail = self.descriptor(store, Some(&alias), object_id, judged.tip)?;
                if detail.class != class.code()
                    || detail.occurrence_tuple != *occurrence_tuple
                    || detail.artifact_digest != *artifact_digest
                    || detail.evidence_id != *evidence_id
                {
                    return Err(refuse(Some(&alias), RefusalCode::ExpectationChanged));
                }
                let held = self.hold_evidence(store, &alias, evidence_id, now_ms)?;
                let bytes = self.load_range(store, &alias, &held, range.clone())?;
                // Two excerpts or revisions of one native source are one origin: the key is the identity tuple, without revision, representation, or span.
                let origin_key = format!(
                    "native:{}:{}",
                    class.code(),
                    detail
                        .identity
                        .iter()
                        .map(|(field, value)| format!("{}={}", field.len(), value))
                        .collect::<Vec<_>>()
                        .join("\u{1f}")
                );
                (
                    bytes,
                    Some(judged),
                    held.sensitivity,
                    expectation.policy_member(None),
                    origin_key,
                )
            }
            ReferenceExpectation::CanonicalSource {
                evidence_id,
                decision_source_revision,
                ..
            } => {
                let (judged, _) = self.judge_canonical_source(store, Some(&alias), &expectation)?;
                let held = self.hold_evidence(store, &alias, evidence_id, now_ms)?;
                let bytes = self.load_range(store, &alias, &held, range.clone())?;
                (
                    bytes,
                    Some(judged),
                    held.sensitivity,
                    expectation.policy_member(Some(*decision_source_revision)),
                    expectation.origin_key(),
                )
            }
        };
        check_render(&bytes, Some(&alias))?;
        let charged = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        self.ledger.record_disclosure(&alias, member.clone());
        self.origins
            .entry(origin_key.clone())
            .or_insert_with(|| alias.clone());
        Ok(EvidenceRead {
            buffer: RenderedBuffer {
                bytes,
                tag: ProvenanceTag {
                    origin: expectation.origin(),
                    alias: Some(alias.clone()),
                    prompt_range: 0..0,
                    verdict,
                    charged_bytes: charged,
                },
            },
            alias,
            origin_key,
            member,
            sensitivity,
        })
    }

    /// Reads a canonical source's artifact without disclosing it: the same Kernel judgement, descriptor check, egress verdict, and render check as [`Self::read`], but no alias, hold, retained buffer, charge, or ledger entry. Related-memory discovery uses it to test relatedness; a candidate that passes is then disclosed through `read`, which revalidates. The whole artifact is read and render-checked, and the bytes the descriptor's span selects are returned with the span's start: only those bytes carry the descriptor's provenance. `Probed::TooLarge` reports an artifact longer than `max_bytes` before any byte is read; `Probed::Refused` reports one that was read and then refused, with the bytes it cost. Only canonical sources are probed; any other expectation is refused as unsupported.
    pub(crate) fn probe_canonical_source(
        &self,
        store: &KernelStore,
        expectation: &ReferenceExpectation,
        max_bytes: u64,
    ) -> Result<Probed, Refusal> {
        let (_, detail) = self.judge_canonical_source(store, None, expectation)?;
        let ReferenceExpectation::CanonicalSource {
            artifact_digest,
            evidence_id,
            ..
        } = expectation
        else {
            return Err(refuse(None, RefusalCode::UnsupportedQuestion));
        };
        let handle = ArtifactHandle {
            digest: artifact_digest.clone(),
            evidence_id: evidence_id.clone(),
        };
        self.egress_allowed(store, None, &handle)?;
        let byte_length = store
            .artifact_byte_length(&handle)
            .map_err(|_| refuse(None, RefusalCode::Store))?
            .ok_or_else(|| refuse(None, RefusalCode::ExpectationChanged))?;
        if byte_length > max_bytes {
            return Ok(Probed::TooLarge { byte_length });
        }
        let Ok(bytes) = store.read_artifact(&handle) else {
            return Ok(Probed::Refused { byte_length });
        };
        if check_render(&bytes, None).is_err() {
            return Ok(Probed::Refused { byte_length });
        }
        let Some((start, end)) = detail.span else {
            return Ok(Probed::Bytes { bytes, start: 0 });
        };
        let selected = usize::try_from(start)
            .ok()
            .zip(usize::try_from(end).ok())
            .and_then(|(start, end)| bytes.get(start..end))
            .ok_or_else(|| refuse(None, RefusalCode::ExpectationChanged))?;
        Ok(Probed::Bytes {
            bytes: selected.to_vec(),
            start,
        })
    }

    /// Judges a canonical descriptor together with its originating decision, then checks the live descriptor detail against the expectation and returns both. The decision's standing caps the descriptor's (Q34): a retracted, superseded, or hidden decision revokes every form derived from it. Any other expectation kind is refused as unsupported.
    fn judge_canonical_source(
        &self,
        store: &KernelStore,
        alias: Option<&Alias>,
        expectation: &ReferenceExpectation,
    ) -> Result<(JudgedAt, SourceDescriptorDetail), Refusal> {
        let ReferenceExpectation::CanonicalSource {
            object_id,
            class,
            source_revision,
            artifact_digest,
            evidence_id,
            originating_decision_id,
            decision_source_revision,
        } = expectation
        else {
            return Err(refuse(alias, RefusalCode::UnsupportedQuestion));
        };
        let judged = self.judge(
            store,
            alias,
            object_id,
            *source_revision,
            artifact_digest,
            Some((originating_decision_id, *decision_source_revision)),
        )?;
        let detail = self.descriptor(store, alias, object_id, judged.tip)?;
        let decision_in_tuple = detail
            .identity
            .iter()
            .any(|(_, value)| value == originating_decision_id);
        if detail.class != class.code()
            || !decision_in_tuple
            || detail.artifact_digest != *artifact_digest
            || detail.evidence_id != *evidence_id
        {
            return Err(refuse(alias, RefusalCode::ExpectationChanged));
        }
        Ok((judged, detail))
    }

    /// The Kernel's egress verdict folds every live reference to the digest for this destination; default-Sensitive policy means unproven evidence never reaches a remote model.
    fn egress_allowed(
        &self,
        store: &KernelStore,
        alias: Option<&Alias>,
        handle: &ArtifactHandle,
    ) -> Result<(), Refusal> {
        let facts = store
            .artifact_egress_facts(handle, self.binding.destination)
            .map_err(|_| refuse(alias, RefusalCode::ExpectationChanged))?;
        if facts.eligibility != ArtifactEligibility::Allowed {
            return Err(refuse(alias, RefusalCode::PolicyBlocked));
        }
        Ok(())
    }

    /// Grows the execution hold over the artifact before any byte is read, then returns the held facts.
    fn hold_evidence(
        &self,
        store: &KernelStore,
        alias: &Alias,
        evidence_id: &str,
        now_ms: i64,
    ) -> Result<HeldEvidence, Refusal> {
        store
            .extend_execution_hold(
                &self.binding.hold_id,
                &self.binding.hold,
                std::slice::from_ref(&evidence_id.to_string()),
            )
            .map_err(|error| refuse(Some(alias), hold_refusal(error)))?;
        let mut held = store
            .validate_held_evidence(
                &self.binding.hold_id,
                CuratorHoldKind::Execution,
                &self.binding.hold,
                std::slice::from_ref(&evidence_id.to_string()),
                now_ms,
            )
            .map_err(|error| refuse(Some(alias), hold_refusal(error)))?;
        let held = held
            .pop()
            .ok_or_else(|| refuse(Some(alias), RefusalCode::HoldInvalid))?;
        self.egress_allowed(
            store,
            Some(alias),
            &ArtifactHandle {
                digest: held.artifact_digest.clone(),
                evidence_id: held.evidence_id.clone(),
            },
        )?;
        Ok(held)
    }

    /// Staged subjects are not artifacts; the same destination rule applies to their stored class.
    fn gate_destination(&self, alias: &Alias, sensitivity: Sensitivity) -> Result<(), Refusal> {
        if sensitivity == Sensitivity::Secret
            || (self.binding.destination == ArtifactDestination::Remote
                && sensitivity != Sensitivity::Normal)
        {
            return Err(refuse(Some(alias), RefusalCode::PolicyBlocked));
        }
        Ok(())
    }

    fn charge(&mut self, alias: &Alias, bytes: usize) -> Result<(), Refusal> {
        self.accounting
            .charge_render(Some(alias), u64::try_from(bytes).unwrap_or(u64::MAX))
    }

    /// Charges the range, loads the artifact once, checks the whole buffer on first load, and copies the requested range out of the retained buffer. The whole-buffer check means a range split can never hide a marker or secret.
    fn load_range(
        &mut self,
        store: &KernelStore,
        alias: &Alias,
        held: &HeldEvidence,
        range: Option<Range<u64>>,
    ) -> Result<Vec<u8>, Refusal> {
        let range = range.unwrap_or(0..held.byte_length);
        if range.end > held.byte_length {
            return Err(refuse(Some(alias), RefusalCode::InvalidRange));
        }
        self.charge(
            alias,
            usize::try_from(range.end - range.start).unwrap_or(usize::MAX),
        )?;
        let loaded = self
            .buffers
            .load(store, held)
            .map_err(|error| match error {
                RunBufferRefusal::CapacityExhausted => {
                    refuse(Some(alias), RefusalCode::BufferLimit)
                }
                RunBufferRefusal::Unreadable | RunBufferRefusal::RangeOutOfBounds => {
                    refuse(Some(alias), RefusalCode::ExpectationChanged)
                }
            })?;
        if loaded || !self.checked_artifacts.contains(&held.artifact_digest) {
            let whole = self
                .buffers
                .slice(&held.artifact_digest, 0..held.byte_length)
                .map_err(|_| refuse(Some(alias), RefusalCode::ExpectationChanged))?;
            check_render(whole, Some(alias))?;
            self.checked_artifacts.insert(held.artifact_digest.clone());
        }
        self.buffers
            .slice(&held.artifact_digest, range)
            .map(<[u8]>::to_vec)
            .map_err(|_| refuse(Some(alias), RefusalCode::InvalidRange))
    }

    /// Judges the descriptor, and the originating decision when there is one, for the run's destination under its project scope on the explicit-search surface. The decision's standing caps the descriptor's; the tag records the descriptor's own verdict and visibility.
    fn judge(
        &self,
        store: &KernelStore,
        alias: Option<&Alias>,
        object_id: &str,
        source_revision: i64,
        artifact_digest: &str,
        decision: Option<(&str, i64)>,
    ) -> Result<JudgedAt, Refusal> {
        let mut candidates = vec![EligibilityCandidate {
            object_id: object_id.to_string(),
            source_revision,
            artifact_digest: Some(artifact_digest.to_string()),
        }];
        if let Some((decision_id, decision_revision)) = decision {
            candidates.push(EligibilityCandidate {
                object_id: decision_id.to_string(),
                source_revision: decision_revision,
                artifact_digest: None,
            });
        }
        let batch = store
            .judge_surface_eligibility(
                &self.binding.project,
                self.binding.destination,
                Surface::ExplicitSearch,
                &candidates,
            )
            .map_err(|_| refuse(alias, RefusalCode::Store))?;
        // A snapshot taken during a classification change is not a grant.
        if !batch.is_reusable() {
            return Err(refuse(alias, RefusalCode::Store));
        }
        for (position, verdict) in batch.verdicts.iter().enumerate() {
            let is_decision = position == 1;
            let code = match verdict.verdict {
                EligibilityVerdict::Ok => None,
                // A descriptor is never a served object; its bytes are judged by the artifact egress fold. A decision no surface serves is not disclosable.
                EligibilityVerdict::Hidden if !is_decision => None,
                EligibilityVerdict::Hidden | EligibilityVerdict::ProviderSensitive => {
                    Some(RefusalCode::PolicyBlocked)
                }
                EligibilityVerdict::WrongScope => Some(RefusalCode::Scope),
                EligibilityVerdict::Retracted
                | EligibilityVerdict::Superseded
                | EligibilityVerdict::Stale => Some(if is_decision {
                    RefusalCode::OriginRevoked
                } else {
                    RefusalCode::ExpectationChanged
                }),
            };
            if let Some(code) = code {
                return Err(refuse(alias, code));
            }
        }
        let descriptor = batch.verdicts[0];
        Ok(JudgedAt {
            verdict: descriptor.verdict,
            visibility: descriptor.visibility,
            tip: batch.snapshot.tip,
        })
    }

    /// The descriptor observation of `object_id` at `tip`; a missing row, another observation kind, or an undecodable detail refuses.
    fn descriptor(
        &self,
        store: &KernelStore,
        alias: Option<&Alias>,
        object_id: &str,
        tip: i64,
    ) -> Result<SourceDescriptorDetail, Refusal> {
        let row = store
            .observation_for_object_as_of(object_id, tip)
            .map_err(|_| refuse(alias, RefusalCode::Store))?
            .filter(|row| row.observation_kind == SOURCE_DESCRIPTOR_KIND)
            .ok_or_else(|| refuse(alias, RefusalCode::ExpectationChanged))?;
        row.payload
            .detail
            .as_deref()
            .and_then(|detail| serde_json::from_str::<SourceDescriptorDetail>(detail).ok())
            .filter(|detail| detail.descriptor_version == kernel::SOURCE_DESCRIPTOR_DETAIL_VERSION)
            .ok_or_else(|| refuse(alias, RefusalCode::ExpectationChanged))
    }
}

fn staged_refusal(error: ReviewReadError) -> RefusalCode {
    match error {
        ReviewReadError::Refused(kernel::ReviewReadRefusal::ScopeMismatch)
        | ReviewReadError::Refused(kernel::ReviewReadRefusal::IncarnationMismatch) => {
            RefusalCode::Scope
        }
        ReviewReadError::Refused(_) | ReviewReadError::Invalid => RefusalCode::ExpectationChanged,
        ReviewReadError::Store(_) => RefusalCode::Store,
    }
}

fn hold_refusal(error: CuratorHoldError) -> RefusalCode {
    match error {
        CuratorHoldError::Refused(kernel::CuratorHoldRefusal::UnavailableEvidence)
        | CuratorHoldError::Refused(kernel::CuratorHoldRefusal::NotCovered) => {
            RefusalCode::ExpectationChanged
        }
        CuratorHoldError::Refused(kernel::CuratorHoldRefusal::ProjectBackingExhausted)
        | CuratorHoldError::Refused(kernel::CuratorHoldRefusal::HostBackingExhausted)
        | CuratorHoldError::Refused(kernel::CuratorHoldRefusal::TooManyReferences) => {
            RefusalCode::HoldLimit
        }
        CuratorHoldError::Refused(_) => RefusalCode::HoldInvalid,
        CuratorHoldError::Store(KernelError::Deadline) | CuratorHoldError::Store(_) => {
            RefusalCode::Store
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The requested range of `bytes`, or the whole slice when no range was asked for; an out-of-bounds range is `None`.
fn slice_range(bytes: &[u8], range: Option<&Range<u64>>) -> Option<Vec<u8>> {
    match range {
        None => Some(bytes.to_vec()),
        Some(range) => {
            let start = usize::try_from(range.start).ok()?;
            let end = usize::try_from(range.end).ok()?;
            bytes.get(start..end).map(<[u8]>::to_vec)
        }
    }
}
