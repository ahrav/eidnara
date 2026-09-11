//! Runs one bounded identity sweep over the search projection: selects finished, unreferenced embedding identities, asks the in-process Synapse host whether it still holds any of them, and reclaims the rest inside one fenced write transaction that rechecks eligibility row by row.
//!
//! A host job is a physical holder: while this component's table still holds it, its native input or result lease is alive, so a row cut short while the job ran stays whatever its logical state says. A job issued by another incarnation has no holder left. An `obsolete` row never gains a holder again, because only pending rows are admitted, so a holder check taken before the write transaction cannot go stale in the deleting direction. A reclamation whose COMMIT reply is lost is reconciled from the rows themselves; deleting the same identity twice has no second effect. Integrity and storage failures quarantine the sweeper and retain every cleanup obligation.

use std::num::NonZeroUsize;

use host_runtime::synapse::SynapseComponent;
use kernel::applicability::EvalBudget;
use retrieval::identity_sweep::{Candidate, candidates, presence, reclaim};

use crate::search_projection::{SearchProjection, SearchProjectionError};
use crate::search_writer::{Quarantine, QuarantineKind};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Finished, unreferenced identities the selection found, before any holder or recheck excluded them.
    pub candidates: usize,
    /// Candidates the host still holds; their rows stay until the holder exits.
    pub held: Vec<Candidate>,
    pub vectors_reclaimed: usize,
    pub jobs_reclaimed: usize,
    /// Candidates whose eligibility no longer held when the delete ran.
    pub survivors: usize,
    /// The budget ended before selection or before the write; whatever it left is ready for the next sweep.
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
    synapse: &'a SynapseComponent,
    quarantine: Option<Quarantine>,
    lose_reclaim_reply: bool,
}

impl<'a> IdentitySweeper<'a> {
    pub fn new(projection: &'a SearchProjection, synapse: &'a SynapseComponent) -> Self {
        Self {
            projection,
            synapse,
            quarantine: None,
            lose_reclaim_reply: false,
        }
    }

    /// Makes the next reclamation return as if its COMMIT reply were lost after the store applied it, so the reconciliation path can be exercised.
    #[cfg(feature = "test-support")]
    pub fn lose_next_reclaim_reply_for_test(&mut self) {
        self.lose_reclaim_reply = true;
    }

    /// Selects at most `max_candidates` identities and reclaims those no holder protects. The budget is checked before selection and again before the write: an exhausted budget selects nothing, or leaves every free candidate as a survivor for the next sweep, and the report says the budget ended it.
    ///
    /// # Errors
    ///
    /// Returns [`SweepError::Read`] when selection fails and [`SweepError::Quarantined`] once a reclamation is refused or its outcome cannot be reconciled, and on every later call of this sweeper.
    pub fn run_sweep(
        &mut self,
        max_candidates: NonZeroUsize,
        budget: &EvalBudget,
    ) -> Result<SweepReport, SweepError> {
        if let Some(quarantine) = &self.quarantine {
            return Err(SweepError::Quarantined(quarantine.clone()));
        }
        if budget.is_exhausted() {
            return Ok(SweepReport {
                budget_exhausted: true,
                ..SweepReport::default()
            });
        }
        let selected = self
            .projection
            .read(|conn| candidates(conn, max_candidates))
            .map_err(SweepError::Read)?;
        let mut report = SweepReport {
            candidates: selected.len(),
            ..SweepReport::default()
        };
        let (held, free): (Vec<Candidate>, Vec<Candidate>) = selected
            .into_iter()
            .partition(|candidate| self.holds(candidate));
        report.held = held;
        if free.is_empty() {
            return Ok(report);
        }
        if budget.is_exhausted() {
            report.survivors = free.len();
            report.budget_exhausted = true;
            return Ok(report);
        }
        let reclaimed = self.projection.write(|conn| reclaim(conn, &free));
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

    /// Whether this host still holds the candidate's job. A consumed result (`embedded`, `published`) has no holder left that matters; an `obsolete` row was cut short, and its job is held while this component's table still answers for it. A job another incarnation issued is never held.
    fn holds(&self, candidate: &Candidate) -> bool {
        candidate.state == "obsolete"
            && candidate
                .host_job_id
                .as_deref()
                .is_some_and(|job_id| self.synapse.holds_job(job_id))
    }

    fn enter_quarantine(
        &mut self,
        kind: QuarantineKind,
        error: &dyn std::fmt::Display,
    ) -> SweepError {
        let quarantine = Quarantine::new(kind, error);
        self.quarantine = Some(quarantine.clone());
        SweepError::Quarantined(quarantine)
    }
}
