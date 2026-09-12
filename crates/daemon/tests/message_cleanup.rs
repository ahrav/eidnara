//! Bounded message-index cleanup against a real kernel and a real projection: an independent ledger of eligible and protected rows predicts exactly what one slice removes; a replayed catch-up window, a repeated slice, and a reopened projection resurrect nothing; bounds and the original budget stop admission without a partial page.

use std::collections::BTreeSet;
use std::num::{NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

mod support;

use daemon::harness_sources::{Representation, SourcePublisher, SourceUnit};
use daemon::message_cleanup::{CleanupBounds, CleanupStop, MessageCleanup};
use daemon::search_projection::SearchProjection;
use kernel::applicability::EvalBudget;
use kernel::source_identity::OccurrenceClass;
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
const DOMAIN: &str = "chat";
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const MODEL: &str = "tiny-test-model";
const FINGERPRINT: &str = "a2b4c6d8e0f01234a2b4c6d8e0f01234a2b4c6d8e0f01234a2b4c6d8e0f01234";
const GENERATION: &str = "gen-1";
const NOW: i64 = 1_000;

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "daemon-message-cleanup-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

fn bounds() -> CleanupBounds {
    CleanupBounds {
        page_rows: NonZeroUsize::new(2).unwrap(),
        max_pages: NonZeroUsize::new(8).unwrap(),
        max_reclaimed: NonZeroUsize::new(16).unwrap(),
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

fn message(id: &str, revision: i64, text: &str) -> SourceUnit {
    SourceUnit {
        class: OccurrenceClass::Messages,
        identity: vec![
            ("project_id", PROJECT.to_string()),
            ("harness", "opencode".to_string()),
            ("session_id", "ses_1".to_string()),
            ("message_id", id.to_string()),
            ("block_index", "0".to_string()),
        ],
        revision: revision.to_string(),
        representation: Representation::Text,
        text: text.to_string(),
        role: "user".to_string(),
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
                    name: "Chat".to_string(),
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

    /// Publishes one message unit and returns its occurrence id and descriptor object id.
    fn publish(&self, unit: &SourceUnit) -> (String, String) {
        let published = SourcePublisher {
            kernel: &self.kernel,
            domain_id: DOMAIN,
            scope_id: Some(SCOPE),
            egress: ProviderEgress::LocalOnly,
            sensitivity: Sensitivity::Normal,
        }
        .publish(unit, NOW)
        .unwrap();
        (published.occurrence_id, published.object_id)
    }

    fn retire(&self, descriptor_object_id: &str) {
        self.kernel
            .commit(
                intent(&format!("retire:{descriptor_object_id}")),
                |envelope| {
                    envelope.retire_observation(descriptor_object_id)?;
                    Ok(String::new())
                },
            )
            .unwrap();
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

    /// Applies `rows` as the window through `through` and returns the batch's rows for replay.
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

    /// Extends the hold and applies one window: the catch-up from S when `after` is `None`, otherwise the delta after `after`, through `through`.
    fn catch_up(
        &self,
        projection: &SearchProjection,
        hold: &SourceHold,
        after: Option<i64>,
        through: i64,
    ) -> (Vec<SourceRow>, i64) {
        self.kernel
            .extend_source_hold(&self.binding(), &hold.hold_id, through, hold_admission())
            .unwrap();
        let window = match after {
            None => ExportWindow::CatchUp { through },
            Some(after) => ExportWindow::Delta { after, through },
        };
        let delta = self.export_window(hold, window);
        self.apply(projection, hold, &delta, through);
        (delta, through)
    }
}

/// Projection state through an independent connection: `(occurrence_id, tombstoned, job rows, payload_id)` for every occurrence row.
fn rows(data_home: &Path) -> BTreeSet<(String, bool, i64, String)> {
    Connection::open_with_flags(
        data_home.join("search").join("search.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
    .prepare(
        "SELECT o.occurrence_id,t.occurrence_id IS NOT NULL,
                (SELECT COUNT(*) FROM embedding_jobs j WHERE j.occurrence_id=o.occurrence_id),
                o.payload_id
         FROM occurrences o LEFT JOIN occurrence_tombstones t ON t.occurrence_id=o.occurrence_id",
    )
    .unwrap()
    .query_map([], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    })
    .unwrap()
    .collect::<rusqlite::Result<_>>()
    .unwrap()
}

fn payloads(data_home: &Path) -> BTreeSet<String> {
    Connection::open_with_flags(
        data_home.join("search").join("search.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
    .prepare("SELECT payload_id FROM payloads")
    .unwrap()
    .query_map([], |row| row.get(0))
    .unwrap()
    .collect::<rusqlite::Result<_>>()
    .unwrap()
}

/// Removes the finished job rows of every tombstoned occurrence except `keep`, as the identity sweep does before cleanup can consider them.
fn sweep_except(projection: &SearchProjection, keep: &str) {
    projection
        .write(|conn| {
            let page =
                retrieval::identity_sweep::candidates(conn, NonZeroUsize::new(64).unwrap(), None)?;
            let candidates: Vec<_> = page
                .candidates
                .into_iter()
                .filter(|candidate| candidate.occurrence_id != keep)
                .collect();
            retrieval::identity_sweep::reclaim(conn, &candidates)?;
            Ok(())
        })
        .unwrap();
}

/// The fixture every test builds: six message occurrences in a projection whose catch-up tombstoned four of them.
struct Fixture {
    dir: tempfile::TempDir,
    corpus: Corpus,
    projection: Option<SearchProjection>,
    hold: SourceHold,
    /// The acknowledged window that carried the eligible tombstones, an older prefix once the current window applied.
    window: (Vec<SourceRow>, i64),
    /// Live rows: the survivor of a revision, the survivor of a shared payload, and an untouched message.
    live: BTreeSet<String>,
    /// Tombstoned rows the ledger predicts cleanup removes.
    eligible: BTreeSet<String>,
    /// Tombstoned rows the ledger predicts cleanup keeps: one still carrying a finished job row, one whose job the host still holds, one above the acknowledged prefix, and one of another class.
    held: String,
    admitted: String,
    late: String,
    tool: String,
    /// The acknowledged prefix the slices judge against: every tombstone but `late`'s.
    acknowledged: i64,
}

impl Fixture {
    fn build() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let corpus = Corpus::open(dir.path());
        corpus.seed();
        let shared = "shared words";
        let (revised_old, _) = corpus.publish(&message("m-revised", 1, "first words"));
        let (retired, retired_object) = corpus.publish(&message("m-retired", 1, "retired words"));
        let (held, held_object) = corpus.publish(&message("m-held", 1, "held words"));
        let (shared_old, shared_old_object) = corpus.publish(&message("m-shared-old", 1, shared));
        let (shared_live, _) = corpus.publish(&message("m-shared-live", 1, shared));
        let (untouched, _) = corpus.publish(&message("m-untouched", 1, "untouched words"));
        let (late, late_object) = corpus.publish(&message("m-late", 1, "late words"));
        let (admitted, admitted_object) =
            corpus.publish(&message("m-admitted", 1, "admitted words"));
        let (tool, tool_object) = corpus.publish(&SourceUnit {
            class: OccurrenceClass::RawToolSpans,
            identity: vec![
                ("project_id", PROJECT.to_string()),
                ("harness", "opencode".to_string()),
                ("session_id", "ses_1".to_string()),
                ("parent_message_id", "msg_1".to_string()),
                ("tool_call_id", "call_1".to_string()),
                ("result_revision", "1".to_string()),
                ("block_index", "0".to_string()),
            ],
            revision: "1".to_string(),
            representation: Representation::ToolOutput,
            text: "tool words".to_string(),
            role: "toolResult".to_string(),
        });
        let hold = corpus.capture();
        let projection = corpus.bootstrap(dir.path(), &hold);
        // Revise one, retire two, retire the shared-payload twin; then, above the acknowledged prefix, retire the late message.
        let (revised_new, _) = corpus.publish(&message("m-revised", 2, "second words"));
        corpus.retire(&retired_object);
        corpus.retire(&held_object);
        corpus.retire(&admitted_object);
        corpus.retire(&tool_object);
        corpus.retire(&shared_old_object);
        let acknowledged = corpus.kernel.tip().unwrap();
        // The first window ends at the acknowledged prefix; the late retirement lands in the current, unacknowledged window.
        let window = corpus.catch_up(&projection, &hold, None, acknowledged);
        corpus.retire(&late_object);
        let tip = corpus.kernel.tip().unwrap();
        corpus.catch_up(&projection, &hold, Some(acknowledged), tip);
        sweep_except(&projection, &held);
        // A job the host still holds: its row stays in `admitted` whatever the tombstone says.
        projection
            .write(|conn| {
                conn.execute(
                    "INSERT INTO embedding_jobs(job_id,occurrence_id,generation_id,state,attempts,created_at,updated_at)
                     VALUES ('held-by-host',?1,?2,'admitted',1,1,1)",
                    rusqlite::params![admitted, GENERATION],
                )?;
                Ok(())
            })
            .unwrap();
        Self {
            live: [revised_new, shared_live, untouched].into_iter().collect(),
            eligible: [revised_old, retired, shared_old].into_iter().collect(),
            held,
            admitted,
            late,
            tool,
            acknowledged,
            window,
            dir,
            corpus,
            projection: Some(projection),
            hold,
        }
    }

    fn data_home(&self) -> &Path {
        self.dir.path()
    }

    fn projection(&self) -> &SearchProjection {
        self.projection.as_ref().unwrap()
    }

    /// Closes the projection and opens it again from its files.
    fn reopen(&mut self) {
        drop(self.projection.take());
        self.projection = Some(SearchProjection::open(self.dir.path()).unwrap());
    }

    fn tombstoned(&self) -> BTreeSet<String> {
        rows(self.data_home())
            .into_iter()
            .filter(|row| row.1)
            .map(|row| row.0)
            .collect()
    }

    fn present(&self) -> BTreeSet<String> {
        rows(self.data_home())
            .into_iter()
            .map(|row| row.0)
            .collect()
    }
}

fn unbounded() -> EvalBudget {
    EvalBudget::unbounded()
}

/// AC1, AC3: the ledger predicts exactly the removed rows; live rows, the tombstone still carrying a job row, the tombstone above the acknowledged prefix, and the shared payload's live twin stay; the payload of the removed twin survives through the live one; a repeated slice, a replayed catch-up window, and a reopened projection change nothing more.
#[test]
fn cleanup_removes_exactly_the_eligible_rows_and_replays_resurrect_nothing() {
    let mut fixture = Fixture::build();
    let before = rows(fixture.data_home());
    assert_eq!(before.len(), 10, "nine originals and one revision");
    let kept_tombstones = [
        &fixture.held,
        &fixture.admitted,
        &fixture.late,
        &fixture.tool,
    ];
    let all_tombstoned: BTreeSet<String> = fixture
        .eligible
        .iter()
        .chain(kept_tombstones)
        .cloned()
        .collect();
    assert_eq!(fixture.tombstoned(), all_tombstoned);
    assert_eq!(
        before.iter().find(|row| row.0 == fixture.held).unwrap().2,
        1,
        "the held row still carries its job"
    );
    let shared_payload = before
        .iter()
        .find(|row| {
            fixture.live.contains(&row.0)
                && row.3
                    == before
                        .iter()
                        .find(|r| fixture.eligible.contains(&r.0) && r.3 == row.3)
                        .map(|r| r.3.clone())
                        .unwrap_or_default()
        })
        .map(|row| row.3.clone());
    let payloads_before = payloads(fixture.data_home());

    let mut cleanup = MessageCleanup::new(fixture.projection(), fixture.acknowledged);
    let report = cleanup
        .run_slice(
            &support::projection_gate::open_gate(),
            bounds(),
            &unbounded(),
        )
        .unwrap();
    assert_eq!(report.stop, None, "{report:?}");
    assert_eq!(report.cursor, None, "the scan was exhausted");
    assert_eq!(
        report.reclaimed.occurrences,
        fixture.eligible.len(),
        "{report:?}"
    );
    assert_eq!(report.reclaimed.protected, 0);
    assert!(
        report.reclaimed.occurrences > 0,
        "a no-op slice is not cleanup"
    );
    let after = fixture.present();
    let expected: BTreeSet<String> = fixture
        .live
        .iter()
        .chain(kept_tombstones)
        .cloned()
        .collect();
    assert_eq!(after, expected);
    assert_eq!(
        fixture.tombstoned(),
        kept_tombstones.iter().map(|id| (*id).clone()).collect(),
        "a finished job row, a held job row, a tombstone above the prefix, and another class all stay"
    );
    // The removed rows' payloads are gone except the one a live twin still references.
    let payloads_after = payloads(fixture.data_home());
    assert_eq!(
        payloads_before.len() - payloads_after.len(),
        report.reclaimed.payloads
    );
    assert_eq!(
        report.reclaimed.payloads, 2,
        "the revised and retired texts; the shared text stays"
    );
    if let Some(shared) = shared_payload {
        assert!(payloads_after.contains(&shared));
    }

    let again = MessageCleanup::new(fixture.projection(), fixture.acknowledged)
        .run_slice(
            &support::projection_gate::open_gate(),
            bounds(),
            &unbounded(),
        )
        .unwrap();
    assert_eq!(
        (again.reclaimed.occurrences, again.reclaimed.payloads),
        (0, 0),
        "{again:?}"
    );
    assert_eq!(fixture.present(), expected);

    // Replaying the acknowledged window is an older prefix below the checkpoint: nothing comes back and no job is created.
    let jobs_before_replay: i64 = rows(fixture.data_home()).iter().map(|row| row.2).sum();
    fixture.corpus.apply(
        fixture.projection(),
        &fixture.hold,
        &fixture.window.0,
        fixture.window.1,
    );
    assert_eq!(fixture.present(), expected);
    let jobs_after_replay: i64 = rows(fixture.data_home()).iter().map(|row| row.2).sum();
    assert_eq!(
        jobs_after_replay, jobs_before_replay,
        "the replayed window queued nothing"
    );
    fixture.reopen();
    let reopened = fixture.projection();
    let mut cleanup = MessageCleanup::new(reopened, fixture.acknowledged);
    let report = cleanup
        .run_slice(
            &support::projection_gate::open_gate(),
            bounds(),
            &unbounded(),
        )
        .unwrap();
    assert_eq!(report.reclaimed.occurrences, 0);
    assert_eq!(
        rows(fixture.data_home())
            .into_iter()
            .map(|row| row.0)
            .collect::<BTreeSet<_>>(),
        expected
    );

    // The late tombstone becomes eligible once the acknowledged prefix covers it; the held ones never do while their job rows exist, and the other class never does. A prefix claimed above the projection's own checkpoint is capped by the store.
    let tip = fixture.corpus.kernel.tip().unwrap();
    let report = MessageCleanup::new(reopened, tip + 1_000)
        .run_slice(
            &support::projection_gate::open_gate(),
            bounds(),
            &unbounded(),
        )
        .unwrap();
    assert_eq!(report.reclaimed.occurrences, 1, "{report:?}");
    assert_eq!(
        fixture.tombstoned(),
        [
            fixture.held.clone(),
            fixture.admitted.clone(),
            fixture.tool.clone()
        ]
        .into_iter()
        .collect()
    );
}

/// AC4, AC5: a row bound stops the slice after exactly that many rows with a cursor the next slice resumes from; the original budget's cancellation and deadline stop admission before any page and cannot be renewed by the same identity, while a fresh budget proceeds.
#[test]
fn bounds_and_the_original_budget_stop_admission_without_partial_pages() {
    let fixture = Fixture::build();
    let expected_final: BTreeSet<String> = fixture
        .live
        .iter()
        .chain([
            &fixture.held,
            &fixture.admitted,
            &fixture.late,
            &fixture.tool,
        ])
        .cloned()
        .collect();

    let cancelled = unbounded();
    cancelled.cancel();
    let mut cleanup = MessageCleanup::new(fixture.projection(), fixture.acknowledged);
    let report = cleanup
        .run_slice(&support::projection_gate::open_gate(), bounds(), &cancelled)
        .unwrap();
    assert_eq!(report.stop, Some(CleanupStop::Cancelled));
    assert_eq!((report.inspected, report.reclaimed.occurrences), (0, 0));
    assert_eq!(fixture.present().len(), 10);
    // Sticky cancellation: the same budget refuses again; a deadline that passed refuses too and is never renewed.
    let report = cleanup
        .run_slice(&support::projection_gate::open_gate(), bounds(), &cancelled)
        .unwrap();
    assert_eq!(report.stop, Some(CleanupStop::Cancelled));
    let expired = EvalBudget::new(Some(Instant::now()), Arc::new(AtomicBool::new(false)));
    std::thread::sleep(std::time::Duration::from_millis(2));
    let report = cleanup
        .run_slice(&support::projection_gate::open_gate(), bounds(), &expired)
        .unwrap();
    assert_eq!(report.stop, Some(CleanupStop::Cancelled));
    let report = cleanup
        .run_slice(&support::projection_gate::open_gate(), bounds(), &expired)
        .unwrap();
    assert_eq!(
        report.stop,
        Some(CleanupStop::Cancelled),
        "a passed deadline does not come back"
    );
    assert_eq!(fixture.present().len(), 10);

    // One row per slice: each slice reclaims exactly one and hands the cursor on.
    let one = CleanupBounds {
        max_reclaimed: NonZeroUsize::new(1).unwrap(),
        ..bounds()
    };
    let mut removed = BTreeSet::new();
    let mut cursor = None;
    for _ in 0..fixture.eligible.len() {
        let mut slice = MessageCleanup::new(fixture.projection(), fixture.acknowledged)
            .resuming(cursor.clone());
        let report = slice
            .run_slice(&support::projection_gate::open_gate(), one, &unbounded())
            .unwrap();
        assert_eq!(report.reclaimed.occurrences, 1, "{report:?}");
        assert_eq!(report.stop, Some(CleanupStop::BoundReached));
        assert!(report.cursor.is_some());
        cursor = report.cursor;
        let now = fixture.present();
        let gone: BTreeSet<String> = fixture.eligible.difference(&now).cloned().collect();
        assert_eq!(gone.len(), removed.len() + 1);
        removed = gone;
    }
    let mut last = MessageCleanup::new(fixture.projection(), fixture.acknowledged).resuming(cursor);
    let report = last
        .run_slice(&support::projection_gate::open_gate(), one, &unbounded())
        .unwrap();
    assert_eq!(report.reclaimed.occurrences, 0);
    assert_eq!(report.cursor, None, "the scan is exhausted");
    assert_eq!(fixture.present(), expected_final);

    // A grant revoked between pages stops the slice at the next admission: the first page stands, nothing later is inspected.
    let fixture = Fixture::build();
    let gate = support::projection_gate::open_gate();
    let closer = Arc::clone(&gate);
    let mut revoked = MessageCleanup::new(fixture.projection(), fixture.acknowledged)
        .with_after_page_for_test(move || closer.close());
    let report = revoked.run_slice(&gate, bounds(), &unbounded()).unwrap();
    assert_eq!(report.stop, Some(CleanupStop::Cancelled), "{report:?}");
    assert_eq!(report.inspected, 2, "one page, then the revoked grant");
    assert_eq!(
        fixture.present().len(),
        10 - report.reclaimed.occurrences,
        "only the committed page's rows are gone"
    );

    // A page bound of one page per slice inspects only one page and resumes.
    let fixture = Fixture::build();
    let page = CleanupBounds {
        max_pages: NonZeroUsize::new(1).unwrap(),
        ..bounds()
    };
    let mut slice = MessageCleanup::new(fixture.projection(), fixture.acknowledged);
    let report = slice
        .run_slice(&support::projection_gate::open_gate(), page, &unbounded())
        .unwrap();
    assert_eq!(report.inspected, 2);
    assert_eq!(report.stop, Some(CleanupStop::BoundReached));
    assert!(report.cursor.is_some());
}
