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
fn a_damaged_object_at_the_offered_digest_is_refused_as_corruption_without_replacement() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    let offered = b"the bytes being offered".to_vec();
    let digest = format!("{:x}", Sha256::digest(&offered));
    // An object whose SHA-256 differs from its path digest is corrupt. Both
    // lanes must report a store fault rather than blame the payload.
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
        assert_eq!(error.kind(), ArtifactErrorKind::CorruptObject, "{lane}");
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

fn reservation_count(root: &Path) -> i64 {
    rusqlite::Connection::open_with_flags(
        root.join("kernel.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
    .query_row(
        "SELECT COUNT(*) FROM artifact_ingestion_reservations",
        [],
        |row| row.get(0),
    )
    .unwrap()
}

/// A batch stores each distinct payload once, records every request's evidence and receipt in the commit that also runs its operation, and each request ingested alone afterward replays that receipt without writing.
#[test]
fn a_batch_retains_each_payload_once_and_every_request_replays_alone() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    let texts = [FIXTURES[1].1, FIXTURES[2].1, FIXTURES[1].1];
    let requests = || {
        texts
            .iter()
            .enumerate()
            .map(|(index, text)| request(&format!("batch-{index}"), text.as_bytes().to_vec()))
            .collect::<Vec<_>>()
    };
    let staged = store.staged_artifacts_for_test();
    let tip = store.tip().unwrap();
    let mut seen = Vec::new();
    let receipt = store
        .ingest_exact_artifacts_with(intent("batch", b"batch"), requests(), None, |_, handles| {
            seen = handles.to_vec();
            Ok("batched".to_string())
        })
        .unwrap();
    assert!(!receipt.replayed);
    assert_eq!(receipt.result, "batched");
    assert_eq!(
        store.tip().unwrap(),
        tip + 1,
        "one commit for the whole batch"
    );
    assert_eq!(
        store.staged_artifacts_for_test(),
        staged + 2,
        "equal bytes stage once"
    );
    assert_eq!(object_count(root.path()), 2);
    assert_eq!(evidence_count(root.path()), 3);
    assert_eq!(
        (tmp_count(root.path()), reservation_count(root.path())),
        (0, 0)
    );
    for (handle, text) in seen.iter().zip(texts) {
        assert_eq!(
            handle.digest,
            format!("{:x}", Sha256::digest(text.as_bytes()))
        );
        assert_eq!(store.read_artifact(handle).unwrap(), text.as_bytes());
    }
    for (request, handle) in requests().into_iter().zip(&seen) {
        assert_eq!(&store.ingest_exact_artifact(request).unwrap(), handle);
    }
    // The batch's own key replays too, before any byte is staged: no operation, no reference, no temporary file.
    let staged = store.staged_artifacts_for_test();
    let replayed = store
        .ingest_exact_artifacts_with(intent("batch", b"batch"), requests(), None, |_, _| {
            panic!("a replayed batch runs no operation")
        })
        .unwrap();
    assert!(replayed.replayed);
    assert_eq!(store.staged_artifacts_for_test(), staged);
    assert_eq!(store.tip().unwrap(), tip + 1);
    assert_eq!(object_count(root.path()), 2);
    assert_eq!(evidence_count(root.path()), 3);
    assert_eq!(
        (tmp_count(root.path()), reservation_count(root.path())),
        (0, 0)
    );
}

/// A refused batch commits nothing: a failing operation, a request whose key another commit already recorded, and an inadmissible payload each leave no evidence, reservation, temporary file, or new object, while an object an earlier reference holds stays.
#[test]
fn a_refused_batch_commits_nothing_and_removes_only_the_bytes_it_added() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    let tip = store.tip().unwrap();
    // Two requests share the first payload's object; the refusal removes it once both reservations are released.
    let refused = store.ingest_exact_artifacts_with(
        intent("refused", b"refused"),
        vec![
            request("fresh", FIXTURES[1].1.as_bytes().to_vec()),
            request("other", FIXTURES[3].1.as_bytes().to_vec()),
            request("fresh-again", FIXTURES[1].1.as_bytes().to_vec()),
        ],
        None,
        |_, _| Err(kernel::KernelError::Conflict),
    );
    assert_eq!(refused.unwrap_err(), kernel::KernelError::Conflict);
    assert_eq!(store.tip().unwrap(), tip);
    assert_eq!(
        (object_count(root.path()), evidence_count(root.path())),
        (0, 0)
    );
    assert_eq!(
        (tmp_count(root.path()), reservation_count(root.path())),
        (0, 0)
    );

    let held = store
        .ingest_exact_artifact(request("held", FIXTURES[2].1.as_bytes().to_vec()))
        .unwrap();
    let tip = store.tip().unwrap();
    let reused = store.ingest_exact_artifacts_with(
        intent("reused", b"reused"),
        vec![
            request("new", FIXTURES[3].1.as_bytes().to_vec()),
            request("held", FIXTURES[2].1.as_bytes().to_vec()),
        ],
        None,
        |_, _| Ok(String::new()),
    );
    assert_eq!(reused.unwrap_err(), kernel::KernelError::Conflict);
    assert_eq!(store.tip().unwrap(), tip);
    assert_eq!(
        (object_count(root.path()), evidence_count(root.path())),
        (1, 1)
    );
    assert_eq!(
        (tmp_count(root.path()), reservation_count(root.path())),
        (0, 0)
    );
    assert_eq!(
        store.read_artifact(&held).unwrap(),
        FIXTURES[2].1.as_bytes()
    );

    let staged = store.staged_artifacts_for_test();
    let secret = store.ingest_exact_artifacts_with(
        intent("secret", b"secret"),
        vec![
            request("clean", FIXTURES[1].1.as_bytes().to_vec()),
            request("secret", format!("token {SECRET}").into_bytes()),
        ],
        None,
        |_, _| Ok(String::new()),
    );
    assert_eq!(secret.unwrap_err(), kernel::KernelError::InvalidInput);
    assert_eq!(
        store.staged_artifacts_for_test(),
        staged,
        "refused before staging"
    );
    assert!(!residue_has(root.path(), SECRET.as_bytes()));
    assert_eq!(store.tip().unwrap(), tip);
}

/// A batch with no requests is one commit of its operation, which may record receipts for other keys; each recorded key then replays its result instead of running a later operation, and recording a key twice refuses the commit.
#[test]
fn an_empty_batch_commits_its_operation_and_recorded_receipts_replay() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    let tip = store.tip().unwrap();
    let receipt = store
        .ingest_exact_artifacts_with(
            intent("empty", b"empty"),
            Vec::new(),
            None,
            |envelope, handles| {
                assert!(handles.is_empty());
                envelope.record_receipt(intent("recorded", b"recorded"), "recorded result")?;
                Ok("main".to_string())
            },
        )
        .unwrap();
    assert!(!receipt.replayed);
    assert_eq!(store.tip().unwrap(), tip + 1);
    let replay = store
        .commit(intent("recorded", b"recorded"), |_| {
            panic!("a recorded key runs no operation")
        })
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(
        (replay.commit_seq, replay.result.as_str()),
        (tip + 1, "recorded result")
    );
    // The same key under another request digest is the conflict the digest exists to catch.
    assert_eq!(
        store
            .commit(intent("recorded", b"other"), |_| Ok(String::new()))
            .unwrap_err(),
        kernel::KernelError::Conflict
    );
    let twice = store.commit(intent("twice", b"twice"), |envelope| {
        envelope.record_receipt(intent("recorded", b"recorded"), "again")?;
        Ok(String::new())
    });
    assert_eq!(twice.unwrap_err(), kernel::KernelError::Conflict);
    assert_eq!(store.tip().unwrap(), tip + 1);
}

/// A batch larger than one staging chunk retains every payload, and a refusal after staging several chunks leaves no temporary file, reservation, or object behind.
#[test]
fn a_batch_of_many_payloads_stages_in_chunks_and_a_refusal_leaves_a_clean_store() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    let requests = |prefix: &str| {
        (0..40)
            .map(|index| {
                request(
                    &format!("{prefix}-{index}"),
                    format!("payload {index}").into_bytes(),
                )
            })
            .collect::<Vec<_>>()
    };
    let refused = store.ingest_exact_artifacts_with(
        intent("many-refused", b"many-refused"),
        requests("refused"),
        None,
        |_, _| Err(kernel::KernelError::Conflict),
    );
    assert_eq!(refused.unwrap_err(), kernel::KernelError::Conflict);
    assert_eq!(
        (object_count(root.path()), evidence_count(root.path())),
        (0, 0)
    );
    assert_eq!(
        (tmp_count(root.path()), reservation_count(root.path())),
        (0, 0)
    );
    let receipt = store
        .ingest_exact_artifacts_with(
            intent("many", b"many"),
            requests("kept"),
            None,
            |_, handles| {
                assert_eq!(handles.len(), 40);
                Ok(String::new())
            },
        )
        .unwrap();
    assert!(!receipt.replayed);
    assert_eq!(
        (object_count(root.path()), evidence_count(root.path())),
        (40, 40)
    );
    assert_eq!(
        (tmp_count(root.path()), reservation_count(root.path())),
        (0, 0)
    );
}
