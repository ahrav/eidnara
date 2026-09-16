//! Compacts a composition's frozen prefix into one new base holding exactly the rows the prefix resolves to, then republishes it under whatever deltas were published after the cut. The cut is the composition a reader's view pinned: its base and deltas are the prefix and are read only through the pinned files, so what compaction consumed is what it read. Every masked row and every superseded row of the prefix is absent from the new base and every tombstone of the prefix is applied by that absence, so the new base carries none; the tail keeps its own tombstones and its own order above the new base, and the resolver reads the compacted composition to the same winners as the old.
//! Publication reads the selected composition back first and refuses unless it still stands on the cut's base with the cut's deltas as a prefix; only then is the new base staged. Whatever follows the cut is the tail, recorded independently of the compactor and carried over exactly once. A cut whose base has moved was compacted by someone else and refuses rather than publishing a second history.
//! Both steps run under the caller's exclusive transaction lock, carried by the staging handle, so the store cannot change under the walk or between the readback and the rename.

use std::num::NonZeroUsize;
use std::path::Path;

use host_runtime::generation::{CurrentProfile, GenerationError, ProfileEvent};
use retrieval::dense::codec::ARTIFACT_HEADER_BYTES;
use retrieval::dense::export::{ExportedRow, LiveRows};
use retrieval::dense::{ResolveRefusal, RowAccess, RowFault, resolve};

use crate::vector_admission::{self, Reservation, ResourceClass};
use crate::vector_composition::{
    self, Composition, CompositionRefusal, CompositionSpec, Progress, VerifiedComposition,
};
use crate::vector_generation::{self, BuiltVectors, ExpectedVectors, Staging, VectorRefusal};
use crate::vector_reader::PinnedVectors;

/// The frozen prefix: the composition a view pinned when compaction began, member by member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cut {
    pub digest: String,
    pub base: String,
    pub deltas: Vec<String>,
}

impl Cut {
    pub fn of(view: &PinnedVectors) -> Self {
        let members = view.members();
        Self {
            digest: view.digest.clone(),
            base: members[0].clone(),
            deltas: members[1..].to_vec(),
        }
    }

    /// How many of `current`'s deltas are the cut's own, when it still stands on the cut's base and deltas; everything past them is the tail.
    fn prefix_len(&self, current: &Composition) -> Option<usize> {
        (current.base == self.base && current.deltas.starts_with(&self.deltas))
            .then_some(self.deltas.len())
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CompactionRefusal {
    #[error("layer {digest} does not carry the expectation's {field}")]
    Identity { digest: String, field: &'static str },
    #[error("the prefix does not resolve: {0}")]
    Resolve(#[from] ResolveRefusal),
    #[error("row {row} of layer {layer} could not be read: {fault}")]
    Row {
        layer: usize,
        row: usize,
        fault: RowFault,
    },
    /// At the delta cap this leaves the namespace unable to admit a delta or compact, until a full base is published afresh.
    #[error("the prefix resolves to no live row; a base needs at least one")]
    Empty,
    #[error("the ledger refused the compaction's bytes: {0}")]
    Reservation(#[from] vector_admission::Refusal),
    #[error("building the compacted base: {0}")]
    Build(VectorRefusal),
    #[error("staging the compacted base: {0}")]
    Stage(VectorRefusal),
    #[error(
        "the selected composition no longer stands on the cut; it was compacted or replaced by another publisher"
    )]
    PrefixMoved,
    #[error("no composition is selected")]
    NoSelection,
    #[error(transparent)]
    Composition(#[from] CompositionRefusal),
    #[error("the work directory could not be removed: {0}")]
    Discard(String),
    #[error(
        "the publication's outcome is unknown after {progress:?}; reconcile {digest} before retrying"
    )]
    Unknown { digest: String, progress: Progress },
}

impl From<GenerationError> for CompactionRefusal {
    fn from(error: GenerationError) -> Self {
        Self::Composition(error.into())
    }
}

/// A compacted base built in a work directory. The files it wrote are disk scratch reserved in the ledger until [`Self::discard`] removes them; dropping without discarding leaves the files on disk uncounted, so a caller that gives up on a compaction discards it.
#[derive(Debug)]
pub struct Compacted {
    pub cut: Cut,
    pub built: BuiltVectors,
    pub winners: usize,
    pub superseded: usize,
    pub masked: usize,
    _scratch: Reservation,
}

impl Compacted {
    /// Removes the work directory and releases the scratch reservation with it.
    ///
    /// # Errors
    ///
    /// A directory that cannot be removed; the reservation is released either way, since the caller has given the files up.
    pub fn discard(self) -> Result<(), CompactionRefusal> {
        std::fs::remove_dir_all(&self.built.dir)
            .map_err(|error| CompactionRefusal::Discard(error.kind().to_string()))
    }
}

/// The bytes the sidecar and the row artifact header take beyond the rows; the identifier list is bounded per row.
const FIXED_SCRATCH_BYTES: u64 = 4096 + ARTIFACT_HEADER_BYTES as u64;
/// One quoted 64-hex identifier with its separator in `row-ids.json`.
const IDENTIFIER_BYTES: u64 = 67;

/// Resolves the view's prefix and builds one base of exactly its winners, at the checkpoint of the prefix's last layer, so every tail delta still follows it. Every layer is checked against `expected` before anything is reserved or read, and the rows and the files are reserved before any row is read; the row reservation ends with the build, since the rows are then on disk.
///
/// # Errors
///
/// A layer that does not carry the expectation, a prefix that does not resolve, a row that cannot be read, an empty result, a refused reservation, or a build refusal; nothing is staged, and a refusal before the build leaves the work directory empty.
pub fn compact(
    view: &PinnedVectors,
    expected: &ExpectedVectors<'_>,
    staging: &Staging<'_>,
    max_entries: NonZeroUsize,
    work_dir: &Path,
) -> Result<Compacted, CompactionRefusal> {
    for layer in &view.layers {
        vector_generation::check_identity(&layer.sidecar, expected).map_err(|refusal| {
            CompactionRefusal::Identity {
                digest: layer.digest.clone(),
                field: match refusal {
                    VectorRefusal::Identity { field } => field,
                    _ => "identity",
                },
            }
        })?;
    }
    let layers = view.resolver_layers();
    let resolved = resolve(&layers, max_entries)?;
    if resolved.winners.is_empty() {
        return Err(CompactionRefusal::Empty);
    }
    let dimension = u64::from(view.layout.dimension);
    let rows = resolved.winners.len() as u64;
    let row_bytes = rows.saturating_mul(dimension).saturating_mul(4);
    // The build holds the decoded rows, the encoded row artifact, and the codes at once.
    let resident = row_bytes
        .saturating_mul(2)
        .saturating_add(rows.saturating_mul(dimension));
    let scratch = row_bytes
        .saturating_add(rows.saturating_mul(dimension))
        .saturating_add(dimension.saturating_mul(4))
        .saturating_add(rows.saturating_mul(IDENTIFIER_BYTES))
        .saturating_add(FIXED_SCRATCH_BYTES);
    let rows_held =
        staging
            .ledger
            .reserve(staging.admission, ResourceClass::RowBuffers, resident)?;
    let scratch = staging.ledger.reserve_disk(
        staging.admission,
        ResourceClass::CompactionScratch,
        scratch,
        staging.store,
    )?;
    let mut exported = Vec::with_capacity(resolved.winners.len());
    for winner in &resolved.winners {
        let vector =
            view.layers[winner.layer]
                .row(winner.row)
                .map_err(|fault| CompactionRefusal::Row {
                    layer: winner.layer,
                    row: winner.row,
                    fault,
                })?;
        exported.push(ExportedRow {
            occurrence_id: winner.occurrence_id.clone(),
            vector,
        });
    }
    let checkpoint = view
        .layers
        .last()
        .expect("a view has a base")
        .checkpoint
        .clone();
    let built = vector_generation::build(
        expected,
        &LiveRows {
            checkpoint,
            rows: exported,
            tombstones: Vec::new(),
        },
        work_dir,
    )
    .map_err(CompactionRefusal::Build)?;
    drop(rows_held);
    let written: u64 = built.sidecar.files.iter().map(|file| file.size).sum();
    if written > scratch.bytes() {
        return Err(CompactionRefusal::Reservation(
            vector_admission::Refusal::Overflow {
                pool: vector_admission::Pool::Disk,
            },
        ));
    }
    Ok(Compacted {
        cut: Cut::of(view),
        built,
        winners: resolved.winners.len(),
        superseded: resolved.superseded,
        masked: resolved.masked,
        _scratch: scratch,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    pub digest: String,
    pub base: String,
    /// Deltas published after the cut and carried over unchanged, in order.
    pub tail: Vec<String>,
    pub sequence: u64,
}

/// Reads the selected composition back, refuses unless it still stands on the cut, then stages the compacted base and publishes it under the deltas the selection carries past the cut. `staging` holds the exclusive transaction lock, so the selection cannot move between the readback and the publication.
///
/// # Errors
///
/// A selection that no longer stands on the cut, with nothing staged; a staging or composition refusal; or an unknown outcome after the selector rename returned, which the caller settles with [`vector_composition::reconcile`] before any retry.
pub fn publish(
    compacted: &Compacted,
    staging: &Staging<'_>,
    expected: &ExpectedVectors<'_>,
    max_deltas: NonZeroUsize,
    work_dir: &Path,
    observer: &mut dyn FnMut(ProfileEvent) -> Result<(), GenerationError>,
) -> Result<Published, CompactionRefusal> {
    let selected = match staging.store.read_vector_current()? {
        CurrentProfile::Current(digest) => digest,
        CurrentProfile::Absent => return Err(CompactionRefusal::NoSelection),
        CurrentProfile::Quarantined => return Err(CompositionRefusal::Quarantined.into()),
    };
    let VerifiedComposition {
        composition: current,
        deltas: current_deltas,
        ..
    } = vector_composition::verify_composition(staging.store, &selected, expected, max_deltas)?;
    let prefix = compacted
        .cut
        .prefix_len(&current)
        .ok_or(CompactionRefusal::PrefixMoved)?;
    let carried: Vec<_> = current_deltas.into_iter().skip(prefix).collect();
    let tail: Vec<String> = carried.iter().map(|delta| delta.digest.clone()).collect();

    let base_digest =
        vector_generation::stage(&compacted.built, staging).map_err(CompactionRefusal::Stage)?;
    let base = vector_generation::verify(staging.store, &base_digest, expected)
        .map_err(CompactionRefusal::Stage)?;
    let composition = vector_composition::compose(&CompositionSpec {
        expected,
        sequence: current.sequence + 1,
        base: &base,
        deltas: &carried,
        max_deltas,
    })?;
    match vector_composition::publish(&composition, staging, work_dir, observer) {
        Ok(digest) => Ok(Published {
            digest,
            base: base_digest,
            tail,
            sequence: composition.sequence,
        }),
        Err(failure) if matches!(failure.progress, Progress::Acknowledged | Progress::Durable) => {
            Err(CompactionRefusal::Unknown {
                digest: composition.digest(),
                progress: failure.progress,
            })
        }
        Err(failure) => Err(failure.refusal.into()),
    }
}
