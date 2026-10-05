//! The SigV4 signer against pinned vectors: every header-signing case of the aws-c-auth suite at
//! `3281f8692e6fd10562c4585a4dded5c16b322698`, session-token cases included, and a local Bedrock
//! case whose canonical request botocore produced. Each case checks the canonical request, the
//! string to sign, the signature, and the `Authorization` header.

use std::path::{Path, PathBuf};

use daemon::memory_reviewer::sigv4::{
    Request, RequestTime, Scope, authorization, canonical_uri, signature, string_to_sign,
    string_to_sign_of_digest,
};
use sha2::{Digest, Sha256};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sigv4")
}

/// One raw request: the request line's method and target, its headers with continuation lines
/// folded into one space-joined value, and its body.
struct Raw {
    method: String,
    path: String,
    query: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn parse(text: &str) -> Raw {
    let (head, body) = text.split_once("\n\n").unwrap_or((text, ""));
    let mut lines = head.lines();
    let line = lines.next().unwrap();
    let (method, rest) = line.split_once(' ').unwrap();
    let target = rest.strip_suffix(" HTTP/1.1").unwrap();
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in lines {
        if line.starts_with(char::is_whitespace) {
            let (_, value) = headers.last_mut().unwrap();
            value.push(' ');
            value.push_str(line.trim());
        } else {
            let (name, value) = line.split_once(':').unwrap();
            headers.push((name.to_string(), value.to_string()));
        }
    }
    Raw {
        method: method.to_string(),
        path: path.to_string(),
        query: query.to_string(),
        headers,
        body: body.as_bytes().to_vec(),
    }
}

/// RFC 3986 dot-segment removal with empty segments dropped, keeping a trailing slash.
fn normalize(path: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            segment => segments.push(segment),
        }
    }
    let mut out = format!("/{}", segments.join("/"));
    if path.ends_with('/') && out != "/" {
        out.push('/');
    }
    out
}

fn decode(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap();
            out.push(u8::from_str_radix(hex, 16).unwrap());
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    out
}

fn encode(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
                char::from(*byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

fn canonical_query(query: &str) -> String {
    let mut pairs: Vec<(String, String)> = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (encode(&decode(key)), encode(&decode(value)))
        })
        .collect();
    pairs.sort();
    pairs
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn read(case: &Path, file: &str) -> String {
    std::fs::read_to_string(case.join(file)).unwrap()
}

#[test]
fn every_pinned_aws_c_auth_header_case_signs_as_the_suite_expects() {
    let mut cases: Vec<PathBuf> = std::fs::read_dir(fixtures().join("aws-c-auth"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect();
    cases.sort();
    assert_eq!(cases.len(), 38, "the pinned suite has 38 v4 cases");
    for case in cases {
        let name = case.file_name().unwrap().to_string_lossy().to_string();
        let context: serde_json::Value =
            serde_json::from_str(&read(&case, "context.json")).unwrap();
        let raw = parse(&read(&case, "request.txt"));
        let time = RequestTime::at(
            chrono::DateTime::parse_from_rfc3339(context["timestamp"].as_str().unwrap())
                .unwrap()
                .timestamp_millis(),
        )
        .unwrap();
        let payload = format!("{:x}", Sha256::digest(&raw.body));
        let mut headers = raw.headers.clone();
        headers.push(("X-Amz-Date".to_string(), time.amz_date.clone()));
        if context["sign_body"].as_bool() == Some(true) {
            headers.push(("X-Amz-Content-Sha256".to_string(), payload.clone()));
        }
        let credentials = &context["credentials"];
        if let Some(token) = credentials["token"].as_str()
            && context["omit_session_token"].as_bool() != Some(true)
        {
            headers.push(("X-Amz-Security-Token".to_string(), token.to_string()));
        }
        let path = if context["normalize"].as_bool() == Some(true) {
            normalize(&raw.path)
        } else {
            raw.path.clone()
        };
        let uri = canonical_uri(&path);
        let query = canonical_query(&raw.query);
        let pairs: Vec<(&str, &str)> = headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        let request = Request {
            method: &raw.method,
            canonical_uri: &uri,
            canonical_query: &query,
            headers: &pairs,
            payload_sha256: &payload,
        };
        let (canonical, signed) = request.canonical();
        let (canonical_digest, digest_signed) = request.digest();
        assert_eq!(
            *canonical,
            read(&case, "header-canonical-request.txt"),
            "{name}"
        );
        let scope = Scope {
            date: time.date(),
            region: context["region"].as_str().unwrap(),
            service: context["service"].as_str().unwrap(),
        };
        let to_sign = string_to_sign(&time, &scope, &canonical);
        assert_eq!(to_sign, read(&case, "header-string-to-sign.txt"), "{name}");
        assert_eq!(
            string_to_sign_of_digest(&time, &scope, &canonical_digest),
            to_sign,
            "{name}: the digest path signs the same string"
        );
        assert_eq!(digest_signed, signed, "{name}");
        let secret = credentials["secret_access_key"].as_str().unwrap();
        let signed_value = signature(secret, &scope, &to_sign);
        assert_eq!(
            signed_value,
            read(&case, "header-signature.txt").trim(),
            "{name}"
        );
        let expected_authorization = read(&case, "header-signed-request.txt")
            .lines()
            .find_map(|line| line.strip_prefix("Authorization:").map(str::to_string))
            .unwrap();
        assert_eq!(
            authorization(
                credentials["access_key_id"].as_str().unwrap(),
                &scope,
                &signed,
                &signed_value
            ),
            expected_authorization,
            "{name}"
        );
    }
}

#[test]
fn a_bedrock_model_path_double_encodes_its_colon_as_botocore_does() {
    let case: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixtures().join("bedrock-colon.json")).unwrap(),
    )
    .unwrap();
    let text = |key: &str| case[key].as_str().unwrap().to_string();
    let time = RequestTime::at(
        chrono::DateTime::parse_from_rfc3339(&text("timestamp"))
            .unwrap()
            .timestamp_millis(),
    )
    .unwrap();
    let body = text("body");
    let payload = format!("{:x}", Sha256::digest(body.as_bytes()));
    let token = text("session_token");
    let host = text("host");
    let headers = [
        ("content-type", "application/json"),
        ("host", host.as_str()),
        ("x-amz-content-sha256", payload.as_str()),
        ("x-amz-date", time.amz_date.as_str()),
        ("x-amz-security-token", token.as_str()),
    ];
    let uri = canonical_uri(&text("wire_path"));
    assert!(uri.contains("v1%253A0"), "{uri}");
    let request = Request {
        method: "POST",
        canonical_uri: &uri,
        canonical_query: "",
        headers: &headers,
        payload_sha256: &payload,
    };
    let (canonical, signed) = request.canonical();
    let (canonical_digest, digest_signed) = request.digest();
    assert_eq!(*canonical, text("canonical_request"));
    let scope = Scope {
        date: time.date(),
        region: &text("region"),
        service: &text("service"),
    };
    let to_sign = string_to_sign(&time, &scope, &canonical);
    assert_eq!(to_sign, text("string_to_sign"));
    assert_eq!(
        string_to_sign_of_digest(&time, &scope, &canonical_digest),
        to_sign
    );
    assert_eq!(digest_signed, signed);
    let value = signature(&text("secret_access_key"), &scope, &to_sign);
    assert_eq!(value, text("signature"));
    assert_eq!(
        authorization(&text("access_key_id"), &scope, &signed, &value),
        text("authorization")
    );
}
