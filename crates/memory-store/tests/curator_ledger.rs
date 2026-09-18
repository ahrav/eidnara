//! Real-store proofs for the Curator receipt and attempt ledger: marker ordering around the commit-then-handoff seam, commit-failure refusal, charged-unsent expiry, reopen and takeover fencing, atomic completion, and four-attempt exhaustion across generations.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use memory_store::curator_jobs::{
    CausalInputs, CuratorJobInput, CuratorJobOutcome, CuratorJobState, EvidenceAvailability,
    ProducerBinding, ReserveOutcome, ReviewTarget,
};
use memory_store::curator_ledger::{
    AttemptMarker, CURATOR_ATTEMPT_MAX_MS, CURATOR_MAX_ATTEMPTS, CURATOR_RUN_DEADLINE_MS,
    CURATOR_SETTLEMENT_RESERVE_MS, CURATOR_TASK_LEASE_MS, CuratorAttemptTerminal,
    CuratorBeginOutcome, CuratorLedgerError, CuratorLedgerRefusal, CuratorReceiptTerminal,
    DispatchOutcome, ResultSelection,
};
use memory_store::{LeaseAcquireOutcome, LeaseCompleteOutcome, MemoryStore};
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
    storage::after_commit_for_test(move || committed.borrow_mut().push("committed".to_string()));
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
    assert_eq!(
        outcome,
        DispatchOutcome::Handed {
            attempt_index: 0,
            attempt_deadline_ms: T0 + 1 + CURATOR_ATTEMPT_MAX_MS,
            handoff: 6
        }
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
    assert_eq!(
        outcome,
        DispatchOutcome::ChargedNotDispatched {
            attempt_index: 1,
            reason: CuratorLedgerRefusal::Cutoff,
            finished: true,
        },
        "the claim outlives the attempt bound, so only the attempt deadline lapsed"
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
    // A commit that fails after the insert statement hands nothing off and charges nothing: the model exceeds the column bound the Rust pre-check does not know.
    let mut long_model = marker(1);
    long_model.model = "m".repeat(257);
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
