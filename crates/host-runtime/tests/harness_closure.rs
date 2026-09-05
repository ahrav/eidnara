use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use host_runtime::harness_closure::{
    ClosureCandidate, ClosureDependency, ClosureManifest, ClosureNode, DependencyKind,
    HarnessClosureStore, NodeKind, STALE_TEMP_AFTER, manifest_digest, validate_manifest,
};
use sha2::{Digest, Sha256};

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn node(
    path: &str,
    source_path: &str,
    kind: NodeKind,
    bytes: &[u8],
    dependencies: Vec<ClosureDependency>,
) -> ClosureNode {
    ClosureNode {
        path: path.to_owned(),
        source_root: "install".to_owned(),
        source_path: source_path.to_owned(),
        kind,
        mode: if matches!(kind, NodeKind::Executable | NodeKind::Interpreter) {
            0o700
        } else {
            0o600
        },
        size_bytes: bytes.len() as u64,
        sha256: sha256(bytes),
        dependencies,
    }
}

fn dependency(path: &str, kind: DependencyKind) -> ClosureDependency {
    ClosureDependency {
        path: path.to_owned(),
        kind,
    }
}

fn fixture(source: &Path) -> ClosureCandidate {
    let files = [
        ("bin/node", b"node-runtime".as_slice()),
        (
            "node_modules/pi/dist/cli.js",
            b"import './helper.js'; import './addon.node'".as_slice(),
        ),
        (
            "node_modules/pi/dist/helper.js",
            b"export const answer = 42".as_slice(),
        ),
        (
            "node_modules/pi/dist/addon.node",
            b"native-addon".as_slice(),
        ),
        (
            "node_modules/provider/a.js",
            b"export const provider = 'a'".as_slice(),
        ),
        (
            "node_modules/provider/b.js",
            b"export const provider = 'b'".as_slice(),
        ),
    ];
    for (path, bytes) in files {
        let destination = source.join(path);
        std::fs::create_dir_all(destination.parent().expect("parent")).expect("create parent");
        std::fs::write(&destination, bytes).expect("write source");
    }
    let nodes = vec![
        node(
            "bin/node",
            "bin/node",
            NodeKind::Interpreter,
            b"node-runtime",
            vec![],
        ),
        node(
            "node_modules/pi/dist/addon.node",
            "node_modules/pi/dist/addon.node",
            NodeKind::NativeAddon,
            b"native-addon",
            vec![],
        ),
        node(
            "node_modules/pi/dist/cli.js",
            "node_modules/pi/dist/cli.js",
            NodeKind::Module,
            b"import './helper.js'; import './addon.node'",
            vec![
                dependency("node_modules/pi/dist/addon.node", DependencyKind::Native),
                dependency("node_modules/pi/dist/helper.js", DependencyKind::Static),
            ],
        ),
        node(
            "node_modules/pi/dist/helper.js",
            "node_modules/pi/dist/helper.js",
            NodeKind::Module,
            b"export const answer = 42",
            vec![],
        ),
        node(
            "node_modules/provider/a.js",
            "node_modules/provider/a.js",
            NodeKind::Extension,
            b"export const provider = 'a'",
            vec![],
        ),
        node(
            "node_modules/provider/b.js",
            "node_modules/provider/b.js",
            NodeKind::Extension,
            b"export const provider = 'b'",
            vec![],
        ),
    ];
    ClosureCandidate {
        manifest: ClosureManifest {
            schema: "eidnara.host-harness-closure/v1".to_owned(),
            harness: "pi".to_owned(),
            package: "@earendil-works/pi-coding-agent".to_owned(),
            version: "0.80.2".to_owned(),
            argument_variant: "run_prompt".to_owned(),
            source_roots: vec!["install".to_owned()],
            executable: None,
            interpreter: Some("bin/node".to_owned()),
            entrypoint: Some("node_modules/pi/dist/cli.js".to_owned()),
            extensions: vec![
                "node_modules/provider/a.js".to_owned(),
                "node_modules/provider/b.js".to_owned(),
            ],
            nodes,
        },
        source_roots: BTreeMap::from([("install".to_owned(), source.to_path_buf())]),
    }
}

fn setup() -> (tempfile::TempDir, PathBuf, ClosureCandidate) {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("source");
    std::fs::create_dir(&source).expect("source");
    let candidate = fixture(&source);
    (temp, source, candidate)
}

#[test]
fn resolved_descriptor_is_rewound_after_verification() {
    let (temp, _source, candidate) = setup();
    let store = HarnessClosureStore::open(&temp.path().join("closures")).expect("store");
    let closure = store
        .materialize(&candidate, &BTreeSet::new())
        .expect("materialize");

    let node = closure
        .resolve_node_descriptor("node_modules/pi/dist/helper.js")
        .expect("resolve node");

    // A macOS child opening `/dev/fd/N` receives a duplicate descriptor with the original offset, so the handed-out descriptor must start at offset 0.
    // SAFETY: `node` owns this descriptor for the duration of the borrow.
    let inherited = unsafe { std::os::fd::BorrowedFd::borrow_raw(node.inherited_fd()) };
    let offset = rustix::fs::seek(inherited, rustix::fs::SeekFrom::Current(0))
        .expect("query descriptor offset");
    assert_eq!(
        offset, 0,
        "a handed-out node descriptor must be positioned at the start of the file"
    );
    // `pread` reads at an explicit offset without advancing the open file description's shared
    // offset; a `dup` + `read` would advance `node.inherited_fd()` itself.
    let mut bytes = vec![0u8; 64];
    let count = rustix::io::pread(inherited, &mut bytes, 0).expect("pread inherited descriptor");
    assert_eq!(&bytes[..count], b"export const answer = 42");
    let offset_after = rustix::fs::seek(inherited, rustix::fs::SeekFrom::Current(0))
        .expect("query descriptor offset after read");
    assert_eq!(
        offset_after, 0,
        "reading the bytes must not move the shared offset"
    );
}

#[test]
fn materialization_preserves_layout_and_security() {
    let (temp, _source, candidate) = setup();
    let store_root = temp.path().join("closures");
    let store = HarnessClosureStore::open(&store_root).expect("store");
    let closure = store
        .materialize(&candidate, &BTreeSet::new())
        .expect("materialize");

    let entrypoint = closure
        .resolve_node_descriptor("node_modules/pi/dist/cli.js")
        .expect("entrypoint");
    assert_eq!(
        std::fs::read(entrypoint.closure_path()).expect("read copied entrypoint"),
        b"import './helper.js'; import './addon.node'"
    );
    assert_eq!(closure.manifest().extensions, candidate.manifest.extensions);
    for node in &candidate.manifest.nodes {
        let path = closure.path().join("files").join(&node.path);
        let metadata = std::fs::symlink_metadata(path).expect("copied node metadata");
        assert_eq!(metadata.permissions().mode() & 0o777, node.mode);
        assert_eq!(metadata.nlink(), 1);
    }
}

#[test]
fn retained_closure_survives_source_deletion_and_deduplicates_by_digest() {
    let (temp, source, candidate) = setup();
    let store_root = temp.path().join("closures");
    let store = HarnessClosureStore::open(&store_root).expect("store");
    let first = store
        .materialize(&candidate, &BTreeSet::new())
        .expect("first materialize");
    let digest = first.digest().to_owned();
    std::fs::remove_dir_all(source).expect("delete source");

    let second = store
        .materialize(&candidate, &BTreeSet::new())
        .expect("dedupe does not reopen deleted source");
    assert_eq!(second.digest(), digest);
    assert_eq!(
        std::fs::read(
            second
                .resolve_node_descriptor("node_modules/pi/dist/helper.js")
                .expect("resolve retained node")
                .closure_path()
        )
        .expect("read retained node"),
        b"export const answer = 42"
    );
    let digest_directories = std::fs::read_dir(&store_root)
        .expect("read store")
        .filter_map(Result::ok)
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with(".tmp-"))
        .count();
    assert_eq!(digest_directories, 1);

    let descriptor_path = second
        .resolve_node_descriptor("node_modules/pi/dist/helper.js")
        .expect("descriptor-rooted retained node");
    let retained = store_root.join(&digest);
    let moved = store_root.join("moved-retained");
    std::fs::rename(&retained, &moved).expect("rename retained closure");
    let replacement = retained.join("files/node_modules/pi/dist");
    std::fs::create_dir_all(&replacement).expect("replacement tree");
    std::fs::write(
        replacement.join("helper.js"),
        b"export const answer = 'malicious'",
    )
    .expect("replacement bytes");
    assert_eq!(
        std::fs::read(descriptor_path.path()).expect("read descriptor-rooted node"),
        b"export const answer = 42",
        "path replacement must not change the retained closure object"
    );
    assert_eq!(
        second
            .resolve_node_descriptor("node_modules/pi/dist/helper.js")
            .err()
            .map(|error| error.detail()),
        Some("closure pathname no longer names the validated node"),
        "a resolution after the swap must not hand out a pathname into the replacement"
    );
}

#[test]
fn retained_executable_loads_dependency_and_extension_after_source_deletion() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("source");
    std::fs::create_dir_all(source.join("bin")).expect("bin");
    std::fs::create_dir_all(source.join("node_modules/pkg")).expect("package");
    let script = b"#!/bin/sh\nroot=$(CDPATH= cd -- \"$(dirname -- \"$0\")/..\" && pwd)\nprintf '%s' \"$(cat \"$root/node_modules/pkg/dep\")$(cat \"$root/node_modules/pkg/ext\")\"\n";
    std::fs::write(source.join("bin/run"), script).expect("script");
    std::fs::write(source.join("node_modules/pkg/dep"), b"dependency").expect("dependency");
    std::fs::write(source.join("node_modules/pkg/ext"), b"extension").expect("extension");
    let manifest = ClosureManifest {
        schema: "eidnara.host-harness-closure/v1".to_owned(),
        harness: "execution-test".to_owned(),
        package: "execution-test".to_owned(),
        version: "1.0.0".to_owned(),
        argument_variant: "run_prompt".to_owned(),
        source_roots: vec!["install".to_owned()],
        executable: Some("bin/run".to_owned()),
        interpreter: None,
        entrypoint: None,
        extensions: vec!["node_modules/pkg/ext".to_owned()],
        nodes: vec![
            node(
                "bin/run",
                "bin/run",
                NodeKind::Executable,
                script,
                vec![dependency("node_modules/pkg/dep", DependencyKind::Static)],
            ),
            node(
                "node_modules/pkg/dep",
                "node_modules/pkg/dep",
                NodeKind::Data,
                b"dependency",
                vec![],
            ),
            node(
                "node_modules/pkg/ext",
                "node_modules/pkg/ext",
                NodeKind::Extension,
                b"extension",
                vec![],
            ),
        ],
    };
    let store = HarnessClosureStore::open(&temp.path().join("closures")).expect("store");
    let closure = store
        .materialize(
            &ClosureCandidate {
                manifest,
                source_roots: BTreeMap::from([("install".to_owned(), source.clone())]),
            },
            &BTreeSet::new(),
        )
        .expect("materialize");
    std::fs::remove_dir_all(source).expect("delete source");

    let output = std::process::Command::new(
        closure
            .resolve_node_descriptor("bin/run")
            .expect("retained executable")
            .closure_path(),
    )
    .env_clear()
    .output()
    .expect("execute retained closure");
    assert!(output.status.success());
    assert_eq!(output.stdout, b"dependencyextension");
}

#[test]
fn source_and_retained_hash_mismatches_fail_closed() {
    let (temp, _source, candidate) = setup();
    let store = HarnessClosureStore::open(&temp.path().join("closures")).expect("store");
    let bad_source = candidate.clone();
    std::fs::write(
        bad_source.source_roots["install"].join("node_modules/pi/dist/helper.js"),
        b"export const answer = 41",
    )
    .expect("mutate source");
    assert_eq!(
        store
            .materialize(&bad_source, &BTreeSet::new())
            .expect_err("source hash mismatch")
            .detail(),
        "source node bytes diverge from manifest"
    );

    std::fs::write(
        bad_source.source_roots["install"].join("node_modules/pi/dist/helper.js"),
        b"export const answer = 42",
    )
    .expect("restore source");
    let closure = store
        .materialize(&candidate, &BTreeSet::new())
        .expect("materialize");
    let retained = closure.path().join("files/node_modules/pi/dist/helper.js");
    std::fs::write(&retained, b"export const answer = 41").expect("mutate retained");
    assert_eq!(
        store
            .validate(closure.digest())
            .expect_err("retained hash mismatch")
            .detail(),
        "closure node hash diverges from manifest"
    );
    // The overwrite kept the length, mode, and link count, so only a rehash on the handed-out
    // descriptor can catch it after the closure was already validated.
    assert_eq!(
        closure
            .resolve_node_descriptor("node_modules/pi/dist/helper.js")
            .err()
            .map(|error| error.detail()),
        Some("closure node hash diverges from manifest"),
        "resolution must rehash the retained node"
    );
}

#[test]
fn traversal_and_symlink_sources_are_rejected() {
    let (temp, source, candidate) = setup();
    let mut traversal = candidate.clone();
    traversal.manifest.nodes[0].source_path = "../node".to_owned();
    assert_eq!(
        validate_manifest(&traversal.manifest)
            .expect_err("traversal")
            .detail(),
        "manifest path has an invalid component"
    );

    let real = source.join("real-node");
    std::fs::write(&real, b"node-runtime").expect("real source");
    std::fs::remove_file(source.join("bin/node")).expect("remove original");
    std::os::unix::fs::symlink(&real, source.join("bin/node")).expect("symlink source");
    let store = HarnessClosureStore::open(&temp.path().join("closures")).expect("store");
    assert_eq!(
        store
            .materialize(&candidate, &BTreeSet::new())
            .expect_err("symlink refused")
            .detail(),
        "source node is missing or insecure"
    );
}

#[test]
fn missing_dependency_and_unreachable_nodes_are_rejected() {
    let (_temp, _source, candidate) = setup();
    let mut missing = candidate.manifest.clone();
    missing.nodes[2].dependencies[1].path = "node_modules/pi/dist/missing.js".to_owned();
    assert_eq!(
        validate_manifest(&missing)
            .expect_err("missing dependency")
            .detail(),
        "manifest references a missing node"
    );

    let mut unreachable = candidate.manifest.clone();
    unreachable.nodes[2].dependencies.pop();
    assert_eq!(
        validate_manifest(&unreachable)
            .expect_err("unreachable helper")
            .detail(),
        "manifest contains an unreachable node"
    );
}

#[test]
fn ordered_extensions_are_part_of_manifest_identity() {
    let (_temp, _source, candidate) = setup();
    let first = manifest_digest(&candidate.manifest).expect("first digest");
    let mut reordered = candidate.manifest.clone();
    reordered.extensions.reverse();
    let second = manifest_digest(&reordered).expect("second digest");
    assert_ne!(first, second);
}

/// Every declared root is held open while staging, so the count is bounded to keep an
/// otherwise valid manifest from failing on `RLIMIT_NOFILE`.
#[test]
fn a_manifest_declaring_too_many_source_roots_is_rejected() {
    let (_temp, _source, candidate) = setup();
    let mut crowded = candidate.manifest.clone();
    crowded.source_roots = (0..65).map(|index| format!("root-{index:03}")).collect();
    crowded.source_roots.sort();
    assert_eq!(
        validate_manifest(&crowded)
            .expect_err("65 roots exceed the bound")
            .detail(),
        "manifest declares too many source roots"
    );
}

#[test]
fn strict_manifest_decode_rejects_unknown_fields() {
    let (_temp, _source, candidate) = setup();
    let mut value = serde_json::to_value(&candidate.manifest).expect("value");
    value
        .as_object_mut()
        .expect("object")
        .insert("ambient_path".to_owned(), serde_json::Value::Bool(true));
    assert!(serde_json::from_value::<ClosureManifest>(value).is_err());
}

#[test]
fn canonical_manifest_digest_is_pinned() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/harness-closures/pi-valid.json");
    let manifest: ClosureManifest =
        serde_json::from_slice(&std::fs::read(fixture).expect("read closure fixture"))
            .expect("decode closure fixture");
    assert_eq!(
        manifest_digest(&manifest).expect("digest"),
        "5386c2004cc31abbdd98e766be193f78e1a74937254681e6db47bd700961f911"
    );
}

#[test]
#[ignore = "requires U9 external closure roots; run explicitly in release qualification"]
fn production_closures_from_environment_materialize() {
    let opencode_root =
        std::env::var_os("EIDNARA_OPENCODE_CLOSURE_RUNTIME_ROOT").expect("OpenCode closure root");
    let pi_install = PathBuf::from(
        std::env::var_os("EIDNARA_PI_CLOSURE_INSTALL_ROOT")
            .expect("EIDNARA_PI_CLOSURE_INSTALL_ROOT accompanies OpenCode root"),
    );
    let pi_runtime = PathBuf::from(
        std::env::var_os("EIDNARA_PI_CLOSURE_RUNTIME_ROOT")
            .expect("EIDNARA_PI_CLOSURE_RUNTIME_ROOT accompanies OpenCode root"),
    );
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let read_manifest = |name: &str| -> ClosureManifest {
        serde_json::from_slice(
            &std::fs::read(repo.join("release/harness-closures").join(name))
                .expect("read production closure manifest"),
        )
        .expect("decode production closure manifest")
    };
    let store_root = tempfile::tempdir().expect("closure store");
    let store =
        HarnessClosureStore::open(&store_root.path().join("closures")).expect("open closure store");

    let opencode = store
        .materialize(
            &ClosureCandidate {
                manifest: read_manifest("opencode-linux-x64-1.18.22.json"),
                source_roots: BTreeMap::from([(
                    "runtime".to_owned(),
                    PathBuf::from(opencode_root),
                )]),
            },
            &BTreeSet::new(),
        )
        .expect("materialize OpenCode closure");
    assert!(
        opencode
            .resolve_node_descriptor("bin/opencode")
            .expect("OpenCode executable")
            .closure_path()
            .is_file()
    );

    let pi = store
        .materialize(
            &ClosureCandidate {
                manifest: read_manifest("pi-linux-x64-node-24.18.0.json"),
                source_roots: BTreeMap::from([
                    ("pi-install".to_owned(), pi_install),
                    ("runtime".to_owned(), pi_runtime),
                ]),
            },
            &BTreeSet::new(),
        )
        .expect("materialize Pi closure");
    assert!(
        pi.resolve_node_descriptor("node_modules/@earendil-works/pi-coding-agent/dist/cli.js")
            .expect("Pi entrypoint")
            .closure_path()
            .is_file()
    );
    assert_eq!(pi.manifest().nodes.len(), 3_081);
}

#[test]
fn retained_closure_rejects_extra_missing_and_wrong_mode_nodes() {
    let (extra_temp, _source, extra_candidate) = setup();
    let extra_store =
        HarnessClosureStore::open(&extra_temp.path().join("closures")).expect("store");
    let extra = extra_store
        .materialize(&extra_candidate, &BTreeSet::new())
        .expect("materialize");
    std::fs::write(extra.path().join("files/unlisted"), b"extra").expect("extra file");
    assert_eq!(
        extra_store
            .validate(extra.digest())
            .expect_err("unlisted file must fail")
            .detail(),
        "closure contains an unlisted file"
    );

    let (missing_temp, _source, missing_candidate) = setup();
    let missing_store =
        HarnessClosureStore::open(&missing_temp.path().join("closures")).expect("store");
    let missing = missing_store
        .materialize(&missing_candidate, &BTreeSet::new())
        .expect("materialize");
    std::fs::remove_file(missing.path().join("files/node_modules/pi/dist/helper.js"))
        .expect("remove retained node");
    assert_eq!(
        missing_store
            .validate(missing.digest())
            .expect_err("missing node must fail")
            .detail(),
        "closure is missing a manifest-listed node"
    );

    let (mode_temp, _source, mode_candidate) = setup();
    let mode_store = HarnessClosureStore::open(&mode_temp.path().join("closures")).expect("store");
    let mode = mode_store
        .materialize(&mode_candidate, &BTreeSet::new())
        .expect("materialize");
    let helper = mode.path().join("files/node_modules/pi/dist/helper.js");
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700))
        .expect("change retained mode");
    assert_eq!(
        mode_store
            .validate(mode.digest())
            .expect_err("wrong mode must fail")
            .detail(),
        "closure file is not owner-only single-link"
    );
}

/// The helper sets the entry's mtime to twice `STALE_TEMP_AFTER` ago so `prune` treats it as abandoned.
fn age_past_stale_threshold(path: &Path) {
    let old = std::time::SystemTime::now() - STALE_TEMP_AFTER * 2;
    let secs = old
        .duration_since(std::time::UNIX_EPOCH)
        .expect("epoch")
        .as_secs();
    let stamp = rustix::fs::Timespec {
        tv_sec: secs as _,
        tv_nsec: 0,
    };
    rustix::fs::utimensat(
        rustix::fs::CWD,
        path,
        &rustix::fs::Timestamps {
            last_access: stamp,
            last_modification: stamp,
        },
        rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
    )
    .expect("age entry");
}

#[test]
fn prune_reclaims_unprotected_digests_and_stale_temps_only() {
    let (temp, _source, candidate) = setup();
    let store_root = temp.path().join("closures");
    let store = HarnessClosureStore::open(&store_root).expect("store");
    let closure = store
        .materialize(&candidate, &BTreeSet::new())
        .expect("materialize");
    let digest = closure.digest().to_owned();

    // A stale temp inherits the ambient umask; `prune` must not require mode `0o700`.
    let stale = store_root.join(".tmp-deadbeefdeadbeefdeadbeef");
    std::fs::create_dir(&stale).expect("stale temp");
    std::fs::write(stale.join("partial"), b"torn").expect("partial file");
    age_past_stale_threshold(&stale);
    let live = store_root.join(".tmp-0123456789abcdef01234567");
    std::fs::create_dir(&live).expect("live temp");
    std::fs::create_dir(store_root.join("foreign-entry")).expect("foreign entry");
    // Only the exact `.tmp-<24 hex>` shape is a store temp; a stale lookalike and a non-UTF-8
    // name are foreign entries that must neither be removed nor abort the sweep.
    let lookalike = store_root.join(".tmp-backup");
    std::fs::create_dir(&lookalike).expect("lookalike temp");
    age_past_stale_threshold(&lookalike);
    let unnamed = store_root.join(std::ffi::OsStr::from_bytes(b"\xff\xfe-not-utf8"));
    std::fs::create_dir(&unnamed).expect("non-utf8 entry");

    let protected = BTreeSet::from([digest.clone()]);
    store.prune(&protected).expect("prune with protection");
    assert!(
        store_root.join(&digest).is_dir(),
        "protected digest survives"
    );
    assert!(!stale.exists(), "stale staging directory is reclaimed");
    assert!(
        live.is_dir(),
        "a young staging directory may belong to an in-flight materialize and survives"
    );
    assert!(
        store_root.join("foreign-entry").is_dir(),
        "entries the store did not create are left untouched"
    );
    assert!(
        lookalike.is_dir(),
        "a stale .tmp- lookalike is not a store temp"
    );
    assert!(unnamed.is_dir(), "a non-UTF-8 name is preserved");
    store
        .validate(&digest)
        .expect("protected closure still validates");

    store
        .prune(&BTreeSet::new())
        .expect("prune without protection");
    assert!(
        !store_root.join(&digest).is_dir(),
        "unprotected digest is reclaimed"
    );
}

/// Writes through `root_fd` land in a detached tree after the store root is renamed and replaced, so `materialize` and `prune` must fail rather than report success.
#[test]
fn a_replaced_store_root_fails_materialize_and_prune() {
    let (temp, _source, candidate) = setup();
    let store_root = temp.path().join("closures");
    let store = HarnessClosureStore::open(&store_root).expect("store");
    store
        .materialize(&candidate, &BTreeSet::new())
        .expect("first materialize");

    let detached = temp.path().join("closures-detached");
    std::fs::rename(&store_root, &detached).expect("detach the store root");
    std::fs::create_dir(&store_root).expect("replacement root");
    std::fs::set_permissions(&store_root, std::fs::Permissions::from_mode(0o700)).expect("mode");

    let mut second = candidate.clone();
    second.manifest.version = "0.80.3".to_owned();
    assert_eq!(
        store
            .materialize(&second, &BTreeSet::new())
            .err()
            .map(|error| error.detail()),
        Some("closure store was replaced under the mutator")
    );
    assert_eq!(
        store
            .materialize(&candidate, &BTreeSet::new())
            .err()
            .map(|error| error.detail()),
        Some("closure store was replaced under the mutator"),
        "an already-valid occupant in the detached tree is not a success either"
    );
    assert_eq!(
        store
            .prune(&BTreeSet::new())
            .expect_err("prune must not report success into a detached tree")
            .detail(),
        "closure store was replaced under the mutator"
    );
    assert!(
        std::fs::read_dir(&store_root)
            .expect("read replacement root")
            .next()
            .is_none(),
        "nothing was written into the replacement root"
    );
}

/// Opening an existing owned store root repairs mode `0o755` to `0o700` instead of refusing the store.
#[test]
fn an_existing_owned_store_root_is_repaired_to_owner_only() {
    let (temp, _source, candidate) = setup();
    let store_root = temp.path().join("closures");
    std::fs::create_dir(&store_root).expect("pre-created store root");
    std::fs::set_permissions(&store_root, std::fs::Permissions::from_mode(0o755)).expect("mode");

    let store = HarnessClosureStore::open(&store_root).expect("open repairs the mode");
    assert_eq!(
        std::fs::metadata(&store_root)
            .expect("stat")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    store
        .materialize(&candidate, &BTreeSet::new())
        .expect("materialize");
}

/// One unremovable entry must not abort reclamation of the entries sorted after it.
#[test]
fn prune_continues_past_an_unremovable_entry() {
    let (temp, _source, candidate) = setup();
    let store_root = temp.path().join("closures");
    let store = HarnessClosureStore::open(&store_root).expect("store");
    let digest = store
        .materialize(&candidate, &BTreeSet::new())
        .expect("materialize")
        .digest()
        .to_owned();

    // The `.tmp-*` entry sorts before the digest. Mode `0o000` prevents listing its child, so removal fails; `prune` must still reclaim the later digest.
    let blocked = store_root.join(".tmp-000000000000000000000000");
    std::fs::create_dir(&blocked).expect("blocked temp");
    std::fs::write(blocked.join("child"), b"x").expect("child");
    age_past_stale_threshold(&blocked);
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).expect("mode");

    let result = store.prune(&BTreeSet::new());
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o700)).expect("restore");
    assert!(result.is_err(), "the blocked entry is reported");
    assert!(
        !store_root.join(&digest).is_dir(),
        "the digest sorted after the blocked entry is still reclaimed"
    );
}

/// A crash between `rename_no_replace` and the store fsync can leave a promoted digest
/// missing nested entries. Under `transaction.lock` nothing else owns it, so `materialize`
/// removes the torn occupant and restages instead of wedging the digest.
#[test]
fn a_torn_digest_directory_is_repaired_by_materialize() {
    let (temp, _source, candidate) = setup();
    let store_root = temp.path().join("closures");
    let store = HarnessClosureStore::open(&store_root).expect("store");
    let digest = manifest_digest(&candidate.manifest).expect("digest");

    let torn = store_root.join(&digest);
    std::fs::create_dir_all(torn.join("files/bin")).expect("torn layout without bin/node");
    std::fs::write(
        torn.join("manifest.json"),
        serde_json::to_vec_pretty(&serde_json::to_value(&candidate.manifest).expect("value"))
            .expect("manifest bytes"),
    )
    .expect("valid manifest");
    for path in [torn.clone(), torn.join("files"), torn.join("files/bin")] {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).expect("mode");
    }
    std::fs::set_permissions(
        torn.join("manifest.json"),
        std::fs::Permissions::from_mode(0o600),
    )
    .expect("mode");
    assert_eq!(
        store.validate(&digest).expect_err("torn").detail(),
        "closure is missing a manifest-listed node"
    );

    // A restage that fails must leave the torn occupant untouched rather than unlink it first.
    let mut unstageable = candidate.clone();
    unstageable.source_roots.insert(
        "install".to_owned(),
        temp.path().join("missing-source-root"),
    );
    assert_eq!(
        store
            .materialize(&unstageable, &BTreeSet::new())
            .expect_err("missing source root cannot stage")
            .detail(),
        "source root open failed"
    );
    assert!(
        torn.join("manifest.json").is_file(),
        "a failed restage must not remove the torn occupant"
    );

    // A protected digest names a tree a live harness may still open through; it is refused,
    // not swapped out from under that harness.
    assert_eq!(
        store
            .materialize(&candidate, &BTreeSet::from([digest.clone()]))
            .err()
            .map(|error| error.detail()),
        Some("corrupt digest target is protected")
    );
    assert!(
        torn.join("manifest.json").is_file(),
        "a refused repair leaves the occupant in place"
    );

    let closure = store
        .materialize(&candidate, &BTreeSet::new())
        .expect("materialize repairs the torn digest");
    assert_eq!(closure.digest(), digest);
    store.validate(&digest).expect("repaired closure validates");
    assert!(torn.join("files/bin/node").is_file());
    let leftover_temps = std::fs::read_dir(&store_root)
        .expect("read store")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(".tmp-"))
        .count();
    assert_eq!(leftover_temps, 0, "the swapped-out torn tree is removed");
}

/// The child must see the node at the exact descriptor number `module_path` names, and only
/// after `inherit_in_child` clears close-on-exec in the forked child.
#[cfg(target_os = "linux")]
#[test]
fn inherit_in_child_exposes_the_node_at_its_descriptor_path() {
    use std::os::unix::process::CommandExt;

    let (temp, _source, candidate) = setup();
    let store = HarnessClosureStore::open(&temp.path().join("closures")).expect("store");
    let closure = store
        .materialize(&candidate, &BTreeSet::new())
        .expect("materialize");
    let node = closure
        .resolve_node_descriptor("node_modules/pi/dist/helper.js")
        .expect("resolve node");
    let module_path = node.module_path().to_path_buf();
    assert_eq!(
        module_path,
        Path::new("/proc/self/fd").join(node.inherited_fd().to_string())
    );

    // Without `inherit_in_child`, close-on-exec closes the descriptor before the child can read it.
    let closed = std::process::Command::new("cat")
        .arg(&module_path)
        .env_clear()
        .output()
        .expect("run cat without inheritance");
    assert!(
        !closed.status.success(),
        "a close-on-exec descriptor must not be visible to the child"
    );

    let mut command = std::process::Command::new("cat");
    command.arg(&module_path).env_clear();
    // SAFETY: `inherit_in_child` performs one `fcntl` and no allocation or locking between fork and exec.
    unsafe {
        command.pre_exec(move || node.inherit_in_child());
    }
    let output = command.output().expect("run cat with inheritance");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"export const answer = 42");
}

/// A shebang script launched through `path()` makes the kernel pass `/proc/self/fd/N` to the
/// interpreter as its script argument; the interpreter reopens that name after exec, so the
/// descriptor must survive exec via `inherit_in_child`.
#[cfg(target_os = "linux")]
#[test]
fn a_shebang_executable_launches_through_its_descriptor_path() {
    use std::os::unix::process::CommandExt;

    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("source");
    std::fs::create_dir_all(source.join("bin")).expect("bin");
    let script = b"#!/bin/sh\nprintf '%s' launched\n";
    std::fs::write(source.join("bin/run"), script).expect("script");
    let manifest = ClosureManifest {
        schema: "eidnara.host-harness-closure/v1".to_owned(),
        harness: "sh".to_owned(),
        package: "script".to_owned(),
        version: "1".to_owned(),
        argument_variant: "run".to_owned(),
        source_roots: vec!["install".to_owned()],
        executable: Some("bin/run".to_owned()),
        interpreter: None,
        entrypoint: None,
        extensions: vec![],
        nodes: vec![node(
            "bin/run",
            "bin/run",
            NodeKind::Executable,
            script,
            vec![],
        )],
    };
    let store = HarnessClosureStore::open(&temp.path().join("closures")).expect("store");
    let closure = store
        .materialize(
            &ClosureCandidate {
                manifest,
                source_roots: BTreeMap::from([("install".to_owned(), source)]),
            },
            &BTreeSet::new(),
        )
        .expect("materialize");
    let node = closure
        .resolve_node_descriptor("bin/run")
        .expect("resolve executable");
    let exec_path = node.path().to_path_buf();

    let closed = std::process::Command::new(&exec_path)
        .env_clear()
        .output()
        .expect("launch without inheritance");
    assert!(
        !closed.status.success(),
        "the interpreter cannot reopen a descriptor path that exec closed"
    );

    let mut command = std::process::Command::new(&exec_path);
    command.env_clear();
    // SAFETY: `inherit_in_child` performs one `fcntl` and no allocation or locking between fork and exec.
    unsafe {
        command.pre_exec(move || node.inherit_in_child());
    }
    let output = command.output().expect("launch with inheritance");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"launched");
}

#[test]
fn native_edges_and_native_addons_must_correspond_exactly() {
    let (_temp, _source, candidate) = setup();

    // Validation rejects a non-`Native` dependency edge to a `NativeAddon`.
    let mut static_edge = candidate.clone();
    static_edge.manifest.nodes[2].dependencies = vec![
        dependency("node_modules/pi/dist/addon.node", DependencyKind::Static),
        dependency("node_modules/pi/dist/helper.js", DependencyKind::Static),
    ];
    assert_eq!(
        validate_manifest(&static_edge.manifest)
            .expect_err("static edge onto native addon")
            .detail(),
        "native dependency kind must correspond exactly to a native addon target"
    );

    // Native edges are checked before reachability, so the missing edge is reported first.
    let mut unclaimed = candidate.clone();
    unclaimed.manifest.nodes[2].dependencies = vec![dependency(
        "node_modules/pi/dist/helper.js",
        DependencyKind::Static,
    )];
    assert_eq!(
        validate_manifest(&unclaimed.manifest)
            .expect_err("native addon without an inbound native edge")
            .detail(),
        "native addon lacks an explicit native dependency edge"
    );
}

/// Lexicographic path order can place `bin/node.dat` between `bin/node` and `bin/node/main.js`.
/// Validation must compare every path with its ancestor paths, not only the immediately preceding sorted entry.
/// Validation rejects a regular file that prefixes another manifest path.
#[test]
fn a_parent_file_collision_is_caught_across_an_intervening_sibling() {
    let (_temp, _source, candidate) = setup();

    let mut adjacent = candidate.manifest.clone();
    adjacent.nodes.push(node(
        "bin/node/main.js",
        "bin/node/main.js",
        NodeKind::Module,
        b"nested under a file",
        vec![],
    ));
    adjacent.nodes.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(
        validate_manifest(&adjacent)
            .expect_err("a child of a regular file is not materializable")
            .detail(),
        "manifest node path collides with a parent file"
    );

    let mut separated = adjacent.clone();
    separated.nodes.push(node(
        "bin/node.dat",
        "bin/node.dat",
        NodeKind::Data,
        b"sorts between the parent and its child",
        vec![],
    ));
    separated.nodes.sort_by(|a, b| a.path.cmp(&b.path));
    let paths: Vec<&str> = separated
        .nodes
        .iter()
        .map(|entry| entry.path.as_str())
        .collect();
    assert_eq!(
        &paths[..3],
        &["bin/node", "bin/node.dat", "bin/node/main.js"],
        "the sibling must sort between the parent file and its child for this case to bite"
    );
    assert_eq!(
        validate_manifest(&separated)
            .expect_err("an intervening sibling must not hide the collision")
            .detail(),
        "manifest node path collides with a parent file"
    );
}

/// Validation rejects multiple dependencies that name the same target, regardless of `DependencyKind`.
/// Dependencies with the same path but different kinds require explicit duplicate-path validation.
#[test]
fn duplicate_dependency_targets_are_rejected_across_kinds() {
    let (_temp, _source, candidate) = setup();
    let mut duplicated = candidate.manifest.clone();
    duplicated.nodes[2].dependencies = vec![
        dependency("node_modules/pi/dist/addon.node", DependencyKind::Native),
        dependency("node_modules/pi/dist/helper.js", DependencyKind::Static),
        dependency("node_modules/pi/dist/helper.js", DependencyKind::Native),
    ];
    assert_eq!(
        validate_manifest(&duplicated)
            .expect_err("one target named twice")
            .detail(),
        "node dependencies are not uniquely sorted by target path"
    );
}
