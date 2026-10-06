use std::collections::BTreeMap;
use std::ffi::OsString;

use host_runtime::model_execution::aws_source::AwsProfileSource;
use host_runtime::model_execution::protocol::{ErrorScope, UnknownErrorScope};
use host_runtime::model_execution::source_claim::{
    ClaimSource, SOURCE_CLAIM_CANONICALIZATION, SOURCE_CLAIM_DOMAIN, presented_claim_matches,
    source_claim, source_claim_preimage,
};
use host_runtime::model_execution::subprocess::EnvSnapshot;
use serde_json::{Value, json};

fn vectors() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/source-claim-vectors.json"
    );
    serde_json::from_slice(&std::fs::read(path).expect("vectors")).expect("vectors JSON")
}

fn key(hex: &str) -> [u8; 32] {
    let mut key = [0u8; 32];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * index..2 * index + 2], 16).unwrap();
    }
    key
}

fn row(entries: &Value) -> Vec<(OsString, OsString)> {
    entries
        .as_array()
        .unwrap()
        .iter()
        .map(|pair| {
            (
                OsString::from(pair[0].as_str().unwrap()),
                OsString::from(pair[1].as_str().unwrap()),
            )
        })
        .collect()
}

fn profile() -> AwsProfileSource {
    serde_json::from_value(json!({
        "kind": "profile", "profile": "corp", "region": "us-east-1",
        "config_file": "/home/u/.aws/config", "credentials_file": "/home/u/.aws/credentials",
        "sso_cache_root": "/home/u/.aws/sso/cache",
    }))
    .unwrap()
}

#[test]
fn every_vector_matches_its_preimage_and_claim() {
    let vectors = vectors();
    assert_eq!(vectors["domain"], SOURCE_CLAIM_DOMAIN);
    assert_eq!(vectors["canonicalization"], SOURCE_CLAIM_CANONICALIZATION);
    for vector in vectors["vectors"].as_array().unwrap() {
        let source = &vector["source"];
        let entries;
        let profile: AwsProfileSource;
        let claim_source = if source["kind"] == "env" {
            entries = row(&source["entries"]);
            ClaimSource::Env(&entries)
        } else {
            profile = serde_json::from_value(source["profile"].clone()).unwrap();
            ClaimSource::AwsProfile(&profile)
        };
        let preimage = source_claim_preimage(
            vector["harness"].as_str().unwrap(),
            vector["provider"].as_str().unwrap(),
            &claim_source,
        );
        assert_eq!(
            String::from_utf8(preimage.clone()).unwrap(),
            vector["preimage"].as_str().unwrap(),
            "{}",
            vector["name"]
        );
        let key = key(vector["key_hex"].as_str().unwrap());
        let claim = source_claim(&key, &preimage);
        assert_eq!(
            claim,
            vector["claim"].as_str().unwrap(),
            "{}",
            vector["name"]
        );
        let mut vars: Vec<(String, String)> = match &claim_source {
            ClaimSource::Env(row) => row
                .iter()
                .map(|(n, v)| (n.to_string_lossy().into(), v.to_string_lossy().into()))
                .collect(),
            ClaimSource::AwsProfile(_) => Vec::new(),
        };
        vars.reverse();
        vars.push(("UNRELATED".into(), "x".into()));
        let pairs: Vec<(&str, &str)> = vars.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
        let aws_source = match &claim_source {
            ClaimSource::AwsProfile(profile) => Some(*profile),
            ClaimSource::Env(_) => None,
        };
        let through_snapshot = snapshot(&pairs)
            .source_claim(
                &key,
                vector["harness"].as_str().unwrap(),
                vector["provider"].as_str().unwrap(),
                aws_source,
            )
            .unwrap();
        assert_eq!(
            through_snapshot, claim,
            "{} through the row path",
            vector["name"]
        );
    }
}

fn pinned(name: &str) -> String {
    vectors()["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == name)
        .map(|v| v["claim"].as_str().unwrap().to_owned())
        .unwrap()
}

#[test]
fn aliases_and_an_empty_token_claim_their_canonical_rows() {
    let key: [u8; 32] = std::array::from_fn(|i| i as u8);
    let openai = snapshot(&[("OPENAI_API_KEY", "o-key")]);
    assert_eq!(
        openai
            .source_claim(&key, "pi", "openai-codex", None)
            .unwrap(),
        pinned("openai env for pi")
    );
    let google = snapshot(&[("GEMINI_API_KEY", "g-key")]);
    assert_eq!(
        google
            .source_claim(&key, "pi", "google-antigravity", None)
            .unwrap(),
        pinned("google env")
    );
    let empty_token = snapshot(&[
        ("AWS_ACCESS_KEY_ID", "AKIA1"),
        ("AWS_SECRET_ACCESS_KEY", "s3cr3t"),
        ("AWS_SESSION_TOKEN", ""),
        ("AWS_REGION", "us-east-1"),
    ]);
    assert_eq!(
        empty_token
            .source_claim(&key, "pi", "amazon-bedrock", None)
            .unwrap(),
        pinned("bedrock env without token")
    );
}

fn snapshot(vars: &[(&str, &str)]) -> EnvSnapshot {
    EnvSnapshot::capture_from(
        vars.iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value))),
    )
    .unwrap()
}

#[test]
fn a_profile_claim_ignores_rotating_rows_and_follows_the_bearer_and_selector() {
    let key: [u8; 32] = std::array::from_fn(|i| i as u8);
    let profile = profile();
    let row = |id, secret, token| {
        snapshot(&[
            ("AWS_ACCESS_KEY_ID", id),
            ("AWS_SECRET_ACCESS_KEY", secret),
            ("AWS_SESSION_TOKEN", token),
            ("AWS_REGION", "us-east-1"),
            ("ANTHROPIC_API_KEY", "k"),
        ])
    };
    let first = row("ASIA1", "secret-1", "token-1");
    let rotated = row("ASIA2", "secret-2", "token-2");
    let claim = |env: &EnvSnapshot, key: &[u8; 32], source: Option<&AwsProfileSource>| {
        env.source_claim(key, "pi", "amazon-bedrock", source)
    };
    let base = claim(&first, &key, Some(&profile)).unwrap();
    assert_eq!(base, pinned("bedrock profile"));
    assert_eq!(claim(&rotated, &key, Some(&profile)).unwrap(), base);
    assert_ne!(
        claim(&first, &key, None).unwrap(),
        claim(&rotated, &key, None).unwrap()
    );
    assert_ne!(claim(&first, &[8u8; 32], Some(&profile)).unwrap(), base);
    let replaced: AwsProfileSource = serde_json::from_value(json!({
        "kind": "profile", "profile": "other", "region": "us-east-1",
        "config_file": "/home/u/.aws/config", "credentials_file": "/home/u/.aws/credentials",
        "sso_cache_root": "/home/u/.aws/sso/cache",
    }))
    .unwrap();
    assert_ne!(claim(&first, &key, Some(&replaced)).unwrap(), base);
    let tokenless = snapshot(&[("AWS_SESSION_TOKEN", "t")]);
    assert!(
        claim(&tokenless, &key, None).is_err(),
        "environment mode needs the static row"
    );
    let direct = first
        .source_claim(&key, "pi", "anthropic", Some(&profile))
        .unwrap();
    assert_eq!(
        direct,
        first.source_claim(&key, "pi", "anthropic", None).unwrap(),
        "a direct key claims its environment row in both modes"
    );
}

#[test]
fn environment_and_profile_claims_never_collide() {
    let key = [7u8; 32];
    let env = snapshot(&[
        ("AWS_ACCESS_KEY_ID", "AKIA"),
        ("AWS_SECRET_ACCESS_KEY", "s"),
        ("AWS_REGION", "us-east-1"),
    ]);
    let profile = profile();
    assert_ne!(
        env.source_claim(&key, "pi", "amazon-bedrock", None)
            .unwrap(),
        env.source_claim(&key, "pi", "amazon-bedrock", Some(&profile))
            .unwrap()
    );
    assert_ne!(
        env.source_claim(&key, "pi", "amazon-bedrock", Some(&profile))
            .unwrap(),
        env.source_claim(&key, "opencode", "amazon-bedrock", Some(&profile))
            .unwrap()
    );
}

#[test]
fn a_presented_claim_must_match_the_expected_value_for_its_provider() {
    let expected = "a".repeat(64);
    let presented = BTreeMap::from([("amazon-bedrock".to_owned(), expected.clone())]);
    assert!(presented_claim_matches(
        &expected,
        &presented,
        "amazon-bedrock"
    ));
    assert!(!presented_claim_matches(&expected, &presented, "anthropic"));
    for malformed in [
        "b".repeat(64),
        expected.to_uppercase(),
        expected[..63].to_owned(),
        format!("{expected}a"),
    ] {
        let presented = BTreeMap::from([("amazon-bedrock".to_owned(), malformed)]);
        assert!(!presented_claim_matches(
            &expected,
            &presented,
            "amazon-bedrock"
        ));
    }
}

#[test]
fn error_scope_decodes_absent_as_model_and_rejects_every_unknown_value() {
    let decode = |body: Value| ErrorScope::decode(&body);
    assert_eq!(decode(json!({"class": "transient"})), Ok(ErrorScope::Model));
    assert_eq!(decode(json!({"scope": "model"})), Ok(ErrorScope::Model));
    assert_eq!(
        decode(json!({"scope": "credential_source"})),
        Ok(ErrorScope::CredentialSource)
    );
    for body in [json!("x"), json!(null), json!([])] {
        assert_eq!(decode(body.clone()), Err(UnknownErrorScope(body)));
    }
    for unknown in [
        json!("auth"),
        json!(""),
        json!(null),
        json!(1),
        json!("Model"),
        json!({}),
    ] {
        assert_eq!(
            decode(json!({"scope": unknown.clone()})),
            Err(UnknownErrorScope(unknown))
        );
    }
    for scope in [ErrorScope::Model, ErrorScope::CredentialSource] {
        assert_eq!(decode(json!({"scope": scope.as_wire_str()})), Ok(scope));
    }
}
