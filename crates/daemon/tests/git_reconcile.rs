//! Git reconciliation retires only the retained commits a complete traversal from the permitted refs never reached: branch removal, a rewritten ref, and a verified empty selection retire exactly the ledger's excluded commits while the evidence stays retained; an unresolved ref, an unopenable or unreadable repository, an exhausted budget, and an exceeded work or inventory bound retire nothing; a repeated episode and a live projection's catch-up carry the tombstones without resurrection.

use std::collections::BTreeSet;
use std::num::{NonZeroU64, NonZeroUsize};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

mod support;

use daemon::git_reconcile::{
    GitReconciler, InventoryBounds, Probe, ReconcileBlocked, ReconcileEnd, ReconcilePhase,
    ReconcileReport, ReconcileScope,
};
use daemon::git_sources::{
    COMMIT_ROLE, GIT_COMMIT_REVISION, GitReadBounds, GitRefusal, RepositoryBinding, read_selection,
};
use daemon::harness_sources::{Published, Representation, SourcePublisher, SourceUnit};
use daemon::search_projection::SearchProjection;
use kernel::applicability::EvalBudget;
use kernel::source_identity::{Occurrence, OccurrenceClass, encode_preserving_span};
use kernel::{
    ArtifactDeletionIdentity, ArtifactDeletionKind, ArtifactDeletionRequest, CommitIntent,
    Dimension, DomainSpec, ExportWindow, KernelError, KernelStore, ProviderEgress, ScopeSpec,
    ScopeTermSpec, Sensitivity, SourceHold, SourceHoldAdmission, SourceHoldBinding,
    SourceHoldBounds, SourcePageBounds, SourceRow,
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
const REPO: &str = "repo-alpha";
const MAIN: &str = "refs/heads/main";
const FEATURE: &str = "refs/heads/feature";
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const MODEL: &str = "tiny-test-model";
const FINGERPRINT: &str = "a2b4c6d8e0f01234a2b4c6d8e0f01234a2b4c6d8e0f01234a2b4c6d8e0f01234";
const GENERATION: &str = "gen-1";
const NOW: i64 = 1_000;

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "daemon-git-reconcile-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

fn bounds() -> InventoryBounds {
    InventoryBounds {
        page_rows: NonZeroUsize::new(2).unwrap(),
        max_scanned: NonZeroUsize::new(64).unwrap(),
        max_retained: NonZeroUsize::new(16).unwrap(),
        max_commits: NonZeroUsize::new(16).unwrap(),
        max_object_bytes: NonZeroU64::new(4096).unwrap(),
    }
}

fn read_bounds() -> GitReadBounds {
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

fn hold_admission() -> SourceHoldAdmission {
    SourceHoldAdmission {
        max_references: NonZeroUsize::new(64).unwrap(),
        max_encoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
    }
}

fn unbounded() -> EvalBudget {
    EvalBudget::unbounded()
}

fn exhausted() -> EvalBudget {
    EvalBudget::new(Some(Instant::now()), Arc::new(AtomicBool::new(false)))
}

fn signature(seconds: i64) -> gix::actor::Signature {
    gix::actor::Signature {
        name: "fixture".into(),
        email: "fixture@example.com".into(),
        time: gix::date::Time::new(seconds, 0),
    }
}

/// A repository with a `main` history and a `feature` branch, written with gix APIs.
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

    /// Commits `message` on `branch` with `parents`; the branch must currently point at the first parent or not exist.
    fn commit(&self, branch: &str, parents: &[&str], message: &str, seconds: i64) -> String {
        let tree = self
            .repo
            .write_object(gix::objs::Tree::empty())
            .unwrap()
            .detach();
        let signature = signature(seconds);
        let parents: Vec<gix::ObjectId> = parents
            .iter()
            .map(|parent| gix::ObjectId::from_hex(parent.as_bytes()).unwrap())
            .collect();
        self.repo
            .commit_as(
                signature.to_ref(&mut Default::default()),
                signature.to_ref(&mut Default::default()),
                branch,
                message,
                tree,
                parents,
            )
            .unwrap()
            .detach()
            .to_string()
    }

    fn delete_ref(&self, name: &str) {
        self.repo.find_reference(name).unwrap().delete().unwrap();
    }

    /// Rewrites a loose ref in place, as a force-push or reset does.
    fn point_ref(&self, name: &str, oid: &str) {
        let path = self.root.join(".git").join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("{oid}\n")).unwrap();
    }

    /// Appends `section` to the repository's own config, which an isolated open still reads.
    fn append_config(&self, section: &str) {
        let path = self.root.join(".git/config");
        let mut config = std::fs::read_to_string(&path).unwrap();
        config.push_str(section);
        std::fs::write(path, config).unwrap();
    }

    /// Marks `oid` as a shallow boundary, as `git fetch --depth` does on an existing clone.
    fn set_shallow(&self, oid: &str) {
        std::fs::write(self.root.join(".git/shallow"), format!("{oid}\n")).unwrap();
    }

    fn clear_shallow(&self) {
        std::fs::remove_file(self.root.join(".git/shallow")).unwrap();
    }

    fn binding(&self) -> RepositoryBinding {
        RepositoryBinding {
            repository_id: REPO.to_string(),
            path: self.root.clone(),
        }
    }

    fn scope(&self, refs: &[&str]) -> ReconcileScope {
        ReconcileScope {
            binding: self.binding(),
            permitted_refs: refs.iter().map(|name| name.to_string()).collect(),
        }
    }

    /// Garbles the loose object of `oid` in place.
    fn loose_object(&self, oid: &str) -> PathBuf {
        self.root
            .join(".git/objects")
            .join(&oid[..2])
            .join(&oid[2..])
    }

    fn garble(&self, oid: &str) {
        let loose = self.loose_object(oid);
        std::fs::set_permissions(&loose, PermissionsExt::from_mode(0o644)).unwrap();
        std::fs::write(&loose, b"not zlib").unwrap();
    }

    /// Stores `source`'s bytes under `oid`, so the object named `oid` decodes as a valid commit that does not hash to its id.
    fn substitute(&self, oid: &str, source: &str) {
        let loose = self.loose_object(oid);
        std::fs::set_permissions(&loose, PermissionsExt::from_mode(0o644)).unwrap();
        std::fs::copy(self.loose_object(source), loose).unwrap();
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

    fn reopen(self) -> Self {
        let root = self.root.clone();
        drop(self);
        Self::open(&root)
    }

    fn kernel_incarnation_id(&self) -> String {
        Connection::open_with_flags(
            self.root.join("kernel/kernel.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap()
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

    /// Publishes every commit of `oids` from `repo` under `repository_id` as a retained source.
    fn publish_as(&self, repo: &Repo, repository_id: &str, oids: &[String]) -> Vec<Published> {
        let binding = RepositoryBinding {
            repository_id: repository_id.to_string(),
            path: repo.root.clone(),
        };
        let selection = read_selection(
            &support::projection_gate::open_gate(),
            &binding,
            oids,
            read_bounds(),
        )
        .unwrap();
        assert_eq!(selection.units.len(), oids.len());
        selection
            .units
            .iter()
            .map(|unit| self.publisher().publish(unit, NOW).unwrap())
            .collect()
    }

    fn publish(&self, repo: &Repo, oids: &[String]) -> Vec<Published> {
        self.publish_as(repo, REPO, oids)
    }

    fn publish_foreign_format(&self, object_format: &str, oid: &str) -> Published {
        let unit = SourceUnit {
            class: OccurrenceClass::GitCommits,
            identity: vec![
                ("repository_id", REPO.to_string()),
                ("object_format", object_format.to_string()),
                ("oid", oid.to_string()),
            ],
            revision: GIT_COMMIT_REVISION.to_string(),
            representation: Representation::CommitMessage,
            text: "foreign format\n".to_string(),
            role: COMMIT_ROLE.to_string(),
        };
        self.publisher().publish(&unit, NOW).unwrap()
    }

    /// Deletes the artifact behind `published`, which invalidates the evidence and leaves the descriptor row untouched.
    fn delete_evidence(&self, published: &Published, key: &str) {
        let evidence_id = published
            .evidence_object_id
            .strip_prefix("srcev-object:")
            .map(|key| format!("srcev:{key}"))
            .unwrap();
        self.kernel
            .delete_artifact(ArtifactDeletionRequest {
                intent: intent(key),
                identity: ArtifactDeletionIdentity::EvidenceId(evidence_id),
                kind: ArtifactDeletionKind::Delete,
                operator_id: None,
                target_locator: None,
                reason: None,
                deleted_at: NOW,
            })
            .unwrap();
    }

    fn retire_descriptor(&self, object_id: &str, key: &str) {
        self.kernel
            .commit(intent(key), |envelope| {
                envelope.retire_observation(object_id)?;
                Ok(String::new())
            })
            .unwrap();
    }

    /// Publishes one message occurrence, a source of another class the reconciler must never touch.
    fn publish_message(&self) {
        let unit = SourceUnit {
            class: OccurrenceClass::Messages,
            identity: vec![
                ("project_id", PROJECT.to_string()),
                ("harness", "opencode".to_string()),
                ("session_id", "ses_1".to_string()),
                ("message_id", "msg_1".to_string()),
                ("block_index", "0".to_string()),
            ],
            revision: "5".to_string(),
            representation: Representation::Text,
            text: "a message".to_string(),
            role: "user".to_string(),
        };
        self.publisher().publish(&unit, NOW).unwrap();
    }

    fn reconcile(&self, scope: &ReconcileScope) -> ReconcileReport {
        GitReconciler::new(&self.kernel)
            .run_episode(
                &support::projection_gate::open_gate(),
                scope,
                bounds(),
                &unbounded(),
            )
            .unwrap()
    }

    /// Live descriptors by class and repository: `(class, repository_id or "")`.
    fn live_classes(&self) -> Vec<(String, String)> {
        let hold = self.capture();
        let rows = self.export_window(&hold, ExportWindow::Snapshot);
        self.kernel
            .release_source_hold(&self.binding(), &hold.hold_id, hold.captured_at)
            .unwrap();
        let mut out: Vec<(String, String)> = rows
            .into_iter()
            .filter(|row| row.invalidated_commit_seq.is_none())
            .map(|row| {
                let repository = row
                    .detail
                    .identity
                    .iter()
                    .find(|(field, _)| field == "repository_id")
                    .map(|(_, value)| value.clone())
                    .unwrap_or_default();
                (row.detail.class, repository)
            })
            .collect();
        out.sort();
        out
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
                    admission: hold_admission(),
                    expiry_ms: NonZeroU64::new((20 * DAY_MS) as u64).unwrap(),
                },
            )
            .unwrap()
    }

    fn export_window(&self, hold: &SourceHold, window: ExportWindow) -> Vec<SourceRow> {
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
                    window,
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

    /// The live retained commit ids of the repository, read through the kernel's export.
    fn live_oids(&self) -> BTreeSet<String> {
        let hold = self.capture();
        let rows = self.export_window(&hold, ExportWindow::Snapshot);
        self.kernel
            .release_source_hold(&self.binding(), &hold.hold_id, hold.captured_at)
            .unwrap();
        rows.into_iter()
            .filter(|row| {
                row.invalidated_commit_seq.is_none()
                    && row
                        .detail
                        .identity
                        .iter()
                        .any(|(field, value)| field == "repository_id" && value == REPO)
            })
            .map(|row| {
                row.detail
                    .identity
                    .iter()
                    .find(|(field, _)| field == "oid")
                    .map(|(_, value)| value.clone())
                    .unwrap()
            })
            .collect()
    }

    /// Live and retired evidence objects in the kernel; retirement of a descriptor must leave its evidence live under its holds.
    fn evidence_counts(&self) -> (i64, i64) {
        let db = Connection::open_with_flags(
            self.root.join("kernel/kernel.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        db.query_row(
            "SELECT SUM(invalidated_commit_seq IS NULL),SUM(invalidated_commit_seq IS NOT NULL)
             FROM object_registry WHERE object_kind='evidence'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
    }

    fn bootstrap(&self, data_home: &Path, hold: &SourceHold) -> SearchProjection {
        let rows = self.export_window(hold, ExportWindow::Snapshot);
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
                        identity_contract_version: "search-projection-identity-v3".to_string(),
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
        self.apply(&projection, hold, &rows, hold.snapshot);
        projection
    }

    fn apply(
        &self,
        projection: &SearchProjection,
        hold: &SourceHold,
        rows: &[SourceRow],
        through: i64,
    ) {
        let identities = row_identities(rows);
        let batch = batch_from_rows(
            rows,
            &identities,
            MutationIdentity {
                kernel_incarnation_id: self.kernel_incarnation_id(),
                hold_id: hold.hold_id.clone(),
                snapshot_commit_seq: hold.snapshot,
                through_commit_seq: through,
            },
            Some(GENERATION),
        )
        .unwrap();
        projection.apply_batch(&batch, batch_bounds(), 2).unwrap();
    }
}

/// Projection rows: (oid occurrence id, tombstoned, pending jobs).
fn projection_rows(data_home: &Path) -> BTreeSet<(String, bool, i64)> {
    Connection::open_with_flags(
        data_home.join("search").join("search.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
    .prepare(
        "SELECT o.occurrence_id,t.occurrence_id IS NOT NULL,
                (SELECT COUNT(*) FROM embedding_jobs j WHERE j.occurrence_id=o.occurrence_id AND j.state='pending')
         FROM occurrences o LEFT JOIN occurrence_tombstones t ON t.occurrence_id=o.occurrence_id",
    )
    .unwrap()
    .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
    .unwrap()
    .collect::<rusqlite::Result<_>>()
    .unwrap()
}

fn occurrence_id(oid: &str) -> String {
    encode_preserving_span(&Occurrence {
        class: "git_commits",
        identity: &[
            ("repository_id", REPO),
            ("object_format", "sha1"),
            ("oid", oid),
        ],
        revision: "1",
        representation: "commit_message",
        span: None,
    })
    .unwrap()
    .occurrence_id
}

fn set(oids: &[&String]) -> BTreeSet<String> {
    oids.iter().map(|oid| (*oid).clone()).collect()
}

/// AC1, AC4, AC5: the independent ref ledger predicts the surviving and retired commits after a branch is deleted, a ref is rewritten, and the permitted set is emptied; every evidence object stays retained; a repeated episode and a reopened kernel retire nothing more; a live projection catching up over the window carries exactly the tombstones with their pending work withdrawn, and replaying the window changes nothing.
#[test]
fn reconciliation_retires_exactly_the_unreachable_commits() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let repo = Repo::init(&dir.path().join("repo"));
    let c1 = repo.commit(MAIN, &[], "one\n", 1);
    let c2 = repo.commit(MAIN, &[&c1], "two\n", 2);
    let c3 = repo.commit(MAIN, &[&c2], "three\n", 3);
    let f1 = repo.commit(FEATURE, &[&c1], "feature\n", 4);
    let all = [c1.clone(), c2.clone(), c3.clone(), f1.clone()];
    corpus.publish(&repo, &all);
    // Another repository's commit and a message occurrence are outside every episode below.
    let beta = Repo::init(&dir.path().join("beta"));
    let b1 = beta.commit(MAIN, &[], "beta\n", 7);
    corpus.publish_as(&beta, "repo-beta", std::slice::from_ref(&b1));
    corpus.publish_message();
    let foreign = vec![
        ("git_commits".to_string(), "repo-beta".to_string()),
        ("messages".to_string(), String::new()),
    ];
    assert_eq!(corpus.live_oids(), set(&[&c1, &c2, &c3, &f1]));
    let hold = corpus.capture();
    let projection = corpus.bootstrap(dir.path(), &hold);

    // Everything reachable: a complete episode that retires nothing still certifies the whole inventory.
    let kept = corpus.reconcile(&repo.scope(&[MAIN, FEATURE]));
    assert_eq!(kept.end, ReconcileEnd::Complete, "{kept:?}");
    assert_eq!(
        (kept.retained, kept.reachable, kept.preserved, kept.retired),
        (4, Some(4), 4, 0)
    );
    assert_eq!(corpus.live_oids(), set(&[&c1, &c2, &c3, &f1]));

    // The feature branch still exists but is not permitted: its tip is the one commit no permitted ref reaches.
    let unpermitted = corpus.reconcile(&repo.scope(&[MAIN]));
    assert_eq!(unpermitted.end, ReconcileEnd::Complete, "{unpermitted:?}");
    assert_eq!(
        (
            unpermitted.retained,
            unpermitted.reachable,
            unpermitted.preserved,
            unpermitted.retired
        ),
        (4, Some(3), 3, 1)
    );
    assert_eq!(corpus.live_oids(), set(&[&c1, &c2, &c3]));

    // Deleting the branch afterwards changes nothing: the inventory already excludes it.
    repo.delete_ref(FEATURE);
    let after_delete = corpus.reconcile(&repo.scope(&[MAIN]));
    assert_eq!(
        (after_delete.retained, after_delete.retired),
        (3, 0),
        "{after_delete:?}"
    );

    // main is rewritten to drop its tip.
    repo.point_ref(MAIN, &c2);
    let after_rewrite = corpus.reconcile(&repo.scope(&[MAIN]));
    assert_eq!(
        (
            after_rewrite.retained,
            after_rewrite.reachable,
            after_rewrite.preserved,
            after_rewrite.retired
        ),
        (3, Some(2), 2, 1)
    );
    assert_eq!(corpus.live_oids(), set(&[&c1, &c2]));
    assert_eq!(
        corpus.evidence_counts(),
        (6, 0),
        "retirement releases no retained evidence"
    );

    // The same episode again retires nothing more.
    let again = corpus.reconcile(&repo.scope(&[MAIN]));
    assert_eq!((again.retained, again.retired), (2, 0), "{again:?}");
    assert_eq!(corpus.live_oids(), set(&[&c1, &c2]));

    // A verified empty selection retires the rest; the evidence stays.
    let emptied = corpus.reconcile(&repo.scope(&[]));
    assert_eq!(emptied.end, ReconcileEnd::Complete, "{emptied:?}");
    assert_eq!(
        (
            emptied.retained,
            emptied.reachable,
            emptied.preserved,
            emptied.retired
        ),
        (2, Some(0), 0, 2)
    );
    assert!(corpus.live_oids().is_empty());
    assert_eq!(
        corpus.live_classes(),
        foreign,
        "the other repository and the message class are untouched"
    );
    assert_eq!(corpus.evidence_counts(), (6, 0));

    // The live projection catches up over every retirement: four tombstones, no pending work left, and an identical replay.
    let through = corpus.kernel.tip().unwrap();
    corpus
        .kernel
        .extend_source_hold(&corpus.binding(), &hold.hold_id, through, hold_admission())
        .unwrap();
    let delta = corpus.export_window(&hold, ExportWindow::CatchUp { through });
    corpus.apply(&projection, &hold, &delta, through);
    let rows = projection_rows(dir.path());
    assert_eq!(
        rows.len(),
        6,
        "four alpha commits, the beta commit, and the message"
    );
    assert_eq!(
        rows.iter().filter(|row| !row.1).count(),
        2,
        "the beta commit and the message stay live with their pending work"
    );
    for oid in &all {
        let row = rows.iter().find(|row| row.0 == occurrence_id(oid)).unwrap();
        assert!(row.1, "{oid} is tombstoned");
        assert_eq!(row.2, 0, "{oid} has no pending work");
    }
    corpus.apply(&projection, &hold, &delta, through);
    assert_eq!(projection_rows(dir.path()), rows);

    // A reopened kernel sees the same retired inventory and an episode over it retires nothing more.
    drop(projection);
    let corpus = corpus.reopen();
    assert!(corpus.live_oids().is_empty());
    let reopened = corpus.reconcile(&repo.scope(&[]));
    assert_eq!(
        (reopened.retained, reopened.retired),
        (0, 0),
        "{reopened:?}"
    );
    assert_eq!(corpus.evidence_counts(), (6, 0));
    assert_eq!(corpus.live_classes(), foreign);
}

/// A retirement page that waits for the kernel writer past its budget's deadline is cancelled, not committed once the writer frees: the page's commit is bounded by the same deadline and rechecks the budget under the writer.
#[test]
fn retirement_does_not_wait_out_its_budget_behind_the_writer() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let repo = Repo::init(&dir.path().join("repo"));
    let c1 = repo.commit(MAIN, &[], "one\n", 1);
    let c2 = repo.commit(MAIN, &[&c1], "two\n", 2);
    corpus.publish(&repo, &[c1.clone(), c2.clone()]);
    repo.point_ref(MAIN, &c1);
    let budget = EvalBudget::new(
        Some(Instant::now() + Duration::from_millis(400)),
        Arc::new(AtomicBool::new(false)),
    );
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let kernel = &corpus.kernel;
    let (report, waited) = std::thread::scope(|scope| {
        // Holds the kernel writer until released, well after the episode's deadline.
        scope.spawn(move || {
            let _ = kernel.commit(intent("hold-writer"), |_| {
                held_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Err(KernelError::Fault)
            });
        });
        held_rx.recv().unwrap();
        // The holder lets go only long after the deadline; an episode that returns sooner did not wait it out.
        scope.spawn(move || {
            std::thread::sleep(Duration::from_secs(5));
            release_tx.send(()).unwrap();
        });
        let started = Instant::now();
        let report = GitReconciler::new(&corpus.kernel)
            .run_episode(
                &support::projection_gate::open_gate(),
                &repo.scope(&[MAIN]),
                bounds(),
                &budget,
            )
            .unwrap();
        (report, started.elapsed())
    });
    assert_eq!(
        report.end,
        ReconcileEnd::Blocked(ReconcileBlocked::Cancelled(ReconcilePhase::Retirement)),
        "{report:?}"
    );
    assert!(
        waited < Duration::from_secs(4),
        "waited {waited:?} for the writer"
    );
    assert_eq!(report.retired, 0);
    assert_eq!(corpus.live_oids(), set(&[&c1, &c2]));
}

/// A stored descriptor payload that is internally consistent but belongs to another descriptor's object id is refused by the inventory: the reconciler would otherwise judge one commit's reachability and retire another commit's row.
#[test]
fn inventory_refuses_a_payload_bound_to_another_object_id() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let repo = Repo::init(&dir.path().join("repo"));
    let c1 = repo.commit(MAIN, &[], "one\n", 1);
    let c2 = repo.commit(MAIN, &[&c1], "two\n", 2);
    let published = corpus.publish(&repo, &[c1.clone(), c2.clone()]);
    // Overwrite c2's stored payload with c1's, which is valid on its own and names c1's oid under c2's object id.
    let db = Connection::open(dir.path().join("kernel/kernel.sqlite")).unwrap();
    db.execute(
        "UPDATE observations SET observation_payload=(SELECT observation_payload FROM observations WHERE object_id=?1) WHERE object_id=?2",
        [&published[0].object_id, &published[1].object_id],
    )
    .unwrap();
    drop(db);
    let page = corpus.kernel.live_source_descriptors(
        OccurrenceClass::GitCommits,
        corpus.kernel.tip().unwrap(),
        None,
        NonZeroUsize::new(64).unwrap(),
        &unbounded(),
    );
    assert!(
        matches!(page, Err(KernelError::CorruptCanonicalRow)),
        "{page:?}"
    );
    let episode = GitReconciler::new(&corpus.kernel).run_episode(
        &support::projection_gate::open_gate(),
        &repo.scope(&[MAIN]),
        bounds(),
        &unbounded(),
    );
    assert!(
        matches!(episode, Err(KernelError::CorruptCanonicalRow)),
        "{episode:?}"
    );
}

/// A tip whose stored bytes are another, parentless commit's bytes is refused: the walk would otherwise certify only the tip as reachable and retire its real ancestors.
#[test]
fn traversal_refuses_a_commit_that_does_not_hash_to_its_id() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let repo = Repo::init(&dir.path().join("repo"));
    let c1 = repo.commit(MAIN, &[], "one\n", 1);
    let c2 = repo.commit(MAIN, &[&c1], "two\n", 2);
    let c3 = repo.commit(MAIN, &[&c2], "three\n", 3);
    corpus.publish(&repo, &[c1.clone(), c2.clone(), c3.clone()]);
    repo.substitute(&c3, &c1);
    let report = corpus.reconcile(&repo.scope(&[MAIN]));
    assert_eq!(
        report.end,
        ReconcileEnd::Blocked(ReconcileBlocked::Repository(GitRefusal::HashMismatch(
            c3.clone()
        ))),
        "{report:?}"
    );
    assert_eq!(report.retired, 0);
    assert_eq!(corpus.live_oids(), set(&[&c1, &c2, &c3]));
}

/// A budget with no deadline cancels the wait for the kernel writer through its interrupt, so a cancelled episode returns while another writer still holds the kernel rather than after it lets go.
#[test]
fn retirement_stops_waiting_for_the_writer_when_the_budget_is_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let repo = Repo::init(&dir.path().join("repo"));
    let c1 = repo.commit(MAIN, &[], "one\n", 1);
    let c2 = repo.commit(MAIN, &[&c1], "two\n", 2);
    corpus.publish(&repo, &[c1.clone(), c2.clone()]);
    repo.point_ref(MAIN, &c1);
    let budget = EvalBudget::new(None, Arc::new(AtomicBool::new(false)));
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let kernel = &corpus.kernel;
    let (report, waited) = std::thread::scope(|scope| {
        scope.spawn(move || {
            let _ = kernel.commit(intent("hold-writer"), |_| {
                held_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Err(KernelError::Fault)
            });
        });
        held_rx.recv().unwrap();
        let cancel = &budget;
        scope.spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            cancel.cancel();
        });
        // The holder lets go only long after the cancellation; an episode that returns sooner did not wait it out.
        scope.spawn(move || {
            std::thread::sleep(Duration::from_secs(5));
            release_tx.send(()).unwrap();
        });
        let started = Instant::now();
        let report = GitReconciler::new(&corpus.kernel)
            .run_episode(
                &support::projection_gate::open_gate(),
                &repo.scope(&[MAIN]),
                bounds(),
                &budget,
            )
            .unwrap();
        (report, started.elapsed())
    });
    assert_eq!(
        report.end,
        ReconcileEnd::Blocked(ReconcileBlocked::Cancelled(ReconcilePhase::Retirement)),
        "{report:?}"
    );
    assert!(
        waited < Duration::from_secs(4),
        "waited {waited:?} for the writer"
    );
    assert_eq!(report.retired, 0);
    assert_eq!(corpus.live_oids(), set(&[&c1, &c2]));
}

/// The inventory's reads acquire pooled readers under the budget: a descriptor page under an exhausted budget refuses at once, and an episode cancelled while every reader is held returns `Cancelled(Inventory)` while they are still held.
#[test]
fn inventory_stops_waiting_for_a_reader_when_the_budget_is_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let repo = Repo::init(&dir.path().join("repo"));
    let c1 = repo.commit(MAIN, &[], "one\n", 1);
    corpus.publish(&repo, std::slice::from_ref(&c1));
    let budget = EvalBudget::new(None, Arc::new(AtomicBool::new(false)));
    let snapshot = corpus.kernel.tip().unwrap();
    // The kernel's read pool holds two connections; both are occupied for the whole test.
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let release_rx = std::sync::Mutex::new(release_rx);
    let kernel = &corpus.kernel;
    let (report, waited) = std::thread::scope(|scope| {
        for _ in 0..2 {
            let held_tx = held_tx.clone();
            let release_rx = &release_rx;
            scope.spawn(move || {
                kernel
                    .preview(Instant::now() + Duration::from_secs(30), |_| {
                        held_tx.send(()).unwrap();
                        let _ = release_rx.lock().unwrap().recv();
                        Ok(())
                    })
                    .unwrap();
            });
        }
        held_rx.recv().unwrap();
        held_rx.recv().unwrap();
        // A descriptor page under an already exhausted budget refuses without waiting for a reader; the episode below then covers the snapshot read the same way.
        let page = corpus.kernel.live_source_descriptors(
            OccurrenceClass::GitCommits,
            snapshot,
            None,
            NonZeroUsize::new(64).unwrap(),
            &exhausted(),
        );
        assert!(matches!(page, Err(KernelError::Deadline)), "{page:?}");
        let cancel = &budget;
        scope.spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            cancel.cancel();
        });
        // The readers are released only long after the cancellation; an episode that returns sooner did not wait for one.
        scope.spawn(move || {
            std::thread::sleep(Duration::from_secs(5));
            drop(release_tx);
        });
        let started = Instant::now();
        let report = GitReconciler::new(&corpus.kernel)
            .run_episode(
                &support::projection_gate::open_gate(),
                &repo.scope(&[MAIN]),
                bounds(),
                &budget,
            )
            .unwrap();
        (report, started.elapsed())
    });
    assert_eq!(
        report.end,
        ReconcileEnd::Blocked(ReconcileBlocked::Cancelled(ReconcilePhase::Inventory)),
        "{report:?}"
    );
    assert!(
        waited < Duration::from_secs(4),
        "waited {waited:?} for a reader"
    );
    assert_eq!((report.retained, report.retired), (0, 0));
}

/// AC2, AC3, AC6: a missing permitted ref, an unopenable repository, an unreadable commit in the traversal, an exhausted budget, an exceeded traversal bound, and an exceeded inventory bound each end the episode without retiring any commit, and the report claims no coverage for the phase that did not complete.
#[test]
fn incomplete_scans_retire_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let repo = Repo::init(&dir.path().join("repo"));
    let c1 = repo.commit(MAIN, &[], "one\n", 1);
    let c2 = repo.commit(MAIN, &[&c1], "two\n", 2);
    let f1 = repo.commit(FEATURE, &[&c1], "feature\n", 4);
    corpus.publish(&repo, &[c1.clone(), c2.clone(), f1.clone()]);
    let before = corpus.live_oids();
    assert_eq!(before.len(), 3);
    // Every scope below excludes `feature`, so a scan that wrongly treated its failure as emptiness would retire `f1`.
    let missing_ref = corpus.reconcile(&repo.scope(&[MAIN, "refs/heads/gone"]));
    assert_eq!(
        missing_ref.end,
        ReconcileEnd::Blocked(ReconcileBlocked::RefUnresolved(
            "refs/heads/gone".to_string()
        ))
    );
    assert_eq!((missing_ref.reachable, missing_ref.retired), (None, 0));

    let unopenable = ReconcileScope {
        binding: RepositoryBinding {
            repository_id: REPO.to_string(),
            path: dir.path().join("nowhere"),
        },
        permitted_refs: vec![MAIN.to_string()],
    };
    let opened = corpus.reconcile(&unopenable);
    assert_eq!(
        opened.end,
        ReconcileEnd::Blocked(ReconcileBlocked::Repository(GitRefusal::Open))
    );
    assert_eq!(opened.retired, 0);

    let cancelled = GitReconciler::new(&corpus.kernel)
        .run_episode(
            &support::projection_gate::open_gate(),
            &repo.scope(&[MAIN]),
            bounds(),
            &exhausted(),
        )
        .unwrap();
    assert_eq!(
        cancelled.end,
        ReconcileEnd::Blocked(ReconcileBlocked::Cancelled(ReconcilePhase::Inventory))
    );
    assert_eq!((cancelled.retained, cancelled.retired), (0, 0));

    let too_much_work = GitReconciler::new(&corpus.kernel)
        .run_episode(
            &support::projection_gate::open_gate(),
            &repo.scope(&[MAIN]),
            InventoryBounds {
                max_commits: NonZeroUsize::new(1).unwrap(),
                ..bounds()
            },
            &unbounded(),
        )
        .unwrap();
    assert_eq!(
        too_much_work.end,
        ReconcileEnd::Blocked(ReconcileBlocked::WorkExceeded { max: 1 })
    );
    assert_eq!((too_much_work.reachable, too_much_work.retired), (None, 0));

    let too_many_retained = GitReconciler::new(&corpus.kernel)
        .run_episode(
            &support::projection_gate::open_gate(),
            &repo.scope(&[MAIN]),
            InventoryBounds {
                max_retained: NonZeroUsize::new(2).unwrap(),
                ..bounds()
            },
            &unbounded(),
        )
        .unwrap();
    assert_eq!(
        too_many_retained.end,
        ReconcileEnd::Blocked(ReconcileBlocked::RetainedExceeded { max: 2 })
    );
    assert_eq!(too_many_retained.retired, 0);

    // A ref that moves after the walk, and a budget cancelled after the walk, both leave the excluded commit alone.
    let moved = GitReconciler::new(&corpus.kernel)
        .with_probe_for_test(Probe::AfterTraversal, || repo.point_ref(MAIN, &c1))
        .run_episode(
            &support::projection_gate::open_gate(),
            &repo.scope(&[MAIN]),
            bounds(),
            &unbounded(),
        )
        .unwrap();
    assert_eq!(
        moved.end,
        ReconcileEnd::Blocked(ReconcileBlocked::RefsChanged)
    );
    assert_eq!((moved.reachable, moved.retired), (Some(2), 0));
    repo.point_ref(MAIN, &c2);
    let budget = unbounded();
    let cancelled_late = GitReconciler::new(&corpus.kernel)
        .with_probe_for_test(Probe::AfterTraversal, || budget.cancel())
        .run_episode(
            &support::projection_gate::open_gate(),
            &repo.scope(&[MAIN]),
            bounds(),
            &budget,
        )
        .unwrap();
    assert_eq!(
        cancelled_late.end,
        ReconcileEnd::Blocked(ReconcileBlocked::Cancelled(ReconcilePhase::Retirement))
    );
    assert_eq!(cancelled_late.retired, 0);
    assert_eq!(corpus.live_oids(), before);

    // A grant revoked after the walk stops the episode the same way: the gate closed under the slice, so nothing runs on the old evidence.
    let gate = support::projection_gate::open_gate();
    let revoked = GitReconciler::new(&corpus.kernel)
        .with_probe_for_test(Probe::AfterTraversal, || gate.close())
        .run_episode(&gate, &repo.scope(&[MAIN]), bounds(), &unbounded())
        .unwrap();
    assert_eq!(
        revoked.end,
        ReconcileEnd::Blocked(ReconcileBlocked::Cancelled(ReconcilePhase::Retirement))
    );
    assert_eq!(revoked.retired, 0);
    assert_eq!(corpus.live_oids(), before);

    // An unreadable commit inside the walk is a store failure, never an absent source.
    repo.garble(&c1);
    let unreadable = corpus.reconcile(&repo.scope(&[MAIN]));
    assert!(
        matches!(
            unreadable.end,
            ReconcileEnd::Blocked(ReconcileBlocked::Repository(GitRefusal::Unreadable(_)))
        ),
        "{unreadable:?}"
    );
    assert_eq!(unreadable.retired, 0);
    assert_eq!(
        corpus.live_oids(),
        before,
        "no incomplete scan retired a source"
    );
}

/// A repository whose object graph is substituted, cut, shadowed, oversized, or of another format than its retained rows is refused whole: a `refs/replace` entry the repository's own config enables, a shallow boundary, a permitted ref given as a partial name that a tag shadows, a commit object above the byte bound, and a retained row of another object format each end the episode without retiring a commit that the true history reaches.
#[test]
fn traversal_refuses_substituted_cut_shadowed_or_foreign_graphs() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let repo = Repo::init(&dir.path().join("repo"));
    let c1 = repo.commit(MAIN, &[], "one\n", 1);
    let c2 = repo.commit(MAIN, &[&c1], "two\n", 2);
    let c3 = repo.commit(MAIN, &[&c2], "three\n", 3);
    corpus.publish(&repo, &[c1.clone(), c2.clone(), c3.clone()]);
    let before = corpus.live_oids();
    assert_eq!(before.len(), 3);

    // `refs/replace/<c2>` names a rootless commit; gix 0.87.1 loads replacement refs when the repository's own config carries `core.useReplaceRefs = false`. Read through the replacement, `c2` has no parent and `c1` looks unreachable although `main` reaches it.
    let orphan = repo.commit("refs/heads/orphan", &[], "orphan\n", 9);
    repo.point_ref(&format!("refs/replace/{c2}"), &orphan);
    repo.append_config("[core]\n\tuseReplaceRefs = false\n");
    let replaced = corpus.reconcile(&repo.scope(&[MAIN]));
    assert_eq!(replaced.end, ReconcileEnd::Complete, "{replaced:?}");
    assert_eq!(
        (
            replaced.retained,
            replaced.reachable,
            replaced.preserved,
            replaced.retired
        ),
        (3, Some(3), 3, 0),
        "the walk follows the true parents, not the replacement"
    );
    assert_eq!(corpus.live_oids(), before);

    // A shallow boundary at `c2` cuts `c1` out of the walk while its object is still in the store.
    repo.set_shallow(&c2);
    let shallow = corpus.reconcile(&repo.scope(&[MAIN]));
    assert_eq!(
        shallow.end,
        ReconcileEnd::Blocked(ReconcileBlocked::Repository(GitRefusal::Shallow)),
        "{shallow:?}"
    );
    assert_eq!((shallow.reachable, shallow.retired), (None, 0));
    repo.clear_shallow();
    assert_eq!(corpus.live_oids(), before);

    // A tag named `main` pointing at `c1` shadows the branch when the permitted ref is only a partial name.
    repo.point_ref("refs/tags/main", &c1);
    let partial = corpus.reconcile(&repo.scope(&["main"]));
    assert_eq!(
        partial.end,
        ReconcileEnd::Blocked(ReconcileBlocked::RefUnresolved("main".to_string())),
        "{partial:?}"
    );
    assert_eq!((partial.reachable, partial.retired), (None, 0));
    let full = corpus.reconcile(&repo.scope(&[MAIN]));
    assert_eq!(full.end, ReconcileEnd::Complete, "{full:?}");
    assert_eq!(full.retired, 0);
    assert_eq!(corpus.live_oids(), before);

    // A retained row of another object format cannot be judged by a sha1 walk: it is not unreachable, it is unjudgeable.
    let foreign_oid = "f".repeat(64);
    let foreign_row = corpus.publish_foreign_format("sha256", &foreign_oid);
    let mut with_foreign = before.clone();
    with_foreign.insert(foreign_oid);
    assert_eq!(corpus.live_oids(), with_foreign);
    let foreign = corpus.reconcile(&repo.scope(&[MAIN]));
    assert_eq!(
        foreign.end,
        ReconcileEnd::Blocked(ReconcileBlocked::ObjectFormatMismatch("sha256".to_string())),
        "{foreign:?}"
    );
    assert_eq!(foreign.retired, 0);
    assert_eq!(corpus.live_oids(), with_foreign);
    corpus.retire_descriptor(&foreign_row.object_id, "retire-foreign");
    assert_eq!(corpus.live_oids(), before);

    // A commit object above the byte bound is refused by the object store before the walk reads it, so the episode ends unreadable instead of walking a truncated graph.
    let big = repo.commit(MAIN, &[&c3], &"m".repeat(8192), 4);
    repo.commit(MAIN, &[&big], "cap\n", 5);
    let oversized = corpus.reconcile(&repo.scope(&[MAIN]));
    assert!(
        matches!(
            oversized.end,
            ReconcileEnd::Blocked(ReconcileBlocked::Repository(GitRefusal::Unreadable(_)))
        ),
        "{oversized:?}"
    );
    assert_eq!((oversized.reachable, oversized.retired), (None, 0));
    assert_eq!(corpus.live_oids(), before);
}

/// The inventory is bounded and cancellable on its own, agrees with the export about which descriptors are live, and reports only the descriptors its commits invalidated.
#[test]
fn inventory_is_bounded_and_agrees_with_the_export() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let repo = Repo::init(&dir.path().join("repo"));
    let c1 = repo.commit(MAIN, &[], "one\n", 1);
    let c2 = repo.commit(MAIN, &[&c1], "two\n", 2);
    let f1 = repo.commit(FEATURE, &[&c1], "feature\n", 4);
    let published = corpus.publish(&repo, &[c1.clone(), c2.clone(), f1.clone()]);
    let beta = Repo::init(&dir.path().join("beta"));
    let b1 = beta.commit(MAIN, &[], "beta\n", 7);
    corpus.publish_as(&beta, "repo-beta", std::slice::from_ref(&b1));
    let before = corpus.live_oids();
    assert_eq!(before.len(), 3);

    // The class holds four live rows across two repositories; a scan bound of three ends the episode before the repository's rows are all found, although its retained bound of sixteen is far away.
    let scanned = GitReconciler::new(&corpus.kernel)
        .run_episode(
            &support::projection_gate::open_gate(),
            &repo.scope(&[MAIN]),
            InventoryBounds {
                max_scanned: NonZeroUsize::new(3).unwrap(),
                ..bounds()
            },
            &unbounded(),
        )
        .unwrap();
    assert_eq!(
        scanned.end,
        ReconcileEnd::Blocked(ReconcileBlocked::ScannedExceeded { max: 3 }),
        "{scanned:?}"
    );
    assert_eq!((scanned.retained, scanned.retired), (0, 0));

    // The scan bound limits the rows the kernel is asked for, not only the rows counted after they arrive: with pages of one row and a bound of two, the overflow is reported after the second page's lookahead, without a third page.
    let pages = std::cell::Cell::new(0usize);
    let clamped = GitReconciler::new(&corpus.kernel)
        .with_probe_for_test(Probe::AfterInventoryPage, || pages.set(pages.get() + 1))
        .run_episode(
            &support::projection_gate::open_gate(),
            &repo.scope(&[MAIN]),
            InventoryBounds {
                page_rows: NonZeroUsize::new(1).unwrap(),
                max_scanned: NonZeroUsize::new(2).unwrap(),
                ..bounds()
            },
            &unbounded(),
        )
        .unwrap();
    assert_eq!(
        clamped.end,
        ReconcileEnd::Blocked(ReconcileBlocked::ScannedExceeded { max: 2 }),
        "{clamped:?}"
    );
    assert_eq!(
        pages.get(),
        2,
        "a third page would exceed the scan bound before its rows were counted"
    );

    // A budget cancelled after the first inventory page stops the inventory itself, not the traversal after it.
    let budget = unbounded();
    let cancelled = GitReconciler::new(&corpus.kernel)
        .with_probe_for_test(Probe::AfterInventoryPage, || budget.cancel())
        .run_episode(
            &support::projection_gate::open_gate(),
            &repo.scope(&[MAIN]),
            bounds(),
            &budget,
        )
        .unwrap();
    assert_eq!(
        cancelled.end,
        ReconcileEnd::Blocked(ReconcileBlocked::Cancelled(ReconcilePhase::Inventory)),
        "{cancelled:?}"
    );
    assert_eq!((cancelled.retained, cancelled.retired), (0, 0));
    assert_eq!(corpus.live_oids(), before);

    // A descriptor whose evidence was deleted is absent from every export snapshot, so the inventory does not count or retire it either.
    corpus.delete_evidence(&published[2], "delete-f1");
    assert_eq!(corpus.live_oids(), set(&[&c1, &c2]));
    let page = corpus
        .kernel
        .live_source_descriptors(
            OccurrenceClass::GitCommits,
            corpus.kernel.tip().unwrap(),
            None,
            NonZeroUsize::new(64).unwrap(),
            &unbounded(),
        )
        .unwrap();
    assert_eq!(page.rows.len(), 3, "two alpha rows and the beta row");
    let after_delete = corpus.reconcile(&repo.scope(&[MAIN]));
    assert_eq!(after_delete.end, ReconcileEnd::Complete, "{after_delete:?}");
    assert_eq!(
        (
            after_delete.retained,
            after_delete.preserved,
            after_delete.retired
        ),
        (2, 2, 0)
    );

    // A descriptor another writer retired between the inventory and the retirement page is skipped by the page's commit, and the report counts only what the commit invalidated.
    let c4 = repo.commit(MAIN, &[&c2], "four\n", 5);
    let c5 = repo.commit(MAIN, &[&c4], "five\n", 6);
    let late = corpus.publish(&repo, &[c4.clone(), c5.clone()]);
    repo.point_ref(MAIN, &c2);
    let c5_object = late[1].object_id.clone();
    let counted = GitReconciler::new(&corpus.kernel)
        .with_probe_for_test(Probe::AfterTraversal, || {
            corpus.retire_descriptor(&c5_object, "retire-c5")
        })
        .run_episode(
            &support::projection_gate::open_gate(),
            &repo.scope(&[MAIN]),
            bounds(),
            &unbounded(),
        )
        .unwrap();
    assert_eq!(counted.end, ReconcileEnd::Complete, "{counted:?}");
    assert_eq!(
        (counted.retained, counted.preserved, counted.retired),
        (4, 2, 1),
        "c4 is retired by the episode; c5 was retired by the other writer"
    );
    assert_eq!(corpus.live_oids(), set(&[&c1, &c2]));
}
