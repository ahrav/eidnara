//! The evaluator's `LlmExecutionBackend` cassette, keyed on the pinned
//! `BackendRequest` covered fields. The direct-host fixture includes this file
//! by path so its model backend can record to, or replay from, the same
//! cassette the tests read.

use std::sync::atomic::{AtomicUsize, Ordering};
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
use serde_json::Value;

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

fn finish_reason(text: &str) -> Option<FinishReason> {
    [FinishReason::Completed, FinishReason::Length]
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
        let class = [
            ErrorClass::Transient,
            ErrorClass::Permanent,
            ErrorClass::AuthRequired,
            ErrorClass::ContextOverflow,
        ]
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
    cassette: Arc<Mutex<Cassette>>,
    inner: Option<Arc<dyn LlmExecutionBackend>>,
    declarations: Declarations,
    /// The declared reasons as `'static` strings, leaked once here so the
    /// trait's `&'static str` return needs no per-call allocation.
    reasons: [Option<&'static str>; 2],
    refusals: Arc<AtomicUsize>,
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
            cassette: Arc::new(Mutex::new(cassette)),
            inner,
            declarations,
            reasons,
            refusals: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// The persisted cassette after recording; a refused recording has none.
    pub fn file(&self) -> Result<Value, CassetteError> {
        Ok(serde_json::to_value(self.cassette.lock().unwrap().to_file()?).unwrap())
    }

    /// Requests answered with a `cassette_miss` terminal, including every one
    /// refused after the first miss latched.
    pub fn refusals(&self) -> usize {
        self.refusals.load(Ordering::SeqCst)
    }

    pub fn terminal(&self) -> Option<CassetteMiss> {
        self.cassette.lock().unwrap().terminal().cloned()
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
        let tee = {
            let seen = seen.clone();
            EventSink::new(Arc::new(move |event: BackendEvent| {
                let status = events.emit(event.clone());
                if status == SinkStatus::Accepted {
                    seen.lock().unwrap().push(WireEvent::from(&event));
                }
                status
            }))
        };
        let inner = inner.execute(request, tee, cancel);
        let cassette = self.cassette.clone();
        let namespace = self.namespace.clone();
        Box::pin(async move {
            let terminal = inner.await;
            let exchange = WireExchange {
                events: seen.lock().unwrap().clone(),
                terminal: WireTerminal::from(&terminal),
            };
            let response = serde_json::to_value(exchange).unwrap();
            match cassette
                .lock()
                .unwrap()
                .record(&namespace, Boundary::Backend, covered, response)
            {
                Ok(_) => terminal,
                Err(error) => Self::refused("redaction_refused", error),
            }
        })
    }

    fn replay_from(&self, covered: Value, events: EventSink) -> BackendFuture {
        let cassette = self.cassette.clone();
        let namespace = self.namespace.clone();
        let refusals = self.refusals.clone();
        Box::pin(async move {
            let outcome = cassette
                .lock()
                .unwrap()
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

impl LlmExecutionBackend for CassetteBackend {
    fn execute(
        &self,
        request: BackendRequest,
        events: EventSink,
        cancel: CancellationToken,
    ) -> BackendFuture {
        let covered = match record_of(&request).and_then(|record| record.covered()) {
            Ok(covered) => covered,
            Err(error) => return Box::pin(async move { Self::refused("cassette_request", error) }),
        };
        match &self.inner {
            Some(inner) => self.record_through(inner, covered, request, events, cancel),
            None => self.replay_from(covered, events),
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
