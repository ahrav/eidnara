//! Content-addressed artifact types, limits, layout, and fail-closed state.
//!
//! Payload limits are byte counts. The store latches non-capacity storage
//! failures with release/acquire ordering so later ingestion fails closed.

mod deletion;
pub(super) mod gc;
mod ingest;
mod read;

use std::fmt;
use std::fs::File;
use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use rustix::fs::{self as rfs, AtFlags};

use super::{CommitIntent, RepositoryProvenance, Sensitivity};
use crate::durable_fs::{StorageError, open_or_create_secure_directory, open_secure_directory};
use crate::{KernelError, KernelStore};

/// Default total artifact capacity in bytes.
pub(super) const DEFAULT_ARTIFACT_CAP: u64 = 4 * 1024 * 1024 * 1024;
/// Maximum payload accepted by one ingest, in bytes. Ingestion accepts exactly
/// this many bytes and rejects one more with `PayloadTooLarge`.
pub const MAX_PAYLOAD_BYTES: usize = 64 * 1024 * 1024;
/// Maximum recognized-secret findings accepted in one payload, counted before
/// overlapping findings merge into detections; the scan stops at the cap.
pub(super) const MAX_PAYLOAD_DETECTIONS: usize = 4096;
/// Maximum UTF-8 byte length of each artifact text field.
pub(super) const MAX_TEXT_FIELD_BYTES: usize = 1024;

#[cfg(feature = "test-support")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactIngestFault {
    Write,
    FileSync,
    ReservationCommit,
    Rename,
    AfterDirectorySync,
    /// Fails after the directory sync, and while the failed reference is being
    /// cleaned up another connection tries to raise the durable fence between
    /// the fence check and the unlink of the published object.
    TakeoverBeforeCleanupUnlink,
    AfterEvents,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactIngestHook {
    AfterReservation,
    AfterPublish,
}

/// Injection flags carried through ingest so the private path keeps one signature
/// in every feature configuration. Production passes `Default`, which folds away.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct IngestFaults {
    pub write: bool,
    pub file_sync: bool,
    pub reservation_commit: bool,
    pub rename: bool,
    pub after_directory_sync: bool,
    pub takeover_before_cleanup_unlink: bool,
    pub after_events: bool,
}

#[cfg(feature = "test-support")]
impl From<ArtifactIngestFault> for IngestFaults {
    fn from(fault: ArtifactIngestFault) -> Self {
        Self {
            write: fault == ArtifactIngestFault::Write,
            file_sync: fault == ArtifactIngestFault::FileSync,
            reservation_commit: fault == ArtifactIngestFault::ReservationCommit,
            rename: fault == ArtifactIngestFault::Rename,
            after_directory_sync: matches!(
                fault,
                ArtifactIngestFault::AfterDirectorySync
                    | ArtifactIngestFault::TakeoverBeforeCleanupUnlink
            ),
            takeover_before_cleanup_unlink: fault
                == ArtifactIngestFault::TakeoverBeforeCleanupUnlink,
            after_events: fault == ArtifactIngestFault::AfterEvents,
        }
    }
}

/// Most permissive destination class allowed by a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderEgress {
    RemoteAllowed,
    LocalOnly,
}

impl ProviderEgress {
    /// Egress proofs derive provider classes from this list.
    pub const ALL: &'static [Self] = &[Self::RemoteAllowed, Self::LocalOnly];

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::RemoteAllowed => "remote_allowed",
            Self::LocalOnly => "local_only",
        }
    }

    pub(super) fn from_stored(value: &str) -> Self {
        match value {
            "remote_allowed" => Self::RemoteAllowed,
            _ => Self::LocalOnly,
        }
    }

    #[must_use = "returns the stricter label; dropping it keeps the weaker one"]
    pub(super) fn restrictive(self, other: Self) -> Self {
        if self == Self::LocalOnly || other == Self::LocalOnly {
            Self::LocalOnly
        } else {
            Self::RemoteAllowed
        }
    }
}

/// Complete metadata and payload for one atomic artifact ingest.
///
/// Validation rejects oversized payloads, oversized text fields, negative source
/// revisions, and negative retention deadlines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactIngestRequest {
    pub intent: CommitIntent,
    pub payload: Vec<u8>,
    pub evidence_id: String,
    pub object_id: String,
    pub object_kind: String,
    pub domain_id: String,
    pub source_kind: String,
    pub source_id: String,
    /// Nonnegative revision supplied by the source system.
    pub source_revision: i64,
    pub media_type: String,
    pub retention_class: String,
    /// Optional absolute retention deadline in milliseconds.
    pub retain_until: Option<i64>,
    pub asserted_sensitivity: Sensitivity,
    pub provider_egress: ProviderEgress,
    pub provenance: Option<RepositoryProvenance>,
}

/// Stable identifiers returned after artifact bytes and evidence are committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactHandle {
    pub digest: String,
    pub evidence_id: String,
}

/// Destination considered by artifact egress eligibility checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArtifactDestination {
    Local,
    Remote,
}

impl ArtifactDestination {
    /// Egress proofs derive destinations from this list.
    pub const ALL: &'static [Self] = &[Self::Local, Self::Remote];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EligibilityDeniedReason {
    UnknownSensitive,
    SensitiveRemote,
    ProviderRestricted,
    Secret,
    Tombstoned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactEligibility {
    Allowed,
    Denied(EligibilityDeniedReason),
}

/// An egress verdict and the class it was derived from, read in one snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArtifactEgressFacts {
    pub eligibility: ArtifactEligibility,
    /// The restrictive fold over every live reference to the digest; `None`
    /// when no live reference exists or the digest is tombstoned.
    pub stored_class: Option<(Sensitivity, ProviderEgress)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactErrorKind {
    PayloadTooLarge,
    Capacity,
    StorageExhausted,
    IngestionFailClosed,
    ReAdmissionBlocked,
    MissingObject,
    CorruptObject,
    ReferenceUnavailable,
    ReferenceCommit,
    AlignmentRebuild,
    ReclaimInProgress,
    UnredactableSecret,
    /// The secret scan stopped before covering the whole payload, so the payload is refused unscanned rather than stored unproven.
    ScanIncomplete,
    DetectionLimit,
    TextFieldTooLong,
    InvalidInput,
    /// The intent's `operation_key` already carries a receipt for a different
    /// `request_digest`; no retry with this key can succeed.
    OperationKeyReused,
    /// A database constraint rejected the reference: an `object_id` or
    /// `evidence_id` the registry already holds, or a reference to a row that
    /// does not exist. Retrying the same request cannot succeed.
    StorageConstraint,
    PurgeIntent,
    PurgeUnlinkPending,
    /// An exact ingest found a recognized secret. Redaction would store bytes other than the ones
    /// offered, so the payload is refused instead; the reason names no content.
    ExactBytesRewritten,
    /// An exact ingest received bytes that are not valid UTF-8 text.
    UnsupportedShape,
    /// The digest names a stored object whose bytes differ from the payload. Nothing is replaced;
    /// the offered bytes are refused.
    DigestCollision,
}

impl ArtifactErrorKind {
    /// Every variant, in declaration order, so consumers can prove they map
    /// each one. Adding a variant fails `all_names_every_variant_once` to
    /// compile until its match is extended, which is where this list is
    /// revisited.
    pub const ALL: &'static [Self] = &[
        Self::PayloadTooLarge,
        Self::Capacity,
        Self::StorageExhausted,
        Self::IngestionFailClosed,
        Self::ReAdmissionBlocked,
        Self::MissingObject,
        Self::CorruptObject,
        Self::ReferenceUnavailable,
        Self::ReferenceCommit,
        Self::AlignmentRebuild,
        Self::ReclaimInProgress,
        Self::UnredactableSecret,
        Self::ScanIncomplete,
        Self::DetectionLimit,
        Self::TextFieldTooLong,
        Self::InvalidInput,
        Self::OperationKeyReused,
        Self::StorageConstraint,
        Self::PurgeIntent,
        Self::PurgeUnlinkPending,
        Self::ExactBytesRewritten,
        Self::UnsupportedShape,
        Self::DigestCollision,
    ];
}

/// Whether ingestion may store bytes other than the ones offered. `Redacting` replaces recognized
/// secrets in text payloads with placeholders and stores the result; `Exact` stores the offered
/// UTF-8 text unchanged or refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PayloadFidelity {
    Redacting,
    Exact,
}

/// Bounded artifact failure with optional capacity or digest context.
///
/// Display and debug output expose no payload bytes.
#[derive(thiserror::Error)]
#[error("{}", artifact_error_message(.kind, .usage, .cap, .digest))]
pub struct ArtifactError {
    kind: ArtifactErrorKind,
    usage: Option<u64>,
    cap: Option<u64>,
    digest: Option<String>,
}

impl ArtifactError {
    pub fn kind(&self) -> ArtifactErrorKind {
        self.kind
    }

    pub fn usage(&self) -> Option<u64> {
        self.usage
    }

    pub fn cap(&self) -> Option<u64> {
        self.cap
    }

    pub fn digest(&self) -> Option<&str> {
        self.digest.as_deref()
    }

    pub fn is_retriable(&self) -> bool {
        matches!(
            self.kind,
            ArtifactErrorKind::Capacity
                | ArtifactErrorKind::StorageExhausted
                | ArtifactErrorKind::ReclaimInProgress
                | ArtifactErrorKind::PurgeUnlinkPending
        )
    }

    pub(super) fn new(kind: ArtifactErrorKind) -> Self {
        Self {
            kind,
            usage: None,
            cap: None,
            digest: None,
        }
    }

    pub(super) fn capacity(usage: u64, cap: u64) -> Self {
        Self {
            kind: ArtifactErrorKind::Capacity,
            usage: Some(usage),
            cap: Some(cap),
            digest: None,
        }
    }

    pub(super) fn for_digest(kind: ArtifactErrorKind, digest: &str) -> Self {
        Self {
            kind,
            usage: None,
            cap: None,
            digest: Some(digest.to_string()),
        }
    }
}

/// `Display` and `Debug` for `ArtifactError` both write through this type into the
/// caller's `Formatter`, which avoids building an owned `String` per rendering.
struct ArtifactErrorMessage<'a> {
    kind: &'a ArtifactErrorKind,
    usage: &'a Option<u64>,
    cap: &'a Option<u64>,
    digest: &'a Option<String>,
}

fn artifact_error_message<'a>(
    kind: &'a ArtifactErrorKind,
    usage: &'a Option<u64>,
    cap: &'a Option<u64>,
    digest: &'a Option<String>,
) -> ArtifactErrorMessage<'a> {
    ArtifactErrorMessage {
        kind,
        usage,
        cap,
        digest,
    }
}

impl fmt::Display for ArtifactErrorMessage<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            ArtifactErrorKind::PayloadTooLarge => write!(
                formatter,
                "artifact payload exceeds {MAX_PAYLOAD_BYTES} bytes"
            ),
            ArtifactErrorKind::Capacity => write!(
                formatter,
                "artifact capacity exceeded (usage={}, cap={})",
                self.usage.unwrap_or(0),
                self.cap.unwrap_or(0)
            ),
            ArtifactErrorKind::StorageExhausted => {
                formatter.write_str("artifact storage capacity is exhausted")
            }
            ArtifactErrorKind::IngestionFailClosed => {
                formatter.write_str("artifact ingestion is fail-closed until the store is reopened")
            }
            ArtifactErrorKind::ReAdmissionBlocked => write!(
                formatter,
                "artifact re-admission is blocked for digest {}",
                self.digest.as_deref().unwrap_or("unknown")
            ),
            ArtifactErrorKind::MissingObject => write!(
                formatter,
                "artifact object is missing for digest {}",
                self.digest.as_deref().unwrap_or("unknown")
            ),
            ArtifactErrorKind::CorruptObject => write!(
                formatter,
                "artifact object hash mismatch for digest {}",
                self.digest.as_deref().unwrap_or("unknown")
            ),
            ArtifactErrorKind::ReferenceUnavailable => {
                formatter.write_str("artifact reference is not live")
            }
            ArtifactErrorKind::ReferenceCommit => {
                formatter.write_str("artifact canonical reference commit failed")
            }
            ArtifactErrorKind::AlignmentRebuild => {
                formatter.write_str("artifact deletion could not rebuild the alignment projection")
            }
            ArtifactErrorKind::ReclaimInProgress => {
                formatter.write_str("artifact reclamation is in progress; retry ingestion")
            }
            ArtifactErrorKind::UnredactableSecret => formatter
                .write_str("artifact payload holds a recognized secret that cannot be redacted"),
            ArtifactErrorKind::ScanIncomplete => {
                formatter.write_str("artifact payload could not be fully scanned for secrets")
            }
            ArtifactErrorKind::TextFieldTooLong => write!(
                formatter,
                "artifact text field exceeds {MAX_TEXT_FIELD_BYTES} bytes"
            ),
            ArtifactErrorKind::DetectionLimit => write!(
                formatter,
                "artifact payload exceeds {MAX_PAYLOAD_DETECTIONS} recognized secrets"
            ),
            ArtifactErrorKind::InvalidInput => formatter.write_str("artifact input is invalid"),
            ArtifactErrorKind::PurgeIntent => {
                formatter.write_str("artifact purge intent could not be made durable")
            }
            ArtifactErrorKind::PurgeUnlinkPending => {
                formatter.write_str("artifact purge committed with durable unlink pending")
            }
            ArtifactErrorKind::OperationKeyReused => {
                formatter.write_str("operation key already used with a different request digest")
            }
            ArtifactErrorKind::StorageConstraint => {
                formatter.write_str("artifact reference violates a storage constraint")
            }
            ArtifactErrorKind::ExactBytesRewritten => formatter.write_str(
                "artifact payload holds a recognized secret and exact retention refuses to rewrite it",
            ),
            ArtifactErrorKind::UnsupportedShape => {
                formatter.write_str("artifact payload is not valid UTF-8 text")
            }
            ArtifactErrorKind::DigestCollision => write!(
                formatter,
                "artifact digest {} names stored bytes that differ from the payload",
                self.digest.as_deref().unwrap_or("unknown")
            ),
        }
    }
}

#[cfg(feature = "test-support")]
pub use deletion::{ArtifactDeletionFault, ArtifactDeletionHook};
pub use deletion::{
    ArtifactDeletionIdentity, ArtifactDeletionKind, ArtifactDeletionRequest,
    ArtifactDeletionResult, BarrierConsumerStatus, DeletionBarrierStatus,
};
#[cfg(feature = "test-support")]
pub use gc::ArtifactGcFault;
pub use gc::ArtifactGcResult;
pub(crate) use ingest::is_exact_retention;
pub(crate) use read::egress_facts_tx;

impl fmt::Debug for ArtifactError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

/// The artifact tree's two mutable children, opened `NOFOLLOW` below the held
/// store root when the store opens and held for its lifetime.
pub(super) struct ArtifactDirectories {
    pub(super) objects: File,
    pub(super) tmp: File,
}

/// Creates or opens the artifact tree below the held store root and returns the
/// descriptors the store keeps for its lifetime. Every later object and staging
/// path resolves below one of them: a same-UID process that renames `objects`
/// and creates another owner-only directory in its place cannot receive an
/// ingest whose reference commits against the original tree.
/// The device and inode that identify an open directory.
fn directory_identity(directory: &File) -> Result<(u64, u64), StorageError> {
    use std::os::unix::fs::MetadataExt;
    let metadata = directory.metadata().map_err(StorageError::Other)?;
    Ok((metadata.dev(), metadata.ino()))
}

pub(super) fn prepare_layout(root_directory: &File) -> Result<ArtifactDirectories, KernelError> {
    let artifacts = open_or_create_secure_directory(root_directory, "artifacts")
        .map_err(|_| KernelError::Io)?;
    let objects =
        open_or_create_secure_directory(&artifacts, "objects").map_err(|_| KernelError::Io)?;
    let tmp = open_or_create_secure_directory(&artifacts, "tmp").map_err(|_| KernelError::Io)?;
    // Enumerated through the descriptor just opened, like every other walk of the
    // artifact tree, so a same-UID swap of `artifacts` or `tmp` for a symlink
    // cannot redirect the sweep.
    for entry in rfs::Dir::read_from(&tmp).map_err(|_| KernelError::Io)? {
        let entry = entry.map_err(|_| KernelError::Io)?;
        let name = entry.file_name();
        if ingest::is_dot_entry(name) {
            continue;
        }
        let stat = match rfs::statat(&tmp, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => stat,
            Err(rustix::io::Errno::NOENT) => continue,
            Err(_) => return Err(KernelError::Io),
        };
        let kind = rfs::FileType::from_raw_mode(stat.st_mode);
        if kind.is_file() || kind.is_symlink() {
            let Ok(name) = name.to_str() else {
                continue;
            };
            crate::durable_fs::durable_unlink(&tmp, name).map_err(|_| KernelError::Io)?;
        }
    }
    Ok(ArtifactDirectories { objects, tmp })
}

impl KernelStore {
    /// The held descriptor for the shard `digest` lives in, opened below the
    /// held `objects` on first use and shared thereafter. `None` when the shard
    /// does not exist and `create` is false. A shard swapped for another
    /// owner-only directory after first use is not consulted again: the bytes
    /// and the reference committed against them stay in the directory this
    /// store holds.
    pub(super) fn shard_directory(
        &self,
        digest: &str,
        create: bool,
    ) -> Result<Option<Arc<File>>, StorageError> {
        let name = &digest[..2];
        let mut shards = self
            .shard_directories
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(shard) = shards.get(name) {
            return Ok(Some(Arc::clone(shard)));
        }
        let opened = if create {
            open_or_create_secure_directory(&self.objects_directory, name)
        } else {
            match open_secure_directory(&self.objects_directory, name) {
                Err(StorageError::Other(source))
                    if source.kind() == std::io::ErrorKind::NotFound =>
                {
                    return Ok(None);
                }
                other => other,
            }
        };
        let shard = Arc::new(opened?);
        // A shard renamed to another two-character name would be reached here
        // under that name while its objects keep the digests of the name it was
        // created under. The directory this store already holds is the only
        // identity it trusts, so an inode retained under another name is refused
        // rather than opened twice.
        let identity = directory_identity(&shard)?;
        for held in shards.values() {
            if directory_identity(held)? == identity {
                return Err(StorageError::Other(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "shard directory is already held under another name",
                )));
            }
        }
        shards.insert(name.to_string(), Arc::clone(&shard));
        Ok(Some(shard))
    }

    pub(super) fn cas_is_failed(&self) -> bool {
        self.cas_failed.load(Ordering::Acquire)
    }

    pub(super) fn latch_cas_failure(&self) {
        self.cas_failed.store(true, Ordering::Release);
    }

    pub(super) fn map_cas_storage_error(
        &self,
        error: StorageError,
        non_capacity_kind: ArtifactErrorKind,
    ) -> ArtifactError {
        match error {
            StorageError::Exhausted(_) => ArtifactError::new(ArtifactErrorKind::StorageExhausted),
            StorageError::Other(_) => {
                self.latch_cas_failure();
                ArtifactError::new(non_capacity_kind)
            }
        }
    }
}

pub(super) fn read_capped(source: impl std::io::Read) -> std::io::Result<Option<Vec<u8>>> {
    let limit = u64::try_from(MAX_PAYLOAD_BYTES)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut bytes = Vec::new();
    source.take(limit).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_PAYLOAD_BYTES {
        return Ok(None);
    }
    Ok(Some(bytes))
}

pub(crate) fn is_artifact_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod error_kind_tests {
    use super::ArtifactErrorKind;

    #[test]
    fn all_names_every_variant_once() {
        for kind in ArtifactErrorKind::ALL {
            // The wildcard-free match forces a new arm, and with it a review
            // of `ALL`, whenever a variant is added.
            match kind {
                ArtifactErrorKind::PayloadTooLarge
                | ArtifactErrorKind::Capacity
                | ArtifactErrorKind::StorageExhausted
                | ArtifactErrorKind::IngestionFailClosed
                | ArtifactErrorKind::ReAdmissionBlocked
                | ArtifactErrorKind::MissingObject
                | ArtifactErrorKind::CorruptObject
                | ArtifactErrorKind::ReferenceUnavailable
                | ArtifactErrorKind::ReferenceCommit
                | ArtifactErrorKind::AlignmentRebuild
                | ArtifactErrorKind::ReclaimInProgress
                | ArtifactErrorKind::UnredactableSecret
                | ArtifactErrorKind::ScanIncomplete
                | ArtifactErrorKind::DetectionLimit
                | ArtifactErrorKind::TextFieldTooLong
                | ArtifactErrorKind::InvalidInput
                | ArtifactErrorKind::OperationKeyReused
                | ArtifactErrorKind::StorageConstraint
                | ArtifactErrorKind::PurgeIntent
                | ArtifactErrorKind::PurgeUnlinkPending
                | ArtifactErrorKind::ExactBytesRewritten
                | ArtifactErrorKind::UnsupportedShape
                | ArtifactErrorKind::DigestCollision => {}
            }
        }
        for (index, kind) in ArtifactErrorKind::ALL.iter().enumerate() {
            assert!(!ArtifactErrorKind::ALL[..index].contains(kind));
        }
    }
}

#[cfg(test)]
mod lattice_tests {
    use super::ProviderEgress;
    use crate::envelope::Sensitivity;

    #[test]
    fn sensitivity_merge_keeps_the_more_restrictive_class() {
        use Sensitivity::{Normal, Secret, Sensitive};
        for (left, right, expected) in [
            (Normal, Normal, Normal),
            (Normal, Sensitive, Sensitive),
            (Normal, Secret, Secret),
            (Sensitive, Normal, Sensitive),
            (Sensitive, Sensitive, Sensitive),
            (Sensitive, Secret, Secret),
            (Secret, Normal, Secret),
            (Secret, Sensitive, Secret),
            (Secret, Secret, Secret),
        ] {
            assert_eq!(left.restrictive(right), expected, "{left:?} + {right:?}");
        }
    }

    #[test]
    fn egress_merge_keeps_the_more_restrictive_class() {
        use ProviderEgress::{LocalOnly, RemoteAllowed};
        for (left, right, expected) in [
            (RemoteAllowed, RemoteAllowed, RemoteAllowed),
            (RemoteAllowed, LocalOnly, LocalOnly),
            (LocalOnly, RemoteAllowed, LocalOnly),
            (LocalOnly, LocalOnly, LocalOnly),
        ] {
            assert_eq!(left.restrictive(right), expected, "{left:?} + {right:?}");
        }
    }

    #[test]
    fn unrecognized_stored_classes_read_back_restrictive() {
        assert_eq!(Sensitivity::from_stored("internal"), Sensitivity::Secret);
        assert_eq!(
            ProviderEgress::from_stored("internal"),
            ProviderEgress::LocalOnly
        );
    }
}
