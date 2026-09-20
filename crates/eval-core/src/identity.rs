use std::collections::BTreeSet;
use std::fmt;

use context_core::canonical_json::{ContractError, is_lower_hex, protocol_digest};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const RUN_ID_PROTOCOL: &str = "eval-run-id/v1";
pub const BUILD_PROTOCOL: &str = "eval-build/v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BinaryDigest {
    Present { sha256: String },
    Absent { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildRecord {
    pub code_sha: String,
    pub dirty: bool,
    pub lockfile_digest: String,
    pub rustc_version: String,
    pub features: BTreeSet<String>,
    pub target_triple: String,
    pub binary_digest: BinaryDigest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunIdentity {
    pub build: BuildRecord,
    pub simulator_version: String,
    pub config: Value,
    pub scenario: Value,
    /// Serialized as a canonical decimal string: canonical JSON rejects integers above 2^53 - 1.
    #[serde(with = "decimal_u64")]
    pub root_seed: u64,
    pub random_schema_version: String,
    pub generator_version: String,
    pub eligibility_spec_digest: String,
    pub linearization_rule_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityError {
    MalformedDigest {
        field: &'static str,
    },
    EmptyComponent {
        field: &'static str,
    },
    /// The SHA-256 of zero bytes names no build.
    ZeroBytesBinaryDigest,
    NotCanonical(ContractError),
}

/// Variant names and fields are the message; callers match on the variant.
impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

impl std::error::Error for IdentityError {}

impl From<ContractError> for IdentityError {
    fn from(error: ContractError) -> Self {
        Self::NotCanonical(error)
    }
}

pub fn zero_bytes_sha256() -> String {
    format!("{:x}", Sha256::digest(b""))
}

fn require_hex(field: &'static str, text: &str, len: usize) -> Result<(), IdentityError> {
    if is_lower_hex(text, len) {
        Ok(())
    } else {
        Err(IdentityError::MalformedDigest { field })
    }
}

fn require_non_empty(field: &'static str, text: &str) -> Result<(), IdentityError> {
    if text.is_empty() {
        Err(IdentityError::EmptyComponent { field })
    } else {
        Ok(())
    }
}

impl BuildRecord {
    pub fn validate(&self) -> Result<(), IdentityError> {
        require_hex("code_sha", &self.code_sha, 40)?;
        require_hex("lockfile_digest", &self.lockfile_digest, 64)?;
        require_non_empty("rustc_version", &self.rustc_version)?;
        require_non_empty("target_triple", &self.target_triple)?;
        if let BinaryDigest::Present { sha256 } = &self.binary_digest {
            require_hex("binary_digest", sha256, 64)?;
            if *sha256 == zero_bytes_sha256() {
                return Err(IdentityError::ZeroBytesBinaryDigest);
            }
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<String, IdentityError> {
        self.validate()?;
        Ok(protocol_digest(
            BUILD_PROTOCOL,
            &serde_json::to_value(self).expect("build record serializes"),
        )?)
    }
}

impl RunIdentity {
    pub fn validate(&self) -> Result<(), IdentityError> {
        self.build.validate()?;
        require_non_empty("simulator_version", &self.simulator_version)?;
        require_non_empty("random_schema_version", &self.random_schema_version)?;
        require_non_empty("generator_version", &self.generator_version)?;
        require_hex("eligibility_spec_digest", &self.eligibility_spec_digest, 64)?;
        require_non_empty(
            "linearization_rule_version",
            &self.linearization_rule_version,
        )
    }
}

/// Hashes the nine-component tuple with `build` replaced by its own digest.
pub fn eval_run_id(identity: &RunIdentity) -> Result<String, IdentityError> {
    identity.validate()?;
    let mut tuple = serde_json::to_value(identity).expect("run identity serializes");
    tuple["build"] = Value::String(identity.build.digest()?);
    Ok(protocol_digest(RUN_ID_PROTOCOL, &tuple)?)
}

mod decimal_u64 {
    use super::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.to_string())
    }

    /// Accepts only the string `u64::to_string` produces, so one value has one encoding.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        let text = String::deserialize(deserializer)?;
        let value: u64 = text.parse().map_err(serde::de::Error::custom)?;
        if value.to_string() != text {
            return Err(serde::de::Error::custom(format!(
                "root_seed {text:?} is not the canonical decimal form"
            )));
        }
        Ok(value)
    }
}
