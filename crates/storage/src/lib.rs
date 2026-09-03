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
        path::{Path, PathBuf},
        sync::Mutex,
        time::Duration,
    };

    use lease::{FileIdentity, FileLeaseStore, HeldFileLease, protect_file};
    use rusqlite::{Connection, OpenFlags};
    use sha2::{Digest, Sha256};

    /// `PRAGMA application_id` of every Eidnara-owned SQLite file (`EIDN` in ASCII).
    pub const APPLICATION_ID: u32 = 0x4549_444E;
    /// `PRAGMA user_version` of every Eidnara-owned SQLite file.
    pub const USER_VERSION: u32 = 1;
    /// The objects every store carries ahead of the consumer's baseline.
    const STORE_BASELINE: &str = include_str!("../baseline.sql");

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
        /// `ATTACH`/`DETACH`, and writes to the fence and format-marker tables are denied.
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
            let mut guard = self.conn.lock().unwrap_or_else(|p| p.into_inner());
            // A superseded writer must not touch the file at all, and the durability pin
            // rewrites the journal mode. This read-only precheck refuses it before the
            // pragmas run; the claim inside the transaction remains the authoritative check.
            precheck_fence(&guard, self.epoch)?;
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

    /// The connection is shared by reads, fenced writes, and maintenance, so
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
        /// The `query_only` value to restore: `Some(prior)` when the scope switched it on
        /// for a read, `None` when the scope left it alone.
        query_only_before: Option<bool>,
        /// Infrastructure-named schema objects at install, compared again at release.
        infrastructure_before: Vec<String>,
    }

    /// Every main- or temp-schema object whose name is an infrastructure table name,
    /// tagged with its schema and type so a swap of table for view is visible.
    fn infrastructure_objects(conn: &Connection) -> Result<Vec<String>, StoreError> {
        let mut statement = conn
            .prepare(
                "SELECT 'main', type, name FROM main.sqlite_schema \
                 WHERE lower(name) IN ('fence', 'format_marker') \
                 UNION ALL \
                 SELECT 'temp', type, name FROM temp.sqlite_schema \
                 WHERE lower(name) IN ('fence', 'format_marker') \
                 ORDER BY 1, 2, 3",
            )
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        let rows = statement
            .query_map([], |row| {
                Ok(format!(
                    "{}.{} {}",
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(1)?
                ))
            })
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| StoreError::Backend(e.to_string()))
    }

    /// Lower-cased names of every main-schema object; SQLite resolves unqualified names
    /// through the temp schema first, so a temp object under one of these names would
    /// capture the writes a callback believes it makes to the store.
    fn main_schema_names(
        conn: &Connection,
    ) -> Result<std::collections::HashSet<String>, StoreError> {
        let mut statement = conn
            .prepare("SELECT lower(name) FROM main.sqlite_schema")
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        rows.collect::<Result<_, _>>()
            .map_err(|e| StoreError::Backend(e.to_string()))
    }

    impl<'c> CallbackScope<'c> {
        /// Denies writes as well as the escapes, for a callback that must not mutate.
        fn read_only(conn: &'c Connection) -> Result<Self, StoreError> {
            let prior: bool = conn
                .query_row("PRAGMA query_only", [], |row| row.get(0))
                .map_err(|e| StoreError::Backend(e.to_string()))?;
            conn.pragma_update(None, "query_only", "ON")
                .map_err(|e| StoreError::Backend(e.to_string()))?;
            // A failure between the pragma and the finished scope would leave the shared
            // connection read-only with no guard to restore it.
            Self::install(conn, Some(prior)).inspect_err(|_| {
                let _ = Self::restore(conn, Some(prior));
            })
        }

        /// Denies the escapes only, for a callback inside a fence-checked transaction.
        fn writable(conn: &'c Connection) -> Result<Self, StoreError> {
            Self::install(conn, None)
        }

        fn install(
            conn: &'c Connection,
            query_only_before: Option<bool>,
        ) -> Result<Self, StoreError> {
            let infrastructure_before = infrastructure_objects(conn)?;
            let main_names = main_schema_names(conn)?;
            let scope = Self {
                conn: Some(conn),
                query_only_before,
                infrastructure_before,
            };
            conn.authorizer(Some(move |context: rusqlite::hooks::AuthContext<'_>| {
                deny_scope_escapes(context, &main_names)
            }))
            .map_err(|e| StoreError::Backend(e.to_string()))?;
            Ok(scope)
        }

        /// Reports a failed release to the caller, which `Drop` cannot do.
        fn release(mut self) -> Result<(), StoreError> {
            match self.conn.take() {
                Some(conn) => {
                    let unchanged =
                        Self::require_infrastructure_unchanged(conn, &self.infrastructure_before);
                    Self::restore(conn, self.query_only_before).and(unchanged)
                }
                None => Ok(()),
            }
        }

        /// `AuthAction::AlterTable` reports the source name, so a rename cannot be judged
        /// when it is authorized: a table the callback created and renamed to an
        /// infrastructure name is trusted by the next fence claim. The set of
        /// infrastructure-named objects in the main and temp schemas is compared before
        /// and after the callback, before its transaction commits.
        fn require_infrastructure_unchanged(
            conn: &Connection,
            before: &[String],
        ) -> Result<(), StoreError> {
            let after = infrastructure_objects(conn)?;
            if after != before {
                return Err(StoreError::Backend(format!(
                    "the callback changed the infrastructure schema objects from {before:?} to {after:?}"
                )));
            }
            Ok(())
        }

        /// Puts `query_only` back to the value the scope found, so a connection that
        /// maintenance left read-only stays read-only.
        fn restore(conn: &Connection, query_only_before: Option<bool>) -> Result<(), StoreError> {
            let cleared = conn.authorizer(
                None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>,
            );
            let unlocked = match query_only_before {
                Some(prior) => conn.pragma_update(None, "query_only", prior),
                None => Ok(()),
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
                let _ = Self::restore(conn, self.query_only_before);
            }
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

    /// Enumerating individual pragma names cannot be complete: `ignore_check_constraints`
    /// disables the fence table's constraint, `defer_foreign_keys` and `writable_schema`
    /// reach schema invariants, and pragma names are case-insensitive. Denying the whole
    /// capability class avoids that race. A pragma read carries no value and stays
    /// allowed, as do the ordinary statements a callback exists to run. The argumentless
    /// pragmas that still perform work are denied by name.
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
                pragma_value: Some(_),
                ..
            }
            | AuthAction::Transaction { .. }
            | AuthAction::Savepoint { .. }
            | AuthAction::Attach { .. }
            | AuthAction::Detach { .. } => Authorization::Deny,
            AuthAction::Pragma {
                pragma_name,
                pragma_value: None,
            } if is_side_effecting_pragma(pragma_name) => Authorization::Deny,
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
        [
            "wal_checkpoint",
            "incremental_vacuum",
            "optimize",
            "shrink_memory",
        ]
        .iter()
        .any(|pragma| name.eq_ignore_ascii_case(pragma))
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
        table_name.eq_ignore_ascii_case("fence") || table_name.eq_ignore_ascii_case("format_marker")
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
        // An existing file is inspected on a read-only connection before anything can
        // change it. A read-write open would let SQLite recover or checkpoint a foreign
        // WAL and rewrite the file on close, and a fence row in a foreign file would
        // otherwise raise the lease floor before the file was ever classified.
        let (epoch_floor, inspected) = expected.inspect_existing(Path::new(&path))?;
        // The lease will issue at least `epoch_floor + 1`, and that value must be storable
        // in the fence row; refusing here keeps an unrepresentable successor out of the
        // lease sidecar, which would otherwise stay poisoned after the row was repaired.
        if epoch_floor >= i64::MAX as u64 {
            return Err(StoreError::FenceExhausted {
                db_epoch: epoch_floor,
            });
        }
        let db_file_name = Path::new(&path)
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());
        let lease = FileLeaseStore::new(&parent)
            .map_err(StoreError::Io)?
            .acquire_above(&lease_key(descriptor, &db_file_name)?, epoch_floor)
            .map_err(StoreError::Lease)?;
        let epoch = lease.epoch();

        let conn = open_claimed(&path, &expected, inspected, epoch)?;

        Ok(SqliteStore {
            conn: Mutex::new(conn),
            epoch,
            _lease: Some(lease),
        })
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
    ) -> Result<Connection, StoreError> {
        // Owner-only from creation: SQLite gives sidecars the database file's mode.
        create_database_file_owner_only(Path::new(path)).map_err(StoreError::Io)?;
        // `SQLITE_OPEN_NOFOLLOW` closes the window between the owner-only creation and this
        // open, so a symlink swapped in between is refused rather than followed.
        let mut conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(|e| StoreError::Backend(e.to_string()))?;
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
        pin_fence_durability(&conn)?;
        // The busy timeout makes transient locks wait rather than fail, and
        // foreign-key enforcement is enabled.
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| StoreError::Backend(e.to_string()))?;

        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        if state == FileState::Pristine {
            expected.apply(&tx)?;
        }
        claim_fence_strict(&tx, epoch, state)?;
        tx.commit()
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        Ok(conn)
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

    const FENCE_EPOCH_SQL: &str = "SELECT epoch FROM fence WHERE id = 0";

    /// The fence epoch, or `None` when the row is absent. The caller guarantees that the
    /// `fence` table exists; only the transaction that initializes a pristine file may see
    /// no row, since the baseline creates the table and the first claim writes the row.
    fn read_fence_epoch_in(conn: &Connection) -> Result<Option<u64>, StoreError> {
        let epoch: Option<i64> = conn
            .query_row(FENCE_EPOCH_SQL, [], |row| row.get(0))
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })
            .map_err(|e| StoreError::Backend(e.to_string()))?;
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

        /// Applies the baseline to a pristine file inside the caller's transaction.
        fn apply(&self, tx: &rusqlite::Transaction<'_>) -> Result<(), StoreError> {
            tx.execute_batch(&self.text)
                .map_err(|e| StoreError::Backend(e.to_string()))?;
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
        let db_epoch = read_fence_epoch_in(conn)?.ok_or(StoreError::FenceMissing)?;
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
            .execute(
                "INSERT INTO fence (id, epoch) VALUES (0, ?1) \
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
}

#[cfg(feature = "sqlite")]
pub use sqlite_backend::{
    APPLICATION_ID, GuardedConn, MaintenanceConn, SchemaObject, SqliteStore, USER_VERSION,
    open_sqlite, schema_inventory,
};

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use super::sqlite_backend::{
        ExpectedIdentity, FileState, InspectionCopy, claim_fence, claim_fence_strict,
        create_database_file_owner_only, immutable_uri, open_claimed,
    };
    use super::*;
    use lease::FileIdentity;
    use std::path::Path;

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

    /// A temporary object in a baseline would exist on the first open only.
    #[test]
    fn a_baseline_that_creates_temporary_objects_is_rejected() {
        let (root, d) = tmp();
        let StorageBackend::Sqlite { path } = &d.backend else {
            panic!("sqlite descriptor");
        };
        for ddl in [
            "CREATE TEMP TABLE scratch (k TEXT);",
            "CREATE TABLE kv (k TEXT); CREATE TEMP VIEW kv_view AS SELECT k FROM kv;",
        ] {
            match open_sqlite(&d, ddl).map(|_| ()) {
                Err(StoreError::Baseline(m)) => {
                    assert!(m.contains("temporary object"), "unexpected message: {m}")
                }
                other => panic!("{ddl} must be rejected, got {other:?}"),
            }
        }
        assert!(!std::path::Path::new(path).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A FIFO at the database path would block SQLite's open before any timeout applies.
    #[cfg(unix)]
    #[test]
    fn a_fifo_at_the_database_path_is_refused_before_sqlite_opens_it() {
        let (root, d) = tmp();
        let StorageBackend::Sqlite { path } = &d.backend else {
            panic!("sqlite descriptor");
        };
        std::fs::create_dir_all(&root).expect("root");
        let c_path = std::ffi::CString::new(path.as_str()).expect("path");
        // SAFETY: `c_path` is a valid NUL-terminated string for the duration of the call.
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0, "mkfifo");
        match open_sqlite(&d, "").map(|_| ()) {
            Err(StoreError::Baseline(m)) => {
                assert!(m.contains("not a regular file"), "unexpected message: {m}")
            }
            other => panic!("a FIFO must be refused without blocking, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A FIFO at the `-journal` path is refused with the other sidecars before the inspection
    /// would copy it, so the open cannot block on it.
    #[cfg(unix)]
    #[test]
    fn a_fifo_at_the_journal_path_is_refused_before_inspection() {
        let (root, d) = tmp();
        let StorageBackend::Sqlite { path } = &d.backend else {
            panic!("sqlite descriptor");
        };
        drop(open_sqlite(&d, "").expect("first open"));
        let journal = format!("{path}-journal");
        let c_path = std::ffi::CString::new(journal.as_str()).expect("path");
        // SAFETY: `c_path` is a valid NUL-terminated string for the duration of the call.
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0, "mkfifo");
        match open_sqlite(&d, "").map(|_| ()) {
            Err(StoreError::Baseline(m)) => {
                assert!(m.contains("not a regular file"), "unexpected message: {m}")
            }
            other => panic!("a FIFO journal must be refused without blocking, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A baseline may address the main schema only: no attachments, no pragma writes, no
    /// transaction control, and the store's own tables keep the definitions `baseline.sql`
    /// gives them.
    #[test]
    fn a_baseline_that_attaches_or_redefines_infrastructure_is_rejected() {
        let (root, d) = tmp();
        let StorageBackend::Sqlite { path } = &d.backend else {
            panic!("sqlite descriptor");
        };
        for (ddl, needle) in [
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
        ] {
            match open_sqlite(&d, ddl).map(|_| ()) {
                Err(StoreError::Baseline(m)) => {
                    assert!(m.contains(needle), "unexpected message: {m}")
                }
                other => panic!("{ddl} must be rejected, got {other:?}"),
            }
        }
        assert!(!std::path::Path::new(path).exists());
        let _ = std::fs::remove_dir_all(&root);
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

    /// A WAL-mode foreign database that was closed cleanly has no sidecars; the refusal
    /// creates none, since the inspection opens the file `immutable`.
    #[test]
    fn a_refused_foreign_wal_mode_database_gains_no_sidecars() {
        let (root, d) = tmp();
        let StorageBackend::Sqlite { path } = &d.backend else {
            panic!("sqlite descriptor");
        };
        std::fs::create_dir_all(&root).expect("root");
        {
            let foreign = rusqlite::Connection::open(path).expect("foreign database");
            foreign
                .execute_batch("PRAGMA journal_mode = WAL; CREATE TABLE theirs (k TEXT);")
                .expect("foreign schema");
        }
        assert!(!std::path::Path::new(&format!("{path}-wal")).exists());
        let before = std::fs::read(path).expect("bytes before");
        assert!(matches!(
            open_sqlite(&d, "").map(|_| ()),
            Err(StoreError::Baseline(_))
        ));
        assert_eq!(std::fs::read(path).expect("bytes after"), before);
        for suffix in ["-wal", "-shm"] {
            assert!(
                !std::path::Path::new(&format!("{path}{suffix}")).exists(),
                "refusal must not create {suffix}"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
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

    /// The identity the read-write open is checked against changes when the path is
    /// re-pointed at a different file, and is stable across a rewrite in place.
    #[cfg(unix)]
    #[test]
    fn file_identity_follows_the_inode_not_the_bytes() {
        let (root, d) = tmp();
        let StorageBackend::Sqlite { path } = &d.backend else {
            panic!("sqlite descriptor");
        };
        std::fs::create_dir_all(&root).expect("root");
        std::fs::write(path, b"one").expect("write");
        let first = FileIdentity::of_path(Path::new(path)).expect("identity");
        std::fs::write(path, b"one rewritten in place").expect("rewrite");
        assert_eq!(
            FileIdentity::of_path(Path::new(path)).expect("identity"),
            first
        );
        let replacement = root.join("replacement.db");
        std::fs::write(&replacement, b"one").expect("write replacement");
        std::fs::rename(&replacement, path).expect("swap the file in");
        assert_ne!(
            FileIdentity::of_path(Path::new(path)).expect("identity"),
            first
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

    /// A superseded writer is refused before the durability pin runs, so a journal mode
    /// that maintenance on the newer writer chose is not switched back by the stale one.
    #[test]
    fn a_superseded_writer_does_not_change_the_journal_mode_before_being_fenced() {
        let (root, d) = tmp();
        let StorageBackend::Sqlite { path } = &d.backend else {
            panic!("sqlite descriptor");
        };
        drop(open_sqlite(&d, KV_BASELINE).expect("seed schema"));
        let stale = SqliteStore::for_test(rusqlite::Connection::open(path).unwrap(), 1);
        let newer = SqliteStore::for_test(rusqlite::Connection::open(path).unwrap(), 2);
        newer
            .with_conn_fenced(|tx| {
                tx.execute("INSERT INTO kv (k, v) VALUES ('owner', '2')", [])
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
            tx.execute("INSERT INTO kv (k, v) VALUES ('stale', '1')", [])
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

        let conn = open_claimed(path, &expected, None, 11).expect("a newer epoch opens");
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

    /// A consumer baseline cannot hang a trigger or index on the fence or the marker.
    #[test]
    fn a_baseline_that_hooks_an_infrastructure_table_is_rejected() {
        let (root, d) = tmp();
        let StorageBackend::Sqlite { path } = &d.backend else {
            panic!("sqlite descriptor");
        };
        for hook in [
            "CREATE TRIGGER marker_undo BEFORE INSERT ON format_marker BEGIN SELECT RAISE(IGNORE); END;",
            "CREATE TRIGGER fence_undo AFTER UPDATE ON fence BEGIN UPDATE fence SET epoch = OLD.epoch WHERE id = 0; END;",
            "CREATE INDEX fence_idx ON fence (epoch);",
        ] {
            match open_sqlite(&d, hook).map(|_| ()) {
                Err(StoreError::Baseline(m)) => {
                    assert!(
                        m.contains("infrastructure table"),
                        "unexpected message: {m}"
                    )
                }
                other => panic!("{hook} must be rejected, got {other:?}"),
            }
        }
        assert!(
            !std::path::Path::new(path).exists(),
            "a rejected baseline must not create the file"
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
        assert!(
            matches!(
                open_sqlite(&via_dir, "").map(|_| ()),
                Err(StoreError::Backend(_))
            ),
            "a directory-symlink alias must be refused"
        );

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

    #[test]
    fn a_read_callback_cannot_checkpoint_the_wal() {
        let (root, d) = tmp();
        let store = open_sqlite(&d, "").expect("open");
        let denied =
            store.with_conn(|c| c.query_row("PRAGMA wal_checkpoint", [], |r| r.get::<_, i64>(0)));
        assert!(
            matches!(&denied, Err(StoreError::Backend(m)) if m.contains("authorization denied") || m.contains("not authorized")),
            "wal_checkpoint must be denied inside a read callback, got {denied:?}"
        );
        let mode: String = store
            .with_conn(|c| c.query_row("PRAGMA journal_mode", [], |r| r.get(0)))
            .expect("reading a pragma stays allowed");
        assert!(mode.eq_ignore_ascii_case("wal"));
        drop(store);
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
    fn a_file_with_foreign_objects_is_refused_without_mutation() {
        let (root, d) = tmp();
        let path = sqlite_path(&d);
        std::fs::create_dir_all(&root).expect("create store directory");
        let conn = rusqlite::Connection::open(&path).expect("create a foreign database");
        conn.execute_batch("CREATE TABLE unfenced_data (id INTEGER PRIMARY KEY);")
            .expect("create schema");
        drop(conn);
        let before = std::fs::read(&path).expect("read the foreign file");

        let refused = open_sqlite(&d, "");
        assert!(
            matches!(refused, Err(StoreError::Baseline(_))),
            "a foreign file is refused, got {:?}",
            refused.map(|_| ())
        );
        let after = std::fs::read(&path).expect("read the foreign file again");
        assert!(before == after, "the refused open changed the file's bytes");
        for suffix in ["-wal", "-shm"] {
            assert!(
                !std::path::Path::new(&format!("{path}{suffix}")).exists(),
                "the refused open left a {suffix} sidecar behind"
            );
        }
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

    #[test]
    fn distinct_databases_do_not_falsely_contend() {
        let (root_a, a) = tmp();
        let (root_b, b) = tmp();
        let held_a = open_sqlite(&a, "").expect("open a");
        let held_b = open_sqlite(&b, "").expect("open b - distinct db, must not contend with a");
        drop((held_a, held_b));
        let _ = std::fs::remove_dir_all(&root_a);
        let _ = std::fs::remove_dir_all(&root_b);
    }

    #[test]
    fn distinct_databases_in_one_directory_do_not_falsely_contend() {
        let (root, a) = tmp();
        let mut b = a.clone();
        b.backend = StorageBackend::Sqlite {
            path: root.join("other.db").to_string_lossy().into_owned(),
        };
        let held_a = open_sqlite(&a, "").expect("open a");
        let held_b = open_sqlite(&b, "").expect("open b - distinct db in the same directory as a");
        drop((held_a, held_b));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn suppressed_fence_update_is_an_error_not_a_silent_success() {
        let (root, d) = tmp();
        let StorageBackend::Sqlite { path } = &d.backend else {
            panic!("sqlite descriptor");
        };
        let path = path.clone();
        drop(open_sqlite(&d, "").expect("seed database at epoch 1"));

        let mut conn = rusqlite::Connection::open(&path).expect("reopen raw");
        conn.execute_batch(
            "CREATE TRIGGER fence_suppressor BEFORE UPDATE ON fence \
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

    #[test]
    fn undone_fence_update_is_an_error_not_a_silent_success() {
        let (root, d) = tmp();
        let StorageBackend::Sqlite { path } = &d.backend else {
            panic!("sqlite descriptor");
        };
        let path = path.clone();
        drop(open_sqlite(&d, "").expect("seed database at epoch 1"));

        let mut conn = rusqlite::Connection::open(&path).expect("reopen raw");
        conn.execute_batch(
            "CREATE TRIGGER fence_undo AFTER UPDATE ON fence \
             BEGIN UPDATE fence SET epoch = OLD.epoch WHERE id = 0; END",
        )
        .expect("install undoing trigger");
        let tx = conn.transaction().expect("tx");
        match claim_fence(&tx, 99) {
            Err(StoreError::Backend(m)) => {
                assert!(m.contains("reads back as 1"), "unexpected message: {m}")
            }
            other => panic!("undone fence write must fail, got {other:?}"),
        }
        drop(tx);
        drop(conn);
        let _ = std::fs::remove_dir_all(&root);
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
    fn fenced_write_commits_and_persists() {
        let (root, d) = tmp();
        let store = open_sqlite(&d, KV_BASELINE).expect("open");
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
    fn open_pins_full_synchronous() {
        let (root, d) = tmp();
        let store = open_sqlite(&d, "").expect("open");
        let sync: i64 = store
            .with_conn(|c| c.query_row("PRAGMA synchronous", [], |r| r.get(0)))
            .expect("read synchronous");
        assert_eq!(sync, 2, "fence durability requires synchronous=FULL");
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
            "every fenced write re-pins synchronous=FULL"
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
    fn a_callback_cannot_damage_the_fence_row_it_is_checked_against() {
        let (root, d) = tmp();
        let store = open_sqlite(&d, KV_BASELINE).expect("open");
        let epoch = store.epoch();

        for sql in [
            "UPDATE fence SET epoch = 0 WHERE id = 0",
            "DELETE FROM fence WHERE id = 0",
            "INSERT INTO format_marker (id, baseline_sha256) VALUES (0, 'forged')",
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

        // Schema changes are denied outright, so a rename can never shadow the fence.
        for target in ["fence", "FENCE", "Fence"] {
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
                    "SELECT COUNT(*) FROM temp.sqlite_schema WHERE lower(name) = 'fence'",
                    [],
                    |r| r.get(0),
                )
            })
            .expect("inspect temp schema");
        assert_eq!(shadows, 0, "no temporary object may shadow the fence");
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
        store
            .with_conn_fenced(|tx| {
                tx.execute("INSERT INTO kv (k, v) VALUES ('after-shadow', '1')", [])
                    .map(|_| ())
            })
            .expect("the fenced path still works after a rejected shadow");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_fenced_callback_cannot_rewrite_the_format_marker() {
        let (root, d) = tmp();
        let store = open_sqlite(&d, "").expect("open");
        let before: String = store
            .with_conn(|c| {
                c.query_row(
                    "SELECT baseline_sha256 FROM format_marker WHERE id = 0",
                    [],
                    |r| r.get(0),
                )
            })
            .expect("read the marker");

        let forged = "f".repeat(64);
        let rewritten = store.with_conn_fenced(|tx| {
            tx.execute(
                "UPDATE format_marker SET baseline_sha256 = ?1 WHERE id = 0",
                rusqlite::params![forged],
            )
            .map(|_| ())
        });
        assert!(
            matches!(&rewritten, Err(StoreError::Backend(m)) if m.contains("not authorized")),
            "rewriting the format marker is denied, got {rewritten:?}"
        );

        let renamed = store.with_conn_fenced(|tx| {
            tx.execute(
                "CREATE TEMP TABLE benign (id INTEGER, baseline_sha256 TEXT)",
                [],
            )?;
            tx.execute("ALTER TABLE benign RENAME TO format_marker", [])
                .map(|_| ())
        });
        assert!(
            matches!(&renamed, Err(StoreError::Backend(m)) if m.contains("not authorized")),
            "a temporary table renamed onto the marker is rejected, got {renamed:?}"
        );

        let (after, rows): (String, i64) = store
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
            (after, rows),
            (before, 1),
            "the format marker row is unchanged"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn maintenance_runs_through_the_unfenced_path() {
        let (root, d) = tmp();
        let store = open_sqlite(&d, KV_BASELINE).expect("open");
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
    fn superseded_writer_is_fenced_out_after_handover() {
        let (root, d) = tmp();
        let path = sqlite_path(&d);

        drop(open_sqlite(&d, KV_BASELINE).expect("seed schema"));

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

        let (v, stale_tables): (String, i64) = new
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
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn equal_epoch_writer_is_not_fenced() {
        let (root, d) = tmp();
        let path = sqlite_path(&d);
        drop(open_sqlite(&d, KV_BASELINE).expect("seed"));
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
