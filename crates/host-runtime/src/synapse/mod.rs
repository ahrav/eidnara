//! Synapse is an optional, certified CPU-only local embedding component behind the `synapse/management_surface` target.
//!
//! Missing configuration, invalid bundles, incompatible ONNX Runtime, and failed certification disable only Synapse.
//! Artifact faults keep Synapse's catalog identity published, make binds reject with `artifact_invalid`, and make internal health report degraded.
//! Panics and invariant violations mark the lane failing; the composite reports that state as host health.
//!
//! Jobs are process-local and ephemeral; route loss cancels only response delivery.
//! Every started native inference call remains owned by the component's incarnation tracker until the component stops.
//! Shutdown drains the incarnation tracker before release.

pub mod bundle;
pub mod embed_tokens;
pub mod inference;
pub mod jobs;
pub mod preflight;
pub mod protocol;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::TryAcquireError;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::composite::{CompositeComponent, SecondaryComponent};
use crate::handler::{
    BindOutcome, HealthReport, HealthStatus, InitError, ManifestSnapshot, RequestCtx,
    RequestOutcome, RouteHandle, RouteIdentity,
};
pub use embed_tokens::EmbedTokens;
use inference::{Backend, InferenceError, OrtIdentity};
use jobs::{AdmitOutcome, JobTable};
pub use jobs::{PollOutcome, ResultLease, ResultPage, failure_is_permanent};
pub use preflight::{
    AdmittedInput, DenseUnavailable, EmbeddingIdentity, EmbeddingInputLimits, InferenceFailureKind,
    LaneUnavailableState,
};
use protocol::{Request, RequestError};

pub const SYNAPSE_MODULE_ID: &str = "synapse";

/// Only trusted startup configuration sets Synapse's finite lane capacities.
/// Requests cannot select `SynapseLimits`; only trusted startup configuration provides them.
#[derive(Debug, Clone)]
pub struct SynapseLimits {
    /// `max_waiting_queries` limits queries waiting behind the one running query.
    /// When `max_waiting_queries` is zero, one query may run and every concurrent query is rejected immediately.
    pub max_waiting_queries: usize,
    pub max_queued_jobs: usize,
    pub max_queued_request_bytes: u64,
    pub max_retained_jobs: usize,
    /// `retained_resident_bytes` includes local-input capacity plus this retained-result cap.
    /// `HostLimits::max_resident_bytes` must fund that sum above the runtime floor.
    pub max_retained_result_bytes: u64,
    pub max_batch_items: usize,
    pub max_batch_text_bytes: usize,
    pub max_text_bytes: usize,
    pub max_page_vectors: usize,
    pub max_page_encoded_bytes: usize,
    pub retention: std::time::Duration,
    pub retry_after_ms: u64,
    pub query_retry_after_ms: u64,
}

impl SynapseLimits {
    /// `per_waiter_charge_bound` bounds resident memory retained by one admitted query while it waits for or uses the CPU lane.
    /// JSON decoding can retain twice the decoded text length as `String` capacity.
    /// The handler retains response scratch until it encodes the terminal response.
    /// A completed query keeps its returned vector (at most `MAX_DIMS` components) while its permit is held, so the bound covers one maximal vector too.
    pub fn per_waiter_charge_bound(&self) -> Option<u64> {
        u64::try_from(self.max_text_bytes)
            .ok()?
            .checked_mul(2)?
            .checked_add(RESPONSE_SCRATCH_BYTES as u64)?
            .checked_add(bundle::MAX_DIMS.checked_mul(std::mem::size_of::<f32>() as u64)?)
    }

    /// `query_admission_permits` returns permits for one running query plus every allowed waiter.
    /// `query_admission_permits` is the single derivation of the permit rule.
    pub(crate) fn query_admission_permits(&self) -> Option<usize> {
        self.max_waiting_queries
            .checked_add(1)
            .filter(|permits| *permits <= tokio::sync::Semaphore::MAX_PERMITS)
    }

    /// A job holds at most `max_batch_items` items, so no page can hold more.
    /// The pager places at least one item in every page.
    /// `page_item_bound` is shared by runtime page reservation and startup validation.
    /// Sharing `page_item_bound` keeps startup validation aligned with runtime page reservation.
    pub(crate) fn page_item_bound(&self) -> usize {
        self.max_page_vectors
            .max(1)
            .min(self.max_batch_items.max(1))
    }
}

impl Default for SynapseLimits {
    fn default() -> Self {
        let max_batch_items = 64;
        Self {
            max_waiting_queries: 0,
            max_queued_jobs: 64,
            max_queued_request_bytes: 64 * 1024 * 1024,
            max_retained_jobs: 64,
            max_retained_result_bytes: 64 * 1024 * 1024,
            max_batch_items,
            max_batch_text_bytes: 8 * 1024 * 1024,
            max_text_bytes: 1024 * 1024,
            max_page_vectors: 16,
            max_page_encoded_bytes: 2 * 1024 * 1024,
            retention: std::time::Duration::from_secs(15 * 60),
            retry_after_ms: 50,
            query_retry_after_ms: 50,
        }
    }
}

/// A component-level failure omits Synapse from the deployment.
#[derive(Debug, Clone)]
pub struct SynapseConfig {
    pub bundle_dir: PathBuf,
    /// The configured digest covers `bundle_dir/manifest.json`.
    /// The daemon supplies the selected generation's digest.
    /// The selected generation's digest binds every bundle artifact to the generation where it was staged.
    /// Hermetic fixtures without a generation root supply `None`.
    pub bundle_manifest_sha256: Option<String>,
    pub ort_library: PathBuf,
    pub ort_library_sha256: String,
    pub limits: SynapseLimits,
}

/// The verified manifest pins the catalog-facing lane identity.
#[derive(Debug, Clone)]
pub struct LaneInfo {
    pub model: String,
    pub fingerprint: String,
    pub table_epoch: u64,
    pub dims: usize,
    pub execution_provider: &'static str,
    /// Inference truncates tokens at `max_tokens`.
    /// Clients must chunk at `max_tokens` rather than a hardcoded limit.
    pub max_tokens: u32,
    /// `max_text_bytes` limits the UTF-8 bytes in one query or batch item.
    /// Clients must enforce `max_tokens` and `max_text_bytes` because token count has no fixed UTF-8 byte ratio.
    pub max_text_bytes: usize,
    pub provenance: serde_json::Value,
    pub recommended_rows: u32,
    pub recommended_token_budget: u32,
}

impl LaneInfo {
    fn from_bundle(bundle: &bundle::VerifiedBundle) -> Self {
        let manifest = &bundle.manifest;
        Self {
            model: manifest.model.clone(),
            fingerprint: manifest.fingerprint.clone(),
            table_epoch: manifest.table_epoch,
            dims: manifest.dims as usize,
            execution_provider: "cpu",
            // The manifest schema limits `max_tokens` to 1_048_576.
            // Casting `manifest.max_tokens` to `u32` is lossless.
            max_tokens: manifest.max_tokens as u32,
            max_text_bytes: bundle.max_text_bytes,
            provenance: manifest.provenance.clone(),
            recommended_rows: manifest.recommended_batch.rows,
            recommended_token_budget: manifest.recommended_batch.token_budget,
        }
    }
}

/// Tests can substitute an `EmbeddingEngine` implementation.
pub trait EmbeddingEngine: Send + Sync + 'static {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, InferenceError>;

    /// The full token count of `text` under the engine's own tokenizer, with nothing truncated and no padding.
    fn untruncated_token_len(&self, text: &str) -> Result<EmbedTokens, InferenceError>;
}

impl EmbeddingEngine for Backend {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, InferenceError> {
        Backend::embed(self, texts)
    }

    fn untruncated_token_len(&self, text: &str) -> Result<EmbedTokens, InferenceError> {
        Backend::untruncated_token_len(self, text)
    }
}

struct ReadyLane {
    backend: Arc<dyn EmbeddingEngine>,
    lane: LaneInfo,
    /// Caching one `models.list` body per lane keeps its serialization off the request path, where no reservation covers it.
    models_list: Vec<u8>,
}

impl ReadyLane {
    fn new(backend: Arc<dyn EmbeddingEngine>, lane: LaneInfo) -> Self {
        let models_list = protocol::models_list_body(&lane);
        Self {
            backend,
            lane,
            models_list,
        }
    }
}

#[derive(Debug, Clone)]
pub enum SynapseStatus {
    Ready(LaneInfo),
    Starting,
    Disabled { reason: String },
    Failing { reason: String },
}

enum LaneState {
    Starting,
    Disabled {
        reason: String,
    },
    Ready(Arc<ReadyLane>),
    Failing {
        reason: String,
    },
    /// Late count failures may escalate runtime disablement, but cannot replace completed shutdown.
    ShutDown,
}

struct SynapseInner {
    config: Option<SynapseConfig>,
    unsupported_reason: Option<&'static str>,
    limits: SynapseLimits,
    state: Mutex<LaneState>,
    jobs: JobTable,
    /// In-process submissions have no request scratch charge, so they reserve their retained inputs here before allocation.
    local_inputs: crate::wire::ByteBudget,
    /// `cpu` has one permit, so at most one native inference call runs at a time.
    /// The semaphore serves waiters in registration order.
    /// Semaphore registration order prevents starvation among queued waiters.
    /// Semaphore registration order does not guarantee host admission order.
    /// `cpu` queue order need not match host admission order because each query registers from a separate task.
    cpu: Arc<tokio::sync::Semaphore>,
    /// One running query plus at most `max_waiting_queries` waiters may use the serialized CPU lane.
    /// Admission is a non-blocking count: it decides whether a query may wait, not where it enters the queue.
    /// Batch work is bounded separately by the job table.
    query_admission: Arc<tokio::sync::Semaphore>,
    /// The component owns every started native call through shutdown.
    tracker: TaskTracker,
    /// Shutdown cancels queued work and closes admission.
    closing: CancellationToken,
}

impl SynapseInner {
    /// A poisoned lock still yields the lane state, so a panicking holder cannot make every later caller panic.
    fn lock_state(&self) -> MutexGuard<'_, LaneState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

pub struct SynapseComponent {
    inner: Arc<SynapseInner>,
}

impl SynapseComponent {
    pub fn new(config: Option<SynapseConfig>) -> Self {
        let limits = config
            .as_ref()
            .map(|config| config.limits.clone())
            .unwrap_or_default();
        // Invalid configured limits make `initialize` return its typed error before bundle work begins.
        // Construction must not panic so `initialize` can return the typed limit-validation error.
        let query_admission_permits = limits.query_admission_permits().unwrap_or(1);
        Self {
            inner: Arc::new(SynapseInner {
                config,
                unsupported_reason: None,
                jobs: JobTable::new(limits.clone()),
                local_inputs: crate::wire::ByteBudget::new(constructor_local_input_capacity(
                    &limits,
                )),
                limits,
                state: Mutex::new(LaneState::Disabled {
                    reason: "not initialized".to_owned(),
                }),
                cpu: Arc::new(tokio::sync::Semaphore::new(1)),
                query_admission: Arc::new(tokio::sync::Semaphore::new(query_admission_permits)),
                tracker: TaskTracker::new(),
                closing: CancellationToken::new(),
            }),
        }
    }

    pub fn unsupported(reason: &'static str) -> Self {
        let limits = SynapseLimits::default();
        let query_admission_permits = limits.query_admission_permits().unwrap_or(1);
        Self {
            inner: Arc::new(SynapseInner {
                config: None,
                unsupported_reason: Some(reason),
                jobs: JobTable::new(limits.clone()),
                local_inputs: crate::wire::ByteBudget::new(constructor_local_input_capacity(
                    &limits,
                )),
                limits,
                state: Mutex::new(LaneState::Disabled {
                    reason: reason.to_owned(),
                }),
                cpu: Arc::new(tokio::sync::Semaphore::new(1)),
                query_admission: Arc::new(tokio::sync::Semaphore::new(query_admission_permits)),
                tracker: TaskTracker::new(),
                closing: CancellationToken::new(),
            }),
        }
    }

    /// The test constructor creates a component with an immediately ready lane.
    /// The test constructor uses the supplied engine without bundle loading or ORT.
    ///
    /// # Errors
    ///
    /// `ready_with_engine` returns `bundle::BundleError` when lane or serving-limit validation fails.
    /// It enforces the startup bounds used for loaded bundles.
    pub fn ready_with_engine(
        mut lane: LaneInfo,
        engine: Arc<dyn EmbeddingEngine>,
        limits: SynapseLimits,
    ) -> Result<Self, bundle::BundleError> {
        bundle::validate_lane_identity(&lane)?;
        bundle::validate_serving_limits(lane.dims, lane.recommended_rows as usize, &limits)?;
        // `validate_serving_limits` rejects permit-count overflow.
        // Validated limits always have a permit count.
        let query_admission_permits = limits
            .query_admission_permits()
            .expect("validate_serving_limits proves the permit count");
        let local_input_bytes = checked_local_input_capacity(&limits)
            .expect("validate_serving_limits proves the local input capacity");
        lane.max_text_bytes = limits.max_text_bytes;
        Ok(Self {
            inner: Arc::new(SynapseInner {
                config: None,
                unsupported_reason: None,
                jobs: JobTable::new(limits.clone()),
                local_inputs: crate::wire::ByteBudget::new(local_input_bytes),
                limits,
                state: Mutex::new(LaneState::Ready(Arc::new(ReadyLane::new(engine, lane)))),
                cpu: Arc::new(tokio::sync::Semaphore::new(1)),
                query_admission: Arc::new(tokio::sync::Semaphore::new(query_admission_permits)),
                tracker: TaskTracker::new(),
                closing: CancellationToken::new(),
            }),
        })
    }

    /// # Errors
    ///
    /// Returns `bundle::BundleError` when the replacement identity or serving limits are invalid.
    #[cfg(any(test, feature = "test-support"))]
    pub fn replace_ready_with_engine_for_test(
        &self,
        mut lane: LaneInfo,
        engine: Arc<dyn EmbeddingEngine>,
    ) -> Result<(), bundle::BundleError> {
        bundle::validate_lane_identity(&lane)?;
        bundle::validate_serving_limits(
            lane.dims,
            lane.recommended_rows as usize,
            &self.inner.limits,
        )?;
        lane.max_text_bytes = self.inner.limits.max_text_bytes;
        *self.inner.lock_state() = LaneState::Ready(Arc::new(ReadyLane::new(engine, lane)));
        Ok(())
    }

    pub fn status(&self) -> SynapseStatus {
        match &*self.inner.lock_state() {
            LaneState::Ready(lane) => SynapseStatus::Ready(lane.lane.clone()),
            LaneState::Starting => SynapseStatus::Starting,
            LaneState::Disabled { reason } => SynapseStatus::Disabled {
                reason: reason.clone(),
            },
            LaneState::Failing { reason } => SynapseStatus::Failing {
                reason: reason.clone(),
            },
            LaneState::ShutDown => SynapseStatus::Disabled {
                reason: SHUT_DOWN_REASON.to_owned(),
            },
        }
    }

    fn ready_lane(&self) -> Option<Arc<ReadyLane>> {
        match &*self.inner.lock_state() {
            LaneState::Ready(lane) => Some(Arc::clone(lane)),
            _ => None,
        }
    }

    /// Judges one text for embedding under the ready lane: bytes first, then the exact untruncated count from the lane's own tokenizer, then the window.
    /// The count runs outside the `cpu` permit, so a refused text never contends with inference.
    ///
    /// # Errors
    ///
    /// Returns [`DenseUnavailable`] naming the first check that failed; the text is never part of the reason.
    /// A count failure settles the lane by the same classes as inference: `Artifact` disables it and `Invariant` marks it failing, with fixed reasons because a count error message can echo the text.
    /// Counter panics are quarantined as invariant failures. Invariants escalate runtime disablement, but completed shutdown remains disabled.
    pub fn preflight_embedding<'t>(
        &self,
        limits: EmbeddingInputLimits,
        text: &'t str,
    ) -> Result<AdmittedInput<'t>, DenseUnavailable> {
        let lane = self.ready_or_unavailable()?;
        self.preflight_ready(&lane, limits, text)
    }

    /// Identity checking precedes input validation so a replacement's limits or tokenizer cannot misclassify the expected lane's input.
    /// Count failures settle the lane as in [`Self::preflight_embedding`].
    ///
    /// # Errors
    ///
    /// Returns [`DenseUnavailable::IdentityChanged`] when the ready lane differs from `expected`; otherwise returns the same refusals as [`Self::preflight_embedding`].
    pub fn preflight_embedding_for_lane<'t>(
        &self,
        expected: &LaneInfo,
        text: &'t str,
    ) -> Result<AdmittedInput<'t>, DenseUnavailable> {
        let lane = self.ready_or_unavailable()?;
        if !EmbeddingIdentity::of_lane(expected).matches(&lane.lane) {
            return Err(DenseUnavailable::IdentityChanged);
        }
        self.preflight_ready(&lane, EmbeddingInputLimits::of_lane(expected), text)
    }

    /// Both entry points validate and count against their captured lane, without reading a replacement between identity checking and counting.
    fn preflight_ready<'t>(
        &self,
        lane: &ReadyLane,
        limits: EmbeddingInputLimits,
        text: &'t str,
    ) -> Result<AdmittedInput<'t>, DenseUnavailable> {
        let admitted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            preflight::admit(&lane.lane, &*lane.backend, limits, text)
        }))
        .map_err(|payload| {
            crate::composite::discard_payload(payload);
            mark_failing(&self.inner, "token counting panicked".to_owned());
            DenseUnavailable::CountUnavailable(InferenceFailureKind::Invariant)
        })?;
        if let Err(DenseUnavailable::CountUnavailable(kind)) = &admitted {
            match kind {
                InferenceFailureKind::Artifact => mark_disabled(
                    &self.inner,
                    "token counting declared the artifact unusable".to_owned(),
                ),
                InferenceFailureKind::Invariant => {
                    mark_failing(&self.inner, "token counting failed an invariant".to_owned())
                }
                InferenceFailureKind::Input | InferenceFailureKind::Execution => {}
            }
        }
        admitted
    }

    /// Embeds one admitted text, byte for byte as admitted, under the lane it was admitted for.
    ///
    /// # Errors
    ///
    /// Returns [`DenseUnavailable::IdentityChanged`] when the serving lane is no longer the admitting one, [`DenseUnavailable::ByteOverflow`] when the admitted bytes exceed the serving lane's own cap, [`DenseUnavailable::LaneUnavailable`] when no lane serves, [`DenseUnavailable::LaneBusy`] when another call holds the lane's permit, and [`DenseUnavailable::Inference`] when inference refuses or fails.
    pub fn embed_admitted(
        &self,
        admitted: &AdmittedInput<'_>,
    ) -> Result<Vec<f32>, DenseUnavailable> {
        let lane = self.ready_or_unavailable()?;
        if !admitted.identity().matches(&lane.lane) {
            return Err(DenseUnavailable::IdentityChanged);
        }
        // The byte cap is host configuration, not part of the identity, so a lane serving the
        // same bundle under a narrower cap must re-judge the admitted bytes as its own.
        if admitted.bytes() > lane.lane.max_text_bytes {
            return Err(DenseUnavailable::ByteOverflow {
                bytes: admitted.bytes(),
                max_bytes: lane.lane.max_text_bytes,
            });
        }
        let mut vectors = self
            .run_inference(&lane, &[admitted.text()])
            .map_err(|refusal| match refusal {
                EmbedRefusal::Busy => DenseUnavailable::LaneBusy {
                    retry_after_ms: self.inner.limits.query_retry_after_ms,
                },
                EmbedRefusal::Inference(error) => DenseUnavailable::Inference((&error).into()),
            })?;
        // `check_engine_vectors` accepted exactly one vector for the one text.
        Ok(vectors.pop().expect("one vector for one admitted text"))
    }

    /// The job table's incarnation nonce; every job identifier this component issues starts with it, so a persisted job identifier names the host that admitted it.
    pub fn host_incarnation(&self) -> &str {
        self.inner.jobs.incarnation()
    }

    /// Admits one preflighted text into the job table as a single-item batch keyed by `item_id`, under the lane it was admitted for.
    /// The canonical request key binds the item and text to the admitting model, fingerprint, and epoch. A retained job is reused only under that identity.
    /// Admission reserves the dedicated local-input budget instead of wire scratch. Admission requires a Tokio runtime context because it starts an inference worker.
    ///
    /// # Errors
    ///
    /// Returns [`DenseUnavailable::LaneUnavailable`] when no lane serves and [`DenseUnavailable::IdentityChanged`] when the serving lane is no longer the admitting one.
    pub fn submit_admitted(
        &self,
        admitted: &AdmittedInput<'_>,
        item_id: &str,
    ) -> Result<SubmitOutcome, DenseUnavailable> {
        let lane = self.ready_or_unavailable()?;
        if !admitted.identity().matches(&lane.lane) {
            return Err(DenseUnavailable::IdentityChanged);
        }
        let Some(charge_bound) = local_input_charge_bound(item_id, admitted.text()) else {
            return Ok(SubmitOutcome::Refused("unsupported_shape"));
        };
        let content_sha256 = protocol::sha256_hex(admitted.text().as_bytes());
        let key = local_key(&lane.lane, item_id, &content_sha256);
        let payload_digest =
            jobs::payload_digest(&key, std::iter::once((item_id, content_sha256.as_str())));
        if let Some(outcome) = self.inner.jobs.probe_retained(&key, payload_digest) {
            return Ok(match outcome {
                AdmitOutcome::Existing(descriptor) => SubmitOutcome::Queued {
                    job_id: descriptor.job_id,
                },
                AdmitOutcome::Conflict => SubmitOutcome::Refused("idempotency_conflict"),
                AdmitOutcome::Closed => SubmitOutcome::Closing,
                _ => unreachable!("retained probe returns only retained outcomes"),
            });
        }
        let Some(mut charge) = self.inner.local_inputs.try_charge(charge_bound) else {
            return Ok(SubmitOutcome::Full);
        };
        let item = local_item(item_id, admitted.text(), content_sha256);
        let input_bytes = jobs::job_input_bytes(&key, std::slice::from_ref(&item));
        if let Some(outcome) = local_input_charge_mismatch(input_bytes, charge.bytes()) {
            debug_assert!(
                input_bytes <= charge.bytes(),
                "local input charge mismatch: input_bytes={input_bytes}, charge_bytes={}, capacity={}",
                charge.bytes(),
                self.inner.local_inputs.capacity(),
            );
            return Ok(outcome);
        }
        charge.shrink_to(input_bytes);
        let dims = lane.lane.dims;
        Ok(
            match self
                .inner
                .jobs
                .admit_charged(key.clone(), &key, vec![item], dims, &mut charge)
            {
                AdmitOutcome::Existing(descriptor) => SubmitOutcome::Queued {
                    job_id: descriptor.job_id,
                },
                AdmitOutcome::Admitted { job_id, seq } => {
                    self.spawn_batch_worker(lane, seq);
                    SubmitOutcome::Queued { job_id }
                }
                // The submitted key is its own canonical key, so only a retained key with a different payload can mismatch.
                AdmitOutcome::Conflict | AdmitOutcome::KeyMismatch => {
                    SubmitOutcome::Refused("idempotency_conflict")
                }
                AdmitOutcome::Full => SubmitOutcome::Full,
                AdmitOutcome::ResultTooLarge => SubmitOutcome::Refused("unsupported_shape"),
                AdmitOutcome::Closed => SubmitOutcome::Closing,
            },
        )
    }

    /// Whether this component's job table still holds `job_id`: queued, running, retaining a result, or completed with a served result page still alive. A job identifier from another incarnation is never held. The answer does not refresh the job's retention rank.
    pub fn holds_job(&self, job_id: &str) -> bool {
        self.inner.jobs.retains(job_id)
    }

    /// Whether a result page served for `job_id` is still alive. The job's own retained result does not count; only a page a caller still holds does.
    pub fn holds_result_page(&self, job_id: &str) -> bool {
        self.inner.jobs.result_in_use(job_id)
    }

    /// Polls using the frozen admitting lane, item identity, and text, so a replacement lane cannot retrieve another lane's result.
    /// A single-item job returns its whole result in one page; the page's lease keeps the result bytes counted while the caller holds the vector.
    /// A job identifier from another incarnation or an evicted job polls as [`PollOutcome::Restarted`]; a failed lane still answers for the jobs it settled.
    pub fn poll_admitted(
        &self,
        lane: &LaneInfo,
        job_id: &str,
        item_id: &str,
        text: &str,
    ) -> PollOutcome {
        let content_sha256 = protocol::sha256_hex(text.as_bytes());
        let key = local_key(lane, item_id, &content_sha256);
        self.inner.jobs.poll(job_id, &key, None)
    }

    /// One lock acquisition answers both whether a lane serves and, if not, which state refuses.
    fn ready_or_unavailable(&self) -> Result<Arc<ReadyLane>, DenseUnavailable> {
        let state = match &*self.inner.lock_state() {
            LaneState::Ready(lane) => return Ok(Arc::clone(lane)),
            LaneState::Starting => LaneUnavailableState::Starting,
            LaneState::Disabled { reason } if is_unsupported_reason(reason) => {
                LaneUnavailableState::Unsupported
            }
            LaneState::Disabled { .. } | LaneState::ShutDown => LaneUnavailableState::Disabled,
            LaneState::Failing { .. } => LaneUnavailableState::Failing,
        };
        Err(DenseUnavailable::LaneUnavailable { state })
    }

    /// `embed_blocking` shares the lane's single `cpu` permit with routed queries and batch workers, so at most one native call runs at a time.
    /// A lane whose permit is held or closed reports `Artifact` without waiting, because a synchronous caller cannot park on the async semaphore.
    /// `Invariant` errors mark the lane failing before returning, so later callers cannot obtain vectors from a suspect backend.
    /// The lane is read under one lock acquisition so a concurrent `activate` cannot change the state between the readiness check and the reason lookup.
    pub fn embed_blocking(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, InferenceError> {
        // The synchronous path admits only what a routed batch could: the same item count, per-text, and aggregate byte bounds the lane was validated and certified against.
        let limits = &self.inner.limits;
        if texts.is_empty() || texts.len() > limits.max_batch_items {
            return Err(InferenceError::Input(format!(
                "text count {} is outside 1..={}",
                texts.len(),
                limits.max_batch_items
            )));
        }
        let mut total_bytes = 0usize;
        for text in texts {
            if text.is_empty() || text.len() > limits.max_text_bytes {
                return Err(InferenceError::Input(format!(
                    "text length {} is outside 1..={}",
                    text.len(),
                    limits.max_text_bytes
                )));
            }
            total_bytes = total_bytes.saturating_add(text.len());
        }
        if total_bytes > limits.max_batch_text_bytes {
            return Err(InferenceError::Input(format!(
                "aggregate text {total_bytes} bytes exceeds {}",
                limits.max_batch_text_bytes
            )));
        }
        let lane = {
            let state = self.inner.lock_state();
            match &*state {
                LaneState::Ready(lane) => Arc::clone(lane),
                LaneState::Starting => {
                    return Err(InferenceError::Artifact(STARTING_REASON.to_owned()));
                }
                LaneState::Disabled { reason } | LaneState::Failing { reason } => {
                    return Err(InferenceError::Artifact(reason.clone()));
                }
                LaneState::ShutDown => {
                    return Err(InferenceError::Artifact(SHUT_DOWN_REASON.to_owned()));
                }
            }
        };
        self.run_inference(&lane, texts)
            .map_err(|refusal| match refusal {
                EmbedRefusal::Busy => InferenceError::Artifact(BUSY_REASON.to_owned()),
                EmbedRefusal::Inference(error) => error,
            })
    }

    /// Runs one native call for `lane` under the single `cpu` permit, settling failures and panics into the lane state.
    fn run_inference(
        &self,
        lane: &ReadyLane,
        texts: &[&str],
    ) -> Result<Vec<Vec<f32>>, EmbedRefusal> {
        let _permit = self
            .inner
            .cpu
            .try_acquire()
            .map_err(|_| EmbedRefusal::Busy)?;
        // A concurrent holder can mark the lane failing between the state read and this acquisition; the captured backend must not run after that transition.
        if let Some(reason) = lane_failure_reason(&self.inner) {
            return Err(InferenceError::Artifact(reason).into());
        }
        // A panicking backend is quarantined the same way the routed workers quarantine a panicked blocking task; the caller sees the same `Invariant` instead of an unwind that leaves the lane `Ready`.
        let joined =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| lane.backend.embed(texts)))
                .map_err(|_| PanickedBackend);
        settle_inference(&self.inner, lane.lane.dims, texts.len(), joined)
            .map_err(EmbedRefusal::Inference)
    }
}

/// Why one native call did not run to a vector: the permit was held, or inference itself refused or failed.
enum EmbedRefusal {
    Busy,
    Inference(InferenceError),
}

impl From<InferenceError> for EmbedRefusal {
    fn from(error: InferenceError) -> Self {
        Self::Inference(error)
    }
}

/// The reason is retained uncharged for the component lifetime and cloned into status and error paths, so it is bounded to the shared diagnostic cap.
/// A lane disabled at runtime by an artifact fault can still have workers in flight; an invariant failure or panic from one of them must surface as `Failing`, so both `Ready` and `Disabled` transition.
fn mark_failing(inner: &SynapseInner, mut reason: String) {
    protocol::bound_diagnostic(&mut reason);
    let mut state = inner.lock_state();
    if matches!(&*state, LaneState::Ready(_) | LaneState::Disabled { .. }) {
        *state = LaneState::Failing { reason };
    }
}

fn mark_disabled(inner: &SynapseInner, mut reason: String) {
    protocol::bound_diagnostic(&mut reason);
    let mut state = inner.lock_state();
    if matches!(&*state, LaneState::Ready(_)) {
        *state = LaneState::Disabled { reason };
    }
}

/// How the job table answered an in-process single-item admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitOutcome {
    /// The job is queued, running, or already complete; poll it with [`SynapseComponent::poll_admitted`].
    Queued { job_id: String },
    /// Admission or result capacity is exhausted; the same submission may succeed once the table drains.
    Full,
    /// The table refuses the item for good: `idempotency_conflict` when its key is retained with a different payload, `unsupported_shape` when its identity exceeds [`jobs::MAX_ITEM_ID_BYTES`] or its single result exceeds the retained-result byte limit.
    Refused(&'static str),
    /// The component is shutting down and admits nothing.
    Closing,
}

// Wire and in-process requests share a job table; the prefix keeps their idempotency namespaces separate.
fn local_key(lane: &LaneInfo, item_id: &str, content_sha256: &str) -> String {
    let canonical =
        protocol::canonical_request_key_parts(lane, std::iter::once((item_id, content_sha256)));
    protocol::sha256_hex(format!("local\u{1f}{canonical}").as_bytes())
}

fn local_item(item_id: &str, text: &str, content_sha256: String) -> jobs::BatchItem {
    jobs::BatchItem {
        id: exact_string(item_id),
        content_sha256,
        text: exact_string(text),
    }
}

fn exact_string(value: &str) -> String {
    let mut owned = String::with_capacity(value.len());
    owned.push_str(value);
    owned
}

fn local_input_charge_bound(item_id: &str, text: &str) -> Option<usize> {
    Some(jobs::job_input_bytes_for_shape(
        protocol::CANONICAL_REQUEST_KEY_BYTES,
        std::iter::once(jobs::InputShape::exact(item_id.len(), text.len())?),
    ))
}

fn local_input_charge_mismatch(input_bytes: usize, charge_bytes: usize) -> Option<SubmitOutcome> {
    (input_bytes > charge_bytes).then_some(SubmitOutcome::Refused("unsupported_shape"))
}

fn checked_local_input_capacity(limits: &SynapseLimits) -> Option<u64> {
    let max_permits = u64::try_from(tokio::sync::Semaphore::MAX_PERMITS).unwrap_or(u64::MAX);
    bundle::max_queued_input_bytes(limits)
        .and_then(|queued| {
            bundle::max_retained_input_bytes(limits)
                .and_then(|retained| queued.checked_add(retained))
        })
        .filter(|capacity| *capacity <= max_permits)
}

fn constructor_local_input_capacity(limits: &SynapseLimits) -> u64 {
    checked_local_input_capacity(limits).unwrap_or(0)
}

fn declared_local_input_capacity(limits: &SynapseLimits) -> u64 {
    checked_local_input_capacity(limits).unwrap_or(u64::MAX)
}

/// Captured `Arc<ReadyLane>` values can outlive a failing state transition, so callers must not run a captured backend after the transition.
fn lane_failure_reason(inner: &SynapseInner) -> Option<String> {
    match &*inner.lock_state() {
        LaneState::Ready(_) => None,
        LaneState::Starting => Some(STARTING_REASON.to_owned()),
        LaneState::Disabled { reason } | LaneState::Failing { reason } => Some(reason.clone()),
        LaneState::ShutDown => Some(SHUT_DOWN_REASON.to_owned()),
    }
}

/// `STARTING_REASON` provides a fixed reason until activation settles the lane.
const STARTING_REASON: &str = "the synapse lane is still starting";

/// `BUSY_REASON` reports a `cpu` permit that a synchronous caller could not take without waiting.
const BUSY_REASON: &str = "the synapse lane is busy";

const SHUT_DOWN_REASON: &str = "the synapse lane is shut down";

/// The disabled reason the host passes to `SynapseComponent::unsupported` on a platform with no lane.
/// Both the `synapse_state` health metric and `LaneUnavailableState` classify a disabled lane by this reason, so the two projections cannot disagree.
const UNSUPPORTED_REASON: &str = "synapse_unsupported";

fn is_unsupported_reason(reason: &str) -> bool {
    reason == UNSUPPORTED_REASON
}

/// Engines supplied through `ready_with_engine` bypass `Backend`'s own output checks, so the served-vector contract is enforced here for every engine: one row per input text, each with `dims` finite unit-norm components. A violation quarantines the lane like any other invariant failure.
fn check_engine_vectors(
    inner: &SynapseInner,
    dims: usize,
    expected_rows: usize,
    vectors: &[Vec<f32>],
) -> Result<(), String> {
    let violation = if vectors.len() != expected_rows {
        Some(format!(
            "inference returned {} vectors for {expected_rows} texts",
            vectors.len()
        ))
    } else {
        vectors
            .iter()
            .find_map(|vector| inference::validate_unit_vector(dims, vector).err())
    };
    if let Some(reason) = violation {
        mark_failing(inner, reason.clone());
        return Err(reason);
    }
    Ok(())
}

/// A backend panic caught on the synchronous path; it settles like a panicked blocking task.
struct PanickedBackend;

impl From<tokio::task::JoinError> for PanickedBackend {
    fn from(_: tokio::task::JoinError) -> Self {
        Self
    }
}

/// `Invariant` failures and panicked backends mark the lane failing before any sink receives the error, preventing later callers from receiving vectors from a suspect backend.
fn settle_inference(
    inner: &SynapseInner,
    dims: usize,
    expected_rows: usize,
    joined: Result<Result<Vec<Vec<f32>>, InferenceError>, impl Into<PanickedBackend>>,
) -> Result<Vec<Vec<f32>>, InferenceError> {
    match joined {
        Ok(Ok(vectors)) => {
            check_engine_vectors(inner, dims, expected_rows, &vectors)
                .map_err(InferenceError::Invariant)?;
            // The state lock orders validated completion against concurrent count failures.
            match lane_failure_reason(inner) {
                Some(reason) => Err(InferenceError::Artifact(reason)),
                None => Ok(vectors),
            }
        }
        // Reasons are bounded once here so the retained state and the propagated error carry the same capped text; an oversized error would otherwise be downgraded to `internal_error` at the terminal.
        Ok(Err(InferenceError::Invariant(mut reason))) => {
            protocol::bound_diagnostic(&mut reason);
            mark_failing(inner, reason.clone());
            Err(InferenceError::Invariant(reason))
        }
        // A runtime artifact fault means the backend declared its artifact unusable, so the lane is disabled rather than left serving it.
        Ok(Err(InferenceError::Artifact(mut reason))) => {
            protocol::bound_diagnostic(&mut reason);
            mark_disabled(inner, reason.clone());
            Err(InferenceError::Artifact(reason))
        }
        Ok(Err(InferenceError::Input(mut reason))) => {
            protocol::bound_diagnostic(&mut reason);
            Err(InferenceError::Input(reason))
        }
        Ok(Err(InferenceError::Execution(mut reason))) => {
            protocol::bound_diagnostic(&mut reason);
            Err(InferenceError::Execution(reason))
        }
        Err(_panicked) => {
            let reason = "inference task panicked".to_owned();
            mark_failing(inner, reason.clone());
            Err(InferenceError::Invariant(reason))
        }
    }
}

/// A distinct error wrapper prevents an engine from spoofing cancellation or expiry.
enum QueryFault {
    Cancelled,
    /// The deadline passed before the worker held the CPU permit, so no engine call was made.
    Expired,
    Engine(InferenceError),
}

/// The drop guard fails a started batch job unless publication disarms it, preventing an unwinding worker from leaving the job running with its charge held.
struct AbandonGuard {
    inner: Arc<SynapseInner>,
    seq: u64,
    armed: bool,
}

impl Drop for AbandonGuard {
    fn drop(&mut self) {
        if self.armed {
            // A worker that exits without publication reports a host task failure and leaves the lane serving.
            self.inner.jobs.publish_failed(
                self.seq,
                "internal_error".to_owned(),
                "batch worker exited before publication".to_owned(),
            );
        }
    }
}

const RESPONSE_SCRATCH_BYTES: usize = 256;

/// After `shrink_to(owned)`, the resident charge must contain `owned`; a smaller charge undercharges the request because `split_or_take` can return less than requested.
/// `shrink_covered` asserts in debug builds and returns `internal_error` in release builds when `charge.bytes() < owned`.
fn shrink_covered(
    charge: &mut crate::wire::ByteCharge,
    owned: usize,
) -> Result<(), RequestOutcome> {
    charge.shrink_to(owned);
    if charge.bytes() < owned {
        debug_assert!(
            false,
            "parse reservation ({} bytes) is smaller than the post-decode owned bytes ({owned})",
            charge.bytes(),
        );
        return Err(app_error(
            "internal_error",
            "the parse reservation did not cover the decoded request",
        ));
    }
    Ok(())
}

pub(crate) fn owned_input_bytes(request: &Request) -> usize {
    let owned = match request {
        // `Request::ModelsList` uses lane info and releases its charge before responding.
        Request::ModelsList => 0,
        Request::EmbedQuery { text, .. } => text.capacity(),
        Request::EmbedBatch {
            request_key,
            canonical_key,
            items,
        } => jobs::job_input_bytes(request_key, items)
            .saturating_add(request_key.capacity())
            .saturating_add(canonical_key.capacity()),
        Request::EmbedResult {
            job_id,
            request_key,
            cursor,
        } => job_id
            .capacity()
            .saturating_add(request_key.capacity())
            .saturating_add(cursor.as_ref().map_or(0, String::capacity)),
    };
    owned.saturating_add(RESPONSE_SCRATCH_BYTES)
}

fn request_error(error: RequestError) -> RequestOutcome {
    RequestOutcome::error(error.code, error.message)
}

fn app_error(code: &str, message: &str) -> RequestOutcome {
    RequestOutcome::error(code, message)
}

/// The handler is the only deadline owner, so one message covers a query that expired while queued and one that expired while running.
fn expired_query() -> RequestOutcome {
    app_error("timeout", "the query deadline expired")
}

async fn respond(ctx: &RequestCtx, body: &[u8]) -> RequestOutcome {
    let Ok(mut output) = ctx.reserve_output(body.len()).await else {
        return app_error("internal_error", "output reservation failed");
    };
    if output.extend_from_slice(body).is_err() {
        return app_error("internal_error", "output reservation too small");
    }
    RequestOutcome::Response {
        body: output,
        binary: false,
    }
}

/// `respond_vectors` reserves output before serialization so resident-byte accounting covers the body buffer.
/// Only vector-bearing response bodies use the paged-response path.
/// At most `max_handler_tasks` vector-bearing response bodies are in flight.
/// The reservation uses the page's item count rather than the page cap.
/// An oversized reservation holds egress budget for the buffer's lifetime.
async fn respond_vectors(
    ctx: &RequestCtx,
    lane: &LaneInfo,
    items: &[protocol::VectorItemView<'_>],
    done: bool,
    next_cursor: Option<&str>,
) -> RequestOutcome {
    let reservation = protocol::vector_body_reservation(lane, items, next_cursor);
    let Ok(mut output) = ctx.reserve_output(reservation).await else {
        return app_error("internal_error", "output reservation failed");
    };
    if protocol::write_vector_body(&mut output, lane, items, done, next_cursor).is_err() {
        return app_error("internal_error", "output reservation too small");
    }
    RequestOutcome::Response {
        body: output,
        binary: false,
    }
}

impl SynapseComponent {
    /// `received_at` is the handler-entry instant, so the deadline covers preflight, reservation, and decoding rather than restarting after them.
    async fn handle_query(
        &self,
        ctx: &RequestCtx,
        lane: Arc<ReadyLane>,
        text: String,
        deadline_ms: Option<u64>,
        received_at: tokio::time::Instant,
        text_charge: crate::wire::ByteCharge,
    ) -> RequestOutcome {
        let deadline = received_at
            + std::time::Duration::from_millis(
                deadline_ms.unwrap_or(protocol::DEFAULT_DEADLINE_MS),
            );
        // Cancellation outranks expiry on every path: a query whose deadline passed while the host began shutting down reports `cancelled`, which clients retry as a transport-class failure.
        if self.inner.closing.is_cancelled() {
            return app_error("cancelled", "the host is shutting down");
        }
        // A query that spent its deadline on parsing is reported expired here, before it takes an admission slot or spawns a worker, and before saturation could misreport it as `queue_full`.
        if tokio::time::Instant::now() >= deadline {
            return expired_query();
        }
        // Shutdown closes `query_admission` before it drains the tracker, so a closed semaphore reports cancellation rather than overload.
        let query_permit = match Arc::clone(&self.inner.query_admission).try_acquire_owned() {
            Ok(permit) => permit,
            Err(TryAcquireError::Closed) => {
                return app_error("cancelled", "the host is shutting down");
            }
            Err(TryAcquireError::NoPermits) => {
                return RequestOutcome::error_retry_after(
                    "queue_full",
                    "query admission capacity is exhausted",
                    self.inner.limits.query_retry_after_ms,
                );
            }
        };
        // The handler's copy of the admission permit is released once the verdict arrives; the worker's copy remains held through native calls that can outlive request deadlines.
        let handler_query_permit = Arc::new(query_permit);
        let worker_query_permit = Arc::clone(&handler_query_permit);
        let content_sha256 = protocol::sha256_hex(text.as_bytes());
        let (tx, rx) = tokio::sync::oneshot::channel::<Result<Vec<Vec<f32>>, QueryFault>>();
        let inner = Arc::clone(&self.inner);
        let lane_task = Arc::clone(&lane);
        // The tracked task owns the native call; the handler future only waits for its response.
        // The handler owns the deadline: dropping `rx` on expiry or route loss closes `tx`, which cancels a queued call before native work starts.
        self.inner.tracker.spawn(async move {
            let _query_permit = worker_query_permit;
            let _text_charge = text_charge;
            let mut tx = tx;
            let permit = tokio::select! {
                biased;
                () = inner.closing.cancelled() => {
                    let _ = tx.send(Err(QueryFault::Cancelled));
                    return;
                }
                // Once the permit is held, the native call runs to completion even if the receiver closes.
                () = tx.closed() => return,
                permit = Arc::clone(&inner.cpu).acquire_owned() => permit,
            };
            let Ok(_permit) = permit else {
                let _ = tx.send(Err(QueryFault::Engine(InferenceError::Invariant(
                    "cpu semaphore closed".to_owned(),
                ))));
                return;
            };
            // The handler drops its receiver at the deadline, but a handler still hashing or descheduled has not dropped it yet; this check keeps an expired query from consuming the serialized lane.
            if tokio::time::Instant::now() >= deadline {
                let _ = tx.send(Err(QueryFault::Expired));
                return;
            }
            // A predecessor's invariant failure can mark the serialized lane while a query waits for the permit.
            // The failing-lane branch reports the lane's existing fault rather than creating a new one.
            if let Some(reason) = lane_failure_reason(&inner) {
                let _ = tx.send(Err(QueryFault::Engine(InferenceError::Artifact(reason))));
                return;
            }
            let lane_blocking = Arc::clone(&lane_task);
            let joined =
                tokio::task::spawn_blocking(move || lane_blocking.backend.embed(&[text.as_str()]))
                    .await;
            // The vector contract is checked here, while the permit is still held, so a malformed engine result quarantines the lane even when the handler has already expired or been cancelled and no later worker can slip past `lane_failure_reason` first.
            let result = settle_inference(&inner, lane_task.lane.dims, 1, joined)
                .map_err(QueryFault::Engine);
            let _ = tx.send(result);
        });

        let mut rx = rx;
        let result = tokio::select! {
            biased;
            result = &mut rx => match result {
                Err(_) => return app_error("internal_error", "the inference task was lost"),
                Ok(result) => result,
            },
            () = tokio::time::sleep_until(deadline) => return expired_query(),
        };
        // If both arms are ready after descheduling, `biased` selects the receiver, so a vector sent after the deadline needs this post-receive check to be rejected.
        // Cancellation takes precedence over expiry.
        if tokio::time::Instant::now() >= deadline && !matches!(result, Err(QueryFault::Cancelled))
        {
            return expired_query();
        }
        match result {
            Ok(vectors) => {
                // The worker already enforced one valid row; an empty verdict here is a host task fault, not an engine fault.
                let Some(vector) = vectors.first() else {
                    return app_error("internal_error", "the inference verdict carried no vector");
                };
                // The CPU lane is idle once the verdict arrives, so the handler's admission slot can be released before response reservation waits on egress. The returned vector then lives outside the slot's bound, so it is charged to the resident pool first; when that charge is unavailable the slot stays held as the bound instead. The worker's copy still covers a native call that outlives its receiver.
                let vector_bytes = vector.len().saturating_mul(std::mem::size_of::<f32>());
                let _vector_charge = match ctx.try_reserve_resident(vector_bytes) {
                    Some(charge) => {
                        drop(handler_query_permit);
                        Some(charge)
                    }
                    None => None,
                };
                let items = [protocol::VectorItemView {
                    id: "query",
                    content_sha256: &content_sha256,
                    vector,
                }];
                // Output reservation can wait on egress, so the deadline covers response construction too; dropping the future releases any reservation it holds.
                match tokio::time::timeout_at(
                    deadline,
                    respond_vectors(ctx, &lane.lane, &items, true, None),
                )
                .await
                {
                    Ok(outcome) => outcome,
                    Err(_) => expired_query(),
                }
            }
            Err(QueryFault::Cancelled) => app_error("cancelled", "the host is shutting down"),
            Err(QueryFault::Expired) => expired_query(),
            Err(QueryFault::Engine(InferenceError::Input(reason))) => {
                app_error("schema_violation", &reason)
            }
            Err(QueryFault::Engine(InferenceError::Execution(reason))) => {
                app_error("internal_error", &reason)
            }
            Err(QueryFault::Engine(InferenceError::Artifact(reason)))
            | Err(QueryFault::Engine(InferenceError::Invariant(reason))) => {
                app_error("artifact_invalid", &reason)
            }
        }
    }

    async fn handle_batch(
        &self,
        ctx: &RequestCtx,
        lane: Arc<ReadyLane>,
        request_key: String,
        canonical_key: String,
        items: Vec<jobs::BatchItem>,
        mut charge: crate::wire::ByteCharge,
    ) -> RequestOutcome {
        let retry_after_ms = self.inner.limits.retry_after_ms;
        match self.inner.jobs.admit_charged(
            request_key.clone(),
            &canonical_key,
            items,
            lane.lane.dims,
            &mut charge,
        ) {
            AdmitOutcome::Existing(descriptor) => {
                respond(
                    ctx,
                    &protocol::job_descriptor_body(
                        &descriptor.job_id,
                        &request_key,
                        descriptor.status,
                        retry_after_ms,
                    ),
                )
                .await
            }
            AdmitOutcome::Conflict => app_error(
                "idempotency_conflict",
                "the request_key is retained with a different payload",
            ),
            AdmitOutcome::KeyMismatch => app_error(
                "schema_violation",
                "request_key does not match the canonical payload",
            ),
            AdmitOutcome::Full => RequestOutcome::error_retry_after(
                "queue_full",
                "job admission capacity is exhausted",
                retry_after_ms,
            ),
            AdmitOutcome::ResultTooLarge => app_error(
                "schema_violation",
                "batch result exceeds the retained-result byte limit",
            ),
            AdmitOutcome::Closed => app_error("cancelled", "the host is shutting down"),
            AdmitOutcome::Admitted { job_id, seq } => {
                self.spawn_batch_worker(Arc::clone(&lane), seq);
                respond(
                    ctx,
                    &protocol::job_descriptor_body(&job_id, &request_key, "queued", retry_after_ms),
                )
                .await
            }
        }
    }

    fn spawn_batch_worker(&self, lane: Arc<ReadyLane>, seq: u64) {
        let inner = Arc::clone(&self.inner);
        self.inner.tracker.spawn(async move {
            let permit = tokio::select! {
                biased;
                () = inner.closing.cancelled() => return,
                permit = Arc::clone(&inner.cpu).acquire_owned() => permit,
            };
            let Ok(_permit) = permit else { return };
            if let Some(reason) = lane_failure_reason(&inner) {
                inner
                    .jobs
                    .publish_failed(seq, "artifact_invalid".to_owned(), reason);
                return;
            }
            let Some(items) = inner.jobs.start(seq) else {
                return;
            };
            let mut settle_guard = AbandonGuard {
                inner: Arc::clone(&inner),
                seq,
                armed: true,
            };
            let lane_blocking = Arc::clone(&lane);
            let item_count = items.len();
            let joined = tokio::task::spawn_blocking(move || {
                let texts: Vec<&str> = items.iter().map(|item| item.text.as_str()).collect();
                lane_blocking.backend.embed(&texts)
            })
            .await;
            match settle_inference(&inner, lane.lane.dims, item_count, joined) {
                Ok(vectors) => inner.jobs.publish_ready(seq, vectors),
                Err(InferenceError::Input(reason)) => {
                    inner
                        .jobs
                        .publish_failed(seq, "schema_violation".to_owned(), reason);
                }
                // A native execution fault is a retryable job failure; the identical resubmission replaces it.
                Err(InferenceError::Execution(reason)) => {
                    inner
                        .jobs
                        .publish_failed(seq, "internal_error".to_owned(), reason);
                }
                Err(InferenceError::Artifact(reason)) | Err(InferenceError::Invariant(reason)) => {
                    inner
                        .jobs
                        .publish_failed(seq, "artifact_invalid".to_owned(), reason);
                }
            }
            settle_guard.armed = false;
        });
    }

    async fn handle_result(
        &self,
        ctx: &RequestCtx,
        lane: Arc<ReadyLane>,
        job_id: String,
        request_key: String,
        cursor: Option<String>,
    ) -> RequestOutcome {
        // The handler reserves the maximum page-metadata charge before polling because the measured metadata is unavailable until afterward.
        let page_meta_bound = self
            .inner
            .limits
            .page_item_bound()
            .saturating_mul(jobs::MAX_ITEM_ID_BYTES + jobs::CONTENT_SHA256_BYTES);
        let reserved = match ctx.try_reserve_resident(page_meta_bound) {
            Some(charge) => Some(charge),
            None => {
                self.inner.jobs.sweep();
                ctx.try_reserve_resident(page_meta_bound)
            }
        };
        let Some(mut meta_charge) = reserved else {
            return RequestOutcome::error_retry_after(
                "queue_full",
                "resident capacity for the result page is exhausted",
                self.inner.limits.retry_after_ms,
            );
        };
        match self
            .inner
            .jobs
            .poll(&job_id, &request_key, cursor.as_deref())
        {
            PollOutcome::Restarted => app_error(
                "module_restarted",
                "the job is unknown to this host incarnation",
            ),
            PollOutcome::KeyMismatch => {
                app_error("schema_violation", "request_key does not match the job")
            }
            PollOutcome::BadCursor => {
                app_error("schema_violation", "cursor is not valid for this job")
            }
            PollOutcome::Failed { code, message } => RequestOutcome::error(code, message),
            PollOutcome::Pending { status } => {
                respond(
                    ctx,
                    &protocol::pending_body(&job_id, status, self.inner.limits.retry_after_ms),
                )
                .await
            }
            PollOutcome::Page(page) => {
                let meta_bytes: usize = page
                    .vectors
                    .iter()
                    .map(|(id, hash, _)| id.len() + hash.len())
                    .sum();
                if let Err(outcome) = shrink_covered(&mut meta_charge, meta_bytes) {
                    return outcome;
                }
                let items: Vec<protocol::VectorItemView<'_>> = page
                    .vectors
                    .iter()
                    .map(|(id, hash, vector)| protocol::VectorItemView {
                        id,
                        content_sha256: hash,
                        vector,
                    })
                    .collect();
                let outcome = respond_vectors(
                    ctx,
                    &lane.lane,
                    &items,
                    page.done,
                    page.next_cursor.as_deref(),
                )
                .await;
                drop(meta_charge);
                outcome
            }
        }
    }
}

impl CompositeComponent for SynapseComponent {
    fn manifest(&self) -> ManifestSnapshot {
        ManifestSnapshot {
            module_id: SYNAPSE_MODULE_ID.to_owned(),
            module_version: env!("CARGO_PKG_VERSION").to_owned(),
            provides: vec![serde_json::json!({"role": "management_surface"})],
            control_ops: Vec::new(),
        }
    }

    fn resources(&self) -> crate::handler::ResourceDeclaration {
        if self.inner.config.is_none() && self.ready_lane().is_none() {
            return crate::handler::ResourceDeclaration::default();
        }
        crate::handler::ResourceDeclaration {
            // The declared hold bound is the permit count so the startup starvation guard sees exactly the queries that can park.
            general_task_hold_bound: self.inner.limits.query_admission_permits().unwrap_or(1),
            // Local job inputs and retained results live outside ingress charges, so the runtime reserves both caps before sizing ingress.
            // The shared job-table limits prevent wire and local queues from both reaching their full configured count and text caps; reserving the full local cap is conservative.
            retained_resident_bytes: declared_local_input_capacity(&self.inner.limits)
                .saturating_add(self.inner.limits.max_retained_result_bytes),
            ..Default::default()
        }
    }

    async fn bind(&self, _route: RouteHandle, _identity: RouteIdentity) -> BindOutcome {
        match self.status() {
            SynapseStatus::Ready(_) => BindOutcome::Accept,
            SynapseStatus::Starting => BindOutcome::Reject {
                code: "module_reloading".to_owned(),
                message: STARTING_REASON.to_owned(),
            },
            SynapseStatus::Disabled { .. } | SynapseStatus::Failing { .. } => BindOutcome::Reject {
                code: "artifact_invalid".to_owned(),
                message: "the synapse model bundle is unavailable".to_owned(),
            },
        }
    }

    async fn handle(&self, ctx: RequestCtx) -> RequestOutcome {
        // A request-scoped deadline runs from here so a large body cannot buy itself a fresh deadline after parsing.
        let received_at = tokio::time::Instant::now();
        let Some(lane) = self.ready_lane() else {
            return app_error("artifact_invalid", "the synapse lane is unavailable");
        };
        if let Err(error) = protocol::preflight(&ctx.body, ctx.binary) {
            return request_error(error);
        }
        let Some(reservation_bytes) =
            protocol::parse_reservation_bytes(ctx.body.len(), &self.inner.limits)
        else {
            // An overflowing reservation can never be admitted, so it is a size rejection rather than transient backpressure.
            return app_error(
                "schema_violation",
                "request body is too large for this host",
            );
        };
        // A reservation above `capacity` remains unadmittable after draining, so it gets a size rejection instead of `queue_full`.
        let capacity = ctx.resident_capacity();
        if reservation_bytes > capacity {
            return request_error(protocol::unservable_body_error(
                ctx.body.len(),
                reservation_bytes,
                capacity,
            ));
        }
        // The handler sweeps expired jobs after reservation failure because expired charges may be blocking admission.
        // Sweeping only after reservation failure avoids the job-table lock and expiry scan on successful requests.
        let reserved = match ctx.try_reserve_resident(reservation_bytes) {
            Some(charge) => Some(charge),
            None => {
                self.inner.jobs.sweep();
                ctx.try_reserve_resident(reservation_bytes)
            }
        };
        let Some(mut charge) = reserved else {
            return RequestOutcome::error_retry_after(
                "queue_full",
                "resident capacity for request parsing is exhausted",
                self.inner.limits.retry_after_ms,
            );
        };
        let request = match protocol::decode_request(&ctx.body, &lane.lane, &self.inner.limits) {
            Ok(request) => request,
            Err(error) => {
                drop(charge);
                return request_error(error);
            }
        };
        // `owned_input_bytes` must fit within `charge`.
        if let Err(outcome) = shrink_covered(&mut charge, owned_input_bytes(&request)) {
            return outcome;
        }
        match request {
            Request::ModelsList => {
                drop(charge);
                respond(&ctx, &lane.models_list).await
            }
            Request::EmbedQuery { text, deadline_ms } => {
                let text_charge = charge.split_or_take(text.capacity());
                let _handler_charge = charge;
                self.handle_query(&ctx, lane, text, deadline_ms, received_at, text_charge)
                    .await
            }
            Request::EmbedBatch {
                request_key,
                canonical_key,
                items,
            } => {
                self.handle_batch(&ctx, lane, request_key, canonical_key, items, charge)
                    .await
            }
            Request::EmbedResult {
                job_id,
                request_key,
                cursor,
            } => {
                let _handler_charge = charge;
                self.handle_result(&ctx, lane, job_id, request_key, cursor)
                    .await
            }
        }
    }

    async fn route_gone(&self, _route: RouteHandle) {}

    async fn health(&self) -> HealthReport {
        match self.status() {
            SynapseStatus::Ready(_) => HealthReport {
                status: HealthStatus::Ok,
                detail: None,
                metrics: Some(serde_json::json!({"synapse_state": "ready"})),
            },
            SynapseStatus::Starting => HealthReport {
                status: HealthStatus::Degraded,
                detail: Some(STARTING_REASON.to_owned()),
                metrics: Some(serde_json::json!({"synapse_state": "starting"})),
            },
            SynapseStatus::Disabled { reason } => HealthReport {
                status: HealthStatus::Degraded,
                metrics: Some(serde_json::json!({
                    "synapse_state": if is_unsupported_reason(&reason) {
                        "unsupported"
                    } else {
                        "degraded"
                    }
                })),
                detail: Some(reason),
            },
            SynapseStatus::Failing { reason } => HealthReport {
                status: HealthStatus::Failing,
                detail: Some(reason),
                metrics: Some(serde_json::json!({"synapse_state": "degraded"})),
            },
        }
    }

    /// Shutdown closes admission and cancels queued wrappers before joining every started native inference call through its incarnation.
    /// Shutdown never aborts a started native call.
    /// Token counting is not joined: each in-flight count retains its lane through an `Arc`, and its settlement cannot replace completed shutdown.
    /// The lane ends `Disabled` so a late `bind`, `health`, or `embed_blocking` observes the shutdown instead of a ready lane whose admission is closed.
    /// `embed_blocking` calls are not tracked, so shutdown joins them through the CPU permit: an in-flight blocking call holds that permit until it returns, and the terminal state is written while shutdown holds it, so a call that read `Ready` earlier observes `Disabled` when it rechecks after acquiring the permit, and a joined call that failed cannot overwrite the shutdown state.
    async fn shutdown(&self) -> Result<(), crate::composite::ShutdownError> {
        self.inner.closing.cancel();
        self.inner.jobs.close_admission();
        // Closing `query_admission` before the tracker makes late queries observe cancellation instead of spawning workers into a draining tracker.
        self.inner.query_admission.close();
        self.inner.tracker.close();
        self.inner.tracker.wait().await;
        self.inner.jobs.clear();
        // Taking the permit joins an in-flight `embed_blocking` call; its own failure transition settles before the shutdown state is written.
        let permit = self.inner.cpu.acquire().await;
        *self.inner.lock_state() = LaneState::ShutDown;
        drop(permit);
        Ok(())
    }
}

impl SecondaryComponent for SynapseComponent {
    async fn initialize(&self) -> Result<(), InitError> {
        let mut state = self.inner.lock_state();
        // A pre-readied lane has no configuration to load and remains ready.
        if matches!(&*state, LaneState::Ready(_)) {
            return Ok(());
        }
        if let Some(config) = &self.inner.config
            && let Err(error) = bundle::validate_limits(&config.limits)
        {
            return Err(InitError(format!(
                "synapse limits are invalid: {}",
                error.0
            )));
        }
        *state = if self.inner.config.is_some() {
            // Transport does not wait for bundle verification, ORT loading, or model construction.
            // Pre-publication bootstrap records only that the lane is starting.
            LaneState::Starting
        } else if let Some(reason) = self.inner.unsupported_reason {
            LaneState::Disabled {
                reason: reason.to_owned(),
            }
        } else {
            LaneState::Disabled {
                reason: "no bundle configured".to_owned(),
            }
        };
        Ok(())
    }

    async fn activate(&self) -> Result<(), InitError> {
        let Some(config) = self.inner.config.clone() else {
            return Ok(());
        };
        // Dropping the activation future does not stop the blocking task.
        let blocking = tokio::task::spawn_blocking(move || {
            let bundle = bundle::load_bundle(
                &config.bundle_dir,
                &config.limits,
                config.bundle_manifest_sha256.as_deref(),
            )
            .map_err(|error| InferenceError::Artifact(error.0))?;
            let ort = OrtIdentity {
                library: config.ort_library.clone(),
                sha256: config.ort_library_sha256.clone(),
            };
            let lane = LaneInfo::from_bundle(&bundle);
            let backend = Backend::load(bundle, &ort)?;
            Ok::<_, InferenceError>(ReadyLane::new(Arc::new(backend), lane))
        });
        let loaded = match self.inner.tracker.spawn(blocking).await {
            Ok(joined) => joined,
            Err(join_error) => Err(join_error),
        };
        match loaded {
            Ok(loaded) => {
                *self.inner.lock_state() = lane_state_after_load(loaded);
                Ok(())
            }
            Err(join_error) => Err(InitError(format!(
                "synapse activation task failed: {join_error}"
            ))),
        }
    }
}

/// A failed load disables or fails the lane without failing activation, so the host keeps serving its other components.
fn lane_state_after_load(loaded: Result<ReadyLane, InferenceError>) -> LaneState {
    match loaded {
        Ok(lane) => LaneState::Ready(Arc::new(lane)),
        Err(InferenceError::Invariant(reason)) => LaneState::Failing { reason },
        Err(InferenceError::Artifact(reason))
        | Err(InferenceError::Input(reason))
        | Err(InferenceError::Execution(reason)) => LaneState::Disabled { reason },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoopEngine(f32);

    impl EmbeddingEngine for NoopEngine {
        fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, InferenceError> {
            Ok(texts.iter().map(|_| vec![self.0]).collect())
        }

        fn untruncated_token_len(&self, text: &str) -> Result<EmbedTokens, InferenceError> {
            Ok(EmbedTokens::new(text.split_whitespace().count() as u32))
        }
    }

    fn lane() -> LaneInfo {
        LaneInfo {
            model: "m".to_owned(),
            fingerprint: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                .to_owned(),
            table_epoch: 1,
            dims: 1,
            execution_provider: "cpu",
            max_tokens: 8,
            max_text_bytes: 16,
            provenance: serde_json::Value::Null,
            recommended_rows: 1,
            recommended_token_budget: 8,
        }
    }

    #[test]
    fn the_declared_hold_bound_is_the_query_permit_count() {
        for max_waiting_queries in [0usize, 1, 2, 5] {
            let limits = SynapseLimits {
                max_waiting_queries,
                max_queued_request_bytes: 8 * 1024 * 1024,
                ..SynapseLimits::default()
            };
            let expected = limits
                .query_admission_permits()
                .expect("default-shaped limits have a permit count");
            let component =
                SynapseComponent::ready_with_engine(lane(), Arc::new(NoopEngine(1.0)), limits)
                    .expect("limits validate");
            assert_eq!(
                component.resources().general_task_hold_bound,
                expected,
                "max_waiting_queries {max_waiting_queries}"
            );
        }
    }

    #[test]
    fn local_job_inputs_are_declared_as_retained_resident_memory() {
        let limits = SynapseLimits {
            max_queued_jobs: 1,
            max_queued_request_bytes: 8 * 1024 * 1024,
            ..SynapseLimits::default()
        };
        let expected = limits
            .max_retained_result_bytes
            .checked_add(bundle::max_queued_input_bytes(&limits).unwrap())
            .and_then(|bytes| bytes.checked_add(bundle::max_retained_input_bytes(&limits).unwrap()))
            .unwrap();
        let component =
            SynapseComponent::ready_with_engine(lane(), Arc::new(NoopEngine(1.0)), limits)
                .expect("limits validate");

        assert_eq!(
            component.resources().retained_resident_bytes,
            expected,
            "local input retention is reserved outside ingress"
        );
    }

    #[test]
    fn local_input_charge_mismatch_is_an_unsupported_shape() {
        assert_eq!(
            local_input_charge_mismatch(2, 1),
            Some(SubmitOutcome::Refused("unsupported_shape"))
        );
        assert_eq!(local_input_charge_mismatch(1, 1), None);
    }

    #[tokio::test]
    async fn overflowing_unvalidated_limits_fail_initialization_without_panicking() {
        let limits = SynapseLimits {
            max_queued_jobs: usize::MAX,
            max_queued_request_bytes: u64::MAX,
            max_batch_items: usize::MAX,
            ..SynapseLimits::default()
        };
        let component = SynapseComponent::new(Some(SynapseConfig {
            bundle_dir: PathBuf::from("unused"),
            bundle_manifest_sha256: None,
            ort_library: PathBuf::from("unused"),
            ort_library_sha256: String::new(),
            limits,
        }));
        let declaration = component.resources().retained_resident_bytes;
        let initialized = SecondaryComponent::initialize(&component).await;

        assert!(
            declaration == u64::MAX && initialized.is_err(),
            "unvalidated declaration={declaration}, initialize={initialized:?}"
        );
    }

    #[test]
    fn large_unvalidated_item_count_does_not_delay_construction() {
        let limits = SynapseLimits {
            max_batch_items: 1_000_000_000_000,
            ..SynapseLimits::default()
        };
        let (sent, received) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let component = SynapseComponent::new(Some(SynapseConfig {
                bundle_dir: PathBuf::from("unused"),
                bundle_manifest_sha256: None,
                ort_library: PathBuf::from("unused"),
                ort_library_sha256: String::new(),
                limits,
            }));
            let _ = sent.send(component.resources().retained_resident_bytes);
        });

        received
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("construction must not iterate over the configured item count");
    }

    #[tokio::test]
    async fn maximal_retained_result_limit_declares_failure_without_panicking() {
        let limits = SynapseLimits {
            max_retained_result_bytes: u64::MAX,
            ..SynapseLimits::default()
        };
        let component = SynapseComponent::new(Some(SynapseConfig {
            bundle_dir: PathBuf::from("unused"),
            bundle_manifest_sha256: None,
            ort_library: PathBuf::from("unused"),
            ort_library_sha256: String::new(),
            limits,
        }));

        assert_eq!(component.resources().retained_resident_bytes, u64::MAX);
        let error = SecondaryComponent::initialize(&component)
            .await
            .expect_err("the combined resource declaration must be representable");
        assert!(
            error
                .to_string()
                .contains("combined local input and retained result capacity overflows"),
            "{error}"
        );
    }

    struct CountingEngine(Arc<std::sync::atomic::AtomicUsize>);

    impl EmbeddingEngine for CountingEngine {
        fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, InferenceError> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(texts.iter().map(|_| vec![1.0]).collect())
        }

        fn untruncated_token_len(&self, text: &str) -> Result<EmbedTokens, InferenceError> {
            Ok(EmbedTokens::new(text.split_whitespace().count() as u32))
        }
    }

    #[tokio::test]
    async fn local_input_budget_refuses_before_copy_and_recovers_after_release() {
        let limits = SynapseLimits {
            max_queued_jobs: 1,
            max_queued_request_bytes: 8 * 1024 * 1024,
            ..SynapseLimits::default()
        };
        let queued_capacity = bundle::max_queued_input_bytes(&limits).unwrap() as usize;
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let component = SynapseComponent::ready_with_engine(
            lane(),
            Arc::new(CountingEngine(Arc::clone(&calls))),
            limits,
        )
        .unwrap();
        let admitted = component
            .preflight_embedding_for_lane(&lane(), "local input")
            .unwrap();
        assert_eq!(
            component
                .submit_admitted(&admitted, &"x".repeat(jobs::MAX_ITEM_ID_BYTES + 1))
                .unwrap(),
            SubmitOutcome::Refused("unsupported_shape")
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        let capacity = component.inner.local_inputs.capacity();
        assert!(
            capacity > queued_capacity,
            "local capacity must also retain completed-job metadata"
        );
        let held = component.inner.local_inputs.try_charge(capacity).unwrap();

        assert_eq!(
            component.submit_admitted(&admitted, "item").unwrap(),
            SubmitOutcome::Full
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        drop(held);

        let cpu = component.inner.cpu.acquire().await.unwrap();
        let SubmitOutcome::Queued { job_id } =
            component.submit_admitted(&admitted, "item").unwrap()
        else {
            panic!("released local budget admits work");
        };
        let charged = component.inner.local_inputs.available();
        assert_eq!(
            charged,
            capacity - local_input_charge_bound("item", "local input").unwrap(),
            "queued input holds its exact owned charge"
        );
        drop(cpu);
        component.inner.tracker.close();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            component.inner.tracker.wait(),
        )
        .await
        .unwrap();
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(matches!(
            component.poll_admitted(&lane(), &job_id, "item", "local input"),
            PollOutcome::Page(_)
        ));
        assert_eq!(
            component.inner.local_inputs.available(),
            capacity
                - jobs::retained_input_bytes(
                    protocol::CANONICAL_REQUEST_KEY_BYTES,
                    std::iter::once(("item".len(), jobs::CONTENT_SHA256_BYTES)),
                ),
            "completion keeps only key and item metadata"
        );
        component.inner.jobs.clear();
        assert_eq!(component.inner.local_inputs.available(), capacity);
    }

    #[tokio::test]
    async fn retained_local_replay_bypasses_saturated_input_budget() {
        let component = SynapseComponent::ready_with_engine(
            lane(),
            Arc::new(NoopEngine(1.0)),
            SynapseLimits {
                max_queued_jobs: 1,
                max_queued_request_bytes: 8 * 1024 * 1024,
                ..SynapseLimits::default()
            },
        )
        .unwrap();
        let cpu = Arc::clone(&component.inner.cpu)
            .acquire_owned()
            .await
            .unwrap();
        let admitted = component
            .preflight_embedding_for_lane(&lane(), "retained replay")
            .unwrap();
        let SubmitOutcome::Queued { job_id } =
            component.submit_admitted(&admitted, "item").unwrap()
        else {
            panic!("first submit must queue")
        };
        let saturation = component
            .inner
            .local_inputs
            .try_charge(component.inner.local_inputs.available())
            .unwrap();

        assert_eq!(
            component.submit_admitted(&admitted, "item").unwrap(),
            SubmitOutcome::Queued { job_id }
        );

        component.inner.jobs.clear();
        drop(saturation);
        drop(cpu);
        component.inner.closing.cancel();
        component.inner.tracker.close();
        component.inner.tracker.wait().await;
    }

    #[test]
    fn retained_local_conflict_precedes_input_budget_refusal() {
        let component = SynapseComponent::ready_with_engine(
            lane(),
            Arc::new(NoopEngine(1.0)),
            SynapseLimits {
                max_queued_jobs: 1,
                max_queued_request_bytes: 8 * 1024 * 1024,
                ..SynapseLimits::default()
            },
        )
        .unwrap();
        let admitted = component
            .preflight_embedding_for_lane(&lane(), "expected payload")
            .unwrap();
        let expected_digest = protocol::sha256_hex(admitted.text().as_bytes());
        let key = local_key(&lane(), "item", &expected_digest);
        let conflicting_digest = protocol::sha256_hex(b"different payload");
        assert!(matches!(
            component.inner.jobs.admit_uncharged_for_tests(
                key,
                vec![local_item(
                    "different",
                    "different payload",
                    conflicting_digest,
                )],
                1,
            ),
            jobs::AdmitOutcome::Admitted { .. }
        ));
        let saturation = component
            .inner
            .local_inputs
            .try_charge(component.inner.local_inputs.available())
            .unwrap();

        assert_eq!(
            component.submit_admitted(&admitted, "item").unwrap(),
            SubmitOutcome::Refused("idempotency_conflict")
        );

        component.inner.jobs.clear();
        drop(saturation);
    }

    #[test]
    fn an_unready_lane_reports_its_reason_without_panicking() {
        let component = SynapseComponent::unsupported("synapse_unsupported");
        match component.embed_blocking(&["x"]) {
            Err(InferenceError::Artifact(reason)) => assert_eq!(reason, "synapse_unsupported"),
            other => panic!("expected an artifact error, got {other:?}"),
        }
        let component = SynapseComponent::new(None);
        match component.embed_blocking(&["x"]) {
            Err(InferenceError::Artifact(reason)) => assert_eq!(reason, "not initialized"),
            other => panic!("expected an artifact error, got {other:?}"),
        }
    }

    #[test]
    fn embed_blocking_shares_the_cpu_permit_and_reports_a_held_lane() {
        let component = SynapseComponent::ready_with_engine(
            lane(),
            Arc::new(NoopEngine(1.0)),
            SynapseLimits {
                max_queued_request_bytes: 8 * 1024 * 1024,
                ..SynapseLimits::default()
            },
        )
        .expect("limits validate");
        let held = component
            .inner
            .cpu
            .try_acquire()
            .expect("the lane permit is free");
        match component.embed_blocking(&["x"]) {
            Err(InferenceError::Artifact(reason)) => assert_eq!(reason, BUSY_REASON),
            other => panic!("expected a busy artifact error, got {other:?}"),
        }
        drop(held);
        assert_eq!(
            component
                .embed_blocking(&["x"])
                .expect("released lane embeds"),
            vec![vec![1.0]]
        );
        assert_eq!(component.inner.cpu.available_permits(), 1);
    }

    #[tokio::test]
    async fn shutdown_disables_the_lane_for_late_callers() {
        let component = SynapseComponent::ready_with_engine(
            lane(),
            Arc::new(NoopEngine(1.0)),
            SynapseLimits {
                max_queued_request_bytes: 8 * 1024 * 1024,
                ..SynapseLimits::default()
            },
        )
        .expect("limits validate");
        assert!(matches!(component.status(), SynapseStatus::Ready(_)));

        component.shutdown().await.expect("shutdown drains cleanly");

        match component.status() {
            SynapseStatus::Disabled { reason } => assert_eq!(reason, SHUT_DOWN_REASON),
            other => panic!("a drained lane must be disabled, got {other:?}"),
        }
        let route = RouteHandle {
            channel: 1,
            epoch: 1,
        };
        let identity = RouteIdentity {
            project_root: PathBuf::from("/"),
            harness: "test".to_owned(),
            session: "test".to_owned(),
            consumer_module_id: None,
            consumer_launch_nonce: None,
            consumer_capabilities: Vec::new(),
            admission_facts: None,
            credential_fingerprints: Default::default(),
        };
        assert!(matches!(
            component.bind(route, identity).await,
            BindOutcome::Reject { code, .. } if code == "artifact_invalid"
        ));
        assert_eq!(component.health().await.status, HealthStatus::Degraded);
        match component.embed_blocking(&["x"]) {
            Err(InferenceError::Artifact(reason)) => assert_eq!(reason, SHUT_DOWN_REASON),
            other => panic!("a drained lane must refuse to embed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn local_jobs_are_keyed_by_the_admitting_lane() {
        let lane_a = lane();
        let mut lane_b = lane_a.clone();
        lane_b.model = "replacement".to_owned();
        lane_b.fingerprint = "b2".repeat(32);
        lane_b.table_epoch += 1;
        let component = SynapseComponent::ready_with_engine(
            lane_a.clone(),
            Arc::new(NoopEngine(1.0)),
            SynapseLimits::default(),
        )
        .unwrap();
        let permit = component.inner.cpu.acquire().await.unwrap();
        let mut jobs = Vec::new();
        for identity in [&lane_a, &lane_b] {
            let admitted = component
                .preflight_embedding(EmbeddingInputLimits::of_lane(identity), "same text")
                .unwrap();
            let submitted = component.submit_admitted(&admitted, "same-item").unwrap();
            assert_eq!(
                component.submit_admitted(&admitted, "same-item").unwrap(),
                submitted,
                "a retained job is idempotent within its lane"
            );
            let SubmitOutcome::Queued { job_id } = submitted else {
                panic!("submission must queue")
            };
            jobs.push(job_id);
            component
                .replace_ready_with_engine_for_test(lane_b.clone(), Arc::new(NoopEngine(-1.0)))
                .unwrap();
        }
        assert_ne!(
            jobs[0], jobs[1],
            "a replacement lane must get a distinct job"
        );
        drop(permit);
        component.inner.tracker.close();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            component.inner.tracker.wait(),
        )
        .await
        .unwrap();
        mark_failing(&component.inner, "test failure".to_owned());
        for ((identity, job), expected) in
            [&lane_a, &lane_b].into_iter().zip(&jobs).zip([1.0, -1.0])
        {
            let PollOutcome::Page(page) =
                component.poll_admitted(identity, job, "same-item", "same text")
            else {
                panic!("settled jobs must remain pollable under their admitted lane")
            };
            assert_eq!(
                page.vectors[0].2.as_ref(),
                &[expected],
                "worker must retain its engine"
            );
            let other_job = if job == &jobs[0] { &jobs[1] } else { &jobs[0] };
            assert!(matches!(
                component.poll_admitted(identity, other_job, "same-item", "same text"),
                PollOutcome::KeyMismatch
            ));
        }
    }

    #[tokio::test]
    async fn wire_key_cannot_poll_local_job() {
        let identity = lane();
        let component = SynapseComponent::ready_with_engine(
            identity.clone(),
            Arc::new(NoopEngine(1.0)),
            SynapseLimits::default(),
        )
        .unwrap();
        let admitted = component
            .preflight_embedding(EmbeddingInputLimits::of_lane(&identity), "text")
            .unwrap();
        let SubmitOutcome::Queued { job_id } =
            component.submit_admitted(&admitted, "item").unwrap()
        else {
            panic!("submission must queue")
        };
        component.inner.tracker.close();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            component.inner.tracker.wait(),
        )
        .await
        .unwrap();
        assert!(matches!(
            component.poll_admitted(&identity, &job_id, "item", "text"),
            PollOutcome::Page(_)
        ));
        let wire_key = protocol::canonical_request_key(
            &identity,
            &[local_item("item", "text", protocol::sha256_hex(b"text"))],
        );
        assert!(
            matches!(
                component.inner.jobs.poll(&job_id, &wire_key, None),
                PollOutcome::KeyMismatch
            ),
            "a wire canonical key must not poll a local job"
        );
    }

    #[test]
    fn load_failures_route_invariants_to_failing_and_artifacts_to_disabled() {
        match lane_state_after_load(Err(InferenceError::Invariant("bad norm".to_owned()))) {
            LaneState::Failing { reason } => assert_eq!(reason, "bad norm"),
            _ => panic!("an invariant failure must mark the lane failing"),
        }
        match lane_state_after_load(Err(InferenceError::Artifact("missing".to_owned()))) {
            LaneState::Disabled { reason } => assert_eq!(reason, "missing"),
            _ => panic!("an artifact failure must disable the lane"),
        }
        assert!(matches!(
            lane_state_after_load(Ok(ReadyLane::new(Arc::new(NoopEngine(1.0)), lane()))),
            LaneState::Ready(_)
        ));
    }

    #[tokio::test]
    async fn local_poll_uses_frozen_identity_even_when_the_lane_fails() {
        let identity = lane();
        let component = SynapseComponent::ready_with_engine(
            identity.clone(),
            Arc::new(NoopEngine(1.0)),
            SynapseLimits::default(),
        )
        .unwrap();
        let admitted = component
            .preflight_embedding(EmbeddingInputLimits::of_lane(&identity), "text")
            .unwrap();
        let SubmitOutcome::Queued { job_id } =
            component.submit_admitted(&admitted, "item").unwrap()
        else {
            panic!("submission must queue")
        };
        component.inner.tracker.close();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            component.inner.tracker.wait(),
        )
        .await
        .unwrap();
        mark_failing(&component.inner, "test failure".to_owned());
        let PollOutcome::Page(page) = component.poll_admitted(&identity, &job_id, "item", "text")
        else {
            panic!("settled result must not depend on lane availability")
        };
        assert_eq!(page.vectors[0].2.as_ref(), &[1.0]);
        let mut wrong_lane = identity.clone();
        wrong_lane.table_epoch += 1;
        for (lane, item, text) in [
            (&wrong_lane, "item", "text"),
            (&identity, "other", "text"),
            (&identity, "item", "other"),
        ] {
            assert!(matches!(
                component.poll_admitted(lane, &job_id, item, text),
                PollOutcome::KeyMismatch
            ));
        }
    }

    struct GatedCountEngine {
        entered: std::sync::mpsc::Sender<()>,
        release: std::sync::Mutex<std::sync::mpsc::Receiver<InferenceError>>,
    }

    impl EmbeddingEngine for GatedCountEngine {
        fn embed(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, InferenceError> {
            Err(InferenceError::Artifact("artifact fault".to_owned()))
        }

        fn untruncated_token_len(&self, text: &str) -> Result<EmbedTokens, InferenceError> {
            if text == "artifact" {
                return Err(InferenceError::Artifact("count artifact fault".to_owned()));
            }
            self.entered.send(()).expect("test observes entry");
            let error = self
                .release
                .lock()
                .expect("release receiver")
                .recv()
                .expect("injected counter panic: release channel closed");
            Err(error)
        }
    }

    fn gated_count_component() -> (
        Arc<SynapseComponent>,
        std::sync::mpsc::Receiver<()>,
        std::sync::mpsc::Sender<InferenceError>,
    ) {
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let component = Arc::new(
            SynapseComponent::ready_with_engine(
                lane(),
                Arc::new(GatedCountEngine {
                    entered: entered_tx,
                    release: std::sync::Mutex::new(release_rx),
                }),
                SynapseLimits {
                    max_queued_request_bytes: 8 * 1024 * 1024,
                    ..SynapseLimits::default()
                },
            )
            .expect("limits validate"),
        );
        (component, entered_rx, release_tx)
    }

    #[test]
    fn frozen_preflight_settles_count_failures_and_panics() {
        for error in [
            InferenceError::Input("private input".to_owned()),
            InferenceError::Execution("private input".to_owned()),
            InferenceError::Artifact("private input".to_owned()),
            InferenceError::Invariant("private input".to_owned()),
        ] {
            let kind = InferenceFailureKind::from(&error);
            let (component, _entered, release) = gated_count_component();
            release.send(error).unwrap();
            assert_eq!(
                component.preflight_embedding_for_lane(&lane(), "private input"),
                Err(DenseUnavailable::CountUnavailable(kind))
            );
            match (kind, component.status()) {
                (
                    InferenceFailureKind::Input | InferenceFailureKind::Execution,
                    SynapseStatus::Ready(_),
                ) => {}
                (InferenceFailureKind::Artifact, SynapseStatus::Disabled { reason }) => {
                    assert_eq!(reason, "token counting declared the artifact unusable");
                }
                (InferenceFailureKind::Invariant, SynapseStatus::Failing { reason }) => {
                    assert_eq!(reason, "token counting failed an invariant");
                }
                (_, state) => panic!("unexpected state for {kind:?}: {state:?}"),
            }
        }

        let (component, _entered, release) = gated_count_component();
        drop(release);
        assert_eq!(
            component.preflight_embedding_for_lane(&lane(), "private input"),
            Err(DenseUnavailable::CountUnavailable(
                InferenceFailureKind::Invariant
            ))
        );
        let SynapseStatus::Failing { reason } = component.status() else {
            panic!("a counter panic quarantines the frozen lane");
        };
        assert_eq!(reason, "token counting panicked");
    }

    #[tokio::test]
    async fn a_count_failure_that_settles_after_shutdown_preserves_the_terminal_state() {
        let (component, entered_rx, release_tx) = gated_count_component();
        let budget = std::time::Duration::from_secs(5);
        let limits = EmbeddingInputLimits::of_lane(&lane());
        let counting = {
            let component = Arc::clone(&component);
            tokio::task::spawn_blocking(move || component.preflight_embedding(limits, "alpha beta"))
        };
        entered_rx
            .recv_timeout(budget)
            .expect("the count is entered");

        let shutdown = tokio::time::timeout(budget, component.shutdown()).await;
        let stopped = component.status();

        release_tx
            .send(InferenceError::Invariant("count overflowed".to_owned()))
            .expect("the count is released");
        let refusal = tokio::time::timeout(budget, counting)
            .await
            .expect("the released count completes within the budget")
            .expect("the counting thread joins")
            .expect_err("an invariant count fails the preflight");
        shutdown
            .expect("shutdown must not wait for the blocked count")
            .expect("shutdown drains cleanly");
        match stopped {
            SynapseStatus::Disabled { reason } => assert_eq!(reason, SHUT_DOWN_REASON),
            other => panic!("a drained lane must be disabled, got {other:?}"),
        }
        assert_eq!(
            refusal,
            DenseUnavailable::CountUnavailable(InferenceFailureKind::Invariant)
        );
        match component.status() {
            SynapseStatus::Disabled { reason } => assert_eq!(
                reason, SHUT_DOWN_REASON,
                "a late count failure must not overwrite completed shutdown"
            ),
            other => panic!("the terminal shutdown state must survive, got {other:?}"),
        }
        assert_eq!(component.health().await.status, HealthStatus::Degraded);
    }

    #[tokio::test]
    async fn a_count_invariant_escalates_runtime_disablement() {
        for artifact_source in ["inference", "count"] {
            let (component, entered_rx, release_tx) = gated_count_component();
            let budget = std::time::Duration::from_secs(5);
            let limits = EmbeddingInputLimits::of_lane(&lane());
            let counting = {
                let component = Arc::clone(&component);
                tokio::task::spawn_blocking(move || {
                    component.preflight_embedding(limits, "alpha beta")
                })
            };
            entered_rx
                .recv_timeout(budget)
                .expect("the count is entered");
            if artifact_source == "inference" {
                assert!(matches!(
                    component.embed_blocking(&["alpha"]),
                    Err(InferenceError::Artifact(_))
                ));
            } else {
                assert_eq!(
                    component.preflight_embedding(limits, "artifact"),
                    Err(DenseUnavailable::CountUnavailable(
                        InferenceFailureKind::Artifact
                    ))
                );
            }
            assert!(matches!(component.status(), SynapseStatus::Disabled { .. }));
            release_tx
                .send(InferenceError::Invariant("count overflowed".to_owned()))
                .expect("the count is released");
            let result = tokio::time::timeout(budget, counting)
                .await
                .expect("the count completes")
                .expect("the count does not panic");
            assert_eq!(
                result,
                Err(DenseUnavailable::CountUnavailable(
                    InferenceFailureKind::Invariant
                ))
            );
            assert!(
                matches!(component.status(), SynapseStatus::Failing { .. }),
                "an invariant must escalate {artifact_source} disablement: {:?}",
                component.status()
            );
            assert_eq!(component.health().await.status, HealthStatus::Failing);
        }
    }

    #[tokio::test]
    async fn a_counter_panic_is_quarantined_as_a_content_free_invariant() {
        let (component, entered_rx, release_tx) = gated_count_component();
        let budget = std::time::Duration::from_secs(5);
        let limits = EmbeddingInputLimits::of_lane(&lane());
        let counting = {
            let component = Arc::clone(&component);
            tokio::task::spawn_blocking(move || component.preflight_embedding(limits, "alpha beta"))
        };
        entered_rx
            .recv_timeout(budget)
            .expect("the count is entered");
        drop(release_tx);
        let result = tokio::time::timeout(budget, counting)
            .await
            .expect("the count completes")
            .expect("preflight must contain a backend panic");
        assert_eq!(
            result,
            Err(DenseUnavailable::CountUnavailable(
                InferenceFailureKind::Invariant
            ))
        );
        let SynapseStatus::Failing { reason } = component.status() else {
            panic!("a counter panic quarantines the lane");
        };
        assert_eq!(reason, "token counting panicked");
        assert_eq!(component.health().await.status, HealthStatus::Failing);
        assert_eq!(
            component.preflight_embedding(limits, "alpha beta"),
            Err(DenseUnavailable::LaneUnavailable {
                state: LaneUnavailableState::Failing
            })
        );
    }
}
