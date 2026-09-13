//! Both harness adapters against a real kernel and a real projection: native identity is bound or refused, equal text stays distinct, tool strings survive exactly, secret-bearing bytes retain nothing, replay returns receipts, revisions succeed atomically, and only message text creates dense work.

use std::collections::{BTreeMap, BTreeSet};
use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use daemon::embedding_dispatch::{DispatchBounds, DispatchEvent, EmbeddingDispatcher};
use daemon::harness_sources::{
    AdapterRefusal, Harness, PublishError, Representation, SessionIdentity, SourcePublisher,
    SourceUnit, opencode_units, pi_units,
};
use daemon::search_projection::SearchProjection;
use host_runtime::synapse::inference::InferenceError;
use host_runtime::synapse::{
    EmbedTokens, EmbeddingEngine, LaneInfo, SynapseComponent, SynapseLimits,
};
use kernel::applicability::EvalBudget;
use kernel::source_identity::OccurrenceClass;
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

fn component(engine: &Arc<TestEngine>, limits: SynapseLimits) -> SynapseComponent {
    SynapseComponent::ready_with_engine(
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
        identity_contract_version: "search-projection-identity-v3".to_string(),
        limit_manifest_protocol_version: "limits.v1".to_string(),
        embedding_model: MODEL.to_string(),
        tokenizer_fingerprint: FINGERPRINT.to_string(),
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
    assert_eq!(
        pi_units(&pi_session(), &json!({ "type": "session", "id": "x" })).unwrap(),
        vec![]
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

/// The request digest is persisted in the descriptor receipt under a key that does not change with it, so a stored receipt replays only while the digest's byte layout is the one that wrote it. The pinned value fails this test whenever the layout moves.
#[test]
fn publication_receipt_digest_layout_is_pinned() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let unit = pi_units(&pi_session(), &pi_user("pi-u1", "hello", 100))
        .unwrap()
        .remove(0);
    corpus.publisher().publish(&unit, NOW).unwrap();
    let digest: String =
        Connection::open_with_flags(&corpus.kernel_db, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap()
            .query_row(
                "SELECT request_digest FROM operation_receipts
             WHERE producer='eidnara-daemon/harness-sources'
               AND operation_key LIKE 'source-descriptor:%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
    assert_eq!(
        digest,
        "9ff69ac08200498a7b8827c97faabbcdfaf323061593dccce0e6ea31668f3723"
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
            assert_eq!(published.provenance, *expected_provenance, "{unit:?}");
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
    let synapse = component(&engine, SynapseLimits::default());
    let project = ProjectScope::new(PROJECT).unwrap();
    let mut events = Vec::new();
    let mut dispatcher = EmbeddingDispatcher::new(&corpus.kernel, &projection, &synapse);
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
