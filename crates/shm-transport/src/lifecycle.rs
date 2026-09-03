use std::fmt;

/// Close states in the order a clean shutdown visits them. `Lifecycle::advance` admits only
/// the listed edges; `Joined` and `Quarantined` are terminal. Several states name the native
/// addon's environment thread and JavaScript aliases because the same machine runs on both
/// sides of the ring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseState {
    /// Frames are admitted and published.
    Open,
    /// No new frames are admitted; in-flight frames continue.
    Quiescing,
    /// Already-published frames are draining.
    DrainingPublished,
    /// New environment work has stopped.
    StoppingEnvScheduling,
    /// The environment thread detaches JavaScript aliases.
    RevokingJsOnEnv,
    /// N-API asynchronous cleanup is joining native workers.
    AsyncCleanupJoin,
    /// Lexical Rust receive scopes are draining.
    AwaitingRustScopes,
    /// The lifecycle releases backend samples.
    ReleasingSamples,
    /// The lifecycle drops transport mappings and objects.
    DroppingTransport,
    /// All resources are released; the storage may host a new ring.
    Joined,
    /// Cleanup outcome is uncertain, so the storage is withheld forever rather than risk
    /// handing a live mapping to a new attachment.
    Quarantined,
}

/// Close-state machine for one ring attachment. `mark_prepared` records that the peer has
/// been told the ring exists; from then on a failure must be reported to the peer, because
/// the peer would otherwise wait on a ring that will never open.
pub struct Lifecycle {
    state: CloseState,
    prepared: bool,
}

impl Lifecycle {
    /// Starts in `CloseState::Open`, not yet prepared.
    pub const fn new() -> Self {
        Self {
            state: CloseState::Open,
            prepared: false,
        }
    }

    /// Records that the ring was announced to the peer. Allowed once, only while `Open`.
    pub fn mark_prepared(&mut self) -> Result<(), LifecycleError> {
        if self.state != CloseState::Open || self.prepared {
            return Err(LifecycleError::InvalidTransition);
        }
        self.prepared = true;
        Ok(())
    }

    /// Whether the peer has been told about the ring, so a failure must surface to it.
    pub const fn must_fail_closed(&self) -> bool {
        self.prepared
    }

    /// State the machine is in.
    pub const fn state(&self) -> CloseState {
        self.state
    }

    /// Moves to `next` if it is a listed successor of the current state. `Quarantined` is
    /// reachable only from `RevokingJsOnEnv`, the one step whose failure leaves ownership of
    /// the mapping uncertain. A terminal state returns `LifecycleError::Terminal`; any other
    /// skipped or reversed step returns `LifecycleError::InvalidTransition`.
    pub fn advance(&mut self, next: CloseState) -> Result<(), LifecycleError> {
        let valid = matches!(
            (self.state, next),
            (CloseState::Open, CloseState::Quiescing)
                | (CloseState::Quiescing, CloseState::DrainingPublished)
                | (
                    CloseState::DrainingPublished,
                    CloseState::StoppingEnvScheduling
                )
                | (
                    CloseState::StoppingEnvScheduling,
                    CloseState::RevokingJsOnEnv
                )
                | (CloseState::RevokingJsOnEnv, CloseState::AsyncCleanupJoin)
                | (CloseState::RevokingJsOnEnv, CloseState::Quarantined)
                | (CloseState::AsyncCleanupJoin, CloseState::AwaitingRustScopes)
                | (CloseState::AwaitingRustScopes, CloseState::ReleasingSamples)
                | (CloseState::ReleasingSamples, CloseState::DroppingTransport)
                | (CloseState::DroppingTransport, CloseState::Joined)
        );
        if !valid {
            return Err(
                if matches!(self.state, CloseState::Joined | CloseState::Quarantined) {
                    LifecycleError::Terminal
                } else {
                    LifecycleError::InvalidTransition
                },
            );
        }
        self.state = next;
        Ok(())
    }

    /// Whether the close reached `Joined`, so the storage may host a new ring.
    pub fn reusable(&self) -> bool {
        self.state == CloseState::Joined
    }
}

impl Default for Lifecycle {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Lifecycle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Lifecycle")
            .field("state", &self.state)
            .field("prepared", &self.prepared)
            .finish()
    }
}

/// Why a lifecycle transition was refused.
#[derive(Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LifecycleError {
    /// The requested state is not the successor of the current one.
    #[error("invalid lifecycle transition")]
    InvalidTransition,
    /// The current state is `Joined` or `Quarantined`.
    #[error("lifecycle state is terminal")]
    Terminal,
}

impl fmt::Debug for LifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}
