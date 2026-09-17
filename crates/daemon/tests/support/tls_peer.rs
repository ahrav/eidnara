//! A scripted local TLS peer for the Curator sender proofs: a test-only authority and leaf for `localhost`, an encrypted-byte counter on the peer's socket, and one connection per scripted exchange.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use daemon::curator::model_request::{Credential, Endpoint, Sender};
use rustls::pki_types::PrivateKeyDer;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;

pub const OBSERVATION_WINDOW: Duration = Duration::from_millis(300);

/// The peer's socket, counting every encrypted byte the client sends so the proof compares exact counters rather than only waiting.
pub struct Counting {
    inner: TcpStream,
    received: Arc<AtomicUsize>,
}

impl AsyncRead for Counting {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let polled = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &polled {
            self.received
                .fetch_add(buf.filled().len() - before, Ordering::SeqCst);
        }
        polled
    }
}

impl AsyncWrite for Counting {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

pub type PeerStream = tokio_rustls::server::TlsStream<Counting>;
pub type BeforeRead = Box<
    dyn FnOnce(
            &mut PeerStream,
            Arc<AtomicUsize>,
        ) -> Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>>
        + Send,
>;

/// A scripted TLS peer that speaks for `localhost` under a test-only authority.
pub struct Peer {
    pub port: u16,
    pub roots: rustls::RootCertStore,
    pub connections: Arc<AtomicUsize>,
    listener: Option<TcpListener>,
    acceptor: TlsAcceptor,
}

/// What the peer saw; every judgement is made by the test, not inside the peer task.
#[derive(Debug, Default)]
pub struct Observed {
    pub head: String,
    pub body: Vec<u8>,
    /// Whether a second connection arrived within the observation window after the exchange.
    pub reconnected: bool,
    /// Encrypted bytes received once the handshake had completed and again after the client's handoff, when the script measured them.
    pub after_handshake: usize,
    pub after_handoff: Option<usize>,
}

impl Peer {
    pub async fn start() -> Self {
        let mut ca_params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let ca =
            rcgen::CertifiedIssuer::self_signed(ca_params, rcgen::KeyPair::generate().unwrap())
                .unwrap();
        let leaf_key = rcgen::KeyPair::generate().unwrap();
        let leaf = rcgen::CertificateParams::new(vec!["localhost".to_string()])
            .unwrap()
            .signed_by(&leaf_key, &ca)
            .unwrap();
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(AsRef::<rcgen::Certificate>::as_ref(&ca).der().clone())
            .unwrap();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut config = rustls::ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![leaf.der().clone()],
                PrivateKeyDer::try_from(leaf_key.serialize_der()).unwrap(),
            )
            .unwrap();
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        Self {
            port: listener.local_addr().unwrap().port(),
            roots,
            connections: Arc::new(AtomicUsize::new(0)),
            listener: Some(listener),
            acceptor: TlsAcceptor::from(Arc::new(config)),
        }
    }

    pub fn sender(&self) -> Sender {
        self.sender_with_credential("sk-test-credential")
    }

    pub fn sender_with_credential(&self, credential: &str) -> Sender {
        Sender::new(
            Endpoint::for_test("localhost", self.port, self.roots.clone()).unwrap(),
            Credential::new(credential.to_string()).unwrap(),
        )
    }

    /// Accepts one TLS connection, runs `before_read` after the handshake, reads one full request if the client sends one, answers with `respond`, and reports what it observed. The peer never asserts; a client that goes away early yields a partial report.
    pub fn serve(
        &mut self,
        before_read: BeforeRead,
        respond: impl FnOnce(&Observed) -> Vec<u8> + Send + 'static,
    ) -> tokio::task::JoinHandle<Observed> {
        let listener = self.listener.take().unwrap();
        let acceptor = self.acceptor.clone();
        let connections = self.connections.clone();
        tokio::spawn(async move {
            let mut observed = Observed::default();
            let (tcp, _) = listener.accept().await.unwrap();
            connections.fetch_add(1, Ordering::SeqCst);
            let received = Arc::new(AtomicUsize::new(0));
            let counting = Counting {
                inner: tcp,
                received: received.clone(),
            };
            let Ok(mut tls) = acceptor.accept(counting).await else {
                observed.reconnected = tokio::time::timeout(OBSERVATION_WINDOW, listener.accept())
                    .await
                    .is_ok();
                return observed;
            };
            observed.after_handshake = received.load(Ordering::SeqCst);
            before_read(&mut tls, received.clone()).await;
            let mut raw = Vec::new();
            let mut buffer = [0u8; 16 * 1024];
            loop {
                let Ok(read) = tls.read(&mut buffer).await else {
                    return observed;
                };
                if read == 0 {
                    return observed;
                }
                raw.extend_from_slice(&buffer[..read]);
                if let Some(split) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
                    observed.head = String::from_utf8(raw[..split].to_vec()).unwrap();
                    let length: usize = observed
                        .head
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .unwrap()
                        .parse()
                        .unwrap();
                    while raw.len() < split + 4 + length {
                        let Ok(read) = tls.read(&mut buffer).await else {
                            return observed;
                        };
                        if read == 0 {
                            return observed;
                        }
                        raw.extend_from_slice(&buffer[..read]);
                    }
                    observed.body = raw[split + 4..split + 4 + length].to_vec();
                    break;
                }
            }
            observed.after_handoff = observed
                .after_handoff
                .or(Some(received.load(Ordering::SeqCst)));
            // The client may refuse and hang up mid-write; that is its right and not the peer's failure.
            let _ = tls.write_all(&respond(&observed)).await;
            let _ = tls.shutdown().await;
            observed.reconnected = tokio::time::timeout(OBSERVATION_WINDOW, listener.accept())
                .await
                .is_ok();
            observed
        })
    }
}

impl Peer {
    /// Serves one scripted response per accepted connection, in order, and reports what each connection carried. The task ends after the last response; a client that connects fewer times leaves the remaining script unserved and the report short.
    pub fn serve_script(
        &mut self,
        responses: Vec<Vec<u8>>,
    ) -> tokio::task::JoinHandle<Vec<Observed>> {
        self.serve_script_with(responses, |_| Box::pin(async {}))
    }

    /// [`Self::serve_script`] with `before_respond` run after each request is read and before its response is written, given the connection's index.
    pub fn serve_script_with(
        &mut self,
        responses: Vec<Vec<u8>>,
        mut before_respond: impl FnMut(usize) -> Pin<Box<dyn Future<Output = ()> + Send>>
        + Send
        + 'static,
    ) -> tokio::task::JoinHandle<Vec<Observed>> {
        let listener = self.listener.take().unwrap();
        let acceptor = self.acceptor.clone();
        let connections = self.connections.clone();
        tokio::spawn(async move {
            let mut observations = Vec::with_capacity(responses.len());
            for (index, response) in responses.into_iter().enumerate() {
                let Ok(Ok((tcp, _))) =
                    tokio::time::timeout(Duration::from_secs(5), listener.accept()).await
                else {
                    return observations;
                };
                connections.fetch_add(1, Ordering::SeqCst);
                let counting = Counting {
                    inner: tcp,
                    received: Arc::new(AtomicUsize::new(0)),
                };
                let Ok(mut tls) = acceptor.accept(counting).await else {
                    return observations;
                };
                let mut observed = Observed::default();
                let mut raw = Vec::new();
                let mut buffer = [0u8; 16 * 1024];
                loop {
                    let Ok(read) = tls.read(&mut buffer).await else {
                        return observations;
                    };
                    if read == 0 {
                        return observations;
                    }
                    raw.extend_from_slice(&buffer[..read]);
                    if let Some(split) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
                        observed.head = String::from_utf8(raw[..split].to_vec()).unwrap();
                        let length: usize = observed
                            .head
                            .to_ascii_lowercase()
                            .lines()
                            .find_map(|line| {
                                line.strip_prefix("content-length: ").map(str::to_string)
                            })
                            .unwrap()
                            .parse()
                            .unwrap();
                        while raw.len() < split + 4 + length {
                            let Ok(read) = tls.read(&mut buffer).await else {
                                return observations;
                            };
                            if read == 0 {
                                return observations;
                            }
                            raw.extend_from_slice(&buffer[..read]);
                        }
                        observed.body = raw[split + 4..split + 4 + length].to_vec();
                        break;
                    }
                }
                before_respond(index).await;
                let _ = tls.write_all(&response).await;
                let _ = tls.shutdown().await;
                observations.push(observed);
            }
            observations
        })
    }
}

/// A Messages response whose one text block is `text`, JSON-escaped.
pub fn text_response(text: &str) -> Vec<u8> {
    let escaped = serde_json::to_string(text).unwrap();
    json_response(
        "200 OK",
        &format!(
            r#"{{"id":"msg_1","type":"message","role":"assistant","model":"claude-canonical-1","content":[{{"type":"text","text":{escaped}}}],"stop_reason":"end_turn","usage":{{"input_tokens":3,"output_tokens":4}}}}"#
        ),
        "",
    )
}

pub fn no_wait() -> BeforeRead {
    Box::new(|_, _| Box::pin(async {}))
}

pub fn json_response(status: &str, body: &str, extra_headers: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n{extra_headers}\r\n{body}",
        body.len()
    )
    .into_bytes()
}

pub fn chunked_response(status: &str, chunks: impl Iterator<Item = Vec<u8>>) -> Vec<u8> {
    let mut response = format!("HTTP/1.1 {status}\r\ncontent-type: application/json\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n").into_bytes();
    for chunk in chunks {
        response.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
        response.extend_from_slice(&chunk);
        response.extend_from_slice(b"\r\n");
    }
    response.extend_from_slice(b"0\r\n\r\n");
    response
}

pub fn message(text: &str) -> String {
    format!(
        r#"{{"id":"msg_1","type":"message","role":"assistant","model":"claude","content":[{{"type":"text","text":"{text}"}}],"stop_reason":"end_turn","usage":{{"input_tokens":3,"output_tokens":4}}}}"#
    )
}
