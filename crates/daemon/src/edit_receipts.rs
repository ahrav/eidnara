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

use crate::dispatch::{PreparedOutcome, PreparedOutput};
use crate::{HandlerCore, invalid_params_error};

pub(crate) const PREPARE: &str = "retrieval.prepare";
pub(crate) const APPLY: &str = "retrieval.apply";
pub(crate) const CONFIRM: &str = "retrieval.confirm";

/// Two 30 s wire request deadlines (`docs/host-wire-protocol.md` Section 11), so one request plus one full-length retry never meets an expired key. The wire deadline is fixed; it is not the operator's `deadline_ceiling` for `retrieval.query`.
pub const RETENTION_FLOOR: Duration = Duration::from_secs(60);

/// Eviction scans one project's receipts on every mint, so `max_keys` is capped at `MAX_KEYS_CEILING`. Expiry scans every live receipt, so `retention` bounds receipts to those minted within the last `retention`.
pub const MAX_KEYS_CEILING: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReceiptLimitsRefusal {
    #[error(
        "retention {retention:?} is shorter than the longest supported retry path {RETENTION_FLOOR:?}"
    )]
    RetentionBelowFloor { retention: Duration },
    #[error("max_keys {max_keys} is above the ceiling {MAX_KEYS_CEILING}")]
    MaxKeysAboveCeiling { max_keys: usize },
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
        if self.max_keys.get() > MAX_KEYS_CEILING {
            return Err(ReceiptLimitsRefusal::MaxKeysAboveCeiling {
                max_keys: self.max_keys.get(),
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

/// `ReceiptStore` applies `max_keys` independently to each project.
#[derive(Debug, Default)]
struct ProjectReceipts {
    receipts: BTreeMap<String, Receipt>,
}

impl ProjectReceipts {
    fn expire(&mut self, now: Instant, retention: Duration) {
        self.receipts
            .retain(|_, receipt| now.duration_since(receipt.created) < retention);
    }

    /// When adding a receipt exceeds `max_keys`, remove the oldest settled receipts; fail if too few are available, because an in-flight or unknown receipt's edit may already be applied.
    fn make_room(&mut self, max_keys: NonZeroUsize) -> bool {
        let excess = (self.receipts.len() + 1).saturating_sub(max_keys.get());
        if excess == 0 {
            return true;
        }
        let mut settled: Vec<(Instant, &String)> = self
            .receipts
            .iter()
            .filter(|(_, receipt)| {
                !matches!(
                    receipt.state,
                    State::InFlight { .. } | State::Unknown { .. }
                )
            })
            .map(|(id, receipt)| (receipt.created, id))
            .collect();
        if settled.len() < excess {
            return false;
        }
        settled.sort_unstable();
        let victims: Vec<String> = settled
            .into_iter()
            .take(excess)
            .map(|(_, id)| id.clone())
            .collect();
        for id in victims {
            self.receipts.remove(&id);
        }
        true
    }
}

/// Bounded by count and by age; an entry past either bound is dropped, and a dropped key is refused rather than replayed.
pub struct ReceiptStore {
    limits: ReceiptLimits,
    incarnation: String,
    sequence: u64,
    projects: BTreeMap<String, ProjectReceipts>,
}

impl ReceiptStore {
    pub fn new(limits: ReceiptLimits, incarnation: String) -> Self {
        Self {
            limits,
            incarnation,
            sequence: 0,
            projects: BTreeMap::new(),
        }
    }

    fn expire(&mut self, now: Instant) {
        let retention = self.limits.retention;
        self.projects.retain(|_, project| {
            project.expire(now, retention);
            !project.receipts.is_empty()
        });
    }

    fn project(&mut self, project: &str) -> &mut ProjectReceipts {
        self.projects.entry(project.to_string()).or_default()
    }

    fn receipts(&self, project: &str) -> Option<&BTreeMap<String, Receipt>> {
        self.projects.get(project).map(|p| &p.receipts)
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

/// `span: null` selects the whole buffer; a misspelled or omitted `span` key is refused rather than silently widening the selection.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireSpan {
    pub occurrence_id: String,
    pub buffer_len: u64,
    #[serde(deserialize_with = "crate::deserialize_nullable")]
    pub span: Option<Option<(u64, u64)>>,
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
                    wire.span.flatten().map(|(start, end)| Span { start, end }),
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
    /// Required on the wire: `null` is a lost acknowledgment, an omitted field is `invalid_params`.
    #[serde(deserialize_with = "crate::deserialize_nullable")]
    applied_identity: Option<Option<String>>,
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

const INCARNATION_BYTES: usize = 8;
const INCARNATION_HEX_LEN: usize = INCARNATION_BYTES * 2;
const IDENTITY_HEX_LEN: usize = 64;

fn lowercase_hex(text: &str) -> bool {
    text.bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Lowercase hex prevents case variants from representing distinct identities.
fn well_formed_identity(identity: &str) -> bool {
    identity.len() == IDENTITY_HEX_LEN && lowercase_hex(identity)
}

fn well_formed_preparation_id(preparation_id: &str) -> bool {
    preparation_id
        .split_once('-')
        .is_some_and(|(incarnation, identity)| {
            incarnation.len() == INCARNATION_HEX_LEN
                && lowercase_hex(incarnation)
                && well_formed_identity(identity)
        })
}

impl ReceiptStore {
    /// Parent Q7: a second application of one selection is a legal intent, so the tuple is a fingerprint and the identity minted here is the key.
    pub fn prepare(
        &mut self,
        now: Instant,
        project: &str,
        context: &Context,
        action: Action,
        accounting_profile: &str,
        edit_bytes: u64,
    ) -> Result<PrepareOutcome, IdentityRefusal> {
        self.expire(now);
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
        let max_keys = self.limits.max_keys;
        if !self.project(project).make_room(max_keys) {
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
        self.project(project).receipts.insert(
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
        project: &str,
        preparation_id: &str,
        context: &Context,
    ) -> Result<ApplyOutcome, ApplyRefusal> {
        self.expire(now);
        let digest = context.digest()?;
        if !self.owns(preparation_id) {
            let recorded = self
                .receipts(project)
                .and_then(|receipts| receipts.get(preparation_id))
                .map(|receipt| &receipt.state);
            return Ok(match recorded {
                Some(State::Complete { outcome, .. }) => {
                    ApplyOutcome::Complete { outcome: *outcome }
                }
                _ => ApplyOutcome::Unknown,
            });
        }
        let Some(receipt) = self
            .projects
            .get_mut(project)
            .and_then(|receipts| receipts.receipts.get_mut(preparation_id))
        else {
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
            State::Unknown { .. } => {
                if prepared != Some(digest) {
                    return Err(Refusal::Conflict.into());
                }
                ApplyOutcome::Unknown
            }
        };
        Ok(outcome)
    }

    /// Applied is recorded only when the acknowledgment's applied identity equals the identity the daemon forwarded; an acknowledgment without one leaves the key `Unknown`.
    /// For a key of another incarnation the daemon has no record; it accepts only a minted-shape key with equal, well-formed forwarded and applied identities, then records the classification so the key does not fall back to `Unknown`.
    pub fn confirm(
        &mut self,
        now: Instant,
        project: &str,
        preparation_id: &str,
        forwarded_identity: &str,
        applied_identity: Option<&str>,
        outcome: Outcome,
    ) -> Result<ConfirmOutcome, Refusal> {
        self.expire(now);
        if !self.owns(preparation_id) {
            if let Some(receipt) = self
                .receipts(project)
                .and_then(|receipts| receipts.get(preparation_id))
                && let State::Complete {
                    forwarded_identity: recorded,
                    outcome: known,
                } = &receipt.state
            {
                return if recorded == forwarded_identity
                    && applied_identity == Some(recorded.as_str())
                    && *known == outcome
                {
                    Ok(ConfirmOutcome::Complete { outcome })
                } else {
                    Err(Refusal::Conflict)
                };
            }
            let read_back = applied_identity.is_some_and(|applied| {
                applied == forwarded_identity
                    && well_formed_identity(applied)
                    && well_formed_preparation_id(preparation_id)
            });
            if !read_back {
                return Ok(ConfirmOutcome::Unknown);
            }
            // Answering `complete` for a read-back that is not recorded would let the key fall back to `unknown` and accept a contradictory second outcome.
            let max_keys = self.limits.max_keys;
            let receipts = self.project(project);
            if !receipts.make_room(max_keys) {
                return Err(Refusal::ReceiptUnavailable);
            }
            receipts.receipts.insert(
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
            return Ok(ConfirmOutcome::Complete { outcome });
        }
        let Some(receipt) = self
            .projects
            .get_mut(project)
            .and_then(|receipts| receipts.receipts.get_mut(preparation_id))
        else {
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
        self.projects.values().map(|p| p.receipts.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn holds(&self, project: &str, preparation_id: &str) -> bool {
        self.receipts(project)
            .is_some_and(|receipts| receipts.contains_key(preparation_id))
    }
}

fn fresh_incarnation() -> String {
    let mut nonce = [0u8; INCARNATION_BYTES];
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
    /// A limits change keeps every receipt: the keys stay owned by this incarnation, so an in-flight edit can still be confirmed.
    /// Uninstalling drops the store, and the next install mints a fresh incarnation.
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
                *slot = Some(ReceiptStore::new(limits, fresh_incarnation()));
            }
            (None, _) => *slot = None,
        }
        Ok(())
    }

    /// The bound project scopes every receipt, so a key is honored only on a route of the project that prepared it.
    fn with_receipts<T>(
        &self,
        channel: RouteHandle,
        request: Value,
        operation: &str,
        f: impl FnOnce(&mut ReceiptStore, &str, T) -> PreparedOutcome,
    ) -> PreparedOutcome
    where
        T: serde::de::DeserializeOwned,
    {
        let (scope, parsed) = match self.kernel_request::<T>(channel, request, operation) {
            Ok(bound) => bound,
            Err(outcome) => return outcome,
        };
        let project = scope.project.scope_id();
        let mut slot = self
            .edit_receipts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(store) = slot.as_mut() else {
            return refusal(Refusal::Disabled);
        };
        f(store, &project, parsed)
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
            |store, project, parsed: PrepareRequest| match store.prepare(
                Instant::now(),
                project,
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
                Err(identity) => identity_refusal(PREPARE, identity),
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
            |store, project, parsed: ApplyRequest| match store.apply(
                Instant::now(),
                project,
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
            |store, project, parsed: ConfirmRequest| match store.confirm(
                Instant::now(),
                project,
                &parsed.preparation_id,
                &parsed.forwarded_identity,
                parsed.applied_identity.flatten().as_deref(),
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

    const PROJECT: &str = "project:test";

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
            .prepare(now, PROJECT, &context("rev"), Action::Append, "profile", 1)
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
            assert!(
                store.apply(now, PROJECT, &key, &context("rev")).is_ok(),
                "{step}"
            );
        }
        assert!(matches!(
            store.apply(
                start + Duration::from_secs(10),
                PROJECT,
                &key,
                &context("rev")
            ),
            Err(ApplyRefusal::Refused(Refusal::ReceiptUnavailable))
        ));
        assert!(store.is_empty());
    }

    /// A key another daemon incarnation minted: sixteen hex characters, a dash, sixty-four hex characters.
    fn foreign_key(seed: &str) -> String {
        format!("{}-{}", seed.repeat(8), seed.repeat(32))
    }

    #[test]
    fn a_foreign_key_completes_only_on_a_well_formed_matching_read_back_and_stays_complete() {
        let now = Instant::now();
        let mut store = ReceiptStore::new(limits(8, RETENTION_FLOOR), "inc".to_string());
        let key = &foreign_key("0f");
        assert!(matches!(
            store.apply(now, PROJECT, key, &context("rev")),
            Ok(ApplyOutcome::Unknown)
        ));
        assert!(matches!(
            store.confirm(now, PROJECT, key, "bogus", Some("bogus"), Outcome::Keep),
            Ok(ConfirmOutcome::Unknown)
        ));
        let upper = "AB".repeat(32);
        assert!(matches!(
            store.confirm(now, PROJECT, key, &upper, Some(&upper), Outcome::Keep),
            Ok(ConfirmOutcome::Unknown)
        ));
        let identity = "ab".repeat(32);
        for malformed in ["other-abc", "", &"ab".repeat(40), &identity] {
            assert!(
                matches!(
                    store.confirm(
                        now,
                        PROJECT,
                        malformed,
                        &identity,
                        Some(&identity),
                        Outcome::Keep
                    ),
                    Ok(ConfirmOutcome::Unknown)
                ),
                "{malformed:?}"
            );
            assert!(!store.holds(PROJECT, malformed), "{malformed:?}");
        }
        assert!(matches!(
            store.confirm(
                now,
                PROJECT,
                key,
                &identity,
                Some("cd".repeat(32).as_str()),
                Outcome::Keep
            ),
            Ok(ConfirmOutcome::Unknown)
        ));
        assert!(matches!(
            store.confirm(now, PROJECT, key, &identity, Some(&identity), Outcome::Keep),
            Ok(ConfirmOutcome::Complete {
                outcome: Outcome::Keep
            })
        ));
        assert!(matches!(
            store.apply(now, PROJECT, key, &context("rev")),
            Ok(ApplyOutcome::Complete {
                outcome: Outcome::Keep
            })
        ));
        assert!(matches!(
            store.confirm(
                now,
                PROJECT,
                key,
                &identity,
                Some(&identity),
                Outcome::Append
            ),
            Err(Refusal::Conflict)
        ));
        assert!(
            matches!(
                store.confirm(now, PROJECT, key, &identity, None, Outcome::Keep),
                Err(Refusal::Conflict)
            ),
            "a recorded read-back is judged against its applied identity, not only its forward"
        );
        assert!(matches!(
            store.confirm(
                now,
                PROJECT,
                key,
                &identity,
                Some("cd".repeat(32).as_str()),
                Outcome::Keep
            ),
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
            store.apply(now, PROJECT, &key, &context("rev")),
            Ok(ApplyOutcome::Forwarded { .. })
        ));
        assert!(matches!(
            store.prepare(now, PROJECT, &context("rev"), Action::Append, "profile", 1),
            Ok(PrepareOutcome::Failure("receipt_capacity"))
        ));
        assert!(store.holds(PROJECT, &key));
    }

    #[test]
    fn an_unknown_receipt_is_never_the_victim_because_its_edit_may_be_applied() {
        let now = Instant::now();
        let mut store = ReceiptStore::new(limits(1, RETENTION_FLOOR), "inc".to_string());
        let key = prepared(&mut store, now);
        let Ok(ApplyOutcome::Forwarded {
            forwarded_identity, ..
        }) = store.apply(now, PROJECT, &key, &context("rev"))
        else {
            panic!("first apply forwards");
        };
        assert!(matches!(
            store.confirm(now, PROJECT, &key, &forwarded_identity, None, Outcome::Keep),
            Ok(ConfirmOutcome::Unknown)
        ));
        assert!(matches!(
            store.prepare(now, PROJECT, &context("rev"), Action::Append, "profile", 1),
            Ok(PrepareOutcome::Failure("receipt_capacity"))
        ));
        assert!(store.holds(PROJECT, &key));
        assert!(matches!(
            store.confirm(
                now,
                PROJECT,
                &key,
                &forwarded_identity,
                Some(&forwarded_identity),
                Outcome::Keep
            ),
            Ok(ConfirmOutcome::Complete {
                outcome: Outcome::Keep
            })
        ));
    }

    #[test]
    fn a_refused_request_still_expires_receipts_past_retention() {
        let start = Instant::now();
        let mut store = ReceiptStore::new(limits(8, RETENTION_FLOOR), "inc".to_string());
        prepared(&mut store, start);
        let later = start + RETENTION_FLOOR;
        assert!(matches!(
            store.prepare(
                later,
                PROJECT,
                &context("rev"),
                Action::Append,
                "profile",
                101
            ),
            Ok(PrepareOutcome::Failure("append_allowance"))
        ));
        assert!(store.is_empty(), "an over-capacity prepare expires");
        prepared(&mut store, start);
        let malformed = Context {
            selection: vec!["zz".to_string()],
            ..context("rev")
        };
        assert!(matches!(
            store.apply(later, PROJECT, "inc-x", &malformed),
            Err(ApplyRefusal::Invalid(_))
        ));
        assert!(store.is_empty(), "a malformed apply expires");
    }

    #[test]
    fn a_read_back_the_store_cannot_record_is_refused_rather_than_answered_complete() {
        let now = Instant::now();
        let mut store = ReceiptStore::new(limits(1, RETENTION_FLOOR), "inc".to_string());
        let key = prepared(&mut store, now);
        assert!(matches!(
            store.apply(now, PROJECT, &key, &context("rev")),
            Ok(ApplyOutcome::Forwarded { .. })
        ));
        let foreign = foreign_key("0a");
        let identity = "ab".repeat(32);
        assert!(matches!(
            store.confirm(
                now,
                PROJECT,
                &foreign,
                &identity,
                Some(&identity),
                Outcome::Keep
            ),
            Err(Refusal::ReceiptUnavailable)
        ));
        assert!(!store.holds(PROJECT, &foreign));
        assert!(
            store.holds(PROJECT, &key),
            "the in-flight receipt is never the victim"
        );
        assert!(
            matches!(
                store.apply(now, PROJECT, &foreign, &context("rev")),
                Ok(ApplyOutcome::Unknown)
            ),
            "an unrecorded read-back leaves the key unknown"
        );
    }

    #[test]
    fn a_changed_digest_against_an_unknown_receipt_is_a_conflict() {
        let now = Instant::now();
        let mut store = ReceiptStore::new(limits(8, RETENTION_FLOOR), "inc".to_string());
        let key = prepared(&mut store, now);
        let Ok(ApplyOutcome::Forwarded {
            forwarded_identity, ..
        }) = store.apply(now, PROJECT, &key, &context("rev"))
        else {
            panic!("first apply forwards");
        };
        assert!(matches!(
            store.confirm(
                now,
                PROJECT,
                &key,
                &forwarded_identity,
                None,
                Outcome::Append
            ),
            Ok(ConfirmOutcome::Unknown)
        ));
        assert!(matches!(
            store.apply(now, PROJECT, &key, &context("other")),
            Err(ApplyRefusal::Refused(Refusal::Conflict))
        ));
        assert!(matches!(
            store.apply(now, PROJECT, &key, &context("rev")),
            Ok(ApplyOutcome::Unknown)
        ));
    }

    #[test]
    fn max_keys_is_capped_and_a_narrower_limit_evicts_the_oldest_settled_receipts_at_the_next_mint()
    {
        let start = Instant::now();
        assert!(matches!(
            limits(MAX_KEYS_CEILING + 1, RETENTION_FLOOR).validate(),
            Err(ReceiptLimitsRefusal::MaxKeysAboveCeiling { .. })
        ));
        assert!(limits(MAX_KEYS_CEILING, RETENTION_FLOOR).validate().is_ok());

        let mut store = ReceiptStore::new(limits(8, RETENTION_FLOOR), "inc".to_string());
        let keys: Vec<String> = (0..8)
            .map(|step| prepared(&mut store, start + Duration::from_secs(step)))
            .collect();
        let in_flight = &keys[0];
        assert!(matches!(
            store.apply(start, PROJECT, in_flight, &context("rev")),
            Ok(ApplyOutcome::Forwarded { .. })
        ));
        store.set_limits(limits(3, RETENTION_FLOOR));
        let newest = prepared(&mut store, start + Duration::from_secs(9));
        assert_eq!(store.len(), 3);
        assert!(
            store.holds(PROJECT, in_flight),
            "the in-flight receipt is never the victim"
        );
        assert!(
            store.holds(PROJECT, &keys[7]),
            "the youngest settled receipt survives"
        );
        assert!(store.holds(PROJECT, &newest));
        for evicted in &keys[1..7] {
            assert!(!store.holds(PROJECT, evicted));
        }
    }
}
