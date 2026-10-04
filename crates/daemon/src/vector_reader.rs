//! Reads a vector composition through pinned files. Acquisition takes the lifecycle's shared protection only around manifest reads: it observes the selector, pins the candidate record and every member it names with shared locks, and reserves the bytes the view keeps resident and records the bytes it pins on disk in the ledger, then releases the protection before verification hashes the members under the pins alone; it keeps the row and code artifacts open on the descriptors verification hashed them through and re-reads the selector under the protection once more before handoff. A failure anywhere returns nothing, and the pins, descriptors, and reservations go with it.
//! Ranking reads only the rows the resolver names, each by its offset in the row artifact into scratch charged for one page, and decodes them through the original-row codec. The candidate scan reads a layer's int8 codes through that layer's `CodeWindow` and scores only the rows the resolver names, under the layer's own scales. Positioned reads after acquisition rely on the hashes verification checked: pins protect lifetime, and the cooperative-file threat model assumes same-user files stay unchanged after verification.
//! A caller runs the ranking through the request's blocking seam with the view moved into the work, so the view lives until the physical read returns whatever happens to the caller.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::fs::File;
use std::num::NonZeroUsize;
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use host_runtime::generation::{
    CurrentProfile, GenerationError, GenerationStore, PinnedGeneration, ValidatedGeneration,
};
use host_runtime::lifecycle::LifecycleTransactionLock;
use kernel::KernelStore;
use kernel::applicability::EvalBudget;
use retrieval::batch::ProjectionCheckpoint;
use retrieval::dense::codec::{self, ARTIFACT_HEADER_BYTES, RowLayout};
use retrieval::dense::scalar::{self, Scales};
use retrieval::dense::{
    CandidateCapacity, CandidatePool, CandidateQuery, CandidateRefusal, CodeAccess, Layer,
    LayerCodes, LayeredQuery, LayeredRanking, LayeredRefusal, OracleBounds, OracleRefusal,
    Precedence, RescoreRefusal, Rescored, RowAccess, RowFault, RowRejection, ScanBounds, Window,
    WinnerRow, rank_layers, rescore_pool, select_candidates_observed,
};
use retrieval::eligibility::Authority;
use storage::GuardedConn;

use crate::projection_gates::Admission;
use crate::vector_admission::{self, Ledger, Pinned, Reservation, ResourceClass};
use crate::vector_composition::{self, Candidate, SelectorState, Unavailable, VerifiedComposition};
use crate::vector_generation::{
    self, ExpectedVectors, VectorRefusal, VectorSidecar, VerifiedVectors,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReaderBounds {
    pub max_deltas: NonZeroUsize,
    /// Heap bytes verification of one member may hold at once, as [`vector_generation::verification_bytes`] counts them.
    pub max_member_bytes: u64,
    /// Compositions recovery may fully verify when the selected one does not.
    pub recovery_bound: NonZeroUsize,
}

/// Where acquisition stands; a test may hold or mutate the store here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquireEvent {
    /// The candidate's record and members are pinned, its resident bytes are reserved, and the transaction lock is released; verification is about to hash the members.
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
    #[error("the ledger refused the view's bytes: {0}")]
    Reservation(#[from] vector_admission::Refusal),
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

    pub fn tombstones(&self) -> &[String] {
        &self.tombstones
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
        self.check_declared(index)?;
        let offset = header as u64 + index as u64 * width as u64;
        let mut bytes = vec![0u8; width];
        file.read_exact_at(&mut bytes, offset)
            .map_err(|error| read_fault(index, &error))?;
        Ok(bytes)
    }

    fn check_declared(&self, index: usize) -> Result<(), RowFault> {
        check_declared(index, self.occurrence_ids.len())
    }

    /// The original row `index`, read through `bytes` into `into`; both buffers are reused, and `into` holds exactly the decoded row on success.
    fn row_into(
        &self,
        index: usize,
        bytes: &mut Vec<u8>,
        into: &mut Vec<f32>,
    ) -> Result<(), RowFault> {
        self.check_declared(index)?;
        let width = self.dimension() * 4;
        let offset = ARTIFACT_HEADER_BYTES as u64 + index as u64 * width as u64;
        bytes.resize(width, 0);
        self.rows
            .read_exact_at(bytes, offset)
            .map_err(|error| read_fault(index, &error))?;
        let dimension = self.sidecar.vector_dimension;
        codec::decode_length_into(bytes, dimension, into)
            .and_then(|()| codec::validate_shape(into, dimension))
            .map_err(RowFault::Rejected)
    }

    /// The int8 codes of row `index`; they score with this layer's scales and no other's.
    ///
    /// # Errors
    ///
    /// A row past the layer's declared rows, a short read, or bytes the recipe refuses.
    pub fn codes(&self, index: usize) -> Result<Vec<i8>, RowFault> {
        let bytes = self.read_row_bytes(&self.codes, 0, self.dimension(), index)?;
        scalar::decode_codes(&bytes, self.sidecar.vector_dimension)
            .map_err(|rejection| RowFault::Missing(rejection.to_string()))
    }
}

/// A row at or past `declared` is not where its layer says it is.
fn check_declared(index: usize, declared: usize) -> Result<(), RowFault> {
    if index < declared {
        return Ok(());
    }
    Err(RowFault::Missing(format!(
        "row {index} is past the {declared} rows the layer declares"
    )))
}

/// The byte budget of one layer's scan window; a window holds one row even when that row is wider.
const CODE_WINDOW_BYTES: usize = 64 * 1024;

/// One layer's codes for one candidate scan: `declared` rows of `width` bytes each, from offset 0 of `codes`. A row outside the held window refills the window with the run of declared rows that starts at that row, so later rows inside the run are served without another read.
struct CodeWindow<'a> {
    codes: &'a File,
    width: usize,
    declared: usize,
    /// The per-fill row limit: what [`CODE_WINDOW_BYTES`] holds, one row when a row is wider, capped at the layer's declared rows. A failed window read drops it to one row for the rest of the scan.
    capacity: Cell<usize>,
    held: RefCell<HeldCodes>,
}

/// Raw codes of rows `first..first + rows`.
#[derive(Default)]
struct HeldCodes {
    first: usize,
    rows: usize,
    bytes: Vec<u8>,
}

impl<'a> CodeWindow<'a> {
    fn new(codes: &'a File, width: usize, declared: usize) -> Self {
        Self {
            codes,
            width,
            declared,
            capacity: Cell::new(window_rows(width, declared)),
            held: RefCell::default(),
        }
    }

    fn of(layer: &'a PinnedLayer) -> Self {
        Self::new(&layer.codes, layer.dimension(), layer.occurrence_ids.len())
    }

    /// A window that fails to read whole falls back to the requested row alone, so a file cut short or a failed sector past that row classifies exactly as a single-row read does. Later fills read one row each, so the failing span is not read again for every row before it.
    fn fill(&self, held: &mut HeldCodes, index: usize) -> Result<(), RowFault> {
        check_declared(index, self.declared)?;
        let width = self.width;
        let rows = self.capacity.get().min(self.declared - index);
        let offset = index as u64 * width as u64;
        held.rows = 0;
        held.bytes.resize(rows * width, 0);
        let read = self.codes.read_exact_at(&mut held.bytes, offset);
        let rows = match read {
            Ok(()) => rows,
            Err(error) if rows == 1 => return Err(read_fault(index, &error)),
            Err(_) => {
                self.capacity.set(1);
                held.bytes.truncate(width);
                self.codes
                    .read_exact_at(&mut held.bytes, offset)
                    .map_err(|error| read_fault(index, &error))?;
                1
            }
        };
        held.first = index;
        held.rows = rows;
        Ok(())
    }
}

impl CodeAccess for CodeWindow<'_> {
    fn row_count(&self) -> usize {
        self.declared
    }

    /// The candidate scan checks the dimension and the reserved code.
    fn codes_into(&self, index: usize, into: &mut Vec<i8>) -> Result<(), RowFault> {
        let mut held = self.held.borrow_mut();
        if !(held.first..held.first + held.rows).contains(&index) {
            self.fill(&mut held, index)?;
        }
        let width = self.width;
        let start = (index - held.first) * width;
        into.clear();
        into.extend(
            held.bytes[start..start + width]
                .iter()
                .map(|byte| *byte as i8),
        );
        Ok(())
    }
}

fn window_rows(width: usize, declared: usize) -> usize {
    (CODE_WINDOW_BYTES / width.max(1)).max(1).min(declared)
}

/// Scan scratch: one block of decoded codes per layer, held through the scan, and every layer's window of raw codes.
fn scan_scratch_bytes(view: &PinnedVectors) -> u64 {
    let dimension = u64::from(view.layout.dimension);
    let windows: u64 = view
        .layers
        .iter()
        .map(|layer| window_rows(layer.dimension(), layer.occurrence_ids.len()) as u64)
        .sum();
    (retrieval::dense::BLOCK_ROWS as u64)
        .saturating_mul(view.layers.len() as u64)
        .saturating_add(windows)
        .saturating_mul(dimension)
}

impl RowAccess for PinnedLayer {
    fn row_count(&self) -> usize {
        self.occurrence_ids.len()
    }

    fn row(&self, index: usize) -> Result<Vec<f32>, RowFault> {
        let mut row = Vec::new();
        self.row_into(index, &mut Vec::new(), &mut row)?;
        Ok(row)
    }
}

/// A complete verified composition held open: the record and every member pinned, every artifact open, and the resident and pinned bytes reserved until the view is dropped. Acquire a view once and share it; acquisition hashes every member, holding its pins but not the lifecycle lock while it does.
pub struct PinnedVectors {
    digest: String,
    layout: RowLayout,
    /// The base first, then the deltas in application order.
    layers: Vec<PinnedLayer>,
    _record: ValidatedGeneration,
    resident: Reservation,
    pinned: Pinned,
    /// Set when a read found this view's rows or codes missing or corrupt; every later ranking over this view refuses. The flag belongs to this view alone: another acquisition of the same composition re-verifies its members and refuses one whose files no longer hash.
    quarantined: AtomicBool,
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

    /// Bytes on disk the view pins: the record and every member's files.
    pub fn pinned_bytes(&self) -> u64 {
        self.pinned.bytes()
    }

    fn quarantine(&self) {
        self.quarantined.store(true, Ordering::Relaxed);
    }

    pub fn is_quarantined(&self) -> bool {
        self.quarantined.load(Ordering::Relaxed)
    }

    pub fn members(&self) -> Vec<String> {
        self.layers
            .iter()
            .map(|layer| layer.digest.clone())
            .collect()
    }

    /// The layers in the resolver's shape, the base as ordinal zero and each delta by its position.
    pub fn resolver_layers(&self) -> Vec<Layer<'_>> {
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

/// One candidate secured before its files are hashed: the record and every member the record's members file names, each pinned by a manifest read, the resident bytes of the members reserved, and every pinned byte recorded.
struct Secured {
    pins: Vec<PinnedGeneration>,
    resident: Reservation,
    pinned: Pinned,
}

/// Pins `candidate` and its members under `protection`, reserves the members' resident bytes, and records the pinned bytes before the protection goes. `None` when the record or a member cannot be pinned, does not name its members, or names more members than `max_deltas` admits: the candidate would not verify either, and the next one is tried without pinning what it lists.
///
/// # Errors
///
/// A ledger that cannot hold the members' resident bytes; no other candidate can change that.
fn secure(
    store: &GenerationStore,
    candidate: &Candidate,
    max_deltas: NonZeroUsize,
    ledger: &Arc<Ledger>,
    grant: &Admission,
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
    let pinned = pins
        .iter()
        .flat_map(|pin| pin.manifest.files.iter())
        .map(|file| file.size)
        .fold(0u64, u64::saturating_add);
    let resident = ledger.reserve(grant, ResourceClass::LayerTables, resident)?;
    // Recorded before the protection goes, so a prune that takes the exclusive lock next reads back no pin the ledger has not been told about.
    let pinned = ledger.pin(pinned);
    drop(protection);
    Ok(Some(Secured {
        pins,
        resident,
        pinned,
    }))
}

/// Takes the lifecycle's shared lock only around manifest reads: once to list the candidates and pin one, once more to re-read the selector before handoff. Hashing every member runs between them under the pins alone, so a mutator waits on this reader for no longer than a pin; one that holds the exclusive lock past the bounded wait refuses the reader instead. Run it on a blocking thread.
///
/// # Errors
///
/// No shared protection within the wait, no verifying composition, a store refusal, a ledger that cannot hold the view's resident bytes, or a selector that moved while the view was being taken. Nothing is handed out on any of them.
pub fn acquire(
    root: Option<&Path>,
    expected: &ExpectedVectors<'_>,
    bounds: ReaderBounds,
    ledger: &Arc<Ledger>,
    grant: &Admission,
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
        let Some(secured) = secure(
            &store,
            &candidate,
            bounds.max_deltas,
            ledger,
            grant,
            protection,
        )?
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
    let Secured {
        pins,
        resident,
        pinned,
    } = secured;
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
        resident,
        pinned,
        quarantined: AtomicBool::new(false),
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
    #[error("the view of composition {digest} is quarantined")]
    Quarantined { digest: String },
    #[error("one page of rows needs {bytes} bytes of scratch: {refusal}")]
    Scratch {
        bytes: u64,
        refusal: vector_admission::Refusal,
    },
    #[error(transparent)]
    Layered(#[from] LayeredRefusal),
}

/// Ranks `request` over the view's layers inside the caller's read transaction. Every layer's sidecar is checked against the request's expectation first, and one page of row scratch is reserved for the walk's duration in the ledger that holds the view's tables, so the two are judged against one resident total.
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
    grant: &Admission,
) -> Result<LayeredRanking, RankRefusal> {
    if view.is_quarantined() {
        return Err(RankRefusal::Quarantined {
            digest: view.digest.clone(),
        });
    }
    let expected = request.expected;
    check_view(view, expected).map_err(|field| RankRefusal::Identity { field })?;
    // The page's decoded rows plus the one raw row being read.
    let bytes = u64::try_from(request.bounds.page_rows.min(request.bounds.max_rows).get())
        .ok()
        .and_then(|rows| rows.checked_add(1))
        .and_then(|rows| rows.checked_mul(u64::from(view.layout.dimension)))
        .and_then(|elements| elements.checked_mul(size_of::<f32>() as u64))
        .ok_or(RankRefusal::Scratch {
            bytes: u64::MAX,
            refusal: vector_admission::Refusal::Overflow {
                pool: vector_admission::Pool::Resident,
            },
        })?;
    let _scratch = view
        .resident
        .ledger()
        .reserve(grant, ResourceClass::Scratch, bytes)
        .map_err(|refusal| RankRefusal::Scratch { bytes, refusal })?;
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
    rank_layers(conn, kernel, &query, budget).map_err(|refusal| {
        if walk_corruption(&refusal) {
            view.quarantine();
        }
        RankRefusal::Layered(refusal)
    })
}

/// Every layer's sidecar against the request's expectation; the refused field names the mismatch.
fn check_view(view: &PinnedVectors, expected: &ExpectedVectors<'_>) -> Result<(), &'static str> {
    for layer in &view.layers {
        vector_generation::check_identity(&layer.sidecar, expected).map_err(
            |refusal| match refusal {
                VectorRefusal::Identity { field } => field,
                _ => "identity",
            },
        )?;
    }
    Ok(())
}

/// A failed positioned read: a file that ends before the row is a missing row; any other error is a failed read.
fn read_fault(index: usize, error: &std::io::Error) -> RowFault {
    match error.kind() {
        std::io::ErrorKind::UnexpectedEof => RowFault::Missing(format!("row {index}: {error}")),
        _ => RowFault::Unavailable(format!("row {index}: {error}")),
    }
}

/// Not `Debug`: the query row is embedding content.
pub struct CompressedRequest<'a> {
    /// What the caller expects the view to be; every identity field of every layer is rechecked first.
    pub expected: &'a ExpectedVectors<'a>,
    pub query: &'a [f32],
    pub authority: Authority<'a>,
    pub capacity: CandidateCapacity,
    pub bounds: ScanBounds,
    /// Rows and tombstones the layers may carry together.
    pub max_entries: NonZeroUsize,
    /// Generations the view may hold, the base included.
    pub max_layers: NonZeroUsize,
    /// Bytes on disk the view may pin.
    pub max_pinned_bytes: u64,
    /// Original-row bytes the rescore may read. Each pool entry is one positioned read of exactly one row, so a full pool reads `R` rows and nothing else.
    pub max_read_bytes: u64,
}

/// The rescored ranking and the quantized pool it came from, kept apart so candidate coverage and rescored quality are measured on their own stages.
#[derive(Debug, Clone, PartialEq)]
pub struct CompressedRanking {
    pub pool: CandidatePool,
    pub rescored: Rescored,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CompressedRefusal {
    #[error("the view's {field} is not the request's")]
    Identity { field: &'static str },
    #[error("the view of composition {digest} is quarantined")]
    Quarantined { digest: String },
    #[error("the view holds {layers} generations, over the bound of {limit}")]
    Layers { layers: usize, limit: usize },
    #[error("the view pins {bytes} bytes, over the bound of {limit}")]
    PinnedBytes { bytes: u64, limit: u64 },
    #[error("a full pool reads {bytes} original-row bytes, over the bound of {limit}")]
    ReadBytes { bytes: u64, limit: u64 },
    #[error("the ranking needs {bytes} bytes of {class:?}: {refusal}")]
    Reservation {
        class: ResourceClass,
        bytes: u64,
        refusal: vector_admission::Refusal,
    },
    /// The scan refused; when the generation's own codes were missing or malformed, the view is quarantined too.
    #[error(transparent)]
    Candidates(CandidateRefusal),
    #[error("the query is not a member of the generation: {0}")]
    Query(RowRejection),
    #[error("the request's budget ended during the scan or the rescore")]
    Budget,
    /// An accepted occurrence's original row is missing or malformed in member `member`; the view is quarantined and nothing older or reconstructed stands in for it.
    #[error("the original row of occurrence {occurrence_id} in member {member}: {fault}")]
    Corrupt {
        member: String,
        occurrence_id: String,
        fault: RowFault,
    },
    /// An accepted row's read failed with its bytes possibly intact; the view stays usable.
    #[error(
        "reading the original row of occurrence {occurrence_id} in member {member} failed: {detail}"
    )]
    Io {
        member: String,
        occurrence_id: String,
        detail: String,
    },
}

/// Why the rescore stopped reading.
enum ReadStop {
    Budget,
    Fault(RowFault),
}

/// Where a compressed ranking stands; a test may hold or mutate the store here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RescoreEvent<'a> {
    /// A point inside the candidate scan.
    Scan(Window<'a>),
    /// The pool is selected; no original row has been read.
    AfterSelection,
    /// The original row `row` of member `member` is about to be read.
    ReadOriginal { member: &'a str, row: usize },
}

/// Selects one quantized pool over the view's layers inside the caller's read transaction, then rescores it from the original rows of the same pinned layers and returns the best `K`.
/// The view's identity, quarantine, generation count, pinned bytes, and the full pool's read bytes are checked, and the code scratch and the pool's row buffers are reserved in the ledger that holds the view's tables, before the projection is read.
/// An accepted row that is missing or fails the codec quarantines the view: the request refuses, and so does every later ranking over the view.
/// Run it through the request's blocking seam with the `Arc<PinnedVectors>` moved into the work, as [`rank`] is.
pub fn rank_compressed(
    view: &PinnedVectors,
    conn: &GuardedConn<'_>,
    kernel: &KernelStore,
    request: &CompressedRequest<'_>,
    budget: &EvalBudget,
    grant: &Admission,
    observe: &mut dyn FnMut(RescoreEvent<'_>),
) -> Result<CompressedRanking, CompressedRefusal> {
    if view.is_quarantined() {
        return Err(CompressedRefusal::Quarantined {
            digest: view.digest.clone(),
        });
    }
    check_view(view, request.expected).map_err(|field| CompressedRefusal::Identity { field })?;
    if view.layers.len() > request.max_layers.get() {
        return Err(CompressedRefusal::Layers {
            layers: view.layers.len(),
            limit: request.max_layers.get(),
        });
    }
    if view.pinned_bytes() > request.max_pinned_bytes {
        return Err(CompressedRefusal::PinnedBytes {
            bytes: view.pinned_bytes(),
            limit: request.max_pinned_bytes,
        });
    }
    let row_bytes = codec::row_bytes(view.layout.dimension);
    let read_bytes = (request.capacity.candidates().get() as u64).saturating_mul(row_bytes);
    if read_bytes > request.max_read_bytes {
        return Err(CompressedRefusal::ReadBytes {
            bytes: read_bytes,
            limit: request.max_read_bytes,
        });
    }
    let ledger = view.resident.ledger();
    let reserve = |class: ResourceClass, bytes: u64| {
        ledger
            .reserve(grant, class, bytes)
            .map_err(|refusal| CompressedRefusal::Reservation {
                class,
                bytes,
                refusal,
            })
    };
    let _scratch = reserve(ResourceClass::Scratch, scan_scratch_bytes(view))?;
    // The rescore reads one row at a time: its raw bytes and its decoded coordinates.
    let _rows = reserve(ResourceClass::RowBuffers, row_bytes.saturating_mul(2))?;
    let layers = view.resolver_layers();
    let windows: Vec<CodeWindow<'_>> = view.layers.iter().map(CodeWindow::of).collect();
    let codes: Vec<LayerCodes<'_>> = view
        .layers
        .iter()
        .zip(&windows)
        .map(|(layer, window)| LayerCodes {
            scales: &layer.scales,
            codes: window,
        })
        .collect();
    let query = CandidateQuery {
        generation: request.expected.generation,
        metric: view.layout.metric,
        unit_norm_tolerance: view.layout.unit_norm_tolerance,
        query: request.query,
        authority: request.authority,
        capacity: request.capacity,
        bounds: request.bounds,
        layers: &layers,
        codes: &codes,
        max_entries: request.max_entries,
    };
    let pool = select_candidates_observed(conn, kernel, &query, budget, |window| {
        observe(RescoreEvent::Scan(window));
    })
    .map_err(|refusal| {
        if let CandidateRefusal::Layered(layered) = &refusal
            && walk_corruption(layered)
        {
            view.quarantine();
        }
        CompressedRefusal::Candidates(refusal)
    })?;
    // A scan the budget ended returns a discarded pool, so the request refuses before a selection is reported.
    budget.check().map_err(|_| CompressedRefusal::Budget)?;
    observe(RescoreEvent::AfterSelection);
    let mut bytes = Vec::new();
    let rescored = rescore_pool(
        &pool,
        &view.layout,
        request.query,
        request.capacity.k(),
        |winner: &WinnerRow, row: &mut Vec<f32>| {
            if budget.check().is_err() {
                return Err(ReadStop::Budget);
            }
            let layer = &view.layers[winner.layer];
            observe(RescoreEvent::ReadOriginal {
                member: &layer.digest,
                row: winner.row,
            });
            layer
                .row_into(winner.row, &mut bytes, row)
                .map_err(ReadStop::Fault)
        },
    );
    let rescored = rescored.map_err(|refusal| rescore_refusal(view, refusal))?;
    budget.check().map_err(|_| CompressedRefusal::Budget)?;
    Ok(CompressedRanking { pool, rescored })
}

/// A missing or malformed accepted row quarantines the view; a failed read and an ended budget leave it usable.
fn rescore_refusal(view: &PinnedVectors, refusal: RescoreRefusal<ReadStop>) -> CompressedRefusal {
    let (occurrence_id, winner, fault) = match refusal {
        RescoreRefusal::Query(rejection) => return CompressedRefusal::Query(rejection),
        RescoreRefusal::Read {
            fault: ReadStop::Budget,
            ..
        } => return CompressedRefusal::Budget,
        RescoreRefusal::Read {
            occurrence_id,
            winner,
            fault: ReadStop::Fault(fault),
        } => (occurrence_id, winner, fault),
        RescoreRefusal::Row {
            occurrence_id,
            winner,
            rejection,
        } => (occurrence_id, winner, RowFault::Rejected(rejection)),
    };
    let member = view.layers[winner.layer].digest.clone();
    if let RowFault::Unavailable(detail) = fault {
        return CompressedRefusal::Io {
            member,
            occurrence_id,
            detail,
        };
    }
    view.quarantine();
    CompressedRefusal::Corrupt {
        member,
        occurrence_id,
        fault,
    }
}

/// Rows or codes a walk could not read or that the recipe refuses: the generation's own bytes are bad.
fn walk_corruption(refusal: &LayeredRefusal) -> bool {
    matches!(
        refusal,
        LayeredRefusal::Oracle(OracleRefusal::Unreadable { .. } | OracleRefusal::StoredRow { .. })
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only a file that ends before the row reads as a missing row; every other error leaves the row's bytes possibly intact.
    #[test]
    fn a_short_read_is_a_missing_row_and_any_other_read_error_is_a_failed_read() {
        let short = std::io::Error::from(std::io::ErrorKind::UnexpectedEof);
        assert!(matches!(read_fault(3, &short), RowFault::Missing(_)));
        for kind in [
            std::io::ErrorKind::Other,
            std::io::ErrorKind::PermissionDenied,
        ] {
            let error = std::io::Error::from(kind);
            assert!(
                matches!(read_fault(3, &error), RowFault::Unavailable(_)),
                "{kind:?}"
            );
        }
        let eio = std::io::Error::from_raw_os_error(5);
        assert!(matches!(read_fault(3, &eio), RowFault::Unavailable(_)));
    }

    const WIDTH: usize = 4;
    const DECLARED: usize = 5;

    fn all_codes() -> Vec<u8> {
        (0..DECLARED * WIDTH).map(|byte| byte as u8).collect()
    }

    fn row_codes(codes: &[u8], row: usize) -> Vec<i8> {
        codes[row * WIDTH..(row + 1) * WIDTH]
            .iter()
            .map(|byte| *byte as i8)
            .collect()
    }

    /// A whole file fills one window with every declared row, so later rows are served without another read.
    #[test]
    fn a_whole_code_file_fills_one_window_with_every_declared_row() {
        let codes = all_codes();
        let file = tempfile::tempfile().unwrap();
        file.write_all_at(&codes, 0).unwrap();
        let window = CodeWindow::new(&file, WIDTH, DECLARED);
        let mut into = Vec::new();
        for row in 0..DECLARED {
            window.codes_into(row, &mut into).unwrap();
            assert_eq!(into, row_codes(&codes, row), "row {row}");
            assert_eq!(window.held.borrow().first, 0, "row {row} refilled");
            assert_eq!(window.held.borrow().rows, DECLARED);
        }
    }

    /// After a window read fails, every later fill of the scan reads one row, so a failing span past the requested row is not read again for each row before it.
    /// The missing rows are restored after the first fill: a retried window read would then succeed and hold every remaining row, which makes the retry observable.
    #[test]
    fn a_failed_window_read_is_not_retried_for_the_rows_after_it() {
        let codes = all_codes();
        let file = tempfile::tempfile().unwrap();
        file.write_all_at(&codes[..2 * WIDTH], 0).unwrap();
        let window = CodeWindow::new(&file, WIDTH, DECLARED);
        let mut into = Vec::new();
        window.codes_into(0, &mut into).unwrap();
        assert_eq!(into, row_codes(&codes, 0));
        assert_eq!(window.held.borrow().rows, 1);
        file.write_all_at(&codes[2 * WIDTH..], (2 * WIDTH) as u64)
            .unwrap();
        for row in 1..DECLARED {
            window.codes_into(row, &mut into).unwrap();
            assert_eq!(into, row_codes(&codes, row), "row {row}");
            assert_eq!(
                window.held.borrow().rows,
                1,
                "row {row} retried the full window"
            );
        }
    }

    /// A failed read leaves the view usable; missing or malformed rows or codes quarantine it.
    #[test]
    fn only_missing_or_malformed_rows_count_as_walk_corruption() {
        let oracle = LayeredRefusal::Oracle;
        let id = || "occurrence".to_owned();
        assert!(!walk_corruption(&oracle(OracleRefusal::ReadFailed {
            occurrence_id: id(),
            detail: "eio".to_owned()
        })));
        assert!(!walk_corruption(&oracle(OracleRefusal::BudgetExhausted)));
        assert!(walk_corruption(&oracle(OracleRefusal::Unreadable {
            occurrence_id: id(),
            detail: "short".to_owned()
        })));
        assert!(walk_corruption(&oracle(OracleRefusal::StoredRow {
            occurrence_id: id(),
            rejection: RowRejection::ZeroNorm
        })));
    }
}
