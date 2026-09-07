#![cfg(feature = "test-support")]

use std::cell::Cell;
use std::fs::{self, OpenOptions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use kernel::filesystem_is_unsafe_for_test;
use kernel::schema::apply_kernel_connection_profile;
use kernel::{
    BackupRequest, CommitIntent, DomainSpec, KernelError, KernelStore, RestoreFault, Sensitivity,
    owner_is_current_for_test, sensitivity_bearing_tables_for_test,
    verify_backup_with_deadline_for_test,
};
use rusqlite::{Connection, params};

#[path = "support/canonical_state.rs"]
mod canonical_state;

use canonical_state::{Profile, digest};

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "kernel-backup-test".to_string(),
        operation_key: key.to_string(),
        request_digest: "a".repeat(64),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

fn domain(index: i64, sensitivity: Sensitivity) -> DomainSpec {
    DomainSpec {
        domain_id: format!("domain-{index}"),
        object_id: format!("object-{index}"),
        name: format!("name-{index}"),
        source_kind: "fixture".to_string(),
        source_id: format!("source-{index}"),
        source_revision: index,
        sensitivity,
    }
}

fn private_dir() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    directory
}

fn request(destination: &Path) -> BackupRequest {
    BackupRequest {
        destination_directory: destination.to_path_buf(),
        deadline: Instant::now() + Duration::from_secs(30),
        capture_pin_expires_at: None,
    }
}

fn insert_domain(store: &KernelStore, index: i64, sensitivity: Sensitivity) -> i64 {
    store
        .commit(intent(&format!("domain-{index}")), |envelope| {
            envelope.insert_domain(domain(index, sensitivity))?;
            Ok("stored".to_string())
        })
        .unwrap()
        .commit_seq
}

fn inspect(root: &Path) -> Connection {
    Connection::open(root.join("kernel.sqlite")).unwrap()
}

fn seed_evidence(root: &Path, evidence_id: &str, sensitivity: &str) {
    let mut connection = inspect(root);
    apply_kernel_connection_profile(&mut connection, 5_000).unwrap();
    let tx = connection.transaction().unwrap();
    let epoch: i64 = tx
        .query_row(
            "SELECT writer_epoch FROM writer_fence WHERE id=0",
            [],
            |row| row.get(0),
        )
        .unwrap();
    tx.execute(
        "INSERT INTO commit_log(
             transaction_id,writer_epoch,producer,operation_key,request_digest,
             recorded_at,actor,cause
         ) VALUES (?1,?2,'fixture',?1,'digest',1,'test','evidence')",
        params![format!("evidence-{evidence_id}"), epoch],
    )
    .unwrap();
    let commit_seq = tx.last_insert_rowid();
    tx.execute(
        "INSERT INTO object_registry(
             object_id,object_kind,domain_id,source_kind,source_id,source_revision,
             created_commit_seq,sensitivity_class
         ) VALUES (?1,'evidence','domain-1','fixture',?1,1,?2,?3)",
        params![
            format!("evidence-object-{evidence_id}"),
            commit_seq,
            sensitivity
        ],
    )
    .unwrap();
    tx.execute(
        "INSERT INTO evidence_meta(
             evidence_id,object_id,artifact_reference,artifact_digest,byte_length,media_type,
             retention_class,provider_egress_class,redaction_metadata,created_commit_seq,
             sensitivity_class
         ) VALUES (?1,?2,'local','digest',1,'text/plain','durable','local',X'',?3,?4)",
        params![
            evidence_id,
            format!("evidence-object-{evidence_id}"),
            commit_seq,
            sensitivity,
        ],
    )
    .unwrap();
    tx.commit().unwrap();
}

fn destination_entries(path: &Path) -> Vec<PathBuf> {
    fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect()
}

fn unix_time_ms() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

#[test]
fn fresh_store_backup_has_zero_tip_and_no_capture_pin() {
    let root = private_dir();
    let destination = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    let backup = store.backup(request(destination.path())).unwrap();
    assert_eq!(backup.captured_commit_seq, 0);
    assert!(backup.evidence_refs.is_empty());
    assert_eq!(backup.capture_pin_id, None);
    assert_eq!(destination_entries(destination.path()).len(), 1);
}

#[test]
fn backup_restores_exact_snapshot_and_reclaims_writer_fence() {
    let root = private_dir();
    let destination = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    assert_eq!(insert_domain(&store, 1, Sensitivity::Normal), 1);
    store
        .commit(intent("consumer"), |envelope| {
            envelope.register_outbox_consumer("restore-oracle", 1)?;
            Ok("registered".to_string())
        })
        .unwrap();
    let expected = store.known_as_of(2).unwrap();
    let backup = store.backup(request(destination.path())).unwrap();
    let expected_oracle = digest(root.path(), Profile::SameRoot);
    assert_eq!(backup.captured_commit_seq, 2);
    assert!(
        backup
            .destination_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("-2-")
    );
    assert_eq!(
        fs::metadata(&backup.destination_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );

    insert_domain(&store, 2, Sensitivity::Normal);
    assert_eq!(
        store
            .restore_with_hook_for_test(&backup.destination_path, || {
                assert!(!root.path().join("kernel.sqlite").exists());
                assert_eq!(
                    fs::metadata(root.path().join("kernel.sqlite.restore"))
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o600
                );
                assert_eq!(
                    KernelStore::open(root.path()).unwrap_err(),
                    KernelError::Held
                );
            })
            .unwrap(),
        2
    );
    assert_eq!(store.known_as_of(2).unwrap(), expected);
    digest(root.path(), Profile::SameRoot).assert_same(&expected_oracle, "restored state");
    assert_eq!(
        store.known_as_of(3).unwrap_err(),
        KernelError::FutureSnapshot
    );
    assert_eq!(insert_domain(&store, 3, Sensitivity::Normal), 3);
    assert!(!root.path().join("kernel.sqlite.restore").exists());
    assert_eq!(
        KernelStore::open(root.path()).unwrap_err(),
        KernelError::Held
    );
}

#[test]
fn queued_writer_waits_for_backup_coordinator_then_proceeds() {
    let root = private_dir();
    let destination = private_dir();
    let store = Arc::new(KernelStore::open(root.path()).unwrap());
    insert_domain(&store, 1, Sensitivity::Normal);
    let backup_store = Arc::clone(&store);
    let (locked_tx, locked_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let backup_destination = destination.path().to_path_buf();
    let backup_thread = std::thread::spawn(move || {
        backup_store.backup_with_hook_for_test(request(&backup_destination), || {
            locked_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        })
    });
    locked_rx.recv().unwrap();

    let writer_store = Arc::clone(&store);
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let writer_thread = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        done_tx
            .send(insert_domain(&writer_store, 2, Sensitivity::Normal))
            .unwrap();
    });
    started_rx.recv().unwrap();
    assert!(matches!(
        done_rx.recv_timeout(Duration::from_millis(300)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    release_tx.send(()).unwrap();
    assert!(backup_thread.join().unwrap().is_ok());
    assert_eq!(done_rx.recv().unwrap(), 2);
    writer_thread.join().unwrap();
}

#[test]
fn timeout_and_pre_publish_fault_leave_no_artifacts_and_retry_succeeds() {
    let root = private_dir();
    let destination = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Normal);
    // The hook sleeps past the 200 ms deadline, so expiry occurs mid-flight.
    // `hook_ran` fails the test if `secure_destination` skips the hook.
    let expired = BackupRequest {
        destination_directory: destination.path().to_path_buf(),
        deadline: Instant::now() + Duration::from_millis(200),
        capture_pin_expires_at: None,
    };
    let hook_ran = Cell::new(false);
    assert_eq!(
        store
            .backup_with_hook_for_test(expired, || {
                hook_ran.set(true);
                let entries = destination_entries(destination.path());
                assert_eq!(entries.len(), 1);
                assert!(
                    entries[0]
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .ends_with(".tmp")
                );
                std::thread::sleep(Duration::from_millis(250));
            })
            .unwrap_err(),
        KernelError::Deadline
    );
    assert!(hook_ran.get(), "mid-flight assertions never executed");
    assert!(destination_entries(destination.path()).is_empty());
    assert_eq!(
        store
            .backup_with_fault_before_rename_for_test(request(destination.path()))
            .unwrap_err(),
        KernelError::Fault
    );
    assert!(destination_entries(destination.path()).is_empty());
    assert!(store.backup(request(destination.path())).is_ok());
    assert_eq!(destination_entries(destination.path()).len(), 1);
}

#[test]
fn backup_deadline_interrupts_verification_sql() {
    let root = private_dir();
    let destination = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Normal);
    let backup = store.backup(request(destination.path())).unwrap();

    assert_eq!(
        verify_backup_with_deadline_for_test(
            &backup.destination_path,
            backup.captured_commit_seq,
            Instant::now() - Duration::from_millis(1),
        )
        .unwrap_err(),
        KernelError::Deadline
    );
}

#[test]
fn backup_name_collision_preserves_preexisting_file() {
    let root = private_dir();
    let destination = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Normal);
    let final_name = "preexisting.sqlite";
    let final_path = destination.path().join(final_name);
    let original = b"must remain unchanged";
    fs::write(&final_path, original).unwrap();
    fs::set_permissions(&final_path, fs::Permissions::from_mode(0o600)).unwrap();

    assert_eq!(
        store
            .backup_with_final_name_for_test(request(destination.path()), final_name)
            .unwrap_err(),
        KernelError::Io
    );
    assert_eq!(fs::read(&final_path).unwrap(), original);
    assert_eq!(destination_entries(destination.path()), [final_path]);
}

#[test]
fn evidence_manifest_pin_release_and_stale_reap_are_typed() {
    let root = private_dir();
    let destination = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Normal);
    let normal_backup = store.backup(request(destination.path())).unwrap();
    assert_eq!(normal_backup.max_sensitivity, Sensitivity::Normal);
    drop(store);
    seed_evidence(root.path(), "z-evidence", "sensitive");
    seed_evidence(root.path(), "a-evidence", "normal");
    let store = KernelStore::open(root.path()).unwrap();
    assert_eq!(
        store
            .backup_with_fault_before_rename_for_test(request(destination.path()))
            .unwrap_err(),
        KernelError::Fault
    );
    assert_eq!(
        inspect(root.path())
            .query_row("SELECT COUNT(*) FROM capture_pins", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
    assert_eq!(
        inspect(root.path())
            .query_row("SELECT COUNT(*) FROM capture_pin_refs", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
    let backup = store.backup(request(destination.path())).unwrap();
    assert_eq!(
        backup.evidence_refs,
        ["a-evidence".to_string(), "z-evidence".to_string()]
    );
    assert_eq!(backup.max_sensitivity, Sensitivity::Sensitive);
    let pin = backup.capture_pin_id.unwrap();
    assert_eq!(
        inspect(root.path())
            .query_row(
                "SELECT COUNT(*) FROM capture_pin_refs
                 WHERE capture_pin_id=?1 AND released_at IS NULL",
                [&pin],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        2
    );
    let mut pinned_connection = inspect(root.path());
    apply_kernel_connection_profile(&mut pinned_connection, 5_000).unwrap();
    assert!(
        pinned_connection
            .execute(
                "DELETE FROM evidence_meta WHERE evidence_id='a-evidence'",
                [],
            )
            .is_err()
    );
    store.release_capture_pin(&pin, 10).unwrap();
    assert_eq!(
        inspect(root.path())
            .query_row(
                "SELECT COUNT(*) FROM capture_pin_refs
                 WHERE capture_pin_id=?1 AND released_at=10",
                [&pin],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        2
    );
    drop(pinned_connection);

    assert_eq!(
        store
            .backup(BackupRequest {
                capture_pin_expires_at: Some(0),
                ..request(destination.path())
            })
            .unwrap_err(),
        KernelError::InvalidInput
    );
    let now = unix_time_ms();
    let stale = store
        .backup(BackupRequest {
            capture_pin_expires_at: Some(now + 60_000),
            ..request(destination.path())
        })
        .unwrap()
        .capture_pin_id
        .unwrap();
    let non_stale = store
        .backup(BackupRequest {
            capture_pin_expires_at: Some(now + 120_000),
            ..request(destination.path())
        })
        .unwrap()
        .capture_pin_id
        .unwrap();
    assert_eq!(
        inspect(root.path())
            .query_row(
                "SELECT expires_at FROM capture_pins WHERE capture_pin_id=?1",
                [&non_stale],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        now + 120_000
    );
    let defaulted = store
        .backup(request(destination.path()))
        .unwrap()
        .capture_pin_id
        .unwrap();
    let default_lifetime: i64 = inspect(root.path())
        .query_row(
            "SELECT expires_at-created_at FROM capture_pins WHERE capture_pin_id=?1",
            [&defaulted],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(default_lifetime, 24 * 60 * 60 * 1_000);
    drop(store);
    let reopened = KernelStore::open(root.path()).unwrap();
    reopened.run_capture_pin_maintenance(now + 90_000).unwrap();
    assert!(
        inspect(root.path())
            .query_row(
                "SELECT released_at IS NOT NULL FROM capture_pins WHERE capture_pin_id=?1",
                [&stale],
                |row| row.get::<_, bool>(0),
            )
            .unwrap()
    );
    for live in [&non_stale, &defaulted] {
        assert!(
            !inspect(root.path())
                .query_row(
                    "SELECT released_at IS NOT NULL FROM capture_pins WHERE capture_pin_id=?1",
                    [live],
                    |row| row.get::<_, bool>(0),
                )
                .unwrap()
        );
    }
    assert_eq!(
        inspect(root.path())
            .query_row(
                "SELECT COUNT(*) FROM capture_pin_refs
                 WHERE capture_pin_id=?1 AND released_at=?2",
                rusqlite::params![stale, now + 90_000],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        2
    );
}

#[test]
fn staging_sensitivity_marks_backup_sensitive() {
    let root = private_dir();
    let destination = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    store
        .stage_candidate(kernel::StagingCandidateSpec {
            extraction_run_id: "sensitive-run".to_string(),
            candidate_id: "sensitive-candidate".to_string(),
            extractor: "fixture".to_string(),
            source_kind: "fixture".to_string(),
            source_id: "source".to_string(),
            source_revision: 1,
            candidate_kind: "fact".to_string(),
            payload: "payload".to_string(),
            provenance: None,
            recorded_at: 1,
            lease_expires_at: 2,
        })
        .unwrap();
    assert_eq!(
        store
            .backup(request(destination.path()))
            .unwrap()
            .max_sensitivity,
        Sensitivity::Sensitive
    );
}

#[test]
fn unsafe_destinations_and_restore_sources_are_rejected_before_live_touch() {
    let root = private_dir();
    let destination = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Normal);

    let file_destination = destination.path().join("file");
    fs::write(&file_destination, b"not a directory").unwrap();
    assert_eq!(
        store.backup(request(&file_destination)).unwrap_err(),
        KernelError::UnsafeDestination
    );
    let symlink_destination = destination.path().join("link");
    std::os::unix::fs::symlink(root.path(), &symlink_destination).unwrap();
    assert_eq!(
        store.backup(request(&symlink_destination)).unwrap_err(),
        KernelError::UnsafeDestination
    );
    let broad_destination = private_dir();
    fs::set_permissions(broad_destination.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        store.backup(request(broad_destination.path())).unwrap_err(),
        KernelError::UnsafeDestination
    );
    #[cfg(target_os = "linux")]
    {
        assert!(filesystem_is_unsafe_for_test(0x6969));
        assert!(filesystem_is_unsafe_for_test(0x6573_5546));
        assert!(filesystem_is_unsafe_for_test(0x1234_5678));
        assert!(!filesystem_is_unsafe_for_test(0x794c_7630));
    }
    #[cfg(target_os = "macos")]
    {
        for remote in ["nfs", "smbfs", "osxfuse", "macfuse", "webdav"] {
            assert!(kernel::filesystem_name_is_unsafe_for_test(remote));
        }
        for local in ["apfs", "hfs", "tmpfs"] {
            assert!(!kernel::filesystem_name_is_unsafe_for_test(local));
        }
    }
    let current_uid = rustix::process::geteuid().as_raw();
    assert!(owner_is_current_for_test(current_uid));
    assert!(!owner_is_current_for_test(current_uid.wrapping_add(1)));

    let backup = store.backup(request(destination.path())).unwrap();
    let source_symlink = destination.path().join("source-link.sqlite");
    std::os::unix::fs::symlink(&backup.destination_path, &source_symlink).unwrap();
    assert_eq!(
        store.restore(&source_symlink).unwrap_err(),
        KernelError::InvalidRestore
    );
    assert_eq!(
        store.restore(destination.path()).unwrap_err(),
        KernelError::InvalidRestore
    );
    let corrupt = destination.path().join(
        backup
            .destination_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .replace(".sqlite", "-corrupt.sqlite"),
    );
    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    std::io::Write::write_all(&mut options.open(&corrupt).unwrap(), b"truncated").unwrap();
    let hook_called = Cell::new(false);
    assert_eq!(
        store
            .restore_with_hook_for_test(&corrupt, || hook_called.set(true))
            .unwrap_err(),
        KernelError::InvalidRestore
    );
    assert!(!hook_called.get());
    assert_eq!(store.known_as_of(1).unwrap().tip, 1);
    assert_eq!(insert_domain(&store, 2, Sensitivity::Normal), 2);
}

#[test]
fn failed_restore_after_displacement_recovers_live_family() {
    let root = private_dir();
    let destination = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Normal);
    let backup = store.backup(request(destination.path())).unwrap();
    insert_domain(&store, 2, Sensitivity::Normal);
    assert_eq!(
        store
            .restore_with_fault_for_test(&backup.destination_path, RestoreFault::AfterDisplace,)
            .unwrap_err(),
        KernelError::Fault
    );
    assert_eq!(store.known_as_of(2).unwrap().tip, 2);
    assert_eq!(insert_domain(&store, 3, Sensitivity::Normal), 3);
    assert!(!root.path().join("kernel.sqlite.restore").exists());
}

#[test]
fn restore_rejects_lost_fence_before_displacement() {
    let root = private_dir();
    let destination = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Normal);
    let backup = store.backup(request(destination.path())).unwrap();
    insert_domain(&store, 2, Sensitivity::Normal);
    store.invalidate_writer_fence_for_test().unwrap();

    assert_eq!(
        store.restore(&backup.destination_path).unwrap_err(),
        KernelError::FenceLost
    );
    assert_eq!(store.facts(0).unwrap().commit_seq, 2);
    assert!(!root.path().join("kernel.sqlite.restore").exists());
    assert!(!fs::read_dir(root.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".restore-")
    }));
}

#[test]
fn restore_into_fresh_store_family_allows_immediate_mutation() {
    let source_root = private_dir();
    let destination = private_dir();
    let source = KernelStore::open(source_root.path()).unwrap();
    insert_domain(&source, 1, Sensitivity::Normal);
    let backup = source.backup(request(destination.path())).unwrap();

    let fresh_root = private_dir();
    assert!(!fresh_root.path().join("leases").exists());
    let initial = KernelStore::open(fresh_root.path()).unwrap();
    let initial_epoch = initial.lease_epoch();
    drop(initial);
    let fresh = KernelStore::open(fresh_root.path()).unwrap();
    assert_ne!(fresh.lease_epoch(), initial_epoch);
    let source_epoch: i64 = Connection::open(&backup.destination_path)
        .unwrap()
        .query_row(
            "SELECT writer_epoch FROM writer_fence WHERE id=0",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_ne!(source_epoch, i64::try_from(fresh.lease_epoch()).unwrap());
    let renamed = destination.path().join("owner-only-renamed.sqlite");
    fs::rename(&backup.destination_path, &renamed).unwrap();
    assert_eq!(fresh.restore(&renamed).unwrap(), 1);
    let restored_epoch: i64 = inspect(fresh_root.path())
        .query_row(
            "SELECT writer_epoch FROM writer_fence WHERE id=0",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(restored_epoch, i64::try_from(fresh.lease_epoch()).unwrap());
    assert_eq!(insert_domain(&fresh, 2, Sensitivity::Normal), 2);
}

#[test]
fn unrecoverable_restore_poisons_the_handle_then_reopen_rolls_the_family_back() {
    let root = private_dir();
    let destination = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Normal);
    let backup = store.backup(request(destination.path())).unwrap();
    insert_domain(&store, 2, Sensitivity::Normal);
    let live_oracle = digest(root.path(), Profile::SameRoot);

    assert_eq!(
        store
            .restore_with_fault_for_test(&backup.destination_path, RestoreFault::RecoveryFailure,)
            .unwrap_err(),
        KernelError::InvalidRestore
    );
    assert_eq!(
        store.known_as_of(1).unwrap_err(),
        KernelError::InvalidRestore
    );
    assert_eq!(
        insert_domain_result(&store, 3).unwrap_err(),
        KernelError::InvalidRestore
    );
    assert!(fs::read_dir(root.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".restore-")
    }));
    assert!(root.path().join("kernel.sqlite.restore").exists());
    assert!(!root.path().join("kernel.sqlite").exists());
    drop(store);

    let reopened = KernelStore::open(root.path()).unwrap();
    assert!(root.path().join("kernel.sqlite").exists());
    assert!(!root.path().join("kernel.sqlite.restore").exists());
    assert!(!fs::read_dir(root.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".restore-")
    }));
    assert_eq!(reopened.facts(1).unwrap().commit_seq, 2);
    digest(root.path(), Profile::SameRoot).assert_same(&live_oracle, "live state");
    assert_eq!(insert_domain(&reopened, 3, Sensitivity::Normal), 3);
}

fn insert_domain_result(store: &KernelStore, index: i64) -> Result<i64, kernel::KernelError> {
    store
        .commit(intent(&format!("domain-{index}")), |envelope| {
            envelope.insert_domain(domain(index, Sensitivity::Normal))?;
            Ok("stored".to_string())
        })
        .map(|result| result.commit_seq)
}

#[test]
#[ignore = "writes about 3 GiB; run with: cargo test -p kernel --features test-support --test kernel_backup threshold_size_restore_rto -- --ignored --nocapture"]
fn threshold_size_restore_rto() {
    let root = private_dir();
    let destination = private_dir();
    // The fixture, its backup and the restore copy are each about 1 GiB.
    for area in [root.path(), destination.path()] {
        let available = rustix::fs::statvfs(area).unwrap();
        let free = available.f_bavail * available.f_frsize;
        assert!(
            free >= 4 * 1024 * 1024 * 1024,
            "{} has {free} bytes free; this proof writes about 3 GiB",
            area.display()
        );
    }
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Normal);
    drop(store);
    let mut connection = inspect(root.path());
    apply_kernel_connection_profile(&mut connection, 5_000).unwrap();
    connection
        .pragma_update(None, "synchronous", "OFF")
        .unwrap();
    let tx = connection.transaction().unwrap();
    for ordinal in 0..1024_i64 {
        tx.execute(
            "INSERT INTO outbox(
                 commit_seq,ordinal,object_id,object_kind,source_kind,source_id,
                 source_revision,sensitivity_class,payload,created_at
             ) VALUES (1,?1,?2,'fixture','fixture',?2,1,'normal',zeroblob(1048576),1)",
            params![ordinal + 1, format!("bulk-{ordinal}")],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    drop(connection);
    assert!(
        fs::metadata(root.path().join("kernel.sqlite"))
            .unwrap()
            .len()
            >= 1024 * 1024 * 1024
    );
    let store = KernelStore::open(root.path()).unwrap();
    // The shared `request` helper allows 30 s, which a 1 GiB copy plus a full
    // integrity check can exceed on constrained I/O.
    let rto_request = BackupRequest {
        destination_directory: destination.path().to_path_buf(),
        deadline: Instant::now() + Duration::from_secs(300),
        capture_pin_expires_at: None,
    };
    let backup = store.backup(rto_request).unwrap();
    let started = Instant::now();
    store.restore(&backup.destination_path).unwrap();
    // `restore_elapsed` excludes backup time and the post-restore assertions.
    let restore_elapsed = started.elapsed();
    assert_eq!(store.known_as_of(1).unwrap().tip, 1);
    assert_eq!(insert_domain(&store, 2, Sensitivity::Normal), 2);
    assert!(
        restore_elapsed <= Duration::from_secs(300),
        "restore took {restore_elapsed:?}"
    );
}

fn header_write_read_versions(path: &Path) -> (u8, u8) {
    use std::io::Read;
    let mut header = [0u8; 20];
    fs::File::open(path)
        .unwrap()
        .read_exact(&mut header)
        .unwrap();
    (header[18], header[19])
}

#[test]
fn published_artifact_is_self_contained_and_restores_from_read_only_media() {
    let root = private_dir();
    let destination = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Normal);
    let backup = store.backup(request(destination.path())).unwrap();

    assert_eq!(header_write_read_versions(&backup.destination_path), (1, 1));
    for suffix in ["-wal", "-shm", "-journal"] {
        let sidecar = PathBuf::from(format!(
            "{}{suffix}",
            backup.destination_path.to_str().unwrap()
        ));
        assert!(!sidecar.exists(), "{suffix} left beside the artifact");
    }
    assert_eq!(destination_entries(destination.path()).len(), 1);

    let archive = private_dir();
    let archived = archive.path().join("archived.sqlite");
    fs::copy(&backup.destination_path, &archived).unwrap();
    fs::set_permissions(&archived, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(archive.path(), fs::Permissions::from_mode(0o500)).unwrap();

    let target_root = private_dir();
    let target = KernelStore::open(target_root.path()).unwrap();
    assert_eq!(target.restore(&archived).unwrap(), 1);
    assert_eq!(target.facts(1).unwrap().commit_seq, 1);

    fs::set_permissions(archive.path(), fs::Permissions::from_mode(0o700)).unwrap();
    for suffix in ["-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{suffix}", archived.to_str().unwrap()));
        assert!(
            !sidecar.exists(),
            "restore created {suffix} beside the source"
        );
    }
}

#[test]
fn restore_interrupted_before_the_swap_rolls_back_on_the_next_open() {
    let root = private_dir();
    let destination = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Normal);
    let backup = store.backup(request(destination.path())).unwrap();
    insert_domain(&store, 2, Sensitivity::Normal);
    let live_oracle = digest(root.path(), Profile::SameRoot);

    // `AfterDisplace` abandons the family in the recovery directory with the
    // marker still published, matching a process killed mid-replacement.
    assert_eq!(
        store
            .restore_with_fault_for_test(&backup.destination_path, RestoreFault::AfterDisplace)
            .unwrap_err(),
        KernelError::Fault
    );
    drop(store);

    let reopened = KernelStore::open(root.path()).unwrap();
    digest(root.path(), Profile::SameRoot).assert_same(&live_oracle, "live state");
    assert_eq!(reopened.facts(1).unwrap().commit_seq, 2);
    assert!(!root.path().join("kernel.sqlite.restore").exists());
    assert!(!fs::read_dir(root.path()).unwrap().any(|entry| {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        name.contains(".restore-")
    }));
    assert_eq!(insert_domain(&reopened, 3, Sensitivity::Normal), 3);
}

#[test]
fn a_forged_restore_marker_fails_closed_instead_of_moving_the_family() {
    let root = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Normal);
    drop(store);
    let live_oracle = digest(root.path(), Profile::SameRoot);

    let marker = root.path().join("kernel.sqlite.restore");
    fs::write(
        &marker,
        b"{\"protocol\":\"eidnara-kernel-restore-marker-v1\"}",
    )
    .unwrap();
    assert_eq!(
        KernelStore::open(root.path()).unwrap_err(),
        KernelError::Inconclusive
    );
    assert!(root.path().join("kernel.sqlite").exists());
    assert!(!fs::read_dir(root.path()).unwrap().any(|entry| {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        name.contains(".restore-")
    }));

    // `Profile::SameRoot` hashes the marker bytes into `recovery_markers`.
    fs::remove_file(&marker).unwrap();
    digest(root.path(), Profile::SameRoot).assert_same(&live_oracle, "live state");
    let reopened = KernelStore::open(root.path()).unwrap();
    assert_eq!(reopened.facts(1).unwrap().commit_seq, 1);
}

#[test]
fn sensitivity_classification_scans_every_table_carrying_the_column() {
    let root = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Normal);
    drop(store);

    let mut connection = inspect(root.path());
    let scanned = sensitivity_bearing_tables_for_test(&mut connection);
    let mut expected: Vec<String> = connection
        .prepare(
            "SELECT m.name FROM sqlite_schema m, pragma_table_info(m.name) p
             WHERE m.type='table' AND p.name='sensitivity_class'",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    expected.sort();

    assert!(!scanned.is_empty());
    assert_eq!(scanned, expected);
    for required in [
        "domains",
        "evidence_meta",
        "outbox",
        "candidates",
        "admission_decisions",
    ] {
        assert!(
            scanned.iter().any(|name| name == required),
            "{required} missing from the sensitivity scan"
        );
    }
}

#[test]
fn restore_interrupted_before_displacement_keeps_the_live_family() {
    let root = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Normal);
    insert_domain(&store, 2, Sensitivity::Normal);
    let live_oracle = digest(root.path(), Profile::SameRoot);

    // A process killed here leaves the marker published, the recovery directory
    // empty, and the live family as the only copy of the data.
    let recovery_dir = store.abandon_restore_marker_for_test().unwrap();
    assert!(recovery_dir.is_dir());
    assert_eq!(fs::read_dir(&recovery_dir).unwrap().count(), 0);
    assert!(root.path().join("kernel.sqlite").exists());
    drop(store);

    let reopened = KernelStore::open(root.path()).unwrap();
    digest(root.path(), Profile::SameRoot).assert_same(&live_oracle, "live state");
    assert_eq!(reopened.facts(1).unwrap().commit_seq, 2);
    assert!(!root.path().join("kernel.sqlite.restore").exists());
    assert!(!recovery_dir.exists());
    assert_eq!(insert_domain(&reopened, 3, Sensitivity::Normal), 3);
}

#[test]
fn restore_interrupted_after_sidecars_move_keeps_the_live_main_file() {
    let root = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Normal);
    insert_domain(&store, 2, Sensitivity::Normal);
    let live_oracle = digest(root.path(), Profile::SameRoot);

    // `displace_family` moves sidecars before the main file, so a kill mid-way
    // leaves the main file live and its sidecars in the recovery directory.
    let recovery_dir = store.abandon_restore_marker_for_test().unwrap();
    drop(store);
    for suffix in ["-wal", "-shm"] {
        let sidecar = PathBuf::from(format!(
            "{}{suffix}",
            root.path().join("kernel.sqlite").to_str().unwrap()
        ));
        if sidecar.exists() {
            fs::rename(&sidecar, recovery_dir.join(sidecar.file_name().unwrap())).unwrap();
        }
    }
    assert!(root.path().join("kernel.sqlite").exists());

    let reopened = KernelStore::open(root.path()).unwrap();
    digest(root.path(), Profile::SameRoot).assert_same(&live_oracle, "live state");
    assert_eq!(reopened.facts(1).unwrap().commit_seq, 2);
    assert!(!recovery_dir.exists());
    assert_eq!(insert_domain(&reopened, 3, Sensitivity::Normal), 3);
}

#[test]
fn a_partially_completed_rollback_keeps_the_members_already_restored() {
    let root = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Normal);
    insert_domain(&store, 2, Sensitivity::Normal);
    let live_oracle = digest(root.path(), Profile::SameRoot);

    let recovery_dir = store.abandon_restore_marker_for_test().unwrap();
    drop(store);
    // `restore_displaced_family` moves the main file first, so a recovery directory
    // holding only sidecars is a rollback that already restored the main file. The
    // live main file must survive the re-run rather than be deleted as residue.
    let main = root.path().join("kernel.sqlite");
    let live_bytes = fs::read(&main).unwrap();
    fs::write(recovery_dir.join("kernel.sqlite-wal"), b"").unwrap();
    assert!(main.exists());

    let reopened = KernelStore::open(root.path()).unwrap();
    assert_eq!(fs::read(&main).unwrap(), live_bytes);
    digest(root.path(), Profile::SameRoot).assert_same(&live_oracle, "live state");
    assert_eq!(reopened.facts(1).unwrap().commit_seq, 2);
    assert!(!recovery_dir.exists());
    assert!(!root.path().join("kernel.sqlite.restore").exists());
    assert_eq!(insert_domain(&reopened, 3, Sensitivity::Normal), 3);
}

#[test]
fn backup_reports_deadline_rather_than_blocking_on_a_held_writer() {
    let root = private_dir();
    let destination = private_dir();
    let store = Arc::new(KernelStore::open(root.path()).unwrap());
    insert_domain(&store, 1, Sensitivity::Normal);

    let (holding_tx, holding_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let holder = {
        let store = Arc::clone(&store);
        std::thread::spawn(move || {
            store
                .commit(intent("hold-the-writer"), |_| {
                    holding_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok("held".to_string())
                })
                .unwrap();
        })
    };
    holding_rx.recv().unwrap();

    let started = Instant::now();
    let outcome = store.backup(BackupRequest {
        destination_directory: destination.path().to_path_buf(),
        deadline: Instant::now() + Duration::from_millis(150),
        capture_pin_expires_at: None,
    });
    let elapsed = started.elapsed();
    release_tx.send(()).unwrap();
    holder.join().unwrap();

    assert_eq!(outcome.unwrap_err(), KernelError::Deadline);
    assert!(
        elapsed < Duration::from_secs(5),
        "backup blocked on the writer for {elapsed:?}"
    );
    assert!(destination_entries(destination.path()).is_empty());
}

#[test]
fn a_secret_row_classifies_the_backup_secret_not_merely_sensitive() {
    let root = private_dir();
    let destination = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Sensitive);
    let sensitive = store.backup(request(destination.path())).unwrap();
    assert_eq!(sensitive.max_sensitivity, Sensitivity::Sensitive);

    insert_domain(&store, 2, Sensitivity::Secret);
    let secret = store.backup(request(destination.path())).unwrap();
    assert_eq!(
        secret.max_sensitivity,
        Sensitivity::Secret,
        "a secret row must not be reported as merely sensitive"
    );
}

#[test]
fn an_orphaned_recovery_directory_is_reclaimed_on_the_next_open() {
    let root = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Normal);
    let live_oracle = digest(root.path(), Profile::SameRoot);

    // A crash after the marker is removed but before cleanup leaves the prior
    // family, which may hold sensitive rows, with nothing to reclaim it.
    let recovery_dir = store.abandon_restore_marker_for_test().unwrap();
    fs::write(recovery_dir.join("kernel.sqlite"), b"prior family bytes").unwrap();
    fs::remove_file(root.path().join("kernel.sqlite.restore")).unwrap();
    drop(store);
    assert!(recovery_dir.is_dir());

    let reopened = KernelStore::open(root.path()).unwrap();
    assert!(
        !recovery_dir.exists(),
        "recovery directory was not reclaimed"
    );
    digest(root.path(), Profile::SameRoot).assert_same(&live_oracle, "live state");
    assert_eq!(insert_domain(&reopened, 2, Sensitivity::Normal), 2);
}

#[test]
fn an_oversized_or_special_restore_marker_is_refused_before_it_is_read() {
    let root = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Normal);
    drop(store);
    let marker = root.path().join("kernel.sqlite.restore");

    fs::write(&marker, vec![b'{'; 64 * 1024 + 1]).unwrap();
    assert_eq!(
        KernelStore::open(root.path()).unwrap_err(),
        KernelError::Inconclusive
    );
    fs::remove_file(&marker).unwrap();

    rustix::fs::mknodat(
        rustix::fs::CWD,
        &marker,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::from_raw_mode(0o600),
        0,
    )
    .unwrap();
    assert_eq!(
        KernelStore::open(root.path()).unwrap_err(),
        KernelError::Inconclusive
    );
    fs::remove_file(&marker).unwrap();

    let reopened = KernelStore::open(root.path()).unwrap();
    assert_eq!(reopened.facts(1).unwrap().commit_seq, 1);
}

#[test]
fn a_recovery_directory_not_created_by_the_store_is_left_untouched() {
    let root = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Normal);
    drop(store);

    // `allocate_recovery_dir` appends only digits, so an operator copy sharing the
    // prefix must survive an open, contents included.
    let operator_copy = root.path().join("kernel.sqlite.restore-incident-4821");
    fs::create_dir(&operator_copy).unwrap();
    fs::write(operator_copy.join("kernel.sqlite"), b"operator evidence").unwrap();
    fs::write(operator_copy.join("notes.txt"), b"do not delete").unwrap();

    let reopened = KernelStore::open(root.path()).unwrap();
    assert!(operator_copy.is_dir(), "operator directory was reaped");
    assert_eq!(
        fs::read(operator_copy.join("kernel.sqlite")).unwrap(),
        b"operator evidence"
    );
    assert_eq!(
        fs::read(operator_copy.join("notes.txt")).unwrap(),
        b"do not delete"
    );
    assert_eq!(reopened.facts(1).unwrap().commit_seq, 1);
}

#[test]
fn restore_verifies_the_staged_copy_so_a_mutated_source_cannot_install() {
    let root = private_dir();
    let destination = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Normal);
    let backup = store.backup(request(destination.path())).unwrap();
    insert_domain(&store, 2, Sensitivity::Normal);
    let live_oracle = digest(root.path(), Profile::SameRoot);

    // Rewriting the artifact in place keeps its device and inode, so only verifying
    // the staged copy can reject it.
    let mut corrupted = fs::read(&backup.destination_path).unwrap();
    let tail = corrupted.len() - 512;
    corrupted[tail..].fill(0x5a);
    fs::write(&backup.destination_path, &corrupted).unwrap();

    assert_eq!(
        store.restore(&backup.destination_path).unwrap_err(),
        KernelError::InvalidRestore
    );
    digest(root.path(), Profile::SameRoot).assert_same(&live_oracle, "live state");
    assert_eq!(store.facts(1).unwrap().commit_seq, 2);
    assert!(!fs::read_dir(root.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".restore-")
    }));
    assert_eq!(insert_domain(&store, 3, Sensitivity::Normal), 3);
}

#[test]
fn orphan_cleanup_does_not_follow_symlinks_out_of_the_store_root() {
    let root = private_dir();
    let outside = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Normal);
    drop(store);

    // `is_dir` follows symlinks, so a symlinked candidate with a generated-looking
    // suffix could otherwise redirect the unlinks at another directory.
    let victim = outside.path().join("kernel.sqlite");
    fs::write(&victim, b"someone else's database").unwrap();
    let victim_wal = outside.path().join("kernel.sqlite-wal");
    fs::write(&victim_wal, b"someone else's wal").unwrap();
    std::os::unix::fs::symlink(
        outside.path(),
        root.path().join("kernel.sqlite.restore-123"),
    )
    .unwrap();

    let reopened = KernelStore::open(root.path()).unwrap();
    assert_eq!(fs::read(&victim).unwrap(), b"someone else's database");
    assert_eq!(fs::read(&victim_wal).unwrap(), b"someone else's wal");
    assert_eq!(reopened.facts(1).unwrap().commit_seq, 1);
}

#[test]
fn scratch_cleanup_spares_files_the_store_could_not_have_written() {
    let root = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Normal);
    drop(store);

    // `restore_temp_path` writes only decimal digits between the prefix and suffix.
    let operator = root.path().join("kernel.sqlite.restore-incident.tmp");
    fs::write(&operator, b"operator diagnostic").unwrap();
    let generated = root.path().join("kernel.sqlite.restore-4242.tmp");
    fs::write(&generated, b"leftover staging").unwrap();

    let reopened = KernelStore::open(root.path()).unwrap();
    assert_eq!(
        fs::read(&operator).unwrap(),
        b"operator diagnostic",
        "operator file was deleted"
    );
    assert!(!generated.exists(), "generated scratch was not reclaimed");
    assert_eq!(reopened.facts(1).unwrap().commit_seq, 1);
}

#[test]
fn a_dangling_reference_is_refused_even_when_integrity_check_passes() {
    let root = private_dir();
    let destination = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Normal);
    let backup = store.backup(request(destination.path())).unwrap();
    insert_domain(&store, 2, Sensitivity::Normal);
    let live_oracle = digest(root.path(), Profile::SameRoot);

    // `foreign_keys=OFF` lets a dangling row be written that `integrity_check`
    // still reports as ok, so only a referential check can reject it.
    let artifact = Connection::open(&backup.destination_path).unwrap();
    artifact.pragma_update(None, "foreign_keys", "OFF").unwrap();
    artifact
        .execute(
            "INSERT INTO capture_pin_refs(capture_pin_id,evidence_id,expires_at)
             VALUES ('no-such-pin','no-such-evidence',NULL)",
            [],
        )
        .unwrap();
    let integrity: String = artifact
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .unwrap();
    assert_eq!(integrity, "ok", "fixture must pass integrity_check");
    let violations: i64 = artifact
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert!(violations > 0, "fixture must have a dangling reference");
    drop(artifact);

    assert_eq!(
        store.restore(&backup.destination_path).unwrap_err(),
        KernelError::InvalidRestore
    );
    digest(root.path(), Profile::SameRoot).assert_same(&live_oracle, "live state");
    assert_eq!(insert_domain(&store, 3, Sensitivity::Normal), 3);
}

#[test]
fn a_lone_wal_mode_main_file_is_refused_as_a_restore_source() {
    let root = private_dir();
    let source_root = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Normal);
    let live_oracle = digest(root.path(), Profile::SameRoot);

    // A byte-copy of a live main file keeps the WAL header while its committed pages
    // may live only in a `-wal` that was not copied.
    let other = KernelStore::open(source_root.path()).unwrap();
    insert_domain(&other, 9, Sensitivity::Normal);
    let bare = root.path().join("bare-main.sqlite");
    fs::copy(source_root.path().join("kernel.sqlite"), &bare).unwrap();
    fs::set_permissions(&bare, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(header_write_read_versions(&bare), (2, 2));

    assert_eq!(
        store.restore(&bare).unwrap_err(),
        KernelError::InvalidRestore
    );
    digest(root.path(), Profile::SameRoot).assert_same(&live_oracle, "live state");

    // A sealed artifact from `backup` is accepted, so the check is not rejecting
    // every source.
    let destination = private_dir();
    let sealed = store.backup(request(destination.path())).unwrap();
    assert_eq!(header_write_read_versions(&sealed.destination_path), (1, 1));
    assert_eq!(store.restore(&sealed.destination_path).unwrap(), 1);
}

#[test]
fn a_restore_advances_the_egress_generation_even_when_the_tip_is_unchanged() {
    let source_root = private_dir();
    let destination = private_dir();
    let source = KernelStore::open(source_root.path()).unwrap();
    insert_domain(&source, 1, Sensitivity::Normal);
    let backup = source.backup(request(destination.path())).unwrap();

    let target_root = private_dir();
    let target = KernelStore::open(target_root.path()).unwrap();
    insert_domain(&target, 1, Sensitivity::Normal);
    let before = target.egress_snapshot().unwrap();
    assert_eq!(before.tip, 1);
    let generation_before = before.classification_generation.unwrap();

    assert_eq!(target.restore(&backup.destination_path).unwrap(), 1);

    let after = target.egress_snapshot().unwrap();
    assert_eq!(
        after.tip, before.tip,
        "the restored tip matches the displaced one"
    );
    let generation_after = after
        .classification_generation
        .expect("no classification change is in flight after the restore returns");
    assert_ne!(
        generation_after, generation_before,
        "a restore that kept the tip left the egress generation unchanged"
    );
}

#[test]
fn a_failed_restore_still_leaves_an_even_egress_generation() {
    let root = private_dir();
    let destination = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Normal);
    let backup = store.backup(request(destination.path())).unwrap();
    insert_domain(&store, 2, Sensitivity::Normal);
    assert_eq!(
        store
            .restore_with_fault_for_test(&backup.destination_path, RestoreFault::AfterDisplace)
            .unwrap_err(),
        KernelError::Fault
    );
    assert!(
        store
            .egress_snapshot()
            .unwrap()
            .classification_generation
            .is_some(),
        "a recovered restore left the classification window open"
    );
}

#[test]
fn an_unrecognized_sensitivity_label_classifies_the_backup_as_secret() {
    let root = private_dir();
    let destination = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Normal);
    drop(store);
    seed_evidence(root.path(), "odd-evidence", "classified");
    let store = KernelStore::open(root.path()).unwrap();

    assert_eq!(
        store
            .backup(request(destination.path()))
            .unwrap()
            .max_sensitivity,
        Sensitivity::Secret,
        "a label outside the vocabulary was classified below Secret"
    );
}

#[test]
fn a_restore_completes_the_purge_unlink_the_backup_recorded_as_pending() {
    use kernel::{
        ArtifactDeletionFault, ArtifactDeletionIdentity, ArtifactDeletionKind,
        ArtifactDeletionRequest, ArtifactErrorKind, ArtifactIngestRequest, ProviderEgress,
    };

    let root = private_dir();
    let destination = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Normal);
    let handle = store
        .ingest_artifact(ArtifactIngestRequest {
            intent: intent("ingest"),
            payload: b"purged secret".to_vec(),
            evidence_id: "evidence".to_string(),
            object_id: "evidence-object".to_string(),
            object_kind: "evidence".to_string(),
            domain_id: "domain-1".to_string(),
            source_kind: "repository".to_string(),
            source_id: "src/evidence".to_string(),
            source_revision: 1,
            media_type: "text/plain".to_string(),
            retention_class: "canonical".to_string(),
            retain_until: None,
            asserted_sensitivity: Sensitivity::Normal,
            provider_egress: ProviderEgress::RemoteAllowed,
            provenance: None,
        })
        .unwrap();
    let object_path = root
        .path()
        .join("artifacts/objects")
        .join(&handle.digest[..2])
        .join(&handle.digest[2..]);

    // The purge commits but its unlink never runs, which is the state a backup
    // taken between the two captures.
    let error = store
        .delete_artifact_with_fault_for_test(
            ArtifactDeletionRequest {
                intent: intent("purge"),
                identity: ArtifactDeletionIdentity::Digest(handle.digest.clone()),
                kind: ArtifactDeletionKind::Purge,
                operator_id: Some("operator-1".to_string()),
                target_locator: Some("incident://secret-1".to_string()),
                reason: Some("secret".to_string()),
                deleted_at: 42,
            },
            ArtifactDeletionFault::AfterCommit,
        )
        .unwrap_err();
    assert_eq!(error.kind(), ArtifactErrorKind::PurgeUnlinkPending);
    assert!(object_path.exists());
    let backup = store.backup(request(destination.path())).unwrap();
    assert!(object_path.exists(), "the backup itself must not unlink");

    store.restore(&backup.destination_path).unwrap();

    assert!(
        !object_path.exists(),
        "a restored pending unlink left the purged bytes readable"
    );
    assert_eq!(
        store.read_artifact(&handle).unwrap_err().kind(),
        ArtifactErrorKind::ReferenceUnavailable
    );
    assert_eq!(
        inspect(root.path())
            .query_row(
                "SELECT COUNT(*) FROM artifact_pending_unlinks WHERE artifact_digest=?1",
                [&handle.digest],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
}

#[test]
fn a_destination_swapped_before_the_copy_receives_no_database_pages() {
    let root = private_dir();
    let destination = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Normal);

    // The hook runs after the temporary file exists in the verified directory and
    // before SQLite opens it by pathname. A same-UID process swapping the
    // directory here presents a replacement holding the observed temporary name.
    let original = destination.path().to_path_buf();
    let displaced = original.with_file_name("displaced-destination");
    let decoy_file = Cell::new(None);
    let error = store
        .backup_with_hook_for_test(request(&original), || {
            let entries = destination_entries(&original);
            assert_eq!(entries.len(), 1);
            let temp_name = entries[0].file_name().unwrap().to_os_string();
            fs::rename(&original, &displaced).unwrap();
            fs::create_dir(&original).unwrap();
            fs::set_permissions(&original, fs::Permissions::from_mode(0o700)).unwrap();
            let decoy = original.join(&temp_name);
            fs::write(&decoy, b"").unwrap();
            decoy_file.set(Some(decoy));
        })
        .unwrap_err();
    assert_eq!(error, KernelError::InvalidBackup);

    let decoy = decoy_file.into_inner().expect("the hook ran");
    let decoy_bytes = fs::read(&decoy).unwrap_or_default();
    assert!(
        decoy_bytes.is_empty(),
        "the swapped-in destination received {} bytes of database copy",
        decoy_bytes.len()
    );
    assert!(
        !decoy_bytes.starts_with(b"SQLite format 3"),
        "the swapped-in destination holds a database"
    );
    assert!(
        destination_entries(&displaced).is_empty(),
        "cleanup left the temporary file in the verified directory"
    );
    // Restore the directory so the tempdir guard can remove it.
    fs::remove_dir_all(&original).unwrap();
    fs::rename(&displaced, &original).unwrap();
}

#[test]
fn a_recovery_directory_swapped_for_a_symlink_fails_closed_on_reopen() {
    let root = private_dir();
    let destination = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Normal);
    let backup = store.backup(request(destination.path())).unwrap();
    insert_domain(&store, 2, Sensitivity::Normal);
    // `RecoveryFailure` abandons the family in `.restore-<n>` with the marker
    // still published, matching a process killed mid-replacement.
    assert_eq!(
        store
            .restore_with_fault_for_test(&backup.destination_path, RestoreFault::RecoveryFailure)
            .unwrap_err(),
        KernelError::InvalidRestore
    );
    drop(store);

    let recovery = fs::read_dir(root.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .contains(".restore-")
                && path.is_dir()
        })
        .expect("an interrupted restore leaves its recovery directory");
    let displaced_family = fs::read_dir(&recovery).unwrap().count();
    assert!(displaced_family > 0);

    // A same-UID process moves the real directory aside and points the marker's
    // recovery path at a decoy holding a `kernel.sqlite` of its choosing.
    let stashed = root.path().join("stashed-recovery");
    fs::rename(&recovery, &stashed).unwrap();
    let decoy = private_dir();
    fs::write(decoy.path().join("kernel.sqlite"), b"not a kernel database").unwrap();
    std::os::unix::fs::symlink(decoy.path(), &recovery).unwrap();

    assert_eq!(
        KernelStore::open(root.path()).unwrap_err(),
        KernelError::Inconclusive,
        "a symlinked recovery directory was followed"
    );
    assert!(
        !root.path().join("kernel.sqlite").exists(),
        "the decoy database was moved into the store root"
    );
    assert!(
        decoy.path().join("kernel.sqlite").exists(),
        "the decoy's file was moved out of its directory"
    );
    assert_eq!(
        fs::read_dir(&stashed).unwrap().count(),
        displaced_family,
        "the real displaced family was touched"
    );
    assert!(root.path().join("kernel.sqlite.restore").exists());

    // Putting the real directory back lets the next open roll the family back.
    fs::remove_file(&recovery).unwrap();
    fs::rename(&stashed, &recovery).unwrap();
    let reopened = KernelStore::open(root.path()).unwrap();
    assert_eq!(reopened.facts(1).unwrap().commit_seq, 2);
}

#[test]
fn a_restore_removes_bytes_the_restored_history_recorded_as_purged() {
    use kernel::{
        ArtifactDeletionIdentity, ArtifactDeletionKind, ArtifactDeletionRequest, ArtifactErrorKind,
        ArtifactIngestRequest, ProviderEgress,
    };

    fn ingest_request(key: &str) -> ArtifactIngestRequest {
        ArtifactIngestRequest {
            intent: intent(key),
            payload: b"shared secret payload".to_vec(),
            evidence_id: format!("evidence-{key}"),
            object_id: format!("evidence-object-{key}"),
            object_kind: "evidence".to_string(),
            domain_id: "domain-1".to_string(),
            source_kind: "repository".to_string(),
            source_id: format!("src/{key}"),
            source_revision: 1,
            media_type: "text/plain".to_string(),
            retention_class: "canonical".to_string(),
            retain_until: None,
            asserted_sensitivity: Sensitivity::Normal,
            provider_egress: ProviderEgress::RemoteAllowed,
            provenance: None,
        }
    }

    // Store A ingests the payload and purges it completely: tombstone recorded,
    // bytes unlinked, no pending unlink left behind.
    let source_root = private_dir();
    let destination = private_dir();
    let source = KernelStore::open(source_root.path()).unwrap();
    insert_domain(&source, 1, Sensitivity::Normal);
    let handle = source.ingest_artifact(ingest_request("source")).unwrap();
    source
        .delete_artifact(ArtifactDeletionRequest {
            intent: intent("purge"),
            identity: ArtifactDeletionIdentity::Digest(handle.digest.clone()),
            kind: ArtifactDeletionKind::Purge,
            operator_id: Some("operator-1".to_string()),
            target_locator: Some("incident://secret-1".to_string()),
            reason: Some("secret".to_string()),
            deleted_at: 42,
        })
        .unwrap();
    let backup = source.backup(request(destination.path())).unwrap();
    assert_eq!(
        inspect(source_root.path())
            .query_row("SELECT COUNT(*) FROM artifact_pending_unlinks", [], |row| {
                row.get::<_, i64>(0)
            },)
            .unwrap(),
        0,
        "the purge completed, so the backup carries no pending unlink"
    );

    // Store B holds the same bytes live, then takes A's history.
    let target_root = private_dir();
    let target = KernelStore::open(target_root.path()).unwrap();
    insert_domain(&target, 1, Sensitivity::Normal);
    let live = target.ingest_artifact(ingest_request("target")).unwrap();
    assert_eq!(live.digest, handle.digest);
    let object_path = target_root
        .path()
        .join("artifacts/objects")
        .join(&handle.digest[..2])
        .join(&handle.digest[2..]);
    assert!(object_path.exists());

    target.restore(&backup.destination_path).unwrap();

    assert!(
        !object_path.exists(),
        "bytes the restored history recorded as purged stayed readable"
    );
    assert_eq!(
        target.read_artifact(&handle).unwrap_err().kind(),
        ArtifactErrorKind::ReferenceUnavailable
    );
    assert_eq!(
        inspect(target_root.path())
            .query_row("SELECT COUNT(*) FROM artifact_pending_unlinks", [], |row| {
                row.get::<_, i64>(0)
            },)
            .unwrap(),
        0,
        "the re-armed unlink was not cleared after completing"
    );
}

#[test]
fn a_staged_restore_file_swapped_after_verification_is_not_installed() {
    // Two backups that both verify, share a commit sequence of 1, and differ in
    // the domain they hold.
    let verified_root = private_dir();
    let verified_destination = private_dir();
    let verified_store = KernelStore::open(verified_root.path()).unwrap();
    insert_domain(&verified_store, 1, Sensitivity::Normal);
    let verified = verified_store
        .backup(request(verified_destination.path()))
        .unwrap();
    let decoy_root = private_dir();
    let decoy_destination = private_dir();
    let decoy_store = KernelStore::open(decoy_root.path()).unwrap();
    insert_domain(&decoy_store, 7, Sensitivity::Normal);
    let decoy = decoy_store
        .backup(request(decoy_destination.path()))
        .unwrap();
    assert_eq!(verified.captured_commit_seq, decoy.captured_commit_seq);

    let root = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 2, Sensitivity::Normal);
    insert_domain(&store, 3, Sensitivity::Normal);
    let live_oracle = digest(root.path(), Profile::SameRoot);

    // The hook runs after the live family is displaced and before the staged file
    // is renamed into place. A same-UID process swaps the verified staged copy for
    // the decoy here.
    let swapped = Cell::new(false);
    let error = store
        .restore_with_hook_for_test(&verified.destination_path, || {
            let staged = fs::read_dir(root.path())
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| path.extension().is_some_and(|extension| extension == "tmp"))
                .expect("the staged copy is present while the hook runs");
            fs::remove_file(&staged).unwrap();
            fs::copy(&decoy.destination_path, &staged).unwrap();
            fs::set_permissions(&staged, fs::Permissions::from_mode(0o600)).unwrap();
            swapped.set(true);
        })
        .unwrap_err();
    assert!(swapped.get());
    assert_eq!(
        error,
        KernelError::InvalidRestore,
        "a staged file swapped after verification was installed"
    );

    // The displaced family came back and the decoy's domain never appeared.
    digest(root.path(), Profile::SameRoot).assert_same(&live_oracle, "live state");
    let known = store.known_as_of(store.tip().unwrap()).unwrap();
    let mut domains = known
        .objects
        .iter()
        .filter(|object| object.object_kind == "domain")
        .map(|object| object.object_id.as_str())
        .collect::<Vec<_>>();
    domains.sort_unstable();
    assert_eq!(domains, ["object-2", "object-3"]);
    assert_eq!(insert_domain(&store, 4, Sensitivity::Normal), 3);
}

#[test]
fn a_store_root_swapped_during_a_restore_is_not_adopted() {
    // A decoy store whose database is a valid kernel family at commit 1.
    let decoy_root = private_dir();
    let decoy_store = KernelStore::open(decoy_root.path()).unwrap();
    insert_domain(&decoy_store, 7, Sensitivity::Normal);
    drop(decoy_store);
    let fence_epoch = |database: &Path| -> i64 {
        Connection::open(database)
            .unwrap()
            .query_row(
                "SELECT writer_epoch FROM writer_fence WHERE id=0",
                [],
                |row| row.get(0),
            )
            .unwrap()
    };
    let decoy_epoch = fence_epoch(&decoy_root.path().join("kernel.sqlite"));

    let parent = private_dir();
    let root = parent.path().join("store");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let destination = private_dir();
    // Opened twice so the store's lease epoch differs from the decoy's fence: a
    // fence stamped on the decoy is then visible as a changed epoch.
    drop(KernelStore::open(&root).unwrap());
    let store = KernelStore::open(&root).unwrap();
    assert_ne!(fence_epoch(&root.join("kernel.sqlite")), decoy_epoch);
    insert_domain(&store, 1, Sensitivity::Normal);
    let backup = store.backup(request(destination.path())).unwrap();
    insert_domain(&store, 2, Sensitivity::Normal);

    // After the live family is displaced and before the verified copy is
    // installed, the whole root is renamed away and the decoy root takes its path.
    let moved = parent.path().join("moved-store");
    let swapped = Cell::new(false);
    let error = store
        .restore_with_hook_for_test(&backup.destination_path, || {
            fs::rename(&root, &moved).unwrap();
            fs::rename(decoy_root.path(), &root).unwrap();
            swapped.set(true);
        })
        .unwrap_err();
    assert!(swapped.get());
    assert_eq!(
        error,
        KernelError::InvalidRestore,
        "the connections were switched to a database the held root never received"
    );

    // The decoy's database was never installed over or adopted in place of the
    // store's own family, which still sits in the moved root.
    let decoy_domains: i64 = Connection::open(root.join("kernel.sqlite"))
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM domains WHERE domain_id='domain-7'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(decoy_domains, 1, "the decoy database was replaced");
    // The restore opened the decoy's database by pathname; the swap was seen
    // before that connection wrote, so the decoy's own fence is untouched.
    assert_eq!(
        fence_epoch(&root.join("kernel.sqlite")),
        decoy_epoch,
        "the restore stamped its fence on a database its root never held"
    );
    assert!(
        moved.join("kernel.sqlite").exists() || {
            fs::read_dir(&moved).unwrap().any(|entry| {
                entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains(".restore-")
            })
        }
    );
    // Put the root back so the tempdir guards can clean up.
    fs::rename(&root, decoy_root.path()).unwrap();
    fs::rename(&moved, &root).unwrap();
}

#[test]
fn a_restore_reports_a_purge_unlink_it_could_not_complete() {
    use kernel::{
        ArtifactDeletionFault, ArtifactDeletionIdentity, ArtifactDeletionKind,
        ArtifactDeletionRequest, ArtifactErrorKind, ArtifactIngestRequest, ProviderEgress,
    };

    let root = private_dir();
    let destination = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Normal);
    let handle = store
        .ingest_artifact(ArtifactIngestRequest {
            intent: intent("ingest"),
            payload: b"purged but stuck".to_vec(),
            evidence_id: "evidence".to_string(),
            object_id: "evidence-object".to_string(),
            object_kind: "evidence".to_string(),
            domain_id: "domain-1".to_string(),
            source_kind: "repository".to_string(),
            source_id: "src/evidence".to_string(),
            source_revision: 1,
            media_type: "text/plain".to_string(),
            retention_class: "canonical".to_string(),
            retain_until: None,
            asserted_sensitivity: Sensitivity::Normal,
            provider_egress: ProviderEgress::RemoteAllowed,
            provenance: None,
        })
        .unwrap();
    let shard = root
        .path()
        .join("artifacts/objects")
        .join(&handle.digest[..2]);
    let object_path = shard.join(&handle.digest[2..]);
    let error = store
        .delete_artifact_with_fault_for_test(
            ArtifactDeletionRequest {
                intent: intent("purge"),
                identity: ArtifactDeletionIdentity::Digest(handle.digest.clone()),
                kind: ArtifactDeletionKind::Purge,
                operator_id: Some("operator-1".to_string()),
                target_locator: Some("incident://secret-1".to_string()),
                reason: Some("secret".to_string()),
                deleted_at: 42,
            },
            ArtifactDeletionFault::AfterCommit,
        )
        .unwrap_err();
    assert_eq!(error.kind(), ArtifactErrorKind::PurgeUnlinkPending);
    let backup = store.backup(request(destination.path())).unwrap();

    // The shard loses its owner-only mode, so the unlink the restored history
    // still owes cannot open it.
    fs::set_permissions(&shard, fs::Permissions::from_mode(0o500)).unwrap();
    let restored = store.restore(&backup.destination_path);
    fs::set_permissions(&shard, fs::Permissions::from_mode(0o700)).unwrap();

    assert_eq!(
        restored.unwrap_err(),
        KernelError::Io,
        "a restore whose purge unlink failed reported success"
    );
    assert!(
        object_path.exists(),
        "the bytes were removed despite the error"
    );
    assert_eq!(
        inspect(root.path())
            .query_row(
                "SELECT COUNT(*) FROM artifact_pending_unlinks WHERE artifact_digest=?1",
                [&handle.digest],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1,
        "the pending unlink must stay recorded for the next recovery"
    );
}

fn evidence_ingest(key: &str, payload: &[u8]) -> kernel::ArtifactIngestRequest {
    kernel::ArtifactIngestRequest {
        intent: intent(key),
        payload: payload.to_vec(),
        evidence_id: format!("evidence-{key}"),
        object_id: format!("evidence-object-{key}"),
        object_kind: "evidence".to_string(),
        domain_id: "domain-1".to_string(),
        source_kind: "repository".to_string(),
        source_id: format!("src/{key}"),
        source_revision: 1,
        media_type: "text/plain".to_string(),
        retention_class: "canonical".to_string(),
        retain_until: None,
        asserted_sensitivity: Sensitivity::Normal,
        provider_egress: kernel::ProviderEgress::RemoteAllowed,
        provenance: None,
    }
}

fn purge(store: &KernelStore, key: &str, digest: &str) {
    use kernel::{ArtifactDeletionIdentity, ArtifactDeletionKind, ArtifactDeletionRequest};
    store
        .delete_artifact(ArtifactDeletionRequest {
            intent: intent(key),
            identity: ArtifactDeletionIdentity::Digest(digest.to_string()),
            kind: ArtifactDeletionKind::Purge,
            operator_id: Some("operator-1".to_string()),
            target_locator: Some("incident://secret-1".to_string()),
            reason: Some("secret".to_string()),
            deleted_at: 42,
        })
        .unwrap();
}

#[test]
fn a_restore_sweeps_staging_temps_of_a_purge_whose_object_is_already_gone() {
    // Store A purges the payload completely and backs up that history.
    let source_root = private_dir();
    let destination = private_dir();
    let source = KernelStore::open(source_root.path()).unwrap();
    insert_domain(&source, 1, Sensitivity::Normal);
    let handle = source
        .ingest_artifact(evidence_ingest("source", b"purged staging payload"))
        .unwrap();
    purge(&source, "purge", &handle.digest);
    let backup = source.backup(request(destination.path())).unwrap();

    // Store B never published the object, but a failed ingest left its staging
    // temp behind after the store opened, so the open-time sweep never saw it.
    let target_root = private_dir();
    let target = KernelStore::open(target_root.path()).unwrap();
    insert_domain(&target, 1, Sensitivity::Normal);
    let tmp = target_root.path().join("artifacts/tmp");
    let leftover = tmp.join(format!(".artifact-{}-1234.tmp", handle.digest));
    fs::write(&leftover, b"purged staging payload").unwrap();
    fs::set_permissions(&leftover, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        !target_root
            .path()
            .join("artifacts/objects")
            .join(&handle.digest[..2])
            .exists()
    );

    target.restore(&backup.destination_path).unwrap();

    assert!(
        !leftover.exists(),
        "a purge the restored history recorded left its bytes in a staging temp"
    );
    assert_eq!(
        inspect(target_root.path())
            .query_row("SELECT COUNT(*) FROM artifact_pending_unlinks", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0,
        "the re-armed unlink was not cleared after completing"
    );
}

#[test]
fn a_backup_whose_evidence_was_purged_after_capture_cannot_be_restored() {
    use kernel::ArtifactErrorKind;

    let root = private_dir();
    let destination = private_dir();
    let store = KernelStore::open(root.path()).unwrap();
    insert_domain(&store, 1, Sensitivity::Normal);
    let handle = store
        .ingest_artifact(evidence_ingest("pinned", b"pinned then purged"))
        .unwrap();
    let backup = store.backup(request(destination.path())).unwrap();
    let pin_id = backup
        .capture_pin_id
        .clone()
        .expect("live evidence pins the capture");

    // The purge wins over the pin: the bytes are gone and the pin records why.
    purge(&store, "purge", &handle.digest);
    let connection = inspect(root.path());
    let (degraded, tombstones, tip): (Option<i64>, i64, i64) = connection
        .query_row(
            "SELECT
                (SELECT purge_degraded_at FROM capture_pins WHERE capture_pin_id=?1),
                (SELECT COUNT(*) FROM artifact_purge_tombstones WHERE artifact_digest=?2),
                (SELECT MAX(commit_seq) FROM commit_log)",
            params![pin_id, handle.digest],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!((degraded, tombstones), (Some(42), 1));
    drop(connection);

    // The backup would reinstate a live evidence row for bytes no longer held.
    assert_eq!(
        store.restore(&backup.destination_path).unwrap_err(),
        KernelError::InvalidRestore
    );

    // The live family is untouched: purge history stands, the reference stays
    // tombstoned rather than dangling, and nothing was staged into the root.
    let connection = inspect(root.path());
    let (tombstones_after, tip_after): (i64, i64) = connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM artifact_purge_tombstones WHERE artifact_digest=?1),
                    (SELECT MAX(commit_seq) FROM commit_log)",
            [&handle.digest],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((tombstones_after, tip_after), (1, tip));
    assert_eq!(
        store.read_artifact(&handle).unwrap_err().kind(),
        ArtifactErrorKind::ReferenceUnavailable
    );
    let stray: Vec<_> = fs::read_dir(root.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.starts_with("kernel.sqlite") && name.contains("restore"))
        .collect();
    assert!(stray.is_empty(), "restore left staging behind: {stray:?}");

    // Writes after the refused restore proceed on the live family.
    insert_domain(&store, 2, Sensitivity::Normal);
}

#[test]
fn a_backup_cannot_reverse_a_purge_the_target_has_committed() {
    use kernel::{
        ArtifactDeletionFault, ArtifactDeletionIdentity, ArtifactDeletionKind,
        ArtifactDeletionRequest, ArtifactErrorKind,
    };

    // Store A holds the payload live and backs that up.
    let source_root = private_dir();
    let destination = private_dir();
    let source = KernelStore::open(source_root.path()).unwrap();
    insert_domain(&source, 1, Sensitivity::Normal);
    let handle = source
        .ingest_artifact(evidence_ingest("source", b"purged on the target"))
        .unwrap();
    let backup = source.backup(request(destination.path())).unwrap();

    // Store B has purged the same bytes, but the unlink has not run yet: the
    // object is still a regular file at its path, so presence alone would let
    // A's live evidence for it back in.
    let target_root = private_dir();
    let target = KernelStore::open(target_root.path()).unwrap();
    insert_domain(&target, 1, Sensitivity::Normal);
    let live = target
        .ingest_artifact(evidence_ingest("target", b"purged on the target"))
        .unwrap();
    assert_eq!(live.digest, handle.digest);
    let error = target
        .delete_artifact_with_fault_for_test(
            ArtifactDeletionRequest {
                intent: intent("purge"),
                identity: ArtifactDeletionIdentity::Digest(handle.digest.clone()),
                kind: ArtifactDeletionKind::Purge,
                operator_id: Some("operator-1".to_string()),
                target_locator: Some("incident://secret-1".to_string()),
                reason: Some("secret".to_string()),
                deleted_at: 42,
            },
            ArtifactDeletionFault::AfterCommit,
        )
        .unwrap_err();
    assert_eq!(error.kind(), ArtifactErrorKind::PurgeUnlinkPending);
    let object_path = target_root
        .path()
        .join("artifacts/objects")
        .join(&handle.digest[..2])
        .join(&handle.digest[2..]);
    assert!(
        object_path.exists(),
        "the pending unlink leaves the bytes in place"
    );

    assert_eq!(
        target.restore(&backup.destination_path).unwrap_err(),
        KernelError::InvalidRestore
    );

    // The purge stands: tombstone and pending unlink survive, the reference is
    // still tombstoned, and the owed unlink still completes.
    let (tombstones, pending): (i64, i64) = inspect(target_root.path())
        .query_row(
            "SELECT (SELECT COUNT(*) FROM artifact_purge_tombstones WHERE artifact_digest=?1),
                    (SELECT COUNT(*) FROM artifact_pending_unlinks WHERE artifact_digest=?1)",
            [&handle.digest],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((tombstones, pending), (1, 1));
    assert_eq!(
        target.read_artifact(&live).unwrap_err().kind(),
        ArtifactErrorKind::ReferenceUnavailable
    );
    drop(target);
    let reopened = KernelStore::open(target_root.path()).unwrap();
    assert!(!object_path.exists(), "reopening finishes the purge unlink");
    assert_eq!(
        reopened.read_artifact(&live).unwrap_err().kind(),
        ArtifactErrorKind::ReferenceUnavailable
    );
}

#[test]
fn a_backup_whose_required_object_fails_verification_cannot_be_restored() {
    let source_root = private_dir();
    let destination = private_dir();
    let source = KernelStore::open(source_root.path()).unwrap();
    insert_domain(&source, 1, Sensitivity::Normal);
    let handle = source
        .ingest_artifact(evidence_ingest(
            "source",
            b"the whole payload the digest names",
        ))
        .unwrap();
    let backup = source.backup(request(destination.path())).unwrap();

    // The target holds a regular file at the required path whose bytes are not
    // the payload: an orphan left by an interrupted process, truncated on disk.
    let target_root = private_dir();
    let target = KernelStore::open(target_root.path()).unwrap();
    insert_domain(&target, 1, Sensitivity::Normal);
    let shard = target_root
        .path()
        .join("artifacts/objects")
        .join(&handle.digest[..2]);
    fs::create_dir(&shard).unwrap();
    fs::set_permissions(&shard, fs::Permissions::from_mode(0o700)).unwrap();
    let object_path = shard.join(&handle.digest[2..]);
    fs::write(&object_path, b"the whole").unwrap();
    fs::set_permissions(&object_path, fs::Permissions::from_mode(0o600)).unwrap();

    assert_eq!(
        target.restore(&backup.destination_path).unwrap_err(),
        KernelError::InvalidRestore
    );
    // The live family is untouched and the orphan is left for GC to judge.
    assert_eq!(
        inspect(target_root.path())
            .query_row("SELECT COUNT(*) FROM evidence_meta", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
    assert!(object_path.exists());
    insert_domain(&target, 2, Sensitivity::Normal);
}

#[test]
fn a_backup_that_does_not_carry_a_purge_the_target_committed_cannot_be_restored() {
    use kernel::ArtifactErrorKind;

    // Store A never saw the purged bytes at all; its backup carries no tombstone
    // and references no artifact, so nothing but the missing tombstone stands
    // between it and the target.
    let source_root = private_dir();
    let destination = private_dir();
    let source = KernelStore::open(source_root.path()).unwrap();
    insert_domain(&source, 1, Sensitivity::Normal);
    let backup = source.backup(request(destination.path())).unwrap();

    // Store B purged a digest completely: tombstone recorded, bytes gone.
    let target_root = private_dir();
    let target = KernelStore::open(target_root.path()).unwrap();
    insert_domain(&target, 1, Sensitivity::Normal);
    let purged = target
        .ingest_artifact(evidence_ingest("purged", b"never to return"))
        .unwrap();
    purge(&target, "purge", &purged.digest);

    // Installing A's history would drop B's tombstone and let the bytes back in.
    assert_eq!(
        target.restore(&backup.destination_path).unwrap_err(),
        KernelError::InvalidRestore
    );
    assert_eq!(
        inspect(target_root.path())
            .query_row(
                "SELECT COUNT(*) FROM artifact_purge_tombstones WHERE artifact_digest=?1",
                [&purged.digest],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
    assert_eq!(
        target
            .ingest_artifact(evidence_ingest("again", b"never to return"))
            .unwrap_err()
            .kind(),
        ArtifactErrorKind::ReAdmissionBlocked,
        "the purge still blocks re-admission"
    );

    // A backup taken from B itself after the purge carries the tombstone and
    // restores.
    let later = target.backup(request(destination.path())).unwrap();
    target.restore(&later.destination_path).unwrap();
}
