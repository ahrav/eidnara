//! Backend mechanics for module storage: open a database from a
//! [`StorageDescriptor`], guard it with the single-writer lease, and give it
//! exactly one schema.
//!
//! Modules pass a resolved descriptor and one baseline DDL text here, then run
//! domain queries against the lease-guarded connection. A pristine file receives
//! the baseline once; any other file must already carry the baseline's identity
//! (application id, user version, format-marker digest, and `sqlite_schema`
//! inventory) or the open is refused without mutation. There is no version
//! ledger and no code path that upgrades one schema into another. Backends are
//! feature-gated, so module code does not branch on the descriptor's backend.
//!
//! The single-writer lease ([`lease`]) is keyed by
//! `(module_id, backend, storage_namespace/database file name)`, so stores that share
//! a lease root do not collide. The persisted epoch serves as the fence token for
//! epoch-checked writes.

pub use storage_types::{
    Isolation, StorageBackend, StorageDescriptor, postgres_database_name, sqlite_store_path,
};

use lease::LeaseError;
#[cfg(feature = "sqlite")]
use lease::LeaseKey;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// A conflicting live holder prevented acquisition, or lease I/O failed.
    #[error("storage lease: {0}")]
    Lease(#[source] LeaseError),
    /// The descriptor asked for a backend this build was not compiled with.
    #[error("storage backend '{0}' is not supported by this build (missing feature)")]
    UnsupportedBackend(String),
    /// The file is neither pristine nor identical to the baseline identity.
    #[error("database does not match the baseline: {0}")]
    Baseline(String),
    /// A backend (database driver) operation failed.
    #[error("storage backend: {0}")]
    Backend(String),
    /// A bounded write reached its deadline before `BEGIN` ran, so it applied nothing.
    #[error("storage write lock was not acquired before the deadline")]
    Deadline,
    /// An io failure preparing the store location.
    #[error("storage io: {0}")]
    Io(#[source] std::io::Error),
    /// A fenced (epoch-checked) write was rejected because the database has already
    /// been claimed by a newer writer. `db_epoch` (the epoch stamped in the
    /// database) is greater than `holder_epoch` (this store's lease epoch), so this
    /// writer has been superseded — for example a draining old instance attempting a
    /// late write after a replacement took the lease. The write was not applied.
    #[error(
        "fenced write rejected: this writer holds epoch {holder_epoch} but the \
         database was claimed by a newer writer at epoch {db_epoch}"
    )]
    Fenced { holder_epoch: u64, db_epoch: u64 },
    /// An out-of-range database epoch prevents proving monotonic fencing. The store
    /// refuses to open until an operator resets `fence.epoch`.
    #[error(
        "database fence epoch {db_epoch} is outside the supported range; reset \
         fence.epoch to at least the highest epoch a writer has used"
    )]
    FenceCorrupt { db_epoch: i64 },
    /// An initialized store carries no `fence` row. The row is the writer authority, so
    /// its absence is corruption rather than a fresh start at epoch zero.
    #[error("database fence row is missing from an initialized store; restore it before reopening")]
    FenceMissing,
    /// The stored fence epoch has no representable successor, so no further writer epoch
    /// can be issued for this store until the row is repaired.
    #[error(
        "database fence epoch {db_epoch} has no representable successor; repair fence.epoch \
         before reopening"
    )]
    FenceExhausted { db_epoch: u64 },
}

/// The lease key includes the module, backend, storage namespace, and the
/// database file name. The lease root is the database's parent directory, so
/// two distinct database files in one directory need distinct keys or they
/// falsely contend. File names cannot contain `/`, so the `namespace/file`
/// join is unambiguous.
#[cfg(feature = "sqlite")]
fn lease_key(descriptor: &StorageDescriptor, db_file_name: &str) -> Result<LeaseKey, StoreError> {
    // `LeaseKey::identity` treats a U+001F inside a field as a programming error and
    // panics; a descriptor is deserialized input, so the separator is refused here with
    // an error instead.
    for (name, field) in [
        ("module_id", descriptor.module_id.as_str()),
        ("storage_namespace", descriptor.storage_namespace.as_str()),
        ("sqlite file name", db_file_name),
    ] {
        if field.contains('\u{1f}') {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("descriptor {name} {field:?} contains U+001F, the lease-key separator"),
            )));
        }
    }
    Ok(LeaseKey::new(
        &descriptor.module_id,
        descriptor.backend.label(),
        format!("{}/{}", descriptor.storage_namespace, db_file_name),
    ))
}

#[cfg(feature = "sqlite")]
mod sqlite_backend {
    use super::*;
    use std::{
        collections::HashSet,
        ffi::c_int,
        ops::{Deref, DerefMut},
        path::{Path, PathBuf},
        sync::{Arc, Mutex, MutexGuard},
        thread::{self, ThreadId},
        time::{Duration, Instant},
    };

    use lease::{FileIdentity, FileLeaseStore, HeldFileLease, protect_file};
    use rusqlite::{Connection, OpenFlags};
    use sha2::{Digest, Sha256};

    /// How long a statement waits on another connection's lock before SQLite reports `SQLITE_BUSY`.
    const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
    /// `Mutex::lock` has no timeout, so a bounded acquisition polls at this interval.
    const CONN_ACQUIRE_POLL: Duration = Duration::from_millis(1);

    #[cfg(test)]
    thread_local! {
        static BEFORE_NEXT_BUSY_WAIT: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
            std::cell::RefCell::new(None);
    }

    #[cfg(test)]
    pub(super) fn before_next_busy_wait_for_test(observer: impl FnOnce() + 'static) {
        BEFORE_NEXT_BUSY_WAIT.with(|slot| {
            assert!(slot.borrow_mut().replace(Box::new(observer)).is_none());
        });
    }

    #[cfg(test)]
    fn observe_busy_wait_for_test() {
        BEFORE_NEXT_BUSY_WAIT.with(|slot| {
            if let Some(observer) = slot.borrow_mut().take() {
                observer();
            }
        });
    }

    /// `PRAGMA application_id` of every Eidnara-owned SQLite file (`EIDN` in ASCII).
    pub const APPLICATION_ID: u32 = 0x4549_444E;
    /// `PRAGMA user_version` of every Eidnara-owned SQLite file.
    pub const USER_VERSION: u32 = 1;
    /// The objects every store carries ahead of the consumer's baseline.
    pub const STORE_BASELINE: &str = include_str!("../baseline.sql");
    /// `STORE_BASELINE` creates these tables; `is_infrastructure_table` and the
    /// authorizer treat exactly these names as store-owned.
    pub const INFRASTRUCTURE_TABLES: &[&str] = &["fence", "format_marker"];

    /// A lease-guarded SQLite store. The lease remains held for the store's lifetime.
    /// A single mutexed connection preserves connection-local configuration and transaction scope.
    /// [`open_sqlite`] claims the database lease epoch before returning the store.
    ///
    /// Every callback runs while the connection lock is held.
    /// Re-entry into the same store from a callback returns [`StoreError::Backend`] instead of blocking.
    /// The re-entry check is per thread; another thread blocks because the callback holds
    /// the connection lock, so a callback must not wait on that thread.
    pub struct SqliteStore {
        conn: Mutex<Connection>,
        holder: Mutex<Option<ThreadId>>,
        epoch: u64,
        /// The connection's only authorizer reads this gate; a callback switches its mode
        /// rather than installing a policy.
        gate: Arc<AuthorityGate>,
        // Declared after `conn` so the connection closes before the lease unlocks;
        // `None` is reserved for `for_test`.
        _lease: Option<HeldFileLease>,
    }

    /// A connection together with the gate its authorizer reads. [`AuthorityGate::install`]
    /// is the only constructor, so a store cannot pair a connection with a gate that no
    /// authorizer consults.
    pub(crate) struct ClaimedConnection {
        pub(crate) conn: Connection,
        gate: Arc<AuthorityGate>,
    }

    /// Dropping the guard clears the holder record before the connection lock releases.
    struct ConnGuard<'a> {
        conn: MutexGuard<'a, Connection>,
        holder: &'a Mutex<Option<ThreadId>>,
    }

    impl Deref for ConnGuard<'_> {
        type Target = Connection;

        fn deref(&self) -> &Connection {
            &self.conn
        }
    }

    impl DerefMut for ConnGuard<'_> {
        fn deref_mut(&mut self) -> &mut Connection {
            &mut self.conn
        }
    }

    impl Drop for ConnGuard<'_> {
        fn drop(&mut self) {
            // Clear `holder` while `conn` remains locked, so no other thread can set it first.
            *self.holder.lock().unwrap_or_else(|p| p.into_inner()) = None;
        }
    }

    impl SqliteStore {
        pub fn epoch(&self) -> u64 {
            self.epoch
        }

        /// Construct a store over an open connection without acquiring a lease.
        ///
        /// Tests use this to model stale and replacement connections at different
        /// epochs, a state the OS lock prevents constructing through `open_sqlite`.
        #[cfg(test)]
        pub(crate) fn for_test(conn: Connection, epoch: u64) -> Self {
            let claimed =
                AuthorityGate::install(conn).expect("the test connection accepts the gate");
            Self::over(claimed, epoch, None)
        }

        /// Starts recording statement-cache reuse; an earlier recording is discarded.
        #[cfg(any(test, feature = "test-support"))]
        pub fn start_statement_reuse_probe(&self) {
            self.gate.lock().statement_probe = Some(std::collections::BTreeMap::new());
        }

        /// Per cache key passed to [`GuardedConn::prepare_cached`] since the probe started,
        /// the highest run count a returned handle carried. SQLite counts a run per step
        /// sequence ended by a reset and keeps the count across in-place re-preparation, so
        /// a text's count is the number of times the connection ran it since the handle
        /// was created. Empty when the probe was never started.
        #[cfg(any(test, feature = "test-support"))]
        pub fn statement_runs(&self) -> std::collections::BTreeMap<String, i32> {
            self.gate
                .lock()
                .statement_probe
                .iter()
                .flat_map(|probe| probe.iter())
                .map(|(sql, reuse)| (sql.clone(), reuse.max_runs))
                .collect()
        }

        /// Per cache key passed to [`GuardedConn::prepare_cached`] since the probe started,
        /// how many times the statement cache handed out a re-created handle after a
        /// returned handle of that key had run. Every key prepared appears; a cache sized
        /// for the hot set shows zero for each. Empty when the probe was never started.
        #[cfg(any(test, feature = "test-support"))]
        pub fn statement_evictions(&self) -> std::collections::BTreeMap<String, u32> {
            self.gate
                .lock()
                .statement_probe
                .iter()
                .flat_map(|probe| probe.iter())
                .map(|(sql, reuse)| (sql.clone(), reuse.evictions))
                .collect()
        }

        fn over(claimed: ClaimedConnection, epoch: u64, lease: Option<HeldFileLease>) -> Self {
            SqliteStore {
                conn: Mutex::new(claimed.conn),
                holder: Mutex::new(None),
                epoch,
                gate: claimed.gate,
                _lease: lease,
            }
        }

        /// `lock_conn` rejects same-thread re-entry because relocking `conn` can deadlock or panic.
        /// `holder` is written only after `conn` is acquired and cleared only before `conn`
        /// is released, so a thread that reads its own id in `holder` holds `conn`, and the
        /// check cannot race with another thread taking the lock.
        fn lock_conn(&self) -> Result<ConnGuard<'_>, StoreError> {
            self.refuse_reentry()?;
            let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
            Ok(self.guard(conn))
        }

        /// Acquires the connection by polling until `deadline` rather than blocking past the caller's budget.
        fn lock_conn_within(&self, deadline: Instant) -> Result<ConnGuard<'_>, StoreError> {
            self.refuse_reentry()?;
            loop {
                match self.conn.try_lock() {
                    Ok(conn) => return Ok(self.guard(conn)),
                    Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                        return Ok(self.guard(poisoned.into_inner()));
                    }
                    Err(std::sync::TryLockError::WouldBlock) => {}
                }
                if Instant::now() >= deadline {
                    return Err(StoreError::Deadline);
                }
                thread::sleep(CONN_ACQUIRE_POLL);
            }
        }

        fn refuse_reentry(&self) -> Result<(), StoreError> {
            let current = thread::current().id();
            if *self.holder.lock().unwrap_or_else(|p| p.into_inner()) == Some(current) {
                return Err(StoreError::Backend(
                    "store re-entered from a callback running under its connection lock"
                        .to_string(),
                ));
            }
            Ok(())
        }

        fn guard<'a>(&'a self, conn: MutexGuard<'a, Connection>) -> ConnGuard<'a> {
            *self.holder.lock().unwrap_or_else(|p| p.into_inner()) = Some(thread::current().id());
            ConnGuard {
                conn,
                holder: &self.holder,
            }
        }

        /// `with_conn` permits read-only queries and connection-local configuration.
        /// `PRAGMA query_only` makes database writes fail with `SQLITE_READONLY`,
        /// which keeps every durable write on the fenced path
        /// ([`Self::with_conn_fenced`]). The callback receives a [`GuardedConn`] rather
        /// than the connection, so it cannot replace the guard, set pragmas, run
        /// statement batches, or control transactions. Statements that reach the database
        /// are additionally checked: pragma writes, transaction control, savepoints,
        /// `ATTACH`/`DETACH`, and writes to the fence and format-marker tables are denied.
        ///
        /// Every statement the callback issues shares one deferred-transaction snapshot,
        /// so a write another connection commits between two of its reads stays invisible
        /// to the second read. Transaction control is denied, so the callback cannot end
        /// that transaction.
        ///
        /// # Errors
        ///
        /// Returns [`StoreError::Backend`] if the callback returns an error, issues a write
        /// or denied statement, or if the transaction or scope fails to start or release.
        pub fn with_conn<T>(
            &self,
            f: impl FnOnce(&GuardedConn<'_>) -> rusqlite::Result<T>,
        ) -> Result<T, StoreError> {
            let mut guard = self.lock_conn()?;
            let tx = guard
                .transaction_with_behavior(rusqlite::TransactionBehavior::Deferred)
                .map_err(|e| StoreError::Backend(e.to_string()))?;
            // `scope` is declared after `tx` so its mode leaves before `tx` issues `ROLLBACK` during unwinding.
            let scope = CallbackScope::read_only(&tx, &self.gate)?;
            let out = f(&GuardedConn::new(&tx, &self.gate))
                .map_err(|e| StoreError::Backend(e.to_string()));
            let restored = scope.release();
            // Finishing the read transaction releases its snapshot; there is nothing to
            // commit.
            let finished = tx.finish().map_err(|e| StoreError::Backend(e.to_string()));
            with_cleanup_failure(with_cleanup_failure(out, restored), finished)
        }

        /// SQLite refuses `VACUUM` inside a transaction, and both
        /// [`Self::with_conn`] and [`Self::with_conn_fenced`] open one, so
        /// maintenance statements need this unguarded path.
        /// Fence-protected durable mutations belong
        /// in [`Self::with_conn_fenced`]; SQLite does not enforce that
        /// restriction here. The handle reaches pragmas and statement batches but not the
        /// authorizer, which only the store installs.
        ///
        /// # Errors
        ///
        /// Returns [`StoreError::Backend`] when the callback fails.
        pub fn with_conn_unfenced<T>(
            &self,
            f: impl FnOnce(&MaintenanceConn<'_>) -> rusqlite::Result<T>,
        ) -> Result<T, StoreError> {
            let guard = self.lock_conn()?;
            let _exit = MaintenanceExit {
                conn: &guard,
                gate: &self.gate,
            };
            f(&MaintenanceConn::new(&guard)).map_err(|e| StoreError::Backend(e.to_string()))
        }

        /// Run a closure inside an epoch-fenced write transaction. The write is
        /// rejected ([`StoreError::Fenced`]) if a newer writer has taken over the
        /// database; otherwise it commits atomically.
        ///
        /// The persisted epoch rejects late writes from an instance that has released
        /// its lease.
        ///
        /// Mechanism: an IMMEDIATE transaction reads the database's stored fence
        /// epoch and, if it is greater than this store's lease epoch, rejects without
        /// applying `f` (a newer writer owns the database). Otherwise it claims the
        /// database for this epoch and runs `f`, committing atomically. Returning an
        /// error from `f` rolls the transaction back.
        ///
        /// The callback receives a [`GuardedConn`], so it holds neither the transaction
        /// nor the connection and cannot commit, replace the guard, or set pragmas.
        /// Transaction control, savepoints, `ATTACH`/`DETACH`, and writes to the fence
        /// and format-marker tables are denied for its duration, so no statement of its can
        /// commit outside the checked transaction or alter the authority that checked
        /// it.
        ///
        /// # Errors
        ///
        /// Returns [`StoreError::Fenced`] if the persisted database epoch exceeds the store epoch.
        /// Returns [`StoreError::Backend`] if transaction setup, fence access, the callback, a denied statement, the durability pin, the scope, or commit fails.
        pub fn with_conn_fenced<T>(
            &self,
            f: impl FnOnce(&GuardedConn<'_>) -> rusqlite::Result<T>,
        ) -> Result<T, StoreError> {
            let guard = self.lock_conn()?;
            self.fenced_write(&guard, None, f)
        }

        /// [`Self::with_conn_fenced`] whose connection and write-lock acquisition end at `deadline`.
        ///
        /// The connection is polled rather than awaited, and `BEGIN IMMEDIATE` waits for another
        /// writer only until `deadline`. Once the transaction is open the write is unbounded, as
        /// in [`Self::with_conn_fenced`].
        ///
        /// # Errors
        ///
        /// Returns [`StoreError::Deadline`] when the connection or the write lock is still held
        /// at `deadline`; nothing was written. Every other error is as for [`Self::with_conn_fenced`].
        pub fn with_conn_fenced_within<T>(
            &self,
            deadline: Instant,
            f: impl FnOnce(&GuardedConn<'_>) -> rusqlite::Result<T>,
        ) -> Result<T, StoreError> {
            let guard = self.lock_conn_within(deadline)?;
            self.fenced_write(&guard, Some(deadline), f)
        }

        fn fenced_write<T>(
            &self,
            conn: &Connection,
            deadline: Option<Instant>,
            f: impl FnOnce(&GuardedConn<'_>) -> rusqlite::Result<T>,
        ) -> Result<T, StoreError> {
            // A superseded writer must not touch the file at all, and the durability pin
            // rewrites the journal mode. This read-only precheck refuses it before the
            // pragmas run; the claim inside the transaction remains the authoritative check.
            let tx = match deadline {
                None => {
                    precheck_fence(conn, self.epoch)?;
                    self.gate.pin_durability_once(conn, None)?;
                    rusqlite::Transaction::new_unchecked(
                        conn,
                        rusqlite::TransactionBehavior::Immediate,
                    )
                    .map_err(|e| StoreError::Backend(e.to_string()))?
                }
                Some(deadline) => {
                    prepare_fenced_within(conn, &self.gate, self.epoch, deadline)?;
                    begin_immediate_within(conn, deadline)?
                }
            };

            claim_fence(&tx, self.epoch)?;

            let scope = CallbackScope::writable(&tx, &self.gate)?;
            let out = f(&GuardedConn::new(&tx, &self.gate));
            // The callback error outranks a release error: losing the reason the
            // write failed is worse than losing the scope-release failure.
            let released = scope.release();
            let out = out.map_err(|e| StoreError::Backend(e.to_string()))?;
            released?;
            tx.commit()
                .map_err(|e| StoreError::Backend(e.to_string()))?;
            Ok(out)
        }
    }

    /// Opens an IMMEDIATE transaction whose wait for another writer ends at `deadline`.
    ///
    /// The busy timeout is connection state, so it is narrowed to the remaining budget for this
    /// `BEGIN` alone and restored to [`BUSY_TIMEOUT`] before returning, whether or not `BEGIN` ran.
    fn begin_immediate_within(
        conn: &Connection,
        deadline: Instant,
    ) -> Result<rusqlite::Transaction<'_>, StoreError> {
        with_busy_timeout_until(conn, deadline, |conn| {
            rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
                .map_err(deadline_on_lock_wait)
        })
    }

    /// Bounds each fence operation by the budget that remains when it starts.
    fn prepare_fenced_within(
        conn: &Connection,
        gate: &AuthorityGate,
        holder_epoch: u64,
        deadline: Instant,
    ) -> Result<(), StoreError> {
        with_busy_timeout_until(conn, deadline, |conn| {
            precheck_fence_via(conn, holder_epoch, deadline_on_lock_wait)
        })?;
        gate.pin_durability_once(conn, Some(deadline))
    }

    /// Runs one potentially blocking statement under the caller's remaining wait budget.
    fn with_busy_timeout_until<'conn, T>(
        conn: &'conn Connection,
        deadline: Instant,
        f: impl FnOnce(&'conn Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(StoreError::Deadline);
        }
        conn.busy_timeout(remaining).map_err(backend_error)?;
        #[cfg(test)]
        observe_busy_wait_for_test();
        let result = f(conn);
        let restored = conn.busy_timeout(BUSY_TIMEOUT).map_err(backend_error);
        with_cleanup_failure(result, restored)
    }

    /// `DatabaseBusy` and `DatabaseLocked` return [`StoreError::Deadline`] after the
    /// caller's deadline sets the busy timeout.
    fn deadline_on_lock_wait(e: rusqlite::Error) -> StoreError {
        match &e {
            rusqlite::Error::SqliteFailure(failure, _)
                if matches!(
                    failure.code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                ) =>
            {
                StoreError::Deadline
            }
            _ => backend_error(e),
        }
    }

    fn backend_error(e: rusqlite::Error) -> StoreError {
        StoreError::Backend(e.to_string())
    }

    /// Folds a cleanup result into the callback result without discarding either error.
    /// A `StoreError::Backend` keeps its variant and appends the cleanup error message.
    /// Other earlier errors are returned unchanged.
    /// A cleanup failure after success becomes the error.
    fn with_cleanup_failure<T>(
        result: Result<T, StoreError>,
        cleanup: Result<(), StoreError>,
    ) -> Result<T, StoreError> {
        match (result, cleanup) {
            (result, Ok(())) => result,
            (Ok(_), Err(cleanup_error)) => Err(cleanup_error),
            (Err(StoreError::Backend(message)), Err(cleanup_error)) => {
                Err(StoreError::Backend(format!("{message}; {cleanup_error}")))
            }
            (Err(earlier), Err(_)) => Err(earlier),
        }
    }

    /// Everything the connection derived before a maintenance callback is discarded when
    /// the callback ends, including by unwinding: the maintenance path is unrestricted, so
    /// the schema, the temp objects, the durability pragmas, and the statement cache may
    /// all have changed. The authorizer runs at prepare time and a cached statement is not
    /// re-authorized on reuse, so nothing prepared under the unrestricted mode may remain in
    /// the cache when a guarded callback runs.
    struct MaintenanceExit<'a> {
        conn: &'a Connection,
        gate: &'a AuthorityGate,
    }

    impl Drop for MaintenanceExit<'_> {
        fn drop(&mut self) {
            self.conn.flush_prepared_statement_cache();
            self.gate.forget_after_maintenance();
        }
    }

    /// `Connection::authorizer` takes `&self`, so a callback holding `&Connection` could
    /// replace the gate installed by [`SqliteStore`]. `GuardedConn` omits authorizer
    /// control, pragma writes, statement batches, and transaction control.
    pub struct GuardedConn<'a> {
        conn: &'a Connection,
        #[cfg(any(test, feature = "test-support"))]
        gate: &'a AuthorityGate,
    }

    /// A statement from the connection's cache, returned to it on drop. Dereferences to the
    /// prepared statement.
    pub struct CachedStatement<'a> {
        inner: rusqlite::CachedStatement<'a>,
        /// The armed probe and the cache key, so the run count can be recorded when the
        /// statement returns to the cache.
        #[cfg(any(test, feature = "test-support"))]
        probe: Option<(&'a AuthorityGate, String)>,
    }

    impl<'a> Deref for CachedStatement<'a> {
        type Target = rusqlite::Statement<'a>;

        fn deref(&self) -> &rusqlite::Statement<'a> {
            &self.inner
        }
    }

    impl<'a> DerefMut for CachedStatement<'a> {
        fn deref_mut(&mut self) -> &mut rusqlite::Statement<'a> {
            &mut self.inner
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    impl Drop for CachedStatement<'_> {
        fn drop(&mut self) {
            if let Some((gate, sql)) = &self.probe {
                gate.record_statement_return(sql, &self.inner);
            }
        }
    }

    /// Bytes the SQLite library holds through its allocator, across every connection in
    /// the process, so an assertion on a delta must leave room for concurrent connections.
    #[cfg(any(test, feature = "test-support"))]
    pub fn library_memory_used() -> i64 {
        // SAFETY: `sqlite3_memory_used` takes no pointers and reads no Rust-managed memory.
        unsafe { rusqlite::ffi::sqlite3_memory_used() }
    }

    /// Reaches pragmas and statement batches but not the authorizer, so a maintenance
    /// callback cannot replace the gate installed by [`SqliteStore`].
    pub struct MaintenanceConn<'a> {
        conn: &'a Connection,
    }

    impl<'a> MaintenanceConn<'a> {
        fn new(conn: &'a Connection) -> Self {
            Self { conn }
        }

        /// # Errors
        ///
        /// Returns the SQLite error from preparing or running `sql`, or from `f`.
        pub fn query_row<T, P, F>(&self, sql: &str, params: P, f: F) -> rusqlite::Result<T>
        where
            P: rusqlite::Params,
            F: FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
        {
            self.conn.query_row(sql, params, f)
        }

        /// # Errors
        ///
        /// Returns the SQLite error from preparing or running `sql`.
        pub fn execute<P: rusqlite::Params>(
            &self,
            sql: &str,
            params: P,
        ) -> rusqlite::Result<usize> {
            self.conn.execute(sql, params)
        }

        /// # Errors
        ///
        /// Returns the SQLite error from running any statement in `sql`.
        pub fn execute_batch(&self, sql: &str) -> rusqlite::Result<()> {
            self.conn.execute_batch(sql)
        }

        /// # Errors
        ///
        /// Returns the SQLite error from preparing `sql`.
        pub fn prepare(&self, sql: &str) -> rusqlite::Result<rusqlite::Statement<'a>> {
            self.conn.prepare(sql)
        }

        /// # Errors
        ///
        /// Returns the SQLite error from setting the pragma.
        pub fn pragma_update(
            &self,
            schema: Option<&str>,
            name: &str,
            value: impl rusqlite::ToSql,
        ) -> rusqlite::Result<()> {
            self.conn.pragma_update(schema, name, value)
        }

        /// Register the function before the first statement that fires a baseline trigger
        /// calling it; SQLite resolves function names at statement preparation, not at
        /// trigger creation. Registration is connection-local configuration, so no
        /// authorizer sees it. Only the maintenance handle exposes it because registered
        /// functions run arbitrary code during later statements.
        ///
        /// A trigger on `fence` or `format_marker` must not call the function.
        /// [`open_sqlite`] writes those tables before any maintenance handle exists, so
        /// such a trigger fails the open with `no such function`.
        ///
        /// The function runs on the thread executing the statement that invokes it, while
        /// that statement's callback holds the store's connection lock. A call from the
        /// function into the same [`SqliteStore`] returns [`StoreError::Backend`]; the
        /// function may read only its arguments and captured state.
        ///
        /// The connection owns the function for the rest of the store's lifetime. A function
        /// that captures an `Arc` of its own store therefore keeps the connection open and
        /// the lease held after every other handle drops; capture a `Weak` instead.
        ///
        /// # Errors
        ///
        /// Returns the SQLite error from registering the function.
        pub fn create_scalar_function<F, T>(
            &self,
            name: &str,
            argument_count: c_int,
            flags: rusqlite::functions::FunctionFlags,
            function: F,
        ) -> rusqlite::Result<()>
        where
            F: Fn(&rusqlite::functions::Context<'_>) -> rusqlite::Result<T> + Send + 'static,
            T: rusqlite::types::ToSql,
        {
            self.conn
                .create_scalar_function(name, argument_count, flags, function)
        }

        /// Statements cached inside a fenced callback survive to the next fenced callback,
        /// because a mode switch expires nothing. A read callback's `query_only` toggle is a
        /// flag pragma, which expires every statement on the connection, and a foreign
        /// main-schema change stales each cached statement's schema cookie so it is
        /// re-prepared on its next run. Size the cache for the distinct statements of the
        /// longest run of fenced callbacks.
        ///
        /// Only a guarded callback may populate the cache: [`MaintenanceConn`] exposes no
        /// cached preparation, the store's own statements are uncached, and the cache is
        /// flushed when a maintenance callback returns.
        pub fn set_prepared_statement_cache_capacity(&self, capacity: usize) {
            self.conn.set_prepared_statement_cache_capacity(capacity);
        }
    }

    impl<'a> GuardedConn<'a> {
        fn new(conn: &'a Connection, gate: &'a AuthorityGate) -> Self {
            #[cfg(not(any(test, feature = "test-support")))]
            let _ = gate;
            Self {
                conn,
                #[cfg(any(test, feature = "test-support"))]
                gate,
            }
        }

        /// # Errors
        ///
        /// Returns the SQLite error from preparing or running `sql`, or from `f`.
        pub fn query_row<T, P, F>(&self, sql: &str, params: P, f: F) -> rusqlite::Result<T>
        where
            P: rusqlite::Params,
            F: FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
        {
            self.conn.query_row(sql, params, f)
        }

        /// # Errors
        ///
        /// Returns the SQLite error from preparing or running `sql`.
        pub fn execute<P: rusqlite::Params>(
            &self,
            sql: &str,
            params: P,
        ) -> rusqlite::Result<usize> {
            self.conn.execute(sql, params)
        }

        /// # Errors
        ///
        /// Returns the SQLite error from preparing `sql`.
        pub fn prepare(&self, sql: &str) -> rusqlite::Result<rusqlite::Statement<'a>> {
            self.conn.prepare(sql)
        }

        /// A cached write meets `query_only` in a read callback, and a cached pragma
        /// write meets the authorizer, as uncached statements do.
        ///
        /// # Errors
        ///
        /// Returns the SQLite error from preparing `sql`.
        pub fn prepare_cached(&self, sql: &str) -> rusqlite::Result<CachedStatement<'a>> {
            let inner = self.conn.prepare_cached(sql)?;
            Ok(CachedStatement {
                #[cfg(any(test, feature = "test-support"))]
                probe: self.gate.record_statement_handout(sql, &inner),
                inner,
            })
        }

        pub fn last_insert_rowid(&self) -> i64 {
            self.conn.last_insert_rowid()
        }

        pub fn changes(&self) -> u64 {
            self.conn.changes()
        }

        /// Returns cumulative row changes over the connection lifetime, including earlier
        /// callbacks, maintenance, and the rows triggers and foreign-key actions changed.
        /// A caller wanting one callback's total takes the difference between two reads
        /// inside that callback.
        pub fn total_changes(&self) -> u64 {
            self.conn.total_changes()
        }
    }

    enum ConnectionMode {
        /// The rest state: store-issued statements and [`SqliteStore::with_conn_unfenced`]
        /// callbacks. Statements prepared here never enter the statement cache.
        Unrestricted,
        /// A [`SqliteStore::with_conn`] or [`SqliteStore::with_conn_fenced`] callback:
        /// `deny_scope_escapes` applies with the main-schema names of the snapshot taken at
        /// callback entry. The two callback kinds share one prepare-time policy, so a
        /// statement one of them cached may run in the other; `query_only` separates them
        /// at step time.
        Guarded(Arc<SchemaSnapshot>),
        /// The baseline DDL applied to a pristine file: `deny_baseline_escapes` applies.
        Baseline,
    }

    /// The main-schema facts a guarded callback needs, keyed on the header schema version
    /// and the pager data version. Every main-schema DDL on any connection bumps the schema
    /// version. The schema version is a stored header field, so a foreign connection could
    /// write it back; that write is a commit, and every foreign commit moves the data
    /// version, which is computed by the pager and stored nowhere. On this connection
    /// defensive mode refuses `PRAGMA schema_version = N` and `writable_schema = ON`.
    struct SchemaSnapshot {
        key: SchemaKey,
        /// Lower-cased main-schema names; a temp object under one of them would capture the
        /// writes a callback believes it makes to the store.
        main_names: HashSet<String>,
        /// Infrastructure-named objects in the main and temp schemas at scan time, tagged
        /// with schema and type. A guarded callback cannot create a temp object under an
        /// infrastructure name, and the maintenance path discards the snapshot, so the
        /// temp half is compared only when the schema version moved.
        infrastructure: Vec<String>,
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    struct SchemaKey {
        schema_version: i64,
        data_version: i64,
    }

    impl SchemaKey {
        /// Both pragmas read the current transaction's page one, so inside a transaction
        /// the key names the schema that transaction sees.
        fn read(conn: &Connection) -> Result<Self, StoreError> {
            Ok(Self {
                schema_version: schema_version(conn)?,
                data_version: conn
                    .query_row("PRAGMA data_version", [], |row| row.get(0))
                    .map_err(|e| StoreError::Backend(e.to_string()))?,
            })
        }
    }

    /// A snapshot above this size is used for its callback but not retained, so the
    /// per-connection retention this crate adds stays within the declared bound. A consumer
    /// baseline whose names exceed the bound rescans on every callback.
    pub const SCHEMA_SNAPSHOT_RETAINED_BYTES_BOUND: usize = 64 * 1024;

    impl SchemaSnapshot {
        /// Reads the key and scans inside the caller's transaction, so the key names the
        /// schema the scan saw.
        fn scan(conn: &Connection) -> Result<Self, StoreError> {
            Ok(Self {
                key: SchemaKey::read(conn)?,
                main_names: main_schema_names(conn)?,
                infrastructure: infrastructure_objects(conn)?,
            })
        }

        /// Heap the snapshot retains: hashbrown holds seven entries per eight buckets and
        /// sizes to a power of two, each bucket an inline `String` plus one control byte.
        fn retained_bytes(&self) -> usize {
            let buckets = (self.main_names.capacity() * 8 / 7).next_power_of_two();
            std::mem::size_of::<Self>()
                + buckets * (std::mem::size_of::<String>() + 1)
                + self.main_names.iter().map(String::capacity).sum::<usize>()
                + self.infrastructure.capacity() * std::mem::size_of::<String>()
                + self
                    .infrastructure
                    .iter()
                    .map(String::capacity)
                    .sum::<usize>()
        }
    }

    fn schema_version(conn: &Connection) -> Result<i64, StoreError> {
        conn.query_row("PRAGMA schema_version", [], |row| row.get(0))
            .map_err(|e| StoreError::Backend(e.to_string()))
    }

    struct GateState {
        mode: ConnectionMode,
        /// Discarded by the maintenance path.
        schema: Option<Arc<SchemaSnapshot>>,
        /// Whether `pin_fence_durability` has run on the connection since open or since the
        /// last maintenance callback, which may have lowered what it pinned.
        durability_pinned: bool,
        /// Armed by a test through [`SqliteStore::start_statement_reuse_probe`]; `None`
        /// costs one load per cached preparation. Per cache key: the highest run count a
        /// returned handle carried, and how many handouts with no runs followed a return
        /// that had run. That sequence means the statement cache re-created the handle; a
        /// handle that was prepared and never stepped keeps a zero run count without an
        /// eviction.
        #[cfg(any(test, feature = "test-support"))]
        statement_probe: Option<std::collections::BTreeMap<String, StatementReuse>>,
    }

    #[cfg(any(test, feature = "test-support"))]
    #[derive(Clone, Copy, Default)]
    struct StatementReuse {
        max_runs: i32,
        evictions: u32,
    }

    /// `sqlite3_set_authorizer` with a non-null hook expires every prepared statement, so the
    /// store installs its hook once, at open, and callbacks switch the mode the hook reads.
    /// The same state carries the per-connection caches the maintenance path invalidates.
    ///
    /// The lock is uncontended: the hook runs on the thread preparing a statement, and that
    /// thread holds the store's connection lock while it switches the mode. The hook holds
    /// the lock while it evaluates a policy, so a policy must not prepare a statement, and
    /// no method holds the lock across a statement.
    struct AuthorityGate {
        state: Mutex<GateState>,
    }

    impl AuthorityGate {
        fn install(conn: Connection) -> Result<ClaimedConnection, StoreError> {
            // Defensive mode refuses the statements that rewrite schema state without DDL:
            // `PRAGMA writable_schema = ON`, `PRAGMA schema_version = N`, and
            // `PRAGMA journal_mode = OFF`.
            conn.set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)
                .map_err(|e| StoreError::Backend(e.to_string()))?;
            let gate = Arc::new(Self {
                state: Mutex::new(GateState {
                    mode: ConnectionMode::Unrestricted,
                    schema: None,
                    durability_pinned: false,
                    #[cfg(any(test, feature = "test-support"))]
                    statement_probe: None,
                }),
            });
            let hook = Arc::clone(&gate);
            conn.authorizer(Some(move |context: rusqlite::hooks::AuthContext<'_>| {
                hook.authorize(context)
            }))
            .map_err(|e| StoreError::Backend(e.to_string()))?;
            Ok(ClaimedConnection { conn, gate })
        }

        fn lock(&self) -> MutexGuard<'_, GateState> {
            self.state.lock().unwrap_or_else(|p| p.into_inner())
        }

        fn authorize(
            &self,
            context: rusqlite::hooks::AuthContext<'_>,
        ) -> rusqlite::hooks::Authorization {
            match &self.lock().mode {
                ConnectionMode::Unrestricted => rusqlite::hooks::Authorization::Allow,
                ConnectionMode::Guarded(snapshot) => {
                    deny_scope_escapes(context, &snapshot.main_names)
                }
                ConnectionMode::Baseline => deny_baseline_escapes(context),
            }
        }

        /// Holds are not nested: the connection lock serializes callbacks, and the store
        /// enters a mode only from the rest state.
        fn enter(&self, mode: ConnectionMode) -> ModeHold<'_> {
            let mut state = self.lock();
            debug_assert!(
                matches!(state.mode, ConnectionMode::Unrestricted),
                "a mode was entered while another mode was held"
            );
            state.mode = mode;
            ModeHold { gate: self }
        }

        /// The retained snapshot when its key still matches, otherwise a fresh scan that is
        /// retained when it fits the declared bound. The lock is released before every
        /// statement, because the authorizer takes it during prepare.
        fn schema_snapshot(&self, conn: &Connection) -> Result<Arc<SchemaSnapshot>, StoreError> {
            let key = SchemaKey::read(conn)?;
            let retained = self.lock().schema.clone();
            if let Some(snapshot) = retained.filter(|snapshot| snapshot.key == key) {
                return Ok(snapshot);
            }
            let snapshot = Arc::new(SchemaSnapshot::scan(conn)?);
            if snapshot.retained_bytes() <= SCHEMA_SNAPSHOT_RETAINED_BYTES_BOUND {
                self.lock().schema = Some(Arc::clone(&snapshot));
            }
            Ok(snapshot)
        }

        /// Pins durability unless the pin has held since open or since the last maintenance
        /// callback. Guarded callbacks cannot touch the durability pragmas, `synchronous`
        /// is connection-local, and another connection cannot leave WAL while this one
        /// holds the database open, so the pin holds until the maintenance path runs.
        fn pin_durability_once(
            &self,
            conn: &Connection,
            deadline: Option<Instant>,
        ) -> Result<(), StoreError> {
            if self.lock().durability_pinned {
                return Ok(());
            }
            match deadline {
                Some(deadline) => with_busy_timeout_until(conn, deadline, |conn| {
                    pin_fence_durability_via(conn, deadline_on_lock_wait)
                })?,
                None => pin_fence_durability(conn)?,
            }
            self.lock().durability_pinned = true;
            Ok(())
        }

        /// SQLite keeps the run count on a handle it re-prepares in place, so a handle with
        /// no runs after a returned handle of the same key had run is a handle the
        /// statement cache re-created. The key is trimmed as the cache trims it. Returns
        /// the probe reference the statement records its return against, when armed.
        #[cfg(any(test, feature = "test-support"))]
        fn record_statement_handout<'g>(
            &'g self,
            sql: &str,
            statement: &rusqlite::Statement<'_>,
        ) -> Option<(&'g AuthorityGate, String)> {
            let runs = statement.get_status(rusqlite::StatementStatus::Run);
            let mut state = self.lock();
            let probe = state.statement_probe.as_mut()?;
            let key = sql.trim().to_string();
            let reuse = probe.entry(key.clone()).or_default();
            if runs == 0 && reuse.max_runs > 0 {
                reuse.evictions += 1;
            }
            Some((self, key))
        }

        #[cfg(any(test, feature = "test-support"))]
        fn record_statement_return(&self, key: &str, statement: &rusqlite::Statement<'_>) {
            let runs = statement.get_status(rusqlite::StatementStatus::Run);
            let mut state = self.lock();
            if let Some(reuse) = state
                .statement_probe
                .as_mut()
                .and_then(|probe| probe.get_mut(key))
            {
                reuse.max_runs = reuse.max_runs.max(runs);
            }
        }

        /// The maintenance path is unrestricted, so it may have changed the schema, the
        /// temp objects, or the durability pragmas; nothing derived before it is trusted.
        fn forget_after_maintenance(&self) {
            let mut state = self.lock();
            state.schema = None;
            state.durability_pinned = false;
        }
    }

    /// Returns the gate to [`ConnectionMode::Unrestricted`] on drop, including on unwind.
    struct ModeHold<'g> {
        gate: &'g AuthorityGate,
    }

    impl Drop for ModeHold<'_> {
        fn drop(&mut self) {
            self.gate.lock().mode = ConnectionMode::Unrestricted;
        }
    }

    /// The connection is shared by reads, fenced writes, and maintenance, so
    /// a callback that keeps the full capability of the connection can leave the scope it
    /// was given: any pragma write reconfigures every later statement, and transaction
    /// control ends the transaction whose fence check authorized the callback.
    /// `CallbackScope` withdraws those capabilities for the callback's duration, and
    /// restores them even when the callback unwinds, because a poisoned lock is recovered
    /// and hands the same connection to the next caller.
    struct CallbackScope<'c> {
        /// `None` once left, so `Drop` never repeats a release whose failure
        /// [`Self::release`] already reported.
        active: Option<(&'c Connection, ModeHold<'c>)>,
        /// The `query_only` value to restore: `Some(prior)` when the scope switched it on
        /// for a read, `None` when the scope left it alone.
        query_only_before: Option<bool>,
        /// The snapshot the callback runs under; its infrastructure objects are compared
        /// again at release when the schema version moved.
        snapshot: Arc<SchemaSnapshot>,
    }

    /// Every main- or temp-schema object whose name is an infrastructure table name,
    /// tagged with its schema and type so a swap of table for view is visible.
    /// Match names in Rust because a maintenance-handle scalar function can replace SQLite `lower`.
    fn infrastructure_objects(conn: &Connection) -> Result<Vec<String>, StoreError> {
        let mut statement = conn
            .prepare(
                "SELECT 'main', type, name FROM main.sqlite_schema \
                 UNION ALL \
                 SELECT 'temp', type, name FROM temp.sqlite_schema \
                 ORDER BY 1, 2, 3",
            )
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        rows.filter_map(|row| match row {
            Ok((schema, kind, name)) if is_infrastructure_table(&name) => {
                Some(Ok(format!("{schema}.{name} {kind}")))
            }
            Ok(_) => None,
            Err(e) => Some(Err(StoreError::Backend(e.to_string()))),
        })
        .collect()
    }

    /// Lower-cased names of every main-schema object; SQLite resolves unqualified names
    /// through the temp schema first, so a temp object under one of these names would
    /// capture the writes a callback believes it makes to the store.
    /// Lower-case in Rust because a maintenance-handle scalar function can replace SQLite `lower`.
    fn main_schema_names(
        conn: &Connection,
    ) -> Result<std::collections::HashSet<String>, StoreError> {
        let mut statement = conn
            .prepare("SELECT name FROM main.sqlite_schema")
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        let rows = statement
            .query_map([], |row| Ok(row.get::<_, String>(0)?.to_ascii_lowercase()))
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        rows.collect::<Result<_, _>>()
            .map_err(|e| StoreError::Backend(e.to_string()))
    }

    /// The first temp-schema object whose lower-cased name is one of `main_names`, if any.
    fn temp_shadow_of(
        conn: &Connection,
        main_names: &std::collections::HashSet<String>,
    ) -> Result<Option<String>, StoreError> {
        let mut statement = conn
            .prepare("SELECT name FROM temp.sqlite_schema ORDER BY name")
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        for name in rows {
            let name = name.map_err(|e| StoreError::Backend(e.to_string()))?;
            if main_names.contains(&name.to_ascii_lowercase()) {
                return Ok(Some(name));
            }
        }
        Ok(None)
    }

    impl<'c> CallbackScope<'c> {
        /// Denies writes as well as the escapes, for a callback that must not mutate.
        fn read_only(conn: &'c Connection, gate: &'c AuthorityGate) -> Result<Self, StoreError> {
            let prior: bool = conn
                .query_row("PRAGMA query_only", [], |row| row.get(0))
                .map_err(|e| StoreError::Backend(e.to_string()))?;
            conn.pragma_update(None, "query_only", "ON")
                .map_err(|e| StoreError::Backend(e.to_string()))?;
            // A failure between the pragma and the finished scope would leave the shared
            // connection read-only with no guard to restore it.
            Self::install(conn, gate, Some(prior)).inspect_err(|_| {
                let _ = Self::restore(conn, Some(prior));
            })
        }

        /// Denies the escapes only, for a callback inside a fence-checked transaction.
        fn writable(conn: &'c Connection, gate: &'c AuthorityGate) -> Result<Self, StoreError> {
            Self::install(conn, gate, None)
        }

        fn install(
            conn: &'c Connection,
            gate: &'c AuthorityGate,
            query_only_before: Option<bool>,
        ) -> Result<Self, StoreError> {
            let snapshot = gate.schema_snapshot(conn)?;
            // The authorizer denies a callback from creating a shadow, but maintenance
            // through `with_conn_unfenced` may already have left one on this connection,
            // and a callback's unqualified statement would resolve to it instead of the
            // store's object. The temp schema is normally empty and has no version to key
            // on, so this scan stays uncached.
            if let Some(shadow) = temp_shadow_of(conn, &snapshot.main_names)? {
                return Err(StoreError::Backend(format!(
                    "temporary object `{shadow}` shadows the store's `{shadow}` on this \
                     connection; drop it before running a callback"
                )));
            }
            Ok(Self {
                active: Some((
                    conn,
                    gate.enter(ConnectionMode::Guarded(Arc::clone(&snapshot))),
                )),
                query_only_before,
                snapshot,
            })
        }

        /// Reports a failed release to the caller, which `Drop` cannot do. The infrastructure
        /// refusal is the primary error; a failed `query_only` restore is appended to it.
        fn release(mut self) -> Result<(), StoreError> {
            let Some((conn, _)) = self.active.as_ref() else {
                return Ok(());
            };
            let unchanged = Self::require_infrastructure_unchanged(conn, &self.snapshot);
            with_cleanup_failure(unchanged, self.leave())
        }

        /// Leaves the guarded mode, then restores `query_only`, in that order: the guarded
        /// policy denies the restoring pragma write.
        fn leave(&mut self) -> Result<(), StoreError> {
            match self.active.take() {
                Some((conn, hold)) => {
                    drop(hold);
                    Self::restore(conn, self.query_only_before)
                }
                None => Ok(()),
            }
        }

        /// `AuthAction::AlterTable` reports the source name, so a rename cannot be judged
        /// when it is authorized: a table the callback created and renamed to an
        /// infrastructure name is trusted by the next fence claim. Every main-schema
        /// change bumps the schema version, so an unchanged version leaves the snapshot's
        /// infrastructure objects standing; a changed one is rescanned and compared before
        /// the callback's transaction commits.
        fn require_infrastructure_unchanged(
            conn: &Connection,
            snapshot: &SchemaSnapshot,
        ) -> Result<(), StoreError> {
            if schema_version(conn)? == snapshot.key.schema_version {
                return Ok(());
            }
            let after = infrastructure_objects(conn)?;
            if after != snapshot.infrastructure {
                return Err(StoreError::Backend(format!(
                    "the callback changed the infrastructure schema objects from {:?} to {after:?}",
                    snapshot.infrastructure
                )));
            }
            Ok(())
        }

        /// Puts `query_only` back to the value the scope found, so a connection that
        /// maintenance left read-only stays read-only.
        fn restore(conn: &Connection, query_only_before: Option<bool>) -> Result<(), StoreError> {
            match query_only_before {
                Some(prior) => conn.pragma_update(None, "query_only", prior).map_err(|e| {
                    StoreError::Backend(format!("failed to release the callback scope: {e}"))
                }),
                None => Ok(()),
            }
        }
    }

    impl Drop for CallbackScope<'_> {
        fn drop(&mut self) {
            // Drop ignores cleanup errors because it cannot return them.
            let _ = self.leave();
        }
    }

    /// A baseline is DDL for the main schema and nothing else, and it writes no row into the
    /// store's own tables. An attached database is
    /// outside the file and outside the identity comparison, and a file-backed attachment
    /// reaches a database the store holds no lease on. A pragma write outlives the
    /// baseline: `writable_schema` or `ignore_check_constraints` set here would stay in
    /// force on the connection handed to callbacks, whose authorizer denies pragma writes
    /// only from that point on. Transaction control would break out of the `IMMEDIATE`
    /// transaction the baseline is applied inside.
    fn deny_baseline_escapes(
        context: rusqlite::hooks::AuthContext<'_>,
    ) -> rusqlite::hooks::Authorization {
        use rusqlite::hooks::{AuthAction, Authorization};
        match context.action {
            AuthAction::Attach { .. }
            | AuthAction::Detach { .. }
            | AuthAction::Transaction { .. }
            | AuthAction::Savepoint { .. }
            | AuthAction::Pragma {
                pragma_value: Some(_),
                ..
            } => Authorization::Deny,
            AuthAction::Pragma {
                pragma_name,
                pragma_value: None,
            } if is_side_effecting_pragma(pragma_name) => Authorization::Deny,
            // A row written into `fence` or `format_marker` by the baseline would be the
            // authority every later open checks against; the first claim and the marker
            // write are the only writers of those tables. The baseline's own DDL creates
            // them, so only row operations are denied here.
            AuthAction::Insert { table_name }
            | AuthAction::Update { table_name, .. }
            | AuthAction::Delete { table_name }
                if is_infrastructure_table(table_name) =>
            {
                Authorization::Deny
            }
            _ => Authorization::Allow,
        }
    }

    /// Pragmas whose argument names a schema object to describe rather than a value to set.
    /// Every other pragma carrying an argument is treated as a write.
    const SCHEMA_INTROSPECTION_PRAGMAS: &[&str] = &[
        "table_info",
        "table_xinfo",
        "table_list",
        "index_list",
        "index_info",
        "index_xinfo",
        "foreign_key_list",
    ];

    /// SQLite pragma names are case-insensitive, so the comparison ignores ASCII case.
    fn pragma_in(name: &str, list: &[&str]) -> bool {
        list.iter().any(|pragma| name.eq_ignore_ascii_case(pragma))
    }

    /// A pragma denylist is never complete, so every value-carrying pragma is denied
    /// unless allowlisted.
    /// `ignore_check_constraints` disables CHECK constraint enforcement.
    /// `defer_foreign_keys` defers foreign-key enforcement.
    /// `writable_schema` permits direct schema-table writes.
    fn pragma_authorization(
        pragma_name: &str,
        pragma_value: Option<&str>,
    ) -> rusqlite::hooks::Authorization {
        use rusqlite::hooks::Authorization;
        match pragma_value {
            // Both `PRAGMA table_info(t)` and `pragma_table_info('t')` are read-only.
            // Both forms report the table name as the pragma value.
            // The statement form reports the pragma name as the caller spelled it.
            Some(_) if pragma_in(pragma_name, SCHEMA_INTROSPECTION_PRAGMAS) => Authorization::Allow,
            Some(_) => Authorization::Deny,
            None if is_side_effecting_pragma(pragma_name) => Authorization::Deny,
            None => Authorization::Allow,
        }
    }

    /// Denies every statement that escapes the callback's scope.
    /// Ordinary statements that do not target an infrastructure table stay allowed.
    ///
    /// Main-schema DDL is denied whatever it targets: the file's schema is the baseline
    /// the next open compares against, so a committed `CREATE`, `ALTER`, or `DROP` would
    /// make the store fail to reopen. Temporary objects live outside that comparison and
    /// stay allowed unless they carry an infrastructure name, shadow a main-schema name, or
    /// are triggers on a main-schema table: any of those would redirect or rewrite the
    /// writes of this and every later callback on the shared connection.
    fn deny_scope_escapes(
        context: rusqlite::hooks::AuthContext<'_>,
        main_names: &std::collections::HashSet<String>,
    ) -> rusqlite::hooks::Authorization {
        use rusqlite::hooks::{AuthAction, Authorization};
        let shadows = |name: &str| main_names.contains(&name.to_ascii_lowercase());
        match context.action {
            AuthAction::CreateTempTable { table_name } if shadows(table_name) => {
                Authorization::Deny
            }
            AuthAction::CreateTempView { view_name } if shadows(view_name) => Authorization::Deny,
            AuthAction::CreateTempIndex { index_name, .. } if shadows(index_name) => {
                Authorization::Deny
            }
            AuthAction::CreateTempTrigger {
                trigger_name,
                table_name,
            } if shadows(trigger_name) || shadows(table_name) => Authorization::Deny,
            AuthAction::Pragma {
                pragma_name,
                pragma_value,
            } => pragma_authorization(pragma_name, pragma_value),
            AuthAction::Transaction { .. }
            | AuthAction::Savepoint { .. }
            | AuthAction::Attach { .. }
            | AuthAction::Detach { .. } => Authorization::Deny,
            AuthAction::CreateTable { .. }
            | AuthAction::DropTable { .. }
            | AuthAction::AlterTable { .. }
            | AuthAction::CreateIndex { .. }
            | AuthAction::DropIndex { .. }
            | AuthAction::CreateTrigger { .. }
            | AuthAction::DropTrigger { .. }
            | AuthAction::CreateView { .. }
            | AuthAction::DropView { .. }
            | AuthAction::CreateVtable { .. }
            | AuthAction::DropVtable { .. } => Authorization::Deny,
            action => match infrastructure_target(&action) {
                Some(_) => Authorization::Deny,
                None => Authorization::Allow,
            },
        }
    }

    /// `PRAGMA query_only` does not stop these: they take no value yet checkpoint, vacuum,
    /// reorganize, or release memory on the shared connection.
    fn is_side_effecting_pragma(name: &str) -> bool {
        pragma_in(
            name,
            &[
                "wal_checkpoint",
                "incremental_vacuum",
                "optimize",
                "shrink_memory",
            ],
        )
    }

    /// Reads leave the row's authority intact, so they stay allowed. Every other action
    /// naming an infrastructure table is refused, including the schema operations that
    /// reach the row indirectly: a `BEFORE UPDATE` trigger raising `IGNORE` suppresses a
    /// later opener's fence claim while that claim still reports success.
    fn infrastructure_target<'a>(action: &rusqlite::hooks::AuthAction<'a>) -> Option<&'a str> {
        use rusqlite::hooks::AuthAction;
        let table = match *action {
            AuthAction::Read { .. } | AuthAction::Select => return None,
            AuthAction::Insert { table_name }
            | AuthAction::Update { table_name, .. }
            | AuthAction::Delete { table_name }
            | AuthAction::CreateTable { table_name }
            | AuthAction::CreateTempTable { table_name }
            | AuthAction::DropTable { table_name }
            | AuthAction::DropTempTable { table_name }
            | AuthAction::AlterTable { table_name, .. }
            | AuthAction::CreateIndex { table_name, .. }
            | AuthAction::CreateTempIndex { table_name, .. }
            | AuthAction::DropIndex { table_name, .. }
            | AuthAction::DropTempIndex { table_name, .. }
            | AuthAction::CreateTrigger { table_name, .. }
            | AuthAction::CreateTempTrigger { table_name, .. }
            | AuthAction::DropTrigger { table_name, .. }
            | AuthAction::DropTempTrigger { table_name, .. }
            | AuthAction::Reindex {
                index_name: table_name,
            }
            | AuthAction::Analyze { table_name }
            | AuthAction::CreateVtable { table_name, .. }
            | AuthAction::DropVtable { table_name, .. } => table_name,
            // A view resolves ahead of the table it shadows on this connection, so a
            // forged `fence` would let a stale writer read its own epoch.
            AuthAction::CreateView { view_name }
            | AuthAction::CreateTempView { view_name }
            | AuthAction::DropView { view_name }
            | AuthAction::DropTempView { view_name } => view_name,
            _ => return None,
        };
        is_infrastructure_table(table).then_some(table)
    }

    /// The fence row carries the authority a fenced write is checked against, and the
    /// format marker carries the file's schema identity. A callback that changed
    /// either, or the schema reaching either, would let a superseded writer reclaim the
    /// database or pass a foreign file off as this baseline.
    fn is_infrastructure_table(table_name: &str) -> bool {
        INFRASTRUCTURE_TABLES
            .iter()
            .any(|name| table_name.eq_ignore_ascii_case(name))
    }

    /// `with_conn_unfenced` remains unrestricted by contract, so `synchronous` and the
    /// journal mode can still be lowered between protected transactions. With WAL and
    /// `synchronous=NORMAL`, power loss can roll back committed transactions.
    fn pin_fence_durability(conn: &Connection) -> Result<(), StoreError> {
        pin_fence_durability_via(conn, backend_error)
    }

    fn pin_fence_durability_via(
        conn: &Connection,
        map: fn(rusqlite::Error) -> StoreError,
    ) -> Result<(), StoreError> {
        conn.pragma_update(None, "synchronous", "FULL")
            .map_err(map)?;
        let mode: String = conn
            .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
            .map_err(map)?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(StoreError::Backend(format!(
                "fenced writes require a crash-safe journal, but journal_mode is {mode}"
            )));
        }
        Ok(())
    }

    /// Open a module's SQLite store from its descriptor and its baseline DDL.
    ///
    /// `baseline` is the consumer's complete schema as one DDL text; the store's own
    /// objects (the fence and the format marker from `baseline.sql`) precede it. A
    /// pristine file (no schema, zero `application_id`, zero `user_version`) receives
    /// the whole baseline once, together with [`APPLICATION_ID`], [`USER_VERSION`],
    /// and a format-marker row holding the SHA-256 of the baseline text. Any other
    /// file must carry exactly that identity, object for object, or the open returns
    /// [`StoreError::Baseline`] before writing a byte. No file is upgraded, adopted,
    /// or repaired.
    ///
    /// The returned store has already claimed its lease epoch in the database. The
    /// stored database fence becomes the lease floor, so deleting or restoring an old
    /// lease sidecar cannot reissue an epoch represented in the database.
    ///
    /// The lease lives next to the database file (its parent directory), derived
    /// from the descriptor's path rather than passed in, and its key carries the
    /// module, namespace, and file name. Two distinct database paths get distinct
    /// leases, and descriptors that agree on module and namespace get one lease per
    /// database path, so a second such open returns [`StoreError::Lease`].
    /// Descriptors for one path that disagree on module or namespace derive
    /// separate leases; the fence row, not the lease, is what stops the superseded
    /// store from writing.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::UnsupportedBackend`] for non-SQLite descriptors.
    /// Returns [`StoreError::Io`] when the parent directory or file cannot be created.
    /// Returns [`StoreError::Lease`] when lease acquisition fails.
    /// Returns [`StoreError::Baseline`] when the file is neither pristine nor identical to the baseline identity.
    /// Returns [`StoreError::Fenced`] if the database advances during open.
    /// Returns [`StoreError::FenceCorrupt`] if the stored fence epoch is out of range.
    /// Returns [`StoreError::Backend`] when SQLite inspection, setup, or fence claim fails.
    pub fn open_sqlite(
        descriptor: &StorageDescriptor,
        baseline: &str,
    ) -> Result<SqliteStore, StoreError> {
        let expected = ExpectedIdentity::for_baseline(baseline)?;
        let path = match &descriptor.backend {
            StorageBackend::Sqlite { path } => path.clone(),
            other => return Err(StoreError::UnsupportedBackend(other.label().to_string())),
        };
        // A relative path names a different file after every change of the process's
        // working directory, so one descriptor could open two stores and two leases.
        if !Path::new(&path).is_absolute() {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("sqlite path {path} is not absolute"),
            )));
        }

        let parent = Path::new(&path)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        // The lease refuses a directory another principal can rename in, so a directory
        // this crate creates is owner-only from the start rather than left to the umask.
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&parent).map_err(StoreError::Io)?;

        refuse_unfit_store_files(Path::new(&path))?;
        let db_file_name = Path::new(&path)
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());
        let leases = FileLeaseStore::new(&parent).map_err(StoreError::Io)?;
        let key = lease_key(descriptor, &db_file_name)?;
        // An existing file is inspected on a read-only connection before anything can
        // change it. A read-write open would let SQLite recover or checkpoint a foreign
        // WAL and rewrite the file on close, and a fence row in a foreign file would
        // otherwise raise the lease floor before the file was ever classified. The
        // inspection runs under a shared lease: a live store holds the exclusive lease, so
        // its owner is reported as `Held` rather than read while it checkpoints, and no
        // cooperating writer can start while the inspection copy is taken. The shared
        // lease leaves the persisted epoch alone.
        let inspection_guard = leases.acquire_shared(&key).map_err(StoreError::Lease)?;
        let (epoch_floor, inspected) = expected.inspect_existing(Path::new(&path))?;
        drop(inspection_guard);
        // The lease will issue at least `epoch_floor + 1`, and that value must be storable
        // in the fence row; refusing here keeps an unrepresentable successor out of the
        // lease sidecar, which would otherwise stay poisoned after the row was repaired.
        if epoch_floor >= i64::MAX as u64 {
            return Err(StoreError::FenceExhausted {
                db_epoch: epoch_floor,
            });
        }
        let lease = leases
            .acquire_above(&key, epoch_floor)
            .map_err(StoreError::Lease)?;
        let epoch = lease.epoch();

        let claimed = open_claimed(&path, &expected, inspected, epoch)?;
        Ok(SqliteStore::over(claimed, epoch, Some(lease)))
    }

    /// Opens the read-write connection for `epoch`, which the lease has already issued,
    /// and claims the fence. The file is refused if it was re-pointed since `inspected`
    /// was taken; the connection then classifies it on its own view. On an initialized
    /// file a fence row at or above `epoch` refuses the open before the durability
    /// pragmas, so an opener superseded between inspection and claim leaves the journal
    /// mode as it found it; the strict claim inside the transaction remains the authority
    /// against an advance after that read.
    pub(crate) fn open_claimed(
        path: &str,
        expected: &ExpectedIdentity,
        inspected: Option<FileIdentity>,
        epoch: u64,
    ) -> Result<ClaimedConnection, StoreError> {
        // Owner-only from creation: SQLite gives sidecars the database file's mode.
        create_database_file_owner_only(Path::new(path)).map_err(StoreError::Io)?;
        // `SQLITE_OPEN_NOFOLLOW` closes the window between the owner-only creation and this
        // open, so a symlink swapped in between is refused rather than followed.
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(|e| StoreError::Backend(e.to_string()))?;
        // Installed before the first statement, so no later install expires one.
        let ClaimedConnection { mut conn, gate } = AuthorityGate::install(conn)?;
        // SQLite has opened the file but read nothing: WAL recovery happens on the first
        // read and the close-time checkpoint only after the WAL was opened. A file swapped
        // in since the inspection is refused here, before either can happen. The window
        // between SQLite's own `open(2)` and this `stat(2)` remains; the store directory is
        // created by this crate and the lease serializes cooperating writers.
        if let Some(pinned) = inspected {
            let now = FileIdentity::of_path(Path::new(path)).map_err(StoreError::Io)?;
            if now != pinned {
                return Err(StoreError::Baseline(format!(
                    "{path} was replaced between inspection and open"
                )));
            }
        }
        // The read-write connection re-establishes identity so the connection handed out
        // is classified on its own view of the file, not on the inspection that preceded
        // the lease.
        let state = expected.classify(&conn)?;
        if state == FileState::Baseline {
            precheck_fence(&conn, epoch)?;
        }
        // Pre-existing permissive files are narrowed before the fence claim writes any bytes.
        for suffix in ["", "-wal", "-shm"] {
            protect_file(Path::new(&format!("{path}{suffix}")))
                .map_err(|e| StoreError::Backend(e.to_string()))?;
        }
        // A VFS that cannot switch to WAL answers the pragma with the unchanged
        // mode; the fence claim must not commit under a journal mode every later
        // fenced write would reject.
        gate.pin_durability_once(&conn, None)?;
        // The busy timeout makes transient locks wait rather than fail, and
        // foreign-key enforcement is enabled.
        conn.busy_timeout(BUSY_TIMEOUT)
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| StoreError::Backend(e.to_string()))?;

        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        if state == FileState::Pristine {
            expected.apply(&tx, &gate)?;
        }
        claim_fence_strict(&tx, epoch, state)?;
        tx.commit()
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        Ok(ClaimedConnection { conn, gate })
    }

    /// SQLite URI for `path` with `immutable=1`: the connection takes no locks, reads no
    /// `-wal`, and creates no sidecar. Every byte outside the unreserved ASCII set is
    /// percent-encoded, so `%`, `?`, `#`, spaces, and each byte of a non-ASCII name reach
    /// SQLite's decoder as the bytes the filesystem holds.
    pub(crate) fn immutable_uri(path: &Path) -> String {
        let mut out = String::from("file:");
        for byte in path.as_os_str().as_encoded_bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                    out.push(char::from(*byte))
                }
                _ => out.push_str(&format!("%{byte:02X}")),
            }
        }
        out.push_str("?immutable=1");
        out
    }

    /// A private copy of a database and its sidecars, removed on drop. Opening the copy
    /// read-write replays its WAL or rolls its hot journal back without touching the
    /// original files.
    pub(crate) struct InspectionCopy {
        pub(crate) directory: PathBuf,
        pub(crate) database: PathBuf,
    }

    impl InspectionCopy {
        pub(crate) fn of(path: &Path) -> Result<Self, StoreError> {
            let parent = path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            let file_name = path.file_name().ok_or_else(|| {
                StoreError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("{} has no file name", path.display()),
                ))
            })?;
            let directory = parent.join(format!(
                ".inspect-{}-{}",
                std::process::id(),
                unique_nanos()
            ));
            // The copy holds the store's contents, so the directory is owner-only from
            // creation rather than left to the umask.
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(&directory).map_err(StoreError::Io)?;
            let copy = Self {
                database: directory.join(file_name),
                directory,
            };
            for suffix in ["", "-wal", "-shm", "-journal"] {
                let source = PathBuf::from(format!("{}{suffix}", path.display()));
                let target = PathBuf::from(format!("{}{suffix}", copy.database.display()));
                copy_regular_file(&source, &target)?;
            }
            Ok(copy)
        }
    }

    /// Copies `source` to `target` when `source` exists, opening the source without following
    /// a final symlink and refusing anything that is not a regular file, so a path swapped
    /// for a symlink or a FIFO after the unfit check is neither followed nor waited on.
    fn copy_regular_file(source: &Path, target: &Path) -> Result<(), StoreError> {
        let mut from = match open_no_follow(source) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(StoreError::Io(e)),
        };
        let meta = from.metadata().map_err(StoreError::Io)?;
        if !meta.is_file() {
            return Err(StoreError::Baseline(format!(
                "{} is not a regular file",
                source.display()
            )));
        }
        // The copy holds the store's contents, so it is owner-only from creation rather
        // than left to the umask.
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut to = options.open(target).map_err(StoreError::Io)?;
        std::io::copy(&mut from, &mut to).map_err(StoreError::Io)?;
        Ok(())
    }

    impl Drop for InspectionCopy {
        fn drop(&mut self) {
            // Drop ignores cleanup errors because it cannot return them.
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    fn unique_nanos() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    }

    /// The database and its sidecars must each be a regular file with exactly one name.
    /// A FIFO or device would block or misbehave inside SQLite's open or the inspection
    /// copy before any timeout applies, a symlink is an alias, and two names for one file
    /// derive two leases and two sidecar sets, so neither writer sees the other's fence
    /// claim. The link count comes from an opened handle so the rule holds on Windows too.
    fn refuse_unfit_store_files(path: &Path) -> Result<(), StoreError> {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let candidate = PathBuf::from(format!("{}{suffix}", path.display()));
            let meta = match std::fs::symlink_metadata(&candidate) {
                Ok(meta) => meta,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(StoreError::Io(e)),
            };
            if !meta.is_file() {
                return Err(StoreError::Baseline(format!(
                    "{} is not a regular file",
                    candidate.display()
                )));
            }
            let file = open_no_follow(&candidate).map_err(StoreError::Io)?;
            let names = lease::link_count(&file).map_err(StoreError::Io)?;
            if names > 1 {
                return Err(StoreError::Baseline(format!(
                    "{} has {names} names; a store file must have exactly one",
                    candidate.display()
                )));
            }
        }
        Ok(())
    }

    /// Opens `path` for reading without following a final symlink and without waiting on a
    /// FIFO; on Windows a reparse point is opened as itself.
    fn open_no_follow(path: &Path) -> std::io::Result<std::fs::File> {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
            options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        options.open(path)
    }

    /// Creates the database file with mode `0600` when it does not exist.
    ///
    /// Does nothing to an existing file. On non-Unix targets this is a no-op;
    /// SQLite creates the file itself.
    pub(crate) fn create_database_file_owner_only(path: &Path) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // `O_NOFOLLOW` refuses a symlink at the database path, so a dangling
            // link cannot make creation land outside the store directory.
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(path)?;
        }
        #[cfg(not(unix))]
        let _ = path;
        Ok(())
    }

    /// The store's own fence statements run under the unrestricted mode and never through
    /// the statement cache: a cached statement is not re-authorized on reuse, so one prepared
    /// unrestricted could hand a guarded callback the fence upsert.
    const FENCE_EPOCH_SQL: &str = "SELECT epoch FROM fence WHERE id = 0";
    pub(crate) const FENCE_CLAIM_SQL: &str = "INSERT INTO fence (id, epoch) VALUES (0, ?1) \
         ON CONFLICT(id) DO UPDATE SET epoch = excluded.epoch";

    /// The fence epoch, or `None` when the row is absent. The caller guarantees that the
    /// `fence` table exists; only the transaction that initializes a pristine file may see
    /// no row, since the baseline creates the table and the first claim writes the row.
    fn read_fence_epoch_in(conn: &Connection) -> Result<Option<u64>, StoreError> {
        read_fence_epoch_via(conn, backend_error)
    }

    fn read_fence_epoch_via(
        conn: &Connection,
        map: fn(rusqlite::Error) -> StoreError,
    ) -> Result<Option<u64>, StoreError> {
        let epoch: Option<i64> = conn
            .query_row(FENCE_EPOCH_SQL, [], |row| row.get(0))
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })
            .map_err(map)?;
        epoch.map(decode_fence_epoch).transpose()
    }

    /// One row of `sqlite_schema` without its root page, which varies with allocation
    /// order and carries no identity.
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
    pub struct SchemaObject {
        pub kind: String,
        pub name: String,
        pub table: String,
        pub sql: Option<String>,
    }

    /// `sqlite_schema` of `conn`'s main database in a fixed order.
    pub fn schema_inventory(conn: &Connection) -> Result<Vec<SchemaObject>, StoreError> {
        let mut statement = conn
            .prepare("SELECT type, name, tbl_name, sql FROM main.sqlite_schema ORDER BY type, name")
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        let rows = statement
            .query_map([], |row| {
                Ok(SchemaObject {
                    kind: row.get(0)?,
                    name: row.get(1)?,
                    table: row.get(2)?,
                    sql: row.get(3)?,
                })
            })
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| StoreError::Backend(e.to_string()))
    }

    /// Whether an opened file may receive the baseline or already carries it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum FileState {
        Pristine,
        Baseline,
    }

    /// The identity every file opened against one baseline text must present.
    pub(crate) struct ExpectedIdentity {
        text: String,
        digest: String,
        objects: Vec<SchemaObject>,
    }

    impl ExpectedIdentity {
        /// The inventory comes from applying the text to an in-memory database, so
        /// the comparison uses SQLite's own normalization of the DDL rather than a
        /// second parser.
        pub(crate) fn for_baseline(consumer: &str) -> Result<Self, StoreError> {
            let text = format!("{STORE_BASELINE}\n{consumer}");
            let digest = format!("{:x}", Sha256::digest(text.as_bytes()));
            let scratch =
                Connection::open_in_memory().map_err(|e| StoreError::Backend(e.to_string()))?;
            // The authorizer runs before each statement, so an `ATTACH` is refused before
            // it can open a file, a `DETACH` cannot hide one that ran, and a pragma write
            // never reaches the connection state.
            scratch
                .authorizer(Some(deny_baseline_escapes))
                .map_err(|e| StoreError::Backend(e.to_string()))?;
            scratch
                .execute_batch(&text)
                .map_err(|e| StoreError::Baseline(format!("baseline text does not apply: {e}")))?;
            scratch
                .authorizer(
                    None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>,
                )
                .map_err(|e| StoreError::Backend(e.to_string()))?;
            let objects = schema_inventory(&scratch)?;
            // The consumer text runs after the store's own DDL, so it could drop and
            // recreate `fence` or `format_marker` with different constraints.
            let store_only =
                Connection::open_in_memory().map_err(|e| StoreError::Backend(e.to_string()))?;
            store_only
                .execute_batch(STORE_BASELINE)
                .map_err(|e| StoreError::Backend(e.to_string()))?;
            for expected in schema_inventory(&store_only)? {
                if !objects.contains(&expected) {
                    return Err(StoreError::Baseline(format!(
                        "baseline redefines infrastructure object `{}`",
                        expected.name
                    )));
                }
            }
            // A temporary object exists only on the connection that created it, so a
            // baseline that creates one presents a different schema after every reopen.
            let temporary: i64 = scratch
                .query_row("SELECT COUNT(*) FROM temp.sqlite_schema", [], |row| {
                    row.get(0)
                })
                .map_err(|e| StoreError::Backend(e.to_string()))?;
            if temporary != 0 {
                return Err(StoreError::Baseline(format!(
                    "baseline creates {temporary} temporary object(s); every baseline object must be persistent"
                )));
            }
            // A trigger or index on `fence` or `format_marker` could rewrite the epoch
            // or the marker underneath the checks that read them.
            if let Some(hooked) = objects
                .iter()
                .find(|o| o.kind != "table" && is_infrastructure_table(&o.table))
            {
                return Err(StoreError::Baseline(format!(
                    "baseline attaches {} `{}` to infrastructure table `{}`",
                    hooked.kind, hooked.name, hooked.table
                )));
            }
            Ok(Self {
                text,
                digest,
                objects,
            })
        }

        /// The identity `consumer` would present with every check of
        /// [`Self::for_baseline`] skipped: the scratch authorizer, the infrastructure
        /// redefinition comparison, the temporary-object count, and the infrastructure hook
        /// scan. A test uses it to hand [`open_claimed`] a baseline whose escapes only the
        /// store connection's gate refuses.
        #[cfg(test)]
        pub(crate) fn unchecked_for_test(consumer: &str) -> Result<Self, StoreError> {
            let text = format!("{STORE_BASELINE}\n{consumer}");
            let digest = format!("{:x}", Sha256::digest(text.as_bytes()));
            let scratch =
                Connection::open_in_memory().map_err(|e| StoreError::Backend(e.to_string()))?;
            scratch
                .execute_batch(&text)
                .map_err(|e| StoreError::Backend(e.to_string()))?;
            Ok(Self {
                text,
                digest,
                objects: schema_inventory(&scratch)?,
            })
        }

        /// Classifies an existing file on a read-only connection and returns the fence
        /// epoch it stores, or zero for a missing or pristine file, together with the
        /// identity of the inspected file so the read-write open can be checked against it.
        /// A read-only connection neither recovers nor checkpoints a WAL, so a refused file
        /// keeps its database, `-wal`, and `-shm` bytes; the epoch is read only after the
        /// file is known to be this store's, so a foreign `fence` row cannot raise the lease
        /// floor.
        fn inspect_existing(&self, path: &Path) -> Result<(u64, Option<FileIdentity>), StoreError> {
            if !path.try_exists().map_err(StoreError::Io)? {
                return Ok((0, None));
            }
            let identity = FileIdentity::of_path(path).map_err(StoreError::Io)?;
            // Any ordinary SQLite connection to a WAL-mode file creates the `-wal` and
            // `-shm` sidecars if they are missing, so the inspection never opens the file
            // itself in that mode. Without a `-wal` or a `-journal` the main file is the
            // whole database and an `immutable` open reads it while creating nothing. With
            // either sidecar present the main file may lag, be torn mid-checkpoint, or hold
            // pages an interrupted rollback-journal transaction spilled, so the database
            // and its sidecars are copied into a private directory and the copy is opened
            // read-write so SQLite replays the WAL or rolls the hot journal back there; the
            // originals are never opened. The copy costs one pass over the file and runs
            // only when a writer did not finish cleanly.
            let has_sidecar = ["-wal", "-journal"]
                .iter()
                .map(|suffix| PathBuf::from(format!("{}{suffix}", path.display())))
                .map(|sidecar| sidecar.try_exists().map_err(StoreError::Io))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .any(|exists| exists);
            let floor = if has_sidecar {
                let scratch = InspectionCopy::of(path)?;
                let conn = Connection::open_with_flags(
                    &scratch.database,
                    OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
                )
                .map_err(|e| StoreError::Backend(e.to_string()))?;
                self.floor_of(&conn)?
            } else {
                let conn = Connection::open_with_flags(
                    immutable_uri(path),
                    OpenFlags::SQLITE_OPEN_READ_ONLY
                        | OpenFlags::SQLITE_OPEN_NOFOLLOW
                        | OpenFlags::SQLITE_OPEN_URI,
                )
                .map_err(|e| StoreError::Backend(e.to_string()))?;
                self.floor_of(&conn)?
            };
            Ok((floor, Some(identity)))
        }

        /// The fence floor an inspected connection yields: zero for a pristine file, the
        /// stored epoch for this store's file, and a refusal for anything else.
        fn floor_of(&self, conn: &Connection) -> Result<u64, StoreError> {
            match self.classify(conn)? {
                FileState::Pristine => Ok(0),
                FileState::Baseline => read_fence_epoch_in(conn)?.ok_or(StoreError::FenceMissing),
            }
        }

        /// Reads only: a file that is refused keeps every byte it had.
        fn classify(&self, conn: &Connection) -> Result<FileState, StoreError> {
            let application_id: u32 = pragma_u32(conn, "application_id")?;
            let user_version: u32 = pragma_u32(conn, "user_version")?;
            let objects = schema_inventory(conn)?;
            if application_id == 0 && user_version == 0 && objects.is_empty() {
                return Ok(FileState::Pristine);
            }
            if application_id != APPLICATION_ID {
                return Err(StoreError::Baseline(format!(
                    "application_id is {application_id:#x}, expected {APPLICATION_ID:#x}"
                )));
            }
            if user_version != USER_VERSION {
                return Err(StoreError::Baseline(format!(
                    "user_version is {user_version}, expected {USER_VERSION}"
                )));
            }
            if objects != self.objects {
                let names = |list: &[SchemaObject]| {
                    list.iter()
                        .map(|o| o.name.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                return Err(StoreError::Baseline(format!(
                    "schema objects [{}] differ from the baseline objects [{}]",
                    names(&objects),
                    names(&self.objects)
                )));
            }
            let stored: Option<String> = conn
                .query_row(
                    "SELECT baseline_sha256 FROM format_marker WHERE id = 0",
                    [],
                    |row| row.get(0),
                )
                .or_else(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(other),
                })
                .map_err(|e| StoreError::Backend(e.to_string()))?;
            match stored {
                Some(found) if found == self.digest => Ok(FileState::Baseline),
                Some(found) => Err(StoreError::Baseline(format!(
                    "format marker {found} does not match the baseline digest {}",
                    self.digest
                ))),
                None => Err(StoreError::Baseline("format marker row is missing".into())),
            }
        }

        /// Applies the baseline to a pristine file inside the caller's transaction. The DDL
        /// runs under [`ConnectionMode::Baseline`]; the marker and version writes that
        /// follow are the store's own and run under the maintenance mode.
        fn apply(
            &self,
            tx: &rusqlite::Transaction<'_>,
            gate: &AuthorityGate,
        ) -> Result<(), StoreError> {
            let applied = {
                let _baseline = gate.enter(ConnectionMode::Baseline);
                tx.execute_batch(&self.text)
            };
            applied.map_err(|e| StoreError::Backend(e.to_string()))?;
            tx.pragma_update(None, "application_id", APPLICATION_ID)
                .map_err(|e| StoreError::Backend(e.to_string()))?;
            tx.pragma_update(None, "user_version", USER_VERSION)
                .map_err(|e| StoreError::Backend(e.to_string()))?;
            let rows = tx
                .execute(
                    "INSERT INTO format_marker (id, baseline_sha256) VALUES (0, ?1)",
                    rusqlite::params![self.digest],
                )
                .map_err(|e| StoreError::Backend(e.to_string()))?;
            let stored: Option<String> = tx
                .query_row(
                    "SELECT baseline_sha256 FROM format_marker WHERE id = 0",
                    [],
                    |row| row.get(0),
                )
                .or_else(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(other),
                })
                .map_err(|e| StoreError::Backend(e.to_string()))?;
            if rows != 1 || stored.as_deref() != Some(self.digest.as_str()) {
                return Err(StoreError::Backend(format!(
                    "format marker write affected {rows} rows and reads back as {stored:?}; expected {}",
                    self.digest
                )));
            }
            Ok(())
        }
    }

    fn pragma_u32(conn: &Connection, name: &str) -> Result<u32, StoreError> {
        let value: i64 = conn
            .query_row(&format!("PRAGMA {name}"), [], |row| row.get(0))
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        // SQLite stores both pragmas as 32-bit words; a negative read is the signed view
        // of a value above `i32::MAX`.
        Ok(value as u32)
    }

    /// Binds fence comparison and claim to the caller's protected transaction.
    ///
    /// An epoch equal to the stored epoch permits repeated writes.
    /// Reads the fence outside any transaction and refuses a holder the database has
    /// superseded. This is a filter, not a claim: a concurrent takeover after this read is
    /// caught by `claim_fence` inside the transaction.
    fn precheck_fence(conn: &Connection, holder_epoch: u64) -> Result<(), StoreError> {
        precheck_fence_via(conn, holder_epoch, backend_error)
    }

    fn precheck_fence_via(
        conn: &Connection,
        holder_epoch: u64,
        map: fn(rusqlite::Error) -> StoreError,
    ) -> Result<(), StoreError> {
        let db_epoch = read_fence_epoch_via(conn, map)?.ok_or(StoreError::FenceMissing)?;
        if db_epoch > holder_epoch {
            return Err(StoreError::Fenced {
                holder_epoch,
                db_epoch,
            });
        }
        Ok(())
    }

    pub(crate) fn claim_fence(
        tx: &rusqlite::Transaction<'_>,
        holder_epoch: u64,
    ) -> Result<(), StoreError> {
        let holder_epoch_sql = fence_epoch_sql_value(holder_epoch)?;
        // Fenced writes run on an initialized store, where the row is the authority.
        let db_epoch = read_fence_epoch_in(tx)?.ok_or(StoreError::FenceMissing)?;

        if db_epoch > holder_epoch {
            return Err(StoreError::Fenced {
                holder_epoch,
                db_epoch,
            });
        }
        if holder_epoch > db_epoch {
            write_fence(tx, holder_epoch_sql)?;
        }
        Ok(())
    }

    /// A stale externally derived floor can otherwise reissue the stored epoch.
    pub(crate) fn claim_fence_strict(
        tx: &rusqlite::Transaction<'_>,
        holder_epoch: u64,
        state: FileState,
    ) -> Result<(), StoreError> {
        let holder_epoch_sql = fence_epoch_sql_value(holder_epoch)?;
        // A pristine file has no fence row until this claim writes it. On an initialized
        // file the row is the writer authority; a file that lost it is not adopted at epoch
        // zero, since a reissued epoch could readmit a retained stale writer.
        let db_epoch = match (read_fence_epoch_in(tx)?, state) {
            (Some(epoch), _) => epoch,
            (None, FileState::Pristine) => 0,
            (None, FileState::Baseline) => return Err(StoreError::FenceMissing),
        };

        if holder_epoch <= db_epoch {
            return Err(StoreError::Fenced {
                holder_epoch,
                db_epoch,
            });
        }
        write_fence(tx, holder_epoch_sql)
    }

    /// `i64::try_from` rejects unrepresentable epochs before any database access.
    fn fence_epoch_sql_value(holder_epoch: u64) -> Result<i64, StoreError> {
        i64::try_from(holder_epoch).map_err(|_| {
            StoreError::Backend(format!(
                "lease epoch {holder_epoch} exceeds SQLite INTEGER maximum"
            ))
        })
    }

    fn write_fence(
        tx: &rusqlite::Transaction<'_>,
        holder_epoch_sql: i64,
    ) -> Result<(), StoreError> {
        let rows = tx
            .execute(FENCE_CLAIM_SQL, rusqlite::params![holder_epoch_sql])
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        // A trigger running `RAISE(IGNORE)` reports success with zero changed
        // rows; proceeding without the persisted epoch would break fencing.
        if rows != 1 {
            return Err(StoreError::Backend(format!(
                "fence epoch write affected {rows} rows; expected 1"
            )));
        }
        // An AFTER trigger can undo the row while leaving the change count at one.
        let stored: i64 = tx
            .query_row(FENCE_EPOCH_SQL, [], |row| row.get(0))
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        if stored != holder_epoch_sql {
            return Err(StoreError::Backend(format!(
                "fence epoch reads back as {stored} after writing {holder_epoch_sql}"
            )));
        }
        Ok(())
    }

    /// Rejects negative SQLite integers instead of wrapping them into writer epochs.
    fn decode_fence_epoch(epoch: i64) -> Result<u64, StoreError> {
        u64::try_from(epoch).map_err(|_| StoreError::FenceCorrupt { db_epoch: epoch })
    }

    #[cfg(test)]
    mod gate_tests {
        use super::*;

        fn gated_memory_store() -> ClaimedConnection {
            let conn = Connection::open_in_memory().expect("in-memory connection");
            conn.execute_batch(STORE_BASELINE).expect("store baseline");
            conn.execute_batch("CREATE TABLE kv (k TEXT PRIMARY KEY, v TEXT NOT NULL);")
                .expect("consumer table");
            AuthorityGate::install(conn).expect("install the gate")
        }

        fn guarded(conn: &Connection) -> ConnectionMode {
            ConnectionMode::Guarded(Arc::new(SchemaSnapshot::scan(conn).expect("snapshot")))
        }

        /// The authorizer runs at prepare time, so a statement cached under the unrestricted
        /// mode would run inside a guarded callback with the unrestricted verdict. Flushing
        /// the cache at the end of the unrestricted work forces a re-prepare, which the
        /// guarded policy refuses.
        #[test]
        fn an_unrestricted_cached_statement_does_not_reach_a_guarded_callback() {
            let ClaimedConnection { conn, gate } = gated_memory_store();
            conn.prepare_cached(FENCE_CLAIM_SQL)
                .expect("prepare under the unrestricted mode")
                .execute([7])
                .expect("the store's own claim runs unrestricted");
            conn.flush_prepared_statement_cache();
            let hold = gate.enter(guarded(&conn));
            let refused = conn.prepare_cached(FENCE_CLAIM_SQL).map(|_| ());
            assert!(
                matches!(&refused, Err(e) if e.to_string().contains("not authorized")),
                "the guarded mode re-authorizes the flushed statement, got {refused:?}"
            );
            drop(hold);
            let epoch: i64 = conn
                .query_row(FENCE_EPOCH_SQL, [], |row| row.get(0))
                .expect("read the fence");
            assert_eq!(epoch, 7, "the refused statement changed nothing");
        }

        /// Without the flush the cached handle is handed back without re-authorization;
        /// this pins the hazard the flush exists for.
        #[test]
        fn an_unflushed_unrestricted_statement_is_reused_without_re_authorization() {
            let ClaimedConnection { conn, gate } = gated_memory_store();
            conn.prepare_cached(FENCE_CLAIM_SQL)
                .expect("prepare under the unrestricted mode")
                .execute([7])
                .expect("unrestricted claim");
            let hold = gate.enter(guarded(&conn));
            let reused = conn
                .prepare_cached(FENCE_CLAIM_SQL)
                .and_then(|mut statement| statement.execute([8]));
            drop(hold);
            assert!(
                reused.is_ok(),
                "a cached statement runs without re-authorization, got {reused:?}"
            );
        }

        /// Every denial `deny_baseline_escapes` makes is reachable through the gate, and a
        /// trigger on `fence` is not among them: the identity check refuses it instead.
        #[test]
        fn the_baseline_mode_denies_each_escape_and_allows_plain_ddl() {
            let ClaimedConnection { conn, gate } = gated_memory_store();
            let hold = gate.enter(ConnectionMode::Baseline);
            for escape in [
                "PRAGMA writable_schema = ON",
                "PRAGMA wal_checkpoint",
                "ATTACH DATABASE ':memory:' AS other",
                "DETACH DATABASE other",
                "BEGIN",
                "SAVEPOINT s",
                "INSERT INTO fence (id, epoch) VALUES (0, 1)",
                "UPDATE fence SET epoch = 2",
                "DELETE FROM fence",
                "INSERT INTO format_marker (id, baseline_sha256) VALUES (0, '')",
                "DELETE FROM format_marker",
            ] {
                let denied = conn.execute_batch(escape);
                assert!(
                    matches!(&denied, Err(e) if e.to_string().contains("not authorized")),
                    "`{escape}` must be denied under the baseline mode, got {denied:?}"
                );
            }
            conn.execute_batch(
                "CREATE TABLE plain (x); CREATE INDEX plain_x ON plain (x); \
                 CREATE TRIGGER fence_hook AFTER INSERT ON fence BEGIN SELECT 1; END;",
            )
            .expect("plain DDL and an infrastructure trigger pass the baseline mode");
            drop(hold);
        }

        /// The snapshot is reused while both versions stand, replaced once the schema
        /// version moves, and discarded by the maintenance exit whatever the key.
        #[test]
        fn the_schema_snapshot_is_keyed_on_the_schema_and_data_versions() {
            let ClaimedConnection { conn, gate } = gated_memory_store();
            let first = gate.schema_snapshot(&conn).expect("scan");
            let again = gate.schema_snapshot(&conn).expect("reuse");
            assert!(
                Arc::ptr_eq(&first, &again),
                "an unchanged key reuses the snapshot"
            );
            assert!(first.main_names.contains("kv"));

            conn.execute_batch("ALTER TABLE kv RENAME TO renamed;")
                .expect("rename bumps the schema version");
            let rescanned = gate.schema_snapshot(&conn).expect("rescan");
            assert!(!Arc::ptr_eq(&first, &rescanned));
            assert_ne!(rescanned.key.schema_version, first.key.schema_version);
            assert!(rescanned.main_names.contains("renamed"));
            assert!(!rescanned.main_names.contains("kv"));

            gate.forget_after_maintenance();
            let after_maintenance = gate.schema_snapshot(&conn).expect("scan");
            assert!(
                !Arc::ptr_eq(&rescanned, &after_maintenance),
                "maintenance discards the snapshot even at an unchanged key"
            );
        }

        /// Defensive mode neutralizes the two statements that would move schema state under
        /// the key without DDL, on this connection.
        #[test]
        fn the_claimed_connection_refuses_schema_rewrites_without_ddl() {
            let ClaimedConnection { conn, .. } = gated_memory_store();
            let version = schema_version(&conn).expect("version");
            conn.execute_batch(&format!("PRAGMA schema_version = {}", version + 7))
                .expect("defensive mode turns the write into a no-op rather than an error");
            assert_eq!(
                schema_version(&conn).expect("version"),
                version,
                "the schema version write did not land"
            );
            conn.execute_batch("PRAGMA writable_schema = ON")
                .expect("the pragma statement itself is accepted");
            let direct = conn.execute(
                "UPDATE sqlite_schema SET name = 'fence2' WHERE name = 'kv'",
                [],
            );
            assert!(
                direct.is_err(),
                "writable_schema is a no-op under defensive mode"
            );
        }

        /// A rename another connection commits moves both halves of the key; a commit that
        /// writes the old schema version back still moves the data version, so the
        /// snapshot is rescanned either way.
        #[test]
        fn a_foreign_commit_that_restores_the_schema_version_still_moves_the_key() {
            let dir = std::env::temp_dir().join(format!("storage-key-{}", unique_nanos()));
            std::fs::create_dir_all(&dir).expect("dir");
            let path = dir.join("store.db");
            let conn = Connection::open(&path).expect("open");
            conn.execute_batch(STORE_BASELINE).expect("baseline");
            conn.execute_batch("CREATE TABLE kv (k TEXT PRIMARY KEY, v TEXT NOT NULL);")
                .expect("consumer table");
            conn.execute_batch("PRAGMA journal_mode = WAL")
                .expect("wal");
            let ClaimedConnection { conn, gate } = AuthorityGate::install(conn).expect("gate");
            let first = gate.schema_snapshot(&conn).expect("scan");

            let raw = Connection::open(&path).expect("second connection");
            raw.execute_batch(&format!(
                "ALTER TABLE kv RENAME TO kv2; PRAGMA schema_version = {};",
                first.key.schema_version
            ))
            .expect("rename, then write the old schema version back");
            drop(raw);

            let rescanned = gate.schema_snapshot(&conn).expect("scan");
            assert_eq!(rescanned.key.schema_version, first.key.schema_version);
            assert_ne!(rescanned.key.data_version, first.key.data_version);
            assert!(
                rescanned.main_names.contains("kv2"),
                "the forged version did not keep the stale names"
            );
            drop(conn);
            let _ = std::fs::remove_dir_all(&dir);
        }

        /// A snapshot larger than the declared bound serves its callback and is not kept.
        /// The bound is compared against an independent lower estimate of the heap the
        /// collections own.
        #[test]
        fn a_snapshot_above_the_retained_bound_is_not_kept() {
            let ClaimedConnection { conn, gate } = gated_memory_store();
            let wide_name = "w".repeat(200);
            let tables = SCHEMA_SNAPSHOT_RETAINED_BYTES_BOUND / 200 + 2;
            let ddl: String = (0..tables)
                .map(|i| format!("CREATE TABLE {wide_name}{i} (x);"))
                .collect();
            conn.execute_batch(&ddl).expect("wide schema");
            let first = gate.schema_snapshot(&conn).expect("scan");
            let floor = tables * (200 + std::mem::size_of::<String>() + 1);
            assert!(
                first.retained_bytes() >= floor,
                "{} < {floor}",
                first.retained_bytes()
            );
            assert!(first.retained_bytes() > SCHEMA_SNAPSHOT_RETAINED_BYTES_BOUND);
            let again = gate.schema_snapshot(&conn).expect("scan again");
            assert!(
                !Arc::ptr_eq(&first, &again),
                "an oversized snapshot is not retained"
            );
        }

        /// The release comparison is satisfied by an unchanged schema version and compares
        /// the infrastructure objects otherwise: a moved version with the same objects
        /// passes, a moved version with a new infrastructure-named object is refused.
        #[test]
        fn the_release_comparison_rescans_only_when_the_schema_version_moved() {
            let ClaimedConnection { conn, .. } = gated_memory_store();
            let snapshot = SchemaSnapshot::scan(&conn).expect("scan");
            CallbackScope::require_infrastructure_unchanged(&conn, &snapshot)
                .expect("an unchanged version passes without a rescan");
            conn.execute_batch("CREATE TABLE unrelated (x);")
                .expect("bump the version without touching infrastructure");
            CallbackScope::require_infrastructure_unchanged(&conn, &snapshot)
                .expect("a moved version with the same infrastructure objects passes");
            conn.execute_batch("CREATE TEMP VIEW fence AS SELECT 0 AS id, 0 AS epoch;")
                .expect("a temp view under the infrastructure name");
            let refused = CallbackScope::require_infrastructure_unchanged(&conn, &snapshot);
            assert!(
                matches!(&refused, Err(StoreError::Backend(m)) if m.contains("infrastructure schema objects")),
                "a moved version with a new infrastructure object is refused, got {refused:?}"
            );
        }

        /// The durability pin runs once per connection: a lowered `synchronous` that no
        /// maintenance callback caused is not re-pinned by a fenced write, and the first
        /// fenced write after maintenance re-pins. The lowering below is reachable only
        /// through the raw connection; the per-write pin is traded for that assumption,
        /// which the guarded policy and WAL's locking uphold.
        #[test]
        fn the_durability_pin_runs_once_per_connection_until_maintenance() {
            let root = std::env::temp_dir().join(format!("storage-pin-{}", unique_nanos()));
            std::fs::create_dir_all(&root).expect("store directory");
            let path = root.join("store.db").to_string_lossy().into_owned();
            let expected = ExpectedIdentity::for_baseline(
                "CREATE TABLE kv (k TEXT PRIMARY KEY, v TEXT NOT NULL);",
            )
            .expect("identity");
            let claimed = open_claimed(&path, &expected, None, 1).expect("open");
            claimed
                .conn
                .pragma_update(None, "synchronous", "OFF")
                .expect("lower on the raw connection");
            let store = SqliteStore::over(claimed, 1, None);
            let synchronous = |store: &SqliteStore| -> i64 {
                store
                    .with_conn(|c| c.query_row("PRAGMA synchronous", [], |r| r.get(0)))
                    .expect("read synchronous")
            };
            store
                .with_conn_fenced(|tx| tx.execute("INSERT INTO kv VALUES ('a', '1')", []))
                .expect("fenced write");
            assert_eq!(
                synchronous(&store),
                0,
                "the fenced write did not re-run the pin"
            );
            store.with_conn_unfenced(|_| Ok(())).expect("maintenance");
            store
                .with_conn_fenced(|tx| tx.execute("INSERT INTO kv VALUES ('b', '2')", []))
                .expect("fenced write after maintenance");
            assert_eq!(
                synchronous(&store),
                2,
                "the first fenced write after maintenance re-pins FULL"
            );
            drop(store);
            let _ = std::fs::remove_dir_all(&root);
        }

        /// A maintenance callback that unwinds still discards the pin and the snapshot:
        /// the shadow it left is refused, and the next fenced write re-pins.
        #[test]
        fn a_panicking_maintenance_callback_still_invalidates_the_connection_caches() {
            let root = std::env::temp_dir().join(format!("storage-unwind-{}", unique_nanos()));
            std::fs::create_dir_all(&root).expect("store directory");
            let path = root.join("store.db").to_string_lossy().into_owned();
            let expected = ExpectedIdentity::for_baseline(
                "CREATE TABLE kv (k TEXT PRIMARY KEY, v TEXT NOT NULL);",
            )
            .expect("identity");
            let claimed = open_claimed(&path, &expected, None, 1).expect("open");
            let store = SqliteStore::over(claimed, 1, None);
            store
                .with_conn(|c| c.query_row("SELECT COUNT(*) FROM kv", [], |r| r.get::<_, i64>(0)))
                .expect("retain a snapshot");
            let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                store.with_conn_unfenced(|c| -> rusqlite::Result<()> {
                    c.pragma_update(None, "synchronous", "OFF")?;
                    c.execute_batch("CREATE TEMP TABLE kv (k TEXT PRIMARY KEY, v TEXT NOT NULL)")?;
                    panic!("maintenance panics after lowering durability and leaving a shadow")
                })
            }));
            assert!(panicked.is_err());
            assert!(
                !store.gate.lock().durability_pinned,
                "the unwind re-armed the pin"
            );
            assert!(
                store.gate.lock().schema.is_none(),
                "the unwind discarded the snapshot"
            );
            let refused = store.with_conn(|c| c.query_row("SELECT 1", [], |r| r.get::<_, i64>(0)));
            assert!(
                matches!(&refused, Err(StoreError::Backend(m)) if m.contains("shadows")),
                "the shadow the panicking maintenance left is refused, got {refused:?}"
            );
            store
                .with_conn_unfenced(|c| c.execute_batch("DROP TABLE temp.kv"))
                .expect("drop the shadow");
            store
                .with_conn_fenced(|tx| tx.execute("INSERT INTO kv VALUES ('a', '1')", []))
                .expect("fenced write");
            let synchronous: i64 = store
                .with_conn(|c| c.query_row("PRAGMA synchronous", [], |r| r.get(0)))
                .expect("read synchronous");
            assert_eq!(
                synchronous, 2,
                "the fenced write after the unwind re-pinned FULL"
            );
            drop(store);
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        #[should_panic(expected = "another mode was held")]
        fn entering_a_mode_while_one_is_held_is_a_programming_error() {
            let ClaimedConnection { gate, .. } = gated_memory_store();
            let _outer = gate.enter(ConnectionMode::Baseline);
            let _inner = gate.enter(ConnectionMode::Baseline);
        }
    }
}

#[cfg(all(feature = "sqlite", any(test, feature = "test-support")))]
pub use sqlite_backend::library_memory_used;
#[cfg(feature = "sqlite")]
pub use sqlite_backend::{
    APPLICATION_ID, CachedStatement, GuardedConn, INFRASTRUCTURE_TABLES, MaintenanceConn,
    SCHEMA_SNAPSHOT_RETAINED_BYTES_BOUND, STORE_BASELINE, SchemaObject, SqliteStore, USER_VERSION,
    open_sqlite, schema_inventory,
};

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use super::sqlite_backend::{
        ClaimedConnection, ExpectedIdentity, FileState, InspectionCopy,
        before_next_busy_wait_for_test, claim_fence, claim_fence_strict,
        create_database_file_owner_only, immutable_uri, open_claimed,
    };
    use super::*;
    use std::path::Path;
    use std::time::{Duration, Instant};

    #[test]
    fn a_read_scope_restores_the_query_only_value_it_found() {
        let (root, d) = tmp();
        let store = open_sqlite(&d, "").expect("open");
        store
            .with_conn_unfenced(|c| c.pragma_update(None, "query_only", true))
            .expect("maintenance sets query_only");
        store
            .with_conn(|c| c.query_row("SELECT 1", [], |r| r.get::<_, i64>(0)))
            .expect("read");
        let still_on: bool = store
            .with_conn_unfenced(|c| c.query_row("PRAGMA query_only", [], |r| r.get(0)))
            .expect("read pragma");
        assert!(
            still_on,
            "a read scope must not clear a query_only that maintenance set"
        );
        store
            .with_conn_unfenced(|c| c.pragma_update(None, "query_only", false))
            .expect("maintenance clears query_only");
        let off: bool = store
            .with_conn_unfenced(|c| c.query_row("PRAGMA query_only", [], |r| r.get(0)))
            .expect("read pragma");
        assert!(!off);
        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }

    const KV_BASELINE: &str = "CREATE TABLE kv (k TEXT PRIMARY KEY, v TEXT NOT NULL);";

    const INVENTORY_FIXTURE: &str =
        include_str!("../../../fixtures/schema/storage-inventory-v1.json");

    /// A second name for the database inode would give a second store its own lease and
    /// its own WAL, so a hard-linked database is refused before it is opened.
    #[cfg(unix)]
    #[test]
    fn a_hard_linked_database_is_refused() {
        let (root, d) = tmp();
        let StorageBackend::Sqlite { path } = &d.backend else {
            panic!("sqlite descriptor");
        };
        drop(open_sqlite(&d, "").expect("create the database"));
        let alias = root.join("alias.db");
        std::fs::hard_link(path, &alias).expect("hard link");
        let via_alias = StorageDescriptor {
            backend: StorageBackend::Sqlite {
                path: alias.to_string_lossy().into_owned(),
            },
            ..d.clone()
        };
        for descriptor in [&d, &via_alias] {
            match open_sqlite(descriptor, "").map(|_| ()) {
                Err(StoreError::Baseline(m)) => assert!(m.contains("names"), "unexpected: {m}"),
                other => panic!("a hard-linked database must be refused, got {other:?}"),
            }
        }
        std::fs::remove_file(&alias).expect("remove alias");
        drop(open_sqlite(&d, "").expect("a single name opens again"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The main schema is the only target a baseline addresses; the store's own tables keep
    /// the definitions `baseline.sql` gives them.
    #[test]
    fn a_baseline_outside_the_main_schema_contract_is_rejected_without_creating_the_file() {
        let (root, d) = tmp();
        let StorageBackend::Sqlite { path } = &d.backend else {
            panic!("sqlite descriptor");
        };
        for (ddl, needle) in [
            ("CREATE TEMP TABLE scratch (k TEXT);", "temporary object"),
            (
                "CREATE TABLE kv (k TEXT); CREATE TEMP VIEW kv_view AS SELECT k FROM kv;",
                "temporary object",
            ),
            (
                "ATTACH ':memory:' AS aux; CREATE TABLE aux.t (k TEXT);",
                "not authorized",
            ),
            (
                "ATTACH ':memory:' AS aux; CREATE TABLE aux.t (k TEXT); DETACH aux;",
                "not authorized",
            ),
            (
                "INSERT INTO fence VALUES (0, 9223372036854775807);",
                "not authorized",
            ),
            (
                "UPDATE format_marker SET baseline_sha256 = baseline_sha256;",
                "not authorized",
            ),
            ("DELETE FROM fence;", "not authorized"),
            ("PRAGMA writable_schema = ON;", "not authorized"),
            ("PRAGMA ignore_check_constraints = ON;", "not authorized"),
            ("PRAGMA foreign_keys = OFF;", "not authorized"),
            ("PRAGMA shrink_memory;", "not authorized"),
            ("BEGIN; CREATE TABLE t (k TEXT); COMMIT;", "not authorized"),
            // Dropping a table deletes its rows, which the authorizer refuses first.
            (
                "DROP TABLE fence; CREATE TABLE fence (id INTEGER PRIMARY KEY CHECK (id = 0), epoch INTEGER NOT NULL CHECK (epoch = 1));",
                "not authorized",
            ),
            (
                "DROP TABLE format_marker; CREATE TABLE format_marker (id INTEGER PRIMARY KEY, baseline_sha256 TEXT);",
                "not authorized",
            ),
            (
                "ALTER TABLE fence ADD COLUMN extra TEXT;",
                "redefines infrastructure object `fence`",
            ),
            (
                "ALTER TABLE format_marker RENAME COLUMN baseline_sha256 TO digest;",
                "redefines infrastructure object `format_marker`",
            ),
            (
                "CREATE TRIGGER marker_undo BEFORE INSERT ON format_marker BEGIN SELECT RAISE(IGNORE); END;",
                "infrastructure table",
            ),
            (
                "CREATE TRIGGER fence_undo AFTER UPDATE ON fence BEGIN UPDATE fence SET epoch = OLD.epoch WHERE id = 0; END;",
                "infrastructure table",
            ),
            (
                "CREATE INDEX fence_idx ON fence (epoch);",
                "infrastructure table",
            ),
        ] {
            match open_sqlite(&d, ddl).map(|_| ()) {
                Err(StoreError::Baseline(m)) => {
                    assert!(m.contains(needle), "{ddl}: unexpected message: {m}")
                }
                other => panic!("{ddl} must be rejected, got {other:?}"),
            }
        }
        assert!(
            !std::path::Path::new(path).exists(),
            "a rejected baseline must not create the file"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A FIFO at the database path blocks SQLite's open before any timeout applies. A FIFO at
    /// the `-journal` path blocks the inspection copy. Both are refused as non-regular files.
    #[cfg(unix)]
    #[test]
    fn a_fifo_at_the_database_or_journal_path_is_refused_before_it_is_opened() {
        for (suffix, seed_database) in [("", false), ("-journal", true)] {
            let (root, d) = tmp();
            let StorageBackend::Sqlite { path } = &d.backend else {
                panic!("sqlite descriptor");
            };
            if seed_database {
                drop(open_sqlite(&d, "").expect("first open"));
            } else {
                std::fs::create_dir_all(&root).expect("root");
            }
            let fifo = format!("{path}{suffix}");
            let c_path = std::ffi::CString::new(fifo.as_str()).expect("path");
            // SAFETY: `c_path` is a valid NUL-terminated string for the duration of the call.
            assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0, "mkfifo");
            match open_sqlite(&d, "").map(|_| ()) {
                Err(StoreError::Baseline(m)) => {
                    assert!(m.contains("not a regular file"), "unexpected message: {m}")
                }
                other => panic!("a FIFO at {fifo} must be refused without blocking, got {other:?}"),
            }
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    /// A baseline that attaches a file is refused before the attachment opens it, so the
    /// external database is never touched even when the text detaches it again.
    #[test]
    fn a_baseline_that_attaches_a_file_never_writes_to_it() {
        let (root, d) = tmp();
        std::fs::create_dir_all(&root).expect("root");
        let external = root.join("outside.db");
        let ddl = format!(
            "ATTACH '{}' AS aux; CREATE TABLE aux.t (k TEXT); DETACH aux;",
            external.display()
        );
        match open_sqlite(&d, &ddl).map(|_| ()) {
            Err(StoreError::Baseline(m)) => {
                assert!(m.contains("not authorized"), "unexpected message: {m}")
            }
            other => panic!("attaching a file must be rejected, got {other:?}"),
        }
        assert!(!external.exists(), "the attached path must not be created");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A foreign database with committed WAL frames is refused without SQLite recovering or
    /// checkpointing it: the database, `-wal`, and `-shm` bytes are untouched.
    #[test]
    fn a_refused_foreign_wal_database_is_left_unrecovered() {
        let (root, d) = tmp();
        let StorageBackend::Sqlite { path } = &d.backend else {
            panic!("sqlite descriptor");
        };
        std::fs::create_dir_all(&root).expect("root");
        // Build the WAL elsewhere, then copy the frames alongside a copy of the database
        // before the writer closes; closing the last connection would checkpoint them.
        let origin = root.join("origin.db");
        {
            let foreign = rusqlite::Connection::open(&origin).expect("foreign database");
            foreign
                .execute_batch(
                    "PRAGMA journal_mode = WAL; PRAGMA wal_autocheckpoint = 0; \
                     CREATE TABLE theirs (k TEXT); INSERT INTO theirs VALUES ('frame');",
                )
                .expect("foreign schema in the WAL");
            std::fs::copy(&origin, path).expect("copy database");
            std::fs::copy(format!("{}-wal", origin.display()), format!("{path}-wal"))
                .expect("copy wal");
        }
        let wal_before = std::fs::read(format!("{path}-wal")).expect("wal bytes");
        assert!(!wal_before.is_empty(), "the copied WAL must carry frames");
        let db_before = std::fs::read(path).expect("db bytes");
        assert!(matches!(
            open_sqlite(&d, "").map(|_| ()),
            Err(StoreError::Baseline(_))
        ));
        assert_eq!(
            std::fs::read(format!("{path}-wal")).expect("wal after"),
            wal_before
        );
        assert_eq!(std::fs::read(path).expect("db after"), db_before);
        assert!(
            !std::path::Path::new(&format!("{path}-shm")).exists(),
            "inspection must not create the foreign store's shared-memory file"
        );
        assert!(
            std::fs::read_dir(&root).expect("root").all(|e| {
                !e.expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".inspect-")
            }),
            "the inspection copy is removed"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_refused_foreign_database_keeps_its_bytes_and_gains_no_sidecars() {
        for foreign_schema in [
            "CREATE TABLE unfenced_data (id INTEGER PRIMARY KEY);",
            "PRAGMA journal_mode = WAL; CREATE TABLE theirs (k TEXT);",
        ] {
            let (root, d) = tmp();
            let StorageBackend::Sqlite { path } = &d.backend else {
                panic!("sqlite descriptor");
            };
            std::fs::create_dir_all(&root).expect("root");
            {
                let foreign = rusqlite::Connection::open(path).expect("foreign database");
                foreign
                    .execute_batch(foreign_schema)
                    .expect("foreign schema");
            }
            assert!(!std::path::Path::new(&format!("{path}-wal")).exists());
            let before = std::fs::read(path).expect("bytes before");
            let refused = open_sqlite(&d, "");
            assert!(
                matches!(refused, Err(StoreError::Baseline(_))),
                "a foreign file is refused, got {:?}",
                refused.map(|_| ())
            );
            assert_eq!(
                std::fs::read(path).expect("bytes after"),
                before,
                "the refused open changed the file's bytes"
            );
            for suffix in ["-wal", "-shm"] {
                assert!(
                    !std::path::Path::new(&format!("{path}{suffix}")).exists(),
                    "refusal must not create {suffix}"
                );
            }
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    /// A store whose last writer left its frames in the WAL reopens with the epoch those
    /// frames hold as the floor, so a lost lease sidecar cannot reissue it.
    #[test]
    fn a_store_left_with_wal_frames_reopens_above_the_wal_epoch() {
        let (root, d) = tmp();
        let StorageBackend::Sqlite { path } = &d.backend else {
            panic!("sqlite descriptor");
        };
        let store = open_sqlite(&d, "").expect("first open");
        assert_eq!(store.epoch(), 1);
        // The store is still open, so its baseline and fence live in the WAL. Copy the
        // database and WAL to a fresh path with no lease sidecar: a crashed writer's state.
        let crashed_root = root.join("crashed");
        std::fs::create_dir_all(&crashed_root).expect("crashed root");
        let crashed_path = crashed_root.join("store.db");
        std::fs::copy(path, &crashed_path).expect("copy database");
        std::fs::copy(
            format!("{path}-wal"),
            format!("{}-wal", crashed_path.display()),
        )
        .expect("copy wal");
        drop(store);
        let crashed = StorageDescriptor {
            backend: StorageBackend::Sqlite {
                path: crashed_path.to_string_lossy().into_owned(),
            },
            ..d.clone()
        };
        let reopened = open_sqlite(&crashed, "").expect("reopen from WAL state");
        assert_eq!(
            reopened.epoch(),
            2,
            "the floor comes from the fence row in the WAL, not from the empty main file"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An initialized store whose fence row is gone is refused rather than adopted at epoch
    /// zero.
    #[test]
    fn an_initialized_store_without_a_fence_row_is_refused() {
        let (root, d) = tmp();
        let StorageBackend::Sqlite { path } = &d.backend else {
            panic!("sqlite descriptor");
        };
        drop(open_sqlite(&d, "").expect("first open"));
        {
            let raw = rusqlite::Connection::open(path).expect("raw connection");
            raw.execute("DELETE FROM fence", [])
                .expect("remove the fence row");
        }
        match open_sqlite(&d, "").map(|_| ()) {
            Err(StoreError::FenceMissing) => {}
            other => panic!("a missing fence row must be refused, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A `fence` row in a foreign file is not a floor: the lease epoch stays untouched when
    /// the file is refused, so the path is usable again once the foreign file is gone.
    #[test]
    fn a_foreign_fence_row_does_not_raise_the_lease_floor() {
        let (root, d) = tmp();
        let StorageBackend::Sqlite { path } = &d.backend else {
            panic!("sqlite descriptor");
        };
        std::fs::create_dir_all(&root).expect("root");
        {
            let foreign = rusqlite::Connection::open(path).expect("foreign database");
            foreign
                .execute_batch(&format!(
                    "CREATE TABLE fence (id INTEGER PRIMARY KEY, epoch INTEGER NOT NULL); \
                     INSERT INTO fence VALUES (0, {}); CREATE TABLE theirs (k TEXT);",
                    i64::MAX
                ))
                .expect("foreign fence");
        }
        assert!(matches!(
            open_sqlite(&d, "").map(|_| ()),
            Err(StoreError::Baseline(_))
        ));
        std::fs::remove_file(path).expect("remove the foreign file");
        let store = open_sqlite(&d, "").expect("a fresh store opens once the foreign file is gone");
        assert_eq!(
            store.epoch(),
            1,
            "the lease was never raised by the foreign row"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A relative database path is refused before any directory or file is created, since
    /// the same descriptor would name a different store after a change of working directory.
    #[test]
    fn a_relative_sqlite_path_is_refused() {
        let (root, d) = tmp();
        let relative = StorageDescriptor {
            backend: StorageBackend::Sqlite {
                path: "relative/dir/store.db".into(),
            },
            ..d.clone()
        };
        match open_sqlite(&relative, "").map(|_| ()) {
            Err(StoreError::Io(e)) => {
                assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput, "{e}");
                assert!(e.to_string().contains("not absolute"), "{e}");
            }
            other => panic!("a relative path must be refused, got {other:?}"),
        }
        assert!(!std::path::Path::new("relative").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Fenced writes and schema changes from a writer whose epoch is below the database
    /// epoch fail before they reach the file. The refusal precedes the durability pin, so a
    /// journal mode that maintenance on the newer writer chose is not switched back by the
    /// stale one.
    #[test]
    fn a_superseded_writer_is_fenced_before_it_writes_or_repins_the_journal() {
        let (root, d) = tmp();
        let StorageBackend::Sqlite { path } = &d.backend else {
            panic!("sqlite descriptor");
        };
        drop(open_sqlite(&d, KV_BASELINE).expect("seed schema"));
        let stale = SqliteStore::for_test(rusqlite::Connection::open(path).unwrap(), 1);
        let newer = SqliteStore::for_test(rusqlite::Connection::open(path).unwrap(), 2);
        newer
            .with_conn_fenced(|tx| {
                tx.execute("INSERT INTO kv (k, v) VALUES ('owner', 'new')", [])
                    .map(|_| ())
            })
            .expect("the newer writer claims epoch 2");
        newer
            .with_conn_unfenced(|conn| {
                conn.query_row("PRAGMA journal_mode = DELETE", [], |row| {
                    row.get::<_, String>(0)
                })
                .map(|_| ())
            })
            .expect("maintenance switches the journal mode");
        let result = stale.with_conn_fenced(|tx| {
            tx.execute("UPDATE kv SET v = 'clobbered' WHERE k = 'owner'", [])
                .map(|_| ())
        });
        assert!(
            matches!(
                result,
                Err(StoreError::Fenced {
                    holder_epoch: 1,
                    db_epoch: 2
                })
            ),
            "the stale writer is fenced, got {result:?}"
        );
        assert!(
            matches!(
                stale.with_conn_fenced(|tx| {
                    tx.execute("CREATE TABLE stale_schema (id INTEGER PRIMARY KEY)", [])
                        .map(|_| ())
                }),
                Err(StoreError::Fenced { .. })
            ),
            "a superseded writer cannot change the schema"
        );
        let (v, stale_tables): (String, i64) = newer
            .with_conn(|c| {
                c.query_row(
                    "SELECT (SELECT v FROM kv WHERE k = 'owner'), \
                     (SELECT COUNT(*) FROM sqlite_schema \
                      WHERE type = 'table' AND name = 'stale_schema')",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
            })
            .expect("read");
        assert_eq!(v, "new", "stale writer was fenced out, no clobber");
        assert_eq!(
            stale_tables, 0,
            "the fenced-out schema change left no table"
        );
        let mode: String = stale
            .with_conn(|conn| conn.query_row("PRAGMA journal_mode", [], |row| row.get(0)))
            .expect("read journal mode");
        assert!(
            mode.eq_ignore_ascii_case("delete"),
            "the fenced writer must not have re-pinned WAL, journal_mode is {mode}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An opener whose lease epoch is already at or below the fence row is refused on
    /// its read-only look at the row, before `journal_mode = WAL` is pinned, so a store
    /// that unfenced maintenance left in rollback-journal mode keeps that mode when a
    /// superseded opener is turned away.
    #[test]
    fn a_stale_epoch_is_refused_before_the_journal_mode_is_pinned() {
        let (root, d) = tmp();
        let StorageBackend::Sqlite { path } = &d.backend else {
            panic!("sqlite descriptor");
        };
        drop(open_sqlite(&d, KV_BASELINE).expect("seed schema"));
        let expected = ExpectedIdentity::for_baseline(KV_BASELINE).expect("baseline");
        let raw = rusqlite::Connection::open(path).expect("maintenance connection");
        let mode: String = raw
            .query_row("PRAGMA journal_mode = DELETE", [], |row| row.get(0))
            .expect("switch to rollback journal");
        assert!(
            mode.eq_ignore_ascii_case("delete"),
            "journal_mode is {mode}"
        );
        raw.execute("UPDATE fence SET epoch = 10 WHERE id = 0", [])
            .expect("advance the fence");
        drop(raw);

        let result = open_claimed(path, &expected, None, 3);
        assert!(
            matches!(
                result,
                Err(StoreError::Fenced {
                    holder_epoch: 3,
                    db_epoch: 10
                })
            ),
            "a stale epoch is fenced, got {:?}",
            result.map(|_| ())
        );
        let raw = rusqlite::Connection::open(path).expect("inspect journal mode");
        let mode: String = raw
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("read journal mode");
        assert!(
            mode.eq_ignore_ascii_case("delete"),
            "the refused opener must not have pinned WAL, journal_mode is {mode}"
        );
        drop(raw);

        let ClaimedConnection { conn, .. } =
            open_claimed(path, &expected, None, 11).expect("a newer epoch opens");
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("read journal mode");
        assert!(mode.eq_ignore_ascii_case("wal"), "journal_mode is {mode}");
        let epoch: i64 = conn
            .query_row("SELECT epoch FROM fence WHERE id = 0", [], |row| row.get(0))
            .expect("read fence");
        assert_eq!(epoch, 11);
        drop(conn);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A descriptor field holding U+001F, the lease-key separator, is refused as an
    /// `InvalidInput` error instead of reaching `LeaseKey::identity`, which panics on it.
    #[test]
    fn a_descriptor_field_holding_the_lease_separator_is_refused_without_panicking() {
        let (root, base) = tmp();
        let StorageBackend::Sqlite { path } = &base.backend else {
            panic!("sqlite descriptor");
        };
        let parent = std::path::Path::new(path)
            .parent()
            .expect("parent")
            .to_path_buf();
        let mut cases = vec![
            StorageDescriptor {
                module_id: "mod\u{1f}ule".to_string(),
                ..base.clone()
            },
            StorageDescriptor {
                storage_namespace: "ns\u{1f}".to_string(),
                ..base.clone()
            },
        ];
        cases.push(StorageDescriptor {
            backend: StorageBackend::Sqlite {
                path: parent
                    .join("sep\u{1f}arated.db")
                    .to_string_lossy()
                    .into_owned(),
            },
            ..base.clone()
        });
        for descriptor in cases {
            match open_sqlite(&descriptor, KV_BASELINE) {
                Err(StoreError::Io(error)) => {
                    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput, "{error}");
                    assert!(error.to_string().contains("U+001F"), "{error}");
                }
                Err(other) => panic!("expected an InvalidInput error, got {other}"),
                Ok(_) => panic!("a separator in the descriptor must be refused"),
            }
        }
        assert!(
            std::fs::read_dir(&parent).map(|d| d.count()).unwrap_or(0) == 0,
            "no lease or database is created for a refused descriptor"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A store another process holds is reported as `Lease(Held)` before its file is read:
    /// the inspection takes a shared lease first, which the live owner's exclusive lease
    /// refuses, so a file the owner is checkpointing is never copied half-written and
    /// misreported as foreign or corrupt.
    #[test]
    fn a_live_holder_is_reported_before_the_file_is_inspected() {
        let (root, d) = tmp();
        let StorageBackend::Sqlite { path } = &d.backend else {
            panic!("sqlite descriptor");
        };
        std::fs::create_dir_all(&root).expect("root");
        // Bytes that are not a SQLite database stand in for a half-checkpointed copy.
        std::fs::write(path, b"not a database while the owner writes it").expect("junk");
        let leases = lease::FileLeaseStore::new(&root).expect("lease store");
        let key = lease_key(&d, "store.db").expect("key");
        let owner = leases
            .acquire(&key)
            .expect("the live owner's exclusive lease");
        match open_sqlite(&d, KV_BASELINE).map(|_| ()) {
            Err(StoreError::Lease(lease::LeaseError::Held { .. })) => {}
            other => panic!("a held store must be reported as held, got {other:?}"),
        }
        drop(owner);
        match open_sqlite(&d, KV_BASELINE).map(|_| ()) {
            Err(StoreError::Baseline(_) | StoreError::Backend(_)) => {}
            other => panic!("without a holder the file itself is judged, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A fence at `i64::MAX` is refused before the lease is touched, so repairing the row is
    /// enough to reopen; the lease sidecar never holds an epoch SQLite cannot store.
    #[test]
    fn a_fence_at_the_integer_maximum_is_refused_before_the_lease_advances() {
        let (root, d) = tmp();
        let StorageBackend::Sqlite { path } = &d.backend else {
            panic!("sqlite descriptor");
        };
        drop(open_sqlite(&d, "").expect("first open"));
        {
            let raw = rusqlite::Connection::open(path).expect("raw connection");
            raw.execute("UPDATE fence SET epoch = ?1 WHERE id = 0", [i64::MAX])
                .expect("exhaust the fence");
        }
        match open_sqlite(&d, "").map(|_| ()) {
            Err(StoreError::FenceExhausted { db_epoch }) => {
                assert_eq!(db_epoch, i64::MAX as u64)
            }
            other => panic!("an exhausted fence must be refused, got {other:?}"),
        }
        {
            let raw = rusqlite::Connection::open(path).expect("raw connection");
            raw.execute("UPDATE fence SET epoch = 1 WHERE id = 0", [])
                .expect("repair the fence");
        }
        assert_eq!(
            open_sqlite(&d, "").expect("reopen after repair").epoch(),
            2,
            "the lease sidecar still holds epoch 1, so the repaired store issues 2"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A store whose maintenance switched it to a rollback journal and whose writer died
    /// mid-transaction leaves spilled pages in the main file and their originals in a hot
    /// `-journal`. Here the spilled page holds an uncommitted fence at `i64::MAX`, which a
    /// bare read of the main file would take as the floor and refuse as exhausted; the
    /// inspection rolls the copy back first, so the store reopens above the committed epoch.
    #[test]
    fn a_store_with_a_hot_rollback_journal_is_classified_after_rollback() {
        let (root, d) = tmp();
        let StorageBackend::Sqlite { path } = &d.backend else {
            panic!("sqlite descriptor");
        };
        {
            let store = open_sqlite(&d, KV_BASELINE).expect("first open");
            store
                .with_conn_unfenced(|conn| {
                    conn.query_row("PRAGMA journal_mode = DELETE", [], |row| {
                        row.get::<_, String>(0)
                    })
                    .map(|_| ())
                })
                .expect("switch to a rollback journal");
        }
        // A tiny page cache forces the dirtied fence page out to the main file while the
        // transaction is still open; the journal holds the committed page.
        let writer = rusqlite::Connection::open(path).expect("writer");
        writer
            .execute_batch(&format!(
                "PRAGMA cache_size = 1; BEGIN; UPDATE fence SET epoch = {} WHERE id = 0; \
                 WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 4000) \
                 INSERT INTO kv SELECT hex(randomblob(16)), hex(randomblob(128)) FROM n;",
                i64::MAX
            ))
            .expect("open transaction with spilled pages");
        let journal = format!("{path}-journal");
        assert!(
            std::fs::metadata(&journal).map(|m| m.len()).unwrap_or(0) > 0,
            "the transaction must leave a journal on disk"
        );
        // Copy the crashed state aside while the writer still holds it open, then let the
        // copy stand in for a store whose process died before commit or rollback.
        let crashed_root = root.join("crashed");
        std::fs::create_dir_all(&crashed_root).expect("crashed root");
        let crashed_path = crashed_root.join("store.db");
        std::fs::copy(path, &crashed_path).expect("copy database");
        std::fs::copy(&journal, format!("{}-journal", crashed_path.display()))
            .expect("copy journal");
        drop(writer);
        let crashed = StorageDescriptor {
            backend: StorageBackend::Sqlite {
                path: crashed_path.to_string_lossy().into_owned(),
            },
            ..d.clone()
        };
        let reopened = open_sqlite(&crashed, KV_BASELINE).expect("reopen after rollback");
        assert_eq!(
            reopened.epoch(),
            2,
            "the floor is the committed epoch 1 from the rolled-back copy, not the spilled value"
        );
        let rows: i64 = reopened
            .with_conn(|conn| conn.query_row("SELECT count(*) FROM kv", [], |row| row.get(0)))
            .expect("row count");
        assert_eq!(rows, 0, "the uncommitted rows were rolled back");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The inspection copy is owner-only whatever the umask, since it holds the store's bytes
    /// for the length of the inspection.
    #[cfg(unix)]
    #[test]
    fn the_inspection_copy_is_owner_only_under_a_permissive_umask() {
        use std::os::unix::fs::PermissionsExt;
        let (root, d) = tmp();
        let StorageBackend::Sqlite { path } = &d.backend else {
            panic!("sqlite descriptor");
        };
        let store = open_sqlite(&d, "").expect("open");
        // The store is still open, so a WAL sidecar exists to be copied along.
        // SAFETY: `umask` only reads and sets the process file-mode creation mask.
        let previous = unsafe { libc::umask(0o022) };
        let copy = InspectionCopy::of(Path::new(path));
        // SAFETY: restores the mask read above.
        unsafe { libc::umask(previous) };
        let copy = copy.expect("inspection copy");
        let dir_mode = std::fs::metadata(&copy.directory)
            .expect("copy directory")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700, "copy directory mode {dir_mode:o}");
        for suffix in ["", "-wal"] {
            let file = format!("{}{suffix}", copy.database.display());
            let mode = std::fs::metadata(&file).expect(&file).permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{file} has mode {mode:o}");
        }
        drop(copy);
        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A refused foreign file keeps its permission bits as well as its bytes.
    #[cfg(unix)]
    #[test]
    fn a_refused_foreign_file_keeps_its_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let (root, d) = tmp();
        let StorageBackend::Sqlite { path } = &d.backend else {
            panic!("sqlite descriptor");
        };
        std::fs::create_dir_all(&root).expect("root");
        {
            let foreign = rusqlite::Connection::open(path).expect("foreign database");
            foreign
                .execute_batch("CREATE TABLE theirs (k TEXT);")
                .expect("foreign schema");
        }
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o664)).expect("chmod");
        let before = std::fs::read(path).expect("bytes before");
        assert!(matches!(
            open_sqlite(&d, "").map(|_| ()),
            Err(StoreError::Baseline(_))
        ));
        let mode = std::fs::metadata(path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o664, "a refused file keeps its mode");
        assert_eq!(std::fs::read(path).expect("bytes after"), before);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A store whose path holds non-ASCII bytes reopens cleanly: the inspection URI carries
    /// each path byte percent-encoded rather than as a re-encoded scalar.
    #[test]
    fn a_store_under_a_non_ascii_path_reopens() {
        let (root, d) = tmp();
        let StorageBackend::Sqlite { path } = &d.backend else {
            panic!("sqlite descriptor");
        };
        let odd_dir = root.join("Ünïcødé dir #1 100%");
        let odd_path = odd_dir.join("store.db");
        let odd = StorageDescriptor {
            backend: StorageBackend::Sqlite {
                path: odd_path.to_string_lossy().into_owned(),
            },
            ..d.clone()
        };
        let _ = path;
        assert_eq!(open_sqlite(&odd, "").expect("first open").epoch(), 1);
        assert!(
            !std::path::Path::new(&format!("{}-wal", odd_path.display())).exists(),
            "a clean close leaves no WAL, so the reopen inspects through the immutable URI"
        );
        assert_eq!(open_sqlite(&odd, "").expect("reopen").epoch(), 2);
        assert_eq!(
            immutable_uri(Path::new("/tmp/ü %?#.sqlite3")),
            "file:/tmp/%C3%BC%20%25%3F%23.sqlite3?immutable=1"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A temp object under a main-schema name, or a temp trigger on a main table, would
    /// capture the writes of every later callback on the connection; both are denied while
    /// an unrelated scratch table stays allowed.
    #[test]
    fn a_callback_cannot_shadow_a_baseline_table_with_a_temp_object() {
        let (root, d) = tmp();
        let store = open_sqlite(&d, KV_BASELINE).expect("open");
        for ddl in [
            "CREATE TEMP TABLE kv (k TEXT PRIMARY KEY, v TEXT)",
            "CREATE TEMP TABLE KV (k TEXT)",
            "CREATE TEMP VIEW kv AS SELECT 'x' AS k, 'y' AS v",
            "CREATE TEMP TRIGGER swallow BEFORE INSERT ON kv BEGIN SELECT RAISE(IGNORE); END",
        ] {
            let result = store.with_conn_fenced(|tx| tx.execute(ddl, []).map(|_| ()));
            match result {
                Err(StoreError::Backend(m)) => {
                    assert!(
                        m.contains("not authorized"),
                        "{ddl}: unexpected message: {m}"
                    )
                }
                other => panic!("{ddl} must be denied, got {other:?}"),
            }
        }
        store
            .with_conn_fenced(|tx| {
                tx.execute("CREATE TEMP TABLE scratch (k TEXT)", [])
                    .map(|_| ())
            })
            .expect("a scratch table under a fresh name stays allowed");
        // The write reaches the durable table and survives a reopen.
        store
            .with_conn_fenced(|tx| {
                tx.execute("INSERT INTO kv (k, v) VALUES ('a', 'b')", [])
                    .map(|_| ())
            })
            .expect("write");
        drop(store);
        let reopened = open_sqlite(&d, KV_BASELINE).expect("reopen");
        let count: i64 = reopened
            .with_conn(|conn| conn.query_row("SELECT count(*) FROM kv", [], |row| row.get(0)))
            .expect("count");
        assert_eq!(count, 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A temp object that maintenance left under a main-schema name is refused when a
    /// callback scope is installed, so a fenced write cannot commit into the shadow and a
    /// read cannot answer from it; dropping the shadow restores both callbacks.
    #[test]
    fn a_preexisting_temp_shadow_refuses_callbacks_until_it_is_dropped() {
        let (root, d) = tmp();
        let store = open_sqlite(&d, KV_BASELINE).expect("open");
        store
            .with_conn_unfenced(|conn| {
                conn.execute("CREATE TEMP TABLE KV (k TEXT PRIMARY KEY, v TEXT)", [])
                    .map(|_| ())
            })
            .expect("maintenance creates a temp shadow");
        let write = store.with_conn_fenced(|tx| {
            tx.execute("INSERT INTO kv (k, v) VALUES ('a', 'b')", [])
                .map(|_| ())
        });
        match write {
            Err(StoreError::Backend(m)) => assert!(m.contains("shadows"), "{m}"),
            other => panic!("the fenced callback must be refused, got {other:?}"),
        }
        let read = store.with_conn(|conn| {
            conn.query_row("SELECT count(*) FROM kv", [], |row| row.get::<_, i64>(0))
        });
        match read {
            Err(StoreError::Backend(m)) => assert!(m.contains("shadows"), "{m}"),
            other => panic!("the read callback must be refused, got {other:?}"),
        }
        store
            .with_conn_unfenced(|conn| conn.execute("DROP TABLE temp.KV", []).map(|_| ()))
            .expect("maintenance drops the shadow");
        store
            .with_conn_fenced(|tx| {
                tx.execute("INSERT INTO kv (k, v) VALUES ('a', 'b')", [])
                    .map(|_| ())
            })
            .expect("the write reaches the store once the shadow is gone");
        drop(store);
        let reopened = open_sqlite(&d, KV_BASELINE).expect("reopen");
        let count: i64 = reopened
            .with_conn(|conn| conn.query_row("SELECT count(*) FROM kv", [], |row| row.get(0)))
            .expect("count");
        assert_eq!(count, 1, "the write after the drop is durable");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The shadow and infrastructure guards must compare identifiers without calling
    /// SQLite's overridable `lower` function.
    #[test]
    fn a_scalar_function_named_lower_does_not_blind_the_shadow_guard() {
        let (root, d) = tmp();
        let store = open_sqlite(&d, KV_BASELINE).expect("open");
        store
            .with_conn_unfenced(|c| {
                c.create_scalar_function(
                    "lower",
                    1,
                    rusqlite::functions::FunctionFlags::SQLITE_UTF8,
                    |_| Ok("unrelated".to_string()),
                )
            })
            .expect("register a function that replaces lower");
        let lowered: String = store
            .with_conn(|c| c.query_row("SELECT lower('KV')", [], |r| r.get(0)))
            .expect("call");
        assert_eq!(
            lowered, "unrelated",
            "the built-in is replaced on this connection"
        );

        let shadow = store.with_conn_fenced(|tx| {
            tx.execute("CREATE TEMP TABLE KV (k TEXT PRIMARY KEY, v TEXT)", [])
                .map(|_| ())
        });
        assert!(
            matches!(&shadow, Err(StoreError::Backend(m)) if m.contains("not authorized")),
            "a temp shadow of a main table must still be denied, got {shadow:?}"
        );
        store
            .with_conn_unfenced(|c| {
                c.execute("CREATE TEMP TABLE Fence (epoch INTEGER)", [])
                    .map(|_| ())
            })
            .expect("maintenance creates a temp object under an infrastructure name");
        let refused = store.with_conn(|c| c.query_row("SELECT 1", [], |_| Ok(())));
        assert!(
            matches!(&refused, Err(StoreError::Backend(m)) if m.contains("shadows")),
            "a temp `Fence` must still be detected, got {refused:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A committed schema change would make the next open refuse the file, so the
    /// fenced path denies main-schema DDL and the store reopens under its baseline.
    #[test]
    fn fenced_callbacks_cannot_change_the_schema_so_the_store_stays_reopenable() {
        let (root, d) = tmp();
        let store = open_sqlite(&d, KV_BASELINE).expect("open");
        for ddl in [
            "CREATE TABLE extra (k TEXT)",
            "ALTER TABLE kv ADD COLUMN extra TEXT",
            "CREATE INDEX kv_v ON kv (v)",
            "CREATE VIEW kv_view AS SELECT k FROM kv",
            "CREATE TRIGGER kv_trigger AFTER INSERT ON kv BEGIN SELECT 1; END",
            "DROP TABLE kv",
        ] {
            let denied = store.with_conn_fenced(|tx| tx.execute(ddl, []).map(|_| ()));
            assert!(
                matches!(&denied, Err(StoreError::Backend(m)) if m.contains("not authorized")),
                "{ddl} must be denied, got {denied:?}"
            );
        }
        // Temporary objects are outside the baseline comparison and stay allowed.
        store
            .with_conn_fenced(|tx| {
                tx.execute("CREATE TEMP TABLE scratch (k TEXT)", [])
                    .map(|_| ())
            })
            .expect("temporary objects stay allowed");
        drop(store);
        drop(open_sqlite(&d, KV_BASELINE).expect("the file still matches its baseline"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Any symlink in the database path, whether a directory component or the file
    /// itself, is refused, so no alias can reach the bytes or a second lease.
    #[cfg(unix)]
    #[test]
    fn symlinked_database_paths_are_refused_never_aliased() {
        let (root, real) = tmp();
        let StorageBackend::Sqlite { path: real_path } = &real.backend else {
            panic!("sqlite descriptor");
        };
        let real_path = std::path::PathBuf::from(real_path);
        let held = open_sqlite(&real, "").expect("hold the real path");

        let dir_alias = root.join("dir-alias");
        std::os::unix::fs::symlink(real_path.parent().unwrap(), &dir_alias).expect("dir symlink");
        let via_dir = StorageDescriptor {
            backend: StorageBackend::Sqlite {
                path: dir_alias.join("store.db").to_string_lossy().into_owned(),
            },
            ..real.clone()
        };
        // While the real path is held, the alias resolves to the held lease and is refused
        // before its file is read; once released, the alias is refused at the inspection
        // open, which does not follow a symlinked component.
        let via_dir_result = open_sqlite(&via_dir, "").map(|_| ());
        assert!(
            matches!(
                via_dir_result,
                Err(StoreError::Lease(lease::LeaseError::Held { .. }))
            ),
            "a directory-symlink alias of a held store must be refused as held, got {via_dir_result:?}"
        );
        drop(held);
        let via_dir_result = open_sqlite(&via_dir, "").map(|_| ());
        assert!(
            matches!(via_dir_result, Err(StoreError::Backend(_))),
            "a directory-symlink alias must be refused, got {via_dir_result:?}"
        );
        let held = open_sqlite(&real, "").expect("hold the real path again");

        let file_alias_dir = root.join("file-alias");
        std::fs::create_dir_all(&file_alias_dir).expect("alias dir");
        let file_alias = file_alias_dir.join("other.db");
        std::os::unix::fs::symlink(&real_path, &file_alias).expect("file symlink");
        let via_file = StorageDescriptor {
            backend: StorageBackend::Sqlite {
                path: file_alias.to_string_lossy().into_owned(),
            },
            ..real.clone()
        };
        assert!(
            matches!(
                open_sqlite(&via_file, "").map(|_| ()),
                Err(StoreError::Baseline(_))
            ),
            "a file-symlink alias must be refused"
        );
        drop(held);
        // Refusal does not depend on contention: both aliases stay refused once free.
        assert!(matches!(
            open_sqlite(&via_dir, "").map(|_| ()),
            Err(StoreError::Backend(_))
        ));
        assert!(matches!(
            open_sqlite(&via_file, "").map(|_| ()),
            Err(StoreError::Baseline(_))
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_at_the_database_path_is_refused_without_creating_the_target() {
        let (root, d) = tmp();
        let StorageBackend::Sqlite { path } = &d.backend else {
            panic!("sqlite descriptor");
        };
        std::fs::create_dir_all(&root).expect("root");
        let target = root.join("elsewhere.db");
        std::os::unix::fs::symlink(&target, path).expect("dangling symlink");
        match open_sqlite(&d, "").map(|_| ()) {
            Err(StoreError::Baseline(m)) => {
                assert!(m.contains("not a regular file"), "unexpected message: {m}")
            }
            Ok(()) => panic!("open through a dangling symlink must fail"),
            Err(other) => panic!("expected a non-regular-file refusal, got {other:?}"),
        }
        assert!(!target.exists(), "the link target must not be created");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn new_database_file_is_owner_only_at_creation() {
        use std::os::unix::fs::PermissionsExt;

        let (root, _) = tmp();
        std::fs::create_dir_all(&root).expect("root");
        let path = root.join("fresh.db");
        let previous = unsafe { libc::umask(0o022) };
        let created = create_database_file_owner_only(&path);
        unsafe { libc::umask(previous) };
        created.expect("create database file");
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "created with umask 022, got {mode:o}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn reopening_a_permissive_store_protects_the_database_and_its_wal() {
        use std::os::unix::fs::PermissionsExt;

        let (root, descriptor) = tmp();
        let StorageBackend::Sqlite { path } = &descriptor.backend else {
            panic!("sqlite descriptor");
        };
        let path = std::path::PathBuf::from(path);
        let wal = std::path::PathBuf::from(format!("{}-wal", path.display()));
        let shm = std::path::PathBuf::from(format!("{}-shm", path.display()));

        drop(open_sqlite(&descriptor, KV_BASELINE).expect("first open"));

        std::fs::write(&wal, b"").expect("leave a WAL behind");
        std::fs::write(&shm, b"").expect("leave an SHM behind");

        for file in [&path, &wal, &shm] {
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o644))
                .expect("set permissive mode");
        }

        let store = open_sqlite(&descriptor, KV_BASELINE).expect("reopen");

        let mode = |p: &std::path::Path| {
            std::fs::metadata(p)
                .unwrap_or_else(|error| panic!("stat {}: {error}", p.display()))
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(
            mode(&path),
            0o600,
            "the database stayed group/world readable on reopen"
        );
        assert_eq!(
            mode(&wal),
            0o600,
            "the WAL stayed group/world readable while the database looked correct"
        );
        assert_eq!(
            mode(&shm),
            0o600,
            "the SHM stayed group/world readable while the database looked correct"
        );

        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn protection_failure_aborts_open_before_the_fence_write() {
        let (root, descriptor) = tmp();
        let StorageBackend::Sqlite { path } = &descriptor.backend else {
            panic!("sqlite descriptor");
        };
        let path = path.clone();
        let seeded = open_sqlite(&descriptor, "").expect("seed").epoch();

        let shm = format!("{path}-shm");
        std::fs::create_dir(&shm).expect("plant a directory at the shm path");
        match open_sqlite(&descriptor, "") {
            Err(StoreError::Backend(_) | StoreError::Baseline(_)) => {}
            Err(other) => panic!("protection failure must abort the open, got {other}"),
            Ok(_) => panic!("protection failure must abort the open, got a store"),
        }

        let conn = rusqlite::Connection::open(&path).expect("inspect fence");
        let epoch: i64 = conn
            .query_row("SELECT epoch FROM fence WHERE id = 0", [], |r| r.get(0))
            .expect("read fence epoch");
        assert_eq!(
            epoch as u64, seeded,
            "the aborted open wrote a fence epoch before protecting the files"
        );
        drop(conn);
        std::fs::remove_dir(&shm).expect("remove planted directory");
        let _ = std::fs::remove_dir_all(&root);
    }

    fn tmp() -> (std::path::PathBuf, StorageDescriptor) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "storage-{}-{}-{}",
            std::process::id(),
            now_nanos(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let db = root.join("store.db");
        let descriptor = StorageDescriptor {
            module_id: "test-module".into(),
            storage_namespace: "main".into(),
            isolation: Isolation::Module,
            backend: StorageBackend::Sqlite {
                path: db.to_string_lossy().into_owned(),
            },
        };
        (root, descriptor)
    }

    fn now_nanos() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }

    fn sqlite_path(d: &StorageDescriptor) -> String {
        match &d.backend {
            StorageBackend::Sqlite { path } => path.clone(),
            _ => unreachable!(),
        }
    }

    fn remove_lease_sidecar(root: &std::path::Path) {
        let lease = std::fs::read_dir(root)
            .expect("read store directory")
            .map(|entry| entry.expect("directory entry").path())
            .find(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "lease")
            })
            .expect("lease sidecar");
        std::fs::remove_file(lease).expect("remove lease sidecar");
    }

    #[test]
    fn open_claims_fence_before_return() {
        let (root, d) = tmp();
        let store = open_sqlite(&d, "").expect("open");
        let (claimed, rows): (i64, i64) = store
            .with_conn(|c| {
                c.query_row(
                    "SELECT (SELECT epoch FROM fence WHERE id = 0), (SELECT COUNT(*) FROM fence)",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
            })
            .expect("open claimed fence");
        assert_eq!((claimed as u64, rows), (store.epoch(), 1));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn open_claim_rejects_an_epoch_the_database_already_stores() {
        let (root, d) = tmp();
        let path = sqlite_path(&d);
        open_sqlite(&d, "").expect("seed database");

        let mut conn = rusqlite::Connection::open(&path).expect("reopen database");
        let stored: u64 = conn
            .query_row("SELECT epoch FROM fence WHERE id = 0", [], |row| {
                row.get::<_, i64>(0)
            })
            .map(|epoch| epoch as u64)
            .expect("stored fence");

        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .expect("claim transaction");
        match claim_fence_strict(&tx, stored, FileState::Baseline) {
            Err(StoreError::Fenced {
                holder_epoch,
                db_epoch,
            }) => {
                assert_eq!(holder_epoch, stored);
                assert_eq!(db_epoch, stored);
            }
            other => panic!("expected an equal epoch to be rejected, got {other:?}"),
        }
        assert!(
            claim_fence(&tx, stored).is_ok(),
            "an equal epoch stays authorized for a holder that already claimed it"
        );
        claim_fence_strict(&tx, stored + 1, FileState::Baseline)
            .expect("a strictly greater epoch claims");
        drop(tx);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn fresh_file_matches_the_baseline_inventory() {
        let (root, d) = tmp();
        let path = sqlite_path(&d);
        drop(open_sqlite(&d, "").expect("open a fresh store"));

        let conn = rusqlite::Connection::open(&path).expect("reopen the fresh file");
        let application_id: i64 = conn
            .query_row("PRAGMA application_id", [], |r| r.get(0))
            .expect("application_id");
        let user_version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .expect("user_version");
        assert_eq!(application_id as u32, APPLICATION_ID);
        assert_eq!(user_version as u32, USER_VERSION);
        let (marker_rows, marker): (i64, String) = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM format_marker), \
                 (SELECT baseline_sha256 FROM format_marker WHERE id = 0)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("format marker");
        assert_eq!(marker_rows, 1, "exactly one format-marker row");
        let inventory = schema_inventory(&conn).expect("schema inventory");
        drop(conn);

        let fixture: serde_json::Value =
            serde_json::from_str(INVENTORY_FIXTURE).expect("fixture parses");
        assert_eq!(
            fixture["application_id"].as_u64(),
            Some(u64::from(APPLICATION_ID))
        );
        assert_eq!(
            fixture["user_version"].as_u64(),
            Some(u64::from(USER_VERSION))
        );
        assert_eq!(
            fixture["baseline_sha256"].as_str(),
            Some(marker.as_str()),
            "the format marker matches the fixture digest"
        );
        let expected = fixture["objects"].as_array().expect("objects array");
        assert_eq!(
            inventory.len(),
            expected.len(),
            "object count differs from the fixture: {inventory:?}"
        );
        for (found, wanted) in inventory.iter().zip(expected) {
            assert_eq!(Some(found.kind.as_str()), wanted["type"].as_str());
            assert_eq!(Some(found.name.as_str()), wanted["name"].as_str());
            assert_eq!(Some(found.table.as_str()), wanted["tbl_name"].as_str());
            assert_eq!(found.sql.as_deref(), wanted["sql"].as_str());
        }
        for object in &inventory {
            for token in ["schema_version", "migration"] {
                assert!(
                    !object.name.contains(token),
                    "object `{}` names a version ledger (`{token}`)",
                    object.name
                );
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_consumer_baseline_is_applied_once_and_verified_on_reopen() {
        let (root, d) = tmp();
        let path = sqlite_path(&d);
        {
            let store = open_sqlite(&d, KV_BASELINE).expect("first open applies the baseline");
            assert_eq!(store.epoch(), 1);
            store
                .with_conn_fenced(|tx| {
                    tx.execute("INSERT INTO kv (k, v) VALUES ('a', '1')", [])
                        .map(|_| ())
                })
                .expect("fenced write");
        }
        {
            let store = open_sqlite(&d, KV_BASELINE).expect("reopen with the same baseline");
            assert_eq!(
                store.epoch(),
                2,
                "the lease epoch is monotonic across opens"
            );
            let v: String = store
                .with_conn(|c| c.query_row("SELECT v FROM kv WHERE k = 'a'", [], |r| r.get(0)))
                .expect("the row survives the reopen");
            assert_eq!(v, "1");
        }

        let before = std::fs::read(&path).expect("read the file before the refused open");
        let refused = open_sqlite(
            &d,
            "CREATE TABLE kv (k TEXT PRIMARY KEY, v TEXT NOT NULL, extra TEXT);",
        );
        assert!(
            matches!(refused, Err(StoreError::Baseline(_))),
            "a different baseline is refused, got {:?}",
            refused.map(|_| ())
        );
        let after = std::fs::read(&path).expect("read the file after the refused open");
        assert!(before == after, "the refused open changed the file's bytes");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_baseline_that_does_not_apply_is_rejected_before_the_file_is_touched() {
        let (root, d) = tmp();
        let path = sqlite_path(&d);
        let refused = open_sqlite(&d, "CREATE TABLE (");
        assert!(
            matches!(refused, Err(StoreError::Baseline(_))),
            "an unparseable baseline is refused, got {:?}",
            refused.map(|_| ())
        );
        assert!(
            !std::path::Path::new(&path).exists(),
            "the refused open must not create the database file"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn database_epoch_survives_repeated_lease_sidecar_loss() {
        let (root, d) = tmp();
        let first = open_sqlite(&d, "").expect("first open");
        let first_epoch = first.epoch();
        drop(first);

        remove_lease_sidecar(&root);
        let second = open_sqlite(&d, "").expect("open after first sidecar loss");
        assert!(second.epoch() > first_epoch);
        let second_epoch = second.epoch();
        drop(second);

        remove_lease_sidecar(&root);
        let third = open_sqlite(&d, "").expect("open after second sidecar loss");
        assert!(third.epoch() > second_epoch);
        let db_epoch: i64 = third
            .with_conn(|conn| {
                conn.query_row("SELECT epoch FROM fence WHERE id = 0", [], |row| row.get(0))
            })
            .expect("database fence");
        assert_eq!(db_epoch as u64, third.epoch());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn second_live_writer_is_rejected() {
        let (root, d) = tmp();
        let _held = open_sqlite(&d, "").expect("first open");
        match open_sqlite(&d, "") {
            Err(StoreError::Lease(_)) => {}
            Err(e) => panic!("expected Lease(Held), got {e}"),
            Ok(_) => panic!("expected Lease(Held), got a second open"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Lease keys include the store directory and the database file name, so distinct
    /// databases open concurrently.
    #[test]
    fn distinct_databases_do_not_falsely_contend() {
        let (root_a, a) = tmp();
        let (root_b, b) = tmp();
        let mut c = a.clone();
        c.backend = StorageBackend::Sqlite {
            path: root_a.join("other.db").to_string_lossy().into_owned(),
        };
        let held_a = open_sqlite(&a, "").expect("open a");
        let held_b = open_sqlite(&b, "").expect("open b - same file name in another directory");
        let held_c = open_sqlite(&c, "").expect("open c - distinct db in the same directory as a");
        drop((held_a, held_b, held_c));
        let _ = std::fs::remove_dir_all(&root_a);
        let _ = std::fs::remove_dir_all(&root_b);
    }

    /// `claim_fence` rejects suppressed and undone updates instead of returning a stale claim.
    #[test]
    fn a_fence_update_a_trigger_suppresses_or_undoes_is_an_error_not_a_silent_success() {
        for (trigger, needle) in [
            (
                "CREATE TRIGGER fence_suppressor BEFORE UPDATE ON fence \
                 BEGIN SELECT RAISE(IGNORE); END",
                "affected 0 rows",
            ),
            (
                "CREATE TRIGGER fence_undo AFTER UPDATE ON fence \
                 BEGIN UPDATE fence SET epoch = OLD.epoch WHERE id = 0; END",
                "reads back as 1",
            ),
        ] {
            let (root, d) = tmp();
            let StorageBackend::Sqlite { path } = &d.backend else {
                panic!("sqlite descriptor");
            };
            let path = path.clone();
            drop(open_sqlite(&d, "").expect("seed database at epoch 1"));

            let mut conn = rusqlite::Connection::open(&path).expect("reopen raw");
            conn.execute_batch(trigger).expect("install trigger");
            let tx = conn.transaction().expect("tx");
            match claim_fence(&tx, 99) {
                Err(StoreError::Backend(m)) => {
                    assert!(m.contains(needle), "{trigger}: unexpected message: {m}")
                }
                other => panic!("{trigger}: fence write must fail, got {other:?}"),
            }
            drop(tx);
            drop(conn);
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    #[cfg(unix)]
    #[test]
    fn fresh_open_creates_owner_only_sidecars_under_a_permissive_umask() {
        use std::os::unix::fs::PermissionsExt;

        let (root, d) = tmp();
        let StorageBackend::Sqlite { path } = &d.backend else {
            panic!("sqlite descriptor");
        };
        let path = path.clone();
        let previous = unsafe { libc::umask(0o022) };
        let opened = open_sqlite(&d, "");
        unsafe { libc::umask(previous) };
        let store = opened.expect("fresh open");
        for suffix in ["", "-wal", "-shm"] {
            let sidecar = format!("{path}{suffix}");
            let mode = std::fs::metadata(&sidecar)
                .unwrap_or_else(|e| panic!("{sidecar} must exist after open: {e}"))
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(
                mode, 0o600,
                "{sidecar} created under umask 022 has mode {mode:o}"
            );
        }
        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn store_error_source_preserves_the_underlying_errno() {
        let err = StoreError::Io(std::io::Error::from_raw_os_error(28));
        let source = std::error::Error::source(&err).expect("Io must expose a source");
        let io = source
            .downcast_ref::<std::io::Error>()
            .expect("source is the io error");
        assert_eq!(io.raw_os_error(), Some(28));

        let err = StoreError::Lease(lease::LeaseError::Io(std::io::Error::from_raw_os_error(
            122,
        )));
        let mut cursor: &dyn std::error::Error = &err;
        let mut errno = None;
        while let Some(next) = cursor.source() {
            if let Some(io) = next.downcast_ref::<std::io::Error>() {
                errno = io.raw_os_error();
            }
            cursor = next;
        }
        assert_eq!(
            errno,
            Some(122),
            "errno unreachable through the Lease chain"
        );
    }

    #[test]
    fn unsupported_backend_is_rejected() {
        let d = StorageDescriptor {
            module_id: "m".into(),
            storage_namespace: "n".into(),
            isolation: Isolation::Module,
            backend: StorageBackend::Postgres {
                dsn: "postgres://x".into(),
                database: "y".into(),
            },
        };
        match open_sqlite(&d, "") {
            Err(StoreError::UnsupportedBackend(b)) => assert_eq!(b, "postgres"),
            Err(e) => panic!("expected UnsupportedBackend, got {e}"),
            Ok(_) => panic!("expected UnsupportedBackend, got an open store"),
        }
    }

    #[test]
    fn unfenced_connection_rejects_writes() {
        let (root, d) = tmp();
        let store = open_sqlite(&d, KV_BASELINE).expect("open");
        let r = store.with_conn(|c| {
            c.execute("INSERT INTO kv (k, v) VALUES ('sneak', '1')", [])
                .map(|_| ())
        });
        assert!(
            matches!(&r, Err(StoreError::Backend(m)) if m.contains("readonly")),
            "unfenced write must fail with SQLITE_READONLY, got {r:?}"
        );
        let n: i64 = store
            .with_conn(|c| c.query_row("SELECT COUNT(*) FROM kv", [], |r| r.get(0)))
            .expect("count");
        assert_eq!(n, 0, "the rejected write left no row");
        store
            .with_conn_fenced(|tx| {
                tx.execute("INSERT INTO kv (k, v) VALUES ('a', '1')", [])
                    .map(|_| ())
            })
            .expect("fenced writes still work after the read-only guard clears");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_panicking_read_does_not_strand_the_connection_read_only() {
        let (root, d) = tmp();
        let store = open_sqlite(&d, KV_BASELINE).expect("open");

        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            store.with_conn(|_| -> rusqlite::Result<()> { panic!("callback panics") })
        }));
        assert!(panicked.is_err(), "the callback's panic propagates");

        store
            .with_conn_fenced(|tx| {
                tx.execute("INSERT INTO kv (k, v) VALUES ('after-panic', '1')", [])
                    .map(|_| ())
            })
            .expect("a fenced write after a panicking read is still authorized");
        store
            .with_conn_unfenced(|c| c.execute_batch("VACUUM"))
            .expect("maintenance after a panicking read still reaches the database");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_read_callback_cannot_lower_fence_durability() {
        let (root, d) = tmp();
        let store = open_sqlite(&d, KV_BASELINE).expect("open");

        let lowered = store.with_conn(|c| c.execute("PRAGMA synchronous = OFF", []));
        assert!(
            matches!(&lowered, Err(StoreError::Backend(m)) if m.contains("not authorized")),
            "the read guard denies lowering synchronous, got {lowered:?}"
        );
        let unchanged: i64 = store
            .with_conn(|c| c.query_row("PRAGMA synchronous", [], |r| r.get(0)))
            .expect("reading a pragma stays allowed");
        assert_eq!(unchanged, 2, "the denied pragma left synchronous=FULL");

        store
            .with_conn_unfenced(|c| c.pragma_update(None, "synchronous", "OFF"))
            .expect("maintenance may lower it");
        store
            .with_conn_fenced(|tx| {
                tx.execute("INSERT INTO kv (k, v) VALUES ('durable', '1')", [])
                    .map(|_| ())
            })
            .expect("fenced write");
        let after: i64 = store
            .with_conn(|c| c.query_row("PRAGMA synchronous", [], |r| r.get(0)))
            .expect("read synchronous");
        assert_eq!(
            after, 2,
            "the fenced write re-pinned synchronous=FULL, so the committed epoch is crash-durable"
        );

        store
            .with_conn_unfenced(|c| c.pragma_update(None, "synchronous", "NORMAL"))
            .expect("lower again");
        store
            .with_conn_fenced(|tx| {
                tx.execute("INSERT INTO kv (k, v) VALUES ('again', 'v')", [])
                    .map(|_| ())
            })
            .expect("second fenced write");
        let after_second: i64 = store
            .with_conn(|c| c.query_row("PRAGMA synchronous", [], |r| r.get(0)))
            .expect("read synchronous");
        assert_eq!(
            after_second, 2,
            "the first fenced write after each maintenance callback re-pins synchronous=FULL"
        );

        store
            .with_conn_unfenced(|c| c.pragma_update(None, "journal_mode", "MEMORY"))
            .expect("maintenance may drop the journal");
        store
            .with_conn_fenced(|tx| {
                tx.execute("INSERT INTO kv (k, v) VALUES ('journal', '1')", [])
                    .map(|_| ())
            })
            .expect("fenced write");
        let journal: String = store
            .with_conn(|c| c.query_row("PRAGMA journal_mode", [], |r| r.get(0)))
            .expect("read journal_mode");
        assert!(
            journal.eq_ignore_ascii_case("wal"),
            "the fenced write restored a crash-safe journal, got {journal}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_read_callback_cannot_clear_the_read_only_guard() {
        let (root, d) = tmp();
        let store = open_sqlite(&d, KV_BASELINE).expect("open");

        let bypass = store.with_conn(|c| {
            c.execute("PRAGMA query_only = OFF", [])?;
            c.execute("INSERT INTO kv (k, v) VALUES ('bypass', '1')", [])
                .map(|_| ())
        });
        assert!(
            matches!(&bypass, Err(StoreError::Backend(m)) if m.contains("not authorized")),
            "clearing the guard is denied before any write runs, got {bypass:?}"
        );
        for pragma in ["journal_mode", "locking_mode", "writable_schema"] {
            let denied =
                store.with_conn(|c| c.execute(&format!("PRAGMA {pragma} = EXCLUSIVE"), []));
            assert!(
                matches!(&denied, Err(StoreError::Backend(m)) if m.contains("not authorized")),
                "setting {pragma} from a read callback is denied, got {denied:?}"
            );
        }
        for spelling in ["QUERY_ONLY", "Query_Only", "qUeRy_OnLy"] {
            let denied = store.with_conn(|c| {
                c.execute(&format!("PRAGMA {spelling} = OFF"), [])?;
                c.execute("INSERT INTO kv (k, v) VALUES ('cased', '1')", [])
                    .map(|_| ())
            });
            assert!(
                matches!(&denied, Err(StoreError::Backend(m)) if m.contains("not authorized")),
                "`PRAGMA {spelling}` is denied like the lowercase spelling, got {denied:?}"
            );
        }
        let rows: i64 = store
            .with_conn(|c| c.query_row("SELECT COUNT(*) FROM kv", [], |r| r.get(0)))
            .expect("count");
        assert_eq!(
            rows, 0,
            "no spelling of the guard pragma let a write through"
        );

        store
            .with_conn_fenced(|tx| {
                tx.execute("INSERT INTO kv (k, v) VALUES ('fenced', '1')", [])
                    .map(|_| ())
            })
            .expect("the fenced path still works after a denied callback");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_callback_cannot_end_the_fence_checked_transaction() {
        let (root, d) = tmp();
        let store = open_sqlite(&d, KV_BASELINE).expect("open");

        for control in ["COMMIT", "ROLLBACK", "SAVEPOINT s", "BEGIN"] {
            let r = store.with_conn_fenced(|tx| {
                tx.execute(control, [])?;
                tx.execute("INSERT INTO kv (k, v) VALUES ('escaped', '1')", [])
                    .map(|_| ())?;
                Ok(())
            });
            assert!(
                matches!(&r, Err(StoreError::Backend(m)) if m.contains("not authorized")),
                "`{control}` is denied before it can end the transaction, got {r:?}"
            );
        }
        let escaped: i64 = store
            .with_conn(|c| c.query_row("SELECT COUNT(*) FROM kv", [], |r| r.get(0)))
            .expect("count");
        assert_eq!(
            escaped, 0,
            "denial happens before the statement runs, so nothing commits unfenced"
        );

        let ddl = store.with_conn_fenced(|tx| {
            tx.execute("COMMIT", [])?;
            tx.execute("CREATE TABLE escaped_ddl (v TEXT)", [])
                .map(|_| ())
        });
        assert!(
            matches!(&ddl, Err(StoreError::Backend(m)) if m.contains("not authorized")),
            "a callback that ends its transaction before DDL is denied, got {ddl:?}"
        );
        let ddl_exists: bool = store
            .with_conn(|c| {
                c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name = 'escaped_ddl')",
                    [],
                    |r| r.get(0),
                )
            })
            .expect("schema lookup");
        assert!(!ddl_exists, "the denied callback created no table");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_callback_cannot_damage_the_fence_row_or_format_marker_it_is_checked_against() {
        let (root, d) = tmp();
        let store = open_sqlite(&d, KV_BASELINE).expect("open");
        let epoch = store.epoch();
        let marker_before: String = store
            .with_conn(|c| {
                c.query_row(
                    "SELECT baseline_sha256 FROM format_marker WHERE id = 0",
                    [],
                    |r| r.get(0),
                )
            })
            .expect("read the marker");
        let forged_marker = format!(
            "UPDATE format_marker SET baseline_sha256 = '{}' WHERE id = 0",
            "f".repeat(64)
        );

        for sql in [
            "UPDATE fence SET epoch = 0 WHERE id = 0",
            "DELETE FROM fence WHERE id = 0",
            "INSERT INTO format_marker (id, baseline_sha256) VALUES (0, 'forged')",
            forged_marker.as_str(),
            "DELETE FROM format_marker WHERE id = 0",
            "CREATE TRIGGER freeze_fence BEFORE UPDATE ON fence \
             BEGIN SELECT RAISE(IGNORE); END",
            "DROP TABLE fence",
            "DROP TABLE format_marker",
            "CREATE INDEX fence_idx ON fence (epoch)",
            "CREATE TEMP VIEW fence AS SELECT 0 AS id, 1 AS epoch",
            "CREATE TEMP VIEW format_marker AS SELECT 0 AS id, 'forged' AS baseline_sha256",
        ] {
            let r = store.with_conn_fenced(|tx| tx.execute(sql, []).map(|_| ()));
            assert!(
                matches!(&r, Err(StoreError::Backend(m)) if m.contains("not authorized")),
                "`{sql}` is denied inside a fenced callback, got {r:?}"
            );
        }

        let objects: i64 = store
            .with_conn(|c| {
                c.query_row(
                    "SELECT COUNT(*) FROM sqlite_schema WHERE name IN \
                     ('freeze_fence', 'fence_idx') \
                     OR (name IN ('fence', 'format_marker') AND type <> 'table')",
                    [],
                    |r| r.get(0),
                )
            })
            .expect("schema lookup");
        assert_eq!(objects, 0, "no denied schema object was created");
        let shadow: i64 = store
            .with_conn(|c| {
                c.query_row(
                    "SELECT COUNT(*) FROM temp.sqlite_schema \
                     WHERE name IN ('fence', 'format_marker')",
                    [],
                    |r| r.get(0),
                )
            })
            .expect("temp schema lookup");
        assert_eq!(
            shadow, 0,
            "no temporary object shadows an infrastructure table"
        );

        let stored: i64 = store
            .with_conn(|c| c.query_row("SELECT epoch FROM fence WHERE id = 0", [], |r| r.get(0)))
            .expect("read the fence row");
        assert_eq!(
            stored, epoch as i64,
            "the fence row still carries the epoch the callbacks were checked against"
        );

        // Schema changes are denied, so renames cannot shadow `fence` or `format_marker`.
        for target in ["fence", "FENCE", "Fence", "format_marker"] {
            let renamed = store.with_conn_fenced(|tx| {
                tx.execute("CREATE TEMP TABLE benign (id INTEGER, epoch INTEGER)", [])?;
                tx.execute(&format!("ALTER TABLE benign RENAME TO {target}"), [])
                    .map(|_| ())
            });
            assert!(
                matches!(&renamed, Err(StoreError::Backend(m)) if m.contains("not authorized")),
                "a temporary table renamed to `{target}` is rejected, got {renamed:?}"
            );
        }
        let shadows: i64 = store
            .with_conn(|c| {
                c.query_row(
                    "SELECT COUNT(*) FROM temp.sqlite_schema \
                     WHERE lower(name) IN ('fence', 'format_marker')",
                    [],
                    |r| r.get(0),
                )
            })
            .expect("inspect temp schema");
        assert_eq!(
            shadows, 0,
            "no temporary object may shadow an infrastructure table"
        );
        let shadow_after: i64 = store
            .with_conn(|c| {
                c.query_row(
                    "SELECT COUNT(*) FROM temp.sqlite_schema WHERE name = 'fence'",
                    [],
                    |r| r.get(0),
                )
            })
            .expect("temp schema lookup");
        assert_eq!(
            shadow_after, 0,
            "rejecting before commit rolls the temporary object back with the transaction"
        );
        let (marker_after, marker_rows): (String, i64) = store
            .with_conn(|c| {
                c.query_row(
                    "SELECT (SELECT baseline_sha256 FROM format_marker WHERE id = 0), \
                     (SELECT COUNT(*) FROM format_marker)",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
            })
            .expect("read the marker again");
        assert_eq!(
            (marker_after, marker_rows),
            (marker_before, 1),
            "the format marker row is unchanged"
        );
        store
            .with_conn_fenced(|tx| {
                tx.execute("INSERT INTO kv (k, v) VALUES ('after-shadow', '1')", [])
                    .map(|_| ())
            })
            .expect("the fenced path still works after a rejected shadow");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `VACUUM` fails because read callbacks run inside a transaction; authorization does not
    /// reject it. The authorizer denies `wal_checkpoint`, `incremental_vacuum`, `optimize`,
    /// and `shrink_memory`, which `PRAGMA query_only` does not stop.
    #[test]
    fn maintenance_statements_are_refused_in_read_callbacks_and_run_through_the_unfenced_path() {
        let (root, d) = tmp();
        let store = open_sqlite(&d, KV_BASELINE).expect("open");
        let r = store.with_conn(|c| c.execute("VACUUM", []));
        assert!(
            matches!(&r, Err(StoreError::Backend(m)) if m.contains("cannot VACUUM from within a transaction")),
            "VACUUM must not pass the read callback, got {r:?}"
        );
        for pragma in [
            "wal_checkpoint",
            "incremental_vacuum",
            "optimize",
            "shrink_memory",
        ] {
            let denied =
                store.with_conn(|c| c.query_row(&format!("PRAGMA {pragma}"), [], |_| Ok(())));
            assert!(
                matches!(&denied, Err(StoreError::Backend(m)) if m.contains("authorization denied") || m.contains("not authorized")),
                "{pragma} must be denied by the read callback scope, got {denied:?}"
            );
        }
        let mode: String = store
            .with_conn(|c| c.query_row("PRAGMA journal_mode", [], |r| r.get(0)))
            .expect("reading a pragma stays allowed");
        assert!(mode.eq_ignore_ascii_case("wal"));
        store
            .with_conn_unfenced(|c| c.execute_batch("VACUUM"))
            .expect("VACUUM through the maintenance path");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_maintenance_registered_scalar_function_serves_reads_writes_and_triggers() {
        let (root, d) = tmp();
        let store = open_sqlite(
            &d,
            "CREATE TABLE t (k TEXT PRIMARY KEY, v TEXT);\
             CREATE TRIGGER t_tag AFTER INSERT ON t BEGIN \
               UPDATE t SET v = eidnara_tag(NEW.k) WHERE k = NEW.k; END;",
        )
        .expect("open with a trigger calling the function");
        // SQLite resolves the function when it prepares the statement that fires the
        // trigger, so an insert before registration fails.
        let unregistered =
            store.with_conn_fenced(|tx| tx.execute("INSERT INTO t (k) VALUES ('early')", []));
        assert!(
            matches!(&unregistered, Err(StoreError::Backend(m)) if m.contains("no such function: eidnara_tag")),
            "an insert before registration must fail, got {unregistered:?}"
        );
        store
            .with_conn_unfenced(|c| {
                c.create_scalar_function(
                    "eidnara_tag",
                    1,
                    rusqlite::functions::FunctionFlags::SQLITE_UTF8
                        | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
                    |context| Ok(format!("tagged:{}", context.get::<String>(0)?)),
                )
            })
            .expect("register");
        store
            .with_conn_fenced(|tx| tx.execute("INSERT INTO t (k) VALUES ('a')", []))
            .expect("insert fires the trigger");
        let (rows, v): (i64, String) = store
            .with_conn(|c| {
                c.query_row(
                    "SELECT COUNT(*), (SELECT eidnara_tag(v) FROM t WHERE k = 'a') FROM t",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
            })
            .expect("read calls the function too");
        assert_eq!(rows, 1, "the failed early insert rolled back");
        assert_eq!(v, "tagged:tagged:a");
        // The authorizer still denies `PRAGMA query_only = OFF` after function registration.
        let r = store.with_conn(|c| c.execute("PRAGMA query_only = OFF", []));
        assert!(
            matches!(&r, Err(StoreError::Backend(m)) if m.contains("not authorized")),
            "{r:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_callback_that_reenters_the_store_gets_an_error_instead_of_blocking() {
        let (root, d) = tmp();
        let store = std::sync::Arc::new(open_sqlite(&d, KV_BASELINE).expect("open"));
        let reentered = "store re-entered from a callback";

        // A `Weak` keeps the connection from owning the store that owns the connection.
        let inner = std::sync::Arc::downgrade(&store);
        store
            .with_conn_unfenced(|c| {
                c.create_scalar_function(
                    "reenter",
                    0,
                    rusqlite::functions::FunctionFlags::SQLITE_UTF8,
                    move |_| {
                        let store = inner.upgrade().ok_or_else(|| {
                            rusqlite::Error::UserFunctionError("store dropped".into())
                        })?;
                        store
                            .with_conn(|g| g.query_row("SELECT 1", [], |r| r.get::<_, i64>(0)))
                            .map_err(user_error)
                    },
                )
            })
            .expect("register");
        let r = store.with_conn(|g| g.query_row("SELECT reenter()", [], |r| r.get::<_, i64>(0)));
        assert!(
            matches!(&r, Err(StoreError::Backend(m)) if m.contains(reentered)),
            "scalar-function re-entry must surface as an error, got {r:?}"
        );

        let r = store.with_conn(|_| store.with_conn(|_| Ok(())).map_err(user_error));
        assert!(
            matches!(&r, Err(StoreError::Backend(m)) if m.contains(reentered)),
            "{r:?}"
        );
        let r = store.with_conn_fenced(|_| store.with_conn_fenced(|_| Ok(())).map_err(user_error));
        assert!(
            matches!(&r, Err(StoreError::Backend(m)) if m.contains(reentered)),
            "{r:?}"
        );
        let r =
            store.with_conn_unfenced(|_| store.with_conn_unfenced(|_| Ok(())).map_err(user_error));
        assert!(
            matches!(&r, Err(StoreError::Backend(m)) if m.contains(reentered)),
            "{r:?}"
        );

        // The refusal leaves the store usable, and another thread still takes the lock.
        store
            .with_conn(|g| g.query_row("SELECT 1", [], |_| Ok(())))
            .expect("store still works");
        let other = std::sync::Arc::clone(&store);
        std::thread::spawn(move || other.with_conn(|g| g.query_row("SELECT 1", [], |_| Ok(()))))
            .join()
            .expect("thread")
            .expect("another thread is not re-entry");

        // The registered function holds no strong reference, so the last handle closes the
        // connection and releases the lease.
        drop(store);
        open_sqlite(&d, KV_BASELINE).expect("the lease is released once the last handle drops");
        let _ = std::fs::remove_dir_all(&root);
    }

    fn user_error(e: StoreError) -> rusqlite::Error {
        rusqlite::Error::UserFunctionError(e.to_string().into())
    }

    #[test]
    fn a_bounded_fenced_write_stops_at_its_deadline_and_writes_nothing() {
        let (root, d) = tmp();
        let store = open_sqlite(&d, KV_BASELINE).expect("open");

        // Without contention the bounded write behaves as the unbounded one.
        store
            .with_conn_fenced_within(Instant::now() + Duration::from_secs(5), |tx| {
                tx.execute("INSERT INTO kv (k, v) VALUES ('free', '1')", [])
            })
            .expect("an uncontended bounded write commits");

        // Another connection holds the database write lock past the deadline.
        let blocker = rusqlite::Connection::open(sqlite_path(&d)).expect("blocker");
        blocker.busy_timeout(Duration::ZERO).expect("no wait");
        blocker
            .execute_batch("BEGIN IMMEDIATE")
            .expect("hold the write lock");
        let started = Instant::now();
        let r = store.with_conn_fenced_within(started + Duration::from_millis(200), |tx| {
            tx.execute("INSERT INTO kv (k, v) VALUES ('blocked', '1')", [])
        });
        assert!(matches!(r, Err(StoreError::Deadline)), "{r:?}");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the write waited {:?}, past its deadline and toward the connection's busy timeout",
            started.elapsed()
        );
        drop(blocker);
        let rows: i64 = store
            .with_conn(|c| c.query_row("SELECT COUNT(*) FROM kv", [], |r| r.get(0)))
            .expect("count");
        assert_eq!(rows, 1, "the refused write left no row");

        // The connection's busy timeout is restored, so a later unbounded write waits for a lock the bounded write would have given up on.
        let blocker = rusqlite::Connection::open(sqlite_path(&d)).expect("blocker");
        blocker.busy_timeout(Duration::ZERO).expect("no wait");
        blocker
            .execute_batch("BEGIN IMMEDIATE")
            .expect("hold the write lock");
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            blocker.execute_batch("ROLLBACK").expect("release");
        });
        store
            .with_conn_fenced(|tx| tx.execute("INSERT INTO kv (k, v) VALUES ('waited', '1')", []))
            .expect("the unbounded write outwaits a 400 ms holder");
        release.join().expect("release thread");

        // A bounded write returns `StoreError::Deadline` when an in-process connection stays held past its deadline.
        let held = std::sync::Arc::new(store);
        let (took, release) = std::sync::mpsc::channel::<()>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let holder = std::sync::Arc::clone(&held);
        let thread = std::thread::spawn(move || {
            holder
                .with_conn(|_| {
                    took.send(()).expect("signal");
                    release_rx.recv().expect("wait");
                    Ok(())
                })
                .expect("held read");
        });
        release.recv().expect("the connection is held");
        let started = Instant::now();
        let r = held.with_conn_fenced_within(started + Duration::from_millis(200), |tx| {
            tx.execute("INSERT INTO kv (k, v) VALUES ('held', '1')", [])
        });
        assert!(matches!(r, Err(StoreError::Deadline)), "{r:?}");
        assert!(started.elapsed() < Duration::from_secs(2));
        release_tx.send(()).expect("release");
        thread.join().expect("holder thread");
        drop(held);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_bounded_fenced_write_stops_at_its_deadline_before_begin_runs() {
        let (root, d) = tmp();
        let store = open_sqlite(&d, KV_BASELINE).expect("open");

        // Maintenance is unrestricted by contract and can lower the journal mode
        // between protected transactions; `pin_fence_durability` exists to rewrite it.
        store
            .with_conn_unfenced(|conn| {
                conn.query_row("PRAGMA journal_mode = DELETE", [], |row| {
                    row.get::<_, String>(0)
                })
            })
            .expect("lower the journal mode");

        // In rollback-journal mode an exclusive holder blocks even the fence
        // precheck's read, before `BEGIN` would have narrowed the busy timeout.
        let blocker = rusqlite::Connection::open(sqlite_path(&d)).expect("blocker");
        blocker.busy_timeout(Duration::ZERO).expect("no wait");
        blocker
            .execute_batch("BEGIN EXCLUSIVE")
            .expect("hold the file exclusively");
        let started = Instant::now();
        let r = store.with_conn_fenced_within(started + Duration::from_millis(200), |tx| {
            tx.execute("INSERT INTO kv (k, v) VALUES ('blocked', '1')", [])
        });
        assert!(matches!(r, Err(StoreError::Deadline)), "{r:?}");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the fence precheck waited {:?}, past the deadline and toward the connection's busy timeout",
            started.elapsed()
        );
        blocker.execute_batch("ROLLBACK").expect("release");
        drop(blocker);

        // The busy timeout is restored and the durability pin re-establishes WAL,
        // so a later bounded write commits.
        store
            .with_conn_fenced_within(Instant::now() + Duration::from_secs(5), |tx| {
                tx.execute("INSERT INTO kv (k, v) VALUES ('after', '1')", [])
            })
            .expect("the store recovers once the holder releases");
        let mode: String = store
            .with_conn(|c| c.query_row("PRAGMA journal_mode", [], |r| r.get(0)))
            .expect("mode");
        assert!(mode.eq_ignore_ascii_case("wal"), "journal_mode is {mode}");
        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_bounded_fenced_write_bounds_the_fence_read_and_durability_pin_together() {
        let (root, d) = tmp();
        let store = open_sqlite(&d, KV_BASELINE).expect("open");
        let path = sqlite_path(&d);

        store
            .with_conn_unfenced(|conn| {
                conn.query_row("PRAGMA journal_mode = DELETE", [], |row| {
                    row.get::<_, String>(0)
                })
                .map(|_| ())
            })
            .expect("lower the journal mode");
        store
            .with_conn_unfenced(|conn| conn.pragma_update(None, "synchronous", "NORMAL"))
            .expect("lower synchronous");

        // This reader blocks the durability pin's EXCLUSIVE lock for the whole attempt.
        let reader = rusqlite::Connection::open(&path).expect("reader");
        reader.busy_timeout(Duration::ZERO).expect("no wait");
        reader
            .execute_batch("BEGIN")
            .expect("open read transaction");
        let _: i64 = reader
            .query_row("SELECT COUNT(*) FROM fence", [], |row| row.get(0))
            .expect("take shared lock");

        static BUSY_HANDLER_ENTERED: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);
        static RELEASE_BUSY_HANDLER: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);
        fn controlled_busy_handler(_: i32) -> bool {
            BUSY_HANDLER_ENTERED.store(true, std::sync::atomic::Ordering::Release);
            if RELEASE_BUSY_HANDLER.load(std::sync::atomic::Ordering::Acquire) {
                false
            } else {
                std::thread::sleep(Duration::from_millis(1));
                true
            }
        }

        BUSY_HANDLER_ENTERED.store(false, std::sync::atomic::Ordering::Release);
        RELEASE_BUSY_HANDLER.store(false, std::sync::atomic::Ordering::Release);
        let blocker_path = path.clone();
        let blocker = std::thread::spawn(move || {
            let connection = rusqlite::Connection::open(&blocker_path).expect("pending holder");
            connection
                .busy_handler(Some(controlled_busy_handler))
                .expect("controlled busy handler");
            assert!(
                connection.execute_batch("BEGIN EXCLUSIVE").is_err(),
                "the reader's shared lock must refuse EXCLUSIVE",
            );
        });
        let pending_deadline = Instant::now() + Duration::from_secs(1);
        while !BUSY_HANDLER_ENTERED.load(std::sync::atomic::Ordering::Acquire) {
            assert!(
                Instant::now() < pending_deadline,
                "the EXCLUSIVE attempt never reached its busy handler",
            );
            std::thread::yield_now();
        }

        let wait_started = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let wait_signal = std::sync::Arc::clone(&wait_started);
        before_next_busy_wait_for_test(move || {
            wait_signal.store(true, std::sync::atomic::Ordering::Release);
        });
        let started = Instant::now();
        let release_deadline = Instant::now() + Duration::from_secs(2);
        let release = std::thread::spawn(move || {
            while !wait_started.load(std::sync::atomic::Ordering::Acquire) {
                if Instant::now() >= release_deadline {
                    RELEASE_BUSY_HANDLER.store(true, std::sync::atomic::Ordering::Release);
                    return false;
                }
                std::thread::yield_now();
            }
            std::thread::sleep(Duration::from_millis(400));
            RELEASE_BUSY_HANDLER.store(true, std::sync::atomic::Ordering::Release);
            true
        });
        let result = store.with_conn_fenced_within(started + Duration::from_millis(1_200), |tx| {
            tx.execute("INSERT INTO kv (k, v) VALUES ('blocked', '1')", [])
        });
        let waited = started.elapsed();
        let wait_was_observed = release.join().expect("release timer thread");
        blocker.join().expect("pending holder thread");

        assert!(wait_was_observed, "the bounded wait hook did not run");
        assert!(matches!(result, Err(StoreError::Deadline)), "{result:?}");
        let synchronous: i64 = store
            .with_conn_unfenced(|conn| conn.query_row("PRAGMA synchronous", [], |row| row.get(0)))
            .expect("read synchronous");
        assert_eq!(
            synchronous, 2,
            "the fence read must succeed and reach the durability pin",
        );
        assert!(
            waited < Duration::from_millis(1_400),
            "the bounded write waited {waited:?} for a 1,200 ms deadline",
        );
        let timeout: i64 = store
            .with_conn_unfenced(|conn| conn.query_row("PRAGMA busy_timeout", [], |row| row.get(0)))
            .expect("read busy timeout");
        assert_eq!(timeout, 5_000, "the connection busy timeout is restored");

        drop(reader);
        let rows: i64 = store
            .with_conn(|conn| conn.query_row("SELECT COUNT(*) FROM kv", [], |row| row.get(0)))
            .expect("count");
        assert_eq!(rows, 0, "the refused write left no row");
        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cached_statements_run_under_the_callback_scope() {
        let (root, d) = tmp();
        let store = open_sqlite(&d, KV_BASELINE).expect("open");
        store
            .with_conn_unfenced(|c| {
                c.set_prepared_statement_cache_capacity(4);
                Ok(())
            })
            .expect("size the cache");
        store
            .with_conn_fenced(|tx| {
                tx.prepare_cached("INSERT INTO kv (k, v) VALUES (?1, ?2)")?
                    .execute(["a", "1"])?;
                tx.prepare_cached("INSERT INTO kv (k, v) VALUES (?1, ?2)")?
                    .execute(["b", "2"])
            })
            .expect("cached inserts inside the fenced write");
        let n: i64 = store
            .with_conn(|c| {
                c.prepare_cached("SELECT COUNT(*) FROM kv")?
                    .query_row([], |r| r.get(0))
            })
            .expect("cached read");
        assert_eq!(n, 2);
        // A cached write statement is still a write; `query_only` refuses it.
        let r = store.with_conn(|c| {
            c.prepare_cached("INSERT INTO kv (k, v) VALUES (?1, ?2)")?
                .execute(["c", "3"])
        });
        assert!(
            matches!(&r, Err(StoreError::Backend(m)) if m.contains("readonly")),
            "a cached write must be refused in the read scope, got {r:?}"
        );
        let r = store.with_conn(|c| {
            c.prepare_cached("PRAGMA journal_size_limit = 1")?
                .execute([])
        });
        assert!(
            matches!(&r, Err(StoreError::Backend(m)) if m.contains("not authorized")),
            "a cached pragma write must be denied in the read scope, got {r:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// SQLite counts a re-prepare on the statement handle whenever a step finds the
    /// statement expired or its schema cookie stale; the counter accumulates on the handle
    /// the cache keeps, so zero means the cached program has run only as first prepared.
    fn reprepare_count(statement: &rusqlite::Statement<'_>) -> i32 {
        statement.get_status(rusqlite::StatementStatus::RePrepare)
    }

    const CACHED_INSERT: &str = "INSERT INTO kv (k, v) VALUES (?1, ?2)";
    const CACHED_TEMP_SHADOW: &str = "CREATE TEMP TABLE late (x)";

    /// The mode gate installs nothing per call, so a statement cached by one fenced
    /// callback runs unchanged in the next. A schema change on another connection is
    /// still observed: the next callback snapshots the main-schema names at entry, and a
    /// stale schema cookie forces the cached statement to re-prepare against that policy.
    #[test]
    fn a_warm_fenced_statement_survives_across_store_calls_until_the_schema_changes() {
        let (root, d) = tmp();
        let path = sqlite_path(&d);
        let store = open_sqlite(&d, KV_BASELINE).expect("open");
        store
            .with_conn_fenced(|tx| tx.prepare_cached(CACHED_INSERT)?.execute(["a", "1"]))
            .expect("warm the statement");
        let (reprepared, after_temp_ddl) = store
            .with_conn_fenced(|tx| {
                let mut statement = tx.prepare_cached(CACHED_INSERT)?;
                statement.execute(["b", "2"])?;
                let reprepared = reprepare_count(&statement);
                // Warmed while `late` is not a main-schema name, so the guarded policy
                // allows it; dropped so the connection carries no temp object out. Temp
                // DDL expires every statement on the connection, so the insert's next run
                // re-prepares once here and the count after the foreign DDL must exceed it.
                tx.prepare_cached(CACHED_TEMP_SHADOW)?.execute([])?;
                tx.execute("DROP TABLE temp.late", [])?;
                statement.execute(["b2", "2"])?;
                Ok((reprepared, reprepare_count(&statement)))
            })
            .expect("the second fenced call");
        assert_eq!(
            reprepared, 0,
            "the warm statement ran in the second fenced call without a re-prepare \
             (bundled SQLite 3.51.3: `synchronous` and `journal_mode` emit no expiry)"
        );
        assert!(
            after_temp_ddl >= 1,
            "temp DDL expires statements, got {after_temp_ddl}"
        );

        // The store holds no transaction between callbacks, so the second connection's
        // DDL cannot contend. The file no longer matches the baseline after this, so the
        // store must not be reopened in this test.
        let raw = rusqlite::Connection::open(&path).expect("second connection");
        raw.execute_batch("CREATE TABLE late (x);")
            .expect("DDL on the second connection");
        drop(raw);

        let (reprepared, late_rows) = store
            .with_conn_fenced(|tx| {
                let mut statement = tx.prepare_cached(CACHED_INSERT)?;
                statement.execute(["c", "3"])?;
                let late_rows: i64 = tx.query_row("SELECT COUNT(*) FROM late", [], |r| r.get(0))?;
                Ok((reprepare_count(&statement), late_rows))
            })
            .expect("the fenced call after the foreign DDL");
        assert!(
            reprepared > after_temp_ddl,
            "the foreign DDL forced a re-prepare of the warm statement, got {reprepared} \
             after {after_temp_ddl}"
        );
        assert_eq!(
            late_rows, 0,
            "the callback reads the table the foreign DDL created"
        );
        let shadow = store.with_conn_fenced(|tx| {
            tx.prepare_cached(CACHED_TEMP_SHADOW)?
                .execute([])
                .map(|_| ())
        });
        assert!(
            matches!(&shadow, Err(StoreError::Backend(m)) if m.contains("not authorized")),
            "the cached temp-shadow statement is re-authorized against the new main-schema \
             names and denied, got {shadow:?}"
        );
        let rows: i64 = store
            .with_conn(|c| c.query_row("SELECT COUNT(*) FROM kv", [], |r| r.get(0)))
            .expect("count");
        assert_eq!(rows, 4);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Main-schema DDL on another connection bumps the header schema version, so the next
    /// callback rescans its snapshot: a temp shadow of the new name is denied and the old
    /// name is free. The temp scan is uncached, so a shadow that maintenance leaves is
    /// still refused.
    #[test]
    fn a_rename_through_a_second_connection_is_observed_by_the_next_callback() {
        let (root, d) = tmp();
        let path = sqlite_path(&d);
        let store = open_sqlite(&d, KV_BASELINE).expect("open");
        store
            .with_conn(|c| c.query_row("SELECT COUNT(*) FROM kv", [], |r| r.get::<_, i64>(0)))
            .expect("warm the snapshot");
        let denied = store.with_conn_fenced(|tx| tx.execute("CREATE TEMP TABLE kv (x)", []));
        assert!(
            matches!(&denied, Err(StoreError::Backend(m)) if m.contains("not authorized")),
            "a temp shadow of `kv` is denied before the rename, got {denied:?}"
        );

        let raw = rusqlite::Connection::open(&path).expect("second connection");
        raw.execute_batch("ALTER TABLE kv RENAME TO kv_renamed;")
            .expect("rename on the second connection");
        drop(raw);

        let denied =
            store.with_conn_fenced(|tx| tx.execute("CREATE TEMP TABLE kv_renamed (x)", []));
        assert!(
            matches!(&denied, Err(StoreError::Backend(m)) if m.contains("not authorized")),
            "the next callback denies a temp shadow of the renamed table, got {denied:?}"
        );
        store
            .with_conn_fenced(|tx| {
                tx.execute("CREATE TEMP TABLE kv (x)", [])?;
                tx.execute("DROP TABLE temp.kv", []).map(|_| ())
            })
            .expect("the old name no longer shadows anything after the rescan");

        store
            .with_conn_unfenced(|c| c.execute_batch("CREATE TEMP TABLE kv_renamed (x)"))
            .expect("maintenance leaves a shadow of the renamed table");
        let refused = store.with_conn(|c| c.query_row("SELECT 1", [], |r| r.get::<_, i64>(0)));
        assert!(
            matches!(&refused, Err(StoreError::Backend(m)) if m.contains("shadows")),
            "the uncached temp scan still refuses the shadow, got {refused:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The page-cache, temporary-store, and memory-map pragmas belong to the connection-open
    /// path; a guarded callback of either kind is denied them, and a denied pragma leaves
    /// the value the maintenance path set.
    #[test]
    fn guarded_callbacks_are_denied_the_connection_resource_pragmas() {
        let (root, d) = tmp();
        let store = open_sqlite(&d, KV_BASELINE).expect("open");
        store
            .with_conn_unfenced(|c| {
                c.pragma_update(None, "cache_size", -512)?;
                c.pragma_update(None, "temp_store", "MEMORY")?;
                c.pragma_update(None, "mmap_size", 0)
            })
            .expect("the open path owns the resource pragmas");
        for (pragma, value) in [
            ("cache_size", "-8"),
            ("temp_store", "FILE"),
            ("mmap_size", "65536"),
        ] {
            let read = store.with_conn(|c| c.execute(&format!("PRAGMA {pragma} = {value}"), []));
            assert!(
                matches!(&read, Err(StoreError::Backend(m)) if m.contains("not authorized")),
                "a read callback is denied `PRAGMA {pragma}`, got {read:?}"
            );
            let fenced =
                store.with_conn_fenced(|tx| tx.execute(&format!("PRAGMA {pragma} = {value}"), []));
            assert!(
                matches!(&fenced, Err(StoreError::Backend(m)) if m.contains("not authorized")),
                "a fenced callback is denied `PRAGMA {pragma}`, got {fenced:?}"
            );
        }
        let (cache_size, temp_store, mmap_size): (i64, i64, i64) = store
            .with_conn(|c| {
                Ok((
                    c.query_row("PRAGMA cache_size", [], |r| r.get(0))?,
                    c.query_row("PRAGMA temp_store", [], |r| r.get(0))?,
                    c.query_row("PRAGMA mmap_size", [], |r| r.get(0))?,
                ))
            })
            .expect("reading the pragmas stays allowed");
        assert_eq!((cache_size, temp_store, mmap_size), (-512, 2, 0));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A handle with no runs handed out after a returned handle of its key had run means
    /// the cache re-created it. A cache sized below the working set churns, a cache sized
    /// for it does not.
    #[test]
    fn statement_evictions_are_counted_per_text() {
        let sequence = [
            "SELECT 1", "SELECT 2", "SELECT 1", "SELECT 3", "SELECT 2", "SELECT 1",
        ];
        let run_sequence = |store: &SqliteStore| {
            store
                .with_conn_fenced(|tx| {
                    for text in sequence {
                        tx.prepare_cached(text)?
                            .query_row([], |r| r.get::<_, i64>(0))?;
                    }
                    Ok(())
                })
                .expect("run the sequence")
        };
        let size = |store: &SqliteStore, capacity: usize| {
            store
                .with_conn_unfenced(|c| {
                    c.set_prepared_statement_cache_capacity(capacity);
                    Ok(())
                })
                .expect("size the cache")
        };

        let (root, d) = tmp();
        let store = open_sqlite(&d, KV_BASELINE).expect("open");
        size(&store, 2);
        store.start_statement_reuse_probe();
        run_sequence(&store);
        let churned = store.statement_evictions();
        // Capacity two under the sequence: `SELECT 1` is evicted by `SELECT 3` and
        // re-created at the end; `SELECT 2` is evicted by the second `SELECT 1` and
        // re-created before its second run.
        assert_eq!(
            churned,
            std::collections::BTreeMap::from([
                ("SELECT 1".to_string(), 1),
                ("SELECT 2".to_string(), 1),
                ("SELECT 3".to_string(), 0),
            ]),
            "the undersized cache re-created every revisited statement once"
        );
        drop(store);
        let _ = std::fs::remove_dir_all(&root);

        let (root, d) = tmp();
        let store = open_sqlite(&d, KV_BASELINE).expect("open");
        size(&store, 3);
        store.start_statement_reuse_probe();
        run_sequence(&store);
        run_sequence(&store);
        let fitted = store.statement_evictions();
        assert_eq!(fitted.len(), 3);
        assert!(
            fitted.values().all(|evictions| *evictions == 0),
            "a fitted cache re-creates no handle, got {fitted:?}"
        );
        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The read path's `query_only` toggle is a flag pragma; SQLite expires every prepared
    /// statement on the connection for each flag pragma, so a read callback re-prepares
    /// what it cached and stales what fenced callbacks cached. Removing the toggle is the
    /// open question of the connection-open unit; this pins the behavior it would change.
    #[test]
    fn a_read_callback_still_expires_cached_statements_through_query_only() {
        let (root, d) = tmp();
        let store = open_sqlite(&d, KV_BASELINE).expect("open");
        const COUNT: &str = "SELECT COUNT(*) FROM kv";
        let count_reprepares = |c: &GuardedConn<'_>| -> rusqlite::Result<i32> {
            let mut statement = c.prepare_cached(COUNT)?;
            statement.query_row([], |r| r.get::<_, i64>(0))?;
            Ok(reprepare_count(&statement))
        };
        let observed = [
            store.with_conn(count_reprepares).expect("read"),
            store.with_conn(count_reprepares).expect("read"),
            store.with_conn_fenced(count_reprepares).expect("fenced"),
            store.with_conn_fenced(count_reprepares).expect("fenced"),
        ];
        assert_eq!(observed[0], 0, "first use of the statement");
        assert!(
            observed[1] >= 1,
            "the read toggle expired it, got {observed:?}"
        );
        assert!(
            observed[2] > observed[1],
            "the read release expired it, got {observed:?}"
        );
        assert_eq!(
            observed[3], observed[2],
            "two fenced calls in a row re-prepare nothing, got {observed:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `query_only` is the read path's only write barrier and applies to every database on
    /// the connection, so temp writes are refused in a read callback and allowed in a
    /// fenced one. Removing the toggle would change this; the connection-open unit owns
    /// that question.
    #[test]
    fn a_read_callback_cannot_write_the_temp_database_but_a_fenced_callback_can() {
        let (root, d) = tmp();
        let store = open_sqlite(&d, KV_BASELINE).expect("open");
        let refused = store.with_conn(|c| c.execute("CREATE TEMP TABLE scratch (x)", []));
        assert!(
            matches!(&refused, Err(StoreError::Backend(m)) if m.contains("readonly")),
            "temp DDL must be refused in a read callback, got {refused:?}"
        );
        store
            .with_conn_unfenced(|c| c.execute_batch("CREATE TEMP TABLE scratch (x)"))
            .expect("maintenance creates a temp table that shadows nothing");
        let refused = store.with_conn(|c| c.execute("INSERT INTO scratch VALUES (1)", []));
        assert!(
            matches!(&refused, Err(StoreError::Backend(m)) if m.contains("readonly")),
            "a temp write must be refused in a read callback, got {refused:?}"
        );
        store
            .with_conn_fenced(|tx| {
                tx.execute("INSERT INTO scratch VALUES (1)", [])?;
                tx.execute("DROP TABLE temp.scratch", []).map(|_| ())
            })
            .expect("a fenced callback may write the temp database");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A stuck guarded mode would deny maintenance its pragma writes, so a successful
    /// lowering after each panic shows the mode returned to the rest state; the fenced
    /// write then re-pins durability as it does after any maintenance.
    #[test]
    fn a_panicking_callback_restores_the_unrestricted_mode() {
        let (root, d) = tmp();
        let store = open_sqlite(&d, KV_BASELINE).expect("open");

        let read_panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            store.with_conn(|c| -> rusqlite::Result<()> {
                c.query_row("SELECT COUNT(*) FROM kv", [], |r| r.get::<_, i64>(0))?;
                panic!("read callback panics after a statement")
            })
        }));
        assert!(read_panicked.is_err());
        let query_only: bool = store
            .with_conn_unfenced(|c| c.query_row("PRAGMA query_only", [], |r| r.get(0)))
            .expect("read the pragma");
        assert!(!query_only, "the panicking read restored query_only");
        store
            .with_conn_unfenced(|c| c.pragma_update(None, "synchronous", "OFF"))
            .expect("maintenance is unrestricted again after a panicking read");

        let write_panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            store.with_conn_fenced(|tx| -> rusqlite::Result<()> {
                tx.execute("INSERT INTO kv (k, v) VALUES ('partial', '1')", [])?;
                panic!("write callback panics after a statement")
            })
        }));
        assert!(write_panicked.is_err());
        store
            .with_conn_unfenced(|c| c.pragma_update(None, "synchronous", "OFF"))
            .expect("maintenance is unrestricted again after a panicking fenced write");
        let partial: i64 = store
            .with_conn(|c| {
                c.query_row("SELECT COUNT(*) FROM kv WHERE k = 'partial'", [], |r| {
                    r.get(0)
                })
            })
            .expect("count");
        assert_eq!(partial, 0, "the panicking fenced callback rolled back");

        store
            .with_conn_fenced(|tx| {
                tx.execute("INSERT INTO kv (k, v) VALUES ('after', '1')", [])
                    .map(|_| ())
            })
            .expect("fenced write");
        let synchronous: i64 = store
            .with_conn(|c| c.query_row("PRAGMA synchronous", [], |r| r.get(0)))
            .expect("read synchronous");
        assert_eq!(
            synchronous, 2,
            "the fenced write re-pinned synchronous=FULL"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The scratch check in `for_baseline` refuses these texts first; this test bypasses
    /// it so the store connection's own gate is the refusing party, and shows the refusal
    /// leaves the file pristine for a later good open.
    #[test]
    fn baseline_application_is_denied_its_escapes_on_the_store_connection() {
        let (root, d) = tmp();
        let path = sqlite_path(&d);
        std::fs::create_dir_all(&root).expect("store directory");
        for escape in [
            "PRAGMA writable_schema = ON;",
            "ATTACH DATABASE ':memory:' AS other;",
            "BEGIN;",
            "SAVEPOINT s;",
            "INSERT INTO fence (id, epoch) VALUES (0, 999);",
            "DELETE FROM format_marker;",
        ] {
            let text = format!("{KV_BASELINE}\n{escape}");
            let expected = ExpectedIdentity::unchecked_for_test(&text).expect("unchecked identity");
            let result = open_claimed(&path, &expected, None, 1);
            assert!(
                matches!(&result, Err(StoreError::Backend(m)) if m.contains("not authorized")),
                "`{escape}` must be denied by the baseline mode, got {:?}",
                result.map(|_| ())
            );
        }
        let benign = ExpectedIdentity::unchecked_for_test(KV_BASELINE).expect("identity");
        let ClaimedConnection { conn, .. } =
            open_claimed(&path, &benign, None, 1).expect("the refusals left the file pristine");
        let epoch: i64 = conn
            .query_row("SELECT epoch FROM fence WHERE id = 0", [], |row| row.get(0))
            .expect("the benign baseline applied and the fence was claimed");
        assert_eq!(epoch, 1);
        drop(conn);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The fence statements the store issues under the unrestricted mode never enter the
    /// statement cache: a cached statement is not re-authorized on reuse, so a cached fence
    /// upsert would let a guarded callback rewrite the epoch. A statement retrieved from
    /// the cache keeps its run count; zero runs means the store never cached it.
    #[test]
    fn the_store_never_caches_its_own_fence_statements() {
        let (root, d) = tmp();
        let path = sqlite_path(&d);
        std::fs::create_dir_all(&root).expect("store directory");
        let expected = ExpectedIdentity::for_baseline(KV_BASELINE).expect("identity");
        let ClaimedConnection { conn, .. } = open_claimed(&path, &expected, None, 1).expect("open");
        for sql in [
            super::sqlite_backend::FENCE_CLAIM_SQL,
            "SELECT epoch FROM fence WHERE id = 0",
            "PRAGMA synchronous = FULL",
        ] {
            let statement = conn.prepare_cached(sql).expect("prepare");
            assert_eq!(
                statement.get_status(rusqlite::StatementStatus::Run),
                0,
                "`{sql}` was found in the statement cache with prior runs"
            );
        }
        drop(conn);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_read_callback_observes_one_snapshot_across_its_statements() {
        let (root, d) = tmp();
        let path = sqlite_path(&d);
        let store = open_sqlite(&d, KV_BASELINE).expect("open");
        store
            .with_conn_fenced(|tx| tx.execute("INSERT INTO kv (k, v) VALUES ('a', '1')", []))
            .expect("seed");
        let mut raw = rusqlite::Connection::open(&path).expect("raw connection");
        raw.pragma_update(None, "busy_timeout", 5_000)
            .expect("busy timeout");
        // Under a rollback journal the raw writer would block on the reader's shared lock
        // until `busy_timeout`; WAL lets it commit while the reader keeps its snapshot.
        let mode: String = store
            .with_conn(|c| c.query_row("PRAGMA journal_mode", [], |r| r.get(0)))
            .expect("journal mode");
        assert!(mode.eq_ignore_ascii_case("wal"), "journal_mode is {mode}");
        let (first, second): (String, String) = store
            .with_conn(|c| {
                let first: String =
                    c.query_row("SELECT v FROM kv WHERE k = 'a'", [], |r| r.get(0))?;
                // A separate connection changes the row between the two reads, so equal
                // values prove the callback reads one snapshot.
                let tx = raw.transaction().expect("raw transaction");
                tx.execute("UPDATE kv SET v = '2' WHERE k = 'a'", [])
                    .expect("raw update");
                tx.commit().expect("raw commit");
                let second: String =
                    c.query_row("SELECT v FROM kv WHERE k = 'a'", [], |r| r.get(0))?;
                Ok((first, second))
            })
            .expect("read callback");
        assert_eq!((first.as_str(), second.as_str()), ("1", "1"));
        let after: String = store
            .with_conn(|c| c.query_row("SELECT v FROM kv WHERE k = 'a'", [], |r| r.get(0)))
            .expect("next callback sees the commit");
        assert_eq!(after, "2");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_read_callback_cannot_end_its_own_transaction() {
        let (root, d) = tmp();
        let store = open_sqlite(&d, KV_BASELINE).expect("open");
        store
            .with_conn_fenced(|tx| tx.execute("INSERT INTO kv (k, v) VALUES ('a', '1')", []))
            .expect("seed");
        let (first, denials, second): (String, Vec<String>, String) = store
            .with_conn(|c| {
                let first: String =
                    c.query_row("SELECT v FROM kv WHERE k = 'a'", [], |r| r.get(0))?;
                let denials = ["COMMIT", "ROLLBACK"]
                    .iter()
                    .map(|control| match c.execute(control, []) {
                        Ok(_) => format!("{control} was allowed"),
                        Err(e) => e.to_string(),
                    })
                    .collect();
                let second: String =
                    c.query_row("SELECT v FROM kv WHERE k = 'a'", [], |r| r.get(0))?;
                Ok((first, denials, second))
            })
            .expect("the callback continues after the denied controls");
        for denial in &denials {
            assert!(
                denial.contains("not authorized"),
                "transaction control must be denied in the read scope, got {denial}"
            );
        }
        assert_eq!((first.as_str(), second.as_str()), ("1", "1"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn guarded_callbacks_cannot_attach_or_detach_databases() {
        let (root, d) = tmp();
        let store = open_sqlite(&d, KV_BASELINE).expect("open");
        let r = store.with_conn(|c| c.execute("ATTACH DATABASE ':memory:' AS side", []));
        assert!(
            matches!(&r, Err(StoreError::Backend(m)) if m.contains("not authorized")),
            "ATTACH must be denied by the read callback scope, got {r:?}"
        );
        let r = store.with_conn_fenced(|tx| tx.execute("ATTACH DATABASE ':memory:' AS side", []));
        assert!(
            matches!(&r, Err(StoreError::Backend(m)) if m.contains("not authorized")),
            "ATTACH must be denied by the fenced callback scope, got {r:?}"
        );
        store
            .with_conn_unfenced(|c| c.execute_batch("ATTACH DATABASE ':memory:' AS side"))
            .expect("attach through the maintenance path");
        let r = store.with_conn(|c| c.execute("DETACH DATABASE side", []));
        assert!(
            matches!(&r, Err(StoreError::Backend(m)) if m.contains("not authorized")),
            "DETACH must be denied by the read callback scope, got {r:?}"
        );
        let r = store.with_conn_fenced(|tx| tx.execute("DETACH DATABASE side", []));
        assert!(
            matches!(&r, Err(StoreError::Backend(m)) if m.contains("not authorized")),
            "DETACH must be denied by the fenced callback scope, got {r:?}"
        );
        store
            .with_conn_unfenced(|c| c.execute_batch("DETACH DATABASE side"))
            .expect("detach through the maintenance path");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn schema_introspection_pragmas_pass_the_callback_scope_while_setting_pragmas_does_not() {
        let (root, d) = tmp();
        let store = open_sqlite(
            &d,
            "CREATE TABLE kv (k TEXT PRIMARY KEY, v TEXT NOT NULL);\
             CREATE INDEX kv_v ON kv (v);\
             CREATE TABLE child (id INTEGER PRIMARY KEY, k TEXT REFERENCES kv (k));",
        )
        .expect("open");
        let has_k: bool = store
            .with_conn(|c| {
                c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('kv') WHERE name = 'k')",
                    [],
                    |r| r.get(0),
                )
            })
            .expect("table-valued introspection is a read");
        assert!(has_k);
        // The allowlist accepts mixed-case pragma names such as `Table_XInfo`.
        let pragmas = [
            ("PRAGMA table_info(kv)", 2),
            ("PRAGMA Table_XInfo(kv)", 2),
            ("PRAGMA table_list(kv)", 1),
            ("PRAGMA index_list(kv)", 2),
            ("PRAGMA index_info(kv_v)", 1),
            ("PRAGMA index_xinfo(kv_v)", 2),
            ("PRAGMA foreign_key_list(child)", 1),
        ];
        for (sql, expected_rows) in pragmas {
            let count = |c: &GuardedConn<'_>| -> rusqlite::Result<usize> {
                let mut statement = c.prepare(sql)?;
                let rows = statement.query_map([], |_| Ok(()))?;
                Ok(rows.count())
            };
            let in_read = store
                .with_conn(count)
                .unwrap_or_else(|e| panic!("{sql} must pass the read scope: {e:?}"));
            assert_eq!(in_read, expected_rows, "{sql} in the read scope");
            let in_fenced = store
                .with_conn_fenced(count)
                .unwrap_or_else(|e| panic!("{sql} must pass the fenced scope: {e:?}"));
            assert_eq!(in_fenced, expected_rows, "{sql} in the fenced scope");
        }
        for sql in [
            "PRAGMA journal_size_limit = 1",
            "PRAGMA writable_schema = ON",
            "PRAGMA ignore_check_constraints = ON",
        ] {
            let r = store.with_conn(|c| c.execute(sql, []));
            assert!(
                matches!(&r, Err(StoreError::Backend(m)) if m.contains("not authorized")),
                "{sql} must be denied in the read scope, got {r:?}"
            );
            let r = store.with_conn_fenced(|tx| tx.execute(sql, []));
            assert!(
                matches!(&r, Err(StoreError::Backend(m)) if m.contains("not authorized")),
                "{sql} must be denied in the fenced scope, got {r:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn fenced_write_rolls_back_on_error() {
        let (root, d) = tmp();
        let path = sqlite_path(&d);
        drop(open_sqlite(&d, KV_BASELINE).expect("open"));
        let store = SqliteStore::for_test(rusqlite::Connection::open(path).unwrap(), 2);
        let r: Result<(), StoreError> = store.with_conn_fenced(|tx| {
            tx.execute("INSERT INTO kv (k, v) VALUES ('a', '1')", [])?;
            tx.query_row("SELECT * FROM does_not_exist", [], |_| Ok(()))?;
            Ok(())
        });
        assert!(
            matches!(r, Err(StoreError::Backend(_))),
            "closure error surfaces"
        );
        let n: i64 = store
            .with_conn(|c| c.query_row("SELECT COUNT(*) FROM kv", [], |r| r.get(0)))
            .expect("count");
        assert_eq!(n, 0, "the failed fenced write rolled back");
        let claimed: i64 = store
            .with_conn(|c| c.query_row("SELECT epoch FROM fence WHERE id = 0", [], |r| r.get(0)))
            .expect("read fence");
        assert_eq!(
            claimed, 1,
            "the failed callback did not roll back its fence claim"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn negative_database_fence_fails_closed() {
        let (root, d) = tmp();
        let path = sqlite_path(&d);
        drop(open_sqlite(&d, "").expect("seed database"));

        let conn = rusqlite::Connection::open(&path).expect("reopen raw");
        conn.execute_batch(
            "PRAGMA ignore_check_constraints = ON; \
             UPDATE fence SET epoch = -1 WHERE id = 0;",
        )
        .expect("corrupt the fence through the constraint bypass");
        drop(conn);

        let error = match open_sqlite(&d, "") {
            Err(error) => error,
            Ok(_) => panic!("negative fence must fail closed"),
        };
        assert!(
            matches!(error, StoreError::FenceCorrupt { db_epoch } if db_epoch == -1),
            "expected FenceCorrupt, got {error:?}"
        );
        let persisted: i64 = rusqlite::Connection::open(&path)
            .expect("reopen database")
            .query_row("SELECT epoch FROM fence WHERE id = 0", [], |row| row.get(0))
            .expect("read unchanged negative fence");
        assert_eq!(persisted, -1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn epoch_above_sqlite_integer_range_fails() {
        let (root, d) = tmp();
        let path = sqlite_path(&d);
        drop(open_sqlite(&d, "").expect("seed database"));
        let too_large = SqliteStore::for_test(
            rusqlite::Connection::open(&path).unwrap(),
            (i64::MAX as u64) + 1,
        );
        let error = too_large
            .with_conn_fenced(|_| Ok(()))
            .expect_err("epochs above SQLite INTEGER range must fail");
        assert!(
            matches!(error, StoreError::Backend(message) if message.contains("exceeds SQLite INTEGER maximum"))
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
