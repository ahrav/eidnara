//! The host route of a review request: one prepared system text and prompt sent through the History Summarizer's producer factory to the host's model execution.
//!
//! The host runs the harness's child with the deployment's own credential source, so a review reaches the model the deployment already reaches, under the credentials the harness holds, whether they are keys, a profile, or a refreshed token. The request names its `provider/model` at dispatch and the host runs exactly that pair. The answer is the run's assistant text and whether the output cap cut it. Each request runs in a session of its own that is purged when the request ends, whatever its outcome.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use memory_store::memory_reviewer_ledger::{MAX_PROVIDER_BYTES, ResponseAllowance};
use sha2::{Digest, Sha256};
use tokio::time::Instant;

use super::broker::check_render;
use super::model_request::{AssistantText, ResponseAccounting, SendError};
use super::model_response::StopReason;
use crate::HistorySummarizerProducerFactory;
use crate::history_summarizer_producer::{
    HistorySummarizerProducerError, HistorySummarizerSendOutcome,
};

/// The credential an attempt marker names for a request the host carries: the host's own credential source, held by the host and its harness child.
pub const HOST_CREDENTIAL_ID: &str = "host-model-execution";
/// Every child session of a review request starts with this prefix.
pub const CHILD_SESSION_PREFIX: &str = "eidnara-memory_reviewer:";
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
        let mut producer = match tokio::time::timeout(remaining(), connect).await {
            Ok(Ok(producer)) => producer,
            Ok(Err(_)) => return Err(unsent(SendError::Connect)),
            Err(_) => return Err(unsent(SendError::Deadline)),
        };
        let started = tokio::time::timeout(
            remaining(),
            producer.start_with_generation(
                session,
                system,
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
                let _ = producer.purge_session(session).await;
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
                let _ = producer.purge_session(session).await;
                return Err(HostFailure {
                    error: SendError::Deadline,
                    sent: true,
                });
            }
        };
        let output = producer
            .await_output_with_timeout(&handle.run_id, remaining())
            .await;
        // The run's session holds the prompt and the answer; it is deleted on every path.
        let _ = producer.purge_session(session).await;
        let sent = |error| HostFailure { error, sent: true };
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
        if output.text.trim().is_empty() {
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
            model: None,
            accounting: *accounting,
        })
    }
}
