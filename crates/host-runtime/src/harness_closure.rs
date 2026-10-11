//! Managed ModelExecution harnesses use immutable, content-addressed runtime closures.
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
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::Duration;

use rustix::fd::OwnedFd;
use rustix::fs::{AtFlags, CWD, Mode, OFlags, fsync, mkdirat, openat};
use sha2::{Digest, Sha256};

use crate::file_mode::raw_mode;
use crate::instance::{
    S_IFDIR, S_IFMT, S_IFREG, hex, mode_bits, owner_uid, read_all_fd, secure_runtime_dir,
    stat_identity,
};
use crate::lifecycle::is_canonical_payload_digest;
use crate::store_fs::{
    HARDENED_DIR_FLAGS, HASH_BUFFER_BYTES, MAX_PATH_COMPONENTS, create_owned_dir, exchange_dirs,
    hash_copy, hash_copy_with, hash_pipelined, is_stale_mtime, is_temp_name, open_created_dir,
    open_dir_for_removal, open_or_create_parents, open_rel_nofollow, path_names_descriptor,
    read_dir_names, read_dir_names_partitioned, remove_tree, rename_no_replace, same_snapshot,
    write_new_file,
};

const MANIFEST_NAME: &str = "manifest.json";
const FILES_NAME: &str = "files";
/// The only schema id `validate` accepts.
pub const CLOSURE_SCHEMA: &str = "eidnara.host-harness-closure/v1";
const TEMP_PREFIX: &str = ".tmp-";
/// `create_temp` draws 12 random bytes, so a temp name carries 24 hex digits.
const TEMP_HEX_LEN: usize = 24;
const MAX_MANIFEST_BYTES: usize = 16 * 1024 * 1024;
const MAX_NODES: usize = 65_536;
/// Every declared root is opened and held for the duration of staging, so the count is bounded
/// well below common `RLIMIT_NOFILE` soft limits.
const MAX_SOURCE_ROOTS: usize = 64;
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
    root: PathBuf,
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
    ///
    /// `closure_path` must still identify the verified inode; pathname-based consumers such as
    /// the macOS `module_path` could otherwise launch a replacement.
    /// Re-verifies the whole closure — every node's mode, link count, size, and hash — and returns a fresh handle pinned to the re-verified tree.
    ///
    /// `resolve_node_descriptor` re-hashes only the object it opens, but a Node entrypoint loads its transitive dependencies by pathname, and retained files stay owner-writable. Launching from a handle obtained immediately before spawn narrows the window in which a same-UID writer can alter a dependency from "since startup" to the launch itself.
    pub fn revalidate(&self) -> Result<ValidatedHarnessClosure, HarnessClosureError> {
        HarnessClosureStore::open(&self.root)?.validate(&self.digest)
    }

    /// [`Self::revalidate`] that also resolves `nodes`, in order, from the same pass.
    ///
    /// Each resolved descriptor is the one whose bytes the re-verification hashed, so it carries the
    /// proof [`Self::resolve_node_descriptor`] gives without a second read of the node. A name listed
    /// twice, or one the manifest does not list, resolves through [`Self::resolve_node_descriptor`].
    pub fn revalidate_resolving(
        &self,
        nodes: &[&str],
    ) -> Result<(ValidatedHarnessClosure, Vec<ResolvedHarnessNode>), HarnessClosureError> {
        let (fresh, mut retained) =
            HarnessClosureStore::open(&self.root)?.validate_retaining(&self.digest, nodes)?;
        let resolved = nodes
            .iter()
            .zip(retained.iter_mut())
            .map(|(node, fd)| match fd.take() {
                Some(fd) => fresh.hand_out(node, fd),
                None => fresh.resolve_node_descriptor(node),
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok((fresh, resolved))
    }

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
        self.hand_out(node_path, fd)
    }

    /// Hands out `fd`, a verified descriptor for `node_path`, once the closure pathname still names its inode.
    fn hand_out(
        &self,
        node_path: &str,
        fd: OwnedFd,
    ) -> Result<ResolvedHarnessNode, HarnessClosureError> {
        let closure_path = self.path.join(FILES_NAME).join(node_path);
        let verified = rustix::fs::fstat(&fd).map_err(|_| invalid("closure node stat failed"))?;
        // Re-resolving through the store root one component at a time avoids `ENAMETOOLONG`
        // on a single joined pathname.
        let by_name = openat(CWD, &self.root, HARDENED_DIR_FLAGS, Mode::empty())
            .and_then(|root| {
                open_rel_nofollow(
                    &root,
                    &format!("{}/{FILES_NAME}/{node_path}", self.digest),
                    false,
                )
            })
            .and_then(|fd| rustix::fs::fstat(&fd))
            .map_err(|_| invalid("closure pathname no longer names the validated node"))?;
        #[allow(clippy::unnecessary_cast)]
        let same_inode = verified.st_dev as u64 == by_name.st_dev as u64
            && verified.st_ino as u64 == by_name.st_ino as u64;
        if !same_inode {
            return Err(invalid(
                "closure pathname no longer names the validated node",
            ));
        }
        // Hashing advanced the offset; a child opening `/dev/fd/N` shares it and must start at 0.
        rustix::fs::seek(&fd, rustix::fs::SeekFrom::Start(0))
            .map_err(|_| invalid("resolved node rewind failed"))?;
        Ok(ResolvedHarnessNode {
            descriptor_path: descriptor_path(fd.as_raw_fd()),
            closure_path,
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
    ///
    /// A pathname-based `module_path` is verified against the resolved inode at resolution time only.
    /// On macOS the caller must hold `transaction.lock` from `resolve_node_descriptor` through the child's `exec`, because that lock excludes `materialize` and `prune`, the only writers that rename digest directories.
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
    if manifest.source_roots.len() > MAX_SOURCE_ROOTS {
        return Err(invalid("manifest declares too many source roots"));
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
    if path.split('/').count() > MAX_PATH_COMPONENTS {
        return Err(invalid("manifest path is too deep"));
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
    /// An existing owned root that group or other principals could write is rejected, since it may already hold planted files; a root with only wider read or execute bits (such as `0o755`) is tightened to `0o700` through its pinned descriptor.
    ///
    /// A relative `root` is made absolute against the working directory at open time, so the
    /// pathnames a closure hands out keep naming the opened store after a later `chdir`.
    pub fn open(root: &Path) -> Result<Self, HarnessClosureError> {
        let root = std::path::absolute(root)
            .map_err(|_| invalid("closure store path cannot be made absolute"))?;
        let root_fd =
            secure_runtime_dir(&root).map_err(|_| invalid("closure store path is insecure"))?;
        verify_owned_directory(&root_fd)?;
        Ok(Self { root, root_fd })
    }

    /// An existing digest directory that fails validation is a torn closure (crash between
    /// promotion and durability, or later corruption). The replacement is staged in full before
    /// the occupant is touched and then swapped in atomically, so a staging failure leaves the
    /// store unchanged and the digest name is never absent. Because the caller holds
    /// `transaction.lock`, nothing else mutates the occupant during the swap.
    ///
    /// `protected` contains digests whose retained harness descriptors may still reference their trees.
    /// Repair unlinks the swapped-out tree, so a corrupt protected digest is refused rather than repaired; the caller decides when a retry is safe.
    ///
    /// A "closure store fsync failed" error after promotion means the digest may already be
    /// named by the store but its durability is unproven; retrying revalidates it in place.
    pub fn materialize(
        &self,
        candidate: &ClosureCandidate,
        protected: &BTreeSet<String>,
    ) -> Result<ValidatedHarnessClosure, HarnessClosureError> {
        validate_manifest(&candidate.manifest)?;
        validate_source_root_set(candidate)?;
        let digest = manifest_digest(&candidate.manifest)?;
        if let Ok(validated) = self.validate(&digest) {
            // A prior run may have crashed between its rename and the root fsync, so the
            // occupant's dirent is made durable before it is reported as materialized.
            fsync(&self.root_fd).map_err(|_| invalid("closure store fsync failed"))?;
            self.verify_named_identity()?;
            return Ok(validated);
        }

        let (temp_name, temp_fd) = self.create_temp()?;
        let promoted = self
            .stage_candidate(&temp_fd, candidate)
            .and_then(|()| self.promote_temp(&temp_name, &digest, protected));
        // After a successful exchange the torn tree sits at `temp_name`; after any failure the
        // staged tree does. Either way the temp is discarded, best effort: a survivor ages past
        // `STALE_TEMP_AFTER` and `prune` reclaims it.
        let _ = remove_tree(&self.root_fd, &temp_name);
        promoted?;
        fsync(&self.root_fd).map_err(|_| invalid("closure store fsync failed"))?;
        self.verify_named_identity()?;
        self.validate(&digest)
    }

    /// The named store root must still resolve to `root_fd`; a root renamed and replaced after
    /// `open` holds only detached writes.
    fn verify_named_identity(&self) -> Result<(), HarnessClosureError> {
        match path_names_descriptor(&self.root, &self.root_fd) {
            Ok(true) => Ok(()),
            Ok(false) => Err(invalid("closure store was replaced under the mutator")),
            Err(_) => Err(invalid("closure store identity check failed")),
        }
    }

    /// Moves the staged temp to `digest`. A valid occupant wins and the temp is left for the
    /// caller to discard.
    fn promote_temp(
        &self,
        temp_name: &str,
        digest: &str,
        protected: &BTreeSet<String>,
    ) -> Result<(), HarnessClosureError> {
        match rename_no_replace(&self.root_fd, temp_name, digest) {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(_) => return Err(invalid("closure promotion failed")),
        }
        if self.validate(digest).is_ok() {
            return Ok(());
        }
        if protected.contains(digest) {
            return Err(invalid("corrupt digest target is protected"));
        }
        if open_dir_for_removal(&self.root_fd, digest).is_err() {
            return Err(invalid("digest target exists but is invalid"));
        }
        exchange_dirs(&self.root_fd, temp_name, digest)
            .map_err(|_| invalid("torn closure exchange failed"))
    }

    /// `prune` preserves staging temps younger than [`STALE_TEMP_AFTER`] because an
    /// in-flight `materialize` may still own them. Only exact temp names and canonical digests
    /// are ever removed; a foreign or non-UTF-8 name is left alone.
    ///
    /// One unremovable entry does not stop the sweep: every reclaimable entry is removed
    /// first, then the first removal error is returned.
    pub fn prune(&self, protected: &BTreeSet<String>) -> Result<(), HarnessClosureError> {
        let mut first_error = None;
        let (names, _) = read_dir_names_partitioned(&self.root_fd)
            .map_err(|_| invalid("closure directory read failed"))?;
        for name in names {
            if protected.contains(&name) {
                continue;
            }
            if is_temp_name(&name, TEMP_PREFIX, TEMP_HEX_LEN) {
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
        self.verify_named_identity()?;
        first_error.map_or(Ok(()), Err)
    }

    pub fn validate(&self, digest: &str) -> Result<ValidatedHarnessClosure, HarnessClosureError> {
        self.validate_retaining(digest, &[])
            .map(|(closure, _)| closure)
    }

    /// [`Self::validate`] that keeps the verified descriptor of each listed file in `retain`.
    /// The returned descriptors align with `retain`; a name the walk did not admit, or the second
    /// listing of a name, has no descriptor.
    fn validate_retaining(
        &self,
        digest: &str,
        retain: &[&str],
    ) -> Result<(ValidatedHarnessClosure, Vec<Option<OwnedFd>>), HarnessClosureError> {
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
        let check_manifest = || {
            validate_manifest(&manifest)?;
            if canonical_manifest(&manifest)? != bytes {
                return Err(invalid("retained manifest is not canonical"));
            }
            Ok(())
        };
        // The file walk reads only names it lists and uses the manifest as a lookup table, so it runs
        // beside the manifest's own checks; a manifest failure still takes precedence over every file
        // failure, as it would in sequence.
        let walk_files = || {
            let files_fd = open_owned_dir(&dir_fd, FILES_NAME)?;
            let expected: BTreeMap<&str, &ClosureNode> = manifest
                .nodes
                .iter()
                .map(|node| (node.path.as_str(), node))
                .collect();
            let mut slots: BTreeMap<&str, usize> = BTreeMap::new();
            for (slot, name) in retain.iter().enumerate() {
                slots.entry(name).or_insert(slot);
            }
            let (found, retained) = validate_files(&files_fd, &expected, &slots, retain.len())?;
            Ok((files_fd, found == expected.len(), retained))
        };
        let (files_fd, complete, retained) = std::thread::scope(|scope| {
            let Ok(checking) = std::thread::Builder::new()
                .name("closure-manifest".to_owned())
                .spawn_scoped(scope, check_manifest)
            else {
                check_manifest()?;
                return walk_files();
            };
            let walked = walk_files();
            checking
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic))?;
            walked
        })?;
        if !complete {
            return Err(invalid("closure is missing a manifest-listed node"));
        }
        let entries = list_names(&dir_fd)?;
        if entries != BTreeSet::from([FILES_NAME.to_owned(), MANIFEST_NAME.to_owned()]) {
            return Err(invalid("closure directory contains an unlisted entry"));
        }
        Ok((
            ValidatedHarnessClosure {
                digest: digest.to_owned(),
                manifest,
                root: self.root.clone(),
                path: self.root.join(digest),
                files_fd,
            },
            retained,
        ))
    }

    fn create_temp(&self) -> Result<(String, OwnedFd), HarnessClosureError> {
        let mut random = [0u8; 12];
        getrandom::getrandom(&mut random)
            .map_err(|_| invalid("temporary name generation failed"))?;
        let name = format!("{TEMP_PREFIX}{}", hex(&random));
        mkdirat(&self.root_fd, name.as_str(), Mode::from_raw_mode(0o700))
            .map_err(|_| invalid("temporary closure creation failed"))?;
        // The error path removes the unreturned directory so retries do not accumulate `.tmp-*` directories.
        let opened = open_created_dir(&self.root_fd, &name)
            .map_err(|_| invalid("temporary closure open failed"))
            .and_then(|fd| verify_owned_directory(&fd).map(|()| fd));
        match opened {
            Ok(fd) => Ok((name, fd)),
            Err(error) => {
                let _ = remove_tree(&self.root_fd, &name);
                Err(error)
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
    if !same_snapshot(&before, &after) || copied != node.size_bytes || sha256 != node.sha256 {
        return Err(invalid("source node bytes diverge from manifest"));
    }
    verify_secure_file(&destination, node.mode)?;
    Ok(())
}

fn create_parent_dirs(
    root: &OwnedFd,
    relative: &str,
) -> Result<(OwnedFd, String), HarnessClosureError> {
    open_or_create_parents(root, relative).map_err(|e| match e {
        rustix::io::Errno::PERM => invalid("closure directory is not owner-only"),
        rustix::io::Errno::INVAL => invalid("closure node path is empty"),
        _ => invalid("closure layout directory creation failed"),
    })
}

/// Threads that walk and hash one closure tree, at most this many per validation.
const MAX_WALK_THREADS: usize = 8;

/// Up to this many queued directories retain open descriptors; additional directories close their descriptors and
/// reopen by path from the files directory when a thread takes them.
const MAX_QUEUED_DIRS: usize = 64;

/// The walk-order key of `name` inside the directory keyed `parent`.
///
/// A key joins path components with NUL, which sorts below every byte a name can hold, so byte order on keys
/// is the order a sequential depth-first walk over sorted names visits entries in, and a directory sorts before
/// everything inside it.
fn walk_key(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}\0{name}")
    }
}

/// The failure earliest in walk order, so a concurrent walk reports the failure a sequential walk reaches first.
#[derive(Default)]
struct FirstFailure(Mutex<Option<(String, HarnessClosureError)>>);

impl FirstFailure {
    fn record(&self, key: String, error: HarnessClosureError) {
        let mut first = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if first.as_ref().is_none_or(|(at, _)| key < *at) {
            *first = Some((key, error));
        }
    }

    /// Whether a failure earlier than `key` already decides the validation.
    fn precedes(&self, key: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|(at, _)| at.as_str() < key)
    }

    fn into_inner(self) -> Option<HarnessClosureError> {
        self.0
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner)
            .map(|(_, error)| error)
    }
}

/// A queued directory whose descriptor was closed keeps only its place, so reopening it by path must reach `identity`.
struct DirPlace {
    key: String,
    relative: String,
    /// Path components in `relative`; the files directory itself has none.
    depth: usize,
    identity: (u64, u64),
}

/// A directory the walk admitted by membership, owner, and mode; its entries await inspection.
struct DirJob {
    place: DirPlace,
    fd: OwnedFd,
}

#[derive(Default)]
struct WalkQueue {
    open: Vec<DirJob>,
    closed: Vec<DirPlace>,
    /// Number of directories being inspected by threads.
    active: usize,
}

/// Verifies every entry under `files_fd` against `expected` and returns how many listed files it verified, plus the
/// verified descriptor of each file `slots` names, at that slot.
///
/// Up to [`MAX_WALK_THREADS`] threads take directories from a shared queue, check each entry's shape, owner, mode,
/// link count, size, and membership, and hash each admitted file. Every check carries its entry's [`walk_key`], and
/// validation reports the failure with the smallest key, which is the failure a sequential sorted walk returns.
fn validate_files(
    files_fd: &OwnedFd,
    expected: &BTreeMap<&str, &ClosureNode>,
    slots: &BTreeMap<&str, usize>,
    slot_count: usize,
) -> Result<(usize, Vec<Option<OwnedFd>>), HarnessClosureError> {
    let root = rustix::io::fcntl_dupfd_cloexec(files_fd, 0)
        .map_err(|_| invalid("closure files directory open failed"))?;
    let root_stat =
        rustix::fs::fstat(&root).map_err(|_| invalid("closure directory stat failed"))?;
    let walk = TreeWalk {
        files_fd,
        expected,
        slots,
        uid: owner_uid(),
        found: AtomicUsize::new(0),
        failure: FirstFailure::default(),
        retained: Mutex::new((0..slot_count).map(|_| None).collect()),
        queue: Mutex::new(WalkQueue {
            open: vec![DirJob {
                place: DirPlace {
                    key: String::new(),
                    relative: String::new(),
                    depth: 0,
                    identity: stat_identity(&root_stat),
                },
                fd: root,
            }],
            closed: Vec::new(),
            active: 0,
        }),
        ready: Condvar::new(),
    };
    let threads = std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .min(MAX_WALK_THREADS);
    std::thread::scope(|scope| {
        for _ in 1..threads {
            // A thread that cannot start leaves its share to the threads that did.
            let _ = std::thread::Builder::new()
                .name("closure-walk".to_owned())
                .spawn_scoped(scope, || walk.run());
        }
        walk.run();
    });
    let TreeWalk {
        found,
        failure,
        retained,
        ..
    } = walk;
    match failure.into_inner() {
        Some(error) => Err(error),
        None => Ok((
            found.into_inner(),
            retained
                .into_inner()
                .unwrap_or_else(PoisonError::into_inner),
        )),
    }
}

/// The state every walk thread of one [`validate_files`] call shares.
struct TreeWalk<'a> {
    files_fd: &'a OwnedFd,
    expected: &'a BTreeMap<&'a str, &'a ClosureNode>,
    slots: &'a BTreeMap<&'a str, usize>,
    uid: u32,
    found: AtomicUsize,
    failure: FirstFailure,
    retained: Mutex<Vec<Option<OwnedFd>>>,
    queue: Mutex<WalkQueue>,
    ready: Condvar,
}

impl TreeWalk<'_> {
    /// Takes queued directories until none remain and no thread can queue more.
    fn run(&self) {
        let mut buffer = vec![0u8; HASH_BUFFER_BYTES];
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            let open = queue.open.pop();
            let closed = if open.is_none() {
                queue.closed.pop()
            } else {
                None
            };
            if open.is_none() && closed.is_none() {
                if queue.active == 0 {
                    return;
                }
                queue = self
                    .ready
                    .wait(queue)
                    .unwrap_or_else(PoisonError::into_inner);
                continue;
            }
            queue.active += 1;
            drop(queue);
            if let Some(dir) = open.or_else(|| closed.and_then(|place| self.reopen(place))) {
                self.visit(dir, &mut buffer);
            }
            queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
            queue.active -= 1;
            if queue.active == 0 && queue.open.is_empty() && queue.closed.is_empty() {
                self.ready.notify_all();
            }
        }
    }

    fn spill(&self, dir: DirJob) {
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        let closed_fd = if queue.open.len() < MAX_QUEUED_DIRS {
            queue.open.push(dir);
            None
        } else {
            queue.closed.push(dir.place);
            Some(dir.fd)
        };
        drop(queue);
        self.ready.notify_one();
        drop(closed_fd);
    }

    fn reopen(&self, place: DirPlace) -> Option<DirJob> {
        if self.failure.precedes(&place.key) {
            return None;
        }
        let reopened = open_rel_nofollow(self.files_fd, &place.relative, true)
            .map_err(|_| invalid("closure tree entry open failed"))
            .and_then(|fd| {
                let stat = rustix::fs::fstat(&fd)
                    .map_err(|_| invalid("closure tree entry stat failed"))?;
                if stat_identity(&stat) != place.identity {
                    return Err(invalid("closure directory changed during validation"));
                }
                owned_directory_stat(&stat, self.uid)?;
                Ok(fd)
            });
        match reopened {
            Ok(fd) => Some(DirJob { place, fd }),
            Err(error) => {
                self.failure.record(place.key, error);
                None
            }
        }
    }

    /// Each directory closes before the thread lists its first subdirectory, so a thread holds at most the directory
    /// it lists, the first subdirectory it will descend into, and the entry it inspects, whatever the tree's depth.
    fn visit(&self, mut dir: DirJob, buffer: &mut [u8]) {
        while let Some(subdir) = self.visit_entries(dir, buffer) {
            dir = subdir;
        }
    }

    /// Inspects every entry of `dir`, queues each subdirectory but the first, and returns that first subdirectory.
    fn visit_entries(&self, dir: DirJob, buffer: &mut [u8]) -> Option<DirJob> {
        let names = match list_names(&dir.fd) {
            Ok(names) => names,
            Err(error) => {
                // A listing failure follows the directory's own entry and precedes everything inside it.
                self.failure.record(format!("{}\0", dir.place.key), error);
                return None;
            }
        };
        // This thread walks the directory's first subdirectory itself once the directory is done and queues the
        // others, so the subtree that sorts first, and any large file in it, starts at once.
        let mut first_subdir = None;
        for name in names {
            let key = walk_key(&dir.place.key, &name);
            // Names arrive sorted, so once an earlier failure precedes this entry it precedes the rest of the directory.
            if self.failure.precedes(&key) {
                break;
            }
            let relative = if dir.place.relative.is_empty() {
                name.clone()
            } else {
                format!("{}/{name}", dir.place.relative)
            };
            match self.inspect(&dir, &name, key.clone(), relative, buffer) {
                Ok(None) => {}
                Ok(Some(subdir)) if first_subdir.is_none() => first_subdir = Some(subdir),
                Ok(Some(subdir)) => self.spill(subdir),
                Err(error) => {
                    self.failure.record(key, error);
                    break;
                }
            }
        }
        first_subdir
    }

    fn inspect(
        &self,
        dir: &DirJob,
        name: &str,
        key: String,
        relative: String,
        buffer: &mut [u8],
    ) -> Result<Option<DirJob>, HarnessClosureError> {
        let fd = openat(
            &dir.fd,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(|_| invalid("closure tree entry open failed"))?;
        let stat = rustix::fs::fstat(&fd).map_err(|_| invalid("closure tree entry stat failed"))?;
        match mode_bits(&stat) & S_IFMT {
            S_IFDIR => {
                let depth = dir.place.depth + 1;
                // A valid manifest path has at most `MAX_PATH_COMPONENTS` components, so no listed directory reaches
                // that depth.
                if depth >= MAX_PATH_COMPONENTS {
                    return Err(invalid("closure contains an unlisted directory"));
                }
                let prefix = format!("{relative}/");
                // Keys sharing `prefix` are contiguous in the sorted map, so the first key
                // at or after `prefix` decides membership in O(log n) rather than a full scan.
                let listed = self
                    .expected
                    .range(prefix.as_str()..)
                    .next()
                    .is_some_and(|(path, _)| path.starts_with(prefix.as_str()));
                if !listed {
                    return Err(invalid("closure contains an unlisted directory"));
                }
                owned_directory_stat(&stat, self.uid)?;
                Ok(Some(DirJob {
                    place: DirPlace {
                        key,
                        relative,
                        depth,
                        identity: stat_identity(&stat),
                    },
                    fd,
                }))
            }
            S_IFREG => {
                let node = self
                    .expected
                    .get(relative.as_str())
                    .ok_or_else(|| invalid("closure contains an unlisted file"))?;
                node_file_stat(&stat, node, self.uid)?;
                verify_node_bytes(&fd, node, buffer)?;
                self.found.fetch_add(1, AtomicOrdering::Relaxed);
                if let Some(&slot) = self.slots.get(relative.as_str()) {
                    self.retained.lock().unwrap_or_else(PoisonError::into_inner)[slot] = Some(fd);
                }
                Ok(None)
            }
            _ => Err(invalid("closure contains a non-regular entry")),
        }
    }
}

fn verify_node_file(fd: &OwnedFd, node: &ClosureNode) -> Result<(), HarnessClosureError> {
    let stat = rustix::fs::fstat(fd).map_err(|_| invalid("closure file stat failed"))?;
    node_file_stat(&stat, node, owner_uid())?;
    let mut buffer = vec![0u8; HASH_BUFFER_BYTES];
    verify_node_bytes(fd, node, &mut buffer)
}

/// The metadata half of node verification: an owner-only single-link regular file with the manifest's mode and size.
fn node_file_stat(
    stat: &rustix::fs::Stat,
    node: &ClosureNode,
    uid: u32,
) -> Result<(), HarnessClosureError> {
    secure_file_stat(stat, node.mode, uid)?;
    if stat.st_size as u64 != node.size_bytes {
        return Err(invalid("closure node size diverges from manifest"));
    }
    Ok(())
}

/// A node at least this large hashes through [`hash_pipelined`]; a Pi closure's Node runtime is about 120 MiB, and its
/// hash is the longest single task in a validation.
const PIPELINED_HASH_MIN_BYTES: u64 = 16 * 1024 * 1024;

/// The content half of node verification: the descriptor's bytes from its current offset match the manifest size and hash.
fn verify_node_bytes(
    fd: &OwnedFd,
    node: &ClosureNode,
    buffer: &mut [u8],
) -> Result<(), HarnessClosureError> {
    let hashed = if node.size_bytes >= PIPELINED_HASH_MIN_BYTES {
        hash_pipelined(fd, node.size_bytes)
    } else {
        hash_copy_with(fd, None, node.size_bytes, Sha256::new(), 0, buffer)
    };
    let (total, sha256) = hashed.map_err(|error| {
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
    secure_file_stat(&stat, expected_mode, owner_uid())
}

fn secure_file_stat(
    stat: &rustix::fs::Stat,
    expected_mode: u32,
    uid: u32,
) -> Result<(), HarnessClosureError> {
    let mode = mode_bits(stat);
    if mode & S_IFMT != S_IFREG
        || stat.st_uid != uid
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
    owned_directory_stat(&stat, owner_uid())
}

fn owned_directory_stat(stat: &rustix::fs::Stat, uid: u32) -> Result<(), HarnessClosureError> {
    let mode = mode_bits(stat);
    if mode & S_IFMT != S_IFDIR || stat.st_uid != uid || mode & 0o7777 != 0o700 {
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

fn is_stale_temp(parent: &OwnedFd, name: &str) -> bool {
    rustix::fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)
        .is_ok_and(|stat| is_stale_mtime(stat.st_mtime))
}
