use super::*;
use crate::embedding_supervisor::{
    EmbeddingSupervisor, Maintained, SliceBounds, Stop, SupervisorEvent,
};
use crate::projection_lifecycle::{ControlState, DisabledIntent, EpisodeAccounting, IntentRefusal};
use kernel::CommitIntent;
use sha2::{Digest, Sha256};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;

pub(super) struct Maintenance {
    supervisor: Arc<EmbeddingSupervisor>,
    task: JoinHandle<()>,
}

impl Drop for Maintenance {
    fn drop(&mut self) {
        self.supervisor.request_shutdown();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisableEvent {
    AdmissionClosed,
    IntentPersisted,
    DrainStarted,
    WorkersJoined,
    Retirement(retirement::RetirementEvent),
    LocalReleased,
    Acknowledged,
    BeforeDeregister,
    Deregistered,
}

impl SearchSelection {
    /// Starts the selected family's supervisor and retains its reader through native completion.
    /// The supplied projection must be the selected connection, not a second connection to its path.
    pub fn start_maintenance(
        &mut self,
        maintained: Maintained,
        bounds: SliceBounds,
        now: Arc<dyn Fn() -> i64 + Send + Sync>,
        events: tokio::sync::mpsc::UnboundedSender<SupervisorEvent>,
    ) -> Result<(), BuildError> {
        if self.maintenance.is_some() {
            return Err(BuildError::Invalid("maintenance already owned"));
        }
        if matches!(
            ProjectionLifecycle::read_at(&self.data_home),
            ControlState::Disabled(_) | ControlState::Unavailable(_)
        ) {
            return Err(IntentRefusal::Disabled.into());
        }
        let family = self
            .selected
            .load_full()
            .ok_or(BuildError::Invalid("search unavailable"))?;
        if !Arc::ptr_eq(&family.projection, &maintained.projection) {
            return Err(BuildError::Invalid("maintenance names another projection"));
        }
        let grant = maintained
            .gate
            .admit(ProjectionHook::EmbeddingBackfill, EntryPoint::Startup)?;
        let supervisor = EmbeddingSupervisor::new(maintained, bounds, now, events);
        let task = supervisor.spawn_pinned(SearchReader { family, grant });
        self.maintenance = Some(Maintenance { supervisor, task });
        Ok(())
    }

    /// Closes admission before filesystem I/O. A failed persistence leaves this process closed.
    /// A crash before record replacement leaves the prior durable intent and an unknown disable request.
    /// Cleanup requires the durable record; restarting does not grant recovery authority.
    pub fn begin_disable(
        &self,
        gate: &HookGate,
        observer: &mut dyn FnMut(DisableEvent),
    ) -> Result<DisabledIntent, BuildError> {
        gate.disable();
        if let Some(family) = self.selected.load_full() {
            family.unavailable.store(true, Ordering::Release);
        }
        if let Some(owner) = &self.maintenance {
            owner.supervisor.request_shutdown();
        }
        observer(DisableEvent::AdmissionClosed);
        let lifecycle = ProjectionLifecycle::open(&self.data_home)?;
        #[cfg(feature = "test-support")]
        let lifecycle = match self.disable_barrier.clone() {
            Some(barrier) => lifecycle.with_write_barrier_for_test(move |event| barrier(event)),
            None => lifecycle,
        };
        let intent = lifecycle.disable(gate, super::super::wall_ms()?)?;
        observer(DisableEvent::IntentPersisted);
        Ok(intent)
    }

    /// Joins the owned supervisor before reconciling its released local prefix and ordinary deregistration.
    /// Cancellation or an expired wait retains the supervisor and join handle; its tracked task owns the reader.
    /// An empty selection or an already completed deregistration returns without consuming a cleanup episode.
    /// Filesystem calls require healthy dependencies; a deadline cannot interrupt a blocked syscall.
    pub async fn reconcile_disabled(
        &mut self,
        kernel: &KernelStore,
        gate: &HookGate,
        spec: &super::super::ReplacementSpec,
        budget: &EvalBudget,
        observer: &mut dyn FnMut(DisableEvent),
    ) -> Result<DisabledIntent, BuildError> {
        let lifecycle = ProjectionLifecycle::open(&self.data_home)?;
        let mut disabled = match lifecycle.read() {
            ControlState::Disabled(intent) => intent,
            _ => return Err(IntentRefusal::Disabled.into()),
        };
        gate.disable();
        if let Some(owner) = &self.maintenance {
            owner.supervisor.request_shutdown();
        }
        if self.maintenance.is_none() && self.selected.load().is_none() {
            if disabled.deregistered {
                return Ok(disabled);
            }
            if disabled.handoff.is_none() {
                let transaction =
                    LifecycleTransactionLock::acquire_exclusive(Some(&self.data_home))?;
                if GenerationStore::open(Some(&self.data_home))?.reconcile_search(&transaction)?
                    == CurrentProfile::Absent
                {
                    return Ok(disabled);
                }
            }
        }
        if disabled.handoff.is_none() {
            return Err(BuildError::Invalid(
                "disabled handoff missing for owned selection",
            ));
        }
        deadline(budget)?;
        if disabled.episodes.is_none() {
            let (allowance, duration) =
                gate.cleanup_envelope(&InvalidationIdentity::from(&self.identity))?;
            let mut next = disabled.clone();
            next.episodes = Some(EpisodeAccounting {
                allowance,
                consumed: 0,
                deadline: disabled
                    .recorded_at
                    .checked_add(duration)
                    .ok_or(BuildError::Expired)?,
            });
            lifecycle.update_disabled(&disabled, &next)?;
            disabled = next;
        }
        let end = self.cleanup_admission(gate, spec, budget, &disabled)?;
        if let Some(owner) = &mut self.maintenance {
            observer(DisableEvent::DrainStarted);
            let report = owner
                .supervisor
                .shutdown(end.saturating_duration_since(Instant::now()))
                .await?;
            if !matches!(report.stop, Some(Stop::Shutdown)) {
                return Err(BuildError::Invalid("maintenance requires reconciliation"));
            }
            let joined = tokio::time::timeout_at(end.into(), &mut owner.task)
                .await
                .map_err(|_| BuildError::Expired)?;
            self.maintenance = None;
            joined.map_err(|_| BuildError::Invalid("maintenance task failed"))?;
        }
        observer(DisableEvent::WorkersJoined);
        deadline(budget)?;
        if disabled.deregistered {
            return Ok(disabled);
        }
        let transaction = LifecycleTransactionLock::acquire_exclusive(Some(&self.data_home))?;
        let mut next = disabled.clone();
        let episodes = next.episodes.as_mut().expect("cleanup envelope");
        if episodes.consumed >= episodes.allowance {
            return Err(IntentRefusal::AllowanceExhausted.into());
        }
        episodes.consumed += 1;
        lifecycle.update_disabled(&disabled, &next)?;
        disabled = next;
        let intent = disabled.handoff.as_deref().expect("checked handoff");
        let digest = intent
            .staged_seed_digest
            .as_deref()
            .ok_or(BuildError::Invalid("selection is unfinished"))?;
        if GenerationStore::open(Some(&self.data_home))?.reconcile_search(&transaction)?
            != CurrentProfile::Current(digest.to_owned())
        {
            return Err(BuildError::Invalid(
                "disabled selection differs from handoff",
            ));
        }
        let check = || {
            self.cleanup_admission(gate, spec, budget, &disabled)?;
            if lifecycle.read() != ControlState::Disabled(disabled.clone()) {
                return Err(IntentRefusal::Disabled.into());
            }
            Ok(())
        };
        let consumer = intent.consumer.consumer_id.clone();
        if kernel.database_incarnation_id_within_budget(budget)? != intent.kernel_incarnation_id {
            return Err(ProjectionError::IdentityMismatch.into());
        }
        let checkpoint = kernel.outbox_consumer_checkpoint_within_budget(budget, &consumer)?;
        if checkpoint.is_some() {
            let family = match self.selected.load_full() {
                Some(family) => family,
                None => {
                    let family = Arc::new(self.open_family(digest, kernel, budget)?);
                    self.selected.store(Some(Arc::clone(&family)));
                    family
                }
            };
            family.unavailable.store(true, Ordering::Release);
            if family.certificate.intent != *intent {
                return Err(BuildError::Invalid(
                    "disabled handoff differs from selected certificate",
                ));
            }
            if family._seed_pin.digest != digest
                || Arc::strong_count(&family) != 2
                || Arc::strong_count(&family.projection) != 1
            {
                return Err(IntentRefusal::FamilyHeld.into());
            }
            self.validate_family(&family, kernel, budget)?;
            if family.certificate.retiring.is_some() {
                self.retire_bound(
                    &family,
                    retirement::RetirementRun {
                        kernel,
                        spec,
                        budget,
                        transaction: &transaction,
                        check: &check,
                    },
                    &mut |event| observer(DisableEvent::Retirement(event)),
                )?;
            }
            let through = family.projection.read_within(deadline(budget)?, |conn| {
                retrieval::batch::read_checkpoint(conn, &self.identity.kernel_incarnation_id)?
                    .map(|c| c.checkpoint_commit_seq)
                    .ok_or(ProjectionError::CorruptRow)
            })?;
            if disabled.through.is_some_and(|fixed| fixed != through) {
                return Err(BuildError::Invalid("disabled prefix changed"));
            }
            check()?;
            self.selected.store(None);
            drop(family);
            let mut next = disabled.clone();
            next.through = Some(through);
            lifecycle.update_disabled(&disabled, &next)?;
            disabled = next;
            observer(DisableEvent::LocalReleased);
            self.cleanup_admission(gate, spec, budget, &disabled)?;
            kernel.acknowledge_outbox_within_budget(
                budget,
                &consumer,
                through,
                super::super::wall_ms()?,
            )?;
            observer(DisableEvent::Acknowledged);
        }
        let through = disabled
            .through
            .ok_or(BuildError::Invalid("missing disabled prefix"))?;
        let intent = disabled.handoff.as_deref().expect("handoff");
        self.cleanup_admission(gate, spec, budget, &disabled)?;
        observer(DisableEvent::BeforeDeregister);
        self.cleanup_admission(gate, spec, budget, &disabled)?;
        kernel.commit_within_budget(
            budget,
            CommitIntent {
                producer: "search-disable".to_owned(),
                operation_key: intent.attempt_id.clone(),
                request_digest: format!(
                    "{:x}",
                    Sha256::digest(
                        serde_json::to_vec(&(
                            &intent.staged_seed_digest,
                            &intent.consumer,
                            through
                        ))
                        .map_err(|_| BuildError::Invalid("disable receipt encoding"))?
                    )
                ),
                actor: "daemon".to_owned(),
                cause: "disabled consumer deregistration".to_owned(),
            },
            |envelope| {
                envelope.deregister_outbox_consumer(
                    &intent.consumer.consumer_id,
                    super::super::wall_ms().map_err(|_| kernel::KernelError::InvalidInput)?,
                )?;
                Ok(String::new())
            },
        )?;
        observer(DisableEvent::Deregistered);
        let mut next = disabled.clone();
        next.deregistered = true;
        lifecycle.update_disabled(&disabled, &next)?;
        Ok(next)
    }

    #[cfg(feature = "test-support")]
    pub fn with_disable_write_barrier_for_test(
        mut self,
        barrier: impl Fn(crate::projection_lifecycle::WriteBarrier) + Send + Sync + 'static,
    ) -> Self {
        self.disable_barrier = Some(Arc::new(barrier));
        self
    }

    fn cleanup_admission(
        &self,
        gate: &HookGate,
        spec: &super::super::ReplacementSpec,
        budget: &EvalBudget,
        disabled: &DisabledIntent,
    ) -> Result<Instant, BuildError> {
        let intent = disabled.handoff.as_deref().ok_or(IntentRefusal::NoIntent)?;
        let episodes = disabled
            .episodes
            .ok_or(BuildError::Invalid("missing cleanup envelope"))?;
        spec.identity.require_compatible(&self.identity)?;
        if intent.kernel_incarnation_id != self.identity.kernel_incarnation_id {
            return Err(ProjectionError::IdentityMismatch.into());
        }
        let remaining = episodes
            .deadline
            .checked_sub(super::super::wall_ms()?)
            .and_then(|n| u64::try_from(n).ok())
            .ok_or(BuildError::Expired)?;
        let end = deadline(budget)?;
        if end > Instant::now() + Duration::from_millis(remaining) {
            return Err(BuildError::Invalid("cleanup exceeds original deadline"));
        }
        let work = spec.episode.max_source_pages.get() as u64;
        gate.cleanup_limits(
            &InvalidationIdentity::from(&self.identity),
            &[
                ("physical_drain_ms", remaining),
                (
                    "B_recovery_ms",
                    episodes
                        .deadline
                        .checked_sub(disabled.recorded_at)
                        .and_then(|n| u64::try_from(n).ok())
                        .ok_or(BuildError::Expired)?,
                ),
                ("retry_attempts", u64::from(episodes.allowance)),
                (
                    "catchup_batch_commits",
                    work.checked_mul(spec.episode.commits.max_commits.get() as u64)
                        .ok_or(BuildError::InventoryBound)?,
                ),
                (
                    "catchup_batch_encoded_bytes",
                    work.checked_mul(spec.episode.commits.max_payload_bytes.get())
                        .ok_or(BuildError::InventoryBound)?,
                ),
                (
                    "local_transaction_rows",
                    (spec.episode.batch.persist.max_records.get() as u64)
                        .saturating_add(1)
                        .max(self.bounds.max_live().saturating_add(
                            self.bounds.max_tombstoned_per_class.get().saturating_mul(5),
                        ) as u64)
                        .max(spec.episode.commits.max_rows.get() as u64),
                ),
                (
                    "export_page_rows",
                    self.bounds.max_live_per_class.get() as u64,
                ),
                (
                    "local_transaction_bytes",
                    spec.episode
                        .max_source_encoded_bytes
                        .get()
                        .checked_add(MAX_RECORD_BYTES)
                        .ok_or(BuildError::InventoryBound)?,
                ),
            ],
        )?;
        Ok(end)
    }
}
