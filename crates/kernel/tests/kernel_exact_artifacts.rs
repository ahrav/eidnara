//! Independent native-string and commit-message fixtures round-trip through the real CAS under exact retention, and every refusal happens before a byte is persisted.

#![cfg(feature = "test-support")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use kernel::{
    ArtifactErrorKind, ArtifactIngestRequest, CommitIntent, DomainSpec, KernelStore,
    ProviderEgress, RepositoryProvenance, Sensitivity,
};
use sha2::{Digest, Sha256};

const SECRET: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyzABCDEFGH12345678";
const HOUR_MS: i64 = 60 * 60 * 1_000;

/// Native fixtures written out by hand, with the digest each one must store under.
const FIXTURES: &[(&str, &str)] = &[
    ("empty", ""),
    ("ascii", "Deploy failed"),
    ("crlf", "error: line 1\r\nwarning: naïve 日本語 🎉\r\n"),
    ("whitespace", "  leading\ttab\r\nCRLF and trailing  \n\n"),
    ("unicode", "naïve — 日本語 🎉 \u{200b} zero-width"),
    (
        "multiline_commit",
        "Fix the thing\n\nBody line.\n\nSigned-off-by: someone\n",
    ),
    ("json_looking", "{\"count\": 1, \"items\": [true, null]}\n"),
    (
        "traceback",
        "Traceback (most recent call last):\n  File \"x.py\"\n",
    ),
];

fn intent(key: &str, payload: &[u8]) -> CommitIntent {
    CommitIntent {
        producer: "kernel-exact-artifact-test".to_string(),
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

fn request(key: &str, payload: Vec<u8>) -> ArtifactIngestRequest {
    ArtifactIngestRequest {
        intent: intent(key, &payload),
        payload,
        evidence_id: format!("evidence-{key}"),
        object_id: format!("evidence-object-{key}"),
        object_kind: "evidence".to_string(),
        domain_id: "domain".to_string(),
        source_kind: "tool_output".to_string(),
        source_id: format!("native/{key}"),
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

fn object_path(root: &Path, digest: &str) -> PathBuf {
    root.join("artifacts/objects")
        .join(&digest[..2])
        .join(&digest[2..])
}

fn object_count(root: &Path) -> usize {
    let objects = root.join("artifacts/objects");
    if !objects.exists() {
        return 0;
    }
    fs::read_dir(objects)
        .unwrap()
        .flat_map(|shard| fs::read_dir(shard.unwrap().path()).unwrap())
        .count()
}

fn tmp_count(root: &Path) -> usize {
    fs::read_dir(root.join("artifacts/tmp"))
        .map(|entries| entries.count())
        .unwrap_or(0)
}

fn evidence_count(root: &Path) -> i64 {
    rusqlite::Connection::open_with_flags(
        root.join("kernel.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
    .query_row("SELECT COUNT(*) FROM evidence_meta", [], |row| row.get(0))
    .unwrap()
}

/// Every byte of the store's files and database, so a refusal can be shown to
/// have left nothing behind, including redacted copies and temporary files.
fn residue_has(root: &Path, needle: &[u8]) -> bool {
    fn walk(path: &Path, needle: &[u8], hit: &mut bool) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let file_type = entry.file_type().unwrap();
            if file_type.is_dir() {
                walk(&entry.path(), needle, hit);
            } else if file_type.is_file() {
                let bytes = fs::read(entry.path()).unwrap();
                if bytes.windows(needle.len()).any(|window| window == needle) {
                    *hit = true;
                }
            }
        }
    }
    let mut hit = false;
    walk(root, needle, &mut hit);
    hit
}

#[test]
fn native_fixtures_round_trip_through_exact_retention_with_identical_bytes_and_digests() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    for (key, text) in FIXTURES {
        let expected_digest = format!("{:x}", Sha256::digest(text.as_bytes()));
        let handle = store
            .ingest_exact_artifact(request(key, text.as_bytes().to_vec()))
            .unwrap();
        assert_eq!(handle.digest, expected_digest, "{key}");
        let stored = store.read_artifact(&handle).unwrap();
        assert_eq!(stored, text.as_bytes(), "{key} bytes");
        assert_eq!(
            fs::read(object_path(root.path(), &handle.digest)).unwrap(),
            text.as_bytes(),
            "{key} object file"
        );
        // The same text through the redacting lane stores the same bytes: a
        // clean payload is not rewritten, so the two lanes agree.
        let redacting = store
            .ingest_artifact(request(
                &format!("{key}-redacting"),
                text.as_bytes().to_vec(),
            ))
            .unwrap();
        assert_eq!(redacting.digest, expected_digest, "{key} redacting lane");
    }
    // Two distinct records with equal text share one object and keep two
    // evidence rows.
    assert_eq!(object_count(root.path()), FIXTURES.len());
    assert_eq!(evidence_count(root.path()), 2 * FIXTURES.len() as i64);
    assert_eq!(tmp_count(root.path()), 0);
}

#[test]
fn a_secret_is_refused_before_persistence_with_a_reason_that_names_no_content() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    store
        .ingest_exact_artifact(request("clean", b"clean text".to_vec()))
        .unwrap();
    let objects = object_count(root.path());
    let evidence = evidence_count(root.path());
    let tip = store.tip().unwrap();
    let staged = store.staged_artifacts_for_test();

    // The last canary carries no credential; the scanner's generic key rule
    // still rewrites it, and exact retention refuses rather than storing the
    // rewritten form.
    let canaries: Vec<Vec<u8>> = vec![
        format!("prefix {SECRET} suffix").into_bytes(),
        SECRET.as_bytes().to_vec(),
        format!("line 1\n{SECRET}\nline 3\n").into_bytes(),
        b"{\"key\": \"value\"}\n".to_vec(),
    ];
    for (index, canary) in canaries.iter().enumerate() {
        let error = store
            .ingest_exact_artifact(request(&format!("canary-{index}"), canary.clone()))
            .unwrap_err();
        assert_eq!(error.kind(), ArtifactErrorKind::ExactBytesRewritten);
        // The message is a fixed sentence; nothing from the payload reaches it.
        assert_eq!(
            format!("{error}"),
            "artifact payload holds a recognized secret and exact retention refuses to rewrite it"
        );
        assert_eq!(format!("{error:?}"), format!("{error}"));
    }
    // Scan failures on the exact lane keep their existing non-content kinds
    // and are refused at the same point.
    let flood = "key=a ".repeat(5000).into_bytes();
    let error = store
        .ingest_exact_artifact(request("flood", flood))
        .unwrap_err();
    assert_eq!(error.kind(), ArtifactErrorKind::DetectionLimit);
    let mut windowed = String::new();
    while windowed.len() < 2 * context_core::redaction::MAX_REDACTABLE_BYTES {
        windowed.push_str("password=hunter-two-");
        windowed.push_str(&windowed.len().to_string());
        windowed.push('\n');
    }
    let error = store
        .ingest_exact_artifact(request("windowed", windowed.into_bytes()))
        .unwrap_err();
    assert_eq!(error.kind(), ArtifactErrorKind::DetectionLimit);

    // Nothing was staged or written: the staging step never ran, no object,
    // no temp file, no evidence row, no commit, and the canary bytes are
    // nowhere under the root.
    assert_eq!(
        store.staged_artifacts_for_test(),
        staged,
        "a refusal reached staging"
    );
    assert_eq!(object_count(root.path()), objects);
    assert_eq!(tmp_count(root.path()), 0);
    assert_eq!(evidence_count(root.path()), evidence);
    assert_eq!(store.tip().unwrap(), tip);
    assert!(!residue_has(root.path(), SECRET.as_bytes()));
    assert!(!residue_has(root.path(), b"hunter-two"));
    assert!(residue_has(root.path(), b"clean text"));

    // The redacting lane keeps its established behavior on the same input:
    // it stores the placeholder form under a different digest.
    let redacted = store
        .ingest_artifact(request("redacted", canaries[0].clone()))
        .unwrap();
    assert_ne!(
        redacted.digest,
        format!("{:x}", Sha256::digest(&canaries[0]))
    );
    let stored = store.read_artifact(&redacted).unwrap();
    assert!(
        !stored
            .windows(SECRET.len())
            .any(|window| window == SECRET.as_bytes())
    );
    assert!(!residue_has(root.path(), SECRET.as_bytes()));
}

#[test]
fn oversized_and_non_utf8_payloads_are_refused_before_materialization() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    let tip = store.tip().unwrap();
    let staged_before = store.staged_artifacts_for_test();

    let oversized = vec![b'a'; kernel::MAX_PAYLOAD_BYTES + 1];
    let error = store
        .ingest_exact_artifact(request("oversized", oversized))
        .unwrap_err();
    assert_eq!(error.kind(), ArtifactErrorKind::PayloadTooLarge);

    for (key, bytes) in [
        ("truncated_multibyte", b"caf\xc3".to_vec()),
        ("invalid_start", vec![0xff, 0xfe, b'a']),
        ("nul_in_binary", vec![0x00, 0x80, 0x01]),
    ] {
        let staged = store.staged_artifacts_for_test();
        let error = store
            .ingest_exact_artifact(request(key, bytes.clone()))
            .unwrap_err();
        assert_eq!(error.kind(), ArtifactErrorKind::UnsupportedShape, "{key}");
        assert_eq!(
            format!("{error}"),
            "artifact payload is not valid UTF-8 text",
            "{key}"
        );
        assert_eq!(
            store.staged_artifacts_for_test(),
            staged,
            "{key} reached staging"
        );
        // The redacting lane still accepts these as binary payloads.
        let handle = store
            .ingest_artifact(request(&format!("{key}-binary"), bytes.clone()))
            .unwrap();
        assert_eq!(store.read_artifact(&handle).unwrap(), bytes);
    }
    assert_eq!(tmp_count(root.path()), 0);
    // Only the three redacting-lane ingests staged and committed.
    assert_eq!(store.staged_artifacts_for_test(), staged_before + 3);
    assert_eq!(store.tip().unwrap(), tip + 3);
    assert_eq!(object_count(root.path()), 3);
}

#[test]
fn a_forced_digest_collision_fails_without_replacing_stored_evidence() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    let offered = b"the bytes being offered".to_vec();
    let digest = format!("{:x}", Sha256::digest(&offered));
    // Plant different bytes at the path the offered digest maps to. The store
    // has never seen either payload, so this is the only way two byte strings
    // can meet under one digest.
    let planted = b"different stored bytes";
    let path = object_path(root.path(), &digest);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(&path, planted).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let evidence = evidence_count(root.path());
    let tip = store.tip().unwrap();

    for lane in ["exact", "redacting"] {
        let error = match lane {
            "exact" => store.ingest_exact_artifact(request(lane, offered.clone())),
            _ => store.ingest_artifact(request(lane, offered.clone())),
        }
        .unwrap_err();
        assert_eq!(error.kind(), ArtifactErrorKind::DigestCollision, "{lane}");
        assert!(format!("{error}").contains(&digest));
        assert_eq!(
            fs::read(&path).unwrap(),
            planted,
            "{lane} replaced the stored bytes"
        );
        assert_eq!(evidence_count(root.path()), evidence, "{lane}");
        assert_eq!(store.tip().unwrap(), tip, "{lane}");
        assert_eq!(tmp_count(root.path()), 0, "{lane}");
    }
    // The planted object has no reference, so it is an orphan outside the
    // source inventory; a later sweep may reclaim it, but nothing cites it.
    let referenced: i64 = rusqlite::Connection::open_with_flags(
        root.path().join("kernel.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
    .query_row(
        "SELECT COUNT(*) FROM evidence_meta WHERE artifact_digest=?1",
        [&digest],
        |row| row.get(0),
    )
    .unwrap();
    assert_eq!(referenced, 0);
}

#[test]
fn replaying_an_exact_ingest_after_reopen_returns_the_receipt_without_a_second_object() {
    let root = tempfile::tempdir().unwrap();
    let text = FIXTURES[2].1;
    let (first, tip) = {
        let store = KernelStore::open(root.path()).unwrap();
        seed_domain(&store);
        let handle = store
            .ingest_exact_artifact(request("replay", text.as_bytes().to_vec()))
            .unwrap();
        (handle, store.tip().unwrap())
    };
    let store = KernelStore::open(root.path()).unwrap();
    let again = store
        .ingest_exact_artifact(request("replay", text.as_bytes().to_vec()))
        .unwrap();
    assert_eq!(again, first);
    assert_eq!(store.tip().unwrap(), tip);
    assert_eq!(object_count(root.path()), 1);
    assert_eq!(evidence_count(root.path()), 1);
    assert_eq!(store.read_artifact(&again).unwrap(), text.as_bytes());
    // The same intent with different bytes is not a replay.
    let error = store
        .ingest_exact_artifact(request("replay", b"other bytes".to_vec()))
        .unwrap_err();
    assert_eq!(error.kind(), ArtifactErrorKind::OperationKeyReused);
}

#[test]
fn cas_only_objects_stay_outside_the_source_inventory_and_get_swept() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    let held = store
        .ingest_exact_artifact(request("held", b"held text".to_vec()))
        .unwrap();
    // An object with no evidence row is CAS-only: not cited, not inventory.
    let orphan_digest = "c".repeat(64);
    let orphan = object_path(root.path(), &orphan_digest);
    fs::create_dir_all(orphan.parent().unwrap()).unwrap();
    fs::set_permissions(orphan.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(&orphan, b"orphan").unwrap();
    fs::File::open(&orphan)
        .unwrap()
        .set_times(
            fs::FileTimes::new()
                .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1)),
        )
        .unwrap();

    let result = store.run_staging_maintenance(2 * HOUR_MS).unwrap();
    assert_eq!(result.artifact_gc.reclaimed_objects, 1);
    assert!(!orphan.exists());
    assert!(object_path(root.path(), &held.digest).exists());
    assert_eq!(store.read_artifact(&held).unwrap(), b"held text");
}
