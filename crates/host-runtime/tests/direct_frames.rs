//! Direct-serialize responses through a real host and its shared-memory ring.

mod support;

use std::time::Duration;

use support::raw_client::{FLAGS_INTERACTIVE, TY_REQUEST, TY_RESPONSE};
use support::{LINKED_MODULE_ID, TestHost, mode_body};

const BUDGET: Duration = Duration::from_secs(20);
const ROOT: &str = "/workspace/project";
/// With this headroom the egress budget sits at its one-frame floor, so a charge that never returns blocks every later reservation of this size.
const HEADROOM: u64 = 64 * 1024;
/// Two frames of this size do not fit the floored egress budget together.
const BODY: usize = 40 * 1024 * 1024;

fn direct_fill(fault: &str) -> Vec<u8> {
    mode_body(serde_json::json!({
        "mode": "direct_fill",
        "bytes": BODY,
        "value": 0x5a,
        "fault": fault,
    }))
}

/// T3: a serializer that underfills, overfills, fails, or panics publishes no frame for its correlation, and the failure closes the connection. Every arm returns the egress charge: on a host whose egress budget holds one such frame, a fresh connection's full-size direct frame still publishes, with exactly the declared bytes.
#[tokio::test]
async fn a_failed_direct_serializer_publishes_nothing_and_returns_its_charge() {
    let host = TestHost::start_with(|config| {
        config.limits.max_resident_bytes = host_runtime::config::MIN_RESIDENT_BYTES + HEADROOM;
    })
    .await;
    for fault in ["underfill", "overflow", "error", "panic"] {
        let mut client = host.client().await;
        let (channel, epoch) = client
            .route_open(LINKED_MODULE_ID, ROOT, "opencode", fault)
            .await
            .expect("route");
        let corr = client.next_corr();
        client
            .send_frame(
                TY_REQUEST,
                FLAGS_INTERACTIVE,
                channel,
                epoch,
                corr,
                &direct_fill(fault),
            )
            .await
            .expect("send");
        let frames = client.drain_until_close(BUDGET).await;
        assert!(
            frames.iter().all(|frame| frame.corr != corr),
            "{fault}: no frame reaches the client for the failed correlation"
        );

        let mut fresh = host.client().await;
        let (channel, epoch) = fresh
            .route_open(
                LINKED_MODULE_ID,
                ROOT,
                "opencode",
                &format!("{fault}-after"),
            )
            .await
            .expect("route");
        let corr = fresh.next_corr();
        fresh
            .send_frame(
                TY_REQUEST,
                FLAGS_INTERACTIVE,
                channel,
                epoch,
                corr,
                &direct_fill("none"),
            )
            .await
            .expect("send");
        let (_, frame) = fresh
            .frames_until_corr(corr, BUDGET)
            .await
            .unwrap_or_else(|error| panic!("{fault}: the charge returned: {error}"));
        assert_eq!(frame.ty, TY_RESPONSE, "{fault}");
        assert_eq!(frame.len as usize, BODY, "{fault}");
        assert_eq!(frame.body, vec![0x5a; BODY], "{fault}");
    }
    host.shutdown_gracefully().await;
}

/// T4: a unary direct frame is queued from settlement after its handler future has ended, so its serializer runs, and holds the response's source, in the window between the handler's end and publication.
#[tokio::test]
async fn a_direct_frame_serializes_after_its_handler_has_ended() {
    let host = TestHost::start().await;
    let mut client = host.client().await;
    let (channel, epoch) = client
        .route_open(LINKED_MODULE_ID, ROOT, "opencode", "outlived")
        .await
        .expect("route");
    let corr = client.next_corr();
    client
        .send_frame(
            TY_REQUEST,
            FLAGS_INTERACTIVE,
            channel,
            epoch,
            corr,
            &direct_fill("none"),
        )
        .await
        .expect("send");
    let (_, frame) = client.frames_until_corr(corr, BUDGET).await.expect("frame");
    assert_eq!(frame.ty, TY_RESPONSE);
    assert_eq!(frame.body, vec![0x5a; BODY]);
    assert_eq!(
        host.handler.direct_outlived_handler_count(),
        1,
        "the serializer ran after the handler future ended"
    );
    host.shutdown_gracefully().await;
}
