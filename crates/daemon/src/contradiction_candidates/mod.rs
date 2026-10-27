//! Write-time contradiction candidates: derived rows that name a decision a
//! published descriptor overlaps lexically, for a reader to confirm or ignore.
//!
//! Every candidate surface runs only under `contradiction_candidates.enabled`.
//! Its counters carry a closed vocabulary of counts, byte totals, and
//! milliseconds, never descriptor text, ids, digests, or per-row scores.

use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use tokio::sync::Semaphore;

pub mod eligibility;
pub mod pass;
pub mod scorer;
pub mod sweep;

crate::source_capture::counts! {
    Offered => "offered",
    Admitted => "admitted",
    RefusedPassBusy => "refused_pass_busy",
    RefusedShuttingDown => "refused_shutting_down",
    StoreUnavailable => "store_unavailable",
    KernelUnavailable => "kernel_unavailable",
    DescriptorsAnalyzed => "descriptors_analyzed",
    DescriptorsRefusedOversize => "descriptors_refused_oversize",
    DescriptorsUnscored => "descriptors_unscored",
    CandidatesTruncated => "candidates_truncated",
    SummariesLoaded => "summaries_loaded",
    SummariesCutBytes => "summaries_cut_bytes",
    SummariesUnscorable => "summaries_unscorable",
    BudgetExhausted => "budget_exhausted",
    RowsWritten => "rows_written",
    DroppedAtCap => "dropped_at_cap",
    SweepExamined => "sweep_examined",
    SweepDropped => "sweep_dropped",
    SweepSkippedKernelUnavailable => "sweep_skipped_kernel_unavailable",
    ReadFilteredStale => "read_filtered_stale",
    Completed => "completed",
    Deadline => "deadline",
    WorkerPanicked => "worker_panicked",
    Active => "active",
    RetainedBytes => "retained_bytes",
    PassMsLast => "pass_ms_last",
    PassMsMax => "pass_ms_max",
}

/// A test hook that receives a descriptor's publish-order index.
#[cfg(test)]
pub(crate) type DescriptorHook = Arc<dyn Fn(usize) + Send + Sync>;

/// The daemon's candidate supervisor: the store's one pass permit and the
/// counters reported under `contradiction_candidates` in the status block.
/// `active`, `retained_bytes`, and `pass_ms_last` are gauges; every other
/// value only grows.
pub struct Candidates {
    counters: [AtomicU64; Count::ALL.len()],
    slot: Arc<Semaphore>,
    /// Bounds a test sets in place of [`pass::PassBounds::default`].
    #[cfg(test)]
    pub(crate) test_bounds: Mutex<Option<pass::PassBounds>>,
    /// Runs at the start of a pass's blocking closure, where a test parks the pass.
    #[cfg(test)]
    pub(crate) before_pass: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// Runs before each descriptor's scoring with its publish-order index.
    #[cfg(test)]
    pub(crate) before_descriptor: Mutex<Option<DescriptorHook>>,
}

impl Default for Candidates {
    fn default() -> Self {
        Self {
            counters: std::array::from_fn(|_| AtomicU64::new(0)),
            slot: Arc::new(Semaphore::new(1)),
            #[cfg(test)]
            test_bounds: Mutex::new(None),
            #[cfg(test)]
            before_pass: Mutex::new(None),
            #[cfg(test)]
            before_descriptor: Mutex::new(None),
        }
    }
}

impl Candidates {
    pub(crate) fn add(&self, count: Count, value: u64) {
        self.counters[count as usize].fetch_add(value, Ordering::Relaxed);
    }

    pub(crate) fn sub(&self, count: Count, value: u64) {
        self.counters[count as usize].fetch_sub(value, Ordering::Relaxed);
    }

    pub(crate) fn set(&self, count: Count, value: u64) {
        self.counters[count as usize].store(value, Ordering::Relaxed);
    }

    pub(crate) fn max(&self, count: Count, value: u64) {
        self.counters[count as usize].fetch_max(value, Ordering::Relaxed);
    }

    /// The bounds a pass runs under.
    pub fn bounds(&self) -> pass::PassBounds {
        #[cfg(test)]
        if let Some(bounds) = *self.test_bounds.lock().unwrap() {
            return bounds;
        }
        pass::PassBounds::default()
    }

    /// Whether no pass holds the permit.
    pub fn is_idle(&self) -> bool {
        self.slot.available_permits() == 1
    }

    /// Every name of the vocabulary with its value.
    pub fn report(&self) -> Value {
        Value::Object(
            Count::ALL
                .into_iter()
                .map(|count| {
                    let value = self.counters[count as usize].load(Ordering::Relaxed);
                    (count.name().to_owned(), Value::from(value))
                })
                .collect(),
        )
    }
}
