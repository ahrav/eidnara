//! Verifier input classification: which live decisions the verifier can check, and the [`Claim`]
//! each one makes.
//!
//! [`classify`] reads one decision's registry row, decision row, and latest own admission, its
//! anchor, its row-level and event-level citations, and the `repository_documents` descriptor
//! citing each cited evidence row, every read at one commit sequence under one evaluation budget,
//! and then confirms the store is still the target's incarnation. The result is a function of the
//! Kernel snapshot at that sequence and of the bound roots the caller passes: the classifier reads
//! no worktree, clock, or network, and opens no repository. It carries evidence ids and never
//! evidence bytes, since document evidence is `Sensitive`.
//!
//! The first read decides membership: an object that is absent, not a decision, not live, or whose
//! latest own admission is not `Active` is [`Class::NotInput`], and a remaining decision the
//! verifier authored is [`Unverifiable::SelfSource`], both before its anchor or citations are read.
//! Every other decision that classifies is either [`Class::Verifiable`] or exactly one
//! [`Unverifiable`] reason, checked in this order: the anchor (`no_anchor`, `anchor_not_git`), then the citations
//! (`no_citation`, then per cited evidence row, the row-level `evidence_id` first and then at most
//! [`MAX_EVENT_CITATIONS`] event-level citations in the linked-evidence order: `evidence_retired`,
//! `descriptor_retired`, `digest_missing`, or no document descriptor). The first cited row that
//! resolves to a live document descriptor with a digest and a `relative_path` inside the document
//! path rule makes the decision verifiable. When no cited row does, a cited row whose descriptor
//! has a digest and a path outside the rule makes the classification
//! [`ClassifyError::InvalidDocumentPath`], ahead of every reason; otherwise the first cited row's
//! reason other than "no document descriptor" names it, and `citation_not_document` names a
//! decision whose every inspected row has no document descriptor. The reasons describe the
//! inspected citations only: event-level citations past the cap, or older than the newest
//! [`kernel::LINKED_EVENT_WINDOW`] × ([`MAX_EVENT_CITATIONS`] + 1) decision events the
//! linked-evidence read walks, are not inspected, so a decision whose only document citation lies
//! past them classifies `no_citation` or `citation_not_document`. A Kernel refusal or
//! `InvalidDocumentPath` returns an error and no class, and counts under no reason.

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use kernel::applicability::EvalBudget;
use kernel::source_identity::OccurrenceClass;
use kernel::{
    AdmissionFacts, AnchorCondition, CitingDescriptor, ClaimDecisionFacts, ClaimFactsError,
    CommitReadTarget, Disposition, EvidenceLiveness, EvidenceLivenessError, GitCondition,
    KernelError, KernelStore, LinkedEvidence, LiveDescriptor, ObjectRow, Sensitivity, SourceClass,
    TaintClass,
};

use super::{Claim, Reason};

/// The `source_kind` of every decision the verifier writes.
pub const VERIFIER_SOURCE_KIND: &str = "verifier";

/// The event-level citations one classification reads, newest first and ties in evidence id
/// order, besides the decision row's own `evidence_id`, which is always read first. Citations
/// past the cap are dropped before any is resolved, and the classification records no sign of
/// the drop.
pub const MAX_EVENT_CITATIONS: NonZeroUsize = NonZeroUsize::new(32).unwrap();

/// A bound project root and the repository identity its documents are published under.
///
/// A document descriptor's `relative_path` is relative to the root of the project that captured
/// it, and the claim carries that path unchanged, so verifying the claim against `root` names the
/// captured document when the capturing project's root is its repository's top level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundRoot<'a> {
    pub root: &'a Path,
    pub repository_id: &'a str,
}

/// One decision's classification at `target`: every read ran at `target.through_commit`, and the
/// store was `target.incarnation` from the first read through the last.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classification {
    pub target: CommitReadTarget,
    pub class: Class,
}

/// Why [`classify`] returned no classification for a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ClassifyError {
    /// A Kernel read refused, or the store left `target`'s incarnation during the reads.
    #[error(transparent)]
    Facts(#[from] ClaimFactsError),
    /// A cited live document descriptor with a valid SHA-256 digest has a stored `relative_path`
    /// outside the document path rule [`Claim::new`] enforces, and no other cited row resolves to
    /// a verifiable document. A descriptor without a valid digest yields `digest_missing` before
    /// its path is checked.
    /// The refusal concerns that decision's descriptors alone.
    #[error("the cited document descriptor's path is outside the document path rule")]
    InvalidDocumentPath,
}

impl DecisionFacts {
    fn new(object: ObjectRow, decision: ClaimDecisionFacts, admission: AdmissionFacts) -> Self {
        Self {
            object_id: object.object_id,
            decision_id: decision.decision_id,
            source_kind: object.source_kind,
            source_id: object.source_id,
            source_revision: object.source_revision,
            created_commit_seq: object.created_commit_seq,
            sensitivity: object.sensitivity,
            anchor_id: decision.anchor_id,
            evidence_id: decision.evidence_id,
            disposition: admission.disposition,
            source_class: admission.source_class,
            taint_class: admission.taint_class,
        }
    }
}

impl From<KernelError> for ClassifyError {
    fn from(error: KernelError) -> Self {
        Self::Facts(error.into())
    }
}

/// Where a decision stands with respect to the verifier's input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Class {
    /// A live, `Active` decision the verifier can check, with its claim.
    Verifiable(Box<VerifierInput>),
    /// A live, `Active` decision outside the verifier's input, for exactly one reason.
    Unverifiable(Unverifiable),
    /// The object is absent, not a decision, not live, or its latest own admission is missing or
    /// has a disposition other than `Active`; it counts under no reason.
    NotInput,
}

/// Why a live, `Active` decision is outside the verifier's input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unverifiable {
    /// The decision names no anchor, or no anchor of its id is live at the sequence.
    NoAnchor,
    /// The live anchor's condition is other than git `reachable_from`: `Exact`, git
    /// `ReachableBetween`, or a non-git kind.
    AnchorNotGit,
    /// The decision row has no `evidence_id` and the inspected events cite no evidence.
    NoCitation,
    /// No inspected citation, the row-level one and at most [`MAX_EVENT_CITATIONS`] event-level
    /// ones, has a document descriptor.
    CitationNotDocument,
    /// The first cited row with a reason names evidence invalidated at the sequence.
    EvidenceRetired,
    /// Descriptors cited the evidence, and the cited revision is no longer live at the sequence:
    /// retired or superseded by a newer revision of its lineage.
    DescriptorRetired,
    /// The first cited row with a reason has a live document descriptor whose `artifact_digest`
    /// is not a lowercase hex SHA-256 digest.
    DigestMissing,
    /// The verifier authored the decision: its `source_kind` is [`VERIFIER_SOURCE_KIND`].
    SelfSource,
}

impl Unverifiable {
    /// The stable snake_case name, the suffix of the reason's counter.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoAnchor => "no_anchor",
            Self::AnchorNotGit => "anchor_not_git",
            Self::NoCitation => "no_citation",
            Self::CitationNotDocument => "citation_not_document",
            Self::EvidenceRetired => "evidence_retired",
            Self::DescriptorRetired => "descriptor_retired",
            Self::DigestMissing => "digest_missing",
            Self::SelfSource => "self_source",
        }
    }
}

/// The model decision's registered facts and its latest own admission at the classification's
/// sequence, which a write set rechecks before it acts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionFacts {
    pub object_id: String,
    pub decision_id: String,
    pub source_kind: String,
    pub source_id: String,
    pub source_revision: i64,
    pub created_commit_seq: i64,
    pub sensitivity: Sensitivity,
    pub anchor_id: Option<String>,
    /// The decision row's own `evidence_id`; event-level citations are separate.
    pub evidence_id: Option<String>,
    pub disposition: Disposition,
    pub source_class: SourceClass,
    pub taint_class: TaintClass,
}

/// A verifiable decision: its claim and the rows the claim came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifierInput {
    pub decision: DecisionFacts,
    /// The descriptor's `relative_path` and `artifact_digest`, the evidence row's `byte_length`,
    /// and the `reachable_from` anchor's oid.
    pub claim: Claim,
    /// The cited evidence row the claim resolved through.
    pub evidence_id: String,
    pub descriptor_object_id: String,
    pub lineage_id: String,
    pub descriptor_revision: String,
    pub repository_id: String,
    /// The first bound root whose repository identity is the descriptor's, or
    /// `Err(Reason::RepositoryUnbound)` when no bound root has it.
    pub root: Result<PathBuf, Reason>,
}

impl VerifierInput {
    /// The kind of the claimed document, from its path.
    pub fn document_kind(&self) -> DocumentKind {
        DocumentKind::of(self.claim.relative_path())
    }
}

/// What a document is evidence of, from its path alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    Code,
    Config,
}

/// File name extensions of configuration documents. A name with no other dot, such as `.env` or
/// `.editorconfig`, takes its whole name after the dot as its extension.
const CONFIG_EXTENSIONS: [&str; 9] = [
    "toml",
    "yaml",
    "yml",
    "json",
    "ini",
    "cfg",
    "conf",
    "env",
    "editorconfig",
];

impl DocumentKind {
    /// `Config` for a path whose file name ends in a configuration extension, compared exactly;
    /// `Code` for every other path, prose documents such as `ARCHITECTURE.md` included.
    pub fn of(relative_path: &str) -> Self {
        let name = relative_path
            .rsplit_once('/')
            .map_or(relative_path, |(_, name)| name);
        match name.rsplit_once('.') {
            Some((_, extension)) if CONFIG_EXTENSIONS.contains(&extension) => Self::Config,
            _ => Self::Code,
        }
    }

    pub fn taint_class(self) -> TaintClass {
        match self {
            Self::Code => TaintClass::CurrentCode,
            Self::Config => TaintClass::CurrentConfig,
        }
    }

    /// The observation kind of the witness that records the document present.
    pub fn witness_kind(self) -> &'static str {
        match self {
            Self::Code => "code_present",
            Self::Config => "config_present",
        }
    }
}

/// Classifies the decision `object_id` at `target.through_commit` under `budget`, mapping its
/// document's repository to the first of `roots` with that repository identity. It reads the
/// decision row's own `evidence_id` and at most [`MAX_EVENT_CITATIONS`] event-level citations.
///
/// The standing read and the row-level evidence liveness read compare the store's incarnation
/// with `target.incarnation` under their reader guards; the anchor, event-level citation, and
/// descriptor reads take the sequence alone, and after the last read, whether it succeeded or
/// refused, the incarnation is compared again. A
/// restore only advances the store's restore generation, so an equal incarnation after the last
/// read means every read saw the history `target` was captured from.
///
/// # Errors
///
/// Returns the Kernel reads' refusals, including: `Facts(IncarnationMismatch)` when the store is not
/// `target`'s incarnation at the first read or after the last, `Facts(Kernel(Deadline))` when the
/// budget runs out, `Facts(Kernel(FutureSnapshot))` past the tip, and
/// `Facts(Kernel(CorruptCanonicalRow))` for a stored decision, anchor, or descriptor that fails
/// its row checks, `Facts(Kernel(InvalidInput))` for a negative sequence or an `object_id` the
/// standing read refuses, `Facts(MalformedRequiredField)` for a decision or admission field this
/// build cannot interpret, and the store's other read errors (`Busy`, `Io`, `Conflict`). Returns [`ClassifyError::InvalidDocumentPath`] when a cited live document
/// descriptor with a valid digest has a `relative_path` that passes the descriptor row checks and
/// is outside the document path rule, and no cited row resolves to a verifiable document. `IncarnationMismatch` takes
/// precedence over every other refusal, since a read of another incarnation decides nothing.
pub fn classify(
    kernel: &KernelStore,
    object_id: &str,
    target: CommitReadTarget,
    roots: &[BoundRoot<'_>],
    budget: &EvalBudget,
) -> Result<Classification, ClassifyError> {
    let class = class_of(kernel, object_id, target, roots, budget);
    // A restore advances the generation and never returns it, so this comparison detects a restore
    // between any two reads above without holding a reader guard.
    kernel
        .require_incarnation(target.incarnation)
        .map_err(|_| ClaimFactsError::IncarnationMismatch)?;
    Ok(Classification {
        target,
        class: class?,
    })
}

fn class_of(
    kernel: &KernelStore,
    object_id: &str,
    target: CommitReadTarget,
    roots: &[BoundRoot<'_>],
    budget: &EvalBudget,
) -> Result<Class, ClassifyError> {
    let requested = target.through_commit;
    let ids = [object_id.to_owned()];
    let standing =
        match kernel.claim_standing_at_within_budget(&ids, target, NonZeroUsize::MIN, budget) {
            Ok(mut standings) => standings.pop().flatten(),
            Err(ClaimFactsError::NotADecision) => None,
            Err(error) => return Err(error.into()),
        };
    let Some(standing) =
        standing.filter(|standing| standing.object.invalidated_commit_seq.is_none())
    else {
        return Ok(Class::NotInput);
    };
    let Some(admission) = standing
        .own_admission
        .filter(|admission| admission.disposition == Disposition::Active)
    else {
        return Ok(Class::NotInput);
    };
    if standing.object.source_kind == VERIFIER_SOURCE_KIND {
        return Ok(Class::Unverifiable(Unverifiable::SelfSource));
    }
    let facts = DecisionFacts::new(standing.object, standing.decision, admission);
    let unverifiable = |reason| Ok(Class::Unverifiable(reason));
    // An anchor retired by the sequence leaves the decision with no live anchor.
    let condition = match &facts.anchor_id {
        Some(anchor_id) => kernel.anchor_condition_as_of(anchor_id, requested, budget)?,
        None => None,
    };
    let oid = match condition {
        Some(AnchorCondition::Git(GitCondition::ReachableFrom { oid, .. })) => oid,
        Some(_) => return unverifiable(Unverifiable::AnchorNotGit),
        None => return unverifiable(Unverifiable::NoAnchor),
    };
    let cited = citations(
        kernel,
        object_id,
        facts.evidence_id.as_deref(),
        target,
        budget,
    )?;
    if cited.is_empty() {
        return unverifiable(Unverifiable::NoCitation);
    }
    let mut first_reason = None;
    let mut invalid_path = false;
    for evidence in cited {
        let (descriptor, digest) = match resolve_citation(kernel, &evidence, requested, budget)? {
            Resolution::Document(descriptor, digest) => (descriptor, digest),
            Resolution::Reason(reason) => {
                first_reason.get_or_insert(reason);
                continue;
            }
            Resolution::NotDocument => continue,
        };
        match VerifierInput::new(
            &facts,
            evidence.evidence_id,
            *descriptor,
            digest,
            &oid,
            roots,
        ) {
            Ok(input) => return Ok(Class::Verifiable(Box::new(input))),
            Err(ClassifyError::InvalidDocumentPath) => invalid_path = true,
            Err(error) => return Err(error),
        }
    }
    if invalid_path {
        return Err(ClassifyError::InvalidDocumentPath);
    }
    unverifiable(first_reason.unwrap_or(Unverifiable::CitationNotDocument))
}

/// The decision row's own evidence first, then its newest event-level citations of other
/// evidence, at most [`MAX_EVENT_CITATIONS`] of them, so no event citation displaces the row's.
fn citations(
    kernel: &KernelStore,
    object_id: &str,
    own: Option<&str>,
    target: CommitReadTarget,
    budget: &EvalBudget,
) -> Result<Vec<LinkedEvidence>, ClassifyError> {
    let mut cited = Vec::new();
    if let Some(evidence_id) = own {
        let liveness = kernel
            .evidence_liveness_at_within_budget(&[evidence_id], target, budget)
            .map_err(|error| match error {
                EvidenceLivenessError::IncarnationMismatch => ClaimFactsError::IncarnationMismatch,
                EvidenceLivenessError::Kernel(error) => error.into(),
                // The read passes one id, under the per-read id limit.
                EvidenceLivenessError::TooManyIds => KernelError::InvalidInput.into(),
            })?;
        // The decision row's foreign key keeps its evidence row, created no later than the
        // decision, so a live decision's evidence is live or retired; any other answer is a
        // corrupt store.
        let live = match liveness.first() {
            Some(EvidenceLiveness::Live) => true,
            Some(EvidenceLiveness::Retired) => false,
            Some(EvidenceLiveness::Unknown) | None => {
                return Err(KernelError::CorruptCanonicalRow.into());
            }
        };
        cited.push(LinkedEvidence {
            evidence_id: evidence_id.to_owned(),
            live,
        });
    }
    // One row more than the cap leaves the cap after the row's own evidence is dropped.
    let max = MAX_EVENT_CITATIONS.saturating_add(1);
    let events = kernel.linked_decision_evidence_within_budget(
        object_id,
        target.through_commit,
        max,
        budget,
    )?;
    // The linked-evidence read bounds only its reader wait by the budget, so a budget that ran
    // out during its query refuses here instead of yielding a reason from a late read.
    if budget.is_exhausted() {
        return Err(KernelError::Deadline.into());
    }
    cited.extend(
        events
            .into_iter()
            .filter(|evidence| Some(evidence.evidence_id.as_str()) != own)
            .take(MAX_EVENT_CITATIONS.get()),
    );
    Ok(cited)
}

/// What one cited evidence row contributes to the classification.
enum Resolution {
    /// A live document descriptor with a digest cites it.
    Document(Box<LiveDescriptor>, [u8; 32]),
    Reason(Unverifiable),
    /// No document descriptor cites it.
    NotDocument,
}

fn resolve_citation(
    kernel: &KernelStore,
    evidence: &LinkedEvidence,
    requested: i64,
    budget: &EvalBudget,
) -> Result<Resolution, KernelError> {
    if !evidence.live {
        return Ok(Resolution::Reason(Unverifiable::EvidenceRetired));
    }
    let found = kernel.source_descriptor_citing_evidence(
        OccurrenceClass::RepositoryDocuments,
        &evidence.evidence_id,
        requested,
        budget,
    )?;
    Ok(match found {
        CitingDescriptor::Live(descriptor) => {
            match crate::memory_capture::cite::digest_bytes(&descriptor.detail.artifact_digest) {
                Some(digest) => Resolution::Document(descriptor, digest),
                None => Resolution::Reason(Unverifiable::DigestMissing),
            }
        }
        CitingDescriptor::Retired => Resolution::Reason(Unverifiable::DescriptorRetired),
        CitingDescriptor::Uncited => Resolution::NotDocument,
    })
}

impl VerifierInput {
    fn new(
        decision: &DecisionFacts,
        evidence_id: String,
        descriptor: LiveDescriptor,
        digest: [u8; 32],
        oid: &str,
        roots: &[BoundRoot<'_>],
    ) -> Result<Self, ClassifyError> {
        let field = |name: &str| {
            crate::git_reconcile::identity_field(&descriptor, name)
                .map(str::to_owned)
                .ok_or(KernelError::CorruptCanonicalRow)
        };
        let relative_path = field("relative_path")?;
        let repository_id = field("repository_id")?;
        let claim = Claim::new(&relative_path, digest, descriptor.byte_length, oid)
            .map_err(|_| ClassifyError::InvalidDocumentPath)?;
        let root = roots
            .iter()
            .find(|bound| bound.repository_id == repository_id)
            .map(|bound| bound.root.to_path_buf())
            .ok_or(Reason::RepositoryUnbound);
        Ok(Self {
            decision: decision.clone(),
            claim,
            evidence_id,
            descriptor_object_id: descriptor.object_id,
            lineage_id: descriptor.detail.lineage_id,
            descriptor_revision: descriptor.detail.revision,
            repository_id,
            root,
        })
    }
}
