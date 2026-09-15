use std::collections::BTreeSet;
use std::num::NonZeroUsize;

use kernel::applicability::EvalBudget;
use kernel::{
    ArtifactDestination, CommitReadTarget, EgressSnapshot, EligibilityVerdict, KernelError,
    KernelStore, MAX_ELIGIBILITY_CANDIDATES, ProjectScope,
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
    pub inventory_epoch: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolveBounds {
    pub page_rows: NonZeroUsize,
    pub max_rows: NonZeroUsize,
    pub max_pages: NonZeroUsize,
    pub max_retained: NonZeroUsize,
    pub max_retained_bytes: NonZeroUsize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestIntent {
    WholeRequest,
    Mention,
}

pub struct ResolveRequest<'a> {
    pub query: ExactQuery<'a>,
    pub intent: RequestIntent,
    pub certificate: &'a CompletenessCertificate,
    pub authority: Authority<'a>,
    pub bounds: ResolveBounds,
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
    /// The kernel tip and the certified horizon disagree in either direction:
    /// a tip ahead of the horizon means an unprojected collision may exist,
    /// and a tip behind it means the projection may describe history a
    /// restore rolled back.
    ProjectionLag {
        tip: i64,
        complete_through: i64,
    },
    ClassificationUnknown,
    SnapshotChanged,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Observations {
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

/// Egress eligibility only; surface permission and checkout applicability
/// are judged elsewhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactProof {
    target_id: String,
    occurrences: Vec<AssociationRow>,
    certificate: CompletenessCertificate,
    project: ProjectScope,
    destination: ArtifactDestination,
    snapshot: EgressSnapshot,
    kernel: CommitReadTarget,
}

impl ExactProof {
    pub fn target_id(&self) -> &str {
        &self.target_id
    }

    pub fn occurrences(&self) -> &[AssociationRow] {
        &self.occurrences
    }

    pub fn certificate(&self) -> &CompletenessCertificate {
        &self.certificate
    }

    pub fn snapshot(&self) -> EgressSnapshot {
        self.snapshot
    }

    pub fn kernel(&self) -> CommitReadTarget {
        self.kernel
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub completion: Completion,
    /// The first observation that bars bypass; later eligible rows never clear it.
    pub disqualified: Option<Disqualification>,
    /// Targets among the retained rows, whatever the completion.
    pub observed_targets: BTreeSet<String>,
    pub retained: Vec<AssociationRow>,
    pub observations: Observations,
    pub consumed: Consumed,
    pub distinct_keys: usize,
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
    #[error("max retained exceeds the kernel's {MAX_ELIGIBILITY_CANDIDATES} candidate bound")]
    RetainedOverBound,
    #[error("the kernel returned {verdicts} verdicts for {candidates} candidates")]
    VerdictMismatch { candidates: usize, verdicts: usize },
    #[error(transparent)]
    Certificate(#[from] CertificateRefusal),
    #[error(transparent)]
    Lookup(#[from] LookupRefusal),
    #[error(transparent)]
    Projection(#[from] ProjectionError),
    #[error("kernel: {0}")]
    Kernel(#[from] KernelError),
}

fn candidate(row: &AssociationRow) -> OccurrenceCandidate {
    OccurrenceCandidate::new(
        row.occurrence_id.clone(),
        row.class,
        row.source_object_id.clone(),
        row.revision,
        row.source_artifact_digest.clone(),
    )
}

fn retained_bytes(row: &AssociationRow) -> usize {
    row.key.len()
        + row.target_id.len()
        + row.occurrence_id.len()
        + row.lineage_id.len()
        + row.representation.len()
        + row.payload_id.len()
        + row.source_object_id.len()
        + row.source_evidence_id.len()
        + row.source_artifact_digest.len()
}

struct Attempt<'a> {
    request: &'a ResolveRequest<'a>,
    kernel: &'a KernelStore,
    budget: &'a EvalBudget,
    initial: CommitReadTarget,
    snapshot: Option<EgressSnapshot>,
    stopped: Option<IncompleteReason>,
    resolution: Resolution,
}

fn budget_refusal(error: KernelError) -> ResolveRefusal {
    match error {
        KernelError::Deadline => ResolveRefusal::BudgetExhausted,
        other => ResolveRefusal::Kernel(other),
    }
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
    // Final use re-judges every proof occurrence in one kernel batch, so
    // retention past the batch bound could mint a proof no use can validate.
    if request.bounds.max_retained.get() > MAX_ELIGIBILITY_CANDIDATES {
        return Err(ResolveRefusal::RetainedOverBound);
    }
    let certificate = request.certificate;
    admit_certificate(conn, request)?;
    let installed = kernel
        .database_incarnation_id_within_budget(budget)
        .map_err(budget_refusal)?;
    if installed != certificate.canonical_incarnation_id {
        return Err(CertificateRefusal::IncarnationMismatch.into());
    }
    let initial = kernel
        .capture_commit_read_target_within_budget(budget)
        .map_err(budget_refusal)?;
    let mut attempt = Attempt {
        request,
        kernel,
        budget,
        initial,
        snapshot: None,
        stopped: None,
        resolution: Resolution {
            completion: Completion::Complete,
            disqualified: None,
            observed_targets: BTreeSet::new(),
            retained: Vec::new(),
            observations: Observations::default(),
            consumed: Consumed::default(),
            distinct_keys: 0,
            proof: None,
        },
    };
    if initial.through_commit != certificate.complete_through_commit_seq {
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
    if request.authority.inventory_epoch != certificate.inventory_epoch {
        return Err(CertificateRefusal::InventoryEpochMismatch {
            certified: certificate.inventory_epoch.clone(),
            current: request.authority.inventory_epoch.to_string(),
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
        self.stopped.get_or_insert(reason);
    }

    fn late_failure(&mut self, refusal: ResolveRefusal) -> Result<(), ResolveRefusal> {
        match &refusal {
            ResolveRefusal::Lookup(LookupRefusal::BudgetExhausted)
            | ResolveRefusal::Kernel(KernelError::Deadline) => {}
            _ => return Err(refusal),
        }
        if self.resolution.consumed.pages == 0 {
            return Err(ResolveRefusal::BudgetExhausted);
        }
        self.stop(IncompleteReason::BudgetExhausted);
        Ok(())
    }

    fn enumerate(&mut self, conn: &GuardedConn<'_>) -> Result<(), ResolveRefusal> {
        let bounds = self.request.bounds;
        let mut cursor: Option<Cursor> = None;
        loop {
            if self.stopped.is_some() {
                return Ok(());
            }
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
            let page = match page(conn, &context, &self.request.query, cursor.as_ref()) {
                Ok(page) => page,
                Err(refusal) => return self.late_failure(refusal.into()),
            };
            self.resolution.consumed.pages += 1;
            self.resolution.consumed.rows += page.rows.len();
            self.resolution.distinct_keys += page.distinct_keys;
            if let Err(refusal) = self.judge(&page.rows) {
                return self.late_failure(refusal);
            }
            match page.next {
                Some(next) => cursor = Some(next),
                None => return Ok(()),
            }
        }
    }

    fn judge(&mut self, rows: &[AssociationRow]) -> Result<(), ResolveRefusal> {
        let mut live = Vec::with_capacity(rows.len());
        for row in rows {
            match row.tombstone {
                Some(tombstone) => {
                    self.resolution.observations.tombstoned += 1;
                    self.disqualify(Disqualification::Tombstoned(tombstone.reason));
                }
                None => live.push(row),
            }
        }
        if live.is_empty() {
            return Ok(());
        }
        let candidates: Vec<OccurrenceCandidate> = live.iter().map(|row| candidate(row)).collect();
        let report = judge_occurrences(
            self.kernel,
            self.request.authority.project,
            self.request.authority.destination,
            &candidates,
        )?;
        if report.occurrences.len() != live.len() {
            return Err(ResolveRefusal::VerdictMismatch {
                candidates: live.len(),
                verdicts: report.occurrences.len(),
            });
        }
        self.resolution.consumed.validated += candidates.len();
        let target = self
            .kernel
            .capture_commit_read_target_within_budget(self.budget)?;
        if target.incarnation != self.initial.incarnation {
            self.disqualify(Disqualification::SnapshotChanged);
            self.stop(IncompleteReason::KernelIncarnationChanged);
            return Ok(());
        }
        if report.snapshot.classification_generation.is_none() {
            self.disqualify(Disqualification::ClassificationUnknown);
        }
        if report.snapshot.tip != self.initial.through_commit
            || self
                .snapshot
                .is_some_and(|previous| previous != report.snapshot)
        {
            self.disqualify(Disqualification::SnapshotChanged);
        }
        self.snapshot.get_or_insert(report.snapshot);
        for (row, judged) in live.iter().zip(&report.occurrences) {
            match judged.disposition {
                Disposition::Eligible => self.retain(row),
                Disposition::PolicyExcluded(verdict) => {
                    self.resolution.observations.record(verdict);
                    self.disqualify(Disqualification::Verdict(verdict));
                }
            }
        }
        Ok(())
    }

    fn retain(&mut self, row: &AssociationRow) {
        if self.stopped.is_some() {
            return;
        }
        let bounds = self.request.bounds;
        let consumed = self.resolution.consumed;
        let bytes = retained_bytes(row);
        if consumed.retained == bounds.max_retained.get()
            || consumed.retained_bytes + bytes > bounds.max_retained_bytes.get()
        {
            self.stop(IncompleteReason::RetentionExhausted);
            return;
        }
        self.resolution.observations.record(EligibilityVerdict::Ok);
        self.resolution.consumed.retained += 1;
        self.resolution.consumed.retained_bytes += bytes;
        self.resolution
            .observed_targets
            .insert(row.target_id.clone());
        self.resolution.retained.push(row.clone());
    }

    fn finish(mut self) -> Resolution {
        self.resolution.completion = match self.stopped {
            Some(reason) => Completion::Incomplete(reason),
            None if self.resolution.consumed.rows == 0 => Completion::NoMatch,
            None => Completion::Complete,
        };
        let sha_unique = match self.request.query {
            ExactQuery::Sha(_) => self.resolution.distinct_keys == 1,
            ExactQuery::CanonicalObject(_) => true,
        };
        if self.request.intent == RequestIntent::WholeRequest
            && self.resolution.completion == Completion::Complete
            && self.resolution.disqualified.is_none()
            && self.resolution.observed_targets.len() == 1
            && sha_unique
            && let Some(snapshot) = self.snapshot
            && let Some(target) = self.resolution.observed_targets.first()
        {
            self.resolution.proof = Some(ExactProof {
                target_id: target.clone(),
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
    AuthorityMismatch,
    InventoryEpochChanged,
    ProjectionChanged,
    IncarnationChanged,
    CanonicalChanged {
        proven: i64,
        current: i64,
    },
    SnapshotChanged,
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
    if authority.inventory_epoch != certificate.inventory_epoch {
        return Ok(Err(ProofInvalidation::InventoryEpochChanged));
    }
    let checkpoint = read_checkpoint(conn, &certificate.canonical_incarnation_id)?;
    if checkpoint.as_ref() != Some(&certificate.projection) {
        return Ok(Err(ProofInvalidation::ProjectionChanged));
    }
    let kernel_state = (|| -> Result<_, KernelError> {
        let incarnation = kernel.database_incarnation_id_within_budget(budget)?;
        let target = kernel.capture_commit_read_target_within_budget(budget)?;
        Ok((incarnation, target))
    })();
    let (incarnation, target) = match kernel_state {
        Ok(state) => state,
        Err(KernelError::Deadline) => return Ok(Err(ProofInvalidation::BudgetExhausted)),
        Err(error) => return Err(error.into()),
    };
    if incarnation != certificate.canonical_incarnation_id
        || target.incarnation != proof.kernel.incarnation
    {
        return Ok(Err(ProofInvalidation::IncarnationChanged));
    }
    if target.through_commit != proof.kernel.through_commit {
        return Ok(Err(ProofInvalidation::CanonicalChanged {
            proven: proof.kernel.through_commit,
            current: target.through_commit,
        }));
    }
    let candidates: Vec<OccurrenceCandidate> = proof.occurrences.iter().map(candidate).collect();
    let report = match judge_occurrences(
        kernel,
        authority.project,
        authority.destination,
        &candidates,
    ) {
        Ok(report) => report,
        Err(KernelError::Deadline) => return Ok(Err(ProofInvalidation::BudgetExhausted)),
        Err(error) => return Err(error.into()),
    };
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
