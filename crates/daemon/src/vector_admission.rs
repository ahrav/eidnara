//! One ledger for every byte vector work holds, resident or on disk, judged against the manifest's vector limits through the evidence gate. A reservation is taken atomically against what is already held: the ledger locks its tally, adds the increment, asks the gate whether the total is within the limit, and records it only on a yes, so two reservations racing for the last bytes cannot both win. Dropping a reservation releases it; nothing else does, so cancellation of the work that took it releases nothing until that work lets go.
//! The disk pool counts the store's own generations as well: a disk reservation walks the store's generations directory itself, so staging and compaction are refused before they would push the store past its bound, not after. Every disk reserver runs under the lifecycle's exclusive transaction lock, which is what keeps one reserver's copy in flight from being counted twice by another's walk.
//! Generations a reader pins are already in that store total while they exist; the ledger records pins beside the pools, for the census and for reconciling a prune's readback, and never adds them to a limit.
//! Static residents (model memory, tokenizer, SQLite cache) are charged once each; a second charge for the same class is refused rather than doubled.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use host_runtime::generation::{GENERATIONS_DIR_NAME, GenerationStore, PruneReport};

use crate::projection_gates::{Admission, Denial, HookGate, InvalidationIdentity, VectorJudgment};

/// The limits the ledger's two pools and delta admission are judged against, named where the manifest parser accepts them so the two cannot drift.
pub use crate::projection_gates::{
    VECTOR_DELTA_COUNT as DELTA_LIMIT, VECTOR_DISK_BYTES as DISK_LIMIT,
    VECTOR_RESIDENT_BYTES as RESIDENT_LIMIT,
};

/// Implicit discriminants make `variant as usize` the variant's index in `ALL`, so arrays sized `ALL.len()` have a slot for every variant.
macro_rules! indexed_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $($(#[$variant_meta:meta])* $variant:ident,)+
        }
    ) => {
        $(#[$meta])*
        pub enum $name {
            $($(#[$variant_meta])* $variant,)+
        }

        impl $name {
            const ALL: [Self; [$(stringify!($variant)),+].len()] = [$(Self::$variant),+];
        }
    };
}

indexed_enum! {
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
    }
}

indexed_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Pool {
        Resident,
        Disk,
    }
}

impl ResourceClass {
    pub fn pool(self) -> Pool {
        match self {
            Self::ModelMemory
            | Self::TokenizerCache
            | Self::SqliteCache
            | Self::Text
            | Self::Scratch
            | Self::RowBuffers
            | Self::LayerTables => Pool::Resident,
            Self::Staging | Self::CompactionScratch => Pool::Disk,
        }
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
    #[error("a {pool:?} class was asked for the other pool")]
    WrongPool { pool: Pool },
    #[error("the {pool:?} total would leave the byte domain")]
    Overflow { pool: Pool },
    #[error("the lifecycle store could not be measured: {0}")]
    Store(String),
}

/// What the ledger holds at one instant: each pool's reserved bytes, the bytes readers pin, and the reservations by class. `disk` is the ledger's own disk reservations; the store's bytes join them only at reservation time.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Census {
    pub held: BTreeMap<ResourceClass, u64>,
    pub resident: u64,
    pub disk: u64,
    pub pinned: u64,
}

/// A prune's readback set against the ledger's pinned bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reconciliation {
    /// Bytes readers told the ledger they pin.
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

// `charged` holds one bit per class.
const _: () = assert!(ResourceClass::ALL.len() <= u16::BITS as usize);

/// `held` indexes classes by declaration order. `charged` tracks each class from its first successful reservation until a release leaves its held bytes at zero, so a zero-byte reservation still charges a static class. `pools` caches each pool's sum of `held` for constant-time reservation checks.
#[derive(Default)]
struct Tally {
    held: [u64; ResourceClass::ALL.len()],
    charged: u16,
    pools: [u64; Pool::ALL.len()],
    pinned: u64,
    judgment: VectorJudgment,
}

impl Tally {
    fn is_charged(&self, class: ResourceClass) -> bool {
        self.charged & (1 << class as u16) != 0
    }
}

pub struct Ledger {
    gate: Arc<HookGate>,
    identity: InvalidationIdentity,
    /// Locked before the gate's own lock and never the other way, so a reservation cannot deadlock against a gate operation.
    tally: Mutex<Tally>,
}

impl std::fmt::Debug for Ledger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ledger")
            .field("census", &self.census())
            .finish_non_exhaustive()
    }
}

impl Ledger {
    /// One ledger per daemon: the tally is this value's alone, so two ledgers over one gate each judge their own total against the whole limit and together exceed it. The component that wires vector work into the daemon constructs the one ledger and hands the same `Arc` to every charger.
    pub fn new(gate: Arc<HookGate>, identity: InvalidationIdentity) -> Arc<Self> {
        Arc::new(Self {
            gate,
            identity,
            tally: Mutex::new(Tally::default()),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Tally> {
        self.tally
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn census(&self) -> Census {
        let tally = self.lock();
        Census {
            held: ResourceClass::ALL
                .into_iter()
                .filter(|class| tally.is_charged(*class))
                .map(|class| (class, tally.held[class as usize]))
                .collect(),
            resident: tally.pools[Pool::Resident as usize],
            disk: tally.pools[Pool::Disk as usize],
            pinned: tally.pinned,
        }
    }

    /// Returns the total size of regular files under the store's generations directory. Uses file metadata instead of manifests so files with unreadable manifests still count. Does not follow symlinks.
    ///
    /// # Errors
    ///
    /// A directory under the generations directory that cannot be read.
    pub fn store_bytes(store: &GenerationStore) -> Result<u64, Refusal> {
        fn walk(generations: PathBuf) -> std::io::Result<u64> {
            let mut total = 0u64;
            let mut pending = vec![generations];
            while let Some(dir) = pending.pop() {
                for entry in std::fs::read_dir(&dir)? {
                    let entry = entry?;
                    // `DirEntry::metadata` does not follow symlinks, so a link is neither a directory nor a file here.
                    let metadata = entry.metadata()?;
                    if metadata.is_dir() {
                        pending.push(entry.path());
                    } else if metadata.is_file() {
                        total = total.saturating_add(metadata.len());
                    }
                }
            }
            Ok(total)
        }
        walk(store.root().join(GENERATIONS_DIR_NAME))
            .map_err(|error| Refusal::Store(error.kind().to_string()))
    }

    /// Reserves `bytes` of a resident class against `vector_resident_bytes` under `grant`, atomically with respect to every other reservation.
    ///
    /// # Errors
    ///
    /// The gate's denial when the total would exceed the limit or the limit is absent, an overflow, a second charge of a static class, or a disk class.
    pub fn reserve(
        self: &Arc<Self>,
        grant: &Admission,
        class: ResourceClass,
        bytes: u64,
    ) -> Result<Reservation, Refusal> {
        if class.pool() != Pool::Resident {
            return Err(Refusal::WrongPool { pool: Pool::Disk });
        }
        self.reserve_in(grant, class, bytes, 0)
    }

    /// Reserves `bytes` of a disk class against `vector_disk_bytes` on top of what `store` holds on disk, under `grant`. The caller holds the lifecycle's exclusive transaction lock, so no other disk reserver's copy is in flight during the walk.
    ///
    /// # Errors
    ///
    /// As [`Self::reserve`], a store that cannot be measured, or a resident class.
    pub fn reserve_disk(
        self: &Arc<Self>,
        grant: &Admission,
        class: ResourceClass,
        bytes: u64,
        store: &GenerationStore,
    ) -> Result<Reservation, Refusal> {
        if class.pool() != Pool::Disk {
            return Err(Refusal::WrongPool {
                pool: Pool::Resident,
            });
        }
        let base = Self::store_bytes(store)?;
        self.reserve_in(grant, class, bytes, base)
    }

    fn reserve_in(
        self: &Arc<Self>,
        grant: &Admission,
        class: ResourceClass,
        bytes: u64,
        base: u64,
    ) -> Result<Reservation, Refusal> {
        let pool = class.pool();
        let mut tally = self.lock();
        if class.is_static() && tally.is_charged(class) {
            return Err(Refusal::AlreadyCharged { class });
        }
        let held = tally.pools[pool as usize]
            .checked_add(bytes)
            .ok_or(Refusal::Overflow { pool })?;
        let total = held.checked_add(base).ok_or(Refusal::Overflow { pool })?;
        self.gate.check_vector_limit(
            &mut tally.judgment,
            grant,
            &self.identity,
            pool.limit(),
            total,
        )?;
        // A class holds at most its pool's total, so the class sum fits wherever the pool sum does.
        tally.held[class as usize] += bytes;
        tally.charged |= 1 << class as u16;
        tally.pools[pool as usize] = held;
        Ok(Reservation {
            ledger: Arc::clone(self),
            class,
            bytes,
        })
    }

    /// Records `bytes` a reader pins on disk. The store's total already holds them, so no limit is consulted; the record is what a prune's readback is reconciled against.
    pub fn pin(self: &Arc<Self>, bytes: u64) -> Pinned {
        let mut tally = self.lock();
        tally.pinned = tally.pinned.saturating_add(bytes);
        Pinned {
            ledger: Arc::clone(self),
            bytes,
        }
    }

    /// Whether a composition of `deltas` deltas is within the manifest's delta bound. The count is the composition's own, so nothing is held; publication asks before it stages the record.
    ///
    /// # Errors
    ///
    /// The gate's denial.
    pub fn admit_deltas(&self, grant: &Admission, deltas: usize) -> Result<(), Denial> {
        let mut tally = self.lock();
        self.gate.check_vector_limit(
            &mut tally.judgment,
            grant,
            &self.identity,
            DELTA_LIMIT,
            deltas as u64,
        )
    }

    pub fn reconcile(&self, report: &PruneReport) -> Reconciliation {
        Reconciliation {
            ledger_pinned: self.lock().pinned,
            prune_retained: report.retained_bytes,
        }
    }

    fn release(&self, class: ResourceClass, bytes: u64) {
        let mut tally = self.lock();
        if tally.is_charged(class) {
            let released = bytes.min(tally.held[class as usize]);
            tally.held[class as usize] -= released;
            tally.pools[class.pool() as usize] -= released;
            if tally.held[class as usize] == 0 {
                tally.charged &= !(1 << class as u16);
            }
        }
    }

    fn unpin(&self, bytes: u64) {
        let mut tally = self.lock();
        tally.pinned = tally.pinned.saturating_sub(bytes);
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
    /// The ledger this reservation is held in; work that extends what the reservation holds charges the same ledger.
    pub fn ledger(&self) -> &Arc<Ledger> {
        &self.ledger
    }

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

/// Bytes a reader pins on disk, recorded until dropped.
#[derive(Debug)]
pub struct Pinned {
    ledger: Arc<Ledger>,
    bytes: u64,
}

impl Pinned {
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

impl Drop for Pinned {
    fn drop(&mut self) {
        self.ledger.unpin(self.bytes);
    }
}
