//! Selected occurrences retain attribution when sharing payload bytes; groups
//! share class, parent, canonical revision, and representation.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::Path;

use kernel::source_identity::{Occurrence, OccurrenceClass, Span, encode, payload_id};
use kernel::{ArtifactDestination, KernelStore, ProjectScope, Sensitivity};
use retrieval::eligibility::judge_occurrences;
use retrieval::fusion::{IdentityRefusal, OccurrenceId};
use retrieval::packing::{
    Grouping, PayloadRef, Provenance, SelectedOccurrence, fetch_payload, read_selected,
};
use retrieval::{OccurrenceRecord, Payload, PersistBounds, ProjectionError, persist_occurrences};
use storage::{
    GuardedConn, Isolation, SqliteStore, StorageBackend, StorageDescriptor, open_sqlite,
};

const PROJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const MAX: NonZeroUsize = NonZeroUsize::MIN.saturating_add(63);

fn open(dir: &Path) -> SqliteStore {
    open_sqlite(
        &StorageDescriptor {
            module_id: "eidnara-test".to_string(),
            storage_namespace: "search-projection".to_string(),
            isolation: Isolation::Module,
            backend: StorageBackend::Sqlite {
                path: dir.join("search.sqlite").to_string_lossy().into_owned(),
            },
        },
        retrieval::BASELINE,
    )
    .unwrap()
}

fn bounds() -> PersistBounds {
    PersistBounds {
        max_records: MAX,
        max_payload_bytes: NonZeroUsize::new(4096).unwrap(),
        max_tuple_bytes: NonZeroUsize::new(2048).unwrap(),
    }
}

struct ToolSpan {
    call: &'static str,
    revision: &'static str,
    representation: &'static str,
    span: Option<Span>,
    sensitivity: Sensitivity,
    source: &'static str,
}

const BUFFER: &str = "error: line 1\nwarning: shared bytes both spans select\n";

impl ToolSpan {
    fn identity(&self) -> [(&'static str, &'static str); 7] {
        [
            ("project_id", "proj-a"),
            ("harness", "opencode"),
            ("session_id", "sess-01"),
            ("parent_message_id", "msg-002"),
            ("tool_call_id", self.call),
            ("result_revision", "1"),
            ("block_index", "0"),
        ]
    }

    fn occurrence<'a>(&self, identity: &'a [(&'a str, &'a str)]) -> Occurrence<'a> {
        Occurrence {
            class: OccurrenceClass::RawToolSpans.code(),
            identity,
            revision: self.revision,
            representation: self.representation,
            span: self.span,
        }
    }

    fn record<'a>(&'a self, identity: &'a [(&'a str, &'a str)]) -> OccurrenceRecord<'a> {
        OccurrenceRecord {
            occurrence: self.occurrence(identity),
            payload: Payload::Whole(BUFFER),
            domain_id: "domain-stable-id",
            sensitivity: self.sensitivity,
            source_object_id: self.source,
            source_evidence_id: "evidence-of-the-call",
            source_artifact_digest: DIGEST,
            created_commit_seq: 7,
        }
    }

    fn id(&self) -> OccurrenceId {
        let identity = self.identity();
        OccurrenceId::parse(
            &encode(&self.occurrence(&identity), BUFFER)
                .unwrap()
                .occurrence_id,
        )
        .unwrap()
    }

    fn grouping(&self) -> Result<Grouping, IdentityRefusal> {
        let identity = self.identity();
        let encoded = encode(&self.occurrence(&identity), BUFFER).unwrap();
        Grouping::derive(
            &encoded.tuple,
            encoded.class,
            encoded.revision,
            self.representation,
            encoded.span,
        )
    }
}

const fn tool_span(
    call: &'static str,
    revision: &'static str,
    representation: &'static str,
    span: Option<Span>,
    sensitivity: Sensitivity,
    source: &'static str,
) -> ToolSpan {
    ToolSpan {
        call,
        revision,
        representation,
        span,
        sensitivity,
        source,
    }
}

const fn range(start: u64, end: u64) -> Option<Span> {
    Some(Span { start, end })
}

fn persist(conn: &GuardedConn<'_>, spans: &[ToolSpan]) {
    let identities: Vec<_> = spans.iter().map(ToolSpan::identity).collect();
    let records: Vec<OccurrenceRecord<'_>> = spans
        .iter()
        .zip(&identities)
        .map(|(span, identity)| span.record(identity))
        .collect();
    persist_occurrences(conn, &records, bounds(), 1).unwrap();
}

fn counts(conn: &GuardedConn<'_>) -> (i64, i64) {
    let count = |table: &str| -> i64 {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
    };
    (count("payloads"), count("occurrences"))
}

#[test]
fn byte_twins_share_one_payload_row_and_keep_their_own_attribution() {
    let twins = [
        tool_span(
            "call-1",
            "1",
            "tool_output",
            None,
            Sensitivity::Normal,
            "src-1",
        ),
        tool_span(
            "call-2",
            "1",
            "tool_output",
            None,
            Sensitivity::Secret,
            "src-2",
        ),
    ];
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let kernel = KernelStore::open(dir.path().join("kernel")).unwrap();
    let ids = [twins[0].id(), twins[1].id()];
    let selected = store
        .with_conn_fenced(|conn| {
            persist(conn, &twins);
            assert_eq!(counts(conn), (1, 2));
            Ok(read_selected(conn, &ids, MAX).unwrap())
        })
        .unwrap();

    assert_eq!(selected.len(), 2);
    let payload = PayloadRef {
        payload_id: payload_id(BUFFER.as_bytes()),
        byte_length: BUFFER.len() as u64,
    };
    for (twin, read) in twins.iter().zip(&selected) {
        let expected = SelectedOccurrence {
            occurrence: twin.id(),
            class: OccurrenceClass::RawToolSpans,
            revision: 1,
            representation: "tool_output".to_owned(),
            span: None,
            payload: payload.clone(),
            sensitivity: twin.sensitivity,
            provenance: Provenance {
                domain_id: "domain-stable-id".to_owned(),
                source_object_id: twin.source.to_owned(),
                source_evidence_id: "evidence-of-the-call".to_owned(),
                source_artifact_digest: DIGEST.to_owned(),
                created_commit_seq: 7,
            },
            tombstone: None,
            grouping: twin.grouping().unwrap(),
        };
        assert_eq!(*read, expected);
        let candidate = read.eligibility_candidate();
        assert_eq!(candidate.occurrence_id, twin.id().to_string());
        assert_eq!(candidate.candidate.object_id, twin.source);
        assert_eq!(candidate.candidate.source_revision, 1);
        assert_eq!(candidate.candidate.artifact_digest.as_deref(), Some(DIGEST));
    }
    assert_ne!(selected[0].grouping, selected[1].grouping);

    let as_occurrence = OccurrenceId::parse(&payload.payload_id).unwrap();
    let by_payload_id = store
        .with_conn(|conn| Ok(read_selected(conn, &[as_occurrence], MAX)))
        .unwrap();
    assert_eq!(
        by_payload_id,
        Err(ProjectionError::UnknownOccurrence {
            occurrence_id: payload.payload_id.clone(),
        })
    );

    let by_payload: BTreeMap<&str, &SelectedOccurrence> = selected
        .iter()
        .map(|read| (read.payload.payload_id.as_str(), read))
        .collect();
    assert_eq!(by_payload.len(), 1, "payload-keyed collapse loses a twin");
    let survivor = by_payload[selected[0].payload.payload_id.as_str()];
    let lost = selected
        .iter()
        .find(|read| read.occurrence != survivor.occurrence)
        .unwrap();
    assert_ne!(
        survivor.sensitivity, lost.sensitivity,
        "the survivor's sensitivity would be attributed to the lost twin"
    );

    let candidates: Vec<_> = selected
        .iter()
        .map(SelectedOccurrence::eligibility_candidate)
        .collect();
    let report = judge_occurrences(
        &kernel,
        &ProjectScope::new(PROJECT).unwrap(),
        ArtifactDestination::Local,
        &candidates,
    )
    .unwrap();
    let judged: Vec<&str> = report
        .occurrences
        .iter()
        .map(|judged| judged.occurrence_id.as_str())
        .collect();
    let expected: Vec<String> = ids.iter().map(ToString::to_string).collect();
    assert_eq!(judged, expected);
}

#[test]
fn reads_refuse_unknown_identities_oversized_selections_and_disagreeing_columns() {
    let whole = BUFFER.len() as u64;
    let span = tool_span(
        "call-1",
        "1",
        "tool_output",
        range(0, whole),
        Sensitivity::Normal,
        "src-1",
    );
    let missing = tool_span(
        "call-9",
        "1",
        "tool_output",
        None,
        Sensitivity::Normal,
        "src-9",
    );
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    store
        .with_conn_fenced(|conn| {
            let ids = [span.id(), missing.id()];
            persist(conn, std::slice::from_ref(&span));
            assert_eq!(
                read_selected(conn, &ids, MAX),
                Err(ProjectionError::UnknownOccurrence {
                    occurrence_id: missing.id().to_string(),
                })
            );
            assert_eq!(
                read_selected(conn, &ids, NonZeroUsize::MIN),
                Err(ProjectionError::TooManyRecords { count: 2 })
            );
            let one = read_selected(conn, &ids[..1], NonZeroUsize::MIN).unwrap();
            assert_eq!(one.len(), 1);
            assert_eq!(
                one[0].span, None,
                "a whole-buffer range reads back as the whole buffer"
            );

            Ok(())
        })
        .unwrap();
    drop(store);

    let foreign_identity = missing.identity();
    let foreign_tuple = encode(&missing.occurrence(&foreign_identity), BUFFER)
        .unwrap()
        .tuple;
    let foreign_tuple = format!(
        "UPDATE occurrences SET tuple=X'{}'",
        foreign_tuple
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    for update in [
        "UPDATE occurrences SET representation='tool_error'",
        "UPDATE occurrences SET revision=2",
        "UPDATE occurrences SET span_start=0,span_end=5",
        "UPDATE occurrences SET class='messages',representation='text'",
        foreign_tuple.as_str(),
    ] {
        let raw = || rusqlite::Connection::open(dir.path().join("search.sqlite")).unwrap();
        raw().execute(update, []).unwrap();
        let store = open(dir.path());
        let ids = [span.id()];
        let read = store
            .with_conn(|conn| Ok(read_selected(conn, &ids, MAX)))
            .unwrap();
        assert_eq!(read, Err(ProjectionError::CorruptRow), "{update}");
        drop(store);
        let identity = span.identity();
        let tuple = encode(&span.occurrence(&identity), BUFFER).unwrap().tuple;
        raw()
            .execute(
                "UPDATE occurrences SET class='raw_tool_spans',representation='tool_output',\
                 revision=1,span_start=NULL,span_end=NULL,tuple=?1",
                [tuple],
            )
            .unwrap();
    }
}

/// One well-formed identity per class; the field values that carry a format
/// rule get a value that passes it.
fn whole_object_identity(class: OccurrenceClass) -> Vec<(&'static str, &'static str)> {
    class
        .identity_fields()
        .iter()
        .map(|field| match *field {
            "harness" => (*field, "opencode"),
            "object_format" => (*field, "sha1"),
            "oid" => (*field, "0123456789abcdef0123456789abcdef01234567"),
            _ => (*field, "value"),
        })
        .collect()
}

#[test]
fn non_grouping_rows_whose_columns_disagree_with_the_tuple_are_refused() {
    let non_grouping: Vec<OccurrenceClass> = OccurrenceClass::ALL
        .into_iter()
        .filter(|class| !Grouping::applies_to(*class))
        .collect();
    for class in &non_grouping {
        let class = *class;
        let identity = whole_object_identity(class);
        let representation = class.representations()[0];
        let occurrence = Occurrence {
            class: class.code(),
            identity: &identity,
            revision: "1",
            representation,
            span: None,
        };
        let id = OccurrenceId::parse(&encode(&occurrence, BUFFER).unwrap().occurrence_id).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path());
        store
            .with_conn_fenced(|conn| {
                let record = OccurrenceRecord {
                    occurrence: occurrence.clone(),
                    payload: Payload::Whole(BUFFER),
                    domain_id: "domain-stable-id",
                    sensitivity: Sensitivity::Normal,
                    source_object_id: "src-1",
                    source_evidence_id: "evidence-of-the-object",
                    source_artifact_digest: DIGEST,
                    created_commit_seq: 7,
                };
                persist_occurrences(conn, &[record], bounds(), 1).unwrap();
                let read = read_selected(conn, &[id], MAX).unwrap();
                assert_eq!(read[0].grouping, Grouping::NonGrouping(class), "{class:?}");
                Ok(())
            })
            .unwrap();
        drop(store);

        let relabel = non_grouping
            .iter()
            .find(|other| **other != class)
            .map(|other| {
                format!(
                    "UPDATE occurrences SET class='{}',representation='{}'",
                    other.code(),
                    other.representations()[0]
                )
            })
            .unwrap();
        let mut updates = vec![
            "UPDATE occurrences SET revision=2".to_owned(),
            "UPDATE occurrences SET span_start=0,span_end=5".to_owned(),
            relabel,
        ];
        if let Some(other) = class.representations().get(1) {
            updates.push(format!("UPDATE occurrences SET representation='{other}'"));
        }
        for update in updates {
            let raw = || rusqlite::Connection::open(dir.path().join("search.sqlite")).unwrap();
            raw().execute(&update, []).unwrap();
            let store = open(dir.path());
            let read = store
                .with_conn(|conn| Ok(read_selected(conn, &[id], MAX)))
                .unwrap();
            assert_eq!(
                read,
                Err(ProjectionError::CorruptRow),
                "{class:?}: {update}"
            );
            drop(store);
            raw()
                .execute(
                    "UPDATE occurrences SET class=?1,revision=1,representation=?2,\
                     span_start=NULL,span_end=NULL",
                    [class.code(), representation],
                )
                .unwrap();
        }
    }
}

#[test]
fn payload_fetch_is_length_guarded_in_sql_and_verified_by_digest_afterwards() {
    let span = tool_span(
        "call-1",
        "1",
        "tool_output",
        None,
        Sensitivity::Normal,
        "src-1",
    );
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let reference = store
        .with_conn_fenced(|conn| {
            persist(conn, std::slice::from_ref(&span));
            Ok(read_selected(conn, &[span.id()], MAX).unwrap()[0]
                .payload
                .clone())
        })
        .unwrap();
    assert_eq!(reference.byte_length, BUFFER.len() as u64);

    let fetched = store
        .with_conn(|conn| Ok(fetch_payload(conn, &reference)))
        .unwrap()
        .unwrap();
    assert_eq!(fetched, BUFFER.as_bytes());
    assert_eq!(reference.verify(&fetched), Ok(()));

    let shorter = PayloadRef {
        byte_length: reference.byte_length - 1,
        ..reference.clone()
    };
    let beyond_kernel = PayloadRef {
        byte_length: kernel::MAX_PAYLOAD_BYTES as u64 + 1,
        ..reference.clone()
    };
    let unknown = PayloadRef {
        payload_id: "f".repeat(64),
        ..reference.clone()
    };
    for (name, wrong) in [
        ("shorter", &shorter),
        ("beyond the kernel bound", &beyond_kernel),
        ("unknown", &unknown),
    ] {
        let fetched = store
            .with_conn(|conn| Ok(fetch_payload(conn, wrong)))
            .unwrap();
        assert_eq!(fetched, Err(ProjectionError::CorruptRow), "{name}");
    }

    let mut altered = BUFFER.as_bytes().to_vec();
    altered[0] ^= 1;
    assert_eq!(reference.verify(&altered), Err(ProjectionError::CorruptRow));
    assert_eq!(
        reference.verify(&BUFFER.as_bytes()[..BUFFER.len() - 1]),
        Err(ProjectionError::CorruptRow)
    );
    drop(store);
    rusqlite::Connection::open(dir.path().join("search.sqlite"))
        .unwrap()
        .execute(
            "UPDATE payloads SET bytes=?1 WHERE payload_id=?2",
            rusqlite::params![altered, reference.payload_id],
        )
        .unwrap();
    let store = open(dir.path());
    let fetched = store
        .with_conn(|conn| Ok(fetch_payload(conn, &reference)))
        .unwrap()
        .unwrap();
    assert_eq!(fetched, altered, "the fetch alone does not hash");
    assert_eq!(reference.verify(&fetched), Err(ProjectionError::CorruptRow));
}

#[test]
fn grouping_keys_need_parent_revision_and_representation_together() {
    let base = tool_span(
        "call-1",
        "1",
        "tool_output",
        range(0, 13),
        Sensitivity::Normal,
        "s",
    );
    let same_parent = [
        tool_span(
            "call-1",
            "1",
            "tool_output",
            range(14, 20),
            Sensitivity::Normal,
            "s",
        ),
        tool_span("call-1", "1", "tool_output", None, Sensitivity::Secret, "t"),
    ];
    let other_group = [
        tool_span(
            "call-1",
            "2",
            "tool_output",
            range(0, 13),
            Sensitivity::Normal,
            "s",
        ),
        tool_span(
            "call-1",
            "1",
            "tool_error",
            range(0, 13),
            Sensitivity::Normal,
            "s",
        ),
        tool_span(
            "call-2",
            "1",
            "tool_output",
            range(0, 13),
            Sensitivity::Normal,
            "s",
        ),
    ];
    let Grouping::Grouped(key) = base.grouping().unwrap() else {
        panic!("raw tool spans group");
    };
    assert_eq!(key.class(), OccurrenceClass::RawToolSpans);
    assert_eq!(key.parent().revision(), 1);
    assert_eq!(key.representation(), "tool_output");
    for span in &same_parent {
        assert_eq!(span.grouping().unwrap(), Grouping::Grouped(key.clone()));
    }
    for span in &other_group {
        let Grouping::Grouped(other) = span.grouping().unwrap() else {
            panic!("raw tool spans group");
        };
        assert_ne!(other, key);
        let parent_only = other.parent().parent() == key.parent().parent();
        assert_eq!(
            parent_only,
            span.call == base.call && span.representation == base.representation
        );
    }
}

#[test]
fn classes_outside_the_grouping_set_yield_the_typed_non_grouping_result() {
    for class in OccurrenceClass::ALL {
        let identity = whole_object_identity(class);
        let representation = class.representations()[0];
        let encoded = encode(
            &Occurrence {
                class: class.code(),
                identity: &identity,
                revision: "1",
                representation,
                span: None,
            },
            BUFFER,
        )
        .unwrap();
        let grouping = Grouping::derive(
            &encoded.tuple,
            class,
            encoded.revision,
            representation,
            encoded.span,
        )
        .unwrap();
        let grouped = class == OccurrenceClass::RawToolSpans;
        assert_eq!(Grouping::applies_to(class), grouped, "{class:?}");
        if grouped {
            assert!(matches!(grouping, Grouping::Grouped(_)), "{class:?}");
        } else {
            assert_eq!(grouping, Grouping::NonGrouping(class), "{class:?}");
        }
        assert_eq!(
            Grouping::derive(&encoded.tuple, class, 2, representation, encoded.span),
            Err(IdentityRefusal::TupleMismatch),
            "{class:?}: a revision the tuple does not carry is refused whether or not the class groups"
        );
    }
}

#[test]
fn grouping_refuses_columns_that_disagree_with_the_tuple() {
    let span = tool_span(
        "call-1",
        "1",
        "tool_output",
        range(0, 13),
        Sensitivity::Normal,
        "s",
    );
    let identity = span.identity();
    let encoded = encode(&span.occurrence(&identity), BUFFER).unwrap();
    let derive = |revision: i64, representation: &str, range: Option<Span>| {
        Grouping::derive(
            &encoded.tuple,
            encoded.class,
            revision,
            representation,
            range,
        )
    };
    assert!(derive(1, "tool_output", encoded.span).is_ok());
    assert_eq!(
        derive(2, "tool_output", encoded.span),
        Err(IdentityRefusal::TupleMismatch)
    );
    assert_eq!(
        derive(1, "tool_error", encoded.span),
        Err(IdentityRefusal::TupleMismatch)
    );
    assert_eq!(
        derive(1, "tool_output", range(0, 14)),
        Err(IdentityRefusal::TupleMismatch)
    );
    let key = derive(1, "tool_output", encoded.span).unwrap();
    for at in 0..encoded.tuple.len() {
        for bit in 0..8 {
            let mut damaged = encoded.tuple.clone();
            damaged[at] ^= 1 << bit;
            let derived = Grouping::derive(&damaged, encoded.class, 1, "tool_output", encoded.span);
            assert_ne!(
                derived,
                Ok(key.clone()),
                "flip of bit {bit} at {at} kept the key"
            );
        }
    }
}
