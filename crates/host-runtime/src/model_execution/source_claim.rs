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

fn field(message: &mut Vec<u8>, bytes: &[u8]) {
    message.extend_from_slice(bytes.len().to_string().as_bytes());
    message.push(b':');
    message.extend_from_slice(bytes);
}

pub fn source_claim_preimage(harness: &str, provider: &str, source: &ClaimSource<'_>) -> Vec<u8> {
    let mut message = Vec::new();
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
                field(&mut message, name.as_os_str().as_bytes());
                field(&mut message, value.len().to_string().as_bytes());
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
    let mut derive =
        Hmac::<Sha256>::new_from_slice(connection_key).expect("HMAC accepts any key length");
    derive.update(SOURCE_CLAIM_DOMAIN.as_bytes());
    let derived = derive.finalize().into_bytes();
    let mut mac = Hmac::<Sha256>::new_from_slice(&derived).expect("HMAC accepts any key length");
    mac.update(preimage);
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

impl EnvSnapshot {
    pub fn source_claim(
        &self,
        connection_key: &[u8; 32],
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
        Ok(source_claim(connection_key, &preimage))
    }
}

pub fn presented_claim_matches(
    expected: &str,
    presented: &BTreeMap<String, String>,
    canonical_provider: &str,
) -> bool {
    presented
        .get(canonical_provider)
        .is_some_and(|actual| expected.as_bytes().ct_eq(actual.as_bytes()).into())
}
