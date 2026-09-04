//!
//! All mutations use one validated directory descriptor opened with `O_NOFOLLOW`.
//! Path-based operations remain inside the validated directory.
//! A concurrent path or symlink swap cannot redirect create, rename, or unlink outside the validated directory.

use std::fmt;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

use rustix::fd::OwnedFd;
use rustix::fs::{
    AtFlags, CWD, FlockOperation, Mode, OFlags, flock, fsync, mkdirat, openat, renameat, unlinkat,
};

use crate::connection_file::{
    ConnectionInfo, DAEMON_ID_LEN, KEY_LEN, MAX_CONNECTION_FILE_LEN, SCHEMA_VERSION,
};

/// `CONNECTION_FILE_NAME` names the canonical publication file inside the runtime directory.
pub const CONNECTION_FILE_NAME: &str = "connection.json";

/// `STALE_TEMP_AFTER` removes abandoned publication files after 600 seconds.
/// (protocol §4.2).
const STALE_TEMP_AFTER: Duration = Duration::from_secs(600);

/// `ConnectionKey` redacts diagnostic output to prevent credential disclosure.
/// (protocol V24).
pub struct ConnectionKey(pub(crate) [u8; KEY_LEN]);

impl fmt::Debug for ConnectionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ConnectionKey(<{KEY_LEN} bytes redacted>)")
    }
}

impl ConnectionKey {
    pub(crate) fn bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }
}

/// `InstanceError` never stores key bytes.
#[derive(Debug)]
pub enum InstanceError {
    UnsupportedPlatform,
    NoDataDir,
    Io {
        op: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    /// The runtime directory, lock, or publication failed a security check.
    Insecure {
        what: &'static str,
        path: PathBuf,
    },
    /// Another live host instance holds the lock.
    AlreadyRunning,
    /// The supplied payload-manifest digest must contain 64 lowercase hexadecimal characters.
    InvalidPayloadDigest,
    /// Unknown-schema lifecycle-state bytes are never interpreted, migrated, overwritten, or removed.
    UnsupportedStateSchema {
        path: PathBuf,
    },
    /// `NamespaceDrift` requires the holder to abort its named-namespace result when a retained descriptor no longer matches the identity resolved by its name.
    NamespaceDrift {
        path: PathBuf,
    },
    Random,
}

impl fmt::Display for InstanceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform => write!(
                f,
                "eidnara-host only conforms on Unix numeric-IPv4-loopback profiles"
            ),
            Self::NoDataDir => write!(
                f,
                "no data directory: set XDG_DATA_HOME, HOME, or an explicit override"
            ),
            Self::Io { op, path, source } => {
                write!(f, "instance {op} failed for {}: {source}", path.display())
            }
            Self::Insecure { what, path } => write!(
                f,
                "refusing insecure {what} at {}: wrong type, owner, mode, or link count",
                path.display()
            ),
            Self::AlreadyRunning => write!(f, "another eidnara-host instance holds the lock"),
            Self::InvalidPayloadDigest => write!(
                f,
                "payload-manifest digest must be {} lowercase hex characters",
                crate::lifecycle::PAYLOAD_MANIFEST_DIGEST_LEN
            ),
            Self::UnsupportedStateSchema { path } => write!(
                f,
                "refusing to touch an unknown lifecycle state schema at {}",
                path.display()
            ),
            Self::NamespaceDrift { path } => write!(
                f,
                "managed namespace identity drifted at {}",
                path.display()
            ),
            Self::Random => write!(f, "OS CSPRNG failure while minting credentials"),
        }
    }
}

impl std::error::Error for InstanceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub(crate) fn io_err(op: &'static str, path: &Path, source: rustix::io::Errno) -> InstanceError {
    InstanceError::Io {
        op,
        path: path.to_path_buf(),
        source: io::Error::from(source),
    }
}

/// Resolves the data root: an absolute override, else the default root the environment
/// implies. A relative override is refused because the setup socket path derives from it.
pub fn data_dir_path(data_dir_override: Option<&Path>) -> Result<PathBuf, InstanceError> {
    match data_dir_override {
        // The setup socket path uses this directory; `ConnectionInfo::validate` rejects relative `setup_socket` paths.
        Some(dir) if !dir.is_absolute() => Err(InstanceError::Insecure {
            what: "data directory override is not absolute",
            path: dir.to_path_buf(),
        }),
        Some(dir) => Ok(dir.to_path_buf()),
        None => default_data_root(DataRootEnv {
            xdg_data_home: std::env::var_os("XDG_DATA_HOME"),
            home: std::env::var_os("HOME"),
        }),
    }
}

/// The two environment values the default data root is derived from, named so a caller
/// cannot swap them.
struct DataRootEnv {
    xdg_data_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
}

/// The data root implied by the environment: an absolute `XDG_DATA_HOME` wins, then
/// `$HOME/.local/share` for an absolute `HOME`. A relative or empty value is ignored rather
/// than joined to the working directory.
fn default_data_root(env: DataRootEnv) -> Result<PathBuf, InstanceError> {
    fn absolute(value: std::ffi::OsString) -> Option<PathBuf> {
        let path = PathBuf::from(value);
        path.is_absolute().then_some(path)
    }
    match env.xdg_data_home.and_then(absolute) {
        Some(dir) => Ok(dir),
        None => match env.home.and_then(absolute) {
            Some(home) => Ok(home.join(".local").join("share")),
            None => Err(InstanceError::NoDataDir),
        },
    }
}

/// The managed-subtree constant defines the only managed segment; every managed path derives from it so a rename cannot leave part of the tree behind.
pub const MANAGED_DIR_NAME: &str = "eidnara";

/// The runtime-directory segment under the managed subtree, holding the
/// publication and lock files.
pub const RUNTIME_DIR_NAME: &str = "run";

/// Resolves `${dataDir}/eidnara`: the replaceable managed subtree that
/// holds the runtime directory, the lifecycle root, and module storage.
pub fn managed_dir_path(data_dir_override: Option<&Path>) -> Result<PathBuf, InstanceError> {
    Ok(data_dir_path(data_dir_override)?.join(MANAGED_DIR_NAME))
}

/// order.
pub fn runtime_dir_path(data_dir_override: Option<&Path>) -> Result<PathBuf, InstanceError> {
    Ok(managed_dir_path(data_dir_override)?.join(RUNTIME_DIR_NAME))
}

/// An `InstanceGuard` represents one secured host incarnation.
/// An `InstanceGuard` retains validated directory, lock, credentials, and publication identity after `publish`.
///
/// Dropping the guard best-effort removes its fenced publication.
/// Dropping the guard releases the lock; callers must retain it until handlers drop.
/// `Drop` best-effort removes this guard's publication when a `run` future is cancelled or aborted.
/// `Drop` best-effort removes this guard's publication when `run` is cancelled or aborted.
/// `Drop` best-effort removes the canonical file only when it still names this guard's publication.
pub struct InstanceGuard {
    dir: OwnedFd,
    dir_path: PathBuf,
    key: ConnectionKey,
    daemon_id: [u8; DAEMON_ID_LEN],
    launch_id: [u8; 16],
    payload_manifest_digest: String,
    publication: Option<PublicationIdentity>,
    setup_socket: Option<SetupSocketIdentity>,
    /// The stable incarnation fence, declared after `dir` so the runtime
    /// lock releases first and the lifetime fence outlives every
    /// descriptor-relative cleanup step.
    _lifetime: crate::lifecycle::LifetimeLock,
}

/// Cleanup checks that the canonical file still names this publication before unlinking it.
struct PublicationIdentity {
    dev: u64,
    ino: u64,
}

/// Cleanup unlinks the setup socket through the runtime-directory descriptor and only while the name still resolves to the registered inode.
struct SetupSocketIdentity {
    name: std::ffi::OsString,
    dev: u64,
    ino: u64,
}

impl fmt::Debug for InstanceGuard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Errors name only the directory, never key bytes.
        f.debug_struct("InstanceGuard")
            .field("dir", &self.dir_path)
            .finish_non_exhaustive()
    }
}

impl InstanceGuard {
    /// Credentials are minted only after the runtime-directory lock is acquired, so a lost lock race cannot create credentials.
    /// The lifetime fence prevents managed-subtree replacement from admitting an overlapping incarnation while this guard lives.
    ///
    /// `payload_manifest_digest` must contain exactly 64 lowercase hexadecimal characters.
    /// `payload_manifest_digest` must contain exactly 64 lowercase hexadecimal characters.
    pub fn acquire(
        data_dir_override: Option<&Path>,
        payload_manifest_digest: &str,
    ) -> Result<Self, InstanceError> {
        if !cfg!(unix) {
            return Err(InstanceError::UnsupportedPlatform);
        }
        if !crate::lifecycle::is_canonical_payload_digest(payload_manifest_digest) {
            return Err(InstanceError::InvalidPayloadDigest);
        }
        // The lifetime fence survives replacement of the `eidnara` subtree.
        // The lifetime fence is the authority that survives replacement of the `run` directory or the managed subtree.
        // The runtime-directory lock fences descriptor-relative publication and cleanup.
        let lifetime = crate::lifecycle::LifetimeLock::acquire(data_dir_override)?;
        let dir_path = runtime_dir_path(data_dir_override)?;
        let dir = secure_runtime_dir(&dir_path)?;
        lock_instance(&dir, &dir_path)?;
        // An unknown lifecycle schema at the record name blocks startup to prevent overwrite.
        if crate::lifecycle::quarantined_record_present(&dir, &dir_path)? {
            return Err(InstanceError::UnsupportedStateSchema {
                path: dir_path.join(crate::lifecycle::LIFECYCLE_RECORD_NAME),
            });
        }
        // Startup removes stale predecessor files left by incarnations that crashed before publication.
        sweep_stale_temps(&dir, &dir_path);

        let mut key = [0u8; KEY_LEN];
        getrandom::getrandom(&mut key).map_err(|_| InstanceError::Random)?;
        let mut daemon_id = [0u8; DAEMON_ID_LEN];
        getrandom::getrandom(&mut daemon_id).map_err(|_| InstanceError::Random)?;
        let mut launch_id = [0u8; 16];
        getrandom::getrandom(&mut launch_id).map_err(|_| InstanceError::Random)?;

        Ok(Self {
            dir,
            dir_path,
            key: ConnectionKey(key),
            daemon_id,
            launch_id,
            payload_manifest_digest: payload_manifest_digest.to_owned(),
            publication: None,
            setup_socket: None,
            _lifetime: lifetime,
        })
    }

    pub fn key(&self) -> &ConnectionKey {
        &self.key
    }

    pub fn daemon_id(&self) -> &[u8; DAEMON_ID_LEN] {
        &self.daemon_id
    }

    pub fn launch_id(&self) -> &[u8; 16] {
        &self.launch_id
    }

    pub(crate) fn payload_manifest_digest(&self) -> &str {
        &self.payload_manifest_digest
    }

    pub(crate) fn dir(&self) -> &OwnedFd {
        &self.dir
    }

    pub(crate) fn dir_path(&self) -> &Path {
        &self.dir_path
    }

    /// Registers the bound setup socket for fenced removal on drop.
    ///
    /// `path` must name an entry directly inside the runtime directory.
    /// A renamed or replaced `run` directory cannot redirect the descriptor-relative unlink,
    /// and a replacement occupying the name fails the identity check and is left alone.
    pub(crate) fn register_setup_socket(&mut self, path: &Path) -> Result<(), InstanceError> {
        let name = match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) if parent == self.dir_path => name.to_os_string(),
            _ => {
                return Err(InstanceError::Insecure {
                    what: "setup socket outside the runtime directory",
                    path: path.to_path_buf(),
                });
            }
        };
        let stat = rustix::fs::statat(&self.dir, name.as_os_str(), AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|e| io_err("stat_setup_socket", path, e))?;
        if !is_own_socket(&stat) {
            return Err(InstanceError::Insecure {
                what: "setup socket",
                path: path.to_path_buf(),
            });
        }
        let (dev, ino) = stat_identity(&stat);
        self.setup_socket = Some(SetupSocketIdentity { name, dev, ino });
        Ok(())
    }

    /// Checks that the registered name resolves to the registered inode before unlinking.
    /// Removal cannot exclude replacement between the identity check and the unlink.
    fn remove_setup_socket(&mut self) {
        let Some(socket) = self.setup_socket.take() else {
            return;
        };
        let Ok(stat) = rustix::fs::statat(
            &self.dir,
            socket.name.as_os_str(),
            AtFlags::SYMLINK_NOFOLLOW,
        ) else {
            return;
        };
        if !is_own_socket(&stat) || stat_identity(&stat) != (socket.dev, socket.ino) {
            return;
        }
        let _ = unlinkat(&self.dir, socket.name.as_os_str(), AtFlags::empty());
    }

    /// Atomically publishes the schema-1 connection file for this incarnation
    /// (protocol §4.1, §4.2). The owner-only `O_EXCL` temp file and the rename
    /// over the canonical name both stay relative to the pinned directory
    /// descriptor, so the swap cannot cross filesystems or follow links.
    ///
    /// The publication is refused when `ConnectionInfo::validate` or the client reader rejects it:
    /// a relative or non-UTF-8 `setup_socket`, a `daemon_ver` that is empty, lacks the published prefix, or cannot fit the authentication frame, or a serialized file over `MAX_CONNECTION_FILE_LEN`.
    /// A file rejected by a conforming client must not be installed because publication would succeed but discovery would fail.
    pub fn publish(&mut self, setup_socket: &Path, daemon_ver: &str) -> Result<(), InstanceError> {
        if !setup_socket.is_absolute() {
            return Err(InstanceError::Insecure {
                what: "relative setup socket path",
                path: setup_socket.to_path_buf(),
            });
        }
        let info = ConnectionInfo {
            schema: SCHEMA_VERSION,
            wire_version: crate::wire::PROTOCOL_VERSION,
            setup_socket: setup_socket
                .to_str()
                .ok_or_else(|| InstanceError::Insecure {
                    what: "non-UTF-8 setup socket path",
                    path: setup_socket.to_path_buf(),
                })?
                .to_owned(),
            key: self.key.0.to_vec(),
            daemon_id: self.daemon_id,
            pid: std::process::id(),
            daemon_ver: daemon_ver.to_owned(),
        };
        // The client reader applies `validate` to every publication, so a file it would refuse is never installed.
        if let Err(error) = info.validate() {
            let what = match error {
                crate::connection_file::ConnectionFileError::Invalid(what) => what,
                _ => "publication fails client validation",
            };
            return Err(InstanceError::Insecure {
                what,
                path: self.dir_path.join(CONNECTION_FILE_NAME),
            });
        }
        let json =
            serde_json::to_vec_pretty(&info).expect("connection info serialization cannot fail");
        if json.len() > MAX_CONNECTION_FILE_LEN {
            return Err(InstanceError::Insecure {
                what: "connection file exceeds the discovery cap",
                path: self.dir_path.join(CONNECTION_FILE_NAME),
            });
        }

        let stat = write_atomic_owner_only(&self.dir, &self.dir_path, CONNECTION_FILE_NAME, &json)?;
        let (dev, ino) = stat_identity(&stat);
        let identity = PublicationIdentity { dev, ino };
        self.publication = Some(identity);
        Ok(())
    }

    /// Cleanup verifies the canonical publication's retained identity before attempting removal.
    /// Cleanup requires a no-follow open of a secure regular file whose retained `(dev, ino)` matches.
    /// Cleanup also requires the daemon ID to match; a mismatch leaves the path unchanged.
    /// Cleanup cannot exclude replacement between the final identity check and unlink.
    pub fn remove_publication(&mut self) {
        let Some(identity) = self.publication.take() else {
            return;
        };
        // Transient failures must retain `identity` so `Drop` can retry.
        // Drop retries after connection descriptors close.
        // Only successful unlink or a definitive identity mismatch clears `self.publication`.
        // `O_NONBLOCK` prevents a FIFO at `CONNECTION_FILE_NAME` from blocking `openat`
        // until a writer arrives; `fstat` then rejects the FIFO as a non-regular file.
        let Ok(fd) = openat(
            &self.dir,
            CONNECTION_FILE_NAME,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        ) else {
            self.publication = Some(identity);
            return;
        };
        let Ok(stat) = rustix::fs::fstat(&fd) else {
            self.publication = Some(identity);
            return;
        };
        if !is_secure_regular(&stat) {
            // An extra link or loosened mode is not an identity mismatch.
            // Either can be reverted before `Drop` runs, so the identity is retained.
            self.publication = Some(identity);
            return;
        }
        if stat_identity(&stat) != (identity.dev, identity.ino) {
            return;
        }
        let Ok(bytes) = read_all_fd(&fd, 65_536) else {
            // A transient read failure retains `identity` for `Drop` retry.
            self.publication = Some(identity);
            return;
        };
        let Ok(info) = serde_json::from_slice::<ConnectionInfo>(&bytes) else {
            // The inode already matched, so unparseable bytes are not evidence
            // that another incarnation owns the name.
            self.publication = Some(identity);
            return;
        };
        if info.daemon_id != self.daemon_id {
            return;
        }
        // A transient unlink failure retains `identity` so `Drop` can retry removal.
        // published.
        if unlinkat(&self.dir, CONNECTION_FILE_NAME, AtFlags::empty()).is_err() {
            self.publication = Some(identity);
        }
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // Idempotent: the graceful path already removed the publication and
        // took the retained identity, making this a no-op. The same
        // best-effort identity checks run before unlink on the drop path.
        // The publication is withdrawn before the endpoint it advertises is unlinked,
        // so a client that reads the publication is not directed at a socket that no longer exists.
        self.remove_publication();
        self.remove_setup_socket();
        self.remove_lifecycle_record();
    }
}

/// `open_secure_dir_existing` traverses an existing managed directory path without following symlinks.
/// `Ok(None)` means a component is absent and the subtree does not yet exist.
/// `Err` means a component is insecure, unreadable, or does not belong to this instance.
///
/// Observational callers must distinguish absent components from insecure or unreadable components.
/// Mapping an insecure component to absence would report hostile persisted state as "nothing installed yet".
/// Resolving each component through the previous pinned descriptor prevents intermediate symlinks from redirecting traversal.
pub(crate) fn open_secure_dir_existing(dir_path: &Path) -> Result<Option<OwnedFd>, InstanceError> {
    let mut current = open_safe_anchor(dir_path)
        .map_err(|e| io_err("open_anchor", dir_path, e))?
        .ok_or_else(|| InstanceError::Insecure {
            what: "managed directory ancestor",
            path: dir_path.to_path_buf(),
        })?;
    let mut walked = if dir_path.is_absolute() {
        PathBuf::from("/")
    } else {
        PathBuf::new()
    };
    let names = normal_components(dir_path).ok_or_else(|| InstanceError::Insecure {
        what: "managed directory path",
        path: dir_path.to_path_buf(),
    })?;
    if names.is_empty() {
        return Err(InstanceError::Insecure {
            what: "managed directory path",
            path: dir_path.to_path_buf(),
        });
    }
    let last = names.len() - 1;
    for (index, name) in names.into_iter().enumerate() {
        walked.push(name);
        let next = match openat(&current, name, HARDENED_DIR_FLAGS, Mode::empty()) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            // `O_NOFOLLOW` fails a symlink component with `ELOOP`; a non-directory
            // component fails with `ENOTDIR`. Both are hostile shapes, not absence.
            Err(rustix::io::Errno::LOOP) | Err(rustix::io::Errno::NOTDIR) => {
                return Err(InstanceError::Insecure {
                    what: "managed directory component",
                    path: walked.clone(),
                });
            }
            Err(e) => return Err(io_err("open_component", &walked, e)),
        };
        // Traversal must not resolve later components through a replaceable pathname.
        // A principal that can rename or swap an intermediate component can redirect later pathname resolution.
        // The caller validates the final component.
        if index != last {
            let stat =
                rustix::fs::fstat(&next).map_err(|e| io_err("fstat_component", &walked, e))?;
            if !is_safe_ancestor(&stat) {
                return Err(InstanceError::Insecure {
                    what: "managed directory ancestor",
                    path: walked.clone(),
                });
            }
        }
        current = next;
    }
    Ok(Some(current))
}

/// `secure_runtime_dir` traverses and validates `dir_path` without following symlinks.
/// `secure_runtime_dir` normalizes newly created components to mode 0700.
/// `secure_runtime_dir` returns a pinned descriptor for the final directory after validating its ownership and mode.
pub(crate) fn secure_runtime_dir(dir_path: &Path) -> Result<OwnedFd, InstanceError> {
    let flags = HARDENED_DIR_FLAGS;
    let mut current = open_safe_anchor(dir_path)
        .map_err(|e| io_err("open_anchor", dir_path, e))?
        .ok_or_else(|| InstanceError::Insecure {
            what: "runtime directory ancestor",
            path: dir_path.to_path_buf(),
        })?;
    let mut walked = if dir_path.is_absolute() {
        PathBuf::from("/")
    } else {
        PathBuf::new()
    };
    // `secure_runtime_dir` must not chmod ancestor directories such as `/tmp` or `$HOME`.
    // `secure_runtime_dir` validates and tightens the final directory to mode 0700 through its pinned descriptor.
    // descriptor below.
    let names = normal_components(dir_path).ok_or_else(|| InstanceError::Insecure {
        what: "runtime directory path",
        path: dir_path.to_path_buf(),
    })?;
    let saw_component = !names.is_empty();

    let last = names.len().saturating_sub(1);
    for (index, name) in names.into_iter().enumerate() {
        walked.push(name);
        let next = match openat(&current, name, flags, Mode::empty()) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT) => {
                let created = match mkdirat(&current, name, Mode::from_raw_mode(0o700)) {
                    Ok(()) => true,
                    Err(rustix::io::Errno::EXIST) => false,
                    Err(e) => return Err(io_err("mkdir_component", &walked, e)),
                };
                // umask filters `mkdirat` modes; `chmodat` restores owner access before reopening.
                // The traversal restores owner access before reopening a component created with mode 0000 by a restrictive umask.
                // The subsequent no-follow open pins the reopened directory.
                if created {
                    rustix::fs::chmodat(
                        &current,
                        name,
                        Mode::from_raw_mode(0o700),
                        AtFlags::empty(),
                    )
                    .map_err(|e| io_err("chmod_component", &walked, e))?;
                }
                let fd = openat(&current, name, flags, Mode::empty())
                    .map_err(|e| io_err("open_component", &walked, e))?;
                if created {
                    rustix::fs::fchmod(&fd, Mode::from_raw_mode(0o700))
                        .map_err(|e| io_err("fchmod_component", &walked, e))?;
                }
                fd
            }
            // A crash between `mkdirat` and `chmodat` can leave an owner-owned directory without owner permissions.
            Err(rustix::io::Errno::ACCESS) => {
                if !repair_owner_access(&current, name, S_IFDIR, 0o700)
                    .map_err(|e| io_err("repair_component", &walked, e))?
                {
                    return Err(io_err("open_component", &walked, rustix::io::Errno::ACCESS));
                }
                openat(&current, name, flags, Mode::empty())
                    .map_err(|e| io_err("open_component", &walked, e))?
            }
            // A symlink or non-directory at a managed name is a hostile shape, not an I/O fault.
            Err(rustix::io::Errno::LOOP) | Err(rustix::io::Errno::NOTDIR) => {
                return Err(InstanceError::Insecure {
                    what: "runtime directory component",
                    path: walked.clone(),
                });
            }
            Err(e) => return Err(io_err("open_component", &walked, e)),
        };
        // Replacing an intermediate component can make clients and successors resolve different inodes.
        // The final component is validated and tightened after the loop because ownership validation permits repair.
        if index != last {
            let next_stat =
                rustix::fs::fstat(&next).map_err(|e| io_err("fstat_component", &walked, e))?;
            if !is_safe_ancestor(&next_stat) {
                return Err(InstanceError::Insecure {
                    what: "runtime directory ancestor",
                    path: walked.clone(),
                });
            }
        }
        current = next;
    }
    if !saw_component {
        return Err(InstanceError::Insecure {
            what: "runtime directory path",
            path: dir_path.to_path_buf(),
        });
    }

    let stat = rustix::fs::fstat(&current).map_err(|e| io_err("fstat_dir", dir_path, e))?;
    let mode = mode_bits(&stat);
    let is_dir = (mode & S_IFMT) == S_IFDIR;
    let owner_ok = stat.st_uid == rustix::process::geteuid().as_raw();
    if !is_dir || !owner_ok {
        return Err(InstanceError::Insecure {
            what: "runtime directory",
            path: dir_path.to_path_buf(),
        });
    }
    rustix::fs::fchmod(&current, Mode::from_raw_mode(0o700))
        .map_err(|e| io_err("chmod_dir", dir_path, e))?;
    Ok(current)
}

/// The function atomically installs `bytes` at `name` through the pinned `dir` descriptor.
/// Rename publishes only a fully written and synced file.
/// On failure, cleanup removes only this attempt's temp file; canonical files remain untouched.
pub(crate) fn write_atomic_owner_only(
    dir: &OwnedFd,
    dir_path: &Path,
    name: &str,
    bytes: &[u8],
) -> Result<rustix::fs::Stat, InstanceError> {
    // The stale-temp sweep reclaims temp files from crashed writes for this name.
    // `ATOMIC_WRITE_NAMES` must include every `name` used here so stale-temp sweeping reclaims crashed writes.
    debug_assert!(
        ATOMIC_WRITE_NAMES.contains(&name),
        "{name} is not registered in ATOMIC_WRITE_NAMES; its crashed temps would never be swept"
    );
    let mut suffix = [0u8; 16];
    getrandom::getrandom(&mut suffix).map_err(|_| InstanceError::Random)?;
    let temp_name = format!(".{name}.{}.{}.tmp", std::process::id(), hex(&suffix));
    // Directory-relative operations remain anchored to `dir`.
    let temp_path = dir_path.join(&temp_name);

    let fd = openat(
        dir,
        temp_name.as_str(),
        OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )
    .map_err(|e| io_err("create_temp", &temp_path, e))?;

    let result = (|| -> Result<rustix::fs::Stat, InstanceError> {
        // umask can reduce the requested `0o600` mode to `0o000`; `fchmod` restores `0o600`.
        // Under Unix mode-bit checks, `0o600` grants read access only to the file owner.
        rustix::fs::fchmod(&fd, Mode::from_raw_mode(0o600))
            .map_err(|e| io_err("chmod_temp", &temp_path, e))?;
        write_all_fd(&fd, bytes).map_err(|source| InstanceError::Io {
            op: "write_temp",
            path: temp_path.clone(),
            source,
        })?;
        fsync(&fd).map_err(|e| io_err("fsync_temp", &temp_path, e))?;
        let stat = rustix::fs::fstat(&fd).map_err(|e| io_err("fstat_temp", &temp_path, e))?;
        renameat(dir, temp_name.as_str(), dir, name)
            .map_err(|e| io_err("rename", &temp_path, e))?;
        Ok(stat)
    })();

    if result.is_err() {
        let _ = unlinkat(dir, temp_name.as_str(), AtFlags::empty());
    }
    result
}

/// The nonblocking exclusive advisory lock makes this process the publication owner for `dir`.
///
/// `run` retries on an async timer; synchronous callers use `flock_exclusive_bounded`.
/// [`flock_exclusive_bounded`].
///
/// The lock on `dir` fences only descriptor-relative publication and evidence cleanup.
/// Replacing the runtime directory can bypass this lock and allow overlap.
/// Renaming `run` or `eidnara` away lets a successor lock a new inode at the same path.
/// `crate::lifecycle::LifetimeLock` fences coordination-aware processes against directory-replacement overlap.
/// `crate::lifecycle::LifetimeLock` is acquired before this lock on `.eidnara-coordination`.
/// The lifetime fence works only between coordination-aware releases.
/// A release without the lifetime fence can overlap after directory replacement.
/// Do not run a release without the lifetime fence while another release may hold the coordination lock.
/// Renaming `.eidnara-coordination` externally splits the lifetime fence.
/// `.eidnara-coordination` must not be renamed externally.
fn lock_instance(dir: &OwnedFd, dir_path: &Path) -> Result<(), InstanceError> {
    match flock(dir, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => Ok(()),
        // WOULDBLOCK and AGAIN are one errno on Linux.
        Err(rustix::io::Errno::WOULDBLOCK) => Err(InstanceError::AlreadyRunning),
        Err(e) => Err(io_err("flock", dir_path, e)),
    }
}

/// Callers should tolerate exclusive-lock contention until the bounded lock timeout expires.
///
/// An observer holds the lifecycle lock only while reading it.
/// A probe tests instance-lock freedom with a shared lock.
/// A probe holds the coordination transaction lock shared for one sample; that shared lock blocks exclusive acquisition.
/// A mutator's exclusive lock acquisition can contend with a probe's shared lock.
/// Brief retries prevent probe contention from being reported as a live holder.
pub(crate) const LOCK_RETRY_ATTEMPTS: u32 = 4;
pub(crate) const LOCK_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(25);

/// `Ok(true)` means the lock was acquired; `Ok(false)` means every attempt returned `WOULDBLOCK`.
/// The caller defines the consequence of exhausted retries.
/// evidence-only).
///
/// `flock_bounded` sleeps the calling thread between retries; async callers must not use it.
pub(crate) fn flock_bounded(
    dir: &OwnedFd,
    dir_path: &Path,
    op: &'static str,
    operation: FlockOperation,
) -> Result<bool, InstanceError> {
    for attempt in 0..LOCK_RETRY_ATTEMPTS {
        match flock(dir, operation) {
            Ok(()) => return Ok(true),
            // WOULDBLOCK and AGAIN are one errno on Linux.
            Err(rustix::io::Errno::WOULDBLOCK) => {
                if attempt + 1 < LOCK_RETRY_ATTEMPTS {
                    std::thread::sleep(LOCK_RETRY_DELAY);
                }
            }
            Err(e) => return Err(io_err(op, dir_path, e)),
        }
    }
    Ok(false)
}

pub(crate) fn flock_exclusive_bounded(
    dir: &OwnedFd,
    dir_path: &Path,
    op: &'static str,
) -> Result<(), InstanceError> {
    if flock_bounded(dir, dir_path, op, FlockOperation::NonBlockingLockExclusive)? {
        Ok(())
    } else {
        Err(InstanceError::AlreadyRunning)
    }
}

pub(crate) const S_IFMT: u32 = 0o170000;
const S_ISVTX: u32 = 0o1000;
pub(crate) const S_IFDIR: u32 = 0o040000;
pub(crate) const S_IFREG: u32 = 0o100000;
pub(crate) const S_IFLNK: u32 = 0o120000;
const S_IFSOCK: u32 = 0o140000;

// `Stat::st_dev` is `i32` on macOS, so `stat_identity` casts it to `u64`.
// The cast is required on platforms where `st_dev` is not `u64`.
#[allow(clippy::unnecessary_cast)]
pub(crate) fn stat_identity(stat: &rustix::fs::Stat) -> (u64, u64) {
    (stat.st_dev as u64, stat.st_ino as u64)
}

pub(crate) fn owner_uid() -> u32 {
    rustix::process::geteuid().as_raw()
}

#[cfg(target_os = "macos")]
pub(crate) fn mode_bits(stat: &rustix::fs::Stat) -> u32 {
    u32::from(stat.st_mode)
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn mode_bits(stat: &rustix::fs::Stat) -> u32 {
    stat.st_mode
}

///
/// All anchor opens must use `HARDENED_DIR_FLAGS` to prevent symlink traversal.
pub(crate) const HARDENED_DIR_FLAGS: OFlags = OFlags::DIRECTORY
    .union(OFlags::NOFOLLOW)
    .union(OFlags::RDONLY)
    .union(OFlags::CLOEXEC);

///
/// walking `..` would let a pathname climb out of the tree the anchor pinned.
/// Rejecting `ParentDir` and `Prefix` prevents traversal outside the anchored tree.
pub(crate) fn normal_components(path: &Path) -> Option<Vec<&std::ffi::OsStr>> {
    let mut names = Vec::new();
    for component in path.components() {
        match component {
            Component::RootDir | Component::CurDir => continue,
            Component::Normal(name) => names.push(name),
            Component::ParentDir | Component::Prefix(_) => return None,
        }
    }
    Some(names)
}

/// The function rejects anchors that another principal can replace.
/// replaceable.
///
/// For relative paths, `open_safe_anchor` validates the process working directory before resolving path components.
/// `open_safe_anchor` returns `Ok(None)` when the opened anchor fails `is_safe_ancestor`.
pub(crate) fn open_safe_anchor(path: &Path) -> Result<Option<OwnedFd>, rustix::io::Errno> {
    let anchor = openat(
        CWD,
        if path.is_absolute() { "/" } else { "." },
        HARDENED_DIR_FLAGS,
        Mode::empty(),
    )?;
    let stat = rustix::fs::fstat(&anchor)?;
    Ok(is_safe_ancestor(&stat).then_some(anchor))
}

/// A safe directory is owned by us or root and is not group- or other-writable unless sticky.
/// A sticky directory restricts who may rename or remove its entries.
/// A sticky directory allows only the entry owner, directory owner, or root to rename an entry.
pub(crate) fn is_safe_ancestor(stat: &rustix::fs::Stat) -> bool {
    let mode = mode_bits(stat);
    if (mode & S_IFMT) != S_IFDIR {
        return false;
    }
    let ours = rustix::process::geteuid().as_raw();
    if stat.st_uid != ours && stat.st_uid != 0 {
        return false;
    }
    mode & 0o022 == 0 || mode & S_ISVTX != 0
}

pub(crate) fn is_secure_regular(stat: &rustix::fs::Stat) -> bool {
    let mode = mode_bits(stat);
    (mode & S_IFMT) == S_IFREG
        && stat.st_nlink == 1
        && stat.st_uid == rustix::process::geteuid().as_raw()
        && mode & 0o077 == 0
}

pub(crate) fn is_owner_only_dir(stat: &rustix::fs::Stat) -> bool {
    let mode = mode_bits(stat);
    (mode & S_IFMT) == S_IFDIR
        && stat.st_uid == rustix::process::geteuid().as_raw()
        && mode & 0o077 == 0
}

fn is_own_socket(stat: &rustix::fs::Stat) -> bool {
    (mode_bits(stat) & S_IFMT) == S_IFSOCK && stat.st_uid == rustix::process::geteuid().as_raw()
}

/// Repairs owner-owned entries left without owner permissions when creation stops before `chmod`.
///
/// `mkdirat` and `openat(O_CREAT)` apply the umask before the separate `chmod`.
/// A crash after creation but before `chmod` can leave an owner-owned entry without owner permissions.
/// A later open that needs owner permission fails with `EACCES` until the mode is repaired.
/// The umask only removes bits from an owner-only request, so an entry with group or other
/// bits was not left behind by that crash and is never chmodded.
///
/// `Ok(true)` means the mode was restored to `mode` and the caller may reopen.
pub(crate) fn repair_owner_access(
    dir: &OwnedFd,
    name: &std::ffi::OsStr,
    kind: u32,
    mode: u32,
) -> Result<bool, rustix::io::Errno> {
    let stat = rustix::fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW)?;
    let current = mode_bits(&stat);
    let owner_ok = stat.st_uid == rustix::process::geteuid().as_raw();
    let shape_ok = (current & S_IFMT) == kind
        && owner_ok
        && current & 0o077 == 0
        && (kind != S_IFREG || stat.st_nlink == 1);
    if !shape_ok {
        return Ok(false);
    }
    rustix::fs::chmodat(dir, name, Mode::from_raw_mode(mode), AtFlags::empty())?;
    Ok(true)
}

/// `ATOMIC_WRITE_NAMES` lists the names accepted by `write_atomic_owner_only`.
/// The writer asserts membership, so every newly atomically written file is sweepable.
pub(crate) const ATOMIC_WRITE_NAMES: [&str; 2] = [
    CONNECTION_FILE_NAME,
    crate::lifecycle::LIFECYCLE_RECORD_NAME,
];

/// The sweep ignores removal failures so startup can continue.
/// The sweep unlinks descriptor-relatively, but metadata checks and unlinking are not atomic.
/// The sweep ignores failures and examines at most 1024 successfully read entries.
fn sweep_stale_temps(dir: &OwnedFd, dir_path: &Path) {
    const MAX_SWEEP_ENTRIES: usize = 1024;
    let prefixes = ATOMIC_WRITE_NAMES.map(|name| format!(".{name}."));
    let Ok(entries) = std::fs::read_dir(dir_path) else {
        return;
    };
    for entry in entries.flatten().take(MAX_SWEEP_ENTRIES) {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let matches_temp = name.ends_with(".tmp")
            && prefixes
                .iter()
                .any(|prefix| name.starts_with(prefix.as_str()));
        if !matches_temp {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .map(|modified| {
                SystemTime::now()
                    .duration_since(modified)
                    .is_ok_and(|age| age >= STALE_TEMP_AFTER)
            })
            .unwrap_or(false);
        if stale {
            let _ = unlinkat(dir, name, AtFlags::empty());
        }
    }
}

pub(crate) fn write_all_fd(fd: &OwnedFd, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        let written = rustix::io::write(fd, bytes).map_err(io::Error::from)?;
        if written == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "write returned 0"));
        }
        bytes = &bytes[written..];
    }
    Ok(())
}

pub(crate) fn read_all_fd(fd: &OwnedFd, cap: usize) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let read = rustix::io::read(fd, &mut buf).map_err(io::Error::from)?;
        if read == 0 {
            return Ok(out);
        }
        out.extend_from_slice(&buf[..read]);
        if out.len() > cap {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "file too large"));
        }
    }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

    const TEST_DIGEST: &str = "3d7f9a1c5b2e8f0a6d4c7b9e1f3a5c8d2b4e6f0a1c3d5e7f9b0d2f4a6c8e0b1d";

    fn temp_root() -> tempfile::TempDir {
        tempfile::tempdir().expect("temp data root")
    }

    fn mode_of(path: &Path) -> u32 {
        std::fs::symlink_metadata(path)
            .expect("stat")
            .permissions()
            .mode()
            & 0o7777
    }

    fn published(guard: &InstanceGuard) -> PathBuf {
        guard.dir_path().join(CONNECTION_FILE_NAME)
    }

    #[test]
    fn explicit_override_resolves_canonical_layout() {
        let root = temp_root();
        let dir = runtime_dir_path(Some(root.path())).expect("resolve");
        assert_eq!(dir, root.path().join("eidnara").join("run"));
    }

    #[test]
    fn default_root_follows_xdg_then_home() {
        let os = |value: &str| Some(std::ffi::OsString::from(value));
        let resolve = |xdg_data_home: Option<std::ffi::OsString>,
                       home: Option<std::ffi::OsString>| {
            default_data_root(DataRootEnv {
                xdg_data_home,
                home,
            })
            .map(|root| root.join(MANAGED_DIR_NAME).join(RUNTIME_DIR_NAME))
        };

        assert_eq!(
            resolve(os("/xdg-root"), os("/home-root")).expect("xdg"),
            PathBuf::from("/xdg-root/eidnara/run")
        );

        // A relative or empty XDG_DATA_HOME must never be joined to cwd.
        for ignored in ["relative/xdg", "./xdg", ""] {
            assert_eq!(
                resolve(os(ignored), os("/home-root")).expect("relative xdg falls back to home"),
                PathBuf::from("/home-root/.local/share/eidnara/run"),
                "XDG_DATA_HOME={ignored:?}"
            );
        }

        assert_eq!(
            resolve(None, os("/home-root")).expect("home"),
            PathBuf::from("/home-root/.local/share/eidnara/run")
        );

        // A relative `HOME` is ignored; without an absolute root, the result is exactly `NoDataDir`.
        assert!(matches!(
            resolve(None, os("relative-home")),
            Err(InstanceError::NoDataDir)
        ));

        assert!(matches!(resolve(None, None), Err(InstanceError::NoDataDir)));
    }

    #[test]
    fn permissive_umask_still_yields_owner_only_dir_and_file() {
        let root = temp_root();
        let mut guard = InstanceGuard::acquire(Some(root.path()), TEST_DIGEST).expect("acquire");
        guard
            .publish(&guard.dir_path().join("setup.sock"), "eidnara-host/test")
            .expect("publish");

        assert_eq!(mode_of(guard.dir_path()), 0o700);
        let file = published(&guard);
        assert_eq!(mode_of(&file), 0o600);
        let meta = std::fs::symlink_metadata(&file).expect("stat file");
        assert!(meta.file_type().is_file());
        assert_eq!(meta.uid(), rustix::process::geteuid().as_raw());
    }

    #[test]
    fn publication_rejects_non_utf8_setup_socket_path() {
        let root = temp_root();
        let mut guard = InstanceGuard::acquire(Some(root.path()), TEST_DIGEST).expect("acquire");
        let path = PathBuf::from(OsString::from_vec(b"/tmp/eidnara-host-\xff.sock".to_vec()));
        assert!(matches!(
            guard.publish(&path, "eidnara-host/test"),
            Err(InstanceError::Insecure {
                what: "non-UTF-8 setup socket path",
                ..
            })
        ));
        assert!(!published(&guard).exists());
    }

    /// `ConnectionInfo::validate` rejects relative socket paths and empty daemon versions, and `read_for_client` rejects files over `MAX_CONNECTION_FILE_LEN`, so `publish` must refuse all three before installing a file no client accepts.
    #[test]
    fn publication_rejects_what_clients_would_refuse() {
        let root = temp_root();
        let mut guard = InstanceGuard::acquire(Some(root.path()), TEST_DIGEST).expect("acquire");

        assert!(matches!(
            guard.publish(Path::new("relative/setup.sock"), "eidnara-host/test"),
            Err(InstanceError::Insecure {
                what: "relative setup socket path",
                ..
            })
        ));
        assert!(matches!(
            guard.publish(&guard.dir_path().join("setup.sock"), ""),
            Err(InstanceError::Insecure {
                what: "empty daemon version",
                ..
            })
        ));
        assert!(matches!(
            guard.publish(&guard.dir_path().join("setup.sock"), "test"),
            Err(InstanceError::Insecure {
                what: "daemon version lacks the published prefix",
                ..
            })
        ));
        let oversized = format!(
            "{}{}",
            crate::config::DAEMON_VER_PREFIX,
            "v".repeat(MAX_CONNECTION_FILE_LEN)
        );
        assert!(matches!(
            guard.publish(&guard.dir_path().join("setup.sock"), &oversized),
            Err(InstanceError::Insecure {
                what: "daemon version cannot fit the authentication frame",
                ..
            })
        ));
        assert!(!published(&guard).exists());
        assert!(
            std::fs::read_dir(guard.dir_path())
                .expect("list run dir")
                .flatten()
                .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp")),
            "a refused publication must leave no temp file"
        );
    }

    #[test]
    fn world_writable_intermediate_is_rejected() {
        let root = temp_root();
        // Acquisition refuses an intermediate directory another principal can rename because we cannot repair it like the final directory.
        let loose = root.path().join("loose");
        std::fs::create_dir_all(&loose).expect("create intermediate");
        std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o777))
            .expect("loosen intermediate");

        let err = InstanceGuard::acquire(Some(&loose), TEST_DIGEST).expect_err("must refuse");
        assert!(
            matches!(
                err,
                InstanceError::Insecure {
                    what: "runtime directory ancestor",
                    ..
                }
            ),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn pre_existing_permissive_dir_is_tightened() {
        let root = temp_root();
        let dir = runtime_dir_path(Some(root.path())).expect("resolve");
        std::fs::create_dir_all(&dir).expect("create dir");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).expect("loosen dir");

        let guard = InstanceGuard::acquire(Some(root.path()), TEST_DIGEST).expect("acquire");
        assert_eq!(mode_of(guard.dir_path()), 0o700);
    }

    /// A crash between `mkdirat` and `chmodat` under a restrictive umask leaves an owner-owned component at mode `0000`; the next start must repair it rather than fail on `EACCES` forever.
    #[test]
    fn partially_created_components_are_repaired_on_restart() {
        for stranded in ["eidnara", "run"] {
            let root = temp_root();
            let dir = runtime_dir_path(Some(root.path())).expect("resolve");
            std::fs::create_dir_all(&dir).expect("create dirs");
            let victim = if stranded == "run" {
                dir.clone()
            } else {
                dir.parent().expect("managed dir").to_path_buf()
            };
            std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o000))
                .expect("strand component");

            let guard = InstanceGuard::acquire(Some(root.path()), TEST_DIGEST)
                .unwrap_or_else(|e| panic!("a stranded {stranded} must be repaired: {e}"));
            assert_eq!(mode_of(&victim), 0o700, "stranded {stranded}");
            assert_eq!(mode_of(guard.dir_path()), 0o700);
        }
    }

    /// Repair applies only to the umask-stranded shape. A `mkdirat(0700)` filtered by any umask never carries group or other bits, so an unreadable directory that does was not stranded by this code and stays untouched.
    #[test]
    fn unreadable_dirs_with_group_or_other_bits_are_not_repaired() {
        let root = temp_root();
        let dir = runtime_dir_path(Some(root.path())).expect("resolve");
        std::fs::create_dir_all(&dir).expect("create dirs");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o070)).expect("strand");

        let err = InstanceGuard::acquire(Some(root.path()), TEST_DIGEST).expect_err("must refuse");
        assert!(
            matches!(
                &err,
                InstanceError::Io { op: "open_component", source, .. }
                    if source.kind() == io::ErrorKind::PermissionDenied
            ),
            "unexpected error: {err:?}"
        );
        assert_eq!(
            mode_of(&dir),
            0o070,
            "a foreign-mode directory must not be chmodded"
        );
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).expect("restore");
    }

    #[test]
    fn publication_matches_schema_2_shape() {
        let root = temp_root();
        let mut guard = InstanceGuard::acquire(Some(root.path()), TEST_DIGEST).expect("acquire");
        guard
            .publish(&guard.dir_path().join("setup.sock"), "eidnara-host/test")
            .expect("publish");

        let bytes = std::fs::read(published(&guard)).expect("read publication");
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("parse");
        assert_eq!(json["schema"], 2);
        assert_eq!(json["wire_version"], 2);
        assert_eq!(
            json["setup_socket"],
            guard
                .dir_path()
                .join("setup.sock")
                .to_string_lossy()
                .as_ref()
        );
        assert_eq!(json["key"].as_array().expect("key").len(), 32);
        assert_eq!(json["daemon_id"].as_array().expect("daemon_id").len(), 16);
        assert_eq!(json["pid"], std::process::id());
        assert_eq!(json["daemon_ver"], "eidnara-host/test");
    }

    #[test]
    fn second_instance_fails_before_touching_the_first_publication() {
        let root = temp_root();
        let mut first =
            InstanceGuard::acquire(Some(root.path()), TEST_DIGEST).expect("first acquire");
        first
            .publish(&first.dir_path().join("setup.sock"), "eidnara-host/first")
            .expect("publish");
        let before = std::fs::read(published(&first)).expect("read first");

        let err =
            InstanceGuard::acquire(Some(root.path()), TEST_DIGEST).expect_err("second must fail");
        assert!(matches!(err, InstanceError::AlreadyRunning));

        let after = std::fs::read(published(&first)).expect("read first again");
        assert_eq!(before, after, "loser must not disturb the holder's file");
    }

    #[test]
    fn lock_ownership_survives_renaming_the_runtime_dir() {
        let root = temp_root();
        let mut first =
            InstanceGuard::acquire(Some(root.path()), TEST_DIGEST).expect("first acquire");
        first
            .publish(&first.dir_path().join("setup.sock"), "eidnara-host/first")
            .expect("publish");
        let before = std::fs::read(published(&first)).expect("read first");

        let moved = root.path().join("eidnara").join("run-moved");
        std::fs::rename(first.dir_path(), &moved).expect("rename runtime dir");

        // Lock ownership belongs to the inode rather than its pathname; renaming the directory does not release the holder's lock.
        let reopened = openat(
            CWD,
            &moved,
            OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::RDONLY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .expect("reopen moved dir");
        assert!(
            matches!(
                flock(&reopened, FlockOperation::NonBlockingLockExclusive),
                Err(rustix::io::Errno::WOULDBLOCK)
            ),
            "the holder must still own the renamed directory's lock"
        );

        // A successor anchors a fresh runtime inode, but the lifetime fence outside the replaceable subtree rejects a second incarnation.
        // The lifetime fence is held for the incarnation outside the replaceable subtree.
        let successor = InstanceGuard::acquire(Some(root.path()), TEST_DIGEST);
        assert!(
            matches!(successor, Err(InstanceError::AlreadyRunning)),
            "a renamed runtime directory must not admit an overlapping incarnation"
        );
        assert_eq!(
            std::fs::read(moved.join(CONNECTION_FILE_NAME)).expect("read moved"),
            before,
            "the holder's publication must be untouched"
        );

        // The open directory handle lets `first` remove its publication after the runtime directory is renamed.
        // the rename.
        first.remove_publication();
        assert!(!moved.join(CONNECTION_FILE_NAME).exists());

        // Tearing down the displaced holder releases the runtime and lifetime fences, allowing a successor.
        drop(first);
        let second =
            InstanceGuard::acquire(Some(root.path()), TEST_DIGEST).expect("successor acquires");
        drop(second);
    }

    #[test]
    fn symlinked_runtime_dir_fails_closed() {
        let root = temp_root();
        let elsewhere = temp_root();
        let dir = runtime_dir_path(Some(root.path())).expect("resolve");
        std::fs::create_dir_all(dir.parent().expect("parent")).expect("create parents");
        symlink(elsewhere.path(), &dir).expect("symlink runtime dir");

        assert!(InstanceGuard::acquire(Some(root.path()), TEST_DIGEST).is_err());
    }

    #[test]
    fn symlinked_runtime_ancestor_fails_closed() {
        let root = temp_root();
        let elsewhere = temp_root();
        symlink(elsewhere.path(), root.path().join("eidnara")).expect("symlink runtime ancestor");

        assert!(InstanceGuard::acquire(Some(root.path()), TEST_DIGEST).is_err());
        assert!(
            !elsewhere.path().join("run").exists(),
            "the symlink target must not receive host files"
        );
    }

    #[test]
    fn nonregular_publication_target_is_replaced_not_followed() {
        let root = temp_root();
        let dir = runtime_dir_path(Some(root.path())).expect("resolve");
        std::fs::create_dir_all(&dir).expect("create dir");
        // A directory at the publication name prevents rename(2) from replacing it.
        // Publication must fail closed rather than clobber a directory at the publication name.
        std::fs::create_dir(dir.join(CONNECTION_FILE_NAME)).expect("plant directory");

        let mut guard = InstanceGuard::acquire(Some(root.path()), TEST_DIGEST).expect("acquire");
        assert!(
            guard
                .publish(&guard.dir_path().join("setup.sock"), "eidnara-host/test")
                .is_err()
        );
        assert!(
            std::fs::symlink_metadata(dir.join(CONNECTION_FILE_NAME))
                .expect("stat")
                .is_dir(),
            "the planted entry must be left alone"
        );
    }

    #[test]
    fn publication_replaces_a_planted_symlink_without_following_it() {
        let root = temp_root();
        let outside = temp_root();
        let victim = outside.path().join("victim");
        std::fs::write(&victim, b"untouched").expect("write victim");

        let dir = runtime_dir_path(Some(root.path())).expect("resolve");
        std::fs::create_dir_all(&dir).expect("create dir");
        symlink(&victim, dir.join(CONNECTION_FILE_NAME)).expect("plant symlink");

        let mut guard = InstanceGuard::acquire(Some(root.path()), TEST_DIGEST).expect("acquire");
        guard
            .publish(&guard.dir_path().join("setup.sock"), "eidnara-host/test")
            .expect("publish");

        // rename(2) replaces the link itself, so the outside target is intact.
        assert_eq!(
            std::fs::read(&victim).expect("read victim"),
            b"untouched".to_vec()
        );
        let meta = std::fs::symlink_metadata(published(&guard)).expect("stat");
        assert!(
            meta.file_type().is_file(),
            "publication must be a real file"
        );
    }

    #[test]
    fn cleanup_removes_only_our_own_publication() {
        let root = temp_root();
        let mut guard = InstanceGuard::acquire(Some(root.path()), TEST_DIGEST).expect("acquire");
        guard
            .publish(&guard.dir_path().join("setup.sock"), "eidnara-host/test")
            .expect("publish");
        let file = published(&guard);
        assert!(file.exists());
        guard.remove_publication();
        assert!(!file.exists(), "our own publication must be removed");
    }

    /// The setup socket is unlinked through the runtime-directory descriptor and only while the name still resolves to the registered inode, matching the publication fence.
    #[test]
    fn setup_socket_cleanup_is_fenced_to_the_registered_inode() {
        use std::os::unix::fs::FileTypeExt;
        use std::os::unix::net::UnixListener;

        // Registration refuses a path outside the runtime directory and a non-socket.
        {
            let root = temp_root();
            let mut guard =
                InstanceGuard::acquire(Some(root.path()), TEST_DIGEST).expect("acquire");
            assert!(matches!(
                guard.register_setup_socket(&root.path().join("setup.sock")),
                Err(InstanceError::Insecure {
                    what: "setup socket outside the runtime directory",
                    ..
                })
            ));
            let plain = guard.dir_path().join("setup.sock");
            std::fs::write(&plain, b"not a socket").expect("plant file");
            assert!(matches!(
                guard.register_setup_socket(&plain),
                Err(InstanceError::Insecure {
                    what: "setup socket",
                    ..
                })
            ));
            drop(guard);
            assert!(plain.exists(), "an unregistered entry must survive drop");
        }

        {
            let root = temp_root();
            let mut guard =
                InstanceGuard::acquire(Some(root.path()), TEST_DIGEST).expect("acquire");
            let socket = guard.dir_path().join("setup.sock");
            let _listener = UnixListener::bind(&socket).expect("bind");
            guard.register_setup_socket(&socket).expect("register");
            drop(guard);
            assert!(!socket.exists(), "our own socket must be removed on drop");
        }

        // A replacement occupying the name is not ours and survives drop.
        {
            let root = temp_root();
            let mut guard =
                InstanceGuard::acquire(Some(root.path()), TEST_DIGEST).expect("acquire");
            let socket = guard.dir_path().join("setup.sock");
            let _ours = UnixListener::bind(&socket).expect("bind");
            guard.register_setup_socket(&socket).expect("register");
            std::fs::remove_file(&socket).expect("displace");
            let _replacement = UnixListener::bind(&socket).expect("rebind");
            drop(guard);
            assert!(
                std::fs::symlink_metadata(&socket)
                    .expect("stat replacement")
                    .file_type()
                    .is_socket(),
                "a replacement socket at the name must be left alone"
            );
        }

        // Renaming the runtime directory does not strand the socket: the unlink follows the descriptor, not the path.
        {
            let root = temp_root();
            let mut guard =
                InstanceGuard::acquire(Some(root.path()), TEST_DIGEST).expect("acquire");
            let socket = guard.dir_path().join("setup.sock");
            let _listener = UnixListener::bind(&socket).expect("bind");
            guard.register_setup_socket(&socket).expect("register");
            let moved = root.path().join("eidnara").join("run-moved");
            std::fs::rename(guard.dir_path(), &moved).expect("rename runtime dir");
            drop(guard);
            assert!(
                !moved.join("setup.sock").exists(),
                "the socket in the displaced directory must be removed"
            );
        }
    }

    #[test]
    fn replaced_inode_prevents_unlink() {
        let root = temp_root();
        let mut guard = InstanceGuard::acquire(Some(root.path()), TEST_DIGEST).expect("acquire");
        guard
            .publish(&guard.dir_path().join("setup.sock"), "eidnara-host/test")
            .expect("publish");
        let file = published(&guard);

        // A successor publishes over the path: same name, different inode.
        let successor = guard.dir_path().join("successor.tmp");
        std::fs::write(&successor, b"{\"schema\":1}").expect("write successor");
        std::fs::set_permissions(&successor, std::fs::Permissions::from_mode(0o600)).expect("mode");
        std::fs::rename(&successor, &file).expect("replace inode");

        guard.remove_publication();
        assert!(file.exists(), "a successor's publication must survive");
    }

    #[test]
    fn mismatched_daemon_id_prevents_unlink() {
        let root = temp_root();
        let mut guard = InstanceGuard::acquire(Some(root.path()), TEST_DIGEST).expect("acquire");
        guard
            .publish(&guard.dir_path().join("setup.sock"), "eidnara-host/test")
            .expect("publish");
        let file = published(&guard);

        // Same inode, rewritten daemon ID: an old incarnation must not delete a credential it no longer owns.
        let bytes = std::fs::read(&file).expect("read");
        let mut json: serde_json::Value = serde_json::from_slice(&bytes).expect("parse");
        json["daemon_id"] = serde_json::json!(vec![0u8; DAEMON_ID_LEN]);
        let fd = openat(CWD, &file, OFlags::WRONLY | OFlags::TRUNC, Mode::empty())
            .expect("reopen in place");
        write_all_fd(&fd, &serde_json::to_vec(&json).expect("serialize")).expect("rewrite");
        drop(fd);

        guard.remove_publication();
        assert!(file.exists(), "daemon-ID mismatch must prevent unlink");
    }

    #[test]
    fn hard_linked_publication_prevents_unlink() {
        let root = temp_root();
        let mut guard = InstanceGuard::acquire(Some(root.path()), TEST_DIGEST).expect("acquire");
        guard
            .publish(&guard.dir_path().join("setup.sock"), "eidnara-host/test")
            .expect("publish");
        let file = published(&guard);
        std::fs::hard_link(&file, guard.dir_path().join("extra-link")).expect("hard link");

        guard.remove_publication();
        assert!(
            file.exists(),
            "an unexpected second link means the file is not solely ours"
        );

        // `Drop` retries the deferred cleanup after the extra link is removed.
        std::fs::remove_file(guard.dir_path().join("extra-link")).expect("drop extra link");
        drop(guard);
        assert!(
            !file.exists(),
            "a reverted link count must not leave the key-bearing publication behind"
        );
    }

    /// A decode failure on the inode this guard published is not proof that a
    /// successor owns the name, so `Drop` must still retry once the bytes are sane.
    #[test]
    fn unparseable_bytes_on_our_own_inode_defer_rather_than_abandon_cleanup() {
        let root = temp_root();
        let mut guard = InstanceGuard::acquire(Some(root.path()), TEST_DIGEST).expect("acquire");
        guard
            .publish(&guard.dir_path().join("setup.sock"), "eidnara-host/test")
            .expect("publish");
        let file = published(&guard);
        let original = std::fs::read(&file).expect("read");

        // Truncating the file preserves its inode while making its contents unparseable.
        let fd = openat(CWD, &file, OFlags::WRONLY | OFlags::TRUNC, Mode::empty())
            .expect("reopen in place");
        write_all_fd(&fd, b"{\"schema\":").expect("corrupt");
        drop(fd);
        guard.remove_publication();
        assert!(file.exists(), "corrupt bytes must not be unlinked blindly");

        // Restoring the bytes in place leaves the inode unchanged and still ours.
        let fd = openat(CWD, &file, OFlags::WRONLY | OFlags::TRUNC, Mode::empty())
            .expect("reopen in place");
        write_all_fd(&fd, &original).expect("restore");
        drop(fd);
        drop(guard);
        assert!(
            !file.exists(),
            "cleanup must retry after a transient decode failure on our own inode"
        );
    }

    #[test]
    fn publication_survives_and_replaces_across_republish() {
        let root = temp_root();
        let mut guard = InstanceGuard::acquire(Some(root.path()), TEST_DIGEST).expect("acquire");
        guard
            .publish(&guard.dir_path().join("setup-1.sock"), "eidnara-host/test")
            .expect("first publish");
        let first: serde_json::Value =
            serde_json::from_slice(&std::fs::read(published(&guard)).expect("read"))
                .expect("parse");
        guard
            .publish(&guard.dir_path().join("setup-2.sock"), "eidnara-host/test")
            .expect("second publish");
        let second: serde_json::Value =
            serde_json::from_slice(&std::fs::read(published(&guard)).expect("read"))
                .expect("parse");

        assert!(
            first["setup_socket"]
                .as_str()
                .unwrap()
                .ends_with("setup-1.sock")
        );
        assert!(
            second["setup_socket"]
                .as_str()
                .unwrap()
                .ends_with("setup-2.sock")
        );
        // Credentials belong to the incarnation, not the publish call.
        assert_eq!(first["key"], second["key"]);
        guard.remove_publication();
        assert!(!published(&guard).exists());
    }

    #[test]
    fn stale_temps_are_swept_and_fresh_ones_spared() {
        let root = temp_root();
        // Dropping the first guard releases the lock so its successor can acquire the directory.
        let dir = {
            let guard =
                InstanceGuard::acquire(Some(root.path()), TEST_DIGEST).expect("first acquire");
            guard.dir_path().to_path_buf()
        };

        let stale = dir.join(format!(".{CONNECTION_FILE_NAME}.99999.deadbeef.tmp"));
        std::fs::write(&stale, b"stranded").expect("write stale");
        std::fs::File::options()
            .write(true)
            .open(&stale)
            .expect("open stale")
            .set_modified(SystemTime::now() - Duration::from_secs(3600))
            .expect("backdate");

        let fresh = dir.join(format!(".{CONNECTION_FILE_NAME}.99998.feedface.tmp"));
        std::fs::write(&fresh, b"in flight").expect("write fresh");

        let unrelated = dir.join("unrelated.txt");
        std::fs::write(&unrelated, b"not ours").expect("write unrelated");
        std::fs::File::options()
            .write(true)
            .open(&unrelated)
            .expect("open unrelated")
            .set_modified(SystemTime::now() - Duration::from_secs(3600))
            .expect("backdate");

        // The successor sweeps staged files at lock acquisition, before publishing, so a crash loop that never reaches publish still reclaims temps.
        let mut guard = InstanceGuard::acquire(Some(root.path()), TEST_DIGEST).expect("acquire");

        assert!(!stale.exists(), "a stale temp must be swept");
        assert!(fresh.exists(), "an in-flight temp must be spared");
        assert!(unrelated.exists(), "age alone must not condemn other files");
        guard
            .publish(&guard.dir_path().join("setup.sock"), "eidnara-host/test")
            .expect("publish");
        assert!(published(&guard).exists(), "publication must still land");
    }

    #[test]
    fn no_temp_files_remain_after_a_successful_publish() {
        let root = temp_root();
        let mut guard = InstanceGuard::acquire(Some(root.path()), TEST_DIGEST).expect("acquire");
        guard
            .publish(&guard.dir_path().join("setup.sock"), "eidnara-host/test")
            .expect("publish");

        let prefix = format!(".{CONNECTION_FILE_NAME}.");
        let leftovers: Vec<_> = std::fs::read_dir(guard.dir_path())
            .expect("read dir")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(&prefix) && name.ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "secret-bearing temps must not survive: {leftovers:?}"
        );
    }

    #[test]
    fn diagnostics_never_render_key_bytes() {
        let root = temp_root();
        let guard = InstanceGuard::acquire(Some(root.path()), TEST_DIGEST).expect("acquire");

        assert_eq!(
            format!("{:?}", guard.key()),
            format!("ConnectionKey(<{KEY_LEN} bytes redacted>)")
        );

        let key_hex: String = guard
            .key()
            .bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let key_decimals = format!("{:?}", guard.key().bytes().to_vec());
        for rendered in [format!("{:?}", guard.key()), format!("{guard:?}")] {
            assert!(!rendered.contains(&key_hex), "{rendered}");
            assert!(!rendered.contains(&key_decimals), "{rendered}");
            for window in key_hex.as_bytes().windows(16) {
                let window = std::str::from_utf8(window).expect("hex is ASCII");
                assert!(!rendered.contains(window), "{rendered} leaked {window}");
            }
        }
    }

    #[test]
    fn error_display_carries_paths_but_no_secrets() {
        let root = temp_root();
        let _holder = InstanceGuard::acquire(Some(root.path()), TEST_DIGEST).expect("acquire");
        let err = InstanceGuard::acquire(Some(root.path()), TEST_DIGEST).expect_err("second fails");
        let rendered = format!("{err}");
        assert!(rendered.contains("holds the lock"), "{rendered}");
        assert!(!rendered.contains("key"), "{rendered}");
    }
}
