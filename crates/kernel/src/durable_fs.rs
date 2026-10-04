//! Descriptor-anchored filesystem primitives for durable artifact publication.

use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Write};
use std::os::unix::fs::{FileExt, MetadataExt, PermissionsExt};
use std::sync::atomic::{AtomicU64, Ordering};

use rustix::fs::{self as rfs, AtFlags, Mode, OFlags};

static UNIQUE_ID: AtomicU64 = AtomicU64::new(0);

/// Filesystem failure classified by whether storage capacity is exhausted.
#[derive(thiserror::Error, Debug)]
pub(super) enum StorageError {
    #[error("{0}")]
    Exhausted(#[source] io::Error),
    #[error("{0}")]
    Other(#[source] io::Error),
}

impl StorageError {
    pub(super) fn raw_os_error(&self) -> Option<i32> {
        match self {
            Self::Exhausted(source) | Self::Other(source) => source.raw_os_error(),
        }
    }
}

/// Result of no-replace publication under the caller's publisher lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PublishOutcome {
    Published,
    // The rename fallback linked `final_name` but could neither unlink the
    // temp name nor roll the link back; the caller must remove or retry
    // removal of the temp link.
    PublishedTempRetained,
    AlreadyExists,
}

pub(super) fn classify_io(source: io::Error) -> StorageError {
    let exhausted = matches!(
        source.raw_os_error(),
        Some(code)
            if code == rustix::io::Errno::NOSPC.raw_os_error()
                || code == rustix::io::Errno::DQUOT.raw_os_error()
    );
    if exhausted {
        StorageError::Exhausted(source)
    } else {
        StorageError::Other(source)
    }
}

pub(super) fn classify_errno(source: rustix::io::Errno) -> StorageError {
    classify_io(io::Error::from(source))
}

fn invalid_name() -> StorageError {
    classify_io(io::Error::new(
        io::ErrorKind::InvalidInput,
        "filesystem name must be one normal component",
    ))
}

fn validate_name(name: &str) -> Result<(), StorageError> {
    validate_os_name(OsStr::new(name))
}

// `OsStr` preserves non-UTF-8 names.
fn validate_os_name(name: &OsStr) -> Result<(), StorageError> {
    let path = std::path::Path::new(name);
    if name.is_empty() || name == "." || name == ".." || path.file_name() != Some(name) {
        return Err(invalid_name());
    }
    Ok(())
}

/// Creates an owner-only directory relative to `parent` and syncs both descriptors.
///
/// The name must be one path component. Symlinks are never followed. The
/// directory is created under a temporary name, opened, secured, and verified
/// through that descriptor, then renamed into place without replacing an
/// existing entry; the returned descriptor is therefore the inode this call
/// created, not whatever `name` resolves to afterwards. On failure, cleanup is
/// best effort and the original classified I/O error is returned. An existing
/// entry at `name` surfaces as `AlreadyExists`.
pub(super) fn create_secure_directory(parent: &File, name: &OsStr) -> Result<File, StorageError> {
    create_secure_directory_inner(parent, name, None)
}

/// `hook` runs after the directory exists under its temporary name and before
/// it is renamed into place, where a concurrent writer could occupy `name`.
fn create_secure_directory_inner(
    parent: &File,
    name: &OsStr,
    mut hook: Option<&mut dyn FnMut()>,
) -> Result<File, StorageError> {
    validate_os_name(name)?;
    let temp = temp_name("dir");
    rfs::mkdirat(parent, temp.as_str(), Mode::from_raw_mode(0o700)).map_err(classify_errno)?;
    let secured = (|| {
        let directory = open_new_owner_only_directory(parent, &temp)?;
        sync_directory(&directory)?;
        if let Some(hook) = hook.as_mut() {
            hook();
        }
        match rfs::renameat_with(
            parent,
            temp.as_str(),
            parent,
            name,
            rfs::RenameFlags::NOREPLACE,
        ) {
            Ok(()) => {}
            Err(rustix::io::Errno::EXIST) | Err(rustix::io::Errno::NOTEMPTY) => {
                return Err(classify_errno(rustix::io::Errno::EXIST));
            }
            Err(error) => return Err(classify_errno(error)),
        }
        sync_directory(parent)?;
        Ok(directory)
    })();
    if secured.is_err() {
        let _ = rfs::unlinkat(parent, temp.as_str(), AtFlags::REMOVEDIR);
    }
    secured
}

/// Opens the directory `temp` just created under `parent` without following symlinks and makes it owner-only.
fn open_new_owner_only_directory(parent: &File, temp: &str) -> Result<File, StorageError> {
    let descriptor = rfs::openat(
        parent,
        temp,
        OFlags::DIRECTORY | OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(classify_errno)?;
    let directory = File::from(descriptor);
    // `fchmod` on the descriptor defeats the umask without re-resolving a name.
    rfs::fchmod(&directory, Mode::from_raw_mode(0o700)).map_err(classify_errno)?;
    let metadata = directory.metadata().map_err(classify_io)?;
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err(classify_io(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "directory is not owner-only",
        )));
    }
    Ok(directory)
}

/// Creates each of `names` under `parent` as [`create_secure_directory`] does: every new directory is synced before it is renamed into place and `parent` is synced after the renames. The new directories sync concurrently and `parent` syncs once. A name some other writer created first is opened with [`open_secure_directory`].
pub(super) fn create_secure_directories(
    parent: &File,
    names: &[&str],
) -> Result<Vec<File>, StorageError> {
    for name in names {
        validate_name(name)?;
    }
    let mut temps: Vec<String> = Vec::with_capacity(names.len());
    let mut renamed = false;
    let created = (|| {
        let renamed = &mut renamed;
        let mut made = Vec::with_capacity(names.len());
        for _ in names {
            let temp = temp_name("dir");
            rfs::mkdirat(parent, temp.as_str(), Mode::from_raw_mode(0o700))
                .map_err(classify_errno)?;
            temps.push(temp);
            made.push(open_new_owner_only_directory(
                parent,
                temps.last().expect("pushed"),
            )?);
        }
        sync_all_concurrently(&made.iter().collect::<Vec<_>>())?;
        let mut directories = Vec::with_capacity(names.len());
        for ((name, temp), directory) in names.iter().zip(&temps).zip(made) {
            let renaming = rfs::renameat_with(
                parent,
                temp.as_str(),
                parent,
                *name,
                rfs::RenameFlags::NOREPLACE,
            );
            match renaming {
                Ok(()) => {
                    *renamed = true;
                    directories.push(directory);
                }
                Err(rustix::io::Errno::EXIST) | Err(rustix::io::Errno::NOTEMPTY) => {
                    let _ = rfs::unlinkat(parent, temp.as_str(), AtFlags::REMOVEDIR);
                    directories.push(open_secure_directory(parent, name)?);
                }
                Err(error) => return Err(classify_errno(error)),
            }
        }
        Ok(directories)
    })();
    if created.is_err() {
        for temp in &temps {
            let _ = rfs::unlinkat(parent, temp.as_str(), AtFlags::REMOVEDIR);
        }
    }
    // A directory renamed into place before a later failure stays, so its entry is made durable either way; the earlier failure is the one reported.
    if renamed {
        let synced = sync_directory(parent);
        if created.is_ok() {
            synced?;
        }
    }
    created
}

/// Opens an owner-only directory without following symlinks.
///
/// Returns an error unless the object is a directory owned by the effective UID
/// with mode `0700`.
pub(super) fn open_secure_directory(parent: &File, name: &str) -> Result<File, StorageError> {
    validate_name(name)?;
    let descriptor = rfs::openat(
        parent,
        name,
        OFlags::DIRECTORY | OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(classify_errno)?;
    let directory = File::from(descriptor);
    let metadata = directory.metadata().map_err(classify_io)?;
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err(classify_io(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "directory is not owner-only",
        )));
    }
    Ok(directory)
}

pub(super) fn open_or_create_secure_directory(
    parent: &File,
    name: &str,
) -> Result<File, StorageError> {
    match open_secure_directory(parent, name) {
        Err(StorageError::Other(source)) if source.kind() == io::ErrorKind::NotFound => {}
        opened => return opened,
    }
    match create_secure_directory(parent, OsStr::new(name)) {
        Ok(directory) => Ok(directory),
        Err(StorageError::Other(source)) if source.kind() == io::ErrorKind::AlreadyExists => {
            open_secure_directory(parent, name)
        }
        Err(error) => Err(error),
    }
}

/// Exclusively creates a non-followed file with mode `0600`, opened for writing.
pub(super) fn create_new_file(directory: &File, name: &str) -> Result<File, StorageError> {
    create_new_file_with(directory, name, OFlags::WRONLY)
}

/// [`create_new_file`] opened for reading and writing, for a file the caller
/// also inspects through the same descriptor.
pub(super) fn create_new_file_rw(directory: &File, name: &str) -> Result<File, StorageError> {
    create_new_file_with(directory, name, OFlags::RDWR)
}

fn create_new_file_with(
    directory: &File,
    name: &str,
    access: OFlags,
) -> Result<File, StorageError> {
    validate_name(name)?;
    let descriptor = rfs::openat(
        directory,
        name,
        OFlags::CREATE | OFlags::EXCL | access | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )
    .map_err(classify_errno)?;
    rfs::fchmod(&descriptor, Mode::from_raw_mode(0o600)).map_err(classify_errno)?;
    Ok(File::from(descriptor))
}

/// Opens `name` for appending, creating it exclusively when absent. A file this
/// call creates is returned through the descriptor that created it, so the
/// appends go to that inode and not to whatever `name` resolves to afterwards.
pub(super) fn open_or_create_append_file(
    directory: &File,
    name: &str,
) -> Result<File, StorageError> {
    open_or_create_append_file_inner(directory, name, None)
}

/// `hook` runs after a new file is created and before it is returned, where a
/// concurrent writer could replace the entry at `name`.
fn open_or_create_append_file_inner(
    directory: &File,
    name: &str,
    mut hook: Option<&mut dyn FnMut()>,
) -> Result<File, StorageError> {
    match create_new_file_with(directory, name, OFlags::RDWR | OFlags::APPEND) {
        Ok(file) => {
            sync_directory(directory)?;
            if let Some(hook) = hook.as_mut() {
                hook();
            }
            Ok(file)
        }
        Err(StorageError::Other(source)) if source.kind() == io::ErrorKind::AlreadyExists => {
            validate_name(name)?;
            let descriptor = rfs::openat(
                directory,
                name,
                OFlags::APPEND | OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(classify_errno)?;
            let file = File::from(descriptor);
            let metadata = file.metadata().map_err(classify_io)?;
            if !metadata.is_file()
                || metadata.nlink() != 1
                || metadata.uid() != rustix::process::geteuid().as_raw()
                || metadata.permissions().mode() & 0o777 != 0o600
            {
                return Err(classify_io(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "append file is not an exclusive owner-only regular file",
                )));
            }
            Ok(file)
        }
        Err(error) => Err(error),
    }
}

/// Appends one record and synchronizes file data and metadata.
///
/// A partial prior append can omit its trailing newline. In that case, this
/// function inserts a newline before `bytes`. The caller must serialize writers
/// because separate metadata reads and appends do not form one atomic operation.
pub(super) fn append_and_sync(file: &mut File, bytes: &[u8]) -> Result<(), StorageError> {
    let length = file.metadata().map_err(classify_io)?.len();
    if length > 0 {
        let mut last = [0_u8; 1];
        file.read_exact_at(&mut last, length - 1)
            .map_err(classify_io)?;
        if last[0] != b'\n' {
            file.write_all(b"\n").map_err(classify_io)?;
        }
    }
    file.write_all(bytes).map_err(classify_io)?;
    sync_file(file)
}

pub(super) fn open_regular_nofollow(directory: &File, name: &str) -> Result<File, StorageError> {
    validate_name(name)?;
    let descriptor = rfs::openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(classify_errno)?;
    let file = File::from(descriptor);
    let metadata = file.metadata().map_err(classify_io)?;
    if !metadata.file_type().is_file()
        || metadata.nlink() != 1
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o177 != 0o000
    {
        return Err(classify_io(io::Error::new(
            io::ErrorKind::InvalidData,
            "object is not an exclusively owned regular file",
        )));
    }
    Ok(file)
}

pub(super) fn write_and_sync(file: &mut File, bytes: &[u8]) -> Result<(), StorageError> {
    file.write_all(bytes).map_err(classify_io)?;
    sync_file(file)
}

fn sync_file(file: &File) -> Result<(), StorageError> {
    loop {
        match file.sync_all() {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(classify_io(error)),
        }
    }
}

pub(super) fn sync_directory(directory: &File) -> Result<(), StorageError> {
    sync_file(directory)
}

pub(super) const MAX_CONCURRENT_SYNCS: usize = 16;

/// On success, every file and directory in `files` is durable. Up to [`MAX_CONCURRENT_SYNCS`] syncs run at once on scoped threads. Every sync runs even if another fails, and the error returned is one of the failures.
pub(super) fn sync_all_concurrently(files: &[&File]) -> Result<(), StorageError> {
    if files.len() <= 1 {
        return files.iter().try_for_each(|file| sync_file(file));
    }
    let workers = files.len().min(MAX_CONCURRENT_SYNCS);
    let stride = move |worker: usize| {
        files
            .iter()
            .skip(worker)
            .step_by(workers)
            .map(|file| sync_file(file))
            .fold(Ok(()), Result::and)
    };
    std::thread::scope(|scope| {
        // A stride whose thread cannot be created syncs on this thread instead.
        let handles: Vec<_> = (0..workers)
            .map(|worker| {
                std::thread::Builder::new()
                    .spawn_scoped(scope, move || stride(worker))
                    .map_err(|_| worker)
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| match handle {
                Ok(handle) => handle
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
                Err(worker) => stride(worker),
            })
            .fold(Ok(()), Result::and)
    })
}

/// Applies a publication durability barrier to destination, then distinct source.
///
/// Directory identity uses device and inode numbers. Callers publishing across
/// two directories own this barrier after publication.
pub(super) fn sync_publish_directories_with(
    source_directory: &File,
    destination_directory: &File,
    mut sync: impl FnMut(&File) -> Result<(), StorageError>,
) -> Result<(), StorageError> {
    let source = source_directory.metadata().map_err(classify_io)?;
    let destination = destination_directory.metadata().map_err(classify_io)?;
    sync(destination_directory)?;
    if source.dev() != destination.dev() || source.ino() != destination.ino() {
        sync(source_directory)?;
    }
    Ok(())
}

/// Publishes `temp_name` as `final_name` without replacing an existing entry.
///
/// Callers must serialize publishers within this process and sync the directory
/// after success. This function does not provide the durability barrier.
pub(super) fn publish_noreplace_locked(
    directory: &File,
    temp_name: &str,
    final_name: &str,
) -> Result<PublishOutcome, StorageError> {
    publish_noreplace_between_locked(directory, temp_name, directory, final_name)
}

/// Publishes across directories without replacing `final_name`.
///
/// Callers must hold the process-local publisher lock. The native rename path
/// is atomic. The link fallback may return [`PublishOutcome::PublishedTempRetained`]
/// when publication succeeded but both cleanup and rollback failed.
pub(super) fn publish_noreplace_between_locked(
    source_directory: &File,
    temp_name: &str,
    destination_directory: &File,
    final_name: &str,
) -> Result<PublishOutcome, StorageError> {
    validate_name(temp_name)?;
    validate_name(final_name)?;

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        match rfs::renameat_with(
            source_directory,
            temp_name,
            destination_directory,
            final_name,
            rfs::RenameFlags::NOREPLACE,
        ) {
            Ok(()) => return Ok(PublishOutcome::Published),
            Err(rustix::io::Errno::EXIST) | Err(rustix::io::Errno::NOTEMPTY) => {
                return Ok(PublishOutcome::AlreadyExists);
            }
            // `NOTSUP` equals `OPNOTSUPP` on Linux, so equality guards avoid
            // duplicate or-patterns.
            Err(error)
                if error == rustix::io::Errno::INVAL
                    || error == rustix::io::Errno::NOSYS
                    || error == rustix::io::Errno::OPNOTSUPP
                    || error == rustix::io::Errno::NOTSUP => {}
            Err(error) => return Err(classify_errno(error)),
        }
    }

    // `linkat` atomically creates `final_name` or returns `EEXIST`, preventing
    // a concurrent publisher from replacing it.
    match rfs::linkat(
        source_directory,
        temp_name,
        destination_directory,
        final_name,
        AtFlags::empty(),
    ) {
        Ok(()) => match rfs::unlinkat(source_directory, temp_name, AtFlags::empty()) {
            Ok(()) | Err(rustix::io::Errno::NOENT) => Ok(PublishOutcome::Published),
            Err(unlink_error) => {
                match rfs::unlinkat(destination_directory, final_name, AtFlags::empty()) {
                    Ok(()) | Err(rustix::io::Errno::NOENT) => Err(classify_errno(unlink_error)),
                    Err(_) => Ok(PublishOutcome::PublishedTempRetained),
                }
            }
        },
        Err(rustix::io::Errno::EXIST) => Ok(PublishOutcome::AlreadyExists),
        Err(error) => Err(classify_errno(error)),
    }
}

// Retrying after a failed post-unlink sync can observe `NOENT`; sync again so
// the unlink reaches the durability boundary.
pub(super) fn durable_unlink(directory: &File, name: &str) -> Result<(), StorageError> {
    validate_name(name)?;
    match rfs::unlinkat(directory, name, AtFlags::empty()) {
        Ok(()) => sync_directory(directory),
        Err(rustix::io::Errno::NOENT) => sync_directory(directory),
        Err(error) => Err(classify_errno(error)),
    }
}

/// Removes `name` from `directory` without syncing it; the caller syncs `directory` before relying on the removal.
pub(super) fn unlink_temp(directory: &File, name: &str) -> Result<(), StorageError> {
    validate_name(name)?;
    match rfs::unlinkat(directory, name, AtFlags::empty()) {
        Ok(()) | Err(rustix::io::Errno::NOENT) => Ok(()),
        Err(error) => Err(classify_errno(error)),
    }
}

pub(super) fn temp_name(stem: &str) -> String {
    debug_assert!(validate_name(stem).is_ok());
    format!(".{stem}-{}.tmp", next_unique_id())
}

/// Returns a process-local identifier with a 32-bit wrapping counter.
///
/// Relaxed ordering is sufficient because uniqueness depends only on the atomic
/// modification order. The clock-derived prefix reduces, but does not prove,
/// uniqueness across process restarts.
pub(super) fn next_unique_id() -> u64 {
    let counter = UNIQUE_ID.fetch_add(1, Ordering::Relaxed) & 0xffff_ffff;
    (unique_prefix() << 32) | counter
}

// A PID alone repeats across restarts under namespace-local container PIDs, and a
// repeated name makes `O_EXCL` creation and `RENAME_NOREPLACE` publication fail
// with an opaque `EEXIST`.
fn unique_prefix() -> u64 {
    static PREFIX: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *PREFIX.get_or_init(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.subsec_nanos())
            .unwrap_or(0);
        u64::from(nanos) ^ u64::from(std::process::id())
    })
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File};
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

    use super::*;

    #[test]
    fn a_directory_occupied_while_being_created_is_not_adopted() {
        let root = tempfile::tempdir().unwrap();
        let root_dir = File::open(root.path()).unwrap();
        // Between creating the directory and publishing it under `shard`, a
        // same-UID process moves whatever holds that name aside and puts its
        // own owner-only directory there.
        let mut occupy = || {
            if root.path().join("shard").exists() {
                fs::rename(root.path().join("shard"), root.path().join("moved")).unwrap();
            }
            fs::create_dir(root.path().join("shard")).unwrap();
            fs::set_permissions(root.path().join("shard"), fs::Permissions::from_mode(0o700))
                .unwrap();
        };
        let error =
            create_secure_directory_inner(&root_dir, OsStr::new("shard"), Some(&mut occupy))
                .expect_err("the occupied name was adopted as the created directory");
        assert!(matches!(
            &error,
            StorageError::Other(source) if source.kind() == io::ErrorKind::AlreadyExists
        ));
        // The temporary is gone and the occupant is untouched.
        let names = fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(names, ["shard"]);
    }

    #[test]
    fn a_created_directory_is_returned_through_the_descriptor_that_created_it() {
        let root = tempfile::tempdir().unwrap();
        let root_dir = File::open(root.path()).unwrap();
        let directory = create_secure_directory(&root_dir, OsStr::new("objects")).unwrap();
        let created = directory.metadata().unwrap().ino();
        assert_eq!(
            fs::metadata(root.path().join("objects")).unwrap().ino(),
            created
        );
        assert!(
            fs::read_dir(root.path())
                .unwrap()
                .all(|entry| entry.unwrap().file_name() == "objects"),
            "a temporary was left behind"
        );
    }

    #[test]
    fn a_created_append_file_is_returned_through_the_descriptor_that_created_it() {
        let root = tempfile::tempdir().unwrap();
        let root_dir = File::open(root.path()).unwrap();
        // Between creating `log` and returning it, a same-UID process replaces
        // the entry with its own file.
        let mut replace = || {
            fs::remove_file(root.path().join("log")).unwrap();
            fs::write(root.path().join("log"), b"").unwrap();
            fs::set_permissions(root.path().join("log"), fs::Permissions::from_mode(0o600))
                .unwrap();
        };
        let mut file =
            open_or_create_append_file_inner(&root_dir, "log", Some(&mut replace)).unwrap();
        let replacement = fs::metadata(root.path().join("log")).unwrap().ino();
        assert_ne!(
            file.metadata().unwrap().ino(),
            replacement,
            "the appends were bound to the replacement, not to the file this call created"
        );
        append_and_sync(&mut file, b"record").unwrap();
        assert_eq!(fs::read(root.path().join("log")).unwrap(), b"");
        assert!(
            rfs::fcntl_getfl(&file).unwrap().contains(OFlags::APPEND),
            "the created descriptor is not in append mode"
        );
    }

    #[test]
    fn durable_publish_happy_path() {
        let root = tempfile::tempdir().unwrap();
        let root_dir = File::open(root.path()).unwrap();
        let directory = create_secure_directory(&root_dir, OsStr::new("objects")).unwrap();
        let temp = temp_name("artifact");
        let mut file = create_new_file(&directory, &temp).unwrap();
        write_and_sync(&mut file, b"payload").unwrap();

        assert_eq!(
            publish_noreplace_locked(&directory, &temp, "digest").unwrap(),
            PublishOutcome::Published
        );

        let mut bytes = Vec::new();
        File::open(root.path().join("objects/digest"))
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(bytes, b"payload");
    }

    #[test]
    fn cross_directory_publish_syncs_destination_and_source() {
        let root = tempfile::tempdir().unwrap();
        let root_dir = File::open(root.path()).unwrap();
        let source = create_secure_directory(&root_dir, OsStr::new("tmp")).unwrap();
        let destination = create_secure_directory(&root_dir, OsStr::new("shard")).unwrap();
        let mut temp = create_new_file(&source, "artifact.tmp").unwrap();
        write_and_sync(&mut temp, b"payload").unwrap();
        let mut synced = Vec::new();

        sync_publish_directories_with(&source, &destination, |directory| {
            synced.push(directory.metadata().unwrap().ino());
            Ok(())
        })
        .unwrap();

        assert_eq!(
            synced,
            [
                destination.metadata().unwrap().ino(),
                source.metadata().unwrap().ino()
            ]
        );
        assert_eq!(
            publish_noreplace_between_locked(&source, "artifact.tmp", &destination, "digest")
                .unwrap(),
            PublishOutcome::Published
        );
        assert!(!root.path().join("tmp/artifact.tmp").exists());
        assert_eq!(
            fs::read(root.path().join("shard/digest")).unwrap(),
            b"payload"
        );
    }

    #[test]
    fn same_directory_publish_syncs_once() {
        let root = tempfile::tempdir().unwrap();
        let directory = File::open(root.path()).unwrap();
        let mut sync_count = 0;

        sync_publish_directories_with(&directory, &directory, |_| {
            sync_count += 1;
            Ok(())
        })
        .unwrap();

        assert_eq!(sync_count, 1);
    }

    #[test]
    fn occupied_publish_preserves_destination_and_temp() {
        let root = tempfile::tempdir().unwrap();
        let directory = File::open(root.path()).unwrap();
        let mut destination = create_new_file(&directory, "digest").unwrap();
        write_and_sync(&mut destination, b"old").unwrap();
        let mut temp = create_new_file(&directory, "temp").unwrap();
        write_and_sync(&mut temp, b"new").unwrap();

        assert_eq!(
            publish_noreplace_locked(&directory, "temp", "digest").unwrap(),
            PublishOutcome::AlreadyExists
        );
        assert_eq!(fs::read(root.path().join("digest")).unwrap(), b"old");
        assert_eq!(fs::read(root.path().join("temp")).unwrap(), b"new");
    }

    #[test]
    fn batched_directories_are_owner_only_and_a_name_taken_first_is_opened() {
        let root = tempfile::tempdir().unwrap();
        let root_dir = File::open(root.path()).unwrap();
        fs::create_dir(root.path().join("bb")).unwrap();
        fs::set_permissions(root.path().join("bb"), fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(root.path().join("bb/held"), b"kept").unwrap();
        let names = ["aa", "bb", "cc"];
        let created = create_secure_directories(&root_dir, &names).unwrap();
        assert_eq!(created.len(), names.len());
        for (name, directory) in names.iter().zip(&created) {
            let metadata = fs::symlink_metadata(root.path().join(name)).unwrap();
            assert!(metadata.is_dir());
            assert_eq!(metadata.permissions().mode() & 0o777, 0o700, "{name}");
            assert_eq!(
                directory.metadata().unwrap().ino(),
                metadata.ino(),
                "{name}"
            );
        }
        assert_eq!(fs::read(root.path().join("bb/held")).unwrap(), b"kept");
        let leftovers: Vec<_> = fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| !names.iter().any(|kept| name == *kept))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn a_batched_name_that_is_not_an_owner_only_directory_refuses_and_leaves_no_temporary() {
        let root = tempfile::tempdir().unwrap();
        let root_dir = File::open(root.path()).unwrap();
        fs::create_dir(root.path().join("bb")).unwrap();
        fs::set_permissions(root.path().join("bb"), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(create_secure_directories(&root_dir, &["aa", "bb"]).is_err());
        let mut names: Vec<_> = fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["aa", "bb"]);
    }

    #[test]
    fn concurrent_syncs_cover_every_file_and_report_a_failure() {
        let root = tempfile::tempdir().unwrap();
        let directory = File::open(root.path()).unwrap();
        let files: Vec<File> = (0..40)
            .map(|index| {
                let mut file = create_new_file(&directory, &format!("f{index}")).unwrap();
                file.write_all(b"payload").unwrap();
                file
            })
            .collect();
        let mut all: Vec<&File> = files.iter().collect();
        all.push(&directory);
        sync_all_concurrently(&all).unwrap();
        sync_all_concurrently(&[]).unwrap();
        sync_all_concurrently(&all[..1]).unwrap();
        // A pipe cannot be synced, so its failure surfaces whichever worker meets it.
        let (reader, _writer) = std::io::pipe().unwrap();
        let pipe = File::from(std::os::fd::OwnedFd::from(reader));
        let mut with_pipe = all.clone();
        with_pipe.insert(17, &pipe);
        assert!(sync_all_concurrently(&with_pipe).is_err());
    }

    #[test]
    fn durable_unlink_is_idempotent_when_absent() {
        let root = tempfile::tempdir().unwrap();
        let directory = File::open(root.path()).unwrap();

        durable_unlink(&directory, "missing").unwrap();
        let mut file = create_new_file(&directory, "present").unwrap();
        write_and_sync(&mut file, b"payload").unwrap();
        durable_unlink(&directory, "present").unwrap();
        durable_unlink(&directory, "present").unwrap();

        assert!(!root.path().join("present").exists());
    }

    #[test]
    fn storage_errors_separate_exhaustion_from_other_io() {
        for errno in [rustix::io::Errno::NOSPC, rustix::io::Errno::DQUOT] {
            assert!(matches!(
                classify_io(std::io::Error::from_raw_os_error(errno.raw_os_error())),
                StorageError::Exhausted(_)
            ));
        }
        assert!(matches!(
            classify_io(std::io::Error::from_raw_os_error(
                rustix::io::Errno::IO.raw_os_error()
            )),
            StorageError::Other(_)
        ));
    }

    #[test]
    fn exclusive_create_refuses_symlink_destination() {
        let root = tempfile::tempdir().unwrap();
        let directory = File::open(root.path()).unwrap();
        fs::write(root.path().join("target"), b"untouched").unwrap();
        symlink("target", root.path().join("link")).unwrap();

        assert!(create_new_file(&directory, "link").is_err());
        assert_eq!(fs::read(root.path().join("target")).unwrap(), b"untouched");
    }

    #[test]
    fn secure_directory_creation_refuses_symlink_destination() {
        let root = tempfile::tempdir().unwrap();
        let root_dir = File::open(root.path()).unwrap();
        fs::create_dir(root.path().join("target")).unwrap();
        symlink("target", root.path().join("link")).unwrap();

        assert!(create_secure_directory(&root_dir, OsStr::new("link")).is_err());
    }

    #[test]
    fn created_directories_and_files_are_owner_only() {
        let root = tempfile::tempdir().unwrap();
        let root_dir = File::open(root.path()).unwrap();
        let directory = create_secure_directory(&root_dir, OsStr::new("objects")).unwrap();
        let file = create_new_file(&directory, "artifact").unwrap();

        assert_eq!(
            fs::metadata(root.path().join("objects"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(file.metadata().unwrap().mode() & 0o777, 0o600);
        assert_eq!(
            file.metadata().unwrap().uid(),
            rustix::process::geteuid().as_raw()
        );
    }
}
