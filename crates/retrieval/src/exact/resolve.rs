use std::collections::BTreeSet;
use std::num::NonZeroUsize;

use kernel::applicability::EvalBudget;

use kernel::source_identity::OccurrenceClass;
use kernel::{
    ArtifactDestination, CommitReadTarget, EgressSnapshot, EligibilityCandidate,
    EligibilityVerdict, KernelError, KernelStore, MAX_ELIGIBILITY_CANDIDATES, ProjectScope,
};
use storage::GuardedConn;

use crate::batch::{ProjectionCheckpoint, read_checkpoint};
use crate::eligibility::{Disposition, OccurrenceCandidate, judge_occurrences};
use crate::exact::association::EXTRACTION_VERSION;
use crate::exact::lookup::{
    AssociationRow, Cursor, ExactQuery, LookupContext, LookupRefusal, page,
};
use crate::{ProjectionError, TombstoneReason, read_identity};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletenessCertificate {
    pub canonical_incarnation_id: String,
    pub inventory_epoch: String,
    pub identity_contract_version: String,
    pub extraction_version: u32,
    pub complete_through_commit_seq: i64,
    pub projection: ProjectionCheckpoint,
}

#[derive(Debug, Clone, Copy)]
pub struct Authority<'a> {
    pub project: &'a ProjectScope,
    pub destination: ArtifactDestination,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolveBounds {
    pub page_rows: NonZeroUsize,
    pub max_rows: NonZeroUsize,
    pub max_pages: NonZeroUsize,
    pub max_retained: NonZeroUsize,
    pub max_retained_bytes: NonZeroUsize,
}

pub struct ResolveRequest<'a> {
    pub query: ExactQuery<'a>,
    pub whole_request: bool,
    pub certificate: &'a CompletenessCertificate,
    pub inventory_epoch: &'a str,
    pub authority: Authority<'a>,
    pub bounds: ResolveBounds,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedOccurrence {
    pub key: Vec<u8>,
    pub target_id: String,
    pub occurrence_id: String,
    pub lineage_id: String,
    pub class: OccurrenceClass,
    pub revision: i64,
    pub representation: String,
    pub span: Option<(u64, u64)>,
    pub payload_id: String,
    pub source_object_id: String,
    pub source_evidence_id: String,
    pub source_artifact_digest: String,
}

impl RetainedOccurrence {
    fn from_row(row: &AssociationRow) -> Self {
        Self {
            key: row.key.clone(),
            target_id: row.target_id.clone(),
            occurrence_id: row.occurrence_id.clone(),
            lineage_id: row.lineage_id.clone(),
            class: row.class,
            revision: row.revision,
            representation: row.representation.clone(),
            span: row.span,
            payload_id: row.payload_id.clone(),
            source_object_id: row.source_object_id.clone(),
            source_evidence_id: row.source_evidence_id.clone(),
            source_artifact_digest: row.source_artifact_digest.clone(),
        }
    }

    fn retained_bytes(&self) -> usize {
        self.key.len()
            + self.target_id.len()
            + self.occurrence_id.len()
            + self.lineage_id.len()
            + self.representation.len()
            + self.payload_id.len()
            + self.source_object_id.len()
            + self.source_evidence_id.len()
            + self.source_artifact_digest.len()
    }

    fn candidate(&self) -> OccurrenceCandidate {
        OccurrenceCandidate {
            occurrence_id: self.occurrence_id.clone(),
            class: self.class,
            candidate: EligibilityCandidate {
                object_id: self.source_object_id.clone(),
                source_revision: self.revision,
                artifact_digest: Some(self.source_artifact_digest.clone()),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncompleteReason {
    RowBound,
    PageBound,
    RetentionExhausted,
    BudgetExhausted,
    KernelIncarnationChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completion {
    Complete,
    NoMatch,
    Incomplete(IncompleteReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disqualification {
    Verdict(EligibilityVerdict),
    Tombstoned(TombstoneReason),
    ProjectionLag { tip: i64, complete_through: i64 },
    ClassificationUnknown,
    SnapshotChanged,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Observations {
    pub rows: usize,
    pub eligible: usize,
    pub tombstoned: usize,
    pub retracted: usize,
    pub superseded: usize,
    pub stale: usize,
    pub wrong_scope: usize,
    pub hidden: usize,
    pub provider_sensitive: usize,
}

impl Observations {
    fn record(&mut self, verdict: EligibilityVerdict) {
        match verdict {
            EligibilityVerdict::Ok => self.eligible += 1,
            EligibilityVerdict::Retracted => self.retracted += 1,
            EligibilityVerdict::Superseded => self.superseded += 1,
            EligibilityVerdict::Stale => self.stale += 1,
            EligibilityVerdict::WrongScope => self.wrong_scope += 1,
            EligibilityVerdict::Hidden => self.hidden += 1,
            EligibilityVerdict::ProviderSensitive => self.provider_sensitive += 1,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Consumed {
    pub pages: usize,
    pub rows: usize,
    pub validated: usize,
    pub retained: usize,
    pub retained_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactProof {
    pub target_id: String,
    pub occurrences: Vec<RetainedOccurrence>,
    pub certificate: CompletenessCertificate,
    pub project: ProjectScope,
    pub destination: ArtifactDestination,
    pub snapshot: EgressSnapshot,
    pub kernel: CommitReadTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub completion: Completion,
    pub disqualified: Option<Disqualification>,
    pub targets: BTreeSet<String>,
    pub retained: Vec<RetainedOccurrence>,
    pub observations: Observations,
    pub consumed: Consumed,
    pub distinct_keys: usize,
    pub snapshot: Option<EgressSnapshot>,
    pub proof: Option<ExactProof>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CertificateRefusal {
    #[error("the certificate names another canonical incarnation")]
    IncarnationMismatch,
    #[error("the certificate names inventory epoch {certified:?} but {current:?} is current")]
    InventoryEpochMismatch { certified: String, current: String },
    #[error("the certificate was issued under extraction version {certified}, not {expected}")]
    ExtractionVersion { certified: u32, expected: u32 },
    #[error("the certificate names another identity contract version")]
    IdentityContract,
    #[error("the certificate does not describe the projection's checkpoint")]
    ProjectionMismatch,
    #[error("the certificate's complete-through sequence is not the projection checkpoint")]
    CompleteThrough,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResolveRefusal {
    #[error("the request budget is exhausted")]
    BudgetExhausted,
    #[error("page rows exceed the kernel's {MAX_ELIGIBILITY_CANDIDATES} candidate bound")]
    PageOverBound,
    #[error(transparent)]
    Certificate(#[from] CertificateRefusal),
    #[error(transparent)]
    Lookup(#[from] LookupRefusal),
    #[error("kernel: {0}")]
    Kernel(KernelError),
}

impl From<KernelError> for ResolveRefusal {
    fn from(error: KernelError) -> Self {
        Self::Kernel(error)
    }
}

impl From<ProjectionError> for ResolveRefusal {
    fn from(error: ProjectionError) -> Self {
        Self::Lookup(error.into())
    }
}

struct Attempt<'a> {
    request: &'a ResolveRequest<'a>,
    kernel: &'a KernelStore,
    budget: &'a EvalBudget,
    initial: CommitReadTarget,
    resolution: Resolution,
}

pub fn resolve(
    conn: &GuardedConn<'_>,
    kernel: &KernelStore,
    request: &ResolveRequest<'_>,
    budget: &EvalBudget,
) -> Result<Resolution, ResolveRefusal> {
    budget
        .check()
        .map_err(|_| ResolveRefusal::BudgetExhausted)?;
    if request.bounds.page_rows.get() > MAX_ELIGIBILITY_CANDIDATES {
        return Err(ResolveRefusal::PageOverBound);
    }
    let certificate = request.certificate;
    admit_certificate(conn, request)?;
    if kernel.database_incarnation_id_within_budget(budget)? != certificate.canonical_incarnation_id
    {
        return Err(CertificateRefusal::IncarnationMismatch.into());
    }
    let initial = kernel.capture_commit_read_target_within_budget(budget)?;
    let mut attempt = Attempt {
        request,
        kernel,
        budget,
        initial,
        resolution: Resolution {
            completion: Completion::Complete,
            disqualified: None,
            targets: BTreeSet::new(),
            retained: Vec::new(),
            observations: Observations::default(),
            consumed: Consumed::default(),
            distinct_keys: 0,
            snapshot: None,
            proof: None,
        },
    };
    if initial.through_commit > certificate.complete_through_commit_seq {
        attempt.disqualify(Disqualification::ProjectionLag {
            tip: initial.through_commit,
            complete_through: certificate.complete_through_commit_seq,
        });
    }
    attempt.enumerate(conn)?;
    Ok(attempt.finish())
}

fn admit_certificate(
    conn: &GuardedConn<'_>,
    request: &ResolveRequest<'_>,
) -> Result<(), ResolveRefusal> {
    let certificate = request.certificate;
    if certificate.extraction_version != EXTRACTION_VERSION {
        return Err(CertificateRefusal::ExtractionVersion {
            certified: certificate.extraction_version,
            expected: EXTRACTION_VERSION,
        }
        .into());
    }
    if request.inventory_epoch != certificate.inventory_epoch {
        return Err(CertificateRefusal::InventoryEpochMismatch {
            certified: certificate.inventory_epoch.clone(),
            current: request.inventory_epoch.to_string(),
        }
        .into());
    }
    let identity = read_identity(conn)?.ok_or(ProjectionError::IdentityMismatch)?;
    if identity.kernel_incarnation_id != certificate.canonical_incarnation_id {
        return Err(CertificateRefusal::IncarnationMismatch.into());
    }
    if identity.identity_contract_version != certificate.identity_contract_version {
        return Err(CertificateRefusal::IdentityContract.into());
    }
    let checkpoint = read_checkpoint(conn, &certificate.canonical_incarnation_id)?
        .ok_or(LookupRefusal::NoCheckpoint)?;
    if checkpoint != certificate.projection {
        return Err(CertificateRefusal::ProjectionMismatch.into());
    }
    if certificate.complete_through_commit_seq != checkpoint.checkpoint_commit_seq {
        return Err(CertificateRefusal::CompleteThrough.into());
    }
    Ok(())
}

impl Attempt<'_> {
    fn disqualify(&mut self, reason: Disqualification) {
        self.resolution.disqualified.get_or_insert(reason);
    }

    fn stop(&mut self, reason: IncompleteReason) {
        self.resolution.completion = Completion::Incomplete(reason);
    }

    fn enumerate(&mut self, conn: &GuardedConn<'_>) -> Result<(), ResolveRefusal> {
        let bounds = self.request.bounds;
        let mut cursor: Option<Cursor> = None;
        loop {
            if self.budget.is_exhausted() {
                self.stop(IncompleteReason::BudgetExhausted);
                return Ok(());
            }
            if self.resolution.consumed.pages == bounds.max_pages.get() {
                self.stop(IncompleteReason::PageBound);
                return Ok(());
            }
            let remaining_rows = bounds.max_rows.get() - self.resolution.consumed.rows;
            let Some(page_rows) = NonZeroUsize::new(remaining_rows.min(bounds.page_rows.get()))
            else {
                self.stop(IncompleteReason::RowBound);
                return Ok(());
            };
            let context = LookupContext {
                kernel_incarnation_id: &self.request.certificate.canonical_incarnation_id,
                page_rows,
                budget: self.budget,
            };
            let page = page(conn, &context, &self.request.query, cursor.as_ref())?;
            if page.checkpoint != self.request.certificate.projection {
                return Err(CertificateRefusal::ProjectionMismatch.into());
            }
            self.resolution.consumed.pages += 1;
            self.resolution.consumed.rows += page.rows.len();
            self.resolution.observations.rows += page.rows.len();
            self.resolution.distinct_keys += page.distinct_keys;
            if !self.judge(&page.rows)? {
                return Ok(());
            }
            match page.next {
                Some(next) => cursor = Some(next),
                None => return Ok(()),
            }
        }
    }

    fn judge(&mut self, rows: &[AssociationRow]) -> Result<bool, ResolveRefusal> {
        let live: Vec<&AssociationRow> = rows
            .iter()
            .filter(|row| match row.tombstone {
                Some(tombstone) => {
                    self.resolution.observations.tombstoned += 1;
                    self.disqualify(Disqualification::Tombstoned(tombstone.reason));
                    false
                }
                None => true,
            })
            .collect();
        if live.is_empty() {
            return Ok(true);
        }
        let candidates: Vec<OccurrenceCandidate> = live
            .iter()
            .map(|row| RetainedOccurrence::from_row(row).candidate())
            .collect();
        let report = judge_occurrences(
            self.kernel,
            self.request.authority.project,
            self.request.authority.destination,
            &candidates,
        )?;
        let target = self
            .kernel
            .capture_commit_read_target_within_budget(self.budget)?;
        if target.incarnation != self.initial.incarnation {
            self.disqualify(Disqualification::SnapshotChanged);
            self.stop(IncompleteReason::KernelIncarnationChanged);
            return Ok(false);
        }
        self.resolution.consumed.validated += candidates.len();
        if report.snapshot.classification_generation.is_none() {
            self.disqualify(Disqualification::ClassificationUnknown);
        }
        match self.resolution.snapshot {
            None => self.resolution.snapshot = Some(report.snapshot),
            Some(previous) if previous != report.snapshot => {
                self.disqualify(Disqualification::SnapshotChanged);
            }
            Some(_) => {}
        }
        for (row, judged) in live.iter().zip(&report.occurrences) {
            debug_assert_eq!(judged.occurrence_id, row.occurrence_id);
            match judged.disposition {
                Disposition::Eligible => {
                    self.resolution.observations.record(EligibilityVerdict::Ok);
                    let retained = RetainedOccurrence::from_row(row);
                    let bytes = retained.retained_bytes();
                    let bounds = self.request.bounds;
                    let consumed = self.resolution.consumed;
                    if consumed.retained == bounds.max_retained.get()
                        || consumed.retained_bytes + bytes > bounds.max_retained_bytes.get()
                    {
                        self.stop(IncompleteReason::RetentionExhausted);
                        return Ok(false);
                    }
                    self.resolution.consumed.retained += 1;
                    self.resolution.consumed.retained_bytes += bytes;
                    self.resolution.targets.insert(retained.target_id.clone());
                    self.resolution.retained.push(retained);
                }
                Disposition::PolicyExcluded(verdict) => {
                    self.resolution.observations.record(verdict);
                    self.disqualify(Disqualification::Verdict(verdict));
                }
            }
        }
        Ok(true)
    }

    fn finish(mut self) -> Resolution {
        if self.resolution.completion == Completion::Complete
            && self.resolution.observations.rows == 0
        {
            self.resolution.completion = Completion::NoMatch;
        }
        let sha_unique = match self.request.query {
            ExactQuery::Sha(_) => self.resolution.distinct_keys == 1,
            ExactQuery::CanonicalObject(_) => true,
        };
        if self.request.whole_request
            && self.resolution.completion == Completion::Complete
            && self.resolution.disqualified.is_none()
            && self.resolution.targets.len() == 1
            && sha_unique
            && let Some(snapshot) = self.resolution.snapshot
        {
            self.resolution.proof = Some(ExactProof {
                target_id: self
                    .resolution
                    .targets
                    .iter()
                    .next()
                    .cloned()
                    .unwrap_or_default(),
                occurrences: self.resolution.retained.clone(),
                certificate: self.request.certificate.clone(),
                project: self.request.authority.project.clone(),
                destination: self.request.authority.destination,
                snapshot,
                kernel: self.initial,
            });
        }
        self.resolution
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProofInvalidation {
    BudgetExhausted,
    ProjectionChanged,
    IncarnationChanged,
    CanonicalChanged {
        proven: i64,
        current: i64,
    },
    SnapshotChanged,
    AuthorityMismatch,
    Rejected {
        occurrence_id: String,
        verdict: EligibilityVerdict,
    },
}

pub fn validate_for_use(
    conn: &GuardedConn<'_>,
    kernel: &KernelStore,
    proof: &ExactProof,
    authority: Authority<'_>,
    budget: &EvalBudget,
) -> Result<Result<(), ProofInvalidation>, ResolveRefusal> {
    if budget.is_exhausted() {
        return Ok(Err(ProofInvalidation::BudgetExhausted));
    }
    if *authority.project != proof.project || authority.destination != proof.destination {
        return Ok(Err(ProofInvalidation::AuthorityMismatch));
    }
    let certificate = &proof.certificate;
    let checkpoint = read_checkpoint(conn, &certificate.canonical_incarnation_id)?;
    if checkpoint.as_ref() != Some(&certificate.projection) {
        return Ok(Err(ProofInvalidation::ProjectionChanged));
    }
    if kernel.database_incarnation_id_within_budget(budget)? != certificate.canonical_incarnation_id
    {
        return Ok(Err(ProofInvalidation::IncarnationChanged));
    }
    let target = kernel.capture_commit_read_target_within_budget(budget)?;
    if target.incarnation != proof.kernel.incarnation {
        return Ok(Err(ProofInvalidation::IncarnationChanged));
    }
    if target.through_commit != proof.kernel.through_commit {
        return Ok(Err(ProofInvalidation::CanonicalChanged {
            proven: proof.kernel.through_commit,
            current: target.through_commit,
        }));
    }
    let candidates: Vec<OccurrenceCandidate> = proof
        .occurrences
        .iter()
        .map(RetainedOccurrence::candidate)
        .collect();
    let report = judge_occurrences(
        kernel,
        authority.project,
        authority.destination,
        &candidates,
    )?;
    if report.snapshot != proof.snapshot {
        return Ok(Err(ProofInvalidation::SnapshotChanged));
    }
    for judged in &report.occurrences {
        if let Disposition::PolicyExcluded(verdict) = judged.disposition {
            return Ok(Err(ProofInvalidation::Rejected {
                occurrence_id: judged.occurrence_id.clone(),
                verdict,
            }));
        }
    }
    Ok(Ok(()))
}
