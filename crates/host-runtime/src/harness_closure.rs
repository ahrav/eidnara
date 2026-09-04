//! Managed Broca harnesses use immutable, content-addressed runtime closures.
//!
//! A closure preserves the qualified package layout under `files/`.
//! The canonical manifest commits every launch root, dependency edge, extension position, source identity, file mode, size, and hash.
//!
//! Mutating entry points (`materialize`, `prune`) are not serialized by this module.
//! Callers must hold `transaction.lock` across them; `prune` additionally reclaims
//! staging temps only after [`STALE_TEMP_AFTER`] to avoid deleting an unlocked
//! concurrent `materialize`, which touches its temp root after every copied node.

use std::collections::{BTreeMap, BTreeSet};
use std::os::fd::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rustix::fd::OwnedFd;
use rustix::fs::{AtFlags, CWD, Mode, OFlags, fsync, mkdirat, openat};
use sha2::{Digest, Sha256};

use crate::file_mode::raw_mode;
use crate::instance::{
    S_IFDIR, S_IFMT, S_IFREG, hex, mode_bits, owner_uid, read_all_fd, secure_runtime_dir,
};
use crate::lifecycle::is_canonical_payload_digest;
use crate::store_fs::{
    HARDENED_DIR_FLAGS, create_owned_dir, hash_copy, is_stale_mtime, open_created_dir,
    open_dir_for_removal, open_rel_nofollow, read_dir_names, remove_tree, rename_no_replace,
    write_new_file,
};

const MANIFEST_NAME: &str = "manifest.json";
const FILES_NAME: &str = "files";
const CLOSURE_SCHEMA: &str = "eidnara.host-harness-closure/v1";
const TEMP_PREFIX: &str = ".tmp-";
const MAX_MANIFEST_BYTES: usize = 16 * 1024 * 1024;
const MAX_NODES: usize = 65_536;
const MAX_PATH_BYTES: usize = 4096;
const MAX_STRING_BYTES: usize = 1024;

/// `prune` treats a staging temp older than this as abandoned.
pub const STALE_TEMP_AFTER: Duration = crate::store_fs::STALE_TEMP_AFTER;

/// `ClosureManifest` accepts only schema-1 harness closures.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClosureManifest {
    pub schema: String,
    pub harness: String,
    pub package: String,
    pub version: String,
    pub argument_variant: String,
    pub source_roots: Vec<String>,
    pub executable: Option<String>,
    pub interpreter: Option<String>,
    pub entrypoint: Option<String>,
    pub extensions: Vec<String>,
    pub nodes: Vec<ClosureNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClosureNode {
    pub path: String,
    pub source_root: String,
    pub source_path: String,
    pub kind: NodeKind,
    pub mode: u32,
    pub size_bytes: u64,
    pub sha256: String,
    pub dependencies: Vec<ClosureDependency>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClosureDependency {
    pub path: String,
    pub kind: DependencyKind,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Interpreter,
    Executable,
    Module,
    NativeAddon,
    Extension,
    Data,
}

/// Closed dependency-edge classes. Dynamic imports must be finite and listed.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum DependencyKind {
    Static,
    FiniteDynamic,
    Native,
}

/// `ClosureCandidate` pairs qualified source roots with one exact closure manifest.
#[derive(Debug, Clone)]
pub struct ClosureCandidate {
    pub manifest: ClosureManifest,
    pub source_roots: BTreeMap<String, PathBuf>,
}

/// `ValidatedHarnessClosure` retains an open directory descriptor after validation.
pub struct ValidatedHarnessClosure {
    digest: String,
    manifest: ClosureManifest,
    path: PathBuf,
    files_fd: OwnedFd,
}

impl std::fmt::Debug for ValidatedHarnessClosure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValidatedHarnessClosure")
            .field("digest", &self.digest)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl ValidatedHarnessClosure {
    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn manifest(&self) -> &ClosureManifest {
        &self.manifest
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The node-opening method re-proves one listed node's shape, mode, size, and hash on the
    /// descriptor it hands out.
    ///
    /// The retained directory descriptor and no-follow traversal defend against pathname swaps,
    /// but retained files stay owner-writable, so a same-UID writer can overwrite bytes in place
    /// after `validate` without changing mode, link count, or size. Rehashing the opened
    /// descriptor closes that window for the object that is about to be launched; the cost is one
    /// read of the node per resolution, paid on the launch path rather than per request.
    pub fn resolve_node_descriptor(
        &self,
        node_path: &str,
    ) -> Result<ResolvedHarnessNode, HarnessClosureError> {
        let node = self
            .manifest
            .nodes
            .iter()
            .find(|node| node.path == node_path)
            .ok_or_else(|| invalid("resolved node is not listed by the manifest"))?;
        let fd = open_relative_file(&self.files_fd, node_path)
            .map_err(|_| invalid("resolved node is missing or insecure"))?;
        verify_node_file(&fd, node)?;
        // Hashing advanced the offset; a child opening `/dev/fd/N` shares it and must start at 0.
        rustix::fs::seek(&fd, rustix::fs::SeekFrom::Start(0))
            .map_err(|_| invalid("resolved node rewind failed"))?;
        Ok(ResolvedHarnessNode {
            descriptor_path: descriptor_path(fd.as_raw_fd()),
            closure_path: self.path.join(FILES_NAME).join(node_path),
            fd,
        })
    }
}

///
/// Opening Linux `/proc/self/fd/N` performs a fresh open of the underlying inode at offset 0, and symlink resolution recovers the object's real pathname.
/// macOS `/dev/fd/N` neither opens the inode at offset 0 nor resolves to the object's real pathname.
/// On macOS, the `/dev/fd/N` entry is not a symlink, so a loader cannot walk back to the containing directory.
/// `/proc/self/fd/N` and `/dev/fd/N` both support exec.
pub fn descriptor_path(fd: RawFd) -> PathBuf {
    let root = if cfg!(target_os = "macos") {
        "/dev/fd"
    } else {
        "/proc/self/fd"
    };
    PathBuf::from(root).join(fd.to_string())
}

pub const DESCRIPTOR_PATHS_ARE_FILE_LIKE: bool = !cfg!(target_os = "macos");

pub struct ResolvedHarnessNode {
    descriptor_path: PathBuf,
    closure_path: PathBuf,
    fd: OwnedFd,
}

impl ResolvedHarnessNode {
    /// The exec target path is always descriptor-rooted.
    pub fn path(&self) -> &Path {
        &self.descriptor_path
    }

    /// `module_path` is descriptor-rooted only when that path resolves like the file itself; otherwise it uses the closure pathname.
    ///
    /// A descriptor-rooted `module_path` identifies this node by descriptor number; the child
    /// must retain that descriptor through `exec` via [`Self::inherit_in_child`].
    pub fn module_path(&self) -> &Path {
        if DESCRIPTOR_PATHS_ARE_FILE_LIKE {
            &self.descriptor_path
        } else {
            &self.closure_path
        }
    }

    pub fn closure_path(&self) -> &Path {
        &self.closure_path
    }

    /// The descriptor is close-on-exec until [`Self::inherit_in_child`] runs in the child.
    pub fn inherited_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    /// The child must inherit this descriptor to use [`Self::module_path`].
    /// The inherited descriptor is `None` when `module_path` uses an ordinary pathname.
    /// The descriptor is close-on-exec until [`Self::inherit_in_child`] runs in the child.
    pub fn module_inherited_fd(&self) -> Option<RawFd> {
        DESCRIPTOR_PATHS_ARE_FILE_LIKE.then(|| self.fd.as_raw_fd())
    }

    /// `Command::pre_exec` runs after `fork` and before `exec`, so clearing close-on-exec
    /// there affects only the child's descriptor table; the parent's descriptor stays
    /// close-on-exec and unrelated spawns never inherit it.
    ///
    /// `dup2(N, N)` is not a substitute: it leaves close-on-exec set, `exec` closes the node
    /// descriptor, and the child's `/proc/self/fd/N` can name an unrelated file.
    pub fn inherit_in_child(&self) -> std::io::Result<()> {
        rustix::io::fcntl_setfd(&self.fd, rustix::io::FdFlags::empty())
            .map_err(std::io::Error::from)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessClosureError {
    detail: &'static str,
}

impl HarnessClosureError {
    pub fn detail(&self) -> &'static str {
        self.detail
    }
}

impl std::fmt::Display for HarnessClosureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "harness closure invalid: {}", self.detail)
    }
}

impl std::error::Error for HarnessClosureError {}

fn invalid(detail: &'static str) -> HarnessClosureError {
    HarnessClosureError { detail }
}

/// `digest` returns the SHA-256 of the validated canonical manifest encoding.
pub fn manifest_digest(manifest: &ClosureManifest) -> Result<String, HarnessClosureError> {
    validate_manifest(manifest)?;
    let bytes = canonical_manifest(manifest)?;
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(invalid("canonical manifest exceeds its size cap"));
    }
    Ok(hex(&Sha256::digest(bytes)))
}

/// The `to_value` hop sorts object keys: without `serde_json`'s `preserve_order` feature,
/// `serde_json::Map` is a `BTreeMap`. That feature must stay off or digests change.
fn canonical_manifest(manifest: &ClosureManifest) -> Result<Vec<u8>, HarnessClosureError> {
    let value =
        serde_json::to_value(manifest).map_err(|_| invalid("manifest serialization failed"))?;
    serde_json::to_vec_pretty(&value).map_err(|_| invalid("manifest serialization failed"))
}

pub fn validate_manifest(manifest: &ClosureManifest) -> Result<(), HarnessClosureError> {
    validate_header(manifest)?;
    let mut by_path = BTreeMap::new();
    let mut previous_path: Option<&str> = None;
    for node in &manifest.nodes {
        validate_node(node, manifest, &mut by_path, &mut previous_path)?;
    }
    let roots = collect_roots(manifest, &by_path)?;
    validate_edges_and_reachability(manifest, &by_path, roots)
}

fn validate_header(manifest: &ClosureManifest) -> Result<(), HarnessClosureError> {
    if manifest.schema != CLOSURE_SCHEMA {
        return Err(invalid("unsupported manifest schema"));
    }
    for value in [
        &manifest.harness,
        &manifest.package,
        &manifest.version,
        &manifest.argument_variant,
    ] {
        validate_identifier(value)?;
    }
    if manifest.nodes.is_empty() || manifest.nodes.len() > MAX_NODES {
        return Err(invalid("manifest node count is invalid"));
    }
    if manifest.source_roots.is_empty()
        || manifest
            .source_roots
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
    {
        return Err(invalid("manifest source roots are not uniquely sorted"));
    }
    for root in &manifest.source_roots {
        validate_identifier(root)?;
    }
    if manifest.executable.is_none()
        && !(manifest.interpreter.is_some() && manifest.entrypoint.is_some())
    {
        return Err(invalid("manifest has no complete launch root"));
    }
    if manifest.executable.is_some()
        && (manifest.interpreter.is_some() || manifest.entrypoint.is_some())
    {
        return Err(invalid(
            "manifest mixes executable and interpreted launch roots",
        ));
    }
    Ok(())
}

fn validate_node<'a>(
    node: &'a ClosureNode,
    manifest: &ClosureManifest,
    by_path: &mut BTreeMap<&'a str, &'a ClosureNode>,
    previous_path: &mut Option<&'a str>,
) -> Result<(), HarnessClosureError> {
    validate_relative_path(&node.path)?;
    validate_relative_path(&node.source_path)?;
    validate_identifier(&node.source_root)?;
    if manifest
        .source_roots
        .binary_search(&node.source_root)
        .is_err()
    {
        return Err(invalid("node source root is not declared"));
    }
    validate_hash(&node.sha256)?;
    if node.mode != 0o600 && node.mode != 0o700 {
        return Err(invalid("node mode is not owner-only"));
    }
    match node.kind {
        NodeKind::Executable | NodeKind::Interpreter if node.mode != 0o700 => {
            return Err(invalid("launch node is not executable"));
        }
        NodeKind::Module | NodeKind::NativeAddon | NodeKind::Extension | NodeKind::Data
            if node.mode != 0o600 =>
        {
            return Err(invalid("non-launch node has executable mode"));
        }
        _ => {}
    }
    if previous_path.is_some_and(|previous| previous >= node.path.as_str()) {
        return Err(invalid("manifest nodes are not uniquely sorted by path"));
    }
    // The validator checks every ancestor because path ordering can place sibling files between a parent and child.
    let mut ancestor = node.path.as_str();
    while let Some((parent, _)) = ancestor.rsplit_once('/') {
        if by_path.contains_key(parent) {
            return Err(invalid("manifest node path collides with a parent file"));
        }
        ancestor = parent;
    }
    *previous_path = Some(&node.path);
    if by_path.insert(node.path.as_str(), node).is_some() {
        return Err(invalid("manifest contains a duplicate node path"));
    }
    let mut previous_dependency: Option<&str> = None;
    for dependency in &node.dependencies {
        validate_relative_path(&dependency.path)?;
        // Dependency validation rejects duplicate `path` values regardless of `kind`.
        if previous_dependency.is_some_and(|previous| previous >= dependency.path.as_str()) {
            return Err(invalid(
                "node dependencies are not uniquely sorted by target path",
            ));
        }
        previous_dependency = Some(&dependency.path);
    }
    Ok(())
}

fn collect_roots<'a>(
    manifest: &'a ClosureManifest,
    by_path: &BTreeMap<&str, &ClosureNode>,
) -> Result<Vec<&'a str>, HarnessClosureError> {
    let mut roots = Vec::new();
    for (path, expected_kind) in [
        (manifest.executable.as_deref(), NodeKind::Executable),
        (manifest.interpreter.as_deref(), NodeKind::Interpreter),
    ] {
        if let Some(path) = path {
            require_root(by_path, path, expected_kind)?;
            roots.push(path);
        }
    }
    if let Some(path) = manifest.entrypoint.as_deref() {
        let node = require_existing_node(by_path, path)?;
        if !matches!(node.kind, NodeKind::Module | NodeKind::Executable) {
            return Err(invalid("entrypoint has an invalid node kind"));
        }
        roots.push(path);
    }
    let mut seen_extensions = BTreeSet::new();
    for path in &manifest.extensions {
        validate_relative_path(path)?;
        if !seen_extensions.insert(path.as_str()) {
            return Err(invalid("manifest contains a duplicate extension"));
        }
        require_root(by_path, path, NodeKind::Extension)?;
        roots.push(path);
    }
    Ok(roots)
}

fn validate_edges_and_reachability(
    manifest: &ClosureManifest,
    by_path: &BTreeMap<&str, &ClosureNode>,
    roots: Vec<&str>,
) -> Result<(), HarnessClosureError> {
    // Each `native` edge must target a `native_addon`, and each `native_addon` must have a `native` edge.
    let mut native_targets = BTreeSet::new();
    for node in &manifest.nodes {
        for dependency in &node.dependencies {
            let target = require_existing_node(by_path, &dependency.path)?;
            if (dependency.kind == DependencyKind::Native) != (target.kind == NodeKind::NativeAddon)
            {
                return Err(invalid(
                    "native dependency kind must correspond exactly to a native addon target",
                ));
            }
            if dependency.kind == DependencyKind::Native {
                native_targets.insert(dependency.path.as_str());
            }
        }
    }
    for node in &manifest.nodes {
        if node.kind == NodeKind::NativeAddon && !native_targets.contains(node.path.as_str()) {
            return Err(invalid(
                "native addon lacks an explicit native dependency edge",
            ));
        }
    }

    let mut reachable = BTreeSet::new();
    let mut pending = roots;
    while let Some(path) = pending.pop() {
        if !reachable.insert(path) {
            continue;
        }
        let node = by_path
            .get(path)
            .expect("launch and dependency roots were checked above");
        pending.extend(
            node.dependencies
                .iter()
                .map(|dependency| dependency.path.as_str()),
        );
    }
    if reachable.len() != manifest.nodes.len() {
        return Err(invalid("manifest contains an unreachable node"));
    }
    Ok(())
}

fn require_existing_node<'a>(
    nodes: &'a BTreeMap<&str, &'a ClosureNode>,
    path: &str,
) -> Result<&'a ClosureNode, HarnessClosureError> {
    nodes
        .get(path)
        .copied()
        .ok_or_else(|| invalid("manifest references a missing node"))
}

fn require_root(
    nodes: &BTreeMap<&str, &ClosureNode>,
    path: &str,
    kind: NodeKind,
) -> Result<(), HarnessClosureError> {
    validate_relative_path(path)?;
    if require_existing_node(nodes, path)?.kind != kind {
        return Err(invalid("launch root has an invalid node kind"));
    }
    Ok(())
}

fn validate_identifier(value: &str) -> Result<(), HarnessClosureError> {
    if value.is_empty()
        || value.len() > MAX_STRING_BYTES
        || value
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
    {
        return Err(invalid("manifest identifier is invalid"));
    }
    Ok(())
}

fn validate_relative_path(path: &str) -> Result<(), HarnessClosureError> {
    if path.is_empty() || path.len() > MAX_PATH_BYTES || path.as_bytes().contains(&0) {
        return Err(invalid("manifest path length is invalid"));
    }
    if path.split('/').any(|part| {
        part.is_empty()
            || part == "."
            || part == ".."
            || part.len() > 255
            || part.as_bytes().contains(&b'\\')
    }) {
        return Err(invalid("manifest path has an invalid component"));
    }
    Ok(())
}

fn validate_hash(hash: &str) -> Result<(), HarnessClosureError> {
    if !is_canonical_payload_digest(hash) {
        return Err(invalid("manifest hash is not canonical sha256"));
    }
    Ok(())
}

/// The store's direct children are canonical manifest digests.
pub struct HarnessClosureStore {
    root: PathBuf,
    root_fd: OwnedFd,
}

impl std::fmt::Debug for HarnessClosureStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HarnessClosureStore")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl HarnessClosureStore {
    /// The operation opens or creates an owner-only store without following symlinks in any path component.
    /// An existing owned root with a wider mode is repaired to `0o700` through its pinned descriptor.
    pub fn open(root: &Path) -> Result<Self, HarnessClosureError> {
        let root_fd =
            secure_runtime_dir(root).map_err(|_| invalid("closure store path is insecure"))?;
        verify_owned_directory(&root_fd)?;
        Ok(Self {
            root: root.to_path_buf(),
            root_fd,
        })
    }

    /// An existing digest directory that fails validation is a torn closure (crash between
    /// promotion and durability, or later corruption). Because the caller holds
    /// `transaction.lock`, nothing else owns it, so `materialize` removes it and restages.
    ///
    /// A "closure store fsync failed" error after promotion means the digest may already be
    /// named by the store but its durability is unproven; retrying revalidates it in place.
    pub fn materialize(
        &self,
        candidate: &ClosureCandidate,
    ) -> Result<ValidatedHarnessClosure, HarnessClosureError> {
        validate_manifest(&candidate.manifest)?;
        validate_source_root_set(candidate)?;
        let digest = manifest_digest(&candidate.manifest)?;
        if let Ok(validated) = self.validate(&digest) {
            return Ok(validated);
        }
        if child_exists(&self.root_fd, &digest)? {
            if open_dir_for_removal(&self.root_fd, &digest).is_err() {
                return Err(invalid("digest target exists but is invalid"));
            }
            remove_tree(&self.root_fd, &digest)
                .map_err(|_| invalid("torn closure removal failed"))?;
        }

        let (temp_name, temp_fd) = self.create_temp()?;
        let staged = self.stage_candidate(&temp_fd, candidate);
        if let Err(error) = staged {
            let _ = remove_tree(&self.root_fd, &temp_name);
            return Err(error);
        }
        match rename_no_replace(&self.root_fd, &temp_name, &digest) {
            Ok(true) => {
                fsync(&self.root_fd).map_err(|_| invalid("closure store fsync failed"))?;
            }
            Ok(false) => {
                // The occupant is the result; a failed temp removal must not mask it. A temp that
                // survives here ages past `STALE_TEMP_AFTER` and is reclaimed by `prune`.
                let _ = remove_tree(&self.root_fd, &temp_name);
                return self.validate(&digest);
            }
            Err(_) => {
                let _ = remove_tree(&self.root_fd, &temp_name);
                return Err(invalid("closure promotion failed"));
            }
        }
        self.validate(&digest)
    }

    /// `prune` preserves staging temps younger than [`STALE_TEMP_AFTER`] because an
    /// in-flight `materialize` may still own them.
    ///
    /// One unremovable entry does not stop the sweep: every reclaimable entry is removed
    /// first, then the first removal error is returned.
    pub fn prune(&self, protected: &BTreeSet<String>) -> Result<(), HarnessClosureError> {
        let mut first_error = None;
        for name in list_names(&self.root_fd)? {
            if protected.contains(&name) {
                continue;
            }
            if name.starts_with(TEMP_PREFIX) {
                if !is_stale_temp(&self.root_fd, &name) {
                    continue;
                }
            } else if validate_hash(&name).is_err() {
                continue;
            }
            if remove_tree(&self.root_fd, &name).is_err() {
                first_error.get_or_insert(invalid("closure entry removal failed"));
            }
        }
        fsync(&self.root_fd).map_err(|_| invalid("closure store fsync failed"))?;
        first_error.map_or(Ok(()), Err)
    }

    pub fn validate(&self, digest: &str) -> Result<ValidatedHarnessClosure, HarnessClosureError> {
        validate_hash(digest)?;
        let dir_fd = open_owned_dir(&self.root_fd, digest)?;
        let manifest_fd = open_direct_file(&dir_fd, MANIFEST_NAME)?;
        verify_secure_file(&manifest_fd, 0o600)?;
        let bytes = read_all_fd(&manifest_fd, MAX_MANIFEST_BYTES)
            .map_err(|_| invalid("closure manifest read failed"))?;
        if hex(&Sha256::digest(&bytes)) != digest {
            return Err(invalid("manifest bytes do not match the closure digest"));
        }
        let manifest: ClosureManifest =
            serde_json::from_slice(&bytes).map_err(|_| invalid("manifest decoding failed"))?;
        validate_manifest(&manifest)?;
        let canonical = canonical_manifest(&manifest)?;
        if canonical != bytes {
            return Err(invalid("retained manifest is not canonical"));
        }

        let files_fd = open_owned_dir(&dir_fd, FILES_NAME)?;
        let expected: BTreeMap<&str, &ClosureNode> = manifest
            .nodes
            .iter()
            .map(|node| (node.path.as_str(), node))
            .collect();
        let mut found = BTreeSet::new();
        validate_tree(&files_fd, "", &expected, &mut found)?;
        if found.len() != expected.len() {
            return Err(invalid("closure is missing a manifest-listed node"));
        }
        let entries = list_names(&dir_fd)?;
        if entries != BTreeSet::from([FILES_NAME.to_owned(), MANIFEST_NAME.to_owned()]) {
            return Err(invalid("closure directory contains an unlisted entry"));
        }
        Ok(ValidatedHarnessClosure {
            digest: digest.to_owned(),
            manifest,
            path: self.root.join(digest),
            files_fd,
        })
    }

    fn create_temp(&self) -> Result<(String, OwnedFd), HarnessClosureError> {
        let mut random = [0u8; 12];
        getrandom::getrandom(&mut random)
            .map_err(|_| invalid("temporary name generation failed"))?;
        let name = format!("{TEMP_PREFIX}{}", hex(&random));
        mkdirat(&self.root_fd, name.as_str(), Mode::from_raw_mode(0o700))
            .map_err(|_| invalid("temporary closure creation failed"))?;
        match open_created_dir(&self.root_fd, &name) {
            Ok(fd) => {
                verify_owned_directory(&fd)?;
                Ok((name, fd))
            }
            Err(_) => {
                let _ = remove_tree(&self.root_fd, &name);
                Err(invalid("temporary closure open failed"))
            }
        }
    }

    fn stage_candidate(
        &self,
        temp_fd: &OwnedFd,
        candidate: &ClosureCandidate,
    ) -> Result<(), HarnessClosureError> {
        let files_fd = create_owned_dir(temp_fd, FILES_NAME)
            .map_err(|_| invalid("closure files directory creation failed"))?;
        let source_fds = open_source_roots(&candidate.source_roots)?;
        // Fsyncing a file does not persist its dirent, so every directory that received an entry is fsynced below.
        let mut dirs: BTreeSet<&str> = BTreeSet::new();
        for node in &candidate.manifest.nodes {
            let source_root = source_fds
                .get(&node.source_root)
                .ok_or_else(|| invalid("node source root is missing"))?;
            copy_node(source_root, &files_fd, node)?;
            let mut ancestor = node.path.as_str();
            while let Some((parent, _)) = ancestor.rsplit_once('/') {
                dirs.insert(parent);
                ancestor = parent;
            }
            // The temp root's mtime is `prune`'s liveness signal; node writes land under `files/` and would not refresh it.
            touch(temp_fd).map_err(|_| invalid("temporary closure touch failed"))?;
        }
        // Reverse lexicographic order visits every child before its parent, so each directory's
        // entries are durable before its own dirent.
        for rel in dirs.iter().rev() {
            let dir = open_rel_nofollow(&files_fd, rel, true)
                .map_err(|_| invalid("closure layout directory reopen failed"))?;
            fsync(&dir).map_err(|_| invalid("closure layout directory fsync failed"))?;
        }
        fsync(&files_fd).map_err(|_| invalid("closure files fsync failed"))?;
        let bytes = canonical_manifest(&candidate.manifest)?;
        let manifest_fd = write_new_file(temp_fd, MANIFEST_NAME, &bytes, 0o600)
            .map_err(|_| invalid("closure metadata write failed"))?;
        verify_secure_file(&manifest_fd, 0o600)?;
        fsync(temp_fd).map_err(|_| invalid("temporary closure fsync failed"))?;
        Ok(())
    }
}

fn touch(fd: &OwnedFd) -> rustix::io::Result<()> {
    let now = rustix::fs::Timespec {
        tv_sec: 0,
        tv_nsec: rustix::fs::UTIME_NOW,
    };
    rustix::fs::futimens(
        fd,
        &rustix::fs::Timestamps {
            last_access: now,
            last_modification: now,
        },
    )
}

fn validate_source_root_set(candidate: &ClosureCandidate) -> Result<(), HarnessClosureError> {
    let expected: BTreeSet<&str> = candidate
        .manifest
        .source_roots
        .iter()
        .map(String::as_str)
        .collect();
    let actual: BTreeSet<&str> = candidate.source_roots.keys().map(String::as_str).collect();
    if expected != actual {
        return Err(invalid(
            "candidate source roots do not exactly match the manifest",
        ));
    }
    Ok(())
}

fn open_source_roots(
    roots: &BTreeMap<String, PathBuf>,
) -> Result<BTreeMap<String, OwnedFd>, HarnessClosureError> {
    roots
        .iter()
        .map(|(name, path)| {
            let fd = openat(CWD, path, HARDENED_DIR_FLAGS, Mode::empty())
                .map_err(|_| invalid("source root open failed"))?;
            let stat = rustix::fs::fstat(&fd).map_err(|_| invalid("source root stat failed"))?;
            if mode_bits(&stat) & S_IFMT != S_IFDIR {
                return Err(invalid("source root is not a directory"));
            }
            Ok((name.clone(), fd))
        })
        .collect()
}

fn copy_node(
    source_root: &OwnedFd,
    files_root: &OwnedFd,
    node: &ClosureNode,
) -> Result<(), HarnessClosureError> {
    let source = open_relative_file(source_root, &node.source_path)
        .map_err(|_| invalid("source node is missing or insecure"))?;
    let before = rustix::fs::fstat(&source).map_err(|_| invalid("source node stat failed"))?;
    if mode_bits(&before) & S_IFMT != S_IFREG || before.st_size as u64 != node.size_bytes {
        return Err(invalid("source node shape or size diverges from manifest"));
    }

    let (parent, basename) = create_parent_dirs(files_root, &node.path)?;
    let destination = openat(
        &parent,
        basename.as_str(),
        OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(raw_mode(node.mode)),
    )
    .map_err(|_| invalid("closure node creation failed"))?;
    rustix::fs::fchmod(&destination, Mode::from_raw_mode(raw_mode(node.mode)))
        .map_err(|_| invalid("closure node chmod failed"))?;

    let (copied, sha256) =
        hash_copy(&source, Some(&destination), node.size_bytes).map_err(|error| {
            if error.kind() == std::io::ErrorKind::InvalidData {
                invalid("source node grew during copy")
            } else {
                invalid("closure node copy failed")
            }
        })?;
    fsync(&destination).map_err(|_| invalid("closure node fsync failed"))?;
    let after = rustix::fs::fstat(&source).map_err(|_| invalid("source node stat failed"))?;
    if !same_file_snapshot(&before, &after) || copied != node.size_bytes || sha256 != node.sha256 {
        return Err(invalid("source node bytes diverge from manifest"));
    }
    verify_secure_file(&destination, node.mode)?;
    Ok(())
}

fn same_file_snapshot(before: &rustix::fs::Stat, after: &rustix::fs::Stat) -> bool {
    #[allow(clippy::unnecessary_cast)]
    {
        before.st_dev as u64 == after.st_dev as u64
            && before.st_ino as u64 == after.st_ino as u64
            && before.st_size == after.st_size
            && before.st_mtime == after.st_mtime
            && before.st_mtime_nsec == after.st_mtime_nsec
    }
}

fn create_parent_dirs(
    root: &OwnedFd,
    relative: &str,
) -> Result<(OwnedFd, String), HarnessClosureError> {
    let mut parts = relative.split('/').peekable();
    let mut current = crate::store_fs::dup_cloexec(root)
        .map_err(|_| invalid("closure files descriptor dup failed"))?;
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            return Ok((current, part.to_owned()));
        }
        current = match mkdirat(&current, part, Mode::from_raw_mode(0o700)) {
            Ok(()) => {
                let fd = open_created_dir(&current, part)
                    .map_err(|_| invalid("closure layout directory open failed"))?;
                verify_owned_directory(&fd)?;
                fd
            }
            Err(rustix::io::Errno::EXIST) => open_owned_dir(&current, part)?,
            Err(_) => return Err(invalid("closure layout directory creation failed")),
        };
    }
    Err(invalid("closure node path is empty"))
}

fn validate_tree(
    dir: &OwnedFd,
    prefix: &str,
    expected: &BTreeMap<&str, &ClosureNode>,
    found: &mut BTreeSet<String>,
) -> Result<(), HarnessClosureError> {
    for name in list_names(dir)? {
        let relative = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let fd = openat(
            dir,
            name.as_str(),
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(|_| invalid("closure tree entry open failed"))?;
        let stat = rustix::fs::fstat(&fd).map_err(|_| invalid("closure tree entry stat failed"))?;
        match mode_bits(&stat) & S_IFMT {
            S_IFDIR => {
                let prefix = format!("{relative}/");
                // Keys sharing `prefix` are contiguous in the sorted map, so the first key
                // at or after `prefix` decides membership in O(log n) rather than a full scan.
                let listed = expected
                    .range(prefix.as_str()..)
                    .next()
                    .is_some_and(|(path, _)| path.starts_with(prefix.as_str()));
                if !listed {
                    return Err(invalid("closure contains an unlisted directory"));
                }
                verify_owned_directory(&fd)?;
                validate_tree(&fd, &relative, expected, found)?;
            }
            S_IFREG => {
                let node = expected
                    .get(relative.as_str())
                    .ok_or_else(|| invalid("closure contains an unlisted file"))?;
                verify_node_file(&fd, node)?;
                found.insert(relative);
            }
            _ => return Err(invalid("closure contains a non-regular entry")),
        }
    }
    Ok(())
}

fn verify_node_file(fd: &OwnedFd, node: &ClosureNode) -> Result<(), HarnessClosureError> {
    verify_secure_file(fd, node.mode)?;
    let stat = rustix::fs::fstat(fd).map_err(|_| invalid("closure node stat failed"))?;
    if stat.st_size as u64 != node.size_bytes {
        return Err(invalid("closure node size diverges from manifest"));
    }
    let (total, sha256) = hash_copy(fd, None, node.size_bytes).map_err(|error| {
        if error.kind() == std::io::ErrorKind::InvalidData {
            invalid("closure node grew past its manifest size")
        } else {
            invalid("closure node read failed")
        }
    })?;
    if total != node.size_bytes || sha256 != node.sha256 {
        return Err(invalid("closure node hash diverges from manifest"));
    }
    Ok(())
}

fn verify_secure_file(fd: &OwnedFd, expected_mode: u32) -> Result<(), HarnessClosureError> {
    let stat = rustix::fs::fstat(fd).map_err(|_| invalid("closure file stat failed"))?;
    let mode = mode_bits(&stat);
    if mode & S_IFMT != S_IFREG
        || stat.st_uid != owner_uid()
        || stat.st_nlink != 1
        || mode & 0o7777 != expected_mode
    {
        return Err(invalid("closure file is not owner-only single-link"));
    }
    Ok(())
}

fn open_relative_file(root: &OwnedFd, relative: &str) -> Result<OwnedFd, HarnessClosureError> {
    validate_relative_path(relative)?;
    open_rel_nofollow(root, relative, false).map_err(|_| invalid("relative file traversal failed"))
}

fn open_direct_file(parent: &OwnedFd, name: &str) -> Result<OwnedFd, HarnessClosureError> {
    openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(|_| invalid("closure file open failed"))
}

fn open_owned_dir(parent: &OwnedFd, name: &str) -> Result<OwnedFd, HarnessClosureError> {
    let fd = openat(parent, name, HARDENED_DIR_FLAGS, Mode::empty())
        .map_err(|_| invalid("closure directory open failed"))?;
    verify_owned_directory(&fd)?;
    Ok(fd)
}

fn verify_owned_directory(fd: &OwnedFd) -> Result<(), HarnessClosureError> {
    let stat = rustix::fs::fstat(fd).map_err(|_| invalid("closure directory stat failed"))?;
    let mode = mode_bits(&stat);
    if mode & S_IFMT != S_IFDIR || stat.st_uid != owner_uid() || mode & 0o7777 != 0o700 {
        return Err(invalid("closure directory is not owner-only"));
    }
    Ok(())
}

fn list_names(dir: &OwnedFd) -> Result<BTreeSet<String>, HarnessClosureError> {
    let names = read_dir_names(dir).map_err(|_| invalid("closure directory read failed"))?;
    if names.iter().any(|name| name.len() > 255) {
        return Err(invalid("closure entry name is invalid"));
    }
    Ok(names.into_iter().collect())
}

fn child_exists(parent: &OwnedFd, name: &str) -> Result<bool, HarnessClosureError> {
    match rustix::fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => Ok(true),
        Err(rustix::io::Errno::NOENT) => Ok(false),
        Err(_) => Err(invalid("digest target stat failed")),
    }
}

fn is_stale_temp(parent: &OwnedFd, name: &str) -> bool {
    rustix::fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)
        .is_ok_and(|stat| is_stale_mtime(stat.st_mtime))
}
