//! Early credential-helper mode: one bounded AWS acquisition from an admitted graph.
//!
//! The helper re-admits its canonical graph, runs the pinned profile provider once
//! with explicit regions, one attempt per call, and fixed timeouts, and reports one
//! complete row or a closed failure kind, never token state or SDK error text. A
//! guarded HTTP client refuses destinations outside the graph's regional STS, SSO
//! portal, and OIDC hosts and records whether an OIDC renewal request started. The
//! private SSO token cache under the helper's `HOME` is the only successor channel.
//! The SDK reads `HOME`, endpoint overrides, and CA roots from the environment, so the
//! parent clears it; foreign endpoint overrides fail closed at the guard.

use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_config::BehaviorVersion;
use aws_config::profile::ProfileFileCredentialsProvider;
use aws_config::provider_config::ProviderConfig;
use aws_config::retry::RetryConfig;
use aws_config::timeout::TimeoutConfig;
use aws_credential_types::Credentials;
use aws_credential_types::provider::ProvideCredentials;
use aws_credential_types::provider::error::CredentialsError;
use aws_runtime::env_config::file::{EnvConfigFileKind, EnvConfigFiles};
use aws_smithy_async::time::SharedTimeSource;
use aws_smithy_http_client::tls::{Provider, rustls_provider::CryptoMode};
use aws_smithy_runtime_api::client::http::{
    HttpClient, HttpConnector, HttpConnectorFuture, HttpConnectorSettings, SharedHttpClient,
    SharedHttpConnector,
};
use aws_smithy_runtime_api::client::orchestrator::HttpRequest;
use aws_smithy_runtime_api::client::result::ConnectorError;
use aws_smithy_runtime_api::client::runtime_components::RuntimeComponents;
use aws_types::region::Region;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::aws_profile::{self, AdmittedGraph, CapturedProfileInput, RootIdentity};

/// The only argument that selects helper mode in the host executable.
pub const HELPER_ARG: &str = "aws-credential-helper";
/// Bounds on the stdin request, the stdout report, and each credential field.
pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;
pub const MAX_REPORT_BYTES: usize = 256 * 1024;
pub const MAX_FIELD_BYTES: usize = 16 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(10);
/// Debug builds redirect admitted destinations to this loopback port for tests.
const TEST_ORIGIN_ENV: &str = "EIDNARA_HOST_TEST_AWS_ORIGIN";

/// Canonical admitted graph sent by the parent on stdin.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelperRequest {
    pub profile: String,
    pub region: String,
    /// Canonical emission of the admitted graph; anything else is refused.
    pub config: Zeroizing<String>,
}

impl HelperRequest {
    pub fn from_graph(graph: &AdmittedGraph) -> Self {
        let identity = graph.identity();
        Self {
            profile: identity.profile.clone(),
            region: identity.region.clone(),
            config: graph.emit_config(),
        }
    }
}

/// Whether an SSO OIDC renewal request started. `NotStarted` in a completed report
/// proves the private token was not rotated by this helper.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Renewal {
    NotStarted,
    Started,
}

/// Closed failure vocabulary. SDK failures map by error variant only; HTTP timeouts
/// surface from the pinned SDK as provider errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HelperFailure {
    /// The request was malformed, oversized, or not a canonical admitted graph.
    InvalidInput,
    /// The SDK attempted a destination outside the admitted regional hosts.
    DestinationRefused,
    /// The provider found no usable source, such as a missing or expired login.
    NotLoaded,
    InvalidConfiguration,
    ProviderError,
    /// The returned row lacks a field or a future expiration, or a field exceeds
    /// [`MAX_FIELD_BYTES`].
    Incomplete,
    Unhandled,
}

/// The helper's whole stdout.
#[derive(Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum HelperReport {
    Credentials {
        access_key_id: String,
        secret_access_key: Zeroizing<String>,
        session_token: Zeroizing<String>,
        expires_at_unix_seconds: u64,
        renewal: Renewal,
    },
    Failed {
        failure: HelperFailure,
        renewal: Renewal,
    },
}

impl HelperReport {
    fn failed(failure: HelperFailure, renewal: Renewal) -> Self {
        Self::Failed { failure, renewal }
    }
}

/// Runs helper mode over the process's stdin and stdout and returns its exit code.
/// Exit 0 means a report was written.
pub fn run(stdin: impl Read, mut stdout: impl Write) -> i32 {
    let report = match read_request(stdin) {
        None => HelperReport::failed(HelperFailure::InvalidInput, Renewal::NotStarted),
        Some(request) => acquire_on_own_runtime(&request),
    };
    match serde_json::to_vec(&report).map(Zeroizing::new) {
        Ok(bytes) if bytes.len() <= MAX_REPORT_BYTES => {
            i32::from(stdout.write_all(&bytes).and(stdout.flush()).is_err())
        }
        _ => 1,
    }
}

/// Runs [`acquire`] on a current-thread runtime with at most two blocking threads.
/// Shutdown abandons any blocking resolver thread instead of waiting for it, so the
/// report is written as soon as the SDK returns.
fn acquire_on_own_runtime(request: &HelperRequest) -> HelperReport {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(2)
        .build();
    let Ok(runtime) = runtime else {
        return HelperReport::failed(HelperFailure::Unhandled, Renewal::NotStarted);
    };
    let origin = std::env::var(TEST_ORIGIN_ENV)
        .ok()
        .filter(|_| cfg!(debug_assertions))
        .and_then(|origin| {
            origin
                .strip_prefix("http://127.0.0.1:")?
                .parse::<u16>()
                .ok()
        })
        .map(|port| format!("http://127.0.0.1:{port}"));
    let http = default_client();
    let report = runtime.block_on(acquire(request, http, SharedTimeSource::default(), origin));
    runtime.shutdown_background();
    report
}

fn read_request(stdin: impl Read) -> Option<HelperRequest> {
    let mut bytes = Zeroizing::new(Vec::new());
    let limit = u64::try_from(MAX_REQUEST_BYTES).ok()? + 1;
    stdin.take(limit).read_to_end(&mut bytes).ok()?;
    if bytes.len() > MAX_REQUEST_BYTES {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

/// The pinned SDK's default HTTPS client: rustls over aws-lc with platform roots and
/// normal certificate verification.
fn default_client() -> SharedHttpClient {
    aws_smithy_http_client::Builder::new()
        .tls_provider(Provider::Rustls(CryptoMode::AwsLc))
        .build_https()
}

/// Acquires one credential row for `request` through `http`, judging token and row
/// expiry by `time`. The executable passes the default HTTPS client, system time,
/// and its debug-only test origin; tests pass a synthetic transport and clock. Poll
/// inside a Tokio runtime with I/O and time enabled.
pub async fn acquire(
    request: &HelperRequest,
    http: SharedHttpClient,
    time: SharedTimeSource,
    origin: Option<String>,
) -> HelperReport {
    let invalid = HelperReport::failed(HelperFailure::InvalidInput, Renewal::NotStarted);
    let input = CapturedProfileInput {
        profile: &request.profile,
        region: &request.region,
        config: request.config.as_bytes(),
        credentials: b"",
    };
    let Ok(graph) = aws_profile::admit(input) else {
        return invalid;
    };
    if graph.emit_config() != request.config {
        return invalid;
    }
    let mut destinations = vec![format!("sts.{}.amazonaws.com", request.region)];
    let mut oidc = None;
    if let RootIdentity::Sso { sso_region, .. } = &graph.identity().root {
        destinations.push(format!("portal.sso.{sso_region}.amazonaws.com"));
        oidc = Some(format!("oidc.{sso_region}.amazonaws.com"));
    }
    let guard = Arc::new(Guard {
        destinations,
        oidc,
        origin,
        renewal_started: AtomicBool::new(false),
        refused: AtomicBool::new(false),
    });
    let timeouts = TimeoutConfig::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(ATTEMPT_TIMEOUT)
        .operation_attempt_timeout(ATTEMPT_TIMEOUT)
        .build();
    let config = ProviderConfig::without_region()
        .with_region(Some(Region::new(request.region.clone())))
        .with_behavior_version(Some(BehaviorVersion::latest()))
        .with_time_source(time.clone())
        .with_http_client(GuardedClient {
            inner: http,
            guard: Arc::clone(&guard),
        })
        .with_retry_config(RetryConfig::standard().with_max_attempts(1))
        .with_timeout_config(timeouts);
    let files = EnvConfigFiles::builder()
        .include_default_config_file(false)
        .include_default_credentials_file(false)
        .with_contents(EnvConfigFileKind::Config, request.config.as_str())
        .build();
    let provider = ProfileFileCredentialsProvider::builder()
        .configure(&config)
        .profile_files(files)
        .profile_name(&request.profile)
        .build();
    let result = provider.provide_credentials().await;
    let started = guard.renewal_started.load(Ordering::SeqCst);
    let renewal = [Renewal::NotStarted, Renewal::Started][usize::from(started)];
    if guard.refused.load(Ordering::SeqCst) {
        return HelperReport::failed(HelperFailure::DestinationRefused, renewal);
    }
    match result {
        Ok(row) => complete_row(&row, time.now(), renewal),
        Err(error) => HelperReport::failed(classify(&error), renewal),
    }
}

/// A complete row has three nonempty bounded fields and an expiry strictly after
/// `now`. An absent or past expiry, such as the epoch the SDK substitutes for a
/// response without one, makes the row incomplete.
pub fn complete_row(row: &Credentials, now: SystemTime, renewal: Renewal) -> HelperReport {
    let token = row.session_token().unwrap_or_default();
    let fields = [row.access_key_id(), row.secret_access_key(), token];
    let bounded = |field: &&str| !field.is_empty() && field.len() <= MAX_FIELD_BYTES;
    let expires = row
        .expiry()
        .filter(|expiry| *expiry > now)
        .and_then(|expiry| expiry.duration_since(UNIX_EPOCH).ok());
    match expires {
        Some(expires) if fields.iter().all(bounded) => HelperReport::Credentials {
            access_key_id: fields[0].to_owned(),
            secret_access_key: Zeroizing::new(fields[1].to_owned()),
            session_token: Zeroizing::new(token.to_owned()),
            expires_at_unix_seconds: expires.as_secs(),
            renewal,
        },
        _ => HelperReport::failed(HelperFailure::Incomplete, renewal),
    }
}

fn classify(error: &CredentialsError) -> HelperFailure {
    match error {
        CredentialsError::CredentialsNotLoaded(_) => HelperFailure::NotLoaded,
        CredentialsError::InvalidConfiguration(_) => HelperFailure::InvalidConfiguration,
        CredentialsError::ProviderError(_) | CredentialsError::ProviderTimedOut(_) => {
            HelperFailure::ProviderError
        }
        _ => HelperFailure::Unhandled,
    }
}

#[derive(Debug)]
struct Guard {
    destinations: Vec<String>,
    oidc: Option<String>,
    origin: Option<String>,
    renewal_started: AtomicBool,
    refused: AtomicBool,
}

/// HTTP client that admits only the graph's HTTPS destinations and records the start
/// of any request to the OIDC host. It never inspects request contents.
#[derive(Debug)]
struct GuardedClient {
    inner: SharedHttpClient,
    guard: Arc<Guard>,
}

impl HttpClient for GuardedClient {
    fn http_connector(
        &self,
        settings: &HttpConnectorSettings,
        components: &RuntimeComponents,
    ) -> SharedHttpConnector {
        SharedHttpConnector::new(GuardedConnector {
            guard: Arc::clone(&self.guard),
            inner: self.inner.clone(),
            settings: settings.clone(),
            components: components.clone(),
        })
    }
}

/// GuardedConnector creates the inner connector after the guard admits the destination.
#[derive(Debug)]
struct GuardedConnector {
    guard: Arc<Guard>,
    inner: SharedHttpClient,
    settings: HttpConnectorSettings,
    components: RuntimeComponents,
}

impl HttpConnector for GuardedConnector {
    fn call(&self, mut request: HttpRequest) -> HttpConnectorFuture {
        let guard = &self.guard;
        let authority = request
            .uri()
            .strip_prefix("https://")
            .and_then(|rest| rest.split('/').next())
            .filter(|authority| {
                guard.destinations.iter().any(|d| d == authority)
                    || guard.oidc.as_deref() == Some(authority)
            })
            .map(str::to_owned);
        let Some(authority) = authority else {
            guard.refused.store(true, Ordering::SeqCst);
            let refused = ConnectorError::user("destination outside the admitted graph".into());
            return HttpConnectorFuture::ready(Err(refused));
        };
        if guard.oidc.as_deref() == Some(authority.as_str()) {
            guard.renewal_started.store(true, Ordering::SeqCst);
        }
        if let Some(origin) = &guard.origin {
            let rewritten = request
                .uri()
                .replacen(&format!("https://{authority}"), origin, 1);
            if request.set_uri(rewritten).is_err() {
                guard.refused.store(true, Ordering::SeqCst);
                let refused = ConnectorError::user("test origin is not a URI".into());
                return HttpConnectorFuture::ready(Err(refused));
            }
        }
        self.inner
            .http_connector(&self.settings, &self.components)
            .call(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_runtime_api::client::runtime_components::RuntimeComponentsBuilder;
    use aws_smithy_types::body::SdkBody;

    #[derive(Debug)]
    struct Unreachable;

    impl HttpClient for Unreachable {
        fn http_connector(
            &self,
            _: &HttpConnectorSettings,
            _: &RuntimeComponents,
        ) -> SharedHttpConnector {
            panic!("a refused destination built the transport")
        }
    }

    #[derive(Clone, Debug, Default)]
    struct Recording(Arc<std::sync::Mutex<Vec<String>>>);

    impl HttpClient for Recording {
        fn http_connector(
            &self,
            _: &HttpConnectorSettings,
            _: &RuntimeComponents,
        ) -> SharedHttpConnector {
            SharedHttpConnector::new(self.clone())
        }
    }

    impl HttpConnector for Recording {
        fn call(&self, request: HttpRequest) -> HttpConnectorFuture {
            self.0.lock().unwrap().push(request.uri().to_owned());
            HttpConnectorFuture::ready(Err(ConnectorError::user("recorded".into())))
        }
    }

    fn call(connector: &SharedHttpConnector, uri: &str) {
        let mut request = HttpRequest::new(SdkBody::empty());
        request.set_uri(uri).unwrap();
        let result = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(connector.call(request));
        assert!(result.is_err());
    }

    #[test]
    fn destinations_outside_the_graph_are_refused_before_the_transport() {
        let guard = || {
            Arc::new(Guard {
                destinations: vec!["sts.us-west-2.amazonaws.com".to_owned()],
                oidc: Some("oidc.us-east-1.amazonaws.com".to_owned()),
                origin: None,
                renewal_started: AtomicBool::new(false),
                refused: AtomicBool::new(false),
            })
        };
        let components = RuntimeComponentsBuilder::for_tests().build().unwrap();
        let guarded = |inner: SharedHttpClient, guard: &Arc<Guard>| {
            let client = GuardedClient {
                inner,
                guard: Arc::clone(guard),
            };
            client.http_connector(&HttpConnectorSettings::default(), &components)
        };
        let refusing = guard();
        let connector = guarded(SharedHttpClient::new(Unreachable), &refusing);
        for uri in [
            "https://sts.amazonaws.com/",
            "https://sts.us-east-1.amazonaws.com/",
            "http://sts.us-west-2.amazonaws.com/",
            "https://sts.us-west-2.amazonaws.com.attacker.example/",
            "https://sts.us-west-2.amazonaws.com:8443/",
            "https://user@sts.us-west-2.amazonaws.com/",
        ] {
            refusing.refused.store(false, Ordering::SeqCst);
            call(&connector, uri);
            assert!(refusing.refused.load(Ordering::SeqCst), "{uri}");
        }
        assert!(!refusing.renewal_started.load(Ordering::SeqCst));

        let recording = Recording::default();
        let admitting = guard();
        let connector = guarded(SharedHttpClient::new(recording.clone()), &admitting);
        call(&connector, "https://sts.us-west-2.amazonaws.com/");
        assert!(!admitting.renewal_started.load(Ordering::SeqCst));
        call(&connector, "https://oidc.us-east-1.amazonaws.com/token");
        assert!(admitting.renewal_started.load(Ordering::SeqCst));
        assert!(!admitting.refused.load(Ordering::SeqCst));
        assert_eq!(recording.0.lock().unwrap().len(), 2);
    }

    #[test]
    fn rows_need_bounded_fields_and_a_future_expiry() {
        let now = SystemTime::now();
        let later = Some(now + Duration::from_secs(1));
        let max = "x".repeat(MAX_FIELD_BYTES);
        let over = "x".repeat(MAX_FIELD_BYTES + 1);
        let row = |access: &str, secret: &str, token: &str, expiry| {
            Credentials::new(access, secret, Some(token.to_owned()), expiry, "test")
        };
        let complete = |access: &str, secret: &str, token: &str, expiry| {
            matches!(
                complete_row(
                    &row(access, secret, token, expiry),
                    now,
                    Renewal::NotStarted
                ),
                HelperReport::Credentials { .. }
            )
        };
        assert!(complete("A", "s", &max, later));
        assert!(!complete("", "s", "t", later));
        assert!(!complete("A", "", "t", later));
        assert!(!complete("A", "s", "", later));
        assert!(!complete("A", "s", &over, later));
        assert!(!complete("A", "s", "t", Some(now)));
        assert!(!complete("A", "s", "t", None));
        let report = complete_row(&row("A", &max, &max, later), now, Renewal::Started);
        assert!(serde_json::to_vec(&report).unwrap().len() <= MAX_REPORT_BYTES);
    }
}
