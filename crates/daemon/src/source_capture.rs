//! Publishes the native message text and tool results of a transform window as kernel source descriptors from a background supervisor, after the transform response is handed to the wire writer.
//!
//! Capture runs when `source_capture.enabled` and `memory.enabled` both permit it in the route's consent snapshot and the session is not a subagent's; otherwise the adapters never run. The handler offers the pass's native records to [`SourceCapture::offer`], which holds the store's single publication slot and charges the retained window before any reference outlives the response. A refused offer (`queue_full`, `resident_short`, `window_too_large`) drops its reference and charge at once. The accepted handoff becomes a tracked blocking job that owns the slot permit, the charge, and its completion: it runs the adapters, skips units the memo already knows, and publishes at most one screened batch under a 5 s budget whose origin is the handoff's acceptance. The response bytes never wait on the kernel or the job.
//!
//! Kernel receipts are the durable record. The memo is a bounded, process-local cache keyed by the store's incarnation, the session, and each unit's receipt key; an entry holds the request digest, a disposition, and the time a pending unit was first deferred, never window bytes. A memo hit with an equal digest suppresses the kernel probe; after a restart the memo is empty and the kernel answers from its receipts. A window refused at a full slot and never offered again is lost to capture; the next pass that carries it is captured.
//!
//! Logs and counters carry a closed vocabulary of stages and reasons with counts and byte totals, never unit text, paths, identities, or digests.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use host_runtime::wire::ByteCharge;
use kernel::applicability::EvalBudget;
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
use crate::metered_decode::ResidentReserve;

/// The cooperative budget of one publication, from handoff acceptance.
pub(crate) const CAPTURE_BUDGET: Duration = Duration::from_secs(5);
/// The largest window charge a handoff may hold past its response.
const MAX_RETAINED_WINDOW_BYTES: usize = 32 * 1024 * 1024;
/// The memo's byte budget, counting each entry's key, digest, and bookkeeping.
const MEMO_BUDGET_BYTES: usize = 4 * 1024 * 1024;
/// Bookkeeping bytes charged per memo entry beside its key and digest.
const MEMO_ENTRY_OVERHEAD_BYTES: usize = 160;
/// The most project scopes the supervisor remembers as materialized.
const MAX_READY_SCOPES: usize = 256;
/// What one offer charges: the retained window, the unit copies the adapters make of it, and the batch's artifact preparation.
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
    Active => "active",
    RetainedBytes => "retained_bytes",
    WriterHoldMsLast => "writer_hold_ms_last",
    WriterHoldMsMax => "writer_hold_ms_max",
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
    fn entry_bytes(key: &str, digest: &str) -> usize {
        key.len() + digest.len() + MEMO_ENTRY_OVERHEAD_BYTES
    }

    fn get(&self, key: &str) -> Option<&MemoEntry> {
        self.entries.get(key)
    }

    fn record(&mut self, key: String, digest: String, disposition: Disposition) {
        let bytes = Self::entry_bytes(&key, &digest);
        if let Some(previous) = self.entries.get(&key) {
            self.bytes -= Self::entry_bytes(&key, &previous.digest);
        } else {
            self.order.push_back(key.clone());
        }
        // A deferral keeps the time the unit first waited.
        let disposition = match (self.entries.get(&key), disposition) {
            (Some(previous), Disposition::Pending { .. })
                if previous.digest == digest
                    && matches!(previous.disposition, Disposition::Pending { .. }) =>
            {
                previous.disposition.clone()
            }
            (_, disposition) => disposition,
        };
        self.bytes += bytes;
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

/// The daemon's capture supervisor: one publication slot for its one kernel store, the memo, session generations, and counters.
pub(crate) struct SourceCapture {
    slot: Arc<Semaphore>,
    memo: Mutex<Memo>,
    /// The generation of each session a delete invalidated; an absent session is at generation 0.
    generations: Mutex<HashMap<String, u64>>,
    /// `(incarnation, project)` pairs whose capture domain and scope exist.
    ready_scopes: Mutex<HashSet<(String, String)>>,
    counters: Counters,
    #[cfg(test)]
    pub(crate) before_publish: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl Default for SourceCapture {
    fn default() -> Self {
        Self {
            slot: Arc::new(Semaphore::new(1)),
            memo: Mutex::default(),
            generations: Mutex::default(),
            ready_scopes: Mutex::default(),
            counters: Counters::default(),
            #[cfg(test)]
            before_publish: Mutex::new(None),
        }
    }
}

/// What an offer needs from the pass.
pub(crate) struct Offer<'a> {
    pub source_capture_enabled: bool,
    pub memory_enabled: bool,
    pub is_subagent: bool,
    pub profile: SerializerProfile,
    pub session_id: &'a str,
    pub project: &'a ProjectBinding,
    pub window: Option<&'a [Arc<Value>]>,
}

/// A window accepted for capture once its response is handed to the wire writer. Dropping it releases the slot and the charge.
pub(crate) struct CaptureHandoff {
    capture: Arc<SourceCapture>,
    window: Vec<Arc<Value>>,
    session: SessionIdentity,
    project: ProjectBinding,
    generation: u64,
    retained: RetainedCharge,
    permit: OwnedSemaphorePermit,
}

/// The resident charge of a retained window, mirrored in the `retained_bytes` gauge while it lives.
struct RetainedCharge {
    capture: Arc<SourceCapture>,
    bytes: usize,
    _charge: ByteCharge,
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
    /// Offers a pass's native window. `None` when consent, the profile, or an empty window leaves nothing to capture, and when the slot, the resident pool, or the window bound refuses it; a refusal is counted and retains nothing.
    pub(crate) fn offer(
        self: &Arc<Self>,
        offer: Offer<'_>,
        reserve: &dyn ResidentReserve,
    ) -> Option<CaptureHandoff> {
        if !offer.source_capture_enabled || !offer.memory_enabled || offer.is_subagent {
            return None;
        }
        let harness = match offer.profile {
            SerializerProfile::OpencodeAiSdk => Harness::OpenCode,
            SerializerProfile::Pi => Harness::Pi,
            _ => return None,
        };
        let window = offer.window.filter(|window| !window.is_empty())?;
        let Ok(permit) = Arc::clone(&self.slot).try_acquire_owned() else {
            self.refuse(Count::RefusedQueueFull);
            return None;
        };
        let window_bytes: usize = window
            .iter()
            .map(|value| crate::retained_size::value_retained_bytes(value))
            .sum();
        // The adapters copy every text and identity out of the window once; artifact preparation copies an admitted batch a bounded number of times.
        let bytes = window_bytes
            .saturating_mul(2)
            .saturating_add(4 * MAX_SOURCE_BATCH_BYTES);
        if bytes > MAX_RETAINED_WINDOW_BYTES {
            self.refuse(Count::RefusedWindowTooLarge);
            return None;
        }
        let Some(charge) = reserve.try_reserve(bytes) else {
            self.refuse(Count::RefusedResidentShort);
            return None;
        };
        self.counters.add(Count::RetainedBytes, bytes);
        Some(CaptureHandoff {
            capture: Arc::clone(self),
            window: window.to_vec(),
            session: SessionIdentity {
                project_id: offer.project.digest().to_owned(),
                harness,
                session_id: offer.session_id.to_owned(),
            },
            project: offer.project.clone(),
            generation: self.generation(offer.session_id),
            retained: RetainedCharge {
                capture: Arc::clone(self),
                bytes,
                _charge: charge,
            },
            permit,
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
        *self.memo.lock().expect("capture memo mutex") = Memo::default();
    }

    fn refuse(&self, refusal: Count) {
        self.counters.add(refusal, 1);
        log("handoff refused", refusal, 1);
    }

    fn generation(&self, session: &str) -> u64 {
        self.generations
            .lock()
            .expect("capture generations mutex")
            .get(session)
            .copied()
            .unwrap_or(0)
    }

    /// Invalidates every handoff of `session` taken before now; the caller holds the capture commit gate, so a job that rechecks under it sees the change.
    pub(crate) fn invalidate_session(&self, session: &str) {
        *self
            .generations
            .lock()
            .expect("capture generations mutex")
            .entry(session.to_owned())
            .or_insert(0) += 1;
    }

    /// The observability set: every [`Count`] by name, with the active slot reported as draining once shutdown began, and the age of the oldest deferred unit.
    pub(crate) fn report(&self, draining: bool, now_ms: i64) -> Value {
        let mut report = serde_json::Map::new();
        for count in Count::ALL {
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
            .memo
            .lock()
            .expect("capture memo mutex")
            .oldest_pending_since()
            .map(|since| now_ms.saturating_sub(since).max(0));
        report.insert("oldest_deferred_age_ms".to_owned(), json!(oldest));
        Value::Object(report)
    }
}

impl CaptureHandoff {
    /// The tracked job for this handoff. Its 5 s budget starts now; `cancel` raises the budget's interrupt, and the job stops before its next kernel wait.
    pub(crate) fn into_job(
        self,
        kernel: Arc<KernelOpenCoordinator>,
        commit_gate: Arc<Mutex<()>>,
        cancel: CancellationToken,
    ) -> CaptureJob {
        self.capture.counters.add(Count::Admitted, 1);
        self.capture.counters.add(Count::Active, 1);
        let interrupt = Arc::new(AtomicBool::new(false));
        CaptureJob {
            budget: EvalBudget::new(
                Some(Instant::now() + CAPTURE_BUDGET),
                Arc::clone(&interrupt),
            ),
            interrupt,
            kernel,
            commit_gate,
            cancel,
            slot: ActiveSlot {
                capture: Arc::clone(&self.capture),
                _permit: self.permit,
            },
            retained: self.retained,
            capture: self.capture,
            window: self.window,
            session: self.session,
            project: self.project,
            generation: self.generation,
        }
    }

    /// Counts a handoff whose job could not be admitted because shutdown closed admission; dropping it releases the slot and the charge.
    pub(crate) fn refuse_shutting_down(self) {
        self.capture.refuse(Count::RefusedShuttingDown);
    }
}

/// One accepted capture: the blocking closure owns the slot, the charge, and the window until it returns.
pub(crate) struct CaptureJob {
    capture: Arc<SourceCapture>,
    budget: EvalBudget,
    interrupt: Arc<AtomicBool>,
    kernel: Arc<KernelOpenCoordinator>,
    commit_gate: Arc<Mutex<()>>,
    cancel: CancellationToken,
    slot: ActiveSlot,
    retained: RetainedCharge,
    window: Vec<Arc<Value>>,
    session: SessionIdentity,
    project: ProjectBinding,
    generation: u64,
}

impl CaptureJob {
    /// Runs the publication on the blocking pool and returns once it physically completes; cancellation interrupts its budget and still waits for the closure.
    pub(crate) async fn run(self) -> Count {
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
        match joined {
            Ok(end) => end,
            Err(_) => {
                // The slot, the charge, and the window were dropped while the panic unwound.
                capture.counters.add(Count::WorkerPanicked, 1);
                log("job ended", Count::WorkerPanicked, 1);
                Count::WorkerPanicked
            }
        }
    }

    fn end(&self, end: Count) -> Count {
        self.capture.counters.add(end, 1);
        if end != Count::Completed {
            log("job ended", end, 1);
        }
        end
    }

    fn publish(self) -> Count {
        let _slot = &self.slot;
        let _retained = &self.retained;
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
        let memo_prefix = format!(
            "{incarnation}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}",
            self.session.project_id,
            self.session.harness.as_str(),
            self.session.session_id
        );
        let mut candidates = Vec::new();
        let mut keys = Vec::new();
        {
            let memo = self.capture.memo.lock().expect("capture memo mutex");
            for unit in units {
                let Some((key, digest)) = publisher.receipt_identity(&unit) else {
                    // The screen refuses the unit with its encoding refusal.
                    keys.push(None);
                    candidates.push(unit);
                    continue;
                };
                let key = format!("{memo_prefix}{key}");
                match memo.get(&key) {
                    Some(entry)
                        if entry.digest == digest
                            && !matches!(entry.disposition, Disposition::Pending { .. }) =>
                    {
                        self.capture.counters.add(Count::UnitsMemoSkipped, 1);
                    }
                    _ => {
                        keys.push(Some((key, digest)));
                        candidates.push(unit);
                    }
                }
            }
        }
        if candidates.is_empty() {
            return self.end(Count::Completed);
        }
        // A panic while the shared gate is held would poison it for session deletion; it is caught here and resumed once the guard is released.
        let gate = self.commit_gate.lock().expect("capture publication mutex");
        let published = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.publish_locked(&store, &publisher, &incarnation, candidates, keys)
        }));
        drop(gate);
        match published {
            Ok(end) => end,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }

    /// Publishes under the capture commit gate, after the session's ticket still holds.
    fn publish_locked(
        &self,
        store: &KernelStore,
        publisher: &SourcePublisher<'_>,
        incarnation: &str,
        candidates: Vec<SourceUnit>,
        keys: Vec<Option<(String, String)>>,
    ) -> Count {
        if self.capture.generation(&self.session.session_id) != self.generation {
            return self.end(Count::SessionInvalidated);
        }
        if let Err(error) = self.ensure_scope(store, incarnation) {
            return self.end(kernel_end(error));
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
        if self.budget.is_exhausted() {
            return self.end(Count::Deadline);
        }
        let observed_at = crate::now_ms();
        let started = Instant::now();
        let result = publish_screened(publisher, &candidates, observed_at, Some(&self.budget));
        let held = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let counters = &self.capture.counters;
        counters
            .at(Count::WriterHoldMsLast)
            .store(held, Ordering::Relaxed);
        counters
            .at(Count::WriterHoldMsMax)
            .fetch_max(held, Ordering::Relaxed);
        let mut memo = self.capture.memo.lock().expect("capture memo mutex");
        match result {
            Ok(dispositions) => {
                let mut deferred = 0;
                for (disposition, key) in dispositions.into_iter().zip(keys) {
                    let recorded = match disposition {
                        UnitDisposition::Published(published) => {
                            counters.add(
                                if published.replayed {
                                    Count::UnitsReplayed
                                } else {
                                    Count::UnitsPublished
                                },
                                1,
                            );
                            Disposition::Published
                        }
                        UnitDisposition::UnitTooLarge => {
                            counters.add(Count::UnitsUnitTooLarge, 1);
                            Disposition::Terminal
                        }
                        UnitDisposition::Deferred => {
                            deferred += 1;
                            counters.add(Count::UnitsDeferred, 1);
                            Disposition::Pending {
                                since_ms: observed_at,
                            }
                        }
                        UnitDisposition::Refused(refusal) => self.refused(&refusal, observed_at),
                    };
                    if let Some((key, digest)) = key {
                        memo.record(key, digest, recorded);
                    }
                }
                if deferred > 0 {
                    log("batch bounded", Count::UnitsDeferred, deferred);
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

    fn refused(&self, refusal: &PublishError, observed_at: i64) -> Disposition {
        let (count, disposition) = if matches!(refusal, PublishError::IdentityReused) {
            (Count::UnitsIdentityConflict, Disposition::Terminal)
        } else if refusal.is_permanent() {
            (Count::UnitsRefusedTerminal, Disposition::Terminal)
        } else {
            let since_ms = observed_at;
            (
                Count::UnitsRefusedRetryable,
                Disposition::Pending { since_ms },
            )
        };
        self.capture.counters.add(count, 1);
        disposition
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

    /// Materializes the capture domain and the project's scope once per store incarnation.
    fn ensure_scope(&self, store: &KernelStore, incarnation: &str) -> Result<(), KernelError> {
        let marker = (incarnation.to_owned(), self.project.digest().to_owned());
        if self
            .capture
            .ready_scopes
            .lock()
            .expect("capture scopes mutex")
            .contains(&marker)
        {
            return Ok(());
        }
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
            let mut ready = false;
            let mut refused = None;
            crate::kernel_routes::commit::ensure_scope(
                envelope,
                project,
                &mut ready,
                &mut domains,
                &mut refused,
            )?;
            Ok(String::new())
        })?;
        let mut ready = self
            .capture
            .ready_scopes
            .lock()
            .expect("capture scopes mutex");
        if ready.len() >= MAX_READY_SCOPES {
            ready.clear();
        }
        ready.insert(marker);
        Ok(())
    }
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
    fn the_memo_stays_within_its_byte_budget_and_keeps_the_first_deferral_time() {
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
