//! This module tests search catch-up against real kernel and projection stores.
//! An independent commit ledger predicts every durable state, per window and after every crash cut.
//! The tests verify that the local transaction releases before the driver acquires the kernel writer.
//! Lost replies reconcile to durable facts, and refusals move nothing.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Write};
use std::num::{NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use daemon::search_catchup::{
    Blocked, CatchUpConsumer, CatchUpError, EpisodeBounds, EpisodeEnd, EpisodeEvent, EpisodeFault,
    EpisodeReport, QuarantineKind, SearchCatchUp,
};
use daemon::search_projection::SearchProjection;
use kernel::source_identity::Occurrence;
use kernel::{
    ArtifactDeletionIdentity, ArtifactDeletionKind, ArtifactDeletionRequest,
    ArtifactDeletionResult, ArtifactIngestRequest, CommitIntent, CommitPageBounds, CommitReadError,
    CommitReadRequest, DomainSpec, ExportWindow, KernelStore, ProviderEgress, RepositoryProvenance,
    Sensitivity, SourceDescriptorRequest, SourceHold, SourceHoldAdmission, SourceHoldBinding,
    SourceHoldBounds, SourceHoldError, SourcePageBounds, SourceRow,
};
use retrieval::ProjectionError;
use retrieval::batch::{
    BatchBounds, MutationIdentity, VectorGeneration, batch_from_rows, register_generation,
    row_identities,
};
use retrieval::{PersistBounds, ProjectionIdentity, install_identity};
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};

const CONSUMER: &str = "search";
const KERNEL_INCARNATION: &str = "kernel-1";
const GENERATION: &str = "gen-1";
const POLICY: &str = "source-policy.v1";
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

const CHILD_ROOT: &str = "EIDNARA_SEARCH_CATCHUP_CHILD_ROOT";
const CHILD_CUT: &str = "EIDNARA_SEARCH_CATCHUP_CHILD_CUT";
const CHILD_BARRIER: &str = "EIDNARA_SEARCH_CATCHUP_BARRIER";
const CHILD_LEDGER: &str = "EIDNARA_SEARCH_CATCHUP_LEDGER";

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "daemon-search-catchup-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

fn identity(kernel_incarnation_id: &str) -> ProjectionIdentity {
    ProjectionIdentity {
        schema_version: retrieval::SCHEMA_VERSION,
        kernel_incarnation_id: kernel_incarnation_id.to_string(),
        projection_policy_version: POLICY.to_string(),
        identity_contract_version: "search-projection-identity-v2".to_string(),
        limit_manifest_protocol_version: "limits.v1".to_string(),
        embedding_model: "model-a".to_string(),
        tokenizer_fingerprint: "fp-a".to_string(),
        vector_dimension: 8,
        generation_epoch: 1,
    }
}

fn generation() -> VectorGeneration {
    VectorGeneration {
        generation_id: GENERATION.to_string(),
        embedding_model: "model-a".to_string(),
        tokenizer_fingerprint: "fp-a".to_string(),
        vector_dimension: 8,
        generation_epoch: 1,
    }
}

fn source_page_bounds() -> SourcePageBounds {
    SourcePageBounds {
        max_rows: NonZeroUsize::new(64).unwrap(),
        max_encoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
        max_decoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
        max_row_bytes: NonZeroU64::new(1 << 16).unwrap(),
    }
}

fn hold_admission() -> SourceHoldAdmission {
    SourceHoldAdmission {
        max_references: NonZeroUsize::new(64).unwrap(),
        max_encoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
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

fn bounds() -> EpisodeBounds {
    EpisodeBounds {
        commits: CommitPageBounds {
            max_commits: NonZeroUsize::new(64).unwrap(),
            max_rows: NonZeroUsize::new(64).unwrap(),
            max_payload_bytes: NonZeroU64::new(1 << 20).unwrap(),
        },
        hold_admission: hold_admission(),
        source_page: source_page_bounds(),
        max_source_pages: NonZeroUsize::new(8).unwrap(),
        batch: batch_bounds(),
    }
}

/// Two commits per window forces several windows over one target.
fn two_commit_windows() -> EpisodeBounds {
    let mut bounds = bounds();
    bounds.commits.max_commits = NonZeroUsize::new(2).unwrap();
    bounds
}

/// The test records what it did to the kernel here, so the projection's contents can be predicted without asking the projector.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Ledger {
    /// occurrence id -> (creating commit, exact text)
    rows: BTreeMap<String, (i64, String)>,
    /// occurrence id -> (invalidating commit, reason)
    tombstones: BTreeMap<String, (i64, String)>,
    /// descriptor object id -> occurrence id, for retirements
    objects: BTreeMap<String, String>,
}

impl Ledger {
    fn rows_through(&self, checkpoint: i64) -> BTreeMap<String, String> {
        self.rows
            .iter()
            .filter(|(_, (created, _))| *created <= checkpoint)
            .map(|(id, (_, text))| (id.clone(), text.clone()))
            .collect()
    }

    fn tombstones_through(&self, checkpoint: i64) -> BTreeMap<String, (i64, String)> {
        self.tombstones
            .iter()
            .filter(|(_, (at, _))| *at <= checkpoint)
            .map(|(id, fact)| (id.clone(), fact.clone()))
            .collect()
    }

    /// Live message rows at `checkpoint` are exactly the rows with durable pending work.
    fn pending_through(&self, checkpoint: i64) -> BTreeSet<String> {
        let tombstoned = self.tombstones_through(checkpoint);
        self.rows_through(checkpoint)
            .into_keys()
            .filter(|id| !tombstoned.contains_key(id))
            .collect()
    }

    fn occurrence_of(&self, text: &str) -> String {
        self.rows
            .iter()
            .find(|(_, (_, stored))| stored == text)
            .map(|(id, _)| id.clone())
            .unwrap()
    }

    fn commit_of(&self, text: &str) -> i64 {
        self.rows[&self.occurrence_of(text)].0
    }

    fn encode(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for (id, (created, text)) in &self.rows {
            lines.push(format!(
                "{CHILD_LEDGER} row {id} {created} {}",
                hex(text.as_bytes())
            ));
        }
        for (id, (at, reason)) in &self.tombstones {
            lines.push(format!("{CHILD_LEDGER} tombstone {id} {at} {reason}"));
        }
        lines
    }

    fn decode(lines: &[String]) -> Self {
        let mut ledger = Self::default();
        for line in lines {
            let fields: Vec<&str> = line.split(' ').collect();
            match fields.as_slice() {
                [_, "row", id, created, text] => {
                    ledger.rows.insert(
                        (*id).to_string(),
                        (
                            created.parse().unwrap(),
                            String::from_utf8(unhex(text)).unwrap(),
                        ),
                    );
                }
                [_, "tombstone", id, at, reason] => {
                    ledger.tombstones.insert(
                        (*id).to_string(),
                        (at.parse().unwrap(), (*reason).to_string()),
                    );
                }
                other => panic!("unexpected ledger line {other:?}"),
            }
        }
        ledger
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).unwrap())
        .collect()
}

/// A kernel the test writes canonical facts into, recording each into the ledger.
struct Corpus {
    root: PathBuf,
    kernel: KernelStore,
    ledger: RefCell<Ledger>,
}

impl Corpus {
    fn open(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            kernel: KernelStore::open(root.join("kernel")).unwrap(),
            ledger: RefCell::new(Ledger::default()),
        }
    }

    fn seed(&self) {
        self.kernel
            .commit(intent("seed"), |envelope| {
                envelope.insert_domain(DomainSpec {
                    domain_id: "domain".to_string(),
                    object_id: "domain-object".to_string(),
                    name: "Name".to_string(),
                    source_kind: "fixture".to_string(),
                    source_id: "domain".to_string(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                })?;
                envelope.register_outbox_consumer(CONSUMER, 1)?;
                Ok(String::new())
            })
            .unwrap();
    }

    fn tip(&self) -> i64 {
        self.kernel.tip().unwrap()
    }

    fn ledger(&self) -> Ledger {
        self.ledger.borrow().clone()
    }

    fn kernel_db(&self) -> PathBuf {
        self.root.join("kernel").join("kernel.sqlite")
    }

    fn ingest(&self, key: &str, text: &str) -> kernel::ArtifactHandle {
        self.kernel
            .ingest_exact_artifact(ArtifactIngestRequest {
                intent: intent(&format!("artifact-{key}")),
                payload: text.as_bytes().to_vec(),
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
            })
            .unwrap()
    }

    /// Publishes every message in one commit; a message whose lineage already has a live revision supersedes it in that same commit.
    fn publish(&self, key: &str, messages: &[(&str, &str, &str)]) -> i64 {
        let handles: Vec<_> = messages
            .iter()
            .enumerate()
            .map(|(index, (_, _, text))| self.ingest(&format!("{key}-{index}"), text))
            .collect();
        let mut outcomes = Vec::new();
        let receipt = self
            .kernel
            .commit(intent(&format!("publish-{key}")), |envelope| {
                outcomes.clear();
                for ((message_id, revision, text), handle) in messages.iter().zip(&handles) {
                    let identity = [
                        ("project_id", "proj-a"),
                        ("harness", "opencode"),
                        ("session_id", "sess-01"),
                        ("message_id", *message_id),
                        ("block_index", "0"),
                    ];
                    let outcome = envelope
                        .publish_source_descriptor(&SourceDescriptorRequest {
                            source_policy: kernel::SourceDescriptorPolicy::Native,
                            occurrence: Occurrence {
                                class: "messages",
                                identity: &identity,
                                revision,
                                representation: "text",
                                span: None,
                            },
                            domain_id: "domain",
                            scope_id: None,
                            evidence_id: &handle.evidence_id,
                            artifact_digest: &handle.digest,
                            buffer: text,
                            sensitivity: Sensitivity::Normal,
                            observed_at: 1,
                        })
                        .unwrap();
                    outcomes.push(((*text).to_string(), outcome));
                }
                Ok(String::new())
            })
            .unwrap();
        let mut ledger = self.ledger.borrow_mut();
        for (text, outcome) in outcomes {
            ledger
                .rows
                .insert(outcome.occurrence_id.clone(), (receipt.commit_seq, text));
            ledger
                .objects
                .insert(outcome.object_id.clone(), outcome.occurrence_id.clone());
            if let Some(replaced) = outcome.replaced_object_id {
                let old = ledger.objects[&replaced].clone();
                ledger
                    .tombstones
                    .insert(old, (receipt.commit_seq, "superseded".to_string()));
            }
        }
        receipt.commit_seq
    }

    fn retire(&self, key: &str, text: &str) -> i64 {
        let occurrence = self.ledger().occurrence_of(text);
        let object = self
            .ledger()
            .objects
            .iter()
            .find(|(_, id)| **id == occurrence)
            .map(|(object, _)| object.clone())
            .unwrap();
        let receipt = self
            .kernel
            .commit(intent(&format!("retire-{key}")), |envelope| {
                envelope.retire_observation(&object)?;
                Ok(String::new())
            })
            .unwrap();
        self.ledger
            .borrow_mut()
            .tombstones
            .insert(occurrence, (receipt.commit_seq, "retired".to_string()));
        receipt.commit_seq
    }

    fn empty(&self, key: &str) -> i64 {
        self.kernel
            .commit(intent(&format!("empty-{key}")), |_| Ok(String::new()))
            .unwrap()
            .commit_seq
    }

    /// Logical (non-purge) deletion for `evidence_id`.
    /// The ledger records nothing: the projection must not change until it can tombstone the deletion.
    fn delete(&self, key: &str, evidence_id: &str) -> ArtifactDeletionResult {
        let digest: String = inspect(&self.kernel_db())
            .query_row(
                "SELECT artifact_digest FROM evidence_meta WHERE evidence_id=?1",
                [evidence_id],
                |row| row.get(0),
            )
            .unwrap();
        self.kernel
            .delete_artifact(ArtifactDeletionRequest {
                intent: intent(&format!("delete-{key}")),
                identity: ArtifactDeletionIdentity::Digest(digest),
                kind: ArtifactDeletionKind::Delete,
                operator_id: None,
                target_locator: None,
                reason: None,
                deleted_at: 42,
            })
            .unwrap()
    }

    /// A control commit: another consumer registers, which projects nothing.
    fn control(&self, consumer: &str) -> i64 {
        self.kernel
            .commit(intent(&format!("control-{consumer}")), |envelope| {
                envelope.register_outbox_consumer(consumer, 1)?;
                Ok(String::new())
            })
            .unwrap()
            .commit_seq
    }

    /// Marks every pending outbox row published, so the consumer reads published history.
    fn publish_outbox(&self) {
        let pending = self.kernel.pending_outbox(256).unwrap();
        if let Some(last) = pending.iter().rev().find(|entry| entry.commit_boundary) {
            self.kernel
                .mark_outbox_published_through(last.outbox_position, 1)
                .unwrap();
        }
    }

    fn binding(&self) -> SourceHoldBinding {
        SourceHoldBinding {
            consumer_id: CONSUMER.to_string(),
            lease_epoch: self.kernel.lease_epoch(),
            source_policy_version: POLICY.to_string(),
        }
    }

    /// Captures the hold at S, applies the fixed-S export as the projection's baseline, and acknowledges S, which is where a catch-up episode begins.
    fn bootstrap(&self, data_home: &Path) -> (SearchProjection, CatchUpConsumer, SourceHold) {
        let binding = self.binding();
        let hold = self
            .kernel
            .capture_source_hold(
                &binding,
                SourceHoldBounds {
                    max_descriptor_rows: NonZeroUsize::new(256).unwrap(),
                    admission: hold_admission(),
                    expiry_ms: NonZeroU64::new((20 * DAY_MS) as u64).unwrap(),
                },
            )
            .unwrap();
        let rows = export_all(
            &self.kernel,
            &binding,
            &hold.hold_id,
            hold.captured_at,
            ExportWindow::Snapshot,
        );
        let projection = SearchProjection::open(data_home).unwrap();
        projection
            .write(|conn| {
                install_identity(conn, &identity(KERNEL_INCARNATION), 1)?;
                register_generation(conn, &generation(), 1)?;
                Ok(())
            })
            .unwrap();
        let identities = row_identities(&rows);
        let batch = batch_from_rows(
            &rows,
            &identities,
            MutationIdentity {
                kernel_incarnation_id: KERNEL_INCARNATION.to_string(),
                hold_id: hold.hold_id.clone(),
                snapshot_commit_seq: hold.snapshot,
                through_commit_seq: hold.snapshot,
            },
            Some(GENERATION),
        )
        .unwrap();
        projection.apply_batch(&batch, batch_bounds(), 2).unwrap();
        self.kernel
            .acknowledge_through_source_hold(&binding, &hold.hold_id, hold.snapshot, 2)
            .unwrap();
        let consumer = CatchUpConsumer {
            binding,
            hold_id: hold.hold_id.clone(),
            kernel_incarnation_id: KERNEL_INCARNATION.to_string(),
            generation_id: Some(GENERATION.to_string()),
        };
        (projection, consumer, hold)
    }

    /// The commits after `after` through `through`, counted from the kernel's own log.
    fn commits_in(&self, after: i64, through: i64) -> usize {
        let count: i64 = inspect(&self.kernel_db())
            .query_row(
                "SELECT COUNT(*) FROM commit_log WHERE commit_seq>?1 AND commit_seq<=?2",
                [after, through],
                |row| row.get(0),
            )
            .unwrap();
        count as usize
    }

    /// The outbox rows and payload bytes one commit carries, from the kernel's own table.
    fn outbox_shape(&self, commit_seq: i64) -> (usize, u64) {
        let (rows, bytes): (i64, i64) = inspect(&self.kernel_db())
            .query_row(
                "SELECT COUNT(*),COALESCE(SUM(LENGTH(payload)),0) FROM outbox WHERE commit_seq=?1",
                [commit_seq],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        (rows as usize, bytes as u64)
    }

    fn kernel_checkpoint(&self) -> i64 {
        kernel_checkpoint(&self.root)
    }
}

fn export_all(
    kernel: &KernelStore,
    binding: &SourceHoldBinding,
    hold_id: &str,
    now: i64,
    window: ExportWindow,
) -> Vec<SourceRow> {
    let mut rows = Vec::new();
    let mut cursor = None;
    loop {
        let page = kernel
            .export_source_page(
                binding,
                hold_id,
                now,
                window,
                cursor.as_ref(),
                source_page_bounds(),
            )
            .unwrap();
        rows.extend(page.rows);
        match page.next {
            Some(next) => cursor = Some(next),
            None => return rows,
        }
    }
}

fn inspect(path: &Path) -> Connection {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap()
}

fn mutate(path: &Path) -> Connection {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE).unwrap();
    connection.busy_timeout(Duration::from_secs(5)).unwrap();
    connection
}

/// A write lock on `path` held until dropped; the holder's own statements must never wait.
fn hold_write_lock(path: &Path) -> Connection {
    let blocker = mutate(path);
    blocker.busy_timeout(Duration::ZERO).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    blocker
}

fn search_path(data_home: &Path) -> PathBuf {
    data_home.join("search").join("search.sqlite")
}

/// The kernel's durable consumer checkpoint, read outside every kernel API.
fn kernel_checkpoint(root: &Path) -> i64 {
    inspect(&root.join("kernel").join("kernel.sqlite"))
        .query_row(
            "SELECT checkpoint_commit_seq FROM outbox_consumers WHERE consumer_id=?1",
            [CONSUMER],
            |row| row.get(0),
        )
        .unwrap()
}

/// The projection's durable state, read outside every projection API.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Durable {
    checkpoint: Option<i64>,
    rows: BTreeMap<String, String>,
    tombstones: BTreeMap<String, (i64, String)>,
    pending: BTreeSet<String>,
}

fn durable(data_home: &Path) -> Durable {
    let conn = inspect(&search_path(data_home));
    let checkpoint = conn
        .query_row(
            "SELECT checkpoint_commit_seq FROM projection_checkpoint WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .ok();
    let rows = conn
        .prepare(
            "SELECT o.occurrence_id,p.bytes FROM occurrences o JOIN payloads p ON p.payload_id=o.payload_id",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                String::from_utf8(row.get::<_, Vec<u8>>(1)?).unwrap(),
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let tombstones = conn
        .prepare("SELECT occurrence_id,invalidated_commit_seq,reason FROM occurrence_tombstones")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, (row.get(1)?, row.get(2)?))))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let pending = conn
        .prepare("SELECT occurrence_id FROM embedding_jobs WHERE state='pending'")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    Durable {
        checkpoint,
        rows,
        tombstones,
        pending,
    }
}

fn assert_matches_ledger(data_home: &Path, ledger: &Ledger, checkpoint: i64) {
    let state = durable(data_home);
    assert_eq!(state.checkpoint, Some(checkpoint), "local checkpoint");
    assert_eq!(state.rows, ledger.rows_through(checkpoint), "rows");
    assert_eq!(
        state.tombstones,
        ledger.tombstones_through(checkpoint),
        "tombstones"
    );
    assert_eq!(state.pending, ledger.pending_through(checkpoint), "pending");
}

fn windows(trace: &[EpisodeEvent]) -> Vec<i64> {
    trace
        .iter()
        .filter_map(|event| match event {
            EpisodeEvent::Acknowledged { through } => Some(*through),
            _ => None,
        })
        .collect()
}

fn reached(report: &EpisodeReport) {
    assert_eq!(report.end, EpisodeEnd::ReachedTarget, "{report:?}");
    assert_eq!(report.acknowledged_through, report.target, "{report:?}");
}

fn blocked(report: &EpisodeReport) -> &Blocked {
    match &report.end {
        EpisodeEnd::Blocked(blocked) => blocked,
        EpisodeEnd::ReachedTarget => panic!("the episode was not blocked: {report:?}"),
    }
}

/// Grows the corpus past S with every commit kind the consumer must consume: a multi-descriptor commit, a supersession, a retirement, an empty commit, published rows, and optionally a control commit that registers a second consumer.
fn grow(corpus: &Corpus, with_second_consumer: bool) {
    corpus.publish(
        "many",
        &[
            ("msg-b", "1", "second message"),
            ("msg-c", "1", "third message"),
        ],
    );
    corpus.publish_outbox();
    corpus.publish("revise", &[("msg-a", "2", "first message, revised")]);
    corpus.retire("third", "third message");
    corpus.empty("one");
    if with_second_consumer {
        corpus.control("audit");
    }
}

/// Every window the driver acknowledges is checked against the ledger from outside: the projection holds the window before the kernel learns of it, and the kernel learns of exactly that window.
fn assert_each_window(data_home: &Path, root: &Path, ledger: &Ledger, event: EpisodeEvent) {
    match event {
        EpisodeEvent::LocalReleased { through } => {
            assert_matches_ledger(data_home, ledger, through);
            assert!(kernel_checkpoint(root) < through, "not yet acknowledged");
        }
        EpisodeEvent::Acknowledged { through } => {
            assert_eq!(kernel_checkpoint(root), through);
            assert_matches_ledger(data_home, ledger, through);
        }
        _ => {}
    }
}

#[test]
fn the_episode_consumes_every_commit_kind_and_the_ledger_predicts_each_window() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    corpus.publish("first", &[("msg-a", "1", "first message")]);
    let (projection, consumer, hold) = corpus.bootstrap(dir.path());
    assert_eq!(corpus.kernel_checkpoint(), hold.snapshot);
    assert_matches_ledger(dir.path(), &corpus.ledger(), hold.snapshot);

    grow(&corpus, true);
    let target = corpus.tip();
    let commits = corpus.commits_in(hold.snapshot, target);
    assert!(commits >= 7, "ingests, publishes, retire, empty, control");
    let published: i64 = inspect(&corpus.kernel_db())
        .query_row(
            "SELECT COUNT(*) FROM outbox WHERE commit_seq>?1 AND published_at IS NOT NULL",
            [hold.snapshot],
            |row| row.get(0),
        )
        .unwrap();
    assert!(published > 0, "the window holds published retained rows");

    let ledger = corpus.ledger();
    let mut trace = Vec::new();
    let mut driver = SearchCatchUp::new(&corpus.kernel, &projection);
    let report = driver
        .run_episode(&consumer, &two_commit_windows(), 3, &mut |event| {
            trace.push(event);
            assert_each_window(dir.path(), dir.path(), &ledger, event);
        })
        .unwrap();
    reached(&report);
    assert_eq!(report.target, target);
    assert_eq!(
        report.commits_consumed, commits,
        "every commit through the target"
    );
    assert_eq!(report.batches_applied, commits.div_ceil(2), "{report:?}");
    assert_eq!(corpus.kernel_checkpoint(), target);
    assert_matches_ledger(dir.path(), &ledger, target);

    // Each acknowledged window is a complete commit boundary the ledger knows.
    let acknowledged = windows(&trace);
    assert_eq!(acknowledged.len(), commits.div_ceil(2));
    assert_eq!(*acknowledged.last().unwrap(), target);
    assert!(acknowledged.windows(2).all(|pair| pair[0] < pair[1]));

    // A second episode finds nothing to do and moves nothing.
    let mut trace = Vec::new();
    let again = driver
        .run_episode(&consumer, &two_commit_windows(), 4, &mut |event| {
            trace.push(event)
        })
        .unwrap();
    reached(&again);
    assert_eq!((again.batches_applied, again.commits_consumed), (0, 0));
    assert!(trace.is_empty());
    assert_eq!(corpus.kernel_checkpoint(), target);
    assert_matches_ledger(dir.path(), &ledger, target);

    // A window of row-less commits advances both checkpoints and changes no row.
    corpus.empty("solo");
    corpus.control("second-audit");
    let quiet = corpus.tip();
    let report = driver
        .run_episode(&consumer, &bounds(), 5, &mut |_| {})
        .unwrap();
    reached(&report);
    assert_eq!((report.target, report.commits_consumed), (quiet, 2));
    assert_eq!(corpus.kernel_checkpoint(), quiet);
    assert_matches_ledger(dir.path(), &ledger, quiet);

    // New rows after a completed episode are picked up by the next one.
    corpus.publish("later", &[("msg-d", "1", "fourth message")]);
    let later = corpus.tip();
    let report = driver
        .run_episode(&consumer, &bounds(), 6, &mut |_| {})
        .unwrap();
    reached(&report);
    assert_eq!(report.target, later);
    assert_eq!(corpus.kernel_checkpoint(), later);
    assert_matches_ledger(dir.path(), &corpus.ledger(), later);
}

#[test]
fn refused_inputs_leave_checkpoint_and_acknowledgement_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    corpus.publish("first", &[("msg-a", "1", "first message")]);
    let (projection, consumer, hold) = corpus.bootstrap(dir.path());
    grow(&corpus, true);
    let target = corpus.tip();
    let before = durable(dir.path());
    let mut driver = SearchCatchUp::new(&corpus.kernel, &projection);
    let mut refused = |name: &str, consumer: &CatchUpConsumer, bounds: &EpisodeBounds| -> Blocked {
        let report = driver
            .run_episode(consumer, bounds, 3, &mut |_| {})
            .unwrap();
        assert_eq!(report.batches_applied, 0, "{name}: {report:?}");
        assert_eq!(corpus.kernel_checkpoint(), hold.snapshot, "{name}");
        assert_eq!(durable(dir.path()), before, "{name}");
        blocked(&report).clone()
    };

    // The first commit after S cannot fit one payload byte.
    let first = hold.snapshot + 1;
    let (rows, payload_bytes) = corpus.outbox_shape(first);
    assert!(payload_bytes > 1);
    let mut tiny = bounds();
    tiny.commits.max_payload_bytes = NonZeroU64::new(1).unwrap();
    assert_eq!(
        refused("oversized", &consumer, &tiny),
        Blocked::OversizedCommit {
            commit_seq: first,
            rows,
            payload_bytes,
        }
    );

    // The whole window is one batch that exceeds the local record bound.
    let mut narrow = bounds();
    narrow.batch.persist.max_records = NonZeroUsize::new(1).unwrap();
    assert!(matches!(
        refused("admission", &consumer, &narrow),
        Blocked::Admission(ProjectionError::TooManyRecords { .. })
    ));

    // The delta needs more source pages than one window may span.
    let mut paged = bounds();
    paged.source_page.max_rows = NonZeroUsize::new(1).unwrap();
    paged.max_source_pages = NonZeroUsize::new(1).unwrap();
    assert_eq!(
        refused("pages", &consumer, &paged),
        Blocked::SourcePagesExceeded { through: target }
    );

    // The projection was installed for another kernel.
    let other_kernel = CatchUpConsumer {
        kernel_incarnation_id: "kernel-2".to_string(),
        ..consumer.clone()
    };
    assert_eq!(
        refused("identity", &other_kernel, &bounds()),
        Blocked::ProjectionIdentity
    );

    // The projection's checkpoint names another hold.
    let other_hold = CatchUpConsumer {
        hold_id: "0123456789abcdef0123456789abcdef".to_string(),
        ..consumer.clone()
    };
    assert_eq!(
        refused("hold", &other_hold, &bounds()),
        Blocked::BaselineMismatch {
            hold_id: hold.hold_id.clone(),
            snapshot_commit_seq: hold.snapshot,
        }
    );

    // The multi-descriptor commit's ordinals are no longer 0..n.
    let many = corpus.ledger().commit_of("second message");
    let (many_rows, _) = corpus.outbox_shape(many);
    assert!(many_rows >= 2, "a multi-event commit");
    let last_ordinal = many_rows as i64 - 1;
    mutate(&corpus.kernel_db())
        .execute(
            "UPDATE outbox SET ordinal=ordinal+1 WHERE commit_seq=?1 AND ordinal=?2",
            [many, last_ordinal],
        )
        .unwrap();
    assert_eq!(
        refused("ordinals", &consumer, &bounds()),
        Blocked::Read(CommitReadError::MalformedOrdinals { commit_seq: many })
    );
    mutate(&corpus.kernel_db())
        .execute(
            "UPDATE outbox SET ordinal=ordinal-1 WHERE commit_seq=?1 AND ordinal=?2",
            [many, last_ordinal + 1],
        )
        .unwrap();

    // One retained event of that commit loses its outbox row.
    let deleted = mutate(&corpus.kernel_db())
        .execute(
            "DELETE FROM outbox WHERE commit_seq=?1 AND ordinal=?2",
            [many, last_ordinal],
        )
        .unwrap();
    assert_eq!(deleted, 1);
    assert_eq!(
        refused("history", &consumer, &bounds()),
        Blocked::Read(CommitReadError::MissingHistory { commit_seq: many })
    );
    assert_eq!(
        refused("history again", &consumer, &bounds()),
        Blocked::Read(CommitReadError::MissingHistory { commit_seq: many })
    );

    // A projection with no committed batch has no prefix to extend.
    let empty_home = tempfile::tempdir().unwrap();
    let fresh = SearchProjection::open(empty_home.path()).unwrap();
    fresh
        .write(|conn| install_identity(conn, &identity(KERNEL_INCARNATION), 1))
        .unwrap();
    let mut driver = SearchCatchUp::new(&corpus.kernel, &fresh);
    let report = driver
        .run_episode(&consumer, &bounds(), 3, &mut |_| {})
        .unwrap();
    assert_eq!(*blocked(&report), Blocked::NoLocalBaseline);
    assert_eq!(corpus.kernel_checkpoint(), hold.snapshot);
}

/// Acknowledging past a deletion would satisfy the search consumer's deletion barrier while the projection still serves the deleted text, so the window is refused and none of its commits move.
#[test]
fn a_window_holding_a_deletion_is_refused_and_leaves_its_barrier_unsatisfied() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    corpus.publish("first", &[("msg-a", "1", "first message")]);
    let (projection, consumer, _) = corpus.bootstrap(dir.path());

    // One ingest and one publish fill the first two-commit window; the deletion opens the next.
    let doomed = corpus.publish("doomed", &[("msg-b", "1", "doomed message")]);
    let deletion = corpus.delete("doomed", "evidence-doomed-0");
    assert_eq!(deletion.commit_seq, doomed + 1);
    assert_eq!(corpus.tip(), deletion.commit_seq);
    let ledger = corpus.ledger();
    assert!(
        ledger.tombstones.is_empty(),
        "the ledger predicts the row stays live until a tombstone can be projected"
    );

    let mut driver = SearchCatchUp::new(&corpus.kernel, &projection);
    let mut trace = Vec::new();
    let report = driver
        .run_episode(&consumer, &two_commit_windows(), 3, &mut |event| {
            trace.push(event);
            assert_each_window(dir.path(), dir.path(), &ledger, event);
        })
        .unwrap();
    assert_eq!(
        *blocked(&report),
        Blocked::DeletionUnpropagated {
            commit_seq: deletion.commit_seq,
        }
    );
    assert_eq!(report.target, deletion.commit_seq);
    assert_eq!((report.batches_applied, report.commits_consumed), (1, 2));
    assert_eq!(report.acknowledged_through, doomed);
    assert_eq!(windows(&trace), vec![doomed]);
    assert_eq!(corpus.kernel_checkpoint(), doomed);
    assert_matches_ledger(dir.path(), &ledger, doomed);
    assert!(
        durable(dir.path())
            .rows
            .values()
            .any(|text| text == "doomed message"),
        "the deleted text is still served, so the deletion must not count as propagated"
    );

    let barrier = corpus
        .kernel
        .deletion_barrier(&deletion.barrier_id)
        .unwrap();
    let search = barrier
        .consumers
        .iter()
        .find(|status| status.consumer_id == CONSUMER)
        .unwrap();
    assert_eq!(search.required_checkpoint_commit_seq, deletion.commit_seq);
    assert_eq!(search.checkpoint_commit_seq, Some(doomed));
    assert!(!search.satisfied, "{barrier:?}");
    assert!(!barrier.cleared, "{barrier:?}");

    let before = durable(dir.path());
    let again = driver
        .run_episode(&consumer, &two_commit_windows(), 4, &mut |_| {})
        .unwrap();
    assert_eq!(
        *blocked(&again),
        Blocked::DeletionUnpropagated {
            commit_seq: deletion.commit_seq,
        }
    );
    assert_eq!((again.batches_applied, again.commits_consumed), (0, 0));
    assert_eq!(corpus.kernel_checkpoint(), doomed);
    assert_eq!(durable(dir.path()), before);
    assert!(
        !corpus
            .kernel
            .deletion_barrier(&deletion.barrier_id)
            .unwrap()
            .cleared
    );
}

/// Probes the projection's write lock from an independent connection whenever the driver is about to take the kernel writer; an open local transaction makes the probe fail.
#[derive(Default)]
struct LockProbe {
    events: Vec<EpisodeEvent>,
    /// (window, the probe took the write lock)
    probes: Vec<(i64, bool)>,
}

impl LockProbe {
    fn observe(&mut self, search: &Path, event: EpisodeEvent) {
        self.events.push(event);
        let through = match event {
            EpisodeEvent::AcknowledgementRequested { through }
            | EpisodeEvent::HoldExtensionRequested { through } => through,
            _ => return,
        };
        let probe = Connection::open_with_flags(search, OpenFlags::SQLITE_OPEN_READ_WRITE).unwrap();
        probe.busy_timeout(Duration::ZERO).unwrap();
        let free = probe.execute_batch("BEGIN IMMEDIATE; ROLLBACK;").is_ok();
        self.probes.push((through, free));
    }

    fn all_free(&self) -> bool {
        self.probes.iter().all(|(_, free)| *free)
    }
}

fn released_before_every_acknowledgement(events: &[EpisodeEvent]) -> bool {
    let mut open = false;
    for event in events {
        match event {
            EpisodeEvent::LocalStaged { .. } => open = true,
            EpisodeEvent::LocalReleased { .. } => open = false,
            EpisodeEvent::AcknowledgementRequested { .. }
            | EpisodeEvent::HoldExtensionRequested { .. }
                if open =>
            {
                return false;
            }
            _ => {}
        }
    }
    true
}

#[test]
fn the_local_transaction_is_released_before_the_kernel_writer_is_taken() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    corpus.publish("first", &[("msg-a", "1", "first message")]);
    let (projection, consumer, _hold) = corpus.bootstrap(dir.path());
    grow(&corpus, true);
    let search = search_path(dir.path());

    let mut probe = LockProbe::default();
    let mut driver = SearchCatchUp::new(&corpus.kernel, &projection);
    let report = driver
        .run_episode(&consumer, &two_commit_windows(), 3, &mut |event| {
            probe.observe(&search, event)
        })
        .unwrap();
    reached(&report);
    assert!(probe.probes.len() >= 6, "{:?}", probe.probes);
    assert!(
        probe.all_free(),
        "the write lock was free at every kernel writer acquisition: {:?}",
        probe.probes
    );
    assert!(released_before_every_acknowledgement(&probe.events));
    // Every window crosses its boundaries in order.
    for through in windows(&probe.events) {
        let positions: Vec<usize> = [
            EpisodeEvent::HoldExtensionRequested { through },
            EpisodeEvent::LocalStaged { through },
            EpisodeEvent::LocalReleased { through },
            EpisodeEvent::AcknowledgementRequested { through },
            EpisodeEvent::Acknowledged { through },
        ]
        .iter()
        .map(|wanted| {
            probe
                .events
                .iter()
                .position(|event| event == wanted)
                .unwrap()
        })
        .collect();
        assert!(
            positions.windows(2).all(|pair| pair[0] < pair[1]),
            "{positions:?}"
        );
    }

    // Negative control: a driver that acknowledges inside the local
    // transaction is caught by the same probe and the same ordering check.
    corpus.publish("later", &[("msg-d", "1", "fourth message")]);
    let mut probe = LockProbe::default();
    let report = driver
        .run_episode_with_fault_for_test(
            &consumer,
            &bounds(),
            4,
            &mut |event| probe.observe(&search, event),
            EpisodeFault::AcknowledgeInsideLocalTransaction,
        )
        .unwrap();
    reached(&report);
    assert!(
        probe.probes.iter().any(|(_, free)| !free),
        "the probe must see the held write lock: {:?}",
        probe.probes
    );
    assert!(!released_before_every_acknowledgement(&probe.events));
    assert!(
        driver.quarantine().is_none(),
        "a kernel outcome is not projection corruption"
    );
}

/// Applies `(checkpoint, through]` to the projection without acknowledging it, which is what a lost acknowledgement leaves behind.
fn apply_without_acknowledging(
    corpus: &Corpus,
    projection: &SearchProjection,
    consumer: &CatchUpConsumer,
    hold: &SourceHold,
    through: i64,
) {
    corpus
        .kernel
        .extend_source_hold(
            &consumer.binding,
            &consumer.hold_id,
            through,
            hold_admission(),
        )
        .unwrap();
    let rows = export_all(
        &corpus.kernel,
        &consumer.binding,
        &consumer.hold_id,
        hold.captured_at,
        ExportWindow::Delta {
            after: corpus.kernel_checkpoint(),
            through,
        },
    );
    let identities = row_identities(&rows);
    let batch = batch_from_rows(
        &rows,
        &identities,
        MutationIdentity {
            kernel_incarnation_id: KERNEL_INCARNATION.to_string(),
            hold_id: consumer.hold_id.clone(),
            snapshot_commit_seq: hold.snapshot,
            through_commit_seq: through,
        },
        Some(GENERATION),
    )
    .unwrap();
    projection.apply_batch(&batch, batch_bounds(), 2).unwrap();
}

#[test]
fn lost_replies_and_busy_commits_reconcile_to_durable_facts() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    corpus.publish("first", &[("msg-a", "1", "first message")]);
    let (projection, consumer, hold) = corpus.bootstrap(dir.path());
    let mut driver = SearchCatchUp::new(&corpus.kernel, &projection);
    let search = search_path(dir.path());

    // A lost local COMMIT reply arriving as a store failure: the durable rows
    // say the window applied, so the episode acknowledges it without applying it twice.
    grow(&corpus, true);
    let target = corpus.tip();
    let report = driver
        .run_episode_with_fault_for_test(
            &consumer,
            &bounds(),
            3,
            &mut |_| {},
            EpisodeFault::LoseLocalCommitReply,
        )
        .unwrap();
    reached(&report);
    assert_eq!(report.batches_applied, 1);
    assert_eq!(corpus.kernel_checkpoint(), target);
    assert_matches_ledger(dir.path(), &corpus.ledger(), target);

    // A BUSY local COMMIT: another writer holds the projection, so the batch
    // never committed; nothing is acknowledged and nothing is rolled forward.
    corpus.publish("busy", &[("msg-e", "1", "fifth message")]);
    let busy_target = corpus.tip();
    let blocker = hold_write_lock(&search);
    let report = driver
        .run_episode(&consumer, &bounds(), 4, &mut |_| {})
        .unwrap();
    assert_eq!(
        *blocked(&report),
        Blocked::LocalCommitUnresolved,
        "{report:?}"
    );
    drop(blocker);
    assert_eq!(corpus.kernel_checkpoint(), target);
    assert_matches_ledger(dir.path(), &corpus.ledger(), target);
    let report = driver
        .run_episode(&consumer, &bounds(), 5, &mut |_| {})
        .unwrap();
    reached(&report);
    assert_eq!(corpus.kernel_checkpoint(), busy_target);
    assert_matches_ledger(dir.path(), &corpus.ledger(), busy_target);

    // A lost acknowledgement reply whose acknowledgement committed: the
    // kernel's durable checkpoint says so, and the episode continues.
    corpus.publish("lost-ack", &[("msg-f", "1", "sixth message")]);
    let lost_target = corpus.tip();
    let report = driver
        .run_episode_with_fault_for_test(
            &consumer,
            &bounds(),
            6,
            &mut |_| {},
            EpisodeFault::LoseAcknowledgementReply,
        )
        .unwrap();
    reached(&report);
    assert_eq!(corpus.kernel_checkpoint(), lost_target);

    // A BUSY acknowledgement that never committed: the kernel writer is held
    // from the moment the driver asks for it, the durable checkpoint has not
    // moved, and the local prefix keeps the window without any rollback.
    corpus.publish("busy-ack", &[("msg-g", "1", "seventh message")]);
    let busy_ack_target = corpus.tip();
    let mut kernel_blocker = None;
    let report = driver
        .run_episode(&consumer, &bounds(), 7, &mut |event| {
            if let EpisodeEvent::AcknowledgementRequested { .. } = event {
                kernel_blocker = Some(hold_write_lock(&corpus.kernel_db()));
            }
        })
        .unwrap();
    assert_eq!(
        *blocked(&report),
        Blocked::AcknowledgementUnresolved {
            through: busy_ack_target,
            kernel_checkpoint: Some(lost_target),
        },
        "{report:?}"
    );
    assert_eq!(report.batches_applied, 1);
    drop(kernel_blocker);
    assert_eq!(
        corpus.kernel_checkpoint(),
        lost_target,
        "no acknowledgement"
    );
    assert_matches_ledger(dir.path(), &corpus.ledger(), busy_ack_target);

    // A durable local prefix the kernel never acknowledged, with newer commits
    // behind it: the next episode acknowledges exactly that prefix before it
    // reads anything new, and only the new window is applied.
    corpus.publish("beyond", &[("msg-h", "1", "eighth message")]);
    let beyond = corpus.tip();
    let mut probe = LockProbe::default();
    let report = driver
        .run_episode(&consumer, &bounds(), 8, &mut |event| {
            probe.observe(&search, event)
        })
        .unwrap();
    reached(&report);
    assert_eq!(report.batches_applied, 1, "only the new window is applied");
    assert_eq!(windows(&probe.events), vec![busy_ack_target, beyond]);
    assert!(probe.all_free(), "{:?}", probe.probes);
    assert_eq!(corpus.kernel_checkpoint(), beyond);
    assert_matches_ledger(dir.path(), &corpus.ledger(), beyond);

    // The same shape produced outside the driver: a window applied and never acknowledged.
    corpus.publish("unacked", &[("msg-i", "1", "ninth message")]);
    let unacked = corpus.tip();
    apply_without_acknowledging(&corpus, &projection, &consumer, &hold, unacked);
    corpus.publish("after-unacked", &[("msg-j", "1", "tenth message")]);
    let after_unacked = corpus.tip();
    assert_eq!(corpus.kernel_checkpoint(), beyond);
    assert_eq!(durable(dir.path()).checkpoint, Some(unacked));
    let mut trace = Vec::new();
    let report = driver
        .run_episode(&consumer, &bounds(), 9, &mut |event| trace.push(event))
        .unwrap();
    reached(&report);
    assert_eq!(report.batches_applied, 1);
    assert_eq!(windows(&trace), vec![unacked, after_unacked]);
    assert_eq!(corpus.kernel_checkpoint(), after_unacked);
    assert_matches_ledger(dir.path(), &corpus.ledger(), after_unacked);

    // A kernel checkpoint ahead of the local prefix can never be repaired by a
    // later window; the episode names it and moves nothing.
    corpus.publish("ahead", &[("msg-k", "1", "eleventh message")]);
    let ahead = corpus.tip();
    corpus
        .kernel
        .acknowledge_outbox(CONSUMER, ahead, 10)
        .unwrap();
    let report = driver
        .run_episode(&consumer, &bounds(), 10, &mut |_| {})
        .unwrap();
    assert_eq!(
        *blocked(&report),
        Blocked::AcknowledgedBeyondLocalPrefix {
            local: after_unacked,
            acknowledged: ahead,
        }
    );
    assert_matches_ledger(dir.path(), &corpus.ledger(), after_unacked);
}

/// Runs kernel maintenance between the local commit and its acknowledgement, then checks the window's evidence is still readable.
struct MaintenanceBetween<'a> {
    corpus: &'a Corpus,
    consumer: &'a CatchUpConsumer,
    incarnation: kernel::CommitReadIncarnation,
    after: i64,
    now: i64,
    pruned: Option<usize>,
    history: Option<Result<i64, CommitReadError>>,
}

impl MaintenanceBetween<'_> {
    fn observe(&mut self, event: EpisodeEvent) {
        let EpisodeEvent::LocalReleased { through } = event else {
            return;
        };
        let result = self
            .corpus
            .kernel
            .run_staging_maintenance(self.now)
            .unwrap();
        assert_eq!(
            result.artifact_gc.reclaimed_objects, 0,
            "pinned and unacknowledged evidence is not reclaimed"
        );
        self.pruned = Some(self.corpus.kernel.prune_outbox().unwrap().deleted);
        self.history = Some(
            self.corpus
                .kernel
                .read_complete_commits(
                    &CommitReadRequest {
                        consumer_id: CONSUMER.to_string(),
                        incarnation: self.incarnation,
                        after_commit: self.after,
                        through_commit: through,
                    },
                    bounds().commits,
                )
                .map(|page| page.through),
        );
        let rows = export_all(
            &self.corpus.kernel,
            &self.consumer.binding,
            &self.consumer.hold_id,
            self.now,
            ExportWindow::Delta {
                after: self.after,
                through,
            },
        );
        assert!(
            rows.iter().any(|row| row.text.is_some()),
            "the window's source bytes are retained"
        );
    }
}

#[test]
fn kernel_maintenance_between_commit_and_ack_cannot_remove_the_window_it_has_not_acknowledged() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    corpus.publish("first", &[("msg-a", "1", "first message")]);
    let (projection, consumer, hold) = corpus.bootstrap(dir.path());
    // Only the search consumer is registered, so its checkpoint alone bounds pruning.
    grow(&corpus, false);
    let target = corpus.tip();
    // Fifteen days later the reclamation grace has passed for anything unpinned.
    let now = hold.captured_at + 15 * DAY_MS;
    let incarnation = corpus
        .kernel
        .capture_commit_read_target()
        .unwrap()
        .incarnation;
    let mut observer = MaintenanceBetween {
        corpus: &corpus,
        consumer: &consumer,
        incarnation,
        after: hold.snapshot,
        now,
        pruned: None,
        history: None,
    };
    let mut driver = SearchCatchUp::new(&corpus.kernel, &projection);
    let report = driver
        .run_episode(&consumer, &bounds(), hold.captured_at, &mut |event| {
            observer.observe(event)
        })
        .unwrap();
    reached(&report);
    assert!(
        observer.pruned.unwrap() > 0,
        "pruning ran against the acknowledged prefix"
    );
    assert_eq!(
        observer.history,
        Some(Ok(target)),
        "the window's history is retained"
    );
    assert_eq!(corpus.kernel_checkpoint(), target);
    assert_matches_ledger(dir.path(), &corpus.ledger(), target);

    // Negative control: a driver that acknowledges before the local release
    // lets the same maintenance prune the window's history.
    corpus.publish("later", &[("msg-d", "1", "fourth message")]);
    let mut observer = MaintenanceBetween {
        corpus: &corpus,
        consumer: &consumer,
        incarnation,
        after: target,
        now,
        pruned: None,
        history: None,
    };
    let report = driver
        .run_episode_with_fault_for_test(
            &consumer,
            &bounds(),
            hold.captured_at,
            &mut |event| observer.observe(event),
            EpisodeFault::AcknowledgeInsideLocalTransaction,
        )
        .unwrap();
    reached(&report);
    assert!(observer.pruned.unwrap() > 0);
    assert_eq!(
        observer.history,
        Some(Err(CommitReadError::BelowCheckpoint)),
        "the early acknowledgement released the window's history to pruning"
    );
}

#[test]
fn corruption_and_storage_failures_quarantine_the_driver_without_acknowledgement() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    corpus.publish("first", &[("msg-a", "1", "first message")]);
    let (projection, consumer, hold) = corpus.bootstrap(dir.path());
    grow(&corpus, true);
    let search = search_path(dir.path());
    let mut driver = SearchCatchUp::new(&corpus.kernel, &projection);

    // Corruption found while reconciling a lost commit reply quarantines the
    // driver: the window is not acknowledged and later episodes refuse.
    let mut corrupted = false;
    let error = driver
        .run_episode_with_fault_for_test(
            &consumer,
            &bounds(),
            3,
            &mut |event| {
                if let EpisodeEvent::LocalReleased { .. } = event
                    && !corrupted
                {
                    let changed = mutate(&search)
                        .execute(
                            "UPDATE payloads SET bytes=CAST('corrupt' AS BLOB), byte_length=7
                             WHERE payload_id=(SELECT payload_id FROM payloads ORDER BY created_at DESC, payload_id DESC LIMIT 1)",
                            [],
                        )
                        .unwrap();
                    assert_eq!(changed, 1);
                    corrupted = true;
                }
            },
            EpisodeFault::LoseLocalCommitReply,
        )
        .unwrap_err();
    let CatchUpError::Quarantined(quarantine) = error else {
        panic!("{error:?}");
    };
    assert_eq!(quarantine.kind, QuarantineKind::Integrity);
    assert_eq!(driver.quarantine(), Some(&quarantine));
    assert_eq!(
        corpus.kernel_checkpoint(),
        hold.snapshot,
        "no acknowledgement"
    );
    let again = driver
        .run_episode(&consumer, &bounds(), 4, &mut |_| panic!("no work runs"))
        .unwrap_err();
    assert!(matches!(again, CatchUpError::Quarantined(q) if q == quarantine));
    assert_eq!(corpus.kernel_checkpoint(), hold.snapshot);

    // A projection whose checkpoint cannot be read at all is a storage
    // failure, quarantined before any kernel writer is taken.
    let other = tempfile::tempdir().unwrap();
    let other_corpus = Corpus::open(other.path());
    other_corpus.seed();
    other_corpus.publish("first", &[("msg-a", "1", "first message")]);
    let (projection, consumer, hold) = other_corpus.bootstrap(other.path());
    grow(&other_corpus, true);
    mutate(&search_path(other.path()))
        .execute_batch("DROP TABLE projection_checkpoint")
        .unwrap();
    let mut driver = SearchCatchUp::new(&other_corpus.kernel, &projection);
    let error = driver
        .run_episode(&consumer, &bounds(), 3, &mut |_| panic!("no window runs"))
        .unwrap_err();
    let CatchUpError::Quarantined(quarantine) = error else {
        panic!("{error:?}");
    };
    assert_eq!(quarantine.kind, QuarantineKind::Storage);
    assert_eq!(other_corpus.kernel_checkpoint(), hold.snapshot);
}

// ---- Named-boundary process crashes ----------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cut {
    /// Inside the local transaction, after the batch's statements, before COMMIT.
    Staged,
    /// After the local transaction committed and released, before acknowledgement.
    Released,
    /// After the kernel acknowledged the window.
    Acknowledged,
}

impl Cut {
    fn name(self) -> &'static str {
        match self {
            Cut::Staged => "staged",
            Cut::Released => "released",
            Cut::Acknowledged => "acknowledged",
        }
    }

    fn parse(name: &str) -> Self {
        match name {
            "staged" => Cut::Staged,
            "released" => Cut::Released,
            "acknowledged" => Cut::Acknowledged,
            other => panic!("unknown cut {other}"),
        }
    }

    /// Prints the barrier and parks forever when `event` is this cut's boundary.
    fn barrier(self, event: EpisodeEvent) {
        let hit = matches!(
            (self, event),
            (Cut::Staged, EpisodeEvent::LocalStaged { .. })
                | (Cut::Released, EpisodeEvent::LocalReleased { .. })
                | (Cut::Acknowledged, EpisodeEvent::Acknowledged { .. })
        );
        if hit {
            let mut stdout = std::io::stdout().lock();
            writeln!(stdout, "{CHILD_BARRIER} {}", self.name()).unwrap();
            stdout.flush().unwrap();
            drop(stdout);
            loop {
                std::thread::park();
            }
        }
    }
}

/// The child owns the whole incarnation: it captures the hold, applies the baseline, grows the corpus, prints the ledger, and runs one episode that parks at the named boundary until the parent kills it.
#[test]
#[ignore = "re-executed by the crash-cut test with its environment set"]
fn crash_child_entrypoint_reexecuted_by_the_parent() {
    let root = PathBuf::from(std::env::var(CHILD_ROOT).unwrap());
    let cut = Cut::parse(&std::env::var(CHILD_CUT).unwrap());
    let corpus = Corpus::open(&root);
    corpus.seed();
    corpus.publish("first", &[("msg-a", "1", "first message")]);
    let (projection, consumer, hold) = corpus.bootstrap(&root);
    grow(&corpus, true);
    let target = corpus.tip();
    assert!(
        corpus.commits_in(hold.snapshot, target) <= bounds().commits.max_commits.get(),
        "the whole catch-up is one window under the default bounds"
    );
    let mut stdout = std::io::stdout().lock();
    for line in corpus.ledger().encode() {
        writeln!(stdout, "{line}").unwrap();
    }
    writeln!(
        stdout,
        "{CHILD_LEDGER} hold {} {} {target}",
        hold.hold_id, hold.snapshot
    )
    .unwrap();
    stdout.flush().unwrap();
    drop(stdout);
    let mut driver = SearchCatchUp::new(&corpus.kernel, &projection);
    let report = driver
        .run_episode(&consumer, &bounds(), 3, &mut |event| cut.barrier(event))
        .unwrap();
    panic!("the child was not killed at its barrier: {report:?}");
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

/// Runs the child to its barrier, kills it, and returns its ledger, hold id, S, and target.
fn run_crash_child(root: &Path, cut: Cut) -> (Ledger, String, i64, i64) {
    let mut child = ChildGuard(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "crash_child_entrypoint_reexecuted_by_the_parent",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_ROOT, root)
            .env(CHILD_CUT, cut.name())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let stdout = child.0.stdout.take().unwrap();
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut ledger = Vec::new();
        let mut barrier = None;
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            // The harness prints the test's name ahead of its first output line.
            if let Some(start) = line.find(CHILD_LEDGER) {
                ledger.push(line[start..].to_string());
            } else if line.contains(CHILD_BARRIER) {
                barrier = Some(line);
                break;
            }
        }
        let _ = tx.send((ledger, barrier));
    });
    let (lines, barrier) = rx.recv_timeout(Duration::from_secs(120)).unwrap();
    assert_eq!(
        barrier.as_deref(),
        Some(format!("{CHILD_BARRIER} {}", cut.name()).as_str())
    );
    // Dropping the guard kills and reaps the parked child.
    drop(child);
    let (hold, rows): (Vec<String>, Vec<String>) = lines
        .into_iter()
        .partition(|line| line.starts_with(&format!("{CHILD_LEDGER} hold ")));
    let hold: Vec<&str> = hold[0].split(' ').collect();
    (
        Ledger::decode(&rows),
        hold[2].to_string(),
        hold[3].parse().unwrap(),
        hold[4].parse().unwrap(),
    )
}

#[test]
fn crash_cuts_recover_to_the_ledger_after_two_reopens_and_never_acknowledge_early() {
    for cut in [Cut::Staged, Cut::Released, Cut::Acknowledged] {
        let dir = tempfile::tempdir().unwrap();
        let (ledger, hold_id, snapshot, target) = run_crash_child(dir.path(), cut);
        // The child asserted that every commit after S is one window.
        let (expected_local, expected_kernel) = match cut {
            Cut::Staged => (snapshot, snapshot),
            Cut::Released => (target, snapshot),
            Cut::Acknowledged => (target, target),
        };
        for reopen in 0..2 {
            let kernel = KernelStore::open(dir.path().join("kernel")).unwrap();
            let projection = SearchProjection::open(dir.path()).unwrap();
            assert_eq!(
                kernel_checkpoint(dir.path()),
                expected_kernel,
                "{cut:?} reopen {reopen}: kernel checkpoint"
            );
            assert_matches_ledger(dir.path(), &ledger, expected_local);
            assert!(
                kernel_checkpoint(dir.path()) <= durable(dir.path()).checkpoint.unwrap(),
                "the acknowledgement never exceeds the durable local prefix"
            );

            // The hold belongs to the crashed incarnation. An episode in the
            // new one moves nothing: a durable-but-unacknowledged prefix stays
            // unacknowledged until a new hold is captured, and an acknowledged
            // prefix at the target needs no hold at all. Resuming under a new
            // hold is the lifecycle owner's replacement path, not this driver's.
            let consumer = CatchUpConsumer {
                binding: SourceHoldBinding {
                    consumer_id: CONSUMER.to_string(),
                    lease_epoch: kernel.lease_epoch(),
                    source_policy_version: POLICY.to_string(),
                },
                hold_id: hold_id.clone(),
                kernel_incarnation_id: KERNEL_INCARNATION.to_string(),
                generation_id: Some(GENERATION.to_string()),
            };
            let mut driver = SearchCatchUp::new(&kernel, &projection);
            let report = driver
                .run_episode(&consumer, &bounds(), 4, &mut |_| {})
                .unwrap();
            match cut {
                Cut::Staged => assert_eq!(
                    *blocked(&report),
                    Blocked::HoldExtension(SourceHoldError::BindingMismatch),
                    "{report:?}"
                ),
                Cut::Released => assert_eq!(
                    *blocked(&report),
                    Blocked::Acknowledgement(SourceHoldError::BindingMismatch),
                    "{report:?}"
                ),
                Cut::Acknowledged => {
                    reached(&report);
                    assert_eq!(report.batches_applied, 0);
                }
            }
            assert_eq!(kernel_checkpoint(dir.path()), expected_kernel);
            assert_matches_ledger(dir.path(), &ledger, expected_local);
            drop(driver);
            drop(projection);
            drop(kernel);
        }
    }
}
