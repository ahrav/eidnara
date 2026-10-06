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
    ("aws-config", "1.12.0"),
    ("aws-sdk-sts", "1.118.0"),
    ("aws-sdk-sso", "1.113.0"),
    ("aws-sdk-ssooidc", "1.115.0"),
    ("aws-smithy-http-client", "1.4.2"),
];

const AWS_CONFIG_FEATURES: &[&str] = &["default-https-client", "rt-tokio", "sso"];

const FORBIDDEN_FEATURES: &[&str] = &["credentials-process", "credentials-login"];

/// Crates that would add a model client, a login provider, or a second HTTP stack.
const ABSENT_CRATES: &[&str] = &["aws-sdk-bedrockruntime", "aws-sdk-signin", "reqwest"];

/// Unfiltered `cargo metadata` requires cached foreign-platform dependencies
/// for offline resolution; a host build caches host-platform crates only.
fn host_triple() -> String {
    let output = Command::new(env!("CARGO"))
        .arg("-vV")
        .output()
        .expect("cargo -vV runs");
    assert!(output.status.success(), "cargo -vV failed");
    let stdout = String::from_utf8(output.stdout).expect("cargo -vV is UTF-8");
    stdout
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .expect("cargo -vV reports a host")
        .trim()
        .to_owned()
}

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
            "--filter-platform",
            &host_triple(),
        ])
        .current_dir(&workspace)
        .output()
        .expect("cargo metadata runs");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
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
    for name in ABSENT_CRATES {
        assert!(!versions.contains_key(name), "{name} is in the closure");
    }
    let nodes = metadata["resolve"]["nodes"].as_array().expect("nodes");
    for node in nodes {
        let id = node["id"].as_str().expect("id");
        if id.ends_with("#aws-config@1.12.0") {
            let features: Vec<_> = node["features"]
                .as_array()
                .expect("features")
                .iter()
                .filter_map(Value::as_str)
                .collect();
            assert_eq!(features, AWS_CONFIG_FEATURES, "unified aws-config features");
        }
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
