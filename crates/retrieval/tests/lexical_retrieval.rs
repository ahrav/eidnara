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
const QUALIFYING_MATCHES: usize = 20_000;
const RANK_BUDGET: usize = 30_000;

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

struct Fixture {
    root: tempfile::TempDir,
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
            root,
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
fn admitting_a_released_scan_equals_retrieve_and_carries_the_judged_candidate() {
    let fixture = Fixture::all_admitted();
    let request = probes("parse fetch io");
    let reference = fixture
        .retrieve(&request, bounds(), &EvalBudget::unbounded())
        .unwrap();
    let scanned = fixture
        .store
        .with_conn(|conn| Ok(scan(conn, &request, bounds(), &EvalBudget::unbounded())))
        .unwrap()
        .unwrap();
    assert_eq!(scanned.hits(), 6);
    // The projection connection is released here; admission needs only the kernel.
    let admitted = admit(
        &fixture.kernel,
        fixture.authority(),
        scanned,
        &EvalBudget::unbounded(),
    )
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

#[test]
fn the_accepted_bound_inside_a_batch_still_tallies_the_rest_and_an_exact_fill_stays_complete() {
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
    assert_eq!(retrieval.consumed.judged, 4 + 2);
    assert_eq!(retrieval.consumed.batches, 2);
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

/// `before` is the reference order with `stale` still live. For every `scan_rows` from 1 through
/// `before.len()`, retrieval must return the live prefix of that order and report truncation only
/// while live rows remain past the bound, wherever `stale` sat in the shortlist.
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
    assert_eq!(retrieval.consumed.scanned_rows, before.len() - 1);
    assert!(!ids_of(&retrieval).contains(&stale_id));
    assert_eq!(retrieval.contributions.len(), before.len() - 1);

    let expected: Vec<String> = before
        .iter()
        .filter(|id| **id != stale_id)
        .cloned()
        .collect();
    let stale_at = before.iter().position(|id| *id == stale_id).unwrap();
    for scan_rows in 1..=before.len() {
        let tight = RetrievalBounds {
            scan_rows: NonZeroUsize::new(scan_rows).unwrap(),
            ..bounds()
        };
        let retrieval = fixture
            .retrieve(request, tight, &EvalBudget::unbounded())
            .unwrap();
        let want = &expected[..scan_rows.min(expected.len())];
        assert_eq!(ids_of(&retrieval), want, "scan_rows={scan_rows}");
        assert_eq!(
            retrieval.completion,
            if scan_rows < expected.len() {
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

#[test]
fn tombstoned_rows_inside_the_scan_bound_do_not_take_slots_or_hide_truncation() {
    let fixture = Fixture::all_admitted();
    let bulk = project_bulk(&fixture, 2500);
    let request = probes("parse");
    let reference = fixture.reference(&request);
    let scan = bounds().scan_rows.get();
    let page = scan + 1;
    assert!(reference.len() > 2 * scan);

    // Dead rows cover the whole first page and part of the second, so the live prefix spans three pages.
    let dead = scan + 6;
    for occurrence_id in &reference[..dead] {
        tombstone_raw(&fixture, occurrence_id);
    }
    // Admitted sentinels sit at every edge a paging error could move, so the contributions are exactly the sentinels the scan took.
    let boundary = (dead / page + 1) * page;
    assert!(dead < boundary - 1 && boundary < dead + scan - 1);
    let sentinels = [
        dead - 1,
        dead,
        boundary - 1,
        boundary,
        dead + scan - 1,
        dead + scan,
    ];
    let objects: Vec<&str> = sentinels
        .iter()
        .map(|&index| object_of(&bulk, &reference[index]))
        .collect();
    fixture.admit(&objects);
    let expected: Vec<String> = [dead, boundary - 1, boundary, dead + scan - 1]
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
    assert_eq!(retrieval.consumed.judged, scan + expected.len());
    assert_eq!(
        retrieval.completion,
        Completion::Incomplete(IncompleteReason::ScanBound)
    );
}

#[test]
fn a_dead_row_that_fills_the_page_past_the_bound_leaves_the_result_complete() {
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
    assert_eq!(ids_of(&retrieval), fixture.eligible(&reference[1..]));
    assert_eq!(retrieval.consumed.scanned_rows, scan);
    assert_eq!(
        retrieval.consumed.judged,
        scan + retrieval.contributions.len()
    );
    assert_eq!(retrieval.completion, Completion::Complete);
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

/// 20,001 rows whose terms match exactly the counts the D26b boundaries name: `t19999`, `t20000`, and `t20001` match that many rows;
/// `c15000` matches 15,000 rows and `d14999`, `d15000`, `d15001` match that many, so a pair sums to 29,999, 30,000, or 30,001.
fn project_thresholds(fixture: &Fixture) -> Vec<Row> {
    let rows: Vec<Row> = (0..20_001)
        .map(|n: usize| {
            let mut terms = vec![format!("row{n}")];
            for (term, count) in [
                ("t19999", 19_999),
                ("t20000", 20_000),
                ("t20001", 20_001),
                ("c15000", 15_000),
                ("d14999", 14_999),
                ("d15000", 15_000),
                ("d15001", 15_001),
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
    assert_eq!((QUALIFYING_MATCHES, RANK_BUDGET), (20_000, 30_000));
    let fixture = Fixture::all_admitted();
    project_thresholds(&fixture);
    let run = |request: &str| {
        fixture
            .retrieve(&probes(request), bounds(), &EvalBudget::unbounded())
            .unwrap()
    };

    // A probe at or under the threshold is counted exactly and ranked; one row past it is common and unranked.
    for (term, count) in [("t19999", 19_999), ("t20000", 20_000)] {
        let ranked = run(term);
        assert_eq!(ranked.consumed.counted_rows, count, "{term}");
        assert_eq!(ranked.consumed.ranked_matches, count, "{term}");
        assert_eq!(
            ranked.completion,
            Completion::Incomplete(IncompleteReason::ScanBound)
        );
    }
    let common = run("t20001");
    assert_eq!(
        common.consumed.counted_rows, 20_001,
        "one lookahead row past the threshold"
    );
    assert_eq!(common.consumed.ranked_matches, 0);
    assert_eq!(
        common.completion,
        Completion::Incomplete(IncompleteReason::CommonTerms)
    );
    assert_eq!(common.consumed.scanned_rows, bounds().scan_rows.get());

    // The cumulative budget admits a pair summing to 29,999 or 30,000 and skips the larger probe at 30,001.
    for (second, count) in [("d14999", 14_999), ("d15000", 15_000)] {
        let both = run(&format!("c15000 {second}"));
        assert_eq!(both.consumed.ranked_matches, 15_000 + count, "{second}");
        assert_eq!(
            both.completion,
            Completion::Incomplete(IncompleteReason::ScanBound)
        );
    }
    let skipped = run("c15000 d15001");
    assert_eq!(skipped.consumed.ranked_matches, 15_000);
    assert_eq!(
        skipped.completion,
        Completion::Incomplete(IncompleteReason::RankBudget)
    );
    // Admission follows increasing count, not request order: the smaller probe is ranked even when it comes second.
    let reordered = run("d15001 c15000");
    assert_eq!(reordered.consumed.ranked_matches, 15_000);
    assert_eq!(
        reordered.completion,
        Completion::Incomplete(IncompleteReason::RankBudget)
    );

    // A repeated probe is counted and ranked once.
    let repeated = run("t19999 t19999");
    assert_eq!(repeated.consumed.counted_rows, 19_999);
    assert_eq!(repeated.consumed.ranked_matches, 19_999);
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
    let scanned = fixture
        .store
        .with_conn(|conn| {
            Ok(scan(
                conn,
                &probes("t20001"),
                tight,
                &EvalBudget::unbounded(),
            ))
        })
        .unwrap()
        .unwrap();
    let mut highest: Vec<String> = fixture
        .raw()
        .prepare("SELECT occurrence_id FROM lexical WHERE lexical MATCH 't20001' ORDER BY rowid DESC LIMIT 5")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    highest.sort();
    assert_eq!(scanned.hits(), 5);
    let mut hit: Vec<String> = scanned.hit_ids().map(str::to_string).collect();
    hit.sort();
    assert_eq!(hit, highest, "the common scan keeps the highest rowids");
    // Admitting the kept rows' sources makes every one of them a contribution, so the assertion below sees them all.
    let objects: Vec<&str> = highest.iter().map(|id| object_of(&rows, id)).collect();
    fixture.admit(&objects);
    let only_common = fixture
        .retrieve(&probes("t20001"), tight, &EvalBudget::unbounded())
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
        .retrieve(&probes("row7 t20001"), tight, &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(mixed.consumed.ranked_matches, 1);
    assert_eq!(
        mixed.completion,
        Completion::Incomplete(IncompleteReason::CommonTerms)
    );
    assert_eq!(mixed.consumed.scanned_rows, 1);
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

/// D26b lexical gate at one million occurrences under the production lexical bounds: an open-loop Poisson schedule of
/// rare, qualifying, mixed, and common-only queries, each timed from its scheduled arrival to the end of its scan and
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
        // Filler of 0 to 96 tokens spreads the ranks, and at one million occurrences each `k*` term matches exactly 20,000 rows, the largest qualifying probe.
        let text = format!(
            "w{} m{} k{} c{} common{}",
            n % 50_000,
            n % 500,
            n % 50,
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
            &probes("absentterm t20001"),
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
    let scanned = fixture
        .store
        .with_conn(|conn| Ok(scan(conn, &request, bounds(), &EvalBudget::unbounded())))
        .unwrap()
        .unwrap();
    assert_eq!(scanned.hits(), 64);
    assert_eq!(scanned.hit_ids().collect::<Vec<_>>(), expected);
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
    // A wide scan bound makes the common scan, not its count, most of the `t20001` work at 4,096 rows.
    let wide = RetrievalBounds {
        scan_rows: NonZeroUsize::new(4096).unwrap(),
        max_accepted: NonZeroUsize::new(MAX_ELIGIBILITY_CANDIDATES).unwrap(),
        ..bounds()
    };
    for (request, tight) in [
        ("t20001", tight),
        ("t20001", wide),
        ("t20000", tight),
        ("c15000 d15000", tight),
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
fn dead_rows_leading_a_common_probe_neither_take_its_scan_bound_nor_hide_truncation() {
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
    // The dead rows fill one whole read of `scan_rows + 1` rows and one more, so a single read finds no live row.
    let dead = scan_rows + 2;
    for occurrence_id in &descending[..dead] {
        tombstone_raw(&fixture, occurrence_id);
    }
    let scanned = fixture
        .store
        .with_conn(|conn| {
            Ok(scan(
                conn,
                &probes("parse"),
                common,
                &EvalBudget::unbounded(),
            ))
        })
        .unwrap()
        .unwrap();
    let mut kept: Vec<String> = scanned.hit_ids().map(str::to_string).collect();
    kept.sort();
    let mut expected = descending[dead..dead + scan_rows].to_vec();
    expected.sort();
    assert_eq!(kept, expected);

    let retrieval = fixture
        .retrieve(&probes("parse"), common, &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(retrieval.consumed.scanned_rows, scan_rows);
    assert_eq!(
        retrieval.reasons,
        vec![IncompleteReason::CommonTerms, IncompleteReason::ScanBound]
    );
}
