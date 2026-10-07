use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{Map, Value};
use tokio::time::Instant;

pub const AWS_CREDENTIALS_KEY: &str = "aws_credentials";
pub const FIELDS: [&str; 5] = [
    "kind",
    "state",
    "expires_in_seconds",
    "next_retry_in_seconds",
    "consecutive_failures",
];
pub const MAX_EXPIRES_IN_SECONDS: u64 = 86_400;
pub const MAX_NEXT_RETRY_IN_SECONDS: u64 = 300;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    None,
    Environment,
    Profile,
}

impl SourceKind {
    pub const ALL: [Self; 3] = [Self::None, Self::Environment, Self::Profile];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Environment => "environment",
            Self::Profile => "profile",
        }
    }
}

/// `Ready` means a locally usable cached row, never remote permission or fold progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceState {
    Unknown,
    Ready,
    Refreshing,
    Cooldown,
    LoginRequired,
    Invalid,
}

impl SourceState {
    pub const ALL: [Self; 6] = [
        Self::Unknown,
        Self::Ready,
        Self::Refreshing,
        Self::Cooldown,
        Self::LoginRequired,
        Self::Invalid,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Ready => "ready",
            Self::Refreshing => "refreshing",
            Self::Cooldown => "cooldown",
            Self::LoginRequired => "login_required",
            Self::Invalid => "invalid",
        }
    }
}

/// `SourceHealth` provides a cached credential-source observation for operator health
/// reporting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceHealth {
    pub kind: SourceKind,
    pub state: SourceState,
    pub expires_in_seconds: Option<u64>,
    pub next_retry_in_seconds: Option<u64>,
    pub consecutive_failures: u32,
}

impl SourceHealth {
    /// The closed block. Numbers above their bound are reported at the bound, and
    /// unknown numbers are omitted.
    pub fn to_json(&self) -> Value {
        let mut block = Map::new();
        block.insert("kind".to_owned(), Value::from(self.kind.as_str()));
        block.insert("state".to_owned(), Value::from(self.state.as_str()));
        if let Some(seconds) = self.expires_in_seconds {
            block.insert(
                "expires_in_seconds".to_owned(),
                Value::from(seconds.min(MAX_EXPIRES_IN_SECONDS)),
            );
        }
        if let Some(seconds) = self.next_retry_in_seconds {
            block.insert(
                "next_retry_in_seconds".to_owned(),
                Value::from(seconds.min(MAX_NEXT_RETRY_IN_SECONDS)),
            );
        }
        block.insert(
            "consecutive_failures".to_owned(),
            Value::from(self.consecutive_failures),
        );
        Value::Object(block)
    }
}

/// What the source owner records: absolute times, so each read reports the seconds
/// remaining at that read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceObservation {
    pub kind: SourceKind,
    pub state: SourceState,
    pub expires_at: Option<Instant>,
    pub retry_at: Option<Instant>,
    pub consecutive_failures: u32,
}

impl SourceObservation {
    pub fn unknown(kind: SourceKind) -> Self {
        Self {
            kind,
            state: SourceState::Unknown,
            expires_at: None,
            retry_at: None,
            consecutive_failures: 0,
        }
    }
}

/// The shared cached observation. A ready row past its expiry reads as unknown.
#[derive(Clone, Debug)]
pub struct SourceHealthCell(Arc<Mutex<SourceObservation>>);

impl SourceHealthCell {
    pub fn new(initial: SourceObservation) -> Self {
        Self(Arc::new(Mutex::new(initial)))
    }

    pub fn get(&self) -> SourceHealth {
        let observed = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let now = Instant::now();
        let live = observed.expires_at.filter(|at| *at > now);
        let cooling = observed.retry_at.filter(|at| *at > now);
        let state = match observed.state {
            SourceState::Ready if observed.expires_at.is_some() && live.is_none() => {
                SourceState::Unknown
            }
            SourceState::Cooldown if cooling.is_none() && live.is_some() => SourceState::Ready,
            SourceState::Cooldown if cooling.is_none() => SourceState::Unknown,
            state => state,
        };
        SourceHealth {
            kind: observed.kind,
            state,
            expires_in_seconds: live.map(|at| (at - now).as_secs()),
            next_retry_in_seconds: cooling.map(|at| (at - now).as_secs()),
            consecutive_failures: observed.consecutive_failures,
        }
    }

    pub fn set(&self, observation: SourceObservation) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = observation;
    }
}

/// Parses one block. Any key outside [`FIELDS`], an out-of-set value, or a malformed
/// field drops the whole block.
pub fn parse(raw: &Value) -> Option<SourceHealth> {
    let raw = raw.as_object()?;
    if raw.keys().any(|key| !FIELDS.contains(&key.as_str())) {
        return None;
    }
    let text = |key: &str| raw.get(key).and_then(Value::as_str);
    let kind = SourceKind::ALL
        .into_iter()
        .find(|kind| text("kind") == Some(kind.as_str()))?;
    let state = SourceState::ALL
        .into_iter()
        .find(|state| text("state") == Some(state.as_str()))?;
    let bounded = |key: &str, max: u64| match raw.get(key) {
        None => Some(None),
        Some(value) => value.as_u64().filter(|n| *n <= max).map(Some),
    };
    Some(SourceHealth {
        kind,
        state,
        expires_in_seconds: bounded("expires_in_seconds", MAX_EXPIRES_IN_SECONDS)?,
        next_retry_in_seconds: bounded("next_retry_in_seconds", MAX_NEXT_RETRY_IN_SECONDS)?,
        consecutive_failures: raw
            .get("consecutive_failures")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())?,
    })
}

/// The block re-emitted from its parse, or `None` when it is outside the closed shape.
pub fn sanitize(raw: &Value) -> Option<Value> {
    parse(raw).map(|health| health.to_json())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::*;

    #[test]
    fn every_state_and_kind_round_trips_and_unknown_numbers_are_omitted() {
        for kind in SourceKind::ALL {
            for state in SourceState::ALL {
                let health = SourceHealth {
                    kind,
                    state,
                    expires_in_seconds: None,
                    next_retry_in_seconds: None,
                    consecutive_failures: 0,
                };
                let block = health.to_json();
                assert_eq!(sanitize(&block), Some(block.clone()));
                assert!(block.get("expires_in_seconds").is_none());
                assert!(block.get("next_retry_in_seconds").is_none());
            }
        }
        let bounded = SourceHealth {
            kind: SourceKind::Profile,
            state: SourceState::Cooldown,
            expires_in_seconds: Some(u64::MAX),
            next_retry_in_seconds: Some(u64::MAX),
            consecutive_failures: u32::MAX,
        }
        .to_json();
        assert_eq!(bounded["expires_in_seconds"], MAX_EXPIRES_IN_SECONDS);
        assert_eq!(bounded["next_retry_in_seconds"], MAX_NEXT_RETRY_IN_SECONDS);
        assert_eq!(bounded["consecutive_failures"], u32::MAX);
        assert_eq!(sanitize(&bounded), Some(bounded.clone()));
    }

    #[tokio::test(start_paused = true)]
    async fn timed_fields_count_down_at_read_and_an_expired_ready_row_reads_unknown() {
        let cell = SourceHealthCell::new(SourceObservation::unknown(SourceKind::Profile));
        cell.set(SourceObservation {
            state: SourceState::Ready,
            expires_at: Some(Instant::now() + Duration::from_secs(100)),
            ..SourceObservation::unknown(SourceKind::Profile)
        });
        tokio::time::advance(Duration::from_secs(40)).await;
        assert_eq!(cell.get().expires_in_seconds, Some(60));
        tokio::time::advance(Duration::from_secs(60)).await;
        let expired = cell.get();
        assert_eq!(
            (expired.state, expired.expires_in_seconds),
            (SourceState::Unknown, None)
        );
        cell.set(SourceObservation {
            state: SourceState::LoginRequired,
            expires_at: Some(Instant::now()),
            ..SourceObservation::unknown(SourceKind::Profile)
        });
        let expired = cell.get();
        assert_eq!(
            (expired.state, expired.expires_in_seconds),
            (SourceState::LoginRequired, None),
            "only a ready row changes state at expiry"
        );
        cell.set(SourceObservation {
            state: SourceState::Cooldown,
            retry_at: Some(Instant::now() + Duration::from_secs(75)),
            ..SourceObservation::unknown(SourceKind::Profile)
        });
        tokio::time::advance(Duration::from_secs(5)).await;
        assert_eq!(cell.get().next_retry_in_seconds, Some(70));
        tokio::time::advance(Duration::from_secs(70)).await;
        let elapsed = cell.get();
        assert_eq!(
            (elapsed.state, elapsed.next_retry_in_seconds),
            (SourceState::Unknown, None),
            "an elapsed cooldown without a usable row reads unknown"
        );
        cell.set(SourceObservation {
            state: SourceState::Cooldown,
            retry_at: Some(Instant::now()),
            expires_at: Some(Instant::now() + Duration::from_secs(30)),
            consecutive_failures: 2,
            ..SourceObservation::unknown(SourceKind::Profile)
        });
        let elapsed = cell.get();
        assert_eq!(
            (
                elapsed.state,
                elapsed.next_retry_in_seconds,
                elapsed.expires_in_seconds,
                elapsed.consecutive_failures
            ),
            (SourceState::Ready, None, Some(30), 2),
            "an elapsed cooldown over a usable row reads ready"
        );
    }

    #[test]
    fn a_block_outside_the_closed_shape_is_dropped_whole() {
        let valid = json!({"kind": "profile", "state": "ready", "consecutive_failures": 0});
        assert!(sanitize(&valid).is_some());
        let with = |key: &str, value: Value| {
            let mut block = valid.clone();
            block[key] = value;
            sanitize(&block)
        };
        for (key, value) in [
            ("profile", json!("corp")),
            ("account", json!("123456789012")),
            ("role_arn", json!("arn:aws:iam::123456789012:role/r")),
            ("config_file", json!("/home/u/.aws/config")),
            ("session_token", json!("IQoJb3JpZ2lu")),
            ("error", json!("ExpiredTokenException")),
            ("kind", json!("static")),
            ("state", json!("expired")),
            ("state", json!(null)),
            ("expires_in_seconds", json!(MAX_EXPIRES_IN_SECONDS + 1)),
            (
                "next_retry_in_seconds",
                json!(MAX_NEXT_RETRY_IN_SECONDS + 1),
            ),
            ("expires_in_seconds", json!(-1)),
            ("expires_in_seconds", json!(1.5)),
            ("consecutive_failures", json!(u64::from(u32::MAX) + 1)),
        ] {
            assert_eq!(with(key, value.clone()), None, "{key}={value}");
        }
        let mut missing = valid.clone();
        missing
            .as_object_mut()
            .unwrap()
            .remove("consecutive_failures");
        assert_eq!(sanitize(&missing), None);
        assert_eq!(sanitize(&json!("ready")), None);
    }
}
