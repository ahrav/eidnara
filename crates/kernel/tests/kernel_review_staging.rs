#![cfg(feature = "test-support")]

//! Real-store proofs for the review-staging input path: immutable references, replay comparison, decode refusal, scope and digest mismatch, deadline boundaries, and canonical invisibility.

use kernel::{
    CanonicalTarget, EvidenceReference, ExtractedFact, KernelError, KernelStore, ManifestReference,
    PolicyDependencies, ProposalAction, ProposalTarget, REVIEW_QUEUE_LIFETIME_MS, ReviewBinding,
    ReviewOwner, ReviewPayload, ReviewProposal, ReviewQuestionTemplate, ReviewReadError,
    ReviewReadRefusal, ReviewStagedReference, ReviewStagingSpec, ReviewSubject, Sensitivity,
    SourceDependency, SourceSpan, StagingCandidateSpec, StagingTerminalState, Uncertainty,
    provisional_result_identity,
};
use rusqlite::{Connection, OpenFlags};

const HOUR_MS: i64 = 60 * 60 * 1_000;
const DAY_MS: i64 = 24 * HOUR_MS;
const AWS_KEY: &str = "AKIAQ7RSTUVWXYZ23456";

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .try_into()
        .unwrap()
}

fn inspect<T>(root: &std::path::Path, read: impl FnOnce(&Connection) -> T) -> T {
    let conn =
        Connection::open_with_flags(root.join("kernel.sqlite"), OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    read(&conn)
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

fn lifecycle(root: &std::path::Path, candidate_id: &str) -> (i64, i64, Option<String>) {
    inspect(root, |conn| {
        conn.query_row(
            "SELECT heartbeat_at,lease_expires_at,terminal_state FROM candidates WHERE candidate_id=?1",
            [candidate_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap()
    })
}

fn source() -> SourceDependency {
    SourceDependency {
        source_kind: "conversation".to_string(),
        source_id: "session-1".to_string(),
        source_revision: 3,
    }
}

fn binding(owner: ReviewOwner) -> ReviewBinding {
    ReviewBinding {
        project_digest: "p".repeat(64),
        domain_id: "domain-1".to_string(),
        owner,
        source_dependencies: vec![source()],
    }
}

fn job_binding() -> ReviewBinding {
    binding(ReviewOwner::Job {
        job_id: "job-1".to_string(),
    })
}

fn subject(text: &str) -> ReviewPayload {
    ReviewPayload::Subject(ReviewSubject {
        source: source(),
        facts: vec![ExtractedFact {
            text: text.to_string(),
            span: SourceSpan {
                alias: "s1".to_string(),
                start: 0,
                end: 12,
            },
        }],
    })
}

fn subject_spec(run: &str, candidate: &str, recorded_at: i64) -> ReviewStagingSpec {
    ReviewStagingSpec {
        extraction_run_id: run.to_string(),
        candidate_id: candidate.to_string(),
        producer: "history-summarizer".to_string(),
        binding: job_binding(),
        payload: subject("the build uses bun"),
        recorded_at,
        queue_deadline_at: recorded_at + DAY_MS,
    }
}

fn reference(evidence_id: &str) -> EvidenceReference {
    EvidenceReference {
        evidence_id: evidence_id.to_string(),
        span: None,
    }
}

fn proposal(
    action: ProposalAction,
    target: ProposalTarget,
    new_text: Option<&str>,
) -> ReviewProposal {
    ReviewProposal {
        action,
        target,
        new_text: new_text.map(str::to_string),
        support: vec![reference("ev-1")],
        contradictions: vec![],
        limitations: vec!["search incomplete".to_string()],
        uncertainty: Uncertainty::Medium,
        manifest: ManifestReference {
            manifest_id: "manifest-1".to_string(),
            digest: "d".repeat(64),
        },
        policy_dependencies: PolicyDependencies {
            question_template: ReviewQuestionTemplate::ExtractedFacts,
            disclosed_inputs: vec![reference("ev-1"), reference("ev-2")],
            uncited_disclosed_inputs: vec![reference("ev-2")],
            ancestry: vec!["obj-9".to_string()],
        },
    }
}

fn memory_target() -> ProposalTarget {
    ProposalTarget::Memory(CanonicalTarget {
        object_id: "mem-1".to_string(),
        source_revision: 4,
        known_as_of: 10,
        commit_token: 9,
    })
}

fn staged_target() -> ProposalTarget {
    ProposalTarget::StagedCandidate {
        candidate_id: "cand-1".to_string(),
    }
}

fn proposal_spec(job_id: &str, generation: u64, proposal: ReviewProposal) -> ReviewStagingSpec {
    let identity = provisional_result_identity(job_id, generation);
    ReviewStagingSpec {
        extraction_run_id: identity.extraction_run_id,
        candidate_id: identity.candidate_id,
        producer: "curator".to_string(),
        binding: binding(ReviewOwner::Proposal {
            job_id: job_id.to_string(),
            generation,
        }),
        payload: ReviewPayload::Proposal(proposal),
        recorded_at: now_ms(),
        queue_deadline_at: now_ms() + DAY_MS,
    }
}

fn refusal(error: ReviewReadError) -> ReviewReadRefusal {
    match error {
        ReviewReadError::Refused(refusal) => refusal,
        ReviewReadError::Store(error) => panic!("store error instead of refusal: {error:?}"),
    }
}

#[test]
fn exact_replay_returns_the_same_reference_without_refreshing_deadlines() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    let origin = now_ms();
    let first = store
        .stage_review_input(subject_spec("run-1", "subject-1", origin))
        .unwrap();
    assert_eq!(first.database_incarnation_id, incarnation(directory.path()));
    assert_eq!(
        first.payload_digest,
        subject("the build uses bun").digest().unwrap()
    );
    let before = lifecycle(directory.path(), "subject-1");

    let mut later = subject_spec("run-1", "subject-1", origin + 1_000);
    later.queue_deadline_at = origin + 1_000 + DAY_MS;
    let replayed = store.stage_review_input(later).unwrap();
    assert_eq!(replayed, first);
    assert_eq!(
        lifecycle(directory.path(), "subject-1"),
        before,
        "an exact replay leaves heartbeat and deadline untouched"
    );
    let run: (i64, i64) = inspect(directory.path(), |conn| {
        conn.query_row(
            "SELECT heartbeat_at,lease_expires_at FROM extraction_runs WHERE extraction_run_id='run-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
    });
    assert_eq!(run, (origin, origin + DAY_MS));
}

#[test]
fn changed_bytes_or_binding_conflict_at_one_identity() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    let origin = now_ms();
    store
        .stage_review_input(subject_spec("run-1", "subject-1", origin))
        .unwrap();

    let mut changed = subject_spec("run-1", "subject-1", origin);
    changed.payload = subject("the build uses npm");
    assert_eq!(
        store.stage_review_input(changed).unwrap_err(),
        KernelError::Conflict
    );

    let mut rebound = subject_spec("run-1", "subject-2", origin);
    rebound.binding.domain_id = "domain-2".to_string();
    assert_eq!(
        store.stage_review_input(rebound).unwrap_err(),
        KernelError::Conflict,
        "a run's binding is part of its identity"
    );
    let count: i64 = inspect(directory.path(), |conn| {
        conn.query_row("SELECT COUNT(*) FROM candidates", [], |row| row.get(0))
            .unwrap()
    });
    assert_eq!(count, 1);
}

#[test]
fn only_sealed_subjects_read_and_reads_carry_sensitive_classification() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    let origin = now_ms();
    let reference = store
        .stage_review_input(subject_spec("run-1", "subject-1", origin))
        .unwrap();
    assert_eq!(
        refusal(
            store
                .read_review_input(&reference, &job_binding(), origin)
                .unwrap_err()
        ),
        ReviewReadRefusal::Unsealed
    );
    store
        .finish_staging_run("run-1", StagingTerminalState::Completed, origin + 1)
        .unwrap();
    let row = store
        .read_review_input(&reference, &job_binding(), origin + 2)
        .unwrap();
    assert_eq!(row.sensitivity, Sensitivity::Sensitive);
    assert_eq!(row.payload, subject("the build uses bun"));
    assert_eq!(row.binding, job_binding());
    assert_eq!(row.lifecycle.queue_deadline_at, origin + DAY_MS);
    assert_eq!(row.lifecycle.sealed_at, origin + 1);
    // A restage into a sealed run conflicts instead of reopening it.
    assert_eq!(
        store
            .stage_review_input(subject_spec("run-1", "subject-1", origin))
            .unwrap_err(),
        KernelError::Conflict
    );
    // Reads do not move the deadline or heartbeat.
    assert_eq!(
        lifecycle(directory.path(), "subject-1"),
        (origin, origin + DAY_MS, Some("completed".to_string()))
    );
}

#[test]
fn scope_digest_incarnation_and_missing_refusals_are_distinct() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    let origin = now_ms();
    let reference = store
        .stage_review_input(subject_spec("run-1", "subject-1", origin))
        .unwrap();
    store
        .finish_staging_run("run-1", StagingTerminalState::Completed, origin + 1)
        .unwrap();

    let mut other_scope = job_binding();
    other_scope.project_digest = "q".repeat(64);
    assert_eq!(
        refusal(
            store
                .read_review_input(&reference, &other_scope, origin)
                .unwrap_err()
        ),
        ReviewReadRefusal::ScopeMismatch
    );
    let mut other_owner = job_binding();
    other_owner.owner = ReviewOwner::Job {
        job_id: "job-2".to_string(),
    };
    assert_eq!(
        refusal(
            store
                .read_review_input(&reference, &other_owner, origin)
                .unwrap_err()
        ),
        ReviewReadRefusal::ScopeMismatch
    );
    let changed = ReviewStagedReference {
        payload_digest: "0".repeat(64),
        ..reference.clone()
    };
    assert_eq!(
        refusal(
            store
                .read_review_input(&changed, &job_binding(), origin)
                .unwrap_err()
        ),
        ReviewReadRefusal::Changed
    );
    let foreign = ReviewStagedReference {
        database_incarnation_id: "f".repeat(32),
        ..reference.clone()
    };
    assert_eq!(
        refusal(
            store
                .read_review_input(&foreign, &job_binding(), origin)
                .unwrap_err()
        ),
        ReviewReadRefusal::IncarnationMismatch
    );
    let missing = ReviewStagedReference {
        candidate_id: "subject-9".to_string(),
        ..reference
    };
    assert_eq!(
        refusal(
            store
                .read_review_input(&missing, &job_binding(), origin)
                .unwrap_err()
        ),
        ReviewReadRefusal::Missing
    );
}

#[test]
fn queued_subject_survives_maintenance_and_reopen_past_the_public_lease_until_its_deadline() {
    let directory = tempfile::tempdir().unwrap();
    let origin = now_ms();
    let reference = {
        let store = KernelStore::open(directory.path()).unwrap();
        let reference = store
            .stage_review_input(subject_spec("run-1", "subject-1", origin))
            .unwrap();
        store
            .finish_staging_run("run-1", StagingTerminalState::Completed, origin + 1)
            .unwrap();
        // An unsealed sibling job stays queued past the public lease too.
        store
            .stage_review_input(subject_spec("run-2", "subject-2", origin))
            .unwrap();
        let swept = store.run_staging_maintenance(origin + 2 * HOUR_MS).unwrap();
        assert_eq!(
            swept.abandoned_runs, 0,
            "a 24-hour queue outlives the one-hour public lease"
        );
        reference
    };
    let store = KernelStore::open(directory.path()).unwrap();
    let row = store
        .read_review_input(&reference, &job_binding(), origin + 2 * HOUR_MS)
        .unwrap();
    assert_eq!(row.lifecycle.queue_deadline_at, origin + DAY_MS);
    store
        .read_review_input(&reference, &job_binding(), origin + DAY_MS - 1)
        .unwrap();
    assert_eq!(
        refusal(
            store
                .read_review_input(&reference, &job_binding(), origin + DAY_MS)
                .unwrap_err()
        ),
        ReviewReadRefusal::Expired
    );
    // Expiry sweeps the unsealed sibling and leaves the sealed row for terminal cleanup.
    let swept = store.run_staging_maintenance(origin + DAY_MS).unwrap();
    assert_eq!(swept.abandoned_runs, 1);
    let sibling = ReviewStagedReference {
        candidate_id: "subject-2".to_string(),
        ..reference.clone()
    };
    assert_eq!(
        refusal(
            store
                .read_review_input(&sibling, &job_binding(), origin + DAY_MS - 1)
                .unwrap_err()
        ),
        ReviewReadRefusal::Abandoned
    );
    assert_eq!(
        lifecycle(directory.path(), "subject-1"),
        (origin, origin + DAY_MS, Some("completed".to_string())),
        "expiry never rewrites the sealed row"
    );
    // A late restage of the expired unsealed work is refused rather than rescued.
    assert_eq!(
        store
            .stage_review_input(subject_spec("run-2", "subject-2", origin))
            .unwrap_err(),
        KernelError::Conflict
    );
}

#[test]
fn public_lease_limits_and_private_queue_limits_stay_separate() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    let origin = now_ms();
    let public = StagingCandidateSpec {
        extraction_run_id: "public-run".to_string(),
        candidate_id: "public-candidate".to_string(),
        extractor: "fixture".to_string(),
        source_kind: "repo".to_string(),
        source_id: "source".to_string(),
        source_revision: 1,
        candidate_kind: "domain".to_string(),
        payload: "payload".to_string(),
        provenance: None,
        recorded_at: origin,
        lease_expires_at: origin + HOUR_MS + 1,
    };
    assert_eq!(
        store.stage_candidate(public).unwrap_err(),
        KernelError::InvalidInput,
        "the public one-hour lease cap is unchanged"
    );
    let mut too_long = subject_spec("run-1", "subject-1", origin);
    too_long.queue_deadline_at = origin + REVIEW_QUEUE_LIFETIME_MS + 1;
    assert_eq!(
        store.stage_review_input(too_long).unwrap_err(),
        KernelError::InvalidInput
    );
    let mut at_bound = subject_spec("run-1", "subject-1", origin);
    at_bound.queue_deadline_at = origin + REVIEW_QUEUE_LIFETIME_MS;
    store.stage_review_input(at_bound).unwrap();
}

#[test]
fn schema_illegal_proposals_are_refused_at_decode() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    let illegal = [
        proposal(ProposalAction::Create, memory_target(), Some("text")),
        proposal(ProposalAction::Create, staged_target(), None),
        proposal(ProposalAction::Revise, staged_target(), Some("text")),
        proposal(ProposalAction::Retain, memory_target(), Some("text")),
        proposal(ProposalAction::Retire, staged_target(), None),
        proposal(ProposalAction::NoChange, memory_target(), Some("text")),
    ];
    for proposal in illegal {
        assert_eq!(
            store
                .stage_review_input(proposal_spec("job-1", 1, proposal))
                .unwrap_err(),
            KernelError::InvalidInput
        );
    }
    let mut uncited_drift = proposal(ProposalAction::Retain, memory_target(), None);
    uncited_drift
        .policy_dependencies
        .uncited_disclosed_inputs
        .clear();
    assert_eq!(
        store
            .stage_review_input(proposal_spec("job-1", 1, uncited_drift))
            .unwrap_err(),
        KernelError::InvalidInput
    );
    let mut uncited_support = proposal(ProposalAction::Retain, memory_target(), None);
    uncited_support.support.push(reference("ev-3"));
    assert_eq!(
        store
            .stage_review_input(proposal_spec("job-1", 1, uncited_support))
            .unwrap_err(),
        KernelError::InvalidInput
    );
    let legal = [
        proposal(ProposalAction::Create, staged_target(), Some("text")),
        proposal(ProposalAction::Revise, memory_target(), Some("text")),
        proposal(ProposalAction::Retain, memory_target(), None),
        proposal(ProposalAction::Retire, memory_target(), None),
        proposal(ProposalAction::NoChange, staged_target(), None),
        proposal(ProposalAction::NoChange, memory_target(), None),
    ];
    for (generation, proposal) in (1..).zip(legal) {
        store
            .stage_review_input(proposal_spec("job-1", generation, proposal))
            .unwrap();
    }
    let stale: Vec<u8> = br#"{"version":2,"body":{"kind":"review_subject","source":{"source_kind":"a","source_id":"b","source_revision":1},"facts":[{"text":"t","span":{"alias":"s","start":0,"end":1}}]}}"#.to_vec();
    assert_eq!(
        ReviewPayload::decode(&stale).unwrap_err(),
        KernelError::InvalidInput
    );
    let unknown_field: Vec<u8> = br#"{"version":1,"body":{"kind":"review_subject","source":{"source_kind":"a","source_id":"b","source_revision":1},"facts":[{"text":"t","span":{"alias":"s","start":0,"end":1}}],"extra":1}}"#.to_vec();
    assert_eq!(
        ReviewPayload::decode(&unknown_field).unwrap_err(),
        KernelError::InvalidInput
    );
    let current: Vec<u8> = br#"{"version":1,"body":{"kind":"review_subject","source":{"source_kind":"a","source_id":"b","source_revision":1},"facts":[{"text":"t","span":{"alias":"s","start":0,"end":1}}]}}"#.to_vec();
    ReviewPayload::decode(&current).unwrap();
}

#[test]
fn proposal_identity_is_bound_to_job_and_generation() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    let legal = proposal(ProposalAction::Retain, memory_target(), None);
    let mut wrong_candidate = proposal_spec("job-1", 1, legal.clone());
    wrong_candidate.candidate_id = provisional_result_identity("job-1", 2).candidate_id;
    assert_eq!(
        store.stage_review_input(wrong_candidate).unwrap_err(),
        KernelError::InvalidInput
    );
    let mut subject_as_proposal = proposal_spec("job-1", 1, legal.clone());
    subject_as_proposal.payload = subject("the build uses bun");
    assert_eq!(
        store.stage_review_input(subject_as_proposal).unwrap_err(),
        KernelError::InvalidInput
    );
    let mut proposal_as_subject = subject_spec("run-1", "subject-1", now_ms());
    proposal_as_subject.payload = ReviewPayload::Proposal(legal.clone());
    assert_eq!(
        store.stage_review_input(proposal_as_subject).unwrap_err(),
        KernelError::InvalidInput
    );
    assert_ne!(
        provisional_result_identity("job-1", 1),
        provisional_result_identity("job-1", 2)
    );
    store
        .stage_review_input(proposal_spec("job-1", 1, legal))
        .unwrap();
}

#[test]
fn secrets_and_placeholders_are_refused_rather_than_redacted() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    let origin = now_ms();
    let mut secret = subject_spec("run-1", "subject-1", origin);
    secret.payload = subject(&format!("token {AWS_KEY}"));
    assert_eq!(
        store.stage_review_input(secret).unwrap_err(),
        KernelError::InvalidInput
    );
    let mut placeholder_identity = subject_spec("run-1", "subject-1", origin);
    placeholder_identity.binding.domain_id = kernel::OPERATOR_REDACTION_PLACEHOLDER.to_string();
    assert_eq!(
        store.stage_review_input(placeholder_identity).unwrap_err(),
        KernelError::InvalidInput
    );
    let mut placeholder_text = subject_spec("run-1", "subject-1", origin);
    placeholder_text.payload = subject(&format!("x {}", kernel::OPERATOR_REDACTION_PLACEHOLDER));
    assert_eq!(
        store.stage_review_input(placeholder_text).unwrap_err(),
        KernelError::InvalidInput
    );
    let rows: (i64, i64) = inspect(directory.path(), |conn| {
        conn.query_row(
            "SELECT (SELECT COUNT(*) FROM candidates),(SELECT COUNT(*) FROM extraction_runs)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
    });
    assert_eq!(
        rows,
        (0, 0),
        "a refused input leaves no run or candidate behind"
    );
}

#[test]
fn staged_review_rows_are_invisible_to_canonical_reads() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    let origin = now_ms();
    let tip_before = store.tip().unwrap();
    store
        .stage_review_input(subject_spec("run-1", "subject-1", origin))
        .unwrap();
    store
        .finish_staging_run("run-1", StagingTerminalState::Completed, origin + 1)
        .unwrap();
    let legal = proposal(ProposalAction::Retain, memory_target(), None);
    store
        .stage_review_input(proposal_spec("job-1", 1, legal))
        .unwrap();
    assert_eq!(store.tip().unwrap(), tip_before, "staging writes no commit");
    let snapshot = store.known_as_of(tip_before).unwrap();
    assert!(snapshot.objects.is_empty());
    let (registry, outbox, changes): (i64, i64, i64) = inspect(directory.path(), |conn| {
        conn.query_row(
            "SELECT (SELECT COUNT(*) FROM object_registry),(SELECT COUNT(*) FROM outbox),
                    (SELECT COUNT(*) FROM change_event)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap()
    });
    assert_eq!((registry, outbox, changes), (0, 0, 0));
}
