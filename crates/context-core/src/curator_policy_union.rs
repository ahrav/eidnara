//! The Curator policy-dependency union: every input a review disclosed to the model, cited or not, plus the lineage those inputs resolve through, as one canonical, digestible set.
//!
//! Members are keyed by `(kind, id, revision, owner_id, owner_revision)`, deduplicated by that whole key, and sorted by their own canonical bytes, so two unions with the same members encode to the same bytes regardless of the order inputs were disclosed in. The identity is `SHA-256("eidnara-curator-policy-union-v1\n" + canonical)`. Sensitivity class is not a member field: it is revalidated at every disclosure, not frozen into the union.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use crate::canonical_json::{ContractError, canonical_json_encode, protocol_digest};

pub const CURATOR_POLICY_UNION_VERSION: u32 = 1;
pub const CURATOR_POLICY_UNION_PROTOCOL: &str = "eidnara-curator-policy-union-v1";

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
    fn to_value(&self) -> Value {
        json!({
            "kind": self.kind,
            "id": self.id,
            "revision": self.revision,
            "owner_id": self.owner_id,
            "owner_revision": self.owner_revision,
        })
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

    /// Canonical encoding: members sorted by their own canonical bytes inside `{"v":1,"members":[...]}`.
    pub fn encode(&self) -> Result<EncodedPolicyUnion, ContractError> {
        let mut encoded: Vec<(String, Value)> = self
            .members
            .iter()
            .map(|member| {
                let value = member.to_value();
                canonical_json_encode(&value).map(|bytes| (bytes, value))
            })
            .collect::<Result<_, _>>()?;
        encoded.sort_by(|left, right| left.0.cmp(&right.0));
        let value = json!({
            "v": CURATOR_POLICY_UNION_VERSION,
            "members": encoded.into_iter().map(|(_, value)| value).collect::<Vec<_>>(),
        });
        Ok(EncodedPolicyUnion {
            canonical: canonical_json_encode(&value)?,
            digest: protocol_digest(CURATOR_POLICY_UNION_PROTOCOL, &value)?,
            members: self.members.len(),
        })
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

    const FIXTURE: &str = include_str!("../testdata/canonical-json-contract-v1.json");

    fn fixture() -> Value {
        serde_json::from_str(FIXTURE).expect("golden corpus parses")
    }

    /// Consumers store the digest, so the protocol literal and version are pinned; changing either is a `v2`.
    #[test]
    fn protocol_and_version_are_the_recorded_literals() {
        let fixture = fixture();
        assert_eq!(
            CURATOR_POLICY_UNION_PROTOCOL,
            "eidnara-curator-policy-union-v1"
        );
        assert_eq!(
            fixture["curatorPolicyUnionDigestProtocol"]
                .as_str()
                .unwrap(),
            CURATOR_POLICY_UNION_PROTOCOL
        );
        assert_eq!(
            fixture["curatorPolicyUnionVersion"].as_u64().unwrap(),
            u64::from(CURATOR_POLICY_UNION_VERSION)
        );
    }

    #[test]
    fn encoding_matches_the_golden_vectors() {
        for case in fixture()["curatorPolicyUnion"].as_array().unwrap() {
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
