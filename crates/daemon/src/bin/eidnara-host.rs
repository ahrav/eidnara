//! `eidnara-host` is the lifecycle and serve executable.
//!
//! `eidnara-host` depends on `daemon` and `host-runtime`; neither dependency depends on `eidnara-host`.
//! `--version`, `release-info`, and `input-lock-digest` have no side effects.
//! Each lifecycle command emits exactly one `eidnara.daemon/v1` JSON object on stdout.
//! Exit 0 means `ok:true`; exit 1 indicates an operational failure.
//! Exit 2 indicates a usage error and makes no lifecycle call.

#![deny(unsafe_code)]

#[path = "eidnara_host/serve.rs"]
mod serve;
#[path = "eidnara_host/spawn.rs"]
mod spawn;

use std::collections::BTreeSet;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use daemon::release_contract;
use host_runtime::generation::{GenerationError, GenerationStore, SourceSpec, StageMeta};
use host_runtime::{
    Client, InstanceError, LifecycleProbe, LifecycleState, LifecycleTransactionLock,
    NamespaceAnchor, ProbeFreshness, SendOutcome,
};

// Lifecycle deadlines.

/// The outer aggregate caps fresh Linux request-to-authenticated-transport handling at 60 seconds.
const OUTER_AGGREGATE: Duration = Duration::from_secs(60);
/// The 10-second spawn/publication/auth budget covers staged-generation revalidation and retained harness closure hashing before publication.
/// Before publication, `serve` revalidates the staged generation and hashes every retained harness closure node.
/// Both qualified harnesses can require hashing hundreds of megabytes of closure nodes.
/// The budget covers cold-page-cache reads of the retained harness closure nodes.
const SPAWN_PUBLICATION_AUTH: Duration = Duration::from_secs(10);
/// The teardown budget covers committed shutdown through publication removal and acquisition of both fences.
const STOP_TEARDOWN: Duration = Duration::from_secs(10);
/// The command waits at most 5 seconds for an observed `starting` or `stopping` transition before reporting `lifecycle_busy`.
const TRANSITION_SETTLE: Duration = Duration::from_secs(5);
const REPROBE_INTERVAL: Duration = Duration::from_millis(100);
/// The 500 ms close grace prevents a stalled peer from hanging the phase after a proven handshake.
/// Expiry after a proven handshake does not change the authentication verdict.
/// The handshake settles the authentication verdict before close grace begins.
const CLOSE_GRACE: Duration = Duration::from_millis(500);

/// The override can lengthen only the spawn/publication/auth and teardown caps.
/// The aggregate deadline prevents any phase-cap value from causing an unbounded wait.
/// Release builds ignore the variable, like the other `EIDNARA_HOST_TEST_*` overrides.
#[cfg(debug_assertions)]
const PHASE_CAP_ENV: &str = "EIDNARA_HOST_TEST_PHASE_CAP_MS";

#[cfg(debug_assertions)]
fn phase_cap(default: Duration) -> Duration {
    let raw = std::env::var_os(PHASE_CAP_ENV);
    phase_cap_override(default, raw.as_deref().and_then(|value| value.to_str()))
}

#[cfg(not(debug_assertions))]
fn phase_cap(default: Duration) -> Duration {
    default
}

#[cfg(debug_assertions)]
fn phase_cap_override(default: Duration, raw: Option<&str>) -> Duration {
    match raw.and_then(|value| value.parse::<u64>().ok()) {
        Some(ms) if ms > 0 => default.max(Duration::from_millis(ms)).min(OUTER_AGGREGATE),
        _ => default,
    }
}

fn phase_deadline(outer: Instant, cap: Duration) -> Instant {
    let now = Instant::now();
    let remaining = outer.saturating_duration_since(now);
    now + cap.min(remaining)
}

/// Deadline for the successor's publication and authentication evidence.
///
/// After a committed stop the successor spawn is the only path that restores service, so
/// the wait keeps the full cap even when `outer` has already passed; clamping it would
/// report `startup_timeout` for a daemon that then publishes. Without a committed stop the
/// aggregate deadline still bounds the wait.
fn publication_deadline(outer: Instant, cap: Duration, stop_committed: bool) -> Instant {
    if stop_committed {
        Instant::now() + cap
    } else {
        phase_deadline(outer, cap)
    }
}

// Result reasons use the closed v1 vocabulary.

const SCHEMA: &str = "eidnara.daemon/v1";
/// Reason for a running incarnation whose credential did not authenticate; `finish` also fails the publication check on it.
const AUTHENTICATION_FAILED: &str = "authentication_failed";

fn remediation_for(reason: &'static str) -> Option<&'static str> {
    match reason {
        "internal_error" => Some("report_bug"),
        "no_data_dir" | "unsupported_filesystem" => Some("set_data_directory"),
        "unsupported_platform" => Some("use_supported_platform"),
        "unsupported_install_layout" => Some("use_supported_install_layout"),
        "unsupported_state_schema" => Some("align_versions"),
        "native_payload_invalid" => Some("reinstall_eidnara"),
        "native_payload_missing" => Some("install_native_payload"),
        "insufficient_storage" => Some("free_storage"),
        "native_probe_unavailable" => Some("run_daemon_restart"),
        "wedged"
        | "publication_invalid"
        | "publication_stale"
        | "publication_missing"
        | "authentication_failed"
        | "shutdown_timeout"
        | "startup_timeout" => Some("inspect_daemon_process"),
        "unsupported_proof_version"
        | "incompatible_control"
        | "incompatible_daemon"
        | "incompatible_module"
        | "incompatible_epochs" => Some("align_versions"),
        "lifecycle_busy" | "storage_starting" | "kernel_starting" | "synapse_starting"
        | "stopping" | "starting" => Some("wait_and_retry"),
        "storage_unavailable" | "kernel_unavailable" => Some("inspect_storage"),
        "synapse_degraded" => Some("inspect_synapse"),
        "not_running" => Some("run_daemon_start"),
        // Mirrors `warn_remediations` in `RELEASE_CONTRACT_JSON`.
        "kernel_capacity_warn" => Some("inspect_storage"),
        "kernel_lagging" => Some("inspect_kernel_projector"),
        _ => None,
    }
}

#[derive(serde::Serialize, Clone, Copy)]
struct Effects {
    stop_committed: bool,
    start_committed: bool,
}

#[derive(serde::Serialize)]
struct Check {
    id: &'static str,
    status: &'static str,
    reason: &'static str,
    remediation: Option<&'static str>,
}

#[derive(serde::Serialize, Default)]
struct Versions {
    release: Option<&'static str>,
    proof: Option<&'static str>,
    daemon: Option<String>,
    context: Option<String>,
    synapse: Option<String>,
    broca: Option<String>,
}

impl Versions {
    /// Versions this executable knows without observing a daemon: the release and the module versions the embedded contract carries.
    fn local() -> Self {
        Versions {
            release: Some(release_contract::RELEASE_VERSION),
            context: Some(release_contract::CONTEXT_MODULE_VERSION.to_owned()),
            synapse: Some(release_contract::SYNAPSE_MODULE_VERSION.to_owned()),
            broca: Some(release_contract::BROCA_MODULE_VERSION.to_owned()),
            ..Versions::default()
        }
    }
}

#[derive(serde::Serialize)]
struct DaemonResult {
    schema: &'static str,
    command: &'static str,
    ok: bool,
    state: &'static str,
    reason: &'static str,
    remediation: Option<&'static str>,
    effects: Option<Effects>,
    checks: Vec<Check>,
    versions: Versions,
}

impl DaemonResult {
    fn new(command: &'static str, ok: bool, state: &'static str, reason: &'static str) -> Self {
        DaemonResult {
            schema: SCHEMA,
            command,
            ok,
            state,
            reason,
            remediation: remediation_for(reason),
            effects: None,
            checks: Vec::new(),
            versions: Versions::local(),
        }
    }

    fn with_effects(mut self, effects: Effects) -> Self {
        self.effects = Some(effects);
        self
    }

    fn finish(mut self) -> Self {
        // Applicable checks derive from the final verdict.
        // The lifecycle check reports lock coherence.
        // The publication check reports whether a running incarnation's credential was observed, so a running incarnation whose credential failed authentication fails it.
        // lexicographically sorted.
        let fences = match self.state {
            "wedged" => ("fail", "wedged"),
            "unavailable" => ("skip", "healthy"),
            _ => ("pass", "healthy"),
        };
        let publication = match (self.state, self.reason) {
            ("running", AUTHENTICATION_FAILED) => ("fail", AUTHENTICATION_FAILED),
            ("running", _) => ("pass", "healthy"),
            ("wedged", _) => ("fail", "wedged"),
            _ => ("skip", "healthy"),
        };
        let mut checks = std::mem::take(&mut self.checks);
        checks.push(Check {
            id: "lifecycle.fences",
            status: fences.0,
            reason: fences.1,
            remediation: remediation_for(fences.1),
        });
        checks.push(Check {
            id: "lifecycle.publication",
            status: publication.0,
            reason: publication.1,
            remediation: remediation_for(publication.1),
        });
        checks.sort_by(|a, b| a.id.cmp(b.id));
        self.checks = checks;
        self
    }
}

// Command-line parsing.

enum Command {
    Version,
    ReleaseInfo,
    InputLockDigest,
    Status,
    Start {
        payload_dir: Option<PathBuf>,
        payload_manifest_digest: Option<String>,
    },
    Stop,
    Restart {
        payload_dir: Option<PathBuf>,
        payload_manifest_digest: Option<String>,
    },
    Serve,
}

const USAGE: &str = "usage: eidnara-host <serve|start|stop|restart|status|probe|release-info|input-lock-digest> [--payload-dir <dir> --payload-manifest-digest <sha256>] | --version (probe is an alias of status)";

fn parse_args(args: &[std::ffi::OsString]) -> Result<Command, String> {
    let mut iter = args.iter();
    let Some(first) = iter.next() else {
        return Err("missing command".to_owned());
    };
    let Some(first) = first.to_str() else {
        return Err("command is not valid UTF-8".to_owned());
    };
    let mut payload_dir: Option<PathBuf> = None;
    let mut payload_manifest_digest: Option<String> = None;
    let takes_payload = matches!(first, "start" | "restart");
    while let Some(arg) = iter.next() {
        let Some(arg) = arg.to_str() else {
            return Err("argument is not valid UTF-8".to_owned());
        };
        if takes_payload && arg == "--payload-dir" {
            if payload_dir.is_some() {
                return Err("duplicate --payload-dir".to_owned());
            }
            let Some(value) = iter.next() else {
                return Err("--payload-dir requires a value".to_owned());
            };
            if value.is_empty() || value.as_encoded_bytes().starts_with(b"--") {
                return Err("--payload-dir requires a nonempty value".to_owned());
            }
            payload_dir = Some(PathBuf::from(value));
        } else if takes_payload && arg == "--payload-manifest-digest" {
            if payload_manifest_digest.is_some() {
                return Err("duplicate --payload-manifest-digest".to_owned());
            }
            let Some(value) = iter.next().and_then(|value| value.to_str()) else {
                return Err("--payload-manifest-digest requires a UTF-8 value".to_owned());
            };
            if value.starts_with("--") {
                return Err("--payload-manifest-digest requires a value".to_owned());
            }
            if value.len() != 64
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            {
                return Err("--payload-manifest-digest requires lowercase SHA-256".to_owned());
            }
            payload_manifest_digest = Some(value.to_owned());
        } else {
            return Err(format!("unexpected argument: {arg}"));
        }
    }
    match first {
        "--version" => Ok(Command::Version),
        "release-info" => Ok(Command::ReleaseInfo),
        "input-lock-digest" => Ok(Command::InputLockDigest),
        "status" | "probe" => Ok(Command::Status),
        "start" => Ok(Command::Start {
            payload_dir,
            payload_manifest_digest,
        }),
        "stop" => Ok(Command::Stop),
        "restart" => Ok(Command::Restart {
            payload_dir,
            payload_manifest_digest,
        }),
        "serve" => Ok(Command::Serve),
        other => Err(format!("unknown command: {other}")),
    }
}

// -------------------------------------------------------------------------
// The process never serializes native error detail.
// Only closed static reasons leave this process.
// -------------------------------------------------------------------------

fn instance_failure(error: &InstanceError) -> (&'static str, &'static str) {
    match error {
        InstanceError::NoDataDir => ("unavailable", "no_data_dir"),
        InstanceError::UnsupportedPlatform => ("stopped", "unsupported_platform"),
        InstanceError::AlreadyRunning => ("stopped", "lifecycle_busy"),
        InstanceError::UnsupportedStateSchema { .. } => ("wedged", "unsupported_state_schema"),
        InstanceError::Insecure { .. } | InstanceError::NamespaceDrift { .. } => {
            ("wedged", "wedged")
        }
        InstanceError::Io { .. } | InstanceError::InvalidPayloadDigest | InstanceError::Random => {
            ("wedged", "internal_error")
        }
    }
}

fn generation_failure(error: &GenerationError) -> (&'static str, &'static str) {
    match error {
        GenerationError::InsufficientStorage => ("stopped", "insufficient_storage"),
        GenerationError::NativePayloadInvalid { .. } => ("stopped", "native_payload_invalid"),
        GenerationError::UnsupportedStateSchema => ("stopped", "unsupported_state_schema"),
        GenerationError::Instance(inner) => instance_failure(inner),
    }
}

/// Observes on-disk lifecycle state after a pre-commit rejection.
fn unchanged_state() -> &'static str {
    match probe() {
        Ok(observed) => probe_state(observed.state),
        Err(error) => instance_failure(&error).0,
    }
}

fn probe_state(state: LifecycleState) -> &'static str {
    match state {
        LifecycleState::Stopped => "stopped",
        LifecycleState::Starting => "starting",
        LifecycleState::Running => "running",
        LifecycleState::Stopping => "stopping",
        LifecycleState::Wedged => "wedged",
    }
}

/// Maps an unsupported state schema to its observed lifecycle state and closed reason.
fn quarantined_observation(observed: &LifecycleProbe) -> Option<(&'static str, &'static str)> {
    if observed.reason != host_runtime::UNSUPPORTED_STATE_SCHEMA_REASON {
        return None;
    }
    Some((
        probe_state(observed.state),
        host_runtime::UNSUPPORTED_STATE_SCHEMA_REASON,
    ))
}

// Lifecycle observation helpers.

fn probe() -> Result<LifecycleProbe, InstanceError> {
    host_runtime::probe_lifecycle(None, &ProbeFreshness::default())
}

fn settle_probe(deadline: Instant) -> Result<LifecycleProbe, InstanceError> {
    loop {
        let observed = probe()?;
        match observed.state {
            LifecycleState::Starting | LifecycleState::Stopping if Instant::now() < deadline => {
                std::thread::sleep(REPROBE_INTERVAL);
            }
            _ => return Ok(observed),
        }
    }
}

fn publication_path() -> Result<PathBuf, InstanceError> {
    Ok(host_runtime::runtime_dir_path(None)?.join(host_runtime::CONNECTION_FILE_NAME))
}

fn daemon_log_path() -> Result<PathBuf, InstanceError> {
    Ok(host_runtime::coordination_dir_path(None)?.join("eidnara.log"))
}

/// `Runtime::shutdown` outcomes; `stop_phase` matches these to decide whether a commit probe is required.
const SHUTDOWN_AUTHENTICATION_FAILED: &str = "authentication_failed";
const SHUTDOWN_OUTCOME_UNKNOWN: &str = "shutdown_outcome_unknown";
const SHUTDOWN_FAILED: &str = "shutdown_failed";

struct Runtime {
    inner: tokio::runtime::Runtime,
}

impl Runtime {
    fn new() -> Result<Self, &'static str> {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map(|inner| Runtime { inner })
            .map_err(|_| "tokio runtime construction failed")
    }

    fn authenticate(&self, publication: &Path, deadline: Instant) -> Option<String> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        let path = publication.to_path_buf();
        self.inner.block_on(async move {
            // Only `Client::connect` uses `remaining`; `client.close()` uses `CLOSE_GRACE`.
            // `client.close()` runs after `daemon_ver` is saved and cannot change the returned value.
            match tokio::time::timeout(remaining, Client::connect(&path)).await {
                Ok(Ok(client)) => {
                    let daemon_ver = client.daemon_ver().to_owned();
                    let _ = tokio::time::timeout(CLOSE_GRACE, client.close()).await;
                    Some(daemon_ver)
                }
                _ => None,
            }
        })
    }

    /// `host.shutdown` requires authentication; `Ok` means the full acknowledgement frame was received.
    ///
    /// `Err(SHUTDOWN_OUTCOME_UNKNOWN)` reports an unknown shutdown outcome, not a definite failure.
    fn shutdown(&self, publication: &Path, deadline: Instant) -> Result<(), &'static str> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(SHUTDOWN_FAILED);
        }
        let path = publication.to_path_buf();
        self.inner.block_on(async move {
            // The caller's deadline bounds each attempt in addition to the client's timeouts.
            // An unreachable or stalled peer cannot let an attempt exceed the caller's reserved budget.
            // A timeout reports an unknown outcome because the request may already have been written.
            match tokio::time::timeout(remaining, async {
                let client = Client::connect(&path)
                    .await
                    .map_err(|_| SHUTDOWN_AUTHENTICATION_FAILED)?;
                let result = client.host_shutdown().await;
                let _ = client.close().await;
                result.map_err(|error| match error.outcome() {
                    SendOutcome::OutcomeUnknown => SHUTDOWN_OUTCOME_UNKNOWN,
                    _ => SHUTDOWN_FAILED,
                })
            })
            .await
            {
                Ok(result) => result,
                Err(_) => Err(SHUTDOWN_OUTCOME_UNKNOWN),
            }
        })
    }
}

/// The publication daemon version is unauthenticated: the daemon wrote it, but no handshake proved it.
///
/// `status` reads the unauthenticated version fail-closed, so a forged value can withhold `ok` but never grant it.
/// Start, stop, and selection commit authenticate first and use the handshake's version.
fn publication_daemon_ver(observed: &LifecycleProbe) -> Option<String> {
    observed
        .publication
        .as_ref()
        .map(|publication| publication.daemon_ver.clone())
}

/// Applies the strict semantic-version shape and half-open range the harness lifecycle client mirrors in `evaluateDaemonCompatibility`.
fn daemon_version_compatible(daemon_ver: &str) -> bool {
    fn triple(version: &str) -> Option<[u64; 3]> {
        fn component(part: &str) -> Option<u64> {
            if part.is_empty()
                || !part.bytes().all(|byte| byte.is_ascii_digit())
                || (part.len() > 1 && part.starts_with('0'))
            {
                return None;
            }
            part.parse().ok()
        }
        let mut parts = version.split('.');
        let major = component(parts.next()?)?;
        let minor = component(parts.next()?)?;
        let patch = component(parts.next()?)?;
        if parts.next().is_some() {
            return None;
        }
        Some([major, minor, patch])
    }
    let Some(version) = daemon_ver.strip_prefix("eidnara-host/").and_then(triple) else {
        return false;
    };
    let contract: serde_json::Value = serde_json::from_str(release_contract::RELEASE_CONTRACT_JSON)
        .expect("embedded release contract parses");
    let range = &contract["versions"]["supported_daemon_range"];
    let bound = |key: &str| {
        range[key]
            .as_str()
            .and_then(triple)
            .expect("contract daemon range bounds parse")
    };
    version >= bound("min_inclusive") && version < bound("max_exclusive")
}

// -------------------------------------------------------------------------
// probe
// -------------------------------------------------------------------------

/// Observes the daemon without creating an instance or opening a connection.
///
/// `running` with a contract-conforming publication whose version is inside `supported_daemon_range` is `healthy` with `ok:true`.
/// A running incarnation outside that range is `incompatible_daemon`.
/// Proof stays null and the result carries no readiness object. `versions.daemon` is unauthenticated publication diagnostics.
fn cmd_probe() -> DaemonResult {
    let command = "status";
    let observed = match probe() {
        Ok(observed) => observed,
        Err(error) => {
            let (state, reason) = instance_failure(&error);
            return DaemonResult::new(command, false, state, reason);
        }
    };
    let (ok, reason) = probe_verdict(&observed);
    let mut result = DaemonResult::new(command, ok, probe_state(observed.state), reason);
    result.versions.daemon = publication_daemon_ver(&observed);
    result
}

fn probe_verdict(observed: &LifecycleProbe) -> (bool, &'static str) {
    if let Some((_, reason)) = quarantined_observation(observed) {
        return (false, reason);
    }
    match observed.state {
        LifecycleState::Running => match publication_daemon_ver(observed) {
            Some(daemon_ver) if !daemon_version_compatible(&daemon_ver) => {
                (false, "incompatible_daemon")
            }
            _ => (true, "healthy"),
        },
        LifecycleState::Stopped => (false, "not_running"),
        LifecycleState::Starting => (false, "starting"),
        LifecycleState::Stopping => (false, "stopping"),
        LifecycleState::Wedged => (false, "wedged"),
    }
}

// -------------------------------------------------------------------------
// start
// -------------------------------------------------------------------------

/// Result of resolving, spawning, publishing, and authenticating a successor.
struct StartOutcome {
    ok: bool,
    start_committed: bool,
    state: &'static str,
    reason: &'static str,
    daemon_ver: Option<String>,
    generation_check: Option<(&'static str, &'static str)>,
}

/// Generation input that is either unresolved or validated before a committed stop.
enum SuccessorGeneration<'a> {
    Resolve {
        payload_dir: Option<&'a Path>,
        payload_manifest_digest: Option<&'a str>,
    },
    Preflighted(ResolvedGeneration),
}

fn start_phase(
    runtime: &Runtime,
    generation: SuccessorGeneration<'_>,
    anchor: &NamespaceAnchor,
    outer: Instant,
    launcher_envelope: serve::PreparedLauncherEnvelope,
    stop_committed: bool,
) -> StartOutcome {
    // Resolution failure means no generation passed validation.
    let unresolved = |state: &'static str, reason: &'static str| StartOutcome {
        ok: false,
        start_committed: false,
        state,
        reason,
        daemon_ver: None,
        generation_check: Some(("fail", reason)),
    };
    let resolved_but_failed = |state: &'static str, reason: &'static str| StartOutcome {
        ok: false,
        start_committed: false,
        state,
        reason,
        daemon_ver: None,
        generation_check: Some(("pass", "healthy")),
    };

    let ResolvedGeneration {
        digest,
        launcher: generation_launcher,
    } = match generation {
        SuccessorGeneration::Preflighted(resolved) => resolved,
        SuccessorGeneration::Resolve {
            payload_dir,
            payload_manifest_digest,
        } => match resolve_generation(payload_dir, payload_manifest_digest, None) {
            Ok(resolved) => resolved,
            Err((state, reason)) => return unresolved(state, reason),
        },
    };

    if anchor.verify().is_err() {
        return resolved_but_failed("wedged", "wedged");
    }

    // Generation resolution can outlast `outer` on slow storage.
    // Spawning after `outer` expires would return `startup_timeout` while allowing a daemon to start later.
    // Refusing the spawn prevents a daemon from starting after `startup_timeout` is returned.
    //
    // After a committed stop, spawning is the only path that restores service.
    // After a committed stop, `outer` may expire without preventing the successor spawn.
    // After a committed stop, the aggregate may overrun `outer` to restore service.
    if !stop_committed && Instant::now() >= outer {
        return resolved_but_failed("stopped", "startup_timeout");
    }

    let envelope = launcher_envelope.to_startup(digest);
    let envelope_bytes = match serde_json::to_vec(&envelope) {
        Ok(bytes) => bytes,
        // Unix data-root paths can contain non-UTF-8 bytes that JSON cannot represent.
        Err(_) => return resolved_but_failed("stopped", "internal_error"),
    };
    let log_path = match daemon_log_path() {
        Ok(path) => path,
        Err(_) => return resolved_but_failed("stopped", "internal_error"),
    };
    let child = match spawn::spawn_detached(&log_path, &envelope_bytes, generation_launcher) {
        Ok(child) => child,
        Err(error) => {
            // The result reason vocabulary is closed, so the cause goes to stderr.
            match error.child_error {
                Some(child) => eprintln!("eidnara-host: {} ({child})", error.message),
                None => eprintln!("eidnara-host: {}", error.message),
            }
            return resolved_but_failed("stopped", "internal_error");
        }
    };

    // The loop waits for publication and authentication evidence, not the child PID.
    let deadline = publication_deadline(outer, phase_cap(SPAWN_PUBLICATION_AUTH), stop_committed);
    let publication = match publication_path() {
        Ok(path) => path,
        Err(_) => return resolved_but_failed("stopped", "internal_error"),
    };
    loop {
        // `runtime.authenticate` can authenticate a stale publication for another endpoint, so authentication alone cannot identify this root's daemon.
        // `probe()` returning `LifecycleState::Running` proves that a lock-held incarnation exists at this root.
        // The daemon publishes only after taking its fences.
        if let Some(daemon_ver) = publication
            .exists()
            .then(|| runtime.authenticate(&publication, deadline))
            .flatten()
            && let Ok(observed) = probe()
            && observed.state == LifecycleState::Running
        {
            // A predecessor launcher's orphaned child can win the instance lock, with any generation or harness envelope; `started` is claimed only for the incarnation this command spawned, identified by its unreaped child's PID in the lifecycle record.
            let own_incarnation = observed
                .record
                .as_ref()
                .is_some_and(|record| record.pid == child.pid());
            if !own_incarnation {
                child.terminate(phase_cap(STOP_TEARDOWN));
                return StartOutcome {
                    ok: false,
                    start_committed: false,
                    state: "running",
                    reason: "lifecycle_busy",
                    daemon_ver: Some(daemon_ver),
                    generation_check: Some(("pass", "healthy")),
                };
            }
            if !daemon_version_compatible(&daemon_ver) {
                // The incarnation is this command's child, so it is terminated directly; leaving it running would block a corrected start behind a daemon this command reported as a failure.
                child.terminate(phase_cap(STOP_TEARDOWN));
                return StartOutcome {
                    ok: false,
                    start_committed: true,
                    state: "stopped",
                    reason: "incompatible_daemon",
                    daemon_ver: Some(daemon_ver),
                    generation_check: Some(("pass", "healthy")),
                };
            }
            // The named publication path must still resolve to the namespace observed before the spawn; a replaced managed subtree would make `started` name a daemon clients cannot reach, so the daemon is stopped and the drift reported.
            if anchor.verify().is_err() {
                // The publication path now resolves into a replacement tree, so a path-based shutdown could reach a different daemon; the owned child is terminated through its PID instead.
                child.terminate(phase_cap(STOP_TEARDOWN));
                return StartOutcome {
                    ok: false,
                    start_committed: true,
                    state: "wedged",
                    reason: "wedged",
                    daemon_ver: Some(daemon_ver),
                    generation_check: Some(("pass", "healthy")),
                };
            }
            if launcher_envelope.commit_selection(&publication).is_err() {
                // Publication may legitimately complete after `outer` once a stop is committed, so this teardown gets its own bounded window instead of an already-expired one.
                let cleanup_outer = outer.max(Instant::now() + phase_cap(STOP_TEARDOWN));
                return match stop_phase(runtime, cleanup_outer) {
                    (_, Ok(())) => StartOutcome {
                        ok: false,
                        start_committed: true,
                        state: "stopped",
                        reason: "internal_error",
                        daemon_ver: Some(daemon_ver),
                        generation_check: Some(("pass", "healthy")),
                    },
                    (_, Err((state, reason))) => StartOutcome {
                        ok: false,
                        start_committed: true,
                        state,
                        reason,
                        daemon_ver: Some(daemon_ver),
                        generation_check: Some(("pass", "healthy")),
                    },
                };
            }
            return StartOutcome {
                ok: true,
                start_committed: true,
                state: "running",
                reason: "started",
                daemon_ver: Some(daemon_ver),
                generation_check: Some(("pass", "healthy")),
            };
        }
        if Instant::now() >= deadline {
            // A child still initializing past the cap could publish after this result is emitted, so `startup_timeout` terminates it first and reports the state observed afterward.
            child.terminate(phase_cap(STOP_TEARDOWN));
            // A child that exits before publishing leaves a coherent `stopped` observation.
            // Reporting a pre-publication child exit as `wedged` would falsely claim fence incoherence.
            let state = match probe().map(|observed| observed.state) {
                Ok(LifecycleState::Starting) => "starting",
                Ok(LifecycleState::Stopped) => "stopped",
                _ => "wedged",
            };
            return StartOutcome {
                ok: false,
                start_committed: false,
                state,
                reason: "startup_timeout",
                daemon_ver: None,
                generation_check: Some(("pass", "healthy")),
            };
        }
        std::thread::sleep(REPROBE_INTERVAL);
    }
}

/// `build_target` returns the target in the release contract's platform spelling.
///
/// Staging writes and selection compares `platforms.supported[].target`; sharing this definition prevents spelling drift.
const fn build_target() -> Option<&'static str> {
    if cfg!(all(
        target_os = "linux",
        target_arch = "x86_64",
        target_env = "gnu"
    )) {
        Some("linux-x64-gnu")
    } else {
        None
    }
}

/// `PlatformFloor` stores the runtime floor declared by one `platforms.supported` contract row.
struct PlatformFloor {
    kernel_min: String,
    glibc_min: String,
    procfs_self_fd_exec: bool,
}

fn platform_floor(target: &str) -> PlatformFloor {
    let contract: serde_json::Value = serde_json::from_str(release_contract::RELEASE_CONTRACT_JSON)
        .expect("embedded release contract parses");
    let row = contract["platforms"]["supported"]
        .as_array()
        .expect("contract lists supported platforms")
        .iter()
        .find(|row| row["target"] == target)
        .expect("contract declares the build target");
    let field = |key: &str| {
        row[key]
            .as_str()
            .expect("contract platform floor is a string")
            .to_owned()
    };
    PlatformFloor {
        kernel_min: field("kernel_min"),
        glibc_min: field("glibc_min"),
        procfs_self_fd_exec: row["capabilities"]["procfs_self_fd_exec"] == true,
    }
}

struct HostPlatform {
    kernel_release: Option<String>,
    glibc_version: Option<String>,
    procfs_self_fd: bool,
}

fn observe_host_platform() -> HostPlatform {
    HostPlatform {
        kernel_release: std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .ok()
            .map(|release| release.trim().to_owned()),
        glibc_version: spawn::glibc_version(),
        // `spawn_detached` execs `/proc/self/fd/3`, which requires a mounted procfs whose `self` links resolve.
        procfs_self_fd: std::fs::read_link("/proc/self/exe").is_ok()
            && std::fs::metadata("/proc/self/fd").is_ok_and(|meta| meta.is_dir()),
    }
}

/// Compares the leading `major.minor` of `observed` against `floor`.
///
/// Each component uses its leading digits, so `6.12.103-127.amzn2023.x86_64` compares as `6.12`.
/// `None` means either side lacks a parseable `major.minor`.
fn version_at_least(observed: &str, floor: &str) -> Option<bool> {
    fn major_minor(version: &str) -> Option<(u64, u64)> {
        let mut parts = version.splitn(3, '.');
        let component = |part: &str| -> Option<u64> {
            let digits: &str = &part[..part
                .bytes()
                .position(|byte| !byte.is_ascii_digit())
                .unwrap_or(part.len())];
            if digits.is_empty() {
                return None;
            }
            digits.parse().ok()
        };
        let major = component(parts.next()?)?;
        let minor = component(parts.next()?)?;
        Some((major, minor))
    }
    Some(major_minor(observed)? >= major_minor(floor)?)
}

fn host_meets_platform_floor(floor: &PlatformFloor, host: &HostPlatform) -> bool {
    let at_least = |observed: &Option<String>, min: &str| {
        observed
            .as_deref()
            .and_then(|version| version_at_least(version, min))
            == Some(true)
    };
    at_least(&host.kernel_release, &floor.kernel_min)
        && at_least(&host.glibc_version, &floor.glibc_min)
        && (!floor.procfs_self_fd_exec || host.procfs_self_fd)
}

/// Returns the build target only when the host meets its runtime floor.
///
/// A host that fails the floor cannot exec the payload launcher, so the gate runs before any
/// stop or staging and reports `unsupported_platform` while the incumbent keeps serving.
fn supported_target() -> Result<&'static str, (&'static str, &'static str)> {
    let unsupported = ("stopped", "unsupported_platform");
    let target = build_target().ok_or(unsupported)?;
    if !host_meets_platform_floor(&platform_floor(target), &observe_host_platform()) {
        return Err(unsupported);
    }
    Ok(target)
}

/// The validation confirms that the generation was staged by this release for this target.
///
/// `validate` does not verify that the generation matches the running binary.
fn generation_identity_matches(
    manifest: &host_runtime::generation::GenerationManifest,
    target: &str,
) -> Result<(), (&'static str, &'static str)> {
    if manifest.target != target {
        return Err(("stopped", "native_payload_invalid"));
    }
    if manifest.release_contract_sha256 != release_contract::release_contract_sha256() {
        return Err(("stopped", "native_payload_invalid"));
    }
    Ok(())
}

/// Validated generation identity and optional open launcher descriptor.
///
/// When present, `launcher` is bound to the generation digest.
struct ResolvedGeneration {
    digest: String,
    launcher: Option<std::os::fd::OwnedFd>,
}

/// Opens the production launcher from a validated generation.
///
/// Unqualified development manifests may use current executable only when the test override permits it.
fn generation_launcher(
    validated: &host_runtime::generation::ValidatedGeneration,
) -> Result<Option<std::os::fd::OwnedFd>, (&'static str, &'static str)> {
    const PRODUCTION_LAUNCHER: &str = "payload/bin/eidnara-host";
    if !validated
        .manifest
        .files
        .iter()
        .any(|file| file.path == PRODUCTION_LAUNCHER)
    {
        return if validated.manifest.source_payload_manifest_sha256.as_deref()
            == Some("unqualified-dev-manifest")
            && spawn::test_self_exec_allowed()
        {
            Ok(None)
        } else {
            Err(("stopped", "native_payload_invalid"))
        };
    }
    validated
        .open_verified_file(PRODUCTION_LAUNCHER)
        .map(Some)
        .map_err(|_| ("stopped", "native_payload_invalid"))
}

/// Resolves and validates a staged generation for the current build target.
///
/// `payload_manifest_digest` requires the resolved generation to have been staged from that digest; a mismatch returns `native_payload_invalid`.
///
/// `running_generation` is the digest the incumbent's lifecycle record names when one is
/// running; staging protects it from the prune because that generation is still executing.
fn resolve_generation(
    payload_dir: Option<&Path>,
    payload_manifest_digest: Option<&str>,
    running_generation: Option<&str>,
) -> Result<ResolvedGeneration, (&'static str, &'static str)> {
    // `unsupported_platform` takes precedence over `native_payload_missing`, so `supported_target()` runs before payload inspection.
    let target = supported_target()?;
    match payload_dir {
        Some(dir) => {
            // `stage_and_promote` checks capacity and hashes every source while copying, so a payload that does not fit or does not match its manifest fails here.
            let payload = payload_sources(dir, payload_manifest_digest)?;
            let store = GenerationStore::open(None).map_err(|e| generation_failure(&e))?;
            let mut protected = BTreeSet::new();
            if let Ok(host_runtime::generation::CurrentProfile::Current(current)) =
                store.read_current()
            {
                protected.insert(current);
            }
            if let Some(digest) = running_generation {
                protected.insert(digest.to_owned());
            }
            // The staging transaction prunes unreferenced complete generations and stale staging directories while holding the transaction lock.
            store
                .prune(&protected)
                .map_err(|e| generation_failure(&e))?;
            let meta = StageMeta {
                target: target.to_owned(),
                release_contract_sha256: release_contract::release_contract_sha256().to_owned(),
                // The `unqualified-dev-manifest` value identifies an unqualified dev/test payload; it is not a placeholder hash.
                inputs_lock_sha256: payload.inputs_lock_sha256,
                source_payload_manifest_sha256: payload_manifest_digest
                    .unwrap_or("unqualified-dev-manifest")
                    .to_owned(),
            };
            let digest = store
                .stage_and_promote(&payload.sources, &meta, &protected)
                .map_err(|e| generation_failure(&e))?;
            let validated = store
                .validate(&digest)
                .map_err(|e| generation_failure(&e))?;
            generation_identity_matches(&validated.manifest, target)?;
            let launcher = generation_launcher(&validated)?;
            Ok(ResolvedGeneration { digest, launcher })
        }
        None => {
            let store = GenerationStore::open_probe(None)
                .map_err(|e| generation_failure(&e))?
                .ok_or(("stopped", "native_payload_missing"))?;
            match store.read_current().map_err(|e| generation_failure(&e))? {
                host_runtime::generation::CurrentProfile::Current(digest) => {
                    let validated = store
                        .validate(&digest)
                        .map_err(|e| generation_failure(&e))?;
                    generation_identity_matches(&validated.manifest, target)?;
                    // When `payload_manifest_digest` is supplied, `resolve_generation` rejects a generation whose `source_payload_manifest_sha256` differs or is absent.
                    if payload_manifest_digest.is_some_and(|expected| {
                        validated.manifest.source_payload_manifest_sha256.as_deref()
                            != Some(expected)
                    }) {
                        return Err(("stopped", "native_payload_invalid"));
                    }
                    let launcher = generation_launcher(&validated)?;
                    Ok(ResolvedGeneration { digest, launcher })
                }
                host_runtime::generation::CurrentProfile::Absent => {
                    Err(("stopped", "native_payload_missing"))
                }
                host_runtime::generation::CurrentProfile::Quarantined => {
                    Err(("stopped", "unsupported_state_schema"))
                }
            }
        }
    }
}

/// Schema identifier of the trusted payload manifest release tooling writes next to a staged payload.
const PAYLOAD_MANIFEST_SCHEMA: &str = "eidnara.payload-manifest/v1";

/// Checks whether the running generation was staged from payload manifest digest `expected`.
///
/// `already_running` with `proof:"current"` vouches for the running native code; a named payload returns `native_payload_invalid` when its manifest differs.
/// The running generation is the lifecycle record's digest, which `serve` writes from its startup envelope; a legacy record without a digest cannot prove the match and fails closed.
fn running_generation_matches(
    observed: &LifecycleProbe,
    expected: &str,
) -> Result<(), &'static str> {
    let invalid = "native_payload_invalid";
    let running = observed
        .record
        .as_ref()
        .map(|record| record.payload_manifest_digest.as_str())
        .filter(|digest| !digest.is_empty())
        .ok_or(invalid)?;
    let store = GenerationStore::open_probe(None)
        .map_err(|error| generation_failure(&error).1)?
        .ok_or(invalid)?;
    let validated = store
        .validate(running)
        .map_err(|error| generation_failure(&error).1)?;
    if validated.manifest.source_payload_manifest_sha256.as_deref() != Some(expected) {
        return Err(invalid);
    }
    Ok(())
}

struct PayloadSources {
    sources: Vec<SourceSpec>,
    inputs_lock_sha256: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TrustedPayloadManifest {
    schema: String,
    release: TrustedReleaseIdentity,
    release_contract_sha256: String,
    production_inputs_lock_sha256: String,
    mode: String,
    package: TrustedPackageIdentity,
    platform_floor: serde_json::Value,
    synapse: String,
    launcher: String,
    files: Vec<TrustedPayloadFile>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TrustedReleaseIdentity {
    id: String,
    version: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TrustedPackageIdentity {
    name: String,
    version: String,
    target: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TrustedPayloadFile {
    path: String,
    #[serde(rename = "type")]
    file_type: String,
    size: u64,
    mode: String,
    sha256: String,
}

fn payload_sources(
    dir: &Path,
    expected_manifest_digest: Option<&str>,
) -> Result<PayloadSources, (&'static str, &'static str)> {
    if let Some(expected) = expected_manifest_digest {
        return trusted_payload_sources(dir, expected);
    }
    // A release build stages only manifest-bound payloads: an unqualified tree can carry any `payload/bin/eidnara-host`, and the launcher-present branch of `generation_launcher` would exec it without a trusted identity.
    if !cfg!(debug_assertions) {
        eprintln!(
            "eidnara-host: --payload-dir requires --payload-manifest-digest in a release build"
        );
        return Err(("stopped", "native_payload_invalid"));
    }
    Ok(PayloadSources {
        sources: unqualified_payload_sources(dir)?,
        inputs_lock_sha256: "unqualified-dev-inputs".to_owned(),
    })
}

fn trusted_payload_sources(
    dir: &Path,
    expected_manifest_digest: &str,
) -> Result<PayloadSources, (&'static str, &'static str)> {
    use sha2::Digest;

    let invalid = ("stopped", "native_payload_invalid");
    let manifest_path = dir.join("payload-manifest.json");
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&manifest_path)
        .map_err(|_| invalid)?;
    const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
    let meta = file.metadata().map_err(|_| invalid)?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_MANIFEST_BYTES {
        return Err(invalid);
    }
    // The size check read the metadata, not the bytes; a manifest appended in place after it is bounded here too.
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    (&mut file)
        .take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid)?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(invalid);
    }
    let canonical = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
    // The digest binds the manifest's exact bytes, so serialization must match the producer byte-for-byte.
    if format!("{:x}", sha2::Sha256::digest(canonical)) != expected_manifest_digest {
        return Err(invalid);
    }
    let manifest: TrustedPayloadManifest =
        serde_json::from_slice(canonical).map_err(|_| invalid)?;
    let Some(target) = build_target() else {
        return Err(invalid);
    };
    let expected_package = match target {
        "linux-x64-gnu" => "@eidnara/host-linux-x64-gnu",
        _ => return Err(invalid),
    };
    // The enforced floor is the contract row `supported_target` reads; the manifest's copies are release-tooling output already bound by `release_contract_sha256` and carry no separate authority.
    let _ = (&manifest.platform_floor, &manifest.synapse);
    if manifest.schema != PAYLOAD_MANIFEST_SCHEMA
        || manifest.release.id != "eidnara-host-release"
        || manifest.release.version != release_contract::RELEASE_VERSION
        || manifest.release_contract_sha256 != release_contract::release_contract_sha256()
        || manifest.mode != "production"
        || manifest.package.name != expected_package
        || manifest.package.version != release_contract::RELEASE_VERSION
        || manifest.package.target != target
        || manifest.launcher != "payload/bin/eidnara-host"
        // `production_inputs_lock_sha256` must equal the lock compiled into the executable; hex validation alone would permit unrelated production inputs.
        || manifest.production_inputs_lock_sha256
            != daemon::production_inputs::production_inputs_lock_sha256()
    {
        return Err(invalid);
    }
    let mut sources = Vec::with_capacity(manifest.files.len());
    let mut previous: Option<&str> = None;
    let mut launcher_seen = false;
    for entry in &manifest.files {
        if entry.file_type != "file"
            || entry.size == 0
            || entry.sha256.len() != 64
            || !entry
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || !entry.path.starts_with("payload/")
            || entry
                .path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
            || previous.is_some_and(|value| value >= entry.path.as_str())
        {
            return Err(invalid);
        }
        previous = Some(&entry.path);
        let executable = match entry.mode.as_str() {
            "755" => true,
            "644" => false,
            _ => return Err(invalid),
        };
        if entry.path == manifest.launcher {
            launcher_seen = true;
            if !executable {
                return Err(invalid);
            }
        }
        sources.push(SourceSpec {
            rel_path: entry.path.clone(),
            source: dir.join(&entry.path),
            executable,
            expected_size: Some(entry.size),
            expected_sha256: Some(entry.sha256.clone()),
        });
    }
    if !launcher_seen || sources.is_empty() {
        return Err(invalid);
    }
    Ok(PayloadSources {
        sources,
        inputs_lock_sha256: manifest.production_inputs_lock_sha256,
    })
}

fn unqualified_payload_sources(
    dir: &Path,
) -> Result<Vec<SourceSpec>, (&'static str, &'static str)> {
    use std::os::unix::fs::PermissionsExt;
    fn walk(
        base: &Path,
        rel: &str,
        out: &mut Vec<SourceSpec>,
    ) -> Result<(), (&'static str, &'static str)> {
        let invalid = ("stopped", "native_payload_invalid");
        let dir_path = if rel.is_empty() {
            base.to_path_buf()
        } else {
            base.join(rel)
        };
        let entries = std::fs::read_dir(&dir_path).map_err(|_| invalid)?;
        for entry in entries {
            let entry = entry.map_err(|_| invalid)?;
            let name = entry.file_name().into_string().map_err(|_| invalid)?;
            let child_rel = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            let meta = entry.file_type().map_err(|_| invalid)?;
            if meta.is_symlink() {
                return Err(invalid);
            }
            if meta.is_dir() {
                walk(base, &child_rel, out)?;
            } else if meta.is_file() {
                let metadata = entry.metadata().map_err(|_| invalid)?;
                out.push(SourceSpec {
                    rel_path: child_rel,
                    source: base.join(rel).join(entry.file_name()),
                    executable: metadata.permissions().mode() & 0o111 != 0,
                    expected_size: None,
                    expected_sha256: None,
                });
            } else {
                return Err(invalid);
            }
        }
        Ok(())
    }
    let mut sources = Vec::new();
    walk(dir, "", &mut sources)?;
    if sources.is_empty() {
        return Err(("stopped", "native_payload_invalid"));
    }
    sources.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(sources)
}

fn prepare_launcher_envelope(
    envelope: serve::LauncherEnvelope,
    mode: serve::SelectionMode<'_>,
) -> Result<serve::PreparedLauncherEnvelope, &'static str> {
    let data_dir = host_runtime::data_dir_path(None)
        .ok()
        .ok_or("lifecycle data directory is unavailable")?;
    envelope.prepare(data_dir, mode)
}

fn cmd_start(
    payload_dir: Option<&Path>,
    payload_manifest_digest: Option<&str>,
    launcher_envelope: serve::LauncherEnvelope,
) -> DaemonResult {
    let command = "start";
    let outer = Instant::now() + OUTER_AGGREGATE;
    let runtime = match Runtime::new() {
        Ok(runtime) => runtime,
        Err(_) => return DaemonResult::new(command, false, "stopped", "internal_error"),
    };

    let _tx = match LifecycleTransactionLock::acquire_exclusive(None) {
        Ok(tx) => tx,
        Err(error) => {
            let (state, reason) = instance_failure(&error);
            return DaemonResult::new(command, false, state, reason);
        }
    };
    let anchor = match NamespaceAnchor::capture(None) {
        Ok(anchor) => anchor,
        Err(error) => {
            let (state, reason) = instance_failure(&error);
            return DaemonResult::new(command, false, state, reason);
        }
    };
    let observed = match settle_probe(phase_deadline(outer, TRANSITION_SETTLE)) {
        Ok(observed) => observed,
        Err(error) => {
            let (state, reason) = instance_failure(&error);
            return DaemonResult::new(command, false, state, reason);
        }
    };
    // Quarantined records must block startup in every state, so classify them before dispatching on `wedged`.
    if let Some((state, reason)) = quarantined_observation(&observed) {
        return DaemonResult::new(command, false, state, reason);
    }
    // A stopped observation starts the requested generation; a running one is authenticated and, when its credential is gone, re-probed so an incumbent that exited between the settle probe and authentication is replaced instead of reported.
    let start_from_stopped = |launcher_envelope: serve::LauncherEnvelope| {
        let prepared =
            match prepare_launcher_envelope(launcher_envelope, serve::SelectionMode::Fresh) {
                Ok(prepared) => prepared,
                Err(serve::UNSUPPORTED_SELECTION_SCHEMA) => {
                    return DaemonResult::new(command, false, "wedged", "unsupported_state_schema");
                }
                Err(_) => {
                    return DaemonResult::new(command, false, "stopped", "harness_unavailable");
                }
            };
        let outcome = start_phase(
            &runtime,
            SuccessorGeneration::Resolve {
                payload_dir,
                payload_manifest_digest,
            },
            &anchor,
            outer,
            prepared,
            false,
        );
        start_outcome_result(command, outcome, None)
    };
    match observed.state {
        LifecycleState::Running => {
            let publication = match publication_path() {
                Ok(path) => path,
                Err(_) => return DaemonResult::new(command, false, "running", "internal_error"),
            };
            let auth_deadline = phase_deadline(outer, phase_cap(SPAWN_PUBLICATION_AUTH));
            let Some(daemon_ver) = runtime.authenticate(&publication, auth_deadline) else {
                return match probe() {
                    Ok(fresh) if fresh.state == LifecycleState::Stopped => {
                        start_from_stopped(launcher_envelope)
                    }
                    _ => DaemonResult::new(command, false, "running", "authentication_failed"),
                };
            };
            if !daemon_version_compatible(&daemon_ver) {
                let mut result =
                    DaemonResult::new(command, false, "running", "incompatible_daemon");
                result.versions.daemon = Some(daemon_ver);
                return result;
            }
            if let Some(expected) = payload_manifest_digest
                && let Err(reason) = running_generation_matches(&observed, expected)
            {
                let mut result = DaemonResult::new(command, false, "running", reason);
                result.versions.daemon = Some(daemon_ver);
                result.checks.push(Check {
                    id: "artifact.current_generation",
                    status: "fail",
                    reason,
                    remediation: remediation_for(reason),
                });
                return result;
            }
            let credential_identity_key = match serve::credential_identity_key(&publication) {
                Ok(key) => key,
                Err(_) => {
                    return DaemonResult::new(command, false, "running", "authentication_failed");
                }
            };
            let prepared = match prepare_launcher_envelope(
                launcher_envelope,
                serve::SelectionMode::Running {
                    credential_identity_key: &credential_identity_key,
                    require_previous_credentials: false,
                },
            ) {
                Ok(prepared) => prepared,
                Err(serve::UNSUPPORTED_SELECTION_SCHEMA) => {
                    return DaemonResult::new(command, false, "wedged", "unsupported_state_schema");
                }
                Err(_) => {
                    return DaemonResult::new(command, false, "running", "harness_unavailable");
                }
            };
            if prepared.changed {
                return DaemonResult::new(command, false, "running", "harness_unavailable");
            }
            // The named publication path must still resolve to the namespace this command observed; a replaced managed subtree would make `already_running` name a daemon clients cannot reach.
            if anchor.verify().is_err() {
                return DaemonResult::new(command, false, "wedged", "wedged");
            }
            let mut result = DaemonResult::new(command, true, "running", "already_running");
            result.versions.daemon = Some(daemon_ver);
            result.versions.proof = Some("current");
            result
        }
        LifecycleState::Starting | LifecycleState::Stopping => DaemonResult::new(
            command,
            false,
            probe_state(observed.state),
            "lifecycle_busy",
        ),
        LifecycleState::Wedged => DaemonResult::new(command, false, "wedged", "wedged"),
        LifecycleState::Stopped => start_from_stopped(launcher_envelope),
    }
}

fn start_outcome_result(
    command: &'static str,
    outcome: StartOutcome,
    effects: Option<Effects>,
) -> DaemonResult {
    let mut result = DaemonResult::new(command, outcome.ok, outcome.state, outcome.reason);
    result.versions.daemon = outcome.daemon_ver;
    if outcome.ok {
        result.versions.proof = Some("current");
    }
    if let Some((status, reason)) = outcome.generation_check {
        result.checks.push(Check {
            id: "artifact.current_generation",
            status,
            reason,
            remediation: remediation_for(reason),
        });
    }
    if let Some(effects) = effects {
        result.effects = Some(effects);
    }
    result
}

// -------------------------------------------------------------------------
// stop
// -------------------------------------------------------------------------

/// `stop` and `restart` share this committed-stop phase after the caller acquires the transaction lock and observes `Running`.
/// The function returns `(stop_committed, terminal)`, where `terminal` is `Ok(())` after teardown or a `(state, reason)` failure.
///
/// After a shutdown timeout, the function probes because the host may have committed at full-frame write completion.
fn stop_phase(
    runtime: &Runtime,
    outer: Instant,
) -> (bool, Result<(), (&'static str, &'static str)>) {
    let publication = match publication_path() {
        Ok(path) => path,
        Err(_) => return (false, Err(("running", "internal_error"))),
    };
    let mut commit_uncertain = false;
    match runtime.shutdown(&publication, outer) {
        Ok(()) => {}
        // The incumbent can exit between the caller's authentication and this connection; a stopped observation is a completed stop, not a credential failure.
        Err(SHUTDOWN_AUTHENTICATION_FAILED) => {
            return match probe() {
                Ok(observed) if observed.state == LifecycleState::Stopped => (true, Ok(())),
                _ => (false, Err(("running", "authentication_failed"))),
            };
        }
        // After an in-flight frame times out, probe determines whether the host committed it.
        Err(SHUTDOWN_OUTCOME_UNKNOWN) => commit_uncertain = true,
        // `SHUTDOWN_FAILED`: the request was never sent, because the host rejected it, the budget expired, or the connection closed under it. A daemon that exited after the connect is a completed stop; otherwise it keeps serving.
        Err(_) => {
            return match probe() {
                Ok(observed) if observed.state == LifecycleState::Stopped => (true, Ok(())),
                _ => (false, Err(("running", "lifecycle_busy"))),
            };
        }
    }
    // After acknowledgement or an unresolved in-flight request, probe determines whether the host committed it.
    // The request is already sent, so the teardown window is the full cap regardless of how much of `outer` the caller's earlier phases used: cutting it short would report `shutdown_timeout` for a stop that then completes, and a `restart` would skip its staged successor.
    let deadline = Instant::now() + phase_cap(STOP_TEARDOWN);
    loop {
        match probe() {
            // If the request was never acknowledged, teardown observation determines whether it committed.
            Ok(observed) if observed.state == LifecycleState::Stopped => {
                return (true, Ok(()));
            }
            Ok(_) => {}
            Err(_) => {}
        }
        if Instant::now() >= deadline {
            // An unacknowledged request can still commit when the host writes its queued response, so one `Running` observation cannot clear it; the stop stays reported as committed and the observed state is carried so callers do not assume the daemon kept serving.
            let state = match probe().map(|observed| observed.state) {
                Ok(LifecycleState::Running) if commit_uncertain => "running",
                _ => "stopping",
            };
            return (true, Err((state, "shutdown_timeout")));
        }
        std::thread::sleep(REPROBE_INTERVAL);
    }
}

fn cmd_stop() -> DaemonResult {
    let command = "stop";
    let outer = Instant::now() + OUTER_AGGREGATE;
    let runtime = match Runtime::new() {
        Ok(runtime) => runtime,
        Err(_) => return DaemonResult::new(command, false, "stopped", "internal_error"),
    };
    let _tx = match LifecycleTransactionLock::acquire_exclusive(None) {
        Ok(tx) => tx,
        Err(error) => {
            let (state, reason) = instance_failure(&error);
            return DaemonResult::new(command, false, state, reason);
        }
    };
    let observed = match settle_probe(phase_deadline(outer, TRANSITION_SETTLE)) {
        Ok(observed) => observed,
        Err(error) => {
            let (state, reason) = instance_failure(&error);
            return DaemonResult::new(command, false, state, reason);
        }
    };
    // The function classifies quarantined records before state dispatch to avoid reporting `already_stopped`.
    // Quarantined records require `align_versions` and block the next start.
    if let Some((state, reason)) = quarantined_observation(&observed) {
        return DaemonResult::new(command, false, state, reason);
    }
    match observed.state {
        // The transaction lock confines selector cleanup to stale-state removal.
        LifecycleState::Stopped => match serve::clear_active_selection() {
            Err(serve::UNSUPPORTED_SELECTION_SCHEMA) => {
                DaemonResult::new(command, false, "wedged", "unsupported_state_schema")
            }
            Ok(()) | Err(_) => DaemonResult::new(command, true, "stopped", "already_stopped"),
        },
        LifecycleState::Wedged => {
            let reason = if observed.reason == "unsupported_state_schema" {
                "unsupported_state_schema"
            } else {
                "wedged"
            };
            DaemonResult::new(command, false, "wedged", reason)
        }
        LifecycleState::Starting | LifecycleState::Stopping => DaemonResult::new(
            command,
            false,
            probe_state(observed.state),
            "lifecycle_busy",
        ),
        LifecycleState::Running => match stop_phase(&runtime, outer) {
            (_, Ok(())) => match serve::clear_active_selection() {
                Err(serve::UNSUPPORTED_SELECTION_SCHEMA) => {
                    DaemonResult::new(command, false, "wedged", "unsupported_state_schema")
                }
                Ok(()) | Err(_) => DaemonResult::new(command, true, "stopped", "stopped"),
            },
            (_, Err((state, reason))) => DaemonResult::new(command, false, state, reason),
        },
    }
}

// -------------------------------------------------------------------------
// restart
// -------------------------------------------------------------------------

fn cmd_restart(
    payload_dir: Option<&Path>,
    payload_manifest_digest: Option<&str>,
    launcher_envelope: serve::LauncherEnvelope,
) -> DaemonResult {
    let command = "restart";
    let effects = |stop: bool, start: bool| Effects {
        stop_committed: stop,
        start_committed: start,
    };
    let outer = Instant::now() + OUTER_AGGREGATE;
    let runtime = match Runtime::new() {
        Ok(runtime) => runtime,
        Err(_) => {
            return DaemonResult::new(command, false, "stopped", "internal_error")
                .with_effects(effects(false, false));
        }
    };
    // A single transaction lock spans stop, teardown, promotion, and successor start; every wait below is bounded.
    let _tx = match LifecycleTransactionLock::acquire_exclusive(None) {
        Ok(tx) => tx,
        Err(error) => {
            let (state, reason) = instance_failure(&error);
            return DaemonResult::new(command, false, state, reason)
                .with_effects(effects(false, false));
        }
    };
    let anchor = match NamespaceAnchor::capture(None) {
        Ok(anchor) => anchor,
        Err(error) => {
            let (state, reason) = instance_failure(&error);
            return DaemonResult::new(command, false, state, reason)
                .with_effects(effects(false, false));
        }
    };
    let mut observed = match settle_probe(phase_deadline(outer, TRANSITION_SETTLE)) {
        Ok(observed) => observed,
        Err(error) => {
            let (state, reason) = instance_failure(&error);
            return DaemonResult::new(command, false, state, reason)
                .with_effects(effects(false, false));
        }
    };
    // A quarantined record prevents successor startup before any stop.
    if let Some((state, reason)) = quarantined_observation(&observed) {
        return DaemonResult::new(command, false, state, reason)
            .with_effects(effects(false, false));
    }
    match observed.state {
        LifecycleState::Wedged => {
            return DaemonResult::new(command, false, "wedged", "wedged")
                .with_effects(effects(false, false));
        }
        LifecycleState::Starting | LifecycleState::Stopping => {
            return DaemonResult::new(
                command,
                false,
                probe_state(observed.state),
                "lifecycle_busy",
            )
            .with_effects(effects(false, false));
        }
        LifecycleState::Stopped | LifecycleState::Running => {}
    }
    // The successor generation is staged, promoted, validated, and its launcher opened before the irreversible stop, so a payload that fails or is replaced under the command leaves the incumbent serving; the retained launcher descriptor is immutable from here on.
    let running_generation = observed
        .record
        .as_ref()
        .filter(|_| observed.state == LifecycleState::Running)
        .map(|record| record.payload_manifest_digest.as_str())
        .filter(|digest| !digest.is_empty());
    let resolved =
        match resolve_generation(payload_dir, payload_manifest_digest, running_generation) {
            Ok(resolved) => resolved,
            Err((_, reason)) => {
                // On a resolution failure, the function reports the observed state because the incumbent did not change.
                return DaemonResult::new(command, false, probe_state(observed.state), reason)
                    .with_effects(effects(false, false));
            }
        };
    let credential_identity_key = if observed.state == LifecycleState::Running {
        let publication = match publication_path() {
            Ok(path) => path,
            Err(_) => {
                return DaemonResult::new(command, false, "running", "internal_error")
                    .with_effects(effects(false, false));
            }
        };
        let auth_deadline = phase_deadline(outer, phase_cap(SPAWN_PUBLICATION_AUTH));
        match runtime.authenticate(&publication, auth_deadline) {
            Some(_) => match serve::credential_identity_key(&publication) {
                Ok(key) => Some(key),
                Err(_) => {
                    return DaemonResult::new(command, false, "running", "authentication_failed")
                        .with_effects(effects(false, false));
                }
            },
            // The incumbent can exit between the settle probe and this attempt; a stopped observation continues on the stopped path so the command restores service instead of reporting a daemon that is gone.
            None => match probe() {
                Ok(fresh) if fresh.state == LifecycleState::Stopped => {
                    observed = fresh;
                    None
                }
                _ => {
                    return DaemonResult::new(command, false, "running", "authentication_failed")
                        .with_effects(effects(false, false));
                }
            },
        }
    } else {
        None
    };
    let prepared = match prepare_launcher_envelope(
        launcher_envelope,
        match credential_identity_key.as_ref() {
            Some(key) => serve::SelectionMode::Running {
                credential_identity_key: key,
                require_previous_credentials: true,
            },
            None => serve::SelectionMode::Fresh,
        },
    ) {
        Ok(prepared) => prepared,
        Err(serve::UNSUPPORTED_SELECTION_SCHEMA) => {
            return DaemonResult::new(command, false, "wedged", "unsupported_state_schema")
                .with_effects(effects(false, false));
        }
        Err(_) => {
            return DaemonResult::new(
                command,
                false,
                probe_state(observed.state),
                "harness_unavailable",
            )
            .with_effects(effects(false, false));
        }
    };
    let stop_committed = match observed.state {
        LifecycleState::Running => {
            // The function reserves `start_phase` time before stopping so teardown cannot exhaust the successor-start budget.
            //
            // If reservation fails, return `lifecycle_busy` without effects so callers can retry.
            let stop_deadline = match outer.checked_sub(phase_cap(SPAWN_PUBLICATION_AUTH)) {
                Some(deadline) if deadline > Instant::now() => deadline,
                _ => {
                    return DaemonResult::new(command, false, "running", "lifecycle_busy")
                        .with_effects(effects(false, false));
                }
            };
            // Staging and harness preparation ran since the anchor was captured; a replaced managed subtree would make the shutdown request reach whatever now sits at the publication path, not the incumbent observed above.
            if anchor.verify().is_err() {
                return DaemonResult::new(command, false, "wedged", "wedged")
                    .with_effects(effects(false, false));
            }
            match stop_phase(&runtime, stop_deadline) {
                (_, Ok(())) => true,
                // A pre-acknowledgement failure does not attempt a start and leaves both effects false.
                (false, Err((state, reason))) => {
                    return DaemonResult::new(command, false, state, reason)
                        .with_effects(effects(false, false));
                }
                // If acknowledged teardown misses its deadline, the stop remains committed and no start is attempted.
                (true, Err((state, reason))) => {
                    return DaemonResult::new(command, false, state, reason)
                        .with_effects(effects(true, false));
                }
            }
        }
        _ => false,
    };
    let outcome = start_phase(
        &runtime,
        SuccessorGeneration::Preflighted(resolved),
        &anchor,
        outer,
        prepared,
        stop_committed,
    );
    let start_committed = outcome.start_committed;
    start_outcome_result(
        command,
        outcome,
        Some(effects(stop_committed, start_committed)),
    )
}

// -------------------------------------------------------------------------
// main
// -------------------------------------------------------------------------

/// Remediation for a `harness_unavailable` subreason, mirroring `harness_unavailable.reasons_by_precedence` in `RELEASE_CONTRACT_JSON`.
fn harness_remediation(subreason: &str) -> Option<&'static str> {
    match subreason {
        "descriptor_absent"
        | "descriptor_invalid"
        | "closure_incomplete"
        | "argument_variant_invalid"
        | "credential_missing"
        | "credential_value_too_large"
        | "credential_snapshot_mismatch" => Some("restart_with_supported_harness"),
        _ => None,
    }
}

/// A harness or credential the launcher described incorrectly is `harness_unavailable`, the contract's reason for a supplied harness that cannot serve, with the remediation its subreason carries; a read the command could not complete is `internal_error`.
fn envelope_failure_result(
    command: &'static str,
    error: &serve::LauncherEnvelopeError,
) -> DaemonResult {
    match error {
        serve::LauncherEnvelopeError::Invalid { subreason, .. } => {
            let mut result =
                DaemonResult::new(command, false, unchanged_state(), "harness_unavailable");
            result.remediation = harness_remediation(subreason);
            result
        }
        serve::LauncherEnvelopeError::Unreadable(_) => {
            DaemonResult::new(command, false, unchanged_state(), "internal_error")
        }
    }
}

fn emit(result: DaemonResult) -> i32 {
    let result = result.finish();
    match serde_json::to_string(&result) {
        Ok(json) => {
            println!("{json}");
            if result.ok { 0 } else { 1 }
        }
        Err(_) => {
            eprintln!("eidnara-host: result serialization failed");
            1
        }
    }
}

fn real_main() -> i32 {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let command = match parse_args(&args) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("eidnara-host: {message}");
            eprintln!("{USAGE}");
            return 2;
        }
    };
    match command {
        Command::Version => {
            println!(
                "eidnara-host {} ({})",
                release_contract::RELEASE_VERSION,
                release_contract::DAEMON_VERSION
            );
            0
        }
        Command::ReleaseInfo => {
            println!("{}", release_contract::RELEASE_CONTRACT_JSON);
            0
        }
        Command::InputLockDigest => {
            println!(
                "{}",
                daemon::production_inputs::production_inputs_lock_sha256()
            );
            0
        }
        Command::Status => emit(cmd_probe()),
        Command::Start {
            payload_dir,
            payload_manifest_digest,
        } => {
            spawn::ignore_sigpipe();
            match serve::read_launcher_envelope() {
                Ok(envelope) => emit(cmd_start(
                    payload_dir.as_deref(),
                    payload_manifest_digest.as_deref(),
                    envelope,
                )),
                Err(error) => {
                    // The result reason vocabulary is closed, so the cause goes to stderr.
                    eprintln!("eidnara-host: {}", error.message());
                    emit(envelope_failure_result("start", &error))
                }
            }
        }
        Command::Stop => emit(cmd_stop()),
        Command::Restart {
            payload_dir,
            payload_manifest_digest,
        } => {
            spawn::ignore_sigpipe();
            match serve::read_launcher_envelope() {
                Ok(envelope) => emit(cmd_restart(
                    payload_dir.as_deref(),
                    payload_manifest_digest.as_deref(),
                    envelope,
                )),
                Err(error) => {
                    eprintln!("eidnara-host: {}", error.message());
                    emit(
                        envelope_failure_result("restart", &error).with_effects(Effects {
                            stop_committed: false,
                            start_committed: false,
                        }),
                    )
                }
            }
        }
        Command::Serve => match serve::run() {
            Ok(()) => 0,
            Err(message) => {
                eprintln!("eidnara-host serve: {message}");
                1
            }
        },
    }
}

fn main() {
    std::process::exit(real_main());
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;

    #[test]
    fn phase_cap_override_only_widens() {
        let default = Duration::from_secs(10);
        assert_eq!(phase_cap_override(default, None), default);
        assert_eq!(phase_cap_override(default, Some("1")), default);
        assert_eq!(phase_cap_override(default, Some("9999")), default);
        assert_eq!(
            phase_cap_override(default, Some("30000")),
            Duration::from_secs(30)
        );
        assert_eq!(
            phase_cap_override(default, Some("999999999")),
            OUTER_AGGREGATE
        );
        assert_eq!(phase_cap_override(default, Some("0")), default);
        assert_eq!(phase_cap_override(default, Some("-5")), default);
        assert_eq!(phase_cap_override(default, Some("fast")), default);
    }

    #[test]
    fn publication_deadline_keeps_the_full_cap_after_a_committed_stop() {
        let cap = Duration::from_secs(10);
        let expired_outer = Instant::now() - Duration::from_secs(1);

        let before = Instant::now();
        let after_stop = publication_deadline(expired_outer, cap, true);
        assert!(
            after_stop >= before + cap - Duration::from_millis(50),
            "a committed stop must keep the full publication window even when `outer` has passed"
        );

        let fresh = publication_deadline(expired_outer, cap, false);
        assert!(
            fresh <= Instant::now(),
            "without a committed stop the aggregate deadline still bounds the wait"
        );

        let live_outer = Instant::now() + Duration::from_secs(60);
        let bounded = publication_deadline(live_outer, cap, false);
        assert!(
            bounded <= live_outer && bounded >= Instant::now() + cap - Duration::from_millis(50)
        );
    }

    #[test]
    fn remediation_mapping_matches_release_contract() {
        let contract: serde_json::Value =
            serde_json::from_str(release_contract::RELEASE_CONTRACT_JSON).expect("contract");
        let reasons = &contract["cli"]["reasons"];
        // Reasons whose remediation comes from a subreason carry `remediation: null` in the contract and are not mapped here.
        let failing = reasons["failing_by_precedence"]
            .as_array()
            .expect("failing reasons")
            .iter()
            .filter(|entry| entry["remediation_from_subreason"] != serde_json::Value::Bool(true))
            .map(|entry| {
                (
                    entry["id"].as_str().expect("reason id"),
                    entry["remediation"].as_str(),
                )
            });
        let warn_remediations = reasons["warn_remediations"]
            .as_object()
            .expect("warn remediations");
        let non_failing = reasons["non_failing"]
            .as_array()
            .expect("non-failing reasons")
            .iter()
            .map(|id| {
                let id = id.as_str().expect("reason");
                (
                    id,
                    warn_remediations
                        .get(id)
                        .and_then(serde_json::Value::as_str),
                )
            });
        let mut contract_remediated = BTreeSet::new();
        for (id, expected) in failing.chain(non_failing) {
            let reason: &'static str = Box::leak(id.to_owned().into_boxed_str());
            assert_eq!(
                remediation_for(reason),
                expected,
                "remediation mismatch for {id}"
            );
            if expected.is_some() {
                contract_remediated.insert(id.to_owned());
            }
        }
        // Every `remediation_for` arm names a contract reason; the arms are listed here because the match cannot be enumerated at runtime.
        let arms = [
            "internal_error",
            "no_data_dir",
            "unsupported_filesystem",
            "unsupported_platform",
            "unsupported_install_layout",
            "unsupported_state_schema",
            "native_payload_invalid",
            "native_payload_missing",
            "insufficient_storage",
            "native_probe_unavailable",
            "wedged",
            "publication_invalid",
            "publication_stale",
            "publication_missing",
            "authentication_failed",
            "shutdown_timeout",
            "startup_timeout",
            "unsupported_proof_version",
            "incompatible_control",
            "incompatible_daemon",
            "incompatible_module",
            "incompatible_epochs",
            "lifecycle_busy",
            "storage_starting",
            "kernel_starting",
            "synapse_starting",
            "stopping",
            "starting",
            "storage_unavailable",
            "kernel_unavailable",
            "synapse_degraded",
            "not_running",
            "kernel_capacity_warn",
            "kernel_lagging",
        ];
        let mapped: BTreeSet<String> = arms
            .iter()
            .filter(|id| remediation_for(id).is_some())
            .map(|id| (*id).to_owned())
            .collect();
        assert_eq!(
            mapped.len(),
            arms.len(),
            "every listed arm has a remediation"
        );
        assert_eq!(
            mapped, contract_remediated,
            "the set of remediated reasons must match the contract exactly"
        );
        assert_eq!(remediation_for("harness_unavailable"), None);
    }

    #[test]
    fn harness_remediation_matches_release_contract_subreasons() {
        let contract: serde_json::Value =
            serde_json::from_str(release_contract::RELEASE_CONTRACT_JSON).expect("contract");
        let entries = contract["harness_unavailable"]["reasons_by_precedence"]
            .as_array()
            .expect("harness_unavailable subreasons");
        assert!(!entries.is_empty());
        for entry in entries {
            let id = entry["id"].as_str().expect("subreason id");
            assert_eq!(
                harness_remediation(id),
                entry["remediation"].as_str(),
                "remediation mismatch for {id}"
            );
        }
        assert_eq!(harness_remediation("not_a_subreason"), None);
    }

    #[test]
    fn local_module_versions_match_release_contract() {
        let contract: serde_json::Value =
            serde_json::from_str(release_contract::RELEASE_CONTRACT_JSON).expect("contract");
        let modules = &contract["versions"]["modules"];
        let versions = Versions::local();
        assert_eq!(
            versions.context.as_deref(),
            modules["context"]["version"].as_str()
        );
        assert_eq!(
            modules["context"]["version"].as_str(),
            Some(release_contract::RELEASE_VERSION),
            "the context module version is this crate's release version"
        );
        assert_eq!(
            versions.synapse.as_deref(),
            modules["synapse"]["version"].as_str()
        );
        assert_eq!(
            versions.broca.as_deref(),
            modules["broca"]["version"].as_str()
        );
        assert_eq!(versions.release, Some(release_contract::RELEASE_VERSION));
    }

    #[test]
    fn parse_rejects_unknown_and_duplicate_input() {
        let os = |values: &[&str]| -> Vec<std::ffi::OsString> {
            values.iter().map(std::ffi::OsString::from).collect()
        };
        assert!(parse_args(&os(&["bogus"])).is_err());
        assert!(parse_args(&os(&["start", "extra"])).is_err());
        assert!(parse_args(&os(&["stop", "--payload-dir", "x"])).is_err());
        assert!(parse_args(&os(&["start", "--payload-dir"])).is_err());
        assert!(
            parse_args(&os(&[
                "start",
                "--payload-dir",
                "--payload-manifest-digest",
                &"a".repeat(64),
            ]))
            .is_err()
        );
        assert!(
            parse_args(&os(&[
                "start",
                "--payload-manifest-digest",
                "--payload-dir",
                "a"
            ]))
            .is_err()
        );
        assert!(parse_args(&os(&["start", "--payload-dir", "a", "--payload-dir", "b"])).is_err());
        assert!(parse_args(&os(&[])).is_err());
        assert!(matches!(
            parse_args(&os(&["start", "--payload-dir", "a"])),
            Ok(Command::Start {
                payload_dir: Some(_),
                payload_manifest_digest: None,
            })
        ));
        assert!(matches!(
            parse_args(&os(&[
                "start",
                "--payload-dir",
                "a",
                "--payload-manifest-digest",
                &"a".repeat(64),
            ])),
            Ok(Command::Start {
                payload_manifest_digest: Some(_),
                ..
            })
        ));
        assert!(matches!(
            parse_args(&os(&[
                "start",
                "--payload-manifest-digest",
                &"a".repeat(64),
            ])),
            Ok(Command::Start {
                payload_dir: None,
                payload_manifest_digest: Some(_),
            })
        ));
        assert!(matches!(parse_args(&os(&["status"])), Ok(Command::Status)));
        assert!(matches!(parse_args(&os(&["probe"])), Ok(Command::Status)));
        assert!(matches!(
            parse_args(&os(&["--version"])),
            Ok(Command::Version)
        ));
    }

    #[test]
    fn launcher_envelope_accepts_only_bounded_descriptors_and_credentials() {
        let mut envelope = serve::LauncherEnvelope::empty();
        envelope.opencode = Some(serve::HarnessCandidate {
            manifest_sha256: "ab".repeat(32),
            source_roots: std::collections::BTreeMap::from([(
                "opencode-install".to_owned(),
                PathBuf::from("/opt/opencode"),
            )]),
        });
        envelope
            .credentials
            .insert("ANTHROPIC_API_KEY".to_owned(), "secret".to_owned());
        assert_eq!(envelope.validate(), Ok(()));

        envelope
            .credentials
            .insert("AWS_ACCESS_KEY_ID".to_owned(), "ambient".to_owned());
        assert_eq!(
            envelope.validate(),
            Err("credential source contains an unsupported variable")
        );
        envelope.credentials.remove("AWS_ACCESS_KEY_ID");
        envelope
            .credentials
            .insert("ANTHROPIC_API_KEY".to_owned(), "x".repeat(16 * 1024 + 1));
        assert_eq!(
            envelope.validate(),
            Err("credential value exceeds its size cap")
        );
    }

    #[test]
    fn trusted_payload_manifest_binds_every_staged_file() {
        let payload = tempfile::tempdir().expect("payload");
        let store_root = tempfile::tempdir().expect("store");
        let launcher_path = payload.path().join("payload/bin/eidnara-host");
        let model_path = payload.path().join("payload/model/model.onnx");
        std::fs::create_dir_all(launcher_path.parent().expect("launcher parent")).expect("mkdir");
        std::fs::create_dir_all(model_path.parent().expect("model parent")).expect("mkdir");
        std::fs::write(&launcher_path, b"launcher").expect("launcher");
        std::fs::write(&model_path, b"model-v1").expect("model");
        let hash = |bytes: &[u8]| format!("{:x}", sha2::Sha256::digest(bytes));
        let Some(target) = build_target() else {
            return;
        };
        let package_name = match target {
            "linux-x64-gnu" => "@eidnara/host-linux-x64-gnu",
            _ => return,
        };
        let manifest = serde_json::json!({
            "schema": PAYLOAD_MANIFEST_SCHEMA,
            "release": {"id": "eidnara-host-release", "version": release_contract::RELEASE_VERSION},
            "release_contract_sha256": release_contract::release_contract_sha256(),
            "production_inputs_lock_sha256":
                daemon::production_inputs::production_inputs_lock_sha256(),
            "mode": "production",
            "package": {
                "name": package_name,
                "version": release_contract::RELEASE_VERSION,
                "target": target
            },
            "platform_floor": {"kernel_min": "4.18", "glibc_min": "2.28"},
            "synapse": "certified_cpu",
            "launcher": "payload/bin/eidnara-host",
            "files": [
                {
                    "path": "payload/bin/eidnara-host",
                    "type": "file",
                    "size": 8,
                    "mode": "755",
                    "sha256": hash(b"launcher")
                },
                {
                    "path": "payload/model/model.onnx",
                    "type": "file",
                    "size": 8,
                    "mode": "644",
                    "sha256": hash(b"model-v1")
                }
            ]
        });
        let manifest_bytes = format!(
            "{}\n",
            serde_json::to_string_pretty(&manifest).expect("manifest")
        )
        .into_bytes();
        let manifest_digest = hash(
            manifest_bytes
                .strip_suffix(b"\n")
                .expect("trailing newline"),
        );
        std::fs::write(
            payload.path().join("payload-manifest.json"),
            &manifest_bytes,
        )
        .expect("manifest write");
        let sources =
            trusted_payload_sources(payload.path(), &manifest_digest).expect("trusted sources");
        std::fs::write(&model_path, b"model-v2").expect("mutate model");
        let store = GenerationStore::open(Some(store_root.path())).expect("store");
        let result = store.stage_and_promote(
            &sources.sources,
            &StageMeta {
                target: "linux-x64-gnu".to_owned(),
                release_contract_sha256: release_contract::release_contract_sha256().to_owned(),
                inputs_lock_sha256: sources.inputs_lock_sha256,
                source_payload_manifest_sha256: manifest_digest,
            },
            &BTreeSet::new(),
        );
        assert!(matches!(
            result,
            Err(GenerationError::NativePayloadInvalid { .. })
        ));
    }

    #[test]
    fn launcher_materialization_removes_source_paths_from_serve_envelope() {
        let root = tempfile::tempdir().expect("data root");
        let secret_source = "/private/package-cache/opencode";
        let mut envelope = serve::LauncherEnvelope::empty();
        envelope.opencode = Some(serve::HarnessCandidate {
            manifest_sha256: "ab".repeat(32),
            source_roots: std::collections::BTreeMap::from([(
                "runtime".to_owned(),
                PathBuf::from(secret_source),
            )]),
        });
        let startup = envelope
            .prepare(root.path().to_path_buf(), serve::SelectionMode::Fresh)
            .expect("prepare isolated envelope")
            .to_startup("cd".repeat(32));
        assert!(matches!(
            startup.opencode,
            Some(serve::HarnessSnapshot::Unavailable {
                reason: serve::HarnessUnavailableReason::DescriptorInvalid
            })
        ));
        let serialized = serde_json::to_string(&startup).expect("serialize startup");
        assert!(!serialized.contains(secret_source));
        assert!(!serialized.contains("source_roots"));

        let ready = serve::StartupEnvelope {
            schema: serve::STARTUP_ENVELOPE_SCHEMA,
            data_dir: root.path().to_path_buf(),
            payload_manifest_digest: "cd".repeat(32),
            opencode: Some(serve::HarnessSnapshot::Ready {
                manifest_sha256: "ef".repeat(32),
            }),
            pi: None,
            credentials: std::collections::BTreeMap::new(),
        };
        let ready_json = serde_json::to_value(ready).expect("serialize ready startup");
        assert_eq!(
            ready_json["opencode"],
            serde_json::json!({
                "state": "ready",
                "manifest_sha256": "ef".repeat(32),
            })
        );
        assert!(ready_json.get("source_roots").is_none());
    }

    #[test]
    fn daemon_version_range_check_uses_contract_bounds() {
        assert!(daemon_version_compatible("eidnara-host/0.1.0"));
        assert!(daemon_version_compatible("eidnara-host/0.1.9"));
        assert!(!daemon_version_compatible("eidnara-host/0.2.0"));
        assert!(!daemon_version_compatible("eidnara-host/0.0.9"));
        assert!(!daemon_version_compatible("other/0.1.0"));
        assert!(!daemon_version_compatible("eidnara-host/1"));
    }

    #[test]
    fn status_verdict_applies_the_daemon_range_to_a_running_publication() {
        let running = |daemon_ver: Option<&str>| LifecycleProbe {
            state: LifecycleState::Running,
            reason: "running",
            record: None,
            publication: daemon_ver.map(|daemon_ver| host_runtime::PublicationSummary {
                daemon_id: "00".repeat(16),
                daemon_ver: daemon_ver.to_owned(),
                pid: 1,
                setup_socket: "setup.sock".to_owned(),
            }),
            instance_lock_free: false,
            lifetime_lock_free: false,
        };

        assert_eq!(
            probe_verdict(&running(Some("eidnara-host/0.1.0"))),
            (true, "healthy")
        );
        // Below `min_inclusive`, above `max_exclusive`, and a malformed version all withhold `ok`.
        assert_eq!(
            probe_verdict(&running(Some("eidnara-host/0.0.9"))),
            (false, "incompatible_daemon")
        );
        assert_eq!(
            probe_verdict(&running(Some("eidnara-host/0.2.0"))),
            (false, "incompatible_daemon")
        );
        assert_eq!(
            probe_verdict(&running(Some("other/0.1.0"))),
            (false, "incompatible_daemon")
        );
        // A running incarnation whose publication has not been read yet keeps the state verdict.
        assert_eq!(probe_verdict(&running(None)), (true, "healthy"));
        assert_eq!(
            remediation_for("incompatible_daemon"),
            Some("align_versions")
        );

        let mut stopped = running(None);
        stopped.state = LifecycleState::Stopped;
        assert_eq!(probe_verdict(&stopped), (false, "not_running"));

        let mut quarantined = running(Some("eidnara-host/0.1.0"));
        quarantined.reason = host_runtime::UNSUPPORTED_STATE_SCHEMA_REASON;
        assert_eq!(
            probe_verdict(&quarantined),
            (false, host_runtime::UNSUPPORTED_STATE_SCHEMA_REASON)
        );
    }

    #[test]
    fn publication_check_follows_the_authentication_verdict() {
        let check = |state: &'static str, reason: &'static str| {
            let result = DaemonResult::new("start", false, state, reason).finish();
            let publication = result
                .checks
                .iter()
                .find(|check| check.id == "lifecycle.publication")
                .expect("publication check");
            (
                publication.status,
                publication.reason,
                publication.remediation,
            )
        };
        assert_eq!(
            check("running", AUTHENTICATION_FAILED),
            (
                "fail",
                AUTHENTICATION_FAILED,
                Some("inspect_daemon_process")
            )
        );
        assert_eq!(check("running", "healthy"), ("pass", "healthy", None));
        assert_eq!(
            check("running", "incompatible_daemon"),
            ("pass", "healthy", None)
        );
        assert_eq!(
            check("wedged", "wedged"),
            ("fail", "wedged", Some("inspect_daemon_process"))
        );
        assert_eq!(check("stopped", "not_running"), ("skip", "healthy", None));
    }

    #[test]
    fn version_floor_compares_the_leading_major_minor() {
        assert_eq!(
            version_at_least("6.12.103-127.188.amzn2023.x86_64", "4.18"),
            Some(true)
        );
        assert_eq!(
            version_at_least("4.18.0-553.el8_10.x86_64", "4.18"),
            Some(true)
        );
        assert_eq!(version_at_least("4.17.9", "4.18"), Some(false));
        assert_eq!(version_at_least("3.99.0", "4.18"), Some(false));
        assert_eq!(version_at_least("5.4", "4.18"), Some(true));
        assert_eq!(version_at_least("6.12-rc1", "4.18"), Some(true));
        assert_eq!(version_at_least("2.34", "2.28"), Some(true));
        assert_eq!(version_at_least("2.28", "2.28"), Some(true));
        assert_eq!(version_at_least("2.27", "2.28"), Some(false));
        assert_eq!(version_at_least("2.4", "2.28"), Some(false));
        assert_eq!(version_at_least("", "4.18"), None);
        assert_eq!(version_at_least("6", "4.18"), None);
        assert_eq!(version_at_least("kernel", "4.18"), None);
        assert_eq!(version_at_least("6.12", "x.y"), None);
    }

    #[test]
    fn platform_floor_gate_reads_the_contract_and_fails_closed() {
        let floor = platform_floor("linux-x64-gnu");
        assert_eq!(floor.kernel_min, "4.18");
        assert_eq!(floor.glibc_min, "2.28");
        assert!(floor.procfs_self_fd_exec);

        let host = |kernel: Option<&str>, glibc: Option<&str>, procfs: bool| HostPlatform {
            kernel_release: kernel.map(str::to_owned),
            glibc_version: glibc.map(str::to_owned),
            procfs_self_fd: procfs,
        };
        assert!(host_meets_platform_floor(
            &floor,
            &host(Some("6.12.103-127.amzn2023.x86_64"), Some("2.34"), true)
        ));
        assert!(host_meets_platform_floor(
            &floor,
            &host(Some("4.18.0-553.el8"), Some("2.28"), true)
        ));
        // Each floor component fails closed on its own, including a failed observation.
        assert!(!host_meets_platform_floor(
            &floor,
            &host(Some("4.15.0-1051-aws"), Some("2.34"), true)
        ));
        assert!(!host_meets_platform_floor(
            &floor,
            &host(Some("6.12.0"), Some("2.17"), true)
        ));
        assert!(!host_meets_platform_floor(
            &floor,
            &host(Some("6.12.0"), Some("2.34"), false)
        ));
        assert!(!host_meets_platform_floor(
            &floor,
            &host(None, Some("2.34"), true)
        ));
        assert!(!host_meets_platform_floor(
            &floor,
            &host(Some("6.12.0"), None, true)
        ));

        let mut relaxed = platform_floor("linux-x64-gnu");
        relaxed.procfs_self_fd_exec = false;
        assert!(host_meets_platform_floor(
            &relaxed,
            &host(Some("6.12.0"), Some("2.34"), false)
        ));
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
    #[test]
    fn the_test_host_meets_the_declared_floor() {
        let observed = observe_host_platform();
        assert!(
            observed.kernel_release.is_some(),
            "kernel release is readable"
        );
        assert!(
            observed.glibc_version.is_some(),
            "glibc version is readable"
        );
        assert!(observed.procfs_self_fd, "procfs self links resolve");
        assert_eq!(supported_target(), Ok("linux-x64-gnu"));
    }
    #[test]
    fn daemon_version_shape_matches_the_typescript_gate() {
        assert!(!daemon_version_compatible("eidnara-host/+0.1.0"));
        assert!(!daemon_version_compatible("eidnara-host/0.+1.0"));
        assert!(!daemon_version_compatible("eidnara-host/0.1.+0"));
        assert!(!daemon_version_compatible("eidnara-host/0..0"));
        assert!(!daemon_version_compatible("eidnara-host/0.1."));
        assert!(!daemon_version_compatible("eidnara-host/-0.1.0"));
        assert!(!daemon_version_compatible("eidnara-host/ 0.1.0"));
        assert!(!daemon_version_compatible("eidnara-host/0.1.0 "));
        assert!(!daemon_version_compatible("eidnara-host/0.1.0-rc1"));
        assert!(!daemon_version_compatible("eidnara-host/0.1.0.0"));
        assert!(!daemon_version_compatible("eidnara-host/0.01.0"));
        assert!(!daemon_version_compatible("eidnara-host/00.1.0"));
        assert!(!daemon_version_compatible("eidnara-host/0.1.00"));
        assert!(!daemon_version_compatible("eidnara-host/01.2.3"));
        assert!(daemon_version_compatible("eidnara-host/0.1.0"));
    }
}
