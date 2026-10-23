//! Both harness adapters against a real kernel and a real projection: native identity is bound or refused, equal text stays distinct, tool strings survive exactly, secret-bearing bytes retain nothing, replay returns receipts, revisions succeed atomically, and only message text creates dense work.

use std::collections::{BTreeMap, BTreeSet};
use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use daemon::embedding_dispatch::{DispatchBounds, DispatchEvent, EmbeddingDispatcher};
use daemon::harness_sources::{
    AdapterRefusal, Harness, MAX_SOURCE_BATCH_BYTES, MAX_SOURCE_BATCH_UNITS, MAX_SOURCE_UNIT_BYTES,
    PublishError, Representation, SessionIdentity, SourcePublisher, SourceUnit, UnitDisposition,
    opencode_units, pi_units, publish_screened,
};
use daemon::search_projection::SearchProjection;
use host_runtime::local_embeddings::inference::InferenceError;
use host_runtime::local_embeddings::{
    EmbedTokens, EmbeddingEngine, LaneInfo, LocalEmbeddingsComponent, LocalEmbeddingsLimits,
};
use kernel::applicability::EvalBudget;
use kernel::source_identity::{
    Occurrence, OccurrenceClass, encode_preserving_span, identity_digest,
};
use kernel::{
    ArtifactDestination, ArtifactErrorKind, CommitIntent, Dimension, DomainSpec,
    EligibilityBinding, ExportWindow, KernelStore, ProjectScope, ProviderEgress, ScopeSpec,
    ScopeTermSpec, Sensitivity, SourceClass, SourceDescriptorError, SourceHoldAdmission,
    SourceHoldBinding, SourceHoldBounds, SourcePageBounds, SourceRow, TaintClass,
};
use retrieval::batch::{
    BatchBounds, MutationIdentity, VectorGeneration, batch_from_rows, register_generation,
    row_identities,
};
use retrieval::dispatch::EpisodeGrant;
use retrieval::{PersistBounds, ProjectionIdentity, install_identity};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const CONSUMER: &str = "search";
const POLICY: &str = "source-policy.v1";
const PROJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SCOPE: &str = "project:a";
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const MODEL: &str = "tiny-test-model";
const FINGERPRINT: &str = "a2b4c6d8e0f01234a2b4c6d8e0f01234a2b4c6d8e0f01234a2b4c6d8e0f01234";
const DIMS: usize = 8;
const GENERATION: &str = "gen-1";
const NOW: i64 = 1_000;
const SESSION: &str = "ses_34b41f9e5ffeRfk5oEGgRT1pIP";
const PI_SESSION: &str = "pi-session-7";
/// A credential shape the bounded scanner detects.
const SECRET: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyzABCDEFGH12345678";
/// Exact tool bytes with CRLF, leading and trailing whitespace, multibyte text, and an emoji.
const TOOL_BYTES: &str = "  error: line 1\r\nwarning: naïve 日本語 🎉\r\n\t";

/// A deterministic engine: one whitespace word is one token and the vector is derived from the text; `calls` counts inferences.
struct TestEngine {
    calls: AtomicUsize,
}

impl TestEngine {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn vector_for(text: &str) -> Vec<f32> {
        let digest = Sha256::digest(text.as_bytes());
        let mut vector: Vec<f32> = digest
            .iter()
            .take(DIMS)
            .map(|b| f32::from(*b) + 1.0)
            .collect();
        let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
        for v in &mut vector {
            *v /= norm;
        }
        vector
    }
}

impl EmbeddingEngine for TestEngine {
    fn untruncated_token_len(&self, text: &str) -> Result<EmbedTokens, InferenceError> {
        Ok(EmbedTokens::new(text.split_whitespace().count() as u32))
    }

    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, InferenceError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(texts.iter().map(|text| Self::vector_for(text)).collect())
    }
}

fn lane(fingerprint: &str) -> LaneInfo {
    LaneInfo {
        model: MODEL.to_owned(),
        fingerprint: fingerprint.to_owned(),
        table_epoch: 1,
        dims: DIMS,
        execution_provider: "cpu",
        max_tokens: 512,
        max_text_bytes: 1024 * 1024,
        provenance: serde_json::json!({"source": "deterministic test engine"}),
        recommended_rows: 16,
        recommended_token_budget: 8192,
    }
}

fn component(engine: &Arc<TestEngine>, limits: LocalEmbeddingsLimits) -> LocalEmbeddingsComponent {
    LocalEmbeddingsComponent::ready_with_engine(
        lane(FINGERPRINT),
        Arc::clone(engine) as Arc<dyn EmbeddingEngine>,
        limits,
    )
    .unwrap()
}

fn generation() -> VectorGeneration {
    VectorGeneration {
        generation_id: GENERATION.to_string(),
        embedding_model: MODEL.to_string(),
        tokenizer_fingerprint: FINGERPRINT.to_string(),
        vector_dimension: DIMS as u32,
        generation_epoch: 1,
    }
}

fn identity(kernel_incarnation_id: &str) -> ProjectionIdentity {
    ProjectionIdentity {
        schema_version: retrieval::SCHEMA_VERSION,
        kernel_incarnation_id: kernel_incarnation_id.to_string(),
        projection_policy_version: POLICY.to_string(),
        identity_contract_version: "search-projection-identity-v4".to_string(),
        limit_manifest_protocol_version: "limits.v1".to_string(),
        embedding_model: MODEL.to_string(),
        tokenizer_fingerprint: FINGERPRINT.to_string(),
        analysis_identity: retrieval::lexical::AnalysisIdentity::current()
            .as_str()
            .to_string(),
        vector_dimension: DIMS as u32,
        generation_epoch: 1,
    }
}

fn batch_bounds() -> BatchBounds {
    BatchBounds {
        persist: PersistBounds {
            max_records: NonZeroUsize::new(64).unwrap(),
            max_payload_bytes: NonZeroUsize::new(1 << 16).unwrap(),
            max_tuple_bytes: NonZeroUsize::new(2048).unwrap(),
        },
        max_source_bytes: NonZeroUsize::new(1 << 16).unwrap(),
        max_local_mutations: NonZeroUsize::new(64).unwrap(),
        max_pending: NonZeroUsize::new(64).unwrap(),
    }
}

fn grant(allowance: u32, deadline: i64) -> EpisodeGrant {
    EpisodeGrant {
        allowance: NonZeroU32::new(allowance).unwrap(),
        deadline,
    }
}

fn bounds() -> DispatchBounds {
    DispatchBounds {
        input: daemon::embedding_dispatch::InputEnvelope::UNBOUNDED,
        max_jobs: NonZeroUsize::new(16).unwrap(),
        grant: grant(3, NOW + DAY_MS),
        retry_after: 10,
        result_wait: Duration::from_secs(5),
    }
}

/// One pass budget: an absolute deadline `wait` from now, with its own sticky cancellation.
fn budget(wait: Duration) -> EvalBudget {
    EvalBudget::new(
        Some(std::time::Instant::now() + wait),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
}

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "daemon-harness-sources-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

/// A kernel with one scoped, admitted project, opened at `root`.
struct Corpus {
    kernel: Arc<KernelStore>,
    kernel_db: PathBuf,
}

impl Corpus {
    fn open(root: &Path) -> Self {
        let kernel_root = root.join("kernel");
        Self {
            kernel: Arc::new(KernelStore::open(&kernel_root).unwrap()),
            kernel_db: kernel_root.join("kernel.sqlite"),
        }
    }

    fn kernel_incarnation_id(&self) -> String {
        Connection::open_with_flags(&self.kernel_db, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap()
            .query_row(
                "SELECT database_incarnation_id FROM kernel_format_marker WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn seed(&self) {
        self.kernel
            .commit(intent("seed"), |envelope| {
                envelope.insert_domain(DomainSpec {
                    domain_id: "domain".to_string(),
                    object_id: "domain-object".to_string(),
                    name: "Name".to_string(),
                    source_kind: "fixture".to_string(),
                    source_id: "domain".to_string(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                })?;
                envelope.insert_scope(ScopeSpec {
                    scope_id: SCOPE.to_string(),
                    object_id: SCOPE.to_string(),
                    source_id: SCOPE.to_string(),
                    domain_id: "domain".to_string(),
                    source_kind: "kernel_route".to_string(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                    terms: vec![ScopeTermSpec {
                        dimension: Dimension::Project.as_str().to_string(),
                        operator: "exact".to_string(),
                        exact_value: Some(PROJECT.to_string()),
                        ..ScopeTermSpec::default()
                    }],
                })?;
                envelope.register_outbox_consumer(CONSUMER, 1)?;
                Ok(String::new())
            })
            .unwrap();
    }

    fn tip(&self) -> i64 {
        self.kernel.tip().unwrap()
    }

    fn publisher(&self) -> SourcePublisher<'_> {
        SourcePublisher {
            kernel: &self.kernel,
            domain_id: "domain",
            scope_id: Some(SCOPE),
            egress: ProviderEgress::LocalOnly,
            sensitivity: Sensitivity::Normal,
        }
    }

    fn binding(&self) -> SourceHoldBinding {
        SourceHoldBinding {
            consumer_id: CONSUMER.to_string(),
            lease_epoch: self.kernel.lease_epoch(),
            source_policy_version: POLICY.to_string(),
        }
    }

    /// Exports every descriptor live at a fresh S, with its text.
    fn export(&self) -> Vec<SourceRow> {
        let binding = self.binding();
        let hold = self
            .kernel
            .capture_source_hold(
                &binding,
                SourceHoldBounds {
                    max_descriptor_rows: NonZeroUsize::new(256).unwrap(),
                    admission: SourceHoldAdmission {
                        max_references: NonZeroUsize::new(64).unwrap(),
                        max_encoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
                    },
                    expiry_ms: NonZeroU64::new((20 * DAY_MS) as u64).unwrap(),
                },
            )
            .unwrap();
        let mut rows = Vec::new();
        let mut cursor = None;
        loop {
            let page = self
                .kernel
                .export_source_page(
                    &binding,
                    &hold.hold_id,
                    hold.captured_at,
                    ExportWindow::Snapshot,
                    cursor.as_ref(),
                    SourcePageBounds {
                        max_rows: NonZeroUsize::new(64).unwrap(),
                        max_encoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
                        max_decoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
                        max_row_bytes: NonZeroU64::new(1 << 16).unwrap(),
                    },
                )
                .unwrap();
            rows.extend(page.rows);
            match page.next {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        self.kernel
            .release_source_hold(&binding, &hold.hold_id, hold.captured_at)
            .unwrap();
        rows
    }

    /// Builds the projection from the current export and queues one pending job per dense-eligible occurrence.
    fn bootstrap(&self, data_home: &Path) -> (SearchProjection, Vec<SourceRow>) {
        let rows = self.export();
        let projection = SearchProjection::open(data_home).unwrap();
        let kernel_incarnation_id = self.kernel_incarnation_id();
        projection
            .write(|conn| {
                install_identity(conn, &identity(&kernel_incarnation_id), 1)?;
                register_generation(conn, &generation(), 1)?;
                Ok(())
            })
            .unwrap();
        let identities = row_identities(&rows);
        let snapshot = self.tip();
        let batch = batch_from_rows(
            &rows,
            &identities,
            MutationIdentity {
                kernel_incarnation_id,
                hold_id: "0123456789abcdef0123456789abcdef".to_string(),
                snapshot_commit_seq: snapshot,
                through_commit_seq: snapshot,
            },
            Some(GENERATION),
        )
        .unwrap();
        projection.apply_batch(&batch, batch_bounds(), 2).unwrap();
        (projection, rows)
    }
}

fn opencode_session() -> SessionIdentity {
    SessionIdentity {
        project_id: PROJECT.to_string(),
        harness: Harness::OpenCode,
        session_id: SESSION.to_string(),
    }
}

fn pi_session() -> SessionIdentity {
    SessionIdentity {
        project_id: PROJECT.to_string(),
        harness: Harness::Pi,
        session_id: PI_SESSION.to_string(),
    }
}

/// An OpenCode assistant record shaped like the harness's own capture: a step marker, reasoning, a text part, a settled tool, a failed tool, an empty text part, and a running tool.
fn opencode_assistant(id: &str, completed: u64) -> Value {
    json!({
        "info": {
            "id": id,
            "sessionID": SESSION,
            "role": "assistant",
            "time": { "created": completed - 1000, "completed": completed },
            "parentID": "msg_parent"
        },
        "parts": [
            { "type": "step-start" },
            { "type": "reasoning", "text": "thinking" },
            { "type": "text", "text": "Hello from the assistant" },
            { "type": "tool", "callID": "toolu_01", "tool": "bash",
              "state": { "status": "completed", "input": {}, "output": TOOL_BYTES, "time": { "start": 1, "end": 4200 } } },
            { "type": "tool", "callID": "toolu_02", "tool": "task",
              "state": { "status": "error", "input": {}, "error": "boom\n", "time": { "start": 1, "end": 4300 } } },
            { "type": "text", "text": "" },
            { "type": "tool", "callID": "toolu_03", "tool": "bash", "state": { "status": "running", "input": {} } }
        ]
    })
}

fn opencode_user(id: &str, text: &str, created: u64) -> Value {
    json!({
        "info": { "id": id, "sessionID": SESSION, "role": "user", "time": { "created": created } },
        "parts": [ { "type": "text", "text": text } ]
    })
}

fn pi_user(id: &str, text: &str, stamp: u64) -> Value {
    json!({ "type": "message", "id": id, "parentId": null, "timestamp": "2026-04-27T10:32:43.588Z",
            "message": { "role": "user", "content": text, "timestamp": stamp } })
}

fn pi_user_with_image(id: &str, stamp: u64) -> Value {
    json!({ "type": "message", "id": id, "parentId": null, "timestamp": "2026-04-27T10:32:43.588Z",
            "message": { "role": "user", "timestamp": stamp, "content": [
                { "type": "text", "text": "look at this" },
                { "type": "image", "data": "iVBORw0KGgo=", "mimeType": "image/png" }
            ] } })
}

fn pi_assistant(id: &str, stamp: u64) -> Value {
    json!({ "type": "message", "id": id, "parentId": "pi-u1", "timestamp": "2026-04-27T10:32:44.000Z",
            "message": { "role": "assistant", "timestamp": stamp, "content": [
                { "type": "thinking", "thinking": "hm" },
                { "type": "text", "text": "Let me run it" },
                { "type": "toolCall", "id": "call_65aZ", "name": "bash", "arguments": {} }
            ] } })
}

/// A Pi tool result with two text parts: multipart output stays two blocks.
fn pi_tool_result(id: &str, parent: &str, is_error: bool, stamp: u64) -> Value {
    json!({ "type": "message", "id": id, "parentId": parent, "timestamp": "2026-04-27T10:32:45.000Z",
            "message": { "role": "toolResult", "toolCallId": "call_65aZ", "toolName": "bash", "isError": is_error,
                         "timestamp": stamp,
                         "content": [ { "type": "text", "text": TOOL_BYTES }, { "type": "text", "text": "" } ] } })
}

fn identity_of(unit: &SourceUnit) -> Vec<(&'static str, &str)> {
    unit.identity
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect()
}

/// AC1, AC2, AC3: both adapters bind the exact native identity tuple in the class's field order, keep block order and bytes, keep tool strings out of message text, never join multipart output, and refuse every missing identity, another session, and unsupported shapes.
#[test]
fn adapters_bind_native_identity_exactly_and_refuse_missing_identity() {
    let units = opencode_units(&opencode_session(), &opencode_assistant("msg_a", 5000)).unwrap();
    assert_eq!(
        units.len(),
        4,
        "text, settled tool, failed tool, empty text; nothing for reasoning, step markers, or a running tool"
    );
    assert_eq!(
        identity_of(&units[0]),
        vec![
            ("project_id", PROJECT),
            ("harness", "opencode"),
            ("session_id", SESSION),
            ("message_id", "msg_a"),
            ("block_index", "2")
        ]
    );
    assert_eq!(
        (
            units[0].class,
            units[0].representation,
            units[0].revision.as_str(),
            units[0].text.as_str(),
            units[0].role.as_str()
        ),
        (
            OccurrenceClass::Messages,
            Representation::Text,
            "5000",
            "Hello from the assistant",
            "assistant"
        )
    );
    assert_eq!(
        identity_of(&units[1]),
        vec![
            ("project_id", PROJECT),
            ("harness", "opencode"),
            ("session_id", SESSION),
            ("parent_message_id", "msg_a"),
            ("tool_call_id", "toolu_01"),
            ("result_revision", "4200"),
            ("block_index", "0")
        ]
    );
    assert_eq!(
        (
            units[1].class,
            units[1].representation,
            units[1].text.as_str()
        ),
        (
            OccurrenceClass::RawToolSpans,
            Representation::ToolOutput,
            TOOL_BYTES
        )
    );
    assert_eq!(
        (
            units[2].representation,
            units[2].text.as_str(),
            units[2].revision.as_str()
        ),
        (Representation::ToolError, "boom\n", "4300")
    );
    assert_eq!(
        (
            units[3].class,
            units[3].text.as_str(),
            units[3].identity[4].1.as_str()
        ),
        (OccurrenceClass::Messages, "", "5")
    );
    assert!(
        units
            .iter()
            .filter(|unit| unit.class == OccurrenceClass::Messages)
            .all(|unit| !unit.text.contains("error: line")),
        "tool output is never message text"
    );

    let units = pi_units(&pi_session(), &pi_tool_result("pi-t1", "pi-a1", true, 7777)).unwrap();
    assert_eq!(units.len(), 2, "two text parts stay two blocks");
    assert_eq!(
        identity_of(&units[0]),
        vec![
            ("project_id", PROJECT),
            ("harness", "pi"),
            ("session_id", PI_SESSION),
            ("parent_message_id", "pi-a1"),
            ("tool_call_id", "call_65aZ"),
            ("result_revision", "7777"),
            ("block_index", "0")
        ]
    );
    assert_eq!(
        (units[0].representation, units[0].text.as_str()),
        (Representation::ToolError, TOOL_BYTES)
    );
    assert_eq!(
        (units[1].identity[6].1.as_str(), units[1].text.as_str()),
        ("1", "")
    );
    // A Pi tool result whose native id is a composite `call|item` value keeps the composite verbatim: the identity is the harness's spelling, not the wire codec's canonical call id.
    let mut composite = pi_tool_result("pi-t2", "pi-a1", false, 7778);
    composite["message"]["toolCallId"] = json!("call_65aZ|item-2");
    let units = pi_units(&pi_session(), &composite).unwrap();
    assert_eq!(
        units[0].identity[4],
        ("tool_call_id", "call_65aZ|item-2".to_string())
    );
    let units = pi_units(&pi_session(), &pi_assistant("pi-a1", 6000)).unwrap();
    assert_eq!(
        units.len(),
        1,
        "thinking and tool calls are not text blocks"
    );
    assert_eq!(
        (
            units[0].identity[3].1.as_str(),
            units[0].identity[4].1.as_str(),
            units[0].text.as_str()
        ),
        ("pi-a1", "1", "Let me run it")
    );
    let units = pi_units(&pi_session(), &pi_user("pi-u1", "run it", 5500)).unwrap();
    assert_eq!(
        (
            units.len(),
            units[0].identity[4].1.as_str(),
            units[0].role.as_str()
        ),
        (1, "0", "user")
    );
    // AC10 and OQ10: a row names its entry type, and only a message entry holds sources. Coverage witness `pi_wire_row_without_type`: the `{id, message}` row a pre-U3 plugin sent.
    for (row, refusal) in [
        (
            json!({ "id": "pi-u1", "message": { "role": "user", "content": "x", "timestamp": 1 } }),
            AdapterRefusal::MissingIdentity("type"),
        ),
        (
            json!({ "type": "compaction", "id": "x", "parentId": "pi-u1" }),
            AdapterRefusal::UnsupportedShape("type"),
        ),
        (
            json!({ "type": "session", "id": "x" }),
            AdapterRefusal::UnsupportedShape("type"),
        ),
        (
            json!({ "type": 7, "id": "x" }),
            AdapterRefusal::UnsupportedShape("type"),
        ),
        (
            json!({ "type": null, "id": "x" }),
            AdapterRefusal::MissingIdentity("type"),
        ),
        (
            json!({ "type": "", "id": "x" }),
            AdapterRefusal::MissingIdentity("type"),
        ),
    ] {
        assert_eq!(pi_units(&pi_session(), &row), Err(refusal), "{row}");
    }
    let mut orphan = pi_tool_result("pi-t9", "pi-a1", false, 6000);
    orphan.as_object_mut().unwrap().remove("parentId");
    assert_eq!(
        pi_units(&pi_session(), &orphan),
        Err(AdapterRefusal::MissingIdentity("parentId"))
    );
    // The captured Pi shape: a content array with an image beside the text; the image is not a text source.
    let units = pi_units(&pi_session(), &pi_user_with_image("pi-u2", 5600)).unwrap();
    assert_eq!(
        (
            units.len(),
            units[0].identity[4].1.as_str(),
            units[0].text.as_str()
        ),
        (1, "0", "look at this")
    );
    // An ignored OpenCode text part is not conversation text.
    let mut ignored = opencode_user("msg_i", "not for the model", 1);
    ignored["parts"][0]["ignored"] = json!(true);
    assert_eq!(
        opencode_units(&opencode_session(), &ignored).unwrap(),
        vec![]
    );
    // Debug output of a unit names identities and sizes, never text.
    let debug = format!(
        "{:?}",
        opencode_units(
            &opencode_session(),
            &opencode_user("msg_d", "private words", 1)
        )
        .unwrap()
    );
    assert!(
        debug.contains("msg_d") && debug.contains("text_bytes") && !debug.contains("private words")
    );

    // Refusals name a field or shape, never content.
    let mut other_session = opencode_user("msg_u", "hi", 1);
    other_session["info"]["sessionID"] = json!("ses_other");
    assert_eq!(
        opencode_units(&opencode_session(), &other_session),
        Err(AdapterRefusal::SessionMismatch)
    );
    let mut missing_id = opencode_user("msg_u", "hi", 1);
    missing_id["info"].as_object_mut().unwrap().remove("id");
    assert_eq!(
        opencode_units(&opencode_session(), &missing_id),
        Err(AdapterRefusal::MissingIdentity("id"))
    );
    let mut missing_time = opencode_user("msg_u", "hi", 1);
    missing_time["info"].as_object_mut().unwrap().remove("time");
    assert_eq!(
        opencode_units(&opencode_session(), &missing_time),
        Err(AdapterRefusal::MissingIdentity("time"))
    );
    let mut no_call = opencode_assistant("msg_a", 5000);
    no_call["parts"][3]
        .as_object_mut()
        .unwrap()
        .remove("callID");
    assert_eq!(
        opencode_units(&opencode_session(), &no_call),
        Err(AdapterRefusal::MissingIdentity("callID"))
    );
    let mut no_end = opencode_assistant("msg_a", 5000);
    no_end["parts"][3]["state"]
        .as_object_mut()
        .unwrap()
        .remove("time");
    assert_eq!(
        opencode_units(&opencode_session(), &no_end),
        Err(AdapterRefusal::MalformedRevision("state.time.end"))
    );
    let mut json_output = opencode_assistant("msg_a", 5000);
    json_output["parts"][3]["state"]["output"] = json!({ "structured": true });
    assert_eq!(
        opencode_units(&opencode_session(), &json_output),
        Err(AdapterRefusal::UnsupportedShape("tool output"))
    );
    assert_eq!(
        opencode_units(&opencode_session(), &json!([])),
        Err(AdapterRefusal::NotAnObject)
    );
    for field in ["id", "sessionID", "role"] {
        let mut record = opencode_user("msg_u", "hi", 1);
        record["info"].as_object_mut().unwrap().remove(field);
        assert_eq!(
            opencode_units(&opencode_session(), &record),
            Err(AdapterRefusal::MissingIdentity(field)),
            "{field}"
        );
    }
    let mut stateless = opencode_assistant("msg_a", 5000);
    stateless["parts"][3]
        .as_object_mut()
        .unwrap()
        .remove("state");
    assert_eq!(
        opencode_units(&opencode_session(), &stateless),
        Err(AdapterRefusal::UnsupportedShape("tool part"))
    );
    let mut no_entry_id = pi_user("pi-u1", "run it", 5500);
    no_entry_id.as_object_mut().unwrap().remove("id");
    assert_eq!(
        pi_units(&pi_session(), &no_entry_id),
        Err(AdapterRefusal::MissingIdentity("id"))
    );
    let mut no_message = pi_user("pi-u1", "run it", 5500);
    no_message["message"] = json!("not an object");
    assert_eq!(
        pi_units(&pi_session(), &no_message),
        Err(AdapterRefusal::MissingIdentity("message"))
    );
    let mut no_role = pi_user("pi-u1", "run it", 5500);
    no_role["message"].as_object_mut().unwrap().remove("role");
    assert_eq!(
        pi_units(&pi_session(), &no_role),
        Err(AdapterRefusal::MissingIdentity("role"))
    );
    let mut odd_content = pi_user("pi-u1", "run it", 5500);
    odd_content["message"]["content"] = json!(42);
    assert_eq!(
        pi_units(&pi_session(), &odd_content),
        Err(AdapterRefusal::UnsupportedShape("content"))
    );
    let mut no_stamp = pi_user("pi-u1", "run it", 5500);
    no_stamp["message"]
        .as_object_mut()
        .unwrap()
        .remove("timestamp");
    assert_eq!(
        pi_units(&pi_session(), &no_stamp),
        Err(AdapterRefusal::MalformedRevision("timestamp"))
    );
    let mut no_parent = pi_tool_result("pi-t1", "pi-a1", false, 7777);
    no_parent.as_object_mut().unwrap().remove("parentId");
    assert_eq!(
        pi_units(&pi_session(), &no_parent),
        Err(AdapterRefusal::MissingIdentity("parentId"))
    );
    let mut no_call = pi_tool_result("pi-t1", "pi-a1", false, 7777);
    no_call["message"]
        .as_object_mut()
        .unwrap()
        .remove("toolCallId");
    assert_eq!(
        pi_units(&pi_session(), &no_call),
        Err(AdapterRefusal::MissingIdentity("toolCallId"))
    );
    let mut image = pi_tool_result("pi-t1", "pi-a1", false, 7777);
    image["message"]["content"][1] = json!({ "type": "image", "data": "..." });
    assert_eq!(
        pi_units(&pi_session(), &image),
        Err(AdapterRefusal::UnsupportedShape("tool result item"))
    );
}

/// The `(class, text, occurrence id)` of every live descriptor in the kernel, read through its export.
fn inventory(corpus: &Corpus) -> BTreeSet<(String, String, String)> {
    corpus
        .export()
        .into_iter()
        .filter(|row| row.invalidated_commit_seq.is_none())
        .map(|row| {
            (
                row.detail.class.clone(),
                row.text.clone().unwrap_or_default(),
                row.detail.occurrence_id.clone(),
            )
        })
        .collect()
}

#[test]
fn pi_refuses_unsupported_message_roles() {
    for role in ["system", "developer", "custom", "future-role"] {
        let mut entry = pi_user("pi-u1", "not conversation text", 100);
        entry["message"]["role"] = json!(role);
        assert_eq!(
            pi_units(&pi_session(), &entry),
            Err(AdapterRefusal::UnsupportedShape("message.role")),
            "{role}"
        );
    }
}

#[test]
fn opencode_refuses_unsupported_roles_before_projecting_any_parts() {
    for role in ["system", "developer", "tool", "toolResult", "future-role"] {
        for kind in ["text", "tool", "no-parts"] {
            let mut record = opencode_assistant("msg_a", 5000);
            record["info"]["role"] = json!(role);
            record["parts"]
                .as_array_mut()
                .unwrap()
                .retain(|part| part["type"] == kind);
            assert_eq!(
                opencode_units(&opencode_session(), &record),
                Err(AdapterRefusal::UnsupportedShape("info.role")),
                "role={role}, parts={kind}"
            );
        }
    }
}

#[test]
fn pi_tool_results_require_boolean_error_status() {
    for (is_error, representation) in [
        (false, Representation::ToolOutput),
        (true, Representation::ToolError),
    ] {
        let units = pi_units(
            &pi_session(),
            &pi_tool_result("pi-t1", "pi-a1", is_error, 7777),
        )
        .unwrap();
        assert_eq!(units.len(), 2);
        for unit in &units {
            assert_eq!(unit.representation, representation);
            assert_eq!(unit.class, OccurrenceClass::RawToolSpans);
        }
        assert_eq!(units[0].text, TOOL_BYTES);
        assert_eq!(units[1].text, "");
    }
    for status in [
        None,
        Some(Value::Null),
        Some(json!("true")),
        Some(json!("false")),
        Some(json!(0)),
        Some(json!(1)),
        Some(json!([])),
        Some(json!({})),
    ] {
        for mut entry in [
            pi_user("pi-u1", "user text", 100),
            pi_assistant("pi-a1", 200),
        ] {
            let expected = pi_units(&pi_session(), &entry).unwrap();
            if let Some(status) = status.clone() {
                entry["message"]["isError"] = status;
            }
            assert_eq!(pi_units(&pi_session(), &entry).unwrap(), expected);
        }
        let mut entry = pi_tool_result("pi-t1", "pi-a1", false, 7777);
        entry["message"].as_object_mut().unwrap().remove("isError");
        if let Some(status) = status {
            entry["message"]["isError"] = status;
        }
        assert_eq!(
            pi_units(&pi_session(), &entry),
            Err(AdapterRefusal::UnsupportedShape("message.isError")),
            "{entry:?}"
        );
    }
}

#[test]
fn opencode_refuses_pi_namespace() {
    let session = SessionIdentity {
        harness: Harness::Pi,
        ..opencode_session()
    };
    assert_eq!(
        opencode_units(&session, &opencode_user("msg_u", "hello", 100)),
        Err(AdapterRefusal::UnsupportedShape("harness"))
    );
}

#[test]
fn pi_refuses_opencode_namespace() {
    let session = SessionIdentity {
        harness: Harness::OpenCode,
        ..pi_session()
    };
    assert_eq!(
        pi_units(&session, &pi_user("pi-u1", "hello", 100)),
        Err(AdapterRefusal::UnsupportedShape("harness"))
    );
}

#[test]
fn opencode_requires_parts_array() {
    let mut record = opencode_user("msg_u", "hello", 100);
    for parts in [
        None,
        Some(Value::Null),
        Some(json!({})),
        Some(json!(42)),
        Some(json!("text")),
    ] {
        record.as_object_mut().unwrap().remove("parts");
        if let Some(parts) = parts {
            record["parts"] = parts;
        }
        assert_eq!(
            opencode_units(&opencode_session(), &record),
            Err(AdapterRefusal::UnsupportedShape("parts")),
            "{record:?}"
        );
    }
    for parts in [
        json!([]),
        json!([{ "type": "reasoning", "text": "thinking" }]),
    ] {
        record["parts"] = parts;
        assert_eq!(
            opencode_units(&opencode_session(), &record).unwrap(),
            vec![]
        );
    }
}

#[test]
fn unfinished_assistant_text_is_deferred_but_settled_tools_publish() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let publisher = corpus.publisher();
    let mut record = opencode_assistant("msg_stream", 5000);
    record["info"]["time"]["completed"] = Value::Null;
    for text in ["H", "Hello"] {
        record["parts"][2]["text"] = json!(text);
        let units = opencode_units(&opencode_session(), &record).unwrap();
        assert_eq!(units.len(), 2, "unfinished text must not become a source");
        for unit in &units {
            assert_eq!(unit.class, OccurrenceClass::RawToolSpans);
            publisher.publish(unit, 6000).unwrap();
        }
        record["info"]["time"]
            .as_object_mut()
            .unwrap()
            .remove("completed");
    }
    record["info"]["error"] = json!({ "name": "MessageAbortedError" });
    assert!(
        opencode_units(&opencode_session(), &record)
            .unwrap()
            .iter()
            .all(|unit| unit.class == OccurrenceClass::RawToolSpans),
        "an abort without a completion timestamp provides no stable text revision"
    );
    record["info"]["time"]["completed"] = json!(5000);
    for unit in opencode_units(&opencode_session(), &record).unwrap() {
        publisher.publish(&unit, 6000).unwrap();
    }
    let messages: BTreeSet<_> = inventory(&corpus)
        .into_iter()
        .filter(|(class, _, _)| class == "messages")
        .map(|(_, text, _)| text)
        .collect();
    assert_eq!(
        messages,
        BTreeSet::from(["Hello".to_owned(), String::new()])
    );
    let user = opencode_units(
        &opencode_session(),
        &opencode_user("msg_user", "user text", 100),
    )
    .unwrap();
    assert_eq!(user.len(), 1);
    assert_eq!(user[0].revision, "100");
    publisher.publish(&user[0], 6000).unwrap();
}

/// The request digest is persisted in the descriptor receipt under a key that does not change with it, so a stored receipt replays only while the digest's byte layout is the one that wrote it. The pinned `source-request.v2` values fail this test whenever the layout moves: a message unit, whose record is absent, a Pi tool unit, and an OpenCode tool unit whose record carries truncation and an exit code; the digest binds each record's bytes. The frozen v1 value is pinned in `receipts_written_before_the_outcome_record_replay_under_v2`.
#[test]
fn publication_receipt_digest_layout_is_pinned() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let publisher = corpus.publisher();
    let message = pi_units(&pi_session(), &pi_user("pi-u1", "hello", 100))
        .unwrap()
        .remove(0);
    let tool = pi_units(&pi_session(), &pi_tool_result("pi-t1", "pi-a1", false, 200))
        .unwrap()
        .remove(0);
    let conn =
        Connection::open_with_flags(&corpus.kernel_db, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    // An OpenCode record carrying truncation and an exit code.
    let trace: Value =
        serde_json::from_str(include_str!("../testdata/opencode-tool-outcome-trace.json")).unwrap();
    let exit_1 = opencode_units(
        &opencode_session(),
        &opencode_record(&trace["cases"][0]["info"], &trace["cases"][0]["part"]),
    )
    .unwrap()
    .remove(0);
    let mut digests = Vec::new();
    for unit in [&message, &tool, &exit_1] {
        // The trace's timestamps are real OpenCode clock values; the observation follows them.
        publisher.publish(unit, 1_800_000_000_000).unwrap();
        let (key, _) = publisher.receipt_identity(unit).unwrap();
        let digest: String = conn
            .query_row(
                "SELECT request_digest FROM operation_receipts
                 WHERE producer='eidnara-daemon/harness-sources' AND operation_key=?1",
                [format!("source-descriptor:{key}")],
                |row| row.get(0),
            )
            .unwrap();
        digests.push(digest);
    }
    assert_eq!(
        digests,
        [
            "c7d32963a0dadfd9838219b46efd5f79e7f7abd6403242dddb0ff75372716c85",
            "cfc4bcd532cd5d2151054bcb6dac960de1eeeae9b5236131a0730259b42062e9",
            "bd80d105aa3b57d56c98af10dfb3a8b930f8e59c336d2505311ebcda2982a190"
        ]
    );
}

/// A unit's receipt identity is the key and request digest publication records: the key digests the occurrence id, revision, and representation joined by U+001F, and the request digest frames the format tag, text, domain, scope, role, and sensitivity, each after its big-endian `u64` length, then the scope and egress flags, then the outcome record's presence byte and, when present, its length-prefixed encoding. The texts span lengths from empty to past several SHA-256 blocks, a publication stores exactly that key and digest, and a revision outside its canonical spelling has no receipt identity.
#[test]
fn receipt_identity_keys_the_occurrence_and_frames_the_request() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    let mut units: Vec<SourceUnit> = [0, 1, 7, 8, 55, 56, 63, 64, 65, 119, 120, 1000]
        .into_iter()
        .map(|len| pi_text_unit("pi-u1", &("é".repeat(len / 2) + &"a".repeat(len % 2))))
        .collect();
    units.extend(pi_units(&pi_session(), &pi_tool_result("pi-t1", "pi-a1", true, 300)).unwrap());
    units.extend(pi_units(&pi_session(), &pi_tool_result("pi-t2", "pi-a1", false, 301)).unwrap());
    units.extend(opencode_units(&opencode_session(), &opencode_assistant("msg_a", 5000)).unwrap());
    for scope_id in [None, Some(SCOPE)] {
        for egress in [ProviderEgress::LocalOnly, ProviderEgress::RemoteAllowed] {
            let publisher = SourcePublisher {
                scope_id,
                egress,
                ..corpus.publisher()
            };
            for unit in &units {
                let encoded = encode_preserving_span(&Occurrence {
                    class: unit.class.code(),
                    identity: &identity_of(unit),
                    revision: &unit.revision,
                    representation: unit.representation.as_str(),
                    span: None,
                })
                .unwrap();
                let key = format!(
                    "{}\u{1f}{}\u{1f}{}",
                    encoded.occurrence_id,
                    encoded.revision,
                    unit.representation.as_str()
                );
                let mut request = Vec::new();
                for part in [
                    "source-request.v2",
                    unit.text.as_str(),
                    publisher.domain_id,
                    scope_id.unwrap_or_default(),
                    unit.role.as_str(),
                    publisher.sensitivity.as_str(),
                ] {
                    request.extend_from_slice(&(part.len() as u64).to_be_bytes());
                    request.extend_from_slice(part.as_bytes());
                }
                request.push(u8::from(scope_id.is_some()));
                request.push(match egress {
                    ProviderEgress::RemoteAllowed => 0,
                    ProviderEgress::LocalOnly => 1,
                });
                match &unit.outcome {
                    Some(outcome) => {
                        let encoded = outcome.encode();
                        request.push(1);
                        request.extend_from_slice(&(encoded.len() as u64).to_be_bytes());
                        request.extend_from_slice(&encoded);
                    }
                    None => request.push(0),
                }
                assert_eq!(
                    publisher.receipt_identity(unit),
                    Some((identity_digest(key.as_bytes()), identity_digest(&request))),
                    "{unit:?}"
                );
            }
        }
    }
    corpus.seed();
    let (key, digest) = corpus.publisher().receipt_identity(&units[2]).unwrap();
    corpus.publisher().publish(&units[2], NOW).unwrap();
    let conn =
        Connection::open_with_flags(&corpus.kernel_db, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let stored: Vec<(String, String)> = conn
        .prepare(
            "SELECT operation_key, request_digest FROM operation_receipts
             WHERE producer='eidnara-daemon/harness-sources'
               AND operation_key LIKE 'source-descriptor:%'",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(stored, [(format!("source-descriptor:{key}"), digest)]);
    for revision in ["01", "+5", "9223372036854775808"] {
        let unit = SourceUnit {
            revision: revision.to_owned(),
            ..units[2].clone()
        };
        assert_eq!(
            corpus.publisher().receipt_identity(&unit),
            None,
            "{revision}"
        );
        let refused = corpus.publisher().publish(&unit, NOW);
        assert!(
            matches!(
                refused,
                Err(PublishError::Descriptor { evidence: None, .. })
            ),
            "{revision}: {refused:?}"
        );
    }
}

/// With several fields wrong at once, the Pi adapter names the first one it checks: the role before the timestamp, and the call before the error flag.
#[test]
fn pi_refusals_name_the_first_failing_field() {
    let mut user = pi_user("pi-u1", "hello", 100);
    let message = user["message"].as_object_mut().unwrap();
    message.remove("role");
    message.remove("timestamp");
    assert_eq!(
        pi_units(&pi_session(), &user),
        Err(AdapterRefusal::MissingIdentity("role"))
    );
    let mut result = pi_tool_result("pi-t1", "pi-a1", false, 300);
    let message = result["message"].as_object_mut().unwrap();
    message.remove("toolCallId");
    message.insert("isError".to_owned(), json!("false"));
    message.insert("content".to_owned(), json!("not an array"));
    assert_eq!(
        pi_units(&pi_session(), &result),
        Err(AdapterRefusal::MissingIdentity("toolCallId"))
    );
    let mut result = pi_tool_result("pi-t1", "pi-a1", false, 300);
    result.as_object_mut().unwrap().remove("parentId");
    result["message"]["timestamp"] = json!(-1);
    assert_eq!(
        pi_units(&pi_session(), &result),
        Err(AdapterRefusal::MalformedRevision("timestamp"))
    );
}

#[test]
fn publication_receipt_conflicts_on_provider_egress_change() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let unit = pi_units(&pi_session(), &pi_user("pi-u1", "hello", 100))
        .unwrap()
        .remove(0);
    let remote = SourcePublisher {
        egress: ProviderEgress::RemoteAllowed,
        ..corpus.publisher()
    };
    remote.publish(&unit, NOW).unwrap();
    let tip = corpus.tip();
    let staged = corpus.kernel.staged_artifacts_for_test();
    let result = corpus.publisher().publish(&unit, NOW);
    assert!(
        matches!(result, Err(PublishError::IdentityReused)),
        "{result:?}"
    );
    assert_eq!(corpus.tip(), tip);
    assert_eq!(corpus.kernel.staged_artifacts_for_test(), staged);
}

#[test]
fn publication_receipt_distinguishes_delimiters_inside_fields() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let mut unit = pi_units(&pi_session(), &pi_user("pi-u1", "a\u{1f}b", 100))
        .unwrap()
        .remove(0);
    corpus.publisher().publish(&unit, NOW).unwrap();
    let tip = corpus.tip();
    let staged = corpus.kernel.staged_artifacts_for_test();
    unit.text = "a".to_owned();
    let other = SourcePublisher {
        domain_id: "b\u{1f}domain",
        ..corpus.publisher()
    };
    let result = other.publish(&unit, NOW);
    assert!(
        matches!(result, Err(PublishError::IdentityReused)),
        "{result:?}"
    );
    assert_eq!(corpus.tip(), tip);
    assert_eq!(corpus.kernel.staged_artifacts_for_test(), staged);
}

#[test]
fn publication_store_failure_preserves_evidence_for_retry() {
    for table in ["observations", "admission_decisions"] {
        let dir = tempfile::tempdir().unwrap();
        let corpus = Corpus::open(dir.path());
        corpus.seed();
        let unit = pi_units(&pi_session(), &pi_user("pi-u1", "retry text", 100))
            .unwrap()
            .remove(0);
        let connection = Connection::open(dir.path().join("kernel/kernel.sqlite")).unwrap();
        connection
            .execute_batch(&format!(
                "CREATE TRIGGER publication_failure BEFORE INSERT ON {table}
             BEGIN SELECT injected_store_failure(); END;"
            ))
            .unwrap();
        let failed = corpus.publisher().publish(&unit, NOW).unwrap_err();
        let evidence = match failed {
            PublishError::Kernel {
                error: kernel::KernelError::Io,
                evidence: Some(evidence),
            }
            | PublishError::Descriptor {
                refusal: SourceDescriptorError::Kernel(kernel::KernelError::Io),
                evidence: Some(evidence),
            } => evidence,
            other => panic!("expected store Io after retention, got {other:?}"),
        };
        assert_eq!(
            evidence.retired, None,
            "store failure must not attempt compensation"
        );
        connection
            .execute_batch("DROP TRIGGER publication_failure")
            .unwrap();
        let (_, states) = corpus
            .kernel
            .object_states(std::slice::from_ref(&evidence.object_id))
            .unwrap();
        assert_eq!(
            states[0].as_ref().unwrap().object.invalidated_commit_seq,
            None,
            "store failure must leave retained evidence live for retry ({table})"
        );
        let published = corpus.publisher().publish(&unit, NOW).unwrap();
        assert_eq!(published.evidence_object_id, evidence.object_id);
        assert!(!published.replayed);
        assert!(corpus.publisher().publish(&unit, NOW).unwrap().replayed);
        assert_eq!(inventory(&corpus).len(), 1);
    }
}

#[test]
fn publication_refuses_another_projects_scope_before_retention() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let session = SessionIdentity {
        project_id: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_owned(),
        ..pi_session()
    };
    let mut units = pi_units(&session, &pi_user("pi-u1", "wrong project", 100)).unwrap();
    units.extend(pi_units(&session, &pi_tool_result("pi-t1", "pi-a1", false, 200)).unwrap());
    let tip = corpus.tip();
    let staged = corpus.kernel.staged_artifacts_for_test();
    for unit in units {
        let result = corpus.publisher().publish(&unit, NOW);
        assert!(
            matches!(
                result,
                Err(PublishError::Kernel {
                    error: kernel::KernelError::NotFound,
                    evidence: None,
                })
            ),
            "wrong-project publication must fail before retention: {result:?}"
        );
        assert_eq!(
            corpus.tip(),
            tip,
            "no artifact or descriptor receipt commits"
        );
        assert_eq!(corpus.kernel.staged_artifacts_for_test(), staged);
        assert!(inventory(&corpus).is_empty());
    }
}

#[test]
fn publication_refuses_a_project_unit_without_a_scope_before_retention() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let unscoped = SourcePublisher {
        scope_id: None,
        ..corpus.publisher()
    };
    let mut units = pi_units(&pi_session(), &pi_user("pi-u1", "no scope", 100)).unwrap();
    units.extend(pi_units(&pi_session(), &pi_tool_result("pi-t1", "pi-a1", false, 200)).unwrap());
    let tip = corpus.tip();
    for unit in units {
        let result = unscoped.publish(&unit, NOW);
        assert!(
            matches!(
                result,
                Err(PublishError::Kernel {
                    error: kernel::KernelError::InvalidInput,
                    evidence: None,
                })
            ),
            "a project-bound unit has no route without a scope: {result:?}"
        );
        assert_eq!(corpus.tip(), tip, "nothing committed");
        assert!(inventory(&corpus).is_empty());
    }
}

#[test]
fn publication_rechecks_project_scope_after_retention() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let unit = pi_units(&pi_session(), &pi_user("pi-u1", "scope changes", 100))
        .unwrap()
        .remove(0);
    let connection = Connection::open(dir.path().join("kernel/kernel.sqlite")).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER change_scope AFTER INSERT ON evidence_meta
         BEGIN UPDATE scope_term
           SET exact_value='bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'
           WHERE scope_id='project:a' AND dimension='project'; END;",
        )
        .unwrap();
    let result = corpus.publisher().publish(&unit, NOW);
    let Err(PublishError::Kernel {
        error: kernel::KernelError::NotFound,
        evidence: Some(evidence),
    }) = result
    else {
        panic!("scope must be rechecked in the descriptor commit: {result:?}");
    };
    assert_eq!(evidence.retired, None);
    let (_, states) = corpus
        .kernel
        .object_states(std::slice::from_ref(&evidence.object_id))
        .unwrap();
    assert_eq!(
        states[0].as_ref().unwrap().object.invalidated_commit_seq,
        None
    );
    assert!(inventory(&corpus).is_empty());
    connection
        .execute_batch("DROP TRIGGER change_scope")
        .unwrap();
}

#[test]
fn publication_missing_scope_preserves_evidence_until_scope_is_created() {
    const MISSING_SCOPE: &str = "project:recoverable";
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let publisher = SourcePublisher {
        scope_id: Some(MISSING_SCOPE),
        ..corpus.publisher()
    };
    let unit = pi_units(&pi_session(), &pi_user("pi-u1", "recoverable scope", 100))
        .unwrap()
        .remove(0);
    let tip = corpus.tip();
    let failed = publisher.publish(&unit, NOW).unwrap_err();
    let PublishError::Kernel {
        error: kernel::KernelError::NotFound,
        evidence: Some(evidence),
    } = failed
    else {
        panic!("expected missing scope after retention, got {failed:?}");
    };
    assert_eq!(
        evidence.retired, None,
        "missing scope must not attempt compensation"
    );
    assert_eq!(corpus.tip(), tip + 1, "only artifact retention commits");
    let (_, states) = corpus
        .kernel
        .object_states(std::slice::from_ref(&evidence.object_id))
        .unwrap();
    assert_eq!(
        states[0].as_ref().unwrap().object.invalidated_commit_seq,
        None
    );
    assert!(inventory(&corpus).is_empty());

    corpus
        .kernel
        .commit(intent("create-requested-scope"), |envelope| {
            envelope.insert_scope(ScopeSpec {
                scope_id: MISSING_SCOPE.to_owned(),
                object_id: MISSING_SCOPE.to_owned(),
                source_id: MISSING_SCOPE.to_owned(),
                domain_id: "domain".to_owned(),
                source_kind: "kernel_route".to_owned(),
                source_revision: 1,
                sensitivity: Sensitivity::Normal,
                terms: vec![ScopeTermSpec {
                    dimension: Dimension::Project.as_str().to_owned(),
                    operator: "exact".to_owned(),
                    exact_value: Some(PROJECT.to_owned()),
                    ..ScopeTermSpec::default()
                }],
            })?;
            Ok(String::new())
        })
        .unwrap();
    let tip = corpus.tip();
    let published = publisher.publish(&unit, NOW).unwrap();
    assert_eq!(published.evidence_object_id, evidence.object_id);
    assert!(!published.replayed);
    assert_eq!(corpus.tip(), tip + 1, "retry reuses the artifact receipt");
    let rows = corpus.export();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].object_id, published.object_id);
    assert_eq!(rows[0].text.as_deref(), Some("recoverable scope"));
    let tip = corpus.tip();
    assert!(publisher.publish(&unit, NOW).unwrap().replayed);
    assert_eq!(corpus.tip(), tip);
}

#[test]
fn permanent_refusal_reports_failed_retirement_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let publisher = corpus.publisher();
    let mut unit = pi_units(&pi_session(), &pi_user("pi-u1", "retirement failure", 100))
        .unwrap()
        .remove(0);
    publisher.publish(&unit, NOW).unwrap();
    unit.revision = "50".to_owned();
    let connection = Connection::open(dir.path().join("kernel/kernel.sqlite")).unwrap();
    connection.execute_batch(
        "CREATE TRIGGER retirement_failure BEFORE UPDATE OF invalidated_commit_seq ON evidence_meta
         BEGIN SELECT injected_retirement_failure(); END;"
    ).unwrap();
    let failed = publisher.publish(&unit, NOW).unwrap_err();
    let PublishError::Descriptor {
        refusal: SourceDescriptorError::RevisionNotAdvanced,
        evidence: Some(evidence),
    } = failed
    else {
        panic!("expected permanent refusal with compensation, got {failed:?}");
    };
    assert_eq!(evidence.retired, Some(Err(kernel::KernelError::Io)));
    let (_, states) = corpus
        .kernel
        .object_states(std::slice::from_ref(&evidence.object_id))
        .unwrap();
    assert_eq!(
        states[0].as_ref().unwrap().object.invalidated_commit_seq,
        None
    );
    connection
        .execute_batch("DROP TRIGGER retirement_failure")
        .unwrap();
}

/// AC1, AC3, AC5: equal text under different native identities is two occurrences; publishing a unit again replays its receipts; a newer revision invalidates its predecessor in the same commit; a stale revision is refused; reopen preserves exact bytes.
#[test]
fn equal_text_stays_distinct_and_revisions_replay_or_succeed_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let publisher = corpus.publisher();
    let first = opencode_units(
        &opencode_session(),
        &opencode_user("msg_1", "same words", 100),
    )
    .unwrap();
    let second = opencode_units(
        &opencode_session(),
        &opencode_user("msg_2", "same words", 200),
    )
    .unwrap();
    let pi = pi_units(&pi_session(), &pi_user("pi-1", "same words", 300)).unwrap();
    let a = publisher.publish(&first[0], NOW).unwrap();
    let b = publisher.publish(&second[0], NOW).unwrap();
    let c = publisher.publish(&pi[0], NOW).unwrap();
    assert!(!a.replayed && !b.replayed && !c.replayed);
    let distinct: BTreeSet<&str> = [
        a.occurrence_id.as_str(),
        b.occurrence_id.as_str(),
        c.occurrence_id.as_str(),
    ]
    .into_iter()
    .collect();
    assert_eq!(distinct.len(), 3, "equal bytes, three native occurrences");
    let tip = corpus.tip();

    // Replay: the same unit returns the stored receipts, names the same occurrence, commits nothing, and stages no bytes: the descriptor receipt is consulted before the artifact is offered to the store.
    let staged = corpus.kernel.staged_artifacts_for_test();
    let again = publisher.publish(&first[0], NOW + 1).unwrap();
    assert!(again.replayed);
    assert_eq!(
        (again.object_id.as_str(), again.occurrence_id.as_str()),
        (a.object_id.as_str(), a.occurrence_id.as_str())
    );
    assert_eq!(corpus.tip(), tip, "a replay is not a commit");
    assert_eq!(
        corpus.kernel.staged_artifacts_for_test(),
        staged,
        "a replay stages no artifact"
    );
    // The same identity and revision with other bytes is a conflict, not a replay and not a second retention.
    let mut changed = first[0].clone();
    changed.text = "different words".to_owned();
    assert!(matches!(
        publisher.publish(&changed, NOW + 1),
        Err(PublishError::IdentityReused)
    ));
    assert_eq!(corpus.tip(), tip);
    assert_eq!(corpus.kernel.staged_artifacts_for_test(), staged);
    // The same unit published into another scope is a conflict too: a receipt never moves a row.
    let elsewhere = SourcePublisher {
        scope_id: Some("project:b"),
        ..corpus.publisher()
    };
    assert!(matches!(
        elsewhere.publish(&first[0], NOW + 1),
        Err(PublishError::IdentityReused)
    ));
    assert_eq!(corpus.kernel.staged_artifacts_for_test(), staged);
    // A native timestamp far ahead of the observation is not a revision this publisher can order: refused before any byte is retained, so it cannot pin the lineage.
    let mut ahead = first[0].clone();
    ahead.revision = i64::MAX.to_string();
    assert!(matches!(
        publisher.publish(&ahead, NOW + 1),
        Err(PublishError::RevisionAhead {
            revision: i64::MAX,
            observed_at
        }) if observed_at == NOW + 1
    ));
    assert_eq!(corpus.tip(), tip);
    assert_eq!(corpus.kernel.staged_artifacts_for_test(), staged);

    // Succession: the message edited later carries a newer revision, invalidates the predecessor atomically, and a stale revision is refused.
    let edited = opencode_units(
        &opencode_session(),
        &opencode_user("msg_1", "same words, edited", 150),
    )
    .unwrap();
    let successor = publisher.publish(&edited[0], NOW + 2).unwrap();
    assert_eq!(
        successor.replaced_object_id.as_deref(),
        Some(a.object_id.as_str())
    );
    assert_eq!(
        corpus.tip(),
        tip + 2,
        "one artifact commit and one descriptor commit"
    );
    let stale = opencode_units(
        &opencode_session(),
        &opencode_user("msg_1", "older words", 120),
    )
    .unwrap();
    let refused = publisher.publish(&stale[0], NOW + 3).unwrap_err();
    let PublishError::Descriptor {
        refusal: SourceDescriptorError::RevisionNotAdvanced,
        evidence: Some(evidence),
    } = refused
    else {
        panic!("a stale revision is refused after its evidence was retained: {refused:?}");
    };
    // The refused block's evidence was retained and then retired in the compensating commit: the evidence object is invalidated, so it is not a live source and no descriptor names it.
    assert_eq!(evidence.retired, Some(Ok(())), "{evidence:?}");
    let (_, states) = corpus
        .kernel
        .object_states(std::slice::from_ref(&evidence.object_id))
        .unwrap();
    let state = states[0]
        .as_ref()
        .expect("the evidence object was registered");
    assert_eq!(state.object.object_kind, "evidence");
    assert!(
        state.object.invalidated_commit_seq.is_some(),
        "the refused block's evidence is still live: {state:?}"
    );
    assert!(
        !inventory(&corpus)
            .iter()
            .any(|(_, text, _)| text == "older words")
    );
    // Publishing the stale unit again replays its artifact receipt, which now names retired evidence, so the descriptor is refused for that reason and the replayed compensation reports the evidence retired.
    let again = publisher.publish(&stale[0], NOW + 3).unwrap_err();
    let PublishError::Descriptor {
        refusal: SourceDescriptorError::EvidenceMissing,
        evidence: Some(evidence_again),
    } = again
    else {
        panic!("a repeated stale publication is refused: {again:?}");
    };
    assert_eq!(evidence_again.object_id, evidence.object_id);
    assert_eq!(evidence_again.retired, Some(Ok(())));

    // Tool bytes: exact through publication, export, and reopen.
    let tool = opencode_units(&opencode_session(), &opencode_assistant("msg_a", 5000)).unwrap();
    for unit in &tool {
        publisher.publish(unit, NOW + 4).unwrap();
    }
    let before = inventory(&corpus);
    assert!(
        before
            .iter()
            .any(|(class, text, _)| class == "raw_tool_spans" && text == TOOL_BYTES),
        "the tool bytes are exact"
    );
    assert!(
        before
            .iter()
            .any(|(class, text, _)| class == "raw_tool_spans" && text == "boom\n")
    );
    assert!(
        before
            .iter()
            .any(|(class, text, _)| class == "messages" && text.is_empty()),
        "an empty text block is a retained occurrence"
    );
    drop(corpus);
    let reopened = Corpus::open(dir.path());
    assert_eq!(
        inventory(&reopened),
        before,
        "reopen preserves every byte and identity"
    );
    let texts: Vec<&str> = before
        .iter()
        .filter(|(class, _, _)| class == "messages")
        .map(|(_, text, _)| text.as_str())
        .collect();
    assert!(texts.contains(&"same words, edited") && !texts.contains(&"older words"));
}

/// AC3, AC4: a tool string that carries a credential is refused before retention: no artifact, no descriptor, no commit, and no coverage; an oversized identity value is refused before any byte is retained.
#[test]
fn refused_bytes_and_identities_retain_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let publisher = corpus.publisher();
    let tip = corpus.tip();
    let mut leaked = opencode_assistant("msg_s", 5000);
    leaked["parts"][3]["state"]["output"] = json!(format!("token={SECRET}\n"));
    let units = opencode_units(&opencode_session(), &leaked).unwrap();
    let refused = publisher.publish(&units[1], NOW);
    assert!(
        matches!(
            refused,
            Err(PublishError::Artifact(
                ArtifactErrorKind::ExactBytesRewritten
            ))
        ),
        "{refused:?}"
    );
    assert_eq!(corpus.tip(), tip, "nothing committed");
    assert!(inventory(&corpus).is_empty());
    assert!(
        !format!("{refused:?} {:?}", units[1]).contains(SECRET),
        "neither the refusal nor the unit's debug output names content"
    );

    let mut oversized = pi_units(&pi_session(), &pi_user("pi-1", "fine text", 10)).unwrap();
    oversized[0].identity[2].1 = "s".repeat(600);
    let refused = publisher.publish(&oversized[0], NOW);
    assert!(
        matches!(
            refused,
            Err(PublishError::Descriptor {
                refusal: SourceDescriptorError::Occurrence(_),
                evidence: None,
            })
        ),
        "{refused:?}"
    );
    assert_eq!(corpus.tip(), tip, "refused before any byte was retained");

    // The other units of the same message publish on their own: a refusal is per block.
    let ok = publisher.publish(&units[0], NOW).unwrap();
    assert!(!ok.replayed);
    assert_eq!(inventory(&corpus).len(), 1);
}

fn eligibility(project: &ProjectScope) -> EligibilityBinding<'_> {
    EligibilityBinding {
        project,
        destination: ArtifactDestination::Local,
    }
}

/// AC5, AC6, AC7: a mixed session from both harnesses reaches the projection as lexical rows for every block, pending only for message text, and zero raw-derived admissions or inference calls through a real dispatch pass; a tool string smuggled through the message class is caught by the same pending counter.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mixed_sessions_create_pending_only_for_message_text() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let publisher = corpus.publisher();
    let mut expected_messages = BTreeSet::new();
    let mut expected_tools = BTreeSet::new();
    const USER: (SourceClass, TaintClass) = (SourceClass::ExplicitUser, TaintClass::UserExplicit);
    const ASSISTANT: (SourceClass, TaintClass) =
        (SourceClass::ModelInference, TaintClass::AssistantInference);
    const TOOL: (SourceClass, TaintClass) = (
        SourceClass::TrustedToolResult,
        TaintClass::ToolUntrustedOutput,
    );
    // Each fixture names the admission classes of its units in unit order, independently of the role the adapter read.
    let records = [
        (
            opencode_units(
                &opencode_session(),
                &opencode_user("msg_u", "please run the audit", 100),
            )
            .unwrap(),
            vec![USER],
        ),
        (
            opencode_units(&opencode_session(), &opencode_assistant("msg_a", 5000)).unwrap(),
            vec![ASSISTANT, TOOL, TOOL, ASSISTANT],
        ),
        (
            pi_units(&pi_session(), &pi_user("pi-u1", "run it", 5500)).unwrap(),
            vec![USER],
        ),
        (
            pi_units(&pi_session(), &pi_assistant("pi-a1", 6000)).unwrap(),
            vec![ASSISTANT],
        ),
        (
            pi_units(
                &pi_session(),
                &pi_tool_result("pi-t1", "pi-a1", false, 7777),
            )
            .unwrap(),
            vec![TOOL, TOOL],
        ),
    ];
    let mut empty_block = None;
    let mut evidence_objects = BTreeMap::new();
    let kernel_db = Connection::open_with_flags(
        dir.path().join("kernel/kernel.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    for (units, expected) in &records {
        assert_eq!(units.len(), expected.len(), "{units:?}");
        for (unit, expected_provenance) in units.iter().zip(expected) {
            let published = publisher.publish(unit, NOW).unwrap();
            assert_eq!(published.admission, *expected_provenance, "{unit:?}");
            let admissions: Vec<(String, String, String)> = kernel_db
                .prepare(
                    "SELECT a.source_class,a.taint_class,e.object_id
                 FROM admission_decisions a
                 JOIN observations o ON o.object_id=a.subject_object_id
                 JOIN evidence_meta e ON e.evidence_id=a.evidence_id
                 WHERE a.subject_object_id=?1 AND a.evidence_id=o.evidence_id",
                )
                .unwrap()
                .query_map([&published.object_id], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            assert_eq!(
                admissions,
                vec![(
                    expected_provenance.0.as_str().to_owned(),
                    expected_provenance.1.as_str().to_owned(),
                    published.evidence_object_id.clone(),
                )],
                "durable admission must bind the fixture's classes and evidence to {}",
                published.object_id
            );
            if unit.class == OccurrenceClass::Messages && unit.text.is_empty() {
                empty_block = Some(published.occurrence_id.clone());
            }
            evidence_objects.insert(
                published.occurrence_id.clone(),
                published.evidence_object_id.clone(),
            );
            match unit.class {
                OccurrenceClass::Messages => expected_messages.insert(published.occurrence_id),
                _ => expected_tools.insert(published.occurrence_id),
            };
        }
    }
    assert_eq!((expected_messages.len(), expected_tools.len()), (5, 4));
    let empty_block = empty_block.unwrap();

    let (projection, rows) = corpus.bootstrap(dir.path());
    // Every descriptor records the class the store recorded for its evidence: the store folds harness text asserted `Normal` up to `Sensitive`, and the descriptor is read from the store, not re-derived.
    for row in &rows {
        let evidence_object_id = &evidence_objects[&row.detail.occurrence_id];
        let (_, states) = corpus
            .kernel
            .object_states(std::slice::from_ref(evidence_object_id))
            .unwrap();
        let evidence = states[0].as_ref().unwrap();
        assert_eq!(
            (row.sensitivity, evidence.object.sensitivity),
            (Sensitivity::Sensitive, Sensitivity::Sensitive),
            "{}",
            row.detail.occurrence_id
        );
    }
    let conn = Connection::open_with_flags(
        dir.path().join("search").join("search.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let lexical: i64 = conn
        .query_row("SELECT COUNT(*) FROM occurrences", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        lexical as usize,
        expected_messages.len() + expected_tools.len(),
        "every block is a lexical row"
    );
    let pending: BTreeSet<String> = conn
        .prepare("SELECT occurrence_id FROM embedding_jobs WHERE state='pending'")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        pending, expected_messages,
        "pending exists for message text and for nothing else"
    );
    // Independently of the adapters: the pending rows are exactly these native blocks, named by the fixture.
    let mut pending_blocks: Vec<(String, String)> = rows
        .iter()
        .filter(|row| pending.contains(&row.detail.occurrence_id))
        .map(|row| {
            let field = |name: &str| {
                row.detail
                    .identity
                    .iter()
                    .find(|(k, _)| k == name)
                    .unwrap()
                    .1
                    .clone()
            };
            (field("message_id"), field("block_index"))
        })
        .collect();
    pending_blocks.sort();
    assert_eq!(
        pending_blocks,
        vec![
            ("msg_a".to_string(), "2".to_string()),
            ("msg_a".to_string(), "5".to_string()),
            ("msg_u".to_string(), "0".to_string()),
            ("pi-a1".to_string(), "1".to_string()),
            ("pi-u1".to_string(), "0".to_string()),
        ]
    );
    for row in &rows {
        if row.detail.class == "raw_tool_spans" {
            assert!(
                row.text.as_deref() == Some(TOOL_BYTES)
                    || row.text.as_deref() == Some("boom\n")
                    || row.text.as_deref() == Some(""),
                "{:?}",
                row.text
            );
        }
    }

    // A real pass: one inference per message block, none for tool rows, and the raw rows stay lexical.
    let engine = TestEngine::new();
    let local_embeddings = component(&engine, LocalEmbeddingsLimits::default());
    let project = ProjectScope::new(PROJECT).unwrap();
    let mut events = Vec::new();
    let mut dispatcher = EmbeddingDispatcher::new(&corpus.kernel, &projection, &local_embeddings);
    let end = tokio::task::block_in_place(|| {
        dispatcher
            .run_pass(
                eligibility(&project),
                &bounds(),
                &budget(Duration::from_secs(30)),
                NOW,
                &mut |event| events.push(event),
            )
            .unwrap()
    });
    assert_eq!(end, None);
    let admitted: BTreeSet<String> = events
        .iter()
        .filter_map(|event| match event {
            DispatchEvent::Admitted { job_id, .. } => Some(job_id.clone()),
            _ => None,
        })
        .collect();
    let job_occurrences: BTreeSet<String> = admitted
        .iter()
        .map(|job_id| {
            conn.query_row(
                "SELECT occurrence_id FROM embedding_jobs WHERE job_id=?1",
                [job_id],
                |row| row.get(0),
            )
            .unwrap()
        })
        .collect();
    // The empty text block is lexical only: preflight refuses it with a non-content reason and no inference.
    let mut embeddable = expected_messages.clone();
    embeddable.remove(&empty_block);
    assert_eq!(
        job_occurrences, embeddable,
        "admissions are message blocks only"
    );
    assert_eq!(
        engine.calls(),
        embeddable.len(),
        "one inference per non-empty message block"
    );
    let empty_job: (String, Option<String>) = conn
        .query_row(
            "SELECT state,stop_reason FROM embedding_jobs WHERE occurrence_id=?1",
            [&empty_block],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        empty_job,
        ("failed".to_string(), Some("input_empty".to_string()))
    );
    let raw_jobs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM embedding_jobs j JOIN occurrences o ON o.occurrence_id=j.occurrence_id WHERE o.class='raw_tool_spans'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(raw_jobs, 0);

    drop(dispatcher);
    drop(projection);
    let reopened = SearchProjection::open(dir.path()).unwrap();
    reopened
        .read(|conn| {
            for row in &rows {
                let completion = retrieval::vectors::completion_status(
                    conn,
                    &row.detail.occurrence_id,
                    GENERATION,
                )?;
                if embeddable.contains(&row.detail.occurrence_id) {
                    assert_eq!(
                        completion.job_state.as_deref(),
                        Some("embedded"),
                        "{}",
                        row.object_id
                    );
                    assert!(
                        completion.has_durable_vector(&TestEngine::vector_for(
                            row.text.as_deref().unwrap()
                        )),
                        "{} must retain its own vector",
                        row.object_id
                    );
                } else {
                    assert!(
                        completion.vector.is_none(),
                        "{} is lexical only",
                        row.object_id
                    );
                }
            }
            Ok(())
        })
        .unwrap();
    drop(reopened);

    // Negative control: a tool string leaked through the message class would be counted by the same pending oracle.
    let mut smuggled = opencode_units(&opencode_session(), &opencode_assistant("msg_leak", 9000))
        .unwrap()
        .remove(1);
    smuggled.class = OccurrenceClass::Messages;
    smuggled.representation = Representation::Text;
    smuggled.outcome = None;
    smuggled.identity = vec![
        ("project_id", PROJECT.to_string()),
        ("harness", "opencode".to_string()),
        ("session_id", SESSION.to_string()),
        ("message_id", "msg_leak".to_string()),
        ("block_index", "3".to_string()),
    ];
    let leaked = publisher.publish(&smuggled, NOW).unwrap();
    let dir2 = tempfile::tempdir().unwrap();
    let (_projection, _) = corpus.bootstrap(dir2.path());
    let conn2 = Connection::open_with_flags(
        dir2.path().join("search").join("search.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let pending_now: BTreeSet<String> = conn2
        .prepare("SELECT occurrence_id FROM embedding_jobs WHERE state='pending'")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(
        pending_now.contains(&leaked.occurrence_id),
        "the counter sees the leak"
    );
    assert_eq!(pending_now.len(), expected_messages.len() + 1);
}

#[path = "../src/compression_fidelity_corpus.rs"]
#[allow(dead_code)]
mod compression_fidelity_corpus;
#[path = "../src/compression_fidelity_observation.rs"]
#[allow(dead_code)]
mod compression_fidelity_observation;

const C6_OWNER: &str = "daemon.harness_sources.c6_exact_read";

fn c6_source() -> &'static compression_fidelity_corpus::Source {
    let case = compression_fidelity_corpus::corpus().case("C6").unwrap();
    case.source("C6.V1").unwrap()
}

fn c6_record(
    records: &'static [compression_fidelity_corpus::NativeMessage],
    id: &str,
) -> &'static compression_fidelity_corpus::NativeMessage {
    records.iter().find(|record| record.info.id == id).unwrap()
}

/// The corpus's raw C6 records, as the OpenCode source adapter reads them.
fn c6_records(field: &str) -> Vec<Value> {
    let corpus: Value = serde_json::from_slice(compression_fidelity_corpus::CORPUS_BYTES).unwrap();
    corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "C6")
        .and_then(|case| case["sources"].as_array())
        .and_then(|sources| sources.iter().find(|source| source["id"] == "C6.V1"))
        .and_then(|source| source[field].as_array())
        .unwrap()
        .clone()
}

/// A message occurrence's complete identity in tuple order.
fn c6_target<'a>(session: &'a str, message_id: &'a str, block: &'a str) -> [(&'a str, &'a str); 5] {
    [
        ("project_id", PROJECT),
        ("harness", "opencode"),
        ("session_id", session),
        ("message_id", message_id),
        ("block_index", block),
    ]
}

/// Every live message descriptor at `at`, paged within a fixed budget of pages.
fn live_messages_at(kernel: &KernelStore, at: i64) -> Vec<kernel::LiveDescriptor> {
    const MAX_PAGES: usize = 8;
    let mut rows = Vec::new();
    let mut after: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let page = kernel
            .live_source_descriptors(
                OccurrenceClass::Messages,
                at,
                after.as_deref(),
                NonZeroUsize::new(4).unwrap(),
                &budget(Duration::from_secs(5)),
            )
            .unwrap();
        rows.extend(page.rows);
        match page.next {
            Some(next) => after = Some(next),
            None => return rows,
        }
    }
    panic!("the live descriptor set exceeds {MAX_PAGES} pages");
}

/// The one descriptor whose complete identity, revision, representation, and span match the
/// independent target, never the first row and never a digest match alone.
fn select<'d>(
    rows: &'d [kernel::LiveDescriptor],
    target: &[(&str, &str)],
    revision: &str,
) -> Option<&'d kernel::LiveDescriptor> {
    let matches: Vec<_> = rows
        .iter()
        .filter(|row| {
            let identity: Vec<(&str, &str)> = row
                .detail
                .identity
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect();
            identity == target
                && row.detail.revision == revision
                && row.detail.representation == "text"
                && row.detail.span.is_none()
        })
        .collect();
    assert!(matches.len() <= 1, "two descriptors claim one occurrence");
    matches.into_iter().next()
}

fn read(kernel: &KernelStore, row: &kernel::LiveDescriptor) -> Result<Vec<u8>, ArtifactErrorKind> {
    kernel
        .read_artifact(&kernel::ArtifactHandle {
            digest: row.detail.artifact_digest.clone(),
            evidence_id: row.detail.evidence_id.clone(),
        })
        .map_err(|error| error.kind())
}

#[test]
fn c6_original_revision_bytes_read_exactly_after_succession_and_reopen() {
    use compression_fidelity_corpus::block_text;
    use compression_fidelity_observation::{Observation, Terminal};
    let source = c6_source();
    let original_record = c6_record(&source.messages, "msg_cf_c6_02");
    let successor_record = c6_record(&source.successors, "msg_cf_c6_02");
    let pasted_record = c6_record(&source.messages, "msg_cf_c6_03");
    let original_text = block_text(&original_record.parts[1]).unwrap();
    let successor_text = block_text(&successor_record.parts[1]).unwrap();
    let original_revision = original_record.info.time.completed.unwrap().to_string();
    let successor_revision = successor_record.info.time.completed.unwrap().to_string();
    let pasted_revision = pasted_record.info.time.created.to_string();
    let session = SessionIdentity {
        project_id: PROJECT.to_string(),
        harness: Harness::OpenCode,
        session_id: source.session_id.clone(),
    };
    let target = c6_target(&session.session_id, "msg_cf_c6_02", "1");
    let observed_at = successor_record.info.time.completed.unwrap() as i64 + DAY_MS;

    let dir = tempfile::tempdir().unwrap();
    let fixture = Corpus::open(dir.path());
    fixture.seed();
    let publisher = fixture.publisher();
    for record in c6_records("messages") {
        for unit in opencode_units(&session, &record).unwrap() {
            publisher.publish(&unit, observed_at).unwrap();
        }
    }
    let mut empty = opencode_user("msg_cf_c6_empty", "", 1_758_700_000_000);
    empty["info"]["sessionID"] = json!(session.session_id);
    for unit in opencode_units(&session, &empty).unwrap() {
        publisher.publish(&unit, observed_at).unwrap();
    }
    let saved = fixture.tip();
    let original = select(
        &live_messages_at(&fixture.kernel, saved),
        &target,
        &original_revision,
    )
    .expect("the original is live before succession")
    .clone();

    let mut replaced = None;
    for record in c6_records("successors") {
        for unit in opencode_units(&session, &record).unwrap() {
            let published = publisher.publish(&unit, observed_at).unwrap();
            if unit.revision == successor_revision {
                replaced = published.replaced_object_id;
            }
        }
    }
    assert_eq!(replaced.as_deref(), Some(original.object_id.as_str()));
    assert_eq!(
        Arc::strong_count(&fixture.kernel),
        1,
        "no other handle keeps the first incarnation open"
    );
    drop(fixture);

    let reopened = Corpus::open(dir.path());
    let historical = live_messages_at(&reopened.kernel, saved);
    let selected = select(&historical, &target, &original_revision)
        .expect("historical selection finds the original after reopen");
    assert_eq!(selected.object_id, original.object_id);
    let bytes = read(&reopened.kernel, selected).expect("succession leaves the evidence readable");
    assert_eq!(bytes, original_text.as_bytes(), "exact original bytes");

    let current = live_messages_at(&reopened.kernel, reopened.tip());
    assert!(select(&current, &target, &original_revision).is_none());
    let succeeded = select(&current, &target, &successor_revision).unwrap();
    let successor_bytes = read(&reopened.kernel, succeeded).unwrap();
    assert_eq!(
        successor_bytes.len(),
        bytes.len(),
        "the successor keeps the length"
    );
    assert_ne!(successor_bytes, bytes, "the wrong revision has other bytes");
    assert_eq!(successor_bytes, successor_text.as_bytes());

    let other_occurrence = select(
        &historical,
        &c6_target(&session.session_id, "msg_cf_c6_03", "1"),
        &pasted_revision,
    )
    .expect("the pasted copy is its own occurrence");
    assert_eq!(
        other_occurrence.detail.artifact_digest, selected.detail.artifact_digest,
        "equal text shares a digest"
    );
    assert_ne!(
        other_occurrence.detail.occurrence_id,
        selected.detail.occurrence_id
    );
    assert_ne!(other_occurrence.object_id, selected.object_id);

    let normalized = original_text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert_ne!(
        normalized.as_bytes(),
        bytes.as_slice(),
        "a whitespace-normalized transcript of C6 differs from the bytes the reader returned"
    );

    let empty_row = select(
        &historical,
        &c6_target(&session.session_id, "msg_cf_c6_empty", "0"),
        "1758700000000",
    )
    .unwrap();
    assert_eq!(
        read(&reopened.kernel, empty_row),
        Ok(Vec::new()),
        "present and empty"
    );

    reopened
        .kernel
        .delete_artifact(kernel::ArtifactDeletionRequest {
            intent: intent("c6-delete-original"),
            identity: kernel::ArtifactDeletionIdentity::EvidenceId(
                selected.detail.evidence_id.clone(),
            ),
            kind: kernel::ArtifactDeletionKind::Delete,
            operator_id: None,
            target_locator: None,
            reason: None,
            deleted_at: observed_at,
        })
        .unwrap();
    assert_eq!(
        read(&reopened.kernel, selected),
        Err(ArtifactErrorKind::ReferenceUnavailable),
        "retired evidence is a refusal, not an empty block"
    );
    if let Some(row) = select(
        &live_messages_at(&reopened.kernel, saved),
        &target,
        &original_revision,
    ) {
        assert_eq!(
            read(&reopened.kernel, row),
            Err(ArtifactErrorKind::ReferenceUnavailable),
            "a historical descriptor whose evidence was deleted still refuses its read"
        );
    }

    Observation::new(
        C6_OWNER,
        compression_fidelity_corpus::CORPUS_SHA256,
        "C6",
        &source.id,
        "exact_read",
        Terminal::ReadExact,
    )
    .scenario("C6.S6")
    .with(json!({
        "selected_object_id": selected.object_id,
        "selected_at_commit_seq": saved,
        "revision": original_revision,
        "byte_length": bytes.len(),
        "sha256": format!("{:x}", Sha256::digest(&bytes)),
        "controls": {
            "wrong_revision": "same length, other bytes",
            "equal_text_other_occurrence": "same digest, other occurrence",
            "normalized_substitute": "bytes differ",
            "absent_vs_empty": "empty block reads present and empty",
            "deleted_evidence": "reference_unavailable",
        },
        "consumer_exact_recovery": "unavailable",
    }))
    .emit();
}

/// One Pi user text unit `id` whose text is `text`.
fn pi_text_unit(id: &str, text: &str) -> SourceUnit {
    pi_text_unit_at(id, text, 100)
}

fn pi_text_unit_at(id: &str, text: &str, stamp: u64) -> SourceUnit {
    pi_units(&pi_session(), &pi_user(id, text, stamp))
        .unwrap()
        .remove(0)
}

/// `(count, largest byte length)` of the evidence rows the kernel holds.
fn evidence_rows(corpus: &Corpus) -> (i64, i64) {
    Connection::open_with_flags(&corpus.kernel_db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
        .query_row(
            "SELECT COUNT(*), COALESCE(MAX(byte_length), 0) FROM evidence_meta",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
}

/// Operation keys of every receipt the harness-source publisher recorded.
fn publisher_receipts(corpus: &Corpus) -> BTreeSet<String> {
    let conn =
        Connection::open_with_flags(&corpus.kernel_db, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mut statement = conn
        .prepare(
            "SELECT operation_key FROM operation_receipts
             WHERE producer='eidnara-daemon/harness-sources'",
        )
        .unwrap();
    statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn published(disposition: &UnitDisposition) -> &daemon::harness_sources::Published {
    match disposition {
        UnitDisposition::Published(published) => published,
        other => panic!("expected a publication, got {other:?}"),
    }
}

/// AC8, first two clauses: a unit of exactly the cap publishes with artifact bytes equal to its text; a unit one byte over is refused whole with no evidence and no receipt while its siblings publish.
#[test]
fn capture_batch_refuses_an_oversized_unit_whole_and_publishes_its_siblings() {
    assert_eq!(MAX_SOURCE_UNIT_BYTES, 64 * 1024);
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let publisher = corpus.publisher();

    let at_cap = "a".repeat(MAX_SOURCE_UNIT_BYTES - 1) + "\n";
    let dispositions =
        publish_screened(&publisher, &[pi_text_unit("pi-cap", &at_cap)], NOW, None).unwrap();
    assert!(!published(&dispositions[0]).replayed);
    let rows = live_messages_at(&corpus.kernel, corpus.tip());
    assert_eq!(rows.len(), 1);
    assert_eq!(read(&corpus.kernel, &rows[0]).unwrap(), at_cap.as_bytes());
    assert_eq!(evidence_rows(&corpus), (1, MAX_SOURCE_UNIT_BYTES as i64));

    let receipts = publisher_receipts(&corpus);
    let over_cap = "b".repeat(MAX_SOURCE_UNIT_BYTES + 1);
    let units = [
        pi_text_unit("pi-a", "first sibling"),
        pi_text_unit("pi-big", &over_cap),
        pi_text_unit("pi-b", "second sibling"),
    ];
    let dispositions = publish_screened(&publisher, &units, NOW, None).unwrap();
    assert!(!published(&dispositions[0]).replayed);
    assert!(matches!(dispositions[1], UnitDisposition::UnitTooLarge));
    assert!(!published(&dispositions[2]).replayed);
    let texts: BTreeSet<String> = inventory(&corpus)
        .into_iter()
        .map(|(_, text, _)| text)
        .collect();
    assert!(texts.contains("first sibling") && texts.contains("second sibling"));
    assert_eq!(
        evidence_rows(&corpus),
        (3, MAX_SOURCE_UNIT_BYTES as i64),
        "the oversized unit retained no evidence"
    );
    let added: Vec<String> = publisher_receipts(&corpus)
        .difference(&receipts)
        .cloned()
        .collect();
    assert_eq!(
        added.len(),
        4,
        "a descriptor and an evidence receipt per sibling, none for the refused unit: {added:?}"
    );

    // The refusal is terminal for unchanged input and still retains nothing.
    let tip = corpus.tip();
    let again = publish_screened(&publisher, &units[1..2], NOW, None).unwrap();
    assert!(matches!(again[0], UnitDisposition::UnitTooLarge));
    assert_eq!(corpus.tip(), tip);
}

/// AC8, third clause: a 33-unit window publishes 32 and defers exactly one; a window whose text exceeds the aggregate bound defers the units beyond it.
#[test]
fn capture_batch_defers_units_beyond_its_bounds() {
    assert_eq!(MAX_SOURCE_BATCH_UNITS, 32);
    assert_eq!(MAX_SOURCE_BATCH_BYTES, 64 * 1024);
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let publisher = corpus.publisher();

    let units: Vec<SourceUnit> = (0..=MAX_SOURCE_BATCH_UNITS)
        .map(|index| pi_text_unit(&format!("pi-{index}"), &format!("unit {index}")))
        .collect();
    let dispositions = publish_screened(&publisher, &units, NOW, None).unwrap();
    let deferred: Vec<usize> = dispositions
        .iter()
        .enumerate()
        .filter(|(_, disposition)| matches!(disposition, UnitDisposition::Deferred))
        .map(|(index, _)| index)
        .collect();
    assert_eq!(deferred, vec![MAX_SOURCE_BATCH_UNITS]);
    assert_eq!(inventory(&corpus).len(), MAX_SOURCE_BATCH_UNITS);
    assert_eq!(
        evidence_rows(&corpus).0,
        MAX_SOURCE_BATCH_UNITS as i64,
        "the deferred unit retained nothing"
    );

    // The next pass replays the 32 and publishes the deferred unit.
    let dispositions = publish_screened(&publisher, &units[1..], NOW, None).unwrap();
    assert!(
        dispositions[..MAX_SOURCE_BATCH_UNITS - 1]
            .iter()
            .all(|disposition| published(disposition).replayed)
    );
    assert!(!published(&dispositions[MAX_SOURCE_BATCH_UNITS - 1]).replayed);
    assert_eq!(inventory(&corpus).len(), MAX_SOURCE_BATCH_UNITS + 1);

    let third = MAX_SOURCE_BATCH_BYTES / 3 + 1;
    let heavy: Vec<SourceUnit> = ["x", "y", "z"]
        .iter()
        .map(|fill| pi_text_unit(&format!("pi-heavy-{fill}"), &fill.repeat(third)))
        .chain([pi_text_unit("pi-light", "small after the bound")])
        .collect();
    let dispositions = publish_screened(&publisher, &heavy, NOW, None).unwrap();
    assert!(!published(&dispositions[0]).replayed);
    assert!(!published(&dispositions[1]).replayed);
    assert!(matches!(dispositions[2], UnitDisposition::Deferred));
    assert!(
        matches!(dispositions[3], UnitDisposition::Deferred),
        "admission keeps window order, so a unit after the bound waits too"
    );
}

/// Identity refusals are decided per unit before the transaction; a sibling still publishes.
#[test]
fn capture_batch_refuses_a_malformed_identity_without_failing_siblings() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let mut oversized_identity = pi_text_unit("pi-x", "refused identity");
    oversized_identity.identity[2].1 = "s".repeat(600);
    let units = [oversized_identity, pi_text_unit("pi-ok", "kept")];
    let dispositions = publish_screened(&corpus.publisher(), &units, NOW, None).unwrap();
    assert!(
        matches!(
            &dispositions[0],
            UnitDisposition::Refused(PublishError::Descriptor {
                refusal: SourceDescriptorError::Occurrence(_),
                evidence: None,
            })
        ),
        "{:?}",
        dispositions[0]
    );
    assert!(!published(&dispositions[1]).replayed);
    assert_eq!(evidence_rows(&corpus).0, 1);
}

/// A same-key digest conflict fails the attempt, commits nothing, and names the conflicting unit, so the caller marks it terminal and its siblings publish on their own.
#[test]
fn capture_batch_names_the_unit_whose_receipt_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let publisher = corpus.publisher();
    publisher
        .publish(&pi_text_unit("pi-edited", "original"), NOW)
        .unwrap();
    let tip = corpus.tip();
    let staged = corpus.kernel.staged_artifacts_for_test();

    let units = [
        pi_text_unit("pi-fresh", "fresh sibling"),
        pi_text_unit("pi-edited", "edited in place"),
    ];
    let refusal = publish_screened(&publisher, &units, NOW, None).unwrap_err();
    assert_eq!(refusal.error, kernel::KernelError::Conflict);
    assert_eq!(refusal.unit, Some(1));
    assert_eq!(refusal.conflicting_unit(), Some(1));
    assert_eq!(corpus.tip(), tip, "nothing committed");
    assert_eq!(corpus.kernel.staged_artifacts_for_test(), staged);

    let dispositions = publish_screened(&publisher, &units[..1], NOW, None).unwrap();
    assert!(!published(&dispositions[0]).replayed);
}

/// Refused and oversized units take no admission slot: 32 valid units around them all publish.
#[test]
fn capture_batch_refusals_take_no_admission_slot() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let mut units: Vec<SourceUnit> = (0..MAX_SOURCE_BATCH_UNITS)
        .map(|index| pi_text_unit(&format!("pi-{index}"), &format!("unit {index}")))
        .collect();
    let mut malformed = pi_text_unit("pi-malformed", "refused identity");
    malformed.identity[2].1 = "s".repeat(600);
    units.insert(
        10,
        pi_text_unit("pi-big", &"b".repeat(MAX_SOURCE_UNIT_BYTES + 1)),
    );
    units.insert(20, malformed);
    let dispositions = publish_screened(&corpus.publisher(), &units, NOW, None).unwrap();
    assert!(matches!(dispositions[10], UnitDisposition::UnitTooLarge));
    assert!(matches!(
        dispositions[20],
        UnitDisposition::Refused(PublishError::Descriptor { .. })
    ));
    let published = dispositions
        .iter()
        .filter(|disposition| matches!(disposition, UnitDisposition::Published(_)))
        .count();
    assert_eq!(published, MAX_SOURCE_BATCH_UNITS);
    assert!(
        !dispositions
            .iter()
            .any(|disposition| matches!(disposition, UnitDisposition::Deferred))
    );
}

/// A unit whose project the publisher's stored scope does not name is refused on its own before the transaction; its sibling publishes.
#[test]
fn capture_batch_refuses_a_foreign_project_without_failing_siblings() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let mut foreign = pi_text_unit("pi-foreign", "another project");
    foreign.identity[0].1 = "b".repeat(64);
    let units = [foreign, pi_text_unit("pi-home", "this project")];
    let dispositions = publish_screened(&corpus.publisher(), &units, NOW, None).unwrap();
    assert!(
        matches!(
            &dispositions[0],
            UnitDisposition::Refused(PublishError::Kernel {
                error: kernel::KernelError::NotFound,
                evidence: None,
            })
        ),
        "{:?}",
        dispositions[0]
    );
    assert!(!published(&dispositions[1]).replayed);
    assert_eq!(
        evidence_rows(&corpus).0,
        1,
        "the foreign unit retained nothing"
    );
}

#[test]
fn capture_batch_refuses_a_secret_unit_without_failing_siblings() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let staged = corpus.kernel.staged_artifacts_for_test();
    let units = [
        pi_text_unit("pi-secret", &format!("token {SECRET}")),
        pi_text_unit("pi-clean", "a clean sibling"),
    ];
    let dispositions = publish_screened(&corpus.publisher(), &units, NOW, None).unwrap();
    match &dispositions[0] {
        UnitDisposition::Refused(refusal @ PublishError::Artifact(_)) => {
            assert!(refusal.is_permanent(), "{refusal:?}");
        }
        other => panic!("expected an artifact refusal, got {other:?}"),
    }
    assert!(!published(&dispositions[1]).replayed);
    assert_eq!(evidence_rows(&corpus).0, 1, "only the sibling is retained");
    assert_eq!(
        corpus.kernel.staged_artifacts_for_test(),
        staged + 1,
        "only the sibling is staged"
    );
    assert!(
        !inventory(&corpus)
            .iter()
            .any(|(_, text, _)| text.contains(SECRET))
    );
}

/// One window naming a unit twice: an equal repeat waits for a later pass, and a changed repeat under the same identity and revision is refused on its own while the first publishes.
#[test]
fn capture_batch_screens_a_repeated_identity_per_unit() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let units = [
        pi_text_unit("pi-twice", "first bytes"),
        pi_text_unit("pi-twice", "first bytes"),
        pi_text_unit("pi-twice", "second bytes"),
    ];
    let dispositions = publish_screened(&corpus.publisher(), &units, NOW, None).unwrap();
    assert!(!published(&dispositions[0]).replayed);
    assert!(matches!(dispositions[1], UnitDisposition::Deferred));
    assert!(matches!(
        dispositions[2],
        UnitDisposition::Refused(PublishError::IdentityReused)
    ));
    assert_eq!(evidence_rows(&corpus).0, 1);

    let again = publish_screened(&corpus.publisher(), &units[1..2], NOW, None).unwrap();
    assert!(published(&again[0]).replayed);
}

#[test]
fn capture_batch_refuses_a_stale_revision_without_failing_siblings() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let publisher = corpus.publisher();
    publisher
        .publish(&pi_text_unit_at("pi-stale", "newer", 200), NOW)
        .unwrap();
    let receipts = publisher_receipts(&corpus);
    let units = [
        pi_text_unit("pi-fresh", "fresh sibling"),
        pi_text_unit_at("pi-stale", "older", 100),
    ];
    let dispositions = publish_screened(&publisher, &units, NOW, None).unwrap();
    assert!(!published(&dispositions[0]).replayed);
    assert!(
        matches!(
            &dispositions[1],
            UnitDisposition::Refused(PublishError::Descriptor {
                refusal: SourceDescriptorError::RevisionNotAdvanced,
                evidence: None,
            })
        ),
        "{:?}",
        dispositions[1]
    );
    assert_eq!(
        evidence_rows(&corpus).0,
        2,
        "the stale unit retained no evidence"
    );
    assert_eq!(
        publisher_receipts(&corpus).difference(&receipts).count(),
        2,
        "receipts for the sibling only"
    );
}

#[test]
fn capture_batch_defers_a_second_revision_of_an_admitted_lineage() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let publisher = corpus.publisher();
    let units = [
        pi_text_unit_at("pi-edited", "first revision", 100),
        pi_text_unit("pi-sibling", "sibling"),
        pi_text_unit_at("pi-edited", "second revision", 200),
    ];
    let dispositions = publish_screened(&publisher, &units, NOW, None).unwrap();
    let first = published(&dispositions[0]);
    assert!(!first.replayed);
    assert!(!published(&dispositions[1]).replayed);
    assert!(
        matches!(dispositions[2], UnitDisposition::Deferred),
        "{:?}",
        dispositions[2]
    );

    let again = publish_screened(&publisher, &units[2..], NOW, None).unwrap();
    let second = published(&again[0]);
    assert!(!second.replayed);
    assert_eq!(second.replaced_object_id.as_ref(), Some(&first.object_id));
}

/// `publish_batch` names the unit whose step refused the attempt and commits nothing; only a receipt conflict is a terminal conflict.
#[test]
fn batch_refusal_names_the_failing_unit_and_only_a_conflict_is_terminal() {
    use daemon::harness_sources::publish_batch;
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let publisher = corpus.publisher();
    publisher
        .publish(
            &pi_units(&pi_session(), &pi_user("pi-stale", "newer", 200))
                .unwrap()
                .remove(0),
            NOW,
        )
        .unwrap();
    let tip = corpus.tip();
    let staged = corpus.kernel.staged_artifacts_for_test();
    let fresh = pi_text_unit("pi-fresh", "fresh sibling");

    // Two requests under one key in one batch: the later one is the conflict.
    let changed = pi_text_unit("pi-fresh", "changed sibling");
    let refusal = publish_batch(
        &corpus.kernel,
        Vec::new(),
        &[(&publisher, &fresh), (&publisher, &changed)],
        NOW,
        None,
    )
    .unwrap_err();
    assert_eq!(refusal.conflicting_unit(), Some(1), "{refusal:?}");

    // A foreign project fails the receipt preview's scope check: named, not terminal.
    let mut foreign = pi_text_unit("pi-foreign", "another project");
    foreign.identity[0].1 = "b".repeat(64);
    let refusal = publish_batch(
        &corpus.kernel,
        Vec::new(),
        &[(&publisher, &foreign), (&publisher, &fresh)],
        NOW,
        None,
    )
    .unwrap_err();
    assert_eq!(refusal.error, kernel::KernelError::NotFound);
    assert_eq!(refusal.unit, Some(0));
    assert_eq!(refusal.conflicting_unit(), None);
    assert_eq!(refusal.refusal, None);
    assert_eq!(
        corpus.kernel.staged_artifacts_for_test(),
        staged,
        "refusals before the transaction stage no bytes"
    );

    // A revision below the live one passes the preview and is refused by its descriptor inside the commit.
    let stale = pi_units(&pi_session(), &pi_user("pi-stale", "older", 100))
        .unwrap()
        .remove(0);
    let refusal = publish_batch(
        &corpus.kernel,
        Vec::new(),
        &[(&publisher, &fresh), (&publisher, &stale)],
        NOW,
        None,
    )
    .unwrap_err();
    assert_eq!(refusal.error, kernel::KernelError::InvalidInput);
    assert_eq!(refusal.unit, Some(1));
    assert_eq!(refusal.conflicting_unit(), None);
    assert_eq!(
        refusal.refusal,
        Some(SourceDescriptorError::RevisionNotAdvanced)
    );

    assert_eq!(corpus.tip(), tip, "no refused attempt committed");
}

#[test]
fn batch_judges_each_unit_against_its_own_publishers_scope() {
    use daemon::harness_sources::publish_batch;
    const OTHER_SCOPE: &str = "project:b";
    let other_project = "b".repeat(64);
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    corpus
        .kernel
        .commit(intent("other-scope"), |envelope| {
            envelope.insert_scope(ScopeSpec {
                scope_id: OTHER_SCOPE.to_owned(),
                object_id: OTHER_SCOPE.to_owned(),
                source_id: OTHER_SCOPE.to_owned(),
                domain_id: "domain".to_owned(),
                source_kind: "kernel_route".to_owned(),
                source_revision: 1,
                sensitivity: Sensitivity::Normal,
                terms: vec![ScopeTermSpec {
                    dimension: Dimension::Project.as_str().to_owned(),
                    operator: "exact".to_owned(),
                    exact_value: Some(other_project.clone()),
                    ..ScopeTermSpec::default()
                }],
            })?;
            Ok(String::new())
        })
        .unwrap();
    let publisher = corpus.publisher();
    let other = SourcePublisher {
        scope_id: Some(OTHER_SCOPE),
        ..corpus.publisher()
    };
    let first = pi_text_unit("pi-first", "first scope");
    let misplaced = pi_text_unit("pi-misplaced", "project a under project b's scope");
    let mut second = pi_text_unit("pi-second", "second scope");
    second.identity[0].1 = other_project;
    let tip = corpus.tip();
    let staged = corpus.kernel.staged_artifacts_for_test();

    for (units, refused) in [
        ([(&publisher, &first), (&other, &misplaced)], 1),
        ([(&other, &misplaced), (&publisher, &first)], 0),
    ] {
        let refusal = publish_batch(&corpus.kernel, Vec::new(), &units, NOW, None).unwrap_err();
        assert_eq!(refusal.error, kernel::KernelError::NotFound);
        assert_eq!(refusal.unit, Some(refused));
        assert_eq!(refusal.conflicting_unit(), None);
    }
    assert_eq!(corpus.tip(), tip, "no refused attempt committed");
    assert_eq!(
        corpus.kernel.staged_artifacts_for_test(),
        staged,
        "the receipt preview refuses before any byte is staged"
    );

    let published = publish_batch(
        &corpus.kernel,
        Vec::new(),
        &[(&publisher, &first), (&other, &second)],
        NOW,
        None,
    )
    .unwrap();
    assert!(published.iter().all(|published| !published.replayed));
    assert!(corpus.tip() > tip);
}

/// A configured repository document at `revision`.
fn document_unit(path: &str, revision: &str, text: &str) -> SourceUnit {
    SourceUnit {
        class: OccurrenceClass::RepositoryDocuments,
        identity: vec![
            ("project_id", PROJECT.to_string()),
            ("repository_id", PROJECT.to_string()),
            ("relative_path", path.to_string()),
        ],
        revision: revision.to_string(),
        representation: Representation::Text,
        text: text.to_string(),
        role: "document".to_string(),
        outcome: None,
    }
}

/// A proposal more than one revision past the lineage head is refused as a head mismatch. The refusal depends on the head: once the lineage reaches the revision before the proposal, the same unit publishes.
#[test]
fn repository_documents_gap_refusal_is_retryable_once_the_gap_closes() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let publisher = corpus.publisher();

    publisher
        .publish(&document_unit("README.md", "1", "# one\n"), NOW)
        .unwrap();
    publisher
        .publish(&document_unit("README.md", "2", "# two\n"), NOW)
        .unwrap();
    let gap = document_unit("README.md", "4", "# four\n");
    let refused = publisher.publish(&gap, NOW);
    assert!(
        matches!(
            &refused,
            Err(PublishError::Descriptor {
                refusal: SourceDescriptorError::HeadMismatch,
                ..
            })
        ),
        "{refused:?}"
    );
    assert!(!refused.unwrap_err().is_permanent());
    publisher
        .publish(&document_unit("README.md", "3", "# three\n"), NOW)
        .unwrap();
    let landed = publisher.publish(&gap, NOW).unwrap();
    assert!(!landed.replayed, "{landed:?}");
    let rows = corpus.export();
    let live: Vec<&SourceRow> = rows
        .iter()
        .filter(|row| {
            row.detail.class == "repository_documents" && row.invalidated_commit_seq.is_none()
        })
        .collect();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].object_id, landed.object_id);
    assert_eq!(live[0].text.as_deref(), Some("# four\n"));
}

/// A repository document retains its own `Sensitive` evidence even when its text equals a commit message in the same screened batch.
#[test]
fn repository_documents_keep_their_own_evidence_beside_an_equal_commit_message() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let publisher = corpus.publisher();
    let text = "Release notes\n";
    let commit = SourceUnit {
        class: OccurrenceClass::GitCommits,
        identity: vec![
            ("repository_id", PROJECT.to_string()),
            ("object_format", "sha1".to_string()),
            ("oid", "b".repeat(40)),
        ],
        revision: "1".to_string(),
        representation: Representation::CommitMessage,
        text: text.to_string(),
        role: "commit".to_string(),
        outcome: None,
    };
    let units = [commit, document_unit("NOTES.md", "1", text)];

    let dispositions = publish_screened(&publisher, &units, NOW, None).unwrap();
    let commit = published(&dispositions[0]);
    let document = published(&dispositions[1]);
    assert_ne!(commit.evidence_object_id, document.evidence_object_id);
    assert_eq!(
        document.admission,
        (
            SourceClass::UntrustedRepoText,
            TaintClass::RepoUntrustedText
        )
    );
    let rows = corpus.export();
    let row = rows
        .iter()
        .find(|row| row.object_id == document.object_id)
        .unwrap();
    assert_eq!(row.sensitivity, Sensitivity::Sensitive);
}

/// AC16 and the U9 admission policy: a `repository_documents` unit admits as untrusted repository text with no repository provenance, so working-tree bytes store `Sensitive`; the next revision supersedes it; a proposal below the retired head or past the head plus one is refused while the head plus one republishes; and a projection builds from a store holding the class.
#[test]
fn repository_documents_publish_untrusted_supersede_and_project() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let publisher = corpus.publisher();

    let first = publisher
        .publish(
            &document_unit("ARCHITECTURE.md", "1", "# Architecture\n"),
            NOW,
        )
        .unwrap();
    assert_eq!(
        first.admission,
        (
            SourceClass::UntrustedRepoText,
            TaintClass::RepoUntrustedText
        )
    );
    let second = publisher
        .publish(
            &document_unit("ARCHITECTURE.md", "2", "# Architecture \n"),
            NOW,
        )
        .unwrap();
    assert_eq!(
        second.replaced_object_id.as_deref(),
        Some(first.object_id.as_str())
    );
    let rows = corpus.export();
    let live: Vec<&SourceRow> = rows
        .iter()
        .filter(|row| {
            row.detail.class == "repository_documents" && row.invalidated_commit_seq.is_none()
        })
        .collect();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].object_id, second.object_id);
    assert_eq!(live[0].sensitivity, Sensitivity::Sensitive);
    assert_eq!(live[0].text.as_deref(), Some("# Architecture \n"));

    corpus
        .kernel
        .commit(intent("retire-document"), |envelope| {
            envelope.retire_observation(&second.object_id)?;
            Ok(String::new())
        })
        .unwrap();
    // Revisions 1 and 2 hold receipts; revision 0 is below the retired head, and 5 skips the head plus one.
    for (revision, refusal) in [
        ("0", SourceDescriptorError::RevisionNotAdvanced),
        ("5", SourceDescriptorError::HeadMismatch),
        // A counter is never judged against the clock: a far revision is a head mismatch, never a revision lead.
        ("99999999999999", SourceDescriptorError::HeadMismatch),
    ] {
        let refused = publisher.publish(
            &document_unit("ARCHITECTURE.md", revision, "# again\n"),
            NOW,
        );
        assert!(
            matches!(
                &refused,
                Err(PublishError::Descriptor { refusal: actual, .. }) if *actual == refusal
            ),
            "{revision}: {refused:?}"
        );
    }
    let republished = publisher
        .publish(&document_unit("ARCHITECTURE.md", "3", "# again\n"), NOW)
        .unwrap();
    assert!(republished.replaced_object_id.is_none());

    let data_home = dir.path().join("data");
    let (projection, rows) = corpus.bootstrap(&data_home);
    drop(projection);
    assert!(
        rows.iter()
            .any(|row| row.detail.class == "repository_documents")
    );
    let conn = Connection::open_with_flags(
        data_home.join("search").join("search.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let projected: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM occurrences WHERE class='repository_documents'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(projected, 1, "the projection holds the live document");
    let dense: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM embedding_jobs j JOIN occurrences o USING(occurrence_id)
             WHERE o.class='repository_documents'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(dense, 0, "repository documents stay lexical-only");
}

/// The shared Pi codec fixture is the producer's row shape: every message entry row reaches the adapter's sources, and every other entry type is refused by its type alone.
#[test]
fn the_shared_pi_fixture_rows_carry_the_entry_identity_the_adapter_reads() {
    let fixture: Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../packages/pi-plugin/src/transform/__fixtures__/pi-codec-parity.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let rows = fixture["rows"].as_array().unwrap();
    let mut messages = 0;
    for row in rows {
        let adapted = pi_units(&pi_session(), row);
        if row["type"] == "message" {
            messages += 1;
            assert!(
                !matches!(
                    adapted,
                    Err(AdapterRefusal::MissingIdentity("type")
                        | AdapterRefusal::UnsupportedShape("type"))
                ),
                "{}: {adapted:?}",
                row["id"]
            );
        } else {
            assert_eq!(
                adapted,
                Err(AdapterRefusal::UnsupportedShape("type")),
                "{}",
                row["id"]
            );
            // A reserved row names the entry it came from, and its type is that entry's.
            let role = row["id"].as_str().unwrap().split(':').nth(1).unwrap();
            let expected = match role {
                "custom" => "custom_message",
                "compactionSummary" => "compaction",
                "branchSummary" => "branch_summary",
                other => panic!("unexpected reserved role {other}"),
            };
            assert_eq!(row["type"], expected, "{}", row["id"]);
        }
    }
    assert!(messages > 0 && messages < rows.len());
}

/// `(retention_class, retain_until, invalidated_commit_seq)` of an evidence row, read straight from the store.
fn evidence_retention(corpus: &Corpus, object_id: &str) -> (String, Option<i64>, Option<i64>) {
    Connection::open_with_flags(&corpus.kernel_db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
        .query_row(
            "SELECT retention_class,retain_until,invalidated_commit_seq FROM evidence_meta
             WHERE object_id=?1",
            [object_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap()
}

/// `(invalidated_commit_seq, superseded_by)` of a descriptor's observation row; an invalidation with no successor is what the projection tombstones as `Retired`.
fn descriptor_retirement(corpus: &Corpus, object_id: &str) -> (Option<i64>, Option<String>) {
    Connection::open_with_flags(&corpus.kernel_db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
        .query_row(
            "SELECT invalidated_commit_seq,superseded_by FROM observations WHERE object_id=?1",
            [object_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
}

/// The change kinds one commit recorded, in commit order.
fn change_kinds(corpus: &Corpus, commit_seq: i64) -> Vec<String> {
    let conn =
        Connection::open_with_flags(&corpus.kernel_db, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mut statement = conn
        .prepare("SELECT change_kind FROM change_event WHERE commit_seq=?1 ORDER BY ordinal")
        .unwrap();
    statement
        .query_map([commit_seq], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

/// An observation outside the capture that cites `evidence_id`: independent support expiry must keep.
fn cite(corpus: &Corpus, evidence_id: &str) {
    corpus
        .kernel
        .commit(intent(&format!("cite:{evidence_id}")), |envelope| {
            envelope.insert_observation(kernel::ObservationSpec {
                observation_id: format!("citer:{evidence_id}"),
                object_id: format!("citer-object:{evidence_id}"),
                domain_id: "domain".to_string(),
                proposition_id: None,
                scope_id: None,
                anchor_id: None,
                evidence_id: Some(evidence_id.to_string()),
                observation_kind: "note".to_string(),
                payload: kernel::ObservationPayload {
                    summary: "independent support".to_string(),
                    classification: "note".to_string(),
                    detail: None,
                },
                observed_at: NOW,
                dependencies: Vec::new(),
                source_kind: "test".to_string(),
                source_id: format!("citer:{evidence_id}"),
                source_revision: 1,
                sensitivity: Sensitivity::Sensitive,
            })?;
            Ok(String::new())
        })
        .unwrap();
}

/// The evidence id behind a published unit's evidence object.
fn evidence_id_of(corpus: &Corpus, evidence_object_id: &str) -> String {
    Connection::open_with_flags(&corpus.kernel_db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
        .query_row(
            "SELECT evidence_id FROM evidence_meta WHERE object_id=?1",
            [evidence_object_id],
            |row| row.get(0),
        )
        .unwrap()
}

/// AC11 and F11: captured message and tool evidence is `harness_capture` owned for 30 days from observation; a replay, even one observed after the deadline, leaves the deadline where the first publication set it; at `retain_until - 1` the capture survives and at `retain_until` its descriptor is retired without a successor and then its evidence, in one commit; a `canonical` row in the same store never ages out.
#[test]
fn harness_capture_evidence_expires_at_its_deadline_and_replay_never_renews_it() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let publisher = corpus.publisher();
    let message =
        &opencode_units(&opencode_session(), &opencode_user("msg_1", "keep me", 100)).unwrap()[0];
    let tools = pi_units(&pi_session(), &pi_tool_result("pi-t1", "pi-a1", false, 200)).unwrap();
    let tool = tools
        .iter()
        .find(|unit| unit.class == OccurrenceClass::RawToolSpans)
        .unwrap();
    let published = publisher.publish(message, NOW).unwrap();
    let tool_published = publisher.publish(tool, NOW + 5).unwrap();
    let deadline = NOW + 30 * DAY_MS;
    assert_eq!(
        daemon::harness_sources::HARNESS_CAPTURE_RETENTION_MS,
        30 * DAY_MS
    );
    assert_eq!(
        evidence_retention(&corpus, &published.evidence_object_id),
        (
            kernel::HARNESS_CAPTURE_RETENTION_CLASS.to_owned(),
            Some(deadline),
            None
        )
    );
    assert_eq!(
        evidence_retention(&corpus, &tool_published.evidence_object_id).1,
        Some(deadline + 5),
        "tool evidence carries its own observation's deadline"
    );
    // A canonical row beside the captures carries no deadline.
    corpus
        .kernel
        .ingest_exact_artifact(kernel::ArtifactIngestRequest {
            intent: intent("canonical"),
            payload: b"a canonical claim's bytes".to_vec(),
            evidence_id: "canonical".to_string(),
            object_id: "canonical-object".to_string(),
            object_kind: "evidence".to_string(),
            domain_id: "domain".to_string(),
            source_kind: "git_commit".to_string(),
            source_id: "canonical".to_string(),
            source_revision: 1,
            media_type: "text/plain".to_string(),
            retention_class: "canonical".to_string(),
            retain_until: None,
            asserted_sensitivity: Sensitivity::Normal,
            provider_egress: ProviderEgress::LocalOnly,
            provenance: None,
        })
        .unwrap();

    // A repository document published beside the captures is canonical evidence.
    let document = publisher
        .publish(&document_unit("README.md", "1", "# Readme\n"), NOW)
        .unwrap();
    assert_eq!(
        evidence_retention(&corpus, &document.evidence_object_id),
        ("canonical".to_owned(), None, None)
    );

    // The same unit observed again after its deadline replays the first rows: the deadline is the first owner's.
    let again = publisher.publish(message, deadline + DAY_MS).unwrap();
    assert!(again.replayed);
    assert_eq!(
        evidence_retention(&corpus, &published.evidence_object_id).1,
        Some(deadline),
        "a replay never renews the deadline"
    );

    let tip = corpus.tip();
    assert_eq!(
        corpus.kernel.expire_harness_captures(deadline - 1).unwrap(),
        kernel::CaptureExpiry::default(),
        "one millisecond before the deadline nothing expires"
    );
    assert_eq!(corpus.tip(), tip, "an empty sweep commits nothing");
    assert_eq!(
        evidence_retention(&corpus, &published.evidence_object_id).2,
        None
    );
    let swept = corpus.kernel.expire_harness_captures(deadline).unwrap();
    assert_eq!(
        (swept.retired, swept.retained),
        (1, 0),
        "only the message is due"
    );
    let (retired_at, successor) = descriptor_retirement(&corpus, &published.object_id);
    assert_eq!(successor, None, "retired, not superseded");
    let retired_at = retired_at.expect("the descriptor is retired");
    assert_eq!(
        evidence_retention(&corpus, &published.evidence_object_id).2,
        Some(retired_at),
        "the evidence goes in the descriptor's commit"
    );
    assert_eq!(
        change_kinds(&corpus, retired_at),
        ["observation_retire", "evidence_retire"],
        "descriptor first, evidence second"
    );

    // Publishing the unit once more after its capture expired still answers from the receipts: nothing is re-retained and the deadline stays.
    let staged = corpus.kernel.staged_artifacts_for_test();
    let after = publisher.publish(message, deadline + 2 * DAY_MS);
    assert!(
        after.as_ref().is_ok_and(|published| published.replayed),
        "{after:?}"
    );
    assert_eq!(corpus.kernel.staged_artifacts_for_test(), staged);
    assert_eq!(
        evidence_retention(&corpus, &published.evidence_object_id),
        (
            kernel::HARNESS_CAPTURE_RETENTION_CLASS.to_owned(),
            Some(deadline),
            Some(retired_at)
        )
    );

    // Far past every deadline, the tool capture goes and the canonical row stays.
    let late = corpus
        .kernel
        .expire_harness_captures(NOW + 32 * DAY_MS)
        .unwrap();
    assert_eq!((late.retired, late.retained), (1, 0));
    for object in ["canonical-object", document.evidence_object_id.as_str()] {
        assert_eq!(
            evidence_retention(&corpus, object),
            ("canonical".to_owned(), None, None),
            "canonical evidence never ages out"
        );
    }
    assert_eq!(
        corpus
            .kernel
            .expire_harness_captures(NOW + 32 * DAY_MS)
            .unwrap(),
        kernel::CaptureExpiry::default()
    );
}

/// F12, F13, F14: a live hold keeps an expired capture until the hold lapses; a capture another row cites loses its descriptor and keeps its evidence, and leaves the sweep; 65 expired captures with one cited drain in two calls, the cited row blocking nothing behind it in its page.
#[test]
fn harness_capture_expiry_keeps_held_and_cited_evidence_and_drains_in_pages() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let publisher = corpus.publisher();
    let published: Vec<_> = (0..65)
        .map(|index| {
            let unit = &opencode_units(
                &opencode_session(),
                &opencode_user(&format!("msg_{index:02}"), &format!("text {index}"), 100),
            )
            .unwrap()[0];
            publisher.publish(unit, NOW).unwrap()
        })
        .collect();
    let deadline = NOW + 30 * DAY_MS;
    // The first capture in page order is cited by an independent row.
    let mut by_evidence: Vec<_> = published
        .iter()
        .map(|published| {
            (
                evidence_id_of(&corpus, &published.evidence_object_id),
                published,
            )
        })
        .collect();
    by_evidence.sort_by(|a, b| a.0.cmp(&b.0));
    let (cited_evidence, cited) = by_evidence[0].clone();
    cite(&corpus, &cited_evidence);

    // A live hold pins every capture: the sweep retires none until the hold lapses.
    let hold = corpus
        .kernel
        .capture_source_hold(
            &corpus.binding(),
            SourceHoldBounds {
                max_descriptor_rows: NonZeroUsize::new(256).unwrap(),
                admission: SourceHoldAdmission {
                    max_references: NonZeroUsize::new(128).unwrap(),
                    max_encoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
                },
                expiry_ms: NonZeroU64::new(DAY_MS as u64).unwrap(),
            },
        )
        .unwrap();
    assert_eq!(
        corpus.kernel.expire_harness_captures(deadline).unwrap(),
        kernel::CaptureExpiry::default(),
        "held captures are skipped"
    );
    assert!(published.iter().all(|published| {
        evidence_retention(&corpus, &published.evidence_object_id)
            .2
            .is_none()
    }));
    drop(hold);

    // Past the hold's own expiry the captures are due again: 64 in the first call, the cited one among them kept.
    let after_hold = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
        + 2 * DAY_MS;
    let first = corpus.kernel.expire_harness_captures(after_hold).unwrap();
    assert_eq!((first.retired, first.retained), (63, 1));
    assert!(
        descriptor_retirement(&corpus, &cited.object_id).0.is_some(),
        "the cited capture's descriptor is retired"
    );
    assert_eq!(
        evidence_retention(&corpus, &cited.evidence_object_id).2,
        None,
        "evidence another row cites is kept"
    );
    let second = corpus.kernel.expire_harness_captures(after_hold).unwrap();
    assert_eq!(
        (second.retired, second.retained),
        (1, 0),
        "ceil(65 / 64) calls drain the backlog"
    );
    assert!(
        published
            .iter()
            .filter(|published| published.object_id != cited.object_id)
            .all(
                |published| evidence_retention(&corpus, &published.evidence_object_id)
                    .2
                    .is_some()
            )
    );
    assert_eq!(
        corpus.kernel.expire_harness_captures(after_hold).unwrap(),
        kernel::CaptureExpiry::default(),
        "a retained capture leaves the sweep while it is cited"
    );
    // Once the citation goes, the kept evidence is due again and retires. Expiry receipts are keyed by the cutoff, so a sweep at the same cutoff replays the earlier `retained` receipt; the next pass runs at a later clock.
    corpus
        .kernel
        .commit(intent("uncite"), |envelope| {
            envelope.retire_observation(&format!("citer-object:{cited_evidence}"))?;
            Ok(String::new())
        })
        .unwrap();
    let last = corpus
        .kernel
        .expire_harness_captures(after_hold + 1)
        .unwrap();
    assert_eq!((last.retired, last.retained), (1, 0));
    assert!(
        evidence_retention(&corpus, &cited.evidence_object_id)
            .2
            .is_some()
    );
}

/// An OpenCode assistant record in this test's session holding the one tool `part`, under `info`.
fn opencode_record(info: &Value, part: &Value) -> Value {
    let mut info = info.clone();
    info["sessionID"] = json!(SESSION);
    json!({ "info": info, "parts": [part] })
}

/// The outcome record of the one tool unit an OpenCode record yields.
fn opencode_outcome(record: &Value) -> kernel::ToolOutcome {
    let units = opencode_units(&opencode_session(), record).unwrap();
    assert_eq!(units.len(), 1);
    units[0]
        .outcome
        .clone()
        .expect("a tool unit carries its outcome")
}

/// AC12 and OQ3: every case of the pinned OpenCode 1.19.0 trace yields exactly its recorded outcome. `exit` maps to `exit_code` only on a `bash` call and only as an integer; an absent, null, or malformed field stays unknown, never a zero; `exception_ref` names the aborting error only for a call the abort interrupted.
#[test]
fn opencode_tool_outcomes_follow_the_pinned_trace() {
    let trace: Value =
        serde_json::from_str(include_str!("../testdata/opencode-tool-outcome-trace.json")).unwrap();
    for case in trace["cases"].as_array().unwrap() {
        let expected: kernel::ToolOutcome =
            serde_json::from_value(case["outcome"].clone()).unwrap();
        assert_eq!(
            opencode_outcome(&opencode_record(&case["info"], &case["part"])),
            expected,
            "{}",
            case["name"]
        );
    }
    let completed_exit_1 = &trace["cases"][0];
    let with = |edit: &dyn Fn(&mut Value)| {
        let mut part = completed_exit_1["part"].clone();
        edit(&mut part);
        opencode_outcome(&opencode_record(&completed_exit_1["info"], &part))
    };
    // Malformed metadata is unknown, not a default.
    let malformed = with(&|part| {
        part["state"]["metadata"]["exit"] = json!("1");
        part["state"]["metadata"]["truncated"] = json!("no");
    });
    assert_eq!((malformed.exit_code, malformed.truncated), (None, None));
    // Absent metadata reports neither.
    let absent = with(&|part| {
        part["state"].as_object_mut().unwrap().remove("metadata");
    });
    assert_eq!((absent.exit_code, absent.truncated), (None, None));
    // The trace proves `exit` for `bash` only; another tool's `exit` is not an exit code.
    let other_tool = with(&|part| part["tool"] = json!("task"));
    assert_eq!(other_tool.exit_code, None);
    assert_eq!(other_tool.tool_name, "task");
    // A part with no tool name has no outcome, and the adapter refuses it.
    let mut nameless = completed_exit_1["part"].clone();
    nameless.as_object_mut().unwrap().remove("tool");
    assert!(matches!(
        opencode_units(
            &opencode_session(),
            &opencode_record(&completed_exit_1["info"], &nameless)
        ),
        Err(AdapterRefusal::UnsupportedShape("tool"))
    ));

    let aborted = &trace["cases"][5];
    // An aborted turn names the exception only on the call it interrupted.
    let mut settled = aborted["part"].clone();
    settled["state"]["metadata"]["interrupted"] = json!(false);
    assert_eq!(
        opencode_outcome(&opencode_record(&aborted["info"], &settled)).exception_ref,
        None
    );
    // A malformed error name is unknown.
    let mut info = aborted["info"].clone();
    info["error"]["name"] = json!("not a name!");
    assert_eq!(
        opencode_outcome(&opencode_record(&info, &aborted["part"])).exception_ref,
        None
    );
    // A completed turn that names no error leaves the interrupted call's exception unknown.
    info.as_object_mut().unwrap().remove("error");
    assert_eq!(
        opencode_outcome(&opencode_record(&info, &aborted["part"])).exception_ref,
        None
    );
}

/// D8 stability: an interrupted OpenCode call is read once its turn completes, so the abort's error name is final when the record is built. Before completion the call yields no unit; after it, the record names the error and a later observation replays.
#[test]
fn an_interrupted_call_waits_for_its_turn_to_complete() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let publisher = corpus.publisher();
    let trace: Value =
        serde_json::from_str(include_str!("../testdata/opencode-tool-outcome-trace.json")).unwrap();
    let aborted = &trace["cases"][5];
    let mut running = aborted["info"].clone();
    running["time"].as_object_mut().unwrap().remove("completed");
    running.as_object_mut().unwrap().remove("error");
    assert!(
        opencode_units(
            &opencode_session(),
            &opencode_record(&running, &aborted["part"])
        )
        .unwrap()
        .is_empty(),
        "an interrupted call of a running turn is deferred"
    );
    let settled = opencode_record(&aborted["info"], &aborted["part"]);
    let unit = opencode_units(&opencode_session(), &settled)
        .unwrap()
        .remove(0);
    assert_eq!(
        unit.outcome.as_ref().unwrap().exception_ref.as_deref(),
        Some("MessageAbortedError")
    );
    // The trace's timestamps are real OpenCode clock values; the observation follows them.
    let observed_at = 1_800_000_000_000;
    assert!(!publisher.publish(&unit, observed_at).unwrap().replayed);
    let again = opencode_units(&opencode_session(), &settled)
        .unwrap()
        .remove(0);
    assert!(publisher.publish(&again, observed_at).unwrap().replayed);
}

/// AC12 for Pi: a tool result's status follows `isError`, each text block carries its own byte length, truncation is read from the bash tool's `details.truncation`, and a Pi entry yields no exit code or exception name.
#[test]
fn pi_tool_outcomes_read_status_and_reported_truncation() {
    let mut entry = pi_tool_result("pi-t1", "pi-a1", false, 200);
    entry["message"]["toolName"] = json!("bash");
    let units = pi_units(&pi_session(), &entry).unwrap();
    let outcomes: Vec<_> = units
        .iter()
        .map(|unit| unit.outcome.clone().unwrap())
        .collect();
    assert_eq!(
        outcomes[0],
        kernel::ToolOutcome {
            version: kernel::TOOL_OUTCOME_VERSION,
            status: kernel::ToolStatus::Completed,
            tool_name: "bash".to_owned(),
            byte_length: TOOL_BYTES.len() as u64,
            truncated: None,
            exit_code: None,
            exception_ref: None,
        }
    );
    assert_eq!(
        outcomes[1].byte_length, 0,
        "the empty block is its own record"
    );

    entry["message"]["isError"] = json!(true);
    entry["message"]["details"] = json!({ "truncation": { "truncated": true, "truncatedBy": "lines" }, "fullOutputPath": "/tmp/out" });
    let failed = pi_units(&pi_session(), &entry).unwrap()[0]
        .outcome
        .clone()
        .unwrap();
    assert_eq!(
        (failed.status, failed.truncated, failed.exit_code),
        (kernel::ToolStatus::Error, Some(true), None)
    );

    entry["message"].as_object_mut().unwrap().remove("toolName");
    assert!(matches!(
        pi_units(&pi_session(), &entry),
        Err(AdapterRefusal::UnsupportedShape("message.toolName"))
    ));
}

/// D7 and AC12: the record binds to the occurrence, not the artifact. Two `bash` calls that returned the same bytes, one completed with exit 1 and one failed, are two descriptors citing one artifact, each with its own record.
#[test]
fn equal_tool_bytes_with_different_outcomes_are_two_descriptors_of_one_artifact() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let publisher = corpus.publisher();
    let info = json!({ "id": "msg_same", "role": "assistant",
                       "time": { "created": 100, "completed": 900 } });
    let completed = json!({ "type": "tool", "tool": "bash", "callID": "call_a",
        "state": { "status": "completed", "input": {}, "output": "boom\n",
                   "metadata": { "exit": 1, "truncated": false }, "time": { "start": 1, "end": 500 } } });
    let failed = json!({ "type": "tool", "tool": "bash", "callID": "call_b",
        "state": { "status": "error", "input": {}, "error": "boom\n", "time": { "start": 1, "end": 600 } } });
    for part in [&completed, &failed] {
        for unit in opencode_units(&opencode_session(), &opencode_record(&info, part)).unwrap() {
            publisher.publish(&unit, NOW).unwrap();
        }
    }
    let rows = corpus.export();
    assert_eq!(rows.len(), 2, "two occurrences, two descriptors");
    assert_eq!(
        rows[0].detail.artifact_digest,
        rows[1].detail.artifact_digest
    );
    let mut outcomes: Vec<_> = rows
        .iter()
        .map(|row| {
            let outcome = row
                .detail
                .outcome
                .clone()
                .expect("each descriptor stores its record");
            (outcome.status, outcome.exit_code)
        })
        .collect();
    outcomes.sort_by_key(|(_, exit_code)| *exit_code);
    assert_eq!(
        outcomes,
        [
            (kernel::ToolStatus::Error, None),
            (kernel::ToolStatus::Completed, Some(1))
        ]
    );
}

/// AC12: a record the kernel would refuse is refused before any byte is retained: one over 1 KiB, one whose byte length is not its unit's, one on a class other than `raw_tool_spans`, and one with an empty tool name. The screened batch path refuses the same unit and keeps its siblings admissible.
#[test]
fn an_invalid_outcome_record_is_refused_before_retention() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let publisher = corpus.publisher();
    let tool = pi_units(&pi_session(), &pi_tool_result("pi-t1", "pi-a1", false, 200))
        .unwrap()
        .remove(0);
    let mut oversized = tool.clone();
    oversized.outcome.as_mut().unwrap().exception_ref = Some("E".repeat(512));
    oversized.outcome.as_mut().unwrap().tool_name = "t".repeat(512);
    assert!(oversized.outcome.as_ref().unwrap().encode().len() > 1024);
    let mut misdescribed = tool.clone();
    misdescribed.outcome.as_mut().unwrap().byte_length += 1;
    let mut message = pi_units(&pi_session(), &pi_user("pi-u1", "hello", 100))
        .unwrap()
        .remove(0);
    message.outcome = tool.outcome.clone();
    let mut nameless = pi_tool_result("pi-t2", "pi-a1", false, 200);
    nameless["message"]["toolName"] = json!("");
    let nameless = pi_units(&pi_session(), &nameless).unwrap().remove(0);
    let tip = corpus.tip();
    let staged = corpus.kernel.staged_artifacts_for_test();
    for unit in [&oversized, &misdescribed, &message, &nameless] {
        assert!(
            matches!(
                publisher.publish(unit, NOW),
                Err(PublishError::Descriptor {
                    refusal: SourceDescriptorError::OutcomeRefused,
                    evidence: None
                })
            ),
            "{unit:?}"
        );
    }
    assert_eq!(corpus.tip(), tip, "nothing commits");
    assert_eq!(
        corpus.kernel.staged_artifacts_for_test(),
        staged,
        "nothing is retained"
    );
    let dispositions = publish_screened(&publisher, &[oversized, tool], NOW, None).unwrap();
    assert!(matches!(
        dispositions[0],
        UnitDisposition::Refused(PublishError::Descriptor {
            refusal: SourceDescriptorError::OutcomeRefused,
            ..
        })
    ));
    assert!(matches!(dispositions[1], UnitDisposition::Published(_)));
}

/// The frozen `source-request.v1` layout: the v2 inputs before the outcome record, under the v1 tag.
fn v1_digest(unit: &SourceUnit) -> String {
    let mut bytes = Vec::new();
    for part in [
        "source-request.v1",
        unit.text.as_str(),
        "domain",
        SCOPE,
        unit.role.as_str(),
        Sensitivity::Normal.as_str(),
    ] {
        bytes.extend_from_slice(&(part.len() as u64).to_be_bytes());
        bytes.extend_from_slice(part.as_bytes());
    }
    bytes.extend_from_slice(&[1, 1]);
    kernel::source_identity::identity_digest(&bytes)
}

/// Rewrites the stored request digest of `unit`'s `kind` receipt (`source-descriptor` or `source-evidence`) to `digest`, as a store written before the outcome record holds it.
fn store_receipt_as(
    corpus: &Corpus,
    publisher: &SourcePublisher<'_>,
    unit: &SourceUnit,
    kind: &str,
    digest: &str,
) {
    let (key, _) = publisher.receipt_identity(unit).unwrap();
    // The store guards receipt identity with a trigger; the rewrite lifts it for this one statement and reinstalls it unchanged.
    let conn = Connection::open(&corpus.kernel_db).unwrap();
    let guards: Vec<(String, String)> = conn
        .prepare(
            "SELECT name,sql FROM sqlite_master
             WHERE type='trigger' AND tbl_name='operation_receipts'",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    for (name, _) in &guards {
        conn.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();
    }
    let changed = conn
        .execute(
            "UPDATE operation_receipts SET request_digest=?1
             WHERE producer='eidnara-daemon/harness-sources' AND operation_key=?2",
            rusqlite::params![digest, format!("{kind}:{key}")],
        )
        .unwrap();
    for (_, sql) in &guards {
        conn.execute_batch(sql).unwrap();
    }
    assert_eq!(changed, 1, "{kind} receipt");
}

/// AC12 receipts (OQ9): a store with `source-request.v1` receipts under the v2 publisher. A unit whose receipts are v1 replays with `replayed: true` and keeps its stored descriptor without an outcome record; a changed payload or publishing context under that key is `IdentityReused`; a v1 evidence-only receipt completes with a descriptor under v2, on the single and the batch path; and a unit first published after the upgrade publishes and replays under v2.
#[test]
fn receipts_written_before_the_outcome_record_replay_under_v2() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let publisher = corpus.publisher();
    let message = pi_units(&pi_session(), &pi_user("pi-u1", "hello", 100))
        .unwrap()
        .remove(0);
    assert_eq!(
        v1_digest(&message),
        "9ff69ac08200498a7b8827c97faabbcdfaf323061593dccce0e6ea31668f3723",
        "the frozen v1 layout"
    );

    // A pre-upgrade tool descriptor carries no record; its receipts hold the v1 digest.
    let tool = pi_units(&pi_session(), &pi_tool_result("pi-t1", "pi-a1", false, 200))
        .unwrap()
        .remove(0);
    let legacy_tool = SourceUnit {
        outcome: None,
        ..tool.clone()
    };
    for unit in [&message, &legacy_tool] {
        publisher.publish(unit, NOW).unwrap();
        for kind in ["source-descriptor", "source-evidence"] {
            store_receipt_as(&corpus, &publisher, unit, kind, &v1_digest(unit));
        }
    }
    let tip = corpus.tip();
    for unit in [&message, &tool] {
        assert!(publisher.publish(unit, NOW).unwrap().replayed);
    }
    let dispositions =
        publish_screened(&publisher, &[message.clone(), tool.clone()], NOW, None).unwrap();
    assert!(
        dispositions
            .iter()
            .all(|disposition| matches!(disposition, UnitDisposition::Published(published) if published.replayed)),
        "{dispositions:?}"
    );
    assert_eq!(corpus.tip(), tip, "a legacy replay commits nothing");
    let stored_tool = corpus
        .export()
        .into_iter()
        .find(|row| row.detail.class == "raw_tool_spans")
        .unwrap();
    assert_eq!(stored_tool.detail.outcome, None, "a replay does not enrich");

    // Changed bytes or context under a legacy key match neither digest.
    let mut changed = message.clone();
    changed.text = "goodbye".to_owned();
    assert!(matches!(
        publisher.publish(&changed, NOW),
        Err(PublishError::IdentityReused)
    ));
    let elsewhere = SourcePublisher {
        egress: ProviderEgress::RemoteAllowed,
        ..corpus.publisher()
    };
    assert!(matches!(
        elsewhere.publish(&message, NOW),
        Err(PublishError::IdentityReused)
    ));

    // A v1 evidence-only receipt: evidence retained, then its descriptor refused for a missing scope.
    const LATE_SCOPE: &str = "project:late";
    let late = SourcePublisher {
        scope_id: Some(LATE_SCOPE),
        ..corpus.publisher()
    };
    let singles = pi_units(&pi_session(), &pi_user("pi-u2", "single path", 300)).unwrap();
    let batched = pi_units(&pi_session(), &pi_user("pi-u3", "batch path", 400)).unwrap();
    for unit in [&singles[0], &batched[0]] {
        assert!(matches!(
            late.publish(unit, NOW),
            Err(PublishError::Kernel {
                error: kernel::KernelError::NotFound,
                evidence: Some(_)
            })
        ));
        let mut v1 = Vec::new();
        for part in [
            "source-request.v1",
            unit.text.as_str(),
            "domain",
            LATE_SCOPE,
            unit.role.as_str(),
            Sensitivity::Normal.as_str(),
        ] {
            v1.extend_from_slice(&(part.len() as u64).to_be_bytes());
            v1.extend_from_slice(part.as_bytes());
        }
        v1.extend_from_slice(&[1, 1]);
        store_receipt_as(
            &corpus,
            &late,
            unit,
            "source-evidence",
            &kernel::source_identity::identity_digest(&v1),
        );
    }
    corpus
        .kernel
        .commit(intent("late-scope"), |envelope| {
            envelope.insert_scope(ScopeSpec {
                scope_id: LATE_SCOPE.to_owned(),
                object_id: LATE_SCOPE.to_owned(),
                source_id: LATE_SCOPE.to_owned(),
                domain_id: "domain".to_owned(),
                source_kind: "kernel_route".to_owned(),
                source_revision: 1,
                sensitivity: Sensitivity::Normal,
                terms: vec![ScopeTermSpec {
                    dimension: Dimension::Project.as_str().to_owned(),
                    operator: "exact".to_owned(),
                    exact_value: Some(PROJECT.to_owned()),
                    ..ScopeTermSpec::default()
                }],
            })?;
            Ok(String::new())
        })
        .unwrap();
    let single = late.publish(&singles[0], NOW).unwrap();
    assert!(!single.replayed, "the descriptor is new");
    let staged = corpus.kernel.staged_artifacts_for_test();
    let batch = publish_screened(&late, &batched, NOW, None).unwrap();
    assert!(matches!(&batch[0], UnitDisposition::Published(published) if !published.replayed));
    assert_eq!(
        corpus.kernel.staged_artifacts_for_test(),
        staged,
        "the batch cites the v1 evidence without offering its bytes again"
    );
    let conn =
        Connection::open_with_flags(&corpus.kernel_db, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    for unit in [&singles[0], &batched[0]] {
        let (key, v2) = late.receipt_identity(unit).unwrap();
        let stored: String = conn
            .query_row(
                "SELECT request_digest FROM operation_receipts WHERE operation_key=?1",
                [format!("source-descriptor:{key}")],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored, v2, "the completing descriptor is written under v2");
        assert!(late.publish(unit, NOW).unwrap().replayed);
    }

    // A unit first published after the upgrade publishes and replays under v2.
    let fresh = pi_units(&pi_session(), &pi_tool_result("pi-t9", "pi-a9", true, 900)).unwrap();
    assert!(!publisher.publish(&fresh[0], NOW).unwrap().replayed);
    assert!(publisher.publish(&fresh[0], NOW).unwrap().replayed);
}

/// A Pi tool result of call `call` at native timestamp `stamp`, with the given text blocks.
fn pi_rerun(call: &str, stamp: u64, is_error: bool, blocks: &[&str]) -> Vec<SourceUnit> {
    pi_rerun_under("pi-a1", call, stamp, is_error, blocks)
}

fn pi_rerun_under(
    parent: &str,
    call: &str,
    stamp: u64,
    is_error: bool,
    blocks: &[&str],
) -> Vec<SourceUnit> {
    let mut entry = pi_tool_result(&format!("pi-t-{stamp}"), parent, is_error, stamp);
    entry["message"]["toolCallId"] = json!(call);
    entry["message"]["content"] = json!(
        blocks
            .iter()
            .map(|text| json!({ "type": "text", "text": text }))
            .collect::<Vec<_>>()
    );
    pi_units(&pi_session(), &entry).unwrap()
}

/// The live tool descriptors of `call`: `(result_revision, block_index)` pairs, sorted.
fn live_generation_of(corpus: &Corpus, call: &str) -> Vec<(String, String)> {
    let mut live: Vec<(String, String)> = corpus
        .export()
        .into_iter()
        .filter(|row| row.detail.class == "raw_tool_spans")
        .filter(|row| row.detail.identity[4].1 == call)
        .map(|row| {
            (
                row.detail.identity[5].1.clone(),
                row.detail.identity[6].1.clone(),
            )
        })
        .collect();
    live.sort();
    live
}

fn published_units(dispositions: &[UnitDisposition]) -> Vec<&daemon::harness_sources::Published> {
    dispositions
        .iter()
        .map(|disposition| match disposition {
            UnitDisposition::Published(published) => published,
            other => panic!("expected a publication, got {other:?}"),
        })
        .collect()
}

/// AC14 and C8 (`sc-rerun-exactly-one-live-generation`): a re-run of one call retires the predecessor generation in the commit that publishes the successor, whatever the delivery order. In order (1, 2) the predecessor's invalidation is the successor's creation and its row names no successor, the state the projection tombstones as `Retired`, and its bytes stay readable. A redelivered 1 replays and retires nothing (1, 2, 1), a redelivered 1 with other bytes is `IdentityReused`, and a duplicate 2 replays. Delivered reversed (2, 1), the stale generation is superseded and retains nothing. An error that becomes a success, a re-run with fewer blocks, and two generations in one batch each leave one live generation, and a distinct call with equal text coexists.
#[test]
fn a_tool_rerun_leaves_exactly_one_live_generation() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let publisher = corpus.publisher();

    // In order, an error that became a success with fewer blocks.
    let first = pi_rerun("call_a", 100, true, &["error: one\n", "error: two\n"]);
    let second = pi_rerun("call_a", 200, false, &["ok\n"]);
    let sibling = pi_rerun("call_b", 100, true, &["error: one\n", "error: two\n"]);
    let gen1 = published_units(&publish_screened(&publisher, &first, NOW, None).unwrap())
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    publish_screened(&publisher, &sibling, NOW, None).unwrap();
    let gen2 =
        published_units(&publish_screened(&publisher, &second, NOW, None).unwrap())[0].clone();
    assert!(!gen2.replayed);
    assert_eq!(
        live_generation_of(&corpus, "call_a"),
        [("200".to_owned(), "0".to_owned())]
    );
    assert_eq!(
        live_generation_of(&corpus, "call_b").len(),
        2,
        "a distinct call with equal text coexists"
    );
    let conn =
        Connection::open_with_flags(&corpus.kernel_db, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let created: i64 = conn
        .query_row(
            "SELECT created_commit_seq FROM observations WHERE object_id=?1",
            [&gen2.object_id],
            |row| row.get(0),
        )
        .unwrap();
    for old in &gen1 {
        assert_eq!(
            descriptor_retirement(&corpus, &old.object_id),
            (Some(created), None),
            "retired in the successor's commit, with no successor of its own"
        );
        assert_eq!(
            evidence_retention(&corpus, &old.evidence_object_id).2,
            None,
            "the retired generation's bytes stay as history"
        );
    }

    // A stale generation re-offered with other bytes under its key is the identity conflict it is, not a supersession.
    let mut changed = first.clone();
    changed[0].text = "error: rewritten\n".to_owned();
    changed[0].outcome.as_mut().unwrap().byte_length = changed[0].text.len() as u64;
    let tip = corpus.tip();
    let refused = publish_screened(&publisher, &changed[..1], NOW, None).unwrap();
    assert!(matches!(
        refused[..],
        [UnitDisposition::Refused(PublishError::IdentityReused)]
    ));
    assert_eq!(corpus.tip(), tip);
    assert_eq!(
        live_generation_of(&corpus, "call_a"),
        [("200".to_owned(), "0".to_owned())]
    );

    // Redelivery of 1, then a duplicate 2: replays that retire nothing.
    let tip = corpus.tip();
    for window in [&first, &second] {
        let replays = publish_screened(&publisher, window, NOW, None).unwrap();
        assert!(
            published_units(&replays)
                .iter()
                .all(|published| published.replayed)
        );
    }
    assert_eq!(corpus.tip(), tip, "a replay commits nothing");
    assert_eq!(
        live_generation_of(&corpus, "call_a"),
        [("200".to_owned(), "0".to_owned())]
    );

    // Reversed: 2 first, then a 1 that was never published.
    let newer = pi_rerun("call_c", 200, false, &["ok\n"]);
    let older = pi_rerun("call_c", 100, false, &["stale\n"]);
    publish_screened(&publisher, &newer, NOW, None).unwrap();
    let tip = corpus.tip();
    let staged = corpus.kernel.staged_artifacts_for_test();
    let stale = publish_screened(&publisher, &older, NOW, None).unwrap();
    assert!(matches!(stale[..], [UnitDisposition::Superseded]));
    assert_eq!(corpus.tip(), tip, "a stale generation commits nothing");
    assert_eq!(corpus.kernel.staged_artifacts_for_test(), staged);
    assert_eq!(
        live_generation_of(&corpus, "call_c"),
        [("200".to_owned(), "0".to_owned())]
    );

    // A window holding the published generation 1 beside a new generation 2, as a Pi transcript does: 1 replays and 2 retires it in the same commit.
    let shown = pi_rerun("call_e", 100, false, &["first\n"]);
    let gen1 =
        published_units(&publish_screened(&publisher, &shown, NOW, None).unwrap())[0].clone();
    let mut window = shown.clone();
    window.extend(pi_rerun("call_e", 200, false, &["second\n"]));
    let dispositions = publish_screened(&publisher, &window, NOW, None).unwrap();
    let both = published_units(&dispositions);
    assert!(both[0].replayed && !both[1].replayed);
    let created: i64 = conn
        .query_row(
            "SELECT created_commit_seq FROM observations WHERE object_id=?1",
            [&both[1].object_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        descriptor_retirement(&corpus, &gen1.object_id).0,
        Some(created)
    );
    assert_eq!(
        live_generation_of(&corpus, "call_e"),
        [("200".to_owned(), "0".to_owned())]
    );

    // Two generations of one call in one batch: the older is superseded.
    let mut both = pi_rerun("call_d", 100, false, &["first\n"]);
    both.extend(pi_rerun("call_d", 300, false, &["third\n"]));
    let dispositions = publish_screened(&publisher, &both, NOW, None).unwrap();
    assert!(matches!(dispositions[0], UnitDisposition::Superseded));
    assert!(matches!(dispositions[1], UnitDisposition::Published(_)));
    assert_eq!(
        live_generation_of(&corpus, "call_d"),
        [("300".to_owned(), "0".to_owned())]
    );

    // One `tool_call_id` under another parent message is another call: a re-run under the first parent leaves it live.
    let other_parent = pi_rerun_under("pi-a2", "call_a", 100, false, &["other\n"]);
    publish_screened(&publisher, &other_parent, NOW, None).unwrap();
    let third = pi_rerun("call_a", 300, false, &["third\n"]);
    publish_screened(&publisher, &third, NOW, None).unwrap();
    let mut live: Vec<(String, String)> = corpus
        .export()
        .into_iter()
        .filter(|row| row.detail.class == "raw_tool_spans")
        .filter(|row| row.detail.identity[4].1 == "call_a")
        .map(|row| {
            (
                row.detail.identity[3].1.clone(),
                row.detail.identity[5].1.clone(),
            )
        })
        .collect();
    live.sort();
    assert_eq!(
        live,
        [
            ("pi-a1".to_owned(), "300".to_owned()),
            ("pi-a2".to_owned(), "100".to_owned()),
        ]
    );
}

/// OQ11 measurement: the cost of one capture batch of re-runs against a store holding 10 000 live `raw_tool_spans` descriptors, against the 500 ms criterion: a full 32-unit window of distinct calls, every one retiring a predecessor. Run with `cargo +1.98 nextest run --locked -p daemon --all-features --run-ignored only -E 'test(rerun_lookup_cost)' --no-capture`.
#[test]
#[ignore = "measurement"]
fn rerun_lookup_cost_at_ten_thousand_live_tool_descriptors() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let publisher = corpus.publisher();
    for batch in 0..10 {
        let units: Vec<SourceUnit> = (0..1000)
            .flat_map(|index| {
                pi_rerun(
                    &format!("call_{batch}_{index}"),
                    100,
                    false,
                    &[&format!("output {batch}")],
                )
            })
            .collect();
        let pairs: Vec<(&SourcePublisher<'_>, &SourceUnit)> =
            units.iter().map(|unit| (&publisher, unit)).collect();
        daemon::harness_sources::publish_batch(&corpus.kernel, Vec::new(), &pairs, NOW, None)
            .unwrap();
    }
    // A full capture window: 32 tool results of distinct calls, every one a re-run, so the batch reads 32 calls and retires 32 predecessors.
    let mut samples = Vec::new();
    for round in 0..5u64 {
        let window: Vec<SourceUnit> = (0..MAX_SOURCE_BATCH_UNITS)
            .flat_map(|index| {
                pi_rerun(
                    &format!("call_{round}_{index}"),
                    200 + round,
                    false,
                    &["re-run\n"],
                )
            })
            .collect();
        let started = std::time::Instant::now();
        let dispositions = publish_screened(&publisher, &window, NOW, None).unwrap();
        samples.push(started.elapsed());
        assert!(dispositions
            .iter()
            .all(|disposition| matches!(disposition, UnitDisposition::Published(published) if !published.replayed)));
    }
    samples.sort();
    println!(
        "32-call re-run window at 10000 live tool descriptors: min {:?} median {:?} max {:?}",
        samples[0], samples[2], samples[4]
    );
    // The same store publishing message text, which reads no tool inventory: the difference is the lookup.
    let mut baseline = Vec::new();
    for round in 0..5u64 {
        let message = pi_units(
            &pi_session(),
            &pi_user(&format!("pi-m{round}"), "baseline", 100 + round),
        )
        .unwrap();
        let started = std::time::Instant::now();
        publish_screened(&publisher, &message, NOW, None).unwrap();
        baseline.push(started.elapsed());
    }
    baseline.sort();
    println!(
        "message publication on the same store: min {:?} median {:?} max {:?}",
        baseline[0], baseline[2], baseline[4]
    );
    // Each re-run retired its predecessor: the call has one live descriptor, the last.
    let live: i64 =
        Connection::open_with_flags(&corpus.kernel_db, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM observations
             WHERE observation_kind='source_descriptor' AND invalidated_commit_seq IS NULL
               AND instr(observation_payload, ?1) > 0",
                [r#"[\"tool_call_id\",\"call_0_0\"]"#],
                |row| row.get(0),
            )
            .unwrap();
    assert_eq!(live, 1);
}
