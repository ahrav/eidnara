//! The host route of a review request: one prepared system text and prompt sent through the History Summarizer's producer factory to the host's model execution.
//!
//! The host runs the harness's child with the deployment's own credential source, so a review reaches the model the deployment already reaches, under the credentials the harness holds, whether they are keys, a profile, or a refreshed token. The request names its `provider/model` at dispatch and the host runs exactly that pair. The answer is the run's assistant text and whether the output cap cut it. Each request runs in a session of its own that is purged when the request ends, whatever its outcome.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use host_runtime::model_execution::backend::PLAIN_SYSTEM_DIRECTIVE;
use memory_store::memory_reviewer_ledger::{MAX_PROVIDER_BYTES, ResponseAllowance};
use sha2::{Digest, Sha256};
use tokio::time::Instant;

use super::broker::check_render;
use super::model_request::{AssistantText, ResponseAccounting, SendError};
use super::model_response::StopReason;
use crate::HistorySummarizerProducerFactory;
use crate::history_summarizer::HistorySummarizerProducerDriver;
use crate::history_summarizer_producer::{
    HistorySummarizerProducerError, HistorySummarizerSendOutcome,
};

/// The credential an attempt marker names for a request the host carries: the host's own credential source, held by the host and its harness child.
pub const HOST_CREDENTIAL_ID: &str = "host-model-execution";
/// The harness whose child's transcript carries no model id.
const OPENCODE_HARNESS: &str = "opencode";
/// Every child session of a review request starts with this prefix.
pub const CHILD_SESSION_PREFIX: &str = "eidnara-memory_reviewer:";
/// The longest a request waits for its session's deletion before it returns.
const DELETION_WAIT: std::time::Duration = std::time::Duration::from_secs(2);
/// The time before an attempt's deadline at which a request stops waiting for its session's deletion, so the attempt's terminal is recorded inside the deadline.
const DELETION_MARGIN: std::time::Duration = std::time::Duration::from_secs(1);
/// The sampling value a request carries. The host omits it for a model whose provider refuses a temperature.
const TEMPERATURE: f64 = 0.0;

/// One bound route's way to the model: the producer factory, the route facts a producer connects under, and the `provider/model` every request names.
pub struct HostExecution {
    factory: Arc<dyn HistorySummarizerProducerFactory>,
    project_root: PathBuf,
    harness: String,
    credential_fingerprints: BTreeMap<String, String>,
    model_ref: String,
    identity: String,
}

impl std::fmt::Debug for HostExecution {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "HostExecution({})", self.identity)
    }
}

/// The child session of one request and the producer that can delete it. The session holds the prompt and the answer, so it is deleted when the request ends, whatever ends it. The deletion runs as a task of its own: it finishes after an attempt's deadline has passed and after a cancellation dropped the request, and the attempt's terminal never waits on it past the deadline.
struct ChildSession {
    producer: Option<Box<dyn HistorySummarizerProducerDriver + Send>>,
    session: String,
}

impl ChildSession {
    fn producer(&mut self) -> &mut (dyn HistorySummarizerProducerDriver + Send) {
        self.producer
            .as_deref_mut()
            .expect("the producer is held until the session is deleted")
    }

    /// Starts the deletion and returns its task; `None` once it has started.
    fn delete(&mut self) -> Option<tokio::task::JoinHandle<()>> {
        let mut producer = self.producer.take()?;
        let session = std::mem::take(&mut self.session);
        let runtime = tokio::runtime::Handle::try_current().ok()?;
        Some(runtime.spawn(async move {
            let _ = producer.purge_session(&session).await;
        }))
    }

    /// Deletes the session and waits a short while for the deletion, always ending [`DELETION_MARGIN`] before `deadline`. The attempt's terminal is recorded after this returns and the ledger takes it only before the deadline, so a slow deletion is left to its own task.
    async fn end(mut self, deadline: Instant) {
        if let Some(deletion) = self.delete() {
            let until = (Instant::now() + DELETION_WAIT)
                .min(deadline.checked_sub(DELETION_MARGIN).unwrap_or(deadline));
            let _ = tokio::time::timeout_at(until, deletion).await;
        }
    }
}

impl Drop for ChildSession {
    fn drop(&mut self) {
        self.delete();
    }
}

/// A failed host request and whether the request reached the model runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostFailure {
    pub error: SendError,
    pub sent: bool,
}

impl HostExecution {
    /// The host route for `model_ref`, a canonical `provider/model`, under `harness`. [`SendError::Endpoint`] names a model reference outside that form or an identity longer than the attempt ledger's provider bound.
    pub fn new(
        factory: Arc<dyn HistorySummarizerProducerFactory>,
        project_root: PathBuf,
        harness: String,
        credential_fingerprints: BTreeMap<String, String>,
        model_ref: String,
    ) -> Result<Self, SendError> {
        let canonical = model_ref
            .split_once('/')
            .is_some_and(|(provider, model)| !provider.is_empty() && !model.is_empty());
        let identity = format!("host-model-execution/{harness}/{model_ref}");
        if !canonical || harness.is_empty() || identity.len() > MAX_PROVIDER_BYTES {
            return Err(SendError::Endpoint);
        }
        Ok(Self {
            factory,
            project_root,
            harness,
            credential_fingerprints,
            model_ref,
            identity,
        })
    }

    /// The provider identity an attempt marker and an approval record: the host route, the harness whose child runs the request, and the `provider/model` it names.
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// The `provider/model` every request of this route names.
    pub fn model_ref(&self) -> &str {
        &self.model_ref
    }

    /// The model id an answer must report for its text to be released: the `model` of the route's `provider/model`.
    pub fn expected_model(&self) -> &str {
        self.model_ref
            .split_once('/')
            .map_or(self.model_ref.as_str(), |(_, model)| model)
    }

    /// Whether an answer that reports `reported` came from the model the route names. An OpenCode child's transcript carries no model id; there the dispatch, which names the pair, is the binding. On every other harness an answer without a model id is held.
    pub fn answered_by_requested(&self, reported: Option<&str>) -> bool {
        match reported {
            Some(model) => model == self.expected_model(),
            None => self.harness == OPENCODE_HARNESS,
        }
    }

    /// The child session of one attempt: a digest of the job, its receipt generation, and the attempt index, so each attempt runs in a session of its own.
    pub fn child_session(job: &str, generation: u64, attempt_index: u32) -> String {
        let mut hasher = Sha256::new();
        hasher.update(job.as_bytes());
        hasher.update([0]);
        hasher.update(generation.to_le_bytes());
        hasher.update([0]);
        hasher.update(attempt_index.to_le_bytes());
        let digest = format!("{:x}", hasher.finalize());
        format!("{CHILD_SESSION_PREFIX}{}", &digest[..16])
    }

    /// Runs one request to its text. The wait ends at `deadline`; the text is charged against `allowance` and passes the render check before it is released, and `accounting` records what was read whatever the outcome.
    pub async fn complete(
        &self,
        session: &str,
        (system, prompt): (&str, &str),
        max_tokens: u32,
        deadline: Instant,
        allowance: ResponseAllowance,
        accounting: &mut ResponseAccounting,
    ) -> Result<AssistantText, HostFailure> {
        let unsent = |error| HostFailure { error, sent: false };
        let remaining = || deadline.saturating_duration_since(Instant::now());
        if remaining().is_zero() {
            return Err(unsent(SendError::Deadline));
        }
        let connect = self.factory.connect(
            &self.project_root,
            &self.harness,
            &self.credential_fingerprints,
        );
        let mut child = match tokio::time::timeout(remaining(), connect).await {
            Ok(Ok(producer)) => ChildSession {
                producer: Some(producer),
                session: session.to_string(),
            },
            Ok(Err(_)) => return Err(unsent(SendError::Connect)),
            Err(_) => return Err(unsent(SendError::Deadline)),
        };
        // A review's references carry their own times, and the labels state them; the harness's line about today's date would sit beside them as a second clock. The directive asks the harness for the system text as written.
        let system = if system.is_empty() {
            String::new()
        } else {
            format!("{PLAIN_SYSTEM_DIRECTIVE}{system}")
        };
        let started = tokio::time::timeout(
            remaining(),
            child.producer().start_with_generation(
                session,
                &system,
                prompt,
                &self.model_ref,
                max_tokens,
                TEMPERATURE,
            ),
        )
        .await;
        let handle = match started {
            Ok(Ok(handle)) => handle,
            Ok(Err(error)) => {
                let sent = error.send_outcome() != Some(HistorySummarizerSendOutcome::NotSent);
                child.end(deadline).await;
                return Err(HostFailure {
                    error: if sent {
                        SendError::Transport
                    } else {
                        SendError::NotReady
                    },
                    sent,
                });
            }
            Err(_) => {
                child.end(deadline).await;
                return Err(HostFailure {
                    error: SendError::Deadline,
                    sent: true,
                });
            }
        };
        let output = child
            .producer()
            .await_output_with_timeout(&handle.run_id, remaining())
            .await;
        child.end(deadline).await;
        let sent = |error| HostFailure { error, sent: true };
        // The provider ended the response itself without an answer, as a content filter does. The same request meets the same stop again, so the attempt reads as the model declining, which settles the review on this request.
        if let Err(HistorySummarizerProducerError::RunFailed { detail, .. }) = &output
            && host_runtime::model_execution::backend::is_provider_stop(detail)
        {
            return Ok(AssistantText {
                text: String::new(),
                stop_reason: Some(StopReason::Refusal),
                model: None,
                accounting: *accounting,
            });
        }
        let output = output.map_err(|error| {
            sent(match error {
                HistorySummarizerProducerError::TimedOut => SendError::Deadline,
                _ => SendError::Transport,
            })
        })?;
        let bytes = output.text.len();
        let over = |limit: u64| u64::try_from(bytes).unwrap_or(u64::MAX) > limit;
        if over(allowance.raw_response_bytes) || over(allowance.decoded_text_bytes) {
            // A response past the job's remainder spends the whole remainder, as the direct sender's collector records it.
            accounting.transport_bytes =
                usize::try_from(allowance.raw_response_bytes).unwrap_or(usize::MAX);
            accounting.decoded_text_bytes =
                usize::try_from(allowance.decoded_text_bytes).unwrap_or(usize::MAX);
            return Err(sent(SendError::ResponseTooLarge));
        }
        accounting.transport_bytes = bytes;
        accounting.decoded_text_bytes = bytes;
        // An answer the output cap cut before any visible text is a cut answer, which the caller reads by its stop reason. Any other empty answer is a failed request.
        if output.text.trim().is_empty() && !output.length_capped {
            return Err(sent(SendError::Transport));
        }
        check_render(output.text.as_bytes(), None).map_err(|_| sent(SendError::EgressCheck))?;
        Ok(AssistantText {
            text: output.text,
            stop_reason: Some(if output.length_capped {
                StopReason::MaxTokens
            } else {
                StopReason::EndTurn
            }),
            model: output.model,
            accounting: *accounting,
        })
    }
}
