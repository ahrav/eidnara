//! One verified, one-use sender for Anthropic Messages requests.
//!
//! The sender opens a fresh TCP connection to the fixed production host, completes a rustls handshake that verifies the full chain and hostname against the webpki roots with SNI for that host and TLS 1.2 or newer, and completes an HTTP/1 handshake, all without writing a byte of any request. The caller then hands over exactly one owned request; the handoff is synchronous and moves the request into the connection's dispatch queue, and no request byte reaches the peer until the caller asks for completion. Completing polls the connection once, reads the response under the raw-byte allowance, refuses compressed or non-JSON bodies, decodes the message under the closed bounds, runs the common render check, and yields one [`AssistantText`]. There is no retry, redirect, reconnect, proxy, pool, or fallback anywhere in this module; a second request needs a second connection. The TCP connect itself follows the standard resolver contract and may try each address the host resolves to until one accepts, all under the connect deadline; no request exists on the wire until after the TLS and HTTP handshakes, so a refused address is not a resend. The credential is written into the `x-api-key` header and nowhere else.
//!
//! Hyper and rustls connection types never leave this module; callers see [`Sender`], [`Connected`], [`InFlight`], and [`AssistantText`]. An endpoint other than the production one exists only under the `test-support` feature, for the local TLS peer the sender proof runs against.

use std::sync::Arc;

use std::time::Duration;

use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::client::conn::http1::{Connection, SendRequest};
use hyper::header::{self, HeaderName, HeaderValue};
use hyper::{Method, Request, Response, StatusCode, Uri};
use hyper_util::rt::TokioIo;
use rustls::ClientConfig;
use rustls::pki_types::ServerName;
use serde::Serialize;
use sha2::Digest;
use tokio::net::TcpStream;
use tokio::time::Instant;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use zeroize::Zeroizing;

use super::broker::check_render;
pub use memory_store::memory_reviewer_ledger::ResponseAllowance;

use super::model_response::{
    DecodeError, MAX_ASSISTANT_TEXT_BYTES, StopReason, decode_message_within,
};

pub const ANTHROPIC_HOST: &str = "api.anthropic.com";
pub const ANTHROPIC_PORT: u16 = 443;
pub const MESSAGES_PATH: &str = "/v1/messages";
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
/// The one startup-envelope credential this protocol may write into `x-api-key`; another provider's secret never reaches this host.
pub const CREDENTIAL_NAME: &str = "ANTHROPIC_API_KEY";
/// The Bedrock credential an activation record names: the access key id, whose secret and optional session token sign the request.
pub const BEDROCK_CREDENTIAL_NAME: &str = "AWS_ACCESS_KEY_ID";
/// The Anthropic Messages version Bedrock's `InvokeModel` body carries.
pub const BEDROCK_ANTHROPIC_VERSION: &str = "bedrock-2023-05-31";
pub const BEDROCK_PORT: u16 = 443;
/// The SigV4 service name Bedrock signs under.
pub const BEDROCK_SERVICE: &str = "bedrock";
/// How long a signed request stays within the provider's clock-skew window: a request is signed at most this long before its attempt deadline, so the provider never judges a stale request time.
pub const SIGNED_REQUEST_WINDOW_MS: i64 = 5 * 60 * 1000;
const _: () = assert!(
    memory_store::memory_reviewer_ledger::MEMORY_REVIEWER_ATTEMPT_MAX_MS < SIGNED_REQUEST_WINDOW_MS
);
/// Longest region name accepted: one DNS label of the runtime host.
pub const MAX_REGION_BYTES: usize = 63;
/// Longest Bedrock model id or inference profile accepted.
pub const MAX_MODEL_ID_BYTES: usize = 256;
/// Serialized request bytes a send may carry.
pub const MAX_REQUEST_BYTES: usize = 256 * 1024;
/// Output tokens a request may ask for.
pub const MAX_OUTPUT_TOKENS: u32 = 8_000;
/// Raw response body bytes one send may receive, charged before each chunk is kept; a provider error body counts the same.
pub const MAX_RAW_RESPONSE_BYTES: usize = 1024 * 1024;
/// Response head bytes accepted, and the headers a head may hold; the head is not part of the body allowance, so it gets its own bound. Hyper's read buffer is capped at it, with the slack its buffer growth allows, and the parsed head is then held to it exactly: header names and values plus any non-canonical reason phrase, the parts a peer chooses.
pub const MAX_RESPONSE_HEAD_BYTES: usize = 64 * 1024;
pub const MAX_RESPONSE_HEADERS: usize = 32;
/// Body frames one response may deliver, so a peer cannot trickle the allowance one byte at a time.
pub const MAX_RESPONSE_FRAMES: usize = 4096;
/// The connect phase clamps a longer caller deadline to this duration.
pub const MAX_CONNECT_DURATION: Duration = Duration::from_secs(30);
/// Fixed completion allowance before the per-token allowance is added.
pub const COMPLETION_FLOOR: Duration = Duration::from_secs(30);
pub const COMPLETION_PER_TOKEN: Duration = Duration::from_millis(30);
/// Limits the wait for each body frame after the response head arrives.
pub const FRAME_IDLE_TIMEOUT: Duration = Duration::from_secs(10);
const ALPN_HTTP1: &[u8] = b"http/1.1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    pub connect: Duration,
    pub completion_floor: Duration,
    pub completion_per_token: Duration,
    pub frame_idle: Duration,
}

impl Timing {
    pub const PRODUCTION: Timing = Timing {
        connect: MAX_CONNECT_DURATION,
        completion_floor: COMPLETION_FLOOR,
        completion_per_token: COMPLETION_PER_TOKEN,
        frame_idle: FRAME_IDLE_TIMEOUT,
    };

    /// Longest a completion may take for a request that asks for `max_tokens` output tokens.
    pub fn completion_budget(&self, max_tokens: u32) -> Duration {
        self.completion_floor
            .saturating_add(self.completion_per_token.saturating_mul(max_tokens))
    }
}

type Body = Full<Bytes>;
type Transport = TokioIo<TlsStream<TcpStream>>;

/// Why a send failed; host-authored codes and counters only, never provider text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SendError {
    #[error("request_too_large")]
    RequestTooLarge,
    #[error("output_tokens")]
    OutputTokens,
    /// The temperature is non-finite or outside the provider's `0.0..=1.0`.
    #[error("temperature")]
    Temperature,
    /// The credential is not a valid header value, or its identifier is empty.
    #[error("credential")]
    Credential,
    #[error("deadline")]
    Deadline,
    #[error("connect")]
    Connect,
    #[error("tls")]
    Tls,
    #[error("handshake")]
    Handshake,
    /// The connection could not take the request; it has been consumed and nothing was sent.
    #[error("not_ready")]
    NotReady,
    #[error("transport")]
    Transport,
    /// The provider returned a non-success status.
    #[error("status {0}")]
    Status(u16),
    #[error("compressed")]
    Compressed,
    #[error("content_type")]
    ContentType,
    #[error("response_too_large")]
    ResponseTooLarge,
    #[error("decode {0}")]
    Decode(DecodeError),
    #[error("egress_check")]
    EgressCheck,
    /// The region or model id cannot name a Bedrock endpoint.
    #[error("endpoint")]
    Endpoint,
    /// The body handed for signing is not the body the marker's digest names, or another provider shaped it.
    #[error("body_digest")]
    BodyDigest,
    /// The request time cannot be signed: outside the representable range, after the commit, or too far before the attempt deadline.
    #[error("sign_time")]
    SignTime,
}

impl SendError {
    pub fn refused_credential(&self) -> Option<u16> {
        match self {
            Self::Status(status) if refuses_credential(*status) => Some(*status),
            _ => None,
        }
    }
}

fn refuses_credential(status: u16) -> bool {
    matches!(status, 401 | 403)
}

/// A startup credential: the deployment owner's identifier for it and its secret material. The identifier is what an approval and an attempt marker name; an API key is rendered only into its authentication header, and an AWS secret only into the signing key derivation, never onto the wire. `Debug` never shows the material. Clones share one copy of the material, and its bytes are wiped when the last clone drops. Keeping both in one value means the credential a marker records is the one the request carries.
#[derive(Clone)]
pub struct Credential {
    id: String,
    material: Arc<Material>,
}

enum Material {
    /// The API key as the `x-api-key` value each request carries.
    ApiKey(HeaderValue),
    Aws {
        access_key_id: String,
        secret_access_key: Zeroizing<String>,
        /// The session token as the `x-amz-security-token` value each request carries and signs.
        session_token: Option<HeaderValue>,
    },
}

impl Credential {
    /// An API key. Refuses an empty identifier or a secret that cannot be a header value, so the refusal is named at startup rather than at the first send.
    pub fn new(id: String, secret: String) -> Result<Self, SendError> {
        let secret = secret_header(secret)?;
        if id.is_empty() {
            return Err(SendError::Credential);
        }
        Ok(Self {
            id,
            material: Arc::new(Material::ApiKey(secret)),
        })
    }

    /// Static AWS credentials. Refuses an empty identifier, key id, or secret, and a key id or session token that cannot be a header value.
    pub fn aws(
        id: String,
        access_key_id: String,
        secret_access_key: String,
        session_token: Option<String>,
    ) -> Result<Self, SendError> {
        let secret_access_key = Zeroizing::new(secret_access_key);
        // An empty token is no token: it is neither signed nor sent.
        let session_token = session_token
            .filter(|token| !token.is_empty())
            .map(secret_header)
            .transpose()?;
        if id.is_empty() || access_key_id.is_empty() || secret_access_key.is_empty() {
            return Err(SendError::Credential);
        }
        if !visible_ascii(&access_key_id) {
            return Err(SendError::Credential);
        }
        Ok(Self {
            id,
            material: Arc::new(Material::Aws {
                access_key_id,
                secret_access_key,
                session_token,
            }),
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }
}

/// Whether `value` holds only the bytes [`HeaderValue::from_str`] accepts: visible ASCII, space, and tab.
fn visible_ascii(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| (b' '..0x7f).contains(&byte) || byte == b'\t')
}

/// `secret` as a sensitive header value over one buffer that is wiped when the last clone drops; requests carry clones of it. Refuses a secret [`HeaderValue::from_str`] refuses.
fn secret_header(secret: String) -> Result<HeaderValue, SendError> {
    let owner = Zeroizing::new(secret.into_bytes());
    if !std::str::from_utf8(&owner).is_ok_and(visible_ascii) {
        return Err(SendError::Credential);
    }
    let mut value = HeaderValue::from_maybe_shared(Bytes::from_owner(owner))
        .map_err(|_| SendError::Credential)?;
    value.set_sensitive(true);
    Ok(value)
}

/// The text of a header value this module built from visible ASCII.
fn header_text(value: &HeaderValue) -> Result<&str, SendError> {
    std::str::from_utf8(value.as_bytes()).map_err(|_| SendError::Credential)
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Credential({}, <redacted>)", self.id)
    }
}

/// Where the sender connects: a host, a port, and the TLS configuration that verifies it. Production has exactly one.
#[derive(Clone)]
pub struct Endpoint {
    host: String,
    /// `host` as the `host` header value each request carries.
    host_header: HeaderValue,
    port: u16,
    server_name: ServerName<'static>,
    tls: Arc<ClientConfig>,
}

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Endpoint({}:{})", self.host, self.port)
    }
}

impl Endpoint {
    /// The production endpoint: the fixed Anthropic host over the webpki roots.
    fn anthropic() -> Self {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        Self::with_roots(ANTHROPIC_HOST, ANTHROPIC_PORT, roots)
            .expect("the production host name is a valid server name")
    }

    /// The production Bedrock runtime endpoint of `region` over the webpki roots.
    fn bedrock(region: &str) -> Result<Self, SendError> {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        Self::with_roots(
            &format!("bedrock-runtime.{region}.amazonaws.com"),
            BEDROCK_PORT,
            roots,
        )
    }

    /// A local peer for the sender proof; the roots are the test's own authority. Compiled only for tests.
    #[cfg(any(test, feature = "test-support"))]
    pub fn for_test(
        host: &str,
        port: u16,
        roots: rustls::RootCertStore,
    ) -> Result<Self, SendError> {
        Self::with_roots(host, port, roots)
    }

    fn with_roots(host: &str, port: u16, roots: rustls::RootCertStore) -> Result<Self, SendError> {
        let server_name = ServerName::try_from(host.to_string()).map_err(|_| SendError::Tls)?;
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut tls = ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
            .map_err(|_| SendError::Tls)?
            .with_root_certificates(roots)
            .with_no_client_auth();
        tls.alpn_protocols = vec![ALPN_HTTP1.to_vec()];
        Ok(Self {
            host_header: HeaderValue::from_str(host).map_err(|_| SendError::Tls)?,
            host: host.to_string(),
            port,
            server_name,
            tls: Arc::new(tls),
        })
    }
}

/// The provider a sender speaks to: its endpoint identity, the one startup credential its
/// protocol carries, the body and headers it accepts, and the model it reports answering with.
/// The variants are this module's own; code elsewhere holds the providers this module's
/// constructors return and reads what they name, and only this module builds or matches one:
///
/// ```compile_fail,E0603
/// use daemon::memory_reviewer::model_request::Provider;
/// fn variant(provider: &Provider) {
///     let Provider(_) = provider;
/// }
/// ```
#[derive(Debug, Clone)]
pub struct Provider(ProviderKind);

#[derive(Debug, Clone)]
enum ProviderKind {
    /// Anthropic Messages on the fixed production host, or a test peer standing in for it.
    Anthropic(Arc<Endpoint>),
    /// Bedrock `InvokeModel` for one model id in one region, signed with SigV4.
    Bedrock(Bedrock),
}

#[derive(Debug, Clone)]
struct Bedrock {
    endpoint: Arc<Endpoint>,
    region: String,
    /// The identity format is `<host>/model/<model id>/invoke@<version>`.
    identity: String,
    /// The `InvokeModel` path with the model id encoded as one segment, as the wire carries it.
    uri: Uri,
    /// The wire path encoded again, as SigV4 signs it.
    canonical_uri: String,
}

/// Whether `region` can name a Bedrock region: one DNS label of lowercase letters, digits, and inner `-`.
pub fn valid_region(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= MAX_REGION_BYTES
        && !region.starts_with('-')
        && !region.ends_with('-')
        && region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// Whether `model_id` can name a Bedrock model or inference profile as one path segment: printable ASCII, at most [`MAX_MODEL_ID_BYTES`], and neither `.` nor `..`.
pub fn valid_model_id(model_id: &str) -> bool {
    !model_id.is_empty()
        && model_id != "."
        && model_id != ".."
        && model_id.len() <= MAX_MODEL_ID_BYTES
        && model_id
            .bytes()
            .all(|byte| byte.is_ascii() && !byte.is_ascii_control())
}

/// `/model/<model id>/invoke` with every byte of the model id outside the unreserved set percent-encoded, so `:` and `/` stay inside the one segment.
fn invoke_path(model_id: &str) -> Result<String, SendError> {
    if !valid_model_id(model_id) {
        return Err(SendError::Endpoint);
    }
    Ok(format!(
        "/model/{}/invoke",
        super::sigv4::canonical_uri(model_id).replace('/', "%2F")
    ))
}

/// The providers a deployment can dial: Anthropic, and Bedrock in the region the startup envelope names, once an activation record names the model id it invokes. A region outside [`valid_region`] leaves Bedrock undialable for the life of the process.
#[derive(Debug, Clone)]
pub struct Providers {
    anthropic: Provider,
    bedrock: Option<(Arc<Endpoint>, String)>,
}

impl Providers {
    /// The production endpoints, with Bedrock in `region` when it is valid.
    pub fn production(region: Option<&str>) -> Self {
        Self {
            anthropic: Provider::anthropic(),
            bedrock: region
                .filter(|region| valid_region(region))
                .and_then(|region| {
                    Some((
                        Arc::new(Endpoint::bedrock(region).ok()?),
                        region.to_string(),
                    ))
                }),
        }
    }

    /// Both providers on local peers for the sender proof; `region` is signed for as given. Compiled only for tests.
    #[cfg(any(test, feature = "test-support"))]
    pub fn at(anthropic: Endpoint, bedrock: Endpoint, region: &str) -> Self {
        Self {
            anthropic: Provider::anthropic_at(anthropic),
            bedrock: valid_region(region).then(|| (Arc::new(bedrock), region.to_string())),
        }
    }

    pub fn anthropic(&self) -> &Provider {
        &self.anthropic
    }

    /// Bedrock invoking `model_id` in the startup region. [`SendError::Endpoint`] names an invalid or absent region, a model id outside [`valid_model_id`], or an identity longer than the ledger's provider bound.
    pub fn bedrock(&self, model_id: &str) -> Result<Provider, SendError> {
        let (endpoint, region) = self.bedrock.as_ref().ok_or(SendError::Endpoint)?;
        Provider::bedrock_with(endpoint.clone(), region, model_id)
    }
}

impl Provider {
    /// The credential this provider's requests carry, read from the startup envelope's rows by `row` and identified as `id`: the API key for Anthropic; the access key id, secret, and optional session token for Bedrock.
    pub fn credential(
        &self,
        id: String,
        row: impl Fn(&str) -> Option<String>,
    ) -> Result<Credential, SendError> {
        match &self.0 {
            ProviderKind::Anthropic(_) => {
                Credential::new(id, row(CREDENTIAL_NAME).ok_or(SendError::Credential)?)
            }
            ProviderKind::Bedrock(_) => Credential::aws(
                id,
                row(BEDROCK_CREDENTIAL_NAME).ok_or(SendError::Credential)?,
                row("AWS_SECRET_ACCESS_KEY").ok_or(SendError::Credential)?,
                row("AWS_SESSION_TOKEN"),
            ),
        }
    }
}

impl Provider {
    /// Anthropic Messages on the production host over the webpki roots.
    pub fn anthropic() -> Self {
        Self(ProviderKind::Anthropic(Arc::new(Endpoint::anthropic())))
    }

    /// Anthropic Messages on a local peer for the sender proof. Compiled only for tests.
    #[cfg(any(test, feature = "test-support"))]
    pub fn anthropic_at(endpoint: Endpoint) -> Self {
        Self(ProviderKind::Anthropic(Arc::new(endpoint)))
    }

    /// Bedrock on a local peer for the sender proof. Compiled only for tests.
    #[cfg(any(test, feature = "test-support"))]
    pub fn bedrock_at(endpoint: Endpoint, region: &str, model_id: &str) -> Result<Self, SendError> {
        if !valid_region(region) {
            return Err(SendError::Endpoint);
        }
        Self::bedrock_with(Arc::new(endpoint), region, model_id)
    }

    /// The [`MAX_PROVIDER_BYTES`] limit keeps the constructed provider's identity within the attempt-marker ledger's size bound.
    ///
    /// [`MAX_PROVIDER_BYTES`]: memory_store::memory_reviewer_ledger::MAX_PROVIDER_BYTES
    fn bedrock_with(
        endpoint: Arc<Endpoint>,
        region: &str,
        model_id: &str,
    ) -> Result<Self, SendError> {
        let path = invoke_path(model_id)?;
        let identity = [
            endpoint.host.as_str(),
            "/model/",
            model_id,
            "/invoke@",
            BEDROCK_ANTHROPIC_VERSION,
        ]
        .concat();
        if identity.len() > memory_store::memory_reviewer_ledger::MAX_PROVIDER_BYTES {
            return Err(SendError::Endpoint);
        }
        let canonical_uri = super::sigv4::canonical_uri(&path);
        Ok(Self(ProviderKind::Bedrock(Bedrock {
            endpoint,
            region: region.to_string(),
            identity,
            uri: Uri::try_from(path).map_err(|_| SendError::Endpoint)?,
            canonical_uri,
        })))
    }

    fn endpoint(&self) -> &Endpoint {
        match &self.0 {
            ProviderKind::Anthropic(endpoint) => endpoint,
            ProviderKind::Bedrock(bedrock) => &bedrock.endpoint,
        }
    }

    /// The provider identity a disclosure marker and an approval record: the host this provider
    /// dials, the API surface (for Bedrock, the model id it invokes), and the API version it speaks.
    pub fn identity(&self) -> String {
        match &self.0 {
            ProviderKind::Anthropic(endpoint) => [
                endpoint.host.as_str(),
                MESSAGES_PATH,
                "@",
                ANTHROPIC_VERSION,
            ]
            .concat(),
            ProviderKind::Bedrock(bedrock) => bedrock.identity.clone(),
        }
    }

    /// The one startup-envelope credential an activation record names for this provider; another
    /// provider's secret never reaches its host.
    pub fn credential_name(&self) -> &'static str {
        match &self.0 {
            ProviderKind::Anthropic(_) => CREDENTIAL_NAME,
            ProviderKind::Bedrock(_) => BEDROCK_CREDENTIAL_NAME,
        }
    }

    /// The model a response must report for its text to be released when `requested`, the
    /// record's `model`, was asked for. Both providers report it: Anthropic the model id the body
    /// names, and Bedrock the base model behind the path's model id.
    pub fn expected_model<'a>(&self, requested: &'a str) -> &'a str {
        match &self.0 {
            ProviderKind::Anthropic(_) | ProviderKind::Bedrock(_) => requested,
        }
    }

    /// The serialized body this provider accepts for `request`, refused when it asks for more
    /// output than [`MAX_OUTPUT_TOKENS`], exceeds [`MAX_REQUEST_BYTES`], or carries a temperature
    /// JSON cannot represent exactly (non-finite) or the provider does not accept (outside
    /// `0.0..=1.0`). Bedrock's body names no model and no stream; its path names the model.
    fn body(&self, request: &MessagesRequest) -> Result<RequestBody, SendError> {
        if request.max_tokens == 0 || request.max_tokens > MAX_OUTPUT_TOKENS {
            return Err(SendError::OutputTokens);
        }
        if request
            .temperature
            .is_some_and(|temperature| !(0.0..=1.0).contains(&temperature))
        {
            return Err(SendError::Temperature);
        }
        let body = match &self.0 {
            ProviderKind::Anthropic(_) => serde_json::to_vec(&WireRequest {
                model: &request.model,
                system: request.system.as_deref(),
                messages: &request.messages,
                max_tokens: request.max_tokens,
                temperature: request.temperature,
                stream: false,
            }),
            ProviderKind::Bedrock(_) => serde_json::to_vec(&BedrockWireRequest {
                anthropic_version: BEDROCK_ANTHROPIC_VERSION,
                system: request.system.as_deref(),
                messages: &request.messages,
                max_tokens: request.max_tokens,
                temperature: request.temperature,
            }),
        }
        .map_err(|_| SendError::RequestTooLarge)?;
        if body.len() > MAX_REQUEST_BYTES {
            return Err(SendError::RequestTooLarge);
        }
        Ok(RequestBody {
            digest: format!("{:x}", sha2::Sha256::digest(&body)),
            bytes: Bytes::from(body),
            max_tokens: request.max_tokens,
            bedrock: matches!(self.0, ProviderKind::Bedrock(_)),
        })
    }

    /// The one request carrying `body`, signed or authenticated as this provider requires at
    /// `at_ms`. An API key is written into `x-api-key` and nowhere else; AWS credentials sign
    /// the method, the path, `content-type`, `host`, `x-amz-content-sha256` (the body's digest,
    /// which must equal `body_digest`), `x-amz-date`, and the session token when there is one,
    /// and only the access key id and the signature reach the wire. A credential of another
    /// provider's kind is refused.
    fn request(
        &self,
        body: RequestBody,
        body_digest: &str,
        at_ms: i64,
        credential: &Credential,
    ) -> Result<Request<Body>, SendError> {
        if body.digest != body_digest || body.bedrock != matches!(self.0, ProviderKind::Bedrock(_))
        {
            return Err(SendError::BodyDigest);
        }
        let host = &self.endpoint().host_header;
        let uri = match &self.0 {
            ProviderKind::Anthropic(_) => Uri::from_static(MESSAGES_PATH),
            ProviderKind::Bedrock(bedrock) => bedrock.uri.clone(),
        };
        let mut request = Request::new(Full::new(body.bytes));
        *request.method_mut() = Method::POST;
        *request.uri_mut() = uri;
        let headers = request.headers_mut();
        headers.reserve(10);
        headers.insert(header::ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("identity"),
        );
        headers.insert(header::CONNECTION, HeaderValue::from_static("close"));
        match (&self.0, &*credential.material) {
            (ProviderKind::Anthropic(_), Material::ApiKey(key)) => {
                headers.insert(header::HOST, host.clone());
                headers.insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                );
                headers.insert(HeaderName::from_static("x-api-key"), key.clone());
                headers.insert(
                    HeaderName::from_static("anthropic-version"),
                    HeaderValue::from_static(ANTHROPIC_VERSION),
                );
            }
            (
                ProviderKind::Bedrock(bedrock),
                Material::Aws {
                    access_key_id,
                    secret_access_key,
                    session_token,
                },
            ) => {
                let time = super::sigv4::RequestTime::at(at_ms).ok_or(SendError::SignTime)?;
                let value = |text: &str| {
                    HeaderValue::from_str(text).map_err(|_| SendError::RequestTooLarge)
                };
                // The one list of signed headers: the signature covers it, and the request carries it.
                let signed = [
                    (
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("application/json"),
                    ),
                    (header::HOST, host.clone()),
                    (
                        HeaderName::from_static("x-amz-content-sha256"),
                        value(body_digest)?,
                    ),
                    (
                        HeaderName::from_static("x-amz-date"),
                        value(&time.amz_date)?,
                    ),
                    (
                        HeaderName::from_static("x-amz-security-token"),
                        session_token
                            .clone()
                            .unwrap_or(HeaderValue::from_static("")),
                    ),
                ];
                let count = signed.len() - usize::from(session_token.is_none());
                let mut names = [("", ""); 5];
                for (pair, (name, value)) in names.iter_mut().zip(&signed) {
                    *pair = (name.as_str(), header_text(value)?);
                }
                let (canonical_digest, signed_headers) = super::sigv4::Request {
                    method: "POST",
                    canonical_uri: &bedrock.canonical_uri,
                    canonical_query: "",
                    headers: &names[..count],
                    payload_sha256: body_digest,
                }
                .digest();
                let scope = super::sigv4::Scope {
                    date: time.date(),
                    region: &bedrock.region,
                    service: BEDROCK_SERVICE,
                };
                let to_sign =
                    super::sigv4::string_to_sign_of_digest(&time, &scope, &canonical_digest);
                let signature = super::sigv4::signature(secret_access_key, &scope, &to_sign);
                let authorization =
                    super::sigv4::authorization(access_key_id, &scope, &signed_headers, &signature);
                let mut authorization = HeaderValue::from_maybe_shared(Bytes::from(authorization))
                    .map_err(|_| SendError::Credential)?;
                authorization.set_sensitive(true);
                headers.insert(header::AUTHORIZATION, authorization);
                for (name, value) in signed.into_iter().take(count) {
                    headers.insert(name, value);
                }
            }
            _ => return Err(SendError::Credential),
        }
        Ok(request)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

/// One Messages request. It is serialized with `stream: false` and no `tools`; the model, the token cap, and the one sampling value are the whole configurable profile, and an omitted `temperature` is serialized as an omission.
#[derive(Debug, Clone, PartialEq)]
pub struct MessagesRequest {
    pub model: String,
    pub system: Option<String>,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
    pub temperature: Option<f64>,
}

#[derive(Serialize)]
struct WireRequest<'a> {
    model: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<&'a str>,
    messages: &'a [Message],
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    stream: bool,
}

#[derive(Serialize)]
struct BedrockWireRequest<'a> {
    anthropic_version: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<&'a str>,
    messages: &'a [Message],
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
}

/// Serialized request bytes that passed [`Sender::body`]'s shape, token, and size bounds, with their SHA-256 digest and the `max_tokens` the bytes ask for, which sizes the completion budget. [`Sender::body`] is the only constructor and the fields stay fixed afterwards, so a handoff carries bounded bytes and the digest names exactly those bytes. Clones share the byte storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestBody {
    bytes: Bytes,
    digest: String,
    max_tokens: u32,
    /// Whether a Bedrock provider shaped the bytes; a body is sent only by a provider of its own shape.
    bedrock: bool,
}

impl RequestBody {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The lowercase hex SHA-256 of [`Self::as_bytes`].
    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

/// The one validated assistant message a completed send yields, with what receiving it cost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistantText {
    pub text: String,
    pub stop_reason: Option<StopReason>,
    /// The model the provider reports having answered with; the caller compares it with the one it requested.
    pub model: Option<String>,
    pub accounting: ResponseAccounting,
}

/// Bytes a response cost, kept apart: the parsed head (header names and values plus any non-canonical reason phrase, not delimiters), the body the transport delivered, the assistant text the decoder measured, and what the parser allocated for itself. `transport_bytes` and `decoded_text_bytes` are the two the job's cumulative ceilings charge: exact on success, and the whole remaining bound on an overflow refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResponseAccounting {
    pub head_bytes: usize,
    pub transport_bytes: usize,
    pub decoded_text_bytes: usize,
    pub parser_scratch_bytes: usize,
}

// The per-response constants are the per-job ceilings the ledger enforces across attempts; a response may never consume more than the job has left.
const _: () = assert!(
    MAX_RAW_RESPONSE_BYTES as u64
        == memory_store::memory_reviewer_ledger::MEMORY_REVIEWER_MAX_RAW_RESPONSE_BYTES
);
const _: () = assert!(
    MAX_ASSISTANT_TEXT_BYTES as u64
        == memory_store::memory_reviewer_ledger::MEMORY_REVIEWER_MAX_DECODED_TEXT_BYTES
);

#[derive(Debug, Clone)]
pub struct Sender {
    provider: Provider,
    credential: Credential,
    timing: Timing,
}

impl Sender {
    /// The provider identity a disclosure marker records: the host this sender actually dials, the API surface, and the API version it speaks.
    pub fn provider_identity(&self) -> String {
        self.provider.identity()
    }

    pub fn provider(&self) -> &Provider {
        &self.provider
    }

    /// The identifier of the credential this sender writes into the authentication header.
    pub fn credential_id(&self) -> &str {
        self.credential.id()
    }

    /// The body this sender's provider accepts for `request`; the bytes a marker hashes are the
    /// bytes the handoff sends.
    pub fn body(&self, request: &MessagesRequest) -> Result<RequestBody, SendError> {
        self.provider.body(request)
    }

    pub fn new(provider: Provider, credential: Credential) -> Self {
        Self {
            provider,
            credential,
            timing: Timing::PRODUCTION,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn with_timing(provider: Provider, credential: Credential, timing: Timing) -> Self {
        Self {
            provider,
            credential,
            timing,
        }
    }

    /// Opens a fresh connection and completes the TCP, TLS, and HTTP/1 handshakes by `deadline`, clamped to [`Timing::connect`]. No request byte is written.
    pub async fn connect(&self, deadline: Instant) -> Result<Connected, SendError> {
        let endpoint = self.provider.endpoint();
        let handshakes = async {
            let tcp = TcpStream::connect((endpoint.host.as_str(), endpoint.port))
                .await
                .map_err(|_| SendError::Connect)?;
            tcp.set_nodelay(true).map_err(|_| SendError::Connect)?;
            let tls = TlsConnector::from(endpoint.tls.clone())
                .connect(endpoint.server_name.clone(), tcp)
                .await
                .map_err(|_| SendError::Tls)?;
            let (send, connection) = hyper::client::conn::http1::Builder::new()
                .max_buf_size(MAX_RESPONSE_HEAD_BYTES)
                .max_headers(MAX_RESPONSE_HEADERS)
                .handshake(TokioIo::new(tls))
                .await
                .map_err(|_| SendError::Handshake)?;
            Ok(Connected {
                send,
                connection,
                provider: self.provider.clone(),
                credential: self.credential.clone(),
                timing: self.timing,
            })
        };
        tokio::time::timeout_at(clamp(deadline, self.timing.connect), handshakes)
            .await
            .map_err(|_| SendError::Deadline)?
    }
}

fn clamp(deadline: Instant, budget: Duration) -> Instant {
    deadline.min(Instant::now() + budget)
}

/// A handshaken, unpolled connection that can carry exactly one request.
pub struct Connected {
    send: SendRequest<Body>,
    connection: Connection<Transport, Body>,
    provider: Provider,
    credential: Credential,
    timing: Timing,
}

impl std::fmt::Debug for Connected {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Connected({})", self.provider.endpoint().host)
    }
}

impl Connected {
    /// Builds the one request this connection will carry: `body`, whose SHA-256 must be `body_digest`, authenticated or signed as the provider requires at `at_ms`. Every header is computed here and nothing is written to the peer; a signed request that is never handed off is dropped with the connection.
    pub fn sign(
        self,
        body: RequestBody,
        body_digest: &str,
        at_ms: i64,
    ) -> Result<Signed, SendError> {
        let max_tokens = body.max_tokens;
        let request = self
            .provider
            .request(body, body_digest, at_ms, &self.credential)?;
        Ok(Signed {
            send: self.send,
            connection: self.connection,
            request,
            signed_at_ms: at_ms,
            timing: self.timing,
            max_tokens,
        })
    }
}

impl Connected {
    /// [`Self::sign`] over `body`'s own digest at the wall clock, for the sender proof. Compiled only for tests.
    #[cfg(any(test, feature = "test-support"))]
    pub fn sign_now(self, body: RequestBody) -> Result<Signed, SendError> {
        let digest = format!("{:x}", sha2::Sha256::digest(body.as_bytes()));
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| {
                i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
            });
        self.sign(body, &digest, now_ms)
    }
}

/// A connection and the request it will carry, signed but not yet handed off.
pub struct Signed {
    send: SendRequest<Body>,
    connection: Connection<Transport, Body>,
    request: Request<Body>,
    signed_at_ms: i64,
    timing: Timing,
    max_tokens: u32,
}

impl std::fmt::Debug for Signed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Signed(at {})", self.signed_at_ms)
    }
}

impl Signed {
    /// The one-shot handoff: synchronously moves the signed request into the connection's dispatch queue and returns the in-flight send. Signing computed every header and the body was bounded beforehand ([`Sender::body`]), so the handoff encodes and builds nothing. Nothing is written to the peer until [`InFlight::complete`] polls the connection; a connection that turns out unable to take the request reports `NotReady` there, with nothing sent.
    pub fn handoff(
        mut self,
    ) -> InFlight<impl std::future::Future<Output = Result<Response<Incoming>, TrySendError>> + Send>
    {
        // `try_send_request` moves the request into the dispatch queue before it returns its future; a fresh connection admits exactly one request before it is polled.
        let response = self.send.try_send_request(self.request);
        InFlight {
            connection: self.connection,
            response,
            timing: self.timing,
            max_tokens: self.max_tokens,
        }
    }
}

type TrySendError = hyper::client::conn::TrySendError<Request<Body>>;

/// A request the connection holds but has not yet written.
pub struct InFlight<F> {
    connection: Connection<Transport, Body>,
    response: F,
    timing: Timing,
    max_tokens: u32,
}

impl<F> std::fmt::Debug for InFlight<F> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("InFlight")
    }
}

impl<F> InFlight<F>
where
    F: std::future::Future<Output = Result<Response<Incoming>, TrySendError>>,
{
    /// Polls the connection, so the request is written, and reads the one response by `deadline`, clamped to [`Timing::completion_budget`] for the request's `max_tokens`, with at most [`Timing::frame_idle`] between body frames. The connection is dropped afterwards whatever the outcome; there is no second attempt. The body is charged against `allowance` chunk by chunk before it is kept and the text against it before it is allocated; `accounting` is the caller's and holds what was consumed whether the exchange completes, refuses, times out, or is dropped.
    pub async fn complete(
        self,
        deadline: Instant,
        allowance: ResponseAllowance,
        accounting: &mut ResponseAccounting,
    ) -> Result<AssistantText, SendError> {
        let InFlight {
            mut connection,
            response,
            timing,
            max_tokens,
        } = self;
        let mut response = std::pin::pin!(response);
        let raw_bound = usize::try_from(allowance.raw_response_bytes)
            .unwrap_or(usize::MAX)
            .min(MAX_RAW_RESPONSE_BYTES);
        let text_bound = usize::try_from(allowance.decoded_text_bytes).unwrap_or(usize::MAX);
        // Set once a head refusing the credential arrives. A refused credential is the answer whatever the body then does, so its status replaces any later refusal or timeout, and a caller can end every further send on it; the accounting still records what the body consumed.
        let mut refused = None;
        let exchange = async {
            // The connection may finish in the same poll that delivers the response; a finished connection is never polled again, and the response it already delivered is taken from the future.
            let mut connection_done = false;
            let unsent = |error: TrySendError| {
                let mut error = error;
                if error.take_message().is_some() {
                    SendError::NotReady
                } else {
                    SendError::Transport
                }
            };
            // The head wait is the provider's whole generation time for a non-streaming request, so only the completion budget bounds it; the frame idle limit starts with the body.
            let response = tokio::select! {
                biased;
                response = &mut response => response.map_err(unsent),
                closed = &mut connection => {
                    closed.map_err(|_| SendError::Transport)?;
                    connection_done = true;
                    response.await.map_err(unsent)
                }
            }?;
            let (head, body) = response.into_parts();
            refused = Some(head.status.as_u16()).filter(|status| refuses_credential(*status));
            // Hyper refuses a head it cannot buffer; a head it could buffer is still held to the declared bound before anything else is read. The reason phrase is counted because it is the one head part outside the headers that a peer sizes freely.
            accounting.head_bytes = head
                .headers
                .iter()
                .map(|(name, value)| name.as_str().len() + value.len())
                .sum::<usize>()
                + head
                    .extensions
                    .get::<hyper::ext::ReasonPhrase>()
                    .map_or(0, |reason| reason.as_bytes().len());
            if accounting.head_bytes > MAX_RESPONSE_HEAD_BYTES {
                return Err(SendError::ResponseTooLarge);
            }
            if head
                .headers
                .get_all(header::CONTENT_ENCODING)
                .iter()
                .any(|value| !value.as_bytes().eq_ignore_ascii_case(b"identity"))
            {
                return Err(SendError::Compressed);
            }
            // A declared length past the bound refuses without reading the body; the bound counts as consumed, since nothing durable can say less was.
            if let Some(length) = head
                .headers
                .get(header::CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<usize>().ok())
                && length > raw_bound
            {
                accounting.transport_bytes = raw_bound;
                return Err(SendError::ResponseTooLarge);
            }
            let bytes = collect_body(
                body,
                &mut connection,
                connection_done,
                timing.frame_idle,
                raw_bound,
                &mut *accounting,
            )
            .await?;
            if head.status != StatusCode::OK {
                return Err(SendError::Status(head.status.as_u16()));
            }
            let json = head
                .headers
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| {
                    value.split(';').next().is_some_and(|media_type| {
                        media_type.trim().eq_ignore_ascii_case("application/json")
                    })
                });
            if !json {
                return Err(SendError::ContentType);
            }
            let (decoded, text_bytes) = decode_message_within(&bytes, text_bound);
            accounting.decoded_text_bytes = text_bytes;
            let decoded = decoded.map_err(SendError::Decode)?;
            accounting.parser_scratch_bytes = decoded.scratch_bytes;
            check_render(decoded.text.as_bytes(), None).map_err(|_| SendError::EgressCheck)?;
            Ok(AssistantText {
                text: decoded.text,
                stop_reason: decoded.stop_reason,
                model: decoded.model,
                accounting: *accounting,
            })
        };
        let outcome = tokio::time::timeout_at(
            clamp(deadline, timing.completion_budget(max_tokens)),
            exchange,
        )
        .await
        .unwrap_or(Err(SendError::Deadline));
        outcome.map_err(|error| refused.map_or(error, SendError::Status))
    }
}

/// Reads every body frame while keeping the connection polled, charging each chunk against `raw_bound` before it is kept. The bytes of a body that overflows are not retained, and the overflow is recorded as the whole bound consumed. Once the connection has finished, cleanly or not, it is never polled again; frames it already delivered are still drained, and a body cut short reports itself through its own frame error.
async fn collect_body(
    mut body: Incoming,
    connection: &mut Connection<Transport, Body>,
    mut connection_done: bool,
    frame_idle: Duration,
    raw_bound: usize,
    accounting: &mut ResponseAccounting,
) -> Result<Vec<u8>, SendError> {
    let mut bytes = Vec::new();
    let mut frames = 0usize;
    loop {
        let next = tokio::time::timeout(frame_idle, async {
            if connection_done {
                return body.frame().await;
            }
            tokio::select! {
                biased;
                frame = body.frame() => frame,
                _ = &mut *connection => {
                    connection_done = true;
                    body.frame().await
                }
            }
        })
        .await
        .map_err(|_| SendError::Deadline)?;
        let Some(frame) = next else {
            return Ok(bytes);
        };
        let frame = frame.map_err(|_| SendError::Transport)?;
        frames += 1;
        // A body in more frames than admitted is refused like one over the byte bound, and charged the same way: nothing durable says it was shorter.
        if frames > MAX_RESPONSE_FRAMES {
            accounting.transport_bytes = raw_bound;
            return Err(SendError::ResponseTooLarge);
        }
        if let Some(chunk) = frame.data_ref() {
            let charged = accounting.transport_bytes.saturating_add(chunk.len());
            if charged > raw_bound {
                accounting.transport_bytes = raw_bound;
                return Err(SendError::ResponseTooLarge);
            }
            accounting.transport_bytes = charged;
            bytes.extend_from_slice(chunk);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_production_completion_budget_covers_the_largest_request() {
        // Anthropic's SDKs estimate a non-streaming request at 3600 s per 128,000 output tokens and refuse one expected to exceed ten minutes; the budget for the largest admissible request must sit between the two.
        let sdk_estimate = Duration::from_secs(3600 * u64::from(MAX_OUTPUT_TOKENS) / 128_000);
        let budget = Timing::PRODUCTION.completion_budget(MAX_OUTPUT_TOKENS);
        assert!(budget >= sdk_estimate, "{budget:?} < {sdk_estimate:?}");
        assert!(budget <= Duration::from_secs(600), "{budget:?}");
        assert_eq!(
            Timing::PRODUCTION.completion_budget(1),
            COMPLETION_FLOOR + COMPLETION_PER_TOKEN
        );
    }
}
