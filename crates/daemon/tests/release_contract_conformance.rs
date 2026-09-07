use std::ffi::OsString;

use host_runtime::broca::subprocess::{
    CREDENTIAL_FINGERPRINT_CANONICALIZATION, CREDENTIAL_FINGERPRINT_DOMAIN,
    CREDENTIAL_VALUE_CAP_BYTES, EnvSnapshot,
};
use host_runtime::harness_closure::{ClosureManifest, manifest_digest};

fn release_file(name: &str) -> serde_json::Value {
    let path = format!("{}/../../release/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(&path).expect("release file is readable"))
        .expect("release file parses")
}

/// Every committed release file names the same release and the same contract digest.
fn assert_release_binding(doc: &serde_json::Value, name: &str) {
    assert_eq!(
        doc["release"]["version"].as_str(),
        Some(daemon::release_contract::RELEASE_VERSION),
        "{name} must name the daemon crate's release version"
    );
    assert_eq!(
        doc["release_contract_sha256"].as_str(),
        Some(daemon::release_contract::release_contract_sha256()),
        "{name} must pin the committed contract digest"
    );
}

#[test]
fn credential_constants_match_the_release_contract() {
    let contract: serde_json::Value =
        serde_json::from_str(daemon::release_contract::RELEASE_CONTRACT_JSON)
            .expect("release contract parses");

    let fingerprint = &contract["credential_fingerprint"];
    assert_eq!(
        fingerprint["domain"].as_str(),
        Some(CREDENTIAL_FINGERPRINT_DOMAIN),
        "fingerprint key-derivation domain must match the published contract"
    );
    assert_eq!(
        fingerprint["canonicalization"].as_str(),
        Some(CREDENTIAL_FINGERPRINT_CANONICALIZATION),
        "fingerprint canonicalization id must match the published contract"
    );

    let caps = &contract["harness_unavailable"];
    assert_eq!(
        caps["value_cap_bytes"].as_u64(),
        Some(CREDENTIAL_VALUE_CAP_BYTES as u64),
        "credential value cap must match the published contract"
    );
}

#[test]
fn provider_credential_matrix_matches_the_published_doc() {
    let doc = release_file("provider-credentials.json");
    assert_release_binding(&doc, "provider-credentials.json");
    assert_eq!(
        doc["fingerprint"]["domain"].as_str(),
        Some(CREDENTIAL_FINGERPRINT_DOMAIN)
    );
    assert_eq!(
        doc["fingerprint"]["canonicalization"].as_str(),
        Some(CREDENTIAL_FINGERPRINT_CANONICALIZATION)
    );
    assert_eq!(
        doc["caps"]["value_cap_bytes"].as_u64(),
        Some(CREDENTIAL_VALUE_CAP_BYTES as u64)
    );
    assert_eq!(
        doc["caps"].as_object().map(|caps| caps.len()),
        Some(1),
        "the value cap is the only credential cap the runtime enforces"
    );

    let harnesses = doc["harnesses"]
        .as_object()
        .expect("harnesses object is published");
    assert_eq!(
        harnesses.keys().collect::<Vec<_>>(),
        vec!["opencode", "pi"],
        "published harness set must match the runtime allowlist"
    );
    for (harness, spec) in harnesses {
        let providers = spec["providers"]
            .as_object()
            .expect("providers object is published");
        assert_eq!(
            providers.keys().map(String::as_str).collect::<Vec<_>>(),
            EnvSnapshot::SUPPORTED_PROVIDERS,
            "published provider set for {harness} must match the runtime allowlist"
        );
        for (provider, row) in providers {
            assert_eq!(
                host_runtime::broca::subprocess::canonical_provider(harness, provider),
                Ok(provider.as_str()),
                "canonical provider {provider} must be accepted for {harness}"
            );
            let published: Vec<&str> = row["credential_variables"]
                .as_array()
                .expect("credential_variables is an array")
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect();
            // Runtime row selection over exactly the published variables must select exactly those variables.
            let snapshot = EnvSnapshot::capture_from(
                published
                    .iter()
                    .map(|name| (OsString::from(name), OsString::from("secret"))),
            )
            .expect("published variables fit the snapshot ceiling");
            let selected: Vec<String> = snapshot
                .provider_row(harness, provider)
                .expect("published variables satisfy the runtime row")
                .into_iter()
                .map(|(name, _)| name.into_string().expect("variable name is UTF-8"))
                .collect();
            assert_eq!(
                selected, published,
                "published variables for {harness}/{provider} must match the runtime row selection"
            );
        }
        let aliases = spec["aliases"]
            .as_object()
            .expect("aliases object is published");
        for (alias, spec) in aliases {
            let canonical = spec["canonical"].as_str().expect("alias canonical name");
            assert_eq!(
                host_runtime::broca::subprocess::canonical_provider(harness, alias),
                Ok(canonical),
                "published {harness} alias {alias} must canonicalize identically at runtime"
            );
        }
    }
    for harness in ["opencode", "pi"] {
        assert!(
            host_runtime::broca::subprocess::canonical_provider(harness, "bedrock").is_err(),
            "unpublished provider must stay rejected for {harness}"
        );
    }
    assert!(
        host_runtime::broca::subprocess::canonical_provider("opencode", "google-antigravity")
            .is_err(),
        "Pi-only aliases must stay rejected for opencode"
    );
}

#[test]
fn rust_canonical_encoding_reproduces_every_qualified_closure_digest() {
    let lock: serde_json::Value =
        serde_json::from_str(daemon::production_inputs::PRODUCTION_INPUTS_LOCK_JSON)
            .expect("production inputs lock parses");
    assert_release_binding(&lock, "production-inputs.lock.json");
    let harnesses = lock["harnesses"]
        .as_object()
        .expect("lock publishes a harnesses object");
    assert_eq!(
        harnesses.len(),
        daemon::production_inputs::QUALIFIED_HARNESS_CLOSURES.len(),
        "every locked harness has an embedded closure manifest"
    );
    for (name, digest, bytes) in daemon::production_inputs::QUALIFIED_HARNESS_CLOSURES {
        let manifest: ClosureManifest =
            serde_json::from_str(bytes).expect("qualified closure manifest parses");
        assert_eq!(
            manifest_digest(&manifest).expect("manifest validates and digests"),
            *digest,
            "Rust-derived canonical digest for {name} must equal the embedded digest"
        );
        let closure = &harnesses[*name]["closure"];
        assert_eq!(
            closure["sha256"].as_str(),
            Some(*digest),
            "lock digest for {name} must equal the embedded digest"
        );
        assert_eq!(closure["qualified"], serde_json::json!(true));
        let manifest_path = closure["manifest_path"]
            .as_str()
            .expect("lock names the manifest path");
        let on_disk = std::fs::read_to_string(format!(
            "{}/../../{manifest_path}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("locked manifest path is readable");
        assert_eq!(
            on_disk, *bytes,
            "embedded manifest for {name} must be the file the lock names"
        );
        // The lock's anchors let a parent resolve each source root from its own
        // process paths without reading the manifest; each anchor must name the
        // manifest's executable, interpreter, or entrypoint node for that root.
        let anchors = closure["anchors"]
            .as_object()
            .expect("lock closure publishes anchors");
        assert_eq!(
            anchors.keys().collect::<Vec<_>>(),
            manifest.source_roots.iter().collect::<Vec<_>>(),
            "anchors for {name} cover exactly the manifest's source roots"
        );
        for (root, anchor) in anchors {
            let from = anchor["from"].as_str().expect("anchor names its source");
            let expected_path = match from {
                "executable" => manifest.executable.as_deref(),
                "interpreter" => manifest.interpreter.as_deref(),
                "entrypoint" => manifest.entrypoint.as_deref(),
                other => panic!("anchor for {name}/{root} names unknown source {other}"),
            }
            .unwrap_or_else(|| panic!("manifest for {name} has no {from} for root {root}"));
            let node = manifest
                .nodes
                .iter()
                .find(|node| node.path == expected_path && node.source_root == *root)
                .unwrap_or_else(|| {
                    panic!("manifest for {name} has no node {expected_path} under {root}")
                });
            assert_eq!(
                anchor["source_path"].as_str(),
                Some(node.source_path.as_str()),
                "anchor source path for {name}/{root} must be the node's source path"
            );
        }
    }
}

/// `production-inputs.lock.json` repeats the contract's platform floors,
/// install layouts, and model lane. Pinning each copy independently lets them
/// drift; this holds the two files to one value.
#[test]
fn lock_platform_blocks_match_the_release_contract() {
    let contract: serde_json::Value =
        serde_json::from_str(daemon::release_contract::RELEASE_CONTRACT_JSON)
            .expect("release contract parses");
    let lock: serde_json::Value =
        serde_json::from_str(daemon::production_inputs::PRODUCTION_INPUTS_LOCK_JSON)
            .expect("production inputs lock parses");

    assert_eq!(
        lock["install_layouts"], contract["install_layouts"],
        "lock install layouts must equal the contract's"
    );

    for key in ["execution_provider", "id", "platforms"] {
        assert_eq!(
            lock["model_lane"][key], contract["model_lane"][key],
            "lock model_lane.{key} must equal the contract's"
        );
    }

    let supported = contract["platforms"]["supported"]
        .as_array()
        .expect("contract lists supported platforms");
    let floors = lock["platform_floors"]
        .as_object()
        .expect("lock lists platform floors");
    assert_eq!(
        floors.len(),
        supported.len(),
        "lock floors and contract platforms must cover the same target set"
    );
    for platform in supported {
        let target = platform["target"].as_str().expect("platform target");
        let floor = &floors[target];
        assert_eq!(
            floor["glibc_min"], platform["glibc_min"],
            "glibc floor for {target}"
        );
        assert_eq!(
            floor["kernel_min"], platform["kernel_min"],
            "kernel floor for {target}"
        );
        assert_eq!(
            floor["procfs_self_fd_exec"], platform["capabilities"]["procfs_self_fd_exec"],
            "procfs capability for {target}"
        );
    }
}

/// The published `closure_manifest_schema` mirrors `host_runtime::harness_closure`'s
/// `deny_unknown_fields` types. A field or variant added in Rust without a doc
/// update would otherwise drift silently; this derives the published lists from
/// the serde types themselves.
#[test]
fn published_closure_manifest_schema_matches_the_runtime_types() {
    use host_runtime::harness_closure::{
        CLOSURE_SCHEMA, ClosureDependency, ClosureNode, DependencyKind, NodeKind,
    };

    let doc = release_file("provider-credentials.json");
    let schema = &doc["closure_manifest_schema"];

    assert_eq!(
        schema["id"].as_str(),
        Some(CLOSURE_SCHEMA),
        "published schema id must be the id the runtime validates"
    );

    fn object_keys(value: &serde_json::Value) -> Vec<String> {
        value
            .as_object()
            .expect("serialized struct is an object")
            .keys()
            .cloned()
            .collect()
    }
    fn published_strings(value: &serde_json::Value) -> Vec<String> {
        value
            .as_array()
            .expect("published list is an array")
            .iter()
            .map(|v| v.as_str().expect("published list entry").to_owned())
            .collect()
    }

    let node = ClosureNode {
        path: String::new(),
        source_root: String::new(),
        source_path: String::new(),
        kind: NodeKind::Data,
        mode: 0,
        size_bytes: 0,
        sha256: String::new(),
        dependencies: Vec::new(),
    };
    let manifest = ClosureManifest {
        schema: String::new(),
        harness: String::new(),
        package: String::new(),
        version: String::new(),
        argument_variant: String::new(),
        source_roots: Vec::new(),
        executable: None,
        interpreter: None,
        entrypoint: None,
        extensions: Vec::new(),
        nodes: Vec::new(),
    };
    let dependency = ClosureDependency {
        path: String::new(),
        kind: DependencyKind::Static,
    };

    let mut manifest_fields = object_keys(&serde_json::to_value(&manifest).unwrap());
    manifest_fields.sort();
    assert_eq!(
        published_strings(&schema["fields"]),
        manifest_fields,
        "published manifest fields must equal ClosureManifest's serialized keys"
    );

    let mut node_fields = object_keys(&serde_json::to_value(&node).unwrap());
    node_fields.sort();
    assert_eq!(
        published_strings(&schema["node"]["fields"]),
        node_fields,
        "published node fields must equal ClosureNode's serialized keys"
    );

    let mut dependency_fields = object_keys(&serde_json::to_value(&dependency).unwrap());
    dependency_fields.sort();
    assert_eq!(
        published_strings(&schema["dependency"]["fields"]),
        dependency_fields,
        "published dependency fields must equal ClosureDependency's serialized keys"
    );

    // Adding an enum variant breaks these matches at compile time, which
    // forces the lists below (and the published doc) to be revisited.
    let node_kinds = [
        NodeKind::Interpreter,
        NodeKind::Executable,
        NodeKind::Module,
        NodeKind::NativeAddon,
        NodeKind::Extension,
        NodeKind::Data,
    ];
    for kind in node_kinds {
        match kind {
            NodeKind::Interpreter
            | NodeKind::Executable
            | NodeKind::Module
            | NodeKind::NativeAddon
            | NodeKind::Extension
            | NodeKind::Data => {}
        }
    }
    let mut node_kind_names: Vec<String> = node_kinds
        .iter()
        .map(|kind| {
            serde_json::to_value(kind)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    node_kind_names.sort();
    assert_eq!(
        published_strings(&schema["node_kinds"]),
        node_kind_names,
        "published node kinds must equal NodeKind's serialized variants"
    );

    let dependency_kinds = [
        DependencyKind::Static,
        DependencyKind::FiniteDynamic,
        DependencyKind::Native,
    ];
    for kind in dependency_kinds {
        match kind {
            DependencyKind::Static | DependencyKind::FiniteDynamic | DependencyKind::Native => {}
        }
    }
    let mut dependency_kind_names: Vec<String> = dependency_kinds
        .iter()
        .map(|kind| {
            serde_json::to_value(kind)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    dependency_kind_names.sort();
    assert_eq!(
        published_strings(&schema["dependency"]["kinds"]),
        dependency_kind_names,
        "published dependency kinds must equal DependencyKind's serialized variants"
    );
}
