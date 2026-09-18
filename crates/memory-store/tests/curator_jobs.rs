//! Real-store proofs for Curator review jobs and frozen selections: incarnation identity, causal dedup, capacity, frozen-page retention, atomic rollback, expiry, and nonreclaimable receipt charges.

use std::collections::BTreeMap;

use context_core::redaction::RedactionErrorKind;
use memory_store::curator_jobs::{
    CURATOR_JOB_ALLOWANCE_BYTES, CURATOR_QUEUE_LIFETIME_MS, CURATOR_RECEIPT_CHARGE_BYTES,
    CausalInputs, CuratorJobError, CuratorJobInput, CuratorJobOutcome, CuratorJobRefusal,
    CuratorJobState, EnqueueOutcome, EvidenceAvailability, FROZEN_PAGE_RECEIPT_CHARGE_BYTES,
    FROZEN_SELECTION_ALLOWANCE_BYTES, FrozenSelectionPage, FrozenSelectionState,
    MAX_CAUSAL_POLICY_VERSIONS, MAX_CAUSAL_SIGNALS, MAX_CURATOR_METADATA_BYTES_PER_PROJECT,
    MAX_FROZEN_PAGE_BYTES, MAX_FROZEN_SELECTIONS_PER_HOST, MAX_PENDING_CURATOR_JOBS_PER_HOST,
    MAX_PENDING_CURATOR_JOBS_PER_PROJECT, MAX_REQUIRED_EVIDENCE, MAX_SELECTION_REFERENCES,
    ProducerBinding, ReserveOutcome, ReviewTarget, activate_curator_job_in_tx,
    advance_selection_cursor_in_tx, complete_frozen_selection_in_tx,
    enqueue_frozen_selection_in_tx, freeze_selection_in_tx, reserve_curator_job_in_tx,
};
use memory_store::{MemoryStore, MemoryStoreError};
use storage::StorageDescriptor;

const NOW: i64 = 1_700_000_000_000;
const AWS_KEY: &str = "AKIAQ7RSTUVWXYZ23456";
/// A keyed-JSON credential the scanner recognizes only in its raw form; serializing it inside another JSON document escapes the quotes it keys on.
const KEYED_SECRET: &str = r#"{"clientSecret":"hunter-two"}"#;

fn descriptor(dir: &std::path::Path) -> StorageDescriptor {
    MemoryStore::test_descriptor(dir, "eidnara-curator-jobs-test")
}

fn producer(firing: &str) -> ProducerBinding {
    ProducerBinding {
        producer: "history-summarizer".to_string(),
        firing_id: firing.to_string(),
        ordinal: 0,
    }
}

fn subject(candidate: &str) -> ReviewTarget {
    ReviewTarget::StagedSubject {
        kernel_incarnation: "0a".repeat(16),
        candidate_id: candidate.to_string(),
        payload_digest: "0d".repeat(32),
    }
}

fn inputs(candidate: &str) -> CausalInputs {
    CausalInputs {
        target: subject(candidate),
        question_template: "extracted_facts".to_string(),
        signals: vec!["contradiction".to_string(), "drift".to_string()],
        required_evidence: vec![EvidenceAvailability {
            evidence_id: "ev-1".to_string(),
            available: true,
        }],
        policy_versions: BTreeMap::from([("disclosure".to_string(), "3".to_string())]),
    }
}

fn input(candidate: &str) -> CuratorJobInput {
    CuratorJobInput {
        subject: subject(candidate),
        starting_references: vec!["mem-1".to_string(), "mem-2".to_string()],
        question_template: "extracted_facts".to_string(),
    }
}

fn refusal(error: CuratorJobError) -> CuratorJobRefusal {
    match error {
        CuratorJobError::Refused(refusal) => refusal,
        CuratorJobError::Store(error) => panic!("store error instead of refusal: {error:?}"),
    }
}

fn reserved(outcome: ReserveOutcome) -> memory_store::curator_jobs::CuratorJob {
    match outcome {
        ReserveOutcome::Reserved(job) => job,
        ReserveOutcome::Existing(job) => panic!("expected a new reservation, found {job:?}"),
    }
}

#[test]
fn incarnation_survives_reopen_and_changes_on_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let first = {
        let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
        store.curator_store_incarnation().unwrap()
    };
    assert_eq!(first.len(), 32);
    let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
    assert_eq!(store.curator_store_incarnation().unwrap(), first);
    // REPLACE runs as delete-then-insert, and the connection does not enable recursive triggers, so the delete guard alone would not fire.
    let replaced: Result<(), _> = store.with_fenced_conn_for_test(|conn| {
        conn.execute(
            "INSERT OR REPLACE INTO curator_store_identity (id, database_incarnation_id, created_at_ms)
             VALUES (0, ?1, 1)",
            ["ab".repeat(16)],
        )
        .map(drop)
    });
    assert!(replaced.is_err(), "the incarnation row cannot be replaced");
    assert_eq!(store.curator_store_incarnation().unwrap(), first);
    drop(store);
    std::fs::remove_file(dir.path().join("memory.sqlite")).unwrap();
    let replaced = MemoryStore::open(&descriptor(dir.path())).unwrap();
    assert_ne!(replaced.curator_store_incarnation().unwrap(), first);
}

#[test]
fn identical_causal_inputs_deduplicate_and_changed_evidence_permits_one_new_job() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
    let job = reserved(
        store
            .reserve_curator_job("proj", &producer("f1"), &inputs("cand-1"), NOW)
            .unwrap(),
    );
    assert_eq!(job.state, CuratorJobState::Reserved);
    assert_eq!(job.queue_deadline_ms, NOW + CURATOR_QUEUE_LIFETIME_MS);
    assert_eq!(
        job.causal_identity,
        inputs("cand-1").causal_identity().unwrap()
    );

    // Another firing with the same inputs in another order returns the same row.
    let mut reordered = inputs("cand-1");
    reordered.signals.reverse();
    match store
        .reserve_curator_job("proj", &producer("f2"), &reordered, NOW + 5_000)
        .unwrap()
    {
        ReserveOutcome::Existing(existing) => assert_eq!(existing, job),
        other => panic!("expected the existing row, got {other:?}"),
    }
    // A terminal outcome is retained and never retried for the same inputs.
    store
        .finish_curator_job(
            "proj",
            &job.causal_identity,
            CuratorJobOutcome::Failed,
            NOW + 10,
        )
        .unwrap();
    match store
        .reserve_curator_job("proj", &producer("f3"), &inputs("cand-1"), NOW + 20)
        .unwrap()
    {
        ReserveOutcome::Existing(existing) => {
            assert_eq!(
                existing.state,
                CuratorJobState::Terminal(CuratorJobOutcome::Failed)
            );
        }
        other => panic!("expected the terminal row, got {other:?}"),
    }
    assert_eq!(
        refusal(
            store
                .finish_curator_job(
                    "proj",
                    &job.causal_identity,
                    CuratorJobOutcome::Failed,
                    NOW + 30
                )
                .unwrap_err()
        ),
        CuratorJobRefusal::Terminal,
        "a terminal row is never reopened"
    );
    // Newly available linked evidence at the unchanged target is one new causal job.
    let mut with_evidence = inputs("cand-1");
    with_evidence.required_evidence.push(EvidenceAvailability {
        evidence_id: "ev-2".to_string(),
        available: true,
    });
    let renewed = reserved(
        store
            .reserve_curator_job("proj", &producer("f4"), &with_evidence, NOW + 40)
            .unwrap(),
    );
    assert_ne!(renewed.causal_identity, job.causal_identity);
    assert_eq!(renewed.target, job.target);
    match store
        .reserve_curator_job("proj", &producer("f5"), &with_evidence, NOW + 50)
        .unwrap()
    {
        ReserveOutcome::Existing(existing) => {
            assert_eq!(existing.causal_identity, renewed.causal_identity)
        }
        other => panic!("expected dedup, got {other:?}"),
    }
    // Firing ids, ordinals, and clocks are not causal inputs.
    let mut other_firing = producer("f6");
    other_firing.ordinal = 9;
    assert!(matches!(
        store
            .reserve_curator_job("proj", &other_firing, &with_evidence, NOW + 60)
            .unwrap(),
        ReserveOutcome::Existing(_)
    ));
    assert_eq!(
        store
            .lookup_curator_job("proj", &job.causal_identity)
            .unwrap()
            .unwrap()
            .state,
        CuratorJobState::Terminal(CuratorJobOutcome::Failed)
    );
    // Evidence that becomes available at the unchanged target is a causal change; conflicting availability for one id is a producer error.
    let mut flipped = inputs("cand-1");
    flipped.required_evidence[0].available = false;
    let unavailable = reserved(
        store
            .reserve_curator_job("proj", &producer("f7"), &flipped, NOW + 70)
            .unwrap(),
    );
    assert_ne!(unavailable.causal_identity, job.causal_identity);
    let mut conflicting = inputs("cand-1");
    conflicting.required_evidence.push(EvidenceAvailability {
        evidence_id: "ev-1".to_string(),
        available: false,
    });
    assert_eq!(
        refusal(
            store
                .reserve_curator_job("proj", &producer("f8"), &conflicting, NOW + 80)
                .unwrap_err()
        ),
        CuratorJobRefusal::InvalidRequest
    );
}

#[test]
fn a_memory_target_reserves_and_activates_with_the_memory_as_subject() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
    let memory = ReviewTarget::Memory {
        object_id: "mem-7".to_string(),
        source_revision: 3,
    };
    let mut causal = inputs("unused");
    causal.target = memory.clone();
    let job = reserved(
        store
            .reserve_curator_job("proj", &producer("f1"), &causal, NOW)
            .unwrap(),
    );
    let mut memory_input = input("unused");
    memory_input.subject = memory;
    let ready = store
        .activate_curator_job(
            "proj",
            &job.causal_identity,
            &producer("f1"),
            &memory_input,
            NOW,
        )
        .unwrap();
    assert_eq!(ready.state, CuratorJobState::Ready(memory_input));
}

#[test]
fn metadata_quota_exhaustion_refuses_new_work_and_deletes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
    let job = reserved(
        store
            .reserve_curator_job("proj", &producer("f1"), &inputs("cand-1"), NOW)
            .unwrap(),
    );
    // A receipt charge near the project quota stands in for a long history of admitted work.
    let near_quota = i64::try_from(
        MAX_CURATOR_METADATA_BYTES_PER_PROJECT
            - CURATOR_JOB_ALLOWANCE_BYTES
            - CURATOR_RECEIPT_CHARGE_BYTES,
    )
    .unwrap();
    store
        .with_fenced_conn_for_test(|conn| {
            conn.execute(
                "UPDATE curator_jobs SET receipt_charge_bytes = ?1 WHERE causal_identity = ?2",
                rusqlite::params![near_quota, job.causal_identity],
            )
        })
        .unwrap();
    let headroom = store.curator_headroom("proj").unwrap();
    assert_eq!(
        headroom.project_metadata_remaining,
        CURATOR_RECEIPT_CHARGE_BYTES
    );
    assert_eq!(
        refusal(
            store
                .reserve_curator_job("proj", &producer("f2"), &inputs("cand-2"), NOW)
                .unwrap_err()
        ),
        CuratorJobRefusal::MetadataQuota
    );
    assert_eq!(
        refusal(
            store
                .freeze_selection(
                    "proj",
                    "slot-1",
                    "attempt-1",
                    &FrozenSelectionPage {
                        references: vec![inputs("cand-3")],
                        next_cursor: None,
                    },
                    NOW
                )
                .unwrap_err()
        ),
        CuratorJobRefusal::MetadataQuota
    );
    assert_eq!(store.curator_headroom("proj").unwrap(), headroom);
    let rows: (i64, i64) = store
        .with_conn_for_test(|conn| {
            conn.query_row(
                "SELECT (SELECT COUNT(*) FROM curator_jobs), (SELECT COUNT(*) FROM curator_frozen_selections)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
        })
        .unwrap();
    assert_eq!(rows, (1, 0));
    // Finishing the existing job releases only its allowance; the receipt stays and the quota stays exhausted.
    store
        .finish_curator_job(
            "proj",
            &job.causal_identity,
            CuratorJobOutcome::Failed,
            NOW + 1,
        )
        .unwrap();
    assert_eq!(
        store
            .curator_headroom("proj")
            .unwrap()
            .project_metadata_remaining,
        CURATOR_RECEIPT_CHARGE_BYTES + CURATOR_JOB_ALLOWANCE_BYTES
    );
    // Exactly the released allowance admits one more job; the receipt it leaves behind exhausts the quota for good.
    reserved(
        store
            .reserve_curator_job("proj", &producer("f2"), &inputs("cand-2"), NOW)
            .unwrap(),
    );
    assert_eq!(
        store
            .curator_headroom("proj")
            .unwrap()
            .project_metadata_remaining,
        0
    );
    assert_eq!(
        refusal(
            store
                .reserve_curator_job("proj", &producer("f3"), &inputs("cand-3"), NOW)
                .unwrap_err()
        ),
        CuratorJobRefusal::MetadataQuota
    );
}

#[test]
fn activation_requires_the_reservation_and_takes_no_second_slot() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
    let job = reserved(
        store
            .reserve_curator_job("proj", &producer("f1"), &inputs("cand-1"), NOW)
            .unwrap(),
    );
    let before = store.curator_headroom("proj").unwrap();
    assert_eq!(before.pending_jobs, 1);
    let missing = inputs("cand-9").causal_identity().unwrap();
    assert_eq!(
        refusal(
            store
                .activate_curator_job("proj", &missing, &producer("f1"), &input("cand-9"), NOW)
                .unwrap_err()
        ),
        CuratorJobRefusal::Missing
    );
    assert_eq!(
        refusal(
            store
                .activate_curator_job(
                    "proj",
                    &job.causal_identity,
                    &producer("other"),
                    &input("cand-1"),
                    NOW
                )
                .unwrap_err()
        ),
        CuratorJobRefusal::ProducerMismatch
    );
    assert_eq!(
        refusal(
            store
                .activate_curator_job(
                    "proj",
                    &job.causal_identity,
                    &producer("f1"),
                    &input("cand-2"),
                    NOW
                )
                .unwrap_err()
        ),
        CuratorJobRefusal::InvalidRequest,
        "the input subject must be the reserved target"
    );
    let mut other_question = input("cand-1");
    other_question.question_template = "other_question".to_string();
    assert_eq!(
        refusal(
            store
                .activate_curator_job(
                    "proj",
                    &job.causal_identity,
                    &producer("f1"),
                    &other_question,
                    NOW
                )
                .unwrap_err()
        ),
        CuratorJobRefusal::InvalidRequest,
        "the input question template must be the reserved template"
    );
    let mut too_many = input("cand-1");
    too_many.starting_references = (0..9).map(|index| format!("mem-{index}")).collect();
    assert_eq!(
        refusal(
            store
                .activate_curator_job(
                    "proj",
                    &job.causal_identity,
                    &producer("f1"),
                    &too_many,
                    NOW
                )
                .unwrap_err()
        ),
        CuratorJobRefusal::InvalidRequest
    );
    let ready = store
        .activate_curator_job(
            "proj",
            &job.causal_identity,
            &producer("f1"),
            &input("cand-1"),
            NOW + 1,
        )
        .unwrap();
    assert_eq!(ready.state, CuratorJobState::Ready(input("cand-1")));
    assert_eq!(ready.queue_deadline_ms, job.queue_deadline_ms);
    let after = store.curator_headroom("proj").unwrap();
    assert_eq!(after.pending_jobs, 1, "activation consumes no second slot");
    assert_eq!(after.project_metadata_bytes, before.project_metadata_bytes);
    assert_eq!(
        refusal(
            store
                .activate_curator_job(
                    "proj",
                    &job.causal_identity,
                    &producer("f1"),
                    &input("cand-1"),
                    NOW + 2
                )
                .unwrap_err()
        ),
        CuratorJobRefusal::NotReserved
    );
    assert_eq!(
        store.ready_curator_jobs("proj", 10, NOW + 2).unwrap(),
        vec![ready]
    );
    assert_eq!(
        store
            .ready_curator_jobs("proj", 10, job.queue_deadline_ms)
            .unwrap(),
        vec![],
        "a ready row past its deadline is not dispatched before the sweep expires it"
    );
}

#[test]
fn activation_composed_with_a_failing_producer_transaction_commits_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
    let job = reserved(
        store
            .reserve_curator_job("proj", &producer("f1"), &inputs("cand-1"), NOW)
            .unwrap(),
    );
    let result: Result<(), _> = store.with_fenced_conn_for_test(|conn| {
        let activated = activate_curator_job_in_tx(
            conn,
            "proj",
            &job.causal_identity,
            &producer("f1"),
            &input("cand-1"),
            NOW + 1,
        )?;
        assert!(matches!(activated.state, CuratorJobState::Ready(_)));
        // The producer's own progress write fails after activation.
        Err(rusqlite::Error::QueryReturnedNoRows)
    });
    assert!(result.is_err());
    assert_eq!(
        store
            .lookup_curator_job("proj", &job.causal_identity)
            .unwrap()
            .unwrap()
            .state,
        CuratorJobState::Reserved,
        "failed activation commits no progress and no state change"
    );
    // The same primitive reserves and activates in one committed transaction.
    let committed = store
        .with_fenced_conn_for_test(|conn| {
            let outcome =
                reserve_curator_job_in_tx(conn, "proj", &producer("f2"), &inputs("cand-2"), NOW)?;
            let ReserveOutcome::Reserved(job) = outcome else {
                panic!("fresh inputs reserve")
            };
            activate_curator_job_in_tx(
                conn,
                "proj",
                &job.causal_identity,
                &producer("f2"),
                &input("cand-2"),
                NOW,
            )
        })
        .unwrap();
    assert!(matches!(committed.state, CuratorJobState::Ready(_)));
    assert_eq!(store.ready_curator_jobs("proj", 10, NOW).unwrap().len(), 1);
}

#[test]
fn capacity_counts_reserved_and_ready_and_refusal_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
    for index in 0..MAX_PENDING_CURATOR_JOBS_PER_PROJECT {
        let job = reserved(
            store
                .reserve_curator_job(
                    "proj",
                    &producer("f"),
                    &inputs(&format!("cand-{index}")),
                    NOW,
                )
                .unwrap(),
        );
        if index % 2 == 0 {
            store
                .activate_curator_job(
                    "proj",
                    &job.causal_identity,
                    &producer("f"),
                    &input(&format!("cand-{index}")),
                    NOW,
                )
                .unwrap();
        }
    }
    let headroom = store.curator_headroom("proj").unwrap();
    assert_eq!(headroom.pending_jobs, MAX_PENDING_CURATOR_JOBS_PER_PROJECT);
    assert_eq!(
        refusal(
            store
                .reserve_curator_job("proj", &producer("f"), &inputs("cand-over"), NOW)
                .unwrap_err()
        ),
        CuratorJobRefusal::ProjectCapacity
    );
    assert!(
        store
            .lookup_curator_job("proj", &inputs("cand-over").causal_identity().unwrap())
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store.curator_headroom("proj").unwrap(),
        headroom,
        "a refused reservation charges nothing"
    );
    // Existing identities still replay under full capacity.
    assert!(matches!(
        store
            .reserve_curator_job("proj", &producer("f"), &inputs("cand-0"), NOW)
            .unwrap(),
        ReserveOutcome::Existing(_)
    ));
    // A terminal outcome frees the slot but keeps its receipt charge.
    let first = inputs("cand-0").causal_identity().unwrap();
    store
        .finish_curator_job("proj", &first, CuratorJobOutcome::Failed, NOW + 1)
        .unwrap();
    let after = store.curator_headroom("proj").unwrap();
    assert_eq!(after.pending_jobs, MAX_PENDING_CURATOR_JOBS_PER_PROJECT - 1);
    assert_eq!(
        headroom.project_metadata_bytes - after.project_metadata_bytes,
        CURATOR_JOB_ALLOWANCE_BYTES,
        "terminal work releases its allowance and keeps its receipt charge"
    );
    reserved(
        store
            .reserve_curator_job("proj", &producer("f"), &inputs("cand-over"), NOW)
            .unwrap(),
    );
    // The host bound spans projects.
    for project in 1..MAX_PENDING_CURATOR_JOBS_PER_HOST / MAX_PENDING_CURATOR_JOBS_PER_PROJECT {
        for index in 0..MAX_PENDING_CURATOR_JOBS_PER_PROJECT {
            reserved(
                store
                    .reserve_curator_job(
                        &format!("proj-{project}"),
                        &producer("f"),
                        &inputs(&format!("cand-{index}")),
                        NOW,
                    )
                    .unwrap(),
            );
        }
    }
    assert_eq!(
        refusal(
            store
                .reserve_curator_job("proj-host", &producer("f"), &inputs("cand-0"), NOW)
                .unwrap_err()
        ),
        CuratorJobRefusal::HostCapacity
    );
    let host_full = store.curator_headroom("proj-host").unwrap();
    assert_eq!(host_full.pending_jobs, 0);
    assert_eq!(
        host_full.host_pending_jobs, MAX_PENDING_CURATOR_JOBS_PER_HOST,
        "headroom reports the host bound that refuses an otherwise empty project"
    );
}

#[test]
fn expiry_records_terminal_outcomes_without_resurrection_and_receipts_survive_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
    let reserved_job = reserved(
        store
            .reserve_curator_job("proj", &producer("f1"), &inputs("cand-1"), NOW)
            .unwrap(),
    );
    let ready_job = reserved(
        store
            .reserve_curator_job("proj", &producer("f2"), &inputs("cand-2"), NOW)
            .unwrap(),
    );
    store
        .activate_curator_job(
            "proj",
            &ready_job.causal_identity,
            &producer("f2"),
            &input("cand-2"),
            NOW,
        )
        .unwrap();
    let later = reserved(
        store
            .reserve_curator_job("proj", &producer("f3"), &inputs("cand-3"), NOW + 1_000)
            .unwrap(),
    );
    let page = FrozenSelectionPage {
        references: vec![inputs("cand-4")],
        next_cursor: Some("cursor-2".to_string()),
    };
    store
        .freeze_selection("proj", "slot-1", "attempt-1", &page, NOW)
        .unwrap();
    let charged = store.curator_headroom("proj").unwrap();

    let deadline = NOW + CURATOR_QUEUE_LIFETIME_MS;
    assert_eq!(store.expire_curator_work(deadline - 1).unwrap(), (0, 0));
    // A producer that activates after the deadline but before the sweep is refused without a state change.
    assert_eq!(
        refusal(
            store
                .activate_curator_job(
                    "proj",
                    &reserved_job.causal_identity,
                    &producer("f1"),
                    &input("cand-1"),
                    deadline
                )
                .unwrap_err()
        ),
        CuratorJobRefusal::Expired
    );
    assert_eq!(
        store
            .lookup_curator_job("proj", &reserved_job.causal_identity)
            .unwrap()
            .unwrap()
            .state,
        CuratorJobState::Reserved
    );
    // A finish at or after the deadline is refused the same way, so the permanent outcome does not depend on whether the sweep ran first.
    assert_eq!(
        refusal(
            store
                .finish_curator_job(
                    "proj",
                    &ready_job.causal_identity,
                    CuratorJobOutcome::Failed,
                    deadline
                )
                .unwrap_err()
        ),
        CuratorJobRefusal::Expired
    );
    assert!(matches!(
        store
            .lookup_curator_job("proj", &ready_job.causal_identity)
            .unwrap()
            .unwrap()
            .state,
        CuratorJobState::Ready(_)
    ));
    assert_eq!(store.expire_curator_work(deadline).unwrap(), (2, 1));
    let ready_input: Option<String> = store
        .with_conn_for_test(|conn| {
            conn.query_row(
                "SELECT input_json FROM curator_jobs WHERE causal_identity = ?1",
                [ready_job.causal_identity.as_str()],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(
        ready_input, None,
        "a terminal receipt drops its input and stays compact"
    );
    for identity in [&reserved_job.causal_identity, &ready_job.causal_identity] {
        assert_eq!(
            store
                .lookup_curator_job("proj", identity)
                .unwrap()
                .unwrap()
                .state,
            CuratorJobState::Terminal(CuratorJobOutcome::Expired)
        );
    }
    assert_eq!(
        store
            .lookup_curator_job("proj", &later.causal_identity)
            .unwrap()
            .unwrap()
            .state,
        CuratorJobState::Reserved,
        "a later reservation keeps its own deadline"
    );
    let selection = store
        .lookup_frozen_selection("proj", "slot-1", "attempt-1")
        .unwrap()
        .unwrap();
    assert_eq!(selection.state, FrozenSelectionState::Expired);
    assert_eq!(
        selection.page, None,
        "an expired page drops its references and cursor; the slot never advanced"
    );
    let expired_page: Option<String> = store
        .with_conn_for_test(|conn| {
            conn.query_row(
                "SELECT page_json FROM curator_frozen_selections WHERE slot_id = 'slot-1'",
                [],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(expired_page, None, "a terminal page receipt stays compact");
    // Late activation records nothing and does not resurrect the row.
    assert_eq!(
        refusal(
            store
                .activate_curator_job(
                    "proj",
                    &reserved_job.causal_identity,
                    &producer("f1"),
                    &input("cand-1"),
                    deadline + 1
                )
                .unwrap_err()
        ),
        CuratorJobRefusal::Terminal
    );
    let after = store.curator_headroom("proj").unwrap();
    assert_eq!(
        charged.project_metadata_bytes - after.project_metadata_bytes,
        2 * CURATOR_JOB_ALLOWANCE_BYTES + FROZEN_SELECTION_ALLOWANCE_BYTES
    );
    assert_eq!(
        after.project_metadata_bytes,
        3 * CURATOR_RECEIPT_CHARGE_BYTES
            + FROZEN_PAGE_RECEIPT_CHARGE_BYTES
            + CURATOR_JOB_ALLOWANCE_BYTES,
        "every admitted job and every frozen page keeps its permanent receipt charge"
    );
    drop(store);
    let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
    assert_eq!(
        store.curator_headroom("proj").unwrap(),
        after,
        "receipt charges survive reopen"
    );
    assert!(matches!(
        store.reserve_curator_job("proj", &producer("f9"), &inputs("cand-1"), deadline + 10).unwrap(),
        ReserveOutcome::Existing(job) if job.state == CuratorJobState::Terminal(CuratorJobOutcome::Expired)
    ));
}

#[test]
fn frozen_pages_are_bounded_retained_under_deferral_and_enqueued_once() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
    let page = FrozenSelectionPage {
        references: (0..MAX_SELECTION_REFERENCES)
            .map(|index| inputs(&format!("cand-{index}")))
            .collect(),
        next_cursor: Some("cursor-9".to_string()),
    };
    let frozen = store
        .freeze_selection("proj", "slot-1", "attempt-1", &page, NOW)
        .unwrap();
    assert_eq!(frozen.state, FrozenSelectionState::Frozen);
    assert_eq!(
        frozen.selection_deadline_ms,
        NOW + CURATOR_QUEUE_LIFETIME_MS
    );
    let scan_batches = || {
        store
            .with_conn_for_test(|conn| {
                conn.query_row("SELECT COUNT(*) FROM scan_batches", [], |row| {
                    row.get::<_, i64>(0)
                })
            })
            .unwrap()
    };
    let batches_after_freeze = scan_batches();
    assert_eq!(
        store
            .freeze_selection("proj", "slot-1", "attempt-1", &page, NOW + 5)
            .unwrap(),
        frozen,
        "the same attempt replays its page"
    );
    assert_eq!(
        scan_batches(),
        batches_after_freeze,
        "a replayed page records no new scan audit"
    );
    let mut moved_cursor = page.clone();
    moved_cursor.next_cursor = Some("cursor-10".to_string());
    assert_eq!(
        refusal(
            store
                .freeze_selection("proj", "slot-1", "attempt-1", &moved_cursor, NOW + 6)
                .unwrap_err()
        ),
        CuratorJobRefusal::InvalidRequest,
        "an attempt identity freezes exactly one page; a different page under it is refused"
    );
    assert_eq!(
        store
            .lookup_frozen_selection("proj", "slot-1", "attempt-1")
            .unwrap()
            .unwrap(),
        frozen,
        "a refused replay changes nothing"
    );
    let mut nine = page.clone();
    nine.references.push(inputs("cand-9"));
    assert_eq!(
        refusal(
            store
                .freeze_selection("proj", "slot-2", "attempt-1", &nine, NOW)
                .unwrap_err()
        ),
        CuratorJobRefusal::InvalidRequest
    );
    assert_eq!(
        refusal(
            store
                .freeze_selection("proj", "slot-1", "attempt-2", &page, NOW)
                .unwrap_err()
        ),
        CuratorJobRefusal::ProjectSelectionCapacity,
        "one frozen page per project"
    );
    // A capacity deferral is not a state: the page stays frozen for a later attempt.
    assert_eq!(
        store
            .lookup_frozen_selection("proj", "slot-1", "attempt-1")
            .unwrap()
            .unwrap()
            .state,
        FrozenSelectionState::Frozen
    );
    // An enqueue must commit with the slot's cursor advance, so only the composable primitive accepts it.
    assert_eq!(
        refusal(
            store
                .complete_frozen_selection(
                    "proj",
                    "slot-1",
                    "attempt-1",
                    FrozenSelectionState::Enqueued,
                    NOW + 10
                )
                .unwrap_err()
        ),
        CuratorJobRefusal::InvalidRequest,
        "the standalone wrapper cannot enqueue: the cursor would commit without the slot"
    );
    let enqueued = store
        .with_fenced_conn_for_test(|conn| {
            complete_frozen_selection_in_tx(
                conn,
                "proj",
                "slot-1",
                "attempt-1",
                FrozenSelectionState::Enqueued,
                NOW + 10,
            )
        })
        .unwrap();
    assert_eq!(enqueued.state, FrozenSelectionState::Enqueued);
    assert_eq!(
        enqueued
            .page
            .expect("an enqueue returns the page whose cursor the slot advances to")
            .next_cursor,
        Some("cursor-9".to_string())
    );
    assert_eq!(
        refusal(
            store
                .complete_frozen_selection(
                    "proj",
                    "slot-1",
                    "attempt-1",
                    FrozenSelectionState::FailedSlot,
                    NOW + 11
                )
                .unwrap_err()
        ),
        CuratorJobRefusal::Terminal
    );
    assert_eq!(
        refusal(
            store
                .complete_frozen_selection(
                    "proj",
                    "slot-1",
                    "attempt-1",
                    FrozenSelectionState::Frozen,
                    NOW + 11
                )
                .unwrap_err()
        ),
        CuratorJobRefusal::InvalidRequest
    );
    assert_eq!(
        refusal(
            store
                .freeze_selection("proj", "slot-1", "attempt-1", &page, NOW + 12)
                .unwrap_err()
        ),
        CuratorJobRefusal::Terminal,
        "a page that left the frozen state is not re-frozen under its attempt identity"
    );
    // Once enqueued the project may freeze again; the host bound spans projects.
    store
        .freeze_selection("proj", "slot-1", "attempt-2", &page, NOW + 12)
        .unwrap();
    for project in 1..MAX_FROZEN_SELECTIONS_PER_HOST {
        store
            .freeze_selection(
                &format!("proj-{project}"),
                "slot-1",
                "attempt-1",
                &page,
                NOW,
            )
            .unwrap();
    }
    assert_eq!(
        refusal(
            store
                .freeze_selection("proj-host", "slot-1", "attempt-1", &page, NOW)
                .unwrap_err()
        ),
        CuratorJobRefusal::HostSelectionCapacity
    );
    // An expired page cannot be enqueued late.
    let late: Result<(), _> = store.with_fenced_conn_for_test(|conn| {
        complete_frozen_selection_in_tx(
            conn,
            "proj-1",
            "slot-1",
            "attempt-1",
            FrozenSelectionState::Enqueued,
            NOW + CURATOR_QUEUE_LIFETIME_MS,
        )
        .map(drop)
    });
    assert!(
        late.unwrap_err()
            .to_string()
            .contains("deadline has passed"),
        "a late enqueue is refused as expired"
    );
    // Any completion at or after the deadline is refused the same way, so the receipt does not depend on whether the sweep ran first.
    assert_eq!(
        refusal(
            store
                .complete_frozen_selection(
                    "proj-1",
                    "slot-1",
                    "attempt-1",
                    FrozenSelectionState::FailedSlot,
                    NOW + CURATOR_QUEUE_LIFETIME_MS
                )
                .unwrap_err()
        ),
        CuratorJobRefusal::Expired
    );
    let failed = store
        .complete_frozen_selection(
            "proj-2",
            "slot-1",
            "attempt-1",
            FrozenSelectionState::FailedSlot,
            NOW + 20,
        )
        .unwrap();
    assert_eq!(failed.state, FrozenSelectionState::FailedSlot);
    assert_eq!(
        failed.page, None,
        "only an enqueue returns the page; other terminal results are compact"
    );
}

#[test]
fn identities_and_inputs_reject_secrets_and_stay_reference_only() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
    let job = reserved(
        store
            .reserve_curator_job("proj", &producer("f1"), &inputs("cand-1"), NOW)
            .unwrap(),
    );
    let secret_detected = |error: CuratorJobError| {
        assert!(
            matches!(
                error,
                CuratorJobError::Store(MemoryStoreError::Redaction(
                    RedactionErrorKind::SecretDetected
                ))
            ),
            "expected a secret refusal, got {error:?}"
        );
    };
    // A firing id carrying a secret is refused rather than redacted, and no row is written.
    secret_detected(
        store
            .reserve_curator_job(
                "proj",
                &producer(&format!("firing-{AWS_KEY}")),
                &inputs("cand-2"),
                NOW,
            )
            .unwrap_err(),
    );
    let mut secret_signal = inputs("cand-3");
    secret_signal.signals.push(format!("signal-{AWS_KEY}"));
    secret_detected(
        store
            .reserve_curator_job("proj", &producer("f2"), &secret_signal, NOW)
            .unwrap_err(),
    );
    // Reference-only input: a starting reference carrying a secret is refused, and the row stays reserved.
    let mut secret_input = input("cand-1");
    secret_input.starting_references = vec![format!("ref-{AWS_KEY}")];
    secret_detected(
        store
            .activate_curator_job(
                "proj",
                &job.causal_identity,
                &producer("f1"),
                &secret_input,
                NOW,
            )
            .unwrap_err(),
    );
    let mut secret_page = FrozenSelectionPage {
        references: vec![inputs("cand-4")],
        next_cursor: None,
    };
    secret_page.references[0].required_evidence[0].evidence_id = format!("ev-{AWS_KEY}");
    secret_detected(
        store
            .freeze_selection("proj", "slot-1", "attempt-1", &secret_page, NOW)
            .unwrap_err(),
    );
    let secret_cursor = FrozenSelectionPage {
        references: vec![inputs("cand-4")],
        next_cursor: Some(format!("cursor-{AWS_KEY}")),
    };
    secret_detected(
        store
            .freeze_selection("proj", "slot-1", "attempt-1", &secret_cursor, NOW)
            .unwrap_err(),
    );
    // Table triggers reject caller-supplied secret text in every column that stores caller text.
    let raw: Result<(), _> = store.with_fenced_conn_for_test(|conn| {
        activate_curator_job_in_tx(
            conn,
            "proj",
            &job.causal_identity,
            &producer("f1"),
            &secret_input,
            NOW,
        )
        .map(drop)
    });
    assert!(
        raw.is_err(),
        "a raw insert of secret text is refused by the trigger"
    );
    let mut secret_producer = producer("f3");
    secret_producer.producer = format!("history-{AWS_KEY}");
    let raw: Result<(), _> = store.with_fenced_conn_for_test(|conn| {
        reserve_curator_job_in_tx(conn, "proj", &secret_producer, &inputs("cand-5"), NOW).map(drop)
    });
    assert!(
        raw.is_err(),
        "a raw reservation whose producer carries a secret is refused by the trigger"
    );
    // A keyed-JSON credential inside a serialized column is escaped past the detector, so the primitives scan each raw string before encoding.
    let mut keyed_input = input("cand-1");
    keyed_input.starting_references = vec![KEYED_SECRET.to_string()];
    let raw: Result<(), _> = store.with_fenced_conn_for_test(|conn| {
        activate_curator_job_in_tx(
            conn,
            "proj",
            &job.causal_identity,
            &producer("f1"),
            &keyed_input,
            NOW,
        )
        .map(drop)
    });
    assert!(
        raw.is_err(),
        "a raw activation whose reference is a keyed-JSON credential is refused"
    );
    let raw: Result<(), _> = store.with_fenced_conn_for_test(|conn| {
        reserve_curator_job_in_tx(conn, "proj", &producer("f5"), &inputs(KEYED_SECRET), NOW)
            .map(drop)
    });
    assert!(
        raw.is_err(),
        "a raw reservation whose candidate id is a keyed-JSON credential is refused"
    );
    let keyed_page = FrozenSelectionPage {
        references: vec![inputs("cand-7")],
        next_cursor: Some(KEYED_SECRET.to_string()),
    };
    let raw: Result<(), _> = store.with_fenced_conn_for_test(|conn| {
        freeze_selection_in_tx(conn, "proj", "slot-7", "attempt-1", &keyed_page, NOW).map(drop)
    });
    assert!(
        raw.is_err(),
        "a raw freeze whose cursor is a keyed-JSON credential is refused"
    );
    // Fingerprinted causal fields never reach a column, so the primitive scans them itself.
    let raw: Result<(), _> = store.with_fenced_conn_for_test(|conn| {
        reserve_curator_job_in_tx(conn, "proj", &producer("f4"), &secret_signal, NOW).map(drop)
    });
    assert!(
        raw.is_err(),
        "a raw reservation whose signal carries a secret is refused before it is hashed"
    );
    let clean_page = FrozenSelectionPage {
        references: vec![inputs("cand-6")],
        next_cursor: None,
    };
    for (slot_id, attempt) in [
        (format!("slot-{AWS_KEY}"), "attempt-1".to_string()),
        ("slot-1".to_string(), format!("attempt-{AWS_KEY}")),
    ] {
        let raw: Result<(), _> = store.with_fenced_conn_for_test(|conn| {
            freeze_selection_in_tx(conn, "proj", &slot_id, &attempt, &clean_page, NOW).map(drop)
        });
        assert!(
            raw.is_err(),
            "a raw freeze whose slot or attempt identity carries a secret is refused by the trigger"
        );
    }
    assert_eq!(
        store
            .lookup_curator_job("proj", &job.causal_identity)
            .unwrap()
            .unwrap()
            .state,
        CuratorJobState::Reserved
    );
    let stored: Vec<String> = store
        .with_conn_for_test(|conn| {
            let mut rows = Vec::new();
            let mut jobs = conn.prepare(
                "SELECT project || producer || firing_id || target_json || question_template || COALESCE(input_json, '') || causal_identity FROM curator_jobs",
            )?;
            rows.extend(jobs.query_map([], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?);
            let mut pages = conn.prepare(
                "SELECT project || slot_id || selection_attempt || COALESCE(page_json, '') || COALESCE(next_cursor, '') FROM curator_frozen_selections",
            )?;
            rows.extend(pages.query_map([], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?);
            Ok(rows)
        })
        .unwrap();
    assert_eq!(stored.len(), 1, "only the clean reservation exists");
    assert!(
        stored
            .iter()
            .all(|text| !text.contains(AWS_KEY) && !text.contains("hunter-two"))
    );
    assert!(stored[0].contains("cand-1"));
}

/// A selection cursor is caller text bound into the same durable family as a job row: a detected secret refuses the write on a fresh row and on the upsert over an existing one, and the clean cursor stays.
#[test]
fn a_selection_cursor_carrying_a_secret_is_refused_on_insert_and_upsert() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
    let secret = format!("0\u{1f}object-{AWS_KEY}");
    let fresh: Result<(), _> = store.with_fenced_conn_for_test(|conn| {
        advance_selection_cursor_in_tx(conn, "proj", "slot-1", Some(&secret), NOW)
    });
    assert!(
        fresh.is_err(),
        "a fresh cursor row carrying a secret is refused"
    );
    store
        .with_fenced_conn_for_test(|conn| {
            advance_selection_cursor_in_tx(conn, "proj", "slot-1", Some("0\u{1f}object-1"), NOW)
        })
        .unwrap();
    let upsert: Result<(), _> = store.with_fenced_conn_for_test(|conn| {
        advance_selection_cursor_in_tx(conn, "proj", "slot-1", Some(&secret), NOW + 1)
    });
    assert!(upsert.is_err(), "an upsert carrying a secret is refused");
    let keyed: Result<(), _> = store.with_fenced_conn_for_test(|conn| {
        advance_selection_cursor_in_tx(conn, "proj", &format!("slot-{AWS_KEY}"), None, NOW)
    });
    assert!(keyed.is_err(), "a slot id carrying a secret is refused");
    let stored: Vec<String> = store
        .with_conn_for_test(|conn| {
            let mut rows = conn.prepare("SELECT cursor FROM curator_selection_cursors")?;
            rows.query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })
        .unwrap();
    assert_eq!(stored, vec!["0\u{1f}object-1".to_string()]);
}

/// The cursor primitive binds `project` like every other transaction-local primitive of the family: an identity past the bound is refused before the row exists, so no row can be written that the public reader would refuse.
#[test]
fn a_cursor_for_an_overlong_project_is_refused_before_it_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
    let overlong = "p".repeat(257);
    let written: Result<(), _> = store.with_fenced_conn_for_test(|conn| {
        advance_selection_cursor_in_tx(conn, &overlong, "slot-1", Some("0\u{1f}object-1"), NOW)
    });
    assert!(written.is_err(), "an overlong project is refused");
    let rows: i64 = store
        .with_conn_for_test(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM curator_selection_cursors",
                [],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(rows, 0);
}

/// The transaction-local enqueue writes nothing when it defers: a caller that commits its own transaction around a `Deferred` outcome must find the page still frozen with its references, not a committed `enqueued` row without jobs.
#[test]
fn a_deferred_enqueue_leaves_the_page_frozen_even_when_its_transaction_commits() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
    for index in 0..MAX_PENDING_CURATOR_JOBS_PER_PROJECT {
        reserved(
            store
                .reserve_curator_job(
                    "proj",
                    &producer("fill"),
                    &inputs(&format!("fill-{index}")),
                    NOW,
                )
                .unwrap(),
        );
    }
    let page = FrozenSelectionPage {
        references: vec![inputs("cand-1")],
        next_cursor: Some("cursor-2".to_string()),
    };
    let frozen = store
        .freeze_selection("proj", "slot-1", "attempt-1", &page, NOW)
        .unwrap();
    let outcome = store
        .with_fenced_conn_for_test(|conn| {
            enqueue_frozen_selection_in_tx(conn, "proj", &frozen, &producer("f1"), NOW + 1)
        })
        .unwrap();
    assert_eq!(
        outcome,
        EnqueueOutcome::Deferred(CuratorJobRefusal::ProjectCapacity)
    );
    let row = store
        .lookup_frozen_selection("proj", "slot-1", "attempt-1")
        .unwrap()
        .unwrap();
    assert_eq!(row.state, FrozenSelectionState::Frozen);
    assert_eq!(
        row.page,
        Some(page),
        "the deferred page keeps its references"
    );
}

/// A page whose references became jobs keeps the scan audit of those identities: the audit now describes the job rows, which retain the same target, template, signals, and evidence text for the store incarnation.
#[test]
fn an_enqueued_page_keeps_the_scan_audit_of_its_references() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
    let page = FrozenSelectionPage {
        references: vec![inputs("cand-1")],
        next_cursor: None,
    };
    let frozen = store
        .freeze_selection("proj", "slot-1", "attempt-1", &page, NOW)
        .unwrap();
    let outcome = store
        .with_fenced_conn_for_test(|conn| {
            enqueue_frozen_selection_in_tx(conn, "proj", &frozen, &producer("f1"), NOW + 1)
        })
        .unwrap();
    assert!(
        matches!(outcome, EnqueueOutcome::Enqueued { jobs: 1, .. }),
        "{outcome:?}"
    );
    let scans: Vec<String> = store
        .with_conn_for_test(|conn| {
            conn.prepare("SELECT DISTINCT field_id FROM scan_owner_copies ORDER BY field_id")?
                .query_map([], |row| row.get(0))?
                .collect()
        })
        .unwrap();
    assert!(
        scans.iter().any(|field| field == "candidate_id"),
        "the job's target identity keeps its audit: {scans:?}"
    );
    assert!(
        scans.iter().any(|field| field == "question_template"),
        "{scans:?}"
    );
}

/// A page that reaches its deadline between the slot's sweep and its enqueue is recorded as the slot's failure inside the enqueue transaction, not refused back to the caller for another retry.
#[test]
fn an_enqueue_at_or_after_the_page_deadline_records_the_failed_slot() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
    let page = FrozenSelectionPage {
        references: vec![inputs("cand-1")],
        next_cursor: Some("cursor-2".to_string()),
    };
    let frozen = store
        .freeze_selection("proj", "slot-1", "attempt-1", &page, NOW)
        .unwrap();
    let at_deadline = NOW + CURATOR_QUEUE_LIFETIME_MS;
    let outcome = store
        .with_fenced_conn_for_test(|conn| {
            enqueue_frozen_selection_in_tx(conn, "proj", &frozen, &producer("f1"), at_deadline)
        })
        .unwrap();
    assert_eq!(outcome, EnqueueOutcome::Expired);
    let row = store
        .lookup_frozen_selection("proj", "slot-1", "attempt-1")
        .unwrap()
        .unwrap();
    assert_eq!(row.state, FrozenSelectionState::FailedSlot);
    assert_eq!(row.page, None, "a terminal row drops its page");
    assert_eq!(
        store
            .lookup_curator_job("proj", &inputs("cand-1").causal_identity().unwrap())
            .unwrap(),
        None,
        "nothing was enqueued"
    );
}

/// One reference at every identity and list bound; eight of them exceed [`MAX_FROZEN_PAGE_BYTES`].
fn maximal_inputs(index: usize) -> CausalInputs {
    let identity = |prefix: &str, ordinal: usize| format!("{prefix}-{index}-{ordinal:03}");
    CausalInputs {
        target: ReviewTarget::Memory {
            object_id: format!("{:0>256}", format!("object-{index}")),
            source_revision: 7,
        },
        question_template: format!("{:0>256}", "question"),
        signals: (0..MAX_CAUSAL_SIGNALS)
            .map(|ordinal| format!("{:0>256}", identity("signal", ordinal)))
            .collect(),
        required_evidence: (0..MAX_REQUIRED_EVIDENCE)
            .map(|ordinal| EvidenceAvailability {
                evidence_id: format!("{:0>256}", identity("evidence", ordinal)),
                available: ordinal % 2 == 0,
            })
            .collect(),
        policy_versions: (0..MAX_CAUSAL_POLICY_VERSIONS)
            .map(|ordinal| {
                (
                    format!("{:0>256}", identity("policy", ordinal)),
                    format!("{:0>256}", identity("version", ordinal)),
                )
            })
            .collect(),
    }
}

#[test]
fn frozen_page_bound_is_typed_and_admits_pages_the_schema_stores() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
    // A single maximal reference is well over 8 KiB and under the page bound.
    let one = FrozenSelectionPage {
        references: vec![maximal_inputs(0)],
        next_cursor: Some("c".repeat(512)),
    };
    let serialized = serde_json::to_string(&one).unwrap().len();
    assert!(serialized > 8 * 1024 && serialized < MAX_FROZEN_PAGE_BYTES);
    let frozen = store
        .freeze_selection("proj", "slot-1", "attempt-1", &one, NOW)
        .unwrap();
    assert_eq!(frozen.state, FrozenSelectionState::Frozen);
    let stored_len: i64 = store
        .with_conn_for_test(|conn| {
            conn.query_row(
                "SELECT length(page_json) FROM curator_frozen_selections WHERE slot_id = 'slot-1'",
                [],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert!(
        usize::try_from(stored_len).unwrap() > 8 * 1024,
        "the schema stores the page the module admitted"
    );
    // Eight maximal references exceed the bound and are refused with the typed refusal.
    let eight = FrozenSelectionPage {
        references: (0..MAX_SELECTION_REFERENCES).map(maximal_inputs).collect(),
        next_cursor: None,
    };
    assert!(serde_json::to_string(&eight).unwrap().len() > MAX_FROZEN_PAGE_BYTES);
    assert_eq!(
        refusal(
            store
                .freeze_selection("proj-2", "slot-1", "attempt-1", &eight, NOW)
                .unwrap_err()
        ),
        CuratorJobRefusal::PageTooLarge
    );
    assert!(
        store
            .lookup_frozen_selection("proj-2", "slot-1", "attempt-1")
            .unwrap()
            .is_none()
    );
}

#[test]
fn terminal_pages_drop_their_references_and_keep_a_receipt_charge() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
    let page = FrozenSelectionPage {
        references: vec![inputs("cand-1")],
        next_cursor: Some("cursor-3".to_string()),
    };
    store
        .freeze_selection("proj", "slot-1", "attempt-1", &page, NOW)
        .unwrap();
    assert_eq!(
        store
            .curator_headroom("proj")
            .unwrap()
            .project_metadata_bytes,
        FROZEN_PAGE_RECEIPT_CHARGE_BYTES + FROZEN_SELECTION_ALLOWANCE_BYTES,
        "a frozen page holds its allowance and its permanent receipt"
    );
    let enqueued = store
        .with_fenced_conn_for_test(|conn| {
            complete_frozen_selection_in_tx(
                conn,
                "proj",
                "slot-1",
                "attempt-1",
                FrozenSelectionState::Enqueued,
                NOW + 1,
            )
        })
        .unwrap();
    assert_eq!(
        enqueued
            .page
            .as_ref()
            .and_then(|page| page.next_cursor.clone()),
        Some("cursor-3".to_string()),
        "the enqueue hands the slot the cursor it advances to"
    );
    let receipt = store
        .lookup_frozen_selection("proj", "slot-1", "attempt-1")
        .unwrap()
        .unwrap();
    assert_eq!(receipt.state, FrozenSelectionState::Enqueued);
    assert_eq!(receipt.page, None, "a terminal page keeps no references");
    let (page_json, cursor): (Option<String>, Option<String>) = store
        .with_conn_for_test(|conn| {
            conn.query_row(
                "SELECT page_json, next_cursor FROM curator_frozen_selections WHERE slot_id = 'slot-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
        })
        .unwrap();
    assert_eq!(page_json, None);
    assert_eq!(cursor, None);
    assert_eq!(
        store
            .curator_headroom("proj")
            .unwrap()
            .project_metadata_bytes,
        FROZEN_PAGE_RECEIPT_CHARGE_BYTES,
        "the allowance is released; the receipt charge stays for the incarnation"
    );
    // A failed slot also drops its page, and every attempt leaves one receipt behind.
    store
        .freeze_selection("proj", "slot-1", "attempt-2", &page, NOW + 2)
        .unwrap();
    store
        .complete_frozen_selection(
            "proj",
            "slot-1",
            "attempt-2",
            FrozenSelectionState::FailedSlot,
            NOW + 3,
        )
        .unwrap();
    assert_eq!(
        store
            .curator_headroom("proj")
            .unwrap()
            .project_metadata_bytes,
        2 * FROZEN_PAGE_RECEIPT_CHARGE_BYTES
    );
    let retained: i64 = store
        .with_conn_for_test(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM curator_frozen_selections WHERE page_json IS NOT NULL",
                [],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(retained, 0, "no terminal page retains its references");
}

#[test]
fn transaction_local_primitives_validate_the_project_identity() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
    let too_long = "p".repeat(257);
    let page = FrozenSelectionPage {
        references: vec![inputs("cand-1")],
        next_cursor: None,
    };
    for project in ["", too_long.as_str()] {
        let reserve: Result<(), _> = store.with_fenced_conn_for_test(|conn| {
            reserve_curator_job_in_tx(conn, project, &producer("f1"), &inputs("cand-1"), NOW)
                .map(drop)
        });
        let message = reserve.unwrap_err().to_string();
        assert!(
            message.contains(&CuratorJobRefusal::InvalidRequest.to_string()),
            "a raw reservation refuses an invalid project as a typed refusal, got {message}"
        );
        let freeze: Result<(), _> = store.with_fenced_conn_for_test(|conn| {
            freeze_selection_in_tx(conn, project, "slot-1", "attempt-1", &page, NOW).map(drop)
        });
        let message = freeze.unwrap_err().to_string();
        assert!(
            message.contains(&CuratorJobRefusal::InvalidRequest.to_string()),
            "a raw freeze refuses an invalid project as a typed refusal, got {message}"
        );
    }
    let rows: (i64, i64) = store
        .with_conn_for_test(|conn| {
            conn.query_row(
                "SELECT (SELECT COUNT(*) FROM curator_jobs), (SELECT COUNT(*) FROM curator_frozen_selections)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
        })
        .unwrap();
    assert_eq!(rows, (0, 0), "an invalid project writes nothing");
}

/// A frozen page's reference identities are scanned at freeze and the audit rows stay while the page holds its references; a terminal page drops them with the references, keeping only the slot and attempt identities the compact row retains.
#[test]
fn a_terminal_page_releases_the_scan_audit_of_its_references() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
    let page = FrozenSelectionPage {
        references: vec![inputs("cand-1"), inputs("cand-2")],
        next_cursor: Some("cursor-1".to_string()),
    };
    store
        .freeze_selection("proj", "slot-1", "attempt-1", &page, NOW)
        .unwrap();
    let scans = |store: &MemoryStore| -> Vec<String> {
        store
            .with_conn_for_test(|conn| {
                conn.prepare("SELECT DISTINCT field_id FROM scan_owner_copies ORDER BY field_id")?
                    .query_map([], |row| row.get(0))?
                    .collect()
            })
            .unwrap()
    };
    let frozen = scans(&store);
    assert!(
        frozen.iter().any(|field| field == "candidate_id"),
        "{frozen:?}"
    );
    store
        .complete_frozen_selection(
            "proj",
            "slot-1",
            "attempt-1",
            FrozenSelectionState::FailedSlot,
            NOW + 1,
        )
        .unwrap();
    assert_eq!(
        scans(&store),
        ["project", "selection_attempt", "slot_id"],
        "only the identities the compact row still retains keep their audit"
    );
}
