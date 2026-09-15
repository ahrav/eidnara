mod support;

use std::{path::PathBuf, time::Duration};

use host_runtime::{
    Client, HealthStatus, LivenessPolicy, RequestOptions, RouteIdentity, RouteTarget, SendOutcome,
    TargetKind,
};
use support::{LINKED_MODULE_ID, TestHost, mode_body};

fn target() -> RouteTarget {
    RouteTarget {
        module_id: LINKED_MODULE_ID.to_owned(),
        kind: TargetKind::ToolProvider,
    }
}

fn identity(session: &str) -> RouteIdentity {
    RouteIdentity {
        project_root: PathBuf::from("/tmp/eidnara-host-client-test"),
        harness: "client-test".to_owned(),
        session: session.to_owned(),
        consumer_module_id: None,
        consumer_launch_nonce: None,
        consumer_capabilities: Vec::new(),
        admission_facts: None,
        credential_fingerprints: std::collections::BTreeMap::new(),
    }
}

#[tokio::test]
async fn authenticates_attaches_ring_routes_unary_and_closes() {
    let host = TestHost::start().await;
    let publication: serde_json::Value =
        serde_json::from_slice(&std::fs::read(host.publication_path()).unwrap()).unwrap();
    assert!(publication.get("setup_socket").is_some());
    assert!(publication.get("endpoints").is_none());

    let client = Client::connect(host.publication_path())
        .await
        .expect("managed client attaches the mandatory ring");
    assert_eq!(
        client.daemon_id().as_slice(),
        host.info.daemon_id.as_slice()
    );
    let route = client
        .open_route(target(), identity("happy"))
        .await
        .expect("route opens");
    let body = mode_body(serde_json::json!({"mode": "echo", "value": 7}));
    let response = client
        .request(route, body.clone(), RequestOptions::default())
        .await
        .expect("unary response");
    assert_eq!(response.body, body);

    client.close_route(route).await.expect("route closes");
    client.close().await.expect("client closes");
    host.shutdown_gracefully().await;
}

#[tokio::test]
async fn ring_stream_and_control_traffic_share_one_live_generation() {
    let host = TestHost::start_with(|config| {
        config.liveness = Some(LivenessPolicy {
            ping_interval: Duration::from_millis(20),
            pong_deadline: Duration::from_millis(80),
            invalidate_on_missed: true,
        });
    })
    .await;
    let client = Client::connect(host.publication_path()).await.unwrap();
    let route = client
        .open_route(target(), identity("stream-ping"))
        .await
        .unwrap();
    let mut stream = client
        .request_stream(
            route,
            mode_body(serde_json::json!({"mode": "stream_then_hang", "items": 2})),
            RequestOptions {
                timeout: Duration::from_secs(2),
                cancellation: None,
                binary: false,
            },
        )
        .await
        .unwrap();

    for expected in 0..2 {
        let item = stream.next().await.unwrap().expect("stream item");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&item.body).unwrap()["item"],
            expected
        );
    }
    tokio::time::sleep(Duration::from_millis(150)).await;
    let body = mode_body(serde_json::json!({"mode": "echo", "value": "unrelated"}));
    assert_eq!(
        client
            .request(route, body.clone(), RequestOptions::default())
            .await
            .expect("Ping/Pong and stream traffic do not block unary")
            .body,
        body
    );
    stream.cancel().expect("stream cancellation");
    client.close().await.unwrap();
    host.shutdown_gracefully().await;
}

#[tokio::test]
async fn ring_terminal_is_typed_redacted_and_generation_remains_usable() {
    let host = TestHost::start().await;
    let client = Client::connect(host.publication_path()).await.unwrap();
    let route = client
        .open_route(target(), identity("terminal"))
        .await
        .unwrap();
    let sentinel = "CANARY-TERMINAL-BODY-7f31";
    let error = client
        .request(
            route,
            mode_body(serde_json::json!({
                "mode": "error",
                "code": "stable_failure",
                "message": sentinel
            })),
            RequestOptions::default(),
        )
        .await
        .expect_err("host returns Error terminal");
    assert_eq!(error.outcome(), SendOutcome::Terminal);
    assert_eq!(error.code(), "host.stable_failure");
    assert!(!format!("{error:?} {error}").contains(sentinel));

    let body = mode_body(serde_json::json!({"mode": "echo", "after": "error"}));
    assert_eq!(
        client
            .request(route, body.clone(), RequestOptions::default())
            .await
            .expect("terminal is correlation-scoped")
            .body,
        body
    );
    client.close().await.unwrap();
    host.shutdown_gracefully().await;
}

/// Uses literal expected strings so the test does not reproduce the client's prefixing logic.
#[tokio::test]
async fn host_terminal_codes_are_prefixed_and_local_codes_are_not() {
    let host = TestHost::start().await;
    let client = Client::connect(host.publication_path()).await.unwrap();
    let route = client
        .open_route(target(), identity("terminal-prefix"))
        .await
        .unwrap();

    for (raw, expected) in [
        ("unknown_module", "host.unknown_module"),
        ("idempotency_conflict", "host.idempotency_conflict"),
    ] {
        let error = client
            .request(
                route,
                mode_body(serde_json::json!({"mode": "error", "code": raw})),
                RequestOptions::default(),
            )
            .await
            .expect_err("host returns Error terminal");
        assert_eq!(error.outcome(), SendOutcome::Terminal);
        assert_eq!(error.code(), expected);
    }

    client.close_route(route).await.expect("route closes");
    let error = client
        .request(route, b"after-close".to_vec(), RequestOptions::default())
        .await
        .expect_err("closed route is not live");
    assert_eq!(error.outcome(), SendOutcome::NotSent);
    assert_eq!(error.code(), "route_not_live");

    client.close().await.unwrap();
    host.shutdown_gracefully().await;
}

/// The echo handler copies `RequestCtx::binary` to the response.
#[tokio::test]
async fn request_binary_flag_reaches_the_host_in_both_states() {
    let host = TestHost::start().await;
    let client = Client::connect(host.publication_path()).await.unwrap();
    let route = client
        .open_route(target(), identity("binary-flag"))
        .await
        .unwrap();

    for binary in [true, false] {
        let body = mode_body(serde_json::json!({"mode": "echo", "binary": binary}));
        let response = client
            .request(
                route,
                body.clone(),
                RequestOptions {
                    binary,
                    ..Default::default()
                },
            )
            .await
            .expect("echo response");
        assert_eq!(response.body, body);
        assert_eq!(
            response.binary, binary,
            "host observed binary={binary} on the request frame"
        );
    }

    client.close().await.unwrap();
    host.shutdown_gracefully().await;
}

#[tokio::test]
async fn caller_cancellation_is_correlation_scoped() {
    let host = TestHost::start().await;
    let client = Client::connect(host.publication_path()).await.unwrap();
    let route = client
        .open_route(target(), identity("cancel"))
        .await
        .unwrap();
    let cancel = host_runtime::CancellationToken::new();
    let trigger = cancel.clone();
    let request = client.request(
        route,
        mode_body(serde_json::json!({"mode": "await_cancel"})),
        RequestOptions {
            timeout: Duration::from_secs(2),
            cancellation: Some(cancel),
            binary: false,
        },
    );
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        trigger.cancel();
    });
    let error = request.await.expect_err("caller cancellation wins");
    assert!(matches!(
        error.outcome(),
        SendOutcome::NotSent | SendOutcome::OutcomeUnknown
    ));

    let body = mode_body(serde_json::json!({"mode": "echo", "after": "cancel"}));
    let response = client
        .request(route, body.clone(), RequestOptions::default())
        .await
        .expect("later request remains independent");
    assert_eq!(response.body, body);

    client.close().await.unwrap();
    host.shutdown_gracefully().await;
}

#[tokio::test]
async fn request_deadline_is_one_absolute_owner_and_honors_overrides() {
    let host = TestHost::start().await;
    let client = Client::connect(host.publication_path()).await.unwrap();
    let route = client
        .open_route(target(), identity("deadline"))
        .await
        .unwrap();

    let error = client
        .request(
            route,
            mode_body(serde_json::json!({"mode": "slow", "ms": 100})),
            RequestOptions {
                timeout: Duration::from_millis(20),
                cancellation: None,
                binary: false,
            },
        )
        .await
        .expect_err("short caller deadline wins");
    assert_eq!(error.outcome(), SendOutcome::OutcomeUnknown);
    assert_eq!(error.code(), "deadline_expired");

    let response = client
        .request(
            route,
            mode_body(serde_json::json!({"mode": "slow", "ms": 20})),
            RequestOptions {
                timeout: Duration::from_millis(200),
                cancellation: None,
                binary: false,
            },
        )
        .await
        .expect("longer caller deadline is honored");
    assert_eq!(response.body, b"slow-done");

    client.close().await.unwrap();
    host.shutdown_gracefully().await;
}

#[tokio::test]
async fn host_status_decodes_the_hosts_own_response_shape() {
    let host = TestHost::start_with(|config| {
        config.timing.health_interval = Duration::from_millis(20);
    })
    .await;
    let client = Client::connect(host.publication_path()).await.unwrap();

    let snapshot = client.host_status().await.expect("host.status decodes");
    assert_eq!(snapshot.health, HealthStatus::Ok);
    assert!(
        snapshot.metrics.get("components").is_some(),
        "the host wraps component metrics under `components`: {:?}",
        snapshot.metrics
    );
    assert!(snapshot.shared_memory.is_object());

    // `host.status` serves the last completed probe, so the change lands on
    // the next `health_interval` tick rather than synchronously.
    host.handler
        .set_health(HealthStatus::Degraded, Some("client-test"));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = client.host_status().await.expect("host.status decodes");
        if snapshot.health == HealthStatus::Degraded {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "health snapshot never reflected the degraded report"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    client.close().await.expect("client closes");
    host.shutdown_gracefully().await;
}

#[tokio::test]
async fn close_rejects_new_sends() {
    let host = TestHost::start().await;
    let client = Client::connect(host.publication_path()).await.unwrap();
    let route = client
        .open_route(target(), identity("close"))
        .await
        .unwrap();
    client.close().await.unwrap();
    let error = client
        .request(route, b"after-close".to_vec(), RequestOptions::default())
        .await
        .expect_err("closed client rejects sends");
    assert_eq!(error.outcome(), SendOutcome::NotSent);
    assert_eq!(error.code(), "client_closed");
    host.shutdown_gracefully().await;
}

/// Rust-client real-process witness for the payload-pool layout: a managed client attaches
/// the sole profile, completes a daemon request in each direction at the maximum body,
/// refuses one byte over it locally, and records the host's artifact identity and lifecycle
/// counters through `host.status` before and after a controlled close and reconnect.
#[tokio::test]
async fn managed_client_witnesses_current_layout_maximum_bodies_and_controlled_recovery() {
    const MAX_BODY: usize = 64 * 1024 * 1024;
    let host = TestHost::start_with(|config| {
        config.limits.max_resident_bytes = host_runtime::config::MIN_RESIDENT_BYTES + 64 * 1024;
        config.timing.health_interval = Duration::from_millis(20);
    })
    .await;
    let publication: serde_json::Value =
        serde_json::from_slice(&std::fs::read(host.publication_path()).unwrap()).unwrap();
    assert_eq!(
        publication["daemon_ver"], host.info.daemon_ver,
        "the publication names the build the client attached to"
    );

    let client = Client::connect(host.publication_path())
        .await
        .expect("managed client attaches the sole profile");
    let route = client
        .open_route(target(), identity("witness"))
        .await
        .expect("route opens");

    // Both directions at the maximum: a 64 MiB echo request produces a 64 MiB response.
    let prefix = br#"{"mode":"echo","pad":""#;
    let suffix = br#""}"#;
    let mut body = Vec::with_capacity(MAX_BODY);
    body.extend_from_slice(prefix);
    body.extend(std::iter::repeat_n(
        b'a',
        MAX_BODY - prefix.len() - suffix.len(),
    ));
    body.extend_from_slice(suffix);
    assert_eq!(body.len(), MAX_BODY);
    let response = client
        .request(
            route,
            body.clone(),
            RequestOptions {
                timeout: Duration::from_secs(120),
                ..RequestOptions::default()
            },
        )
        .await
        .expect("maximum-size request and response complete");
    assert_eq!(response.body.len(), MAX_BODY);
    assert!(
        response.body == body,
        "the echoed maximum body is byte-identical"
    );
    drop(response);

    // One byte over the maximum is refused locally before any frame exists.
    let error = client
        .request(route, vec![b'x'; MAX_BODY + 1], RequestOptions::default())
        .await
        .expect_err("a body one byte over the wire maximum is refused");
    assert_eq!(error.outcome(), SendOutcome::NotSent);
    assert_eq!(error.code(), "body_too_large");

    let snapshot = client.host_status().await.expect("host.status decodes");
    let shm = &snapshot.shared_memory;
    assert_eq!(shm["artifact"]["profile"], "host-payload-pool-v1");
    assert_eq!(shm["artifact"]["layout_version"], 4);
    assert_eq!(shm["artifact"]["descriptor_schema"], 4);
    assert_eq!(shm["artifact"]["wire_version"], 3);
    assert_eq!(shm["activation"]["completed"], 1);
    assert_eq!(shm["reclamation"]["completed"], 0);
    assert_eq!(
        shm["reclamation"]["meaning"],
        "connection generations ended"
    );
    assert_eq!(
        shm["returns"]["live_backings"], 2,
        "both directions are mapped"
    );
    assert_eq!(shm["returns"]["released_backings"], 0);
    assert_eq!(shm["exhaustion"]["observed"], 0);
    let aggregate = &shm["aggregate"];
    assert_eq!(
        aggregate["total"].as_u64().unwrap(),
        aggregate["transport_bytes"].as_u64().unwrap()
            + aggregate["host_bytes"].as_u64().unwrap()
            + aggregate["terminal_bytes"].as_u64().unwrap()
    );

    client.close_route(route).await.expect("route closes");
    client.close().await.expect("client closes");

    // Controlled recovery: the closed generation is counted, its backing is released once the
    // endpoint unmaps, and a fresh client completes a request on new backing.
    let client = Client::connect(host.publication_path())
        .await
        .expect("a second client attaches after the first closed");
    let route = client
        .open_route(target(), identity("witness-2"))
        .await
        .expect("route opens again");
    let echoed = client
        .request(
            route,
            mode_body(serde_json::json!({"mode": "echo", "value": 2})),
            RequestOptions::default(),
        )
        .await
        .expect("request completes after reconnect");
    assert_eq!(
        echoed.body,
        mode_body(serde_json::json!({"mode": "echo", "value": 2}))
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let snapshot = client.host_status().await.expect("host.status decodes");
        let shm = &snapshot.shared_memory;
        assert_eq!(shm["activation"]["completed"], 2);
        if shm["reclamation"]["completed"] == 1 && shm["returns"]["released_backings"] == 2 {
            assert_eq!(shm["returns"]["live_backings"], 2);
            assert_eq!(shm["returns"]["outstanding"], 0);
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the closed connection's generation never retired or its backing never released: {shm}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    client.close().await.expect("second client closes");
    host.shutdown_gracefully().await;
}

/// A retained response is private: while the caller holds A, more B responses than the 4 KiB
/// class has blocks complete through the same connection, so A pins no transport storage, and
/// A's bytes are unchanged after B's reuse and after the connection closes.
#[tokio::test]
async fn a_retained_response_stays_private_while_transport_storage_is_reused_and_after_close() {
    let host = TestHost::start().await;
    let client = Client::connect(host.publication_path()).await.unwrap();
    let route = client
        .open_route(target(), identity("retained"))
        .await
        .unwrap();
    let a_body =
        mode_body(serde_json::json!({"mode": "echo", "value": "A", "pad": "a".repeat(1_000)}));
    let a = client
        .request(route, a_body.clone(), RequestOptions::default())
        .await
        .expect("A completes");
    assert_eq!(a.body, a_body);

    // More B responses than the 4 KiB class holds: reuse of A's former block is forced while A
    // is retained, and every B completes because A holds no block.
    let block_count = 64 + 8;
    for cycle in 0..block_count {
        let b_body =
            mode_body(serde_json::json!({"mode": "echo", "value": cycle, "pad": "b".repeat(900)}));
        let b = client
            .request(route, b_body.clone(), RequestOptions::default())
            .await
            .expect("B completes while A is retained");
        assert_eq!(b.body, b_body);
    }
    assert_eq!(
        a.body, a_body,
        "A is unchanged by B's reuse of transport storage"
    );

    client.close().await.expect("client closes");
    assert_eq!(
        a.body, a_body,
        "A survives the connection that delivered it"
    );
    host.shutdown_gracefully().await;
}
