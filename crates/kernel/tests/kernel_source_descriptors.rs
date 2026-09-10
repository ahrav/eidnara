//! The kernel's occurrence encoder is compared with the golden identifiers the
//! construction fixtures carry, and descriptors publish through a real store
//! with an independent ledger of identities, revisions, and bytes.

#![cfg(feature = "test-support")]

use std::collections::BTreeMap;
use std::path::Path;

use kernel::source_identity::{
    HARNESSES, Occurrence, OccurrenceClass, OccurrenceRefusal, Span, encode, payload_id, select,
    validate_span,
};
use kernel::{
    ArtifactIngestRequest, CommitIntent, DomainSpec, KernelError, KernelStore, ProviderEgress,
    RemediationTarget, RepositoryProvenance, SOURCE_DESCRIPTOR_KIND, Sensitivity,
    SourceDescriptorDetail, SourceDescriptorError, SourceDescriptorRequest, descriptor_object_id,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

const DOMAIN: &str = "domain";
const SCOPE: &str = "project:a";
const CONSUMER: &str = "probe";

fn fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/search-projection")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

fn record_span(record: &Value) -> Option<Span> {
    let span = record.get("span")?.as_array()?;
    Some(Span {
        start: span.first()?.as_u64()?,
        end: span.get(1)?.as_u64()?,
    })
}

/// Encodes a fixture record the way a producer would, reporting the fixture's
/// refusal name for the shapes the fixture format can express and the kernel
/// encoder cannot see (a payload that is not a string, a span that is not two
/// unsigned integers).
/// `(occurrence_id, lineage_id, payload_id)` for a fixture record.
fn encode_record(record: &Value) -> Result<(String, String, String), &'static str> {
    // Identity values that are not strings are dropped, so the encoder
    // reports them as missing fields; a class the encoder does not know is
    // still refused first.
    let identity: Vec<(String, String)> = record["identity"]
        .as_object()
        .map(|identity| {
            identity
                .iter()
                .filter_map(|(name, value)| Some((name.clone(), value.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default();
    let identity: Vec<(&str, &str)> = identity
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let occurrence = Occurrence {
        class: record["class"].as_str().unwrap_or(""),
        identity: &identity,
        revision: record["revision"].as_str().unwrap_or(""),
        representation: record["representation"].as_str().unwrap_or(""),
        span: None,
    };
    let payload = record["payload"].as_str().ok_or("payload_not_string")?;
    let span = match record.get("span") {
        None | Some(Value::Null) => None,
        Some(span) => {
            let bounds = span.as_array().ok_or("malformed_span")?;
            if bounds.len() != 2 || !bounds.iter().all(Value::is_u64) {
                return Err("malformed_span");
            }
            record_span(record)
        }
    };
    let encoded =
        encode(&Occurrence { span, ..occurrence }, payload).map_err(OccurrenceRefusal::name)?;
    Ok((
        encoded.occurrence_id,
        encoded.lineage_id,
        payload_id(select(encoded.span, payload)),
    ))
}

#[test]
fn the_kernel_encoder_matches_golden_identifiers_and_string_payload_refusals() {
    let fixtures = fixture("source-identity-fixtures.json");
    let contracts = fixture("construction-contracts.json");
    assert_eq!(
        contracts["tuple_encoding"]["version_byte"]
            .as_u64()
            .unwrap(),
        u64::from(kernel::source_identity::OCCURRENCE_ENCODING_VERSION)
    );
    for class in OccurrenceClass::ALL {
        let spec = &contracts["classes"][class.code()];
        let fields: Vec<&str> = spec["identity_fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|field| field.as_str().unwrap())
            .collect();
        assert_eq!(
            class.identity_fields(),
            fields.as_slice(),
            "{}",
            class.code()
        );
        let representations: Vec<&str> = spec["representations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|field| field.as_str().unwrap())
            .collect();
        assert_eq!(class.representations(), representations.as_slice());
    }
    let matrix = fixture("witness-matrix.json");
    let harnesses: Vec<&str> = matrix["harnesses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|harness| harness.as_str().unwrap())
        .collect();
    assert_eq!(HARNESSES.as_slice(), harnesses.as_slice());

    let mut encoded: BTreeMap<String, (String, String, String)> = BTreeMap::new();
    for record in fixtures["records"].as_array().unwrap() {
        let id = record["id"].as_str().unwrap();
        let (occurrence_id, lineage_id, payload_id) =
            encode_record(record).unwrap_or_else(|refusal| panic!("{id} refused: {refusal}"));
        assert_eq!(occurrence_id, record["expected_occurrence_id"], "{id}");
        assert_eq!(lineage_id, record["expected_lineage_id"], "{id}");
        assert_eq!(payload_id, record["expected_payload_id"], "{id}");
        encoded.insert(id.to_owned(), (occurrence_id, lineage_id, payload_id));
    }
    let expectations = &fixtures["expectations"];
    for pair in expectations["distinct_occurrences"].as_array().unwrap() {
        let (a, b) = (pair[0].as_str().unwrap(), pair[1].as_str().unwrap());
        assert_ne!(encoded[a].0, encoded[b].0, "{a}/{b}");
    }
    for pair in expectations["equal_occurrences"].as_array().unwrap() {
        let (a, b) = (pair[0].as_str().unwrap(), pair[1].as_str().unwrap());
        assert_eq!(encoded[a], encoded[b], "{a}/{b} are one occurrence");
    }
    for pair in expectations["equal_lineages"].as_array().unwrap() {
        let (a, b) = (pair[0].as_str().unwrap(), pair[1].as_str().unwrap());
        assert_eq!(encoded[a].1, encoded[b].1, "{a}/{b} lineage");
    }
    for pair in expectations["distinct_lineages"].as_array().unwrap() {
        let (a, b) = (pair[0].as_str().unwrap(), pair[1].as_str().unwrap());
        assert_ne!(encoded[a].1, encoded[b].1, "{a}/{b} lineage");
    }
    let occurrence_ids: std::collections::BTreeSet<&str> =
        encoded.values().map(|ids| ids.0.as_str()).collect();
    for ids in encoded.values() {
        assert!(
            !occurrence_ids.contains(ids.1.as_str()),
            "a lineage id equals an occurrence id"
        );
    }
    for pair in expectations["equal_payloads"].as_array().unwrap() {
        let (a, b) = (pair[0].as_str().unwrap(), pair[1].as_str().unwrap());
        assert_eq!(encoded[a].2, encoded[b].2, "{a}/{b}");
    }
    let precedence: Vec<&str> = fixtures["refusal_precedence"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| name.as_str().unwrap())
        .collect();
    let kernel_names: Vec<&str> = OccurrenceRefusal::ALL.iter().map(|r| r.name()).collect();
    // The fixture format has one refusal the kernel encoder never sees: a
    // payload that is not a string is a shape a typed request cannot express.
    let expected: Vec<&str> = precedence
        .iter()
        .copied()
        .filter(|name| *name != "payload_not_string")
        .collect();
    assert_eq!(kernel_names, expected);
    for record in fixtures["invalid_records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|record| record["payload"].is_string())
    {
        let id = record["id"].as_str().unwrap();
        let expected = record["expected_refusal"].as_str().unwrap();
        match encode_record(record) {
            Ok(_) => panic!("{id} was accepted; expected {expected}"),
            Err(refusal) => assert_eq!(refusal, expected, "{id}"),
        }
    }
}

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "kernel-source-descriptor-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

struct Fixture {
    root: tempfile::TempDir,
    store: KernelStore,
}

#[derive(Debug)]
enum Published {
    Committed {
        outcome: kernel::SourceDescriptorOutcome,
        seq: i64,
    },
    Replayed {
        object_id: String,
        seq: i64,
    },
    Refused(SourceDescriptorError),
}

impl Fixture {
    fn open() -> Self {
        let root = tempfile::tempdir().unwrap();
        let store = KernelStore::open(root.path()).unwrap();
        store
            .commit(intent("seed"), |envelope| {
                envelope.insert_domain(DomainSpec {
                    domain_id: DOMAIN.to_string(),
                    object_id: "domain-object".to_string(),
                    name: "fixture".to_string(),
                    source_kind: "fixture".to_string(),
                    source_id: DOMAIN.to_string(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                })?;
                envelope.insert_scope(kernel::ScopeSpec {
                    scope_id: SCOPE.to_string(),
                    object_id: SCOPE.to_string(),
                    source_id: SCOPE.to_string(),
                    domain_id: DOMAIN.to_string(),
                    source_kind: "kernel_route".to_string(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                    terms: vec![kernel::ScopeTermSpec {
                        dimension: kernel::Dimension::Project.as_str().to_string(),
                        operator: "exact".to_string(),
                        exact_value: Some("a".repeat(64)),
                        ..kernel::ScopeTermSpec::default()
                    }],
                })?;
                envelope.register_outbox_consumer(CONSUMER, 1)?;
                Ok(String::new())
            })
            .unwrap();
        Self { root, store }
    }

    /// The change kinds and replaced objects the commit `seq` published to the
    /// outbox, in ordinal order, with every payload byte checked against
    /// `forbidden` text.
    fn outbox_changes(&self, seq: i64, forbidden: &[&str]) -> Vec<(String, Option<String>)> {
        let page = self
            .store
            .read_complete_commits(
                &kernel::CommitReadRequest {
                    consumer_id: CONSUMER.to_string(),
                    incarnation: self.store.capture_commit_read_target().unwrap().incarnation,
                    after_commit: seq - 1,
                    through_commit: seq,
                },
                kernel::CommitPageBounds {
                    max_commits: 1.try_into().unwrap(),
                    max_rows: 64.try_into().unwrap(),
                    max_payload_bytes: (1 << 20).try_into().unwrap(),
                },
            )
            .unwrap();
        assert_eq!(page.commits.len(), 1);
        page.commits[0]
            .rows
            .iter()
            .map(|row| {
                let text = String::from_utf8(row.payload.clone()).unwrap();
                for needle in forbidden {
                    assert!(!text.contains(needle), "outbox payload carries {needle:?}");
                }
                let payload: Value = serde_json::from_str(&text).unwrap();
                (
                    payload["change_kind"].as_str().unwrap().to_owned(),
                    payload["replaced_object_id"].as_str().map(str::to_owned),
                )
            })
            .collect()
    }

    /// Retains `text` exactly and returns `(evidence_id, digest)`.
    fn retain(&self, key: &str, text: &str) -> (String, String) {
        let handle = self
            .store
            .ingest_exact_artifact(ArtifactIngestRequest {
                intent: intent(&format!("artifact-{key}")),
                payload: text.as_bytes().to_vec(),
                evidence_id: format!("evidence-{key}"),
                object_id: format!("evidence-object-{key}"),
                object_kind: "evidence".to_string(),
                domain_id: DOMAIN.to_string(),
                source_kind: "tool_output".to_string(),
                source_id: format!("native/{key}"),
                source_revision: 1,
                media_type: "text/plain".to_string(),
                retention_class: "canonical".to_string(),
                retain_until: None,
                asserted_sensitivity: Sensitivity::Normal,
                provider_egress: ProviderEgress::RemoteAllowed,
                provenance: Some(RepositoryProvenance {
                    repository_id: "repo".to_string(),
                    revision: "abc123".to_string(),
                }),
            })
            .unwrap();
        (handle.evidence_id, handle.digest)
    }

    fn publish(
        &self,
        key: &str,
        request: &SourceDescriptorRequest<'_>,
    ) -> Result<(kernel::SourceDescriptorOutcome, i64, bool), SourceDescriptorError> {
        match self.publish_outcome(key, request) {
            Published::Committed { outcome, seq } => Ok((outcome, seq, false)),
            Published::Replayed { object_id, seq } => Ok((
                kernel::SourceDescriptorOutcome {
                    object_id,
                    occurrence_id: String::new(),
                    lineage_id: String::new(),
                    payload_id: String::new(),
                    replaced_object_id: None,
                },
                seq,
                true,
            )),
            Published::Refused(error) => Err(error),
        }
    }

    /// Runs one publication commit and reports which of the three things
    /// happened. A replayed intent never runs the operation, so the only
    /// evidence of the earlier publication is the receipt.
    fn publish_outcome(&self, key: &str, request: &SourceDescriptorRequest<'_>) -> Published {
        let mut outcome = None;
        let receipt = self.store.commit(intent(key), |envelope| {
            let published = envelope.publish_source_descriptor(request);
            let result = match &published {
                Ok(published) => Ok(published.object_id.clone()),
                Err(SourceDescriptorError::Kernel(error)) => Err(*error),
                Err(_) => Err(KernelError::InvalidInput),
            };
            outcome = Some(published);
            result
        });
        match (outcome, receipt) {
            (Some(Ok(outcome)), Ok(receipt)) => {
                assert!(!receipt.replayed);
                Published::Committed {
                    outcome,
                    seq: receipt.commit_seq,
                }
            }
            (None, Ok(receipt)) => {
                assert!(receipt.replayed, "the operation ran but left no outcome");
                Published::Replayed {
                    object_id: receipt.result,
                    seq: receipt.commit_seq,
                }
            }
            (Some(Err(error)), Err(_)) => Published::Refused(error),
            (outcome, receipt) => panic!("inconsistent publication: {outcome:?} {receipt:?}"),
        }
    }

    fn detail(&self, object_id: &str) -> Option<SourceDescriptorDetail> {
        let snapshot = self.store.slice_as_of(self.store.tip().unwrap()).unwrap();
        snapshot
            .observations
            .iter()
            .find(|row| row.object_id == object_id)
            .map(|row| {
                assert_eq!(row.observation_kind, SOURCE_DESCRIPTOR_KIND);
                serde_json::from_str(row.payload.detail.as_deref().unwrap()).unwrap()
            })
    }

    fn live(&self, object_id: &str) -> Option<bool> {
        let (_, states) = self.store.object_states(&[object_id.to_string()]).unwrap();
        states[0]
            .as_ref()
            .map(|state| state.object.invalidated_commit_seq.is_none())
    }

    fn publish_batch(
        &self,
        key: &str,
        requests: &[SourceDescriptorRequest<'_>],
    ) -> Result<Vec<kernel::SourceDescriptorOutcome>, SourceDescriptorError> {
        let mut outcome = None;
        let receipt = self.store.commit(intent(key), |envelope| {
            let published = envelope.publish_source_descriptors(requests);
            let result = match &published {
                Ok(_) => Ok(String::new()),
                Err(SourceDescriptorError::Kernel(error)) => Err(*error),
                Err(_) => Err(KernelError::InvalidInput),
            };
            outcome = Some(published);
            result
        });
        let outcome = outcome.expect("the operation ran");
        assert_eq!(outcome.is_ok(), receipt.is_ok());
        outcome
    }

    fn descriptor_object_ids_at_tip(&self) -> Vec<String> {
        let snapshot = self.store.slice_as_of(self.store.tip().unwrap()).unwrap();
        snapshot
            .observations
            .iter()
            .filter(|row| row.observation_kind == SOURCE_DESCRIPTOR_KIND)
            .map(|row| row.object_id.clone())
            .collect()
    }
}

fn message<'a>(identity: &'a [(&'a str, &'a str)], revision: &'a str) -> Occurrence<'a> {
    Occurrence {
        class: "messages",
        identity,
        revision,
        representation: "text",
        span: None,
    }
}

const MSG_A: &[(&str, &str)] = &[
    ("project_id", "proj-a"),
    ("harness", "opencode"),
    ("session_id", "sess-01"),
    ("message_id", "msg-001"),
    ("block_index", "0"),
];
const MSG_B: &[(&str, &str)] = &[
    ("project_id", "proj-a"),
    ("harness", "opencode"),
    ("session_id", "sess-01"),
    ("message_id", "msg-002"),
    ("block_index", "0"),
];
const TOOL: &[(&str, &str)] = &[
    ("project_id", "proj-a"),
    ("harness", "pi"),
    ("session_id", "sess-01"),
    ("parent_message_id", "msg-002"),
    ("tool_call_id", "call-9"),
    ("result_revision", "1"),
    ("block_index", "0"),
];

fn request<'a>(
    occurrence: Occurrence<'a>,
    evidence: &'a (String, String),
    buffer: &'a str,
) -> SourceDescriptorRequest<'a> {
    SourceDescriptorRequest {
        occurrence,
        domain_id: DOMAIN,
        scope_id: Some(SCOPE),
        evidence_id: &evidence.0,
        artifact_digest: &evidence.1,
        buffer,
        sensitivity: Sensitivity::Normal,
        observed_at: 1,
    }
}

#[test]
fn descriptors_match_an_independent_ledger_and_identical_text_stays_distinct() {
    let fixture = Fixture::open();
    let text = "Deploy failed";
    let evidence = fixture.retain("shared", text);
    let tool_text = "error: line 1\r\nwarning: naïve 日本語 🎉\r\n";
    let tool_evidence = fixture.retain("tool", tool_text);
    let span = Some(Span { start: 0, end: 13 });

    // The ledger: what the test expects each publication to produce, computed
    // from the fixture's own inputs.
    let expected_a = encode(&message(MSG_A, "1"), text).unwrap();
    let expected_b = encode(&message(MSG_B, "1"), text).unwrap();
    let expected_tool = encode(
        &Occurrence {
            class: "raw_tool_spans",
            identity: TOOL,
            revision: "1",
            representation: "tool_output",
            span,
        },
        tool_text,
    )
    .unwrap();
    assert_ne!(expected_a.occurrence_id, expected_b.occurrence_id);
    assert_eq!(payload_id(text.as_bytes()), evidence.1);

    let (a, seq_a, _) = fixture
        .publish("a", &request(message(MSG_A, "1"), &evidence, text))
        .unwrap();
    let (b, _, _) = fixture
        .publish("b", &request(message(MSG_B, "1"), &evidence, text))
        .unwrap();
    let tool_request = SourceDescriptorRequest {
        occurrence: Occurrence {
            class: "raw_tool_spans",
            identity: TOOL,
            revision: "1",
            representation: "tool_output",
            span,
        },
        ..request(message(MSG_A, "1"), &tool_evidence, tool_text)
    };
    let (tool, _, _) = fixture.publish("tool", &tool_request).unwrap();

    assert_eq!(a.occurrence_id, expected_a.occurrence_id);
    assert_eq!(b.occurrence_id, expected_b.occurrence_id);
    assert_eq!(tool.occurrence_id, expected_tool.occurrence_id);
    assert_eq!(a.payload_id, b.payload_id, "equal text shares a payload id");
    assert_eq!(a.payload_id, evidence.1);
    assert_eq!(tool.payload_id, payload_id(b"error: line 1"));
    assert_ne!(
        tool.payload_id, tool_evidence.1,
        "a span selects part of the artifact"
    );
    assert_eq!(
        a.object_id,
        descriptor_object_id(&expected_a.lineage_id, "1")
    );
    assert!(a.replaced_object_id.is_none());

    for (outcome, occurrence, evidence, span, buffer) in [
        (&a, message(MSG_A, "1"), &evidence, None, text),
        (&b, message(MSG_B, "1"), &evidence, None, text),
        (
            &tool,
            tool_request.occurrence.clone(),
            &tool_evidence,
            Some((0u64, 13u64)),
            tool_text,
        ),
    ] {
        let detail = fixture.detail(&outcome.object_id).unwrap();
        assert_eq!(detail.occurrence_id, outcome.occurrence_id);
        assert_eq!(
            serde_json::to_value(&detail).unwrap()["occurrence_tuple"],
            serde_json::json!(encode(&occurrence, buffer).unwrap().tuple),
            "the stored descriptor retains the exact occurrence encoding"
        );
        assert_eq!(detail.lineage_id, outcome.lineage_id);
        assert_eq!(detail.payload_id, outcome.payload_id);
        assert_eq!(detail.artifact_digest, evidence.1);
        assert_eq!(detail.evidence_id, evidence.0);
        assert_eq!(detail.class, occurrence.class);
        assert_eq!(detail.revision, occurrence.revision);
        assert_eq!(detail.representation, occurrence.representation);
        assert_eq!(detail.span, span);
        let identity: Vec<(String, String)> = occurrence
            .identity
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        assert_eq!(detail.identity, identity);
        assert_eq!(fixture.live(&outcome.object_id), Some(true));
    }
    // The commit that published `a` carries one observation insert whose
    // payload names identifiers, never the text.
    assert_eq!(
        fixture.outbox_changes(seq_a, &[text]),
        vec![("observation_insert".to_string(), None)]
    );
    let _ = fixture.root.path();
}

#[test]
fn representations_and_spans_of_one_source_are_separate_lineages() {
    let fixture = Fixture::open();
    let claim_text = "Use SQLite WAL for the projection.";
    let claim = fixture.retain("claim", claim_text);
    let tool_text = "error: line 1\r\nwarning: naïve 日本語 🎉\r\n";
    let tool = fixture.retain("tool", tool_text);
    let claim_identity: &[(&str, &str)] = &[("object_id", "obj-77")];
    let claim_occurrence = |representation: &'static str| Occurrence {
        class: "canonical_claims",
        identity: claim_identity,
        revision: "3",
        representation,
        span: None,
    };
    let tool_occurrence = |span: Option<Span>| Occurrence {
        class: "raw_tool_spans",
        identity: TOOL,
        revision: "1",
        representation: "tool_output",
        span,
    };
    let published: Vec<kernel::SourceDescriptorOutcome> = [
        (
            "summary",
            claim_occurrence("decision_summary"),
            &claim,
            claim_text,
        ),
        (
            "rationale",
            claim_occurrence("rationale"),
            &claim,
            claim_text,
        ),
        ("whole", tool_occurrence(None), &tool, tool_text),
        (
            "first",
            tool_occurrence(Some(Span { start: 0, end: 13 })),
            &tool,
            tool_text,
        ),
        (
            "second",
            tool_occurrence(Some(Span { start: 15, end: 40 })),
            &tool,
            tool_text,
        ),
    ]
    .into_iter()
    .map(|(key, occurrence, evidence, buffer)| {
        let (outcome, _, _) = fixture
            .publish(key, &request(occurrence, evidence, buffer))
            .unwrap();
        outcome
    })
    .collect();
    for outcome in &published {
        assert!(
            outcome.replaced_object_id.is_none(),
            "{}",
            outcome.object_id
        );
        assert_eq!(fixture.live(&outcome.object_id), Some(true));
    }
    let lineages: std::collections::BTreeSet<&str> =
        published.iter().map(|o| o.lineage_id.as_str()).collect();
    assert_eq!(lineages.len(), published.len(), "every lineage is distinct");
    assert_eq!(
        published[0].payload_id, published[1].payload_id,
        "same bytes"
    );
    assert_ne!(published[3].payload_id, published[4].payload_id);
    // A span covering the whole buffer is the whole-block selection: it lands
    // in the same lineage as "whole", so at the same revision it does not
    // advance, and at the next revision it supersedes "whole".
    let whole_span = Some(Span {
        start: 0,
        end: tool_text.len() as u64,
    });
    let error = fixture
        .publish(
            "whole-as-span",
            &request(tool_occurrence(whole_span), &tool, tool_text),
        )
        .unwrap_err();
    assert_eq!(error, SourceDescriptorError::RevisionNotAdvanced);
    let (advanced, _, _) = fixture
        .publish(
            "whole-as-span-2",
            &request(
                Occurrence {
                    revision: "2",
                    ..tool_occurrence(whole_span)
                },
                &tool,
                tool_text,
            ),
        )
        .unwrap();
    assert_eq!(advanced.lineage_id, published[2].lineage_id);
    assert_eq!(
        advanced.replaced_object_id.as_deref(),
        Some(published[2].object_id.as_str())
    );
    let detail = fixture.detail(&advanced.object_id).unwrap();
    assert_eq!(detail.span, None, "the stored span is the whole block");
    assert_eq!(
        fixture.live(&published[3].object_id),
        Some(true),
        "narrower spans untouched"
    );
    assert_eq!(fixture.live(&published[4].object_id), Some(true));
}

#[test]
fn concurrent_successors_serialize_into_one_chain() {
    let fixture = Fixture::open();
    let base = fixture.retain("base", "base text");
    let (first, _, _) = fixture
        .publish("base", &request(message(MSG_A, "1"), &base, "base text"))
        .unwrap();
    let store = std::sync::Arc::new(fixture.store);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = ["2", "3"]
        .into_iter()
        .map(|revision| {
            let store = std::sync::Arc::clone(&store);
            let barrier = std::sync::Arc::clone(&barrier);
            let text = format!("revision {revision}");
            std::thread::spawn(move || {
                let handle = store
                    .ingest_exact_artifact(ArtifactIngestRequest {
                        intent: intent(&format!("artifact-rev-{revision}")),
                        payload: text.as_bytes().to_vec(),
                        evidence_id: format!("evidence-rev-{revision}"),
                        object_id: format!("evidence-object-rev-{revision}"),
                        object_kind: "evidence".to_string(),
                        domain_id: DOMAIN.to_string(),
                        source_kind: "tool_output".to_string(),
                        source_id: format!("native/rev-{revision}"),
                        source_revision: 1,
                        media_type: "text/plain".to_string(),
                        retention_class: "canonical".to_string(),
                        retain_until: None,
                        asserted_sensitivity: Sensitivity::Normal,
                        provider_egress: ProviderEgress::RemoteAllowed,
                        provenance: Some(RepositoryProvenance {
                            repository_id: "repo".to_string(),
                            revision: "abc123".to_string(),
                        }),
                    })
                    .unwrap();
                let evidence = (handle.evidence_id, handle.digest);
                barrier.wait();
                let request = request(message(MSG_A, revision), &evidence, &text);
                let mut outcome = None;
                let receipt = store.commit(intent(&format!("rev-{revision}")), |envelope| {
                    let published = envelope.publish_source_descriptor(&request);
                    let result = published
                        .as_ref()
                        .map(|published| published.object_id.clone())
                        .map_err(|_| KernelError::InvalidInput);
                    outcome = Some(published);
                    result
                });
                (
                    revision,
                    outcome.unwrap(),
                    receipt.map(|receipt| receipt.commit_seq),
                )
            })
        })
        .collect();
    let results: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    // Both may succeed (2 then 3) or the lower may lose (3 then 2); either
    // way the chain has exactly one live head and the loser changed nothing.
    let successes: Vec<_> = results.iter().filter(|(_, o, _)| o.is_ok()).collect();
    let failures: Vec<_> = results.iter().filter(|(_, o, _)| o.is_err()).collect();
    assert!(!successes.is_empty());
    for (revision, outcome, _) in &failures {
        assert_eq!(*revision, "2", "only the lower revision can lose");
        assert_eq!(
            *outcome.as_ref().unwrap_err(),
            SourceDescriptorError::RevisionNotAdvanced
        );
    }
    let mut seqs: Vec<i64> = successes
        .iter()
        .map(|(_, _, seq)| *seq.as_ref().unwrap())
        .collect();
    seqs.sort_unstable();
    seqs.dedup();
    assert_eq!(seqs.len(), successes.len(), "distinct commits");
    let head = successes
        .iter()
        .max_by_key(|(revision, _, _)| revision.parse::<i64>().unwrap())
        .unwrap();
    let head_id = head.1.as_ref().unwrap().object_id.clone();
    let lineage = &head.1.as_ref().unwrap().lineage_id;
    let (_, states) = store
        .object_states(&[
            first.object_id.clone(),
            descriptor_object_id(lineage, "2"),
            descriptor_object_id(lineage, "3"),
        ])
        .unwrap();
    let live: Vec<&str> = states
        .iter()
        .flatten()
        .filter(|state| state.object.invalidated_commit_seq.is_none())
        .map(|state| state.object.object_id.as_str())
        .collect();
    assert_eq!(live, [head_id.as_str()], "exactly one live head");
    // Every superseded row names its successor, forming one chain to the head.
    let mut cursor = first.object_id.clone();
    let mut hops = 0;
    while cursor != head_id {
        let (_, state) = store.object_states(&[cursor.clone()]).unwrap();
        cursor = state[0]
            .as_ref()
            .unwrap()
            .object
            .superseded_by
            .clone()
            .expect("a superseded row names its successor");
        hops += 1;
        assert!(hops <= 2);
    }
}

#[test]
fn a_successor_invalidates_exactly_its_predecessor_in_one_commit_and_replays_from_its_receipt() {
    let fixture = Fixture::open();
    let v1 = fixture.retain("v1", "first text");
    let v2 = fixture.retain("v2", "second text");
    let (first, _, _) = fixture
        .publish("rev-1", &request(message(MSG_A, "1"), &v1, "first text"))
        .unwrap();
    let other_evidence = fixture.retain("other", "other lineage");
    let (other, _, _) = fixture
        .publish(
            "other",
            &request(message(MSG_B, "1"), &other_evidence, "other lineage"),
        )
        .unwrap();

    let (second, seq, replayed) = fixture
        .publish("rev-2", &request(message(MSG_A, "2"), &v2, "second text"))
        .unwrap();
    assert!(!replayed);
    assert_eq!(
        second.replaced_object_id.as_deref(),
        Some(first.object_id.as_str())
    );
    assert_eq!(second.lineage_id, first.lineage_id);
    assert_ne!(second.occurrence_id, first.occurrence_id);
    assert_eq!(fixture.live(&first.object_id), Some(false));
    assert_eq!(fixture.live(&second.object_id), Some(true));
    assert_eq!(
        fixture.live(&other.object_id),
        Some(true),
        "another lineage is untouched"
    );
    // Both transitions happened in the one commit.
    let (_, states) = fixture
        .store
        .object_states(&[first.object_id.clone(), second.object_id.clone()])
        .unwrap();
    assert_eq!(
        states[0].as_ref().unwrap().object.invalidated_commit_seq,
        Some(seq)
    );
    assert_eq!(
        states[0].as_ref().unwrap().object.superseded_by.as_deref(),
        Some(second.object_id.as_str())
    );
    assert_eq!(states[1].as_ref().unwrap().object.created_commit_seq, seq);
    assert_eq!(
        fixture.outbox_changes(seq, &["second text", "first text"]),
        vec![(
            "observation_correct".to_string(),
            Some(first.object_id.clone())
        )]
    );

    // A lost response replays the receipt: same object, no new revision.
    let (again, again_seq, replayed) = fixture
        .publish("rev-2", &request(message(MSG_A, "2"), &v2, "second text"))
        .unwrap();
    assert!(replayed);
    assert_eq!(again.object_id, second.object_id);
    assert_eq!(again_seq, seq);
    assert_eq!(fixture.store.tip().unwrap(), seq);

    // A revision that does not advance is refused and changes nothing.
    for stale in ["2", "1", "0"] {
        let error = fixture
            .publish(
                &format!("stale-{stale}"),
                &request(message(MSG_A, stale), &v2, "second text"),
            )
            .unwrap_err();
        assert_eq!(error, SourceDescriptorError::RevisionNotAdvanced, "{stale}");
    }
    assert_eq!(fixture.store.tip().unwrap(), seq);
    assert_eq!(fixture.live(&second.object_id), Some(true));

    // Another domain cannot take over the lineage.
    fixture
        .store
        .commit(intent("other-domain"), |envelope| {
            envelope.insert_domain(DomainSpec {
                domain_id: "other-domain".to_string(),
                object_id: "other-domain-object".to_string(),
                name: "other".to_string(),
                source_kind: "fixture".to_string(),
                source_id: "other-domain".to_string(),
                source_revision: 1,
                sensitivity: Sensitivity::Normal,
            })?;
            Ok(String::new())
        })
        .unwrap();
    let error = fixture
        .publish(
            "foreign-domain",
            &SourceDescriptorRequest {
                domain_id: "other-domain",
                ..request(message(MSG_A, "9"), &v2, "second text")
            },
        )
        .unwrap_err();
    assert_eq!(error, SourceDescriptorError::DomainMismatch);
    let seq = fixture.store.tip().unwrap();

    // A crash before the descriptor commit publishes nothing and keeps the predecessor.
    let v3 = fixture.retain("v3", "third text");
    let before_fault = fixture.store.tip().unwrap();
    let request_v3 = request(message(MSG_A, "3"), &v3, "third text");
    let error = fixture
        .store
        .commit_with_fault_after_events_for_test(intent("rev-3"), |envelope| {
            envelope
                .publish_source_descriptor(&request_v3)
                .map(|outcome| outcome.object_id)
                .map_err(|_| KernelError::InvalidInput)
        })
        .unwrap_err();
    assert_eq!(error, KernelError::Fault);
    assert_eq!(fixture.store.tip().unwrap(), before_fault);
    assert_eq!(fixture.live(&second.object_id), Some(true));
    assert_eq!(
        fixture.live(&descriptor_object_id(&second.lineage_id, "3")),
        None
    );

    // After reopening, the canonical view still shows the atomic transition.
    let Fixture { root, store } = fixture;
    drop(store);
    let reopened = KernelStore::open(root.path()).unwrap();
    let snapshot = reopened.slice_as_of(seq).unwrap();
    let live: Vec<&str> = snapshot
        .observations
        .iter()
        .filter(|row| row.observation_kind == SOURCE_DESCRIPTOR_KIND)
        .map(|row| row.object_id.as_str())
        .collect();
    assert!(live.contains(&second.object_id.as_str()));
    assert!(!live.contains(&first.object_id.as_str()));
    assert!(live.contains(&other.object_id.as_str()));
}

#[test]
fn malformed_requests_fail_closed_before_any_row_is_written() {
    let fixture = Fixture::open();
    let evidence = fixture.retain("text", "some text");
    let other = fixture.retain("other", "other text");
    let unicode = fixture.retain("unicode", "日本");

    let cases: Vec<(&str, SourceDescriptorRequest<'_>, SourceDescriptorError)> = vec![
        (
            "missing identity",
            request(message(&MSG_A[..4], "1"), &evidence, "some text"),
            SourceDescriptorError::Occurrence(OccurrenceRefusal::MissingIdentityField),
        ),
        (
            "unknown class",
            SourceDescriptorRequest {
                occurrence: Occurrence {
                    class: "user_memories",
                    ..message(MSG_A, "1")
                },
                ..request(message(MSG_A, "1"), &evidence, "some text")
            },
            SourceDescriptorError::Occurrence(OccurrenceRefusal::UnknownClass),
        ),
        (
            "wrong scope",
            SourceDescriptorRequest {
                scope_id: Some("project:missing"),
                ..request(message(MSG_A, "1"), &evidence, "some text")
            },
            SourceDescriptorError::Kernel(KernelError::NotFound),
        ),
        (
            "artifact mismatch",
            SourceDescriptorRequest {
                artifact_digest: &other.1,
                ..request(message(MSG_A, "1"), &evidence, "some text")
            },
            SourceDescriptorError::ArtifactMismatch,
        ),
        (
            "buffer mismatch",
            request(message(MSG_A, "1"), &evidence, "some other text"),
            SourceDescriptorError::BufferMismatch,
        ),
        (
            "missing evidence",
            SourceDescriptorRequest {
                evidence_id: "evidence-nowhere",
                ..request(message(MSG_A, "1"), &evidence, "some text")
            },
            SourceDescriptorError::EvidenceMissing,
        ),
        (
            "span past the end",
            SourceDescriptorRequest {
                occurrence: Occurrence {
                    span: Some(Span { start: 0, end: 99 }),
                    ..message(MSG_A, "1")
                },
                ..request(message(MSG_A, "1"), &evidence, "some text")
            },
            SourceDescriptorError::Occurrence(OccurrenceRefusal::SpanOutOfRange),
        ),
        (
            "invalid span precedes missing evidence",
            SourceDescriptorRequest {
                evidence_id: "missing-evidence",
                occurrence: Occurrence {
                    span: Some(Span { start: 0, end: 99 }),
                    ..message(MSG_A, "1")
                },
                ..request(message(MSG_A, "1"), &evidence, "some text")
            },
            SourceDescriptorError::Occurrence(OccurrenceRefusal::SpanOutOfRange),
        ),
        (
            "reversed span",
            SourceDescriptorRequest {
                occurrence: Occurrence {
                    span: Some(Span { start: 5, end: 2 }),
                    ..message(MSG_A, "1")
                },
                ..request(message(MSG_A, "1"), &evidence, "some text")
            },
            SourceDescriptorError::Occurrence(OccurrenceRefusal::SpanReversed),
        ),
        (
            "non-decimal revision",
            request(message(MSG_A, "v1"), &evidence, "some text"),
            SourceDescriptorError::Occurrence(OccurrenceRefusal::MalformedRevision),
        ),
    ];
    for (name, case, expected) in cases {
        let error = fixture.publish(name, &case).unwrap_err();
        assert_eq!(error, expected, "{name}");
    }
    // A redacting-lane artifact is not exact evidence.
    let redacting = fixture
        .store
        .ingest_artifact(ArtifactIngestRequest {
            intent: intent("artifact-redacting"),
            payload: b"prefix sk-ant-api03-abcdefghijklmnopqrstuvwxyzABCDEFGH12345678 suffix"
                .to_vec(),
            evidence_id: "evidence-redacting".to_string(),
            object_id: "evidence-object-redacting".to_string(),
            object_kind: "evidence".to_string(),
            domain_id: DOMAIN.to_string(),
            source_kind: "tool_output".to_string(),
            source_id: "native/redacting".to_string(),
            source_revision: 1,
            media_type: "text/plain".to_string(),
            retention_class: "canonical".to_string(),
            retain_until: None,
            asserted_sensitivity: Sensitivity::Normal,
            provider_egress: ProviderEgress::RemoteAllowed,
            provenance: Some(RepositoryProvenance {
                repository_id: "repo".to_string(),
                revision: "abc123".to_string(),
            }),
        })
        .unwrap();
    let redacted_bytes = fixture.store.read_artifact(&redacting).unwrap();
    let redacted_text = String::from_utf8(redacted_bytes).unwrap();
    let redacting_evidence = (redacting.evidence_id.clone(), redacting.digest.clone());
    let error = fixture
        .publish(
            "redacting-evidence",
            &request(message(MSG_A, "1"), &redacting_evidence, &redacted_text),
        )
        .unwrap_err();
    assert_eq!(error, SourceDescriptorError::EvidenceNotExact);
    // An identity value the redactor would rewrite is refused, so the stored
    // detail is always the bytes that were hashed.
    let secret_identity: &[(&str, &str)] = &[
        ("project_id", "proj-a"),
        ("harness", "opencode"),
        ("session_id", "sess-01"),
        (
            "message_id",
            "sk-ant-api03-abcdefghijklmnopqrstuvwxyzABCDEFGH12345678",
        ),
        ("block_index", "0"),
    ];
    let error = fixture
        .publish(
            "secret-identity",
            &request(message(secret_identity, "1"), &evidence, "some text"),
        )
        .unwrap_err();
    assert_eq!(error, SourceDescriptorError::ContentRefused);
    // A repeated identity field is not a field of the class.
    let duplicated: &[(&str, &str)] = &[
        ("project_id", "proj-a"),
        ("harness", "opencode"),
        ("session_id", "sess-01"),
        ("message_id", "msg-001"),
        ("block_index", "0"),
        ("block_index", "1"),
    ];
    let error = fixture
        .publish(
            "duplicate-field",
            &request(message(duplicated, "1"), &evidence, "some text"),
        )
        .unwrap_err();
    assert_eq!(
        error,
        SourceDescriptorError::Occurrence(OccurrenceRefusal::UnknownIdentityField)
    );
    let tip = fixture.store.tip().unwrap();

    // A span inside a multibyte character is refused against the real buffer.
    let error = fixture
        .publish(
            "misaligned",
            &SourceDescriptorRequest {
                occurrence: Occurrence {
                    class: "raw_tool_spans",
                    identity: TOOL,
                    revision: "1",
                    representation: "tool_output",
                    span: Some(Span { start: 0, end: 2 }),
                },
                ..request(message(MSG_A, "1"), &unicode, "日本")
            },
        )
        .unwrap_err();
    assert_eq!(
        error,
        SourceDescriptorError::Occurrence(OccurrenceRefusal::SpanNotUtf8Aligned)
    );
    assert_eq!(
        fixture.store.tip().unwrap(),
        tip,
        "a refusal commits nothing"
    );
    let snapshot = fixture.store.slice_as_of(tip).unwrap();
    assert!(
        snapshot
            .observations
            .iter()
            .all(|row| row.observation_kind != SOURCE_DESCRIPTOR_KIND)
    );
}

#[test]
fn every_request_in_a_batch_proves_its_buffer_against_the_artifact() {
    let fixture = Fixture::open();
    let text = "some text";
    let evidence = fixture.retain("text", text);
    // The second request reuses `evidence` but supplies `forged`: the span
    // fits `forged` but exceeds the retained artifact, and `forged` has a
    // different digest.
    let forged = "some text that runs on";
    let requests = [
        request(message(MSG_A, "1"), &evidence, text),
        SourceDescriptorRequest {
            occurrence: Occurrence {
                class: "raw_tool_spans",
                identity: TOOL,
                revision: "1",
                representation: "tool_output",
                span: Some(Span { start: 10, end: 14 }),
            },
            ..request(message(MSG_B, "1"), &evidence, forged)
        },
    ];
    let tip = fixture.store.tip().unwrap();
    let error = fixture.publish_batch("forged", &requests).unwrap_err();
    assert_eq!(error, SourceDescriptorError::BufferMismatch);
    assert_eq!(fixture.store.tip().unwrap(), tip);
    assert!(fixture.descriptor_object_ids_at_tip().is_empty());
}

#[test]
fn a_swallowed_refusal_still_fails_the_commit() {
    let fixture = Fixture::open();
    let evidence = fixture.retain("text", "some text");
    let good = request(message(MSG_A, "1"), &evidence, "some text");
    let bad = SourceDescriptorRequest {
        occurrence: Occurrence {
            class: "user_memories",
            ..message(MSG_B, "1")
        },
        ..request(message(MSG_B, "1"), &evidence, "some text")
    };
    let tip = fixture.store.tip().unwrap();
    // The closure discards the refusal and asks to commit anyway.
    let receipt = fixture.store.commit(intent("swallowed"), |envelope| {
        let _ = envelope.publish_source_descriptors(&[good.clone(), bad.clone()]);
        Ok(String::new())
    });
    assert!(
        receipt.is_err(),
        "a refused batch must not commit: {receipt:?}"
    );
    assert_eq!(fixture.store.tip().unwrap(), tip);
    assert!(fixture.descriptor_object_ids_at_tip().is_empty());
    // The commit rejects a refusal raised before any row is written.
    let receipt = fixture.store.commit(intent("swallowed-first"), |envelope| {
        let _ = envelope.publish_source_descriptor(&bad);
        Ok(String::new())
    });
    assert!(receipt.is_err());
    assert_eq!(fixture.store.tip().unwrap(), tip);
}

#[test]
fn retirement_preserves_domain_ownership_without_reusing_old_revision_ids() {
    let fixture = Fixture::open();
    let v1 = fixture.retain("v1", "first text");
    let v2 = fixture.retain("v2", "second text");
    let v3 = fixture.retain("v3", "third text");
    fixture
        .publish("rev-1", &request(message(MSG_A, "1"), &v1, "first text"))
        .unwrap();
    let (third, _, _) = fixture
        .publish("rev-3", &request(message(MSG_A, "3"), &v3, "third text"))
        .unwrap();
    fixture
        .store
        .commit(intent("retire-3"), |envelope| {
            envelope.retire_observation(&third.object_id)?;
            envelope.insert_domain(DomainSpec {
                domain_id: "other-domain".to_string(),
                object_id: "other-domain-object".to_string(),
                name: "other".to_string(),
                source_kind: "fixture".to_string(),
                source_id: "other-domain".to_string(),
                source_revision: 1,
                sensitivity: Sensitivity::Normal,
            })?;
            Ok(String::new())
        })
        .unwrap();
    let tip = fixture.store.tip().unwrap();
    let foreign = SourceDescriptorRequest {
        domain_id: "other-domain",
        ..request(message(MSG_A, "4"), &v3, "third text")
    };
    assert_eq!(
        fixture.publish("foreign-after-retirement", &foreign).err(),
        Some(SourceDescriptorError::DomainMismatch)
    );
    assert_eq!(fixture.store.tip().unwrap(), tip);
    assert!(fixture.descriptor_object_ids_at_tip().is_empty());
    // With no live head, revision 2 starts a fresh chain.
    let (second, _, _) = fixture
        .publish("rev-2", &request(message(MSG_A, "2"), &v2, "second text"))
        .unwrap();
    assert!(second.replaced_object_id.is_none());
    // Revision 3 advances the live head but its object id is already taken
    // by the retired row: that is a registry conflict, not a stale revision.
    let error = fixture
        .publish(
            "rev-3-again",
            &request(message(MSG_A, "3"), &v3, "third text"),
        )
        .unwrap_err();
    assert_eq!(error, SourceDescriptorError::Kernel(KernelError::Conflict));
    assert_eq!(fixture.live(&second.object_id), Some(true));
    // A revision that does not advance is still named as such.
    let error = fixture
        .publish(
            "rev-2-again",
            &request(message(MSG_A, "2"), &v2, "second text"),
        )
        .unwrap_err();
    assert_eq!(error, SourceDescriptorError::RevisionNotAdvanced);
}

#[test]
fn a_lineage_id_never_replaces_native_identity() {
    let fixture = Fixture::open();
    let evidence = fixture.retain("text", "some text");
    let (first, _, _) = fixture
        .publish(
            "first",
            &request(message(MSG_A, "1"), &evidence, "some text"),
        )
        .unwrap();
    // Corrupt the stored detail so the lineage id names a row whose native
    // identity differs: the next revision must refuse rather than fold the
    // two lineages together.
    let connection = rusqlite::Connection::open_with_flags(
        fixture.root.path().join("kernel.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
    )
    .unwrap();
    let payload: Vec<u8> = connection
        .query_row(
            "SELECT observation_payload FROM observations WHERE object_id=?1",
            [&first.object_id],
            |row| row.get(0),
        )
        .unwrap();
    let mut stored: Value = serde_json::from_slice(&payload).unwrap();
    let mut detail: SourceDescriptorDetail =
        serde_json::from_str(stored["detail"].as_str().unwrap()).unwrap();
    detail.identity[3].1 = "msg-999".to_string();
    stored["detail"] = Value::String(serde_json::to_string(&detail).unwrap());
    connection
        .execute(
            "UPDATE observations SET observation_payload=?1 WHERE object_id=?2",
            rusqlite::params![serde_json::to_vec(&stored).unwrap(), first.object_id],
        )
        .unwrap();
    let tip = fixture.store.tip().unwrap();
    let error = fixture
        .publish(
            "second",
            &request(message(MSG_A, "2"), &evidence, "some text"),
        )
        .unwrap_err();
    assert_eq!(error, SourceDescriptorError::LineageCollision);
    assert_eq!(fixture.store.tip().unwrap(), tip);
    assert_eq!(fixture.live(&first.object_id), Some(true));
}

#[test]
fn duplicate_lineages_are_refused_explicitly_within_one_commit() {
    for separate_calls in [false, true] {
        let fixture = Fixture::open();
        let text = "some text";
        let evidence = fixture.retain("text", text);
        let first = request(message(MSG_A, "1"), &evidence, text);
        let second = request(
            Occurrence {
                span: Some(Span {
                    start: 0,
                    end: text.len() as u64,
                }),
                ..message(MSG_A, "2")
            },
            &evidence,
            text,
        );
        let tip = fixture.store.tip().unwrap();
        let receipt = fixture
            .store
            .commit(intent("duplicate-lineage"), |envelope| {
                let error = if separate_calls {
                    envelope.publish_source_descriptor(&first).unwrap();
                    envelope.publish_source_descriptor(&second).unwrap_err()
                } else {
                    envelope
                        .publish_source_descriptors(&[first, second])
                        .unwrap_err()
                };
                assert_eq!(
                    error.to_string(),
                    "descriptor lineage is published twice in one commit"
                );
                Ok(String::new())
            });
        assert!(
            receipt.is_err(),
            "discarding the refusal cannot commit partial work"
        );
        assert_eq!(fixture.store.tip().unwrap(), tip);
        assert!(fixture.descriptor_object_ids_at_tip().is_empty());
    }
}

#[test]
fn descriptor_bound_covers_all_calls_in_the_envelope() {
    let max = kernel::MAX_DESCRIPTORS_PER_COMMIT;
    for chunk_size in [1, max] {
        let fixture = Fixture::open();
        let text = "some text";
        let evidence = fixture.retain("text", text);
        let ids: Vec<String> = (0..=max).map(|i| format!("claim-{i}")).collect();
        let identities: Vec<_> = ids.iter().map(|id| [("object_id", id.as_str())]).collect();
        let requests: Vec<_> = identities
            .iter()
            .map(|identity| {
                request(
                    Occurrence {
                        class: "canonical_claims",
                        identity,
                        revision: "1",
                        representation: "decision_summary",
                        span: None,
                    },
                    &evidence,
                    text,
                )
            })
            .collect();
        assert_eq!(
            fixture.publish_batch("oversized", &requests).err(),
            Some(SourceDescriptorError::BatchTooLarge)
        );
        let tip = fixture.store.tip().unwrap();
        let receipt = fixture
            .store
            .commit(intent("cumulative-bound"), |envelope| {
                for batch in requests[..max].chunks(chunk_size) {
                    envelope.publish_source_descriptors(batch).unwrap();
                }
                assert_eq!(
                    envelope.publish_source_descriptor(&requests[max]).err(),
                    Some(SourceDescriptorError::BatchTooLarge)
                );
                Ok(String::new())
            });
        assert!(receipt.is_err());
        assert_eq!(fixture.store.tip().unwrap(), tip);
        assert!(fixture.descriptor_object_ids_at_tip().is_empty());
        assert_eq!(
            fixture
                .publish_batch("at-bound", &requests[..max])
                .unwrap()
                .len(),
            max
        );
        fixture.publish("fresh-envelope", &requests[max]).unwrap();
        assert_eq!(fixture.descriptor_object_ids_at_tip().len(), max + 1);
    }
}

#[test]
fn an_occurrence_id_never_aliases_unequal_tuple_bytes() {
    for retired in [false, true] {
        let fixture = Fixture::open();
        let text = "some text";
        let evidence = fixture.retain("text", text);
        let (first, _, _) = fixture
            .publish("first", &request(message(MSG_A, "1"), &evidence, text))
            .unwrap();
        if retired {
            fixture
                .store
                .commit(intent("retire"), |envelope| {
                    envelope.retire_observation(&first.object_id)?;
                    Ok(String::new())
                })
                .unwrap();
        }
        let a = encode(&message(MSG_A, "1"), text).unwrap();
        let b = encode(&message(MSG_B, "1"), text).unwrap();
        assert_ne!(a.tuple, b.tuple);
        assert_ne!(a.lineage_id, b.lineage_id);
        let connection = rusqlite::Connection::open_with_flags(
            fixture.root.path().join("kernel.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .unwrap();
        let (observation_id, payload): (String, Vec<u8>) = connection
            .query_row(
                "SELECT observation_id,observation_payload FROM observations WHERE object_id=?1",
                [&first.object_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(observation_id, format!("srcocc:{}", first.occurrence_id));
        let mut stored: Value = serde_json::from_slice(&payload).unwrap();
        let mut detail: Value = serde_json::from_str(stored["detail"].as_str().unwrap()).unwrap();
        detail["occurrence_id"] = serde_json::json!(b.occurrence_id);
        detail["occurrence_tuple"] = serde_json::json!(a.tuple);
        stored["detail"] = Value::String(serde_json::to_string(&detail).unwrap());
        connection.execute(
            "UPDATE observations SET observation_id=?1,observation_payload=?2 WHERE object_id=?3",
            rusqlite::params![format!("srcocc:{}", b.occurrence_id), serde_json::to_vec(&stored).unwrap(), first.object_id],
        ).unwrap();
        let tip = fixture.store.tip().unwrap();
        let result = fixture.publish("colliding", &request(message(MSG_B, "1"), &evidence, text));
        assert_eq!(
            result.err().map(|error| error.to_string()),
            Some("descriptor occurrence id names a different stored tuple".to_string())
        );
        assert_eq!(fixture.store.tip().unwrap(), tip);
        assert_eq!(fixture.live(&first.object_id), Some(!retired));
        assert_eq!(
            fixture.live(&descriptor_object_id(&b.lineage_id, "1")),
            None
        );
    }
}

#[test]
fn public_encoder_canonicalizes_whole_buffer_selection() {
    for buffer in ["aé", ""] {
        let whole = message(MSG_A, "1");
        let explicit = Occurrence {
            span: Some(Span {
                start: 0,
                end: buffer.len() as u64,
            }),
            ..whole.clone()
        };
        validate_span(explicit.span, buffer).unwrap();
        assert_eq!(
            encode(&explicit, buffer).unwrap(),
            encode(&whole, buffer).unwrap()
        );
    }
    let buffer = "aéb";
    let whole = message(MSG_A, "1");
    let part = Occurrence {
        span: Some(Span { start: 0, end: 3 }),
        ..whole.clone()
    };
    assert_ne!(
        encode(&part, buffer).unwrap().occurrence_id,
        encode(&whole, buffer).unwrap().occurrence_id
    );
    for (span, expected) in [
        (Span { start: 3, end: 1 }, OccurrenceRefusal::SpanReversed),
        (Span { start: 0, end: 5 }, OccurrenceRefusal::SpanOutOfRange),
        (
            Span { start: 0, end: 2 },
            OccurrenceRefusal::SpanNotUtf8Aligned,
        ),
    ] {
        let invalid = Occurrence {
            span: Some(span),
            ..whole.clone()
        };
        assert_eq!(encode(&invalid, buffer).err(), Some(expected));
        let invalid_class = Occurrence {
            class: "unknown",
            ..invalid
        };
        assert_eq!(
            encode(&invalid_class, buffer).err(),
            Some(OccurrenceRefusal::UnknownClass)
        );
    }
}

#[test]
fn generic_observation_writes_cannot_mint_or_replace_descriptors() {
    let fixture = Fixture::open();
    let text = "some text";
    let evidence = fixture.retain("text", text);
    let (descriptor, _, _) = fixture
        .publish("typed", &request(message(MSG_A, "1"), &evidence, text))
        .unwrap();
    let generic = kernel::ObservationSpec {
        observation_id: "generic".to_string(),
        object_id: "generic".to_string(),
        domain_id: DOMAIN.to_string(),
        proposition_id: None,
        scope_id: None,
        anchor_id: None,
        evidence_id: None,
        observation_kind: "probe".to_string(),
        payload: kernel::ObservationPayload {
            summary: "probe".to_string(),
            classification: "probe".to_string(),
            detail: None,
        },
        observed_at: 1,
        dependencies: Vec::new(),
        source_kind: "messages".to_string(),
        source_id: descriptor.lineage_id.clone(),
        source_revision: 1,
        sensitivity: Sensitivity::Normal,
    };
    fixture
        .store
        .commit(intent("generic"), |envelope| {
            envelope.insert_observation(generic.clone())?;
            Ok(String::new())
        })
        .unwrap();
    let tip = fixture.store.tip().unwrap();
    for (i, (predecessor, kind)) in [
        (None, SOURCE_DESCRIPTOR_KIND),
        (Some("generic"), SOURCE_DESCRIPTOR_KIND),
        (Some(descriptor.object_id.as_str()), "probe"),
    ]
    .into_iter()
    .enumerate()
    {
        let forged = kernel::ObservationSpec {
            observation_id: format!("forged-{i}"),
            object_id: format!("forged-{i}"),
            observation_kind: kind.to_string(),
            source_revision: 2,
            ..generic.clone()
        };
        let receipt = fixture
            .store
            .commit(intent(&format!("forge-{i}")), |envelope| {
                let result = match predecessor {
                    None => envelope.insert_observation(forged),
                    Some(id) => envelope.correct_observation(id, forged),
                };
                assert_eq!(result.err(), Some(KernelError::InvalidInput));
                Ok(String::new())
            });
        assert_eq!(receipt.err(), Some(KernelError::InvalidInput));
        assert_eq!(fixture.store.tip().unwrap(), tip);
        assert_eq!(fixture.live(&descriptor.object_id), Some(true));
        assert_eq!(fixture.live("generic"), Some(true));
        assert_eq!(fixture.live(&format!("forged-{i}")), None);
    }
}

#[test]
fn lineage_collision_checks_include_other_source_classes() {
    for retired in [false, true] {
        let fixture = Fixture::open();
        let text = "some text";
        let evidence = fixture.retain("text", text);
        let (original, _, _) = fixture
            .publish("first", &request(message(MSG_A, "1"), &evidence, text))
            .unwrap();
        if retired {
            fixture
                .store
                .commit(intent("retire"), |envelope| {
                    envelope.retire_observation(&original.object_id)?;
                    Ok(String::new())
                })
                .unwrap();
        }
        let incoming = Occurrence {
            class: "canonical_claims",
            identity: &[("object_id", "claim-7")],
            revision: "2",
            representation: "decision_summary",
            span: None,
        };
        let encoded = encode(&incoming, text).unwrap();
        let alias_id = descriptor_object_id(&encoded.lineage_id, "1");
        let mut connection = rusqlite::Connection::open_with_flags(
            fixture.root.path().join("kernel.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .unwrap();
        let tx = connection.transaction().unwrap();
        tx.execute(
            "INSERT INTO object_registry SELECT ?1,object_kind,domain_id,source_kind,?2,
             source_revision,created_commit_seq,invalidated_commit_seq,superseded_by,sensitivity_class
             FROM object_registry WHERE object_id=?3",
            rusqlite::params![alias_id, encoded.lineage_id, original.object_id],
        ).unwrap();
        tx.execute(
            "INSERT INTO observations SELECT ?1,?1,proposition_id,scope_id,anchor_id,evidence_id,
             observation_kind,observation_payload,observed_at,created_commit_seq,
             invalidated_commit_seq,superseded_by,sensitivity_class
             FROM observations WHERE object_id=?2",
            rusqlite::params![alias_id, original.object_id],
        )
        .unwrap();
        tx.commit().unwrap();
        let tip = fixture.store.tip().unwrap();
        let result = fixture.publish("collision", &request(incoming, &evidence, text));
        assert_eq!(result.err(), Some(SourceDescriptorError::LineageCollision));
        assert_eq!(fixture.store.tip().unwrap(), tip);
        assert_eq!(fixture.live(&alias_id), Some(!retired));
        assert_eq!(
            fixture.live(&descriptor_object_id(&encoded.lineage_id, "2")),
            None
        );
    }
}

#[test]
fn name_only_remediation_changes_no_descriptor_input() {
    let fixture = Fixture::open();
    let evidence = fixture.retain("text", "some text");
    let (before, _, _) = fixture
        .publish(
            "first",
            &request(message(MSG_A, "1"), &evidence, "some text"),
        )
        .unwrap();
    let detail_before = fixture.detail(&before.object_id).unwrap();
    fixture
        .store
        .commit(intent("remediate"), |envelope| {
            envelope.remediate_text(
                RemediationTarget::CanonicalDomainName {
                    object_id: "domain-object".to_string(),
                },
                "operator",
                5,
            )?;
            Ok(String::new())
        })
        .unwrap();
    let detail_after = fixture.detail(&before.object_id).unwrap();
    assert_eq!(detail_after, detail_before);
    assert_eq!(fixture.live(&before.object_id), Some(true));
    // Re-deriving the identity from the same inputs yields the same ids: the
    // domain name is not an input.
    let again = encode(&message(MSG_A, "1"), "some text").unwrap();
    assert_eq!(again.occurrence_id, before.occurrence_id);
    assert_eq!(again.lineage_id, before.lineage_id);
}
