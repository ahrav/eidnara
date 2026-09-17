//! One policy-validating broker between a Curator run and the stores. Every model-visible byte passes through it, and the broker never invents support: a reference resolves only through Kernel-owned expectations, a canonical and a promoted form of one decision count once, and an unknown or partial disclosure permits no usable conclusion.
//!
//! Aliases are run-local (Q15): issued for one `(job, receipt generation)`, never persisted, and a fabricated alias resolves to nothing before any effect. Accounting follows the resource table: 32 issued inspections, 8 operations per batch, 128 KiB of rendered model-visible bytes charged on every render (Q22), and one append-only 16 MiB buffer map per run (Q4). Every rendered buffer carries a provenance tag (Q16) and passes the detection-only render check first; a hit refuses with the alias and a bounded host-authored code, never a redacted substitute (Q7).

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use context_core::curator_policy_union::{PolicyUnion, PolicyUnionMember};
use context_core::redaction::{contains_redaction_token, reject_secret_text};
use kernel::source_identity::OccurrenceClass;
use kernel::{
    ArtifactDestination, CURATOR_CAPTURE_RETENTION_CLASS, CuratorHold, CuratorHoldBinding,
    CuratorHoldError, CuratorHoldKind, EligibilityCandidate, EligibilityVerdict, HeldEvidence,
    KernelError, KernelStore, OPERATOR_REDACTION_PLACEHOLDER, ProjectScope, ProviderEgress,
    ReviewBinding, ReviewPayload, ReviewReadError, ReviewStagedReference, RunBufferMap,
    RunBufferRefusal, SOURCE_DESCRIPTOR_KIND, Sensitivity, SourceDescriptorDetail, Surface,
};

pub const MAX_ISSUED_INSPECTIONS: usize = 32;
pub const MAX_OPERATIONS_PER_BATCH: usize = 8;
/// Rendered model-visible evidence bytes per run, charged on every render including resends.
pub const MAX_MODEL_VISIBLE_BYTES: u64 = 128 * 1024;
pub const MAX_RUN_BUFFER_BYTES: u64 = 16 * 1024 * 1024;
/// Per-match excerpt bound for search results (Q19), in bytes of the captured buffer.
pub const MAX_SEARCH_EXCERPT_BYTES: u64 = 512;
/// Per-search excerpt total (Q19).
pub const MAX_SEARCH_EXCERPT_TOTAL_BYTES: u64 = 8 * 1024;
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
    #[error("unsupported_question")]
    UnsupportedQuestion,
    #[error("store")]
    Store,
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
    /// The Remote verdict the bytes were judged under and the snapshot tip it was judged at; host-authored text carries `None`.
    pub remote_verdict: Option<(EligibilityVerdict, i64)>,
    pub render_check_passed: bool,
    pub charged_bytes: u64,
}

/// Rendered bytes with their tag; the bytes are owned by the render and borrowed by the assembler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedBuffer {
    pub bytes: Vec<u8>,
    pub tag: ProvenanceTag,
}

/// Detection-only render check over materialized bytes: a secret or a redaction placeholder refuses disclosure (Q7). Host-authored text passes through the same check so a template can never carry protected text.
pub fn check_render(bytes: &[u8], alias: Option<&Alias>) -> Result<(), Refusal> {
    let text = std::str::from_utf8(bytes).map_err(|_| refuse(alias, RefusalCode::RenderCheck))?;
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
    union: PolicyUnion,
}

impl DisclosureLedger {
    pub fn new(question: QuestionTemplate) -> Self {
        let mut ledger = Self::default();
        ledger.union.insert(PolicyUnionMember {
            kind: "question_template".to_string(),
            id: question.id().to_string(),
            revision: "1".to_string(),
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

    /// Whether a conclusion may be published: nothing is usable after an unknown or partial disclosure.
    pub fn conclusions_usable(&self) -> bool {
        !self.uncertain
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
        }
    }

    /// Lowers the inspection ceiling for tests (Q23); production keeps [`MAX_ISSUED_INSPECTIONS`].
    pub fn with_inspection_limit(mut self, max_inspections: usize) -> Self {
        self.accounting = InvestigationAccounting::with_inspection_limit(max_inspections);
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
                remote_verdict: None,
                render_check_passed: true,
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

    /// Reads one reference for disclosure: resolves the alias, grows the execution hold over the artifact, revalidates the Kernel expectation at `now`, loads the bytes once, renders the requested range, runs the render check, charges the bytes, and records the disclosure. Any refusal happens before bytes are retained or disclosed.
    pub fn read(
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
        let (bytes, verdict, sensitivity, member) = match &expectation {
            ReferenceExpectation::StagedSubject { reference, binding } => {
                let row = store
                    .read_review_input(reference, binding, now_ms)
                    .map_err(|error| refuse(Some(&alias), staged_refusal(error)))?;
                if binding.project_digest != self.binding.hold.project_digest {
                    return Err(refuse(Some(&alias), RefusalCode::Scope));
                }
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
                (
                    text.into_bytes(),
                    None,
                    row.sensitivity,
                    expectation.policy_member(None),
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
                let (verdict, tip) = self.judge(
                    store,
                    &alias,
                    object_id,
                    *source_revision,
                    artifact_digest,
                    None,
                )?;
                let detail = self.descriptor(store, &alias, object_id, tip)?;
                if detail.class != class.code()
                    || detail.occurrence_tuple != *occurrence_tuple
                    || detail.artifact_digest != *artifact_digest
                    || detail.evidence_id != *evidence_id
                {
                    return Err(refuse(Some(&alias), RefusalCode::ExpectationChanged));
                }
                let held = self.hold_evidence(store, &alias, evidence_id, now_ms)?;
                let bytes = self.load_range(store, &alias, &held, range.clone())?;
                (
                    bytes,
                    Some((verdict, tip)),
                    held.sensitivity,
                    expectation.policy_member(None),
                )
            }
            ReferenceExpectation::CanonicalSource {
                object_id,
                class,
                source_revision,
                artifact_digest,
                evidence_id,
                originating_decision_id,
                decision_source_revision,
            } => {
                // The originating decision's standing caps the descriptor's (Q34): a retracted, superseded, or hidden decision revokes every form derived from it.
                let (verdict, tip) = self.judge(
                    store,
                    &alias,
                    object_id,
                    *source_revision,
                    artifact_digest,
                    Some((originating_decision_id, *decision_source_revision)),
                )?;
                let detail = self.descriptor(store, &alias, object_id, tip)?;
                let decision_in_tuple = detail
                    .identity
                    .iter()
                    .any(|(_, value)| value == originating_decision_id);
                if detail.class != class.code()
                    || !decision_in_tuple
                    || detail.artifact_digest != *artifact_digest
                    || detail.evidence_id != *evidence_id
                {
                    return Err(refuse(Some(&alias), RefusalCode::ExpectationChanged));
                }
                let held = self.hold_evidence(store, &alias, evidence_id, now_ms)?;
                let bytes = self.load_range(store, &alias, &held, range.clone())?;
                (
                    bytes,
                    Some((verdict, tip)),
                    held.sensitivity,
                    expectation.policy_member(Some(*decision_source_revision)),
                )
            }
        };
        check_render(&bytes, Some(&alias))?;
        let charged = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        self.accounting.charge_render(Some(&alias), charged)?;
        self.ledger.record_disclosure(&alias, member.clone());
        let origin_key = expectation.origin_key();
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
                    remote_verdict: verdict,
                    render_check_passed: true,
                    charged_bytes: charged,
                },
            },
            alias,
            origin_key,
            member,
            sensitivity,
        })
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
        // Default-Sensitive policy: a remote model receives only Normal, remote-allowed evidence; a private read or a repository path proves nothing.
        if self.binding.destination == ArtifactDestination::Remote
            && (held.sensitivity != Sensitivity::Normal
                || held.provider_egress_class != ProviderEgress::RemoteAllowed.as_str())
        {
            return Err(refuse(Some(alias), RefusalCode::PolicyBlocked));
        }
        Ok(held)
    }

    /// Charges and loads the artifact once, then copies the requested range out of the retained buffer.
    fn load_range(
        &mut self,
        store: &KernelStore,
        alias: &Alias,
        held: &HeldEvidence,
        range: Option<Range<u64>>,
    ) -> Result<Vec<u8>, Refusal> {
        self.buffers
            .load(store, held)
            .map_err(|error| match error {
                RunBufferRefusal::CapacityExhausted => {
                    refuse(Some(alias), RefusalCode::BufferLimit)
                }
                RunBufferRefusal::Unreadable | RunBufferRefusal::RangeOutOfBounds => {
                    refuse(Some(alias), RefusalCode::ExpectationChanged)
                }
            })?;
        let range = range.unwrap_or(0..held.byte_length);
        self.buffers
            .slice(&held.artifact_digest, range)
            .map(<[u8]>::to_vec)
            .map_err(|_| refuse(Some(alias), RefusalCode::InvalidRange))
    }

    /// Judges the descriptor, and the originating decision when there is one, for Remote disclosure under the run's project scope on the explicit-search surface. The weaker verdict of the pair governs.
    fn judge(
        &self,
        store: &KernelStore,
        alias: &Alias,
        object_id: &str,
        source_revision: i64,
        artifact_digest: &str,
        decision: Option<(&str, i64)>,
    ) -> Result<(EligibilityVerdict, i64), Refusal> {
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
            .map_err(|_| refuse(Some(alias), RefusalCode::Store))?;
        for (candidate, verdict) in candidates.iter().zip(&batch.verdicts) {
            let is_decision = decision.is_some_and(|(id, _)| id == candidate.object_id);
            let code = match verdict.verdict {
                EligibilityVerdict::Ok => None,
                // A descriptor is never a served object; its disclosure is judged by its evidence below. A decision no surface serves is not disclosable.
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
                return Err(refuse(Some(alias), code));
            }
        }
        Ok((EligibilityVerdict::Ok, batch.snapshot.tip))
    }

    /// The descriptor observation of `object_id` at `tip`; anything but exactly one live descriptor row refuses.
    fn descriptor(
        &self,
        store: &KernelStore,
        alias: &Alias,
        object_id: &str,
        tip: i64,
    ) -> Result<SourceDescriptorDetail, Refusal> {
        let rows = store
            .observations_for_object_as_of(object_id, tip)
            .map_err(|_| refuse(Some(alias), RefusalCode::Store))?;
        let mut descriptors = rows
            .into_iter()
            .filter(|row| row.observation_kind == SOURCE_DESCRIPTOR_KIND);
        let (Some(row), None) = (descriptors.next(), descriptors.next()) else {
            return Err(refuse(Some(alias), RefusalCode::ExpectationChanged));
        };
        row.payload
            .detail
            .as_deref()
            .and_then(|detail| serde_json::from_str::<SourceDescriptorDetail>(detail).ok())
            .ok_or_else(|| refuse(Some(alias), RefusalCode::ExpectationChanged))
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
            RefusalCode::BufferLimit
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

/// A hold the coordinator acquired before the first read; the broker only grows it.
pub fn run_binding(
    project: ProjectScope,
    hold: &CuratorHold,
    binding: CuratorHoldBinding,
) -> RunBinding {
    RunBinding {
        project,
        hold: binding,
        hold_id: hold.hold_id.clone(),
        destination: ArtifactDestination::Remote,
    }
}
