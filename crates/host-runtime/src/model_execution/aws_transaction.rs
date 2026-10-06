//! One explicitly owned credential transaction from owner files to a settled result.
//!
//! The transaction captures the owner's config, credentials, and SSO token through
//! no-follow descriptor walks, admits the selected graph, writes the chosen token into
//! a private scratch `HOME`, runs the host executable's helper mode under fixed resource
//! limits, and returns only after the helper's process group is gone and the scratch
//! directory is removed. The owner's cache is never written. A changed private
//! successor is returned independently of the role row, and both are discarded when
//! the owner's token observation changed during the transaction.

use std::ffi::OsString;
use std::io::Read;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rustix::fs::{Mode, OFlags, openat};
use rustix::process::Resource;
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use super::aws_helper::{self, HelperFailure, HelperReport, HelperRequest, Renewal};
use super::aws_profile::{self, AdmissionError, CapturedProfileInput, GraphIdentity, RootIdentity};
use super::subprocess::group_registry::StateRoot;
use super::subprocess::{self, PrivateDir, SubprocessEnd, SubprocessLimits, SubprocessSpec};
use crate::instance::{
    HARDENED_DIR_FLAGS, S_IFMT, S_IFREG, hex, is_owner_only_dir, is_safe_ancestor, mode_bits,
    normal_components, open_safe_anchor, owner_uid,
};

/// Upper bound on the captured owner SSO token and on the private successor.
pub const MAX_TOKEN_BYTES: usize = 64 * 1024;
/// Budget for one transaction. It bounds when the helper may start and its wall;
/// blocking reads and teardown run to physical completion even past it.
pub const TRANSACTION_BUDGET: Duration = Duration::from_secs(30);
const MAX_STDERR_BYTES: usize = 4 * 1024;
const GRACE: Duration = Duration::from_secs(1);
const ACCESS_TOKEN: usize = 0;
const EXPIRES_AT: usize = 1;
const REFRESH_FIELDS: [usize; 4] = [2, 3, 4, 5];
const REGISTRATION_EXPIRES_AT: usize = 5;
const REGION: usize = 6;
const START_URL: usize = 7;
const TOKEN_FIELDS: [&str; 8] = [
    "accessToken",
    "expiresAt",
    "refreshToken",
    "clientId",
    "clientSecret",
    "registrationExpiresAt",
    "region",
    "startUrl",
];

/// The owner's selected source, with absolute file paths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnerSource {
    pub profile: String,
    pub region: String,
    pub config_file: PathBuf,
    pub credentials_file: PathBuf,
    pub sso_cache_root: PathBuf,
}

/// Identity of one external observation of the owner's SSO token file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenObservation {
    Absent,
    /// The file's device, inode, size, modification and change times, and content
    /// digest.
    Present {
        identity: [i64; 7],
        sha256: [u8; 32],
    },
    /// The path is unsafe, oversized, changed while read, or unreadable.
    Unusable,
}

/// A validated SSO token held only in host memory, with the external observation it
/// supersedes. It is offered to the helper only while that observation is unchanged.
pub struct PrivateToken {
    token: Zeroizing<Vec<u8>>,
    basis: TokenObservation,
}

/// Helper process limits. [`Default`] holds the fixed production values, which
/// release builds always use.
#[derive(Clone, Debug)]
pub struct HelperLimits {
    pub address_space_bytes: u64,
    pub stack_bytes: u64,
    pub open_files: u64,
    pub cpu_seconds: u64,
    /// Largest file the helper may write; the successor token is at most
    /// [`MAX_TOKEN_BYTES`].
    pub file_size_bytes: u64,
    pub wall: Duration,
}

impl Default for HelperLimits {
    fn default() -> Self {
        Self {
            address_space_bytes: 512 * 1024 * 1024,
            stack_bytes: 16 * 1024 * 1024,
            open_files: 64,
            cpu_seconds: 10,
            file_size_bytes: MAX_TOKEN_BYTES as u64,
            wall: Duration::from_secs(15),
        }
    }
}

/// Everything one transaction reads.
pub struct TransactionInput<'a> {
    pub source: &'a OwnerSource,
    /// The newest private successor from an earlier transaction of this incarnation.
    pub predecessor: Option<&'a PrivateToken>,
    /// The host executable to launch in debug builds; release builds launch
    /// `/proc/self/exe`.
    pub executable: &'a Path,
    pub state_root: &'a StateRoot,
    /// Remaining budget, capped at [`TRANSACTION_BUDGET`].
    pub budget: Duration,
    pub limits: HelperLimits,
    /// Debug builds forward this loopback test origin to the helper.
    pub test_origin: Option<String>,
}

/// One complete role row.
pub struct CredentialRow {
    pub access_key_id: String,
    pub secret_access_key: Zeroizing<String>,
    pub session_token: Zeroizing<String>,
    pub expires_at_unix_seconds: u64,
}

/// What the transaction knows about SSO OIDC renewal. Only a completed helper report
/// proves `NotStarted`; a missing report is `Unknown`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenewalEvidence {
    NotStarted,
    Started,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureFailure {
    Missing,
    /// A symlink, non-regular file, foreign owner, permissive mode, or replaceable
    /// ancestor.
    Unsafe,
    TooLarge,
    /// The file changed while it was read.
    Mutated,
    Io,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransactionFailure {
    Capture(CaptureFailure),
    Admission(AdmissionError),
    /// No valid token is available for the SSO root; an external login is needed.
    LoginRequired,
    /// The owner's token observation changed during the transaction.
    Withdrawn,
    Helper(HelperFailure),
    /// The helper ended without a complete report: timeout, crash, or resource limit.
    HelperUnreported,
    Spawn,
    BudgetExhausted,
    Cancelled,
    /// The helper group or scratch directory was not proven gone; the slot stays
    /// unresolved.
    CleanupUnproven,
}

/// The settled result. `row` and `successor` are independent outputs.
pub struct TransactionOutcome {
    pub identity: Option<GraphIdentity>,
    pub row: Option<CredentialRow>,
    pub successor: Option<PrivateToken>,
    /// The external token observation taken last, before adoption.
    pub observation: Option<TokenObservation>,
    pub renewal: RenewalEvidence,
    /// Renewal may have rotated the token and no valid successor was recovered, so
    /// further renewal needs a changed external login observation.
    pub lost_succession: bool,
    /// The helper group is gone but its crash-ownership record stayed for the next
    /// startup sweep.
    pub record_retained: bool,
    pub failure: Option<TransactionFailure>,
}

impl TransactionOutcome {
    fn failed(failure: TransactionFailure) -> Self {
        Self {
            identity: None,
            row: None,
            successor: None,
            observation: None,
            renewal: RenewalEvidence::NotStarted,
            lost_succession: false,
            record_retained: false,
            failure: Some(failure),
        }
    }
}

/// The single transaction slot. `&mut` admits one transaction at a time. The slot is
/// unresolved while a transaction runs and stays unresolved for the incarnation when
/// cleanup is unproven or the future is dropped before settlement; only a restart and
/// its startup sweep recover it.
#[derive(Debug, Default)]
pub struct TransactionSlot {
    unresolved: bool,
}

impl TransactionSlot {
    pub fn is_unresolved(&self) -> bool {
        self.unresolved
    }

    /// Runs one transaction to physical settlement when polled to completion. Blocking
    /// reads run on the bounded blocking pool and are awaited to completion, and the
    /// helper call returns only after its group is reaped or teardown is reported
    /// unconfirmed.
    pub async fn run(
        &mut self,
        input: TransactionInput<'_>,
        cancel: &CancellationToken,
    ) -> TransactionOutcome {
        if self.unresolved {
            return TransactionOutcome::failed(TransactionFailure::CleanupUnproven);
        }
        self.unresolved = true;
        let outcome = transact(input, cancel).await;
        self.unresolved = outcome.failure == Some(TransactionFailure::CleanupUnproven);
        outcome
    }
}

async fn transact(input: TransactionInput<'_>, cancel: &CancellationToken) -> TransactionOutcome {
    use TransactionFailure as F;
    let deadline = Instant::now() + input.budget.min(TRANSACTION_BUDGET);
    let source = input.source.clone();
    let captured = subprocess::off_runtime(move || capture_files(&source)).await;
    let [config, credentials] = match captured {
        Ok(Ok(files)) => files,
        Ok(Err(failure)) => return TransactionOutcome::failed(F::Capture(failure)),
        Err(_) => return TransactionOutcome::failed(F::Capture(CaptureFailure::Io)),
    };
    let admitted = aws_profile::admit(CapturedProfileInput {
        profile: &input.source.profile,
        region: &input.source.region,
        config: &config,
        credentials: &credentials,
    });
    let graph = match admitted {
        Ok(graph) => graph,
        Err(error) => return TransactionOutcome::failed(F::Admission(error)),
    };
    let mut outcome = TransactionOutcome::failed(F::LoginRequired);
    outcome.identity = Some(graph.identity().clone());
    let sso = match &graph.identity().root {
        RootIdentity::Sso {
            session_name,
            sso_region,
            start_url,
            ..
        } => {
            let file_name = format!("{}.json", hex(&Sha1::digest(session_name)));
            Some(SsoToken {
                path: input.source.sso_cache_root.join(&file_name),
                file_name,
                sso_region: sso_region.clone(),
                start_url: start_url.clone(),
            })
        }
        RootIdentity::Static { .. } => None,
    };
    let mut supplied = None;
    if let Some(sso) = &sso {
        let (observation, owner_token) = sso.observe().await;
        outcome.observation = Some(observation.clone());
        supplied = match input.predecessor {
            Some(private) if same_content(&private.basis, &observation) => {
                sso.canonical(&private.token)
            }
            _ => owner_token.and_then(|bytes| sso.canonical(&bytes)),
        };
        if supplied.is_none() {
            outcome.failure = Some(F::LoginRequired);
            return outcome;
        }
    }
    // The helper wall leaves room for TERM, KILL and reap, and the successor read.
    let helper_wall = deadline
        .saturating_duration_since(Instant::now())
        .saturating_sub(3 * GRACE)
        .min(input.limits.wall);
    if cancel.is_cancelled() || helper_wall.is_zero() {
        outcome.failure = Some(if cancel.is_cancelled() {
            F::Cancelled
        } else {
            F::BudgetExhausted
        });
        return outcome;
    }
    let Ok(dir) = PrivateDir::create_async(input.state_root.clone(), "aws-helper").await else {
        outcome.failure = Some(F::Spawn);
        return outcome;
    };
    let prepared = prepare_home(&dir, sso.as_ref(), supplied.as_deref().map(Vec::as_slice)).await;
    let request = serde_json::to_vec(&HelperRequest::from_graph(&graph)).map(Zeroizing::new);
    let mut launch = match (prepared, request) {
        (Ok(()), Ok(mut request)) => {
            let spec = helper_spec(&input, &dir, std::mem::take(&mut *request));
            let limits = SubprocessLimits {
                run_timeout: helper_wall,
                termination_grace: GRACE,
                drain_grace: GRACE,
                max_stdout_bytes: aws_helper::MAX_REPORT_BYTES,
                max_stderr_bytes: MAX_STDERR_BYTES,
            };
            match subprocess::run(spec, &limits, cancel, None).await {
                Ok(result) => Launch::Ran(result),
                Err(error) => Launch::Failed(subprocess::spawn_error_residue(&error)),
            }
        }
        _ => Launch::Failed((false, false)),
    };
    let (group_gone, report) = match &mut launch {
        Launch::Ran(result) => {
            outcome.record_retained = result.record_retained;
            let stdout = Zeroizing::new(std::mem::take(&mut result.stdout));
            let report = (result.end == SubprocessEnd::Exited(0))
                .then(|| serde_json::from_slice::<HelperReport>(&stdout).ok())
                .flatten();
            (result.end != SubprocessEnd::TeardownUnconfirmed, report)
        }
        Launch::Failed((teardown_unproven, record_retained)) => {
            outcome.record_retained = *record_retained;
            (!*teardown_unproven, None)
        }
    };
    let successor = match &sso {
        Some(sso) if group_gone => {
            let path = dir.path().join(".aws/sso/cache").join(&sso.file_name);
            let read = subprocess::off_runtime(move || read_secure(&path, MAX_TOKEN_BYTES, true));
            let bytes = read.await.ok().and_then(Result::ok).flatten();
            bytes
                .and_then(|captured| sso.canonical(&captured.bytes))
                .filter(|token| Some(token) != supplied.as_ref())
        }
        _ => None,
    };
    let cleaned = dir.cleanup_async().await.is_ok();
    // A helper that never received its whole request cannot have started renewal.
    let delivered = matches!(&launch, Launch::Ran(result) if result.prompt_delivered);
    outcome.renewal = match &report {
        Some(HelperReport::Credentials { renewal, .. } | HelperReport::Failed { renewal, .. }) => {
            match renewal {
                Renewal::NotStarted => RenewalEvidence::NotStarted,
                Renewal::Started => RenewalEvidence::Started,
            }
        }
        None if sso.is_some() && delivered => RenewalEvidence::Unknown,
        None => RenewalEvidence::NotStarted,
    };
    let unproven = outcome.renewal != RenewalEvidence::NotStarted;
    if !(group_gone && cleaned) {
        outcome.failure = Some(F::CleanupUnproven);
        outcome.lost_succession = unproven;
        return outcome;
    }
    if let Some(sso) = &sso {
        let (observation, _) = sso.observe().await;
        let started = outcome.observation.replace(observation.clone());
        if !started.is_some_and(|started| same_content(&started, &observation)) {
            // A new external login supersedes any rotation; a deleted or unusable
            // owner token leaves a possible rotation unrecovered.
            let relogin = matches!(observation, TokenObservation::Present { .. });
            outcome.lost_succession = unproven && !relogin;
            outcome.failure = Some(F::Withdrawn);
            return outcome;
        }
        outcome.successor = successor.map(|token| PrivateToken {
            token,
            basis: observation,
        });
        outcome.lost_succession = unproven && outcome.successor.is_none();
    }
    outcome.failure = match report {
        Some(HelperReport::Credentials {
            access_key_id,
            secret_access_key,
            session_token,
            expires_at_unix_seconds,
            ..
        }) => {
            outcome.row = Some(CredentialRow {
                access_key_id,
                secret_access_key,
                session_token,
                expires_at_unix_seconds,
            });
            None
        }
        Some(HelperReport::Failed { failure, .. }) => Some(F::Helper(failure)),
        None if matches!(launch, Launch::Failed(_)) => Some(F::Spawn),
        None => Some(F::HelperUnreported),
    };
    outcome
}

/// Two observations of the owner's token agree when both hold the same content, or
/// when both are absent or unusable. A metadata-only change, such as `touch` or
/// `chmod`, keeps the observation.
fn same_content(a: &TokenObservation, b: &TokenObservation) -> bool {
    match (a, b) {
        (
            TokenObservation::Present { sha256: a, .. },
            TokenObservation::Present { sha256: b, .. },
        ) => a == b,
        _ => a == b,
    }
}

enum Launch {
    Ran(subprocess::SubprocessResult),
    /// No helper ran; the flags report unproven teardown and a retained crash record.
    Failed((bool, bool)),
}

fn helper_spec(input: &TransactionInput<'_>, dir: &PrivateDir, stdin: Vec<u8>) -> SubprocessSpec {
    let mut env = vec![(OsString::from("HOME"), dir.path().as_os_str().to_owned())];
    if let Some(origin) = input
        .test_origin
        .as_ref()
        .filter(|_| cfg!(debug_assertions))
    {
        env.push((aws_helper::TEST_ORIGIN_ENV.into(), origin.into()));
    }
    // Release builds always launch the running executable with the fixed limits.
    let release = !cfg!(debug_assertions);
    let defaults = HelperLimits::default();
    let limits = if release { &defaults } else { &input.limits };
    let executable = if release {
        Path::new("/proc/self/exe")
    } else {
        input.executable
    };
    SubprocessSpec {
        executable: executable.to_path_buf(),
        args: vec![aws_helper::HELPER_ARG.into()],
        env,
        working_dir: dir.path().to_path_buf(),
        stdin,
        inherit_fds: Vec::new(),
        state_root: input.state_root.clone(),
        rlimits: vec![
            (Resource::As, limits.address_space_bytes),
            (Resource::Stack, limits.stack_bytes),
            (Resource::Nofile, limits.open_files),
            (Resource::Cpu, limits.cpu_seconds),
            (Resource::Fsize, limits.file_size_bytes),
            (Resource::Core, 0),
        ],
    }
}

/// Creates `.aws/sso/cache` under the scratch `HOME` with `0700` directories and
/// writes the supplied token as a fresh `0600` file.
async fn prepare_home(
    dir: &PrivateDir,
    sso: Option<&SsoToken>,
    token: Option<&[u8]>,
) -> std::io::Result<()> {
    let (Some(sso), Some(token)) = (sso, token) else {
        return Ok(());
    };
    let home = dir.path().to_path_buf();
    subprocess::off_runtime(move || {
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        let mut path = home;
        for part in [".aws", "sso", "cache"] {
            path.push(part);
            builder.create(&path)?;
        }
        Ok::<_, std::io::Error>(())
    })
    .await??;
    let name = format!(".aws/sso/cache/{}", sso.file_name);
    dir.write_private_async(name, token.to_vec())
        .await
        .map(drop)
}

struct SsoToken {
    path: PathBuf,
    file_name: String,
    sso_region: String,
    start_url: String,
}

impl SsoToken {
    /// Observes the owner's token file and returns its bytes when it is safe to read.
    async fn observe(&self) -> (TokenObservation, Option<Zeroizing<Vec<u8>>>) {
        let path = self.path.clone();
        match subprocess::off_runtime(move || read_secure(&path, MAX_TOKEN_BYTES, true)).await {
            Ok(Ok(None)) => (TokenObservation::Absent, None),
            Ok(Ok(Some(captured))) => {
                let sha256 = Sha256::digest(&*captured.bytes).into();
                let observation = TokenObservation::Present {
                    identity: captured.identity,
                    sha256,
                };
                (observation, Some(captured.bytes))
            }
            _ => (TokenObservation::Unusable, None),
        }
    }

    /// Validates a cached token and re-emits only its supported fields. Recognized
    /// keys match case-insensitively and may not repeat; the access token and expiry
    /// are required, region and start URL must match the admitted session, and the
    /// refresh fields are all present or all absent.
    fn canonical(&self, bytes: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
        let Entries(entries) = serde_json::from_slice(bytes).ok()?;
        let mut fields: [Option<Zeroizing<String>>; 8] = Default::default();
        for (key, value) in entries {
            let Some(slot) = TOKEN_FIELDS
                .iter()
                .position(|f| f.eq_ignore_ascii_case(&key))
            else {
                continue;
            };
            let serde_json::Value::String(value) = value else {
                return None;
            };
            let valid = !value.is_empty() && value.len() <= aws_helper::MAX_FIELD_BYTES;
            if !valid || fields[slot].replace(Zeroizing::new(value)).is_some() {
                return None;
            }
        }
        let text = |slot: usize| fields[slot].as_deref().map(String::as_str);
        let timestamp = |slot: usize| {
            text(slot).is_none_or(|value| {
                aws_smithy_types::DateTime::from_str(
                    value,
                    aws_smithy_types::date_time::Format::DateTime,
                )
                .is_ok()
            })
        };
        let refresh = REFRESH_FIELDS.map(|slot| fields[slot].is_some());
        let consistent = refresh.iter().all(|p| *p) || refresh.iter().all(|p| !p);
        let valid = text(ACCESS_TOKEN).is_some()
            && text(EXPIRES_AT).is_some()
            && timestamp(EXPIRES_AT)
            && timestamp(REGISTRATION_EXPIRES_AT)
            && consistent
            && text(REGION) == Some(self.sso_region.as_str())
            && text(START_URL) == Some(self.start_url.as_str());
        if !valid {
            return None;
        }
        let object: serde_json::Map<String, serde_json::Value> = TOKEN_FIELDS
            .iter()
            .zip(fields.iter())
            .filter_map(|(key, value)| Some(((*key).to_owned(), value.as_deref()?.clone().into())))
            .collect();
        serde_json::to_vec(&object).ok().map(Zeroizing::new)
    }
}

/// JSON object entries in document order, so repeated keys stay visible.
struct Entries(Vec<(String, serde_json::Value)>);

impl<'de> Deserialize<'de> for Entries {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct EntriesVisitor;
        impl<'de> Visitor<'de> for EntriesVisitor {
            type Value = Entries;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Entries, A::Error> {
                let mut entries = Vec::new();
                while let Some(entry) = map.next_entry()? {
                    entries.push(entry);
                }
                Ok(Entries(entries))
            }
        }
        deserializer.deserialize_map(EntriesVisitor)
    }
}

/// Captures the config file (required, not group- or world-writable) and the
/// credentials file (absent reads as empty, owner-only), each bounded.
fn capture_files(source: &OwnerSource) -> Result<[Zeroizing<Vec<u8>>; 2], CaptureFailure> {
    let config = read_secure(
        &source.config_file,
        aws_profile::MAX_CONFIG_FILE_BYTES,
        false,
    )?
    .ok_or(CaptureFailure::Missing)?;
    let credentials = read_secure(
        &source.credentials_file,
        aws_profile::MAX_CONFIG_FILE_BYTES,
        true,
    )?;
    Ok([
        config.bytes,
        credentials.map_or_else(Default::default, |c| c.bytes),
    ])
}

struct Captured {
    bytes: Zeroizing<Vec<u8>>,
    identity: [i64; 7],
}

/// Reads one owner file through a no-follow descriptor walk from `/`. Every ancestor
/// must be owned by this user or root and not writable by others unless sticky; below
/// a shared sticky ancestor every descendant must be owner-only and owned by this user.
/// The file must be a regular file owned by this user, not group- or world-writable,
/// owner-only when `owner_only` or below a shared ancestor, at most `limit` bytes, and
/// unchanged across the read. A missing file is `Ok(None)`.
fn read_secure(
    path: &Path,
    limit: usize,
    owner_only: bool,
) -> Result<Option<Captured>, CaptureFailure> {
    use CaptureFailure::{Io, Mutated, TooLarge, Unsafe};
    let names = normal_components(path)
        .filter(|_| path.is_absolute())
        .ok_or(Unsafe)?;
    let (file, dirs) = names.split_last().ok_or(Unsafe)?;
    let mut current = open_safe_anchor(path).map_err(|_| Io)?.ok_or(Unsafe)?;
    let mut below_shared = false;
    for name in dirs {
        current = match openat(&current, *name, HARDENED_DIR_FLAGS, Mode::empty()) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(rustix::io::Errno::LOOP | rustix::io::Errno::NOTDIR) => return Err(Unsafe),
            Err(_) => return Err(Io),
        };
        let stat = rustix::fs::fstat(&current).map_err(|_| Io)?;
        if !is_safe_ancestor(&stat) || (below_shared && !is_owner_only_dir(&stat)) {
            return Err(Unsafe);
        }
        below_shared |= mode_bits(&stat) & 0o022 != 0;
    }
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::NOCTTY;
    let fd = match openat(&current, *file, flags | OFlags::CLOEXEC, Mode::empty()) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(rustix::io::Errno::LOOP) => return Err(Unsafe),
        Err(_) => return Err(Io),
    };
    let before = rustix::fs::fstat(&fd).map_err(|_| Io)?;
    let mode = mode_bits(&before);
    let private = mode & 0o077 == 0;
    let safe = mode & S_IFMT == S_IFREG
        && before.st_uid == owner_uid()
        && (before.st_nlink == 1 || !(owner_only || below_shared))
        && mode & 0o022 == 0
        && (private || !(owner_only || below_shared));
    if !safe {
        return Err(Unsafe);
    }
    let mut file = std::fs::File::from(fd);
    let mut bytes = Zeroizing::new(Vec::new());
    let cap = u64::try_from(limit).map_err(|_| Io)? + 1;
    (&mut file)
        .take(cap)
        .read_to_end(&mut bytes)
        .map_err(|_| Io)?;
    if bytes.len() > limit {
        return Err(TooLarge);
    }
    let after = rustix::fs::fstat(&file).map_err(|_| Io)?;
    let identity = |s: &rustix::fs::Stat| {
        #[allow(clippy::unnecessary_cast)]
        [
            s.st_dev as i64,
            s.st_ino as i64,
            s.st_size as i64,
            s.st_mtime as i64,
            s.st_mtime_nsec as i64,
            s.st_ctime as i64,
            s.st_ctime_nsec as i64,
        ]
    };
    let unchanged = identity(&before) == identity(&after)
        && u64::try_from(after.st_size).is_ok_and(|size| size == bytes.len() as u64);
    if !unchanged {
        return Err(Mutated);
    }
    Ok(Some(Captured {
        bytes,
        identity: identity(&after),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> SsoToken {
        SsoToken {
            path: PathBuf::new(),
            file_name: String::new(),
            sso_region: "us-east-1".into(),
            start_url: "https://d-1.awsapps.com/start".into(),
        }
    }

    const VALID: &str = r#"{"accessToken":"a","expiresAt":"2030-01-01T00:00:00Z","refreshToken":"r","clientId":"c","clientSecret":"s","registrationExpiresAt":"2030-02-01T00:00:00Z","region":"us-east-1","startUrl":"https://d-1.awsapps.com/start","extra":"x"}"#;

    #[test]
    fn tokens_reemit_only_supported_fields_and_reject_ambiguity() {
        let canonical = session().canonical(VALID.as_bytes()).expect("valid token");
        let value: serde_json::Value = serde_json::from_slice(&canonical).unwrap();
        assert!(value.get("extra").is_none());
        assert_eq!(value["accessToken"], "a");
        assert_eq!(
            session().canonical(&canonical).as_deref(),
            Some(&*canonical)
        );
        let without_refresh = r#"{"accessToken":"a","expiresAt":"2030-01-01T00:00:00Z","region":"us-east-1","startUrl":"https://d-1.awsapps.com/start"}"#;
        assert!(session().canonical(without_refresh.as_bytes()).is_some());
        for invalid in [
            VALID.replace(r#""extra":"x""#, r#""ACCESSTOKEN":"b""#),
            VALID.replace(r#""accessToken":"a""#, r#""accessToken":"""#),
            VALID.replace(r#""accessToken":"a""#, r#""accessToken":1"#),
            VALID.replace("2030-01-01T00:00:00Z", "tomorrow"),
            VALID.replace("2030-02-01T00:00:00Z", "later"),
            VALID.replace(r#""clientSecret":"s","#, ""),
            VALID.replace("us-east-1", "eu-west-1"),
            VALID.replace("d-1.awsapps", "d-2.awsapps"),
            VALID.replace(r#","startUrl":"https://d-1.awsapps.com/start""#, ""),
            VALID[..VALID.len() / 2].to_owned(),
            "[]".to_owned(),
        ] {
            assert!(
                session().canonical(invalid.as_bytes()).is_none(),
                "{invalid}"
            );
        }
    }

    #[tokio::test]
    async fn an_unresolved_slot_refuses_every_later_transaction() {
        let dir = tempfile::tempdir().unwrap();
        let source = OwnerSource {
            profile: "dev".into(),
            region: "us-west-2".into(),
            config_file: dir.path().join("config"),
            credentials_file: dir.path().join("credentials"),
            sso_cache_root: dir.path().to_path_buf(),
        };
        let state = StateRoot::resolve(Some(dir.path())).unwrap();
        let mut slot = TransactionSlot { unresolved: true };
        let input = TransactionInput {
            source: &source,
            predecessor: None,
            executable: Path::new("/nonexistent"),
            state_root: &state,
            budget: TRANSACTION_BUDGET,
            limits: HelperLimits::default(),
            test_origin: None,
        };
        let outcome = slot.run(input, &CancellationToken::new()).await;
        assert_eq!(outcome.failure, Some(TransactionFailure::CleanupUnproven));
        assert!(slot.is_unresolved());
    }
}
