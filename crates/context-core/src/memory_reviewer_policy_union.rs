//! The MemoryReviewer policy-dependency union: every input a review disclosed to the model, cited or not, plus the lineage those inputs resolve through, as one canonical, digestible set.
//!
//! Members are keyed by `(kind, id, revision, owner_id, owner_revision)`, deduplicated by that whole key, and sorted by their own canonical bytes, so two unions with the same members encode to the same bytes regardless of the order inputs were disclosed in. The identity is `SHA-256("eidnara-memory_reviewer-policy-union-v1\n" + canonical)`. Sensitivity class is not a member field: it is revalidated at every disclosure, not frozen into the union.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::canonical_json::{ContractError, encode_canonical_string, protocol_digest_of_canonical};

pub const MEMORY_REVIEWER_POLICY_UNION_VERSION: u32 = 1;
pub const MEMORY_REVIEWER_POLICY_UNION_PROTOCOL: &str = "eidnara-memory_reviewer-policy-union-v1";

/// One dependency of a review: the resolving expectation kind, the object or row it resolved, the revision it was read at (source revision for registry objects, immutable digest for staged rows and captures), and the live owner its eligibility resolved through, when that owner is not itself.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PolicyUnionMember {
    pub kind: String,
    pub id: String,
    pub revision: String,
    pub owner_id: Option<String>,
    pub owner_revision: Option<String>,
}

impl PolicyUnionMember {
    /// The member's canonical JSON object: its keys in code-point order, an absent owner field as `null`.
    fn canonical(&self) -> String {
        let mut out = String::with_capacity(
            64 + self.kind.len()
                + self.id.len()
                + self.revision.len()
                + self.owner_id.as_ref().map_or(0, String::len)
                + self.owner_revision.as_ref().map_or(0, String::len),
        );
        let optional = |out: &mut String, value: &Option<String>| match value {
            Some(value) => encode_canonical_string(out, value),
            None => out.push_str("null"),
        };
        out.push_str("{\"id\":");
        encode_canonical_string(&mut out, &self.id);
        out.push_str(",\"kind\":");
        encode_canonical_string(&mut out, &self.kind);
        out.push_str(",\"owner_id\":");
        optional(&mut out, &self.owner_id);
        out.push_str(",\"owner_revision\":");
        optional(&mut out, &self.owner_revision);
        out.push_str(",\"revision\":");
        encode_canonical_string(&mut out, &self.revision);
        out.push('}');
        out
    }
}

/// The union under construction; grows as inputs are disclosed and never shrinks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PolicyUnion {
    members: BTreeSet<PolicyUnionMember>,
}

/// The canonical bytes and their digest, persisted together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedPolicyUnion {
    pub canonical: String,
    pub digest: String,
    pub members: usize,
}

impl PolicyUnion {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a member; a duplicate of an existing key changes nothing and reports `false`.
    pub fn insert(&mut self, member: PolicyUnionMember) -> bool {
        self.members.insert(member)
    }

    pub fn len(&self) -> usize {
        self.members.len()
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    pub fn members(&self) -> impl Iterator<Item = &PolicyUnionMember> {
        self.members.iter()
    }

    pub fn contains(&self, member: &PolicyUnionMember) -> bool {
        self.members.contains(member)
    }

    /// Canonical encoding: members sorted by their own canonical bytes inside `{"v":1,"members":[...]}`, which is `{"members":[...],"v":1}` in canonical key order.
    pub fn encode(&self) -> Result<EncodedPolicyUnion, ContractError> {
        let mut members: Vec<String> = self
            .members
            .iter()
            .map(PolicyUnionMember::canonical)
            .collect();
        members.sort_unstable();
        let mut canonical = String::with_capacity(
            32 + members.iter().map(|member| member.len() + 1).sum::<usize>(),
        );
        canonical.push_str("{\"members\":[");
        for (index, member) in members.iter().enumerate() {
            if index > 0 {
                canonical.push(',');
            }
            canonical.push_str(member);
        }
        canonical.push_str("],\"v\":");
        canonical.push_str(&MEMORY_REVIEWER_POLICY_UNION_VERSION.to_string());
        canonical.push('}');
        Ok(EncodedPolicyUnion {
            digest: protocol_digest_of_canonical(MEMORY_REVIEWER_POLICY_UNION_PROTOCOL, &canonical),
            canonical,
            members: self.members.len(),
        })
    }

    /// Decodes persisted canonical bytes, accepting them only when they re-encode to themselves and to `digest`; a union that was edited, reordered, or produced under another version is not this union.
    pub fn decode(canonical: &str, digest: &str) -> Result<Self, ContractError> {
        let invalid = || ContractError::NotCanonical("policy union".to_string());
        let value: Value = serde_json::from_str(canonical).map_err(|_| invalid())?;
        let object = value.as_object().ok_or_else(invalid)?;
        if object.get("v").and_then(Value::as_u64)
            != Some(MEMORY_REVIEWER_POLICY_UNION_VERSION.into())
        {
            return Err(invalid());
        }
        let members = object
            .get("members")
            .and_then(Value::as_array)
            .ok_or_else(invalid)?;
        let mut union = Self::new();
        for member in members {
            let required = |name: &str| {
                member
                    .get(name)
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(invalid)
            };
            let optional = |name: &str| match member.get(name) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(value)) => Ok(Some(value.clone())),
                Some(_) => Err(invalid()),
            };
            union.insert(PolicyUnionMember {
                kind: required("kind")?,
                id: required("id")?,
                revision: required("revision")?,
                owner_id: optional("owner_id")?,
                owner_revision: optional("owner_revision")?,
            });
        }
        let encoded = union.encode()?;
        if encoded.canonical != canonical || encoded.digest != digest {
            return Err(invalid());
        }
        Ok(union)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(
        kind: &str,
        id: &str,
        revision: &str,
        owner: Option<(&str, &str)>,
    ) -> PolicyUnionMember {
        PolicyUnionMember {
            kind: kind.to_string(),
            id: id.to_string(),
            revision: revision.to_string(),
            owner_id: owner.map(|(id, _)| id.to_string()),
            owner_revision: owner.map(|(_, revision)| revision.to_string()),
        }
    }

    #[test]
    fn encoding_is_order_independent_and_deduplicated() {
        let mut first = PolicyUnion::new();
        assert!(first.insert(member("staged_subject", "cand-1", "d1", None)));
        assert!(first.insert(member(
            "canonical_source",
            "desc-1",
            "3",
            Some(("decision-1", "2"))
        )));
        assert!(!first.insert(member("staged_subject", "cand-1", "d1", None)));
        let mut second = PolicyUnion::new();
        second.insert(member(
            "canonical_source",
            "desc-1",
            "3",
            Some(("decision-1", "2")),
        ));
        second.insert(member("staged_subject", "cand-1", "d1", None));
        let (a, b) = (first.encode().unwrap(), second.encode().unwrap());
        assert_eq!(a, b);
        assert_eq!(a.members, 2);
        assert!(a.canonical.starts_with("{\"members\":["));
        assert!(a.canonical.ends_with(",\"v\":1}"));
        assert_eq!(a.digest.len(), 64);
        // A changed owner revision is a different union.
        let mut third = second.clone();
        third.insert(member(
            "canonical_source",
            "desc-1",
            "3",
            Some(("decision-1", "3")),
        ));
        assert_ne!(third.encode().unwrap().digest, a.digest);
        assert_eq!(PolicyUnion::new().encode().unwrap().members, 0);
    }

    #[test]
    fn decoding_accepts_only_bytes_that_re_encode_to_themselves_and_their_digest() {
        let mut union = PolicyUnion::new();
        union.insert(member("native_source", "obj-1", "4", None));
        union.insert(member(
            "canonical_source",
            "desc-1",
            "3",
            Some(("decision-1", "2")),
        ));
        let encoded = union.encode().unwrap();
        assert_eq!(
            PolicyUnion::decode(&encoded.canonical, &encoded.digest).unwrap(),
            union
        );
        assert!(PolicyUnion::decode(&encoded.canonical, &"0".repeat(64)).is_err());
        let edited = encoded
            .canonical
            .replace("\"revision\":\"4\"", "\"revision\":\"5\"");
        assert!(PolicyUnion::decode(&edited, &encoded.digest).is_err());
        let reordered = encoded
            .canonical
            .replacen("{\"members\":[", "{\"members\":[ ", 1);
        assert!(PolicyUnion::decode(&reordered, &encoded.digest).is_err());
        assert!(PolicyUnion::decode("{\"members\":[],\"v\":2}", &encoded.digest).is_err());
    }

    /// The union bytes and digest equal the generic canonical encoder's output for the documented `{"v":1,"members":[...]}` value, including strings that need escapes and non-ASCII text.
    #[test]
    fn encoding_equals_the_generic_canonical_encoder() {
        use crate::canonical_json::{canonical_json_encode, protocol_digest};
        use serde_json::json;
        let texts = [
            "",
            "plain",
            "quote\"and\\slash",
            "control\u{0}\u{1f}\n\t",
            "\u{7f}\u{e9}\u{1f600}",
            "zz",
            "A",
        ];
        let mut union = PolicyUnion::new();
        for (index, kind) in texts.iter().enumerate() {
            for id in texts {
                let revision = texts[(index + id.len()) % texts.len()];
                union.insert(member(kind, id, revision, None));
                union.insert(member(kind, id, revision, Some((id, kind))));
            }
        }
        let mut members: Vec<(String, Value)> = union
            .members()
            .map(|member| {
                let value = json!({
                    "kind": member.kind,
                    "id": member.id,
                    "revision": member.revision,
                    "owner_id": member.owner_id,
                    "owner_revision": member.owner_revision,
                });
                (canonical_json_encode(&value).unwrap(), value)
            })
            .collect();
        members.sort_by(|left, right| left.0.cmp(&right.0));
        let value = json!({
            "v": MEMORY_REVIEWER_POLICY_UNION_VERSION,
            "members": members.into_iter().map(|(_, value)| value).collect::<Vec<_>>(),
        });
        let encoded = union.encode().unwrap();
        assert_eq!(encoded.canonical, canonical_json_encode(&value).unwrap());
        assert_eq!(
            encoded.digest,
            protocol_digest(MEMORY_REVIEWER_POLICY_UNION_PROTOCOL, &value).unwrap()
        );
        assert_eq!(
            PolicyUnion::decode(&encoded.canonical, &encoded.digest).unwrap(),
            union
        );
    }

    const FIXTURE: &str = include_str!("../testdata/canonical-json-contract-v1.json");

    fn fixture() -> Value {
        serde_json::from_str(FIXTURE).expect("golden corpus parses")
    }

    /// Consumers store the digest, so the protocol literal and version are pinned; changing either is a `v2`.
    #[test]
    fn protocol_and_version_are_the_recorded_literals() {
        let fixture = fixture();
        assert_eq!(
            MEMORY_REVIEWER_POLICY_UNION_PROTOCOL,
            "eidnara-memory_reviewer-policy-union-v1"
        );
        assert_eq!(
            fixture["memory_reviewerPolicyUnionDigestProtocol"]
                .as_str()
                .unwrap(),
            MEMORY_REVIEWER_POLICY_UNION_PROTOCOL
        );
        assert_eq!(
            fixture["memory_reviewerPolicyUnionVersion"]
                .as_u64()
                .unwrap(),
            u64::from(MEMORY_REVIEWER_POLICY_UNION_VERSION)
        );
    }

    #[test]
    fn encoding_matches_the_golden_vectors() {
        for case in fixture()["memory_reviewerPolicyUnion"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let mut union = PolicyUnion::new();
            for member in case["members"].as_array().unwrap() {
                let text = |key: &str| member[key].as_str().map(str::to_string);
                union.insert(PolicyUnionMember {
                    kind: text("kind").unwrap(),
                    id: text("id").unwrap(),
                    revision: text("revision").unwrap(),
                    owner_id: text("owner_id"),
                    owner_revision: text("owner_revision"),
                });
            }
            let encoded = union.encode().unwrap();
            assert_eq!(
                encoded.canonical,
                case["canonical"].as_str().unwrap(),
                "case {name}"
            );
            assert_eq!(
                encoded.digest,
                case["digest"].as_str().unwrap(),
                "case {name}"
            );
            assert_eq!(encoded.members, union.len(), "case {name}");
        }
    }
}
