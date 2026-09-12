//! One coverage report over a real kernel and a real projection: independent per-class ledgers predict lexical presence, dense requirement, valid vectors, missing, pending, and missing-without-pending for all five classes; stale vectors, obsolete pending, and vectors awaiting bookkeeping fall where the sets say; unavailability replaces any mixed observation; kernel exclusions ride beside the counts per class; a reopened projection reports the same.

use std::collections::BTreeSet;
use std::num::{NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use daemon::claim_sources::ClaimMaterializer;
use daemon::coverage::{ProjectionCoverage, ReportUnavailable, observe_coverage};
use daemon::harness_sources::{Representation, SourcePublisher, SourceUnit};
use daemon::search_projection::SearchProjection;
use kernel::source_identity::OccurrenceClass;
use kernel::{
    AdmissionEvent, AdmissionRequest, ArtifactDestination, CommitIntent, CommitPageBounds,
    DecisionPayload, DecisionSpec, Dimension, DomainSpec, EligibilityVerdict, EventKind,
    ExportWindow, KernelStore, ProjectScope, ProviderEgress, ScopeSpec, ScopeTermSpec, Sensitivity,
    SourceClass, SourceHold, SourceHoldAdmission, SourceHoldBinding, SourceHoldBounds,
    SourcePageBounds, SourceRow, TaintClass,
};
use retrieval::batch::{
    BatchBounds, MutationIdentity, VectorGeneration, batch_from_rows, register_generation,
    row_identities,
};
use retrieval::coverage::{ClassCoverage, CoverageBounds, CoverageUnavailable, DenseDisposition};
use retrieval::{PersistBounds, ProjectionIdentity, install_identity};
use rusqlite::{Connection, OpenFlags, params};
use sha2::{Digest, Sha256};

const CONSUMER: &str = "search";
const POLICY: &str = "source-policy.v1";
const PROJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SCOPE: &str = "project:a";
const MEMORY: &str = "memory";
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const MODEL: &str = "tiny-test-model";
const FINGERPRINT: &str = "a2b4c6d8e0f01234a2b4c6d8e0f01234a2b4c6d8e0f01234a2b4c6d8e0f01234";
const GENERATION: &str = "gen-1";
const DIMS: u32 = 8;
const NOW: i64 = 1_000;

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "daemon-coverage-report-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

fn generation() -> VectorGeneration {
    VectorGeneration {
        generation_id: GENERATION.to_string(),
        embedding_model: MODEL.to_string(),
        tokenizer_fingerprint: FINGERPRINT.to_string(),
        vector_dimension: DIMS,
        generation_epoch: 1,
    }
}

fn identity(kernel_incarnation_id: &str) -> ProjectionIdentity {
    ProjectionIdentity {
        schema_version: retrieval::SCHEMA_VERSION,
        kernel_incarnation_id: kernel_incarnation_id.to_string(),
        projection_policy_version: POLICY.to_string(),
        identity_contract_version: "search-projection-identity-v2".to_string(),
        limit_manifest_protocol_version: "limits.v1".to_string(),
        embedding_model: MODEL.to_string(),
        tokenizer_fingerprint: FINGERPRINT.to_string(),
        vector_dimension: DIMS,
        generation_epoch: 1,
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

fn bounds() -> CoverageBounds {
    CoverageBounds {
        max_live_per_class: NonZeroUsize::new(64).unwrap(),
        max_tombstoned_per_class: NonZeroUsize::new(64).unwrap(),
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

fn tool(call: &str, text: &str) -> SourceUnit {
    SourceUnit {
        class: OccurrenceClass::RawToolSpans,
        identity: vec![
            ("project_id", PROJECT.to_string()),
            ("harness", "opencode".to_string()),
            ("session_id", "ses_1".to_string()),
            ("parent_message_id", "msg_1".to_string()),
            ("tool_call_id", call.to_string()),
            ("result_revision", "1".to_string()),
            ("block_index", "0".to_string()),
        ],
        revision: "1".to_string(),
        representation: Representation::ToolOutput,
        text: text.to_string(),
        role: "toolResult".to_string(),
    }
}

fn commit(oid: &str, text: &str) -> SourceUnit {
    SourceUnit {
        class: OccurrenceClass::GitCommits,
        identity: vec![
            ("repository_id", "repo".to_string()),
            ("object_format", "sha1".to_string()),
            ("oid", oid.to_string()),
        ],
        revision: "1".to_string(),
        representation: Representation::CommitMessage,
        text: text.to_string(),
        role: "commit".to_string(),
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
                    domain_id: MEMORY.to_string(),
                    object_id: format!("{MEMORY}-object"),
                    name: "Memory".to_string(),
                    source_kind: "fixture".to_string(),
                    source_id: MEMORY.to_string(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                })?;
                envelope.insert_scope(ScopeSpec {
                    scope_id: SCOPE.to_string(),
                    object_id: SCOPE.to_string(),
                    source_id: SCOPE.to_string(),
                    domain_id: MEMORY.to_string(),
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
        ClaimMaterializer::register(&self.kernel, NOW).unwrap();
    }

    /// Publishes one unit and returns its occurrence id and descriptor object id.
    fn publish(&self, unit: &SourceUnit) -> (String, String) {
        let published = SourcePublisher {
            kernel: &self.kernel,
            domain_id: MEMORY,
            scope_id: Some(SCOPE),
            egress: ProviderEgress::LocalOnly,
            sensitivity: Sensitivity::Normal,
        }
        .publish(unit, NOW)
        .unwrap();
        (published.occurrence_id, published.object_id)
    }

    /// One admitted positive-category memory decision, materialized into two claim occurrences and one promoted-memory occurrence.
    fn decide(&self, object_id: &str, summary: &str, rationale: &str) {
        let spec = DecisionSpec {
            decision_id: format!("{object_id}-decision"),
            object_id: object_id.to_string(),
            domain_id: MEMORY.to_string(),
            proposition_id: None,
            scope_id: Some(SCOPE.to_string()),
            anchor_id: None,
            evidence_id: None,
            decision_kind: "PROJECT_RULES".to_string(),
            payload: DecisionPayload {
                summary: summary.to_string(),
                rationale: rationale.to_string(),
            },
            source_kind: "assistant".to_string(),
            source_id: format!("{object_id}-lineage"),
            source_revision: 1,
            sensitivity: Sensitivity::Normal,
        };
        self.kernel
            .commit(intent(&format!("decide:{object_id}")), |envelope| {
                envelope.insert_decision(spec.clone())?;
                envelope.record_admission(AdmissionRequest {
                    candidate_id: None,
                    subject_object_id: Some(object_id.to_string()),
                    source_class: Some(SourceClass::ExplicitUser),
                    taint_class: Some(TaintClass::UserExplicit),
                    event: AdmissionEvent {
                        kind: EventKind::Other,
                        trigger_object_id: None,
                        approval_object_id: None,
                        evidence_id: None,
                        reason: "fixture".to_string(),
                    },
                })?;
                Ok(String::new())
            })
            .unwrap();
        let report = ClaimMaterializer::new(&self.kernel, ProviderEgress::LocalOnly)
            .run_episode(
                CommitPageBounds {
                    max_commits: NonZeroUsize::new(8).unwrap(),
                    max_rows: NonZeroUsize::new(64).unwrap(),
                    max_payload_bytes: NonZeroU64::new(1 << 20).unwrap(),
                },
                NOW,
            )
            .unwrap();
        assert_eq!(report.published, 3, "{report:?}");
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
        let incarnation = self.kernel_incarnation_id();
        projection
            .write(|conn| {
                install_identity(conn, &identity(&incarnation), 1)?;
                register_generation(conn, &generation(), 1)?;
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

    fn catch_up(&self, projection: &SearchProjection, hold: &SourceHold) {
        let through = self.kernel.tip().unwrap();
        self.kernel
            .extend_source_hold(&self.binding(), &hold.hold_id, through, hold_admission())
            .unwrap();
        let delta = self.export_window(hold, ExportWindow::CatchUp { through });
        self.apply(projection, hold, &delta, through);
    }

    fn observe(
        &self,
        projection: &SearchProjection,
    ) -> Result<ProjectionCoverage, ReportUnavailable> {
        observe_coverage(
            projection,
            &self.kernel,
            &self.kernel_incarnation_id(),
            &ProjectScope::new(PROJECT).unwrap(),
            ArtifactDestination::Local,
            &generation(),
            bounds(),
        )
        .unwrap()
    }
}

/// Live occurrence ids per class through an independent connection.
fn live(data_home: &Path, class: OccurrenceClass) -> BTreeSet<String> {
    Connection::open_with_flags(
        data_home.join("search").join("search.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
    .prepare(
        "SELECT o.occurrence_id FROM occurrences o
         LEFT JOIN occurrence_tombstones t ON t.occurrence_id=o.occurrence_id
         WHERE o.class=?1 AND t.occurrence_id IS NULL",
    )
    .unwrap()
    .query_map([class.code()], |row| row.get(0))
    .unwrap()
    .collect::<rusqlite::Result<_>>()
    .unwrap()
}

/// Writes a vector row for `occurrence_id` directly: the generation and `dimension` decide whether the report may count it.
fn vector(projection: &SearchProjection, occurrence_id: &str, generation_id: &str, dimension: u32) {
    projection
        .write(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO occurrence_vectors(occurrence_id,generation_id,vector,vector_dimension,input_bytes,input_tokens,completed_at)
                 VALUES (?1,?2,?3,?4,1,1,1)",
                params![occurrence_id, generation_id, vec![0u8; dimension as usize * 4], dimension],
            )?;
            Ok(())
        })
        .unwrap();
}

fn set_job_state(projection: &SearchProjection, occurrence_id: &str, state: &str) {
    projection
        .write(|conn| {
            conn.execute(
                "UPDATE embedding_jobs SET state=?2 WHERE occurrence_id=?1",
                params![occurrence_id, state],
            )?;
            Ok(())
        })
        .unwrap();
}

/// The independent set oracle for one dense class: R is the live set, V the ids the test gave a valid vector, P the ids the test left pending or admitted, and the tombstone count is the test's own.
fn dense_oracle(
    class: OccurrenceClass,
    live: &BTreeSet<String>,
    valid: &BTreeSet<String>,
    pending: &BTreeSet<String>,
    tombstoned: usize,
) -> ClassCoverage {
    let valid: BTreeSet<&String> = live.intersection(valid).collect();
    let missing: BTreeSet<&String> = live.iter().filter(|id| !valid.contains(id)).collect();
    let pending_missing = missing.iter().filter(|id| pending.contains(**id)).count();
    ClassCoverage {
        class,
        dense: DenseDisposition::Required,
        lexical: live.len(),
        dense_required: live.len(),
        valid_vectors: valid.len(),
        missing: missing.len(),
        pending: pending_missing,
        missing_without_pending: missing.len() - pending_missing,
        tombstoned,
    }
}

fn lexical_only_oracle(live: usize, tombstoned: usize) -> ClassCoverage {
    ClassCoverage {
        class: OccurrenceClass::RawToolSpans,
        dense: DenseDisposition::LexicalOnly,
        lexical: live,
        dense_required: 0,
        valid_vectors: 0,
        missing: 0,
        pending: 0,
        missing_without_pending: 0,
        tombstoned,
    }
}

/// AC1, AC2, AC3, AC5, AC6: every class is reported separately against an independent set oracle after ingest, revision, and retirement; a valid vector counts once, a stale one never, a vector of another generation never, a vector awaiting bookkeeping is not missing, an admitted job is pending, an obsolete or failed job is not, a tombstoned row's vector and job count nowhere, raw tools are lexical-only, and kernel exclusions ride beside the counts split into covered and missing; a reopened projection reports the same.
#[test]
fn per_class_sets_match_independent_oracles() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let (m_valid, _) = corpus.publish(&message("m-valid", 1, "valid vector"));
    let (m_stale, _) = corpus.publish(&message("m-stale", 1, "stale vector"));
    let (m_pending, _) = corpus.publish(&message("m-pending", 1, "pending only"));
    let (m_bookkeeping, _) =
        corpus.publish(&message("m-bookkeeping", 1, "vector, job still pending"));
    let (m_failed, _) = corpus.publish(&message("m-failed", 1, "failed job"));
    let (m_admitted, _) = corpus.publish(&message("m-admitted", 1, "admitted job"));
    let (m_obsolete, _) = corpus.publish(&message("m-obsolete", 1, "obsolete job on a live row"));
    let (m_other_generation, _) =
        corpus.publish(&message("m-other", 1, "vector of another generation"));
    let (m_revised, _) = corpus.publish(&message("m-revised", 1, "before"));
    let (t_one, t_one_object) = corpus.publish(&tool("call_1", "tool output"));
    let (_, t_two_object) = corpus.publish(&tool("call_2", "tool error"));
    let (g_one, _) = corpus.publish(&commit("0123456789abcdef0123456789abcdef01234567", "Fix\n"));
    corpus.decide("rule", "Keep the contract.", "Relied on.");
    let hold = corpus.capture();
    let projection = corpus.bootstrap(dir.path(), &hold);
    let other_generation = VectorGeneration {
        generation_id: "gen-2".to_string(),
        generation_epoch: 2,
        ..generation()
    };
    projection
        .write(|conn| {
            register_generation(conn, &other_generation, 1)?;
            Ok(())
        })
        .unwrap();

    // Revise a message and retire a tool span; the catch-up tombstones the old rows and queues work for the new one.
    let (m_revised_new, _) = corpus.publish(&message("m-revised", 2, "after"));
    corpus.retire(&t_two_object);
    corpus.catch_up(&projection, &hold);

    // Vectors: one valid, one under another dimension, one under another generation, one valid while its job is still pending; jobs: one failed, one admitted, one obsolete on a live row; the tombstoned old revision gets a valid-shaped vector and a pending job that must count nowhere.
    vector(&projection, &m_valid, GENERATION, DIMS);
    set_job_state(&projection, &m_valid, "published");
    vector(&projection, &m_stale, GENERATION, DIMS + 1);
    set_job_state(&projection, &m_stale, "published");
    vector(&projection, &m_other_generation, "gen-2", DIMS);
    set_job_state(&projection, &m_other_generation, "published");
    vector(&projection, &m_bookkeeping, GENERATION, DIMS);
    set_job_state(&projection, &m_failed, "failed");
    set_job_state(&projection, &m_admitted, "admitted");
    set_job_state(&projection, &m_obsolete, "obsolete");
    vector(&projection, &m_revised, GENERATION, DIMS);
    set_job_state(&projection, &m_revised, "pending");
    let messages_live = live(dir.path(), OccurrenceClass::Messages);
    assert_eq!(
        messages_live,
        [
            &m_valid,
            &m_stale,
            &m_pending,
            &m_bookkeeping,
            &m_failed,
            &m_admitted,
            &m_obsolete,
            &m_other_generation,
            &m_revised_new
        ]
        .into_iter()
        .cloned()
        .collect()
    );
    assert!(!messages_live.contains(&m_revised));

    let coverage = corpus.observe(&projection).unwrap();
    let valid: BTreeSet<String> = [m_valid.clone(), m_bookkeeping.clone()]
        .into_iter()
        .collect();
    let pending: BTreeSet<String> = [m_pending.clone(), m_revised_new.clone(), m_admitted.clone()]
        .into_iter()
        .collect();
    let expected_messages = dense_oracle(
        OccurrenceClass::Messages,
        &messages_live,
        &valid,
        &pending,
        1,
    );
    assert_eq!(
        (
            expected_messages.valid_vectors,
            expected_messages.missing,
            expected_messages.pending,
            expected_messages.missing_without_pending
        ),
        (2, 7, 3, 4),
        "stale, other-generation, failed, and obsolete are missing without pending; pending-only, admitted, and the revision are pending"
    );
    assert_eq!(
        coverage.class(OccurrenceClass::Messages),
        &expected_messages
    );

    let claim_live = live(dir.path(), OccurrenceClass::CanonicalClaims);
    assert_eq!(claim_live.len(), 2, "summary and rationale");
    assert_eq!(
        coverage.class(OccurrenceClass::CanonicalClaims),
        &dense_oracle(
            OccurrenceClass::CanonicalClaims,
            &claim_live,
            &BTreeSet::new(),
            &claim_live,
            0
        )
    );
    let promoted_live = live(dir.path(), OccurrenceClass::PromotedMemory);
    assert_eq!(promoted_live.len(), 1);
    assert_eq!(
        coverage.class(OccurrenceClass::PromotedMemory),
        &dense_oracle(
            OccurrenceClass::PromotedMemory,
            &promoted_live,
            &BTreeSet::new(),
            &promoted_live,
            0
        )
    );
    let git_live = live(dir.path(), OccurrenceClass::GitCommits);
    assert_eq!(git_live, [g_one].into_iter().collect());
    assert_eq!(
        coverage.class(OccurrenceClass::GitCommits),
        &dense_oracle(
            OccurrenceClass::GitCommits,
            &git_live,
            &BTreeSet::new(),
            &git_live,
            0
        )
    );
    assert_eq!(
        coverage.class(OccurrenceClass::RawToolSpans),
        &lexical_only_oracle(1, 1)
    );
    assert_eq!(
        live(dir.path(), OccurrenceClass::RawToolSpans),
        [t_one].into_iter().collect()
    );
    assert!(
        coverage
            .report
            .classes
            .iter()
            .all(|class| !class.is_known_empty())
    );
    // Totals across classes would hide a missing class; the report has no such cell and every class is present exactly once.
    assert_eq!(
        coverage
            .report
            .classes
            .iter()
            .map(|c| c.class)
            .collect::<Vec<_>>(),
        OccurrenceClass::ALL.to_vec()
    );
    assert_eq!(
        coverage.report.checkpoint.checkpoint_commit_seq,
        corpus.kernel.tip().unwrap()
    );
    assert!(coverage.exclusions.is_empty(), "{:?}", coverage.exclusions);

    // A retirement in the kernel the projection has not caught up to is an exclusion beside the class's counts, not a change to them; the excluded message is one of the covered ones, and the excluded tool span belongs to a class that holds no vector, so it sits in neither the covered nor the missing cell.
    let (_, m_valid_object) = corpus.publish(&message("m-valid", 1, "valid vector"));
    corpus.retire(&m_valid_object);
    corpus.retire(&t_one_object);
    let excluded = corpus.observe(&projection).unwrap();
    assert_eq!(
        excluded.class(OccurrenceClass::Messages),
        &expected_messages
    );
    assert_eq!(
        excluded.class(OccurrenceClass::RawToolSpans),
        &lexical_only_oracle(1, 1)
    );
    for exclusion in &excluded.exclusions {
        let class = excluded.class(exclusion.class);
        assert!(
            exclusion.covered <= class.valid_vectors
                && exclusion.missing <= class.missing
                && exclusion.lexical_only <= class.lexical,
            "an exclusion splits members of the class's own cells: {exclusion:?} against {class:?}"
        );
        assert!(
            match class.dense {
                DenseDisposition::Required => exclusion.lexical_only == 0,
                DenseDisposition::LexicalOnly => exclusion.covered == 0 && exclusion.missing == 0,
            },
            "a class's exclusions use only its own cells: {exclusion:?} against {class:?}"
        );
    }
    assert_eq!(excluded.excluded(OccurrenceClass::Messages), 1);
    assert_eq!(excluded.excluded(OccurrenceClass::RawToolSpans), 1);
    assert_eq!(
        excluded
            .exclusions
            .iter()
            .map(|e| (e.class, e.verdict, e.covered, e.missing, e.lexical_only))
            .collect::<Vec<_>>(),
        vec![
            (
                OccurrenceClass::Messages,
                EligibilityVerdict::Retracted,
                1,
                0,
                0
            ),
            (
                OccurrenceClass::RawToolSpans,
                EligibilityVerdict::Retracted,
                0,
                0,
                1
            )
        ]
    );
    assert!(excluded.kernel_snapshot.tip > coverage.kernel_snapshot.tip);

    // Reopen: the same identity reports the same observation.
    drop(projection);
    let reopened = SearchProjection::open(dir.path()).unwrap();
    let again = corpus.observe(&reopened).unwrap();
    assert_eq!(again.report, excluded.report);
    assert_eq!(again.exclusions, excluded.exclusions);
}

/// An absent identity, a missing checkpoint, a foreign kernel, a generation whose identity disagrees, a retired generation, and a class beyond either the live or the tombstone bound each make the whole report unavailable; a projection with no rows reports every class known-empty rather than unavailable.
#[test]
fn incoherent_observations_are_unavailable_and_empty_classes_are_known_empty() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let empty_dir = dir.path().join("empty");
    let empty = SearchProjection::open(&empty_dir).unwrap();
    assert_eq!(
        corpus.observe(&empty).unwrap_err(),
        ReportUnavailable::Projection(CoverageUnavailable::NoIdentity)
    );
    empty
        .write(|conn| {
            install_identity(conn, &identity(&corpus.kernel_incarnation_id()), 1)?;
            register_generation(conn, &generation(), 1)?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        corpus.observe(&empty).unwrap_err(),
        ReportUnavailable::Projection(CoverageUnavailable::NoCheckpoint)
    );
    let hold = corpus.capture();
    drop(empty);
    let empty = corpus.bootstrap(&empty_dir, &hold);
    let known_empty = corpus.observe(&empty).unwrap();
    assert!(
        known_empty
            .report
            .classes
            .iter()
            .all(|class| class.is_known_empty())
    );
    assert!(known_empty.exclusions.is_empty());

    let foreign = observe_coverage(
        &empty,
        &corpus.kernel,
        "another-kernel",
        &ProjectScope::new(PROJECT).unwrap(),
        ArtifactDestination::Local,
        &generation(),
        bounds(),
    )
    .unwrap()
    .unwrap_err();
    assert!(matches!(
        foreign,
        ReportUnavailable::Projection(CoverageUnavailable::ForeignKernel { .. })
    ));
    let other_model = VectorGeneration {
        tokenizer_fingerprint: "b".repeat(64),
        ..generation()
    };
    let mismatch = observe_coverage(
        &empty,
        &corpus.kernel,
        &corpus.kernel_incarnation_id(),
        &ProjectScope::new(PROJECT).unwrap(),
        ArtifactDestination::Local,
        &other_model,
        bounds(),
    )
    .unwrap()
    .unwrap_err();
    assert_eq!(
        mismatch,
        ReportUnavailable::Projection(CoverageUnavailable::GenerationMismatch)
    );

    corpus.publish(&message("m-1", 1, "one"));
    corpus.publish(&message("m-2", 1, "two"));
    let hold = corpus.capture();
    let projection = corpus.bootstrap(dir.path(), &hold);
    // Revising both messages leaves the class with two live rows and two tombstoned rows, so each bound can be exceeded on its own.
    corpus.publish(&message("m-1", 2, "one, revised"));
    corpus.publish(&message("m-2", 2, "two, revised"));
    corpus.catch_up(&projection, &hold);
    let observe_with = |bounds: CoverageBounds| {
        observe_coverage(
            &projection,
            &corpus.kernel,
            &corpus.kernel_incarnation_id(),
            &ProjectScope::new(PROJECT).unwrap(),
            ArtifactDestination::Local,
            &generation(),
            bounds,
        )
        .unwrap()
    };
    assert_eq!(
        observe_with(CoverageBounds {
            max_live_per_class: NonZeroUsize::new(1).unwrap(),
            max_tombstoned_per_class: NonZeroUsize::new(64).unwrap(),
        })
        .unwrap_err(),
        ReportUnavailable::Projection(CoverageUnavailable::OverBound {
            class: OccurrenceClass::Messages,
            max: 1
        })
    );
    assert_eq!(
        observe_with(CoverageBounds {
            max_live_per_class: NonZeroUsize::new(64).unwrap(),
            max_tombstoned_per_class: NonZeroUsize::new(1).unwrap(),
        })
        .unwrap_err(),
        ReportUnavailable::Projection(CoverageUnavailable::TombstonedOverBound {
            class: OccurrenceClass::Messages,
            max: 1
        })
    );
    // Both bounds exceeded: the walk stops after three of the four rows, and whichever bound the walked prefix already exceeds names the refusal.
    assert!(matches!(
        observe_with(CoverageBounds {
            max_live_per_class: NonZeroUsize::new(1).unwrap(),
            max_tombstoned_per_class: NonZeroUsize::new(1).unwrap(),
        })
        .unwrap_err(),
        ReportUnavailable::Projection(
            CoverageUnavailable::OverBound {
                class: OccurrenceClass::Messages,
                max: 1
            } | CoverageUnavailable::TombstonedOverBound {
                class: OccurrenceClass::Messages,
                max: 1
            }
        )
    ));
    // Bounds met exactly: the walk covers every row, so the counts are exact.
    let fine = observe_with(CoverageBounds {
        max_live_per_class: NonZeroUsize::new(2).unwrap(),
        max_tombstoned_per_class: NonZeroUsize::new(2).unwrap(),
    })
    .unwrap();
    assert_eq!(fine.class(OccurrenceClass::Messages).lexical, 2);
    assert_eq!(fine.class(OccurrenceClass::Messages).tombstoned, 2);
    assert_eq!(fine.class(OccurrenceClass::Messages).pending, 2);
    assert!(fine.class(OccurrenceClass::GitCommits).is_known_empty());
    assert_eq!(corpus.observe(&projection).unwrap().report, fine.report);

    // A retired generation accepts no completion and queues no work, so its open jobs stand for nothing; the observation is unavailable rather than one that reports them as pending.
    projection
        .write(|conn| {
            conn.execute(
                "UPDATE vector_generations SET state='retired' WHERE generation_id=?1",
                [GENERATION],
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        corpus.observe(&projection).unwrap_err(),
        ReportUnavailable::Projection(CoverageUnavailable::RetiredGeneration)
    );
}
