//! JobTable retains bounded process-local batch jobs with deterministic idempotency, ephemeral retention, opaque incarnation-fenced identifiers, and replayable boundary-checked result cursors.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use sha2::{Digest, Sha256};

use super::SynapseLimits;
use crate::wire::ByteCharge;

pub(crate) const MAX_ITEM_ID_BYTES: usize = 256;
pub(crate) const CONTENT_SHA256_BYTES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchItem {
    pub id: String,
    pub content_sha256: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct JobDescriptor {
    pub job_id: String,
    pub status: &'static str,
}

pub enum AdmitOutcome {
    /// Same retained key with a byte-identical canonical payload.
    Existing(JobDescriptor),
    /// Same retained key with a conflicting payload.
    Conflict,
    /// New job admitted; the caller must start exactly one worker for it.
    Admitted { job_id: String, seq: u64 },
    /// Admission capacity is exhausted and nothing evictable remains.
    Full,
    /// This request's result cannot fit the retained-result byte cap.
    ResultTooLarge,
    /// Shutdown already closed admission.
    Closed,
}

pub enum PollOutcome {
    /// Unknown, foreign-incarnation, expired, or evicted job.
    Restarted,
    /// The supplied request_key does not belong to this job.
    KeyMismatch,
    Pending {
        status: &'static str,
    },
    Failed {
        code: String,
        message: String,
    },
    Page(ResultPage),
    BadCursor,
}

pub struct ResultPage {
    /// Shared backing lets concurrent polls serve retained vectors without
    /// copying them.
    pub vectors: Vec<(String, String, Arc<[f32]>)>,
    pub done: bool,
    pub next_cursor: Option<String>,
    /// Keeps the job's result bytes counted as live while this page holds its vectors, so an eviction during response construction cannot free capacity the vectors still occupy.
    pub lease: Arc<ResultLease>,
}

/// One completed job's result bytes, counted in the table's live total for as long as any holder (the job or a served page) keeps the vectors alive.
pub struct ResultLease {
    bytes: u64,
    live: Arc<AtomicU64>,
}

impl ResultLease {
    fn new(bytes: u64, live: Arc<AtomicU64>) -> Self {
        live.fetch_add(bytes, Ordering::Relaxed);
        Self { bytes, live }
    }
}

impl Drop for ResultLease {
    fn drop(&mut self) {
        self.live.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}

enum JobState {
    Queued {
        items: Vec<BatchItem>,
    },
    Running,
    Ready {
        vectors: Vec<Arc<[f32]>>,
        boundaries: Vec<usize>,
        /// Dropped with the job on eviction; pages served from the job hold clones, so the live total falls only when the last holder goes.
        lease: Arc<ResultLease>,
    },
    Failed {
        code: String,
        message: String,
    },
}

struct Job {
    seq: u64,
    key: String,
    payload_digest: [u8; 32],
    /// `(id, content_sha256)` in request order; texts live in `state` only
    /// until inference replaces them with vectors.
    item_meta: Vec<(String, String)>,
    text_bytes: u64,
    dimensions: usize,
    reserved_result_bytes: u64,
    result_bytes: u64,
    state: JobState,
    completed_at: Option<Instant>,
    /// Timestamp of the most recently served result page; `None` until one is served.
    last_polled_at: Option<Instant>,
    /// Resident-byte charge for this job's request inputs. Sized by
    /// [`job_input_bytes`] at admission, shrunk to [`Job::retained_input_bytes`]
    /// when the texts die, and released by dropping the job on removal,
    /// eviction, expiry, or clear.
    charge: ByteCharge,
}

impl Job {
    /// Logical request-input bytes still owned after the item texts die:
    /// both key copies and the id/hash metadata. String contents only;
    /// struct and hash-bucket overhead stay outside the accounting claim.
    fn retained_input_bytes(&self) -> usize {
        retained_input_bytes(
            self.key.len(),
            self.item_meta
                .iter()
                .map(|(id, hash)| (id.len(), hash.len())),
        )
    }

    fn status(&self) -> &'static str {
        match self.state {
            JobState::Queued { .. } => "queued",
            JobState::Running => "running",
            JobState::Ready { .. } => "ready",
            JobState::Failed { .. } => "failed",
        }
    }

    fn is_completed(&self) -> bool {
        matches!(self.state, JobState::Ready { .. } | JobState::Failed { .. })
    }

    /// Eviction rank uses last poll time, falling back to completion time; ties fall to the earlier completion.
    fn retention_rank(&self) -> (Option<Instant>, Option<Instant>) {
        (self.last_polled_at.or(self.completed_at), self.completed_at)
    }
}

struct Jobs {
    by_key: HashMap<String, u64>,
    by_seq: HashMap<u64, Job>,
    next_seq: u64,
    queued_text_bytes: u64,
    retained_result_bytes: u64,
    closed: bool,
}

impl Jobs {
    /// These counters sum live jobs' `text_bytes` and `result_bytes`; a larger release is an accounting bug. Saturation prevents integer wraparound in release builds.
    fn release_bytes(&mut self, text: u64, result: u64) {
        debug_assert!(
            self.queued_text_bytes >= text,
            "queued_text_bytes {} cannot release {text}",
            self.queued_text_bytes
        );
        debug_assert!(
            self.retained_result_bytes >= result,
            "retained_result_bytes {} cannot release {result}",
            self.retained_result_bytes
        );
        self.queued_text_bytes = self.queued_text_bytes.saturating_sub(text);
        self.retained_result_bytes = self.retained_result_bytes.saturating_sub(result);
    }
}

pub struct JobTable {
    limits: SynapseLimits,
    incarnation: String,
    /// Keys the authenticator in every issued cursor. A client can hold a valid cursor only by receiving it from this table, so a never-issued boundary cannot be fabricated even though its position is predictable.
    cursor_key: [u8; 32],
    inner: std::sync::Mutex<Jobs>,
    /// Result bytes still alive anywhere: retained by a job or held by a page being served. `Jobs::retained_result_bytes` counts only retained jobs and drives eviction; this total is what admission measures against the cap.
    live_result_bytes: Arc<AtomicU64>,
}

/// The serialized JSON string contains `s`'s escaped form, excluding its delimiting quotes.
pub(crate) fn escaped_string_bytes(s: &str) -> usize {
    serde_json::to_string(s)
        .expect("string serialization cannot fail")
        .len()
        .checked_sub(2)
        .expect("serialized JSON string includes quotes")
}

/// Only the canonical decimal spelling of a number resolves: no sign, no leading zeros, at most 20 digits.
/// Host-issued job IDs and cursors use this spelling, so any other spelling is a never-issued identifier.
fn parse_canonical_decimal(digits: &str) -> Option<u64> {
    if digits.is_empty() || digits.len() > 20 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if digits.len() > 1 && digits.starts_with('0') {
        return None;
    }
    digits.parse().ok()
}

/// While retained, a stored failure makes identical resubmissions report the same failure.
fn failure_is_permanent(code: &str) -> bool {
    matches!(
        code,
        "artifact_invalid"
            | "substitution_rejected"
            | "schema_violation"
            | "not_certified"
            | "probe_required"
            | "idempotency_conflict"
    )
}

/// `parse_batch` verifies each `content_sha256` against its text before admission, so hashing the verified hash commits to the text bytes without a second pass over them.
fn digest_payload(key: &str, items: &[BatchItem]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    let mut update = |bytes: &[u8]| {
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    };
    update(key.as_bytes());
    for item in items {
        update(item.id.as_bytes());
        update(item.content_sha256.as_bytes());
    }
    hasher.finalize().into()
}

pub(crate) fn job_input_bytes(key: &str, items: &[BatchItem]) -> usize {
    let mut bytes = 2usize.saturating_mul(key.len());
    for item in items {
        bytes = bytes
            .saturating_add(item.id.capacity())
            .saturating_add(item.content_sha256.capacity())
            .saturating_add(item.text.capacity())
            .saturating_add(item.id.len())
            .saturating_add(item.content_sha256.len());
    }
    bytes
}

/// The retained charge for a completed job: two key copies plus each item's id and hash lengths.
/// Startup validation sizes the retained-metadata pool from this same rule so the two cannot drift.
pub(crate) fn retained_input_bytes(
    key_len: usize,
    item_meta_lens: impl IntoIterator<Item = (usize, usize)>,
) -> usize {
    item_meta_lens.into_iter().fold(
        2usize.saturating_mul(key_len),
        |bytes, (id_len, hash_len)| bytes.saturating_add(id_len).saturating_add(hash_len),
    )
}

/// Collects `Job`s and `ByteCharge`s removed under the table lock so they drop after the guard releases; retained vectors and permits are then freed outside the lock.
#[derive(Default)]
struct Released {
    jobs: Vec<Job>,
    charges: Vec<ByteCharge>,
}

impl Released {
    fn job(&mut self, job: Option<Job>) {
        self.jobs.extend(job);
    }

    fn charge(&mut self, charge: ByteCharge) {
        if charge.bytes() > 0 {
            self.charges.push(charge);
        }
    }
}

fn result_bytes(item_count: usize, dimensions: usize, metadata_bytes: usize) -> Option<u64> {
    let vector_bytes = item_count
        .checked_mul(dimensions)?
        .checked_mul(std::mem::size_of::<f32>())?;
    vector_bytes
        .checked_add(metadata_bytes)
        .and_then(|bytes| u64::try_from(bytes).ok())
}

pub(crate) fn max_result_bytes(item_count: usize, dimensions: usize) -> Option<u64> {
    let metadata_bytes = item_count.checked_mul(MAX_ITEM_ID_BYTES + CONTENT_SHA256_BYTES)?;
    result_bytes(item_count, dimensions, metadata_bytes)
}

fn admitted_result_bytes(items: &[BatchItem], dimensions: usize) -> Option<u64> {
    let metadata_bytes = items.iter().try_fold(0usize, |total, item| {
        total
            .checked_add(item.id.len())?
            .checked_add(item.content_sha256.len())
    })?;
    result_bytes(items.len(), dimensions, metadata_bytes)
}

impl JobTable {
    /// The table-lock helper recovers from mutex poisoning.
    fn lock_jobs(&self) -> std::sync::MutexGuard<'_, Jobs> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn new(limits: SynapseLimits) -> Self {
        let mut nonce = [0u8; 8];
        // The incarnation fence must be unpredictable across restarts so stale job IDs cannot name live jobs.
        getrandom::getrandom(&mut nonce).expect("OS entropy for the job incarnation");
        let mut cursor_key = [0u8; 32];
        getrandom::getrandom(&mut cursor_key).expect("OS entropy for the cursor key");
        Self {
            limits,
            incarnation: nonce.iter().map(|b| format!("{b:02x}")).collect(),
            cursor_key,
            live_result_bytes: Arc::new(AtomicU64::new(0)),
            inner: std::sync::Mutex::new(Jobs {
                by_key: HashMap::new(),
                by_seq: HashMap::new(),
                next_seq: 1,
                queued_text_bytes: 0,
                retained_result_bytes: 0,
                closed: false,
            }),
        }
    }

    fn job_id(&self, seq: u64) -> String {
        format!("{}-{seq}", self.incarnation)
    }

    fn parse_job_id(&self, job_id: &str) -> Option<u64> {
        let (incarnation, seq) = job_id.split_once('-')?;
        if incarnation != self.incarnation {
            return None;
        }
        // Only canonical decimal representations resolve; `+1` and `007` must not resolve.
        // Reject noncanonical sequence numbers so each job has one valid ID.
        parse_canonical_decimal(seq)
    }

    pub fn key_is_retained(&self, key: &str) -> bool {
        // Declare permits before the table guard so permit release runs after the guard drops.
        let mut released = Released::default();
        let mut jobs = self.lock_jobs();
        self.sweep_expired(&mut jobs, &mut released);
        jobs.by_key.contains_key(key)
    }

    /// `admit_uncharged_for_tests` retains request input without charging it.
    #[doc(hidden)]
    pub fn admit_uncharged_for_tests(
        &self,
        key: String,
        items: Vec<BatchItem>,
        dimensions: usize,
    ) -> AdmitOutcome {
        self.admit_charged(key, items, dimensions, &mut ByteCharge::none())
    }

    /// `admit_charged` transfers `charge` only when it returns `Admitted`; all other outcomes leave it with the caller.
    pub(crate) fn admit_charged(
        &self,
        key: String,
        items: Vec<BatchItem>,
        dimensions: usize,
        charge: &mut ByteCharge,
    ) -> AdmitOutcome {
        let digest = digest_payload(&key, &items);
        let text_bytes: u64 = items.iter().map(|item| item.text.len() as u64).sum();
        let result_bytes = admitted_result_bytes(&items, dimensions);
        let input_bytes = job_input_bytes(&key, &items);
        // `released` is declared before `jobs` so it drops after the table lock.
        let mut released = Released::default();
        let mut jobs = self.lock_jobs();
        if jobs.closed {
            return AdmitOutcome::Closed;
        }
        self.sweep_expired(&mut jobs, &mut released);

        // `failed_replay` holds a same-digest retryable failure; eviction waits until admission succeeds so rejected retries leave it pollable.
        let mut failed_replay = None;
        if let Some(seq) = jobs.by_key.get(&key).copied() {
            let job = jobs.by_seq.get(&seq).expect("keyed job exists");
            if job.payload_digest != digest {
                return AdmitOutcome::Conflict;
            }
            // An identical re-submission replaces a retryable failure; a permanent failure returns `Existing`.
            match &job.state {
                JobState::Failed { code, .. } if !failure_is_permanent(code) => {
                    failed_replay = Some(seq);
                }
                _ => {
                    return AdmitOutcome::Existing(JobDescriptor {
                        job_id: self.job_id(seq),
                        status: job.status(),
                    });
                }
            }
        }

        let Some(result_bytes) = result_bytes else {
            return AdmitOutcome::ResultTooLarge;
        };
        if result_bytes > self.limits.max_retained_result_bytes {
            return AdmitOutcome::ResultTooLarge;
        }

        let admitted = jobs
            .by_seq
            .values()
            .filter(|job| !job.is_completed())
            .count();
        if admitted >= self.limits.max_queued_jobs
            || jobs.queued_text_bytes.saturating_add(text_bytes)
                > self.limits.max_queued_request_bytes
        {
            // Queued and running jobs are never evicted; a full admission class rejects new work.
            return AdmitOutcome::Full;
        }

        // Result capacity is reserved here, before inference allocates the vectors, so retained bytes plus every in-flight reservation stay within the cap the lane declares to the host; publishing a result can then never overshoot it.
        if !self.reserve_result_bytes(&mut jobs, result_bytes, &mut released) {
            return AdmitOutcome::Full;
        }

        // `failed_replay` is removed before insertion so `by_key` never overwrites a live job's index entry.
        if let Some(seq) = failed_replay {
            released.job(Self::remove(&mut jobs, seq));
        }

        let seq = jobs.next_seq;
        jobs.next_seq += 1;
        let item_meta = items
            .iter()
            .map(|item| (item.id.clone(), item.content_sha256.clone()))
            .collect();
        jobs.queued_text_bytes += text_bytes;
        jobs.by_key.insert(key.clone(), seq);
        jobs.by_seq.insert(
            seq,
            Job {
                seq,
                key,
                payload_digest: digest,
                item_meta,
                text_bytes,
                dimensions,
                reserved_result_bytes: result_bytes,
                result_bytes: 0,
                state: JobState::Queued { items },
                completed_at: None,
                last_polled_at: None,
                charge: charge.split_or_take(input_bytes),
            },
        );
        AdmitOutcome::Admitted {
            job_id: self.job_id(seq),
            seq,
        }
    }

    pub fn start(&self, seq: u64) -> Option<Vec<BatchItem>> {
        let mut jobs = self.lock_jobs();
        let job = jobs.by_seq.get_mut(&seq)?;
        let JobState::Queued { items } = &mut job.state else {
            return None;
        };
        let items = std::mem::take(items);
        job.state = JobState::Running;
        Some(items)
    }

    pub fn publish_ready(&self, seq: u64, vectors: Vec<Vec<f32>>) {
        // Convert vectors before locking so large inference results do not block admission, polling, or shutdown bookkeeping.
        let vectors: Vec<Arc<[f32]>> = vectors.into_iter().map(Arc::from).collect();
        // `released` drops after `jobs`, so its permits are released outside the table lock.
        let mut released = Released::default();
        let mut jobs = self.lock_jobs();
        let Some(job) = jobs.by_seq.get_mut(&seq) else {
            return;
        };
        // A late publication cannot alter a completed job's result or byte accounting.
        if job.is_completed() {
            return;
        }
        // Reject mismatched item counts before positional pairing to avoid panicking while `jobs` is locked.
        if vectors.len() != job.item_meta.len() {
            self.fail_job(
                &mut jobs,
                seq,
                "artifact_invalid".to_owned(),
                "inference returned a different item count".to_owned(),
                &mut released,
            );
            return;
        }
        if vectors.iter().any(|vector| vector.len() != job.dimensions) {
            self.fail_job(
                &mut jobs,
                seq,
                "artifact_invalid".to_owned(),
                "inference returned a different vector dimension".to_owned(),
                &mut released,
            );
            return;
        }
        let result_bytes = job.reserved_result_bytes;
        let text_bytes = job.text_bytes;
        job.text_bytes = 0;

        let boundaries = self.page_boundaries(&job.item_meta, &vectors);
        job.state = JobState::Ready {
            vectors,
            boundaries,
            lease: Arc::new(ResultLease::new(
                result_bytes,
                Arc::clone(&self.live_result_bytes),
            )),
        };
        job.result_bytes = result_bytes;
        job.completed_at = Some(Instant::now());
        // When a job becomes ready, retain charges only for its key and metadata and release queued-text permits after unlocking.
        let retained = job.retained_input_bytes();
        let excess = job.charge.split_excess(retained);
        released.charge(excess);
        jobs.release_bytes(text_bytes, 0);
        jobs.retained_result_bytes += result_bytes;
        self.enforce_retention(&mut jobs, Some(seq), &mut released);
    }

    pub fn publish_failed(&self, seq: u64, code: String, message: String) {
        // `released` drops after `jobs`, so its permits are released outside the table lock.
        let mut released = Released::default();
        let mut jobs = self.lock_jobs();
        self.fail_job(&mut jobs, seq, code, message, &mut released);
    }

    /// A late abandonment guard leaves a completed job's result and byte accounting unchanged.
    fn fail_job(
        &self,
        jobs: &mut Jobs,
        seq: u64,
        code: String,
        message: String,
        released: &mut Released,
    ) {
        let Some(job) = jobs.by_seq.get_mut(&seq) else {
            return;
        };
        if job.is_completed() {
            return;
        }
        let text_bytes = job.text_bytes;
        job.text_bytes = 0;
        // The failure message is retained uncharged for the retention window, so it is bounded to the diagnostic cap the wire enforces anyway.
        let mut message = message;
        super::protocol::bound_diagnostic(&mut message);
        job.state = JobState::Failed { code, message };
        job.completed_at = Some(Instant::now());
        // Failure drops queued items and worker-owned texts.
        let retained = job.retained_input_bytes();
        let excess = job.charge.split_excess(retained);
        released.charge(excess);
        jobs.release_bytes(text_bytes, 0);
        self.enforce_retention(jobs, Some(seq), released);
    }

    pub fn poll(&self, job_id: &str, key: &str, cursor: Option<&str>) -> PollOutcome {
        // `released` drops after `jobs`, so its permits are released outside the table lock.
        let mut released = Released::default();
        let mut jobs = self.lock_jobs();
        self.sweep_expired(&mut jobs, &mut released);
        let Some(seq) = self.parse_job_id(job_id) else {
            return PollOutcome::Restarted;
        };
        let Some(job) = jobs.by_seq.get_mut(&seq) else {
            return PollOutcome::Restarted;
        };
        if job.key != key {
            return PollOutcome::KeyMismatch;
        }
        // Only a ready job has ever issued a cursor, so any cursor on a queued, running, or failed job is never-issued.
        if cursor.is_some() && !matches!(job.state, JobState::Ready { .. }) {
            return PollOutcome::BadCursor;
        }
        match &job.state {
            JobState::Queued { .. } | JobState::Running => PollOutcome::Pending {
                status: job.status(),
            },
            JobState::Failed { code, message } => PollOutcome::Failed {
                code: code.clone(),
                message: message.clone(),
            },
            JobState::Ready {
                vectors,
                boundaries,
                lease,
            } => {
                let offset = match cursor {
                    None => 0,
                    Some(cursor) => match self.parse_cursor(cursor, seq, boundaries) {
                        Some(offset) => offset,
                        None => return PollOutcome::BadCursor,
                    },
                };
                let next_boundary = boundaries
                    .iter()
                    .copied()
                    .find(|b| *b > offset)
                    .unwrap_or(vectors.len());
                let page = (offset..next_boundary)
                    .map(|index| {
                        let (id, hash) = &job.item_meta[index];
                        (id.clone(), hash.clone(), Arc::clone(&vectors[index]))
                    })
                    .collect();
                let done = next_boundary >= vectors.len();
                let next_cursor = (!done).then(|| self.cursor(seq, next_boundary));
                // A served page marks the job as in use so retention prefers evicting jobs nobody is reading.
                job.last_polled_at = Some(Instant::now());
                PollOutcome::Page(ResultPage {
                    vectors: page,
                    done,
                    next_cursor,
                    lease: Arc::clone(lease),
                })
            }
        }
    }

    /// Cursors are `<job_id>:<boundary>:<authenticator>`. The authenticator is a keyed digest over the job and boundary, so only a cursor this table issued verifies; a client cannot name a legal page it has never been handed, and every issued cursor, including one whose response was lost, replays its page.
    fn cursor(&self, seq: u64, boundary: usize) -> String {
        format!(
            "{}:{boundary}:{}",
            self.job_id(seq),
            self.cursor_authenticator(seq, boundary)
        )
    }

    fn cursor_authenticator(&self, seq: u64, boundary: usize) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.cursor_key);
        hasher.update(seq.to_le_bytes());
        hasher.update((boundary as u64).to_le_bytes());
        hasher
            .finalize()
            .iter()
            .take(16)
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// Accepts a cursor only when it names this job, its boundary is a page start, and its authenticator matches the one this table issues for that boundary.
    /// The boundary uses the same canonical-decimal rule as the job sequence so a never-issued spelling such as `+16` or `0016` is rejected.
    fn parse_cursor(&self, cursor: &str, seq: u64, boundaries: &[usize]) -> Option<usize> {
        let (rest, authenticator) = cursor.rsplit_once(':')?;
        let (job_id, offset) = rest.rsplit_once(':')?;
        if self.parse_job_id(job_id) != Some(seq) {
            return None;
        }
        let offset = usize::try_from(parse_canonical_decimal(offset)?).ok()?;
        (boundaries.contains(&offset) && authenticator == self.cursor_authenticator(seq, offset))
            .then_some(offset)
    }

    /// `MAX_F32_JSON_BYTES` is the longest `serde_json` encoding of a finite `f32`, e.g. `-0.0000010000001`.
    /// `f32_json_encoding_fits_the_component_budget` pins `MAX_F32_JSON_BYTES` against `serde_json`.
    // commentlint: allow(JUDGE)
    pub(crate) const MAX_F32_JSON_BYTES: usize = 16;
    /// Reserve worst-case JSON bytes for one `f32` component plus its `,` separator.
    const ENCODED_BYTES_PER_COMPONENT: usize = Self::MAX_F32_JSON_BYTES + 1;
    /// Charge the fixed JSON envelope for each vector item (field names, quotes, separators).
    const ENCODED_ITEM_OVERHEAD: usize = 64;

    /// Overestimate each result item's JSON size: undercounting can create a page whose body exceeds the frame limit, and no cursor could serve that page.
    pub(crate) fn encoded_item_cost(vector_len: usize, id: &str, hash: &str) -> usize {
        debug_assert!(
            hash.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "content hash must be hexadecimal"
        );
        vector_len
            .checked_mul(Self::ENCODED_BYTES_PER_COMPONENT)
            .and_then(|bytes| bytes.checked_add(escaped_string_bytes(id)))
            .and_then(|bytes| bytes.checked_add(hash.len()))
            .and_then(|bytes| bytes.checked_add(Self::ENCODED_ITEM_OVERHEAD))
            .unwrap_or(usize::MAX)
    }

    fn page_boundaries(
        &self,
        item_meta: &[(String, String)],
        vectors: &[Arc<[f32]>],
    ) -> Vec<usize> {
        let mut boundaries = Vec::new();
        let mut count_in_page = 0usize;
        let mut bytes_in_page = 0usize;
        for (index, vector) in vectors.iter().enumerate() {
            let (id, hash) = &item_meta[index];
            let encoded = Self::encoded_item_cost(vector.len(), id, hash);
            if count_in_page > 0
                && (count_in_page >= self.limits.max_page_vectors
                    || bytes_in_page
                        .checked_add(encoded)
                        .is_none_or(|bytes| bytes > self.limits.max_page_encoded_bytes))
            {
                boundaries.push(index);
                count_in_page = 0;
                bytes_in_page = 0;
            }
            count_in_page += 1;
            bytes_in_page = bytes_in_page.saturating_add(encoded);
        }
        boundaries
    }

    pub fn sweep(&self) {
        // Declare `released` before `jobs` so dropping `released` releases permits after the table lock.
        let mut released = Released::default();
        let mut jobs = self.lock_jobs();
        self.sweep_expired(&mut jobs, &mut released);
    }

    fn sweep_expired(&self, jobs: &mut Jobs, released: &mut Released) {
        let now = Instant::now();
        let expired: Vec<u64> = jobs
            .by_seq
            .values()
            .filter(|job| {
                job.completed_at
                    .is_some_and(|at| now.duration_since(at) >= self.limits.retention)
            })
            .map(|job| job.seq)
            .collect();
        for seq in expired {
            released.job(Self::remove(jobs, seq));
        }
    }

    /// The job identified by `keep` remains exempt while another completed job is eligible for eviction.
    /// Victims are ordered by [`Job::retention_rank`].
    fn enforce_retention(&self, jobs: &mut Jobs, keep: Option<u64>, released: &mut Released) {
        loop {
            let retained = jobs
                .by_seq
                .values()
                .filter(|job| job.is_completed())
                .count();
            let over_count = retained > self.limits.max_retained_jobs;
            let over_bytes = jobs.retained_result_bytes > self.limits.max_retained_result_bytes;
            if !over_count && !over_bytes {
                return;
            }
            let victim = jobs
                .by_seq
                .values()
                .filter(|job| job.is_completed() && Some(job.seq) != keep)
                .min_by_key(|job| job.retention_rank())
                .map(|job| job.seq);
            let Some(seq) = victim else {
                // Evicting the protected job prevents a single oversized result from permanently exceeding a cap.
                let Some(seq) = keep else { return };
                released.job(Self::remove(jobs, seq));
                return;
            };
            released.job(Self::remove(jobs, seq));
        }
    }

    /// Evicts completed jobs, oldest by [`Job::retention_rank`] first, until `result_bytes` fits beside every live result and every queued or running job's reservation.
    /// Bytes that eviction cannot free are checked first: in-flight reservations, the new result, and vectors of already-evicted jobs that a served page still holds. When those alone exceed the cap, nothing is evicted and the caller reports admission as full.
    fn reserve_result_bytes(
        &self,
        jobs: &mut Jobs,
        result_bytes: u64,
        released: &mut Released,
    ) -> bool {
        let cap = self.limits.max_retained_result_bytes;
        let in_flight: u64 = jobs
            .by_seq
            .values()
            .filter(|job| !job.is_completed())
            .map(|job| job.reserved_result_bytes)
            .fold(0, u64::saturating_add);
        let floor = in_flight.saturating_add(result_bytes);
        // Jobs already swept or evicted into `released` drop after the table lock, so their still-counted bytes are subtracted here by hand until then.
        let mut releasing = released
            .jobs
            .iter()
            .filter(|job| Self::eviction_frees_bytes(job))
            .fold(0u64, |total, job| total.saturating_add(job.result_bytes));
        loop {
            let live = self
                .live_result_bytes
                .load(Ordering::Relaxed)
                .saturating_sub(releasing);
            if live.saturating_add(floor) <= cap {
                return true;
            }
            // Only a ready job whose lease no served page shares frees bytes when evicted; a page-leased job or a zero-byte failure record would be lost for nothing.
            let releasable = jobs
                .by_seq
                .values()
                .filter(|job| Self::eviction_frees_bytes(job))
                .fold(0u64, |total, job| total.saturating_add(job.result_bytes));
            if live.saturating_sub(releasable).saturating_add(floor) > cap {
                return false;
            }
            let victim = jobs
                .by_seq
                .values()
                .filter(|job| Self::eviction_frees_bytes(job))
                .min_by_key(|job| job.retention_rank())
                .map(|job| job.seq);
            let Some(seq) = victim else {
                return false;
            };
            let job = Self::remove(jobs, seq);
            releasing = releasing.saturating_add(job.as_ref().map_or(0, |job| job.result_bytes));
            released.job(job);
        }
    }

    /// A ready job with result bytes whose lease has no other holder releases those bytes on eviction; a page mid-response holds a clone and keeps them live, and a failed job holds none.
    fn eviction_frees_bytes(job: &Job) -> bool {
        match &job.state {
            JobState::Ready { lease, .. } => job.result_bytes > 0 && Arc::strong_count(lease) == 1,
            _ => false,
        }
    }

    fn remove(jobs: &mut Jobs, seq: u64) -> Option<Job> {
        let job = jobs.by_seq.remove(&seq)?;
        jobs.release_bytes(job.text_bytes, job.result_bytes);
        jobs.by_key.remove(&job.key);
        Some(job)
    }

    pub fn close_admission(&self) {
        // Declare `released` before `jobs` so shutdown releases charges after dropping the table lock.
        let mut released = Released::default();
        let mut jobs = self.lock_jobs();
        jobs.closed = true;
        let queued: Vec<u64> = jobs
            .by_seq
            .values()
            .filter(|job| matches!(job.state, JobState::Queued { .. }))
            .map(|job| job.seq)
            .collect();
        for seq in queued {
            released.job(Self::remove(&mut jobs, seq));
        }
    }

    pub fn clear(&self) {
        // Declare `released` before `jobs` so drained jobs free after the table lock.
        let mut released = Released::default();
        let mut jobs = self.lock_jobs();
        released
            .jobs
            .extend(jobs.by_seq.drain().map(|(_, job)| job));
        jobs.by_key.clear();
        jobs.queued_text_bytes = 0;
        jobs.retained_result_bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::ByteBudget;

    fn charged_item(id: &str, text: &str) -> BatchItem {
        BatchItem {
            id: id.to_owned(),
            content_sha256: "0".repeat(64),
            text: text.to_owned(),
        }
    }

    fn retained_bytes(key: &str, items: &[BatchItem]) -> usize {
        let meta: usize = items
            .iter()
            .map(|item| item.id.len() + item.content_sha256.len())
            .sum();
        2 * key.len() + meta
    }

    #[test]
    fn a_charged_job_transfers_shrinks_and_releases_exact_permits() {
        const POOL: usize = 1_000_000;
        let budget = ByteBudget::new(POOL as u64);
        let jobs = JobTable::new(SynapseLimits::default());
        let items = vec![charged_item("a", "alpha"), charged_item("b", "beta")];
        let key = "k".repeat(64);
        let job_bytes = job_input_bytes(&key, &items);
        let retained = retained_bytes(&key, &items);
        assert!(job_bytes > retained);

        let mut candidate = budget.try_charge(job_bytes + 500).expect("candidate");
        let AdmitOutcome::Admitted { seq, .. } =
            jobs.admit_charged(key.clone(), items.clone(), 4, &mut candidate)
        else {
            panic!("admitted");
        };
        assert_eq!(
            candidate.bytes(),
            500,
            "admission takes exactly the job-sized portion"
        );
        drop(candidate);
        assert_eq!(budget.available(), POOL - job_bytes);

        let taken = jobs.start(seq).expect("starts");
        assert_eq!(
            budget.available(),
            POOL - job_bytes,
            "the job keeps the whole charge while the worker owns the items"
        );

        jobs.publish_ready(seq, vec![vec![0.0; 4]; taken.len()]);
        assert_eq!(
            budget.available(),
            POOL - retained,
            "publication shrinks the charge to retained metadata"
        );

        jobs.clear();
        assert_eq!(budget.available(), POOL);
    }

    #[test]
    fn non_admitted_outcomes_leave_the_candidate_charge_with_the_caller() {
        const POOL: usize = 1_000_000;
        let budget = ByteBudget::new(POOL as u64);
        let jobs = JobTable::new(SynapseLimits::default());
        let key = "k".repeat(64);
        let items = vec![charged_item("a", "alpha")];
        let job_bytes = job_input_bytes(&key, &items);

        let mut first = budget.try_charge(job_bytes).expect("first");
        let AdmitOutcome::Admitted { .. } =
            jobs.admit_charged(key.clone(), items.clone(), 4, &mut first)
        else {
            panic!("admitted");
        };

        let mut replay = budget.try_charge(job_bytes).expect("replay");
        assert!(matches!(
            jobs.admit_charged(key.clone(), items.clone(), 4, &mut replay),
            AdmitOutcome::Existing(_)
        ));
        assert_eq!(
            replay.bytes(),
            job_bytes,
            "replay keeps its candidate charge"
        );
        drop(replay);

        let other = vec![charged_item("b", "different")];
        let mut conflict = budget.try_charge(1_000).expect("conflict");
        assert!(matches!(
            jobs.admit_charged(key.clone(), other, 4, &mut conflict),
            AdmitOutcome::Conflict
        ));
        assert_eq!(
            conflict.bytes(),
            1_000,
            "conflict keeps its candidate charge"
        );
        drop(conflict);

        jobs.close_admission();
        let mut closed = budget.try_charge(1_000).expect("closed");
        assert!(matches!(
            jobs.admit_charged(
                "x".repeat(64),
                vec![charged_item("c", "gamma")],
                4,
                &mut closed
            ),
            AdmitOutcome::Closed
        ));
        assert_eq!(closed.bytes(), 1_000, "closed keeps its candidate charge");
        drop(closed);

        assert_eq!(
            budget.available(),
            POOL,
            "close_admission dropped the queued job and its whole charge"
        );
    }

    #[test]
    fn failure_eviction_and_expiry_release_their_charges() {
        const POOL: usize = 1_000_000;
        let budget = ByteBudget::new(POOL as u64);
        let limits = SynapseLimits {
            max_retained_jobs: 1,
            ..SynapseLimits::default()
        };
        let jobs = JobTable::new(limits);

        let admit = |key: String, items: Vec<BatchItem>| {
            let bytes = job_input_bytes(&key, &items);
            let mut charge = budget.try_charge(bytes).expect("charge");
            let AdmitOutcome::Admitted { seq, .. } = jobs.admit_charged(key, items, 4, &mut charge)
            else {
                panic!("admitted");
            };
            seq
        };

        let first_key = "f".repeat(64);
        let first_items = vec![charged_item("a", "alpha")];
        let first = admit(first_key.clone(), first_items.clone());
        jobs.publish_failed(first, "artifact_invalid".to_owned(), "boom".to_owned());
        assert_eq!(
            budget.available(),
            POOL - retained_bytes(&first_key, &first_items),
            "a failed job that never started still shrinks to retained metadata"
        );

        jobs.publish_failed(first, "artifact_invalid".to_owned(), "late".to_owned());
        assert_eq!(
            budget.available(),
            POOL - retained_bytes(&first_key, &first_items),
            "a late abandonment settlement cannot alter completed accounting"
        );

        let second_key = "e".repeat(64);
        let second_items = vec![charged_item("b", "beta")];
        let second = admit(second_key.clone(), second_items.clone());
        jobs.start(second).expect("starts");
        jobs.publish_ready(second, vec![vec![0.0; 4]]);
        assert_eq!(
            budget.available(),
            POOL - retained_bytes(&second_key, &second_items),
            "eviction of the oldest retained job released its remaining charge"
        );

        jobs.clear();
        assert_eq!(budget.available(), POOL);
    }

    #[test]
    fn sweep_releases_expired_charges_without_a_request_path() {
        const POOL: usize = 1_000_000;
        let budget = ByteBudget::new(POOL as u64);
        let limits = SynapseLimits {
            retention: std::time::Duration::ZERO,
            ..SynapseLimits::default()
        };
        let jobs = JobTable::new(limits);

        let key = "s".repeat(64);
        let items = vec![charged_item("a", "alpha")];
        let mut charge = budget
            .try_charge(job_input_bytes(&key, &items))
            .expect("charge");
        let AdmitOutcome::Admitted { seq, .. } = jobs.admit_charged(key, items, 4, &mut charge)
        else {
            panic!("admitted");
        };
        jobs.publish_failed(seq, "artifact_invalid".to_owned(), "boom".to_owned());
        assert!(budget.available() < POOL, "the retained charge is held");

        jobs.sweep();
        assert_eq!(
            budget.available(),
            POOL,
            "sweep released the expired job's charge"
        );
    }

    #[test]
    fn an_identical_retry_replaces_a_failed_job() {
        const POOL: usize = 1_000_000;
        let budget = ByteBudget::new(POOL as u64);
        let jobs = JobTable::new(SynapseLimits::default());
        let key = "r".repeat(64);
        let items = vec![charged_item("a", "alpha")];
        let job_bytes = job_input_bytes(&key, &items);

        let mut first = budget.try_charge(job_bytes).expect("charge");
        let AdmitOutcome::Admitted { seq, .. } =
            jobs.admit_charged(key.clone(), items.clone(), 4, &mut first)
        else {
            panic!("admitted");
        };
        jobs.publish_failed(seq, "internal_error".to_owned(), "worker died".to_owned());

        let mut rejected = budget.try_charge(job_bytes).expect("rejected charge");
        assert!(matches!(
            jobs.admit_charged(key.clone(), items.clone(), usize::MAX, &mut rejected),
            AdmitOutcome::ResultTooLarge
        ));
        drop(rejected);
        assert!(
            jobs.key_is_retained(&key),
            "a bounced retry leaves the failed job retained"
        );

        let mut second = budget.try_charge(job_bytes).expect("recharge");
        let AdmitOutcome::Admitted { seq: retry_seq, .. } =
            jobs.admit_charged(key.clone(), items.clone(), 4, &mut second)
        else {
            panic!("retry admitted");
        };
        assert_ne!(seq, retry_seq, "the retry is a new job");
        assert_eq!(
            budget.available(),
            POOL - job_bytes,
            "the failed job's retained charge was released with its eviction"
        );

        // A permanent failure replays as `Existing` instead of re-running inference.
        jobs.publish_failed(
            retry_seq,
            "schema_violation".to_owned(),
            "bad input".to_owned(),
        );
        let mut third = budget.try_charge(job_bytes).expect("third charge");
        assert!(
            matches!(
                jobs.admit_charged(key.clone(), items.clone(), 4, &mut third),
                AdmitOutcome::Existing(descriptor) if descriptor.status == "failed"
            ),
            "a permanent failure replays rather than re-running"
        );
        drop(third);

        jobs.clear();
        assert_eq!(budget.available(), POOL);
    }

    #[test]
    fn ready_polls_share_the_retained_vector_allocation() {
        let jobs = JobTable::new(SynapseLimits::default());
        let batch = vec![BatchItem {
            id: "large-vector".to_owned(),
            content_sha256: "0".repeat(64),
            text: "alpha".to_owned(),
        }];
        let AdmitOutcome::Admitted { job_id, seq } =
            jobs.admit_uncharged_for_tests("key".to_owned(), batch, 256 * 1024)
        else {
            panic!("job is admitted");
        };
        jobs.start(seq).expect("job starts");
        jobs.publish_ready(seq, vec![vec![0.5; 256 * 1024]]);

        let PollOutcome::Page(first) = jobs.poll(&job_id, "key", None) else {
            panic!("first poll returns a page");
        };
        let second = jobs.poll(&job_id, "key", None);
        let PollOutcome::Page(second) = second else {
            panic!("second poll returns a page");
        };
        assert!(
            Arc::ptr_eq(&first.vectors[0].2, &second.vectors[0].2),
            "ready polls must not allocate or clone vector elements"
        );

        jobs.clear();
        assert_eq!(first.vectors[0].2.len(), 256 * 1024);
        assert_eq!(second.vectors[0].2[0], 0.5);
    }

    #[test]
    fn a_late_publish_ready_leaves_a_completed_job_unchanged() {
        let jobs = JobTable::new(SynapseLimits::default());
        let item = |id: &str| vec![charged_item(id, "alpha")];

        let AdmitOutcome::Admitted { job_id, seq } =
            jobs.admit_uncharged_for_tests("ready".to_owned(), item("a"), 2)
        else {
            panic!("ready job is admitted");
        };
        jobs.start(seq).expect("ready job starts");
        jobs.publish_ready(seq, vec![vec![0.5, 0.5]]);
        let retained = jobs.lock_jobs().retained_result_bytes;
        assert!(retained > 0, "publication charges retained result bytes");

        jobs.publish_ready(seq, vec![vec![1.0, 0.0]]);
        assert_eq!(
            jobs.lock_jobs().retained_result_bytes,
            retained,
            "a second publication is a no-op on accounting"
        );
        let PollOutcome::Page(page) = jobs.poll(&job_id, "ready", None) else {
            panic!("the first result is still served");
        };
        assert_eq!(&page.vectors[0].2[..], &[0.5, 0.5]);

        let AdmitOutcome::Admitted { job_id, seq } =
            jobs.admit_uncharged_for_tests("failed".to_owned(), item("b"), 2)
        else {
            panic!("failed job is admitted");
        };
        jobs.start(seq).expect("failed job starts");
        jobs.publish_failed(seq, "internal_error".to_owned(), "worker died".to_owned());
        jobs.publish_ready(seq, vec![vec![0.5, 0.5]]);
        assert!(
            matches!(
                jobs.poll(&job_id, "failed", None),
                PollOutcome::Failed { code, .. } if code == "internal_error"
            ),
            "a publication after failure does not resurrect the job"
        );
        assert_eq!(
            jobs.lock_jobs().retained_result_bytes,
            retained,
            "a failed job never charges result bytes"
        );
    }

    #[test]
    fn f32_json_encoding_fits_the_component_budget() {
        // Exponent and fixed-point boundary values for the linked serializer.
        let boundary_values = [
            -1.000_000_1e-6_f32,
            -1.000_000_1e-5,
            -0.000_100_000_01,
            -1.175_494_4e-38,
            -3.402_823_5e38,
            f32::MIN_POSITIVE,
            -f32::MIN_POSITIVE,
            f32::MAX,
            f32::MIN,
            f32::from_bits(1),
            -f32::from_bits(1),
        ];
        for value in boundary_values {
            let encoded = serde_json::to_string(&value).expect("f32 serializes");
            assert!(
                encoded.len() <= JobTable::MAX_F32_JSON_BYTES,
                "{value:?} encodes as {encoded} ({} bytes), above MAX_F32_JSON_BYTES",
                encoded.len()
            );
        }
        let longest = serde_json::to_string(&-1.000_000_1e-6_f32).expect("f32 serializes");
        assert_eq!(
            longest.len(),
            JobTable::MAX_F32_JSON_BYTES,
            "the budget must be tight: {longest}"
        );
    }

    #[test]
    fn result_byte_boundary_keeps_accepted_job_and_rejects_oversize_before_start() {
        let dimensions = 2;
        let exact_bytes =
            result_bytes(1, dimensions, 1 + CONTENT_SHA256_BYTES).expect("small result size");
        let limits = SynapseLimits {
            max_retained_result_bytes: exact_bytes,
            ..SynapseLimits::default()
        };
        let jobs = JobTable::new(limits);
        let item = |id: &str| {
            vec![BatchItem {
                id: id.to_owned(),
                content_sha256: "0".repeat(CONTENT_SHA256_BYTES),
                text: "alpha".to_owned(),
            }]
        };

        let AdmitOutcome::Admitted { job_id, seq } =
            jobs.admit_uncharged_for_tests("exact".to_owned(), item("a"), dimensions)
        else {
            panic!("exact result is admitted");
        };
        jobs.start(seq).expect("exact job starts");
        jobs.publish_ready(seq, vec![vec![0.5; dimensions]]);
        assert!(matches!(
            jobs.poll(&job_id, "exact", None),
            PollOutcome::Page(ResultPage { done: true, .. })
        ));

        assert!(matches!(
            jobs.admit_uncharged_for_tests("oversize".to_owned(), item("ab"), dimensions),
            AdmitOutcome::ResultTooLarge
        ));
        assert!(!jobs.key_is_retained("oversize"));
    }

    #[test]
    fn admission_reserves_result_capacity_before_inference_allocates_it() {
        let dimensions = 2;
        let one_result =
            result_bytes(1, dimensions, 1 + CONTENT_SHA256_BYTES).expect("small result size");
        let limits = SynapseLimits {
            max_retained_result_bytes: one_result,
            ..SynapseLimits::default()
        };
        let jobs = JobTable::new(limits);
        let item = |id: &str| vec![charged_item(id, "alpha")];

        let AdmitOutcome::Admitted {
            job_id: first_id,
            seq: first,
        } = jobs.admit_uncharged_for_tests("first".to_owned(), item("a"), dimensions)
        else {
            panic!("first job is admitted");
        };
        jobs.start(first).expect("first job starts");
        jobs.publish_ready(first, vec![vec![0.5; dimensions]]);
        assert!(matches!(
            jobs.poll(&first_id, "first", None),
            PollOutcome::Page(_)
        ));

        // The retained result fills the cap, so admitting a second job evicts it before any inference runs.
        let AdmitOutcome::Admitted { seq: second, .. } =
            jobs.admit_uncharged_for_tests("second".to_owned(), item("b"), dimensions)
        else {
            panic!("second job is admitted by evicting the retained result");
        };
        assert!(
            matches!(jobs.poll(&first_id, "first", None), PollOutcome::Restarted),
            "the retained result was evicted at admission"
        );
        assert_eq!(jobs.lock_jobs().retained_result_bytes, 0);

        // With the cap held by an in-flight reservation and nothing completed to evict, admission is full.
        assert!(matches!(
            jobs.admit_uncharged_for_tests("third".to_owned(), item("c"), dimensions),
            AdmitOutcome::Full
        ));
        jobs.start(second).expect("second job starts");
        jobs.publish_ready(second, vec![vec![0.5; dimensions]]);
        assert_eq!(jobs.lock_jobs().retained_result_bytes, one_result);
    }

    #[test]
    fn a_page_leased_result_is_not_evicted_while_its_page_is_served() {
        let dimensions = 2;
        let one_result =
            result_bytes(1, dimensions, 1 + CONTENT_SHA256_BYTES).expect("small result size");
        let limits = SynapseLimits {
            max_retained_result_bytes: one_result,
            ..SynapseLimits::default()
        };
        let jobs = JobTable::new(limits);
        let item = |id: &str| vec![charged_item(id, "alpha")];

        let AdmitOutcome::Admitted {
            job_id: first_id,
            seq: first,
        } = jobs.admit_uncharged_for_tests("first".to_owned(), item("a"), dimensions)
        else {
            panic!("first job is admitted");
        };
        jobs.start(first).expect("first job starts");
        jobs.publish_ready(first, vec![vec![0.5; dimensions]]);
        let PollOutcome::Page(page) = jobs.poll(&first_id, "first", None) else {
            panic!("the result is served");
        };

        // The served page holds the job's vectors, so evicting the job could not free its bytes: admission is full and the job stays retained for its reader.
        assert!(matches!(
            jobs.admit_uncharged_for_tests("second".to_owned(), item("b"), dimensions),
            AdmitOutcome::Full
        ));
        assert!(matches!(
            jobs.poll(&first_id, "first", None),
            PollOutcome::Page(_)
        ));
        assert_eq!(jobs.live_result_bytes.load(Ordering::Relaxed), one_result);

        // Once the page is gone the job is an ordinary eviction candidate again.
        drop(page);
        assert_eq!(jobs.live_result_bytes.load(Ordering::Relaxed), one_result);
        assert!(matches!(
            jobs.admit_uncharged_for_tests("second".to_owned(), item("b"), dimensions),
            AdmitOutcome::Admitted { .. }
        ));
        assert!(matches!(
            jobs.poll(&first_id, "first", None),
            PollOutcome::Restarted
        ));
        assert_eq!(jobs.live_result_bytes.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn an_unsatisfiable_result_reservation_evicts_nothing() {
        let dimensions = 2;
        let two_results =
            result_bytes(2, dimensions, 2 * (1 + CONTENT_SHA256_BYTES)).expect("two-item size");
        let limits = SynapseLimits {
            max_retained_result_bytes: two_results,
            ..SynapseLimits::default()
        };
        let jobs = JobTable::new(limits);
        let item = |id: &str| vec![charged_item(id, "alpha")];

        // One retained result and one in-flight reservation share the cap.
        let AdmitOutcome::Admitted {
            job_id: retained_id,
            seq: retained,
        } = jobs.admit_uncharged_for_tests("retained".to_owned(), item("a"), dimensions)
        else {
            panic!("retained job is admitted");
        };
        jobs.start(retained).expect("retained job starts");
        jobs.publish_ready(retained, vec![vec![0.5; dimensions]]);
        assert!(matches!(
            jobs.admit_uncharged_for_tests("running".to_owned(), item("b"), dimensions),
            AdmitOutcome::Admitted { .. }
        ));

        // A two-item request cannot fit beside the in-flight reservation even with the cap emptied, so the retained result survives the rejection.
        let two_items = vec![charged_item("c", "gamma"), charged_item("d", "delta")];
        assert!(matches!(
            jobs.admit_uncharged_for_tests("large".to_owned(), two_items, dimensions),
            AdmitOutcome::Full
        ));
        assert!(
            matches!(
                jobs.poll(&retained_id, "retained", None),
                PollOutcome::Page(_)
            ),
            "a rejection that eviction could not have satisfied leaves retained results alone"
        );
    }

    #[test]
    fn only_cursors_the_table_issued_resolve() {
        let limits = SynapseLimits {
            max_page_vectors: 1,
            ..SynapseLimits::default()
        };
        let jobs = JobTable::new(limits);
        let items = vec![
            charged_item("a", "alpha"),
            charged_item("b", "beta"),
            charged_item("c", "gamma"),
        ];
        let AdmitOutcome::Admitted { job_id, seq } =
            jobs.admit_uncharged_for_tests("paged".to_owned(), items, 1)
        else {
            panic!("paged job is admitted");
        };
        jobs.start(seq).expect("paged job starts");
        jobs.publish_ready(seq, vec![vec![1.0], vec![1.0], vec![1.0]]);

        // Boundaries 1 and 2 are legal page starts, but a client cannot name them without the table's authenticator.
        for fabricated in [
            format!("{job_id}:1"),
            format!("{job_id}:2"),
            format!("{job_id}:1:{}", "0".repeat(32)),
            format!("{job_id}:2:{}", "f".repeat(32)),
        ] {
            assert!(
                matches!(
                    jobs.poll(&job_id, "paged", Some(&fabricated)),
                    PollOutcome::BadCursor
                ),
                "fabricated cursor {fabricated} must not resolve"
            );
        }

        let PollOutcome::Page(first) = jobs.poll(&job_id, "paged", None) else {
            panic!("the first page is served from a null cursor");
        };
        let second_cursor = first.next_cursor.expect("the first page carries a cursor");
        assert!(second_cursor.starts_with(&format!("{job_id}:1:")));
        assert!(second_cursor.len() <= super::super::protocol::MAX_CURSOR_BYTES);

        // An issued cursor's authenticator is bound to its boundary, so moving it to another boundary fails.
        let authenticator = second_cursor.rsplit(':').next().expect("authenticator");
        assert!(matches!(
            jobs.poll(
                &job_id,
                "paged",
                Some(&format!("{job_id}:2:{authenticator}"))
            ),
            PollOutcome::BadCursor
        ));

        let PollOutcome::Page(second) = jobs.poll(&job_id, "paged", Some(&second_cursor)) else {
            panic!("an issued cursor serves its page");
        };
        assert_eq!(second.vectors[0].0, "b");
        let third_cursor = second
            .next_cursor
            .expect("the second page carries a cursor");

        // Replaying an earlier issued cursor stays valid after later pages were served.
        assert!(matches!(
            jobs.poll(&job_id, "paged", Some(&second_cursor)),
            PollOutcome::Page(_)
        ));
        let PollOutcome::Page(third) = jobs.poll(&job_id, "paged", Some(&third_cursor)) else {
            panic!("the final issued cursor serves the last page");
        };
        assert!(third.done);
        assert_eq!(third.vectors[0].0, "c");
    }
}
