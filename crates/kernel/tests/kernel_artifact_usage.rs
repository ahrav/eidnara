//! Artifact usage accounting against an independent `st_size` walk.
//!
//! The store walks the object tree when it opens and then keeps the count
//! under the writer lock. Every test here compares the usage the store reports
//! with a walk of `artifacts/objects` this file performs through `std::fs`,
//! and every decrement path records the walk before and after it, so a path
//! that removed no bytes cannot satisfy its marker.

#![cfg(feature = "test-support")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

use kernel::{
    ArtifactDeletionIdentity, ArtifactDeletionKind, ArtifactDeletionRequest, ArtifactErrorKind,
    ArtifactGcFault, ArtifactIngestFault, ArtifactIngestHook, ArtifactIngestRequest, BackupRequest,
    CommitIntent, DomainSpec, KernelStore, ProviderEgress, RepositoryProvenance, Sensitivity,
};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};

const DAY_MS: i64 = 24 * 60 * 60 * 1_000;

const CLEANUP_MARKER: &str = "artifact-byte-decrement-paths-are-exercised-cleanup";
const RECOVERY_MARKER: &str = "artifact-byte-decrement-paths-are-exercised-recovery";
const RECLAIM_MARKER: &str = "artifact-byte-decrement-paths-are-exercised-reclaim";
const PURGE_MARKER: &str = "artifact-byte-decrement-paths-are-exercised-purge";
const RETRY_MARKER: &str = "artifact-byte-decrement-paths-are-exercised-retry";

fn intent(key: &str, payload: &[u8]) -> CommitIntent {
    CommitIntent {
        producer: "kernel-artifact-usage-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(payload)),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

fn seed_domain(store: &KernelStore) {
    store
        .commit(intent("domain", b"domain"), |envelope| {
            envelope.insert_domain(DomainSpec {
                domain_id: "domain".to_string(),
                object_id: "domain-object".to_string(),
                name: "fixture".to_string(),
                source_kind: "fixture".to_string(),
                source_id: "domain".to_string(),
                source_revision: 1,
                sensitivity: Sensitivity::Normal,
            })?;
            Ok("domain".to_string())
        })
        .unwrap();
}

fn request(key: &str, payload: &[u8]) -> ArtifactIngestRequest {
    ArtifactIngestRequest {
        intent: intent(key, payload),
        payload: payload.to_vec(),
        evidence_id: format!("evidence-{key}"),
        object_id: format!("evidence-object-{key}"),
        object_kind: "evidence".to_string(),
        domain_id: "domain".to_string(),
        source_kind: "repository".to_string(),
        source_id: format!("src/{key}"),
        source_revision: 1,
        media_type: "text/plain".to_string(),
        retention_class: "canonical".to_string(),
        retain_until: None,
        asserted_sensitivity: Sensitivity::Normal,
        provider_egress: ProviderEgress::RemoteAllowed,
        provenance: Some(RepositoryProvenance {
            repository_id: "repo".to_string(),
            revision: "abc123".to_string(),
        }),
    }
}

fn purge(key: &str, digest: &str) -> ArtifactDeletionRequest {
    ArtifactDeletionRequest {
        intent: intent(&format!("purge-{key}"), key.as_bytes()),
        identity: ArtifactDeletionIdentity::Digest(digest.to_string()),
        kind: ArtifactDeletionKind::Purge,
        operator_id: Some("operator-1".to_string()),
        target_locator: Some("incident://secret-1".to_string()),
        reason: Some("secret".to_string()),
        deleted_at: 42,
    }
}

fn walked_bytes(root: &Path) -> u64 {
    let mut bytes = 0;
    for entry in fs::read_dir(root.join("artifacts/objects")).unwrap() {
        let entry = entry.unwrap();
        let metadata = fs::symlink_metadata(entry.path()).unwrap();
        if metadata.is_file() {
            bytes += metadata.len();
        } else if metadata.is_dir() {
            for object in fs::read_dir(entry.path()).unwrap() {
                let metadata = fs::symlink_metadata(object.unwrap().path()).unwrap();
                if metadata.is_file() {
                    bytes += metadata.len();
                }
            }
        }
    }
    bytes
}

fn reported(store: &KernelStore) -> u64 {
    store.artifact_budget_facts().unwrap().usage_bytes
}

fn assert_usage_matches_walk(store: &KernelStore, root: &Path, context: &str) {
    assert_eq!(reported(store), walked_bytes(root), "{context}");
}

fn object_path(root: &Path, digest: &str) -> std::path::PathBuf {
    root.join("artifacts/objects")
        .join(&digest[..2])
        .join(&digest[2..])
}

fn invalidate(root: &Path, evidence_id: &str) {
    let connection = Connection::open(root.join("kernel.sqlite")).unwrap();
    let next: i64 = connection
        .query_row(
            "SELECT COALESCE(MAX(commit_seq),0)+1 FROM commit_log",
            [],
            |row| row.get(0),
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO commit_log(commit_seq,transaction_id,writer_epoch,producer,operation_key,request_digest,recorded_at,actor,cause)
             VALUES (?1,?2,1,'test',?2,?3,0,'test','invalidate')",
            params![next, format!("invalidate-{evidence_id}"), "a".repeat(64)],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE evidence_meta SET invalidated_commit_seq=?1 WHERE evidence_id=?2",
            params![next, evidence_id],
        )
        .unwrap();
}

fn reservation_count(root: &Path) -> i64 {
    Connection::open(root.join("kernel.sqlite"))
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM artifact_ingestion_reservations",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn deduplicated_bytes_add_zero_at_exactly_the_cap() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open_with_artifact_cap_for_test(root.path(), 4).unwrap();
    seed_domain(&store);
    store.ingest_artifact(request("full", b"1234")).unwrap();
    assert_usage_matches_walk(&store, root.path(), "publish to the cap");
    store
        .ingest_artifact(request("same-bytes", b"1234"))
        .unwrap();
    assert_eq!(reported(&store), 4, "a present digest adds zero");
    assert_usage_matches_walk(&store, root.path(), "dedup at the cap");
}

#[test]
fn a_refused_ingest_leaves_no_reservation_and_no_object() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open_with_artifact_cap_for_test(root.path(), 4).unwrap();
    seed_domain(&store);
    store.ingest_artifact(request("three", b"123")).unwrap();
    let refused = request("two", b"45");
    let digest = format!("{:x}", Sha256::digest(&refused.payload));
    let error = store.ingest_artifact(refused).unwrap_err();
    assert_eq!(error.kind(), ArtifactErrorKind::Capacity);
    assert_eq!(error.usage(), Some(3));
    assert!(!object_path(root.path(), &digest).exists());
    assert_eq!(reservation_count(root.path()), 0);
    assert_usage_matches_walk(&store, root.path(), "refusal");
}

#[test]
fn failed_reference_cleanup_subtracts_the_bytes_it_published() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open_with_artifact_cap_for_test(root.path(), 8).unwrap();
    seed_domain(&store);
    store.ingest_artifact(request("kept", b"12")).unwrap();
    let mut published = None;
    let error = store
        .ingest_artifact_with_protocol_hook_for_test(
            request("cleaned", b"345678"),
            Some(ArtifactIngestFault::AfterEvents),
            |point| {
                if point == ArtifactIngestHook::AfterPublish {
                    published = Some(walked_bytes(root.path()));
                }
            },
        )
        .unwrap_err();
    assert_eq!(error.kind(), ArtifactErrorKind::ReferenceCommit);
    let removed = published.unwrap() - walked_bytes(root.path());
    assert_eq!(removed, 6, "{CLEANUP_MARKER}");
    assert_usage_matches_walk(&store, root.path(), "cleanup");
    store.ingest_artifact(request("fits", b"abcdef")).unwrap();
    assert_usage_matches_walk(&store, root.path(), "ingest after cleanup");
}

#[test]
fn an_unrecovered_publish_counts_until_startup_recovery_removes_it() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open_with_artifact_cap_for_test(root.path(), 8).unwrap();
    seed_domain(&store);
    // A panic after the publish stands in for a writer that died before its
    // reference committed: the bytes and the live reservation stay behind.
    let abandoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        store.ingest_artifact_with_protocol_hook_for_test(
            request("orphan", b"123456"),
            None,
            |point| {
                if point == ArtifactIngestHook::AfterPublish {
                    panic!("writer died after publishing");
                }
            },
        )
    }));
    assert!(abandoned.is_err());
    assert_usage_matches_walk(&store, root.path(), "orphan publish");
    let error = store.ingest_artifact(request("over", b"789")).unwrap_err();
    assert_eq!(error.kind(), ArtifactErrorKind::Capacity);
    assert_eq!(error.usage(), Some(6), "the orphan's bytes are counted");
    drop(store);

    let before = walked_bytes(root.path());
    let store = KernelStore::open_with_artifact_cap_for_test(root.path(), 8).unwrap();
    let removed = before - walked_bytes(root.path());
    assert_eq!(removed, 6, "{RECOVERY_MARKER}");
    assert_usage_matches_walk(&store, root.path(), "startup recovery");
    // Admission stays open only if the walk after recovery matched the count
    // recovery kept.
    store.ingest_artifact(request("after", b"789")).unwrap();
    assert_usage_matches_walk(&store, root.path(), "ingest after recovery");
}

#[test]
fn reclaim_purge_and_a_retried_unlink_subtract_the_bytes_they_remove() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open_with_artifact_cap_for_test(root.path(), 64).unwrap();
    seed_domain(&store);
    let reclaimed = store
        .ingest_artifact(request("reclaimed", b"1234"))
        .unwrap();
    let retried = store.ingest_artifact(request("retried", b"56789")).unwrap();
    let purged = store.ingest_artifact(request("purged", b"secret")).unwrap();
    store.ingest_artifact(request("kept", b"kept")).unwrap();
    assert_usage_matches_walk(&store, root.path(), "publish");

    invalidate(root.path(), &reclaimed.evidence_id);
    let before = walked_bytes(root.path());
    store.run_staging_maintenance(15 * DAY_MS).unwrap();
    assert_eq!(before - walked_bytes(root.path()), 4, "{RECLAIM_MARKER}");
    assert_usage_matches_walk(&store, root.path(), "reclaim");

    let before = walked_bytes(root.path());
    store
        .delete_artifact(purge("secret", &purged.digest))
        .unwrap();
    assert_eq!(before - walked_bytes(root.path()), 6, "{PURGE_MARKER}");
    assert_usage_matches_walk(&store, root.path(), "purge");

    invalidate(root.path(), &retried.evidence_id);
    let before = walked_bytes(root.path());
    assert!(
        store
            .run_staging_maintenance_with_fault_for_test(15 * DAY_MS, ArtifactGcFault::Unlink)
            .is_ok_and(|result| result.artifact_gc.failed_candidates == 1)
    );
    assert_eq!(
        walked_bytes(root.path()),
        before,
        "the failed unlink kept the bytes"
    );
    assert_usage_matches_walk(&store, root.path(), "failed unlink");
    drop(store);
    // The failed unlink latched the fail-closed state, so a fresh store retries.
    let store = KernelStore::open_with_artifact_cap_for_test(root.path(), 64).unwrap();
    store.run_staging_maintenance(15 * DAY_MS).unwrap();
    assert_eq!(before - walked_bytes(root.path()), 5, "{RETRY_MARKER}");
    assert_usage_matches_walk(&store, root.path(), "retried unlink");
    store.ingest_artifact(request("after", b"after")).unwrap();
    assert_usage_matches_walk(&store, root.path(), "ingest after every decrement");
}

#[test]
fn bytes_added_outside_the_store_fail_the_next_admission_closed() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open_with_artifact_cap_for_test(root.path(), 64).unwrap();
    seed_domain(&store);
    store.ingest_artifact(request("one", b"1234")).unwrap();
    fs::write(root.path().join("artifacts/objects/stray"), b"drift").unwrap();
    store.run_staging_maintenance(0).unwrap();
    let error = store.ingest_artifact(request("two", b"56")).unwrap_err();
    assert_eq!(error.kind(), ArtifactErrorKind::IngestionFailClosed);
    drop(store);

    // A new store seeds its count from the tree as it now stands.
    let store = KernelStore::open_with_artifact_cap_for_test(root.path(), 64).unwrap();
    assert_usage_matches_walk(&store, root.path(), "reopen after drift");
    store.ingest_artifact(request("two", b"56")).unwrap();
}

#[test]
fn bytes_removed_outside_the_store_fail_the_next_admission_closed() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open_with_artifact_cap_for_test(root.path(), 64).unwrap();
    seed_domain(&store);
    let handle = store.ingest_artifact(request("one", b"1234")).unwrap();
    fs::remove_file(object_path(root.path(), &handle.digest)).unwrap();
    store.run_staging_maintenance(0).unwrap();
    let error = store.ingest_artifact(request("two", b"56")).unwrap_err();
    assert_eq!(error.kind(), ArtifactErrorKind::IngestionFailClosed);
}

#[test]
fn a_batch_counts_each_distinct_payload_once_and_a_refused_batch_subtracts_it() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open_with_artifact_cap_for_test(root.path(), 64).unwrap();
    seed_domain(&store);
    store.ingest_artifact(request("single", b"single")).unwrap();
    store
        .ingest_exact_artifacts_with(
            intent("batch", b"batch"),
            vec![
                request("first", b"first"),
                request("second", b"second"),
                request("first-again", b"first"),
            ],
            None,
            |_, _| Ok("batched".to_string()),
        )
        .unwrap();
    assert_eq!(reported(&store), 17);
    assert_usage_matches_walk(&store, root.path(), "batch publish");

    let refused = store.ingest_exact_artifacts_with(
        intent("refused", b"refused"),
        vec![request("third", b"third"), request("held", b"single")],
        None,
        |_, _| Err(kernel::KernelError::Conflict),
    );
    assert_eq!(refused.unwrap_err(), kernel::KernelError::Conflict);
    assert_eq!(reported(&store), 17);
    assert_usage_matches_walk(&store, root.path(), "refused batch");
    store.ingest_artifact(request("after", b"after")).unwrap();
}

#[test]
fn a_removal_larger_than_the_count_fails_the_next_admission_closed() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open_with_artifact_cap_for_test(root.path(), 64).unwrap();
    seed_domain(&store);
    let handle = store.ingest_artifact(request("grown", b"1234")).unwrap();
    // Growing the object outside the store makes its unlink remove more bytes
    // than the store ever counted.
    let path = object_path(root.path(), &handle.digest);
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(&path, permissions).unwrap();
    fs::write(&path, b"1234 and more bytes").unwrap();
    store
        .delete_artifact(purge("grown", &handle.digest))
        .unwrap();
    assert_usage_matches_walk(&store, root.path(), "purge after growth");
    let error = store.ingest_artifact(request("next", b"56")).unwrap_err();
    assert_eq!(error.kind(), ArtifactErrorKind::IngestionFailClosed);
}

#[test]
fn a_restore_reconciles_the_count_it_held_and_keeps_admission_open() {
    let root = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    fs::set_permissions(destination.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let store = KernelStore::open_with_artifact_cap_for_test(root.path(), 64).unwrap();
    seed_domain(&store);
    store.ingest_artifact(request("kept", b"kept")).unwrap();
    let backup = store
        .backup(BackupRequest {
            destination_directory: destination.path().to_path_buf(),
            deadline: Instant::now() + Duration::from_secs(30),
            capture_pin_expires_at: None,
        })
        .unwrap();
    store.ingest_artifact(request("later", b"later")).unwrap();
    let walks = store.artifact_census_walks_for_test();
    store.restore(&backup.destination_path).unwrap();
    assert_eq!(
        store.artifact_census_walks_for_test(),
        walks + 1,
        "restore recovery reconciles once"
    );
    assert_usage_matches_walk(&store, root.path(), "restore");
    store.ingest_artifact(request("after", b"after")).unwrap();
    assert_usage_matches_walk(&store, root.path(), "ingest after restore");
}
