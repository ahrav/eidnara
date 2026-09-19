//! Real-store proofs for confined project-text inspection: captures as owned evidence with typed detail and hold charging, reuse without duplication, confinement and protected-location refusals, special files, caps, search bounds, remote refusal, and expiry that keeps independent support.

use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use daemon::memory_reviewer::broker::{
    EvidenceBroker, MAX_ISSUED_INSPECTIONS, MAX_OPERATIONS_PER_BATCH, QuestionTemplate,
    RefusalCode, RunBinding,
};
use daemon::memory_reviewer::project_text::{
    InspectionBinding, MAX_CAPTURE_BYTES, MAX_DEPTH, MAX_SCAN_BYTES, MAX_VISITED_ENTRIES,
    ProjectText, ProtectedLocations, SearchQuery,
};
use daemon::memory_reviewer::{Completeness, MAX_EXCERPT_BYTES};
use kernel::{
    ArtifactDestination, ArtifactHandle, ArtifactIngestRequest, CommitIntent, DomainSpec,
    KernelStore, MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS, MemoryReviewerHoldBinding,
    MemoryReviewerHoldKind, ProviderEgress, Sensitivity,
};
use sha2::{Digest, Sha256};

const DOMAIN: &str = "domain";
const PROJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HOUR_MS: i64 = 60 * 60 * 1_000;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .try_into()
        .unwrap()
}

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "memory_reviewer-project-text-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

fn incarnation(root: &Path) -> String {
    rusqlite::Connection::open_with_flags(
        root.join("kernel.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
    .query_row(
        "SELECT database_incarnation_id FROM kernel_format_marker",
        [],
        |row| row.get(0),
    )
    .unwrap()
}

struct Fixture {
    store_dir: tempfile::TempDir,
    project: tempfile::TempDir,
    store: KernelStore,
    now: i64,
}

impl Fixture {
    fn open() -> Self {
        let store_dir = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let store = KernelStore::open(store_dir.path()).unwrap();
        store
            .commit(intent("seed"), |envelope| {
                envelope.insert_domain(DomainSpec {
                    domain_id: DOMAIN.to_string(),
                    object_id: "domain-object".to_string(),
                    name: "fixture".to_string(),
                    source_kind: "fixture".to_string(),
                    source_id: DOMAIN.to_string(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                })?;
                Ok(String::new())
            })
            .unwrap();
        Self {
            store_dir,
            project,
            store,
            now: now_ms(),
        }
    }

    fn write(&self, relative: &str, bytes: &[u8]) -> PathBuf {
        let path = self.project.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn hold_binding(&self) -> MemoryReviewerHoldBinding {
        self.hold_binding_for(PROJECT)
    }

    fn hold_binding_for(&self, project: &str) -> MemoryReviewerHoldBinding {
        self.hold_binding_with(project, "job-1")
    }

    fn hold_binding_with(&self, project: &str, subject: &str) -> MemoryReviewerHoldBinding {
        MemoryReviewerHoldBinding {
            project_digest: project.to_string(),
            kernel_incarnation: incarnation(self.store_dir.path()),
            memstore_incarnation: "m".repeat(32),
            subject: subject.to_string(),
            generation: 1,
        }
    }

    /// A broker and inspection binding for the same job identity under another MemoryStore incarnation, as after a MemoryStore recreation the Kernel store survived.
    fn run_of_another_memstore(&self) -> (EvidenceBroker, InspectionBinding) {
        let mut hold = self.hold_binding();
        hold.memstore_incarnation = "n".repeat(32);
        let anchor = self
            .store
            .ingest_artifact(ArtifactIngestRequest {
                intent: intent("anchor-other-memstore"),
                payload: b"anchor other memstore".to_vec(),
                evidence_id: "evidence-anchor-other-memstore".to_string(),
                object_id: "evidence-object-anchor-other-memstore".to_string(),
                object_kind: "evidence".to_string(),
                domain_id: DOMAIN.to_string(),
                source_kind: "conversation".to_string(),
                source_id: "src/anchor".to_string(),
                source_revision: 1,
                media_type: "text/plain".to_string(),
                retention_class: "canonical".to_string(),
                retain_until: None,
                asserted_sensitivity: Sensitivity::Normal,
                provider_egress: ProviderEgress::RemoteAllowed,
                provenance: None,
            })
            .unwrap();
        let acquired = self
            .store
            .acquire_execution_hold(&hold, &[anchor.evidence_id], self.now + 2 * HOUR_MS)
            .unwrap();
        let broker = EvidenceBroker::new(
            RunBinding {
                hold: hold.clone(),
                hold_id: acquired.hold_id,
                destination: ArtifactDestination::Local,
            },
            QuestionTemplate::ExtractedFacts,
        )
        .unwrap();
        let mut binding = self.binding();
        binding.hold = hold;
        (broker, binding)
    }

    /// A broker with an execution hold over `anchor` for this project, disclosing to `destination`.
    fn broker(&self, destination: ArtifactDestination) -> EvidenceBroker {
        self.broker_for(PROJECT, destination)
    }

    /// A broker for a run of `project`, whose hold subject and generation are the fixture's.
    fn broker_for(&self, project: &str, destination: ArtifactDestination) -> EvidenceBroker {
        self.broker_with_references(project, destination, 1)
    }

    /// A broker for another run of the fixture's project: same project, another job subject.
    fn broker_of_run(&self, subject: &str) -> EvidenceBroker {
        let anchor = self
            .store
            .ingest_artifact(ArtifactIngestRequest {
                intent: intent(&format!("anchor-{subject}")),
                payload: format!("anchor {subject}").into_bytes(),
                evidence_id: format!("evidence-anchor-{subject}"),
                object_id: format!("evidence-object-anchor-{subject}"),
                object_kind: "evidence".to_string(),
                domain_id: DOMAIN.to_string(),
                source_kind: "conversation".to_string(),
                source_id: "src/anchor".to_string(),
                source_revision: 1,
                media_type: "text/plain".to_string(),
                retention_class: "canonical".to_string(),
                retain_until: None,
                asserted_sensitivity: Sensitivity::Normal,
                provider_egress: ProviderEgress::RemoteAllowed,
                provenance: None,
            })
            .unwrap();
        let binding = self.hold_binding_with(PROJECT, subject);
        let hold = self
            .store
            .acquire_execution_hold(&binding, &[anchor.evidence_id], self.now + 2 * HOUR_MS)
            .unwrap();
        EvidenceBroker::new(
            RunBinding {
                hold: binding,
                hold_id: hold.hold_id,
                destination: ArtifactDestination::Local,
            },
            QuestionTemplate::ExtractedFacts,
        )
        .unwrap()
    }

    /// A broker whose execution hold already carries `references` canonical references, so at most `MAX_MEMORY_REVIEWER_HOLD_REFERENCES - references` captures can still be pinned.
    fn broker_with_references(
        &self,
        project: &str,
        destination: ArtifactDestination,
        references: usize,
    ) -> EvidenceBroker {
        let held: Vec<String> = (0..references)
            .map(|index| {
                let key = format!("anchor-{index}-{destination:?}-{project}");
                self.store
                    .ingest_artifact(ArtifactIngestRequest {
                        intent: intent(&key),
                        payload: format!("anchor {index}").into_bytes(),
                        evidence_id: format!("evidence-{key}"),
                        object_id: format!("evidence-object-{key}"),
                        object_kind: "evidence".to_string(),
                        domain_id: DOMAIN.to_string(),
                        source_kind: "conversation".to_string(),
                        source_id: "src/anchor".to_string(),
                        source_revision: 1,
                        media_type: "text/plain".to_string(),
                        retention_class: "canonical".to_string(),
                        retain_until: None,
                        asserted_sensitivity: Sensitivity::Normal,
                        provider_egress: ProviderEgress::RemoteAllowed,
                        provenance: None,
                    })
                    .unwrap()
                    .evidence_id
            })
            .collect();
        let binding = self.hold_binding_for(project);
        let hold = self
            .store
            .acquire_execution_hold(&binding, &held, self.now + 2 * HOUR_MS)
            .unwrap();
        EvidenceBroker::new(
            RunBinding {
                hold: binding,
                hold_id: hold.hold_id,
                destination,
            },
            QuestionTemplate::ExtractedFacts,
        )
        .unwrap()
    }

    fn binding(&self) -> InspectionBinding {
        self.binding_for(PROJECT)
    }

    fn binding_for(&self, project: &str) -> InspectionBinding {
        InspectionBinding {
            hold: self.hold_binding_for(project),
            domain_id: DOMAIN.to_string(),
            scope_id: None,
            retain_until: self.now + HOUR_MS,
        }
    }

    fn text(&self, protected: &ProtectedLocations) -> ProjectText {
        ProjectText::open(self.project.path(), protected, self.binding()).unwrap()
    }

    /// The Kernel store's own root is always a protected location.
    fn protected(&self) -> ProtectedLocations {
        ProtectedLocations::new([self.store_dir.path().to_path_buf()]).unwrap()
    }

    fn hold_references(&self, hold_id: &str) -> i64 {
        rusqlite::Connection::open_with_flags(
            self.store_dir.path().join("kernel.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM capture_pin_refs WHERE capture_pin_id=?1 AND released_at IS NULL",
            [hold_id],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn capture_rows(&self) -> Vec<(String, String, Option<i64>, String, String)> {
        let connection = rusqlite::Connection::open_with_flags(
            self.store_dir.path().join("kernel.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let mut statement = connection
            .prepare(
                "SELECT evidence_id,artifact_digest,retain_until,sensitivity_class,provider_egress_class
                 FROM evidence_meta WHERE retention_class=?1 AND invalidated_commit_seq IS NULL
                 ORDER BY evidence_id",
            )
            .unwrap();
        statement
            .query_map([MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }
}

fn text_of(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[test]
fn a_read_captures_exact_bytes_once_with_typed_detail_and_a_charged_hold() {
    let fixture = Fixture::open();
    let body = "fn main() { println!(\"bün builds the wörkspace\"); }\n";
    fixture.write("src/main.rs", body.as_bytes());
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Local);

    let read = text
        .read(
            &fixture.store,
            &mut broker,
            "src/main.rs",
            Some(3..12),
            fixture.now,
        )
        .unwrap();
    assert_eq!(read.buffer.bytes(), &body.as_bytes()[3..12]);
    assert_eq!(read.buffer.tag().charged_bytes, 9);
    let rows = fixture.capture_rows();
    assert_eq!(rows.len(), 1, "one capture row");
    let (evidence_id, digest, retain_until, sensitivity, egress) = rows[0].clone();
    assert_eq!(digest, format!("{:x}", Sha256::digest(body.as_bytes())));
    assert_eq!(
        retain_until,
        Some(fixture.now + HOUR_MS),
        "the acquisition reference is finite from creation"
    );
    assert_eq!(
        sensitivity, "sensitive",
        "a repository path grants no default-Sensitive exemption"
    );
    assert_eq!(egress, "local_only");
    // The typed detail names the project, the path, the time, the digest, and the whole-file range; no Git identity is claimed for a working-tree read. The capture is registered in the inspection's domain and scope with its acquisition reference.
    let capture = fixture
        .store
        .local_file_capture(&evidence_id)
        .unwrap()
        .unwrap();
    assert_eq!(capture.domain_id, DOMAIN);
    assert_eq!(capture.scope_id, None);
    assert_eq!(capture.retain_until, fixture.now + HOUR_MS);
    let detail = capture.detail;
    assert_eq!(detail.detail_version, kernel::LOCAL_FILE_DETAIL_VERSION);
    assert_eq!(detail.project_digest, PROJECT);
    assert_eq!(detail.relative_path, "src/main.rs");
    assert!(
        detail.captured_at >= fixture.now,
        "the capture is dated at the read, never before the run clock"
    );
    assert_eq!(detail.buffer_digest, digest);
    assert_eq!(detail.range, (0, body.len() as u64));
    // Ownership was charged to the run's execution hold at capture, before the disclosing read.
    let held = fixture
        .store
        .validate_held_evidence(
            broker.hold_id(),
            MemoryReviewerHoldKind::Execution,
            &fixture.hold_binding(),
            std::slice::from_ref(&evidence_id),
            fixture.now,
        )
        .unwrap();
    assert_eq!(held.len(), 1);
    assert_eq!(
        held[0].retention_class,
        MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS
    );
    // The disclosure is a union member and a charged inspection.
    assert_eq!(broker.ledger.disclosed().count(), 1);
    assert_eq!(broker.accounting.issued_inspections(), 1);
    // A second read of the same bytes reuses the evidence: no new row, no new observation, a fresh alias sharing the origin.
    let again = text
        .read(
            &fixture.store,
            &mut broker,
            "src/main.rs",
            None,
            fixture.now,
        )
        .unwrap();
    assert_eq!(again.buffer.bytes(), body.as_bytes());
    assert_eq!(fixture.capture_rows().len(), 1);
    assert_ne!(again.alias, read.alias);
    assert_eq!(
        broker.shared_origin(again.alias.as_str()).unwrap(),
        Some(&read.alias)
    );
    // The same bytes under another path are one artifact but another capture: the CAS holds the object once, and each path has its own evidence row and detail.
    fixture.write("copy/main.rs", body.as_bytes());
    text.read(
        &fixture.store,
        &mut broker,
        "copy/main.rs",
        Some(0..4),
        fixture.now,
    )
    .unwrap();
    let rows = fixture.capture_rows();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].1, rows[1].1, "one object backs both captures");
}

#[test]
fn confinement_refuses_escapes_links_special_files_and_protected_locations() {
    let fixture = Fixture::open();
    fixture.write("ok.txt", b"plain text");
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), b"outside").unwrap();
    symlink(
        outside.path().join("secret.txt"),
        fixture.project.path().join("link.txt"),
    )
    .unwrap();
    symlink(outside.path(), fixture.project.path().join("linkdir")).unwrap();
    std::fs::create_dir_all(fixture.project.path().join("sub")).unwrap();
    symlink("..", fixture.project.path().join("sub/up")).unwrap();
    fixture.write(".git/config", b"[core]");
    fixture.write(".eidnara/eidnara.jsonc", b"{}");
    let fifo =
        std::ffi::CString::new(fixture.project.path().join("pipe").to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    // A store file hard-linked into the project resolves to a protected identity.
    let store_file = fixture.store_dir.path().join("kernel.sqlite");
    std::fs::hard_link(&store_file, fixture.project.path().join("linked.sqlite")).unwrap();
    // A store location that cannot be identified refuses construction rather than going unprotected.
    assert_eq!(
        ProtectedLocations::new([PathBuf::from("/definitely/missing/path")])
            .unwrap_err()
            .code,
        RefusalCode::Unavailable
    );
    let protected =
        ProtectedLocations::new([fixture.store_dir.path().to_path_buf(), store_file]).unwrap();
    // A hard link to a store file the identity set does not list is still refused: a linked file is not ordinary.
    let unlisted = fixture.store_dir.path().join("unlisted-store-file");
    std::fs::write(&unlisted, b"store internals").unwrap();
    std::fs::hard_link(&unlisted, fixture.project.path().join("linked.wal")).unwrap();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Local);
    let tip = fixture.store.tip().unwrap();
    for (path, code) in [
        ("link.txt", RefusalCode::NotRegularFile),
        ("linkdir/secret.txt", RefusalCode::Confinement),
        ("sub/up/ok.txt", RefusalCode::Confinement),
        ("../ok.txt", RefusalCode::InvalidPath),
        ("/etc/hostname", RefusalCode::InvalidPath),
        (".git/config", RefusalCode::Protected),
        (".eidnara/eidnara.jsonc", RefusalCode::Protected),
        ("linked.sqlite", RefusalCode::Protected),
        ("linked.wal", RefusalCode::NotRegularFile),
        ("pipe", RefusalCode::NotRegularFile),
        ("sub", RefusalCode::NotRegularFile),
        ("missing.txt", RefusalCode::NotFound),
    ] {
        let refusal = text
            .read(&fixture.store, &mut broker, path, None, fixture.now)
            .unwrap_err();
        assert_eq!(refusal.code, code, "{path}");
    }
    assert_eq!(
        fixture.store.tip().unwrap(),
        tip,
        "a refused path captures nothing"
    );
    assert!(fixture.capture_rows().is_empty());
    assert_eq!(broker.ledger.disclosed().count(), 0);
    // The ordinary file beside them is still readable.
    text.read(&fixture.store, &mut broker, "ok.txt", None, fixture.now)
        .unwrap();
    // A root that contains a protected data root, or lies inside one, is unavailable as a whole.
    let inside = fixture.store_dir.path().join("project-inside-store");
    std::fs::create_dir_all(&inside).unwrap();
    assert_eq!(
        ProjectText::open(&inside, &protected, fixture.binding())
            .unwrap_err()
            .code,
        RefusalCode::Unavailable
    );
    let over = ProtectedLocations::new([fixture.project.path().join("sub")]).unwrap();
    assert_eq!(
        ProjectText::open(fixture.project.path(), &over, fixture.binding())
            .unwrap_err()
            .code,
        RefusalCode::Unavailable
    );
}

#[test]
fn oversized_undecodable_and_secret_bearing_files_are_refused_before_any_write() {
    let fixture = Fixture::open();
    let big = usize::try_from(MAX_CAPTURE_BYTES).unwrap();
    fixture.write("big.txt", &vec![b'x'; big + 1]);
    fixture.write("exact.txt", &"y".repeat(big).into_bytes());
    fixture.write("binary.bin", &[0xff, 0xfe, 0x00, 0x41]);
    fixture.write("secret.env", b"AWS_ACCESS_KEY_ID=AKIAQ7RSTUVWXYZ23456\n");
    fixture.write(
        "marker.txt",
        format!("x {} y", kernel::OPERATOR_REDACTION_PLACEHOLDER).as_bytes(),
    );
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Local);
    let tip = fixture.store.tip().unwrap();
    for (path, code) in [
        ("big.txt", RefusalCode::TooLarge),
        ("binary.bin", RefusalCode::Undecodable),
        ("secret.env", RefusalCode::RenderCheck),
        ("marker.txt", RefusalCode::RenderCheck),
    ] {
        assert_eq!(
            text.read(&fixture.store, &mut broker, path, None, fixture.now)
                .unwrap_err()
                .code,
            code,
            "{path}"
        );
    }
    assert_eq!(
        fixture.store.tip().unwrap(),
        tip,
        "a refused capture writes nothing"
    );
    assert!(fixture.capture_rows().is_empty());
    // Exactly the cap is accepted.
    let read = text
        .read(
            &fixture.store,
            &mut broker,
            "exact.txt",
            Some(0..8),
            fixture.now,
        )
        .unwrap();
    assert_eq!(read.buffer.bytes(), b"yyyyyyyy");
    assert_eq!(fixture.capture_rows().len(), 1);
}

#[test]
fn a_remote_destination_cannot_capture_anything() {
    let fixture = Fixture::open();
    fixture.write("notes.md", b"bun builds the workspace");
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Remote);
    let tip = fixture.store.tip().unwrap();
    assert_eq!(
        text.read(&fixture.store, &mut broker, "notes.md", None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::PolicyBlocked
    );
    assert_eq!(
        text.search(
            &fixture.store,
            &mut broker,
            SearchQuery::Content("bun"),
            fixture.now,
        )
        .unwrap_err()
        .code,
        RefusalCode::PolicyBlocked,
        "a remote run is refused before any file is read"
    );
    assert_eq!(fixture.store.tip().unwrap(), tip);
    assert!(fixture.capture_rows().is_empty());
    assert_eq!(broker.ledger.disclosed().count(), 0);
}

#[test]
fn search_matches_paths_names_and_content_within_bounds_and_discloses_excerpts_only() {
    let fixture = Fixture::open();
    fixture.write("README.md", b"# Project\nbun builds the workspace\n");
    fixture.write("src/lib.rs", b"pub fn build() {}\n");
    fixture.write("src/nested/deep.rs", b"// the workspace builds with bun\n");
    fixture.write("docs/guide.md", b"unrelated\n");
    fixture.write(".git/HEAD", b"ref: refs/heads/main\n");
    fixture.write(
        "src/big.rs",
        &vec![b'z'; usize::try_from(MAX_CAPTURE_BYTES).unwrap() + 1],
    );
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Local);
    let tip = fixture.store.tip().unwrap();

    let content = text
        .search(
            &fixture.store,
            &mut broker,
            SearchQuery::Content("bun"),
            fixture.now,
        )
        .unwrap();
    assert_eq!(content.completeness, Completeness::Complete);
    assert_eq!(content.hits.len(), 2);
    assert!(
        content.withheld,
        "the Git store and the oversized file are withheld"
    );
    for hit in &content.hits {
        assert!(text_of(hit.buffer.bytes()).contains("bun"));
        assert!(hit.span.end - hit.span.start <= MAX_EXCERPT_BYTES as u64);
        assert_eq!(
            hit.span.end - hit.span.start,
            hit.buffer.bytes().len() as u64
        );
    }
    assert!(
        fixture.store.tip().unwrap() > tip,
        "hits are captures, so a search commits their evidence"
    );
    assert_eq!(fixture.capture_rows().len(), 2);
    broker.accounting.end_batch();

    let name = text
        .search(
            &fixture.store,
            &mut broker,
            SearchQuery::Name(".rs"),
            fixture.now,
        )
        .unwrap();
    assert_eq!(
        name.hits.len(),
        2,
        "lib and deep are delivered; the oversized file's name matches but it is withheld"
    );
    assert!(name.withheld);
    assert!(name.hits.iter().all(|hit| hit.span.start == 0));
    broker.accounting.end_batch();

    let path = text
        .search(
            &fixture.store,
            &mut broker,
            SearchQuery::Path("src/nested"),
            fixture.now,
        )
        .unwrap();
    assert_eq!(path.hits.len(), 1);
    assert_eq!(
        fixture.capture_rows().len(),
        3,
        "captures are reused across searches"
    );
    // Every hit is a disclosed union member; nothing names a path.
    assert_eq!(broker.ledger.disclosed().count(), 2 + 2 + 1);
    broker.accounting.end_batch();

    assert_eq!(
        text.search(
            &fixture.store,
            &mut broker,
            SearchQuery::Content(""),
            fixture.now
        )
        .unwrap_err()
        .code,
        RefusalCode::InvalidPath
    );
}

#[test]
fn search_bounds_stop_with_explicit_incompleteness() {
    let fixture = Fixture::open();
    // Batch headroom: nine matching files, one batch of eight operations.
    for index in 0..MAX_OPERATIONS_PER_BATCH + 1 {
        fixture.write(
            &format!("m{index:02}.txt"),
            format!("bun {index}").as_bytes(),
        );
    }
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Local);
    let first = text
        .search(
            &fixture.store,
            &mut broker,
            SearchQuery::Content("bun"),
            fixture.now,
        )
        .unwrap();
    assert_eq!(first.completeness, Completeness::CapacityBound);
    assert_eq!(first.hits.len(), MAX_OPERATIONS_PER_BATCH);
    assert!(
        broker.ledger.conclusions_usable(),
        "stopping at the headroom is not a partial disclosure"
    );
    assert_eq!(
        text.search(
            &fixture.store,
            &mut broker,
            SearchQuery::Content("bun"),
            fixture.now
        )
        .unwrap_err()
        .code,
        RefusalCode::BatchLimit
    );
    // Depth: a directory chain deeper than the bound is withheld, never descended.
    let deep = vec!["d"; MAX_DEPTH + 1].join("/");
    fixture.write(&format!("{deep}/leaf.txt"), b"bun deep");
    broker.accounting.end_batch();
    let mut fresh = fixture.text(&protected);
    let outcome = fresh
        .search(
            &fixture.store,
            &mut broker,
            SearchQuery::Name("leaf"),
            fixture.now,
        )
        .unwrap();
    assert!(outcome.hits.is_empty());
    assert!(outcome.withheld);
    // Visited entries: more files than one search may visit.
    let many = Fixture::open();
    for index in 0..MAX_VISITED_ENTRIES + 1 {
        many.write(&format!("f/{index:05}.txt"), b"filler");
    }
    many.write("zz-last.txt", b"bun at the end");
    let protected = many.protected();
    let mut text = many.text(&protected);
    let mut broker = many.broker(ArtifactDestination::Local);
    let outcome = text
        .search(
            &many.store,
            &mut broker,
            SearchQuery::Content("bun"),
            many.now,
        )
        .unwrap();
    assert_eq!(
        outcome.completeness,
        Completeness::CandidateBound,
        "a listing cut short is incompleteness, not absence"
    );
    assert!(outcome.withheld);
    assert_eq!(
        outcome.hits.len(),
        1,
        "entries listed before the bound are still examined"
    );
    // Scan bytes: more matching-candidate bytes than one search may read.
    let heavy = Fixture::open();
    let megabyte = "filler words ".repeat(usize::try_from(MAX_CAPTURE_BYTES).unwrap() / 13);
    for index in 0..(MAX_SCAN_BYTES / MAX_CAPTURE_BYTES + 1) {
        heavy.write(&format!("h{index:02}.txt"), megabyte.as_bytes());
    }
    let protected = heavy.protected();
    let mut text = heavy.text(&protected);
    let mut broker = heavy.broker(ArtifactDestination::Local);
    let outcome = text
        .search(
            &heavy.store,
            &mut broker,
            SearchQuery::Content("zzz"),
            heavy.now,
        )
        .unwrap();
    assert_eq!(outcome.completeness, Completeness::ProbeBound);
    assert!(outcome.hits.is_empty());
    assert!(
        heavy.capture_rows().is_empty(),
        "an unmatched file is never captured"
    );
}

#[test]
fn a_matching_refused_file_is_indistinguishable_from_no_match() {
    let fixture = Fixture::open();
    fixture.write("keys.env", b"AWS_ACCESS_KEY_ID=AKIAQ7RSTUVWXYZ23456\n");
    fixture.write("notes.txt", b"nothing secret here");
    // The path, not the content, trips the render check.
    fixture.write(
        &format!("tagged {}.txt", kernel::OPERATOR_REDACTION_PLACEHOLDER),
        b"tagged body",
    );
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Local);
    let tip = fixture.store.tip().unwrap();
    // Each pair names one literal that only a refused file satisfies and one that nothing satisfies.
    let pairs: [(SearchQuery<'_>, SearchQuery<'_>); 4] = [
        (
            SearchQuery::Content("AKIAQ7"),
            SearchQuery::Content("no such literal"),
        ),
        (
            SearchQuery::Content("tagged body"),
            SearchQuery::Content("no such literal"),
        ),
        (SearchQuery::Name(".env"), SearchQuery::Name(".zzz")),
        (SearchQuery::Path("keys"), SearchQuery::Path("locks")),
    ];
    for (probe, absent) in pairs {
        let matching = text
            .search(&fixture.store, &mut broker, probe, fixture.now)
            .unwrap();
        let absent = text
            .search(&fixture.store, &mut broker, absent, fixture.now)
            .unwrap();
        assert!(
            matching.hits.is_empty() && absent.hits.is_empty(),
            "{probe:?}"
        );
        assert_eq!(
            matching.withheld, absent.withheld,
            "withholding does not depend on the literal: {probe:?}"
        );
        assert_eq!(matching.completeness, absent.completeness, "{probe:?}");
    }
    assert_eq!(fixture.store.tip().unwrap(), tip);
    assert_eq!(broker.ledger.disclosed().count(), 0);
}

#[test]
fn every_listed_name_counts_against_the_visited_bound() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let fixture = Fixture::open();
    std::fs::create_dir(fixture.project.path().join("names")).unwrap();
    for index in 0..MAX_VISITED_ENTRIES {
        // A trailing 0xff byte makes the name invalid UTF-8.
        let name = [format!("n{index:05}").as_bytes(), b"\xff"].concat();
        std::fs::write(
            fixture
                .project
                .path()
                .join("names")
                .join(OsStr::from_bytes(&name)),
            b"filler",
        )
        .unwrap();
    }
    fixture.write("zz-last.txt", b"bun at the end");
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Local);
    let outcome = text
        .search(
            &fixture.store,
            &mut broker,
            SearchQuery::Content("bun"),
            fixture.now,
        )
        .unwrap();
    assert_eq!(
        outcome.completeness,
        Completeness::CandidateBound,
        "names that are not UTF-8 still count as listed entries"
    );
    assert!(outcome.withheld);
}

#[test]
fn an_empty_file_that_matches_by_name_is_a_hit_with_an_empty_excerpt() {
    let fixture = Fixture::open();
    fixture.write("pkg/__init__.py", b"");
    fixture.write("pkg/module.py", b"def bun():\n    pass\n");
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Local);
    let outcome = text
        .search(
            &fixture.store,
            &mut broker,
            SearchQuery::Name("__init__"),
            fixture.now,
        )
        .unwrap();
    assert_eq!(
        outcome.hits.len(),
        1,
        "an empty match is a hit, not a refusal"
    );
    assert_eq!(outcome.hits[0].span, 0..0);
    assert!(outcome.hits[0].buffer.bytes().is_empty());
    assert!(!outcome.withheld);
    assert_eq!(outcome.completeness, Completeness::Complete);
    assert_eq!(
        broker.accounting.issued_inspections(),
        1,
        "the hit costs one operation and nothing is spent on a refusal"
    );
    let read = broker
        .read(
            &fixture.store,
            outcome.hits[0].alias.as_str(),
            None,
            fixture.now,
        )
        .unwrap();
    assert!(read.buffer.bytes().is_empty());
    let direct = text
        .read(
            &fixture.store,
            &mut broker,
            "pkg/__init__.py",
            None,
            fixture.now,
        )
        .unwrap();
    assert!(direct.buffer.bytes().is_empty());
    assert_eq!(
        fixture.capture_rows().len(),
        1,
        "one capture backs every read"
    );
}

#[test]
fn a_capture_is_charged_to_the_hold_before_any_disclosure() {
    let fixture = Fixture::open();
    // More than one render may show the model: the disclosing read is refused after the capture is charged.
    let oversized =
        usize::try_from(daemon::memory_reviewer::broker::MAX_MODEL_VISIBLE_BYTES).unwrap() + 1;
    fixture.write("a.txt", &vec![b'x'; oversized]);
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Local);
    assert_eq!(
        text.read(&fixture.store, &mut broker, "a.txt", None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::ByteLimit
    );
    let rows = fixture.capture_rows();
    assert_eq!(rows.len(), 1);
    let held = fixture
        .store
        .validate_held_evidence(
            broker.hold_id(),
            MemoryReviewerHoldKind::Execution,
            &fixture.hold_binding(),
            std::slice::from_ref(&rows[0].0),
            fixture.now,
        )
        .unwrap();
    assert_eq!(
        held.len(),
        1,
        "the hold covers the capture though nothing was disclosed"
    );
    assert_eq!(broker.ledger.disclosed().count(), 0);
    assert!(
        fixture
            .store
            .local_file_capture(&rows[0].0)
            .unwrap()
            .is_some()
    );
}

#[test]
fn expiry_retires_the_capture_and_its_detail_but_keeps_independent_support() {
    let fixture = Fixture::open();
    let body = b"shared bytes: bun builds the workspace\n";
    // The same bytes already exist as canonical evidence.
    let canonical = fixture
        .store
        .ingest_artifact(ArtifactIngestRequest {
            intent: intent("canonical"),
            payload: body.to_vec(),
            evidence_id: "evidence-canonical".to_string(),
            object_id: "evidence-object-canonical".to_string(),
            object_kind: "evidence".to_string(),
            domain_id: DOMAIN.to_string(),
            source_kind: "conversation".to_string(),
            source_id: "src/canonical".to_string(),
            source_revision: 1,
            media_type: "text/plain".to_string(),
            retention_class: "canonical".to_string(),
            retain_until: None,
            asserted_sensitivity: Sensitivity::Normal,
            provider_egress: ProviderEgress::RemoteAllowed,
            provenance: None,
        })
        .unwrap();
    fixture.write("notes.txt", body);
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Local);
    text.read(&fixture.store, &mut broker, "notes.txt", None, fixture.now)
        .unwrap();
    let rows = fixture.capture_rows();
    assert_eq!(rows.len(), 1);
    let (evidence_id, digest, ..) = rows[0].clone();
    assert_eq!(digest, canonical.digest, "one object backs both references");
    // Nothing expires while the hold is live, even past retain_until.
    let later = fixture.now + HOUR_MS + 1;
    assert_eq!(
        fixture.store.expire_local_file_captures(later).unwrap(),
        kernel::CaptureExpiry::default()
    );
    // Once the hold has lapsed, expiry retires the detail and then the evidence; the canonical reference and its bytes remain.
    let after_hold = fixture.now + 3 * HOUR_MS;
    assert_eq!(
        fixture
            .store
            .expire_local_file_captures(after_hold)
            .unwrap()
            .retired,
        1
    );
    assert!(fixture.capture_rows().is_empty());
    assert!(
        fixture
            .store
            .local_file_capture(&evidence_id)
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .store
            .read_artifact(&ArtifactHandle {
                digest: digest.clone(),
                evidence_id
            })
            .is_err()
    );
    assert_eq!(fixture.store.read_artifact(&canonical).unwrap(), body);
    assert_eq!(
        fixture
            .store
            .expire_local_file_captures(after_hold + 1)
            .unwrap()
            .retired,
        0
    );
    // A capture some other live observation cites keeps its evidence: only the capture's own detail is retired, and the foreign row is untouched.
    fixture.write("cited.txt", b"cited by another observation");
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Local);
    broker.accounting.end_batch();
    text.read(&fixture.store, &mut broker, "cited.txt", None, fixture.now)
        .unwrap();
    let cited = fixture.capture_rows()[0].0.clone();
    fixture
        .store
        .commit(intent("foreign"), |envelope| {
            envelope.insert_observation(note_observation("foreign-1", &cited, "note"))?;
            Ok(String::new())
        })
        .unwrap();
    assert_eq!(
        fixture
            .store
            .expire_local_file_captures(after_hold + 2)
            .unwrap(),
        kernel::CaptureExpiry {
            retired: 0,
            retained: 1
        },
        "the cited capture is retained, and the sweep reports that work"
    );
    assert_eq!(
        fixture.capture_rows().len(),
        1,
        "the cited evidence stays live"
    );
    assert!(
        fixture.store.local_file_capture(&cited).unwrap().is_none(),
        "the capture's own detail is retired"
    );
    // A retained capture leaves the sweep: a later call neither revisits it nor commits anything for it, and once the citation is gone the evidence is retired.
    let tip = fixture.store.tip().unwrap();
    assert_eq!(
        fixture
            .store
            .expire_local_file_captures(after_hold + 3)
            .unwrap()
            .retired,
        0
    );
    assert_eq!(
        fixture.store.tip().unwrap(),
        tip,
        "a retained capture is not re-processed on every sweep"
    );
    fixture
        .store
        .commit(intent("release-foreign"), |envelope| {
            envelope.retire_observation("foreign-1-object")?;
            Ok(String::new())
        })
        .unwrap();
    assert_eq!(
        fixture
            .store
            .expire_local_file_captures(after_hold + 4)
            .unwrap()
            .retired,
        1,
        "the evidence is retired once nothing else cites it"
    );
    assert!(fixture.capture_rows().is_empty());
}

fn note_observation(id: &str, evidence_id: &str, kind: &str) -> kernel::ObservationSpec {
    kernel::ObservationSpec {
        observation_id: id.to_string(),
        object_id: format!("{id}-object"),
        domain_id: DOMAIN.to_string(),
        proposition_id: None,
        scope_id: None,
        anchor_id: None,
        evidence_id: Some(evidence_id.to_string()),
        observation_kind: kind.to_string(),
        payload: kernel::ObservationPayload {
            summary: "independent support".to_string(),
            classification: kind.to_string(),
            detail: None,
        },
        observed_at: 1,
        dependencies: Vec::new(),
        source_kind: "test".to_string(),
        source_id: "foreign".to_string(),
        source_revision: 1,
        sensitivity: Sensitivity::Sensitive,
    }
}

#[test]
fn retained_captures_do_not_starve_newer_expired_captures() {
    let fixture = Fixture::open();
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Local);
    // One more expired, unpinned capture than one sweep page; live observations cite all but the newest capture. Each broker is a run with its own inspection ceiling.
    for index in 0..=kernel::MAX_EXPIRED_CAPTURES_PER_CALL {
        if index > 0 && index % MAX_ISSUED_INSPECTIONS == 0 {
            broker = fixture.broker(ArtifactDestination::Local);
        }
        if index % MAX_OPERATIONS_PER_BATCH == 0 {
            broker.accounting.end_batch();
        }
        let relative = format!("f{index:03}.txt");
        fixture.write(&relative, format!("capture {index}").as_bytes());
        text.read(&fixture.store, &mut broker, &relative, None, fixture.now)
            .unwrap();
    }
    let rows = fixture.capture_rows();
    assert_eq!(rows.len(), kernel::MAX_EXPIRED_CAPTURES_PER_CALL + 1);
    fixture
        .store
        .commit(intent("cite-all-but-one"), |envelope| {
            for (index, (evidence_id, ..)) in rows.iter().enumerate() {
                if evidence_id != &rows[rows.len() - 1].0 {
                    envelope.insert_observation(note_observation(
                        &format!("cite-{index}"),
                        evidence_id,
                        "note",
                    ))?;
                }
            }
            Ok(String::new())
        })
        .unwrap();
    let after_hold = fixture.now + 3 * HOUR_MS;
    let first = fixture
        .store
        .expire_local_file_captures(after_hold)
        .unwrap()
        .retired;
    let second = fixture
        .store
        .expire_local_file_captures(after_hold + 1)
        .unwrap()
        .retired;
    assert_eq!(
        first + second,
        1,
        "the one uncited capture is retired even though a full page of captures is retained"
    );
    assert_eq!(
        fixture.capture_rows().len(),
        kernel::MAX_EXPIRED_CAPTURES_PER_CALL,
        "retained captures stay live"
    );
    let tip = fixture.store.tip().unwrap();
    assert_eq!(
        fixture
            .store
            .expire_local_file_captures(after_hold + 2)
            .unwrap()
            .retired,
        0
    );
    assert_eq!(
        fixture.store.tip().unwrap(),
        tip,
        "a sweep with nothing to retire commits nothing"
    );
}

#[test]
fn a_capture_the_store_refuses_to_retire_does_not_stall_the_sweep() {
    let fixture = Fixture::open();
    // A full page of MemoryReviewer-capture rows whose registry objects are not evidence objects: retiring one is refused as NotFound on every sweep. Their acquisition references sort ahead of the run's capture, so a sweep that only ever looked at the first page would never reach it.
    for index in 0..kernel::MAX_EXPIRED_CAPTURES_PER_CALL {
        fixture
            .store
            .ingest_exact_artifact(ArtifactIngestRequest {
                intent: intent(&format!("unretirable-{index}")),
                payload: format!("not an evidence object {index}").into_bytes(),
                evidence_id: format!("unretirable-{index:03}"),
                object_id: format!("unretirable-object-{index:03}"),
                object_kind: "artifact".to_string(),
                domain_id: DOMAIN.to_string(),
                source_kind: "local_file".to_string(),
                source_id: "unretirable.txt".to_string(),
                source_revision: 1,
                media_type: "text/plain".to_string(),
                retention_class: MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS.to_string(),
                retain_until: Some(fixture.now + HOUR_MS - 1),
                asserted_sensitivity: Sensitivity::Sensitive,
                provider_egress: ProviderEgress::LocalOnly,
                provenance: None,
            })
            .unwrap();
    }
    fixture.write("a.txt", b"captured behind the refused rows");
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Local);
    text.read(&fixture.store, &mut broker, "a.txt", None, fixture.now)
        .unwrap();
    assert_eq!(
        fixture.capture_rows().len(),
        kernel::MAX_EXPIRED_CAPTURES_PER_CALL + 1
    );
    let after_hold = fixture.now + 3 * HOUR_MS;
    assert_eq!(
        fixture
            .store
            .expire_local_file_captures(after_hold)
            .unwrap()
            .retired,
        1,
        "the capture behind the refused rows is retired in the same sweep"
    );
    let rows = fixture.capture_rows();
    assert_eq!(rows.len(), kernel::MAX_EXPIRED_CAPTURES_PER_CALL);
    assert!(rows.iter().all(|row| row.0.starts_with("unretirable-")));
}

#[test]
fn a_generic_correction_cannot_forge_a_capture_observation() {
    let fixture = Fixture::open();
    fixture.write("a.txt", b"captured");
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Local);
    text.read(&fixture.store, &mut broker, "a.txt", None, fixture.now)
        .unwrap();
    let capture = fixture.capture_rows()[0].0.clone();
    fixture
        .store
        .commit(intent("ordinary"), |envelope| {
            envelope.insert_observation(note_observation("ordinary", &capture, "note"))?;
            Ok(String::new())
        })
        .unwrap();
    for (key, kind, id) in [
        ("kind", kernel::LOCAL_FILE_KIND, "corrected"),
        ("observation-id", "note", "localfile:forged"),
        ("object-id", "note", "forged"),
    ] {
        let mut replacement = note_observation(id, &capture, kind);
        if key == "object-id" {
            replacement.object_id = "localfileobj:forged".to_string();
        }
        replacement.source_revision = 2;
        let refused = fixture.store.commit(intent(key), |envelope| {
            envelope.correct_observation("ordinary-object", replacement.clone())?;
            Ok(String::new())
        });
        assert_eq!(refused, Err(kernel::KernelError::InvalidInput), "{key}");
    }
}

#[test]
fn a_root_that_exists_but_cannot_be_opened_is_not_reported_as_absent() {
    let fixture = Fixture::open();
    let protected = fixture.protected();
    let file = fixture.write("plain.txt", b"a file, not a directory");
    assert_eq!(
        ProjectText::open(&file, &protected, fixture.binding())
            .unwrap_err()
            .code,
        RefusalCode::NotRegularFile,
        "an existing root that is not a directory is a type refusal"
    );
    assert_eq!(
        ProjectText::open(
            &fixture.project.path().join("missing"),
            &protected,
            fixture.binding()
        )
        .unwrap_err()
        .code,
        RefusalCode::NotFound
    );
    assert_eq!(
        ProjectText::open(&file.join("below-a-file"), &protected, fixture.binding())
            .unwrap_err()
            .code,
        RefusalCode::Unavailable,
        "a root whose ancestor is not a directory is a host failure, not absence"
    );
}

#[test]
fn refused_files_are_charged_against_the_scan_bound() {
    let fixture = Fixture::open();
    // One more undecodable file than the scan bound admits, each read in full before it is refused.
    let mut undecodable = vec![b'x'; usize::try_from(MAX_CAPTURE_BYTES).unwrap()];
    undecodable[0] = 0xff;
    for index in 0..(MAX_SCAN_BYTES / MAX_CAPTURE_BYTES + 1) {
        fixture.write(&format!("b{index:02}.bin"), &undecodable);
    }
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Local);
    let outcome = text
        .search(
            &fixture.store,
            &mut broker,
            SearchQuery::Content("zzz"),
            fixture.now,
        )
        .unwrap();
    assert_eq!(
        outcome.completeness,
        Completeness::ProbeBound,
        "bytes read from a refused file count toward the scan bound"
    );
    assert!(outcome.withheld);
    assert!(outcome.hits.is_empty());
}

#[test]
fn captures_of_identical_bytes_by_two_projects_are_two_captures() {
    let fixture = Fixture::open();
    let body = b"the same bytes in two projects\n";
    fixture.write("same.txt", body);
    let other_root = tempfile::tempdir().unwrap();
    std::fs::write(other_root.path().join("elsewhere.txt"), body).unwrap();
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Local);
    text.read(&fixture.store, &mut broker, "same.txt", None, fixture.now)
        .unwrap();
    // A second project whose run shares the first's job subject and generation captures the same bytes under another path.
    let other_project = "b".repeat(64);
    let mut other_text = ProjectText::open(
        other_root.path(),
        &protected,
        fixture.binding_for(&other_project),
    )
    .unwrap();
    let mut other_broker = fixture.broker_for(&other_project, ArtifactDestination::Local);
    other_text
        .read(
            &fixture.store,
            &mut other_broker,
            "elsewhere.txt",
            None,
            fixture.now,
        )
        .unwrap();
    let rows = fixture.capture_rows();
    assert_eq!(rows.len(), 2, "one capture per project, not a replay");
    let details: Vec<(String, String)> = rows
        .iter()
        .map(|(evidence_id, ..)| {
            let detail = fixture
                .store
                .local_file_capture(evidence_id)
                .unwrap()
                .unwrap();
            (detail.detail.project_digest, detail.detail.relative_path)
        })
        .collect();
    assert!(details.contains(&(PROJECT.to_string(), "same.txt".to_string())));
    assert!(details.contains(&(other_project, "elsewhere.txt".to_string())));
}

#[test]
fn the_capture_writer_refuses_a_detail_that_does_not_describe_a_confined_capture() {
    let fixture = Fixture::open();
    // Two bodies: the store folds egress across every row of one digest, so remote-allowed evidence needs bytes of its own.
    let body = b"bytes the writer is asked to describe";
    let remote_body = b"bytes that may leave the host";
    let ingest = |evidence_id: &str, payload: &[u8], provider_egress| {
        fixture
            .store
            .ingest_exact_artifact(ArtifactIngestRequest {
                intent: intent(&format!("ingest-{evidence_id}")),
                payload: payload.to_vec(),
                evidence_id: evidence_id.to_string(),
                object_id: format!("{evidence_id}-object"),
                object_kind: "evidence".to_string(),
                domain_id: DOMAIN.to_string(),
                source_kind: "local_file".to_string(),
                source_id: "src/main.rs".to_string(),
                source_revision: 1,
                media_type: "text/plain".to_string(),
                retention_class: MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS.to_string(),
                retain_until: Some(fixture.now + HOUR_MS),
                asserted_sensitivity: Sensitivity::Sensitive,
                provider_egress,
                provenance: None,
            })
            .unwrap();
    };
    ingest("local", body, ProviderEgress::LocalOnly);
    ingest("remote", remote_body, ProviderEgress::RemoteAllowed);
    let record = |key: &str, evidence_id: &str, project_digest: &str, relative_path: &str| {
        let bytes: &[u8] = if evidence_id == "remote" {
            remote_body
        } else {
            body
        };
        fixture.store.commit(intent(key), |envelope| {
            envelope.record_local_file_capture(&kernel::LocalFileCaptureRequest {
                project_digest,
                relative_path,
                captured_at: fixture.now,
                domain_id: DOMAIN,
                scope_id: None,
                evidence_id,
                artifact_digest: &format!("{:x}", Sha256::digest(bytes)),
                byte_length: u64::try_from(bytes.len()).unwrap(),
            })?;
            Ok(String::new())
        })
    };
    for (key, project, path) in [
        ("absolute", PROJECT, "/etc/passwd"),
        ("traversing", PROJECT, "src/../../etc/passwd"),
        ("dot", PROJECT, "./src/main.rs"),
        ("empty-component", PROJECT, "src//main.rs"),
        ("nul", PROJECT, "src/\0main.rs"),
        ("empty", PROJECT, ""),
        ("not-a-digest", "the project", "src/main.rs"),
        ("uppercase-digest", &"A".repeat(64), "src/main.rs"),
    ] {
        assert_eq!(
            record(key, "local", project, path),
            Err(kernel::KernelError::InvalidInput),
            "{key}"
        );
    }
    assert_eq!(
        record("remote", "remote", PROJECT, "src/main.rs"),
        Err(kernel::KernelError::NotFound),
        "evidence that may leave the host is not a project capture"
    );
    record("ok", "local", PROJECT, "src/main.rs").unwrap();
    assert_eq!(
        fixture
            .store
            .local_file_capture("local")
            .unwrap()
            .unwrap()
            .detail
            .relative_path,
        "src/main.rs"
    );
}

#[test]
fn a_hold_capacity_refusal_on_a_capture_marks_the_evidence_set_partial() {
    let fixture = Fixture::open();
    fixture.write("a.txt", b"bun one");
    fixture.write("b.txt", b"bun two, longer");
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    // Room on the hold for the first capture only: the second capture's hold extension is a capacity refusal before any disclosure of it.
    let mut broker = fixture.broker_with_references(
        PROJECT,
        ArtifactDestination::Local,
        kernel::MAX_MEMORY_REVIEWER_HOLD_REFERENCES - 1,
    );
    let outcome = text
        .search(
            &fixture.store,
            &mut broker,
            SearchQuery::Content("bun"),
            fixture.now,
        )
        .unwrap();
    assert_eq!(outcome.completeness, Completeness::CapacityBound);
    assert_eq!(outcome.hits.len(), 1);
    assert!(
        !broker.ledger.conclusions_usable(),
        "a hold capacity refusal on a capture truncates the evidence set like one on a read"
    );
}

#[test]
fn a_hold_capacity_refusal_on_the_first_capture_is_the_answer_and_still_marks_the_set() {
    let fixture = Fixture::open();
    fixture.write("a.txt", b"bun one");
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    // A full hold: the first capture's extension is refused before any disclosure.
    let mut broker = fixture.broker_with_references(
        PROJECT,
        ArtifactDestination::Local,
        kernel::MAX_MEMORY_REVIEWER_HOLD_REFERENCES,
    );
    assert_eq!(
        text.read(&fixture.store, &mut broker, "a.txt", None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::HoldLimit
    );
    assert!(!broker.ledger.conclusions_usable());
}

#[test]
fn a_root_inside_a_refused_component_is_protected() {
    let fixture = Fixture::open();
    fixture.write(
        ".git/config",
        b"[remote \"origin\"]\n\turl = https://token@example.invalid/repo.git\n",
    );
    fixture.write(".eidnara/store/state.json", b"{}");
    fixture.write("ok/a.txt", b"plain");
    let protected = fixture.protected();
    for root in [".git", ".eidnara/store"] {
        assert_eq!(
            ProjectText::open(
                &fixture.project.path().join(root),
                &protected,
                fixture.binding()
            )
            .unwrap_err()
            .code,
            RefusalCode::Protected,
            "{root}"
        );
    }
    ProjectText::open(
        &fixture.project.path().join("ok"),
        &protected,
        fixture.binding(),
    )
    .unwrap();
}

#[test]
fn a_broker_of_another_project_is_refused_before_any_path_is_read() {
    let fixture = Fixture::open();
    fixture.write("a.txt", b"bun in project a");
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut other = fixture.broker_for(&"b".repeat(64), ArtifactDestination::Local);
    let tip = fixture.store.tip().unwrap();
    assert_eq!(
        text.read(&fixture.store, &mut other, "a.txt", None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::Scope
    );
    assert_eq!(
        text.search(
            &fixture.store,
            &mut other,
            SearchQuery::Content("bun"),
            fixture.now
        )
        .unwrap_err()
        .code,
        RefusalCode::Scope
    );
    assert_eq!(fixture.store.tip().unwrap(), tip, "nothing was captured");
    assert!(fixture.capture_rows().is_empty());
    assert_eq!(other.ledger.disclosed().count(), 0);
}

#[test]
fn a_seated_observation_receipt_cannot_stand_in_for_the_capture_detail() {
    let fixture = Fixture::open();
    let body = b"bytes whose capture identity is predictable";
    fixture.write("a.txt", body);
    let digest = format!("{:x}", Sha256::digest(body));
    let hold = fixture.hold_binding();
    let evidence_id = kernel::local_file_capture_id(&hold, &digest, "a.txt");
    // A caller seats a receipt under the capture's observation intent before the run captures the file.
    fixture
        .store
        .commit(
            CommitIntent {
                producer: "memory_reviewer".to_string(),
                operation_key: format!("{evidence_id}:observation"),
                request_digest: digest.clone(),
                actor: "memory_reviewer".to_string(),
                cause: "project_text_capture".to_string(),
            },
            |_| Ok(String::new()),
        )
        .unwrap();
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Local);
    assert_eq!(
        text.read(&fixture.store, &mut broker, "a.txt", None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::Store,
        "a capture without its typed detail is not disclosed"
    );
    assert!(
        fixture
            .store
            .local_file_capture(&evidence_id)
            .unwrap()
            .is_none()
    );
    assert_eq!(broker.ledger.disclosed().count(), 0);
}

#[test]
fn two_paths_with_identical_bytes_are_two_captures_with_their_own_paths() {
    let fixture = Fixture::open();
    let body = b"the same bytes under two names";
    fixture.write("a.txt", body);
    fixture.write("b/copy.txt", body);
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Local);
    let outcome = text
        .search(
            &fixture.store,
            &mut broker,
            SearchQuery::Name("copy"),
            fixture.now,
        )
        .unwrap();
    assert_eq!(outcome.hits.len(), 1);
    let (_, expectation) = broker
        .aliases
        .resolve(outcome.hits[0].alias.as_str())
        .unwrap();
    let daemon::memory_reviewer::broker::ReferenceExpectation::TemporaryCapture {
        evidence_id, ..
    } = expectation
    else {
        panic!("a search hit is a temporary capture");
    };
    assert_eq!(
        fixture
            .store
            .local_file_capture(evidence_id)
            .unwrap()
            .unwrap()
            .detail
            .relative_path,
        "b/copy.txt",
        "the hit's provenance names the path that matched"
    );
    text.read(&fixture.store, &mut broker, "a.txt", None, fixture.now)
        .unwrap();
    let rows = fixture.capture_rows();
    assert_eq!(rows.len(), 2, "one capture per path");
    assert_eq!(rows[0].1, rows[1].1, "one object backs both");
}

#[test]
fn a_generic_writer_cannot_retire_a_capture_observation() {
    let fixture = Fixture::open();
    fixture.write("a.txt", b"captured");
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Local);
    text.read(&fixture.store, &mut broker, "a.txt", None, fixture.now)
        .unwrap();
    let evidence_id = fixture.capture_rows()[0].0.clone();
    let refused = fixture.store.commit(intent("retire-capture"), |envelope| {
        envelope.retire_observation(&format!("localfileobj:{evidence_id}"))?;
        Ok(String::new())
    });
    assert_eq!(refused, Err(kernel::KernelError::InvalidInput));
    assert!(
        fixture
            .store
            .local_file_capture(&evidence_id)
            .unwrap()
            .is_some(),
        "the capture's detail is still live"
    );
    // Expiry, the coordinated path, still retires it.
    assert_eq!(
        fixture
            .store
            .expire_local_file_captures(fixture.now + 3 * HOUR_MS)
            .unwrap()
            .retired,
        1
    );
}

#[test]
fn a_subtree_that_cannot_be_listed_makes_the_inventory_incomplete() {
    if rustix::process::geteuid().is_root() {
        // Root reads any directory; there is no listing failure to provoke.
        return;
    }
    let fixture = Fixture::open();
    fixture.write("ok.txt", b"bun");
    fixture.write("locked/hidden.txt", b"bun inside");
    let locked = fixture.project.path().join("locked");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Local);
    let outcome = text.search(
        &fixture.store,
        &mut broker,
        SearchQuery::Content("bun"),
        fixture.now,
    );
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    let outcome = outcome.unwrap();
    assert_eq!(outcome.hits.len(), 1);
    assert!(outcome.withheld);
    assert_eq!(
        outcome.completeness,
        Completeness::CandidateBound,
        "an unlisted subtree is incompleteness, not absence"
    );
}

#[test]
fn a_broker_of_another_run_of_the_same_project_is_refused() {
    let fixture = Fixture::open();
    fixture.write("a.txt", b"bun");
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut other = fixture.broker_of_run("job-2");
    let tip = fixture.store.tip().unwrap();
    assert_eq!(
        text.read(&fixture.store, &mut other, "a.txt", None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::Scope
    );
    assert_eq!(fixture.store.tip().unwrap(), tip, "nothing was captured");
}

#[test]
fn the_same_job_under_another_memstore_incarnation_is_another_capture() {
    let fixture = Fixture::open();
    fixture.write("a.txt", b"bytes captured across a memstore recreation");
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Local);
    text.read(&fixture.store, &mut broker, "a.txt", None, fixture.now)
        .unwrap();
    let (mut other_broker, other_binding) = fixture.run_of_another_memstore();
    let mut other_text =
        ProjectText::open(fixture.project.path(), &protected, other_binding).unwrap();
    other_text
        .read(
            &fixture.store,
            &mut other_broker,
            "a.txt",
            None,
            fixture.now,
        )
        .unwrap();
    assert_eq!(
        fixture.capture_rows().len(),
        2,
        "a recreated MemoryStore's job does not replay the old incarnation's capture"
    );
}

#[test]
fn the_capture_writer_binds_the_detail_to_the_evidence_registry_row() {
    let fixture = Fixture::open();
    let body = b"bytes under a registry row that is not a local-file evidence object";
    let digest = format!("{:x}", Sha256::digest(body));
    let byte_length = u64::try_from(body.len()).unwrap();
    let ingest = |evidence_id: &str, object_kind: &str, source_kind: &str, source_id: &str| {
        fixture
            .store
            .ingest_exact_artifact(ArtifactIngestRequest {
                intent: intent(&format!("ingest-{evidence_id}")),
                payload: body.to_vec(),
                evidence_id: evidence_id.to_string(),
                object_id: format!("{evidence_id}-object"),
                object_kind: object_kind.to_string(),
                domain_id: DOMAIN.to_string(),
                source_kind: source_kind.to_string(),
                source_id: source_id.to_string(),
                source_revision: 1,
                media_type: "text/plain".to_string(),
                retention_class: MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS.to_string(),
                retain_until: Some(fixture.now + HOUR_MS),
                asserted_sensitivity: Sensitivity::Sensitive,
                provider_egress: ProviderEgress::LocalOnly,
                provenance: None,
            })
            .unwrap();
    };
    ingest(
        "artifact-kind",
        "artifact",
        kernel::LOCAL_FILE_SOURCE_KIND,
        "src/main.rs",
    );
    ingest("other-source", "evidence", "conversation", "src/main.rs");
    ingest(
        "other-path",
        "evidence",
        kernel::LOCAL_FILE_SOURCE_KIND,
        "src/other.rs",
    );
    ingest(
        "well-formed",
        "evidence",
        kernel::LOCAL_FILE_SOURCE_KIND,
        "src/main.rs",
    );
    let record = |key: &str, evidence_id: &str| {
        fixture.store.commit(intent(key), |envelope| {
            envelope.record_local_file_capture(&kernel::LocalFileCaptureRequest {
                project_digest: PROJECT,
                relative_path: "src/main.rs",
                captured_at: fixture.now,
                domain_id: DOMAIN,
                scope_id: None,
                evidence_id,
                artifact_digest: &digest,
                byte_length,
            })?;
            Ok(String::new())
        })
    };
    for evidence_id in ["artifact-kind", "other-source", "other-path"] {
        assert_eq!(
            record(&format!("record-{evidence_id}"), evidence_id),
            Err(kernel::KernelError::NotFound),
            "{evidence_id}"
        );
    }
    record("record-well-formed", "well-formed").unwrap();
}

#[test]
fn a_capture_refused_by_the_hold_leaves_no_rows_behind() {
    let fixture = Fixture::open();
    fixture.write("a.txt", b"bytes the hold has no room for");
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker_with_references(
        PROJECT,
        ArtifactDestination::Local,
        kernel::MAX_MEMORY_REVIEWER_HOLD_REFERENCES,
    );
    assert_eq!(
        text.read(&fixture.store, &mut broker, "a.txt", None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::HoldLimit
    );
    assert!(
        fixture.capture_rows().is_empty(),
        "a capture the hold cannot carry is not left live until expiry"
    );
    assert!(broker.aliases.is_empty());
}

#[test]
fn refused_reads_of_a_cached_capture_issue_no_aliases() {
    let fixture = Fixture::open();
    fixture.write("a.txt", b"read once, then refused");
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture
        .broker(ArtifactDestination::Local)
        .with_inspection_limit(1);
    text.read(&fixture.store, &mut broker, "a.txt", None, fixture.now)
        .unwrap();
    let issued = broker.aliases.len();
    for _ in 0..3 {
        broker.accounting.end_batch();
        assert_eq!(
            text.read(&fixture.store, &mut broker, "a.txt", None, fixture.now)
                .unwrap_err()
                .code,
            RefusalCode::InspectionLimit
        );
    }
    assert_eq!(
        broker.aliases.len(),
        issued,
        "a read the inspection ceiling refuses issues no alias"
    );
}

#[test]
fn an_inspection_ceiling_refusal_on_a_capture_marks_the_evidence_set_partial() {
    let fixture = Fixture::open();
    fixture.write("a.txt", b"bun one");
    fixture.write("b.txt", b"bun two");
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture
        .broker(ArtifactDestination::Local)
        .with_inspection_limit(1);
    let outcome = text
        .search(
            &fixture.store,
            &mut broker,
            SearchQuery::Content("bun"),
            fixture.now,
        )
        .unwrap();
    assert_eq!(outcome.completeness, Completeness::CapacityBound);
    assert_eq!(outcome.hits.len(), 1);
    assert!(
        !broker.ledger.conclusions_usable(),
        "the inspection ceiling truncates the evidence set whether a read or a capture hits it"
    );
    // A single read refused by the ceiling marks the set the same way.
    let mut text = fixture.text(&protected);
    let mut broker = fixture
        .broker(ArtifactDestination::Local)
        .with_inspection_limit(0);
    assert_eq!(
        text.read(&fixture.store, &mut broker, "a.txt", None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::InspectionLimit
    );
    assert!(!broker.ledger.conclusions_usable());
}

#[test]
fn a_capture_whose_detail_cannot_be_recorded_leaves_no_evidence_behind() {
    let fixture = Fixture::open();
    fixture.write("a.txt", b"bytes whose detail the store refuses");
    let protected = fixture.protected();
    // A scope the store does not know: the detail's observation cannot be written.
    let mut binding = fixture.binding();
    binding.scope_id = Some("scope-that-does-not-exist".to_string());
    let mut text = ProjectText::open(fixture.project.path(), &protected, binding).unwrap();
    let mut broker = fixture.broker(ArtifactDestination::Local);
    assert_eq!(
        text.read(&fixture.store, &mut broker, "a.txt", None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::Store
    );
    assert!(
        fixture.capture_rows().is_empty(),
        "evidence without its detail is not left live"
    );
    assert!(broker.aliases.is_empty());
    // The hold carries no reference for it either: a later capture has the whole hold to use.
    fixture.write("b.txt", b"a capture that must still fit");
    let mut sound = fixture.text(&protected);
    sound
        .read(&fixture.store, &mut broker, "b.txt", None, fixture.now)
        .unwrap();
    fixture
        .store
        .validate_held_evidence(
            broker.hold_id(),
            MemoryReviewerHoldKind::Execution,
            &fixture.hold_binding(),
            &[],
            fixture.now,
        )
        .unwrap();
    assert_eq!(
        fixture.hold_references(broker.hold_id()),
        2,
        "the anchor and the sound capture; nothing for the abandoned one"
    );
}

/// The capture identity `capture` derives for `relative` holding `body` under the fixture's run.
fn predicted_capture_id(fixture: &Fixture, relative: &str, body: &[u8]) -> String {
    kernel::local_file_capture_id(
        &fixture.hold_binding(),
        &format!("{:x}", Sha256::digest(body)),
        relative,
    )
}

#[test]
fn a_seated_detail_that_does_not_describe_this_capture_is_not_reused() {
    let fixture = Fixture::open();
    let body = b"bytes whose detail someone else recorded first";
    fixture.write("a.txt", body);
    let evidence_id = predicted_capture_id(&fixture, "a.txt", body);
    let digest = format!("{:x}", Sha256::digest(body));
    // A caller ingests the predictable capture row under the capture's own ingest intent, so the run's ingest replays it, and records a detail naming another project.
    fixture
        .store
        .ingest_exact_artifact(ArtifactIngestRequest {
            intent: CommitIntent {
                producer: "memory_reviewer".to_string(),
                operation_key: evidence_id.clone(),
                request_digest: digest.clone(),
                actor: "memory_reviewer".to_string(),
                cause: "project_text_capture".to_string(),
            },
            payload: body.to_vec(),
            evidence_id: evidence_id.clone(),
            object_id: format!("curcapobj:{evidence_id}"),
            object_kind: "evidence".to_string(),
            domain_id: DOMAIN.to_string(),
            source_kind: kernel::LOCAL_FILE_SOURCE_KIND.to_string(),
            source_id: "a.txt".to_string(),
            source_revision: 1,
            media_type: "text/plain; charset=utf-8".to_string(),
            retention_class: MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS.to_string(),
            retain_until: Some(fixture.now + HOUR_MS),
            asserted_sensitivity: Sensitivity::Sensitive,
            provider_egress: ProviderEgress::LocalOnly,
            provenance: None,
        })
        .unwrap();
    fixture
        .store
        .commit(intent("seat-detail"), |envelope| {
            envelope.record_local_file_capture(&kernel::LocalFileCaptureRequest {
                project_digest: &"b".repeat(64),
                relative_path: "a.txt",
                captured_at: 1,
                domain_id: DOMAIN,
                scope_id: None,
                evidence_id: &evidence_id,
                artifact_digest: &digest,
                byte_length: u64::try_from(body.len()).unwrap(),
            })?;
            Ok(String::new())
        })
        .unwrap();
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Local);
    assert_eq!(
        text.read(&fixture.store, &mut broker, "a.txt", None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::Store,
        "a detail that names another project is not this run's provenance"
    );
    assert_eq!(broker.ledger.disclosed().count(), 0);
}

#[test]
fn a_seated_receipt_cannot_keep_a_refused_capture_alive() {
    let fixture = Fixture::open();
    let body = b"bytes the hold has no room for";
    fixture.write("a.txt", body);
    let evidence_id = predicted_capture_id(&fixture, "a.txt", body);
    // The abandonment commit runs under the kernel's reserved producer, so a caller cannot seat a receipt in its place; the attempt is refused outright.
    assert_eq!(
        fixture.store.commit(
            CommitIntent {
                producer: format!(
                    "{}local-file-abandon",
                    CommitIntent::RESERVED_PRODUCER_PREFIX
                ),
                operation_key: evidence_id.clone(),
                request_digest: format!("{:x}", Sha256::digest(evidence_id.as_bytes())),
                actor: "local-file-abandon".to_string(),
                cause: "capture abandoned by its run".to_string(),
            },
            |_| Ok("abandoned".to_string()),
        ),
        Err(kernel::KernelError::InvalidInput),
        "the abandonment producer is reserved to the store"
    );
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker_with_references(
        PROJECT,
        ArtifactDestination::Local,
        kernel::MAX_MEMORY_REVIEWER_HOLD_REFERENCES,
    );
    assert_eq!(
        text.read(&fixture.store, &mut broker, "a.txt", None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::HoldLimit
    );
    assert!(
        fixture.capture_rows().is_empty(),
        "the refused capture is abandoned through the store"
    );
}

#[test]
fn a_run_whose_hold_has_ended_may_not_inspect() {
    let fixture = Fixture::open();
    fixture.write("a.txt", b"bun");
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Local);
    fixture
        .store
        .release_execution_hold(broker.hold_id(), &fixture.hold_binding())
        .unwrap();
    assert_eq!(
        text.search(
            &fixture.store,
            &mut broker,
            SearchQuery::Content("zzz"),
            fixture.now
        )
        .unwrap_err()
        .code,
        RefusalCode::HoldInvalid,
        "a search under an ended hold is refused before any file is read"
    );
    assert_eq!(
        text.read(&fixture.store, &mut broker, "a.txt", None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::HoldInvalid
    );
}

#[test]
fn only_the_run_that_owns_a_live_hold_may_abandon_its_own_capture() {
    let fixture = Fixture::open();
    let body = b"a capture only its run may abandon";
    fixture.write("a.txt", body);
    let digest = format!("{:x}", Sha256::digest(body));
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Local);
    text.read(&fixture.store, &mut broker, "a.txt", None, fixture.now)
        .unwrap();
    // Another run's binding derives another identity: nothing of this run's is found.
    let other = fixture.broker_of_run("job-2");
    assert_eq!(
        fixture.store.abandon_local_file_capture(
            other.hold_id(),
            &other.binding().hold,
            &digest,
            "a.txt"
        ),
        Err(kernel::KernelError::NotFound)
    );
    // This run's identity under a hold it does not own, or one that has ended, is refused.
    assert_eq!(
        fixture.store.abandon_local_file_capture(
            other.hold_id(),
            &fixture.hold_binding(),
            &digest,
            "a.txt"
        ),
        Err(kernel::KernelError::Conflict)
    );
    // The owning run cannot abandon what its hold still pins.
    assert_eq!(
        fixture.store.abandon_local_file_capture(
            broker.hold_id(),
            &fixture.hold_binding(),
            &digest,
            "a.txt"
        ),
        Err(kernel::KernelError::Conflict)
    );
    fixture
        .store
        .release_execution_hold(broker.hold_id(), &fixture.hold_binding())
        .unwrap();
    assert_eq!(
        fixture.store.abandon_local_file_capture(
            broker.hold_id(),
            &fixture.hold_binding(),
            &digest,
            "a.txt"
        ),
        Err(kernel::KernelError::Conflict),
        "a released hold proves nothing"
    );
    assert_eq!(fixture.capture_rows().len(), 1, "the capture is untouched");
}

/// Seats the capture row the fixture's run would create for `a.txt` holding `body`, under the run's own ingest intent so the run's ingest replays it, with a detail that matches the capture in every field but is registered in `domain` under `scope` with acquisition reference `retain_until`.
fn seat_matching_capture(
    fixture: &Fixture,
    body: &[u8],
    domain: &str,
    scope: Option<&str>,
    retain_until: i64,
) -> String {
    let evidence_id = predicted_capture_id(fixture, "a.txt", body);
    let digest = format!("{:x}", Sha256::digest(body));
    fixture
        .store
        .ingest_exact_artifact(ArtifactIngestRequest {
            intent: CommitIntent {
                producer: "memory_reviewer".to_string(),
                operation_key: evidence_id.clone(),
                request_digest: digest.clone(),
                actor: "memory_reviewer".to_string(),
                cause: "project_text_capture".to_string(),
            },
            payload: body.to_vec(),
            evidence_id: evidence_id.clone(),
            object_id: format!("curcapobj:{evidence_id}"),
            object_kind: "evidence".to_string(),
            domain_id: domain.to_string(),
            source_kind: kernel::LOCAL_FILE_SOURCE_KIND.to_string(),
            source_id: "a.txt".to_string(),
            source_revision: 1,
            media_type: "text/plain; charset=utf-8".to_string(),
            retention_class: MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS.to_string(),
            retain_until: Some(retain_until),
            asserted_sensitivity: Sensitivity::Sensitive,
            provider_egress: ProviderEgress::LocalOnly,
            provenance: None,
        })
        .unwrap();
    fixture
        .store
        .commit(intent("seat-detail"), |envelope| {
            envelope.record_local_file_capture(&kernel::LocalFileCaptureRequest {
                project_digest: PROJECT,
                relative_path: "a.txt",
                captured_at: 1,
                domain_id: domain,
                scope_id: scope,
                evidence_id: &evidence_id,
                artifact_digest: &digest,
                byte_length: u64::try_from(body.len()).unwrap(),
            })?;
            Ok(String::new())
        })
        .unwrap();
    evidence_id
}

#[test]
fn a_reused_capture_may_not_outlive_the_inspections_reference() {
    let fixture = Fixture::open();
    let body = b"bytes whose row was given a later deadline";
    fixture.write("a.txt", body);
    // A caller seats the capture's row and a matching detail, with an acquisition reference well past the inspection's.
    seat_matching_capture(&fixture, body, DOMAIN, None, fixture.now + 10 * HOUR_MS);
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Local);
    assert_eq!(
        text.read(&fixture.store, &mut broker, "a.txt", None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::Store,
        "a row that would outlive the inspection's reference is not this run's capture"
    );
    assert_eq!(broker.ledger.disclosed().count(), 0);
    assert_eq!(
        fixture.hold_references(broker.hold_id()),
        1,
        "the refused row was never pinned; only the anchor is held"
    );
}

#[test]
fn a_capture_is_dated_no_earlier_than_the_wall_clock() {
    let fixture = Fixture::open();
    fixture.write("a.txt", b"bytes read now, whatever the run clock says");
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Local);
    let before = now_ms();
    // A run clock far in the past: the hold check clamps it to the wall clock, and so must the capture time.
    text.read(&fixture.store, &mut broker, "a.txt", None, 1)
        .unwrap();
    let evidence_id = fixture.capture_rows()[0].0.clone();
    let detail = fixture
        .store
        .local_file_capture(&evidence_id)
        .unwrap()
        .unwrap()
        .detail;
    assert!(
        detail.captured_at >= before,
        "captured_at {} predates the read at {before}",
        detail.captured_at
    );
}

#[test]
fn an_invalid_range_is_refused_before_anything_is_captured() {
    let fixture = Fixture::open();
    fixture.write("a.txt", b"twelve bytes");
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Local);
    let tip = fixture.store.tip().unwrap();
    // Reversed, empty, past the end, and starting at the end.
    let reversed = std::ops::Range { start: 5, end: 3 };
    for range in [reversed, 3..3, 0..13, 12..13] {
        assert_eq!(
            text.read(
                &fixture.store,
                &mut broker,
                "a.txt",
                Some(range.clone()),
                fixture.now
            )
            .unwrap_err()
            .code,
            RefusalCode::InvalidRange,
            "{range:?}"
        );
    }
    assert_eq!(fixture.store.tip().unwrap(), tip, "nothing was captured");
    assert!(fixture.capture_rows().is_empty());
    assert!(broker.aliases.is_empty());
    assert_eq!(broker.accounting.issued_inspections(), 0);
    // A valid range still discloses.
    let read = text
        .read(
            &fixture.store,
            &mut broker,
            "a.txt",
            Some(0..6),
            fixture.now,
        )
        .unwrap();
    assert_eq!(read.buffer.bytes(), b"twelve");
}

#[test]
fn a_reused_capture_must_belong_to_the_inspections_domain_and_scope() {
    let fixture = Fixture::open();
    fixture
        .store
        .commit(intent("other-domain"), |envelope| {
            envelope.insert_domain(DomainSpec {
                domain_id: "other-domain".to_string(),
                object_id: "other-domain-object".to_string(),
                name: "other".to_string(),
                source_kind: "fixture".to_string(),
                source_id: "other-domain".to_string(),
                sensitivity: Sensitivity::Normal,
                source_revision: 1,
            })?;
            Ok(String::new())
        })
        .unwrap();
    let body = b"bytes registered under another domain";
    fixture.write("a.txt", body);
    // A matching row and detail, registered in another domain.
    seat_matching_capture(&fixture, body, "other-domain", None, fixture.now + HOUR_MS);
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Local);
    assert_eq!(
        text.read(&fixture.store, &mut broker, "a.txt", None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::Store,
        "a capture registered in another domain is not this inspection's"
    );
    assert_eq!(broker.ledger.disclosed().count(), 0);
    assert_eq!(fixture.hold_references(broker.hold_id()), 1);
}

#[test]
fn a_reused_capture_whose_reference_has_lapsed_is_not_pinned() {
    let fixture = Fixture::open();
    let body = b"bytes whose reference already lapsed";
    fixture.write("a.txt", body);
    // A matching row whose acquisition reference is earlier than the inspection's, and already in the past at the read.
    let lapsed = now_ms() + 400;
    seat_matching_capture(&fixture, body, DOMAIN, None, lapsed);
    while now_ms() <= lapsed {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Local);
    assert_eq!(
        text.read(&fixture.store, &mut broker, "a.txt", None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::Store,
        "a lapsed reference cannot be reused"
    );
    assert_eq!(
        fixture.hold_references(broker.hold_id()),
        1,
        "the lapsed row was never pinned"
    );
}
