//! One verified, one-use sender for Anthropic Messages requests.
//!
//! The sender opens a fresh TCP connection to the fixed production host, completes a rustls handshake that verifies the full chain and hostname against the webpki roots with SNI for that host and TLS 1.2 or newer, and completes an HTTP/1 handshake, all without writing a byte of any request. The caller then hands over exactly one owned request; the handoff is synchronous and moves the request into the connection's dispatch queue, and no request byte reaches the peer until the caller asks for completion. Completing polls the connection once, reads the response under the raw-byte allowance, refuses compressed or non-JSON bodies, decodes the message under the closed bounds, runs the common render check, and yields one [`AssistantText`]. There is no retry, redirect, reconnect, proxy, pool, or fallback anywhere in this module; a second request needs a second connection. The credential is written into the `x-api-key` header and nowhere else.
//!
//! Hyper and rustls connection types never leave this module; callers see [`Sender`], [`Connected`], [`InFlight`], and [`AssistantText`]. An endpoint other than the production one exists only under the `test-support` feature, for the local TLS peer the sender proof runs against.

use std::sync::Arc;

use std::time::Duration;

use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::client::conn::http1::{Connection, SendRequest};
use hyper::header::{self, HeaderValue};
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use rustls::ClientConfig;
use rustls::pki_types::ServerName;
use serde::Serialize;
use tokio::net::TcpStream;
use tokio::time::Instant;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use zeroize::Zeroizing;

use super::broker::check_render;
use super::model_response::{DecodeError, StopReason, decode_message};

pub const ANTHROPIC_HOST: &str = "api.anthropic.com";
pub const ANTHROPIC_PORT: u16 = 443;
pub const MESSAGES_PATH: &str = "/v1/messages";
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Serialized request bytes a send may carry.
pub const MAX_REQUEST_BYTES: usize = 256 * 1024;
/// Output tokens a request may ask for.
pub const MAX_OUTPUT_TOKENS: u32 = 8_000;
/// Raw response body bytes one send may receive, charged before each chunk is kept; a provider error body counts the same.
pub const MAX_RAW_RESPONSE_BYTES: usize = 1024 * 1024;
/// Response head bytes accepted, and the headers a head may hold; the head is not part of the body allowance, so it gets its own bound, applied both to hyper's read buffer and to the parsed headers.
pub const MAX_RESPONSE_HEAD_BYTES: usize = 64 * 1024;
pub const MAX_RESPONSE_HEADERS: usize = 32;
/// Body frames one response may deliver, so a peer cannot trickle the allowance one byte at a time.
pub const MAX_RESPONSE_FRAMES: usize = 4096;
/// Longest a connect or a completion may take whatever deadline the caller passes.
pub const MAX_PHASE_DURATION: Duration = Duration::from_secs(30);
/// Longest wait for the next byte of a response.
pub const FRAME_IDLE_TIMEOUT: Duration = Duration::from_secs(10);
const ALPN_HTTP1: &[u8] = b"http/1.1";

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
    /// The provider answered with a non-success status; the body was read and discarded under the allowance.
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
}

/// A startup credential: the deployment owner's identifier for it and the secret. The identifier is what an approval and an attempt marker name; the secret is rendered only into the authentication header, `Debug` never shows it, and its bytes are wiped when the last copy drops. Keeping both in one value means the credential a marker records is the one the header carries.
#[derive(Clone)]
pub struct Credential {
    id: String,
    secret: Zeroizing<String>,
}

impl Credential {
    /// Refuses an empty identifier or a secret that cannot be a header value, so the refusal is named at startup rather than at the first send.
    pub fn new(id: String, secret: String) -> Result<Self, SendError> {
        let secret = Zeroizing::new(secret);
        if id.is_empty() {
            return Err(SendError::Credential);
        }
        HeaderValue::from_str(&secret).map_err(|_| SendError::Credential)?;
        Ok(Self { id, secret })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    fn header(&self) -> HeaderValue {
        let mut value =
            HeaderValue::from_str(&self.secret).unwrap_or_else(|_| HeaderValue::from_static(""));
        value.set_sensitive(true);
        value
    }
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
    pub fn anthropic() -> Self {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        Self::with_roots(ANTHROPIC_HOST, ANTHROPIC_PORT, roots)
            .expect("the production host name is a valid server name")
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
            host: host.to_string(),
            port,
            server_name,
            tls: Arc::new(tls),
        })
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

/// Serialized request bytes that passed [`MessagesRequest::body`]'s shape, token, and size bounds. Only that constructor produces one, so a handoff cannot carry bytes the bounds never saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestBody(Vec<u8>);

impl RequestBody {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl MessagesRequest {
    /// The serialized body, refused when it asks for more output than [`MAX_OUTPUT_TOKENS`], exceeds [`MAX_REQUEST_BYTES`], or carries a temperature JSON cannot represent exactly (non-finite) or the provider does not accept (outside `0.0..=1.0`).
    pub fn body(&self) -> Result<RequestBody, SendError> {
        if self.max_tokens == 0 || self.max_tokens > MAX_OUTPUT_TOKENS {
            return Err(SendError::OutputTokens);
        }
        if self
            .temperature
            .is_some_and(|temperature| !(0.0..=1.0).contains(&temperature))
        {
            return Err(SendError::Temperature);
        }
        let body = serde_json::to_vec(&WireRequest {
            model: &self.model,
            system: self.system.as_deref(),
            messages: &self.messages,
            max_tokens: self.max_tokens,
            temperature: self.temperature,
            stream: false,
        })
        .map_err(|_| SendError::RequestTooLarge)?;
        if body.len() > MAX_REQUEST_BYTES {
            return Err(SendError::RequestTooLarge);
        }
        Ok(RequestBody(body))
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

/// Bytes a response cost, kept apart: the head, the body the transport delivered, and what the parser allocated for itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResponseAccounting {
    pub head_bytes: usize,
    pub transport_bytes: usize,
    pub parser_scratch_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct Sender {
    endpoint: Endpoint,
    credential: Credential,
}

impl Sender {
    /// The provider identity a disclosure marker records: the host this sender actually dials, the API surface, and the API version it speaks.
    pub fn provider_identity(&self) -> String {
        format!("{}{MESSAGES_PATH}@{ANTHROPIC_VERSION}", self.endpoint.host)
    }

    /// The identifier of the credential this sender writes into the authentication header.
    pub fn credential_id(&self) -> &str {
        self.credential.id()
    }

    pub fn new(endpoint: Endpoint, credential: Credential) -> Self {
        Self {
            endpoint,
            credential,
        }
    }

    /// Opens a fresh connection and completes the TCP, TLS, and HTTP/1 handshakes by `deadline`, clamped to [`MAX_PHASE_DURATION`]. No request byte is written; the connection is not polled again until [`InFlight::complete`].
    pub async fn connect(&self, deadline: Instant) -> Result<Connected, SendError> {
        let handshakes = async {
            let tcp = TcpStream::connect((self.endpoint.host.as_str(), self.endpoint.port))
                .await
                .map_err(|_| SendError::Connect)?;
            tcp.set_nodelay(true).map_err(|_| SendError::Connect)?;
            let tls = TlsConnector::from(self.endpoint.tls.clone())
                .connect(self.endpoint.server_name.clone(), tcp)
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
                host: self.endpoint.host.clone(),
                credential: self.credential.clone(),
            })
        };
        tokio::time::timeout_at(clamp(deadline), handshakes)
            .await
            .map_err(|_| SendError::Deadline)?
    }
}

fn clamp(deadline: Instant) -> Instant {
    deadline.min(Instant::now() + MAX_PHASE_DURATION)
}

/// A handshaken, unpolled connection that can carry exactly one request.
pub struct Connected {
    send: SendRequest<Body>,
    connection: Connection<Transport, Body>,
    host: String,
    credential: Credential,
}

impl std::fmt::Debug for Connected {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Connected({})", self.host)
    }
}

impl Connected {
    /// The one-shot handoff: synchronously moves the already serialized `body` into the connection's dispatch queue and returns the in-flight send. The caller serializes and bounds the body beforehand ([`MessagesRequest::body`]), so the bytes it hashed are the bytes sent and nothing is encoded here. Nothing is written to the peer until [`InFlight::complete`] polls the connection; a connection that turns out unable to take the request reports `NotReady` there, with nothing sent.
    pub fn handoff(mut self, body: RequestBody) -> Result<InFlight, SendError> {
        let wire = Request::post(MESSAGES_PATH)
            .header(header::HOST, self.host.as_str())
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json")
            .header(header::ACCEPT_ENCODING, "identity")
            .header(header::CONNECTION, "close")
            .header("x-api-key", self.credential.header())
            .header("anthropic-version", ANTHROPIC_VERSION)
            .body(Full::new(Bytes::from(body.0)))
            .map_err(|_| SendError::RequestTooLarge)?;
        // `try_send_request` moves the request into the dispatch queue before it returns its future; a fresh connection admits exactly one request before it is polled.
        let response = Box::pin(self.send.try_send_request(wire));
        Ok(InFlight {
            connection: self.connection,
            response,
        })
    }
}

type ResponseFuture = std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = Result<
                    Response<Incoming>,
                    hyper::client::conn::TrySendError<Request<Body>>,
                >,
            > + Send,
    >,
>;

/// A request the connection holds but has not yet written.
pub struct InFlight {
    connection: Connection<Transport, Body>,
    response: ResponseFuture,
}

impl std::fmt::Debug for InFlight {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("InFlight")
    }
}

impl InFlight {
    /// Polls the connection, so the request is written, and reads the one response by `deadline`, clamped to [`MAX_PHASE_DURATION`], with at most [`FRAME_IDLE_TIMEOUT`] between bytes. The connection is dropped afterwards whatever the outcome; there is no second attempt.
    pub async fn complete(self, deadline: Instant) -> Result<AssistantText, SendError> {
        let InFlight {
            mut connection,
            mut response,
        } = self;
        let mut accounting = ResponseAccounting::default();
        let exchange = async {
            // The connection may finish in the same poll that delivers the response; a finished connection is never polled again, and the response it already delivered is taken from the future.
            let mut connection_done = false;
            let unsent = |error: hyper::client::conn::TrySendError<Request<Body>>| {
                let mut error = error;
                if error.take_message().is_some() {
                    SendError::NotReady
                } else {
                    SendError::Transport
                }
            };
            let response = tokio::time::timeout(FRAME_IDLE_TIMEOUT, async {
                tokio::select! {
                    biased;
                    response = &mut response => response.map_err(unsent),
                    closed = &mut connection => {
                        closed.map_err(|_| SendError::Transport)?;
                        connection_done = true;
                        response.await.map_err(unsent)
                    }
                }
            })
            .await
            .map_err(|_| SendError::Deadline)??;
            let (head, body) = response.into_parts();
            // Hyper refuses a head it cannot buffer; a head it could buffer is still held to the declared bound before anything else is read.
            accounting.head_bytes = head
                .headers
                .iter()
                .map(|(name, value)| name.as_str().len() + value.len())
                .sum();
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
            if let Some(length) = head
                .headers
                .get(header::CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<usize>().ok())
                && length > MAX_RAW_RESPONSE_BYTES
            {
                return Err(SendError::ResponseTooLarge);
            }
            let bytes =
                collect_body(body, &mut connection, connection_done, &mut accounting).await?;
            if head.status != StatusCode::OK {
                return Err(SendError::Status(head.status.as_u16()));
            }
            let json = head
                .headers
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| {
                    value
                        .trim_start()
                        .to_ascii_lowercase()
                        .starts_with("application/json")
                });
            if !json {
                return Err(SendError::ContentType);
            }
            let decoded = decode_message(&bytes).map_err(SendError::Decode)?;
            accounting.parser_scratch_bytes = decoded.scratch_bytes;
            check_render(decoded.text.as_bytes(), None).map_err(|_| SendError::EgressCheck)?;
            Ok(AssistantText {
                text: decoded.text,
                stop_reason: decoded.stop_reason,
                model: decoded.model,
                accounting,
            })
        };
        tokio::time::timeout_at(clamp(deadline), exchange)
            .await
            .map_err(|_| SendError::Deadline)?
    }
}

/// Reads every body frame while keeping the connection polled, charging each chunk against [`MAX_RAW_RESPONSE_BYTES`] before it is kept. The bytes of a body that overflows are not retained. Once the connection has finished, cleanly or not, it is never polled again; frames it already delivered are still drained, and a body cut short reports itself through its own frame error.
async fn collect_body(
    mut body: Incoming,
    connection: &mut Connection<Transport, Body>,
    mut connection_done: bool,
    accounting: &mut ResponseAccounting,
) -> Result<Vec<u8>, SendError> {
    let mut bytes = Vec::new();
    let mut frames = 0usize;
    loop {
        let next = tokio::time::timeout(FRAME_IDLE_TIMEOUT, async {
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
        if frames > MAX_RESPONSE_FRAMES {
            return Err(SendError::ResponseTooLarge);
        }
        if let Some(chunk) = frame.data_ref() {
            let charged = accounting.transport_bytes.saturating_add(chunk.len());
            if charged > MAX_RAW_RESPONSE_BYTES {
                accounting.transport_bytes = MAX_RAW_RESPONSE_BYTES;
                return Err(SendError::ResponseTooLarge);
            }
            accounting.transport_bytes = charged;
            bytes.extend_from_slice(chunk);
        }
    }
}
