//! Publishes the native message text and tool results of a transform window as kernel source descriptors from a background supervisor, after the transform response is handed to the wire writer.
//!
//! Capture runs when `source_capture.enabled` and `memory.enabled` both permit it in the route's consent snapshot and the session is not a subagent's; otherwise the adapters never run. Once the pass has built its response, the handler offers the pass's native records to [`SourceCapture::offer`], which charges the retained window to the supervisor's own byte budget. When the response is handed to the wire writer, [`CaptureHandoff::into_job`] takes the store's single publication slot; a refused offer or handoff (`refused_window_too_large`, `refused_resident_short`, `refused_queue_full`, `refused_shutting_down`) drops its reference and charge at once. The accepted job owns the slot permit, the charges, and its completion: it runs the adapters, charges their copies, skips units the memo already knows, and publishes at most one screened batch under a 5 s budget whose origin is the handoff's acceptance. The response bytes never wait on the kernel or the job.
//!
//! Kernel receipts are the durable record. The memo is a bounded, process-local cache keyed by the store's incarnation, the session, and each unit's receipt key; an entry holds the request digest, a disposition, and the time a pending unit first waited, never window bytes. A memo hit with an equal digest suppresses the kernel probe; after a restart the memo is empty and the kernel answers from its receipts. Pending units are offered ahead of units the memo does not know. A window refused at a full slot and never offered again is lost to capture; the next pass that carries it is captured.
//!
//! Logs and counters carry a closed vocabulary of stages and reasons with counts and byte totals, never unit text, paths, identities, or digests.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use host_runtime::wire::{ByteBudget, ByteCharge};
use kernel::applicability::EvalBudget;
use kernel::source_identity::OccurrenceClass;
use kernel::{CommitIntent, KernelError, KernelStore, ProviderEgress, Sensitivity};
use serde_json::{Value, json};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::harness_sources::{
    Harness, MAX_SOURCE_BATCH_BYTES, PublishError, SessionIdentity, SourcePublisher, SourceUnit,
    UnitDisposition, opencode_units, pi_units, publish_screened,
};
use crate::healing::SerializerProfile;
use crate::kernel_routes::{KernelOpenCoordinator, ProjectBinding};

/// The cooperative budget of one publication, from handoff acceptance.
pub(crate) const CAPTURE_BUDGET: Duration = Duration::from_secs(5);
/// The supervisor's byte budget: the retained window, the adapters' copies, and the batch's artifact preparation.
const MAX_RETAINED_WINDOW_BYTES: usize = 32 * 1024 * 1024;
/// Artifact preparation copies an admitted batch a bounded number of times.
const ARTIFACT_PREPARATION_BYTES: usize = 4 * MAX_SOURCE_BATCH_BYTES;
/// The memo's byte budget, counting each entry's two key copies, digest, and bookkeeping.
const MEMO_BUDGET_BYTES: usize = 4 * 1024 * 1024;
/// Bookkeeping bytes charged per memo entry beside its key copies and digest, including its table slot.
const MEMO_ENTRY_OVERHEAD_BYTES: usize = 192;
/// The supervisor's retained bytes: its byte budget plus the memo budget.
pub(crate) const RETAINED_BYTES_BOUND: u64 =
    MAX_RETAINED_WINDOW_BYTES as u64 + MEMO_BUDGET_BYTES as u64;

/// The domain every captured descriptor belongs to; one lineage lives in one domain.
const CAPTURE_DOMAIN_ID: &str = "harness-capture";
const PRODUCER: &str = "eidnara-daemon/source-capture";

/// Declares the closed vocabulary with each variant's report and log name.
macro_rules! counts {
    ($($variant:ident => $name:literal,)*) => {
        /// The closed vocabulary of counters, gauges, job ends, and log reasons; every value is a count, a byte total, or milliseconds.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub(crate) enum Count {
            $($variant,)*
        }

        impl Count {
            const ALL: [Self; [$($name),*].len()] = [$(Self::$variant),*];

            const fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $name,)*
                }
            }
        }
    };
}

counts! {
    Admitted => "admitted",
    RefusedQueueFull => "refused_queue_full",
    RefusedResidentShort => "refused_resident_short",
    RefusedWindowTooLarge => "refused_window_too_large",
    RefusedShuttingDown => "refused_shutting_down",
    Completed => "completed",
    Discarded => "discarded",
    SessionInvalidated => "session_invalidated",
    KernelUnavailable => "kernel_unavailable",
    Deadline => "deadline",
    KernelError => "kernel_error",
    WorkerPanicked => "worker_panicked",
    AdapterRefused => "adapter_refused",
    UnitsPublished => "units_published",
    UnitsReplayed => "units_replayed",
    UnitsMemoSkipped => "units_memo_skipped",
    UnitsDeferred => "units_deferred",
    UnitsUnitTooLarge => "units_unit_too_large",
    UnitsIdentityConflict => "units_identity_conflict",
    UnitsRefusedTerminal => "units_refused_terminal",
    UnitsRefusedRetryable => "units_refused_retryable",
    UnitsSuperseded => "units_superseded",
    Active => "active",
    RetainedBytes => "retained_bytes",
    PublishMsLast => "publish_ms_last",
    PublishMsMax => "publish_ms_max",
    ExpiryRetired => "expiry_retired",
    ExpiryRetained => "expiry_retained",
}

#[derive(Default)]
struct Counters([AtomicU64; Count::ALL.len()]);

impl Counters {
    fn at(&self, count: Count) -> &AtomicU64 {
        &self.0[count as usize]
    }

    fn add(&self, count: Count, value: usize) {
        self.at(count).fetch_add(value as u64, Ordering::Relaxed);
    }

    fn sub(&self, count: Count, value: usize) {
        self.at(count).fetch_sub(value as u64, Ordering::Relaxed);
    }

    fn load(&self, count: Count) -> u64 {
        self.at(count).load(Ordering::Relaxed)
    }
}

/// One log line from a fixed stage, a closed reason, and a count; nothing a unit carried reaches it.
fn log_line(stage: &'static str, reason: Count, count: usize) -> String {
    format!(
        "daemon: source capture {stage}: {} ({count})",
        reason.name()
    )
}

fn log(stage: &'static str, reason: Count, count: usize) {
    eprintln!("{}", log_line(stage, reason, count));
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Disposition {
    Published,
    Terminal,
    Pending { since_ms: i64 },
}

struct MemoEntry {
    digest: String,
    disposition: Disposition,
}

/// Bounded, byte-accounted, process-local; the kernel's receipts stay the only durable record.
#[derive(Default)]
struct Memo {
    entries: HashMap<String, MemoEntry>,
    order: VecDeque<String>,
    bytes: usize,
}

impl Memo {
    /// Each key lives in the map and in the eviction order.
    fn entry_bytes(key: &str, digest: &str) -> usize {
        2 * key.len() + digest.len() + MEMO_ENTRY_OVERHEAD_BYTES
    }

    fn get(&self, key: &str) -> Option<&MemoEntry> {
        self.entries.get(key)
    }

    fn record(&mut self, key: String, digest: String, disposition: Disposition) {
        let disposition = match self.entries.get(&key) {
            Some(previous) => {
                self.bytes -= Self::entry_bytes(&key, &previous.digest);
                // A deferral keeps the time the unit first waited.
                match (&previous.disposition, disposition) {
                    (Disposition::Pending { .. }, Disposition::Pending { .. })
                        if previous.digest == digest =>
                    {
                        previous.disposition.clone()
                    }
                    (_, disposition) => disposition,
                }
            }
            None => {
                self.order.push_back(key.clone());
                disposition
            }
        };
        self.bytes += Self::entry_bytes(&key, &digest);
        self.entries.insert(
            key,
            MemoEntry {
                digest,
                disposition,
            },
        );
        while self.bytes > MEMO_BUDGET_BYTES {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(evicted) = self.entries.remove(&oldest) {
                self.bytes -= Self::entry_bytes(&oldest, &evicted.digest);
            }
        }
    }

    fn oldest_pending_since(&self) -> Option<i64> {
        self.entries
            .values()
            .filter_map(|entry| match entry.disposition {
                Disposition::Pending { since_ms } => Some(since_ms),
                _ => None,
            })
            .min()
    }
}

/// The daemon's capture supervisor: one publication slot for its one kernel store, its byte budget, the memo, the delete epoch, and counters.
pub(crate) struct SourceCapture {
    slot: Arc<Semaphore>,
    budget: ByteBudget,
    memo: Mutex<Memo>,
    /// Bumped by every `session.delete` under the capture commit gate; a job taken at an older epoch publishes nothing, and the next pass offers its window again.
    delete_epoch: AtomicU64,
    counters: Counters,
    #[cfg(test)]
    pub(crate) before_publish: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// F23: when set, `session.delete` returns after the Kernel retirement commits and before the memory-store delete: an application-fault cut between the two commits.
    #[cfg(test)]
    pub(crate) stop_after_retirement: AtomicBool,
}

impl Default for SourceCapture {
    fn default() -> Self {
        Self {
            slot: Arc::new(Semaphore::new(1)),
            budget: ByteBudget::new(MAX_RETAINED_WINDOW_BYTES as u64),
            memo: Mutex::default(),
            delete_epoch: AtomicU64::new(0),
            counters: Counters::default(),
            #[cfg(test)]
            before_publish: Mutex::new(None),
            #[cfg(test)]
            stop_after_retirement: AtomicBool::new(false),
        }
    }
}

/// Whether the route's consent snapshot, the session, and the profile admit capture at all; otherwise no adapter runs. The profile's harness must be the one the route claims, so the harness capture stamps on a descriptor is the one `session.delete` retires by.
pub(crate) fn consents(
    config: &crate::config::DaemonConfig,
    is_subagent: bool,
    profile: SerializerProfile,
    route_harness: &str,
) -> bool {
    config.source_capture_enabled
        && config.memory_enabled
        && !is_subagent
        && profile.serves_native()
        && profile_harness(profile).is_some_and(|harness| harness.as_str() == route_harness)
}

/// The harness whose native records a serializer profile carries.
fn profile_harness(profile: SerializerProfile) -> Option<Harness> {
    match profile {
        SerializerProfile::Pi => Some(Harness::Pi),
        SerializerProfile::OpencodeAiSdk => Some(Harness::OpenCode),
        _ => None,
    }
}

/// A consented pass's native window, held by the pass until its response is built.
pub(crate) struct Offer {
    pub profile: SerializerProfile,
    pub session_id: String,
    pub project: ProjectBinding,
    pub window: Vec<Arc<Value>>,
    /// [`SourceCapture::epoch`] before the pass ran.
    pub epoch: u64,
}

/// A charged window waiting for its response to reach the wire writer. Dropping it releases the charge.
pub(crate) struct CaptureHandoff {
    capture: Arc<SourceCapture>,
    window: Vec<Arc<Value>>,
    session: SessionIdentity,
    project: ProjectBinding,
    epoch: u64,
    window_bytes: usize,
    retained: RetainedCharge,
}

/// A charge against the supervisor's budget, mirrored in the `retained_bytes` gauge while it lives.
struct RetainedCharge {
    capture: Arc<SourceCapture>,
    bytes: usize,
    _charge: ByteCharge,
}

impl RetainedCharge {
    fn take(capture: &Arc<SourceCapture>, bytes: usize) -> Option<Self> {
        let charge = capture.budget.try_charge(bytes)?;
        capture.counters.add(Count::RetainedBytes, bytes);
        Some(Self {
            capture: Arc::clone(capture),
            bytes,
            _charge: charge,
        })
    }
}

impl Drop for RetainedCharge {
    fn drop(&mut self) {
        self.capture.counters.sub(Count::RetainedBytes, self.bytes);
    }
}

/// Counts the slot as active while the permit lives, so a panicking job releases both.
struct ActiveSlot {
    capture: Arc<SourceCapture>,
    _permit: OwnedSemaphorePermit,
}

impl Drop for ActiveSlot {
    fn drop(&mut self) {
        self.capture.counters.sub(Count::Active, 1);
    }
}

impl SourceCapture {
    /// Charges a consented pass's native window for capture. `None` for an empty window, and when the window bound or the budget refuses it; a refusal is counted and retains nothing.
    pub(crate) fn offer(self: &Arc<Self>, offer: Offer) -> Option<CaptureHandoff> {
        let harness = profile_harness(offer.profile)?;
        if offer.window.is_empty() {
            return None;
        }
        let window_bytes: usize = offer
            .window
            .iter()
            .map(|value| crate::retained_size::value_retained_bytes(value))
            .sum();
        // The adapters copy about the window's bytes once more; the job tops up for any excess.
        let bytes = window_bytes
            .saturating_mul(2)
            .saturating_add(ARTIFACT_PREPARATION_BYTES);
        if bytes > MAX_RETAINED_WINDOW_BYTES {
            self.refuse(Count::RefusedWindowTooLarge);
            return None;
        }
        let Some(retained) = RetainedCharge::take(self, bytes) else {
            self.refuse(Count::RefusedResidentShort);
            return None;
        };
        Some(CaptureHandoff {
            capture: Arc::clone(self),
            window: offer.window,
            session: SessionIdentity {
                project_id: offer.project.digest().to_owned(),
                harness,
                session_id: offer.session_id,
            },
            project: offer.project,
            epoch: offer.epoch,
            window_bytes,
            retained,
        })
    }

    /// Whether no job holds the slot.
    #[cfg(test)]
    pub(crate) fn is_idle(&self) -> bool {
        self.slot.available_permits() == 1
    }

    /// Empties the memo, as a restart does.
    #[cfg(test)]
    pub(crate) fn forget_memo_for_test(&self) {
        *self.memo() = Memo::default();
    }

    pub(crate) fn refuse(&self, refusal: Count) {
        self.counters.add(refusal, 1);
        log("handoff refused", refusal, 1);
    }

    /// A poisoned memo is a lost cache: it starts empty and the kernel's receipts answer.
    fn memo(&self) -> MutexGuard<'_, Memo> {
        self.memo.lock().unwrap_or_else(|poisoned| {
            self.memo.clear_poison();
            let mut memo = poisoned.into_inner();
            *memo = Memo::default();
            memo
        })
    }

    /// The delete epoch a pass samples before it runs.
    pub(crate) fn epoch(&self) -> u64 {
        self.delete_epoch.load(Ordering::Acquire)
    }

    /// Invalidates every handoff whose pass began before now and empties the memo, so the next pass of any session asks the kernel, as after a restart; the caller holds the capture commit gate, so a job that rechecks under it sees the change.
    pub(crate) fn invalidate_sessions(&self) {
        self.delete_epoch.fetch_add(1, Ordering::AcqRel);
        *self.memo() = Memo::default();
    }

    /// Adds one Kernel maintenance pass's harness-capture expiry progress: captures retired outright, and captures whose descriptors were retired while a citation kept their evidence.
    pub(crate) fn record_expiry(&self, expiry: kernel::CaptureExpiry) {
        self.counters.add(Count::ExpiryRetired, expiry.retired);
        self.counters.add(Count::ExpiryRetained, expiry.retained);
    }

    /// The observability set: every [`Count`] by name, the active slot reported as draining once shutdown began, and the age of the oldest pending unit.
    pub(crate) fn report(&self, draining: bool, now_ms: i64) -> Value {
        let mut report = serde_json::Map::new();
        for count in Count::ALL
            .into_iter()
            .filter(|count| *count != Count::Active)
        {
            report.insert(count.name().to_owned(), json!(self.counters.load(count)));
        }
        let active = self.counters.load(Count::Active);
        report.insert(
            "active".to_owned(),
            json!(if draining { 0 } else { active }),
        );
        report.insert(
            "draining".to_owned(),
            json!(if draining { active } else { 0 }),
        );
        let oldest = self
            .memo()
            .oldest_pending_since()
            .map(|since| now_ms.saturating_sub(since).max(0));
        report.insert("oldest_deferred_age_ms".to_owned(), json!(oldest));
        Value::Object(report)
    }
}

impl CaptureHandoff {
    /// The supervisor this handoff charges.
    pub(crate) fn supervisor(&self) -> Arc<SourceCapture> {
        Arc::clone(&self.capture)
    }

    /// Takes the store's publication slot and starts the job's 5 s budget. `None` when a batch already holds the slot; the refusal is counted and the handoff's charge released.
    pub(crate) fn into_job(
        self,
        kernel: Arc<KernelOpenCoordinator>,
        commit_gate: Arc<Mutex<()>>,
        cancel: CancellationToken,
    ) -> Option<CaptureJob> {
        let Ok(permit) = Arc::clone(&self.capture.slot).try_acquire_owned() else {
            self.capture.refuse(Count::RefusedQueueFull);
            return None;
        };
        self.capture.counters.add(Count::Active, 1);
        let interrupt = Arc::new(AtomicBool::new(false));
        Some(CaptureJob {
            budget: EvalBudget::new(
                Some(Instant::now() + CAPTURE_BUDGET),
                Arc::clone(&interrupt),
            ),
            interrupt,
            kernel,
            commit_gate,
            cancel,
            _slot: ActiveSlot {
                capture: Arc::clone(&self.capture),
                _permit: permit,
            },
            _retained: self.retained,
            capture: self.capture,
            window: self.window,
            session: self.session,
            project: self.project,
            epoch: self.epoch,
            window_bytes: self.window_bytes,
        })
    }
}

/// One accepted capture: the blocking closure owns the slot, the charges, and the window until it returns.
pub(crate) struct CaptureJob {
    capture: Arc<SourceCapture>,
    budget: EvalBudget,
    interrupt: Arc<AtomicBool>,
    kernel: Arc<KernelOpenCoordinator>,
    commit_gate: Arc<Mutex<()>>,
    cancel: CancellationToken,
    _slot: ActiveSlot,
    _retained: RetainedCharge,
    window: Vec<Arc<Value>>,
    session: SessionIdentity,
    project: ProjectBinding,
    epoch: u64,
    window_bytes: usize,
}

impl CaptureJob {
    /// Runs the publication on the blocking pool and returns once it physically completes. Cancellation interrupts the budget, so the job stops before its next kernel wait; the wait for completion is then bounded by the budget, the capture commit gate's current holder, and the adapters' work on the window.
    pub(crate) async fn run(self) -> Count {
        self.capture.counters.add(Count::Admitted, 1);
        let interrupt = Arc::clone(&self.interrupt);
        let cancel = self.cancel.clone();
        let capture = Arc::clone(&self.capture);
        let mut join = tokio::task::spawn_blocking(move || self.publish());
        let joined = tokio::select! {
            joined = &mut join => joined,
            () = cancel.cancelled() => {
                interrupt.store(true, Ordering::Release);
                join.await
            }
        };
        // A failed join dropped the slot, the charges, and the window while the closure unwound.
        let end = match joined {
            Ok(end) => return end,
            Err(error) if error.is_panic() => Count::WorkerPanicked,
            Err(_) => Count::Discarded,
        };
        capture.counters.add(end, 1);
        log("job ended", end, 1);
        end
    }

    fn end(&self, end: Count) -> Count {
        self.capture.counters.add(end, 1);
        if end != Count::Completed {
            log("job ended", end, 1);
        }
        end
    }

    fn publish(self) -> Count {
        if self.cancel.is_cancelled() {
            return self.end(Count::Discarded);
        }
        let Ok(store) = self.kernel.kernel_store() else {
            return self.end(Count::KernelUnavailable);
        };
        if self.budget.is_exhausted() {
            return self.end(Count::Deadline);
        }
        let units = self.units();
        // The adapters' copies beyond the window charge are charged as soon as they exist.
        let copies: usize = units
            .iter()
            .map(|unit| {
                size_of::<SourceUnit>()
                    + unit.text.len()
                    + unit.role.len()
                    + unit.revision.len()
                    + unit
                        .identity
                        .iter()
                        .map(|(_, value)| 48 + value.len())
                        .sum::<usize>()
            })
            .sum();
        let Some(_copies) =
            RetainedCharge::take(&self.capture, copies.saturating_sub(self.window_bytes))
        else {
            return self.end(Count::RefusedResidentShort);
        };
        if units.is_empty() {
            return self.end(Count::Completed);
        }
        let incarnation = match store.database_incarnation_id_within_budget(&self.budget) {
            Ok(incarnation) => incarnation,
            Err(error) => return self.end(kernel_end(error)),
        };
        let scope_id = self.project.scope_id();
        let publisher = SourcePublisher {
            kernel: &store,
            domain_id: CAPTURE_DOMAIN_ID,
            scope_id: Some(&scope_id),
            egress: ProviderEgress::LocalOnly,
            sensitivity: Sensitivity::Sensitive,
        };
        let prefix = format!(
            "{incarnation}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}",
            self.session.project_id,
            self.session.harness.as_str(),
            self.session.session_id
        );
        let identities: Vec<Option<(String, String)>> = units
            .iter()
            .map(|unit| {
                let (key, digest) = publisher.receipt_identity(unit)?;
                Some((format!("{prefix}{key}"), digest))
            })
            .collect();
        // Pending units go first, then units the memo does not know; a known unit with an equal digest, and a repeat within the window, is skipped.
        let (mut pending, mut unknown) = (Vec::new(), Vec::new());
        {
            let memo = self.capture.memo();
            let mut seen = HashSet::new();
            for (unit, identity) in units.into_iter().zip(identities) {
                let entry = identity.as_ref().and_then(|(key, _)| memo.get(key));
                let fresh = identity
                    .as_ref()
                    .is_none_or(|(key, _)| seen.insert(key.clone()));
                match entry {
                    _ if !fresh => {}
                    Some(entry) if entry.digest == identity.as_ref().expect("keyed").1 => {
                        if matches!(entry.disposition, Disposition::Pending { .. }) {
                            pending.push((unit, identity));
                        } else {
                            self.capture.counters.add(Count::UnitsMemoSkipped, 1);
                        }
                    }
                    _ => unknown.push((unit, identity)),
                }
            }
        }
        let (candidates, keys): (Vec<SourceUnit>, Vec<Option<(String, String)>>) =
            pending.into_iter().chain(unknown).unzip();
        if candidates.is_empty() {
            return self.end(Count::Completed);
        }
        #[cfg(test)]
        let hook = self
            .capture
            .before_publish
            .lock()
            .expect("capture hook mutex")
            .clone();
        #[cfg(test)]
        if let Some(hook) = hook {
            hook();
        }
        if self.cancel.is_cancelled() {
            return self.end(Count::Discarded);
        }
        // A panic while the shared gate is held would poison it for session deletion; it is caught here and resumed once the guard is released.
        let gate = self.commit_gate.lock().expect("capture publication mutex");
        let published = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.publish_locked(&store, &publisher, &candidates, keys)
        }));
        drop(gate);
        match published {
            Ok(end) => end,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }

    /// Publishes under the capture commit gate once the delete epoch still holds; `keys[i]` is the memo key of `candidates[i]`.
    fn publish_locked(
        &self,
        store: &KernelStore,
        publisher: &SourcePublisher<'_>,
        candidates: &[SourceUnit],
        keys: Vec<Option<(String, String)>>,
    ) -> Count {
        if self.capture.delete_epoch.load(Ordering::Acquire) != self.epoch {
            return self.end(Count::SessionInvalidated);
        }
        if let Err(error) = self.ensure_scope(store) {
            return self.end(kernel_end(error));
        }
        if self.budget.is_exhausted() {
            return self.end(Count::Deadline);
        }
        let observed_at = crate::now_ms();
        let started = Instant::now();
        let result = publish_screened(publisher, candidates, observed_at, Some(&self.budget));
        let held = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let counters = &self.capture.counters;
        counters
            .at(Count::PublishMsLast)
            .store(held, Ordering::Relaxed);
        counters
            .at(Count::PublishMsMax)
            .fetch_max(held, Ordering::Relaxed);
        let mut memo = self.capture.memo();
        match result {
            Ok(dispositions) => {
                for (disposition, key) in dispositions.into_iter().zip(keys) {
                    let (count, recorded) = classify(disposition, observed_at);
                    counters.add(count, 1);
                    if let Some((key, digest)) = key {
                        memo.record(key, digest, recorded);
                    }
                }
                drop(memo);
                self.end(Count::Completed)
            }
            Err(refusal) => {
                // A same-key conflict is terminal for its unit; every other unit stays pending for the next pass.
                if let Some(index) = refusal.conflicting_unit()
                    && let Some(Some((key, digest))) = keys.get(index)
                {
                    counters.add(Count::UnitsIdentityConflict, 1);
                    memo.record(key.clone(), digest.clone(), Disposition::Terminal);
                    log("unit terminal", Count::UnitsIdentityConflict, 1);
                }
                drop(memo);
                self.end(kernel_end(refusal.error))
            }
        }
    }

    /// Every unit of the window, in record order; a record the adapter refuses contributes none and is counted.
    fn units(&self) -> Vec<SourceUnit> {
        let mut units = Vec::new();
        let mut refused = 0;
        for record in &self.window {
            let adapted = match self.session.harness {
                Harness::OpenCode => opencode_units(&self.session, record),
                Harness::Pi => pi_units(&self.session, record),
            };
            match adapted {
                Ok(adapted) => units.extend(adapted),
                Err(_) => refused += 1,
            }
        }
        if refused > 0 {
            self.capture.counters.add(Count::AdapterRefused, refused);
            log("adapter", Count::AdapterRefused, refused);
        }
        units
    }

    /// Materializes the capture domain and the project's scope; the receipt makes every later attempt a replay.
    fn ensure_scope(&self, store: &KernelStore) -> Result<(), KernelError> {
        let operation_key = format!("{}:source-capture:scope", self.project.digest());
        let intent = CommitIntent {
            producer: PRODUCER.to_owned(),
            request_digest: kernel::source_identity::identity_digest(operation_key.as_bytes()),
            operation_key,
            actor: self.session.harness.as_str().to_owned(),
            cause: "source capture scope".to_owned(),
        };
        let project = &self.project;
        store.commit_within_budget(&self.budget, intent, |envelope| {
            let mut domains = HashSet::new();
            crate::kernel_routes::commit::ensure_domain(envelope, CAPTURE_DOMAIN_ID, &mut domains)?;
            let (mut ready, mut refused) = (false, None);
            crate::kernel_routes::commit::ensure_scope(
                envelope,
                project,
                &mut ready,
                &mut domains,
                &mut refused,
            )?;
            Ok(String::new())
        })?;
        Ok(())
    }
}

/// A unit's counter and the memo disposition its outcome leaves: a publication or replay is known, a size, conflict, or permanent refusal is terminal, and a deferral or retryable refusal waits for the next pass.
fn classify(disposition: UnitDisposition, observed_at: i64) -> (Count, Disposition) {
    let pending = Disposition::Pending {
        since_ms: observed_at,
    };
    match disposition {
        UnitDisposition::Published(published) if published.replayed => {
            (Count::UnitsReplayed, Disposition::Published)
        }
        UnitDisposition::Published(_) => (Count::UnitsPublished, Disposition::Published),
        UnitDisposition::UnitTooLarge => (Count::UnitsUnitTooLarge, Disposition::Terminal),
        UnitDisposition::Deferred => (Count::UnitsDeferred, pending),
        UnitDisposition::Refused(PublishError::IdentityReused) => {
            (Count::UnitsIdentityConflict, Disposition::Terminal)
        }
        UnitDisposition::Refused(refusal) if refusal.is_permanent() => {
            (Count::UnitsRefusedTerminal, Disposition::Terminal)
        }
        UnitDisposition::Refused(_) => (Count::UnitsRefusedRetryable, pending),
        UnitDisposition::Superseded => (Count::UnitsSuperseded, Disposition::Terminal),
    }
}

/// Live descriptors one session-retirement page retires.
pub(crate) const SESSION_RETIREMENT_PAGE: usize = 256;
/// How long a session-retirement preview waits for a pooled reader.
const SESSION_RETIREMENT_WAIT: Duration = Duration::from_secs(5);
/// Leads the bytes a session-retirement page's receipt key binds.
const SESSION_RETIREMENT_FORMAT: &str = "source-session-retire.v1";

/// One captured session: the identity fields its `messages` and `raw_tool_spans` descriptors name.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CapturedSession<'a> {
    pub(crate) project_id: &'a str,
    pub(crate) harness: &'a str,
    pub(crate) session_id: &'a str,
}

/// Retires every live `messages` and `raw_tool_spans` descriptor of `session`, at most `pages` pages of at most `page_rows` descriptors, and returns whether none remains live. Each page is one commit that retires the rows its inventory listed, so a retry inventories again and finds only what is still live. A page's receipt is keyed by the session and the page's sorted object ids, never the clock ([`session_page_intent`]); a page that replays means the inventory listed rows a committed page retired, which is [`KernelError::Conflict`]. The caller holds the capture commit gate, so no capture publication interleaves; the maintenance expiry sweep can retire listed rows first, and the page then retires the rest.
///
/// # Errors
///
/// Returns the first inventory or commit error; pages committed before it stay retired.
pub(crate) fn retire_session_pages(
    store: &KernelStore,
    session: CapturedSession<'_>,
    page_rows: std::num::NonZeroUsize,
    pages: usize,
) -> Result<bool, KernelError> {
    for _ in 0..pages {
        let (_, page) = store.preview(Instant::now() + SESSION_RETIREMENT_WAIT, |preview| {
            session_page(preview, session, page_rows)
        })?;
        if page.is_empty() {
            return Ok(true);
        }
        let receipt = store.commit(session_page_intent(session, &page), |envelope| {
            crate::harness_sources::retire_descriptors(envelope, &page)
        })?;
        if receipt.replayed {
            return Err(KernelError::Conflict);
        }
    }
    Ok(false)
}

/// The receipt of one session-retirement page: keyed and fingerprinted by the format tag, the session's identity fields, and the page's sorted object ids.
pub(crate) fn session_page_intent(session: CapturedSession<'_>, page: &[String]) -> CommitIntent {
    let mut bytes = Vec::new();
    for part in [
        SESSION_RETIREMENT_FORMAT,
        session.project_id,
        session.harness,
        session.session_id,
    ]
    .into_iter()
    .chain(page.iter().map(String::as_str))
    {
        bytes.extend_from_slice(&(part.len() as u64).to_be_bytes());
        bytes.extend_from_slice(part.as_bytes());
    }
    let digest = kernel::source_identity::identity_digest(&bytes);
    CommitIntent {
        producer: PRODUCER.to_owned(),
        operation_key: format!("source-session-retire:{digest}"),
        request_digest: digest,
        actor: session.harness.to_owned(),
        cause: "session deleted".to_owned(),
    }
}

/// Up to `page_rows` live captured descriptors of `session`, sorted by object id.
fn session_page(
    preview: &kernel::Envelope<'_>,
    session: CapturedSession<'_>,
    page_rows: std::num::NonZeroUsize,
) -> Result<Vec<String>, KernelError> {
    let mut page = Vec::new();
    for class in [OccurrenceClass::Messages, OccurrenceClass::RawToolSpans] {
        let mut after = None;
        while let Some(room) = std::num::NonZeroUsize::new(page_rows.get() - page.len()) {
            let rows = preview.live_source_descriptors_with(
                class,
                "session_id",
                &[session.session_id],
                after.as_deref(),
                room,
            )?;
            page.extend(
                rows.rows
                    .iter()
                    .filter(|row| {
                        crate::git_reconcile::identity_field(row, "project_id")
                            == Some(session.project_id)
                            && crate::git_reconcile::identity_field(row, "harness")
                                == Some(session.harness)
                    })
                    .map(|row| row.object_id.clone()),
            );
            match rows.next {
                Some(next) => after = Some(next),
                None => break,
            }
        }
    }
    page.sort();
    Ok(page)
}

fn kernel_end(error: KernelError) -> Count {
    match error {
        KernelError::Deadline => Count::Deadline,
        _ => Count::KernelError,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every log line is built from a fixed stage, a closed reason, and a count, so no input byte can reach it.
    #[test]
    fn log_lines_carry_only_fixed_codes_and_counts() {
        for reason in Count::ALL {
            let line = log_line("handoff refused", reason, 1);
            assert!(
                line.bytes()
                    .all(|byte| byte.is_ascii_lowercase() || b" :_()1".contains(&byte)),
                "{line}"
            );
        }
    }

    #[test]
    fn the_memo_stays_within_its_byte_budget_and_keeps_the_first_deferral() {
        let mut memo = Memo::default();
        memo.record("k".into(), "d".into(), Disposition::Pending { since_ms: 5 });
        memo.record("k".into(), "d".into(), Disposition::Pending { since_ms: 9 });
        assert_eq!(memo.oldest_pending_since(), Some(5));
        memo.record("k".into(), "d".into(), Disposition::Published);
        assert_eq!(memo.oldest_pending_since(), None);
        for index in 0..100_000 {
            memo.record(
                format!("key-{index:064}"),
                "d".repeat(64),
                Disposition::Published,
            );
        }
        assert!(memo.bytes <= MEMO_BUDGET_BYTES);
        assert_eq!(
            memo.bytes,
            memo.entries
                .iter()
                .map(|(key, entry)| Memo::entry_bytes(key, &entry.digest))
                .sum::<usize>()
        );
        assert!(memo.get("k").is_none(), "the oldest entries were evicted");
    }
}
