//! Runs one bounded identity sweep over the search projection: selects finished, unreferenced embedding identities, asks the in-process LocalEmbeddings host whether it still holds any of them, and reclaims the rest inside one fenced write transaction that rechecks eligibility row by row.
//!
//! A live native lease, component-table entry, or served result page prevents row reclamation. A holder check taken before the write transaction cannot go stale in the deleting direction: only pending rows are admitted, so an `obsolete` row never gains a holder again, and the dispatcher obtains the page it holds across publication before the row finishes, so a finished row's protecting page exists at check time. A job issued by another incarnation has no holder left. Lost COMMIT replies reconcile from stored rows, and repeated deletion is idempotent. Integrity or storage failures quarantine the projection and preserve cleanup obligations.

use std::num::NonZeroUsize;

use host_runtime::local_embeddings::LocalEmbeddingsComponent;
use kernel::applicability::EvalBudget;
use retrieval::identity_sweep::{Candidate, candidates, presence, reclaim};
use tokio_util::sync::CancellationToken;

use crate::search_projection::{SearchProjection, SearchProjectionError};
use crate::search_writer::{Quarantine, QuarantineKind};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Job rows inspected, including identities that remain referenced or unfinished.
    pub inspected: usize,
    /// Finished, unreferenced identities the selection found, before any holder or recheck excluded them.
    pub candidates: usize,
    /// Candidates the host still holds; their rows stay until the holder exits.
    pub held: Vec<Candidate>,
    pub vectors_reclaimed: usize,
    pub jobs_reclaimed: usize,
    /// Candidates whose eligibility no longer held when the delete ran.
    pub survivors: usize,
    /// The budget ended before selection or before the write; the free candidates it left are deferred, not survivors, and the next sweep selects them again.
    pub budget_exhausted: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum SweepError {
    #[error("identity sweep is quarantined: {}", .0.detail)]
    Quarantined(Quarantine),
    /// Candidate selection failed before anything was decided; the sweep may simply run again.
    #[error(transparent)]
    Read(SearchProjectionError),
}

/// Sweeps one projection against one in-process host. `run_sweep` blocks on the projection and belongs on a blocking thread.
pub struct IdentitySweeper<'a> {
    projection: &'a SearchProjection,
    local_embeddings: &'a LocalEmbeddingsComponent,
    cursor: Option<String>,
    invalidated: Option<CancellationToken>,
    lose_reclaim_reply: bool,
}

impl<'a> IdentitySweeper<'a> {
    pub fn new(
        projection: &'a SearchProjection,
        local_embeddings: &'a LocalEmbeddingsComponent,
    ) -> Self {
        Self::resuming(projection, local_embeddings, None)
    }

    /// A sweeper whose first selection starts after `cursor`, the job identifier a previous sweeper's [`Self::cursor`] handed back; `None` starts at the first identity.
    pub fn resuming(
        projection: &'a SearchProjection,
        local_embeddings: &'a LocalEmbeddingsComponent,
        cursor: Option<String>,
    ) -> Self {
        Self {
            projection,
            local_embeddings,
            cursor,
            invalidated: None,
            lose_reclaim_reply: false,
        }
    }

    /// Ends the sweep where the budget would when `invalidated` is cancelled: the grant the sweep runs under was revoked.
    pub fn cancelled_by(mut self, invalidated: CancellationToken) -> Self {
        self.invalidated = Some(invalidated);
        self
    }

    fn ended(&self, budget: &EvalBudget) -> bool {
        budget.is_exhausted()
            || self
                .invalidated
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled)
    }

    /// Where the next selection resumes: the job identifier the last full page ended at, or `None` once a pass over the table is complete. A caller that builds a sweeper per sweep threads this through [`Self::resuming`] so held identities at the head cannot starve those behind them.
    pub fn cursor(&self) -> Option<&str> {
        self.cursor.as_deref()
    }

    /// Makes the next reclamation return as if its COMMIT reply were lost after the store applied it, so the reconciliation path can be exercised.
    #[cfg(feature = "test-support")]
    pub fn lose_next_reclaim_reply_for_test(&mut self) {
        self.lose_reclaim_reply = true;
    }

    /// Inspects at most `max_jobs` job rows and reclaims finished, unreferenced identities that no holder protects.
    /// Selection advances past every inspected row, even on candidate-free pages, and wraps after a short page.
    /// Keeping `cursor` across calls prevents live or held rows from starving later identities.
    /// The budget, and the grant token when one is attached, is checked before selection and again before the write: an exhausted budget selects nothing, or leaves the selected page and cursor unchanged for the next sweep, and the report says the budget ended it.
    ///
    /// # Errors
    ///
    /// Returns [`SweepError::Read`] when selection fails and [`SweepError::Quarantined`] once a reclamation is refused, its outcome cannot be reconciled, or any writer has quarantined the projection.
    pub fn run_sweep(
        &mut self,
        max_jobs: NonZeroUsize,
        budget: &EvalBudget,
    ) -> Result<SweepReport, SweepError> {
        if let Some(quarantine) = self.projection.quarantine() {
            return Err(SweepError::Quarantined(quarantine));
        }
        if self.ended(budget) {
            return Ok(SweepReport {
                budget_exhausted: true,
                ..SweepReport::default()
            });
        }
        let mut report = SweepReport::default();
        // Both connection acquisitions end at the budget's deadline when it has one; a connection still held then leaves the page and cursor for the next sweep.
        let select =
            |conn: &storage::GuardedConn<'_>| candidates(conn, max_jobs, self.cursor.as_deref());
        let page = match budget.deadline() {
            Some(deadline) => self.projection.read_within(deadline, select),
            None => self.projection.read(select),
        };
        let page = match page {
            Ok(page) => page,
            Err(SearchProjectionError::Store(storage::StoreError::Deadline)) => {
                return Ok(SweepReport {
                    budget_exhausted: true,
                    ..SweepReport::default()
                });
            }
            Err(error) => return Err(SweepError::Read(error)),
        };
        report.inspected = page.inspected;
        report.candidates = page.candidates.len();
        // A short page ends one pass over the table; a full page resumes after its last row once this call has judged it.
        let next_cursor = if page.inspected < max_jobs.get() {
            None
        } else {
            page.last_job_id
        };
        let mut free = Vec::with_capacity(page.candidates.len());
        for candidate in page.candidates {
            if self.holds(&candidate) {
                report.held.push(candidate);
            } else {
                free.push(candidate);
            }
        }
        if free.is_empty() {
            self.cursor = next_cursor;
            return Ok(report);
        }
        if self.ended(budget) {
            // The cursor stays before this page: nothing in it was reclaimed, so the next sweep selects it again. No delete ran, so no candidate is a survivor of one.
            report.budget_exhausted = true;
            return Ok(report);
        }
        let prior_cursor = std::mem::replace(&mut self.cursor, next_cursor);
        // The budget and grant are judged once more inside the transaction, so a cancellation that lands while the connection was awaited commits nothing.
        let ended = |conn: &storage::GuardedConn<'_>| {
            if self.ended(budget) {
                return Ok(None);
            }
            reclaim(conn, &free).map(Some)
        };
        let reclaimed = match budget.deadline() {
            Some(deadline) => self.projection.write_within(deadline, ended),
            None => self.projection.write(ended),
        };
        let reclaimed = match reclaimed {
            Ok(Some(reclaimed)) => Ok(reclaimed),
            // Nothing was applied: at the deadline the connection was still held, or the budget ended once it was ours. The cursor returns to before this page.
            Ok(None) | Err(SearchProjectionError::Store(storage::StoreError::Deadline)) => {
                self.cursor = prior_cursor;
                report.budget_exhausted = true;
                return Ok(report);
            }
            Err(error) => Err(error),
        };
        let reclaimed = if std::mem::take(&mut self.lose_reclaim_reply) && reclaimed.is_ok() {
            Err(SearchProjectionError::Store(storage::StoreError::Backend(
                "database is locked".to_owned(),
            )))
        } else {
            reclaimed
        };
        match reclaimed {
            Ok(reclaimed) => {
                report.vectors_reclaimed = reclaimed.vectors;
                report.jobs_reclaimed = reclaimed.jobs;
                report.survivors = reclaimed.survivors;
            }
            // Every refusal reaching here is the store's own error, so nothing was applied and the store is suspect.
            Err(SearchProjectionError::Projection(error)) => {
                return Err(self.enter_quarantine(QuarantineKind::Storage, &error));
            }
            // A backend reply was lost: the rows, not the error, say what the store applied. A row still present was not reclaimed and stays a candidate for the next sweep.
            Err(lost @ SearchProjectionError::Store(storage::StoreError::Backend(_))) => {
                for candidate in &free {
                    match self.projection.read(|conn| presence(conn, candidate)) {
                        Ok((true, _)) => report.survivors += 1,
                        Ok((false, vector_present)) => {
                            report.jobs_reclaimed += 1;
                            report.vectors_reclaimed +=
                                usize::from(candidate.has_vector && !vector_present);
                        }
                        Err(_) => {
                            return Err(self.enter_quarantine(QuarantineKind::Storage, &lost));
                        }
                    }
                }
            }
            // A fence loss, lease loss, I/O failure, or connection fault is not an unknown outcome; it is a store the sweeper must stop using.
            Err(error) => return Err(self.enter_quarantine(QuarantineKind::Storage, &error)),
        }
        Ok(report)
    }

    /// Whether this host still holds the candidate's job. An `obsolete` row is held while the component's table answers for its job or a served page keeps its result alive. Any other finished row is held only while a served result page for its job is alive. A job another incarnation issued is never held.
    fn holds(&self, candidate: &Candidate) -> bool {
        let Some(job_id) = candidate.host_job_id.as_deref() else {
            return false;
        };
        if candidate.state == "obsolete" {
            self.local_embeddings.holds_job(job_id)
        } else {
            self.local_embeddings.holds_result_page(job_id)
        }
    }

    fn enter_quarantine(&self, kind: QuarantineKind, error: &dyn std::fmt::Display) -> SweepError {
        SweepError::Quarantined(self.projection.enter_quarantine(kind, error))
    }
}
