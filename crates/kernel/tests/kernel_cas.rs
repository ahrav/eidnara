//! These tests cover artifact ingestion, policy, durability, and filesystem hardening.

#![cfg(feature = "test-support")]

use std::fs;
use std::os::unix::fs::PermissionsExt;

use kernel::{
    ArtifactDestination, ArtifactEgressFacts, ArtifactEligibility, ArtifactErrorKind,
    ArtifactHandle, ArtifactIngestFault, ArtifactIngestRequest, CommitIntent, DomainSpec,
    EligibilityDeniedReason, KernelStore, MAX_PAYLOAD_BYTES, ProviderEgress, RepositoryProvenance,
    Sensitivity,
};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};

const SECRET: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyzABCDEFGH12345678";
const MIB: usize = 1024 * 1024;

fn assert_send_sync<T: Send + Sync>() {}

fn intent(key: &str, payload: &[u8]) -> CommitIntent {
    CommitIntent {
        producer: "kernel-cas-test".to_string(),
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

fn artifact_path(root: &std::path::Path, digest: &str) -> std::path::PathBuf {
    root.join("artifacts/objects")
        .join(&digest[..2])
        .join(&digest[2..])
}

fn invalidate_evidence(store: &KernelStore, root: &std::path::Path, evidence_id: &str) {
    // invalidated_commit_seq must reference a commit strictly later than the one
    // that created the row, so take a real later sequence rather than reusing
    // created_commit_seq.
    let commit_seq = store
        .commit(
            intent(&format!("invalidate-{evidence_id}"), evidence_id.as_bytes()),
            |_| Ok(evidence_id.to_string()),
        )
        .unwrap()
        .commit_seq;
    let connection = Connection::open(root.join("kernel.sqlite")).unwrap();
    connection
        .execute(
            "UPDATE evidence_meta SET invalidated_commit_seq=?2 WHERE evidence_id=?1",
            params![evidence_id, commit_seq],
        )
        .unwrap();
}

/// An unreadable entry panics so a secret-absence scan cannot pass vacuously.
fn tree_bytes(path: &std::path::Path) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut pending = vec![path.to_path_buf()];
    while let Some(path) = pending.pop() {
        let metadata = fs::symlink_metadata(&path)
            .unwrap_or_else(|error| panic!("stat {}: {error}", path.display()));
        if metadata.is_dir() {
            pending.extend(
                fs::read_dir(&path)
                    .unwrap_or_else(|error| panic!("read_dir {}: {error}", path.display()))
                    .map(|entry| entry.unwrap().path()),
            );
        } else if metadata.is_file() {
            bytes.extend(
                fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display())),
            );
        }
    }
    assert!(!bytes.is_empty(), "no file bytes under {}", path.display());
    bytes
}

#[test]
fn ingest_publishes_sharded_redacted_bytes_and_commits_live_reference() {
    assert_send_sync::<KernelStore>();
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    let payload = format!("prefix {SECRET} suffix").into_bytes();

    let handle = store.ingest_artifact(request("happy", payload)).unwrap();
    let stored = store.read_artifact(&handle).unwrap();

    assert!(
        !stored
            .windows(SECRET.len())
            .any(|window| window == SECRET.as_bytes())
    );
    assert_eq!(format!("{:x}", Sha256::digest(&stored)), handle.digest);
    assert_eq!(
        fs::read(artifact_path(root.path(), &handle.digest)).unwrap(),
        stored
    );
    assert_eq!(
        fs::metadata(root.path().join("artifacts"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert!(
        !tree_bytes(root.path())
            .windows(SECRET.len())
            .any(|window| window == SECRET.as_bytes())
    );

    let connection = Connection::open(root.path().join("kernel.sqlite")).unwrap();
    let row: (i64, i64, String, String) = connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM evidence_meta WHERE evidence_id=?1 AND invalidated_commit_seq IS NULL),
                    (SELECT COUNT(*) FROM artifact_ingestion_reservations WHERE artifact_digest=?2),
                    detector_id,secret_type
             FROM evidence_meta WHERE evidence_id=?1",
            params![handle.evidence_id, handle.digest],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        row,
        (
            1,
            0,
            "eidnara-secret-scanner".to_string(),
            "anthropic_api_key".to_string()
        )
    );
}

fn live_reservations(root: &std::path::Path) -> i64 {
    Connection::open(root.join("kernel.sqlite"))
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM artifact_ingestion_reservations",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

fn temp_entries(root: &std::path::Path) -> usize {
    fs::read_dir(root.join("artifacts/tmp"))
        .map(|entries| entries.count())
        .unwrap_or(0)
}

/// UTF-8 payload of `total` bytes made of neutral lines, with `line` inserted
/// at the first line boundary at or before `target_offset`.
fn large_text_payload(total: usize, target_offset: usize, line: &str) -> (Vec<u8>, usize) {
    const FILLER: &str = "plain filler line without any credential words 0123\n";
    let mut text = String::with_capacity(total + FILLER.len() + line.len());
    while text.len() + FILLER.len() <= target_offset {
        text.push_str(FILLER);
    }
    let offset = text.len();
    text.push_str(line);
    text.push('\n');
    while text.len() < total {
        text.push_str(FILLER);
    }
    text.truncate(total);
    (text.into_bytes(), offset)
}

#[test]
fn payload_limit_is_inclusive_at_the_artifact_cap() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);

    assert_eq!(MAX_PAYLOAD_BYTES, 64 * MIB);
    // Direct redaction would replace text this long with one placeholder; the
    // windowed payload scan keeps the bytes and finds nothing to redact.
    let (payload, _) = large_text_payload(MAX_PAYLOAD_BYTES, 0, "first");
    assert!(payload.len() > context_core::redaction::MAX_REDACTABLE_BYTES);
    let handle = store
        .ingest_artifact(request("limit", payload.clone()))
        .unwrap();
    assert_eq!(store.read_artifact(&handle).unwrap(), payload);
    assert_eq!(
        store
            .artifact_eligibility(&handle, ArtifactDestination::Remote)
            .unwrap(),
        ArtifactEligibility::Allowed
    );

    let (payload, _) = large_text_payload(MAX_PAYLOAD_BYTES + 1, 0, "first");
    let error = store
        .ingest_artifact(request("over-limit", payload))
        .unwrap_err();
    assert_eq!(error.kind(), ArtifactErrorKind::PayloadTooLarge);
    assert_eq!(live_reservations(root.path()), 0);
    assert_eq!(temp_entries(root.path()), 0);
}

#[test]
fn cap_sized_payload_is_redacted_in_windows_and_the_digest_names_the_redacted_bytes() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);

    let secret_line = format!("key {SECRET} tail");
    let (payload, offset) = large_text_payload(MAX_PAYLOAD_BYTES, 40 * MIB, &secret_line);
    assert_eq!(payload.len(), MAX_PAYLOAD_BYTES);
    let handle = store
        .ingest_artifact(request("deep-secret", payload.clone()))
        .unwrap();

    let stored = store.read_artifact(&handle).unwrap();
    assert!(
        !stored
            .windows(SECRET.len())
            .any(|window| window == SECRET.as_bytes())
    );
    assert!(
        stored
            .windows(b"<ANTHROPIC_API_KEY_REDACTED>".len())
            .any(|window| window == b"<ANTHROPIC_API_KEY_REDACTED>")
    );
    assert_eq!(handle.digest, format!("{:x}", Sha256::digest(&stored)));
    assert_eq!(
        stored.len(),
        payload.len() - SECRET.len() + "<ANTHROPIC_API_KEY_REDACTED>".len()
    );
    // Bytes on either side of the placeholder are untouched.
    assert_eq!(&stored[..offset + 4], &payload[..offset + 4]);

    let (detector, secret_type, redaction_offset): (String, String, i64) =
        Connection::open(root.path().join("kernel.sqlite"))
            .unwrap()
            .query_row(
                "SELECT detector_id,secret_type,source_utf8_offset FROM durable_text_redactions
                 WHERE owner_kind='evidence' AND owner_id=?1",
                [handle.evidence_id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
    assert_eq!(detector, "eidnara-secret-scanner");
    assert_eq!(secret_type, "anthropic_api_key");
    assert_eq!(redaction_offset, i64::try_from(offset + 4).unwrap());
    assert!(
        !tree_bytes(root.path())
            .windows(SECRET.len())
            .any(|window| window == SECRET.as_bytes())
    );
}

#[test]
fn cap_sized_binary_payload_is_inspected_whole() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);

    // Every 0xff widens to a three-byte replacement character, so the inspected
    // text is three times the payload; the windowed scan still covers it.
    let mut clean = vec![0xff_u8; 2 * MIB];
    clean[MIB] = b'\n';
    let handle = store
        .ingest_artifact(request("wide-binary", clean.clone()))
        .unwrap();
    assert_eq!(store.read_artifact(&handle).unwrap(), clean);
    assert_eq!(
        store
            .artifact_eligibility(&handle, ArtifactDestination::Remote)
            .unwrap(),
        ArtifactEligibility::Denied(EligibilityDeniedReason::SensitiveRemote)
    );

    let mut leaking = vec![0xff_u8; 2 * MIB];
    leaking[MIB..MIB + SECRET.len()].copy_from_slice(SECRET.as_bytes());
    let error = store
        .ingest_artifact(request("wide-binary-secret", leaking))
        .unwrap_err();
    assert_eq!(error.kind(), ArtifactErrorKind::UnredactableSecret);
    assert_eq!(live_reservations(root.path()), 0);
}

#[test]
fn unproven_normal_and_non_utf8_are_clamped_sensitive() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    let mut unproven = request("unproven", b"clean".to_vec());
    unproven.provenance = None;
    let unproven = store.ingest_artifact(unproven).unwrap();
    let binary = store
        .ingest_artifact(request("binary", vec![0xff, 0xfe, 0xfd]))
        .unwrap();

    for handle in [&unproven, &binary] {
        assert_eq!(
            store
                .artifact_eligibility(handle, ArtifactDestination::Remote)
                .unwrap(),
            ArtifactEligibility::Denied(EligibilityDeniedReason::SensitiveRemote)
        );
        assert_eq!(
            store
                .artifact_eligibility(handle, ArtifactDestination::Local)
                .unwrap(),
            ArtifactEligibility::Allowed
        );
    }
}

#[test]
fn cap_error_reports_usage_and_cap_and_counts_an_invalidated_retained_object() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open_with_artifact_cap_for_test(root.path(), 8).unwrap();
    seed_domain(&store);
    let handle = store
        .ingest_artifact(request("within-cap", b"12345678".to_vec()))
        .unwrap();
    let error = store
        .ingest_artifact(request("over-cap", b"9".to_vec()))
        .unwrap_err();

    assert_eq!(error.kind(), ArtifactErrorKind::Capacity);
    assert_eq!(error.usage(), Some(8));
    assert_eq!(error.cap(), Some(8));
    assert_eq!(store.read_artifact(&handle).unwrap(), b"12345678");

    // Invalidating the only reference retains the object bytes, and retained
    // bytes still count against the cap.
    invalidate_evidence(&store, root.path(), &handle.evidence_id);
    let error = store
        .ingest_artifact(request("over-retained-cap", b"9".to_vec()))
        .unwrap_err();
    assert_eq!(error.kind(), ArtifactErrorKind::Capacity);
    assert_eq!(error.usage(), Some(8));
}

fn make_fifo(path: &std::path::Path) {
    let made = std::process::Command::new("mkfifo")
        .arg("-m")
        .arg("600")
        .arg(path)
        .status()
        .unwrap();
    assert!(made.success(), "mkfifo failed");
}

#[test]
fn read_rejects_missing_corrupt_oversized_and_fifo_swapped_objects() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);

    for (key, swap, expected) in [
        (
            "missing",
            (|path: &std::path::Path| fs::remove_file(path).unwrap()) as fn(&std::path::Path),
            ArtifactErrorKind::MissingObject,
        ),
        (
            "corrupt",
            |path: &std::path::Path| fs::write(path, b"wrong").unwrap(),
            ArtifactErrorKind::CorruptObject,
        ),
        (
            "fifo-swap",
            |path: &std::path::Path| {
                fs::remove_file(path).unwrap();
                make_fifo(path);
            },
            ArtifactErrorKind::MissingObject,
        ),
        (
            "oversize",
            |path: &std::path::Path| fs::write(path, vec![b'z'; 64 * MIB + 1]).unwrap(),
            ArtifactErrorKind::CorruptObject,
        ),
    ] {
        let handle = store
            .ingest_artifact(request(key, format!("{key} payload").into_bytes()))
            .unwrap();
        swap(&artifact_path(root.path(), &handle.digest));
        assert_eq!(
            store.read_artifact(&handle).unwrap_err().kind(),
            expected,
            "{key}"
        );
    }
}

#[test]
fn read_rejects_malformed_digests_without_path_derivation() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();

    for digest in ["abc".to_string(), format!("{}g", "a".repeat(63))] {
        let handle = ArtifactHandle {
            digest,
            evidence_id: "untrusted".to_string(),
        };
        assert_eq!(
            store.read_artifact(&handle).unwrap_err().kind(),
            ArtifactErrorKind::InvalidInput
        );
    }
}

#[test]
fn eligibility_matrix_includes_secret_unknown_and_tombstone() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    let normal = store
        .ingest_artifact(request("normal", b"public".to_vec()))
        .unwrap();
    let mut sensitive_request = request("sensitive", b"private".to_vec());
    sensitive_request.asserted_sensitivity = Sensitivity::Sensitive;
    let sensitive = store.ingest_artifact(sensitive_request).unwrap();
    let mut secret_request = request("secret", b"classified".to_vec());
    secret_request.asserted_sensitivity = Sensitivity::Secret;
    let secret = store.ingest_artifact(secret_request).unwrap();

    assert_eq!(
        store
            .artifact_eligibility(&normal, ArtifactDestination::Remote)
            .unwrap(),
        ArtifactEligibility::Allowed
    );
    assert_eq!(
        store
            .artifact_eligibility(&sensitive, ArtifactDestination::Local)
            .unwrap(),
        ArtifactEligibility::Allowed
    );
    assert_eq!(
        store
            .artifact_eligibility(&sensitive, ArtifactDestination::Remote)
            .unwrap(),
        ArtifactEligibility::Denied(EligibilityDeniedReason::SensitiveRemote)
    );
    assert_eq!(
        store
            .artifact_eligibility(&secret, ArtifactDestination::Local)
            .unwrap(),
        ArtifactEligibility::Denied(EligibilityDeniedReason::Secret)
    );
    assert_eq!(
        store
            .artifact_eligibility(&secret, ArtifactDestination::Remote)
            .unwrap(),
        ArtifactEligibility::Denied(EligibilityDeniedReason::Secret)
    );
    let unknown = ArtifactHandle {
        digest: "a".repeat(64),
        evidence_id: "absent".to_string(),
    };
    assert_eq!(
        store
            .artifact_eligibility(&unknown, ArtifactDestination::Local)
            .unwrap(),
        ArtifactEligibility::Allowed
    );
    assert_eq!(
        store
            .artifact_eligibility(&unknown, ArtifactDestination::Remote)
            .unwrap(),
        ArtifactEligibility::Denied(EligibilityDeniedReason::UnknownSensitive)
    );

    let connection = Connection::open(root.path().join("kernel.sqlite")).unwrap();
    let commit_seq: i64 = connection
        .query_row("SELECT MAX(commit_seq) FROM commit_log", [], |row| {
            row.get(0)
        })
        .unwrap();
    connection.execute(
        "INSERT INTO artifact_purge_tombstones(artifact_digest,artifact_reference,operator_id,reason,purged_at,commit_seq) VALUES (?1,?2,'operator','test',1,?3)",
        params![normal.digest, format!("objects/{}/{}", &normal.digest[..2], &normal.digest[2..]), commit_seq],
    ).unwrap();
    assert_eq!(
        store
            .artifact_eligibility(&normal, ArtifactDestination::Local)
            .unwrap(),
        ArtifactEligibility::Denied(EligibilityDeniedReason::Tombstoned)
    );
    assert_eq!(
        store
            .artifact_eligibility(&normal, ArtifactDestination::Remote)
            .unwrap(),
        ArtifactEligibility::Denied(EligibilityDeniedReason::Tombstoned)
    );
    assert_eq!(
        store
            .ingest_artifact(request("tombstone-reingest", b"public".to_vec()))
            .unwrap_err()
            .kind(),
        ArtifactErrorKind::ReAdmissionBlocked
    );
}

#[test]
fn egress_facts_carry_the_eligibility_verdict_and_the_stored_class() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    let mut checked = 0;
    for (index, &sensitivity) in Sensitivity::ALL.iter().enumerate() {
        for (offset, &egress) in ProviderEgress::ALL.iter().enumerate() {
            let key = format!("class-{index}-{offset}");
            let mut ingest = request(&key, format!("payload {key}").into_bytes());
            ingest.asserted_sensitivity = sensitivity;
            ingest.provider_egress = egress;
            let handle = store.ingest_artifact(ingest).unwrap();
            for &destination in ArtifactDestination::ALL {
                let facts = store.artifact_egress_facts(&handle, destination).unwrap();
                assert_eq!(
                    facts,
                    ArtifactEgressFacts {
                        eligibility: store.artifact_eligibility(&handle, destination).unwrap(),
                        stored_class: Some((sensitivity, egress)),
                    },
                    "{sensitivity:?} {egress:?} to {destination:?}"
                );
                checked += 1;
            }
        }
    }
    assert_eq!(
        checked,
        Sensitivity::ALL.len() * ProviderEgress::ALL.len() * ArtifactDestination::ALL.len()
    );

    // Two references over the same bytes fold to the stricter class.
    let payload = b"shared bytes".to_vec();
    let merged = store
        .ingest_artifact(request("merged-normal", payload.clone()))
        .unwrap();
    let mut stricter = request("merged-sensitive", payload);
    stricter.asserted_sensitivity = Sensitivity::Sensitive;
    stricter.provider_egress = ProviderEgress::LocalOnly;
    let second = store.ingest_artifact(stricter).unwrap();
    assert_eq!(second.digest, merged.digest);
    assert_eq!(
        fs::read_dir(artifact_path(root.path(), &merged.digest).parent().unwrap())
            .unwrap()
            .count(),
        1
    );
    for handle in [&merged, &second] {
        assert_eq!(
            store
                .artifact_egress_facts(handle, ArtifactDestination::Remote)
                .unwrap(),
            ArtifactEgressFacts {
                eligibility: ArtifactEligibility::Denied(EligibilityDeniedReason::SensitiveRemote),
                stored_class: Some((Sensitivity::Sensitive, ProviderEgress::LocalOnly)),
            }
        );
    }

    let other_payload = b"same public bytes".to_vec();
    let open = store
        .ingest_artifact(request("egress-open", other_payload.clone()))
        .unwrap();
    let mut local_only = request("egress-local", other_payload);
    local_only.provider_egress = ProviderEgress::LocalOnly;
    store.ingest_artifact(local_only).unwrap();
    assert_eq!(
        store
            .artifact_egress_facts(&open, ArtifactDestination::Remote)
            .unwrap(),
        ArtifactEgressFacts {
            eligibility: ArtifactEligibility::Denied(EligibilityDeniedReason::ProviderRestricted),
            stored_class: Some((Sensitivity::Normal, ProviderEgress::LocalOnly)),
        }
    );

    let unknown = ArtifactHandle {
        digest: "a".repeat(64),
        evidence_id: "absent".to_string(),
    };
    for &destination in ArtifactDestination::ALL {
        assert_eq!(
            store.artifact_egress_facts(&unknown, destination).unwrap(),
            ArtifactEgressFacts {
                eligibility: store.artifact_eligibility(&unknown, destination).unwrap(),
                stored_class: None,
            }
        );
    }
    assert_eq!(
        store
            .artifact_egress_facts(&unknown, ArtifactDestination::Remote)
            .unwrap()
            .eligibility,
        ArtifactEligibility::Denied(EligibilityDeniedReason::UnknownSensitive)
    );
}

/// Repeated object ids and digests must produce the same per-candidate
/// results as one read per candidate, in candidate order.
#[test]
fn egress_candidates_line_up_by_index_when_ids_and_digests_repeat() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    let normal = store
        .ingest_artifact(request("normal", b"public".to_vec()))
        .unwrap();
    let mut sensitive_request = request("sensitive", b"private".to_vec());
    sensitive_request.asserted_sensitivity = Sensitivity::Sensitive;
    let sensitive = store.ingest_artifact(sensitive_request).unwrap();
    let unknown_digest = "a".repeat(64);
    let normal_object = "evidence-object-normal".to_string();
    let sensitive_object = "evidence-object-sensitive".to_string();

    // "never-written" has no registry row; `unknown_digest` has no artifact.
    let candidates: Vec<(String, Option<String>)> = vec![
        (normal_object.clone(), Some(normal.digest.clone())),
        (sensitive_object.clone(), Some(sensitive.digest.clone())),
        (normal_object.clone(), Some(sensitive.digest.clone())),
        ("never-written".to_string(), Some(normal.digest.clone())),
        (sensitive_object, None),
        ("never-written".to_string(), Some(unknown_digest.clone())),
        (normal_object, Some(unknown_digest)),
    ];

    for &destination in ArtifactDestination::ALL {
        let (snapshot, batch) = store.egress_candidates(&candidates, destination).unwrap();
        assert_eq!(batch.len(), candidates.len(), "{destination:?}");
        for (index, candidate) in candidates.iter().enumerate() {
            let (single_snapshot, single) = store
                .egress_candidates(std::slice::from_ref(candidate), destination)
                .unwrap();
            assert_eq!(single_snapshot, snapshot, "{destination:?} #{index}");
            assert_eq!(batch[index], single[0], "{destination:?} #{index}");
        }
        assert_eq!(batch[0].state, batch[2].state, "{destination:?}");
        assert!(batch[0].state.is_some(), "{destination:?}");
        assert_eq!(batch[0].artifact, batch[3].artifact, "{destination:?}");
        assert_eq!(batch[1].artifact, batch[2].artifact, "{destination:?}");
        assert_eq!(batch[5].artifact, batch[6].artifact, "{destination:?}");
        assert_ne!(batch[0].artifact, batch[1].artifact, "{destination:?}");
        assert!(batch[3].state.is_none(), "{destination:?}");
        assert!(batch[4].artifact.is_none(), "{destination:?}");
    }
}

#[test]
fn commit_failure_cleans_reference_and_errors_never_leak_payload() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    let payload = format!("payload {SECRET}").into_bytes();
    let error = store
        .ingest_artifact_with_fault_for_test(
            request("fault", payload),
            ArtifactIngestFault::AfterEvents,
        )
        .unwrap_err();

    assert_eq!(error.kind(), ArtifactErrorKind::ReferenceCommit);
    assert!(!error.to_string().contains(SECRET));
    assert!(!format!("{error:?}").contains(SECRET));
    assert!(
        !tree_bytes(root.path())
            .windows(SECRET.len())
            .any(|window| window == SECRET.as_bytes()),
        "a failed commit left the raw payload on disk"
    );
    let connection = Connection::open(root.path().join("kernel.sqlite")).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM evidence_meta WHERE evidence_id='evidence-fault'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM artifact_ingestion_reservations",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn failed_repopulation_commit_keeps_invalidated_retained_object_bytes() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    let payload = b"retained dedup bytes".to_vec();
    let existing = store
        .ingest_artifact(request("dedup-existing", payload.clone()))
        .unwrap();
    invalidate_evidence(&store, root.path(), &existing.evidence_id);
    let connection = Connection::open(root.path().join("kernel.sqlite")).unwrap();
    let commit_seq: i64 = connection
        .query_row(
            "SELECT created_commit_seq FROM evidence_meta WHERE evidence_id=?1",
            [&existing.evidence_id],
            |row| row.get(0),
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO capture_pins(
                 capture_pin_id,pin_kind,owner_id,commit_seq,lease_epoch,writer_epoch,
                 created_at,expires_at
             ) VALUES ('retained-pin','backup','test',?1,?2,?2,1,1000)",
            params![commit_seq, i64::try_from(store.lease_epoch()).unwrap()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO capture_pin_refs(capture_pin_id,evidence_id,expires_at)
             VALUES ('retained-pin',?1,1000)",
            [&existing.evidence_id],
        )
        .unwrap();
    drop(connection);
    let object_path = artifact_path(root.path(), &existing.digest);
    fs::remove_file(&object_path).unwrap();
    assert!(!object_path.exists());

    let error = store
        .ingest_artifact_with_fault_for_test(
            request("dedup-failed", payload),
            ArtifactIngestFault::AfterEvents,
        )
        .unwrap_err();
    assert_eq!(error.kind(), ArtifactErrorKind::ReferenceCommit);
    assert!(object_path.is_file());
}

#[test]
fn a_failed_ingest_removes_its_bytes_before_another_writer_can_take_over() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    let fence_before: i64 = Connection::open(root.path().join("kernel.sqlite"))
        .unwrap()
        .query_row(
            "SELECT writer_epoch FROM writer_fence WHERE id=0",
            [],
            |row| row.get(0),
        )
        .unwrap();

    // The ingest publishes new bytes and then fails. While the failed reference
    // is cleaned up, another connection tries to raise the fence between the
    // fence check and the unlink. The cleanup holds the write lock across both,
    // so the attempt finds the database busy and the fence is unchanged.
    let error = store
        .ingest_artifact_with_fault_for_test(
            request("taken-over", b"taken over".to_vec()),
            ArtifactIngestFault::TakeoverBeforeCleanupUnlink,
        )
        .unwrap_err();
    assert_eq!(error.kind(), ArtifactErrorKind::IngestionFailClosed);
    let fence_after: i64 = Connection::open(root.path().join("kernel.sqlite"))
        .unwrap()
        .query_row(
            "SELECT writer_epoch FROM writer_fence WHERE id=0",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        fence_after, fence_before,
        "another writer raised the fence between the cleanup's fence check and its unlink"
    );
    let digest = format!("{:x}", Sha256::digest(b"taken over"));
    assert!(
        !root
            .path()
            .join("artifacts/objects")
            .join(&digest[..2])
            .join(&digest[2..])
            .exists(),
        "the failed ingest left its bytes behind"
    );
}

#[test]
fn directory_sync_failure_latches_ingestion_but_keeps_reads_available() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    let healthy = store
        .ingest_artifact(request("before-fault", b"healthy".to_vec()))
        .unwrap();

    let error = store
        .ingest_artifact_with_fault_for_test(
            request("fsync-fault", b"faulted".to_vec()),
            ArtifactIngestFault::AfterDirectorySync,
        )
        .unwrap_err();
    assert_eq!(error.kind(), ArtifactErrorKind::IngestionFailClosed);
    assert_eq!(
        store
            .ingest_artifact(request("after-fault", b"blocked".to_vec()))
            .unwrap_err()
            .kind(),
        ArtifactErrorKind::IngestionFailClosed
    );
    assert_eq!(store.read_artifact(&healthy).unwrap(), b"healthy");

    let connection = Connection::open(root.path().join("kernel.sqlite")).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM evidence_meta WHERE evidence_id='evidence-fsync-fault'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn reopen_reclaims_stale_temps() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    drop(store);
    let stale = root.path().join("artifacts/tmp/.stale.tmp");
    fs::write(&stale, b"stale").unwrap();
    fs::set_permissions(&stale, fs::Permissions::from_mode(0o600)).unwrap();

    let _store = KernelStore::open(root.path()).unwrap();
    assert!(!stale.exists());
}

fn staged_entries(root: &std::path::Path) -> usize {
    fs::read_dir(root.join("artifacts/tmp")).unwrap().count()
}

fn published_objects(root: &std::path::Path) -> Vec<String> {
    let mut names = Vec::new();
    let mut pending = vec![root.join("artifacts/objects")];
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                pending.push(entry.path());
            } else {
                names.push(entry.file_name().into_string().unwrap());
            }
        }
    }
    names.sort();
    names
}

fn reservation_count(root: &std::path::Path) -> i64 {
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
fn repeated_fence_loss_stages_no_surviving_payload() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    store.invalidate_writer_fence_for_test().unwrap();

    for attempt in 0..3 {
        let error = store
            .ingest_artifact(request(&format!("fence-{attempt}"), vec![b'p'; 4 * 1024]))
            .unwrap_err();
        assert_eq!(error.kind(), ArtifactErrorKind::ReferenceCommit);
    }

    assert_eq!(staged_entries(root.path()), 0);
    assert_eq!(published_objects(root.path()), Vec::<String>::new());
}

#[test]
fn replayed_intent_returns_the_committed_reference() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    let payload = b"replayed payload".to_vec();

    let first = store
        .ingest_artifact(request("replay", payload.clone()))
        .unwrap();
    let second = store.ingest_artifact(request("replay", payload)).unwrap();

    assert_eq!(second.digest, first.digest);
    assert_eq!(second.evidence_id, first.evidence_id);
    assert_eq!(
        store.read_artifact(&second).unwrap(),
        b"replayed payload".to_vec()
    );
    assert_eq!(published_objects(root.path()).len(), 1);
    assert_eq!(staged_entries(root.path()), 0);
    assert_eq!(reservation_count(root.path()), 0);
}

#[test]
fn replayed_intent_over_different_bytes_publishes_no_unreferenced_object() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    let pinned = format!("{:x}", Sha256::digest(b"pinned-request"));

    let mut first = request("replay-mismatch", b"committed bytes".to_vec());
    first.intent.request_digest = pinned.clone();
    let handle = store.ingest_artifact(first).unwrap();
    let published = published_objects(root.path());

    let mut second = request("replay-mismatch", b"divergent bytes".to_vec());
    second.intent.request_digest = pinned;
    second.evidence_id = "evidence-replay-divergent".to_string();
    second.object_id = "evidence-object-replay-divergent".to_string();
    let error = store.ingest_artifact(second).unwrap_err();

    assert_eq!(error.kind(), ArtifactErrorKind::InvalidInput);
    assert_eq!(published_objects(root.path()), published);
    assert_eq!(staged_entries(root.path()), 0);
    assert_eq!(reservation_count(root.path()), 0);
    assert_eq!(
        store.read_artifact(&handle).unwrap(),
        b"committed bytes".to_vec()
    );
}

#[test]
fn invalidated_classification_still_restricts_re_admission_of_identical_bytes() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    let payload = b"classified by assertion".to_vec();

    let mut asserted = request("classified", payload.clone());
    asserted.asserted_sensitivity = Sensitivity::Secret;
    let first = store.ingest_artifact(asserted).unwrap();
    invalidate_evidence(&store, root.path(), &first.evidence_id);

    let mut relaxed = request("reclassified", payload);
    relaxed.asserted_sensitivity = Sensitivity::Normal;
    let second = store.ingest_artifact(relaxed).unwrap();

    assert_eq!(second.digest, first.digest);
    assert_eq!(
        store
            .artifact_eligibility(&second, ArtifactDestination::Local)
            .unwrap(),
        ArtifactEligibility::Denied(EligibilityDeniedReason::Secret)
    );
}

#[test]
fn malformed_intent_digest_is_rejected_before_staging() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);

    let mut malformed = request("malformed-intent", b"payload".to_vec());
    malformed.intent.request_digest = "not-a-digest".to_string();
    let error = store.ingest_artifact(malformed).unwrap_err();

    assert_eq!(error.kind(), ArtifactErrorKind::InvalidInput);
    assert_eq!(staged_entries(root.path()), 0);
    assert_eq!(published_objects(root.path()), Vec::<String>::new());
    assert_eq!(reservation_count(root.path()), 0);
}

#[test]
fn uninspectable_payload_never_persists_a_recognized_secret() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    let mut payload = vec![0xff, 0xfe];
    payload.extend_from_slice(format!("prefix {SECRET} suffix").as_bytes());

    let error = store
        .ingest_artifact(request("binary-secret", payload))
        .unwrap_err();

    assert_eq!(error.kind(), ArtifactErrorKind::UnredactableSecret);
    assert!(!format!("{error}").contains(SECRET));
    assert!(
        !tree_bytes(root.path())
            .windows(SECRET.len())
            .any(|window| window == SECRET.as_bytes())
    );
    assert_eq!(staged_entries(root.path()), 0);
    assert_eq!(published_objects(root.path()), Vec::<String>::new());
}

fn plant_shard_entry(root: &std::path::Path, digest: &str, plant: impl FnOnce(&std::path::Path)) {
    let shard = root.join("artifacts/objects").join(&digest[..2]);
    fs::create_dir_all(&shard).unwrap();
    fs::set_permissions(&shard, fs::Permissions::from_mode(0o700)).unwrap();
    plant(&shard.join(&digest[2..]));
}

#[test]
fn an_unverifiable_object_at_the_digest_path_is_neither_admitted_nor_replaced() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);

    // Each planted entry passes the publish step as "already exists" and then
    // fails verification; the ingest must neither reference it nor remove it.
    for (key, plant, expected) in [
        (
            "symlink",
            (|root: &std::path::Path, digest: &str, payload: &[u8]| {
                let outside = root.join("outside-the-cas");
                fs::write(&outside, payload).unwrap();
                plant_shard_entry(root, digest, |object| {
                    std::os::unix::fs::symlink(&outside, object).unwrap();
                });
            }) as fn(&std::path::Path, &str, &[u8]),
            ArtifactErrorKind::MissingObject,
        ),
        (
            "hardlink",
            |root: &std::path::Path, digest: &str, payload: &[u8]| {
                let outside = root.join("outside-alias");
                fs::write(&outside, payload).unwrap();
                fs::set_permissions(&outside, fs::Permissions::from_mode(0o600)).unwrap();
                plant_shard_entry(root, digest, |object| {
                    fs::hard_link(&outside, object).unwrap();
                });
            },
            ArtifactErrorKind::MissingObject,
        ),
        (
            "fifo",
            |root: &std::path::Path, digest: &str, _payload: &[u8]| {
                plant_shard_entry(root, digest, make_fifo);
            },
            ArtifactErrorKind::MissingObject,
        ),
        (
            "oversize-verify",
            |root: &std::path::Path, digest: &str, _payload: &[u8]| {
                plant_shard_entry(root, digest, |object| {
                    // An ingest never stores more than the payload cap, so an
                    // oversized object at this digest is store corruption, not
                    // a collision.
                    fs::write(object, vec![b'z'; 64 * MIB + 1]).unwrap();
                    fs::set_permissions(object, fs::Permissions::from_mode(0o600)).unwrap();
                });
            },
            ArtifactErrorKind::CorruptObject,
        ),
    ] {
        let payload = format!("{key} bait").into_bytes();
        let digest = format!("{:x}", Sha256::digest(&payload));
        plant(root.path(), &digest, &payload);

        let error = store.ingest_artifact(request(key, payload)).unwrap_err();

        assert_eq!(error.kind(), expected, "{key}");
        let references: i64 = Connection::open(root.path().join("kernel.sqlite"))
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM evidence_meta WHERE artifact_digest=?1",
                [&digest],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(references, 0, "{key}");
        assert_eq!(reservation_count(root.path()), 0, "{key}");
        assert_eq!(staged_entries(root.path()), 0, "{key}");
        assert!(
            fs::symlink_metadata(artifact_path(root.path(), &digest)).is_ok(),
            "{key}: a pre-existing entry is never replaced"
        );
    }
}

#[test]
fn secret_in_an_identity_or_provenance_field_is_refused_rather_than_redacted() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);

    for mutate in [
        (|r: &mut ArtifactIngestRequest| r.evidence_id = format!("evidence key={SECRET}"))
            as fn(&mut ArtifactIngestRequest),
        |r: &mut ArtifactIngestRequest| r.object_id = format!("object key={SECRET}"),
        |r: &mut ArtifactIngestRequest| r.object_kind = format!("evidence key={SECRET}"),
        |r: &mut ArtifactIngestRequest| r.source_kind = format!("repository key={SECRET}"),
        |r: &mut ArtifactIngestRequest| r.source_id = format!("src key={SECRET}"),
        |r: &mut ArtifactIngestRequest| {
            r.provenance = Some(RepositoryProvenance {
                repository_id: format!("repo key={SECRET}"),
                revision: "abc123".to_string(),
            })
        },
        |r: &mut ArtifactIngestRequest| {
            r.provenance = Some(RepositoryProvenance {
                repository_id: "repo".to_string(),
                revision: format!("abc key={SECRET}"),
            })
        },
    ] {
        let mut tainted = request("identity-secret", b"identity payload".to_vec());
        mutate(&mut tainted);
        let error = store.ingest_artifact(tainted).unwrap_err();
        assert_eq!(error.kind(), ArtifactErrorKind::InvalidInput);
    }

    assert!(
        !tree_bytes(root.path())
            .windows(SECRET.len())
            .any(|window| window == SECRET.as_bytes())
    );
    assert_eq!(reservation_count(root.path()), 0);
    assert_eq!(staged_entries(root.path()), 0);
    assert_eq!(published_objects(root.path()), Vec::<String>::new());
}

#[test]
fn secret_in_artifact_metadata_raises_the_classification_and_reaches_the_ledger() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);

    let mut tainted = request("metadata-secret", b"clean payload".to_vec());
    tainted.media_type = format!("text/plain; key={SECRET}");
    tainted.retention_class = format!("canonical token={SECRET}");
    let handle = store.ingest_artifact(tainted).unwrap();

    assert_eq!(
        store
            .artifact_eligibility(&handle, ArtifactDestination::Local)
            .unwrap(),
        ArtifactEligibility::Denied(EligibilityDeniedReason::Secret)
    );
    assert!(
        !tree_bytes(root.path())
            .windows(SECRET.len())
            .any(|window| window == SECRET.as_bytes())
    );

    let connection = Connection::open(root.path().join("kernel.sqlite")).unwrap();
    let mut statement = connection
        .prepare(
            "SELECT field_name FROM durable_text_redactions
             WHERE owner_kind='evidence' AND owner_id=?1 ORDER BY field_name",
        )
        .unwrap();
    let fields = statement
        .query_map([&handle.evidence_id], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|row| row.unwrap())
        .collect::<Vec<_>>();
    for expected in ["media_type", "retention_class"] {
        assert!(
            fields.iter().any(|field| field == expected),
            "evidence ledger is missing {expected}: {fields:?}"
        );
    }
}

#[test]
fn oversized_text_fields_are_rejected_before_staging() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);

    for mutate in [
        (|r: &mut ArtifactIngestRequest| r.intent.cause = "key=a ".repeat(4000))
            as fn(&mut ArtifactIngestRequest),
        |r: &mut ArtifactIngestRequest| r.media_type = "key=a ".repeat(4000),
        |r: &mut ArtifactIngestRequest| r.evidence_id = "key=a ".repeat(4000),
        |r: &mut ArtifactIngestRequest| r.object_id = "key=a ".repeat(4000),
        |r: &mut ArtifactIngestRequest| r.object_kind = "key=a ".repeat(4000),
        |r: &mut ArtifactIngestRequest| r.domain_id = "key=a ".repeat(4000),
        |r: &mut ArtifactIngestRequest| r.source_kind = "key=a ".repeat(4000),
        |r: &mut ArtifactIngestRequest| r.source_id = "key=a ".repeat(4000),
        |r: &mut ArtifactIngestRequest| {
            r.provenance = Some(RepositoryProvenance {
                repository_id: "r".repeat(4000),
                revision: "abc123".to_string(),
            })
        },
        |r: &mut ArtifactIngestRequest| {
            r.provenance = Some(RepositoryProvenance {
                repository_id: "repo".to_string(),
                revision: "v".repeat(4000),
            })
        },
    ] {
        let mut flooded = request("text-flood", b"small payload".to_vec());
        mutate(&mut flooded);
        let error = store.ingest_artifact(flooded).unwrap_err();

        assert_eq!(error.kind(), ArtifactErrorKind::TextFieldTooLong);
    }

    assert_eq!(staged_entries(root.path()), 0);
    assert_eq!(published_objects(root.path()), Vec::<String>::new());
    assert_eq!(reservation_count(root.path()), 0);
}

#[test]
fn detection_dense_payloads_are_rejected_before_staging_on_both_scan_paths() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);

    // The short flood takes the direct redaction path; the long one exceeds
    // the direct limit and takes the windowed scan instead.
    let mut windowed = String::new();
    while windowed.len() < 2 * context_core::redaction::MAX_REDACTABLE_BYTES {
        windowed.push_str("password=hunter-two-");
        windowed.push_str(&windowed.len().to_string());
        windowed.push('\n');
    }
    for (key, payload) in [
        ("detection-flood", "key=a ".repeat(5000).into_bytes()),
        ("windowed-detection-flood", windowed.into_bytes()),
    ] {
        let error = store.ingest_artifact(request(key, payload)).unwrap_err();
        assert_eq!(error.kind(), ArtifactErrorKind::DetectionLimit, "{key}");
    }

    assert_eq!(staged_entries(root.path()), 0);
    assert_eq!(published_objects(root.path()), Vec::<String>::new());
    assert_eq!(reservation_count(root.path()), 0);
}

#[test]
fn usage_totaling_skips_symlinks_and_nested_directories_under_objects() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open_with_artifact_cap_for_test(root.path(), 4096).unwrap();
    seed_domain(&store);
    let first = store
        .ingest_artifact(request("usage-base", b"usage base".to_vec()))
        .unwrap();

    // The external symlink target and the nested file exceed the cap;
    // counting either would refuse the next ingest.
    let objects = root.path().join("artifacts/objects");
    std::os::unix::fs::symlink(&objects, objects.join("zz")).unwrap();
    let outside = root.path().join("outside-bulk");
    fs::write(&outside, vec![b'q'; 8192]).unwrap();
    std::os::unix::fs::symlink(&outside, objects.join("zy")).unwrap();
    let nested = objects.join("zx/nested");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("bulk"), vec![b'q'; 8192]).unwrap();

    let second = store
        .ingest_artifact(request("usage-after", b"usage after".to_vec()))
        .unwrap();

    assert_ne!(first.digest, second.digest);
    assert_eq!(
        store.read_artifact(&second).unwrap(),
        b"usage after".to_vec()
    );
}

#[test]
fn swapped_objects_directory_is_not_followed_when_reading() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    let handle = store
        .ingest_artifact(request("swap-read", b"swap read bytes".to_vec()))
        .unwrap();
    assert_eq!(store.read_artifact(&handle).unwrap(), b"swap read bytes");

    let artifacts = root.path().join("artifacts");
    let objects = artifacts.join("objects");
    let shard = &handle.digest[..2];
    let name = &handle.digest[2..];

    let foreign = root.path().join("foreign-objects");
    fs::create_dir(&foreign).unwrap();
    fs::set_permissions(&foreign, fs::Permissions::from_mode(0o700)).unwrap();
    let foreign_shard = foreign.join(shard);
    fs::create_dir(&foreign_shard).unwrap();
    fs::set_permissions(&foreign_shard, fs::Permissions::from_mode(0o700)).unwrap();
    // The foreign copy differs from the stored bytes so a read that followed
    // the swapped pathname would be observable.
    let foreign_object = foreign_shard.join(name);
    fs::write(&foreign_object, b"foreign bytes at the same digest").unwrap();
    fs::set_permissions(&foreign_object, fs::Permissions::from_mode(0o600)).unwrap();

    fs::rename(&objects, artifacts.join("objects-real")).unwrap();
    std::os::unix::fs::symlink(&foreign, &objects).unwrap();

    // The store holds the `objects` descriptor it opened, so the read resolves
    // below the original tree wherever its pathname now points.
    assert_eq!(store.read_artifact(&handle).unwrap(), b"swap read bytes");
}

#[test]
fn a_replaced_objects_directory_does_not_receive_an_ingest() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    let artifacts = root.path().join("artifacts");
    let objects = artifacts.join("objects");
    let original = artifacts.join("objects-real");

    // A same-UID process renames `objects` away and puts another owner-only
    // directory in its place. The replacement passes every ownership check a
    // fresh open would apply.
    fs::rename(&objects, &original).unwrap();
    fs::create_dir(&objects).unwrap();
    fs::set_permissions(&objects, fs::Permissions::from_mode(0o700)).unwrap();

    let handle = store
        .ingest_artifact(request(
            "diverted",
            b"bytes the reference must reach".to_vec(),
        ))
        .unwrap();
    let shard = &handle.digest[..2];
    let name = &handle.digest[2..];
    assert!(
        original.join(shard).join(name).is_file(),
        "the ingest left the tree the store opened"
    );
    assert!(
        !objects.join(shard).exists(),
        "the ingest published into the replacement directory"
    );

    // Swapping the original back leaves the committed reference with its bytes.
    fs::remove_dir(&objects).unwrap();
    fs::rename(&original, &objects).unwrap();
    assert_eq!(
        store.read_artifact(&handle).unwrap(),
        b"bytes the reference must reach"
    );
    drop(store);
    let reopened = KernelStore::open(root.path()).unwrap();
    assert_eq!(
        reopened.read_artifact(&handle).unwrap(),
        b"bytes the reference must reach"
    );
}

#[test]
fn a_secret_longer_than_the_match_bound_rejects_the_payload_instead_of_storing_it() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);

    // A PEM body past the scanner's match bound cannot be described as a
    // finding; the scan reports the limit and the payload is refused rather
    // than stored in cleartext with no detections.
    let body_line = "MIIEpAIBAAKCAQEA7bq2k0v9xR3sY1nQ4dJ6fH8zL2mW5cP0uT9eG7iK3oB1aV\n";
    let mut pem = String::from("-----BEGIN RSA PRIVATE KEY-----\n");
    while pem.len() < secret_scanner::MAX_MATCH_BYTES + body_line.len() {
        pem.push_str(body_line);
    }
    pem.push_str("-----END RSA PRIVATE KEY-----");
    assert!(pem.len() > secret_scanner::MAX_MATCH_BYTES);

    for (key, payload) in [
        ("text", format!("prefix\n{pem}\nsuffix\n").into_bytes()),
        ("binary", {
            let mut bytes = format!("\u{fffd}{pem}\n").into_bytes();
            bytes[0] = 0xff;
            bytes
        }),
    ] {
        let error = store.ingest_artifact(request(key, payload)).unwrap_err();
        assert_eq!(error.kind(), ArtifactErrorKind::UnredactableSecret, "{key}");
    }
    assert_eq!(live_reservations(root.path()), 0);
    assert!(
        !tree_bytes(root.path())
            .windows(body_line.len())
            .any(|window| window == body_line.as_bytes())
    );
}

#[test]
fn an_open_store_keeps_publishing_into_the_tree_it_opened() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("store");
    let store = KernelStore::open(&root).unwrap();
    seed_domain(&store);

    // A same-UID process moves the store root aside and puts another owner-only
    // tree at the same pathname after the store is open.
    let moved = parent.path().join("moved-store");
    fs::rename(&root, &moved).unwrap();
    for directory in [
        root.clone(),
        root.join("artifacts"),
        root.join("artifacts/objects"),
        root.join("artifacts/tmp"),
    ] {
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    }

    let handle = store
        .ingest_artifact(request("anchored", b"anchored payload".to_vec()))
        .unwrap();

    assert_eq!(
        published_objects(&root),
        Vec::<String>::new(),
        "an ingest published bytes into a directory swapped in after the open"
    );
    assert_eq!(
        published_objects(&moved),
        vec![handle.digest[2..].to_string()]
    );
    // The evidence row committed to the database the store opened, which moved
    // with the tree that received the bytes.
    let references: i64 = Connection::open(moved.join("kernel.sqlite"))
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM evidence_meta WHERE artifact_digest=?1",
            [&handle.digest],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(references, 1);
}

#[test]
fn a_replayed_stricter_classification_reaches_the_served_surface() {
    use kernel::{AdmissionEvent, AdmissionRequest, EventKind, SourceClass, Surface, TaintClass};

    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    let payload = b"served then tightened".to_vec();
    let handle = store
        .ingest_artifact(request("served", payload.clone()))
        .unwrap();
    // The evidence object earns labeled standing while it is classified normal.
    store
        .commit(intent("admit-served", b"admit"), |envelope| {
            envelope.record_admission(AdmissionRequest {
                candidate_id: None,
                subject_object_id: Some("evidence-object-served".to_string()),
                source_class: Some(SourceClass::TrustedLocalCode),
                taint_class: Some(TaintClass::CurrentCode),
                event: AdmissionEvent {
                    kind: EventKind::Other,
                    trigger_object_id: None,
                    approval_object_id: None,
                    evidence_id: Some(handle.evidence_id.clone()),
                    reason: "fixture".to_string(),
                },
            })?;
            Ok(String::new())
        })
        .unwrap();
    let tip = store.tip().unwrap();
    let served_before = store.visible_as_of(Surface::ExplicitSearch, tip).unwrap();
    assert!(
        served_before
            .rows
            .iter()
            .any(|row| row.object.object_id == "evidence-object-served"),
        "the fixture must serve before the replay: {served_before:?}"
    );

    // An idempotent replay tightens the artifact to secret. The tightening is a
    // change of its own, so it commits one log row; repeating it commits none.
    let mut stricter = request("served", payload.clone());
    stricter.asserted_sensitivity = Sensitivity::Secret;
    let replayed = store.ingest_artifact(stricter.clone()).unwrap();
    assert_eq!(replayed.digest, handle.digest);
    let tightened_tip = store.tip().unwrap();
    assert_eq!(tightened_tip, tip + 1, "a tightening replay is one commit");
    store.ingest_artifact(stricter).unwrap();
    assert_eq!(
        store.tip().unwrap(),
        tightened_tip,
        "a repeated tightening commits nothing"
    );
    let mut same = request("served", payload);
    same.asserted_sensitivity = Sensitivity::Normal;
    store.ingest_artifact(same).unwrap();
    assert_eq!(
        store.tip().unwrap(),
        tightened_tip,
        "a looser replay commits nothing"
    );
    assert_ne!(
        store
            .artifact_eligibility(&handle, ArtifactDestination::Remote)
            .unwrap(),
        ArtifactEligibility::Allowed
    );

    let served_after = store
        .visible_as_of(Surface::ExplicitSearch, tightened_tip)
        .unwrap();
    assert!(
        !served_after
            .rows
            .iter()
            .any(|row| row.object.object_id == "evidence-object-served"),
        "a secret artifact stayed served after the replay"
    );
}

#[test]
fn intents_that_split_the_same_text_differently_derive_distinct_classification_keys() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    // `a` + `b#c` and `a#b` + `c` concatenate to the same text. Each names its
    // own artifact, and each is later replayed with the same tightening, so the
    // two derived classification commits must not share a key.
    let split = |producer: &str, key: &str, payload: &[u8]| {
        let mut request = request(&format!("{producer}-{key}"), payload.to_vec());
        request.intent.producer = producer.to_string();
        request.intent.operation_key = key.to_string();
        request
    };
    let first = split("a", "b#c", b"first artifact");
    let second = split("a#b", "c", b"second artifact");
    let first_handle = store.ingest_artifact(first.clone()).unwrap();
    let second_handle = store.ingest_artifact(second.clone()).unwrap();
    assert_ne!(first_handle.digest, second_handle.digest);

    let mut first_secret = first;
    first_secret.asserted_sensitivity = Sensitivity::Secret;
    store.ingest_artifact(first_secret).unwrap();
    let mut second_secret = second;
    second_secret.asserted_sensitivity = Sensitivity::Secret;
    store
        .ingest_artifact(second_secret)
        .expect("the second tightening must not collide with the first receipt");
    for handle in [&first_handle, &second_handle] {
        assert_ne!(
            store
                .artifact_eligibility(handle, ArtifactDestination::Remote)
                .unwrap(),
            ArtifactEligibility::Allowed,
            "{} was not tightened",
            handle.digest
        );
    }
}

#[test]
fn a_classification_tightening_reaches_the_outbox_for_every_row_it_changes() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    let payload = b"one payload, two references".to_vec();
    let first = store
        .ingest_artifact(request("first", payload.clone()))
        .unwrap();
    // Everything published so far is acknowledged; what follows is exactly what
    // a publisher would see after the tightening.
    let published = store.pending_outbox(64).unwrap();
    let last = published.last().unwrap();
    assert!(last.commit_boundary);
    store
        .mark_outbox_published_through(last.outbox_position, 1)
        .unwrap();

    // A replay of the first reference asserting `Secret` tightens the digest.
    let mut stricter = request("first", payload.clone());
    stricter.asserted_sensitivity = Sensitivity::Secret;
    assert_eq!(
        store.ingest_artifact(stricter).unwrap().digest,
        first.digest
    );

    let pending = store.pending_outbox(64).unwrap();
    let classify: Vec<_> = pending
        .iter()
        .filter(|entry| {
            serde_json::from_slice::<serde_json::Value>(&entry.payload).unwrap()["change_kind"]
                == "classify"
        })
        .collect();
    assert_eq!(classify.len(), 1, "{pending:?}");
    assert_eq!(classify[0].object_id, "evidence-object-first");
    assert_eq!(classify[0].sensitivity, Sensitivity::Secret);
    assert!(classify[0].commit_boundary);
    let audit = serde_json::from_slice::<serde_json::Value>(&classify[0].payload).unwrap();
    assert_eq!(audit["audit"]["sensitivity"], "secret");
    assert_eq!(audit["audit"]["artifact_digest"], first.digest.as_str());
    store
        .mark_outbox_published_through(classify[0].outbox_position, 2)
        .unwrap();

    // A second reference to the same bytes asserting `LocalOnly` tightens the
    // first reference's egress as well; both the new insert and the sibling's
    // reclassification are published in that commit.
    let mut second = request("second", payload);
    second.provider_egress = ProviderEgress::LocalOnly;
    store.ingest_artifact(second).unwrap();
    let pending = store.pending_outbox(64).unwrap();
    let kinds: Vec<(String, String)> = pending
        .iter()
        .map(|entry| {
            let payload = serde_json::from_slice::<serde_json::Value>(&entry.payload).unwrap();
            (
                entry.object_id.clone(),
                payload["change_kind"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("evidence-object-first".to_string(), "classify".to_string()),
            ("evidence-object-second".to_string(), "insert".to_string()),
        ]
    );
    let egress: Vec<String> = Connection::open(root.path().join("kernel.sqlite"))
        .unwrap()
        .prepare("SELECT provider_egress_class FROM evidence_meta ORDER BY evidence_id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(egress, vec!["local_only", "local_only"]);
}

#[test]
fn a_tightening_asserted_while_no_reference_is_live_governs_the_next_one() {
    use kernel::{ArtifactDeletionIdentity, ArtifactDeletionKind, ArtifactDeletionRequest};

    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    let payload = b"classified after deletion".to_vec();
    let handle = store
        .ingest_artifact(request("gone", payload.clone()))
        .unwrap();
    store
        .delete_artifact(ArtifactDeletionRequest {
            intent: intent("delete-gone", b"delete"),
            identity: ArtifactDeletionIdentity::Digest(handle.digest.clone()),
            kind: ArtifactDeletionKind::Delete,
            operator_id: None,
            target_locator: None,
            reason: None,
            deleted_at: 42,
        })
        .unwrap();
    let tip = store.tip().unwrap();

    // Only a deleted row remains for the digest. A replay tightening it has no
    // live object to publish a change for, but the class is still a fact about
    // the bytes and is committed.
    let mut stricter = request("gone", payload.clone());
    stricter.asserted_sensitivity = Sensitivity::Secret;
    stricter.provider_egress = ProviderEgress::LocalOnly;
    assert_eq!(
        store.ingest_artifact(stricter).unwrap().digest,
        handle.digest
    );
    assert_eq!(store.tip().unwrap(), tip + 1);
    let stored: (String, String) = Connection::open(root.path().join("kernel.sqlite"))
        .unwrap()
        .query_row(
            "SELECT sensitivity_class,provider_egress_class FROM evidence_meta
             WHERE evidence_id='evidence-gone'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(stored, ("secret".to_string(), "local_only".to_string()));

    // The next reference to the same bytes, asserting nothing stricter than
    // normal, inherits the tightened class rather than reopening remote egress.
    let next = store.ingest_artifact(request("back", payload)).unwrap();
    assert_eq!(next.digest, handle.digest);
    assert_ne!(
        store
            .artifact_eligibility(&next, ArtifactDestination::Remote)
            .unwrap(),
        ArtifactEligibility::Allowed,
        "a deleted-then-tightened digest served remotely again"
    );
}

#[test]
fn a_caller_cannot_reach_the_classification_receipt_namespace() {
    use kernel::{ArtifactDeletionIdentity, ArtifactDeletionKind, ArtifactDeletionRequest};

    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    let payload = b"namespace payload".to_vec();
    let first = request("first", payload.clone());
    let handle = store.ingest_artifact(first.clone()).unwrap();

    // Every caller-facing entry refuses an intent under the store's reserved
    // producer, so no caller can plant a receipt where the tightening looks.
    let mut reserved = first.intent.clone();
    reserved.producer = format!("{}classification", CommitIntent::RESERVED_PRODUCER_PREFIX);
    assert_eq!(
        store
            .commit(reserved.clone(), |_| Ok("forged".to_string()))
            .unwrap_err(),
        kernel::KernelError::InvalidInput
    );
    let mut reserved_ingest = request("reserved", b"other".to_vec());
    reserved_ingest.intent.producer = reserved.producer.clone();
    assert_eq!(
        store.ingest_artifact(reserved_ingest).unwrap_err().kind(),
        ArtifactErrorKind::InvalidInput
    );
    assert_eq!(
        store
            .delete_artifact(ArtifactDeletionRequest {
                intent: reserved,
                identity: ArtifactDeletionIdentity::Digest(handle.digest.clone()),
                kind: ArtifactDeletionKind::Delete,
                operator_id: None,
                target_locator: None,
                reason: None,
                deleted_at: 1,
            })
            .unwrap_err()
            .kind(),
        ArtifactErrorKind::InvalidInput
    );

    // A caller writing the derived-looking key under its own producer, with the
    // same request digest and the exact result string the tightening records,
    // occupies nothing the tightening consults.
    let mut lookalike = first.intent.clone();
    lookalike.operation_key = format!(
        "{}#{}#classify:secret:remote_allowed",
        first.intent.producer, first.intent.operation_key
    );
    let expected_result = format!("classify:{}:secret:remote_allowed", handle.digest);
    store.commit(lookalike, |_| Ok(expected_result)).unwrap();

    let mut stricter = first;
    stricter.asserted_sensitivity = Sensitivity::Secret;
    store.ingest_artifact(stricter).unwrap();
    assert_ne!(
        store
            .artifact_eligibility(&handle, ArtifactDestination::Remote)
            .unwrap(),
        ArtifactEligibility::Allowed,
        "the tightening was replayed against a caller's receipt"
    );
}

#[test]
fn a_replaced_shard_directory_does_not_receive_an_ingest() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    seed_domain(&store);
    // The first ingest binds its shard to the store; a second payload in the
    // same shard is found by trying digests until one shares the first's prefix.
    let first_payload = b"shard-bound payload".to_vec();
    let first = store
        .ingest_artifact(request("shard-first", first_payload.clone()))
        .unwrap();
    let prefix = first.digest[..2].to_string();
    let second_payload = (0u32..)
        .map(|nonce| format!("same-shard payload {nonce}").into_bytes())
        .find(|payload| format!("{:x}", Sha256::digest(payload)).starts_with(&prefix))
        .unwrap();

    // A same-UID process renames the shard away and puts another owner-only
    // directory in its place; it passes every ownership check a fresh open
    // would apply.
    let objects = root.path().join("artifacts/objects");
    let shard = objects.join(&prefix);
    let moved = objects.join(format!("{prefix}-moved"));
    fs::rename(&shard, &moved).unwrap();
    fs::create_dir(&shard).unwrap();
    fs::set_permissions(&shard, fs::Permissions::from_mode(0o700)).unwrap();

    let second = store
        .ingest_artifact(request("shard-second", second_payload.clone()))
        .unwrap();
    assert_eq!(&second.digest[..2], prefix.as_str());
    assert!(
        moved.join(&second.digest[2..]).is_file(),
        "the ingest left the shard the store bound"
    );
    assert!(
        !shard.join(&second.digest[2..]).exists(),
        "the ingest published into the replacement shard"
    );
    // Reads resolve through the same held shard, so the committed reference
    // still has its bytes whatever the pathname now names.
    assert_eq!(store.read_artifact(&second).unwrap(), second_payload);
    assert_eq!(store.read_artifact(&first).unwrap(), first_payload);

    // Swapping the original back leaves both references intact for a fresh open.
    fs::remove_dir(&shard).unwrap();
    fs::rename(&moved, &shard).unwrap();
    drop(store);
    let reopened = KernelStore::open(root.path()).unwrap();
    assert_eq!(reopened.read_artifact(&second).unwrap(), second_payload);
}
