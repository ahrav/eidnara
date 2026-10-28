//! The per-session settlement record a memory-capture submit waits on before
//! its citation lookup. The supervisor writes it at each handoff exit: a
//! job taking the publication slot is `Running`, a handoff refusal is
//! `Refused`, and the job's end is `Ended`. A key with no entry is settled,
//! which is exact after a restart because no earlier job can still run.
//!
//! Each entry holds its state inline. A submit that finds a `Running` entry
//! subscribes to the entry's `tokio::sync::watch` channel, which the first
//! such submit creates; the entry's next write sends on it and drops it, so
//! the channel exists only while a submit waits.

use std::collections::VecDeque;
use std::hash::BuildHasher;

use hashbrown::{DefaultHashBuilder, HashTable};
use tokio::sync::watch;
use tokio::time::Instant;

use super::{CAPTURE_BUDGET, Count};
use crate::harness_sources::SessionIdentity;

/// Entries the record keeps. Eviction drops the oldest entry that is not
/// `Running`; a `Running` entry outlives its budget before it can go.
pub(crate) const MAX_SETTLEMENTS: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Settlement {
    /// A job of the session holds the publication slot since `accepted_at`,
    /// the instant its 5 s budget started.
    Running {
        accepted_at: Instant,
    },
    Ended {
        outcome: Count,
    },
    Refused,
}

impl Settlement {
    /// The outcome a terminal state settles to; `None` while `Running`.
    pub(crate) fn outcome(self) -> Option<SettlementOutcome> {
        match self {
            Self::Running { .. } => None,
            Self::Ended {
                outcome: Count::Deadline,
            }
            | Self::Refused => Some(SettlementOutcome::BudgetExpired),
            Self::Ended { .. } => Some(SettlementOutcome::Settled),
        }
    }
}

/// What a submit's citation lookup may conclude from an empty read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettlementOutcome {
    /// Source or memory consent is off: no lookup runs.
    Off,
    /// The session's publish ended or none ran: an empty read means the
    /// descriptor does not exist.
    Settled,
    /// The publish was refused, ran out its budget, or was still running at
    /// the deadline: an empty read means the descriptor may not have landed.
    BudgetExpired,
}

struct Entry {
    key: SessionIdentity,
    /// The write that inserted the entry; its place in the eviction order.
    inserted: u64,
    /// The acceptance or refusal that last wrote the entry; a terminal write
    /// from an earlier one is ignored.
    acceptance: u64,
    state: Settlement,
    /// The channel of the submits waiting on this `Running` state, present
    /// from the first such submit until the entry's next write.
    waiters: Option<watch::Sender<Settlement>>,
}

impl Entry {
    /// Writes `state` and wakes every waiting submit with it; a waiter reads
    /// the new state before it sees the channel close.
    fn set(&mut self, state: Settlement) {
        self.state = state;
        if let Some(waiters) = self.waiters.take() {
            waiters.send_replace(state);
        }
    }
}

#[derive(Default)]
pub(super) struct Settlements {
    /// Entries hashed by key with foldhash; the bound caps a full collision
    /// chain at `MAX_SETTLEMENTS` key comparisons, the cost of one scan of
    /// the record.
    entries: HashTable<Entry>,
    hasher: DefaultHashBuilder,
    /// Each entry's key hash and `inserted`, oldest insertion first.
    order: VecDeque<(u64, u64)>,
    acceptances: u64,
}

impl Settlements {
    /// Records that a job of `key` took the slot; returns the acceptance its
    /// end must name.
    pub(super) fn accept(&mut self, key: &SessionIdentity, accepted_at: Instant) -> u64 {
        self.write(key, Settlement::Running { accepted_at })
    }

    pub(super) fn refuse(&mut self, key: &SessionIdentity) {
        self.write(key, Settlement::Refused);
    }

    /// Records the end of the job that took the slot at `acceptance`.
    pub(super) fn end(&mut self, key: &SessionIdentity, acceptance: u64, outcome: Count) {
        if let Some(entry) = self.current(key, acceptance) {
            entry.set(Settlement::Ended { outcome });
        }
    }

    /// Ends the job at `acceptance` as `outcome` when nothing has ended it,
    /// so a released slot leaves no `Running` entry behind.
    pub(super) fn end_if_running(
        &mut self,
        key: &SessionIdentity,
        acceptance: u64,
        outcome: Count,
    ) {
        if let Some(entry) = self.current(key, acceptance)
            && matches!(entry.state, Settlement::Running { .. })
        {
            entry.set(Settlement::Ended { outcome });
        }
    }

    /// The entry's state now and, for a `Running` state, a receiver of the
    /// entry's next write; absent is settled.
    pub(super) fn observe(&mut self, key: &SessionIdentity) -> Observed {
        let Some(entry) = self.entry_mut(key) else {
            return Observed(Observation::Done(SettlementOutcome::Settled));
        };
        let Settlement::Running { accepted_at } = entry.state else {
            return Observed(Observation::Done(
                entry.state.outcome().unwrap_or(SettlementOutcome::Settled),
            ));
        };
        let state = entry.state;
        let receiver = entry
            .waiters
            .get_or_insert_with(|| watch::Sender::new(state))
            .subscribe();
        Observed(Observation::Running {
            accepted_at,
            receiver,
        })
    }

    /// The outcome `key` has reached now: a running publish reads as
    /// `BudgetExpired`, an absent key as `Settled`.
    pub(super) fn now(&self, key: &SessionIdentity) -> SettlementOutcome {
        self.entries
            .find(self.hasher.hash_one(key), |entry| entry.key == *key)
            .map_or(SettlementOutcome::Settled, |entry| {
                entry
                    .state
                    .outcome()
                    .unwrap_or(SettlementOutcome::BudgetExpired)
            })
    }

    #[cfg(test)]
    pub(super) fn state(&self, key: &SessionIdentity) -> Option<Settlement> {
        self.entries
            .find(self.hasher.hash_one(key), |entry| entry.key == *key)
            .map(|entry| entry.state)
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Drops every entry and keeps the acceptance count.
    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }

    fn entry_mut(&mut self, key: &SessionIdentity) -> Option<&mut Entry> {
        self.entries
            .find_mut(self.hasher.hash_one(key), |entry| entry.key == *key)
    }

    /// The entry of `key` while the write at `acceptance` is its latest.
    fn current(&mut self, key: &SessionIdentity, acceptance: u64) -> Option<&mut Entry> {
        self.entry_mut(key)
            .filter(|entry| entry.acceptance == acceptance)
    }

    fn write(&mut self, key: &SessionIdentity, state: Settlement) -> u64 {
        self.acceptances += 1;
        let acceptance = self.acceptances;
        let hash = self.hasher.hash_one(key);
        if let Some(entry) = self.entries.find_mut(hash, |entry| entry.key == *key) {
            entry.acceptance = acceptance;
            entry.set(state);
            return acceptance;
        }
        if self.entries.len() >= MAX_SETTLEMENTS {
            self.evict();
        }
        let hasher = &self.hasher;
        self.entries.insert_unique(
            hash,
            Entry {
                key: key.clone(),
                inserted: acceptance,
                acceptance,
                state,
                waiters: None,
            },
            |entry| hasher.hash_one(&entry.key),
        );
        self.order.push_back((hash, acceptance));
        acceptance
    }

    /// Drops the oldest entry that is not `Running`. One slot means at most
    /// one entry is `Running`, so the walk ends within two steps.
    fn evict(&mut self) {
        let entries = &self.entries;
        let Some(position) = self.order.iter().position(|&(hash, inserted)| {
            entries
                .find(hash, |entry| entry.inserted == inserted)
                .is_some_and(|entry| entry.state.outcome().is_some())
        }) else {
            return;
        };
        let (hash, inserted) = self.order.remove(position).expect("a found position");
        if let Ok(entry) = self
            .entries
            .find_entry(hash, |entry| entry.inserted == inserted)
        {
            entry.remove();
        }
    }
}

enum Observation {
    Done(SettlementOutcome),
    Running {
        accepted_at: Instant,
        receiver: watch::Receiver<Settlement>,
    },
}

/// One session's entry as a submit observed it: its outcome, or for a
/// `Running` entry the deadline and a receiver of the entry's next write.
pub(crate) struct Observed(Observation);

impl Observed {
    #[cfg(test)]
    pub(super) fn is_running(&self) -> bool {
        matches!(self.0, Observation::Running { .. })
    }
}

/// Settles an observation, bounded by the running job's budget deadline. Any
/// write after an observed `Running` state ends the wait: the entry's next
/// write is the job's end, a refusal, or a newer acceptance, and a newer pass
/// treats the older one as ended. A record cleared under a waiter closes its
/// channel, which reads as settled.
pub(crate) async fn settle(observed: Observed) -> SettlementOutcome {
    let (accepted_at, mut receiver) = match observed.0 {
        Observation::Done(outcome) => return outcome,
        Observation::Running {
            accepted_at,
            receiver,
        } => (accepted_at, receiver),
    };
    match tokio::time::timeout_at(accepted_at + CAPTURE_BUDGET, receiver.changed()).await {
        Err(_elapsed) => SettlementOutcome::BudgetExpired,
        Ok(Err(_closed)) => SettlementOutcome::Settled,
        Ok(Ok(())) => receiver
            .borrow()
            .outcome()
            .unwrap_or(SettlementOutcome::Settled),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use proptest::strategy::Strategy as _;

    use super::*;
    use crate::harness_sources::Harness;

    fn key(session: &str) -> SessionIdentity {
        SessionIdentity {
            project_id: "a".repeat(64),
            harness: Harness::Pi,
            session_id: session.to_owned(),
        }
    }

    async fn outcome(settlements: &mut Settlements, session: &str) -> SettlementOutcome {
        settle(settlements.observe(&key(session))).await
    }

    #[tokio::test(start_paused = true)]
    async fn a_terminal_or_absent_entry_settles_without_waiting() {
        let mut settlements = Settlements::default();
        let started = Instant::now();
        assert_eq!(
            outcome(&mut settlements, "absent").await,
            SettlementOutcome::Settled
        );
        let completed = settlements.accept(&key("completed"), Instant::now());
        settlements.end(&key("completed"), completed, Count::Completed);
        assert_eq!(
            outcome(&mut settlements, "completed").await,
            SettlementOutcome::Settled
        );
        let deadline = settlements.accept(&key("deadline"), Instant::now());
        settlements.end(&key("deadline"), deadline, Count::Deadline);
        assert_eq!(
            outcome(&mut settlements, "deadline").await,
            SettlementOutcome::BudgetExpired
        );
        settlements.refuse(&key("refused"));
        assert_eq!(
            outcome(&mut settlements, "refused").await,
            SettlementOutcome::BudgetExpired
        );
        assert_eq!(Instant::now(), started, "no wait was armed");
    }

    #[tokio::test(start_paused = true)]
    async fn a_running_entry_waits_for_its_end_or_its_budget_deadline() {
        let settlements = std::sync::Arc::new(std::sync::Mutex::new(Settlements::default()));
        let accepted_at = Instant::now();
        let acceptance = settlements
            .lock()
            .unwrap()
            .accept(&key("ends"), accepted_at);
        let receiver = settlements.lock().unwrap().observe(&key("ends"));
        let waiter = tokio::spawn(settle(receiver));
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(!waiter.is_finished());
        settlements
            .lock()
            .unwrap()
            .end(&key("ends"), acceptance, Count::Completed);
        assert_eq!(waiter.await.unwrap(), SettlementOutcome::Settled);
        assert_eq!(Instant::now() - accepted_at, Duration::from_secs(1));

        settlements
            .lock()
            .unwrap()
            .accept(&key("stalls"), Instant::now());
        let started = Instant::now();
        let receiver = settlements.lock().unwrap().observe(&key("stalls"));
        let waiter = tokio::spawn(settle(receiver));
        tokio::time::advance(CAPTURE_BUDGET - Duration::from_millis(1)).await;
        assert!(!waiter.is_finished());
        tokio::time::advance(Duration::from_millis(1)).await;
        assert_eq!(waiter.await.unwrap(), SettlementOutcome::BudgetExpired);
        assert_eq!(Instant::now() - started, CAPTURE_BUDGET);
    }

    #[tokio::test(start_paused = true)]
    async fn a_refusal_overwrites_a_running_entry_and_its_late_end_is_ignored() {
        let mut settlements = Settlements::default();
        let first = settlements.accept(&key("ses"), Instant::now());
        let receiver = settlements.observe(&key("ses"));
        settlements.refuse(&key("ses"));
        settlements.end(&key("ses"), first, Count::Completed);
        settlements.end_if_running(&key("ses"), first, Count::Discarded);
        assert_eq!(settle(receiver).await, SettlementOutcome::BudgetExpired);
        assert_eq!(settlements.state(&key("ses")).unwrap(), Settlement::Refused);
    }

    #[test]
    fn a_released_slot_ends_a_running_entry_once() {
        let mut settlements = Settlements::default();
        let acceptance = settlements.accept(&key("ses"), Instant::now());
        settlements.end_if_running(&key("ses"), acceptance, Count::Discarded);
        settlements.end(&key("ses"), acceptance, Count::WorkerPanicked);
        settlements.end_if_running(&key("ses"), acceptance, Count::Discarded);
        assert_eq!(
            settlements.state(&key("ses")).unwrap(),
            Settlement::Ended {
                outcome: Count::WorkerPanicked
            }
        );
    }

    #[test]
    fn the_record_keeps_its_bound_by_dropping_the_oldest_settled_entry() {
        let mut settlements = Settlements::default();
        settlements.accept(&key("running"), Instant::now());
        for index in 0..MAX_SETTLEMENTS {
            settlements.refuse(&key(&format!("settled-{index}")));
        }
        assert_eq!(settlements.len(), MAX_SETTLEMENTS);
        assert!(
            settlements.state(&key("running")).is_some(),
            "a running entry is never evicted"
        );
        assert!(settlements.state(&key("settled-0")).is_none());
        assert!(settlements.state(&key("settled-1")).is_some());
        settlements.refuse(&key("one-more"));
        assert_eq!(settlements.len(), MAX_SETTLEMENTS);
        assert!(settlements.state(&key("settled-1")).is_none());
        assert!(settlements.state(&key("running")).is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn an_evicted_entry_reads_as_settled_to_its_waiter() {
        let mut settlements = Settlements::default();
        settlements.refuse(&key("oldest"));
        let acceptance = settlements.accept(&key("waited"), Instant::now());
        let receiver = settlements.observe(&key("waited"));
        settlements.end(&key("waited"), acceptance, Count::Completed);
        for index in 0..MAX_SETTLEMENTS {
            settlements.refuse(&key(&format!("filler-{index}")));
        }
        assert!(settlements.state(&key("waited")).is_none());
        assert_eq!(settle(receiver).await, SettlementOutcome::Settled);
    }

    #[test]
    fn only_a_deadline_end_or_a_refusal_reads_as_budget_expired() {
        for outcome in Count::ALL {
            let expected = if outcome == Count::Deadline {
                SettlementOutcome::BudgetExpired
            } else {
                SettlementOutcome::Settled
            };
            assert_eq!(
                Settlement::Ended { outcome }.outcome(),
                Some(expected),
                "{outcome:?}"
            );
        }
        assert_eq!(
            Settlement::Refused.outcome(),
            Some(SettlementOutcome::BudgetExpired)
        );
        assert_eq!(
            Settlement::Running {
                accepted_at: Instant::now()
            }
            .outcome(),
            None
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_later_acceptance_after_the_awaited_end_reads_as_settled() {
        let settlements = std::sync::Arc::new(std::sync::Mutex::new(Settlements::default()));
        let first = settlements
            .lock()
            .unwrap()
            .accept(&key("ses"), Instant::now());
        let receiver = settlements.lock().unwrap().observe(&key("ses"));
        let waiter = tokio::spawn(settle(receiver));
        tokio::task::yield_now().await;
        {
            // The newer acceptance reads the same instant as the first.
            let mut settlements = settlements.lock().unwrap();
            settlements.end(&key("ses"), first, Count::Completed);
            settlements.accept(&key("ses"), Instant::now());
        }
        let started = Instant::now();
        assert_eq!(waiter.await.unwrap(), SettlementOutcome::Settled);
        assert_eq!(Instant::now(), started, "the newer pass ends the wait");
    }

    #[tokio::test(start_paused = true)]
    async fn a_waiter_whose_record_is_cleared_reads_as_settled() {
        let mut settlements = Settlements::default();
        let first = settlements.accept(&key("ses"), Instant::now());
        let receiver = settlements.observe(&key("ses"));
        let waiter = tokio::spawn(settle(receiver));
        tokio::task::yield_now().await;
        settlements.clear();
        assert_eq!(waiter.await.unwrap(), SettlementOutcome::Settled);
        let next = settlements.accept(&key("ses"), Instant::now());
        assert!(next > first, "a cleared record keeps its acceptance count");
        settlements.end(&key("ses"), first, Count::Completed);
        assert!(matches!(
            settlements.state(&key("ses")).unwrap(),
            Settlement::Running { .. }
        ));
    }

    #[test]
    fn the_current_outcome_reads_a_running_publish_as_budget_expired() {
        let mut settlements = Settlements::default();
        assert_eq!(settlements.now(&key("ses")), SettlementOutcome::Settled);
        let acceptance = settlements.accept(&key("ses"), Instant::now());
        assert_eq!(
            settlements.now(&key("ses")),
            SettlementOutcome::BudgetExpired
        );
        settlements.end(&key("ses"), acceptance, Count::Completed);
        assert_eq!(settlements.now(&key("ses")), SettlementOutcome::Settled);
        settlements.refuse(&key("ses"));
        assert_eq!(
            settlements.now(&key("ses")),
            SettlementOutcome::BudgetExpired
        );
    }

    #[tokio::test(start_paused = true)]
    async fn sessions_observed_together_settle_by_the_one_running_deadline() {
        let settlements = std::sync::Arc::new(std::sync::Mutex::new(Settlements::default()));
        let running = settlements
            .lock()
            .unwrap()
            .accept(&key("a"), Instant::now());
        let observed = {
            let mut settlements = settlements.lock().unwrap();
            [key("a"), key("b")].map(|session| settlements.observe(&session))
        };
        let started = Instant::now();
        let waiter = tokio::spawn(async move {
            let mut outcomes = Vec::new();
            for observed in observed {
                outcomes.push(settle(observed).await);
            }
            outcomes
        });
        tokio::time::advance(Duration::from_secs(1)).await;
        {
            // The running job ends and a later pass of the second session
            // takes the slot, both after the observation.
            let mut settlements = settlements.lock().unwrap();
            settlements.end(&key("a"), running, Count::Completed);
            settlements.accept(&key("b"), Instant::now());
        }
        assert_eq!(
            waiter.await.unwrap(),
            [SettlementOutcome::Settled, SettlementOutcome::Settled]
        );
        assert_eq!(
            Instant::now() - started,
            Duration::from_secs(1),
            "the later pass arms no wait"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn every_submit_waiting_on_one_running_entry_reads_its_next_write() {
        let settlements = std::sync::Arc::new(std::sync::Mutex::new(Settlements::default()));
        let acceptance = settlements
            .lock()
            .unwrap()
            .accept(&key("ses"), Instant::now());
        let waiters: Vec<_> = (0..2)
            .map(|_| {
                let observed = settlements.lock().unwrap().observe(&key("ses"));
                assert!(observed.is_running());
                tokio::spawn(settle(observed))
            })
            .collect();
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(waiters.iter().all(|waiter| !waiter.is_finished()));
        settlements
            .lock()
            .unwrap()
            .end(&key("ses"), acceptance, Count::Deadline);
        for waiter in waiters {
            assert_eq!(waiter.await.unwrap(), SettlementOutcome::BudgetExpired);
        }
        let later = settlements.lock().unwrap().observe(&key("ses"));
        assert!(!later.is_running());
        assert_eq!(settle(later).await, SettlementOutcome::BudgetExpired);
    }

    /// A newer pass that takes the slot before the waiter runs leaves the
    /// waiter with the end it was woken by.
    #[tokio::test(start_paused = true)]
    async fn a_waiter_reads_the_first_write_after_its_observation() {
        let mut settlements = Settlements::default();
        let first = settlements.accept(&key("ses"), Instant::now());
        let waiter = tokio::spawn(settle(settlements.observe(&key("ses"))));
        settlements.end(&key("ses"), first, Count::Deadline);
        settlements.accept(&key("ses"), Instant::now());
        let started = Instant::now();
        assert_eq!(waiter.await.unwrap(), SettlementOutcome::BudgetExpired);
        assert_eq!(Instant::now(), started);
    }

    #[test]
    fn an_end_naming_another_session_or_an_evicted_acceptance_is_ignored() {
        let mut settlements = Settlements::default();
        let acceptance = settlements.accept(&key("ses"), Instant::now());
        settlements.end(&key("other"), acceptance, Count::Completed);
        settlements.end_if_running(&key("other"), acceptance, Count::Discarded);
        assert!(matches!(
            settlements.state(&key("ses")).unwrap(),
            Settlement::Running { .. }
        ));
        assert!(settlements.state(&key("other")).is_none());
        settlements.end(&key("ses"), acceptance, Count::Completed);
        for index in 0..MAX_SETTLEMENTS {
            settlements.refuse(&key(&format!("filler-{index}")));
        }
        assert!(settlements.state(&key("ses")).is_none());
        settlements.refuse(&key("ses"));
        settlements.end(&key("ses"), acceptance, Count::Completed);
        settlements.end_if_running(&key("ses"), acceptance, Count::Discarded);
        assert_eq!(settlements.state(&key("ses")), Some(Settlement::Refused));
        assert_eq!(settlements.len(), MAX_SETTLEMENTS);
    }

    /// The record as a linear list in insertion order, the form #1136
    /// specifies, against which the indexed record is checked.
    #[derive(Default)]
    struct Model {
        entries: VecDeque<(SessionIdentity, u64, Settlement)>,
        acceptances: u64,
    }

    impl Model {
        fn get(
            &mut self,
            key: &SessionIdentity,
        ) -> Option<&mut (SessionIdentity, u64, Settlement)> {
            self.entries.iter_mut().find(|entry| entry.0 == *key)
        }

        fn state(&self, key: &SessionIdentity) -> Option<Settlement> {
            self.entries
                .iter()
                .find(|entry| entry.0 == *key)
                .map(|entry| entry.2)
        }

        fn write(&mut self, key: &SessionIdentity, state: Settlement) -> u64 {
            self.acceptances += 1;
            let acceptance = self.acceptances;
            if let Some(entry) = self.get(key) {
                (entry.1, entry.2) = (acceptance, state);
            } else {
                if self.entries.len() >= MAX_SETTLEMENTS
                    && let Some(oldest) = self
                        .entries
                        .iter()
                        .position(|entry| entry.2.outcome().is_some())
                {
                    self.entries.remove(oldest);
                }
                self.entries.push_back((key.clone(), acceptance, state));
            }
            acceptance
        }

        /// Applies a terminal write; returns whether it wrote.
        fn end(
            &mut self,
            key: &SessionIdentity,
            acceptance: u64,
            outcome: Count,
            running_only: bool,
        ) -> bool {
            match self.get(key) {
                Some(entry)
                    if entry.1 == acceptance
                        && (!running_only || matches!(entry.2, Settlement::Running { .. })) =>
                {
                    entry.2 = Settlement::Ended { outcome };
                    true
                }
                _ => false,
            }
        }
    }

    #[derive(Debug, Clone)]
    enum Op {
        Accept(usize),
        Refuse(usize),
        /// The running job ends with `Count::ALL[_]` and releases its slot.
        End(usize),
        /// The running job's slot is released with no end recorded.
        Release,
        /// A late terminal write naming an earlier acceptance, optionally
        /// with another session's key.
        Stale {
            pick: usize,
            key: Option<usize>,
            outcome: usize,
            running_only: bool,
        },
        Observe(usize),
        Clear,
    }

    fn ops(
        universe: usize,
        refusals: u32,
        len: std::ops::Range<usize>,
    ) -> impl proptest::strategy::Strategy<Value = Vec<Op>> {
        use proptest::prelude::*;
        let outcome = 0..Count::ALL.len();
        let op = prop_oneof![
            3 => (0..universe).prop_map(Op::Accept),
            refusals => (0..universe).prop_map(Op::Refuse),
            2 => outcome.clone().prop_map(Op::End),
            1 => Just(Op::Release),
            2 => (any::<usize>(), proptest::option::of(0..universe), outcome, any::<bool>())
                .prop_map(|(pick, key, outcome, running_only)| Op::Stale { pick, key, outcome, running_only }),
            2 => (0..universe).prop_map(Op::Observe),
            1 => Just(Op::Clear),
        ];
        proptest::collection::vec(op, len)
    }

    /// Runs `ops` against the record and the model under the single
    /// publication slot, checking every observable after each step and
    /// each waiter's outcome at the end; returns the record's largest size.
    fn check_against_model(universe: usize, ops: Vec<Op>, every_key: bool) -> usize {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .start_paused(true)
            .build()
            .unwrap();
        runtime.block_on(async {
            let mut largest = 0;
            let keys: Vec<SessionIdentity> = (0..universe)
                .map(|index| SessionIdentity {
                    project_id: if index % 5 == 0 { "b" } else { "a" }.repeat(64),
                    harness: if index % 3 == 0 {
                        Harness::OpenCode
                    } else {
                        Harness::Pi
                    },
                    session_id: format!("ses-{index}"),
                })
                .collect();
            let (mut record, mut model) = (Settlements::default(), Model::default());
            let mut slot: Option<(usize, u64, u64)> = None;
            let mut history: Vec<(usize, u64, u64)> = Vec::new();
            type Waiter = (
                usize,
                Option<SettlementOutcome>,
                std::pin::Pin<Box<dyn Future<Output = SettlementOutcome>>>,
            );
            let mut waiters: Vec<Waiter> = Vec::new();
            let resolve =
                |waiters: &mut Vec<Waiter>, key: Option<usize>, state: Option<Settlement>| {
                    for waiter in waiters.iter_mut().filter(|waiter| {
                        waiter.1.is_none() && key.is_none_or(|key| key == waiter.0)
                    }) {
                        waiter.1 = Some(
                            state
                                .and_then(Settlement::outcome)
                                .unwrap_or(SettlementOutcome::Settled),
                        );
                    }
                };
            for op in ops {
                let touched = match op {
                    Op::Accept(k) if slot.is_none() => {
                        let at = Instant::now();
                        let ours = record.accept(&keys[k], at);
                        let theirs = model.write(&keys[k], Settlement::Running { accepted_at: at });
                        resolve(&mut waiters, Some(k), model.state(&keys[k]));
                        slot = Some((k, ours, theirs));
                        history.push((k, ours, theirs));
                        Some(k)
                    }
                    Op::Accept(k) | Op::Refuse(k) => {
                        record.refuse(&keys[k]);
                        model.write(&keys[k], Settlement::Refused);
                        resolve(&mut waiters, Some(k), model.state(&keys[k]));
                        Some(k)
                    }
                    Op::End(outcome) => slot.take().map(|(k, ours, theirs)| {
                        let outcome = Count::ALL[outcome];
                        record.end(&keys[k], ours, outcome);
                        record.end_if_running(&keys[k], ours, Count::Discarded);
                        if model.end(&keys[k], theirs, outcome, false) {
                            resolve(&mut waiters, Some(k), model.state(&keys[k]));
                        }
                        k
                    }),
                    Op::Release => slot.take().map(|(k, ours, theirs)| {
                        record.end_if_running(&keys[k], ours, Count::Discarded);
                        if model.end(&keys[k], theirs, Count::Discarded, true) {
                            resolve(&mut waiters, Some(k), model.state(&keys[k]));
                        }
                        k
                    }),
                    Op::Stale {
                        pick,
                        key,
                        outcome,
                        running_only,
                    } if !history.is_empty() => {
                        let (own, ours, theirs) = history[pick % history.len()];
                        let k = key.unwrap_or(own);
                        let outcome = Count::ALL[outcome];
                        if running_only {
                            record.end_if_running(&keys[k], ours, outcome);
                        } else {
                            record.end(&keys[k], ours, outcome);
                        }
                        if model.end(&keys[k], theirs, outcome, running_only) {
                            resolve(&mut waiters, Some(k), model.state(&keys[k]));
                        }
                        Some(k)
                    }
                    Op::Stale { .. } => None,
                    Op::Observe(k) => {
                        let state = model.state(&keys[k]);
                        let resolution = match state {
                            Some(Settlement::Running { .. }) => None,
                            state => Some(
                                state
                                    .and_then(Settlement::outcome)
                                    .unwrap_or(SettlementOutcome::Settled),
                            ),
                        };
                        waiters.push((k, resolution, Box::pin(settle(record.observe(&keys[k])))));
                        Some(k)
                    }
                    Op::Clear => {
                        record.clear();
                        model.entries.clear();
                        resolve(&mut waiters, None, None);
                        None
                    }
                };
                assert_eq!(record.len(), model.entries.len());
                assert!(record.len() <= MAX_SETTLEMENTS);
                largest = largest.max(record.len());
                let checked: Vec<usize> = if every_key {
                    (0..universe).collect()
                } else {
                    touched.into_iter().collect()
                };
                for k in checked {
                    assert_eq!(
                        record.state(&keys[k]),
                        model.state(&keys[k]),
                        "{:?}",
                        keys[k]
                    );
                    assert_eq!(
                        record.now(&keys[k]),
                        model
                            .state(&keys[k])
                            .map_or(SettlementOutcome::Settled, |state| state
                                .outcome()
                                .unwrap_or(SettlementOutcome::BudgetExpired))
                    );
                }
            }
            for key in &keys {
                assert_eq!(record.state(key), model.state(key));
            }
            let mut context = std::task::Context::from_waker(std::task::Waker::noop());
            for (_, resolution, waiter) in &mut waiters {
                assert_eq!(
                    waiter.as_mut().poll(&mut context),
                    resolution.map_or(std::task::Poll::Pending, std::task::Poll::Ready)
                );
            }
            tokio::time::advance(CAPTURE_BUDGET).await;
            for (_, _, waiter) in waiters.iter_mut().filter(|waiter| waiter.1.is_none()) {
                assert_eq!(waiter.await, SettlementOutcome::BudgetExpired);
            }
            largest
        })
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(256))]

        #[test]
        fn a_few_sessions_match_the_linear_model(ops in ops(3, 3, 0..64)) {
            check_against_model(3, ops, true);
        }
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(16))]

        #[test]
        fn sessions_past_the_bound_match_the_linear_model(
            ops in ops(2000, 60, 1900..2100)
                .prop_map(|ops| ops.into_iter().filter(|op| !matches!(op, Op::Clear)).collect())
        ) {
            proptest::prop_assert_eq!(check_against_model(2000, ops, false), MAX_SETTLEMENTS);
        }
    }

    /// Measurement, run by hand: `cargo nextest run -p daemon --all-features
    /// --run-ignored only -E 'test(settlement_cost)' --no-capture`.
    #[test]
    #[ignore = "measurement"]
    fn settlement_cost_measurement() {
        let mut settlements = Settlements::default();
        for index in 0..MAX_SETTLEMENTS {
            settlements.refuse(&key(&format!("filler-{index}")));
        }
        const RUNS: u32 = 1_000;
        let started = std::time::Instant::now();
        for run in 0..RUNS {
            let session = key(&format!("probe-{run}"));
            let acceptance = settlements.accept(&session, Instant::now());
            settlements.end(&session, acceptance, Count::Completed);
            settlements.end_if_running(&session, acceptance, Count::Discarded);
            assert!(!settlements.observe(&session).is_running());
        }
        let per_job = started.elapsed() / RUNS;
        println!(
            "settlement cost at {MAX_SETTLEMENTS} entries: {per_job:?} per job (accept, end, release, observe)"
        );
    }
}
