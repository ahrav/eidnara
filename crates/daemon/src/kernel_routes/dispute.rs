//! The evidence a `kernel.commit` dispute cites and the structured `reason`
//! the route stores for it; the grammar is specified in the `commit` module.

use context_core::redaction::redact_durable_text;
use kernel::{EventKind, TaintClass, is_lower_hex_oid};
use serde::Deserialize;

use super::InvalidReason;
use crate::confined_path;

/// The longest repository path an `evidence_ref` may name.
pub const MAX_EVIDENCE_PATH_BYTES: usize = 1024;
/// The longest structured reason the route stores.
pub const MAX_REASON_BYTES: usize = 4096;
/// The grammar version the first field names.
const VERSION: &str = "1";
/// The route the second field names.
const OPERATION: &str = "kernel.commit";

/// A repository file at a git object id, as the wire carries it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRef {
    pub path: String,
    pub git_object_id: String,
}

/// The typed content of a route-written dispute's `reason`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisputeReason {
    /// `MarkDisputed` or `Contradict`.
    pub event: EventKind,
    /// The reporter's taint as the route derived it from `source_kind`.
    pub reporter_taint: TaintClass,
    pub evidence_id: Option<String>,
    pub evidence_ref: Option<EvidenceRef>,
}

/// Whether `event` is a dispute, the two events that must cite evidence.
pub(crate) const fn is_dispute(event: EventKind) -> bool {
    matches!(event, EventKind::MarkDisputed | EventKind::Contradict)
}

impl DisputeReason {
    /// The validated reason for a dispute, or `InvalidInput` when the
    /// operation cites no evidence, cites an empty or malformed value, or
    /// renders to a reason over [`MAX_REASON_BYTES`] or holding a
    /// secret-scanner detection.
    pub(crate) fn validated(
        event: EventKind,
        reporter_taint: TaintClass,
        evidence_id: Option<&str>,
        evidence_ref: Option<&EvidenceRef>,
    ) -> Result<String, InvalidReason> {
        if !is_dispute(event) || (evidence_id.is_none() && evidence_ref.is_none()) {
            return Err(InvalidReason::InvalidInput);
        }
        if let Some(evidence_id) = evidence_id
            && (evidence_id.is_empty()
                // The length check bounds the input to `decodes_to_itself` and
                // `redact_durable_text` to `MAX_REASON_BYTES`.
                || evidence_id.len() > MAX_REASON_BYTES
                || !decodes_to_itself(evidence_id)
                || !redact_durable_text(evidence_id).detections.is_empty())
        {
            return Err(InvalidReason::InvalidInput);
        }
        if let Some(reference) = evidence_ref
            && !reference.is_well_formed()
        {
            return Err(InvalidReason::InvalidInput);
        }
        let reason = Self {
            event,
            reporter_taint,
            evidence_id: evidence_id.map(str::to_string),
            evidence_ref: evidence_ref.cloned(),
        }
        .render();
        if reason.len() > MAX_REASON_BYTES || !redact_durable_text(&reason).detections.is_empty() {
            return Err(InvalidReason::InvalidInput);
        }
        Ok(reason)
    }

    #[must_use]
    pub fn render(&self) -> String {
        let mut reason = format!(
            "v={VERSION};op={OPERATION};event={};reporter_taint={}",
            self.event.as_str(),
            self.reporter_taint.as_str()
        );
        if let Some(evidence_id) = &self.evidence_id {
            reason.push_str(";evidence_id=");
            reason.push_str(&encode(evidence_id));
        }
        if let Some(reference) = &self.evidence_ref {
            reason.push_str(";path=");
            reason.push_str(&encode(&reference.path));
            reason.push_str(";oid=");
            reason.push_str(&encode(&reference.git_object_id));
        }
        reason
    }

    /// The typed reason `reason` renders, or `None` when it is not a
    /// rendering [`Self::render`] produces: a field out of order or missing, a
    /// value outside its vocabulary, a non-canonical escape, or a value that
    /// decodes to a different string a second time.
    #[must_use]
    pub fn parse(reason: &str) -> Option<Self> {
        let mut fields = reason.split(';').map(|field| field.split_once('='));
        let mut next = |key: &str| match fields.next() {
            Some(Some((name, value))) if name == key => Some(value),
            _ => None,
        };
        if next("v")? != VERSION || next("op")? != OPERATION {
            return None;
        }
        let event = EventKind::try_from(next("event")?).ok()?;
        let reporter_taint = TaintClass::try_from(next("reporter_taint")?).ok()?;
        let rest: Vec<(&str, &str)> = fields.collect::<Option<Vec<_>>>()?;
        let value = |key: &str| -> Option<Option<String>> {
            match rest.iter().find(|(name, _)| *name == key) {
                None => Some(None),
                Some((_, value)) => decode(value)
                    .filter(|text| decodes_to_itself(text))
                    .map(Some),
            }
        };
        let evidence_id = value("evidence_id")?;
        let path = value("path")?;
        let oid = value("oid")?;
        let evidence_ref = match (path, oid) {
            (None, None) => None,
            (Some(path), Some(git_object_id)) if is_lower_hex_oid(&git_object_id) => {
                Some(EvidenceRef {
                    path,
                    git_object_id,
                })
            }
            _ => return None,
        };
        let parsed = Self {
            event,
            reporter_taint,
            evidence_id,
            evidence_ref,
        };
        (is_dispute(event) && parsed.render() == reason).then_some(parsed)
    }
}

impl EvidenceRef {
    fn is_well_formed(&self) -> bool {
        is_lower_hex_oid(&self.git_object_id)
            && self.path.len() <= MAX_EVIDENCE_PATH_BYTES
            && confined_path::check(&self.path).is_ok()
            && decodes_to_itself(&self.path)
            && redact_durable_text(&self.path).detections.is_empty()
    }
}

const fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

fn encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if is_unreserved(byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn hex_value(byte: u8) -> Option<u8> {
    char::from(byte)
        .to_digit(16)
        .and_then(|digit| u8::try_from(digit).ok())
}

/// The bytes `value` encodes, as UTF-8; `None` for a `%` without two hex
/// digits after it or bytes that are not UTF-8.
fn decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = hex_value(*bytes.get(index + 1)?)?;
            let low = hex_value(*bytes.get(index + 2)?)?;
            decoded.push(high << 4 | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

/// Whether `value` holds no `%` followed by two hex digits, so decoding it
/// again leaves it unchanged.
fn decodes_to_itself(value: &str) -> bool {
    !value.as_bytes().windows(3).any(|window| {
        window[0] == b'%' && hex_value(window[1]).is_some() && hex_value(window[2]).is_some()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const OID: &str = "0123456789abcdef0123456789abcdef01234567";

    fn reference(path: &str, oid: &str) -> EvidenceRef {
        EvidenceRef {
            path: path.to_string(),
            git_object_id: oid.to_string(),
        }
    }

    fn validated(path: &str, oid: &str) -> Result<String, InvalidReason> {
        DisputeReason::validated(
            EventKind::MarkDisputed,
            TaintClass::UserInferred,
            None,
            Some(&reference(path, oid)),
        )
    }

    #[test]
    fn the_documented_rendering_parses_back() {
        let reason = validated("src/a.rs", OID).unwrap();
        assert_eq!(
            reason,
            format!(
                "v=1;op=kernel.commit;event=mark_disputed;reporter_taint=user_inferred;path=src%2Fa.rs;oid={OID}"
            )
        );
        let parsed = DisputeReason::parse(&reason).unwrap();
        assert_eq!(parsed.evidence_ref, Some(reference("src/a.rs", OID)));
        assert_eq!(parsed.render(), reason);
    }

    #[test]
    fn render_and_parse_are_inverse_over_reserved_and_non_ascii_bytes() {
        for path in [
            "a;b.rs",
            "a=b",
            "100%",
            "a%zz/b",
            "x@y@z",
            "with space/file name.md",
            "dir/ünïcödé/日本.rs",
            "tilde~dash-under_score.dot",
        ] {
            for (event, taint) in [
                (EventKind::MarkDisputed, TaintClass::UserInferred),
                (EventKind::Contradict, TaintClass::AssistantInference),
            ] {
                let reason = DisputeReason::validated(
                    event,
                    taint,
                    Some("evidence;=%"),
                    Some(&reference(path, OID)),
                );
                let reason = reason.unwrap_or_else(|error| panic!("{path:?}: {error:?}"));
                assert!(reason.is_ascii(), "{reason}");
                let parsed = DisputeReason::parse(&reason).unwrap();
                assert_eq!(parsed.event, event);
                assert_eq!(parsed.reporter_taint, taint);
                assert_eq!(parsed.evidence_id.as_deref(), Some("evidence;=%"));
                assert_eq!(parsed.evidence_ref.as_ref().unwrap().path, path);
                assert_eq!(parsed.render(), reason);
            }
        }
    }

    #[test]
    fn reserved_and_multi_byte_values_encode_to_uppercase_escapes() {
        assert_eq!(encode(";=%ü/a-b.c_d~e"), "%3B%3D%25%C3%BC%2Fa-b.c_d~e");
        assert_eq!(
            decode("%3B%3D%25%C3%BC%2Fa-b.c_d~e").as_deref(),
            Some(";=%ü/a-b.c_d~e")
        );
    }

    proptest::proptest! {
        /// Every accepted value renders to an ASCII reason within the cap
        /// whose parsed fields equal the inputs.
        #[test]
        fn every_accepted_reference_parses_back_to_its_inputs(
            path in "[^\\x00]{1,64}",
            evidence_id in proptest::option::of(".{1,64}"),
        ) {
            let reference = reference(&path, OID);
            let Ok(reason) = DisputeReason::validated(
                EventKind::Contradict,
                TaintClass::AssistantInference,
                evidence_id.as_deref(),
                Some(&reference),
            ) else {
                return Ok(());
            };
            proptest::prop_assert!(reason.is_ascii() && reason.len() <= MAX_REASON_BYTES);
            let parsed = DisputeReason::parse(&reason).expect("an accepted reason parses");
            proptest::prop_assert_eq!(parsed.evidence_ref, Some(reference));
            proptest::prop_assert_eq!(parsed.evidence_id, evidence_id);
            proptest::prop_assert_eq!(decode(&encode(&path)), Some(path));
        }
    }

    #[test]
    fn malformed_references_are_invalid_input() {
        let upper = OID.to_uppercase();
        let sha256 = "a".repeat(64);
        let long = format!("{}/b", "a".repeat(MAX_EVIDENCE_PATH_BYTES));
        let sixteen = vec!["d"; 16].join("/");
        let seventeen = vec!["d"; 17].join("/");
        assert!(validated("src/a.rs", &sha256).is_ok());
        assert!(validated(&sixteen, OID).is_ok());
        for (path, oid) in [
            ("src/a.rs", upper.as_str()),
            ("src/a.rs", &OID[..39]),
            ("src/a.rs", &sha256[..63]),
            ("src/a.rs", ""),
            ("a/../b", OID),
            ("/etc/passwd", OID),
            ("", OID),
            ("a\0b", OID),
            (".git/config", OID),
            ("src/.eidnara/x", OID),
            (long.as_str(), OID),
            (seventeen.as_str(), OID),
            ("double%2Fencoded", OID),
        ] {
            assert_eq!(
                validated(path, oid),
                Err(InvalidReason::InvalidInput),
                "{path:?} {oid:?}"
            );
        }
    }

    #[test]
    fn a_path_the_secret_scanner_detects_is_invalid_input() {
        let secret = "sk-ant-api03-abcdefghijklmnopqrstuvwxyzABCDEFGH12345678";
        assert!(!redact_durable_text(secret).detections.is_empty());
        assert_eq!(
            validated(&format!("keys/{secret}.txt"), OID),
            Err(InvalidReason::InvalidInput)
        );
    }

    /// Git object ids are high-entropy hex; the scanner must leave them alone
    /// or every reference would be refused.
    #[test]
    fn git_object_ids_and_ordinary_paths_carry_no_detection() {
        use sha2::{Digest, Sha256};
        for seed in 0..64_u32 {
            let digest = format!("{:x}", Sha256::digest(seed.to_le_bytes()));
            for oid in [&digest[..40], digest.as_str()] {
                let reason = validated("src/kernel_routes/commit.rs", oid)
                    .unwrap_or_else(|error| panic!("{oid}: {error:?}"));
                assert!(
                    redact_durable_text(&reason).detections.is_empty(),
                    "{reason}"
                );
            }
        }
    }

    #[test]
    fn a_dispute_needs_evidence_and_other_events_take_none() {
        let refused = |event, evidence_id: Option<&str>| {
            DisputeReason::validated(event, TaintClass::UserInferred, evidence_id, None)
        };
        assert_eq!(
            refused(EventKind::MarkDisputed, None),
            Err(InvalidReason::InvalidInput)
        );
        assert_eq!(
            refused(EventKind::Contradict, Some("")),
            Err(InvalidReason::InvalidInput)
        );
        assert_eq!(
            refused(EventKind::MarkStale, Some("evidence")),
            Err(InvalidReason::InvalidInput)
        );
        assert!(refused(EventKind::Contradict, Some("evidence")).is_ok());
    }

    #[test]
    fn a_reason_over_the_cap_is_invalid_input() {
        let long_id = "e".repeat(MAX_REASON_BYTES);
        assert_eq!(
            DisputeReason::validated(
                EventKind::MarkDisputed,
                TaintClass::UserInferred,
                Some(&long_id),
                None
            ),
            Err(InvalidReason::InvalidInput)
        );
        let prefix =
            "v=1;op=kernel.commit;event=mark_disputed;reporter_taint=user_inferred;evidence_id=";
        let room = MAX_REASON_BYTES - prefix.len();
        let at_cap = DisputeReason::validated(
            EventKind::MarkDisputed,
            TaintClass::UserInferred,
            Some(&"e".repeat(room)),
            None,
        )
        .unwrap();
        assert_eq!(at_cap.len(), MAX_REASON_BYTES);
        for over in [
            "e".repeat(room + 1),
            "/".repeat(room / 3 + 1),
            "e".repeat(1 << 20),
        ] {
            assert_eq!(
                DisputeReason::validated(
                    EventKind::MarkDisputed,
                    TaintClass::UserInferred,
                    Some(&over),
                    None
                ),
                Err(InvalidReason::InvalidInput)
            );
        }
    }

    #[test]
    fn parse_refuses_every_rendering_render_does_not_produce() {
        let reason = validated("src/a.rs", OID).unwrap();
        for altered in [
            reason.replace("v=1", "v=2"),
            reason.replace("%2F", "%2f"),
            reason.replace("src%2Fa.rs", "src/a.rs"),
            reason.replace("src%2Fa", "%73rc%2Fa"),
            reason.replace(";oid=", ";oid=x").replace(OID, ""),
            reason.replace("event=mark_disputed", "event=mark_stale"),
            reason.replace("reporter_taint=user_inferred", "reporter_taint=oracle"),
            reason.replace(&format!(";oid={OID}"), ""),
            format!("{reason};extra=1"),
            reason.replace("path=src%2Fa.rs", "path=%25252F"),
            "kernel.commit".to_string(),
        ] {
            assert_eq!(DisputeReason::parse(&altered), None, "{altered}");
        }
    }
}
