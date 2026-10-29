//! Verifier input classification: which live decisions the verifier can check, and the [`Claim`]
//! each one makes.
//!
//! [`classify`] reads one decision's registry row, decision row, and latest own admission, its
//! anchor, its row-level and event-level citations, and the `repository_documents` descriptor
//! citing each cited evidence row, every read at one commit sequence under one evaluation budget.
//! The result is a function of the Kernel snapshot at that sequence and of the bound roots the
//! caller passes: the classifier reads no worktree, clock, or network, and opens no repository. It
//! carries evidence ids and never evidence bytes, since document evidence is `Sensitive`.
//!
//! A decision the verifier authored is [`Unverifiable::SelfSource`] before its anchor or citations
//! are read. A live, `Active` decision is then either [`Class::Verifiable`] or exactly one
//! [`Unverifiable`] reason, checked in this order: the anchor (`no_anchor`, `anchor_not_git`),
//! then the citations (`no_citation`, then per cited evidence row in the linked-evidence order:
//! `evidence_retired`, `descriptor_retired`, `digest_missing`, or no document descriptor). The
//! first cited row that resolves to a live document descriptor with a digest makes the decision
//! verifiable; otherwise the first cited row's reason other than "no document descriptor" names
//! it, and `citation_not_document` names a decision whose every cited row has no document
//! descriptor.

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use kernel::applicability::EvalBudget;
use kernel::source_identity::OccurrenceClass;
use kernel::{
    AnchorCondition, CitingDescriptor, ClaimFactsError, CommitReadTarget, Disposition,
    GitCondition, KernelError, KernelStore, Sensitivity, SourceClass, TaintClass,
};

use super::{Claim, Reason};

/// The `source_kind` of every decision the verifier writes.
pub const VERIFIER_SOURCE_KIND: &str = "verifier";

/// The cited evidence rows one classification reads: the decision's own and its newest events'.
pub const MAX_CITATIONS: NonZeroUsize = NonZeroUsize::new(32).unwrap();

/// A bound project root and the repository identity its documents are published under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundRoot<'a> {
    pub root: &'a Path,
    pub repository_id: &'a str,
}

/// One decision's classification at `commit_seq`, the sequence every read ran at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classification {
    pub commit_seq: i64,
    pub class: Class,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Class {
    Verifiable(Box<VerifierInput>),
    Unverifiable(Unverifiable),
    /// The object is absent, not a decision, not live, or its latest own admission is missing or
    /// has a disposition other than `Active`; it counts under no reason.
    NotInput,
}

/// Why a live, `Active` decision is outside the verifier's input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unverifiable {
    NoAnchor,
    AnchorNotGit,
    NoCitation,
    CitationNotDocument,
    EvidenceRetired,
    DescriptorRetired,
    DigestMissing,
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
    pub document_kind: DocumentKind,
    /// The first bound root whose repository identity is the descriptor's, or
    /// `Err(Reason::RepositoryUnbound)` when no bound root has it.
    pub root: Result<PathBuf, Reason>,
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
        let name = relative_path.rsplit('/').next().unwrap_or(relative_path);
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
/// document's repository to the first of `roots` with that repository identity.
///
/// # Errors
///
/// Returns the Kernel reads' refusals: `IncarnationMismatch` when the store is not `target`'s
/// incarnation, `Kernel(Deadline)` when the budget runs out, `Kernel(FutureSnapshot)` past the
/// tip, and `Kernel(CorruptCanonicalRow)` for a stored anchor, descriptor, or document path that
/// fails its row checks.
pub fn classify(
    kernel: &KernelStore,
    object_id: &str,
    target: CommitReadTarget,
    roots: &[BoundRoot<'_>],
    budget: &EvalBudget,
) -> Result<Classification, ClaimFactsError> {
    Ok(Classification {
        commit_seq: target.through_commit,
        class: class_of(kernel, object_id, target, roots, budget)?,
    })
}

fn class_of(
    kernel: &KernelStore,
    object_id: &str,
    target: CommitReadTarget,
    roots: &[BoundRoot<'_>],
    budget: &EvalBudget,
) -> Result<Class, ClaimFactsError> {
    let requested = target.through_commit;
    let ids = [object_id.to_owned()];
    let standing =
        match kernel.claim_standing_at_within_budget(&ids, target, NonZeroUsize::MIN, budget) {
            Ok(mut standings) => standings.pop().flatten(),
            Err(ClaimFactsError::NotADecision) => None,
            Err(error) => return Err(error),
        };
    let Some(standing) =
        standing.filter(|standing| standing.object.invalidated_commit_seq.is_none())
    else {
        return Ok(Class::NotInput);
    };
    if standing.object.source_kind == VERIFIER_SOURCE_KIND {
        return Ok(Class::Unverifiable(Unverifiable::SelfSource));
    }
    let Some(admission) = standing
        .own_admission
        .filter(|admission| admission.disposition == Disposition::Active)
    else {
        return Ok(Class::NotInput);
    };
    let (object, decision) = (standing.object, standing.decision);
    let facts = DecisionFacts {
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
    };
    let unverifiable = |reason| Ok(Class::Unverifiable(reason));
    // An anchor retired by the sequence leaves the decision with no live anchor.
    let condition = match &facts.anchor_id {
        Some(anchor_id) => kernel.anchor_condition_at(anchor_id, requested, budget)?,
        None => None,
    };
    let oid = match condition {
        Some(AnchorCondition::Git(GitCondition::ReachableFrom { oid, .. })) => oid,
        Some(_) => return unverifiable(Unverifiable::AnchorNotGit),
        None => return unverifiable(Unverifiable::NoAnchor),
    };
    let cited = kernel.linked_decision_evidence_within_budget(
        object_id,
        requested,
        MAX_CITATIONS,
        budget,
    )?;
    if cited.is_empty() {
        return unverifiable(Unverifiable::NoCitation);
    }
    let mut first_reason = None;
    for evidence in cited {
        let reason = if evidence.live {
            let found = kernel.source_descriptor_citing_evidence(
                OccurrenceClass::RepositoryDocuments,
                &evidence.evidence_id,
                requested,
                budget,
            )?;
            match found {
                CitingDescriptor::Live(descriptor) => {
                    match crate::memory_capture::cite::digest_bytes(
                        &descriptor.detail.artifact_digest,
                    ) {
                        Some(digest) => {
                            let input = VerifierInput::new(
                                facts,
                                evidence.evidence_id,
                                *descriptor,
                                digest,
                                &oid,
                                roots,
                            )?;
                            return Ok(Class::Verifiable(Box::new(input)));
                        }
                        None => Unverifiable::DigestMissing,
                    }
                }
                CitingDescriptor::Retired => Unverifiable::DescriptorRetired,
                CitingDescriptor::Uncited => continue,
            }
        } else {
            Unverifiable::EvidenceRetired
        };
        first_reason.get_or_insert(reason);
    }
    unverifiable(first_reason.unwrap_or(Unverifiable::CitationNotDocument))
}

impl VerifierInput {
    fn new(
        decision: DecisionFacts,
        evidence_id: String,
        descriptor: kernel::LiveDescriptor,
        digest: [u8; 32],
        oid: &str,
        roots: &[BoundRoot<'_>],
    ) -> Result<Self, KernelError> {
        let field = |name: &str| {
            descriptor
                .detail
                .identity
                .iter()
                .find(|(field, _)| field == name)
                .map(|(_, value)| value.clone())
                .ok_or(KernelError::CorruptCanonicalRow)
        };
        let relative_path = field("relative_path")?;
        let repository_id = field("repository_id")?;
        let claim = Claim::new(&relative_path, digest, descriptor.byte_length, oid)
            .map_err(|_| KernelError::CorruptCanonicalRow)?;
        let root = roots
            .iter()
            .find(|bound| bound.repository_id == repository_id)
            .map(|bound| bound.root.to_path_buf())
            .ok_or(Reason::RepositoryUnbound);
        Ok(Self {
            decision,
            document_kind: DocumentKind::of(&relative_path),
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
