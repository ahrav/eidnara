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
use kernel::{KernelError, KernelStore, OpenPhase};
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
        wal_path(root),
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

fn wal_path(root: &Path) -> PathBuf {
    PathBuf::from(format!("{}-wal", core_path(root).display()))
}

/// SQLite WAL-mode access replays or truncates the log, so a sentinel `-wal`
/// makes any such write observable through `assert_family_bytes_unchanged`.
fn write_wal_sentinel_and_snapshot(root: &Path, sentinel: &[u8]) -> (Vec<u8>, Vec<u8>) {
    fs::write(wal_path(root), sentinel).unwrap();
    let before_main = fs::read(core_path(root)).unwrap();
    let before_wal = fs::read(wal_path(root)).unwrap();
    (before_main, before_wal)
}

fn assert_family_bytes_unchanged(
    root: &Path,
    before_main: &[u8],
    before_wal: &[u8],
    context: &str,
) {
    assert_eq!(fs::read(core_path(root)).unwrap(), before_main, "{context}");
    assert_eq!(fs::read(wal_path(root)).unwrap(), before_wal, "{context}");
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
    for mismatch in ["epoch", "user_version", "digest", "extra_object"] {
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
            // An extra table changes the schema digest, so this arm exercises the
            // digest conjunct; the inventory conjunct cannot fail on its own here.
            "extra_object" => {
                conn.execute_batch("CREATE TABLE unexpected(value INTEGER) STRICT;")
                    .unwrap();
            }
            _ => unreachable!(),
        }
        drop(conn);
        let (before_main, before_wal) =
            write_wal_sentinel_and_snapshot(dir.path(), b"mismatched wal");

        assert_eq!(
            KernelStore::open(dir.path()).unwrap_err(),
            KernelError::IdentityMismatch,
            "{mismatch}"
        );
        assert_family_bytes_unchanged(dir.path(), &before_main, &before_wal, mismatch);
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
        fs::write(core_path(dir.path()), &bytes).unwrap();
        let (before_main, before_wal) =
            write_wal_sentinel_and_snapshot(dir.path(), b"non-sqlite wal");
        assert_eq!(before_main, bytes);

        assert_eq!(KernelStore::open(dir.path()).unwrap_err(), expected);
        assert_family_bytes_unchanged(dir.path(), &before_main, &before_wal, "non-sqlite");
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
    let (before_main, before_wal) = write_wal_sentinel_and_snapshot(dir.path(), b"foreign wal");

    assert_eq!(
        KernelStore::open(dir.path()).unwrap_err(),
        KernelError::Foreign
    );
    assert_family_bytes_unchanged(dir.path(), &before_main, &before_wal, "foreign");
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
    let (before_main, before_wal) = write_wal_sentinel_and_snapshot(dir.path(), b"sibling wal");

    assert_eq!(
        KernelStore::open(dir.path()).unwrap_err(),
        KernelError::Inconclusive
    );
    assert_family_bytes_unchanged(dir.path(), &before_main, &before_wal, "sibling");
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
        let (before_main, before_wal) =
            write_wal_sentinel_and_snapshot(dir.path(), b"malformed marker wal");

        assert_eq!(
            KernelStore::open(dir.path()).unwrap_err(),
            KernelError::Inconclusive,
            "{tamper}"
        );
        assert_family_bytes_unchanged(dir.path(), &before_main, &before_wal, tamper);
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

/// A store root renamed away and replaced with another store between the lease
/// and the first write must not have the replacement stamped with this open's
/// epoch. The hook runs after the writer connection is open and before the
/// fence is written, which is the window a pathname-resolved open leaves.
#[test]
fn a_root_replaced_before_the_first_write_is_refused_before_it_is_stamped() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("store");
    // Two stores that both exist on disk. `other` is the one that will be
    // swapped into `root`'s pathname.
    drop(KernelStore::open(&root).unwrap());
    let other = parent.path().join("other");
    drop(KernelStore::open(&other).unwrap());
    let other_epoch_before: i64 = inspect(&other, |conn| {
        conn.query_row(
            "SELECT writer_epoch FROM writer_fence WHERE id=0",
            [],
            |row| row.get(0),
        )
        .unwrap()
    });
    let moved = parent.path().join("moved-away");

    let error = KernelStore::open_with_hook_for_test(&root, |phase| {
        if phase == OpenPhase::BeforeFirstWrite {
            std::fs::rename(&root, &moved).unwrap();
            std::fs::rename(&other, &root).unwrap();
        }
    })
    .unwrap_err();
    assert_eq!(error, KernelError::Io);

    // The replacement's fence is exactly what it was: nothing was stamped
    // through a connection the held root does not cover.
    let other_epoch_after: i64 = inspect(&root, |conn| {
        conn.query_row(
            "SELECT writer_epoch FROM writer_fence WHERE id=0",
            [],
            |row| row.get(0),
        )
        .unwrap()
    });
    assert_eq!(other_epoch_after, other_epoch_before);
    // Both stores still open on their own once the pathnames settle.
    std::fs::rename(&root, &other).unwrap();
    std::fs::rename(&moved, &root).unwrap();
    drop(KernelStore::open(&root).unwrap());
    drop(KernelStore::open(&other).unwrap());
}

/// A pristine root replaced by an empty owner-only directory before the
/// database is created must not have a database bootstrapped into the
/// replacement: the file is created below the held root, and SQLite must find
/// exactly that file before any schema is written.
#[test]
fn a_pristine_root_replaced_before_bootstrap_receives_no_database() {
    use std::os::unix::fs::PermissionsExt;
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("store");
    let other = parent.path().join("other");
    std::fs::create_dir(&other).unwrap();
    std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o700)).unwrap();
    let moved = parent.path().join("moved-away");

    let error = KernelStore::open_with_hook_for_test(&root, |phase| {
        if phase == OpenPhase::BeforeDatabaseOpen {
            std::fs::rename(&root, &moved).unwrap();
            std::fs::rename(&other, &root).unwrap();
        }
    })
    .unwrap_err();
    assert_eq!(error, KernelError::Io);

    // The replacement holds no database of any size: not bootstrapped, not even
    // created empty by a SQLite open.
    assert!(
        !root.join("kernel.sqlite").exists(),
        "a database was created in the replacement directory"
    );
    // The held root's own pristine file is what a later open bootstraps.
    std::fs::rename(&root, &other).unwrap();
    std::fs::rename(&moved, &root).unwrap();
    drop(KernelStore::open(&root).unwrap());
    assert!(root.join("kernel.sqlite").metadata().unwrap().len() > 0);
}

/// The `leases` child is a mutable pathname. Renaming it beside a running
/// writer lets a second opener acquire a fresh lease, and fresh leases start
/// at the same epoch. The durable fence is what the two cannot share: the
/// second opener's lease must exceed the fence the database carries, and its
/// stamp then fences the first writer out.
#[test]
fn a_swapped_lease_directory_cannot_seat_a_second_writer_beside_the_first() {
    let dir = tempfile::tempdir().unwrap();
    let first = KernelStore::open(dir.path()).unwrap();
    let fence_before = inspect(dir.path(), |conn| {
        conn.query_row(
            "SELECT writer_epoch FROM writer_fence WHERE id=0",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
    });

    // Same-UID actor moves the lease namespace aside; a second opener finds an
    // empty one and creates a lease whose epoch would otherwise repeat.
    fs::rename(dir.path().join("leases"), dir.path().join("leases-moved")).unwrap();
    let second = KernelStore::open(dir.path()).unwrap();
    let fence_after = inspect(dir.path(), |conn| {
        conn.query_row(
            "SELECT writer_epoch FROM writer_fence WHERE id=0",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
    });
    assert!(
        fence_after > fence_before,
        "the second opener must raise the fence, not repeat it: {fence_before} -> {fence_after}"
    );

    // Exactly one of them writes: the first is fenced out on its next commit.
    let intent = |key: &str| kernel::CommitIntent {
        producer: "kernel-open-test".to_string(),
        operation_key: key.to_string(),
        request_digest: "a".repeat(64),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    };
    let fenced = first
        .commit(intent("first-after-swap"), |_| Ok(String::new()))
        .unwrap_err();
    assert_eq!(fenced, KernelError::FenceLost);
    second
        .commit(intent("second-after-swap"), |_| Ok(String::new()))
        .unwrap();
    drop(second);
    drop(first);
}

/// A bootstrap keeps the descriptor of the file it created, so a file swapped
/// under the same name before SQLite opens it is refused before any schema is
/// written into it.
#[test]
fn a_database_file_swapped_during_bootstrap_receives_no_schema() {
    use std::os::unix::fs::PermissionsExt;
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("store");
    let planted = parent.path().join("planted.sqlite");
    fs::write(&planted, b"").unwrap();
    fs::set_permissions(&planted, fs::Permissions::from_mode(0o600)).unwrap();

    let error = KernelStore::open_with_hook_for_test(&root, |phase| {
        if phase == OpenPhase::AfterDatabaseCreated {
            fs::rename(
                root.join("kernel.sqlite"),
                parent.path().join("created-moved"),
            )
            .unwrap();
            fs::rename(&planted, root.join("kernel.sqlite")).unwrap();
        }
    })
    .unwrap_err();
    assert_eq!(error, KernelError::Io);
    assert_eq!(
        fs::metadata(root.join("kernel.sqlite")).unwrap().len(),
        0,
        "the planted file received a schema"
    );
}
