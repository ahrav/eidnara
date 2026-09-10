//! Canonical JSON encoding and the digests built over it, shared with the
//! TypeScript runtime.
//!
//! Golden cross-runtime cases live in `testdata/canonical-json-contract-v1.json`.
//!
//! Canonical values are null, booleans, safe integers with |n| <= 2^53 - 1, strings, arrays, and objects.
//! Floats with fractional parts, non-finite numbers, and out-of-range integers are rejected.
//! Safe integral floats encode as integers.
//! `JSON.parse` cannot distinguish `1e3` from `1000`.
//! Objects sort keys by Unicode code point (Rust `str` ordering).
//! Canonical strings escape `"`, `\`, and U+0000–U+001F as lowercase `\u00xx`.
//! Canonical strings use no short escapes and emit every other code point literally.
//! Canonical JSON contains no insignificant whitespace.
//!
//! Digests are SHA-256 hex over `<protocol>\n<canonical JSON>` UTF-8 bytes.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde_json::{Number, Value};
use sha2::{Digest, Sha256};

/// Version of the input shape a Dreamer request digest is computed over.
pub const DREAMER_REQUEST_ENCODING_VERSION: u32 = 1;
pub const DREAMER_REQUEST_DIGEST_PROTOCOL: &str = "eidnara-dreamer-request-v1";

/// Both runtimes represent integers through 2^53 - 1 exactly.
const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum ContractError {
    /// `NotCanonical` reports fractional floats, out-of-range integers, and non-finite numbers.
    #[error("not canonical: {0}")]
    NotCanonical(String),
}

/// Return the exact cross-runtime safe integer represented by a JSON number.
/// `None` indicates that the number is not an exact cross-runtime safe integer.
///
/// An integer literal above `i64::MAX` has no `as_i64` view and falls through to the
/// float check, where its magnitude exceeds `MAX_SAFE_INTEGER` and is rejected.
fn number_as_safe_integer(number: &Number) -> Option<i64> {
    if let Some(value) = number.as_i64() {
        return (-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER)
            .contains(&value)
            .then_some(value);
    }
    let value = number.as_f64()?;
    if !value.is_finite() || value.fract() != 0.0 || value.abs() > MAX_SAFE_INTEGER as f64 {
        return None;
    }
    Some(value as i64)
}

fn encode_canonical_string(out: &mut String, value: &str) {
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn encode_canonical_value(out: &mut String, value: &Value) -> Result<(), ContractError> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(number) => {
            let integer = number_as_safe_integer(number).ok_or_else(|| {
                ContractError::NotCanonical(format!("number {number} is not a safe integer"))
            })?;
            let _ = write!(out, "{integer}");
        }
        Value::String(text) => encode_canonical_string(out, text),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                encode_canonical_value(out, item)?;
            }
            out.push(']');
        }
        Value::Object(entries) => {
            let sorted: BTreeMap<&String, &Value> = entries.iter().collect();
            out.push('{');
            for (index, (key, item)) in sorted.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                encode_canonical_string(out, key);
                out.push(':');
                encode_canonical_value(out, item)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

/// Encodes a JSON value using the canonical form documented by this module.
///
/// Returns [`ContractError::NotCanonical`] when a number is fractional, non-finite,
/// or outside the cross-runtime safe-integer range.
pub fn canonical_json_encode(value: &Value) -> Result<String, ContractError> {
    let mut out = String::new();
    encode_canonical_value(&mut out, value)?;
    Ok(out)
}

fn lower_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Hashes `<protocol>\n<canonical JSON>` without materializing the joined string.
fn protocol_digest(protocol: &str, value: &Value) -> Result<String, ContractError> {
    let canonical = canonical_json_encode(value)?;
    let mut hasher = Sha256::new();
    hasher.update(protocol.as_bytes());
    hasher.update(b"\n");
    hasher.update(canonical.as_bytes());
    Ok(lower_hex(&hasher.finalize()))
}

/// Computes the digest a Dreamer receipt binds its request to, over the
/// effect-defining inputs the caller assembled.
pub fn compute_dreamer_request_digest(inputs: &Value) -> Result<String, ContractError> {
    protocol_digest(DREAMER_REQUEST_DIGEST_PROTOCOL, inputs)
}

/// Reports whether `text` contains exactly `expected_len` lowercase ASCII hex bytes.
pub fn is_lower_hex(text: &str, expected_len: usize) -> bool {
    text.len() == expected_len
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../testdata/canonical-json-contract-v1.json");

    fn fixture() -> Value {
        serde_json::from_str(FIXTURE).expect("golden corpus parses")
    }

    /// Pins the protocol strings so a rename is observed here rather than
    /// only through the fixture that was renamed alongside it.
    #[test]
    fn digest_protocols_are_the_recorded_literals() {
        let fixture = fixture();
        assert_eq!(
            fixture["protocol"].as_str().unwrap(),
            "eidnara-canonical-json-contract-v1"
        );
        assert_eq!(
            DREAMER_REQUEST_DIGEST_PROTOCOL,
            "eidnara-dreamer-request-v1"
        );
        assert_eq!(
            fixture["dreamerRequestDigestProtocol"].as_str().unwrap(),
            DREAMER_REQUEST_DIGEST_PROTOCOL
        );
        assert_eq!(
            fixture["dreamerRequestEncodingVersion"].as_u64().unwrap(),
            u64::from(DREAMER_REQUEST_ENCODING_VERSION)
        );
    }

    #[test]
    fn canonical_bytes_match_fixture() {
        for case in fixture()["canonicalization"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let canonical = canonical_json_encode(&case["value"])
                .unwrap_or_else(|error| panic!("case {name}: {error}"));
            assert_eq!(
                canonical,
                case["canonical"].as_str().unwrap(),
                "case {name}"
            );
        }
    }

    /// The digest is the documented formula over the canonical bytes, computed
    /// here independently of `protocol_digest` and of `lower_hex`.
    #[test]
    fn dreamer_request_digest_is_sha256_over_protocol_and_canonical_bytes() {
        for case in fixture()["canonicalization"].as_array().unwrap() {
            let canonical = case["canonical"].as_str().unwrap();
            let mut hasher = Sha256::new();
            hasher.update(b"eidnara-dreamer-request-v1\n");
            hasher.update(canonical.as_bytes());
            let expected = format!("{:x}", hasher.finalize());
            assert_eq!(
                compute_dreamer_request_digest(&case["value"]).unwrap(),
                expected,
                "case {}",
                case["name"]
            );
        }
        // Integral floats and negative zero encode as the integer they equal.
        for (raw, canonical) in [("1e3", "1000"), ("1.0", "1"), ("-0.0", "0"), ("-0", "0")] {
            let value: Value = serde_json::from_str(raw).unwrap();
            assert_eq!(canonical_json_encode(&value).unwrap(), canonical, "{raw}");
        }
        // Key order does not change the digest; a value change does.
        let a = serde_json::json!({"b": 1, "a": [1, 2]});
        let b = serde_json::json!({"a": [1, 2], "b": 1});
        let c = serde_json::json!({"a": [2, 1], "b": 1});
        assert_eq!(
            compute_dreamer_request_digest(&a).unwrap(),
            compute_dreamer_request_digest(&b).unwrap()
        );
        assert_ne!(
            compute_dreamer_request_digest(&a).unwrap(),
            compute_dreamer_request_digest(&c).unwrap()
        );
    }

    #[test]
    fn non_canonical_numbers_are_rejected() {
        for case in fixture()["invalidCanonical"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let reason = case["reason"].as_str().unwrap();
            let value: Value = serde_json::from_str(case["valueJson"].as_str().unwrap()).unwrap();
            let error =
                canonical_json_encode(&value).expect_err(&format!("case {name} must be rejected"));
            assert!(
                matches!(&error, ContractError::NotCanonical(msg) if msg.contains(reason)),
                "case {name}: expected NotCanonical containing {reason:?}, got {error:?}"
            );
        }
    }

    #[test]
    fn integer_above_i64_max_is_not_canonical() {
        let value: Value = serde_json::from_str("18446744073709551615").unwrap();
        assert!(value.as_i64().is_none());
        assert!(value.as_u64().is_some());
        assert!(matches!(
            canonical_json_encode(&value),
            Err(ContractError::NotCanonical(_))
        ));
    }

    #[test]
    fn lower_hex_check_is_exact_in_length_and_alphabet() {
        assert!(is_lower_hex("00ff", 4));
        assert!(is_lower_hex(&"a".repeat(64), 64));
        assert!(!is_lower_hex("00fF", 4));
        assert!(!is_lower_hex("00ff", 3));
        assert!(!is_lower_hex("00fg", 4));
        assert!(is_lower_hex("", 0));
        assert!(!is_lower_hex("0 ff", 4));
    }
}
