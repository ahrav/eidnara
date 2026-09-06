//! Integration tests for kernel open, fencing, identity refusal, and file modes.
//!
//! Tests verify that conclusive mismatches are refused, inconclusive or
//! foreign families remain untouched, and each successful reopen advances the
//! writer fence.

use kernel::schema::{
    KERNEL_APPLICATION_ID, apply_kernel_connection_profile, apply_kernel_schema,
    kernel_schema_digest,
};
#[cfg(feature = "test-support")]
use kernel::sqlite_runtime::SqliteEngineIdentity;
use kernel::sqlite_runtime::{DIRECT_FORMAT_EPOCH, compute_marker_digest};
use kernel::{KernelError, KernelStore};
use rusqlite::{Connection, OpenFlags};
use std::fs;
use std::path::{Path, PathBuf};

const INCARNATION: &str = "0123456789abcdef0123456789abcdef";

fn core_path(root: &Path) -> PathBuf {
    root.join("kernel.sqlite")
}

fn inspect<T>(root: &Path, query: impl FnOnce(&Connection) -> T) -> T {
    let conn = Connection::open_with_flags(core_path(root), OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open read-only inspection connection");
    query(&conn)
}

fn seed_kernel(root: &Path) -> Connection {
    fs::create_dir_all(root).unwrap();
    let mut conn = Connection::open(core_path(root)).unwrap();
    apply_kernel_connection_profile(&mut conn, 5_000).unwrap();
    apply_kernel_schema(&mut conn, INCARNATION, 1_000).unwrap();
    conn
}

fn marker_digest(epoch: i64, schema_digest: &str) -> String {
    compute_marker_digest(epoch, INCARNATION, schema_digest, 1_000)
}

fn replace_marker(conn: &Connection, epoch: i64, schema_digest: &str) {
    conn.execute_batch(
        "DROP TRIGGER kernel_format_marker_no_update;
         DROP TRIGGER kernel_format_marker_no_delete;",
    )
    .unwrap();
    conn.execute(
        "UPDATE kernel_format_marker
         SET format_epoch=?1, schema_digest=?2, marker_digest=?3",
        (epoch, schema_digest, marker_digest(epoch, schema_digest)),
    )
    .unwrap();
    conn.execute_batch(
        "CREATE TRIGGER kernel_format_marker_no_update
         BEFORE UPDATE ON kernel_format_marker BEGIN
           SELECT RAISE(ABORT, 'kernel_format_marker is immutable');
         END;
         CREATE TRIGGER kernel_format_marker_no_delete
         BEFORE DELETE ON kernel_format_marker BEGIN
           SELECT RAISE(ABORT, 'kernel_format_marker is immutable');
         END;",
    )
    .unwrap();
}

/// A refused open leaves only what every open creates before classification:
/// the lease directory, the artifact layout, and the `-shm` sidecar SQLite
/// needs to read a WAL-mode family. Anything else is a write into the family.
fn assert_refusal_left_only_open_scaffolding(root: &Path) {
    let allowed = [
        root.join("leases"),
        root.join("artifacts"),
        core_path(root),
        PathBuf::from(format!("{}-wal", core_path(root).display())),
        PathBuf::from(format!("{}-shm", core_path(root).display())),
    ];
    let mut stray = fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| !allowed.contains(path))
        .collect::<Vec<_>>();
    stray.sort();
    assert_eq!(stray, Vec::<PathBuf>::new());
}

#[test]
fn fresh_open_and_exact_reopen_preserve_identity_and_advance_fence() {
    let dir = tempfile::tempdir().unwrap();
    let first = KernelStore::open(dir.path()).unwrap();
    let first_epoch = first.lease_epoch();
    // ASCII `EIDN` and format epoch 1, stamped by `open` on a fresh root.
    assert_eq!(
        inspect(dir.path(), |conn| conn.query_row(
            "PRAGMA application_id",
            [],
            |row| row.get::<_, u32>(0)
        ))
        .unwrap(),
        0x4549_444E
    );
    assert_eq!(
        inspect(dir.path(), |conn| conn.query_row(
            "PRAGMA user_version",
            [],
            |row| row.get::<_, i64>(0)
        ))
        .unwrap(),
        1
    );
    let incarnation = inspect(dir.path(), |conn| {
        conn.query_row(
            "SELECT database_incarnation_id FROM kernel_format_marker",
            [],
            |row| row.get::<_, String>(0),
        )
    })
    .unwrap();
    assert_eq!(
        inspect(dir.path(), |conn| conn.query_row(
            "SELECT writer_epoch FROM writer_fence WHERE id=0",
            [],
            |row| row.get::<_, i64>(0)
        ))
        .unwrap(),
        i64::try_from(first_epoch).unwrap()
    );
    drop(first);

    let second = KernelStore::open(dir.path()).unwrap();
    assert!(second.lease_epoch() > first_epoch);
    assert_eq!(
        inspect(dir.path(), |conn| {
            conn.query_row(
                "SELECT database_incarnation_id FROM kernel_format_marker",
                [],
                |row| row.get::<_, String>(0),
            )
        })
        .unwrap(),
        incarnation
    );
}

#[test]
fn second_opener_is_held_without_touching_database_family() {
    let dir = tempfile::tempdir().unwrap();
    let first = KernelStore::open(dir.path()).unwrap();
    let before = fs::read(core_path(dir.path())).unwrap();
    assert_eq!(
        KernelStore::open(dir.path()).unwrap_err(),
        KernelError::Held
    );
    assert_eq!(fs::read(core_path(dir.path())).unwrap(), before);
    drop(first);
}

#[test]
fn every_conclusive_kernel_mismatch_is_refused_and_left_untouched() {
    for mismatch in ["epoch", "user_version", "digest", "inventory"] {
        let dir = tempfile::tempdir().unwrap();
        let conn = seed_kernel(dir.path());
        match mismatch {
            "epoch" => {
                let digest = kernel_schema_digest(&conn).unwrap();
                replace_marker(&conn, 2, &digest);
            }
            "digest" => replace_marker(&conn, 1, &"a".repeat(64)),
            // The marker still says epoch 1 with a valid digest; only the header differs.
            "user_version" => conn.pragma_update(None, "user_version", 7).unwrap(),
            "inventory" => {
                conn.execute_batch("CREATE TABLE unexpected(value INTEGER) STRICT;")
                    .unwrap();
            }
            _ => unreachable!(),
        }
        drop(conn);
        let path = core_path(dir.path());
        fs::write(format!("{}-wal", path.display()), b"mismatched wal").unwrap();
        let before_main = fs::read(&path).unwrap();
        let before_wal = fs::read(format!("{}-wal", path.display())).unwrap();

        assert_eq!(
            KernelStore::open(dir.path()).unwrap_err(),
            KernelError::IdentityMismatch,
            "{mismatch}"
        );
        assert_eq!(fs::read(&path).unwrap(), before_main, "{mismatch}");
        assert_eq!(
            fs::read(format!("{}-wal", path.display())).unwrap(),
            before_wal,
            "{mismatch}"
        );
        assert_refusal_left_only_open_scaffolding(dir.path());
    }
}

/// Bytes that are not a SQLite header are refused as `Foreign` once the file is
/// long enough to hold a header; a shorter file cannot be classified at all.
#[test]
fn non_sqlite_bytes_are_refused_and_left_untouched() {
    for (bytes, expected) in [
        (vec![b'x'; 4096], KernelError::Foreign),
        (b"not a database".to_vec(), KernelError::Inconclusive),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = core_path(dir.path());
        fs::write(&path, &bytes).unwrap();

        assert_eq!(KernelStore::open(dir.path()).unwrap_err(), expected);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_refusal_left_only_open_scaffolding(dir.path());
    }
}

#[test]
fn foreign_family_is_refused_before_sqlite_can_touch_it() {
    // `FOREIGN_APPLICATION_ID` must differ from `KERNEL_APPLICATION_ID` so the
    // fixture exercises foreign-header classification.
    const FOREIGN_APPLICATION_ID: u32 = 0x5A5A_5A5A;
    assert_ne!(FOREIGN_APPLICATION_ID, KERNEL_APPLICATION_ID);

    let dir = tempfile::tempdir().unwrap();
    let path = core_path(dir.path());
    let conn = Connection::open(&path).unwrap();
    conn.pragma_update(None, "application_id", FOREIGN_APPLICATION_ID)
        .unwrap();
    conn.execute_batch("CREATE TABLE legacy(value TEXT);")
        .unwrap();
    drop(conn);
    fs::write(format!("{}-wal", path.display()), b"foreign wal").unwrap();
    let before_main = fs::read(&path).unwrap();
    let before_wal = fs::read(format!("{}-wal", path.display())).unwrap();

    assert_eq!(
        KernelStore::open(dir.path()).unwrap_err(),
        KernelError::Foreign
    );
    assert_eq!(fs::read(&path).unwrap(), before_main);
    assert_eq!(
        fs::read(format!("{}-wal", path.display())).unwrap(),
        before_wal
    );
    assert_refusal_left_only_open_scaffolding(dir.path());
}

/// The kernel restates the workspace-wide `application_id` and `user_version`
/// instead of importing them. Divergence would turn a sibling Eidnara store
/// from `Inconclusive` (schema says so) into `Foreign` (header says so).
#[test]
fn kernel_identity_pragmas_match_every_other_eidnara_store() {
    assert_eq!(KERNEL_APPLICATION_ID, storage::APPLICATION_ID);
    assert_eq!(DIRECT_FORMAT_EPOCH, i64::from(storage::USER_VERSION));
}

#[test]
fn a_sibling_family_with_the_kernel_application_id_is_refused_and_left_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = core_path(dir.path());
    let conn = Connection::open(&path).unwrap();
    conn.pragma_update(None, "application_id", storage::APPLICATION_ID)
        .unwrap();
    conn.pragma_update(None, "user_version", storage::USER_VERSION)
        .unwrap();
    conn.execute_batch("CREATE TABLE legacy(value TEXT);")
        .unwrap();
    drop(conn);
    let before_main = fs::read(&path).unwrap();

    assert_eq!(
        KernelStore::open(dir.path()).unwrap_err(),
        KernelError::Inconclusive
    );
    assert_eq!(fs::read(&path).unwrap(), before_main);
    assert_refusal_left_only_open_scaffolding(dir.path());
}

#[test]
fn malformed_marker_is_inconclusive_and_untouched() {
    // "g" fails the lowercase-hex check; the all-zero digest fails digest
    // comparison; the third case leaves the digest intact and edits a field it
    // covers, so the digest stops matching its own row.
    for tamper in [
        "UPDATE kernel_format_marker SET marker_digest='gggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggg'",
        "UPDATE kernel_format_marker SET marker_digest='0000000000000000000000000000000000000000000000000000000000000000'",
        "UPDATE kernel_format_marker SET format_epoch=2",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let conn = seed_kernel(dir.path());
        conn.execute_batch("DROP TRIGGER kernel_format_marker_no_update;")
            .unwrap();
        conn.execute(tamper, []).unwrap();
        drop(conn);
        let path = core_path(dir.path());
        let before = fs::read(&path).unwrap();

        assert_eq!(
            KernelStore::open(dir.path()).unwrap_err(),
            KernelError::Inconclusive,
            "{tamper}"
        );
        assert_eq!(fs::read(&path).unwrap(), before, "{tamper}");
        assert_refusal_left_only_open_scaffolding(dir.path());
    }
}

#[test]
fn writer_and_sidecars_are_owner_only() {
    let dir = tempfile::tempdir().unwrap();
    let store = KernelStore::open(dir.path()).unwrap();
    let path = core_path(dir.path());
    assert_owner_only(&path, 0o600);
    assert_owner_only(&PathBuf::from(format!("{}-wal", path.display())), 0o600);
    assert_owner_only(&PathBuf::from(format!("{}-shm", path.display())), 0o600);
    assert_eq!(
        inspect(dir.path(), |conn| conn.query_row(
            "PRAGMA journal_mode",
            [],
            |row| row.get::<_, String>(0)
        ))
        .unwrap(),
        "wal"
    );
    drop(store);
}

#[cfg(feature = "test-support")]
#[test]
fn unsupported_engine_is_rejected_before_creating_lease_or_database_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("kernel");
    let identity = SqliteEngineIdentity {
        sqlite_version: "3.51.2".to_string(),
        sqlite_source_id: "2026-01-01 00:00:00 0123456789abcdef0123456789abcdef01234567"
            .to_string(),
    };
    assert_eq!(
        KernelStore::open_with_engine_identity_for_test(&root, &identity).unwrap_err(),
        KernelError::EngineUnsupported
    );
    assert!(!root.exists());
}

#[test]
fn kernel_with_uncheckpointed_wal_opens_and_preserves_rows() {
    let dir = tempfile::tempdir().unwrap();
    drop(KernelStore::open(dir.path()).unwrap());

    let conn = Connection::open(core_path(dir.path())).unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    conn.execute(
        "INSERT INTO commit_log(
             transaction_id,writer_epoch,producer,operation_key,request_digest,
             recorded_at,actor,cause
         ) VALUES('t1',1,'fixture','t1','',1,'actor','cause')",
        [],
    )
    .unwrap();
    let wal = PathBuf::from(format!("{}-wal", core_path(dir.path()).display()));
    assert!(wal.is_file(), "the row should still be in the WAL");
    // Leaking skips the clean close that would checkpoint and remove the WAL.
    // A crashed writer leaves the uncheckpointed WAL on disk.
    std::mem::forget(conn);

    let _store = KernelStore::open(dir.path()).unwrap();
    assert_eq!(
        inspect(dir.path(), |conn| conn.query_row(
            "SELECT COUNT(*) FROM commit_log",
            [],
            |row| row.get::<_, i64>(0)
        ))
        .unwrap(),
        1
    );
}

#[cfg(unix)]
fn assert_owner_only(path: &Path, expected: u32) {
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        expected
    );
}

#[cfg(not(unix))]
fn assert_owner_only(_path: &Path, _expected: u32) {}
