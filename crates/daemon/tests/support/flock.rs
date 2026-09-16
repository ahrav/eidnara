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

/// Opens the lifecycle control directory, retrying `WouldBlock` for up to five seconds.
/// `open` refuses without waiting while the directory's flock is held, and in a test
/// binary that spawns children a sibling's fork carries every held flock until its exec.
pub fn open_lifecycle(data_home: &Path) -> daemon::projection_lifecycle::ProjectionLifecycle {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match daemon::projection_lifecycle::ProjectionLifecycle::open(data_home) {
            Ok(lifecycle) => return lifecycle,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the lifecycle directory stayed locked for five seconds"
                );
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(error) => panic!("{error}"),
        }
    }
}
