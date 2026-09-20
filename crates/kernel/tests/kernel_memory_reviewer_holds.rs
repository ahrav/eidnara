#![cfg(feature = "test-support")]

//! Real-store proofs for MemoryReviewer execution and review holds: atomic bounded acquisition, quota refusal that writes nothing, review transfer that acquires before releasing, kind-specific release authority, stale-worker refusal, reopen, purge and GC interplay, and the append-only run buffer map at its capacity boundary.

use kernel::{
    ArtifactDeletionIdentity, ArtifactDeletionKind, ArtifactDeletionRequest, ArtifactErrorKind,
    ArtifactIngestRequest, CommitIntent, DomainSpec, HeldEvidence, KernelStore,
    MAX_MEMORY_REVIEWER_HOLD_REFERENCES, MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS,
    MEMORY_REVIEWER_EXECUTION_HOLD_KIND, MEMORY_REVIEWER_REVIEW_HOLD_KIND,
    MemoryReviewerHoldBinding, MemoryReviewerHoldError, MemoryReviewerHoldKind,
    MemoryReviewerHoldRefusal, ProviderEgress, REVIEW_EXPIRY_MAX_MS, ReviewBinding, ReviewOwner,
    ReviewPayload, ReviewProposal, ReviewStagingSpec, RunBufferMap, RunBufferRefusal, Sensitivity,
    SourceDependency, StagingTerminalState, provisional_result_identity,
};
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};

const HOUR_MS: i64 = 60 * 60 * 1_000;
const DAY_MS: i64 = 24 * HOUR_MS;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .try_into()
        .unwrap()
}

fn intent(key: &str, payload: &[u8]) -> CommitIntent {
    CommitIntent {
        producer: "kernel-memory_reviewer-hold-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(payload)),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

fn inspect<T>(root: &std::path::Path, read: impl FnOnce(&Connection) -> T) -> T {
    let conn =
        Connection::open_with_flags(root.join("kernel.sqlite"), OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    read(&conn)
}

fn mutate(root: &std::path::Path, sql: &str, params: impl rusqlite::Params) {
    let conn = Connection::open(root.join("kernel.sqlite")).unwrap();
    conn.execute(sql, params).unwrap();
}

fn incarnation(root: &std::path::Path) -> String {
    inspect(root, |conn| {
        conn.query_row(
            "SELECT database_incarnation_id FROM kernel_format_marker",
            [],
            |row| row.get(0),
        )
        .unwrap()
    })
}

struct Fixture {
    directory: tempfile::TempDir,
    store: KernelStore,
}

impl Fixture {
    fn open() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let store = KernelStore::open(directory.path()).unwrap();
        store
            .commit(intent("domain", b"domain"), |envelope| {
                envelope.insert_domain(DomainSpec {
                    domain_id: "domain".to_string(),
                    object_id: "domain-object".to_string(),
                    name: "fixture".to_string(),
                    source_kind: "fixture".to_string(),
                    source_id: "domain".to_string(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                })?;
                Ok("domain".to_string())
            })
            .unwrap();
        Self { directory, store }
    }

    fn root(&self) -> &std::path::Path {
        self.directory.path()
    }

    fn binding(&self, subject: &str, generation: u64) -> MemoryReviewerHoldBinding {
        MemoryReviewerHoldBinding {
            project_digest: "0a".repeat(32),
            kernel_incarnation: incarnation(self.root()),
            memstore_incarnation: "m".repeat(32),
            subject: subject.to_string(),
            generation,
        }
    }

    /// Ingests `payload` as canonical evidence, or as a MemoryReviewer capture with `retain_until` when given.
    fn ingest(&self, key: &str, payload: &[u8], retain_until: Option<i64>) -> String {
        let memory_reviewer = retain_until.is_some();
        self.store
            .ingest_artifact(ArtifactIngestRequest {
                intent: intent(key, payload),
                payload: payload.to_vec(),
                evidence_id: format!("evidence-{key}"),
                object_id: format!("evidence-object-{key}"),
                object_kind: "evidence".to_string(),
                domain_id: "domain".to_string(),
                source_kind: if memory_reviewer {
                    "local_file"
                } else {
                    "repository"
                }
                .to_string(),
                source_id: format!("src/{key}"),
                source_revision: 1,
                media_type: "text/plain".to_string(),
                retention_class: if memory_reviewer {
                    MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS.to_string()
                } else {
                    "canonical".to_string()
                },
                retain_until,
                asserted_sensitivity: Sensitivity::Sensitive,
                provider_egress: ProviderEgress::LocalOnly,
                provenance: None,
            })
            .unwrap()
            .evidence_id
    }

    fn pin(&self, hold_id: &str) -> (String, String, Option<i64>, Option<i64>, i64) {
        inspect(self.root(), |conn| {
            conn.query_row(
                "SELECT pin_kind,owner_id,expires_at,released_at,
                        (SELECT COUNT(*) FROM capture_pin_refs r
                         WHERE r.capture_pin_id=p.capture_pin_id AND r.released_at IS NULL)
                 FROM capture_pins p WHERE capture_pin_id=?1",
                [hold_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap()
        })
    }

    fn pins(&self) -> i64 {
        inspect(self.root(), |conn| {
            conn.query_row("SELECT COUNT(*) FROM capture_pins", [], |row| row.get(0))
                .unwrap()
        })
    }

    fn review_binding(&self, job_id: &str, generation: u64) -> ReviewBinding {
        ReviewBinding {
            project_digest: "0a".repeat(32),
            domain_id: "domain".to_string(),
            owner: ReviewOwner::Proposal {
                job_id: job_id.to_string(),
                generation,
            },
            subject_source: SourceDependency {
                source_kind: "conversation".to_string(),
                source_id: "session-1".to_string(),
                source_revision: 1,
            },
            reference_sources: vec![],
        }
    }

    fn proposal_payload(&self) -> ReviewPayload {
        self.proposal_payload_disclosing(&[])
    }

    /// A retain proposal whose policy dependencies disclose (and leave uncited) `disclosed`.
    fn proposal_payload_disclosing(&self, disclosed: &[String]) -> ReviewPayload {
        let disclosed: Vec<kernel::EvidenceReference> = disclosed
            .iter()
            .map(|evidence_id| kernel::EvidenceReference {
                evidence_id: evidence_id.clone(),
                span: None,
            })
            .collect();
        ReviewPayload::Proposal(Box::new(ReviewProposal {
            action: kernel::ProposalAction::Retain,
            target: kernel::ProposalTarget::Memory(kernel::CanonicalTarget {
                object_id: "mem-1".to_string(),
                source_revision: 1,
                known_as_of: 1,
                commit_token: 1,
            }),
            new_text: None,
            support: vec![],
            contradictions: vec![],
            limitations: vec![],
            uncertainty: kernel::Uncertainty::Low,
            manifest: kernel::ManifestReference {
                manifest_id: "manifest-1".to_string(),
                digest: "d".repeat(64),
            },
            policy_dependencies: kernel::PolicyDependencies {
                question_template: kernel::ReviewQuestionTemplate::ExtractedFacts,
                disclosed_inputs: disclosed.clone(),
                uncited_disclosed_inputs: disclosed,
                ancestry: vec![],
            },
        }))
    }

    fn proposal_digest(&self) -> String {
        self.proposal_payload().digest().unwrap()
    }

    /// Stages and seals this generation's provisional proposal; returns its `created_at`.
    fn stage_proposal(
        &self,
        job_id: &str,
        generation: u64,
        recorded_at: i64,
        deadline: i64,
    ) -> i64 {
        self.stage_proposal_with(
            job_id,
            generation,
            recorded_at,
            deadline,
            self.proposal_payload(),
        )
    }

    fn stage_proposal_with(
        &self,
        job_id: &str,
        generation: u64,
        recorded_at: i64,
        deadline: i64,
        payload: ReviewPayload,
    ) -> i64 {
        let identity = provisional_result_identity(job_id, generation);
        self.store
            .stage_review_input(ReviewStagingSpec {
                extraction_run_id: identity.extraction_run_id.clone(),
                candidate_id: identity.candidate_id,
                producer: "memory_reviewer".to_string(),
                binding: self.review_binding(job_id, generation),
                payload,
                recorded_at,
                queue_deadline_at: deadline,
            })
            .unwrap();
        self.store
            .finish_staging_run(
                &identity.extraction_run_id,
                StagingTerminalState::Completed,
                recorded_at + 1,
            )
            .unwrap();
        recorded_at
    }

    fn retain_until(&self, evidence_id: &str) -> Option<i64> {
        inspect(self.root(), |conn| {
            conn.query_row(
                "SELECT retain_until FROM evidence_meta WHERE evidence_id=?1",
                [evidence_id],
                |row| row.get(0),
            )
            .unwrap()
        })
    }

    fn retention_class(&self, evidence_id: &str) -> String {
        inspect(self.root(), |conn| {
            conn.query_row(
                "SELECT retention_class FROM evidence_meta WHERE evidence_id=?1",
                [evidence_id],
                |row| row.get(0),
            )
            .unwrap()
        })
    }

    fn invalidated(&self, evidence_id: &str) -> bool {
        inspect(self.root(), |conn| {
            conn.query_row(
                "SELECT invalidated_commit_seq IS NOT NULL FROM evidence_meta WHERE evidence_id=?1",
                [evidence_id],
                |row| row.get(0),
            )
            .unwrap()
        })
    }
}

fn refusal(error: MemoryReviewerHoldError) -> MemoryReviewerHoldRefusal {
    match error {
        MemoryReviewerHoldError::Refused(refusal) => refusal,
        MemoryReviewerHoldError::Store(error) => {
            panic!("store error instead of refusal: {error:?}")
        }
    }
}

#[test]
fn acquisition_is_atomic_and_bounded_and_reads_back_after_reopen() {
    let fixture = Fixture::open();
    let now = now_ms();
    let first = fixture.ingest("a", b"alpha bytes", None);
    let second = fixture.ingest("b", b"beta bytes!!", None);
    let binding = fixture.binding("job-1", 1);
    let hold = fixture
        .store
        .acquire_execution_hold(
            &binding,
            &[first.clone(), second.clone()],
            now + 2 * HOUR_MS,
        )
        .unwrap();
    assert_eq!(hold.kind, MemoryReviewerHoldKind::Execution);
    assert_eq!(hold.references, 2);
    assert_eq!(hold.backing_bytes, 23);
    let (kind, owner, expires_at, released_at, refs) = fixture.pin(&hold.hold_id);
    assert_eq!(kind, MEMORY_REVIEWER_EXECUTION_HOLD_KIND);
    assert_eq!(
        owner,
        format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}job-1\u{1f}1",
            "0a".repeat(32),
            incarnation(fixture.root()),
            "m".repeat(32)
        ),
        "the project digest leads so a project's holds are one prefix range"
    );
    assert_eq!(
        (expires_at, released_at, refs),
        (Some(now + 2 * HOUR_MS), None, 2)
    );

    // One unknown id refuses the whole batch and writes no pin.
    let pins_before = fixture.pins();
    assert_eq!(
        refusal(
            fixture
                .store
                .acquire_execution_hold(
                    &fixture.binding("job-2", 1),
                    &[first.clone(), "evidence-missing".to_string()],
                    now + HOUR_MS
                )
                .unwrap_err()
        ),
        MemoryReviewerHoldRefusal::UnavailableEvidence
    );
    assert_eq!(fixture.pins(), pins_before);
    let too_many: Vec<String> = (0..=MAX_MEMORY_REVIEWER_HOLD_REFERENCES)
        .map(|index| format!("evidence-{index}"))
        .collect();
    assert_eq!(
        refusal(
            fixture
                .store
                .acquire_execution_hold(&fixture.binding("job-2", 1), &too_many, now + HOUR_MS)
                .unwrap_err()
        ),
        MemoryReviewerHoldRefusal::TooManyReferences
    );
    assert_eq!(fixture.pins(), pins_before);

    // The hold survives a process restart and validates its evidence set.
    let Fixture { directory, store } = fixture;
    drop(store);
    let store = KernelStore::open(directory.path()).unwrap();
    let fixture = Fixture { directory, store };
    let facts = fixture
        .store
        .validate_held_evidence(
            &hold.hold_id,
            MemoryReviewerHoldKind::Execution,
            &binding,
            &[second.clone(), first.clone()],
            now,
        )
        .unwrap();
    assert_eq!(facts.len(), 2);
    assert_eq!(facts[0].evidence_id, second);
    assert_eq!(facts[0].byte_length, 12);
    assert_eq!(facts[0].sensitivity, Sensitivity::Sensitive);
    assert_eq!(
        refusal(
            fixture
                .store
                .validate_held_evidence(
                    &hold.hold_id,
                    MemoryReviewerHoldKind::Execution,
                    &binding,
                    &[fixture.ingest("c", b"gamma", None)],
                    now,
                )
                .unwrap_err()
        ),
        MemoryReviewerHoldRefusal::NotCovered
    );
    assert_eq!(
        refusal(
            fixture
                .store
                .validate_held_evidence(
                    &hold.hold_id,
                    MemoryReviewerHoldKind::Execution,
                    &binding,
                    std::slice::from_ref(&first),
                    now + 2 * HOUR_MS,
                )
                .unwrap_err()
        ),
        MemoryReviewerHoldRefusal::Expired
    );
    let mut foreign = binding.clone();
    foreign.kernel_incarnation = "f".repeat(32);
    assert_eq!(
        refusal(
            fixture
                .store
                .validate_held_evidence(
                    &hold.hold_id,
                    MemoryReviewerHoldKind::Execution,
                    &foreign,
                    &[first],
                    now
                )
                .unwrap_err()
        ),
        MemoryReviewerHoldRefusal::IncarnationMismatch
    );
}

#[test]
fn extension_grows_the_union_without_moving_the_deadline_or_double_charging() {
    let fixture = Fixture::open();
    let now = now_ms();
    let first = fixture.ingest("a", b"alpha bytes", None);
    let shared = fixture.ingest("shared", b"same bytes as later", None);
    let binding = fixture.binding("job-1", 1);
    let hold = fixture
        .store
        .acquire_execution_hold(&binding, std::slice::from_ref(&first), now + HOUR_MS)
        .unwrap();
    let extended = fixture
        .store
        .extend_execution_hold(
            &hold.hold_id,
            &binding,
            &[shared.clone(), first.clone(), shared.clone()],
        )
        .unwrap();
    assert_eq!(extended.references, 2);
    assert_eq!(extended.expires_at, hold.expires_at);
    assert_eq!(extended.backing_bytes, 11 + 19);
    // A second evidence row over identical bytes shares the artifact and is charged once.
    let duplicate_bytes = fixture.ingest("shared-2", b"same bytes as later", None);
    let extended = fixture
        .store
        .extend_execution_hold(&hold.hold_id, &binding, &[duplicate_bytes])
        .unwrap();
    assert_eq!(extended.references, 3);
    assert_eq!(extended.backing_bytes, 11 + 19);
    let (_, _, expires_at, _, refs) = fixture.pin(&hold.hold_id);
    assert_eq!((expires_at, refs), (Some(now + HOUR_MS), 3));
    // A stale generation cannot extend or release the hold.
    let stale = fixture.binding("job-1", 0);
    assert_eq!(
        refusal(
            fixture
                .store
                .extend_execution_hold(&hold.hold_id, &stale, &[first])
                .unwrap_err()
        ),
        MemoryReviewerHoldRefusal::Missing
    );
    assert_eq!(
        refusal(
            fixture
                .store
                .release_execution_hold(&hold.hold_id, &stale)
                .unwrap_err()
        ),
        MemoryReviewerHoldRefusal::Missing
    );
    fixture
        .store
        .release_execution_hold(&hold.hold_id, &binding)
        .unwrap();
    assert_eq!(
        refusal(
            fixture
                .store
                .release_execution_hold(&hold.hold_id, &binding)
                .unwrap_err()
        ),
        MemoryReviewerHoldRefusal::Released
    );
    assert!(fixture.pin(&hold.hold_id).3.is_some());
}

/// A staged-subject job has nothing captured when it starts: the hold is acquired empty, counts toward the project's active holds, keeps its expiry, and grows through extension like any other.
#[test]
fn an_empty_execution_hold_is_admitted_counts_as_active_and_grows_by_extension() {
    let fixture = Fixture::open();
    let now = now_ms();
    let binding = fixture.binding("job-empty", 1);
    let hold = fixture
        .store
        .acquire_execution_hold(&binding, &[], now + HOUR_MS)
        .unwrap();
    assert_eq!((hold.references, hold.backing_bytes), (0, 0));
    assert_eq!(fixture.pins(), 1);
    let (_, _, expires_at, released_at, refs) = fixture.pin(&hold.hold_id);
    assert_eq!(
        (expires_at, released_at, refs),
        (Some(now + HOUR_MS), None, 0)
    );
    let evidence = fixture.ingest("later", b"read during the run", None);
    let extended = fixture
        .store
        .extend_execution_hold(&hold.hold_id, &binding, std::slice::from_ref(&evidence))
        .unwrap();
    assert_eq!(extended.references, 1);
    assert_eq!(extended.expires_at, hold.expires_at);
    assert_eq!(extended.backing_bytes, 19);
    assert_eq!(
        fixture
            .store
            .validate_held_evidence(
                &hold.hold_id,
                MemoryReviewerHoldKind::Execution,
                &binding,
                std::slice::from_ref(&evidence),
                now,
            )
            .unwrap()
            .len(),
        1
    );
    // An expiry already behind the clock is still refused, empty or not.
    assert_eq!(
        refusal(
            fixture
                .store
                .acquire_execution_hold(&fixture.binding("job-late", 1), &[], now - 1)
                .unwrap_err()
        ),
        MemoryReviewerHoldRefusal::InvalidRequest
    );
    fixture
        .store
        .release_execution_hold(&hold.hold_id, &binding)
        .unwrap();
}

#[test]
fn quota_refusal_retains_nothing_and_admits_exactly_at_the_bound() {
    let fixture = Fixture::open();
    let now = now_ms();
    let ten = fixture.ingest("ten", &[b'x'; 10], None);
    let six = fixture.ingest("six", &[b'y'; 6], None);
    let five = fixture.ingest("five", &[b'z'; 5], None);
    let binding = fixture.binding("job-1", 1);
    let hold = fixture
        .store
        .acquire_execution_hold_with_quota_for_test(
            &binding,
            std::slice::from_ref(&ten),
            now + HOUR_MS,
            16,
            1_000,
        )
        .unwrap();
    assert_eq!(hold.backing_bytes, 10);
    // Another job in the same project: exactly at the project bound is admitted, one byte over is refused whole.
    let pins_before = fixture.pins();
    assert_eq!(
        refusal(
            fixture
                .store
                .acquire_execution_hold_with_quota_for_test(
                    &fixture.binding("job-2", 1),
                    &[six.clone(), five.clone()],
                    now + HOUR_MS,
                    16,
                    1_000,
                )
                .unwrap_err()
        ),
        MemoryReviewerHoldRefusal::ProjectBackingExhausted
    );
    assert_eq!(
        fixture.pins(),
        pins_before,
        "a refused acquisition leaves no pin or reference"
    );
    let at_bound = fixture
        .store
        .acquire_execution_hold_with_quota_for_test(
            &fixture.binding("job-2", 1),
            std::slice::from_ref(&six),
            now + HOUR_MS,
            16,
            1_000,
        )
        .unwrap();
    assert_eq!(at_bound.backing_bytes, 6);
    // Another project is charged separately, and the host bound applies across projects.
    let mut other_project = fixture.binding("job-3", 1);
    other_project.project_digest = "0b".repeat(32);
    fixture
        .store
        .acquire_execution_hold_with_quota_for_test(
            &other_project,
            std::slice::from_ref(&five),
            now + HOUR_MS,
            16,
            21,
        )
        .unwrap();
    let mut fourth = fixture.binding("job-4", 1);
    fourth.project_digest = "0c".repeat(32);
    let one = fixture.ingest("one", b"q", None);
    assert_eq!(
        refusal(
            fixture
                .store
                .acquire_execution_hold_with_quota_for_test(&fourth, &[one], now + HOUR_MS, 16, 21)
                .unwrap_err()
        ),
        MemoryReviewerHoldRefusal::HostBackingExhausted
    );
}

#[test]
fn review_transfer_acquires_before_releasing_and_moves_only_live_memory_reviewer_references() {
    let fixture = Fixture::open();
    let now = now_ms();
    let canonical = fixture.ingest("canonical", b"canonical evidence", None);
    let live_capture = fixture.ingest("capture-live", b"captured live", Some(now + HOUR_MS));
    let long_capture = fixture.ingest("capture-long", b"captured long", Some(now + 20 * DAY_MS));
    let execution = fixture.binding("job-1", 1);
    let hold = fixture
        .store
        .acquire_execution_hold(
            &execution,
            &[
                canonical.clone(),
                live_capture.clone(),
                long_capture.clone(),
            ],
            now + HOUR_MS,
        )
        .unwrap();
    // The proposal this job's generation staged: its creation time anchors the review window.
    let created = fixture.stage_proposal("job-1", 1, now - 1_000, now + DAY_MS - 1_000);
    let identity = provisional_result_identity("job-1", 1);
    let review = fixture.binding(&identity.candidate_id, 1);
    let wrong_generation = fixture.binding(&identity.candidate_id, 2);
    // A subject that is not this generation's provisional result, or a window past seven days, is refused whole.
    for (binding, expires_at) in [
        (&wrong_generation, created + REVIEW_EXPIRY_MAX_MS),
        (&review, created + REVIEW_EXPIRY_MAX_MS + 1),
    ] {
        assert_eq!(
            refusal(
                fixture
                    .store
                    .transfer_execution_to_review(&hold.hold_id, &execution, binding, expires_at)
                    .unwrap_err()
            ),
            MemoryReviewerHoldRefusal::InvalidRequest
        );
    }
    assert!(fixture.pin(&hold.hold_id).3.is_none());
    let review_expires_at = created + REVIEW_EXPIRY_MAX_MS;
    let review_hold = fixture
        .store
        .transfer_execution_to_review(&hold.hold_id, &execution, &review, review_expires_at)
        .unwrap();
    assert_eq!(review_hold.kind, MemoryReviewerHoldKind::Review);
    assert_eq!(review_hold.references, 3);
    assert_eq!(review_hold.expires_at, review_expires_at);
    let (kind, _, expires_at, released_at, refs) = fixture.pin(&review_hold.hold_id);
    assert_eq!(kind, MEMORY_REVIEWER_REVIEW_HOLD_KIND);
    assert_eq!(
        (expires_at, released_at, refs),
        (Some(review_expires_at), None, 3)
    );
    let (_, _, _, execution_released, execution_refs) = fixture.pin(&hold.hold_id);
    assert!(execution_released.is_some());
    assert_eq!(execution_refs, 0);
    // The review binding alone resolves the hold; the execution binding, another generation, and a clock past expiry resolve nothing.
    assert_eq!(
        fixture.store.lookup_review_hold(&review, now).unwrap(),
        Some(review_hold.clone())
    );
    assert_eq!(
        fixture.store.lookup_review_hold(&execution, now).unwrap(),
        None
    );
    assert_eq!(
        fixture
            .store
            .lookup_review_hold(&fixture.binding(&review.subject, 2), now)
            .unwrap(),
        None
    );
    assert_eq!(
        fixture
            .store
            .lookup_review_hold(&review, review_expires_at)
            .unwrap(),
        None
    );
    assert_eq!(
        fixture.retain_until(&live_capture),
        Some(review_expires_at),
        "a live MemoryReviewer acquisition reference moves to the review expiry"
    );
    assert_eq!(
        fixture.retain_until(&long_capture),
        Some(now + 20 * DAY_MS),
        "a reference already retained past the review window is never shortened"
    );
    assert_eq!(
        fixture.retain_until(&canonical),
        None,
        "independently owned evidence keeps its own retention"
    );
    // Q27: the transfer proves retention, not selection. The staged proposal keeps the job's queue deadline on both its candidate and run rows, so an unselected result expires with its queue while the hold outlives it.
    let queue_deadline = now + DAY_MS - 1_000;
    let (candidate_deadline, run_deadline): (i64, i64) = inspect(fixture.root(), |conn| {
        conn.query_row(
            "SELECT c.lease_expires_at,r.lease_expires_at FROM candidates c
             JOIN extraction_runs r USING(extraction_run_id) WHERE c.candidate_id=?1",
            [identity.candidate_id.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
    });
    assert_eq!(
        (candidate_deadline, run_deadline),
        (queue_deadline, queue_deadline)
    );
    let reference = kernel::ReviewStagedReference {
        database_incarnation_id: incarnation(fixture.root()),
        candidate_id: identity.candidate_id.clone(),
        payload_digest: fixture.proposal_digest(),
    };
    // A live read past the queue deadline is expired even though the review hold is live for seven days.
    assert_eq!(
        fixture
            .store
            .read_review_input(
                &reference,
                &fixture.review_binding("job-1", 1),
                queue_deadline
            )
            .unwrap_err(),
        kernel::ReviewReadError::Refused(kernel::ReviewReadRefusal::Expired)
    );
    // A selected read judges the row against the selection time: a selection inside the queue window reads, a selection at or after the deadline does not, whatever the clock says now.
    let row = fixture
        .store
        .read_selected_review_input(&reference, queue_deadline - 1)
        .unwrap();
    assert_eq!(row.lifecycle.queue_deadline_at, queue_deadline);
    assert_eq!(row.binding, fixture.review_binding("job-1", 1));
    assert_eq!(
        fixture
            .store
            .read_selected_review_input(&reference, queue_deadline)
            .unwrap_err(),
        kernel::ReviewReadError::Refused(kernel::ReviewReadRefusal::Expired)
    );
    // A review hold cannot grow; the execution binding cannot release it; the review binding can, once.
    assert_eq!(
        refusal(
            fixture
                .store
                .extend_execution_hold(
                    &review_hold.hold_id,
                    &review,
                    std::slice::from_ref(&canonical)
                )
                .unwrap_err()
        ),
        MemoryReviewerHoldRefusal::Missing
    );
    assert_eq!(
        refusal(
            fixture
                .store
                .release_review_hold(&review_hold.hold_id, &execution)
                .unwrap_err()
        ),
        MemoryReviewerHoldRefusal::Missing
    );
    // A retried transfer whose first result was lost gets the committed review hold back, and writes nothing.
    let pins_before = fixture.pins();
    let retried = fixture
        .store
        .transfer_execution_to_review(&hold.hold_id, &execution, &review, review_expires_at)
        .unwrap();
    assert_eq!(retried.hold_id, review_hold.hold_id);
    assert_eq!(retried.expires_at, review_expires_at);
    assert_eq!(fixture.pins(), pins_before);
    fixture
        .store
        .release_review_hold(&review_hold.hold_id, &review)
        .unwrap();
    assert!(fixture.pin(&review_hold.hold_id).3.is_some());
    assert_eq!(
        fixture.store.lookup_review_hold(&review, now).unwrap(),
        None
    );
    assert_eq!(
        refusal(
            fixture
                .store
                .transfer_execution_to_review(&hold.hold_id, &execution, &review, review_expires_at)
                .unwrap_err()
        ),
        MemoryReviewerHoldRefusal::Released,
        "once the review hold is gone too, the released execution hold cannot be transferred again"
    );
}

/// A live read judges the row's deadline against the store clock as read, not as the call began: a wait for a reader connection that crosses the queue deadline refuses `Expired` instead of returning the Sensitive payload late. A selected read keeps its selection time.
#[test]
fn a_live_read_that_waits_for_a_reader_across_the_queue_deadline_is_expired() {
    let fixture = Fixture::open();
    let now = now_ms();
    let deadline = now + 1_500;
    fixture.stage_proposal("job-1", 1, now, deadline);
    let identity = provisional_result_identity("job-1", 1);
    let reference = kernel::ReviewStagedReference {
        database_incarnation_id: incarnation(fixture.root()),
        candidate_id: identity.candidate_id.clone(),
        payload_digest: fixture.proposal_digest(),
    };
    let binding = fixture.review_binding("job-1", 1);
    let held = std::sync::Barrier::new(2);
    let (live, selected) = std::thread::scope(|scope| {
        scope.spawn(|| {
            fixture
                .store
                .hold_readers_for_test(&held, std::time::Duration::from_millis(3_000));
        });
        held.wait();
        // Both calls start before the deadline and acquire a reader only after it has passed.
        let live = fixture.store.read_review_input(&reference, &binding, now);
        let selected = fixture
            .store
            .read_selected_review_input(&reference, deadline - 1);
        (live, selected)
    });
    assert!(
        now_ms() >= deadline,
        "the reader hold outlasted the deadline"
    );
    assert_eq!(
        live.unwrap_err(),
        kernel::ReviewReadError::Refused(kernel::ReviewReadRefusal::Expired)
    );
    assert_eq!(selected.unwrap().lifecycle.queue_deadline_at, deadline);
}

#[test]
fn review_transfer_requires_the_execution_generation_and_a_live_proposal_deadline() {
    let fixture = Fixture::open();
    let now = now_ms();
    let evidence = fixture.ingest("held", b"held bytes", None);
    let stale = fixture.binding("job-1", 1);
    let stale_hold = fixture
        .store
        .acquire_execution_hold(&stale, std::slice::from_ref(&evidence), now + HOUR_MS)
        .unwrap();
    // A successor generation staged its own proposal; the stale worker's hold cannot become its review hold.
    let created = fixture.stage_proposal("job-1", 2, now - 1_000, now + DAY_MS - 1_000);
    let successor = provisional_result_identity("job-1", 2);
    let successor_review = fixture.binding(&successor.candidate_id, 2);
    let pins_before = fixture.pins();
    assert_eq!(
        refusal(
            fixture
                .store
                .transfer_execution_to_review(
                    &stale_hold.hold_id,
                    &stale,
                    &successor_review,
                    created + REVIEW_EXPIRY_MAX_MS,
                )
                .unwrap_err()
        ),
        MemoryReviewerHoldRefusal::InvalidRequest,
        "the review subject must be the execution generation's own provisional result"
    );
    assert_eq!(
        fixture.pins(),
        pins_before,
        "a refused transfer writes no review pin"
    );
    assert!(
        fixture.pin(&stale_hold.hold_id).3.is_none(),
        "a refused transfer leaves the execution hold live"
    );
    let deadline: i64 = inspect(fixture.root(), |conn| {
        conn.query_row(
            "SELECT lease_expires_at FROM candidates WHERE candidate_id=?1",
            [successor.candidate_id.as_str()],
            |row| row.get(0),
        )
        .unwrap()
    });
    assert_eq!(
        deadline,
        now + DAY_MS - 1_000,
        "a refused transfer moves no proposal deadline"
    );

    // The matching proposal's lease has expired, so no review hold may pin its bytes.
    let execution = fixture.binding("job-2", 1);
    let hold = fixture
        .store
        .acquire_execution_hold(&execution, std::slice::from_ref(&evidence), now + HOUR_MS)
        .unwrap();
    let created = fixture.stage_proposal("job-2", 1, now - 1_000, now + DAY_MS - 1_000);
    let identity = provisional_result_identity("job-2", 1);
    let review = fixture.binding(&identity.candidate_id, 1);
    mutate(
        fixture.root(),
        "UPDATE candidates SET lease_expires_at=?1 WHERE candidate_id=?2",
        rusqlite::params![now - 1, identity.candidate_id],
    );
    mutate(
        fixture.root(),
        "UPDATE extraction_runs SET lease_expires_at=?1 WHERE extraction_run_id=?2",
        rusqlite::params![now - 1, identity.extraction_run_id],
    );
    let pins_before = fixture.pins();
    assert_eq!(
        refusal(
            fixture
                .store
                .transfer_execution_to_review(
                    &hold.hold_id,
                    &execution,
                    &review,
                    created + REVIEW_EXPIRY_MAX_MS,
                )
                .unwrap_err()
        ),
        MemoryReviewerHoldRefusal::InvalidRequest,
        "an expired proposal cannot anchor a review hold"
    );
    assert_eq!(fixture.pins(), pins_before);
    assert!(fixture.pin(&hold.hold_id).3.is_none());
    assert_eq!(
        fixture.retain_until(&evidence),
        None,
        "a refused transfer moves no retention"
    );
}

#[test]
fn expiry_purge_and_reclamation_follow_the_capture_pin_contract() {
    let fixture = Fixture::open();
    let now = now_ms();
    let payload = b"reclaimable evidence bytes";
    let digest = format!("{:x}", Sha256::digest(payload));
    let evidence = fixture.ingest("gc", payload, None);
    let binding = fixture.binding("job-1", 1);
    let hold = fixture
        .store
        .acquire_execution_hold(&binding, std::slice::from_ref(&evidence), now + 30 * DAY_MS)
        .unwrap();
    // Retire the evidence so only the hold keeps the bytes past the invalidation grace.
    fixture
        .store
        .commit(intent("retire", b"retire"), |envelope| {
            envelope.retire_evidence("evidence-object-gc")?;
            Ok(String::new())
        })
        .unwrap();
    let past_grace = now + 15 * DAY_MS;
    let swept = fixture.store.run_staging_maintenance(past_grace).unwrap();
    assert_eq!(
        swept.artifact_gc.reclaimed_objects, 0,
        "an active hold retains bytes whose evidence is already invalidated"
    );
    assert_eq!(
        refusal(
            fixture
                .store
                .validate_held_evidence(
                    &hold.hold_id,
                    MemoryReviewerHoldKind::Execution,
                    &binding,
                    std::slice::from_ref(&evidence),
                    now
                )
                .unwrap_err()
        ),
        MemoryReviewerHoldRefusal::UnavailableEvidence,
        "protecting bytes is not evidence liveness"
    );
    fixture
        .store
        .release_execution_hold(&hold.hold_id, &binding)
        .unwrap();
    let swept = fixture.store.run_staging_maintenance(past_grace).unwrap();
    assert_eq!(
        swept.artifact_gc.reclaimed_objects, 1,
        "release starts the reclaim grace"
    );
    assert!(
        !fixture
            .root()
            .join("artifacts/objects")
            .join(&digest[..2])
            .join(&digest[2..])
            .exists()
    );

    // Fixed expiry releases a hold through capture-pin maintenance without a caller.
    let short = fixture.ingest("short", b"short-lived hold", None);
    let binding = fixture.binding("job-2", 1);
    let hold = fixture
        .store
        .acquire_execution_hold(&binding, &[short], now + HOUR_MS)
        .unwrap();
    fixture
        .store
        .run_capture_pin_maintenance(now + HOUR_MS)
        .unwrap();
    assert_eq!(fixture.pin(&hold.hold_id).3, Some(now + HOUR_MS));
    assert_eq!(
        refusal(
            fixture
                .store
                .release_execution_hold(&hold.hold_id, &binding)
                .unwrap_err()
        ),
        MemoryReviewerHoldRefusal::Released
    );

    // Purge degrades a live hold and wins over it.
    let payload = b"purged evidence bytes";
    let purged = fixture.ingest("purge", payload, None);
    let binding = fixture.binding("job-3", 1);
    let hold = fixture
        .store
        .acquire_execution_hold(&binding, std::slice::from_ref(&purged), now + HOUR_MS)
        .unwrap();
    fixture
        .store
        .delete_artifact(ArtifactDeletionRequest {
            intent: intent("purge", payload),
            identity: ArtifactDeletionIdentity::Digest(format!("{:x}", Sha256::digest(payload))),
            kind: ArtifactDeletionKind::Purge,
            operator_id: Some("operator".to_string()),
            target_locator: Some("incident://purge".to_string()),
            reason: Some("retired".to_string()),
            deleted_at: now,
        })
        .unwrap();
    assert_eq!(
        refusal(
            fixture
                .store
                .validate_held_evidence(
                    &hold.hold_id,
                    MemoryReviewerHoldKind::Execution,
                    &binding,
                    &[purged],
                    now
                )
                .unwrap_err()
        ),
        MemoryReviewerHoldRefusal::PurgeDegraded
    );
}

#[test]
fn memory_reviewer_captures_must_carry_a_finite_acquisition_reference() {
    let fixture = Fixture::open();
    let error = fixture
        .store
        .ingest_artifact(ArtifactIngestRequest {
            intent: intent("no-deadline", b"capture"),
            payload: b"capture".to_vec(),
            evidence_id: "evidence-no-deadline".to_string(),
            object_id: "evidence-object-no-deadline".to_string(),
            object_kind: "evidence".to_string(),
            domain_id: "domain".to_string(),
            source_kind: "local_file".to_string(),
            source_id: "src/file".to_string(),
            source_revision: 1,
            media_type: "text/plain".to_string(),
            retention_class: MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS.to_string(),
            retain_until: None,
            asserted_sensitivity: Sensitivity::Sensitive,
            provider_egress: ProviderEgress::LocalOnly,
            provenance: None,
        })
        .unwrap_err();
    assert_eq!(error.kind(), ArtifactErrorKind::InvalidInput);
    let stale = fixture
        .store
        .ingest_artifact(ArtifactIngestRequest {
            intent: intent("stale-deadline", b"capture"),
            payload: b"capture".to_vec(),
            evidence_id: "evidence-stale-deadline".to_string(),
            object_id: "evidence-object-stale-deadline".to_string(),
            object_kind: "evidence".to_string(),
            domain_id: "domain".to_string(),
            source_kind: "local_file".to_string(),
            source_id: "src/file".to_string(),
            source_revision: 1,
            media_type: "text/plain".to_string(),
            retention_class: MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS.to_string(),
            retain_until: Some(now_ms() - 1),
            asserted_sensitivity: Sensitivity::Sensitive,
            provider_egress: ProviderEgress::LocalOnly,
            provenance: None,
        })
        .unwrap_err();
    assert_eq!(
        stale.kind(),
        ArtifactErrorKind::InvalidInput,
        "an already-dead reference is refused"
    );
    fixture.ingest("with-deadline", b"capture", Some(now_ms() + HOUR_MS));
}

#[test]
fn run_buffers_load_each_artifact_once_and_refuse_at_the_exact_boundary() {
    let fixture = Fixture::open();
    let now = now_ms();
    let ten = fixture.ingest("ten", &[b'x'; 10], None);
    let ten_again = fixture.ingest("ten-again", &[b'x'; 10], None);
    let six = fixture.ingest("six", b"abcdef", None);
    let five = fixture.ingest("five", b"vwxyz", None);
    let binding = fixture.binding("job-1", 1);
    let hold = fixture
        .store
        .acquire_execution_hold(
            &binding,
            &[ten.clone(), ten_again.clone(), six.clone(), five.clone()],
            now + HOUR_MS,
        )
        .unwrap();
    let facts: Vec<HeldEvidence> = fixture
        .store
        .validate_held_evidence(
            &hold.hold_id,
            MemoryReviewerHoldKind::Execution,
            &binding,
            &[ten, ten_again, six, five],
            now,
        )
        .unwrap();
    let mut buffers = RunBufferMap::new(16);
    let reads_before = fixture.store.verified_object_reads_for_test();
    assert!(buffers.load(&fixture.store, &facts[0]).unwrap());
    assert!(
        !buffers.load(&fixture.store, &facts[1]).unwrap(),
        "one artifact behind two evidence rows loads once"
    );
    assert_eq!(buffers.loaded(), 1);
    assert_eq!(buffers.remaining(), 6);
    assert_eq!(
        fixture.store.verified_object_reads_for_test() - reads_before,
        1
    );
    assert!(buffers.load(&fixture.store, &facts[2]).unwrap());
    assert_eq!(
        buffers.remaining(),
        0,
        "exactly the remaining capacity is admitted"
    );
    assert_eq!(
        buffers.load(&fixture.store, &facts[3]).unwrap_err(),
        RunBufferRefusal::CapacityExhausted
    );
    assert_eq!(
        buffers.loaded(),
        2,
        "a refused artifact is never loaded or retained"
    );
    let (ten_digest, six_digest) = (&facts[0].artifact_digest, &facts[2].artifact_digest);
    assert_eq!(buffers.slice(six_digest, 1..4).unwrap(), b"bcd");
    assert_eq!(buffers.slice(ten_digest, 0..10).unwrap(), &[b'x'; 10]);
    assert_eq!(
        buffers.slice(six_digest, 0..7).unwrap_err(),
        RunBufferRefusal::RangeOutOfBounds
    );
    assert_eq!(
        buffers.slice(&facts[3].artifact_digest, 0..1).unwrap_err(),
        RunBufferRefusal::RangeOutOfBounds
    );
    // Repeated ranges reuse the loaded buffer; no reload happens.
    let reads = fixture.store.verified_object_reads_for_test();
    assert!(!buffers.load(&fixture.store, &facts[2]).unwrap());
    assert_eq!(fixture.store.verified_object_reads_for_test(), reads);
    // An artifact larger than the whole run budget is refused before any read.
    let mut small = RunBufferMap::new(4);
    assert_eq!(
        small.load(&fixture.store, &facts[0]).unwrap_err(),
        RunBufferRefusal::CapacityExhausted
    );
    assert_eq!(fixture.store.verified_object_reads_for_test(), reads);
}

#[test]
fn review_transfer_requires_a_sealed_proposal_row_covered_by_the_review_window() {
    let fixture = Fixture::open();
    let now = now_ms();
    let evidence = fixture.ingest("held", b"held bytes", None);

    // Persisted but not sealed: `read_review_input` would report `Unsealed`, so no review hold may cover it.
    let execution = fixture.binding("job-1", 1);
    let hold = fixture
        .store
        .acquire_execution_hold(&execution, std::slice::from_ref(&evidence), now + HOUR_MS)
        .unwrap();
    let identity = provisional_result_identity("job-1", 1);
    fixture
        .store
        .stage_review_input(ReviewStagingSpec {
            extraction_run_id: identity.extraction_run_id.clone(),
            candidate_id: identity.candidate_id.clone(),
            producer: "memory_reviewer".to_string(),
            binding: fixture.review_binding("job-1", 1),
            payload: fixture.proposal_payload(),
            recorded_at: now - 1_000,
            queue_deadline_at: now + DAY_MS - 1_000,
        })
        .unwrap();
    let review = fixture.binding(&identity.candidate_id, 1);
    let pins_before = fixture.pins();
    assert_eq!(
        refusal(
            fixture
                .store
                .transfer_execution_to_review(
                    &hold.hold_id,
                    &execution,
                    &review,
                    now + REVIEW_EXPIRY_MAX_MS - 1_000
                )
                .unwrap_err()
        ),
        MemoryReviewerHoldRefusal::InvalidRequest,
        "an unsealed proposal cannot anchor a review hold"
    );
    assert_eq!(fixture.pins(), pins_before);
    assert!(fixture.pin(&hold.hold_id).3.is_none());

    // A review expiry before the proposal's queue deadline would leave the proposal readable after its hold lapsed.
    let execution = fixture.binding("job-2", 1);
    let hold = fixture
        .store
        .acquire_execution_hold(&execution, std::slice::from_ref(&evidence), now + HOUR_MS)
        .unwrap();
    let created = fixture.stage_proposal("job-2", 1, now - 1_000, now + DAY_MS - 1_000);
    let identity = provisional_result_identity("job-2", 1);
    let review = fixture.binding(&identity.candidate_id, 1);
    let pins_before = fixture.pins();
    assert_eq!(
        refusal(
            fixture
                .store
                .transfer_execution_to_review(&hold.hold_id, &execution, &review, now + HOUR_MS)
                .unwrap_err()
        ),
        MemoryReviewerHoldRefusal::InvalidRequest,
        "the review window must reach the proposal's queue deadline"
    );
    assert_eq!(fixture.pins(), pins_before);
    assert!(fixture.pin(&hold.hold_id).3.is_none());
    // Exactly at the deadline is covered.
    let review_hold = fixture
        .store
        .transfer_execution_to_review(&hold.hold_id, &execution, &review, now + DAY_MS - 1_000)
        .unwrap();
    assert_eq!(review_hold.expires_at, now + DAY_MS - 1_000);
    assert!(created <= now);

    // A completed row that merely carries the derived candidate id, staged through the public path under another run and kind, is not the proposal.
    let execution = fixture.binding("job-3", 1);
    let hold = fixture
        .store
        .acquire_execution_hold(&execution, std::slice::from_ref(&evidence), now + HOUR_MS)
        .unwrap();
    let identity = provisional_result_identity("job-3", 1);
    fixture
        .store
        .stage_candidate(kernel::StagingCandidateSpec {
            extraction_run_id: "generic-run".to_string(),
            candidate_id: identity.candidate_id.clone(),
            extractor: "extractor".to_string(),
            source_kind: "repository".to_string(),
            source_id: "src/generic".to_string(),
            source_revision: 1,
            candidate_kind: "generic".to_string(),
            payload: "{}".to_string(),
            provenance: None,
            recorded_at: now - 1_000,
            lease_expires_at: now + HOUR_MS - 1_000,
        })
        .unwrap();
    fixture
        .store
        .finish_staging_run("generic-run", StagingTerminalState::Completed, now)
        .unwrap();
    let review = fixture.binding(&identity.candidate_id, 1);
    let pins_before = fixture.pins();
    assert_eq!(
        refusal(
            fixture
                .store
                .transfer_execution_to_review(
                    &hold.hold_id,
                    &execution,
                    &review,
                    now + REVIEW_EXPIRY_MAX_MS - 1_000
                )
                .unwrap_err()
        ),
        MemoryReviewerHoldRefusal::InvalidRequest,
        "only the derived run's review proposal row anchors a review hold"
    );
    assert_eq!(fixture.pins(), pins_before);
    assert!(fixture.pin(&hold.hold_id).3.is_none());
}

#[test]
fn acquisition_reuses_the_live_hold_of_a_binding() {
    let fixture = Fixture::open();
    let now = now_ms();
    let first = fixture.ingest("a", b"alpha bytes", None);
    let second = fixture.ingest("b", b"beta bytes!!", None);
    let binding = fixture.binding("job-1", 1);
    let hold = fixture
        .store
        .acquire_execution_hold(&binding, std::slice::from_ref(&first), now + HOUR_MS)
        .unwrap();
    let pins_before = fixture.pins();
    // A retry whose first response was lost lands on the same hold: same id, same expiry, no second pin.
    let retried = fixture
        .store
        .acquire_execution_hold(&binding, &[first.clone(), second], now + 2 * HOUR_MS)
        .unwrap();
    assert_eq!(retried.hold_id, hold.hold_id);
    assert_eq!(
        retried.expires_at, hold.expires_at,
        "the expiry never moves"
    );
    assert_eq!(retried.references, 2);
    assert_eq!(fixture.pins(), pins_before);
    // Once released, the binding may hold again under a new pin.
    fixture
        .store
        .release_execution_hold(&hold.hold_id, &binding)
        .unwrap();
    let fresh = fixture
        .store
        .acquire_execution_hold(&binding, &[first], now + HOUR_MS)
        .unwrap();
    assert_ne!(fresh.hold_id, hold.hold_id);
    assert_eq!(fixture.pins(), pins_before + 1);
}

#[test]
fn purged_bytes_stop_charging_the_backing_quota() {
    let fixture = Fixture::open();
    let now = now_ms();
    let purged_payload = b"purged evidence bytes";
    let purged = fixture.ingest("purge", purged_payload, None);
    let kept = fixture.ingest("kept", &[b'k'; 10], None);
    let hold = fixture
        .store
        .acquire_execution_hold(&fixture.binding("job-1", 1), &[purged, kept], now + HOUR_MS)
        .unwrap();
    assert_eq!(hold.backing_bytes, 21 + 10);
    fixture
        .store
        .delete_artifact(ArtifactDeletionRequest {
            intent: intent("purge", purged_payload),
            identity: ArtifactDeletionIdentity::Digest(format!(
                "{:x}",
                Sha256::digest(purged_payload)
            )),
            kind: ArtifactDeletionKind::Purge,
            operator_id: Some("operator".to_string()),
            target_locator: Some("incident://purge".to_string()),
            reason: Some("retired".to_string()),
            deleted_at: now,
        })
        .unwrap();
    // The degraded hold still charges its surviving 10 bytes; the purged 21 are gone from the host.
    let next = fixture.ingest("next", &[b'n'; 10], None);
    let admitted = fixture
        .store
        .acquire_execution_hold_with_quota_for_test(
            &fixture.binding("job-2", 1),
            std::slice::from_ref(&next),
            now + HOUR_MS,
            20,
            20,
        )
        .unwrap();
    assert_eq!(admitted.backing_bytes, 10);
}

/// Holds an `IMMEDIATE` transaction on the store file for `hold_ms` while `work` runs, so the store's next write waits on the busy handler.
fn with_writer_blocked<T>(root: &std::path::Path, hold_ms: u64, work: impl FnOnce() -> T) -> T {
    let path = root.join("kernel.sqlite");
    let (ready, wait) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        ready.send(()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(hold_ms));
        conn.execute_batch("COMMIT").unwrap();
    });
    wait.recv().unwrap();
    let result = work();
    holder.join().unwrap();
    result
}

#[test]
fn deadlines_are_judged_by_the_clock_inside_the_write_transaction() {
    let fixture = Fixture::open();
    let evidence = fixture.ingest("held", b"held bytes", None);

    // Acquisition: the expiry passes while the call waits for the writer.
    let binding = fixture.binding("job-1", 1);
    let pins_before = fixture.pins();
    let result = with_writer_blocked(fixture.root(), 400, || {
        fixture.store.acquire_execution_hold(
            &binding,
            std::slice::from_ref(&evidence),
            now_ms() + 100,
        )
    });
    assert_eq!(
        refusal(result.unwrap_err()),
        MemoryReviewerHoldRefusal::InvalidRequest,
        "an expiry that has passed by the time the writer is held is refused"
    );
    assert_eq!(fixture.pins(), pins_before);

    // Transfer: the review expiry passes while the call waits for the writer.
    let execution = fixture.binding("job-2", 1);
    let hold = fixture
        .store
        .acquire_execution_hold(
            &execution,
            std::slice::from_ref(&evidence),
            now_ms() + HOUR_MS,
        )
        .unwrap();
    let now = now_ms();
    fixture.stage_proposal("job-2", 1, now - 1_000, now + 150);
    let identity = provisional_result_identity("job-2", 1);
    let review = fixture.binding(&identity.candidate_id, 1);
    let pins_before = fixture.pins();
    let result = with_writer_blocked(fixture.root(), 400, || {
        fixture
            .store
            .transfer_execution_to_review(&hold.hold_id, &execution, &review, now + 150)
    });
    assert_eq!(
        refusal(result.unwrap_err()),
        MemoryReviewerHoldRefusal::InvalidRequest,
        "a review window that has closed by the time the writer is held is refused"
    );
    assert_eq!(fixture.pins(), pins_before);
    assert!(
        fixture.pin(&hold.hold_id).3.is_none(),
        "the execution hold stays live"
    );
}

#[test]
fn a_transferred_generation_acquires_no_further_execution_hold() {
    let fixture = Fixture::open();
    let now = now_ms();
    let evidence = fixture.ingest("held", b"held bytes", None);
    let execution = fixture.binding("job-1", 1);
    let hold = fixture
        .store
        .acquire_execution_hold(&execution, std::slice::from_ref(&evidence), now + HOUR_MS)
        .unwrap();
    let created = fixture.stage_proposal("job-1", 1, now - 1_000, now + DAY_MS - 1_000);
    let identity = provisional_result_identity("job-1", 1);
    let review = fixture.binding(&identity.candidate_id, 1);
    let review_hold = fixture
        .store
        .transfer_execution_to_review(
            &hold.hold_id,
            &execution,
            &review,
            created + REVIEW_EXPIRY_MAX_MS,
        )
        .unwrap();
    // A late acquisition retry for the same generation cannot open a second, replacement execution hold.
    let pins_before = fixture.pins();
    assert_eq!(
        refusal(
            fixture
                .store
                .acquire_execution_hold(&execution, std::slice::from_ref(&evidence), now + HOUR_MS)
                .unwrap_err()
        ),
        MemoryReviewerHoldRefusal::InvalidRequest,
        "the generation's bytes are already under its review hold"
    );
    assert_eq!(fixture.pins(), pins_before);
    // The successor generation is unaffected.
    fixture
        .store
        .acquire_execution_hold(
            &fixture.binding("job-1", 2),
            std::slice::from_ref(&evidence),
            now + HOUR_MS,
        )
        .unwrap();
    fixture
        .store
        .release_review_hold(&review_hold.hold_id, &review)
        .unwrap();
    // The generation's proposal is terminal; even after its review hold is gone, no execution hold reopens it.
    let pins_before = fixture.pins();
    assert_eq!(
        refusal(
            fixture
                .store
                .acquire_execution_hold(&execution, std::slice::from_ref(&evidence), now + HOUR_MS)
                .unwrap_err()
        ),
        MemoryReviewerHoldRefusal::InvalidRequest,
        "a transferred generation never acquires again"
    );
    assert_eq!(fixture.pins(), pins_before);
}

#[test]
fn review_transfer_requires_the_hold_to_cover_every_disclosed_input() {
    let fixture = Fixture::open();
    let now = now_ms();
    let held = fixture.ingest("held", b"held bytes", None);
    let unheld = fixture.ingest("unheld", b"never held", None);
    let execution = fixture.binding("job-1", 1);
    let hold = fixture
        .store
        .acquire_execution_hold(&execution, std::slice::from_ref(&held), now + HOUR_MS)
        .unwrap();
    let created = fixture.stage_proposal_with(
        "job-1",
        1,
        now - 1_000,
        now + DAY_MS - 1_000,
        fixture.proposal_payload_disclosing(&[held.clone(), unheld.clone()]),
    );
    let identity = provisional_result_identity("job-1", 1);
    let review = fixture.binding(&identity.candidate_id, 1);
    let pins_before = fixture.pins();
    assert_eq!(
        refusal(
            fixture
                .store
                .transfer_execution_to_review(
                    &hold.hold_id,
                    &execution,
                    &review,
                    created + REVIEW_EXPIRY_MAX_MS,
                )
                .unwrap_err()
        ),
        MemoryReviewerHoldRefusal::NotCovered,
        "a disclosed input outside the execution hold refuses the transfer"
    );
    assert_eq!(fixture.pins(), pins_before);
    assert!(fixture.pin(&hold.hold_id).3.is_none());
    // Extending the hold over the missing input makes the same transfer admissible.
    fixture
        .store
        .extend_execution_hold(&hold.hold_id, &execution, std::slice::from_ref(&unheld))
        .unwrap();
    let review_hold = fixture
        .store
        .transfer_execution_to_review(
            &hold.hold_id,
            &execution,
            &review,
            created + REVIEW_EXPIRY_MAX_MS,
        )
        .unwrap();
    assert_eq!(review_hold.references, 2);
}

#[test]
fn acquisition_retry_recovers_the_hold_after_a_covered_row_is_invalidated() {
    let fixture = Fixture::open();
    let now = now_ms();
    let first = fixture.ingest("a", b"alpha bytes", None);
    let second = fixture.ingest("b", b"beta bytes!!", None);
    let binding = fixture.binding("job-1", 1);
    let hold = fixture
        .store
        .acquire_execution_hold(&binding, &[first.clone(), second.clone()], now + HOUR_MS)
        .unwrap();
    fixture
        .store
        .commit(intent("retire", b"retire"), |envelope| {
            envelope.retire_evidence("evidence-object-a")?;
            Ok(String::new())
        })
        .unwrap();
    // The identical retry recovers the committed hold although one of its rows is no longer live.
    let retried = fixture
        .store
        .acquire_execution_hold(&binding, &[first, second], now + HOUR_MS)
        .unwrap();
    assert_eq!(retried.hold_id, hold.hold_id);
    assert_eq!(retried.references, 2);
    // A retry that adds an unavailable id is still refused.
    assert_eq!(
        refusal(
            fixture
                .store
                .acquire_execution_hold(&binding, &["evidence-missing".to_string()], now + HOUR_MS)
                .unwrap_err()
        ),
        MemoryReviewerHoldRefusal::UnavailableEvidence
    );
}

#[test]
fn a_transfer_retry_recovers_a_purge_degraded_review_hold() {
    let fixture = Fixture::open();
    let now = now_ms();
    let payload = b"purged during review";
    let purged = fixture.ingest("purge", payload, None);
    let execution = fixture.binding("job-1", 1);
    let hold = fixture
        .store
        .acquire_execution_hold(&execution, std::slice::from_ref(&purged), now + HOUR_MS)
        .unwrap();
    let created = fixture.stage_proposal("job-1", 1, now - 1_000, now + DAY_MS - 1_000);
    let identity = provisional_result_identity("job-1", 1);
    let review = fixture.binding(&identity.candidate_id, 1);
    let review_expires_at = created + REVIEW_EXPIRY_MAX_MS;
    let review_hold = fixture
        .store
        .transfer_execution_to_review(&hold.hold_id, &execution, &review, review_expires_at)
        .unwrap();
    fixture
        .store
        .delete_artifact(ArtifactDeletionRequest {
            intent: intent("purge", payload),
            identity: ArtifactDeletionIdentity::Digest(format!("{:x}", Sha256::digest(payload))),
            kind: ArtifactDeletionKind::Purge,
            operator_id: Some("operator".to_string()),
            target_locator: Some("incident://purge".to_string()),
            reason: Some("retired".to_string()),
            deleted_at: now,
        })
        .unwrap();
    // The retry still learns the committed hold's id, so the degraded pin can be released rather than left to expire.
    let retried = fixture
        .store
        .transfer_execution_to_review(&hold.hold_id, &execution, &review, review_expires_at)
        .unwrap();
    assert_eq!(retried.hold_id, review_hold.hold_id);
    // So does a settlement that recovers by lookup: the degraded pin is still the binding's committed hold, and validation under it is what refuses.
    assert_eq!(
        fixture
            .store
            .lookup_review_hold(&review, now)
            .unwrap()
            .map(|found| found.hold_id),
        Some(review_hold.hold_id.clone())
    );
    assert_eq!(
        refusal(
            fixture
                .store
                .validate_held_evidence(
                    &retried.hold_id,
                    MemoryReviewerHoldKind::Review,
                    &review,
                    &[purged],
                    now
                )
                .unwrap_err()
        ),
        MemoryReviewerHoldRefusal::PurgeDegraded
    );
    fixture
        .store
        .release_review_hold(&retried.hold_id, &review)
        .unwrap();
}

#[test]
fn acquisition_bounds_its_input_before_scanning_covered_ids() {
    let fixture = Fixture::open();
    let now = now_ms();
    let evidence = fixture.ingest("a", b"alpha bytes", None);
    let binding = fixture.binding("job-1", 1);
    fixture
        .store
        .acquire_execution_hold(&binding, std::slice::from_ref(&evidence), now + HOUR_MS)
        .unwrap();
    let repeated = vec![evidence; MAX_MEMORY_REVIEWER_HOLD_REFERENCES + 1];
    assert_eq!(
        refusal(
            fixture
                .store
                .acquire_execution_hold(&binding, &repeated, now + HOUR_MS)
                .unwrap_err()
        ),
        MemoryReviewerHoldRefusal::TooManyReferences,
        "an over-long retry is refused before any per-id work"
    );
}

#[test]
fn a_committed_capture_replays_after_its_retention_deadline() {
    let fixture = Fixture::open();
    let request = |retain_until: i64| ArtifactIngestRequest {
        intent: intent("capture-replay", b"capture"),
        payload: b"capture".to_vec(),
        evidence_id: "evidence-capture-replay".to_string(),
        object_id: "evidence-object-capture-replay".to_string(),
        object_kind: "evidence".to_string(),
        domain_id: "domain".to_string(),
        source_kind: "local_file".to_string(),
        source_id: "src/file".to_string(),
        source_revision: 1,
        media_type: "text/plain".to_string(),
        retention_class: MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS.to_string(),
        retain_until: Some(retain_until),
        asserted_sensitivity: Sensitivity::Sensitive,
        provider_egress: ProviderEgress::LocalOnly,
        provenance: None,
    };
    let retain_until = now_ms() + 200;
    let first = fixture
        .store
        .ingest_artifact(request(retain_until))
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    // The identical intent replays its committed receipt even though the reference is now dead; only a new reference needs a live deadline.
    let replayed = fixture
        .store
        .ingest_artifact(request(retain_until))
        .unwrap();
    assert_eq!(replayed, first);
}

#[test]
fn a_degraded_hold_is_recovered_but_gains_no_references() {
    let fixture = Fixture::open();
    let now = now_ms();
    let payload = b"purged before the retry";
    let purged = fixture.ingest("purge", payload, None);
    let extra = fixture.ingest("extra", b"extra bytes", None);
    let binding = fixture.binding("job-1", 1);
    let hold = fixture
        .store
        .acquire_execution_hold(&binding, std::slice::from_ref(&purged), now + HOUR_MS)
        .unwrap();
    fixture
        .store
        .delete_artifact(ArtifactDeletionRequest {
            intent: intent("purge", payload),
            identity: ArtifactDeletionIdentity::Digest(format!("{:x}", Sha256::digest(payload))),
            kind: ArtifactDeletionKind::Purge,
            operator_id: Some("operator".to_string()),
            target_locator: Some("incident://purge".to_string()),
            reason: Some("retired".to_string()),
            deleted_at: now,
        })
        .unwrap();
    // The retry learns the degraded hold's id but attaches nothing new to a pin that can no longer protect anything.
    let retried = fixture
        .store
        .acquire_execution_hold(&binding, &[purged, extra.clone()], now + HOUR_MS)
        .unwrap();
    assert_eq!(retried.hold_id, hold.hold_id);
    assert_eq!(retried.references, 1);
    assert_eq!(fixture.pin(&hold.hold_id).4, 1);
    assert_eq!(
        refusal(
            fixture
                .store
                .validate_held_evidence(
                    &retried.hold_id,
                    MemoryReviewerHoldKind::Execution,
                    &binding,
                    &[extra],
                    now
                )
                .unwrap_err()
        ),
        MemoryReviewerHoldRefusal::PurgeDegraded
    );
    fixture
        .store
        .release_execution_hold(&retried.hold_id, &binding)
        .unwrap();
}

/// The pin kinds and the capture retention class are storage-format text: a store's
/// rows carry them exactly as written, whatever the role is called in code. A hold and
/// a capture stored under that text are still recognized by every reader keyed on it:
/// hold validation and release, and the capture expiry sweep.
#[test]
fn stored_hold_kinds_and_capture_class_are_recognized_by_their_on_disk_text() {
    let fixture = Fixture::open();
    let now = now_ms();
    let evidence = fixture.ingest("stored", b"stored capture", Some(now + HOUR_MS));
    let binding = fixture.binding("job-stored", 1);
    let hold = fixture
        .store
        .acquire_execution_hold(&binding, std::slice::from_ref(&evidence), now + HOUR_MS)
        .unwrap();
    // The rows as an existing store holds them.
    mutate(
        fixture.root(),
        "UPDATE evidence_meta SET retention_class='curator_capture' WHERE evidence_id=?1",
        [&evidence],
    );
    mutate(
        fixture.root(),
        "UPDATE capture_pins SET pin_kind='curator_execution' WHERE capture_pin_id=?1",
        [&hold.hold_id],
    );
    assert_eq!(
        fixture.pin(&hold.hold_id).0,
        MEMORY_REVIEWER_EXECUTION_HOLD_KIND
    );
    assert_eq!(
        fixture.retention_class(&evidence),
        MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS
    );

    let held = fixture
        .store
        .validate_held_evidence(
            &hold.hold_id,
            MemoryReviewerHoldKind::Execution,
            &binding,
            std::slice::from_ref(&evidence),
            now,
        )
        .expect("a stored execution hold validates for its owner");
    assert_eq!(held.len(), 1);
    fixture
        .store
        .release_execution_hold(&hold.hold_id, &binding)
        .expect("a stored execution hold releases for its owner");

    // Once its acquisition reference lapses and nothing pins it, the sweep retires it.
    mutate(
        fixture.root(),
        "UPDATE evidence_meta SET retain_until=?1 WHERE evidence_id=?2",
        rusqlite::params![now - 1, evidence],
    );
    let swept = fixture.store.expire_local_file_captures(now).unwrap();
    assert_eq!(
        swept.retired, 1,
        "a stored capture past its reference is retired"
    );
    assert!(
        fixture.invalidated(&evidence),
        "the retired capture's evidence row is invalidated"
    );

    // A review hold stored under its text is likewise recognized.
    let review_evidence = fixture.ingest("stored-review", b"stored review", Some(now + HOUR_MS));
    let review_binding = fixture.binding("job-stored-review", 1);
    let review = fixture
        .store
        .acquire_execution_hold(
            &review_binding,
            std::slice::from_ref(&review_evidence),
            now + HOUR_MS,
        )
        .unwrap();
    mutate(
        fixture.root(),
        "UPDATE capture_pins SET pin_kind='curator_review' WHERE capture_pin_id=?1",
        [&review.hold_id],
    );
    assert_eq!(
        fixture.pin(&review.hold_id).0,
        MEMORY_REVIEWER_REVIEW_HOLD_KIND
    );
    fixture
        .store
        .release_review_hold(&review.hold_id, &review_binding)
        .expect("a stored review hold releases for its owner");
}
