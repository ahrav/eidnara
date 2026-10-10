use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::ffi::OsStrExt;

use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use super::aws_source::AwsProfileSource;
use super::subprocess::{CredentialRowError, EnvSnapshot, canonical_provider};

pub const SOURCE_CLAIM_DOMAIN: &str = "eidnara-model-execution-credential-v4";
pub const SOURCE_CLAIM_CANONICALIZATION: &str = "harness-provider-source/2";

pub(super) const PROFILE_PROVIDER: &str = "amazon-bedrock";

const PREIMAGE_CAPACITY: usize = 256;

pub enum ClaimSource<'a> {
    Env(&'a [(OsString, OsString)]),
    AwsProfile(&'a AwsProfileSource),
}

impl ClaimSource<'_> {
    fn kind(&self) -> &'static str {
        match self {
            Self::Env(_) => "env",
            Self::AwsProfile(_) => "aws_profile",
        }
    }
}

fn decimal(mut value: usize, digits: &mut [u8; 20]) -> &[u8] {
    let mut start = digits.len();
    loop {
        start -= 1;
        digits[start] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            return &digits[start..];
        }
    }
}

fn field(message: &mut Vec<u8>, bytes: &[u8]) {
    let mut digits = [0; 20];
    message.extend_from_slice(decimal(bytes.len(), &mut digits));
    message.push(b':');
    message.extend_from_slice(bytes);
}

pub fn source_claim_preimage(harness: &str, provider: &str, source: &ClaimSource<'_>) -> Vec<u8> {
    let mut message = Vec::with_capacity(PREIMAGE_CAPACITY);
    for part in [
        SOURCE_CLAIM_CANONICALIZATION,
        harness,
        provider,
        source.kind(),
    ] {
        field(&mut message, part.as_bytes());
    }
    match source {
        ClaimSource::Env(row) => {
            for (name, value) in *row {
                let value = value.as_os_str().as_bytes();
                let mut digits = [0; 20];
                field(&mut message, name.as_os_str().as_bytes());
                field(&mut message, decimal(value.len(), &mut digits));
                field(&mut message, value);
            }
        }
        ClaimSource::AwsProfile(profile) => {
            for part in [
                profile.profile(),
                profile.region(),
                profile.config_file(),
                profile.credentials_file(),
                profile.sso_cache_root(),
            ] {
                field(&mut message, part.as_bytes());
            }
        }
    }
    message
}

pub fn source_claim(connection_key: &[u8; 32], preimage: &[u8]) -> String {
    ClaimKey::new(connection_key).claim(preimage)
}

#[derive(Clone)]
pub(super) struct ClaimKey(Hmac<Sha256>);

impl ClaimKey {
    pub(super) fn new(connection_key: &[u8; 32]) -> Self {
        let mut derive =
            Hmac::<Sha256>::new_from_slice(connection_key).expect("HMAC accepts any key length");
        derive.update(SOURCE_CLAIM_DOMAIN.as_bytes());
        let derived = derive.finalize().into_bytes();
        Self(Hmac::<Sha256>::new_from_slice(&derived).expect("HMAC accepts any key length"))
    }

    pub(super) fn claim(&self, preimage: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut mac = self.0.clone();
        mac.update(preimage);
        let tag = mac.finalize().into_bytes();
        let mut claim = String::with_capacity(2 * tag.len());
        for byte in tag {
            claim.push(char::from(HEX[usize::from(byte >> 4)]));
            claim.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        claim
    }
}

impl EnvSnapshot {
    pub fn source_claim(
        &self,
        connection_key: &[u8; 32],
        harness: &str,
        provider: &str,
        aws_source: Option<&AwsProfileSource>,
    ) -> Result<String, CredentialRowError> {
        self.source_claim_under(
            &ClaimKey::new(connection_key),
            harness,
            provider,
            aws_source,
        )
    }

    pub(super) fn source_claim_under(
        &self,
        key: &ClaimKey,
        harness: &str,
        provider: &str,
        aws_source: Option<&AwsProfileSource>,
    ) -> Result<String, CredentialRowError> {
        let canonical = canonical_provider(harness, provider)?;
        let preimage = match aws_source {
            Some(profile) if canonical == PROFILE_PROVIDER => {
                source_claim_preimage(harness, canonical, &ClaimSource::AwsProfile(profile))
            }
            _ => {
                let row = self.provider_row(harness, canonical)?;
                source_claim_preimage(harness, canonical, &ClaimSource::Env(&row))
            }
        };
        Ok(key.claim(&preimage))
    }
}

pub fn presented_claim_matches(
    expected: &str,
    presented: &BTreeMap<String, String>,
    canonical_provider: &str,
) -> bool {
    presented
        .get(canonical_provider)
        .is_some_and(|actual| equal_in_constant_time(expected.as_bytes(), actual.as_bytes()))
}

/// For equal-length inputs, comparison time depends only on input length.
fn equal_in_constant_time(expected: &[u8], actual: &[u8]) -> bool {
    if expected.len() != actual.len() {
        return false;
    }
    let (expected_words, expected_tail) = expected.as_chunks::<8>();
    let (actual_words, actual_tail) = actual.as_chunks::<8>();
    let words = expected_words
        .iter()
        .zip(actual_words)
        .fold(0, |diff, (expected, actual)| {
            diff | (u64::from_ne_bytes(*expected) ^ u64::from_ne_bytes(*actual))
        });
    let diff = expected_tail
        .iter()
        .zip(actual_tail)
        .fold(words, |diff, (expected, actual)| {
            diff | u64::from(expected ^ actual)
        });
    diff.ct_eq(&0).into()
}

#[cfg(test)]
mod tests {
    use super::{ClaimKey, decimal, equal_in_constant_time, source_claim};

    #[test]
    fn a_reused_claim_key_matches_a_fresh_derivation_for_every_preimage() {
        let key = [0x5a; 32];
        let reused = ClaimKey::new(&key);
        for preimage in [&b""[..], b"2:pi", &[0xff; 300]] {
            assert_eq!(reused.claim(preimage), source_claim(&key, preimage));
        }
    }

    #[test]
    fn decimal_writes_every_length_in_base_ten() {
        for value in [0, 9, 10, 4_096, usize::MAX] {
            let mut digits = [0; 20];
            assert_eq!(decimal(value, &mut digits), value.to_string().as_bytes());
        }
    }

    #[test]
    fn the_word_comparison_finds_every_differing_byte_at_every_length() {
        for len in [0, 1, 7, 8, 9, 63, 64, 65] {
            let expected: Vec<u8> = (0..len).map(|i| b'a' + (i % 26) as u8).collect();
            assert!(equal_in_constant_time(&expected, &expected.clone()));
            for at in 0..len {
                for bit in 0..8 {
                    let mut actual = expected.clone();
                    actual[at] ^= 1 << bit;
                    assert!(
                        !equal_in_constant_time(&expected, &actual),
                        "len {len} at {at}"
                    );
                }
            }
            let mut longer = expected.clone();
            longer.push(b'a');
            assert!(!equal_in_constant_time(&expected, &longer));
            assert!(!equal_in_constant_time(&longer, &expected));
        }
    }
}
