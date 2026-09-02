//! Backend mechanics for module storage: open a database from a
//! [`StorageDescriptor`], guard it with the single-writer lease, and apply
//! versioned migrations once.
//!
//! Modules pass a resolved descriptor and ordered migrations here, then run domain
//! queries against the lease-guarded, migrated connection. Backends are
//! feature-gated, so module code does not branch on the descriptor's backend. Each
//! module owns its store trait, migrations, and queries.
//!
//! The single-writer lease ([`lease`]) is keyed by
//! `(module_id, backend, storage_namespace)`, preventing collisions between stores
//! that share a lease root. The persisted epoch serves as the fence token for
//! epoch-checked writes.

pub use storage_types::{
    Isolation, Migration, StorageBackend, StorageDescriptor, postgres_database_name,
    sqlite_store_path,
};

use lease::LeaseError;
#[cfg(feature = "sqlite")]
use lease::LeaseKey;

#[derive(Debug)]
pub enum StoreError {
    /// A conflicting live holder prevented acquisition, or lease I/O failed.
    Lease(LeaseError),
    /// The descriptor asked for a backend this build was not compiled with.
    UnsupportedBackend(String),
    /// A migration or schema-version operation failed.
    Migration(String),
    /// A backend (database driver) operation failed.
    Backend(String),
    /// An io failure preparing the store location.
    Io(std::io::Error),
    /// A fenced (epoch-checked) write was rejected because the database has already
    /// been claimed by a newer writer. `db_epoch` (the epoch stamped in the
    /// database) is greater than `holder_epoch` (this store's lease epoch), so this
    /// writer has been superseded — for example a draining old instance attempting a
    /// late write after a replacement took the lease. The write was not applied.
    Fenced { holder_epoch: u64, db_epoch: u64 },
    /// An out-of-range database epoch prevents proving monotonic fencing. The store
    /// refuses to open until an operator resets `cortexkit_fence.epoch`.
    FenceCorrupt { db_epoch: i64 },
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Lease(e) => write!(f, "storage lease: {e}"),
            StoreError::UnsupportedBackend(b) => write!(
                f,
                "storage backend '{b}' is not supported by this build (missing feature)"
            ),
            StoreError::Migration(m) => write!(f, "migration: {m}"),
            StoreError::Backend(m) => write!(f, "storage backend: {m}"),
            StoreError::Io(e) => write!(f, "storage io: {e}"),
            StoreError::Fenced {
                holder_epoch,
                db_epoch,
            } => write!(
                f,
                "fenced write rejected: this writer holds epoch {holder_epoch} but the \
                 database was claimed by a newer writer at epoch {db_epoch}"
            ),
            StoreError::FenceCorrupt { db_epoch } => write!(
                f,
                "database fence epoch {db_epoch} is outside the supported range; reset \
                 cortexkit_fence.epoch to at least the highest epoch a writer has used"
            ),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StoreError::Lease(e) => Some(e),
            StoreError::Io(e) => Some(e),
            StoreError::UnsupportedBackend(_)
            | StoreError::Migration(_)
            | StoreError::Backend(_)
            | StoreError::Fenced { .. }
            | StoreError::FenceCorrupt { .. } => None,
        }
    }
}

/// The lease key includes the module, backend, storage namespace, and the
/// database file name. The lease root is the database's parent directory, so
/// two distinct database files in one directory need distinct keys or they
/// falsely contend. File names cannot contain `/`, so the `namespace/file`
/// join is unambiguous.
#[cfg(feature = "sqlite")]
fn lease_key(descriptor: &StorageDescriptor, db_file_name: &str) -> LeaseKey {
    LeaseKey::new(
        &descriptor.module_id,
        descriptor.backend.label(),
        format!("{}/{}", descriptor.storage_namespace, db_file_name),
    )
}

#[cfg(feature = "sqlite")]
mod sqlite_backend {
    use super::*;
    use std::{
        path::{Path, PathBuf},
        sync::Mutex,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use lease::{FileLeaseStore, HeldFileLease, protect_file};
    use rusqlite::{Connection, OpenFlags};

    /// A lease-guarded SQLite store. The lease remains held for the store's lifetime.
    /// A single mutexed connection preserves connection-local configuration and transaction scope.
    /// [`open_sqlite`] claims the database lease epoch before returning the store.
    pub struct SqliteStore {
        conn: Mutex<Connection>,
        epoch: u64,
        // Declared after `conn` so the connection closes before the lease unlocks;
        // `None` is reserved for `for_test`.
        _lease: Option<HeldFileLease>,
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
            SqliteStore {
                conn: Mutex::new(conn),
                epoch,
                _lease: None,
            }
        }

        /// `with_conn` permits read-only queries and connection-local configuration.
        /// `PRAGMA query_only` makes database writes fail with `SQLITE_READONLY`,
        /// which keeps every durable write on the fenced path
        /// ([`Self::with_conn_fenced`]). The callback receives a [`GuardedConn`] rather
        /// than the connection, so it cannot replace the guard, set pragmas, run
        /// statement batches, or control transactions. Statements that reach the database
        /// are additionally checked: pragma writes, transaction control, savepoints,
        /// `ATTACH`/`DETACH`, and writes to the fence and version tables are denied.
        ///
        /// # Errors
        ///
        /// Returns [`StoreError::Backend`] if the callback returns an error, attempts a
        /// write or a denied statement, or if the scope cannot be installed or released.
        pub fn with_conn<T>(
            &self,
            f: impl FnOnce(&GuardedConn<'_>) -> rusqlite::Result<T>,
        ) -> Result<T, StoreError> {
            let guard = self.conn.lock().unwrap_or_else(|p| p.into_inner());
            let scope = CallbackScope::read_only(&guard)?;
            let out = f(&GuardedConn::new(&guard));
            let restored = scope.release();
            let out = out.map_err(|e| StoreError::Backend(e.to_string()))?;
            restored?;
            Ok(out)
        }

        /// [`Self::with_conn`]'s read-only guard rejects `VACUUM` as a write,
        /// and SQLite rejects it inside [`Self::with_conn_fenced`]'s
        /// transaction, so maintenance statements run here on the
        /// lease-holding connection. Fence-protected durable mutations belong
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
            let guard = self.conn.lock().unwrap_or_else(|p| p.into_inner());
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
        /// and version tables are denied for its duration, so no statement of its can
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
            let mut guard = self.conn.lock().unwrap_or_else(|p| p.into_inner());
            pin_fence_durability(&guard)?;
            let tx = guard
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(|e| StoreError::Backend(e.to_string()))?;

            claim_fence(&tx, self.epoch)?;

            let scope = CallbackScope::writable(&tx)?;
            let out = f(&GuardedConn::new(&tx));
            // The callback error outranks a release error: losing the reason the
            // write failed is worse than losing the scope-release failure.
            let released = scope.release();
            let out = out.map_err(|e| StoreError::Backend(e.to_string()))?;
            released?;
            tx.commit()
                .map_err(|e| StoreError::Backend(e.to_string()))?;
            Ok(out)
        }

        /// Applies a `namespace`'s migration chain using its recorded maximum as a
        /// watermark.
        ///
        /// Each namespace has an independent migration history. Versions at or
        /// below its watermark are silently skipped.
        /// Every transaction checks the persisted fence before executing schema
        /// changes.
        ///
        /// # Errors
        ///
        /// Returns [`StoreError::Fenced`] when a newer writer owns the database.
        /// Returns [`StoreError::Backend`] when the fence check fails.
        /// Returns [`StoreError::Migration`] if migration setup, SQL execution, recording, or commit fails.
        pub fn migrate(&self, namespace: &str, migrations: &[Migration]) -> Result<(), StoreError> {
            let mut guard = self.conn.lock().unwrap_or_else(|p| p.into_inner());
            run_migrations(&mut guard, self.epoch, namespace, migrations)
        }
    }

    /// A callback that holds `&Connection` can call `Connection::authorizer` and replace
    /// the guard installed for it, because that method takes `&self`. No authorizer,
    /// pragma, or statement rule can survive that, so a guarded callback receives this
    /// handle instead: it forwards ordinary statements and omits authorizer control,
    /// pragma writes, statement batches, and transaction control.
    pub struct GuardedConn<'a> {
        conn: &'a Connection,
    }

    /// [`SqliteStore::with_conn_unfenced`] must reach pragmas and statement batches, but
    /// an authorizer installed through it would be cleared by the next guarded callback's
    /// scope, silently dropping the caller's access policy. Authorizer control therefore
    /// stays out of both handles, which leaves the store the only installer.
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
    }

    impl<'a> GuardedConn<'a> {
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
        /// Returns the SQLite error from preparing `sql`.
        pub fn prepare(&self, sql: &str) -> rusqlite::Result<rusqlite::Statement<'a>> {
            self.conn.prepare(sql)
        }

        pub fn last_insert_rowid(&self) -> i64 {
            self.conn.last_insert_rowid()
        }

        pub fn changes(&self) -> u64 {
            self.conn.changes()
        }
    }

    /// The connection is shared by reads, fenced writes, migrations, and maintenance, so
    /// a callback that keeps the full capability of the connection can leave the scope it
    /// was given: any pragma write reconfigures every later statement, and transaction
    /// control ends the transaction whose fence check authorized the callback.
    /// `CallbackScope` withdraws those capabilities for the callback's duration, and
    /// restores them even when the callback unwinds, because a poisoned lock is recovered
    /// and hands the same connection to the next caller.
    struct CallbackScope<'c> {
        /// `None` once released, so `Drop` never repeats a release whose failure
        /// [`Self::release`] already reported.
        conn: Option<&'c Connection>,
        /// Reads additionally run under `query_only`; a fenced transaction must write.
        read_only: bool,
    }

    impl<'c> CallbackScope<'c> {
        /// Denies writes as well as the escapes, for a callback that must not mutate.
        fn read_only(conn: &'c Connection) -> Result<Self, StoreError> {
            conn.pragma_update(None, "query_only", "ON")
                .map_err(|e| StoreError::Backend(e.to_string()))?;
            Self::install(conn, true)
        }

        /// Denies the escapes only, for a callback inside a fence-checked transaction.
        fn writable(conn: &'c Connection) -> Result<Self, StoreError> {
            Self::install(conn, false)
        }

        fn install(conn: &'c Connection, read_only: bool) -> Result<Self, StoreError> {
            let scope = Self {
                conn: Some(conn),
                read_only,
            };
            conn.authorizer(Some(deny_scope_escapes))
                .map_err(|e| StoreError::Backend(e.to_string()))?;
            Ok(scope)
        }

        /// Reports a failed release to the caller, which `Drop` cannot do.
        fn release(mut self) -> Result<(), StoreError> {
            match self.conn.take() {
                Some(conn) => {
                    let shadowed = Self::require_no_shadow(conn);
                    Self::restore(conn, self.read_only).and(shadowed)
                }
                None => Ok(()),
            }
        }

        /// `AuthAction::AlterTable` reports the source name, so a rename cannot be judged
        /// when it is authorized: a benign temporary table renamed to an infrastructure
        /// name shadows the real one on this connection, and a later fence claim would
        /// read the callback's own epoch. The name is checked once the callback returns,
        /// before its transaction commits.
        fn require_no_shadow(conn: &Connection) -> Result<(), StoreError> {
            let shadow: Option<String> = conn
                .query_row(
                    "SELECT name FROM temp.sqlite_schema \
                     WHERE lower(name) IN ('cortexkit_fence', 'cortexkit_schema_version') \
                     LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .or_else(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(other),
                })
                .map_err(|e| StoreError::Backend(e.to_string()))?;
            match shadow {
                Some(name) => Err(StoreError::Backend(format!(
                    "a temporary `{name}` shadows the infrastructure table of the same name"
                ))),
                None => Ok(()),
            }
        }

        fn restore(conn: &Connection, read_only: bool) -> Result<(), StoreError> {
            let cleared = conn.authorizer(
                None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>,
            );
            let unlocked = if read_only {
                conn.pragma_update(None, "query_only", "OFF")
            } else {
                Ok(())
            };
            cleared.and(unlocked).map_err(|e| {
                StoreError::Backend(format!("failed to release the callback scope: {e}"))
            })
        }
    }

    impl Drop for CallbackScope<'_> {
        fn drop(&mut self) {
            if let Some(conn) = self.conn.take() {
                // Drop ignores cleanup errors because it cannot return them.
                let _ = Self::restore(conn, self.read_only);
            }
        }
    }

    /// Enumerating individual pragma names cannot be complete: `ignore_check_constraints`
    /// disables the fence table's constraint, `defer_foreign_keys` and `writable_schema`
    /// reach schema invariants, and pragma names are case-insensitive. Denying the whole
    /// capability class avoids that race. A pragma read carries no value and stays
    /// allowed, as do the ordinary statements a callback exists to run.
    fn deny_scope_escapes(
        context: rusqlite::hooks::AuthContext<'_>,
    ) -> rusqlite::hooks::Authorization {
        use rusqlite::hooks::{AuthAction, Authorization};
        match context.action {
            AuthAction::Pragma {
                pragma_value: Some(_),
                ..
            }
            | AuthAction::Transaction { .. }
            | AuthAction::Savepoint { .. }
            | AuthAction::Attach { .. }
            | AuthAction::Detach { .. } => Authorization::Deny,
            action => match infrastructure_target(&action) {
                Some(_) => Authorization::Deny,
                None => Authorization::Allow,
            },
        }
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
            // forged `cortexkit_fence` would let a stale writer read its own epoch.
            AuthAction::CreateView { view_name }
            | AuthAction::CreateTempView { view_name }
            | AuthAction::DropView { view_name }
            | AuthAction::DropTempView { view_name } => view_name,
            _ => return None,
        };
        is_infrastructure_table(table).then_some(table)
    }

    /// The fence row carries the authority a fenced write is checked against, and the
    /// version table records which migrations ran. A callback that changed either, or the
    /// schema reaching either, would let a superseded writer reclaim the database or
    /// re-run a migration.
    fn is_infrastructure_table(table_name: &str) -> bool {
        table_name.eq_ignore_ascii_case("cortexkit_fence")
            || table_name.eq_ignore_ascii_case("cortexkit_schema_version")
    }

    /// `with_conn_unfenced` remains unrestricted by contract, so `synchronous` and the
    /// journal mode can still be lowered between protected transactions. With WAL and
    /// `synchronous=NORMAL`, power loss can roll back committed transactions.
    fn pin_fence_durability(conn: &Connection) -> Result<(), StoreError> {
        conn.pragma_update(None, "synchronous", "FULL")
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        let mode: String = conn
            .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(StoreError::Backend(format!(
                "fenced writes require a crash-safe journal, but journal_mode is {mode}"
            )));
        }
        Ok(())
    }

    /// Open a module's SQLite store from its descriptor.
    ///
    /// The returned store has already claimed its lease epoch in the database.
    /// Call [`SqliteStore::migrate`] separately for each domain migration chain.
    ///
    /// The stored database fence becomes the lease floor. Deleting or restoring an
    /// old lease sidecar cannot reissue an epoch represented in the database.
    /// Databases created by older versions without a fence table use floor zero.
    ///
    /// The lease lives next to the database file (its parent directory), derived
    /// from the descriptor's path rather than passed in. This makes the
    /// one-lease-per-database invariant structural: two distinct database paths get
    /// distinct leases (correct isolation), and the same database path gets one
    /// lease (the single-writer guarantee). A caller cannot accidentally point a
    /// shared lease directory at several distinct databases (which would falsely
    /// make them contend) or split one database across lease directories (which
    /// would break single-writer).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::UnsupportedBackend`] for non-SQLite descriptors.
    /// Returns [`StoreError::Io`] when the parent directory cannot be created.
    /// Returns [`StoreError::Lease`] when lease acquisition fails.
    /// Returns [`StoreError::Fenced`] if the database advances during open.
    /// Returns [`StoreError::FenceCorrupt`] if the stored fence epoch is out of range.
    /// Returns [`StoreError::Backend`] when SQLite inspection, setup, or fence claim fails.
    pub fn open_sqlite(descriptor: &StorageDescriptor) -> Result<SqliteStore, StoreError> {
        let path = match &descriptor.backend {
            StorageBackend::Sqlite { path } => path.clone(),
            other => return Err(StoreError::UnsupportedBackend(other.label().to_string())),
        };

        let parent = Path::new(&path)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        std::fs::create_dir_all(&parent).map_err(StoreError::Io)?;

        let epoch_floor = read_fence_epoch(Path::new(&path))?;
        let db_file_name = Path::new(&path)
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());
        let lease = FileLeaseStore::new(&parent)
            .acquire_above(&lease_key(descriptor, &db_file_name), epoch_floor)
            .map_err(StoreError::Lease)?;
        let epoch = lease.epoch();

        let mut conn = Connection::open(&path).map_err(|e| StoreError::Backend(e.to_string()))?;
        // Owner-only permissions apply before the fence claim below writes any bytes.
        for suffix in ["", "-wal", "-shm"] {
            protect_file(Path::new(&format!("{path}{suffix}")))
                .map_err(|e| StoreError::Backend(e.to_string()))?;
        }
        // WAL permits concurrent readers. The busy timeout makes transient locks
        // wait rather than fail, and foreign-key enforcement is enabled.
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        // In WAL mode, `synchronous=NORMAL` may lose the most recent commits
        // after power loss, which would roll the persisted fence epoch backward.
        conn.pragma_update(None, "synchronous", "FULL")
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| StoreError::Backend(e.to_string()))?;

        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        ensure_fence_table(&tx)?;
        claim_fence_strict(&tx, epoch)?;
        tx.commit()
            .map_err(|e| StoreError::Backend(e.to_string()))?;

        Ok(SqliteStore {
            conn: Mutex::new(conn),
            epoch,
            _lease: Some(lease),
        })
    }

    fn read_fence_epoch(path: &Path) -> Result<u64, StoreError> {
        if !path.try_exists().map_err(StoreError::Io)? {
            return Ok(0);
        }
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        let has_fence: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'cortexkit_fence')",
                [],
                |row| row.get(0),
            )
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        if !has_fence {
            return Ok(0);
        }
        read_fence_epoch_in(&conn)
    }

    const FENCE_EPOCH_SQL: &str =
        "SELECT COALESCE((SELECT epoch FROM cortexkit_fence WHERE id = 0), 0)";

    /// The caller guarantees that `cortexkit_fence` exists.
    fn read_fence_epoch_in(conn: &Connection) -> Result<u64, StoreError> {
        let epoch: i64 = conn
            .query_row(FENCE_EPOCH_SQL, [], |row| row.get(0))
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        decode_fence_epoch(epoch)
    }

    /// Initializes fence storage before `SqliteStore` is exposed.
    fn ensure_fence_table(tx: &rusqlite::Transaction<'_>) -> Result<(), StoreError> {
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS cortexkit_fence (\
                 id INTEGER PRIMARY KEY CHECK (id = 0), \
                 epoch INTEGER NOT NULL CHECK (epoch >= 0))",
        )
        .map_err(|e| StoreError::Backend(e.to_string()))
    }

    /// Binds fence comparison and claim to the caller's protected transaction.
    ///
    /// An epoch equal to the stored epoch permits repeated writes.
    pub(crate) fn claim_fence(
        tx: &rusqlite::Transaction<'_>,
        holder_epoch: u64,
    ) -> Result<(), StoreError> {
        let holder_epoch_sql = fence_epoch_sql_value(holder_epoch)?;
        let db_epoch = read_fence_epoch_in(tx)?;

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
    ) -> Result<(), StoreError> {
        let holder_epoch_sql = fence_epoch_sql_value(holder_epoch)?;
        let db_epoch = read_fence_epoch_in(tx)?;

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
            .execute(
                "INSERT INTO cortexkit_fence (id, epoch) VALUES (0, ?1) \
                 ON CONFLICT(id) DO UPDATE SET epoch = excluded.epoch",
                rusqlite::params![holder_epoch_sql],
            )
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        // A trigger running `RAISE(IGNORE)` reports success with zero changed
        // rows; proceeding without the persisted epoch would break fencing.
        if rows != 1 {
            return Err(StoreError::Backend(format!(
                "fence epoch write affected {rows} rows; expected 1"
            )));
        }
        Ok(())
    }

    /// Rejects negative SQLite integers instead of wrapping them into writer epochs.
    fn decode_fence_epoch(epoch: i64) -> Result<u64, StoreError> {
        u64::try_from(epoch).map_err(|_| StoreError::FenceCorrupt { db_epoch: epoch })
    }

    /// Apply un-applied migrations for one `namespace` in ascending version order,
    /// each in its own transaction together with its version record, so a migration
    /// and the record that it ran commit atomically (a crash mid-migration leaves
    /// it un-recorded and it re-runs cleanly next open).
    ///
    /// Applied migrations are keyed by `(namespace, version)`, so independent
    /// domain chains in one database never collide or re-run each other.
    fn run_migrations(
        conn: &mut Connection,
        holder_epoch: u64,
        namespace: &str,
        migrations: &[Migration],
    ) -> Result<(), StoreError> {
        pin_fence_durability(conn)?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| StoreError::Migration(e.to_string()))?;
        claim_fence(&tx, holder_epoch)?;
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS cortexkit_schema_version (\
                 namespace TEXT NOT NULL, \
                 version INTEGER NOT NULL, \
                 applied_at_unix INTEGER NOT NULL, \
                 PRIMARY KEY (namespace, version)\
             )",
        )
        .map_err(|e| StoreError::Migration(e.to_string()))?;

        let current: u32 = tx
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM cortexkit_schema_version WHERE namespace = ?1",
                rusqlite::params![namespace],
                |r| r.get(0),
            )
            .map_err(|e| StoreError::Migration(e.to_string()))?;
        tx.commit()
            .map_err(|e| StoreError::Migration(e.to_string()))?;

        let mut ordered: Vec<&Migration> = migrations.iter().collect();
        ordered.sort_by_key(|m| m.version);
        if let Some(pair) = ordered.windows(2).find(|w| w[0].version == w[1].version) {
            return Err(StoreError::Migration(format!(
                "namespace '{namespace}' declares migration version {} more than once",
                pair[0].version
            )));
        }

        for m in ordered {
            if m.version <= current {
                continue;
            }
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(|e| StoreError::Migration(e.to_string()))?;
            claim_fence(&tx, holder_epoch)?;
            let scope = CallbackScope::writable(&tx)?;
            let applied = tx.execute_batch(m.statements);
            let released = scope.release();
            applied.map_err(|e| {
                StoreError::Migration(format!(
                    "namespace '{namespace}' migration {}: {e}",
                    m.version
                ))
            })?;
            released.map_err(|e| {
                StoreError::Migration(format!(
                    "namespace '{namespace}' migration {}: {e}",
                    m.version
                ))
            })?;
            tx.execute(
                "INSERT INTO cortexkit_schema_version (namespace, version, applied_at_unix) \
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![namespace, m.version, now_unix()],
            )
            .map_err(|e| StoreError::Migration(e.to_string()))?;
            tx.commit()
                .map_err(|e| StoreError::Migration(e.to_string()))?;
        }
        Ok(())
    }

    fn now_unix() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}

#[cfg(feature = "sqlite")]
pub use sqlite_backend::{GuardedConn, MaintenanceConn, SqliteStore, open_sqlite};

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use super::sqlite_backend::{claim_fence, claim_fence_strict};
    use super::*;

    /// Reopening covers pre-existing permissive files. A first open cannot test
    /// permissive WAL repair because a fresh WAL inherits the restricted database
    /// mode.
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

        {
            let store = open_sqlite(&descriptor).expect("first open");
            store
                .migrate(
                    "perm",
                    &[Migration {
                        version: 1,
                        statements: "CREATE TABLE t (k TEXT);",
                    }],
                )
                .expect("migrate");
        }

        std::fs::write(&wal, b"").expect("leave a WAL behind");
        std::fs::write(&shm, b"").expect("leave an SHM behind");

        for file in [&path, &wal, &shm] {
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o644))
                .expect("set permissive mode");
        }

        let store = open_sqlite(&descriptor).expect("reopen");

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

    /// Protection runs before the fence write. A reopen whose `-shm` path is a
    /// directory fails `protect_file`; the fence epoch must still hold the
    /// seeded value, proving no fence bytes were written before protection.
    #[cfg(unix)]
    #[test]
    fn protection_failure_aborts_open_before_the_fence_write() {
        let (root, descriptor) = tmp();
        let StorageBackend::Sqlite { path } = &descriptor.backend else {
            panic!("sqlite descriptor");
        };
        let path = path.clone();
        let seeded = open_sqlite(&descriptor).expect("seed").epoch();

        let shm = format!("{path}-shm");
        std::fs::create_dir(&shm).expect("plant a directory at the shm path");
        match open_sqlite(&descriptor) {
            Err(StoreError::Backend(_)) => {}
            Err(other) => panic!("protection failure must abort the open, got {other}"),
            Ok(_) => panic!("protection failure must abort the open, got a store"),
        }

        let conn = rusqlite::Connection::open(&path).expect("inspect fence");
        let epoch: i64 = conn
            .query_row("SELECT epoch FROM cortexkit_fence WHERE id = 0", [], |r| {
                r.get(0)
            })
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
        // Per-call atomic counter (not a clock) guarantees a unique dir even when
        // tests run in parallel and the clock resolution is coarse.
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

    const M1: &[Migration] = &[Migration {
        version: 1,
        statements: "CREATE TABLE facts (id INTEGER PRIMARY KEY, name TEXT NOT NULL); \
                     INSERT INTO facts (id, name) VALUES (1, 'seed-a'), (2, 'seed-b');",
    }];

    #[test]
    fn open_claims_fence_before_return() {
        let (root, d) = tmp();
        let store = open_sqlite(&d).expect("open");
        let claimed: i64 = store
            .with_conn(|c| {
                c.query_row("SELECT epoch FROM cortexkit_fence WHERE id = 0", [], |r| {
                    r.get(0)
                })
            })
            .expect("open claimed fence");
        assert_eq!(claimed as u64, store.epoch());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Models the interleaving where a floor read before lease acquisition goes stale:
    /// an opener issues the epoch the database already stores. `claim_fence` authorizes
    /// that equal epoch, which would place two holders on one epoch, so open uses
    /// `claim_fence_strict` instead.
    #[test]
    fn open_claim_rejects_an_epoch_the_database_already_stores() {
        let (root, d) = tmp();
        let path = sqlite_path(&d);
        open_sqlite(&d).expect("seed database");

        let mut conn = rusqlite::Connection::open(&path).expect("reopen database");
        let stored: u64 = conn
            .query_row(
                "SELECT epoch FROM cortexkit_fence WHERE id = 0",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|epoch| epoch as u64)
            .expect("stored fence");

        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .expect("claim transaction");
        match claim_fence_strict(&tx, stored) {
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
        claim_fence_strict(&tx, stored + 1).expect("a strictly greater epoch claims");
        drop(tx);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn migrations_seed_once_across_reopen() {
        let (root, d) = tmp();
        {
            let store = open_sqlite(&d).expect("open");
            store.migrate("facts", M1).expect("migrate");
            let n: i64 = store
                .with_conn(|c| c.query_row("SELECT COUNT(*) FROM facts", [], |r| r.get(0)))
                .expect("count");
            assert_eq!(n, 2, "seed rows inserted");
            assert_eq!(store.epoch(), 1);
        }
        {
            let store = open_sqlite(&d).expect("reopen");
            store.migrate("facts", M1).expect("migrate again");
            let n: i64 = store
                .with_conn(|c| c.query_row("SELECT COUNT(*) FROM facts", [], |r| r.get(0)))
                .expect("count");
            assert_eq!(n, 2, "seed not re-inserted on reopen (run-once)");
            assert_eq!(store.epoch(), 2, "lease epoch is monotonic across opens");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn database_epoch_survives_repeated_lease_sidecar_loss() {
        let (root, d) = tmp();
        let first = open_sqlite(&d).expect("first open");
        let first_epoch = first.epoch();
        drop(first);

        remove_lease_sidecar(&root);
        let second = open_sqlite(&d).expect("open after first sidecar loss");
        assert!(second.epoch() > first_epoch);
        let second_epoch = second.epoch();
        drop(second);

        remove_lease_sidecar(&root);
        let third = open_sqlite(&d).expect("open after second sidecar loss");
        assert!(third.epoch() > second_epoch);
        let db_epoch: i64 = third
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT epoch FROM cortexkit_fence WHERE id = 0",
                    [],
                    |row| row.get(0),
                )
            })
            .expect("database fence");
        assert_eq!(db_epoch as u64, third.epoch());
        let _ = std::fs::remove_dir_all(&root);
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
    fn second_live_writer_is_rejected() {
        let (root, d) = tmp();
        let _held = open_sqlite(&d).expect("first open");
        match open_sqlite(&d) {
            Err(StoreError::Lease(_)) => {}
            Err(e) => panic!("expected Lease(Held), got {e}"),
            Ok(_) => panic!("expected Lease(Held), got a second open"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn distinct_databases_do_not_falsely_contend() {
        let (root_a, a) = tmp();
        let (root_b, b) = tmp();
        let held_a = open_sqlite(&a).expect("open a");
        let held_b = open_sqlite(&b).expect("open b - distinct db, must not contend with a");
        drop((held_a, held_b));
        let _ = std::fs::remove_dir_all(&root_a);
        let _ = std::fs::remove_dir_all(&root_b);
    }

    /// Two distinct database files in ONE directory share a lease root, so the
    /// lease key must include the database file identity or they falsely contend.
    #[test]
    fn distinct_databases_in_one_directory_do_not_falsely_contend() {
        let (root, a) = tmp();
        let mut b = a.clone();
        b.backend = StorageBackend::Sqlite {
            path: root.join("other.db").to_string_lossy().into_owned(),
        };
        let held_a = open_sqlite(&a).expect("open a");
        let held_b = open_sqlite(&b).expect("open b - distinct db in the same directory as a");
        drop((held_a, held_b));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A `RAISE(IGNORE)` trigger can make the fence upsert affect zero rows.
    /// Accepting a zero-row fence update would let a writer proceed without
    /// persisting its claimed epoch.
    #[test]
    fn suppressed_fence_update_is_an_error_not_a_silent_success() {
        let (root, d) = tmp();
        let StorageBackend::Sqlite { path } = &d.backend else {
            panic!("sqlite descriptor");
        };
        let path = path.clone();
        drop(open_sqlite(&d).expect("seed database at epoch 1"));

        let mut conn = rusqlite::Connection::open(&path).expect("reopen raw");
        conn.execute_batch(
            "CREATE TRIGGER fence_suppressor BEFORE UPDATE ON cortexkit_fence \
             BEGIN SELECT RAISE(IGNORE); END",
        )
        .expect("install suppressing trigger");
        let tx = conn.transaction().expect("tx");
        match claim_fence(&tx, 99) {
            Err(StoreError::Backend(m)) => {
                assert!(m.contains("affected 0 rows"), "unexpected message: {m}")
            }
            other => panic!("suppressed fence write must fail, got {other:?}"),
        }
        drop(tx);
        drop(conn);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `StoreError` must preserve underlying io errors via `source()`,
    /// including the errno two hops down a `Lease` chain.
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

    /// Duplicate migration versions must be rejected before applying any migration.
    #[test]
    fn duplicate_migration_versions_are_rejected_before_any_apply() {
        let (root, d) = tmp();
        let store = open_sqlite(&d).expect("open");
        let dup: &[Migration] = &[
            Migration {
                version: 1,
                statements: "CREATE TABLE dup_a (k TEXT);",
            },
            Migration {
                version: 1,
                statements: "CREATE TABLE dup_b (k TEXT);",
            },
        ];
        match store.migrate("dup", dup) {
            Err(StoreError::Migration(m)) => {
                assert!(m.contains("more than once"), "unexpected message: {m}")
            }
            other => panic!("duplicate versions must be rejected, got {other:?}"),
        }
        let applied: i64 = store
            .with_conn(|c| {
                c.query_row(
                    "SELECT COUNT(*) FROM sqlite_schema WHERE name IN ('dup_a', 'dup_b')",
                    [],
                    |r| r.get(0),
                )
            })
            .expect("inspect schema");
        assert_eq!(applied, 0, "a duplicate batch must apply nothing");
        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn later_migration_applies_on_top_of_earlier() {
        let (root, d) = tmp();
        {
            let s = open_sqlite(&d).expect("v1");
            s.migrate("facts", M1).expect("v1 migrate");
        }
        const M2: &[Migration] = &[
            Migration {
                version: 1,
                statements: "CREATE TABLE facts (id INTEGER PRIMARY KEY, name TEXT NOT NULL);",
            },
            Migration {
                version: 2,
                statements: "ALTER TABLE facts ADD COLUMN weight REAL NOT NULL DEFAULT 0;",
            },
        ];
        let store = open_sqlite(&d).expect("v2");
        store.migrate("facts", M2).expect("v2 migrate");
        let ok: i64 = store
            .with_conn(|c| {
                c.query_row("SELECT COUNT(*) FROM facts WHERE weight = 0", [], |r| {
                    r.get(0)
                })
            })
            .expect("weight column queryable");
        assert_eq!(ok, 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn independent_namespace_chains_in_one_database() {
        let (root, d) = tmp();
        const WORK_GRAPH: &[Migration] = &[Migration {
            version: 1,
            statements: "CREATE TABLE wg_nodes (id INTEGER PRIMARY KEY);",
        }];
        const HIRES: &[Migration] = &[Migration {
            version: 1,
            statements: "CREATE TABLE hires (id INTEGER PRIMARY KEY);",
        }];
        let store = open_sqlite(&d).expect("open");
        store.migrate("work_graph", WORK_GRAPH).expect("work_graph");
        store.migrate("hires", HIRES).expect("hires");
        store
            .migrate("work_graph", WORK_GRAPH)
            .expect("work_graph again");
        let tables: i64 = store
            .with_conn(|c| {
                c.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('wg_nodes','hires')",
                    [],
                    |r| r.get(0),
                )
            })
            .expect("count tables");
        assert_eq!(
            tables, 2,
            "both domains' tables exist; version 1 did not collide across namespaces"
        );
        let _ = std::fs::remove_dir_all(&root);
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
        match open_sqlite(&d) {
            Err(StoreError::UnsupportedBackend(b)) => assert_eq!(b, "postgres"),
            Err(e) => panic!("expected UnsupportedBackend, got {e}"),
            Ok(_) => panic!("expected UnsupportedBackend, got an open store"),
        }
    }

    const FENCE_SCHEMA: &[Migration] = &[Migration {
        version: 1,
        statements: "CREATE TABLE kv (k TEXT PRIMARY KEY, v TEXT NOT NULL);",
    }];

    #[test]
    fn fenced_write_commits_and_persists() {
        let (root, d) = tmp();
        let store = open_sqlite(&d).expect("open");
        store.migrate("kv", FENCE_SCHEMA).expect("migrate");
        store
            .with_conn_fenced(|tx| {
                tx.execute("INSERT INTO kv (k, v) VALUES ('a', '1')", [])?;
                Ok(())
            })
            .expect("fenced write");
        let v: String = store
            .with_conn(|c| c.query_row("SELECT v FROM kv WHERE k = 'a'", [], |r| r.get(0)))
            .expect("read back");
        assert_eq!(v, "1");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unfenced_connection_rejects_writes() {
        let (root, d) = tmp();
        let store = open_sqlite(&d).expect("open");
        store.migrate("kv", FENCE_SCHEMA).expect("migrate");
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
    fn open_pins_full_synchronous() {
        let (root, d) = tmp();
        let store = open_sqlite(&d).expect("open");
        let sync: i64 = store
            .with_conn(|c| c.query_row("PRAGMA synchronous", [], |r| r.get(0)))
            .expect("read synchronous");
        assert_eq!(sync, 2, "fence durability requires synchronous=FULL");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_panicking_read_does_not_strand_the_connection_read_only() {
        let (root, d) = tmp();
        let store = open_sqlite(&d).expect("open");
        store.migrate("kv", FENCE_SCHEMA).expect("migrate");

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
        let store = open_sqlite(&d).expect("open");
        store.migrate("kv", FENCE_SCHEMA).expect("migrate");

        let lowered = store.with_conn(|c| c.execute("PRAGMA synchronous = OFF", []));
        assert!(
            matches!(&lowered, Err(StoreError::Backend(m)) if m.contains("not authorized")),
            "the read guard denies lowering synchronous, got {lowered:?}"
        );
        let unchanged: i64 = store
            .with_conn(|c| c.query_row("PRAGMA synchronous", [], |r| r.get(0)))
            .expect("reading a pragma stays allowed");
        assert_eq!(unchanged, 2, "the denied pragma left synchronous=FULL");

        // The fenced write restores `synchronous=FULL` after unrestricted maintenance
        // changes it.
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
        let second = &[Migration {
            version: 1,
            statements: "CREATE TABLE kv2 (k TEXT PRIMARY KEY);",
        }];
        store.migrate("kv2", second).expect("migrate");
        let after_migrate: i64 = store
            .with_conn(|c| c.query_row("PRAGMA synchronous", [], |r| r.get(0)))
            .expect("read synchronous");
        assert_eq!(after_migrate, 2, "migration re-pinned synchronous=FULL");

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
        let store = open_sqlite(&d).expect("open");
        store.migrate("kv", FENCE_SCHEMA).expect("migrate");

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
        // SQLite pragma names are case-insensitive.
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
        let n: i64 = store
            .with_conn(|c| c.query_row("SELECT COUNT(*) FROM kv", [], |r| r.get(0)))
            .expect("count");
        assert_eq!(n, 0, "the denied callback wrote nothing");

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
        let store = open_sqlite(&d).expect("open");
        store.migrate("kv", FENCE_SCHEMA).expect("migrate");

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

        let migration = &[Migration {
            version: 9,
            statements: "COMMIT; CREATE TABLE escaped_ddl (v TEXT);",
        }];
        let m = store.migrate("escape", migration);
        assert!(
            matches!(&m, Err(StoreError::Migration(msg)) if msg.contains("not authorized")),
            "a migration that ends its transaction is denied, got {m:?}"
        );
        let recorded: i64 = store
            .with_conn(|c| {
                c.query_row(
                    "SELECT COUNT(*) FROM cortexkit_schema_version WHERE namespace = 'escape'",
                    [],
                    |r| r.get(0),
                )
            })
            .expect("count");
        assert_eq!(recorded, 0, "the rejected migration recorded no version");

        let ddl_exists: bool = store
            .with_conn(|c| {
                c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name = 'escaped_ddl')",
                    [],
                    |r| r.get(0),
                )
            })
            .expect("schema lookup");
        assert!(!ddl_exists, "the denied migration created no table");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_callback_cannot_damage_the_fence_row_it_is_checked_against() {
        let (root, d) = tmp();
        let store = open_sqlite(&d).expect("open");
        store.migrate("kv", FENCE_SCHEMA).expect("migrate");
        let epoch = store.epoch();

        for sql in [
            "UPDATE cortexkit_fence SET epoch = 0 WHERE id = 0",
            "DELETE FROM cortexkit_fence WHERE id = 0",
            "INSERT INTO cortexkit_schema_version (namespace, version, applied_at_unix) \
             VALUES ('kv', 99, 0)",
            // A trigger reaches the row without naming it in a DML statement: raising
            // IGNORE on update suppresses a later opener's claim while it still succeeds.
            "CREATE TRIGGER freeze_fence BEFORE UPDATE ON cortexkit_fence \
             BEGIN SELECT RAISE(IGNORE); END",
            "DROP TABLE cortexkit_fence",
            "CREATE INDEX fence_idx ON cortexkit_fence (epoch)",
            // A view resolves ahead of the table it shadows, so a stale connection could
            // read its own epoch and skip the claim entirely.
            "CREATE TEMP VIEW cortexkit_fence AS SELECT 0 AS id, 1 AS epoch",
            "CREATE VIEW cortexkit_schema_version AS SELECT 1",
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
                     ('freeze_fence', 'fence_idx', 'cortexkit_schema_version') \
                     AND type <> 'table'",
                    [],
                    |r| r.get(0),
                )
            })
            .expect("schema lookup");
        assert_eq!(objects, 0, "no denied schema object was created");
        let shadow: i64 = store
            .with_conn(|c| {
                c.query_row(
                    "SELECT COUNT(*) FROM temp.sqlite_schema WHERE name = 'cortexkit_fence'",
                    [],
                    |r| r.get(0),
                )
            })
            .expect("temp schema lookup");
        assert_eq!(shadow, 0, "no temporary object shadows the fence table");

        let stored: i64 = store
            .with_conn(|c| {
                c.query_row("SELECT epoch FROM cortexkit_fence WHERE id = 0", [], |r| {
                    r.get(0)
                })
            })
            .expect("read the fence row");
        assert_eq!(
            stored, epoch as i64,
            "the fence row still carries the epoch the callbacks were checked against"
        );

        let migration = &[Migration {
            version: 8,
            statements: "UPDATE cortexkit_fence SET epoch = 0 WHERE id = 0;",
        }];
        let m = store.migrate("fencerow", migration);
        assert!(
            matches!(&m, Err(StoreError::Migration(msg)) if msg.contains("not authorized")),
            "a migration that lowers the fence row is denied, got {m:?}"
        );

        // `AuthAction::AlterTable` reports the source name, so the rename is authorized
        // and has to be caught by name once the callback returns.
        // SQLite resolves identifiers case-insensitively, so the check must too.
        for target in ["cortexkit_fence", "CORTEXKIT_FENCE", "CortexKit_Fence"] {
            let renamed = store.with_conn_fenced(|tx| {
                tx.execute("CREATE TEMP TABLE benign (id INTEGER, epoch INTEGER)", [])?;
                tx.execute(&format!("ALTER TABLE benign RENAME TO {target}"), [])
                    .map(|_| ())
            });
            assert!(
                matches!(&renamed, Err(StoreError::Backend(m)) if m.contains("shadows the infrastructure table")),
                "a temporary table renamed to `{target}` is rejected, got {renamed:?}"
            );
        }
        let shadow_after: i64 = store
            .with_conn(|c| {
                c.query_row(
                    "SELECT COUNT(*) FROM temp.sqlite_schema WHERE name = 'cortexkit_fence'",
                    [],
                    |r| r.get(0),
                )
            })
            .expect("temp schema lookup");
        assert_eq!(
            shadow_after, 0,
            "rejecting before commit rolls the temporary object back with the transaction"
        );
        store
            .with_conn_fenced(|tx| {
                tx.execute("INSERT INTO kv (k, v) VALUES ('after-shadow', '1')", [])
                    .map(|_| ())
            })
            .expect("the fenced path still works after a rejected shadow");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn maintenance_runs_through_the_unfenced_path() {
        let (root, d) = tmp();
        let store = open_sqlite(&d).expect("open");
        store.migrate("kv", FENCE_SCHEMA).expect("migrate");
        let r = store.with_conn(|c| c.execute("VACUUM", []));
        assert!(
            matches!(&r, Err(StoreError::Backend(m)) if m.contains("authorization denied")),
            "VACUUM must not pass the read callback scope, got {r:?}"
        );
        store
            .with_conn_unfenced(|c| c.execute_batch("VACUUM"))
            .expect("VACUUM through the maintenance path");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn fenced_write_rolls_back_on_error() {
        let (root, d) = tmp();
        let path = sqlite_path(&d);
        open_sqlite(&d)
            .expect("open")
            .migrate("kv", FENCE_SCHEMA)
            .expect("migrate");
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
            .with_conn(|c| {
                c.query_row("SELECT epoch FROM cortexkit_fence WHERE id = 0", [], |r| {
                    r.get(0)
                })
            })
            .expect("read fence");
        assert_eq!(
            claimed, 1,
            "the failed callback did not roll back its fence claim"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn legacy_database_without_fence_table_uses_zero_floor() {
        let (root, d) = tmp();
        let path = sqlite_path(&d);
        std::fs::create_dir_all(&root).expect("create store directory");
        let conn = rusqlite::Connection::open(&path).expect("create legacy database");
        conn.execute_batch("CREATE TABLE legacy_data (id INTEGER PRIMARY KEY);")
            .expect("create legacy schema");
        drop(conn);

        let store = open_sqlite(&d).expect("open legacy database");
        assert_eq!(store.epoch(), 1, "missing fence table must use floor zero");
        let claimed: i64 = store
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT epoch FROM cortexkit_fence WHERE id = 0",
                    [],
                    |row| row.get(0),
                )
            })
            .expect("read claimed fence");
        assert_eq!(claimed, 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn legacy_negative_database_fence_fails_closed() {
        let (root, d) = tmp();
        let path = sqlite_path(&d);
        std::fs::create_dir_all(&root).expect("create store directory");
        let conn = rusqlite::Connection::open(&path).expect("create legacy database");
        conn.execute_batch(
            "CREATE TABLE cortexkit_fence (id INTEGER PRIMARY KEY, epoch INTEGER NOT NULL); \
             INSERT INTO cortexkit_fence (id, epoch) VALUES (0, -1);",
        )
        .expect("seed pre-fence-validation database");
        drop(conn);

        let error = match open_sqlite(&d) {
            Err(error) => error,
            Ok(_) => panic!("negative fence must fail closed"),
        };
        assert!(matches!(error, StoreError::FenceCorrupt { db_epoch } if db_epoch == -1));
        let persisted: i64 = rusqlite::Connection::open(&path)
            .expect("reopen legacy database")
            .query_row(
                "SELECT epoch FROM cortexkit_fence WHERE id = 0",
                [],
                |row| row.get(0),
            )
            .expect("read unchanged negative fence");
        assert_eq!(persisted, -1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn superseded_writer_is_fenced_out_after_handover() {
        // Model the post-handover state directly: the OS lock prevents two live
        // lease holders, but a stale connection can persist after releasing its lease.
        let (root, d) = tmp();
        let path = sqlite_path(&d);

        open_sqlite(&d)
            .expect("seed schema")
            .migrate("kv", FENCE_SCHEMA)
            .expect("migrate");

        let new = SqliteStore::for_test(rusqlite::Connection::open(&path).unwrap(), 2);
        new.with_conn_fenced(|tx| {
            tx.execute("INSERT INTO kv (k, v) VALUES ('owner', 'new')", [])
                .map(|_| ())
        })
        .expect("replacement claims the db at epoch 2");

        let stale = SqliteStore::for_test(rusqlite::Connection::open(&path).unwrap(), 1);
        match stale.with_conn_fenced(|tx| {
            tx.execute("UPDATE kv SET v = 'clobbered' WHERE k = 'owner'", [])
                .map(|_| ())
        }) {
            Err(StoreError::Fenced {
                holder_epoch,
                db_epoch,
            }) => {
                assert_eq!(holder_epoch, 1);
                assert_eq!(db_epoch, 2);
            }
            other => panic!("expected Fenced, got {other:?}"),
        }

        let v: String = new
            .with_conn(|c| c.query_row("SELECT v FROM kv WHERE k = 'owner'", [], |r| r.get(0)))
            .expect("read");
        assert_eq!(v, "new", "stale writer was fenced out, no clobber");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn superseded_writer_cannot_migrate() {
        let (root, d) = tmp();
        let path = sqlite_path(&d);
        open_sqlite(&d).expect("seed database");

        let replacement = SqliteStore::for_test(rusqlite::Connection::open(&path).unwrap(), 2);
        replacement
            .with_conn_fenced(|_| Ok(()))
            .expect("replacement claim");
        let stale = SqliteStore::for_test(rusqlite::Connection::open(&path).unwrap(), 1);
        let migration = [Migration {
            version: 1,
            statements: "CREATE TABLE stale_schema (id INTEGER PRIMARY KEY);",
        }];
        assert!(matches!(
            stale.migrate("stale", &migration),
            Err(StoreError::Fenced { .. })
        ));

        let tables: i64 = replacement
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name = 'stale_schema'",
                    [],
                    |row| row.get(0),
                )
            })
            .expect("schema state");
        assert_eq!(tables, 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn equal_epoch_writer_is_not_fenced() {
        let (root, d) = tmp();
        let path = sqlite_path(&d);
        open_sqlite(&d)
            .expect("seed")
            .migrate("kv", FENCE_SCHEMA)
            .expect("migrate");
        let s = SqliteStore::for_test(rusqlite::Connection::open(&path).unwrap(), 5);
        s.with_conn_fenced(|tx| {
            tx.execute("INSERT INTO kv (k, v) VALUES ('a', '1')", [])
                .map(|_| ())
        })
        .expect("claims at 5");
        s.with_conn_fenced(|tx| {
            tx.execute("INSERT INTO kv (k, v) VALUES ('b', '2')", [])
                .map(|_| ())
        })
        .expect("same epoch 5 still writes");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn epoch_above_sqlite_integer_range_fails() {
        let (root, d) = tmp();
        let path = sqlite_path(&d);
        open_sqlite(&d).expect("seed database");
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

    fn sqlite_path(d: &StorageDescriptor) -> String {
        match &d.backend {
            StorageBackend::Sqlite { path } => path.clone(),
            _ => unreachable!(),
        }
    }

    /// Existing databases require these exact table names and definitions.
    #[test]
    fn fence_and_version_tables_keep_their_ddl() {
        let (root, d) = tmp();
        let store = open_sqlite(&d).expect("open");
        store
            .migrate(
                "ddl",
                &[Migration {
                    version: 1,
                    statements: "CREATE TABLE t (k TEXT);",
                }],
            )
            .expect("migrate");
        let schema: Vec<(String, String)> = store
            .with_conn(|c| {
                let mut statement = c.prepare(
                    "SELECT name, sql FROM sqlite_schema \
                     WHERE type = 'table' AND name LIKE 'cortexkit%' ORDER BY name",
                )?;
                let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
                rows.collect()
            })
            .expect("read schema");
        assert_eq!(
            schema,
            vec![
                (
                    "cortexkit_fence".to_string(),
                    "CREATE TABLE cortexkit_fence (id INTEGER PRIMARY KEY CHECK (id = 0), \
                     epoch INTEGER NOT NULL CHECK (epoch >= 0))"
                        .to_string(),
                ),
                (
                    "cortexkit_schema_version".to_string(),
                    "CREATE TABLE cortexkit_schema_version (namespace TEXT NOT NULL, \
                     version INTEGER NOT NULL, applied_at_unix INTEGER NOT NULL, \
                     PRIMARY KEY (namespace, version))"
                        .to_string(),
                ),
            ]
        );
        let (epoch, rows): (i64, i64) = store
            .with_conn(|c| {
                c.query_row(
                    "SELECT (SELECT epoch FROM cortexkit_fence WHERE id = 0), \
                     (SELECT COUNT(*) FROM cortexkit_fence)",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
            })
            .expect("read fence row");
        assert_eq!((epoch, rows), (store.epoch() as i64, 1));
        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }
}
