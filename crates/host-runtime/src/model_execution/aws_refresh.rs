//! The host incarnation's single AWS profile source owner.
//!
//! The owner holds one admitted source, at most one cached role row, and at most one
//! private SSO successor, all in memory. Demand the cached row cannot satisfy starts one
//! physical refresh on an owner task; concurrent demand waits on that refresh, and a
//! waiter's cancellation or deadline detaches only that waiter. The refresh keeps the
//! owner's single slot until it settles physically, so a replacement never overlaps it.
//! Adoption publishes a row or successor only when the owner is open, its epoch is the
//! refresh's epoch, the admitted graph identity is unchanged, and the transaction did not
//! observe a withdrawal.

use std::ffi::OsString;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::watch;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use super::aws_helper::HelperFailure;
use super::aws_profile::GraphIdentity;
use super::aws_transaction::{
    CaptureFailure, CredentialRow, HelperLimits, OwnerSource, PrivateToken, RenewalEvidence,
    TRANSACTION_BUDGET, TokenObservation, TransactionFailure, TransactionInput, TransactionOutcome,
    TransactionSlot, same_content,
};
use super::backend::{BackendError, BackendTerminal, ErrorClass, Harness};
use super::source_claim;
use super::source_health::{SourceHealthCell, SourceKind, SourceObservation, SourceState};
use super::subprocess::{self, EnvSnapshot, group_registry::StateRoot};

/// A row is usable only when it expires strictly later than the model deadline plus
/// this skew.
pub const EXPIRY_SKEW: Duration = Duration::from_secs(60);
pub const COOLDOWN_BASE: Duration = Duration::from_secs(60);
pub const COOLDOWN_JITTER: Duration = Duration::from_secs(15);
/// The pinned helper runs one attempt per call and reports throttling as an opaque
/// provider error with no typed retry-after, so a cooldown is the base plus jitter, capped
/// here.
pub const MAX_COOLDOWN: Duration = Duration::from_secs(300);
/// A row's computed lifetime is capped here, so an absurd expiry cannot overflow the
/// monotonic clock.
pub const MAX_ROW_LIFETIME: Duration = Duration::from_secs(86_400);

/// What one physical refresh receives from the owner.
pub struct RefreshRequest {
    pub admitted: GraphIdentity,
    pub predecessor: Option<Arc<PrivateToken>>,
    pub superseded: Option<TokenObservation>,
}

pub type RefreshFuture = Pin<Box<dyn Future<Output = TransactionOutcome> + Send>>;

/// One physical refresh. The returned future resolves only after the refresh's blocking
/// reads, helper process group, and scratch cleanup have settled. The owner calls
/// `refresh` under its state lock, so `refresh` only builds the future; all work runs when
/// the future is polled.
pub trait Refresh: Send + Sync + 'static {
    fn refresh(&self, request: RefreshRequest, cancel: CancellationToken) -> RefreshFuture;
}

/// Wall time and cooldown jitter. Monotonic time is `tokio::time::Instant`.
pub trait OwnerClock: Send + Sync + 'static {
    /// Time since the Unix epoch, or `None` when the wall clock reads before it.
    fn wall(&self) -> Option<Duration>;
    /// A uniform draw from `[0, COOLDOWN_JITTER]`.
    fn jitter(&self) -> Duration;
}

pub struct SystemClock;

impl OwnerClock for SystemClock {
    fn wall(&self) -> Option<Duration> {
        SystemTime::now().duration_since(UNIX_EPOCH).ok()
    }

    fn jitter(&self) -> Duration {
        let mut bytes = [0; 8];
        let draw = getrandom::getrandom(&mut bytes).map_or(0, |()| u64::from_le_bytes(bytes));
        let span = COOLDOWN_JITTER.as_millis() as u64 + 1;
        Duration::from_millis(draw % span)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceError {
    Cancelled,
    DeadlineExceeded,
    Shutdown,
    Cooldown {
        retry_in: Duration,
    },
    /// A transient source failure; the source is in cooldown.
    Unavailable,
    /// An external `aws sso login` is needed.
    LoginRequired,
    /// The source no longer matches its admission, or its cleanup is unproven.
    Invalid,
    /// The owner token changed during the refresh, invalidating the cached material.
    Withdrawn,
    /// The newest row cannot outlive the deadline plus [`EXPIRY_SKEW`].
    InsufficientLifetime,
}

/// A cached row with the two expiry bounds the lifetime predicate checks.
#[derive(Clone)]
pub struct Lease {
    pub row: Arc<CredentialRow>,
    usable_until: Instant,
    expires_at_wall: Duration,
}

struct Cached {
    lease: Lease,
    /// The owner token observation the row was minted under.
    basis: Option<TokenObservation>,
}

#[derive(Default)]
struct State {
    epoch: u64,
    row: Option<Cached>,
    successor: Option<Arc<PrivateToken>>,
    superseded: Option<TokenObservation>,
    in_flight: Option<watch::Receiver<bool>>,
    cooldown_until: Option<Instant>,
    failure: Option<SourceError>,
    failures: u32,
    unresolved: bool,
    /// A refresh retained its crash-ownership record.
    record_retained: bool,
    closed: bool,
}

impl State {
    /// The fence persists across missing, unreadable, or same-content observations so
    /// a superseded token still on disk stays fenced. A different present token clears
    /// the fence when `renewal_lost` is false.
    fn observe_fence(&mut self, observed: &TokenObservation, renewal_lost: bool) {
        let present = matches!(observed, TokenObservation::Present { .. });
        if renewal_lost {
            if present || !self.fences_present_token() {
                self.superseded = Some(observed.clone());
            }
        } else if present
            && self
                .superseded
                .as_ref()
                .is_none_or(|fenced| !same_content(fenced, observed))
        {
            self.superseded = None;
        }
    }

    /// The successor's rotation consumed the owner token it was minted from, so
    /// offering that token again would replay a rotated refresh token.
    fn discard_successor(&mut self) {
        if let Some(successor) = self.successor.take()
            && !self.fences_present_token()
        {
            self.superseded = Some(successor.basis().clone());
        }
    }

    fn fences_present_token(&self) -> bool {
        matches!(self.superseded, Some(TokenObservation::Present { .. }))
    }
}

struct Inner {
    refresh: Box<dyn Refresh>,
    clock: Box<dyn OwnerClock>,
    admitted: GraphIdentity,
    health: SourceHealthCell,
    cancel: CancellationToken,
    tasks: TaskTracker,
    state: RwLock<State>,
}

/// A shared handle. Hosts call [`SourceOwner::close`] and [`SourceOwner::join`] at
/// shutdown; dropping every handle leaves a running refresh to settle on its own task.
#[derive(Clone)]
pub struct SourceOwner(Arc<Inner>);

impl SourceOwner {
    pub fn new(
        refresh: impl Refresh,
        clock: impl OwnerClock,
        admitted: GraphIdentity,
        health: SourceHealthCell,
    ) -> Self {
        health.set(SourceObservation::unknown(SourceKind::Profile));
        Self(Arc::new(Inner {
            refresh: Box::new(refresh),
            clock: Box::new(clock),
            admitted,
            health,
            cancel: CancellationToken::new(),
            tasks: TaskTracker::new(),
            state: RwLock::new(State::default()),
        }))
    }

    /// Returns a row that expires strictly after `deadline` plus [`EXPIRY_SKEW`]. A warm
    /// row returns without credential I/O. Otherwise this waits, until `deadline` or
    /// `cancel`, on the single refresh, starting it when none runs.
    pub async fn acquire(
        &self,
        deadline: Instant,
        cancel: &CancellationToken,
    ) -> Result<Lease, SourceError> {
        // Warm hits share the read lock. A miss checks again under the exclusive lock
        // before it joins or starts the refresh.
        if let Some(found) = self.0.usable(&self.0.read(), deadline) {
            return found;
        }
        let mut settled = {
            let mut state = self.0.lock();
            if let Some(found) = self.0.usable(&state, deadline) {
                return found;
            }
            // On a miss, cancellation or deadline expiry ends the demand before any
            // credential I/O begins.
            if cancel.is_cancelled() {
                return Err(SourceError::Cancelled);
            }
            if deadline <= Instant::now() {
                return Err(SourceError::DeadlineExceeded);
            }
            match &state.in_flight {
                Some(settled) => settled.clone(),
                None => {
                    let now = Instant::now();
                    if state.unresolved {
                        return Err(SourceError::Invalid);
                    }
                    if let Some(until) = state.cooldown_until.filter(|until| *until > now) {
                        return Err(SourceError::Cooldown {
                            retry_in: until - now,
                        });
                    }
                    self.start(&mut state, deadline)
                }
            }
        };
        tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(SourceError::Cancelled),
            () = tokio::time::sleep_until(deadline) => return Err(SourceError::DeadlineExceeded),
            _ = settled.wait_for(|done| *done) => {}
        }
        let state = self.0.read();
        self.0.usable(&state, deadline).unwrap_or(Err(state
            .failure
            .unwrap_or(SourceError::InsufficientLifetime)))
    }

    /// The cell this owner publishes its observation to.
    pub fn health(&self) -> SourceHealthCell {
        self.0.health.clone()
    }

    /// Discards the cached row and successor and fences any running refresh out of
    /// adoption.
    pub fn withdraw(&self) {
        let mut state = self.0.lock();
        state.epoch += 1;
        state.row = None;
        state.discard_successor();
        self.0.publish(&state);
    }

    /// Fences admission and adoption and signals the running refresh to cancel.
    pub fn close(&self) {
        let mut state = self.0.lock();
        state.closed = true;
        state.row = None;
        state.successor = None;
        self.0.publish(&state);
        drop(state);
        self.0.cancel.cancel();
        self.0.tasks.close();
    }

    /// Closes the owner and waits for its refresh to settle physically. Returns whether
    /// every refresh proved its cleanup, including removal of its crash-ownership record.
    pub async fn join(&self) -> bool {
        self.close();
        self.0.tasks.wait().await;
        let state = self.0.read();
        !(state.unresolved || state.record_retained)
    }

    /// Starts the single refresh for the demand that ends at `demand`. The refresh runs
    /// on its own task with the fixed [`TRANSACTION_BUDGET`], so a waiter's deadline
    /// bounds only that waiter. A refresh that panics leaves the owner unresolved.
    fn start(&self, state: &mut State, demand: Instant) -> watch::Receiver<bool> {
        let (done, settled) = watch::channel(false);
        state.in_flight = Some(settled.clone());
        let epoch = state.epoch;
        let request = RefreshRequest {
            admitted: self.0.admitted.clone(),
            predecessor: state.successor.clone(),
            superseded: state.superseded.clone(),
        };
        let physical = self.0.refresh.refresh(request, self.0.cancel.child_token());
        let physical = self.0.tasks.spawn(physical);
        self.0.publish(state);
        let inner = Arc::clone(&self.0);
        self.0.tasks.spawn(async move {
            let outcome = physical.await.unwrap_or_else(|_| unsettled());
            inner.adopt(epoch, demand, outcome);
            done.send_replace(true);
        });
        settled
    }
}

/// The outcome of a refresh whose task ended without settling.
fn unsettled() -> TransactionOutcome {
    TransactionOutcome {
        identity: None,
        row: None,
        successor: None,
        observation: None,
        renewal: RenewalEvidence::Unknown,
        lost_succession: true,
        record_retained: false,
        failure: Some(TransactionFailure::CleanupUnproven),
    }
}

/// Whether a failure discards the cached row and private successor.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Authority {
    Keep,
    Discard,
}

impl Inner {
    fn lock(&self) -> RwLockWriteGuard<'_, State> {
        self.state.write().unwrap_or_else(PoisonError::into_inner)
    }

    fn read(&self) -> RwLockReadGuard<'_, State> {
        self.state.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn usable(&self, state: &State, deadline: Instant) -> Option<Result<Lease, SourceError>> {
        if state.closed {
            return Some(Err(SourceError::Shutdown));
        }
        let lease = &state.row.as_ref()?.lease;
        self.covers(lease, deadline).then(|| Ok(lease.clone()))
    }

    /// The lifetime predicate: `lease` expires strictly after `deadline` plus
    /// [`EXPIRY_SKEW`] by both its monotonic bound and the current wall clock.
    fn covers(&self, lease: &Lease, deadline: Instant) -> bool {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let monotonic = deadline
            .checked_add(EXPIRY_SKEW)
            .is_some_and(|needed| lease.usable_until > needed);
        let wall = self
            .clock
            .wall()
            .and_then(|wall| wall.checked_add(remaining)?.checked_add(EXPIRY_SKEW))
            .is_some_and(|needed| lease.expires_at_wall > needed);
        monotonic && wall
    }

    fn adopt(&self, epoch: u64, demand: Instant, mut outcome: TransactionOutcome) {
        let mut state = self.lock();
        state.in_flight = None;
        state.unresolved |= outcome.failure == Some(TransactionFailure::CleanupUnproven);
        state.record_retained |= outcome.record_retained;
        let stale = epoch != state.epoch;
        let verdict = if outcome
            .identity
            .as_ref()
            .is_some_and(|identity| *identity != self.admitted)
        {
            Some((SourceError::Invalid, Authority::Discard))
        } else {
            outcome.failure.map(classify)
        };
        let discard = matches!(verdict, Some((_, Authority::Discard)));
        let login = matches!(verdict, Some((SourceError::LoginRequired, _)));
        // A successor discarded by a fence or verdict was a rotation the owner cannot keep.
        let rotated =
            outcome.lost_succession || ((stale || discard) && outcome.successor.is_some());
        let renewal_lost = rotated || login;
        if let Some(observed) = &outcome.observation {
            state.observe_fence(observed, renewal_lost);
        }
        if stale {
            state.failure = Some(SourceError::Withdrawn);
        }
        if state.closed || stale || outcome.failure == Some(TransactionFailure::Cancelled) {
            self.publish(&state);
            return;
        }
        let invalidated = discard || withdrawn(state.row.as_ref(), outcome.observation.as_ref());
        if invalidated {
            state.row = None;
        }
        match (discard, outcome.successor.take()) {
            (false, Some(successor)) => state.successor = Some(Arc::new(successor)),
            (false, None) if !invalidated && !renewal_lost => {}
            _ => state.discard_successor(),
        }
        let failure = match (verdict, outcome.row.take()) {
            (Some((error, _)), _) => Some(error),
            (None, None) => Some(SourceError::Unavailable),
            (None, Some(row)) => self.cache(&mut state, row, outcome.observation, demand),
        };
        state.failure = failure;
        state.failures = failure.map_or(0, |_| state.failures.saturating_add(1));
        // An observed withdrawal already discarded authority, and a login-required source
        // refuses before the helper until the owner token changes, so neither waits out a
        // cooldown before the next demand observes the owner token again.
        state.cooldown_until = failure
            .filter(|failure| {
                !matches!(failure, SourceError::Withdrawn | SourceError::LoginRequired)
            })
            .map(|_| Instant::now() + (COOLDOWN_BASE + self.clock.jitter()).min(MAX_COOLDOWN));
        self.publish(&state);
    }

    /// Caches `row` with a monotonic expiry computed now. A row that cannot cover the
    /// demand that started its refresh stays cached for shorter demands and reports
    /// [`SourceError::InsufficientLifetime`], which cools the source down.
    fn cache(
        &self,
        state: &mut State,
        row: CredentialRow,
        basis: Option<TokenObservation>,
        demand: Instant,
    ) -> Option<SourceError> {
        let Some(wall) = self.clock.wall() else {
            return Some(SourceError::Unavailable);
        };
        let lifetime = Duration::from_secs(row.expires_at_unix_seconds)
            .saturating_sub(wall)
            .min(MAX_ROW_LIFETIME);
        if lifetime <= EXPIRY_SKEW {
            return Some(SourceError::InsufficientLifetime);
        }
        let usable_until = Instant::now() + lifetime;
        state.row = Some(Cached {
            lease: Lease {
                row: Arc::new(row),
                usable_until,
                expires_at_wall: wall + lifetime,
            },
            basis,
        });
        demand
            .checked_add(EXPIRY_SKEW)
            .is_none_or(|needed| usable_until <= needed)
            .then_some(SourceError::InsufficientLifetime)
    }

    fn publish(&self, state: &State) {
        let now = Instant::now();
        // A row serves demand only while `usable_until` exceeds the deadline plus the
        // skew, so ready health ends `EXPIRY_SKEW` before `usable_until`.
        let usable = state
            .row
            .as_ref()
            .and_then(|row| row.lease.usable_until.checked_sub(EXPIRY_SKEW))
            .filter(|boundary| *boundary > now);
        let cooling = state.cooldown_until.filter(|until| *until > now);
        let condition = if state.closed {
            SourceState::Unknown
        } else if state.unresolved {
            SourceState::Invalid
        } else if state.in_flight.is_some() {
            SourceState::Refreshing
        } else {
            match state.failure {
                Some(SourceError::LoginRequired) => SourceState::LoginRequired,
                Some(SourceError::Invalid) => SourceState::Invalid,
                Some(_) if cooling.is_some() => SourceState::Cooldown,
                _ if usable.is_some() => SourceState::Ready,
                _ => SourceState::Unknown,
            }
        };
        self.health.set(SourceObservation {
            kind: SourceKind::Profile,
            state: condition,
            expires_at: usable,
            retry_at: cooling.filter(|_| condition == SourceState::Cooldown),
            consecutive_failures: state.failures,
        });
    }
}

/// A refresh that observed the owner token deleted, unusable, or changed from the
/// observation a cached row was minted under withdraws that row.
fn withdrawn(row: Option<&Cached>, observed: Option<&TokenObservation>) -> bool {
    let Some(observed) = observed else {
        return false;
    };
    let changed = row
        .and_then(|row| row.basis.as_ref())
        .is_some_and(|basis| !same_content(basis, observed));
    changed || !matches!(observed, TokenObservation::Present { .. })
}

fn classify(failure: TransactionFailure) -> (SourceError, Authority) {
    use Authority::{Discard, Keep};
    use TransactionFailure as F;
    match failure {
        F::LoginRequired | F::Helper(HelperFailure::NotLoaded) => {
            (SourceError::LoginRequired, Keep)
        }
        F::Withdrawn => (SourceError::Withdrawn, Discard),
        F::GraphChanged | F::Admission(_) | F::Helper(HelperFailure::InvalidConfiguration) => {
            (SourceError::Invalid, Discard)
        }
        F::Capture(CaptureFailure::Missing | CaptureFailure::Unsafe | CaptureFailure::TooLarge)
        | F::Helper(HelperFailure::InvalidInput | HelperFailure::DestinationRefused)
        | F::CleanupUnproven => (SourceError::Invalid, Keep),
        F::Capture(CaptureFailure::Mutated | CaptureFailure::Io)
        | F::Helper(
            HelperFailure::ProviderError | HelperFailure::Incomplete | HelperFailure::Unhandled,
        )
        | F::HelperUnreported
        | F::Spawn
        | F::BudgetExhausted
        | F::Cancelled => (SourceError::Unavailable, Keep),
    }
}

/// The profile source as the model adapters use it: the shared owner and the selected
/// region the child row carries.
#[derive(Clone)]
pub struct AwsDispatch(Arc<Dispatch>);

struct Dispatch {
    owner: SourceOwner,
    region: String,
}

impl AwsDispatch {
    pub fn new(owner: SourceOwner, region: String) -> Self {
        Self(Arc::new(Dispatch { owner, region }))
    }

    pub fn owner(&self) -> &SourceOwner {
        &self.0.owner
    }
}

/// The lease one child's row came from and the dispatch's owner, kept for the recheck
/// immediately before spawn.
pub(super) struct ChildLease<'a> {
    lease: Lease,
    owner: &'a SourceOwner,
}

/// The child's credential row and, for a profile source, the lease to recheck before
/// spawn. A selected profile source answers Bedrock; every other provider reads the
/// startup environment row.
pub(super) async fn child_credentials<'a>(
    env: &EnvSnapshot,
    aws: Option<&'a AwsDispatch>,
    harness: Harness,
    provider: &str,
    deadline: Instant,
    cancel: &CancellationToken,
) -> Result<(Vec<(OsString, OsString)>, Option<ChildLease<'a>>), BackendTerminal> {
    let canonical = subprocess::canonical_provider(harness.as_str(), provider);
    let Some(aws) = aws.filter(|_| canonical == Ok(source_claim::PROFILE_PROVIDER)) else {
        return env
            .provider_row(harness.as_str(), provider)
            .map(|row| (row, None))
            .map_err(|error| subprocess::credential_failure(harness, error));
    };
    let lease = aws
        .owner()
        .acquire(deadline, cancel)
        .await
        .map_err(|error| source_terminal(harness, error))?;
    let row = &lease.row;
    let env = vec![
        (
            "AWS_ACCESS_KEY_ID".into(),
            row.access_key_id.as_str().into(),
        ),
        (
            "AWS_SECRET_ACCESS_KEY".into(),
            row.secret_access_key.as_str().into(),
        ),
        (
            "AWS_SESSION_TOKEN".into(),
            row.session_token.as_str().into(),
        ),
        ("AWS_REGION".into(), aws.0.region.as_str().into()),
    ];
    let owner = aws.owner();
    Ok((env, Some(ChildLease { lease, owner })))
}

/// The recheck immediately before a child spawns, under the same lifetime predicate and
/// clock as acquisition.
pub(super) fn recheck_before_spawn(
    lease: Option<&ChildLease<'_>>,
    harness: Harness,
    deadline: Instant,
) -> Result<(), BackendTerminal> {
    match lease {
        // A forward wall-clock jump during setup causes a transient failure when the lease
        // stops covering the run deadline.
        Some(child) if !child.owner.0.covers(&child.lease, deadline) => Err(source_failed(
            harness,
            ErrorClass::Transient,
            "row stopped covering the run deadline before spawn",
            None,
        )),
        _ => Ok(()),
    }
}

/// The accepted credential-source terminal for one acquisition failure. A cancelled run
/// or a closing owner ends as a setup abort, which is not a source failure.
fn source_terminal(harness: Harness, error: SourceError) -> BackendTerminal {
    let (class, detail, retry_after_secs) = match error {
        SourceError::Cancelled | SourceError::Shutdown => {
            return subprocess::setup_aborted_terminal(harness, subprocess::SetupAbort::Cancelled);
        }
        SourceError::Cooldown { retry_in } => (
            ErrorClass::Transient,
            "is cooling down after a failed refresh",
            // Rounding `retry_in` up keeps `retry_after_secs` covering the remaining cooldown.
            Some((retry_in.as_secs() + u64::from(retry_in.subsec_nanos() > 0)).max(1)),
        ),
        SourceError::Unavailable | SourceError::Withdrawn => {
            (ErrorClass::Transient, "is unavailable", None)
        }
        SourceError::DeadlineExceeded => (
            ErrorClass::Transient,
            "did not answer within the run budget",
            None,
        ),
        SourceError::LoginRequired => (
            ErrorClass::AuthRequired,
            "needs a new `aws sso login`",
            None,
        ),
        SourceError::Invalid => (
            ErrorClass::Permanent,
            "no longer matches its admitted configuration",
            None,
        ),
        SourceError::InsufficientLifetime => (
            ErrorClass::Permanent,
            "has no row that outlives the run deadline",
            None,
        ),
    };
    source_failed(harness, class, detail, retry_after_secs)
}

fn source_failed(
    harness: Harness,
    class: ErrorClass,
    detail: &str,
    retry_after_secs: Option<u64>,
) -> BackendTerminal {
    BackendTerminal::SourceFailed(BackendError {
        class,
        message: format!("{} AWS credential source {detail}", harness.as_str()),
        retry_after_secs,
        provider_code: None,
    })
}

/// The production [`Refresh`]: one [`TransactionSlot`] over the owner's files.
pub struct TransactionRefresh(Arc<TransactionContext>);

struct TransactionContext {
    source: OwnerSource,
    executable: PathBuf,
    state_root: StateRoot,
    limits: HelperLimits,
    test_origin: Option<String>,
    slot: tokio::sync::Mutex<TransactionSlot>,
}

impl TransactionRefresh {
    pub fn new(
        source: OwnerSource,
        executable: PathBuf,
        state_root: StateRoot,
        limits: HelperLimits,
        test_origin: Option<String>,
    ) -> Self {
        Self(Arc::new(TransactionContext {
            source,
            executable,
            state_root,
            limits,
            test_origin,
            slot: tokio::sync::Mutex::default(),
        }))
    }
}

impl Refresh for TransactionRefresh {
    fn refresh(&self, request: RefreshRequest, cancel: CancellationToken) -> RefreshFuture {
        let context = Arc::clone(&self.0);
        Box::pin(async move {
            let mut slot = context.slot.lock().await;
            let input = TransactionInput {
                source: &context.source,
                admitted: Some(&request.admitted),
                predecessor: request.predecessor.as_deref(),
                superseded: request.superseded.as_ref(),
                executable: &context.executable,
                state_root: &context.state_root,
                budget: TRANSACTION_BUDGET,
                limits: context.limits.clone(),
                test_origin: context.test_origin.clone(),
            };
            slot.run(input, &cancel).await
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

    use tokio::sync::oneshot;
    use zeroize::Zeroizing;

    use super::*;
    use crate::model_execution::aws_profile::{AdmissionError, RootIdentity};
    use crate::model_execution::aws_transaction::RenewalEvidence;

    fn identity(account: &str) -> GraphIdentity {
        GraphIdentity {
            profile: "dev".into(),
            region: "us-west-2".into(),
            roles: Vec::new(),
            root: RootIdentity::Sso {
                profile: "dev".into(),
                session_name: "corp".into(),
                start_url: "https://example.awsapps.com/start".into(),
                sso_region: "us-east-1".into(),
                account_id: account.into(),
                role_name: "Dev".into(),
            },
        }
    }

    fn observed(n: u8) -> TokenObservation {
        TokenObservation::Present {
            identity: [i64::from(n); 7],
            sha256: [n; 32],
        }
    }

    /// The predecessor bytes, superseded observation, and cancellation one refresh received.
    type Seen = (Option<Vec<u8>>, Option<TokenObservation>, CancellationToken);

    type Case<T> = (&'static str, fn(&Harness) -> T);

    struct Step {
        gate: Option<oneshot::Receiver<()>>,
        outcome: TransactionOutcome,
    }

    #[derive(Default)]
    struct Script {
        steps: Mutex<VecDeque<Step>>,
        requests: Mutex<Vec<Seen>>,
        running: AtomicUsize,
        overlapped: AtomicUsize,
    }

    #[derive(Clone, Default)]
    struct Fake(Arc<Script>);

    impl Fake {
        fn push(&self, outcome: TransactionOutcome) {
            self.0.steps.lock().unwrap().push_back(Step {
                gate: None,
                outcome,
            });
        }

        /// The refresh settles physically only after the returned sender fires.
        fn push_held(&self, outcome: TransactionOutcome) -> oneshot::Sender<()> {
            let (release, gate) = oneshot::channel();
            self.0.steps.lock().unwrap().push_back(Step {
                gate: Some(gate),
                outcome,
            });
            release
        }

        fn calls(&self) -> usize {
            self.0.requests.lock().unwrap().len()
        }

        fn request(&self, index: usize) -> Seen {
            self.0.requests.lock().unwrap()[index].clone()
        }
    }

    impl Refresh for Fake {
        fn refresh(&self, request: RefreshRequest, cancel: CancellationToken) -> RefreshFuture {
            let script = Arc::clone(&self.0);
            script.requests.lock().unwrap().push((
                request.predecessor.map(|token| token.bytes().to_vec()),
                request.superseded,
                cancel,
            ));
            let step = script
                .steps
                .lock()
                .unwrap()
                .pop_front()
                .expect("scripted step");
            Box::pin(async move {
                if script.running.fetch_add(1, Ordering::SeqCst) > 0 {
                    script.overlapped.fetch_add(1, Ordering::SeqCst);
                }
                if let Some(gate) = step.gate {
                    let _ = gate.await;
                }
                script.running.fetch_sub(1, Ordering::SeqCst);
                step.outcome
            })
        }
    }

    /// Wall time follows the paused monotonic clock plus an adjustable offset.
    #[derive(Clone)]
    struct Clock {
        start: Instant,
        offset: Arc<AtomicI64>,
    }

    const WALL_BASE: Duration = Duration::from_secs(1_700_000_000);

    impl Clock {
        fn new() -> Self {
            Self {
                start: Instant::now(),
                offset: Arc::new(AtomicI64::new(0)),
            }
        }

        fn shift(&self, seconds: i64) {
            self.offset.fetch_add(seconds, Ordering::SeqCst);
        }
    }

    impl OwnerClock for Clock {
        fn wall(&self) -> Option<Duration> {
            let base = WALL_BASE + self.start.elapsed();
            let offset = self.offset.load(Ordering::SeqCst);
            if offset >= 0 {
                Some(base + Duration::from_secs(offset.unsigned_abs()))
            } else {
                base.checked_sub(Duration::from_secs(offset.unsigned_abs()))
            }
        }

        fn jitter(&self) -> Duration {
            COOLDOWN_JITTER
        }
    }

    struct Harness {
        never: CancellationToken,
        owner: SourceOwner,
        fake: Fake,
        clock: Clock,
        health: SourceHealthCell,
    }

    fn harness() -> Harness {
        let fake = Fake::default();
        let clock = Clock::new();
        let health = SourceHealthCell::new(SourceObservation::unknown(SourceKind::Profile));
        let owner = SourceOwner::new(
            fake.clone(),
            clock.clone(),
            identity("111111111111"),
            health.clone(),
        );
        Harness {
            never: CancellationToken::new(),
            owner,
            fake,
            clock,
            health,
        }
    }

    impl Harness {
        fn outcome(
            &self,
            lifetime: Option<u64>,
            failure: Option<TransactionFailure>,
        ) -> TransactionOutcome {
            let wall = self.clock.wall().map_or(0, |wall| wall.as_secs());
            TransactionOutcome {
                identity: Some(identity("111111111111")),
                row: lifetime.map(|seconds| CredentialRow {
                    access_key_id: format!("ASIA{seconds}"),
                    secret_access_key: Zeroizing::new("secret".into()),
                    session_token: Zeroizing::new("session".into()),
                    expires_at_unix_seconds: wall + seconds,
                }),
                successor: None,
                observation: Some(observed(1)),
                renewal: RenewalEvidence::NotStarted,
                lost_succession: false,
                record_retained: false,
                failure,
            }
        }

        fn ok(&self, lifetime: u64) -> TransactionOutcome {
            self.outcome(Some(lifetime), None)
        }

        fn failed(&self, failure: TransactionFailure) -> TransactionOutcome {
            self.outcome(None, Some(failure))
        }

        async fn acquire_for(&self, model: Duration) -> Result<Lease, SourceError> {
            self.owner
                .acquire(Instant::now() + model, &CancellationToken::new())
                .await
        }
    }

    const MODEL: Duration = Duration::from_secs(660);
    const HOUR: u64 = 3_600;

    #[tokio::test(start_paused = true)]
    async fn concurrent_demand_shares_one_refresh_and_warm_hits_do_no_credential_io() {
        let h = harness();
        let release = h.fake.push_held(h.ok(HOUR));
        let mut waiters = tokio::task::JoinSet::new();
        for _ in 0..32 {
            let owner = h.owner.clone();
            waiters.spawn(async move {
                owner
                    .acquire(Instant::now() + MODEL, &CancellationToken::new())
                    .await
            });
        }
        tokio::task::yield_now().await;
        let _ = release.send(());
        let rows = waiters.join_all().await;
        assert!(rows.iter().all(|row| {
            row.as_ref()
                .is_ok_and(|r| r.row.access_key_id == "ASIA3600")
        }));
        assert_eq!(h.fake.calls(), 1, "32 waiters share one physical refresh");
        for _ in 0..100 {
            h.acquire_for(MODEL).await.expect("warm row");
        }
        assert_eq!(h.fake.calls(), 1, "a warm acquisition starts no refresh");
        assert_eq!(h.health.get().state, SourceState::Ready);
        assert_eq!(
            h.health.get().expires_in_seconds,
            Some(HOUR - EXPIRY_SKEW.as_secs())
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_already_cancelled_or_expired_demand_starts_no_refresh() {
        let h = harness();
        h.fake.push(h.ok(HOUR));
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(
            h.owner.acquire(Instant::now() + MODEL, &cancel).await.err(),
            Some(SourceError::Cancelled)
        );
        let expired = Instant::now() - Duration::from_secs(1);
        assert_eq!(
            h.owner.acquire(expired, &h.never).await.err(),
            Some(SourceError::DeadlineExceeded)
        );
        assert_eq!(
            h.fake.calls(),
            0,
            "an aborted demand starts no credential I/O"
        );
        assert_eq!(h.health.get().state, SourceState::Unknown);
    }

    #[tokio::test(start_paused = true)]
    async fn a_cancelled_or_expired_waiter_detaches_while_the_refresh_keeps_its_slot() {
        let h = harness();
        let release = h.fake.push_held(h.ok(HOUR));
        let cancel = CancellationToken::new();
        let cancelled = h.owner.acquire(Instant::now() + MODEL, &cancel);
        let short = h
            .owner
            .acquire(Instant::now() + Duration::from_secs(5), &h.never);
        cancel.cancel();
        assert_eq!(cancelled.await.err(), Some(SourceError::Cancelled));
        assert_eq!(short.await.err(), Some(SourceError::DeadlineExceeded));
        assert_eq!(h.health.get().state, SourceState::Refreshing);

        tokio::time::advance(Duration::from_secs(120)).await;
        let joined = h.owner.acquire(Instant::now() + MODEL, &h.never);
        let late = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let _ = release.send(());
        });
        assert!(
            joined.await.is_ok(),
            "a stalled refresh with zero waiters still adopts"
        );
        late.await.unwrap();
        assert_eq!(
            h.fake.calls(),
            1,
            "no replacement refresh overlaps the stalled one"
        );
        assert_eq!(h.fake.0.overlapped.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn ready_health_ends_where_the_row_stops_serving_demand() {
        let h = harness();
        h.fake.push(h.ok(HOUR));
        h.acquire_for(MODEL).await.expect("row");
        let skew = EXPIRY_SKEW.as_secs();
        assert_eq!(h.health.get().expires_in_seconds, Some(HOUR - skew));
        tokio::time::advance(Duration::from_secs(HOUR - skew - 1)).await;
        let health = h.health.get();
        assert_eq!(
            (health.state, health.expires_in_seconds),
            (SourceState::Ready, Some(1))
        );
        tokio::time::advance(Duration::from_secs(1)).await;
        let health = h.health.get();
        assert_eq!(
            (health.state, health.expires_in_seconds),
            (SourceState::Unknown, None),
            "a row inside the skew serves no demand"
        );
        assert_eq!(h.fake.calls(), 1, "the health read starts no refresh");
    }

    #[tokio::test(start_paused = true)]
    async fn a_waiter_dead_on_arrival_starts_no_refresh() {
        let h = harness();
        let _held = h.fake.push_held(h.ok(HOUR));
        tokio::time::advance(Duration::from_secs(2)).await;
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(
            h.owner.acquire(Instant::now() + MODEL, &cancel).await.err(),
            Some(SourceError::Cancelled)
        );
        assert_eq!(
            h.owner
                .acquire(Instant::now() - Duration::from_secs(1), &h.never)
                .await
                .err(),
            Some(SourceError::DeadlineExceeded)
        );
        assert_eq!(h.fake.calls(), 0, "dead demand starts no credential I/O");
        assert_eq!(h.health.get().state, SourceState::Unknown);
    }

    #[tokio::test(start_paused = true)]
    async fn a_retained_crash_record_keeps_the_source_usable_and_fails_the_join() {
        let h = harness();
        let mut retained = h.ok(HOUR);
        retained.record_retained = true;
        h.fake.push(retained);
        h.acquire_for(MODEL).await.expect("row");
        h.acquire_for(MODEL)
            .await
            .expect("a retained record leaves the source usable");
        assert_eq!(h.fake.calls(), 1);
        assert!(
            !h.owner.join().await,
            "a retained crash record is cleanup debt at shutdown"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn lifetime_must_exceed_the_deadline_plus_skew_strictly() {
        let skew = EXPIRY_SKEW.as_secs();
        let model = MODEL.as_secs();
        for (lifetime, usable) in [
            (model + skew - 1, false),
            (model + skew, false),
            (model + skew + 1, true),
        ] {
            let h = harness();
            h.fake.push(h.ok(lifetime));
            let first = h.acquire_for(MODEL).await;
            assert_eq!(first.is_ok(), usable, "lifetime {lifetime}");
            if !usable {
                assert_eq!(first.err(), Some(SourceError::InsufficientLifetime));
                assert!(
                    matches!(
                        h.acquire_for(MODEL).await,
                        Err(SourceError::Cooldown { .. })
                    ),
                    "a row too short for its demand cools the source down"
                );
            }
        }
        let h = harness();
        h.fake.push(h.ok(skew));
        assert_eq!(
            h.acquire_for(Duration::from_secs(1)).await.err(),
            Some(SourceError::InsufficientLifetime),
            "a row at the skew is refused"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn clock_rollback_never_extends_life_and_a_forward_jump_shortens_it() {
        let h = harness();
        h.fake.push(h.ok(MODEL.as_secs() + 600));
        h.acquire_for(MODEL).await.expect("row");
        h.clock.shift(-(HOUR as i64));
        tokio::time::advance(Duration::from_secs(540)).await;
        h.fake.push(h.failed(TransactionFailure::Spawn));
        assert_eq!(
            h.acquire_for(MODEL).await.err(),
            Some(SourceError::Unavailable),
            "the monotonic bound holds after rollback"
        );
        assert_eq!(h.fake.calls(), 2);

        let h = harness();
        h.fake.push(h.ok(MODEL.as_secs() + 600));
        h.acquire_for(MODEL).await.expect("row");
        h.clock.shift(540);
        h.fake.push(h.failed(TransactionFailure::Spawn));
        assert_eq!(
            h.acquire_for(MODEL).await.err(),
            Some(SourceError::Unavailable),
            "a forward wall jump shortens life"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn transient_failure_cools_down_for_base_plus_jitter_and_counts_failures() {
        let h = harness();
        h.fake.push(h.failed(TransactionFailure::HelperUnreported));
        assert_eq!(
            h.acquire_for(MODEL).await.err(),
            Some(SourceError::Unavailable)
        );
        let health = h.health.get();
        assert_eq!(
            (health.state, health.consecutive_failures),
            (SourceState::Cooldown, 1)
        );
        assert_eq!(health.next_retry_in_seconds, Some(75));
        tokio::time::advance(Duration::from_secs(74)).await;
        assert!(
            matches!(h.acquire_for(MODEL).await, Err(SourceError::Cooldown { retry_in }) if retry_in == Duration::from_secs(1))
        );
        assert_eq!(h.fake.calls(), 1, "cooldown starts no refresh");
        tokio::time::advance(Duration::from_secs(1)).await;
        h.fake
            .push(h.failed(TransactionFailure::Helper(HelperFailure::ProviderError)));
        assert_eq!(
            h.acquire_for(MODEL).await.err(),
            Some(SourceError::Unavailable)
        );
        assert_eq!(h.health.get().consecutive_failures, 2);
        tokio::time::advance(COOLDOWN_BASE + COOLDOWN_JITTER).await;
        h.fake.push(h.ok(HOUR));
        h.acquire_for(MODEL).await.expect("row");
        let health = h.health.get();
        assert_eq!(
            (
                health.state,
                health.consecutive_failures,
                health.next_retry_in_seconds
            ),
            (SourceState::Ready, 0, None)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_expired_cooldown_reads_from_the_cached_row_without_a_republish() {
        let h = harness();
        h.fake.push(h.ok(MODEL.as_secs()));
        assert_eq!(
            h.acquire_for(MODEL).await.err(),
            Some(SourceError::InsufficientLifetime)
        );
        assert_eq!(h.health.get().state, SourceState::Cooldown);
        tokio::time::advance(COOLDOWN_BASE + COOLDOWN_JITTER).await;
        h.acquire_for(Duration::from_secs(60))
            .await
            .expect("warm row for a short demand");
        assert_eq!(h.fake.calls(), 1, "a warm hit starts no refresh");
        let health = h.health.get();
        assert_eq!(
            (
                health.state,
                health.next_retry_in_seconds,
                health.expires_in_seconds,
                health.consecutive_failures
            ),
            (
                SourceState::Ready,
                None,
                Some(MODEL.as_secs() - 75 - EXPIRY_SKEW.as_secs()),
                1
            ),
            "an elapsed cooldown over a usable row reads ready at the health read"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_successor_survives_role_failure_and_feeds_the_next_refresh() {
        let h = harness();
        let mut first = h.ok(MODEL.as_secs() + 120);
        first.successor = Some(PrivateToken::for_test(b"r2", observed(1)));
        h.fake.push(first);
        h.acquire_for(MODEL).await.expect("row");
        tokio::time::advance(Duration::from_secs(120)).await;
        let mut role_failed = h.failed(TransactionFailure::Helper(HelperFailure::ProviderError));
        role_failed.successor = Some(PrivateToken::for_test(b"r3", observed(1)));
        h.fake.push(role_failed);
        assert_eq!(
            h.acquire_for(MODEL).await.err(),
            Some(SourceError::Unavailable)
        );
        assert!(
            h.acquire_for(Duration::from_secs(1)).await.is_ok(),
            "an adequate row stays usable across a failed refresh"
        );
        tokio::time::advance(COOLDOWN_BASE + COOLDOWN_JITTER).await;
        h.fake.push(h.failed(TransactionFailure::HelperUnreported));
        assert_eq!(
            h.acquire_for(MODEL).await.err(),
            Some(SourceError::Unavailable)
        );
        tokio::time::advance(COOLDOWN_BASE + COOLDOWN_JITTER).await;
        h.fake.push(h.ok(HOUR));
        h.acquire_for(MODEL).await.expect("row");
        assert_eq!(h.fake.request(1).0.as_deref(), Some(&b"r2"[..]));
        assert_eq!(h.fake.request(2).0.as_deref(), Some(&b"r3"[..]));
        assert_eq!(
            h.fake.request(3).0.as_deref(),
            Some(&b"r3"[..]),
            "the successor outlived the role failure and the resource failure"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn lost_succession_needs_a_changed_login_before_further_renewal() {
        let h = harness();
        let mut first = h.ok(HOUR);
        first.successor = Some(PrivateToken::for_test(b"r2", observed(1)));
        h.fake.push(first);
        h.acquire_for(MODEL).await.expect("row");
        tokio::time::advance(Duration::from_secs(HOUR)).await;
        let mut lost = h.failed(TransactionFailure::HelperUnreported);
        lost.renewal = RenewalEvidence::Unknown;
        lost.lost_succession = true;
        h.fake.push(lost);
        assert_eq!(
            h.acquire_for(MODEL).await.err(),
            Some(SourceError::Unavailable)
        );
        tokio::time::advance(COOLDOWN_BASE + COOLDOWN_JITTER).await;
        h.fake.push(h.failed(TransactionFailure::LoginRequired));
        assert_eq!(
            h.acquire_for(MODEL).await.err(),
            Some(SourceError::LoginRequired)
        );
        let (predecessor, superseded, _) = h.fake.request(2);
        assert_eq!(
            (predecessor, superseded),
            (None, Some(observed(1))),
            "neither the predecessor nor the unchanged owner token is replayed"
        );
        assert_eq!(h.health.get().state, SourceState::LoginRequired);
    }

    #[tokio::test(start_paused = true)]
    async fn an_observed_withdrawal_discards_the_row_and_no_stale_completion_restores_it() {
        let h = harness();
        let mut first = h.ok(MODEL.as_secs() + 120);
        first.successor = Some(PrivateToken::for_test(b"r2", observed(1)));
        h.fake.push(first);
        h.acquire_for(MODEL).await.expect("row");
        h.owner.withdraw();
        h.fake.push(h.failed(TransactionFailure::Withdrawn));
        assert_eq!(
            h.acquire_for(Duration::from_secs(1)).await.err(),
            Some(SourceError::Withdrawn)
        );
        let (predecessor, superseded, _) = h.fake.request(1);
        assert_eq!(
            (predecessor, superseded),
            (None, Some(observed(1))),
            "withdraw discarded the successor and fenced the owner token it rotated"
        );

        let release = h.fake.push_held(h.ok(HOUR));
        let pending = h.owner.acquire(Instant::now() + MODEL, &h.never);
        let stale = async {
            tokio::task::yield_now().await;
            h.owner.withdraw();
            let _ = release.send(());
        };
        let (row, ()) = tokio::join!(pending, stale);
        assert_eq!(row.err(), Some(SourceError::Withdrawn));
        assert_eq!(
            h.health.get().state,
            SourceState::Unknown,
            "a fenced completion publishes no row"
        );
        h.fake.push(h.ok(HOUR));
        h.acquire_for(MODEL).await.expect("a fresh refresh adopts");
        assert_eq!(h.fake.calls(), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn a_changed_graph_is_invalid_and_drops_cached_material() {
        let h = harness();
        let mut first = h.ok(MODEL.as_secs() + 120);
        first.successor = Some(PrivateToken::for_test(b"r2", observed(1)));
        h.fake.push(first);
        h.acquire_for(MODEL).await.expect("row");
        tokio::time::advance(Duration::from_secs(60)).await;
        let mut edited = h.ok(HOUR);
        edited.identity = Some(identity("222222222222"));
        h.fake.push(edited);
        assert_eq!(h.acquire_for(MODEL).await.err(), Some(SourceError::Invalid));
        assert_eq!(h.health.get().state, SourceState::Invalid);
        assert_eq!(
            h.acquire_for(Duration::from_secs(1))
                .await
                .err()
                .map(|e| matches!(e, SourceError::Cooldown { .. })),
            Some(true)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_fences_late_adoption_and_keeps_unproven_cleanup_as_failure() {
        let h = harness();
        let mut unproven = h.ok(HOUR);
        unproven.failure = Some(TransactionFailure::CleanupUnproven);
        let release = h.fake.push_held(unproven);
        let pending = h.owner.acquire(Instant::now() + MODEL, &h.never);
        let stop = async {
            tokio::task::yield_now().await;
            h.owner.close();
            assert!(
                h.fake.request(0).2.is_cancelled(),
                "close signals the refresh before any join"
            );
            let mut join = std::pin::pin!(h.owner.join());
            let early = tokio::time::timeout(Duration::from_secs(1), join.as_mut()).await;
            assert!(early.is_err(), "join waits for physical settlement");
            let _ = release.send(());
            join.await
        };
        let (row, proven) = tokio::join!(pending, stop);
        assert_eq!(row.err(), Some(SourceError::Shutdown));
        assert!(!proven, "unproven cleanup stays a failure after shutdown");
        assert_eq!(
            h.acquire_for(MODEL).await.err(),
            Some(SourceError::Shutdown)
        );
        assert_eq!(h.fake.calls(), 1, "a closed owner admits no refresh");
    }

    #[tokio::test(start_paused = true)]
    async fn a_late_completion_after_close_cannot_refill_the_cache() {
        let h = harness();
        let release = h.fake.push_held(h.ok(HOUR));
        let pending = h.owner.acquire(Instant::now() + MODEL, &h.never);
        let stop = async {
            tokio::task::yield_now().await;
            h.owner.close();
            let _ = release.send(());
            h.owner.join().await
        };
        let (row, proven) = tokio::join!(pending, stop);
        assert_eq!(row.err(), Some(SourceError::Shutdown));
        assert!(proven);
        assert!(h.owner.0.lock().row.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn a_cancelled_waiter_leaves_an_attached_waiter_and_the_refresh_running() {
        let h = harness();
        let release = h.fake.push_held(h.ok(HOUR));
        let (cancel, attached) = (CancellationToken::new(), CancellationToken::new());
        let deadline = Instant::now() + MODEL;
        let first = tokio::spawn({
            let (owner, cancel) = (h.owner.clone(), cancel.clone());
            async move { owner.acquire(deadline, &cancel).await }
        });
        let second = tokio::spawn({
            let (owner, attached) = (h.owner.clone(), attached.clone());
            async move { owner.acquire(deadline, &attached).await }
        });
        tokio::task::yield_now().await;
        assert_eq!(h.health.get().state, SourceState::Refreshing);
        cancel.cancel();
        assert_eq!(first.await.unwrap().err(), Some(SourceError::Cancelled));
        assert!(
            !h.fake.request(0).2.is_cancelled(),
            "a waiter's cancellation stays with it"
        );
        let _ = release.send(());
        assert!(second.await.unwrap().is_ok());
        assert_eq!(h.fake.calls(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn an_invalidating_refresh_discards_the_row_and_fences_the_successors_owner_token() {
        let cases: [Case<(TransactionOutcome, SourceError)>; 4] = [
            ("withdrawn", |h| {
                (
                    h.failed(TransactionFailure::Withdrawn),
                    SourceError::Withdrawn,
                )
            }),
            ("inadmissible", |h| {
                let mut failed =
                    h.failed(TransactionFailure::Admission(AdmissionError::Unparseable));
                (failed.identity, failed.observation) = (None, None);
                (failed, SourceError::Invalid)
            }),
            ("misconfigured", |h| {
                let failure = TransactionFailure::Helper(HelperFailure::InvalidConfiguration);
                (h.failed(failure), SourceError::Invalid)
            }),
            ("regraphed", |h| {
                let mut edited = h.ok(HOUR);
                edited.identity = Some(identity("222222222222"));
                (edited, SourceError::Invalid)
            }),
        ];
        for (name, case) in cases {
            let h = harness();
            let mut first = h.ok(MODEL.as_secs() + 120);
            first.successor = Some(PrivateToken::for_test(b"r2", observed(1)));
            h.fake.push(first);
            h.acquire_for(MODEL).await.expect("row");
            tokio::time::advance(Duration::from_secs(60)).await;
            let (outcome, error) = case(&h);
            h.fake.push(outcome);
            assert_eq!(h.acquire_for(MODEL).await.err(), Some(error), "{name}");
            tokio::time::advance(COOLDOWN_BASE + COOLDOWN_JITTER).await;
            h.fake.push(h.failed(TransactionFailure::Spawn));
            assert_eq!(
                h.acquire_for(Duration::from_secs(1)).await.err(),
                Some(SourceError::Unavailable),
                "{name} discarded the row"
            );
            let (predecessor, superseded, _) = h.fake.request(2);
            assert_eq!(
                (predecessor, superseded),
                (None, Some(observed(1))),
                "{name} discarded the successor and fenced the owner token it rotated"
            );
        }
        let h = harness();
        let mut first = h.ok(MODEL.as_secs() + 120);
        first.successor = Some(PrivateToken::for_test(b"r2", observed(1)));
        h.fake.push(first);
        h.acquire_for(MODEL).await.expect("row");
        tokio::time::advance(Duration::from_secs(60)).await;
        h.fake
            .push(h.failed(TransactionFailure::Capture(CaptureFailure::Missing)));
        assert_eq!(h.acquire_for(MODEL).await.err(), Some(SourceError::Invalid));
        assert!(
            h.acquire_for(Duration::from_secs(1)).await.is_ok(),
            "a capture failure keeps the row"
        );
        tokio::time::advance(COOLDOWN_BASE + COOLDOWN_JITTER).await;
        h.fake.push(h.ok(HOUR));
        h.acquire_for(MODEL).await.expect("row");
        assert_eq!(h.fake.request(2).0.as_deref(), Some(&b"r2"[..]));
    }

    #[tokio::test(start_paused = true)]
    async fn unproven_cleanup_on_an_open_owner_refuses_every_later_refresh() {
        let h = harness();
        h.fake.push(h.failed(TransactionFailure::CleanupUnproven));
        assert_eq!(h.acquire_for(MODEL).await.err(), Some(SourceError::Invalid));
        tokio::time::advance(COOLDOWN_BASE + COOLDOWN_JITTER).await;
        assert_eq!(h.acquire_for(MODEL).await.err(), Some(SourceError::Invalid));
        assert_eq!(h.fake.calls(), 1, "no replacement refresh starts");
        assert_eq!(h.health.get().state, SourceState::Invalid);
        assert!(!h.owner.join().await);
    }

    #[tokio::test(start_paused = true)]
    async fn a_wall_clock_before_the_epoch_or_an_absurd_expiry_never_grants_unbounded_life() {
        let h = harness();
        h.clock.shift(-(WALL_BASE.as_secs() as i64) - 1);
        h.fake.push(h.ok(HOUR));
        assert_eq!(
            h.acquire_for(MODEL).await.err(),
            Some(SourceError::Unavailable)
        );

        let h = harness();
        let mut absurd = h.ok(HOUR);
        absurd.row.as_mut().unwrap().expires_at_unix_seconds = u64::MAX;
        h.fake.push(absurd);
        h.acquire_for(MODEL).await.expect("row");
        assert_eq!(
            h.health.get().expires_in_seconds,
            Some((MAX_ROW_LIFETIME - EXPIRY_SKEW).as_secs())
        );
        h.clock.shift(-(WALL_BASE.as_secs() as i64) - 1);
        h.fake.push(h.failed(TransactionFailure::Spawn));
        assert_eq!(
            h.acquire_for(Duration::from_secs(1)).await.err(),
            Some(SourceError::Unavailable),
            "an unreadable wall clock makes the cached row unusable"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_superseded_fence_survives_failures_that_observe_nothing() {
        let h = harness();
        let mut lost = h.failed(TransactionFailure::HelperUnreported);
        lost.lost_succession = true;
        h.fake.push(lost);
        assert!(h.acquire_for(MODEL).await.is_err());
        tokio::time::advance(COOLDOWN_BASE + COOLDOWN_JITTER).await;
        let mut blind = h.failed(TransactionFailure::Capture(CaptureFailure::Io));
        blind.observation = None;
        h.fake.push(blind);
        assert!(h.acquire_for(MODEL).await.is_err());
        tokio::time::advance(COOLDOWN_BASE + COOLDOWN_JITTER).await;
        let mut relogin = h.ok(HOUR);
        relogin.observation = Some(observed(2));
        h.fake.push(relogin);
        h.acquire_for(MODEL)
            .await
            .expect("row after a changed login");
        tokio::time::advance(Duration::from_secs(HOUR)).await;
        h.fake.push(h.failed(TransactionFailure::Spawn));
        assert!(h.acquire_for(MODEL).await.is_err());
        let superseded: Vec<_> = (1..4).map(|n| h.fake.request(n).1).collect();
        assert_eq!(superseded, [Some(observed(1)), Some(observed(1)), None]);
    }

    #[tokio::test(start_paused = true)]
    async fn an_unread_owner_token_or_a_graph_mismatch_never_lifts_the_superseded_fence() {
        let cases: [Case<TransactionOutcome>; 3] = [
            ("unusable", |h| {
                let mut refused = h.failed(TransactionFailure::LoginRequired);
                refused.observation = Some(TokenObservation::Unusable);
                refused
            }),
            ("absent", |h| {
                let mut refused = h.failed(TransactionFailure::LoginRequired);
                refused.observation = Some(TokenObservation::Absent);
                refused
            }),
            ("regraphed", |h| {
                let mut refused = h.failed(TransactionFailure::LoginRequired);
                refused.identity = Some(identity("222222222222"));
                refused
            }),
        ];
        for (name, intervening) in cases {
            let h = harness();
            let mut lost = h.failed(TransactionFailure::HelperUnreported);
            lost.lost_succession = true;
            h.fake.push(lost);
            assert!(h.acquire_for(MODEL).await.is_err());
            tokio::time::advance(COOLDOWN_BASE + COOLDOWN_JITTER).await;
            h.fake.push(intervening(&h));
            assert!(h.acquire_for(MODEL).await.is_err(), "{name}");
            tokio::time::advance(COOLDOWN_BASE + COOLDOWN_JITTER).await;
            h.fake.push(h.failed(TransactionFailure::LoginRequired));
            assert!(h.acquire_for(MODEL).await.is_err(), "{name}");
            assert_eq!(
                h.fake.request(2).1,
                Some(observed(1)),
                "{name} kept the fence on the unchanged owner token"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_fenced_rotation_counts_as_lost_succession() {
        let h = harness();
        let mut rotated = h.ok(HOUR);
        rotated.successor = Some(PrivateToken::for_test(b"r2", observed(1)));
        let release = h.fake.push_held(rotated);
        let pending = h.owner.acquire(Instant::now() + MODEL, &h.never);
        let fence = async {
            tokio::task::yield_now().await;
            h.owner.withdraw();
            let _ = release.send(());
        };
        let (row, ()) = tokio::join!(pending, fence);
        assert_eq!(row.err(), Some(SourceError::Withdrawn));
        h.fake.push(h.failed(TransactionFailure::LoginRequired));
        assert!(h.acquire_for(MODEL).await.is_err());
        let (predecessor, superseded, _) = h.fake.request(1);
        assert_eq!((predecessor, superseded), (None, Some(observed(1))));
    }

    #[tokio::test(start_paused = true)]
    async fn a_wall_jump_before_spawn_is_a_transient_source_failure() {
        let h = harness();
        h.fake.push(h.ok(HOUR));
        let deadline = Instant::now() + MODEL;
        let lease = h.owner.acquire(deadline, &h.never).await.expect("row");
        let child = ChildLease {
            lease,
            owner: &h.owner,
        };
        assert!(recheck_before_spawn(Some(&child), super::Harness::Pi, deadline).is_ok());
        h.clock.shift(HOUR as i64);
        let terminal = recheck_before_spawn(Some(&child), super::Harness::Pi, deadline)
            .expect_err("a forward wall jump refuses the spawn");
        let BackendTerminal::SourceFailed(error) = terminal else {
            panic!("{terminal:?}");
        };
        assert_eq!(
            error.class,
            ErrorClass::Transient,
            "the next run refreshes against the new clock"
        );
    }

    #[test]
    fn a_cooldown_retry_hint_covers_the_whole_remaining_cooldown() {
        for (retry_in, secs) in [
            (Duration::from_millis(1_500), 2),
            (Duration::from_millis(400), 1),
            (Duration::from_secs(60), 60),
        ] {
            let terminal = source_terminal(super::Harness::Pi, SourceError::Cooldown { retry_in });
            let BackendTerminal::SourceFailed(error) = terminal else {
                panic!("{retry_in:?}: {terminal:?}");
            };
            assert_eq!(error.retry_after_secs, Some(secs), "{retry_in:?}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_panicking_refresh_leaves_the_owner_unresolved() {
        struct Panics;
        impl Refresh for Panics {
            fn refresh(&self, _: RefreshRequest, _: CancellationToken) -> RefreshFuture {
                Box::pin(async { panic!("refresh panicked") })
            }
        }
        let health = SourceHealthCell::new(SourceObservation::unknown(SourceKind::Profile));
        let owner = SourceOwner::new(Panics, Clock::new(), identity("111111111111"), health);
        let never = CancellationToken::new();
        assert_eq!(
            owner.acquire(Instant::now() + MODEL, &never).await.err(),
            Some(SourceError::Invalid)
        );
        assert!(!owner.join().await, "a panic is never proven cleanup");
    }

    #[test]
    fn production_jitter_stays_within_its_window() {
        for _ in 0..1_000 {
            assert!(SystemClock.jitter() <= COOLDOWN_JITTER);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn an_observed_deletion_or_change_of_the_owner_token_withdraws_the_cached_row() {
        let changed = |h: &Harness| {
            let mut failed = h.failed(TransactionFailure::Helper(HelperFailure::ProviderError));
            failed.observation = Some(observed(2));
            (failed, SourceError::Unavailable)
        };
        let deleted = |h: &Harness| {
            let mut failed = h.failed(TransactionFailure::LoginRequired);
            failed.observation = Some(TokenObservation::Absent);
            (failed, SourceError::LoginRequired)
        };
        let unusable = |h: &Harness| {
            let mut failed = h.failed(TransactionFailure::LoginRequired);
            failed.observation = Some(TokenObservation::Unusable);
            (failed, SourceError::LoginRequired)
        };
        for case in [changed, deleted, unusable] {
            let h = harness();
            let mut first = h.ok(MODEL.as_secs() + 120);
            first.successor = Some(PrivateToken::for_test(b"r2", observed(1)));
            h.fake.push(first);
            h.acquire_for(MODEL).await.expect("row");
            tokio::time::advance(Duration::from_secs(60)).await;
            let (outcome, error) = case(&h);
            h.fake.push(outcome);
            assert_eq!(h.acquire_for(MODEL).await.err(), Some(error));
            tokio::time::advance(COOLDOWN_BASE + COOLDOWN_JITTER).await;
            h.fake.push(h.failed(TransactionFailure::Spawn));
            assert_eq!(
                h.acquire_for(Duration::from_secs(1)).await.err(),
                Some(SourceError::Unavailable),
                "the observed withdrawal discarded a row that still had lifetime"
            );
            assert_eq!(h.fake.request(2).0, None, "and the private successor");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_successor_discarded_with_an_invalid_verdict_sets_the_fence() {
        let h = harness();
        let mut invalid = h.failed(TransactionFailure::Helper(
            HelperFailure::InvalidConfiguration,
        ));
        invalid.successor = Some(PrivateToken::for_test(b"r2", observed(1)));
        h.fake.push(invalid);
        assert_eq!(h.acquire_for(MODEL).await.err(), Some(SourceError::Invalid));
        tokio::time::advance(COOLDOWN_BASE + COOLDOWN_JITTER).await;
        h.fake.push(h.failed(TransactionFailure::LoginRequired));
        assert_eq!(
            h.acquire_for(MODEL).await.err(),
            Some(SourceError::LoginRequired)
        );
        let (predecessor, superseded, _) = h.fake.request(1);
        assert_eq!((predecessor, superseded), (None, Some(observed(1))));
    }

    #[tokio::test(start_paused = true)]
    async fn login_required_observes_the_owner_token_again_on_the_next_demand() {
        let h = harness();
        h.fake.push(h.failed(TransactionFailure::LoginRequired));
        assert_eq!(
            h.acquire_for(MODEL).await.err(),
            Some(SourceError::LoginRequired)
        );
        h.fake.push(h.failed(TransactionFailure::LoginRequired));
        assert_eq!(
            h.acquire_for(MODEL).await.err(),
            Some(SourceError::LoginRequired)
        );
        assert_eq!(
            h.fake.request(1).1,
            Some(observed(1)),
            "the next demand carries the fence, so an unchanged token refuses before the helper"
        );
        let mut relogin = h.ok(HOUR);
        relogin.observation = Some(observed(2));
        h.fake.push(relogin);
        h.acquire_for(MODEL)
            .await
            .expect("a new login is used without a cooldown");
        assert_eq!(h.fake.calls(), 3);
    }
}
