//! AWS Signature Version 4 header signing for the MemoryReviewer's Bedrock requests, on the
//! workspace's HMAC and SHA-256 crates. The canonical request names the method, the canonical
//! URI, the canonical query, every signed header, and the payload digest; the string to sign
//! binds it to the request time and the credential scope; the signature is the HMAC of that
//! string under the key derived from the secret, the date, the region, and the service. Derived
//! keys are wiped when they drop, and the secret never leaves this module's arguments.

use std::borrow::Cow;

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
    /// Appends the signing scope, `<date>/<region>/<service>/aws4_request`, to `out`.
    fn push_credential_scope(&self, out: &mut String) {
        for part in [
            self.date,
            "/",
            self.region,
            "/",
            self.service,
            "/aws4_request",
        ] {
            out.push_str(part);
        }
    }

    fn credential_scope_len(&self) -> usize {
        self.date.len() + self.region.len() + self.service.len() + "///aws4_request".len()
    }
}

/// Appends the hex digits of `bytes` to `out`, two per byte, from `digits`.
fn push_hex(out: &mut String, bytes: &[u8], digits: &[u8; 16]) {
    for byte in bytes {
        out.push(char::from(digits[usize::from(byte >> 4)]));
        out.push(char::from(digits[usize::from(byte & 0x0f)]));
    }
}

const LOWER_HEX: &[u8; 16] = b"0123456789abcdef";
const UPPER_HEX: &[u8; 16] = b"0123456789ABCDEF";

/// A request time in the two forms SigV4 uses: `YYYYMMDDTHHMMSSZ` and its `YYYYMMDD` date.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestTime {
    pub amz_date: String,
}

impl RequestTime {
    /// The time `unix_ms` milliseconds after the epoch, in UTC; `None` outside chrono's range.
    pub fn at(unix_ms: i64) -> Option<Self> {
        use chrono::{Datelike, Timelike};
        let time = chrono::DateTime::from_timestamp_millis(unix_ms)?;
        let Ok(year @ 0..=9999) = u32::try_from(time.year()) else {
            // A year outside four digits carries chrono's sign and width.
            return Some(Self {
                amz_date: time.format("%Y%m%dT%H%M%SZ").to_string(),
            });
        };
        let push = |out: &mut String, value: u32, width: u32| {
            for place in (0..width).rev() {
                out.push(char::from(b'0' + (value / 10u32.pow(place) % 10) as u8));
            }
        };
        let mut amz_date = String::with_capacity(16);
        push(&mut amz_date, year, 4);
        push(&mut amz_date, time.month(), 2);
        push(&mut amz_date, time.day(), 2);
        amz_date.push('T');
        push(&mut amz_date, time.hour(), 2);
        push(&mut amz_date, time.minute(), 2);
        push(&mut amz_date, time.second(), 2);
        amz_date.push('Z');
        Some(Self { amz_date })
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
            out.push('%');
            push_hex(&mut out, &[byte], UPPER_HEX);
        }
    }
    out
}

/// Where canonical request text goes: a buffer that keeps it, or a digest that absorbs each piece as it is written.
trait Sink {
    fn put(&mut self, text: &str);
}

impl Sink for String {
    fn put(&mut self, text: &str) {
        self.push_str(text);
    }
}

impl Sink for Sha256 {
    fn put(&mut self, text: &str) {
        self.update(text.as_bytes());
    }
}

/// Appends `value` to `out`, trimming Unicode `White_Space` and collapsing each inner run to one space.
fn push_trimmed(out: &mut impl Sink, value: &str) {
    // Printable ASCII holds no whitespace, so such a value is already trimmed and collapsed; the fold reads every byte so the check vectorizes.
    if value.bytes().fold(true, |printable, byte| {
        printable & (0x21..=0x7e).contains(&byte)
    }) {
        out.put(value);
        return;
    }
    let mut push = |index: usize, word: &str| {
        if index > 0 {
            out.put(" ");
        }
        out.put(word);
    };
    // In ASCII text free of vertical tabs, `split_ascii_whitespace` splits on exactly the characters `split_whitespace` splits on, and it scans bytes.
    if value.is_ascii() && !value.as_bytes().contains(&0x0b) {
        value
            .split_ascii_whitespace()
            .enumerate()
            .for_each(|(index, word)| push(index, word));
    } else {
        value
            .split_whitespace()
            .enumerate()
            .for_each(|(index, word)| push(index, word));
    }
}

/// Writes the canonical header block for `headers` into `out` and returns the signed header list: names lowercased and sorted, values trimmed with inner runs of whitespace collapsed to one space, and the values of a repeated name joined with commas in their order.
fn write_canonical_headers(out: &mut impl Sink, headers: &[(&str, &str)]) -> String {
    let mut sorted: Vec<(Cow<'_, str>, &str)> = headers
        .iter()
        .map(|&(name, value)| {
            let name = if name.bytes().any(|byte| byte.is_ascii_uppercase()) {
                Cow::Owned(name.to_ascii_lowercase())
            } else {
                Cow::Borrowed(name)
            };
            (name, value)
        })
        .collect();
    // A stable sort keeps a repeated name's values in their order.
    sorted.sort_by(|(left, _), (right, _)| left.cmp(right));
    let mut signed = String::with_capacity(sorted.iter().map(|(name, _)| name.len() + 1).sum());
    let mut previous: Option<&str> = None;
    for (name, value) in &sorted {
        if previous == Some(name.as_ref()) {
            out.put(",");
        } else {
            if previous.is_some() {
                out.put("\n");
                signed.push(';');
            }
            out.put(name);
            out.put(":");
            signed.push_str(name);
        }
        push_trimmed(out, value);
        previous = Some(name);
    }
    if previous.is_some() {
        out.put("\n");
    }
    signed
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
    fn canonical_capacity(&self) -> usize {
        let headers: usize = self
            .headers
            .iter()
            .map(|(name, value)| 2 * name.len() + value.len() + 3)
            .sum();
        self.method.len()
            + self.canonical_uri.len()
            + self.canonical_query.len()
            + headers
            + self.payload_sha256.len()
            + 5
    }

    /// Writes the canonical request into `out` and returns its signed header list.
    fn write(&self, out: &mut impl Sink) -> String {
        for part in [self.method, self.canonical_uri, self.canonical_query] {
            out.put(part);
            out.put("\n");
        }
        let signed = write_canonical_headers(out, self.headers);
        out.put("\n");
        out.put(&signed);
        out.put("\n");
        out.put(self.payload_sha256);
        signed
    }

    /// The canonical request, whose buffer is wiped when it drops because it holds a session token's bytes, and its signed header list. The request's own header value keeps its copy of the token for the wire.
    pub fn canonical(&self) -> (Zeroizing<String>, String) {
        let mut canonical = Zeroizing::new(String::with_capacity(self.canonical_capacity()));
        let signed = self.write(&mut *canonical);
        (canonical, signed)
    }

    /// The SHA-256 of [`Self::canonical`]'s text and its signed header list. The digest absorbs each piece as it is written, so the session token's bytes stay in the caller's header value.
    pub fn digest(&self) -> ([u8; 32], String) {
        let mut hasher = Sha256::new();
        let signed = self.write(&mut hasher);
        (hasher.finalize().into(), signed)
    }
}

/// The string to sign for `canonical_request` at `time` within `scope`.
pub fn string_to_sign(time: &RequestTime, scope: &Scope<'_>, canonical_request: &str) -> String {
    string_to_sign_of_digest(
        time,
        scope,
        &Sha256::digest(canonical_request.as_bytes()).into(),
    )
}

/// The string to sign at `time` within `scope` for the canonical request whose SHA-256 is `canonical_digest`.
pub fn string_to_sign_of_digest(
    time: &RequestTime,
    scope: &Scope<'_>,
    canonical_digest: &[u8; 32],
) -> String {
    let head = [ALGORITHM, "\n", &time.amz_date, "\n"];
    let mut out = String::with_capacity(
        head.iter().map(|part| part.len()).sum::<usize>() + scope.credential_scope_len() + 65,
    );
    head.iter().for_each(|part| out.push_str(part));
    scope.push_credential_scope(&mut out);
    out.push('\n');
    push_hex(&mut out, canonical_digest, LOWER_HEX);
    out
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
    lower_hex(hmac(key.as_slice(), string_to_sign.as_bytes()).as_slice())
}

/// The lowercase hex digits of `bytes`, two per byte.
pub(crate) fn lower_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(2 * bytes.len());
    push_hex(&mut hex, bytes, LOWER_HEX);
    hex
}

/// The `Authorization` header value for a signature over `signed_headers`.
pub fn authorization(
    access_key_id: &str,
    scope: &Scope<'_>,
    signed_headers: &str,
    signature: &str,
) -> String {
    let head = [ALGORITHM, " Credential=", access_key_id, "/"];
    let tail = [
        ", SignedHeaders=",
        signed_headers,
        ", Signature=",
        signature,
    ];
    let mut out = String::with_capacity(
        head.iter()
            .chain(&tail)
            .map(|part| part.len())
            .sum::<usize>()
            + scope.credential_scope_len(),
    );
    head.iter().for_each(|part| out.push_str(part));
    scope.push_credential_scope(&mut out);
    tail.iter().for_each(|part| out.push_str(part));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical request as the SigV4 definition states it, built header by header.
    fn reference(request: &Request<'_>) -> (String, String) {
        let mut named: Vec<(String, String)> = Vec::new();
        for (name, value) in request.headers {
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
        let block: String = named
            .iter()
            .map(|(name, value)| format!("{name}:{value}\n"))
            .collect();
        let signed = named
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(";");
        (
            format!(
                "{}\n{}\n{}\n{block}\n{signed}\n{}",
                request.method,
                request.canonical_uri,
                request.canonical_query,
                request.payload_sha256
            ),
            signed,
        )
    }

    proptest::proptest! {
        /// Every header list, with repeated and mixed-case names and values carrying ASCII, vertical-tab, and Unicode whitespace, canonicalizes as the definition states, in the one buffer sized for it.
        #[test]
        fn the_canonical_request_matches_the_definition_in_one_buffer(
            headers in proptest::collection::vec(
                (
                    proptest::sample::select(vec!["host", "Host", "x-amz-date", "X-Amz-Date", "content-type", "my-Header"]),
                    "[ \t\n\x0B\x0C\r\u{a0}\u{2003}aZ,;:é]{0,12}",
                ),
                0..7,
            ),
        ) {
            let pairs: Vec<(&str, &str)> = headers.iter().map(|(name, value)| (*name, value.as_str())).collect();
            let request = Request {
                method: "POST",
                canonical_uri: "/model/m%253A0/invoke",
                canonical_query: "",
                headers: &pairs,
                payload_sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            };
            let (canonical, signed) = request.canonical();
            let (expected, expected_signed) = reference(&request);
            proptest::prop_assert_eq!(canonical.as_str(), expected.as_str());
            proptest::prop_assert_eq!(&signed, &expected_signed);
            proptest::prop_assert_eq!(canonical.capacity(), request.canonical_capacity());
            let (digest, digest_signed) = request.digest();
            proptest::prop_assert_eq!(digest, <[u8; 32]>::from(Sha256::digest(canonical.as_bytes())));
            proptest::prop_assert_eq!(digest_signed, signed);
        }
    }

    proptest::proptest! {
        /// Every representable request time renders as chrono's `%Y%m%dT%H%M%SZ`, and the scope and header strings are sized exactly.
        #[test]
        fn request_times_and_signing_strings_match_their_formatting_definitions(
            unix_ms in proptest::prop_oneof![
                chrono::DateTime::<chrono::Utc>::MIN_UTC.timestamp_millis()..=chrono::DateTime::<chrono::Utc>::MAX_UTC.timestamp_millis(),
                -62_167_219_200_000i64..=253_402_300_799_999,
            ],
            region in "[a-z0-9-]{1,20}",
        ) {
            let time = RequestTime::at(unix_ms).unwrap();
            let expected = chrono::DateTime::from_timestamp_millis(unix_ms)
                .unwrap()
                .format("%Y%m%dT%H%M%SZ")
                .to_string();
            proptest::prop_assert_eq!(&time.amz_date, &expected);
            if time.amz_date.len() == 16 {
                let scope = Scope { date: time.date(), region: &region, service: "bedrock" };
                let to_sign = string_to_sign_of_digest(&time, &scope, &[0xab; 32]);
                let expected_scope = format!("{}/{region}/bedrock/aws4_request", time.date());
                proptest::prop_assert_eq!(
                    &to_sign,
                    &format!("{ALGORITHM}\n{}\n{expected_scope}\n{}", time.amz_date, "ab".repeat(32))
                );
                proptest::prop_assert_eq!(to_sign.capacity(), to_sign.len());
                let header = authorization("AKID", &scope, "host;x-amz-date", "f00d");
                proptest::prop_assert_eq!(
                    &header,
                    &format!("{ALGORITHM} Credential=AKID/{expected_scope}, SignedHeaders=host;x-amz-date, Signature=f00d")
                );
                proptest::prop_assert_eq!(header.capacity(), header.len());
            }
        }
    }

    #[test]
    fn hex_and_uri_encoding_match_their_formatting_definitions() {
        let bytes: Vec<u8> = (0..=255).collect();
        let mut hex = String::new();
        push_hex(&mut hex, &bytes, LOWER_HEX);
        assert_eq!(
            hex,
            bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        let path: String = (1..=127u8).map(char::from).chain("é/".chars()).collect();
        let expected: String = path
            .bytes()
            .map(|byte| {
                if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/')
                {
                    char::from(byte).to_string()
                } else {
                    format!("%{byte:02X}")
                }
            })
            .collect();
        assert_eq!(canonical_uri(&path), expected);
    }
}
