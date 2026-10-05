use std::collections::BTreeMap;
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
    let nodes = metadata["resolve"]["nodes"].as_array().expect("nodes");
    let host_runtime = nodes
        .iter()
        .find(|node| {
            node["id"]
                .as_str()
                .is_some_and(|id| id.contains("host-runtime"))
        })
        .expect("host-runtime node");
    let mut pending = vec![host_runtime];
    let mut seen = std::collections::BTreeSet::new();
    while let Some(node) = pending.pop() {
        for dep in node["deps"].as_array().expect("deps") {
            let normal = dep["dep_kinds"]
                .as_array()
                .expect("dep kinds")
                .iter()
                .any(|kind| kind["kind"].is_null());
            let id = dep["pkg"].as_str().expect("pkg");
            if normal && seen.insert(id) {
                let name = dep["name"].as_str().expect("dep name").replace('_', "-");
                assert!(
                    !NETWORK_CRATES.contains(&name.as_str()),
                    "host-runtime reaches {name}"
                );
                pending.push(nodes.iter().find(|n| n["id"] == id).expect("dep node"));
            }
        }
    }
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
