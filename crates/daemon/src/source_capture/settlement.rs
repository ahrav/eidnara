//! The per-session settlement record a memory-capture submit waits on before
//! its citation lookup. The supervisor writes it at each handoff exit: a
//! job taking the publication slot is `Running`, a handoff refusal is
//! `Refused`, and the job's end is `Ended`. A key with no entry is settled,
//! which is exact after a restart because no earlier job can still run.

use std::collections::VecDeque;

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
    /// The acceptance or refusal that last wrote the entry; a terminal write
    /// from an earlier one is ignored.
    acceptance: u64,
    state: watch::Sender<Settlement>,
}

#[derive(Default)]
pub(super) struct Settlements {
    entries: VecDeque<Entry>,
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
            entry.state.send_replace(Settlement::Ended { outcome });
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
            && matches!(*entry.state.borrow(), Settlement::Running { .. })
        {
            entry.state.send_replace(Settlement::Ended { outcome });
        }
    }

    /// The entry's state now and a receiver of its later writes; absent is
    /// settled.
    pub(super) fn observe(&self, key: &SessionIdentity) -> Observed {
        Observed(self.subscribe(key).map(|mut receiver| {
            let state = *receiver.borrow_and_update();
            (state, receiver)
        }))
    }

    pub(super) fn subscribe(&self, key: &SessionIdentity) -> Option<watch::Receiver<Settlement>> {
        self.entries
            .iter()
            .find(|entry| entry.key == *key)
            .map(|entry| entry.state.subscribe())
    }

    /// The outcome `key` has reached now: a running publish reads as
    /// `BudgetExpired`, an absent key as `Settled`.
    pub(super) fn now(&self, key: &SessionIdentity) -> SettlementOutcome {
        self.entries.iter().find(|entry| entry.key == *key).map_or(
            SettlementOutcome::Settled,
            |entry| {
                entry
                    .state
                    .borrow()
                    .outcome()
                    .unwrap_or(SettlementOutcome::BudgetExpired)
            },
        )
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Drops every entry and keeps the acceptance count.
    pub(super) fn clear(&mut self) {
        self.entries.clear();
    }

    fn entry(&mut self, key: &SessionIdentity) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|entry| entry.key == *key)
    }

    /// The entry of `key` while the write at `acceptance` is its latest.
    fn current(&mut self, key: &SessionIdentity, acceptance: u64) -> Option<&mut Entry> {
        self.entry(key)
            .filter(|entry| entry.acceptance == acceptance)
    }

    fn write(&mut self, key: &SessionIdentity, state: Settlement) -> u64 {
        self.acceptances += 1;
        let acceptance = self.acceptances;
        if let Some(entry) = self.entry(key) {
            entry.acceptance = acceptance;
            entry.state.send_replace(state);
            return acceptance;
        }
        if self.entries.len() >= MAX_SETTLEMENTS {
            self.evict();
        }
        self.entries.push_back(Entry {
            key: key.clone(),
            acceptance,
            state: watch::Sender::new(state),
        });
        acceptance
    }

    /// Drops the oldest entry that is not `Running`. One slot means at most
    /// one entry is `Running`, so the walk ends within two steps.
    fn evict(&mut self) {
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.state.borrow().outcome().is_some())
        {
            self.entries.remove(index);
        }
    }
}

/// One session's entry as a submit observed it, with a receiver of the
/// writes after that observation.
pub(crate) struct Observed(Option<(Settlement, watch::Receiver<Settlement>)>);

impl Observed {
    #[cfg(test)]
    pub(super) fn is_running(&self) -> bool {
        matches!(self.0, Some((Settlement::Running { .. }, _)))
    }
}

/// Settles an observation, bounded by the running job's budget deadline. Any
/// write after an observed `Running` state ends the wait: the entry's next
/// write is the job's end, a refusal, or a newer acceptance, and a newer pass
/// treats the older one as ended.
pub(crate) async fn settle(observed: Observed) -> SettlementOutcome {
    let Some((state, mut receiver)) = observed.0 else {
        return SettlementOutcome::Settled;
    };
    let Settlement::Running { accepted_at } = state else {
        return state.outcome().unwrap_or(SettlementOutcome::Settled);
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

    use super::*;
    use crate::harness_sources::Harness;

    fn key(session: &str) -> SessionIdentity {
        SessionIdentity {
            project_id: "a".repeat(64),
            harness: Harness::Pi,
            session_id: session.to_owned(),
        }
    }

    async fn outcome(settlements: &Settlements, session: &str) -> SettlementOutcome {
        settle(settlements.observe(&key(session))).await
    }

    #[tokio::test(start_paused = true)]
    async fn a_terminal_or_absent_entry_settles_without_waiting() {
        let mut settlements = Settlements::default();
        let started = Instant::now();
        assert_eq!(
            outcome(&settlements, "absent").await,
            SettlementOutcome::Settled
        );
        let completed = settlements.accept(&key("completed"), Instant::now());
        settlements.end(&key("completed"), completed, Count::Completed);
        assert_eq!(
            outcome(&settlements, "completed").await,
            SettlementOutcome::Settled
        );
        let deadline = settlements.accept(&key("deadline"), Instant::now());
        settlements.end(&key("deadline"), deadline, Count::Deadline);
        assert_eq!(
            outcome(&settlements, "deadline").await,
            SettlementOutcome::BudgetExpired
        );
        settlements.refuse(&key("refused"));
        assert_eq!(
            outcome(&settlements, "refused").await,
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
        assert_eq!(
            *settlements.subscribe(&key("ses")).unwrap().borrow(),
            Settlement::Refused
        );
    }

    #[test]
    fn a_released_slot_ends_a_running_entry_once() {
        let mut settlements = Settlements::default();
        let acceptance = settlements.accept(&key("ses"), Instant::now());
        settlements.end_if_running(&key("ses"), acceptance, Count::Discarded);
        settlements.end(&key("ses"), acceptance, Count::WorkerPanicked);
        settlements.end_if_running(&key("ses"), acceptance, Count::Discarded);
        assert_eq!(
            *settlements.subscribe(&key("ses")).unwrap().borrow(),
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
            settlements.subscribe(&key("running")).is_some(),
            "a running entry is never evicted"
        );
        assert!(settlements.subscribe(&key("settled-0")).is_none());
        assert!(settlements.subscribe(&key("settled-1")).is_some());
        settlements.refuse(&key("one-more"));
        assert_eq!(settlements.len(), MAX_SETTLEMENTS);
        assert!(settlements.subscribe(&key("settled-1")).is_none());
        assert!(settlements.subscribe(&key("running")).is_some());
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
        assert!(settlements.subscribe(&key("waited")).is_none());
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
            *settlements.subscribe(&key("ses")).unwrap().borrow(),
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
            let settlements = settlements.lock().unwrap();
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

    /// Measurement, run by hand: `cargo nextest run -p daemon --all-features
    /// --run-ignored only -E 'test(settlement_cost)' --no-capture`. Times the
    /// record's writes and subscribe at `MAX_SETTLEMENTS` entries, where each
    /// scan visits every entry.
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
            assert!(settlements.subscribe(&session).is_some());
        }
        let per_job = started.elapsed() / RUNS;
        println!(
            "settlement cost at {MAX_SETTLEMENTS} entries: {per_job:?} per job (accept, end, release, subscribe)"
        );
    }
}
