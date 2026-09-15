//! One ledger for every byte vector work holds, resident or on disk, judged against the manifest's vector limits through the evidence gate. A reservation is taken atomically against what is already held: the ledger locks its tally, adds the increment, asks the gate whether the total is within the limit, and records it only on a yes, so two reservations racing for the last bytes cannot both win. Dropping a reservation releases it; nothing else does, so cancellation of the work that took it releases nothing until that work lets go.
//! The disk pool counts the store's own generations as well: a reservation states what the store holds so staging, promotion, and compaction are refused before they would push the store past its bound, not after. Generations a reader pins are already in that store total while they exist, so the pinned class is recorded for the census and for reconciling a prune's readback, not added to the limit a second time.
//! Static residents (model memory, tokenizer, SQLite cache) are charged once each; a second charge for the same class is refused rather than doubled.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use host_runtime::generation::{GENERATIONS_DIR_NAME, GenerationStore, PruneReport};

use crate::projection_gates::{Admission, Denial, HookGate, InvalidationIdentity};

/// The limits the ledger's two pools are judged against.
pub const RESIDENT_LIMIT: &str = "vector_resident_bytes";
pub const DISK_LIMIT: &str = "vector_disk_bytes";
/// Deltas a composition may name; checked at delta admission through the same gate.
pub const DELTA_LIMIT: &str = "vector_delta_count";

/// What a reservation holds. Every class belongs to one pool and one limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResourceClass {
    /// The embedding model's resident memory; charged once per process.
    ModelMemory,
    /// Tokenizer instances and their cache; charged once per process.
    TokenizerCache,
    /// The projection's SQLite page cache; charged once per process.
    SqliteCache,
    /// Text held for embedding.
    Text,
    /// Row scratch a ranking reads into.
    Scratch,
    /// Decoded f32 or int8 rows held beyond one page.
    RowBuffers,
    /// A view's identifiers, tombstones, scales, and sidecars.
    LayerTables,
    /// Files being copied into the store.
    Staging,
    /// A compactor's working files.
    CompactionScratch,
    /// Generations a view pins; the store's total already holds their bytes, so this class is census only.
    PinnedGenerations,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pool {
    Resident,
    Disk,
}

impl ResourceClass {
    pub const ALL: [ResourceClass; 10] = [
        Self::ModelMemory,
        Self::TokenizerCache,
        Self::SqliteCache,
        Self::Text,
        Self::Scratch,
        Self::RowBuffers,
        Self::LayerTables,
        Self::Staging,
        Self::CompactionScratch,
        Self::PinnedGenerations,
    ];

    pub fn pool(self) -> Pool {
        match self {
            Self::ModelMemory
            | Self::TokenizerCache
            | Self::SqliteCache
            | Self::Text
            | Self::Scratch
            | Self::RowBuffers
            | Self::LayerTables => Pool::Resident,
            Self::Staging | Self::CompactionScratch | Self::PinnedGenerations => Pool::Disk,
        }
    }

    /// Whether the class adds to its pool's limit; pinned generations are already in the store total a disk reservation states.
    pub fn counted(self) -> bool {
        self != Self::PinnedGenerations
    }

    /// Charged once for the process; a second reservation is a double charge.
    pub fn is_static(self) -> bool {
        matches!(
            self,
            Self::ModelMemory | Self::TokenizerCache | Self::SqliteCache
        )
    }
}

impl Pool {
    pub fn limit(self) -> &'static str {
        match self {
            Self::Resident => RESIDENT_LIMIT,
            Self::Disk => DISK_LIMIT,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error(transparent)]
    Denied(#[from] Denial),
    #[error("{class:?} is already charged; a static resident is charged once")]
    AlreadyCharged { class: ResourceClass },
    #[error("the {pool:?} total would leave the byte domain")]
    Overflow { pool: Pool },
    #[error("the lifecycle store refused: {0}")]
    Store(String),
}

/// What the ledger holds at one instant, by class and by pool. The pool totals are what the limits see: `disk` leaves out the census-only pinned class, which the store's own total already carries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Census {
    pub held: BTreeMap<ResourceClass, u64>,
    pub resident: u64,
    pub disk: u64,
}

/// A prune's readback set against the ledger's pinned generations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reconciliation {
    /// Bytes the ledger holds for pinned generations.
    pub ledger_pinned: u64,
    /// Bytes the prune could not reclaim because readers pin them.
    pub prune_retained: u64,
}

impl Reconciliation {
    /// Readers pin bytes the ledger was never told about.
    pub fn unaccounted(&self) -> bool {
        self.prune_retained > self.ledger_pinned
    }
}

pub struct Ledger {
    gate: Arc<HookGate>,
    identity: InvalidationIdentity,
    held: Mutex<BTreeMap<ResourceClass, u64>>,
}

impl std::fmt::Debug for Ledger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ledger")
            .field("census", &self.census())
            .finish_non_exhaustive()
    }
}

impl Ledger {
    pub fn new(gate: Arc<HookGate>, identity: InvalidationIdentity) -> Arc<Self> {
        Arc::new(Self {
            gate,
            identity,
            held: Mutex::new(BTreeMap::new()),
        })
    }

    pub fn census(&self) -> Census {
        let held = self
            .held
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut census = Census {
            held: held.clone(),
            resident: 0,
            disk: 0,
        };
        for (class, bytes) in held.iter().filter(|(class, _)| class.counted()) {
            match class.pool() {
                Pool::Resident => census.resident += bytes,
                Pool::Disk => census.disk += bytes,
            }
        }
        census
    }

    /// The bytes on disk under the store's generations directory, complete generations, staging residue, and corrupt entries alike: what the disk pool must count before any reservation. On-disk sizes, not manifests, so an entry whose manifest is unreadable still counts what it occupies.
    ///
    /// # Errors
    ///
    /// A generations directory that cannot be walked.
    pub fn store_bytes(store: &GenerationStore) -> Result<u64, Refusal> {
        fn walk(dir: &Path, total: &mut u64) -> std::io::Result<()> {
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                let metadata = entry.metadata()?;
                if metadata.is_dir() {
                    walk(&entry.path(), total)?;
                } else if metadata.is_file() {
                    *total = total.saturating_add(metadata.len());
                }
            }
            Ok(())
        }
        let mut total = 0u64;
        walk(&store.root().join(GENERATIONS_DIR_NAME), &mut total)
            .map_err(|error| Refusal::Store(error.kind().to_string()))?;
        Ok(total)
    }

    /// Reserves `bytes` of `class` against the pool's limit under `grant`; the `store_bytes` a disk reservation states are counted with what the ledger holds. Atomic with respect to every other reservation.
    ///
    /// # Errors
    ///
    /// The gate's denial when the total would exceed the limit or the limit is absent, an overflow, or a second charge of a static class.
    pub fn reserve(
        self: &Arc<Self>,
        grant: &Admission,
        class: ResourceClass,
        bytes: u64,
        store_bytes: u64,
    ) -> Result<Reservation, Refusal> {
        let mut held = self
            .held
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if class.is_static() && held.contains_key(&class) {
            return Err(Refusal::AlreadyCharged { class });
        }
        let pool = class.pool();
        let pool_held = held
            .iter()
            .filter(|(other, _)| other.pool() == pool && other.counted())
            .map(|(_, held)| *held)
            .try_fold(0u64, |sum, held| sum.checked_add(held))
            .ok_or(Refusal::Overflow { pool })?;
        let base = if pool == Pool::Disk { store_bytes } else { 0 };
        let increment = if class.counted() { bytes } else { 0 };
        let total = pool_held
            .checked_add(increment)
            .and_then(|total| total.checked_add(base))
            .ok_or(Refusal::Overflow { pool })?;
        self.gate
            .check_limits(grant, &self.identity, &[(pool.limit(), total)])?;
        *held.entry(class).or_insert(0) += bytes;
        Ok(Reservation {
            ledger: Arc::clone(self),
            class,
            bytes,
        })
    }

    /// Whether a composition of `deltas` deltas is within the manifest's delta bound.
    ///
    /// # Errors
    ///
    /// The gate's denial.
    pub fn admit_deltas(&self, grant: &Admission, deltas: usize) -> Result<(), Denial> {
        self.gate
            .check_limits(grant, &self.identity, &[(DELTA_LIMIT, deltas as u64)])
    }

    pub fn reconcile(&self, report: &PruneReport) -> Reconciliation {
        Reconciliation {
            ledger_pinned: self
                .census()
                .held
                .get(&ResourceClass::PinnedGenerations)
                .copied()
                .unwrap_or(0),
            prune_retained: report.retained_bytes,
        }
    }

    fn release(&self, class: ResourceClass, bytes: u64) {
        let mut held = self
            .held
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(entry) = held.get_mut(&class) {
            *entry = entry.saturating_sub(bytes);
            if *entry == 0 {
                held.remove(&class);
            }
        }
    }
}

/// Held bytes of one class; dropping it releases them. Nothing else does, so work that keeps its output keeps its charge.
#[derive(Debug)]
pub struct Reservation {
    ledger: Arc<Ledger>,
    class: ResourceClass,
    bytes: u64,
}

impl Reservation {
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub fn class(&self) -> ResourceClass {
        self.class
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.ledger.release(self.class, self.bytes);
    }
}
