//! A search projection plus a seeded kernel whose admitted objects judge
//! `Ok`, for packer entry tests.

use std::num::{NonZeroU64, NonZeroUsize};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use daemon::packing::{
    AccountingBounds, AccountingProfile, ClaudeTokens, PackingTrace, PreparationRefusal,
    RequiredInputs, RequiredMaterialization, prepare_required, render,
};
use kernel::applicability::EvalBudget;
use kernel::source_identity::{Occurrence, OccurrenceClass, Span, encode};
use kernel::{
    AdmissionEvent, AdmissionRequest, ArtifactDestination, CommitIntent, DecisionPayload,
    DecisionSpec, Dimension, DomainSpec, EventKind, KernelStore, ProjectScope, ScopeSpec,
    ScopeTermSpec, Sensitivity, SourceClass, TaintClass,
};
use retrieval::fusion::OccurrenceId;
use retrieval::packing::{RequiredBounds, RequiredRequest};
use retrieval::{OccurrenceRecord, Payload, PersistBounds, persist_occurrences};
use sha2::{Digest, Sha256};
use storage::{
    GuardedConn, Isolation, SqliteStore, StorageBackend, StorageDescriptor, open_sqlite,
};

pub const PROJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub const DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";
pub const DOMAIN: &str = "domain";
pub const SCOPE: &str = "project:a";

/// One token per rendered byte with no headroom, so limits can be set exactly
/// at a fragment boundary and deltas are linear.
pub fn byte_profile() -> AccountingProfile {
    AccountingProfile::heuristic("one-token-per-byte", "bytes", 0, str::len)
}

/// The byte-profile charge of the block open plus every required fragment.
pub fn required_render_total(spans: &[ToolSpan]) -> u64 {
    let fragments: usize = spans
        .iter()
        .map(|span| render::required_fragment(span.id(), span.selected_bytes()).len())
        .sum();
    (daemon::packing::BLOCK_OPEN_FRAGMENT.len() + fragments) as u64
}

pub fn accounting_bounds() -> AccountingBounds {
    AccountingBounds {
        max_rendered_bytes: 1 << 22,
        max_estimated_tokens: ClaudeTokens::new(1 << 22),
    }
}

#[derive(Clone, Copy)]
pub struct ToolSpan {
    pub call: &'static str,
    pub revision: &'static str,
    pub payload: &'static str,
    pub span: Option<Span>,
}

impl ToolSpan {
    pub fn identity(&self) -> [(&'static str, &'static str); 7] {
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

    pub fn occurrence<'a>(&self, identity: &'a [(&'a str, &'a str)]) -> Occurrence<'a> {
        Occurrence {
            class: OccurrenceClass::RawToolSpans.code(),
            identity,
            revision: self.revision,
            representation: "tool_output",
            span: self.span,
        }
    }

    pub fn id(&self) -> OccurrenceId {
        let identity = self.identity();
        OccurrenceId::parse(
            &encode(&self.occurrence(&identity), self.payload)
                .unwrap()
                .occurrence_id,
        )
        .unwrap()
    }

    pub fn request(&self) -> RequiredRequest {
        RequiredRequest {
            occurrence: self.id(),
            revision: self.revision.parse().unwrap(),
        }
    }

    pub fn selected_bytes(&self) -> &'static [u8] {
        match self.span {
            Some(span) => &self.payload.as_bytes()[span.start as usize..span.end as usize],
            None => self.payload.as_bytes(),
        }
    }
}

pub const fn tool_span(
    call: &'static str,
    revision: &'static str,
    payload: &'static str,
) -> ToolSpan {
    ToolSpan {
        call,
        revision,
        payload,
        span: None,
    }
}

pub const fn tool_range(
    call: &'static str,
    payload: &'static str,
    start: u64,
    end: u64,
) -> ToolSpan {
    ToolSpan {
        call,
        revision: "1",
        payload,
        span: Some(Span { start, end }),
    }
}

pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub store: SqliteStore,
    pub kernel: KernelStore,
    pub project: ProjectScope,
}

/// Every span's source object is admitted in the kernel, so eligibility is
/// `Ok` unless a test names an object the kernel never saw.
pub fn seed_kernel(kernel: &KernelStore, objects: &[&str]) {
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
    pub fn new(spans: &[ToolSpan]) -> Self {
        let mut admitted: Vec<&str> = spans.iter().map(|span| span.call).collect();
        admitted.sort_unstable();
        admitted.dedup();
        Self::with_admitted(spans, &admitted)
    }

    pub fn with_admitted(spans: &[ToolSpan], admitted: &[&str]) -> Self {
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

    pub fn sqlite_path(&self) -> PathBuf {
        self.dir.path().join("search.sqlite")
    }

    pub fn prepare(
        &self,
        requests: &[RequiredRequest],
        bounds: &RequiredBounds<ClaudeTokens>,
        budget: &EvalBudget,
    ) -> (
        Result<RequiredMaterialization, PreparationRefusal>,
        PackingTrace,
    ) {
        let mut trace = PackingTrace::default();
        trace.note_retrieval_call();
        let profile = byte_profile();
        let result = prepare_required(
            &self.store,
            RequiredInputs {
                kernel: &self.kernel,
                project: &self.project,
                destination: ArtifactDestination::Local,
                budget,
                profile: &profile,
            },
            requests,
            bounds,
            &accounting_bounds(),
            &mut trace,
        );
        (result, trace)
    }
}

pub fn persist(conn: &GuardedConn<'_>, spans: &[ToolSpan]) {
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

pub fn bounds(token_limit: u64) -> RequiredBounds<ClaudeTokens> {
    RequiredBounds {
        max_payload_loads: NonZeroUsize::new(8).unwrap(),
        max_payload_bytes: NonZeroU64::new(1 << 20).unwrap(),
        max_item_bytes: NonZeroU64::new(1 << 19).unwrap(),
        token_limit: ClaudeTokens::new(token_limit),
    }
}

/// Holds the projection connection on another thread until `prepare` returns
/// or `HOLD_CEILING` passes, then reports how long `prepare` took.
pub fn while_connection_is_held<T>(
    fixture: &Fixture,
    prepare: impl FnOnce() -> T,
) -> (Duration, T) {
    const HOLD_CEILING: Duration = Duration::from_secs(5);
    let (holding_tx, holding) = mpsc::channel::<()>();
    let (release_tx, release) = mpsc::channel::<()>();
    std::thread::scope(|scope| {
        scope.spawn(move || {
            fixture
                .store
                .with_conn(|_| {
                    holding_tx.send(()).unwrap();
                    let _ = release.recv_timeout(HOLD_CEILING);
                    Ok(())
                })
                .unwrap();
        });
        holding.recv_timeout(HOLD_CEILING).unwrap();
        let started = Instant::now();
        let outcome = prepare();
        let waited = started.elapsed();
        // The holder has already left once `HOLD_CEILING` passed; the elapsed
        // assertion reports that, not this send.
        let _ = release_tx.send(());
        (waited, outcome)
    })
}
