//! Native harnesses own model/auth execution. The daemon owns source leases,
//! validation, frozen plans, and publication; no credentials cross this API.

use super::*;
use std::collections::HashMap;

const LEASE_DURATION: Duration = Duration::from_secs(180);
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

fn enabled(binding: &SessionBinding) -> bool {
    binding.config.memory_enabled
        && binding.config.auto_promote
        && binding.config.memory_auto_capture
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
        if !enabled(&binding) {
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
        let project = binding.project_root.to_string_lossy().into_owned();
        let key = (project.clone(), binding.harness.clone());
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
        let mut guard = LeaseGuard {
            state: Arc::clone(&self.native_capture),
            key,
            token: token.clone(),
            retained: false,
        };
        let work = self.native_capture_work(Arc::clone(&store), binding);
        let result = work.next_native_plan(primary.as_deref()).await;
        match result {
            Ok(Some((plan, prompt))) => {
                let response = json!({"state":"work", "lease":token, "model":plan.model,
                    "system":CAPTURE_SYSTEM_PROMPT,"prompt":prompt,
                    "max_output_tokens":8192,"max_output_bytes":MAX_CAPTURE_OUTPUT_BYTES,"max_duration_ms":90_000});
                let mut state = self
                    .native_capture
                    .lock()
                    .expect("native capture leases mutex");
                let Some(lease) = state
                    .leases
                    .get_mut(&guard.key)
                    .filter(|lease| lease.token == token && lease.expires > Instant::now())
                else {
                    return respond(json!({"state":"stale"}));
                };
                lease.plan = Some(plan);
                guard.retained = true;
                respond(response)
            }
            Ok(None) => match store.memory_capture_status(&project) {
                Ok(status) => respond(
                    json!({"state":if status.pending == 0 { "ready" } else { "pending" },
                    "pending":status.pending,"failed":status.failed,"completed":status.completed}),
                ),
                Err(_) => respond(json!({"state":"store_failed"})),
            },
            Err(code) => {
                eprintln!("daemon: native memory capture preparation failed: {code}");
                respond(json!({"state":"pending"}))
            }
        }
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
                token.len() == 32 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
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
        let key = (
            binding.project_root.to_string_lossy().into_owned(),
            binding.harness.clone(),
        );
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
        let _guard = LeaseGuard {
            state: Arc::clone(&self.native_capture),
            key,
            token: token.into(),
            retained: false,
        };
        if !enabled(&binding) {
            return respond(json!({"state":"disabled"}));
        }
        let Some(store) = self.store() else {
            return store_unavailable_error();
        };
        let work = self.native_capture_work(store, binding);
        let attempt = plan.jobs.iter().map(|job| job.attempts).max().unwrap_or(0);
        let retry_at = now_ms().saturating_add(capture_retry_delay_ms(attempt));
        if let Some(error) = error {
            for job in &plan.jobs {
                if work
                    .store
                    .fail_memory_capture(
                        &job.project,
                        &job.job_id,
                        error,
                        if error == "cancelled" {
                            now_ms()
                        } else {
                            retry_at
                        },
                        matches!(error, "model_failed" | "output_limit"),
                        now_ms(),
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
        match work
            .accept_native_output(plan, output.unwrap_or_default(), retry_at)
            .await
        {
            Ok(()) => respond(json!({"state":"processed"})),
            Err(code) => {
                eprintln!("daemon: native memory capture submission failed: {code}");
                if work.failed(&jobs, "store_failed", retry_at).is_err() {
                    return respond(json!({"state":"store_failed"}));
                }
                respond(json!({"state":"pending"}))
            }
        }
    }
}

impl CaptureWork {
    async fn next_native_plan(
        &self,
        primary: Option<&str>,
    ) -> Result<Option<(NativePlan, String)>, &'static str> {
        let project = self.binding.project_root.to_string_lossy();
        let jobs = self
            .store
            .pending_memory_captures(&project, &self.binding.harness, now_ms())
            .map_err(|_| "store_failed")?;
        let mut unprepared = Vec::new();
        for job in jobs {
            if job.prepared.is_some() {
                let id = job.job_id.clone();
                let frozen = job.prepared.clone().unwrap_or_default();
                if let Err(code) = self.publish(job).await {
                    if code == "reconciliation_conflict" {
                        self.store
                            .retry_memory_capture_reconciliation(&project, &id, &frozen)
                            .map_err(|_| "store_failed")?;
                    } else {
                        self.store
                            .fail_memory_capture(
                                &project,
                                &id,
                                "kernel_write_failed",
                                now_ms().saturating_add(5000),
                                false,
                                now_ms(),
                            )
                            .map_err(|_| "store_failed")?;
                    }
                }
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
        let failures = unprepared.iter().map(|job| job.failures).max().unwrap_or(0);
        let model = models[failures as usize % models.len()].clone();
        if !valid_capture_model(&model) {
            return Err("invalid_model_configuration");
        }
        let mut messages = messages_for(&unprepared);
        let mut existing = self.existing_memories(&messages).await?;
        let prompt = loop {
            let prompt = render_capture_prompt_with_existing(&messages, &existing)?;
            if prompt.len() <= MAX_NATIVE_PROMPT_BYTES {
                break prompt;
            }
            if existing.pop().is_none() {
                if unprepared.len() <= 1 {
                    return Err("source_too_large");
                }
                unprepared.pop();
                messages = messages_for(&unprepared);
            }
        };
        for job in &unprepared {
            if !self
                .store
                .begin_memory_capture_attempt(&job.project, &job.job_id, now_ms())
                .map_err(|_| "store_failed")?
            {
                return Ok(None);
            }
        }
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

    async fn accept_native_output(
        &self,
        plan: NativePlan,
        output: &str,
        retry_at: i64,
    ) -> Result<(), &'static str> {
        let decisions = match parse_capture_batch(output, &plan.messages, &plan.existing) {
            Ok(decisions) => decisions,
            Err(reason) => {
                eprintln!("daemon: native memory capture output rejected: {reason}");
                return self.failed(&plan.jobs, "extraction_failed", retry_at);
            }
        };
        for (mut job, decision) in plan.jobs.into_iter().zip(decisions) {
            let decision = match decision {
                Ok(decision) => decision,
                Err(reason) => {
                    eprintln!("daemon: native memory capture source rejected: {reason}");
                    self.failed(std::slice::from_ref(&job), "extraction_failed", retry_at)?;
                    continue;
                }
            };
            let existing = plan
                .existing
                .iter()
                .filter(|previous| {
                    decision.memories.iter().any(|memory| {
                        memory.replaces.as_deref() == Some(&previous.id)
                            || memory.duplicate_of.as_deref() == Some(&previous.id)
                    })
                })
                .cloned()
                .collect();
            let memories = decision
                .memories
                .into_iter()
                .map(|mut memory| {
                    let attribution = if job.role == "user" {
                        "User stated"
                    } else {
                        "Assistant reported"
                    };
                    memory.content = format!("{attribution}: {}", memory.quote);
                    memory
                })
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
                        self.failed(std::slice::from_ref(&job), "extraction_failed", retry_at)?;
                        continue;
                    }
                    Err(_) => return Err("store_failed"),
                };
            if let Some(frozen) = prepared {
                let id = job.job_id.clone();
                let project = job.project.clone();
                job.prepared = Some(frozen.clone());
                if let Err(code) = self.publish(job).await {
                    if code == "reconciliation_conflict" {
                        self.store
                            .retry_memory_capture_reconciliation(&project, &id, &frozen)
                            .map_err(|_| "store_failed")?;
                    } else {
                        self.store
                            .fail_memory_capture(
                                &project,
                                &id,
                                "kernel_write_failed",
                                now_ms().saturating_add(5000),
                                false,
                                now_ms(),
                            )
                            .map_err(|_| "store_failed")?;
                    }
                }
            }
        }
        Ok(())
    }
}
