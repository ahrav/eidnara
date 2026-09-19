//! One authorized prepared body reaches the model through one supervised, durably charged handoff.
//!
//! The body is assembled once from the broker's rendered, provenance-tagged buffers and the complete policy union those disclosures produced; the bytes hashed into the marker are the bytes handed to the connection. The deployment owner's startup approval must name the provider the sender dials, the model, the credential, and both live store incarnations, or the run is unavailable and no evidence leaves the host. A broker whose disclosures were judged for a local destination never sends: the local egress fold admits evidence the remote fold refuses.
//!
//! Order matters because each step's authority is a different owner's. The Kernel re-judges every disclosed reference at the current clock and validates the execution hold over the whole evidence set before the sender opens a connection, so a revoked input never produces even a handshake, and once more after the handshake, so an input revoked while it was in flight never reaches the marker. The Memory Store commits the attempt marker under the hold's own job and generation, rechecks claim, deadlines, cutoff, and cancellation, and hands the prepared bytes over exactly once inside its own connection ownership; the handoff touches no store and encodes nothing, so the connection is never held across an allocation or a wait. The network wait begins only after both stores have released and ends no later than the attempt deadline the marker committed under. A commit that fails sends nothing; a recheck that lapses leaves the charged marker and sends nothing; a provider failure ends the attempt and never sends again. Text is released only when the provider reports the requested model and the ledger records the attempt complete.
//!
//! The complete API and model profile (canonical model id, `max_tokens`, `temperature` or its omission) is bound to the marker through the body digest: the profile is serialized into the body, and the marker records that body's digest and byte length.

use std::ops::Range;
use std::time::Duration;

use kernel::{ArtifactDestination, CuratorHoldKind, KernelStore};
use memory_store::MemoryStore;
use memory_store::curator_ledger::{
    AttemptMarker, CuratorAttemptTerminal, CuratorLedgerError, CuratorLedgerRefusal,
    DispatchOutcome,
};
use sha2::{Digest, Sha256};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::broker::{
    BrokerId, EvidenceBroker, OriginClass, ProvenanceTag, Refusal, RefusalCode, RenderedBuffer,
    check_render, hold_refusal,
};
use super::model_request::{
    AssistantText, Message, MessagesRequest, RequestBody, Role, SendError, Sender,
};

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
    body_digest: String,
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
        &self.body_digest
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
    Ledger(CuratorLedgerRefusal),
    /// A store failed; the rendered failure is the store's own.
    #[error("store {0}")]
    Store(String),
    /// The attempt marker committed and the post-commit recheck lapsed; the attempt is charged and nothing was sent. `terminal_recorded` is false when the ledger also refused the `NotDispatched` terminal; the attempt then stays unterminated and counts as unknown.
    #[error("charged_not_dispatched {reason}")]
    ChargedNotDispatched {
        attempt_index: u32,
        reason: CuratorLedgerRefusal,
        terminal_recorded: bool,
    },
    /// The sender refused before or after the handoff; when `sent` is false no request byte left the host by the sender's account. A refusal after the marker committed ends the attempt `Failed` whatever `sent` says: `NotDispatched` is the ledger's own proof and only its dispatch path writes it.
    #[error("send {error}")]
    Send {
        attempt_index: Option<u32>,
        error: SendError,
        sent: bool,
    },
    /// The provider answered under another model than the one requested, or none; the text is withheld.
    #[error("model_mismatch")]
    ModelMismatch { attempt_index: u32 },
    /// The ledger did not record the attempt's terminal, so the attempt is unknown: nothing durable says whether it completed, failed, or was cancelled. Any text is withheld, and the cause that would have ended the attempt is logged rather than returned, because an unknown attempt outranks its provider outcome (Q20).
    #[error("terminal_not_recorded {error}")]
    TerminalNotRecorded { attempt_index: u32, error: String },
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
    let body = request.body().map_err(|error| DisclosureRefusal::Send {
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
        body_digest: format!("{:x}", Sha256::digest(body.as_bytes())),
        tags,
        body,
        policy_union_digest: union.digest,
        policy_union_canonical: union.canonical,
    })
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
        if self.broker.binding().destination != ArtifactDestination::Remote {
            return Err(DisclosureRefusal::DestinationNotRemote);
        }
        self.approved(prepared.model())?;
        if cancel.is_cancelled() {
            return Err(DisclosureRefusal::Cancelled);
        }
        let evidence = self.revalidate(prepared)?;
        self.guard(&evidence)?;
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
            body_digest: prepared.body_digest.clone(),
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
        // Copied before the ledger is entered so the handoff allocates nothing while the connection is owned.
        let body = prepared.body.clone();
        let hold = &self.broker.binding().hold;
        let outcome = self
            .ledger
            .dispatch_curator_attempt(
                self.project,
                &hold.subject,
                hold.generation,
                self.claim_id,
                &hold.kernel_incarnation,
                &marker,
                (connected, body),
                self.now_ms,
                |(connected, body)| connected.handoff(body),
            )
            .map_err(|error| match error {
                CuratorLedgerError::Refused(reason) => DisclosureRefusal::Ledger(reason),
                CuratorLedgerError::Store(error) => DisclosureRefusal::Store(error.to_string()),
            })?;
        let (attempt_index, in_flight, attempt_deadline_ms) = match outcome {
            DispatchOutcome::ChargedNotDispatched {
                attempt_index,
                reason,
                finished,
            } => {
                if !finished {
                    eprintln!(
                        "daemon: curator disclosure terminal NotDispatched not recorded for {}/{} attempt {attempt_index}: {reason}",
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
                handoff,
                attempt_deadline_ms,
                release,
            } => {
                // No request byte has been written yet. A ledger that could not restore its view after the handoff may refuse this attempt's terminal, so the unwritten request is dropped rather than sent under a store that cannot record its outcome.
                if let Err(error) = release {
                    drop(handoff);
                    return Err(self.end(
                        attempt_index,
                        CuratorAttemptTerminal::Failed,
                        DisclosureRefusal::Store(error.to_string()),
                    ));
                }
                match handoff {
                    // The sender knows the connection never took the request, but `NotDispatched` is the ledger's proof of no disclosure and only the dispatch path may write it; the attempt is charged and ends `Failed`, and `sent: false` reports what the sender saw.
                    Err(error) => {
                        return Err(self.end(
                            attempt_index,
                            CuratorAttemptTerminal::Failed,
                            DisclosureRefusal::Send {
                                attempt_index: Some(attempt_index),
                                error,
                                sent: false,
                            },
                        ));
                    }
                    Ok(in_flight) => (attempt_index, in_flight, attempt_deadline_ms),
                }
            }
        };
        // The ledger bounded the attempt when it committed the marker; a response after that bound is not this attempt's.
        let remaining = u64::try_from(attempt_deadline_ms - (self.now_ms)()).unwrap_or(0);
        let deadline = deadline.min(Instant::now() + Duration::from_millis(remaining));
        let completed = tokio::select! {
            biased;
            () = cancel.cancelled() => {
                // The request may already be on the wire; the attempt is cancelled, never retried.
                return Err(self.end(attempt_index, CuratorAttemptTerminal::Cancelled, DisclosureRefusal::Cancelled));
            }
            completed = in_flight.complete(deadline) => completed,
        };
        let text = match completed {
            Ok(text) => text,
            // `NotReady` means the connection took the request back unwritten. The ledger still ends the attempt `Failed`, because no-disclosure proof is the dispatch path's alone; `sent: false` carries the sender's report.
            Err(SendError::NotReady) => {
                return Err(self.end(
                    attempt_index,
                    CuratorAttemptTerminal::Failed,
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
                    CuratorAttemptTerminal::Failed,
                    DisclosureRefusal::Send {
                        attempt_index: Some(attempt_index),
                        error,
                        sent: true,
                    },
                ));
            }
        };
        if text.model.as_deref() != Some(prepared.model()) {
            return Err(self.end(
                attempt_index,
                CuratorAttemptTerminal::Failed,
                DisclosureRefusal::ModelMismatch { attempt_index },
            ));
        }
        self.finish(attempt_index, CuratorAttemptTerminal::Complete)
            .map_err(|error| DisclosureRefusal::TerminalNotRecorded {
                attempt_index,
                error,
            })?;
        Ok(Disclosed {
            attempt_index,
            text,
        })
    }

    /// The approval must name exactly the provider this sender dials, the requested model, the credential, and both live incarnations. The hold's Memory Store incarnation is checked against the live store because the hold copied it at acquisition and a restored ledger changes it.
    fn approved(&self, model: &str) -> Result<(), DisclosureRefusal> {
        let approval = self.approval.ok_or(DisclosureRefusal::Unavailable)?;
        let hold = &self.broker.binding().hold;
        let live_memstore = self
            .ledger
            .curator_store_incarnation()
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

    /// Re-judges every disclosed alias at the current clock, once per alias, and collects the evidence ids they name.
    fn revalidate(&self, prepared: &PreparedBody) -> Result<Vec<String>, DisclosureRefusal> {
        let now = (self.now_ms)();
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
                CuratorHoldKind::Execution,
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
        terminal: CuratorAttemptTerminal,
        refusal: DisclosureRefusal,
    ) -> DisclosureRefusal {
        match self.finish(attempt_index, terminal) {
            Ok(()) => refusal,
            Err(error) => {
                let hold = &self.broker.binding().hold;
                eprintln!(
                    "daemon: curator disclosure terminal {terminal:?} not recorded for {}/{} attempt {attempt_index} ({refusal}): {error}",
                    self.project, hold.subject
                );
                DisclosureRefusal::TerminalNotRecorded {
                    attempt_index,
                    error,
                }
            }
        }
    }

    fn finish(&self, attempt_index: u32, terminal: CuratorAttemptTerminal) -> Result<(), String> {
        let hold = &self.broker.binding().hold;
        self.ledger
            .finish_curator_attempt(
                self.project,
                &hold.subject,
                hold.generation,
                self.claim_id,
                attempt_index,
                terminal,
                (self.now_ms)(),
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}
