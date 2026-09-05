//! (R3, R28).
//!

pub mod backend;
pub mod config;
pub mod opencode;
pub mod pi;
pub mod protocol;
pub mod subprocess;
pub mod supervisor;

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};

use crate::composite::{CompositeComponent, SecondaryComponent, ShutdownError};
use crate::handler::{
    BindOutcome, HealthReport, HealthStatus, InitError, ManifestSnapshot, RequestCtx,
    RequestOutcome, ResourceDeclaration, RouteClass, RouteHandle, RouteIdentity,
};
use subtle::ConstantTimeEq;

use backend::{Harness, LlmExecutionBackend};
use protocol::{Request, RequestError};
use subprocess::EnvSnapshot;
use subprocess::group_registry::StateRoot;
use supervisor::{SessionKey, Supervisor};

pub const BROCA_MODULE_ID: &str = "broca";

pub struct BrocaComponent {
    supervisor: Arc<Supervisor>,
    /// Each route handle maps to the session key validated at bind time.
    /// The map resolves body-less `subscribe` and `delete` requests to their session scope.
    routes: Arc<Mutex<HashMap<RouteHandle, SessionKey>>>,
    route_fingerprints: Arc<Mutex<HashMap<RouteHandle, BTreeMap<String, String>>>>,
    credential_verifier: Option<Arc<CredentialVerifier>>,
    /// The same root the backends write crash-ownership records to; `initialize` sweeps it.
    state_root: StateRoot,
}

struct CredentialVerifier {
    env: EnvSnapshot,
    key: OnceLock<[u8; 32]>,
}

impl CredentialVerifier {
    fn verify(
        &self,
        harness: Harness,
        provider: &str,
        presented: &BTreeMap<String, String>,
    ) -> Result<(), &'static str> {
        let key = self.key.get().ok_or("credential_snapshot_mismatch")?;
        let canonical = subprocess::canonical_provider(harness.as_str(), provider)
            .map_err(|error| error.subreason())?;
        let expected = self
            .env
            .credential_fingerprint(key, harness.as_str(), provider)
            .map_err(|error| error.subreason())?;
        let actual = presented
            .get(canonical)
            .ok_or("credential_snapshot_mismatch")?;
        if expected.as_bytes().ct_eq(actual.as_bytes()).into() {
            Ok(())
        } else {
            Err("credential_snapshot_mismatch")
        }
    }
}

impl BrocaComponent {
    pub fn new(backend: Arc<dyn LlmExecutionBackend>, state_root: StateRoot) -> Self {
        Self {
            supervisor: Arc::new(Supervisor::new(backend)),
            routes: Arc::new(Mutex::new(HashMap::new())),
            route_fingerprints: Arc::new(Mutex::new(HashMap::new())),
            credential_verifier: None,
            state_root,
        }
    }

    pub fn new_with_credentials(
        backend: Arc<dyn LlmExecutionBackend>,
        env: EnvSnapshot,
        state_root: StateRoot,
    ) -> Self {
        Self {
            supervisor: Arc::new(Supervisor::new(backend)),
            routes: Arc::new(Mutex::new(HashMap::new())),
            route_fingerprints: Arc::new(Mutex::new(HashMap::new())),
            credential_verifier: Some(Arc::new(CredentialVerifier {
                env,
                key: OnceLock::new(),
            })),
            state_root,
        }
    }

    pub fn supervisor(&self) -> Arc<Supervisor> {
        Arc::clone(&self.supervisor)
    }

    fn key_of_route(&self, route: RouteHandle) -> Option<SessionKey> {
        self.routes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&route)
            .cloned()
    }
}

fn request_error(error: RequestError) -> RequestOutcome {
    RequestOutcome::error(error.code, error.message)
}

fn app_error(code: &str, message: &str) -> RequestOutcome {
    RequestOutcome::error(code, message)
}

async fn respond(ctx: &RequestCtx, body: Vec<u8>) -> RequestOutcome {
    let Ok(mut output) = ctx.reserve_output(body.len()).await else {
        return app_error("internal_error", "output reservation failed");
    };
    if output.extend_from_slice(&body).is_err() {
        return app_error("internal_error", "output reservation too small");
    }
    RequestOutcome::Response {
        body: output,
        binary: false,
    }
}

impl CompositeComponent for BrocaComponent {
    fn install_connection_key(&self, key: [u8; 32]) {
        if let Some(verifier) = &self.credential_verifier {
            let _ = verifier.key.set(key);
        }
    }

    fn manifest(&self) -> ManifestSnapshot {
        ManifestSnapshot {
            module_id: BROCA_MODULE_ID.to_owned(),
            module_version: env!("CARGO_PKG_VERSION").to_owned(),
            provides: vec![serde_json::json!({"role": "management_surface"})],
            control_ops: Vec::new(),
        }
    }

    fn resources(&self) -> ResourceDeclaration {
        // These constants declare the product contract independently of the supervisor limit.
        ResourceDeclaration {
            reserved_handler_tasks: config::RESERVED_HANDLER_TASKS,
            reserved_pending_requests: config::RESERVED_PENDING_REQUESTS,
            retained_resident_bytes: config::DECLARED_RETAINED_RESIDENT_BYTES,
            general_task_hold_bound: 0,
            route_class: RouteClass::Reserved,
        }
    }

    async fn bind(&self, route: RouteHandle, identity: RouteIdentity) -> BindOutcome {
        // Route claims scope runs but grant no authority.
        // Request validation requires only fields needed to construct the supervisor key.
        // Messages carry no authority-bearing values.
        if identity.project_root.as_os_str().is_empty() || !identity.project_root.is_absolute() {
            return BindOutcome::Reject {
                code: "invalid_identity".to_owned(),
                message: "project_root must be a nonempty absolute path".to_owned(),
            };
        }
        if identity.session.is_empty() {
            return BindOutcome::Reject {
                code: "invalid_identity".to_owned(),
                message: "session must be nonempty".to_owned(),
            };
        }
        let Some(harness) = Harness::parse(&identity.harness) else {
            return BindOutcome::Reject {
                code: "invalid_identity".to_owned(),
                message: "harness must be \"opencode\" or \"pi\"".to_owned(),
            };
        };
        let key = SessionKey {
            project_root: identity.project_root,
            harness,
            session: identity.session,
        };
        let credential_fingerprints = identity.credential_fingerprints;
        let mut routes = self
            .routes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Binding beyond `MAX_BOUND_ROUTES` retains bytes outside the reservation when the host route limit is higher.
        if routes.len() >= config::MAX_BOUND_ROUTES && !routes.contains_key(&route) {
            return BindOutcome::Reject {
                code: "queue_full".to_owned(),
                message: "broca route capacity is exhausted".to_owned(),
            };
        }
        routes.insert(route, key);
        self.route_fingerprints
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(route, credential_fingerprints);
        BindOutcome::Accept
    }

    async fn handle(&self, ctx: RequestCtx) -> RequestOutcome {
        let Some(key) = self.key_of_route(ctx.route) else {
            return app_error("internal_error", "route is not bound to a broca session");
        };
        // The handler reserves `ctx.body.len() + 512` bytes before parsing so parser allocations count against resident capacity.
        let Some(scratch) = ctx.try_reserve_resident(ctx.body.len() + 512) else {
            return app_error(
                "queue_full",
                "resident capacity for request handling is exhausted",
            );
        };
        let request = match protocol::parse_request(&ctx.body, ctx.binary) {
            Ok(request) => request,
            Err(error) => return request_error(error),
        };
        match request {
            Request::Send(send) => {
                // A byte-identical resend of a live run resolves before any admission gate so a client recovering a lost response is not rejected by a harness that became unavailable after the run started. commentlint: allow(JUDGE)
                if let Some(run_id) = self.supervisor.resend_of_live_run(&key, &ctx.body) {
                    return respond(&ctx, protocol::send_response_body(&run_id)).await;
                }
                // The handler checks harness availability before credentials because descriptor failures take precedence over credential failures.
                // The probe opens, hashes, and stats closure nodes, so it runs on the blocking pool like the execution-path resolution; a stalled closure store must not occupy runtime workers. commentlint: allow(JUDGE)
                let supervisor = Arc::clone(&self.supervisor);
                let harness = key.harness;
                let probe =
                    subprocess::off_runtime(move || supervisor.harness_unavailable_reason(harness))
                        .await;
                match probe {
                    Ok(None) => {}
                    Ok(Some(reason)) => return app_error("harness_unavailable", reason),
                    Err(_) => return app_error("harness_unavailable", "closure_unverified"),
                }
                if let Some(verifier) = &self.credential_verifier {
                    let fingerprints = self
                        .route_fingerprints
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .get(&ctx.route)
                        .cloned()
                        .unwrap_or_default();
                    if let Err(reason) = verifier.verify(key.harness, &send.provider, &fingerprints)
                    {
                        return app_error("harness_unavailable", reason);
                    }
                }
                match self.supervisor.send(&key, send, &ctx.body) {
                    Ok(run_id) => respond(&ctx, protocol::send_response_body(&run_id)).await,
                    Err(error) => request_error(error),
                }
            }
            Request::Status { run_id } => match self.supervisor.status(&key, &run_id) {
                Ok(state) => respond(&ctx, protocol::status_response_body(&run_id, state)).await,
                Err(error) => request_error(error),
            },
            Request::Cancel { run_id } => match self.supervisor.cancel(&key, &run_id).await {
                Ok(()) => respond(&ctx, protocol::ok_response_body()).await,
                Err(error) => request_error(error),
            },
            Request::Delete => match self.supervisor.delete(&key).await {
                Ok(()) => respond(&ctx, protocol::ok_response_body()).await,
                Err(error) => request_error(error),
            },
            Request::Subscribe => {
                // `Subscribe` carries no parsed payload, and its streaming loop can hold this
                // request open for minutes; releasing the parse reservation here keeps resident
                // capacity scaled to in-flight parses rather than live subscriptions.
                drop(scratch);
                let mut subscription = match self.supervisor.subscribe(&key) {
                    Ok(subscription) => subscription,
                    Err(error) => return request_error(error),
                };
                loop {
                    let unit = tokio::select! {
                        biased;
                        // Cancelling a request drops its `Subscription` without cancelling the run.
                        // Dropping the `Subscription` detaches the waiter without cancelling the run.
                        // continues untouched.
                        () = ctx.cancelled() => break,
                        unit = subscription.next() => unit,
                    };
                    let Some(unit) = unit else { break };
                    let Ok(mut item) = ctx.reserve_output(unit.len()).await else {
                        break;
                    };
                    if item.extend_from_slice(&unit).is_err() {
                        break;
                    }
                    if ctx.stream(item, false).await.is_err() {
                        // Route closure and TCP loss drop the `Subscription` without cancelling the run.
                        break;
                    }
                }
                RequestOutcome::Streamed
            }
        }
    }

    async fn route_gone(&self, route: RouteHandle) {
        // After route shutdown, the handler removes only the identity mapping; dropped `Subscription` waiters do not cancel runs.
        // Runs outlive their routes.
        self.routes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&route);
        self.route_fingerprints
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&route);
    }

    async fn health(&self) -> HealthReport {
        // The wire contract admits `ready | unavailable`; `unavailable` means no supported harness can run.
        // A harness whose descriptor resolves but whose snapshot holds no usable credential rejects every send at `CredentialVerifier::verify`, so it cannot run either. commentlint: allow(JUDGE)
        // The descriptor probes open, hash, and stat closure nodes, so they run on the blocking pool; a probe that cannot complete reads as unavailable. commentlint: allow(JUDGE)
        let supervisor = Arc::clone(&self.supervisor);
        let descriptor_unavailable = subprocess::off_runtime(move || {
            [Harness::OpenCode, Harness::Pi]
                .map(|harness| supervisor.harness_unavailable_reason(harness).is_some())
        })
        .await
        .unwrap_or([true, true]);
        let unavailable = [Harness::OpenCode, Harness::Pi]
            .into_iter()
            .zip(descriptor_unavailable)
            .all(|(harness, descriptor_unavailable)| {
                descriptor_unavailable
                    || self.credential_verifier.as_ref().is_some_and(|verifier| {
                        !verifier.env.any_credential_available(harness.as_str())
                    })
            });
        if unavailable {
            return HealthReport {
                status: HealthStatus::Degraded,
                detail: None,
                metrics: Some(serde_json::json!({"broca_state": "unavailable"})),
            };
        }
        HealthReport {
            status: HealthStatus::Ok,
            detail: None,
            metrics: Some(serde_json::json!({"broca_state": "ready"})),
        }
    }

    async fn shutdown(&self) -> Result<(), ShutdownError> {
        // One run can leave several residue classes at once; suppressing any of them would hide
        // caller-private material or registry state, so all are aggregated into one error.
        let unresolved = self.supervisor.shutdown().await;
        self.routes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.route_fingerprints
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        let mut failures = Vec::new();
        // Teardown failures lead because a provider descendant may still be running.
        if unresolved > 0 {
            failures.push(format!(
                "{unresolved} run(s) ended without confirming harness \
                 process-group teardown; provider work may still be running"
            ));
        }
        // Cleanup residue outranks a clean report: caller-private prompt material may remain on disk.
        let residue = self.supervisor.cleanup_unresolved_runs();
        if residue > 0 {
            failures.push(format!(
                "{residue} run(s) could not remove their private run files; \
                 caller-private prompt material may remain on disk"
            ));
        }
        // A retained crash-ownership record remains in the registry for a successor to sweep.
        let records = self.supervisor.record_unresolved_runs();
        if records > 0 {
            failures.push(format!(
                "{records} run(s) could not remove their crash-ownership records; \
                 the registry retains them for a later sweep"
            ));
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(ShutdownError(failures.join("; ")))
        }
    }
}

impl SecondaryComponent for BrocaComponent {
    async fn initialize(&self) -> Result<(), InitError> {
        // The crash-ownership registry requires Linux `/proc` start times, boot IDs, and process-group scans to prove process identity.
        // The handler rejects unsupported platforms before attempting `/proc` reads.
        if cfg!(not(target_os = "linux")) {
            return Err(InitError(
                "broca requires Linux: crash-ownership records and sweeps \
                 depend on /proc process identity"
                    .to_owned(),
            ));
        }
        // Both sweeps walk `/proc` and may block for the member grace; the composite initializes siblings concurrently on one task, so the sweeps run on the blocking pool.
        let state_root = self.state_root.clone();
        let swept = tokio::task::spawn_blocking(move || {
            subprocess::group_registry::sweep_orphaned_groups(&state_root).map_err(|err| {
                InitError(format!(
                    "broca could not sweep crash-orphaned process groups: {err}"
                ))
            })?;
            subprocess::group_registry::sweep_orphaned_run_dirs(&state_root).map_err(|err| {
                InitError(format!(
                    "broca could not sweep crash-orphaned run directories: {err}"
                ))
            })?;
            Ok(())
        })
        .await;
        match swept {
            Ok(result) => result,
            Err(join) => Err(InitError(format!(
                "broca crash-orphan sweep did not complete: {join}"
            ))),
        }
    }
}
