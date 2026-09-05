// This crate root must declare `file_mode` because `harness_closure.rs` uses `crate::file_mode`.
#[path = "../src/file_mode.rs"]
mod file_mode;
#[path = "../src/harness_closure.rs"]
mod harness_closure;

use std::collections::BTreeMap;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use harness_closure::{
    ClosureCandidate, ClosureDependency, ClosureManifest, ClosureNode, DependencyKind,
    HarnessClosureStore, NodeKind, manifest_digest, validate_manifest,
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
    let closure = store.materialize(&candidate).expect("materialize");

    let node = closure
        .resolve_node_descriptor("node_modules/pi/dist/helper.js")
        .expect("resolve node");

    // A macOS child opening `/dev/fd/N` receives a duplicate descriptor with the original offset, so the handed-out descriptor must start at offset 0.
    // A macOS child opening `/dev/fd/N` receives a duplicate descriptor with the original offset, so the handed-out descriptor must start at offset 0.
    // SAFETY: `node` owns this descriptor for the duration of the borrow.
    let inherited = unsafe { std::os::fd::BorrowedFd::borrow_raw(node.inherited_fd()) };
    let offset = rustix::fs::seek(inherited, rustix::fs::SeekFrom::Current(0))
        .expect("query descriptor offset");
    assert_eq!(
        offset, 0,
        "a handed-out node descriptor must be positioned at the start of the file"
    );
    let mut bytes = Vec::new();
    std::fs::File::from(rustix::io::dup(inherited).expect("duplicate inherited descriptor"))
        .read_to_end(&mut bytes)
        .expect("read inherited descriptor");
    assert_eq!(bytes, b"export const answer = 42");
}

#[test]
fn materialization_preserves_layout_and_security() {
    let (temp, _source, candidate) = setup();
    let store_root = temp.path().join("closures");
    let store = HarnessClosureStore::open(&store_root).expect("store");
    let closure = store.materialize(&candidate).expect("materialize");

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
    let first = store.materialize(&candidate).expect("first materialize");
    let digest = first.digest().to_owned();
    std::fs::remove_dir_all(source).expect("delete source");

    let second = store
        .materialize(&candidate)
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
        .materialize(&ClosureCandidate {
            manifest,
            source_roots: BTreeMap::from([("install".to_owned(), source.clone())]),
        })
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
            .materialize(&bad_source)
            .expect_err("source hash mismatch")
            .detail(),
        "source node bytes diverge from manifest"
    );

    std::fs::write(
        bad_source.source_roots["install"].join("node_modules/pi/dist/helper.js"),
        b"export const answer = 42",
    )
    .expect("restore source");
    let closure = store.materialize(&candidate).expect("materialize");
    let retained = closure.path().join("files/node_modules/pi/dist/helper.js");
    std::fs::write(&retained, b"export const answer = 41").expect("mutate retained");
    assert_eq!(
        store
            .validate(closure.digest())
            .expect_err("retained hash mismatch")
            .detail(),
        "closure node hash diverges from manifest"
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
            .materialize(&candidate)
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

/// Emits `value` as JSON text with every object's keys in reverse order, so the text differs
/// from serde's key-sorted output while denoting the same manifest.
fn json_with_reversed_keys(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            keys.reverse();
            let fields: Vec<String> = keys
                .into_iter()
                .map(|key| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(key).expect("key"),
                        json_with_reversed_keys(&map[key])
                    )
                })
                .collect();
            format!("{{{}}}", fields.join(","))
        }
        serde_json::Value::Array(values) => {
            let items: Vec<String> = values.iter().map(json_with_reversed_keys).collect();
            format!("[{}]", items.join(","))
        }
        scalar => serde_json::to_string(scalar).expect("scalar"),
    }
}

#[test]
fn manifest_digest_is_stable_under_key_reordering() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/harness-closures/pi-valid.json");
    let text = std::fs::read_to_string(fixture).expect("read closure fixture");
    let manifest: ClosureManifest = serde_json::from_str(&text).expect("decode closure fixture");
    let value = serde_json::to_value(&manifest).expect("value");
    let reordered_text = json_with_reversed_keys(&value);
    assert_ne!(
        reordered_text,
        serde_json::to_string(&manifest).expect("sorted text"),
        "the reordered text must be a different byte sequence for the same manifest"
    );
    let reordered: ClosureManifest =
        serde_json::from_str(&reordered_text).expect("decode reordered manifest");
    assert_eq!(
        manifest_digest(&reordered).expect("digest"),
        manifest_digest(&manifest).expect("digest"),
        "key order in the input must not change the digest"
    );
}

/// Sorts every object's keys so the text matches the canonical form's key order.
fn json_with_sorted_keys(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            serde_json::Value::Object(
                keys.into_iter()
                    .map(|key| (key.clone(), json_with_sorted_keys(&map[key])))
                    .collect(),
            )
        }
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(json_with_sorted_keys).collect())
        }
        scalar => scalar.clone(),
    }
}

/// The digest is reproduced from the fixture's own JSON text, never from the crate's
/// `Serialize` impl, so a field the impl dropped (a node path, a dependency edge) would
/// leave the two digests different even though every in-crate mutation still moved it.
#[test]
fn manifest_digest_matches_an_external_canonicalization_of_the_fixture_text() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/harness-closures/pi-valid.json");
    let text = std::fs::read_to_string(fixture).expect("read closure fixture");
    let manifest: ClosureManifest = serde_json::from_str(&text).expect("decode closure fixture");
    let raw: serde_json::Value = serde_json::from_str(&text).expect("parse fixture text");
    let canonical = serde_json::to_vec_pretty(&json_with_sorted_keys(&raw)).expect("pretty");
    let external = format!("{:x}", Sha256::digest(&canonical));
    assert_eq!(manifest_digest(&manifest).expect("digest"), external);
    // Every node path and dependency edge is present in the canonical text as many times
    // as the fixture names it.
    let canonical_text = String::from_utf8(canonical).expect("utf8");
    // A multibyte identifier: the canonical form must carry its UTF-8 bytes rather than a
    // `\\u` escape, and the digest must follow the external canonicalization of the same
    // text edit. A serializer that started escaping non-ASCII would change the digest of
    // every such manifest while leaving the ASCII fixture untouched.
    let multibyte = "p\u{ef}";
    assert_eq!(multibyte.len(), 3);
    assert_eq!(multibyte.chars().count(), 2);
    let edited_text = text.replacen("\"harness\": \"pi\"", "\"harness\": \"p\u{ef}\"", 1);
    assert_ne!(
        edited_text, text,
        "the fixture must name the harness \"pi\" once"
    );
    let edited: ClosureManifest = serde_json::from_str(&edited_text).expect("decode edited");
    assert_eq!(edited.harness, multibyte);
    let edited_raw: serde_json::Value = serde_json::from_str(&edited_text).expect("parse");
    let edited_canonical =
        serde_json::to_vec_pretty(&json_with_sorted_keys(&edited_raw)).expect("pretty");
    assert!(
        edited_canonical
            .windows(multibyte.len())
            .any(|window| window == multibyte.as_bytes()),
        "the canonical form must carry the identifier's UTF-8 bytes"
    );
    assert!(!String::from_utf8_lossy(&edited_canonical).contains("\\u00ef"));
    assert_eq!(
        manifest_digest(&edited).expect("digest"),
        format!("{:x}", Sha256::digest(&edited_canonical))
    );
    for node in &manifest.nodes {
        let needle = format!("\"{}\"", node.path);
        let in_fixture = text.matches(needle.as_str()).count();
        assert!(in_fixture >= 1);
        assert_eq!(
            canonical_text.matches(needle.as_str()).count(),
            in_fixture,
            "node path {} occurs a different number of times in the canonical form",
            node.path
        );
    }
}

#[test]
fn manifest_digest_changes_when_any_field_changes() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/harness-closures/pi-valid.json");
    let baseline: ClosureManifest =
        serde_json::from_slice(&std::fs::read(fixture).expect("read closure fixture"))
            .expect("decode closure fixture");
    let before = manifest_digest(&baseline).expect("digest");
    // The schema is not a digest input but a gate: any other value is refused before
    // hashing, so no manifest with a different schema can reproduce this digest.
    let mut other_schema = baseline.clone();
    other_schema.schema.push('x');
    assert!(manifest_digest(&other_schema).is_err());
    // Each entry changes one field and nothing else, and keeps the manifest valid so the
    // digest is computed. A field the canonical form drops leaves the digest equal to
    // `before`.
    type Mutation = (&'static str, fn(&mut ClosureManifest));
    let mutations: [Mutation; 19] = [
        ("harness", |m| m.harness.push('x')),
        ("package", |m| m.package.push('x')),
        ("version", |m| m.version.push('x')),
        ("argument_variant", |m| m.argument_variant.push('x')),
        // Roots must stay uniquely sorted; a new last root that sorts after the others is
        // declared but unreferenced, which the validator allows.
        ("source_roots", |m| {
            m.source_roots.push("zz-extra-root".to_owned())
        }),
        // `interpreter` and `entrypoint` are changed alone in
        // `launch_roots_participate_in_the_digest_on_their_own`, which needs a second
        // eligible node to point at.
        ("nodes[0].path", |m| {
            m.nodes[0].path = "bin/node2".to_owned();
            m.interpreter = Some("bin/node2".to_owned());
        }),
        ("nodes[1].path", |m| {
            let path = "node_modules/@earendil-works/pi-coding-agent/dist/cli2.js".to_owned();
            m.nodes[1].path = path.clone();
            m.entrypoint = Some(path);
        }),
        ("extensions", |m| m.extensions.clear()),
        ("nodes[0].source_root", |m| {
            m.nodes[0].source_root = "pi-install".to_owned()
        }),
        ("nodes[0].source_path", |m| m.nodes[0].source_path.push('x')),
        ("nodes[0].sha256", |m| {
            m.nodes[0].sha256 = format!("{:0>64}", "1")
        }),
        ("nodes[0].size_bytes", |m| m.nodes[0].size_bytes += 1),
        // A module may become data without changing its mode or its edges.
        ("nodes[2].kind", |m| m.nodes[2].kind = NodeKind::Data),
        // The static edge to helper.js may become a finite dynamic edge.
        ("nodes[1].dependencies[0].kind", |m| {
            m.nodes[1].dependencies[0].kind = DependencyKind::FiniteDynamic;
        }),
        // Retargeting the finite dynamic edge from the extension to the interpreter keeps
        // its kind, keeps the extension reachable as a root, and re-sorts the edge list.
        ("nodes[1].dependencies[1].path", |m| {
            m.nodes[1].dependencies[1].path = "bin/node".to_owned();
            m.nodes[1].dependencies.sort_by(|a, b| a.path.cmp(&b.path));
        }),
        ("nodes[1].dependencies.len", |m| {
            m.nodes[1].dependencies.truncate(1)
        }),
        ("nodes[1].source_path", |m| m.nodes[1].source_path.push('x')),
        ("nodes[1].sha256", |m| {
            m.nodes[1].sha256 = format!("{:0>64}", "2")
        }),
        // Every node must be reachable, so the new data node hangs off the entrypoint.
        ("nodes.len", |m| {
            let mut extra = m.nodes[2].clone();
            extra.path = "zz/extra.data".to_owned();
            extra.kind = NodeKind::Data;
            extra.dependencies.clear();
            m.nodes.push(extra);
            m.nodes[1].dependencies.push(ClosureDependency {
                path: "zz/extra.data".to_owned(),
                kind: DependencyKind::Static,
            });
        }),
    ];
    // `mode` is fixed by `kind`, so a different mode is refused before hashing rather than
    // hashed differently.
    let mut other_mode = baseline.clone();
    other_mode.nodes[2].mode = 0o700;
    assert!(manifest_digest(&other_mode).is_err());
    let mut seen = std::collections::BTreeSet::from([before.clone()]);
    for (name, mutate) in mutations {
        let mut manifest = baseline.clone();
        mutate(&mut manifest);
        let after = manifest_digest(&manifest)
            .unwrap_or_else(|error| panic!("{name} left the manifest invalid: {error:?}"));
        assert_ne!(before, after, "{name} does not participate in the digest");
        assert!(
            seen.insert(after),
            "{name} yields the same digest as another mutation"
        );
    }
}

/// A manifest whose interpreter and entrypoint each have one eligible alternative node.
/// The alternatives hang off the extension root so every node stays reachable when a
/// launch field moves away from the node it named.
fn manifest_with_alternate_launch_roots() -> ClosureManifest {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/harness-closures/pi-valid.json");
    let mut manifest: ClosureManifest =
        serde_json::from_slice(&std::fs::read(fixture).expect("read closure fixture"))
            .expect("decode closure fixture");
    let mut alternate_interpreter = manifest.nodes[0].clone();
    alternate_interpreter.path = "bin/node2".to_owned();
    let mut alternate_entrypoint = manifest.nodes[1].clone();
    alternate_entrypoint.path =
        "node_modules/@earendil-works/pi-coding-agent/dist/cli2.js".to_owned();
    alternate_entrypoint.dependencies.clear();
    manifest.nodes.insert(1, alternate_interpreter);
    manifest.nodes.insert(3, alternate_entrypoint);
    let extension = manifest
        .nodes
        .iter_mut()
        .find(|node| node.kind == NodeKind::Extension)
        .expect("fixture has an extension node");
    extension.dependencies = vec![
        ClosureDependency {
            path: "bin/node".to_owned(),
            kind: DependencyKind::Static,
        },
        ClosureDependency {
            path: "bin/node2".to_owned(),
            kind: DependencyKind::Static,
        },
        ClosureDependency {
            path: "node_modules/@earendil-works/pi-coding-agent/dist/cli.js".to_owned(),
            kind: DependencyKind::Static,
        },
        ClosureDependency {
            path: "node_modules/@earendil-works/pi-coding-agent/dist/cli2.js".to_owned(),
            kind: DependencyKind::Static,
        },
    ];
    validate_manifest(&manifest).expect("alternate-root manifest is valid");
    manifest
}

#[test]
fn launch_roots_participate_in_the_digest_on_their_own() {
    let baseline = manifest_with_alternate_launch_roots();
    let before = manifest_digest(&baseline).expect("digest");

    let mut other_interpreter = baseline.clone();
    other_interpreter.interpreter = Some("bin/node2".to_owned());
    let interpreter_digest = manifest_digest(&other_interpreter).expect("digest");
    assert_ne!(
        before, interpreter_digest,
        "interpreter does not participate in the digest"
    );

    let mut other_entrypoint = baseline.clone();
    other_entrypoint.entrypoint =
        Some("node_modules/@earendil-works/pi-coding-agent/dist/cli2.js".to_owned());
    let entrypoint_digest = manifest_digest(&other_entrypoint).expect("digest");
    assert_ne!(
        before, entrypoint_digest,
        "entrypoint does not participate in the digest"
    );
    assert_ne!(interpreter_digest, entrypoint_digest);

    // The executable launch form: both `bin/node` nodes become executables, the
    // interpreted roots are cleared, and `executable` alone moves between them.
    let mut executable_form = baseline;
    for node in executable_form.nodes.iter_mut() {
        if node.kind == NodeKind::Interpreter {
            node.kind = NodeKind::Executable;
        }
    }
    executable_form.interpreter = None;
    executable_form.entrypoint = None;
    executable_form.executable = Some("bin/node".to_owned());
    validate_manifest(&executable_form).expect("executable-form manifest is valid");
    let executable_before = manifest_digest(&executable_form).expect("digest");
    let mut other_executable = executable_form.clone();
    other_executable.executable = Some("bin/node2".to_owned());
    let executable_digest = manifest_digest(&other_executable).expect("digest");
    assert_ne!(
        executable_before, executable_digest,
        "executable does not participate in the digest"
    );
    assert_ne!(executable_before, before);
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
        .materialize(&ClosureCandidate {
            manifest: read_manifest("opencode-linux-x64-1.18.22.json"),
            source_roots: BTreeMap::from([("runtime".to_owned(), PathBuf::from(opencode_root))]),
        })
        .expect("materialize OpenCode closure");
    assert!(
        opencode
            .resolve_node_descriptor("bin/opencode")
            .expect("OpenCode executable")
            .closure_path()
            .is_file()
    );

    let pi = store
        .materialize(&ClosureCandidate {
            manifest: read_manifest("pi-linux-x64-node-24.18.0.json"),
            source_roots: BTreeMap::from([
                ("pi-install".to_owned(), pi_install),
                ("runtime".to_owned(), pi_runtime),
            ]),
        })
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
        .materialize(&extra_candidate)
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
        .materialize(&missing_candidate)
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
        .materialize(&mode_candidate)
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

#[test]
fn prune_reclaims_unprotected_digests_and_stale_temps_only() {
    let (temp, _source, candidate) = setup();
    let store_root = temp.path().join("closures");
    let store = HarnessClosureStore::open(&store_root).expect("store");
    let closure = store.materialize(&candidate).expect("materialize");
    let digest = closure.digest().to_owned();

    std::fs::create_dir(store_root.join(".tmp-deadbeefdeadbeef")).expect("stale temp");
    std::fs::set_permissions(
        store_root.join(".tmp-deadbeefdeadbeef"),
        std::fs::Permissions::from_mode(0o700),
    )
    .expect("temp mode");
    std::fs::create_dir(store_root.join("foreign-entry")).expect("foreign entry");

    let protected = std::collections::BTreeSet::from([digest.clone()]);
    store.prune(&protected).expect("prune with protection");
    assert!(
        store_root.join(&digest).is_dir(),
        "protected digest survives"
    );
    assert!(
        !store_root.join(".tmp-deadbeefdeadbeef").exists(),
        "stale staging directory is reclaimed"
    );
    assert!(
        store_root.join("foreign-entry").is_dir(),
        "entries the store did not create are left untouched"
    );
    store
        .validate(&digest)
        .expect("protected closure still validates");

    store
        .prune(&std::collections::BTreeSet::new())
        .expect("prune without protection");
    assert!(
        !store_root.join(&digest).is_dir(),
        "unprotected digest is reclaimed"
    );
}

#[test]
fn native_edges_and_native_addons_must_correspond_exactly() {
    let (_temp, _source, candidate) = setup();

    // Validation rejects a non-`Native` dependency edge to a `NativeAddon`.
    // qualification-side biconditional.
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

    // Validation rejects a `NativeAddon` with no inbound `Native` dependency even if graph traversal reaches it.
    let mut unclaimed = candidate.clone();
    unclaimed.manifest.nodes[2].dependencies = vec![
        dependency(
            "node_modules/pi/dist/addon.node",
            DependencyKind::FiniteDynamic,
        ),
        dependency("node_modules/pi/dist/helper.js", DependencyKind::Static),
    ];
    assert_eq!(
        validate_manifest(&unclaimed.manifest)
            .expect_err("finite_dynamic edge onto native addon")
            .detail(),
        "native dependency kind must correspond exactly to a native addon target"
    );
}

/// Lexicographic path order can place `bin/node.dat` between `bin/node` and `bin/node/main.js`.
/// Lexicographic path order can place `bin/node.dat` between `bin/node` and `bin/node/main.js`.
/// Validation must reject a regular file that prefixes another manifest path.
/// Validation must compare every path with its ancestor paths, not only the immediately preceding sorted entry.
/// Validation rejects a regular file that prefixes another manifest path.
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
/// Dependencies with the same path but different kinds require explicit duplicate-path validation.
/// Duplicate dependency validation rejects dependencies with equal paths even when their `DependencyKind` values differ.
/// Duplicate dependency validation rejects dependencies with equal paths even when their `DependencyKind` values differ.
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
