//! Store primitives with no dependency on `crate::instance`.

use rustix::fd::OwnedFd;
use rustix::fs::{AtFlags, renameat};

/// Plain `dup` duplicates are inherited across exec; `fcntl_dupfd_cloexec` sets close-on-exec.
pub(crate) fn dup_cloexec<Fd: rustix::fd::AsFd>(fd: Fd) -> rustix::io::Result<OwnedFd> {
    rustix::io::fcntl_dupfd_cloexec(fd, 0)
}

/// `Ok(true)` means the rename happened; `Ok(false)` means `to` is occupied and nothing moved.
///
/// `RENAME_NOREPLACE` atomically rejects an occupied target on Linux. `EINVAL`, `ENOSYS`, and
/// `EOPNOTSUPP` mean the filesystem lacks `renameat2` flags. The fallback checks occupancy
/// first because plain `renameat` replaces an empty target directory. The fallback is not
/// atomic against a concurrent creator of `to`, so callers must exclude concurrent writers.
pub(crate) fn rename_no_replace(
    dir: &OwnedFd,
    from: &str,
    to: &str,
) -> Result<bool, rustix::io::Errno> {
    #[cfg(target_os = "linux")]
    {
        match rustix::fs::renameat_with(dir, from, dir, to, rustix::fs::RenameFlags::NOREPLACE) {
            Ok(()) => return Ok(true),
            Err(rustix::io::Errno::EXIST) | Err(rustix::io::Errno::NOTEMPTY) => return Ok(false),
            Err(rustix::io::Errno::INVAL)
            | Err(rustix::io::Errno::NOSYS)
            | Err(rustix::io::Errno::OPNOTSUPP) => {}
            Err(e) => return Err(e),
        }
    }
    match rustix::fs::statat(dir, to, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => return Ok(false),
        Err(rustix::io::Errno::NOENT) => {}
        Err(e) => return Err(e),
    }
    match renameat(dir, from, dir, to) {
        Ok(()) => Ok(true),
        Err(rustix::io::Errno::EXIST) | Err(rustix::io::Errno::NOTEMPTY) => Ok(false),
        Err(e) => Err(e),
    }
}
