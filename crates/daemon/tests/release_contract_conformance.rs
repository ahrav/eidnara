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
    }
}
