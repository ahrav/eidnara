use super::*;
use crate::embedding_supervisor::{
    DrainReport, EmbeddingSupervisor, Maintained, SliceBounds, SupervisorEvent, Unresolved,
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
    scope: ProjectScope,
}

impl Maintenance {
    /// A finished task has dropped its reader, so this owner pins nothing.
    pub(super) fn pins(&self) -> bool {
        !self.task.is_finished()
    }
}

/// A live supervisor and the project scope it maintains. The supervisor can be joined through the handle after the selection that started it has been taken elsewhere.
#[derive(Clone)]
pub struct MaintenanceHandle {
    supervisor: Arc<EmbeddingSupervisor>,
    // A scope is about a kilobyte; the handle travels inside slice outcomes, which stay small.
    pub scope: Box<ProjectScope>,
}

impl std::fmt::Debug for MaintenanceHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MaintenanceHandle")
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

impl MaintenanceHandle {
    /// Stops the supervisor and waits up to `grace` for its slice and native work to exit.
    ///
    /// # Errors
    ///
    /// Returns the drain the supervisor could not resolve within `grace`; its work stays owned by its task.
    pub async fn stop(&self, grace: Duration) -> Result<DrainReport, Unresolved> {
        self.supervisor.shutdown(grace).await
    }
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
    /// Starts the selected family's supervisor and retains its reader through native completion; the kernel reads that validate the start end within `budget`.
    /// The supplied projection must be the selected connection, not a second connection to its path.
    pub fn start_maintenance(
        &mut self,
        maintained: Maintained,
        bounds: SliceBounds,
        now: Arc<dyn Fn() -> i64 + Send + Sync>,
        events: tokio::sync::mpsc::UnboundedSender<SupervisorEvent>,
        budget: &EvalBudget,
    ) -> Result<(), BuildError> {
        if self.maintenance.as_ref().is_some_and(|owner| !owner.pins()) {
            self.maintenance = None;
        }
        if self.maintenance.is_some() {
            return Err(BuildError::Invalid("maintenance already owned"));
        }
        let family = self
            .selected
            .load_full()
            .ok_or(BuildError::Invalid("search unavailable"))?;
        if !Arc::ptr_eq(&family.projection, &maintained.projection) {
            return Err(BuildError::Invalid("maintenance names another projection"));
        }
        maintained
            .gate
            .require_binding(&self.data_home, &self.identity)?;
        let grant = maintained
            .gate
            .admit(ProjectionHook::EmbeddingBackfill, EntryPoint::Startup)?;
        let slice_ms = u64::try_from(bounds.slice.as_millis())
            .ok()
            .and_then(|whole| {
                whole.checked_add(u64::from(
                    !bounds.slice.subsec_nanos().is_multiple_of(1_000_000),
                ))
            })
            .ok_or(BuildError::Invalid("maintenance slice"))?;
        maintained.gate.check_limits(
            &grant,
            &InvalidationIdentity::from(&self.identity),
            &[
                (
                    "embedding_recovery_attempts",
                    u64::from(bounds.dispatch.grant.allowance.get()),
                ),
                ("pending_count", bounds.dispatch.max_jobs.get() as u64),
                ("supervisor_slice_ms", slice_ms),
                (
                    "local_transaction_rows",
                    bounds.sweep_candidates.get() as u64,
                ),
            ],
        )?;
        family.check_kernel(&maintained.kernel, budget)?;
        let scope = maintained.project.clone();
        let supervisor = EmbeddingSupervisor::new(maintained, bounds, now, events);
        let task = supervisor.spawn_pinned(SearchReader { family, grant });
        self.maintenance = Some(Maintenance {
            supervisor,
            task,
            scope,
        });
        Ok(())
    }

    /// The supervisor still pinning the selected family, if any; a finished one is reaped.
    pub fn maintenance(&mut self) -> Option<MaintenanceHandle> {
        if self.maintenance.as_ref().is_some_and(|owner| !owner.pins()) {
            self.maintenance = None;
        }
        self.maintenance.as_ref().map(|owner| MaintenanceHandle {
            supervisor: Arc::clone(&owner.supervisor),
            scope: Box::new(owner.scope.clone()),
        })
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
    /// A supervisor that already stopped on its own joins like one this disable cancelled.
    /// Cancellation or an expired wait retains the supervisor and join handle; its tracked task owns the reader.
    /// An empty selection, a recorded deregistration, or one the kernel already holds completes without consuming a cleanup episode; a recorded deregistration still spends one budgeted kernel read to confirm the consumer is absent.
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
        #[cfg(feature = "test-support")]
        let lifecycle = match self.disable_barrier.clone() {
            Some(barrier) => lifecycle.with_write_barrier_for_test(move |event| barrier(event)),
            None => lifecycle,
        };
        let mut disabled = match lifecycle.read() {
            ControlState::Disabled(intent) => intent,
            _ => return Err(IntentRefusal::Disabled.into()),
        };
        gate.disable();
        if let Some(owner) = &self.maintenance {
            owner.supervisor.request_shutdown();
        }
        // A validated record with `deregistered` names its consumer; a kernel that holds that consumer again was restored from before the disable, and the marker does not outrank it. The read waits for a reader only within the caller's budget.
        let registered_again = |disabled: &DisabledIntent| -> Result<bool, BuildError> {
            let Some(handoff) = disabled.handoff.as_deref() else {
                return Ok(false);
            };
            Ok(kernel
                .outbox_consumer_checkpoint_within_budget(budget, &handoff.consumer.consumer_id)?
                .is_some())
        };
        if self.maintenance.is_none() && self.selected.load().is_none() {
            if disabled.deregistered {
                if registered_again(&disabled)? {
                    return Err(BuildError::Invalid(
                        "deregistered consumer is registered again",
                    ));
                }
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
        gate.require_selection_home(&self.data_home)?;
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
            // A resolved drain has joined every slice and owned call regardless of what stopped the loop; refusing an earlier stop here would block cleanup until restart.
            owner
                .supervisor
                .shutdown(end.saturating_duration_since(Instant::now()))
                .await?;
            let joined = tokio::time::timeout_at(end.into(), &mut owner.task)
                .await
                .map_err(|_| BuildError::Expired)?;
            self.maintenance = None;
            joined.map_err(|_| BuildError::Invalid("maintenance task failed"))?;
        }
        observer(DisableEvent::WorkersJoined);
        deadline(budget)?;
        if disabled.deregistered {
            if registered_again(&disabled)? {
                return Err(BuildError::Invalid(
                    "deregistered consumer is registered again",
                ));
            }
            return Ok(disabled);
        }
        let transaction = LifecycleTransactionLock::acquire_exclusive(Some(&self.data_home))?;
        let handoff = disabled.handoff.clone().expect("checked handoff");
        let digest = handoff
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
        let consumer = handoff.consumer.consumer_id.clone();
        if kernel.database_incarnation_id_within_budget(budget)? != handoff.kernel_incarnation_id {
            return Err(ProjectionError::IdentityMismatch.into());
        }
        // The history this attempt observed the consumer in; a validated family narrows it to the one its prefix was read from.
        let mut history = kernel
            .capture_commit_read_target_within_budget(budget)?
            .incarnation;
        let checkpoint = kernel.outbox_consumer_checkpoint_within_budget(budget, &consumer)?;
        // A restore keeps the durable identity but can renumber history; a prefix read under one incarnation is acknowledged and deregistered only under the same one.
        let settled = |disabled: &DisabledIntent, history: CommitReadIncarnation| {
            self.cleanup_admission(gate, spec, budget, disabled)?;
            if lifecycle.read() != ControlState::Disabled(disabled.clone()) {
                return Err(IntentRefusal::Disabled.into());
            }
            if kernel
                .capture_commit_read_target_within_budget(budget)?
                .incarnation
                != history
            {
                return Err(ProjectionError::IdentityMismatch.into());
            }
            Ok::<(), BuildError>(())
        };
        if checkpoint.is_some() {
            // Only outstanding local work consumes an episode; a consumer the kernel already deregistered leaves nothing to retry but the receipt replay.
            let mut next = disabled.clone();
            let episodes = next.episodes.as_mut().expect("cleanup envelope");
            if episodes.consumed >= episodes.allowance {
                return Err(IntentRefusal::AllowanceExhausted.into());
            }
            episodes.consumed += 1;
            lifecycle.update_disabled(&disabled, &next)?;
            disabled = next;
            let intent = &*handoff;
            let family = match self.selected.load_full() {
                Some(family) => family,
                None => {
                    let family = Arc::new(self.open_family(digest, kernel, budget)?);
                    self.selected.store(Some(Arc::clone(&family)));
                    family
                }
            };
            family.unavailable.store(true, Ordering::Release);
            if !family.names_operation(intent) {
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
            let incarnation = family.incarnation;
            history = incarnation;
            if family.certificate.retiring.is_some() {
                self.retire_bound(
                    &family,
                    retirement::RetirementRun {
                        kernel,
                        spec,
                        budget,
                        transaction: &transaction,
                        check: &|| settled(&disabled, history),
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
            settled(&disabled, history)?;
            self.selected.store(None);
            // Taking the last reference is atomic; a count can be satisfied and then raced by a `Weak` upgrade.
            let SelectedFamily { projection, .. } =
                Arc::into_inner(family).ok_or(IntentRefusal::FamilyHeld)?;
            drop(Arc::into_inner(projection).ok_or(IntentRefusal::FamilyHeld)?);
            let mut next = disabled.clone();
            next.through = Some(through);
            lifecycle.update_disabled(&disabled, &next)?;
            disabled = next;
            observer(DisableEvent::LocalReleased);
            settled(&disabled, history)?;
            // The writer re-checks the incarnation, so a restore between `settled` and this write is refused rather than acknowledged.
            kernel.acknowledge_outbox_within_budget(
                budget,
                &consumer,
                through,
                super::super::wall_ms()?,
                incarnation,
            )?;
            observer(DisableEvent::Acknowledged);
        }
        let through = disabled
            .through
            .ok_or(BuildError::Invalid("missing disabled prefix"))?;
        let intent = disabled.handoff.as_deref().expect("handoff");
        settled(&disabled, history)?;
        observer(DisableEvent::BeforeDeregister);
        settled(&disabled, history)?;
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
                kernel.require_incarnation(history)?;
                envelope.deregister_outbox_consumer(
                    &intent.consumer.consumer_id,
                    super::super::wall_ms().map_err(|_| kernel::KernelError::InvalidInput)?,
                )?;
                Ok(String::new())
            },
        )?;
        observer(DisableEvent::Deregistered);
        if registered_again(&disabled)? {
            return Err(BuildError::Invalid(
                "deregistered consumer is registered again",
            ));
        }
        let mut next = disabled.clone();
        next.deregistered = true;
        lifecycle.update_disabled(&disabled, &next)?;
        if registered_again(&next)? {
            return Err(BuildError::Invalid(
                "deregistered consumer is registered again",
            ));
        }
        Ok(next)
    }

    /// The supervisor this selection owns, so a test can drive it to a stop the selection did not request.
    #[cfg(feature = "test-support")]
    pub fn maintenance_supervisor_for_test(&self) -> Option<Arc<EmbeddingSupervisor>> {
        self.maintenance
            .as_ref()
            .map(|owner| Arc::clone(&owner.supervisor))
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
        let (retirement_rows, retirement_bytes) = retirement::retirement_transaction_charges(spec)?;
        let mut requested = vec![
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
                "local_transaction_rows",
                (spec.episode.batch.persist.max_records.get() as u64)
                    .saturating_add(1)
                    .max(self.bounds.max_rows() as u64)
                    .max(spec.episode.commits.max_rows.get() as u64)
                    .max(retirement_rows),
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
                    .ok_or(BuildError::InventoryBound)?
                    .max(retirement_bytes),
            ),
        ];
        requested.extend(spec.catchup_page_charges());
        gate.cleanup_limits(&InvalidationIdentity::from(&self.identity), &requested)?;
        Ok(end)
    }
}
