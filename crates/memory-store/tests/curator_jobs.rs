//! Real-store proofs for Curator review jobs and frozen selections: incarnation identity, causal dedup, capacity, frozen-page retention, atomic rollback, expiry, and nonreclaimable receipt charges.

use std::collections::BTreeMap;

use memory_store::MemoryStore;
use memory_store::curator_jobs::{
    CURATOR_JOB_ALLOWANCE_BYTES, CURATOR_QUEUE_LIFETIME_MS, CURATOR_RECEIPT_CHARGE_BYTES,
    CausalInputs, CuratorJobError, CuratorJobInput, CuratorJobOutcome, CuratorJobRefusal,
    CuratorJobState, EvidenceAvailability, FROZEN_SELECTION_ALLOWANCE_BYTES, FrozenSelectionPage,
    FrozenSelectionState, MAX_FROZEN_SELECTIONS_PER_HOST, MAX_PENDING_CURATOR_JOBS_PER_HOST,
    MAX_PENDING_CURATOR_JOBS_PER_PROJECT, MAX_SELECTION_REFERENCES, ProducerBinding,
    ReserveOutcome, ReviewTarget, activate_curator_job_in_tx, reserve_curator_job_in_tx,
};
use storage::StorageDescriptor;

const NOW: i64 = 1_700_000_000_000;
const AWS_KEY: &str = "AKIAQ7RSTUVWXYZ23456";

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
            CuratorJobOutcome::Abstained,
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
                CuratorJobState::Terminal(CuratorJobOutcome::Abstained)
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
        CuratorJobState::Terminal(CuratorJobOutcome::Abstained)
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
    let mut oversized = input("cand-1");
    oversized.starting_references = vec!["m".repeat(200); 8];
    oversized.question_template = "q".repeat(256);
    let _ = oversized;
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
    assert_eq!(store.ready_curator_jobs("proj", 10).unwrap(), vec![ready]);
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
    assert_eq!(store.ready_curator_jobs("proj", 10).unwrap().len(), 1);
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
        .finish_curator_job("proj", &first, CuratorJobOutcome::Completed, NOW + 1)
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
    assert_eq!(store.expire_curator_work(deadline).unwrap(), (2, 1));
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
        selection.page.next_cursor,
        Some("cursor-2".to_string()),
        "expiry does not advance the cursor"
    );
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
        3 * CURATOR_RECEIPT_CHARGE_BYTES + CURATOR_JOB_ALLOWANCE_BYTES,
        "every admitted job keeps its permanent receipt charge"
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
    assert_eq!(
        store
            .freeze_selection("proj", "slot-1", "attempt-1", &page, NOW + 5)
            .unwrap(),
        frozen,
        "the same attempt replays its page"
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
    let enqueued = store
        .complete_frozen_selection(
            "proj",
            "slot-1",
            "attempt-1",
            FrozenSelectionState::Enqueued,
            NOW + 10,
        )
        .unwrap();
    assert_eq!(enqueued.state, FrozenSelectionState::Enqueued);
    assert_eq!(enqueued.page.next_cursor, Some("cursor-9".to_string()));
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
    assert_eq!(
        refusal(
            store
                .complete_frozen_selection(
                    "proj-1",
                    "slot-1",
                    "attempt-1",
                    FrozenSelectionState::Enqueued,
                    NOW + CURATOR_QUEUE_LIFETIME_MS
                )
                .unwrap_err()
        ),
        CuratorJobRefusal::Expired
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
    // A firing id carrying a secret is refused rather than redacted, and no row is written.
    let secret_firing = producer(&format!("firing-{AWS_KEY}"));
    assert!(matches!(
        store
            .reserve_curator_job("proj", &secret_firing, &inputs("cand-2"), NOW)
            .unwrap_err(),
        CuratorJobError::Store(_)
    ));
    assert!(
        store
            .lookup_curator_job("proj", &inputs("cand-2").causal_identity().unwrap())
            .unwrap()
            .is_none()
    );
    // Reference-only input: a starting reference carrying a secret is refused, and the row stays reserved.
    let mut secret_input = input("cand-1");
    secret_input.starting_references = vec![format!("ref-{AWS_KEY}")];
    assert!(
        store
            .activate_curator_job(
                "proj",
                &job.causal_identity,
                &producer("f1"),
                &secret_input,
                NOW
            )
            .is_err()
    );
    assert_eq!(
        store
            .lookup_curator_job("proj", &job.causal_identity)
            .unwrap()
            .unwrap()
            .state,
        CuratorJobState::Reserved
    );
    let stored: Vec<Option<String>> = store
        .with_conn_for_test(|conn| {
            conn.query_row(
                "SELECT input_json, firing_id FROM curator_jobs WHERE project = 'proj'",
                [],
                |row| Ok(vec![row.get(0)?, row.get(1)?]),
            )
        })
        .unwrap();
    assert!(stored.iter().flatten().all(|text| !text.contains(AWS_KEY)));
}
