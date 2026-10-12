/// The daemon's allocation recorder: it records only on the thread that opened a window, so the other tests in this binary run unrecorded.
#[path = "../../daemon/tests/support/alloc_recorder.rs"]
#[allow(dead_code)]
mod alloc_recorder;

#[global_allocator]
static GLOBAL: alloc_recorder::RecordingAlloc = alloc_recorder::RecordingAlloc;

use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use kernel::applicability::EvalBudget;
use kernel::source_identity::{Occurrence, OccurrenceClass};
use kernel::{
    AdmissionEvent, AdmissionRequest, ArtifactDestination, BackupRequest, CommitIntent,
    DecisionPayload, DecisionSpec, Dimension, DomainSpec, EligibilityVerdict, EventKind,
    KernelStore, MAX_ELIGIBILITY_CANDIDATES, ProjectScope, ScopeSpec, ScopeTermSpec, Sensitivity,
    SourceClass, TaintClass,
};
use retrieval::batch::{BatchBounds, MutationIdentity, ProjectionBatch, apply_batch};
use retrieval::lexical::{
    Authority, Completion, IncompleteReason, LexicalBounds, Probe, Retrieval, RetrievalBounds,
    RetrievalRefusal, Window, admit, analyze, compile, retrieve, retrieve_with_hook_for_test, scan,
};
use retrieval::{OccurrenceRecord, Payload, PersistBounds, ProjectionIdentity, install_identity};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use storage::{
    GuardedConn, Isolation, SqliteStore, StorageBackend, StorageDescriptor, open_sqlite,
};

const DOMAIN: &str = "domain";
const PROJECT_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SCOPE_A: &str = "project:a";
const HOLD: &str = "hold-1";
const DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const OBJECTS: [&str; 6] = ["alpha", "beta", "gamma", "delta", "epsilon", "zeta"];
const COMMIT_OID: &str = "0123456789abcdef0123456789abcdef01234567";

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "lexical-retrieval-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "retrieval".to_string(),
    }
}

fn scope(scope_id: &str, digest: &str) -> ScopeSpec {
    ScopeSpec {
        scope_id: scope_id.to_string(),
        object_id: scope_id.to_string(),
        source_id: scope_id.to_string(),
        domain_id: DOMAIN.to_string(),
        source_kind: "kernel_route".to_string(),
        source_revision: 1,
        sensitivity: Sensitivity::Normal,
        terms: vec![ScopeTermSpec {
            dimension: Dimension::Project.as_str().to_string(),
            operator: "exact".to_string(),
            exact_value: Some(digest.to_string()),
            ..ScopeTermSpec::default()
        }],
    }
}

fn decision(object: &str) -> DecisionSpec {
    DecisionSpec {
        decision_id: format!("decision-{object}"),
        object_id: object.to_string(),
        domain_id: DOMAIN.to_string(),
        proposition_id: None,
        scope_id: Some(SCOPE_A.to_string()),
        anchor_id: None,
        evidence_id: None,
        decision_kind: "architecture".to_string(),
        payload: DecisionPayload {
            summary: format!("summary {object}"),
            rationale: format!("rationale {object}"),
        },
        source_kind: "repo".to_string(),
        source_id: format!("src/{object}"),
        source_revision: 1,
        sensitivity: Sensitivity::Normal,
    }
}

fn admission(object: &str) -> AdmissionRequest {
    AdmissionRequest {
        candidate_id: None,
        subject_object_id: Some(object.to_string()),
        source_class: Some(SourceClass::ExplicitUser),
        taint_class: Some(TaintClass::UserExplicit),
        event: AdmissionEvent {
            kind: EventKind::Other,
            trigger_object_id: None,
            approval_object_id: None,
            evidence_id: None,
            reason: "test".to_string(),
        },
    }
}

/// Opens the projection with the pragmas the daemon's `SearchProjection::pin_connection` pins, so the latency gate measures the production connection.
fn open_store(dir: &Path) -> SqliteStore {
    let store = open_sqlite(
        &StorageDescriptor {
            module_id: "eidnara-test".to_string(),
            storage_namespace: "search-projection".to_string(),
            isolation: Isolation::Module,
            backend: StorageBackend::Sqlite {
                path: dir
                    .join("search")
                    .join("search.sqlite")
                    .to_string_lossy()
                    .into_owned(),
            },
        },
        retrieval::BASELINE,
    )
    .unwrap();
    store
        .with_conn_unfenced(|conn| {
            conn.pragma_update(None, "cache_size", -(8 * 1024))?;
            conn.pragma_update(None, "temp_store", "MEMORY")?;
            conn.pragma_update(None, "mmap_size", 0x4_0000_0000_i64)
        })
        .unwrap();
    store
}

fn batch_bounds() -> BatchBounds {
    BatchBounds {
        persist: PersistBounds {
            max_records: NonZeroUsize::new(4096).unwrap(),
            max_payload_bytes: NonZeroUsize::new(4096).unwrap(),
            max_tuple_bytes: NonZeroUsize::new(2048).unwrap(),
        },
        max_source_bytes: NonZeroUsize::new(1 << 20).unwrap(),
        max_local_mutations: NonZeroUsize::new(8192).unwrap(),
        max_pending: NonZeroUsize::new(8192).unwrap(),
    }
}

/// D26b's qualification threshold and ranked-match budget, the values the threshold fixtures are built around.
const QUALIFYING_MATCHES: usize = 10_000;
const RANK_BUDGET: usize = 15_000;

fn bounds() -> RetrievalBounds {
    RetrievalBounds {
        max_probes: NonZeroUsize::new(8).unwrap(),
        scan_rows: NonZeroUsize::new(64).unwrap(),
        max_accepted: NonZeroUsize::new(64).unwrap(),
        batch_rows: NonZeroUsize::new(2).unwrap(),
        qualifying_matches: NonZeroUsize::new(QUALIFYING_MATCHES).unwrap(),
        rank_budget: NonZeroUsize::new(RANK_BUDGET).unwrap(),
    }
}

fn probes(request: &str) -> Vec<Probe> {
    compile(
        &analyze(
            request,
            LexicalBounds {
                max_input_bytes: NonZeroUsize::new(256).unwrap(),
                max_atoms: NonZeroUsize::new(8).unwrap(),
            },
        )
        .unwrap(),
    )
}

#[derive(Debug, Clone)]
struct Row {
    class: OccurrenceClass,
    object: String,
    text: String,
}

impl Row {
    fn claim(object: &str, text: &str) -> Self {
        Self {
            class: OccurrenceClass::CanonicalClaims,
            object: object.to_string(),
            text: text.to_string(),
        }
    }

    fn identity(&self) -> Vec<(&str, &str)> {
        match self.class {
            OccurrenceClass::GitCommits => vec![
                ("repository_id", "repo"),
                ("object_format", "sha1"),
                ("oid", COMMIT_OID),
            ],
            _ => vec![("object_id", self.object.as_str())],
        }
    }

    fn representation(&self) -> &'static str {
        match self.class {
            OccurrenceClass::GitCommits => "commit_message",
            _ => "decision_summary",
        }
    }

    fn occurrence<'a>(&'a self, identity: &'a [(&'a str, &'a str)]) -> Occurrence<'a> {
        Occurrence {
            class: self.class.code(),
            identity,
            revision: "1",
            representation: self.representation(),
            span: None,
        }
    }

    fn occurrence_id(&self) -> String {
        kernel::source_identity::encode_preserving_span(&self.occurrence(&self.identity()))
            .unwrap()
            .occurrence_id
    }
}

fn corpus() -> Vec<Row> {
    vec![
        Row::claim("alpha", "parse_request handles HTTPServer input"),
        Row::claim("beta", "fetch OR fallback parse"),
        Row::claim("gamma", "parse parse parse"),
        Row::claim("delta", "an unrelated note about io"),
        Row::claim("epsilon", "NEAR miss on a fetch path"),
        Row {
            class: OccurrenceClass::GitCommits,
            object: "zeta".to_string(),
            text: "commit: parse the manifest".to_string(),
        },
    ]
}

/// A fixture's directory: a temporary one removed on drop, or one an earlier scale run kept.
enum Root {
    Temp(tempfile::TempDir),
    Kept(std::path::PathBuf),
}

impl Root {
    fn path(&self) -> &Path {
        match self {
            Self::Temp(dir) => dir.path(),
            Self::Kept(path) => path,
        }
    }
}

struct Fixture {
    root: Root,
    kernel: KernelStore,
    store: SqliteStore,
    incarnation: String,
    project: ProjectScope,
    rows: Vec<Row>,
}

impl Fixture {
    fn new(admitted: &[&str]) -> Self {
        let root = tempfile::tempdir().unwrap();
        let kernel = KernelStore::open(root.path().join("kernel")).unwrap();
        kernel
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
                envelope.insert_scope(scope(SCOPE_A, PROJECT_A))?;
                for object in OBJECTS {
                    envelope.insert_decision(decision(object))?;
                    if admitted.contains(&object) {
                        envelope.record_admission(admission(object))?;
                    }
                }
                Ok(String::new())
            })
            .unwrap();
        let incarnation = kernel
            .database_incarnation_id_within_budget(&EvalBudget::unbounded())
            .unwrap();
        let store = open_store(root.path());
        store
            .with_conn_fenced(|conn| {
                install_identity(
                    conn,
                    &ProjectionIdentity {
                        schema_version: retrieval::SCHEMA_VERSION,
                        kernel_incarnation_id: incarnation.clone(),
                        projection_policy_version: "source-policy.v1".to_string(),
                        identity_contract_version: "search-projection-identity-v3".to_string(),
                        limit_manifest_protocol_version: "limits.v1".to_string(),
                        embedding_model: "model-a".to_string(),
                        tokenizer_fingerprint: "fp-a".to_string(),
                        analysis_identity: retrieval::lexical::AnalysisIdentity::current()
                            .as_str()
                            .to_string(),
                        vector_dimension: 8,
                        generation_epoch: 1,
                    },
                    1,
                )
                .unwrap();
                Ok(())
            })
            .unwrap();
        let fixture = Self {
            root: Root::Temp(root),
            kernel,
            store,
            incarnation,
            project: ProjectScope::new(PROJECT_A).unwrap(),
            rows: corpus(),
        };
        fixture.project(&fixture.rows);
        fixture
    }

    fn all_admitted() -> Self {
        Self::new(&OBJECTS)
    }

    /// Reopens a projection and kernel a scale run kept with `EIDNARA_LEXICAL_SCALE_KEEP`, so later measurements reuse one build.
    fn reopen(root: Root) -> Self {
        let kernel = KernelStore::open(root.path().join("kernel")).unwrap();
        let incarnation = kernel
            .database_incarnation_id_within_budget(&EvalBudget::unbounded())
            .unwrap();
        let store = open_store(root.path());
        Self {
            root,
            kernel,
            store,
            incarnation,
            project: ProjectScope::new(PROJECT_A).unwrap(),
            rows: corpus(),
        }
    }

    fn retire(&self, object: &str) {
        self.kernel
            .commit(intent(&format!("retire-{object}")), |envelope| {
                envelope.retire_decision(object)?;
                Ok(String::new())
            })
            .unwrap();
    }

    /// Makes `objects` decisions the kernel admits, so their occurrences become eligible contributions.
    fn admit(&self, objects: &[&str]) {
        self.kernel
            .commit(
                intent(&format!("admit-{}", objects.join("+"))),
                |envelope| {
                    for object in objects {
                        envelope.insert_decision(decision(object))?;
                        envelope.record_admission(admission(object))?;
                    }
                    Ok(String::new())
                },
            )
            .unwrap();
    }

    fn project(&self, rows: &[Row]) {
        let through = self.kernel.tip().unwrap();
        let identities: Vec<Vec<(&str, &str)>> = rows.iter().map(Row::identity).collect();
        let records: Vec<OccurrenceRecord<'_>> = rows
            .iter()
            .zip(&identities)
            .map(|(row, identity)| OccurrenceRecord {
                occurrence: row.occurrence(identity),
                payload: Payload::Whole(&row.text),
                domain_id: DOMAIN,
                sensitivity: Sensitivity::Normal,
                source_object_id: &row.object,
                source_evidence_id: "evidence",
                source_artifact_digest: DIGEST,
                created_commit_seq: through,
            })
            .collect();
        let batch = ProjectionBatch {
            identity: MutationIdentity {
                kernel_incarnation_id: self.incarnation.clone(),
                hold_id: HOLD.to_string(),
                snapshot_commit_seq: 0,
                through_commit_seq: through,
            },
            records,
            invalidations: vec![],
            generation_id: None,
        };
        self.store
            .with_conn_fenced(|conn| {
                apply_batch(conn, &batch, batch_bounds(), through).unwrap();
                Ok(())
            })
            .unwrap();
    }

    fn id(&self, object: &str) -> String {
        self.rows
            .iter()
            .find(|row| row.object == object)
            .unwrap()
            .occurrence_id()
    }

    fn authority(&self) -> Authority<'_> {
        Authority {
            project: &self.project,
            destination: ArtifactDestination::Local,
        }
    }

    fn retrieve(
        &self,
        probes: &[Probe],
        bounds: RetrievalBounds,
        budget: &EvalBudget,
    ) -> Result<Retrieval, RetrievalRefusal> {
        self.store
            .with_conn(|conn| {
                Ok(retrieve(
                    conn,
                    &self.kernel,
                    probes,
                    self.authority(),
                    bounds,
                    budget,
                ))
            })
            .unwrap()
    }

    /// The occurrence identifiers `scan` keeps, in comparator order, before any occurrence row is read.
    fn kept(&self, probes: &[Probe], bounds: RetrievalBounds) -> Vec<String> {
        self.store
            .with_conn(|conn| {
                let budget = EvalBudget::unbounded();
                Ok(scan(conn, probes, bounds, &budget)
                    .unwrap()
                    .hit_ids(&budget)
                    .unwrap())
            })
            .unwrap()
    }

    /// `retrieve` with the allocator recording only the retrieval call, after the connection is checked out.
    fn retrieve_recorded(
        &self,
        probes: &[Probe],
        bounds: RetrievalBounds,
    ) -> (Retrieval, alloc_recorder::Ledger) {
        self.store
            .with_conn(|conn| {
                Ok(alloc_recorder::record_window(|| {
                    retrieve(
                        conn,
                        &self.kernel,
                        probes,
                        self.authority(),
                        bounds,
                        &EvalBudget::unbounded(),
                    )
                    .unwrap()
                }))
            })
            .unwrap()
    }

    fn retrieve_with_hook(
        &self,
        probes: &[Probe],
        bounds: RetrievalBounds,
        budget: &EvalBudget,
        hook: impl FnMut(Window),
    ) -> Result<Retrieval, RetrievalRefusal> {
        self.store
            .with_conn(|conn| {
                Ok(retrieve_with_hook_for_test(
                    conn,
                    &self.kernel,
                    probes,
                    self.authority(),
                    bounds,
                    budget,
                    hook,
                ))
            })
            .unwrap()
    }

    fn ids(&self, probes: &[Probe]) -> Vec<String> {
        ids_of(
            &self
                .retrieve(probes, bounds(), &EvalBudget::unbounded())
                .unwrap(),
        )
    }

    fn reference(&self, probes: &[Probe]) -> Vec<String> {
        self.keyed_reference(probes)
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    }

    /// The test oracle: each occurrence's lowest SQL rank across `probes`, sorted by rank then id.
    fn keyed_reference(&self, probes: &[Probe]) -> Vec<(String, f64)> {
        self.store
            .with_conn(|conn| {
                let mut best: Vec<(String, f64)> = Vec::new();
                for probe in probes {
                    for (id, rank) in ranks(conn, probe) {
                        match best.iter_mut().find(|(seen, _)| *seen == id) {
                            Some((_, current)) => *current = current.min(rank),
                            None => best.push((id, rank)),
                        }
                    }
                }
                best.sort_by(|(a_id, a), (b_id, b)| a.total_cmp(b).then_with(|| a_id.cmp(b_id)));
                Ok(best)
            })
            .unwrap()
    }

    fn raw(&self) -> Connection {
        Connection::open(self.root.path().join("search").join("search.sqlite")).unwrap()
    }

    /// The subsequence of `occurrence_ids` the kernel admits: only the seeded corpus objects are decisions it knows.
    fn eligible(&self, occurrence_ids: &[String]) -> Vec<String> {
        let admitted: Vec<String> = self.rows.iter().map(Row::occurrence_id).collect();
        occurrence_ids
            .iter()
            .filter(|occurrence_id| admitted.contains(occurrence_id))
            .cloned()
            .collect()
    }
}

fn ranks(conn: &GuardedConn<'_>, probe: &Probe) -> Vec<(String, f64)> {
    conn.prepare("SELECT occurrence_id, rank FROM lexical WHERE lexical MATCH ?1")
        .unwrap()
        .query_map([probe], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn ids_of(retrieval: &Retrieval) -> Vec<String> {
    retrieval
        .contributions
        .iter()
        .map(|c| c.occurrence_id.clone())
        .collect()
}

fn sorted(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids
}

fn keyed(retrieval: &Retrieval) -> Vec<(String, f64)> {
    retrieval
        .contributions
        .iter()
        .map(|c| (c.occurrence_id.clone(), c.rank))
        .collect()
}

fn ordinal_of(retrieval: &Retrieval, occurrence_id: &str) -> usize {
    retrieval
        .contributions
        .iter()
        .find(|c| c.occurrence_id == occurrence_id)
        .unwrap()
        .ordinal
}

#[test]
fn a_probe_matches_only_its_term_and_operators_in_text_stay_literal() {
    let fixture = Fixture::all_admitted();

    let parse = fixture
        .retrieve(&probes("parse"), bounds(), &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(
        sorted(ids_of(&parse)),
        sorted(vec![
            fixture.id("alpha"),
            fixture.id("beta"),
            fixture.id("gamma"),
            fixture.id("zeta"),
        ])
    );
    for contribution in &parse.contributions {
        let expected = if contribution.occurrence_id == fixture.id("zeta") {
            OccurrenceClass::GitCommits
        } else {
            OccurrenceClass::CanonicalClaims
        };
        assert_eq!(contribution.class, expected);
    }
    assert_eq!(fixture.ids(&probes("OR")), vec![fixture.id("beta")]);
    assert_eq!(fixture.ids(&probes("NEAR")), vec![fixture.id("epsilon")]);
    assert_eq!(fixture.ids(&probes("io")), vec![fixture.id("delta")]);
    assert_eq!(fixture.ids(&probes("server")), vec![fixture.id("alpha")]);
    assert!(fixture.ids(&probes("absent")).is_empty());
}

#[test]
fn zero_probes_run_no_match_while_a_control_probe_contributes() {
    let fixture = Fixture::all_admitted();

    let empty = fixture
        .retrieve(&probes("!!! ..."), bounds(), &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(empty.completion, Completion::Empty);
    assert!(empty.contributions.is_empty());
    assert_eq!(empty.consumed.probes, 0);
    assert_eq!(empty.consumed.scanned_rows, 0);
    assert_eq!(empty.consumed.batches, 0);
    assert_eq!(empty.snapshot, None);

    let control = fixture
        .retrieve(&probes("io"), bounds(), &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(control.completion, Completion::Complete);
    assert_eq!(control.contributions.len(), 1);
    assert_eq!(control.consumed.probes, 1);
    assert_eq!(control.consumed.scanned_rows, 1);
    assert_eq!(control.consumed.batches, 2);
    assert!(control.snapshot.is_some());
}

#[test]
fn scan_then_admit_under_one_connection_equals_retrieve_and_carries_the_judged_candidate() {
    let fixture = Fixture::all_admitted();
    let request = probes("parse fetch io");
    let reference = fixture
        .retrieve(&request, bounds(), &EvalBudget::unbounded())
        .unwrap();
    let admitted = fixture
        .store
        .with_conn(|conn| {
            let scanned = scan(conn, &request, bounds(), &EvalBudget::unbounded()).unwrap();
            assert_eq!(scanned.hits(), 6);
            // Admission reads each candidate's occurrence row under the connection the scan used.
            Ok(admit(
                scanned,
                &fixture.kernel,
                fixture.authority(),
                &EvalBudget::unbounded(),
            ))
        })
        .unwrap()
        .unwrap();
    assert_eq!(admitted, reference);
    for contribution in &admitted.contributions {
        let candidate = contribution.occurrence_candidate();
        assert_eq!(candidate.occurrence_id, contribution.occurrence_id);
        assert_eq!(candidate.class, contribution.class);
        assert_eq!(
            candidate.candidate.artifact_digest.as_deref(),
            Some(DIGEST),
            "the contribution carries the terms the kernel judged"
        );
    }
}

#[test]
fn contributions_follow_the_reference_order_and_survive_probe_duplication_and_permutation() {
    let fixture = Fixture::all_admitted();

    let request = probes("parse fetch io");
    let retrieval = fixture
        .retrieve(&request, bounds(), &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(retrieval.completion, Completion::Complete);
    assert_eq!(keyed(&retrieval), fixture.keyed_reference(&request));
    assert_eq!(retrieval.contributions.len(), 6);
    assert!(
        retrieval
            .contributions
            .windows(2)
            .all(|pair| pair[0].rank <= pair[1].rank)
    );

    let permuted = fixture
        .retrieve(
            &probes("io fetch parse"),
            bounds(),
            &EvalBudget::unbounded(),
        )
        .unwrap();
    let duplicated = fixture
        .retrieve(
            &probes("parse parse io fetch fetch parse"),
            bounds(),
            &EvalBudget::unbounded(),
        )
        .unwrap();
    assert_eq!(keyed(&permuted), keyed(&retrieval));
    assert_eq!(keyed(&duplicated), keyed(&retrieval));
    assert_eq!(duplicated.consumed.probes, 6);
    let gamma = fixture.id("gamma");
    assert_eq!(ordinal_of(&retrieval, &gamma), 0);
    assert_eq!(ordinal_of(&permuted, &gamma), 2);
    assert_eq!(ordinal_of(&duplicated, &gamma), 0);
}

#[test]
fn equal_ranks_from_distinct_probes_keep_the_lowest_ordinal() {
    let fixture = Fixture::all_admitted();
    let delta = fixture.id("delta");
    let forward = fixture
        .retrieve(
            &probes("note unrelated"),
            bounds(),
            &EvalBudget::unbounded(),
        )
        .unwrap();
    let backward = fixture
        .retrieve(
            &probes("unrelated note"),
            bounds(),
            &EvalBudget::unbounded(),
        )
        .unwrap();
    let rank_of = |probe: &str| {
        fixture
            .store
            .with_conn(|conn| Ok(ranks(conn, &probes(probe)[0])))
            .unwrap()
            .into_iter()
            .find(|(id, _)| *id == delta)
            .map(|(_, rank)| rank)
            .unwrap()
    };
    assert_eq!(rank_of("note"), rank_of("unrelated"));
    assert_eq!(ordinal_of(&forward, &delta), 0);
    assert_eq!(ordinal_of(&backward, &delta), 0);
    assert_eq!(keyed(&forward), keyed(&backward));
}

#[test]
fn an_ineligible_leader_is_excluded_without_taking_an_accepted_slot() {
    let fixture = Fixture::new(&["alpha", "beta", "delta", "epsilon", "zeta"]);

    let request = probes("parse");
    let reference = fixture.reference(&request);
    assert_eq!(reference[0], fixture.id("gamma"));
    let one_slot = RetrievalBounds {
        max_accepted: NonZeroUsize::new(1).unwrap(),
        batch_rows: NonZeroUsize::new(1).unwrap(),
        ..bounds()
    };
    let retrieval = fixture
        .retrieve(&request, one_slot, &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(ids_of(&retrieval), vec![reference[1].clone()]);
    assert_eq!(
        retrieval.completion,
        Completion::Incomplete(IncompleteReason::AcceptedBound)
    );
    assert_eq!(
        retrieval.consumed.excluded,
        vec![(EligibilityVerdict::Hidden, 1)]
    );
    assert_eq!(retrieval.consumed.judged, 3);
    assert_eq!(retrieval.consumed.batches, 3);

    let all = fixture
        .retrieve(&request, bounds(), &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(all.completion, Completion::Complete);
    assert_eq!(ids_of(&all), reference[1..].to_vec());
}

/// The first batch judges the open accepted slots and each later one scales the slots left by the matches read per acceptance so far, so a short tail is judged in one small batch rather than one candidate at a time.
#[test]
fn admission_batches_scale_the_open_slots_by_the_acceptance_so_far_and_an_exact_fill_stays_complete()
 {
    let fixture = Fixture::new(&["alpha", "beta", "delta", "epsilon", "zeta"]);
    let request = probes("parse");
    let reference = fixture.reference(&request);
    assert_eq!(reference.len(), 4);

    let two_of_three = RetrievalBounds {
        max_accepted: NonZeroUsize::new(2).unwrap(),
        batch_rows: NonZeroUsize::new(4).unwrap(),
        ..bounds()
    };
    let retrieval = fixture
        .retrieve(&request, two_of_three, &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(ids_of(&retrieval), reference[1..3].to_vec());
    assert_eq!(
        retrieval.completion,
        Completion::Incomplete(IncompleteReason::AcceptedBound)
    );
    // Two open slots take the hidden leader and one eligible row; one acceptance in two reads scales the one slot left to two candidates, and revalidation re-judges both accepted rows.
    assert_eq!(retrieval.consumed.judged, 2 + 2 + 2);
    assert_eq!(retrieval.consumed.batches, 3);
    assert_eq!(
        retrieval.consumed.excluded,
        vec![(EligibilityVerdict::Hidden, 1)]
    );

    let exact_fill = RetrievalBounds {
        max_accepted: NonZeroUsize::new(3).unwrap(),
        batch_rows: NonZeroUsize::new(4).unwrap(),
        ..bounds()
    };
    let filled = fixture
        .retrieve(&request, exact_fill, &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(filled.completion, Completion::Complete);
    assert_eq!(ids_of(&filled), reference[1..].to_vec());
}

#[test]
fn the_scan_bound_marks_incomplete_only_past_the_bound_and_refusals_precede_every_probe() {
    let fixture = Fixture::all_admitted();

    let one_row = RetrievalBounds {
        scan_rows: NonZeroUsize::new(1).unwrap(),
        ..bounds()
    };
    let truncated = fixture
        .retrieve(&probes("parse"), one_row, &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(
        truncated.completion,
        Completion::Incomplete(IncompleteReason::ScanBound)
    );
    assert_eq!(truncated.contributions.len(), 1);
    assert_eq!(truncated.consumed.scanned_rows, 1);

    let exact = RetrievalBounds {
        scan_rows: NonZeroUsize::new(4).unwrap(),
        ..bounds()
    };
    let filled = fixture
        .retrieve(&probes("parse"), exact, &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(filled.completion, Completion::Complete);
    assert_eq!(filled.consumed.scanned_rows, 4);

    let two_probes = RetrievalBounds {
        max_probes: NonZeroUsize::new(2).unwrap(),
        ..bounds()
    };
    assert_eq!(
        fixture.retrieve(
            &probes("parse fetch io"),
            two_probes,
            &EvalBudget::unbounded()
        ),
        Err(RetrievalRefusal::ProbesOverBound {
            probes: 3,
            bound: 2
        })
    );

    let over = MAX_ELIGIBILITY_CANDIDATES + 1;
    for (bound, over_kernel) in [
        (
            "max_accepted",
            RetrievalBounds {
                max_accepted: NonZeroUsize::new(over).unwrap(),
                ..two_probes
            },
        ),
        (
            "batch_rows",
            RetrievalBounds {
                batch_rows: NonZeroUsize::new(over).unwrap(),
                ..two_probes
            },
        ),
    ] {
        assert_eq!(
            fixture.retrieve(
                &probes("parse fetch io"),
                over_kernel,
                &EvalBudget::unbounded()
            ),
            Err(RetrievalRefusal::BatchOverBound { bound, value: over })
        );
        let cancelled = EvalBudget::unbounded();
        cancelled.cancel();
        assert_eq!(
            fixture.retrieve(&probes("parse"), over_kernel, &cancelled),
            Err(RetrievalRefusal::BudgetExhausted)
        );
    }
}

#[test]
fn a_tombstoned_row_is_excluded_at_the_engine() {
    let fixture = Fixture::all_admitted();
    let request = probes("parse");
    let before = fixture.reference(&request);
    assert!(before.contains(&fixture.id("gamma")));

    fixture
        .raw()
        .execute(
            "INSERT INTO occurrence_tombstones(occurrence_id, invalidated_commit_seq, reason, recorded_at) VALUES (?1, 99, 'retired', 0)",
            [fixture.id("gamma")],
        )
        .unwrap();

    assert_stale_row_excluded_at_every_scan_bound(&fixture, &request, &before, "gamma");
}

#[test]
fn an_orphaned_lexical_row_is_excluded_at_the_engine() {
    let fixture = Fixture::all_admitted();
    let request = probes("parse");
    let before = fixture.reference(&request);
    assert!(before.contains(&fixture.id("gamma")));

    // Removing the occurrence while its lexical row stays orphans the match.
    let raw = fixture.raw();
    raw.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
    raw.execute(
        "DELETE FROM occurrences WHERE occurrence_id=?1",
        [fixture.id("gamma")],
    )
    .unwrap();
    let orphaned: i64 = raw
        .query_row(
            "SELECT count(*) FROM lexical WHERE occurrence_id=?1",
            [fixture.id("gamma")],
            |row| row.get(0),
        )
        .unwrap();
    assert!(orphaned > 0);

    assert_stale_row_excluded_at_every_scan_bound(&fixture, &request, &before, "gamma");
}

/// `before` is the reference order with `stale` still live. For every `scan_rows` from 1 through `before.len()`, the
/// window keeps the first `scan_rows` matches of that order with `stale` among them, admission drops `stale` without
/// replacing it, and truncation is reported while matches remain past the bound.
fn assert_stale_row_excluded_at_every_scan_bound(
    fixture: &Fixture,
    request: &[Probe],
    before: &[String],
    stale: &str,
) {
    let stale_id = fixture.id(stale);
    let retrieval = fixture
        .retrieve(request, bounds(), &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(retrieval.completion, Completion::Complete);
    assert_eq!(retrieval.consumed.scanned_rows, before.len());
    assert_eq!(retrieval.consumed.dead_rows, 1);
    assert!(!ids_of(&retrieval).contains(&stale_id));
    assert_eq!(retrieval.contributions.len(), before.len() - 1);

    let stale_at = before.iter().position(|id| *id == stale_id).unwrap();
    for scan_rows in 1..=before.len() {
        let tight = RetrievalBounds {
            scan_rows: NonZeroUsize::new(scan_rows).unwrap(),
            ..bounds()
        };
        let retrieval = fixture
            .retrieve(request, tight, &EvalBudget::unbounded())
            .unwrap();
        let want: Vec<String> = before[..scan_rows]
            .iter()
            .filter(|id| **id != stale_id)
            .cloned()
            .collect();
        assert_eq!(ids_of(&retrieval), want, "scan_rows={scan_rows}");
        assert_eq!(
            retrieval.consumed.dead_rows,
            usize::from(stale_at < scan_rows),
            "scan_rows={scan_rows} stale_at={stale_at}"
        );
        assert_eq!(
            retrieval.completion,
            if scan_rows < before.len() {
                Completion::Incomplete(IncompleteReason::ScanBound)
            } else {
                Completion::Complete
            },
            "scan_rows={scan_rows} stale_at={stale_at}"
        );
    }
}

/// Every `bulk` row says `parse` in three tokens, so their ranks tie and only identifier bytes order them.
fn project_bulk(fixture: &Fixture, count: usize) -> Vec<Row> {
    let bulk: Vec<Row> = (0..count)
        .map(|n| Row::claim(&format!("bulk-{n}"), &format!("bulk parse {n}")))
        .collect();
    fixture.project(&bulk);
    bulk
}

fn object_of<'a>(rows: &'a [Row], occurrence_id: &str) -> &'a str {
    &rows
        .iter()
        .find(|row| row.occurrence_id() == occurrence_id)
        .unwrap()
        .object
}

fn tombstone_raw(fixture: &Fixture, occurrence_id: &str) {
    fixture
        .raw()
        .execute(
            "INSERT INTO occurrence_tombstones(occurrence_id, invalidated_commit_seq, reason, recorded_at) VALUES (?1, 99, 'retired', 0)",
            [occurrence_id],
        )
        .unwrap();
}

/// Tombstoned rows inside the scan bound keep their window slots and are dropped unjudged at admission, so the live rows past the bound stay unread and truncation is still reported.
#[test]
fn tombstoned_rows_inside_the_scan_bound_are_dropped_without_replacement() {
    let fixture = Fixture::all_admitted();
    let bulk = project_bulk(&fixture, 2500);
    let request = probes("parse");
    let reference = fixture.reference(&request);
    let scan = bounds().scan_rows.get();
    assert!(reference.len() > 2 * scan);

    let dead = 6;
    for occurrence_id in &reference[..dead] {
        tombstone_raw(&fixture, occurrence_id);
    }
    // Admitted sentinels sit on the last dead row, the first live row, the last kept row, and the first row past the bound.
    let sentinels = [dead - 1, dead, scan - 1, scan];
    let objects: Vec<&str> = sentinels
        .iter()
        .map(|&index| object_of(&bulk, &reference[index]))
        .collect();
    fixture.admit(&objects);
    let expected: Vec<String> = [dead, scan - 1]
        .iter()
        .map(|&index| reference[index].clone())
        .collect();

    let wide = RetrievalBounds {
        max_accepted: NonZeroUsize::new(MAX_ELIGIBILITY_CANDIDATES).unwrap(),
        batch_rows: NonZeroUsize::new(MAX_ELIGIBILITY_CANDIDATES).unwrap(),
        ..bounds()
    };
    let retrieval = fixture
        .retrieve(&request, wide, &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(ids_of(&retrieval), expected);
    assert_eq!(retrieval.consumed.scanned_rows, scan);
    assert_eq!(retrieval.consumed.dead_rows, dead);
    assert_eq!(retrieval.consumed.judged, scan - dead + expected.len());
    assert_eq!(
        retrieval.completion,
        Completion::Incomplete(IncompleteReason::ScanBound)
    );
}

/// A dead row inside the window still counts toward the scan bound: one match past the bound reports truncation even though the live matches would fit.
#[test]
fn a_dead_row_inside_the_window_still_counts_toward_the_bound() {
    let fixture = Fixture::all_admitted();
    let request = probes("parse");
    let scan = bounds().scan_rows.get();
    let corpus_hits = fixture.reference(&request).len();
    project_bulk(&fixture, scan + 1 - corpus_hits);
    let reference = fixture.reference(&request);
    assert_eq!(reference.len(), scan + 1);

    tombstone_raw(&fixture, &reference[0]);
    let retrieval = fixture
        .retrieve(
            &request,
            RetrievalBounds {
                max_accepted: NonZeroUsize::new(MAX_ELIGIBILITY_CANDIDATES).unwrap(),
                batch_rows: NonZeroUsize::new(MAX_ELIGIBILITY_CANDIDATES).unwrap(),
                ..bounds()
            },
            &EvalBudget::unbounded(),
        )
        .unwrap();
    assert_eq!(ids_of(&retrieval), fixture.eligible(&reference[1..scan]));
    assert_eq!(retrieval.consumed.scanned_rows, scan);
    assert_eq!(retrieval.consumed.dead_rows, 1);
    assert_eq!(
        retrieval.consumed.judged,
        scan - 1 + retrieval.contributions.len()
    );
    assert_eq!(
        retrieval.completion,
        Completion::Incomplete(IncompleteReason::ScanBound)
    );
}

#[test]
fn a_repeated_probe_runs_the_engine_once_and_adds_no_work() {
    let fixture = Fixture::all_admitted();
    project_bulk(&fixture, 2500);
    let wide = RetrievalBounds {
        scan_rows: NonZeroUsize::new(4096).unwrap(),
        max_accepted: NonZeroUsize::new(MAX_ELIGIBILITY_CANDIDATES).unwrap(),
        batch_rows: NonZeroUsize::new(MAX_ELIGIBILITY_CANDIDATES).unwrap(),
        ..bounds()
    };
    let polls_for = |request: &str| {
        let polls = Arc::new(AtomicUsize::new(0));
        let counting = {
            let polls = Arc::clone(&polls);
            move || {
                polls.fetch_add(1, Ordering::Relaxed);
                false
            }
        };
        let retrieval = fixture
            .store
            .with_conn_interruptible(Instant::now() + Duration::from_secs(60), counting, |conn| {
                Ok(retrieve(
                    conn,
                    &fixture.kernel,
                    &probes(request),
                    fixture.authority(),
                    wide,
                    &EvalBudget::unbounded(),
                ))
            })
            .unwrap()
            .unwrap();
        (retrieval, polls.load(Ordering::Relaxed))
    };

    let (single, single_polls) = polls_for("parse");
    let (repeated, repeated_polls) = polls_for("parse parse parse parse");
    assert_eq!(single.completion, Completion::Complete);
    assert_eq!(single.consumed.probes, 1);
    assert_eq!(single.consumed.scanned_rows, 2504);
    assert_eq!(repeated.consumed.probes, 4);
    // Counters record engine work, so the repeats add no counted or scanned rows.
    assert_eq!(repeated.consumed.scanned_rows, 2504);
    assert_eq!(repeated.consumed.counted_rows, single.consumed.counted_rows);
    assert_eq!(
        repeated.consumed.ranked_matches,
        single.consumed.ranked_matches
    );
    assert_eq!(keyed(&repeated), keyed(&single));
    assert!(
        repeated
            .contributions
            .iter()
            .all(|contribution| contribution.ordinal == 0)
    );
    assert!(single_polls >= 3, "the handler polled: {single_polls}");
    // The progress handler polls per VM-instruction interval, so four engine runs would poll about four times as often.
    assert!(
        repeated_polls < 2 * single_polls,
        "repeated probes rescanned: {repeated_polls} polls against {single_polls}"
    );

    let distinct = fixture
        .retrieve(
            &probes("parse fetch parse fetch"),
            wide,
            &EvalBudget::unbounded(),
        )
        .unwrap();
    let forward = fixture
        .retrieve(&probes("parse fetch"), wide, &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(keyed(&distinct), keyed(&forward));
    assert_eq!(distinct.consumed.probes, 4);
    assert_eq!(
        distinct.consumed.scanned_rows,
        forward.consumed.scanned_rows
    );
}

#[test]
fn revalidation_drops_an_occurrence_retired_after_admission() {
    let fixture = Fixture::all_admitted();
    let request = probes("parse");
    let retrieval = fixture
        .retrieve_with_hook(&request, bounds(), &EvalBudget::unbounded(), |window| {
            if window == Window::BeforeRevalidation {
                fixture.retire("gamma");
            }
        })
        .unwrap();
    assert!(!ids_of(&retrieval).contains(&fixture.id("gamma")));
    assert_eq!(retrieval.contributions.len(), 3);
    assert_eq!(
        retrieval.completion,
        Completion::Incomplete(IncompleteReason::SnapshotChanged)
    );
    assert_eq!(
        retrieval.consumed.excluded,
        vec![(EligibilityVerdict::Retracted, 1)]
    );
}

#[test]
fn a_snapshot_that_moves_between_admission_batches_stops_admission() {
    let fixture = Fixture::all_admitted();
    let request = probes("parse");
    let reference = fixture.reference(&request);
    let one_per_batch = RetrievalBounds {
        batch_rows: NonZeroUsize::new(1).unwrap(),
        ..bounds()
    };
    let retrieval = fixture
        .retrieve_with_hook(
            &request,
            one_per_batch,
            &EvalBudget::unbounded(),
            |window| {
                if window == Window::AfterBatch(1) {
                    fixture.retire("delta");
                }
            },
        )
        .unwrap();
    assert_eq!(
        retrieval.completion,
        Completion::Incomplete(IncompleteReason::SnapshotChanged)
    );
    assert_eq!(ids_of(&retrieval), vec![reference[0].clone()]);
    assert_eq!(retrieval.consumed.batches, 3);
    assert_eq!(retrieval.consumed.judged, 3);
}

#[test]
fn a_moved_snapshot_batch_still_tallies_its_exclusions() {
    let fixture = Fixture::all_admitted();
    let request = probes("parse");
    let reference = fixture.reference(&request);
    let second = fixture
        .rows
        .iter()
        .find(|row| row.occurrence_id() == reference[1])
        .unwrap()
        .object
        .clone();
    let one_per_batch = RetrievalBounds {
        batch_rows: NonZeroUsize::new(1).unwrap(),
        ..bounds()
    };
    let retrieval = fixture
        .retrieve_with_hook(
            &request,
            one_per_batch,
            &EvalBudget::unbounded(),
            |window| {
                if window == Window::AfterBatch(1) {
                    fixture.retire(&second);
                }
            },
        )
        .unwrap();
    assert_eq!(
        retrieval.completion,
        Completion::Incomplete(IncompleteReason::SnapshotChanged)
    );
    assert_eq!(ids_of(&retrieval), vec![reference[0].clone()]);
    assert_eq!(retrieval.consumed.judged, 3);
    assert_eq!(
        retrieval.consumed.excluded,
        vec![(EligibilityVerdict::Retracted, 1)]
    );
}

#[test]
fn a_kernel_restore_before_revalidation_marks_the_incarnation_change() {
    let fixture = Fixture::all_admitted();
    let backup_dir = tempfile::tempdir().unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(backup_dir.path(), std::fs::Permissions::from_mode(0o700))
            .unwrap();
    }
    let manifest = fixture
        .kernel
        .backup(BackupRequest {
            destination_directory: backup_dir.path().to_path_buf(),
            deadline: Instant::now() + Duration::from_secs(10),
            capture_pin_expires_at: None,
        })
        .unwrap();
    let request = probes("parse");
    let retrieval = fixture
        .retrieve_with_hook(&request, bounds(), &EvalBudget::unbounded(), |window| {
            if window == Window::BeforeRevalidation {
                fixture.kernel.restore(&manifest.destination_path).unwrap();
            }
        })
        .unwrap();
    assert_eq!(
        retrieval.completion,
        Completion::Incomplete(IncompleteReason::KernelIncarnationChanged)
    );
    assert_eq!(retrieval.contributions.len(), 4);
}

#[test]
fn retrieval_reads_no_payload_bytes() {
    let fixture = Fixture::all_admitted();
    let request = probes("parse fetch io");
    let before = fixture
        .retrieve(&request, bounds(), &EvalBudget::unbounded())
        .unwrap();

    // Renaming the table makes a payload read fail instead of returning zeroed bytes.
    let raw = fixture.raw();
    raw.execute("ALTER TABLE payloads RENAME TO payloads_hidden", [])
        .unwrap();
    assert!(
        raw.query_row("SELECT count(*) FROM payloads", [], |row| row
            .get::<_, i64>(0))
            .is_err()
    );

    let after = fixture
        .retrieve(&request, bounds(), &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(after, before);
    assert_eq!(after.contributions.len(), 6);
}

#[test]
fn a_budget_that_ends_after_a_probe_yields_an_incomplete_result_with_no_contributions() {
    let fixture = Fixture::all_admitted();
    let request = probes("parse fetch io");

    let cancelled = EvalBudget::unbounded();
    cancelled.cancel();
    assert_eq!(
        fixture.retrieve(&request, bounds(), &cancelled),
        Err(RetrievalRefusal::BudgetExhausted)
    );

    let budget = EvalBudget::unbounded();
    let after_first_batch = fixture
        .retrieve_with_hook(&request, bounds(), &budget, |window| {
            if window == Window::AfterBatch(1) {
                budget.cancel();
            }
        })
        .unwrap();
    assert_eq!(
        after_first_batch.completion,
        Completion::Incomplete(IncompleteReason::BudgetExhausted)
    );
    assert!(after_first_batch.contributions.is_empty());
    assert_eq!(after_first_batch.consumed.batches, 1);
    assert_eq!(after_first_batch.consumed.probes, 3);

    let budget = EvalBudget::unbounded();
    let before_revalidation = fixture
        .retrieve_with_hook(&request, bounds(), &budget, |window| {
            if window == Window::BeforeRevalidation {
                budget.cancel();
            }
        })
        .unwrap();
    assert_eq!(
        before_revalidation.completion,
        Completion::Incomplete(IncompleteReason::BudgetExhausted)
    );
    assert!(before_revalidation.contributions.is_empty());
    assert_eq!(before_revalidation.consumed.batches, 3);

    for (text, limits, completion) in [
        (
            "parse",
            RetrievalBounds {
                scan_rows: NonZeroUsize::new(1).unwrap(),
                ..bounds()
            },
            Completion::Incomplete(IncompleteReason::ScanBound),
        ),
        (
            "parse",
            RetrievalBounds {
                max_accepted: NonZeroUsize::new(1).unwrap(),
                ..bounds()
            },
            Completion::Incomplete(IncompleteReason::AcceptedBound),
        ),
        ("absent", bounds(), Completion::Complete),
    ] {
        let request = probes(text);
        let control = fixture
            .retrieve(&request, limits, &EvalBudget::unbounded())
            .unwrap();
        assert_eq!(control.completion, completion);
        let budget = EvalBudget::unbounded();
        let cancelled = fixture
            .retrieve_with_hook(&request, limits, &budget, |window| {
                if window == Window::BeforeRevalidation {
                    budget.cancel();
                }
            })
            .unwrap();
        assert_eq!(
            cancelled.completion,
            Completion::Incomplete(IncompleteReason::BudgetExhausted),
            "cancellation after {completion:?} must remain distinguishable"
        );
        assert!(cancelled.contributions.is_empty());
    }

    let started = Instant::now();
    fixture
        .retrieve(&request, bounds(), &EvalBudget::unbounded())
        .unwrap();
    let attempt = started.elapsed().max(Duration::from_micros(64));
    for step in 0..32u32 {
        let deadline = Instant::now() + attempt.mul_f64(f64::from(step) / 16.0);
        let budget = EvalBudget::new(Some(deadline), Arc::new(AtomicBool::new(false)));
        match fixture.retrieve(&request, bounds(), &budget) {
            Ok(retrieval) => match retrieval.completion {
                Completion::Complete => assert_eq!(retrieval.contributions.len(), 6),
                Completion::Incomplete(IncompleteReason::BudgetExhausted) => {
                    assert!(retrieval.contributions.is_empty());
                }
                other => {
                    panic!("a deadline may only end an attempt as budget exhaustion: {other:?}")
                }
            },
            Err(RetrievalRefusal::BudgetExhausted) => {}
            Err(other) => panic!("a deadline crossing must refuse as budget exhaustion: {other:?}"),
        }
    }
}

#[test]
fn an_engine_interrupt_from_the_connection_ends_the_request_as_budget_exhaustion() {
    let fixture = Fixture::all_admitted();
    let bulk: Vec<Row> = (0..2500)
        .map(|n| Row::claim(&format!("bulk-{n}"), &format!("bulk parse {n}")))
        .collect();
    fixture.project(&bulk);
    let request = probes("parse");
    let wide = RetrievalBounds {
        scan_rows: NonZeroUsize::new(4096).unwrap(),
        batch_rows: NonZeroUsize::new(MAX_ELIGIBILITY_CANDIDATES).unwrap(),
        ..bounds()
    };

    let flag = Arc::new(AtomicBool::new(false));
    let budget = EvalBudget::new(None, Arc::clone(&flag));
    let polls = Arc::new(AtomicUsize::new(0));
    let stop = {
        let flag = Arc::clone(&flag);
        let polls = Arc::clone(&polls);
        move || {
            if polls.fetch_add(1, Ordering::Relaxed) >= 2 {
                flag.store(true, Ordering::Relaxed);
            }
            flag.load(Ordering::Relaxed)
        }
    };
    let interrupted = fixture
        .store
        .with_conn_interruptible(Instant::now() + Duration::from_secs(30), stop, |conn| {
            Ok(retrieve(
                conn,
                &fixture.kernel,
                &request,
                fixture.authority(),
                wide,
                &budget,
            ))
        })
        .unwrap();
    assert!(polls.load(Ordering::Relaxed) >= 3, "the handler polled");
    assert_eq!(interrupted, Err(RetrievalRefusal::BudgetExhausted));

    let polls = Arc::new(AtomicUsize::new(0));
    let counting = {
        let polls = Arc::clone(&polls);
        move || {
            polls.fetch_add(1, Ordering::Relaxed);
            false
        }
    };
    let control = fixture
        .store
        .with_conn_interruptible(Instant::now() + Duration::from_secs(60), counting, |conn| {
            Ok(retrieve(
                conn,
                &fixture.kernel,
                &request,
                fixture.authority(),
                wide,
                &EvalBudget::unbounded(),
            ))
        })
        .unwrap()
        .unwrap();
    assert!(polls.load(Ordering::Relaxed) >= 3, "the handler polled");
    assert_eq!(control.completion, Completion::Complete);
    assert_eq!(control.consumed.scanned_rows, 2504);
    assert_eq!(control.contributions.len(), 4);

    let polls = Arc::new(AtomicUsize::new(0));
    let stop = {
        let polls = Arc::clone(&polls);
        move || polls.fetch_add(1, Ordering::Relaxed) >= 10
    };
    let budget = EvalBudget::unbounded();
    let interrupted = fixture
        .store
        .with_conn_interruptible(Instant::now() + Duration::from_secs(30), stop, |conn| {
            Ok(retrieve(
                conn,
                &fixture.kernel,
                &probes("fetch parse"),
                fixture.authority(),
                RetrievalBounds {
                    scan_rows: NonZeroUsize::new(1).unwrap(),
                    ..bounds()
                },
                &budget,
            ))
        })
        .unwrap()
        .unwrap();
    assert!(!budget.is_exhausted(), "only the SQLite handler stopped");
    assert!(polls.load(Ordering::Relaxed) >= 11);
    // Every probe is counted before any is ranked, so the interrupt lands while parse is counted.
    assert_eq!(
        interrupted.consumed.probes, 1,
        "fetch was counted before parse stopped"
    );
    assert!(interrupted.consumed.counted_rows > 0);
    assert_eq!(interrupted.consumed.scanned_rows, 0);
    assert_eq!(
        interrupted.completion,
        Completion::Incomplete(IncompleteReason::BudgetExhausted)
    );
    assert!(interrupted.contributions.is_empty());
}

#[test]
fn a_held_kernel_reader_does_not_outlive_the_budget() {
    let fixture = Fixture::all_admitted();
    let request = probes("parse");
    let held = std::sync::Barrier::new(2);
    let hold = Duration::from_secs(3);
    let deadline = Duration::from_millis(300);
    let bound = hold / 2;
    let (result, elapsed) = std::thread::scope(|scope| {
        scope.spawn(|| fixture.kernel.hold_readers_for_test(&held, hold));
        held.wait();
        let started = Instant::now();
        let budget = EvalBudget::new(Some(started + deadline), Arc::new(AtomicBool::new(false)));
        (
            fixture.retrieve(&request, bounds(), &budget),
            started.elapsed(),
        )
    });
    assert!(
        elapsed < bound,
        "the request waited for the held kernel reader: {elapsed:?}"
    );
    let retrieval = result.unwrap();
    assert_eq!(
        retrieval.completion,
        Completion::Incomplete(IncompleteReason::BudgetExhausted)
    );
    assert!(retrieval.contributions.is_empty());
    assert_eq!(retrieval.consumed.batches, 0);
    assert_eq!(retrieval.consumed.probes, 1);
}

/// 10,001 rows whose terms match exactly the counts the D26b boundaries name: `t9999`, `t10000`, and `t10001` match that many rows;
/// `c7500` matches 7,500 rows and `d7499`, `d7500`, `d7501` match that many, so a pair sums to 14,999, 15,000, or 15,001.
fn project_thresholds(fixture: &Fixture) -> Vec<Row> {
    let rows: Vec<Row> = (0..10_001)
        .map(|n: usize| {
            let mut terms = vec![format!("row{n}")];
            for (term, count) in [
                ("t9999", 9_999),
                ("t10000", 10_000),
                ("t10001", 10_001),
                ("c7500", 7_500),
                ("d7499", 7_499),
                ("d7500", 7_500),
                ("d7501", 7_501),
            ] {
                if n < count {
                    terms.push(term.to_string());
                }
            }
            Row::claim(&format!("threshold-{n}"), &terms.join(" "))
        })
        .collect();
    for chunk in rows.chunks(4000) {
        fixture.project(chunk);
    }
    rows
}

#[test]
fn ranking_work_is_admitted_by_exact_counts_at_the_d26b_boundaries() {
    assert_eq!((QUALIFYING_MATCHES, RANK_BUDGET), (10_000, 15_000));
    let fixture = Fixture::all_admitted();
    project_thresholds(&fixture);
    let run = |request: &str| {
        fixture
            .retrieve(&probes(request), bounds(), &EvalBudget::unbounded())
            .unwrap()
    };

    // A probe at or under the threshold is counted exactly and ranked; one row past it is common and unranked.
    for (term, count) in [("t9999", 9_999), ("t10000", 10_000)] {
        let ranked = run(term);
        assert_eq!(ranked.consumed.counted_rows, count, "{term}");
        assert_eq!(ranked.consumed.ranked_matches, count, "{term}");
        assert_eq!(
            ranked.completion,
            Completion::Incomplete(IncompleteReason::ScanBound)
        );
    }
    let common = run("t10001");
    assert_eq!(
        common.consumed.counted_rows, 10_001,
        "one lookahead row past the threshold"
    );
    assert_eq!(common.consumed.ranked_matches, 0);
    assert_eq!(
        common.completion,
        Completion::Incomplete(IncompleteReason::CommonTerms)
    );
    assert_eq!(common.consumed.scanned_rows, bounds().scan_rows.get());

    // The cumulative budget admits a pair summing to 14,999 or 15,000 and skips the larger probe at 15,001.
    for (second, count) in [("d7499", 7_499), ("d7500", 7_500)] {
        let both = run(&format!("c7500 {second}"));
        assert_eq!(both.consumed.ranked_matches, 7_500 + count, "{second}");
        assert_eq!(
            both.completion,
            Completion::Incomplete(IncompleteReason::ScanBound)
        );
    }
    let skipped = run("c7500 d7501");
    assert_eq!(skipped.consumed.ranked_matches, 7_500);
    assert_eq!(
        skipped.completion,
        Completion::Incomplete(IncompleteReason::RankBudget)
    );
    // Admission follows increasing count, not request order: the smaller probe is ranked even when it comes second.
    let reordered = run("d7501 c7500");
    assert_eq!(reordered.consumed.ranked_matches, 7_500);
    assert_eq!(
        reordered.completion,
        Completion::Incomplete(IncompleteReason::RankBudget)
    );

    // A repeated probe is counted and ranked once.
    let repeated = run("t9999 t9999");
    assert_eq!(repeated.consumed.counted_rows, 9_999);
    assert_eq!(repeated.consumed.ranked_matches, 9_999);
    assert_eq!(repeated.consumed.probes, 2);
}

#[test]
fn common_probes_are_read_in_descending_rowid_order_and_a_mixed_query_ranks_only_the_qualifying_probe()
 {
    let fixture = Fixture::all_admitted();
    let rows = project_thresholds(&fixture);
    let tight = RetrievalBounds {
        scan_rows: NonZeroUsize::new(5).unwrap(),
        ..bounds()
    };
    let scanned = fixture.kept(&probes("t10001"), tight);
    let mut highest: Vec<String> = fixture
        .raw()
        .prepare("SELECT occurrence_id FROM lexical WHERE lexical MATCH 't10001' ORDER BY rowid DESC LIMIT 5")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    highest.sort();
    assert_eq!(scanned.len(), 5);
    let mut hit: Vec<String> = scanned.clone();
    hit.sort();
    assert_eq!(hit, highest, "the common scan keeps the highest rowids");
    // Admitting the kept rows' sources makes every one of them a contribution, so the assertion below sees them all.
    let objects: Vec<&str> = highest.iter().map(|id| object_of(&rows, id)).collect();
    fixture.admit(&objects);
    let only_common = fixture
        .retrieve(&probes("t10001"), tight, &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(
        only_common.completion,
        Completion::Incomplete(IncompleteReason::CommonTerms)
    );
    assert_eq!(only_common.consumed.ranked_matches, 0);
    assert_eq!(ids_of(&only_common), highest);
    assert!(
        only_common
            .contributions
            .iter()
            .all(|contribution| contribution.rank == 0.0)
    );

    // The rare probe is ranked and the common one skipped; the skip stays visible.
    let mixed = fixture
        .retrieve(&probes("row7 t10001"), tight, &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(mixed.consumed.ranked_matches, 1);
    assert_eq!(
        mixed.completion,
        Completion::Incomplete(IncompleteReason::CommonTerms)
    );
    assert_eq!(mixed.consumed.scanned_rows, 1);
}

/// An interrupt from the connection's progress handler while admission reads a later batch's occurrence rows ends the request as budget exhaustion with no contributions, though the request's own budget never ends.
#[test]
fn an_interrupt_while_admission_reads_occurrence_rows_is_budget_exhaustion() {
    let fixture = Fixture::all_admitted();
    let bulk = project_bulk(&fixture, 600);
    // Every other bulk row is eligible, so the first batch leaves slots open and a second batch reads more occurrence rows.
    let admitted: Vec<&str> = bulk
        .iter()
        .step_by(2)
        .map(|row| row.object.as_str())
        .collect();
    fixture.admit(&admitted);
    let request = probes("parse");
    let wide = RetrievalBounds {
        scan_rows: NonZeroUsize::new(4096).unwrap(),
        max_accepted: NonZeroUsize::new(128).unwrap(),
        batch_rows: NonZeroUsize::new(128).unwrap(),
        ..bounds()
    };
    let interrupt = Arc::new(AtomicBool::new(false));
    let stop = {
        let interrupt = Arc::clone(&interrupt);
        move || interrupt.load(Ordering::Relaxed)
    };
    let budget = EvalBudget::unbounded();
    let interrupted = fixture
        .store
        .with_conn_interruptible(Instant::now() + Duration::from_secs(30), stop, |conn| {
            Ok(retrieve_with_hook_for_test(
                conn,
                &fixture.kernel,
                &request,
                fixture.authority(),
                wide,
                &budget,
                |window| {
                    if window == Window::AfterBatch(1) {
                        interrupt.store(true, Ordering::Relaxed);
                    }
                },
            ))
        })
        .unwrap()
        .unwrap();
    assert!(
        !budget.is_exhausted(),
        "only the connection was interrupted"
    );
    assert_eq!(
        interrupted.completion,
        Completion::Incomplete(IncompleteReason::BudgetExhausted)
    );
    assert!(interrupted.contributions.is_empty());
    assert_eq!(interrupted.consumed.batches, 1);

    let control = fixture
        .retrieve(&request, wide, &EvalBudget::unbounded())
        .unwrap();
    assert!(
        control.consumed.batches > 2,
        "the control reads a second batch"
    );
    assert_eq!(control.contributions.len(), 128);
}

/// Equal-rank matches from two probes that straddle the shared window's cut are kept by occurrence identifier, whichever probe matched them.
#[test]
fn equal_ranks_from_two_probes_across_the_window_cut_keep_the_lowest_identifiers() {
    let fixture = Fixture::all_admitted();
    // The two terms match equally many rows of equal length, so every match of either ranks the same.
    let rows: Vec<Row> = (0..40)
        .map(|n| {
            let term = if n % 2 == 0 { "tiea" } else { "tieb" };
            Row::claim(&format!("straddle-{n}"), &format!("{term} filler"))
        })
        .collect();
    project_admitted(&fixture, &rows);
    let request = probes("tiea tieb");
    let keyed = fixture.keyed_reference(&request);
    assert_eq!(keyed.len(), 40);
    assert!(
        keyed
            .windows(2)
            .all(|pair| pair[0].1.to_bits() == pair[1].1.to_bits()),
        "every match of both probes ties"
    );
    let mut lowest: Vec<String> = rows.iter().map(Row::occurrence_id).collect();
    lowest.sort();
    lowest.truncate(13);
    for term in ["tiea", "tieb"] {
        assert!(
            rows.iter()
                .any(|row| row.text.starts_with(term) && lowest.contains(&row.occurrence_id())),
            "the cut keeps {term} rows"
        );
    }
    let tight = RetrievalBounds {
        scan_rows: NonZeroUsize::new(13).unwrap(),
        ..bounds()
    };
    assert_eq!(fixture.kept(&request, tight), lowest);
    let reversed = probes("tieb tiea");
    assert_eq!(fixture.kept(&reversed, tight), lowest);
    let retrieval = fixture
        .retrieve(&request, tight, &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(ids_of(&retrieval), lowest);
    assert_eq!(
        retrieval.completion,
        Completion::Incomplete(IncompleteReason::ScanBound)
    );
}

/// A repeated common probe is counted and merged once, so it adds no engine work to the common scan.
#[test]
fn a_repeated_common_probe_adds_no_work() {
    let fixture = Fixture::all_admitted();
    project_thresholds(&fixture);
    let tight = RetrievalBounds {
        scan_rows: NonZeroUsize::new(5).unwrap(),
        ..bounds()
    };
    let once = fixture
        .retrieve(&probes("t10001"), tight, &EvalBudget::unbounded())
        .unwrap();
    let twice = fixture
        .retrieve(&probes("t10001 t10001"), tight, &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(twice.consumed.probes, 2);
    assert_eq!(twice.consumed.counted_rows, once.consumed.counted_rows);
    assert_eq!(twice.consumed.sql_steps, once.consumed.sql_steps);
    assert_eq!(ids_of(&twice), ids_of(&once));
    assert!(twice.contributions.iter().all(|c| c.ordinal == 0));
}

/// On the common path an ineligible leader keeps its window slot and is judged and excluded, so it never reaches the result.
#[test]
fn an_ineligible_leader_of_a_common_probe_is_excluded_and_keeps_its_slot() {
    let fixture = Fixture::all_admitted();
    let rows = project_thresholds(&fixture);
    let tight = RetrievalBounds {
        scan_rows: NonZeroUsize::new(5).unwrap(),
        ..bounds()
    };
    let mut kept = fixture.kept(&probes("t10001"), tight);
    assert_eq!(kept.len(), 5);
    kept.sort();
    // Admitting all but the comparator's first kept occurrence leaves that leader without an admission.
    let objects: Vec<&str> = kept[1..].iter().map(|id| object_of(&rows, id)).collect();
    fixture.admit(&objects);
    let retrieval = fixture
        .retrieve(&probes("t10001"), tight, &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(ids_of(&retrieval), kept[1..].to_vec());
    assert_eq!(retrieval.consumed.scanned_rows, 5);
    assert_eq!(
        retrieval.consumed.excluded,
        vec![(EligibilityVerdict::Retracted, 1)]
    );
}

/// A common scan wider than one stream page refills each stream below its last rowid, so the window is exactly the highest rowids across pages and across probes.
#[test]
fn common_streams_refill_across_pages_without_repeating_or_skipping_rowids() {
    let fixture = Fixture::all_admitted();
    project_bulk(&fixture, 1100);
    let wide = RetrievalBounds {
        scan_rows: NonZeroUsize::new(1030).unwrap(),
        ..small_thresholds()
    };
    let raw = fixture.raw();
    let highest = |query: &str| {
        let mut ids: Vec<String> = raw
            .prepare(&format!(
                "SELECT occurrence_id FROM lexical WHERE lexical MATCH '{query}' ORDER BY rowid DESC LIMIT 1030"
            ))
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        ids.sort();
        ids
    };
    for (request, matched) in [("parse", "parse"), ("parse bulk", "parse OR bulk")] {
        let mut kept = fixture.kept(&probes(request), wide);
        kept.sort();
        assert_eq!(kept, highest(matched), "{request}");
        let retrieval = fixture
            .retrieve(&probes(request), wide, &EvalBudget::unbounded())
            .unwrap();
        assert_eq!(retrieval.consumed.scanned_rows, 1030, "{request}");
        assert!(retrieval.reasons.contains(&IncompleteReason::ScanBound));
    }
}

/// Each sizing rule decides a batch: a later batch with nothing accepted takes a whole batch, and a fractional ratio rounds up.
#[test]
fn admission_batch_sizes_follow_each_sizing_rule() {
    let fixture = Fixture::all_admitted();
    let bulk = project_bulk(&fixture, 12);
    let request = probes("bulk");
    let order = fixture.reference(&request);
    assert_eq!(order.len(), 12);
    // Leaves the first `hidden` comparator rows unadmitted and admits the rest.
    let admit_after = |fixture: &Fixture, hidden: usize| {
        let objects: Vec<&str> = order[hidden..]
            .iter()
            .map(|id| object_of(&bulk, id))
            .collect();
        fixture.admit(&objects);
    };
    let run = |fixture: &Fixture, max_accepted: usize, batch_rows: usize| {
        fixture
            .retrieve(
                &request,
                RetrievalBounds {
                    max_accepted: NonZeroUsize::new(max_accepted).unwrap(),
                    batch_rows: NonZeroUsize::new(batch_rows).unwrap(),
                    ..bounds()
                },
                &EvalBudget::unbounded(),
            )
            .unwrap()
    };

    // Two open slots read two ineligible rows; with nothing accepted the next batch takes all eight rows, then revalidation re-judges two.
    admit_after(&fixture, 2);
    let empty_first = run(&fixture, 2, 8);
    assert_eq!(empty_first.consumed.judged, 2 + 8 + 2);
    assert_eq!(empty_first.consumed.batches, 3);
    assert_eq!(ids_of(&empty_first), order[2..4].to_vec());

    // A fresh fixture with one ineligible leader: three open slots accept two of three reads, so one open slot becomes ceil(1 * 3 / 2) = 2 rows.
    let fixture = Fixture::all_admitted();
    project_bulk(&fixture, 12);
    assert_eq!(fixture.reference(&request), order);
    admit_after(&fixture, 1);
    let fractional = run(&fixture, 3, 8);
    assert_eq!(fractional.consumed.judged, 3 + 2 + 3);
    assert_eq!(fractional.consumed.batches, 3);
    assert_eq!(ids_of(&fractional), order[1..4].to_vec());
    assert!(
        fractional
            .reasons
            .contains(&IncompleteReason::AcceptedBound)
    );
}

/// After an eligible prefix leaves one accepted slot open, each batch that accepts nothing doubles the next batch up to `batch_rows`, so an ineligible tail is judged in seven admission batches.
#[test]
fn a_dry_batch_after_an_eligible_prefix_doubles_the_next_batch() {
    let fixture = Fixture::all_admitted();
    let bulk = project_bulk(&fixture, 64);
    let request = probes("bulk");
    let order = fixture.reference(&request);
    assert_eq!(order.len(), 64);
    let prefix: Vec<&str> = order[..15].iter().map(|id| object_of(&bulk, id)).collect();
    fixture.admit(&prefix);

    let retrieval = fixture
        .retrieve(
            &request,
            RetrievalBounds {
                max_accepted: NonZeroUsize::new(16).unwrap(),
                batch_rows: NonZeroUsize::new(16).unwrap(),
                ..bounds()
            },
            &EvalBudget::unbounded(),
        )
        .unwrap();
    assert_eq!(ids_of(&retrieval), order[..15].to_vec());
    assert_eq!(retrieval.completion, Completion::Complete);
    // Sixteen open slots accept fifteen; one slot left takes ceil(1 * 16 / 15) = 2 rows, and each batch that accepts nothing doubles the next: 4, 8, then 16, 16, and the last 2 rows. Revalidation re-judges the fifteen.
    assert_eq!(retrieval.consumed.judged, 64 + 15);
    assert_eq!(retrieval.consumed.batches, 7 + 1);
    assert_eq!(
        retrieval.consumed.excluded,
        vec![(EligibilityVerdict::Retracted, 64 - 15)]
    );
}

/// Projects `rows` and admits their sources, so every one of their occurrences is an eligible contribution.
fn project_admitted(fixture: &Fixture, rows: &[Row]) {
    fixture.project(rows);
    let objects: Vec<&str> = rows.iter().map(|row| row.object.as_str()).collect();
    fixture.admit(&objects);
}

/// The scan bound is one window for the whole request: ranked probes merge in comparator order before any slot is taken, so the request keeps the best `scan_rows` occurrences across its probes rather than `scan_rows` per probe.
#[test]
fn ranked_probes_share_one_scan_window_in_comparator_order() {
    let fixture = Fixture::all_admitted();
    // Filler lengths spread the ranks, and the two terms interleave across them.
    let rows: Vec<Row> = (0..12)
        .map(|n| {
            let term = if n % 2 == 0 { "aa" } else { "bb" };
            Row::claim(
                &format!("window-{n}"),
                &format!("{term}{}", " pad".repeat(n)),
            )
        })
        .collect();
    project_admitted(&fixture, &rows);
    let request = probes("aa bb");
    let reference = fixture.reference(&request);
    assert_eq!(reference.len(), 12);
    let tight = RetrievalBounds {
        scan_rows: NonZeroUsize::new(4).unwrap(),
        ..bounds()
    };
    let retrieval = fixture
        .retrieve(&request, tight, &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(ids_of(&retrieval), reference[..4].to_vec());
    assert_eq!(retrieval.consumed.scanned_rows, 4);
    assert_eq!(retrieval.consumed.ranked_matches, 12);
    assert_eq!(
        retrieval.completion,
        Completion::Incomplete(IncompleteReason::ScanBound)
    );
    // Either probe's best rows alone would differ from the shared window's.
    for single in ["aa", "bb"] {
        let alone = fixture
            .retrieve(&probes(single), tight, &EvalBudget::unbounded())
            .unwrap();
        assert_ne!(ids_of(&alone), ids_of(&retrieval), "{single}");
    }
    let whole = fixture
        .retrieve(&request, bounds(), &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(ids_of(&whole), reference);
    assert_eq!(whole.completion, Completion::Complete);
}

/// Common probes merge their rowids in descending order into one window; a row both probes match takes one slot, under the lower probe ordinal.
#[test]
fn common_probes_share_one_descending_rowid_window() {
    let fixture = Fixture::all_admitted();
    let rows: Vec<Row> = (0..10)
        .map(|n| {
            let text = match n % 3 {
                0 => "cc",
                1 => "dd",
                _ => "cc dd",
            };
            Row::claim(&format!("common-{n}"), text)
        })
        .collect();
    project_admitted(&fixture, &rows);
    let tight = RetrievalBounds {
        scan_rows: NonZeroUsize::new(4).unwrap(),
        qualifying_matches: NonZeroUsize::new(2).unwrap(),
        rank_budget: NonZeroUsize::new(2).unwrap(),
        ..bounds()
    };
    let raw = fixture.raw();
    let mut expected: Vec<String> = raw
        .prepare("SELECT occurrence_id FROM lexical WHERE lexical MATCH 'cc OR dd' ORDER BY rowid DESC LIMIT 4")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    expected.sort();
    let both: Vec<String> = raw
        .prepare("SELECT occurrence_id FROM lexical WHERE lexical MATCH 'cc AND dd'")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(
        expected.iter().any(|id| both.contains(id)),
        "the window holds a row both probes match"
    );
    let retrieval = fixture
        .retrieve(&probes("cc dd"), tight, &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(ids_of(&retrieval), expected);
    assert_eq!(retrieval.consumed.scanned_rows, 4);
    assert_eq!(retrieval.consumed.ranked_matches, 0);
    assert_eq!(
        retrieval.reasons,
        vec![IncompleteReason::CommonTerms, IncompleteReason::ScanBound]
    );
    for contribution in &retrieval.contributions {
        let shared = both.contains(&contribution.occurrence_id);
        let only_dd = rows
            .iter()
            .any(|row| row.text == "dd" && row.occurrence_id() == contribution.occurrence_id);
        let expected_ordinal = if shared || !only_dd { 0 } else { 1 };
        assert_eq!(contribution.ordinal, expected_ordinal);
        assert_eq!(contribution.rank, 0.0);
    }
}

/// The lexical lane's production retrieval bounds. The daemon's `production_limits_are_the_d23_set` pins
/// `QueryRouteLimits::production().lexical_retrieval_bounds()` to these same values.
fn production_bounds() -> RetrievalBounds {
    let n = |value: usize| NonZeroUsize::new(value).unwrap();
    RetrievalBounds {
        max_probes: n(16),
        scan_rows: n(4096),
        max_accepted: n(128),
        batch_rows: n(128),
        qualifying_matches: n(QUALIFYING_MATCHES),
        rank_budget: n(RANK_BUDGET),
    }
}

/// A projection of `occurrences` claims, every one whose index is not a multiple of 16 admitted.
fn build_scale_fixture(occurrences: usize) -> Fixture {
    let fixture = Fixture::all_admitted();
    let built = Instant::now();
    let mut chunk = Vec::with_capacity(1000);
    let flush = |chunk: &mut Vec<Row>| {
        fixture.project(chunk);
        let admitted: Vec<&str> = chunk
            .iter()
            .filter(|row| {
                row.object
                    .strip_prefix("scale-")
                    .and_then(|n| n.parse::<usize>().ok())
                    .is_some_and(|n| n % 16 != 0)
            })
            .map(|row| row.object.as_str())
            .collect();
        fixture.admit(&admitted);
        chunk.clear();
    };
    for n in 0..occurrences {
        // Filler of 0 to 96 tokens spreads the ranks, and at one million occurrences each `k*` term matches exactly 10,000 rows, the largest qualifying probe.
        let text = format!(
            "w{} m{} k{} c{} common{}",
            n % 50_000,
            n % 500,
            n % 100,
            n % 5,
            " filler".repeat(n % 97)
        );
        chunk.push(Row::claim(&format!("scale-{n}"), &text));
        if chunk.len() == 1000 {
            flush(&mut chunk);
        }
    }
    flush(&mut chunk);
    eprintln!(
        "projected and admitted {occurrences} occurrences in {:?}",
        built.elapsed()
    );
    fixture
}

/// D26b lexical gate at one million occurrences under the production lexical bounds: an open-loop Poisson schedule of
/// rare, qualifying, mixed, budget-skipping, and common-only queries, each timed from its scheduled arrival to the end of its scan and
/// admission. Every occurrence whose index is not a multiple of 16 is admitted, so admission judges, accepts, and
/// revalidates as it does for a mostly eligible corpus. Run in release on the D21 host with `--ignored`;
/// `EIDNARA_LEXICAL_SCALE_SAMPLES` names a file for the raw microsecond samples, kept outside the repository.
#[test]
#[ignore = "builds a 1M-occurrence projection; run in release on the D21 host with --ignored"]
fn lexical_scan_p99_at_one_million_occurrences() {
    fn setting<T: std::str::FromStr>(name: &str) -> Option<T> {
        std::env::var(name)
            .ok()
            .and_then(|value| value.parse().ok())
    }
    let occurrences: usize = setting("EIDNARA_LEXICAL_SCALE_OCCURRENCES").unwrap_or(1_000_000);
    let queries: usize = setting("EIDNARA_LEXICAL_SCALE_QUERIES").unwrap_or(600);
    let rate_per_second: f64 = setting("EIDNARA_LEXICAL_SCALE_RATE").unwrap_or(10.0);
    let first_after_build = queries.min(50);
    assert!(
        queries > first_after_build,
        "the warm window needs queries past the first {first_after_build}"
    );
    // `EIDNARA_LEXICAL_SCALE_ROOT` names a projection an earlier run kept; the run reuses it instead of building one.
    let fixture = match std::env::var_os("EIDNARA_LEXICAL_SCALE_ROOT") {
        Some(root) => Fixture::reopen(Root::Kept(root.into())),
        None => build_scale_fixture(occurrences),
    };
    // A reused projection reports the occurrences it holds, not the requested count.
    let occurrences: i64 = fixture
        .raw()
        .query_row("SELECT count(*) FROM occurrences", [], |row| row.get(0))
        .unwrap();
    let mix = [
        "w123",
        "m42",
        "w7 c3",
        "common",
        "c1 c2",
        "c1 c2 c3 c4",
        "m1 m2",
        "w4242 m17",
        "k3",
        "k3 m5",
        "k3 k4",
    ];
    // A fixed linear congruential stream keeps the schedule reproducible.
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut uniform = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((state >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    };
    let mut at = 0.0f64;
    let schedule: Vec<(f64, &str)> = (0..queries)
        .map(|index| {
            at += -uniform().ln() / rate_per_second;
            (at, mix[index % mix.len()])
        })
        .collect();
    // Each request runs the lexical lane's scan and admission with a deadline taken at its scheduled arrival, as the route does.
    struct Sample {
        scheduled_us: u64,
        sent_us: u64,
        done_us: u64,
        request: &'static str,
        outcome: String,
        censored: bool,
        counted: usize,
        ranked: usize,
        contributions: usize,
    }
    let start = Instant::now();
    let micros =
        |at: Instant| u64::try_from(at.duration_since(start).as_micros()).unwrap_or(u64::MAX);
    let mut samples: Vec<Sample> = Vec::with_capacity(queries);
    for (scheduled, request) in &schedule {
        let due = start + Duration::from_secs_f64(*scheduled);
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        let sent = Instant::now();
        let budget = EvalBudget::new(
            Some(due + Duration::from_secs(5)),
            Arc::new(AtomicBool::new(false)),
        );
        let outcome = fixture.retrieve(&probes(request), production_bounds(), &budget);
        let done = Instant::now();
        let (outcome, censored, counted, ranked, contributions) = match outcome {
            Ok(retrieval)
                if retrieval.completion
                    == Completion::Incomplete(IncompleteReason::BudgetExhausted) =>
            {
                ("budget exhausted".to_string(), true, 0, 0, 0)
            }
            Ok(retrieval) => (
                format!("{:?}", retrieval.reasons),
                false,
                retrieval.consumed.counted_rows,
                retrieval.consumed.ranked_matches,
                retrieval.contributions.len(),
            ),
            Err(refusal) => (refusal.to_string(), true, 0, 0, 0),
        };
        samples.push(Sample {
            scheduled_us: micros(due),
            sent_us: micros(sent),
            done_us: micros(done),
            request,
            outcome,
            censored,
            counted,
            ranked,
            contributions,
        });
    }
    let completed = samples.iter().filter(|sample| !sample.censored).count();
    let latency = |range: std::ops::Range<usize>| {
        let mut waits: Vec<u64> = samples[range]
            .iter()
            .map(|sample| sample.done_us.saturating_sub(sample.scheduled_us))
            .collect();
        waits.sort_unstable();
        let p = |q: f64| waits[((waits.len() as f64 * q).ceil() as usize).saturating_sub(1)];
        (p(0.5), p(0.99), *waits.last().unwrap())
    };
    // The page cache holds the projection the build just wrote, so the first queries are first-after-build, not cold-cache.
    let first = latency(0..first_after_build);
    let warm = latency(first_after_build..queries);
    let most_ranked = samples
        .iter()
        .map(|sample| sample.ranked)
        .max()
        .unwrap_or(0);
    eprintln!(
        "rate {rate_per_second}/s; offered {queries} sent {} completed {completed}; most ranked matches {most_ranked}; first-after-build p50/p99/max us {first:?}; warm p50/p99/max us {warm:?}",
        samples.len()
    );
    if let Some(path) = std::env::var_os("EIDNARA_LEXICAL_SCALE_SAMPLES") {
        let mut rows = vec![format!(
            "{{\"rate_per_second\":{rate_per_second},\"seed\":\"0x9e3779b97f4a7c15\",\"occurrences\":{occurrences},\"offered\":{queries},\"completed\":{completed}}}"
        )];
        rows.extend(samples.iter().map(|sample| {
            format!(
                "{{\"scheduled_us\":{},\"sent_us\":{},\"done_us\":{},\"query\":{:?},\"outcome\":{:?},\"censored\":{},\"counted_rows\":{},\"ranked_matches\":{},\"contributions\":{}}}",
                sample.scheduled_us,
                sample.sent_us,
                sample.done_us,
                sample.request,
                sample.outcome,
                sample.censored,
                sample.counted,
                sample.ranked,
                sample.contributions
            )
        }));
        std::fs::write(path, rows.join("\n")).unwrap();
    }
    if std::env::var_os("EIDNARA_LEXICAL_SCALE_KEEP").is_some() {
        eprintln!("kept projection under {}", fixture.root.path().display());
        std::mem::forget(fixture);
    }
    // A refused, exhausted, or empty answer is not a latency sample the gate may pass on.
    assert_eq!(completed, queries, "every offered query must complete");
    // The mix exercises the ranked regime it names: the largest qualifying probe, a budget skip, and no request past the budget.
    for sample in &samples {
        assert!(sample.ranked <= RANK_BUDGET, "{}", sample.request);
        match sample.request {
            "k3" => assert_eq!(
                sample.ranked, QUALIFYING_MATCHES,
                "k3 matches exactly the qualifying threshold only at one million occurrences"
            ),
            "k3 k4" => assert!(
                sample.outcome.contains("RankBudget"),
                "k3 k4 passes the ranked budget only at one million occurrences: {}",
                sample.outcome
            ),
            _ => {}
        }
    }
    for sample in &samples {
        assert!(
            sample.contributions > 0,
            "{} returned no contribution: {}",
            sample.request,
            sample.outcome
        );
    }
    assert!(
        warm.1 <= 50_000,
        "warm lexical p99 {}us exceeds 50ms",
        warm.1
    );
}

#[test]
fn a_probe_with_no_match_leaves_the_common_probe_to_its_bounded_scan() {
    let fixture = Fixture::all_admitted();
    project_thresholds(&fixture);
    let tight = RetrievalBounds {
        scan_rows: NonZeroUsize::new(5).unwrap(),
        ..bounds()
    };
    let retrieval = fixture
        .retrieve(
            &probes("absentterm t10001"),
            tight,
            &EvalBudget::unbounded(),
        )
        .unwrap();
    assert_eq!(retrieval.consumed.ranked_matches, 0);
    assert_eq!(
        retrieval.consumed.scanned_rows, 5,
        "the common probe was scanned"
    );
    assert_eq!(
        retrieval.completion,
        Completion::Incomplete(IncompleteReason::CommonTerms)
    );
}

/// 3,000 rows of identical rank cross a 64-row bound; the kept rows are exactly the lowest occurrence identifiers, whatever order the rows were stored in.
#[test]
fn a_large_equal_rank_group_at_the_bound_keeps_the_lowest_identifiers() {
    let fixture = Fixture::all_admitted();
    let mut tied: Vec<Row> = (0..3000)
        .map(|n| Row::claim(&format!("tie-{n}"), &format!("tiegroup filler {n}")))
        .collect();
    tied.reverse();
    for chunk in tied.chunks(1000) {
        fixture.project(chunk);
    }
    let request = probes("tiegroup");
    let mut expected: Vec<String> = tied.iter().map(Row::occurrence_id).collect();
    expected.sort();
    expected.truncate(64);
    let scanned = fixture.kept(&request, bounds());
    assert_eq!(scanned.len(), 64);
    assert_eq!(scanned, expected);
    let reference = fixture.reference(&request);
    assert_eq!(
        &reference[..64],
        expected.as_slice(),
        "the oracle agrees the ranks tie"
    );
    let retrieval = fixture
        .retrieve(&request, bounds(), &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(
        retrieval.completion,
        Completion::Incomplete(IncompleteReason::ScanBound)
    );
    assert_eq!(retrieval.consumed.ranked_matches, 3000);
}

/// Tie-breaking by occurrence identifier selects the same 64 rows whether the tie group fills the table or shares it with 3,000 unrelated rows.
#[test]
fn a_tie_group_spread_thinly_through_the_table_keeps_the_lowest_identifiers() {
    let fixture = Fixture::all_admitted();
    let mut rows: Vec<Row> = (0..3000)
        .map(|n| Row::claim(&format!("gap-{n}"), &format!("unrelated filler {n}")))
        .collect();
    let tied: Vec<Row> = (0..240)
        .map(|n| Row::claim(&format!("tie-{n}"), &format!("tiegroup filler {n}")))
        .collect();
    rows.extend(tied.iter().cloned());
    for chunk in rows.chunks(1000) {
        fixture.project(chunk);
    }
    let request = probes("tiegroup");
    let mut expected: Vec<String> = tied.iter().map(Row::occurrence_id).collect();
    expected.sort();
    let whole = fixture.kept(
        &request,
        RetrievalBounds {
            scan_rows: NonZeroUsize::new(240).unwrap(),
            ..bounds()
        },
    );
    assert_eq!(whole, expected, "every tied row is identified");
    expected.truncate(64);
    let scanned = fixture.kept(&request, bounds());
    assert_eq!(scanned, expected);
    let retrieval = fixture
        .retrieve(&request, bounds(), &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(
        retrieval.completion,
        Completion::Incomplete(IncompleteReason::ScanBound)
    );
    assert_eq!(retrieval.consumed.ranked_matches, 240);
    assert_eq!(retrieval.consumed.scanned_rows, 64);
}

#[test]
fn dead_rows_in_a_large_tie_group_keep_their_window_slots() {
    let fixture = Fixture::all_admitted();
    let mut tied: Vec<Row> = (0..3000)
        .map(|n| Row::claim(&format!("tie-{n}"), &format!("tiegroup filler {n}")))
        .collect();
    tied.reverse();
    for chunk in tied.chunks(1000) {
        fixture.project(chunk);
    }
    let mut ids: Vec<String> = tied.iter().map(Row::occurrence_id).collect();
    ids.sort();
    let tombstone = |dead: &[String]| {
        let mut raw = fixture.raw();
        let tx = raw.transaction().unwrap();
        for occurrence_id in dead {
            tx.execute(
                "INSERT INTO occurrence_tombstones(occurrence_id, invalidated_commit_seq, reason, recorded_at) VALUES (?1, 99, 'retired', 0)",
                [occurrence_id],
            )
            .unwrap();
        }
        tx.commit().unwrap();
    };
    let request = probes("tiegroup");
    let scan_rows = bounds().scan_rows.get();
    let scanned = |fixture: &Fixture| fixture.kept(&request, bounds());

    // Dead rows at the front of the group keep their slots: the window is the group's lowest identifiers whatever their liveness.
    let dead = 7;
    tombstone(&ids[..dead]);
    let kept = scanned(&fixture);
    assert_eq!(kept, ids[..scan_rows].to_vec());
    let retrieval = fixture
        .retrieve(&request, bounds(), &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(retrieval.consumed.scanned_rows, scan_rows);
    assert_eq!(retrieval.consumed.dead_rows, dead);
    assert!(retrieval.reasons.contains(&IncompleteReason::ScanBound));

    // Rows past the window take no slot a dead row leaves, so a window of dead rows returns nothing and still reports truncation.
    tombstone(&ids[dead..scan_rows]);
    let retrieval = fixture
        .retrieve(&request, bounds(), &EvalBudget::unbounded())
        .unwrap();
    assert!(retrieval.contributions.is_empty());
    assert_eq!(retrieval.consumed.dead_rows, scan_rows);
    assert_eq!(retrieval.consumed.judged, 0);
    assert!(retrieval.reasons.contains(&IncompleteReason::ScanBound));
}

/// An interrupt from the connection's progress handler at any point of counting, ranking, or a common scan ends the request as
/// budget exhaustion with no contributions, and the connection serves the next request normally.
#[test]
fn an_interrupt_anywhere_in_counting_ranking_or_a_common_scan_is_budget_exhaustion() {
    let fixture = Fixture::all_admitted();
    project_thresholds(&fixture);
    let tight = RetrievalBounds {
        scan_rows: NonZeroUsize::new(5).unwrap(),
        ..bounds()
    };
    let run_until = |request: &str, tight: RetrievalBounds, stop_after: usize| {
        let flag = Arc::new(AtomicBool::new(false));
        let budget = EvalBudget::new(None, Arc::clone(&flag));
        let polls = Arc::new(AtomicUsize::new(0));
        let stop = {
            let flag = Arc::clone(&flag);
            let polls = Arc::clone(&polls);
            move || {
                if polls.fetch_add(1, Ordering::Relaxed) >= stop_after {
                    flag.store(true, Ordering::Relaxed);
                }
                flag.load(Ordering::Relaxed)
            }
        };
        let outcome = fixture
            .store
            .with_conn_interruptible(Instant::now() + Duration::from_secs(60), stop, |conn| {
                Ok(retrieve(
                    conn,
                    &fixture.kernel,
                    &probes(request),
                    fixture.authority(),
                    tight,
                    &budget,
                ))
            })
            // An interrupt outside the request's statements ends the connection's interval itself as its deadline.
            .unwrap_or_else(|error| {
                assert!(format!("{error:?}").contains("Deadline"), "{error:?}");
                Err(RetrievalRefusal::BudgetExhausted)
            });
        (outcome, polls.load(Ordering::Relaxed))
    };
    // A wide scan bound makes the common scan, not its count, most of the `t10001` work at 4,096 rows.
    let wide = RetrievalBounds {
        scan_rows: NonZeroUsize::new(4096).unwrap(),
        max_accepted: NonZeroUsize::new(MAX_ELIGIBILITY_CANDIDATES).unwrap(),
        ..bounds()
    };
    for (request, tight) in [
        ("t10001", tight),
        ("t10001", wide),
        ("t10000", tight),
        ("c7500 d7500", tight),
    ] {
        let (complete, total) = run_until(request, tight, usize::MAX);
        assert!(!matches!(
            complete.unwrap().completion,
            Completion::Incomplete(IncompleteReason::BudgetExhausted)
        ));
        assert!(total > 8, "{request} polled {total} times");
        for step in 0..8 {
            let (outcome, _) = run_until(request, tight, total * step / 8);
            match outcome {
                Err(RetrievalRefusal::BudgetExhausted) => {}
                Ok(retrieval) => {
                    assert_eq!(
                        retrieval.completion,
                        Completion::Incomplete(IncompleteReason::BudgetExhausted),
                        "{request} at poll {}",
                        total * step / 8
                    );
                    assert!(retrieval.contributions.is_empty());
                }
                Err(other) => panic!("{request}: {other:?}"),
            }
        }
        // The progress handler was removed, so the connection serves an unbudgeted request.
        let after = fixture
            .retrieve(&probes(request), tight, &EvalBudget::unbounded())
            .unwrap();
        assert!(!matches!(
            after.completion,
            Completion::Incomplete(IncompleteReason::BudgetExhausted)
        ));
    }
}

/// Bounds under which the fixture corpus's `parse` (four matches) is common, while `fetch` (two) and `io` (one) qualify
/// and only one match fits the ranked budget.
fn small_thresholds() -> RetrievalBounds {
    RetrievalBounds {
        qualifying_matches: NonZeroUsize::new(2).unwrap(),
        rank_budget: NonZeroUsize::new(2).unwrap(),
        ..bounds()
    }
}

#[test]
fn a_mixed_query_records_both_its_common_skip_and_its_budget_skip() {
    let fixture = Fixture::all_admitted();
    for (term, count) in [("parse", 4), ("fetch", 2), ("io", 1)] {
        assert_eq!(fixture.reference(&probes(term)).len(), count, "{term}");
    }
    let retrieval = fixture
        .retrieve(
            &probes("parse fetch io"),
            small_thresholds(),
            &EvalBudget::unbounded(),
        )
        .unwrap();
    assert_eq!(retrieval.consumed.ranked_matches, 1);
    assert_eq!(ids_of(&retrieval), vec![fixture.id("delta")]);
    assert_eq!(
        retrieval.completion,
        Completion::Incomplete(IncompleteReason::CommonTerms)
    );
    assert_eq!(
        retrieval.reasons,
        vec![IncompleteReason::CommonTerms, IncompleteReason::RankBudget]
    );
}

#[test]
fn an_authority_move_outranks_a_coverage_bound_already_recorded() {
    let fixture = Fixture::all_admitted();
    let retrieval = fixture
        .retrieve_with_hook(
            &probes("parse"),
            small_thresholds(),
            &EvalBudget::unbounded(),
            |window| {
                if window == Window::BeforeRevalidation {
                    fixture.retire("gamma");
                }
            },
        )
        .unwrap();
    assert_eq!(
        retrieval.completion,
        Completion::Incomplete(IncompleteReason::SnapshotChanged),
        "the same move leaves a complete request `SnapshotChanged`"
    );
    assert_eq!(
        retrieval.reasons,
        vec![
            IncompleteReason::CommonTerms,
            IncompleteReason::SnapshotChanged
        ]
    );
}

#[test]
fn dead_rows_leading_a_common_probe_keep_their_window_slots() {
    let fixture = Fixture::all_admitted();
    project_bulk(&fixture, 20);
    let scan_rows = 5;
    let common = RetrievalBounds {
        scan_rows: NonZeroUsize::new(scan_rows).unwrap(),
        ..small_thresholds()
    };
    let descending: Vec<String> = fixture
        .raw()
        .prepare(
            "SELECT occurrence_id FROM lexical WHERE lexical MATCH 'parse' ORDER BY rowid DESC",
        )
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(descending.len(), 24);
    // Dead rows lead the descending order; the window keeps them, and admission drops them without reading further rows.
    let dead = 2;
    for occurrence_id in &descending[..dead] {
        tombstone_raw(&fixture, occurrence_id);
    }
    let scanned = fixture.kept(&probes("parse"), common);
    let mut kept: Vec<String> = scanned.clone();
    kept.sort();
    let mut expected = descending[..scan_rows].to_vec();
    expected.sort();
    assert_eq!(kept, expected);

    let retrieval = fixture
        .retrieve(&probes("parse"), common, &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(retrieval.consumed.scanned_rows, scan_rows);
    assert_eq!(retrieval.consumed.dead_rows, dead);
    assert!(
        ids_of(&retrieval)
            .iter()
            .all(|id| descending[dead..scan_rows].contains(id))
    );
    assert_eq!(
        retrieval.reasons,
        vec![IncompleteReason::CommonTerms, IncompleteReason::ScanBound]
    );
}

/// Equal-byte occurrences of distinct objects stay distinct contributions: they tie on raw rank, the occurrence comparator orders them, and with one-row batches the eligible siblings fill every accepted slot past an ineligible leader. Duplicate and permuted probes leave the result unchanged.
#[test]
fn equal_byte_siblings_stay_distinct_and_fill_past_an_ineligible_leader() {
    let fixture = Fixture::all_admitted();
    let objects = ["twin-a", "twin-b", "twin-c"];
    let mut siblings: Vec<Row> = objects
        .iter()
        .map(|object| Row::claim(object, "sibling twin text"))
        .collect();
    siblings.sort_by_key(Row::occurrence_id);
    // The comparator's first sibling stays an unadmitted decision, so the kernel hides it.
    let leader = siblings[0].object.clone();
    fixture
        .kernel
        .commit(intent("twin-leader"), |envelope| {
            envelope.insert_decision(decision(&leader))?;
            Ok(String::new())
        })
        .unwrap();
    let admitted: Vec<&str> = siblings[1..]
        .iter()
        .map(|row| row.object.as_str())
        .collect();
    fixture.admit(&admitted);
    fixture.project(&siblings);

    let two_one_row_batches = RetrievalBounds {
        max_accepted: NonZeroUsize::new(2).unwrap(),
        batch_rows: NonZeroUsize::new(1).unwrap(),
        ..bounds()
    };
    let expected: Vec<String> = siblings[1..].iter().map(Row::occurrence_id).collect();
    for request in ["sibling", "sibling sibling", "twin sibling", "sibling twin"] {
        let retrieval = fixture
            .retrieve(
                &probes(request),
                two_one_row_batches,
                &EvalBudget::unbounded(),
            )
            .unwrap();
        assert_eq!(ids_of(&retrieval), expected, "{request}");
        assert_eq!(retrieval.completion, Completion::Complete, "{request}");
        assert_eq!(
            retrieval.consumed.excluded,
            vec![(EligibilityVerdict::Hidden, 1)],
            "{request}"
        );
        let ranks: Vec<f64> = keyed(&retrieval)
            .into_iter()
            .map(|(_, rank)| rank)
            .collect();
        assert_eq!(
            ranks[0].to_bits(),
            ranks[1].to_bits(),
            "{request}: equal bytes tie"
        );
    }
}

/// Allocations follow the work the bounds admit. A scan bound one row lower allocates less. A common-term scan stops counting at its qualifying threshold and scanning at its scan bound, so matching rows past both add no allocation. A ranked probe scores every match within `rank_budget`, so its allocations grow with `ranked_matches`. The recorder sees Rust allocations on the calling thread; SQLite's own allocator stays outside it.
#[test]
fn allocations_follow_the_scan_bound_and_the_rank_budget() {
    let fixture = Fixture::all_admitted();
    let request = probes("parse parse");
    let measure = |bounds: RetrievalBounds| {
        // The first run prepares and caches the statements, so the recorded run measures steady-state work.
        fixture.retrieve_recorded(&request, bounds);
        let (retrieval, ledger) = fixture.retrieve_recorded(&request, bounds);
        assert!(!ledger.overflow);
        (retrieval, ledger)
    };
    let scan = |scan_rows: usize, thresholds: RetrievalBounds| RetrievalBounds {
        scan_rows: NonZeroUsize::new(scan_rows).unwrap(),
        ..thresholds
    };
    let (exact, at_bound) = measure(scan(4, bounds()));
    assert_eq!(exact.consumed.scanned_rows, 4);
    let (below, under) = measure(scan(3, bounds()));
    assert_eq!(below.consumed.scanned_rows, 3);
    assert!(
        under.allocation_events < at_bound.allocation_events
            && under.requested_bytes < at_bound.requested_bytes,
        "{under:?} against {at_bound:?}"
    );
    let (common, common_before) = measure(scan(3, small_thresholds()));
    assert_eq!(
        common.completion,
        Completion::Incomplete(IncompleteReason::CommonTerms)
    );

    let extra: Vec<String> = (0..32)
        .map(|index| format!("parse-extra-{index}"))
        .collect();
    let extra_refs: Vec<&str> = extra.iter().map(String::as_str).collect();
    fixture.admit(&extra_refs);
    let rows: Vec<Row> = extra
        .iter()
        .map(|object| Row::claim(object, "parse past the bound"))
        .collect();
    fixture.project(&rows);

    let (common_grown, common_after) = measure(scan(3, small_thresholds()));
    // The common scan reads newest rows first, so it now scans three of the new rows; their text sets the requested bytes, and the count of allocations stays fixed.
    assert_eq!(
        (
            common_grown.consumed.counted_rows,
            common_grown.consumed.scanned_rows,
        ),
        (common.consumed.counted_rows, common.consumed.scanned_rows),
    );
    assert_eq!(
        common_after.allocation_events, common_before.allocation_events,
        "a common scan stops counting at its threshold and scanning at its bound, so matches past both allocate nothing"
    );
    let (common_tighter, _) = measure(scan(2, small_thresholds()));
    assert_eq!(common_tighter.consumed.scanned_rows, 2);
    assert!(
        common_tighter.consumed.sql_steps < common_grown.consumed.sql_steps
            && common_grown.consumed.sql_steps
                < common.consumed.sql_steps + common_tighter.consumed.sql_steps,
        "the grown table's common scan follows its scan bound: {} steps at two rows, {} at three, {} at three before growth",
        common_tighter.consumed.sql_steps,
        common_grown.consumed.sql_steps,
        common.consumed.sql_steps
    );
    let (ranked, ranked_after) = measure(scan(3, bounds()));
    assert_eq!(ranked.consumed.ranked_matches, 36);
    assert_eq!(ranked.consumed.scanned_rows, 3);
    assert!(
        ranked_after.allocation_events > under.allocation_events,
        "ranked scoring allocates for each match within the rank budget: {ranked_after:?} against {under:?}"
    );
}

/// Every counter reports the work the request did at the scan bound and one row below it: probes compiled, rows counted and scanned, kernel judgments in their batches, and the SQL steps the count and scan statements ran. Below the match count, a smaller scan bound runs fewer steps on the same counted rows, a result cap keeps the scan bound and reads fewer occurrence rows, and one more matching row adds SQL steps on a ranked run and on a common run.
#[test]
fn work_counters_report_exact_and_over_bound_work() {
    let fixture = Fixture::all_admitted();
    let request = probes("parse parse");
    let run = |bounds: RetrievalBounds| {
        fixture
            .retrieve(&request, bounds, &EvalBudget::unbounded())
            .unwrap()
    };
    let exact = run(RetrievalBounds {
        scan_rows: NonZeroUsize::new(4).unwrap(),
        ..bounds()
    });
    assert_eq!(exact.completion, Completion::Complete);
    let consumed = &exact.consumed;
    assert_eq!(consumed.probes, 2, "a repeated probe is counted");
    assert_eq!(consumed.counted_rows, 4, "a repeated probe is counted once");
    assert_eq!(consumed.scanned_rows, 4);
    assert_eq!(consumed.ranked_matches, 4);
    // Four hits judged in batches of two, then the four accepted re-judged in one final batch.
    assert_eq!((consumed.judged, consumed.batches), (8, 3));
    assert!(consumed.sql_steps > 0);

    let over = run(RetrievalBounds {
        scan_rows: NonZeroUsize::new(3).unwrap(),
        ..bounds()
    });
    assert_eq!(
        over.completion,
        Completion::Incomplete(IncompleteReason::ScanBound)
    );
    assert_eq!(over.consumed.scanned_rows, 3);
    assert_eq!(over.consumed.counted_rows, 4);
    let further = run(RetrievalBounds {
        scan_rows: NonZeroUsize::new(2).unwrap(),
        ..bounds()
    });
    assert_eq!(further.consumed.scanned_rows, 2);
    assert_eq!(further.consumed.counted_rows, 4);
    assert!(
        further.consumed.sql_steps < over.consumed.sql_steps,
        "the scan side stops at its bound: {} against {}",
        further.consumed.sql_steps,
        over.consumed.sql_steps
    );

    let capped = run(RetrievalBounds {
        max_accepted: NonZeroUsize::new(1).unwrap(),
        ..bounds()
    });
    assert_eq!(capped.contributions.len(), 1);
    assert_eq!(
        capped.consumed.scanned_rows, consumed.scanned_rows,
        "the result cap leaves the scan bound to bound scan work"
    );
    assert!(
        capped.consumed.sql_steps < consumed.sql_steps,
        "admission reads occurrence rows only for the candidates it judges: {} against {}",
        capped.consumed.sql_steps,
        consumed.sql_steps
    );

    // `parse` is common past two qualifying matches, so this run takes the common scan.
    let common_before = run(small_thresholds());
    assert_eq!(
        common_before.completion,
        Completion::Incomplete(IncompleteReason::CommonTerms)
    );
    fixture.admit(&["parse-more"]);
    fixture.project(&[Row::claim("parse-more", "parse once more")]);
    let common_after = run(small_thresholds());
    assert_eq!(
        common_after.consumed.scanned_rows,
        common_before.consumed.scanned_rows + 1
    );
    assert!(
        common_after.consumed.sql_steps > common_before.consumed.sql_steps,
        "the common scan's steps grow with its rows"
    );
    let grown = run(RetrievalBounds {
        scan_rows: NonZeroUsize::new(5).unwrap(),
        ..bounds()
    });
    assert_eq!(grown.consumed.counted_rows, 5);
    assert_eq!(grown.consumed.scanned_rows, 5);
    assert!(
        grown.consumed.sql_steps > consumed.sql_steps,
        "{} steps for five rows against {} for four",
        grown.consumed.sql_steps,
        consumed.sql_steps
    );
}

/// A ranked probe with more matches than the scan bound and one rank per match keeps the first `scan_rows` matches of the reference order whatever their liveness, drops the dead ones at admission, and reports truncation while matches lie past the bound.
#[test]
fn dead_rows_inside_a_distinct_rank_probe_keep_their_window_slots() {
    let fixture = Fixture::all_admitted();
    // Each row adds one more filler token, so every match has its own document length and rank.
    let rows: Vec<Row> = (0..150)
        .map(|n| {
            Row::claim(
                &format!("ladder-{n}"),
                &format!("ladder{}", " pad".repeat(n)),
            )
        })
        .collect();
    fixture.project(&rows);
    let request = probes("ladder");
    let keyed = fixture.keyed_reference(&request);
    assert_eq!(keyed.len(), 150);
    assert!(
        keyed.windows(2).all(|pair| pair[0].1 < pair[1].1),
        "the oracle ranks every match distinctly"
    );
    let reference: Vec<String> = keyed.into_iter().map(|(id, _)| id).collect();
    let scan_rows = bounds().scan_rows.get();
    assert!(reference.len() > 2 * scan_rows);
    // Admitting every ladder row makes each live kept match a contribution.
    let objects: Vec<&str> = rows.iter().map(|row| row.object.as_str()).collect();
    fixture.admit(&objects);
    let wide = RetrievalBounds {
        max_accepted: NonZeroUsize::new(MAX_ELIGIBILITY_CANDIDATES).unwrap(),
        batch_rows: NonZeroUsize::new(MAX_ELIGIBILITY_CANDIDATES).unwrap(),
        ..bounds()
    };

    let dead: Vec<usize> = (0..10).chain(scan_rows - 4..scan_rows + 6).collect();
    for &index in &dead {
        tombstone_raw(&fixture, &reference[index]);
    }
    let kept = fixture.kept(&request, wide);
    assert_eq!(kept, reference[..scan_rows].to_vec());
    let retrieval = fixture
        .retrieve(&request, wide, &EvalBudget::unbounded())
        .unwrap();
    let live: Vec<String> = (0..scan_rows)
        .filter(|index| !dead.contains(index))
        .map(|index| reference[index].clone())
        .collect();
    assert_eq!(ids_of(&retrieval), live);
    assert_eq!(retrieval.consumed.ranked_matches, reference.len());
    assert_eq!(retrieval.consumed.scanned_rows, scan_rows);
    assert_eq!(retrieval.consumed.dead_rows, 14);
    assert_eq!(
        retrieval.completion,
        Completion::Incomplete(IncompleteReason::ScanBound)
    );
}

/// With `max_accepted = 1`, retrieval reads identifiers for the first rank group and the group cut by `scan_rows`. Reading the entire tied group requires more SQL steps than reading distinct-rank groups, with equal counted, ranked, scanned, and judged work.
#[test]
fn identifiers_are_read_for_the_rank_groups_admission_reaches() {
    let scan_rows = bounds().scan_rows.get();
    let matches = 150;
    let run = |text: &dyn Fn(usize) -> String| {
        let fixture = Fixture::all_admitted();
        let rows: Vec<Row> = (0..matches)
            .map(|n| Row::claim(&format!("lazy-{n}"), &text(n)))
            .collect();
        fixture.project(&rows);
        let objects: Vec<&str> = rows.iter().map(|row| row.object.as_str()).collect();
        fixture.admit(&objects);
        let one = RetrievalBounds {
            max_accepted: NonZeroUsize::new(1).unwrap(),
            ..bounds()
        };
        fixture
            .retrieve(&probes("lazy"), one, &EvalBudget::unbounded())
            .unwrap()
    };
    // The increasing token counts give each `ladder` match a distinct rank.
    let ladder = run(&|n| format!("lazy{}", " pad".repeat(n)));
    let tied = run(&|_| "lazy pad".to_string());
    for retrieval in [&ladder, &tied] {
        assert_eq!(retrieval.consumed.counted_rows, matches);
        assert_eq!(retrieval.consumed.ranked_matches, matches);
        assert_eq!(retrieval.consumed.scanned_rows, scan_rows);
        assert_eq!(retrieval.contributions.len(), 1);
        assert_eq!(
            (retrieval.consumed.judged, retrieval.consumed.batches),
            (2, 2),
            "retrieval judges one candidate at admission and re-judges that candidate once"
        );
        assert_eq!(retrieval.consumed.dead_rows, 0);
        assert_eq!(
            retrieval.completion,
            Completion::Incomplete(IncompleteReason::ScanBound)
        );
    }
    assert!(
        ladder.consumed.sql_steps < tied.consumed.sql_steps,
        "distinct ranks read two identifiers against {scan_rows}: {} steps against {}",
        ladder.consumed.sql_steps,
        tied.consumed.sql_steps
    );
}

/// Retrieval counts a duplicate lexical row, which only an inconsistent projection stores, toward `scanned_rows` and deduplicates its `occurrence_id` before judgment, retaining the better rank for the single contribution.
#[test]
fn a_second_lexical_row_for_one_occurrence_yields_one_contribution() {
    let fixture = Fixture::all_admitted();
    let delta = fixture.id("delta");
    let request = probes("io");
    let before = fixture
        .retrieve(&request, bounds(), &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(ids_of(&before), [delta.as_str()]);
    assert_eq!(before.consumed.scanned_rows, 1);
    // Rowid 7 lets both lexical rows coexist; the inserted row's better rank makes it supply the contribution.
    fixture
        .raw()
        .execute(
            "INSERT INTO lexical(rowid, original, parts, occurrence_id) VALUES (7, 'io io io', 'io io io', ?1)",
            [&delta],
        )
        .unwrap();
    let after = fixture
        .retrieve(&request, bounds(), &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(ids_of(&after), [delta.as_str()]);
    assert_eq!(after.consumed.scanned_rows, 2);
    assert_eq!(after.consumed.dead_rows, 0);
    assert_eq!(
        (after.consumed.judged, after.consumed.batches),
        (before.consumed.judged, before.consumed.batches),
        "the duplicate row adds no kernel judgment"
    );
    assert!(
        after.contributions[0].rank < before.contributions[0].rank,
        "the better-ranked row supplies the contribution's rank"
    );
    assert_eq!(after.completion, Completion::Complete);
}

/// When a ranked probe's `scan_rows` bound ends at a rank change, one match from the next rank group suffices to prove truncation, so that group's size leaves the probe's SQL steps unchanged.
#[test]
fn a_bound_that_ends_at_a_rank_change_reads_one_row_of_the_next_rank_group() {
    let scan_rows = bounds().scan_rows.get();
    let tail = 5 * scan_rows;
    let run = |tail_text: &dyn Fn(usize) -> String| {
        let fixture = Fixture::all_admitted();
        let mut head: Vec<Row> = (0..scan_rows)
            .map(|n| Row::claim(&format!("head-{n}"), "align align align"))
            .collect();
        let rows: Vec<Row> = (0..tail)
            .map(|n| Row::claim(&format!("tail-{n}"), &tail_text(n)))
            .collect();
        fixture.project(&head);
        fixture.project(&rows);
        let request = probes("align");
        let keyed = fixture.keyed_reference(&request);
        assert_eq!(keyed.len(), scan_rows + tail);
        assert!(
            keyed[scan_rows - 1].1 < keyed[scan_rows].1,
            "the bound ends at a rank change"
        );
        head.sort_by_key(Row::occurrence_id);
        let expected: Vec<String> = head.iter().map(Row::occurrence_id).collect();
        let scanned = fixture.kept(&request, bounds());
        assert_eq!(scanned, expected);
        let retrieval = fixture
            .retrieve(&request, bounds(), &EvalBudget::unbounded())
            .unwrap();
        assert_eq!(
            retrieval.completion,
            Completion::Incomplete(IncompleteReason::ScanBound)
        );
        assert_eq!(retrieval.consumed.ranked_matches, scan_rows + tail);
        assert_eq!(retrieval.consumed.scanned_rows, scan_rows);
        (keyed, retrieval.consumed.sql_steps)
    };

    let (tied, tied_steps) = run(&|n| format!("align filler {n}"));
    assert!(
        tied[scan_rows..]
            .windows(2)
            .all(|pair| pair[0].1.to_bits() == pair[1].1.to_bits()),
        "every row past the bound shares one rank"
    );
    // Each row adds one more filler token, so every row past the bound has its own document length and rank.
    let (distinct, distinct_steps) = run(&|n| format!("align{}", " pad".repeat(n + 1)));
    assert!(
        distinct[scan_rows..]
            .windows(2)
            .all(|pair| pair[0].1 < pair[1].1),
        "every row past the bound has its own rank"
    );
    assert_eq!(
        tied_steps, distinct_steps,
        "one tied group past the bound costs the SQL steps of rows with one rank each"
    );
}

/// A dead row inside a bound that ends at a rank change keeps its slot, so no row of the next rank group enters the window.
#[test]
fn a_dead_row_at_a_rank_change_opens_no_slot_for_the_next_rank_group() {
    let fixture = Fixture::all_admitted();
    let scan_rows = bounds().scan_rows.get();
    let mut head: Vec<Row> = (0..scan_rows)
        .map(|n| Row::claim(&format!("head-{n}"), "align align align"))
        .collect();
    let mut tail: Vec<Row> = (0..3 * scan_rows)
        .map(|n| Row::claim(&format!("tail-{n}"), &format!("align filler {n}")))
        .collect();
    // Rowid order reaches the tail row with the highest occurrence identifier first.
    tail.sort_by_key(|row| std::cmp::Reverse(row.occurrence_id()));
    fixture.project(&head);
    fixture.project(&tail);
    let request = probes("align");
    let keyed = fixture.keyed_reference(&request);
    assert!(keyed[scan_rows - 1].1 < keyed[scan_rows].1);
    head.sort_by_key(Row::occurrence_id);
    tombstone_raw(&fixture, &head[0].occurrence_id());
    let expected: Vec<String> = head.iter().map(Row::occurrence_id).collect();

    let scanned = fixture.kept(&request, bounds());
    assert_eq!(scanned, expected);
    let retrieval = fixture
        .retrieve(&request, bounds(), &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(retrieval.consumed.scanned_rows, scan_rows);
    assert_eq!(retrieval.consumed.dead_rows, 1);
    assert_eq!(
        retrieval.completion,
        Completion::Incomplete(IncompleteReason::ScanBound)
    );
}

/// `hit_ids` reads the identifiers admission would otherwise read, so a scan admitted after `hit_ids` reports the same `sql_steps` as `retrieve`.
#[test]
fn hit_ids_before_admission_charges_the_identifier_reads_to_sql_steps() {
    let fixture = Fixture::all_admitted();
    let request = probes("parse fetch io");
    let reference = fixture
        .retrieve(&request, bounds(), &EvalBudget::unbounded())
        .unwrap();
    let admitted = fixture
        .store
        .with_conn(|conn| {
            let budget = EvalBudget::unbounded();
            let mut scanned = scan(conn, &request, bounds(), &budget).unwrap();
            assert_eq!(scanned.hit_ids(&budget).unwrap().len(), 6);
            Ok(admit(
                scanned,
                &fixture.kernel,
                fixture.authority(),
                &budget,
            ))
        })
        .unwrap()
        .unwrap();
    assert_eq!(admitted.contributions, reference.contributions);
    assert_eq!(
        admitted.consumed.sql_steps, reference.consumed.sql_steps,
        "the identifier reads `hit_ids` ran are retrieval work"
    );
}

/// The scan bound limits the match window; `usize::MAX` keeps every match on both the ranked and the common path.
#[test]
fn a_scan_bound_of_usize_max_keeps_every_match_on_both_paths() {
    let fixture = Fixture::all_admitted();
    let unbounded_window = |thresholds: RetrievalBounds| RetrievalBounds {
        scan_rows: NonZeroUsize::MAX,
        ..thresholds
    };
    let ranked = fixture
        .retrieve(
            &probes("parse"),
            unbounded_window(bounds()),
            &EvalBudget::unbounded(),
        )
        .unwrap();
    assert_eq!(ranked.completion, Completion::Complete);
    assert_eq!(ranked.consumed.ranked_matches, 4);
    assert_eq!(ids_of(&ranked), fixture.reference(&probes("parse")));
    let common = fixture
        .retrieve(
            &probes("parse"),
            unbounded_window(small_thresholds()),
            &EvalBudget::unbounded(),
        )
        .unwrap();
    assert_eq!(
        common.completion,
        Completion::Incomplete(IncompleteReason::CommonTerms)
    );
    assert_eq!(common.consumed.scanned_rows, 4);
    assert_eq!(common.contributions.len(), 4);
}
