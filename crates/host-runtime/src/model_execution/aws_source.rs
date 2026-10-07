use std::collections::BTreeMap;
use std::fmt;

use serde::de::value::MapAccessDeserializer;
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use super::aws_profile::{MAX_PROFILE_NAME_BYTES, MAX_REGION_BYTES};
use super::subprocess::{CredentialMechanism, credential_variable_mechanism};

const MAX_PATH_BYTES: usize = 4096;
const SSO_CACHE_SUFFIX: &str = "/.aws/sso/cache";
const KIND_NAMES: &[&str] = &["profile"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AwsSourceKind {
    Profile,
}

/// `AwsSourceKind` decodes only from a string, so a map-form tag such as
/// `{"profile": null}` fails as a type error.
impl<'de> Deserialize<'de> for AwsSourceKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct KindName;

        impl Visitor<'_> for KindName {
            type Value = AwsSourceKind;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("the string \"profile\"")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<AwsSourceKind, E> {
                match value {
                    "profile" => Ok(AwsSourceKind::Profile),
                    other => Err(E::unknown_variant(other, KIND_NAMES)),
                }
            }
        }

        deserializer.deserialize_str(KindName)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAwsProfileSource {
    kind: AwsSourceKind,
    profile: String,
    region: String,
    config_file: String,
    credentials_file: String,
    sso_cache_root: String,
}

/// `RawAwsProfileObject` restricts the derived struct decoder to map input, which enforces
/// the object-shaped wire contract.
struct RawAwsProfileObject(RawAwsProfileSource);

impl<'de> Deserialize<'de> for RawAwsProfileObject {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ObjectOnly;

        impl<'de> Visitor<'de> for ObjectOnly {
            type Value = RawAwsProfileObject;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an aws profile source object")
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                RawAwsProfileSource::deserialize(MapAccessDeserializer::new(map))
                    .map(RawAwsProfileObject)
            }
        }

        deserializer.deserialize_map(ObjectOnly)
    }
}

/// Decoding validates every field, so each value of this type satisfies the selector bounds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawAwsProfileObject")]
pub struct AwsProfileSource {
    kind: AwsSourceKind,
    profile: String,
    region: String,
    config_file: String,
    credentials_file: String,
    sso_cache_root: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AwsSourceError {
    Empty(&'static str),
    TooLong(&'static str),
    ContainsNul(&'static str),
    PathNotNormal(&'static str),
    SsoCacheRoot,
    ConflictingCredential,
}

impl std::fmt::Display for AwsSourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty(field) => write!(f, "aws source {field} is empty"),
            Self::TooLong(field) => write!(f, "aws source {field} exceeds its byte bound"),
            Self::ContainsNul(field) => write!(f, "aws source {field} contains a NUL byte"),
            Self::PathNotNormal(field) => {
                write!(f, "aws source {field} is not absolute and lexically normal")
            }
            Self::SsoCacheRoot => {
                write!(f, "aws source sso_cache_root is not under .aws/sso/cache")
            }
            Self::ConflictingCredential => {
                write!(
                    f,
                    "aws profile source conflicts with a legacy AWS credential"
                )
            }
        }
    }
}

impl std::error::Error for AwsSourceError {}

/// Accepts `/` and absolute paths whose `/`-separated segments are all nonempty and other
/// than `.` and `..`.
fn is_lexically_normal(path: &str) -> bool {
    path == "/"
        || path
            .strip_prefix('/')
            .is_some_and(|rest| rest.split('/').all(|part| !matches!(part, "" | "." | "..")))
}

fn bounded(field: &'static str, value: &str, max: usize) -> Result<(), AwsSourceError> {
    if value.is_empty() {
        return Err(AwsSourceError::Empty(field));
    }
    if value.len() > max {
        return Err(AwsSourceError::TooLong(field));
    }
    if value.contains('\0') {
        return Err(AwsSourceError::ContainsNul(field));
    }
    Ok(())
}

fn normal_path(field: &'static str, value: &str) -> Result<(), AwsSourceError> {
    bounded(field, value, MAX_PATH_BYTES)?;
    if !is_lexically_normal(value) {
        return Err(AwsSourceError::PathNotNormal(field));
    }
    Ok(())
}

impl TryFrom<RawAwsProfileObject> for AwsProfileSource {
    type Error = AwsSourceError;

    fn try_from(RawAwsProfileObject(raw): RawAwsProfileObject) -> Result<Self, AwsSourceError> {
        bounded("profile", &raw.profile, MAX_PROFILE_NAME_BYTES)?;
        bounded("region", &raw.region, MAX_REGION_BYTES)?;
        normal_path("config_file", &raw.config_file)?;
        normal_path("credentials_file", &raw.credentials_file)?;
        normal_path("sso_cache_root", &raw.sso_cache_root)?;
        if !raw.sso_cache_root.ends_with(SSO_CACHE_SUFFIX) {
            return Err(AwsSourceError::SsoCacheRoot);
        }
        Ok(Self {
            kind: raw.kind,
            profile: raw.profile,
            region: raw.region,
            config_file: raw.config_file,
            credentials_file: raw.credentials_file,
            sso_cache_root: raw.sso_cache_root,
        })
    }
}

impl AwsProfileSource {
    pub fn profile(&self) -> &str {
        &self.profile
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    pub fn config_file(&self) -> &str {
        &self.config_file
    }

    pub fn credentials_file(&self) -> &str {
        &self.credentials_file
    }

    pub fn sso_cache_root(&self) -> &str {
        &self.sso_cache_root
    }
}

pub fn validate_source_binding(
    aws_source: Option<&AwsProfileSource>,
    credentials: &BTreeMap<String, String>,
) -> Result<(), AwsSourceError> {
    if aws_source.is_some() && credentials.keys().any(|name| is_static_aws(name)) {
        return Err(AwsSourceError::ConflictingCredential);
    }
    Ok(())
}

fn is_static_aws(name: &str) -> bool {
    credential_variable_mechanism(name) == Some(CredentialMechanism::StaticCredentials)
}

pub fn profile_mode_credentials(
    credentials: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    credentials
        .iter()
        .filter(|(name, _)| !is_static_aws(name))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

pub fn deserialize_present<'de, D>(deserializer: D) -> Result<Option<AwsProfileSource>, D::Error>
where
    D: Deserializer<'de>,
{
    AwsProfileSource::deserialize(deserializer).map(Some)
}
