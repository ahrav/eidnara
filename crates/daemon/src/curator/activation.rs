//! The deployment owner's activation record for Curator disclosure: `runtime-identity.json` under `<home>/curator-activation/`, an owner-only JSON record read anew on every gate refresh. It binds the running binary's identity tuple (model, question template revision, step schema version, scanner ruleset, egress policy version, both stores' baseline digests and incarnations, provider identity, credential name) to the owner's provider-retention attestation and finite-work-exposure acknowledgement. A missing or refused record closes the gate; a record naming another identity denies with the field that differs; only a matching, attested record yields the startup approval the sender requires before any byte is disclosed. Restore or a new incarnation changes the pair the record names, so it needs a new record.

use std::path::Path;

use context_core::curator_policy_union::CURATOR_POLICY_UNION_VERSION;
use serde::Deserialize;

use crate::projection_lifecycle::{RecordRead, read_owner_only_record};

use super::broker::QuestionTemplate;
use super::disclosure::DisclosureApproval;
use super::steps::STEP_VERSION;

pub const ACTIVATION_DIR: &str = "curator-activation";
pub const IDENTITY_RECORD: &str = "runtime-identity.json";
pub const IDENTITY_SCHEMA: u32 = 1;
/// Longest attestation field kept; the record is the owner's, so the surface only needs to identify it.
const MAX_ATTESTATION_FIELD_BYTES: usize = 256;

/// The owner's record as written.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeIdentityRecord {
    pub schema: u32,
    /// The canonical model id every Curator request names; the provider's reported model is checked against it separately at response time.
    pub model: String,
    pub prompt_template_version: String,
    pub step_schema_version: u32,
    pub scanner_ruleset_version: String,
    pub egress_policy_version: u32,
    pub kernel_baseline_digest: String,
    pub memstore_baseline_digest: String,
    pub kernel_incarnation: String,
    pub memstore_incarnation: String,
    /// `<host>/v1/messages@<version>`, as the sender identifies itself.
    pub provider: String,
    /// The startup-envelope credential name the sender dials with.
    pub credential: String,
    /// The fingerprint of that credential's secret as [`credential_fingerprint`](super::worker::credential_fingerprint) derives it: the attestation is about one provider account, so a rotated secret closes the gate until the owner re-attests.
    pub credential_fingerprint: String,
    pub provider_retention: ProviderRetention,
}

/// The owner's statement about the provider account: an input the deployment owner vouches for, never an inference.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderRetention {
    pub attested_by: String,
    pub attested_on: String,
    pub retention_terms: String,
    pub finite_work_exposure_acknowledged: bool,
}

/// What the running daemon is, for the record to match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveIdentity {
    pub kernel_baseline_digest: String,
    pub memstore_baseline_digest: String,
    pub kernel_incarnation: String,
    pub memstore_incarnation: String,
    pub provider: String,
    /// Credential names the startup envelope carries, each with its fingerprint.
    pub credentials: Vec<(String, String)>,
}

impl LiveIdentity {
    pub fn prompt_template_version() -> String {
        QuestionTemplate::ExtractedFacts.revision()
    }

    pub fn scanner_ruleset_version() -> String {
        context_core::redaction::detector_revision()
    }
}

/// The approval a matching record yields, with what the sender needs beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activation {
    pub approval: DisclosureApproval,
    pub credential: String,
}

/// Why the gate is closed. Every variant is bounded and names no record content beyond a field name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Closed {
    #[error("no activation record")]
    Missing,
    #[error("activation record refused: {0}")]
    Refused(&'static str),
    #[error("activation record unreadable: {0}")]
    Unreadable(String),
    #[error("activation record is not a record of its schema")]
    Malformed,
    #[error("activation record names another {0}")]
    IdentityMismatch(&'static str),
    #[error("activation record does not acknowledge finite work exposure")]
    Unacknowledged,
    #[error("activation record names a credential the startup envelope does not carry")]
    UnknownCredential,
}

/// Reads the record under `home` and evaluates it against `live`.
pub fn read_gate(home: &Path, live: &LiveIdentity) -> Result<Activation, Closed> {
    let bytes = match read_owner_only_record(&home.join(ACTIVATION_DIR), IDENTITY_RECORD) {
        Ok(RecordRead::Bytes(bytes)) => bytes,
        Ok(RecordRead::Absent) => return Err(Closed::Missing),
        Ok(RecordRead::Refused(reason)) => return Err(Closed::Refused(reason)),
        Err(error) => return Err(Closed::Unreadable(error.0)),
    };
    let record: RuntimeIdentityRecord =
        serde_json::from_slice(&bytes).map_err(|_| Closed::Malformed)?;
    evaluate(&record, live)
}

/// The identity terms a record must match, each with the live deployment's value, ordered as `evaluate` compares them.
pub fn identity_terms(live: &LiveIdentity) -> [(&'static str, String); 9] {
    [
        (
            "prompt template version",
            LiveIdentity::prompt_template_version(),
        ),
        ("step schema version", STEP_VERSION.to_string()),
        (
            "scanner ruleset version",
            LiveIdentity::scanner_ruleset_version(),
        ),
        (
            "egress policy version",
            CURATOR_POLICY_UNION_VERSION.to_string(),
        ),
        ("kernel baseline", live.kernel_baseline_digest.clone()),
        (
            "memory store baseline",
            live.memstore_baseline_digest.clone(),
        ),
        ("kernel incarnation", live.kernel_incarnation.clone()),
        (
            "memory store incarnation",
            live.memstore_incarnation.clone(),
        ),
        ("provider", live.provider.clone()),
    ]
}

/// The record's value for each term of [`identity_terms`], in the same order.
fn recorded_terms(record: &RuntimeIdentityRecord) -> [String; 9] {
    [
        record.prompt_template_version.clone(),
        record.step_schema_version.to_string(),
        record.scanner_ruleset_version.clone(),
        record.egress_policy_version.to_string(),
        record.kernel_baseline_digest.clone(),
        record.memstore_baseline_digest.clone(),
        record.kernel_incarnation.clone(),
        record.memstore_incarnation.clone(),
        record.provider.clone(),
    ]
}

/// The match every activation requires: schema, every identity term, an attestation with bounded fields, and a credential the envelope carries.
pub fn evaluate(record: &RuntimeIdentityRecord, live: &LiveIdentity) -> Result<Activation, Closed> {
    if record.schema != IDENTITY_SCHEMA {
        return Err(Closed::Malformed);
    }
    let attestation = &record.provider_retention;
    if attestation.attested_by.is_empty()
        || attestation.attested_by.len() > MAX_ATTESTATION_FIELD_BYTES
        || attestation.attested_on.is_empty()
        || attestation.attested_on.len() > MAX_ATTESTATION_FIELD_BYTES
        || attestation.retention_terms.is_empty()
        || attestation.retention_terms.len() > MAX_ATTESTATION_FIELD_BYTES
        || record.model.is_empty()
        || record.model.len() > MAX_ATTESTATION_FIELD_BYTES
    {
        return Err(Closed::Malformed);
    }
    if let Some(((field, _), _)) = identity_terms(live)
        .iter()
        .zip(recorded_terms(record).iter())
        .find(|((_, live_value), recorded)| live_value != *recorded)
    {
        return Err(Closed::IdentityMismatch(field));
    }
    if !attestation.finite_work_exposure_acknowledged {
        return Err(Closed::Unacknowledged);
    }
    let credential_id = live
        .credentials
        .iter()
        .find(|(name, _)| *name == record.credential)
        .map(|(_, fingerprint)| fingerprint.clone())
        .ok_or(Closed::UnknownCredential)?;
    if credential_id != record.credential_fingerprint {
        return Err(Closed::IdentityMismatch("credential"));
    }
    Ok(Activation {
        approval: DisclosureApproval {
            provider: record.provider.clone(),
            model: record.model.clone(),
            credential_id,
            kernel_incarnation: record.kernel_incarnation.clone(),
            memstore_incarnation: record.memstore_incarnation.clone(),
        },
        credential: record.credential.clone(),
    })
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn live() -> LiveIdentity {
        LiveIdentity {
            kernel_baseline_digest: "k".repeat(64),
            memstore_baseline_digest: "m".repeat(64),
            kernel_incarnation: "kernel-1".into(),
            memstore_incarnation: "memstore-1".into(),
            provider: "api.anthropic.com/v1/messages@2023-06-01".into(),
            credentials: vec![("ANTHROPIC_API_KEY".into(), "fp-1".into())],
        }
    }

    fn record() -> serde_json::Value {
        serde_json::json!({
            "schema": IDENTITY_SCHEMA,
            "model": "claude-x",
            "prompt_template_version": LiveIdentity::prompt_template_version(),
            "step_schema_version": STEP_VERSION,
            "scanner_ruleset_version": LiveIdentity::scanner_ruleset_version(),
            "egress_policy_version": CURATOR_POLICY_UNION_VERSION,
            "kernel_baseline_digest": "k".repeat(64),
            "memstore_baseline_digest": "m".repeat(64),
            "kernel_incarnation": "kernel-1",
            "memstore_incarnation": "memstore-1",
            "provider": "api.anthropic.com/v1/messages@2023-06-01",
            "credential": "ANTHROPIC_API_KEY",
            "credential_fingerprint": "fp-1",
            "provider_retention": {
                "attested_by": "owner",
                "attested_on": "2026-09-18",
                "retention_terms": "zero data retention addendum on file",
                "finite_work_exposure_acknowledged": true
            }
        })
    }

    fn write(home: &Path, value: &serde_json::Value, mode: u32) {
        let dir = home.join(ACTIVATION_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.join(IDENTITY_RECORD);
        std::fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    /// A matching, attested record yields the approval bound to the live pair and the named credential's fingerprint; each identity term that differs closes the gate naming that term; a missing acknowledgement, an unknown credential, a group-readable record, and an absent record close it too.
    #[test]
    fn the_gate_opens_only_for_a_matching_attested_owner_only_record() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(read_gate(home.path(), &live()), Err(Closed::Missing));
        write(home.path(), &record(), 0o600);
        let activation = read_gate(home.path(), &live()).unwrap();
        assert_eq!(
            activation.approval,
            DisclosureApproval {
                provider: "api.anthropic.com/v1/messages@2023-06-01".into(),
                model: "claude-x".into(),
                credential_id: "fp-1".into(),
                kernel_incarnation: "kernel-1".into(),
                memstore_incarnation: "memstore-1".into(),
            }
        );
        assert_eq!(activation.credential, "ANTHROPIC_API_KEY");

        for (field, value, expected) in [
            (
                "prompt_template_version",
                serde_json::json!("other"),
                Closed::IdentityMismatch("prompt template version"),
            ),
            (
                "step_schema_version",
                serde_json::json!(STEP_VERSION + 1),
                Closed::IdentityMismatch("step schema version"),
            ),
            (
                "scanner_ruleset_version",
                serde_json::json!("other"),
                Closed::IdentityMismatch("scanner ruleset version"),
            ),
            (
                "egress_policy_version",
                serde_json::json!(CURATOR_POLICY_UNION_VERSION + 1),
                Closed::IdentityMismatch("egress policy version"),
            ),
            (
                "kernel_baseline_digest",
                serde_json::json!("x".repeat(64)),
                Closed::IdentityMismatch("kernel baseline"),
            ),
            (
                "memstore_baseline_digest",
                serde_json::json!("x".repeat(64)),
                Closed::IdentityMismatch("memory store baseline"),
            ),
            (
                "kernel_incarnation",
                serde_json::json!("kernel-2"),
                Closed::IdentityMismatch("kernel incarnation"),
            ),
            (
                "memstore_incarnation",
                serde_json::json!("memstore-2"),
                Closed::IdentityMismatch("memory store incarnation"),
            ),
            (
                "provider",
                serde_json::json!("other.example/v1/messages@2023-06-01"),
                Closed::IdentityMismatch("provider"),
            ),
            (
                "credential",
                serde_json::json!("OPENAI_API_KEY"),
                Closed::UnknownCredential,
            ),
            (
                "credential_fingerprint",
                serde_json::json!("fp-2"),
                Closed::IdentityMismatch("credential"),
            ),
            ("schema", serde_json::json!(2), Closed::Malformed),
        ] {
            let mut changed = record();
            changed[field] = value;
            write(home.path(), &changed, 0o600);
            assert_eq!(
                read_gate(home.path(), &live()),
                Err(expected.clone()),
                "{field}"
            );
            // Every identity term the gate can name carries the live value the owner has to write; the credential fingerprint is derived from the secret and is not a term.
            if let Closed::IdentityMismatch(term) = expected {
                let live_value = identity_terms(&live())
                    .into_iter()
                    .find(|(name, _)| *name == term)
                    .map(|(_, value)| value);
                if term == "credential" {
                    assert_eq!(live_value, None, "{field}");
                } else {
                    let expected_value = record()[field].clone();
                    let expected_value = expected_value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| expected_value.to_string());
                    assert_eq!(live_value, Some(expected_value), "{field}");
                }
            }
        }
        let mut unacknowledged = record();
        unacknowledged["provider_retention"]["finite_work_exposure_acknowledged"] =
            serde_json::json!(false);
        write(home.path(), &unacknowledged, 0o600);
        assert_eq!(read_gate(home.path(), &live()), Err(Closed::Unacknowledged));
        let mut extra = record();
        extra["billing_cap_usd"] = serde_json::json!(100);
        write(home.path(), &extra, 0o600);
        assert_eq!(
            read_gate(home.path(), &live()),
            Err(Closed::Malformed),
            "unknown fields are refused, not ignored"
        );
        // A record another user could read is not the owner's private record.
        write(home.path(), &record(), 0o644);
        assert!(matches!(
            read_gate(home.path(), &live()),
            Err(Closed::Refused(_))
        ));
        // The record is read anew: restoring it reopens the gate without a restart.
        write(home.path(), &record(), 0o600);
        assert!(read_gate(home.path(), &live()).is_ok());
    }
}
