//!
//! OpenCode and Pi use this runner to prevent their subprocess-security behavior from diverging.
//! The runner does not invoke a shell and uses a dedicated Unix process group.
//! The runner uses a dedicated Unix process group as the cancellation unit and snapshots the daemon-startup environment.
//! The runner removes host launch-identity variables from its environment snapshot and delivers prompts over stdin.
//! The runner creates temp directories and files with `0700` and `0600` permissions and drains stdout and stderr concurrently with bounded buffers.
//! On timeout, the runner terminates the process group gracefully, then escalates to `SIGKILL`.
//! The runner reaps the process-group leader before reporting completion and emits only redacted structural diagnostics.
//! only.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use hmac::{Hmac, Mac};
use sha2::Sha256;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use super::backend::{
    BackendError, BackendEvent, BackendTerminal, ErrorClass, EventSink, Harness, SinkStatus,
};

/// Every child environment excludes the host launch-identity variables.
/// A harness child inheriting these variables could reconnect to the daemon as the supervised module.
pub const HOST_LAUNCH_IDENTITY_VARS: [&str; 2] = [
    crate::wire::EIDNARA_MODULE_ID_ENV,
    crate::wire::EIDNARA_LAUNCH_NONCE_ENV,
];

/// `EnvSnapshot` preserves an immutable copy of the daemon-startup environment.
/// `EnvSnapshot` treats provider credentials and user configuration as trusted inputs.
/// Capture removes host launch-identity variables so later environment composition cannot reintroduce them.
///
/// The `Arc` lets per-run `execute` clones and the Pi provider fallback share the environment allocation.
#[derive(Clone)]
pub struct EnvSnapshot {
    vars: Arc<[(OsString, OsString)]>,
}

/// This cap rejects credential values larger than 16 KiB.
pub const CREDENTIAL_VALUE_CAP_BYTES: usize = 16 * 1024;
/// Every credential fingerprint includes this key-derivation domain.
pub const CREDENTIAL_FINGERPRINT_DOMAIN: &str = "eidnara-broca-credential-v1";
/// This identifier fixes the credential-fingerprint pre-image layout.
/// `credential_fingerprint.canonicalization`.
pub const CREDENTIAL_FINGERPRINT_CANONICALIZATION: &str = "harness-provider-name-length-value/1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialRowError {
    ProviderUnsupported,
    CredentialMissing,
    CredentialValueTooLarge,
}

impl CredentialRowError {
    pub fn subreason(self) -> &'static str {
        match self {
            Self::ProviderUnsupported => "provider_unsupported",
            Self::CredentialMissing => "credential_missing",
            Self::CredentialValueTooLarge => "credential_value_too_large",
        }
    }
}

/// All provider canonicalization resolves through this function so aliases accepted during row selection remain accepted with the same canonical name during fingerprint derivation and send-time verification.
pub fn canonical_provider(
    harness: &str,
    provider: &str,
) -> Result<&'static str, CredentialRowError> {
    match (harness, provider) {
        ("pi", "google-antigravity") => Ok("google"),
        ("pi", "openai-codex") => Ok("openai"),
        ("opencode" | "pi", "anthropic") => Ok("anthropic"),
        ("opencode" | "pi", "google") => Ok("google"),
        ("opencode" | "pi", "openai") => Ok("openai"),
        _ => Err(CredentialRowError::ProviderUnsupported),
    }
}

impl EnvSnapshot {
    /// The constructor builds a bounded snapshot from explicit startup variables.
    /// Each variable is charged its string bytes plus `ENV_ENTRY_OVERHEAD_BYTES`, so many short variables cannot bypass the ceiling through per-entry container overhead.
    ///
    ///
    /// [`ENV_ENTRY_OVERHEAD_BYTES`]: super::config::ENV_ENTRY_OVERHEAD_BYTES
    /// [`MAX_ENV_SNAPSHOT_BYTES`]: super::config::MAX_ENV_SNAPSHOT_BYTES
    pub fn capture_from(vars: impl IntoIterator<Item = (OsString, OsString)>) -> io::Result<Self> {
        let snapshot = Self::from_vars(vars);
        let bytes: usize = snapshot
            .vars
            .iter()
            .map(|(name, value)| {
                name.as_os_str().len()
                    + value.as_os_str().len()
                    + 2
                    + super::config::ENV_ENTRY_OVERHEAD_BYTES
            })
            .sum();
        if bytes > super::config::MAX_ENV_SNAPSHOT_BYTES {
            return Err(io::Error::other(format!(
                "startup environment charges {bytes} bytes ({} variables), \
                 over the {} byte snapshot ceiling",
                snapshot.vars.len(),
                super::config::MAX_ENV_SNAPSHOT_BYTES
            )));
        }
        Ok(snapshot)
    }

    /// Snapshots never carry `EIDNARA_MODULE_ID` or `EIDNARA_LAUNCH_NONCE`.
    /// Snapshots exclude `EIDNARA_MODULE_ID` and `EIDNARA_LAUNCH_NONCE` regardless of construction path.
    pub fn from_vars(vars: impl IntoIterator<Item = (OsString, OsString)>) -> Self {
        let vars = vars
            .into_iter()
            .filter(|(name, _)| {
                !HOST_LAUNCH_IDENTITY_VARS
                    .iter()
                    .any(|identity| OsStr::new(identity) == name.as_os_str())
            })
            .collect::<Vec<_>>()
            .into();
        Self { vars }
    }

    pub fn vars(&self) -> &[(OsString, OsString)] {
        &self.vars
    }

    /// No ambient loader, proxy, cloud-chain, package manager, HOME/XDG, PATH, or unrelated provider variable survives.
    pub fn provider_row(
        &self,
        harness: &str,
        provider: &str,
    ) -> Result<Vec<(OsString, OsString)>, CredentialRowError> {
        let variable = match canonical_provider(harness, provider)? {
            "anthropic" => "ANTHROPIC_API_KEY",
            "google" => "GEMINI_API_KEY",
            "openai" => "OPENAI_API_KEY",
            _ => return Err(CredentialRowError::ProviderUnsupported),
        };
        let Some((name, value)) = self
            .vars
            .iter()
            .find(|(name, _)| name.as_os_str() == OsStr::new(variable))
        else {
            return Err(CredentialRowError::CredentialMissing);
        };
        if value.is_empty() {
            return Err(CredentialRowError::CredentialMissing);
        }
        if value.len() > CREDENTIAL_VALUE_CAP_BYTES {
            return Err(CredentialRowError::CredentialValueTooLarge);
        }
        Ok(vec![(name.clone(), value.clone())])
    }

    pub fn credential_fingerprint(
        &self,
        connection_key: &[u8; 32],
        harness: &str,
        provider: &str,
    ) -> Result<String, CredentialRowError> {
        let canonical = canonical_provider(harness, provider)?;
        let row = self.provider_row(harness, canonical)?;
        let encoded = |field: &str| format!("{}:{field}", field.len());
        let mut message = encoded(CREDENTIAL_FINGERPRINT_CANONICALIZATION)
            + &encoded(harness)
            + &encoded(canonical);
        for (name, value) in row {
            let name = name.to_string_lossy();
            let value = value.to_string_lossy();
            message.push_str(&encoded(&name));
            message.push_str(&encoded(&value.len().to_string()));
            message.push_str(&encoded(&value));
        }
        let mut derive =
            Hmac::<Sha256>::new_from_slice(connection_key).expect("HMAC accepts any key length");
        derive.update(CREDENTIAL_FINGERPRINT_DOMAIN.as_bytes());
        let derived = derive.finalize().into_bytes();
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&derived).expect("HMAC accepts any key length");
        mac.update(message.as_bytes());
        Ok(mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect())
    }
}

impl std::fmt::Debug for EnvSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Environment values are credentials; report only the count.
        f.debug_struct("EnvSnapshot")
            .field("var_count", &self.vars.len())
            .finish()
    }
}

/// `ExecutionBounds` applies to one harness run.
/// fast.
#[derive(Clone, Debug)]
pub struct SubprocessLimits {
    /// `timeout` terminates an elapsed run and maps it to one failed terminal.
    pub run_timeout: Duration,
    /// `termination_grace` delays group SIGKILL after group SIGTERM.
    pub termination_grace: Duration,
    /// `post_eof_grace` waits for the child to exit after clean stream EOF before forced termination.
    /// Clean stream EOF can precede process exit.
    pub drain_grace: Duration,
    /// `stdout_limit` stops a child that exceeds the retained stdout bound.
    pub max_stdout_bytes: usize,
    /// The runner retains bounded stderr for diagnostics only.
    pub max_stderr_bytes: usize,
}

impl Default for SubprocessLimits {
    fn default() -> Self {
        Self {
            run_timeout: Duration::from_secs(660),
            termination_grace: Duration::from_secs(5),
            drain_grace: Duration::from_secs(2),
            max_stdout_bytes: super::config::MAX_BACKEND_STDOUT_BYTES,
            max_stderr_bytes: super::config::MAX_BACKEND_STDERR_BYTES,
        }
    }
}

/// The prompt travels only through stdin.
/// `stdin` carries the prompt; argv carries flags and trusted paths, never caller text.
pub struct SubprocessSpec {
    pub executable: PathBuf,
    /// Path arguments use `OsString` so non-UTF-8 paths reach the child unchanged.
    pub args: Vec<OsString>,
    /// The child uses `env_clear`; adapter-owned control variables follow snapshot variables and win collisions.
    pub env: Vec<(OsString, OsString)>,
    pub working_dir: PathBuf,
    pub stdin: Vec<u8>,
    /// `inherit_fds` retains descriptors referenced by child path arguments.
    pub inherit_fds: Vec<RawFd>,
    /// The crash-ownership record for the child's process group is written here before the child execs. commentlint: allow(JUDGE)
    pub state_root: group_registry::StateRoot,
}

/// child output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubprocessEnd {
    Exited(i32),
    /// DrainKilled means the transcript completed but the leader exceeded the drain grace or kept pipes open after a decisive probe; parsers treat it as a clean exit.
    /// DrainKilled applies when the leader outlives the drain grace after clean stream EOF.
    /// DrainKilled also applies when a terminal probe recognizes a decisive transcript while the child keeps its pipes open.
    DrainKilled,
    /// `Signaled` means `run` did not send the killing signal.
    Signaled,
    TimedOut,
    Cancelled,
    StdoutOverflow,
    StderrOverflow,
    /// CaptureFailed records a stdout read failure; parsers distrust the transcript even after a clean exit.
    CaptureFailed,
    /// The leader exited but its status could not be reaped, so a clean exit cannot be proven; parsers distrust the transcript. commentlint: allow(JUDGE)
    ExitUnknown,
    /// A group signal failure other than "already gone" leaves the run unsettled because descendants may still execute billable requests.
    TeardownUnconfirmed,
}

/// ProbeSignal tells run whether newly completed stdout permits shortening the deadline to the drain grace without treating a retryable failure as final.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeSignal {
    /// `Quiet` provides no completion signal.
    Quiet,
    /// `Decisive` prevents later output from changing the terminal classification.
    Decisive,
    /// A retryable terminal failure arms the drain grace so a final failure retains its classification; ProbeSignal::Continues before the grace expires restores the full deadline.
    /// deadline.
    Provisional,
    /// ProbeSignal::Continues indicates that new work began, so provisional drain arming was premature.
    Continues,
}

pub struct SubprocessResult {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub end: SubprocessEnd,
    /// prompt_delivered is true only when the whole prompt reached the child's stdin before it closed; parsers reject transcripts otherwise, even after a clean end state.
    pub prompt_delivered: bool,
    /// `record_retained` is true when the group is proven gone but its crash-ownership record could not be removed, so a successor's startup sweep will encounter it.
    pub record_retained: bool,
}

/// Returns after the leader is reaped on every path, including timeout, cancellation, and overflow, so lifecycle completion upstream can never observe a live child.
///
/// `terminal_probe` inspects each newly completed stdout-line region exactly once.
/// A terminal signal rearms the run deadline to the drain grace.
/// If the harness resumes before the grace expires, it is not killed during its retry.
pub async fn run(
    spec: SubprocessSpec,
    limits: &SubprocessLimits,
    cancel: &CancellationToken,
    terminal_probe: Option<fn(&[u8]) -> ProbeSignal>,
) -> io::Result<SubprocessResult> {
    let SubprocessSpec {
        executable,
        args,
        env,
        working_dir,
        stdin: prompt,
        inherit_fds,
        state_root,
    } = spec;
    // The budget is anchored before spawn and registration: a slow crash-record publication
    // consumes this run's budget rather than granting the child a stale full remainder measured
    // by the caller before `run` was called. commentlint: allow(JUDGE)
    let run_deadline = tokio::time::Instant::now() + limits.run_timeout;
    let mut command = tokio::process::Command::new(&executable);
    command
        .args(&args)
        .env_clear()
        .current_dir(&working_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // A fresh process group makes the harness descendant tree the cancellation unit.
        // Group cleanup targets provider and extension grandchildren in the harness process group.
        .process_group(0)
        // Ordinary paths reap explicitly; `Drop` is only a backstop.
        .kill_on_drop(true);
    // On Linux, `pdeathsig` (`PR_SET_PDEATHSIG`) asks the kernel to SIGKILL the leader when the
    // *thread* that forked it exits, not merely the parent process. `queue_spawn`
    // forks from a thread that remains alive for the leader's lifetime; a retired
    // `spawn_blocking` thread would SIGKILL the leader, and forking on a runtime worker would
    // block that worker for the whole exec-barrier handshake.
    // Drop cleanup does not run after SIGKILL.
    // `pdeathsig` applies only to the leader.
    // The startup sweep in [`group_registry`] handles descendants that survive the leader.
    // Group cleanup handles descendants on ordinary termination paths.
    //
    // The parent check closes the window where the host dies between `fork` and `pdeathsig` setup.
    // The check compares the child's parent PID with `host_pid` captured before `fork`.
    // Comparing with `host_pid` permits a host running as PID 1.
    // The parent check aborts when the child no longer has `host_pid` as its parent.
    let host_pid = std::process::id();
    let child_inherit_fds = inherit_fds.clone();
    // Register crash ownership before `exec` so successor sweeps can find helpers after a host crash: `pdeathsig` kills only the leader, and a helper forked before record publication would otherwise be unrecorded. commentlint: allow(JUDGE)
    // `spawn` waits for `exec`, so the blocking-pool registrar reads `pid_report`, publishes the record, then releases `exec_barrier`.
    let (pid_report_read, pid_report_write) =
        rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)?;
    let (exec_barrier_read, exec_barrier_write) =
        rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)?;
    let child_pid_report_write = pid_report_write.as_raw_fd();
    let child_exec_barrier_read = exec_barrier_read.as_raw_fd();
    let child_pid_report_read = pid_report_read.as_raw_fd();
    let child_exec_barrier_write = exec_barrier_write.as_raw_fd();
    // SAFETY: `pre_exec` runs after `fork` and before `exec`, so its closure must avoid allocation and locking.
    // Every error is built from a raw errno; `io::Error::other` would allocate.
    #[allow(unsafe_code)]
    unsafe {
        command.pre_exec(move || {
            #[cfg(target_os = "linux")]
            rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::KILL))?;
            let parent = rustix::process::getppid().map(|pid| pid.as_raw_nonzero().get() as u32);
            if parent != Some(host_pid) {
                // The host exited before spawn completed.
                return Err(io::Error::from_raw_os_error(libc::ESRCH));
            }
            for fd in &child_inherit_fds {
                if libc::fcntl(*fd, libc::F_SETFD, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            // The child closes registrar-side pipe ends so dropping `exec_barrier_write` delivers EOF to `child_exec_barrier_read`.
            libc::close(child_pid_report_read);
            libc::close(child_exec_barrier_write);
            let pid = libc::getpid().to_ne_bytes();
            let mut sent = 0;
            while sent < pid.len() {
                let written = libc::write(
                    child_pid_report_write,
                    pid.as_ptr().add(sent).cast(),
                    pid.len() - sent,
                );
                if written < 0 {
                    // `last_os_error` reads `errno` portably (glibc-only `__errno_location` breaks non-Linux builds) and stores the raw code inline without allocating. commentlint: allow(JUDGE)
                    let err = io::Error::last_os_error();
                    if err.raw_os_error() == Some(libc::EINTR) {
                        continue;
                    }
                    return Err(err);
                }
                sent += written as usize;
            }
            libc::close(child_pid_report_write);
            let mut byte = 0u8;
            loop {
                let received = libc::read(child_exec_barrier_read, (&raw mut byte).cast(), 1);
                if received == 1 {
                    break;
                }
                if received == 0 {
                    // EOF means the registrar dropped the barrier without publishing a record.
                    // Executing without a published record could orphan helper processes outside the registry.
                    return Err(io::Error::from_raw_os_error(libc::ECANCELED));
                }
                let err = io::Error::last_os_error();
                if err.raw_os_error() != Some(libc::EINTR) {
                    return Err(err);
                }
            }
            Ok(())
        });
    }
    for (name, value) in &env {
        command.env(name, value);
    }

    // The registrar starts before the spawn job because `spawn` blocks the spawner thread until the child execs.
    // `abort_registration` keeps a stalled record publication from wedging this task: once set, the registrar withholds the exec barrier, so the child aborts before executing harness code. commentlint: allow(JUDGE)
    // The pid is reported the moment the child writes it, so an aborting caller can tear down the group without waiting for the registry filesystem. commentlint: allow(JUDGE)
    let abort_registration = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let registrar_abort = Arc::clone(&abort_registration);
    let (pid_reported_send, mut pid_reported) = tokio::sync::oneshot::channel::<i32>();
    let registrar = tokio::task::spawn_blocking(move || {
        let mut pid_bytes = [0u8; size_of::<libc::pid_t>()];
        let mut filled = 0;
        while filled < pid_bytes.len() {
            match rustix::io::read(&pid_report_read, &mut pid_bytes[filled..]) {
                // EOF: the child died before reporting its pid.
                Ok(0) => return None,
                Ok(read) => filled += read,
                Err(rustix::io::Errno::INTR) => {}
                Err(_) => return None,
            }
        }
        let leader = libc::pid_t::from_ne_bytes(pid_bytes);
        let _ = pid_reported_send.send(leader);
        if registrar_abort.load(std::sync::atomic::Ordering::SeqCst) {
            // Withholding the barrier aborts the child before exec, so no record is needed.
            return None;
        }
        let record = group_registry::GroupRecord::record(&state_root, leader)?;
        if registrar_abort.load(std::sync::atomic::Ordering::SeqCst) {
            // The run gave up during publication; the barrier stays withheld so the child aborts, and the published record is returned for the abort path's cleanup task. commentlint: allow(JUDGE)
            return Some(record);
        }
        // The already-published record is returned even when this write fails; the caller owns its removal. commentlint: allow(JUDGE)
        let _ = rustix::io::write(&exec_barrier_write, &[1u8]);
        Some(record)
    });

    let mut spawn_reply = queue_spawn(command, pid_report_write, exec_barrier_read)?;
    // Dropping `env` releases its environment-sized allocation before concurrent runs continue. commentlint: allow(JUDGE)
    drop(env);
    // A stalled record publication blocks the spawner thread, not this task: cancellation and the run deadline stop waiting, and the abort flag makes the registrar withhold the exec barrier. commentlint: allow(JUDGE)
    let (spawned, aborted_end) = tokio::select! {
        biased;
        spawned = &mut spawn_reply => (Some(spawned), None),
        () = cancel.cancelled() => (None, Some(SubprocessEnd::Cancelled)),
        () = tokio::time::sleep_until(run_deadline) => (None, Some(SubprocessEnd::TimedOut)),
    };
    if let Some(end) = aborted_end {
        abort_registration.store(true, std::sync::atomic::Ordering::SeqCst);
        let mut spawn_reply = Some(spawn_reply);
        // The child reports its pid immediately after `fork`, so this wait is bounded by process startup rather than the registry filesystem. commentlint: allow(JUDGE)
        // The timeout bounds pid reporting; the abort flag makes a still-queued job's registrar withhold the exec barrier when it runs. commentlint: allow(JUDGE)
        let group_gone = match tokio::time::timeout(limits.termination_grace, &mut pid_reported)
            .await
        {
            // The reported PID identifies the group; killing it and waiting for members covers helpers, and reaping the settled spawn reply proves the leader itself exited — a queued SIGKILL alone does not. commentlint: allow(JUDGE)
            Ok(Ok(pid)) => {
                let group = rustix::process::Pid::from_raw(pid);
                let signalled = kill_group(group, rustix::process::Signal::KILL).is_ok();
                // The unreaped leader pins the pgid during the member scan, so the reap comes after. commentlint: allow(JUDGE)
                let members_gone = wait_other_members_gone(group, limits.termination_grace).await;
                let reply = spawn_reply
                    .take()
                    .expect("the abort path consumes the reply once");
                let leader_reaped =
                    match tokio::time::timeout(limits.termination_grace, reply).await {
                        Ok(Ok(Ok(mut child))) => {
                            tokio::time::timeout(limits.termination_grace, child.wait())
                                .await
                                .is_ok_and(|waited| waited.is_ok())
                        }
                        // A spawn error means std already reaped the failed child.
                        Ok(Ok(Err(_))) => true,
                        _ => false,
                    };
                signalled && members_gone && leader_reaped
            }
            // A closed channel means the child died before completing its pid report, so it never passed the barrier or forked; reaping the settled spawn reply proves the leader gone. commentlint: allow(JUDGE)
            Ok(Err(_)) => {
                let reply = spawn_reply
                    .take()
                    .expect("the abort path consumes the reply once");
                match tokio::time::timeout(limits.termination_grace, reply).await {
                    Ok(Ok(Ok(mut child))) => {
                        tokio::time::timeout(limits.termination_grace, child.wait())
                            .await
                            .is_ok_and(|waited| waited.is_ok())
                    }
                    // A spawn error means std already reaped the failed child.
                    Ok(Ok(Err(_))) => true,
                    _ => false,
                }
            }
            // Without a pid the child cannot be identified, so teardown stays unproven. commentlint: allow(JUDGE)
            Err(_) => false,
        };
        let mut registrar = registrar;
        // A registrar that settles within the grace yields an exact record verdict; one still
        // stalled has an unknown record fate, so the run conservatively reports the record
        // retained (when teardown is proven) and detaches best-effort removal. commentlint: allow(JUDGE)
        let record_retained =
            match tokio::time::timeout(limits.termination_grace, &mut registrar).await {
                Ok(joined) => match joined.ok().flatten() {
                    Some(record) if group_gone => {
                        remove_record_bounded(record, limits.termination_grace).await
                    }
                    // An unproven teardown intentionally retains the record for a successor sweep, matching the drain-loop paths. commentlint: allow(JUDGE)
                    _ => false,
                },
                Err(_) => {
                    tokio::spawn(async move {
                        let record = registrar.await.ok().flatten();
                        if group_gone && let Some(record) = record {
                            let _ = off_runtime(move || record.remove()).await;
                        }
                    });
                    group_gone
                }
            };
        // The leader is reaped explicitly whenever the spawner eventually replies; `kill_on_drop` plus the runtime's orphan reaper only backstop a lost reply. commentlint: allow(JUDGE)
        if let Some(reply) = spawn_reply.take() {
            tokio::spawn(async move {
                if let Ok(Ok(mut child)) = reply.await {
                    let _ = child.wait().await;
                }
            });
        }
        return Ok(SubprocessResult {
            stdout: Vec::new(),
            stderr: Vec::new(),
            end: if group_gone {
                end
            } else {
                SubprocessEnd::TeardownUnconfirmed
            },
            prompt_delivered: false,
            record_retained,
        });
    }
    let spawned = spawned
        .expect("the biased select returned either a spawn result or an abort")
        .map_err(|_| io::Error::other("the broca-spawner thread dropped a spawn reply"))?;
    let group_record = registrar.await.ok().flatten();
    let mut child = match spawned {
        Ok(child) => child,
        Err(err) => {
            // A record published before an `exec` failure covers an empty group (the child exited without executing harness code), so only the record needs removing. commentlint: allow(JUDGE)
            if let Some(record) = group_record
                && remove_record_bounded(record, limits.termination_grace).await
            {
                return Err(io::Error::other(SpawnRecordRetained { kind: err.kind() }));
            }
            // `ECANCELED` comes only from the withheld exec barrier: registration failed, the child aborted before exec, and a later attempt can succeed. commentlint: allow(JUDGE)
            if err.raw_os_error() == Some(libc::ECANCELED) {
                return Err(io::Error::other(RegistrationFailed));
            }
            return Err(err);
        }
    };
    // With process_group(0) the leader's pid IS the group id.
    let group = child
        .id()
        .and_then(|pid| i32::try_from(pid).ok())
        .and_then(rustix::process::Pid::from_raw);
    // A successful `spawn` implies the registrar published a record before releasing the barrier; a missing record after a registrar join failure leaves the group unobservable, so it is torn down. commentlint: allow(JUDGE)
    if group_record.is_none() {
        let signalled = kill_group(group, rustix::process::Signal::KILL).is_ok();
        // `child.start_kill()` covers a missing or unaddressable process group.
        let _ = child.start_kill();
        // `timeout` prevents an uninterruptible leader from blocking `work_done` waiters indefinitely.
        let members_gone = wait_other_members_gone(group, limits.termination_grace).await;
        let reaped = tokio::time::timeout(limits.termination_grace, child.wait())
            .await
            .is_ok();
        if !(signalled && members_gone && reaped) {
            // The registration-failure path returns `RegistrationTeardownUnproven` when teardown cannot be proven.
            // No prompt bytes have flowed because registration precedes stdin delivery.
            // The host cannot report the group as terminated until teardown is proven.
            return Err(io::Error::other(RegistrationTeardownUnproven));
        }
        return Err(io::Error::other(RegistrationFailed));
    }
    // `group_record` remains `Some` until teardown is proven, retaining the registry record.
    // The successor sweeps the group's descendants after this host exits.
    let mut group_record = group_record;

    // Cancellation can arrive during adapter setup or registration, before the drain loop below polls the token.
    // Delivering the prompt after cancellation can start a provider request for an already-cancelled run.
    if cancel.is_cancelled() {
        let group_gone = terminate_group(group, &mut child, limits.termination_grace)
            .await
            .is_ok();
        let mut record_retained = false;
        if group_gone && let Some(record) = group_record.take() {
            record_retained = remove_record_bounded(record, limits.termination_grace).await;
        }
        return Ok(SubprocessResult {
            stdout: Vec::new(),
            stderr: Vec::new(),
            end: if group_gone {
                SubprocessEnd::Cancelled
            } else {
                SubprocessEnd::TeardownUnconfirmed
            },
            prompt_delivered: false,
            record_retained,
        });
    }

    // Concurrent prompt delivery and output draining prevent a child that fills stdout before reading stdin from deadlocking the host.
    // Prompt delivery drops stdin after writing so print-mode reads receive EOF.
    // Cancellation wins races with prompt delivery, preventing writes after cancellation; a prompt delivered to an already-cancelled run could otherwise start a provider request. commentlint: allow(JUDGE)
    // The run deadline bounds delivery the same way so a budget exhausted during registration cannot admit provider work. commentlint: allow(JUDGE)
    // The abandoned write reports non-delivery.
    let writer_cancel = cancel.clone();
    let mut stdin_pipe = child.stdin.take();
    let mut stdin_task = tokio::spawn(async move {
        let Some(mut stdin) = stdin_pipe.take() else {
            return true;
        };
        tokio::select! {
            biased;
            () = writer_cancel.cancelled() => false,
            () = tokio::time::sleep_until(run_deadline) => false,
            delivered = async {
                let written = stdin.write_all(&prompt).await.is_ok();
                stdin.shutdown().await.is_ok() && written
            } => delivered,
        }
    });

    let mut stdout_pipe = child.stdout.take().expect("stdout is piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr is piped");
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stdout_open = true;
    let mut stderr_open = true;
    let mut stdout_chunk = [0u8; 8192];
    let mut stderr_chunk = [0u8; 8192];
    // `run_deadline` preserves the original timeout budget when provisional arming is revoked.
    let deadline = tokio::time::sleep_until(run_deadline);
    tokio::pin!(deadline);
    let mut terminal_seen = false;
    let mut arming_revocable = false;
    // Bytes before `probed_to` have already been probed.
    // line is inspected exactly once no matter how the reads chunk it.
    let mut probed_to = 0usize;

    let abnormal = loop {
        if !stdout_open && !stderr_open {
            break None;
        }
        tokio::select! {
            biased;
            () = cancel.cancelled() => break Some(SubprocessEnd::Cancelled),
            // After the probe fires, the rearmed deadline limits drain grace because the transcript is complete.
            // After the transcript completes, an open pipe marks a shutdown gap rather than a run timeout.
            () = &mut deadline => break Some(if terminal_seen {
                SubprocessEnd::DrainKilled
            } else {
                SubprocessEnd::TimedOut
            }),
            read = stdout_pipe.read(&mut stdout_chunk), if stdout_open => match read {
                Ok(0) => stdout_open = false,
                // A read error is not EOF because the remaining transcript is unknown.
                // A parseable prefix and clean child exit could publish a truncated answer as success.
                // A truncated transcript could omit a contradictory terminal.
                Err(_) => break Some(SubprocessEnd::CaptureFailed),
                Ok(n) => {
                    if stdout.len() + n > limits.max_stdout_bytes {
                        break Some(SubprocessEnd::StdoutOverflow);
                    }
                    stdout.extend_from_slice(&stdout_chunk[..n]);
                    // Probing continues after revocable arming because revocation arrives later.
                    if (!terminal_seen || arming_revocable)
                        && let Some(probe) = terminal_probe {
                            // Searching only new bytes prevents a long line from making newline search quadratic.
                            // Bytes preceding the newly appended chunk were already searched and contained no newline.
                            let appended_at = stdout.len() - n;
                            if let Some(last_newline) = stdout[appended_at..]
                                .iter()
                                .rposition(|byte| *byte == b'\n')
                            {
                                let end = appended_at + last_newline + 1;
                                match probe(&stdout[probed_to..end]) {
                                    ProbeSignal::Quiet => {}
                                    signal @ (ProbeSignal::Decisive
                                    | ProbeSignal::Provisional) => {
                                        terminal_seen = true;
                                        arming_revocable =
                                            signal == ProbeSignal::Provisional;
                                        let drain_deadline =
                                            tokio::time::Instant::now() + limits.drain_grace;
                                        if drain_deadline < deadline.deadline() {
                                            deadline.as_mut().reset(drain_deadline);
                                        }
                                    }
                                    // The code undoes only revocable arming.
                                    // A decisive terminal remains valid after subsequent output.
                                    // The restored deadline may already be past, ending the run as a timeout rather than granting retry free budget.
                                    ProbeSignal::Continues => {
                                        if arming_revocable {
                                            terminal_seen = false;
                                            arming_revocable = false;
                                            deadline.as_mut().reset(run_deadline);
                                        }
                                    }
                                }
                                probed_to = end;
                            }
                        }
                }
            },
            read = stderr_pipe.read(&mut stderr_chunk), if stderr_open => match read {
                Ok(0) | Err(_) => stderr_open = false,
                Ok(n) => {
                    if stderr.len() + n > limits.max_stderr_bytes {
                        break Some(SubprocessEnd::StderrOverflow);
                    }
                    stderr.extend_from_slice(&stderr_chunk[..n]);
                }
            },
        }
    };

    let group_gone;
    let end = match abnormal {
        Some(end) => {
            group_gone = terminate_group(group, &mut child, limits.termination_grace)
                .await
                .is_ok();
            end
        }
        None => {
            // After both streams reach EOF, the code waits up to `limits.drain_grace` for the leader to exit without reaping it; its zombie pins the pgid until the descendant sweep completes.
            let exit = wait_exited_unreaped(group, limits.drain_grace).await;
            if exit != LeaderExit::Running {
                let signalled = kill_group_fenced(
                    group,
                    exit,
                    rustix::process::Signal::KILL,
                    limits.termination_grace,
                )
                .await
                .is_ok();
                // A fenced `KILL` only signals; the code retains the process-group fence until the sweep completes to prevent signaling a recycled pgid.
                // The teardown succeeds only after the process group is observed gone.
                // The teardown checks member disappearance while the unreaped leader pins the pgid.
                // the pgid.
                group_gone =
                    signalled && wait_other_members_gone(group, limits.termination_grace).await;
                // A failed reap (`ExitedUnfenced`: another in-process reaper consumed the leader) leaves the exit status unknown; treating it as a clean drain would let a nonzero exit publish a parseable transcript as success. commentlint: allow(JUDGE)
                child
                    .wait()
                    .await
                    .map_or(SubprocessEnd::ExitUnknown, |status| {
                        status
                            .code()
                            .map_or(SubprocessEnd::Signaled, SubprocessEnd::Exited)
                    })
            } else {
                group_gone = terminate_group(group, &mut child, limits.termination_grace)
                    .await
                    .is_ok();
                SubprocessEnd::DrainKilled
            }
        }
    };
    // The record is removed only once the group is proven gone; otherwise the host retains it for a later sweep.
    let mut record_retained = false;
    let end = if group_gone {
        if let Some(record) = group_record.take() {
            record_retained = remove_record_bounded(record, limits.termination_grace).await;
        }
        end
    } else {
        SubprocessEnd::TeardownUnconfirmed
    };

    // The timeout bounds the write when a surviving process retains stdin.
    // An incomplete write sets `prompt_delivered` to false.
    // A timeout caused by an inherited stdin fd surviving the sweep counts as non-delivery.
    let prompt_delivered = match tokio::time::timeout(Duration::from_secs(1), &mut stdin_task).await
    {
        Ok(joined) => joined.unwrap_or(false),
        Err(_) => {
            stdin_task.abort();
            false
        }
    };
    Ok(SubprocessResult {
        stdout,
        stderr,
        end,
        prompt_delivered,
        record_retained,
    })
}

/// `ESRCH` means the group is already gone; any other failure may leave descendants running.
fn kill_group(
    group: Option<rustix::process::Pid>,
    signal: rustix::process::Signal,
) -> io::Result<()> {
    let Some(group) = group else { return Ok(()) };
    match rustix::process::kill_process_group(group, signal) {
        Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// against reuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LeaderExit {
    Running,
    /// An exited, unreaped leader's zombie holds the pgid, so group signals cannot reach a recycled group.
    ExitedFenced,
    /// If `SIGCHLD=SIG_IGN`, `SA_NOCLDWAIT`, or another in-process reaper has reaped the leader, no zombie fences the pgid.
    /// Without a zombie fence, the pgid may already belong to an unrelated group.
    ExitedUnfenced,
}

/// The function waits up to `budget` for the leader to exit without reaping it.
/// An unreaped zombie pins the process-group ID, preventing PID/PGID recycling during descendant sweeps.
async fn wait_exited_unreaped(group: Option<rustix::process::Pid>, budget: Duration) -> LeaderExit {
    let Some(pid) = group else {
        return LeaderExit::Running;
    };
    // Tokio owns the child's `SIGCHLD` handling, so the function uses a 10 ms bounded poll.
    const POLL: Duration = Duration::from_millis(10);
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        let options = rustix::process::WaitIdOptions::EXITED
            | rustix::process::WaitIdOptions::NOWAIT
            | rustix::process::WaitIdOptions::NOHANG;
        match rustix::process::waitid(rustix::process::WaitId::Pid(pid), options) {
            Ok(Some(_)) => return LeaderExit::ExitedFenced,
            // `NotWaitable` proves that the leader exited.
            // deadline decides.
            Err(rustix::io::Errno::CHILD | rustix::io::Errno::SRCH) => {
                return LeaderExit::ExitedUnfenced;
            }
            Err(_) | Ok(None) => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return LeaderExit::Running;
        }
        tokio::time::sleep(POLL).await;
    }
}

/// Membership checks reduce, but cannot eliminate, recycled-pgid signaling.
///
/// A zombie leader pins the pgid, preventing reuse.
/// When the leader has been reaped, the numeric pgid may belong to an unrelated group.
/// Scan errors do not suppress the signal, prioritizing descendant cleanup over a possible leaked group.
/// The check-to-signal window can still target a recycled pgid; never signaling can leak descendants.
async fn kill_group_fenced(
    group: Option<rustix::process::Pid>,
    exit: LeaderExit,
    signal: rustix::process::Signal,
    grace: Duration,
) -> io::Result<()> {
    if exit == LeaderExit::ExitedUnfenced {
        let Some(pid) = group else { return Ok(()) };
        let pgid = pid.as_raw_nonzero().get();
        // Expiry and scan errors treat the group as live to prioritize descendant cleanup over a possibly recycled pgid.
        let live = tokio::time::timeout(
            grace,
            off_runtime(move || {
                group_registry::group_has_members(pgid)
                    .or_else(|_| group_registry::group_has_members(pgid))
                    .unwrap_or(true)
            }),
        )
        .await
        .map_or(true, |scan| scan.unwrap_or(true));
        if !live {
            return Ok(());
        }
    }
    kill_group(group, signal)
}

/// Removes a crash-ownership record on the blocking pool and reports whether it was retained. commentlint: allow(JUDGE)
/// The registry shares a filesystem with record creation, which already runs off-runtime; a stalled removal must likewise stall a pool thread rather than a runtime worker. commentlint: allow(JUDGE)
async fn remove_record_off_runtime(record: group_registry::GroupRecord) -> bool {
    !matches!(off_runtime(move || record.remove()).await, Ok(Ok(())))
}

/// Bounds record removal so a stalled registry cannot wedge the caller past the grace; expiry counts as retained because the removal was not proven. commentlint: allow(JUDGE)
async fn remove_record_bounded(record: group_registry::GroupRecord, grace: Duration) -> bool {
    tokio::time::timeout(grace, remove_record_off_runtime(record))
        .await
        .unwrap_or(true)
}

/// Bounds private-directory cleanup so a stalled filesystem cannot wedge the caller past the grace; expiry reports unproven cleanup rather than claiming the files are gone. commentlint: allow(JUDGE)
pub(crate) async fn bounded_cleanup(
    dir: PrivateDir,
    grace: Duration,
) -> Result<(), CleanupFailure> {
    match tokio::time::timeout(grace, dir.cleanup_async()).await {
        Ok(cleanup) => cleanup,
        Err(_) => Err(cleanup_unproven()),
    }
}

/// Runs a synchronous `/proc` or filesystem step on the blocking pool so it never stalls a runtime worker.
/// A cancelled or panicked blocking task reads as an unknown answer.
pub(crate) async fn off_runtime<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> io::Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|err| io::Error::other(format!("blocking task failed: {err}")))
}

/// Queues a fork on one immortal OS thread and returns the reply channel without waiting. commentlint: allow(JUDGE)
///
/// `pdeathsig` SIGKILLs a leader when its forking thread exits, so a retiring `spawn_blocking` thread may not fork. commentlint: allow(JUDGE)
/// The spawn blocks its thread for the whole exec-barrier handshake, so forking on runtime workers would let concurrent runs occupy every worker and stall request handling, cancellation, and shutdown. commentlint: allow(JUDGE)
/// The job owns the parent-side handshake pipe ends: the raw fd numbers captured by `pre_exec` must stay valid until the fork happens, even when the requesting task gives up first. commentlint: allow(JUDGE)
fn queue_spawn(
    command: tokio::process::Command,
    pid_report_write: std::os::fd::OwnedFd,
    exec_barrier_read: std::os::fd::OwnedFd,
) -> io::Result<tokio::sync::oneshot::Receiver<io::Result<tokio::process::Child>>> {
    struct SpawnJob {
        command: tokio::process::Command,
        // Held through the fork; dropped once `spawn` returns so the registrar sees EOF when no child reported a pid. commentlint: allow(JUDGE)
        handshake_fds: (std::os::fd::OwnedFd, std::os::fd::OwnedFd),
        reply: tokio::sync::oneshot::Sender<io::Result<tokio::process::Child>>,
        handle: tokio::runtime::Handle,
    }
    static SPAWNER: OnceLock<std::sync::mpsc::SyncSender<SpawnJob>> = OnceLock::new();
    let sender = SPAWNER.get_or_init(|| {
        // The queue is bounded at the backend-process cap so a stalled spawn causes later runs to fail fast instead of retaining unbounded jobs, pipe descriptors, and blocked registrars.
        let (sender, jobs) =
            std::sync::mpsc::sync_channel::<SpawnJob>(super::config::MAX_BACKEND_PROCESSES);
        std::thread::Builder::new()
            .name("broca-spawner".to_owned())
            .spawn(move || {
                // The static sender keeps the channel open, so this loop never ends and the thread outlives every leader it forks. commentlint: allow(JUDGE)
                while let Ok(mut job) = jobs.recv() {
                    // `tokio::process::Command::spawn` registers `SIGCHLD` interest and needs the caller's runtime entered on this thread. commentlint: allow(JUDGE)
                    let _guard = job.handle.enter();
                    let spawned = job.command.spawn();
                    drop(job.handshake_fds);
                    let _ = job.reply.send(spawned);
                }
            })
            .expect("spawn the broca-spawner thread");
        sender
    });
    let (reply, spawned) = tokio::sync::oneshot::channel();
    sender
        .try_send(SpawnJob {
            command,
            handshake_fds: (pid_report_write, exec_barrier_read),
            reply,
            handle: tokio::runtime::Handle::current(),
        })
        .map_err(|err| match err {
            std::sync::mpsc::TrySendError::Full(_) => io::Error::other(SpawnerBacklogged),
            std::sync::mpsc::TrySendError::Disconnected(_) => {
                io::Error::other("the broca-spawner thread is gone")
            }
        })?;
    Ok(spawned)
}

/// Each poll walks all of `/proc`; the backoff bounds how many walks a lingering member costs.
const MEMBER_POLL_MIN: Duration = Duration::from_millis(10);
const MEMBER_POLL_MAX: Duration = Duration::from_millis(250);

fn next_member_poll(current: Duration) -> Duration {
    current.saturating_mul(5).min(MEMBER_POLL_MAX)
}

/// Callers must keep the leader unreaped so its zombie prevents pgid recycling during the poll.
/// Return `false` on deadline expiry; scan failures leave teardown unproven and continue polling.
/// Each scan is raced against the remaining budget: a stalled `/proc` walk or a saturated
/// blocking pool must not extend the advertised bound, so expiry mid-scan reads as unproven. commentlint: allow(JUDGE)
async fn wait_other_members_gone(group: Option<rustix::process::Pid>, budget: Duration) -> bool {
    let Some(pid) = group else { return true };
    let pgid = pid.as_raw_nonzero().get();
    let deadline = tokio::time::Instant::now() + budget;
    let mut poll = MEMBER_POLL_MIN;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return false;
        }
        let empty = tokio::time::timeout(
            remaining,
            off_runtime(move || group_registry::group_has_other_members(pgid)),
        )
        .await;
        match empty {
            Ok(scan) => {
                if matches!(scan, Ok(Ok(false))) {
                    return true;
                }
            }
            Err(_) => return false,
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(poll).await;
        poll = next_member_poll(poll);
    }
}

/// Send the final `SIGKILL` before reaping so the zombie pins the pgid.
async fn terminate_group(
    group: Option<rustix::process::Pid>,
    child: &mut tokio::process::Child,
    grace: Duration,
) -> io::Result<()> {
    kill_group(group, rustix::process::Signal::TERM)?;
    if group.is_none() {
        let _ = child.start_kill();
    }
    let mut exit = wait_exited_unreaped(group, grace).await;
    if exit == LeaderExit::Running {
        kill_group(group, rustix::process::Signal::KILL)?;
        let _ = child.start_kill();
        // SIGKILL cannot be caught; the bound limits time spent waiting for exit.
        exit = wait_exited_unreaped(group, grace).await;
    }
    let signalled = kill_group_fenced(group, exit, rustix::process::Signal::KILL, grace).await;
    // Check that no other group members remain before reaping the leader so its zombie pins the pgid.
    // Wait for members to disappear rather than treating `SIGKILL` delivery as teardown proof.
    let members_gone = wait_other_members_gone(group, grace).await;
    // Bound `child.wait()` by `grace` to prevent an unreapable leader from blocking teardown indefinitely.
    if tokio::time::timeout(grace, child.wait()).await.is_err() {
        return Err(io::Error::other(
            "harness leader was not reapable within the termination grace",
        ));
    }
    if !members_gone {
        return Err(io::Error::other(
            "harness group members could not be confirmed stopped within the termination grace",
        ));
    }
    signalled
}

/// Report sensitive-file cleanup failures by error kind only; never include paths or file contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CleanupFailure {
    pub kind: io::ErrorKind,
}

/// The provider fallback uses `CleanupFailure`'s `Display` text as its retry discriminator.
/// The Pi fallback gate does not retry after cleanup leaves private prompt material on disk.
pub(crate) const CLEANUP_FAILURE_MARKER: &str = "cleanup failed";

/// Settlement uses this text to recognize a retained crash-ownership record in a terminal message.
pub(crate) const RECORD_RETAINED_MARKER: &str = "could not remove its crash-ownership record";

impl std::fmt::Display for CleanupFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "sensitive temp {CLEANUP_FAILURE_MARKER} ({})", self.kind)
    }
}

/// A cleanup whose blocking work was still in flight when the caller stopped waiting; residue may exist, so the run reports a cleanup failure rather than claiming the files are gone. commentlint: allow(JUDGE)
pub(crate) fn cleanup_unproven() -> CleanupFailure {
    CleanupFailure {
        kind: io::ErrorKind::TimedOut,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SetupAbort {
    Cancelled,
    DeadlineExpired,
}

/// Races one pre-spawn setup step against cancellation and the setup deadline so a stalled filesystem cannot wedge cancel, delete, or shutdown before a child exists. commentlint: allow(JUDGE)
/// The caller keeps the pinned step and decides whether to grace-wait it for accurate residue reporting. commentlint: allow(JUDGE)
pub(crate) async fn race_setup<F: std::future::Future + Unpin>(
    cancel: &CancellationToken,
    deadline: tokio::time::Instant,
    step: &mut F,
) -> Result<F::Output, SetupAbort> {
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(SetupAbort::Cancelled),
        () = tokio::time::sleep_until(deadline) => Err(SetupAbort::DeadlineExpired),
        value = step => Ok(value),
    }
}

/// Terminal for a run stopped during pre-spawn setup; no child existed, so there is no teardown question. commentlint: allow(JUDGE)
pub(crate) fn setup_aborted_terminal(harness: Harness, abort: SetupAbort) -> BackendTerminal {
    let name = harness.as_str();
    let message = match abort {
        SetupAbort::Cancelled => format!("{name} backend run was cancelled during setup"),
        SetupAbort::DeadlineExpired => {
            format!("{name} backend setup exhausted the run budget")
        }
    };
    BackendTerminal::Failed(BackendError {
        class: ErrorClass::Transient,
        message,
        retry_after_secs: None,
        provider_code: None,
    })
}

/// This run's sensitive files use a fresh directory forced to `0700` despite the inherited umask.
/// Cleanup failures remain observable because callers handle cleanup explicitly.
/// `Drop` backstops early-return paths with best-effort cleanup.
pub struct PrivateDir {
    path: Option<PathBuf>,
}

impl PrivateDir {
    pub fn create(root: &group_registry::StateRoot, prefix: &str) -> io::Result<Self> {
        use std::os::unix::fs::DirBuilderExt;
        // The crash sweeper removes directories in the run root whose names identify their creator.
        // The crash sweeper removes stale prompt and transcript directories.
        let base = root.run_root()?;
        let owner_boot = group_registry::owner_boot_tag()?;
        let owner_pid = std::process::id();
        let owner_start = group_registry::owner_start_time()?;
        // Requesting `0700` at `mkdir(2)` prevents a permissive umask from exposing the new directory before `set_permissions` runs.
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        for _ in 0..16 {
            let mut nonce = [0u8; 8];
            getrandom::getrandom(&mut nonce)
                .map_err(|_| io::Error::other("temp-dir nonce generation failed"))?;
            let candidate = base.join(format!(
                "{prefix}-{owner_boot}-{owner_pid}-{owner_start}-{:016x}",
                u64::from_le_bytes(nonce)
            ));
            // `create` rejects existing entries, including symlinks, so success uses a previously absent pathname.
            match builder.create(&candidate) {
                Ok(()) => {
                    // `mkdir(2)` modes are umask-filtered, so `set_permissions` forces `0700`.
                    fs::set_permissions(&candidate, fs::Permissions::from_mode(0o700))?;
                    let meta = fs::symlink_metadata(&candidate)?;
                    if !meta.file_type().is_dir() || meta.file_type().is_symlink() {
                        return Err(io::Error::other("private temp path is not a fresh dir"));
                    }
                    return Ok(Self {
                        path: Some(candidate),
                    });
                }
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
                Err(err) => return Err(err),
            }
        }
        Err(io::Error::other("could not allocate a fresh private dir"))
    }

    pub fn path(&self) -> &Path {
        self.path.as_deref().expect("live until cleanup")
    }

    /// `create` walks the registry tree and reads `/proc`, so async callers run it on the blocking pool.
    pub async fn create_async(
        root: group_registry::StateRoot,
        prefix: &'static str,
    ) -> io::Result<Self> {
        off_runtime(move || Self::create(&root, prefix)).await?
    }

    /// Creates `name` as a fresh regular file forced to `0600` regardless of the inherited umask.
    pub fn write_private(&self, name: &str, bytes: &[u8]) -> io::Result<PathBuf> {
        Self::write_private_at(self.path(), name, bytes)
    }

    /// The managed data directory can sit on a slow filesystem, so async callers run the open/write/chmod sequence on the blocking pool.
    pub async fn write_private_async(&self, name: String, bytes: Vec<u8>) -> io::Result<PathBuf> {
        let dir = self.path().to_path_buf();
        off_runtime(move || Self::write_private_at(&dir, &name, &bytes)).await?
    }

    fn write_private_at(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<PathBuf> {
        use std::io::Write;
        let path = dir.join(name);
        // `create_new` rejects existing entries and symlinks, so success uses a previously absent pathname.
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        file.write_all(bytes)?;
        // No `fsync`: the child reads this file before cleanup.
        drop(file);
        // The `open(2)` mode is umask-filtered, so `set_permissions` forces `0600`.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        let meta = fs::symlink_metadata(&path)?;
        if !meta.file_type().is_file() || meta.file_type().is_symlink() {
            return Err(io::Error::other("private temp file is not a fresh file"));
        }
        Ok(path)
    }

    /// `merge_cleanup` propagates cleanup failures to the run terminal.
    pub fn cleanup(mut self) -> Result<(), CleanupFailure> {
        let path = self.path.take().expect("cleanup runs once");
        fs::remove_dir_all(&path).map_err(|err| CleanupFailure { kind: err.kind() })
    }

    /// The child controls the size of the tree under `HOME`, so the recursive delete runs on the blocking pool.
    /// A failed blocking task still reports a cleanup failure so no path can claim the private files are gone without proof.
    pub async fn cleanup_async(self) -> Result<(), CleanupFailure> {
        match tokio::task::spawn_blocking(move || self.cleanup()).await {
            Ok(result) => result,
            Err(_) => Err(CleanupFailure {
                kind: io::ErrorKind::Other,
            }),
        }
    }
}

impl Drop for PrivateDir {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = fs::remove_dir_all(path);
        }
    }
}

/// A cleanup failure converts a completed run to a failure because sensitive files may remain.
pub fn merge_cleanup(
    terminal: BackendTerminal,
    cleanup: Result<(), CleanupFailure>,
) -> BackendTerminal {
    let Err(failure) = cleanup else {
        return terminal;
    };
    match terminal {
        BackendTerminal::Completed { finish_reason } => BackendTerminal::Failed(BackendError {
            class: ErrorClass::Transient,
            message: format!(
                "run completed ({}) but {failure}; success withheld",
                finish_reason.as_wire_str()
            ),
            retry_after_secs: None,
            provider_code: None,
        }),
        BackendTerminal::Failed(mut error) => {
            error.message = format!("{}; additionally {failure}", error.message);
            BackendTerminal::Failed(error)
        }
        // `merge_cleanup` must not downgrade an unresolved teardown failure.
        // Cancel and delete must not claim work stopped while teardown remains unresolved.
        BackendTerminal::FailedUnresolved(mut error) => {
            error.message = format!("{}; additionally {failure}", error.message);
            BackendTerminal::FailedUnresolved(error)
        }
    }
}

/// `terminal_reports_cleanup_residue` is the settlement-side discriminator for private run files left on disk.
/// The supervisor latches it before a committed cancellation terminal replaces the backend terminal that carries the failure text.
/// `Completed` never carries residue because `merge_cleanup` converts a completed run with a cleanup failure to `Failed`.
pub(crate) fn terminal_reports_cleanup_residue(terminal: &BackendTerminal) -> bool {
    terminal_message(terminal).is_some_and(|message| message.contains(CLEANUP_FAILURE_MARKER))
}

/// `terminal_reports_record_residue` is the settlement-side discriminator for a retained crash-ownership record.
/// A retained record outlives the run, so settlement must observe it even when a cancellation terminal replaces the backend terminal.
pub(crate) fn terminal_reports_record_residue(terminal: &BackendTerminal) -> bool {
    terminal_message(terminal).is_some_and(|message| message.contains(RECORD_RETAINED_MARKER))
}

fn terminal_message(terminal: &BackendTerminal) -> Option<&str> {
    match terminal {
        BackendTerminal::Completed { .. } => None,
        BackendTerminal::Failed(error) | BackendTerminal::FailedUnresolved(error) => {
            Some(&error.message)
        }
    }
}

/// `RegistrationTeardownUnproven` marks a spawn error whose pre-prompt process group was not proven torn down.
/// Crash-ownership registration failure leaves no registry record for a successor to sweep.
/// The marker requires a missing registry record and a SIGKILLed group not confirmed gone within the grace period.
/// Cancel, delete, and shutdown latch `work_unresolved` for `BackendTerminal::FailedUnresolved`.
/// `work_unresolved` prevents shutdown from claiming an unproven teardown.
#[derive(Debug)]
struct RegistrationTeardownUnproven;

impl std::fmt::Display for RegistrationTeardownUnproven {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "crash-ownership registration failed and the process group \
             could not be confirmed stopped"
        )
    }
}

impl std::error::Error for RegistrationTeardownUnproven {}

/// Crash-ownership registration failed and the child aborted before exec, so no prompt bytes or provider work exist and a later attempt can succeed; `spawn_failure` classifies this transient. commentlint: allow(JUDGE)
#[derive(Debug)]
struct RegistrationFailed;

impl std::fmt::Display for RegistrationFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "crash-ownership registration failed before the child could start"
        )
    }
}

impl std::error::Error for RegistrationFailed {}

/// The spawner's bounded queue is full behind a stalled harness start; no child was forked for this run, so a later attempt can succeed and `spawn_failure` classifies this transient. commentlint: allow(JUDGE)
#[derive(Debug)]
struct SpawnerBacklogged;

impl std::fmt::Display for SpawnerBacklogged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the spawn queue is full behind a stalled harness start")
    }
}

impl std::error::Error for SpawnerBacklogged {}

/// Spawn failed after publishing a crash-ownership record whose removal also failed.
/// Its `Display` carries [`RECORD_RETAINED_MARKER`] so settlement latches the retained record.
#[derive(Debug)]
struct SpawnRecordRetained {
    kind: io::ErrorKind,
}

impl std::fmt::Display for SpawnRecordRetained {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "run could not start ({}); additionally the backend {RECORD_RETAINED_MARKER}",
            self.kind
        )
    }
}

impl std::error::Error for SpawnRecordRetained {}

/// A shared wall-clock budget can elapse during setup, before a child exists to time out.
/// A later attempt can still fit the budget, so the class is transient.
pub(crate) fn budget_exhausted_failure(harness: Harness) -> BackendTerminal {
    BackendTerminal::Failed(BackendError {
        class: ErrorClass::Transient,
        message: format!(
            "{} backend exhausted its run budget before the child started",
            harness.as_str()
        ),
        retry_after_secs: None,
        provider_code: None,
    })
}

pub(crate) fn spawn_failure(harness: Harness, err: &io::Error) -> BackendTerminal {
    if err
        .get_ref()
        .is_some_and(|inner| inner.is::<RegistrationTeardownUnproven>())
    {
        return BackendTerminal::FailedUnresolved(BackendError {
            class: ErrorClass::Transient,
            message: format!(
                "{} backend crash-ownership registration failed; the \
                 process group could not be confirmed stopped",
                harness.as_str()
            ),
            retry_after_secs: None,
            provider_code: None,
        });
    }
    if err
        .get_ref()
        .is_some_and(|inner| inner.is::<RegistrationFailed>())
    {
        return BackendTerminal::Failed(BackendError {
            class: ErrorClass::Transient,
            message: format!(
                "{} backend crash-ownership registration failed before \
                 the child could start; the run may be retried",
                harness.as_str()
            ),
            retry_after_secs: None,
            provider_code: None,
        });
    }
    if err
        .get_ref()
        .is_some_and(|inner| inner.is::<SpawnerBacklogged>())
    {
        return BackendTerminal::Failed(BackendError {
            class: ErrorClass::Transient,
            message: format!(
                "{} backend spawn queue is full behind a stalled harness \
                 start; the run may be retried",
                harness.as_str()
            ),
            retry_after_secs: None,
            provider_code: None,
        });
    }
    if let Some(retained) = err
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<SpawnRecordRetained>())
    {
        return BackendTerminal::Failed(BackendError {
            class: spawn_error_class(retained.kind),
            message: format!(
                "{} backend run could not start ({}); additionally the {} backend {RECORD_RETAINED_MARKER}",
                harness.as_str(),
                retained.kind,
                harness.as_str(),
            ),
            retry_after_secs: None,
            provider_code: None,
        });
    }
    BackendTerminal::Failed(BackendError {
        class: spawn_error_class(err.kind()),
        message: format!(
            "{} backend run could not start ({})",
            harness.as_str(),
            err.kind()
        ),
        retry_after_secs: None,
        provider_code: None,
    })
}

fn spawn_error_class(kind: io::ErrorKind) -> ErrorClass {
    match kind {
        io::ErrorKind::WouldBlock | io::ErrorKind::OutOfMemory => ErrorClass::Transient,
        _ => ErrorClass::Permanent,
    }
}

pub(crate) fn credential_failure(harness: Harness, error: CredentialRowError) -> BackendTerminal {
    harness_unavailable_failure(harness, error.subreason())
}

pub(crate) fn harness_unavailable_failure(
    harness: Harness,
    reason: &'static str,
) -> BackendTerminal {
    BackendTerminal::Failed(BackendError {
        class: ErrorClass::Permanent,
        message: format!("{} harness_unavailable: {}", harness.as_str(), reason),
        retry_after_secs: None,
        provider_code: None,
    })
}

/// The parser returns `None` for ends it should interpret from the transcript.
/// Classification may use bounded stderr for the error class but never quote child output.
pub(crate) fn abnormal_end_terminal(
    harness: Harness,
    end: SubprocessEnd,
    stderr: &[u8],
    limits: &SubprocessLimits,
) -> Option<BackendTerminal> {
    let name = harness.as_str();
    let error = match end {
        SubprocessEnd::Exited(0) | SubprocessEnd::DrainKilled => return None,
        SubprocessEnd::Exited(code) => {
            let stderr_text = String::from_utf8_lossy(stderr);
            BackendError {
                class: classify_failure_text(&stderr_text),
                message: format!("{name} backend exited with status {code}"),
                retry_after_secs: retry_after_secs_in_text(&stderr_text),
                provider_code: None,
            }
        }
        SubprocessEnd::Signaled => BackendError {
            class: ErrorClass::Permanent,
            message: format!("{name} backend was terminated by a signal"),
            retry_after_secs: None,
            provider_code: None,
        },
        SubprocessEnd::TimedOut => BackendError {
            class: ErrorClass::Transient,
            message: format!(
                "{name} backend timed out after {}s",
                limits.run_timeout.as_secs()
            ),
            retry_after_secs: None,
            provider_code: None,
        },
        SubprocessEnd::Cancelled => BackendError {
            class: ErrorClass::Transient,
            message: format!("{name} backend run was cancelled"),
            retry_after_secs: None,
            provider_code: None,
        },
        SubprocessEnd::StdoutOverflow | SubprocessEnd::StderrOverflow => BackendError {
            class: ErrorClass::Permanent,
            message: format!("{name} backend exceeded its bounded output limit"),
            retry_after_secs: None,
            provider_code: None,
        },
        // A pipe I/O failure does not classify the request, so a retry can succeed.
        SubprocessEnd::CaptureFailed => BackendError {
            class: ErrorClass::Transient,
            message: format!("{name} backend output capture failed"),
            retry_after_secs: None,
            provider_code: None,
        },
        // An unreapable exit status does not classify the request either.
        SubprocessEnd::ExitUnknown => BackendError {
            class: ErrorClass::Transient,
            message: format!("{name} backend exit status could not be determined"),
            retry_after_secs: None,
            provider_code: None,
        },
        // Signal denial is transient because a later run may be permitted.
        // The terminal must report that the work did not settle.
        // The supervisor must not treat `BackendTerminal::FailedUnresolved` as proof that the work stopped.
        SubprocessEnd::TeardownUnconfirmed => {
            return Some(BackendTerminal::FailedUnresolved(BackendError {
                class: ErrorClass::Transient,
                message: format!("{name} backend process group teardown was not confirmed"),
                retry_after_secs: None,
                provider_code: None,
            }));
        }
    };
    Some(BackendTerminal::Failed(error))
}

/// A bounded parse failure records structural position, never line content.
pub(crate) fn parse_failure(harness: Harness, detail: &str) -> BackendTerminal {
    BackendTerminal::Failed(BackendError {
        class: ErrorClass::Permanent,
        message: format!("{} backend output rejected: {detail}", harness.as_str()),
        retry_after_secs: None,
        provider_code: None,
    })
}

/// Authentication and context-overflow classes are checked first because their phrasing can also mention retries.
pub(crate) fn classify_failure_text(text: &str) -> ErrorClass {
    let lower = text.to_ascii_lowercase();
    const AUTH: [&str; 5] = [
        "api key",
        "unauthorized",
        "authentication",
        "credential",
        "forbidden",
    ];
    const AUTH_CODES: [&str; 2] = ["401", "403"];
    const OVERFLOW: [&str; 20] = [
        "context length",
        "context window",
        "prompt is too long",
        "prompt too long",
        "maximum context",
        "context_length",
        "context length exceeded",
        "input is too long",
        "input token count",
        "maximum prompt length is",
        "reduce the length of the messages",
        "maximum model length is",
        "exceeds the limit of",
        "exceeds the available context size",
        "greater than the context length",
        "exceeded model token limit",
        "request entity too large",
        "too large for model with",
        "model_context_window_exceeded",
        "context size has been exceeded",
    ];
    const TRANSIENT: [&str; 8] = [
        "rate limit",
        "rate_limit",
        "overloaded",
        "timeout",
        "timed out",
        "temporarily",
        "try again",
        "unavailable",
    ];
    const TRANSIENT_CODES: [&str; 3] = ["429", "503", "529"];
    if AUTH.iter().any(|needle| lower.contains(needle))
        || AUTH_CODES
            .iter()
            .any(|code| contains_status_code(&lower, code))
    {
        return ErrorClass::AuthRequired;
    }
    if OVERFLOW.iter().any(|needle| lower.contains(needle)) {
        return ErrorClass::ContextOverflow;
    }
    if TRANSIENT.iter().any(|needle| lower.contains(needle))
        || TRANSIENT_CODES
            .iter()
            .any(|code| contains_status_code(&lower, code))
    {
        return ErrorClass::Transient;
    }
    ErrorClass::Permanent
}

/// A substring match misreads `401` in `req-40123` or `req-x401abc`.
/// Boundaries are non-alphanumeric on both sides, so `status 401`, `(401)`, and `401:` match while `x401abc` does not.
fn contains_status_code(haystack: &str, code: &str) -> bool {
    haystack.match_indices(code).any(|(index, _)| {
        let before = haystack[..index].chars().next_back();
        let after = haystack[index + code.len()..].chars().next();
        before.is_none_or(|c| !c.is_ascii_alphanumeric())
            && after.is_none_or(|c| !c.is_ascii_alphanumeric())
    })
}

/// The parser caps retry delays from untrusted provider text.
pub(crate) const MAX_RETRY_AFTER_SECS: u64 = 3600;

/// The parser extracts only explicit retry delays from provider failure text and clamps them to [`MAX_RETRY_AFTER_SECS`].
///
pub(crate) fn retry_after_secs_in_text(text: &str) -> Option<u64> {
    let lower = text.to_ascii_lowercase();
    let mut saw_verb = false;
    let mut saw_delay_keyword = false;
    let mut pending: Option<u64> = None;
    let tokens = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty());
    for token in tokens {
        // After an explicit delay, an unrecognized following token leaves the delay in seconds.
        if let Some(value) = pending {
            return Some(apply_delay_unit(value, token));
        }
        if saw_delay_keyword {
            saw_delay_keyword = false;
            if let Some((value, unit)) = split_delay_number(token) {
                if unit.is_empty() {
                    pending = Some(value);
                    continue;
                }
                return Some(apply_delay_unit(value, unit));
            }
        }
        if saw_verb {
            saw_verb = false;
            if matches!(token, "after" | "in") {
                saw_delay_keyword = true;
                continue;
            }
        }
        match token {
            // Exact matching prevents `retrieval after 300 items` from being parsed as a retry delay.
            "retry" | "retries" | "retried" | "retrying" => saw_verb = true,
            // `retryAfter` lowercases to `retryafter`, so it is treated as an explicit retry-delay marker.
            "retryafter" => saw_delay_keyword = true,
            _ => {}
        }
    }
    pending.map(|value| value.min(MAX_RETRY_AFTER_SECS))
}

/// A digit run that overflows `u64` clamps to `MAX_RETRY_AFTER_SECS` rather than discarding the retry signal.
fn split_delay_number(token: &str) -> Option<(u64, &str)> {
    let digits_end = token
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(token.len());
    if digits_end == 0 {
        return None;
    }
    let value = token[..digits_end].parse().unwrap_or(MAX_RETRY_AFTER_SECS);
    Some((value, &token[digits_end..]))
}

fn apply_delay_unit(value: u64, unit: &str) -> u64 {
    let secs = match unit {
        // A bare number after an explicit delay keyword means seconds.
        "" | "s" | "sec" | "secs" | "second" | "seconds" => value,
        "ms" | "millisecond" | "milliseconds" => value.div_ceil(1000).max(1),
        "m" | "min" | "mins" | "minute" | "minutes" => value.saturating_mul(60),
        "h" | "hr" | "hrs" | "hour" | "hours" => value.saturating_mul(3600),
        // An unrecognized unit after an explicit delay form leaves the number in seconds.
        _ => value,
    };
    secs.min(MAX_RETRY_AFTER_SECS)
}

/// `MAX_LINE_JSON_NODES` caps structural JSON nodes in one harness output line.
///
/// Parsing untyped `serde_json::Value` allocates a node for each array element and object entry, so tiny values can exceed the capture budget's allocation model.
/// Bounding the node count prevents tiny JSON values from amplifying DOM allocation beyond the transcript-sized capture budget.
/// The bound limits node count without limiting line length.
///
pub(crate) const MAX_LINE_JSON_NODES: usize = 32_768;

pub(crate) fn json_nodes_within_bound(text: &str) -> bool {
    let mut nodes = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for byte in text.as_bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            // Each opener or separator admits at most one more node.
            b'{' | b'[' | b',' => {
                nodes += 1;
                if nodes > MAX_LINE_JSON_NODES {
                    return false;
                }
            }
            _ => {}
        }
    }
    true
}

/// The wire admits provider codes only if they match `[A-Za-z0-9_.-]{1,64}`.
/// forwarded.
pub(crate) fn sanitized_provider_code(code: &str) -> Option<String> {
    (!code.is_empty()
        && code.len() <= 64
        && code
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')))
    .then(|| code.to_owned())
}

/// A success terminal and an error terminal cannot both be reported.
pub(crate) fn commit_terminal(
    slot: &mut Option<BackendTerminal>,
    terminal: BackendTerminal,
    line_no: usize,
) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("contradictory terminal at line {line_no}"));
    }
    *slot = Some(terminal);
    Ok(())
}

pub(crate) fn parse_clean_transcript(
    result: &SubprocessResult,
    events: &EventSink,
    parse: impl FnOnce(&[u8]) -> Result<(Vec<BackendEvent>, BackendTerminal), String>,
) -> Result<BackendTerminal, String> {
    if !matches!(
        result.end,
        SubprocessEnd::Exited(0) | SubprocessEnd::DrainKilled
    ) {
        return Err("transcript unavailable".to_owned());
    }
    // Reject transcripts when stdin closes before the whole prompt is delivered; they may answer a truncated prompt.
    if !result.prompt_delivered {
        return Err("prompt delivery failed before the child closed stdin".to_owned());
    }
    let (parsed_events, terminal) = parse(&result.stdout)?;
    for event in parsed_events {
        if events.emit(event) == SinkStatus::Closed {
            break;
        }
    }
    Ok(terminal)
}

/// Abnormal ends take precedence over parsed terminals and parse failures.
/// `finalize` merges cleanup last so cleanup failures are observable on every path.
pub(crate) fn finalize(
    harness: Harness,
    result: &SubprocessResult,
    parsed: Result<BackendTerminal, String>,
    limits: &SubprocessLimits,
    cleanup: Result<(), CleanupFailure>,
) -> BackendTerminal {
    let terminal = match abnormal_end_terminal(harness, result.end, &result.stderr, limits) {
        Some(terminal) => terminal,
        None => match parsed {
            Ok(terminal) => terminal,
            Err(detail) => parse_failure(harness, &detail),
        },
    };
    let terminal = merge_record_retained(harness, terminal, result.record_retained);
    merge_cleanup(terminal, cleanup)
}

/// A retained crash-ownership record withholds success because the run cannot report the coordination state as removed.
/// Removal can succeed later, so the class is transient.
fn merge_record_retained(
    harness: Harness,
    terminal: BackendTerminal,
    retained: bool,
) -> BackendTerminal {
    if !retained {
        return terminal;
    }
    let detail = format!("{} backend {RECORD_RETAINED_MARKER}", harness.as_str());
    match terminal {
        BackendTerminal::Completed { finish_reason } => BackendTerminal::Failed(BackendError {
            class: ErrorClass::Transient,
            message: format!(
                "run completed ({}) but {detail}; success withheld",
                finish_reason.as_wire_str()
            ),
            retry_after_secs: None,
            provider_code: None,
        }),
        BackendTerminal::Failed(mut error) => {
            error.message = format!("{}; additionally {detail}", error.message);
            BackendTerminal::Failed(error)
        }
        // A retained record does not downgrade an unresolved teardown failure.
        BackendTerminal::FailedUnresolved(mut error) => {
            error.message = format!("{}; additionally {detail}", error.message);
            BackendTerminal::FailedUnresolved(error)
        }
    }
}

/// The crash-orphan registry stores one file per live harness process group.
/// replacement host can kill groups a crashed predecessor left behind.
///
/// `pdeathsig` terminates only the group leader; provider and extension descendants can survive it.
/// Descendants can survive the host and continue executing after the leader exits.
/// The host sweeps orphaned groups at startup before answering status.
///
/// The orphan sweep kills an entry's group only when its recording host is dead and the group matches the recorded run.
/// A group matches its recorded run only if its leader's PID, start time, and boot ID match, or the leader is gone while processes remain in its group.
/// The group can empty between the membership check and `kill(-pgid)`.
pub mod group_registry {
    use super::*;

    /// The Linux `/proc/<pid>/stat` field 22 records a process start time in clock ticks since boot.
    /// For `/proc/<pid>/stat`, `Ok(None)` means the PID provably does not exist; `Err` means the answer is unknown.
    /// Callers must not guess on `Err`, because treating the owner or leader as gone can kill a live group or remove its registry entry.
    fn proc_start_time(pid: i32) -> io::Result<Option<u64>> {
        proc_stat_fields(pid).map(|fields| fields.map(|(_, start)| start))
    }

    /// A zombie counts as dead because an unreaped crashed host can retain its PID and start time.
    /// holding runs.
    fn proc_live_start_time(pid: i32) -> io::Result<Option<u64>> {
        Ok(proc_stat_fields(pid)?.and_then(|(state, start)| (state != 'Z').then_some(start)))
    }

    /// `ENOENT` means the PID was gone at `open(2)`; `ESRCH` means the task exited between `open(2)` and `read(2)` of its stat file.
    pub(super) fn pid_vanished(err: &io::Error) -> bool {
        err.kind() == io::ErrorKind::NotFound || err.raw_os_error() == Some(libc::ESRCH)
    }

    /// Parse after the last `)` because `comm` may contain spaces and parentheses.
    fn proc_stat_fields(pid: i32) -> io::Result<Option<(char, u64)>> {
        let stat = match fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => stat,
            Err(err) if pid_vanished(&err) || err.kind() == io::ErrorKind::PermissionDenied => {
                // A recycled PID owned by another user is not this host's process; treat it as gone, as `scan_group_members` does.
                return Ok(None);
            }
            Err(err) => return Err(err),
        };
        let unreadable = || io::Error::other("unreadable /proc stat format");
        let rest = stat.rsplit_once(')').ok_or_else(unreadable)?.1;
        let mut fields = rest.split_ascii_whitespace();
        let state = fields
            .next()
            .and_then(|field| field.chars().next())
            .ok_or_else(unreadable)?;
        let start = fields
            .nth(18)
            .ok_or_else(unreadable)?
            .parse()
            .map_err(|_| unreadable())?;
        Ok(Some((state, start)))
    }

    /// `group_has_members` checks whether any non-zombie process belongs to PGID `pgid`.
    /// `group_has_members` runs only after the recorded leader is gone.
    /// The kernel reserves a PGID while the group has members.
    /// Any non-zombie process found then belongs to the recorded group.
    /// Zombies do not count because they cannot execute work.
    /// A zombie is reaped by its parent or init independently of the sweep.
    /// A scan that cannot complete returns an error rather than “no members” so the sweep neither skips the kill nor deletes the orphan record.
    pub(crate) fn group_has_members(pgid: i32) -> io::Result<bool> {
        scan_group_members(pgid, None)
    }

    /// Exclude the leader because its unreaped zombie retains the PGID but cannot execute work.
    /// The deliberately unreaped zombie's `/proc` entry still names the PGID.
    /// The zombie prevents PGID reuse but cannot execute work.
    /// surviving member.
    pub(crate) fn group_has_other_members(pgid: i32) -> io::Result<bool> {
        scan_group_members(pgid, Some(pgid))
    }

    /// Membership plus identity: a member must share `owner_sid` (descendants never leave the owner's session without also leaving the pgid, because only `setsid` changes a session and it also creates a new group) and must have started at or after the recorded leader. commentlint: allow(JUDGE)
    /// A group that recycled the numeric pgid inside another session fails the session check; one inside the same session with pre-leader members fails the start check. commentlint: allow(JUDGE)
    fn group_has_verified_descendants(
        pgid: i32,
        owner_sid: i32,
        leader_start: u64,
    ) -> io::Result<bool> {
        for proc_entry in fs::read_dir("/proc")? {
            let Some(pid) = proc_entry?
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<i32>().ok())
            else {
                continue;
            };
            match proc_stat_row(pid) {
                Ok(Some(row))
                    if row.pgrp == pgid
                        && row.state != 'Z'
                        && row.state != 'X'
                        && row.sid == owner_sid
                        && row.start >= leader_start =>
                {
                    return Ok(true);
                }
                Ok(_) => {}
                Err(err) if pid_vanished(&err) => {}
                Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {}
                Err(err) => return Err(err),
            }
        }
        Ok(false)
    }

    fn scan_group_members(pgid: i32, exclude_pid: Option<i32>) -> io::Result<bool> {
        for proc_entry in fs::read_dir("/proc")? {
            let Some(pid) = proc_entry?
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<i32>().ok())
            else {
                continue;
            };
            if Some(pid) == exclude_pid {
                continue;
            }
            // A process that exits mid-scan is not a member.
            // Harness descendants run as this user, so a process whose stat is unreadable under `hidepid` is not a member.
            match proc_stat_pgrp_state(pid) {
                Ok(Some((pgrp, state))) if pgrp == pgid && state != 'Z' && state != 'X' => {
                    return Ok(true);
                }
                Ok(_) => {}
                Err(err) if pid_vanished(&err) => {}
                Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {}
                Err(err) => return Err(err),
            }
        }
        Ok(false)
    }

    /// `/proc/<pid>/stat` stores state and process group as the first and third fields after `comm`.
    fn proc_stat_pgrp_state(pid: i32) -> io::Result<Option<(i32, char)>> {
        Ok(proc_stat_row(pid)?.map(|row| (row.pgrp, row.state)))
    }

    struct ProcStatRow {
        state: char,
        pgrp: i32,
        sid: i32,
        start: u64,
    }

    /// After `comm`, `/proc/<pid>/stat` lists state, ppid, pgrp, session, ... and start time as the 20th field. commentlint: allow(JUDGE)
    fn proc_stat_row(pid: i32) -> io::Result<Option<ProcStatRow>> {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
        let unreadable = || io::Error::other("unreadable /proc stat format");
        let rest = stat.rsplit_once(')').ok_or_else(unreadable)?.1;
        let mut fields = rest.split_ascii_whitespace();
        let state = fields
            .next()
            .and_then(|field| field.chars().next())
            .ok_or_else(unreadable)?;
        let pgrp = fields
            .nth(1)
            .ok_or_else(unreadable)?
            .parse()
            .map_err(|_| unreadable())?;
        let sid = fields
            .next()
            .ok_or_else(unreadable)?
            .parse()
            .map_err(|_| unreadable())?;
        // `start` is stat field 22: after state (3), ppid (4), pgrp (5), session (6) are consumed, it is 15 fields further on. commentlint: allow(JUDGE)
        let start = fields
            .nth(15)
            .ok_or_else(unreadable)?
            .parse()
            .map_err(|_| unreadable())?;
        Ok(Some(ProcStatRow {
            state,
            pgrp,
            sid,
            start,
        }))
    }

    /// `boot_id` must not use a placeholder, because one would make existing records appear to come from a different boot.
    /// `sweep` would delete all records without checking or killing their groups if `boot_id` used a placeholder.
    fn boot_id() -> io::Result<String> {
        Ok(fs::read_to_string("/proc/sys/kernel/random/boot_id")?
            .trim()
            .to_owned())
    }

    pub const STATE_DIR_NAME: &str = "broca";

    /// `<managed eidnara dir>/broca`: the crash-ownership registry plus the `runs/` root for per-run private directories.
    ///
    /// The directory is shared by every host incarnation configured with the same data root, so a successor finds its predecessor's records.
    /// The sweep kills groups named by registry files, so no other principal may write the directory or any ancestor; `resolve` enforces that through the hardened traversal.
    /// The daemon passes its configured data directory; deriving the root from `HOME` alone would diverge from a host started with a data-directory override.
    #[derive(Clone, Debug)]
    pub struct StateRoot {
        dir: PathBuf,
    }

    impl StateRoot {
        pub fn resolve(data_dir_override: Option<&Path>) -> io::Result<Self> {
            let dir = crate::instance::managed_dir_path(data_dir_override)
                .map_err(|err| io::Error::other(format!("broca state root: {err}")))?
                .join(STATE_DIR_NAME);
            secure_dir(&dir)?;
            Ok(Self { dir })
        }

        pub fn path(&self) -> &Path {
            &self.dir
        }

        /// Every use re-validates the tree so a directory replaced after startup cannot redirect record writes or sweeps.
        fn registry_dir(&self) -> io::Result<PathBuf> {
            secure_dir(&self.dir)?;
            Ok(self.dir.clone())
        }

        pub fn run_root(&self) -> io::Result<PathBuf> {
            let root = self.registry_dir()?.join("runs");
            secure_dir(&root)?;
            Ok(root)
        }
    }

    /// Callers operate by pathname after the hardened traversal; every ancestor is owned by this user or root and is not writable by others, so another principal cannot redirect the pathname.
    fn secure_dir(dir: &Path) -> io::Result<()> {
        crate::instance::secure_runtime_dir(dir)
            .map(drop)
            .map_err(|err| io::Error::other(format!("group registry dir is not private: {err}")))
    }

    struct Entry {
        boot_id: String,
        leader_pid: i32,
        leader_start: u64,
        owner_pid: i32,
        owner_start: u64,
        /// The leader joins a fresh process group but keeps the owner's session, so every descendant shares this session id; it is the identity proof for a group whose leader is gone. commentlint: allow(JUDGE)
        owner_sid: i32,
    }

    impl Entry {
        fn parse(text: &str) -> Option<Self> {
            let mut lines = text.lines();
            if lines.next()? != "v2" {
                return None;
            }
            let boot_id = lines.next()?.to_owned();
            let pid_line = |lines: &mut std::str::Lines| -> Option<(i32, u64)> {
                let (pid, start) = lines.next()?.split_once(' ')?;
                Some((pid.parse().ok()?, start.parse().ok()?))
            };
            let (leader_pid, leader_start) = pid_line(&mut lines)?;
            let (owner_pid, owner_start) = pid_line(&mut lines)?;
            let owner_sid = lines.next()?.parse().ok()?;
            Some(Self {
                boot_id,
                leader_pid,
                leader_start,
                owner_pid,
                owner_start,
                owner_sid,
            })
        }
    }

    /// The holder calls [`GroupRecord::remove`] once the group is proven gone; a record that is
    /// merely dropped stays on disk so a later host sweep can still find surviving descendants.
    pub struct GroupRecord {
        path: PathBuf,
    }

    impl GroupRecord {
        /// `record` returns `None` if it cannot write the record or establish either process identity.
        /// The registrar then withholds the exec barrier, so the child aborts before executing harness code.
        pub fn record(root: &StateRoot, leader_pid: i32) -> Option<Self> {
            let leader_start = proc_start_time(leader_pid).ok().flatten()?;
            let owner_pid = std::process::id() as i32;
            let owner_start = proc_start_time(owner_pid).ok().flatten()?;
            let owner_sid = rustix::process::getsid(None).ok()?.as_raw_nonzero().get();
            let dir = root.registry_dir().ok()?;
            let mut nonce = [0u8; 8];
            getrandom::getrandom(&mut nonce).ok()?;
            let name = format!("{leader_pid}-{:016x}", u64::from_le_bytes(nonce));
            let path = dir.join(&name);
            let body = format!(
                "v2\n{}\n{leader_pid} {leader_start}\n{owner_pid} {owner_start}\n{owner_sid}\n",
                boot_id().ok()?
            );
            // A record is visible only once it is complete: a host crash between create and write must not leave a truncated record that the sweep discards without signaling its group.
            let temp = dir.join(format!(".{name}.tmp"));
            let written = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temp)
                .and_then(|mut file| {
                    use std::io::Write;
                    // The `open(2)` mode is umask-filtered; `fchmod` through the descriptor forces `0600` so a successor daemon can read the record after a crash.
                    file.set_permissions(fs::Permissions::from_mode(0o600))?;
                    file.write_all(body.as_bytes())
                })
                .and_then(|()| fs::rename(&temp, &path));
            if written.is_err() {
                let _ = fs::remove_file(&temp);
                return None;
            }
            Some(Self { path })
        }
    }

    impl GroupRecord {
        /// Remove the record after the group is proven to have exited.
        /// A retained record fails a successor's mandatory startup sweep, so the caller reports the failure rather than claiming the coordination state is gone.
        /// `NotFound` counts as removed.
        pub fn remove(self) -> io::Result<()> {
            match fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(err) => Err(err),
            }
        }
    }

    /// `sweep` kills groups recorded by dead host incarnations and removes their entries.
    /// `sweep` leaves entries owned by live hosts untouched, so concurrent hosts do not sweep each other's runs.
    ///
    /// `sweep` propagates unreadable-registry, unreadable-record, and indeterminate `/proc` lookup errors without removing the record.
    /// Treating an unknown `/proc` result as "no orphan" can refire a run while its descendant executes.
    pub fn sweep_orphaned_groups(root: &StateRoot) -> io::Result<usize> {
        let dir = root.registry_dir()?;
        let current_boot = boot_id()?;
        let mut killed = 0;
        for file in fs::read_dir(&dir)? {
            let path = file?.path();
            // Only regular files are registry records; `sweep_orphaned_run_dirs` sweeps `runs/`.
            if !path.is_file() {
                continue;
            }
            // Hosts sharing one state root sweep concurrently with each other's record writes. commentlint: allow(JUDGE)
            // Deleting a dot-prefixed temp younger than `UNPUBLISHED_TEMP_GRACE` could unlink an in-flight write, causing its publishing rename to fail with `ENOENT`.
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with('.'))
            {
                let stale = fs::symlink_metadata(&path)
                    .and_then(|meta| meta.modified())
                    .map(|modified| {
                        std::time::SystemTime::now()
                            .duration_since(modified)
                            .is_ok_and(|age| age >= UNPUBLISHED_TEMP_GRACE)
                    })
                    .unwrap_or(false);
                if stale {
                    remove_swept_record(&path)?;
                }
                continue;
            }
            // A record names a kill target, so the sweep parses only a pinned regular file owned by this user.
            // A planted foreign-owned file or symlink fails the sweep closed instead of driving `kill_process_group`.
            let text = {
                use std::os::unix::fs::MetadataExt;
                let mut file = match fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                    .open(&path)
                {
                    Ok(file) => file,
                    // `NotFound` means another process removed the record after directory enumeration.
                    Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                    Err(err) => return Err(err),
                };
                let meta = file.metadata()?;
                if !meta.is_file() || meta.uid() != rustix::process::geteuid().as_raw() {
                    return Err(io::Error::other(
                        "a registry record is not a regular file owned by this user",
                    ));
                }
                let mut text = String::new();
                std::io::Read::read_to_string(&mut file, &mut text)?;
                text
            };
            // A record that does not parse cannot identify a group.
            let Some(entry) = Entry::parse(&text) else {
                remove_swept_record(&path)?;
                continue;
            };
            // A record from a different boot must be removed without signaling because its PIDs may be reused.
            if entry.boot_id != current_boot {
                remove_swept_record(&path)?;
                continue;
            }
            if proc_live_start_time(entry.owner_pid)? == Some(entry.owner_start) {
                continue;
            }
            let group_live = match proc_start_time(entry.leader_pid)? {
                // Matching `leader_pid` and `leader_start` identifies the recorded leader.
                Some(start) if start == entry.leader_start => true,
                // A reused leader PID proves the recorded group is gone: a PID used as a PGID cannot be reallocated while group members remain.
                // provably gone.
                Some(_) => false,
                // The leader was reaped (pdeathsig kills only the leader), so the numeric pgid alone
                // is no proof: it can be recycled into an unrelated same-UID group whose own leader
                // has also exited. A surviving member counts as this run's descendant only when it
                // shares the recording owner's session and started no earlier than the recorded
                // leader. commentlint: allow(JUDGE)
                None => group_has_verified_descendants(
                    entry.leader_pid,
                    entry.owner_sid,
                    entry.leader_start,
                )?,
            };
            if group_live {
                if let Some(group) = rustix::process::Pid::from_raw(entry.leader_pid) {
                    match rustix::process::kill_process_group(group, rustix::process::Signal::KILL)
                    {
                        Ok(()) => killed += 1,
                        // `SRCH` means the group exited after membership verification; treat it as resolved rather than failing startup.
                        Err(rustix::io::Errno::SRCH) => {}
                        Err(err) => return Err(err.into()),
                    }
                }
                // The caller keeps the record after `SIGKILL` until membership drains, because surviving members could otherwise cause recovery to refire the run beside them.
                if !wait_group_empty_blocking(entry.leader_pid, SWEEP_MEMBER_GRACE)? {
                    return Err(io::Error::other(
                        "a swept group's members could not be confirmed stopped",
                    ));
                }
            }
            remove_swept_record(&path)?;
        }
        Ok(killed)
    }

    /// SIGKILL cannot be caught; members in uninterruptible kernel state can delay group removal.
    /// `SWEEP_MEMBER_GRACE` bounds the wait for uninterruptible members before startup fails closed.
    const SWEEP_MEMBER_GRACE: Duration = Duration::from_secs(5);

    /// The sweep deletes dot-prefixed record temps only after `UNPUBLISHED_TEMP_GRACE`; newer
    /// files may belong to a concurrent host's active write. A skipped young temp is
    /// reconsidered by the next startup sweep.
    const UNPUBLISHED_TEMP_GRACE: Duration = Duration::from_secs(600);

    /// The startup sweep runs before request work, so `wait_group_empty_blocking` may block until `budget` elapses.
    /// The startup sweep runs before request work, so blocking cannot starve requests.
    /// `group_has_members` errors propagate so an unverifiable scan never reads as empty.
    fn wait_group_empty_blocking(pgid: i32, budget: Duration) -> io::Result<bool> {
        let deadline = std::time::Instant::now() + budget;
        loop {
            if !group_has_members(pgid)? {
                return Ok(true);
            }
            if std::time::Instant::now() >= deadline {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// `remove_swept_record` treats `NotFound` as success because the record is already absent.
    /// Any other removal error prevents the sweep from proving that the record was removed.
    fn remove_swept_record(path: &Path) -> io::Result<()> {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
        }
    }

    /// `owner_boot_tag` retains the first 16 alphanumeric characters of the boot ID for filename-safe run-directory names.
    pub(crate) fn owner_boot_tag() -> io::Result<String> {
        Ok(boot_id()?
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(16)
            .collect())
    }

    pub(crate) fn owner_start_time() -> io::Result<u64> {
        let owner_pid = std::process::id() as i32;
        proc_start_time(owner_pid)?.ok_or_else(|| io::Error::other("this process has no stat"))
    }

    /// `sweep_orphaned_run_dirs` removes a directory only after proving that its recorded owner is gone.
    /// (R17/R19).
    ///
    /// The sweep uses the recorded PID and start time to avoid deleting a live owner's directory.
    /// The sweep leaves unverifiable directories in place because deleting a live run's private files would break that run.
    /// disk cost.
    pub fn sweep_orphaned_run_dirs(state: &StateRoot) -> io::Result<usize> {
        let root = state.run_root()?;
        let current_boot = owner_boot_tag()?;
        let mut removed = 0;
        for entry in fs::read_dir(&root)? {
            let path = entry?.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let mut fields = name.rsplitn(5, '-');
            let (Some(_nonce), Some(start), Some(pid), Some(boot)) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            let (Ok(pid), Ok(start)) = (pid.parse::<i32>(), start.parse::<u64>()) else {
                continue;
            };
            if boot != current_boot {
                remove_run_dir(&path, &mut removed)?;
                continue;
            }
            // Treat zombie owners as dead because they cannot use these files.
            if proc_live_start_time(pid)? == Some(start) {
                continue;
            }
            remove_run_dir(&path, &mut removed)?;
        }
        Ok(removed)
    }

    fn remove_run_dir(path: &Path, removed: &mut usize) -> io::Result<()> {
        match fs::remove_dir_all(path) {
            Ok(()) => {
                *removed += 1;
                Ok(())
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::EnvSnapshot;

    /// A task that exits between `open` and `read` of its `/proc` stat fails with `ESRCH`, not `ENOENT`; both prove the PID is gone.
    #[test]
    fn vanished_pid_errors_are_not_scan_failures() {
        use super::group_registry::pid_vanished;
        assert!(pid_vanished(&std::io::Error::from_raw_os_error(
            libc::ENOENT
        )));
        assert!(pid_vanished(&std::io::Error::from_raw_os_error(
            libc::ESRCH
        )));
        assert!(!pid_vanished(&std::io::Error::from_raw_os_error(
            libc::EACCES
        )));
        assert!(!pid_vanished(&std::io::Error::from_raw_os_error(libc::EIO)));
    }

    /// The vector was produced outside this crate from the documented derivation, so a change to
    /// the domain separator, the canonicalization id, or the row layout fails here.
    #[test]
    fn credential_fingerprint_matches_the_committed_vector() {
        let key = std::array::from_fn(|index| index as u8);
        let snapshot = EnvSnapshot::capture_from(vec![(
            OsString::from("ANTHROPIC_API_KEY"),
            OsString::from("secret"),
        )])
        .expect("vector snapshot");
        assert_eq!(
            snapshot
                .credential_fingerprint(&key, "opencode", "anthropic")
                .expect("fingerprint"),
            "ecac831b94bb1d9e972ee993f7798c9ff7c6133b545e489ac1a3f60448127e80"
        );
        // A different connection key over the same row must not collide.
        assert_ne!(
            snapshot
                .credential_fingerprint(&[0u8; 32], "opencode", "anthropic")
                .expect("fingerprint"),
            "ecac831b94bb1d9e972ee993f7798c9ff7c6133b545e489ac1a3f60448127e80"
        );
    }
}
