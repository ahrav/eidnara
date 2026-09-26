//! The daemon's vendored summarizer seeds are ordinary engineering prose. The
//! production redactor must admit every one, raw and as canonical JSON.

use context_core::{canonical_json::canonical_json_encode, redaction::Redactor};
use serde_json::Value;

const SEEDS: &str = include_str!("../../daemon/testdata/reference-seeds.json");

#[test]
fn reference_seeds_carry_no_secret_findings() {
    let redactor = Redactor::new().unwrap();
    let seeds: Vec<Value> = serde_json::from_str(SEEDS).unwrap();
    assert_eq!(seeds.len(), 60);
    let mut failures = Vec::new();
    for (index, seed) in seeds.iter().enumerate() {
        let block = seed["block"].as_str().unwrap();
        let encoded = canonical_json_encode(seed).unwrap();
        for text in [block, encoded.as_str()] {
            for detection in redactor.redact(text).unwrap().detections {
                let span = &text[detection.offset..detection.offset + detection.length];
                failures.push(format!("seed {index}: {} {span:?}", detection.secret_type));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
