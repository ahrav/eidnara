//! The Curator's operator surface and expiry maintenance: one task per data home samples content-free ledger facts from the Memory Store into the `metrics.curator` block and runs the bounded, kind-specific cleanup between samples. The Memory Store closes jobs and frozen selections past their queue deadlines with recorded terminal outcomes. The Kernel retires expired Curator-only captures, then abandons expired staging runs and review inputs, releases capture pins, and reclaims unreferenced artifacts through its own staging maintenance, called at most four times per slice so a backlog drains a bounded batch at a time. Neither sweep touches a live reservation, an accepted dependency, or an independent original: the Memory Store spares receipts in progress, and the Kernel abandons only runs past their deadline and deletes only terminal rows past retention.

use std::sync::Arc;
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use memory_store::{CuratorStatusFacts, MemoryStore};
use serde::Serialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::kernel_routes::{KernelOpenCoordinator, KernelState};

/// Idle interval between passes when the last slice advanced nothing and the sample landed.
pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(30);
/// Interval after a pass that advanced cleanup or failed to sample, so a backlog drains and a failure is retried without waiting out the idle interval.
pub const SAMPLE_RETRY_INTERVAL: Duration = Duration::from_secs(5);
/// A `ready` block older than this reads as unavailable, on the monotonic clock.
pub const SAMPLE_STALE_AFTER: Duration = Duration::from_secs(300);
/// Staging maintenance passes per slice; each deletes at most one batch of aged runs.
pub const MAINTENANCE_PASSES_PER_SLICE: usize = 4;

/// Whether the deployment owner's activation record admits Curator disclosure, from a closed set: `open`, or the closed reason's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationState {
    Open,
    Closed(&'static str),
}

impl Serialize for ActivationStateText {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0.as_str())
    }
}

/// The wire spelling of an activation state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivationStateText(pub ActivationState);

impl ActivationState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed(reason) => reason,
        }
    }
}

impl From<&super::activation::Closed> for ActivationState {
    fn from(closed: &super::activation::Closed) -> Self {
        use super::activation::Closed;
        Self::Closed(match closed {
            Closed::Missing => "missing",
            Closed::Refused(_) => "refused",
            Closed::Unreadable(_) => "unreadable",
            Closed::Malformed => "malformed",
            Closed::IdentityMismatch(_) => "identity_mismatch",
            Closed::Unacknowledged => "unacknowledged",
            Closed::UnknownCredential => "unknown_credential",
        })
    }
}

/// The `curator` block under the context component's metrics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CuratorHealthBlock {
    pub curator_state: CuratorState,
    /// The activation gate as the worker last evaluated it; `unknown` until the worker's first pass, `stale` once that evaluation has outlived [`SAMPLE_STALE_AFTER`].
    pub activation_state: ActivationStateText,
    /// Milliseconds since the epoch when the facts were last sampled; `None` until the first sample lands.
    pub sampled_at_ms: Option<i64>,
    /// Jobs and selections the last sweep closed as expired.
    pub swept_jobs: u64,
    pub swept_selections: u64,
    #[serde(flatten)]
    pub facts: Option<CuratorStatusFacts>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CuratorState {
    Starting,
    Ready,
    Unavailable,
}

impl CuratorHealthBlock {
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).expect("curator health block serializes")
    }

    fn starting() -> Self {
        Self {
            curator_state: CuratorState::Starting,
            activation_state: ActivationStateText(ActivationState::Closed("unknown")),
            sampled_at_ms: None,
            swept_jobs: 0,
            swept_selections: 0,
            facts: None,
        }
    }

    /// Cleanup or sampling failed: the block reads as unavailable and keeps the last successful sample time, so the age of the last good sample stays visible.
    fn unavailable(sampled_at_ms: Option<i64>) -> Self {
        Self {
            curator_state: CuratorState::Unavailable,
            activation_state: ActivationStateText(ActivationState::Closed("unknown")),
            sampled_at_ms,
            swept_jobs: 0,
            swept_selections: 0,
            facts: None,
        }
    }
}

struct Published {
    block: CuratorHealthBlock,
    stale_at: Instant,
}

/// The sampler's published projection; `health()` reads it and never touches the store. The activation state is the worker's, published beside the sampler's block.
pub struct CuratorStatus {
    snapshot: ArcSwap<Published>,
    /// The worker's latest gate evaluation and when it stops being current: a worker that stopped evaluating must not keep reporting `open`.
    activation: ArcSwap<(ActivationState, Instant)>,
}

impl Default for CuratorStatus {
    fn default() -> Self {
        Self {
            snapshot: ArcSwap::from_pointee(Published {
                block: CuratorHealthBlock::starting(),
                stale_at: Instant::now() + SAMPLE_STALE_AFTER,
            }),
            activation: ArcSwap::from_pointee((
                ActivationState::Closed("unknown"),
                Instant::now() + SAMPLE_STALE_AFTER,
            )),
        }
    }
}

impl CuratorStatus {
    /// The block a reader reports: the published block, or its unavailable projection once a `ready` block has outlived [`SAMPLE_STALE_AFTER`], carrying the worker's latest activation state either way. An activation older than [`SAMPLE_STALE_AFTER`] reports `stale`: the worker stopped evaluating the gate, so nothing vouches for `open`.
    pub fn reported(&self) -> CuratorHealthBlock {
        let now = Instant::now();
        let published = self.snapshot.load();
        let mut block =
            if published.block.curator_state == CuratorState::Ready && now >= published.stale_at {
                CuratorHealthBlock::unavailable(published.block.sampled_at_ms)
            } else {
                published.block.clone()
            };
        let (activation, stale_at) = **self.activation.load();
        block.activation_state = ActivationStateText(if now >= stale_at {
            ActivationState::Closed("stale")
        } else {
            activation
        });
        block
    }

    pub fn set_activation(&self, state: ActivationState) {
        self.activation
            .store(Arc::new((state, Instant::now() + SAMPLE_STALE_AFTER)));
    }

    fn last_sampled_at_ms(&self) -> Option<i64> {
        self.snapshot.load().block.sampled_at_ms
    }

    fn publish(&self, block: CuratorHealthBlock, sampled: Instant) {
        self.snapshot.store(Arc::new(Published {
            block,
            stale_at: sampled + SAMPLE_STALE_AFTER,
        }));
    }

    /// Moves the current block's stale deadline to now.
    #[cfg(any(test, feature = "test-support"))]
    pub fn expire_for_test(&self) {
        self.snapshot.rcu(|current| Published {
            block: current.block.clone(),
            stale_at: Instant::now(),
        });
    }
}

/// What one pass did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pass {
    pub block: CuratorHealthBlock,
    /// Whether cleanup retired or reclaimed anything, so the next pass follows without the idle interval.
    pub advanced: bool,
    /// Whether every step succeeded; a failed step publishes `unavailable` and the next pass follows at the retry interval.
    pub healthy: bool,
}

/// One Kernel maintenance slice: expired Curator-only captures are retired first, then staging maintenance runs until it deletes less than a full batch or the pass cap is reached. Returns whether anything advanced.
fn kernel_slice(kernel: &kernel::KernelStore, now_ms: i64) -> Result<bool, kernel::KernelError> {
    let mut advanced = kernel.expire_local_file_captures(now_ms)? > 0;
    for _ in 0..MAINTENANCE_PASSES_PER_SLICE {
        let result = kernel.run_staging_maintenance(now_ms)?;
        let progressed = result.abandoned_runs > 0
            || result.deleted_runs > 0
            || result.artifact_gc.reclaimed_objects > 0;
        advanced |= progressed;
        if !progressed {
            break;
        }
    }
    Ok(advanced)
}

/// One cleanup-and-sample pass: the Memory Store closes expired jobs and selections, the Kernel runs one maintenance slice when it is ready, and the facts are sampled. `last_sampled_at_ms` is kept on the unavailable block when a step fails.
pub fn sweep_and_sample(
    store: &MemoryStore,
    kernel: Option<&kernel::KernelStore>,
    now_ms: i64,
    last_sampled_at_ms: Option<i64>,
) -> Pass {
    let unavailable = |advanced: bool| Pass {
        block: CuratorHealthBlock::unavailable(last_sampled_at_ms),
        advanced,
        healthy: false,
    };
    let (swept_jobs, swept_selections) = match store.expire_curator_work(now_ms) {
        Ok((jobs, selections)) => (jobs as u64, selections as u64),
        Err(error) => {
            eprintln!("daemon: curator expiry sweep failed: {error}");
            return unavailable(false);
        }
    };
    let mut advanced = swept_jobs + swept_selections > 0;
    if let Some(kernel) = kernel {
        match kernel_slice(kernel, now_ms) {
            Ok(slice_advanced) => advanced |= slice_advanced,
            Err(error) => {
                eprintln!("daemon: kernel staging maintenance failed: {error:?}");
                return unavailable(advanced);
            }
        }
    }
    match store.curator_status_facts() {
        Ok(facts) => Pass {
            block: CuratorHealthBlock {
                curator_state: CuratorState::Ready,
                activation_state: ActivationStateText(ActivationState::Closed("unknown")),
                sampled_at_ms: Some(now_ms),
                swept_jobs,
                swept_selections,
                facts: Some(facts),
            },
            advanced,
            healthy: true,
        },
        Err(error) => {
            eprintln!("daemon: curator facts sample failed: {error}");
            unavailable(advanced)
        }
    }
}

/// Runs passes until cancelled. The Kernel joins a pass only while it is ready. The blocking work runs off the runtime thread and is joined before the task ends, so shutdown observes its physical completion.
pub(crate) async fn run(
    status: Arc<CuratorStatus>,
    store: Arc<MemoryStore>,
    kernel: Arc<KernelOpenCoordinator>,
    cancel: CancellationToken,
) {
    loop {
        let kernel_store = (kernel.state() == KernelState::Ready)
            .then(|| kernel.kernel_store().ok())
            .flatten();
        let started = Instant::now();
        let now_ms = crate::now_ms();
        let last_sampled_at_ms = status.last_sampled_at_ms();
        let store = Arc::clone(&store);
        let mut worker = tokio::task::spawn_blocking(move || {
            sweep_and_sample(&store, kernel_store.as_deref(), now_ms, last_sampled_at_ms)
        });
        let pass = tokio::select! {
            _ = cancel.cancelled() => {
                // The pass finishes on its own thread before the task ends, so shutdown observes its physical completion.
                let _ = (&mut worker).await;
                return;
            }
            joined = &mut worker => match joined {
                Ok(pass) => pass,
                Err(error) => {
                    eprintln!("daemon: curator sampler worker failed: {error}");
                    Pass {
                        block: CuratorHealthBlock::unavailable(last_sampled_at_ms),
                        advanced: false,
                        healthy: false,
                    }
                }
            },
        };
        status.publish(pass.block, started);
        let interval = if pass.healthy && !pass.advanced {
            SAMPLE_INTERVAL
        } else {
            SAMPLE_RETRY_INTERVAL
        };
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(interval) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use memory_store::curator_jobs::{
        CURATOR_QUEUE_LIFETIME_MS, CausalInputs, CuratorJobOutcome, CuratorJobState,
        ProducerBinding, ReviewTarget,
    };

    use super::*;

    fn producer(index: u64) -> ProducerBinding {
        ProducerBinding {
            producer: "history_summarizer".to_string(),
            firing_id: format!("ses#{index}"),
            ordinal: index,
        }
    }

    fn inputs(index: u64) -> CausalInputs {
        CausalInputs {
            target: ReviewTarget::Memory {
                object_id: format!("memory-{index}"),
                source_revision: 1,
            },
            question_template: "extracted_facts".to_string(),
            signals: Vec::new(),
            required_evidence: Vec::new(),
            policy_versions: BTreeMap::new(),
        }
    }

    /// The sweep closes reserved work past its deadline with a recorded outcome, leaves live work alone, and the sample reports every count content-free; a staged review input past its deadline is abandoned by the Kernel's own maintenance in the same pass.
    #[test]
    fn the_sweep_closes_expired_work_and_the_sample_counts_the_rest() {
        let store_dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(&MemoryStore::test_descriptor(
            store_dir.path(),
            "eidnara-curator-lifecycle-test",
        ))
        .unwrap();
        let kernel_dir = tempfile::tempdir().unwrap();
        let kernel_store = kernel::KernelStore::open(kernel_dir.path()).unwrap();
        let t0 = crate::now_ms();
        // One reservation far enough back to have expired, one live, one activated to Ready.
        store
            .reserve_curator_job(
                "git:p",
                &producer(1),
                &inputs(1),
                t0 - CURATOR_QUEUE_LIFETIME_MS - 1,
            )
            .unwrap();
        let live = match store
            .reserve_curator_job("git:p", &producer(2), &inputs(2), t0)
            .unwrap()
        {
            memory_store::curator_jobs::ReserveOutcome::Reserved(job) => job,
            other => panic!("{other:?}"),
        };
        let ready = match store
            .reserve_curator_job("git:p", &producer(3), &inputs(3), t0)
            .unwrap()
        {
            memory_store::curator_jobs::ReserveOutcome::Reserved(job) => job,
            other => panic!("{other:?}"),
        };
        store
            .activate_curator_job(
                "git:p",
                &ready.causal_identity,
                &producer(3),
                &memory_store::curator_jobs::CuratorJobInput {
                    subject: ready.target.clone(),
                    starting_references: Vec::new(),
                    question_template: "extracted_facts".to_string(),
                },
                t0,
            )
            .unwrap();
        // A review input whose queue deadline the sweep clock will have passed.
        let soon = t0 + 1_000;
        let binding = crate::curator::handoff::review_binding(
            &"a".repeat(64),
            "memory",
            "ses",
            1,
            &live.causal_identity,
        );
        let payload = kernel::ReviewPayload::Subject(kernel::ReviewSubject {
            facts: vec![kernel::ExtractedFact {
                text: "f".to_string(),
                spans: vec![kernel::SourceSpan {
                    alias: "s1".to_string(),
                    start: 0,
                    end: 1,
                }],
            }],
            origins: vec![kernel::SubjectOrigin {
                alias: "s1".to_string(),
                message_id: "m1".to_string(),
                ordinal: 1,
                block_ids: vec!["m1#0".to_string()],
                block_hashes: vec!["0".repeat(64)],
                ranges: vec![kernel::ByteRange { start: 0, end: 1 }],
            }],
        });
        let staged = kernel_store
            .stage_review_input(kernel::ReviewStagingSpec {
                extraction_run_id: "run-expired".to_string(),
                candidate_id: "subject-expired".to_string(),
                producer: "history_summarizer".to_string(),
                binding: binding.clone(),
                payload,
                recorded_at: t0,
                queue_deadline_at: soon,
            })
            .unwrap();
        assert!(matches!(
            kernel_store.read_review_input(&staged, &binding, t0),
            Err(kernel::ReviewReadError::Refused(
                kernel::ReviewReadRefusal::Unsealed
            ))
        ));

        let sweep_at = t0 + 2_000;
        let pass = sweep_and_sample(&store, Some(&kernel_store), sweep_at, None);
        assert!(pass.healthy);
        assert!(
            pass.advanced,
            "the expired job and the abandoned run advanced cleanup"
        );
        let block = pass.block;
        assert_eq!(block.curator_state, CuratorState::Ready);
        assert_eq!(block.sampled_at_ms, Some(sweep_at));
        assert_eq!(block.swept_jobs, 1);
        assert_eq!(block.swept_selections, 0);
        let facts = block.facts.expect("facts sampled");
        assert_eq!(facts.jobs_reserved, 1);
        assert_eq!(facts.jobs_ready, 1);
        assert_eq!(facts.jobs_expired, 1);
        assert_eq!(
            facts.jobs_expired_unseen, 1,
            "nothing ever claimed the expired job"
        );
        assert_eq!(
            facts.jobs_nonadmitted + facts.jobs_failed + facts.jobs_completed,
            0
        );
        assert_eq!(facts.attempts_attempted, 0);
        assert_eq!(facts.receipts_in_progress, 0);
        assert_eq!(
            facts.receipt_charge_bytes,
            3 * memory_store::curator_jobs::CURATOR_RECEIPT_CHARGE_BYTES
        );
        assert_eq!(
            facts.allowance_bytes,
            2 * memory_store::curator_jobs::CURATOR_JOB_ALLOWANCE_BYTES,
            "the expired job released its allowance"
        );
        assert_eq!(
            facts.metadata_bytes,
            facts.receipt_charge_bytes + facts.allowance_bytes
        );
        assert_eq!(
            facts.metadata_headroom_bytes,
            facts.metadata_quota_bytes - facts.metadata_bytes
        );
        assert_eq!(facts.nonadmissions, 0);
        assert_eq!(facts.sessions_with_reservation, 0);
        assert_eq!(
            store
                .lookup_curator_job("git:p", &live.causal_identity)
                .unwrap()
                .unwrap()
                .state,
            CuratorJobState::Reserved,
            "a live reservation is not touched"
        );
        // The Kernel abandoned the expired staged input.
        assert!(matches!(
            kernel_store.read_review_input(&staged, &binding, sweep_at),
            Err(kernel::ReviewReadError::Refused(
                kernel::ReviewReadRefusal::Abandoned
            ))
        ));
        // A second pass sweeps nothing more, counts the same, and advances nothing, so the loop returns to its idle interval.
        let again = sweep_and_sample(&store, Some(&kernel_store), sweep_at + 1, Some(sweep_at));
        assert!(again.healthy);
        assert!(!again.advanced);
        assert_eq!(again.block.swept_jobs, 0);
        assert_eq!(again.block.facts.unwrap().jobs_expired, 1);
        assert_eq!(
            store
                .lookup_curator_job(
                    "git:p",
                    &store
                        .reserve_curator_job("git:p", &producer(1), &inputs(1), t0)
                        .map(|outcome| match outcome {
                            memory_store::curator_jobs::ReserveOutcome::Existing(job) =>
                                job.causal_identity,
                            other => panic!("{other:?}"),
                        })
                        .unwrap()
                )
                .unwrap()
                .unwrap()
                .state,
            CuratorJobState::Terminal(CuratorJobOutcome::Expired),
            "the expired reservation keeps its recorded outcome and is not reopened"
        );
    }

    /// A `ready` block outlives its staleness bound as `unavailable`, with the sample time kept; a fresh publication reports as published.
    #[test]
    fn a_stale_ready_block_reads_as_unavailable() {
        let status = CuratorStatus::default();
        assert_eq!(status.reported().curator_state, CuratorState::Starting);
        status.publish(
            CuratorHealthBlock {
                curator_state: CuratorState::Ready,
                activation_state: ActivationStateText(ActivationState::Open),
                sampled_at_ms: Some(7),
                swept_jobs: 0,
                swept_selections: 0,
                facts: Some(CuratorStatusFacts::default()),
            },
            Instant::now(),
        );
        assert_eq!(status.reported().curator_state, CuratorState::Ready);
        status.expire_for_test();
        let stale = status.reported();
        assert_eq!(stale.curator_state, CuratorState::Unavailable);
        assert_eq!(stale.sampled_at_ms, Some(7));
        assert!(stale.facts.is_none());
        // The JSON shape flattens the facts beside the state so the wire sanitizer reads one flat block.
        let json = CuratorHealthBlock {
            curator_state: CuratorState::Ready,
            activation_state: ActivationStateText(ActivationState::Open),
            sampled_at_ms: Some(1),
            swept_jobs: 2,
            swept_selections: 0,
            facts: Some(CuratorStatusFacts {
                jobs_ready: 3,
                ..CuratorStatusFacts::default()
            }),
        }
        .to_json();
        assert_eq!(json["curator_state"], "ready");
        assert_eq!(json["jobs_ready"], 3);
        assert_eq!(json["swept_jobs"], 2);
    }

    /// The serialized block carries exactly the counters the wire contract lists, so a field added on one side cannot drift silently past the host sanitizer.
    #[test]
    fn the_block_carries_exactly_the_documented_counters() {
        const DOCUMENTED: [&str; 39] = [
            "curator_state",
            "activation_state",
            "sampled_at_ms",
            "swept_jobs",
            "swept_selections",
            "jobs_reserved",
            "jobs_ready",
            "jobs_expired",
            "jobs_expired_unseen",
            "jobs_nonadmitted",
            "jobs_failed",
            "jobs_unknown",
            "jobs_completed",
            "jobs_abstained",
            "selections_frozen",
            "selections_enqueued",
            "selections_expired",
            "selections_failed_slot",
            "attempts_attempted",
            "attempts_acknowledged",
            "attempts_failed",
            "attempts_cancelled",
            "attempts_unknown",
            "attempts_not_dispatched",
            "attempts_open",
            "receipts_in_progress",
            "receipts_complete",
            "receipt_charge_bytes",
            "allowance_bytes",
            "metadata_bytes",
            "metadata_quota_bytes",
            "metadata_headroom_bytes",
            "nonadmissions",
            "sessions_with_reservation",
            "latest_nonadmission_curator_unavailable",
            "latest_nonadmission_capacity_full",
            "latest_nonadmission_evidence_unavailable",
            "latest_nonadmission_fact_set_rejected",
            "latest_nonadmission_subject_refused",
        ];
        let json = CuratorHealthBlock {
            curator_state: CuratorState::Ready,
            activation_state: ActivationStateText(ActivationState::Open),
            sampled_at_ms: Some(1),
            swept_jobs: 0,
            swept_selections: 0,
            facts: Some(CuratorStatusFacts::default()),
        }
        .to_json();
        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        let mut documented = DOCUMENTED.to_vec();
        documented.sort_unstable();
        assert_eq!(keys, documented);
        let wire = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/host-wire-protocol.md"
        ))
        .unwrap();
        for name in DOCUMENTED {
            assert!(
                wire.contains(&format!("`{name}`")),
                "{name} is not in the wire document"
            );
        }
    }

    /// A failed step publishes `unavailable` with the last good sample time, never a `ready` block with zeroed sweep counts.
    #[test]
    fn a_failed_step_reads_as_unavailable_and_keeps_the_last_sample_time() {
        let store_dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(&MemoryStore::test_descriptor(
            store_dir.path(),
            "eidnara-curator-lifecycle-failure-test",
        ))
        .unwrap();
        let kernel_dir = tempfile::tempdir().unwrap();
        let kernel_store = kernel::KernelStore::open(kernel_dir.path()).unwrap();
        // A negative clock is refused by the Kernel's maintenance and by the store's expiry alike.
        let pass = sweep_and_sample(&store, Some(&kernel_store), -1, Some(41));
        assert!(!pass.healthy);
        assert_eq!(pass.block.curator_state, CuratorState::Unavailable);
        assert_eq!(pass.block.sampled_at_ms, Some(41));
        assert!(pass.block.facts.is_none());
    }
}
