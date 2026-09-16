//! Selection is not permission and preparation is not application: a receipt records what the daemon forwarded, and only an acknowledgment carrying the applied identity completes it.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use host_runtime::RouteHandle;
use kernel::source_identity::Span;
use retrieval::fusion::{
    ContextRepresentation, ContextRevision, IdentityRefusal, OccurrenceId, PreparationDigest,
    PreparationInputs, SelectedSpan, SelectionDigest,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::dispatch::{PreparedOutcome, PreparedOutput};
use crate::{HandlerCore, invalid_params_error};

pub(crate) const PREPARE: &str = "retrieval.prepare";
pub(crate) const APPLY: &str = "retrieval.apply";
pub(crate) const CONFIRM: &str = "retrieval.confirm";

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReceiptLimitsRefusal {
    #[error("retention {retention:?} is shorter than the longest supported retry path {floor:?}")]
    RetentionBelowFloor {
        retention: Duration,
        floor: Duration,
    },
    #[error("retention must be positive")]
    ZeroRetention,
}

/// Parent Q9: `retention` must cover `retention_floor`, the longest supported retry path, so a legal retry never meets an expired key.
#[derive(Debug, Clone, Copy)]
pub struct ReceiptLimits {
    pub max_keys: NonZeroUsize,
    pub retention: Duration,
    pub retention_floor: Duration,
    pub append_allowance_bytes: u64,
    pub replacement_capacity_bytes: u64,
}

impl ReceiptLimits {
    pub fn validate(&self) -> Result<(), ReceiptLimitsRefusal> {
        if self.retention.is_zero() {
            return Err(ReceiptLimitsRefusal::ZeroRetention);
        }
        if self.retention < self.retention_floor {
            return Err(ReceiptLimitsRefusal::RetentionBelowFloor {
                retention: self.retention,
                floor: self.retention_floor,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Append,
    Replace,
}

impl Action {
    fn code(self) -> &'static str {
        match self {
            Self::Append => "append",
            Self::Replace => "replace",
        }
    }
}

/// The closed outcome set; `Unknown` is a receipt state, not an outcome a harness may report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Keep,
    Append,
    AppliedReplacement,
    PreparationFailure,
}

impl Outcome {
    pub fn code(self) -> &'static str {
        match self {
            Self::Keep => "keep",
            Self::Append => "append",
            Self::AppliedReplacement => "applied_replacement",
            Self::PreparationFailure => "preparation_failure",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Disabled,
    /// The key names this incarnation but the store holds nothing for it: expired, evicted, or never prepared; it never authorizes a replay.
    ReceiptUnavailable,
    StalePreparation,
    /// Same key, different digest, identity, or outcome.
    Conflict,
}

impl Refusal {
    pub fn code(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::ReceiptUnavailable => "receipt_unavailable",
            Self::StalePreparation => "stale_preparation",
            Self::Conflict => "conflict",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    Prepared,
    InFlight {
        forwarded_identity: String,
    },
    Complete {
        forwarded_identity: String,
        outcome: Outcome,
    },
    Unknown,
}

#[derive(Debug, Clone)]
struct Receipt {
    digest: PreparationDigest,
    action: Action,
    edit_bytes: u64,
    touched: Instant,
    state: State,
}

/// Bounded by count and by age; an entry past either bound is dropped, and a dropped key is refused rather than replayed.
pub struct ReceiptStore {
    limits: ReceiptLimits,
    incarnation: String,
    sequence: u64,
    receipts: BTreeMap<String, Receipt>,
}

impl ReceiptStore {
    pub fn new(limits: ReceiptLimits, incarnation: String) -> Self {
        Self {
            limits,
            incarnation,
            sequence: 0,
            receipts: BTreeMap::new(),
        }
    }

    fn sweep(&mut self, now: Instant) {
        let retention = self.limits.retention;
        self.receipts
            .retain(|_, receipt| now.duration_since(receipt.touched) < retention);
        while self.receipts.len() >= self.limits.max_keys.get() {
            let oldest = self
                .receipts
                .iter()
                .min_by_key(|(_, receipt)| receipt.touched)
                .map(|(id, _)| id.clone());
            match oldest {
                Some(id) => {
                    self.receipts.remove(&id);
                }
                None => break,
            }
        }
    }

    fn owns(&self, preparation_id: &str) -> bool {
        preparation_id
            .strip_prefix(&self.incarnation)
            .is_some_and(|rest| rest.starts_with('-'))
    }
}

fn derive(domain: &str, components: &[&[u8]]) -> String {
    let mut hash = Sha256::new();
    hash.update(domain.as_bytes());
    hash.update([0u8]);
    for component in components {
        hash.update((component.len() as u64).to_be_bytes());
        hash.update(component);
    }
    hash.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Debug, Clone, Deserialize)]
pub struct WireSpan {
    pub occurrence_id: String,
    pub buffer_len: u64,
    pub span: Option<(u64, u64)>,
}

/// The context a preparation binds and an apply restates; the digest over it is what stale detection compares.
#[derive(Debug, Clone, Deserialize)]
pub struct Context {
    pub context_revision: String,
    pub representation: String,
    pub spans: Vec<WireSpan>,
    pub selection: Vec<String>,
}

impl Context {
    fn digest(&self) -> Result<PreparationDigest, IdentityRefusal> {
        let context = ContextRevision::parse(&self.context_revision)?;
        let representation = ContextRepresentation::parse(&self.representation)?;
        let spans = self
            .spans
            .iter()
            .map(|wire| {
                SelectedSpan::new(
                    OccurrenceId::parse(&wire.occurrence_id)?,
                    wire.span.map(|(start, end)| Span { start, end }),
                    wire.buffer_len,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let selection = self
            .selection
            .iter()
            .map(|id| OccurrenceId::parse(id))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(PreparationDigest::derive(PreparationInputs {
            context: &context,
            representation: &representation,
            spans: &spans,
            selection: &SelectionDigest::derive(&selection),
        }))
    }

    fn selection_digest(&self) -> Result<SelectionDigest, IdentityRefusal> {
        let selection = self
            .selection
            .iter()
            .map(|id| OccurrenceId::parse(id))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(SelectionDigest::derive(&selection))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PrepareRequest {
    #[serde(flatten)]
    context: Context,
    action: Action,
    accounting_profile: String,
    edit_bytes: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyRequest {
    preparation_id: String,
    #[serde(flatten)]
    context: Context,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfirmRequest {
    preparation_id: String,
    forwarded_identity: String,
    applied_identity: Option<String>,
    outcome: Outcome,
}

pub struct Prepared {
    pub preparation_id: String,
    pub preparation_digest: String,
    pub fingerprint: String,
}

pub enum PrepareOutcome {
    Prepared(Prepared),
    /// Over the append allowance or the replacement capacity, decided before any identity is minted.
    Failure(&'static str),
}

pub enum ApplyOutcome {
    Forwarded {
        forwarded_identity: String,
        action: Action,
        edit_bytes: u64,
    },
    InFlight {
        forwarded_identity: String,
    },
    Complete {
        outcome: Outcome,
    },
    Unknown,
}

pub enum ConfirmOutcome {
    Complete { outcome: Outcome },
    Unknown,
}

impl ReceiptStore {
    /// Parent Q7: a second application of one selection is a legal intent, so the tuple is a fingerprint and the identity minted here is the key.
    pub fn prepare(
        &mut self,
        now: Instant,
        context: &Context,
        action: Action,
        accounting_profile: &str,
        edit_bytes: u64,
    ) -> Result<PrepareOutcome, IdentityRefusal> {
        let capacity = match action {
            Action::Append => (self.limits.append_allowance_bytes, "append_allowance"),
            Action::Replace => (
                self.limits.replacement_capacity_bytes,
                "replacement_capacity",
            ),
        };
        if edit_bytes > capacity.0 {
            return Ok(PrepareOutcome::Failure(capacity.1));
        }
        let digest = context.digest()?;
        let selection = context.selection_digest()?;
        let fingerprint = derive(
            "eidnara-daemon-preparation-fingerprint-v1",
            &[
                self.incarnation.as_bytes(),
                context.context_revision.as_bytes(),
                action.code().as_bytes(),
                selection.as_bytes(),
                accounting_profile.as_bytes(),
            ],
        );
        self.sweep(now);
        self.sequence += 1;
        let preparation_id = format!(
            "{}-{}",
            self.incarnation,
            derive(
                "eidnara-daemon-preparation-id-v1",
                &[fingerprint.as_bytes(), &self.sequence.to_be_bytes()],
            )
        );
        self.receipts.insert(
            preparation_id.clone(),
            Receipt {
                digest,
                action,
                edit_bytes,
                touched: now,
                state: State::Prepared,
            },
        );
        Ok(PrepareOutcome::Prepared(Prepared {
            preparation_id,
            preparation_digest: digest.to_string(),
            fingerprint,
        }))
    }

    /// Parent Q8: a key of another incarnation is `Unknown`; a duplicate while an apply is in flight receives the in-flight state and forwards nothing.
    pub fn apply(
        &mut self,
        now: Instant,
        preparation_id: &str,
        context: &Context,
    ) -> Result<Result<ApplyOutcome, Refusal>, IdentityRefusal> {
        if !self.owns(preparation_id) {
            return Ok(Ok(ApplyOutcome::Unknown));
        }
        let digest = context.digest()?;
        self.sweep(now);
        let Some(receipt) = self.receipts.get_mut(preparation_id) else {
            return Ok(Err(Refusal::ReceiptUnavailable));
        };
        receipt.touched = now;
        let outcome = match &receipt.state {
            State::Prepared => {
                if receipt.digest != digest {
                    return Ok(Err(Refusal::StalePreparation));
                }
                let forwarded_identity = derive(
                    "eidnara-daemon-forwarded-identity-v1",
                    &[preparation_id.as_bytes(), digest.as_bytes()],
                );
                receipt.state = State::InFlight {
                    forwarded_identity: forwarded_identity.clone(),
                };
                ApplyOutcome::Forwarded {
                    forwarded_identity,
                    action: receipt.action,
                    edit_bytes: receipt.edit_bytes,
                }
            }
            State::InFlight { forwarded_identity } => {
                if receipt.digest != digest {
                    return Ok(Err(Refusal::Conflict));
                }
                ApplyOutcome::InFlight {
                    forwarded_identity: forwarded_identity.clone(),
                }
            }
            State::Complete { outcome, .. } => {
                if receipt.digest != digest {
                    return Ok(Err(Refusal::Conflict));
                }
                ApplyOutcome::Complete { outcome: *outcome }
            }
            State::Unknown => ApplyOutcome::Unknown,
        };
        Ok(Ok(outcome))
    }

    /// Applied is recorded only when the acknowledgment's applied identity equals the identity the daemon forwarded; an acknowledgment without one leaves the key `Unknown`.
    pub fn confirm(
        &mut self,
        now: Instant,
        preparation_id: &str,
        forwarded_identity: &str,
        applied_identity: Option<&str>,
        outcome: Outcome,
    ) -> Result<ConfirmOutcome, Refusal> {
        if !self.owns(preparation_id) {
            return match applied_identity {
                Some(applied) if applied == forwarded_identity => {
                    Ok(ConfirmOutcome::Complete { outcome })
                }
                _ => Ok(ConfirmOutcome::Unknown),
            };
        }
        self.sweep(now);
        let Some(receipt) = self.receipts.get_mut(preparation_id) else {
            return Err(Refusal::ReceiptUnavailable);
        };
        receipt.touched = now;
        match &receipt.state {
            State::Prepared => Err(Refusal::Conflict),
            State::InFlight {
                forwarded_identity: recorded,
            } => {
                if recorded != forwarded_identity {
                    return Err(Refusal::Conflict);
                }
                match applied_identity {
                    Some(applied) if applied == recorded => {
                        receipt.state = State::Complete {
                            forwarded_identity: recorded.clone(),
                            outcome,
                        };
                        Ok(ConfirmOutcome::Complete { outcome })
                    }
                    Some(_) => Err(Refusal::Conflict),
                    None => {
                        receipt.state = State::Unknown;
                        Ok(ConfirmOutcome::Unknown)
                    }
                }
            }
            State::Complete {
                forwarded_identity: recorded,
                outcome: known,
            } => {
                if recorded == forwarded_identity
                    && applied_identity == Some(recorded.as_str())
                    && *known == outcome
                {
                    Ok(ConfirmOutcome::Complete { outcome })
                } else {
                    Err(Refusal::Conflict)
                }
            }
            State::Unknown => match applied_identity {
                Some(applied) if applied == forwarded_identity => {
                    receipt.state = State::Complete {
                        forwarded_identity: forwarded_identity.to_string(),
                        outcome,
                    };
                    Ok(ConfirmOutcome::Complete { outcome })
                }
                _ => Ok(ConfirmOutcome::Unknown),
            },
        }
    }

    pub fn len(&self) -> usize {
        self.receipts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.receipts.is_empty()
    }
}

pub(crate) fn fresh_incarnation() -> String {
    let mut nonce = [0u8; 8];
    getrandom::getrandom(&mut nonce).expect("OS entropy for the receipt incarnation");
    nonce.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn response(body: Value) -> PreparedOutcome {
    PreparedOutcome::Response(PreparedOutput::json(body))
}

fn refusal(refusal: Refusal) -> PreparedOutcome {
    response(json!({ "kind": "terminal", "terminal": refusal.code() }))
}

fn identity_refusal(operation: &str, refusal: IdentityRefusal) -> PreparedOutcome {
    invalid_params_error(format!("{operation}: {refusal}"))
}

impl HandlerCore {
    pub fn set_edit_receipt_limits(
        &self,
        limits: Option<ReceiptLimits>,
    ) -> Result<(), ReceiptLimitsRefusal> {
        if let Some(limits) = &limits {
            limits.validate()?;
        }
        let mut slot = self
            .edit_receipts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *slot = limits.map(|limits| {
            Arc::new(Mutex::new(ReceiptStore::new(
                limits,
                self.edit_incarnation.clone(),
            )))
        });
        Ok(())
    }

    fn receipt_store(&self) -> Option<Arc<Mutex<ReceiptStore>>> {
        self.edit_receipts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn handle_retrieval_prepare(
        &self,
        channel: RouteHandle,
        request: Value,
    ) -> PreparedOutcome {
        let (_scope, parsed) =
            match self.kernel_request::<PrepareRequest>(channel, request, PREPARE) {
                Ok(bound) => bound,
                Err(outcome) => return outcome,
            };
        let Some(store) = self.receipt_store() else {
            return refusal(Refusal::Disabled);
        };
        let mut store = store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match store.prepare(
            Instant::now(),
            &parsed.context,
            parsed.action,
            &parsed.accounting_profile,
            parsed.edit_bytes,
        ) {
            Ok(PrepareOutcome::Prepared(prepared)) => response(json!({
                "kind": "prepared",
                "preparation_id": prepared.preparation_id,
                "preparation_digest": prepared.preparation_digest,
                "fingerprint": prepared.fingerprint,
            })),
            Ok(PrepareOutcome::Failure(reason)) => response(json!({
                "kind": "outcome",
                "outcome": Outcome::PreparationFailure.code(),
                "reason": reason,
            })),
            Err(refusal) => identity_refusal(PREPARE, refusal),
        }
    }

    pub(crate) fn handle_retrieval_apply(
        &self,
        channel: RouteHandle,
        request: Value,
    ) -> PreparedOutcome {
        let (_scope, parsed) = match self.kernel_request::<ApplyRequest>(channel, request, APPLY) {
            Ok(bound) => bound,
            Err(outcome) => return outcome,
        };
        let Some(store) = self.receipt_store() else {
            return refusal(Refusal::Disabled);
        };
        let mut store = store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match store.apply(Instant::now(), &parsed.preparation_id, &parsed.context) {
            Ok(Ok(ApplyOutcome::Forwarded {
                forwarded_identity,
                action,
                edit_bytes,
            })) => response(json!({
                "kind": "forwarded",
                "preparation_id": parsed.preparation_id,
                "forwarded_identity": forwarded_identity,
                "action": action.code(),
                "edit_bytes": edit_bytes,
            })),
            Ok(Ok(ApplyOutcome::InFlight { forwarded_identity })) => response(json!({
                "kind": "receipt",
                "state": "in_flight",
                "forwarded_identity": forwarded_identity,
            })),
            Ok(Ok(ApplyOutcome::Complete { outcome })) => response(json!({
                "kind": "receipt",
                "state": "complete",
                "outcome": outcome.code(),
            })),
            Ok(Ok(ApplyOutcome::Unknown)) => {
                response(json!({ "kind": "receipt", "state": "unknown" }))
            }
            Ok(Err(refused)) => refusal(refused),
            Err(identity) => identity_refusal(APPLY, identity),
        }
    }

    pub(crate) fn handle_retrieval_confirm(
        &self,
        channel: RouteHandle,
        request: Value,
    ) -> PreparedOutcome {
        let (_scope, parsed) =
            match self.kernel_request::<ConfirmRequest>(channel, request, CONFIRM) {
                Ok(bound) => bound,
                Err(outcome) => return outcome,
            };
        let Some(store) = self.receipt_store() else {
            return refusal(Refusal::Disabled);
        };
        let mut store = store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match store.confirm(
            Instant::now(),
            &parsed.preparation_id,
            &parsed.forwarded_identity,
            parsed.applied_identity.as_deref(),
            parsed.outcome,
        ) {
            Ok(ConfirmOutcome::Complete { outcome }) => response(json!({
                "kind": "receipt",
                "state": "complete",
                "outcome": outcome.code(),
            })),
            Ok(ConfirmOutcome::Unknown) => {
                response(json!({ "kind": "receipt", "state": "unknown" }))
            }
            Err(refused) => refusal(refused),
        }
    }
}
