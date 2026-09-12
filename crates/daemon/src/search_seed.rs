//! A closed, journal-complete copy of the search projection: the seed a replacement is built from. Quiescing takes the projection's last handle, checkpoints and truncates its WAL, closes the connection while keeping the database lease, and verifies the closed file on its own: no sidecar left, integrity, foreign keys, identity, generation, checkpoint, rows, tombstones, work, and vectors. Staging copies the verified bytes into the host lifecycle store under a manifest whose identity slots carry the seed's compatibility identity, its verification report, and its digest, so the staged object is bound to exactly the bytes that were verified. Staging selects nothing, and the store reclaims a staged seed like any other unprotected generation unless its digest is pinned.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::num::NonZeroU32;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use host_runtime::generation::{GenerationError, GenerationStore, SourceSpec, StageMeta};
use kernel::applicability::EvalBudget;
use retrieval::ProjectionIdentity;
use rusqlite::{Connection, OpenFlags};
use rustix::fs::OFlags;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use storage::immutable_uri;

use crate::projection_gates::{EntryPoint, HookGate, ProjectionHook};
use crate::search_projection::SearchProjection;
use crate::search_writer::Quarantine;

/// The manifest target every staged search seed carries; host generations carry their own.
pub const SEED_TARGET: &str = "search-projection-seed";
pub const SEED_FILE: &str = "search.sqlite";
pub const SEED_REPORT_FILE: &str = "seed-report.json";
const REPORT_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeedBounds {
    /// Checkpoints attempted before a held-back log is reported blocked.
    pub checkpoint_attempts: NonZeroU32,
    /// The wait between attempts.
    pub attempt_wait: Duration,
    /// Bytes the closed database may occupy.
    pub max_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SeedRefusal {
    #[error("the projection gate denied the seed: {0}")]
    Denied(crate::projection_gates::Denial),
    #[error("the projection is quarantined: {}", .0.detail)]
    Quarantined(Quarantine),
    #[error("{0} other handles still reach the projection")]
    ActiveConnections(usize),
    #[error("{0} physical workers are still running")]
    ActiveWorkers(usize),
    /// Every attempt left frames in the log or found the checkpoint held back; the projection stays open and unchanged.
    #[error(
        "the checkpoint was held back after {attempts} attempts: {wal_frames} frames logged, {checkpointed} checkpointed"
    )]
    CheckpointBlocked {
        attempts: u32,
        wal_frames: i64,
        checkpointed: i64,
    },
    #[error("the budget ended before the checkpoint completed")]
    Cancelled,
    /// A `-wal`, `-shm`, or `-journal` sidecar exists: a connection is open or a close did not finish, so the main file alone is not the database.
    #[error("sidecar {0} exists; the database is not closed")]
    SidecarPresent(String),
    /// Rows still admitted to a native worker: the work is in flight and the file alone does not carry its outcome.
    #[error("{0} embedding jobs are admitted to a running worker")]
    AdmittedWork(u64),
    #[error("integrity check reported: {0}")]
    Integrity(String),
    #[error("{0} foreign-key violations")]
    ForeignKeys(u64),
    #[error("the projection identity row is missing or corrupt")]
    Identity,
    #[error("the projection identity does not match the identity the seed was requested for")]
    IdentityMismatch,
    /// The closed file no longer verifies to the report that certified it; the certificate named other bytes.
    #[error("the seed's bytes changed after they were verified")]
    BytesChanged,
    #[error("no vector generation matches the identity")]
    Generation,
    #[error("the projection checkpoint row is missing or corrupt")]
    Checkpoint,
    #[error("{0} vectors are not of the identity's dimension")]
    VectorContract(u64),
    #[error("the database occupies {bytes} bytes, above {max}")]
    TooLarge { bytes: u64, max: u64 },
    #[error("store access failed: {0}")]
    Store(String),
    #[error("I/O failed: {0}")]
    Io(String),
    #[error("staging failed: {0}")]
    Staging(String),
    /// The store refused for capacity; the seed is intact and staging can run again.
    #[error("insufficient storage for staging")]
    InsufficientStorage,
}

/// What verification found in the closed file, bound to the bytes by `sha256`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedVerification {
    pub schema: u32,
    pub schema_version: u32,
    pub kernel_incarnation_id: String,
    pub projection_policy_version: String,
    pub identity_contract_version: String,
    pub limit_manifest_protocol_version: String,
    pub embedding_model: String,
    pub tokenizer_fingerprint: String,
    pub vector_dimension: u32,
    pub generation_epoch: u64,
    pub generation_id: String,
    pub generation_state: String,
    pub snapshot_commit_seq: i64,
    pub checkpoint_commit_seq: i64,
    pub occurrences: u64,
    pub tombstones: u64,
    pub pending_jobs: u64,
    pub admitted_jobs: u64,
    pub vectors: u64,
    pub bytes: u64,
    pub sha256: String,
}

impl SeedVerification {
    pub fn identity(&self) -> ProjectionIdentity {
        ProjectionIdentity {
            schema_version: self.schema_version,
            kernel_incarnation_id: self.kernel_incarnation_id.clone(),
            projection_policy_version: self.projection_policy_version.clone(),
            identity_contract_version: self.identity_contract_version.clone(),
            limit_manifest_protocol_version: self.limit_manifest_protocol_version.clone(),
            embedding_model: self.embedding_model.clone(),
            tokenizer_fingerprint: self.tokenizer_fingerprint.clone(),
            vector_dimension: self.vector_dimension,
            generation_epoch: self.generation_epoch,
        }
    }

    /// The report's bytes as staged beside the seed; the same bytes for the same verification.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("report serialization cannot fail")
    }

    /// The sha256 of [`Self::canonical_bytes`].
    pub fn report_sha256(&self) -> String {
        hex(&Sha256::digest(self.canonical_bytes()))
    }
}

/// A closed database that passed verification. The seed holds the database lease, so no store opens the file while it lives; dropping the seed releases the file for [`SearchProjection::open`].
pub struct ClosedSeed {
    pub path: PathBuf,
    pub verification: SeedVerification,
    lease: Option<lease::HeldFileLease>,
}

impl ClosedSeed {
    /// A seed over `path` certified by `verification`, holding no lease; tests use it to stage a file whose bytes they have changed since.
    #[cfg(feature = "test-support")]
    pub fn for_test(path: PathBuf, verification: SeedVerification) -> Self {
        Self {
            path,
            verification,
            lease: None,
        }
    }

    /// Gives the file back: drops the lease so [`SearchProjection::open`] can take it, and returns the path.
    pub fn release(self) -> PathBuf {
        let ClosedSeed { path, lease, .. } = self;
        drop(lease);
        path
    }
}

impl std::fmt::Debug for ClosedSeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClosedSeed")
            .field("path", &self.path)
            .field("verification", &self.verification)
            .finish_non_exhaustive()
    }
}

/// Points at which a test child parks so the parent can cut the process there.
#[cfg(feature = "test-support")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedBarrier {
    BeforeCheckpoint,
    AfterCheckpoint,
    AfterClose,
}

/// A refused quiesce. `projection` is the handle given back when the refusal came before the close, so its owner can keep serving; `None` after the close, when the closed file stands as it is and the caller reopens it through [`SearchProjection::open`].
pub struct QuiesceRefused {
    pub projection: Option<Arc<SearchProjection>>,
    pub refusal: SeedRefusal,
}

impl std::fmt::Debug for QuiesceRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuiesceRefused")
            .field("projection_returned", &self.projection.is_some())
            .field("refusal", &self.refusal)
            .finish()
    }
}

fn refused(projection: Option<Arc<SearchProjection>>, refusal: SeedRefusal) -> QuiesceRefused {
    QuiesceRefused {
        projection,
        refusal,
    }
}

/// Quiesces the projection: with no worker running and no other handle, checkpoints and truncates the log under `bounds`, closes the connection while keeping its lease, and verifies the closed file for `expected`.
///
/// # Errors
///
/// Returns the refusal with the projection handed back when nothing was closed.
#[allow(clippy::result_large_err)]
pub fn quiesce(
    projection: Arc<SearchProjection>,
    gate: &HookGate,
    workers: usize,
    expected: &ProjectionIdentity,
    bounds: SeedBounds,
    budget: &EvalBudget,
) -> Result<ClosedSeed, QuiesceRefused> {
    quiesce_at(
        projection,
        gate,
        workers,
        expected,
        bounds,
        budget,
        &mut |_| {},
    )
}

/// [`quiesce`] that calls `barrier` at each [`SeedBarrier`], so a test child can park there.
///
/// # Errors
///
/// As [`quiesce`].
#[cfg(feature = "test-support")]
#[allow(clippy::result_large_err)]
pub fn quiesce_with_barrier_for_test(
    projection: Arc<SearchProjection>,
    gate: &HookGate,
    workers: usize,
    expected: &ProjectionIdentity,
    bounds: SeedBounds,
    budget: &EvalBudget,
    barrier: &mut dyn FnMut(SeedBarrier),
) -> Result<ClosedSeed, QuiesceRefused> {
    quiesce_at(projection, gate, workers, expected, bounds, budget, barrier)
}

#[cfg(not(feature = "test-support"))]
#[derive(Clone, Copy)]
enum SeedBarrier {
    BeforeCheckpoint,
    AfterCheckpoint,
    AfterClose,
}

#[allow(clippy::result_large_err)]
fn quiesce_at(
    projection: Arc<SearchProjection>,
    gate: &HookGate,
    workers: usize,
    expected: &ProjectionIdentity,
    bounds: SeedBounds,
    budget: &EvalBudget,
    barrier: &mut dyn FnMut(SeedBarrier),
) -> Result<ClosedSeed, QuiesceRefused> {
    if let Err(denial) = gate.admit(ProjectionHook::EmbeddingBootstrap, EntryPoint::Reload) {
        return Err(refused(Some(projection), SeedRefusal::Denied(denial)));
    }
    if workers > 0 {
        return Err(refused(
            Some(projection),
            SeedRefusal::ActiveWorkers(workers),
        ));
    }
    let projection = match Arc::try_unwrap(projection) {
        Ok(projection) => projection,
        Err(shared) => {
            let others = Arc::strong_count(&shared) - 1;
            return Err(refused(
                Some(shared),
                SeedRefusal::ActiveConnections(others),
            ));
        }
    };
    if let Some(quarantine) = projection.quarantine() {
        return Err(refused(
            Some(Arc::new(projection)),
            SeedRefusal::Quarantined(quarantine),
        ));
    }
    barrier(SeedBarrier::BeforeCheckpoint);
    let mut last = (0, 0, 0);
    let mut completed = false;
    for attempt in 0..bounds.checkpoint_attempts.get() {
        if budget.check().is_err() {
            return Err(refused(Some(Arc::new(projection)), SeedRefusal::Cancelled));
        }
        last = match projection.checkpoint_truncate() {
            Ok(result) => result,
            Err(error) => {
                return Err(refused(
                    Some(Arc::new(projection)),
                    SeedRefusal::Store(error.to_string()),
                ));
            }
        };
        // `busy` is set when a reader or writer held the checkpoint back; the frame counts are diagnostic. A TRUNCATE checkpoint that was not held back has reset the log.
        if last.0 == 0 {
            completed = true;
            break;
        }
        if attempt + 1 < bounds.checkpoint_attempts.get() {
            std::thread::sleep(bounds.attempt_wait);
        }
    }
    if !completed {
        return Err(refused(
            Some(Arc::new(projection)),
            SeedRefusal::CheckpointBlocked {
                attempts: bounds.checkpoint_attempts.get(),
                wal_frames: last.1,
                checkpointed: last.2,
            },
        ));
    }
    barrier(SeedBarrier::AfterCheckpoint);
    let (path, lease) = projection.close();
    barrier(SeedBarrier::AfterClose);
    let verification = verify_closed(&path, expected, bounds.max_bytes)
        .map_err(|refusal| refused(None, refusal))?;
    Ok(ClosedSeed {
        path,
        verification,
        lease,
    })
}

/// Verifies a closed database file on its own connection: no sidecar, `integrity_check` ok, no foreign-key violations, the identity `expected`, a generation of that identity, a checkpoint row, no work admitted to a worker, and every vector of the identity's dimension. Returns the report with the file's digest.
///
/// # Errors
///
/// Returns the first failing check as its [`SeedRefusal`].
pub fn verify_closed(
    path: &Path,
    expected: &ProjectionIdentity,
    max_bytes: u64,
) -> Result<SeedVerification, SeedRefusal> {
    let io = |error: io::Error| SeedRefusal::Io(error.kind().to_string());
    // SQLite unlinks the log and the shared-memory index when the last connection closes; a rollback journal belongs to no closed database.
    for suffix in ["sqlite-wal", "sqlite-shm", "sqlite-journal"] {
        match fs::symlink_metadata(path.with_extension(suffix)) {
            Ok(_) => return Err(SeedRefusal::SidecarPresent(suffix.to_owned())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io(error)),
        }
    }
    let bytes = fs::metadata(path).map_err(io)?.len();
    if bytes > max_bytes {
        return Err(SeedRefusal::TooLarge {
            bytes,
            max: max_bytes,
        });
    }
    // `immutable` reads the file as it is: no log is created, replayed, or expected.
    let conn = Connection::open_with_flags(
        immutable_uri(path),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| SeedRefusal::Store(error.to_string()))?;
    let store = |error: rusqlite::Error| SeedRefusal::Store(error.to_string());
    let integrity: String = conn
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(store)?;
    if integrity != "ok" {
        return Err(SeedRefusal::Integrity(integrity));
    }
    let violations: u64 = conn
        .prepare("PRAGMA foreign_key_check")
        .and_then(|mut statement| {
            statement
                .query_map([], |_| Ok(()))?
                .count()
                .try_into()
                .map_err(|_| rusqlite::Error::InvalidQuery)
        })
        .map_err(store)?;
    if violations > 0 {
        return Err(SeedRefusal::ForeignKeys(violations));
    }
    let identity: ProjectionIdentity = conn
        .query_row(
            "SELECT schema_version,kernel_incarnation_id,projection_policy_version,identity_contract_version,
                    limit_manifest_protocol_version,embedding_model,tokenizer_fingerprint,vector_dimension,generation_epoch
             FROM projection_identity WHERE singleton=1",
            [],
            |row| {
                Ok(ProjectionIdentity {
                    schema_version: row.get(0)?,
                    kernel_incarnation_id: row.get(1)?,
                    projection_policy_version: row.get(2)?,
                    identity_contract_version: row.get(3)?,
                    limit_manifest_protocol_version: row.get(4)?,
                    embedding_model: row.get(5)?,
                    tokenizer_fingerprint: row.get(6)?,
                    vector_dimension: row.get(7)?,
                    generation_epoch: u64::try_from(row.get::<_, i64>(8)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                })
            },
        )
        .map_err(|_| SeedRefusal::Identity)?;
    if identity != *expected {
        return Err(SeedRefusal::IdentityMismatch);
    }
    let (generation_id, generation_state): (String, String) = conn
        .query_row(
            "SELECT generation_id,state FROM vector_generations
             WHERE embedding_model=?1 AND tokenizer_fingerprint=?2 AND vector_dimension=?3 AND generation_epoch=?4
             ORDER BY generation_id LIMIT 1",
            rusqlite::params![
                identity.embedding_model,
                identity.tokenizer_fingerprint,
                identity.vector_dimension,
                i64::try_from(identity.generation_epoch).map_err(|_| SeedRefusal::Identity)?
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| SeedRefusal::Generation)?;
    let (snapshot_commit_seq, checkpoint_commit_seq): (i64, i64) = conn
        .query_row(
            "SELECT snapshot_commit_seq,checkpoint_commit_seq FROM projection_checkpoint WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| SeedRefusal::Checkpoint)?;
    let count = |sql: &str| -> Result<u64, SeedRefusal> {
        conn.query_row(sql, [], |row| row.get::<_, i64>(0))
            .map_err(store)
            .and_then(|n| {
                u64::try_from(n).map_err(|_| SeedRefusal::Store("negative count".to_owned()))
            })
    };
    let off_dimension = conn
        .query_row(
            "SELECT count(*) FROM occurrence_vectors WHERE vector_dimension<>?1",
            [identity.vector_dimension],
            |row| row.get::<_, i64>(0),
        )
        .map_err(store)?;
    if off_dimension > 0 {
        return Err(SeedRefusal::VectorContract(off_dimension.unsigned_abs()));
    }
    let occurrences = count("SELECT count(*) FROM occurrences")?;
    let tombstones = count("SELECT count(*) FROM occurrence_tombstones")?;
    let pending_jobs = count("SELECT count(*) FROM embedding_jobs WHERE state='pending'")?;
    let admitted_jobs = count("SELECT count(*) FROM embedding_jobs WHERE state='admitted'")?;
    if admitted_jobs > 0 {
        return Err(SeedRefusal::AdmittedWork(admitted_jobs));
    }
    let vectors = count("SELECT count(*) FROM occurrence_vectors")?;
    drop(conn);
    let sha256 = file_sha256(path).map_err(io)?;
    Ok(SeedVerification {
        schema: REPORT_SCHEMA,
        schema_version: identity.schema_version,
        kernel_incarnation_id: identity.kernel_incarnation_id,
        projection_policy_version: identity.projection_policy_version,
        identity_contract_version: identity.identity_contract_version,
        limit_manifest_protocol_version: identity.limit_manifest_protocol_version,
        embedding_model: identity.embedding_model,
        tokenizer_fingerprint: identity.tokenizer_fingerprint,
        vector_dimension: identity.vector_dimension,
        generation_epoch: identity.generation_epoch,
        generation_id,
        generation_state,
        snapshot_commit_seq,
        checkpoint_commit_seq,
        occurrences,
        tombstones,
        pending_jobs,
        admitted_jobs,
        vectors,
        bytes,
        sha256,
    })
}

/// The staged seed: the generation digest that names the directory object holding the seed and its report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedSeed {
    pub digest: String,
    pub verification: SeedVerification,
}

/// The manifest identity a seed stages under: the target names the seed kind, the contract slot carries the compatibility identity's digest, the inputs slot the verification report's digest, and the payload slot the seed bytes' digest. No release contract or inputs lock exists for a seed; the slots bind what a seed has.
pub fn seed_stage_meta(verification: &SeedVerification) -> StageMeta {
    let identity = verification.identity();
    let compatibility = format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        identity.schema_version,
        identity.projection_policy_version,
        identity.identity_contract_version,
        identity.limit_manifest_protocol_version,
        identity.embedding_model,
        identity.tokenizer_fingerprint,
        identity.vector_dimension,
        identity.generation_epoch
    );
    StageMeta {
        target: SEED_TARGET.to_owned(),
        release_contract_sha256: hex(&Sha256::digest(compatibility.as_bytes())),
        inputs_lock_sha256: verification.report_sha256(),
        source_payload_manifest_sha256: verification.sha256.clone(),
    }
}

/// Verifies the closed seed's bytes again, writes its report beside the control record, and stages both into `store` under [`seed_stage_meta`]. A seed whose bytes changed since it was closed is refused: its certificate named other bytes.
///
/// # Errors
///
/// Returns [`SeedRefusal::BytesChanged`] when the bytes no longer verify to the same report, and [`SeedRefusal::Staging`] for the store's refusal; nothing is selected in either case.
pub fn stage(
    seed: &ClosedSeed,
    store: &GenerationStore,
    report_dir: &Path,
    protected: &BTreeSet<String>,
) -> Result<StagedSeed, SeedRefusal> {
    let io = |error: io::Error| SeedRefusal::Io(error.kind().to_string());
    let expected = seed.verification.identity();
    let again = verify_closed(&seed.path, &expected, seed.verification.bytes)?;
    if again != seed.verification {
        return Err(SeedRefusal::BytesChanged);
    }
    let report_path = report_dir.join(format!(
        "{SEED_REPORT_FILE}.{}.{}",
        std::process::id(),
        seed.verification.report_sha256()
    ));
    let report = seed.verification.canonical_bytes();
    let written = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags((OFlags::NOFOLLOW | OFlags::CLOEXEC).bits() as i32)
        .open(&report_path)
        .and_then(|mut file| {
            file.write_all(&report)?;
            file.sync_all()
        });
    if let Err(error) = written {
        let _ = fs::remove_file(&report_path);
        return Err(io(error));
    }
    let sources = [
        SourceSpec {
            rel_path: SEED_FILE.to_owned(),
            source: seed.path.clone(),
            executable: false,
            expected_size: Some(seed.verification.bytes),
            expected_sha256: Some(seed.verification.sha256.clone()),
        },
        SourceSpec {
            rel_path: SEED_REPORT_FILE.to_owned(),
            source: report_path.clone(),
            executable: false,
            expected_size: Some(report.len() as u64),
            expected_sha256: Some(seed.verification.report_sha256()),
        },
    ];
    let staged = store.stage(&sources, &seed_stage_meta(&seed.verification), protected);
    let _ = fs::remove_file(&report_path);
    let digest = staged.map_err(|error| match error {
        GenerationError::InsufficientStorage => SeedRefusal::InsufficientStorage,
        other => SeedRefusal::Staging(other.to_string()),
    })?;
    Ok(StagedSeed {
        digest,
        verification: seed.verification.clone(),
    })
}

fn file_sha256(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
