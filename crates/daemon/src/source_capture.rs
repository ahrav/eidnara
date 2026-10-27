//! Publishes the native message text and tool results of a transform window as kernel source descriptors from a background supervisor, after the transform response is handed to the wire writer.
//!
//! Capture runs when `source_capture.enabled` and `memory.enabled` both permit it in the route's consent snapshot and the session is not a subagent's; otherwise the adapters never run. Once the pass has built its response, the handler offers the pass's native records to [`SourceCapture::offer`], which charges the retained window to the supervisor's own byte budget. When the response is handed to the wire writer, [`CaptureHandoff::into_job`] takes the store's single publication slot; a refused offer or handoff (`refused_window_too_large`, `refused_resident_short`, `refused_queue_full`, `refused_shutting_down`) drops its reference and charge at once. The accepted job owns the slot permit, the charges, and its completion: it runs the adapters, charges their copies, skips units the memo already knows, and publishes at most one screened batch under a 5 s budget whose origin is the handoff's acceptance. The response bytes never wait on the kernel or the job.
//!
//! Kernel receipts are the durable record. The memo is a bounded, process-local cache keyed by the store's incarnation and each unit's native identity, revision, and representation; an entry holds the request digest, a disposition, and the time a pending unit first waited, never window bytes. A memo hit with an equal digest suppresses the kernel probe; after a restart the memo is empty and the kernel answers from its receipts. Pending units are offered ahead of units the memo does not know. A window refused at a full slot and never offered again is lost to capture; the next pass that carries it is captured.
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
    Harness, MAX_SOURCE_BATCH_BYTES, MAX_SOURCE_UNIT_BYTES, PublishError, Published,
    Representation, SessionIdentity, SourcePublisher, SourceUnit, UnitDisposition,
    opencode_units_within, pi_units_within, publish_screened, publish_screened_between,
    receipt_key_of,
};
use crate::healing::SerializerProfile;
use crate::kernel_routes::{KernelOpenCoordinator, ProjectBinding};
use crate::project_docs::{DocumentPath, DocumentRead};
use crate::repository_identity::{
    DerivationFailure, MAX_ROOT_WALK_COMMITS, RepositoryIdentity, derive,
};

/// The cooperative budget of one publication, from handoff acceptance.
pub(crate) const CAPTURE_BUDGET: Duration = Duration::from_secs(5);
/// The share of the job's budget a repository identity derivation may spend, so a walk that does not finish leaves the window and the documents' commits the rest.
pub(crate) const REPOSITORY_WALK_BUDGET: Duration = Duration::from_secs(2);
/// The supervisor's byte budget: the retained window, the adapters' copies, and the batch's artifact preparation.
const MAX_RETAINED_WINDOW_BYTES: usize = 32 * 1024 * 1024;
/// Artifact preparation copies an admitted batch a bounded number of times.
const ARTIFACT_PREPARATION_BYTES: usize = 4 * MAX_SOURCE_BATCH_BYTES;
/// Capture scopes recorded as ensured before the record starts over; a cleared record costs one replayed scope commit per project.
const MAX_RECORDED_PROJECTS: usize = 1024;
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

pub(crate) use counts;

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
    DocumentsUnchanged => "documents_unchanged",
    DocumentsRetired => "documents_retired",
    DocumentsMissing => "documents_missing",
    DocumentsNotRegular => "documents_not_regular",
    DocumentsOversize => "documents_oversize",
    DocumentsNotUtf8 => "documents_not_utf8",
    DocumentsUnreadable => "documents_unreadable",
    DocumentsRefused => "documents_refused",
    ExpiryRetired => "expiry_retired",
    ExpiryRetained => "expiry_retained",
}

struct Counters([AtomicU64; Count::ALL.len()]);

impl Default for Counters {
    fn default() -> Self {
        Self(std::array::from_fn(|_| AtomicU64::new(0)))
    }
}

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
    /// The database incarnation and project of each capture scope a commit has ensured. The scope commit's receipt replays without rerunning the commit, so a recorded pair needs no second commit; a new database incarnation keys a new pair.
    ensured_scopes: Mutex<HashSet<String>>,
    /// Each project's repository identity, fixed for the incarnation once a derivation settles it, so a repository that changes mid-incarnation moves its documents at the next restart rather than between passes.
    repository_ids: Mutex<HashMap<String, RepositoryIdentity>>,
    swept_paths: Mutex<HashSet<String>>,
    /// Bumped by every `session.delete` under the capture commit gate; a job taken at an older epoch publishes nothing, and the next pass offers its window again.
    delete_epoch: AtomicU64,
    counters: Counters,
    #[cfg(test)]
    pub(crate) before_publish: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// Runs inside the batch publication after screening and before the commit, where a purge reaches the commit without the pre-screen seeing it.
    #[cfg(test)]
    pub(crate) between_screen_and_commit: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// Runs after a documents batch retires stale-identity rows and before it publishes, where a crash leaves the path with no live row.
    #[cfg(test)]
    pub(crate) between_document_retire_and_publish: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// Runs before a project's repository identity is derived, where a slow walk spends the job's budget.
    #[cfg(test)]
    pub(crate) before_repository_derivation: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    #[cfg(test)]
    pub(crate) before_stale_sweep: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
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
            ensured_scopes: Mutex::default(),
            repository_ids: Mutex::default(),
            swept_paths: Mutex::default(),
            delete_epoch: AtomicU64::new(0),
            counters: Counters::default(),
            #[cfg(test)]
            before_publish: Mutex::new(None),
            #[cfg(test)]
            between_screen_and_commit: Mutex::new(None),
            #[cfg(test)]
            between_document_retire_and_publish: Mutex::new(None),
            #[cfg(test)]
            before_repository_derivation: Mutex::new(None),
            #[cfg(test)]
            before_stale_sweep: Mutex::new(None),
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
    /// Where the pass leaves the configured repository documents it reads for its prompt; `None` when the route injects no documents. A pass that composes no prompt block leaves none.
    pub documents: Option<Arc<crate::project_docs::DocumentReads>>,
    /// The route's frozen `contradiction_candidates.enabled`: the job then keeps each published unit's text for one candidate pass after it ends.
    pub candidates: bool,
}

/// What a `Completed` job that published at least one unit hands to the candidate pass once the slot is released: each published unit's exact text with its receipt, the project, and the job's retained charge, which stays charged until the pass drops this.
pub(crate) struct SettledPublication {
    pub(crate) project: ProjectBinding,
    pub(crate) units: Vec<(String, Published)>,
    _retained: RetainedCharge,
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
    documents: PendingDocuments,
    candidates: bool,
}

/// The document reads a capture holds until its documents batch takes them. A handoff or job dropped with reads still held dropped them before the batch, so the drop counts them as `documents_refused`; the reads wait for the next pass that composes the block.
struct PendingDocuments {
    capture: Arc<SourceCapture>,
    reads: Vec<(DocumentPath, DocumentRead)>,
    /// The project's repository identity, derived before the commit gate so a slow walk holds no other capture or session deletion.
    identity: Option<Derivation>,
}

/// A repository identity derivation's outcome.
type Derivation = Result<RepositoryIdentity, DerivationFailure>;

impl PendingDocuments {
    fn take(mut self) -> (Vec<(DocumentPath, DocumentRead)>, Option<Derivation>) {
        (std::mem::take(&mut self.reads), self.identity.take())
    }
}

impl Drop for PendingDocuments {
    fn drop(&mut self) {
        if !self.reads.is_empty() {
            self.capture
                .counters
                .add(Count::DocumentsRefused, self.reads.len());
        }
    }
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

/// The top-up target amortizes budget charges across copied units; admission falls back to the exact deficit when the target charge fails.
const COPY_TOP_UP_BYTES: usize = 64 * 1024;

struct CopyCharge<'a> {
    capture: &'a Arc<SourceCapture>,
    granted: usize,
    used: usize,
    top_ups: Vec<RetainedCharge>,
}

impl CopyCharge<'_> {
    fn admit(&mut self, bytes: usize) -> bool {
        self.used = self.used.saturating_add(bytes);
        let Some(deficit) = self
            .used
            .checked_sub(self.granted)
            .filter(|deficit| *deficit > 0)
        else {
            return true;
        };
        let Some(charge) = RetainedCharge::take(self.capture, deficit.max(COPY_TOP_UP_BYTES))
            .or_else(|| RetainedCharge::take(self.capture, deficit))
        else {
            return false;
        };
        self.granted += charge.bytes;
        self.top_ups.push(charge);
        true
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
    /// Charges a consented pass's native window and the documents it read for capture. `None` when the pass leaves neither, and when the window bound or the budget refuses it; a refusal is counted and retains nothing.
    pub(crate) fn offer(self: &Arc<Self>, offer: Offer) -> Option<CaptureHandoff> {
        let harness = profile_harness(offer.profile)?;
        // The documents the pass read when it composed its prompt block are the ones captured; a refusal below drops them counted.
        let documents = PendingDocuments {
            capture: Arc::clone(self),
            reads: crate::project_docs::take_reads(offer.documents.as_deref()).unwrap_or_default(),
            identity: None,
        };
        if offer.window.is_empty() && documents.reads.is_empty() {
            return None;
        }
        let window_bytes: usize = offer
            .window
            .iter()
            .map(|value| crate::retained_size::value_retained_bytes(value))
            .sum();
        let document_bytes: usize = documents
            .reads
            .iter()
            .map(|(_, read)| match read {
                DocumentRead::Text(text) => text.len(),
                _ => 0,
            })
            .sum();
        // The adapters copy about the window's bytes once more, and the job tops up for any excess. A document's text is charged twice as well: its unit holds it, and the second share covers the copy publication makes.
        let bytes = window_bytes
            .saturating_add(document_bytes)
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
            documents,
            candidates: offer.candidates,
        })
    }

    /// Whether no job holds the slot.
    #[cfg(any(test, feature = "test-support", feature = "direct-host-fixture"))]
    pub(crate) fn is_idle(&self) -> bool {
        self.slot.available_permits() == 1
    }

    #[cfg(any(test, feature = "test-support", feature = "direct-host-fixture"))]
    pub(crate) fn settled_report(&self, draining: bool, now_ms: i64) -> (Value, bool) {
        let idle = self.is_idle();
        (self.report(draining, now_ms), idle)
    }

    /// Empties the memo and the ensured-scope record, as a restart does.
    #[cfg(test)]
    pub(crate) fn forget_memo_for_test(&self) {
        *self.memo() = Memo::default();
        self.ensured_scopes().clear();
        self.repository_ids().clear();
        self.swept_paths().clear();
    }

    fn repository_ids(&self) -> MutexGuard<'_, HashMap<String, RepositoryIdentity>> {
        self.repository_ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn swept_paths(&self) -> MutexGuard<'_, HashSet<String>> {
        self.swept_paths
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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

    /// A poisoned record is an empty one: the next job of each project replays its scope commit.
    fn ensured_scopes(&self) -> MutexGuard<'_, HashSet<String>> {
        self.ensured_scopes.lock().unwrap_or_else(|poisoned| {
            self.ensured_scopes.clear_poison();
            let mut scopes = poisoned.into_inner();
            scopes.clear();
            scopes
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
            documents: self.documents,
            kept_for_pass: self.candidates.then(std::cell::RefCell::default),
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
    documents: PendingDocuments,
    /// The text and receipt of each unit this job published, kept only when the route consents to candidates.
    kept_for_pass: Option<std::cell::RefCell<Vec<(String, Published)>>>,
}

impl CaptureJob {
    #[cfg(test)]
    pub(crate) async fn run(self) -> Count {
        self.run_settled().await.0
    }

    /// Runs the publication on the blocking pool and returns once it physically completes, with the publication a candidate pass scores; the blocking closure has returned, so the slot permit is released and the commit gate is free. Cancellation interrupts the budget, so the job stops before its next kernel wait; the wait for completion is then bounded by the budget, the capture commit gate's current holder, and the adapters' work on the window.
    pub(crate) async fn run_settled(self) -> (Count, Option<SettledPublication>) {
        self.capture.counters.add(Count::Admitted, 1);
        let interrupt = Arc::clone(&self.interrupt);
        let cancel = self.cancel.clone();
        let capture = Arc::clone(&self.capture);
        let mut join = tokio::task::spawn_blocking(move || self.publish_settled());
        let joined = tokio::select! {
            joined = &mut join => joined,
            () = cancel.cancelled() => {
                interrupt.store(true, Ordering::Release);
                join.await
            }
        };
        // A failed join dropped the slot, the charges, and the window while the closure unwound.
        let end = match joined {
            Ok(settled) => return settled,
            Err(error) if error.is_panic() => Count::WorkerPanicked,
            Err(_) => Count::Discarded,
        };
        capture.counters.add(end, 1);
        log("job ended", end, 1);
        (end, None)
    }

    /// Publishes, then releases the slot permit by dropping the job before it returns. The pass holds a charge for the kept texts and receipts in place of the job's charge; when the budget cannot take it while the job's charge is still held, the pass keeps the job's larger charge.
    fn publish_settled(mut self) -> (Count, Option<SettledPublication>) {
        let end = self.publish();
        let CaptureJob {
            capture,
            project,
            _retained,
            kept_for_pass,
            ..
        } = self;
        let units = kept_for_pass
            .map(std::cell::RefCell::into_inner)
            .unwrap_or_default();
        if end != Count::Completed || units.is_empty() {
            return (end, None);
        }
        let kept: usize = units
            .iter()
            .map(|(text, published)| {
                text.len()
                    + published.object_id.len()
                    + published.occurrence_id.len()
                    + published.evidence_object_id.len()
                    + published.evidence_id.len()
                    + std::mem::size_of::<(String, Published)>()
            })
            .sum();
        let retained = RetainedCharge::take(&capture, kept).unwrap_or(_retained);
        (
            end,
            Some(SettledPublication {
                project,
                units,
                _retained: retained,
            }),
        )
    }

    /// Keeps a published unit's text and receipt for the candidate pass when the route consents to one.
    fn keep_published(&self, disposition: &UnitDisposition, text: String) {
        if let (Some(kept), UnitDisposition::Published(published)) =
            (&self.kept_for_pass, disposition)
        {
            kept.borrow_mut().push((text, published.clone()));
        }
    }

    fn end(&self, end: Count) -> Count {
        self.capture.counters.add(end, 1);
        if end != Count::Completed {
            log("job ended", end, 1);
        }
        end
    }

    fn publish(&mut self) -> Count {
        if self.cancel.is_cancelled() {
            return self.end(Count::Discarded);
        }
        let Ok(store) = self.kernel.kernel_store() else {
            return self.end(Count::KernelUnavailable);
        };
        if self.budget.is_exhausted() {
            return self.end(Count::Deadline);
        }
        let mut documents = PendingDocuments {
            capture: Arc::clone(&self.capture),
            reads: std::mem::take(&mut self.documents.reads),
            identity: None,
        };
        let Some((units, mut copies)) = self.units() else {
            return self.end(Count::RefusedResidentShort);
        };
        if units.is_empty() && documents.reads.is_empty() {
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
        // A memo key names the unit's native identity in class field order with its revision and representation; `well_formed_value` rejects control characters, so U+001F separates encodable identity fields unambiguously.
        let mut identities: Vec<(String, String)> = Vec::with_capacity(units.len());
        for unit in &units {
            let fields = unit.class.identity_fields().iter();
            let key = [incarnation.as_str(), unit.class.code(), &unit.revision]
                .into_iter()
                .chain([unit.representation.as_str()])
                .chain(fields.map(|field| unit.identity_value(field).unwrap_or_default()))
                .collect::<Vec<_>>()
                .join("\u{1f}");
            let digest = publisher.request_digest(unit);
            // `seen` holds a second copy of the key and digest.
            if !copies.admit(2 * (key.len() + digest.len())) {
                return self.end(Count::RefusedResidentShort);
            }
            identities.push((key, digest));
        }
        let found: Vec<Option<bool>> = {
            let memo = self.capture.memo();
            let known = |(key, digest): &(String, String)| {
                memo.get(key).filter(|entry| entry.digest == *digest)
            };
            identities
                .iter()
                .map(|identity| {
                    known(identity)
                        .map(|entry| matches!(entry.disposition, Disposition::Pending { .. }))
                })
                .collect()
        };
        let (mut pending, mut unknown) = (Vec::new(), Vec::new());
        // Each `seen` entry records the first occurrence's digest and whether this pass offers that occurrence.
        let mut seen: HashMap<String, (String, bool)> = HashMap::with_capacity(identities.len());
        let mut conflicts = 0;
        for ((unit, (key, digest)), found) in units.into_iter().zip(identities).zip(found) {
            let keyed = found.is_some() || receipt_key_of(&unit).is_some();
            if keyed {
                if let Some((first, offered)) = seen.get(&key) {
                    if *offered && *first != digest {
                        conflicts += 1;
                    }
                    continue;
                }
                seen.insert(key.clone(), (digest.clone(), found != Some(false)));
            }
            match found {
                Some(true) => pending.push((unit, Some((key, digest)))),
                Some(false) => self.capture.counters.add(Count::UnitsMemoSkipped, 1),
                None => unknown.push((unit, keyed.then_some((key, digest)))),
            }
        }
        if conflicts > 0 {
            self.capture
                .counters
                .add(Count::UnitsIdentityConflict, conflicts);
            log("unit terminal", Count::UnitsIdentityConflict, conflicts);
        }
        let (candidates, keys): (Vec<SourceUnit>, Vec<Option<(String, String)>>) =
            pending.into_iter().chain(unknown).unzip();
        if candidates.is_empty() && documents.reads.is_empty() {
            return self.end(Count::Completed);
        }
        #[cfg(test)]
        run_hook(&self.capture.before_publish);
        if self.cancel.is_cancelled() {
            return self.end(Count::Discarded);
        }
        if !documents.reads.is_empty() {
            documents.identity = Some(self.repository_identity());
        }
        // A panic while the shared gate is held would poison it for session deletion; it is caught here and resumed once the guard is released.
        let gate = self.commit_gate.lock().expect("capture publication mutex");
        let published = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.publish_locked(
                &store,
                &publisher,
                &incarnation,
                candidates,
                keys,
                documents,
            )
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
        incarnation: &str,
        candidates: Vec<SourceUnit>,
        keys: Vec<Option<(String, String)>>,
        documents: PendingDocuments,
    ) -> Count {
        if self.capture.delete_epoch.load(Ordering::Acquire) != self.epoch {
            return self.end(Count::SessionInvalidated);
        }
        if let Err(error) = self.ensure_scope(store, incarnation) {
            return self.end(kernel_end(error));
        }
        if self.budget.is_exhausted() {
            return self.end(Count::Deadline);
        }
        if candidates.is_empty() {
            let end = self.capture_documents(store, publisher, incarnation, documents);
            return self.end(end);
        }
        let observed_at = crate::now_ms();
        let started = Instant::now();
        let result = publish_screened_between(
            publisher,
            &candidates,
            observed_at,
            Some(&self.budget),
            || {
                #[cfg(test)]
                run_hook(&self.capture.between_screen_and_commit);
            },
        );
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
                for ((disposition, key), unit) in dispositions.into_iter().zip(keys).zip(candidates)
                {
                    self.keep_published(&disposition, unit.text);
                    let (count, recorded) = classify(disposition, observed_at);
                    counters.add(count, 1);
                    if let Some((key, digest)) = key {
                        memo.record(key, digest, recorded);
                    }
                }
                drop(memo);
                let end = self.capture_documents(store, publisher, incarnation, documents);
                self.end(end)
            }
            Err(refusal) => {
                let culprit = refusal.conflicting_unit();
                for (index, key) in keys.iter().enumerate() {
                    let Some((key, digest)) = key else {
                        continue;
                    };
                    if culprit == Some(index) {
                        counters.add(Count::UnitsIdentityConflict, 1);
                        memo.record(key.clone(), digest.clone(), Disposition::Terminal);
                        log("unit terminal", Count::UnitsIdentityConflict, 1);
                    } else {
                        memo.record(
                            key.clone(),
                            digest.clone(),
                            Disposition::Pending {
                                since_ms: observed_at,
                            },
                        );
                    }
                }
                drop(memo);
                let documents = self.capture_documents(store, publisher, incarnation, documents);
                if documents != Count::Completed {
                    log("documents ended", documents, 1);
                }
                self.end(kernel_end(refusal.error))
            }
        }
    }

    /// Captures the configured documents the pass read when it composed its prompt block, in their own batch after the window's, under the commit gate, and returns how the batch ended.
    ///
    /// - Identity: the project, its repository identity ([`crate::repository_identity`]), and the path. A derivation the environment cuts short refuses every read as `documents_refused`, retires and publishes nothing, and memoizes nothing.
    /// - A path whose read settles its current lineage (text to publish, unchanged text, or a verified missing file) first retires every live row this capture's domain owns for the path under another repository identity.
    /// - Text whose bytes equal the live revision's artifact is unchanged.
    /// - Other text publishes at the lineage's historical head plus one, which the descriptor commit checks as the expected head; a head that moved refuses as `documents_refused`.
    /// - `documents_refused` also counts every read the pass's capture drops before this batch handles it: a refused or dropped handoff, a job that ends before the batch, a deadline, a failed preview, or a failed retirement.
    /// - A verified missing file retires its live revision when this capture's domain owns it; every other read is counted and retires nothing.
    ///
    /// A refused document leaves the window's publication as it was, and a refused window leaves the documents to this batch. A read this job drops, through a refusal, a busy slot, or a deadline, waits for the next pass that composes the block; that compose reads afresh and republishes what differs from the live revision, so the lineage converges on the latest compose read.
    fn capture_documents(
        &self,
        store: &KernelStore,
        publisher: &SourcePublisher<'_>,
        incarnation: &str,
        documents: PendingDocuments,
    ) -> Count {
        let (documents, identity) = documents.take();
        let count = documents.len();
        if count == 0 {
            return Count::Completed;
        }
        if self.budget.is_exhausted() {
            self.capture.counters.add(Count::DocumentsRefused, count);
            return Count::Deadline;
        }
        // `publish` derives the identity of every pass that holds document reads.
        let identity = match identity.unwrap_or(Err(DerivationFailure::Unreadable)) {
            Ok(identity) => identity,
            Err(failure) => {
                self.capture.counters.add(Count::DocumentsRefused, count);
                log("documents refused", Count::DocumentsRefused, count);
                return match failure {
                    DerivationFailure::Budget => Count::Deadline,
                    DerivationFailure::Unreadable => Count::Completed,
                };
            }
        };
        let repository_id = identity.value(&self.session.project_id);
        let plan = match self.plan_documents(store, incarnation, repository_id, documents) {
            Ok(plan) => plan,
            Err(error) => {
                self.capture.counters.add(Count::DocumentsRefused, count);
                return kernel_end(error);
            }
        };
        // Every stale row retires in its own commit before the current lineage publishes, so a crash between the two leaves the path with no live row rather than two.
        for object_id in plan.stale {
            if let Err(error) =
                self.retire_document(store, &object_id, "repository identity changed")
            {
                self.capture.counters.add(Count::DocumentsRefused, count);
                return kernel_end(error);
            }
            self.capture.counters.add(Count::DocumentsRetired, 1);
        }
        self.capture.swept_paths().extend(plan.swept);
        #[cfg(test)]
        run_hook(&self.capture.between_document_retire_and_publish);
        let mut units = Vec::new();
        // Reads handled so far; a failed retirement counts every other read as refused.
        let mut handled = 0;
        for step in plan.steps {
            match step {
                DocumentStep::Publish(unit) => units.push(unit),
                DocumentStep::Retire(object_id) => {
                    if let Err(error) =
                        self.retire_document(store, &object_id, "repository document removed")
                    {
                        self.capture
                            .counters
                            .add(Count::DocumentsRefused, count - handled);
                        return kernel_end(error);
                    }
                    self.capture.counters.add(Count::DocumentsRetired, 1);
                    handled += 1;
                }
                DocumentStep::Skip(skip) => {
                    self.capture.counters.add(skip, 1);
                    handled += 1;
                }
            }
        }
        if units.is_empty() {
            return Count::Completed;
        }
        match publish_screened(publisher, &units, crate::now_ms(), Some(&self.budget)) {
            Ok(dispositions) => {
                for (disposition, unit) in dispositions.into_iter().zip(units) {
                    self.keep_published(&disposition, unit.text);
                    let count = match disposition {
                        UnitDisposition::Published(published) if published.replayed => {
                            Count::UnitsReplayed
                        }
                        UnitDisposition::Published(_) => Count::UnitsPublished,
                        // Every other disposition of a document counts here: a refused identity or revision, an oversize or deferred unit.
                        _ => Count::DocumentsRefused,
                    };
                    self.capture.counters.add(count, 1);
                }
                Count::Completed
            }
            Err(refusal) => {
                self.capture
                    .counters
                    .add(Count::DocumentsRefused, units.len());
                kernel_end(refusal.error)
            }
        }
    }

    fn plan_documents(
        &self,
        store: &KernelStore,
        incarnation: &str,
        repository_id: &str,
        documents: Vec<(DocumentPath, DocumentRead)>,
    ) -> Result<DocumentPlan, KernelError> {
        let project_id = self.session.project_id.as_str();
        let unswept: Vec<Option<String>> = {
            let swept = self.capture.swept_paths();
            documents
                .iter()
                .map(|(path, _)| {
                    let path = path.as_str();
                    let key = format!("{incarnation}\u{1f}{project_id}\u{1f}{path}");
                    (!swept.contains(&key)).then_some(key)
                })
                .collect()
        };
        let (_, plan) = store.preview(self.preview_deadline(), |preview| {
            let (mut settled, mut swept) = (Vec::new(), Vec::new());
            let mut steps = Vec::with_capacity(documents.len());
            for ((path, read), key) in documents.into_iter().zip(unswept) {
                let step = self.plan_document(preview, repository_id, path.as_str(), read)?;
                if let Some(key) = key.filter(|_| step.settles_lineage()) {
                    settled.push(path);
                    swept.push(key);
                }
                steps.push(step);
            }
            #[cfg(test)]
            if !settled.is_empty() {
                run_hook(&self.capture.before_stale_sweep);
            }
            let settled_paths: Vec<&str> = settled.iter().map(DocumentPath::as_str).collect();
            let stale = stale_documents(
                preview,
                &self.budget,
                project_id,
                repository_id,
                &settled_paths,
            )?;
            Ok(DocumentPlan {
                steps,
                stale,
                swept,
            })
        })?;
        Ok(plan)
    }

    /// The project's repository identity, derived once per incarnation; an environmental failure memoizes nothing, so the next pass derives again.
    fn repository_identity(&self) -> Result<RepositoryIdentity, DerivationFailure> {
        let project_id = &self.session.project_id;
        if let Some(identity) = self.capture.repository_ids().get(project_id) {
            return Ok(identity.clone());
        }
        // A private interrupt the job's cancellation still raises: the walk's own deadline ends the walk alone, never the job.
        let walk = self
            .budget
            .linked()
            .bounded_by(Instant::now() + REPOSITORY_WALK_BUDGET);
        #[cfg(test)]
        run_hook(&self.capture.before_repository_derivation);
        let identity = derive(self.project.root(), &walk, MAX_ROOT_WALK_COMMITS)?;
        Ok(self
            .capture
            .repository_ids()
            .entry(project_id.clone())
            .or_insert(identity)
            .clone())
    }

    /// What capture does with one configured document, given its lineage's live revision.
    fn plan_document(
        &self,
        preview: &kernel::Preview<'_>,
        repository_id: &str,
        path: &str,
        read: DocumentRead,
    ) -> Result<DocumentStep, KernelError> {
        let text = match read {
            // Text within the read cap but past the unit cap counts with the reads past the read cap.
            DocumentRead::Text(text) if text.len() > MAX_SOURCE_UNIT_BYTES => {
                return Ok(DocumentStep::Skip(Count::DocumentsOversize));
            }
            DocumentRead::Text(text) => Some(text),
            DocumentRead::Missing => None,
            DocumentRead::NotRegular => return Ok(DocumentStep::Skip(Count::DocumentsNotRegular)),
            DocumentRead::Oversize => return Ok(DocumentStep::Skip(Count::DocumentsOversize)),
            DocumentRead::NotUtf8 => return Ok(DocumentStep::Skip(Count::DocumentsNotUtf8)),
            DocumentRead::Unreadable => return Ok(DocumentStep::Skip(Count::DocumentsUnreadable)),
        };
        let project_id = self.session.project_id.as_str();
        let identity = vec![
            ("project_id", project_id.to_owned()),
            ("repository_id", repository_id.to_owned()),
            ("relative_path", path.to_owned()),
        ];
        let lineage = document_lineage(&identity)?;
        // The live revision is read with every row check: a retirement names it, and a changed document publishes through the descriptor commit, which checks the predecessor it supersedes. A row another domain owns is not this capture's revision: a missing file retires nothing, and a publication is refused as a mismatched domain.
        let live = preview
            .live_source_descriptor_of_lineage(OccurrenceClass::RepositoryDocuments, &lineage)?
            .filter(|row| row.domain_id == CAPTURE_DOMAIN_ID);
        let Some(text) = text else {
            return Ok(
                live.map_or(DocumentStep::Skip(Count::DocumentsMissing), |row| {
                    DocumentStep::Retire(row.object_id)
                }),
            );
        };
        if live.is_some_and(|row| {
            row.detail.artifact_digest == crate::project_docs::sha256_hex(text.as_bytes())
        }) {
            return Ok(DocumentStep::Skip(Count::DocumentsUnchanged));
        }
        let head = preview.source_lineage_head(&lineage)?.unwrap_or(0);
        Ok(DocumentStep::Publish(SourceUnit {
            class: OccurrenceClass::RepositoryDocuments,
            identity,
            revision: (head + 1).to_string(),
            representation: Representation::Text,
            text,
            role: "document".to_owned(),
            outcome: None,
        }))
    }

    /// Retires the live revision `object_id` of a document whose file a read found missing, within the job's budget; the receipt is keyed by the revision, so a retry replays it.
    fn retire_document(
        &self,
        store: &KernelStore,
        object_id: &str,
        cause: &str,
    ) -> Result<(), KernelError> {
        let digest = kernel::source_identity::identity_digest(object_id.as_bytes());
        store
            .commit_within_budget(
                &self.budget,
                CommitIntent {
                    producer: PRODUCER.to_owned(),
                    operation_key: format!("source-document-retire:{digest}"),
                    request_digest: digest,
                    actor: "repository".to_owned(),
                    cause: cause.to_owned(),
                },
                |envelope| {
                    crate::harness_sources::retire_descriptors(envelope, &[object_id.to_owned()])
                },
            )
            .map(|_| ())
    }

    /// A preview's reader wait: the receipt wait, cut to the job's deadline.
    fn preview_deadline(&self) -> Instant {
        let wait = Instant::now() + PREVIEW_WAIT;
        self.budget
            .deadline()
            .map_or(wait, |deadline| deadline.min(wait))
    }

    /// Every unit of the window, in record order, with the charges for copies beyond the window share the offer charged; a record the adapter refuses contributes none and is counted. `None` once the budget refuses a unit's copy, which the adapter drops before building the next one.
    fn units(&self) -> Option<(Vec<SourceUnit>, CopyCharge<'_>)> {
        let mut units = Vec::new();
        let mut refused = 0;
        let mut copies = CopyCharge {
            capture: &self.capture,
            granted: self.window_bytes,
            used: 0,
            top_ups: Vec::new(),
        };
        let mut stopped = false;
        for record in &self.window {
            let admit = &mut |bytes| copies.admit(bytes);
            let adapted = match self.session.harness {
                Harness::OpenCode => opencode_units_within(&self.session, record, admit),
                Harness::Pi => pi_units_within(&self.session, record, admit),
            };
            match adapted {
                Ok(Some(adapted)) => units.extend(adapted),
                Ok(None) => {
                    stopped = true;
                    break;
                }
                Err(_) => refused += 1,
            }
        }
        if refused > 0 {
            self.capture.counters.add(Count::AdapterRefused, refused);
            log("adapter", Count::AdapterRefused, refused);
        }
        (!stopped).then_some((units, copies))
    }

    /// Materializes the capture domain and the project's scope; the receipt makes every later attempt a replay.
    fn ensure_scope(&self, store: &KernelStore, incarnation: &str) -> Result<(), KernelError> {
        let ensured = format!("{incarnation}\u{1f}{}", self.project.digest());
        if self.capture.ensured_scopes().contains(&ensured) {
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
        let mut scopes = self.capture.ensured_scopes();
        if scopes.len() >= MAX_RECORDED_PROJECTS {
            scopes.clear();
        }
        scopes.insert(ensured);
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
/// Pages one session-retirement inventory snapshot lists. The snapshot is one read transaction, so its pages commit while it reads on, and at most this many listed pages wait for their commit.
const SESSION_INVENTORY_PAGES: usize = 16;
/// How long a session-retirement or document preview waits for a pooled reader.
const PREVIEW_WAIT: Duration = Duration::from_secs(5);
/// Leads the bytes a session-retirement page's receipt key binds.
const SESSION_RETIREMENT_FORMAT: &str = "source-session-retire.v1";

/// One captured session: the identity fields its `messages` and `raw_tool_spans` descriptors name.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CapturedSession<'a> {
    pub(crate) project_id: &'a str,
    pub(crate) harness: &'a str,
    pub(crate) session_id: &'a str,
}

/// Retires the live `messages` and `raw_tool_spans` descriptors of `session` in at most `pages` pages of at most `page_rows` descriptors. Returns `true` once the inventory has passed both classes and `false` when the page bound ends first. The inventory makes one keyset pass over each class; each snapshot lists up to [`SESSION_INVENTORY_PAGES`] pages and the next snapshot resumes where it stopped. A scoped thread lists while this thread commits: full pages leave as soon as they are listed, each class's descriptors in creation order, so commits of one class overlap the listing of the next. Creation order keeps one page's writes on neighbouring registry and observation rows. The caller holds the capture commit gate, which serializes capture publication with retirement, so the one pass lists every descriptor the session holds. Each page is one commit. A retry starts a fresh pass. The session and the page's sorted object ids determine the page's receipt key ([`session_page_intent`]); a replayed receipt means the inventory listed rows a committed page retired, which is [`KernelError::Conflict`]. The maintenance expiry sweep can retire listed rows first, and the page then retires the remaining rows.
///
/// # Errors
///
/// Returns the first commit error, or else the first inventory error; pages committed before it stay retired.
pub(crate) fn retire_session_pages(
    store: &KernelStore,
    session: CapturedSession<'_>,
    page_rows: std::num::NonZeroUsize,
    pages: usize,
) -> Result<bool, KernelError> {
    std::thread::scope(|scope| {
        let (sender, receiver) =
            std::sync::mpsc::sync_channel::<Vec<String>>(SESSION_INVENTORY_PAGES);
        let lister =
            scope.spawn(move || list_session_pages(store, session, page_rows, pages, &sender));
        for created in receiver {
            let mut page = created.clone();
            page.sort_unstable();
            let receipt = store.commit(session_page_intent(session, &page), |envelope| {
                crate::harness_sources::retire_descriptors(envelope, &created)
            })?;
            if receipt.replayed {
                return Err(KernelError::Conflict);
            }
        }
        lister
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

/// Lists `session`'s pages for [`retire_session_pages`] into `out`, at most `pages` of them, and returns whether the inventory passed both classes. Stops once the committing side has gone.
fn list_session_pages(
    store: &KernelStore,
    session: CapturedSession<'_>,
    page_rows: std::num::NonZeroUsize,
    pages: usize,
    out: &std::sync::mpsc::SyncSender<Vec<String>>,
) -> Result<bool, KernelError> {
    let mut cursor = InventoryCursor::default();
    let mut remaining = pages;
    while remaining > 0 {
        let snapshot_pages = remaining.min(SESSION_INVENTORY_PAGES);
        let (_, listed) = store.preview(Instant::now() + PREVIEW_WAIT, |preview| {
            let mut cursor = cursor.clone();
            let mut left = snapshot_pages;
            let mut pending: Vec<(i64, String)> = Vec::new();
            let send = |chunk: &[(i64, String)]| {
                out.send(chunk.iter().map(|(_, id)| id.clone()).collect())
                    .is_ok()
            };
            while let (Some(&class), Some(room)) = (
                SESSION_CLASSES.get(cursor.class),
                std::num::NonZeroUsize::new(
                    left.saturating_mul(page_rows.get())
                        .saturating_sub(pending.len()),
                ),
            ) {
                let found = preview.live_source_descriptors_with(
                    class,
                    "session_id",
                    &[session.session_id],
                    cursor.after.as_deref(),
                    room,
                )?;
                let mut batch: Vec<(i64, String)> = found
                    .rows
                    .iter()
                    .filter(|row| {
                        crate::git_reconcile::identity_field(row, "project_id")
                            == Some(session.project_id)
                            && crate::git_reconcile::identity_field(row, "harness")
                                == Some(session.harness)
                    })
                    .map(|row| (row.created_commit_seq, row.object_id.clone()))
                    .collect();
                batch.sort_unstable();
                pending.extend(batch);
                while pending.len() >= page_rows.get() {
                    if !send(&pending[..page_rows.get()]) {
                        return Ok(None);
                    }
                    pending.drain(..page_rows.get());
                    left -= 1;
                }
                cursor = match found.next {
                    Some(next) => InventoryCursor {
                        class: cursor.class,
                        after: Some(next),
                    },
                    None => InventoryCursor {
                        class: cursor.class + 1,
                        after: None,
                    },
                };
            }
            if !pending.is_empty() {
                if !send(&pending) {
                    return Ok(None);
                }
                left -= 1;
            }
            Ok(Some((cursor, snapshot_pages - left)))
        })?;
        let Some((next, listed_pages)) = listed else {
            return Ok(false);
        };
        if next.class == SESSION_CLASSES.len() {
            return Ok(true);
        }
        remaining -= listed_pages;
        cursor = next;
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

const SESSION_CLASSES: [OccurrenceClass; 2] =
    [OccurrenceClass::Messages, OccurrenceClass::RawToolSpans];

/// Where a session's inventory resumes: in `SESSION_CLASSES[class]` past the object id `after`. A `class` past the last one marks the inventory complete.
#[derive(Debug, Clone, Default)]
struct InventoryCursor {
    class: usize,
    after: Option<String>,
}

struct DocumentPlan {
    /// Each read's step under the project's repository identity, in read order.
    steps: Vec<DocumentStep>,
    /// The live rows of the inventoried paths under another repository identity, which retire before any step runs.
    stale: Vec<String>,
    /// The `swept_paths` keys of the inventoried paths, recorded once every stale row has retired.
    swept: Vec<String>,
}

/// What capture does with one configured document.
enum DocumentStep {
    Publish(SourceUnit),
    Retire(String),
    Skip(Count),
}

impl DocumentStep {
    /// Whether the read decides the path's current lineage: text to publish or unchanged, or a verified missing file. Only such a path moves off every other repository identity; a read that decides nothing keeps the row the path has.
    fn settles_lineage(&self) -> bool {
        matches!(
            self,
            Self::Publish(_)
                | Self::Retire(_)
                | Self::Skip(Count::DocumentsUnchanged | Count::DocumentsMissing)
        )
    }
}

/// Runs a test hook with its mutex released, so the hook can take the capture's own locks.
#[cfg(test)]
fn run_hook(slot: &Mutex<Option<Arc<dyn Fn() + Send + Sync>>>) {
    let hook = slot.lock().expect("capture hook mutex").clone();
    if let Some(hook) = hook {
        hook();
    }
}

/// The lineage of a document identity. A lineage names the identity, the class, and the representation, never the revision, so any revision encodes it.
fn document_lineage(identity: &[(&'static str, String)]) -> Result<String, KernelError> {
    let identity: Vec<(&str, &str)> = identity
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    kernel::source_identity::encode_preserving_span(&kernel::source_identity::Occurrence {
        class: OccurrenceClass::RepositoryDocuments.code(),
        identity: &identity,
        revision: "1",
        representation: Representation::Text.as_str(),
        span: None,
    })
    .map(|encoded| encoded.lineage_id)
    .map_err(|_| KernelError::InvalidInput)
}

/// Live document rows one stale-identity page reads.
const STALE_DOCUMENT_PAGE_ROWS: std::num::NonZeroUsize = std::num::NonZeroUsize::new(256).unwrap();

/// The live capture-domain document rows of `project_id` at a path in `paths` whose `repository_id` differs from `repository_id`: lineages an earlier identity of the same checkout left live.
fn stale_documents(
    preview: &kernel::Envelope<'_>,
    budget: &EvalBudget,
    project_id: &str,
    repository_id: &str,
    paths: &[&str],
) -> Result<Vec<String>, KernelError> {
    let mut stale = Vec::new();
    if paths.is_empty() {
        return Ok(stale);
    }
    let mut after: Option<String> = None;
    loop {
        let page = preview.live_source_descriptors_with(
            OccurrenceClass::RepositoryDocuments,
            "project_id",
            &[project_id],
            after.as_deref(),
            STALE_DOCUMENT_PAGE_ROWS,
        )?;
        stale.extend(
            page.rows
                .iter()
                .filter(|row| {
                    row.domain_id == CAPTURE_DOMAIN_ID
                        && crate::git_reconcile::identity_field(row, "repository_id")
                            != Some(repository_id)
                        && crate::git_reconcile::identity_field(row, "relative_path")
                            .is_some_and(|path| paths.contains(&path))
                })
                .map(|row| row.object_id.clone()),
        );
        match page.next {
            Some(_) if budget.is_exhausted() => return Err(KernelError::Deadline),
            Some(next) => after = Some(next),
            None => return Ok(stale),
        }
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
                line.bytes().all(|byte| byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || b" :_()".contains(&byte)),
                "{line}"
            );
        }
    }

    /// The restart simulation forgets every per-process record, so the next job of a project replays its scope commit as a restarted daemon would.
    #[test]
    fn forgetting_the_memo_for_a_test_forgets_the_scope_record_too() {
        let capture = SourceCapture::default();
        capture
            .ensured_scopes()
            .insert("incarnation\u{1f}project".to_owned());
        capture.forget_memo_for_test();
        assert!(capture.ensured_scopes().is_empty());
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

    #[test]
    fn a_settled_report_counts_the_job_that_released_the_slot() {
        let capture = Arc::new(SourceCapture::default());
        let job = Arc::clone(&capture.slot).try_acquire_owned().unwrap();
        let memo = capture.memo();
        let reader = {
            let capture = Arc::clone(&capture);
            std::thread::spawn(move || capture.settled_report(false, 0))
        };
        std::thread::sleep(Duration::from_millis(200));
        capture.counters.add(Count::UnitsPublished, 1);
        drop(job);
        drop(memo);
        let (report, idle) = reader.join().unwrap();
        if idle {
            assert_eq!(
                report["units_published"],
                json!(1),
                "an idle report missed the count of the job that released the slot while `report` waited on the memo lock"
            );
        }
    }
}
