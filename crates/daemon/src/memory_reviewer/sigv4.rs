//! AWS Signature Version 4 header signing for the MemoryReviewer's Bedrock requests, on the
//! workspace's HMAC and SHA-256 crates. The canonical request names the method, the canonical
//! URI, the canonical query, every signed header, and the payload digest; the string to sign
//! binds it to the request time and the credential scope; the signature is the HMAC of that
//! string under the key derived from the secret, the date, the region, and the service. Derived
//! keys are wiped when they drop, and the secret never leaves this module's arguments.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

pub const ALGORITHM: &str = "AWS4-HMAC-SHA256";

/// The scope a signature is valid for: the date of the request time, the region, and the service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scope<'a> {
    /// `YYYYMMDD`, the date of [`RequestTime`].
    pub date: &'a str,
    pub region: &'a str,
    pub service: &'a str,
}

impl Scope<'_> {
    fn credential_scope(&self) -> String {
        format!(
            "{}/{}/{}/aws4_request",
            self.date, self.region, self.service
        )
    }
}

/// A request time in the two forms SigV4 uses: `YYYYMMDDTHHMMSSZ` and its `YYYYMMDD` date.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestTime {
    pub amz_date: String,
}

impl RequestTime {
    /// The time `unix_ms` milliseconds after the epoch, in UTC; `None` outside chrono's range.
    pub fn at(unix_ms: i64) -> Option<Self> {
        let time = chrono::DateTime::from_timestamp_millis(unix_ms)?;
        Some(Self {
            amz_date: time.format("%Y%m%dT%H%M%SZ").to_string(),
        })
    }

    pub fn date(&self) -> &str {
        &self.amz_date[..8]
    }
}

/// `path` with every byte outside the unreserved set and `/` percent-encoded, as SigV4 encodes
/// a non-S3 path: a path already encoded for the wire is encoded again, so `%3A` becomes
/// `%253A`.
pub fn canonical_uri(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The canonical header block and the signed header list for `headers`: names lowercased and
/// sorted, values trimmed with inner runs of whitespace collapsed to one space, and the values
/// of a repeated name joined with commas in their order.
pub fn canonical_headers(headers: &[(&str, &str)]) -> (String, String) {
    let mut named: Vec<(String, String)> = Vec::with_capacity(headers.len());
    for (name, value) in headers {
        let name = name.to_ascii_lowercase();
        let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
        match named.iter_mut().find(|(existing, _)| *existing == name) {
            Some((_, joined)) => {
                joined.push(',');
                joined.push_str(&value);
            }
            None => named.push((name, value)),
        }
    }
    named.sort_by(|(left, _), (right, _)| left.cmp(right));
    let block = named
        .iter()
        .map(|(name, value)| format!("{name}:{value}\n"))
        .collect();
    let signed = named
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(";");
    (block, signed)
}

/// One request to sign: its method, canonical URI and query, headers, and payload digest.
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    pub method: &'a str,
    pub canonical_uri: &'a str,
    pub canonical_query: &'a str,
    pub headers: &'a [(&'a str, &'a str)],
    /// Lowercase hex SHA-256 of the body.
    pub payload_sha256: &'a str,
}

impl Request<'_> {
    /// The canonical request, whose final buffer is wiped when it drops because it holds a session token's bytes, and its signed header list. The request's own header value keeps its copy of the token for the wire.
    pub fn canonical(&self) -> (Zeroizing<String>, String) {
        let (block, signed) = canonical_headers(self.headers);
        let block = Zeroizing::new(block);
        (
            Zeroizing::new(format!(
                "{}\n{}\n{}\n{}\n{signed}\n{}",
                self.method,
                self.canonical_uri,
                self.canonical_query,
                block.as_str(),
                self.payload_sha256
            )),
            signed,
        )
    }
}

/// The string to sign for `canonical_request` at `time` within `scope`.
pub fn string_to_sign(time: &RequestTime, scope: &Scope<'_>, canonical_request: &str) -> String {
    format!(
        "{ALGORITHM}\n{}\n{}\n{:x}",
        time.amz_date,
        scope.credential_scope(),
        Sha256::digest(canonical_request.as_bytes())
    )
}

fn hmac(key: &[u8], data: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    Zeroizing::new(mac.finalize().into_bytes().into())
}

/// The signing key for `scope`, derived from `secret_access_key`; every intermediate key is
/// wiped when it drops.
fn signing_key(secret_access_key: &str, scope: &Scope<'_>) -> Zeroizing<[u8; 32]> {
    let mut seed = Zeroizing::new(String::with_capacity(4 + secret_access_key.len()));
    seed.push_str("AWS4");
    seed.push_str(secret_access_key);
    let date = hmac(seed.as_bytes(), scope.date.as_bytes());
    let region = hmac(date.as_slice(), scope.region.as_bytes());
    let service = hmac(region.as_slice(), scope.service.as_bytes());
    hmac(service.as_slice(), b"aws4_request")
}

/// The lowercase hex signature of `string_to_sign` under the key `secret_access_key` derives
/// for `scope`.
pub fn signature(secret_access_key: &str, scope: &Scope<'_>, string_to_sign: &str) -> String {
    let key = signing_key(secret_access_key, scope);
    let signature = hmac(key.as_slice(), string_to_sign.as_bytes());
    signature.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The `Authorization` header value for a signature over `signed_headers`.
pub fn authorization(
    access_key_id: &str,
    scope: &Scope<'_>,
    signed_headers: &str,
    signature: &str,
) -> String {
    format!(
        "{ALGORITHM} Credential={access_key_id}/{}, SignedHeaders={signed_headers}, Signature={signature}",
        scope.credential_scope()
    )
}
