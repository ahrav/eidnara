//! Compacts a composition's frozen prefix into one new base holding exactly the rows the prefix resolves to, then republishes it under whatever deltas were published after the cut. The cut is the composition a reader's view pinned: its base and deltas are the prefix and are read only through the pinned files, so what compaction consumed is what it read. Every masked row and every superseded row of the prefix is absent from the new base and every tombstone of the prefix is applied by that absence, so the new base carries none; the tail keeps its own tombstones and its own order above the new base, and the resolver reads the compacted composition to the same winners as the old.
//! Publication reads the selected composition back first and refuses unless it still stands on the cut's base with the cut's deltas as a prefix; only then is the new base staged. Whatever follows the cut is the tail, recorded independently of the compactor and carried over exactly once. A cut whose base has moved was compacted by someone else and refuses rather than publishing a second history.
//! Both steps run under the caller's exclusive transaction lock, carried by the staging handle, so the store cannot change under the walk or between the readback and the rename.

use std::mem::size_of;
use std::num::NonZeroUsize;
use std::path::Path;

use host_runtime::generation::{CurrentProfile, GenerationError, ProfileEvent};
use retrieval::dense::export::{ExportedRow, LiveRows};
use retrieval::dense::{ResolveRefusal, RowAccess, RowFault, Winner, resolve};

use crate::vector_admission::{self, Reservation, ResourceClass};
use crate::vector_composition::{self, Composition, CompositionRefusal, CompositionSpec, Progress};
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
            digest: view.digest().to_owned(),
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
    #[error("the ledger refused the compaction's bytes: {0}")]
    Reservation(#[from] vector_admission::Refusal),
    /// The build's inventory exceeded the footprint it was sized from; the files are removed with the reservation.
    #[error("the build wrote {written} bytes against a {reserved}-byte scratch reservation")]
    ScratchUnderestimated { reserved: u64, written: u64 },
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

/// A compacted base built in a work directory. Its files are disk scratch reserved in the ledger until [`Self::discard`] removes them and releases the charge; dropping without discarding removes them too, so a caller that gives up on a compaction leaves nothing behind either way.
#[derive(Debug)]
pub struct Compacted {
    cut: Cut,
    pub built: BuiltVectors,
    pub winners: usize,
    pub superseded: usize,
    pub masked: usize,
    _scratch: Reservation,
}

impl Compacted {
    /// The prefix this output stands on. It is fixed at `compact`, so `publish` judges the selection against what was read.
    ///
    /// ```compile_fail,E0616
    /// use daemon::vector_compaction::Compacted;
    /// fn retarget(compacted: &mut Compacted, base: String) {
    ///     compacted.cut.base = base;
    /// }
    /// ```
    pub fn cut(&self) -> &Cut {
        &self.cut
    }

    /// Unlinks the build's file names, removes the work directory, and releases the scratch reservation. Files under other names are left in place; the build's names are the module's own in its own work directory, whoever wrote them.
    ///
    /// # Errors
    ///
    /// A build file that cannot be unlinked or a directory that cannot be removed, because it holds something the build did not write; the reservation is released either way, since the caller has given the files up.
    pub fn discard(self) -> Result<(), CompactionRefusal> {
        vector_generation::remove_build(&self.built.dir)
            .map_err(|error| CompactionRefusal::Discard(error.kind().to_string()))
    }
}

impl Drop for Compacted {
    fn drop(&mut self) {
        // A second removal after `discard` finds nothing to unlink; a failure here has no caller to report to.
        let _ = vector_generation::remove_build(&self.built.dir);
    }
}

/// `compact` validates every prefix layer against `expected` before reserving memory or reading rows. It builds a base from the prefix winners at the last layer's checkpoint so each tail delta follows that base. It reserves `vector_generation::footprint` plus per-row compaction state before reading rows; the resident charge ends after the rows reach disk, while the disk charge remains with the output.
///
/// # Errors
///
/// Returns an error without staging output if a prefix layer rejects `expected`, prefix resolution or a row read fails, a reservation or the build is refused, or the inventory exceeds `vector_generation::footprint`; no failed attempt leaves a build file in the work directory.
pub fn compact(
    view: &PinnedVectors,
    expected: &ExpectedVectors<'_>,
    staging: &Staging<'_>,
    max_entries: NonZeroUsize,
    work_dir: &Path,
) -> Result<Compacted, CompactionRefusal> {
    for layer in view.layers() {
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
    let checkpoint = view
        .layers()
        .last()
        .expect("a view has a base")
        .checkpoint
        .clone();
    let footprint = vector_generation::footprint(
        expected,
        &checkpoint,
        resolved
            .winners
            .iter()
            .map(|winner| winner.occurrence_id.as_str()),
        std::iter::empty(),
    );
    // Beyond the build's own peak, compaction holds each winner's identifier twice, in the resolution and in the export, the prefix's checkpoint it hands the build, and one row's bytes and decode in flight while the export fills.
    let identifier_bytes = resolved.winners.iter().fold(0u64, |total, winner| {
        total.saturating_add(winner.occurrence_id.len() as u64)
    });
    let rows = resolved.winners.len() as u64;
    let dimension = u64::from(view.layout().dimension);
    let own = identifier_bytes
        .saturating_mul(2)
        .saturating_add(
            rows.saturating_mul((size_of::<Winner>() + size_of::<ExportedRow>()) as u64),
        )
        .saturating_add(checkpoint.hold_id.len() as u64)
        .saturating_add(dimension.saturating_mul(8));
    let rows_held = staging.ledger.reserve(
        staging.admission,
        ResourceClass::RowBuffers,
        footprint.resident.saturating_add(own),
    )?;
    let scratch = staging.ledger.reserve_disk(
        staging.admission,
        ResourceClass::CompactionScratch,
        footprint.disk,
        staging.store,
    )?;
    let mut exported = Vec::with_capacity(resolved.winners.len());
    for winner in &resolved.winners {
        let vector = view.layers()[winner.layer]
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
    let built = match vector_generation::build(
        expected,
        &LiveRows {
            checkpoint,
            rows: exported,
            tombstones: Vec::new(),
        },
        work_dir,
    ) {
        Ok(built) => built,
        Err(refusal) => {
            // The build may have written some of its files before refusing; none of them is charged once the reservation goes.
            let _ = vector_generation::remove_build(work_dir);
            return Err(CompactionRefusal::Build(refusal));
        }
    };
    // The inventory is sized while the rows' reservation still covers the sidecar's serialization.
    let written: u64 = built
        .sidecar
        .stage_manifest()
        .files
        .iter()
        .map(|file| file.size)
        .sum();
    drop(rows_held);
    let compacted = Compacted {
        cut: Cut::of(view),
        built,
        winners: resolved.winners.len(),
        superseded: resolved.superseded,
        masked: resolved.masked,
        _scratch: scratch,
    };
    if written > footprint.disk {
        // Dropping `compacted` removes what the build wrote with the reservation.
        return Err(CompactionRefusal::ScratchUnderestimated {
            reserved: footprint.disk,
            written,
        });
    }
    Ok(compacted)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    pub digest: String,
    pub base: String,
    /// Deltas published after the cut and carried over unchanged, in order.
    pub tail: Vec<String>,
    pub sequence: u64,
}

/// Only the deltas past the cut are verified here: the view verified and pinned the cut's members at acquisition, and none of them is a member of the new composition. `staging` holds the exclusive transaction lock, so the selection cannot move between the readback and the publication.
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
    let current = vector_composition::verify_record(staging.store, &selected, expected)?;
    let prefix = compacted
        .cut
        .prefix_len(&current)
        .ok_or(CompactionRefusal::PrefixMoved)?;
    // No sequence follows the last one, so the refusal comes before anything is staged.
    let sequence = current
        .sequence
        .checked_add(1)
        .ok_or(CompositionRefusal::Sequence {
            sequence: current.sequence,
            selected: current.sequence,
        })?;
    let tail = current.deltas[prefix..].to_vec();
    let carried = tail
        .iter()
        .map(|delta| vector_composition::verify_member(staging.store, delta, expected))
        .collect::<Result<Vec<_>, _>>()?;

    let base_digest =
        vector_generation::stage(&compacted.built, staging).map_err(CompactionRefusal::Stage)?;
    let base = vector_generation::verify(staging.store, &base_digest, expected)
        .map_err(CompactionRefusal::Stage)?;
    let composition = vector_composition::compose(&CompositionSpec {
        expected,
        sequence,
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
