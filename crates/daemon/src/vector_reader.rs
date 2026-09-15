//! Reads a vector composition through pinned files. Acquisition observes the selector under the lifecycle's shared protection, recovers and verifies the composition, pins the record and every member with shared locks, opens every required file through the retained directory descriptors, charges the bytes the view keeps resident, and re-reads the selector before the protection is released; a failure anywhere returns nothing, and the pins, descriptors, and charge go with it.
//! Ranking reads only the rows the resolver names, each by its offset in the row artifact into scratch charged for one page, and decodes them through the original-row codec; the int8 codes are read the same way under the layer's own scales.
//! A ranking run on a worker owns the view until the physical work returns, whatever happens to the caller, and the output it returns keeps the view until it is dropped.

use std::num::NonZeroUsize;
use std::os::fd::OwnedFd;
use std::path::Path;
use std::sync::Arc;

use host_runtime::generation::{
    CurrentProfile, GenerationError, GenerationStore, ValidatedGeneration,
};
use host_runtime::lifecycle::LifecycleTransactionLock;
use host_runtime::wire::{ByteBudget, ByteCharge};
use kernel::applicability::EvalBudget;
use kernel::{ArtifactDestination, KernelStore, ProjectScope};
use retrieval::batch::{ProjectionCheckpoint, VectorGeneration};
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
    CODES_FILE, ExpectedVectors, ROW_IDS_FILE, ROWS_FILE, SCALES_FILE, SIDECAR_FILE,
    TOMBSTONES_FILE, VectorIdentity, VectorSidecar, VerifiedVectors,
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
    /// Every pin is held and every required file but the last is open.
    BeforeLastOpen,
    /// Every file is open; the selector is about to be re-read.
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
    #[error("the layer {digest} does not agree with itself about {detail}")]
    Layer {
        digest: String,
        detail: &'static str,
    },
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

/// One member's open files and resident tables; the layer keeps its shared lock through `_generation`.
pub struct PinnedLayer {
    pub digest: String,
    pub sidecar: VectorSidecar,
    pub precedence: Precedence,
    pub checkpoint: ProjectionCheckpoint,
    pub occurrence_ids: Vec<String>,
    pub tombstones: Vec<String>,
    pub scales: Scales,
    rows: OwnedFd,
    codes: OwnedFd,
    dimension: u32,
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
    fn row_bytes(&self) -> usize {
        self.dimension as usize * 4
    }

    /// The bytes of row `index` in `file`, whose rows follow `header` bytes; the offset is checked against the identifier count so no read can leave the rows the sidecar declares.
    fn read_row_bytes(
        &self,
        file: &OwnedFd,
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
        let mut filled = 0;
        while filled < width {
            let read = rustix::io::pread(file, &mut bytes[filled..], offset + filled as u64)
                .map_err(|error| RowFault::Unavailable(format!("row {index}: {error}")))?;
            if read == 0 {
                return Err(RowFault::Unavailable(format!(
                    "row {index} is truncated after {filled} of {width} bytes"
                )));
            }
            filled += read;
        }
        Ok(bytes)
    }

    /// The int8 codes of row `index`, decoded under the layer's own dimension; they score with this layer's scales and no other's.
    ///
    /// # Errors
    ///
    /// A row past the layer's declared rows, a short read, or bytes the recipe refuses.
    pub fn codes(&self, index: usize) -> Result<Vec<i8>, RowFault> {
        let bytes = self.read_row_bytes(&self.codes, 0, self.dimension as usize, index)?;
        scalar::decode_codes(&bytes, self.dimension)
            .map_err(|rejection| RowFault::Unavailable(rejection.to_string()))
    }
}

impl RowAccess for PinnedLayer {
    fn len(&self) -> usize {
        self.occurrence_ids.len()
    }

    fn row(&self, index: usize) -> Result<Vec<f32>, RowFault> {
        let bytes =
            self.read_row_bytes(&self.rows, ARTIFACT_HEADER_BYTES, self.row_bytes(), index)?;
        codec::decode_shape(&bytes, self.dimension).map_err(RowFault::Rejected)
    }
}

/// A complete verified composition held open: the record and every member pinned, every required file open, and the resident bytes charged until the view is dropped.
pub struct PinnedVectors {
    pub digest: String,
    pub sequence: u64,
    pub selector: SelectorState,
    pub identity: VectorIdentity,
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
            .field("sequence", &self.sequence)
            .field("selector", &self.selector)
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

/// # Errors
///
/// No shared protection, no verifying composition, a store refusal, a residency budget that cannot hold the view's resident bytes, a member that disagrees with itself, or a selector that moved while the view was being opened. Nothing is handed out on any of them.
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
        base,
        deltas,
    } = recovered.composition;
    let record = store.validate(&digest)?;
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
    let mut layers = Vec::with_capacity(members.len());
    let last = members.len() - 1;
    for (ordinal, member) in members.into_iter().enumerate() {
        layers.push(open_layer(
            member,
            epoch,
            ordinal as u32,
            (ordinal == last).then_some(&mut *observe),
        )?);
    }
    observe(AcquireEvent::BeforeSelectorRecheck);
    let current = store.read_vector_current()?;
    let unchanged = match (&recovered.selector, &current) {
        (SelectorState::Current, CurrentProfile::Current(now)) => *now == digest,
        (SelectorState::Stale(was), CurrentProfile::Current(now)) => now == was,
        (SelectorState::Absent, CurrentProfile::Absent) => true,
        _ => false,
    };
    if !unchanged {
        return Err(AcquireRefusal::SelectorMoved {
            observed: recovered.selector,
            current,
        });
    }
    drop(protection);
    Ok(Arc::new(PinnedVectors {
        digest,
        sequence: composition.sequence,
        selector: recovered.selector,
        identity: composition.identity(),
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

/// The files a view keeps decoded in memory for its lifetime; rows and codes stay on disk and are read by offset.
const RESIDENT_FILES: [&str; 4] = [ROW_IDS_FILE, TOMBSTONES_FILE, SCALES_FILE, SIDECAR_FILE];

/// Opens one verified member's files through its retained directory descriptor, each rehashed on open, and checks the row artifact's length against the identifiers so every offset the reader will compute lies inside it. `observe`, given for the final member, runs before its last open.
fn open_layer(
    member: VerifiedVectors,
    epoch: u64,
    ordinal: u32,
    observe: Option<&mut dyn FnMut(AcquireEvent)>,
) -> Result<PinnedLayer, AcquireRefusal> {
    let VerifiedVectors {
        digest,
        sidecar,
        generation,
    } = member;
    let layer_fault = |detail| AcquireRefusal::Layer {
        digest: digest.clone(),
        detail,
    };
    let occurrence_ids: Vec<String> =
        serde_json::from_slice(&generation.read_verified_file(ROW_IDS_FILE)?)
            .map_err(|_| layer_fault("identifiers"))?;
    let tombstones: Vec<String> =
        serde_json::from_slice(&generation.read_verified_file(TOMBSTONES_FILE)?)
            .map_err(|_| layer_fault("tombstones"))?;
    let scales = Scales::decode(
        &generation.read_verified_file(SCALES_FILE)?,
        sidecar.vector_dimension,
    )
    .map_err(|_| layer_fault("scales"))?;
    let rows = generation.open_verified_file(ROWS_FILE)?;
    let declared = |path: &str| {
        generation
            .manifest
            .files
            .iter()
            .find(|file| file.path == path)
            .map_or(0, |file| file.size)
    };
    let row_bytes = u64::from(sidecar.vector_dimension) * 4;
    let count = occurrence_ids.len() as u64;
    if declared(ROWS_FILE) != ARTIFACT_HEADER_BYTES as u64 + count * row_bytes {
        return Err(layer_fault("row artifact length"));
    }
    if declared(CODES_FILE) != count * u64::from(sidecar.vector_dimension) {
        return Err(layer_fault("codes length"));
    }
    if let Some(observe) = observe {
        observe(AcquireEvent::BeforeLastOpen);
    }
    let codes = generation.open_verified_file(CODES_FILE)?;
    Ok(PinnedLayer {
        precedence: Precedence {
            base_epoch: epoch,
            delta_ordinal: ordinal,
        },
        checkpoint: sidecar.checkpoint(),
        dimension: sidecar.vector_dimension,
        digest,
        sidecar,
        occurrence_ids,
        tombstones,
        scales,
        rows,
        codes,
        _generation: generation,
    })
}

/// Not `Debug`: the query row is embedding content.
pub struct RankRequest<'a> {
    pub generation: &'a VectorGeneration,
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
    #[error("the projection could not be read: {0}")]
    Projection(String),
}

/// Ranks `request` over the view's layers inside the caller's read transaction. The view's identity is checked against the request's generation first, and one page of row scratch is charged for the walk's duration.
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
    let generation = request.generation;
    let field = if view.identity.embedding_model != generation.embedding_model {
        Some("embedding_model")
    } else if view.identity.tokenizer_fingerprint != generation.tokenizer_fingerprint {
        Some("tokenizer_fingerprint")
    } else if view.identity.vector_dimension != generation.vector_dimension {
        Some("vector_dimension")
    } else if view.identity.generation_epoch != generation.generation_epoch {
        Some("generation_epoch")
    } else if view
        .layers
        .iter()
        .any(|layer| layer.sidecar.generation_id != generation.generation_id)
    {
        Some("generation_id")
    } else {
        None
    };
    if let Some(field) = field {
        return Err(RankRefusal::Identity { field });
    }
    let bytes = request.bounds.page_rows.get() * view.layout.dimension as usize * 4;
    let _scratch = scratch
        .try_charge(bytes)
        .ok_or(RankRefusal::Scratch { bytes })?;
    let layers = view.resolver_layers();
    let query = LayeredQuery {
        generation,
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

/// A ranking that keeps its view: the pins, descriptors, and resident charge live until the output is dropped.
#[derive(Debug)]
pub struct RankedView {
    pub ranking: LayeredRanking,
    pub view: Arc<PinnedVectors>,
}

/// Everything a worker needs to own outright, since the caller may be gone before the work returns.
pub struct WorkerRequest {
    pub generation: VectorGeneration,
    pub query: Vec<f32>,
    pub project: ProjectScope,
    pub destination: ArtifactDestination,
    pub bounds: OracleBounds,
    pub max_entries: NonZeroUsize,
}

/// Runs [`rank`] on a blocking worker that owns `view`, `kernel`, and `scratch` until the physical read returns; the caller dropping the returned handle does not stop the work or release anything it holds. `with_conn` supplies the projection read transaction the walk runs inside.
pub fn rank_on_worker<R>(
    view: Arc<PinnedVectors>,
    kernel: Arc<KernelStore>,
    with_conn: R,
    request: WorkerRequest,
    budget: EvalBudget,
    scratch: ByteBudget,
) -> tokio::task::JoinHandle<Result<RankedView, RankRefusal>>
where
    R: FnOnce(&mut dyn FnMut(&GuardedConn<'_>)) -> Result<(), String> + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let mut outcome = None;
        with_conn(&mut |conn| {
            let request = RankRequest {
                generation: &request.generation,
                query: &request.query,
                authority: Authority {
                    project: &request.project,
                    destination: request.destination,
                },
                bounds: request.bounds,
                max_entries: request.max_entries,
            };
            outcome = Some(rank(&view, conn, &kernel, &request, &budget, &scratch));
        })
        .map_err(RankRefusal::Projection)?;
        let ranking = outcome.ok_or_else(|| {
            RankRefusal::Projection("the projection read never ran the walk".to_owned())
        })??;
        Ok(RankedView { ranking, view })
    })
}
