//! The sender proof: through a real local TLS peer, the one-shot handoff writes nothing until completion, the connection is one-use with no retry, the transport verifies chain and hostname, and responses are bounded, decoded, and egress-checked with host-authored refusals.

mod support;

use std::sync::atomic::Ordering;
use std::time::Duration;

use daemon::curator::model_request::{
    ANTHROPIC_VERSION, AssistantText, Credential, Endpoint, MAX_OUTPUT_TOKENS,
    MAX_RAW_RESPONSE_BYTES, MAX_REQUEST_BYTES, MAX_RESPONSE_FRAMES, MAX_RESPONSE_HEAD_BYTES,
    MAX_RESPONSE_HEADERS, Message, MessagesRequest, Role, SendError, Sender,
};
use daemon::curator::model_response::{DecodeError, StopReason};
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use tokio::time::Instant;

use support::tls_peer::{
    OBSERVATION_WINDOW, Observed, Peer, chunked_response, json_response, message, no_wait,
};

fn request() -> MessagesRequest {
    MessagesRequest {
        model: "claude".to_string(),
        system: Some("Answer plainly.".to_string()),
        messages: vec![Message {
            role: Role::User,
            content: "What builds the workspace?".to_string(),
        }],
        max_tokens: 64,
        temperature: None,
    }
}

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(10)
}

/// One complete send against a peer scripted to answer `response`.
async fn exchange_with(
    peer: &mut Peer,
    response: Vec<u8>,
) -> (Result<AssistantText, SendError>, Observed) {
    let server = peer.serve(no_wait(), move |_| response);
    let outcome = peer
        .sender()
        .connect(deadline())
        .await
        .unwrap()
        .handoff(request().body().unwrap())
        .unwrap()
        .complete(deadline())
        .await;
    let observed = server.await.unwrap();
    assert!(
        !observed.reconnected,
        "the client opened a second connection"
    );
    (outcome, observed)
}

async fn refused_with(response: Vec<u8>) -> SendError {
    let mut peer = Peer::start().await;
    let (outcome, _) = exchange_with(&mut peer, response).await;
    outcome.unwrap_err()
}

#[tokio::test]
async fn the_handoff_writes_nothing_until_completion_and_the_connection_is_one_use() {
    let mut peer = Peer::start().await;
    let (armed_tx, armed_rx) = tokio::sync::oneshot::channel::<()>();
    let server = peer.serve(
        Box::new(move |tls, received| {
            Box::pin(async move {
                // The client has handed its request over by the time this fires. The peer polls its socket for the observation window and then compares exact byte counts: encrypted request bytes would have raised the counter.
                armed_rx.await.unwrap();
                let mut probe = [0u8; 1];
                let _ = tokio::time::timeout(OBSERVATION_WINDOW, tls.read(&mut probe)).await;
                let _ = received.load(Ordering::SeqCst);
            })
        }),
        |_| json_response("200 OK", &message("bun builds it"), ""),
    );
    let sender = peer.sender();
    let connected = sender.connect(deadline()).await.unwrap();
    let in_flight = connected.handoff(request().body().unwrap()).unwrap();
    armed_tx.send(()).unwrap();
    // Give the peer its whole observation window before the connection is polled.
    tokio::time::sleep(OBSERVATION_WINDOW + Duration::from_millis(100)).await;
    let answer = in_flight.complete(deadline()).await.unwrap();
    let observed = server.await.unwrap();
    assert_eq!(answer.text, "bun builds it");
    assert_eq!(answer.stop_reason, Some(StopReason::EndTurn));
    assert_eq!(
        answer.accounting.transport_bytes,
        message("bun builds it").len()
    );
    assert!(answer.accounting.parser_scratch_bytes < answer.accounting.transport_bytes);
    assert!(answer.accounting.head_bytes > 0);
    assert!(
        observed.after_handoff.unwrap() > observed.after_handshake,
        "the request arrived only once the connection was polled"
    );
    assert!(!observed.reconnected);
    assert_eq!(peer.connections.load(Ordering::SeqCst), 1);
    // The wire request: fixed path and version, identity encoding, one-use connection, no tools, no streaming, and the credential exactly once, in its header.
    assert!(observed.head.starts_with("POST /v1/messages HTTP/1.1\r\n"));
    let headers = observed.head.to_ascii_lowercase();
    assert!(headers.contains("host: localhost\r\n"));
    assert_eq!(
        headers.matches("x-api-key: sk-test-credential\r\n").count(),
        1
    );
    assert_eq!(headers.matches("sk-test-credential").count(), 1);
    assert!(headers.contains(&format!("anthropic-version: {ANTHROPIC_VERSION}\r\n")));
    assert!(headers.contains("accept-encoding: identity\r\n"));
    assert!(headers.contains("connection: close\r\n"));
    assert!(headers.contains("content-type: application/json\r\n"));
    let body: serde_json::Value = serde_json::from_slice(&observed.body).unwrap();
    assert_eq!(body["stream"], serde_json::Value::Bool(false));
    assert!(body.get("tools").is_none());
    assert_eq!(body["max_tokens"], 64);
    assert!(
        body.get("temperature").is_none(),
        "an omitted sampling value is serialized as an omission"
    );
    assert_eq!(body["system"], "Answer plainly.");
    assert_eq!(answer.model.as_deref(), Some("claude"));
    assert!(!String::from_utf8_lossy(&observed.body).contains("sk-test-credential"));
}

#[tokio::test]
async fn no_request_byte_reaches_the_peer_before_the_connection_is_polled() {
    // The exact-counter form of the proof: the peer records the encrypted byte count right after the handshake and again after the client's handoff has completed and it has polled its socket for the whole window.
    let mut peer = Peer::start().await;
    let (armed_tx, armed_rx) = tokio::sync::oneshot::channel::<()>();
    let (count_tx, count_rx) = tokio::sync::oneshot::channel::<(usize, usize)>();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let server = peer.serve(
        Box::new(move |tls, received| {
            Box::pin(async move {
                let after_handshake = received.load(Ordering::SeqCst);
                armed_rx.await.unwrap();
                let mut probe = [0u8; 1];
                let _ = tokio::time::timeout(OBSERVATION_WINDOW, tls.read(&mut probe)).await;
                count_tx
                    .send((after_handshake, received.load(Ordering::SeqCst)))
                    .unwrap();
                release_rx.await.unwrap();
            })
        }),
        |_| json_response("200 OK", &message("later"), ""),
    );
    let in_flight = peer
        .sender()
        .connect(deadline())
        .await
        .unwrap()
        .handoff(request().body().unwrap())
        .unwrap();
    armed_tx.send(()).unwrap();
    let (after_handshake, after_handoff) = count_rx.await.unwrap();
    assert_eq!(
        after_handshake, after_handoff,
        "encrypted bytes reached the peer between the handoff and the first poll"
    );
    release_tx.send(()).unwrap();
    let answer = in_flight.complete(deadline()).await.unwrap();
    assert_eq!(answer.text, "later");
    let observed = server.await.unwrap();
    assert!(observed.after_handoff.unwrap() > after_handoff);
}

#[tokio::test]
async fn the_transport_verifies_the_chain_and_the_hostname_without_retrying() {
    // An authority the peer's certificate does not chain to.
    let mut peer = Peer::start().await;
    let server = peer.serve(no_wait(), |_| Vec::new());
    let untrusted = Sender::new(
        Endpoint::for_test("localhost", peer.port, rustls::RootCertStore::empty()).unwrap(),
        Credential::new("k".to_string()).unwrap(),
    );
    assert_eq!(
        untrusted.connect(deadline()).await.unwrap_err(),
        SendError::Tls
    );
    assert!(!server.await.unwrap().reconnected);
    assert_eq!(peer.connections.load(Ordering::SeqCst), 1);
    // A trusted authority but a name the certificate does not carry.
    let mut peer = Peer::start().await;
    let server = peer.serve(no_wait(), |_| Vec::new());
    let wrong_name = Sender::new(
        Endpoint::for_test("127.0.0.1", peer.port, peer.roots.clone()).unwrap(),
        Credential::new("k".to_string()).unwrap(),
    );
    assert_eq!(
        wrong_name.connect(deadline()).await.unwrap_err(),
        SendError::Tls
    );
    assert!(!server.await.unwrap().reconnected);
    assert_eq!(peer.connections.load(Ordering::SeqCst), 1);
    // Nothing listening: a connect failure, not a retry loop.
    let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    let nobody = Sender::new(
        Endpoint::for_test("localhost", port, peer.roots.clone()).unwrap(),
        Credential::new("k".to_string()).unwrap(),
    );
    assert_eq!(
        nobody.connect(deadline()).await.unwrap_err(),
        SendError::Connect
    );
    // A credential that cannot be a header value is refused at startup.
    assert_eq!(
        Credential::new("line\nbreak".to_string()).unwrap_err(),
        SendError::Credential
    );
    assert_eq!(
        format!("{:?}", Credential::new("sk-secret".to_string()).unwrap()),
        "Credential(<redacted>)"
    );
}

#[tokio::test]
async fn compressed_non_json_and_error_responses_are_refused() {
    assert_eq!(
        refused_with(json_response(
            "200 OK",
            &message("x"),
            "content-encoding: gzip\r\n"
        ))
        .await,
        SendError::Compressed
    );
    assert_eq!(
        refused_with(json_response(
            "200 OK",
            &message("x"),
            "content-encoding: identity\r\ncontent-encoding: br\r\n"
        ))
        .await,
        SendError::Compressed
    );
    let html =
        "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello"
            .to_string();
    assert_eq!(
        refused_with(html.into_bytes()).await,
        SendError::ContentType
    );
    let error_body =
        r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#;
    let mut peer = Peer::start().await;
    let (outcome, _) = exchange_with(
        &mut peer,
        json_response("429 Too Many Requests", error_body, ""),
    )
    .await;
    assert_eq!(outcome.unwrap_err(), SendError::Status(429));
    // An error body is charged like any other, and one past the allowance is too large before its status is judged.
    let piece = vec![b'e'; 64 * 1024];
    let oversized = chunked_response(
        "429 Too Many Requests",
        std::iter::repeat_n(piece, MAX_RAW_RESPONSE_BYTES / (64 * 1024) + 1),
    );
    assert_eq!(refused_with(oversized).await, SendError::ResponseTooLarge);
}

#[tokio::test]
async fn decoding_and_egress_refusals_carry_host_codes() {
    assert_eq!(
        refused_with(json_response(
            "200 OK",
            r#"{"id":"m","type":"message","role":"assistant","content":[{"type":"tool_use","id":"t","name":"run","input":{}}]}"#,
            ""
        ))
        .await,
        SendError::Decode(DecodeError::ToolUse)
    );
    assert_eq!(
        refused_with(json_response(
            "200 OK",
            &message("key AKIAQ7RSTUVWXYZ23456 leaked"),
            ""
        ))
        .await,
        SendError::EgressCheck
    );
    // An undecodable body that also looks like a leak is a decode refusal; the egress check never sees it.
    assert_eq!(
        refused_with(json_response("200 OK", "AKIAQ7RSTUVWXYZ23456", "")).await,
        SendError::Decode(DecodeError::Syntax)
    );
}

#[tokio::test]
async fn response_size_bounds_hold_for_declared_chunked_trickled_and_wide_heads() {
    // A declared length past the allowance is refused before any body byte is read.
    let declared = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        MAX_RAW_RESPONSE_BYTES + 1
    );
    assert_eq!(
        refused_with(declared.into_bytes()).await,
        SendError::ResponseTooLarge
    );
    // A chunked body that overflows is refused when the chunk that crosses the allowance arrives, and the charge stops at the allowance.
    let piece = vec![b'a'; 64 * 1024];
    let mut peer = Peer::start().await;
    let (outcome, _) = exchange_with(
        &mut peer,
        chunked_response(
            "200 OK",
            std::iter::repeat_n(piece, MAX_RAW_RESPONSE_BYTES / (64 * 1024) + 1),
        ),
    )
    .await;
    assert_eq!(outcome.unwrap_err(), SendError::ResponseTooLarge);
    // A body trickled in more frames than the allowance admits is refused even when its bytes would fit.
    assert_eq!(
        refused_with(chunked_response(
            "200 OK",
            std::iter::repeat_n(b"a".to_vec(), MAX_RESPONSE_FRAMES + 1)
        ))
        .await,
        SendError::ResponseTooLarge
    );
    // A head past its bound, in one value or in many headers, never reaches decoding.
    let padded = json_response(
        "200 OK",
        &message("x"),
        &format!("x-padding: {}\r\n", "p".repeat(MAX_RESPONSE_HEAD_BYTES)),
    );
    assert!(matches!(
        refused_with(padded).await,
        SendError::ResponseTooLarge | SendError::Transport
    ));
    let many = (0..MAX_RESPONSE_HEADERS + 1)
        .map(|index| format!("x-h{index}: v\r\n"))
        .collect::<String>();
    assert_eq!(
        refused_with(json_response("200 OK", &message("x"), &many)).await,
        SendError::Transport
    );
}

#[tokio::test]
async fn request_bounds_and_deadlines_are_enforced_by_the_sender() {
    let mut oversized = request();
    oversized.max_tokens = MAX_OUTPUT_TOKENS + 1;
    assert_eq!(oversized.body().unwrap_err(), SendError::OutputTokens);
    let mut huge = request();
    huge.messages[0].content = "x".repeat(MAX_REQUEST_BYTES);
    assert_eq!(huge.body().unwrap_err(), SendError::RequestTooLarge);
    let mut hot = request();
    hot.temperature = Some(1.5);
    assert_eq!(hot.body().unwrap_err(), SendError::Temperature);
    hot.temperature = Some(f64::NAN);
    assert_eq!(hot.body().unwrap_err(), SendError::Temperature);
    // A connection dropped without a handoff sends nothing: the peer's byte count does not move after the handshake.
    let mut peer = Peer::start().await;
    let (count_tx, count_rx) = tokio::sync::oneshot::channel::<(usize, usize)>();
    let server = peer.serve(
        Box::new(move |tls, received| {
            Box::pin(async move {
                let after_handshake = received.load(Ordering::SeqCst);
                let mut probe = [0u8; 1];
                let _ = tokio::time::timeout(OBSERVATION_WINDOW, tls.read(&mut probe)).await;
                count_tx
                    .send((after_handshake, received.load(Ordering::SeqCst)))
                    .unwrap();
            })
        }),
        |_| Vec::new(),
    );
    let connected = peer.sender().connect(deadline()).await.unwrap();
    drop(connected);
    let (after_handshake, after_refusal) = count_rx.await.unwrap();
    assert_eq!(after_handshake, after_refusal);
    server.await.unwrap();
    // A peer that never answers: the completion deadline ends the send with no second attempt.
    let mut peer = Peer::start().await;
    let (hold_tx, hold_rx) = tokio::sync::oneshot::channel::<()>();
    let server = peer.serve(
        Box::new(move |_, _| {
            Box::pin(async move {
                hold_rx.await.ok();
            })
        }),
        |_| Vec::new(),
    );
    let in_flight = peer
        .sender()
        .connect(deadline())
        .await
        .unwrap()
        .handoff(request().body().unwrap())
        .unwrap();
    let outcome = in_flight
        .complete(Instant::now() + Duration::from_millis(500))
        .await;
    assert_eq!(outcome.unwrap_err(), SendError::Deadline);
    drop(hold_tx);
    let observed = server.await.unwrap();
    assert!(!observed.reconnected);
    assert_eq!(peer.connections.load(Ordering::SeqCst), 1);
}
