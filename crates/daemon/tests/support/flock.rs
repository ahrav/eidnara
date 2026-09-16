//! A competing lock attempt as an independent witness: whoever holds `path` shared or exclusive makes the exclusive probe fail.

use std::fs::File;
use std::path::Path;

use rustix::fs::FlockOperation;

/// Tries an exclusive, non-blocking flock on `path` and releases it at once; `false` means another holder is present.
pub fn try_exclusive(path: &Path) -> bool {
    let file = File::open(path).expect("lock target opens");
    let locked = rustix::fs::flock(&file, FlockOperation::NonBlockingLockExclusive).is_ok();
    if locked {
        let _ = rustix::fs::flock(&file, FlockOperation::Unlock);
    }
    locked
}
