//! The daemon's connection to the disposable `search.sqlite` projection.
//! The retrieval crate owns the schema and pure mutations.
//! This module owns file placement, the fenced writer, query-only reads,
//! connection checks, and owner-only permissions.
//!
//! This module does not rebuild, delete, or migrate the projection.
//! Lifecycle state, recovery authorization, and replacement selection belong
//! outside this disposable database.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use retrieval::batch::{BatchBounds, BatchOutcome, BatchStatus, ProjectionBatch};
use retrieval::{BASELINE, ProjectionError};
use storage::{
    GuardedConn, Isolation, SqliteStore, StorageBackend, StorageDescriptor, StoreError, open_sqlite,
};

use crate::search_writer::{Quarantine, QuarantineKind};

/// The page cache the projection connection is allowed, in KiB.
pub const CACHE_KIB: u32 = 8 * 1024;
/// Pages SQLite may hold in memory for one transient index or sort.
const TEMP_STORE_MEMORY: &str = "MEMORY";

/// What the opened connection was verified to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionFacts {
    pub journal_mode: String,
    /// `PRAGMA synchronous`: 2 is FULL.
    pub synchronous: i64,
    pub foreign_keys: bool,
    /// Negative: KiB of page cache.
    pub cache_size: i64,
    pub temp_store: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchProjectionError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Projection(#[from] ProjectionError),
    #[error("search.sqlite connection is not in the required state: {0}")]
    Connection(String),
    #[error("the search projection is quarantined: {}", .0.detail)]
    Quarantined(Quarantine),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StoreFailure {
    Deadline,
    Rejected,
    Integrity,
    Unknown,
}

pub(crate) fn classify_store_failure(error: &StoreError) -> StoreFailure {
    match error {
        StoreError::Deadline => StoreFailure::Deadline,
        StoreError::Baseline(_)
        | StoreError::FenceCorrupt { .. }
        | StoreError::FenceMissing
        | StoreError::FenceExhausted { .. } => StoreFailure::Integrity,
        StoreError::Fenced { .. } => StoreFailure::Rejected,
        StoreError::Lease(_)
        | StoreError::UnsupportedBackend(_)
        | StoreError::Backend(_)
        | StoreError::Io(_) => StoreFailure::Unknown,
    }
}

pub struct SearchProjection {
    store: SqliteStore,
    path: PathBuf,
    /// Shares one quarantine among the projection's writers.
    quarantine: Mutex<Option<Quarantine>>,
    quarantined: AtomicBool,
}

impl SearchProjection {
    /// Opens `<data_home>/search/search.sqlite` with the storage lease and fence.
    /// A missing file is created; a pristine file receives the retrieval baseline.
    /// An existing initialized file must match the baseline.
    /// The connection's pragmas are pinned and verified.
    /// On Unix, storage keeps the directory, database, and journals owner-only.
    ///
    /// Call [`retrieval::install_identity`] separately with the expected identity.
    /// Opening does not establish projection compatibility or completeness.
    /// A successful open does not authorize serving search.
    ///
    /// # Errors
    ///
    /// Returns [`SearchProjectionError::Store`] on storage or pragma access errors.
    /// A baseline mismatch is a storage error.
    /// Returns [`SearchProjectionError::Connection`] on pragma value mismatches.
    pub fn open(data_home: &Path) -> Result<Self, SearchProjectionError> {
        let path = data_home.join("search").join("search.sqlite");
        let descriptor = StorageDescriptor {
            module_id: "eidnara".to_string(),
            storage_namespace: "search-projection".to_string(),
            isolation: Isolation::Module,
            backend: StorageBackend::Sqlite {
                path: path
                    .to_str()
                    .ok_or_else(|| {
                        StoreError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "search projection path is not valid UTF-8",
                        ))
                    })?
                    .to_owned(),
            },
        };
        let store = open_sqlite(&descriptor, BASELINE)?;
        let projection = Self {
            store,
            path,
            quarantine: Mutex::new(None),
            quarantined: AtomicBool::new(false),
        };
        projection.pin_connection()?;
        projection.verify_connection()?;
        Ok(projection)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn quarantine(&self) -> Option<Quarantine> {
        if !self.quarantined.load(Ordering::Acquire) {
            return None;
        }
        self.quarantine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The shared quarantine state preserves the first cause so every writer
    /// returns the same [`Quarantine`].
    pub(crate) fn enter_quarantine(
        &self,
        kind: QuarantineKind,
        error: &dyn std::fmt::Display,
    ) -> Quarantine {
        let quarantine = self.publish_quarantine_intent(kind, error);
        self.synchronize_quarantine(quarantine)
    }

    pub(crate) fn publish_quarantine_intent(
        &self,
        kind: QuarantineKind,
        error: &dyn std::fmt::Display,
    ) -> Quarantine {
        let quarantine = self.record_quarantine(kind, error);
        self.quarantined.store(true, Ordering::Release);
        quarantine
    }

    pub(crate) fn synchronize_quarantine(&self, quarantine: Quarantine) -> Quarantine {
        // Wait for active transactions so their final quarantine checks can roll back their writes.
        let _ = self.store.with_conn_unfenced(|_| Ok(()));
        quarantine
    }

    fn record_quarantine(&self, kind: QuarantineKind, error: &dyn std::fmt::Display) -> Quarantine {
        self.quarantine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_or_insert_with(|| Quarantine::new(kind, error))
            .clone()
    }

    pub(crate) fn allows_acknowledgement(&self) -> bool {
        !self.quarantined.load(Ordering::Acquire)
    }

    /// Forces quarantine without constructing a storage failure, so tests can
    /// place the state change at an exact writer boundary.
    #[cfg(feature = "test-support")]
    pub fn enter_quarantine_for_test(&self, kind: QuarantineKind, detail: &str) -> Quarantine {
        self.enter_quarantine(kind, &detail)
    }

    /// Bounds the page cache and keeps transient sort and index storage in
    /// memory, where the bound applies, rather than in unowned temp files.
    fn pin_connection(&self) -> Result<(), SearchProjectionError> {
        self.store.with_conn_unfenced(|conn| {
            conn.pragma_update(None, "cache_size", -i64::from(CACHE_KIB))?;
            conn.pragma_update(None, "temp_store", TEMP_STORE_MEMORY)?;
            Ok(())
        })?;
        Ok(())
    }

    /// Changes one connection pragma so a test can watch the verification
    /// refuse the state it names.
    #[cfg(feature = "test-support")]
    pub fn set_pragma_for_test(&self, pragma: &str, value: i64) {
        self.store
            .with_conn_unfenced(|conn| conn.pragma_update(None, pragma, value))
            .expect("pragma update");
    }

    /// Reads the connection state and refuses anything but a crash-safe,
    /// constraint-enforcing, bounded connection.
    pub fn verify_connection(&self) -> Result<ConnectionFacts, SearchProjectionError> {
        let facts = self.store.with_conn_unfenced(|conn| {
            Ok(ConnectionFacts {
                journal_mode: conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))?,
                synchronous: conn.query_row("PRAGMA synchronous", [], |row| row.get(0))?,
                foreign_keys: conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?,
                cache_size: conn.query_row("PRAGMA cache_size", [], |row| row.get(0))?,
                temp_store: conn.query_row("PRAGMA temp_store", [], |row| row.get(0))?,
            })
        })?;
        // 2 is FULL for synchronous and MEMORY for temp_store.
        let expectations: [(&str, bool, String); 5] = [
            (
                "journal_mode",
                facts.journal_mode.eq_ignore_ascii_case("wal"),
                facts.journal_mode.clone(),
            ),
            (
                "synchronous",
                facts.synchronous == 2,
                facts.synchronous.to_string(),
            ),
            (
                "foreign_keys",
                facts.foreign_keys,
                facts.foreign_keys.to_string(),
            ),
            (
                "cache_size",
                facts.cache_size == -i64::from(CACHE_KIB),
                facts.cache_size.to_string(),
            ),
            (
                "temp_store",
                facts.temp_store == 2,
                facts.temp_store.to_string(),
            ),
        ];
        if let Some((name, _, actual)) = expectations.iter().find(|(_, holds, _)| !holds) {
            return Err(SearchProjectionError::Connection(format!(
                "{name} is {actual}"
            )));
        }
        Ok(facts)
    }

    /// Applies one complete canonical prefix in one fenced transaction and
    /// returns only after the transaction has committed and released the
    /// connection, so the caller acknowledges kernel progress afterwards and
    /// never while the local transaction is open. Nothing here touches the
    /// kernel.
    pub fn apply_batch(
        &self,
        batch: &ProjectionBatch<'_>,
        bounds: BatchBounds,
        now: i64,
    ) -> Result<BatchOutcome, SearchProjectionError> {
        self.write(|conn| retrieval::batch::apply_batch(conn, batch, bounds, now))
    }

    /// Reconciles a lost commit outcome by reading checkpoint coverage and required batch effects in one transaction.
    /// Conflicting stored identity or payload bytes propagate as projection errors.
    pub fn batch_status(
        &self,
        batch: &ProjectionBatch<'_>,
    ) -> Result<BatchStatus, SearchProjectionError> {
        self.read(|conn| retrieval::batch::batch_status(conn, batch))
    }

    /// One epoch-fenced write transaction. The closure's rows commit together
    /// or not at all, and a store superseded by a newer opener cannot write.
    pub fn write<T>(
        &self,
        f: impl FnOnce(&GuardedConn<'_>) -> Result<T, ProjectionError>,
    ) -> Result<T, SearchProjectionError> {
        self.run(f, Access::Write)
    }

    /// [`Self::write`] whose connection and write-lock acquisition end at `deadline`.
    ///
    /// # Errors
    ///
    /// Returns [`SearchProjectionError::Store`] carrying [`StoreError::Deadline`] when the
    /// connection or the write lock is still held at `deadline`; nothing was written.
    pub fn write_within<T>(
        &self,
        deadline: Instant,
        f: impl FnOnce(&GuardedConn<'_>) -> Result<T, ProjectionError>,
    ) -> Result<T, SearchProjectionError> {
        self.run(f, Access::WriteWithin(deadline))
    }

    /// One query-only read transaction on the same connection; a write inside
    /// it is refused by the store's authorizer.
    pub fn read<T>(
        &self,
        f: impl FnOnce(&GuardedConn<'_>) -> Result<T, ProjectionError>,
    ) -> Result<T, SearchProjectionError> {
        self.run(f, Access::Read)
    }

    /// Runs `f` under the store's transaction of the given access. A refusal
    /// from `f` rolls the transaction back and is returned as itself; a store
    /// failure is returned as the store's.
    fn run<T>(
        &self,
        f: impl FnOnce(&GuardedConn<'_>) -> Result<T, ProjectionError>,
        access: Access,
    ) -> Result<T, SearchProjectionError> {
        let mut outcome: Option<Result<T, ProjectionError>> = None;
        let mut quarantine = None;
        let inner = |conn: &GuardedConn<'_>| -> rusqlite::Result<()> {
            if !matches!(access, Access::Read)
                && let Some(found) = self.quarantine()
            {
                quarantine = Some(found);
                return Err(rusqlite::Error::QueryReturnedNoRows);
            }
            let result = f(conn);
            let failed = result.is_err();
            outcome = Some(result);
            if failed {
                return Err(rusqlite::Error::QueryReturnedNoRows);
            }
            if !matches!(access, Access::Read)
                && let Some(found) = self.quarantine()
            {
                quarantine = Some(found);
                return Err(rusqlite::Error::QueryReturnedNoRows);
            }
            Ok(())
        };
        let store_result = match access {
            Access::Write => self.store.with_conn_fenced(inner),
            Access::WriteWithin(deadline) => self.store.with_conn_fenced_within(deadline, inner),
            Access::Read => self.store.with_conn(inner),
        };
        if let Some(quarantine) = quarantine {
            return Err(SearchProjectionError::Quarantined(quarantine));
        }
        match outcome {
            Some(Ok(value)) => {
                store_result?;
                Ok(value)
            }
            Some(Err(error)) => Err(error.into()),
            None => {
                store_result?;
                Err(SearchProjectionError::Connection(
                    "the closure did not run".to_string(),
                ))
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Access {
    Write,
    WriteWithin(Instant),
    Read,
}
