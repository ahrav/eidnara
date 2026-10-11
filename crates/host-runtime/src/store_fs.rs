//! Filesystem primitives shared by the closure and generation stores.
//!
//! Every helper returns a raw `rustix`/`std::io` error so each store can map it onto its own
//! closed error type.

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::unix::ffi::OsStringExt;
use std::path::Path;
use std::time::{Duration, SystemTime};

use rustix::fd::OwnedFd;
use rustix::fs::{AtFlags, Mode, OFlags, fsync, mkdirat, openat, renameat, unlinkat};
use sha2::Digest;

use crate::file_mode::raw_mode;
pub(crate) use crate::instance::HARDENED_DIR_FLAGS;
use crate::instance::{
    S_IFDIR, S_IFMT, hex, mode_bits, open_secure_dir_existing, owner_uid, stat_identity,
    write_all_fd,
};

/// Validators walk a tree recursively and hold one directory descriptor per level, so a path
/// may have at most this many components; the bound keeps the walk far below common
/// `RLIMIT_NOFILE` soft limits.
pub(crate) const MAX_PATH_COMPONENTS: usize = 128;

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

/// `true` when two `stat` results describe the same unchanged inode.
///
/// `ctime` is included because an unprivileged owner can rewrite a file and then restore its
/// `mtime` with `utimensat`, but cannot set `ctime`; every write and metadata change bumps it.
pub(crate) fn same_snapshot(left: &rustix::fs::Stat, right: &rustix::fs::Stat) -> bool {
    #[allow(clippy::unnecessary_cast)]
    {
        left.st_dev as u64 == right.st_dev as u64
            && left.st_ino as u64 == right.st_ino as u64
            && left.st_size == right.st_size
            && left.st_mtime == right.st_mtime
            && left.st_mtime_nsec == right.st_mtime_nsec
            && left.st_ctime == right.st_ctime
            && left.st_ctime_nsec == right.st_ctime_nsec
    }
}

/// `Ok(true)` when the directory `path` currently names is the same inode as `retained`.
/// A mutator writing through a pinned descriptor must confirm that `path` still resolves to `retained` before reporting success; otherwise writes landed in a detached tree.
pub(crate) fn path_names_descriptor(path: &Path, retained: &OwnedFd) -> rustix::io::Result<bool> {
    let by_name = match open_secure_dir_existing(path) {
        Ok(Some(fd)) => fd,
        Ok(None) => return Ok(false),
        Err(_) => return Ok(false),
    };
    let named = rustix::fs::fstat(&by_name)?;
    let pinned = rustix::fs::fstat(retained)?;
    Ok(stat_identity(&named) == stat_identity(&pinned))
}

/// Atomically swaps the entries `a` and `b` under `dir`; both must exist.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn exchange_dirs(dir: &OwnedFd, a: &str, b: &str) -> rustix::io::Result<()> {
    rustix::fs::renameat_with(dir, a, dir, b, rustix::fs::RenameFlags::EXCHANGE)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn exchange_dirs(_dir: &OwnedFd, _a: &str, _b: &str) -> rustix::io::Result<()> {
    Err(rustix::io::Errno::OPNOTSUPP)
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

pub(crate) fn is_owned_private_dir(stat: &rustix::fs::Stat) -> bool {
    let mode = mode_bits(stat);
    mode & S_IFMT == S_IFDIR && stat.st_uid == owner_uid() && mode & 0o7777 == 0o700
}

/// `O_NOFOLLOW` on a multi-component `openat` protects only the final component.
/// Components are opened one at a time relative to the previous pinned descriptor.
/// `EPERM` reports an existing component that is not an owner-only directory.
pub(crate) fn open_or_create_parents(
    root: &OwnedFd,
    relative: &str,
) -> rustix::io::Result<(OwnedFd, String)> {
    let mut parts = relative.split('/').peekable();
    let mut current = dup_cloexec(root)?;
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            return Ok((current, part.to_owned()));
        }
        let next = match mkdirat(&current, part, Mode::from_raw_mode(0o700)) {
            Ok(()) => open_created_dir(&current, part)?,
            Err(rustix::io::Errno::EXIST) => {
                openat(&current, part, HARDENED_DIR_FLAGS, Mode::empty())?
            }
            Err(e) => return Err(e),
        };
        if !is_owned_private_dir(&rustix::fs::fstat(&next)?) {
            return Err(rustix::io::Errno::PERM);
        }
        current = next;
    }
    Err(rustix::io::Errno::INVAL)
}

/// Removal accepts directories owned by `owner_uid()` regardless of their mode bits, so a
/// stale entry with a foreign mode can still be reclaimed. A foreign owner fails with `EPERM`.
pub(crate) fn open_dir_for_removal<N: AsRef<OsStr> + ?Sized>(
    parent: &OwnedFd,
    name: &N,
) -> rustix::io::Result<OwnedFd> {
    let fd = openat(parent, name.as_ref(), HARDENED_DIR_FLAGS, Mode::empty())?;
    let stat = rustix::fs::fstat(&fd)?;
    if mode_bits(&stat) & S_IFMT != S_IFDIR || stat.st_uid != owner_uid() {
        return Err(rustix::io::Errno::PERM);
    }
    Ok(fd)
}

/// Removes `name` under `parent`, recursing through owned directories. A missing entry is `Ok`
/// and an entry with a foreign owner fails with `EPERM`.
///
/// Ownership is checked on the entry itself before any unlink or open, because both `unlink`
/// and `rmdir` are authorized by the writable parent alone and would otherwise remove a
/// foreign-owned file, symlink, or empty directory that happens to carry a managed-looking
/// name. A directory that passes that check but cannot be opened for traversal (mode `000`
/// left by a crash between `mkdirat` and `chmodat`) is still removed when it is empty.
pub(crate) fn remove_tree<N: AsRef<OsStr> + ?Sized>(
    parent: &OwnedFd,
    name: &N,
) -> rustix::io::Result<()> {
    let name = name.as_ref();
    let stat = match rustix::fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(rustix::io::Errno::NOENT) => return Ok(()),
        Err(e) => return Err(e),
    };
    if stat.st_uid != owner_uid() {
        return Err(rustix::io::Errno::PERM);
    }
    if mode_bits(&stat) & S_IFMT != S_IFDIR {
        return match unlinkat(parent, name, AtFlags::empty()) {
            Ok(()) | Err(rustix::io::Errno::NOENT) => Ok(()),
            Err(e) => Err(e),
        };
    }
    let dir = match open_dir_for_removal(parent, name) {
        Ok(dir) => dir,
        // The opened inode had a foreign owner; the pathname check above was raced.
        Err(rustix::io::Errno::PERM) => return Err(rustix::io::Errno::PERM),
        Err(open_error) => {
            return unlinkat(parent, name, AtFlags::REMOVEDIR).map_err(|_| open_error);
        }
    };
    for child in read_dir_entries(&dir)? {
        remove_tree(&dir, &child)?;
    }
    unlinkat(parent, name, AtFlags::REMOVEDIR)
}

/// Enumerates the already-open directory, so a pathname swap cannot change the listing, and
/// excludes `.` and `..` so callers cannot delete the directory or its parent.
pub(crate) fn read_dir_entries(dir: &OwnedFd) -> rustix::io::Result<Vec<OsString>> {
    let mut names = Vec::new();
    for entry in rustix::fs::Dir::read_from(dir)? {
        let raw = entry?.file_name().to_bytes().to_owned();
        if raw == b"." || raw == b".." {
            continue;
        }
        names.push(OsString::from_vec(raw));
    }
    Ok(names)
}

/// UTF-8 names only; a non-UTF-8 entry fails with `EILSEQ`. Validators use this form because
/// any entry they cannot name is an entry they cannot match against a manifest.
pub(crate) fn read_dir_names(dir: &OwnedFd) -> rustix::io::Result<Vec<String>> {
    read_dir_entries(dir)?
        .into_iter()
        .map(|name| name.into_string().map_err(|_| rustix::io::Errno::ILSEQ))
        .collect()
}

/// UTF-8 names plus the count of entries whose names are not UTF-8. Sweeps use this form so one
/// foreign entry cannot abort reclamation of everything else.
pub(crate) fn read_dir_names_partitioned(
    dir: &OwnedFd,
) -> rustix::io::Result<(Vec<String>, usize)> {
    let mut names = Vec::new();
    let mut unnamed = 0;
    for name in read_dir_entries(dir)? {
        match name.into_string() {
            Ok(name) => names.push(name),
            Err(_) => unnamed += 1,
        }
    }
    Ok((names, unnamed))
}

/// `true` when `name` is `prefix` followed by exactly `hex_len` lowercase hex digits, the only
/// shape the stores' own temp creators produce. Sweeps must not reclaim a merely similar name.
pub(crate) fn is_temp_name(name: &str, prefix: &str, hex_len: usize) -> bool {
    name.strip_prefix(prefix).is_some_and(|suffix| {
        suffix.len() == hex_len
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

/// Creates `name` exclusively, writes `bytes`, fsyncs, and returns the still-open descriptor so
/// the caller can verify the inode it wrote.
pub(crate) fn write_new_file(
    dir: &OwnedFd,
    name: &str,
    bytes: &[u8],
    mode: u32,
) -> io::Result<OwnedFd> {
    let fd = write_new_file_unsynced(dir, name, bytes, mode)?;
    fsync(&fd)?;
    Ok(fd)
}

/// The caller must sync the returned descriptor to make `bytes` durable.
pub(crate) fn write_new_file_unsynced(
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
    Ok(fd)
}

pub(crate) const CONCURRENT_SYNCS: usize = 8;

/// Fsyncs every descriptor in `files` and returns the first failure. Up to [`CONCURRENT_SYNCS`] syncs run at once, so a journaling filesystem can make several files durable with one log commit and one device cache flush. A sync whose thread cannot be spawned runs on the calling thread. Every sync has returned when this returns, whatever its result.
pub(crate) fn fsync_all(files: &[OwnedFd]) -> rustix::io::Result<()> {
    let mut result = Ok(());
    for group in files.chunks(CONCURRENT_SYNCS) {
        let (first, rest) = group.split_first().expect("chunks are nonempty");
        let synced = std::thread::scope(|scope| {
            let mut synced = Ok(());
            let mut others = Vec::with_capacity(rest.len());
            for fd in rest {
                match std::thread::Builder::new().spawn_scoped(scope, move || fsync(fd)) {
                    Ok(handle) => others.push(handle),
                    Err(_) => synced = synced.and(fsync(fd)),
                }
            }
            synced = synced.and(fsync(first));
            for other in others {
                synced = synced.and(other.join().unwrap_or(Err(rustix::io::Errno::IO)));
            }
            synced
        });
        result = result.and(synced);
    }
    result
}

pub(crate) const HASH_BUFFER_BYTES: usize = 128 * 1024;

/// Reads `source` to EOF, hashing every byte and copying it to `destination` when given.
/// Returns the byte count and lowercase SHA-256 hex. Once the count exceeds `cap` the copy
/// fails with `InvalidData`, so a source that grows mid-copy cannot overrun its manifest size.
pub(crate) fn hash_copy(
    source: &OwnedFd,
    destination: Option<&OwnedFd>,
    cap: u64,
) -> io::Result<(u64, String)> {
    hash_copy_from(source, destination, cap, sha2::Sha256::new(), 0)
}

/// [`hash_copy`] from `source`'s current offset, continuing a `hasher` that has already taken the `total` bytes before it.
pub(crate) fn hash_copy_from(
    source: &OwnedFd,
    destination: Option<&OwnedFd>,
    cap: u64,
    hasher: sha2::Sha256,
    total: u64,
) -> io::Result<(u64, String)> {
    let mut buffer = vec![0u8; HASH_BUFFER_BYTES];
    hash_copy_with(source, destination, cap, hasher, total, &mut buffer)
}

/// Chunk size, and the number of chunks in flight, for [`hash_pipelined`].
const PIPELINE_CHUNK_BYTES: usize = 1024 * 1024;
const PIPELINE_CHUNKS: usize = 3;

/// [`hash_copy`] without a destination, with one thread reading `source` while the calling thread hashes, so a
/// large file costs about its hash time rather than its read time plus its hash time.
///
/// A reader thread that cannot start leaves the whole file to [`hash_copy`] on the calling thread.
pub(crate) fn hash_pipelined(source: &OwnedFd, cap: u64) -> io::Result<(u64, String)> {
    let (filled, chunks) = std::sync::mpsc::sync_channel::<io::Result<Vec<u8>>>(PIPELINE_CHUNKS);
    let (recycle, empties) = std::sync::mpsc::sync_channel::<Vec<u8>>(PIPELINE_CHUNKS);
    for _ in 0..PIPELINE_CHUNKS {
        recycle
            .send(Vec::with_capacity(PIPELINE_CHUNK_BYTES))
            .map_err(|_| io::Error::other("hash pipeline closed"))?;
    }
    std::thread::scope(|scope| {
        let read_ahead = move || {
            let mut total = 0u64;
            while let Ok(mut chunk) = empties.recv() {
                chunk.resize(PIPELINE_CHUNK_BYTES, 0);
                let count = loop {
                    match rustix::io::read(source, &mut chunk) {
                        Err(rustix::io::Errno::INTR) => {}
                        result => break result,
                    }
                };
                let next = match count {
                    Ok(count) => match total.checked_add(count as u64).filter(|sum| *sum <= cap) {
                        Some(sum) => {
                            total = sum;
                            chunk.truncate(count);
                            Ok(chunk)
                        }
                        None => Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "source grew past its cap",
                        )),
                    },
                    Err(errno) => Err(errno.into()),
                };
                let last = next.as_ref().map_or(true, Vec::is_empty);
                if filled.send(next).is_err() || last {
                    return;
                }
            }
        };
        if std::thread::Builder::new()
            .name("hash-read".to_owned())
            .spawn_scoped(scope, read_ahead)
            .is_err()
        {
            return hash_copy(source, None, cap);
        }
        let mut hasher = sha2::Sha256::new();
        let mut total = 0u64;
        loop {
            let chunk = chunks
                .recv()
                .map_err(|_| io::Error::other("hash pipeline reader stopped"))??;
            if chunk.is_empty() {
                return Ok((total, hex(&hasher.finalize())));
            }
            hasher.update(&chunk);
            total += chunk.len() as u64;
            // The reader may already have finished, which drops the recycling receiver.
            let _ = recycle.send(chunk);
        }
    })
}

/// [`hash_copy_from`] reading through caller-owned storage, so callers can reuse one buffer across files.
pub(crate) fn hash_copy_with(
    source: &OwnedFd,
    destination: Option<&OwnedFd>,
    cap: u64,
    mut hasher: sha2::Sha256,
    mut total: u64,
    buffer: &mut [u8],
) -> io::Result<(u64, String)> {
    loop {
        let count = match rustix::io::read(source, &mut *buffer) {
            Err(rustix::io::Errno::INTR) => continue,
            result => result?,
        };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fsync_all_syncs_every_group_and_reports_a_failure_in_any_of_them() {
        let dir = tempfile::tempdir().expect("temp dir");
        let dir_fd = openat(
            rustix::fs::CWD,
            dir.path(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .expect("open dir");
        let mut files: Vec<OwnedFd> = (0..CONCURRENT_SYNCS * 2 + 3)
            .map(|index| {
                write_new_file_unsynced(&dir_fd, &format!("file-{index}"), b"bytes", 0o600)
                    .expect("write file")
            })
            .collect();
        assert_eq!(fsync_all(&files), Ok(()));
        assert_eq!(fsync_all(&[]), Ok(()));
        let (reader, _writer) = rustix::pipe::pipe().expect("pipe");
        files.insert(CONCURRENT_SYNCS + 1, reader);
        assert_eq!(fsync_all(&files), Err(rustix::io::Errno::INVAL));
    }
}
