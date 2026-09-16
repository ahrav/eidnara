//! Reads a vector composition through pinned files. Acquisition takes the lifecycle's shared protection only around manifest reads: it observes the selector, pins the candidate record and every member it names with shared locks, and charges the bytes the view keeps resident, then releases the protection before verification hashes the members under the pins alone; it keeps the row and code artifacts open on the descriptors verification hashed them through and re-reads the selector under the protection once more before handoff. A failure anywhere returns nothing, and the pins, descriptors, and charge go with it.
//! Ranking reads only the rows the resolver names, each by its offset in the row artifact into scratch charged for one page, and decodes them through the original-row codec; the int8 codes are read the same way under the layer's own scales. Positioned reads after acquisition are not re-hashed: pins protect lifetime, and same-user writes after verification are outside the cooperative-file threat model.
//! A caller runs the ranking through the request's blocking seam with the view moved into the work, so the view lives until the physical read returns whatever happens to the caller.

use std::collections::VecDeque;
use std::fs::File;
use std::num::NonZeroUsize;
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::sync::Arc;

use host_runtime::generation::{
    CurrentProfile, GenerationError, GenerationStore, PinnedGeneration, ValidatedGeneration,
};
use host_runtime::lifecycle::LifecycleTransactionLock;
use host_runtime::wire::{ByteBudget, ByteCharge};
use kernel::KernelStore;
use kernel::applicability::EvalBudget;
use retrieval::batch::ProjectionCheckpoint;
use retrieval::dense::codec::{self, ARTIFACT_HEADER_BYTES, RowLayout};
use retrieval::dense::scalar::{self, Scales};
use retrieval::dense::{
    Layer, LayeredQuery, LayeredRanking, LayeredRefusal, OracleBounds, Precedence, RowAccess,
    RowFault, rank_layers,
};
use retrieval::eligibility::Authority;
use storage::GuardedConn;

use crate::vector_composition::{self, Candidate, SelectorState, Unavailable, VerifiedComposition};
use crate::vector_generation::{
    self, ExpectedVectors, VectorRefusal, VectorSidecar, VerifiedVectors,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReaderBounds {
    pub max_deltas: NonZeroUsize,
    /// Bytes one member's payload may total; verification holds every member's payload at once.
    pub max_member_bytes: u64,
    /// Compositions recovery may fully verify when the selected one does not.
    pub recovery_bound: NonZeroUsize,
}

/// Where acquisition stands; a test may hold or mutate the store here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquireEvent {
    /// The candidate's record and members are pinned, its resident bytes are charged, and the transaction lock is released; verification is about to hash the members.
    BeforeVerification,
    /// Every pin and every charge is held; the last layer is about to be taken.
    BeforeLastLayer,
    /// Every layer is held; the selector is about to be re-read.
    BeforeSelectorRecheck,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AcquireRefusal {
    #[error("the lifecycle store is not observable: {0}")]
    Unprotected(String),
    #[error(transparent)]
    Unavailable(#[from] Unavailable),
    #[error("the lifecycle store refused: {0}")]
    Store(String),
    #[error("the view would keep {bytes} bytes resident; the residency budget refused them")]
    Residency { bytes: usize },
    #[error("the selector moved during acquisition: it named {observed:?}, now {current:?}")]
    SelectorMoved {
        observed: SelectorState,
        current: CurrentProfile,
    },
}

impl From<GenerationError> for AcquireRefusal {
    fn from(error: GenerationError) -> Self {
        Self::Store(error.to_string())
    }
}

/// One member's open artifacts and resident tables; the layer keeps its shared lock through `_generation`.
pub struct PinnedLayer {
    pub digest: String,
    pub sidecar: VectorSidecar,
    pub precedence: Precedence,
    pub checkpoint: ProjectionCheckpoint,
    pub scales: Scales,
    occurrence_ids: Vec<String>,
    tombstones: Vec<String>,
    rows: File,
    codes: File,
    _generation: ValidatedGeneration,
}

impl std::fmt::Debug for PinnedLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PinnedLayer")
            .field("digest", &self.digest)
            .field("precedence", &self.precedence)
            .field("rows", &self.occurrence_ids.len())
            .field("tombstones", &self.tombstones.len())
            .finish_non_exhaustive()
    }
}

impl PinnedLayer {
    /// Verification proved the artifacts hold exactly one row per identifier, so the row set of the composition is what the reader hands the resolver.
    fn from_member(member: VerifiedVectors, epoch: u64, ordinal: u32) -> Self {
        let VerifiedVectors {
            digest,
            sidecar,
            generation,
            rows,
            codes,
            scales,
            occurrence_ids,
            tombstones,
        } = member;
        Self {
            precedence: Precedence {
                base_epoch: epoch,
                delta_ordinal: ordinal,
            },
            checkpoint: sidecar.checkpoint(),
            digest,
            sidecar,
            scales,
            occurrence_ids,
            tombstones,
            rows,
            codes,
            _generation: generation,
        }
    }

    /// In row order; the bound every positioned read is checked against.
    pub fn occurrence_ids(&self) -> &[String] {
        &self.occurrence_ids
    }

    fn dimension(&self) -> usize {
        self.sidecar.vector_dimension as usize
    }

    /// The `width` bytes of row `index` in `file`, whose rows follow `header` bytes. The index is checked against the identifier count, and verification proved the artifact holds exactly that many rows, so no read can leave the rows the sidecar declares; a file cut short afterwards fails the read instead.
    fn read_row_bytes(
        &self,
        file: &File,
        header: usize,
        width: usize,
        index: usize,
    ) -> Result<Vec<u8>, RowFault> {
        if index >= self.occurrence_ids.len() {
            return Err(RowFault::Unavailable(format!(
                "row {index} is past the {} rows the layer declares",
                self.occurrence_ids.len()
            )));
        }
        let offset = header as u64 + index as u64 * width as u64;
        let mut bytes = vec![0u8; width];
        file.read_exact_at(&mut bytes, offset)
            .map_err(|error| RowFault::Unavailable(format!("row {index}: {error}")))?;
        Ok(bytes)
    }

    /// The int8 codes of row `index`; they score with this layer's scales and no other's.
    ///
    /// # Errors
    ///
    /// A row past the layer's declared rows, a short read, or bytes the recipe refuses.
    pub fn codes(&self, index: usize) -> Result<Vec<i8>, RowFault> {
        let bytes = self.read_row_bytes(&self.codes, 0, self.dimension(), index)?;
        scalar::decode_codes(&bytes, self.sidecar.vector_dimension)
            .map_err(|rejection| RowFault::Unavailable(rejection.to_string()))
    }
}

impl RowAccess for PinnedLayer {
    fn row_count(&self) -> usize {
        self.occurrence_ids.len()
    }

    fn row(&self, index: usize) -> Result<Vec<f32>, RowFault> {
        let bytes = self.read_row_bytes(
            &self.rows,
            ARTIFACT_HEADER_BYTES,
            self.dimension() * 4,
            index,
        )?;
        codec::decode_shape(&bytes, self.sidecar.vector_dimension).map_err(RowFault::Rejected)
    }
}

/// A complete verified composition held open: the record and every member pinned, every artifact open, and the resident bytes charged until the view is dropped. Acquire a view once and share it; acquisition hashes every member, holding its pins but not the lifecycle lock while it does.
pub struct PinnedVectors {
    digest: String,
    layout: RowLayout,
    /// The base first, then the deltas in application order.
    layers: Vec<PinnedLayer>,
    _record: ValidatedGeneration,
    _residency: ByteCharge,
}

impl std::fmt::Debug for PinnedVectors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PinnedVectors")
            .field("digest", &self.digest)
            .field("layers", &self.layers)
            .finish_non_exhaustive()
    }
}

impl PinnedVectors {
    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn layout(&self) -> RowLayout {
        self.layout
    }

    /// ```compile_fail,E0616
    /// use daemon::vector_reader::PinnedVectors;
    /// fn detach(view: &mut PinnedVectors) {
    ///     let _layers = std::mem::take(&mut view.layers);
    /// }
    /// ```
    pub fn layers(&self) -> &[PinnedLayer] {
        &self.layers
    }

    pub fn members(&self) -> Vec<String> {
        self.layers
            .iter()
            .map(|layer| layer.digest.clone())
            .collect()
    }

    fn resolver_layers(&self) -> Vec<Layer<'_>> {
        self.layers
            .iter()
            .map(|layer| Layer {
                precedence: layer.precedence,
                checkpoint: &layer.checkpoint,
                occurrence_ids: &layer.occurrence_ids,
                rows: layer,
                tombstones: &layer.tombstones,
            })
            .collect()
    }
}

impl SelectorState {
    /// Whether `current` still says what this state observed about `digest`.
    fn still_holds(&self, current: &CurrentProfile, digest: &str) -> bool {
        match (self, current) {
            (Self::Current, CurrentProfile::Current(now)) => now == digest,
            (Self::Stale(was), CurrentProfile::Current(now)) => now == was,
            (Self::Absent, CurrentProfile::Absent) => true,
            _ => false,
        }
    }
}

/// The lifecycle's shared lock, or the refusal a mutator that outlasted the bounded wait leaves.
fn protect(root: Option<&Path>) -> Result<LifecycleTransactionLock, AcquireRefusal> {
    LifecycleTransactionLock::acquire_shared(root)
        .map_err(|error| AcquireRefusal::Unprotected(error.to_string()))?
        .ok_or_else(|| {
            AcquireRefusal::Unprotected(
                "no coordination root, or a mutator outlasted the wait".to_owned(),
            )
        })
}

/// One candidate secured before its files are hashed: the record and every member the record's members file names, each pinned by a manifest read, and the resident bytes of the members charged.
struct Secured {
    pins: Vec<PinnedGeneration>,
    residency: ByteCharge,
}

/// Pins `candidate` and its members under `protection`, then charges the members' resident bytes. `None` when the record or a member cannot be pinned, does not name its members, or names more members than `max_deltas` admits: the candidate would not verify either, and the next one is tried without pinning what it lists.
///
/// # Errors
///
/// A residency budget that cannot hold the members' resident bytes; no other candidate can change that.
fn secure(
    store: &GenerationStore,
    candidate: &Candidate,
    max_deltas: NonZeroUsize,
    residency: &ByteBudget,
    protection: LifecycleTransactionLock,
) -> Result<Option<Secured>, AcquireRefusal> {
    let Ok(record) = store.pin(&candidate.digest) else {
        return Ok(None);
    };
    let Ok(members) = record.members() else {
        return Ok(None);
    };
    // The base and at most `max_deltas` deltas; more would be refused at verification after every listed member was pinned under the lock.
    if members.len().saturating_sub(1) > max_deltas.get() {
        return Ok(None);
    }
    let mut pins = Vec::with_capacity(members.len() + 1);
    pins.push(record);
    let mut resident = 0u64;
    for member in &members {
        let Ok(pin) = store.pin(member) else {
            return Ok(None);
        };
        resident = resident.saturating_add(vector_generation::resident_bytes(&pin.manifest));
        pins.push(pin);
    }
    drop(protection);
    let resident = usize::try_from(resident).unwrap_or(usize::MAX);
    let residency = residency
        .try_charge(resident)
        .ok_or(AcquireRefusal::Residency { bytes: resident })?;
    Ok(Some(Secured { pins, residency }))
}

/// Takes the lifecycle's shared lock only around manifest reads: once to list the candidates and pin one, once more to re-read the selector before handoff. Hashing every member runs between them under the pins alone, so a mutator waits on this reader for no longer than a pin; one that holds the exclusive lock past the bounded wait refuses the reader instead. Run it on a blocking thread.
///
/// # Errors
///
/// No shared protection within the wait, no verifying composition, a store refusal, a residency budget that cannot hold the view's resident bytes, or a selector that moved while the view was being taken. Nothing is handed out on any of them.
pub fn acquire(
    root: Option<&Path>,
    expected: &ExpectedVectors<'_>,
    bounds: ReaderBounds,
    residency: &ByteBudget,
    observe: &mut dyn FnMut(AcquireEvent),
) -> Result<Arc<PinnedVectors>, AcquireRefusal> {
    let store = GenerationStore::open(root)?;
    let mut candidates: Option<VecDeque<Candidate>> = None;
    let mut examined = 0;
    loop {
        let protection = protect(root)?;
        let current = store.read_vector_current()?;
        let queue = match &mut candidates {
            Some(queue) => queue,
            None => candidates.insert(
                vector_composition::candidates(&store, &current, bounds.recovery_bound)?.into(),
            ),
        };
        let Some(candidate) = queue.pop_front() else {
            return Err(Unavailable::NoCompatibleTarget { examined }.into());
        };
        examined += 1;
        if !candidate.selector.still_holds(&current, &candidate.digest) {
            return Err(AcquireRefusal::SelectorMoved {
                observed: candidate.selector,
                current,
            });
        }
        let Some(secured) = secure(&store, &candidate, bounds.max_deltas, residency, protection)?
        else {
            continue;
        };
        observe(AcquireEvent::BeforeVerification);
        let Ok(verified) = vector_composition::verify_composition(
            &store,
            &candidate.digest,
            expected,
            bounds.max_deltas,
            bounds.max_member_bytes,
        ) else {
            continue;
        };
        return hand_off(
            root,
            &store,
            expected,
            candidate.selector,
            verified,
            secured,
            observe,
        );
    }
}

/// Moves the pins onto the descriptors verification hashed through, builds the layers, and re-reads the selector under the shared lock before the view is handed out.
fn hand_off(
    root: Option<&Path>,
    store: &GenerationStore,
    expected: &ExpectedVectors<'_>,
    selector: SelectorState,
    verified: VerifiedComposition,
    secured: Secured,
    observe: &mut dyn FnMut(AcquireEvent),
) -> Result<Arc<PinnedVectors>, AcquireRefusal> {
    let VerifiedComposition {
        digest,
        composition,
        record,
        base,
        deltas,
    } = verified;
    // The validated descriptors take their own shared locks before the manifest-read pins go, so no instant leaves a member unpinned.
    record.pin()?;
    let members: Vec<VerifiedVectors> = std::iter::once(base).chain(deltas).collect();
    for member in &members {
        member.generation.pin()?;
    }
    let Secured { pins, residency } = secured;
    drop(pins);
    let epoch = composition.generation_epoch;
    let last = members.len() - 1;
    let mut layers = Vec::with_capacity(members.len());
    for (ordinal, member) in members.into_iter().enumerate() {
        if ordinal == last {
            observe(AcquireEvent::BeforeLastLayer);
        }
        layers.push(PinnedLayer::from_member(member, epoch, ordinal as u32));
    }
    observe(AcquireEvent::BeforeSelectorRecheck);
    let current = {
        let _protection = protect(root)?;
        store.read_vector_current()?
    };
    if !selector.still_holds(&current, &digest) {
        return Err(AcquireRefusal::SelectorMoved {
            observed: selector,
            current,
        });
    }
    Ok(Arc::new(PinnedVectors {
        digest,
        layout: RowLayout {
            dimension: composition.vector_dimension,
            metric: expected.metric,
            unit_norm_tolerance: composition.unit_norm_tolerance,
        },
        layers,
        _record: record,
        _residency: residency,
    }))
}

/// Not `Debug`: the query row is embedding content.
pub struct RankRequest<'a> {
    /// What the caller expects the view to be; every identity field of every layer is rechecked at handoff.
    pub expected: &'a ExpectedVectors<'a>,
    pub query: &'a [f32],
    pub authority: Authority<'a>,
    pub bounds: OracleBounds,
    /// Rows and tombstones the layers may carry together.
    pub max_entries: NonZeroUsize,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RankRefusal {
    #[error("the view's {field} is not the request's")]
    Identity { field: &'static str },
    #[error("one page of rows needs {bytes} bytes of scratch; the budget refused them")]
    Scratch { bytes: usize },
    #[error(transparent)]
    Layered(#[from] LayeredRefusal),
}

/// Ranks `request` over the view's layers inside the caller's read transaction. Every layer's sidecar is checked against the request's expectation first, and one page of row scratch is charged for the walk's duration.
/// Run it through the request's blocking seam with the `Arc<PinnedVectors>` moved into the work, so the view outlives a caller that drops its future, cancels, or times out.
///
/// # Errors
///
/// An identity the request does not share, scratch the budget cannot hold, or a layered refusal.
pub fn rank(
    view: &PinnedVectors,
    conn: &GuardedConn<'_>,
    kernel: &KernelStore,
    request: &RankRequest<'_>,
    budget: &EvalBudget,
    scratch: &ByteBudget,
) -> Result<LayeredRanking, RankRefusal> {
    let expected = request.expected;
    for layer in &view.layers {
        vector_generation::check_identity(&layer.sidecar, expected).map_err(|refusal| {
            RankRefusal::Identity {
                field: match refusal {
                    VectorRefusal::Identity { field } => field,
                    _ => "identity",
                },
            }
        })?;
    }
    // The page's decoded rows plus the one raw row being read.
    let bytes = request
        .bounds
        .page_rows
        .min(request.bounds.max_rows)
        .get()
        .checked_add(1)
        .and_then(|rows| rows.checked_mul(view.layout.dimension as usize))
        .and_then(|elements| elements.checked_mul(size_of::<f32>()))
        .ok_or(RankRefusal::Scratch { bytes: usize::MAX })?;
    let _scratch = scratch
        .try_charge(bytes)
        .ok_or(RankRefusal::Scratch { bytes })?;
    let layers = view.resolver_layers();
    let query = LayeredQuery {
        generation: expected.generation,
        metric: view.layout.metric,
        unit_norm_tolerance: view.layout.unit_norm_tolerance,
        query: request.query,
        authority: request.authority,
        bounds: request.bounds,
        layers: &layers,
        max_entries: request.max_entries,
    };
    Ok(rank_layers(conn, kernel, &query, budget)?)
}
