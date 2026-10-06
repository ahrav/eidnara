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
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rustix::fs::{Mode, OFlags, openat};
use rustix::process::Resource;
use serde::de::{self, IgnoredAny, MapAccess, Visitor};
use serde::ser::{SerializeMap, Serializer};
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
use crate::store_fs::create_owned_dir;

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
/// Slots in byte order of their key, so the canonical token is emitted with sorted
/// keys.
const EMIT_ORDER: [usize; 8] = [
    ACCESS_TOKEN,
    3,
    4,
    EXPIRES_AT,
    2,
    REGION,
    REGISTRATION_EXPIRES_AT,
    START_URL,
];
/// Bytes one emitted field adds beyond its key and value: four quotes, a colon, and
/// a separating comma or closing brace.
const FIELD_OVERHEAD: usize = 6;
type TokenFields = [Option<Zeroizing<String>>; 8];

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
#[derive(Clone, Debug, PartialEq, Eq)]
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
    let limits = launch_limits(&input.limits, !cfg!(debug_assertions));
    if cancel.is_cancelled() || launch_wall(deadline, &limits).is_zero() {
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
    let request = helper_request(&graph);
    let mut launch = match (prepared, request) {
        // The launch deadline includes time spent preparing the scratch directory.
        (Ok(()), Ok(request)) => match launch_wall(deadline, &limits) {
            Duration::ZERO => Launch::Expired,
            helper_wall => {
                let spec = helper_spec(&input, &dir, &limits, request);
                let run_limits = SubprocessLimits {
                    run_timeout: helper_wall,
                    termination_grace: GRACE,
                    drain_grace: GRACE,
                    max_stdout_bytes: aws_helper::MAX_REPORT_BYTES,
                    max_stderr_bytes: MAX_STDERR_BYTES,
                };
                match subprocess::run(spec, &run_limits, cancel, None).await {
                    Ok(result) => Launch::Ran(result),
                    Err(error) => Launch::Failed(subprocess::spawn_error_residue(&error)),
                }
            }
        },
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
        Launch::Expired => (true, None),
    };
    // Only an executed helper can have saved a successor.
    let successor = match (&sso, &launch) {
        (Some(sso), Launch::Ran(_)) if group_gone => {
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
        None if matches!(launch, Launch::Expired) => Some(F::BudgetExhausted),
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

/// The counting pass reserves exact space for the serialized request, keeping the
/// static secret it can carry in the one allocation that `Zeroizing` wipes on drop.
fn helper_request(graph: &aws_profile::AdmittedGraph) -> serde_json::Result<Zeroizing<Vec<u8>>> {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let request = HelperRequest::from_graph(graph);
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, &request)?;
    let mut out = Zeroizing::new(Vec::with_capacity(counter.0));
    serde_json::to_writer(&mut *out, &request)?;
    Ok(out)
}

fn launch_limits(requested: &HelperLimits, release: bool) -> HelperLimits {
    if release {
        HelperLimits::default()
    } else {
        requested.clone()
    }
}

fn launch_wall(deadline: Instant, limits: &HelperLimits) -> Duration {
    deadline
        .saturating_duration_since(Instant::now())
        .saturating_sub(3 * GRACE)
        .min(limits.wall)
}

enum Launch {
    Ran(subprocess::SubprocessResult),
    /// No helper ran; the flags report unproven teardown and a retained crash record.
    Failed((bool, bool)),
    /// The budget expired before helper launch.
    Expired,
}

fn helper_spec(
    input: &TransactionInput<'_>,
    dir: &PrivateDir,
    limits: &HelperLimits,
    stdin: Zeroizing<Vec<u8>>,
) -> SubprocessSpec {
    let mut env = vec![(OsString::from("HOME"), dir.path().as_os_str().to_owned())];
    if let Some(origin) = input
        .test_origin
        .as_ref()
        .filter(|_| cfg!(debug_assertions))
    {
        env.push((aws_helper::TEST_ORIGIN_ENV.into(), origin.into()));
    }
    let executable = if cfg!(debug_assertions) {
        input.executable
    } else {
        Path::new("/proc/self/exe")
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
/// writes the supplied token as a fresh `0600` file, in one blocking step.
async fn prepare_home(
    dir: &PrivateDir,
    sso: Option<&SsoToken>,
    token: Option<&[u8]>,
) -> std::io::Result<()> {
    let (Some(sso), Some(token)) = (sso, token) else {
        return Ok(());
    };
    let home = dir.path().to_path_buf();
    let name = format!(".aws/sso/cache/{}", sso.file_name);
    let token = Zeroizing::new(token.to_vec());
    subprocess::off_runtime(move || {
        let mut parent = openat(rustix::fs::CWD, &home, HARDENED_DIR_FLAGS, Mode::empty())?;
        for part in [".aws", "sso", "cache"] {
            parent = create_owned_dir(&parent, part)?;
        }
        PrivateDir::write_private_at(&home, &name, &token).map(drop)
    })
    .await?
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
        let Fields(fields) = serde_json::from_slice(bytes).ok()?;
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
        // The present fields are serialized straight from their zeroizing slots into a
        // buffer sized for the unescaped emission, so no plain copy of a secret is made.
        let present = EMIT_ORDER
            .iter()
            .filter_map(|slot| Some((TOKEN_FIELDS[*slot], text(*slot)?)));
        let capacity = present
            .clone()
            .map(|(key, value)| key.len() + value.len() + FIELD_OVERHEAD)
            .sum::<usize>()
            + 1;
        let mut out = Zeroizing::new(Vec::with_capacity(capacity));
        let mut serializer = serde_json::Serializer::new(&mut *out);
        let mut object = serializer.serialize_map(None).ok()?;
        for (key, value) in present {
            object.serialize_entry(key, value).ok()?;
        }
        object.end().ok()?;
        Some(out)
    }
}

/// The recognized token fields of one JSON object. A recognized key that repeats,
/// is not a string, is empty, or exceeds [`aws_helper::MAX_FIELD_BYTES`] fails the
/// parse; unrecognized keys are skipped.
struct Fields(TokenFields);

impl<'de> Deserialize<'de> for Fields {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FieldsVisitor;
        impl<'de> Visitor<'de> for FieldsVisitor {
            type Value = Fields;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Fields, A::Error> {
                let mut fields = TokenFields::default();
                while let Some(Slot(slot)) = map.next_key()? {
                    let Some(slot) = slot else {
                        map.next_value::<IgnoredAny>()?;
                        continue;
                    };
                    let value: String = map.next_value()?;
                    let valid = !value.is_empty() && value.len() <= aws_helper::MAX_FIELD_BYTES;
                    if !valid || fields[slot].replace(Zeroizing::new(value)).is_some() {
                        return Err(de::Error::custom("invalid or repeated token field"));
                    }
                }
                Ok(Fields(fields))
            }
        }
        deserializer.deserialize_map(FieldsVisitor)
    }
}

/// The slot of one object key in [`TOKEN_FIELDS`], matched case-insensitively without
/// copying the key; an unrecognized key is `None`.
struct Slot(Option<usize>);

impl<'de> Deserialize<'de> for Slot {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SlotVisitor;
        impl Visitor<'_> for SlotVisitor {
            type Value = Slot;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an object key")
            }
            fn visit_str<E: de::Error>(self, key: &str) -> Result<Slot, E> {
                Ok(Slot(
                    TOKEN_FIELDS
                        .iter()
                        .position(|field| field.eq_ignore_ascii_case(key)),
                ))
            }
        }
        deserializer.deserialize_str(SlotVisitor)
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
    let cap = u64::try_from(limit).map_err(|_| Io)? + 1;
    let reserve = usize::try_from(before.st_size).unwrap_or(0).min(limit) + 1;
    let mut bytes = Zeroizing::new(Vec::with_capacity(reserve));
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
    use std::os::unix::fs::PermissionsExt;

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
        let sorted: serde_json::Map<String, serde_json::Value> =
            serde_json::from_slice(&canonical).unwrap();
        assert_eq!(
            serde_json::to_vec(&sorted).unwrap(),
            *canonical,
            "the emission is compact with sorted keys"
        );
        assert_eq!(canonical.capacity(), canonical.len());
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

    /// `STATIC_ROLE` feeds a self-referencing role from a static root, so the helper
    /// request carries the static secret.
    const STATIC_ROLE: &str = "[profile app]\nrole_arn = arn:aws:iam::444455556666:role/app\nsource_profile = app\naws_access_key_id = AKIAIOSFODNN7EXAMPLE\naws_secret_access_key = staticsecret0000\n";

    fn static_owner(dir: &Path) -> OwnerSource {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let config_file = dir.join("config");
        PrivateDir::write_private_at(dir, "config", STATIC_ROLE.as_bytes()).unwrap();
        OwnerSource {
            profile: "app".into(),
            region: "us-west-2".into(),
            config_file,
            credentials_file: dir.join("credentials"),
            sso_cache_root: dir.to_path_buf(),
        }
    }

    #[test]
    fn release_launches_use_every_default_limit() {
        let requested = HelperLimits {
            address_space_bytes: 1,
            stack_bytes: 2,
            open_files: 3,
            cpu_seconds: 4,
            file_size_bytes: 5,
            wall: Duration::from_secs(60),
        };
        assert_eq!(launch_limits(&requested, true), HelperLimits::default());
        assert_eq!(launch_limits(&requested, false), requested);
    }

    #[test]
    fn the_helper_request_is_serialized_without_reallocation() {
        let config = STATIC_ROLE.as_bytes();
        let graph = aws_profile::admit(CapturedProfileInput {
            profile: "app",
            region: "us-west-2",
            config,
            credentials: b"",
        })
        .expect("static role admits");
        let request = helper_request(&graph).unwrap();
        assert_eq!(
            request.capacity(),
            request.len(),
            "a grown buffer leaves unwiped copies of the static secret"
        );
        let parsed: HelperRequest = serde_json::from_slice(&request).unwrap();
        assert_eq!(parsed.config, graph.emit_config());
    }

    async fn hold_slot() -> std::sync::mpsc::Sender<()> {
        let (release, released) = std::sync::mpsc::channel::<()>();
        let (held, holding) = tokio::sync::oneshot::channel();
        tokio::spawn(subprocess::off_runtime(move || {
            let _ = held.send(());
            let _ = released.recv();
        }));
        holding.await.unwrap();
        release
    }

    #[tokio::test]
    async fn a_budget_spent_waiting_for_scratch_launches_no_helper() {
        let dir = tempfile::tempdir().unwrap();
        let source = static_owner(dir.path());
        let state = StateRoot::resolve(Some(dir.path())).unwrap();
        let mut held = Vec::new();
        for _ in 0..subprocess::BLOCKING_SLOTS {
            held.push(hold_slot().await);
        }
        let mut slot = TransactionSlot::default();
        let input = TransactionInput {
            source: &source,
            predecessor: None,
            executable: Path::new("/nonexistent/eidnara-host"),
            state_root: &state,
            budget: 3 * GRACE + Duration::from_secs(1),
            limits: HelperLimits::default(),
            test_origin: None,
        };
        let cancel = CancellationToken::new();
        let run = slot.run(input, &cancel);
        let stall_scratch = async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            // Tokio's semaphore is fair, so this holder, queued after the capture,
            // receives the slot the capture releases.
            let (late_release, late_released) = std::sync::mpsc::channel::<()>();
            tokio::spawn(subprocess::off_runtime(move || {
                let _ = late_released.recv();
            }));
            tokio::time::sleep(Duration::from_millis(100)).await;
            drop(held.pop());
            tokio::time::sleep(Duration::from_millis(1500)).await;
            drop(late_release);
            held.clear();
        };
        let (outcome, ()) = tokio::join!(run, stall_scratch);
        assert_eq!(outcome.failure, Some(TransactionFailure::BudgetExhausted));
        assert_eq!(outcome.renewal, RenewalEvidence::NotStarted);
        assert!(!slot.is_unresolved());
        assert_eq!(
            std::fs::read_dir(state.run_root().unwrap())
                .unwrap()
                .count(),
            0
        );
    }

    #[tokio::test]
    async fn scratch_home_directories_are_owner_only_under_any_umask() {
        let dir = tempfile::tempdir().unwrap();
        let state = StateRoot::resolve(Some(dir.path())).unwrap();
        let home = PrivateDir::create_async(state, "aws-helper").await.unwrap();
        let sso = SsoToken {
            file_name: "token.json".into(),
            ..session()
        };
        let previous = rustix::process::umask(Mode::from_raw_mode(0o777));
        let prepared = prepare_home(&home, Some(&sso), Some(b"{}")).await;
        rustix::process::umask(previous);
        prepared.expect("an owner-bit-clearing umask still prepares the scratch HOME");
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o7777;
        let mut path = home.path().to_path_buf();
        for part in [".aws", "sso", "cache"] {
            path.push(part);
            assert_eq!(mode(&path), 0o700, "{part}");
        }
        assert_eq!(mode(&path.join("token.json")), 0o600);
        home.cleanup().unwrap();
    }
}
