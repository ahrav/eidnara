//! Reads a vector composition through pinned files. Acquisition observes the selector under the lifecycle's shared protection, recovers and verifies the composition, pins the record and every member with shared locks, keeps the row and code artifacts open on the descriptors verification hashed them through, charges the bytes the view keeps resident, and re-reads the selector before the protection is released; a failure anywhere returns nothing, and the pins, descriptors, and charge go with it.
//! Ranking reads only the rows the resolver names, each by its offset in the row artifact into scratch charged for one page, and decodes them through the original-row codec; the int8 codes are read the same way under the layer's own scales. Positioned reads after acquisition are not re-hashed: pins protect lifetime, and same-user writes after verification are outside the cooperative-file threat model.
//! A caller runs the ranking through the request's blocking seam with the view moved into the work, so the view lives until the physical read returns whatever happens to the caller.

use std::fs::File;
use std::num::NonZeroUsize;
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::sync::Arc;

use host_runtime::generation::{
    CurrentProfile, GenerationError, GenerationStore, ValidatedGeneration,
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

use crate::vector_composition::{self, SelectorState, Unavailable, VerifiedComposition};
use crate::vector_generation::{
    self, ExpectedVectors, ROW_IDS_FILE, SCALES_FILE, SIDECAR_FILE, TOMBSTONES_FILE, VectorRefusal,
    VectorSidecar, VerifiedVectors,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReaderBounds {
    pub max_deltas: NonZeroUsize,
    /// Compositions recovery may fully verify when the selected one does not.
    pub recovery_bound: NonZeroUsize,
}

/// Where acquisition stands; a test may hold or mutate the store here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquireEvent {
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

/// A complete verified composition held open: the record and every member pinned, every artifact open, and the resident bytes charged until the view is dropped. Acquire a view once and share it; acquisition hashes every member and holds the lifecycle's shared lock while it does.
pub struct PinnedVectors {
    pub digest: String,
    pub layout: RowLayout,
    /// The base first, then the deltas in application order.
    pub layers: Vec<PinnedLayer>,
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

/// The files a view keeps decoded in memory for its lifetime, charged at their manifest-declared sizes; rows and codes stay on disk and are read by offset.
const RESIDENT_FILES: [&str; 4] = [ROW_IDS_FILE, TOMBSTONES_FILE, SCALES_FILE, SIDECAR_FILE];

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

/// Blocks on the lifecycle lock and hashes every member; run it on a blocking thread.
///
/// # Errors
///
/// No shared protection, no verifying composition, a store refusal, a residency budget that cannot hold the view's resident bytes, or a selector that moved while the view was being taken. Nothing is handed out on any of them.
pub fn acquire(
    root: Option<&Path>,
    expected: &ExpectedVectors<'_>,
    bounds: ReaderBounds,
    residency: &ByteBudget,
    observe: &mut dyn FnMut(AcquireEvent),
) -> Result<Arc<PinnedVectors>, AcquireRefusal> {
    let protection = LifecycleTransactionLock::acquire_shared(root)
        .map_err(|error| AcquireRefusal::Unprotected(error.to_string()))?
        .ok_or_else(|| {
            AcquireRefusal::Unprotected(
                "no coordination root, or a mutator outlasted the wait".to_owned(),
            )
        })?;
    let store = GenerationStore::open(root)?;
    let recovered = vector_composition::recover(
        &store,
        &protection,
        expected,
        bounds.max_deltas,
        bounds.recovery_bound,
    )?;
    let VerifiedComposition {
        digest,
        composition,
        record,
        base,
        deltas,
    } = recovered.composition;
    record.pin()?;
    let members: Vec<VerifiedVectors> = std::iter::once(base).chain(deltas).collect();
    for member in &members {
        member.generation.pin()?;
    }
    let resident: u64 = members
        .iter()
        .flat_map(|member| member.generation.manifest.files.iter())
        .filter(|file| RESIDENT_FILES.contains(&file.path.as_str()))
        .map(|file| file.size)
        .sum();
    let resident = usize::try_from(resident).unwrap_or(usize::MAX);
    let residency = residency
        .try_charge(resident)
        .ok_or(AcquireRefusal::Residency { bytes: resident })?;
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
    let current = store.read_vector_current()?;
    if !recovered.selector.still_holds(&current, &digest) {
        return Err(AcquireRefusal::SelectorMoved {
            observed: recovered.selector,
            current,
        });
    }
    drop(protection);
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
    let bytes = (request.bounds.page_rows.get() + 1)
        .checked_mul(view.layout.dimension as usize * 4)
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
