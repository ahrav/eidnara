//! Compacts a composition's frozen prefix into one new base holding exactly the rows the prefix resolves to, then republishes it under whatever deltas were published after the cut. The cut is the composition a reader's view pinned: its base and deltas are the prefix and are read only through the pinned files, so what compaction consumed is what it read. Every masked row and every superseded row of the prefix is absent from the new base and every tombstone of the prefix is applied by that absence, so the new base carries none; the tail keeps its own tombstones and its own order above the new base, and the resolver reads the compacted composition to the same winners as the old.
//! Publication reads the selected composition back and refuses unless it still stands on the cut's base with the cut's deltas as a prefix; whatever follows them is the tail, recorded independently of the compactor and carried over exactly once. A cut whose base has moved was compacted by someone else and refuses rather than publishing a second history.
//! Resources are reserved through the ledger before any row is read and released only when the compaction's output is dropped, never on cancellation alone.

use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;

use host_runtime::generation::{GenerationError, GenerationStore, ProfileEvent};
use retrieval::dense::export::{ExportedRow, LiveRows};
use retrieval::dense::{ResolveRefusal, RowAccess, RowFault, resolve};

use crate::projection_gates::Admission;
use crate::vector_admission::{self, Ledger, Reservation, ResourceClass};
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

    /// The deltas `current` carries past this cut, when it still stands on the cut's base and deltas.
    fn tail_of<'a>(&self, current: &'a Composition) -> Option<&'a [String]> {
        (current.base == self.base && current.deltas.starts_with(&self.deltas))
            .then(|| &current.deltas[self.deltas.len()..])
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CompactionRefusal {
    #[error("the prefix does not resolve: {0}")]
    Resolve(#[from] ResolveRefusal),
    #[error("row {row} of layer {layer} could not be read: {fault}")]
    Row {
        layer: usize,
        row: usize,
        fault: RowFault,
    },
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
    #[error("the lifecycle store refused: {0}")]
    Store(String),
    #[error(
        "the publication's outcome is unknown after {progress:?}; reconcile {digest} before retrying"
    )]
    Unknown { digest: String, progress: Progress },
}

impl From<GenerationError> for CompactionRefusal {
    fn from(error: GenerationError) -> Self {
        Self::Store(error.to_string())
    }
}

/// A compacted base built in a work directory, with the resources it holds until dropped: the rows it read into memory and the scratch its files occupy.
pub struct Compacted {
    pub cut: Cut,
    pub built: BuiltVectors,
    pub winners: usize,
    pub superseded: usize,
    pub masked: usize,
    _rows: Reservation,
    _scratch: Reservation,
}

impl std::fmt::Debug for Compacted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Compacted")
            .field("cut", &self.cut)
            .field("digest", &self.built.digest())
            .field("winners", &self.winners)
            .field("superseded", &self.superseded)
            .field("masked", &self.masked)
            .finish_non_exhaustive()
    }
}

/// Whether a composition of `deltas` deltas stands at the delta cap `cap`, so the next delta must wait for compaction: publication at `cap + 1` is refused by the ledger, and compaction is due at `cap`.
pub fn due(deltas: usize, cap: u64) -> bool {
    deltas as u64 >= cap
}

/// Resolves the view's prefix and builds one base of exactly its winners, at the checkpoint of the prefix's last layer, so every tail delta still follows it.
///
/// # Errors
///
/// A prefix that does not resolve, a row that cannot be read, an empty result, a refused reservation, or a build refusal; nothing is staged.
pub fn compact(
    view: &PinnedVectors,
    expected: &ExpectedVectors<'_>,
    ledger: &Arc<Ledger>,
    grant: &Admission,
    store: &GenerationStore,
    max_entries: NonZeroUsize,
    work_dir: &Path,
) -> Result<Compacted, CompactionRefusal> {
    let layers = view.resolver_layers();
    let resolved = resolve(&layers, max_entries)?;
    if resolved.winners.is_empty() {
        return Err(CompactionRefusal::Empty);
    }
    let dimension = u64::from(view.layout.dimension);
    let rows = resolved.winners.len() as u64;
    // The winners' f32 rows are held in memory until the build has written them; the files the build writes are scratch until they are staged.
    let row_bytes = rows.saturating_mul(dimension).saturating_mul(4);
    let scratch = row_bytes
        .saturating_add(rows.saturating_mul(dimension))
        .saturating_add(dimension.saturating_mul(4))
        .saturating_add(rows.saturating_mul(80));
    let _rows = ledger.reserve(grant, ResourceClass::RowBuffers, row_bytes)?;
    let _scratch = ledger.reserve_disk(grant, ResourceClass::CompactionScratch, scratch, store)?;
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
    Ok(Compacted {
        cut: Cut::of(view),
        built,
        winners: resolved.winners.len(),
        superseded: resolved.superseded,
        masked: resolved.masked,
        _rows,
        _scratch,
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

/// Stages the compacted base, reads the selected composition back, and publishes the new base under the deltas the selection carries past the cut. `staging` holds the exclusive transaction lock, so the selection cannot move between the readback and the publication.
///
/// # Errors
///
/// A staging refusal, a selection that no longer stands on the cut, a composition refusal before staging, or an unknown outcome after the selector rename returned, which the caller settles with [`vector_composition::reconcile`] before any retry.
pub fn publish(
    compacted: &Compacted,
    staging: &Staging<'_>,
    expected: &ExpectedVectors<'_>,
    max_deltas: NonZeroUsize,
    work_dir: &Path,
    observer: &mut dyn FnMut(ProfileEvent) -> Result<(), GenerationError>,
) -> Result<Published, CompactionRefusal> {
    let base_digest =
        vector_generation::stage(&compacted.built, staging).map_err(CompactionRefusal::Stage)?;
    let base = vector_generation::verify(staging.store, &base_digest, expected)
        .map_err(CompactionRefusal::Stage)?;
    let selected = match staging.store.read_vector_current()? {
        host_runtime::generation::CurrentProfile::Current(digest) => digest,
        host_runtime::generation::CurrentProfile::Absent => {
            return Err(CompactionRefusal::NoSelection);
        }
        host_runtime::generation::CurrentProfile::Quarantined => {
            return Err(CompositionRefusal::Quarantined.into());
        }
    };
    let VerifiedComposition {
        composition: current,
        deltas: current_deltas,
        ..
    } = vector_composition::verify_composition(staging.store, &selected, expected, max_deltas)?;
    let tail = compacted
        .cut
        .tail_of(&current)
        .ok_or(CompactionRefusal::PrefixMoved)?;
    let carried: Vec<_> = current_deltas
        .into_iter()
        .skip(compacted.cut.deltas.len())
        .collect();
    debug_assert_eq!(carried.len(), tail.len());
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
            tail: tail.to_vec(),
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
