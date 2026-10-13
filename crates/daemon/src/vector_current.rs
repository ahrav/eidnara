//! The current-generation owner: one verified view of the selected vector composition, shared by every request that ranks through it and replaced only when the selector moves.
//!
//! Acquisition hashes every member of a composition, so the owner acquires once and hands the same view to each caller until the store's selector names another composition, the view is quarantined, or the caller expects another identity. A replaced view stays alive, with its pins and reservations, for as long as a caller still holds it.

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use retrieval::dense::{CandidatePolicy, ScanBounds, StorageBounds};

use crate::projection_gates::{ActivationGrant, Admission};
use crate::query_route::{CompressedLimits, DenseVectors};
use crate::vector_admission::Ledger;
use crate::vector_generation::ExpectedVectors;
use crate::vector_reader::{AcquireEvent, AcquireRefusal, PinnedVectors, ReaderBounds, acquire};

/// Shares one verified view of the selected composition and reacquires it only when the selector moves.
pub struct CurrentVectors {
    root: Option<PathBuf>,
    bounds: ReaderBounds,
    ledger: Arc<Ledger>,
    grant: Admission,
    held: Mutex<Option<Arc<PinnedVectors>>>,
}

impl CurrentVectors {
    /// An owner over the store at `root`, charging each view to `ledger` under `grant`.
    pub fn new(
        root: Option<PathBuf>,
        bounds: ReaderBounds,
        ledger: Arc<Ledger>,
        grant: Admission,
    ) -> Self {
        Self {
            root,
            bounds,
            ledger,
            grant,
            held: Mutex::new(None),
        }
    }

    /// The view callers share. The held view is returned while it is unquarantined, carries `expected`'s identity, and the selector still says what it said at acquisition; otherwise the owner releases its hold and acquires again, reporting each acquisition step to `observe`. Callers serialize here, so at most one acquisition runs at a time. Run it on a blocking thread.
    ///
    /// # Errors
    ///
    /// The refusals of [`acquire`], and of reading the selector for the held view; the owner then holds no view.
    pub fn view(
        &self,
        expected: &ExpectedVectors<'_>,
        observe: &mut dyn FnMut(AcquireEvent),
    ) -> Result<Arc<PinnedVectors>, AcquireRefusal> {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(view) = held.as_ref()
            && !view.is_quarantined()
            && view.matches(expected)
        {
            match view.selector_holds(self.root.as_deref()) {
                Ok(true) => return Ok(Arc::clone(view)),
                Ok(false) => {}
                Err(refusal) => {
                    *held = None;
                    return Err(refusal);
                }
            }
        }
        // The owner's hold goes first, so a replaced view's charges leave the ledger as soon as its last request does.
        *held = None;
        let view = acquire(
            self.root.as_deref(),
            expected,
            self.bounds,
            &self.ledger,
            &self.grant,
            observe,
        )?;
        *held = Some(Arc::clone(&view));
        Ok(view)
    }

    /// The route's dense vectors over the shared view, charged under this owner's grant and used under `activation`.
    ///
    /// # Errors
    ///
    /// As [`Self::view`].
    pub fn dense_vectors(
        &self,
        expected: &ExpectedVectors<'_>,
        activation: ActivationGrant,
        limits: CompressedLimits,
    ) -> Result<DenseVectors, AcquireRefusal> {
        let view = self.view(expected, &mut |_| {})?;
        Ok(DenseVectors::new(
            view,
            self.grant.clone(),
            activation,
            limits,
        ))
    }

    /// Releases the held view; requests that still hold it keep it until they finish.
    pub fn release(&self) {
        let retired = self
            .held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        drop(retired);
    }
}

impl CompressedLimits {
    /// #825 D27's production bounds: one global pool of `4 * k` candidates, at most 256, for the route's `k` of 64, scanned on one thread in pages one kernel eligibility batch holds, over a base and its deltas.
    pub fn production() -> Self {
        let n = |value: usize| NonZeroUsize::new(value).expect("production bounds are positive");
        Self {
            candidates: CandidatePolicy {
                alpha: 4.0,
                cap: n(256),
            },
            scan: ScanBounds {
                page_rows: n(256),
                max_rows: n(1_500_000),
                storage: StorageBounds {
                    batch_bytes: n(4 << 20),
                    heap_bytes: n(4 << 20),
                },
            },
            max_entries: n(3_000_000),
            max_layers: n(8),
            max_pinned_bytes: 8 << 30,
            max_read_bytes: 4 << 20,
        }
    }
}
