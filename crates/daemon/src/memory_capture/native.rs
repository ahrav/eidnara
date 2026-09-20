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
}

#[cfg(test)]
impl NativeCaptureState {
    pub(crate) fn expire_ready_for_test(&mut self) {
        for lease in self.leases.values_mut() {
            if lease.plan.is_some() {
                lease.expires = Instant::now();
            }
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
            role: if job.role == "user" {
                CaptureRole::User
            } else {
                CaptureRole::Assistant
            },
            text: job.text.clone(),
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
    fn native_capture_work(&self, store: Arc<MemoryStore>, binding: SessionBinding) -> CaptureWork {
        CaptureWork {
            store,
            kernel: Arc::clone(&self.kernel),
            models: binding.config.model_chain.clone(),
            binding,
            cancel: self.cancel.clone(),
            commit_gate: Arc::clone(&self.capture_commit_gate),
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
        // the dispatch is recorded only once the work is issued.
        kernel_routes::blocking(move || {
            let mut guard = guard;
            let (plan, prompt) = match work.claim(primary.as_deref()) {
                NextStep::Work(plan, prompt) => (plan, prompt),
                NextStep::Done(outcome) => return outcome,
            };
            if !wanted.load(Ordering::Acquire) {
                return respond(json!({"state":"pending"}));
            }
            // One dispatch for the whole batch or none: a source swept since
            // the queue was read leaves the others' counts untouched.
            let job_ids: Vec<&str> = plan.jobs.iter().map(|job| job.job_id.as_str()).collect();
            match work
                .store
                .begin_memory_capture_attempts(&work.project(), &job_ids, now_ms())
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
            work.settle(plan, output.as_deref(), error.as_deref())
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
/// is not lost.
pub(super) fn collapse_quotations(
    attribution: &str,
    proposed: Vec<CapturedMemory>,
) -> Vec<CapturedMemory> {
    let mut memories: Vec<CapturedMemory> = Vec::new();
    for mut memory in proposed {
        memory.content = format!("{attribution}: {}", memory.quote);
        match memories
            .iter_mut()
            .find(|kept| kept.category == memory.category && kept.content == memory.content)
        {
            Some(kept) => {
                if kept.replaces.is_none()
                    && (kept.duplicate_of.is_none() || memory.replaces.is_some())
                {
                    kept.replaces = memory.replaces;
                    kept.duplicate_of = memory.duplicate_of;
                }
            }
            None => memories.push(memory),
        }
    }
    memories
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
        match self.accept_native_output(plan, output.unwrap_or_default(), now) {
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
        for job in jobs {
            if job.prepared.is_some() {
                self.publish_or_recover(&job)?;
            } else {
                unprepared.push(job);
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
        let mut existing = self.existing_memories(&messages)?;
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
    ) -> Result<(), &'static str> {
        let decisions = match parse_capture_batch(output, &plan.messages, &plan.existing) {
            Ok(decisions) => decisions,
            Err(reason) => {
                eprintln!("daemon: native memory capture output rejected: {reason}");
                return self.failed(&plan.jobs, "extraction_failed", now);
            }
        };
        for (mut job, decision) in plan.jobs.into_iter().zip(decisions) {
            let decision = match decision {
                Ok(decision) => decision,
                Err(reason) => {
                    eprintln!("daemon: native memory capture source rejected: {reason}");
                    self.failed(std::slice::from_ref(&job), "extraction_failed", now)?;
                    continue;
                }
            };
            let attribution = if job.role == "user" {
                "User stated"
            } else {
                "Assistant reported"
            };
            // The stored text is the attributed quotation, so two facts citing
            // one clause collapse here; the parse-time check saw the model's
            // paraphrases, which can differ.
            let memories = collapse_quotations(attribution, decision.memories);
            let existing = plan
                .existing
                .iter()
                .filter(|previous| {
                    memories.iter().any(|memory| {
                        memory.replaces.as_deref() == Some(&previous.id)
                            || memory.duplicate_of.as_deref() == Some(&previous.id)
                    })
                })
                .cloned()
                .collect();
            let output = PreparedCapture {
                version: CAPTURE_SCHEMA_VERSION,
                model: plan.model.clone(),
                existing,
                memories,
            };
            let frozen = serde_json::to_string(&output).map_err(|_| "output_encoding_failed")?;
            let prepared =
                match self
                    .store
                    .prepare_memory_capture(&job.project, &job.job_id, &frozen)
                {
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
                self.publish_or_recover(&job)?;
            }
        }
        Ok(())
    }
}
