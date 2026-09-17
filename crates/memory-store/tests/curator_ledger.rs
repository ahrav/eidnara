//! Real-store proofs for the Curator receipt and attempt ledger: marker ordering around the commit-then-handoff seam, commit-failure refusal, charged-unsent expiry, reopen and takeover fencing, atomic completion, and four-attempt exhaustion across generations.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use memory_store::curator_jobs::{
    CURATOR_QUEUE_LIFETIME_MS, CausalInputs, CuratorJobInput, CuratorJobOutcome, CuratorJobState,
    EvidenceAvailability, ProducerBinding, ReserveOutcome, ReviewTarget,
};
use memory_store::curator_ledger::{
    AttemptMarker, CURATOR_ATTEMPT_MAX_MS, CURATOR_MAX_ATTEMPTS, CURATOR_RUN_DEADLINE_MS,
    CURATOR_SETTLEMENT_RESERVE_MS, CURATOR_TASK_LEASE_MS, CuratorAttemptTerminal,
    CuratorBeginOutcome, CuratorLedgerError, CuratorLedgerRefusal, CuratorReceiptTerminal,
    DispatchOutcome, ResultSelection,
};
use memory_store::{LeaseAcquireOutcome, LeaseCompleteOutcome, MemoryStore, MemoryStoreError};
use storage::StorageDescriptor;

const PROJECT: &str = "proj";
const T0: i64 = 1_700_000_000_000;
const KERNEL: &str = "0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a";

fn descriptor(dir: &std::path::Path) -> StorageDescriptor {
    MemoryStore::test_descriptor(dir, "eidnara-curator-ledger-test")
}

struct Fixture {
    dir: tempfile::TempDir,
    store: MemoryStore,
    registration: i64,
    identity: String,
}

impl Fixture {
    fn open() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
        let registration = activate_memories(&store);
        let identity = ready_job(&store, "cand-1");
        Self {
            dir,
            store,
            registration,
            identity,
        }
    }

    fn reopen(&mut self) {
        let dir = std::mem::replace(&mut self.dir, tempfile::tempdir().unwrap());
        let placeholder = std::mem::replace(
            &mut self.store,
            MemoryStore::open(&descriptor(self.dir.path())).unwrap(),
        );
        drop(placeholder);
        self.store = MemoryStore::open(&descriptor(dir.path())).unwrap();
        self.dir = dir;
    }

    fn claim(&self, acquisition: &str, worker: &str, now: i64) -> Option<String> {
        match self
            .store
            .acquire_curator_task(
                PROJECT,
                acquisition,
                worker,
                0,
                self.registration,
                &self.identity,
                now,
            )
            .unwrap()
        {
            LeaseAcquireOutcome::Claim { claim, task, .. } => {
                assert_eq!(task, self.identity);
                Some(claim.claim_id)
            }
            LeaseAcquireOutcome::NoWork { .. } => None,
            other => panic!("unexpected acquisition outcome {other:?}"),
        }
    }

    fn begin(&self, claim: &str, now: i64) -> CuratorBeginOutcome {
        self.store
            .begin_curator_receipt(PROJECT, &self.identity, KERNEL, claim, now)
            .unwrap()
    }

    fn dispatch(
        &self,
        generation: u64,
        claim: &str,
        now: i64,
    ) -> Result<DispatchOutcome<&'static str>, CuratorLedgerError> {
        self.store.dispatch_curator_attempt(
            PROJECT,
            &self.identity,
            generation,
            claim,
            KERNEL,
            &marker(generation),
            "prepared-request",
            || now,
            |prepared| prepared,
        )
    }

    fn attempts(&self) -> usize {
        self.store
            .list_curator_attempts(PROJECT, &self.identity)
            .unwrap()
            .len()
    }
}

fn activate_memories(store: &MemoryStore) -> i64 {
    let preparing = store
        .authority_begin_prepare("ctx", PROJECT, "memories")
        .unwrap();
    let row = store
        .authority_finish_prepare(
            "ctx",
            PROJECT,
            "memories",
            preparing.generation,
            "hash",
            "hash",
            true,
        )
        .unwrap();
    i64::try_from(row.generation).unwrap()
}

/// Drains the `memories` authority and re-activates it at the next generation, fencing every live claim.
fn rotate_memories_authority(store: &MemoryStore, now: i64) -> i64 {
    let draining = store
        .authority_begin_drain("ctx", PROJECT, "memories", "lease", now + 1_000_000, now)
        .unwrap();
    let token = draining
        .coordinator_token
        .clone()
        .expect("coordinator token");
    for step in [
        "seed",
        "memories",
        "notes",
        "history_segments",
        "reconcile",
        "verify",
    ] {
        store
            .authority_drain_step(
                "ctx",
                PROJECT,
                "memories",
                draining.generation,
                step,
                Some(0),
                &token,
                now,
            )
            .unwrap();
    }
    store
        .authority_finish_drain(
            "ctx",
            PROJECT,
            "memories",
            draining.generation,
            "hash",
            "hash",
            true,
            &token,
            now,
        )
        .unwrap();
    activate_memories(store)
}

fn subject(candidate: &str) -> ReviewTarget {
    ReviewTarget::StagedSubject {
        kernel_incarnation: KERNEL.to_string(),
        candidate_id: candidate.to_string(),
        payload_digest: "0d".repeat(32),
    }
}

fn ready_job(store: &MemoryStore, candidate: &str) -> String {
    let producer = ProducerBinding {
        producer: "history-summarizer".to_string(),
        firing_id: format!("firing-{candidate}"),
        ordinal: 0,
    };
    let inputs = CausalInputs {
        target: subject(candidate),
        question_template: "extracted_facts".to_string(),
        signals: vec![],
        required_evidence: vec![EvidenceAvailability {
            evidence_id: "ev-1".to_string(),
            available: true,
        }],
        policy_versions: BTreeMap::new(),
    };
    let ReserveOutcome::Reserved(job) = store
        .reserve_curator_job(PROJECT, &producer, &inputs, T0)
        .unwrap()
    else {
        panic!("fresh inputs reserve")
    };
    store
        .activate_curator_job(
            PROJECT,
            &job.causal_identity,
            &producer,
            &CuratorJobInput {
                subject: subject(candidate),
                starting_references: vec![],
                question_template: "extracted_facts".to_string(),
            },
            T0,
        )
        .unwrap();
    job.causal_identity
}

fn marker(generation: u64) -> AttemptMarker {
    AttemptMarker {
        body_digest: format!("{generation:064}"),
        request_bytes: 4_096,
        provider: "anthropic".to_string(),
        model: "claude".to_string(),
        credential_id: "cred-1".to_string(),
        policy_union_digest: "e".repeat(64),
    }
}

fn refusal(error: CuratorLedgerError) -> CuratorLedgerRefusal {
    match error {
        CuratorLedgerError::Refused(refusal) => refusal,
        CuratorLedgerError::Store(error) => panic!("store error instead of refusal: {error:?}"),
    }
}

#[test]
fn first_claim_fixes_both_deadlines_and_a_takeover_inherits_them() {
    let mut fixture = Fixture::open();
    let claim = fixture.claim("acq-1", "worker-a", T0).unwrap();
    let CuratorBeginOutcome::Begun(receipt) = fixture.begin(&claim, T0) else {
        panic!("first claim begins")
    };
    assert_eq!(receipt.generation, 1);
    assert_eq!(receipt.run_deadline_ms, T0 + CURATOR_RUN_DEADLINE_MS);
    assert_eq!(
        receipt.execution_cutoff_ms,
        T0 + CURATOR_RUN_DEADLINE_MS - CURATOR_SETTLEMENT_RESERVE_MS
    );
    assert_eq!(
        receipt.database_incarnation_id,
        fixture.store.curator_store_incarnation().unwrap()
    );
    assert!(matches!(
        fixture.begin(&claim, T0 + 1),
        CuratorBeginOutcome::InProgress(_)
    ));
    // Another store incarnation or Kernel incarnation cannot adopt the receipt.
    assert_eq!(
        refusal(
            fixture
                .store
                .begin_curator_receipt(PROJECT, &fixture.identity, &"0b".repeat(16), &claim, T0)
                .unwrap_err()
        ),
        CuratorLedgerRefusal::BindingMismatch
    );
    assert!(
        fixture.claim("acq-2", "worker-b", T0 + 1).is_none(),
        "the task is held"
    );

    // The worker crashes; its claim lapses; a successor takes over after a reopen and inherits both deadlines.
    fixture.reopen();
    let later = T0 + CURATOR_TASK_LEASE_MS + 1;
    let successor = fixture.claim("acq-3", "worker-b", later).unwrap();
    assert_eq!(
        refusal(
            fixture
                .store
                .take_over_curator_receipt(PROJECT, &fixture.identity, 1, &claim, later)
                .unwrap_err()
        ),
        CuratorLedgerRefusal::ClaimInvalid,
        "an expired claim cannot carry a takeover"
    );
    let taken = fixture
        .store
        .take_over_curator_receipt(PROJECT, &fixture.identity, 1, &successor, later)
        .unwrap();
    assert_eq!(taken.generation, 2);
    assert_eq!(taken.run_deadline_ms, receipt.run_deadline_ms);
    assert_eq!(taken.execution_cutoff_ms, receipt.execution_cutoff_ms);
    assert_eq!(
        refusal(
            fixture
                .store
                .take_over_curator_receipt(PROJECT, &fixture.identity, 1, &successor, later)
                .unwrap_err()
        ),
        CuratorLedgerRefusal::Fenced,
        "the predecessor generation is gone"
    );
    // The predecessor cannot dispatch or complete under its lost generation.
    assert_eq!(
        refusal(fixture.dispatch(1, &claim, later).unwrap_err()),
        CuratorLedgerRefusal::Fenced
    );
    assert_eq!(
        fixture
            .store
            .complete_curator_receipt(
                PROJECT,
                &fixture.identity,
                &claim,
                "completion-1",
                "worker-a",
                0,
                1,
                KERNEL,
                CuratorReceiptTerminal::Abstained,
                None,
                later
            )
            .unwrap(),
        LeaseCompleteOutcome::Conflict { kind: "fenced" }
    );
}

#[test]
fn markers_commit_before_handoff_and_every_committed_attempt_stays_consumed() {
    let mut fixture = Fixture::open();
    let claim = fixture.claim("acq-1", "worker-a", T0).unwrap();
    fixture.begin(&claim, T0);
    let events = Rc::new(RefCell::new(Vec::new()));
    let committed = Rc::clone(&events);
    // The post-commit barrier fires after COMMIT while the connection is still held, before any handoff.
    storage::after_commit_for_test(move |_| committed.borrow_mut().push("committed".to_string()));
    let seen = Rc::clone(&events);
    let outcome = fixture
        .store
        .dispatch_curator_attempt(
            PROJECT,
            &fixture.identity,
            1,
            &claim,
            KERNEL,
            &marker(1),
            "body-1",
            || T0 + 1,
            |prepared: &str| {
                seen.borrow_mut().push(format!("handoff:{prepared}"));
                prepared.len()
            },
        )
        .unwrap();
    assert!(
        matches!(
            outcome,
            DispatchOutcome::Handed {
                attempt_index: 0,
                handoff: 6,
                release: Ok(())
            }
        ),
        "{outcome:?}"
    );
    assert_eq!(
        events.borrow().as_slice(),
        ["committed", "handoff:body-1"],
        "the marker commits before the handoff runs"
    );
    assert_eq!(fixture.attempts(), 1);
    let attempt = &fixture
        .store
        .list_curator_attempts(PROJECT, &fixture.identity)
        .unwrap()[0];
    assert_eq!(attempt.attempt_deadline_ms, T0 + 1 + CURATOR_ATTEMPT_MAX_MS);
    assert_eq!(attempt.marker, marker(1));

    // A commit failure hands nothing off: the same body digest at a malformed size is refused before the insert.
    let mut bad = marker(1);
    bad.request_bytes = 0;
    let never = Rc::new(RefCell::new(0));
    let counter = Rc::clone(&never);
    let error = fixture
        .store
        .dispatch_curator_attempt(
            PROJECT,
            &fixture.identity,
            1,
            &claim,
            KERNEL,
            &bad,
            (),
            || T0 + 2,
            |()| {
                *counter.borrow_mut() += 1;
            },
        )
        .unwrap_err();
    assert_eq!(refusal(error), CuratorLedgerRefusal::InvalidRequest);
    assert_eq!(*never.borrow(), 0);
    assert_eq!(fixture.attempts(), 1);

    // Expiry between commit and handoff leaves a charged marker and sends nothing.
    let clock = RefCell::new(T0 + 3);
    let outcome = fixture
        .store
        .dispatch_curator_attempt(
            PROJECT,
            &fixture.identity,
            1,
            &claim,
            KERNEL,
            &marker(1),
            (),
            || {
                let now = *clock.borrow();
                // The clock jumps past the attempt deadline right after the marker commits.
                *clock.borrow_mut() = T0 + 3 + CURATOR_ATTEMPT_MAX_MS;
                now
            },
            |()| panic!("nothing is handed off after the deadline lapsed"),
        )
        .unwrap();
    // The claim and the attempt bound lapse together, so either recheck may fire first.
    assert!(
        matches!(
            outcome,
            DispatchOutcome::ChargedNotDispatched {
                attempt_index: 1,
                reason: CuratorLedgerRefusal::Cutoff,
                finished: true,
            }
        ),
        "the claim outlives the attempt bound, so only the attempt deadline lapsed: {outcome:?}"
    );
    assert_eq!(fixture.attempts(), 2, "the unsent marker stays consumed");
    let attempts = fixture
        .store
        .list_curator_attempts(PROJECT, &fixture.identity)
        .unwrap();
    assert_eq!(
        attempts[1].terminal.map(|(kind, _)| kind),
        Some(CuratorAttemptTerminal::NotDispatched),
        "the dispatch path records the no-disclosure proof itself"
    );
    assert_eq!(
        refusal(
            fixture
                .store
                .finish_curator_attempt(
                    PROJECT,
                    &fixture.identity,
                    1,
                    &claim,
                    1,
                    CuratorAttemptTerminal::Failed,
                    T0 + 5
                )
                .unwrap_err()
        ),
        CuratorLedgerRefusal::AttemptTerminal
    );
    // A stale claim cannot stamp an outcome on the sent attempt; the owning claim can, once.
    assert_eq!(
        refusal(
            fixture
                .store
                .finish_curator_attempt(
                    PROJECT,
                    &fixture.identity,
                    1,
                    "crc:someone-else",
                    0,
                    CuratorAttemptTerminal::Complete,
                    T0 + 5
                )
                .unwrap_err()
        ),
        CuratorLedgerRefusal::AttemptTerminal
    );
    // A commit that fails inside the insert statement hands nothing off and charges nothing: the credential carries a secret the table trigger refuses and the Rust pre-check does not scan.
    let mut long_model = marker(1);
    long_model.credential_id = format!("cred-{AWS_KEY}");
    let counter = Rc::clone(&never);
    assert!(matches!(
        fixture
            .store
            .dispatch_curator_attempt(
                PROJECT,
                &fixture.identity,
                1,
                &claim,
                KERNEL,
                &long_model,
                (),
                || T0 + 6,
                |()| {
                    *counter.borrow_mut() += 1;
                }
            )
            .unwrap_err(),
        CuratorLedgerError::Store(_)
    ));
    assert_eq!(*never.borrow(), 0);
    assert_eq!(fixture.attempts(), 2);
    // A sent attempt left unterminated is unknown: reopen changes nothing and no path finishes or redispatches it.
    let before = fixture
        .store
        .list_curator_attempts(PROJECT, &fixture.identity)
        .unwrap();
    assert_eq!(before[0].terminal, None);
    fixture.reopen();
    assert_eq!(
        fixture
            .store
            .list_curator_attempts(PROJECT, &fixture.identity)
            .unwrap(),
        before,
        "an ambiguous marker survives reopen exactly as committed"
    );
    // Cancellation withholds the handoff too, and refuses further markers.
    fixture
        .store
        .cancel_curator_receipt(PROJECT, &fixture.identity, T0 + 6)
        .unwrap();
    assert_eq!(
        refusal(fixture.dispatch(1, &claim, T0 + 7).unwrap_err()),
        CuratorLedgerRefusal::Cancelled
    );
    assert_eq!(fixture.attempts(), 2);
}

#[test]
fn four_attempts_across_generations_exhaust_the_allowance_and_the_cutoff_starts_none() {
    let fixture = Fixture::open();
    let claim = fixture.claim("acq-1", "worker-a", T0).unwrap();
    fixture.begin(&claim, T0);
    assert!(matches!(
        fixture.dispatch(1, &claim, T0 + 1).unwrap(),
        DispatchOutcome::Handed {
            attempt_index: 0,
            ..
        }
    ));
    assert!(matches!(
        fixture.dispatch(1, &claim, T0 + 2).unwrap(),
        DispatchOutcome::Handed {
            attempt_index: 1,
            ..
        }
    ));
    // The first worker's claim lapses; a successor inherits the remaining allowance, not a fresh one.
    let later = T0 + CURATOR_TASK_LEASE_MS + 1;
    let successor = fixture.claim("acq-2", "worker-b", later).unwrap();
    fixture
        .store
        .take_over_curator_receipt(PROJECT, &fixture.identity, 1, &successor, later)
        .unwrap();
    assert!(matches!(
        fixture.dispatch(2, &successor, later + 1).unwrap(),
        DispatchOutcome::Handed {
            attempt_index: 0,
            ..
        }
    ));
    assert!(matches!(
        fixture.dispatch(2, &successor, later + 2).unwrap(),
        DispatchOutcome::Handed {
            attempt_index: 1,
            ..
        }
    ));
    assert_eq!(
        fixture.attempts(),
        usize::try_from(CURATOR_MAX_ATTEMPTS).unwrap()
    );
    assert_eq!(
        refusal(fixture.dispatch(2, &successor, later + 3).unwrap_err()),
        CuratorLedgerRefusal::AttemptsExhausted,
        "no fifth attempt commits across generations"
    );
    assert_eq!(fixture.attempts(), 4);

    // A fresh job at the cutoff starts nothing, and an attempt bound is clipped to the cutoff.
    let cutoff = T0 + CURATOR_RUN_DEADLINE_MS - CURATOR_SETTLEMENT_RESERVE_MS;
    let identity = ready_job(&fixture.store, "cand-2");
    let claim = match fixture
        .store
        .acquire_curator_task(
            PROJECT,
            "acq-3",
            "worker-c",
            1,
            fixture.registration,
            &identity,
            T0,
        )
        .unwrap()
    {
        LeaseAcquireOutcome::Claim { claim, .. } => claim.claim_id,
        other => panic!("{other:?}"),
    };
    fixture
        .store
        .begin_curator_receipt(PROJECT, &identity, KERNEL, &claim, T0)
        .unwrap();
    // A live worker renews between attempts; renewal moves the claim, never the receipt's deadlines.
    let mut tick = T0;
    while tick < cutoff - 5_000 {
        tick = (tick + CURATOR_TASK_LEASE_MS - 1_000).min(cutoff - 5_000);
        let renewed = fixture
            .store
            .renew_curator_task(PROJECT, &claim, "worker-c", 1, fixture.registration, tick)
            .unwrap();
        assert!(matches!(
            renewed,
            memory_store::NoteEvalRenewOutcome::Renewed { .. }
        ));
    }
    let near = fixture
        .store
        .dispatch_curator_attempt(
            PROJECT,
            &identity,
            1,
            &claim,
            KERNEL,
            &marker(1),
            (),
            || cutoff - 5_000,
            |()| (),
        )
        .unwrap();
    assert!(matches!(
        near,
        DispatchOutcome::Handed {
            attempt_index: 0,
            ..
        }
    ));
    assert_eq!(
        fixture
            .store
            .list_curator_attempts(PROJECT, &identity)
            .unwrap()[0]
            .attempt_deadline_ms,
        cutoff,
        "an attempt never outlives the cutoff"
    );
    // The attempt that ran to its bound still completes under its claim.
    let finished = fixture
        .store
        .complete_curator_receipt(
            PROJECT,
            &identity,
            &claim,
            "completion-near",
            "worker-c",
            1,
            1,
            KERNEL,
            CuratorReceiptTerminal::Abstained,
            None,
            cutoff - 1,
        )
        .unwrap();
    assert!(
        matches!(finished, LeaseCompleteOutcome::Applied { .. }),
        "{finished:?}"
    );
    assert_eq!(
        refusal(
            fixture
                .store
                .dispatch_curator_attempt(
                    PROJECT,
                    &identity,
                    1,
                    &claim,
                    KERNEL,
                    &marker(1),
                    (),
                    || cutoff,
                    |()| ()
                )
                .unwrap_err()
        ),
        CuratorLedgerRefusal::Fenced,
        "a completed receipt takes no attempt, and the cutoff would refuse one anyway"
    );
}

#[test]
fn completion_selects_one_result_atomically_with_the_lease_and_fences_losers() {
    let fixture = Fixture::open();
    let claim = fixture.claim("acq-1", "worker-a", T0).unwrap();
    fixture.begin(&claim, T0);
    let selection = ResultSelection {
        candidate_id: "review-result:abc".to_string(),
        payload_digest: "f".repeat(64),
    };
    // A completion without a selection cannot claim `complete`, and vice versa.
    assert_eq!(
        fixture
            .store
            .complete_curator_receipt(
                PROJECT,
                &fixture.identity,
                &claim,
                "c-0",
                "worker-a",
                0,
                1,
                KERNEL,
                CuratorReceiptTerminal::Complete,
                None,
                T0 + 1
            )
            .unwrap(),
        LeaseCompleteOutcome::Conflict { kind: "invalid" }
    );
    // The wrong generation writes nothing.
    assert_eq!(
        fixture
            .store
            .complete_curator_receipt(
                PROJECT,
                &fixture.identity,
                &claim,
                "c-1",
                "worker-a",
                0,
                2,
                KERNEL,
                CuratorReceiptTerminal::Complete,
                Some(&selection),
                T0 + 1
            )
            .unwrap(),
        LeaseCompleteOutcome::Conflict { kind: "fenced" }
    );
    assert!(
        fixture
            .store
            .lookup_curator_receipt(PROJECT, &fixture.identity)
            .unwrap()
            .unwrap()
            .terminal
            .is_none()
    );
    let applied = fixture
        .store
        .complete_curator_receipt(
            PROJECT,
            &fixture.identity,
            &claim,
            "c-2",
            "worker-a",
            0,
            1,
            KERNEL,
            CuratorReceiptTerminal::Complete,
            Some(&selection),
            T0 + 2,
        )
        .unwrap();
    assert!(
        matches!(applied, LeaseCompleteOutcome::Applied { .. }),
        "{applied:?}"
    );
    let receipt = fixture
        .store
        .lookup_curator_receipt(PROJECT, &fixture.identity)
        .unwrap()
        .unwrap();
    assert_eq!(receipt.terminal, Some(CuratorReceiptTerminal::Complete));
    assert_eq!(receipt.selected, Some((1, selection.clone())));
    assert_eq!(
        fixture
            .store
            .lookup_curator_job(PROJECT, &fixture.identity)
            .unwrap()
            .unwrap()
            .state,
        CuratorJobState::Terminal(CuratorJobOutcome::Completed),
        "the job outcome commits with the receipt and the lease"
    );
    // The same completion replays; a second completion is a conflict; the task is no longer leasable.
    assert!(matches!(
        fixture
            .store
            .complete_curator_receipt(
                PROJECT,
                &fixture.identity,
                &claim,
                "c-2",
                "worker-a",
                0,
                1,
                KERNEL,
                CuratorReceiptTerminal::Complete,
                Some(&selection),
                T0 + 3
            )
            .unwrap(),
        LeaseCompleteOutcome::Replayed { .. }
    ));
    assert!(matches!(
        fixture
            .store
            .complete_curator_receipt(
                PROJECT,
                &fixture.identity,
                &claim,
                "c-3",
                "worker-a",
                0,
                1,
                KERNEL,
                CuratorReceiptTerminal::Failed,
                None,
                T0 + 4
            )
            .unwrap(),
        LeaseCompleteOutcome::Conflict { .. }
    ));
    assert!(fixture.claim("acq-9", "worker-z", T0 + 5).is_none());
    assert!(matches!(
        fixture.begin(&claim, T0 + 6),
        CuratorBeginOutcome::Complete(_)
    ));
    assert_eq!(
        refusal(fixture.dispatch(1, &claim, T0 + 7).unwrap_err()),
        CuratorLedgerRefusal::Fenced
    );
}

/// A bounded transition table over charged-not-dispatched attempts, takeover, expiry, reopen, and recovery. Every sequence must keep the consumed total at or under four across generations, never commit a fifth, never move either absolute deadline, and complete only through the current generation.
#[test]
fn bounded_transition_sequences_preserve_allowance_deadlines_and_generation_fencing() {
    #[derive(Clone, Copy, Debug)]
    enum Step {
        Dispatch,
        ChargedUnsent,
        Reopen,
        Takeover,
        Complete,
    }
    let sequences: &[&[Step]] = &[
        &[
            Step::Dispatch,
            Step::ChargedUnsent,
            Step::Takeover,
            Step::Dispatch,
            Step::Dispatch,
            Step::Dispatch,
            Step::Complete,
        ],
        &[
            Step::ChargedUnsent,
            Step::ChargedUnsent,
            Step::Reopen,
            Step::Takeover,
            Step::ChargedUnsent,
            Step::ChargedUnsent,
            Step::Dispatch,
        ],
        &[
            Step::Takeover,
            Step::Reopen,
            Step::Takeover,
            Step::Dispatch,
            Step::Complete,
            Step::Dispatch,
            Step::Takeover,
        ],
        &[
            Step::Dispatch,
            Step::Reopen,
            Step::Dispatch,
            Step::Complete,
            Step::Complete,
        ],
    ];
    for (index, sequence) in sequences.iter().enumerate() {
        let mut fixture = Fixture::open();
        let mut now = T0;
        let mut generation = 1u64;
        let mut worker = 0u32;
        let mut claim = fixture.claim("acq-0", "worker-0", now).unwrap();
        let CuratorBeginOutcome::Begun(first) = fixture.begin(&claim, now) else {
            panic!("sequence {index}: first claim begins")
        };
        let mut completed = false;
        for step in *sequence {
            now += 1;
            match step {
                Step::Dispatch | Step::ChargedUnsent => {
                    let charged_unsent = matches!(step, Step::ChargedUnsent);
                    let before = fixture.attempts();
                    let reads = RefCell::new(0u32);
                    let result = fixture.store.dispatch_curator_attempt(
                        PROJECT,
                        &fixture.identity,
                        generation,
                        &claim,
                        KERNEL,
                        &marker(generation),
                        (),
                        || {
                            // The first read commits the marker; a charged-unsent step lapses before the recheck.
                            *reads.borrow_mut() += 1;
                            if charged_unsent && *reads.borrow() > 1 {
                                now + CURATOR_ATTEMPT_MAX_MS
                            } else {
                                now
                            }
                        },
                        |()| (),
                    );
                    match result {
                        Ok(DispatchOutcome::Handed { .. }) => {
                            assert!(!charged_unsent && !completed)
                        }
                        Ok(DispatchOutcome::ChargedNotDispatched {
                            reason, finished, ..
                        }) => {
                            assert!(charged_unsent);
                            assert!(matches!(
                                reason,
                                CuratorLedgerRefusal::Cutoff | CuratorLedgerRefusal::ClaimInvalid
                            ));
                            assert!(finished, "the withheld marker is finished not_dispatched");
                        }
                        Err(error) => {
                            let reason = refusal(error);
                            assert!(
                                completed && reason == CuratorLedgerRefusal::Fenced
                                    || before >= 4
                                        && reason == CuratorLedgerRefusal::AttemptsExhausted
                                    || charged_unsent && reason == CuratorLedgerRefusal::Cutoff,
                                "sequence {index}: unexpected refusal {reason:?} at {step:?}"
                            );
                            assert_eq!(
                                fixture.attempts(),
                                before,
                                "a refused attempt is never charged"
                            );
                        }
                    }
                }
                Step::Reopen => fixture.reopen(),
                Step::Takeover => {
                    now += CURATOR_TASK_LEASE_MS;
                    worker += 1;
                    let Some(successor) =
                        fixture.claim(&format!("acq-{worker}"), &format!("worker-{worker}"), now)
                    else {
                        assert!(
                            completed,
                            "sequence {index}: a live job stays leasable after the claim lapses"
                        );
                        continue;
                    };
                    match fixture.store.take_over_curator_receipt(
                        PROJECT,
                        &fixture.identity,
                        generation,
                        &successor,
                        now,
                    ) {
                        Ok(receipt) => {
                            generation = receipt.generation;
                            claim = successor;
                        }
                        Err(error) => {
                            assert!(completed && refusal(error) == CuratorLedgerRefusal::Fenced)
                        }
                    }
                }
                Step::Complete => {
                    let outcome = fixture
                        .store
                        .complete_curator_receipt(
                            PROJECT,
                            &fixture.identity,
                            &claim,
                            &format!("completion-{generation}"),
                            &format!("worker-{worker}"),
                            0,
                            generation,
                            KERNEL,
                            CuratorReceiptTerminal::Abstained,
                            None,
                            now,
                        )
                        .unwrap();
                    match outcome {
                        LeaseCompleteOutcome::Applied { .. } => {
                            assert!(!completed);
                            completed = true;
                        }
                        LeaseCompleteOutcome::Replayed { .. }
                        | LeaseCompleteOutcome::Conflict { .. } => {}
                    }
                }
            }
            let receipt = fixture
                .store
                .lookup_curator_receipt(PROJECT, &fixture.identity)
                .unwrap()
                .unwrap();
            assert_eq!(
                receipt.run_deadline_ms, first.run_deadline_ms,
                "sequence {index}: the run deadline never moves"
            );
            assert_eq!(
                receipt.execution_cutoff_ms, first.execution_cutoff_ms,
                "sequence {index}: the cutoff never moves"
            );
            assert!(
                fixture.attempts() <= 4,
                "sequence {index}: at most four committed attempts"
            );
            assert_eq!(receipt.terminal.is_some(), completed);
            if completed {
                assert_eq!(
                    receipt.generation, generation,
                    "sequence {index}: only the current generation completes"
                );
            }
        }
    }
}

#[test]
fn the_sweep_releases_a_job_whose_receipt_was_orphaned_by_a_crashed_worker() {
    let fixture = Fixture::open();
    let claim = fixture.claim("acq-1", "worker-a", T0).unwrap();
    let CuratorBeginOutcome::Begun(receipt) = fixture.begin(&claim, T0) else {
        panic!("first claim begins")
    };
    // One attempt is handed off and never finished: its outcome is unknown.
    assert!(matches!(
        fixture.dispatch(1, &claim, T0 + 1).unwrap(),
        DispatchOutcome::Handed { .. }
    ));
    // A second job's worker crashes before any attempt.
    let untouched = ready_job(&fixture.store, "cand-2");
    let other = match fixture
        .store
        .acquire_curator_task(
            PROJECT,
            "acq-2",
            "worker-b",
            0,
            fixture.registration,
            &untouched,
            T0 + 2,
        )
        .unwrap()
    {
        LeaseAcquireOutcome::Claim { claim, .. } => claim.claim_id,
        other => panic!("{other:?}"),
    };
    fixture
        .store
        .begin_curator_receipt(PROJECT, &untouched, KERNEL, &other, T0 + 2)
        .unwrap();
    let queue_deadline = T0 + CURATOR_QUEUE_LIFETIME_MS;

    // Inside the run deadline the receipt shields the job, whatever the queue deadline says.
    assert_eq!(
        fixture
            .store
            .expire_curator_work(receipt.run_deadline_ms - 1)
            .unwrap(),
        (0, 0),
        "a receipt inside its run deadline is left to its worker or a successor"
    );

    // Both workers crashed and no successor ever claimed either job. Once the queue deadline passes
    // the sweep must release the jobs and close the receipts, or each job holds its allowance and a
    // pending slot for the rest of the store incarnation.
    assert_eq!(
        fixture.store.expire_curator_work(queue_deadline).unwrap(),
        (2, 0),
        "an orphaned receipt past its run deadline no longer shields an expired job"
    );
    let job = fixture
        .store
        .lookup_curator_job(PROJECT, &fixture.identity)
        .unwrap()
        .unwrap();
    assert_eq!(
        job.state,
        CuratorJobState::Terminal(CuratorJobOutcome::Unknown),
        "an unterminated attempt leaves the job outcome unknown, not expired"
    );
    let closed = fixture
        .store
        .lookup_curator_receipt(PROJECT, &fixture.identity)
        .unwrap()
        .unwrap();
    assert_eq!(closed.terminal, Some(CuratorReceiptTerminal::Unknown));
    assert_eq!(closed.run_deadline_ms, receipt.run_deadline_ms);
    assert_eq!(closed.execution_cutoff_ms, receipt.execution_cutoff_ms);
    assert_eq!(
        fixture
            .store
            .lookup_curator_job(PROJECT, &untouched)
            .unwrap()
            .unwrap()
            .state,
        CuratorJobState::Terminal(CuratorJobOutcome::Expired),
        "a receipt with no attempt closes as expired"
    );
    assert_eq!(
        fixture
            .store
            .lookup_curator_receipt(PROJECT, &untouched)
            .unwrap()
            .unwrap()
            .terminal,
        Some(CuratorReceiptTerminal::Expired)
    );
    // A late completion by the crashed worker's claim is fenced by the closed receipt.
    assert_eq!(
        fixture
            .store
            .complete_curator_receipt(
                PROJECT,
                &fixture.identity,
                &claim,
                "late",
                "worker-a",
                0,
                1,
                KERNEL,
                CuratorReceiptTerminal::Abstained,
                None,
                queue_deadline + 1,
            )
            .unwrap(),
        LeaseCompleteOutcome::Conflict { kind: "expired" },
        "the crashed worker's claim expired long before the sweep"
    );
    // Both pending slots and both allowances are released; only the permanent receipt charges remain.
    let headroom = fixture.store.curator_headroom(PROJECT).unwrap();
    assert_eq!(headroom.pending_jobs, 0);
    assert_eq!(
        headroom.project_metadata_bytes,
        2 * memory_store::curator_jobs::CURATOR_RECEIPT_CHARGE_BYTES
    );
}

#[test]
fn a_claim_on_another_job_cannot_begin_take_over_or_attempt_this_receipt() {
    let fixture = Fixture::open();
    // Worker A holds a live claim on job B only.
    let job_b = ready_job(&fixture.store, "cand-b");
    let claim_b = match fixture
        .store
        .acquire_curator_task(
            PROJECT,
            "acq-b",
            "worker-a",
            0,
            fixture.registration,
            &job_b,
            T0,
        )
        .unwrap()
    {
        LeaseAcquireOutcome::Claim { claim, task, .. } => {
            assert_eq!(task, job_b);
            claim.claim_id
        }
        other => panic!("{other:?}"),
    };
    // That claim carries no authority over job A: its receipt cannot begin under it.
    assert_eq!(
        refusal(
            fixture
                .store
                .begin_curator_receipt(PROJECT, &fixture.identity, KERNEL, &claim_b, T0 + 1)
                .unwrap_err()
        ),
        CuratorLedgerRefusal::ClaimInvalid,
        "a claim binds one job; another job's receipt refuses it"
    );
    assert!(
        fixture
            .store
            .lookup_curator_receipt(PROJECT, &fixture.identity)
            .unwrap()
            .is_none()
    );
    // Job A's own claim begins its receipt; job B's claim cannot take it over or attempt under it.
    let claim_a = fixture.claim("acq-a", "worker-c", T0 + 2).unwrap();
    fixture.begin(&claim_a, T0 + 2);
    assert_eq!(
        refusal(
            fixture
                .store
                .take_over_curator_receipt(PROJECT, &fixture.identity, 1, &claim_b, T0 + 3)
                .unwrap_err()
        ),
        CuratorLedgerRefusal::ClaimInvalid
    );
    assert_eq!(
        refusal(fixture.dispatch(1, &claim_b, T0 + 4).unwrap_err()),
        CuratorLedgerRefusal::ClaimInvalid
    );
    assert_eq!(fixture.attempts(), 0);
    // Job A's own claim still works, so the binding check refuses only foreign claims.
    assert!(matches!(
        fixture.dispatch(1, &claim_a, T0 + 5).unwrap(),
        DispatchOutcome::Handed { .. }
    ));
}

#[test]
fn a_failure_after_the_marker_commits_is_a_charged_unsent_attempt_not_a_failed_commit() {
    let fixture = Fixture::open();
    let claim = fixture.claim("acq-1", "worker-a", T0).unwrap();
    fixture.begin(&claim, T0);
    // Between COMMIT and the handoff the connection is left in a state the read-only view refuses.
    storage::after_commit_for_test(|conn| {
        conn.execute("CREATE TEMP TABLE curator_receipts (x)", [])
            .expect("plant a shadow after the commit");
    });
    let outcome = fixture
        .store
        .dispatch_curator_attempt(
            PROJECT,
            &fixture.identity,
            1,
            &claim,
            KERNEL,
            &marker(1),
            (),
            || T0 + 1,
            |()| panic!("nothing is handed off when the recheck cannot run"),
        )
        .expect("the marker committed, so the caller must not see a failed commit");
    assert!(
        matches!(
            outcome,
            DispatchOutcome::ChargedNotDispatched {
                attempt_index: 0,
                reason: CuratorLedgerRefusal::RecheckUnavailable,
                finished: false,
            }
        ),
        "the shadow still blocks the finish transaction, so the marker stays unterminated: {outcome:?}"
    );
    fixture
        .store
        .execute_tag_sql_for_test("DROP TABLE temp.curator_receipts")
        .unwrap();
    let attempts = fixture
        .store
        .list_curator_attempts(PROJECT, &fixture.identity)
        .unwrap();
    assert_eq!(attempts.len(), 1, "the marker is charged");
    assert_eq!(
        attempts[0].terminal, None,
        "and stays unknown until its owner finishes it"
    );
    // Only the dispatch path may write the no-disclosure proof; the owning claim can still close the marker honestly as unknown once the store recovers.
    assert_eq!(
        refusal(
            fixture
                .store
                .finish_curator_attempt(
                    PROJECT,
                    &fixture.identity,
                    1,
                    &claim,
                    0,
                    CuratorAttemptTerminal::NotDispatched,
                    T0 + 2,
                )
                .unwrap_err()
        ),
        CuratorLedgerRefusal::InvalidRequest
    );
    fixture
        .store
        .finish_curator_attempt(
            PROJECT,
            &fixture.identity,
            1,
            &claim,
            0,
            CuratorAttemptTerminal::Unknown,
            T0 + 2,
        )
        .unwrap();
}

#[test]
fn a_clock_behind_the_newest_marker_is_refused_as_clock_behind_not_exhaustion() {
    let fixture = Fixture::open();
    let claim = fixture.claim("acq-1", "worker-a", T0).unwrap();
    fixture.begin(&claim, T0);
    assert!(matches!(
        fixture.dispatch(1, &claim, T0 + 10).unwrap(),
        DispatchOutcome::Handed { .. }
    ));
    // The same live claim reads a clock 3 ms below the newest marker; three attempts remain.
    let mut skewed = marker(1);
    skewed.body_digest = "b".repeat(64);
    let error = fixture
        .store
        .dispatch_curator_attempt(
            PROJECT,
            &fixture.identity,
            1,
            &claim,
            KERNEL,
            &skewed,
            (),
            || T0 + 7,
            |()| (),
        )
        .unwrap_err();
    assert_eq!(
        refusal(error),
        CuratorLedgerRefusal::ClockBehind,
        "a clock-floor refusal must not read as exhaustion, or the worker settles a job with attempts left"
    );
    assert_eq!(
        fixture.attempts(),
        1,
        "a clock-floor refusal charges nothing"
    );
    // Once the clock catches up the same marker commits.
    assert!(matches!(
        fixture
            .store
            .dispatch_curator_attempt(
                PROJECT,
                &fixture.identity,
                1,
                &claim,
                KERNEL,
                &skewed,
                (),
                || T0 + 10,
                |()| (),
            )
            .unwrap(),
        DispatchOutcome::Handed {
            attempt_index: 1,
            ..
        }
    ));
}

#[test]
fn ledger_writes_bound_the_project_like_every_other_curator_write() {
    let fixture = Fixture::open();
    let claim = fixture.claim("acq-1", "worker-a", T0).unwrap();
    // An empty project is malformed input, not an authority transition or an unknown claim.
    assert!(matches!(
        fixture
            .store
            .begin_curator_receipt("", &fixture.identity, KERNEL, &claim, T0)
            .unwrap_err(),
        CuratorLedgerError::Store(_)
    ));
    assert!(matches!(
        fixture
            .store
            .take_over_curator_receipt("", &fixture.identity, 1, &claim, T0)
            .unwrap_err(),
        CuratorLedgerError::Store(_)
    ));
    assert!(matches!(
        fixture
            .store
            .cancel_curator_receipt("", &fixture.identity, T0)
            .unwrap_err(),
        CuratorLedgerError::Store(_)
    ));
}

const AWS_KEY: &str = "AKIAQ7RSTUVWXYZ23456";

fn selection() -> ResultSelection {
    ResultSelection {
        candidate_id: "review-result:abc".to_string(),
        payload_digest: "f".repeat(64),
    }
}

#[allow(clippy::too_many_arguments)]
fn complete(
    fixture: &Fixture,
    identity: &str,
    claim: &str,
    completion_id: &str,
    worker: &str,
    generation: u64,
    terminal: CuratorReceiptTerminal,
    selection: Option<&ResultSelection>,
    now: i64,
) -> Result<LeaseCompleteOutcome, MemoryStoreError> {
    fixture.store.complete_curator_receipt(
        PROJECT,
        identity,
        claim,
        completion_id,
        worker,
        0,
        generation,
        KERNEL,
        terminal,
        selection,
        now,
    )
}

fn receipt(fixture: &Fixture) -> memory_store::curator_ledger::CuratorReceipt {
    fixture
        .store
        .lookup_curator_receipt(PROJECT, &fixture.identity)
        .unwrap()
        .unwrap()
}

#[test]
fn a_takeover_under_a_changed_authority_is_refused_and_cannot_publish() {
    let mut fixture = Fixture::open();
    let claim = fixture.claim("acq-1", "worker-a", T0).unwrap();
    fixture.begin(&claim, T0);
    // The `memories` authority moves; the first claim is fenced and the job stays Ready.
    fixture.registration = rotate_memories_authority(&fixture.store, T0 + 1);
    let successor = fixture.claim("acq-2", "worker-b", T0 + 1).unwrap();
    assert_eq!(
        refusal(
            fixture
                .store
                .take_over_curator_receipt(PROJECT, &fixture.identity, 1, &successor, T0 + 2)
                .unwrap_err()
        ),
        CuratorLedgerRefusal::AuthorityChanged
    );
    let r = receipt(&fixture);
    assert_eq!((r.generation, r.claim_id.as_str()), (1, claim.as_str()));
    assert_eq!(
        complete(
            &fixture,
            &fixture.identity,
            &successor,
            "c-1",
            "worker-b",
            2,
            CuratorReceiptTerminal::Complete,
            Some(&selection()),
            T0 + 3
        )
        .unwrap(),
        LeaseCompleteOutcome::Conflict { kind: "fenced" }
    );
    assert!(receipt(&fixture).terminal.is_none());
}

#[test]
fn a_completion_against_a_job_another_owner_closed_writes_nothing_to_the_receipt() {
    let fixture = Fixture::open();
    let claim = fixture.claim("acq-1", "worker-a", T0).unwrap();
    fixture.begin(&claim, T0);
    fixture
        .store
        .finish_curator_job(
            PROJECT,
            &fixture.identity,
            CuratorJobOutcome::Failed,
            T0 + 1,
        )
        .unwrap();
    assert_eq!(
        complete(
            &fixture,
            &fixture.identity,
            &claim,
            "c-1",
            "worker-a",
            1,
            CuratorReceiptTerminal::Complete,
            Some(&selection()),
            T0 + 2
        )
        .unwrap(),
        LeaseCompleteOutcome::Conflict { kind: "stale" }
    );
    let r = receipt(&fixture);
    assert_eq!((r.terminal, r.selected.clone()), (None, None), "{r:?}");
}

#[test]
fn not_dispatched_is_written_only_by_the_dispatch_path() {
    let fixture = Fixture::open();
    let claim = fixture.claim("acq-1", "worker-a", T0).unwrap();
    fixture.begin(&claim, T0);
    let DispatchOutcome::Handed { attempt_index, .. } =
        fixture.dispatch(1, &claim, T0 + 1).unwrap()
    else {
        panic!("first attempt hands off")
    };
    assert_eq!(
        refusal(
            fixture
                .store
                .finish_curator_attempt(
                    PROJECT,
                    &fixture.identity,
                    1,
                    &claim,
                    attempt_index,
                    CuratorAttemptTerminal::NotDispatched,
                    T0 + 2,
                )
                .unwrap_err()
        ),
        CuratorLedgerRefusal::InvalidRequest
    );
    let attempts = fixture
        .store
        .list_curator_attempts(PROJECT, &fixture.identity)
        .unwrap();
    assert_eq!(
        attempts[0].terminal, None,
        "a handed-off attempt keeps no false proof"
    );
}

#[test]
fn a_selected_candidate_carrying_a_secret_is_refused_at_the_receipt() {
    let fixture = Fixture::open();
    let claim = fixture.claim("acq-1", "worker-a", T0).unwrap();
    fixture.begin(&claim, T0);
    let leaked = ResultSelection {
        candidate_id: format!("review-result:{AWS_KEY}"),
        payload_digest: "f".repeat(64),
    };
    let error = complete(
        &fixture,
        &fixture.identity,
        &claim,
        "c-1",
        "worker-a",
        1,
        CuratorReceiptTerminal::Complete,
        Some(&leaked),
        T0 + 1,
    )
    .unwrap_err();
    assert!(
        matches!(
            error,
            MemoryStoreError::Redaction(
                context_core::redaction::RedactionErrorKind::SecretDetected
            )
        ),
        "{error:?}"
    );
    let r = receipt(&fixture);
    assert_eq!((r.terminal, r.selected.clone()), (None, None), "{r:?}");
}

#[test]
fn marker_and_selection_digests_must_be_lowercase_hex() {
    let fixture = Fixture::open();
    let claim = fixture.claim("acq-1", "worker-a", T0).unwrap();
    fixture.begin(&claim, T0);
    for (body, policy) in [
        ("z".repeat(64), "e".repeat(64)),
        ("e".repeat(64), "Z".repeat(64)),
    ] {
        let mut bad = marker(1);
        bad.body_digest = body;
        bad.policy_union_digest = policy;
        let error = fixture
            .store
            .dispatch_curator_attempt(
                PROJECT,
                &fixture.identity,
                1,
                &claim,
                KERNEL,
                &bad,
                "prepared",
                || T0 + 1,
                |prepared| prepared,
            )
            .unwrap_err();
        assert_eq!(refusal(error), CuratorLedgerRefusal::InvalidRequest);
    }
    assert_eq!(fixture.attempts(), 0);
    let mut bad = selection();
    bad.payload_digest = "g".repeat(64);
    assert_eq!(
        complete(
            &fixture,
            &fixture.identity,
            &claim,
            "c-1",
            "worker-a",
            1,
            CuratorReceiptTerminal::Complete,
            Some(&bad),
            T0 + 2
        )
        .unwrap(),
        LeaseCompleteOutcome::Conflict { kind: "invalid" }
    );
}

#[test]
fn a_completed_receipt_still_fences_a_claim_that_does_not_own_it() {
    let fixture = Fixture::open();
    let claim = fixture.claim("acq-1", "worker-a", T0).unwrap();
    fixture.begin(&claim, T0);
    assert!(matches!(
        complete(
            &fixture,
            &fixture.identity,
            &claim,
            "c-1",
            "worker-a",
            1,
            CuratorReceiptTerminal::Complete,
            Some(&selection()),
            T0 + 1
        )
        .unwrap(),
        LeaseCompleteOutcome::Applied { .. }
    ));
    // The same worker leases another job, then names the completed receipt with that claim.
    let other = ready_job(&fixture.store, "cand-2");
    let LeaseAcquireOutcome::Claim {
        claim: other_claim, ..
    } = fixture
        .store
        .acquire_curator_task(
            PROJECT,
            "acq-2",
            "worker-a",
            0,
            fixture.registration,
            &other,
            T0 + 2,
        )
        .unwrap()
    else {
        panic!("the second job is leasable")
    };
    assert_eq!(
        complete(
            &fixture,
            &fixture.identity,
            &other_claim.claim_id,
            "c-2",
            "worker-a",
            1,
            CuratorReceiptTerminal::Failed,
            None,
            T0 + 3
        )
        .unwrap(),
        LeaseCompleteOutcome::Conflict { kind: "fenced" }
    );
    assert!(
        matches!(
            fixture
                .store
                .renew_curator_task(
                    PROJECT,
                    &other_claim.claim_id,
                    "worker-a",
                    0,
                    fixture.registration,
                    T0 + 4
                )
                .unwrap(),
            memory_store::NoteEvalRenewOutcome::Renewed { .. }
        ),
        "the unrelated live claim survives"
    );
}

#[test]
fn a_completion_at_or_after_the_run_deadline_records_expired_not_the_worker_result() {
    let fixture = Fixture::open();
    let claim = fixture.claim("acq-1", "worker-a", T0).unwrap();
    let CuratorBeginOutcome::Begun(r) = fixture.begin(&claim, T0) else {
        panic!("first claim begins")
    };
    // A live worker renews between attempts, so its claim outlives the fixed run budget.
    let mut now = T0;
    while now + CURATOR_TASK_LEASE_MS <= r.run_deadline_ms {
        now += CURATOR_TASK_LEASE_MS / 2;
        assert!(matches!(
            fixture
                .store
                .renew_curator_task(PROJECT, &claim, "worker-a", 0, fixture.registration, now)
                .unwrap(),
            memory_store::NoteEvalRenewOutcome::Renewed { .. }
        ));
    }
    let outcome = complete(
        &fixture,
        &fixture.identity,
        &claim,
        "c-1",
        "worker-a",
        1,
        CuratorReceiptTerminal::Complete,
        Some(&selection()),
        r.run_deadline_ms,
    )
    .unwrap();
    assert_eq!(
        outcome,
        LeaseCompleteOutcome::Applied {
            response_json: "{\"terminal\":\"expired\"}".to_string()
        }
    );
    let r = receipt(&fixture);
    assert_eq!(
        (r.terminal, r.selected),
        (Some(CuratorReceiptTerminal::Expired), None)
    );
    assert_eq!(
        fixture
            .store
            .lookup_curator_job(PROJECT, &fixture.identity)
            .unwrap()
            .unwrap()
            .state,
        CuratorJobState::Terminal(CuratorJobOutcome::Expired)
    );
}

#[test]
fn the_sweep_fences_the_live_claim_of_a_job_it_expires() {
    let fixture = Fixture::open();
    let late = T0 + CURATOR_QUEUE_LIFETIME_MS - 1;
    let claim = fixture.claim("acq-1", "worker-a", late).unwrap();
    assert_eq!(fixture.store.expire_curator_work(late + 1).unwrap(), (1, 0));
    assert_eq!(
        fixture
            .store
            .renew_curator_task(
                PROJECT,
                &claim,
                "worker-a",
                0,
                fixture.registration,
                late + 2
            )
            .unwrap(),
        memory_store::NoteEvalRenewOutcome::TerminalReplay {
            kind: "expired".to_string(),
            response: Some("{\"result\":\"expired\"}".to_string())
        }
    );
}

#[test]
fn the_post_commit_recheck_withholds_a_handoff_once_the_receipt_moved_on() {
    let fixture = Fixture::open();
    let claim = fixture.claim("acq-1", "worker-a", T0).unwrap();
    fixture.begin(&claim, T0);
    // Between COMMIT and the handoff the receipt is taken over: another generation owns it now.
    storage::after_commit_for_test(|conn| {
        conn.execute(
            "UPDATE curator_receipts SET generation = generation + 1, claim_id = 'crc:other'",
            [],
        )
        .expect("plant the takeover after the commit");
    });
    let outcome = fixture.dispatch(1, &claim, T0 + 1).unwrap();
    assert!(
        matches!(
            outcome,
            DispatchOutcome::ChargedNotDispatched {
                attempt_index: 0,
                reason: CuratorLedgerRefusal::Fenced,
                ..
            }
        ),
        "{outcome:?}"
    );
    assert_eq!(fixture.attempts(), 1, "the marker stays charged");
}

#[test]
fn every_ledger_entry_point_bounds_the_project() {
    let fixture = Fixture::open();
    let claim = fixture.claim("acq-1", "worker-a", T0).unwrap();
    fixture.begin(&claim, T0);
    assert!(matches!(
        fixture
            .store
            .dispatch_curator_attempt(
                "",
                &fixture.identity,
                1,
                &claim,
                KERNEL,
                &marker(1),
                "prepared",
                || T0 + 1,
                |prepared| prepared,
            )
            .unwrap_err(),
        CuratorLedgerError::Store(_)
    ));
    assert_eq!(fixture.attempts(), 0);
    assert!(
        fixture
            .store
            .lookup_curator_receipt("", &fixture.identity)
            .is_err()
    );
    assert!(
        fixture
            .store
            .list_curator_attempts("", &fixture.identity)
            .is_err()
    );
}

#[test]
fn the_receipt_authority_binding_is_written_once() {
    let fixture = Fixture::open();
    let claim = fixture.claim("acq-1", "worker-a", T0).unwrap();
    fixture.begin(&claim, T0);
    let error = fixture
        .store
        .execute_tag_sql_for_test(
            "UPDATE curator_receipts SET authority_generation = authority_generation + 1",
        )
        .unwrap_err();
    assert!(error.to_string().contains("written once"), "{error}");
    assert_eq!(
        receipt(&fixture).authority_generation,
        fixture.registration as u64
    );
}

#[test]
fn a_complete_attempt_terminal_at_or_after_the_attempt_deadline_is_refused() {
    let fixture = Fixture::open();
    let claim = fixture.claim("acq-1", "worker-a", T0).unwrap();
    fixture.begin(&claim, T0);
    let DispatchOutcome::Handed { attempt_index, .. } =
        fixture.dispatch(1, &claim, T0 + 1).unwrap()
    else {
        panic!("first attempt hands off")
    };
    let deadline = fixture
        .store
        .list_curator_attempts(PROJECT, &fixture.identity)
        .unwrap()[0]
        .attempt_deadline_ms;
    assert_eq!(deadline, T0 + 1 + CURATOR_ATTEMPT_MAX_MS);
    let finish = |terminal, now| {
        fixture.store.finish_curator_attempt(
            PROJECT,
            &fixture.identity,
            1,
            &claim,
            attempt_index,
            terminal,
            now,
        )
    };
    assert_eq!(
        refusal(finish(CuratorAttemptTerminal::Complete, deadline).unwrap_err()),
        CuratorLedgerRefusal::Cutoff
    );
    // An honest late closure is still recorded.
    finish(CuratorAttemptTerminal::Failed, deadline).unwrap();
    assert_eq!(
        fixture
            .store
            .list_curator_attempts(PROJECT, &fixture.identity)
            .unwrap()[0]
            .terminal,
        Some((CuratorAttemptTerminal::Failed, deadline))
    );
}

#[test]
fn a_cancelled_receipt_records_cancelled_whatever_the_worker_reports() {
    let fixture = Fixture::open();
    let claim = fixture.claim("acq-1", "worker-a", T0).unwrap();
    fixture.begin(&claim, T0);
    assert!(matches!(
        fixture.dispatch(1, &claim, T0 + 1).unwrap(),
        DispatchOutcome::Handed { .. }
    ));
    fixture
        .store
        .cancel_curator_receipt(PROJECT, &fixture.identity, T0 + 2)
        .unwrap();
    let outcome = complete(
        &fixture,
        &fixture.identity,
        &claim,
        "c-1",
        "worker-a",
        1,
        CuratorReceiptTerminal::Complete,
        Some(&selection()),
        T0 + 3,
    )
    .unwrap();
    assert_eq!(
        outcome,
        LeaseCompleteOutcome::Applied {
            response_json: "{\"terminal\":\"cancelled\"}".to_string()
        }
    );
    let r = receipt(&fixture);
    assert_eq!(
        (r.terminal, r.selected),
        (Some(CuratorReceiptTerminal::Cancelled), None)
    );
    assert_eq!(
        fixture
            .store
            .lookup_curator_job(PROJECT, &fixture.identity)
            .unwrap()
            .unwrap()
            .state,
        CuratorJobState::Terminal(CuratorJobOutcome::Failed)
    );
}

#[test]
fn the_sweep_closes_a_receipt_whose_job_another_owner_already_closed() {
    let fixture = Fixture::open();
    let claim = fixture.claim("acq-1", "worker-a", T0).unwrap();
    let CuratorBeginOutcome::Begun(r) = fixture.begin(&claim, T0) else {
        panic!("first claim begins")
    };
    fixture
        .store
        .finish_curator_job(
            PROJECT,
            &fixture.identity,
            CuratorJobOutcome::Failed,
            T0 + 1,
        )
        .unwrap();
    fixture
        .store
        .expire_curator_work(r.run_deadline_ms - 1)
        .unwrap();
    assert_eq!(
        receipt(&fixture).terminal,
        None,
        "not before the run deadline"
    );
    fixture
        .store
        .expire_curator_work(r.run_deadline_ms)
        .unwrap();
    assert_eq!(
        receipt(&fixture).terminal,
        Some(CuratorReceiptTerminal::Expired)
    );
}

#[test]
fn marker_text_is_bounded_in_bytes_not_characters() {
    let fixture = Fixture::open();
    let claim = fixture.claim("acq-1", "worker-a", T0).unwrap();
    fixture.begin(&claim, T0);
    // 128 two-byte characters pass the schema's character bound but hold twice the bytes the charge accounts for.
    let mut wide = marker(1);
    wide.provider = "é".repeat(128);
    let error = fixture
        .store
        .dispatch_curator_attempt(
            PROJECT,
            &fixture.identity,
            1,
            &claim,
            KERNEL,
            &wide,
            "prepared",
            || T0 + 1,
            |prepared| prepared,
        )
        .unwrap_err();
    assert_eq!(refusal(error), CuratorLedgerRefusal::InvalidRequest);
    assert_eq!(fixture.attempts(), 0);
}

#[test]
fn a_receipt_binds_only_a_well_formed_kernel_incarnation_that_matches_its_staged_subject() {
    let fixture = Fixture::open();
    let claim = fixture.claim("acq-1", "worker-a", T0).unwrap();
    let begin = |kernel: &str| {
        fixture
            .store
            .begin_curator_receipt(PROJECT, &fixture.identity, kernel, &claim, T0)
    };
    assert_eq!(
        refusal(begin(&"z".repeat(32)).unwrap_err()),
        CuratorLedgerRefusal::InvalidRequest
    );
    // Well formed, but not the incarnation the staged subject was sealed under.
    assert_eq!(
        refusal(begin(&"1b".repeat(16)).unwrap_err()),
        CuratorLedgerRefusal::BindingMismatch
    );
    assert!(
        fixture
            .store
            .lookup_curator_receipt(PROJECT, &fixture.identity)
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        begin(KERNEL).unwrap(),
        CuratorBeginOutcome::Begun(_)
    ));
}

#[test]
fn an_attempt_never_outlives_the_job_queue_deadline() {
    let fixture = Fixture::open();
    let queue_deadline = T0 + CURATOR_QUEUE_LIFETIME_MS;
    let late = queue_deadline - 10_000;
    let claim = fixture.claim("acq-1", "worker-a", late).unwrap();
    fixture.begin(&claim, late);
    assert!(matches!(
        fixture.dispatch(1, &claim, late).unwrap(),
        DispatchOutcome::Handed { .. }
    ));
    let attempt = &fixture
        .store
        .list_curator_attempts(PROJECT, &fixture.identity)
        .unwrap()[0];
    assert_eq!(attempt.attempt_deadline_ms, queue_deadline);
    // A completion once the queue deadline has passed writes nothing: the job expired underneath the claim.
    assert_eq!(
        complete(
            &fixture,
            &fixture.identity,
            &claim,
            "c-1",
            "worker-a",
            1,
            CuratorReceiptTerminal::Failed,
            None,
            queue_deadline,
        )
        .unwrap(),
        LeaseCompleteOutcome::Conflict { kind: "stale" }
    );
    assert_eq!(receipt(&fixture).terminal, None);
}

#[test]
fn the_curator_lease_wrappers_bound_the_project_before_the_ledger() {
    let fixture = Fixture::open();
    let long = "p".repeat(257);
    // The generic lease path accepts any non-empty project once an authority row exists for it.
    let preparing = fixture
        .store
        .authority_begin_prepare("ctx", &long, "memories")
        .unwrap();
    fixture
        .store
        .authority_finish_prepare(
            "ctx",
            &long,
            "memories",
            preparing.generation,
            "hash",
            "hash",
            true,
        )
        .unwrap();
    assert!(
        fixture
            .store
            .acquire_curator_task(&long, "acq-1", "worker-a", 0, 1, &fixture.identity, T0)
            .is_err()
    );
    assert!(
        fixture
            .store
            .renew_curator_task(&long, "crc:x", "worker-a", 0, 1, T0)
            .is_err()
    );
}
