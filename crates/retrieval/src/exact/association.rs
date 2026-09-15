use kernel::source_identity::OccurrenceClass;
use rusqlite::{OptionalExtension, params};
use storage::GuardedConn;

use crate::exact::selector::Family;
use crate::{OccurrenceRecord, ProjectionError};

/// Changing a mapping changes derived rows, so increment this and rebuild the projection.
pub const EXTRACTION_VERSION: u32 = 1;

pub const MAX_KEYS_PER_RECORD: usize = Family::ALL.len();

pub const CANONICAL_OBJECT_NAMESPACE: &str = "canonical_object";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    Extracted(&'static [OccurrenceClass]),
    Missing,
}

pub fn coverage(family: Family) -> Coverage {
    match family {
        Family::Id => Coverage::Extracted(&[
            OccurrenceClass::CanonicalClaims,
            OccurrenceClass::PromotedMemory,
        ]),
        Family::Sha => Coverage::Extracted(&[OccurrenceClass::GitCommits]),
        Family::Path | Family::Symbol | Family::Command | Family::Config | Family::Error => {
            Coverage::Missing
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssociationKey {
    pub family: Family,
    pub namespace: String,
    pub key: Vec<u8>,
    pub target_id: String,
}

pub fn sha_namespace(object_format: &str, repository_id: &str) -> String {
    format!("{object_format}:{repository_id}")
}

pub fn extract(record: &OccurrenceRecord<'_>, lineage_id: &str) -> Vec<AssociationKey> {
    let field = |name: &str| {
        record
            .occurrence
            .identity
            .iter()
            .find(|(field, _)| *field == name)
            .map(|(_, value)| *value)
    };
    match OccurrenceClass::from_code(record.occurrence.class) {
        Some(OccurrenceClass::GitCommits) => {
            match (field("repository_id"), field("object_format"), field("oid")) {
                (Some(repository), Some(format), Some(oid)) => vec![AssociationKey {
                    family: Family::Sha,
                    namespace: sha_namespace(format, repository),
                    key: oid.as_bytes().to_vec(),
                    target_id: lineage_id.to_string(),
                }],
                _ => Vec::new(),
            }
        }
        Some(OccurrenceClass::CanonicalClaims) => canonical_object(field("object_id")),
        Some(OccurrenceClass::PromotedMemory) => canonical_object(field("decision_object_id")),
        Some(OccurrenceClass::Messages | OccurrenceClass::RawToolSpans) | None => Vec::new(),
    }
}

fn canonical_object(object_id: Option<&str>) -> Vec<AssociationKey> {
    object_id
        .map(|object_id| AssociationKey {
            family: Family::Id,
            namespace: CANONICAL_OBJECT_NAMESPACE.to_string(),
            key: object_id.as_bytes().to_vec(),
            target_id: object_id.to_string(),
        })
        .into_iter()
        .collect()
}

pub(crate) fn persist(
    conn: &GuardedConn<'_>,
    occurrence_id: &str,
    keys: &[AssociationKey],
    created_commit_seq: i64,
) -> Result<usize, ProjectionError> {
    let mut lookup = conn.prepare_cached(
        "SELECT target_id,extraction_version FROM exact_associations
         WHERE family=?1 AND namespace=?2 AND key=?3 AND occurrence_id=?4",
    )?;
    let mut insert = conn.prepare_cached(
        "INSERT INTO exact_associations(
             family,namespace,key,occurrence_id,target_id,extraction_version,created_commit_seq
         ) VALUES (?1,?2,?3,?4,?5,?6,?7)",
    )?;
    let mut inserted = 0;
    for key in keys {
        let stored: Option<(String, u32)> = lookup
            .query_row(
                params![key.family.keyword(), key.namespace, key.key, occurrence_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        match stored {
            Some((target, version)) if target == key.target_id && version == EXTRACTION_VERSION => {
            }
            Some(_) => {
                return Err(ProjectionError::AssociationCollision {
                    occurrence_id: occurrence_id.to_string(),
                });
            }
            None => {
                insert.execute(params![
                    key.family.keyword(),
                    key.namespace,
                    key.key,
                    occurrence_id,
                    key.target_id,
                    EXTRACTION_VERSION,
                    created_commit_seq,
                ])?;
                inserted += 1;
            }
        }
    }
    Ok(inserted)
}

pub(crate) fn stored(
    conn: &GuardedConn<'_>,
    occurrence_id: &str,
    keys: &[AssociationKey],
) -> Result<bool, ProjectionError> {
    let mut lookup = conn.prepare_cached(
        "SELECT EXISTS(SELECT 1 FROM exact_associations
         WHERE family=?1 AND namespace=?2 AND key=?3 AND occurrence_id=?4 AND target_id=?5)",
    )?;
    for key in keys {
        let present: bool = lookup.query_row(
            params![
                key.family.keyword(),
                key.namespace,
                key.key,
                occurrence_id,
                key.target_id
            ],
            |row| row.get(0),
        )?;
        if !present {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Payload;
    use kernel::Sensitivity;
    use kernel::source_identity::Occurrence;

    fn record<'a>(class: &'a str, identity: &'a [(&'a str, &'a str)]) -> OccurrenceRecord<'a> {
        OccurrenceRecord {
            occurrence: Occurrence {
                class,
                identity,
                revision: "1",
                representation: "x",
                span: None,
            },
            payload: Payload::Whole("text"),
            domain_id: "d",
            sensitivity: Sensitivity::Normal,
            source_object_id: "s",
            source_evidence_id: "e",
            source_artifact_digest: "a",
            created_commit_seq: 1,
        }
    }

    #[test]
    fn git_commits_map_to_repository_scoped_sha_keys_targeting_the_lineage() {
        let oid = "a".repeat(40);
        let identity = [
            ("repository_id", "repo-1"),
            ("object_format", "sha1"),
            ("oid", oid.as_str()),
        ];
        let keys = extract(&record("git_commits", &identity), "lineage-1");
        assert_eq!(
            keys,
            vec![AssociationKey {
                family: Family::Sha,
                namespace: "sha1:repo-1".into(),
                key: oid.as_bytes().to_vec(),
                target_id: "lineage-1".into(),
            }]
        );
    }

    #[test]
    fn claims_and_promoted_memory_of_one_object_share_a_target() {
        let claim = extract(&record("canonical_claims", &[("object_id", "obj-1")]), "l1");
        let memory = extract(
            &record("promoted_memory", &[("decision_object_id", "obj-1")]),
            "l2",
        );
        assert_eq!(claim.len(), 1);
        assert_eq!(claim[0].family, Family::Id);
        assert_eq!(claim[0].namespace, CANONICAL_OBJECT_NAMESPACE);
        assert_eq!(claim[0].key, b"obj-1");
        assert_eq!(claim[0].target_id, memory[0].target_id);
    }

    #[test]
    fn classes_without_a_mapping_yield_no_keys_and_families_report_missing_coverage() {
        assert!(extract(&record("messages", &[("message_id", "m")]), "l").is_empty());
        assert!(extract(&record("raw_tool_spans", &[]), "l").is_empty());
        assert!(extract(&record("unknown", &[]), "l").is_empty());
        for family in [
            Family::Path,
            Family::Symbol,
            Family::Command,
            Family::Config,
            Family::Error,
        ] {
            assert_eq!(coverage(family), Coverage::Missing, "{family:?}");
        }
        assert!(matches!(coverage(Family::Id), Coverage::Extracted(_)));
        assert!(matches!(coverage(Family::Sha), Coverage::Extracted(_)));
    }
}
