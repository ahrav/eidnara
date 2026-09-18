//! One authorized prepared body reaches the model through one supervised, durably charged handoff.
//!
//! The body is assembled once from the broker's rendered, provenance-tagged buffers and the complete policy union those disclosures produced; the bytes hashed into the marker are the bytes handed to the connection. The deployment owner's startup approval must name the provider the sender dials, the model, the credential, and both live store incarnations, or the run is unavailable and no evidence leaves the host. A broker whose disclosures were judged for a local destination never sends: the local egress fold admits evidence the remote fold refuses.
//!
//! Order matters because each step's authority is a different owner's. The Kernel re-judges every disclosed reference at the current clock and validates the execution hold over the whole evidence set before the sender opens a connection, so a revoked input never produces even a handshake. The Memory Store commits the attempt marker, rechecks claim, deadlines, cutoff, and cancellation, and hands the prepared bytes over exactly once inside its own connection ownership; the handoff touches no store and encodes nothing, so the connection is never held across an allocation or a wait. The network wait begins only after both stores have released. A commit that fails sends nothing; a recheck that lapses leaves the charged marker and sends nothing; a provider failure ends the attempt and never sends again. Text is released only when the provider reports the requested model and the ledger records the attempt complete.
//!
//! The complete API and model profile (canonical model id, `max_tokens`, `temperature` or its omission) is bound to the marker through the body digest: the profile is serialized into the body, and the marker records that body's digest and byte length.

use std::ops::Range;

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
    EvidenceBroker, OriginClass, ProvenanceTag, Refusal, RefusalCode, RenderedBuffer, hold_refusal,
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

/// The deployment owner's startup approval of provider retention and finite work exposure for one provider, model, credential, and pair of store incarnations. `provider` is compared against [`Sender::provider_identity`], so an approval for one host never authorizes a sender dialing another. Account facts are inputs; nothing here infers retention guarantees. A restored store or a new incarnation carries a different pair and needs a new approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisclosureApproval {
    pub provider: String,
    pub model: String,
    pub credential_id: String,
    pub kernel_incarnation: String,
    pub memstore_incarnation: String,
}

/// The durable identity of the attempt being authorized, as the Memory Store ledger binds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptBinding {
    pub project: String,
    pub causal_identity: String,
    pub generation: u64,
    pub claim_id: String,
    pub credential_id: String,
}

/// The assembled request bytes with the provenance of every prompt byte in them and the policy union they disclose. Built once by [`prepare_body`]; nothing mutates it afterwards, so the digest, the size, and the bytes handed over agree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedBody {
    model: String,
    tags: Vec<ProvenanceTag>,
    body: RequestBody,
    body_digest: String,
    policy_union_digest: String,
    policy_union_canonical: String,
}

impl PreparedBody {
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
    #[error("cancelled")]
    Cancelled,
    #[error("prompt_not_utf8")]
    PromptNotUtf8,
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
    /// The marker committed and the post-commit recheck lapsed; the attempt is charged and nothing was sent.
    #[error("charged_not_dispatched {reason}")]
    ChargedNotDispatched {
        attempt_index: u32,
        reason: CuratorLedgerRefusal,
    },
    /// The sender refused before or after the handoff; when `sent` is false no request byte left the host.
    #[error("send {error}")]
    Send {
        attempt_index: Option<u32>,
        error: SendError,
        sent: bool,
    },
    /// The provider answered under another model than the one requested, or none; the text is withheld.
    #[error("model_mismatch")]
    ModelMismatch { attempt_index: u32 },
    /// The provider answered under the requested model but the ledger did not record the attempt complete; the text is withheld because nothing durable says the attempt finished.
    #[error("terminal_not_recorded {error}")]
    TerminalNotRecorded { attempt_index: u32, error: String },
}

/// One accepted disclosure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disclosed {
    pub attempt_index: u32,
    pub text: AssistantText,
}

/// Everything one attempt needs, borrowed for the call: the stores, the broker whose disclosures it sends, the sender, the approval, the binding, and the ledger clock.
pub struct Disclosure<'a> {
    pub store: &'a KernelStore,
    pub ledger: &'a MemoryStore,
    pub broker: &'a EvidenceBroker,
    pub sender: &'a Sender,
    pub approval: Option<&'a DisclosureApproval>,
    pub binding: &'a AttemptBinding,
    pub now_ms: &'a (dyn Fn() -> i64 + Sync),
}

/// Assembles the body from `system` (host-authored) and the `turn` buffers, in order, recording each buffer's range in the prompt text. Every buffer keeps the tag the broker gave it; the union is the broker's complete disclosed-input union, encoded canonically.
pub fn prepare_body(
    broker: &EvidenceBroker,
    profile: &ModelProfile,
    system: RenderedBuffer,
    turn: Vec<RenderedBuffer>,
) -> Result<PreparedBody, DisclosureRefusal> {
    let mut tags = Vec::with_capacity(turn.len() + 1);
    let mut prompt_end = 0usize;
    let mut system_text = Vec::new();
    let mut content = Vec::new();
    for (index, buffer) in std::iter::once(system).chain(turn).enumerate() {
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
        model: request.model,
        body_digest: format!("{:x}", Sha256::digest(body.as_bytes())),
        tags,
        body,
        policy_union_digest: union.digest,
        policy_union_canonical: union.canonical,
    })
}

impl Disclosure<'_> {
    /// Runs one attempt end to end. `deadline` bounds the connection and the network wait; the ledger's own attempt deadline, cutoff, claim, and job deadline bound the commit and the handoff.
    pub async fn disclose(
        &self,
        prepared: &PreparedBody,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Disclosed, DisclosureRefusal> {
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
            credential_id: self.binding.credential_id.clone(),
            policy_union_digest: prepared.policy_union_digest.clone(),
        };
        // Copied before the ledger is entered so the handoff allocates nothing while the connection is owned.
        let body = prepared.body.clone();
        let binding = self.binding;
        let outcome = self
            .ledger
            .dispatch_curator_attempt(
                &binding.project,
                &binding.causal_identity,
                binding.generation,
                &binding.claim_id,
                &self.broker.binding().hold.kernel_incarnation,
                &marker,
                (connected, body),
                self.now_ms,
                |(connected, body)| connected.handoff(body),
            )
            .map_err(|error| match error {
                CuratorLedgerError::Refused(reason) => DisclosureRefusal::Ledger(reason),
                CuratorLedgerError::Store(error) => DisclosureRefusal::Store(error.to_string()),
            })?;
        let (attempt_index, attempt_deadline_ms, in_flight) = match outcome {
            DispatchOutcome::ChargedNotDispatched {
                attempt_index,
                reason,
                ..
            } => {
                return Err(DisclosureRefusal::ChargedNotDispatched {
                    attempt_index,
                    reason,
                });
            }
            DispatchOutcome::Handed {
                attempt_index,
                handoff: Err(error),
                ..
            } => {
                return Err(self.end(
                    attempt_index,
                    CuratorAttemptTerminal::NotDispatched,
                    DisclosureRefusal::Send {
                        attempt_index: Some(attempt_index),
                        error,
                        sent: false,
                    },
                ));
            }
            DispatchOutcome::Handed {
                attempt_index,
                attempt_deadline_ms,
                handoff: Ok(in_flight),
            } => (attempt_index, attempt_deadline_ms, in_flight),
        };
        // The network wait ends at the marker's deadline, which the claim expiry and cutoff bound: dispatched work never outlives the authority that must record its terminal.
        let remaining = attempt_deadline_ms.saturating_sub((self.now_ms)());
        let deadline = deadline.min(
            Instant::now()
                + std::time::Duration::from_millis(u64::try_from(remaining).unwrap_or(0)),
        );
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
            // `NotReady` means the connection took the request back unwritten: the attempt was never dispatched.
            Err(SendError::NotReady) => {
                return Err(self.end(
                    attempt_index,
                    CuratorAttemptTerminal::NotDispatched,
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
            && approval.credential_id == self.binding.credential_id
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

    /// Records a non-complete terminal and returns the refusal that caused it. A terminal the ledger refuses leaves the attempt unknown, which nothing redispatches; the cause still reaches the caller, so the ledger failure is logged here.
    fn end(
        &self,
        attempt_index: u32,
        terminal: CuratorAttemptTerminal,
        refusal: DisclosureRefusal,
    ) -> DisclosureRefusal {
        if let Err(error) = self.finish(attempt_index, terminal) {
            eprintln!(
                "daemon: curator disclosure terminal {terminal:?} not recorded for {}/{} attempt {attempt_index}: {error}",
                self.binding.project, self.binding.causal_identity
            );
        }
        refusal
    }

    fn finish(&self, attempt_index: u32, terminal: CuratorAttemptTerminal) -> Result<(), String> {
        let binding = self.binding;
        self.ledger
            .finish_curator_attempt(
                &binding.project,
                &binding.causal_identity,
                binding.generation,
                &binding.claim_id,
                attempt_index,
                terminal,
                (self.now_ms)(),
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}
