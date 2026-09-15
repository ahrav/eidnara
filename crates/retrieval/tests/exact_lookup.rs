//! Exact key pages must agree with an inventory built before any row exists.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;
use std::path::Path;

use kernel::Sensitivity;
use kernel::applicability::EvalBudget;
use kernel::source_identity::{Occurrence, OccurrenceClass};
use retrieval::batch::{
    BatchBounds, BatchFault, BatchStatus, Invalidation, MutationIdentity, ProjectionBatch,
    apply_batch, apply_batch_with_fault_for_test, batch_status,
};
use retrieval::exact::{
    Cursor, ExactQuery, HexPrefix, LookupContext, LookupRefusal, ObjectFormat, Page,
    ShaPrefixQuery, ShaQueryRefusal, page,
};
use retrieval::{
    OccurrenceRecord, Payload, PersistBounds, ProjectionError, ProjectionIdentity, Tombstone,
    TombstoneReason, install_identity, message_cleanup,
};
use storage::{
    GuardedConn, Isolation, SqliteStore, StorageBackend, StorageDescriptor, open_sqlite,
};

const KERNEL: &str = "kernel-1";
const HOLD: &str = "hold-1";

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

fn identity() -> ProjectionIdentity {
    ProjectionIdentity {
        schema_version: retrieval::SCHEMA_VERSION,
        kernel_incarnation_id: KERNEL.to_string(),
        projection_policy_version: "source-policy.v1".to_string(),
        identity_contract_version: "search-projection-identity-v3".to_string(),
        limit_manifest_protocol_version: "limits.v1".to_string(),
        embedding_model: "model-a".to_string(),
        tokenizer_fingerprint: "fp-a".to_string(),
        vector_dimension: 8,
        generation_epoch: 1,
    }
}

fn bounds() -> BatchBounds {
    BatchBounds {
        persist: PersistBounds {
            max_records: NonZeroUsize::new(256).unwrap(),
            max_payload_bytes: NonZeroUsize::new(4096).unwrap(),
            max_tuple_bytes: NonZeroUsize::new(2048).unwrap(),
        },
        max_source_bytes: NonZeroUsize::new(1 << 20).unwrap(),
        max_local_mutations: NonZeroUsize::new(512).unwrap(),
        max_pending: NonZeroUsize::new(512).unwrap(),
    }
}

fn mutation(snapshot: i64, through: i64) -> MutationIdentity {
    MutationIdentity {
        kernel_incarnation_id: KERNEL.to_string(),
        hold_id: HOLD.to_string(),
        snapshot_commit_seq: snapshot,
        through_commit_seq: through,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Source {
    Commit {
        repository: &'static str,
        format: ObjectFormat,
        oid: String,
        revision: i64,
    },
    Claim {
        object_id: &'static str,
        revision: i64,
        representation: &'static str,
    },
    Memory {
        object_id: &'static str,
    },
    Message {
        message_id: &'static str,
    },
}

impl Source {
    fn class(&self) -> &'static str {
        match self {
            Self::Commit { .. } => "git_commits",
            Self::Claim { .. } => "canonical_claims",
            Self::Memory { .. } => "promoted_memory",
            Self::Message { .. } => "messages",
        }
    }

    fn identity(&self) -> Vec<(String, String)> {
        match self {
            Self::Commit {
                repository,
                format,
                oid,
                ..
            } => vec![
                ("repository_id".into(), (*repository).into()),
                ("object_format".into(), format.code().into()),
                ("oid".into(), oid.clone()),
            ],
            Self::Claim { object_id, .. } => vec![("object_id".into(), (*object_id).into())],
            Self::Memory { object_id } => {
                vec![("decision_object_id".into(), (*object_id).into())]
            }
            Self::Message { message_id } => vec![
                ("project_id".into(), "proj-a".into()),
                ("harness".into(), "opencode".into()),
                ("session_id".into(), "sess-01".into()),
                ("message_id".into(), (*message_id).into()),
                ("block_index".into(), "0".into()),
            ],
        }
    }

    fn revision(&self) -> String {
        match self {
            Self::Commit { revision, .. } | Self::Claim { revision, .. } => revision.to_string(),
            Self::Memory { .. } | Self::Message { .. } => "1".to_string(),
        }
    }

    fn representation(&self) -> &'static str {
        match self {
            Self::Commit { .. } => "commit_message",
            Self::Claim { representation, .. } => representation,
            Self::Memory { .. } => "summary",
            Self::Message { .. } => "text",
        }
    }

    fn query(&self) -> Option<QueryKey> {
        match self {
            Self::Commit {
                repository,
                format,
                oid,
                ..
            } => Some(QueryKey::Sha {
                repository,
                format: *format,
                oid: oid.clone(),
            }),
            Self::Claim { object_id, .. } | Self::Memory { object_id } => {
                Some(QueryKey::Object(object_id))
            }
            Self::Message { .. } => None,
        }
    }

    fn text(&self) -> String {
        format!("{self:?}")
    }
}

type SortKey = (u8, &'static str, String, Vec<u8>);

#[derive(Debug, Clone, PartialEq, Eq)]
enum QueryKey {
    Object(&'static str),
    Sha {
        repository: &'static str,
        format: ObjectFormat,
        oid: String,
    },
}

impl QueryKey {
    fn key_bytes(&self) -> Vec<u8> {
        match self {
            Self::Object(object_id) => object_id.as_bytes().to_vec(),
            Self::Sha { oid, .. } => oid.as_bytes().to_vec(),
        }
    }

    fn sort_key(&self) -> SortKey {
        match self {
            Self::Object(_) => (0, "", String::new(), self.key_bytes()),
            Self::Sha {
                repository, format, ..
            } => (1, repository, format.code().to_string(), self.key_bytes()),
        }
    }

    fn run(&self, store: &SqliteStore, page_rows: usize) -> (Vec<Page>, usize) {
        match self {
            Self::Object(object_id) => all_pages(store, page_rows, |conn, cursor| {
                page(
                    conn,
                    &context(page_rows),
                    &ExactQuery::CanonicalObject(object_id.as_bytes()),
                    cursor,
                )
            }),
            Self::Sha {
                repository,
                format,
                oid,
            } => {
                let prefix = HexPrefix::parse(oid).unwrap();
                let query = ShaPrefixQuery::bind(repository, *format, &prefix).unwrap();
                all_pages(store, page_rows, |conn, cursor| {
                    page(
                        conn,
                        &context(page_rows),
                        &ExactQuery::Sha(query.clone()),
                        cursor,
                    )
                })
            }
        }
    }
}

struct Arena {
    identities: Vec<Vec<(String, String)>>,
    revisions: Vec<String>,
    texts: Vec<String>,
}

impl Arena {
    fn new(sources: &[Source]) -> Self {
        Self {
            identities: sources.iter().map(Source::identity).collect(),
            revisions: sources.iter().map(Source::revision).collect(),
            texts: sources.iter().map(Source::text).collect(),
        }
    }

    fn borrow(&self) -> Vec<Vec<(&str, &str)>> {
        self.identities
            .iter()
            .map(|fields| {
                fields
                    .iter()
                    .map(|(n, v)| (n.as_str(), v.as_str()))
                    .collect()
            })
            .collect()
    }
}

fn records<'a>(
    sources: &'a [Source],
    arena: &'a Arena,
    borrowed: &'a [Vec<(&'a str, &'a str)>],
    created: i64,
) -> Vec<OccurrenceRecord<'a>> {
    sources
        .iter()
        .enumerate()
        .map(|(index, source)| OccurrenceRecord {
            occurrence: Occurrence {
                class: source.class(),
                identity: &borrowed[index],
                revision: &arena.revisions[index],
                representation: source.representation(),
                span: None,
            },
            payload: Payload::Whole(&arena.texts[index]),
            domain_id: "domain",
            sensitivity: Sensitivity::Normal,
            source_object_id: &arena.texts[index],
            source_evidence_id: "evidence",
            source_artifact_digest: "0000000000000000000000000000000000000000000000000000000000000000",
            created_commit_seq: created,
        })
        .collect()
}

fn encoded(source: &Source) -> kernel::source_identity::EncodedOccurrence {
    let identity = source.identity();
    let borrowed: Vec<(&str, &str)> = identity
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_str()))
        .collect();
    let revision = source.revision();
    kernel::source_identity::encode_preserving_span(&Occurrence {
        class: source.class(),
        identity: &borrowed,
        revision: &revision,
        representation: source.representation(),
        span: None,
    })
    .unwrap()
}

fn occurrence_id(source: &Source) -> String {
    encoded(source).occurrence_id
}

fn apply(
    store: &SqliteStore,
    sources: &[Source],
    identity: MutationIdentity,
    invalidations: Vec<Invalidation>,
    now: i64,
) -> Result<retrieval::batch::BatchOutcome, ProjectionError> {
    let arena = Arena::new(sources);
    let borrowed = arena.borrow();
    let created = identity.through_commit_seq;
    let batch = ProjectionBatch {
        identity,
        records: records(sources, &arena, &borrowed, created),
        invalidations,
        generation_id: None,
    };
    let mut outcome = None;
    let _ = store.with_conn_fenced(|conn| {
        let result = apply_batch(conn, &batch, bounds(), now);
        let failed = result.is_err();
        outcome = Some(result);
        if failed {
            Err(rusqlite::Error::QueryReturnedNoRows)
        } else {
            Ok(())
        }
    });
    outcome.unwrap()
}

fn setup(store: &SqliteStore) {
    store
        .with_conn_fenced(|conn| {
            install_identity(conn, &identity(), 1).unwrap();
            Ok(())
        })
        .unwrap();
}

fn context(page_rows: usize) -> LookupContext<'static> {
    static BUDGET: std::sync::OnceLock<EvalBudget> = std::sync::OnceLock::new();
    LookupContext {
        kernel_incarnation_id: KERNEL,
        page_rows: NonZeroUsize::new(page_rows).unwrap(),
        budget: BUDGET.get_or_init(EvalBudget::unbounded),
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Expected {
    targets: BTreeSet<String>,
    occurrences: BTreeSet<String>,
}

fn inventory(sources: &[Source]) -> Vec<(QueryKey, Expected)> {
    let mut inventory: BTreeMap<SortKey, (QueryKey, Expected)> = BTreeMap::new();
    for source in sources {
        let Some(key) = source.query() else { continue };
        let entry = inventory
            .entry(key.sort_key())
            .or_insert_with(|| (key.clone(), Expected::default()));
        entry.1.occurrences.insert(occurrence_id(source));
        entry.1.targets.insert(match source {
            Source::Commit { .. } => encoded(source).lineage_id,
            Source::Claim { object_id, .. } | Source::Memory { object_id } => (*object_id).into(),
            Source::Message { .. } => unreachable!(),
        });
    }
    inventory.into_values().collect()
}

fn all_pages(
    store: &SqliteStore,
    page_rows: usize,
    mut fetch: impl FnMut(&GuardedConn<'_>, Option<&Cursor>) -> Result<Page, LookupRefusal>,
) -> (Vec<Page>, usize) {
    let mut pages = Vec::new();
    let mut cursor = None;
    let mut distinct = 0;
    loop {
        let page = store
            .with_conn(|conn| Ok(fetch(conn, cursor.as_ref())))
            .unwrap()
            .unwrap();
        assert!(page.rows.len() <= page_rows);
        distinct += page.distinct_keys;
        cursor = page.next.clone();
        pages.push(page);
        if cursor.is_none() {
            return (pages, distinct);
        }
    }
}

fn hex(prefix: &str) -> HexPrefix {
    HexPrefix::parse(prefix).unwrap()
}

fn oid40(prefix: &str, fill: char) -> String {
    format!("{prefix}{}", fill.to_string().repeat(40 - prefix.len()))
}

fn oid64(prefix: &str, fill: char) -> String {
    format!("{prefix}{}", fill.to_string().repeat(64 - prefix.len()))
}

fn sibling40() -> String {
    let mut oid = oid40("abc1", '0');
    oid.pop();
    oid.push('1');
    oid
}

fn commit(repository: &'static str, format: ObjectFormat, oid: String, revision: i64) -> Source {
    Source::Commit {
        repository,
        format,
        oid,
        revision,
    }
}

fn claim(object_id: &'static str, revision: i64, representation: &'static str) -> Source {
    Source::Claim {
        object_id,
        revision,
        representation,
    }
}

fn corpus() -> Vec<Source> {
    vec![
        commit("repo-a", ObjectFormat::Sha1, oid40("abc1", '0'), 1),
        commit("repo-a", ObjectFormat::Sha1, oid40("abc1", '0'), 2),
        commit("repo-a", ObjectFormat::Sha1, oid40("abc2", '0'), 1),
        commit("repo-a", ObjectFormat::Sha1, sibling40(), 1),
        commit("repo-a", ObjectFormat::Sha1, oid40("abd", '0'), 1),
        commit("repo-a", ObjectFormat::Sha1, oid40("f", 'f'), 1),
        commit("repo-a", ObjectFormat::Sha1, oid40("0", '0'), 1),
        commit("repo-b", ObjectFormat::Sha1, oid40("abc1", '0'), 1),
        commit("repo-c", ObjectFormat::Sha256, oid64("abc1", '0'), 1),
        commit("repo-c", ObjectFormat::Sha256, oid64("abc", '1'), 1),
        claim("obj-1", 1, "decision_summary"),
        claim("obj-1", 1, "rationale"),
        Source::Memory { object_id: "obj-1" },
        claim("obj-10", 1, "decision_summary"),
        claim("obj-2", 1, "decision_summary"),
        Source::Message { message_id: "m-1" },
    ]
}

#[test]
fn pages_agree_with_the_inventory_whatever_the_insertion_or_page_order() {
    let sources = corpus();
    let expected = inventory(&sources);
    assert!(expected.len() >= 10, "the inventory covers both families");

    let mut orders = vec![sources.clone()];
    let mut reversed = sources.clone();
    reversed.reverse();
    orders.push(reversed);
    let mut interleaved = sources.clone();
    interleaved.rotate_left(5);
    orders.push(interleaved);

    let mut observed_by_order = Vec::new();
    for order in &orders {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path());
        setup(&store);
        let (first, second) = order.split_at(order.len() / 2);
        let outcome = apply(&store, first, mutation(0, 3), vec![], 10).unwrap();
        assert_eq!(
            outcome.associations_inserted,
            first.iter().filter(|s| s.query().is_some()).count()
        );
        apply(&store, second, mutation(0, 4), vec![], 11).unwrap();

        let mut observed = Vec::new();
        let mut sequences = BTreeMap::new();
        for (key, expected) in &expected {
            let mut got = Expected::default();
            for page_rows in [1usize, 2, 64] {
                let (pages, distinct) = key.run(&store, page_rows);
                assert_eq!(distinct, 1, "one key is one distinct key: {key:?}");
                let rows: Vec<_> = pages.into_iter().flat_map(|page| page.rows).collect();
                let ids: Vec<String> = rows.iter().map(|row| row.occurrence_id.clone()).collect();
                assert_eq!(
                    ids.len(),
                    expected.occurrences.len(),
                    "no row is repeated or dropped across pages: {key:?} {page_rows}"
                );
                let mut sorted = ids.clone();
                sorted.sort();
                assert_eq!(ids, sorted, "rows arrive in stable binary occurrence order");
                assert!(
                    rows.iter()
                        .all(|row| row.key == key.key_bytes() && row.tombstone.is_none())
                );
                sequences
                    .entry((key.sort_key(), page_rows))
                    .or_insert(ids.clone());
                got.occurrences = ids.into_iter().collect();
                got.targets = rows.iter().map(|row| row.target_id.clone()).collect();
            }
            observed.push(got);
        }
        observed_by_order.push((observed, sequences));
    }
    for (observed, _) in &observed_by_order {
        for ((key, expected), got) in expected.iter().zip(observed) {
            assert_eq!(got, expected, "{key:?}");
        }
    }
    let baseline = &observed_by_order[0].1;
    for (_, sequences) in &observed_by_order[1..] {
        assert_eq!(
            sequences, baseline,
            "page sequences are insertion-order independent"
        );
    }

    let alias = expected
        .iter()
        .find(|(key, _)| *key == QueryKey::Object("obj-1"))
        .map(|(_, expected)| expected)
        .unwrap();
    assert_eq!(
        alias.occurrences.len(),
        3,
        "summary, rationale, and memory of one object"
    );
    assert_eq!(alias.targets.len(), 1, "aliases share one target");
    let longer = expected
        .iter()
        .find(|(key, _)| *key == QueryKey::Object("obj-10"))
        .map(|(_, expected)| expected)
        .unwrap();
    assert_eq!(longer.occurrences.len(), 1, "obj-10 is not an obj-1 row");

    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    setup(&store);
    let twins = vec![
        claim("twin-a", 1, "decision_summary"),
        claim("twin-b", 1, "decision_summary"),
    ];
    let arena = Arena {
        identities: twins.iter().map(Source::identity).collect(),
        revisions: twins.iter().map(Source::revision).collect(),
        texts: vec!["same".into(), "same".into()],
    };
    let borrowed = arena.borrow();
    let batch = ProjectionBatch {
        identity: mutation(0, 1),
        records: records(&twins, &arena, &borrowed, 1),
        invalidations: vec![],
        generation_id: None,
    };
    store
        .with_conn_fenced(|conn| {
            let outcome = apply_batch(conn, &batch, bounds(), 1).unwrap();
            assert_eq!(outcome.rows_inserted, 2);
            assert_eq!(outcome.associations_inserted, 2);
            Ok(())
        })
        .unwrap();
    let (a, _) = QueryKey::Object("twin-a").run(&store, 8);
    let (b, _) = QueryKey::Object("twin-b").run(&store, 8);
    let (a, b) = (&a[0].rows[0], &b[0].rows[0]);
    assert_eq!(a.payload_id, b.payload_id, "equal payloads share bytes");
    assert_ne!(a.target_id, b.target_id, "but not a target");
    assert_ne!(a.occurrence_id, b.occurrence_id);
    assert_eq!(a.class, OccurrenceClass::CanonicalClaims);
}

#[test]
fn sha_prefix_enumeration_is_repository_and_algorithm_scoped_and_counts_complete_oids() {
    let sources = corpus();
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    setup(&store);
    apply(&store, &sources, mutation(0, 3), vec![], 10).unwrap();
    let retired = occurrence_id(&sources[2]);
    apply(
        &store,
        &[],
        mutation(0, 4),
        vec![Invalidation {
            occurrence_id: retired.clone(),
            tombstone: Tombstone {
                invalidated_commit_seq: 4,
                reason: TombstoneReason::Retired,
            },
        }],
        11,
    )
    .unwrap();

    let enumerate = |repository: &'static str, format: ObjectFormat, prefix: &str, page_rows| {
        let prefix = hex(prefix);
        let query = ShaPrefixQuery::bind(repository, format, &prefix).unwrap();
        let (pages, distinct) = all_pages(&store, page_rows, |conn, cursor| {
            page(
                conn,
                &context(page_rows),
                &ExactQuery::Sha(query.clone()),
                cursor,
            )
        });
        let rows: Vec<_> = pages.into_iter().flat_map(|page| page.rows).collect();
        (rows, distinct)
    };

    for page_rows in [1usize, 2, 3, 64] {
        let (rows, distinct) = enumerate("repo-a", ObjectFormat::Sha1, "abc", page_rows);
        assert_eq!(rows.len(), 4, "{page_rows}");
        assert_eq!(distinct, 3, "two revisions of one oid are not a collision");
        let keys: BTreeSet<_> = rows.iter().map(|row| row.key.clone()).collect();
        assert_eq!(
            keys,
            [oid40("abc1", '0'), sibling40(), oid40("abc2", '0')]
                .into_iter()
                .map(String::into_bytes)
                .collect()
        );
        let retired_rows: Vec<_> = rows
            .iter()
            .filter(|row| row.occurrence_id == retired)
            .collect();
        assert_eq!(retired_rows.len(), 1);
        assert_eq!(
            retired_rows[0].tombstone,
            Some(Tombstone {
                invalidated_commit_seq: 4,
                reason: TombstoneReason::Retired,
            }),
            "ineligible oids stay in the universe and carry their tombstone"
        );
        assert!(
            rows.iter()
                .all(|row| row.class == OccurrenceClass::GitCommits)
        );
        let mut ordered = rows
            .iter()
            .map(|row| (row.key.clone(), row.occurrence_id.clone()))
            .collect::<Vec<_>>();
        let unsorted = ordered.clone();
        ordered.sort();
        assert_eq!(unsorted, ordered, "key then occurrence order is stable");

        assert_eq!(enumerate("repo-a", ObjectFormat::Sha1, "a", page_rows).1, 4);
        assert_eq!(
            enumerate("repo-a", ObjectFormat::Sha1, "abc10", page_rows).1,
            2
        );
        assert_eq!(enumerate("repo-a", ObjectFormat::Sha1, "0", page_rows).1, 1);
        assert_eq!(enumerate("repo-a", ObjectFormat::Sha1, "f", page_rows).1, 1);
        assert_eq!(
            enumerate("repo-a", ObjectFormat::Sha1, &"f".repeat(40), page_rows).1,
            1
        );
        assert_eq!(enumerate("repo-a", ObjectFormat::Sha1, "e", page_rows).1, 0);
        let (full, distinct) =
            enumerate("repo-a", ObjectFormat::Sha1, &oid40("abc1", '0'), page_rows);
        assert_eq!(
            full.len(),
            2,
            "a full-width oid excludes its 39-nibble sibling"
        );
        assert_eq!(distinct, 1);
        assert_eq!(
            enumerate("repo-a", ObjectFormat::Sha1, &sibling40(), page_rows).1,
            1
        );
        assert_eq!(
            enumerate("repo-a", ObjectFormat::Sha1, "ABC", page_rows).1,
            3
        );
        assert_eq!(
            enumerate("repo-b", ObjectFormat::Sha1, "abc", page_rows).1,
            1
        );
        assert_eq!(
            enumerate("repo-c", ObjectFormat::Sha256, "abc", page_rows).1,
            2
        );
        assert_eq!(
            enumerate("repo-c", ObjectFormat::Sha256, "abc1", page_rows).1,
            2
        );
        assert_eq!(
            enumerate("repo-c", ObjectFormat::Sha256, "abc10", page_rows).1,
            1
        );
        assert_eq!(
            enumerate("repo-a", ObjectFormat::Sha256, "abc", page_rows).1,
            0
        );
        assert_eq!(
            enumerate("repo-c", ObjectFormat::Sha1, "abc", page_rows).1,
            0
        );
        assert_eq!(
            enumerate("repo-none", ObjectFormat::Sha1, "abc", page_rows).1,
            0
        );
    }
    assert_eq!(
        ShaPrefixQuery::bind("repo-a", ObjectFormat::Sha1, &hex(&"a".repeat(41))).unwrap_err(),
        ShaQueryRefusal::PrefixTooLong {
            digits: 41,
            format: ObjectFormat::Sha1
        }
    );
    assert!(ShaPrefixQuery::bind("repo-c", ObjectFormat::Sha256, &hex(&"a".repeat(41))).is_ok());
}

#[test]
fn bounds_stale_cursors_interruption_and_missing_context_report_incompleteness() {
    let sources = corpus();
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let query = ExactQuery::CanonicalObject(b"obj-1");
    store
        .with_conn(|conn| {
            assert_eq!(
                page(conn, &context(2), &query, None).unwrap_err(),
                LookupRefusal::Projection(ProjectionError::IdentityMismatch),
                "no installed identity"
            );
            Ok(())
        })
        .unwrap();
    setup(&store);
    store
        .with_conn(|conn| {
            assert_eq!(
                page(conn, &context(2), &query, None).unwrap_err(),
                LookupRefusal::NoCheckpoint,
                "no applied prefix means no universe"
            );
            Ok(())
        })
        .unwrap();
    apply(&store, &sources, mutation(0, 3), vec![], 10).unwrap();

    let first = store
        .with_conn(|conn| {
            let page = page(conn, &context(2), &query, None).unwrap();
            assert_eq!(page.rows.len(), 2);
            assert!(
                page.next.is_some(),
                "a bound before exhaustion is incomplete"
            );
            assert_eq!(page.checkpoint.checkpoint_commit_seq, 3);
            Ok(page)
        })
        .unwrap();
    let cursor = first.next.clone().unwrap();

    store
        .with_conn(|conn| {
            assert_eq!(
                page(
                    conn,
                    &context(2),
                    &ExactQuery::CanonicalObject(b"obj-2"),
                    Some(&cursor)
                )
                .unwrap_err(),
                LookupRefusal::ForeignCursor
            );
            let other_kernel = LookupContext {
                kernel_incarnation_id: "kernel-2",
                ..context(2)
            };
            assert_eq!(
                page(conn, &other_kernel, &query, Some(&cursor)).unwrap_err(),
                LookupRefusal::Projection(ProjectionError::IdentityMismatch)
            );
            let budget = EvalBudget::unbounded();
            budget.cancel();
            let cancelled = LookupContext {
                budget: &budget,
                ..context(2)
            };
            assert_eq!(
                page(conn, &cancelled, &query, Some(&cursor)).unwrap_err(),
                LookupRefusal::BudgetExhausted
            );
            Ok(())
        })
        .unwrap();

    store
        .with_conn(|conn| {
            let page = page(conn, &context(2), &query, Some(&cursor)).unwrap();
            assert_eq!(page.rows.len(), 1);
            assert!(page.next.is_none());
            assert_eq!(
                page.distinct_keys, 0,
                "the key was first seen on the earlier page"
            );
            Ok(())
        })
        .unwrap();

    apply(&store, &[], mutation(0, 4), vec![], 11).unwrap();
    store
        .with_conn(|conn| {
            assert_eq!(
                page(conn, &context(2), &query, Some(&cursor)).unwrap_err(),
                LookupRefusal::StaleCursor {
                    cursor: 3,
                    current: 4
                },
                "an advanced checkpoint retires the cursor"
            );
            Ok(())
        })
        .unwrap();

    store
        .with_conn_fenced(|conn| {
            conn.execute(
                "UPDATE exact_associations SET extraction_version=99 WHERE key=CAST('obj-1' AS BLOB)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    store
        .with_conn(|conn| {
            assert_eq!(
                page(conn, &context(8), &query, None).unwrap_err(),
                LookupRefusal::ExtractionMismatch {
                    stored: 99,
                    expected: retrieval::exact::EXTRACTION_VERSION,
                },
                "rows from another extraction version are never read as keys"
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn a_page_refuses_the_same_damaged_rows_as_the_occurrence_reader() {
    let sources = vec![
        claim("obj-1", 1, "decision_summary"),
        claim("obj-1", 1, "rationale"),
    ];
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    setup(&store);
    apply(&store, &sources, mutation(0, 5), vec![], 10).unwrap();
    let damaged = occurrence_id(&sources[0]);
    let query = ExactQuery::CanonicalObject(b"obj-1");
    for (name, invalidated_commit_seq) in [("at creation", 5), ("before creation", 3)] {
        store
            .with_conn_fenced(|conn| {
                conn.execute(
                    "INSERT OR REPLACE INTO occurrence_tombstones(
                         occurrence_id,invalidated_commit_seq,reason,recorded_at
                     ) VALUES (?1,?2,'retired',1)",
                    rusqlite::params![damaged, invalidated_commit_seq],
                )?;
                Ok(())
            })
            .unwrap();
        store
            .with_conn(|conn| {
                assert_eq!(
                    retrieval::read_occurrence(conn, &damaged).unwrap_err(),
                    ProjectionError::CorruptRow,
                    "tombstone {name}: the occurrence reader refuses the row"
                );
                assert_eq!(
                    page(conn, &context(8), &query, None).unwrap_err(),
                    LookupRefusal::Projection(ProjectionError::CorruptRow),
                    "tombstone {name}: the page reader applies the same rule"
                );
                Ok(())
            })
            .unwrap();
    }
    store
        .with_conn_fenced(|conn| {
            conn.execute(
                "DELETE FROM occurrence_tombstones WHERE occurrence_id=?1",
                [&damaged],
            )?;
            Ok(())
        })
        .unwrap();
    for (name, sql) in [
        (
            "lineage",
            "UPDATE occurrences SET lineage_id='elsewhere' WHERE occurrence_id=?1",
        ),
        (
            "tuple",
            "UPDATE occurrences SET tuple=x'00' WHERE occurrence_id=?1",
        ),
    ] {
        let original: (String, Vec<u8>) = store
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT lineage_id,tuple FROM occurrences WHERE occurrence_id=?1",
                    [&damaged],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
            })
            .unwrap();
        store
            .with_conn_fenced(|conn| {
                conn.execute(sql, [&damaged])?;
                Ok(())
            })
            .unwrap();
        store
            .with_conn(|conn| {
                assert_eq!(
                    retrieval::read_occurrence(conn, &damaged).unwrap_err(),
                    ProjectionError::CorruptRow,
                    "{name}: the occurrence reader refuses the row"
                );
                assert_eq!(
                    page(conn, &context(8), &query, None).unwrap_err(),
                    LookupRefusal::Projection(ProjectionError::CorruptRow),
                    "{name}: the page reader applies the same rule"
                );
                Ok(())
            })
            .unwrap();
        store
            .with_conn_fenced(|conn| {
                conn.execute(
                    "UPDATE occurrences SET lineage_id=?2,tuple=?3 WHERE occurrence_id=?1",
                    rusqlite::params![damaged, original.0, original.1],
                )?;
                Ok(())
            })
            .unwrap();
    }
}

fn association_rows(conn: &GuardedConn<'_>) -> Vec<(String, String, Vec<u8>, String, String)> {
    let mut statement = conn
        .prepare(
            "SELECT family,namespace,key,occurrence_id,target_id FROM exact_associations
             ORDER BY family,namespace,key,occurrence_id",
        )
        .unwrap();
    statement
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn associations_replay_revise_and_invalidate_atomically_with_the_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    setup(&store);
    let first = vec![
        claim("obj-1", 1, "decision_summary"),
        commit("repo-a", ObjectFormat::Sha1, oid40("abc", '0'), 1),
        Source::Message { message_id: "m-1" },
    ];
    let arena = Arena::new(&first);
    let borrowed = arena.borrow();
    let batch = ProjectionBatch {
        identity: mutation(0, 2),
        records: records(&first, &arena, &borrowed, 2),
        invalidations: vec![],
        generation_id: None,
    };
    let query = ExactQuery::CanonicalObject(b"obj-1");
    for fault in [
        BatchFault::AfterAdmission,
        BatchFault::AfterRows,
        BatchFault::AfterAssociations,
        BatchFault::AfterTombstones,
        BatchFault::AfterPending,
        BatchFault::AfterCheckpoint,
    ] {
        let result: Result<(), _> = store.with_conn_fenced(|conn| {
            match apply_batch_with_fault_for_test(conn, &batch, bounds(), 5, fault) {
                Err(ProjectionError::Sqlite(text)) if text.contains("injected") => {
                    Err(rusqlite::Error::QueryReturnedNoRows)
                }
                other => panic!("{fault:?}: {other:?}"),
            }
        });
        assert!(result.is_err());
        store
            .with_conn(|conn| {
                assert!(association_rows(conn).is_empty(), "{fault:?}");
                assert_eq!(batch_status(conn, &batch).unwrap(), BatchStatus::NotApplied);
                assert_eq!(
                    page(conn, &context(8), &query, None).unwrap_err(),
                    LookupRefusal::NoCheckpoint,
                    "{fault:?}: no checkpoint claims completeness ahead of its rows"
                );
                Ok(())
            })
            .unwrap();
    }
    let outcome = apply(&store, &first, mutation(0, 2), vec![], 5).unwrap();
    assert_eq!(outcome.rows_inserted, 3);
    assert_eq!(
        outcome.associations_inserted, 2,
        "the message has no exact key"
    );
    let committed = store.with_conn(|conn| Ok(association_rows(conn))).unwrap();
    assert_eq!(committed.len(), 2);

    let replay = apply(&store, &first, mutation(0, 2), vec![], 6).unwrap();
    assert_eq!(replay.rows_replayed, 3);
    assert_eq!(replay.associations_inserted, 0);
    store
        .with_conn(|conn| {
            assert_eq!(association_rows(conn), committed);
            assert_eq!(batch_status(conn, &batch).unwrap(), BatchStatus::Applied);
            Ok(())
        })
        .unwrap();

    store
        .with_conn_fenced(|conn| {
            conn.execute("DELETE FROM exact_associations WHERE family='sha'", [])?;
            Ok(())
        })
        .unwrap();
    store
        .with_conn(|conn| {
            assert_eq!(
                batch_status(conn, &batch).unwrap(),
                BatchStatus::NotApplied,
                "a covering checkpoint without the association rows is not applied"
            );
            Ok(())
        })
        .unwrap();
    let repaired = apply(&store, &first, mutation(0, 2), vec![], 7).unwrap();
    assert_eq!(repaired.associations_inserted, 1);

    store
        .with_conn_fenced(|conn| {
            conn.execute(
                "UPDATE exact_associations SET target_id='elsewhere' WHERE family='id'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        apply(&store, &first, mutation(0, 2), vec![], 8).unwrap_err(),
        ProjectionError::AssociationCollision {
            occurrence_id: occurrence_id(&first[0]),
        },
        "a stored row with another target is refused, never overwritten"
    );
    store
        .with_conn_fenced(|conn| {
            conn.execute(
                "UPDATE exact_associations SET target_id='obj-1',extraction_version=99 WHERE family='id'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        apply(&store, &first, mutation(0, 2), vec![], 8).unwrap_err(),
        ProjectionError::ExtractionVersionMismatch {
            stored: 99,
            expected: retrieval::exact::EXTRACTION_VERSION,
        }
    );
    store
        .with_conn(|conn| {
            assert_eq!(
                batch_status(conn, &batch).unwrap(),
                BatchStatus::NotApplied,
                "rows derived under another mapping do not count as applied"
            );
            Ok(())
        })
        .unwrap();
    store
        .with_conn_fenced(|conn| {
            conn.execute(
                "UPDATE exact_associations SET extraction_version=?1 WHERE family='id'",
                [retrieval::exact::EXTRACTION_VERSION],
            )?;
            Ok(())
        })
        .unwrap();

    let second = vec![claim("obj-1", 2, "decision_summary")];
    let superseded = occurrence_id(&first[0]);
    let outcome = apply(
        &store,
        &second,
        mutation(0, 3),
        vec![Invalidation {
            occurrence_id: superseded.clone(),
            tombstone: Tombstone {
                invalidated_commit_seq: 3,
                reason: TombstoneReason::Superseded,
            },
        }],
        9,
    )
    .unwrap();
    assert_eq!(outcome.associations_inserted, 1);
    assert_eq!(outcome.tombstones_recorded, 1);
    store
        .with_conn(|conn| {
            let page = page(conn, &context(8), &query, None).unwrap();
            assert_eq!(page.rows.len(), 2, "both revisions stay in the key page");
            assert!(page.next.is_none());
            let by_revision: BTreeMap<i64, Option<Tombstone>> = page
                .rows
                .iter()
                .map(|row| (row.revision, row.tombstone))
                .collect();
            assert_eq!(
                by_revision[&1],
                Some(Tombstone {
                    invalidated_commit_seq: 3,
                    reason: TombstoneReason::Superseded,
                })
            );
            assert_eq!(by_revision[&2], None);
            assert!(page.rows.iter().all(|row| row.target_id == "obj-1"));
            Ok(())
        })
        .unwrap();
}

#[test]
fn message_cleanup_reclaims_association_rows_with_their_occurrence() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    setup(&store);
    let message = Source::Message { message_id: "m-1" };
    apply(
        &store,
        std::slice::from_ref(&message),
        mutation(0, 2),
        vec![],
        5,
    )
    .unwrap();
    let message_id = occurrence_id(&message);
    store
        .with_conn_fenced(|conn| {
            conn.execute(
                "INSERT INTO exact_associations(family,namespace,key,occurrence_id,target_id,
                     extraction_version,created_commit_seq)
                 VALUES ('symbol','n',CAST('k' AS BLOB),?1,'t',?2,2)",
                rusqlite::params![message_id, retrieval::exact::EXTRACTION_VERSION],
            )?;
            Ok(())
        })
        .unwrap();
    apply(
        &store,
        &[],
        mutation(0, 3),
        vec![Invalidation {
            occurrence_id: message_id.clone(),
            tombstone: Tombstone {
                invalidated_commit_seq: 3,
                reason: TombstoneReason::Retired,
            },
        }],
        6,
    )
    .unwrap();
    let candidates = store
        .with_conn(|conn| {
            Ok(
                message_cleanup::candidates(conn, KERNEL, 3, None, NonZeroUsize::new(8).unwrap())
                    .unwrap(),
            )
        })
        .unwrap();
    assert_eq!(candidates.candidates.len(), 1);
    store
        .with_conn_fenced(|conn| {
            let reclaimed =
                message_cleanup::reclaim(conn, KERNEL, &candidates.candidates, 3).unwrap();
            assert_eq!(reclaimed.occurrences, 1);
            Ok(())
        })
        .unwrap();
    store
        .with_conn(|conn| {
            assert!(association_rows(conn).is_empty());
            assert!(
                retrieval::read_occurrence(conn, &message_id)
                    .unwrap()
                    .is_none()
            );
            Ok(())
        })
        .unwrap();
}
