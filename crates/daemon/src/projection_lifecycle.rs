//! The projection's lifecycle intent, kept outside the disposable `search/` family so a rebuild or an authorized recovery survives deleting the database it is about. One record, `search-lifecycle/intent.json`, names the transition, the selected generation, the kernel incarnation, the consumer binding, the cause, the attempt identity, the fixed recovery target once established, the episode allowance and how much of it is consumed, and the operator authorization a recovery needs. A repeated request with the same attempt identity reconciles to the record already there; a request that disagrees with it is refused and changes nothing. The record is replaced by rename after its bytes are synced and the directory is synced afterwards, so a reader sees the prior record, the new one, or explicit unavailability, never a mixture. The family is deleted under its own storage lease, so a live projection is never unlinked from under itself.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
#[cfg(feature = "test-support")]
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::sync::{Mutex, MutexGuard};

use host_runtime::LifecycleTransactionLock;
use host_runtime::lifecycle::is_canonical_payload_digest;
use kernel::source_identity::well_formed_value;
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
const ACTIVE_SCHEMA: u32 = 2;
const DISABLED_SCHEMA: u32 = 3;
const CURRENT_SCHEMA: u32 = 4;
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
    pub(crate) fn hook(self) -> ProjectionHook {
        match self {
            Self::Rebuilding => ProjectionHook::EmbeddingBootstrap,
            Self::AuthorizedRecovery => ProjectionHook::EmbeddingBackfill,
        }
    }

    /// The manifest limit that bounds the transition's whole episode.
    pub(crate) fn duration_limit(self) -> &'static str {
        match self {
            Self::Rebuilding => "B_recovery_ms",
            Self::AuthorizedRecovery => "B_authorized_recovery_ms",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cause {
    Corruption,
    SchemaMismatch,
    TokenizerMismatch,
    /// The lexical rows were analyzed under another analysis identity.
    AnalysisMismatch,
    EmbeddingModelMismatch,
    ProjectionPolicyMismatch,
    IdentityContractMismatch,
    /// A limit manifest protocol change requires a rebuild.
    LimitProtocolMismatch,
    /// A kernel restore within the same database lineage requires a rebuild.
    KernelRestored,
    /// The projection was deleted after pruning and is rebuilt from the kernel.
    DeletedAfterPruning,
    /// A Disabled projection resumes under operator authorization.
    DisabledRecovery,
    /// No projection exists yet; the installed records admit its first build from the kernel.
    Registration,
    /// Catch-up hold extension for the Current family returned `SourceHoldError::BindingMismatch` or `SourceHoldError::Invalid(_)`.
    CatchUpHoldLost,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplacementCapture {
    pub hold_id: String,
    pub snapshot: i64,
    pub lease_epoch: u64,
    pub source_policy_version: String,
    pub expires_at: i64,
    pub stage: Option<Box<crate::search_seed::SeedVerification>>,
}

impl ReplacementCapture {
    /// A stage certificate must be well formed and name this capture's hold and snapshot.
    fn stage_is_bound(&self) -> bool {
        self.stage.as_deref().is_none_or(|stage| {
            stage.shape_matches_schema()
                && stage.hold_id == self.hold_id
                && stage.snapshot_commit_seq == self.snapshot
        })
    }
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
    /// Omitting the requested target does not reset an existing target.
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement_capture: Option<Box<ReplacementCapture>>,
    pub recorded_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_disabled: Option<Box<DisabledIntent>>,
}

impl LifecycleIntent {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != ACTIVE_SCHEMA {
            return Err(format!("schema {}", self.schema));
        }
        check_invariants(
            &self.consumer,
            self.transition,
            self.authorization_ref.as_deref(),
            self.cause,
        )
        .map_err(|refusal| refusal.to_string())?;
        // `record` admits only a positive allowance and a deadline at or after `recorded_at`,
        // and `consume_episode` never passes the allowance, so other accounting is corruption.
        if self.episodes.allowance == 0
            || self.episodes.consumed > self.episodes.allowance
            || self.episodes.deadline < self.recorded_at
        {
            return Err("impossible episode accounting".to_owned());
        }
        if !self
            .replacement_capture
            .as_deref()
            .is_none_or(ReplacementCapture::stage_is_bound)
        {
            return Err("certificate names another capture".to_owned());
        }
        if let Some(prior) = self.prior_disabled.as_deref() {
            prior.validate()?;
            // `authorize_recovery` retains a second level only as a deregistration proof.
            if let Some(inner) = prior
                .handoff
                .as_deref()
                .and_then(|handoff| handoff.prior_disabled.as_deref())
                && (!inner.deregistered
                    || inner
                        .handoff
                        .as_deref()
                        .is_some_and(|handoff| handoff.prior_disabled.is_some()))
            {
                return Err("nested prior handoff".to_owned());
            }
        }
        Ok(())
    }

    /// Whether `request` is a replay of this record: the same intent in every field the caller supplies.
    pub(crate) fn is_replay_of(&self, request: &LifecycleRequest) -> bool {
        self.transition == request.transition
            && self.selected_generation == request.selected_generation
            && self.kernel_incarnation_id == request.kernel_incarnation_id
            && self.consumer == request.consumer
            && self.cause == request.cause
            && self.attempt_id == request.attempt_id
            && (request.recovery_target.is_none()
                || self.recovery_target == request.recovery_target)
            && self.episodes.allowance == request.allowance
            && self.episodes.deadline == request.deadline
            && self.authorization_ref == request.authorization_ref
    }
}

/// `record` rejects and `read` marks unavailable intents with a blank consumer, a generation id
/// outside the kernel identity-value rule, or an invalid transition, `authorization_ref`, and
/// cause combination. The generation rule is the one `retrieval::fusion::GenerationId` applies,
/// so every generation admitted here has a fusion spelling.
fn check_invariants(
    consumer: &ConsumerBinding,
    transition: Transition,
    authorization_ref: Option<&str>,
    cause: Cause,
) -> Result<(), IntentRefusal> {
    if consumer.consumer_id.trim().is_empty() {
        return Err(IntentRefusal::InvalidConsumer);
    }
    if !well_formed_value(&consumer.generation_id) {
        return Err(IntentRefusal::InvalidGeneration);
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
    /// Current stores completion history; it does not prove live inventory or query authorization.
    Current(LifecycleIntent),
    Disabled(DisabledIntent),
    /// The record exists but cannot be trusted: not a regular owner-only file, over the size cap, malformed, or of another schema. Nothing is decided from it.
    Unavailable(String),
}

/// A durable admission stop. The construction handoff is evidence, not permission to recover.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisabledIntent {
    schema: u32,
    pub handoff: Option<Box<LifecycleIntent>>,
    pub recorded_at: i64,
    pub episodes: Option<EpisodeAccounting>,
    pub through: Option<i64>,
    pub deregistered: bool,
}

impl DisabledIntent {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != DISABLED_SCHEMA
            || self.recorded_at < 0
            || self.through.is_some_and(|n| n < 0)
            || (self.deregistered && self.through.is_none())
            || (self.through.is_some() && self.handoff.is_none())
            || self.episodes.is_some_and(|e| {
                e.allowance == 0 || e.consumed > e.allowance || e.deadline <= self.recorded_at
            })
        {
            return Err("invalid disabled record".to_owned());
        }
        self.handoff
            .as_deref()
            .map_or(Ok(()), LifecycleIntent::validate)
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StoredIntent {
    Active(LifecycleIntent),
    Disabled(DisabledIntent),
    Current(CurrentIntent),
}

impl StoredIntent {
    fn decode(bytes: &[u8]) -> serde_json::Result<Self> {
        if let Ok(current) = serde_json::from_slice(bytes) {
            return Ok(Self::Current(current));
        }
        match serde_json::from_slice::<StoredKeys>(bytes) {
            Ok(StoredKeys { current: true, .. }) => {
                serde_json::from_slice(bytes).map(Self::Current)
            }
            Ok(StoredKeys {
                deregistered: true, ..
            }) => serde_json::from_slice(bytes).map(Self::Disabled),
            Ok(_) => serde_json::from_slice(bytes).map(Self::Active),
            Err(_) => serde_json::from_slice(bytes),
        }
    }
}

#[derive(Default)]
struct StoredKeys {
    current: bool,
    deregistered: bool,
}

#[derive(Deserialize)]
#[serde(field_identifier, rename_all = "snake_case")]
enum StoredKey {
    Current,
    Deregistered,
    #[serde(other)]
    Other,
}

impl<'de> Deserialize<'de> for StoredKeys {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Keys;
        impl<'de> serde::de::Visitor<'de> for Keys {
            type Value = StoredKeys;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a control record object")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<StoredKeys, A::Error> {
                let mut keys = StoredKeys::default();
                while let Some(key) = map.next_key()? {
                    match key {
                        StoredKey::Current => keys.current = true,
                        StoredKey::Deregistered => keys.deregistered = true,
                        StoredKey::Other => {}
                    }
                    map.next_value::<serde::de::IgnoredAny>()?;
                }
                Ok(keys)
            }
        }
        deserializer.deserialize_map(Keys)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CurrentIntent {
    schema: u32,
    current: LifecycleIntent,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IntentRefusal {
    #[error("retrieval is durably disabled; operator recovery is required")]
    Disabled,
    #[error("the consumer binding names no consumer")]
    InvalidConsumer,
    #[error(
        "the consumer binding's generation id is empty, over the identity size bound, or holds a control character"
    )]
    InvalidGeneration,
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
    #[error("the lifecycle timestamp is negative")]
    InvalidTimestamp,
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
    #[error("the staged seed digest is not 64 lowercase hexadecimal digits")]
    InvalidDigest,
    #[error("another lifecycle operation holds the control record lock")]
    WouldBlock,
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
    #[cfg(feature = "test-support")]
    fail_directory_sync: Option<Arc<AtomicBool>>,
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
        // The umask may have narrowed the requested mode; the caller's own directory is set to exactly owner-only before the shared check.
        dir_fd.set_permissions(Permissions::from_mode(0o700))?;
        owner_only_directory(&dir_fd.metadata()?)
            .map_err(|reason| io::Error::new(io::ErrorKind::InvalidData, reason))?;
        let this = Self {
            data_home: data_home.to_path_buf(),
            dir,
            dir_fd,
            threads: Mutex::new(()),
            #[cfg(feature = "test-support")]
            barrier: None,
            #[cfg(feature = "test-support")]
            fail_directory_sync: None,
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
        let threads = match self.threads.try_lock() {
            Ok(threads) => threads,
            Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                return Err(io::ErrorKind::WouldBlock.into());
            }
        };
        rustix::fs::flock(&self.dir_fd, FlockOperation::NonBlockingLockExclusive)?;
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

    /// While `flag` is set, the next directory sync clears it and fails with [`IntentRefusal::DurabilityUnknown`].
    #[cfg(feature = "test-support")]
    pub fn with_directory_sync_failure_for_test(mut self, flag: Arc<AtomicBool>) -> Self {
        self.fail_directory_sync = Some(flag);
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
        Self::read_at(&self.data_home)
    }

    pub(crate) fn read_at(data_home: &Path) -> ControlState {
        Self::probe_at(data_home)
            .unwrap_or_else(|Unreadable(reason)| ControlState::Unavailable(reason))
    }

    /// Reads the record, keeping a record that could not be read apart from one that was read and rejected. `Err` is an I/O failure or a directory whose mode or owner is not the daemon's own; `open` or the next write repairs those, so a caller must not decide a durable stop from one. `Ok(Unavailable)` is a record whose bytes or own metadata were refused.
    pub(crate) fn probe_at(data_home: &Path) -> Result<ControlState, Unreadable> {
        Ok(
            match read_owner_only_record(&data_home.join(CONTROL_DIR), CONTROL_RECORD)? {
                RecordRead::Absent => ControlState::Absent,
                RecordRead::Refused(reason) => ControlState::Unavailable(reason.to_owned()),
                RecordRead::Bytes(bytes) => Self::decode_record(&bytes),
            },
        )
    }

    /// Whether the record at `record` under `dir` is a durable stop: a `Disabled` record, or one [`Self::probe_at`] reads and refuses. `cache` keeps the record open while its path still names it unchanged, and the last bytes this decoded with their verdict; [`Self::decode_record`] reads nothing but the bytes, so a record with the same bytes reuses that verdict.
    ///
    /// # Errors
    ///
    /// As [`Self::probe_at`].
    pub(crate) fn probe_stop(
        dir: &Path,
        record: &Path,
        cache: Option<&mut ControlCache>,
    ) -> Result<bool, Unreadable> {
        let Some(cache) = cache else {
            return Ok(match read_owner_only_file(dir, record, None)? {
                RecordRead::Absent => false,
                RecordRead::Refused(_) => true,
                RecordRead::Bytes(bytes) => Self::decodes_stop(&bytes),
            });
        };
        let bytes = match read_owner_only_file(dir, record, Some(&mut cache.held))? {
            RecordRead::Absent => return Ok(false),
            RecordRead::Refused(_) => return Ok(true),
            RecordRead::Bytes(bytes) => bytes,
        };
        if let Some((decoded, stop)) = &cache.decoded
            && *decoded == bytes
        {
            return Ok(*stop);
        }
        let stop = Self::decodes_stop(&bytes);
        cache.decoded = Some((bytes, stop));
        Ok(stop)
    }

    fn decodes_stop(bytes: &[u8]) -> bool {
        matches!(
            Self::decode_record(bytes),
            ControlState::Disabled(_) | ControlState::Unavailable(_)
        )
    }

    /// The state a record's bytes hold; anything short of a complete record of this schema is [`ControlState::Unavailable`].
    fn decode_record(bytes: &[u8]) -> ControlState {
        match StoredIntent::decode(bytes) {
            Ok(StoredIntent::Current(record)) => {
                let intent = record.current;
                if record.schema != CURRENT_SCHEMA
                    || intent.validate().is_err()
                    || intent.recovery_target.is_none()
                    || intent
                        .recovery_target
                        .is_some_and(|target| target.commit_seq < 0)
                    || intent.episodes.allowance == 0
                    || intent.episodes.consumed > intent.episodes.allowance
                    || intent.recorded_at < 0
                    || intent.episodes.deadline < intent.recorded_at
                    || intent
                        .staged_seed_digest
                        .as_deref()
                        .is_none_or(|d| !is_canonical_payload_digest(d))
                    || intent.replacement_capture.is_some()
                    || intent.prior_disabled.is_some()
                {
                    ControlState::Unavailable("invalid completion record".to_owned())
                } else {
                    ControlState::Current(intent)
                }
            }
            Ok(StoredIntent::Disabled(intent)) => match intent.validate() {
                Ok(()) => ControlState::Disabled(intent),
                Err(_) => ControlState::Unavailable("invalid disabled record".to_owned()),
            },
            Ok(StoredIntent::Active(intent)) => match intent.validate() {
                Ok(()) => ControlState::Intent(intent),
                Err(reason) => ControlState::Unavailable(reason),
            },
            Err(_) => ControlState::Unavailable("malformed record".to_owned()),
        }
    }

    /// Reads projection generation pins for a reclaimer holding the lifecycle transaction lock.
    /// Unavailable intent blocks reclamation because its references are unknown.
    pub fn protected_generations(
        data_home: &Path,
        _transaction: &LifecycleTransactionLock,
    ) -> Result<BTreeSet<String>, IntentRefusal> {
        // A read-only probe must not create a control directory on hosts without a projection.
        match fs::symlink_metadata(data_home.join(CONTROL_DIR)) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
            Err(error) => return Err(io_refusal(error)),
            Ok(_) => {}
        }
        let lifecycle = Self::open(data_home).map_err(io_refusal)?;
        let _lock = lifecycle.lock().map_err(io_refusal)?;
        let intent = match lifecycle.read() {
            ControlState::Absent => return Ok(BTreeSet::new()),
            ControlState::Unavailable(reason) => return Err(IntentRefusal::Unavailable(reason)),
            ControlState::Intent(intent) | ControlState::Current(intent) => intent,
            ControlState::Disabled(disabled) => match disabled.handoff {
                Some(intent) => *intent,
                None => return Ok(BTreeSet::new()),
            },
        };
        let mut protected = BTreeSet::new();
        if let Some(digest) = intent.staged_seed_digest {
            if !is_canonical_payload_digest(&digest) {
                return Err(IntentRefusal::Unavailable(
                    "invalid staged digest".to_owned(),
                ));
            }
            protected.insert(digest);
        }
        if let Some(stage) = intent.replacement_capture.and_then(|capture| capture.stage) {
            // Hashing the manifest constructs this digest instead of accepting persisted text.
            protected.insert(stage.stage_manifest().digest());
        }
        Ok(protected)
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
        self.record_at(gate, request, now, EntryPoint::Reload)
    }

    /// [`Self::record`] with the hook admitted at `entry`: an operator's request is an [`EntryPoint::Explicit`] action.
    ///
    /// # Errors
    ///
    /// As [`Self::record`].
    pub fn record_at(
        &self,
        gate: &HookGate,
        request: &LifecycleRequest,
        now: i64,
        entry: EntryPoint,
    ) -> Result<Recorded, IntentRefusal> {
        check_request(request, now)?;
        let admission = gate
            .admit(request.transition.hook(), entry)
            .map_err(IntentRefusal::Denied)?;
        let _lock = self.lock().map_err(io_refusal)?;
        match self.read() {
            ControlState::Absent => {}
            ControlState::Current(existing)
                if existing.attempt_id != request.attempt_id
                    && existing.staged_seed_digest.as_deref()
                        == Some(&request.selected_generation)
                    && existing.consumer.consumer_id != request.consumer.consumer_id => {}
            ControlState::Current(existing) => {
                return Err(IntentRefusal::Conflict {
                    attempt_id: existing.attempt_id,
                });
            }
            ControlState::Disabled(_) => return Err(IntentRefusal::Disabled),
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
        let intent = new_intent(request, now);
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
        let (expected, _) = self.admitted_intent(gate)?;
        self.consume_expected_episode(gate, &expected, now)
    }

    pub(crate) fn consume_expected_episode(
        &self,
        gate: &HookGate,
        expected: &LifecycleIntent,
        now: i64,
    ) -> Result<EpisodeAccounting, IntentRefusal> {
        let _lock = self.lock().map_err(io_refusal)?;
        let (mut intent, admission) = self.admitted_intent(gate)?;
        if intent != *expected {
            return Err(IntentRefusal::Conflict {
                attempt_id: intent.attempt_id,
            });
        }
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
    /// Caller must hold the data home's lifecycle transaction lock through staging and pinning.
    ///
    /// # Errors
    ///
    /// Returns [`IntentRefusal::NoIntent`], [`IntentRefusal::Unavailable`], [`IntentRefusal::Denied`], [`IntentRefusal::Conflict`] with the recorded attempt when another seed is already pinned, [`IntentRefusal::Oversized`] when the pinned record would not fit once its allowance is consumed, or [`IntentRefusal::Revoked`] when the gate invalidated the admission before the write; the record is unchanged in every refused case.
    /// Noncanonical digests return [`IntentRefusal::InvalidDigest`]; a certificate mismatch returns [`IntentRefusal::Conflict`].
    pub fn pin_seed(
        &self,
        gate: &HookGate,
        _transaction: &LifecycleTransactionLock,
        digest: &str,
    ) -> Result<LifecycleIntent, IntentRefusal> {
        if !is_canonical_payload_digest(digest) {
            return Err(IntentRefusal::InvalidDigest);
        }
        let _lock = self.lock().map_err(io_refusal)?;
        let (mut intent, admission) = self.admitted_intent(gate)?;
        if let Some(stage) = intent
            .replacement_capture
            .as_ref()
            .and_then(|capture| capture.stage.as_ref())
            && stage.stage_manifest().digest() != digest
        {
            return Err(IntentRefusal::Conflict {
                attempt_id: intent.attempt_id,
            });
        }
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

    /// Fixes the recorded intent's recovery target under the gate's admission, for a transition recorded before its target was known. The same target again changes nothing; another target is refused, because a target moves only through a new intent.
    ///
    /// # Errors
    ///
    /// Returns [`IntentRefusal::NoIntent`], [`IntentRefusal::Unavailable`], [`IntentRefusal::Denied`], or [`IntentRefusal::Conflict`] with the recorded attempt when another target is already fixed.
    pub fn fix_target(
        &self,
        gate: &HookGate,
        target: RecoveryTarget,
    ) -> Result<LifecycleIntent, IntentRefusal> {
        if target.commit_seq < 0 {
            return Err(IntentRefusal::IllegalCombination);
        }
        let _lock = self.lock().map_err(io_refusal)?;
        let (mut intent, admission) = self.admitted_intent(gate)?;
        match intent.recovery_target {
            Some(fixed) if fixed == target => {
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
        intent.recovery_target = Some(target);
        fits_when_exhausted(&intent)?;
        self.replace(&intent, &admission)?;
        Ok(intent)
    }

    /// The gate admits the intent's transition before callers modify the control record or remove the disposable family.
    pub(crate) fn admitted_intent(
        &self,
        gate: &HookGate,
    ) -> Result<(LifecycleIntent, Admission), IntentRefusal> {
        let intent = match self.read() {
            ControlState::Absent => return Err(IntentRefusal::NoIntent),
            ControlState::Current(_) => return Err(IntentRefusal::NoIntent),
            ControlState::Disabled(_) => return Err(IntentRefusal::Disabled),
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
        #[cfg(feature = "test-support")]
        if self
            .fail_directory_sync
            .as_ref()
            .is_some_and(|flag| flag.swap(false, Ordering::AcqRel))
        {
            return Err(IntentRefusal::DurabilityUnknown("injected".to_owned()));
        }
        self.dir_fd
            .sync_all()
            .map_err(|error| IntentRefusal::DurabilityUnknown(error.kind().to_string()))
    }

    /// Keep `replacement_capture` until `delete_sqlite_family` succeeds.
    /// The storage primitive acquires its own exclusive lease after `release_lease` closes the owner.
    /// Errors can follow `release_lease`; record the relinquished owner inside `release_lease`.
    pub fn delete_replacement_family(
        &self,
        gate: &HookGate,
        expected_hold: Option<&str>,
        release_lease: impl FnOnce(),
    ) -> Result<(), IntentRefusal> {
        let _lock = self.lock().map_err(io_refusal)?;
        let (intent, admission) = self.admitted_intent(gate)?;
        if intent.staged_seed_digest.is_some()
            || intent
                .replacement_capture
                .as_deref()
                .map(|capture| capture.hold_id.as_str())
                != expected_hold
        {
            return Err(IntentRefusal::Conflict {
                attempt_id: intent.attempt_id,
            });
        }
        self.delete_private_family(
            || {
                if admission.invalidated.is_cancelled() {
                    Err(IntentRefusal::Revoked)
                } else {
                    Ok(())
                }
            },
            || {
                release_lease();
                Ok(())
            },
        )
    }

    pub(crate) fn delete_owned_replacement(
        &self,
        gate: &HookGate,
        expected: &ControlState,
        identity: &crate::projection_gates::InvalidationIdentity,
        prepare: impl FnOnce() -> Result<(), crate::search_replacement::BuildError>,
    ) -> Result<(), crate::search_replacement::BuildError> {
        let _lock = self.lock().map_err(io_refusal)?;
        let admission = match expected {
            ControlState::Intent(intent) => {
                Some(gate.admit(intent.transition.hook(), EntryPoint::Reload)?)
            }
            ControlState::Disabled(disabled) if disabled.handoff.is_some() => None,
            _ => return Err(IntentRefusal::NoIntent.into()),
        };
        self.delete_private_family(
            || {
                if self.read() != *expected {
                    return Err(IntentRefusal::Revoked.into());
                }
                match &admission {
                    Some(grant) => gate.check_limits(grant, identity, &[])?,
                    None => gate.cleanup_limits(identity, &[])?,
                }
                Ok(())
            },
            prepare,
        )
    }

    fn delete_private_family<E: From<IntentRefusal>>(
        &self,
        check: impl Fn() -> Result<(), E>,
        prepare: impl FnOnce() -> Result<(), E>,
    ) -> Result<(), E> {
        check()?;
        self.sync_directory()?;
        let home = self.data_home.join(CONTROL_DIR).join("replacement");
        let descriptor = search_descriptor(&home).map_err(store_refusal)?;
        self.at(WriteBarrier::BeforeFamilyRemoval);
        check()?;
        prepare()?;
        check()?;
        delete_sqlite_family(&descriptor)
            .map_err(store_refusal)
            .map_err(E::from)
    }

    /// Caller must hold the data home's lifecycle transaction lock while changing capture ownership.
    /// Before clearing a staged capture, finish its owned discard and private-family cleanup under that lock.
    pub fn record_capture(
        &self,
        gate: &HookGate,
        _transaction: &LifecycleTransactionLock,
        capture: Option<ReplacementCapture>,
        expected_hold: Option<&str>,
    ) -> Result<(), IntentRefusal> {
        let _lock = self.lock().map_err(io_refusal)?;
        let (mut intent, admission) = self.admitted_intent(gate)?;
        if intent.staged_seed_digest.is_none()
            && intent.replacement_capture.is_none()
            && capture.is_none()
        {
            // An absent capture is already cleared; sync the replay without rewriting it.
            self.sync_directory()?;
            return Ok(());
        }
        if intent.staged_seed_digest.is_some()
            || intent
                .replacement_capture
                .as_deref()
                .map(|capture| capture.hold_id.as_str())
                != expected_hold
        {
            return Err(IntentRefusal::Conflict {
                attempt_id: intent.attempt_id,
            });
        }
        if let (Some(old), Some(new)) = (intent.replacement_capture.as_deref(), capture.as_ref()) {
            // Comparing the clone keeps every non-stage field immutable, including added fields.
            let mut allowed = old.clone();
            allowed.stage = new.stage.clone();
            if allowed != *new
                || (old.stage.is_some() && old.stage != new.stage)
                || !new.stage_is_bound()
            {
                return Err(IntentRefusal::Conflict {
                    attempt_id: intent.attempt_id,
                });
            }
        } else if intent.replacement_capture.is_none()
            && capture
                .as_ref()
                .is_some_and(|capture| capture.stage.is_some())
        {
            return Err(IntentRefusal::IllegalCombination);
        }
        intent.replacement_capture = capture.map(Box::new);
        fits_when_exhausted(&intent)?;
        self.replace(&intent, &admission)
    }

    /// Writes the record to a fresh temp file, syncs it, renames it over the record, and syncs the directory.
    fn replace(
        &self,
        intent: &LifecycleIntent,
        admission: &Admission,
    ) -> Result<(), IntentRefusal> {
        let bytes = encode(intent)?;
        self.write_record(&bytes, Some(admission))
    }

    /// Stops admission without requiring a serving grant or discarding construction obligations.
    /// Replays sync the existing record and preserve its accounting.
    pub fn disable(&self, gate: &HookGate, now: i64) -> Result<DisabledIntent, IntentRefusal> {
        if now < 0 {
            return Err(IntentRefusal::InvalidTimestamp);
        }
        gate.disable();
        let _lock = self.lock().map_err(io_refusal)?;
        let handoff = match self.read() {
            ControlState::Disabled(intent) => {
                self.sync_directory()?;
                return Ok(intent);
            }
            ControlState::Intent(intent) | ControlState::Current(intent) => Some(Box::new(intent)),
            ControlState::Absent => None,
            ControlState::Unavailable(reason) => return Err(IntentRefusal::Unavailable(reason)),
        };
        let intent = DisabledIntent {
            schema: DISABLED_SCHEMA,
            handoff,
            recorded_at: now,
            episodes: None,
            through: None,
            deregistered: false,
        };
        self.write_disabled(&intent)?;
        Ok(intent)
    }

    pub(crate) fn update_disabled(
        &self,
        expected: &DisabledIntent,
        next: &DisabledIntent,
    ) -> Result<(), IntentRefusal> {
        let _lock = self.lock().map_err(io_refusal)?;
        if self.read() != ControlState::Disabled(expected.clone()) {
            return Err(IntentRefusal::Disabled);
        }
        self.write_disabled(next)
    }

    fn write_disabled(&self, intent: &DisabledIntent) -> Result<(), IntentRefusal> {
        let bytes =
            serde_json::to_vec(intent).map_err(|_| IntentRefusal::Io("encode".to_owned()))?;
        self.write_record(&bytes, None)
    }

    pub(crate) fn authorize_recovery(
        &self,
        gate: &HookGate,
        expected: &DisabledIntent,
        request: &LifecycleRequest,
        identity: &crate::projection_gates::InvalidationIdentity,
        now: i64,
    ) -> Result<(), IntentRefusal> {
        if request.transition != Transition::AuthorizedRecovery
            || expected
                .handoff
                .as_ref()
                .is_some_and(|old| old.attempt_id == request.attempt_id)
        {
            return Err(IntentRefusal::IllegalCombination);
        }
        let _lock = self.lock().map_err(io_refusal)?;
        match self.read() {
            ControlState::Disabled(disabled) if disabled == *expected => {
                check_request(request, now)?;
            }
            // A prior write renamed this authorization into place but its directory sync did not return; the replay syncs it and reopens admission.
            ControlState::Intent(existing)
                if existing.prior_disabled.is_some() && existing.is_replay_of(request) =>
            {
                return gate
                    .authorized_recovery(&self.data_home, identity, || self.sync_directory());
            }
            _ => return Err(IntentRefusal::Disabled),
        }
        let mut intent = new_intent(request, now);
        let mut prior = expected.clone();
        if let Some(handoff) = prior.handoff.as_deref_mut()
            && !handoff.prior_disabled.as_deref().is_some_and(|inner| {
                inner.deregistered
                    && inner
                        .handoff
                        .as_deref()
                        .and_then(|h| h.staged_seed_digest.as_deref())
                        == Some(request.selected_generation.as_str())
            })
        {
            handoff.prior_disabled = None;
        }
        intent.prior_disabled = Some(Box::new(prior));
        fits_when_exhausted(&intent)?;
        gate.authorized_recovery(&self.data_home, identity, || {
            self.write_record(&encode(&intent)?, None)
        })
    }

    pub(crate) fn finish_construction(
        &self,
        gate: &HookGate,
        expected: &LifecycleIntent,
    ) -> Result<LifecycleIntent, IntentRefusal> {
        let _lock = self.lock().map_err(io_refusal)?;
        let (mut intent, admission) = self.admitted_intent(gate)?;
        if intent != *expected {
            return Err(IntentRefusal::Conflict {
                attempt_id: intent.attempt_id,
            });
        }
        intent.replacement_capture = None;
        intent.prior_disabled = None;
        self.replace(&intent, &admission)?;
        Ok(intent)
    }

    pub(crate) fn complete(
        &self,
        gate: &HookGate,
        expected: &LifecycleIntent,
    ) -> Result<(), IntentRefusal> {
        let _lock = self.lock().map_err(io_refusal)?;
        let (intent, admission) = self.admitted_intent(gate)?;
        if intent != *expected || intent.replacement_capture.is_some() {
            return Err(IntentRefusal::Conflict {
                attempt_id: intent.attempt_id,
            });
        }
        let bytes = serde_json::to_vec(&CurrentIntent {
            schema: CURRENT_SCHEMA,
            current: intent,
        })
        .map_err(|_| IntentRefusal::Io("encode".to_owned()))?;
        self.write_record(&bytes, Some(&admission))
    }

    pub(crate) fn unpin_construction(
        &self,
        gate: &HookGate,
        expected: &LifecycleIntent,
    ) -> Result<(), IntentRefusal> {
        let _lock = self.lock().map_err(io_refusal)?;
        let (mut intent, admission) = self.admitted_intent(gate)?;
        if intent != *expected {
            return Err(IntentRefusal::Conflict {
                attempt_id: intent.attempt_id,
            });
        }
        intent.staged_seed_digest = None;
        self.replace(&intent, &admission)
    }

    fn write_record(
        &self,
        bytes: &[u8],
        admission: Option<&Admission>,
    ) -> Result<(), IntentRefusal> {
        let io = io_refusal;
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
            file.write_all(bytes)?;
            file.sync_all()?;
            Ok(())
        })();
        if let Err(error) = written {
            let _ = fs::remove_file(&temp);
            return Err(io(error));
        }
        self.at(WriteBarrier::BeforeRename);
        if admission.is_some_and(|grant| grant.invalidated.is_cancelled()) {
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

pub(crate) fn check_request(request: &LifecycleRequest, now: i64) -> Result<(), IntentRefusal> {
    check_invariants(
        &request.consumer,
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
    Ok(())
}

fn new_intent(request: &LifecycleRequest, now: i64) -> LifecycleIntent {
    LifecycleIntent {
        schema: ACTIVE_SCHEMA,
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
        replacement_capture: None,
        recorded_at: now,
        prior_disabled: None,
    }
}

fn encode(intent: &LifecycleIntent) -> Result<Vec<u8>, IntentRefusal> {
    serde_json::to_vec(intent).map_err(|_| IntentRefusal::Io("encode".to_owned()))
}

/// The hex SHA-256 a staged seed's digest takes; the record reserves room for one before any is pinned.
const DIGEST_HEX_LEN: usize = 64;

/// Reserves space for exhausted `episodes`, a staged seed digest, and a `recovery_target` so later valid updates do not make the intent [`IntentRefusal::Oversized`].
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
        recovery_target: Some(intent.recovery_target.unwrap_or(RecoveryTarget {
            commit_seq: i64::MAX,
        })),
        ..intent.clone()
    };
    // `disable` wraps the active record and cleanup later fills every optional field, so the accepted intent must fit in that form too.
    let disabled = DisabledIntent {
        schema: DISABLED_SCHEMA,
        handoff: Some(Box::new(exhausted)),
        recorded_at: i64::MAX,
        episodes: Some(EpisodeAccounting {
            allowance: u32::MAX,
            consumed: u32::MAX,
            deadline: i64::MAX,
        }),
        through: Some(i64::MAX),
        deregistered: false,
    };
    let bytes =
        serde_json::to_vec(&disabled).map_err(|_| IntentRefusal::Io("encode".to_owned()))?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
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

pub(crate) fn open_directory(dir: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags((OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC).bits() as i32)
        .open(dir)
}

/// What a gate keeps between admissions to read the control record: the record held open while its path still names it unchanged, and the last bytes it decoded with whether they hold a durable stop.
#[derive(Default)]
pub(crate) struct ControlCache {
    held: Option<HeldRecord>,
    decoded: Option<(Vec<u8>, bool)>,
}

/// Why a record could not be read: an I/O failure or a failed owner-only check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Unreadable(pub(crate) String);

/// What reading a record under an owner-only directory found. `Refused` names a durable defect of the record itself that no reopen repairs.
pub(crate) enum RecordRead {
    Absent,
    Refused(&'static str),
    Bytes(Vec<u8>),
}

/// Reads `record` under `dir` without following symlinks. The directory must be the caller's own and owner-only; the record must be the caller's own owner-only regular file under [`MAX_RECORD_BYTES`].
///
/// # Errors
///
/// An I/O failure, or a directory whose mode or owner is not the caller's own, is [`Unreadable`]: the directory's opener or next write repairs those, so a caller must not decide a durable stop from one.
pub(crate) fn read_owner_only_record(dir: &Path, record: &str) -> Result<RecordRead, Unreadable> {
    read_owner_only_file(dir, &dir.join(record), None)
}

/// [`read_owner_only_record`] for the record path `record` under `dir`.
fn read_owner_only_file(
    dir: &Path,
    record: &Path,
    held: Option<&mut Option<HeldRecord>>,
) -> Result<RecordRead, Unreadable> {
    let euid = rustix::process::geteuid().as_raw();
    let metadata = match fs::symlink_metadata(dir) {
        Ok(metadata)
            if owner_only_directory_of(&metadata, euid).is_ok()
                && metadata.mode() & 0o500 == 0o500 =>
        {
            metadata
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(RecordRead::Absent),
        _ => {
            let metadata = match open_directory(dir).and_then(|fd| fd.metadata()) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Ok(RecordRead::Absent);
                }
                Err(error) => return Err(Unreadable(error.kind().to_string())),
            };
            owner_only_directory_of(&metadata, euid)
                .map_err(|reason| Unreadable(reason.to_owned()))?;
            metadata
        }
    };
    let mut held = held;
    if let Some(slot) = held.as_deref_mut()
        && let Some(open) = slot.as_ref()
    {
        match fs::symlink_metadata(record) {
            Ok(current) if HeldKey::of(&current) == open.key => {
                return read_open_record(&open.file, &current, euid);
            }
            _ => *slot = None,
        }
    }
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags((OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK).bits() as i32)
        .open(record)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(RecordRead::Absent),
        // A directory without its owner's search bit opens but refuses its entries; `open` repairs that. Otherwise this is the record's own mode, which nothing repairs.
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            if metadata.mode() & 0o100 == 0 {
                return Err(Unreadable("directory not searchable".to_owned()));
            }
            return Ok(RecordRead::Refused("record not readable"));
        }
        Err(error) => return Err(Unreadable(error.kind().to_string())),
    };
    let metadata = match file.metadata() {
        Ok(metadata) => metadata,
        Err(error) => return Err(Unreadable(error.kind().to_string())),
    };
    let read = read_open_record(&file, &metadata, euid)?;
    if let (Some(slot), RecordRead::Bytes(_)) = (held, &read)
        && page_cache_coherent(&file)
    {
        *slot = Some(HeldRecord {
            key: HeldKey::of(&metadata),
            file,
        });
    }
    Ok(read)
}

/// The owner-only checks and the read of an open record whose metadata is `metadata`, from its first byte.
fn read_open_record(
    file: &File,
    metadata: &fs::Metadata,
    euid: u32,
) -> Result<RecordRead, Unreadable> {
    use std::os::unix::fs::FileExt;
    // Nothing repairs the record's own mode or owner, unlike the directory's, so this is a refused record rather than a transient failure.
    if !metadata.is_file() || metadata.mode() & 0o077 != 0 || metadata.uid() != euid {
        return Ok(RecordRead::Refused(
            "not the caller's own owner-only regular file",
        ));
    }
    if metadata.len() > MAX_RECORD_BYTES {
        return Ok(RecordRead::Refused("over the size cap"));
    }
    let len = metadata.len() as usize;
    let cap = MAX_RECORD_BYTES as usize;
    // The extra byte lets the first read detect growth beyond the metadata length, subject to `MAX_RECORD_BYTES`.
    let mut bytes = vec![0; (len + 1).min(cap)];
    let mut filled = 0;
    while filled < bytes.len() {
        match file.read_at(&mut bytes[filled..], filled as u64) {
            Ok(0) => break,
            Ok(read) => {
                filled += read;
                if filled == len {
                    break;
                }
                if filled == bytes.len() && filled < cap {
                    bytes.resize((filled * 2).min(cap), 0);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(Unreadable(error.kind().to_string())),
        }
    }
    bytes.truncate(filled);
    Ok(RecordRead::Bytes(bytes))
}

/// A control record a gate keeps open between admissions, and the metadata it was opened under.
pub(crate) struct HeldRecord {
    file: File,
    key: HeldKey,
}

/// The metadata a held record must still show at its path before it is read again: the same inode, file type and mode, owner, and change time. A chmod, chown, ACL or label change, or a link or rename of the inode updates the change time.
#[derive(PartialEq, Eq)]
struct HeldKey {
    dev: u64,
    ino: u64,
    mode: u32,
    uid: u32,
    ctime: i64,
    ctime_nsec: i64,
}

impl HeldKey {
    fn of(metadata: &fs::Metadata) -> Self {
        Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
            mode: metadata.mode(),
            uid: metadata.uid(),
            ctime: metadata.ctime(),
            ctime_nsec: metadata.ctime_nsec(),
        }
    }
}

/// Whether every read of `file` returns the bytes a fresh open of its inode would: a local filesystem whose page cache all readers share. Network, FUSE, and stacked filesystems revalidate on open or keep a reader on another inode, so their records are opened again for each read.
#[cfg(target_os = "linux")]
fn page_cache_coherent(file: &File) -> bool {
    const LOCAL: [u64; 4] = [
        0x0000_ef53, // ext2, ext3, ext4
        0x5846_5342, // XFS
        0x9123_683e, // Btrfs
        0x0102_1994, // tmpfs
    ];
    // `FsWord` is `c_long`; masking to 32 bits keeps magic values above `i32::MAX` intact on 32-bit targets.
    rustix::fs::fstatfs(file).is_ok_and(|fs| LOCAL.contains(&(fs.f_type as u64 & 0xffff_ffff)))
}

#[cfg(not(target_os = "linux"))]
fn page_cache_coherent(_file: &File) -> bool {
    false
}

/// Requires the caller's own directory with no group or other permission bits; every reader and the opener judge the directory by this one predicate.
pub(crate) fn owner_only_directory(metadata: &fs::Metadata) -> Result<(), &'static str> {
    owner_only_directory_of(metadata, rustix::process::geteuid().as_raw())
}

/// [`owner_only_directory`] for a caller whose effective uid is `euid`.
fn owner_only_directory_of(metadata: &fs::Metadata, euid: u32) -> Result<(), &'static str> {
    if !metadata.is_dir() {
        return Err("the lifecycle path is not a directory");
    }
    if metadata.uid() != euid {
        return Err("the lifecycle directory is not the caller's own");
    }
    if metadata.mode() & 0o077 != 0 {
        return Err("the lifecycle directory is not owner-only");
    }
    Ok(())
}

fn owned_by_caller(metadata: &fs::Metadata) -> bool {
    metadata.uid() == rustix::process::geteuid().as_raw()
}

fn io_refusal(error: io::Error) -> IntentRefusal {
    if error.kind() == io::ErrorKind::WouldBlock {
        IntentRefusal::WouldBlock
    } else {
        IntentRefusal::Io(error.kind().to_string())
    }
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

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{
        ACTIVE_SCHEMA, CURRENT_SCHEMA, Cause, ConsumerBinding, CurrentIntent, DISABLED_SCHEMA,
        DisabledIntent, EpisodeAccounting, LifecycleIntent, RecoveryTarget, StoredIntent,
        Transition,
    };

    fn active() -> LifecycleIntent {
        LifecycleIntent {
            schema: ACTIVE_SCHEMA,
            transition: Transition::Rebuilding,
            selected_generation: "generation".to_owned(),
            kernel_incarnation_id: "kernel".to_owned(),
            consumer: ConsumerBinding {
                consumer_id: "consumer".to_owned(),
                generation_id: "generation".to_owned(),
            },
            cause: Cause::Registration,
            attempt_id: "attempt".to_owned(),
            recovery_target: Some(RecoveryTarget { commit_seq: 3 }),
            episodes: EpisodeAccounting {
                allowance: 2,
                consumed: 1,
                deadline: 9,
            },
            authorization_ref: None,
            staged_seed_digest: None,
            replacement_capture: None,
            recorded_at: 1,
            prior_disabled: None,
        }
    }

    /// The variant and content a decode yields; every decode error reads the same, as `probe_at` reports each as one malformed record.
    fn decoded(result: serde_json::Result<StoredIntent>) -> Option<(&'static str, Value)> {
        Some(match result.ok()? {
            StoredIntent::Active(intent) => ("active", serde_json::to_value(intent).unwrap()),
            StoredIntent::Disabled(intent) => ("disabled", serde_json::to_value(intent).unwrap()),
            StoredIntent::Current(intent) => ("current", serde_json::to_value(intent).unwrap()),
        })
    }

    /// Each stored record, each with a key removed, retyped, or added, records naming another variant's distinguishing key, and inputs that are not one JSON object.
    fn corpus() -> Vec<Vec<u8>> {
        let disabled = DisabledIntent {
            schema: DISABLED_SCHEMA,
            handoff: Some(Box::new(active())),
            recorded_at: 2,
            episodes: None,
            through: Some(4),
            deregistered: true,
        };
        let current = CurrentIntent {
            schema: CURRENT_SCHEMA,
            current: active(),
        };
        let bases = [
            serde_json::to_value(active()).unwrap(),
            serde_json::to_value(disabled).unwrap(),
            serde_json::to_value(current).unwrap(),
        ];
        let mut values = Vec::new();
        for base in &bases {
            values.push(base.clone());
            let fields = base.as_object().unwrap();
            values.push(Value::Array(fields.values().cloned().collect()));
            for key in fields.keys() {
                let mut removed = base.clone();
                removed.as_object_mut().unwrap().remove(key);
                values.push(removed);
                for replacement in [json!(null), json!("x"), json!(4.0), json!(-1), json!([])] {
                    let mut retyped = base.clone();
                    retyped[key] = replacement;
                    values.push(retyped);
                }
            }
            for (key, value) in [
                ("current", serde_json::to_value(active()).unwrap()),
                ("deregistered", json!(false)),
                ("unknown", json!(1)),
            ] {
                let mut added = base.clone();
                added[key] = value;
                values.push(added);
            }
        }
        let mut corpus: Vec<Vec<u8>> = values
            .iter()
            .flat_map(|value| {
                [
                    serde_json::to_vec(value).unwrap(),
                    serde_json::to_vec_pretty(value).unwrap(),
                ]
            })
            .collect();
        let current = serde_json::to_string(&bases[2]).unwrap();
        let disabled = serde_json::to_string(&bases[1]).unwrap();
        corpus.extend(
            [
                current.replacen("\"current\"", "\"curr\\u0065nt\"", 1),
                disabled.replacen("\"deregistered\"", "\"d\\u0065registered\"", 1),
                current.replacen('{', "{\"schema\":4,", 1),
                disabled.replacen('{', "{\"deregistered\":true,", 1),
                format!("{current} "),
                format!("{current}{{}}"),
                format!("[{current}]"),
                current[..current.len() - 1].to_owned(),
                "{}".to_owned(),
                "null".to_owned(),
                "4".to_owned(),
                String::new(),
            ]
            .map(String::into_bytes),
        );
        corpus
    }

    #[test]
    fn decode_selects_the_variant_the_untagged_enum_selects() {
        let mut variants = std::collections::BTreeSet::new();
        for bytes in corpus() {
            let expected = decoded(serde_json::from_slice::<StoredIntent>(&bytes));
            assert_eq!(
                decoded(StoredIntent::decode(&bytes)),
                expected,
                "{}",
                String::from_utf8_lossy(&bytes)
            );
            variants.extend(expected.map(|(variant, _)| variant));
        }
        assert_eq!(
            variants.into_iter().collect::<Vec<_>>(),
            ["active", "current", "disabled"]
        );
    }

    /// `probe_stop` with a gate's cache agrees with `probe_at` on every record state a gate can meet: absent, current, rewritten in place or by rename, disabled, refused by mode or owner-only checks, malformed, unlinked, and replaced by a symlink.
    #[test]
    fn a_cached_stop_verdict_follows_the_record() {
        use std::io::Write;
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

        use super::{
            CONTROL_DIR, CONTROL_RECORD, ControlCache, ControlState, ProjectionLifecycle,
            page_cache_coherent,
        };

        let root = tempfile::tempdir().unwrap();
        let home = root.path();
        let dir = home.join(CONTROL_DIR);
        let record = dir.join(CONTROL_RECORD);
        let mut completed = active();
        completed.staged_seed_digest = Some("a".repeat(64));
        let current = serde_json::to_vec(&CurrentIntent {
            schema: CURRENT_SCHEMA,
            current: completed.clone(),
        })
        .unwrap();
        completed.attempt_id = "another-attempt".to_owned();
        let rewritten = serde_json::to_vec(&CurrentIntent {
            schema: CURRENT_SCHEMA,
            current: completed,
        })
        .unwrap();
        let disabled = serde_json::to_vec(&DisabledIntent {
            schema: DISABLED_SCHEMA,
            handoff: Some(Box::new(active())),
            recorded_at: 2,
            episodes: None,
            through: None,
            deregistered: false,
        })
        .unwrap();
        let renamed = |bytes: &[u8]| {
            let staged = dir.join("staged");
            std::fs::write(&staged, bytes).unwrap();
            std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o600)).unwrap();
            std::fs::rename(&staged, &record).unwrap();
        };
        let in_place = |bytes: &[u8]| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&record)
                .unwrap();
            file.write_all(bytes).unwrap();
        };
        let mode = |mode: u32| {
            std::fs::set_permissions(&record, std::fs::Permissions::from_mode(mode)).unwrap();
        };
        let mut cache = ControlCache::default();
        let mut check = |expected: Option<bool>| {
            let stop = ProjectionLifecycle::probe_stop(&dir, &record, Some(&mut cache)).ok();
            let state = ProjectionLifecycle::probe_at(home);
            let reference = state.as_ref().ok().map(|state| {
                matches!(
                    state,
                    ControlState::Disabled(_) | ControlState::Unavailable(_)
                )
            });
            assert_eq!(stop, reference, "{state:?}");
            assert_eq!(stop, expected, "{state:?}");
            cache.held.is_some()
        };

        check(Some(false));
        std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        check(Some(false));
        renamed(&current);
        check(Some(false));
        let held = check(Some(false));
        let coherent = page_cache_coherent(&std::fs::File::open(&record).unwrap());
        assert_eq!(held, coherent);
        in_place(&disabled);
        check(Some(true));
        in_place(&current);
        check(Some(false));
        renamed(&rewritten);
        check(Some(false));
        renamed(&disabled);
        check(Some(true));
        renamed(&current);
        check(Some(false));
        mode(0o644);
        check(Some(true));
        // The owner cannot read a write-only record unless the daemon runs with the capability to override that, so the verdict here is `probe_at`'s alone.
        mode(0o200);
        let unreadable = ProjectionLifecycle::probe_at(home).map(|state| {
            matches!(
                state,
                ControlState::Disabled(_) | ControlState::Unavailable(_)
            )
        });
        check(unreadable.ok());
        mode(0o600);
        check(Some(false));
        in_place(&current[..current.len() - 1]);
        check(Some(true));
        in_place(&current);
        check(Some(false));
        std::fs::remove_file(&record).unwrap();
        check(Some(false));
        let target = dir.join("target");
        std::fs::write(&target, &current).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&target, &record).unwrap();
        check(None);
        std::fs::remove_file(&record).unwrap();
        renamed(&current);
        check(Some(false));
    }
}
