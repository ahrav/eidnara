//! Explicitly selected commits reach the kernel as `git_commits` descriptors keyed by repository, object format, and object id, with exact message bytes that survive the repository's removal; refused selections publish nothing; replay writes nothing twice.

use std::collections::BTreeSet;
use std::num::{NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use daemon::git_sources::{
    COMMIT_ROLE, GIT_COMMIT_REVISION, GitDisposition, GitReadBounds, GitRefusal, RepositoryBinding,
    read_selection,
};
use daemon::harness_sources::{
    GIT_SOURCE_POLICY_VERSION, PublishError, Representation, SourcePublisher,
};
use daemon::search_projection::SearchProjection;
use gix::bstr::BString;
use kernel::source_identity::{Occurrence, encode_preserving_span};
use kernel::{
    ArtifactErrorKind, CommitIntent, Dimension, DomainSpec, ExportWindow, KernelStore,
    ProviderEgress, ScopeSpec, ScopeTermSpec, Sensitivity, SourceClass, SourceDescriptorPolicy,
    SourceHold, SourceHoldAdmission, SourceHoldBinding, SourceHoldBounds, SourcePageBounds,
    SourceRow, TaintClass,
};
use retrieval::batch::{
    BatchBounds, MutationIdentity, VectorGeneration, batch_from_rows, register_generation,
    row_identities,
};
use retrieval::{PersistBounds, ProjectionIdentity, install_identity};
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};

const CONSUMER: &str = "search";
const POLICY: &str = "source-policy.v1";
const PROJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SCOPE: &str = "project:a";
const DOMAIN: &str = "code";
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const MODEL: &str = "tiny-test-model";
const FINGERPRINT: &str = "a2b4c6d8e0f01234a2b4c6d8e0f01234a2b4c6d8e0f01234a2b4c6d8e0f01234";
const GENERATION: &str = "gen-1";
const NOW: i64 = 1_000;
/// A credential shape the bounded scanner detects.
const SECRET: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyzABCDEFGH12345678";
/// A message with a trailing newline, CRLF, tabs, multibyte text, and an emoji, retained exactly.
const MESSAGE: &str = "Fix the thing\r\n\n\tBody line with naïve 日本語 🎉\n";

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "daemon-git-sources-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

fn bounds() -> GitReadBounds {
    GitReadBounds {
        max_commits: NonZeroUsize::new(8).unwrap(),
        max_object_bytes: NonZeroU64::new(4096).unwrap(),
        max_total_object_bytes: NonZeroU64::new(1 << 16).unwrap(),
    }
}

fn batch_bounds() -> BatchBounds {
    BatchBounds {
        persist: PersistBounds {
            max_records: NonZeroUsize::new(64).unwrap(),
            max_payload_bytes: NonZeroUsize::new(1 << 16).unwrap(),
            max_tuple_bytes: NonZeroUsize::new(2048).unwrap(),
        },
        max_source_bytes: NonZeroUsize::new(1 << 16).unwrap(),
        max_local_mutations: NonZeroUsize::new(64).unwrap(),
        max_pending: NonZeroUsize::new(64).unwrap(),
    }
}

fn signature(seconds: i64) -> gix::actor::Signature {
    gix::actor::Signature {
        name: "fixture".into(),
        email: "fixture@example.com".into(),
        time: gix::date::Time::new(seconds, 0),
    }
}

/// A repository built with gix write APIs; no git CLI.
struct Repo {
    root: PathBuf,
    repo: gix::Repository,
}

impl Repo {
    fn init(root: &Path) -> Self {
        std::fs::create_dir_all(root).unwrap();
        gix::init(root).unwrap();
        let repo = gix::open_opts(root, gix::open::Options::isolated()).unwrap();
        Self {
            root: root.to_path_buf(),
            repo,
        }
    }

    fn tree(&self) -> gix::ObjectId {
        self.repo
            .write_object(gix::objs::Tree::empty())
            .unwrap()
            .detach()
    }

    /// A commit with `message`, its id determined by the message and `seconds`. Written as an object, not through a ref, since identity here is the object id alone.
    fn commit(&self, message: &str, seconds: i64) -> String {
        self.write(message.as_bytes(), None, seconds)
    }

    /// A commit object written raw, so its message bytes and encoding header are exactly `message` and `encoding`.
    fn raw_commit(&self, message: &[u8], encoding: Option<&str>) -> String {
        self.write(message, encoding, 1)
    }

    fn write(&self, message: &[u8], encoding: Option<&str>, seconds: i64) -> String {
        let signature = signature(seconds);
        let commit = gix::objs::Commit {
            tree: self.tree(),
            parents: Default::default(),
            author: signature.clone(),
            committer: signature,
            encoding: encoding.map(BString::from),
            message: BString::from(message),
            extra_headers: Vec::new(),
        };
        self.repo
            .write_object(&commit)
            .unwrap()
            .detach()
            .to_string()
    }

    fn binding(&self, repository_id: &str) -> RepositoryBinding {
        RepositoryBinding {
            repository_id: repository_id.to_string(),
            path: self.root.clone(),
        }
    }
}

struct Corpus {
    kernel: Arc<KernelStore>,
    root: PathBuf,
}

impl Corpus {
    fn open(root: &Path) -> Self {
        Self {
            kernel: Arc::new(KernelStore::open(root.join("kernel")).unwrap()),
            root: root.to_path_buf(),
        }
    }

    fn kernel_db(&self) -> Connection {
        Connection::open_with_flags(
            self.root.join("kernel/kernel.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap()
    }

    fn kernel_incarnation_id(&self) -> String {
        self.kernel_db()
            .query_row(
                "SELECT database_incarnation_id FROM kernel_format_marker WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn seed(&self) {
        self.kernel
            .commit(intent("seed"), |envelope| {
                envelope.insert_domain(DomainSpec {
                    domain_id: DOMAIN.to_string(),
                    object_id: format!("{DOMAIN}-object"),
                    name: "Code".to_string(),
                    source_kind: "fixture".to_string(),
                    source_id: DOMAIN.to_string(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                })?;
                envelope.insert_scope(ScopeSpec {
                    scope_id: SCOPE.to_string(),
                    object_id: SCOPE.to_string(),
                    source_id: SCOPE.to_string(),
                    domain_id: DOMAIN.to_string(),
                    source_kind: "kernel_route".to_string(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                    terms: vec![ScopeTermSpec {
                        dimension: Dimension::Project.as_str().to_string(),
                        operator: "exact".to_string(),
                        exact_value: Some(PROJECT.to_string()),
                        ..ScopeTermSpec::default()
                    }],
                })?;
                envelope.register_outbox_consumer(CONSUMER, 1)?;
                Ok(String::new())
            })
            .unwrap();
    }

    fn publisher(&self) -> SourcePublisher<'_> {
        SourcePublisher {
            kernel: &self.kernel,
            domain_id: DOMAIN,
            scope_id: Some(SCOPE),
            egress: ProviderEgress::LocalOnly,
            sensitivity: Sensitivity::Normal,
        }
    }

    fn binding(&self) -> SourceHoldBinding {
        SourceHoldBinding {
            consumer_id: CONSUMER.to_string(),
            lease_epoch: self.kernel.lease_epoch(),
            source_policy_version: POLICY.to_string(),
        }
    }

    fn capture(&self) -> SourceHold {
        self.kernel
            .capture_source_hold(
                &self.binding(),
                SourceHoldBounds {
                    max_descriptor_rows: NonZeroUsize::new(256).unwrap(),
                    admission: SourceHoldAdmission {
                        max_references: NonZeroUsize::new(64).unwrap(),
                        max_encoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
                    },
                    expiry_ms: NonZeroU64::new((20 * DAY_MS) as u64).unwrap(),
                },
            )
            .unwrap()
    }

    /// Every descriptor live at `hold`'s S, with its text.
    fn export(&self, hold: &SourceHold) -> Vec<SourceRow> {
        let binding = self.binding();
        let mut rows = Vec::new();
        let mut cursor = None;
        loop {
            let page = self
                .kernel
                .export_source_page(
                    &binding,
                    &hold.hold_id,
                    hold.captured_at,
                    ExportWindow::Snapshot,
                    cursor.as_ref(),
                    SourcePageBounds {
                        max_rows: NonZeroUsize::new(64).unwrap(),
                        max_encoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
                        max_decoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
                        max_row_bytes: NonZeroU64::new(1 << 16).unwrap(),
                    },
                )
                .unwrap();
            rows.extend(page.rows);
            match page.next {
                Some(next) => cursor = Some(next),
                None => return rows,
            }
        }
    }

    /// The live inventory as `(repository_id, object_format, oid, revision, representation, text, payload_id, policy)`.
    fn inventory(&self) -> BTreeSet<Ledger> {
        let hold = self.capture();
        let rows = self.export(&hold);
        self.kernel
            .release_source_hold(&self.binding(), &hold.hold_id, hold.captured_at)
            .unwrap();
        rows.into_iter()
            .filter(|row| row.invalidated_commit_seq.is_none())
            .map(|row| {
                assert_eq!(row.detail.class, "git_commits");
                let field = |name: &str| {
                    row.detail
                        .identity
                        .iter()
                        .find(|(field, _)| field == name)
                        .map(|(_, value)| value.clone())
                        .unwrap()
                };
                Ledger {
                    repository_id: field("repository_id"),
                    object_format: field("object_format"),
                    oid: field("oid"),
                    revision: row.revision,
                    representation: row.detail.representation.clone(),
                    text: row.text.clone().unwrap(),
                    payload_id: row.detail.payload_id.clone(),
                    policy: match &row.detail.source_policy {
                        SourceDescriptorPolicy::Git { version } => version.clone(),
                        other => panic!("a git descriptor under {other:?}"),
                    },
                    sensitivity: row.sensitivity,
                }
            })
            .collect()
    }

    /// Builds a projection at a fresh S from the kernel alone and returns its live rows.
    fn rebuild(&self, data_home: &Path) -> BTreeSet<(String, String, String, i64)> {
        let hold = self.capture();
        let rows = self.export(&hold);
        let projection = SearchProjection::open(data_home).unwrap();
        let kernel_incarnation_id = self.kernel_incarnation_id();
        projection
            .write(|conn| {
                install_identity(
                    conn,
                    &ProjectionIdentity {
                        schema_version: retrieval::SCHEMA_VERSION,
                        kernel_incarnation_id: kernel_incarnation_id.clone(),
                        projection_policy_version: POLICY.to_string(),
                        identity_contract_version: "search-projection-identity-v2".to_string(),
                        limit_manifest_protocol_version: "limits.v1".to_string(),
                        embedding_model: MODEL.to_string(),
                        tokenizer_fingerprint: FINGERPRINT.to_string(),
                        vector_dimension: 8,
                        generation_epoch: 1,
                    },
                    1,
                )?;
                register_generation(
                    conn,
                    &VectorGeneration {
                        generation_id: GENERATION.to_string(),
                        embedding_model: MODEL.to_string(),
                        tokenizer_fingerprint: FINGERPRINT.to_string(),
                        vector_dimension: 8,
                        generation_epoch: 1,
                    },
                    1,
                )?;
                Ok(())
            })
            .unwrap();
        let identities = row_identities(&rows);
        let batch = batch_from_rows(
            &rows,
            &identities,
            MutationIdentity {
                kernel_incarnation_id,
                hold_id: hold.hold_id.clone(),
                snapshot_commit_seq: hold.snapshot,
                through_commit_seq: hold.snapshot,
            },
            Some(GENERATION),
        )
        .unwrap();
        projection.apply_batch(&batch, batch_bounds(), 2).unwrap();
        // Applying the same window again is a replay: no new row and no second job.
        projection.apply_batch(&batch, batch_bounds(), 3).unwrap();
        drop(projection);
        self.kernel
            .release_source_hold(&self.binding(), &hold.hold_id, hold.captured_at)
            .unwrap();
        Connection::open_with_flags(
            data_home.join("search").join("search.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap()
        .prepare(
            "SELECT o.occurrence_id,o.payload_id,CAST(p.bytes AS TEXT),
                    (SELECT COUNT(*) FROM embedding_jobs j WHERE j.occurrence_id=o.occurrence_id)
             FROM occurrences o JOIN payloads p ON p.payload_id=o.payload_id
             LEFT JOIN occurrence_tombstones t ON t.occurrence_id=o.occurrence_id
             WHERE t.occurrence_id IS NULL",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
    }
}

/// One occurrence as the independent ledger predicts it from the repository, the object, and the message bytes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Ledger {
    repository_id: String,
    object_format: String,
    oid: String,
    revision: i64,
    representation: String,
    text: String,
    payload_id: String,
    /// The git policy version the descriptor was published under.
    policy: String,
    sensitivity: Sensitivity,
}

impl Ledger {
    fn new(repository_id: &str, oid: &str, message: &str) -> Self {
        Self {
            repository_id: repository_id.to_string(),
            object_format: "sha1".to_string(),
            oid: oid.to_string(),
            revision: 1,
            representation: "commit_message".to_string(),
            text: message.to_string(),
            payload_id: format!("{:x}", Sha256::digest(message.as_bytes())),
            policy: GIT_SOURCE_POLICY_VERSION.to_string(),
            sensitivity: Sensitivity::Normal,
        }
    }

    fn occurrence_id(&self) -> String {
        encode_preserving_span(&Occurrence {
            class: "git_commits",
            identity: &[
                ("repository_id", &self.repository_id),
                ("object_format", &self.object_format),
                ("oid", &self.oid),
            ],
            revision: "1",
            representation: "commit_message",
            span: None,
        })
        .unwrap()
        .occurrence_id
    }
}

/// AC1, AC2, AC4, AC5: two repositories with an equal commit and one repeated message publish distinct occurrences over shared payload bytes; the ledger predicts every tuple, byte, policy, and admission class; a replayed selection writes nothing; after the repositories are deleted the kernel alone rebuilds identical projection rows with exactly one job each; the payload is the message bytes and not an author-and-message concatenation.
#[test]
fn selected_commits_are_retained_exactly_and_rebuild_without_the_repository() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let alpha = Repo::init(&dir.path().join("alpha"));
    let beta = Repo::init(&dir.path().join("beta"));
    let a1 = alpha.commit(MESSAGE, 1);
    let a2 = alpha.commit(MESSAGE, 2);
    let a3 = alpha.commit("Second subject\n", 3);
    let b1 = beta.commit(MESSAGE, 1);
    assert_eq!(
        a1, b1,
        "equal trees, signatures, and messages give equal ids across repositories"
    );
    assert_ne!(
        a1, a2,
        "a repeated message at another time is another commit"
    );

    let alpha_selection = read_selection(
        &alpha.binding("repo-alpha"),
        &[a1.clone(), a2.clone(), a3.clone()],
        bounds(),
    )
    .unwrap();
    let beta_selection = read_selection(
        &beta.binding("repo-beta"),
        std::slice::from_ref(&b1),
        bounds(),
    )
    .unwrap();
    assert!(alpha_selection.dispositions.is_empty() && beta_selection.dispositions.is_empty());
    assert_eq!(alpha_selection.units.len(), 3);
    for unit in alpha_selection.units.iter().chain(&beta_selection.units) {
        assert_eq!(unit.revision, GIT_COMMIT_REVISION);
        assert_eq!(unit.representation, Representation::CommitMessage);
        assert_eq!(unit.role, COMMIT_ROLE);
        assert!(
            !format!("{unit:?}").contains("Body line"),
            "debug output names no message text"
        );
    }
    // Binding the same directory under another id names another repository; the path is not an identity.
    let renamed = read_selection(
        &alpha.binding("repo-gamma"),
        std::slice::from_ref(&a1),
        bounds(),
    )
    .unwrap();
    assert_ne!(renamed.units[0].identity, alpha_selection.units[0].identity);

    let expected: BTreeSet<Ledger> = [
        Ledger::new("repo-alpha", &a1, MESSAGE),
        Ledger::new("repo-alpha", &a2, MESSAGE),
        Ledger::new("repo-alpha", &a3, "Second subject\n"),
        Ledger::new("repo-beta", &b1, MESSAGE),
    ]
    .into_iter()
    .collect();
    let publisher = corpus.publisher();
    let mut first = Vec::new();
    for unit in alpha_selection.units.iter().chain(&beta_selection.units) {
        let published = publisher.publish(unit, NOW).unwrap();
        assert!(!published.replayed);
        assert_eq!(
            published.provenance,
            (
                SourceClass::UntrustedRepoText,
                TaintClass::RepoUntrustedText
            )
        );
        first.push(published.object_id);
    }
    assert_eq!(corpus.inventory(), expected);
    let tip = corpus.kernel.tip().unwrap();
    for unit in alpha_selection.units.iter().chain(&beta_selection.units) {
        assert!(publisher.publish(unit, NOW).unwrap().replayed, "{unit:?}");
    }
    assert_eq!(
        corpus.kernel.tip().unwrap(),
        tip,
        "a replayed selection commits nothing"
    );
    assert_eq!(
        expected
            .iter()
            .map(|row| row.payload_id.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        2,
        "equal bytes share one payload identity across repositories and commits"
    );
    assert_eq!(
        expected
            .iter()
            .map(Ledger::occurrence_id)
            .collect::<BTreeSet<_>>()
            .len(),
        4,
        "every commit is its own occurrence"
    );
    let concatenated = format!(
        "{:x}",
        Sha256::digest(format!("fixture <fixture@example.com>\n{MESSAGE}").as_bytes())
    );
    assert!(expected.iter().all(|row| row.payload_id != concatenated));

    // The same repository at another path is the same repository: identical units, and every publication replays.
    drop(alpha);
    std::fs::rename(dir.path().join("alpha"), dir.path().join("alpha-moved")).unwrap();
    let moved = RepositoryBinding {
        repository_id: "repo-alpha".to_string(),
        path: dir.path().join("alpha-moved"),
    };
    let reread = read_selection(&moved, &[a1.clone(), a2.clone(), a3.clone()], bounds()).unwrap();
    assert_eq!(reread.units, alpha_selection.units);
    for unit in &reread.units {
        assert!(publisher.publish(unit, NOW).unwrap().replayed);
    }
    assert_eq!(corpus.kernel.tip().unwrap(), tip);

    let before = corpus.rebuild(&dir.path().join("first"));
    assert_eq!(before.len(), 4);
    assert!(
        before.iter().all(|row| row.3 == 1),
        "exactly one job per occurrence after a replayed window"
    );
    assert_eq!(
        before
            .iter()
            .map(|row| row.0.clone())
            .collect::<BTreeSet<_>>(),
        expected.iter().map(Ledger::occurrence_id).collect()
    );
    assert!(
        before
            .iter()
            .all(|row| row.2 == MESSAGE || row.2 == "Second subject\n")
    );

    // The repositories and the derived rows are gone; the kernel's evidence and descriptors reconstruct the same rows.
    drop(beta);
    std::fs::remove_dir_all(dir.path().join("alpha-moved")).unwrap();
    std::fs::remove_dir_all(dir.path().join("beta")).unwrap();
    std::fs::remove_dir_all(dir.path().join("first")).unwrap();
    assert!(
        read_selection(
            &RepositoryBinding {
                repository_id: "repo-alpha".into(),
                path: dir.path().join("alpha")
            },
            std::slice::from_ref(&a1),
            bounds()
        )
        .is_err()
    );
    let after = corpus.rebuild(&dir.path().join("second"));
    assert_eq!(after, before);
    assert_eq!(corpus.inventory(), expected);
}

/// AC3: a missing, malformed, or non-commit object, an oversized object, an oversized selection, and a bound-exceeding total refuse the whole selection before any byte is read; a commit message carrying a credential is refused at retention with nothing retained; an undeclared non-UTF-8 message and a declared other encoding are dispositions that leave the other commits publishable.
#[test]
fn refused_selections_publish_nothing_and_encodings_are_dispositions() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let repo = Repo::init(&dir.path().join("repo"));
    let good = repo.commit("Good subject\n", 1);
    let tree = repo.tree().to_string();
    let binding = repo.binding("repo");
    let missing = "0123456789abcdef0123456789abcdef01234567".to_string();
    let other = repo.commit("Other subject\n", 2);
    let cases: Vec<(Vec<String>, GitReadBounds, GitRefusal)> = vec![
        (
            vec![good.clone(), good.clone()],
            bounds(),
            GitRefusal::DuplicateOid(good.clone()),
        ),
        (
            vec![good.clone(), missing.clone()],
            bounds(),
            GitRefusal::MissingObject(missing.clone()),
        ),
        (
            vec!["ABC".to_string()],
            bounds(),
            GitRefusal::MalformedOid("ABC".to_string()),
        ),
        (
            vec![good.to_uppercase()],
            bounds(),
            GitRefusal::MalformedOid(good.to_uppercase()),
        ),
        (
            vec![good.clone(), tree.clone()],
            bounds(),
            GitRefusal::NotACommit(tree.clone()),
        ),
        (
            vec![good.clone()],
            GitReadBounds {
                max_object_bytes: NonZeroU64::new(8).unwrap(),
                ..bounds()
            },
            GitRefusal::ObjectTooLarge {
                oid: good.clone(),
                bytes: 0,
                max: 8,
            },
        ),
        (
            vec![good.clone(), other.clone()],
            GitReadBounds {
                max_commits: NonZeroUsize::new(1).unwrap(),
                ..bounds()
            },
            GitRefusal::SelectionTooLarge { count: 2, max: 1 },
        ),
        (
            vec![good.clone(), other.clone()],
            GitReadBounds {
                max_total_object_bytes: NonZeroU64::new(300).unwrap(),
                ..bounds()
            },
            GitRefusal::TotalBytesExceeded { bytes: 0, max: 300 },
        ),
    ];
    for (oids, bounds, expected) in cases {
        let refusal = read_selection(&binding, &oids, bounds).unwrap_err();
        match (&refusal, &expected) {
            (
                GitRefusal::ObjectTooLarge { oid, bytes, max },
                GitRefusal::ObjectTooLarge {
                    oid: want_oid,
                    max: want_max,
                    ..
                },
            ) => {
                assert_eq!((oid, max), (want_oid, want_max));
                assert!(*bytes > 8);
            }
            (
                GitRefusal::TotalBytesExceeded { bytes, max },
                GitRefusal::TotalBytesExceeded { max: want_max, .. },
            ) => {
                assert_eq!(max, want_max);
                assert!(*bytes > 300);
            }
            _ => assert_eq!(refusal, expected),
        }
    }
    let unopenable = RepositoryBinding {
        repository_id: "nowhere".to_string(),
        path: dir.path().join("nowhere"),
    };
    assert_eq!(
        read_selection(&unopenable, std::slice::from_ref(&good), bounds()),
        Err(GitRefusal::Open)
    );
    // A garbled loose object is unreadable, not missing, and refuses the selection whole.
    let garbled = repo.commit("Soon garbled\n", 9);
    let loose = repo
        .root
        .join(".git/objects")
        .join(&garbled[..2])
        .join(&garbled[2..]);
    std::fs::set_permissions(&loose, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
    std::fs::write(&loose, b"not zlib").unwrap();
    assert_eq!(
        read_selection(&binding, &[good.clone(), garbled.clone()], bounds()),
        Err(GitRefusal::Unreadable(garbled))
    );

    let latin = repo.raw_commit(b"Caf\xe9 au lait\n", Some("ISO-8859-1"));
    let undeclared = repo.raw_commit(b"broken \xff byte\n", None);
    let declared_utf8 = repo.raw_commit("Declared UTF-8\n".as_bytes(), Some("UTF-8"));
    let selection = read_selection(
        &binding,
        &[
            good.clone(),
            latin.clone(),
            undeclared.clone(),
            declared_utf8.clone(),
        ],
        bounds(),
    )
    .unwrap();
    assert_eq!(
        selection
            .units
            .iter()
            .map(|unit| unit.text.clone())
            .collect::<Vec<_>>(),
        vec!["Good subject\n".to_string(), "Declared UTF-8\n".to_string()]
    );
    assert_eq!(
        selection.dispositions,
        vec![
            GitDisposition::UnsupportedEncoding {
                oid: latin,
                declared: Some("ISO-8859-1".to_string()),
            },
            GitDisposition::UnsupportedEncoding {
                oid: undeclared,
                declared: None,
            },
        ]
    );

    let secret = repo.commit(&format!("Leak\n\ntoken {SECRET}\n"), 5);
    let leaking = read_selection(&binding, std::slice::from_ref(&secret), bounds()).unwrap();
    let publisher = corpus.publisher();
    let tip = corpus.kernel.tip().unwrap();
    let error = publisher.publish(&leaking.units[0], NOW).unwrap_err();
    assert!(
        matches!(
            error,
            PublishError::Artifact(ArtifactErrorKind::ExactBytesRewritten)
        ),
        "{error:?}"
    );
    assert!(!format!("{error:?}").contains(SECRET));
    assert_eq!(
        corpus.kernel.tip().unwrap(),
        tip,
        "a refused commit retains nothing"
    );
    assert!(corpus.inventory().is_empty());
    let evidence: i64 = corpus
        .kernel_db()
        .query_row(
            "SELECT COUNT(*) FROM object_registry WHERE object_kind='evidence'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(evidence, 0);
}
