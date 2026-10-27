//! Write-time contradiction candidates: derived rows that name a decision a
//! published descriptor overlaps lexically, for a reader to confirm or ignore.
//!
//! Every candidate surface runs only under `contradiction_candidates.enabled`.
//! Its counters carry a closed vocabulary of counts, byte totals, and
//! milliseconds, never descriptor text, ids, digests, or per-row scores.

use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

pub mod eligibility;
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

/// The daemon's candidate counters, reported under `contradiction_candidates`
/// in the status block. `retained_bytes` is the gauge of the rows' stored
/// bytes; every other value only grows.
pub struct Candidates([AtomicU64; Count::ALL.len()]);

impl Default for Candidates {
    fn default() -> Self {
        Self(std::array::from_fn(|_| AtomicU64::new(0)))
    }
}

impl Candidates {
    pub(crate) fn add(&self, count: Count, value: u64) {
        self.0[count as usize].fetch_add(value, Ordering::Relaxed);
    }

    pub(crate) fn set(&self, count: Count, value: u64) {
        self.0[count as usize].store(value, Ordering::Relaxed);
    }

    /// Every name of the vocabulary with its value.
    pub fn report(&self) -> Value {
        Value::Object(
            Count::ALL
                .into_iter()
                .map(|count| {
                    let value = self.0[count as usize].load(Ordering::Relaxed);
                    (count.name().to_owned(), Value::from(value))
                })
                .collect(),
        )
    }
}
