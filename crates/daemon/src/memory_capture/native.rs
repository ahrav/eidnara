//! Native harnesses own model/auth execution. The daemon owns source leases,
//! validation, frozen plans, and publication; no credentials cross this API.

use super::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) const LEASE_DURATION: Duration = Duration::from_secs(180);
const MAX_NATIVE_PROMPT_BYTES: usize = 240 * 1024;

type LeaseKey = (String, String);

#[derive(Default)]
pub(crate) struct NativeCaptureState {
    leases: HashMap<LeaseKey, NativeLease>,
    waits: CaptureWaits,
}

/// How long capture publication waits: one call's shared allowance for
/// committing frozen plans, and the longest one publication waits for the
/// capture gate and the Kernel writer.
#[derive(Clone, Copy)]
pub(crate) struct CaptureWaits {
    pub(crate) plan_commit_budget: Duration,
    pub(crate) commit_wait: Duration,
}

impl Default for CaptureWaits {
    fn default() -> Self {
        Self {
            plan_commit_budget: super::PLAN_COMMIT_BUDGET,
            commit_wait: super::COMMIT_WAIT,
        }
    }
}

#[cfg(test)]
impl NativeCaptureState {
    pub(crate) fn set_waits_for_test(&mut self, waits: CaptureWaits) {
        self.waits = waits;
    }

    pub(crate) fn expire_ready_for_test(&mut self) {
        for lease in self.leases.values_mut() {
            if lease.plan.is_some() {
                lease.expires = Instant::now();
            }
        }
    }

    pub(crate) fn expire_all_for_test(&mut self) {
        for lease in self.leases.values_mut() {
            lease.expires = Instant::now();
        }
    }

    pub(crate) fn reserved_for_test(&self) -> usize {
        self.leases.len()
    }

    /// The shortest time a lease holding issued work has left.
    pub(crate) fn shortest_ready_lease_for_test(&self) -> Option<Duration> {
        self.leases
            .values()
            .filter(|lease| lease.plan.is_some())
            .map(|lease| lease.expires.saturating_duration_since(Instant::now()))
            .min()
    }
}

struct NativeLease {
    token: String,
    session: String,
    expires: Instant,
    plan: Option<NativePlan>,
}

struct NativePlan {
    jobs: Vec<CaptureJob>,
    messages: Vec<CaptureMessage>,
    existing: Vec<ExistingCaptureMemory>,
    model: String,
}

// Removing a completed/abandoned reservation must never remove its successor.
struct LeaseGuard {
    state: Arc<Mutex<NativeCaptureState>>,
    key: LeaseKey,
    token: String,
    retained: bool,
}

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        if !self.retained {
            let mut state = self.state.lock().expect("native capture leases mutex");
            if state
                .leases
                .get(&self.key)
                .is_some_and(|lease| lease.token == self.token)
            {
                state.leases.remove(&self.key);
            }
        }
    }
}

fn messages_for(jobs: &[CaptureJob]) -> Vec<CaptureMessage> {
    jobs.iter()
        .enumerate()
        .map(|(index, job)| CaptureMessage {
            id: format!("source_{}", index + 1),
            role: if super::is_user_source(job) {
                CaptureRole::User
            } else {
                CaptureRole::Assistant
            },
            text: job.text.clone(),
            own_id: !super::cite::is_hashed_source_id(&job.message_id),
        })
        .collect()
}

/// Dropped with a request future; the blocking claim reads it before it
/// records a dispatch, so a request nobody is waiting on issues no work.
struct DeliveryWatch(Arc<AtomicBool>);

impl Drop for DeliveryWatch {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// What one blocking-pool claim produced: a plan to lease, or the final reply.
enum NextStep {
    Work(NativePlan, String),
    Done(PreparedOutcome),
}

impl HandlerCore {
    /// What each job session's source publish reached before this submit's
    /// citation lookups, or `None` when the daemon was cancelled during a
    /// wait. A plan spans its project's pending jobs, so every distinct job
    /// session is observed, all under one lock: one publication slot means at
    /// most one of them is `Running`, so the waits end by its deadline. No
    /// wait runs for an error submit, with consent off, or for a job whose
    /// source id cannot join.
    async fn capture_settlements(
        &self,
        binding: &SessionBinding,
        plan: &NativePlan,
        error: bool,
    ) -> Option<JobSettlements> {
        if error || !citation_consent(&binding.config) {
            return Some(JobSettlements::new());
        }
        let mut sessions = Vec::new();
        for key in plan.jobs.iter().filter_map(super::cite::settlement_key) {
            if !sessions.contains(&key) {
                sessions.push(key);
            }
        }
        let observed = self.source_capture.observe_settlements(&sessions);
        let mut settlements = JobSettlements::with_capacity(sessions.len());
        for (session, observed) in sessions.into_iter().zip(observed) {
            let outcome = tokio::select! {
                biased;
                () = self.cancel.cancelled() => return None,
                outcome = crate::source_capture::settle(observed) => outcome,
            };
            settlements.push((session, outcome));
        }
        Some(settlements)
    }

    fn native_capture_work(&self, store: Arc<MemoryStore>, binding: SessionBinding) -> CaptureWork {
        let waits = self
            .native_capture
            .lock()
            .expect("native capture mutex")
            .waits;
        CaptureWork {
            store,
            kernel: Arc::clone(&self.kernel),
            models: binding.config.model_chain.clone(),
            binding,
            cancel: self.cancel.clone(),
            commit_gate: Arc::clone(&self.capture_commit_gate),
            source_capture: Arc::clone(&self.source_capture),
            plan_commit_budget: waits.plan_commit_budget,
            commit_wait: waits.commit_wait,
        }
    }

    /// Claims one bounded batch for a connected harness. Expired leases can be
    /// replaced; stale replies cannot publish against the replacement lease.
    pub(crate) async fn handle_native_capture_next(
        &self,
        channel: RouteHandle,
        request: &Value,
    ) -> PreparedOutcome {
        let (session, binding) = match self.memory_capture_binding(
            channel,
            request,
            "memory.capture.next",
            &["model"],
        ) {
            Ok(bound) => bound,
            Err(error) => return error,
        };
        if !capture_enabled(&binding) {
            return respond(json!({"state":"disabled"}));
        }
        // Text a transform admitted but has not yet stored is not drained
        // work yet, and the drain is not done either.
        if self
            .capture_memo
            .lock()
            .expect("capture memo mutex")
            .outstanding(&capture_project(&binding))
            > 0
        {
            return respond(json!({"state":"pending"}));
        }
        let primary = match request.get("model") {
            None => None,
            Some(Value::String(model)) if valid_capture_model(model) => Some(model.clone()),
            _ => return invalid_params_error("capture model must be a provider/model string"),
        };
        let Some(store) = self.store() else {
            return store_unavailable_error();
        };
        let key = (capture_project(&binding), binding.harness.clone());
        let mut nonce = [0_u8; 16];
        if getrandom::getrandom(&mut nonce).is_err() {
            return respond(json!({"state":"unavailable"}));
        }
        let token: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
        {
            let mut state = self
                .native_capture
                .lock()
                .expect("native capture leases mutex");
            state
                .leases
                .retain(|_, lease| lease.plan.is_none() || lease.expires > Instant::now());
            if state.leases.contains_key(&key) || state.leases.len() >= MAX_CAPTURE_WORKERS {
                return respond(json!({"state":"pending"}));
            }
            state.leases.insert(
                key.clone(),
                NativeLease {
                    token: token.clone(),
                    session,
                    expires: Instant::now() + LEASE_DURATION,
                    plan: None,
                },
            );
        }
        let guard = LeaseGuard {
            state: Arc::clone(&self.native_capture),
            key,
            token: token.clone(),
            retained: false,
        };
        let work = self.native_capture_work(store, binding);
        let wanted = Arc::new(AtomicBool::new(true));
        let _watch = DeliveryWatch(Arc::clone(&wanted));
        let state = Arc::clone(&self.native_capture);
        // The guard travels with the blocking work: a request future dropped
        // mid-claim keeps the reservation until that work has finished, and
        // the batch's model dispatch is recorded only once the work is
        // issued. A frozen plan the claim commits and the Kernel refuses
        // records its own dispatch during the claim and keeps it.
        kernel_routes::blocking(move || {
            let mut guard = guard;
            let (plan, prompt) = match work.claim(primary.as_deref()) {
                NextStep::Work(plan, prompt) => (plan, prompt),
                NextStep::Done(outcome) => return outcome,
            };
            if !wanted.load(Ordering::Acquire) {
                return respond(json!({"state":"pending"}));
            }
            // A reservation that lapsed during preparation issues nothing, so
            // it records nothing either.
            if !state
                .lock()
                .expect("native capture leases mutex")
                .leases
                .get(&guard.key)
                .is_some_and(|lease| lease.token == guard.token && lease.expires > Instant::now())
            {
                return respond(json!({"state":"stale"}));
            }
            // One dispatch for the whole batch or none: a source swept since
            // the queue was read leaves the others' counts untouched.
            // Issued work carries its own retry deadline, so a lease lost to a
            // restart is retried after the dispatch backoff, not at once.
            let now = now_ms();
            let jobs: Vec<(&str, i64)> = plan
                .jobs
                .iter()
                .map(|job| {
                    (
                        job.job_id.as_str(),
                        now.saturating_add(capture_retry_delay_ms(job.attempts)),
                    )
                })
                .collect();
            match work
                .store
                .begin_memory_capture_attempts(&work.project(), &jobs)
            {
                Ok(true) => {}
                Ok(false) => return respond(json!({"state":"pending"})),
                Err(_) => return respond(json!({"state":"store_failed"})),
            }
            let response = json!({"state":"work", "lease":guard.token, "model":plan.model,
                "system":CAPTURE_SYSTEM_PROMPT,"prompt":prompt,
                "max_output_tokens":8192,"max_output_bytes":MAX_CAPTURE_OUTPUT_BYTES,"max_duration_ms":90_000});
            let mut state = state.lock().expect("native capture leases mutex");
            let Some(lease) = state
                .leases
                .get_mut(&guard.key)
                .filter(|lease| lease.token == guard.token && lease.expires > Instant::now())
            else {
                return respond(json!({"state":"stale"}));
            };
            // The lease clock starts when work is issued, so the advertised
            // execution window fits however long preparation waited.
            lease.expires = Instant::now() + LEASE_DURATION;
            lease.plan = Some(plan);
            guard.retained = true;
            respond(response)
        })
        .await
        .unwrap_or_else(|_| {
            eprintln!("daemon: native memory capture preparation failed: pool_unavailable");
            respond(json!({"state":"pending"}))
        })
    }

    /// Accepts only a matching, unexpired lease from the same project, harness,
    /// and session. Model output remains an untrusted proposal.
    pub(crate) async fn handle_native_capture_submit(
        &self,
        channel: RouteHandle,
        request: &Value,
    ) -> PreparedOutcome {
        let (session, binding) = match self.memory_capture_binding(
            channel,
            request,
            "memory.capture.submit",
            &["lease", "model", "output", "error"],
        ) {
            Ok(bound) => bound,
            Err(error) => return error,
        };
        let Some(token) = request
            .get("lease")
            .and_then(Value::as_str)
            .filter(|token| {
                token.len() == 32
                    && token
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
        else {
            return invalid_params_error("native capture requires a lease token");
        };
        let output = request.get("output").and_then(Value::as_str);
        let error = request.get("error").and_then(Value::as_str);
        if request.get("output").is_some() != output.is_some()
            || request.get("error").is_some() != error.is_some()
            || !matches!(
                (output, error),
                (Some(_), None)
                    | (
                        None,
                        Some(
                            "cancelled" | "provider_unavailable" | "model_failed" | "output_limit"
                        )
                    )
            )
            || output.is_some_and(|text| text.len() > MAX_CAPTURE_OUTPUT_BYTES)
        {
            return invalid_params_error(
                "native capture requires bounded output or a recognized error",
            );
        }
        let key = (capture_project(&binding), binding.harness.clone());
        let plan = {
            let mut state = self
                .native_capture
                .lock()
                .expect("native capture leases mutex");
            let Some(lease) = state.leases.get_mut(&key).filter(|lease| {
                lease.token == token && lease.session == session && lease.expires > Instant::now()
            }) else {
                return respond(json!({"state":"stale"}));
            };
            if output.is_some()
                && lease.plan.as_ref().is_some_and(|plan| {
                    request.get("model").and_then(Value::as_str) != Some(plan.model.as_str())
                })
            {
                return invalid_params_error("native capture model does not match its lease");
            }
            let Some(plan) = lease.plan.take() else {
                return respond(json!({"state":"pending"}));
            };
            plan
        };
        let guard = LeaseGuard {
            state: Arc::clone(&self.native_capture),
            key,
            token: token.into(),
            retained: false,
        };
        if !capture_enabled(&binding) {
            return respond(json!({"state":"disabled"}));
        }
        // The wait runs here, before the blocking hop that takes the capture
        // commit gate. A cancel returns with nothing recorded; the guard then
        // releases the lease and the jobs stay eligible.
        let Some(settlements) = self
            .capture_settlements(&binding, &plan, error.is_some())
            .await
        else {
            return respond(json!({"state":"pending"}));
        };
        let Some(store) = self.store() else {
            return store_unavailable_error();
        };
        let work = self.native_capture_work(store, binding);
        let output = output.map(str::to_owned);
        let error = error.map(str::to_owned);
        // A stale reply cannot publish against a successor: the reservation
        // outlives a dropped request future for as long as this work runs.
        match kernel_routes::blocking(move || {
            let _guard = guard;
            work.settle(plan, output.as_deref(), error.as_deref(), &settlements)
        })
        .await
        {
            Ok(outcome) => outcome,
            // Nothing was recorded, so the jobs stay eligible for a later drain.
            Err(_) => respond(json!({"state":"store_failed"})),
        }
    }
}

/// Memories of one source sharing a category and quotation freeze as one.
/// A replacement target survives the collapse so the correction it carries
/// is not lost; one quotation superseding two memories keeps one entry per
/// target, since a decision supersedes exactly one predecessor. The
/// collapsed memory is a duplicate only when every memory it joins names
/// the same duplicate target: a memory that names none carries a fact the
/// store does not hold, and that fact is written.
pub(super) fn collapse_quotations(
    attribution: &str,
    proposed: Vec<CapturedMemory>,
) -> Vec<CapturedMemory> {
    let mut memories: Vec<CapturedMemory> = Vec::new();
    for mut memory in proposed {
        memory.content = format!("{attribution}: {}", memory.quote);
        match memories.iter_mut().find(|kept| {
            kept.category == memory.category
                && kept.content == memory.content
                && (kept.replaces.is_none()
                    || memory.replaces.is_none()
                    || kept.replaces == memory.replaces)
        }) {
            Some(kept) => {
                // One quotation proposed under two classes is stored under
                // the one that holds longer, so a statement the extractor
                // read both ways stays eligible for approval.
                kept.lifetime = match (kept.lifetime, memory.lifetime) {
                    (Some(kept), Some(joined)) => Some(kept.max(joined)),
                    _ => None,
                };
                if kept.replaces.is_none() && memory.replaces.is_some() {
                    kept.replaces = memory.replaces;
                    kept.also_replaces = memory.also_replaces;
                    kept.duplicate_of = None;
                } else if kept.duplicate_of != memory.duplicate_of {
                    kept.duplicate_of = None;
                }
                for target in memory.contests {
                    if !kept.contests.contains(&target) {
                        kept.contests.push(target);
                    }
                }
            }
            None => memories.push(memory),
        }
    }
    memories
}

/// Settles the links of one source's memories after collapse and conversion
/// have joined or moved them, so the frozen plan names each target for one
/// purpose. A memory contests no memory it replaces, retires, or duplicates,
/// and names each contest once. A target some memory of the source replaces
/// or retires is contested by none of them, and a memory that only
/// duplicates such a target is dropped: its fact is the one the sibling
/// replaces. A duplicate of such a target that also contests other memories
/// is kept for its contests and written as its own memory. Each target is
/// contested by the first memory that names it.
///
/// Returns, for each memory kept, whether it is such a duplicate kept for
/// its contests; the batch pass drops it when it strips them.
pub(super) fn settle_links(memories: &mut Vec<CapturedMemory>) -> Vec<bool> {
    let removed: Vec<String> = memories
        .iter()
        .flat_map(|memory| memory.replaces.iter().chain(&memory.also_replaces))
        .cloned()
        .collect();
    let mut contested: Vec<String> = Vec::new();
    let mut kept_for_contests = Vec::new();
    memories.retain_mut(|memory| {
        let replaced_duplicate = memory
            .duplicate_of
            .as_ref()
            .is_some_and(|target| removed.contains(target));
        if replaced_duplicate {
            memory.duplicate_of = None;
        }
        let duplicate = memory.duplicate_of.clone();
        // Collapse and conversion can join the contests of several
        // proposals; the joined memory keeps the first
        // `MAX_CONTESTS_PER_MEMORY` it may name.
        let mut named = 0;
        memory.contests.retain(|target| {
            let kept = named < super::MAX_CONTESTS_PER_MEMORY
                && !removed.contains(target)
                && duplicate.as_ref() != Some(target)
                && !contested.contains(target);
            if kept {
                named += 1;
                contested.push(target.clone());
            }
            kept
        });
        let kept = !replaced_duplicate || !memory.contests.is_empty();
        if kept {
            kept_for_contests.push(replaced_duplicate);
        }
        kept
    });
    kept_for_contests
}

/// Settles the contests of one batch across its sources, after each source
/// settled its own links. A target any source of the batch replaces or
/// retires is contested by none of them, so the replacement commits against
/// the row the plan named. A target is contested by the first source that
/// names it, so two sources leave one record on it. A memory its source
/// kept only for its contests (see [`settle_links`]) is dropped when this
/// pass strips them; every other memory is still written.
pub(super) fn settle_batch_contests(batch: &mut [(Vec<CapturedMemory>, Vec<bool>)]) {
    let removed: Vec<String> = batch
        .iter()
        .flat_map(|(memories, _)| memories)
        .flat_map(|memory| memory.replaces.iter().chain(&memory.also_replaces))
        .cloned()
        .collect();
    let mut contested: Vec<String> = Vec::new();
    for (memories, kept_for_contests) in batch {
        let mut named = Vec::new();
        let mut flags = std::mem::take(kept_for_contests).into_iter();
        memories.retain_mut(|memory| {
            memory.contests.retain(|target| {
                let kept = !removed.contains(target) && !contested.contains(target);
                if kept {
                    named.push(target.clone());
                }
                kept
            });
            !(flags.next().unwrap_or(false) && memory.contests.is_empty())
        });
        contested.append(&mut named);
    }
}

/// Widens each cut quotation of a user message to the whole sentence that
/// holds it, so the stored statement keeps a lead-in, a negation, or a
/// condition the extractor left out (see [`super::widened_statements`]).
///
/// Returns the widened quotations.
fn widen_cut_quotations(prose: &super::Prose<'_>, proposed: &mut [CapturedMemory]) -> Vec<String> {
    let widened = super::widened_statements(prose, proposed);
    let mut quotations = Vec::new();
    for (memory, statement) in proposed.iter_mut().zip(widened) {
        if let Some(statement) = statement {
            memory.quote = statement.to_owned();
            quotations.push(statement.to_owned());
        }
    }
    quotations
}

impl CaptureWork {
    fn project(&self) -> String {
        capture_project(&self.binding)
    }

    fn claim(&self, primary: Option<&str>) -> NextStep {
        match self.next_native_plan(primary) {
            Ok(Some((plan, prompt))) => NextStep::Work(plan, prompt),
            Ok(None) => NextStep::Done(match self.store.memory_capture_status(&self.project()) {
                Ok(status) => respond(
                    json!({"state":if status.pending == 0 { "ready" } else { "pending" },
                        "pending":status.pending,"failed":status.failed,"completed":status.completed}),
                ),
                Err(_) => respond(json!({"state":"store_failed"})),
            }),
            Err(code) => {
                eprintln!("daemon: native memory capture preparation failed: {code}");
                NextStep::Done(respond(json!({"state": if code == "store_failed" {
                    "store_failed"
                } else {
                    "pending"
                }})))
            }
        }
    }

    fn settle(
        &self,
        plan: NativePlan,
        output: Option<&str>,
        error: Option<&str>,
        settlements: &JobSettlements,
    ) -> PreparedOutcome {
        let now = now_ms();
        if let Some(error) = error {
            for job in &plan.jobs {
                if self
                    .store
                    .fail_memory_capture(
                        &job.project,
                        &job.job_id,
                        error,
                        // Every dispatch backs off, cancellation included, so a
                        // claim/cancel loop cannot spin the store.
                        now.saturating_add(capture_retry_delay_ms(job.attempts)),
                        matches!(error, "model_failed" | "output_limit"),
                        now,
                    )
                    .is_err()
                {
                    return respond(json!({"state":"store_failed"}));
                }
            }
            return respond(json!({"state":"pending"}));
        }
        // An unrecorded store error would leave these jobs immediately eligible again.
        let jobs = plan.jobs.clone();
        match self.accept_native_output(plan, output.unwrap_or_default(), now, settlements) {
            Ok(()) => respond(json!({"state":"processed"})),
            Err(code) => {
                eprintln!("daemon: native memory capture submission failed: {code}");
                if self.failed(&jobs, "store_failed", now).is_err() {
                    return respond(json!({"state":"store_failed"}));
                }
                respond(json!({"state":"pending"}))
            }
        }
    }

    fn next_native_plan(
        &self,
        primary: Option<&str>,
    ) -> Result<Option<(NativePlan, String)>, &'static str> {
        let project = self.project();
        let jobs = self
            .store
            .pending_memory_captures(&project, &self.binding.harness, now_ms())
            .map_err(|_| "store_failed")?;
        let mut unprepared = Vec::new();
        // The frozen plans share one budget, so a busy Kernel writer delays
        // the extraction window by that budget at most. A plan the budget
        // does not reach stays ripe for the next call.
        let budget = Instant::now() + self.plan_commit_budget;
        for job in jobs {
            if job.prepared.is_none() {
                unprepared.push(job);
            } else if Instant::now() < budget {
                // A replayed plan reads its session's record without waiting.
                let settlement = super::cite::settlement_key(&job)
                    .map_or(SettlementOutcome::Settled, |key| {
                        self.source_capture.settlement_now(&key)
                    });
                let deadline = budget.min(Instant::now() + self.commit_wait);
                self.publish_or_recover(&job, settlement, deadline)?;
            }
        }
        if unprepared.is_empty() {
            return Ok(None);
        }
        let models = if self.models.is_empty() {
            primary.map(str::to_owned).into_iter().collect()
        } else {
            self.models.clone()
        };
        if models.is_empty() {
            return Ok(None);
        }
        // Fallback stage follows failures; a batch shares one model, so it
        // holds only sources at the head's stage. The rest wait for the next
        // drain call rather than skipping their own primary attempt.
        let failures = unprepared[0].failures;
        unprepared.retain(|job| job.failures == failures);
        let model = models[failures as usize % models.len()].clone();
        if !valid_capture_model(&model) {
            return Err("invalid_model_configuration");
        }
        let mut messages = messages_for(&unprepared);
        let mut existing = self.existing_memories(&messages, &unprepared)?;
        let prompt = loop {
            let prompt = render_capture_prompt_with_existing(&messages, &existing)?;
            if prompt.len() <= MAX_NATIVE_PROMPT_BYTES {
                break prompt;
            }
            if existing.pop().is_none() {
                if let [job] = unprepared.as_mut_slice() {
                    // Alone and still too large, this source can never be
                    // prepared. Recording the dispatch and a model failure
                    // moves it off the queue head on the same allowance any
                    // other unusable source consumes, instead of re-selecting
                    // it forever ahead of every later source.
                    self.store
                        .begin_memory_capture_attempt(&job.project, &job.job_id, now_ms())
                        .map_err(|_| "store_failed")?;
                    job.attempts += 1;
                    self.failed(std::slice::from_ref(job), "source_too_large", now_ms())?;
                    return Err("source_too_large");
                }
                unprepared.pop();
                messages = messages_for(&unprepared);
            }
        };
        Ok(Some((
            NativePlan {
                jobs: unprepared,
                messages,
                existing,
                model,
            },
            prompt,
        )))
    }

    fn accept_native_output(
        &self,
        plan: NativePlan,
        output: &str,
        now: i64,
        settlements: &JobSettlements,
    ) -> Result<(), &'static str> {
        // If `states` rejects a user quotation, its approved replacement
        // targets become contests.
        let proses: Vec<super::Prose<'_>> = plan
            .jobs
            .iter()
            .map(|job| super::Prose::of(&job.text))
            .collect();
        let states = |index: usize, memory: &CapturedMemory| {
            let (job, prose) = (&plan.jobs[index], &proses[index]);
            super::states_whole_statements(super::states_whole_message(job), prose, &memory.quote)
        };
        let decisions = match parse_capture_batch(output, &plan.messages, &plan.existing, &states) {
            Ok(decisions) => decisions,
            Err(reason) => {
                eprintln!("daemon: native memory capture output rejected: {reason}");
                return self.failed(&plan.jobs, "extraction_failed", now);
            }
        };
        // Each source's memories are settled first, then the batch's
        // contests across sources, and only then is any plan frozen.
        let mut accepted = Vec::new();
        let mut settled = Vec::new();
        for (index, (job, decision)) in plan.jobs.iter().zip(decisions).enumerate() {
            let decision = match decision {
                Ok(decision) => decision,
                Err(reason) => {
                    eprintln!("daemon: native memory capture source rejected: {reason}");
                    self.failed(std::slice::from_ref(job), "extraction_failed", now)?;
                    continue;
                }
            };
            let attribution = if super::is_user_source(job) {
                USER_STATED
            } else {
                super::ASSISTANT_REPORTED
            };
            let prose = &proses[index];
            let states_whole = super::states_whole_message(job);
            let mut proposed = decision.memories;
            let widened = if super::is_user_source(job)
                && !super::cite::is_hashed_source_id(&job.message_id)
            {
                widen_cut_quotations(prose, &mut proposed)
            } else {
                Vec::new()
            };
            // The stored text is the attributed quotation, so two facts citing
            // one clause collapse here; the parse-time check saw the model's
            // paraphrases, which can differ.
            let mut memories = collapse_quotations(attribution, proposed);
            // A user's statement that repeats an approved memory word for
            // word duplicates that memory when the extractor named a copy
            // that is not served automatically, such as an agent's note. A
            // named copy that is served automatically is kept, so a repeat
            // of a contested statement reaches that statement.
            for memory in &mut memories {
                let Some(named) = memory
                    .duplicate_of
                    .as_ref()
                    .map(|target| plan.existing.iter().find(|previous| previous.id == *target))
                else {
                    continue;
                };
                if named.is_none_or(|previous| !previous.automatic)
                    && let Some(approved) = super::approved_copy(&plan.existing, &memory.content)
                {
                    memory.duplicate_of = Some(approved);
                } else if widened.contains(&memory.quote)
                    && named.is_some_and(|previous| {
                        super::is_another_approved_text(previous, &memory.content)
                    })
                {
                    // The extractor matched the clause it quoted to an
                    // approved statement. The widened sentence is another
                    // text, such as that clause negated, so it is written
                    // as the user's own statement, or names the approved
                    // statement that holds it word for word.
                    memory.duplicate_of = super::approved_copy(&plan.existing, &memory.content);
                }
                // A statement that names a contested statement as the memory
                // that already holds it settles the contest only word for
                // word. Another text is the user's own new statement, of
                // any lifetime class, and is written.
                else if named.is_some_and(|previous| {
                    previous.contested && super::is_another_approved_text(previous, &memory.content)
                }) && super::user_stated(states_whole, prose, memory)
                {
                    memory.duplicate_of = None;
                }
            }
            // A user's contest shows whole sentences of the user's message
            // beside the statement, such as a question or a correction in a
            // sentence after a colon. A colon can introduce another
            // author's words, so the excerpt may be reported text, and
            // keeping the contest approves the quotation as no statement.
            // A clause cut out of a sentence can say the opposite of that
            // sentence and contests nothing. A fragment of a long message
            // has edges the user did not write, so no quotation of it is
            // known to be whole.
            if super::is_user_source(job) {
                let fragment = super::cite::is_hashed_source_id(&job.message_id);
                for memory in &mut memories {
                    if !memory.contests.is_empty()
                        && (fragment || !super::whole_sentences(prose, &memory.quote, true))
                    {
                        memory.contests.clear();
                    }
                }
            }
            let kept_for_contests = settle_links(&mut memories);
            accepted.push((index, job.clone()));
            settled.push((memories, kept_for_contests));
        }
        settle_batch_contests(&mut settled);
        // One read per submission, so every plan it freezes names one HEAD.
        let head = observe_head(self.binding.kernel_project.root());
        let budget = Instant::now() + self.plan_commit_budget;
        for ((index, mut job), (quoted, _)) in accepted.into_iter().zip(settled) {
            let mut memories = quoted.clone();
            failure_evidence::attach(&job.text, &mut memories);
            let attached = keep_statement_text(&job, &proses[index], &quoted, &mut memories);
            let existing = plan
                .existing
                .iter()
                .filter(|previous| {
                    memories.iter().any(|memory| {
                        memory.replaces.as_deref() == Some(&previous.id)
                            || memory.also_replaces.contains(&previous.id)
                            || memory.duplicate_of.as_deref() == Some(&previous.id)
                            || memory.contests.contains(&previous.id)
                    })
                })
                .cloned()
                .collect();
            let mut output = PreparedCapture {
                version: CAPTURE_SCHEMA_VERSION,
                model: plan.model.clone(),
                existing,
                memories,
                head: head.clone(),
            };
            let frozen = serde_json::to_string(&output).map_err(|_| "output_encoding_failed")?;
            let mut prepared =
                self.store
                    .prepare_memory_capture(&job.project, &job.job_id, &frozen);
            // Appended lines sit beside text they were not beside in the redacted
            // source, so the store's secret scan or size bound may refuse a plan its
            // quotations alone pass; that plan freezes without the appended lines.
            if attached
                && matches!(
                    prepared,
                    Err(MemoryStoreError::Serde(_) | MemoryStoreError::Redaction(_))
                )
            {
                output.memories = quoted;
                let frozen =
                    serde_json::to_string(&output).map_err(|_| "output_encoding_failed")?;
                prepared = self
                    .store
                    .prepare_memory_capture(&job.project, &job.job_id, &frozen);
            }
            let prepared = match prepared {
                Ok(prepared) => prepared,
                Err(MemoryStoreError::Serde(_) | MemoryStoreError::Redaction(_)) => {
                    eprintln!("daemon: native memory capture plan refused by the store");
                    self.failed(std::slice::from_ref(&job), "extraction_failed", now)?;
                    continue;
                }
                Err(_) => return Err("store_failed"),
            };
            if let Some(frozen) = prepared {
                job.prepared = Some(frozen);
                // The plan's copy of the job predates the dispatch that
                // produced this output; the backoff counts that dispatch.
                job.attempts += 1;
                // The batch shares one budget: every plan is frozen, and a
                // plan the budget does not reach is committed by the next
                // drain's plan window.
                if Instant::now() < budget {
                    self.publish_or_recover(
                        &job,
                        job_settlement(settlements, &job),
                        budget.min(Instant::now() + self.commit_wait),
                    )?;
                }
            }
        }
        Ok(())
    }
}

/// Restores the quoted text of each memory that passes [`user_attributed`].
/// Failure lines stay on the source's other memories.
/// Returns whether any memory still holds an appended line.
pub(super) fn keep_statement_text(
    job: &CaptureJob,
    prose: &super::Prose<'_>,
    quoted: &[CapturedMemory],
    memories: &mut [CapturedMemory],
) -> bool {
    let mut attached = false;
    let states_whole = super::states_whole_message(job);
    for (memory, quoted) in memories.iter_mut().zip(quoted) {
        if user_attributed(states_whole, prose, quoted) {
            memory.content.clone_from(&quoted.content);
        }
        attached |= memory.content != quoted.content;
    }
    attached
}

mod failure_evidence;
