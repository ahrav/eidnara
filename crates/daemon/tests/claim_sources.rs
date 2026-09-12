//! Tests verify independent claim and promoted-memory inventories, distinct dual-class objects, non-resurrecting correction and retirement tombstones, replay after lost or skipped acknowledgements, and memory-domain-name-independent identities.

use std::collections::{BTreeMap, BTreeSet};
use std::num::{NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use daemon::claim_sources::{
    CLAIM_CONSUMER, ClaimBlocked, ClaimExclusion, ClaimMaterializer, ClaimSubject, EpisodeFault,
    MaterializationEnd, MaterializationReport, claim_units,
};
use daemon::harness_sources::{CANONICAL_ROLE, PublishError, Representation, SourcePublisher};
use daemon::search_projection::SearchProjection;
use kernel::source_identity::{Occurrence, OccurrenceClass, encode_preserving_span};
use kernel::{
    AdmissionEvent, AdmissionRequest, CommitIntent, CommitPageBounds, DecisionPayload, DecisionRow,
    DecisionSpec, Dimension, DomainSpec, EventKind, ExportWindow, KernelError, KernelStore,
    ObservationPayload, ObservationSpec, ProviderEgress, RemediationTarget, ScopeSpec,
    ScopeTermSpec, Sensitivity, SourceClass, SourceHold, SourceHoldAdmission, SourceHoldBinding,
    SourceHoldBounds, SourcePageBounds, SourceRow, TaintClass,
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
const MEMORY: &str = "memory";
const OTHER: &str = "notes";
/// A domain whose name is the memory domain's id and whose id is not.
const NAMESAKE: &str = "memory-archive";
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const MODEL: &str = "tiny-test-model";
const FINGERPRINT: &str = "a2b4c6d8e0f01234a2b4c6d8e0f01234a2b4c6d8e0f01234a2b4c6d8e0f01234";
const GENERATION: &str = "gen-1";
const NOW: i64 = 1_000;
const CLASSES: (SourceClass, TaintClass) = (SourceClass::ExplicitUser, TaintClass::UserExplicit);
const CONTRACT: &str = "Keep the public contract.";
/// The frozen positive-category taxonomy, copied here so a production edit moves the ledger, not the oracle.
const POSITIVE: [&str; 12] = [
    "PROJECT_RULES",
    "ARCHITECTURE",
    "CONSTRAINTS",
    "CONFIG_VALUES",
    "NAMING",
    "USER_DIRECTIVES",
    "USER_PREFERENCES",
    "CONFIG_DEFAULTS",
    "ARCHITECTURE_DECISIONS",
    "ENVIRONMENT",
    "WORKFLOW_RULES",
    "KNOWN_ISSUES",
];

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "daemon-claim-sources-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

fn bounds() -> CommitPageBounds {
    CommitPageBounds {
        max_commits: NonZeroUsize::new(8).unwrap(),
        max_rows: NonZeroUsize::new(64).unwrap(),
        max_payload_bytes: NonZeroU64::new(1 << 20).unwrap(),
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

fn page_bounds() -> SourcePageBounds {
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

fn projection_identity(kernel_incarnation_id: &str) -> ProjectionIdentity {
    ProjectionIdentity {
        schema_version: retrieval::SCHEMA_VERSION,
        kernel_incarnation_id: kernel_incarnation_id.to_string(),
        projection_policy_version: POLICY.to_string(),
        identity_contract_version: "search-projection-identity-v3".to_string(),
        limit_manifest_protocol_version: "limits.v1".to_string(),
        embedding_model: MODEL.to_string(),
        tokenizer_fingerprint: FINGERPRINT.to_string(),
        vector_dimension: 8,
        generation_epoch: 1,
    }
}

/// One occurrence as the independent ledger predicts it: class, identity field and object, revision, representation, exact text, and the domain and scope the row must be published under.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Expected {
    class: &'static str,
    field: &'static str,
    object_id: String,
    revision: i64,
    representation: &'static str,
    text: String,
    domain_id: String,
    scope_id: Option<String>,
}

impl Expected {
    fn occurrence_id(&self) -> String {
        encode_preserving_span(&Occurrence {
            class: self.class,
            identity: &[(self.field, &self.object_id)],
            revision: &self.revision.to_string(),
            representation: self.representation,
            span: None,
        })
        .unwrap()
        .occurrence_id
    }
}

/// One decision the ledger predicts from.
#[derive(Clone, Copy)]
struct Seed<'a> {
    object: &'a str,
    domain: &'a str,
    kind: &'a str,
    revision: i64,
    summary: &'a str,
    rationale: &'a str,
    scoped: bool,
}

impl Seed<'_> {
    const fn scoped(
        object: &'static str,
        domain: &'static str,
        kind: &'static str,
        revision: i64,
        summary: &'static str,
        rationale: &'static str,
    ) -> Seed<'static> {
        Seed {
            object,
            domain,
            kind,
            revision,
            summary,
            rationale,
            scoped: true,
        }
    }

    fn spec(&self) -> DecisionSpec {
        DecisionSpec {
            decision_id: format!("{}-decision-{}", self.object, self.revision),
            object_id: self.object.to_string(),
            domain_id: self.domain.to_string(),
            proposition_id: None,
            scope_id: self.scoped.then(|| SCOPE.to_string()),
            anchor_id: None,
            evidence_id: None,
            decision_kind: self.kind.to_string(),
            payload: DecisionPayload {
                summary: self.summary.to_string(),
                rationale: self.rationale.to_string(),
            },
            source_kind: "assistant".to_string(),
            source_id: format!("{}-lineage", self.object),
            source_revision: self.revision,
            sensitivity: Sensitivity::Normal,
        }
    }

    /// The ledger's mapping, written from the ticket's contract independently of the producer.
    fn ledger(&self) -> (Vec<Expected>, Vec<ClaimExclusion>) {
        let mut rows = Vec::new();
        let mut exclusions = Vec::new();
        if !self.scoped {
            return (rows, vec![ClaimExclusion::Unscoped]);
        }
        let promoted = if self.domain != MEMORY {
            exclusions.push(ClaimExclusion::OutsideMemoryDomain);
            false
        } else if !POSITIVE.contains(&self.kind) {
            exclusions.push(ClaimExclusion::NegativeCategory);
            false
        } else {
            true
        };
        let mut row = |class: &'static str,
                       field: &'static str,
                       representation: &'static str,
                       text: &str,
                       excluded: Representation| {
            if text.is_empty() {
                exclusions.push(ClaimExclusion::EmptyRepresentation(excluded));
            } else {
                rows.push(Expected {
                    class,
                    field,
                    object_id: self.object.to_string(),
                    revision: self.revision,
                    representation,
                    text: text.to_string(),
                    domain_id: self.domain.to_string(),
                    scope_id: Some(SCOPE.to_string()),
                });
            }
        };
        row(
            "canonical_claims",
            "object_id",
            "decision_summary",
            self.summary,
            Representation::DecisionSummary,
        );
        row(
            "canonical_claims",
            "object_id",
            "rationale",
            self.rationale,
            Representation::Rationale,
        );
        if promoted {
            row(
                "promoted_memory",
                "decision_object_id",
                "summary",
                self.summary,
                Representation::Summary,
            );
        }
        (rows, exclusions)
    }
}

fn admission(object_id: &str) -> AdmissionRequest {
    AdmissionRequest {
        candidate_id: None,
        subject_object_id: Some(object_id.to_string()),
        source_class: Some(CLASSES.0),
        taint_class: Some(CLASSES.1),
        event: AdmissionEvent {
            kind: EventKind::Other,
            trigger_object_id: None,
            approval_object_id: None,
            evidence_id: None,
            reason: "fixture".to_string(),
        },
    }
}

/// A kernel and the projections built beside it.
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

    /// Three domains (the memory domain, another, and one merely named after it), one project scope, the search consumer, and the claim consumer.
    fn seed(&self) {
        self.kernel
            .commit(intent("seed"), |envelope| {
                for (domain, name) in [
                    (MEMORY, format!("The {MEMORY} domain")),
                    (OTHER, format!("The {OTHER} domain")),
                    (NAMESAKE, MEMORY.to_string()),
                ] {
                    envelope.insert_domain(DomainSpec {
                        domain_id: domain.to_string(),
                        object_id: format!("{domain}-object"),
                        name,
                        source_kind: "fixture".to_string(),
                        source_id: domain.to_string(),
                        source_revision: 1,
                        sensitivity: Sensitivity::Normal,
                    })?;
                }
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

    /// Inserts one admitted decision.
    fn decide(&self, seed: Seed<'_>) {
        let spec = seed.spec();
        self.kernel
            .commit(
                intent(&format!("decide:{}:{}", seed.object, seed.revision)),
                |envelope| {
                    envelope.insert_decision(spec.clone())?;
                    envelope.record_admission(admission(seed.object))?;
                    Ok(String::new())
                },
            )
            .unwrap();
    }

    fn correct(&self, replaced: &str, seed: Seed<'_>) {
        let spec = DecisionSpec {
            source_id: format!("{replaced}-lineage"),
            ..seed.spec()
        };
        self.kernel
            .commit(
                intent(&format!("correct:{replaced}:{}", seed.object)),
                |envelope| {
                    let outcome = envelope.correct_decision(replaced, spec.clone())?;
                    envelope.record_admission(admission(&outcome.object_id))?;
                    Ok(String::new())
                },
            )
            .unwrap();
    }

    fn retire(&self, object_id: &str) {
        self.kernel
            .commit(intent(&format!("retire:{object_id}")), |envelope| {
                envelope.retire_decision(object_id)?;
                Ok(String::new())
            })
            .unwrap();
    }

    fn materializer(&self) -> ClaimMaterializer<'_> {
        ClaimMaterializer::new(&self.kernel, ProviderEgress::LocalOnly)
    }

    fn materialize(&self) -> MaterializationReport {
        let report = self.materializer().run_episode(bounds(), NOW).unwrap();
        assert!(
            matches!(report.end, MaterializationEnd::ReachedTarget),
            "{report:?}"
        );
        report
    }

    fn checkpoint(&self) -> Option<i64> {
        self.kernel
            .outbox_consumer_checkpoint(CLAIM_CONSUMER)
            .unwrap()
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

    /// Every page of `window` under `hold`.
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
                    page_bounds(),
                )
                .unwrap();
            rows.extend(page.rows);
            match page.next {
                Some(next) => cursor = Some(next),
                None => return rows,
            }
        }
    }

    /// Every descriptor live at a fresh S, with its text.
    fn export(&self) -> Vec<SourceRow> {
        let hold = self.capture();
        let rows = self.export_window(&hold, ExportWindow::Snapshot);
        self.kernel
            .release_source_hold(&self.binding(), &hold.hold_id, hold.captured_at)
            .unwrap();
        rows
    }

    /// The live inventory the kernel exports, as ledger rows; the scope is read from the descriptor object's registry state.
    fn inventory(&self) -> BTreeSet<Expected> {
        let rows = self.export();
        let ids: Vec<String> = rows.iter().map(|row| row.object_id.clone()).collect();
        let states = self.kernel.object_states(&ids).unwrap().1;
        rows.into_iter()
            .zip(states)
            .filter(|(row, _)| row.invalidated_commit_seq.is_none())
            .map(|(row, state)| {
                let (field, object_id) = row.detail.identity[0].clone();
                Expected {
                    class: OccurrenceClass::from_code(&row.detail.class)
                        .unwrap()
                        .code(),
                    field: match field.as_str() {
                        "object_id" => "object_id",
                        "decision_object_id" => "decision_object_id",
                        other => panic!("unexpected identity field {other}"),
                    },
                    object_id,
                    revision: row.revision,
                    representation: match row.detail.representation.as_str() {
                        "decision_summary" => "decision_summary",
                        "rationale" => "rationale",
                        "summary" => "summary",
                        other => panic!("unexpected representation {other}"),
                    },
                    text: row.text.unwrap(),
                    domain_id: row.domain_id,
                    scope_id: state.unwrap().scope_id,
                }
            })
            .collect()
    }

    /// The admission classes recorded for every live descriptor, keyed by descriptor object.
    fn descriptor_admissions(&self) -> BTreeMap<String, (String, String)> {
        self.kernel_db()
            .prepare(
                "SELECT a.subject_object_id,a.source_class,a.taint_class
                 FROM admission_decisions a
                 JOIN observations o ON o.object_id=a.subject_object_id
                 WHERE o.observation_kind=?1",
            )
            .unwrap()
            .query_map([kernel::SOURCE_DESCRIPTOR_KIND], |row| {
                Ok((row.get(0)?, (row.get(1)?, row.get(2)?)))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    /// Opens a projection bound to `hold` and applies the snapshot batch, queueing one pending job per dense-eligible occurrence.
    fn bootstrap(&self, data_home: &Path, hold: &SourceHold) -> (SearchProjection, Vec<SourceRow>) {
        let rows = self.export_window(hold, ExportWindow::Snapshot);
        let projection = SearchProjection::open(data_home).unwrap();
        let kernel_incarnation_id = self.kernel_incarnation_id();
        projection
            .write(|conn| {
                install_identity(conn, &projection_identity(&kernel_incarnation_id), 1)?;
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
        (projection, rows)
    }

    /// Applies `rows` as the batch through `through` under `hold`.
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

/// Projection rows through an independent connection: (class, occurrence_id, payload_id, tombstoned, pending jobs).
fn projection_rows(data_home: &Path) -> BTreeSet<(String, String, String, bool, i64)> {
    Connection::open_with_flags(
        data_home.join("search").join("search.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
    .prepare(
        "SELECT o.class,o.occurrence_id,o.payload_id,t.occurrence_id IS NOT NULL,
                (SELECT COUNT(*) FROM embedding_jobs j
                 WHERE j.occurrence_id=o.occurrence_id AND j.state='pending')
         FROM occurrences o
         LEFT JOIN occurrence_tombstones t ON t.occurrence_id=o.occurrence_id",
    )
    .unwrap()
    .query_map([], |row| {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
        ))
    })
    .unwrap()
    .collect::<rusqlite::Result<_>>()
    .unwrap()
}

fn live_rows(data_home: &Path) -> BTreeSet<(String, String, String, bool, i64)> {
    projection_rows(data_home)
        .into_iter()
        .filter(|row| !row.3)
        .collect()
}

fn occurrence_ids(units: &[daemon::harness_sources::SourceUnit]) -> BTreeSet<String> {
    units
        .iter()
        .map(|unit| {
            let identity: Vec<(&str, &str)> = unit
                .identity
                .iter()
                .map(|(name, value)| (*name, value.as_str()))
                .collect();
            encode_preserving_span(&Occurrence {
                class: unit.class.code(),
                identity: &identity,
                revision: &unit.revision,
                representation: unit.representation.as_str(),
                span: None,
            })
            .unwrap()
            .occurrence_id
        })
        .collect()
}

/// AC1, AC2, AC6: the independent ledger predicts every occurrence, exclusion, domain, and scope; a dual-class object keeps two occurrences over one payload; a negative category, another domain, a domain merely named "memory", an observation, an unscoped decision, an empty summary, and a legacy-kind decision create no promoted row; every descriptor carries its decision's admission classes; dense work is queued for every live occurrence; a second episode is a no-op.
#[test]
fn ledger_predicts_inventory_exclusions_and_dense_work() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let seeds = [
        Seed::scoped(
            "rule",
            MEMORY,
            "PROJECT_RULES",
            3,
            CONTRACT,
            "It is relied on.",
        ),
        Seed::scoped(
            "anti",
            MEMORY,
            "REJECTED_APPROACH",
            1,
            "Do not shelve the design.",
            "It failed once.",
        ),
        Seed::scoped(
            "note",
            OTHER,
            "PROJECT_RULES",
            2,
            CONTRACT,
            "Same words, other domain.",
        ),
        Seed::scoped(
            "namesake",
            NAMESAKE,
            "PROJECT_RULES",
            1,
            "Named like memory.",
            "The id decides.",
        ),
        Seed::scoped("bare", MEMORY, "NAMING", 5, "Name things by domain.", ""),
        Seed::scoped(
            "blank",
            MEMORY,
            "CONSTRAINTS",
            1,
            "",
            "A rationale without a summary.",
        ),
        Seed::scoped(
            "legacy",
            MEMORY,
            "memory",
            1,
            "A legacy-kind decision.",
            "Not a positive category.",
        ),
        Seed {
            scoped: false,
            ..Seed::scoped(
                "floating",
                MEMORY,
                "PROJECT_RULES",
                1,
                "No project.",
                "Serves nobody.",
            )
        },
    ];
    let mut expected_rows = BTreeSet::new();
    let mut expected_exclusions = Vec::new();
    for seed in seeds {
        corpus.decide(seed);
        let (rows, exclusions) = seed.ledger();
        expected_rows.extend(rows);
        expected_exclusions.extend(
            exclusions
                .into_iter()
                .map(|exclusion| (seed.object.to_string(), exclusion)),
        );
    }
    assert!(expected_exclusions.contains(&("floating".to_string(), ClaimExclusion::Unscoped)));
    assert!(expected_exclusions.contains(&(
        "blank".to_string(),
        ClaimExclusion::EmptyRepresentation(Representation::Summary)
    )));
    corpus
        .kernel
        .commit(intent("observe"), |envelope| {
            envelope.insert_observation(ObservationSpec {
                observation_id: "obs".to_string(),
                object_id: "obs-object".to_string(),
                domain_id: MEMORY.to_string(),
                proposition_id: None,
                scope_id: Some(SCOPE.to_string()),
                anchor_id: None,
                evidence_id: None,
                observation_kind: "note".to_string(),
                payload: ObservationPayload {
                    summary: CONTRACT.to_string(),
                    classification: "note".to_string(),
                    detail: None,
                },
                observed_at: 1,
                dependencies: Vec::new(),
                source_kind: "assistant".to_string(),
                source_id: "obs-lineage".to_string(),
                source_revision: 1,
                sensitivity: Sensitivity::Normal,
            })?;
            Ok(String::new())
        })
        .unwrap();

    let report = corpus.materialize();
    assert_eq!(report.published, expected_rows.len(), "{report:?}");
    assert_eq!(report.replayed, 0);
    assert_eq!(report.exclusions, expected_exclusions);
    assert_eq!(report.acknowledged_through, report.target);
    let inventory = corpus.inventory();
    assert_eq!(inventory, expected_rows);
    assert_eq!(
        inventory
            .iter()
            .map(|row| row.class)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["canonical_claims", "promoted_memory"]),
        "the ledger names both classes, so an empty or one-class inventory fails"
    );
    assert!(
        inventory
            .iter()
            .filter(|row| row.class == "promoted_memory")
            .all(|row| row.domain_id == MEMORY)
    );
    let admissions = corpus.descriptor_admissions();
    assert_eq!(admissions.len(), expected_rows.len());
    assert!(
        admissions.values().all(|classes| *classes
            == (
                CLASSES.0.as_str().to_string(),
                CLASSES.1.as_str().to_string()
            )),
        "every descriptor carries its decision's own admission classes: {admissions:?}"
    );

    let hold = corpus.capture();
    let _projection = corpus.bootstrap(dir.path(), &hold);
    let rows = live_rows(dir.path());
    let by_id: BTreeMap<&str, &(String, String, String, bool, i64)> =
        rows.iter().map(|row| (row.1.as_str(), row)).collect();
    assert_eq!(rows.len(), expected_rows.len());
    for expected in &expected_rows {
        let row = by_id
            .get(expected.occurrence_id().as_str())
            .unwrap_or_else(|| panic!("missing {expected:?}"));
        assert_eq!(row.0, expected.class);
        assert_eq!(row.4, 1, "exactly one pending job for {expected:?}");
    }
    let contract_rows: Vec<&Expected> = expected_rows
        .iter()
        .filter(|row| row.text == CONTRACT)
        .collect();
    assert_eq!(
        contract_rows.len(),
        3,
        "claim summary, promoted summary, and the other domain's claim are three occurrences"
    );
    let payloads: BTreeSet<&str> = contract_rows
        .iter()
        .map(|row| by_id[row.occurrence_id().as_str()].2.as_str())
        .collect();
    assert_eq!(payloads.len(), 1, "equal bytes share one payload row");

    let again = corpus.materialize();
    assert_eq!((again.published, again.retired), (0, 0), "{again:?}");
    assert_eq!(corpus.inventory(), expected_rows);
}

/// AC3, AC5: correction retires the predecessor's descriptors before publishing the successor's; retirement retires them; a live projection catching up over the window tombstones the old occurrences and queues exactly one job for the same-text successor, and re-applying the window adds nothing; a stale publication replays its receipt and revives no row.
#[test]
fn correction_retirement_and_replay_cannot_resurrect_stale_rows() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let rule = Seed::scoped("rule", MEMORY, "PROJECT_RULES", 1, CONTRACT, "Relied on.");
    let anti = Seed::scoped(
        "anti",
        MEMORY,
        "PROJECT_RULES",
        1,
        "Shelve the old design.",
        "",
    );
    corpus.decide(rule);
    corpus.decide(anti);
    let first = corpus.materialize();
    assert_eq!(first.published, 5, "{first:?}");
    let before = corpus.inventory();
    let (old_rule, _) = rule.ledger();
    let (old_anti, _) = anti.ledger();
    assert!(
        old_rule
            .iter()
            .chain(&old_anti)
            .all(|row| before.contains(row))
    );
    let hold = corpus.capture();
    let (projection, _) = corpus.bootstrap(dir.path(), &hold);
    assert_eq!(live_rows(dir.path()).len(), 5);

    let successor = Seed::scoped(
        "rule-v2",
        MEMORY,
        "PROJECT_RULES",
        2,
        CONTRACT,
        "Relied on.",
    );
    corpus.correct("rule", successor);
    corpus.retire("anti");
    let second = corpus.materialize();
    let (new_rule, _) = successor.ledger();
    assert_eq!(second.published, new_rule.len(), "{second:?}");
    assert_eq!(
        second.retired,
        old_rule.len() + old_anti.len(),
        "{second:?}"
    );
    let after = corpus.inventory();
    assert_eq!(after, new_rule.iter().cloned().collect());
    // Retirement commits before the successor's publication, so no commit between them holds both revisions.
    let db = corpus.kernel_db();
    let commit_of = |object_id: &str, column: &str| -> i64 {
        db.query_row(
            &format!("SELECT {column} FROM object_registry WHERE object_id=?1"),
            [object_id],
            |row| row.get(0),
        )
        .unwrap()
    };
    let descriptor_object = |expected: &Expected| -> String {
        let encoded = encode_preserving_span(&Occurrence {
            class: expected.class,
            identity: &[(expected.field, &expected.object_id)],
            revision: &expected.revision.to_string(),
            representation: expected.representation,
            span: None,
        })
        .unwrap();
        kernel::descriptor_object_id(&encoded.lineage_id, &expected.revision.to_string())
    };
    let retired_at = old_rule
        .iter()
        .map(|row| commit_of(&descriptor_object(row), "invalidated_commit_seq"))
        .max()
        .unwrap();
    let published_at = new_rule
        .iter()
        .map(|row| commit_of(&descriptor_object(row), "created_commit_seq"))
        .min()
        .unwrap();
    assert!(retired_at < published_at, "{retired_at} vs {published_at}");

    let replay = corpus.materialize();
    assert_eq!((replay.published, replay.retired), (0, 0), "{replay:?}");
    assert_eq!(corpus.inventory(), after);

    // Publishing the retired revision again answers from its receipt: the kernel writes nothing.
    let old = corpus
        .kernel
        .object_states(&["rule".to_string()])
        .unwrap()
        .1
        .remove(0)
        .unwrap();
    let invalidated = old.object.invalidated_commit_seq.unwrap();
    let old_decision = corpus
        .kernel
        .decisions_for_objects_as_of(&["rule".to_string()], invalidated - 1)
        .unwrap()
        .remove(0);
    let units = claim_units(
        &ClaimSubject::from_object(&old.object).unwrap(),
        &old_decision,
    )
    .unwrap();
    let publisher = SourcePublisher {
        kernel: &corpus.kernel,
        domain_id: MEMORY,
        scope_id: Some(SCOPE),
        egress: ProviderEgress::LocalOnly,
        sensitivity: Sensitivity::Normal,
    };
    let tip = corpus.kernel.tip().unwrap();
    for unit in &units.units {
        assert!(publisher.publish(unit, NOW).unwrap().replayed, "{unit:?}");
    }
    assert_eq!(corpus.kernel.tip().unwrap(), tip);
    assert_eq!(corpus.inventory(), after);

    // The live projection catches up over the window: old occurrences are tombstoned, the same-text successor is a new identity with one pending job, and the window applied twice adds nothing.
    let through = corpus.kernel.tip().unwrap();
    corpus
        .kernel
        .extend_source_hold(&corpus.binding(), &hold.hold_id, through, hold_admission())
        .unwrap();
    let delta = corpus.export_window(&hold, ExportWindow::CatchUp { through });
    corpus.apply(&projection, &hold, &delta, through);
    let rows = projection_rows(dir.path());
    let live: BTreeSet<String> = rows
        .iter()
        .filter(|row| !row.3)
        .map(|row| row.1.clone())
        .collect();
    assert_eq!(live, new_rule.iter().map(Expected::occurrence_id).collect());
    for stale in old_rule.iter().chain(&old_anti) {
        let row = rows
            .iter()
            .find(|row| row.1 == stale.occurrence_id())
            .unwrap();
        assert!(row.3, "the stale occurrence is tombstoned: {stale:?}");
        assert_eq!(
            row.4, 0,
            "no pending job survives on a tombstoned occurrence"
        );
    }
    for fresh in &new_rule {
        let row = rows
            .iter()
            .find(|row| row.1 == fresh.occurrence_id())
            .unwrap();
        assert_eq!(
            row.4, 1,
            "exactly one pending job for the successor {fresh:?}"
        );
    }
    corpus.apply(&projection, &hold, &delta, through);
    assert_eq!(
        projection_rows(dir.path()),
        rows,
        "an identical window replays without new work"
    );
}

/// AC3: a page whose acknowledgement reply is lost is reconciled from the durable checkpoint, and a page published but never acknowledged is re-driven on the next episode entirely from receipts; neither path writes a second descriptor or moves the inventory.
#[test]
fn lost_and_skipped_acknowledgements_replay_from_receipts() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    corpus.decide(Seed::scoped(
        "rule",
        MEMORY,
        "PROJECT_RULES",
        1,
        CONTRACT,
        "Relied on.",
    ));
    let lost = corpus
        .materializer()
        .run_episode_with_fault_for_test(bounds(), NOW, EpisodeFault::LoseAcknowledgementReply)
        .unwrap();
    assert!(
        matches!(lost.end, MaterializationEnd::ReachedTarget),
        "{lost:?}"
    );
    assert_eq!(lost.published, 3);
    assert_eq!(corpus.checkpoint(), Some(lost.target));
    let inventory = corpus.inventory();
    assert_eq!(inventory.len(), 3);

    corpus.decide(Seed::scoped(
        "other",
        MEMORY,
        "NAMING",
        1,
        "Name things.",
        "Because.",
    ));
    let skipped = corpus
        .materializer()
        .run_episode_with_fault_for_test(bounds(), NOW, EpisodeFault::SkipAcknowledgement)
        .unwrap();
    assert!(
        matches!(skipped.end, MaterializationEnd::ReachedTarget),
        "{skipped:?}"
    );
    assert_eq!(skipped.published, 3);
    assert_eq!(
        corpus.checkpoint(),
        Some(lost.target),
        "the checkpoint did not move"
    );
    let tip = corpus.kernel.tip().unwrap();
    let inventory = corpus.inventory();
    assert_eq!(inventory.len(), 6);

    let redriven = corpus.materialize();
    assert_eq!(redriven.published, 0, "{redriven:?}");
    assert_eq!(
        redriven.replayed, 3,
        "every descriptor of the unacknowledged page answers from its receipt"
    );
    assert_eq!(corpus.checkpoint(), Some(redriven.target));
    assert_eq!(
        corpus.kernel.tip().unwrap(),
        tip,
        "a replayed page commits nothing"
    );
    assert_eq!(corpus.inventory(), inventory);
    assert_eq!(
        corpus.descriptor_admissions().len(),
        6,
        "no descriptor is admitted twice"
    );
}

/// AC2: a decision with no admission decision authorizes nothing: its descriptors are refused before any byte is retained and the checkpoint does not pass it.
#[test]
fn unadmitted_decision_publishes_nothing_and_blocks_the_episode() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let orphan = Seed::scoped("orphan", MEMORY, "PROJECT_RULES", 1, "Unadmitted.", "").spec();
    corpus
        .kernel
        .commit(intent("orphan"), |envelope| {
            envelope.insert_decision(orphan.clone())?;
            Ok(String::new())
        })
        .unwrap();
    let checkpoint = corpus.checkpoint();
    let report = corpus.materializer().run_episode(bounds(), NOW).unwrap();
    assert!(
        matches!(
            report.end,
            MaterializationEnd::Blocked(ClaimBlocked::Publish {
                ref object_id,
                error: PublishError::UnadmittedSource { evidence: None },
            }) if object_id == "orphan"
        ),
        "{report:?}"
    );
    assert_eq!(report.published, 0);
    assert!(corpus.inventory().is_empty());
    assert_eq!(corpus.checkpoint(), checkpoint);
    let evidence: i64 = corpus
        .kernel_db()
        .query_row(
            "SELECT COUNT(*) FROM object_registry WHERE object_kind='evidence'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        evidence, 0,
        "nothing was retained for the unadmitted decision"
    );
}

/// AC5: a name-only remediation of the memory domain changes no identity, tuple, or payload, and a second episode publishes nothing; a mapping that folded the name into the identity would move.
#[test]
fn domain_name_is_not_an_input() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    corpus.decide(Seed::scoped(
        "rule",
        MEMORY,
        "PROJECT_RULES",
        1,
        CONTRACT,
        "Relied on.",
    ));
    corpus.materialize();
    let before = corpus.inventory();
    let export = |corpus: &Corpus| -> Vec<(String, Vec<u8>, String)> {
        corpus
            .export()
            .into_iter()
            .map(|row| {
                (
                    row.detail.occurrence_id,
                    row.detail.occurrence_tuple,
                    row.detail.payload_id,
                )
            })
            .collect()
    };
    let export_before = export(&corpus);
    corpus
        .kernel
        .commit(intent("remediate"), |envelope| {
            envelope.remediate_text(
                RemediationTarget::CanonicalDomainName {
                    object_id: format!("{MEMORY}-object"),
                },
                "operator",
                5,
            )?;
            Ok(String::new())
        })
        .unwrap();
    let report = corpus.materialize();
    assert_eq!((report.published, report.retired), (0, 0), "{report:?}");
    assert_eq!(corpus.inventory(), before);
    assert_eq!(export(&corpus), export_before);

    let object = corpus
        .kernel
        .object_states(&["rule".to_string()])
        .unwrap()
        .1
        .remove(0)
        .unwrap()
        .object;
    let decision = corpus
        .kernel
        .decisions_for_objects_as_of(&["rule".to_string()], corpus.kernel.tip().unwrap())
        .unwrap()
        .remove(0);
    let recomputed = claim_units(&ClaimSubject::from_object(&object).unwrap(), &decision).unwrap();
    assert!(
        recomputed
            .units
            .iter()
            .all(|unit| unit.role == CANONICAL_ROLE)
    );
    let recomputed_ids = occurrence_ids(&recomputed.units);
    assert_eq!(
        recomputed_ids,
        before.iter().map(Expected::occurrence_id).collect()
    );
    let with_name = encode_preserving_span(&Occurrence {
        class: "canonical_claims",
        identity: &[("object_id", &format!("rule:The {MEMORY} domain"))],
        revision: "1",
        representation: "decision_summary",
        span: None,
    })
    .unwrap()
    .occurrence_id;
    assert!(!recomputed_ids.contains(&with_name));
}

/// A subject and decision naming different objects, or a non-decision registry row, are refused rather than mapped.
#[test]
fn mismatched_object_and_decision_are_refused() {
    let object = kernel::ObjectRow {
        object_id: "a".to_string(),
        object_kind: "decision".to_string(),
        domain_id: MEMORY.to_string(),
        source_kind: "assistant".to_string(),
        source_id: "a-lineage".to_string(),
        source_revision: 1,
        created_commit_seq: 1,
        invalidated_commit_seq: None,
        superseded_by: None,
        sensitivity: Sensitivity::Normal,
    };
    let decision = DecisionRow {
        decision_id: "b-decision".to_string(),
        object_id: "b".to_string(),
        proposition_id: None,
        scope_id: Some(SCOPE.to_string()),
        anchor_id: None,
        evidence_id: None,
        decision_kind: "PROJECT_RULES".to_string(),
        payload: DecisionPayload {
            summary: "x".to_string(),
            rationale: String::new(),
        },
        created_commit_seq: 1,
        sensitivity: Sensitivity::Normal,
    };
    let subject = ClaimSubject::from_object(&object).unwrap();
    assert_eq!(
        claim_units(&subject, &decision).unwrap_err(),
        KernelError::InvalidInput
    );
    let observation = kernel::ObjectRow {
        object_kind: "observation".to_string(),
        object_id: "b".to_string(),
        ..object
    };
    assert_eq!(
        ClaimSubject::from_object(&observation).unwrap_err(),
        KernelError::InvalidInput
    );
}
