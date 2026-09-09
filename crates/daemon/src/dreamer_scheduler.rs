//! Rust-owned scheduling of Dreamer tasks.
//!
//! Each bound project's cron schedule is read from its effective
//! configuration; a due task is leased through the shared task-lease ledger
//! and run through the durable `dreamer.run_task` protocol under a command id
//! derived from the due instant, so an interrupted run that is retried replays
//! or resumes its receipt instead of dispatching a second model call.
//!
//! Time enters through one [`SchedulerClock`] at this boundary; every decision
//! [`DreamerScheduler::tick`] makes reads that clock, so tests advance a manual
//! clock and never sleep.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use memory_store::{LeaseAcquireOutcome, LeaseClaim, LeaseCompleteOutcome, MemoryStore};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::smart_note_evaluation::next_cron_occurrence;

/// The one task the schedule key names.
pub(crate) const REVIEW_USER_MEMORIES_TASK: &str = "review-user-memories";
/// Its identity on the lease ledger.
pub(crate) const REVIEW_USER_MEMORIES_TASK_ID: i64 = 1;
/// The ledger session every scheduled run is recorded under.
pub(crate) const SCHEDULER_LEDGER_SESSION: &str = "eidnara-dreamer-scheduler";
/// The scheduler's worker identity on the lease ledger; one slot per daemon.
pub(crate) const SCHEDULER_INSTANCE: &str = "eidnara-dreamer-scheduler";
pub(crate) const SCHEDULER_SLOT: i64 = 0;
/// How long the loop waits when no project has a schedule, so a schedule set
/// later or a route bound later is noticed without a restart.
pub(crate) const IDLE_POLL: Duration = Duration::from_secs(60);

/// Time as the scheduler sees it. Production wraps the wall clock and Tokio's
/// sleep; tests advance a manual counter.
#[async_trait]
pub(crate) trait SchedulerClock: Send + Sync {
    fn now_ms(&self) -> i64;
    async fn sleep(&self, duration: Duration);
}

pub(crate) struct WallClock;

#[async_trait]
impl SchedulerClock for WallClock {
    fn now_ms(&self) -> i64 {
        crate::now_ms()
    }

    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}

/// One project the scheduler may run a task for: its memories authority is
/// `MODULE`, a live route is bound to it, and the user tier set a schedule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScheduledProject {
    /// The authority project the receipt and lease are keyed by.
    pub(crate) project: String,
    /// The bound route root the run's configuration and producer come from.
    pub(crate) route_root: PathBuf,
    pub(crate) authority_generation: u64,
    pub(crate) schedule: String,
}

/// What running a task produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TaskRunOutcome {
    /// The durable protocol answered; `response` is its wire reply.
    Ran { response: Value },
    /// The task has no Rust-owned inputs on this daemon, so nothing was
    /// dispatched; the slot is still recorded so the scheduler does not
    /// retry it every tick.
    NotRunnable { reason: String },
}

/// The daemon as the scheduler drives it.
#[async_trait]
pub(crate) trait SchedulerHost: Send + Sync {
    fn store(&self) -> &MemoryStore;
    /// Returns at most one scheduled project per authority project; the host
    /// chooses which bound route root represents a project reachable through
    /// several. `Err` is a store failure, not an empty list: the scheduler
    /// leaves the due table unchanged so a later tick retries the same slots.
    fn scheduled_projects(&self) -> Result<Vec<ScheduledProject>, String>;
    /// Runs `task` for `project` under `command_id` through the durable
    /// receipt protocol.
    async fn run_task(
        &self,
        project: &ScheduledProject,
        task: &str,
        command_id: &str,
    ) -> TaskRunOutcome;
}

/// Events emitted by a tick, ordered by execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TickEvent {
    /// The task was leased and run for the slot due at `due_at_ms`. When a
    /// predecessor's live claim was rebound, `due_at_ms` is that claim's slot,
    /// not the one that came due, so the interrupted run is what resumes.
    Ran {
        project: String,
        due_at_ms: i64,
        outcome: TaskRunOutcome,
    },
    /// Another live claim holds the task, or the lease ledger refused; the
    /// slot is skipped.
    Skipped {
        project: String,
        due_at_ms: i64,
        reason: String,
    },
    /// The lease ledger could not be read or written, so the slot has no
    /// claim and is not consumed; it stays due and a later tick retries it.
    Retained {
        project: String,
        due_at_ms: i64,
        reason: String,
    },
    /// The host could not report its projects, so nothing ran and no due
    /// instant moved; the next tick sees the same slots.
    Deferred { reason: String },
}

/// The command id of one scheduled slot: the task and the instant it was due,
/// so a retry of the same slot replays its receipt.
pub(crate) fn slot_command_id(task: &str, due_at_ms: i64) -> String {
    format!("{task}@{due_at_ms}")
}

pub(crate) struct DreamerScheduler {
    clock: Arc<dyn SchedulerClock>,
    /// Allocated from the ledger on first use, above every generation this
    /// instance recorded before, so a claim left by a crashed scheduler is
    /// recovered by its successor under the shared protocol.
    registration_generation: Option<i64>,
    /// The next cron instant per project, computed from the schedule the
    /// project had when it was last seen.
    next_due: HashMap<String, (String, i64)>,
}

impl DreamerScheduler {
    pub(crate) fn new(clock: Arc<dyn SchedulerClock>) -> Self {
        Self {
            clock,
            registration_generation: None,
            next_due: HashMap::new(),
        }
    }

    /// Runs ticks until `cancel`, sleeping until the earliest due instant or
    /// the idle poll, whichever comes first. After a deferred tick or a
    /// retained slot the loop waits the idle poll, not the zero distance to
    /// the slot that is still due, so a failing store is not re-ticked at once.
    pub(crate) async fn run(mut self, host: Arc<dyn SchedulerHost>, cancel: CancellationToken) {
        loop {
            if cancel.is_cancelled() {
                return;
            }
            // Cancellation drops the in-flight tick. The receipt protocol
            // recovers an abandoned run on restart; projects still due are not leased.
            let events = tokio::select! {
                biased;
                () = cancel.cancelled() => return,
                events = self.tick(host.as_ref()) => events,
            };
            let mut store_failed = false;
            for event in events {
                match event {
                    TickEvent::Ran { .. } => {}
                    TickEvent::Skipped {
                        project,
                        due_at_ms,
                        reason,
                    } => {
                        eprintln!(
                            "daemon: dreamer scheduler skipped {project} slot {due_at_ms}: {reason}"
                        );
                    }
                    TickEvent::Retained {
                        project,
                        due_at_ms,
                        reason,
                    } => {
                        store_failed = true;
                        eprintln!(
                            "daemon: dreamer scheduler could not lease {project} slot {due_at_ms}, which stays due: {reason}"
                        );
                    }
                    TickEvent::Deferred { reason } => {
                        store_failed = true;
                        eprintln!(
                            "daemon: dreamer scheduler could not read its projects: {reason}"
                        );
                    }
                }
            }
            let wait = if store_failed {
                IDLE_POLL
            } else {
                self.earliest_due()
                    .map(|due| {
                        Duration::from_millis(due.saturating_sub(self.clock.now_ms()).max(0) as u64)
                    })
                    .unwrap_or(IDLE_POLL)
                    .min(IDLE_POLL)
            };
            tokio::select! {
                () = cancel.cancelled() => return,
                () = self.clock.sleep(wait) => {}
            }
        }
    }

    fn earliest_due(&self) -> Option<i64> {
        self.next_due.values().map(|(_, due)| *due).min()
    }

    fn registration_generation(&mut self, store: &MemoryStore) -> Result<i64, String> {
        if let Some(generation) = self.registration_generation {
            return Ok(generation);
        }
        let generation = store
            .next_dreamer_scheduler_generation(SCHEDULER_INSTANCE)
            .map_err(|error| error.to_string())?;
        self.registration_generation = Some(generation);
        Ok(generation)
    }

    /// One evaluation at the clock's current instant. Projects due at or
    /// before it run in order of their due instant, then project name, so a
    /// backlog drains oldest first. A project's next instant is recomputed
    /// from the clock after its run returns. Lease operations read the clock immediately
    /// before each lease, so a long-running project does not shorten a later
    /// project's lease.
    pub(crate) async fn tick(&mut self, host: &dyn SchedulerHost) -> Vec<TickEvent> {
        let now_ms = self.clock.now_ms();
        let projects = match host.scheduled_projects() {
            Ok(projects) => projects,
            Err(reason) => return vec![TickEvent::Deferred { reason }],
        };
        let due = self.due_projects(&projects, now_ms);
        let mut events = Vec::new();
        for (due_at_ms, project) in due {
            let event = self.run_slot(host, project, due_at_ms).await;
            if !matches!(event, TickEvent::Retained { .. }) {
                self.advance(project, self.clock.now_ms());
            }
            events.push(event);
        }
        events
    }

    /// Reconciles the due table with the projects seen now and returns those
    /// due at or before `now_ms`, oldest first.
    fn due_projects<'a>(
        &mut self,
        projects: &'a [ScheduledProject],
        now_ms: i64,
    ) -> Vec<(i64, &'a ScheduledProject)> {
        self.next_due
            .retain(|project, _| projects.iter().any(|seen| seen.project == *project));
        let mut due: BTreeMap<(i64, &str), &ScheduledProject> = BTreeMap::new();
        for project in projects {
            let entry = self
                .next_due
                .entry(project.project.clone())
                .or_insert_with(|| {
                    (
                        project.schedule.clone(),
                        next_due(&project.schedule, now_ms),
                    )
                });
            if entry.0 != project.schedule {
                *entry = (
                    project.schedule.clone(),
                    next_due(&project.schedule, now_ms),
                );
            }
            if entry.1 <= now_ms {
                due.insert((entry.1, project.project.as_str()), project);
            }
        }
        due.into_iter()
            .map(|((due_at_ms, _), project)| (due_at_ms, project))
            .collect()
    }

    fn advance(&mut self, project: &ScheduledProject, now_ms: i64) {
        self.next_due.insert(
            project.project.clone(),
            (
                project.schedule.clone(),
                next_due(&project.schedule, now_ms),
            ),
        );
    }

    /// Leases the slot, runs it, and records the outcome on the lease. A slot
    /// another live claim holds is skipped. When the ledger hands back a
    /// predecessor's live claim instead of a fresh one, the run is keyed by
    /// that claim's due instant, so the interrupted receipt is what resumes and
    /// no second attempt is dispatched for it.
    async fn run_slot(
        &mut self,
        host: &dyn SchedulerHost,
        project: &ScheduledProject,
        due_at_ms: i64,
    ) -> TickEvent {
        let store = host.store();
        let skipped = |reason: String| TickEvent::Skipped {
            project: project.project.clone(),
            due_at_ms,
            reason,
        };
        let retained = |reason: String| TickEvent::Retained {
            project: project.project.clone(),
            due_at_ms,
            reason,
        };
        let registration_generation = match self.registration_generation(store) {
            Ok(generation) => generation,
            Err(error) => return retained(format!("no registration generation: {error}")),
        };
        let claim: LeaseClaim = match store.acquire_dreamer_task(
            &project.project,
            &slot_command_id(REVIEW_USER_MEMORIES_TASK, due_at_ms),
            SCHEDULER_INSTANCE,
            SCHEDULER_SLOT,
            registration_generation,
            REVIEW_USER_MEMORIES_TASK_ID,
            due_at_ms,
            self.clock.now_ms(),
        ) {
            Ok(LeaseAcquireOutcome::Claim { claim, .. }) => claim,
            Ok(LeaseAcquireOutcome::NoWork { .. }) => {
                return skipped("another scheduler holds the task".to_string());
            }
            Ok(LeaseAcquireOutcome::Terminal { kind, .. }) => {
                return skipped(format!("the slot already ended {kind}"));
            }
            Ok(LeaseAcquireOutcome::Expired) => {
                return skipped("the slot's lease expired before it ran".to_string());
            }
            Ok(LeaseAcquireOutcome::AuthorityChanged) => {
                return skipped("the memories authority is no longer MODULE".to_string());
            }
            Ok(LeaseAcquireOutcome::Busy) => {
                return skipped("the lease ledger is at its cap".to_string());
            }
            Ok(LeaseAcquireOutcome::Invalid) => {
                return skipped("the lease ledger refused the acquisition".to_string());
            }
            Err(error) => return retained(format!("lease acquisition failed: {error}")),
        };
        // The claim's `source_revision` is the due instant it was leased for:
        // this slot's for a fresh claim, an earlier one for a rebound claim.
        let leased_due_at_ms = claim.source_revision;
        let command_id = slot_command_id(REVIEW_USER_MEMORIES_TASK, leased_due_at_ms);
        let outcome = host
            .run_task(project, REVIEW_USER_MEMORIES_TASK, &command_id)
            .await;
        let response = match &outcome {
            TaskRunOutcome::Ran { response } => response.clone(),
            TaskRunOutcome::NotRunnable { reason } => {
                json!({"ok": false, "code": "dreamer_task_not_runnable", "message": reason})
            }
        };
        // The claim's completion carries the run's reply; a conflict means the
        // lease expired or the authority moved under a run that already ended
        // through its own receipt, which is where the outcome is recorded.
        match store.complete_dreamer_task(
            &project.project,
            &claim.claim_id,
            &format!("{command_id}:complete"),
            SCHEDULER_INSTANCE,
            SCHEDULER_SLOT,
            &response.to_string(),
            self.clock.now_ms(),
        ) {
            Ok(LeaseCompleteOutcome::Applied { .. } | LeaseCompleteOutcome::Replayed { .. }) => {}
            Ok(LeaseCompleteOutcome::Conflict { kind }) => {
                eprintln!(
                    "daemon: dreamer scheduler lease completion for {} slot {leased_due_at_ms} was {kind}; the run's receipt holds its outcome",
                    project.project
                );
            }
            Err(error) => {
                eprintln!(
                    "daemon: dreamer scheduler lease completion for {} slot {leased_due_at_ms} failed: {error}",
                    project.project
                );
            }
        }
        TickEvent::Ran {
            project: project.project.clone(),
            due_at_ms: leased_due_at_ms,
            outcome,
        }
    }
}

/// The next cron instant strictly after `now_ms`; an expression with no
/// occurrence in the parser's window never comes due.
fn next_due(schedule: &str, now_ms: i64) -> i64 {
    next_cron_occurrence(schedule, now_ms, &chrono::Local).unwrap_or(i64::MAX)
}

/// A clock tests move by hand. `sleep` returns at once unless parked, so a
/// `run` loop is driven by the test; a parked clock keeps `sleep` pending so
/// cancellation can be observed mid-wait.
#[cfg(test)]
pub(crate) struct ManualClock {
    now_ms: std::sync::atomic::AtomicI64,
    park: bool,
    /// Every duration `sleep` was asked for, in order.
    sleeps: std::sync::Mutex<Vec<Duration>>,
}

#[cfg(test)]
impl ManualClock {
    pub(crate) fn at(now_ms: i64) -> Arc<Self> {
        Arc::new(Self {
            now_ms: std::sync::atomic::AtomicI64::new(now_ms),
            park: false,
            sleeps: std::sync::Mutex::new(Vec::new()),
        })
    }

    pub(crate) fn parked_at(now_ms: i64) -> Arc<Self> {
        Arc::new(Self {
            now_ms: std::sync::atomic::AtomicI64::new(now_ms),
            park: true,
            sleeps: std::sync::Mutex::new(Vec::new()),
        })
    }

    pub(crate) fn advance(&self, by: Duration) {
        self.now_ms
            .fetch_add(by.as_millis() as i64, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) fn sleeps(&self) -> Vec<Duration> {
        self.sleeps.lock().unwrap().clone()
    }

    pub(crate) fn shared(self: &Arc<Self>) -> Arc<dyn SchedulerClock> {
        let clock: Arc<Self> = Arc::clone(self);
        clock
    }
}

#[cfg(test)]
#[async_trait]
impl SchedulerClock for ManualClock {
    fn now_ms(&self) -> i64 {
        self.now_ms.load(std::sync::atomic::Ordering::SeqCst)
    }

    async fn sleep(&self, duration: Duration) {
        self.sleeps.lock().unwrap().push(duration);
        if self.park {
            std::future::pending::<()>().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Mutex;

    use memory_store::DREAMER_TASK_LEASE_MS;

    use super::*;
    use crate::dev_descriptor_at;

    /// A host over a real store whose projects and run replies are scripted;
    /// every `run_task` call is recorded in order, and the clock may be moved
    /// while a run is in progress.
    struct ScriptedHost {
        store: Arc<MemoryStore>,
        projects: Mutex<Vec<ScheduledProject>>,
        runs: Mutex<Vec<(String, String)>>,
        runnable: bool,
        /// Advanced by this much during every run.
        run_takes: Option<(Arc<ManualClock>, Duration)>,
        /// The next `scheduled_projects` fails with this instead of answering.
        fail_projects_once: Mutex<Option<String>>,
        /// Every run parks on this until it is notified.
        block_runs: Option<Arc<tokio::sync::Notify>>,
    }

    #[async_trait]
    impl SchedulerHost for ScriptedHost {
        fn store(&self) -> &MemoryStore {
            &self.store
        }

        fn scheduled_projects(&self) -> Result<Vec<ScheduledProject>, String> {
            if let Some(reason) = self.fail_projects_once.lock().unwrap().take() {
                return Err(reason);
            }
            Ok(self.projects.lock().unwrap().clone())
        }

        async fn run_task(
            &self,
            project: &ScheduledProject,
            _task: &str,
            command_id: &str,
        ) -> TaskRunOutcome {
            self.runs
                .lock()
                .unwrap()
                .push((project.project.clone(), command_id.to_string()));
            if let Some((clock, by)) = &self.run_takes {
                clock.advance(*by);
            }
            if let Some(gate) = &self.block_runs {
                gate.notified().await;
            }
            if self.runnable {
                TaskRunOutcome::Ran {
                    response: json!({"ok": true}),
                }
            } else {
                TaskRunOutcome::NotRunnable {
                    reason: "scripted".to_string(),
                }
            }
        }
    }

    fn open_store(dir: &Path) -> Arc<MemoryStore> {
        let data_home = dir.join("data");
        std::fs::create_dir_all(&data_home).unwrap();
        Arc::new(MemoryStore::open(&dev_descriptor_at(data_home.to_str().unwrap())).unwrap())
    }

    /// Moves `identity` to `MODULE` memories authority and returns its generation.
    fn activate(store: &MemoryStore, identity: &str) -> u64 {
        let preparing = store
            .authority_begin_prepare("context", identity, "memories")
            .unwrap();
        let checksum = store
            .authority_seed_checksum("context", identity, "memories")
            .unwrap();
        store
            .authority_verify_prepare(
                "context",
                identity,
                "memories",
                preparing.generation,
                &checksum,
                &checksum,
            )
            .unwrap();
        let module = store
            .authority_ack_prepare("context", identity, "memories", preparing.generation)
            .unwrap();
        assert_eq!(module.state, "MODULE");
        module.generation
    }

    fn project(store: &MemoryStore, identity: &str, schedule: &str) -> ScheduledProject {
        ScheduledProject {
            project: identity.to_string(),
            route_root: PathBuf::from("/project"),
            authority_generation: activate(store, identity),
            schedule: schedule.to_string(),
        }
    }

    fn scripted(store: &Arc<MemoryStore>, projects: Vec<ScheduledProject>) -> ScriptedHost {
        ScriptedHost {
            store: Arc::clone(store),
            projects: Mutex::new(projects),
            runs: Mutex::new(Vec::new()),
            runnable: true,
            run_takes: None,
            fail_projects_once: Mutex::new(None),
            block_runs: None,
        }
    }

    const MINUTE: Duration = Duration::from_secs(60);
    const MINUTE_MS: i64 = 60_000;
    /// 2026-01-01T00:00:00Z. Every-fifteen-minutes schedules come due at the same
    /// instants in every zone whose offset is a multiple of fifteen minutes.
    const T0: i64 = 1_767_225_600_000;

    fn ran(events: &[TickEvent]) -> Vec<(&str, i64)> {
        events
            .iter()
            .filter_map(|event| match event {
                TickEvent::Ran {
                    project, due_at_ms, ..
                } => Some((project.as_str(), *due_at_ms)),
                TickEvent::Skipped { .. }
                | TickEvent::Retained { .. }
                | TickEvent::Deferred { .. } => None,
            })
            .collect()
    }

    fn command(due_at_ms: i64) -> String {
        slot_command_id(REVIEW_USER_MEMORIES_TASK, due_at_ms)
    }

    /// Leases `due` for `identity` the way a scheduler that then died would
    /// have, and returns the claim it left live.
    fn predecessor_claim(
        store: &MemoryStore,
        identity: &str,
        registration_generation: i64,
        due: i64,
    ) -> LeaseClaim {
        match store
            .acquire_dreamer_task(
                identity,
                &command(due),
                SCHEDULER_INSTANCE,
                SCHEDULER_SLOT,
                registration_generation,
                REVIEW_USER_MEMORIES_TASK_ID,
                due,
                due,
            )
            .unwrap()
        {
            LeaseAcquireOutcome::Claim { claim, .. } => claim,
            other => panic!("{other:?}"),
        }
    }

    /// Replays `acquisition` for the scheduler's own identity, which reports the
    /// slot's recorded decision without changing it.
    fn slot_state(
        store: &MemoryStore,
        identity: &str,
        due: i64,
    ) -> memory_store::DreamerTaskAcquireOutcome {
        store
            .acquire_dreamer_task(
                identity,
                &command(due),
                SCHEDULER_INSTANCE,
                SCHEDULER_SLOT,
                i64::MAX,
                REVIEW_USER_MEMORIES_TASK_ID,
                due,
                due + 10 * DREAMER_TASK_LEASE_MS,
            )
            .unwrap()
    }

    #[tokio::test]
    async fn a_task_runs_only_once_its_cron_instant_has_passed() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path());
        let host = scripted(&store, vec![project(&store, "git:a", "*/15 * * * *")]);
        let clock = ManualClock::at(T0 + 1_000);
        let mut scheduler = DreamerScheduler::new(clock.shared());

        assert!(scheduler.tick(&host).await.is_empty());
        clock.advance(14 * MINUTE);
        assert!(scheduler.tick(&host).await.is_empty());
        clock.advance(MINUTE);
        let events = scheduler.tick(&host).await;
        assert_eq!(ran(&events), vec![("git:a", T0 + 15 * MINUTE_MS)]);
        assert_eq!(
            host.runs.lock().unwrap().as_slice(),
            &[("git:a".to_string(), command(T0 + 15 * MINUTE_MS))]
        );
        // The same instant does not run twice; the next one does.
        assert!(scheduler.tick(&host).await.is_empty());
        clock.advance(15 * MINUTE);
        assert_eq!(
            ran(&scheduler.tick(&host).await),
            vec![("git:a", T0 + 30 * MINUTE_MS)]
        );
    }

    #[tokio::test]
    async fn missed_slots_are_not_back_filled_and_a_backlog_drains_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path());
        // Names sort the other way round from due instants, so the order the
        // host sees is chronological, not lexical.
        let host = scripted(
            &store,
            vec![
                project(&store, "git:a", "*/15 * * * *"),
                project(&store, "git:z", "*/5 * * * *"),
            ],
        );
        let clock = ManualClock::at(T0 + 1_000);
        let mut scheduler = DreamerScheduler::new(clock.shared());
        assert!(scheduler.tick(&host).await.is_empty());

        // An hour passes with no ticks: each project runs once, for its first
        // missed slot, oldest first; the slots in between are skipped.
        clock.advance(60 * MINUTE);
        let events = scheduler.tick(&host).await;
        assert_eq!(
            ran(&events),
            vec![
                ("git:z", T0 + 5 * MINUTE_MS),
                ("git:a", T0 + 15 * MINUTE_MS),
            ]
        );
        assert_eq!(
            host.runs.lock().unwrap().as_slice(),
            &[
                ("git:z".to_string(), command(T0 + 5 * MINUTE_MS)),
                ("git:a".to_string(), command(T0 + 15 * MINUTE_MS)),
            ]
        );
        assert!(scheduler.tick(&host).await.is_empty());
        // Scheduling continues from the tick that drained the backlog.
        clock.advance(5 * MINUTE);
        assert_eq!(
            ran(&scheduler.tick(&host).await),
            vec![("git:z", T0 + 65 * MINUTE_MS)]
        );
    }

    /// A run that takes real time does not eat into the lease of the project
    /// behind it: each acquisition reads the clock as it happens.
    #[tokio::test]
    async fn each_slot_is_leased_at_the_instant_it_is_acquired() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path());
        let clock = ManualClock::at(T0 + 1_000);
        let mut host = scripted(
            &store,
            vec![
                project(&store, "git:a", "*/15 * * * *"),
                project(&store, "git:b", "*/15 * * * *"),
            ],
        );
        host.run_takes = Some((Arc::clone(&clock), 19 * MINUTE));
        let mut scheduler = DreamerScheduler::new(clock.shared());
        assert!(scheduler.tick(&host).await.is_empty());
        clock.advance(15 * MINUTE);
        let events = scheduler.tick(&host).await;
        assert_eq!(events.len(), 2, "{events:?}");
        // Both slots ended `applied`: had the second been leased at tick start,
        // its 20-minute lease would have expired 38 minutes later, before its
        // completion, and the ledger would hold `expired` instead.
        for identity in ["git:a", "git:b"] {
            assert!(
                matches!(
                    slot_state(&store, identity, T0 + 15 * MINUTE_MS),
                    LeaseAcquireOutcome::Terminal { ref kind, .. } if kind == "applied"
                ),
                "{identity}"
            );
        }
        assert_eq!(
            scheduler.next_due["git:a"].1,
            T0 + 45 * MINUTE_MS,
            "a's run crossed the 30-minute slot; its next instant counts from the post-run clock"
        );
        assert_eq!(
            scheduler.next_due["git:b"].1,
            T0 + 60 * MINUTE_MS,
            "b's run crossed the 45-minute slot"
        );
    }

    /// The scheduler waits for the next post-run instant instead of re-ticking an elapsed slot.
    #[tokio::test]
    async fn a_run_that_crosses_the_next_slot_does_not_back_fill_it() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path());
        let clock = ManualClock::at(T0 + 1_000);
        let mut host = scripted(&store, vec![project(&store, "git:a", "*/5 * * * *")]);
        host.run_takes = Some((Arc::clone(&clock), 12 * MINUTE));
        let mut scheduler = DreamerScheduler::new(clock.shared());
        assert!(scheduler.tick(&host).await.is_empty());

        clock.advance(5 * MINUTE);
        let first = T0 + 5 * MINUTE_MS;
        assert_eq!(ran(&scheduler.tick(&host).await), vec![("git:a", first)]);
        assert_eq!(clock.now_ms(), first + 12 * MINUTE_MS + 1_000);
        assert_eq!(
            scheduler.earliest_due(),
            Some(T0 + 20 * MINUTE_MS),
            "the slots at 10 and 15 minutes fell inside the run"
        );
        assert!(scheduler.tick(&host).await.is_empty());
        clock.advance(3 * MINUTE);
        assert_eq!(
            ran(&scheduler.tick(&host).await),
            vec![("git:a", T0 + 20 * MINUTE_MS)]
        );
        assert_eq!(host.runs.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_schedule_change_or_removal_reschedules_from_now() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path());
        let mut scheduled = project(&store, "git:a", "*/15 * * * *");
        let host = scripted(&store, vec![scheduled.clone()]);
        let clock = ManualClock::at(T0 + 1_000);
        let mut scheduler = DreamerScheduler::new(clock.shared());
        assert!(scheduler.tick(&host).await.is_empty());

        // The new schedule counts from the tick that first sees it, so the
        // slot at five minutes is not due: the next one is.
        scheduled.schedule = "*/5 * * * *".to_string();
        *host.projects.lock().unwrap() = vec![scheduled.clone()];
        clock.advance(5 * MINUTE);
        assert!(scheduler.tick(&host).await.is_empty());
        clock.advance(5 * MINUTE);
        let events = scheduler.tick(&host).await;
        assert_eq!(ran(&events), vec![("git:a", T0 + 10 * MINUTE_MS)]);

        host.projects.lock().unwrap().clear();
        clock.advance(60 * MINUTE);
        assert!(scheduler.tick(&host).await.is_empty());
        assert_eq!(scheduler.earliest_due(), None);

        // Re-added after an hour: the next slot counts from now, not from the
        // slot it left at.
        *host.projects.lock().unwrap() = vec![scheduled];
        assert!(scheduler.tick(&host).await.is_empty());
        assert_eq!(host.runs.lock().unwrap().len(), 1);
    }

    /// A host that cannot report its projects is not a host with none: the
    /// tick is deferred, the due table keeps the pending slot, and the next
    /// tick runs the pending slot instead of rescheduling from now.
    #[tokio::test]
    async fn a_failed_project_lookup_defers_the_tick_and_keeps_the_pending_slot() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path());
        let host = scripted(&store, vec![project(&store, "git:a", "*/15 * * * *")]);
        let clock = ManualClock::at(T0 + 1_000);
        let mut scheduler = DreamerScheduler::new(clock.shared());
        assert!(scheduler.tick(&host).await.is_empty());
        let due = T0 + 15 * MINUTE_MS;

        clock.advance(15 * MINUTE);
        *host.fail_projects_once.lock().unwrap() = Some("store unavailable".to_string());
        let events = scheduler.tick(&host).await;
        assert_eq!(
            events,
            vec![TickEvent::Deferred {
                reason: "store unavailable".to_string()
            }]
        );
        assert!(host.runs.lock().unwrap().is_empty(), "nothing ran");
        assert_eq!(
            scheduler.earliest_due(),
            Some(due),
            "the pending slot is still due"
        );

        // The store answers again: the slot that was due runs, not a later one.
        clock.advance(MINUTE);
        assert_eq!(ran(&scheduler.tick(&host).await), vec![("git:a", due)]);
        assert_eq!(
            host.runs.lock().unwrap().as_slice(),
            &[("git:a".to_string(), command(due))]
        );
    }

    /// A lease the ledger could not answer for is not a lease that was
    /// refused: the slot keeps its due instant and the next tick leases and
    /// runs that slot, not the one after it.
    #[tokio::test]
    async fn a_failed_lease_acquisition_retains_the_slot_for_the_next_tick() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path());
        let host = scripted(&store, vec![project(&store, "git:a", "*/15 * * * *")]);
        let clock = ManualClock::at(T0 + 1_000);
        let mut scheduler = DreamerScheduler::new(clock.shared());
        assert!(scheduler.tick(&host).await.is_empty());
        let due = T0 + 15 * MINUTE_MS;

        clock.advance(15 * MINUTE);
        store.fail_next_dreamer_task_acquire_for_test();
        let events = scheduler.tick(&host).await;
        assert!(
            matches!(
                &events[..],
                [TickEvent::Retained { project, due_at_ms, reason }]
                    if project == "git:a" && *due_at_ms == due
                        && reason.contains("injected dreamer task acquire failure")
            ),
            "{events:?}"
        );
        assert!(host.runs.lock().unwrap().is_empty(), "nothing ran");
        assert_eq!(scheduler.earliest_due(), Some(due), "the slot is still due");

        clock.advance(MINUTE);
        assert_eq!(ran(&scheduler.tick(&host).await), vec![("git:a", due)]);
        assert_eq!(
            host.runs.lock().unwrap().as_slice(),
            &[("git:a".to_string(), command(due))]
        );
        assert_eq!(scheduler.earliest_due(), Some(due + 15 * MINUTE_MS));
    }

    /// A restarted scheduler outranks its predecessor from the ledger, not
    /// the wall clock, so a clock that stepped back still recovers the
    /// predecessor's live claim, and the run it dispatches is the interrupted
    /// slot's, under that slot's command id.
    #[tokio::test]
    async fn a_predecessor_claim_is_recovered_under_its_own_slot_by_the_next_generation() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path());
        let scheduled = project(&store, "git:a", "*/15 * * * *");
        let due = T0 + 15 * MINUTE_MS;
        let next = due + 15 * MINUTE_MS;
        let predecessor = predecessor_claim(&store, "git:a", 7, due);

        // The successor's clock is behind the predecessor's: generations come
        // from the ledger, so it still outranks the live claim.
        let clock = ManualClock::at(T0);
        let host = scripted(&store, vec![scheduled.clone()]);
        let mut successor = DreamerScheduler::new(clock.shared());
        assert!(successor.tick(&host).await.is_empty());
        assert_eq!(successor.registration_generation, None);
        clock.advance(15 * MINUTE);
        // The successor's own first slot is `due` too (it started before it),
        // so the recovered slot and the due slot coincide here.
        let events = successor.tick(&host).await;
        assert_eq!(ran(&events), vec![("git:a", due)]);
        assert_eq!(successor.registration_generation, Some(8));
        assert_eq!(
            host.runs.lock().unwrap().as_slice(),
            &[("git:a".to_string(), command(due))]
        );
        assert!(matches!(
            slot_state(&store, "git:a", due),
            LeaseAcquireOutcome::Terminal { ref kind, .. } if kind == "applied"
        ));

        // A predecessor claim rebound at a later slot: the run is the
        // interrupted slot's, and the later slot is consumed without a run of
        // its own.
        let later_due = next + 15 * MINUTE_MS;
        let stranded = predecessor_claim(&store, "git:a", 8, next);
        assert_ne!(stranded.claim_id, predecessor.claim_id);
        let clock = ManualClock::at(next + 1);
        let host = scripted(&store, vec![scheduled.clone()]);
        let mut successor = DreamerScheduler::new(clock.shared());
        assert!(successor.tick(&host).await.is_empty());
        clock.advance(15 * MINUTE);
        let events = successor.tick(&host).await;
        assert_eq!(ran(&events), vec![("git:a", next)]);
        assert_eq!(
            host.runs.lock().unwrap().as_slice(),
            &[("git:a".to_string(), command(next))]
        );
        assert!(scheduler_holds_no_live_claim(&store, "git:a"));
        // The later slot's acquisition id is bound to the recovered claim, so
        // a retry of it replays the terminal decision without running.
        let mut retry = DreamerScheduler::new(ManualClock::at(later_due - 1).shared());
        assert!(retry.tick(&host).await.is_empty());
        retry.clock = ManualClock::at(later_due).shared();
        let events = retry.tick(&host).await;
        assert!(
            matches!(&events[..], [TickEvent::Skipped { reason, .. }] if reason.contains("already ended")),
            "{events:?}"
        );
        assert_eq!(host.runs.lock().unwrap().len(), 1);
    }

    fn scheduler_holds_no_live_claim(store: &MemoryStore, identity: &str) -> bool {
        // A fresh acquisition for a far-future slot leases at once only when
        // nothing live holds the task; abandon it so the ledger is unchanged.
        let probe = T0 + 1_000 * MINUTE_MS;
        match store
            .acquire_dreamer_task(
                identity,
                &command(probe),
                "probe",
                0,
                1,
                REVIEW_USER_MEMORIES_TASK_ID,
                probe,
                probe,
            )
            .unwrap()
        {
            LeaseAcquireOutcome::Claim { claim, .. } => {
                store
                    .abandon_dreamer_task(identity, &claim.claim_id, "probe", 0, probe)
                    .unwrap();
                true
            }
            _ => false,
        }
    }

    /// Whether the predecessor's lease has expired decides between rebinding
    /// its claim and leasing a fresh one; either way exactly one run happens.
    #[tokio::test]
    async fn an_expired_predecessor_lease_yields_a_fresh_claim_a_live_one_is_rebound() {
        for (offset, expired) in [(-1, false), (0, true)] {
            let dir = tempfile::tempdir().unwrap();
            let store = open_store(dir.path());
            let scheduled = project(&store, "git:a", "*/15 * * * *");
            let due = T0 + 15 * MINUTE_MS;
            let predecessor = predecessor_claim(&store, "git:a", 1, due);
            let clock = ManualClock::at(due + DREAMER_TASK_LEASE_MS + offset - 15 * MINUTE_MS);
            let host = scripted(&store, vec![scheduled]);
            let mut successor = DreamerScheduler::new(clock.shared());
            assert!(successor.tick(&host).await.is_empty(), "{offset}");
            clock.advance(15 * MINUTE);
            let events = successor.tick(&host).await;
            assert_eq!(events.len(), 1, "{offset}: {events:?}");
            assert_eq!(host.runs.lock().unwrap().len(), 1, "{offset}");
            if expired {
                // The predecessor's row still carries its own acquisition id,
                // and replaying it reports the collected lease.
                let predecessor_now = slot_state(&store, "git:a", due);
                assert!(
                    matches!(&predecessor_now, LeaseAcquireOutcome::Terminal { kind, .. } if kind == "expired"),
                    "{predecessor_now:?}"
                );
                assert_ne!(ran(&events), vec![("git:a", due)], "a fresh slot ran");
                assert_ne!(host.runs.lock().unwrap()[0].1, command(due));
            } else {
                assert_eq!(ran(&events), vec![("git:a", due)], "the rebound slot ran");
                assert_eq!(
                    host.runs.lock().unwrap()[0].1,
                    command(due),
                    "under the interrupted slot's command id"
                );
            }
            assert!(
                scheduler_holds_no_live_claim(&store, "git:a"),
                "{offset}: the predecessor claim {} is settled",
                predecessor.claim_id
            );
        }
    }

    #[tokio::test]
    async fn a_task_without_inputs_settles_its_lease_as_not_runnable() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path());
        let mut host = scripted(&store, vec![project(&store, "git:a", "*/15 * * * *")]);
        host.runnable = false;
        let clock = ManualClock::at(T0 + 1_000);
        let mut scheduler = DreamerScheduler::new(clock.shared());
        scheduler.tick(&host).await;
        clock.advance(15 * MINUTE);
        let due = T0 + 15 * MINUTE_MS;
        let events = scheduler.tick(&host).await;
        assert!(
            matches!(
                &events[..],
                [TickEvent::Ran {
                    outcome: TaskRunOutcome::NotRunnable { .. },
                    ..
                }]
            ),
            "{events:?}"
        );
        // The slot ended on the ledger with the not-runnable reply, so no
        // scheduler retries it.
        match slot_state(&store, "git:a", due) {
            LeaseAcquireOutcome::Terminal { kind, response } => {
                assert_eq!(kind, "applied");
                let response: Value = serde_json::from_str(&response.unwrap()).unwrap();
                assert_eq!(response["code"], json!("dreamer_task_not_runnable"));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(host.runs.lock().unwrap().len(), 1);
    }

    /// Cancellation during a run drops the tick where it stands: the loop
    /// returns without waiting for the run, and the project still due behind
    /// it is not leased.
    #[tokio::test]
    async fn run_stops_mid_tick_at_cancellation_without_leasing_the_next_slot() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path());
        let gate = Arc::new(tokio::sync::Notify::new());
        let mut host = scripted(
            &store,
            vec![
                project(&store, "git:a", "*/15 * * * *"),
                project(&store, "git:b", "*/15 * * * *"),
            ],
        );
        host.block_runs = Some(Arc::clone(&gate));
        let host = Arc::new(host);
        let clock = ManualClock::parked_at(T0 + 1_000);
        let mut scheduler = DreamerScheduler::new(clock.shared());
        assert!(scheduler.tick(host.as_ref()).await.is_empty());
        clock.advance(15 * MINUTE);
        let cancel = CancellationToken::new();
        let dyn_host: Arc<dyn SchedulerHost> = Arc::clone(&host) as Arc<dyn SchedulerHost>;
        let running = tokio::spawn(scheduler.run(dyn_host, cancel.clone()));
        tokio::task::yield_now().await;
        assert_eq!(
            host.runs.lock().unwrap().len(),
            1,
            "the first slot is running and parked"
        );
        assert!(!running.is_finished());
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(5), running)
            .await
            .expect("run returns without waiting for the parked run")
            .unwrap();
        assert_eq!(
            host.runs.lock().unwrap().as_slice(),
            &[("git:a".to_string(), command(T0 + 15 * MINUTE_MS))],
            "the second project was never leased or run"
        );
        assert!(
            matches!(
                slot_state(&store, "git:b", T0 + 15 * MINUTE_MS),
                LeaseAcquireOutcome::Claim {
                    replayed: false,
                    ..
                }
            ),
            "no decision was recorded for git:b's slot"
        );
    }

    /// Cancellation ends a loop parked in its wait without another tick.
    #[tokio::test]
    async fn run_stops_at_cancellation() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path());
        let host = Arc::new(scripted(
            &store,
            vec![project(&store, "git:a", "*/15 * * * *")],
        ));
        let clock = ManualClock::parked_at(T0 + 1_000);
        let scheduler = DreamerScheduler::new(clock.shared());
        let cancel = CancellationToken::new();
        let dyn_host: Arc<dyn SchedulerHost> = Arc::clone(&host) as Arc<dyn SchedulerHost>;
        let running = tokio::spawn(scheduler.run(dyn_host, cancel.clone()));
        tokio::task::yield_now().await;
        assert!(!running.is_finished(), "the loop is parked in its wait");
        clock.advance(15 * MINUTE);
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(5), running)
            .await
            .expect("run returns once cancelled")
            .unwrap();
        assert!(host.runs.lock().unwrap().is_empty(), "no tick after cancel");
    }

    /// A deferred tick leaves a slot due, which would otherwise make the loop
    /// re-tick at once; a failing store is retried at the idle poll instead.
    #[tokio::test]
    async fn run_waits_the_idle_poll_after_a_deferred_tick_or_a_retained_slot() {
        for fail_at_lease in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let store = open_store(dir.path());
            let host = Arc::new(scripted(
                &store,
                vec![project(&store, "git:a", "*/15 * * * *")],
            ));
            let clock = ManualClock::parked_at(T0 + 1_000);
            let mut scheduler = DreamerScheduler::new(clock.shared());
            assert!(scheduler.tick(host.as_ref()).await.is_empty());
            clock.advance(15 * MINUTE);
            if fail_at_lease {
                store.fail_next_dreamer_task_acquire_for_test();
            } else {
                *host.fail_projects_once.lock().unwrap() = Some("store unavailable".to_string());
            }

            let cancel = CancellationToken::new();
            let dyn_host: Arc<dyn SchedulerHost> = Arc::clone(&host) as Arc<dyn SchedulerHost>;
            let running = tokio::spawn(scheduler.run(dyn_host, cancel.clone()));
            tokio::task::yield_now().await;
            assert_eq!(
                clock.sleeps(),
                vec![IDLE_POLL],
                "{fail_at_lease}: not an immediate re-tick"
            );
            assert!(host.runs.lock().unwrap().is_empty());
            cancel.cancel();
            tokio::time::timeout(Duration::from_secs(5), running)
                .await
                .expect("run returns once cancelled")
                .unwrap();
        }
    }
}
