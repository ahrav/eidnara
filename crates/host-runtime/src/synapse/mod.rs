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
pub mod inference;
pub mod jobs;
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
use inference::{Backend, InferenceError, OrtIdentity};
use jobs::{AdmitOutcome, JobTable, PollOutcome};
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
    /// Retained result vectors are declared to the host as `retained_resident_bytes`, so `HostLimits::max_resident_bytes` must leave room for this cap above the resident floor.
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
    pub fn per_waiter_charge_bound(&self) -> Option<u64> {
        u64::try_from(self.max_text_bytes)
            .ok()?
            .checked_mul(2)?
            .checked_add(RESPONSE_SCRATCH_BYTES as u64)
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
}

impl EmbeddingEngine for Backend {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, InferenceError> {
        Backend::embed(self, texts)
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
    Disabled { reason: String },
    Ready(Arc<ReadyLane>),
    Failing { reason: String },
}

struct SynapseInner {
    config: Option<SynapseConfig>,
    unsupported_reason: Option<&'static str>,
    limits: SynapseLimits,
    state: Mutex<LaneState>,
    jobs: JobTable,
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
        bundle::validate_serving_limits(lane.dims, lane.recommended_rows as usize, &limits)?;
        // `validate_serving_limits` rejects permit-count overflow.
        // Validated limits always have a permit count.
        let query_admission_permits = limits
            .query_admission_permits()
            .expect("validate_serving_limits proves the permit count");
        lane.max_text_bytes = limits.max_text_bytes;
        Ok(Self {
            inner: Arc::new(SynapseInner {
                config: None,
                unsupported_reason: None,
                jobs: JobTable::new(limits.clone()),
                limits,
                state: Mutex::new(LaneState::Ready(Arc::new(ReadyLane::new(engine, lane)))),
                cpu: Arc::new(tokio::sync::Semaphore::new(1)),
                query_admission: Arc::new(tokio::sync::Semaphore::new(query_admission_permits)),
                tracker: TaskTracker::new(),
                closing: CancellationToken::new(),
            }),
        })
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
        }
    }

    fn ready_lane(&self) -> Option<Arc<ReadyLane>> {
        match &*self.inner.lock_state() {
            LaneState::Ready(lane) => Some(Arc::clone(lane)),
            _ => None,
        }
    }

    /// `embed_blocking` shares the lane's single `cpu` permit with routed queries and batch workers, so at most one native call runs at a time.
    /// A lane whose permit is held or closed reports `Artifact` without waiting, because a synchronous caller cannot park on the async semaphore.
    /// `Invariant` errors mark the lane failing before returning, so later callers cannot obtain vectors from a suspect backend.
    /// The lane is read under one lock acquisition so a concurrent `activate` cannot change the state between the readiness check and the reason lookup.
    pub fn embed_blocking(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, InferenceError> {
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
            }
        };
        let _permit = self
            .inner
            .cpu
            .try_acquire()
            .map_err(|_| InferenceError::Artifact(BUSY_REASON.to_owned()))?;
        // A concurrent holder can mark the lane failing between the state read and this acquisition; the captured backend must not run after that transition.
        if let Some(reason) = lane_failure_reason(&self.inner) {
            return Err(InferenceError::Artifact(reason));
        }
        // A panicking backend is quarantined the same way the routed workers quarantine a panicked blocking task; the caller sees the same `Invariant` instead of an unwind that leaves the lane `Ready`.
        let joined =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| lane.backend.embed(texts)))
                .map_err(|_| PanickedBackend);
        settle_inference(&self.inner, joined)
    }
}

fn mark_failing(inner: &SynapseInner, reason: String) {
    let mut state = inner.lock_state();
    if matches!(&*state, LaneState::Ready(_)) {
        *state = LaneState::Failing { reason };
    }
}

/// Captured `Arc<ReadyLane>` values can outlive a failing state transition, so callers must not run a captured backend after the transition.
fn lane_failure_reason(inner: &SynapseInner) -> Option<String> {
    match &*inner.lock_state() {
        LaneState::Ready(_) => None,
        LaneState::Starting => Some(STARTING_REASON.to_owned()),
        LaneState::Disabled { reason } | LaneState::Failing { reason } => Some(reason.clone()),
    }
}

/// `STARTING_REASON` provides a fixed reason until activation settles the lane.
const STARTING_REASON: &str = "the synapse lane is still starting";

/// `BUSY_REASON` reports a `cpu` permit that a synchronous caller could not take without waiting.
const BUSY_REASON: &str = "the synapse lane is busy";

const SHUT_DOWN_REASON: &str = "the synapse lane is shut down";

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
    joined: Result<Result<Vec<Vec<f32>>, InferenceError>, impl Into<PanickedBackend>>,
) -> Result<Vec<Vec<f32>>, InferenceError> {
    match joined {
        Ok(Ok(vectors)) => Ok(vectors),
        Ok(Err(InferenceError::Invariant(reason))) => {
            mark_failing(inner, reason.clone());
            Err(InferenceError::Invariant(reason))
        }
        Ok(Err(other)) => Err(other),
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
    async fn handle_query(
        &self,
        ctx: &RequestCtx,
        lane: Arc<ReadyLane>,
        text: String,
        deadline_ms: Option<u64>,
        text_charge: crate::wire::ByteCharge,
    ) -> RequestOutcome {
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
        let deadline = tokio::time::Instant::now()
            + std::time::Duration::from_millis(
                deadline_ms.unwrap_or(protocol::DEFAULT_DEADLINE_MS),
            );
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
            let result = settle_inference(&inner, joined).map_err(QueryFault::Engine);
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
        // The CPU lane is idle once the verdict arrives, so the handler's admission slot is released before response reservation can wait on egress; the worker's copy still covers a native call that outlives its receiver.
        drop(handler_query_permit);
        match result {
            Ok(vectors) => match vectors.first() {
                Some(vector) => {
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
                None => {
                    mark_failing(
                        &self.inner,
                        "inference returned no vector for one query".to_owned(),
                    );
                    app_error("artifact_invalid", "inference returned no vector")
                }
            },
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
        if request_key != canonical_key {
            return app_error(
                "schema_violation",
                "request_key does not match the canonical payload",
            );
        }
        let retry_after_ms = self.inner.limits.retry_after_ms;
        match self
            .inner
            .jobs
            .admit_charged(request_key.clone(), items, lane.lane.dims, &mut charge)
        {
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
            let joined = tokio::task::spawn_blocking(move || {
                let texts: Vec<&str> = items.iter().map(|item| item.text.as_str()).collect();
                lane_blocking.backend.embed(&texts)
            })
            .await;
            match settle_inference(&inner, joined) {
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
                // The boundary becomes replayable only once a response carrying its cursor exists; a failed reservation leaves it never-issued.
                if let (RequestOutcome::Response { .. }, Some(boundary)) =
                    (&outcome, page.next_boundary)
                {
                    self.inner.jobs.mark_cursor_issued(&job_id, boundary);
                }
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
            // Retained result vectors live outside every `ByteCharge`; declaring their cap makes the runtime subtract it from ingress so a full retention set and a full ingress pool cannot coexist above `max_resident_bytes`.
            retained_resident_bytes: self.inner.limits.max_retained_result_bytes,
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
                self.handle_query(&ctx, lane, text, deadline_ms, text_charge)
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
                    "synapse_state": if reason == "synapse_unsupported" {
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

    /// Shutdown closes admission and cancels queued wrappers before joining every started native call through its incarnation.
    /// Shutdown never aborts a started native call.
    /// The lane ends `Disabled` so a late `bind`, `health`, or `embed_blocking` observes the shutdown instead of a ready lane whose admission is closed.
    /// `embed_blocking` calls are not tracked, so shutdown takes the CPU permit after disabling the lane: an in-flight blocking call holds that permit until it returns, and a call that read `Ready` earlier rechecks the state after acquiring it.
    async fn shutdown(&self) -> Result<(), crate::composite::ShutdownError> {
        self.inner.closing.cancel();
        self.inner.jobs.close_admission();
        // Closing `query_admission` before the tracker makes late queries observe cancellation instead of spawning workers into a draining tracker.
        self.inner.query_admission.close();
        self.inner.tracker.close();
        self.inner.tracker.wait().await;
        self.inner.jobs.clear();
        *self.inner.lock_state() = LaneState::Disabled {
            reason: SHUT_DOWN_REASON.to_owned(),
        };
        // Taking the permit joins an in-flight `embed_blocking` call.
        drop(self.inner.cpu.acquire().await);
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
        // Invalid limits fail activation rather than disabling the lane.
        if let Err(error) = bundle::validate_limits(&config.limits) {
            return Err(InitError(format!(
                "synapse limits are invalid: {}",
                error.0
            )));
        }
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

    struct NoopEngine;

    impl EmbeddingEngine for NoopEngine {
        fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, InferenceError> {
            Ok(texts.iter().map(|_| vec![1.0]).collect())
        }
    }

    fn lane() -> LaneInfo {
        LaneInfo {
            model: "m".to_owned(),
            fingerprint: "f".to_owned(),
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
                SynapseComponent::ready_with_engine(lane(), Arc::new(NoopEngine), limits)
                    .expect("limits validate");
            assert_eq!(
                component.resources().general_task_hold_bound,
                expected,
                "max_waiting_queries {max_waiting_queries}"
            );
        }
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
            Arc::new(NoopEngine),
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
            Arc::new(NoopEngine),
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
            lane_state_after_load(Ok(ReadyLane::new(Arc::new(NoopEngine), lane()))),
            LaneState::Ready(_)
        ));
    }
}
