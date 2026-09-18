//! Rust-owned scheduling of MemoryClassifier tasks.
//!
//! Each bound project's cron schedule is read from its effective
//! configuration; a due task is leased through the shared task-lease ledger
//! and run through the durable `memory_classifier.run_task` protocol under a command id
//! derived from the due instant, so an interrupted run that is retried replays
//! or resumes its receipt instead of dispatching a second model call.
//!
//! Time enters through one [`SchedulerClock`] at this boundary; every decision
//! [`MemoryClassifierScheduler::tick`] makes reads that clock, so tests advance a manual
//! clock and never sleep.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use memory_store::curator_jobs::{
    CuratorJobError, CuratorJobRefusal, EnqueueOutcome, FrozenSelection, FrozenSelectionPage,
    FrozenSelectionState, ProducerBinding,
};
use memory_store::{LeaseAcquireOutcome, LeaseClaim, LeaseCompleteOutcome, MemoryStore};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::conditional_note_evaluation::next_cron_occurrence;

/// The task the user tier's schedule key names.
pub(crate) const REVIEW_USER_MEMORIES_TASK: &str = "review-user-memories";
/// Its identity on the lease ledger.
pub(crate) const REVIEW_USER_MEMORIES_TASK_ID: i64 = 1;
/// The bounded reclamation of obsolete message-index state.
pub(crate) const MESSAGE_INDEX_CLEANUP_TASK: &str = "message-index-cleanup";
pub(crate) const MESSAGE_INDEX_CLEANUP_TASK_ID: i64 = 2;
/// Selection of review targets for the Curator: freezes one page of eligible memories and enqueues them as review jobs.
pub(crate) const CURATOR_REVIEW_SELECTION_TASK: &str = "curator-review-selection";
pub(crate) const CURATOR_REVIEW_SELECTION_TASK_ID: i64 = 3;
/// The producer every scheduler-selected review job is reserved under.
pub(crate) const CURATOR_SELECTION_PRODUCER: &str = "memory-classifier-selection";

/// The task a scheduled slot runs. Every kind is leased through the same ledger under its own task identity and its own ledger slot, so a claim a dead scheduler left on one kind is recovered by that kind's next acquisition and never consumes another kind's slot; the host decides what a kind does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum ScheduledTask {
    ReviewUserMemories,
    /// No production host returns this kind: message-index cleanup has no enable path until its evidence gates exist, so only test hosts schedule it.
    #[cfg_attr(not(test), expect(dead_code))]
    MessageIndexCleanup,
    /// No production host returns this kind yet: Curator selection is enabled by the deployment activation gate, which does not exist until the lifecycle owner installs it, so only test hosts schedule it.
    CuratorReviewSelection,
}

impl ScheduledTask {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::ReviewUserMemories => REVIEW_USER_MEMORIES_TASK,
            Self::MessageIndexCleanup => MESSAGE_INDEX_CLEANUP_TASK,
            Self::CuratorReviewSelection => CURATOR_REVIEW_SELECTION_TASK,
        }
    }

    pub(crate) fn lease_id(self) -> i64 {
        match self {
            Self::ReviewUserMemories => REVIEW_USER_MEMORIES_TASK_ID,
            Self::MessageIndexCleanup => MESSAGE_INDEX_CLEANUP_TASK_ID,
            Self::CuratorReviewSelection => CURATOR_REVIEW_SELECTION_TASK_ID,
        }
    }

    /// The ledger slot the kind's claims live in; slot recovery is keyed by it.
    pub(crate) fn ledger_slot(self) -> i64 {
        match self {
            Self::ReviewUserMemories => SCHEDULER_SLOT,
            Self::MessageIndexCleanup => SCHEDULER_SLOT + 1,
            Self::CuratorReviewSelection => SCHEDULER_SLOT + 2,
        }
    }
}
/// The ledger session every scheduled run is recorded under.
pub(crate) const SCHEDULER_LEDGER_SESSION: &str = "eidnara-memory_classifier-scheduler";
/// The scheduler's worker identity on the lease ledger; one slot per daemon.
pub(crate) const SCHEDULER_INSTANCE: &str = "eidnara-memory_classifier-scheduler";
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
    pub(crate) task: ScheduledTask,
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
    /// The store could not answer; the slot keeps its lease and due instant.
    StoreUnavailable { reason: String },
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

    /// Selects the next page of review targets for `project` from `cursor` through the Kernel. `Err` is a store failure; the slot keeps its claim and due instant.
    fn select_review_page(
        &self,
        project: &ScheduledProject,
        cursor: Option<&str>,
    ) -> Result<FrozenSelectionPage, String>;
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
    /// The slot's frozen page could not be enqueued because review capacity or the metadata quota is full. The page stays frozen in its slot, the claim stays live, and the due instant does not move; the next tick offers the same page again once capacity has returned.
    CapacityDeferred {
        project: String,
        due_at_ms: i64,
        reason: String,
    },
}

/// The command id of one scheduled slot: the task and the instant it was due,
/// so a retry of the same slot replays its receipt.
pub(crate) fn slot_command_id(task: &str, due_at_ms: i64) -> String {
    format!("{task}@{due_at_ms}")
}

pub(crate) struct MemoryClassifierScheduler {
    clock: Arc<dyn SchedulerClock>,
    /// Allocated from the ledger on first use, above every generation this
    /// instance recorded before, so a claim left by a crashed scheduler is
    /// recovered by its successor under the shared protocol.
    registration_generation: Option<i64>,
    /// The next cron instant per project and task, computed from the schedule
    /// the project had when it was last seen.
    next_due: HashMap<(String, ScheduledTask), (String, i64)>,
}

impl MemoryClassifierScheduler {
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
                            "daemon: memory_classifier scheduler skipped {project} slot {due_at_ms}: {reason}"
                        );
                    }
                    TickEvent::Retained {
                        project,
                        due_at_ms,
                        reason,
                    } => {
                        store_failed = true;
                        eprintln!(
                            "daemon: memory_classifier scheduler could not lease {project} slot {due_at_ms}, which stays due: {reason}"
                        );
                    }
                    TickEvent::Deferred { reason } => {
                        store_failed = true;
                        eprintln!(
                            "daemon: memory_classifier scheduler could not read its projects: {reason}"
                        );
                    }
                    TickEvent::CapacityDeferred {
                        project,
                        due_at_ms,
                        reason,
                    } => {
                        store_failed = true;
                        eprintln!(
                            "daemon: memory_classifier scheduler holds {project} selection slot {due_at_ms} until capacity returns: {reason}"
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
            .next_memory_classifier_scheduler_generation(SCHEDULER_INSTANCE)
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
            if !matches!(
                event,
                TickEvent::Retained { .. } | TickEvent::CapacityDeferred { .. }
            ) {
                // The floor prevents a clock step backward during the run from selecting an earlier slot.
                self.advance(project, self.clock.now_ms().max(due_at_ms));
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
        self.next_due.retain(|(project, task), _| {
            projects
                .iter()
                .any(|seen| seen.project == *project && seen.task == *task)
        });
        let mut due: BTreeMap<(i64, &str, ScheduledTask), &ScheduledProject> = BTreeMap::new();
        for project in projects {
            let entry = self
                .next_due
                .entry((project.project.clone(), project.task))
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
                due.insert((entry.1, project.project.as_str(), project.task), project);
            }
        }
        due.into_iter()
            .map(|((due_at_ms, _, _), project)| (due_at_ms, project))
            .collect()
    }

    fn advance(&mut self, project: &ScheduledProject, now_ms: i64) {
        self.next_due.insert(
            (project.project.clone(), project.task),
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
        let claim: LeaseClaim = match store.acquire_memory_classifier_task(
            &project.project,
            &slot_command_id(project.task.name(), due_at_ms),
            SCHEDULER_INSTANCE,
            project.task.ledger_slot(),
            registration_generation,
            project.task.lease_id(),
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
        let command_id = slot_command_id(project.task.name(), leased_due_at_ms);
        if project.task == ScheduledTask::CuratorReviewSelection {
            // The selection slot completes inside the enqueue transaction, so the lease, the page, and the jobs move together or not at all.
            return match self.run_selection_slot(host, project, &claim, &command_id) {
                Ok(SelectionStep::Ran(outcome)) => TickEvent::Ran {
                    project: project.project.clone(),
                    due_at_ms: leased_due_at_ms,
                    outcome,
                },
                Ok(SelectionStep::CapacityDeferred(reason)) => TickEvent::CapacityDeferred {
                    project: project.project.clone(),
                    due_at_ms: leased_due_at_ms,
                    reason,
                },
                Err(reason) => retained(reason),
            };
        }
        let outcome = host
            .run_task(project, project.task.name(), &command_id)
            .await;
        let response = match &outcome {
            TaskRunOutcome::Ran { response } => response.clone(),
            TaskRunOutcome::NotRunnable { reason } => {
                json!({"ok": false, "code": "memory_classifier_task_not_runnable", "message": reason})
            }
            TaskRunOutcome::StoreUnavailable { reason } => {
                return retained(format!(
                    "the durable protocol could not reach the store: {reason}"
                ));
            }
        };
        // The claim's completion carries the run's reply; a conflict means the
        // lease expired or the authority moved under a run that already ended
        // through its own receipt, which is where the outcome is recorded.
        match store.complete_memory_classifier_task(
            &project.project,
            &claim.claim_id,
            &format!("{command_id}:complete"),
            SCHEDULER_INSTANCE,
            project.task.ledger_slot(),
            &response.to_string(),
            self.clock.now_ms(),
        ) {
            Ok(LeaseCompleteOutcome::Applied { .. } | LeaseCompleteOutcome::Replayed { .. }) => {}
            Ok(LeaseCompleteOutcome::Conflict { kind }) => {
                eprintln!(
                    "daemon: memory_classifier scheduler lease completion for {} slot {leased_due_at_ms} was {kind}; the run's receipt holds its outcome",
                    project.project
                );
            }
            Err(error) => return retained(format!("lease completion failed: {error}")),
        }
        TickEvent::Ran {
            project: project.project.clone(),
            due_at_ms: leased_due_at_ms,
            outcome,
        }
    }
}

/// How one selection slot ended: it ran to a recorded outcome, or capacity deferred it with the page retained.
enum SelectionStep {
    Ran(TaskRunOutcome),
    CapacityDeferred(String),
}

impl MemoryClassifierScheduler {
    /// One Curator selection slot: resume the project's frozen page if one exists, else select a page from the last enqueued cursor and freeze it under this slot's attempt, then enqueue it and complete the slot in one transaction. An empty selection completes the slot with nothing frozen; a capacity or quota deferral leaves the page frozen and the slot claimed for a later tick; an expired page completes the slot as failed without moving the cursor. `Err` keeps the slot due.
    fn run_selection_slot(
        &self,
        host: &dyn SchedulerHost,
        project: &ScheduledProject,
        claim: &LeaseClaim,
        command_id: &str,
    ) -> Result<SelectionStep, String> {
        let store = host.store();
        let now_ms = self.clock.now_ms();
        let producer = ProducerBinding {
            producer: CURATOR_SELECTION_PRODUCER.to_string(),
            firing_id: command_id.to_string(),
            ordinal: 0,
        };
        // This slot's own page first: a page it froze and could not enqueue resumes; one that expired or was already enqueued records that outcome and selects nothing new under this attempt.
        if let Some(own) = store
            .lookup_frozen_selection(&project.project, project.task.name(), command_id)
            .map_err(|error| format!("frozen selection lookup failed: {error}"))?
            && own.state != FrozenSelectionState::Frozen
        {
            let enqueued = own.state == FrozenSelectionState::Enqueued;
            let code = if enqueued {
                "curator_selection_enqueued"
            } else {
                "curator_selection_expired"
            };
            return self.complete_selection_slot(
                store,
                project,
                claim,
                command_id,
                None,
                json!({"ok": enqueued, "code": code}),
            );
        }
        let frozen = match store
            .frozen_selection_for_project(&project.project)
            .map_err(|error| format!("frozen selection lookup failed: {error}"))?
        {
            Some(frozen) => frozen,
            None => {
                let cursor = store
                    .selection_cursor(&project.project, project.task.name())
                    .map_err(|error| format!("selection cursor lookup failed: {error}"))?;
                let page = host.select_review_page(project, cursor.as_deref())?;
                if page.references.is_empty() {
                    // Nothing to freeze, but the walk moved: the continuation advances with the slot so the next slot examines new rows.
                    return self.complete_selection_slot(
                        store,
                        project,
                        claim,
                        command_id,
                        Some(page.next_cursor.as_deref()),
                        json!({"ok": true, "code": "curator_selection_empty", "next_cursor": page.next_cursor}),
                    );
                }
                match store.freeze_selection(
                    &project.project,
                    project.task.name(),
                    command_id,
                    &page,
                    now_ms,
                ) {
                    Ok(frozen) => frozen,
                    // A full frozen-page slot or an exhausted metadata quota defers the selection without freezing it; nothing moves until capacity or headroom returns.
                    Err(CuratorJobError::Refused(
                        refusal @ (CuratorJobRefusal::MetadataQuota
                        | CuratorJobRefusal::ProjectSelectionCapacity
                        | CuratorJobRefusal::HostSelectionCapacity),
                    )) => {
                        return Ok(SelectionStep::CapacityDeferred(refusal.to_string()));
                    }
                    Err(CuratorJobError::Refused(refusal)) => {
                        return Err(format!("freezing the selection was refused: {refusal}"));
                    }
                    Err(CuratorJobError::Store(error)) => {
                        return Err(format!("freezing the selection failed: {error}"));
                    }
                }
            }
        };
        if frozen.state != FrozenSelectionState::Frozen {
            return Err(format!(
                "the project's selection page is {:?}",
                frozen.state
            ));
        }
        self.enqueue_selection(store, project, claim, command_id, &frozen, &producer)
    }

    fn enqueue_selection(
        &self,
        store: &MemoryStore,
        project: &ScheduledProject,
        claim: &LeaseClaim,
        command_id: &str,
        frozen: &FrozenSelection,
        producer: &ProducerBinding,
    ) -> Result<SelectionStep, String> {
        match store.enqueue_frozen_selection(
            &project.project,
            &claim.claim_id,
            &format!("{command_id}:complete"),
            SCHEDULER_INSTANCE,
            project.task.ledger_slot(),
            frozen,
            producer,
            self.clock.now_ms(),
        ) {
            Ok(EnqueueOutcome::Enqueued {
                jobs,
                replayed,
                next_cursor,
            }) => Ok(SelectionStep::Ran(TaskRunOutcome::Ran {
                response: json!({"ok": true, "code": "curator_selection_enqueued", "jobs": jobs, "replayed": replayed, "next_cursor": next_cursor}),
            })),
            Ok(EnqueueOutcome::Expired) => Ok(SelectionStep::Ran(TaskRunOutcome::Ran {
                response: json!({"ok": false, "code": "curator_selection_expired"}),
            })),
            Ok(EnqueueOutcome::Replayed) => Ok(SelectionStep::Ran(TaskRunOutcome::Ran {
                response: json!({"ok": true, "code": "curator_selection_replayed"}),
            })),
            // Nothing moved: the page stays frozen in its slot and the claim stays live.
            Ok(EnqueueOutcome::Deferred(reason)) => {
                Ok(SelectionStep::CapacityDeferred(reason.to_string()))
            }
            Err(error) => Err(format!("enqueue failed: {error}")),
        }
    }

    /// Completes a selection slot that froze nothing. `advance` carries the continuation the slot moved to when the walk itself moved; `None` leaves the cursor where it was.
    fn complete_selection_slot(
        &self,
        store: &MemoryStore,
        project: &ScheduledProject,
        claim: &LeaseClaim,
        command_id: &str,
        advance: Option<Option<&str>>,
        response: Value,
    ) -> Result<SelectionStep, String> {
        let completion_id = format!("{command_id}:complete");
        let completed = match advance {
            Some(cursor) => store.complete_selection_slot(
                &project.project,
                &claim.claim_id,
                &completion_id,
                SCHEDULER_INSTANCE,
                project.task.ledger_slot(),
                project.task.name(),
                cursor,
                &response.to_string(),
                self.clock.now_ms(),
            ),
            None => store.complete_memory_classifier_task(
                &project.project,
                &claim.claim_id,
                &completion_id,
                SCHEDULER_INSTANCE,
                project.task.ledger_slot(),
                &response.to_string(),
                self.clock.now_ms(),
            ),
        };
        match completed {
            Ok(LeaseCompleteOutcome::Applied { .. } | LeaseCompleteOutcome::Replayed { .. }) => {
                Ok(SelectionStep::Ran(TaskRunOutcome::Ran { response }))
            }
            Ok(LeaseCompleteOutcome::Conflict { kind }) => {
                Err(format!("selection slot completion was {kind}"))
            }
            Err(error) => Err(format!("selection slot completion failed: {error}")),
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

    /// A wall clock stepping backwards, as an NTP correction would.
    pub(crate) fn step_back(&self, by: Duration) {
        self.now_ms
            .fetch_sub(by.as_millis() as i64, std::sync::atomic::Ordering::SeqCst);
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

    use memory_store::MEMORY_CLASSIFIER_TASK_LEASE_MS;

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
        /// Stepped back by this much during every run.
        run_steps_back: Option<(Arc<ManualClock>, Duration)>,
        /// The next `scheduled_projects` fails with this instead of answering.
        fail_projects_once: Mutex<Option<String>>,
        /// Every run parks on this until it is notified.
        block_runs: Option<Arc<tokio::sync::Notify>>,
        /// Selection pages served in order to `select_review_page`, with the cursor each call received recorded in `selection_cursors`; an exhausted script selects nothing.
        selections: Mutex<Vec<Result<FrozenSelectionPage, String>>>,
        selection_cursors: Mutex<Vec<Option<String>>>,
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
            if let Some((clock, by)) = &self.run_steps_back {
                clock.step_back(*by);
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

        fn select_review_page(
            &self,
            _project: &ScheduledProject,
            cursor: Option<&str>,
        ) -> Result<FrozenSelectionPage, String> {
            self.selection_cursors
                .lock()
                .unwrap()
                .push(cursor.map(str::to_string));
            let mut selections = self.selections.lock().unwrap();
            if selections.is_empty() {
                return Ok(FrozenSelectionPage {
                    references: Vec::new(),
                    next_cursor: None,
                });
            }
            selections.remove(0)
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
            task: ScheduledTask::ReviewUserMemories,
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
            run_steps_back: None,
            fail_projects_once: Mutex::new(None),
            block_runs: None,
            selections: Mutex::new(Vec::new()),
            selection_cursors: Mutex::new(Vec::new()),
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
                | TickEvent::Deferred { .. }
                | TickEvent::CapacityDeferred { .. } => None,
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
            .acquire_memory_classifier_task(
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
    ) -> memory_store::MemoryClassifierTaskAcquireOutcome {
        store
            .acquire_memory_classifier_task(
                identity,
                &command(due),
                SCHEDULER_INSTANCE,
                SCHEDULER_SLOT,
                i64::MAX,
                REVIEW_USER_MEMORIES_TASK_ID,
                due,
                due + 10 * MEMORY_CLASSIFIER_TASK_LEASE_MS,
            )
            .unwrap()
    }

    #[tokio::test]
    async fn a_task_runs_only_once_its_cron_instant_has_passed() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path());
        let host = scripted(&store, vec![project(&store, "git:a", "*/15 * * * *")]);
        let clock = ManualClock::at(T0 + 1_000);
        let mut scheduler = MemoryClassifierScheduler::new(clock.shared());

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
        let mut scheduler = MemoryClassifierScheduler::new(clock.shared());
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
        let mut scheduler = MemoryClassifierScheduler::new(clock.shared());
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
            scheduler.next_due[&("git:a".to_string(), ScheduledTask::ReviewUserMemories)].1,
            T0 + 45 * MINUTE_MS,
            "a's run crossed the 30-minute slot; its next instant counts from the post-run clock"
        );
        assert_eq!(
            scheduler.next_due[&("git:b".to_string(), ScheduledTask::ReviewUserMemories)].1,
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
        let mut scheduler = MemoryClassifierScheduler::new(clock.shared());
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
        let mut scheduler = MemoryClassifierScheduler::new(clock.shared());
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
        let mut scheduler = MemoryClassifierScheduler::new(clock.shared());
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
        let mut scheduler = MemoryClassifierScheduler::new(clock.shared());
        assert!(scheduler.tick(&host).await.is_empty());
        let due = T0 + 15 * MINUTE_MS;

        clock.advance(15 * MINUTE);
        store.fail_next_memory_classifier_task_acquire_for_test();
        let events = scheduler.tick(&host).await;
        assert!(
            matches!(
                &events[..],
                [TickEvent::Retained { project, due_at_ms, reason }]
                    if project == "git:a" && *due_at_ms == due
                        && reason.contains("injected memory_classifier task acquire failure")
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

    /// A completion the ledger could not record leaves the claim live under
    /// its acquisition id; the slot stays due, and the next tick re-leases
    /// that claim, replays the run's reply without a dispatch, and records
    /// the completion. A later slot is not consumed by the rebind.
    #[tokio::test]
    async fn a_failed_lease_completion_retains_the_slot_and_the_retry_records_it() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path());
        let host = scripted(&store, vec![project(&store, "git:a", "*/15 * * * *")]);
        let clock = ManualClock::at(T0 + 1_000);
        let mut scheduler = MemoryClassifierScheduler::new(clock.shared());
        assert!(scheduler.tick(&host).await.is_empty());
        let due = T0 + 15 * MINUTE_MS;

        clock.advance(15 * MINUTE);
        store.fail_next_memory_classifier_task_complete_for_test();
        let events = scheduler.tick(&host).await;
        assert!(
            matches!(
                &events[..],
                [TickEvent::Retained { due_at_ms, reason, .. }]
                    if *due_at_ms == due && reason.contains("lease completion failed")
            ),
            "{events:?}"
        );
        assert_eq!(host.runs.lock().unwrap().len(), 1, "the run happened");
        assert_eq!(scheduler.earliest_due(), Some(due), "the slot is still due");

        clock.advance(MINUTE);
        assert_eq!(ran(&scheduler.tick(&host).await), vec![("git:a", due)]);
        assert_eq!(
            host.runs.lock().unwrap().as_slice(),
            &[
                ("git:a".to_string(), command(due)),
                ("git:a".to_string(), command(due))
            ],
            "the retry asks the host for the same command, which replays its receipt"
        );
        assert!(matches!(
            slot_state(&store, "git:a", due),
            LeaseAcquireOutcome::Terminal { ref kind, .. } if kind == "applied"
        ));
        assert_eq!(scheduler.earliest_due(), Some(due + 15 * MINUTE_MS));
    }

    /// A wall clock that steps back during a run cannot rewind the schedule:
    /// the next instant is computed from no earlier than the slot that ran.
    #[tokio::test]
    async fn a_backward_clock_step_during_a_run_does_not_rewind_the_schedule() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path());
        let clock = ManualClock::at(T0 + 1_000);
        let mut host = scripted(&store, vec![project(&store, "git:a", "*/15 * * * *")]);
        host.run_steps_back = Some((Arc::clone(&clock), 60 * MINUTE));
        let mut scheduler = MemoryClassifierScheduler::new(clock.shared());
        assert!(scheduler.tick(&host).await.is_empty());

        clock.advance(15 * MINUTE);
        let due = T0 + 15 * MINUTE_MS;
        assert_eq!(ran(&scheduler.tick(&host).await), vec![("git:a", due)]);
        assert_eq!(clock.now_ms(), due - 60 * MINUTE_MS + 1_000);
        assert_eq!(
            scheduler.earliest_due(),
            Some(due + 15 * MINUTE_MS),
            "the next slot follows the one that ran, not the stepped-back clock"
        );
        // Nothing runs until the clock reaches that instant again.
        for _ in 0..4 {
            clock.advance(15 * MINUTE);
            assert!(scheduler.tick(&host).await.is_empty());
        }
        assert_eq!(host.runs.lock().unwrap().len(), 1);
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
        let mut successor = MemoryClassifierScheduler::new(clock.shared());
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
        let mut successor = MemoryClassifierScheduler::new(clock.shared());
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
        let mut retry = MemoryClassifierScheduler::new(ManualClock::at(later_due - 1).shared());
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
        holds_no_live_claim(store, identity, ScheduledTask::ReviewUserMemories)
    }

    fn holds_no_live_claim(store: &MemoryStore, identity: &str, task: ScheduledTask) -> bool {
        // A fresh acquisition for a far-future slot leases at once only when
        // nothing live holds the task; abandon it so the ledger is unchanged.
        let probe = T0 + 1_000 * MINUTE_MS;
        match store
            .acquire_memory_classifier_task(
                identity,
                &slot_command_id(task.name(), probe),
                "probe",
                task.ledger_slot(),
                1,
                task.lease_id(),
                probe,
                probe,
            )
            .unwrap()
        {
            LeaseAcquireOutcome::Claim { claim, .. } => {
                store
                    .abandon_memory_classifier_task(
                        identity,
                        &claim.claim_id,
                        "probe",
                        task.ledger_slot(),
                        probe,
                    )
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
            let clock =
                ManualClock::at(due + MEMORY_CLASSIFIER_TASK_LEASE_MS + offset - 15 * MINUTE_MS);
            let host = scripted(&store, vec![scheduled]);
            let mut successor = MemoryClassifierScheduler::new(clock.shared());
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
        let mut scheduler = MemoryClassifierScheduler::new(clock.shared());
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
                assert_eq!(
                    response["code"],
                    json!("memory_classifier_task_not_runnable")
                );
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
        let mut scheduler = MemoryClassifierScheduler::new(clock.shared());
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
        let scheduler = MemoryClassifierScheduler::new(clock.shared());
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
            let mut scheduler = MemoryClassifierScheduler::new(clock.shared());
            assert!(scheduler.tick(host.as_ref()).await.is_empty());
            clock.advance(15 * MINUTE);
            if fail_at_lease {
                store.fail_next_memory_classifier_task_acquire_for_test();
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

    /// A cleanup slot beside the review slot: both are leased under their own task identity through the same ledger and clock, the review task's events, command ids, and schedule are exactly what they are without the cleanup task, a backlog drains oldest first across both kinds, and a predecessor's live cleanup claim is rebound instead of dispatched twice.
    #[tokio::test]
    async fn a_cleanup_slot_shares_the_ledger_without_changing_the_review_task() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path());
        let review = project(&store, "git:a", "*/15 * * * *");
        let cleanup = ScheduledProject {
            task: ScheduledTask::MessageIndexCleanup,
            schedule: "*/30 * * * *".to_string(),
            ..review.clone()
        };
        let host = scripted(&store, vec![cleanup.clone(), review.clone()]);
        let clock = ManualClock::at(T0 + 1_000);
        let mut scheduler = MemoryClassifierScheduler::new(clock.shared());

        assert!(scheduler.tick(&host).await.is_empty());
        clock.advance(15 * MINUTE);
        let events = scheduler.tick(&host).await;
        assert_eq!(ran(&events), vec![("git:a", T0 + 15 * MINUTE_MS)]);
        assert_eq!(
            host.runs.lock().unwrap().as_slice(),
            &[("git:a".to_string(), command(T0 + 15 * MINUTE_MS))],
            "the cleanup schedule is not due yet, and the review run is unchanged"
        );
        clock.advance(15 * MINUTE);
        let events = scheduler.tick(&host).await;
        assert_eq!(
            ran(&events),
            vec![
                ("git:a", T0 + 30 * MINUTE_MS),
                ("git:a", T0 + 30 * MINUTE_MS)
            ]
        );
        let runs = host.runs.lock().unwrap().clone();
        assert_eq!(
            runs[1..]
                .iter()
                .map(|(_, command)| command.as_str())
                .collect::<std::collections::BTreeSet<_>>(),
            [
                slot_command_id(MESSAGE_INDEX_CLEANUP_TASK, T0 + 30 * MINUTE_MS),
                command(T0 + 30 * MINUTE_MS),
            ]
            .iter()
            .map(String::as_str)
            .collect(),
            "each task runs under its own command id for the shared instant"
        );
        // Both slots ended on the ledger under their own task identity.
        for task in [
            ScheduledTask::ReviewUserMemories,
            ScheduledTask::MessageIndexCleanup,
        ] {
            let state = store
                .acquire_memory_classifier_task(
                    "git:a",
                    &slot_command_id(task.name(), T0 + 30 * MINUTE_MS),
                    SCHEDULER_INSTANCE,
                    task.ledger_slot(),
                    scheduler.registration_generation.unwrap(),
                    task.lease_id(),
                    T0 + 30 * MINUTE_MS,
                    clock.now_ms(),
                )
                .unwrap();
            assert!(
                matches!(state, LeaseAcquireOutcome::Terminal { ref kind, .. } if kind == "applied"),
                "{task:?}: {state:?}"
            );
        }
        // The same instant does not run either task twice.
        assert!(scheduler.tick(&host).await.is_empty());

        // A scheduler died holding a cleanup slot: its successor, ranked above it by the ledger, rebinds that claim when the cleanup task next comes due and dispatches the interrupted slot once, under its command id; the review slot runs as always.
        let generation = scheduler.registration_generation.unwrap();
        let stranded_due = T0 + 45 * MINUTE_MS;
        let stranded = match store
            .acquire_memory_classifier_task(
                "git:a",
                &slot_command_id(MESSAGE_INDEX_CLEANUP_TASK, stranded_due),
                SCHEDULER_INSTANCE,
                ScheduledTask::MessageIndexCleanup.ledger_slot(),
                generation,
                MESSAGE_INDEX_CLEANUP_TASK_ID,
                stranded_due,
                T0 + 60 * MINUTE_MS - 1,
            )
            .unwrap()
        {
            LeaseAcquireOutcome::Claim { claim, .. } => claim,
            other => panic!("{other:?}"),
        };
        assert_eq!(stranded.source_revision, stranded_due);
        let clock = ManualClock::at(T0 + 30 * MINUTE_MS + 1);
        let host = scripted(&store, vec![cleanup, review]);
        let mut successor = MemoryClassifierScheduler::new(clock.shared());
        assert!(successor.tick(&host).await.is_empty());
        clock.advance(30 * MINUTE);
        let events = successor.tick(&host).await;
        assert_eq!(
            ran(&events),
            vec![("git:a", stranded_due), ("git:a", stranded_due)],
            "the review slot that came due at 45 minutes, then the cleanup slot rebound to the stranded claim"
        );
        assert_eq!(
            host.runs.lock().unwrap().as_slice(),
            &[
                ("git:a".to_string(), command(stranded_due)),
                (
                    "git:a".to_string(),
                    slot_command_id(MESSAGE_INDEX_CLEANUP_TASK, stranded_due)
                ),
            ]
        );
        for task in [
            ScheduledTask::ReviewUserMemories,
            ScheduledTask::MessageIndexCleanup,
        ] {
            assert!(holds_no_live_claim(&store, "git:a", task), "{task:?}");
        }
    }

    /// A selection page of `count` memory targets whose object ids start at `first`, continuing at `next`.
    fn selection_page(first: usize, count: usize, next: Option<&str>) -> FrozenSelectionPage {
        FrozenSelectionPage {
            references: (first..first + count)
                .map(|index| memory_store::curator_jobs::CausalInputs {
                    target: memory_store::curator_jobs::ReviewTarget::Memory {
                        object_id: format!("memory-{index}"),
                        source_revision: 1,
                    },
                    question_template: "extracted_facts".to_string(),
                    signals: Vec::new(),
                    required_evidence: vec![memory_store::curator_jobs::EvidenceAvailability {
                        evidence_id: format!("evidence-{index}"),
                        available: true,
                    }],
                    policy_versions: std::collections::BTreeMap::new(),
                })
                .collect(),
            next_cursor: next.map(str::to_string),
        }
    }

    fn selection_project(store: &MemoryStore, identity: &str) -> ScheduledProject {
        ScheduledProject {
            task: ScheduledTask::CuratorReviewSelection,
            ..project(store, identity, "*/15 * * * *")
        }
    }

    fn ready_jobs(store: &MemoryStore, identity: &str) -> usize {
        store.ready_curator_jobs(identity, 256).unwrap().len()
    }

    #[tokio::test]
    async fn a_selection_slot_freezes_enqueues_and_completes_in_one_pass_and_advances_its_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path());
        let host = scripted(&store, vec![selection_project(&store, "git:a")]);
        *host.selections.lock().unwrap() = vec![
            Ok(selection_page(0, 3, Some("cursor-3"))),
            // The same three targets again plus two new ones: the three replay their rows, two jobs are new.
            Ok(selection_page(0, 5, None)),
        ];
        let clock = ManualClock::at(T0 + 1_000);
        let mut scheduler = MemoryClassifierScheduler::new(clock.shared());
        assert!(scheduler.tick(&host).await.is_empty());
        clock.advance(15 * MINUTE);
        let events = scheduler.tick(&host).await;
        assert_eq!(ran(&events), vec![("git:a", T0 + 15 * MINUTE_MS)]);
        let TickEvent::Ran { outcome, .. } = &events[0] else {
            panic!("{events:?}")
        };
        assert_eq!(
            *outcome,
            TaskRunOutcome::Ran {
                response: json!({"ok": true, "code": "curator_selection_enqueued", "jobs": 3, "replayed": 0, "next_cursor": "cursor-3"})
            }
        );
        assert_eq!(
            ready_jobs(&store, "git:a"),
            3,
            "every reference is a ready job"
        );
        assert_eq!(store.frozen_selection_for_project("git:a").unwrap(), None);
        assert_eq!(
            store
                .selection_cursor("git:a", CURATOR_REVIEW_SELECTION_TASK)
                .unwrap()
                .as_deref(),
            Some("cursor-3")
        );
        assert!(host.runs.lock().unwrap().is_empty(), "classify never ran");
        // The slot is complete on the lease ledger; a replay of its acquisition reports the terminal.
        let terminal = store
            .acquire_memory_classifier_task(
                "git:a",
                &slot_command_id(CURATOR_REVIEW_SELECTION_TASK, T0 + 15 * MINUTE_MS),
                SCHEDULER_INSTANCE,
                ScheduledTask::CuratorReviewSelection.ledger_slot(),
                i64::MAX,
                CURATOR_REVIEW_SELECTION_TASK_ID,
                T0 + 15 * MINUTE_MS,
                T0 + 16 * MINUTE_MS,
            )
            .unwrap();
        assert!(
            matches!(terminal, LeaseAcquireOutcome::Terminal { .. }),
            "{terminal:?}"
        );
        // The next slot resumes from the advanced cursor; duplicates replay, new targets enqueue.
        clock.advance(15 * MINUTE);
        let events = scheduler.tick(&host).await;
        let TickEvent::Ran { outcome, .. } = &events[0] else {
            panic!("{events:?}")
        };
        assert_eq!(
            *outcome,
            TaskRunOutcome::Ran {
                response: json!({"ok": true, "code": "curator_selection_enqueued", "jobs": 2, "replayed": 3, "next_cursor": null})
            }
        );
        assert_eq!(ready_jobs(&store, "git:a"), 5);
        assert_eq!(
            *host.selection_cursors.lock().unwrap(),
            vec![None, Some("cursor-3".to_string())]
        );
        assert_eq!(
            store
                .selection_cursor("git:a", CURATOR_REVIEW_SELECTION_TASK)
                .unwrap(),
            None
        );
        // An empty selection completes its slot without freezing anything.
        clock.advance(15 * MINUTE);
        let events = scheduler.tick(&host).await;
        let TickEvent::Ran { outcome, .. } = &events[0] else {
            panic!("{events:?}")
        };
        assert_eq!(
            *outcome,
            TaskRunOutcome::Ran {
                response: json!({"ok": true, "code": "curator_selection_empty", "next_cursor": null})
            }
        );
        assert_eq!(ready_jobs(&store, "git:a"), 5);
    }

    #[tokio::test]
    async fn full_capacity_rolls_the_whole_page_back_and_retains_page_slot_and_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path());
        let host = scripted(&store, vec![selection_project(&store, "git:a")]);
        let clock = ManualClock::at(T0 + 1_000);
        let mut scheduler = MemoryClassifierScheduler::new(clock.shared());
        assert!(scheduler.tick(&host).await.is_empty());
        clock.advance(15 * MINUTE);
        // Fill the project to one job short of its pending capacity through another producer, then offer a page of three.
        let producer = ProducerBinding {
            producer: "filler".to_string(),
            firing_id: "f".to_string(),
            ordinal: 0,
        };
        let mut filled = 0;
        while filled < memory_store::curator_jobs::MAX_PENDING_CURATOR_JOBS_PER_PROJECT - 1 {
            let page = selection_page(1_000 + filled, 1, None);
            store
                .reserve_curator_job("git:a", &producer, &page.references[0], T0)
                .unwrap();
            filled += 1;
        }
        *host.selections.lock().unwrap() = vec![Ok(selection_page(0, 3, Some("cursor-3")))];
        let events = scheduler.tick(&host).await;
        assert!(
            matches!(&events[0], TickEvent::CapacityDeferred { .. }),
            "capacity deferral is its own event, not a store failure: {events:?}"
        );
        // Nothing of the page was written: not the two references that fit, not the cursor, and the page is still frozen in its slot.
        assert_eq!(ready_jobs(&store, "git:a"), 0);
        assert_eq!(
            store
                .selection_cursor("git:a", CURATOR_REVIEW_SELECTION_TASK)
                .unwrap(),
            None
        );
        let frozen = store
            .frozen_selection_for_project("git:a")
            .unwrap()
            .unwrap();
        assert_eq!(frozen.state, FrozenSelectionState::Frozen);
        assert_eq!(frozen.page.references.len(), 3);
        // The slot stays due and claimed: the next tick retries the same page without selecting again.
        clock.advance(MINUTE);
        let events = scheduler.tick(&host).await;
        assert!(
            matches!(&events[0], TickEvent::CapacityDeferred { .. }),
            "{events:?}"
        );
        assert_eq!(
            host.selection_cursors.lock().unwrap().len(),
            1,
            "no second selection"
        );
        // Capacity returns as filler jobs finish; the retry enqueues the retained page and completes the slot.
        for index in 0..3 {
            let inputs = &selection_page(1_000 + index, 1, None).references[0];
            store
                .finish_curator_job(
                    "git:a",
                    &inputs.causal_identity().unwrap(),
                    memory_store::curator_jobs::CuratorJobOutcome::Nonadmitted,
                    T0 + 1,
                )
                .unwrap();
        }
        clock.advance(MINUTE);
        let events = scheduler.tick(&host).await;
        assert!(matches!(&events[0], TickEvent::Ran { .. }), "{events:?}");
        assert_eq!(ready_jobs(&store, "git:a"), 3);
        assert_eq!(
            store
                .selection_cursor("git:a", CURATOR_REVIEW_SELECTION_TASK)
                .unwrap()
                .as_deref(),
            Some("cursor-3")
        );
    }

    #[tokio::test]
    async fn a_restart_resumes_the_frozen_page_and_an_expired_page_records_a_failed_slot() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path());
        let host = scripted(&store, vec![selection_project(&store, "git:a")]);
        // A scheduler froze a page and died before enqueueing it.
        let due = T0 + 15 * MINUTE_MS;
        let command = slot_command_id(CURATOR_REVIEW_SELECTION_TASK, due);
        store
            .freeze_selection(
                "git:a",
                CURATOR_REVIEW_SELECTION_TASK,
                &command,
                &selection_page(0, 2, Some("c")),
                due,
            )
            .unwrap();
        let clock = ManualClock::at(T0 + 1_000);
        let mut scheduler = MemoryClassifierScheduler::new(clock.shared());
        assert!(scheduler.tick(&host).await.is_empty());
        clock.advance(15 * MINUTE);
        let events = scheduler.tick(&host).await;
        assert!(matches!(&events[0], TickEvent::Ran { .. }), "{events:?}");
        assert!(
            host.selection_cursors.lock().unwrap().is_empty(),
            "the frozen page is enqueued without selecting again"
        );
        assert_eq!(ready_jobs(&store, "git:a"), 2);
        // A page left frozen past its 24-hour selection deadline is expired by the sweep; the next slot records a failed slot, enqueues nothing, and leaves the cursor untouched.
        let later = due + 15 * MINUTE_MS;
        let command = slot_command_id(CURATOR_REVIEW_SELECTION_TASK, later);
        store
            .freeze_selection(
                "git:a",
                CURATOR_REVIEW_SELECTION_TASK,
                &command,
                &selection_page(10, 2, Some("stale")),
                later,
            )
            .unwrap();
        clock.advance(Duration::from_millis(25 * 60 * MINUTE_MS as u64));
        store.expire_curator_work(clock.now_ms()).unwrap();
        // The slot whose page expired records that and enqueues nothing; the cursor stays at the last enqueued page.
        assert_eq!(store.frozen_selection_for_project("git:a").unwrap(), None);
        let events = scheduler.tick(&host).await;
        let TickEvent::Ran { outcome, .. } = &events[0] else {
            panic!("{events:?}")
        };
        assert_eq!(
            *outcome,
            TaskRunOutcome::Ran {
                response: json!({"ok": false, "code": "curator_selection_expired"})
            }
        );
        // The first page's jobs reached their own 24-hour deadline and expired with the sweep; the expired page's targets never had a job at all.
        assert_eq!(ready_jobs(&store, "git:a"), 0);
        for inputs in &selection_page(10, 2, None).references {
            assert_eq!(
                store
                    .lookup_curator_job("git:a", &inputs.causal_identity().unwrap())
                    .unwrap(),
                None,
                "the expired page's targets were never enqueued"
            );
        }
        assert_eq!(
            store
                .selection_cursor("git:a", CURATOR_REVIEW_SELECTION_TASK)
                .unwrap()
                .as_deref(),
            Some("c")
        );
        // The next slot selects afresh from that cursor.
        *host.selections.lock().unwrap() = vec![Ok(selection_page(20, 1, None))];
        clock.advance(15 * MINUTE);
        let events = scheduler.tick(&host).await;
        assert!(
            matches!(&events[0], TickEvent::Ran { outcome: TaskRunOutcome::Ran { response }, .. } if response["code"] == "curator_selection_enqueued"),
            "{events:?}"
        );
        assert_eq!(
            host.selection_cursors.lock().unwrap().as_slice(),
            [Some("c".to_string())],
            "continuation is the last enqueued page's cursor, never the expired page's"
        );
        assert_eq!(ready_jobs(&store, "git:a"), 1);
    }

    #[tokio::test]
    async fn an_empty_selection_advances_the_continuation_and_a_full_host_defers_without_freezing()
    {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path());
        let host = scripted(&store, vec![selection_project(&store, "git:a")]);
        // The first 256 descriptors all have jobs: the page is empty but the walk moved, so the next slot resumes past them rather than re-examining the same rows forever.
        *host.selections.lock().unwrap() = vec![
            Ok(FrozenSelectionPage {
                references: Vec::new(),
                next_cursor: Some("0\u{1f}object-256".to_string()),
            }),
            Ok(selection_page(0, 1, None)),
        ];
        let clock = ManualClock::at(T0 + 1_000);
        let mut scheduler = MemoryClassifierScheduler::new(clock.shared());
        assert!(scheduler.tick(&host).await.is_empty());
        clock.advance(15 * MINUTE);
        let events = scheduler.tick(&host).await;
        assert!(
            matches!(&events[0], TickEvent::Ran { outcome: TaskRunOutcome::Ran { response }, .. } if response["code"] == "curator_selection_empty"),
            "{events:?}"
        );
        assert_eq!(
            store
                .selection_cursor("git:a", CURATOR_REVIEW_SELECTION_TASK)
                .unwrap()
                .as_deref(),
            Some("0\u{1f}object-256")
        );
        clock.advance(15 * MINUTE);
        scheduler.tick(&host).await;
        assert_eq!(
            *host.selection_cursors.lock().unwrap(),
            vec![None, Some("0\u{1f}object-256".to_string())]
        );
        // Thirty-two frozen pages across other projects fill the host: this project's slot is deferred without freezing, selecting, or moving anything.
        for index in 0..memory_store::curator_jobs::MAX_FROZEN_SELECTIONS_PER_HOST {
            store
                .freeze_selection(
                    &format!("other:{index}"),
                    "slot",
                    "attempt",
                    &selection_page(index * 10, 1, None),
                    T0,
                )
                .unwrap();
        }
        *host.selections.lock().unwrap() = vec![Ok(selection_page(50, 1, None))];
        clock.advance(15 * MINUTE);
        let events = scheduler.tick(&host).await;
        assert!(
            matches!(&events[0], TickEvent::CapacityDeferred { reason, .. } if reason.contains("frozen selections")),
            "{events:?}"
        );
        assert_eq!(store.frozen_selection_for_project("git:a").unwrap(), None);
        assert_eq!(
            store
                .selection_cursor("git:a", CURATOR_REVIEW_SELECTION_TASK)
                .unwrap(),
            None,
            "the cursor stays where the last completed slot left it"
        );
    }

    #[tokio::test]
    async fn a_successor_resumes_a_predecessors_selection_slot_under_its_own_claim() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path());
        let scheduled = selection_project(&store, "git:a");
        let due = T0 + 15 * MINUTE_MS;
        let command = slot_command_id(CURATOR_REVIEW_SELECTION_TASK, due);
        // A predecessor leased the slot, froze its page, and died with the claim live and the page frozen.
        match store
            .acquire_memory_classifier_task(
                "git:a",
                &command,
                SCHEDULER_INSTANCE,
                ScheduledTask::CuratorReviewSelection.ledger_slot(),
                7,
                CURATOR_REVIEW_SELECTION_TASK_ID,
                due,
                due,
            )
            .unwrap()
        {
            LeaseAcquireOutcome::Claim { .. } => {}
            other => panic!("{other:?}"),
        }
        store
            .freeze_selection(
                "git:a",
                CURATOR_REVIEW_SELECTION_TASK,
                &command,
                &selection_page(0, 2, Some("c")),
                due,
            )
            .unwrap();
        // The successor's generation outranks the live claim; it rebinds the slot, finds the frozen page, and enqueues it without selecting.
        let clock = ManualClock::at(T0);
        let host = scripted(&store, vec![scheduled]);
        let mut successor = MemoryClassifierScheduler::new(clock.shared());
        assert!(successor.tick(&host).await.is_empty());
        clock.advance(15 * MINUTE);
        let events = successor.tick(&host).await;
        assert!(
            matches!(&events[0], TickEvent::Ran { due_at_ms, outcome: TaskRunOutcome::Ran { response }, .. } if *due_at_ms == due && response["code"] == "curator_selection_enqueued"),
            "{events:?}"
        );
        assert!(host.selection_cursors.lock().unwrap().is_empty());
        assert_eq!(ready_jobs(&store, "git:a"), 2);
        assert_eq!(
            store
                .selection_cursor("git:a", CURATOR_REVIEW_SELECTION_TASK)
                .unwrap()
                .as_deref(),
            Some("c")
        );
    }
}
