use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use serde_json::Value;

const PINNED: &[(&str, &str)] = &[
    ("aws-runtime", "1.10.0"),
    ("aws-types", "1.6.0"),
    ("aws-credential-types", "1.3.0"),
    ("aws-smithy-runtime", "1.15.0"),
    ("aws-smithy-runtime-api", "1.18.0"),
    ("aws-smithy-types", "1.8.1"),
    ("tokio", "1.53.1"),
];

const FORBIDDEN_FEATURES: &[&str] = &["credentials-process", "credentials-login"];

const NETWORK_CRATES: &[&str] = &[
    "aws-config",
    "aws-smithy-http-client",
    "hyper",
    "rustls",
    "reqwest",
];

#[test]
fn resolved_aws_closure_matches_the_pins() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new(env!("CARGO"))
        .args([
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--offline",
            "--all-features",
        ])
        .current_dir(&workspace)
        .output()
        .expect("cargo metadata runs");
    assert!(output.status.success(), "cargo metadata failed");
    let metadata: Value = serde_json::from_slice(&output.stdout).expect("metadata JSON");
    let mut versions: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for package in metadata["packages"].as_array().expect("packages") {
        let name = package["name"].as_str().expect("name");
        versions
            .entry(name)
            .or_default()
            .push(package["version"].as_str().expect("version"));
    }
    for (name, version) in PINNED {
        assert_eq!(versions.get(name), Some(&vec![*version]), "{name}");
    }
    for name in normal_closure(&metadata, "host-runtime") {
        assert!(
            !NETWORK_CRATES.contains(&name.as_str()),
            "host-runtime reaches {name}"
        );
    }
    let nodes = metadata["resolve"]["nodes"].as_array().expect("nodes");
    for node in nodes {
        let id = node["id"].as_str().expect("id");
        if !id.contains("aws-") {
            continue;
        }
        for feature in node["features"].as_array().expect("features") {
            let feature = feature.as_str().expect("feature");
            assert!(
                !FORBIDDEN_FEATURES.contains(&feature),
                "{id} enables {feature}"
            );
        }
    }
}

/// A dependency entry's `name` is the depending crate's alias, so package names come
/// from `packages` by id.
fn normal_closure(metadata: &Value, root: &str) -> Vec<String> {
    let package_names: BTreeMap<&str, &str> = metadata["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .map(|package| {
            let id = package["id"].as_str().expect("package id");
            (id, package["name"].as_str().expect("package name"))
        })
        .collect();
    let nodes = metadata["resolve"]["nodes"].as_array().expect("nodes");
    let root = nodes
        .iter()
        .find(|node| node["id"].as_str().is_some_and(|id| id.contains(root)))
        .expect("root node");
    let mut pending = vec![root];
    let mut seen = BTreeSet::new();
    let mut names = Vec::new();
    while let Some(node) = pending.pop() {
        for dep in node["deps"].as_array().expect("deps") {
            let normal = dep["dep_kinds"]
                .as_array()
                .expect("dep kinds")
                .iter()
                .any(|kind| kind["kind"].is_null());
            let id = dep["pkg"].as_str().expect("pkg");
            if normal && seen.insert(id) {
                names.push(package_names[id].to_owned());
                pending.push(nodes.iter().find(|n| n["id"] == id).expect("dep node"));
            }
        }
    }
    names
}

#[test]
fn closure_names_renamed_dependencies_by_package() {
    let metadata = serde_json::json!({
        "packages": [
            { "id": "path+file:///w/crates/host-runtime#0.1.0", "name": "host-runtime" },
            { "id": "registry+https://x#hyper@1.0.0", "name": "hyper" },
        ],
        "resolve": { "nodes": [
            {
                "id": "path+file:///w/crates/host-runtime#0.1.0",
                "deps": [{
                    "name": "net_client",
                    "pkg": "registry+https://x#hyper@1.0.0",
                    "dep_kinds": [{ "kind": null }],
                }],
            },
            { "id": "registry+https://x#hyper@1.0.0", "deps": [] },
        ] },
    });
    assert_eq!(normal_closure(&metadata, "host-runtime"), ["hyper"]);
}
