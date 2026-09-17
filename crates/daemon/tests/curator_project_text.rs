//! Real-store proofs for confined project-text inspection: captures as owned evidence with typed detail and hold charging, reuse without duplication, confinement and protected-location refusals, special files, caps, search bounds, remote refusal, and expiry that keeps independent support.

use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use daemon::curator::broker::{
    EvidenceBroker, MAX_OPERATIONS_PER_BATCH, QuestionTemplate, RefusalCode, RunBinding,
};
use daemon::curator::project_text::{
    InspectionBinding, MAX_CAPTURE_BYTES, MAX_DEPTH, MAX_SCAN_BYTES, MAX_VISITED_ENTRIES,
    ProjectText, ProtectedLocations, SearchQuery,
};
use daemon::curator::{Completeness, MAX_EXCERPT_BYTES};
use kernel::{
    ArtifactDestination, ArtifactHandle, ArtifactIngestRequest, CURATOR_CAPTURE_RETENTION_CLASS,
    CommitIntent, CuratorHoldBinding, CuratorHoldKind, DomainSpec, KernelStore, ProjectScope,
    ProviderEgress, Sensitivity,
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
        producer: "curator-project-text-test".to_string(),
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

    fn hold_binding(&self) -> CuratorHoldBinding {
        CuratorHoldBinding {
            project_digest: PROJECT.to_string(),
            kernel_incarnation: incarnation(self.store_dir.path()),
            memstore_incarnation: "m".repeat(32),
            subject: "job-1".to_string(),
            generation: 1,
        }
    }

    /// A broker with an execution hold over `anchor` for this project, disclosing to `destination`.
    fn broker(&self, destination: ArtifactDestination) -> EvidenceBroker {
        let anchor = self
            .store
            .ingest_artifact(ArtifactIngestRequest {
                intent: intent(&format!("anchor-{destination:?}")),
                payload: b"anchor".to_vec(),
                evidence_id: format!("evidence-anchor-{destination:?}"),
                object_id: format!("evidence-object-anchor-{destination:?}"),
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
        let binding = self.hold_binding();
        let hold = self
            .store
            .acquire_execution_hold(&binding, &[anchor.evidence_id], self.now + 2 * HOUR_MS)
            .unwrap();
        EvidenceBroker::new(
            RunBinding {
                project: ProjectScope::new(PROJECT).unwrap(),
                hold: binding,
                hold_id: hold.hold_id,
                destination,
            },
            QuestionTemplate::ExtractedFacts,
        )
    }

    fn binding(&self) -> InspectionBinding {
        InspectionBinding {
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
            .query_map([CURATOR_CAPTURE_RETENTION_CLASS], |row| {
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
    // The typed detail names the project, the path, the time, the digest, and the whole-file range; no Git identity is claimed for a working-tree read.
    let detail = fixture
        .store
        .local_file_capture(&evidence_id)
        .unwrap()
        .unwrap();
    assert_eq!(detail.detail_version, kernel::LOCAL_FILE_DETAIL_VERSION);
    assert_eq!(detail.project_digest, PROJECT);
    assert_eq!(detail.relative_path, "src/main.rs");
    assert_eq!(detail.captured_at, fixture.now);
    assert_eq!(detail.buffer_digest, digest);
    assert_eq!(detail.range, (0, body.len() as u64));
    // Ownership was charged to the run's execution hold at capture, before the disclosing read.
    let held = fixture
        .store
        .validate_held_evidence(
            broker.hold_id(),
            CuratorHoldKind::Execution,
            &fixture.hold_binding(),
            std::slice::from_ref(&evidence_id),
            fixture.now,
        )
        .unwrap();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].retention_class, CURATOR_CAPTURE_RETENTION_CLASS);
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
    // The same bytes under another path are one artifact: the CAS holds them once and the run reuses its capture.
    fixture.write("copy/main.rs", body.as_bytes());
    text.read(
        &fixture.store,
        &mut broker,
        "copy/main.rs",
        Some(0..4),
        fixture.now,
    )
    .unwrap();
    assert_eq!(fixture.capture_rows().len(), 1);
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
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Local);
    let tip = fixture.store.tip().unwrap();
    let matching = text
        .search(
            &fixture.store,
            &mut broker,
            SearchQuery::Content("AKIAQ7"),
            fixture.now,
        )
        .unwrap();
    let absent = text
        .search(
            &fixture.store,
            &mut broker,
            SearchQuery::Content("no such literal"),
            fixture.now,
        )
        .unwrap();
    assert!(matching.hits.is_empty() && absent.hits.is_empty());
    assert_eq!(
        matching.withheld, absent.withheld,
        "withholding does not depend on the literal"
    );
    assert_eq!(matching.completeness, absent.completeness);
    assert_eq!(fixture.store.tip().unwrap(), tip);
    assert_eq!(broker.ledger.disclosed().count(), 0);
}

#[test]
fn a_capture_is_charged_to_the_hold_before_any_disclosure() {
    let fixture = Fixture::open();
    fixture.write("a.txt", b"twelve bytes");
    let protected = fixture.protected();
    let mut text = fixture.text(&protected);
    let mut broker = fixture.broker(ArtifactDestination::Local);
    assert_eq!(
        text.read(
            &fixture.store,
            &mut broker,
            "a.txt",
            Some(0..13),
            fixture.now
        )
        .unwrap_err()
        .code,
        RefusalCode::InvalidRange
    );
    let rows = fixture.capture_rows();
    assert_eq!(rows.len(), 1);
    let held = fixture
        .store
        .validate_held_evidence(
            broker.hold_id(),
            CuratorHoldKind::Execution,
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
    assert_eq!(fixture.store.expire_local_file_captures(later).unwrap(), 0);
    // Once the hold has lapsed, expiry retires the detail and then the evidence; the canonical reference and its bytes remain.
    let after_hold = fixture.now + 3 * HOUR_MS;
    assert_eq!(
        fixture
            .store
            .expire_local_file_captures(after_hold)
            .unwrap(),
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
            .unwrap(),
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
            envelope.insert_observation(kernel::ObservationSpec {
                observation_id: "foreign-1".to_string(),
                object_id: "foreign-object-1".to_string(),
                domain_id: DOMAIN.to_string(),
                proposition_id: None,
                scope_id: None,
                anchor_id: None,
                evidence_id: Some(cited.clone()),
                observation_kind: "note".to_string(),
                payload: kernel::ObservationPayload {
                    summary: "independent support".to_string(),
                    classification: "note".to_string(),
                    detail: None,
                },
                observed_at: fixture.now,
                dependencies: Vec::new(),
                source_kind: "test".to_string(),
                source_id: "foreign".to_string(),
                source_revision: 1,
                sensitivity: Sensitivity::Sensitive,
            })?;
            Ok(String::new())
        })
        .unwrap();
    assert_eq!(
        fixture
            .store
            .expire_local_file_captures(after_hold + 2)
            .unwrap(),
        0
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
}
