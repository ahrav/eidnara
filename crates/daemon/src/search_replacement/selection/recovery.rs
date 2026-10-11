use super::super::{BuildEvent, BuildFailure, ReplacementBuilder, ReplacementSpec, wall_ms};
use super::*;
use crate::projection_lifecycle::{
    ControlState, IntentRefusal, LifecycleRequest, RecoveryTarget, Transition,
};
use kernel::SourceHoldBinding;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryEvent {
    Build(BuildEvent),
    Selection(SelectionEvent),
    Retirement(retirement::RetirementEvent),
    BeforeEpisode,
    /// The selected family is about to catch up to `target`, the tip a still-registered predecessor trailed.
    BeforeCatchUp {
        target: i64,
    },
    /// The selected family applied `target`, the tip a still-registered predecessor trailed, and the recorded target moves forward to it.
    Recertified {
        target: i64,
    },
    BeforeFinalAcknowledgement,
    FinalAcknowledged,
    BeforeCurrent,
    Current,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryProgress {
    Selected,
    Current,
}

/// Whether disposing a construction releases its source hold. A construction that became the selected family keeps the hold, since the family's checkpoint names it and catch-up extends it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConstructionHold {
    Release,
    Retain,
}

/// The construction being disposed: one that completed into the selected family, or one abandoned under the record it left.
enum Disposal<'a> {
    Completed(&'a LifecycleIntent),
    Abandoned(&'a ControlState),
}

#[derive(Debug, thiserror::Error)]
pub enum RecoveryFailure<'a> {
    #[error("construction: {0:?}")]
    Build(Box<BuildFailure<'a>>),
    #[error("selection: {0:?}")]
    Selection(Box<SelectionFailure<'a>>),
    #[error(transparent)]
    Blocked(#[from] BuildError),
}

impl SearchSelection {
    #[cfg(feature = "test-support")]
    pub fn with_recovery_write_barrier_for_test(
        mut self,
        barrier: impl Fn(crate::projection_lifecycle::WriteBarrier) + Send + Sync + 'static,
    ) -> Self {
        self.recovery_barrier = Some(Arc::new(barrier));
        self
    }

    /// Installs `flag` on the recovery lifecycle handles; see `ProjectionLifecycle::with_directory_sync_failure_for_test`.
    #[cfg(feature = "test-support")]
    pub fn with_recovery_directory_sync_failure_for_test(
        mut self,
        flag: Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        self.recovery_sync_failure = Some(flag);
        self
    }

    #[cfg(feature = "test-support")]
    fn recovery_lifecycle(&self, lifecycle: ProjectionLifecycle) -> ProjectionLifecycle {
        let lifecycle = match self.recovery_barrier.clone() {
            Some(barrier) => lifecycle.with_write_barrier_for_test(move |event| barrier(event)),
            None => lifecycle,
        };
        match self.recovery_sync_failure.clone() {
            Some(flag) => lifecycle.with_directory_sync_failure_for_test(flag),
            None => lifecycle,
        }
    }

    /// Operator authorization names a new operation; it cannot amend the disabled operation's target or allowance.
    /// Recovery refuses live readers and maintenance before releasing construction state.
    pub fn begin_authorized_recovery(
        &mut self,
        kernel: &KernelStore,
        gate: &HookGate,
        request: &LifecycleRequest,
        budget: &EvalBudget,
    ) -> Result<(), BuildError> {
        deadline(budget)?;
        if request.transition != Transition::AuthorizedRecovery
            || request
                .authorization_ref
                .as_deref()
                .is_none_or(|r| !retrieval::dispatch::valid_authorization_ref(r))
        {
            return Err(IntentRefusal::MissingAuthorization.into());
        }
        if self.maintenance.is_some() {
            return Err(IntentRefusal::FamilyHeld.into());
        }
        if self.selected.load_full().is_some_and(|family| {
            Arc::strong_count(&family) != 2 || Arc::strong_count(&family.projection) != 1
        }) {
            return Err(IntentRefusal::FamilyHeld.into());
        }
        let transaction = LifecycleTransactionLock::acquire_exclusive(Some(&self.data_home))?;
        let lifecycle = ProjectionLifecycle::open(&self.data_home)?;
        #[cfg(feature = "test-support")]
        let lifecycle = self.recovery_lifecycle(lifecycle);
        let disabled = match lifecycle.read() {
            ControlState::Disabled(disabled) => {
                crate::projection_lifecycle::check_request(request, wall_ms()?)?;
                disabled
            }
            // The record already carries this authorization; only its sync and the gate remain.
            ControlState::Intent(existing)
                if existing.transition == Transition::AuthorizedRecovery
                    && existing.attempt_id == request.attempt_id =>
            {
                let prior = existing
                    .prior_disabled
                    .clone()
                    .ok_or(IntentRefusal::Disabled)?;
                lifecycle.authorize_recovery(
                    gate,
                    &prior,
                    request,
                    &InvalidationIdentity::from(&self.identity),
                    wall_ms()?,
                )?;
                self.selected.store(None);
                self.recovery_incarnation = None;
                return Ok(());
            }
            _ => return Err(IntentRefusal::Disabled.into()),
        };
        if disabled.through.is_some()
            && !disabled.deregistered
            && let Some(handoff) = disabled.handoff.as_deref()
            && kernel
                .outbox_consumer_checkpoint_within_budget(budget, &handoff.consumer.consumer_id)?
                .is_none()
        {
            return Err(BuildError::Invalid(
                "disable deregistration is unreconciled",
            ));
        }
        if kernel.database_incarnation_id_within_budget(budget)? != request.kernel_incarnation_id
            || request.kernel_incarnation_id != self.identity.kernel_incarnation_id
        {
            return Err(ProjectionError::IdentityMismatch.into());
        }
        gate.cleanup_limits(&InvalidationIdentity::from(&self.identity), &[])?;
        let store = GenerationStore::open(Some(&self.data_home))?;
        let selected = store.reconcile_search(&transaction)?;
        let handoff = disabled.handoff.as_deref();
        // A handoff that staged the selected digest serves the selection. A matching
        // `selected_generation` marks an unfinished follow-up; its certified predecessor serves.
        let live_consumer = match &selected {
            CurrentProfile::Current(digest) => {
                if *digest != request.selected_generation {
                    return Err(BuildError::Invalid("recovery names another selection"));
                }
                match handoff {
                    Some(h) if h.staged_seed_digest.as_ref() == Some(digest) => {
                        Some(h.consumer.consumer_id.clone())
                    }
                    Some(h) if h.selected_generation == *digest => Some(
                        self.predecessor(h)?
                            .ok_or(BuildError::Invalid("recovery names another selection"))?
                            .intent
                            .consumer
                            .consumer_id,
                    ),
                    _ => return Err(BuildError::Invalid("recovery names another selection")),
                }
            }
            CurrentProfile::Absent => None,
            _ => return Err(BuildError::Invalid("unknown selector references")),
        };
        if let Some(handoff) = handoff {
            if live_consumer.as_deref() == Some(request.consumer.consumer_id.as_str()) {
                return Err(BuildError::Invalid(
                    "replacement requires a distinct consumer",
                ));
            }
            if live_consumer.as_deref() != Some(handoff.consumer.consumer_id.as_str())
                && request.consumer.consumer_id != handoff.consumer.consumer_id
                && kernel
                    .outbox_consumer_checkpoint_within_budget(
                        budget,
                        &handoff.consumer.consumer_id,
                    )?
                    .is_some()
            {
                return Err(BuildError::Invalid(
                    "unfinished registration requires the same consumer",
                ));
            }
            self.dispose_construction(
                kernel,
                gate,
                &lifecycle,
                Disposal::Abandoned(&ControlState::Disabled(disabled.clone())),
                &transaction,
                budget,
            )?;
        }
        lifecycle.authorize_recovery(
            gate,
            &disabled,
            request,
            &InvalidationIdentity::from(&self.identity),
            wall_ms()?,
        )?;
        self.selected.store(None);
        self.recovery_incarnation = None;
        Ok(())
    }

    /// Selection and completion require separate calls so withholding the next slice cannot publish Current.
    /// Active work uses the recorded envelope; completed observations use the caller's budget without changing that envelope.
    /// Synchronous filesystem calls require healthy I/O; cancellation cannot preempt them.
    pub fn recover_slice<'a>(
        &mut self,
        kernel: &'a KernelStore,
        gate: &'a HookGate,
        spec: &ReplacementSpec,
        budget: &EvalBudget,
        observer: &mut dyn FnMut(RecoveryEvent),
    ) -> Result<RecoveryProgress, RecoveryFailure<'a>> {
        let lifecycle = ProjectionLifecycle::open(&self.data_home).map_err(BuildError::from)?;
        #[cfg(feature = "test-support")]
        let lifecycle = self.recovery_lifecycle(lifecycle);
        let (intent, completed) = match lifecycle.read() {
            ControlState::Intent(intent) => (intent, false),
            ControlState::Current(intent) => (intent, true),
            _ => return Err(BuildError::Intent(IntentRefusal::Disabled).into()),
        };
        self.check_recovery_kernel(kernel, budget, &intent.attempt_id)?;
        let selected = GenerationStore::open(Some(&self.data_home))
            .map_err(BuildError::from)?
            .read_search_current()
            .map_err(BuildError::from)?;
        let is_selected = intent
            .staged_seed_digest
            .as_ref()
            .is_some_and(|digest| selected == CurrentProfile::Current(digest.clone()));
        if completed {
            if !is_selected {
                return Err(BuildError::Invalid("completed selection missing").into());
            }
            let transaction = LifecycleTransactionLock::acquire_exclusive(Some(&self.data_home))
                .map_err(BuildError::from)?;
            if lifecycle.read() != ControlState::Current(intent.clone()) {
                return Err(BuildError::Invalid("completed operation changed").into());
            }
            spec.identity
                .require_compatible(&self.identity)
                .map_err(BuildError::from)?;
            self.reopen_locked(kernel, gate, budget, &transaction)?;
            let family = self
                .selected
                .load_full()
                .ok_or(BuildError::Invalid("missing selected owner"))?;
            if !family.names_operation(&intent)
                || lifecycle.read() != ControlState::Current(intent.clone())
            {
                return Err(BuildError::Invalid("selected operation differs").into());
            }
            self.admit(gate, budget)?;
            observer(RecoveryEvent::Current);
            return Ok(RecoveryProgress::Current);
        }
        self.check_recovery_admission(gate, spec, budget, &intent)?;
        if let Some(stage) = intent
            .replacement_capture
            .as_deref()
            .and_then(|c| c.stage.as_deref())
        {
            // A selected seed's target may have been recertified past its checkpoint, never past the kernel tip it was certified at.
            let recertified = |target: RecoveryTarget| -> Result<bool, BuildError> {
                if !is_selected || target.commit_seq <= stage.checkpoint_commit_seq {
                    return Ok(false);
                }
                let tip = kernel.capture_commit_read_target_within_budget(budget)?;
                Ok(self
                    .recovery_incarnation
                    .as_ref()
                    .is_some_and(|(attempt, incarnation)| {
                        *attempt == intent.attempt_id && *incarnation == tip.incarnation
                    })
                    && target.commit_seq <= tip.through_commit)
            };
            let target_bound = match intent.recovery_target {
                Some(target) if target.commit_seq == stage.checkpoint_commit_seq => true,
                Some(target) => recertified(target)?,
                None => false,
            };
            if intent
                .staged_seed_digest
                .as_deref()
                .is_some_and(|d| d != stage.stage_manifest().digest())
                || !target_bound
            {
                return Err(BuildError::Invalid("construction operation differs").into());
            }
        }
        if !is_selected {
            if let Some(target) = intent.recovery_target {
                let fresh = kernel
                    .capture_commit_read_target_within_budget(budget)
                    .map_err(BuildError::from)?;
                if fresh.through_commit > target.commit_seq {
                    return Err(BuildError::SnapshotBeyondTarget {
                        snapshot: fresh.through_commit,
                        target: target.commit_seq,
                    }
                    .into());
                }
            }
            if intent.staged_seed_digest.is_some() {
                let transaction =
                    LifecycleTransactionLock::acquire_exclusive(Some(&self.data_home))
                        .map_err(BuildError::from)?;
                self.dispose_construction(
                    kernel,
                    gate,
                    &lifecycle,
                    Disposal::Abandoned(&ControlState::Intent(intent.clone())),
                    &transaction,
                    budget,
                )?;
                lifecycle
                    .unpin_construction(gate, &intent)
                    .map_err(BuildError::from)?;
            }
            let candidate = ReplacementBuilder::open(&self.data_home, kernel, gate, spec.clone())?
                .build(budget, &mut |event| observer(RecoveryEvent::Build(event)))
                .map_err(RecoveryFailure::Build)?;
            self.select(candidate, &mut |event| {
                observer(RecoveryEvent::Selection(event));
                Ok(())
            })
            .map_err(RecoveryFailure::Selection)?;
            return Ok(RecoveryProgress::Selected);
        }
        let transaction = LifecycleTransactionLock::acquire_exclusive(Some(&self.data_home))
            .map_err(BuildError::from)?;
        if lifecycle.read() != ControlState::Intent(intent.clone()) {
            return Err(BuildError::Invalid("recovery operation changed").into());
        }
        let reopened = self.reopen_locked(kernel, gate, budget, &transaction)?;
        let family = self
            .selected
            .load_full()
            .ok_or(BuildError::Invalid("missing selected owner"))?;
        if !family.names_operation(&intent) {
            return Err(BuildError::Invalid("selected operation differs").into());
        }
        observer(RecoveryEvent::BeforeEpisode);
        let mut intent = intent;
        intent.episodes = lifecycle
            .consume_expected_episode(gate, &intent, wall_ms()?)
            .map_err(BuildError::from)?;
        let check_operation = |intent: &LifecycleIntent| {
            if lifecycle.read() != ControlState::Intent(intent.clone()) {
                return Err(BuildError::Invalid("recovery operation changed"));
            }
            self.check_recovery_admission(gate, spec, budget, intent)?;
            family.check_kernel(kernel, budget)
        };
        check_operation(&intent)?;
        // The reopen above validated this family; a second scan runs only when its database, the clock, or the consumer's acknowledgement moved since.
        let mut validated = match reopened {
            Some(validated)
                if self.unchanged_since(
                    &family,
                    kernel,
                    budget,
                    &validated.report,
                    validated.version,
                )? =>
            {
                validated
            }
            _ => self.validate_family(&family, kernel, budget)?,
        };
        // Recovery catches the selected family up before recertifying an advanced target, keeping the recorded target within the family's applied commits.
        if let Some(old) = &family.certificate.retiring
            && let Some(fixed) = intent.recovery_target
            && kernel
                .outbox_consumer_checkpoint_within_budget(budget, &old.consumer.consumer_id)
                .map_err(BuildError::from)?
                .is_some()
        {
            let tip = kernel
                .capture_commit_read_target_within_budget(budget)
                .map_err(BuildError::from)?
                .through_commit;
            if tip > fixed.commit_seq {
                if validated.report.checkpoint.checkpoint_commit_seq < tip {
                    observer(RecoveryEvent::BeforeCatchUp { target: tip });
                    self.catch_up_selected(&family, kernel, gate, spec, budget, tip)?;
                    check_operation(&intent)?;
                    validated = self.validate_family(&family, kernel, budget)?;
                }
                observer(RecoveryEvent::Recertified { target: tip });
                intent = lifecycle
                    .recertify_target(
                        gate,
                        &intent,
                        RecoveryTarget { commit_seq: tip },
                        wall_ms()?,
                    )
                    .map_err(BuildError::from)?;
            }
        }
        let check = || check_operation(&intent);
        check()?;
        let target = intent
            .recovery_target
            .ok_or(BuildError::Invalid("missing fixed target"))?
            .commit_seq;
        if validated.report.checkpoint.checkpoint_commit_seq != target {
            return Err(BuildError::Invalid("local prefix differs from target").into());
        }
        // The counters the check before Current expects: the validation's, advanced by the receipt transaction when retirement wrote one right after it.
        let mut expected = Some(validated.version);
        if family.certificate.retiring.is_some()
            && let Some(write) = self.retire_bound(
                &family,
                retirement::RetirementRun {
                    kernel,
                    through: target,
                    spec,
                    budget,
                    transaction: &transaction,
                    check: &check,
                },
                &mut |event| observer(RecoveryEvent::Retirement(event)),
            )?
        {
            expected = (write.before == validated.version).then_some(write.after);
        }
        observer(RecoveryEvent::BeforeFinalAcknowledgement);
        check()?;
        kernel
            .acknowledge_outbox_within_budget(
                budget,
                &intent.consumer.consumer_id,
                target,
                wall_ms()?,
                family.incarnation,
            )
            .map_err(BuildError::from)?;
        if kernel
            .outbox_consumer_checkpoint_within_budget(budget, &intent.consumer.consumer_id)
            .map_err(BuildError::from)?
            != Some(target)
        {
            return Err(BuildError::Invalid("final acknowledgement unresolved").into());
        }
        observer(RecoveryEvent::FinalAcknowledged);
        check()?;
        // The selected family's checkpoint names this hold, and catch-up extends it over later windows; it is released with the consumer at retirement or disable, at the next incarnation's reconciliation, or by its own expiry.
        self.dispose_construction(
            kernel,
            gate,
            &lifecycle,
            Disposal::Completed(&intent),
            &transaction,
            budget,
        )?;
        let cleaned = lifecycle
            .finish_construction(gate, &intent)
            .map_err(BuildError::from)?;
        observer(RecoveryEvent::BeforeCurrent);
        self.check_recovery_admission(gate, spec, budget, &cleaned)?;
        // The validation before retirement still holds when the receipt transaction is the family's only change since.
        let still_valid = match expected {
            Some(version) => {
                self.unchanged_since(&family, kernel, budget, &validated.report, version)?
            }
            None => false,
        };
        if !still_valid {
            self.validate_family(&family, kernel, budget)?;
        }
        lifecycle
            .complete(gate, &cleaned)
            .map_err(BuildError::from)?;
        observer(RecoveryEvent::Current);
        Ok(RecoveryProgress::Current)
    }

    fn check_recovery_kernel(
        &mut self,
        kernel: &KernelStore,
        budget: &EvalBudget,
        attempt_id: &str,
    ) -> Result<(), BuildError> {
        let fresh = kernel
            .capture_commit_read_target_within_budget(budget)?
            .incarnation;
        if self
            .recovery_incarnation
            .as_ref()
            .is_some_and(|(attempt, prior)| attempt == attempt_id && *prior != fresh)
            || kernel.database_incarnation_id_within_budget(budget)?
                != self.identity.kernel_incarnation_id
        {
            return Err(ProjectionError::IdentityMismatch.into());
        }
        self.recovery_incarnation = Some((attempt_id.to_owned(), fresh));
        Ok(())
    }

    /// Applies the commits after the selected family's checkpoint through `target` in one bounded episode under the family's own hold, acknowledging them for the consumer the family's certificate binds. The episode charges the catch-up hooks a Current family's catch-up charges.
    fn catch_up_selected(
        &self,
        family: &SelectedFamily,
        kernel: &KernelStore,
        gate: &HookGate,
        spec: &ReplacementSpec,
        budget: &EvalBudget,
        target: i64,
    ) -> Result<(), BuildError> {
        let bound = &family.certificate.intent.consumer;
        let checkpoint = family
            .projection
            .read_within(deadline(budget)?, |conn| {
                read_checkpoint(conn, &self.identity.kernel_incarnation_id)
            })?
            .ok_or(BuildError::Invalid("selected family has no checkpoint"))?;
        let grants = gate.admit_all(
            &[
                ProjectionHook::EmbeddingBootstrap,
                ProjectionHook::GitDurableRows,
            ],
            EntryPoint::Dispatch,
        )?;
        let expected = InvalidationIdentity::from(&self.identity);
        for grant in &grants {
            gate.check_limits(grant, &expected, &spec.catchup_page_charges())?;
        }
        let consumer = crate::search_catchup::CatchUpConsumer {
            binding: SourceHoldBinding {
                consumer_id: bound.consumer_id.clone(),
                lease_epoch: kernel.lease_epoch(),
                source_policy_version: self.identity.projection_policy_version.clone(),
            },
            hold_id: checkpoint.hold_id,
            kernel_incarnation_id: self.identity.kernel_incarnation_id.clone(),
            generation_id: Some(bound.generation_id.clone()),
        };
        let episode = budget.linked();
        let report = crate::search_catchup::SearchCatchUp::new(kernel, &family.projection)
            .with_budget(episode.clone())
            .run_episode_toward(
                &consumer,
                &spec.episode,
                kernel::CommitReadTarget {
                    through_commit: target,
                    incarnation: family.incarnation,
                },
                wall_ms()?,
                &mut |_| {
                    if grants.iter().any(|grant| grant.invalidated.is_cancelled()) {
                        episode.cancel();
                    }
                },
            )?;
        match report.end {
            crate::search_catchup::EpisodeEnd::ReachedTarget => Ok(()),
            crate::search_catchup::EpisodeEnd::Blocked(blocked) => {
                Err(BuildError::Blocked(blocked))
            }
        }
    }

    fn check_recovery_admission(
        &self,
        gate: &HookGate,
        spec: &ReplacementSpec,
        budget: &EvalBudget,
        intent: &LifecycleIntent,
    ) -> Result<(), BuildError> {
        self.admit_retirement(gate, spec, budget, intent)?;
        self.admit(gate, budget)?;
        Ok(())
    }

    fn dispose_construction(
        &self,
        kernel: &KernelStore,
        gate: &HookGate,
        lifecycle: &ProjectionLifecycle,
        disposal: Disposal<'_>,
        transaction: &LifecycleTransactionLock,
        budget: &EvalBudget,
    ) -> Result<(), BuildError> {
        let (expected, hold) = match disposal {
            Disposal::Completed(intent) => (
                ControlState::Intent(intent.clone()),
                ConstructionHold::Retain,
            ),
            Disposal::Abandoned(expected) => (expected.clone(), ConstructionHold::Release),
        };
        let expected = &expected;
        let intent = match expected {
            ControlState::Intent(intent) => intent,
            ControlState::Disabled(disabled) => {
                disabled.handoff.as_deref().ok_or(IntentRefusal::NoIntent)?
            }
            _ => return Err(IntentRefusal::NoIntent.into()),
        };
        let Some(capture) = intent.replacement_capture.as_deref() else {
            return Ok(());
        };
        deadline(budget)?;
        if kernel.database_incarnation_id_within_budget(budget)? != intent.kernel_incarnation_id {
            return Err(ProjectionError::IdentityMismatch.into());
        }
        let store = GenerationStore::open(Some(&self.data_home))?;
        let mut protected =
            ProjectionLifecycle::protected_generations(&self.data_home, transaction)?;
        lifecycle.delete_owned_replacement(
            gate,
            expected,
            &InvalidationIdentity::from(&self.identity),
            || {
                deadline(budget)?;
                if let Some(seed) = capture.stage.as_deref() {
                    let digest = seed.stage_manifest().digest();
                    if store.read_search_current()? != CurrentProfile::Current(digest.clone()) {
                        self.remove_family(
                            &digest,
                            &Bootstrap {
                                schema: 2,
                                seed: seed.clone(),
                                retiring: self.predecessor(intent)?.map(Bootstrap::into_retiring),
                                intent: intent.clone(),
                            },
                        )?;
                        protected.remove(&digest);
                        store.discard_unselected(
                            &seed.stage_manifest(),
                            transaction,
                            &protected,
                        )?;
                    }
                }
                Ok(())
            },
        )?;
        if hold == ConstructionHold::Release && capture.lease_epoch == kernel.lease_epoch() {
            kernel.release_source_hold_within_budget(
                budget,
                &SourceHoldBinding {
                    consumer_id: intent.consumer.consumer_id.clone(),
                    lease_epoch: capture.lease_epoch,
                    source_policy_version: capture.source_policy_version.clone(),
                },
                &capture.hold_id,
                wall_ms()?,
            )?;
        }
        kernel.reconcile_source_holds_within_budget(
            budget,
            &intent.consumer.consumer_id,
            wall_ms()?,
        )?;
        Ok(())
    }
}
