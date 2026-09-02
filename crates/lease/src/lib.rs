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
        use std::os::unix::fs::PermissionsExt;
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
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options
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
            Ok(file) => {
                protect_open_file(&file, path)?;
                return Ok(file);
            }
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
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
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
        std::fs::create_dir_all(&self.base_dir).map_err(LeaseError::Io)?;
        let path = self.lease_path(key);
        let mut file = open_lease_file(&path).map_err(LeaseError::Io)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(LeaseError::Held { key: key.clone() });
            }
            Err(TryLockError::Error(e)) => return Err(LeaseError::Io(e)),
        }

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
        std::fs::create_dir_all(&self.base_dir).map_err(LeaseError::Io)?;
        let path = self.lease_path(key);
        let mut file = open_lease_file(&path).map_err(LeaseError::Io)?;
        match file.try_lock_shared() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(LeaseError::Held { key: key.clone() });
            }
            Err(TryLockError::Error(e)) => return Err(LeaseError::Io(e)),
        }

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
        (FileLeaseStore::new(dir.path()), dir)
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
    fn legacy_decimal_epoch_is_canonicalized() {
        let (store, _dir) = tmp_store();
        let k = key("legacy");
        let path = seed_epoch(&store, &k, b"41");

        let guard = store.acquire(&k).expect("acquire legacy state");
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
        let store2 = FileLeaseStore::new(dir.path());
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
                LeaseKey::new("magic-context-kernel", "sqlite", "core"),
                "1a0ede79732fcf81",
            ),
            (
                LeaseKey::new("magic-context", "sqlite", "mc_cache"),
                "3af1f17c55068a4d",
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
        let k = LeaseKey::new("magic-context-kernel", "sqlite", "core");
        let guard = store.acquire(&k).expect("acquire production key");
        drop(guard);
        let created: Vec<_> = std::fs::read_dir(dir.path())
            .expect("lease dir")
            .map(|entry| entry.expect("dir entry").file_name())
            .collect();
        assert_eq!(
            created,
            vec![std::ffi::OsString::from("1a0ede79732fcf81.lease")]
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
