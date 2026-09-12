//! Git reconciliation retires only the retained commits a complete traversal from the permitted refs never reached: branch removal, a rewritten ref, and a verified empty selection retire exactly the ledger's excluded commits while the evidence stays retained; an unresolved ref, an unopenable or unreadable repository, an exhausted budget, and an exceeded work or inventory bound retire nothing; a repeated episode and a live projection's catch-up carry the tombstones without resurrection.

use std::collections::BTreeSet;
use std::num::{NonZeroU64, NonZeroUsize};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use daemon::git_reconcile::{
    GitReconciler, InventoryBounds, ReconcileBlocked, ReconcileEnd, ReconcilePhase,
    ReconcileReport, ReconcileScope,
};
use daemon::git_sources::{GitReadBounds, GitRefusal, RepositoryBinding, read_selection};
use daemon::harness_sources::{Representation, SourcePublisher, SourceUnit};
use daemon::search_projection::SearchProjection;
use kernel::applicability::EvalBudget;
use kernel::source_identity::{Occurrence, OccurrenceClass, encode_preserving_span};
use kernel::{
    CommitIntent, Dimension, DomainSpec, ExportWindow, KernelStore, ProviderEgress, ScopeSpec,
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
        max_retained: NonZeroUsize::new(16).unwrap(),
        max_commits: NonZeroUsize::new(16).unwrap(),
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
        std::fs::write(self.root.join(".git").join(name), format!("{oid}\n")).unwrap();
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
    fn garble(&self, oid: &str) {
        let loose = self
            .root
            .join(".git/objects")
            .join(&oid[..2])
            .join(&oid[2..]);
        std::fs::set_permissions(&loose, PermissionsExt::from_mode(0o644)).unwrap();
        std::fs::write(&loose, b"not zlib").unwrap();
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
    fn publish_as(&self, repo: &Repo, repository_id: &str, oids: &[String]) {
        let binding = RepositoryBinding {
            repository_id: repository_id.to_string(),
            path: repo.root.clone(),
        };
        let selection = read_selection(&binding, oids, read_bounds()).unwrap();
        assert_eq!(selection.units.len(), oids.len());
        for unit in &selection.units {
            self.publisher().publish(unit, NOW).unwrap();
        }
    }

    fn publish(&self, repo: &Repo, oids: &[String]) {
        self.publish_as(repo, REPO, oids);
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
            .run_episode(scope, bounds(), &unbounded())
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
        .run_episode(&repo.scope(&[MAIN]), bounds(), &exhausted())
        .unwrap();
    assert_eq!(
        cancelled.end,
        ReconcileEnd::Blocked(ReconcileBlocked::Cancelled(ReconcilePhase::Inventory))
    );
    assert_eq!((cancelled.retained, cancelled.retired), (0, 0));

    let too_much_work = GitReconciler::new(&corpus.kernel)
        .run_episode(
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
        .with_after_traversal_for_test(|| repo.point_ref(MAIN, &c1))
        .run_episode(&repo.scope(&[MAIN]), bounds(), &unbounded())
        .unwrap();
    assert_eq!(
        moved.end,
        ReconcileEnd::Blocked(ReconcileBlocked::RefsChanged)
    );
    assert_eq!((moved.reachable, moved.retired), (Some(2), 0));
    repo.point_ref(MAIN, &c2);
    let budget = unbounded();
    let cancelled_late = GitReconciler::new(&corpus.kernel)
        .with_after_traversal_for_test(|| budget.cancel())
        .run_episode(&repo.scope(&[MAIN]), bounds(), &budget)
        .unwrap();
    assert_eq!(
        cancelled_late.end,
        ReconcileEnd::Blocked(ReconcileBlocked::Cancelled(ReconcilePhase::Retirement))
    );
    assert_eq!(cancelled_late.retired, 0);
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
