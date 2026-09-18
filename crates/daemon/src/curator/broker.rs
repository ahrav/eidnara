//! One policy-validating broker between a Curator run and the stores. Every model-visible byte passes through it, and the broker never invents support: a reference resolves only through Kernel-owned expectations, a canonical and a promoted form of one decision count once, and an unknown or partial disclosure permits no usable conclusion.
//!
//! Consistency model of a read: every check (scope, hold, eligibility, descriptor, egress) is a separate Kernel snapshot taken inside the one `read` call, and the first disk load is the only wait between them, so the hold, evidence facts, object standing, and egress verdict are re-read after it. The remaining windows are between consecutive Kernel reads with no I/O between them; they cannot be closed from this crate and are not narrowed by reordering. Closing them means one Kernel transaction that validates the hold, judges eligibility, and returns the egress verdict together, which is a Kernel API, not a broker change.
//!
//! Aliases are run-local (Q15): issued for one `(job, receipt generation)`, never persisted, and a fabricated alias resolves to nothing before any effect. Accounting follows the resource table: 32 issued inspections, 8 operations per batch, 128 KiB of rendered model-visible bytes charged on every render (Q22), and one append-only 16 MiB buffer map per run (Q4). Every rendered buffer carries a provenance tag (Q16) and passes the detection-only render check first; a hit refuses with the alias and a bounded host-authored code, never a redacted substitute (Q7).

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

use context_core::curator_policy_union::{PolicyUnion, PolicyUnionMember};
use context_core::redaction::{
    contains_redaction_token, detect_windowed_durable_bytes, reject_secret_text,
};
use kernel::source_identity::OccurrenceClass;
use kernel::{
    ArtifactDestination, ArtifactEligibility, ArtifactErrorKind, ArtifactHandle,
    CURATOR_CAPTURE_RETENTION_CLASS, CuratorHold, CuratorHoldBinding, CuratorHoldError,
    CuratorHoldKind, DecisionRow, EligibilityCandidate, EligibilityVerdict, HeldEvidence,
    KernelError, KernelStore, MAX_RUN_BUFFER_BYTES, OPERATOR_REDACTION_PLACEHOLDER, ProjectScope,
    ReviewBinding, ReviewOwner, ReviewPayload, ReviewReadError, ReviewStagedReference,
    RunBufferMap, RunBufferRefusal, SOURCE_DESCRIPTOR_KIND, Sensitivity, SourceDescriptorDetail,
    Surface, SurfaceVisibility,
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

    /// Only the fixed identifiers resolve; any other input, including one carrying question text, is refused with [`RefusalCode::UnsupportedQuestion`].
    pub fn parse(id: &str) -> Result<Self, Refusal> {
        (id == Self::ExtractedFacts.id())
            .then_some(Self::ExtractedFacts)
            .ok_or_else(|| refuse(None, RefusalCode::UnsupportedQuestion))
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
    /// The evidence this reference names; a staged subject is a Kernel row, not evidence.
    pub fn evidence_id(&self) -> Option<&str> {
        match self {
            Self::StagedSubject { .. } => None,
            Self::NativeSource { evidence_id, .. }
            | Self::CanonicalSource { evidence_id, .. }
            | Self::TemporaryCapture { evidence_id, .. } => Some(evidence_id),
        }
    }

    pub fn origin(&self) -> OriginClass {
        match self {
            Self::StagedSubject { .. } => OriginClass::StagedSubject,
            Self::NativeSource { .. } => OriginClass::NativeSource,
            Self::CanonicalSource { .. } => OriginClass::CanonicalSource,
            Self::TemporaryCapture { .. } => OriginClass::TemporaryCapture,
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
    #[error("invalid_path")]
    InvalidPath,
    #[error("protected")]
    Protected,
    #[error("not_regular_file")]
    NotRegularFile,
    #[error("confinement")]
    Confinement,
    #[error("unsupported")]
    Unsupported,
    #[error("not_found")]
    NotFound,
    #[error("too_large")]
    TooLarge,
    #[error("unavailable")]
    Unavailable,
    #[error("unsupported_question")]
    UnsupportedQuestion,
    #[error("store")]
    Store,
}

impl RefusalCode {
    /// Whether the refusal came from a run or batch ceiling rather than from the reference itself; any other refusal is a fact about one reference. A caller walking many references stops at a capacity refusal and skips past any other.
    pub const fn is_capacity(self) -> bool {
        self.truncates_evidence() || matches!(self, Self::BatchLimit)
    }

    /// Whether the refusal left evidence the run asked for undisclosed for want of capacity, so a conclusion drawn afterwards rests on a truncated set. A batch bound only paces the run and is not counted.
    pub const fn truncates_evidence(self) -> bool {
        matches!(
            self,
            Self::InspectionLimit | Self::ByteLimit | Self::BufferLimit | Self::HoldLimit
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

pub(crate) fn refuse(alias: Option<&Alias>, code: RefusalCode) -> Refusal {
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

/// Rendered bytes, their tag, and the identity of the broker that rendered them. The bytes are owned by the render and moved into the assembler once, so a buffer cannot enter a body more times than the broker charged it; the broker stamp lets the assembler refuse a buffer another broker admitted, since aliases are broker-local and two brokers over one hold can both issue `ref-1`.
#[derive(Debug, PartialEq, Eq)]
pub struct RenderedBuffer {
    pub(crate) bytes: Vec<u8>,
    pub(crate) tag: ProvenanceTag,
    pub(crate) broker: BrokerId,
}

impl RenderedBuffer {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn tag(&self) -> &ProvenanceTag {
        &self.tag
    }

    /// The broker that rendered this buffer.
    pub fn broker(&self) -> BrokerId {
        self.broker
    }

    /// The same render for a later body of the same run. The caller charges the bytes again through [`InvestigationAccounting::charge_render`] before the copy is sent (Q22), so a resend costs what the first render did.
    pub fn resend(&self) -> Self {
        Self {
            bytes: self.bytes.clone(),
            tag: self.tag.clone(),
            broker: self.broker,
        }
    }
}

/// Detection-only render check over bytes about to be rendered: a secret or a redaction placeholder refuses disclosure (Q7). Host-authored text passes through the same check so a template can never carry protected text. Rendered ranges are bounded by [`MAX_MODEL_VISIBLE_BYTES`], well inside the scanner's single-pass input limit.
pub fn check_render(bytes: &[u8], alias: Option<&Alias>) -> Result<(), Refusal> {
    let text = std::str::from_utf8(bytes).map_err(|_| refuse(alias, RefusalCode::Undecodable))?;
    if contains_redaction_marker(text) || reject_secret_text(text).is_err() {
        return Err(refuse(alias, RefusalCode::RenderCheck));
    }
    Ok(())
}

/// The whole-artifact form of [`check_render`]: an artifact may exceed the scanner's single-pass input limit, so secrets are detected in windows, the same way ingest checked the bytes. A scan that cannot prove the buffer secret-free refuses.
pub(crate) fn check_whole_artifact(bytes: &[u8], alias: Option<&Alias>) -> Result<(), Refusal> {
    let text = std::str::from_utf8(bytes).map_err(|_| refuse(alias, RefusalCode::Undecodable))?;
    if contains_redaction_marker(text) || detect_windowed_durable_bytes(bytes) != Ok(false) {
        return Err(refuse(alias, RefusalCode::RenderCheck));
    }
    Ok(())
}

/// A scanner token or the operator placeholder: text the redactor already rewrote, which must never render as if it were source.
fn contains_redaction_marker(text: &str) -> bool {
    contains_redaction_token(text) || text.contains(OPERATOR_REDACTION_PLACEHOLDER)
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
    /// The byte ranges of each disclosed alias the model was shown, one per render; a citation span must lie within one of them.
    rendered: BTreeMap<Alias, Vec<Range<u64>>>,
    cited: BTreeSet<Alias>,
    /// Set when any attempt's outcome is unknown: bytes may have reached the model without a recorded disclosure.
    uncertain: bool,
    /// Set when a capacity refusal withheld requested evidence: the model reasoned over a truncated evidence set.
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
    pub fn record_disclosure(
        &mut self,
        alias: &Alias,
        member: PolicyUnionMember,
        rendered: Range<u64>,
    ) {
        self.disclosed.insert(alias.clone());
        self.rendered
            .entry(alias.clone())
            .or_default()
            .push(rendered);
        self.union.insert(member);
    }

    /// Whether `start..end` names at least one byte and lies within one range rendered under `alias`.
    pub fn covers(&self, alias: &Alias, start: u64, end: u64) -> bool {
        start < end
            && self.rendered.get(alias).is_some_and(|ranges| {
                ranges
                    .iter()
                    .any(|range| range.start <= start && end <= range.end)
            })
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

    pub fn is_disclosed(&self, alias: &Alias) -> bool {
        self.disclosed.contains(alias)
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

    /// A capacity refusal leaves the evidence set truncated whether it came before or after a disclosure: the run asked for evidence it will not see.
    pub fn record_partial_disclosure(&mut self) {
        self.partial = true;
    }

    /// Whether a conclusion may be published: nothing is usable after an unknown or partial disclosure.
    pub fn conclusions_usable(&self) -> bool {
        !self.uncertain && !self.partial
    }

    /// Whether any attempt's outcome is unknown.
    pub fn is_uncertain(&self) -> bool {
        self.uncertain
    }

    /// Whether a capacity refusal truncated the disclosed evidence set.
    pub fn is_partial(&self) -> bool {
        self.partial
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
        Self::bounded(MAX_ISSUED_INSPECTIONS)
    }

    /// A lower inspection ceiling for tests, per Q23; the production bound is the ceiling of the ceiling.
    #[cfg(feature = "test-support")]
    pub fn with_inspection_limit(max_inspections: usize) -> Self {
        Self::bounded(max_inspections.min(MAX_ISSUED_INSPECTIONS))
    }

    fn bounded(max_inspections: usize) -> Self {
        Self {
            issued_inspections: 0,
            batch_operations: 0,
            model_visible_bytes: 0,
            max_inspections,
        }
    }

    /// Admits one operation of the current batch; refused-after-admission operations count, decode-rejected steps do not.
    pub fn admit_operation(&mut self, alias: Option<&Alias>) -> Result<(), Refusal> {
        self.admit_check(alias)?;
        self.batch_operations += 1;
        self.issued_inspections += 1;
        Ok(())
    }

    /// The refusal [`Self::admit_operation`] would give right now, without admitting anything; a caller that must do work before its operation is admitted asks first so a refused operation costs nothing.
    pub fn admit_check(&self, alias: Option<&Alias>) -> Result<(), Refusal> {
        if self.batch_operations >= MAX_OPERATIONS_PER_BATCH {
            return Err(refuse(alias, RefusalCode::BatchLimit));
        }
        if self.issued_inspections >= self.max_inspections {
            return Err(refuse(alias, RefusalCode::InspectionLimit));
        }
        Ok(())
    }

    pub fn end_batch(&mut self) {
        self.batch_operations = 0;
    }

    /// Operations the current batch can still admit. A caller that stops at zero ends its work with what it has instead of provoking a `BatchLimit` refusal.
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

    /// Returns a charge taken for a send that never left the host, so the bound counts only bytes a model could have seen.
    pub fn refund_render(&mut self, bytes: u64) {
        self.model_visible_bytes = self.model_visible_bytes.saturating_sub(bytes);
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

/// What judging a canonical reference established: the Kernel's judgement, the live descriptor detail, and the stricter of the descriptor row's and the originating decision's classes, which labels the bytes together with the artifact's own.
struct CanonicalJudgement {
    judged: JudgedAt,
    detail: SourceDescriptorDetail,
    sensitivity: Sensitivity,
}

/// What a read-only probe of a canonical source produced.
#[derive(Debug)]
pub(crate) enum Probed {
    /// The whole artifact, judged, policy-checked, and render-checked but not disclosed, and the range of it the descriptor's span selects; only those bytes carry the descriptor's provenance.
    Bytes {
        bytes: Vec<u8>,
        selected: Range<usize>,
    },
    /// The artifact is longer than the caller's bound; nothing was read.
    TooLarge { byte_length: u64 },
    /// The artifact was read but cannot be disclosed: it is unreadable or fails the render check. The bytes it cost are reported so a caller can charge them.
    Refused { byte_length: u64 },
}

/// What one successful read returns to the coordinator: the rendered buffer, the origin key two forms of one decision share, and the lineage member the disclosure added.
#[derive(Debug, PartialEq, Eq)]
pub struct EvidenceRead {
    pub alias: Alias,
    pub buffer: RenderedBuffer,
    pub origin_key: String,
    pub member: PolicyUnionMember,
    /// The restrictive fold of the stored classes behind the bytes: the artifact's, the descriptor row's, and the originating decision's. The serving-view class eligibility judged against (which folds admission history and can be stricter) is not exposed by the Kernel's verdict; the verdict decided whether the bytes may reach this destination, this field labels them.
    pub sensitivity: Sensitivity,
}

/// The run's Kernel-side bindings: the execution hold every disclosed byte must be protected by, whose project is the run's project.
#[derive(Debug, Clone)]
pub struct RunBinding {
    pub hold: CuratorHoldBinding,
    /// Empty when the run acquired no execution hold; settlement then releases none.
    pub hold_id: String,
    /// Where disclosed bytes go. A remote model admits only `Normal`, remote-allowed evidence; unproven sources stay policy-blocked there.
    pub destination: ArtifactDestination,
}

/// Which hold a revalidation runs under: the run's execution hold, or the review hold the settlement owns.
#[derive(Debug, Clone, Copy)]
pub enum HeldUnder<'a> {
    Execution(&'a RunBinding),
    Review {
        hold: &'a CuratorHold,
        binding: &'a CuratorHoldBinding,
    },
}

impl HeldUnder<'_> {
    pub fn kind(&self) -> CuratorHoldKind {
        match self {
            Self::Execution(_) => CuratorHoldKind::Execution,
            Self::Review { .. } => CuratorHoldKind::Review,
        }
    }

    pub fn hold_id(&self) -> &str {
        match self {
            Self::Execution(run) => &run.hold_id,
            Self::Review { hold, .. } => &hold.hold_id,
        }
    }

    pub fn binding(&self) -> &CuratorHoldBinding {
        match self {
            Self::Execution(run) => &run.hold,
            Self::Review { binding, .. } => binding,
        }
    }

    /// The review hold's expiry; an execution hold has no moved reference.
    fn moved_reference(&self) -> Option<i64> {
        match self {
            Self::Execution(_) => None,
            Self::Review { hold, .. } => Some(hold.expires_at),
        }
    }
}

/// Process-local identity of one broker: its alias table's. A hold id cannot serve, because the Kernel hands the same live hold back to every acquisition under one binding, so two brokers over one hold would share it while their aliases name different evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BrokerId(u64);

static NEXT_BROKER: AtomicU64 = AtomicU64::new(1);

/// The broker: aliases, accounting, buffers, ledger, and the reads that tie them to Kernel expectations.
pub struct EvidenceBroker {
    id: BrokerId,
    pub aliases: AliasTable,
    pub accounting: InvestigationAccounting,
    pub buffers: RunBufferMap,
    pub ledger: DisclosureLedger,
    binding: RunBinding,
    /// The scope eligibility is judged under; derived from the hold's project so the two can never name different projects.
    project: ProjectScope,
    /// First alias disclosed per origin key, so a second form of one decision is reported as the same origin rather than fresh support.
    origins: BTreeMap<String, Alias>,
    /// The origin key each disclosure recorded; `shared_origin` reads this rather than recomputing a key from the expectation.
    origin_keys: BTreeMap<Alias, String>,
    /// Artifacts whose whole buffer passed the render check, so a range split cannot hide a marker or secret across two reads.
    checked_artifacts: BTreeSet<String>,
    /// Artifacts whose whole buffer failed the render check; later reads refuse without a charge or another scan.
    refused_artifacts: BTreeSet<String>,
    /// Evidence already added to the execution hold by this run; a re-render skips the writer transaction.
    extended: BTreeSet<String>,
    /// Runs between the disk read and the post-load egress verdict, so a test can commit a classification change inside that window.
    #[cfg(feature = "test-support")]
    after_load_for_test: Option<AfterLoadHook>,
}

/// A test hook given the store the broker is reading from.
#[cfg(feature = "test-support")]
pub type AfterLoadHook = Box<dyn FnMut(&KernelStore) + Send + Sync>;

impl EvidenceBroker {
    /// Refuses a hold binding whose project digest is not a digest; every other check on it is the Kernel's.
    pub fn new(binding: RunBinding, question: QuestionTemplate) -> Result<Self, KernelError> {
        let project = ProjectScope::new(&binding.hold.project_digest)?;
        Ok(Self {
            id: BrokerId(NEXT_BROKER.fetch_add(1, Ordering::Relaxed)),
            aliases: AliasTable::default(),
            accounting: InvestigationAccounting::new(),
            buffers: RunBufferMap::new(MAX_RUN_BUFFER_BYTES),
            ledger: DisclosureLedger::new(question),
            binding,
            project,
            origins: BTreeMap::new(),
            origin_keys: BTreeMap::new(),
            checked_artifacts: BTreeSet::new(),
            refused_artifacts: BTreeSet::new(),
            extended: BTreeSet::new(),
            #[cfg(feature = "test-support")]
            after_load_for_test: None,
        })
    }

    /// Lowers the inspection ceiling for tests (Q23); a value above [`MAX_ISSUED_INSPECTIONS`] is clamped to it.
    #[cfg(feature = "test-support")]
    pub fn with_inspection_limit(mut self, max_inspections: usize) -> Self {
        self.accounting = InvestigationAccounting::with_inspection_limit(max_inspections);
        self
    }

    /// Lowers the run buffer ceiling for tests; [`RunBufferMap::new`] clamps to the 16 MiB bound.
    #[cfg(feature = "test-support")]
    pub fn with_buffer_limit(mut self, capacity: u64) -> Self {
        self.buffers = RunBufferMap::new(capacity);
        self
    }

    /// Installs a hook that runs after an artifact's bytes are read from disk and before the post-load egress verdict.
    #[cfg(feature = "test-support")]
    pub fn with_after_load_hook_for_test(mut self, hook: AfterLoadHook) -> Self {
        self.after_load_for_test = Some(hook);
        self
    }

    /// The execution hold every disclosed byte is protected by.
    pub fn hold_id(&self) -> &str {
        &self.binding.hold_id
    }

    /// This broker's identity, stamped on every buffer it renders.
    pub fn id(&self) -> BrokerId {
        self.id
    }

    /// The run's Kernel-side bindings.
    pub fn binding(&self) -> &RunBinding {
        &self.binding
    }

    /// The refusal a disclosure would meet at the run's batch and inspection ceilings right now, without admitting anything. A caller that must store or hold before its read asks here first so a refused read costs nothing; a refusal that truncates the evidence set marks the ledger partial, as the read itself would have.
    pub fn admit_check(&mut self, alias: Option<&Alias>) -> Result<(), Refusal> {
        let result = self.accounting.admit_check(alias);
        if let Err(refusal) = &result
            && refusal.code.truncates_evidence()
        {
            self.ledger.record_partial_disclosure();
        }
        result
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
            broker: self.id,
        })
    }

    /// The alias whose origin `alias` shares, when an earlier disclosure already resolved the same decision, source, or row. An alias that has not been disclosed yet has no recorded origin and shares nothing.
    pub fn shared_origin(&self, alias: &str) -> Result<Option<&Alias>, Refusal> {
        let (alias, _) = self.aliases.resolve(alias)?;
        Ok(self
            .origin_keys
            .get(alias)
            .and_then(|key| self.origins.get(key))
            .filter(|first| *first != alias))
    }

    /// Reads one reference for disclosure: resolves the alias, checks the destination verdict, grows the execution hold over the artifact, revalidates the Kernel expectation at `now` (never earlier than the wall clock, so a stale run timestamp cannot revive an expired reference), loads the bytes once, renders the requested range, runs the render check, charges the bytes, and records the disclosure. Any refusal happens before bytes are retained or disclosed, and a refusal that truncates the evidence set ([`RefusalCode::truncates_evidence`]) marks the ledger partial.
    pub fn read(
        &mut self,
        store: &KernelStore,
        alias: &str,
        range: Option<Range<u64>>,
        now_ms: i64,
    ) -> Result<EvidenceRead, Refusal> {
        let result = self.read_inner(store, alias, range, now_ms);
        if let Err(refusal) = &result
            && refusal.code.truncates_evidence()
        {
            self.ledger.record_partial_disclosure();
        }
        result
    }

    fn read_inner(
        &mut self,
        store: &KernelStore,
        alias: &str,
        range: Option<Range<u64>>,
        now_ms: i64,
    ) -> Result<EvidenceRead, Refusal> {
        let now_ms = now_ms.max(crate::now_ms());
        let (alias, expectation) = self
            .aliases
            .resolve(alias)
            .map(|(alias, expectation)| (alias.clone(), expectation.clone()))?;
        self.accounting.admit_operation(Some(&alias))?;
        self.ledger.record_inspection(&alias);
        if range.as_ref().is_some_and(|range| range.end <= range.start) {
            return Err(refuse(Some(&alias), RefusalCode::InvalidRange));
        }
        let (bytes, rendered, verdict, sensitivity, member, origin_key) = match &expectation {
            ReferenceExpectation::StagedSubject { reference, binding } => {
                // A staged row is protected by the run's live execution hold like every artifact, and belongs to the job the hold names.
                if binding.project_digest != self.binding.hold.project_digest
                    || !matches!(&binding.owner, ReviewOwner::Job { job_id } if *job_id == self.binding.hold.subject)
                {
                    return Err(refuse(Some(&alias), RefusalCode::Scope));
                }
                store
                    .validate_held_evidence(
                        &self.binding.hold_id,
                        CuratorHoldKind::Execution,
                        &self.binding.hold,
                        &[],
                        now_ms,
                    )
                    .map_err(|error| refuse(Some(&alias), hold_refusal(error)))?;
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
                let rendered = range
                    .clone()
                    .unwrap_or(0..u64::try_from(text.len()).unwrap_or(u64::MAX));
                let bytes = slice_range(text.as_bytes(), range.as_ref())
                    .ok_or_else(|| refuse(Some(&alias), RefusalCode::InvalidRange))?;
                self.charge(&alias, bytes.len())?;
                (
                    bytes,
                    rendered,
                    None,
                    row.sensitivity,
                    expectation.policy_member(None),
                    format!("staged:{}", reference.candidate_id),
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
                let mut held =
                    self.hold_evidence(store, Some(&alias), evidence_id, artifact_digest, now_ms)?;
                if held.byte_length != *byte_length
                    || held.retention_class != CURATOR_CAPTURE_RETENTION_CLASS
                    || held.retain_until != Some(*retain_until)
                {
                    return Err(refuse(Some(&alias), RefusalCode::ExpectationChanged));
                }
                let (bytes, _, rendered) =
                    self.load_range(store, &alias, &mut held, range.clone(), now_ms)?;
                (
                    bytes,
                    rendered,
                    None,
                    held.sensitivity,
                    expectation.policy_member(None),
                    format!("capture:{evidence_id}"),
                )
            }
            ReferenceExpectation::NativeSource {
                class,
                artifact_digest,
                evidence_id,
                ..
            } => {
                let (judged, detail, descriptor_class) =
                    self.judge_native_source(store, Some(&alias), &expectation)?;
                let range = span_range(&alias, detail.span, range)?;
                let mut held =
                    self.hold_evidence(store, Some(&alias), evidence_id, artifact_digest, now_ms)?;
                let (bytes, loaded, rendered) =
                    self.load_range(store, &alias, &mut held, range, now_ms)?;
                let judged = if loaded {
                    self.judge_native_source(store, Some(&alias), &expectation)?
                        .0
                } else {
                    judged
                };
                (
                    bytes,
                    rendered,
                    Some(judged),
                    held.sensitivity.restrictive(descriptor_class),
                    expectation.policy_member(None),
                    native_origin_key(*class, &detail.identity),
                )
            }
            ReferenceExpectation::CanonicalSource {
                artifact_digest,
                evidence_id,
                originating_decision_id,
                decision_source_revision,
                ..
            } => {
                let judgement = self.judge_canonical_source(store, Some(&alias), &expectation)?;
                let range = span_range(&alias, judgement.detail.span, range)?;
                let mut held =
                    self.hold_evidence(store, Some(&alias), evidence_id, artifact_digest, now_ms)?;
                let (bytes, loaded, rendered) =
                    self.load_range(store, &alias, &mut held, range, now_ms)?;
                let judgement = if loaded {
                    self.judge_canonical_source(store, Some(&alias), &expectation)?
                } else {
                    judgement
                };
                (
                    bytes,
                    rendered,
                    Some(judgement.judged),
                    held.sensitivity.restrictive(judgement.sensitivity),
                    expectation.policy_member(Some(*decision_source_revision)),
                    format!("decision:{originating_decision_id}"),
                )
            }
        };
        check_render(&bytes, Some(&alias))?;
        let charged = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        self.ledger
            .record_disclosure(&alias, member.clone(), rendered);
        self.origins
            .entry(origin_key.clone())
            .or_insert_with(|| alias.clone());
        self.origin_keys.insert(alias.clone(), origin_key.clone());
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
                broker: self.id,
            },
            alias,
            origin_key,
            member,
            sensitivity,
        })
    }

    /// Reads a canonical source's artifact without disclosing it: the same Kernel judgement, descriptor check, egress verdict, and render check as [`Self::read`], but no alias, hold, retained buffer, charge, or ledger entry. Related-memory discovery uses it to test relatedness; a candidate that passes is then disclosed through `read`, which revalidates. The whole artifact is read and render-checked and returned with the range its descriptor's span selects, so a caller charges the bytes read and matches only the selected ones. `Probed::TooLarge` reports an artifact longer than `max_bytes` before any byte is read; `Probed::Refused` reports one that was read and then refused, with the bytes it cost. Only canonical sources are probed; any other expectation is refused as unsupported.
    pub(crate) fn probe_canonical_source(
        &self,
        store: &KernelStore,
        expectation: &ReferenceExpectation,
        max_bytes: u64,
    ) -> Result<Probed, Refusal> {
        let detail = self
            .judge_canonical_source(store, None, expectation)?
            .detail;
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
        let selected = match detail.span {
            None => 0..bytes.len(),
            Some((start, end)) => usize::try_from(start)
                .ok()
                .zip(usize::try_from(end).ok())
                .filter(|(start, end)| start <= end && *end <= bytes.len())
                .map(|(start, end)| start..end)
                .ok_or_else(|| refuse(None, RefusalCode::ExpectationChanged))?,
        };
        Ok(Probed::Bytes { bytes, selected })
    }

    /// Judges a native descriptor and checks the live descriptor detail against the expectation's class, identity tuple, digest, and evidence. A decision-derived class is refused here: it resolves only through its originating decision, and issued as native it would outlive a retraction. Any other expectation kind is refused as unsupported. Returns the judgement, the detail, and the descriptor's own class.
    fn judge_native_source(
        &self,
        store: &KernelStore,
        alias: Option<&Alias>,
        expectation: &ReferenceExpectation,
    ) -> Result<(JudgedAt, SourceDescriptorDetail, Sensitivity), Refusal> {
        let ReferenceExpectation::NativeSource {
            object_id,
            class,
            source_revision,
            artifact_digest,
            evidence_id,
            occurrence_tuple,
        } = expectation
        else {
            return Err(refuse(alias, RefusalCode::UnsupportedQuestion));
        };
        if decision_derived(*class) {
            return Err(refuse(alias, RefusalCode::ExpectationChanged));
        }
        let judged = self.judge(
            store,
            alias,
            object_id,
            *source_revision,
            artifact_digest,
            None,
        )?;
        let (detail, descriptor_class) = self.descriptor(store, alias, object_id, judged.tip)?;
        if detail.class != class.code()
            || detail.occurrence_tuple != *occurrence_tuple
            || detail.artifact_digest != *artifact_digest
            || detail.evidence_id != *evidence_id
        {
            return Err(refuse(alias, RefusalCode::ExpectationChanged));
        }
        Ok((judged, detail, descriptor_class))
    }

    /// Re-judges one disclosed reference at `now` (never earlier than the wall clock) without loading, holding, or charging anything: the same Kernel expectation, scope, hold, and egress checks `read` applied, so a reference reclassified, retired, or expired since it was rendered refuses before the prepared body is disclosed. Returns the evidence id the reference names, when it names one; the caller validates the execution hold over the whole set.
    pub fn revalidate(
        &self,
        store: &KernelStore,
        alias: &str,
        now_ms: i64,
    ) -> Result<Option<String>, Refusal> {
        self.revalidate_under(store, alias, now_ms, HeldUnder::Execution(&self.binding))
    }

    /// [`Self::revalidate`] with the hold that protects the disclosed inputs named explicitly: the execution hold while the run investigates, or the review hold once settlement has moved retention there and the execution hold is released.
    pub fn revalidate_under(
        &self,
        store: &KernelStore,
        alias: &str,
        now_ms: i64,
        hold: HeldUnder<'_>,
    ) -> Result<Option<String>, Refusal> {
        let now_ms = now_ms.max(crate::now_ms());
        let (alias, expectation) = self.aliases.resolve(alias)?;
        match expectation {
            ReferenceExpectation::StagedSubject { reference, binding } => {
                if binding.project_digest != self.binding.hold.project_digest
                    || !matches!(&binding.owner, ReviewOwner::Job { job_id } if *job_id == self.binding.hold.subject)
                {
                    return Err(refuse(Some(alias), RefusalCode::Scope));
                }
                store
                    .validate_held_evidence(
                        hold.hold_id(),
                        hold.kind(),
                        hold.binding(),
                        &[],
                        now_ms,
                    )
                    .map_err(|error| refuse(Some(alias), hold_refusal(error)))?;
                let row = store
                    .read_review_input(reference, binding, now_ms)
                    .map_err(|error| refuse(Some(alias), staged_refusal(error)))?;
                self.gate_destination(alias, row.sensitivity)?;
                Ok(None)
            }
            ReferenceExpectation::TemporaryCapture {
                evidence_id,
                artifact_digest,
                byte_length,
                retain_until,
            } => {
                // The reference in force: the one the alias was issued with, or the review expiry the transfer moved it to.
                if hold.moved_reference().unwrap_or(*retain_until) <= now_ms {
                    return Err(refuse(Some(alias), RefusalCode::ExpectationChanged));
                }
                let held = store
                    .validate_held_evidence(
                        hold.hold_id(),
                        hold.kind(),
                        hold.binding(),
                        std::slice::from_ref(evidence_id),
                        now_ms,
                    )
                    .map_err(|error| refuse(Some(alias), hold_refusal(error)))?
                    .pop()
                    .ok_or_else(|| refuse(Some(alias), RefusalCode::HoldInvalid))?;
                // The hold transfer moves a capture's acquisition reference to the review expiry; under the review hold, that is the other value the reference may legitimately hold. Whichever it is, it must still be ahead of the clock: a reference that had lapsed before the transfer was not moved.
                let reference_stands = held.retain_until.is_some_and(|stored| stored > now_ms)
                    && (held.retain_until == Some(*retain_until)
                        || hold
                            .moved_reference()
                            .is_some_and(|moved| held.retain_until == Some(moved)));
                if held.artifact_digest != *artifact_digest
                    || held.byte_length != *byte_length
                    || held.retention_class != CURATOR_CAPTURE_RETENTION_CLASS
                    || !reference_stands
                {
                    return Err(refuse(Some(alias), RefusalCode::ExpectationChanged));
                }
                self.egress_allowed(
                    store,
                    Some(alias),
                    &ArtifactHandle {
                        digest: artifact_digest.clone(),
                        evidence_id: evidence_id.clone(),
                    },
                )?;
                Ok(Some(evidence_id.clone()))
            }
            ReferenceExpectation::NativeSource {
                artifact_digest,
                evidence_id,
                ..
            } => {
                self.judge_native_source(store, Some(alias), expectation)?;
                self.egress_allowed(
                    store,
                    Some(alias),
                    &ArtifactHandle {
                        digest: artifact_digest.clone(),
                        evidence_id: evidence_id.clone(),
                    },
                )?;
                Ok(Some(evidence_id.clone()))
            }
            ReferenceExpectation::CanonicalSource {
                artifact_digest,
                evidence_id,
                ..
            } => {
                self.judge_canonical_source(store, Some(alias), expectation)?;
                self.egress_allowed(
                    store,
                    Some(alias),
                    &ArtifactHandle {
                        digest: artifact_digest.clone(),
                        evidence_id: evidence_id.clone(),
                    },
                )?;
                Ok(Some(evidence_id.clone()))
            }
        }
    }

    /// Judges a canonical descriptor together with its originating decision, requires the owner to be a decision row at that snapshot, and checks the live descriptor detail against the expectation. The decision's standing caps the descriptor's (Q34): a retracted, superseded, or hidden decision revokes every form derived from it. Any other expectation kind is refused as unsupported.
    fn judge_canonical_source(
        &self,
        store: &KernelStore,
        alias: Option<&Alias>,
        expectation: &ReferenceExpectation,
    ) -> Result<CanonicalJudgement, Refusal> {
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
        // A canonical reference names a form of one decision; any other class is refused as native, where a retraction is not consulted.
        if !decision_derived(*class) {
            return Err(refuse(alias, RefusalCode::ExpectationChanged));
        }
        // The originating decision's standing caps the descriptor's (Q34): a retracted, superseded, or hidden decision revokes every form derived from it.
        let judged = self.judge(
            store,
            alias,
            object_id,
            *source_revision,
            artifact_digest,
            Some((originating_decision_id, *decision_source_revision)),
        )?;
        let (detail, descriptor_class) = self.descriptor(store, alias, object_id, judged.tip)?;
        // Eligibility judges any served object; the owner must also be a decision row at that snapshot, or nothing exists whose retraction could revoke this form.
        let decision = self.decision(store, alias, originating_decision_id, judged.tip)?;
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
        Ok(CanonicalJudgement {
            judged,
            detail,
            sensitivity: descriptor_class.restrictive(decision.sensitivity),
        })
    }

    /// The Kernel's egress verdict folds every live reference to the digest for this destination; default-Sensitive policy means unproven evidence never reaches a remote model.
    fn egress_allowed(
        &self,
        store: &KernelStore,
        alias: Option<&Alias>,
        handle: &ArtifactHandle,
    ) -> Result<(), Refusal> {
        // A digest with no live reference is a denial, not an error; the errors left are a malformed digest and a store that could not answer.
        let facts = store
            .artifact_egress_facts(handle, self.binding.destination)
            .map_err(|error| match error.kind() {
                ArtifactErrorKind::InvalidInput => refuse(alias, RefusalCode::ExpectationChanged),
                _ => refuse(alias, RefusalCode::Store),
            })?;
        if facts.eligibility != ArtifactEligibility::Allowed {
            return Err(refuse(alias, RefusalCode::PolicyBlocked));
        }
        Ok(())
    }

    /// Checks the destination verdict on the expected digest, grows the execution hold over the artifact, and returns the held facts, which must carry that digest. The verdict comes first so a policy-blocked artifact is never pinned or charged against the hold's backing; an artifact already extended in this run skips the writer transaction and is still validated against the live hold. A hold capacity refusal truncates the evidence set whichever caller asked, so it marks the ledger partial here, before any read.
    pub(crate) fn hold_evidence(
        &mut self,
        store: &KernelStore,
        alias: Option<&Alias>,
        evidence_id: &str,
        artifact_digest: &str,
        now_ms: i64,
    ) -> Result<HeldEvidence, Refusal> {
        let result = self.hold_evidence_inner(store, alias, evidence_id, artifact_digest, now_ms);
        if let Err(refusal) = &result
            && refusal.code.truncates_evidence()
        {
            self.ledger.record_partial_disclosure();
        }
        result
    }

    fn hold_evidence_inner(
        &mut self,
        store: &KernelStore,
        alias: Option<&Alias>,
        evidence_id: &str,
        artifact_digest: &str,
        now_ms: i64,
    ) -> Result<HeldEvidence, Refusal> {
        self.egress_allowed(
            store,
            alias,
            &ArtifactHandle {
                digest: artifact_digest.to_string(),
                evidence_id: evidence_id.to_string(),
            },
        )?;
        if !self.extended.contains(evidence_id) {
            store
                .extend_execution_hold(
                    &self.binding.hold_id,
                    &self.binding.hold,
                    std::slice::from_ref(&evidence_id.to_string()),
                )
                .map_err(|error| refuse(alias, hold_refusal(error)))?;
            self.extended.insert(evidence_id.to_string());
        }
        let mut held = store
            .validate_held_evidence(
                &self.binding.hold_id,
                CuratorHoldKind::Execution,
                &self.binding.hold,
                std::slice::from_ref(&evidence_id.to_string()),
                now_ms,
            )
            .map_err(|error| refuse(alias, hold_refusal(error)))?;
        let held = held
            .pop()
            .ok_or_else(|| refuse(alias, RefusalCode::HoldInvalid))?;
        // The verdict above was on the expected digest; the evidence must actually carry it.
        if held.artifact_digest != artifact_digest {
            return Err(refuse(alias, RefusalCode::ExpectationChanged));
        }
        Ok(held)
    }

    /// Staged subjects are not artifacts; the Kernel's class rule for the destination applies to their stored class.
    fn gate_destination(&self, alias: &Alias, sensitivity: Sensitivity) -> Result<(), Refusal> {
        if sensitivity
            .denies_destination(self.binding.destination)
            .is_some()
        {
            return Err(refuse(Some(alias), RefusalCode::PolicyBlocked));
        }
        Ok(())
    }

    /// Charges rendered bytes before any load.
    fn charge(&mut self, alias: &Alias, bytes: usize) -> Result<(), Refusal> {
        self.accounting
            .charge_render(Some(alias), u64::try_from(bytes).unwrap_or(u64::MAX))
    }

    /// Charges the range, loads the artifact once, re-reads the execution hold and the held facts (refreshing `held`) on the bytes that were just loaded, re-reads the egress verdict on every render, checks the whole buffer on first load, and copies the requested range out of the retained buffer. The whole-buffer check means a range split can never hide a marker or secret; a buffer that failed it stays refused without another charge or scan. Returns whether this call loaded the artifact, so the caller can re-judge object standing over the same window.
    fn load_range(
        &mut self,
        store: &KernelStore,
        alias: &Alias,
        held: &mut HeldEvidence,
        range: Option<Range<u64>>,
        now_ms: i64,
    ) -> Result<(Vec<u8>, bool, Range<u64>), Refusal> {
        let range = range.unwrap_or(0..held.byte_length);
        if range.end > held.byte_length {
            return Err(refuse(Some(alias), RefusalCode::InvalidRange));
        }
        if self.refused_artifacts.contains(&held.artifact_digest) {
            return Err(refuse(Some(alias), RefusalCode::RenderCheck));
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
        if loaded {
            #[cfg(feature = "test-support")]
            if let Some(hook) = self.after_load_for_test.as_mut() {
                hook(store);
            }
            // The verdicts before the hold grew and the disk read are separate snapshots. The hold and evidence facts are re-read first: a run whose cutoff passed during the read does not disclose what it loaded, a capture whose acquisition reference lapsed meanwhile is refused, and the class reported with the bytes is the one they carry now. The egress verdict is re-read last, on every render, so a classification tightened at any point before it is the one that decides.
            let now_ms = now_ms.max(crate::now_ms());
            *held = store
                .validate_held_evidence(
                    &self.binding.hold_id,
                    CuratorHoldKind::Execution,
                    &self.binding.hold,
                    std::slice::from_ref(&held.evidence_id),
                    now_ms,
                )
                .map_err(|error| refuse(Some(alias), hold_refusal(error)))?
                .pop()
                .ok_or_else(|| refuse(Some(alias), RefusalCode::HoldInvalid))?;
            // Only a Curator capture's `retain_until` is an acquisition deadline; on other classes it is a GC retention floor the hold overrides.
            if held.retention_class == CURATOR_CAPTURE_RETENTION_CLASS
                && held.retain_until.is_some_and(|until| until <= now_ms)
            {
                return Err(refuse(Some(alias), RefusalCode::ExpectationChanged));
            }
        }
        // Every render, cached or not, ends on the egress verdict: the check before the hold grew and this one are separate snapshots, and a classification tightened between them decides here.
        self.egress_allowed(
            store,
            Some(alias),
            &ArtifactHandle {
                digest: held.artifact_digest.clone(),
                evidence_id: held.evidence_id.clone(),
            },
        )?;
        if loaded || !self.checked_artifacts.contains(&held.artifact_digest) {
            let whole = self
                .buffers
                .slice(&held.artifact_digest, 0..held.byte_length)
                .map_err(|_| refuse(Some(alias), RefusalCode::ExpectationChanged))?;
            if let Err(refusal) = check_whole_artifact(whole, Some(alias)) {
                self.refused_artifacts.insert(held.artifact_digest.clone());
                return Err(refusal);
            }
            self.checked_artifacts.insert(held.artifact_digest.clone());
        }
        self.buffers
            .slice(&held.artifact_digest, range.clone())
            .map(|bytes| (bytes.to_vec(), loaded, range))
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
                &self.project,
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

    /// The descriptor observation of `object_id` at `tip` and the row's own class; a missing row, another observation kind, an undecodable detail, a stored identity that does not re-encode to itself, or a detail citing evidence other than the row's refuses.
    fn descriptor(
        &self,
        store: &KernelStore,
        alias: Option<&Alias>,
        object_id: &str,
        tip: i64,
    ) -> Result<(SourceDescriptorDetail, Sensitivity), Refusal> {
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
            // The span and tuple are trusted only after the Kernel's own consistency test, and the detail must cite the evidence the row cites; anything else is corruption, not a descriptor.
            .filter(|detail| {
                detail.is_consistent_with(object_id)
                    && row.evidence_id.as_deref() == Some(detail.evidence_id.as_str())
            })
            .map(|detail| (detail, row.sensitivity))
            .ok_or_else(|| refuse(alias, RefusalCode::ExpectationChanged))
    }

    /// The live decision row whose object is `object_id` at `tip`; a served object that is not a decision refuses.
    fn decision(
        &self,
        store: &KernelStore,
        alias: Option<&Alias>,
        object_id: &str,
        tip: i64,
    ) -> Result<DecisionRow, Refusal> {
        let mut rows = store
            .decisions_for_objects_as_of(std::slice::from_ref(&object_id.to_string()), tip)
            .map_err(|_| refuse(alias, RefusalCode::Store))?;
        rows.retain(|row| row.object_id == object_id);
        match (rows.pop(), rows.is_empty()) {
            (Some(row), true) => Ok(row),
            _ => Err(refuse(alias, RefusalCode::ExpectationChanged)),
        }
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

pub(crate) fn hold_refusal(error: CuratorHoldError) -> RefusalCode {
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

/// A span descriptor names one selection of its backing artifact, and only those bytes carry its provenance: no range means the span, and a range outside it is refused rather than translated.
fn span_range(
    alias: &Alias,
    span: Option<(u64, u64)>,
    range: Option<Range<u64>>,
) -> Result<Option<Range<u64>>, Refusal> {
    match (span, range) {
        (None, range) => Ok(range),
        (Some((start, end)), None) => Ok(Some(start..end)),
        (Some((start, end)), Some(range)) if range.start >= start && range.end <= end => {
            Ok(Some(range))
        }
        (Some(_), Some(_)) => Err(refuse(Some(alias), RefusalCode::InvalidRange)),
    }
}

/// The classes whose descriptors are forms of one decision, and so carry an originating decision.
const fn decision_derived(class: OccurrenceClass) -> bool {
    matches!(
        class,
        OccurrenceClass::CanonicalClaims | OccurrenceClass::PromotedMemory
    )
}

/// Two excerpts or revisions of one native source are one origin: the key is the class and identity fields, without revision, representation, or span. Identity values are control-character free, so the unit separator cannot occur inside one.
fn native_origin_key(class: OccurrenceClass, identity: &[(String, String)]) -> String {
    let fields = identity
        .iter()
        .map(|(field, value)| format!("{field}={value}"))
        .collect::<Vec<_>>()
        .join("\u{1f}");
    format!("native:{}:{fields}", class.code())
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
