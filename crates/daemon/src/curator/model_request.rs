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
    /// The credential is not a valid header value.
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

/// A startup credential. It is rendered only into the authentication header, `Debug` never shows it, and its bytes are wiped when the last copy drops.
#[derive(Clone)]
pub struct Credential(Zeroizing<String>);

impl Credential {
    /// Refuses a secret that cannot be a header value, so the refusal is named at startup rather than at the first send.
    pub fn new(secret: String) -> Result<Self, SendError> {
        let secret = Zeroizing::new(secret);
        HeaderValue::from_str(&secret).map_err(|_| SendError::Credential)?;
        Ok(Self(secret))
    }

    fn header(&self) -> HeaderValue {
        let mut value = HeaderValue::from_str(&self.0)
            .expect("Credential::new admits only a valid header value");
        value.set_sensitive(true);
        value
    }
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Credential(<redacted>)")
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

/// One Messages request. It is serialized with `stream: false` and no `tools`; nothing else about the wire shape is configurable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessagesRequest {
    pub model: String,
    pub system: Option<String>,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
}

#[derive(Serialize)]
struct WireRequest<'a> {
    model: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<&'a str>,
    messages: &'a [Message],
    max_tokens: u32,
    stream: bool,
}

impl MessagesRequest {
    /// The serialized body, refused when it asks for more output than [`MAX_OUTPUT_TOKENS`] or exceeds [`MAX_REQUEST_BYTES`].
    pub fn body(&self) -> Result<Vec<u8>, SendError> {
        if self.max_tokens == 0 || self.max_tokens > MAX_OUTPUT_TOKENS {
            return Err(SendError::OutputTokens);
        }
        let body = serde_json::to_vec(&WireRequest {
            model: &self.model,
            system: self.system.as_deref(),
            messages: &self.messages,
            max_tokens: self.max_tokens,
            stream: false,
        })
        .map_err(|_| SendError::RequestTooLarge)?;
        if body.len() > MAX_REQUEST_BYTES {
            return Err(SendError::RequestTooLarge);
        }
        Ok(body)
    }
}

/// The one validated assistant message a completed send yields, with what receiving it cost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistantText {
    pub text: String,
    pub stop_reason: Option<StopReason>,
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
    timing: Timing,
}

impl Sender {
    pub fn new(endpoint: Endpoint, credential: Credential) -> Self {
        Self {
            endpoint,
            credential,
            timing: Timing::PRODUCTION,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn with_timing(endpoint: Endpoint, credential: Credential, timing: Timing) -> Self {
        Self {
            endpoint,
            credential,
            timing,
        }
    }

    /// Opens a fresh connection and completes the TCP, TLS, and HTTP/1 handshakes by `deadline`, clamped to [`Timing::connect`]. No request byte is written.
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
    host: String,
    credential: Credential,
    timing: Timing,
}

impl std::fmt::Debug for Connected {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Connected({})", self.host)
    }
}

impl Connected {
    /// The one-shot handoff: synchronously moves the owned request into the connection's dispatch queue and returns the in-flight send. Nothing is written to the peer until [`InFlight::complete`] polls the connection; a connection that turns out unable to take the request reports `NotReady` there, with nothing sent.
    pub fn handoff(mut self, request: &MessagesRequest) -> Result<InFlight, SendError> {
        let body = request.body()?;
        let wire = Request::post(MESSAGES_PATH)
            .header(header::HOST, self.host.as_str())
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json")
            .header(header::ACCEPT_ENCODING, "identity")
            .header(header::CONNECTION, "close")
            .header("x-api-key", self.credential.header())
            .header("anthropic-version", ANTHROPIC_VERSION)
            .body(Full::new(Bytes::from(body)))
            .map_err(|_| SendError::RequestTooLarge)?;
        // `try_send_request` moves the request into the dispatch queue before it returns its future; a fresh connection admits exactly one request before it is polled.
        let response = Box::pin(self.send.try_send_request(wire));
        Ok(InFlight {
            connection: self.connection,
            response,
            timing: self.timing,
            max_tokens: request.max_tokens,
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
    timing: Timing,
    max_tokens: u32,
}

impl std::fmt::Debug for InFlight {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("InFlight")
    }
}

impl InFlight {
    /// Polls the connection, so the request is written, and reads the one response by `deadline`, clamped to [`Timing::completion_budget`] for the request's `max_tokens`, with at most [`Timing::frame_idle`] between body frames. The connection is dropped afterwards whatever the outcome; there is no second attempt.
    pub async fn complete(self, deadline: Instant) -> Result<AssistantText, SendError> {
        let InFlight {
            mut connection,
            mut response,
            timing,
            max_tokens,
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
            let bytes = collect_body(
                body,
                &mut connection,
                connection_done,
                timing.frame_idle,
                &mut accounting,
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
            let decoded = decode_message(&bytes).map_err(SendError::Decode)?;
            accounting.parser_scratch_bytes = decoded.scratch_bytes;
            check_render(decoded.text.as_bytes(), None).map_err(|_| SendError::EgressCheck)?;
            Ok(AssistantText {
                text: decoded.text,
                stop_reason: decoded.stop_reason,
                accounting,
            })
        };
        tokio::time::timeout_at(
            clamp(deadline, timing.completion_budget(max_tokens)),
            exchange,
        )
        .await
        .map_err(|_| SendError::Deadline)?
    }
}

/// Reads every body frame while keeping the connection polled, charging each chunk against [`MAX_RAW_RESPONSE_BYTES`] before it is kept. The bytes of a body that overflows are not retained. Once the connection has finished, cleanly or not, it is never polled again; frames it already delivered are still drained, and a body cut short reports itself through its own frame error.
async fn collect_body(
    mut body: Incoming,
    connection: &mut Connection<Transport, Body>,
    mut connection_done: bool,
    frame_idle: Duration,
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
