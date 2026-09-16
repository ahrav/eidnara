//! The required phase completes or refuses before any optional event, never
//! retrieves, and never truncates a required payload.

use std::num::{NonZeroU64, NonZeroUsize};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use daemon::packing::{
    ClaudeTokens, CostEstimator, PackingTrace, PreparationRefusal, RequiredEvent, RequiredInputs,
    StageEvent, prepare_required,
};
use kernel::applicability::EvalBudget;
use kernel::source_identity::{Occurrence, OccurrenceClass, encode};
use kernel::{
    AdmissionEvent, AdmissionRequest, ArtifactDestination, CommitIntent, DecisionPayload,
    DecisionSpec, Dimension, DomainSpec, EligibilityVerdict, EventKind, KernelStore, ProjectScope,
    ScopeSpec, ScopeTermSpec, Sensitivity, SourceClass, TaintClass,
};
use retrieval::fusion::OccurrenceId;
use retrieval::packing::{RequiredBound, RequiredBounds, RequiredContextFailure, RequiredRequest};
use retrieval::{
    OccurrenceRecord, Payload, PersistBounds, Tombstone, TombstoneReason, persist_occurrences,
    tombstone_occurrence,
};
use sha2::{Digest, Sha256};
use storage::{
    GuardedConn, Isolation, SqliteStore, StorageBackend, StorageDescriptor, open_sqlite,
};

const PROJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const DOMAIN: &str = "domain";
const SCOPE: &str = "project:a";

/// One token per byte, so limits can be set exactly at a payload boundary.
struct ByteEstimator;

impl CostEstimator for ByteEstimator {
    fn profile(&self) -> &'static str {
        "one-token-per-byte"
    }

    fn cost(&self, bytes: &[u8]) -> ClaudeTokens {
        ClaudeTokens::new(bytes.len() as u64)
    }
}

#[derive(Clone, Copy)]
struct ToolSpan {
    call: &'static str,
    revision: &'static str,
    payload: &'static str,
}

impl ToolSpan {
    fn identity(&self) -> [(&'static str, &'static str); 7] {
        [
            ("project_id", "proj-a"),
            ("harness", "opencode"),
            ("session_id", "sess-01"),
            ("parent_message_id", "msg-002"),
            ("tool_call_id", self.call),
            ("result_revision", "1"),
            ("block_index", "0"),
        ]
    }

    fn occurrence<'a>(&self, identity: &'a [(&'a str, &'a str)]) -> Occurrence<'a> {
        Occurrence {
            class: OccurrenceClass::RawToolSpans.code(),
            identity,
            revision: self.revision,
            representation: "tool_output",
            span: None,
        }
    }

    fn id(&self) -> OccurrenceId {
        let identity = self.identity();
        OccurrenceId::parse(
            &encode(&self.occurrence(&identity), self.payload)
                .unwrap()
                .occurrence_id,
        )
        .unwrap()
    }

    fn request(&self) -> RequiredRequest {
        RequiredRequest {
            occurrence: self.id(),
            revision: self.revision.parse().unwrap(),
        }
    }

    fn selected_bytes(&self) -> &'static [u8] {
        self.payload.as_bytes()
    }
}

const fn tool_span(call: &'static str, revision: &'static str, payload: &'static str) -> ToolSpan {
    ToolSpan {
        call,
        revision,
        payload,
    }
}

struct Fixture {
    dir: tempfile::TempDir,
    store: SqliteStore,
    kernel: KernelStore,
    project: ProjectScope,
}

/// Every span's source object is admitted in the kernel, so eligibility is
/// `Ok` unless a test names an object the kernel never saw.
fn seed_kernel(kernel: &KernelStore, objects: &[&str]) {
    kernel
        .commit(
            CommitIntent {
                producer: "packing-required-test".to_string(),
                operation_key: "seed".to_string(),
                request_digest: format!("{:x}", Sha256::digest(b"seed")),
                actor: "test".to_string(),
                cause: "packing".to_string(),
            },
            |envelope| {
                envelope.insert_domain(DomainSpec {
                    domain_id: DOMAIN.to_string(),
                    object_id: "domain-object".to_string(),
                    name: "fixture".to_string(),
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
                for object in objects {
                    envelope.insert_decision(DecisionSpec {
                        decision_id: format!("decision-{object}"),
                        object_id: object.to_string(),
                        domain_id: DOMAIN.to_string(),
                        proposition_id: None,
                        scope_id: Some(SCOPE.to_string()),
                        anchor_id: None,
                        evidence_id: None,
                        decision_kind: "architecture".to_string(),
                        payload: DecisionPayload {
                            summary: format!("summary {object}"),
                            rationale: format!("rationale {object}"),
                        },
                        source_kind: "repository".to_string(),
                        source_id: object.to_string(),
                        source_revision: 1,
                        sensitivity: Sensitivity::Normal,
                    })?;
                    envelope.record_admission(AdmissionRequest {
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
                    })?;
                }
                Ok(String::new())
            },
        )
        .unwrap();
}

impl Fixture {
    fn new(spans: &[ToolSpan]) -> Self {
        Self::with_admitted(
            spans,
            &spans.iter().map(|span| span.call).collect::<Vec<_>>(),
        )
    }

    fn with_admitted(spans: &[ToolSpan], admitted: &[&str]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = open_sqlite(
            &StorageDescriptor {
                module_id: "eidnara-test".to_string(),
                storage_namespace: "search-projection".to_string(),
                isolation: Isolation::Module,
                backend: StorageBackend::Sqlite {
                    path: dir
                        .path()
                        .join("search.sqlite")
                        .to_string_lossy()
                        .into_owned(),
                },
            },
            retrieval::BASELINE,
        )
        .unwrap();
        let kernel = KernelStore::open(dir.path().join("kernel")).unwrap();
        seed_kernel(&kernel, admitted);
        store
            .with_conn_fenced(|conn| {
                persist(conn, spans);
                Ok(())
            })
            .unwrap();
        Self {
            dir,
            store,
            kernel,
            project: ProjectScope::new(PROJECT).unwrap(),
        }
    }

    fn sqlite_path(&self) -> std::path::PathBuf {
        self.dir.path().join("search.sqlite")
    }

    fn prepare(
        &self,
        requests: &[RequiredRequest],
        bounds: &RequiredBounds<ClaudeTokens>,
        budget: &EvalBudget,
    ) -> (
        Result<daemon::packing::RequiredMaterialization, PreparationRefusal>,
        PackingTrace,
    ) {
        let mut trace = PackingTrace::default();
        trace.note_retrieval_call();
        let result = prepare_required(
            &self.store,
            RequiredInputs {
                kernel: &self.kernel,
                project: &self.project,
                destination: ArtifactDestination::Local,
                budget,
                estimator: &ByteEstimator,
            },
            requests,
            bounds,
            &mut trace,
        );
        (result, trace)
    }
}

fn persist(conn: &GuardedConn<'_>, spans: &[ToolSpan]) {
    let identities: Vec<_> = spans.iter().map(ToolSpan::identity).collect();
    let records: Vec<OccurrenceRecord<'_>> = spans
        .iter()
        .zip(&identities)
        .map(|(span, identity)| OccurrenceRecord {
            occurrence: span.occurrence(identity),
            payload: Payload::Whole(span.payload),
            domain_id: "domain-stable-id",
            sensitivity: Sensitivity::Normal,
            source_object_id: span.call,
            source_evidence_id: "evidence",
            source_artifact_digest: DIGEST,
            created_commit_seq: 7,
        })
        .collect();
    persist_occurrences(
        conn,
        &records,
        PersistBounds {
            max_records: NonZeroUsize::new(64).unwrap(),
            max_payload_bytes: NonZeroUsize::new(1 << 20).unwrap(),
            max_tuple_bytes: NonZeroUsize::new(2048).unwrap(),
        },
        1,
    )
    .unwrap();
}

fn bounds(token_limit: u64) -> RequiredBounds<ClaudeTokens> {
    RequiredBounds {
        max_payload_loads: NonZeroUsize::new(8).unwrap(),
        max_payload_bytes: NonZeroU64::new(1 << 20).unwrap(),
        max_item_bytes: NonZeroU64::new(1 << 19).unwrap(),
        token_limit: ClaudeTokens::new(token_limit),
    }
}

fn required_failure(
    result: &Result<daemon::packing::RequiredMaterialization, PreparationRefusal>,
) -> &RequiredContextFailure<ClaudeTokens> {
    match result {
        Err(PreparationRefusal::Required(failure)) => failure,
        other => panic!("expected a required-context failure, got {other:?}"),
    }
}

/// The trace enters every call with one lane retrieval already noted, so an
/// entry that retrieved would show two and one that reset the counter zero.
fn assert_no_optional_work(trace: &PackingTrace) {
    assert_eq!(trace.optional_events(), 0);
    assert_eq!(trace.retrieval_calls(), 1);
}

fn required_events(trace: &PackingTrace) -> Vec<RequiredEvent> {
    trace
        .events()
        .iter()
        .map(|event| match event {
            StageEvent::Required(event, _) => *event,
            StageEvent::Optional => panic!("optional event in the required phase"),
        })
        .collect()
}

type Case = (
    &'static str,
    Vec<RequiredRequest>,
    RequiredBounds<ClaudeTokens>,
    RequiredContextFailure<ClaudeTokens>,
);

const FIRST: ToolSpan = tool_span("call-1", "1", "the first required payload\n");
const SECOND: ToolSpan = tool_span("call-2", "1", "the second one, longer by a bit\n");

#[test]
fn required_cost_at_the_limit_succeeds_and_one_above_fails_without_truncation() {
    let fixture = Fixture::new(&[FIRST, SECOND]);
    let requests = [FIRST.request(), SECOND.request()];
    let total = (FIRST.payload.len() + SECOND.payload.len()) as u64;

    let (ok, trace) = fixture.prepare(&requests, &bounds(total), &EvalBudget::unbounded());
    let materialized = ok.unwrap();
    assert_eq!(materialized.charged, ClaudeTokens::new(total));
    assert_eq!(materialized.profile, "one-token-per-byte");
    let bytes: Vec<&[u8]> = materialized
        .items
        .iter()
        .map(|item| item.bytes.as_slice())
        .collect();
    assert_eq!(bytes, vec![FIRST.selected_bytes(), SECOND.selected_bytes()]);
    assert_eq!(
        materialized
            .items
            .iter()
            .map(|item| item.cost.get())
            .sum::<u64>(),
        total,
        "every materialized byte is charged"
    );
    assert_eq!(trace.payload_loads(), 2);
    assert_eq!(
        required_events(&trace),
        [
            RequiredEvent::Read,
            RequiredEvent::Read,
            RequiredEvent::Judged,
            RequiredEvent::Admitted,
            RequiredEvent::Loaded,
            RequiredEvent::Loaded,
            RequiredEvent::Reserved,
        ]
    );
    assert_no_optional_work(&trace);

    let (above, trace) = fixture.prepare(&requests, &bounds(total - 1), &EvalBudget::unbounded());
    assert_eq!(
        required_failure(&above),
        &RequiredContextFailure::OverBudget {
            limit: ClaudeTokens::new(total - 1),
            charged: ClaudeTokens::new(total),
        }
    );
    assert_eq!(trace.payload_loads(), 2, "bytes were loaded, then refused");
    assert_no_optional_work(&trace);

    let (below, _) = fixture.prepare(&requests, &bounds(total + 1), &EvalBudget::unbounded());
    assert_eq!(below.unwrap().charged, ClaudeTokens::new(total));
}

#[test]
fn each_required_fault_yields_exactly_one_class_with_zero_optional_events() {
    let tombstoned = tool_span("call-3", "1", "retired bytes\n");
    let big = tool_span("call-4", "1", "0123456789abcdef0123456789abcdef");
    let fixture = Fixture::new(&[FIRST, SECOND, tombstoned, big]);
    fixture
        .store
        .with_conn_fenced(|conn| {
            tombstone_occurrence(
                conn,
                &tombstoned.id().to_string(),
                Tombstone {
                    invalidated_commit_seq: 9,
                    reason: TombstoneReason::Retired,
                },
                2,
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
    let missing = tool_span("call-9", "1", "never persisted\n");
    let stale = RequiredRequest {
        occurrence: FIRST.id(),
        revision: 2,
    };
    let small_items = RequiredBounds {
        max_item_bytes: NonZeroU64::new(16).unwrap(),
        ..bounds(1 << 20)
    };
    let one_load = RequiredBounds {
        max_payload_loads: NonZeroUsize::MIN,
        ..bounds(1 << 20)
    };
    let few_bytes = RequiredBounds {
        max_payload_bytes: NonZeroU64::new(FIRST.payload.len() as u64 + 3).unwrap(),
        ..bounds(1 << 20)
    };

    let cases: Vec<Case> = vec![
        (
            "missing",
            vec![FIRST.request(), missing.request()],
            bounds(1 << 20),
            RequiredContextFailure::Missing(missing.id()),
        ),
        (
            "stale",
            vec![stale],
            bounds(1 << 20),
            RequiredContextFailure::Stale(FIRST.id()),
        ),
        (
            "stale",
            vec![tombstoned.request()],
            bounds(1 << 20),
            RequiredContextFailure::Stale(tombstoned.id()),
        ),
        (
            "oversized",
            vec![big.request()],
            small_items,
            RequiredContextFailure::Oversized {
                occurrence: big.id(),
                bound: RequiredBound::ItemBytes,
            },
        ),
        (
            "oversized",
            vec![FIRST.request(), SECOND.request()],
            one_load,
            RequiredContextFailure::Oversized {
                occurrence: SECOND.id(),
                bound: RequiredBound::PayloadLoads,
            },
        ),
        (
            "oversized",
            vec![FIRST.request(), SECOND.request()],
            few_bytes,
            RequiredContextFailure::Oversized {
                occurrence: SECOND.id(),
                bound: RequiredBound::PayloadBytes,
            },
        ),
    ];
    for (class, requests, bounds, expected) in cases {
        let (result, trace) = fixture.prepare(&requests, &bounds, &EvalBudget::unbounded());
        let failure = required_failure(&result);
        assert_eq!(failure, &expected);
        assert_eq!(failure.class(), class);
        assert_eq!(
            trace.payload_loads(),
            0,
            "{class}: refused before any byte is loaded"
        );
        assert!(!required_events(&trace).contains(&RequiredEvent::Loaded));
        assert_no_optional_work(&trace);
    }
}

#[test]
fn a_required_occurrence_the_kernel_excludes_is_ineligible_not_missing() {
    let fixture = Fixture::with_admitted(&[FIRST], &[]);
    let (result, trace) = fixture.prepare(
        &[FIRST.request()],
        &bounds(1 << 20),
        &EvalBudget::unbounded(),
    );
    match required_failure(&result) {
        RequiredContextFailure::Ineligible {
            occurrence,
            verdict,
        } => {
            assert_eq!(*occurrence, FIRST.id());
            assert_eq!(*verdict, EligibilityVerdict::Retracted);
        }
        other => panic!("{other:?}"),
    }
    assert_no_optional_work(&trace);
}

#[test]
fn corrupt_payload_bytes_and_foreign_tuples_are_refused_as_corrupt() {
    let fixture = Fixture::new(&[FIRST, SECOND]);
    let path = fixture.sqlite_path();
    let damage = |sql: &str, params: &[&dyn rusqlite::ToSql]| {
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute(sql, params)
            .unwrap();
    };
    let payload_id = kernel::source_identity::payload_id(FIRST.payload.as_bytes());
    let mut altered = FIRST.payload.as_bytes().to_vec();
    altered[0] ^= 1;
    damage(
        "UPDATE payloads SET bytes=?2 WHERE payload_id=?1",
        &[&payload_id, &altered],
    );
    let (result, trace) = fixture.prepare(
        &[FIRST.request()],
        &bounds(1 << 20),
        &EvalBudget::unbounded(),
    );
    assert_eq!(
        required_failure(&result),
        &RequiredContextFailure::Corrupt(FIRST.id())
    );
    assert_eq!(trace.payload_loads(), 0, "the load is refused, not charged");
    assert_eq!(
        required_events(&trace),
        [
            RequiredEvent::Read,
            RequiredEvent::Judged,
            RequiredEvent::Admitted
        ]
    );
    assert_no_optional_work(&trace);
    damage(
        "UPDATE payloads SET bytes=?2 WHERE payload_id=?1",
        &[&payload_id, &FIRST.payload.as_bytes()],
    );

    let identity = SECOND.identity();
    let foreign = encode(&SECOND.occurrence(&identity), SECOND.payload)
        .unwrap()
        .tuple;
    damage(
        "UPDATE occurrences SET tuple=?2 WHERE occurrence_id=?1",
        &[&FIRST.id().to_string(), &foreign],
    );
    let (result, trace) = fixture.prepare(
        &[FIRST.request()],
        &bounds(1 << 20),
        &EvalBudget::unbounded(),
    );
    assert_eq!(
        required_failure(&result),
        &RequiredContextFailure::Corrupt(FIRST.id())
    );
    assert_no_optional_work(&trace);
}

#[test]
fn an_expired_deadline_refuses_the_required_phase_before_any_optional_event() {
    let fixture = Fixture::new(&[FIRST]);
    let expired = EvalBudget::new(
        Some(Instant::now() - Duration::from_secs(1)),
        Arc::new(AtomicBool::new(false)),
    );
    let (result, trace) = fixture.prepare(&[FIRST.request()], &bounds(1 << 20), &expired);
    assert_eq!(result.unwrap_err(), PreparationRefusal::Deadline);
    assert!(trace.events().is_empty());
    assert_no_optional_work(&trace);

    let cancelled = EvalBudget::unbounded();
    cancelled.cancel();
    let (result, _) = fixture.prepare(&[FIRST.request()], &bounds(1 << 20), &cancelled);
    assert_eq!(result.unwrap_err(), PreparationRefusal::Deadline);
}

#[test]
fn a_required_payload_beyond_the_legacy_cut_is_materialized_and_charged_whole() {
    const LEN: usize = 64 * 1024 + 7;
    static BIG_PAYLOAD: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| "x".repeat(LEN));
    let big = tool_span("call-big", "1", BIG_PAYLOAD.as_str());
    let fixture = Fixture::new(&[big]);
    let (result, _) = fixture.prepare(
        &[big.request()],
        &bounds(LEN as u64),
        &EvalBudget::unbounded(),
    );
    let materialized = result.unwrap();
    assert_eq!(materialized.items.len(), 1);
    assert_eq!(materialized.items[0].bytes.len(), LEN);
    assert_eq!(materialized.items[0].bytes, BIG_PAYLOAD.as_bytes());
    assert_eq!(materialized.charged, ClaudeTokens::new(LEN as u64));
}

#[test]
fn more_requests_than_the_load_bound_are_refused_before_any_read() {
    let fixture = Fixture::new(&[FIRST, SECOND]);
    let one_load = RequiredBounds {
        max_payload_loads: NonZeroUsize::MIN,
        ..bounds(1 << 20)
    };
    let (result, trace) = fixture.prepare(
        &[FIRST.request(), SECOND.request()],
        &one_load,
        &EvalBudget::unbounded(),
    );
    assert_eq!(
        required_failure(&result),
        &RequiredContextFailure::Oversized {
            occurrence: SECOND.id(),
            bound: RequiredBound::PayloadLoads,
        }
    );
    assert!(trace.events().is_empty());
    assert_no_optional_work(&trace);
}

#[test]
fn every_failure_class_has_a_distinct_literal() {
    let id = FIRST.id();
    let literals = [
        RequiredContextFailure::<ClaudeTokens>::Missing(id).class(),
        RequiredContextFailure::<ClaudeTokens>::Stale(id).class(),
        RequiredContextFailure::<ClaudeTokens>::Hidden(id).class(),
        RequiredContextFailure::<ClaudeTokens>::Corrupt(id).class(),
        RequiredContextFailure::<ClaudeTokens>::Ineligible {
            occurrence: id,
            verdict: EligibilityVerdict::WrongScope,
        }
        .class(),
        RequiredContextFailure::<ClaudeTokens>::Oversized {
            occurrence: id,
            bound: RequiredBound::ItemBytes,
        }
        .class(),
        RequiredContextFailure::OverBudget {
            limit: ClaudeTokens::new(1),
            charged: ClaudeTokens::new(2),
        }
        .class(),
    ];
    let distinct: std::collections::BTreeSet<_> = literals.iter().collect();
    assert_eq!(distinct.len(), literals.len());
}
