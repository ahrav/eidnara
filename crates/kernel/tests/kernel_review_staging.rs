#![cfg(feature = "test-support")]

//! Real-store proofs for the review-staging input path: immutable references, replay comparison, decode refusal, scope and digest mismatch, deadline boundaries, and canonical invisibility.

use kernel::{
    AdmissionDomainSpec, AdmissionEvent, AdmissionRequest, ByteRange, CanonicalTarget,
    CommitIntent, DomainSpec, EventKind, EvidenceReference, ExtractedFact, KernelError,
    KernelStore, MAX_FACT_SPANS, MAX_REVIEW_FACTS, MAX_REVIEW_LIMITATIONS,
    MAX_REVIEW_PAYLOAD_BYTES, MAX_REVIEW_REFERENCE_SOURCES, MAX_REVIEW_TEXT_BYTES,
    ManifestReference, PolicyDependencies, ProposalAction, ProposalTarget, REVIEW_PROPOSAL_KIND,
    REVIEW_QUEUE_LIFETIME_MS, REVIEW_SUBJECT_KIND, REVIEW_WITNESS_KIND, ReviewBinding, ReviewOwner,
    ReviewPayload, ReviewProposal, ReviewQuestionTemplate, ReviewReadError, ReviewReadRefusal,
    ReviewStageError, ReviewStageRefusal, ReviewStagedReference, ReviewStagingSpec, ReviewSubject,
    STAGING_RETENTION_MS, Sensitivity, SourceClass, SourceDependency, SourceSpan,
    StagingCandidateSpec, StagingTerminalState, SubjectOrigin, TaintClass, Uncertainty,
    provisional_result_identity,
};
use rusqlite::{Connection, OpenFlags, params};
use sha2::Digest;

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

/// Direct writes stand in for stored-state faults the public API cannot produce.
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

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "kernel-review-staging-test".to_string(),
        operation_key: key.to_string(),
        request_digest: "b".repeat(64),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

fn source(id: &str, revision: i64) -> SourceDependency {
    SourceDependency {
        source_kind: "conversation".to_string(),
        source_id: id.to_string(),
        source_revision: revision,
    }
}

fn binding(owner: ReviewOwner) -> ReviewBinding {
    ReviewBinding {
        project_digest: "0a".repeat(32),
        domain_id: "domain-1".to_string(),
        owner,
        subject_source: source("session-1", 3),
        reference_sources: vec![source("notes", 2), source("session-0", 1)],
    }
}

fn job_binding() -> ReviewBinding {
    binding(ReviewOwner::Job {
        job_id: "job-1".to_string(),
    })
}

fn fact(text: &str) -> ExtractedFact {
    ExtractedFact {
        text: text.to_string(),
        spans: vec![SourceSpan {
            alias: "s1".to_string(),
            start: 0,
            end: 12,
        }],
    }
}

/// The origin every test fact cites: alias `s1` at bytes 0..12 of message `m1`.
fn origin_s1() -> SubjectOrigin {
    SubjectOrigin {
        alias: "s1".to_string(),
        message_id: "m1".to_string(),
        ordinal: 1,
        block_ids: vec!["m1#0".to_string()],
        block_hashes: vec!["0".repeat(64)],
        ranges: vec![ByteRange { start: 0, end: 12 }],
    }
}

fn subject(text: &str) -> ReviewPayload {
    ReviewPayload::Subject(ReviewSubject {
        facts: vec![fact(text)],
        origins: vec![origin_s1()],
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
    let recorded_at = now_ms();
    ReviewStagingSpec {
        extraction_run_id: identity.extraction_run_id,
        candidate_id: identity.candidate_id,
        producer: "curator".to_string(),
        binding: binding(ReviewOwner::Proposal {
            job_id: job_id.to_string(),
            generation,
        }),
        payload: ReviewPayload::Proposal(Box::new(proposal)),
        recorded_at,
        queue_deadline_at: recorded_at + DAY_MS,
    }
}

fn read_refusal(error: ReviewReadError) -> ReviewReadRefusal {
    match error {
        ReviewReadError::Refused(refusal) => refusal,
        other => panic!("expected a read refusal, got {other:?}"),
    }
}

fn stage_refusal(error: ReviewStageError) -> ReviewStageRefusal {
    match error {
        ReviewStageError::Refused(refusal) => refusal,
        ReviewStageError::Store(error) => panic!("store error instead of refusal: {error:?}"),
    }
}

fn seal(store: &KernelStore, run: &str, at: i64) {
    store
        .finish_staging_run(run, StagingTerminalState::Completed, at)
        .unwrap();
}

#[test]
fn exact_replay_returns_the_same_reference_without_refreshing_deadlines() {
    let directory = tempfile::tempdir().unwrap();
    let origin = now_ms();
    let deadline = origin + DAY_MS - 5_000;
    let first = {
        let store = KernelStore::open(directory.path()).unwrap();
        let mut spec = subject_spec("run-1", "subject-1", origin);
        spec.queue_deadline_at = deadline;
        store.stage_review_input(spec).unwrap()
    };
    assert_eq!(first.database_incarnation_id, incarnation(directory.path()));
    assert_eq!(
        first.payload_digest,
        subject("the build uses bun").digest().unwrap()
    );
    let before = lifecycle(directory.path(), "subject-1");
    assert_eq!(before, (origin, deadline, None));

    // A replay after reopen, with a later clock and with an earlier clock, acknowledges the row and changes nothing.
    let store = KernelStore::open(directory.path()).unwrap();
    for recorded_at in [origin + 1_000, origin - 1_000] {
        let mut replay = subject_spec("run-1", "subject-1", recorded_at);
        replay.queue_deadline_at = deadline;
        assert_eq!(store.stage_review_input(replay).unwrap(), first);
        assert_eq!(lifecycle(directory.path(), "subject-1"), before);
    }
    let run: (i64, i64, String) = inspect(directory.path(), |conn| {
        conn.query_row(
            "SELECT heartbeat_at,lease_expires_at,sensitivity_class FROM extraction_runs WHERE extraction_run_id='run-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap()
    });
    assert_eq!(run, (origin, deadline, "sensitive".to_string()));
    let stored_kind: String = inspect(directory.path(), |conn| {
        conn.query_row(
            "SELECT candidate_kind FROM candidates WHERE candidate_id='subject-1'",
            [],
            |row| row.get(0),
        )
        .unwrap()
    });
    assert_eq!(stored_kind, REVIEW_SUBJECT_KIND);
    assert_eq!(REVIEW_SUBJECT_KIND, "review_subject");
    assert_eq!(REVIEW_PROPOSAL_KIND, "review_proposal");
    assert_eq!(REVIEW_WITNESS_KIND, "review");
}

#[test]
fn reference_source_order_does_not_change_identity_and_bounds_hold() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    let origin = now_ms();
    let first = store
        .stage_review_input(subject_spec("run-1", "subject-1", origin))
        .unwrap();
    let mut permuted = subject_spec("run-1", "subject-1", origin);
    permuted.binding.reference_sources.reverse();
    permuted.binding.reference_sources.push(source("notes", 2));
    assert_eq!(store.stage_review_input(permuted).unwrap(), first);
    let witnesses: (Vec<u8>, Vec<u8>) = inspect(directory.path(), |conn| {
        conn.query_row(
            "SELECT r.provenance_witness,c.provenance_witness FROM candidates c
             JOIN extraction_runs r USING(extraction_run_id) WHERE c.candidate_id='subject-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
    });
    assert_eq!(witnesses.0, witnesses.1);
    let witness: serde_json::Value = serde_json::from_slice(&witnesses.0).unwrap();
    assert_eq!(witness["kind"], REVIEW_WITNESS_KIND);
    assert_eq!(witness["binding"]["domain_id"], "domain-1");

    let mut at_bound = subject_spec("run-2", "subject-2", origin);
    at_bound.binding.reference_sources = (0..MAX_REVIEW_REFERENCE_SOURCES)
        .map(|index| source(&format!("ref-{index}"), 1))
        .collect();
    store.stage_review_input(at_bound.clone()).unwrap();
    let mut over_bound = at_bound.clone();
    over_bound.extraction_run_id = "run-3".to_string();
    over_bound.candidate_id = "subject-3".to_string();
    over_bound
        .binding
        .reference_sources
        .push(source("ref-extra", 1));
    assert_eq!(
        stage_refusal(store.stage_review_input(over_bound).unwrap_err()),
        ReviewStageRefusal::Invalid
    );
    let mut subject_repeated = subject_spec("run-4", "subject-4", origin);
    subject_repeated
        .binding
        .reference_sources
        .push(source("session-1", 3));
    assert_eq!(
        stage_refusal(store.stage_review_input(subject_repeated).unwrap_err()),
        ReviewStageRefusal::Invalid
    );
    let mut no_references = subject_spec("run-5", "subject-5", origin);
    no_references.binding.reference_sources.clear();
    store.stage_review_input(no_references).unwrap();
}

#[test]
fn changed_bytes_binding_or_deadline_are_reported_as_changed() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    let origin = now_ms();
    store
        .stage_review_input(subject_spec("run-1", "subject-1", origin))
        .unwrap();

    let mut changed = subject_spec("run-1", "subject-1", origin);
    changed.payload = subject("the build uses npm");
    assert_eq!(
        stage_refusal(store.stage_review_input(changed).unwrap_err()),
        ReviewStageRefusal::Changed
    );
    let mut rebound = subject_spec("run-1", "subject-2", origin);
    rebound.binding.domain_id = "domain-2".to_string();
    assert_eq!(
        stage_refusal(store.stage_review_input(rebound).unwrap_err()),
        ReviewStageRefusal::Changed,
        "a run's binding is part of its identity"
    );
    let mut other_deadline = subject_spec("run-1", "subject-2", origin);
    other_deadline.queue_deadline_at = origin + DAY_MS - 1;
    assert_eq!(
        stage_refusal(store.stage_review_input(other_deadline).unwrap_err()),
        ReviewStageRefusal::Changed,
        "every candidate in a run shares the run's absolute deadline"
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
        read_refusal(
            store
                .read_review_input(&reference, &job_binding(), origin)
                .unwrap_err()
        ),
        ReviewReadRefusal::Unsealed
    );
    seal(&store, "run-1", origin + 1);
    let row = store
        .read_review_input(&reference, &job_binding(), origin + 2)
        .unwrap();
    assert_eq!(row.sensitivity, Sensitivity::Sensitive);
    assert_eq!(row.payload, subject("the build uses bun"));
    assert_eq!(row.binding, job_binding());
    assert_eq!(row.lifecycle.created_at, origin);
    assert_eq!(row.lifecycle.queue_deadline_at, origin + DAY_MS);
    assert_eq!(row.lifecycle.sealed_at, origin + 1);
    assert_eq!(
        stage_refusal(
            store
                .stage_review_input(subject_spec("run-1", "subject-1", origin))
                .unwrap_err()
        ),
        ReviewStageRefusal::Terminal
    );
    assert_eq!(
        lifecycle(directory.path(), "subject-1"),
        (origin, origin + DAY_MS, Some("completed".to_string())),
        "reads and refused restages leave the row untouched"
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
    seal(&store, "run-1", origin + 1);

    let mut other_scope = job_binding();
    other_scope.project_digest = "0b".repeat(32);
    assert_eq!(
        read_refusal(
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
        read_refusal(
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
        read_refusal(
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
        read_refusal(
            store
                .read_review_input(&foreign, &job_binding(), origin)
                .unwrap_err()
        ),
        ReviewReadRefusal::IncarnationMismatch
    );
    let missing = ReviewStagedReference {
        candidate_id: "subject-9".to_string(),
        ..reference.clone()
    };
    assert_eq!(
        read_refusal(
            store
                .read_review_input(&missing, &job_binding(), origin)
                .unwrap_err()
        ),
        ReviewReadRefusal::Missing
    );
    let malformed = ReviewStagedReference {
        payload_digest: "not-a-digest".to_string(),
        ..reference
    };
    assert_eq!(
        store
            .read_review_input(&malformed, &job_binding(), origin)
            .unwrap_err(),
        ReviewReadError::Invalid
    );
}

#[test]
fn stored_bytes_that_no_longer_decode_are_refused_by_a_real_store_read() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    let origin = now_ms();
    let reference = store
        .stage_review_input(subject_spec("run-1", "subject-1", origin))
        .unwrap();
    seal(&store, "run-1", origin + 1);
    drop(store);

    let future_schema = br#"{"version":3,"body":{"kind":"review_subject","facts":[{"text":"t","spans":[{"alias":"s","start":0,"end":1}]}],"origins":[{"alias":"s","message_id":"m","ordinal":1,"block_ids":["m#0"],"block_hashes":["0000000000000000000000000000000000000000000000000000000000000000"],"ranges":[{"start":0,"end":1}]}]}}"#;
    mutate(
        directory.path(),
        "UPDATE candidates SET payload=?1 WHERE candidate_id='subject-1'",
        params![future_schema.as_slice()],
    );
    let store = KernelStore::open(directory.path()).unwrap();
    let rewritten = ReviewStagedReference {
        payload_digest: format!("{:x}", sha2::Sha256::digest(future_schema)),
        ..reference.clone()
    };
    assert_eq!(
        read_refusal(
            store
                .read_review_input(&rewritten, &job_binding(), origin)
                .unwrap_err()
        ),
        ReviewReadRefusal::DecodeRefused
    );
    assert_eq!(
        read_refusal(
            store
                .read_review_input(&reference, &job_binding(), origin)
                .unwrap_err()
        ),
        ReviewReadRefusal::Changed,
        "the original reference no longer names the stored bytes"
    );
    drop(store);

    let current = subject("the build uses bun").encode().unwrap();
    mutate(
        directory.path(),
        "UPDATE candidates SET payload=?1,candidate_kind=?2 WHERE candidate_id='subject-1'",
        params![current.as_bytes(), REVIEW_PROPOSAL_KIND],
    );
    let store = KernelStore::open(directory.path()).unwrap();
    assert_eq!(
        read_refusal(
            store
                .read_review_input(&reference, &job_binding(), origin)
                .unwrap_err()
        ),
        ReviewReadRefusal::DecodeRefused,
        "a payload whose kind disagrees with its column is refused"
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
        seal(&store, "run-1", origin + 1);
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
        read_refusal(
            store
                .read_review_input(&reference, &job_binding(), origin + DAY_MS)
                .unwrap_err()
        ),
        ReviewReadRefusal::Expired
    );
    let swept = store.run_staging_maintenance(origin + DAY_MS).unwrap();
    assert_eq!(
        swept.abandoned_runs, 1,
        "the unsealed sibling expires at its deadline"
    );
    let sibling = ReviewStagedReference {
        candidate_id: "subject-2".to_string(),
        ..reference.clone()
    };
    assert_eq!(
        read_refusal(
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
    assert_eq!(
        stage_refusal(
            store
                .stage_review_input(subject_spec("run-2", "subject-2", origin))
                .unwrap_err()
        ),
        ReviewStageRefusal::Terminal,
        "abandoned work is terminal and is not rescued by a late restage"
    );
    // Terminal cleanup deletes the sealed row thirty days after sealing.
    let cleaned = store
        .run_staging_maintenance(origin + 1 + STAGING_RETENTION_MS - 1)
        .unwrap();
    assert_eq!(cleaned.deleted_runs, 0);
    let cleaned = store
        .run_staging_maintenance(origin + 1 + STAGING_RETENTION_MS)
        .unwrap();
    assert_eq!(cleaned.deleted_runs, 1);
    assert_eq!(
        read_refusal(
            store
                .read_review_input(&reference, &job_binding(), origin + 2)
                .unwrap_err()
        ),
        ReviewReadRefusal::Missing
    );
}

#[test]
fn the_store_clock_bounds_a_deadline_the_caller_clock_would_accept() {
    let directory = tempfile::tempdir().unwrap();
    let origin = now_ms();
    let store = KernelStore::open(directory.path()).unwrap();
    let reference = store
        .stage_review_input(subject_spec("run-1", "subject-1", origin))
        .unwrap();
    seal(&store, "run-1", origin + 1);
    store
        .stage_review_input(subject_spec("run-2", "subject-2", origin))
        .unwrap();
    // The stored deadline moves into the past without a sweep, as after a long outage.
    mutate(
        directory.path(),
        "UPDATE candidates SET created_at=?1,heartbeat_at=?1,lease_expires_at=?2
         WHERE candidate_id IN ('subject-1','subject-2')",
        params![origin - DAY_MS, origin - HOUR_MS],
    );
    mutate(
        directory.path(),
        "UPDATE extraction_runs SET started_at=?1,heartbeat_at=?1,lease_expires_at=?2
         WHERE extraction_run_id IN ('run-1','run-2')",
        params![origin - DAY_MS, origin - HOUR_MS],
    );
    assert_eq!(
        read_refusal(
            store
                .read_review_input(&reference, &job_binding(), origin - DAY_MS)
                .unwrap_err()
        ),
        ReviewReadRefusal::Expired,
        "a caller clock behind the deadline cannot revive a row the store clock has expired"
    );
    assert_eq!(
        stage_refusal(
            store
                .stage_review_input(subject_spec("run-2", "subject-2", origin))
                .unwrap_err()
        ),
        ReviewStageRefusal::Expired,
        "an exact restage of expired unsealed work is refused before any sweep runs"
    );
    assert_eq!(
        lifecycle(directory.path(), "subject-2"),
        (origin - DAY_MS, origin - HOUR_MS, None),
        "the refused restage refreshes nothing"
    );
    drop(store);
    // Reopen runs the sweep, so the expired unsealed run is now terminal.
    let store = KernelStore::open(directory.path()).unwrap();
    assert_eq!(
        stage_refusal(
            store
                .stage_review_input(subject_spec("run-2", "subject-2", origin))
                .unwrap_err()
        ),
        ReviewStageRefusal::Terminal
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
        stage_refusal(store.stage_review_input(too_long).unwrap_err()),
        ReviewStageRefusal::Invalid
    );
    let mut at_bound = subject_spec("run-1", "subject-1", origin);
    at_bound.queue_deadline_at = origin + REVIEW_QUEUE_LIFETIME_MS;
    store.stage_review_input(at_bound).unwrap();
    // Public renewal cannot walk a review run's deadline forward.
    assert_eq!(
        store
            .renew_staging_run("run-1", origin + 1, origin + 1 + HOUR_MS)
            .unwrap_err(),
        KernelError::Conflict
    );
    assert_eq!(
        lifecycle(directory.path(), "subject-1"),
        (origin, origin + REVIEW_QUEUE_LIFETIME_MS, None)
    );
    // A deadline already behind the store clock is refused as expired, not stored.
    let mut stale = subject_spec("run-2", "subject-2", origin - DAY_MS);
    stale.queue_deadline_at = origin - 1;
    assert_eq!(
        stage_refusal(store.stage_review_input(stale).unwrap_err()),
        ReviewStageRefusal::Expired
    );
}

#[test]
fn schema_illegal_proposals_are_refused_at_decode() {
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
            ReviewPayload::Proposal(Box::new(proposal))
                .encode()
                .unwrap_err(),
            ReviewStageRefusal::Invalid
        );
    }
    let mut uncited_drift = proposal(ProposalAction::Retain, memory_target(), None);
    uncited_drift
        .policy_dependencies
        .uncited_disclosed_inputs
        .clear();
    assert_eq!(
        ReviewPayload::Proposal(Box::new(uncited_drift))
            .encode()
            .unwrap_err(),
        ReviewStageRefusal::Invalid
    );
    let mut uncited_support = proposal(ProposalAction::Retain, memory_target(), None);
    uncited_support.support.push(reference("ev-3"));
    assert_eq!(
        ReviewPayload::Proposal(Box::new(uncited_support))
            .encode()
            .unwrap_err(),
        ReviewStageRefusal::Invalid
    );
    let mut short_digest = proposal(ProposalAction::Retain, memory_target(), None);
    short_digest.manifest.digest = "n/a".to_string();
    assert_eq!(
        ReviewPayload::Proposal(Box::new(short_digest))
            .encode()
            .unwrap_err(),
        ReviewStageRefusal::Invalid
    );

    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
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
    let stale = br#"{"version":1,"body":{"kind":"review_subject","facts":[{"text":"t","span":{"alias":"s","start":0,"end":1}}]}}"#;
    assert_eq!(
        ReviewPayload::decode(stale).unwrap_err(),
        ReviewStageRefusal::Invalid
    );
    let unknown_field = br#"{"version":2,"body":{"kind":"review_subject","facts":[{"text":"t","spans":[{"alias":"s","start":0,"end":1}]}],"origins":[{"alias":"s","message_id":"m","ordinal":1,"block_ids":["m#0"],"block_hashes":["0000000000000000000000000000000000000000000000000000000000000000"],"ranges":[{"start":0,"end":1}]}],"extra":1}}"#;
    assert_eq!(
        ReviewPayload::decode(unknown_field).unwrap_err(),
        ReviewStageRefusal::Invalid
    );
    let current = br#"{"version":2,"body":{"kind":"review_subject","facts":[{"text":"t","spans":[{"alias":"s","start":0,"end":1}]}],"origins":[{"alias":"s","message_id":"m","ordinal":1,"block_ids":["m#0"],"block_hashes":["0000000000000000000000000000000000000000000000000000000000000000"],"ranges":[{"start":0,"end":1}]}]}}"#;
    ReviewPayload::decode(current).unwrap();
}

#[test]
fn payload_bounds_are_enforced_at_the_boundary() {
    let ok = |payload: ReviewPayload| payload.encode().unwrap();
    let refused = |payload: ReviewPayload| {
        assert_eq!(payload.encode().unwrap_err(), ReviewStageRefusal::Invalid);
    };
    refused(ReviewPayload::Subject(ReviewSubject {
        facts: vec![],
        origins: vec![origin_s1()],
    }));
    ok(ReviewPayload::Subject(ReviewSubject {
        facts: vec![fact("f"); MAX_REVIEW_FACTS],
        origins: vec![origin_s1()],
    }));
    refused(ReviewPayload::Subject(ReviewSubject {
        facts: vec![fact("f"); MAX_REVIEW_FACTS + 1],
        origins: vec![origin_s1()],
    }));
    ok(subject(&"x".repeat(MAX_REVIEW_TEXT_BYTES)));
    refused(subject(&"x".repeat(MAX_REVIEW_TEXT_BYTES + 1)));
    let mut reversed = fact("f");
    reversed.spans[0].end = 0;
    reversed.spans[0].start = 1;
    refused(ReviewPayload::Subject(ReviewSubject {
        facts: vec![reversed],
        origins: vec![origin_s1()],
    }));
    let mut uncited = fact("f");
    uncited.spans.clear();
    refused(ReviewPayload::Subject(ReviewSubject {
        facts: vec![uncited],
        origins: vec![origin_s1()],
    }));
    let mut crowded = fact("f");
    crowded.spans = vec![crowded.spans[0].clone(); MAX_FACT_SPANS + 1];
    refused(ReviewPayload::Subject(ReviewSubject {
        facts: vec![crowded],
        origins: vec![origin_s1()],
    }));
    // Origins: every cited span must be listed under exactly one origin whose blocks and hashes align.
    let origin = |ranges: Vec<ByteRange>| SubjectOrigin {
        alias: "s1".to_string(),
        message_id: "m1".to_string(),
        ordinal: 1,
        block_ids: vec!["m1#0".to_string()],
        block_hashes: vec!["0".repeat(64)],
        ranges,
    };
    ok(ReviewPayload::Subject(ReviewSubject {
        facts: vec![fact("f")],
        origins: vec![origin(vec![ByteRange { start: 0, end: 12 }])],
    }));
    refused(ReviewPayload::Subject(ReviewSubject {
        facts: vec![fact("f")],
        origins: vec![origin(vec![ByteRange { start: 0, end: 11 }])],
    }));
    refused(ReviewPayload::Subject(ReviewSubject {
        facts: vec![fact("f")],
        origins: vec![
            origin(vec![ByteRange { start: 0, end: 12 }]),
            origin(vec![ByteRange { start: 0, end: 12 }]),
        ],
    }));
    let mut misaligned = origin(vec![ByteRange { start: 0, end: 12 }]);
    misaligned.block_hashes.clear();
    refused(ReviewPayload::Subject(ReviewSubject {
        facts: vec![fact("f")],
        origins: vec![misaligned],
    }));
    // Two facts at the per-field text bound exceed the serialized payload bound.
    refused(ReviewPayload::Subject(ReviewSubject {
        facts: vec![fact(&"x".repeat(MAX_REVIEW_TEXT_BYTES)); 2],
        origins: vec![origin_s1()],
    }));
    let wide = ReviewPayload::Subject(ReviewSubject {
        facts: vec![fact(&"x".repeat(MAX_REVIEW_TEXT_BYTES)); 3],
        origins: vec![origin_s1()],
    });
    assert!(serde_json::to_vec(&wide).unwrap().len() > MAX_REVIEW_PAYLOAD_BYTES);
    let mut limits = proposal(ProposalAction::Retain, memory_target(), None);
    limits.limitations = vec!["l".to_string(); MAX_REVIEW_LIMITATIONS];
    ok(ReviewPayload::Proposal(Box::new(limits.clone())));
    limits.limitations.push("l".to_string());
    refused(ReviewPayload::Proposal(Box::new(limits)));
}

#[test]
fn proposal_identity_is_bound_to_job_and_generation() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    let legal = proposal(ProposalAction::Retain, memory_target(), None);
    let mut wrong_candidate = proposal_spec("job-1", 1, legal.clone());
    wrong_candidate.candidate_id = provisional_result_identity("job-1", 2).candidate_id;
    assert_eq!(
        stage_refusal(store.stage_review_input(wrong_candidate).unwrap_err()),
        ReviewStageRefusal::Invalid
    );
    let mut subject_as_proposal = proposal_spec("job-1", 1, legal.clone());
    subject_as_proposal.payload = subject("the build uses bun");
    assert_eq!(
        stage_refusal(store.stage_review_input(subject_as_proposal).unwrap_err()),
        ReviewStageRefusal::Invalid
    );
    let mut proposal_as_subject = subject_spec("run-1", "subject-1", now_ms());
    proposal_as_subject.payload = ReviewPayload::Proposal(Box::new(legal.clone()));
    assert_eq!(
        stage_refusal(store.stage_review_input(proposal_as_subject).unwrap_err()),
        ReviewStageRefusal::Invalid
    );
    assert_ne!(
        provisional_result_identity("job-1", 1),
        provisional_result_identity("job-1", 2)
    );
    let reference = store
        .stage_review_input(proposal_spec("job-1", 1, legal))
        .unwrap();
    assert_eq!(
        reference.candidate_id,
        provisional_result_identity("job-1", 1).candidate_id
    );
}

#[test]
fn secrets_and_placeholders_are_refused_rather_than_redacted() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    let origin = now_ms();
    let mut secret = subject_spec("run-1", "subject-1", origin);
    secret.payload = subject(&format!("token {AWS_KEY}"));
    assert_eq!(
        stage_refusal(store.stage_review_input(secret).unwrap_err()),
        ReviewStageRefusal::SecretDetected
    );
    let mut secret_proposal = proposal(ProposalAction::Revise, memory_target(), Some("text"));
    secret_proposal.new_text = Some(format!("token {AWS_KEY}"));
    assert_eq!(
        stage_refusal(
            store
                .stage_review_input(proposal_spec("job-1", 1, secret_proposal))
                .unwrap_err()
        ),
        ReviewStageRefusal::SecretDetected
    );
    let mut placeholder_identity = subject_spec("run-1", "subject-1", origin);
    placeholder_identity.binding.domain_id = kernel::OPERATOR_REDACTION_PLACEHOLDER.to_string();
    assert_eq!(
        stage_refusal(store.stage_review_input(placeholder_identity).unwrap_err()),
        ReviewStageRefusal::SecretDetected
    );
    let mut placeholder_text = subject_spec("run-1", "subject-1", origin);
    placeholder_text.payload = subject(&format!("x {}", kernel::OPERATOR_REDACTION_PLACEHOLDER));
    assert_eq!(
        stage_refusal(store.stage_review_input(placeholder_text).unwrap_err()),
        ReviewStageRefusal::SecretDetected
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
fn staged_review_rows_are_invisible_to_canonical_reads_and_inadmissible() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    let origin = now_ms();
    // Positive control: one canonical domain exists before any review row is staged.
    store
        .commit(intent("seed"), |envelope| {
            envelope.insert_domain(DomainSpec {
                domain_id: "domain-1".to_string(),
                object_id: "object-1".to_string(),
                name: "name-1".to_string(),
                source_kind: "fixture".to_string(),
                source_id: "source-1".to_string(),
                source_revision: 1,
                sensitivity: Sensitivity::Normal,
            })?;
            Ok(String::new())
        })
        .unwrap();
    let tip_before = store.tip().unwrap();
    let counts = |root: &std::path::Path| -> (i64, i64, i64, i64) {
        inspect(root, |conn| {
            conn.query_row(
                "SELECT (SELECT COUNT(*) FROM object_registry),(SELECT COUNT(*) FROM outbox),
                        (SELECT COUNT(*) FROM change_event),(SELECT COUNT(*) FROM admission_decisions)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap()
        })
    };
    let before = counts(directory.path());
    assert_eq!(before.0, 1);

    let reference = store
        .stage_review_input(subject_spec("run-1", "subject-1", origin))
        .unwrap();
    seal(&store, "run-1", origin + 1);
    let legal = proposal(ProposalAction::Retain, memory_target(), None);
    let proposal_reference = store
        .stage_review_input(proposal_spec("job-1", 1, legal))
        .unwrap();
    seal(
        &store,
        &provisional_result_identity("job-1", 1).extraction_run_id,
        now_ms() + 1,
    );

    assert_eq!(store.tip().unwrap(), tip_before, "staging writes no commit");
    let snapshot = store.known_as_of(tip_before).unwrap();
    assert_eq!(snapshot.objects.len(), 1);
    assert_eq!(snapshot.objects[0].object_id, "object-1");
    assert_eq!(counts(directory.path()), before);

    // Admission cannot resolve a review row as a candidate, so no decision is written for it.
    for candidate_id in [&reference.candidate_id, &proposal_reference.candidate_id] {
        let result = store.commit(intent(&format!("admit-{candidate_id}")), |envelope| {
            envelope.admit_domain_candidate(
                AdmissionRequest {
                    candidate_id: Some(candidate_id.clone()),
                    subject_object_id: None,
                    source_class: Some(SourceClass::UntrustedRepoText),
                    taint_class: Some(TaintClass::RepoUntrustedText),
                    event: AdmissionEvent {
                        kind: EventKind::ExplicitReject,
                        trigger_object_id: None,
                        approval_object_id: None,
                        evidence_id: None,
                        reason: "review rows are not admission subjects".to_string(),
                    },
                },
                AdmissionDomainSpec {
                    domain_id: "domain-1".to_string(),
                    object_id: "object-review".to_string(),
                    name: "review".to_string(),
                },
            )?;
            Ok(String::new())
        });
        assert_eq!(result.unwrap_err(), KernelError::NotFound);
    }
    assert_eq!(store.tip().unwrap(), tip_before);
    assert_eq!(counts(directory.path()), before);
}
