//! The evaluator's versioned copy of the occurrence identity rule against the
//! kernel encoder and the independent identity goldens.

use std::path::Path;

use eval_core::{EncodingRefusal, IDENTITY_CONTRACT_VERSION, OCCURRENCE_ENCODING_VERSION};
use kernel::source_identity::{
    HARNESSES, OccurrenceClass, OccurrenceRefusal, Span, encode_preserving_span,
};
use serde_json::Value;

fn fixture() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/search-projection/source-identity-fixtures.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn identity_of(record: &Value) -> Vec<(String, String)> {
    record["identity"]
        .as_object()
        .map(|identity| {
            identity
                .iter()
                .filter_map(|(name, value)| Some((name.clone(), value.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

/// The golden's `span` is over the payload; a whole-buffer span carries no identity.
fn span_of(record: &Value) -> Option<(u64, u64)> {
    let bounds = record.get("span")?.as_array()?;
    let (start, end) = (bounds[0].as_u64()?, bounds[1].as_u64()?);
    let whole = record["payload"].as_str().map(|p| p.len() as u64) == Some(end) && start == 0;
    (!whole).then_some((start, end))
}

#[test]
fn the_core_encoder_reproduces_every_identity_golden_and_the_kernel_encoder() {
    let fixture = fixture();
    assert_eq!(
        fixture["identity_contract_version"].as_str().unwrap(),
        IDENTITY_CONTRACT_VERSION
    );
    assert_eq!(
        OCCURRENCE_ENCODING_VERSION,
        kernel::source_identity::OCCURRENCE_ENCODING_VERSION
    );
    for class in OccurrenceClass::ALL {
        let core = eval_core::OccurrenceClass::from_code(class.code()).unwrap();
        assert_eq!(core.identity_fields(), class.identity_fields());
        assert_eq!(core.representations(), class.representations());
    }
    let records = fixture["records"].as_array().unwrap();
    assert_eq!(records.len(), 23);
    for record in records {
        let id = record["id"].as_str().unwrap();
        let identity = identity_of(record);
        let identity: Vec<(&str, &str)> = identity
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        let span = span_of(record);
        let core = eval_core::encode(&eval_core::Occurrence {
            class: record["class"].as_str().unwrap(),
            identity: &identity,
            revision: record["revision"].as_str().unwrap(),
            representation: record["representation"].as_str().unwrap(),
            span: span.map(|(start, end)| eval_core::Span { start, end }),
        })
        .unwrap_or_else(|refusal| panic!("{id} refused: {refusal}"));
        let kernel = encode_preserving_span(&kernel::source_identity::Occurrence {
            class: record["class"].as_str().unwrap(),
            identity: &identity,
            revision: record["revision"].as_str().unwrap(),
            representation: record["representation"].as_str().unwrap(),
            span: span.map(|(start, end)| Span { start, end }),
        })
        .unwrap();
        assert_eq!(core.occurrence_id, record["expected_occurrence_id"], "{id}");
        assert_eq!(core.lineage_id, record["expected_lineage_id"], "{id}");
        assert_eq!(core.occurrence_id, kernel.occurrence_id, "{id}");
        assert_eq!(core.lineage_id, kernel.lineage_id, "{id}");
    }

    // Refusals the tuple rule alone can name agree with the kernel by name and order.
    let mut names = Vec::new();
    for record in fixture["invalid_records"].as_array().unwrap() {
        if !record["payload"].is_string() || record.get("span").is_some() {
            continue;
        }
        let identity = identity_of(record);
        let identity: Vec<(&str, &str)> = identity
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        let occurrence = eval_core::Occurrence {
            class: record["class"].as_str().unwrap_or(""),
            identity: &identity,
            revision: record["revision"].as_str().unwrap_or(""),
            representation: record["representation"].as_str().unwrap_or(""),
            span: None,
        };
        let refusal = eval_core::encode(&occurrence).unwrap_err();
        let name = serde_json::to_value(refusal).unwrap();
        assert_eq!(name, record["expected_refusal"], "{}", record["id"]);
        let kernel = encode_preserving_span(&kernel::source_identity::Occurrence {
            class: occurrence.class,
            identity: &identity,
            revision: occurrence.revision,
            representation: occurrence.representation,
            span: None,
        })
        .unwrap_err();
        assert_eq!(name, kernel.name(), "{}", record["id"]);
        names.push(refusal);
    }
    let kernel_order: Vec<&str> = OccurrenceRefusal::ALL.iter().map(|r| r.name()).collect();
    let core_order: Vec<String> = [
        EncodingRefusal::UnknownClass,
        EncodingRefusal::MissingIdentityField,
        EncodingRefusal::UnknownIdentityField,
        EncodingRefusal::MalformedIdentityValue,
        EncodingRefusal::UnknownHarness,
        EncodingRefusal::MalformedOid,
        EncodingRefusal::MissingRevision,
        EncodingRefusal::MalformedRevision,
        EncodingRefusal::UnknownRepresentation,
    ]
    .iter()
    .map(|r| {
        serde_json::to_value(r)
            .unwrap()
            .as_str()
            .unwrap()
            .to_string()
    })
    .collect();
    assert_eq!(
        core_order,
        kernel_order
            .iter()
            .filter(|name| !name.contains("span"))
            .map(|name| name.to_string())
            .collect::<Vec<_>>()
    );
    assert!(names.len() >= 8, "{names:?}");
    assert_eq!(HARNESSES, ["opencode", "pi"]);
}

/// Valid time is the revision of a message, so a later completion is a new
/// occurrence of the same lineage; a commit's time is not in its identity.
#[test]
fn a_later_message_time_changes_the_occurrence_but_not_the_lineage() {
    let message = |completed: &str| {
        let identity = [
            ("project_id", "proj-a"),
            ("harness", "opencode"),
            ("session_id", "sess-01"),
            ("message_id", "msg-001"),
            ("block_index", "0"),
        ];
        eval_core::encode(&eval_core::Occurrence {
            class: "messages",
            identity: &identity,
            revision: completed,
            representation: "text",
            span: None,
        })
        .unwrap()
    };
    let (first, later) = (message("1700000000000"), message("1700000000001"));
    assert_ne!(first.occurrence_id, later.occurrence_id);
    assert_eq!(first.lineage_id, later.lineage_id);

    let commit = |oid: &str| {
        let identity = [
            ("repository_id", "repo-0"),
            ("object_format", "sha1"),
            ("oid", oid),
        ];
        eval_core::encode(&eval_core::Occurrence {
            class: "git_commits",
            identity: &identity,
            revision: "1",
            representation: "commit_message",
            span: None,
        })
        .unwrap()
    };
    let same = commit(&"a".repeat(40));
    assert_eq!(same, commit(&"a".repeat(40)), "time is not an input");
    assert_ne!(same.occurrence_id, commit(&"b".repeat(40)).occurrence_id);
}
