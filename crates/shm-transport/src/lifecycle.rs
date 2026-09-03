use std::fmt;

/// Ordered close states. `advance` admits exactly the transitions listed there; `Joined` and
/// `Quarantined` are terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseState {
    /// The lifecycle admits and publishes traffic.
    Open,
    /// New admission has stopped.
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
    /// Every resource is released; the storage may be reused.
    Joined,
    /// Storage can never be reused after `Quarantined`.
    Quarantined,
}

/// Close-state machine for one transport attachment. `mark_prepared` records that the peer
/// has been told the ring exists, after which any failure must be reported rather than
/// retried silently.
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

    /// Whether a failure must surface to the peer because the ring was already announced.
    pub const fn must_fail_closed(&self) -> bool {
        self.prepared
    }

    /// Current state.
    pub const fn state(&self) -> CloseState {
        self.state
    }

    /// Moves to `next` if that is the successor of the current state. A terminal state
    /// returns `LifecycleError::Terminal`; any other skipped or reversed step returns
    /// `LifecycleError::InvalidTransition`.
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

    /// Whether the close completed cleanly (`Joined`), so the storage may host a new ring.
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
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LifecycleError {
    /// The requested state is not the successor of the current one.
    InvalidTransition,
    /// The current state is `Joined` or `Quarantined`.
    Terminal,
}

impl fmt::Debug for LifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl fmt::Display for LifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidTransition => "invalid lifecycle transition",
            Self::Terminal => "lifecycle state is terminal",
        })
    }
}

impl std::error::Error for LifecycleError {}
