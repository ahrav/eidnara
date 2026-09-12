//! The shared source-identity fixtures drive persistence: every record is
//! written through the pure mutations, read back, and compared with the
//! fixture's own expectations for identity, multiplicity, sharing, and bytes.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use kernel::Sensitivity;
use kernel::source_identity::{Occurrence, OccurrenceRefusal, Span};
use retrieval::{
    OccurrenceRecord, Payload, PersistBounds, PersistedOccurrence, ProjectionError,
    ProjectionIdentity, Tombstone, TombstoneReason, install_identity, persist_occurrences,
    persist_occurrences_with_digests_for_test, read_identity, read_occurrence,
    tombstone_occurrence,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use storage::{
    GuardedConn, Isolation, SqliteStore, StorageBackend, StorageDescriptor, open_sqlite,
};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixtures() -> Value {
    let path = repo_root()
        .join("crates/kernel/tests/fixtures/search-projection/source-identity-fixtures.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn bounds() -> PersistBounds {
    PersistBounds {
        max_records: NonZeroUsize::new(64).unwrap(),
        max_payload_bytes: NonZeroUsize::new(4096).unwrap(),
        max_tuple_bytes: NonZeroUsize::new(2048).unwrap(),
    }
}

fn open(dir: &Path) -> SqliteStore {
    open_sqlite(
        &StorageDescriptor {
            module_id: "eidnara-test".to_string(),
            storage_namespace: "search-projection".to_string(),
            isolation: Isolation::Module,
            backend: StorageBackend::Sqlite {
                path: dir
                    .join("search")
                    .join("search.sqlite")
                    .to_string_lossy()
                    .into_owned(),
            },
        },
        retrieval::BASELINE,
    )
    .unwrap()
}

/// A fixture record held in owned form so an `Occurrence` can borrow it.
#[derive(Debug, Clone)]
struct Owned {
    id: String,
    source_id: String,
    class: String,
    identity: Vec<(String, String)>,
    revision: String,
    representation: String,
    span: Option<Span>,
    payload: String,
    expected_occurrence_id: Option<String>,
    expected_payload_id: Option<String>,
    expected_lineage_id: Option<String>,
    expected_refusal: Option<String>,
}

impl Owned {
    fn from_json(value: &Value) -> Self {
        let id = value["id"].as_str().unwrap();
        let identity = value["identity"]
            .as_object()
            .map(|fields| {
                fields
                    .iter()
                    .map(|(name, value)| {
                        (
                            name.clone(),
                            value
                                .as_str()
                                .map(str::to_string)
                                .unwrap_or_else(|| value.to_string()),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let span = value
            .get("span")
            .and_then(|span| span.as_array())
            .and_then(|parts| match parts.as_slice() {
                [start, end] => Some(Span {
                    start: start.as_u64()?,
                    end: end.as_u64()?,
                }),
                _ => None,
            });
        Self {
            id: id.to_string(),
            // A replay is metadata-equal to its canonical record.
            source_id: match id {
                "m1dup" => "m1",
                "t_whole_span" => "t_whole",
                other => other,
            }
            .to_string(),
            class: value["class"].as_str().unwrap_or("").to_string(),
            identity,
            revision: value["revision"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_default(),
            representation: value["representation"].as_str().unwrap_or("").to_string(),
            span,
            payload: value["payload"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_default(),
            expected_occurrence_id: value["expected_occurrence_id"].as_str().map(str::to_string),
            expected_payload_id: value["expected_payload_id"].as_str().map(str::to_string),
            expected_lineage_id: value["expected_lineage_id"].as_str().map(str::to_string),
            expected_refusal: value["expected_refusal"].as_str().map(str::to_string),
        }
    }

    /// Whether the fixture's JSON shape is one the Rust request type can even
    /// express; the others are refused by the producer before reaching here.
    fn expressible(value: &Value) -> bool {
        value["payload"].is_string()
            && value.get("span").is_none_or(|span| {
                span.is_null()
                    || span
                        .as_array()
                        .is_some_and(|p| p.len() == 2 && p.iter().all(|x| x.as_u64().is_some()))
            })
            && value["identity"]
                .as_object()
                .is_some_and(|fields| fields.values().all(Value::is_string))
            && value["revision"].is_string()
    }

    fn record<'a>(&'a self, identity: &'a [(&'a str, &'a str)]) -> OccurrenceRecord<'a> {
        OccurrenceRecord {
            occurrence: Occurrence {
                class: &self.class,
                identity,
                revision: &self.revision,
                representation: &self.representation,
                span: self.span,
            },
            payload: Payload::Whole(&self.payload),
            domain_id: "domain-stable-id",
            sensitivity: Sensitivity::Normal,
            source_object_id: &self.source_id,
            source_evidence_id: &self.source_id,
            source_artifact_digest: "0000000000000000000000000000000000000000000000000000000000000000",
            created_commit_seq: 7,
        }
    }
}

fn borrowed(identity: &[(String, String)]) -> Vec<(&str, &str)> {
    identity
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

fn persist_all(
    conn: &GuardedConn<'_>,
    records: &[Owned],
) -> Result<Vec<PersistedOccurrence>, ProjectionError> {
    let identities: Vec<Vec<(&str, &str)>> =
        records.iter().map(|r| borrowed(&r.identity)).collect();
    let requests: Vec<OccurrenceRecord<'_>> = records
        .iter()
        .zip(&identities)
        .map(|(record, identity)| record.record(identity))
        .collect();
    persist_occurrences(conn, &requests, bounds(), 1)
}

fn row_counts(store: &SqliteStore) -> (i64, i64) {
    store
        .with_conn(|conn| {
            Ok((
                conn.query_row("SELECT COUNT(*) FROM occurrences", [], |r| r.get(0))?,
                conn.query_row("SELECT COUNT(*) FROM payloads", [], |r| r.get(0))?,
            ))
        })
        .unwrap()
}

/// The identifier the canonical digests give `record`.
fn occurrence_id_of(record: &Owned) -> String {
    let identity = borrowed(&record.identity);
    kernel::source_identity::encode(&record.record(&identity).occurrence, &record.payload)
        .unwrap()
        .occurrence_id
}

/// Every stored occurrence identifier in `(class, source_object_id, revision)`
/// order, the order a ledger comparison walks.
fn stored_occurrence_ids(conn: &GuardedConn<'_>) -> Vec<String> {
    let mut statement = conn
        .prepare("SELECT occurrence_id FROM occurrences ORDER BY class,source_object_id,revision")
        .unwrap();
    statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn identity() -> ProjectionIdentity {
    ProjectionIdentity {
        schema_version: retrieval::SCHEMA_VERSION,
        kernel_incarnation_id: "kernel-incarnation-1".to_string(),
        projection_policy_version: "source-policy.v1".to_string(),
        identity_contract_version: "search-projection-identity-v2".to_string(),
        limit_manifest_protocol_version: "limits.v1".to_string(),
        embedding_model: "model-a".to_string(),
        tokenizer_fingerprint: "fp-a".to_string(),
        vector_dimension: 8,
        generation_epoch: 1,
    }
}

fn pairs<'a>(fixtures: &'a Value, key: &str) -> Vec<(&'a str, &'a str)> {
    fixtures["expectations"][key]
        .as_array()
        .unwrap()
        .iter()
        .map(|pair| (pair[0].as_str().unwrap(), pair[1].as_str().unwrap()))
        .collect()
}

#[test]
fn fixture_records_survive_write_close_and_reopen_with_their_expected_identities() {
    let fixtures = fixtures();
    let records: Vec<Owned> = fixtures["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(Owned::from_json)
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let outcomes = {
        let store = open(dir.path());
        store
            .with_conn_fenced(|conn| {
                install_identity(conn, &identity(), 1).unwrap();
                Ok(persist_all(conn, &records).unwrap())
            })
            .unwrap()
    };
    let by_id: BTreeMap<&str, &PersistedOccurrence> = records
        .iter()
        .map(|r| r.id.as_str())
        .zip(outcomes.iter())
        .collect();
    for record in &records {
        let outcome = by_id[record.id.as_str()];
        assert_eq!(
            Some(&outcome.occurrence_id),
            record.expected_occurrence_id.as_ref(),
            "{}",
            record.id
        );
        assert_eq!(
            Some(&outcome.payload_id),
            record.expected_payload_id.as_ref(),
            "{}",
            record.id
        );
        assert_eq!(
            Some(&outcome.lineage_id),
            record.expected_lineage_id.as_ref(),
            "{}",
            record.id
        );
    }
    // Distinct identical records stay distinct occurrences, including the
    // claim that is also promoted memory; equal bytes share one payload row;
    // the exact duplicate replayed as a no-op.
    for (a, b) in pairs(&fixtures, "distinct_occurrences") {
        assert_ne!(by_id[a].occurrence_id, by_id[b].occurrence_id, "{a} vs {b}");
    }
    for (a, b) in pairs(&fixtures, "equal_occurrences") {
        assert_eq!(by_id[a].occurrence_id, by_id[b].occurrence_id, "{a} vs {b}");
        assert!(by_id[a].inserted && !by_id[b].inserted, "{b} replays {a}");
    }
    for (a, b) in pairs(&fixtures, "equal_payloads") {
        assert_eq!(by_id[a].payload_id, by_id[b].payload_id, "{a} vs {b}");
        assert!(
            by_id[a].payload_inserted ^ by_id[b].payload_inserted,
            "exactly one of {a} and {b} created the shared payload row"
        );
    }
    for (a, b) in pairs(&fixtures, "distinct_payloads") {
        assert_ne!(by_id[a].payload_id, by_id[b].payload_id, "{a} vs {b}");
    }
    for (a, b) in pairs(&fixtures, "equal_lineages") {
        assert_eq!(by_id[a].lineage_id, by_id[b].lineage_id);
    }
    for (a, b) in pairs(&fixtures, "distinct_lineages") {
        assert_ne!(by_id[a].lineage_id, by_id[b].lineage_id);
    }

    // A span selecting every byte is the whole-block selection: it persists
    // as the same occurrence as the spanless record, not a second one.
    {
        let store = open(dir.path());
        let whole = records.iter().find(|r| r.id == "t_whole").unwrap();
        let mut covering = whole.clone();
        covering.span = Some(Span {
            start: 0,
            end: whole.payload.len() as u64,
        });
        let covering_identity = borrowed(&covering.identity);
        let outcome = store
            .with_conn_fenced(|conn| {
                Ok(
                    persist_occurrences(conn, &[covering.record(&covering_identity)], bounds(), 2)
                        .unwrap(),
                )
            })
            .unwrap();
        assert_eq!(outcome[0].occurrence_id, by_id["t_whole"].occurrence_id);
        assert!(!outcome[0].inserted && !outcome[0].payload_inserted);
    }

    // Reopen: the identity, every tuple, and every byte come back exactly.
    let store = open(dir.path());
    store
        .with_conn(|conn| {
            assert_eq!(read_identity(conn).unwrap(), Some(identity()));
            let distinct: std::collections::BTreeSet<&str> =
                outcomes.iter().map(|o| o.occurrence_id.as_str()).collect();
            // (class, source object, revision) order, one entry per occurrence.
            let mut expected_order: Vec<(String, String, i64, String)> = records
                .iter()
                .map(|r| {
                    (
                        r.class.clone(),
                        r.source_id.clone(),
                        r.revision.parse().unwrap(),
                        by_id[r.id.as_str()].occurrence_id.clone(),
                    )
                })
                .collect();
            expected_order.sort();
            expected_order.dedup_by(|a, b| a.3 == b.3);
            assert_eq!(expected_order.len(), distinct.len());
            assert_eq!(
                stored_occurrence_ids(conn),
                expected_order
                    .into_iter()
                    .map(|(_, _, _, id)| id)
                    .collect::<Vec<_>>()
            );
            let distinct_payloads: std::collections::BTreeSet<&str> =
                outcomes.iter().map(|o| o.payload_id.as_str()).collect();
            let payload_rows: i64 =
                conn.query_row("SELECT COUNT(*) FROM payloads", [], |r| r.get(0))?;
            assert_eq!(
                payload_rows,
                distinct_payloads.len() as i64,
                "one row per distinct payload"
            );
            let exact = fixtures["expectations"]["exact_round_trip"]
                .as_object()
                .unwrap();
            let spans = fixtures["expectations"]["span_payloads"]
                .as_object()
                .unwrap();
            for record in &records {
                let stored = read_occurrence(conn, &by_id[record.id.as_str()].occurrence_id)
                    .unwrap()
                    .expect(&record.id);
                assert_eq!(stored.class, record.class);
                assert_eq!(stored.revision.to_string(), record.revision);
                assert_eq!(stored.representation, record.representation);
                assert_eq!(stored.payload_id, by_id[record.id.as_str()].payload_id);
                assert_eq!(stored.tombstone, None);
                assert_eq!(stored.sensitivity, Sensitivity::Normal);
                assert_eq!(stored.domain_id, "domain-stable-id");
                assert_eq!(stored.source_object_id, record.source_id);
                assert_eq!(stored.source_evidence_id, record.source_id);
                assert_eq!(
                    stored.source_artifact_digest,
                    "0000000000000000000000000000000000000000000000000000000000000000"
                );
                assert_eq!(stored.created_commit_seq, 7);
                let expected_text = exact
                    .get(&record.id)
                    .or_else(|| spans.get(&record.id))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| match record.span {
                        None => record.payload.clone(),
                        Some(span) => {
                            record.payload[span.start as usize..span.end as usize].to_string()
                        }
                    });
                assert_eq!(
                    std::str::from_utf8(&stored.bytes).unwrap(),
                    expected_text,
                    "{}",
                    record.id
                );
                assert!(!stored.tuple.is_empty());
                assert_eq!(stored.tuple[0], 0x02, "encoding version");
                assert_eq!(stored.tuple[1], 0x00, "occurrence role");
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn missing_or_altered_payload_is_corruption_not_absence() {
    let fixtures = fixtures();
    let record = Owned::from_json(&fixtures["records"][0]);
    let mut empty = Owned::from_json(&fixtures["records"][1]);
    empty.payload.clear();
    let mut valid = Owned::from_json(&fixtures["records"][2]);
    valid.payload = "intact payload".to_string();
    let mut altered = record.payload.as_bytes().to_vec();
    altered[0] ^= 1;
    assert_eq!(altered.len(), record.payload.len());
    assert_ne!(altered, record.payload.as_bytes());

    for damaged_payload in [None, Some(altered.as_slice())] {
        let dir = tempfile::tempdir().unwrap();
        let outcomes = {
            let store = open(dir.path());
            store
                .with_conn_fenced(|conn| {
                    Ok(persist_all(conn, &[record.clone(), empty.clone(), valid.clone()]).unwrap())
                })
                .unwrap()
        };

        // Corruption injection uses a raw connection; the guarded store is closed.
        {
            let raw = rusqlite::Connection::open(dir.path().join("search/search.sqlite")).unwrap();
            let changed = match damaged_payload {
                Some(bytes) => {
                    raw.pragma_update(None, "foreign_keys", true).unwrap();
                    raw.execute(
                        "UPDATE payloads SET bytes=?2 WHERE payload_id=?1",
                        rusqlite::params![outcomes[0].payload_id, bytes],
                    )
                }
                None => {
                    raw.pragma_update(None, "foreign_keys", false).unwrap();
                    raw.execute(
                        "DELETE FROM payloads WHERE payload_id=?1",
                        [&outcomes[0].payload_id],
                    )
                }
            };
            assert_eq!(changed.unwrap(), 1);
        }

        let store = open(dir.path());
        assert_eq!(
            row_counts(&store),
            (3, 2 + i64::from(damaged_payload.is_some()))
        );
        store
            .with_conn(|conn| {
                assert_eq!(read_occurrence(conn, "no-such-occurrence"), Ok(None));
                for (outcome, expected) in outcomes[1..].iter().zip([&empty, &valid]) {
                    assert_eq!(
                        read_occurrence(conn, &outcome.occurrence_id)
                            .unwrap()
                            .map(|stored| stored.bytes),
                        Some(expected.payload.as_bytes().to_vec())
                    );
                }
                assert_eq!(
                    read_occurrence(conn, &outcomes[0].occurrence_id),
                    Err(ProjectionError::CorruptRow)
                );
                Ok(())
            })
            .unwrap();
    }
}

#[test]
fn oversized_payload_with_a_matching_digest_is_corruption() {
    let fixtures = fixtures();
    let record = Owned::from_json(&fixtures["records"][0]);
    let dir = tempfile::tempdir().unwrap();
    let outcome = {
        let store = open(dir.path());
        store
            .with_conn_fenced(|conn| {
                Ok(persist_all(conn, std::slice::from_ref(&record))
                    .unwrap()
                    .remove(0))
            })
            .unwrap()
    };
    let oversized = kernel::MAX_PAYLOAD_BYTES + 1;
    let mut digest = Sha256::new();
    let zeros = [0_u8; 64 * 1024];
    for _ in 0..oversized / zeros.len() {
        digest.update(zeros);
    }
    digest.update(&zeros[..oversized % zeros.len()]);
    let oversized_payload_id = format!("{:x}", digest.finalize());
    let oversized_sql = i64::try_from(oversized).unwrap();
    {
        let raw = rusqlite::Connection::open(dir.path().join("search/search.sqlite")).unwrap();
        assert_eq!(
            raw.execute(
                "INSERT INTO payloads(payload_id,bytes,byte_length,created_at)
                 SELECT ?2,zeroblob(?3),?3,created_at FROM payloads WHERE payload_id=?1",
                rusqlite::params![outcome.payload_id, oversized_payload_id, oversized_sql],
            )
            .unwrap(),
            1,
        );
        assert_eq!(
            raw.execute(
                "UPDATE occurrences SET payload_id=?2 WHERE occurrence_id=?1",
                rusqlite::params![outcome.occurrence_id, oversized_payload_id],
            )
            .unwrap(),
            1,
        );
    }

    let store = open(dir.path());
    store
        .with_conn(|conn| {
            assert_eq!(
                read_occurrence(conn, &outcome.occurrence_id),
                Err(ProjectionError::CorruptRow),
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn forced_collisions_refuse_unequal_values_and_replay_keeps_identities() {
    let fixtures = fixtures();
    let records: Vec<Owned> = fixtures["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(Owned::from_json)
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let first = store
        .with_conn_fenced(|conn| Ok(persist_all(conn, &records).unwrap()))
        .unwrap();
    // Replay in reverse order: every identity is the same, nothing is inserted.
    let mut reversed = records.clone();
    reversed.reverse();
    let replay = store
        .with_conn_fenced(|conn| Ok(persist_all(conn, &reversed).unwrap()))
        .unwrap();
    let mut replayed: Vec<(String, String)> = replay
        .iter()
        .map(|o| (o.occurrence_id.clone(), o.payload_id.clone()))
        .collect();
    replayed.sort();
    let mut original: Vec<(String, String)> = first
        .iter()
        .map(|o| (o.occurrence_id.clone(), o.payload_id.clone()))
        .collect();
    original.sort();
    assert_eq!(replayed, original);
    assert!(replay.iter().all(|o| !o.inserted && !o.payload_inserted));
    let rows_after_first: i64 = store
        .with_conn(|conn| conn.query_row("SELECT COUNT(*) FROM occurrences", [], |r| r.get(0)))
        .unwrap();

    // Forced payload collision: a different buffer is presented under an
    // existing payload digest, and refused. Forced tuple collision: a
    // different tuple is presented under an existing occurrence digest, and
    // refused. Neither refusal writes anything.
    let m1 = records.iter().find(|r| r.id == "m1").unwrap();
    // A record the store has never seen, so the only thing that can collide
    // is the digest the test plants.
    let mut fresh = records.iter().find(|r| r.id == "t_err").unwrap().clone();
    fresh.id = "fresh".to_string();
    fresh
        .identity
        .iter_mut()
        .find(|(name, _)| name == "tool_call_id")
        .unwrap()
        .1 = "call-fresh".to_string();
    let m1_identity = borrowed(&m1.identity);
    let t_err_identity = borrowed(&fresh.identity);
    let t_err = &fresh;
    let (rows_before, payloads_before, m1_tuple_len): (i64, i64, usize) = store
        .with_conn(|conn| {
            let tuple: Vec<u8> = conn.query_row(
                "SELECT tuple FROM occurrences WHERE occurrence_id=?1",
                [m1.expected_occurrence_id.as_deref().unwrap()],
                |r| r.get(0),
            )?;
            Ok((
                conn.query_row("SELECT COUNT(*) FROM occurrences", [], |r| r.get(0))?,
                conn.query_row("SELECT COUNT(*) FROM payloads", [], |r| r.get(0))?,
                tuple.len(),
            ))
        })
        .unwrap();
    let collide_payload = store
        .with_conn_fenced(|conn| {
            let request = t_err.record(&t_err_identity);
            let target = m1.expected_payload_id.clone().unwrap();
            let result = persist_occurrences_with_digests_for_test(
                conn,
                &[request],
                bounds(),
                2,
                &|encoded, _| (encoded.occurrence_id.clone(), target.clone()),
            );
            Ok(result)
        })
        .unwrap();
    assert_eq!(
        collide_payload,
        Err(ProjectionError::PayloadCollision {
            payload_id: m1.expected_payload_id.clone().unwrap(),
        })
    );
    let collide_tuple = store
        .with_conn_fenced(|conn| {
            let request = OccurrenceRecord {
                occurrence: t_err.record(&t_err_identity).occurrence,
                ..m1.record(&m1_identity)
            };
            let target = m1.expected_occurrence_id.clone().unwrap();
            let result = persist_occurrences_with_digests_for_test(
                conn,
                &[request],
                bounds(),
                2,
                &|_, selected| {
                    assert_eq!(selected, m1.payload.as_bytes());
                    (
                        target.clone(),
                        kernel::source_identity::payload_id(selected),
                    )
                },
            );
            Ok(result)
        })
        .unwrap();
    assert_eq!(
        collide_tuple,
        Err(ProjectionError::OccurrenceCollision {
            occurrence_id: m1.expected_occurrence_id.clone().unwrap(),
        })
    );
    // Same-length unequal values collide too: a digest never stands in for
    // the bytes or the tuple, however long they are.
    let mut one_byte = m1.clone();
    one_byte.id = "one-byte".to_string();
    let message_id = one_byte
        .identity
        .iter_mut()
        .find(|(name, _)| name == "message_id")
        .unwrap();
    assert_eq!(message_id.1, "msg-001");
    message_id.1 = "msg-00X".to_string();
    one_byte.payload = {
        let mut bytes = m1.payload.clone().into_bytes();
        bytes[0] ^= 0x01;
        String::from_utf8(bytes).unwrap()
    };
    assert_eq!(one_byte.payload.len(), m1.payload.len());
    let one_byte_identity = borrowed(&one_byte.identity);
    let same_length_payload = store
        .with_conn_fenced(|conn| {
            let target = m1.expected_payload_id.clone().unwrap();
            Ok(persist_occurrences_with_digests_for_test(
                conn,
                &[one_byte.record(&one_byte_identity)],
                bounds(),
                2,
                &|encoded, _| (encoded.occurrence_id.clone(), target.clone()),
            ))
        })
        .unwrap();
    assert_eq!(
        same_length_payload,
        Err(ProjectionError::PayloadCollision {
            payload_id: m1.expected_payload_id.clone().unwrap(),
        })
    );
    let same_length_tuple = store
        .with_conn_fenced(|conn| {
            let target = m1.expected_occurrence_id.clone().unwrap();
            Ok(persist_occurrences_with_digests_for_test(
                conn,
                &[OccurrenceRecord {
                    occurrence: one_byte.record(&one_byte_identity).occurrence,
                    ..m1.record(&m1_identity)
                }],
                bounds(),
                2,
                &|encoded, selected| {
                    assert_eq!(encoded.tuple.len(), m1_tuple_len);
                    assert_eq!(selected, m1.payload.as_bytes());
                    (
                        target.clone(),
                        kernel::source_identity::payload_id(selected),
                    )
                },
            ))
        })
        .unwrap();
    assert_eq!(
        same_length_tuple,
        Err(ProjectionError::OccurrenceCollision {
            occurrence_id: m1.expected_occurrence_id.clone().unwrap(),
        })
    );

    // Replaying m1 with equal values and its own digests writes nothing.
    let equal = store
        .with_conn_fenced(|conn| {
            let request = m1.record(&m1_identity);
            let before: i64 = conn.query_row("SELECT total_changes()", [], |row| row.get(0))?;
            let replay = persist_occurrences_with_digests_for_test(
                conn,
                &[request],
                bounds(),
                2,
                &|encoded, selected| {
                    (
                        encoded.occurrence_id.clone(),
                        kernel::source_identity::payload_id(selected),
                    )
                },
            )
            .unwrap();
            let after: i64 = conn.query_row("SELECT total_changes()", [], |row| row.get(0))?;
            assert_eq!(after, before, "equal replay must not mutate rows");
            Ok(replay)
        })
        .unwrap();
    assert!(!equal[0].inserted && !equal[0].payload_inserted);
    let (rows, payload_rows, m1_bytes): (i64, i64, Vec<u8>) = store
        .with_conn(|conn| {
            Ok((
                conn.query_row("SELECT COUNT(*) FROM occurrences", [], |r| r.get(0))?,
                conn.query_row("SELECT COUNT(*) FROM payloads", [], |r| r.get(0))?,
                conn.query_row(
                    "SELECT bytes FROM payloads WHERE payload_id=?1",
                    [m1.expected_payload_id.as_deref().unwrap()],
                    |r| r.get(0),
                )?,
            ))
        })
        .unwrap();
    assert_eq!(rows, rows_after_first);
    assert_eq!(
        m1_bytes,
        m1.payload.as_bytes(),
        "the stored bytes were not replaced"
    );
    assert_eq!(
        (rows, payload_rows),
        (rows_before, payloads_before),
        "refusals wrote nothing"
    );

    // Tombstones: recorded once, replayed as a no-op, never rewritten.
    let victim = first
        .iter()
        .find(|o| o.inserted)
        .unwrap()
        .occurrence_id
        .clone();
    let stone = Tombstone {
        invalidated_commit_seq: 9,
        reason: TombstoneReason::Superseded,
    };
    store
        .with_conn_fenced(|conn| {
            assert!(tombstone_occurrence(conn, &victim, stone, 3).unwrap());
            assert!(!tombstone_occurrence(conn, &victim, stone, 4).unwrap());
            assert_eq!(
                tombstone_occurrence(
                    conn,
                    &victim,
                    Tombstone {
                        invalidated_commit_seq: 10,
                        reason: TombstoneReason::Retired,
                    },
                    4
                ),
                Err(ProjectionError::TombstoneCollision {
                    occurrence_id: victim.clone(),
                })
            );
            assert_eq!(
                tombstone_occurrence(
                    conn,
                    &victim,
                    Tombstone {
                        invalidated_commit_seq: 0,
                        reason: TombstoneReason::Retired,
                    },
                    4
                ),
                Err(ProjectionError::NonPositiveTombstoneSequence {
                    occurrence_id: victim.clone(),
                })
            );
            assert_eq!(
                tombstone_occurrence(conn, "no-such-occurrence", stone, 4),
                Err(ProjectionError::UnknownOccurrence {
                    occurrence_id: "no-such-occurrence".to_string(),
                })
            );
            Ok(())
        })
        .unwrap();
    store
        .with_conn(|conn| {
            assert_eq!(
                read_occurrence(conn, &victim).unwrap().unwrap().tombstone,
                Some(stone)
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn tombstones_require_a_commit_after_occurrence_creation() {
    let fixtures = fixtures();
    let record = Owned::from_json(&fixtures["records"][0]);
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let outcomes = store
        .with_conn_fenced(|conn| Ok(persist_all(conn, &[record]).unwrap()))
        .unwrap();
    let occurrence_id = &outcomes[0].occurrence_id;
    let mut expected = store
        .with_conn(|conn| Ok(read_occurrence(conn, occurrence_id).unwrap().unwrap()))
        .unwrap();
    assert_eq!(expected.created_commit_seq, 7);
    assert_eq!(expected.tombstone, None);

    for invalidated_commit_seq in [6, 7] {
        let (result, changed_rows) = store
            .with_conn_fenced(|conn| {
                let before: i64 = conn.query_row("SELECT total_changes()", [], |row| row.get(0))?;
                let result = tombstone_occurrence(
                    conn,
                    occurrence_id,
                    Tombstone {
                        invalidated_commit_seq,
                        reason: TombstoneReason::Superseded,
                    },
                    2,
                );
                let after: i64 = conn.query_row("SELECT total_changes()", [], |row| row.get(0))?;
                Ok((result, after - before))
            })
            .unwrap();
        assert_eq!(
            result.map_err(|error| error.to_string()),
            Err(format!(
                "the tombstone for occurrence {occurrence_id} must name a commit sequence after its creation"
            )),
            "invalidation at {invalidated_commit_seq} must follow creation at 7"
        );
        assert_eq!(changed_rows, 0, "a rejected request must not mutate rows");
        store
            .with_conn(|conn| {
                assert_eq!(
                    read_occurrence(conn, occurrence_id).unwrap(),
                    Some(expected.clone())
                );
                Ok(())
            })
            .unwrap();
    }

    let stone = Tombstone {
        invalidated_commit_seq: 8,
        reason: TombstoneReason::Superseded,
    };
    store
        .with_conn_fenced(|conn| {
            assert_eq!(
                tombstone_occurrence(conn, occurrence_id, stone, 3),
                Ok(true)
            );
            assert_eq!(
                tombstone_occurrence(conn, occurrence_id, stone, 4),
                Ok(false)
            );
            Ok(())
        })
        .unwrap();
    expected.tombstone = Some(stone);
    store
        .with_conn(|conn| {
            assert_eq!(
                read_occurrence(conn, occurrence_id).unwrap(),
                Some(expected)
            );
            let recorded_at: i64 = conn.query_row(
                "SELECT recorded_at FROM occurrence_tombstones WHERE occurrence_id=?1",
                [occurrence_id],
                |row| row.get(0),
            )?;
            assert_eq!(recorded_at, 3, "replay must retain the original timestamp");
            Ok(())
        })
        .unwrap();
}

#[test]
fn a_collision_anywhere_in_a_batch_writes_none_of_the_batch() {
    let fixtures = fixtures();
    let m1 = Owned::from_json(
        fixtures["records"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == "m1")
            .unwrap(),
    );
    let variant = |id: &str, payload: &str| {
        let mut record = m1.clone();
        record.id = id.to_string();
        record
            .identity
            .iter_mut()
            .find(|(name, _)| name == "message_id")
            .unwrap()
            .1 = format!("msg-{id}");
        record.payload = payload.to_string();
        record
    };
    let a = variant("a", "payload of a");
    let b = variant("b", "payload of b");
    let mut b_altered = b.clone();
    b_altered.id = "b-altered".to_string();
    b_altered.payload = "payload of b, altered".to_string();
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());

    // `b` and `b_altered` share an occurrence identity but select different
    // bytes.
    let in_batch_tuple = store
        .with_conn_fenced(|conn| {
            Ok(persist_all(
                conn,
                &[a.clone(), b.clone(), b_altered.clone()],
            ))
        })
        .unwrap();
    assert_eq!(
        in_batch_tuple,
        Err(ProjectionError::OccurrenceCollision {
            occurrence_id: occurrence_id_of(&b),
        })
    );
    assert_eq!(row_counts(&store), (0, 0), "a and b were not written");

    // `c` is forced under `a`'s payload digest with different bytes.
    let c = variant("c", "payload of c");
    let a_payload_id = kernel::source_identity::payload_id(a.payload.as_bytes());
    let c_occurrence_id = occurrence_id_of(&c);
    let in_batch_payload = store
        .with_conn_fenced(|conn| {
            let identities: Vec<Vec<(&str, &str)>> =
                [&a, &b, &c].iter().map(|r| borrowed(&r.identity)).collect();
            let requests: Vec<OccurrenceRecord<'_>> = [&a, &b, &c]
                .iter()
                .zip(&identities)
                .map(|(record, identity)| record.record(identity))
                .collect();
            Ok(persist_occurrences_with_digests_for_test(
                conn,
                &requests,
                bounds(),
                1,
                &|encoded, selected| {
                    let payload_id = if encoded.occurrence_id == c_occurrence_id {
                        a_payload_id.clone()
                    } else {
                        kernel::source_identity::payload_id(selected)
                    };
                    (encoded.occurrence_id.clone(), payload_id)
                },
            ))
        })
        .unwrap();
    assert_eq!(
        in_batch_payload,
        Err(ProjectionError::PayloadCollision {
            payload_id: a_payload_id,
        })
    );
    assert_eq!(row_counts(&store), (0, 0), "a and b were not written");

    // `a_altered` collides with the `a` row an earlier call stored.
    store
        .with_conn_fenced(|conn| Ok(persist_all(conn, std::slice::from_ref(&a)).unwrap()))
        .unwrap();
    assert_eq!(row_counts(&store), (1, 1));
    let mut a_altered = a.clone();
    a_altered.id = "a-altered".to_string();
    a_altered.payload = "payload of a, altered".to_string();
    let stored_tuple = store
        .with_conn_fenced(|conn| Ok(persist_all(conn, &[b.clone(), a_altered.clone()])))
        .unwrap();
    assert_eq!(
        stored_tuple,
        Err(ProjectionError::OccurrenceCollision {
            occurrence_id: occurrence_id_of(&a),
        })
    );
    assert_eq!(
        row_counts(&store),
        (1, 1),
        "b was not written beside the refused record"
    );

    let outcomes = store
        .with_conn_fenced(|conn| Ok(persist_all(conn, &[b.clone(), a.clone(), b.clone()]).unwrap()))
        .unwrap();
    assert_eq!(
        outcomes.iter().map(|o| o.inserted).collect::<Vec<_>>(),
        [true, false, false],
        "b is new, a replays the stored row, the second b replays the first"
    );
    assert_eq!(
        outcomes
            .iter()
            .map(|o| o.payload_inserted)
            .collect::<Vec<_>>(),
        [true, false, false]
    );
    assert_eq!(row_counts(&store), (2, 2));
}

#[test]
fn malformed_and_oversized_records_refuse_before_anything_is_written() {
    let fixtures = fixtures();
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let mut covered = 0;
    for value in fixtures["invalid_records"].as_array().unwrap() {
        if !Owned::expressible(value) {
            continue;
        }
        let record = Owned::from_json(value);
        let identity = borrowed(&record.identity);
        let expected = record.expected_refusal.clone().unwrap();
        let result = store
            .with_conn_fenced(|conn| {
                Ok(persist_occurrences(
                    conn,
                    &[record.record(&identity)],
                    bounds(),
                    1,
                ))
            })
            .unwrap();
        match result {
            Err(ProjectionError::Occurrence(refusal)) => {
                assert_eq!(refusal.name(), expected, "{}", record.id)
            }
            other => panic!("{}: {other:?}", record.id),
        }
        covered += 1;
    }
    assert!(covered >= 25, "{covered} expressible invalid records");
    assert_eq!(row_counts(&store), (0, 0));

    // Oversized payloads and batches refuse from sizes, and a refusal in the
    // middle of a batch writes none of it.
    let good = Owned::from_json(&fixtures["records"][0]);
    let mut big = good.clone();
    big.id = "big".to_string();
    big.identity
        .iter_mut()
        .find(|(name, _)| name == "message_id")
        .unwrap()
        .1 = "msg-big".to_string();
    big.payload = "x".repeat(5000);
    let tight = PersistBounds {
        max_payload_bytes: NonZeroUsize::new(4096).unwrap(),
        ..bounds()
    };
    let good_identity = borrowed(&good.identity);
    let big_identity = borrowed(&big.identity);
    let result = store
        .with_conn_fenced(|conn| {
            Ok(persist_occurrences(
                conn,
                &[good.record(&good_identity), big.record(&big_identity)],
                tight,
                1,
            ))
        })
        .unwrap();
    assert_eq!(
        result,
        Err(ProjectionError::OverBound {
            index: 1,
            bound: "payload_bytes",
            size: 5000,
        })
    );
    let result = store
        .with_conn_fenced(|conn| {
            Ok(persist_occurrences(
                conn,
                &[good.record(&good_identity), big.record(&big_identity)],
                PersistBounds {
                    max_records: NonZeroUsize::new(1).unwrap(),
                    ..bounds()
                },
                1,
            ))
        })
        .unwrap();
    assert_eq!(result, Err(ProjectionError::TooManyRecords { count: 2 }));
    let rows: i64 = store
        .with_conn(|conn| conn.query_row("SELECT COUNT(*) FROM occurrences", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(
        rows, 0,
        "the good record was not written beside the refused one"
    );

    // An oversized tuple encoding and a non-positive commit sequence refuse
    // before anything is written.
    let result = store
        .with_conn_fenced(|conn| {
            Ok(persist_occurrences(
                conn,
                &[good.record(&good_identity)],
                PersistBounds {
                    max_tuple_bytes: NonZeroUsize::new(64).unwrap(),
                    ..bounds()
                },
                1,
            ))
        })
        .unwrap();
    assert!(
        matches!(
            result,
            Err(ProjectionError::OverBound {
                index: 0,
                bound: "tuple_bytes",
                size
            }) if size > 64
        ),
        "{result:?}"
    );
    let mut zero_seq = good.record(&good_identity);
    zero_seq.created_commit_seq = 0;
    let result = store
        .with_conn_fenced(|conn| Ok(persist_occurrences(conn, &[zero_seq], bounds(), 1)))
        .unwrap();
    assert_eq!(
        result,
        Err(ProjectionError::NonPositiveSequence { index: 0 })
    );
    assert_eq!(
        row_counts(&store),
        (0, 0),
        "no payload row precedes a refused occurrence"
    );

    // A span that is not a UTF-8 boundary or runs past the buffer is refused
    // by the span check, not by a slice panic.
    let mut cut = good.clone();
    cut.payload = "héllo".to_string();
    cut.span = Some(Span { start: 0, end: 2 });
    let cut_identity = borrowed(&cut.identity);
    let result = store
        .with_conn_fenced(|conn| {
            Ok(persist_occurrences(
                conn,
                &[cut.record(&cut_identity)],
                bounds(),
                1,
            ))
        })
        .unwrap();
    assert_eq!(
        result,
        Err(ProjectionError::Occurrence(
            OccurrenceRefusal::SpanNotUtf8Aligned
        ))
    );

    let mut results = Vec::new();
    let mut counts = Vec::new();
    for (span, text) in [
        (Span { start: 2, end: 1 }, ""),
        (
            Span {
                start: i64::MAX as u64,
                end: i64::MAX as u64 + 1,
            },
            "x",
        ),
        (
            Span {
                start: i64::MAX as u64 + 1,
                end: i64::MAX as u64 + 1,
            },
            "",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path());
        let mut selected = good.record(&good_identity);
        selected.occurrence.span = Some(span);
        selected.payload = Payload::Selected(text);
        results.push(
            store
                .with_conn_fenced(|conn| {
                    Ok(persist_occurrences(
                        conn,
                        &[good.record(&good_identity), selected],
                        bounds(),
                        1,
                    ))
                })
                .unwrap(),
        );
        counts.push(
            store
                .with_conn(|conn| {
                    Ok((
                        conn.query_row("SELECT COUNT(*) FROM occurrences", [], |r| {
                            r.get::<_, i64>(0)
                        })?,
                        conn.query_row("SELECT COUNT(*) FROM payloads", [], |r| {
                            r.get::<_, i64>(0)
                        })?,
                    ))
                })
                .unwrap(),
        );
    }
    assert_eq!(
        counts,
        [(0, 0); 3],
        "committing a handled span error writes neither the earlier valid row nor any payload"
    );
    assert_eq!(
        results,
        [
            Err(ProjectionError::Occurrence(OccurrenceRefusal::SpanReversed)),
            Err(ProjectionError::CorruptRow),
            Err(ProjectionError::CorruptRow),
        ]
    );

    for span in [
        Span { start: 0, end: 6 },
        Span {
            start: i64::MAX as u64 - 6,
            end: i64::MAX as u64,
        },
    ] {
        let mut selected = good.record(&good_identity);
        selected.occurrence.span = Some(span);
        selected.payload = Payload::Selected("héllo");
        let expected =
            kernel::source_identity::encode_preserving_span(&selected.occurrence).unwrap();
        store
            .with_conn_fenced(|conn| {
                let persisted = persist_occurrences(conn, &[selected], bounds(), 1).unwrap();
                assert_eq!(persisted[0].occurrence_id, expected.occurrence_id);
                let stored = read_occurrence(conn, &expected.occurrence_id)
                    .unwrap()
                    .unwrap();
                assert_eq!(stored.span, Some((span.start, span.end)));
                assert_eq!(stored.bytes, "héllo".as_bytes());
                Ok(())
            })
            .unwrap();
    }
}

#[test]
fn a_different_identity_cannot_be_installed_over_an_existing_projection() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    store
        .with_conn_fenced(|conn| {
            for schema_version in [
                retrieval::SCHEMA_VERSION - 1,
                retrieval::SCHEMA_VERSION + 1,
                0,
                u32::MAX,
            ] {
                let mut wrong_version = identity();
                wrong_version.schema_version = schema_version;
                assert_eq!(
                    install_identity(conn, &wrong_version, 1),
                    Err(ProjectionError::IdentityMismatch),
                    "an empty projection must reject schema version {schema_version}"
                );
                assert_eq!(read_identity(conn).unwrap(), None);
            }
            install_identity(conn, &identity(), 1).unwrap();
            install_identity(conn, &identity(), 2).unwrap();
            let mut other = identity();
            other.tokenizer_fingerprint = "fp-b".to_string();
            assert_eq!(
                install_identity(conn, &other, 3),
                Err(ProjectionError::IdentityMismatch)
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn replay_with_different_immutable_metadata_is_a_collision_not_a_noop() {
    let fixtures = fixtures();
    let record = Owned::from_json(&fixtures["records"][0]);
    let identity = borrowed(&record.identity);
    let occurrence_id = occurrence_id_of(&record);
    let original = record.record(&identity);
    let mut fresh = record.clone();
    fresh
        .identity
        .iter_mut()
        .find(|(name, _)| name == "message_id")
        .unwrap()
        .1 = "msg-fresh".to_string();
    fresh.payload = "unrelated payload".to_string();
    let fresh_identity = borrowed(&fresh.identity);

    // The preflight rejects metadata variants that conflict with either a
    // stored row or an earlier batch record.
    let variants: Vec<OccurrenceRecord<'_>> = vec![
        OccurrenceRecord {
            sensitivity: Sensitivity::Secret,
            ..record.record(&identity)
        },
        OccurrenceRecord {
            domain_id: "domain-other",
            ..record.record(&identity)
        },
        OccurrenceRecord {
            source_object_id: "other-object",
            ..record.record(&identity)
        },
        OccurrenceRecord {
            source_evidence_id: "other-evidence",
            ..record.record(&identity)
        },
        OccurrenceRecord {
            source_artifact_digest: "1111111111111111111111111111111111111111111111111111111111111111",
            ..record.record(&identity)
        },
        OccurrenceRecord {
            created_commit_seq: 8,
            ..record.record(&identity)
        },
    ];
    for stored in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path());
        if stored {
            store
                .with_conn_fenced(|conn| {
                    Ok(
                        persist_occurrences(conn, std::slice::from_ref(&original), bounds(), 1)
                            .unwrap(),
                    )
                })
                .unwrap();
        }
        let before = store
            .with_conn(|conn| Ok(read_occurrence(conn, &occurrence_id).unwrap()))
            .unwrap();
        assert_eq!(before.is_some(), stored);
        if let Some(row) = &before {
            assert_eq!(row.sensitivity, Sensitivity::Normal);
            assert_eq!(row.created_commit_seq, 7);
        }
        for (index, variant) in variants.iter().enumerate() {
            assert_eq!(variant.occurrence, original.occurrence);
            assert!(matches!(
                (variant.payload, original.payload),
                (Payload::Whole(a), Payload::Whole(b)) if a == b
            ));
            let mut requests = vec![fresh.record(&fresh_identity)];
            if !stored {
                requests.push(original.clone());
            }
            requests.push(variant.clone());
            store
                .with_conn_fenced(|conn| {
                    let changes_before: i64 =
                        conn.query_row("SELECT total_changes()", [], |row| row.get(0))?;
                    let result = persist_occurrences(conn, &requests, bounds(), 2);
                    let changes_after: i64 =
                        conn.query_row("SELECT total_changes()", [], |row| row.get(0))?;
                    assert_eq!(
                        result,
                        Err(ProjectionError::OccurrenceCollision {
                            occurrence_id: occurrence_id.clone()
                        }),
                        "metadata variant {index}, stored {stored}"
                    );
                    assert_eq!(
                        changes_after, changes_before,
                        "preflight must not mutate rows"
                    );
                    assert_eq!(read_occurrence(conn, &occurrence_id).unwrap(), before);
                    Ok(())
                })
                .unwrap();
            assert_eq!(row_counts(&store), (i64::from(stored), i64::from(stored)));
        }
    }
}

/// A stored row whose tuple bytes, derived columns, or tombstone disagree with
/// the retained tuple reads as corruption rather than as a served identity.
#[test]
fn stored_rows_that_disagree_with_their_tuple_are_corruption_not_served_identity() {
    type Damage = Box<dyn Fn(&rusqlite::Connection, &str) -> usize>;
    fn execute(sql: &'static str) -> Damage {
        Box::new(
            move |raw: &rusqlite::Connection, occurrence_id: &str| -> usize {
                raw.execute(sql, [occurrence_id]).unwrap()
            },
        )
    }
    // A same-length bit flip keeps the tuple's size; the digest relation
    // detects it.
    fn flip_last_tuple_byte(raw: &rusqlite::Connection, occurrence_id: &str) -> usize {
        let mut tuple: Vec<u8> = raw
            .query_row(
                "SELECT tuple FROM occurrences WHERE occurrence_id=?1",
                [occurrence_id],
                |row| row.get(0),
            )
            .unwrap();
        let original = tuple.clone();
        *tuple.last_mut().unwrap() ^= 1;
        assert_eq!(tuple.len(), original.len());
        assert_ne!(tuple, original);
        raw.execute(
            "UPDATE occurrences SET tuple=?2 WHERE occurrence_id=?1",
            rusqlite::params![occurrence_id, tuple],
        )
        .unwrap()
    }

    let fixtures = fixtures();
    let record = Owned::from_json(&fixtures["records"][0]);
    let with_span = Owned::from_json(
        fixtures["records"]
            .as_array()
            .unwrap()
            .iter()
            .find(|value| value["id"] == "t_span")
            .unwrap(),
    );

    // The damaged columns stay schema-valid but disagree with the retained
    // tuple. The writer refuses a tombstone at or before `created_commit_seq`
    // (7); both boundary values must also be refused when found already stored.
    let damages: Vec<(&str, Damage)> = vec![
        ("tuple_bit_flip", Box::new(flip_last_tuple_byte) as Damage),
        (
            "class",
            execute("UPDATE occurrences SET class='git_commits' WHERE occurrence_id=?1"),
        ),
        (
            "revision",
            execute("UPDATE occurrences SET revision=9 WHERE occurrence_id=?1"),
        ),
        (
            "representation",
            execute("UPDATE occurrences SET representation='tool_output' WHERE occurrence_id=?1"),
        ),
        (
            "lineage_id",
            execute(
                "UPDATE occurrences SET lineage_id='0000000000000000000000000000000000000000000000000000000000000000' WHERE occurrence_id=?1",
            ),
        ),
        (
            "span_added",
            execute("UPDATE occurrences SET span_start=0, span_end=5 WHERE occurrence_id=?1"),
        ),
        (
            "span_removed",
            execute("UPDATE occurrences SET span_start=NULL, span_end=NULL WHERE occurrence_id=?1"),
        ),
        (
            "span_shifted",
            execute("UPDATE occurrences SET span_end=span_end-1 WHERE occurrence_id=?1"),
        ),
        (
            "tombstone_at_creation",
            execute(
                "INSERT INTO occurrence_tombstones(occurrence_id,invalidated_commit_seq,reason,recorded_at)
                 VALUES (?1,7,'retired',1)",
            ),
        ),
        (
            "tombstone_before_creation",
            execute(
                "INSERT INTO occurrence_tombstones(occurrence_id,invalidated_commit_seq,reason,recorded_at)
                 VALUES (?1,3,'retired',1)",
            ),
        ),
    ];
    for (damage, apply_damage) in &damages {
        let target = match *damage {
            "span_removed" | "span_shifted" => &with_span,
            _ => &record,
        };
        let identity = borrowed(&target.identity);
        let dir = tempfile::tempdir().unwrap();
        let outcome = {
            let store = open(dir.path());
            store
                .with_conn_fenced(|conn| {
                    Ok(
                        persist_occurrences(conn, &[target.record(&identity)], bounds(), 1)
                            .unwrap()
                            .remove(0),
                    )
                })
                .unwrap()
        };
        // Corruption injection uses a raw connection; the guarded store is closed.
        {
            let raw = rusqlite::Connection::open(dir.path().join("search/search.sqlite")).unwrap();
            let changed = apply_damage(&raw, outcome.occurrence_id.as_str());
            assert_eq!(changed, 1, "{damage}");
        }
        let store = open(dir.path());
        store
            .with_conn(|conn| {
                assert_eq!(
                    read_occurrence(conn, &outcome.occurrence_id),
                    Err(ProjectionError::CorruptRow),
                    "{damage}"
                );
                Ok(())
            })
            .unwrap();
    }
}
