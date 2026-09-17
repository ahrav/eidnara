#[cfg(any(test, feature = "test-support"))]
use std::collections::BTreeMap;
#[cfg(any(test, feature = "test-support"))]
use std::sync::{Mutex, PoisonError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Boundary {
    Kernel,
    Projection,
}

/// Projection tables carry no per-write intent, so every projection write declares this cause.
pub const PROJECTION_CAUSE: &str = "projection";

#[cfg(any(test, feature = "test-support"))]
static COUNTS: Mutex<BTreeMap<(Boundary, String), u64>> = Mutex::new(BTreeMap::new());

/// A projection batch records under the caller's transaction, so a batch the caller rolls back is still counted.
#[cfg(any(test, feature = "test-support"))]
pub fn record(boundary: Boundary, cause: &str) {
    let mut counts = COUNTS.lock().unwrap_or_else(PoisonError::into_inner);
    *counts.entry((boundary, cause.to_owned())).or_insert(0) += 1;
}

#[cfg(not(any(test, feature = "test-support")))]
#[inline(always)]
pub fn record(_boundary: Boundary, _cause: &str) {}

#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot(BTreeMap<(Boundary, String), u64>);

#[cfg(any(test, feature = "test-support"))]
impl Snapshot {
    pub fn take() -> Self {
        Self(
            COUNTS
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
        )
    }

    pub fn count(&self, boundary: Boundary, cause: &str) -> u64 {
        self.0
            .get(&(boundary, cause.to_owned()))
            .copied()
            .unwrap_or(0)
    }
}
