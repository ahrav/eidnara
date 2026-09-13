//! Runs one bounded slice of message-index cleanup against the projection: candidate pages are read under the acknowledged kernel prefix, each page is reclaimed in one fenced write transaction that re-checks eligibility, and the cursor is carried across slices so an interrupted slice resumes where it stopped. One `EvalBudget` gates every admission, and its deadline bounds the wait for the projection connection before selection and for the write lock; cancellation leaves whatever committed, since each committed page is complete on its own.

use std::num::NonZeroUsize;
use std::time::Instant;

use kernel::applicability::EvalBudget;
use retrieval::ProjectionError;
use retrieval::message_cleanup::{Candidate, Reclaimed, candidates, present, reclaim};
use storage::{GuardedConn, StoreError};

use crate::projection_gates::{Denial, EntryPoint, HookGate, ProjectionHook};
use crate::search_projection::{SearchProjection, SearchProjectionError};

/// Bounds one slice: how many tombstoned rows one page inspects, how many pages one slice may write, and how many rows one slice may reclaim in total.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CleanupBounds {
    pub page_rows: NonZeroUsize,
    pub max_pages: NonZeroUsize,
    pub max_reclaimed: NonZeroUsize,
}

/// Why a slice stopped before the scan was exhausted. Every committed page stands; the cursor names where the next slice resumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupStop {
    /// The budget was cancelled, its deadline passed, or the slice's grant was invalidated before the next page was admitted, or the deadline passed while waiting for the projection connection or the write lock.
    Cancelled,
    /// The slice reached its page or row bound with rows left to inspect.
    BoundReached,
    /// The store's reply to the page's write was lost at or before COMMIT. `reclaimed.occurrences` counts the admitted rows confirmed gone before the deadline; vectors and payloads count only acknowledged writes. The cursor stays where the page began, so rows still present are selected again next slice.
    Unresolved(String),
    /// The gate denied the hook before any page was read; nothing was inspected or reclaimed.
    Denied(Denial),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupReport {
    pub inspected: usize,
    pub reclaimed: Reclaimed,
    /// `None` once the scan is exhausted; otherwise the occurrence id the next slice continues after.
    pub cursor: Option<String>,
    pub stop: Option<CleanupStop>,
}

pub struct MessageCleanup<'a> {
    projection: &'a SearchProjection,
    kernel_incarnation_id: String,
    acknowledged_through: i64,
    cursor: Option<String>,
    lose_write_reply: bool,
    #[cfg(feature = "test-support")]
    after_page: Option<Box<dyn FnMut()>>,
}

impl<'a> MessageCleanup<'a> {
    /// `acknowledged_through` is the kernel consumer checkpoint the search projection's own acknowledgements reached; the store caps it at the projection's checkpoint, and tombstones above the smaller stay. A projection installed under a kernel incarnation other than `kernel_incarnation_id` is refused by every slice.
    pub fn new(
        projection: &'a SearchProjection,
        kernel_incarnation_id: String,
        acknowledged_through: i64,
    ) -> Self {
        Self {
            projection,
            kernel_incarnation_id,
            acknowledged_through,
            cursor: None,
            lose_write_reply: false,
            #[cfg(feature = "test-support")]
            after_page: None,
        }
    }

    /// Runs `hook` after each page commits and before the next page is admitted.
    #[cfg(feature = "test-support")]
    pub fn with_after_page_for_test(mut self, hook: impl FnMut() + 'static) -> Self {
        self.after_page = Some(Box::new(hook));
        self
    }

    /// Resumes a scan after `cursor`, as a slice interrupted by cancellation or a bound left it.
    pub fn resuming(mut self, cursor: Option<String>) -> Self {
        self.cursor = cursor;
        self
    }

    pub fn cursor(&self) -> Option<&str> {
        self.cursor.as_deref()
    }

    /// Makes the next page's write return as if its COMMIT reply were lost after the store applied it, so the reconciliation path can be exercised. The loss is noticed only once the budget's deadline, when it has one, has passed.
    #[cfg(feature = "test-support")]
    pub fn lose_next_write_reply_for_test(&mut self) {
        self.lose_write_reply = true;
    }

    /// Runs one slice.
    ///
    /// # Errors
    ///
    /// Returns the projection's error when a read fails, a statement fails, or the projection is quarantined, before any read when the quarantine is already in force, so the caller can quarantine as every other projection writer does; a write whose reply the store lost ends the slice in the report as [`CleanupStop::Unresolved`], with the page reconciled from its rows.
    pub fn run_slice(
        &mut self,
        gate: &HookGate,
        bounds: CleanupBounds,
        budget: &EvalBudget,
    ) -> Result<CleanupReport, SearchProjectionError> {
        if let Some(quarantine) = self.projection.quarantine() {
            return Err(SearchProjectionError::Quarantined(quarantine));
        }
        let mut report = CleanupReport {
            inspected: 0,
            reclaimed: Reclaimed::default(),
            cursor: self.cursor.clone(),
            stop: None,
        };
        let admission = match gate.admit(ProjectionHook::MessageCleanup, EntryPoint::Dispatch) {
            Ok(admission) => admission,
            Err(denial) => {
                report.stop = Some(CleanupStop::Denied(denial));
                return Ok(report);
            }
        };
        // The grant's token is polled where the budget is: a manifest installed under the slice stops it before the next read or write.
        let revoked = || budget.check().is_err() || admission.invalidated.is_cancelled();
        for _ in 0..bounds.max_pages.get() {
            if revoked() {
                report.stop = Some(CleanupStop::Cancelled);
                return Ok(report);
            }
            let acknowledged = self.acknowledged_through;
            let kernel = self.kernel_incarnation_id.as_str();
            let after = self.cursor.clone();
            let page = match read(self.projection, budget, |conn| {
                candidates(
                    conn,
                    kernel,
                    acknowledged,
                    after.as_deref(),
                    bounds.page_rows,
                )
            }) {
                Ok(page) => page,
                Err(SearchProjectionError::Store(StoreError::Deadline)) => {
                    report.stop = Some(CleanupStop::Cancelled);
                    return Ok(report);
                }
                Err(error) => return Err(error),
            };
            report.inspected += page.inspected;
            let Some(last) = page.last_occurrence_id else {
                report.cursor = None;
                self.cursor = None;
                return Ok(report);
            };
            let remaining = bounds.max_reclaimed.get() - report.reclaimed.occurrences;
            let found = page.candidates.len();
            let admitted: Vec<Candidate> = page.candidates.into_iter().take(remaining).collect();
            let truncated = admitted.len() < found;
            if revoked() {
                report.stop = Some(CleanupStop::Cancelled);
                return Ok(report);
            }
            if !admitted.is_empty() {
                let write = |conn: &GuardedConn<'_>| reclaim(conn, kernel, &admitted, acknowledged);
                let outcome = match budget.deadline() {
                    Some(deadline) => self.projection.write_within(deadline, write),
                    None => self.projection.write(write),
                };
                let outcome = if std::mem::take(&mut self.lose_write_reply) && outcome.is_ok() {
                    if let Some(deadline) = budget.deadline() {
                        std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
                    }
                    Err(SearchProjectionError::Store(StoreError::Backend(
                        "database is locked".to_owned(),
                    )))
                } else {
                    outcome
                };
                match outcome {
                    Ok(reclaimed) => {
                        report.reclaimed.occurrences += reclaimed.occurrences;
                        report.reclaimed.vectors += reclaimed.vectors;
                        report.reclaimed.payloads += reclaimed.payloads;
                        report.reclaimed.protected += reclaimed.protected;
                    }
                    Err(SearchProjectionError::Store(StoreError::Deadline)) => {
                        report.stop = Some(CleanupStop::Cancelled);
                        return Ok(report);
                    }
                    // `Backend` covers a failed COMMIT, whose outcome is unknown; the rows decide what to count, under the same deadline.
                    Err(SearchProjectionError::Store(StoreError::Backend(text))) => {
                        for candidate in &admitted {
                            match read(self.projection, budget, |conn| present(conn, candidate)) {
                                Ok(true) => {}
                                Ok(false) => report.reclaimed.occurrences += 1,
                                Err(SearchProjectionError::Store(StoreError::Deadline)) => break,
                                Err(error) => return Err(error),
                            }
                        }
                        report.stop = Some(CleanupStop::Unresolved(text));
                        return Ok(report);
                    }
                    Err(error) => return Err(error),
                }
            }
            // A page shorter than the bound that the row bound did not cut proves the scan exhausted.
            if !truncated && page.inspected < bounds.page_rows.get() {
                report.cursor = None;
                self.cursor = None;
                return Ok(report);
            }
            // A page cut by the row bound resumes at the last row it reclaimed; a whole page advances past its last inspected row.
            let cursor = if truncated {
                admitted
                    .last()
                    .map_or(last, |candidate| candidate.occurrence_id.clone())
            } else {
                last
            };
            self.cursor = Some(cursor.clone());
            report.cursor = Some(cursor);
            #[cfg(feature = "test-support")]
            if let Some(hook) = self.after_page.as_mut() {
                hook();
            }
            if report.reclaimed.occurrences >= bounds.max_reclaimed.get() {
                report.stop = Some(CleanupStop::BoundReached);
                return Ok(report);
            }
        }
        report.stop = Some(CleanupStop::BoundReached);
        Ok(report)
    }
}

/// One read whose wait for the connection ends at the budget's deadline, when it has one.
fn read<T>(
    projection: &SearchProjection,
    budget: &EvalBudget,
    f: impl FnOnce(&GuardedConn<'_>) -> Result<T, ProjectionError>,
) -> Result<T, SearchProjectionError> {
    match budget.deadline() {
        Some(deadline) => projection.read_within(deadline, f),
        None => projection.read(f),
    }
}
