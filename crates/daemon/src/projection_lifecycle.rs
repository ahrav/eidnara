//! The projection's lifecycle intent, kept outside the disposable `search/` family so a rebuild or an authorized recovery survives deleting the database it is about. One record, `search-lifecycle/intent.json`, names the transition, the selected generation, the kernel incarnation, the consumer binding, the cause, the attempt identity, the fixed recovery target once established, the episode allowance and how much of it is consumed, and the operator authorization a recovery needs. A repeated request with the same attempt identity reconciles to the record already there; a request that disagrees with it is refused and changes nothing. The record is replaced by rename after its bytes are synced and the directory is synced afterwards, so a reader sees the prior record, the new one, or explicit unavailability, never a mixture. The family is deleted under its own storage lease, so a live projection is never unlinked from under itself.

use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use retrieval::dispatch::valid_authorization_ref;
use rustix::fs::{FlockOperation, OFlags};
use serde::{Deserialize, Serialize};
use storage::{StoreError, delete_sqlite_family};

use crate::projection_gates::{Admission, Denial, EntryPoint, HookGate, ProjectionHook};
use crate::search_projection::search_descriptor;

pub const CONTROL_DIR: &str = "search-lifecycle";
pub const CONTROL_RECORD: &str = "intent.json";
const TEMP_PREFIX: &str = "intent.";
const TEMP_SUFFIX: &str = ".tmp";
/// Schema 2 adds `staged_seed_digest`; a record of another schema is unavailable rather than read with defaults.
const SCHEMA: u32 = 2;
/// A record larger than this is not decoded; it is reported unavailable.
pub const MAX_RECORD_BYTES: u64 = 64 * 1024;

/// The two transitions that persist intent. Every other projection state is derived from the database itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Transition {
    Rebuilding,
    AuthorizedRecovery,
}

impl Transition {
    /// The hook whose admission the transition needs: a rebuild bootstraps the projection; a recovery resumes its backfill.
    fn hook(self) -> ProjectionHook {
        match self {
            Self::Rebuilding => ProjectionHook::EmbeddingBootstrap,
            Self::AuthorizedRecovery => ProjectionHook::EmbeddingBackfill,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cause {
    SchemaMismatch,
    TokenizerMismatch,
    EmbeddingModelMismatch,
    ProjectionPolicyMismatch,
    IdentityContractMismatch,
    /// The projection was deleted after pruning and is rebuilt from the kernel.
    DeletedAfterPruning,
    /// A Disabled projection resumes under operator authorization.
    DisabledRecovery,
}

/// The kernel consumer the projection reads through and the vector generation it produces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsumerBinding {
    pub consumer_id: String,
    pub generation_id: String,
}

/// The kernel commit the transition must reach; fixed when first established and never moved by a retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryTarget {
    pub commit_seq: i64,
}

/// How many episodes the transition may run and how many it has consumed. A restart consumes nothing and grants nothing back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EpisodeAccounting {
    pub allowance: u32,
    pub consumed: u32,
    pub deadline: i64,
}

/// What a caller asks to persist. `attempt_id` is the caller's identity for the request; a replay carries the same one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleRequest {
    pub transition: Transition,
    pub selected_generation: String,
    pub kernel_incarnation_id: String,
    pub consumer: ConsumerBinding,
    pub cause: Cause,
    pub attempt_id: String,
    pub recovery_target: Option<RecoveryTarget>,
    pub allowance: u32,
    pub deadline: i64,
    pub authorization_ref: Option<String>,
}

/// The persisted record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleIntent {
    pub schema: u32,
    pub transition: Transition,
    pub selected_generation: String,
    pub kernel_incarnation_id: String,
    pub consumer: ConsumerBinding,
    pub cause: Cause,
    pub attempt_id: String,
    pub recovery_target: Option<RecoveryTarget>,
    pub episodes: EpisodeAccounting,
    pub authorization_ref: Option<String>,
    /// The lifecycle-store digest of the closed seed staged for this transition; the reclaimer protects it while the intent stands.
    pub staged_seed_digest: Option<String>,
    pub recorded_at: i64,
}

impl LifecycleIntent {
    /// Whether `request` is a replay of this record: the same intent in every field the caller supplies.
    fn is_replay_of(&self, request: &LifecycleRequest) -> bool {
        self.transition == request.transition
            && self.selected_generation == request.selected_generation
            && self.kernel_incarnation_id == request.kernel_incarnation_id
            && self.consumer == request.consumer
            && self.cause == request.cause
            && self.attempt_id == request.attempt_id
            && self.recovery_target == request.recovery_target
            && self.episodes.allowance == request.allowance
            && self.episodes.deadline == request.deadline
            && self.authorization_ref == request.authorization_ref
    }
}

/// `record` rejects and `read` marks unavailable intents with a blank consumer or an invalid transition, `authorization_ref`, and cause combination.
fn check_invariants(
    consumer_id: &str,
    transition: Transition,
    authorization_ref: Option<&str>,
    cause: Cause,
) -> Result<(), IntentRefusal> {
    if consumer_id.trim().is_empty() {
        return Err(IntentRefusal::InvalidConsumer);
    }
    match (transition, authorization_ref, cause) {
        (Transition::AuthorizedRecovery, None, _) => Err(IntentRefusal::MissingAuthorization),
        (_, Some(reference), _) if !valid_authorization_ref(reference) => {
            Err(IntentRefusal::InvalidAuthorization)
        }
        (Transition::AuthorizedRecovery, Some(_), cause) if cause != Cause::DisabledRecovery => {
            Err(IntentRefusal::IllegalCombination)
        }
        (Transition::Rebuilding, Some(_), _)
        | (Transition::Rebuilding, None, Cause::DisabledRecovery) => {
            Err(IntentRefusal::IllegalCombination)
        }
        _ => Ok(()),
    }
}

/// What the control record holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlState {
    Absent,
    Intent(LifecycleIntent),
    /// The record exists but cannot be trusted: not a regular owner-only file, over the size cap, malformed, or of another schema. Nothing is decided from it.
    Unavailable(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IntentRefusal {
    #[error("the consumer binding names no consumer")]
    InvalidConsumer,
    #[error("an authorized recovery needs an operator authorization reference")]
    MissingAuthorization,
    #[error(
        "the authorization reference is not one token of [A-Za-z0-9._:-] within the size bound"
    )]
    InvalidAuthorization,
    #[error(
        "a rebuild carries no operator authorization and a recovery's cause is the disabled projection"
    )]
    IllegalCombination,
    #[error("the projection gate denied the transition: {0}")]
    Denied(Denial),
    /// The gate invalidated the admission before the operation took effect; nothing was changed.
    #[error("the admission was invalidated before the operation took effect")]
    Revoked,
    /// Another intent is already recorded; nothing was changed.
    #[error("an intent with attempt {attempt_id} is already recorded and does not match")]
    Conflict { attempt_id: String },
    #[error("the control record is unavailable: {0}")]
    Unavailable(String),
    #[error("every episode of the allowance is consumed")]
    AllowanceExhausted,
    #[error("the episode deadline has passed")]
    DeadlineExpired,
    #[error("no intent is recorded")]
    NoIntent,
    /// A live store holds the disposable family's storage lease; nothing was removed.
    #[error("a live projection holds the disposable family")]
    FamilyHeld,
    /// The encoded record exceeds [`MAX_RECORD_BYTES`]; nothing was written.
    #[error("the encoded record exceeds the size cap")]
    Oversized,
    #[error("control record I/O failed: {0}")]
    Io(String),
    /// The control directory's sync failed. After a rename the new record is visible and may or may not survive power loss; the caller reads the record again rather than retrying the write.
    #[error("the control directory sync failed: {0}")]
    DurabilityUnknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub intent: LifecycleIntent,
    /// The request matched the record already there; nothing was written.
    pub replayed: bool,
}

/// Points at which a test child parks so the parent can cut the process there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteBarrier {
    /// The temp file holds the synced new bytes; the record still names the prior bytes.
    BeforeRename,
    /// The record names the new bytes; the directory entry is not yet synced.
    AfterRename,
    AfterDirectorySync,
    /// The deletion is admitted and the record synced; no family member is removed yet.
    BeforeFamilyRemoval,
}

/// The daemon's handle on the control record.
pub struct ProjectionLifecycle {
    /// The data home the record governs; the disposable family it may delete is the one under this same root.
    data_home: PathBuf,
    dir: PathBuf,
    /// The directory, held open for the exclusive lock every read-then-replace runs under.
    dir_fd: File,
    /// Serializes this handle's own threads. `flock` belongs to the open file description, so a second lock on `dir_fd` from another thread of this process would be granted at once and the first guard's unlock would release it for both.
    threads: Mutex<()>,
    #[cfg(feature = "test-support")]
    barrier: Option<Box<dyn Fn(WriteBarrier) + Send + Sync>>,
}

impl ProjectionLifecycle {
    /// Opens `<data_home>/search-lifecycle/`, creating it owner-only. Syncs `data_home` after creating or finding the directory to make its entry durable.
    ///
    /// # Errors
    ///
    /// Returns the I/O error when the directory cannot be created, synced, or is not owner-only.
    pub fn open(data_home: &Path) -> io::Result<Self> {
        let dir = data_home.join(CONTROL_DIR);
        if let Err(error) = fs::DirBuilder::new().mode(0o700).create(&dir)
            && error.kind() != io::ErrorKind::AlreadyExists
        {
            return Err(error);
        }
        // The creator may have died before this sync, so an existing directory is synced too.
        File::open(data_home)?.sync_all()?;
        let dir_fd = open_directory(&dir)?;
        let metadata = dir_fd.metadata()?;
        if !metadata.is_dir() || !owned_by_caller(&metadata) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the lifecycle directory is not the caller's own directory",
            ));
        }
        // The umask may have narrowed the requested mode; the caller's own directory is set to exactly owner-only.
        dir_fd.set_permissions(Permissions::from_mode(0o700))?;
        let this = Self {
            data_home: data_home.to_path_buf(),
            dir,
            dir_fd,
            threads: Mutex::new(()),
            #[cfg(feature = "test-support")]
            barrier: None,
        };
        // A temp left by a process cut before its rename is never adopted; it is removed under the writer's lock so a live writer's temp is left alone.
        let lock = this.lock()?;
        for entry in fs::read_dir(&this.dir)?.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(TEMP_PREFIX) && name.ends_with(TEMP_SUFFIX) {
                let _ = fs::remove_file(entry.path());
            }
        }
        // A writer cut after its rename left the record visible but not yet durable.
        this.dir_fd.sync_all()?;
        drop(lock);
        Ok(this)
    }

    /// Takes the handle's thread lock and then the directory's exclusive `flock` for the guard's lifetime, so one read-then-replace at a time runs in this process or any other.
    fn lock(&self) -> io::Result<DirectoryLock<'_>> {
        let threads = self
            .threads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        rustix::fs::flock(&self.dir_fd, FlockOperation::LockExclusive)?;
        Ok(DirectoryLock {
            _flock: FlockRelease(&self.dir_fd),
            _threads: threads,
        })
    }

    /// Calls `barrier` at each write barrier; a test child parks there so its parent can kill it.
    #[cfg(feature = "test-support")]
    pub fn with_write_barrier_for_test(
        mut self,
        barrier: impl Fn(WriteBarrier) + Send + Sync + 'static,
    ) -> Self {
        self.barrier = Some(Box::new(barrier));
        self
    }

    fn at(&self, barrier: WriteBarrier) {
        #[cfg(feature = "test-support")]
        if let Some(hook) = &self.barrier {
            hook(barrier);
        }
        #[cfg(not(feature = "test-support"))]
        let _ = barrier;
    }

    /// Reads the record. Never fails: anything short of a complete owner-only record of this schema is [`ControlState::Unavailable`].
    pub fn read(&self) -> ControlState {
        let path = self.dir.join(CONTROL_RECORD);
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags((OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK).bits() as i32)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return ControlState::Absent,
            Err(error) => return ControlState::Unavailable(error.kind().to_string()),
        };
        let metadata = match file.metadata() {
            Ok(metadata) => metadata,
            Err(error) => return ControlState::Unavailable(error.kind().to_string()),
        };
        if !metadata.is_file() || metadata.mode() & 0o077 != 0 || !owned_by_caller(&metadata) {
            return ControlState::Unavailable(
                "not the caller's own owner-only regular file".to_owned(),
            );
        }
        if metadata.len() > MAX_RECORD_BYTES {
            return ControlState::Unavailable("over the size cap".to_owned());
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        if let Err(error) = (&file).take(MAX_RECORD_BYTES).read_to_end(&mut bytes) {
            return ControlState::Unavailable(error.kind().to_string());
        }
        match serde_json::from_slice::<LifecycleIntent>(&bytes) {
            Ok(intent) if intent.schema != SCHEMA => {
                ControlState::Unavailable(format!("schema {}", intent.schema))
            }
            Ok(intent) => match check_invariants(
                &intent.consumer.consumer_id,
                intent.transition,
                intent.authorization_ref.as_deref(),
                intent.cause,
            ) {
                Ok(()) => ControlState::Intent(intent),
                Err(refusal) => ControlState::Unavailable(refusal.to_string()),
            },
            Err(_) => ControlState::Unavailable("malformed record".to_owned()),
        }
    }

    /// Validates `request` before asking `gate` to admit its hook at [`EntryPoint::Reload`], and reads the record only once admitted.
    ///
    /// # Errors
    ///
    /// Returns [`IntentRefusal::AllowanceExhausted`] when `allowance` is zero or [`IntentRefusal::DeadlineExpired`] when `deadline` precedes `now`, before the gate is asked.
    pub fn record(
        &self,
        gate: &HookGate,
        request: &LifecycleRequest,
        now: i64,
    ) -> Result<Recorded, IntentRefusal> {
        check_invariants(
            &request.consumer.consumer_id,
            request.transition,
            request.authorization_ref.as_deref(),
            request.cause,
        )?;
        if request.allowance == 0 {
            return Err(IntentRefusal::AllowanceExhausted);
        }
        if now > request.deadline {
            return Err(IntentRefusal::DeadlineExpired);
        }
        let admission = gate
            .admit(request.transition.hook(), EntryPoint::Reload)
            .map_err(IntentRefusal::Denied)?;
        let _lock = self.lock().map_err(io_refusal)?;
        match self.read() {
            ControlState::Absent => {}
            ControlState::Intent(existing) if existing.is_replay_of(request) => {
                self.sync_directory()?;
                return Ok(Recorded {
                    intent: existing,
                    replayed: true,
                });
            }
            ControlState::Intent(existing) => {
                return Err(IntentRefusal::Conflict {
                    attempt_id: existing.attempt_id,
                });
            }
            ControlState::Unavailable(reason) => return Err(IntentRefusal::Unavailable(reason)),
        }
        let intent = LifecycleIntent {
            schema: SCHEMA,
            transition: request.transition,
            selected_generation: request.selected_generation.clone(),
            kernel_incarnation_id: request.kernel_incarnation_id.clone(),
            consumer: request.consumer.clone(),
            cause: request.cause,
            attempt_id: request.attempt_id.clone(),
            recovery_target: request.recovery_target,
            episodes: EpisodeAccounting {
                allowance: request.allowance,
                consumed: 0,
                deadline: request.deadline,
            },
            authorization_ref: request.authorization_ref.clone(),
            staged_seed_digest: None,
            recorded_at: now,
        };
        fits_when_exhausted(&intent)?;
        self.replace(&intent, &admission)?;
        Ok(Recorded {
            intent,
            replayed: false,
        })
    }

    /// Consumes one episode of the recorded allowance under the gate's admission of the recorded transition, and returns the accounting after it. The record's target, cause, and authorization are untouched; only `consumed` moves.
    ///
    /// # Errors
    ///
    /// Returns [`IntentRefusal::NoIntent`] with no record, [`IntentRefusal::Unavailable`] with an untrusted one, [`IntentRefusal::Denied`] when the gate refuses the transition, [`IntentRefusal::DeadlineExpired`] past the recorded deadline, and [`IntentRefusal::AllowanceExhausted`] once every episode is consumed.
    pub fn consume_episode(
        &self,
        gate: &HookGate,
        now: i64,
    ) -> Result<EpisodeAccounting, IntentRefusal> {
        let _lock = self.lock().map_err(io_refusal)?;
        let (mut intent, admission) = self.admitted_intent(gate)?;
        if now > intent.episodes.deadline {
            return Err(IntentRefusal::DeadlineExpired);
        }
        if intent.episodes.consumed >= intent.episodes.allowance {
            return Err(IntentRefusal::AllowanceExhausted);
        }
        intent.episodes.consumed += 1;
        self.replace(&intent, &admission)?;
        Ok(intent.episodes)
    }

    /// Pins the staged seed `digest` to the recorded intent under the gate's admission, so the reclaimer keeps that object while the intent stands. Pinning the same digest again changes nothing; another digest is refused, since one transition builds from one seed.
    ///
    /// # Errors
    ///
    /// Returns [`IntentRefusal::NoIntent`], [`IntentRefusal::Unavailable`], [`IntentRefusal::Denied`], [`IntentRefusal::Conflict`] with the recorded attempt when another seed is already pinned, [`IntentRefusal::Oversized`] when the pinned record would not fit once its allowance is consumed, or [`IntentRefusal::Revoked`] when the gate invalidated the admission before the write; the record is unchanged in every refused case.
    pub fn pin_seed(
        &self,
        gate: &HookGate,
        digest: &str,
    ) -> Result<LifecycleIntent, IntentRefusal> {
        let _lock = self.lock().map_err(io_refusal)?;
        let (mut intent, admission) = self.admitted_intent(gate)?;
        match intent.staged_seed_digest.as_deref() {
            // A prior pin may have renamed the record and failed its directory sync; the replay syncs before reporting the pin durable, as `record` does.
            Some(pinned) if pinned == digest => {
                self.sync_directory()?;
                return Ok(intent);
            }
            Some(_) => {
                return Err(IntentRefusal::Conflict {
                    attempt_id: intent.attempt_id,
                });
            }
            None => {}
        }
        intent.staged_seed_digest = Some(digest.to_owned());
        fits_when_exhausted(&intent)?;
        self.replace(&intent, &admission)?;
        Ok(intent)
    }

    /// The gate admits the intent's transition before callers modify the control record or remove the disposable family.
    fn admitted_intent(
        &self,
        gate: &HookGate,
    ) -> Result<(LifecycleIntent, Admission), IntentRefusal> {
        let intent = match self.read() {
            ControlState::Absent => return Err(IntentRefusal::NoIntent),
            ControlState::Unavailable(reason) => return Err(IntentRefusal::Unavailable(reason)),
            ControlState::Intent(intent) => intent,
        };
        let admission = gate
            .admit(intent.transition.hook(), EntryPoint::Reload)
            .map_err(IntentRefusal::Denied)?;
        Ok((intent, admission))
    }

    /// Removes the database and journals of this handle's disposable family while holding the family's exclusive storage lease. The gate must admit a recorded intent whose deadline has not passed and whose episode allowance remains.
    ///
    /// # Errors
    ///
    /// Returns [`IntentRefusal::NoIntent`], [`IntentRefusal::Unavailable`], [`IntentRefusal::Denied`], [`IntentRefusal::DeadlineExpired`], [`IntentRefusal::AllowanceExhausted`], or [`IntentRefusal::FamilyHeld`] before removing anything, [`IntentRefusal::Io`] for a removal that failed for a reason other than the file being absent, and [`IntentRefusal::DurabilityUnknown`] when the family was removed and the directory sync after it failed.
    pub fn delete_disposable_family(
        &self,
        gate: &HookGate,
        now: i64,
    ) -> Result<LifecycleIntent, IntentRefusal> {
        let _lock = self.lock().map_err(io_refusal)?;
        let (intent, admission) = self.admitted_intent(gate)?;
        if now > intent.episodes.deadline {
            return Err(IntentRefusal::DeadlineExpired);
        }
        if intent.episodes.consumed >= intent.episodes.allowance {
            return Err(IntentRefusal::AllowanceExhausted);
        }
        // Nothing is removed on the strength of a record whose own durability is unknown.
        self.sync_directory()?;
        let descriptor = search_descriptor(&self.data_home).map_err(store_refusal)?;
        self.at(WriteBarrier::BeforeFamilyRemoval);
        if admission.invalidated.is_cancelled() {
            return Err(IntentRefusal::Revoked);
        }
        delete_sqlite_family(&descriptor).map_err(store_refusal)?;
        Ok(intent)
    }

    fn sync_directory(&self) -> Result<(), IntentRefusal> {
        self.dir_fd
            .sync_all()
            .map_err(|error| IntentRefusal::DurabilityUnknown(error.kind().to_string()))
    }

    /// Writes the record to a fresh temp file, syncs it, renames it over the record, and syncs the directory.
    fn replace(
        &self,
        intent: &LifecycleIntent,
        admission: &Admission,
    ) -> Result<(), IntentRefusal> {
        let io = io_refusal;
        let bytes = encode(intent)?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(IntentRefusal::Oversized);
        }
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let temp = self.dir.join(format!(
            "{TEMP_PREFIX}{}-{unique}{TEMP_SUFFIX}",
            std::process::id()
        ));
        let written = (|| -> io::Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(OFlags::CLOEXEC.bits() as i32)
                .open(&temp)?;
            file.set_permissions(Permissions::from_mode(0o600))?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            Ok(())
        })();
        if let Err(error) = written {
            let _ = fs::remove_file(&temp);
            return Err(io(error));
        }
        self.at(WriteBarrier::BeforeRename);
        if admission.invalidated.is_cancelled() {
            let _ = fs::remove_file(&temp);
            return Err(IntentRefusal::Revoked);
        }
        if let Err(error) = fs::rename(&temp, self.dir.join(CONTROL_RECORD)) {
            let _ = fs::remove_file(&temp);
            return Err(io(error));
        }
        self.at(WriteBarrier::AfterRename);
        self.sync_directory()?;
        self.at(WriteBarrier::AfterDirectorySync);
        Ok(())
    }
}

fn encode(intent: &LifecycleIntent) -> Result<Vec<u8>, IntentRefusal> {
    serde_json::to_vec(intent).map_err(|_| IntentRefusal::Io("encode".to_owned()))
}

/// The hex SHA-256 a staged seed's digest takes; the record reserves room for one before any is pinned.
const DIGEST_HEX_LEN: usize = 64;

/// `consumed` grows to `allowance` and a seed digest may be pinned; every write of the record checks that it still fits with both, so neither a granted episode nor the pin of an accepted intent is refused as [`IntentRefusal::Oversized`].
fn fits_when_exhausted(intent: &LifecycleIntent) -> Result<(), IntentRefusal> {
    let exhausted = LifecycleIntent {
        episodes: EpisodeAccounting {
            consumed: intent.episodes.allowance,
            ..intent.episodes
        },
        staged_seed_digest: Some(
            intent
                .staged_seed_digest
                .clone()
                .unwrap_or_else(|| "0".repeat(DIGEST_HEX_LEN)),
        ),
        ..intent.clone()
    };
    if encode(&exhausted)?.len() as u64 > MAX_RECORD_BYTES {
        return Err(IntentRefusal::Oversized);
    }
    Ok(())
}

/// Fields drop in declaration order, so the `flock` is released before `_threads`.
struct DirectoryLock<'a> {
    _flock: FlockRelease<'a>,
    _threads: MutexGuard<'a, ()>,
}

struct FlockRelease<'a>(&'a File);

impl Drop for FlockRelease<'_> {
    fn drop(&mut self) {
        let _ = rustix::fs::flock(self.0, FlockOperation::Unlock);
    }
}

fn open_directory(dir: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags((OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC).bits() as i32)
        .open(dir)
}

fn owned_by_caller(metadata: &fs::Metadata) -> bool {
    metadata.uid() == rustix::process::geteuid().as_raw()
}

fn io_refusal(error: io::Error) -> IntentRefusal {
    IntentRefusal::Io(error.kind().to_string())
}

fn store_refusal(error: StoreError) -> IntentRefusal {
    match error {
        StoreError::Lease(lease::LeaseError::Held { .. }) => IntentRefusal::FamilyHeld,
        StoreError::DurabilityUnknown(error) => {
            IntentRefusal::DurabilityUnknown(error.kind().to_string())
        }
        other => IntentRefusal::Io(other.to_string()),
    }
}
