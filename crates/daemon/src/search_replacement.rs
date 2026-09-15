use std::collections::{BTreeMap, BTreeSet};
use std::num::{NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use host_runtime::generation::{CurrentProfile, GenerationStore};
use host_runtime::lifecycle::LifecycleTransactionLock;
use kernel::applicability::EvalBudget;
use kernel::{
    CommitIntent, CommitReadError, ExportWindow, KernelStore, PageBound, SourceExportError,
    SourceHoldBinding, SourceHoldBounds, SourceHoldError, SourceHoldInvalidity, SourceRow,
};
use retrieval::ProjectionIdentity;
use retrieval::batch::{
    MutationIdentity, VectorGeneration, batch_from_rows, register_generation, row_identities,
};
use retrieval::coverage::{CoverageBounds, verify_construction};
use sha2::{Digest, Sha256};

use crate::projection_gates::{
    Admission, Denial, EntryPoint, HookGate, InvalidationIdentity, ProjectionHook,
};
use crate::projection_lifecycle::{
    IntentRefusal, LifecycleIntent, ProjectionLifecycle, RecoveryTarget, ReplacementCapture,
};
use crate::search_catchup::{
    Blocked, CatchUpConsumer, EpisodeBounds, EpisodeEnd, EpisodeEvent, SearchCatchUp,
};
use crate::search_projection::SearchProjection;
use crate::search_seed::{self, ClosedSeed, SeedBounds, StagedSeed};

pub mod selection;

#[derive(Debug, Clone)]
pub struct ReplacementSpec {
    pub identity: ProjectionIdentity,
    pub generation: VectorGeneration,
    pub capture: SourceHoldBounds,
    pub episode: EpisodeBounds,
    pub seed: SeedBounds,
    pub retirement: RetirementBounds,
}

/// `object_registry` is append-only, so the retirement census grows with corpus history.
/// Size these bounds for the whole corpus, not for one batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetirementBounds {
    pub max_obligations: NonZeroUsize,
    /// Encoded kind, identity, and digest bytes; payloads are never read.
    pub max_obligation_bytes: NonZeroU64,
}

impl ReplacementSpec {
    /// The bytes one pending job row charges: two identifiers, the state literal, four integers, and the generation id.
    pub(crate) fn pending_row_width(generation_id: &str) -> u64 {
        (2 * 64 + "pending".len() + 4 * std::mem::size_of::<i64>() + generation_id.len()) as u64
    }

    /// The manifest charges for one commit page of the episode's catch-up read. Every path that pages through commits under `episode.commits` charges these, so one page costs the same at construction and at cleanup.
    pub(crate) fn catchup_page_charges(&self) -> [(&'static str, u64); 3] {
        let episode = &self.episode;
        [
            (
                "catchup_batch_commits",
                episode.commits.max_commits.get() as u64,
            ),
            (
                "catchup_batch_encoded_bytes",
                episode.commits.max_payload_bytes.get(),
            ),
            (
                "catchup_batch_encoded_bytes",
                episode.max_source_encoded_bytes.get(),
            ),
        ]
    }

    fn admit(
        &self,
        gate: &HookGate,
        intent: &LifecycleIntent,
    ) -> Result<Vec<Admission>, BuildError> {
        if self.generation.embedding_model != self.identity.embedding_model
            || self.generation.tokenizer_fingerprint != self.identity.tokenizer_fingerprint
            || self.generation.vector_dimension != self.identity.vector_dimension
            || self.generation.generation_epoch != self.identity.generation_epoch
        {
            return Err(retrieval::ProjectionError::IdentityMismatch.into());
        }
        let hook = intent.transition.hook();
        let hooks = if hook == ProjectionHook::EmbeddingBootstrap {
            vec![hook]
        } else {
            vec![hook, ProjectionHook::EmbeddingBootstrap]
        };
        let grants = gate.admit_all(&hooks, EntryPoint::Reload)?;
        let episode = &self.episode;
        let page = &episode.source_page;
        let batch = episode.batch;
        let capture = self.capture.admission;
        let references = capture
            .max_references
            .get()
            .max(episode.hold_admission.max_references.get()) as u64;
        let source_disk = capture
            .max_encoded_bytes
            .get()
            .max(episode.hold_admission.max_encoded_bytes.get());
        let checkpoints = self.seed.checkpoint_attempts.get();
        let retries = u64::from(intent.episodes.allowance) * u64::from(checkpoints);
        let wait = self
            .seed
            .attempt_wait
            .checked_mul(checkpoints)
            .and_then(|wait| wait.checked_mul(intent.episodes.allowance))
            .and_then(|wait| {
                // `quiesce` arms the full `Duration` each attempt, so round the total up, never each wait down.
                let whole = u64::try_from(wait.as_millis()).ok()?;
                whole.checked_add(u64::from(wait.subsec_nanos() % 1_000_000 != 0))
            })
            .ok_or(BuildError::Invalid("checkpoint wait"))?;
        let rows = batch
            .max_local_mutations
            .get()
            .max(batch.persist.max_records.get())
            .max(self.capture.max_descriptor_rows.get())
            .max(episode.commits.max_rows.get()) as u64;
        let peak_rows = (batch.persist.max_records.get() as u64)
            .checked_add(page.max_rows.get() as u64)
            .ok_or(BuildError::Invalid("row charge overflow"))?;
        let pending_bytes = Self::pending_row_width(&self.generation.generation_id)
            .checked_mul(batch.max_pending.get() as u64)
            .ok_or(BuildError::Invalid("pending charge overflow"))?;
        let local_bytes = (batch.persist.max_tuple_bytes.get() as u64)
            .checked_mul(batch.persist.max_records.get() as u64)
            .and_then(|bytes| bytes.checked_add(batch.max_source_bytes.get() as u64))
            .and_then(|bytes| bytes.checked_add(pending_bytes))
            .ok_or(BuildError::Invalid("local charge overflow"))?;
        let decoded = (batch.max_source_bytes.get() as u64)
            .checked_add(page.max_decoded_bytes.get())
            .ok_or(BuildError::Invalid("decoded charge overflow"))?;
        let disk = self
            .seed
            .max_bytes
            .checked_add(crate::projection_lifecycle::MAX_RECORD_BYTES)
            .and_then(|bytes| bytes.checked_mul(u64::from(intent.episodes.allowance) + 2))
            .and_then(|bytes| bytes.checked_add(source_disk))
            .ok_or(BuildError::Invalid("disk charge overflow"))?;
        let duration = intent
            .episodes
            .deadline
            .checked_sub(intent.recorded_at)
            .and_then(|n| u64::try_from(n).ok())
            .ok_or(BuildError::Invalid("episode duration"))?;
        let duration_limit = intent.transition.duration_limit();
        let mut requested = vec![
            ("export_page_rows", page.max_rows.get() as u64),
            ("export_page_encoded_bytes", page.max_encoded_bytes.get()),
            (
                "export_row_standalone_encoded_bytes",
                page.max_encoded_bytes.get(),
            ),
            ("export_live_decoded_bytes", decoded),
            (
                "catchup_batch_source_bytes",
                batch.max_source_bytes.get() as u64,
            ),
            ("local_transaction_rows", rows.max(peak_rows)),
            ("local_transaction_bytes", local_bytes),
            ("pending_count", batch.max_pending.get() as u64),
            ("pending_bytes", pending_bytes),
            ("capture_reference_count", references),
            ("capture_hold_expiry_ms", self.capture.expiry_ms.get()),
            ("capture_disk_bytes", disk),
            ("retry_attempts", retries),
            (duration_limit, duration),
            (duration_limit, wait),
        ];
        requested.extend(self.catchup_page_charges());
        let expected = InvalidationIdentity::from(&self.identity);
        for grant in &grants {
            gate.check_limits(grant, &expected, &requested)?;
        }
        Ok(grants)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildEvent {
    CaptureHeld { snapshot: i64, hold_id: String },
    Captured { snapshot: i64, hold_id: String },
    Exported { window: ExportWindow, rows: usize },
    BaselineStaged,
    BaselineReleased,
    TargetFixed(i64),
    CatchUp(EpisodeEvent),
    Verifying,
    Closed,
    StagePrepared,
    Staged,
    Pinned,
    Aborting,
}

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error(transparent)]
    UnresolvedDrain(#[from] crate::embedding_supervisor::Unresolved),
    #[error(transparent)]
    Kernel(#[from] kernel::KernelError),
    #[error(transparent)]
    Hold(#[from] kernel::SourceHoldError),
    #[error(transparent)]
    Export(#[from] SourceExportError),
    #[error(transparent)]
    Projection(#[from] crate::search_projection::SearchProjectionError),
    #[error(transparent)]
    Mutation(#[from] retrieval::ProjectionError),
    #[error(transparent)]
    Seed(#[from] search_seed::SeedRefusal),
    #[error(transparent)]
    Intent(#[from] IntentRefusal),
    #[error(transparent)]
    Denied(#[from] Denial),
    #[error(transparent)]
    Generation(#[from] host_runtime::generation::GenerationError),
    #[error(transparent)]
    Instance(#[from] host_runtime::InstanceError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    CatchUp(#[from] crate::search_catchup::CatchUpError),
    #[error("catch-up blocked: {0:?}")]
    Blocked(Blocked),
    #[error("replacement contract mismatch: {0}")]
    Invalid(&'static str),
    #[error("fresh snapshot {snapshot} exceeds fixed target {target}")]
    SnapshotBeyondTarget { snapshot: i64, target: i64 },
    #[error("staging outcome requires reconciliation; capture and family remain owned")]
    StagingUnresolved,
    #[error("replacement budget or gate expired")]
    Expired,
    #[error("source inventory exceeds its aggregate admission")]
    InventoryBound,
}

/// Holds the lifecycle transaction lock and any open or closed replacement file lease.
/// Dropping this owner releases locks, not durable intent or kernel source holds.
///
/// The outbox consumer registered by `run` belongs to `intent.consumer`, not to one attempt:
/// `cleanup` keeps it registered so a replayed registration receipt still names a live consumer.
/// While registered, its checkpoint bounds `prune_outbox` and descriptor-evidence reclamation,
/// so the operation that ends the intent must also deregister or abandon the consumer.
pub struct ReplacementBuilder<'a> {
    kernel: &'a KernelStore,
    gate: &'a HookGate,
    lifecycle: ProjectionLifecycle,
    store: GenerationStore,
    _transaction: LifecycleTransactionLock,
    data_home: PathBuf,
    home: PathBuf,
    spec: ReplacementSpec,
    capture: Option<ReplacementCapture>,
    hold_deadline: Option<Instant>,
    family: Family,
    admission: Option<Run>,
    target: Option<kernel::CommitReadTarget>,
    incarnation: Option<kernel::CommitReadIncarnation>,
}

enum Family {
    Unopened,
    Open(Arc<SearchProjection>),
    Closed(Box<ClosedSeed>),
    Leased { _lease: lease::HeldFileLease },
}

impl Family {
    fn seed(&self) -> Option<&ClosedSeed> {
        match self {
            Self::Closed(seed) => Some(seed),
            _ => None,
        }
    }
}

/// Retain this value to retry cleanup; do not unlink its files independently.
pub struct BuildFailure<'a> {
    pub error: BuildError,
    /// Cleanup is deferred when the operation budget ends; retain the owner and retry cleanup explicitly.
    pub cleanup_error: Option<BuildError>,
    owner: ReplacementBuilder<'a>,
}

impl std::fmt::Debug for BuildFailure<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuildFailure")
            .field("error", &self.error)
            .field("cleanup_error", &self.cleanup_error)
            .finish_non_exhaustive()
    }
}

impl<'a> BuildFailure<'a> {
    pub fn retry(
        self: Box<Self>,
        budget: &EvalBudget,
        observer: &mut dyn FnMut(BuildEvent),
    ) -> Result<VerifiedReplacement<'a>, Box<Self>> {
        self.owner.build(budget, observer)
    }

    /// Bounds explicit cleanup without renewing the construction allowance or deadline.
    pub fn cleanup(&mut self, budget: &EvalBudget) -> Result<(), BuildError> {
        self.owner.cleanup(budget)
    }
}

/// An unselected candidate. Revalidate it immediately before a selection attempt.
/// This value retains the builder's locks; it does not perform selection.
pub struct VerifiedReplacement<'a> {
    owner: ReplacementBuilder<'a>,
    staged: StagedSeed,
}

impl VerifiedReplacement<'_> {
    pub fn staged(&self) -> &StagedSeed {
        &self.staged
    }
    pub fn path(&self) -> &Path {
        self.owner.family.seed().expect("verified seed").path()
    }

    pub fn revalidate(&self) -> Result<(), BuildError> {
        let (intent, _) = self.owner.lifecycle.admitted_intent(self.owner.gate)?;
        let run = self
            .owner
            .admission
            .as_ref()
            .ok_or(BuildError::Invalid("missing admission"))?;
        run.check()?;
        self.owner.spec.admit(self.owner.gate, &intent)?;
        if intent.staged_seed_digest.as_deref() != Some(&self.staged.digest)
            || intent.replacement_capture.as_deref() != self.owner.capture.as_ref()
            || intent.consumer.generation_id != self.staged.verification.generation_id
            || intent.kernel_incarnation_id != self.staged.verification.kernel_incarnation_id
            || intent.recovery_target.map(|target| target.commit_seq)
                != Some(self.staged.verification.checkpoint_commit_seq)
        {
            return Err(BuildError::StagingUnresolved);
        }
        self.owner.check_hold(&run.budget)?;
        let seed = self.owner.family.seed().expect("verified seed");
        search_seed::verify_certificate(seed)?;
        self.owner.store.validate(&self.staged.digest)?;
        self.owner.check_hold(&run.budget)?;
        run.check()
    }
}

impl<'a> ReplacementBuilder<'a> {
    /// Persist the intended transition before opening a builder.
    /// Requested bounds must fit the gate's manifest and identity.
    pub fn open(
        data_home: &Path,
        kernel: &'a KernelStore,
        gate: &'a HookGate,
        spec: ReplacementSpec,
    ) -> Result<Self, BuildError> {
        let grant = gate.admit(ProjectionHook::EmbeddingBootstrap, EntryPoint::Reload)?;
        gate.check_limits(&grant, &InvalidationIdentity::from(&spec.identity), &[])?;
        let transaction = LifecycleTransactionLock::acquire_exclusive(Some(data_home))?;
        let lifecycle = ProjectionLifecycle::open(data_home)?;
        let (intent, _) = lifecycle.admitted_intent(gate)?;
        spec.admit(gate, &intent)?;
        if intent.kernel_incarnation_id != spec.identity.kernel_incarnation_id
            || intent.consumer.generation_id != spec.generation.generation_id
            || intent.staged_seed_digest.is_some()
        {
            return Err(BuildError::Invalid(
                "intent does not name an unselected construction",
            ));
        }
        Ok(Self {
            kernel,
            gate,
            lifecycle,
            store: GenerationStore::open(Some(data_home))?,
            _transaction: transaction,
            data_home: data_home.to_path_buf(),
            home: data_home
                .join(crate::projection_lifecycle::CONTROL_DIR)
                .join("replacement"),
            spec,
            capture: intent.replacement_capture.map(|capture| *capture),
            hold_deadline: None,
            family: Family::Unopened,
            admission: None,
            target: None,
            incarnation: None,
        })
    }

    /// Calls `barrier` at each control-record write barrier of this builder's lifecycle handle.
    #[cfg(feature = "test-support")]
    pub fn with_lifecycle_write_barrier_for_test(
        self,
        barrier: impl Fn(crate::projection_lifecycle::WriteBarrier) + Send + Sync + 'static,
    ) -> Self {
        Self {
            lifecycle: self.lifecycle.with_write_barrier_for_test(barrier),
            ..self
        }
    }

    /// Runs one attempt under `budget` and the intent's original allowance and deadline.
    /// On failure, retain the returned owner until cleanup succeeds or ownership is handed off.
    /// Synchronous filesystem and COMMIT completion require healthy I/O; cancellation cannot preempt them.
    /// The catch-up episode observes a caller interrupt at its next event boundary; only `budget`'s deadline ends a kernel wait inside the episode.
    /// A revoked gate grant never cancels `budget`.
    pub fn build(
        mut self,
        budget: &EvalBudget,
        observer: &mut dyn FnMut(BuildEvent),
    ) -> Result<VerifiedReplacement<'a>, Box<BuildFailure<'a>>> {
        match self.run(budget, observer) {
            Ok(staged) => Ok(VerifiedReplacement {
                owner: self,
                staged,
            }),
            Err(error) => {
                observer(BuildEvent::Aborting);
                let cleanup_error = if budget.is_exhausted() {
                    Some(BuildError::Expired)
                } else {
                    self.cleanup(budget).err()
                };
                Err(Box::new(BuildFailure {
                    error,
                    cleanup_error,
                    owner: self,
                }))
            }
        }
    }

    fn binding(&self) -> Result<SourceHoldBinding, BuildError> {
        let (intent, _) = self.lifecycle.admitted_intent(self.gate)?;
        Ok(SourceHoldBinding {
            consumer_id: intent.consumer.consumer_id,
            lease_epoch: self
                .capture
                .as_ref()
                .map_or(self.kernel.lease_epoch(), |capture| capture.lease_epoch),
            source_policy_version: self.capture.as_ref().map_or_else(
                || self.spec.identity.projection_policy_version.clone(),
                |capture| capture.source_policy_version.clone(),
            ),
        })
    }

    fn check_hold(&self, budget: &EvalBudget) -> Result<(), BuildError> {
        self.check_incarnation(budget)?;
        let deadline = self
            .hold_deadline
            .ok_or(BuildError::Invalid("missing live hold clock"))?;
        if Instant::now() >= deadline {
            return Err(SourceHoldError::Invalid(SourceHoldInvalidity::Expired).into());
        }
        let capture = self
            .capture
            .as_ref()
            .ok_or(BuildError::Invalid("missing capture"))?;
        self.kernel.source_hold_status_within_budget(
            budget,
            &self.binding()?,
            &capture.hold_id,
            wall_ms()?,
        )?;
        if Instant::now() >= deadline {
            return Err(SourceHoldError::Invalid(SourceHoldInvalidity::Expired).into());
        }
        Ok(())
    }

    fn checked_target(&self, budget: &EvalBudget) -> Result<kernel::CommitReadTarget, BuildError> {
        let before = self
            .kernel
            .capture_commit_read_target_within_budget(budget)?;
        let identity = self.kernel.database_incarnation_id_within_budget(budget)?;
        let after = self
            .kernel
            .capture_commit_read_target_within_budget(budget)?;
        if before.incarnation != after.incarnation {
            return Err(BuildError::Blocked(Blocked::Read(
                CommitReadError::IncarnationMismatch,
            )));
        }
        if identity != self.spec.identity.kernel_incarnation_id {
            return Err(retrieval::ProjectionError::IdentityMismatch.into());
        }
        Ok(before)
    }

    fn check_incarnation(&self, budget: &EvalBudget) -> Result<(), BuildError> {
        if let Some(incarnation) = self.incarnation
            && self.checked_target(budget)?.incarnation != incarnation
        {
            return Err(BuildError::Blocked(Blocked::Read(
                CommitReadError::IncarnationMismatch,
            )));
        }
        Ok(())
    }

    fn protected(&self) -> Result<BTreeSet<String>, BuildError> {
        let mut protected =
            ProjectionLifecycle::protected_generations(&self.data_home, &self._transaction)?;
        if let CurrentProfile::Current(current) = self.store.read_current()? {
            protected.insert(current);
        }
        Ok(protected)
    }

    fn run(
        &mut self,
        budget: &EvalBudget,
        observer: &mut dyn FnMut(BuildEvent),
    ) -> Result<StagedSeed, BuildError> {
        if budget.is_exhausted() {
            return Err(BuildError::Expired);
        }
        let (intent, _) = self.lifecycle.admitted_intent(self.gate)?;
        let grants = self.spec.admit(self.gate, &intent)?;
        let started = Instant::now();
        let now = wall_ms()?;
        let end = started
            .checked_add(Duration::from_millis(
                u64::try_from(intent.episodes.deadline - now).map_err(|_| BuildError::Expired)?,
            ))
            .ok_or(BuildError::Expired)?;
        let Some(deadline) = budget.deadline().filter(|deadline| *deadline <= end) else {
            return Err(BuildError::Invalid(
                "budget must end within the original episode deadline",
            ));
        };
        let fresh = self.checked_target(budget)?;
        let staging = self
            .capture
            .as_ref()
            .is_some_and(|capture| capture.stage.is_some())
            && self.family.seed().is_some();
        if staging {
            self.check_hold(budget)?;
        } else {
            self.cleanup(budget)?;
        }
        let mut expected = intent.clone();
        if !staging {
            expected.replacement_capture = None;
        }
        self.lifecycle
            .consume_expected_episode(self.gate, &expected, wall_ms()?)?;
        let run = Run {
            budget: budget.clone(),
            grants,
            deadline,
            started,
            now,
        };
        self.admission = Some(run.clone());
        run.check()?;
        if staging {
            return self.stage(&run, observer);
        }
        self.incarnation = Some(fresh.incarnation);
        self.target = match intent.recovery_target {
            Some(fixed) if fixed.commit_seq > fresh.through_commit => {
                return Err(BuildError::Blocked(Blocked::Read(
                    CommitReadError::TargetBeyondTip,
                )));
            }
            Some(fixed) => Some(kernel::CommitReadTarget {
                through_commit: fixed.commit_seq,
                incarnation: fresh.incarnation,
            }),
            None => None,
        };
        let (intent, _) = self.lifecycle.admitted_intent(self.gate)?;
        let binding = self.binding()?;
        let registration = CommitIntent {
            producer: "search-replacement".to_owned(),
            operation_key: intent.attempt_id.clone(),
            request_digest: format!("{:x}", Sha256::digest(binding.consumer_id.as_bytes())),
            actor: "daemon".to_owned(),
            cause: "replacement capture".to_owned(),
        };
        // Only a handoff that holds its own registration receipt hands the consumer over. The
        // registration attempt still commits under its own key so retries replay its receipt.
        let inherited = intent
            .prior_disabled
            .as_deref()
            .and_then(|disabled| disabled.handoff.as_deref())
            .filter(|handoff| handoff.consumer.consumer_id == binding.consumer_id)
            .map(|handoff| CommitIntent {
                operation_key: handoff.attempt_id.clone(),
                ..registration.clone()
            });
        self.kernel
            .commit_within_budget(&run.budget, registration, |envelope| {
                if let Some(prior) = inherited.clone()
                    && envelope.stored_receipt(prior)?.is_some()
                {
                    return Ok(String::new());
                }
                envelope.register_outbox_consumer(&binding.consumer_id, run.now())?;
                Ok(String::new())
            })?;
        if self
            .kernel
            .outbox_consumer_checkpoint_within_budget(&run.budget, &binding.consumer_id)?
            .is_none()
        {
            return Err(kernel::SourceHoldError::UnknownConsumer.into());
        }
        run.check()?;
        let capture_started = Instant::now();
        let hold = self.kernel.capture_source_hold_within_budget(
            &run.budget,
            &binding,
            self.spec.capture,
        )?;
        self.hold_deadline =
            capture_started.checked_add(Duration::from_millis(self.spec.capture.expiry_ms.get()));
        self.capture = Some(ReplacementCapture {
            hold_id: hold.hold_id.clone(),
            snapshot: hold.snapshot,
            lease_epoch: binding.lease_epoch,
            source_policy_version: binding.source_policy_version.clone(),
            expires_at: hold.expires_at,
            stage: None,
        });
        observer(BuildEvent::CaptureHeld {
            snapshot: hold.snapshot,
            hold_id: hold.hold_id.clone(),
        });
        self.lifecycle
            .record_capture(self.gate, &self._transaction, self.capture.clone(), None)?;
        self.check_incarnation(&run.budget)?;
        if let Some(target) = intent.recovery_target
            && hold.snapshot > target.commit_seq
        {
            return Err(BuildError::SnapshotBeyondTarget {
                snapshot: hold.snapshot,
                target: target.commit_seq,
            });
        }
        observer(BuildEvent::Captured {
            snapshot: hold.snapshot,
            hold_id: hold.hold_id.clone(),
        });
        let rows = self.inventory(&run, &[ExportWindow::Snapshot], observer)?;
        run.check()?;
        self.family = Family::Open(Arc::new(SearchProjection::open(&self.home)?));
        let Family::Open(projection) = &self.family else {
            unreachable!()
        };
        let mutation = MutationIdentity {
            kernel_incarnation_id: self.spec.identity.kernel_incarnation_id.clone(),
            hold_id: hold.hold_id.clone(),
            snapshot_commit_seq: hold.snapshot,
            through_commit_seq: hold.snapshot,
        };
        let identities = row_identities(&rows);
        let batch = batch_from_rows(
            &rows,
            &identities,
            mutation.clone(),
            Some(&self.spec.generation.generation_id),
        )?;
        let mut interrupted = false;
        let applied = projection.write_within(run.deadline, |conn| {
            retrieval::install_identity(conn, &self.spec.identity, run.now())?;
            register_generation(conn, &self.spec.generation, run.now())?;
            retrieval::batch::apply_batch(conn, &batch, self.spec.episode.batch, run.now())?;
            observer(BuildEvent::BaselineStaged);
            if run.check().is_err() {
                interrupted = true;
                return Err(retrieval::ProjectionError::MutationConflict);
            }
            Ok(())
        });
        if interrupted
            || matches!(
                &applied,
                Err(crate::search_projection::SearchProjectionError::Store(
                    storage::StoreError::Deadline
                ))
            )
        {
            return Err(BuildError::Expired);
        }
        applied?;
        observer(BuildEvent::BaselineReleased);
        drop(batch);
        drop(identities);
        drop(rows);
        run.check()?;
        let target = match self.target {
            Some(target) => target,
            None => self
                .kernel
                .capture_commit_read_target_within_budget(&run.budget)?,
        };
        if target.incarnation != fresh.incarnation {
            return Err(BuildError::Blocked(Blocked::Read(
                CommitReadError::IncarnationMismatch,
            )));
        }
        self.target = Some(target);
        self.lifecycle.fix_target(
            self.gate,
            RecoveryTarget {
                commit_seq: target.through_commit,
            },
        )?;
        observer(BuildEvent::TargetFixed(target.through_commit));
        run.check()?;
        let consumer = CatchUpConsumer {
            binding,
            hold_id: hold.hold_id,
            kernel_incarnation_id: mutation.kernel_incarnation_id.clone(),
            generation_id: Some(self.spec.generation.generation_id.clone()),
        };
        // `EvalBudget::cancel` reaches every clone, so the episode gets its own interrupt.
        let episode = EvalBudget::new(Some(run.deadline), Arc::new(AtomicBool::new(false)));
        let report = SearchCatchUp::new(self.kernel, projection)
            .with_budget(episode.clone())
            .run_episode_toward(
                &consumer,
                &self.spec.episode,
                target,
                run.now(),
                &mut |event| {
                    observer(BuildEvent::CatchUp(event));
                    if run.check().is_err() {
                        episode.cancel();
                    }
                },
            )?;
        if let EpisodeEnd::Blocked(blocked) = report.end {
            return Err(BuildError::Blocked(blocked));
        }
        run.check()?;
        observer(BuildEvent::Verifying);
        let rows = self.inventory(
            &run,
            &[
                ExportWindow::Snapshot,
                ExportWindow::CatchUp {
                    through: target.through_commit,
                },
            ],
            observer,
        )?;
        let identities = row_identities(&rows);
        let batch = batch_from_rows(
            &rows,
            &identities,
            MutationIdentity {
                through_commit_seq: target.through_commit,
                ..mutation
            },
            Some(&self.spec.generation.generation_id),
        )?;
        projection.read(|conn| {
            verify_construction(
                conn,
                &batch,
                &self.spec.generation,
                CoverageBounds {
                    max_live_per_class: self.spec.episode.batch.persist.max_records,
                    max_tombstoned_per_class: self.spec.episode.batch.persist.max_records,
                },
            )
        })?;
        drop(batch);
        drop(identities);
        drop(rows);
        self.check_hold(&run.budget)?;
        run.check()?;
        let Family::Open(projection) = std::mem::replace(&mut self.family, Family::Unopened) else {
            unreachable!()
        };
        match search_seed::quiesce(
            projection,
            self.gate,
            0,
            &self.spec.identity,
            self.spec.seed,
            budget,
        ) {
            Ok(seed) => self.family = Family::Closed(Box::new(seed)),
            Err(failure) => {
                self.family = failure.projection.map_or(Family::Unopened, Family::Open);
                return Err(failure.refusal.into());
            }
        }
        observer(BuildEvent::Closed);
        self.stage(&run, observer)
    }

    fn stage(
        &mut self,
        run: &Run,
        observer: &mut dyn FnMut(BuildEvent),
    ) -> Result<StagedSeed, BuildError> {
        run.check()?;
        self.check_hold(&run.budget)?;
        let seed = self.family.seed().expect("closed seed");
        search_seed::verify_certificate(seed)?;
        let expected = seed.verification().stage_manifest().digest();
        let (intent, _) = self.lifecycle.admitted_intent(self.gate)?;
        if let Some(pinned) = intent.staged_seed_digest {
            if pinned != expected {
                return Err(BuildError::StagingUnresolved);
            }
            self.store.validate(&expected)?;
            self.check_hold(&run.budget)?;
            run.check()?;
            return Ok(StagedSeed {
                digest: expected,
                verification: seed.verification().clone(),
            });
        }
        let capture = self.capture.as_mut().expect("captured hold");
        capture.stage = Some(Box::new(seed.verification().clone()));
        let hold_id = capture.hold_id.clone();
        self.lifecycle.record_capture(
            self.gate,
            &self._transaction,
            self.capture.clone(),
            Some(&hold_id),
        )?;
        observer(BuildEvent::StagePrepared);
        run.check()?;
        let protected = self.protected()?;
        let staged = search_seed::stage(
            self.family.seed().expect("closed seed"),
            &self.store,
            &self._transaction,
            &self.home,
            &protected,
        )?;
        observer(BuildEvent::Staged);
        run.check()?;
        self.check_hold(&run.budget)?;
        self.store.validate(&staged.digest)?;
        self.lifecycle
            .pin_seed(self.gate, &self._transaction, &staged.digest)?;
        observer(BuildEvent::Pinned);
        Ok(staged)
    }

    fn inventory(
        &self,
        run: &Run,
        windows: &[ExportWindow],
        observer: &mut dyn FnMut(BuildEvent),
    ) -> Result<Vec<SourceRow>, BuildError> {
        let binding = self.binding()?;
        let capture = self.capture.as_ref().expect("captured hold");
        let mut rows = BTreeMap::<String, SourceRow>::new();
        // `apply_batch` refuses more mutations than `max_local_mutations`, so the smaller record
        // or mutation bound caps the rows retained before that write.
        let batch = &self.spec.episode.batch;
        let mut remaining_rows = batch
            .persist
            .max_records
            .get()
            .min(batch.max_local_mutations.get());
        let mut remaining_bytes = batch.max_source_bytes.get() as u64;
        // The grant reserves the inventory plus one page, including overlays at full capacity.
        for &window in windows {
            if matches!(window, ExportWindow::CatchUp { through } if through == capture.snapshot) {
                continue;
            }
            let mut cursor = None;
            // `max_source_encoded_bytes` applies independently to each export window.
            let mut remaining_encoded = self.spec.episode.max_source_encoded_bytes.get();
            for page_number in 0..self.spec.episode.max_source_pages.get() {
                run.check()?;
                self.check_incarnation(&run.budget)?;
                let mut bounds = self.spec.episode.source_page;
                bounds.max_decoded_bytes = bounds
                    .max_decoded_bytes
                    .min(NonZeroU64::new(remaining_bytes).unwrap_or(NonZeroU64::MIN));
                bounds.max_encoded_bytes = bounds
                    .max_encoded_bytes
                    .min(NonZeroU64::new(remaining_encoded).unwrap_or(NonZeroU64::MIN));
                let page = self
                    .kernel
                    .export_source_page_within_budget(
                        &run.budget,
                        &binding,
                        &capture.hold_id,
                        run.now(),
                        window,
                        cursor.as_ref(),
                        bounds,
                    )
                    .map_err(|error| match error {
                        SourceExportError::OversizedRow {
                            bound: PageBound::Decoded,
                            bytes,
                            ..
                        } if bytes <= self.spec.episode.source_page.max_decoded_bytes.get() => {
                            BuildError::InventoryBound
                        }
                        SourceExportError::OversizedRow {
                            bound: PageBound::Encoded,
                            bytes,
                            ..
                        } if bytes <= self.spec.episode.source_page.max_encoded_bytes.get() => {
                            BuildError::InventoryBound
                        }
                        error => BuildError::Export(error),
                    })?;
                observer(BuildEvent::Exported {
                    window,
                    rows: page.charge.rows,
                });
                remaining_encoded = remaining_encoded
                    .checked_sub(page.charge.encoded_bytes)
                    .ok_or(BuildError::InventoryBound)?;
                for row in page.rows {
                    if let Some(existing) = rows.get_mut(&row.detail.occurrence_id) {
                        if row.text.is_some() || row.detail != existing.detail {
                            return Err(BuildError::Invalid(
                                "duplicate or changed source occurrence",
                            ));
                        }
                        existing.invalidated_commit_seq = row.invalidated_commit_seq;
                        existing.superseded_by = row.superseded_by;
                    } else {
                        if row.text.is_none() {
                            return Err(BuildError::Invalid(
                                "invalidation has no baseline occurrence",
                            ));
                        }
                        remaining_rows = remaining_rows
                            .checked_sub(1)
                            .ok_or(BuildError::InventoryBound)?;
                        remaining_bytes = remaining_bytes
                            .checked_sub(row.text.as_ref().expect("text checked").len() as u64)
                            .ok_or(BuildError::InventoryBound)?;
                        rows.insert(row.detail.occurrence_id.clone(), row);
                    }
                }
                cursor = page.next;
                if cursor.is_none() {
                    break;
                }
                if page_number + 1 == self.spec.episode.max_source_pages.get() {
                    return Err(BuildError::InventoryBound);
                }
            }
        }
        run.check()?;
        self.check_incarnation(&run.budget)?;
        Ok(rows.into_values().collect())
    }

    fn cleanup(&mut self, budget: &EvalBudget) -> Result<(), BuildError> {
        if budget.is_exhausted() {
            return Err(BuildError::Expired);
        }
        let target = self.checked_target(budget)?;
        let (intent, _) = self.lifecycle.admitted_intent(self.gate)?;
        if let Some(capture) = &self.capture
            && let Some(stage) = capture.stage.as_deref()
        {
            if intent.staged_seed_digest.is_some() {
                return Err(BuildError::StagingUnresolved);
            }
            let recorded = intent.replacement_capture.as_deref();
            let unrecorded = ReplacementCapture {
                stage: None,
                ..capture.clone()
            };
            if recorded == Some(capture) {
                let manifest = stage.stage_manifest();
                let mut protected = self.protected()?;
                protected.remove(&manifest.digest());
                self.store
                    .discard_unselected(&manifest, &self._transaction, &protected)?;
            } else if recorded.is_some() && recorded != Some(&unrecorded) {
                return Err(BuildError::StagingUnresolved);
            }
            // An absent record under this owner's lock is its own earlier clear whose directory sync
            // did not return; the discard and family removal before that clear already ran.
            // `stage` writes the certificate before `search_seed::stage`, so an unrecorded certificate left no object to discard.
        }
        self.family = match std::mem::replace(&mut self.family, Family::Unopened) {
            Family::Open(projection) => match Arc::try_unwrap(projection) {
                Ok(projection) => Family::Leased {
                    _lease: projection
                        .close()
                        .1
                        .ok_or(BuildError::Invalid("missing database lease"))?,
                },
                Err(projection) => {
                    self.family = Family::Open(projection);
                    return Err(BuildError::Invalid("live projection handles"));
                }
            },
            family => family,
        };
        let recorded = intent
            .replacement_capture
            .as_deref()
            .map(|capture| capture.hold_id.clone());
        self.lifecycle
            .delete_replacement_family(self.gate, recorded.as_deref(), || {
                self.family = Family::Unopened
            })?;
        if let Some(capture) = &self.capture
            && capture.lease_epoch == self.kernel.lease_epoch()
        {
            let binding = self.binding()?;
            match self.kernel.release_source_hold_within_budget(
                budget,
                &binding,
                &capture.hold_id,
                wall_ms()?,
            ) {
                Err(SourceHoldError::Invalid(SourceHoldInvalidity::Missing))
                    if self
                        .incarnation
                        .is_some_and(|held| held != target.incarnation) => {}
                released => released?,
            }
        }
        let binding = self.binding()?;
        self.kernel.reconcile_source_holds_within_budget(
            budget,
            &binding.consumer_id,
            wall_ms()?,
        )?;
        if recorded.is_some() || self.capture.is_some() {
            // `record_capture` only re-syncs the directory when nothing is recorded, making the earlier clear durable.
            self.lifecycle.record_capture(
                self.gate,
                &self._transaction,
                None,
                recorded.as_deref(),
            )?;
        }
        self.capture = None;
        self.hold_deadline = None;
        self.family = Family::Unopened;
        Ok(())
    }
}

#[derive(Clone)]
struct Run {
    budget: EvalBudget,
    grants: Vec<Admission>,
    deadline: Instant,
    started: Instant,
    now: i64,
}

impl Run {
    fn now(&self) -> i64 {
        crate::search_catchup::audit_time(self.now, self.started.elapsed())
    }
    fn check(&self) -> Result<(), BuildError> {
        if self.budget.check().is_err()
            || self
                .grants
                .iter()
                .any(|grant| grant.invalidated.is_cancelled())
            || Instant::now() >= self.deadline
        {
            Err(BuildError::Expired)
        } else {
            Ok(())
        }
    }
}

fn wall_ms() -> Result<i64, BuildError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|time| i64::try_from(time.as_millis()).ok())
        .ok_or(BuildError::Expired)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_now_rounds_a_partial_millisecond_up() {
        // The kernel refuses a hold at `now >= expires_at`, so elapsed time must never round down.
        let run = Run {
            budget: EvalBudget::unbounded(),
            grants: Vec::new(),
            deadline: Instant::now() + Duration::from_secs(60),
            started: Instant::now() - Duration::from_micros(500),
            now: 1_000,
        };
        assert!(run.now() >= 1_001, "{}", run.now());
    }
}
