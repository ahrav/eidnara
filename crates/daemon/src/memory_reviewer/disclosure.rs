//! One authorized prepared body reaches the model through one supervised, durably charged handoff.
//!
//! The body is assembled once from the broker's rendered, provenance-tagged buffers and the complete policy union those disclosures produced; the bytes hashed into the marker are the bytes handed to the connection. The deployment owner's startup approval must name the provider the sender dials, the model, the credential, and both live store incarnations, or the run is unavailable and no evidence leaves the host. A broker whose disclosures were judged for a local destination never sends: the local egress fold admits evidence the remote fold refuses.
//!
//! Order matters because each step's authority is a different owner's. The Kernel re-judges every disclosed reference at the current clock and validates the execution hold over the whole evidence set before the sender opens a connection, so a revoked input never produces even a handshake, and once more after the handshake, so an input revoked while it was in flight never reaches the marker. The Memory Store commits the attempt marker under the hold's own job and generation, rechecks claim, deadlines, cutoff, and cancellation, and hands the prepared bytes over exactly once inside its own connection ownership; the handoff touches no store and encodes nothing, so the connection is never held across an allocation or a wait. The network wait begins only after both stores have released and ends no later than the attempt deadline the marker committed under. A commit that fails sends nothing; a recheck that lapses leaves the charged marker and sends nothing; a provider failure ends the attempt and never sends again. Text is released only when the provider reports the requested model and the ledger records the attempt complete.
//!
//! The complete API and model profile (canonical model id, `max_tokens`, `temperature` or its omission) is bound to the marker through the body digest: the profile is serialized into the body, and the marker records that body's digest and byte length.

use std::ops::Range;
use std::time::Duration;

use kernel::{ArtifactDestination, KernelStore, MemoryReviewerHoldKind};
use memory_store::MemoryStore;
use memory_store::memory_reviewer_ledger::{
    AttemptMarker, DispatchOutcome, MemoryReviewerAttemptTerminal, MemoryReviewerLedgerError,
    MemoryReviewerLedgerRefusal, ResponseAllowance, ResponseUsage,
};
use sha2::{Digest, Sha256};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::broker::{
    BrokerId, EvidenceBroker, OriginClass, ProvenanceTag, Refusal, RefusalCode, RenderedBuffer,
    check_render, hold_refusal,
};
use super::classifier::{Answers, ClassifierRoute, ClassifyError};
use super::model_request::{
    AssistantText, Message, MessagesRequest, RequestBody, ResponseAccounting, Role,
    SIGNED_REQUEST_WINDOW_MS, SendError, Sender, Signed,
};
use super::model_response::StopReason;
use super::stage_one::{self, ChatAnswers, MAX_REQUEST_BYTES, ReviewState};

/// The provider identity an attempt marker records for a classifier request, which the harness sends under its own registry and credential.
pub const CLASSIFIER_PROVIDER: &str = "harness-classifier";

/// The complete API and model profile of one request: the requested canonical model id and the exact sampling values or their omission. It is serialized into the body before dispatch and bound to the marker through the body digest; nothing resolves an alias or asks the provider what it means.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelProfile {
    pub model: String,
    pub max_tokens: u32,
    pub temperature: Option<f64>,
}

/// The deployment owner's startup approval of provider retention and finite work exposure for one provider, model, credential, and pair of store incarnations. `provider` and `credential_id` are compared against [`Sender::provider_identity`] and [`Sender::credential_id`], so an approval for one host or credential never authorizes a sender dialing another host or presenting another credential. Account facts are inputs; nothing here infers retention guarantees. A restored store or a new incarnation carries a different pair and needs a new approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisclosureApproval {
    pub provider: String,
    pub model: String,
    pub credential_id: String,
    pub kernel_incarnation: String,
    pub memstore_incarnation: String,
}

/// The assembled request bytes with the provenance of every prompt byte in them and the policy union they disclose. Built once by [`prepare_body`]; nothing mutates it afterwards, so the digest, the size, and the bytes handed over agree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedBody {
    broker: BrokerId,
    model: String,
    tags: Vec<ProvenanceTag>,
    body: RequestBody,
    policy_union_digest: String,
    policy_union_canonical: String,
}

impl PreparedBody {
    /// The broker whose buffers this body was assembled from; only that broker's alias table names them.
    pub fn broker(&self) -> BrokerId {
        self.broker
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// One tag per buffer. `prompt_range` indexes the prompt text as the concatenation of the system text followed by the user turn; it is not a range into the serialized body.
    pub fn tags(&self) -> &[ProvenanceTag] {
        &self.tags
    }

    /// The exact serialized request bytes the marker hashes and the connection sends.
    pub fn body(&self) -> &[u8] {
        self.body.as_bytes()
    }

    pub fn body_digest(&self) -> &str {
        self.body.digest()
    }

    pub fn policy_union_digest(&self) -> &str {
        &self.policy_union_digest
    }

    pub fn policy_union_canonical(&self) -> &str {
        &self.policy_union_canonical
    }
}

/// Why a disclosure did not happen or did not yield text; host-authored, never provider text. Aliases and codes inside are the run's own; they are for the host's log, not for a client.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DisclosureRefusal {
    /// No startup approval covers this provider, model, credential, and live incarnation pair; nothing was sent.
    #[error("unavailable")]
    Unavailable,
    /// The broker judged its disclosures for a local destination; they were never admitted for a remote model.
    #[error("destination_not_remote")]
    DestinationNotRemote,
    #[error("broker_mismatch")]
    BrokerMismatch,
    /// The system buffer was not host-authored text; evidence never speaks with the host's authority.
    #[error("system_not_host_authored")]
    SystemNotHostAuthored,
    #[error("cancelled")]
    Cancelled,
    #[error("prompt_not_utf8")]
    PromptNotUtf8,
    /// The assembled prompt failed the render check: a secret or redaction marker was formed where two buffers meet, though each buffer passed alone.
    #[error("render_check")]
    RenderCheck,
    /// The broker's policy union could not be encoded; the body was not prepared.
    #[error("policy_union")]
    PolicyUnion,
    /// A disclosed reference no longer passes the Kernel's checks at the current clock.
    #[error("revalidation {0}")]
    Revalidation(Refusal),
    /// The execution hold or the held evidence set failed the Kernel guard.
    #[error("hold {0}")]
    Hold(RefusalCode),
    /// The marker did not commit; nothing was sent and nothing was charged.
    #[error("ledger {0}")]
    Ledger(MemoryReviewerLedgerRefusal),
    /// A store failed; the rendered failure is the store's own.
    #[error("store {0}")]
    Store(String),
    /// The attempt marker committed and the post-commit recheck lapsed; the attempt is charged and nothing was sent. `terminal_recorded` is false when the ledger also refused the `NotDispatched` terminal; the attempt then stays unterminated and counts as unknown.
    #[error("charged_not_dispatched {reason}")]
    ChargedNotDispatched {
        attempt_index: u32,
        reason: MemoryReviewerLedgerRefusal,
        terminal_recorded: bool,
    },
    /// The sender refused before or after the handoff; when `sent` is false no request byte left the host by the sender's account. A refusal after the marker committed ends the attempt `Failed` whatever `sent` says: `NotDispatched` is the ledger's own proof and only its dispatch path writes it.
    #[error("send {error}")]
    Send {
        attempt_index: Option<u32>,
        error: SendError,
        sent: bool,
    },
    /// The provider reported a model other than `expected`, the model [`super::model_request::Provider::expected_model`] names for the requested one, or none; the text is withheld. `reported` is the provider's model id, kept for the operator's log.
    #[error("model_mismatch")]
    ModelMismatch {
        attempt_index: u32,
        reported: Option<String>,
        expected: String,
    },
    /// The ledger did not record the attempt's terminal, so the attempt is unknown: nothing durable says whether it completed, failed, or was cancelled. Any text is withheld, and the attempt is reported unknown, because an unknown attempt outranks its provider outcome (Q20). `cause` carries the refusal that would have ended it, which the caller reads only to latch a provider rejection.
    #[error("terminal_not_recorded {error}")]
    TerminalNotRecorded {
        attempt_index: u32,
        error: String,
        cause: Option<Box<DisclosureRefusal>>,
    },
}

impl DisclosureRefusal {
    /// Whether a request byte left the host before this refusal: the request was sent and failed, another model answered it, or its terminal could not be recorded. Bytes charged for a request that never left return to the run's bound.
    pub fn left_host(&self) -> bool {
        matches!(
            self,
            Self::Send { sent: true, .. }
                | Self::ModelMismatch { .. }
                | Self::TerminalNotRecorded { .. }
        )
    }
}

/// One accepted disclosure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disclosed {
    pub attempt_index: u32,
    pub text: AssistantText,
}

/// Everything one attempt needs, borrowed for the call: the stores, the broker whose disclosures it sends, the sender, the approval, the live claim, and the ledger clock. The job, its generation, and the Kernel incarnation the ledger binds the attempt to are the broker's execution hold binding, so the marker cannot be charged under a job other than the one whose evidence it sends; the credential the marker names is the sender's own.
pub struct Disclosure<'a> {
    pub store: &'a KernelStore,
    pub ledger: &'a MemoryStore,
    /// The Memory Store project the job's attempt rows live under: the authority key, not the Kernel project digest the hold is scoped by.
    pub project: &'a str,
    pub broker: &'a EvidenceBroker,
    pub sender: &'a Sender,
    pub approval: Option<&'a DisclosureApproval>,
    /// The live claim under the hold's job and generation.
    pub claim_id: &'a str,
    pub now_ms: &'a (dyn Fn() -> i64 + Sync),
}

/// Assembles the body from `system` (which must be host-authored, or the body is refused) and the `turn` buffers, in order, recording each buffer's range in the prompt text. Every buffer keeps the tag the broker gave it; the union is the broker's complete disclosed-input union, encoded canonically. Buffers are consumed: a rendered buffer enters one body once, so the assembled evidence bytes are the bytes the broker charged.
pub fn prepare_body(
    broker: &EvidenceBroker,
    sender: &Sender,
    profile: &ModelProfile,
    system: RenderedBuffer,
    turn: Vec<RenderedBuffer>,
) -> Result<PreparedBody, DisclosureRefusal> {
    let id = broker.id();
    let mut tags = Vec::with_capacity(turn.len() + 1);
    let mut prompt_end = 0usize;
    let mut system_text = Vec::new();
    let mut content = Vec::new();
    for (index, buffer) in std::iter::once(system).chain(turn).enumerate() {
        // Aliases are broker-local, so a buffer from another broker could resolve to a different reference than the one that produced its bytes.
        if buffer.broker != id {
            return Err(DisclosureRefusal::BrokerMismatch);
        }
        // The system field carries the host's instructions; a disclosed buffer there would let evidence instruct the model.
        if index == 0 && buffer.tag.origin != OriginClass::HostAuthored {
            return Err(DisclosureRefusal::SystemNotHostAuthored);
        }
        let range: Range<usize> = prompt_end..prompt_end + buffer.bytes.len();
        prompt_end = range.end;
        tags.push(ProvenanceTag {
            prompt_range: range,
            ..buffer.tag
        });
        if index == 0 {
            system_text = buffer.bytes;
        } else {
            content.extend_from_slice(&buffer.bytes);
        }
    }
    let system_text =
        String::from_utf8(system_text).map_err(|_| DisclosureRefusal::PromptNotUtf8)?;
    let content = String::from_utf8(content).map_err(|_| DisclosureRefusal::PromptNotUtf8)?;
    // Each buffer passed the render check alone; the assembled prompt is checked whole so a secret or marker split across two buffers cannot reach the provider.
    check_render(format!("{system_text}{content}").as_bytes(), None)
        .map_err(|_| DisclosureRefusal::RenderCheck)?;
    let request = MessagesRequest {
        model: profile.model.clone(),
        system: (!system_text.is_empty()).then_some(system_text),
        messages: vec![Message {
            role: Role::User,
            content,
        }],
        max_tokens: profile.max_tokens,
        temperature: profile.temperature,
    };
    let body = sender
        .body(&request)
        .map_err(|error| DisclosureRefusal::Send {
            attempt_index: None,
            error,
            sent: false,
        })?;
    let union = broker
        .ledger
        .union()
        .encode()
        .map_err(|_| DisclosureRefusal::PolicyUnion)?;
    Ok(PreparedBody {
        broker: id,
        model: request.model,
        tags,
        body,
        policy_union_digest: union.digest,
        policy_union_canonical: union.canonical,
    })
}

/// Whether a request signed at `signed_at_ms` may be sent under a marker committed at `committed_at_ms` with `attempt_deadline_ms`: signed no later than the commit, and no longer than [`SIGNED_REQUEST_WINDOW_MS`] before the attempt deadline, so the provider never sees a request time outside its skew window.
pub fn signing_window_admits(
    signed_at_ms: i64,
    committed_at_ms: i64,
    attempt_deadline_ms: i64,
) -> bool {
    signed_at_ms <= committed_at_ms
        && attempt_deadline_ms.saturating_sub(signed_at_ms) <= SIGNED_REQUEST_WINDOW_MS
}

impl Disclosure<'_> {
    /// Runs one attempt end to end. `deadline` bounds the connection and the network wait; the ledger's own attempt deadline, cutoff, claim, and job deadline bound the commit and the handoff, and the committed attempt deadline also caps the network wait.
    pub async fn disclose(
        &self,
        prepared: &PreparedBody,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Disclosed, DisclosureRefusal> {
        if prepared.broker != self.broker.id() {
            return Err(DisclosureRefusal::BrokerMismatch);
        }
        // The broker must have judged its disclosures for the destination this sender reaches: the configured model through the host, a remote model on the sender's own connection.
        let destination = if self.sender.host_route().is_some() {
            ArtifactDestination::ConfiguredModel
        } else {
            ArtifactDestination::Remote
        };
        if self.broker.binding().destination != destination {
            return Err(DisclosureRefusal::DestinationNotRemote);
        }
        self.approved(prepared.model())?;
        if cancel.is_cancelled() {
            return Err(DisclosureRefusal::Cancelled);
        }
        let evidence = self.revalidate(prepared)?;
        self.guard(&evidence)?;
        if let Some(host) = self.sender.host_route() {
            return self
                .disclose_through_host(host, prepared, cancel, deadline)
                .await;
        }
        // The handshakes carry no request byte; a commit that fails simply drops the connection.
        let connected = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(DisclosureRefusal::Cancelled),
            connected = self.sender.connect(deadline) => connected.map_err(|error| DisclosureRefusal::Send {
                attempt_index: None,
                error,
                sent: false,
            })?,
        };
        if cancel.is_cancelled() {
            return Err(DisclosureRefusal::Cancelled);
        }
        // The handshake took real time; the Kernel is asked again so an input revoked meanwhile is refused before the marker commits.
        let evidence = self.revalidate(prepared)?;
        self.guard(&evidence)?;
        let marker = AttemptMarker {
            body_digest: prepared.body_digest().to_string(),
            request_bytes: u64::try_from(prepared.body.len()).map_err(|_| {
                DisclosureRefusal::Send {
                    attempt_index: None,
                    error: SendError::RequestTooLarge,
                    sent: false,
                }
            })?,
            provider: self.sender.provider_identity(),
            model: prepared.model.clone(),
            credential_id: self.sender.credential_id().to_string(),
            policy_union_digest: prepared.policy_union_digest.clone(),
        };
        // Signed before the ledger is entered, at the attempt's clock, so the handoff builds nothing while the connection is owned; a refused commit drops the signed request unsent.
        let signed_at_ms = (self.now_ms)();
        let signed = connected
            .sign(prepared.body.clone(), prepared.body_digest(), signed_at_ms)
            .map_err(|error| DisclosureRefusal::Send {
                attempt_index: None,
                error,
                sent: false,
            })?;
        let hold = &self.broker.binding().hold;
        let outcome = self
            .ledger
            .dispatch_memory_reviewer_attempt(
                self.project,
                &hold.subject,
                hold.generation,
                self.claim_id,
                &hold.kernel_incarnation,
                &marker,
                signed,
                self.now_ms,
                Signed::handoff,
            )
            .map_err(|error| match error {
                MemoryReviewerLedgerError::Refused(reason) => DisclosureRefusal::Ledger(reason),
                MemoryReviewerLedgerError::Store(error) => {
                    DisclosureRefusal::Store(error.to_string())
                }
            })?;
        let (attempt_index, in_flight, attempt_deadline_ms, allowance) = match outcome {
            DispatchOutcome::ChargedNotDispatched {
                attempt_index,
                reason,
                finished,
            } => {
                if !finished {
                    eprintln!(
                        "daemon: memory_reviewer disclosure terminal NotDispatched not recorded for {}/{} attempt {attempt_index}: {reason}",
                        self.project, hold.subject
                    );
                }
                return Err(DisclosureRefusal::ChargedNotDispatched {
                    attempt_index,
                    reason,
                    terminal_recorded: finished,
                });
            }
            DispatchOutcome::Handed {
                attempt_index,
                committed_at_ms,
                handoff,
                attempt_deadline_ms,
                allowance,
                release,
            } => {
                // No request byte has been written yet, so no response byte was consumed. A ledger that could not restore its view after the handoff may refuse this attempt's terminal, so the unwritten request is dropped rather than sent under a store that cannot record its outcome.
                if let Err(error) = release {
                    drop(handoff);
                    return Err(self.end(
                        attempt_index,
                        MemoryReviewerAttemptTerminal::Failed,
                        ResponseUsage::NONE,
                        DisclosureRefusal::Store(error.to_string()),
                    ));
                }
                // The request was signed at the attempt's clock: never after the commit, and never so long before the attempt deadline that the provider would judge its time stale. A request outside that window is dropped unwritten.
                if !signing_window_admits(signed_at_ms, committed_at_ms, attempt_deadline_ms) {
                    drop(handoff);
                    return Err(self.end(
                        attempt_index,
                        MemoryReviewerAttemptTerminal::Failed,
                        ResponseUsage::NONE,
                        DisclosureRefusal::Send {
                            attempt_index: Some(attempt_index),
                            error: SendError::SignTime,
                            sent: false,
                        },
                    ));
                }
                (attempt_index, handoff, attempt_deadline_ms, allowance)
            }
        };
        // The ledger bounded the attempt when it committed the marker; a response after that bound is not this attempt's. The response may consume only what the job has left of its ceilings.
        let remaining = u64::try_from(attempt_deadline_ms - (self.now_ms)()).unwrap_or(0);
        let deadline = deadline.min(Instant::now() + Duration::from_millis(remaining));
        // The caller owns the accounting so the bytes consumed before a cancellation or a refusal are still known when the terminal is written.
        let mut accounting = ResponseAccounting::default();
        let completed = tokio::select! {
            biased;
            () = cancel.cancelled() => {
                // The request may already be on the wire; the attempt is cancelled, never retried, and charged for what it read.
                return Err(self.end(attempt_index, MemoryReviewerAttemptTerminal::Cancelled, consumed(&accounting), DisclosureRefusal::Cancelled));
            }
            completed = in_flight.complete(deadline, allowance, &mut accounting) => completed,
        };
        let usage = consumed(&accounting);
        let text = match completed {
            Ok(text) => text,
            // `NotReady` means the connection took the request back unwritten. The ledger still ends the attempt `Failed`, because no-disclosure proof is the dispatch path's alone; `sent: false` carries the sender's report.
            Err(SendError::NotReady) => {
                return Err(self.end(
                    attempt_index,
                    MemoryReviewerAttemptTerminal::Failed,
                    usage,
                    DisclosureRefusal::Send {
                        attempt_index: Some(attempt_index),
                        error: SendError::NotReady,
                        sent: false,
                    },
                ));
            }
            Err(error) => {
                return Err(self.end(
                    attempt_index,
                    MemoryReviewerAttemptTerminal::Failed,
                    usage,
                    DisclosureRefusal::Send {
                        attempt_index: Some(attempt_index),
                        error,
                        sent: true,
                    },
                ));
            }
        };
        let expected = self.sender.provider().expected_model(prepared.model());
        if text.model.as_deref() != Some(expected) {
            return Err(self.end(
                attempt_index,
                MemoryReviewerAttemptTerminal::Failed,
                usage,
                DisclosureRefusal::ModelMismatch {
                    attempt_index,
                    reported: text.model,
                    expected: expected.to_string(),
                },
            ));
        }
        self.finish(
            attempt_index,
            MemoryReviewerAttemptTerminal::Complete,
            usage,
        )
        .map_err(|error| DisclosureRefusal::TerminalNotRecorded {
            attempt_index,
            error,
            cause: None,
        })?;
        Ok(Disclosed {
            attempt_index,
            text,
        })
    }

    /// One proposal attempt through the host's model execution ([`Self::exchange_through_host`]). The request names the profile's `provider/model`, which the host runs as given.
    async fn disclose_through_host(
        &self,
        host: &super::host_execution::HostExecution,
        prepared: &PreparedBody,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Disclosed, DisclosureRefusal> {
        let unsent = |error| DisclosureRefusal::Send {
            attempt_index: None,
            error,
            sent: false,
        };
        // The prepared bytes are the Messages body the marker hashes; the host takes its system text and prompt from the same bytes.
        let body: HostBody = serde_json::from_slice(prepared.body())
            .map_err(|_| unsent(SendError::RequestTooLarge))?;
        let prompt = body
            .messages
            .into_iter()
            .next()
            .map(|message| message.content)
            .ok_or_else(|| unsent(SendError::RequestTooLarge))?;
        let marker = AttemptMarker {
            body_digest: prepared.body_digest().to_string(),
            request_bytes: u64::try_from(prepared.body.len())
                .map_err(|_| unsent(SendError::RequestTooLarge))?,
            provider: self.sender.provider_identity(),
            model: prepared.model.clone(),
            credential_id: self.sender.credential_id().to_string(),
            policy_union_digest: prepared.policy_union_digest.clone(),
        };
        let (attempt_index, text) = self
            .exchange_through_host(
                host,
                &marker,
                (body.system.as_deref().unwrap_or(""), &prompt),
                body.max_tokens,
                cancel,
                deadline,
            )
            .await?;
        Ok(Disclosed {
            attempt_index,
            text,
        })
    }

    /// Commits `marker` under the claim, deadline, cutoff, and allowance checks of a direct attempt, and returns the attempt's index, its deadline, and the response allowance the ledger granted. A request is sent only once the ledger has handed the attempt over.
    fn begin_attempt(
        &self,
        marker: &AttemptMarker,
    ) -> Result<(u32, i64, ResponseAllowance), DisclosureRefusal> {
        let hold = &self.broker.binding().hold;
        let outcome = self
            .ledger
            .dispatch_memory_reviewer_attempt(
                self.project,
                &hold.subject,
                hold.generation,
                self.claim_id,
                &hold.kernel_incarnation,
                marker,
                (),
                self.now_ms,
                |()| (),
            )
            .map_err(|error| match error {
                MemoryReviewerLedgerError::Refused(reason) => DisclosureRefusal::Ledger(reason),
                MemoryReviewerLedgerError::Store(error) => {
                    DisclosureRefusal::Store(error.to_string())
                }
            })?;
        match outcome {
            DispatchOutcome::ChargedNotDispatched {
                attempt_index,
                reason,
                finished,
            } => Err(DisclosureRefusal::ChargedNotDispatched {
                attempt_index,
                reason,
                terminal_recorded: finished,
            }),
            DispatchOutcome::Handed {
                attempt_index,
                attempt_deadline_ms,
                allowance,
                release,
                ..
            } => match release {
                Ok(()) => Ok((attempt_index, attempt_deadline_ms, allowance)),
                Err(error) => Err(self.end(
                    attempt_index,
                    MemoryReviewerAttemptTerminal::Failed,
                    ResponseUsage::NONE,
                    DisclosureRefusal::Store(error.to_string()),
                )),
            },
        }
    }

    /// One request through the host's model execution under `marker`. The marker commits first ([`Self::begin_attempt`]), and the host is asked only once the ledger hands the attempt over; the wait ends at the attempt deadline the marker committed under. The answer is released only when it comes from the model the route names, and the attempt's terminal is recorded on every path past the marker. Returns the attempt's index with the answer.
    async fn exchange_through_host(
        &self,
        host: &super::host_execution::HostExecution,
        marker: &AttemptMarker,
        (system, prompt): (&str, &str),
        max_tokens: u32,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<(u32, AssistantText), DisclosureRefusal> {
        let (attempt_index, attempt_deadline_ms, allowance) = self.begin_attempt(marker)?;
        let hold = &self.broker.binding().hold;
        let remaining = u64::try_from(attempt_deadline_ms - (self.now_ms)()).unwrap_or(0);
        let deadline = deadline.min(Instant::now() + Duration::from_millis(remaining));
        let session = super::host_execution::HostExecution::child_session(
            &hold.subject,
            hold.generation,
            attempt_index,
        );
        let mut accounting = ResponseAccounting::default();
        let completed = tokio::select! {
            biased;
            () = cancel.cancelled() => {
                return Err(self.end(attempt_index, MemoryReviewerAttemptTerminal::Cancelled, consumed(&accounting), DisclosureRefusal::Cancelled));
            }
            completed = host.complete(
                &session,
                (system, prompt),
                max_tokens,
                deadline,
                allowance,
                &mut accounting,
            ) => completed,
        };
        let usage = consumed(&accounting);
        let text = match completed {
            Ok(text) => text,
            Err(failure) => {
                return Err(self.end(
                    attempt_index,
                    MemoryReviewerAttemptTerminal::Failed,
                    usage,
                    DisclosureRefusal::Send {
                        attempt_index: Some(attempt_index),
                        error: failure.error,
                        sent: failure.sent,
                    },
                ));
            }
        };
        // A response the provider stopped carries no answer and no model id, and releases nothing.
        if text.stop_reason != Some(StopReason::Refusal)
            && !host.answered_by_requested(text.model.as_deref())
        {
            return Err(self.end(
                attempt_index,
                MemoryReviewerAttemptTerminal::Failed,
                usage,
                DisclosureRefusal::ModelMismatch {
                    attempt_index,
                    reported: text.model,
                    expected: host.expected_model().to_string(),
                },
            ));
        }
        self.finish(
            attempt_index,
            MemoryReviewerAttemptTerminal::Complete,
            usage,
        )
        .map_err(|error| DisclosureRefusal::TerminalNotRecorded {
            attempt_index,
            error,
            cause: None,
        })?;
        Ok((attempt_index, text))
    }

    /// The approval must name exactly the provider this sender dials, the requested model, the credential, and both live incarnations. The hold's Memory Store incarnation is checked against the live store because the hold copied it at acquisition and a restored ledger changes it.
    fn approved(&self, model: &str) -> Result<(), DisclosureRefusal> {
        let approval = self.approval.ok_or(DisclosureRefusal::Unavailable)?;
        let hold = &self.broker.binding().hold;
        let live_memstore = self
            .ledger
            .memory_reviewer_store_incarnation()
            .map_err(|error| DisclosureRefusal::Store(error.to_string()))?;
        if hold.memstore_incarnation != live_memstore {
            return Err(DisclosureRefusal::Hold(RefusalCode::HoldInvalid));
        }
        let matches = approval.provider == self.sender.provider_identity()
            && approval.model == model
            && approval.credential_id == self.sender.credential_id()
            && approval.kernel_incarnation == hold.kernel_incarnation
            && approval.memstore_incarnation == live_memstore;
        if !matches {
            return Err(DisclosureRefusal::Unavailable);
        }
        Ok(())
    }

    /// The checks a stage-1 request passes before its marker: the broker judged its disclosures for the configured model, the hold names the live Memory Store, every alias the state quotes still stands, the hold covers their evidence, and `request` passes the size bound and the render check. Each quoted text passed the render check when the broker read it; the request is checked whole in both forms it takes, as the text that leaves the host and as the plain texts in order, which is what the decider reads once the JSON is parsed. A secret or marker formed where two fields meet stays with the host. Returns the digest of the policy union the request discloses under.
    fn admit_stage_one(
        &self,
        review: &ReviewState,
        request: &str,
        cancel: &CancellationToken,
    ) -> Result<String, DisclosureRefusal> {
        if self.broker.binding().destination != ArtifactDestination::ConfiguredModel {
            return Err(DisclosureRefusal::DestinationNotRemote);
        }
        let hold = &self.broker.binding().hold;
        let live_memstore = self
            .ledger
            .memory_reviewer_store_incarnation()
            .map_err(|error| DisclosureRefusal::Store(error.to_string()))?;
        if hold.memstore_incarnation != live_memstore {
            return Err(DisclosureRefusal::Hold(RefusalCode::HoldInvalid));
        }
        if cancel.is_cancelled() {
            return Err(DisclosureRefusal::Cancelled);
        }
        let aliases = review.aliases();
        let evidence = self.revalidate_aliases(aliases.iter().map(String::as_str).collect())?;
        self.guard(&evidence)?;
        admits_request(review, request)?;
        self.broker
            .ledger
            .union()
            .encode()
            .map(|union| union.digest)
            .map_err(|_| DisclosureRefusal::PolicyUnion)
    }

    /// One stage-1 attempt by the chat decider this disclosure's sender reaches through the host: the fixed instructions as the system text and the review state as the prompt, under an attempt marker like a proposal request (`exchange_through_host`). Returns the decider's answers, or `None` for an answer outside the closed form, with the SHA-256 of the request body. An answer the provider stopped or cut at the output cap reads as outside the closed form.
    pub async fn screen(
        &self,
        review: &ReviewState,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<(Option<ChatAnswers>, String), DisclosureRefusal> {
        let Some(host) = self.sender.host_route() else {
            return Err(DisclosureRefusal::Unavailable);
        };
        self.approved(host.model_ref())?;
        let system = stage_one::system_text();
        let prompt = review.state_json();
        let union_digest = self.admit_stage_one(review, &prompt, cancel)?;
        let unsent = |error| DisclosureRefusal::Send {
            attempt_index: None,
            error,
            sent: false,
        };
        let body = self
            .sender
            .body(&MessagesRequest {
                model: host.model_ref().to_string(),
                system: Some(system.clone()),
                messages: vec![Message {
                    role: Role::User,
                    content: prompt.clone(),
                }],
                max_tokens: stage_one::MAX_TOKENS,
                temperature: None,
            })
            .map_err(unsent)?;
        let marker = AttemptMarker {
            body_digest: body.digest().to_string(),
            request_bytes: u64::try_from(body.len())
                .map_err(|_| unsent(SendError::RequestTooLarge))?,
            provider: self.sender.provider_identity(),
            model: host.model_ref().to_string(),
            credential_id: self.sender.credential_id().to_string(),
            policy_union_digest: union_digest,
        };
        let (_, text) = self
            .exchange_through_host(
                host,
                &marker,
                (&system, &prompt),
                stage_one::MAX_TOKENS,
                cancel,
                deadline,
            )
            .await?;
        let answers = (text.stop_reason == Some(StopReason::EndTurn))
            .then(|| ChatAnswers::parse(review, &text.text))
            .flatten();
        Ok((answers, body.digest().to_string()))
    }

    /// One stage-1 attempt by the project's typed classifier, which returns the answers with the SHA-256 of the request text: the review state and its questions go to the classifier through the harness, under the checks and the marker of [`Self::screen`]. The attempt's terminal is recorded on every path past the marker.
    pub async fn classify(
        &self,
        route: &ClassifierRoute,
        review: &ReviewState,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<(Answers, String), DisclosureRefusal> {
        let request = review.request_json();
        let union_digest = self.admit_stage_one(review, &request, cancel)?;
        let body_digest = format!("{:x}", Sha256::digest(request.as_bytes()));
        let marker = AttemptMarker {
            body_digest: body_digest.clone(),
            request_bytes: u64::try_from(request.len()).map_err(|_| DisclosureRefusal::Send {
                attempt_index: None,
                error: SendError::RequestTooLarge,
                sent: false,
            })?,
            provider: CLASSIFIER_PROVIDER.to_string(),
            model: route.classifier.clone(),
            credential_id: self.sender.credential_id().to_string(),
            policy_union_digest: union_digest,
        };
        let (attempt_index, attempt_deadline_ms, _) = self.begin_attempt(&marker)?;
        let remaining = u64::try_from(attempt_deadline_ms - (self.now_ms)()).unwrap_or(0);
        let wait = deadline
            .min(Instant::now() + Duration::from_millis(remaining))
            .saturating_duration_since(Instant::now());
        let answered = route
            .exchange
            .classify(&route.project, &route.classifier, request, wait, cancel)
            .await;
        let failed = |error| DisclosureRefusal::Send {
            attempt_index: Some(attempt_index),
            error,
            sent: true,
        };
        let value = match answered {
            Ok(value) => value,
            Err(ClassifyError::Cancelled) => {
                return Err(self.end(
                    attempt_index,
                    MemoryReviewerAttemptTerminal::Cancelled,
                    ResponseUsage::NONE,
                    DisclosureRefusal::Cancelled,
                ));
            }
            Err(error) => {
                return Err(self.end(
                    attempt_index,
                    MemoryReviewerAttemptTerminal::Failed,
                    ResponseUsage::NONE,
                    failed(match error {
                        ClassifyError::Unanswered => SendError::Deadline,
                        _ => SendError::Transport,
                    }),
                ));
            }
        };
        let answer_bytes = serde_json::to_vec(&value).map_or(0, |bytes| bytes.len());
        let usage = ResponseUsage {
            raw_response_bytes: u64::try_from(answer_bytes).ok(),
            decoded_text_bytes: Some(0),
        };
        let answers = match Answers::parse(review, &value) {
            Ok(answers) => answers,
            Err(_) => {
                return Err(self.end(
                    attempt_index,
                    MemoryReviewerAttemptTerminal::Failed,
                    usage,
                    failed(SendError::Transport),
                ));
            }
        };
        self.finish(
            attempt_index,
            MemoryReviewerAttemptTerminal::Complete,
            usage,
        )
        .map_err(|error| DisclosureRefusal::TerminalNotRecorded {
            attempt_index,
            error,
            cause: None,
        })?;
        Ok((answers, body_digest))
    }

    /// Re-judges every disclosed alias at the current clock, once per alias, and collects the evidence ids they name.
    fn revalidate(&self, prepared: &PreparedBody) -> Result<Vec<String>, DisclosureRefusal> {
        let mut aliases: Vec<&str> = Vec::with_capacity(prepared.tags.len());
        for tag in &prepared.tags {
            match tag.alias.as_ref() {
                Some(alias) => aliases.push(alias.as_str()),
                None if tag.origin == OriginClass::HostAuthored => {}
                None => {
                    return Err(DisclosureRefusal::Revalidation(Refusal {
                        alias: None,
                        code: RefusalCode::UnknownAlias,
                    }));
                }
            }
        }
        self.revalidate_aliases(aliases)
    }

    fn revalidate_aliases(&self, mut aliases: Vec<&str>) -> Result<Vec<String>, DisclosureRefusal> {
        let now = (self.now_ms)();
        aliases.sort_unstable();
        aliases.dedup();
        let mut evidence = Vec::new();
        for alias in aliases {
            if let Some(id) = self
                .broker
                .revalidate(self.store, alias, now)
                .map_err(DisclosureRefusal::Revalidation)?
            {
                evidence.push(id);
            }
        }
        evidence.sort();
        evidence.dedup();
        Ok(evidence)
    }

    /// The Kernel's validation guard over the execution hold and every disclosed evidence id under it, at the current clock. An empty evidence set still validates the hold: a body of host text and staged subjects must not pass an expired or revoked hold.
    fn guard(&self, evidence: &[String]) -> Result<(), DisclosureRefusal> {
        let binding = self.broker.binding();
        self.store
            .validate_held_evidence(
                &binding.hold_id,
                MemoryReviewerHoldKind::Execution,
                &binding.hold,
                evidence,
                (self.now_ms)(),
            )
            .map(|_| ())
            .map_err(|error| DisclosureRefusal::Hold(hold_refusal(error)))
    }

    /// Records a non-complete terminal and returns the refusal that caused it. A terminal the ledger refuses leaves the attempt unknown, which outranks the cause: the caller gets `TerminalNotRecorded`, as on the complete path, and the cause is logged so it is not lost.
    fn end(
        &self,
        attempt_index: u32,
        terminal: MemoryReviewerAttemptTerminal,
        usage: ResponseUsage,
        refusal: DisclosureRefusal,
    ) -> DisclosureRefusal {
        match self.finish(attempt_index, terminal, usage) {
            Ok(()) => refusal,
            Err(error) => {
                let hold = &self.broker.binding().hold;
                eprintln!(
                    "daemon: memory_reviewer disclosure terminal {terminal:?} not recorded for {}/{} attempt {attempt_index} ({refusal}): {error}",
                    self.project, hold.subject
                );
                DisclosureRefusal::TerminalNotRecorded {
                    attempt_index,
                    error,
                    cause: Some(Box::new(refusal)),
                }
            }
        }
    }

    fn finish(
        &self,
        attempt_index: u32,
        terminal: MemoryReviewerAttemptTerminal,
        usage: ResponseUsage,
    ) -> Result<(), String> {
        let hold = &self.broker.binding().hold;
        self.ledger
            .finish_memory_reviewer_attempt(
                self.project,
                &hold.subject,
                hold.generation,
                self.claim_id,
                attempt_index,
                terminal,
                usage,
                (self.now_ms)(),
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

/// Whether a stage-1 request may leave the host: it fits [`MAX_REQUEST_BYTES`] and passes the render check whole, as the text that is sent and as the plain texts of the state in order.
fn admits_request(review: &ReviewState, request: &str) -> Result<(), DisclosureRefusal> {
    if request.len() > MAX_REQUEST_BYTES {
        return Err(DisclosureRefusal::Send {
            attempt_index: None,
            error: SendError::RequestTooLarge,
            sent: false,
        });
    }
    check_render(request.as_bytes(), None).map_err(|_| DisclosureRefusal::RenderCheck)?;
    check_render(review.plain_text().as_bytes(), None).map_err(|_| DisclosureRefusal::RenderCheck)
}

/// The fields of a prepared Messages body the host route sends: the system text, the one user message, and the output cap.
#[derive(serde::Deserialize)]
struct HostBody {
    #[serde(default)]
    system: Option<String>,
    messages: Vec<HostMessage>,
    max_tokens: u32,
}

#[derive(serde::Deserialize)]
struct HostMessage {
    content: String,
}

/// The bytes a response consumed, as the ledger records them: exact counts of what the transport delivered and the decoder measured, whether the exchange completed or stopped.
fn consumed(accounting: &ResponseAccounting) -> ResponseUsage {
    ResponseUsage {
        raw_response_bytes: Some(u64::try_from(accounting.transport_bytes).unwrap_or(u64::MAX)),
        decoded_text_bytes: Some(u64::try_from(accounting.decoded_text_bytes).unwrap_or(u64::MAX)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_reviewer::stage_one::tests::{reference, user_statement};
    use crate::memory_reviewer::stage_one::{StateReference, Writer};

    /// A stage-1 request is refused whole when any text in it holds a credential, whichever decider it is written for, and when it is larger than the bound. The state the broker's reads produce passes.
    #[test]
    fn a_stage_one_request_passes_the_render_check_and_the_size_bound() {
        let clean = user_statement(vec![reference("ref-3", Writer::User, "Deploy from main.")]);
        assert_eq!(admits_request(&clean, &clean.state_json()), Ok(()));
        assert_eq!(admits_request(&clean, &clean.request_json()), Ok(()));

        // Built from parts so the source holds no credential-shaped literal.
        let credential = format!(
            "{}-{}-{}",
            "sk", "ant", "api03-abcdefghijklmnopqrstuvwxyzABCDEFGH12345678"
        );
        let leaking = user_statement(vec![reference(
            "ref-3",
            Writer::User,
            &format!("Deploy from main with {credential}."),
        )]);
        for request in [leaking.state_json(), leaking.request_json()] {
            assert_eq!(
                admits_request(&leaking, &request),
                Err(DisclosureRefusal::RenderCheck)
            );
        }

        // A key block holds line ends, which JSON text escapes. Each form of the request is checked: the text that is sent, and the plain texts a decider reads once the JSON is parsed.
        let key = format!(
            "-----BEGIN {kind} PRIVATE KEY-----\n{body}\n-----END {kind} PRIVATE KEY-----",
            kind = "RSA",
            body = "MIIEowIBAAKCAQEAx3Jq1v0kq9bN8Yd2Hk5mT7uVwXyZaBcDeFgHiJkLmNoPqRsT\nuVwXyZaBcDeFgHiJkLmNoPqRsTuVwXyZaBcDeFgHiJkLmNoPqRsTuVwXyZ01234"
        );
        let block = user_statement(vec![reference("ref-3", Writer::User, &key)]);
        assert!(check_render(block.plain_text().as_bytes(), None).is_err());
        assert_eq!(
            admits_request(&block, &block.state_json()),
            Err(DisclosureRefusal::RenderCheck)
        );

        let large = user_statement(vec![StateReference {
            text: "release ".repeat(MAX_REQUEST_BYTES / 8 + 1),
            ..reference("ref-3", Writer::User, "")
        }]);
        assert!(matches!(
            admits_request(&large, &large.state_json()),
            Err(DisclosureRefusal::Send {
                error: SendError::RequestTooLarge,
                sent: false,
                ..
            })
        ));
    }
}
