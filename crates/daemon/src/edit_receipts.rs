//! Selection is not permission and preparation is not application: a receipt records what the daemon forwarded, and only an acknowledgment carrying the applied identity completes it.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
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

use host_runtime::model_execution::backend::EditClass;

use crate::context_capabilities::CapabilityDenial;
use crate::dispatch::{PreparedOutcome, PreparedOutput};
use crate::kernel_routes::RouteScope;
use crate::{HandlerCore, invalid_params_error};

pub(crate) const PREPARE: &str = "retrieval.prepare";
pub(crate) const APPLY: &str = "retrieval.apply";
pub(crate) const CONFIRM: &str = "retrieval.confirm";

/// Parent Q9: the longest supported retry path, one route deadline ceiling plus one client retry of the same length; a retention below it would expire a key a legal retry still needs.
pub const RETENTION_FLOOR: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReceiptLimitsRefusal {
    #[error(
        "retention {retention:?} is shorter than the longest supported retry path {RETENTION_FLOOR:?}"
    )]
    RetentionBelowFloor { retention: Duration },
}

#[derive(Debug, Clone, Copy)]
pub struct ReceiptLimits {
    pub max_keys: NonZeroUsize,
    pub retention: Duration,
    pub append_allowance_bytes: u64,
    pub replacement_capacity_bytes: u64,
}

impl ReceiptLimits {
    pub fn validate(&self) -> Result<(), ReceiptLimitsRefusal> {
        if self.retention < RETENTION_FLOOR {
            return Err(ReceiptLimitsRefusal::RetentionBelowFloor {
                retention: self.retention,
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
    Suppress,
    Reuse,
}

impl Action {
    fn code(self) -> &'static str {
        match self {
            Self::Append => "append",
            Self::Replace => "replace",
            Self::Suppress => "suppress",
            Self::Reuse => "reuse",
        }
    }

    /// Append is not gated: every harness accepts an appended block.
    fn gated_class(self) -> Option<EditClass> {
        match self {
            Self::Append => None,
            Self::Replace => Some(EditClass::Replacement),
            Self::Suppress => Some(EditClass::Suppression),
            Self::Reuse => Some(EditClass::CrossStepReuse),
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
    /// The route binding's latched declaration does not allow the edit class, or could not be read.
    CapabilityUnsupported(CapabilityDenial),
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
            Self::CapabilityUnsupported(_) => "capability_unsupported",
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
    /// The forwarded identity stays recorded so a read-back is judged against it, never against the caller's own claim.
    Unknown {
        forwarded_identity: String,
    },
}

#[derive(Debug, Clone)]
struct Receipt {
    digest: Option<PreparationDigest>,
    action: Action,
    edit_bytes: u64,
    created: Instant,
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

    fn expire(&mut self, now: Instant) {
        let retention = self.limits.retention;
        self.receipts
            .retain(|_, receipt| now.duration_since(receipt.created) < retention);
    }

    /// The count bound is enforced only when a key is minted: the oldest receipt that is not in flight makes room, and when every receipt is in flight the new preparation fails instead of dropping one whose edit may already be applied.
    fn make_room(&mut self) -> bool {
        while self.receipts.len() >= self.limits.max_keys.get() {
            let victim = self
                .receipts
                .iter()
                .filter(|(_, receipt)| !matches!(receipt.state, State::InFlight { .. }))
                .min_by_key(|(_, receipt)| receipt.created)
                .map(|(id, _)| id.clone());
            match victim {
                Some(id) => {
                    self.receipts.remove(&id);
                }
                None => return false,
            }
        }
        true
    }

    pub fn set_limits(&mut self, limits: ReceiptLimits) {
        self.limits = limits;
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
    /// Adapter-supplied proof of the spans that survive in the current context; suppression is prepared only for selected occurrences this set confirms whole.
    #[serde(default)]
    survivors: Vec<WireSpan>,
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
    /// Over the append allowance or the replacement capacity, or a suppression without confirmed survivors, decided before any identity is minted.
    Failure(&'static str),
}

/// Parent Q10: suppression is whole-message; a survivor confirmed only for a span, or a selected occurrence absent from the survivors, is not a confirmed survivor.
fn unconfirmed_survivor(selection: &[String], survivors: &[WireSpan]) -> Option<&'static str> {
    if survivors.is_empty() {
        return Some("no_survivor_proof");
    }
    for occurrence_id in selection {
        match survivors
            .iter()
            .find(|survivor| survivor.occurrence_id == *occurrence_id)
        {
            None => return Some("unconfirmed_survivor"),
            Some(survivor)
                if survivor
                    .span
                    .is_some_and(|(start, end)| start != 0 || end != survivor.buffer_len) =>
            {
                return Some("span_granularity");
            }
            Some(_) => {}
        }
    }
    None
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyRefusal {
    Refused(Refusal),
    Invalid(IdentityRefusal),
}

impl From<IdentityRefusal> for ApplyRefusal {
    fn from(refusal: IdentityRefusal) -> Self {
        Self::Invalid(refusal)
    }
}

impl From<Refusal> for ApplyRefusal {
    fn from(refusal: Refusal) -> Self {
        Self::Refused(refusal)
    }
}

fn well_formed_identity(identity: &str) -> bool {
    identity.len() == 64 && identity.bytes().all(|b| b.is_ascii_hexdigit())
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
        survivors: &[WireSpan],
    ) -> Result<PrepareOutcome, IdentityRefusal> {
        let capacity = match action {
            Action::Append => Some((self.limits.append_allowance_bytes, "append_allowance")),
            Action::Replace | Action::Reuse => Some((
                self.limits.replacement_capacity_bytes,
                "replacement_capacity",
            )),
            Action::Suppress => None,
        };
        if let Some((bound, reason)) = capacity
            && edit_bytes > bound
        {
            return Ok(PrepareOutcome::Failure(reason));
        }
        if action == Action::Suppress
            && let Some(reason) = unconfirmed_survivor(&context.selection, survivors)
        {
            return Ok(PrepareOutcome::Failure(reason));
        }
        let digest = context.digest()?;
        let selection = context.selection_digest()?;
        self.expire(now);
        if !self.make_room() {
            return Ok(PrepareOutcome::Failure("receipt_capacity"));
        }
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
                digest: Some(digest),
                action,
                edit_bytes,
                created: now,
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
    ) -> Result<ApplyOutcome, ApplyRefusal> {
        let digest = context.digest()?;
        self.expire(now);
        if !self.owns(preparation_id) {
            return Ok(match self.receipts.get(preparation_id).map(|r| &r.state) {
                Some(State::Complete { outcome, .. }) => {
                    ApplyOutcome::Complete { outcome: *outcome }
                }
                _ => ApplyOutcome::Unknown,
            });
        }
        let Some(receipt) = self.receipts.get_mut(preparation_id) else {
            return Err(Refusal::ReceiptUnavailable.into());
        };
        let prepared = receipt.digest;
        let outcome = match &receipt.state {
            State::Prepared => {
                if prepared != Some(digest) {
                    return Err(Refusal::StalePreparation.into());
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
                if prepared != Some(digest) {
                    return Err(Refusal::Conflict.into());
                }
                ApplyOutcome::InFlight {
                    forwarded_identity: forwarded_identity.clone(),
                }
            }
            State::Complete { outcome, .. } => {
                if prepared != Some(digest) {
                    return Err(Refusal::Conflict.into());
                }
                ApplyOutcome::Complete { outcome: *outcome }
            }
            State::Unknown { .. } => ApplyOutcome::Unknown,
        };
        Ok(outcome)
    }

    /// Applied is recorded only when the acknowledgment's applied identity equals the identity the daemon forwarded; an acknowledgment without one leaves the key `Unknown`.
    /// Parent Q9 read-back: for a key of another incarnation the daemon holds no record, so the adapter's own forwarded and applied identities are the evidence; they must be well formed and equal, and the reclassification is recorded so the key does not fall back to `Unknown`.
    pub fn confirm(
        &mut self,
        now: Instant,
        preparation_id: &str,
        forwarded_identity: &str,
        applied_identity: Option<&str>,
        outcome: Outcome,
    ) -> Result<ConfirmOutcome, Refusal> {
        self.expire(now);
        if !self.owns(preparation_id) {
            if let Some(receipt) = self.receipts.get(preparation_id)
                && let State::Complete {
                    forwarded_identity: recorded,
                    outcome: known,
                } = &receipt.state
            {
                return if recorded == forwarded_identity && *known == outcome {
                    Ok(ConfirmOutcome::Complete { outcome })
                } else {
                    Err(Refusal::Conflict)
                };
            }
            return match applied_identity {
                Some(applied) if applied == forwarded_identity && well_formed_identity(applied) => {
                    if self.make_room() {
                        self.receipts.insert(
                            preparation_id.to_string(),
                            Receipt {
                                digest: None,
                                action: Action::Append,
                                edit_bytes: 0,
                                created: now,
                                state: State::Complete {
                                    forwarded_identity: forwarded_identity.to_string(),
                                    outcome,
                                },
                            },
                        );
                    }
                    Ok(ConfirmOutcome::Complete { outcome })
                }
                _ => Ok(ConfirmOutcome::Unknown),
            };
        }
        let Some(receipt) = self.receipts.get_mut(preparation_id) else {
            return Err(Refusal::ReceiptUnavailable);
        };
        match &receipt.state {
            State::Prepared => Err(Refusal::Conflict),
            State::InFlight {
                forwarded_identity: recorded,
            }
            | State::Unknown {
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
                        receipt.state = State::Unknown {
                            forwarded_identity: recorded.clone(),
                        };
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
        }
    }

    pub fn len(&self) -> usize {
        self.receipts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.receipts.is_empty()
    }

    pub fn holds(&self, preparation_id: &str) -> bool {
        self.receipts.contains_key(preparation_id)
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
    match refusal {
        Refusal::CapabilityUnsupported(denial) => response(json!({
            "kind": "terminal",
            "terminal": refusal.code(),
            "class": denial.class().code(),
            "reason": denial.reason(),
        })),
        _ => response(json!({ "kind": "terminal", "terminal": refusal.code() })),
    }
}

fn identity_refusal(operation: &str, refusal: IdentityRefusal) -> PreparedOutcome {
    invalid_params_error(format!("{operation}: {refusal}"))
}

impl HandlerCore {
    /// A limits change keeps every receipt: the keys stay owned by this incarnation, so an in-flight edit can still be confirmed.
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
        match (limits, slot.as_mut()) {
            (Some(limits), Some(store)) => store.set_limits(limits),
            (Some(limits), None) => {
                *slot = Some(ReceiptStore::new(limits, self.edit_incarnation.clone()));
            }
            (None, _) => *slot = None,
        }
        Ok(())
    }

    fn with_receipts<T>(
        &self,
        channel: RouteHandle,
        request: Value,
        operation: &str,
        f: impl FnOnce(&mut ReceiptStore, RouteScope, T) -> PreparedOutcome,
    ) -> PreparedOutcome
    where
        T: serde::de::DeserializeOwned,
    {
        let (scope, parsed) = match self.kernel_request::<T>(channel, request, operation) {
            Ok(bound) => bound,
            Err(outcome) => return outcome,
        };
        let mut slot = self
            .edit_receipts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(store) = slot.as_mut() else {
            return refusal(Refusal::Disabled);
        };
        f(store, scope, parsed)
    }

    pub(crate) fn handle_retrieval_prepare(
        &self,
        channel: RouteHandle,
        request: Value,
    ) -> PreparedOutcome {
        self.with_receipts(
            channel,
            request,
            PREPARE,
            |store, scope, parsed: PrepareRequest| {
                if let Some(class) = parsed.action.gated_class()
                    && let Err(denial) = scope.context_capabilities.gate(class)
                {
                    return refusal(Refusal::CapabilityUnsupported(denial));
                }
                match store.prepare(
                    Instant::now(),
                    &parsed.context,
                    parsed.action,
                    &parsed.accounting_profile,
                    parsed.edit_bytes,
                    &parsed.survivors,
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
                    Err(identity) => identity_refusal(PREPARE, identity),
                }
            },
        )
    }

    pub(crate) fn handle_retrieval_apply(
        &self,
        channel: RouteHandle,
        request: Value,
    ) -> PreparedOutcome {
        self.with_receipts(
            channel,
            request,
            APPLY,
            |store, _scope, parsed: ApplyRequest| match store.apply(
                Instant::now(),
                &parsed.preparation_id,
                &parsed.context,
            ) {
                Ok(ApplyOutcome::Forwarded {
                    forwarded_identity,
                    action,
                    edit_bytes,
                }) => response(json!({
                    "kind": "forwarded",
                    "preparation_id": parsed.preparation_id,
                    "forwarded_identity": forwarded_identity,
                    "action": action.code(),
                    "edit_bytes": edit_bytes,
                })),
                Ok(ApplyOutcome::InFlight { forwarded_identity }) => response(json!({
                    "kind": "receipt",
                    "state": "in_flight",
                    "forwarded_identity": forwarded_identity,
                })),
                Ok(ApplyOutcome::Complete { outcome }) => response(json!({
                    "kind": "receipt",
                    "state": "complete",
                    "outcome": outcome.code(),
                })),
                Ok(ApplyOutcome::Unknown) => {
                    response(json!({ "kind": "receipt", "state": "unknown" }))
                }
                Err(ApplyRefusal::Refused(refused)) => refusal(refused),
                Err(ApplyRefusal::Invalid(identity)) => identity_refusal(APPLY, identity),
            },
        )
    }

    pub(crate) fn handle_retrieval_confirm(
        &self,
        channel: RouteHandle,
        request: Value,
    ) -> PreparedOutcome {
        self.with_receipts(
            channel,
            request,
            CONFIRM,
            |store, _scope, parsed: ConfirmRequest| match store.confirm(
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
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(max_keys: usize, retention: Duration) -> ReceiptLimits {
        ReceiptLimits {
            max_keys: NonZeroUsize::new(max_keys).unwrap(),
            retention,
            append_allowance_bytes: 100,
            replacement_capacity_bytes: 100,
        }
    }

    fn context(revision: &str) -> Context {
        Context {
            context_revision: revision.to_string(),
            representation: "repr".to_string(),
            spans: Vec::new(),
            selection: vec!["11".repeat(32)],
        }
    }

    fn prepared(store: &mut ReceiptStore, now: Instant) -> String {
        match store
            .prepare(now, &context("rev"), Action::Append, "profile", 1, &[])
            .unwrap()
        {
            PrepareOutcome::Prepared(prepared) => prepared.preparation_id,
            PrepareOutcome::Failure(reason) => panic!("{reason}"),
        }
    }

    #[test]
    fn a_key_expires_by_its_creation_time_not_by_its_last_use() {
        let start = Instant::now();
        let mut store = ReceiptStore::new(limits(8, Duration::from_secs(10)), "inc".to_string());
        let key = prepared(&mut store, start);
        for step in 1..5 {
            let now = start + Duration::from_secs(step * 2);
            assert!(store.apply(now, &key, &context("rev")).is_ok(), "{step}");
        }
        assert!(matches!(
            store.apply(start + Duration::from_secs(10), &key, &context("rev")),
            Err(ApplyRefusal::Refused(Refusal::ReceiptUnavailable))
        ));
        assert!(store.is_empty());
    }

    #[test]
    fn a_foreign_key_completes_only_on_a_well_formed_matching_read_back_and_stays_complete() {
        let now = Instant::now();
        let mut store = ReceiptStore::new(limits(8, RETENTION_FLOOR), "inc".to_string());
        let key = "other-abc";
        assert!(matches!(
            store.apply(now, key, &context("rev")),
            Ok(ApplyOutcome::Unknown)
        ));
        assert!(matches!(
            store.confirm(now, key, "bogus", Some("bogus"), Outcome::Keep),
            Ok(ConfirmOutcome::Unknown)
        ));
        let identity = "ab".repeat(32);
        assert!(matches!(
            store.confirm(
                now,
                key,
                &identity,
                Some("cd".repeat(32).as_str()),
                Outcome::Keep
            ),
            Ok(ConfirmOutcome::Unknown)
        ));
        assert!(matches!(
            store.confirm(now, key, &identity, Some(&identity), Outcome::Keep),
            Ok(ConfirmOutcome::Complete {
                outcome: Outcome::Keep
            })
        ));
        assert!(matches!(
            store.apply(now, key, &context("rev")),
            Ok(ApplyOutcome::Complete {
                outcome: Outcome::Keep
            })
        ));
        assert!(matches!(
            store.confirm(now, key, &identity, Some(&identity), Outcome::Append),
            Err(Refusal::Conflict)
        ));
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn a_full_store_of_in_flight_receipts_refuses_a_new_preparation() {
        let now = Instant::now();
        let mut store = ReceiptStore::new(limits(1, RETENTION_FLOOR), "inc".to_string());
        let key = prepared(&mut store, now);
        assert!(matches!(
            store.apply(now, &key, &context("rev")),
            Ok(ApplyOutcome::Forwarded { .. })
        ));
        assert!(matches!(
            store.prepare(now, &context("rev"), Action::Append, "profile", 1, &[]),
            Ok(PrepareOutcome::Failure("receipt_capacity"))
        ));
        assert!(store.holds(&key));
    }
}
