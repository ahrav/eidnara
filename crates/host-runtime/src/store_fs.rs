//! Filesystem primitives shared by the closure and generation stores.
//!
//! Every helper returns a raw `rustix`/`std::io` error so each store can map it onto its own
//! closed error type.

use std::io;
use std::time::{Duration, SystemTime};

use rustix::fd::OwnedFd;
use rustix::fs::{AtFlags, Mode, OFlags, fsync, mkdirat, openat, renameat, unlinkat};
use sha2::Digest;

use crate::file_mode::raw_mode;
pub(crate) use crate::instance::HARDENED_DIR_FLAGS;
use crate::instance::{S_IFDIR, S_IFMT, hex, mode_bits, owner_uid, write_all_fd};

/// Sweeps treat a temp whose mtime is older than this as abandoned.
pub(crate) const STALE_TEMP_AFTER: Duration = Duration::from_secs(600);

/// Unreadable or future mtimes count as live: only a provably old entry is reclaimable.
pub(crate) fn is_stale_mtime(mtime_secs: i64) -> bool {
    let Ok(secs) = u64::try_from(mtime_secs) else {
        return false;
    };
    let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
    SystemTime::now()
        .duration_since(modified)
        .is_ok_and(|age| age >= STALE_TEMP_AFTER)
}

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

/// Opens `rel` under `dir` component by component with `O_NOFOLLOW`, so no intermediate or
/// final symlink is followed. Empty, `.`, and `..` components fail with `EINVAL`.
///
/// A non-directory final component is opened with `O_NONBLOCK`: a planted FIFO passes
/// `O_NOFOLLOW`, and without `NONBLOCK` opening it would block uncancellably.
pub(crate) fn open_rel_nofollow(
    dir: &OwnedFd,
    rel: &str,
    final_is_dir: bool,
) -> rustix::io::Result<OwnedFd> {
    let mut components = rel.split('/').peekable();
    let mut current: Option<OwnedFd> = None;
    while let Some(component) = components.next() {
        if component.is_empty() || component == "." || component == ".." {
            return Err(rustix::io::Errno::INVAL);
        }
        let at = current.as_ref().unwrap_or(dir);
        let flags = if components.peek().is_none() && !final_is_dir {
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK
        } else {
            HARDENED_DIR_FLAGS
        };
        current = Some(openat(at, component, flags, Mode::empty())?);
    }
    current.ok_or(rustix::io::Errno::INVAL)
}

/// `mkdirat` applies the umask; `chmodat` on the just-created name restores `0o700` before the
/// no-follow reopen, and `fchmod` on the pinned descriptor makes the mode independent of the name.
/// The caller's own `mkdirat` of `name` must have succeeded.
pub(crate) fn open_created_dir(parent: &OwnedFd, name: &str) -> rustix::io::Result<OwnedFd> {
    rustix::fs::chmodat(parent, name, Mode::from_raw_mode(0o700), AtFlags::empty())?;
    let fd = openat(parent, name, HARDENED_DIR_FLAGS, Mode::empty())?;
    rustix::fs::fchmod(&fd, Mode::from_raw_mode(0o700))?;
    Ok(fd)
}

/// Creates `name` under `parent` as an owner-only directory and returns its pinned descriptor.
pub(crate) fn create_owned_dir(parent: &OwnedFd, name: &str) -> rustix::io::Result<OwnedFd> {
    mkdirat(parent, name, Mode::from_raw_mode(0o700))?;
    open_created_dir(parent, name)
}

/// Removal accepts directories owned by `owner_uid()` regardless of their mode bits, so a
/// stale entry with a foreign mode can still be reclaimed. A foreign owner fails with `EPERM`.
pub(crate) fn open_dir_for_removal(parent: &OwnedFd, name: &str) -> rustix::io::Result<OwnedFd> {
    let fd = openat(parent, name, HARDENED_DIR_FLAGS, Mode::empty())?;
    let stat = rustix::fs::fstat(&fd)?;
    if mode_bits(&stat) & S_IFMT != S_IFDIR || stat.st_uid != owner_uid() {
        return Err(rustix::io::Errno::PERM);
    }
    Ok(fd)
}

/// Removes `name` under `parent`, recursing through owned directories. A missing entry is `Ok`.
pub(crate) fn remove_tree(parent: &OwnedFd, name: &str) -> rustix::io::Result<()> {
    match unlinkat(parent, name, AtFlags::empty()) {
        Ok(()) | Err(rustix::io::Errno::NOENT) => return Ok(()),
        // Linux reports EPERM for unlink-on-directory on some filesystems.
        Err(rustix::io::Errno::ISDIR) | Err(rustix::io::Errno::PERM) => {}
        Err(e) => return Err(e),
    }
    let dir = open_dir_for_removal(parent, name)?;
    for child in read_dir_names(&dir)? {
        remove_tree(&dir, &child)?;
    }
    unlinkat(parent, name, AtFlags::REMOVEDIR)
}

/// `read_dir_names` enumerates the already-open directory, so a pathname swap cannot change
/// the listing, and excludes `.` and `..` so callers cannot delete the directory or its parent.
/// A non-UTF-8 name fails with `EILSEQ`.
pub(crate) fn read_dir_names(dir: &OwnedFd) -> rustix::io::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in rustix::fs::Dir::read_from(dir)? {
        let raw = entry?.file_name().to_bytes().to_owned();
        if raw == b"." || raw == b".." {
            continue;
        }
        names.push(String::from_utf8(raw).map_err(|_| rustix::io::Errno::ILSEQ)?);
    }
    Ok(names)
}

/// Creates `name` exclusively, writes `bytes`, fsyncs, and returns the still-open descriptor so
/// the caller can verify the inode it wrote.
pub(crate) fn write_new_file(
    dir: &OwnedFd,
    name: &str,
    bytes: &[u8],
    mode: u32,
) -> io::Result<OwnedFd> {
    let fd = openat(
        dir,
        name,
        OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(raw_mode(mode)),
    )?;
    rustix::fs::fchmod(&fd, Mode::from_raw_mode(raw_mode(mode)))?;
    write_all_fd(&fd, bytes)?;
    fsync(&fd)?;
    Ok(fd)
}

/// Reads `source` to EOF, hashing every byte and copying it to `destination` when given.
/// Returns the byte count and lowercase SHA-256 hex. Once the count exceeds `cap` the copy
/// fails with `InvalidData`, so a source that grows mid-copy cannot overrun its manifest size.
pub(crate) fn hash_copy(
    source: &OwnedFd,
    destination: Option<&OwnedFd>,
    cap: u64,
) -> io::Result<(u64, String)> {
    let mut hasher = sha2::Sha256::new();
    let mut total = 0u64;
    let mut buffer = vec![0u8; 128 * 1024];
    loop {
        let count = rustix::io::read(source, &mut buffer)?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count as u64)
            .filter(|total| *total <= cap)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "source grew past its cap")
            })?;
        hasher.update(&buffer[..count]);
        if let Some(destination) = destination {
            write_all_fd(destination, &buffer[..count])?;
        }
    }
    Ok((total, hex(&hasher.finalize())))
}
