//! Single-writer lease with OS advisory locking and epoch fencing.
//! At most one live writer may hold a lease per logical store.
//!
//! Liveness uses an OS advisory lock, which the kernel releases when a process's
//! descriptors close. Fencing uses persisted, monotonically increasing epochs to
//! distinguish writer incarnations. This process-death behavior does not make
//! epoch writes durable across power loss or storage-cache loss.
//!
//! ## Key namespacing
//!
//! A [`LeaseKey`] is `(module_id, backend, scope_key)`. The `module_id` and
//! `backend` are part of the key so two different modules sharing one lease
//! directory can never collide on the same `scope_key` (e.g. two modules both
//! using session id "abc" get distinct locks). This is a deliberate requirement:
//! the shared lease root is shared across all modules.

use std::{
    fs::{File, OpenOptions, TryLockError},
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
};

const EPOCH_WIDTH: usize = 20;

/// Accepts missing sidecars and hardens existing Unix regular-file sidecars.
///
/// On non-Unix targets, this function does nothing.
///
/// Callers must ensure no untrusted principal can replace names in the parent
/// directory during metadata or permission operations.
/// Callers own SQLite databases and other files; this helper must not open them.
/// # Errors
///
/// Returns filesystem errors or `InvalidInput` for a non-regular Unix path.
pub fn protect_file(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        if !metadata.is_file() {
            return Err(non_regular_file(path));
        }
        if metadata.permissions().mode() & 0o777 != 0o600 {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn protect_open_file(file: &File, path: &std::path::Path) -> std::io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(non_regular_file(path));
    }
    // A second name for this file would receive the mode change and the epoch overwrite,
    // and two lease paths joined to one file would share one lock and one epoch despite
    // their distinct keys.
    let names = link_count(file)?;
    if names > 1 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "lease file {} has {names} names; a lease file must have exactly one",
                path.display()
            ),
        ));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(non_regular_file(path));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        // SAFETY: `geteuid` reads the effective user id and has no preconditions.
        let euid = unsafe { libc::geteuid() };
        // A lease file another user owns was placed there by that user; locking it would
        // hand the exclusion decision to a file this process does not control.
        if metadata.uid() != euid {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "lease file {} is owned by uid {}, not the running user {euid}",
                    path.display(),
                    metadata.uid()
                ),
            ));
        }
        if metadata.permissions().mode() & 0o777 != 0o600 {
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
    }
    Ok(())
}

fn non_regular_file(path: &std::path::Path) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        format!(
            "{} is not a regular file; refusing permission or lease operations",
            path.display()
        ),
    )
}

/// The lease directory and every directory above it must not accept renames from another
/// principal. Exclusion rests on every acquirer locking the inode the shared path names; a
/// principal who can rename in the directory can point the path at a second inode between
/// one acquirer's lock and the next acquirer's open, and a principal who can rename in an
/// ancestor can swap the whole directory for another the user owns, so both then hold a
/// lease for one key. On Unix the directory must be owned by the running user with no group
/// or other write bit, and each ancestor must be owned by the running user or root and be
/// either not writable by group or other or sticky, since a sticky directory lets only an
/// entry's owner rename it. Windows has no equivalent mode semantics, and the check is a
/// no-op there.
fn require_private_directory(dir: &std::path::Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        // SAFETY: `geteuid` reads the effective user id and has no preconditions.
        let euid = unsafe { libc::geteuid() };
        require_private_directory_for(dir, euid)
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        Ok(())
    }
}

/// `require_private_directory` for an explicit running user, so the ownership rules can be
/// exercised without a second account.
#[cfg(unix)]
fn require_private_directory_for(dir: &std::path::Path, euid: u32) -> std::io::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    {
        // A symlink anywhere on the unresolved path is an entry its owner can replace, even
        // inside a sticky directory, so each one must belong to the running user or root.
        // Canonicalizing first would drop those entries from the ancestry examined below.
        let dir = if dir.is_absolute() {
            dir.to_path_buf()
        } else {
            std::env::current_dir()?.join(dir)
        };
        let dir = dir.as_path();
        require_symlink_components_owned(dir, euid, 0)?;
        // The component check has accepted any final symlink, so the directory it names is
        // what must be owned by the running user and closed to group and other writers.
        let metadata = std::fs::metadata(dir)?;
        if !metadata.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("lease directory {} is not a directory", dir.display()),
            ));
        }
        if metadata.uid() != euid {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "lease directory {} is owned by uid {}, not the running user {euid}",
                    dir.display(),
                    metadata.uid()
                ),
            ));
        }
        let mode = metadata.permissions().mode() & 0o777;
        if mode & 0o022 != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "lease directory {} has mode {mode:o}; group or other write access lets \
                     another principal replace lease files",
                    dir.display()
                ),
            ));
        }
        // Symlinks along the way are resolved once, so the ancestors examined are the
        // directories that hold the entries an attacker would rename.
        let resolved = std::fs::canonicalize(dir)?;
        for ancestor in resolved.ancestors().skip(1) {
            let meta = std::fs::metadata(ancestor)?;
            require_ancestor_private(ancestor, &meta, euid)?;
        }
    }
    Ok(())
}

/// A directory that holds an entry on the way to the lease directory must be owned by the
/// running user or root and be either not writable by group or other or sticky, since a
/// sticky directory lets only an entry's owner rename it.
#[cfg(unix)]
fn require_ancestor_private(
    ancestor: &std::path::Path,
    meta: &std::fs::Metadata,
    euid: u32,
) -> std::io::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    if meta.uid() != euid && meta.uid() != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "lease directory ancestor {} is owned by uid {}; another principal could \
                 rename the lease directory away",
                ancestor.display(),
                meta.uid()
            ),
        ));
    }
    let mode = meta.permissions().mode();
    let writable_by_others = mode & 0o022 != 0;
    let sticky = mode & 0o1000 != 0;
    if writable_by_others && !sticky {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "lease directory ancestor {} has mode {:o} without the sticky bit; another \
                 principal could rename the lease directory away",
                ancestor.display(),
                mode & 0o7777
            ),
        ));
    }
    Ok(())
}

/// Every symlink among the components of `path`, and of each symlink's target in turn, must
/// be owned by `euid` or root; the walk stops after 40 hops as the kernel does.
#[cfg(unix)]
fn require_symlink_components_owned(
    path: &std::path::Path,
    euid: u32,
    hops: u32,
) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    if hops > 40 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "lease directory {} resolves through more than 40 symlinks",
                path.display()
            ),
        ));
    }
    let mut prefix = std::path::PathBuf::new();
    for component in path.components() {
        prefix.push(component);
        let meta = match std::fs::symlink_metadata(&prefix) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        if meta.file_type().is_dir() {
            // The directory that holds the next entry decides who can replace that entry,
            // whether it is a symlink or the lease directory itself, so every directory on
            // the unresolved path is held to the ancestor rule; canonicalization alone would
            // examine only the directories the final target sits under.
            if prefix != path {
                require_ancestor_private(&prefix, &meta, euid)?;
            }
            continue;
        }
        if !meta.file_type().is_symlink() {
            continue;
        }
        if meta.uid() != euid && meta.uid() != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "lease directory component {} is a symlink owned by uid {}; its owner could \
                     point it at another directory",
                    prefix.display(),
                    meta.uid()
                ),
            ));
        }
        let target = std::fs::read_link(&prefix)?;
        let target = if target.is_absolute() {
            target
        } else {
            prefix.parent().map(|p| p.join(&target)).unwrap_or(target)
        };
        require_symlink_components_owned(&target, euid, hops + 1)?;
    }
    Ok(())
}

/// The identity of a file independent of its name: device and inode on Unix, volume serial
/// number and file index on Windows. Two readings are equal only while they name the same
/// file, so comparing the identity of an opened handle with the identity the path now has
/// shows whether the path was re-pointed after the open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileIdentity {
    volume: u64,
    file: u64,
}

impl FileIdentity {
    /// The identity of the file `path` names, without following a final symlink.
    ///
    /// # Errors
    ///
    /// Returns the I/O error from reading the path's metadata, or `Unsupported` on a target
    /// that is neither Unix nor Windows.
    pub fn of_path(path: &std::path::Path) -> std::io::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let meta = std::fs::symlink_metadata(path)?;
            Ok(Self {
                volume: meta.dev(),
                file: meta.ino(),
            })
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
            // The handle is opened for reading only and closed on return; a reparse point
            // is inspected as itself, matching the Unix reading.
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .open(path)?;
            Self::of_file(&file)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = path;
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "file identity is available on Unix and Windows only",
            ))
        }
    }

    /// The identity of an opened file.
    ///
    /// # Errors
    ///
    /// Returns the I/O error from querying the handle, or `Unsupported` on a target that is
    /// neither Unix nor Windows.
    pub fn of_file(file: &File) -> std::io::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let meta = file.metadata()?;
            Ok(Self {
                volume: meta.dev(),
                file: meta.ino(),
            })
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Storage::FileSystem::{
                BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
            };
            let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
            // SAFETY: `file` holds an open handle for the duration of the call, and `info`
            // points at writable storage of the exact type the call fills in on success.
            let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) };
            if ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            // SAFETY: a non-zero return means the call initialized every field.
            let info = unsafe { info.assume_init() };
            Ok(Self {
                volume: u64::from(info.dwVolumeSerialNumber),
                file: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = file;
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "file identity is available on Unix and Windows only",
            ))
        }
    }
}

/// The number of names a file has, on Unix from `st_nlink` and on Windows from
/// `nNumberOfLinks`, so a hard-linked file is recognized on both.
///
/// # Errors
///
/// Returns the I/O error from querying the handle, or `Unsupported` on a target that is
/// neither Unix nor Windows.
pub fn link_count(file: &File) -> std::io::Result<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(file.metadata()?.nlink())
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };
        let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
        // SAFETY: `file` holds an open handle for the duration of the call, and `info`
        // points at writable storage of the exact type the call fills in on success.
        let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) };
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: a non-zero return means the call initialized every field.
        Ok(u64::from(unsafe { info.assume_init() }.nNumberOfLinks))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "link counts are available on Unix and Windows only",
        ))
    }
}

/// After the lock is held, the path must still name the locked file. A rename between this
/// acquirer's open and its lock would leave it holding a lock on a file the path no longer
/// names, so the next acquirer would lock a different file for the same key. On Windows the
/// lease handle denies delete sharing, so a rename is refused once the handle is open; the
/// comparison still covers a rename that completed before this acquirer's open.
fn require_path_still_names(file: &File, path: &std::path::Path) -> std::io::Result<()> {
    if FileIdentity::of_file(file)? != FileIdentity::of_path(path)? {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "lease path {} was replaced while the lease was being acquired",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn lease_open_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        // Without delete sharing, a rename or delete of the lease file fails while any
        // holder's handle is open, so the path keeps naming the locked file for the
        // guard's lifetime; the default share mode would let another process re-point
        // the path and lock a replacement for the same key.
        options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
    }
    options
}

/// Creates the lease root and any missing ancestors owner-only, independent of the process
/// umask, so a root this crate creates passes `require_private_directory` instead of being
/// refused for the group or other write bits a permissive umask would leave on it. An
/// existing directory is left as it is and judged by that check.
fn create_lease_root(dir: &std::path::Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

fn open_lease_file(path: &std::path::Path) -> std::io::Result<File> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "lease path has no parent directory",
        )
    })?;
    const OPEN_ATTEMPTS: usize = 3;
    for _ in 0..OPEN_ATTEMPTS {
        match lease_open_options().open(path) {
            Ok(file) => {
                protect_open_file(&file, path)?;
                return Ok(file);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }

        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        persist_epoch(temp.as_file_mut(), 0)?;
        match temp.persist_noclobber(path) {
            // The handle `persist` returns was opened by `tempfile` under its own sharing
            // and flags, so the lease handle is taken by the next attempt's open through
            // `lease_open_options` instead.
            Ok(_created) => {}
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.error),
        }
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::WouldBlock,
        format!(
            "lease path {} changed during {OPEN_ATTEMPTS} open attempts",
            path.display()
        ),
    ))
}

/// Shared lease roots require namespaced keys.
///
/// `module_id` and `backend` namespace `scope_key` so shared lease roots cannot collide across modules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseKey {
    pub module_id: String,
    pub backend: String,
    pub scope_key: String,
}

impl LeaseKey {
    pub fn new(
        module_id: impl Into<String>,
        backend: impl Into<String>,
        scope_key: impl Into<String>,
    ) -> Self {
        Self {
            module_id: module_id.into(),
            backend: backend.into(),
            scope_key: scope_key.into(),
        }
    }

    /// Field order and separators are stable because they determine lock identity.
    ///
    /// # Panics
    ///
    /// Panics when a field contains U+001F, the field separator: joining such a
    /// field would let two distinct key tuples collapse into one lock identity.
    /// Keys are program-supplied identifiers, so a separator inside one is a
    /// programming error; failing loudly beats aliasing silently. Rejecting the
    /// separator (rather than escaping it) keeps every existing durable identity
    /// byte-stable; switch to a length-prefixed encoding if separator characters
    /// ever become legitimate field content.
    pub fn identity(&self) -> String {
        for (name, field) in [
            ("module_id", &self.module_id),
            ("backend", &self.backend),
            ("scope_key", &self.scope_key),
        ] {
            assert!(
                !field.contains('\u{1f}'),
                "LeaseKey {name} contains U+001F, the lock-identity separator: {field:?}"
            );
        }
        format!(
            "{}\u{1f}{}\u{1f}{}",
            self.module_id, self.backend, self.scope_key
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LeaseError {
    /// A conflicting live holder owns the lease for this key.
    #[error(
        "storage for module '{}' (backend {}, scope '{}') is held by a conflicting live lease",
        key.module_id, key.backend, key.scope_key
    )]
    Held { key: LeaseKey },
    #[error("lease io: {0}")]
    Io(#[source] std::io::Error),
}

pub struct FileLeaseStore {
    base_dir: PathBuf,
}

impl FileLeaseStore {
    /// A store over `base_dir`, resolved to an absolute path once. A relative root would
    /// name a different directory after each change of the process's working directory, so
    /// one store could hold two live leases for one key.
    ///
    /// # Errors
    ///
    /// Returns the I/O error from reading the current directory when `base_dir` is relative.
    pub fn new(base_dir: impl Into<PathBuf>) -> std::io::Result<Self> {
        let base_dir = base_dir.into();
        let base_dir = if base_dir.is_absolute() {
            base_dir
        } else {
            std::env::current_dir()?.join(base_dir)
        };
        Ok(Self { base_dir })
    }

    fn lease_path(&self, key: &LeaseKey) -> PathBuf {
        self.base_dir
            .join(format!("{}.lease", fnv1a_hex(&key.identity())))
    }

    /// Acquires an exclusive lease and increments its persisted writer epoch.
    ///
    /// # Errors
    ///
    /// Returns [`LeaseError::Held`] when another holder has the lease, or
    /// [`LeaseError::Io`] when the lease file cannot be opened, read, or updated.
    pub fn acquire(&self, key: &LeaseKey) -> Result<HeldFileLease, LeaseError> {
        self.acquire_exclusive(key, None)
    }

    /// Acquires an exclusive lease with an epoch greater than the persisted epoch and `epoch_floor`.
    ///
    /// Empty sidecar recovery uses `epoch_floor` as its sole lower bound, which must cover every epoch previously authorized for the key; malformed nonempty state fails closed.
    ///
    /// # Errors
    ///
    /// Returns [`LeaseError::Held`] when another holder has the lease, or
    /// [`LeaseError::Io`] when the lease file cannot be opened, read, or updated.
    pub fn acquire_above(
        &self,
        key: &LeaseKey,
        epoch_floor: u64,
    ) -> Result<HeldFileLease, LeaseError> {
        self.acquire_exclusive(key, Some(epoch_floor))
    }

    fn acquire_exclusive(
        &self,
        key: &LeaseKey,
        recovery_floor: Option<u64>,
    ) -> Result<HeldFileLease, LeaseError> {
        create_lease_root(&self.base_dir).map_err(LeaseError::Io)?;
        require_private_directory(&self.base_dir).map_err(LeaseError::Io)?;
        let path = self.lease_path(key);
        let mut file = open_lease_file(&path).map_err(LeaseError::Io)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(LeaseError::Held { key: key.clone() });
            }
            Err(TryLockError::Error(e)) => return Err(LeaseError::Io(e)),
        }
        require_path_still_names(&file, &path).map_err(|error| {
            let _ = file.unlock();
            LeaseError::Io(error)
        })?;

        let epoch = bump_epoch_above(&mut file, recovery_floor).map_err(|error| {
            let _ = file.unlock();
            LeaseError::Io(epoch_path_error(&path, "bump", error))
        })?;

        Ok(HeldFileLease {
            epoch,
            file,
            key: key.clone(),
        })
    }

    /// Acquires a shared lease without changing the persisted writer epoch.
    ///
    /// Shared holders coexist and block exclusive acquisition. Use this to protect a
    /// shared resource from exclusive mutation without serializing readers. Its epoch
    /// is the last persisted writer epoch at acquisition, not a write fence.
    ///
    /// # Errors
    ///
    /// Returns [`LeaseError::Held`] when an exclusive holder has the lease, or
    /// [`LeaseError::Io`] when the lease file cannot be opened or read.
    pub fn acquire_shared(&self, key: &LeaseKey) -> Result<HeldFileLease, LeaseError> {
        create_lease_root(&self.base_dir).map_err(LeaseError::Io)?;
        require_private_directory(&self.base_dir).map_err(LeaseError::Io)?;
        let path = self.lease_path(key);
        let mut file = open_lease_file(&path).map_err(LeaseError::Io)?;
        match file.try_lock_shared() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(LeaseError::Held { key: key.clone() });
            }
            Err(TryLockError::Error(e)) => return Err(LeaseError::Io(e)),
        }
        require_path_still_names(&file, &path).map_err(|error| {
            let _ = file.unlock();
            LeaseError::Io(error)
        })?;

        let epoch = read_epoch(&mut file).map_err(|error| {
            let _ = file.unlock();
            LeaseError::Io(epoch_path_error(&path, "read", error))
        })?;

        Ok(HeldFileLease {
            epoch,
            file,
            key: key.clone(),
        })
    }
}

/// A file-backed lease that keeps its OS advisory lock held until drop.
#[derive(Debug)]
#[must_use = "bind the guard for as long as the file lease must remain held"]
pub struct HeldFileLease {
    epoch: u64,
    file: File,
    key: LeaseKey,
}

impl HeldFileLease {
    /// Shared acquisition returns an observation-only epoch, not a write fence.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// These fields determine the persisted lock-path identity.
    pub fn key(&self) -> &LeaseKey {
        &self.key
    }
}

impl Drop for HeldFileLease {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn invalid_epoch(message: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.into())
}

/// Adds path and operation context to an epoch failure without erasing it.
///
/// `std::io::Error::new(kind, String)` discards the original error payload.
/// `raw_os_error()` then returns `None` and the source chain ends there.
/// Holding the original error and reporting it through
/// [`std::error::Error::source`] keeps the errno reachable, which lets a caller
/// separate `ENOSPC` from `EDQUOT` and apply fault-specific handling.
#[derive(Debug, thiserror::Error)]
#[error("failed to {operation} lease epoch at {}: {source}", path.display())]
struct EpochError {
    path: PathBuf,
    operation: &'static str,
    source: std::io::Error,
}

fn epoch_path_error(
    path: &std::path::Path,
    operation: &'static str,
    error: std::io::Error,
) -> std::io::Error {
    // `std::io::Error`'s own `source` forwards to the payload's `source`. The
    // original error is reachable only because `EpochError` reports it.
    let kind = error.kind();
    std::io::Error::new(
        kind,
        EpochError {
            path: path.to_path_buf(),
            operation,
            source: error,
        },
    )
}

/// Shared holders do not modify persisted epoch.
fn read_epoch(file: &mut (impl Read + Seek)) -> std::io::Result<u64> {
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::with_capacity(EPOCH_WIDTH + 1);
    file.take((EPOCH_WIDTH + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > EPOCH_WIDTH {
        return Err(invalid_epoch(format!(
            "lease epoch exceeds {EPOCH_WIDTH} bytes"
        )));
    }
    if bytes.is_empty() {
        return Err(invalid_epoch("lease epoch is empty"));
    }
    if !bytes.iter().all(u8::is_ascii_digit) {
        return Err(invalid_epoch(
            "lease epoch is not an unsigned decimal integer",
        ));
    }
    // Every byte is an ASCII digit, so accumulate directly. A `str` hop would
    // add an unreachable UTF-8 error arm that no input can exercise.
    bytes
        .iter()
        .try_fold(0u64, |accumulated, digit| {
            accumulated
                .checked_mul(10)?
                .checked_add(u64::from(digit - b'0'))
        })
        .ok_or_else(|| invalid_epoch("lease epoch is outside the u64 range"))
}

/// Caller holds the exclusive lock; recovery derives the epoch from persisted state or `recovery_floor`.
fn bump_epoch_above(file: &mut File, recovery_floor: Option<u64>) -> std::io::Result<u64> {
    let epoch_floor = recovery_floor.unwrap_or(0);
    let prev = match recovery_floor {
        Some(floor) if file.metadata()?.len() == 0 => floor,
        _ => read_epoch(file)?,
    };
    let next = prev
        .max(epoch_floor)
        .checked_add(1)
        .ok_or_else(|| invalid_epoch("lease epoch is exhausted"))?;
    persist_epoch(file, next)?;
    Ok(next)
}

/// Caller ensures `epoch` exceeds every valid epoch already represented by the
/// file. With ordered writes, a partial most-significant-first overwrite cannot
/// leave a lower parseable epoch; it may leave invalid content.
fn persist_epoch(file: &mut (impl Write + Seek), epoch: u64) -> std::io::Result<()> {
    let current_len = file.seek(SeekFrom::End(0))?;
    if current_len < EPOCH_WIDTH as u64 {
        file.write_all(&[b'x'; EPOCH_WIDTH][current_len as usize..])?;
    }
    file.seek(SeekFrom::Start(0))?;
    file.write_all(format!("{epoch:0EPOCH_WIDTH$}").as_bytes())?;
    file.flush()
}

/// FNV-1a 64-bit hash of a lease identity.
///
/// Lease lock-file names ([`fnv1a_hex`]) derive from it, and lease files on
/// disk outlive any one binary, so the output for a given input is a
/// compatibility contract across versions.
pub fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// `fnv1a_hex` provides the lock-file name form.
pub fn fnv1a_hex(s: &str) -> String {
    format!("{:016x}", fnv1a(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    fn key(scope: &str) -> LeaseKey {
        LeaseKey::new("test-module", "sqlite", scope)
    }

    fn tmp_store() -> (FileLeaseStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("create temporary lease directory");
        (FileLeaseStore::new(dir.path()).expect("absolute root"), dir)
    }

    fn seed_epoch(store: &FileLeaseStore, key: &LeaseKey, bytes: &[u8]) -> PathBuf {
        let path = store.lease_path(key);
        std::fs::write(&path, bytes).expect("seed lease epoch");
        path
    }

    #[test]
    fn fresh_exclusive_initializes_to_one() {
        assert_send_sync::<FileLeaseStore>();
        assert_send_sync::<HeldFileLease>();
        let (store, _dir) = tmp_store();
        let guard = store.acquire(&key("fresh")).expect("acquire fresh state");
        assert_eq!((guard.epoch(), guard.key()), (1, &key("fresh")));
        drop(guard);

        let path = store.lease_path(&key("fresh"));
        assert_eq!(
            std::fs::read_to_string(&path).expect("read initialized epoch"),
            "00000000000000000001"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path)
                    .expect("stat published lease")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn exclusive_epoch_exceeds_resource_floor() {
        let (store, _dir) = tmp_store();
        let k = key("resource-floor");
        seed_epoch(&store, &k, b"41");

        let guard = store.acquire_above(&k, 100).expect("acquire above floor");
        assert_eq!(guard.epoch(), 101);
        drop(guard);

        let guard = store.acquire(&k).expect("ordinary reacquire");
        assert_eq!(guard.epoch(), 102);

        let path = seed_epoch(&store, &key("empty-recovery"), b"");
        let recovered = store
            .acquire_above(&key("empty-recovery"), 41)
            .expect("recover empty epoch");
        assert_eq!(recovered.epoch(), 42);
        assert_eq!(std::fs::metadata(path).unwrap().len(), EPOCH_WIDTH as u64);
    }

    #[test]
    fn shared_first_initializes_canonical_zero() {
        let (store, _dir) = tmp_store();
        let k = key("shared-first");

        let guard = store.acquire_shared(&k).expect("acquire shared first");
        assert_eq!(guard.epoch(), 0);
        assert_eq!(
            std::fs::read_to_string(store.lease_path(&k)).expect("read initialized epoch"),
            "00000000000000000000"
        );
        assert!(matches!(store.acquire(&k), Err(LeaseError::Held { .. })));
        drop(guard);

        let writer = store.acquire(&k).expect("acquire writer");
        assert_eq!(writer.epoch(), 1);
        drop(writer);
    }

    #[test]
    fn concurrent_shared_first_acquisitions_coexist() {
        use std::sync::{Arc, Barrier, Condvar, Mutex, mpsc};

        const HOLDERS: usize = 8;
        // libtest has no per-test timeout, so an unbounded wait turns a worker
        // that dies before reporting into a hung suite with no diagnostics.
        const WAIT_LIMIT: std::time::Duration = std::time::Duration::from_secs(30);
        let (store, _dir) = tmp_store();
        let store = Arc::new(store);
        let start = Arc::new(Barrier::new(HOLDERS + 1));
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let (tx, rx) = mpsc::channel();
        let mut threads = Vec::new();

        for _ in 0..HOLDERS {
            let store = Arc::clone(&store);
            let start = Arc::clone(&start);
            let release = Arc::clone(&release);
            let tx = tx.clone();
            threads.push(std::thread::spawn(move || {
                start.wait();
                let result = store.acquire_shared(&key("concurrent-shared-first"));
                tx.send(
                    result
                        .as_ref()
                        .map(|guard| guard.epoch())
                        .map_err(ToString::to_string),
                )
                .expect("report acquisition");
                if result.is_ok() {
                    let (released, wake) = &*release;
                    let (_guard, timeout) = wake
                        .wait_timeout_while(
                            released.lock().expect("lock release flag"),
                            WAIT_LIMIT,
                            |released| !*released,
                        )
                        .expect("wait for release");
                    assert!(!timeout.timed_out(), "release signal timed out");
                }
            }));
        }
        drop(tx);
        start.wait();
        let results: Vec<_> = (0..HOLDERS)
            .map(|_| rx.recv_timeout(WAIT_LIMIT).expect("shared holder report"))
            .collect();
        {
            let (released, wake) = &*release;
            *released.lock().expect("lock release flag") = true;
            wake.notify_all();
        }
        for thread in threads {
            thread.join().expect("shared holder thread");
        }

        assert_eq!(results, vec![Ok(0); HOLDERS]);
        assert_eq!(
            std::fs::read_to_string(store.lease_path(&key("concurrent-shared-first")))
                .expect("read initialized epoch"),
            "00000000000000000000"
        );
    }

    #[test]
    fn variable_width_decimal_epoch_is_canonicalized() {
        let (store, _dir) = tmp_store();
        let k = key("variable-width");
        let path = seed_epoch(&store, &k, b"41");

        let guard = store.acquire(&k).expect("acquire variable-width state");
        assert_eq!(guard.epoch(), 42);
        drop(guard);
        assert_eq!(
            std::fs::read_to_string(path).expect("read canonical epoch"),
            "00000000000000000042"
        );
    }

    #[test]
    fn invalid_epoch_states_fail_closed() {
        fn assert_invalid_data(
            result: Result<HeldFileLease, LeaseError>,
            name: &str,
            mode: &str,
            path: &std::path::Path,
        ) {
            match result {
                Err(LeaseError::Io(error)) => {
                    assert_eq!(
                        error.kind(),
                        std::io::ErrorKind::InvalidData,
                        "wrong {mode} error for {name}: {error}"
                    );
                    assert!(
                        error.to_string().contains(&path.display().to_string()),
                        "{mode} error omitted lease path {}: {error}",
                        path.display()
                    );
                }
                other => panic!("{mode} accepted {name}: {other:?}"),
            }
        }

        let cases = [
            ("empty", Vec::new()),
            ("text", b"not-an-epoch".to_vec()),
            ("whitespace", b"1\n".to_vec()),
            ("invalid-utf8", b"\xff".to_vec()),
            ("too-long", b"000000000000000000001".to_vec()),
            ("u64-overflow", b"18446744073709551616".to_vec()),
            // `str::parse::<u64>` accepts a leading `+`; the format does not.
            ("plus-sign", b"+1".to_vec()),
            ("minus-sign", b"-1".to_vec()),
            ("leading-space", b" 1".to_vec()),
            ("trailing-space", b"1 ".to_vec()),
            ("hex", b"0x1f".to_vec()),
            ("digit-separator", b"1_0".to_vec()),
        ];

        for (name, state) in cases {
            let (store, _dir) = tmp_store();
            let k = key(name);
            let path = seed_epoch(&store, &k, &state);

            assert_invalid_data(store.acquire(&k), name, "exclusive", &path);
            assert_invalid_data(store.acquire_shared(&k), name, "shared", &path);
            if !state.is_empty() {
                assert_invalid_data(store.acquire_above(&k, 100), name, "floor", &path);
            }
            assert_eq!(std::fs::read(&path).expect("read epoch"), state);
        }
    }

    #[test]
    fn epoch_errors_keep_the_underlying_os_error() {
        let errno = 28;
        let path = std::path::Path::new("/leases/scope.lease");
        let source = std::io::Error::from_raw_os_error(errno);
        let kind = source.kind();
        let message = source.to_string();

        let wrapped = epoch_path_error(path, "bump", source);

        assert_eq!(wrapped.kind(), kind);
        assert_eq!(
            wrapped.to_string(),
            format!("failed to bump lease epoch at /leases/scope.lease: {message}")
        );

        // A caller that reads only the outer error sees no errno. The original
        // error has to stay reachable through `source`.
        assert_eq!(wrapped.raw_os_error(), None);

        let error = LeaseError::Io(wrapped);
        let underlying = std::error::Error::source(&error)
            .and_then(std::error::Error::source)
            .and_then(|source| source.downcast_ref::<std::io::Error>())
            .expect("original io::Error reachable from LeaseError");
        assert_eq!(underlying.raw_os_error(), Some(errno));
    }

    #[test]
    fn maximum_epoch_is_readable_but_exhausted() {
        let (store, _dir) = tmp_store();
        let k = key("maximum");
        let state = u64::MAX.to_string();
        let path = seed_epoch(&store, &k, state.as_bytes());

        let shared = store.acquire_shared(&k).expect("read maximum epoch");
        assert_eq!(shared.epoch(), u64::MAX);
        drop(shared);
        match store.acquire(&k) {
            Err(LeaseError::Io(error)) => {
                assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
                assert_eq!(
                    error.to_string(),
                    format!(
                        "failed to bump lease epoch at {}: lease epoch is exhausted",
                        path.display()
                    )
                );
            }
            other => panic!("exclusive accepted exhausted epoch: {other:?}"),
        }
        assert_eq!(std::fs::read_to_string(path).expect("read epoch"), state);
    }

    #[test]
    fn interrupted_persist_never_leaves_a_lower_parseable_epoch() {
        struct ShortWriter {
            inner: std::io::Cursor<Vec<u8>>,
            remaining: usize,
        }

        impl Write for ShortWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                if self.remaining == 0 {
                    return Err(std::io::Error::other("injected short write"));
                }
                let len = self.remaining.min(buf.len());
                let written = self.inner.write(&buf[..len])?;
                self.remaining -= written;
                Ok(written)
            }

            fn flush(&mut self) -> std::io::Result<()> {
                self.inner.flush()
            }
        }

        impl Read for ShortWriter {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                self.inner.read(buf)
            }
        }

        impl Seek for ShortWriter {
            fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
                self.inner.seek(position)
            }
        }

        // This models ordered prefix writes only, not File, device, or power-loss behavior.
        struct Case {
            seed: &'static [u8],
            previous: u64,
            next: u64,
            expected_bytes: &'static [u8],
            /// Counts parseable short-write states so the monotonicity assertion
            /// cannot pass vacuously.
            expected_parseable: usize,
        }

        let cases = [
            Case {
                seed: b"",
                previous: 41,
                next: 42,
                expected_bytes: b"00000000000000000042",
                expected_parseable: 0,
            },
            Case {
                seed: b"41",
                previous: 41,
                next: 42,
                expected_bytes: b"00000000000000000042",
                expected_parseable: 1,
            },
            Case {
                seed: b"99",
                previous: 99,
                next: 100,
                expected_bytes: b"00000000000000000100",
                expected_parseable: 1,
            },
            Case {
                seed: b"00000000000000000041",
                previous: 41,
                next: 42,
                expected_bytes: b"00000000000000000042",
                expected_parseable: EPOCH_WIDTH,
            },
            Case {
                seed: b"00000000000000000099",
                previous: 99,
                next: 100,
                expected_bytes: b"00000000000000000100",
                expected_parseable: EPOCH_WIDTH,
            },
        ];

        for case in cases {
            let total_write_len = (EPOCH_WIDTH - case.seed.len()) + EPOCH_WIDTH;
            let mut parseable = 0usize;
            for limit in 0..total_write_len {
                let mut writer = ShortWriter {
                    inner: std::io::Cursor::new(case.seed.to_vec()),
                    remaining: limit,
                };
                persist_epoch(&mut writer, case.next).expect_err("short write must fail");
                if let Ok(parsed) = read_epoch(&mut writer) {
                    parseable += 1;
                    assert!(
                        parsed >= case.previous,
                        "prefix write rolled epoch back: seed {:?}, limit {limit}, parsed {parsed}",
                        case.seed
                    );
                }
            }
            assert_eq!(
                parseable, case.expected_parseable,
                "prefix-write oracle observed {parseable} parseable states for seed {:?}, expected {}",
                case.seed, case.expected_parseable
            );

            let mut writer = ShortWriter {
                inner: std::io::Cursor::new(case.seed.to_vec()),
                remaining: total_write_len,
            };
            persist_epoch(&mut writer, case.next).expect("complete write");
            assert_eq!(
                read_epoch(&mut writer).expect("read complete epoch"),
                case.next
            );
            assert_eq!(writer.inner.into_inner(), case.expected_bytes);
        }
    }

    /// A relative root is pinned to the working directory at construction, so a later change
    /// of directory does not move the store's lease files.
    #[test]
    fn a_relative_root_is_resolved_when_the_store_is_built() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = FileLeaseStore::new("relative/leases").expect("relative root");
        let expected = std::env::current_dir()
            .expect("cwd")
            .join("relative/leases");
        assert_eq!(store.base_dir, expected);
        drop(dir);
    }

    /// A lease directory that another principal could write to is refused by both acquisition
    /// modes before any lease file is created, and accepted again once the write bits are gone.
    #[cfg(unix)]
    #[test]
    fn acquisition_refuses_a_group_or_world_writable_lease_directory() {
        use std::os::unix::fs::PermissionsExt;

        let (store, dir) = tmp_store();
        let k = key("writable-dir");
        for mode in [0o770, 0o707, 0o777] {
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(mode))
                .expect("set directory mode");
            for result in [
                store.acquire(&k).map(|_| ()),
                store.acquire_shared(&k).map(|_| ()),
            ] {
                match result {
                    Err(LeaseError::Io(e)) => {
                        assert_eq!(
                            e.kind(),
                            std::io::ErrorKind::PermissionDenied,
                            "{mode:o}: {e}"
                        );
                        assert!(e.to_string().contains("write access"), "{mode:o}: {e}");
                    }
                    other => panic!("mode {mode:o} must be refused, got {other:?}"),
                }
            }
            assert!(
                !store.lease_path(&k).exists(),
                "no lease file is created under a refused directory"
            );
        }
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
            .expect("restore directory mode");
        assert_eq!(store.acquire(&k).expect("acquire once private").epoch(), 1);
    }

    /// An ancestor another principal can rename in is refused unless it is sticky, since a
    /// sticky directory lets only an entry's owner rename it; the lease directory's own
    /// mode is not enough when the directory entry itself can be swapped.
    #[cfg(unix)]
    #[test]
    fn acquisition_refuses_a_lease_directory_under_a_writable_non_sticky_ancestor() {
        use std::os::unix::fs::PermissionsExt;

        let outer = tempfile::tempdir().expect("outer");
        let ancestor = outer.path().join("shared");
        let base = ancestor.join("leases");
        std::fs::create_dir_all(&base).expect("dirs");
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700)).expect("base");
        let store = FileLeaseStore::new(&base).expect("absolute root");
        let k = key("ancestor");
        std::fs::set_permissions(&ancestor, std::fs::Permissions::from_mode(0o777))
            .expect("open ancestor");
        match store.acquire(&k).map(|_| ()) {
            Err(LeaseError::Io(e)) => {
                assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied, "{e}");
                assert!(e.to_string().contains("without the sticky bit"), "{e}");
            }
            other => panic!("a writable non-sticky ancestor must be refused, got {other:?}"),
        }
        std::fs::set_permissions(&ancestor, std::fs::Permissions::from_mode(0o1777))
            .expect("sticky ancestor");
        assert_eq!(
            store
                .acquire(&k)
                .expect("sticky ancestor is accepted")
                .epoch(),
            1
        );
        std::fs::set_permissions(&ancestor, std::fs::Permissions::from_mode(0o755))
            .expect("private ancestor");
        assert_eq!(
            store
                .acquire(&k)
                .expect("private ancestor is accepted")
                .epoch(),
            2
        );
    }

    /// A lease root that does not yet exist is created owner-only whatever the umask, so the
    /// acquisition that creates it is not refused by its own directory check. The umask is
    /// process-wide, so the permissive mask is set in a child process running only this
    /// test; the parent asserts on that child's outcome.
    #[cfg(unix)]
    #[test]
    fn a_fresh_lease_root_is_owner_only_under_a_permissive_umask() {
        use std::os::unix::fs::PermissionsExt;

        const CHILD_MARKER: &str = "LEASE_UMASK_PROBE_CHILD";
        if std::env::var_os(CHILD_MARKER).is_none() {
            let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
                .args([
                    "--exact",
                    "tests::a_fresh_lease_root_is_owner_only_under_a_permissive_umask",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(CHILD_MARKER, "1")
                .output()
                .expect("run the probe in a child process");
            assert!(
                output.status.success(),
                "probe child failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }

        // SAFETY: `umask` only sets the file-mode creation mask of this child process.
        unsafe { libc::umask(0o000) };
        let outer = tempfile::tempdir().expect("outer");
        std::fs::set_permissions(outer.path(), std::fs::Permissions::from_mode(0o700))
            .expect("owner-only outer");
        let root = outer.path().join("nested").join("leases");
        let store = FileLeaseStore::new(&root).expect("store");
        let lease = store
            .acquire(&key("a"))
            .expect("acquisition creates and accepts its own root");
        for dir in [&root, &outer.path().join("nested")] {
            let mode = std::fs::metadata(dir).expect("dir").permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "{} has mode {mode:o}", dir.display());
        }
        drop(lease);
        let shared = store
            .acquire_shared(&key("a"))
            .expect("shared acquisition through the same root");
        drop(shared);
    }

    /// A lease directory named through the running user's own final symlink is accepted:
    /// the component check owns the link and the directory checks apply to its target.
    #[cfg(unix)]
    #[test]
    fn a_lease_directory_behind_the_running_users_final_symlink_is_accepted() {
        use std::os::unix::fs::MetadataExt;

        let outer = tempfile::tempdir().expect("outer");
        let real = outer.path().join("real");
        std::fs::create_dir(&real).expect("real");
        let link = outer.path().join("link");
        std::os::unix::fs::symlink(&real, &link).expect("final symlink");
        let me = std::fs::metadata(&real).expect("real meta").uid();

        require_private_directory_for(&link, me)
            .expect("the running user's final symlink is accepted");
        let store = FileLeaseStore::new(&link).expect("store through the link");
        let exclusive = store
            .acquire(&key("a"))
            .expect("exclusive through the link");
        drop(exclusive);
        let shared = store
            .acquire_shared(&key("a"))
            .expect("shared through the link");
        let files = std::fs::read_dir(&real).expect("real dir").count();
        assert_eq!(files, 1, "the lease file is created in the link's target");
        drop(shared);
    }

    /// A symlink on the way to the lease directory is an entry its owner can retarget. In a
    /// sticky system directory only the symlink's owner can replace it, so a symlink owned by
    /// another user is refused there while the running user's own symlink is accepted; in a
    /// directory others can write to, even the running user's symlink is replaceable, so
    /// that directory is refused unless it is sticky.
    #[cfg(unix)]
    #[test]
    fn a_lease_path_through_a_foreign_owned_symlink_is_refused() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let outer = tempfile::tempdir().expect("outer");
        let real = outer.path().join("real");
        std::fs::create_dir(&real).expect("real");
        let me = std::fs::metadata(&real).expect("real meta").uid();

        // The system temporary directory stands in for a root-owned sticky directory.
        let system_tmp = std::env::temp_dir();
        let tmp_meta = std::fs::metadata(&system_tmp).expect("temp dir");
        let sticky_root_owned = tmp_meta.uid() == 0 && tmp_meta.permissions().mode() & 0o1000 != 0;
        // A root-owned symlink is accepted for every user, so the foreign-owner case can only
        // be simulated when the running user is not root.
        if sticky_root_owned && me != 0 {
            let link = system_tmp.join(format!("lease-link-{}-{}", std::process::id(), me));
            std::os::unix::fs::symlink(&real, &link).expect("symlink in the sticky directory");
            let base = link.join("leases");
            std::fs::create_dir(&base).expect("base through the link");
            require_private_directory_for(&base, me)
                .expect("the running user's symlink is accepted");
            let error = require_private_directory_for(&base, me + 1)
                .expect_err("a symlink another user owns must be refused");
            assert_eq!(
                error.kind(),
                std::io::ErrorKind::PermissionDenied,
                "{error}"
            );
            assert!(
                error.to_string().contains("is a symlink owned by uid"),
                "{error}"
            );
            let _ = std::fs::remove_file(&link);
        }

        // A user-owned symlink inside a directory others can write to is still replaceable,
        // so the directory that holds the symlink is checked even though the resolved path
        // never passes through it.
        let open_dir = outer.path().join("open");
        std::fs::create_dir(&open_dir).expect("open dir");
        std::os::unix::fs::symlink(&real, open_dir.join("link")).expect("symlink in open dir");
        let through_open = open_dir.join("link").join("leases");
        std::fs::create_dir_all(&through_open).expect("base through the open dir");
        std::fs::set_permissions(&open_dir, std::fs::Permissions::from_mode(0o777))
            .expect("open the directory");
        let error = require_private_directory_for(&through_open, me)
            .expect_err("a writable non-sticky directory holding the symlink must be refused");
        assert!(
            error.to_string().contains("without the sticky bit"),
            "{error}"
        );
        std::fs::set_permissions(&open_dir, std::fs::Permissions::from_mode(0o1777))
            .expect("sticky directory");
        require_private_directory_for(&through_open, me)
            .expect("a sticky directory protects the owner's symlink");
        std::fs::set_permissions(&open_dir, std::fs::Permissions::from_mode(0o755))
            .expect("private directory");
        // The chain is followed: a second hop resolves through the first.
        let hop = outer.path().join("hop");
        std::os::unix::fs::symlink(open_dir.join("link"), &hop).expect("second hop");
        require_private_directory_for(&hop.join("leases"), me)
            .expect("two owned hops are accepted");
    }

    /// A path re-pointed at another inode after the open is detected once the lock is held,
    /// so the acquirer never returns a lease on a file the path no longer names.
    #[cfg(unix)]
    #[test]
    fn a_lease_path_replaced_after_open_is_detected_before_the_lease_is_returned() {
        let (store, dir) = tmp_store();
        let k = key("replaced-after-open");
        let path = store.lease_path(&k);
        let held = open_lease_file(&path).expect("open the lease file");
        require_path_still_names(&held, &path).expect("the path still names the opened inode");
        let replacement = dir.path().join("replacement");
        std::fs::write(&replacement, b"00000000000000000041").expect("write replacement");
        std::fs::rename(&replacement, &path).expect("rename over the lease path");
        let error =
            require_path_still_names(&held, &path).expect_err("the path names another inode");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("was replaced"), "{error}");
    }

    /// A lease path that is a second name for another file is refused before the mode change
    /// and before any epoch byte is written, so the other name keeps its bytes and mode.
    #[cfg(unix)]
    #[test]
    fn acquisition_refuses_a_hard_linked_lease_file_and_leaves_the_other_name_untouched() {
        use std::os::unix::fs::PermissionsExt;

        let (store, dir) = tmp_store();
        let k = key("hardlink-acquire");
        let other = dir.path().join("other");
        let other_bytes = b"00000000000000000041";
        std::fs::write(&other, other_bytes).expect("write other name");
        std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o644))
            .expect("set other mode");
        std::fs::hard_link(&other, store.lease_path(&k)).expect("hard link lease path");

        for result in [
            store.acquire(&k).map(|_| ()),
            store.acquire_shared(&k).map(|_| ()),
        ] {
            match result {
                Err(LeaseError::Io(e)) => {
                    assert_eq!(
                        e.kind(),
                        std::io::ErrorKind::InvalidInput,
                        "unexpected: {e}"
                    );
                    assert!(
                        e.to_string().contains("has 2 names"),
                        "unexpected message: {e}"
                    );
                }
                other => panic!("a hard-linked lease file must be refused, got {other:?}"),
            }
        }

        assert_eq!(std::fs::read(&other).expect("read other name"), other_bytes);
        assert_eq!(
            std::fs::metadata(&other)
                .expect("stat other name")
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
    }

    #[cfg(unix)]
    #[test]
    fn acquisition_refuses_symlink_and_leaves_target_untouched() {
        use std::os::unix::fs::PermissionsExt;

        let (store, dir) = tmp_store();
        let k = key("symlink-acquire");
        let target = dir.path().join("target");
        let target_bytes = b"00000000000000000041";
        std::fs::write(&target, target_bytes).expect("write target");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644))
            .expect("set target mode");
        let link = store.lease_path(&k);
        std::os::unix::fs::symlink(&target, &link).expect("symlink lease path");

        assert!(store.acquire(&k).is_err());
        assert!(store.acquire_shared(&k).is_err());

        assert_eq!(std::fs::read(&target).expect("read target"), target_bytes);
        assert_eq!(
            std::fs::metadata(&target)
                .expect("stat target")
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
    }

    #[cfg(unix)]
    #[test]
    fn acquisition_refuses_fifo_without_blocking() {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};

        let (store, _dir) = tmp_store();
        let k = key("fifo");
        let path = store.lease_path(&k);
        let c_path = CString::new(path.as_os_str().as_bytes()).expect("path has no NUL");
        // SAFETY: `c_path` is NUL-terminated and points to valid memory for the call.
        let result = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
        assert_eq!(
            result,
            0,
            "mkfifo failed: {}",
            std::io::Error::last_os_error()
        );

        assert!(store.acquire(&k).is_err());
        assert!(store.acquire_shared(&k).is_err());
    }

    /// Acquisition sets a pre-existing permissive lease file to owner-only.
    /// Write access to the epoch file permits fence-token forgery.
    #[cfg(unix)]
    #[test]
    fn an_acquired_lease_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let (store, _dir) = tmp_store();
        let k = key("perm");

        // Acquisition must normalize existing files, not only create new files safely.
        let path = store.lease_path(&k);
        std::fs::write(&path, b"0").expect("pre-create lease file");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .expect("set permissive mode");

        let guard = store.acquire(&k).expect("acquire");
        let mode = std::fs::metadata(&path)
            .expect("stat lease")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "the lease file stayed group/world writable at {mode:o}"
        );

        drop(guard);
    }

    /// `protect_file` refuses a symlink rather than following it.
    ///
    /// Following one would change the mode of a file the caller never named,
    /// which is a privilege-escalation primitive wearing a hardening step's
    /// clothes. The assertion is that the TARGET's mode is unchanged, not
    /// merely that an error came back — an implementation could chmod the
    /// target and still return Err.
    #[cfg(unix)]
    #[test]
    fn protect_file_refuses_a_symlink_and_leaves_its_target_untouched() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("create temporary directory");
        let target = dir.path().join("target");
        std::fs::write(&target, b"not mine").expect("write target");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644))
            .expect("set target mode");
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");

        assert_eq!(
            protect_file(&link)
                .expect_err("a symlink must be refused rather than followed")
                .kind(),
            std::io::ErrorKind::InvalidInput
        );
        let mode = std::fs::metadata(&target)
            .expect("stat target")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o644,
            "the symlink target was chmod-ed through the link"
        );
    }

    /// A path that does not exist is not an error: callers pass optional
    /// sidecars (a WAL that exists only while the journal is active), and the
    /// absence of a file is nothing to protect. Without this, a first open of a
    /// fresh database would fail on its missing WAL.
    #[test]
    fn protect_file_ignores_a_missing_path() {
        let dir = tempfile::tempdir().expect("create temporary directory");
        let missing = dir.path().join("missing");
        assert!(protect_file(&missing).is_ok());
    }

    /// Changing the identity or hash derivation orphans existing on-disk lease
    /// files and remaps postgres advisory locks.
    #[test]
    fn identity_hash_derivation_is_stable() {
        assert_eq!(key("main").identity(), "test-module\u{1f}sqlite\u{1f}main");
        assert_eq!(fnv1a_hex(&key("main").identity()), "51a7eaa424b9fd8f");
    }

    #[test]
    fn acquire_then_second_holder_is_rejected() {
        let (store, _dir) = tmp_store();
        let k = key("alpha");

        let g1 = store.acquire(&k).expect("first acquire");
        match store.acquire(&k) {
            Err(LeaseError::Held { key }) => assert_eq!(key.scope_key, "alpha"),
            other => panic!("expected Held, got {other:?}"),
        }
        let e1 = g1.epoch();
        drop(g1);
        let g2 = store.acquire(&k).expect("re-acquire after release");
        assert!(g2.epoch() > e1, "epoch is monotonic across acquisitions");
        drop(g2);
    }

    #[test]
    fn distinct_identity_axes_do_not_conflict() {
        let (store, _dir) = tmp_store();
        let pairs = [
            (key("scope-a"), key("scope-b")),
            (
                LeaseKey::new("module-a", "sqlite", "same-scope"),
                LeaseKey::new("module-b", "sqlite", "same-scope"),
            ),
            (
                LeaseKey::new("module", "sqlite", "same-scope"),
                LeaseKey::new("module", "postgres", "same-scope"),
            ),
        ];

        for (first, second) in pairs {
            let first = store.acquire(&first).expect("acquire first identity");
            let second = store.acquire(&second).expect("acquire distinct identity");
            assert_eq!((first.epoch(), second.epoch()), (1, 1));
        }
    }

    #[test]
    fn shared_holders_coexist_but_block_exclusive() {
        let (store, _dir) = tmp_store();
        let k = key("shared");

        let s1 = store.acquire_shared(&k).expect("first shared");
        let s2 = store
            .acquire_shared(&k)
            .expect("second shared holder coexists");

        // A shared holder blocks the exclusive writer — this is the property
        // the model-cache GC relies on (never delete under a live reader).
        match store.acquire(&k) {
            Err(LeaseError::Held { key }) => assert_eq!(key.scope_key, "shared"),
            other => panic!("exclusive must be Held while shared holders live, got {other:?}"),
        }

        drop(s1);
        // Still one shared holder alive: exclusive must STILL be blocked.
        match store.acquire(&k) {
            Err(LeaseError::Held { .. }) => {}
            other => {
                panic!("exclusive must stay Held until the last shared holder drops, got {other:?}")
            }
        }

        drop(s2);
        let g = store
            .acquire(&k)
            .expect("exclusive after all shared holders released");
        drop(g);
    }

    #[test]
    fn exclusive_holder_blocks_shared() {
        let (store, _dir) = tmp_store();
        let k = key("excl-blocks-shared");

        let g = store.acquire(&k).expect("exclusive");
        match store.acquire_shared(&k) {
            Err(LeaseError::Held { key }) => assert_eq!(key.scope_key, "excl-blocks-shared"),
            other => panic!("shared must be Held while exclusive holder lives, got {other:?}"),
        }
        drop(g);
        let s = store
            .acquire_shared(&k)
            .expect("shared after exclusive released");
        drop(s);
    }

    #[test]
    fn shared_acquisition_does_not_bump_the_write_epoch() {
        let (store, _dir) = tmp_store();
        let k = key("epoch-neutral");

        let g = store.acquire(&k).expect("writer");
        assert_eq!(g.epoch(), 1);
        drop(g);

        // Shared holders observe the persisted epoch but never advance it.
        let s1 = store.acquire_shared(&k).expect("shared");
        assert_eq!(s1.epoch(), 1, "shared handle reports last writer epoch");
        drop(s1);
        let s2 = store.acquire_shared(&k).expect("shared again");
        assert_eq!(s2.epoch(), 1);
        drop(s2);

        let g2 = store.acquire(&k).expect("writer again");
        assert_eq!(
            g2.epoch(),
            2,
            "writer epoch continues from 1: shared holders did not consume epochs"
        );
        drop(g2);
    }

    // Unix-only: the child uses fcntl.flock. On Windows, the same-process tests
    // exercise the real LockFileEx shared/exclusive semantics, because
    // LockFileEx locks are per-handle (two handles in one process behave like
    // two processes for contention purposes).
    #[cfg(unix)]
    #[test]
    fn shared_lease_across_processes_blocks_exclusive() {
        // Cross-PROCESS proof (not just same-process flock semantics): a child
        // process holds a shared lease while the parent tries exclusive.
        // flock/LockFileEx semantics are per-open-file-description, so the
        // same-process tests above could in principle pass with per-fd
        // semantics that differ across processes; this pins the real contract.
        let (store, dir) = tmp_store();
        let k = key("xproc");

        // Learn the exact lock file path by acquiring+releasing once (also
        // seeds the epoch file).
        let g = store.acquire(&k).expect("seed");
        drop(g);
        let lock_path = {
            let mut entries = std::fs::read_dir(dir.path()).expect("lease dir");
            let entry = entries.next().expect("one lease file").expect("dir entry");
            entry.path()
        };

        // The child holds a SHARED flock on the lease file until the parent
        // closes its stdin. `flock(1)` from util-linux is absent on macOS, so a
        // tiny python child does it — python is available on every dev/CI
        // platform we run. Blocking on stdin instead of a fixed sleep removes
        // the scheduler-timing dependency (a stalled parent cannot outlive the
        // child's hold).
        let mut child = std::process::Command::new("python3")
            .arg("-c")
            .arg(format!(
                "import fcntl,sys\nf=open({lock_path:?},'r+')\nfcntl.flock(f,fcntl.LOCK_SH)\nprint('held',flush=True)\nsys.stdin.readline()",
            ))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawn shared-holder child");

        // Wait until the child confirms it holds the shared lock.
        {
            use std::io::BufRead;
            let stdout = child.stdout.take().expect("child stdout");
            let mut line = String::new();
            std::io::BufReader::new(stdout)
                .read_line(&mut line)
                .expect("child readiness line");
            assert_eq!(line.trim(), "held");
        }

        // Parent: exclusive must be Held while the child's shared lock lives.
        match store.acquire(&k) {
            Err(LeaseError::Held { .. }) => {}
            other => {
                panic!("exclusive must be Held under cross-process shared lock, got {other:?}")
            }
        }
        // Shared, however, coexists with the child's shared lock.
        let s = store
            .acquire_shared(&k)
            .expect("shared coexists with cross-process shared holder");
        drop(s);

        // Closing stdin unblocks the child, which exits and drops its shared lock.
        drop(child.stdin.take());
        child.wait().expect("child exit");
        let g = store.acquire(&k).expect("exclusive after child released");
        drop(g);
    }

    #[test]
    fn epoch_persists_across_store_instances() {
        let (store, dir) = tmp_store();
        let k = key("persist");
        let g = store.acquire(&k).expect("acquire");
        assert_eq!(g.epoch(), 1);
        drop(g);
        // A fresh store over the same directory continues the persisted epoch.
        let store2 = FileLeaseStore::new(dir.path()).expect("absolute root");
        let g2 = store2.acquire(&k).expect("re-acquire");
        assert_eq!(g2.epoch(), 2);
        drop(g2);
    }

    /// Externally computed FNV-1a-64 vectors detect an encoding or suffix
    /// change that would orphan existing lease files.
    #[test]
    fn lease_path_vectors_are_version_stable() {
        let (store, dir) = tmp_store();
        let long = "x".repeat(300);
        let vectors = [
            (
                LeaseKey::new("test-module", "sqlite", "main"),
                "51a7eaa424b9fd8f",
            ),
            (
                LeaseKey::new("module-a", "sqlite", "core"),
                "0160e3525823870e",
            ),
            (
                LeaseKey::new("module-b", "sqlite", "cache"),
                "b9ebf913322ef03a",
            ),
            (LeaseKey::new("", "", ""), "0879e907b5281763"),
            (
                LeaseKey::new("módulo", "sqlite", "ключ"),
                "266f8eae208be1ca",
            ),
            // LeaseKey::identity rejects U+001F in every field, so no vector
            // can contain the separator.
            (
                LeaseKey::new(long.as_str(), "sqlite", "y"),
                "8ab902ac85c82726",
            ),
        ];
        for (k, digest) in vectors {
            assert_eq!(
                k.identity(),
                format!("{}\u{1f}{}\u{1f}{}", k.module_id, k.backend, k.scope_key)
            );
            assert_eq!(fnv1a_hex(&k.identity()), digest, "hash drift for {k:?}");
            assert_eq!(
                store.lease_path(&k),
                dir.path().join(format!("{digest}.lease")),
                "lease path drift for {k:?}"
            );
        }

        // The path the store computes is the file acquisition creates.
        let k = LeaseKey::new("module-a", "sqlite", "core");
        let guard = store.acquire(&k).expect("acquire vector key");
        drop(guard);
        let created: Vec<_> = std::fs::read_dir(dir.path())
            .expect("lease dir")
            .map(|entry| entry.expect("dir entry").file_name())
            .collect();
        assert_eq!(
            created,
            vec![std::ffi::OsString::from("0160e3525823870e.lease")]
        );
    }

    /// Acquisition reads at most 21 bytes of a lease file however large the file is.
    #[test]
    fn epoch_read_is_bounded_regardless_of_file_size() {
        struct CountingReader {
            inner: std::io::Cursor<Vec<u8>>,
            read: usize,
        }

        impl Read for CountingReader {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = self.inner.read(buf)?;
                self.read += n;
                Ok(n)
            }
        }

        impl Seek for CountingReader {
            fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
                self.inner.seek(position)
            }
        }

        const SIZE: usize = 1 << 20;
        let mut reader = CountingReader {
            inner: std::io::Cursor::new(vec![b'1'; SIZE]),
            read: 0,
        };
        let error = read_epoch(&mut reader).expect_err("oversized epoch must be rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            reader.read <= EPOCH_WIDTH + 1,
            "read {} bytes of a {SIZE}-byte epoch",
            reader.read
        );

        let (store, _dir) = tmp_store();
        let k = key("huge");
        let state = vec![b'1'; SIZE];
        let path = seed_epoch(&store, &k, &state);
        for (mode, result) in [
            ("exclusive", store.acquire(&k)),
            ("shared", store.acquire_shared(&k)),
            ("floor", store.acquire_above(&k, 1)),
        ] {
            match result {
                Err(LeaseError::Io(error)) => {
                    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData, "{mode}")
                }
                other => panic!("{mode} accepted an oversized epoch: {other:?}"),
            }
        }
        assert_eq!(std::fs::read(&path).expect("read epoch"), state);
    }

    /// Each thread opens its own descriptor, so the kernel sees independent
    /// lock requests; the barrier makes them race rather than serialize.
    #[test]
    fn concurrent_exclusive_acquisitions_admit_exactly_one_holder() {
        use std::sync::{Arc, Barrier, mpsc};

        const RACERS: usize = 8;
        const WAIT_LIMIT: std::time::Duration = std::time::Duration::from_secs(30);
        let (store, _dir) = tmp_store();
        let store = Arc::new(store);
        let start = Arc::new(Barrier::new(RACERS + 1));
        let (tx, rx) = mpsc::channel();
        let mut threads = Vec::new();
        for _ in 0..RACERS {
            let store = Arc::clone(&store);
            let start = Arc::clone(&start);
            let tx = tx.clone();
            threads.push(std::thread::spawn(move || {
                start.wait();
                let result = store.acquire(&key("exclusive-race"));
                let outcome = match &result {
                    Ok(guard) => Ok(guard.epoch()),
                    Err(LeaseError::Held { .. }) => Err("held"),
                    Err(LeaseError::Io(_)) => Err("io"),
                };
                tx.send(outcome).expect("report acquisition");
                // Hold the lease until every racer has reported.
                start.wait();
                drop(result);
            }));
        }
        drop(tx);
        start.wait();
        let outcomes: Vec<_> = (0..RACERS)
            .map(|_| rx.recv_timeout(WAIT_LIMIT).expect("racer report"))
            .collect();
        start.wait();
        for thread in threads {
            thread.join().expect("racer thread");
        }

        let winners: Vec<u64> = outcomes
            .iter()
            .filter_map(|o| o.as_ref().ok().copied())
            .collect();
        assert_eq!(
            winners,
            vec![1],
            "exactly one racer holds the lease at epoch 1"
        );
        assert_eq!(
            outcomes.iter().filter(|o| **o == Err("held")).count(),
            RACERS - 1,
            "every other racer is classified as Held: {outcomes:?}"
        );
        let next = store
            .acquire(&key("exclusive-race"))
            .expect("acquire after release");
        assert_eq!(next.epoch(), 2);
    }
    /// ("a\u{1f}b", "c", "d") and ("a", "b\u{1f}c", "d") would join to the
    /// same identity bytes. `identity` rejects the separator in every field
    /// position instead of producing colliding identity bytes.
    #[test]
    fn separator_in_a_key_field_fails_closed_instead_of_aliasing() {
        let cases = [
            (LeaseKey::new("a\u{1f}b", "c", "d"), "module_id"),
            (LeaseKey::new("a", "b\u{1f}c", "d"), "backend"),
            (LeaseKey::new("a", "b", "c\u{1f}d"), "scope_key"),
        ];
        for (key, field) in cases {
            let panic = match std::panic::catch_unwind(|| key.identity()) {
                Err(payload) => payload,
                Ok(identity) => panic!("separator in {field} must panic, got {identity:?}"),
            };
            let message = panic
                .downcast_ref::<String>()
                .expect("panic carries a message");
            assert!(
                message.contains(field) && message.contains("U+001F"),
                "panic must name the offending field, got: {message}"
            );
        }
    }
}
