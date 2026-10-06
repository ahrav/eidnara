use host_runtime::model_execution::source_health::{FIELDS, sanitize};
use serde_json::Value;

#[test]
fn rust_sanitizer_matches_the_shared_vectors() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/source-health-vectors.json"
    );
    let vectors: Value =
        serde_json::from_slice(&std::fs::read(path).expect("vectors")).expect("vectors JSON");
    assert_eq!(vectors["fields"], serde_json::json!(FIELDS));
    assert_eq!(vectors["max_consecutive_failures"], u32::MAX);
    for case in vectors["cases"].as_array().expect("cases") {
        let closed = Some(case["closed"].clone()).filter(|closed| !closed.is_null());
        assert_eq!(sanitize(&case["input"]), closed, "{}", case["name"]);
    }
}
