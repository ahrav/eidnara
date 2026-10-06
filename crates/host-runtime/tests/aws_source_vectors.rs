use std::collections::BTreeMap;

use host_runtime::model_execution::aws_source::{
    AwsProfileSource, deserialize_present, lexically_normal, profile_mode_credentials,
    validate_source_binding,
};
use serde::Deserialize;
use serde_json::Value;

fn vectors() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/aws-source-vectors.json"
    );
    serde_json::from_slice(&std::fs::read(path).expect("vectors")).expect("vectors JSON")
}

fn cases<'a>(vectors: &'a Value, set: &str) -> &'a [Value] {
    vectors[set].as_array().expect("vector set")
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    #[serde(default, deserialize_with = "deserialize_present")]
    aws_source: Option<AwsProfileSource>,
    #[serde(default)]
    credentials: BTreeMap<String, String>,
}

fn decoded_valid(value: &Value) -> bool {
    serde_json::from_value::<AwsProfileSource>(value.clone()).is_ok()
}

#[test]
fn the_native_validator_agrees_with_every_dto_vector() {
    let vectors = vectors();
    for case in cases(&vectors, "dto") {
        assert_eq!(
            decoded_valid(&case["value"]),
            case["valid"].as_bool().unwrap(),
            "{}",
            case["name"]
        );
    }
}

#[test]
fn every_selector_the_client_builds_passes_the_native_validator() {
    let vectors = vectors();
    let built: Vec<&Value> = cases(&vectors, "selector")
        .iter()
        .filter_map(|case| case["expect"].get("source"))
        .collect();
    assert!(built.len() >= 5);
    for source in built {
        assert!(decoded_valid(source), "{source}");
    }
}

#[test]
fn bindings_reject_null_and_profile_mode_static_aws_credentials() {
    let vectors = vectors();
    for case in cases(&vectors, "binding") {
        let valid = serde_json::from_value::<Binding>(case["binding"].clone())
            .is_ok_and(|b| validate_source_binding(b.aws_source.as_ref(), &b.credentials).is_ok());
        assert_eq!(valid, case["valid"].as_bool().unwrap(), "{}", case["name"]);
    }
    let absent: Binding = serde_json::from_str("{}").unwrap();
    assert!(absent.aws_source.is_none());
}

#[test]
fn profile_mode_stripping_matches_every_shared_vector_and_passes_the_binding_check() {
    let vectors = vectors();
    let source: AwsProfileSource =
        serde_json::from_value(cases(&vectors, "dto")[0]["value"].clone()).unwrap();
    for case in cases(&vectors, "strip") {
        let legacy: BTreeMap<String, String> =
            serde_json::from_value(case["credentials"].clone()).unwrap();
        let expected: BTreeMap<String, String> =
            serde_json::from_value(case["kept"].clone()).unwrap();
        let kept = profile_mode_credentials(&legacy);
        assert_eq!(kept, expected, "{}", case["name"]);
        assert!(validate_source_binding(Some(&source), &kept).is_ok());
        assert!(validate_source_binding(Some(&source), &legacy).is_err());
    }
}

#[test]
fn a_duplicated_field_or_an_invalid_field_fails_at_decode() {
    let duplicated = r#"{"kind":"profile","profile":"corp","profile":"other","region":"us-east-1",
        "config_file":"/c","credentials_file":"/k","sso_cache_root":"/h/.aws/sso/cache"}"#;
    assert!(serde_json::from_str::<AwsProfileSource>(duplicated).is_err());
    let relative = r#"{"aws_source":{"kind":"profile","profile":"corp","region":"us-east-1",
        "config_file":"c","credentials_file":"/k","sso_cache_root":"/h/.aws/sso/cache"}}"#;
    let error = serde_json::from_str::<Binding>(relative)
        .err()
        .expect("relative path");
    assert!(error.to_string().contains("config_file"), "{error}");
}

#[test]
fn lexical_normalization_never_leaves_the_root_and_refuses_relative_paths() {
    assert_eq!(
        lexically_normal("/../../a/./b//c/..").as_deref(),
        Some("/a/b")
    );
    assert_eq!(lexically_normal("/").as_deref(), Some("/"));
    assert_eq!(lexically_normal("a/b"), None);
    assert_eq!(lexically_normal(""), None);
}
