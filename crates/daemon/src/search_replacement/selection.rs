use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::num::NonZeroUsize;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arc_swap::ArcSwapOption;
use host_runtime::LifecycleTransactionLock;
use host_runtime::generation::{
    CurrentProfile, GenerationError, GenerationStore, ProfileEvent, ValidatedGeneration,
};
use kernel::applicability::EvalBudget;
use kernel::{ArtifactDestination, CommitReadIncarnation, KernelStore, ProjectScope};
use retrieval::batch::VectorGeneration;
use retrieval::coverage::{CoverageBounds, CoverageReport, verify_active};
use retrieval::{ProjectionError, ProjectionIdentity};
use serde::{Deserialize, Serialize};
use storage::GuardedConn;

use super::{BuildError, VerifiedReplacement};
use crate::projection_gates::{
    Admission, EntryPoint, HookGate, InvalidationIdentity, ProjectionHook,
};
use crate::projection_lifecycle::{
    ConsumerBinding, LifecycleIntent, MAX_RECORD_BYTES, ProjectionLifecycle,
};
use crate::search_projection::{SearchProjection, search_descriptor};
use crate::search_seed::{SEED_FILE, SeedVerification};
use crate::search_writer::QuarantineKind;

const FAMILIES: &str = "search-families";
const CERTIFICATE: &str = "bootstrap.json";

pub mod disable;
pub mod retirement;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetiringFamily {
    seed: SeedVerification,
    consumer: ConsumerBinding,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Bootstrap {
    schema: u32,
    seed: SeedVerification,
    intent: LifecycleIntent,
    retiring: Option<RetiringFamily>,
}

impl Bootstrap {
    fn into_retiring(self) -> RetiringFamily {
        RetiringFamily {
            seed: self.seed,
            consumer: self.intent.consumer,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionEvent {
    MetadataWritten,
    Copied,
    Profile(ProfileEvent),
}

pub struct SelectionFailure<'a> {
    pub error: BuildError,
    candidate: VerifiedReplacement<'a>,
}

impl std::fmt::Debug for SelectionFailure<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)
    }
}

impl<'a> SelectionFailure<'a> {
    pub fn retry(
        self: Box<Self>,
        selection: &SearchSelection,
        observer: &mut dyn FnMut(SelectionEvent) -> Result<(), GenerationError>,
    ) -> Result<(), Box<Self>> {
        selection.select(self.candidate, observer)
    }

    pub fn reconcile(
        &self,
        selection: &SearchSelection,
        budget: &EvalBudget,
    ) -> Result<(), BuildError> {
        let owner = &self.candidate.owner;
        if selection.data_home != owner.data_home {
            return Err(BuildError::Invalid(
                "candidate belongs to another data home",
            ));
        }
        selection.reopen_locked(owner.kernel, owner.gate, budget, &owner._transaction)
    }
}

/// Reader clones retain the database lease and immutable seed pin even after request cancellation.
#[derive(Clone)]
pub struct SearchReader {
    family: Arc<SelectedFamily>,
    grant: Admission,
}

struct SelectedFamily {
    projection: Arc<SearchProjection>,
    certificate: Bootstrap,
    incarnation: CommitReadIncarnation,
    bounds: CoverageBounds,
    _seed_pin: ValidatedGeneration,
    unavailable: std::sync::atomic::AtomicBool,
}

pub struct SearchSelection {
    data_home: PathBuf,
    identity: ProjectionIdentity,
    bounds: CoverageBounds,
    selected: ArcSwapOption<SelectedFamily>,
    maintenance: Option<disable::Maintenance>,
    #[cfg(feature = "test-support")]
    disable_barrier: Option<Arc<dyn Fn(crate::projection_lifecycle::WriteBarrier) + Send + Sync>>,
}

impl SearchSelection {
    pub fn new(data_home: &Path, identity: ProjectionIdentity, bounds: CoverageBounds) -> Self {
        Self {
            data_home: data_home.to_owned(),
            identity,
            bounds,
            selected: ArcSwapOption::empty(),
            maintenance: None,
            #[cfg(feature = "test-support")]
            disable_barrier: None,
        }
    }

    pub fn pin(
        &self,
        kernel: &KernelStore,
        gate: &HookGate,
        budget: &EvalBudget,
    ) -> Result<SearchReader, BuildError> {
        let grant = self.admit(gate, budget)?;
        let family = self
            .selected
            .load_full()
            .ok_or(BuildError::Invalid("search unavailable; rebuild required"))?;
        family
            .certificate
            .seed
            .identity()
            .require_compatible(&self.identity)?;
        family.check_kernel(kernel, budget)?;
        if family.projection.quarantine().is_some() {
            return Err(BuildError::Invalid("search quarantined; rebuild required"));
        }
        Ok(SearchReader { family, grant })
    }

    fn admit(&self, gate: &HookGate, budget: &EvalBudget) -> Result<Admission, BuildError> {
        deadline(budget)?;
        if matches!(
            ProjectionLifecycle::read_at(&self.data_home),
            crate::projection_lifecycle::ControlState::Disabled(_)
                | crate::projection_lifecycle::ControlState::Unavailable(_)
        ) {
            return Err(crate::projection_lifecycle::IntentRefusal::Disabled.into());
        }
        let grant = gate.admit(ProjectionHook::EmbeddingBootstrap, EntryPoint::Reload)?;
        gate.check_limits(
            &grant,
            &InvalidationIdentity::from(&self.identity),
            &[
                (
                    "local_transaction_rows",
                    self.bounds.max_live().saturating_add(
                        self.bounds.max_tombstoned_per_class.get().saturating_mul(5),
                    ) as u64,
                ),
                (
                    "export_page_rows",
                    self.bounds.max_live_per_class.get() as u64,
                ),
            ],
        )?;
        Ok(grant)
    }

    /// Publishes the durable pointer before swapping the in-process family; errors require reconciliation through `reopen`.
    pub fn select<'a>(
        &self,
        candidate: VerifiedReplacement<'a>,
        observer: &mut dyn FnMut(SelectionEvent) -> Result<(), GenerationError>,
    ) -> Result<(), Box<SelectionFailure<'a>>> {
        self.select_candidate(&candidate, observer)
            .map_err(|error| Box::new(SelectionFailure { error, candidate }))
    }

    /// Synchronous copy and fsync require healthy filesystem I/O; budget checks cannot preempt a blocked syscall.
    fn select_candidate(
        &self,
        candidate: &VerifiedReplacement<'_>,
        observer: &mut dyn FnMut(SelectionEvent) -> Result<(), GenerationError>,
    ) -> Result<(), BuildError> {
        if self.data_home != candidate.owner.data_home {
            return Err(BuildError::Invalid(
                "candidate belongs to another data home",
            ));
        }
        candidate
            .owner
            .spec
            .identity
            .require_compatible(&self.identity)?;
        candidate.revalidate()?;
        let budget = &candidate
            .owner
            .admission
            .as_ref()
            .ok_or(BuildError::Expired)?
            .budget;
        self.admit(candidate.owner.gate, budget)?;
        let (intent, _) = candidate
            .owner
            .lifecycle
            .admitted_intent(candidate.owner.gate)?;
        if let CurrentProfile::Current(current) = candidate.owner.store.read_search_current()?
            && current != intent.selected_generation
            && current != candidate.staged.digest
        {
            return Err(BuildError::Invalid("old selection differs from intent"));
        }
        let prior = self.predecessor(&intent)?;
        if let Some(retiring) = prior.as_ref().and_then(|prior| prior.retiring.as_ref())
            && (candidate
                .owner
                .kernel
                .outbox_consumer_checkpoint_within_budget(budget, &retiring.consumer.consumer_id)?
                .is_some()
                || self
                    .family_home(&retiring.seed.stage_manifest().digest())?
                    .join(CERTIFICATE)
                    .try_exists()?)
        {
            return Err(kernel::KernelError::ConsumerPending.into());
        }
        let certificate = Bootstrap {
            schema: 2,
            seed: candidate.staged.verification.clone(),
            retiring: prior.map(Bootstrap::into_retiring),
            intent,
        };
        let home = self.family_home(&candidate.staged.digest)?;
        create_directory(&self.data_home, FAMILIES)?;
        if home.try_exists()? {
            self.remove_family(&candidate.staged.digest, &certificate)?;
        }
        create_directory(&self.data_home.join(FAMILIES), &candidate.staged.digest)?;
        let bytes = serde_json::to_vec(&certificate)
            .map_err(|_| BuildError::Invalid("bootstrap encoding"))?;
        let mut manifest = create_file(&home.join(CERTIFICATE))?;
        manifest.write_all(&bytes)?;
        observer(SelectionEvent::MetadataWritten)?;
        manifest.sync_all()?;
        open_directory(&home)?.sync_all()?;
        create_directory(&home, "search")?;
        let source = candidate.owner.store.validate(&candidate.staged.digest)?;
        let mut input = File::from(source.open_verified_file(SEED_FILE)?);
        let path = home.join("search").join(SEED_FILE);
        let mut output = create_file(&path)?;
        let mut buffer = [0; 64 * 1024];
        loop {
            deadline(budget)?;
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            output.write_all(&buffer[..count])?;
            observer(SelectionEvent::Copied)?;
        }
        deadline(budget)?;
        output.sync_all()?;
        if crate::search_seed::verify_closed(
            &path,
            &self.identity,
            candidate.owner.spec.seed.max_bytes,
            budget,
        )? != certificate.seed
        {
            return Err(BuildError::Invalid("copied seed differs from certificate"));
        }
        open_directory(&home.join("search"))?.sync_all()?;
        open_directory(&home)?.sync_all()?;
        let family = self.open_family(&candidate.staged.digest, candidate.owner.kernel, budget)?;
        let mut published = false;
        let mut retention_error = None;
        let result = candidate.owner.store.select_search(
            &candidate.staged.digest,
            &candidate.owner._transaction,
            &mut |event| {
                if event == ProfileEvent::AfterRename {
                    published = true;
                }
                observer(SelectionEvent::Profile(event))?;
                if event == ProfileEvent::BeforeRename {
                    candidate.revalidate().map_err(|error| {
                        retention_error = Some(error);
                        GenerationError::NativePayloadInvalid {
                            detail: "candidate retention or identity changed",
                        }
                    })?;
                }
                Ok(())
            },
        );
        if let Err(error) = result {
            if published {
                self.selected.store(None);
            }
            return Err(retention_error.unwrap_or_else(|| error.into()));
        }
        candidate
            .revalidate()
            .inspect_err(|_| self.selected.store(None))?;
        self.selected.store(Some(Arc::new(family)));
        Ok(())
    }

    /// Reopen recovers the selected database's own WAL without reinstalling its identity or copying its seed.
    pub fn reopen(
        &self,
        kernel: &KernelStore,
        gate: &HookGate,
        budget: &EvalBudget,
    ) -> Result<(), BuildError> {
        let transaction = LifecycleTransactionLock::acquire_exclusive(Some(&self.data_home))?;
        self.reopen_locked(kernel, gate, budget, &transaction)
    }

    fn reopen_locked(
        &self,
        kernel: &KernelStore,
        gate: &HookGate,
        budget: &EvalBudget,
        transaction: &LifecycleTransactionLock,
    ) -> Result<(), BuildError> {
        let result = (|| {
            self.admit(gate, budget)?;
            let store = GenerationStore::open(Some(&self.data_home))?;
            match store.reconcile_search(transaction)? {
                CurrentProfile::Absent => {
                    self.selected.store(None);
                    Ok(())
                }
                CurrentProfile::Quarantined => {
                    Err(BuildError::Invalid("search selector quarantined"))
                }
                CurrentProfile::Current(digest) => {
                    let family = match self
                        .selected
                        .load_full()
                        .filter(|family| family._seed_pin.digest == digest)
                    {
                        Some(family) => {
                            self.validate_family(&family, kernel, budget)
                                .inspect_err(|error| {
                                    if matches!(
                                        error,
                                        BuildError::Mutation(_)
                                            | BuildError::Projection(_)
                                            | BuildError::Invalid(_)
                                    ) {
                                        family
                                            .projection
                                            .enter_quarantine(QuarantineKind::Integrity, error);
                                    }
                                })?;
                            family
                        }
                        None => Arc::new(self.open_family(&digest, kernel, budget)?),
                    };
                    self.admit(gate, budget)?;
                    self.selected.store(Some(family));
                    Ok(())
                }
            }
        })();
        result.inspect_err(|_| self.selected.store(None))
    }

    fn family_home(&self, digest: &str) -> Result<PathBuf, BuildError> {
        if !host_runtime::lifecycle::is_canonical_payload_digest(digest) {
            return Err(BuildError::Invalid("noncanonical family digest"));
        }
        Ok(self.data_home.join(FAMILIES).join(digest))
    }

    fn open_family(
        &self,
        digest: &str,
        kernel: &KernelStore,
        budget: &EvalBudget,
    ) -> Result<SelectedFamily, BuildError> {
        let home = self.family_home(digest)?;
        open_directory(&self.data_home.join(FAMILIES))?;
        open_directory(&home)?;
        open_directory(&home.join("search"))?;
        let bytes = certificate_bytes(&home)?;
        let certificate: Bootstrap =
            serde_json::from_slice(&bytes).map_err(|_| BuildError::Invalid("bootstrap corrupt"))?;
        if certificate.schema != 2
            || certificate.seed.stage_manifest().digest() != digest
            || certificate.intent.staged_seed_digest.as_deref() != Some(digest)
            || certificate.intent.consumer.consumer_id.is_empty()
            || certificate.intent.consumer.generation_id != certificate.seed.generation_id
        {
            return Err(BuildError::Invalid("bootstrap binding mismatch"));
        }
        certificate
            .seed
            .identity()
            .require_compatible(&self.identity)?;
        let seed_pin = GenerationStore::open(Some(&self.data_home))?.validate(digest)?;
        seed_pin.pin()?;
        if !home.join("search").join(SEED_FILE).is_file() {
            return Err(BuildError::Invalid("selected database missing"));
        }
        let family = SelectedFamily {
            unavailable: std::sync::atomic::AtomicBool::new(false),
            projection: Arc::new(SearchProjection::open(&home)?),
            certificate,
            incarnation: kernel
                .capture_commit_read_target_within_budget(budget)?
                .incarnation,
            bounds: self.bounds,
            _seed_pin: seed_pin,
        };
        self.validate_family(&family, kernel, budget)?;
        Ok(family)
    }

    fn validate_family(
        &self,
        family: &SelectedFamily,
        kernel: &KernelStore,
        budget: &EvalBudget,
    ) -> Result<(), BuildError> {
        family.check_kernel(kernel, budget)?;
        let report = family.projection.read_within(deadline(budget)?, |conn| {
            verify_active(conn, &self.identity, &family.generation(), self.bounds)
        })?;
        let seed = &family.certificate.seed;
        if report.checkpoint.snapshot_commit_seq != seed.snapshot_commit_seq
            || report.checkpoint.checkpoint_commit_seq < seed.checkpoint_commit_seq
            || report.checkpoint.hold_id != seed.hold_id
            || report.checkpoint.checkpoint_commit_seq
                > kernel
                    .capture_commit_read_target_within_budget(budget)?
                    .through_commit
        {
            return Err(BuildError::Invalid(
                "active checkpoint differs from certified prefix",
            ));
        }
        let ack = kernel.outbox_consumer_checkpoint_within_budget(
            budget,
            &family.certificate.intent.consumer.consumer_id,
        )?;
        if ack.is_none_or(|ack| {
            ack < seed.checkpoint_commit_seq || ack > report.checkpoint.checkpoint_commit_seq
        }) {
            return Err(BuildError::Invalid("selected consumer checkpoint mismatch"));
        }
        for class in kernel::source_identity::OccurrenceClass::ALL {
            let inventory = kernel.live_source_descriptors(
                class,
                report.checkpoint.checkpoint_commit_seq,
                None,
                self.bounds.max_live_per_class,
                budget,
            )?;
            if inventory.next.is_some() || inventory.rows.len() != report.class(class).lexical {
                return Err(BuildError::Invalid("canonical prefix inventory mismatch"));
            }
            family.projection.read_within(deadline(budget)?, |conn| {
                if retrieval::batch::read_checkpoint(conn, &self.identity.kernel_incarnation_id)?
                    .as_ref()
                    != Some(&report.checkpoint)
                {
                    return Err(ProjectionError::MutationConflict);
                }
                for source in inventory.rows {
                    let row = retrieval::read_occurrence(conn, &source.detail.occurrence_id)?
                        .ok_or(ProjectionError::CorruptRow)?;
                    if row.tombstone.is_some()
                        || row.tuple != source.detail.occurrence_tuple
                        || row.payload_id != source.detail.payload_id
                        || row.source_object_id != source.object_id
                        || row.domain_id != source.domain_id
                        || row.source_evidence_id != source.detail.evidence_id
                        || row.source_artifact_digest != source.detail.artifact_digest
                    {
                        return Err(ProjectionError::CorruptRow);
                    }
                }
                Ok(())
            })?;
        }
        family.check_kernel(kernel, budget)
    }

    pub fn reclaim(&self, digest: &str) -> Result<(), BuildError> {
        let transaction = LifecycleTransactionLock::acquire_exclusive(Some(&self.data_home))?;
        if ProjectionLifecycle::protected_generations(&self.data_home, &transaction)?
            .contains(digest)
        {
            return Err(BuildError::Invalid("lifecycle pinned family"));
        }
        let home = self.family_home(digest)?;
        if !home.try_exists()?
            || (!home.join(CERTIFICATE).try_exists()?
                && !home.join("search").join(SEED_FILE).try_exists()?)
        {
            return Ok(());
        }
        let certificate: Bootstrap = serde_json::from_slice(&certificate_bytes(&home)?)
            .map_err(|_| BuildError::Invalid("bootstrap corrupt"))?;
        self.remove_family(digest, &certificate)
    }

    pub fn recover_partial(&self, gate: &HookGate) -> Result<LifecycleIntent, BuildError> {
        let _transaction = LifecycleTransactionLock::acquire_exclusive(Some(&self.data_home))?;
        let (intent, _) = ProjectionLifecycle::open(&self.data_home)?.admitted_intent(gate)?;
        let seed = intent
            .replacement_capture
            .as_ref()
            .and_then(|capture| capture.stage.as_deref())
            .ok_or(BuildError::Invalid("missing ownership certificate"))?
            .clone();
        let digest = seed.stage_manifest().digest();
        if intent.staged_seed_digest.as_deref() != Some(&digest) {
            return Err(BuildError::Invalid("unbound ownership certificate"));
        }
        self.remove_family(
            &digest,
            &Bootstrap {
                schema: 2,
                seed,
                retiring: self.predecessor(&intent)?.map(Bootstrap::into_retiring),
                intent: intent.clone(),
            },
        )?;
        Ok(intent)
    }

    fn remove_family(&self, digest: &str, certificate: &Bootstrap) -> Result<(), BuildError> {
        match GenerationStore::open(Some(&self.data_home))?.read_search_current()? {
            CurrentProfile::Current(current) if current == digest => {
                return Err(BuildError::Invalid("selected family"));
            }
            CurrentProfile::Quarantined => {
                return Err(BuildError::Invalid("unknown selector references"));
            }
            _ => {}
        }
        if certificate.schema != 2 || certificate.seed.stage_manifest().digest() != digest {
            return Err(BuildError::Invalid("foreign family certificate"));
        }
        let home = self.family_home(digest)?;
        if !home.try_exists()? {
            return Ok(());
        }
        open_directory(&home)?;
        open_directory(&self.data_home.join(FAMILIES))?;
        if home.join("search").try_exists()? {
            open_directory(&home.join("search"))?;
        }
        if home.join(CERTIFICATE).try_exists()? {
            let found = certificate_bytes(&home)?;
            let expected = serde_json::to_vec(certificate)
                .map_err(|_| BuildError::Invalid("bootstrap encoding"))?;
            if !expected.starts_with(&found) {
                return Err(BuildError::Invalid("foreign family certificate"));
            }
        }
        let descriptor = search_descriptor(&home)
            .map_err(crate::search_projection::SearchProjectionError::from)?;
        storage::delete_sqlite_family(&descriptor)
            .map_err(crate::search_projection::SearchProjectionError::from)?;
        if home.join(CERTIFICATE).try_exists()? {
            fs::remove_file(home.join(CERTIFICATE))?;
        }
        open_directory(&home)?.sync_all()?;
        for path in [home.join("search"), home.clone()] {
            match fs::remove_dir(path) {
                Ok(()) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                    ) => {}
                Err(error) => return Err(error.into()),
            }
        }
        open_directory(&self.data_home.join(FAMILIES))?.sync_all()?;
        Ok(())
    }
}

fn certificate_bytes(home: &Path) -> Result<Vec<u8>, BuildError> {
    let mut bytes = Vec::new();
    let manifest = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(home.join(CERTIFICATE))?;
    let metadata = manifest.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.len() > MAX_RECORD_BYTES
    {
        return Err(BuildError::Invalid("insecure bootstrap certificate"));
    }
    manifest
        .take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(BuildError::Invalid("bootstrap too large"));
    }
    Ok(bytes)
}

impl SelectedFamily {
    fn generation(&self) -> VectorGeneration {
        let seed = &self.certificate.seed;
        VectorGeneration {
            generation_id: seed.generation_id.clone(),
            embedding_model: seed.embedding_model.clone(),
            tokenizer_fingerprint: seed.tokenizer_fingerprint.clone(),
            vector_dimension: seed.vector_dimension,
            generation_epoch: seed.generation_epoch,
        }
    }

    fn check_kernel(&self, kernel: &KernelStore, budget: &EvalBudget) -> Result<(), BuildError> {
        deadline(budget)?;
        if kernel
            .capture_commit_read_target_within_budget(budget)?
            .incarnation
            != self.incarnation
            || kernel.database_incarnation_id_within_budget(budget)?
                != self.certificate.seed.kernel_incarnation_id
        {
            return Err(ProjectionError::IdentityMismatch.into());
        }
        Ok(())
    }
}

impl SearchReader {
    pub fn digest(&self) -> &str {
        &self.family._seed_pin.digest
    }
    pub fn consumer(&self) -> &ConsumerBinding {
        &self.family.certificate.intent.consumer
    }
    pub fn handoff(&self) -> &LifecycleIntent {
        &self.family.certificate.intent
    }
    /// A connection clone retains the database lease; a reader clone also retains the seed pin.
    pub fn projection(&self) -> &Arc<SearchProjection> {
        &self.family.projection
    }

    /// Local observations do not authorize egress; `authorize` rechecks canonical eligibility at use.
    pub fn read<T>(
        &self,
        budget: &EvalBudget,
        read: impl FnOnce(&GuardedConn<'_>) -> Result<T, ProjectionError>,
    ) -> Result<T, BuildError> {
        if self
            .family
            .unavailable
            .load(std::sync::atomic::Ordering::Acquire)
            || self.grant.invalidated.is_cancelled()
            || self.family.projection.quarantine().is_some()
        {
            return Err(BuildError::Invalid("pinned read unavailable"));
        }
        Ok(self
            .family
            .projection
            .read_within(deadline(budget)?, read)?)
    }

    pub fn coverage(&self, budget: &EvalBudget) -> Result<CoverageReport, BuildError> {
        self.read(budget, |conn| {
            verify_active(
                conn,
                &self.family.certificate.seed.identity(),
                &self.family.generation(),
                self.family.bounds,
            )
        })
    }

    pub fn authorize(
        &self,
        kernel: &KernelStore,
        project: &ProjectScope,
        destination: ArtifactDestination,
        budget: &EvalBudget,
    ) -> Result<retrieval::eligibility::EligibilityReport, BuildError> {
        self.family.check_kernel(kernel, budget)?;
        let candidates = self.read(budget, |conn| {
            retrieval::eligibility::live_candidates(
                conn,
                None,
                NonZeroUsize::new(kernel::MAX_ELIGIBILITY_CANDIDATES).expect("positive kernel cap"),
            )
        })?;
        let result =
            retrieval::eligibility::judge_occurrences(kernel, project, destination, &candidates)?;
        self.family.check_kernel(kernel, budget)?;
        Ok(result)
    }
}

fn deadline(budget: &EvalBudget) -> Result<std::time::Instant, BuildError> {
    if budget.is_exhausted() {
        return Err(BuildError::Expired);
    }
    budget
        .deadline()
        .ok_or(BuildError::Invalid("selection requires a finite budget"))
}

fn create_directory(parent: &Path, name: &str) -> Result<(), BuildError> {
    let parent_dir = crate::projection_lifecycle::open_directory(parent)?;
    let path = parent.join(name);
    if let Err(error) = fs::DirBuilder::new().mode(0o700).create(&path)
        && error.kind() != std::io::ErrorKind::AlreadyExists
    {
        return Err(error.into());
    }
    open_directory(&path)?;
    parent_dir.sync_all()?;
    Ok(())
}

fn create_file(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

fn open_directory(path: &Path) -> std::io::Result<File> {
    let directory = crate::projection_lifecycle::open_directory(path)?;
    let metadata = directory.metadata()?;
    if metadata.mode() & 0o077 != 0 || metadata.uid() != rustix::process::geteuid().as_raw() {
        return Err(std::io::ErrorKind::PermissionDenied.into());
    }
    Ok(directory)
}
