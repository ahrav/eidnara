//! The evaluator's `LlmExecutionBackend` cassette and the MemoryReviewer keyed
//! peer. Both key on production-owned values: the pinned `BackendRequest`
//! covered fields, and the reviewer's attempt-marker tuple (body digest,
//! provider identity, model, credential id).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use eval_core::{
    BackendRecord, Boundary, Cassette, CassetteError, CassetteMiss, Lookup, canonical_decimal_f64,
};
use host_runtime::CancellationToken;
use host_runtime::model_execution::backend::{
    BackendError, BackendEvent, BackendFuture, BackendRequest, BackendTerminal,
    ContextCapabilities, ErrorClass, EventSink, FinishReason, Harness, LlmExecutionBackend,
    SinkStatus,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::tls_peer::{Observed, Peer, json_response};

/// What a backend declares per harness, read once from the real backend at
/// recording and answered verbatim on replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Declared {
    pub unavailable_reason: Option<String>,
    pub suppression: bool,
    pub replacement: bool,
    pub cross_step_reuse: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Declarations {
    pub opencode: Declared,
    pub pi: Declared,
}

impl Declarations {
    pub fn of(backend: &dyn LlmExecutionBackend) -> Self {
        let read = |harness| {
            let capabilities = backend.context_capabilities(harness);
            Declared {
                unavailable_reason: backend.unavailable_reason(harness).map(str::to_string),
                suppression: capabilities.suppression,
                replacement: capabilities.replacement,
                cross_step_reuse: capabilities.cross_step_reuse,
            }
        };
        Self {
            opencode: read(Harness::OpenCode),
            pi: read(Harness::Pi),
        }
    }

    fn for_harness(&self, harness: Harness) -> &Declared {
        match harness {
            Harness::OpenCode => &self.opencode,
            Harness::Pi => &self.pi,
        }
    }
}

/// The covered projection of a request. Destructuring pins the field set:
/// a new `BackendRequest` field fails to compile here until it is classified.
pub fn record_of(request: &BackendRequest) -> Result<BackendRecord, CassetteError> {
    let BackendRequest {
        prompt,
        system,
        provider,
        model,
        max_output_tokens,
        temperature,
        harness,
        session: _,
        run_id: _,
    } = request;
    Ok(BackendRecord {
        prompt: prompt.clone(),
        system: system.clone(),
        provider: provider.clone(),
        model: model.clone(),
        max_output_tokens: *max_output_tokens,
        temperature: temperature.map(canonical_decimal_f64).transpose()?,
        harness: harness.as_str().to_string(),
    })
}

/// The recorded shape of one backend exchange. The host types carry no serde
/// (their `Debug` redacts), so this mirror is the wire form; a new host
/// variant fails to compile in the conversions below until it is mirrored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum WireEvent {
    HarnessDispatch {
        harness: String,
    },
    AssistantText {
        text: String,
        finish_reason: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireError {
    class: String,
    message: String,
    retry_after_secs: Option<u64>,
    provider_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum WireTerminal {
    Completed { finish_reason: String },
    Failed(WireError),
    FailedUnresolved(WireError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireExchange {
    events: Vec<WireEvent>,
    terminal: WireTerminal,
}

impl From<&BackendEvent> for WireEvent {
    fn from(event: &BackendEvent) -> Self {
        match event {
            BackendEvent::HarnessDispatch { harness } => Self::HarnessDispatch {
                harness: harness.as_str().to_string(),
            },
            BackendEvent::AssistantText {
                text,
                finish_reason,
            } => Self::AssistantText {
                text: text.clone(),
                finish_reason: finish_reason.map(|reason| reason.as_wire_str().to_string()),
            },
        }
    }
}

impl From<&BackendError> for WireError {
    fn from(error: &BackendError) -> Self {
        Self {
            class: error.class.as_wire_str().to_string(),
            message: error.message.clone(),
            retry_after_secs: error.retry_after_secs,
            provider_code: error.provider_code.clone(),
        }
    }
}

impl From<&BackendTerminal> for WireTerminal {
    fn from(terminal: &BackendTerminal) -> Self {
        match terminal {
            BackendTerminal::Completed { finish_reason } => Self::Completed {
                finish_reason: finish_reason.as_wire_str().to_string(),
            },
            BackendTerminal::Failed(error) => Self::Failed(error.into()),
            BackendTerminal::FailedUnresolved(error) => Self::FailedUnresolved(error.into()),
        }
    }
}

/// Every `FinishReason`; the `match` fails to compile when the host adds a
/// variant, so the decode list cannot drift behind `as_wire_str`.
pub fn finish_reasons() -> [FinishReason; 2] {
    [FinishReason::Completed, FinishReason::Length].map(|reason| match reason {
        FinishReason::Completed | FinishReason::Length => reason,
    })
}

/// Every `ErrorClass`, guarded the same way as [`finish_reasons`].
pub fn error_classes() -> [ErrorClass; 4] {
    [
        ErrorClass::Transient,
        ErrorClass::Permanent,
        ErrorClass::AuthRequired,
        ErrorClass::ContextOverflow,
    ]
    .map(|class| match class {
        ErrorClass::Transient
        | ErrorClass::Permanent
        | ErrorClass::AuthRequired
        | ErrorClass::ContextOverflow => class,
    })
}

fn finish_reason(text: &str) -> Option<FinishReason> {
    finish_reasons()
        .into_iter()
        .find(|reason| reason.as_wire_str() == text)
}

impl TryFrom<WireEvent> for BackendEvent {
    type Error = String;

    fn try_from(event: WireEvent) -> Result<Self, String> {
        Ok(match event {
            WireEvent::HarnessDispatch { harness } => Self::HarnessDispatch {
                harness: Harness::parse(&harness).ok_or(harness)?,
            },
            WireEvent::AssistantText {
                text,
                finish_reason: reason,
            } => Self::AssistantText {
                text,
                finish_reason: match reason {
                    Some(reason) => Some(finish_reason(&reason).ok_or(reason)?),
                    None => None,
                },
            },
        })
    }
}

impl TryFrom<WireError> for BackendError {
    type Error = String;

    fn try_from(error: WireError) -> Result<Self, String> {
        let class = error_classes()
            .into_iter()
            .find(|class| class.as_wire_str() == error.class)
            .ok_or(error.class)?;
        Ok(Self {
            class,
            message: error.message,
            retry_after_secs: error.retry_after_secs,
            provider_code: error.provider_code,
        })
    }
}

impl TryFrom<WireTerminal> for BackendTerminal {
    type Error = String;

    fn try_from(terminal: WireTerminal) -> Result<Self, String> {
        Ok(match terminal {
            WireTerminal::Completed {
                finish_reason: reason,
            } => Self::Completed {
                finish_reason: finish_reason(&reason).ok_or(reason)?,
            },
            WireTerminal::Failed(error) => Self::Failed(error.try_into()?),
            WireTerminal::FailedUnresolved(error) => Self::FailedUnresolved(error.try_into()?),
        })
    }
}

/// Records through `inner` or replays from a loaded cassette. Every trait
/// method answers from the recorded declarations, so a host that latches
/// capabilities from this backend reads what the real backend declared.
pub struct CassetteBackend {
    namespace: String,
    recording: Arc<Mutex<Recording>>,
    inner: Option<Arc<dyn LlmExecutionBackend>>,
    declarations: Declarations,
    /// The declared reasons as `'static` strings, leaked once here so the
    /// trait's `&'static str` return needs no per-call allocation.
    reasons: [Option<&'static str>; 2],
    refusals: Arc<AtomicUsize>,
}

/// The cassette with its exchanges still between `execute` and `record`,
/// under one lock so `file()` never observes a count and a cassette state
/// from different moments.
struct Recording {
    cassette: Cassette,
    in_flight: usize,
}

impl CassetteBackend {
    pub fn recording(namespace: &str, inner: Arc<dyn LlmExecutionBackend>) -> Arc<Self> {
        let declarations = Declarations::of(inner.as_ref());
        let cassette =
            Cassette::recording(namespace, serde_json::to_value(&declarations).unwrap()).unwrap();
        Self::assemble(namespace, cassette, Some(inner), declarations)
    }

    pub fn replaying(namespace: &str, file: &Value) -> Result<Arc<Self>, CassetteError> {
        let cassette = Cassette::replay(file, namespace)?;
        let declarations = serde_json::from_value(cassette.declarations().clone())
            .map_err(|error| CassetteError::Shape(error.to_string()))?;
        Ok(Self::assemble(namespace, cassette, None, declarations))
    }

    fn assemble(
        namespace: &str,
        cassette: Cassette,
        inner: Option<Arc<dyn LlmExecutionBackend>>,
        declarations: Declarations,
    ) -> Arc<Self> {
        let leak = |reason: &Option<String>| {
            reason
                .as_deref()
                .map(|reason| &*Box::leak(reason.to_string().into_boxed_str()))
        };
        let reasons = [
            leak(&declarations.opencode.unavailable_reason),
            leak(&declarations.pi.unavailable_reason),
        ];
        Arc::new(Self {
            namespace: namespace.to_string(),
            recording: Arc::new(Mutex::new(Recording {
                cassette,
                in_flight: 0,
            })),
            inner,
            declarations,
            reasons,
            refusals: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// The persisted cassette after recording; a refused recording has none,
    /// and a recording with an exchange still in flight has none yet.
    pub fn file(&self) -> Result<Value, CassetteError> {
        let recording = self.recording.lock().unwrap();
        if recording.in_flight > 0 {
            return Err(CassetteError::IncompleteExchange);
        }
        Ok(serde_json::to_value(recording.cassette.to_file()?).unwrap())
    }

    /// Requests answered with a `cassette_miss` terminal, including every one
    /// refused after the first miss latched.
    pub fn refusals(&self) -> usize {
        self.refusals.load(Ordering::SeqCst)
    }

    pub fn terminal(&self) -> Option<CassetteMiss> {
        self.recording.lock().unwrap().cassette.terminal().cloned()
    }

    /// Recorded entries no request has consumed; a faithful replay leaves none.
    pub fn unconsumed(&self) -> usize {
        self.recording.lock().unwrap().cassette.unconsumed()
    }

    fn refused(code: &str, detail: impl std::fmt::Display) -> BackendTerminal {
        BackendTerminal::Failed(BackendError {
            class: ErrorClass::Permanent,
            message: format!("{code}: {detail}"),
            retry_after_secs: None,
            provider_code: Some(code.to_string()),
        })
    }

    fn record_through(
        &self,
        inner: &Arc<dyn LlmExecutionBackend>,
        covered: Value,
        request: BackendRequest,
        events: EventSink,
        cancel: CancellationToken,
    ) -> BackendFuture {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let closed = Arc::new(AtomicBool::new(false));
        let tee = {
            let seen = seen.clone();
            let closed = closed.clone();
            EventSink::new(Arc::new(move |event: BackendEvent| {
                // Forward and record under one lock, so concurrent emitters
                // record in the order the run's sink accepted.
                let mut seen = seen.lock().unwrap();
                let status = events.emit(event.clone());
                match status {
                    SinkStatus::Accepted => seen.push(WireEvent::from(&event)),
                    SinkStatus::Closed => closed.store(true, Ordering::SeqCst),
                }
                status
            }))
        };
        let recording = self.recording.clone();
        let namespace = self.namespace.clone();
        // Armed before the wrapped backend runs, so an `execute` that panics
        // and a future dropped unpolled both refuse the recording.
        let mut exchange = InFlight::new(recording.clone());
        let inner = inner.execute(request, tee, cancel.clone());
        Box::pin(async move {
            let terminal = inner.await;
            let mut recording = recording.lock().unwrap();
            exchange.commit(&mut recording);
            if closed.load(Ordering::SeqCst) || cancel.is_cancelled() {
                // The run's terminal is the supervisor's (a refused event, a
                // cancellation), not this one, and a refused event is missing:
                // the exchange cannot replay the run.
                let error = recording.cassette.refuse(CassetteError::IncompleteExchange);
                return Self::refused("cassette_refused", error);
            }
            let exchange = WireExchange {
                events: seen.lock().unwrap().clone(),
                terminal: WireTerminal::from(&terminal),
            };
            let response = serde_json::to_value(exchange).unwrap();
            match recording
                .cassette
                .record(&namespace, Boundary::Backend, covered, response)
            {
                Ok(_) => terminal,
                Err(error) => Self::refused("redaction_refused", error),
            }
        })
    }

    fn replay_from(
        &self,
        covered: Value,
        events: EventSink,
        cancel: CancellationToken,
    ) -> BackendFuture {
        let recording = self.recording.clone();
        let namespace = self.namespace.clone();
        let refusals = self.refusals.clone();
        Box::pin(async move {
            if cancel.is_cancelled() {
                // The run recorded only its cancellation; no exchange was seen,
                // so none is consumed.
                return Self::refused("cassette_refused", "run cancelled before lookup");
            }
            let outcome = recording
                .lock()
                .unwrap()
                .cassette
                .lookup(&namespace, Boundary::Backend, &covered)
                .map(|found| match found {
                    Lookup::Hit(entry) => Ok(entry.response.clone()),
                    Lookup::Miss(miss) => Err(miss),
                });
            let response = match outcome {
                Ok(Ok(response)) => response,
                Ok(Err(miss)) => {
                    refusals.fetch_add(1, Ordering::SeqCst);
                    return Self::refused(
                        "cassette_miss",
                        format!(
                            "turn {} {:?} nearest={}",
                            miss.turn,
                            miss.class,
                            miss.nearest_recorded.as_deref().unwrap_or("none")
                        ),
                    );
                }
                Err(error) => return Self::refused("cassette_refused", error),
            };
            let exchange: WireExchange = match serde_json::from_value(response) {
                Ok(exchange) => exchange,
                Err(error) => return Self::refused("cassette_refused", error),
            };
            for event in exchange.events {
                let event = match BackendEvent::try_from(event) {
                    Ok(event) => event,
                    Err(unknown) => return Self::refused("cassette_refused", unknown),
                };
                if events.emit(event) == SinkStatus::Closed {
                    break;
                }
            }
            match BackendTerminal::try_from(exchange.terminal) {
                Ok(terminal) => terminal,
                Err(unknown) => Self::refused("cassette_refused", unknown),
            }
        })
    }
}

/// One exchange between `execute` and `record`, counted in
/// `Recording::in_flight`. Dropped before `commit` (a panic, a dropped
/// future), it refuses the recording: the exchange it stands for is in no
/// file. Every count change happens under the recording's lock.
struct InFlight {
    recording: Arc<Mutex<Recording>>,
    committed: bool,
}

impl InFlight {
    fn new(recording: Arc<Mutex<Recording>>) -> Self {
        recording.lock().unwrap().in_flight += 1;
        Self {
            recording,
            committed: false,
        }
    }

    /// Takes the caller's lock, so `file()` never sees the count drop before
    /// the entry is recorded under the same lock.
    fn commit(&mut self, recording: &mut Recording) {
        self.committed = true;
        recording.in_flight -= 1;
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        if !self.committed
            && let Ok(mut recording) = self.recording.lock()
        {
            recording.in_flight -= 1;
            recording.cassette.refuse(CassetteError::IncompleteExchange);
        }
    }
}

impl LlmExecutionBackend for CassetteBackend {
    fn execute(
        &self,
        request: BackendRequest,
        events: EventSink,
        cancel: CancellationToken,
    ) -> BackendFuture {
        let covered = match record_of(&request).and_then(|record| record.covered()) {
            Ok(covered) => covered,
            Err(error) => {
                // The exchange this request stands for can be in no file.
                let error = self.recording.lock().unwrap().cassette.refuse(error);
                return Box::pin(async move { Self::refused("cassette_request", error) });
            }
        };
        match &self.inner {
            Some(inner) => self.record_through(inner, covered, request, events, cancel),
            None => self.replay_from(covered, events, cancel),
        }
    }

    fn unavailable_reason(&self, harness: Harness) -> Option<&'static str> {
        self.reasons[match harness {
            Harness::OpenCode => 0,
            Harness::Pi => 1,
        }]
    }

    fn context_capabilities(&self, harness: Harness) -> ContextCapabilities {
        let declared = self.declarations.for_harness(harness);
        ContextCapabilities {
            suppression: declared.suppression,
            replacement: declared.replacement,
            cross_step_reuse: declared.cross_step_reuse,
        }
    }
}

/// The reviewer's attempt-marker tuple as the peer can recover it from one
/// request: `provider` is `{host}/v1/messages@{anthropic-version}` from the
/// request head, `model` from the body, `body_digest` over the body bytes, and
/// `credential_id` from the peer's configuration because the header carries
/// only the secret.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReviewerKey {
    pub body_digest: String,
    pub provider: String,
    pub model: String,
    pub credential_id: String,
}

impl ReviewerKey {
    pub fn of(observed: &Observed, credential_id: &str) -> Self {
        let header = |name: &str| {
            observed
                .head
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case(name)
                        .then(|| value.trim().to_string())
                })
                .unwrap_or_default()
        };
        let model = serde_json::from_slice::<Value>(&observed.body)
            .ok()
            .and_then(|value| value["model"].as_str().map(str::to_string))
            .unwrap_or_default();
        Self {
            body_digest: format!("{:x}", Sha256::digest(&observed.body)),
            provider: format!(
                "{}/v1/messages@{}",
                header("host"),
                header("anthropic-version")
            ),
            model,
            credential_id: credential_id.to_string(),
        }
    }
}

/// Serves `turns` reviewer connections strictly from `entries`, each entry
/// answering one request; equal keys answer in recorded order, as equal
/// digests do in the core. A request whose key has no unconsumed entry is
/// answered with a typed `cassette_miss` refusal, and every later request is
/// refused too, so the run stops at the first miss as it does at the other two
/// boundaries.
pub fn serve_keyed(
    peer: &mut Peer,
    turns: usize,
    mut entries: Vec<(ReviewerKey, Vec<u8>)>,
    credential_id: &str,
) -> tokio::task::JoinHandle<Vec<Observed>> {
    let credential_id = credential_id.to_string();
    let mut missed = false;
    peer.serve_each(turns, move |request| {
        let key = ReviewerKey::of(request, &credential_id);
        let recorded = entries.iter().position(|(recorded, _)| *recorded == key);
        match recorded {
            Some(index) if !missed => entries.remove(index).1,
            _ => {
                missed = true;
                let body = json!({"type": "error", "error": {"type": "cassette_miss", "body_digest": key.body_digest}});
                json_response("409 Conflict", &body.to_string(), "")
            }
        }
    })
}
