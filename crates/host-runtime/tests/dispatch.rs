//! The dispatcher fences correlations and invokes each handler at most once.
//! The dispatcher orders streams and lets the first terminal settle each race.

mod support;

use std::collections::HashSet;
use std::process::Command;
use std::time::Duration;

use host_runtime::{ResourceDeclaration, RouteClass, StaticComposite};
use support::raw_client::{
    FLAGS_INTERACTIVE, FLAGS_PURE_HEADER, RawClient, RawFrame, TY_CANCEL, TY_ERROR, TY_GOODBYE,
    TY_REQUEST, TY_RESPONSE, TY_STREAM_DATA, TY_STREAM_END,
};
use support::{CompositeTestHost, LINKED_MODULE_ID, TestHost, mode_body};

const BUDGET: Duration = Duration::from_secs(5);
const ROOT: &str = "/workspace/project";
const PANIC_CHILD_ENV: &str = "EIDNARA_HOST_PANIC_REDACTION_CHILD";
const REDACTED_PANIC_DIAGNOSTIC: &str = "eidnara-host handler callback panicked (details redacted)";

#[tokio::test]
async fn a_unary_request_dispatches_once_with_one_matching_terminal() {
    let host = TestHost::start().await;
    let mut client = host.client().await;
    let (channel, epoch) = client
        .route_open(LINKED_MODULE_ID, ROOT, "opencode", "unary")
        .await
        .expect("route");

    let corr = client.next_corr();
    let body = support::echo_body("hello-host");
    client
        .send_frame(TY_REQUEST, FLAGS_INTERACTIVE, channel, epoch, corr, &body)
        .await
        .expect("send request");

    let frame = client.frame_within(BUDGET).await.expect("terminal");
    assert_eq!(frame.ty, TY_RESPONSE);
    assert_eq!(frame.channel, channel, "terminals echo their channel");
    assert_eq!(frame.epoch, epoch, "terminals echo their epoch");
    assert_eq!(frame.corr, corr, "terminals echo their correlation");
    assert_eq!(frame.body, body, "the opaque body round-trips unchanged");
    assert_eq!(host.handler.dispatch_count(), 1);

    assert!(
        tokio::time::timeout(Duration::from_millis(300), client.expect_frame())
            .await
            .is_err(),
        "exactly one terminal per correlation"
    );

    host.shutdown_gracefully().await;
}

#[tokio::test]
async fn an_empty_body_request_dispatches_and_settles() {
    let host = TestHost::start().await;
    let mut client = host.client().await;
    let (channel, epoch) = client
        .route_open(LINKED_MODULE_ID, ROOT, "opencode", "empty")
        .await
        .expect("route");

    let corr = client.next_corr();
    client
        .send_frame(TY_REQUEST, FLAGS_INTERACTIVE, channel, epoch, corr, &[])
        .await
        .expect("send empty request");

    let frame = client.frame_within(BUDGET).await.expect("terminal");
    assert_eq!(frame.ty, TY_RESPONSE);
    assert_eq!(frame.corr, corr);
    assert!(frame.body.is_empty(), "the empty body echoes back empty");
    assert_eq!(host.handler.dispatch_count(), 1);

    host.shutdown_gracefully().await;
}

#[tokio::test]
async fn an_application_error_is_a_terminal_for_its_correlation_only() {
    let host = TestHost::start().await;
    let mut client = host.client().await;
    let (channel, epoch) = client
        .route_open(LINKED_MODULE_ID, ROOT, "opencode", "errors")
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
            &mode_body(serde_json::json!({
                "mode": "error",
                "code": "store_unavailable",
                "message": "storage still opening"
            })),
        )
        .await
        .expect("send");
    let frame = client.frame_within(BUDGET).await.expect("terminal");
    assert_eq!(frame.ty, TY_ERROR);
    assert_eq!(frame.error_code(), "store_unavailable");

    // The transport stays connected and the next request succeeds.
    let corr = client.next_corr();
    client
        .send_frame(
            TY_REQUEST,
            FLAGS_INTERACTIVE,
            channel,
            epoch,
            corr,
            &support::echo_body("still-alive"),
        )
        .await
        .expect("send");
    let frame = client.frame_within(BUDGET).await.expect("terminal");
    assert_eq!(frame.ty, TY_RESPONSE);
    assert_eq!(frame.corr, corr);

    host.shutdown_gracefully().await;
}

#[tokio::test]
async fn streams_are_ordered_and_end_with_exactly_one_terminal() {
    let host = TestHost::start().await;
    let mut client = host.client().await;
    let (channel, epoch) = client
        .route_open(LINKED_MODULE_ID, ROOT, "opencode", "stream")
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
            &mode_body(serde_json::json!({"mode": "stream", "items": 5})),
        )
        .await
        .expect("send");

    let mut items = Vec::new();
    let terminal = loop {
        let frame = client.frame_within(BUDGET).await.expect("stream frame");
        assert_eq!(frame.corr, corr);
        assert_eq!(frame.channel, channel);
        assert_eq!(frame.epoch, epoch);
        match frame.ty {
            TY_STREAM_DATA => items.push(frame.json()["item"].as_u64().expect("item index")),
            _ => break frame,
        }
    };

    assert_eq!(items, vec![0, 1, 2, 3, 4], "stream items must stay ordered");
    assert_eq!(terminal.ty, TY_STREAM_END);
    assert!(terminal.body.is_empty(), "StreamEnd carries no body");
    assert!(
        tokio::time::timeout(Duration::from_millis(300), client.expect_frame())
            .await
            .is_err(),
        "nothing may follow the stream terminal"
    );

    host.shutdown_gracefully().await;
}

/// Each generation owns its correlation namespace.
/// Correlations strictly increase within a generation.
#[tokio::test]
async fn correlation_namespaces_are_per_generation_and_strictly_increasing() {
    let host = TestHost::start().await;

    // Two generations may both use correlation 1 without colliding.
    let mut first = host.client().await;
    let mut second = host.client().await;
    let (first_channel, first_epoch) = first
        .route_open(LINKED_MODULE_ID, ROOT, "opencode", "generation-a")
        .await
        .expect("route a");
    let (second_channel, second_epoch) = second
        .route_open(LINKED_MODULE_ID, ROOT, "opencode", "generation-b")
        .await
        .expect("route b");

    // route_open consumes correlation 1 on each generation.
    // Terminals remain scoped to their generation when both generations reuse a correlation.
    let first_corr = first.next_corr();
    first
        .send_frame(
            TY_REQUEST,
            FLAGS_INTERACTIVE,
            first_channel,
            first_epoch,
            first_corr,
            &support::echo_body("from-a"),
        )
        .await
        .expect("send a");
    let second_corr = second.next_corr();
    second
        .send_frame(
            TY_REQUEST,
            FLAGS_INTERACTIVE,
            second_channel,
            second_epoch,
            second_corr,
            &support::echo_body("from-b"),
        )
        .await
        .expect("send b");
    assert_eq!(
        first_corr, second_corr,
        "the fixture must exercise the same numeric correlation on both"
    );

    let a = first.frame_within(BUDGET).await.expect("terminal a");
    let b = second.frame_within(BUDGET).await.expect("terminal b");
    assert_eq!(a.body, support::echo_body("from-a"));
    assert_eq!(b.body, support::echo_body("from-b"));

    host.shutdown_gracefully().await;
}

#[tokio::test]
async fn a_non_increasing_correlation_closes_the_generation_before_dispatch() {
    for reuse in ["repeat", "lower"] {
        let host = TestHost::start().await;
        let mut client = host.client().await;
        let (channel, epoch) = client
            .route_open(LINKED_MODULE_ID, ROOT, "opencode", "watermark")
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
                &support::echo_body("first"),
            )
            .await
            .expect("send first");
        let frame = client.frame_within(BUDGET).await.expect("terminal");
        assert_eq!(frame.corr, corr);
        let dispatches = host.handler.dispatch_count();

        let violating = if reuse == "repeat" { corr } else { corr - 1 };
        client
            .send_frame(
                TY_REQUEST,
                FLAGS_INTERACTIVE,
                channel,
                epoch,
                violating,
                &support::echo_body("duplicate"),
            )
            .await
            .expect("send violating");

        assert!(
            client
                .drain_until_close(Duration::from_secs(2))
                .await
                .is_empty(),
            "{reuse}: a watermark violation must not produce a terminal"
        );
        assert!(
            client.closed_within(Duration::from_secs(2)).await,
            "{reuse}: the generation must close"
        );
        assert_eq!(
            host.handler.dispatch_count(),
            dispatches,
            "{reuse}: the duplicate must never dispatch"
        );

        host.shutdown_gracefully().await;
    }
}

#[tokio::test]
async fn an_unknown_route_is_refused_with_zero_dispatch() {
    let host = TestHost::start().await;
    let mut client = host.client().await;

    let corr = client.next_corr();
    client
        .send_frame(
            TY_REQUEST,
            FLAGS_INTERACTIVE,
            4242,
            7,
            corr,
            &support::echo_body("nowhere"),
        )
        .await
        .expect("send");
    let frame = client.frame_within(BUDGET).await.expect("terminal");
    assert_eq!(frame.error_code(), "unknown_channel");
    assert_eq!(host.handler.dispatch_count(), 0);

    host.shutdown_gracefully().await;
}

#[tokio::test]
async fn saturated_request_capacity_returns_server_busy_without_dispatch() {
    let host = TestHost::start_with(|config| {
        config.limits.max_pending_requests = 1;
    })
    .await;
    let mut client = host.client().await;
    let (channel, epoch) = client
        .route_open(LINKED_MODULE_ID, ROOT, "opencode", "busy")
        .await
        .expect("route");

    // The never-completing request exhausts the sole pending slot.
    let holding = client.next_corr();
    client
        .send_frame(
            TY_REQUEST,
            FLAGS_INTERACTIVE,
            channel,
            epoch,
            holding,
            &mode_body(serde_json::json!({"mode": "hang"})),
        )
        .await
        .expect("send holding");

    // The handler holds the pending slot before cancellation.
    let deadline = tokio::time::Instant::now() + BUDGET;
    while host.handler.dispatch_count() == 0 {
        assert!(tokio::time::Instant::now() < deadline, "handler never ran");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let dispatches = host.handler.dispatch_count();

    let rejected = client.next_corr();
    client
        .send_frame(
            TY_REQUEST,
            FLAGS_INTERACTIVE,
            channel,
            epoch,
            rejected,
            &support::echo_body("overflow"),
        )
        .await
        .expect("send overflow");
    let (skipped, frame) = client
        .frames_until_corr(rejected, BUDGET)
        .await
        .expect("busy terminal");
    assert!(skipped.is_empty());
    assert_eq!(frame.error_code(), "server_busy");
    assert_eq!(
        host.handler.dispatch_count(),
        dispatches,
        "server_busy must prove no handler dispatch"
    );

    host.shutdown_gracefully().await;
}

/// Cancel and completion race on one settlement object; exactly one terminal may reach the wire.
/// Cancel and completion race on one settlement object; exactly one terminal may reach the wire.
#[tokio::test]
async fn cancel_and_completion_settle_exactly_once() {
    let host = TestHost::start().await;
    let mut client = host.client().await;
    let (channel, epoch) = client
        .route_open(LINKED_MODULE_ID, ROOT, "opencode", "cancel")
        .await
        .expect("route");

    // Cancel wins: the handler waits for cancellation before returning.
    let corr = client.next_corr();
    client
        .send_frame(
            TY_REQUEST,
            FLAGS_INTERACTIVE,
            channel,
            epoch,
            corr,
            &mode_body(serde_json::json!({"mode": "await_cancel"})),
        )
        .await
        .expect("send");
    let deadline = tokio::time::Instant::now() + BUDGET;
    while host.handler.dispatch_count() == 0 {
        assert!(tokio::time::Instant::now() < deadline, "handler never ran");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    client
        .send_frame(TY_CANCEL, FLAGS_PURE_HEADER, channel, epoch, corr, &[])
        .await
        .expect("cancel");

    let frame = client.frame_within(BUDGET).await.expect("terminal");
    assert_eq!(frame.corr, corr);
    assert_eq!(
        frame.error_code(),
        "cancelled",
        "cancellation won the arbiter"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(300), client.expect_frame())
            .await
            .is_err(),
        "the handler's late completion must not settle a second time"
    );

    // Completion wins: a late Cancel against a settled correlation is a no-op.
    let corr = client.next_corr();
    client
        .send_frame(
            TY_REQUEST,
            FLAGS_INTERACTIVE,
            channel,
            epoch,
            corr,
            &support::echo_body("fast"),
        )
        .await
        .expect("send");
    let frame = client.frame_within(BUDGET).await.expect("terminal");
    assert_eq!(frame.ty, TY_RESPONSE);
    client
        .send_frame(TY_CANCEL, FLAGS_PURE_HEADER, channel, epoch, corr, &[])
        .await
        .expect("late cancel");
    assert!(
        tokio::time::timeout(Duration::from_millis(300), client.expect_frame())
            .await
            .is_err(),
        "a late Cancel must be an idempotent no-op"
    );

    // A Cancel for an unknown correlation is a no-op, and the connection survives.
    client
        .send_frame(TY_CANCEL, FLAGS_PURE_HEADER, channel, epoch, 99_999, &[])
        .await
        .expect("unknown cancel");
    let corr = client.next_corr();
    client
        .send_frame(
            TY_REQUEST,
            FLAGS_INTERACTIVE,
            channel,
            epoch,
            corr,
            &support::echo_body("after"),
        )
        .await
        .expect("send");
    let frame = client.frame_within(BUDGET).await.expect("terminal");
    assert_eq!(frame.corr, corr);

    host.shutdown_gracefully().await;
}

#[tokio::test]
async fn simultaneous_cancel_and_completion_still_emit_one_terminal() {
    let host = TestHost::start().await;
    let mut client = host.client().await;
    let (channel, epoch) = client
        .route_open(LINKED_MODULE_ID, ROOT, "opencode", "race")
        .await
        .expect("route");

    for iteration in 0..20 {
        let corr = client.next_corr();
        client
            .send_frame(
                TY_REQUEST,
                FLAGS_INTERACTIVE,
                channel,
                epoch,
                corr,
                &mode_body(serde_json::json!({"mode": "await_completion"})),
            )
            .await
            .expect("send raced request");
        let expected_dispatches = iteration + 1;
        let deadline = tokio::time::Instant::now() + BUDGET;
        while host.handler.dispatch_count() < expected_dispatches {
            assert!(tokio::time::Instant::now() < deadline, "handler never ran");
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        let release = async {
            tokio::task::yield_now().await;
            host.handler.release_completion();
        };
        let cancel = client.send_frame(TY_CANCEL, FLAGS_PURE_HEADER, channel, epoch, corr, &[]);
        let (_, sent) = tokio::join!(release, cancel);
        sent.expect("send cancel");

        let frame = client.frame_within(BUDGET).await.expect("one terminal");
        assert_eq!(frame.corr, corr);
        assert!(matches!(frame.ty, TY_RESPONSE | TY_ERROR));
        assert!(
            tokio::time::timeout(Duration::from_millis(20), client.expect_frame())
                .await
                .is_err(),
            "iteration {iteration} emitted a duplicate terminal"
        );
    }

    host.shutdown_gracefully().await;
}

#[tokio::test]
async fn cancelling_a_stream_stops_it_with_one_terminal() {
    let host = TestHost::start().await;
    let mut client = host.client().await;
    let (channel, epoch) = client
        .route_open(LINKED_MODULE_ID, ROOT, "opencode", "stream-cancel")
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
            &mode_body(serde_json::json!({"mode": "stream_then_hang", "items": 2})),
        )
        .await
        .expect("send");

    let mut seen = 0;
    while seen < 2 {
        let frame = client.frame_within(BUDGET).await.expect("stream item");
        assert_eq!(frame.ty, TY_STREAM_DATA);
        assert_eq!(frame.corr, corr);
        seen += 1;
    }
    client
        .send_frame(TY_CANCEL, FLAGS_PURE_HEADER, channel, epoch, corr, &[])
        .await
        .expect("cancel");

    let frame = client.frame_within(BUDGET).await.expect("terminal");
    assert_eq!(frame.corr, corr);
    assert_eq!(frame.error_code(), "cancelled");
    assert!(
        tokio::time::timeout(Duration::from_millis(300), client.expect_frame())
            .await
            .is_err(),
        "no frame may follow the selected terminal"
    );

    host.shutdown_gracefully().await;
}

/// A request-handler panic is correlation-local and redacted.
#[tokio::test]
async fn a_handler_panic_maps_to_one_redacted_internal_error() {
    let host = TestHost::start().await;
    let mut client = host.client().await;
    let (channel, epoch) = client
        .route_open(LINKED_MODULE_ID, ROOT, "opencode", "panic")
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
            &mode_body(serde_json::json!({"mode": "panic"})),
        )
        .await
        .expect("send");

    let frame = client.frame_within(BUDGET).await.expect("terminal");
    assert_eq!(frame.corr, corr);
    assert_eq!(frame.error_code(), "internal_error");
    let rendered = String::from_utf8_lossy(&frame.body);
    let canary = String::from_utf8_lossy(support::CANARY_BODY);
    assert!(
        !rendered.contains(canary.as_ref()),
        "the panic payload must not reach the wire: {rendered}"
    );

    // The route and connection survive; an unrelated request still completes.
    let corr = client.next_corr();
    client
        .send_frame(
            TY_REQUEST,
            FLAGS_INTERACTIVE,
            channel,
            epoch,
            corr,
            &support::echo_body("after-panic"),
        )
        .await
        .expect("send");
    let frame = client.frame_within(BUDGET).await.expect("terminal");
    assert_eq!(frame.ty, TY_RESPONSE);
    assert_eq!(frame.corr, corr);

    host.shutdown_gracefully().await;
}

#[test]
fn handler_panic_payload_is_redacted_from_process_stderr() {
    assert_panic_payload_is_redacted_from_process_stderr("panic");
}

/// A panic on the blocking thread of request work reaches stderr only as the redacted
/// diagnostic, like a panic on the handler's own poll.
#[test]
fn blocking_work_panic_payload_is_redacted_from_process_stderr() {
    assert_panic_payload_is_redacted_from_process_stderr("blocking_panic");
}

/// Re-runs this binary as the child that panics in `mode`, then inspects its stderr.
fn assert_panic_payload_is_redacted_from_process_stderr(mode: &str) {
    let output = Command::new(std::env::current_exe().expect("test executable"))
        .args(["--exact", "panic_redaction_subprocess_child", "--nocapture"])
        .env(PANIC_CHILD_ENV, mode)
        .output()
        .expect("run panic redaction subprocess");
    assert!(
        output.status.success(),
        "subprocess failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(REDACTED_PANIC_DIAGNOSTIC),
        "fixed redacted diagnostic missing: {stderr}"
    );
    assert!(
        !stderr.contains(String::from_utf8_lossy(support::CANARY_BODY).as_ref()),
        "handler panic payload reached stderr: {stderr}"
    );
    assert!(
        stderr.contains("UNRELATED-PANIC-CANARY"),
        "unrelated panic did not chain to the prior hook: {stderr}"
    );
}

#[tokio::test]
async fn panic_redaction_subprocess_child() {
    let Some(mode) = std::env::var_os(PANIC_CHILD_ENV) else {
        return;
    };
    let mode = mode.to_string_lossy().into_owned();
    std::panic::set_hook(Box::new(|info| eprintln!("prior panic hook: {info}")));

    let host = TestHost::start().await;
    let mut client = host.client().await;
    let (channel, epoch) = client
        .route_open(LINKED_MODULE_ID, ROOT, "opencode", "panic-stderr")
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
            &mode_body(serde_json::json!({"mode": mode})),
        )
        .await
        .expect("send");
    let frame = client.frame_within(BUDGET).await.expect("terminal");
    assert_eq!(frame.error_code(), "internal_error");
    host.shutdown_gracefully().await;

    let _ = std::panic::catch_unwind(|| panic!("UNRELATED-PANIC-CANARY"));
}

/// Releases the held blocking closures when dropped, so a failed assertion cannot leave a
/// blocking thread waiting through the host's shutdown.
struct ReleaseOnDrop<'h>(&'h support::TestHandler);

impl Drop for ReleaseOnDrop<'_> {
    fn drop(&mut self) {
        self.0.release_blocking();
    }
}

/// Sends a request in `mode` on a fresh route and waits until its closure has started on the
/// blocking thread as the `expected_started`th; returns the client, the route, and the
/// request correlation.
async fn start_held_blocking_work(
    host: &TestHost,
    session: &str,
    mode: &str,
    expected_started: usize,
) -> (RawClient, u16, u32, u64) {
    let mut client = host.client().await;
    let (channel, epoch) = client
        .route_open(LINKED_MODULE_ID, ROOT, "opencode", session)
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
            &mode_body(serde_json::json!({"mode": mode})),
        )
        .await
        .expect("send");
    let deadline = tokio::time::Instant::now() + BUDGET;
    while host.handler.blocking_started() < expected_started {
        assert!(
            tokio::time::Instant::now() < deadline,
            "blocking work never started"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    (client, channel, epoch, corr)
}

/// Cancelling a request whose blocking work is held settles nothing until that work
/// finishes; the captured charge is released exactly once, by the work.
#[tokio::test]
async fn cancel_waits_for_the_request_blocking_work() {
    let host = TestHost::start().await;
    let release = ReleaseOnDrop(&host.handler);
    let (mut client, channel, epoch, corr) =
        start_held_blocking_work(&host, "cancel-held", "blocking_hold", 1).await;

    client
        .send_frame(TY_CANCEL, FLAGS_PURE_HEADER, channel, epoch, corr, &[])
        .await
        .expect("cancel");
    assert!(
        tokio::time::timeout(Duration::from_millis(300), client.expect_frame())
            .await
            .is_err(),
        "the cancellation settled while the blocking work was still held"
    );
    assert_eq!(host.handler.blocking_charges_released(), 0);
    drop(release);

    let frame = client.frame_within(BUDGET).await.expect("terminal");
    assert_eq!(frame.corr, corr);
    assert_eq!(frame.error_code(), "cancelled");
    assert_eq!(
        host.handler.blocking_charges_released(),
        1,
        "the work released its charge once"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(300), client.expect_frame())
            .await
            .is_err(),
        "the finished work must not settle a second time"
    );

    // The route still serves.
    let corr = client.next_corr();
    client
        .send_frame(
            TY_REQUEST,
            FLAGS_INTERACTIVE,
            channel,
            epoch,
            corr,
            &mode_body(serde_json::json!({"mode": "echo"})),
        )
        .await
        .expect("send");
    let frame = client.frame_within(BUDGET).await.expect("terminal");
    assert_eq!(frame.ty, TY_RESPONSE);
    assert_eq!(frame.corr, corr);
    host.shutdown_gracefully().await;
}

/// Closing a route whose request holds blocking work runs route-gone only after that work
/// finishes; the request settles as cancelled and the charge is released exactly once.
#[tokio::test]
async fn route_close_waits_for_the_request_blocking_work() {
    let host = TestHost::start().await;
    let release = ReleaseOnDrop(&host.handler);
    let (mut client, channel, epoch, corr) =
        start_held_blocking_work(&host, "close-held", "blocking_hold", 1).await;

    client
        .send_frame(TY_GOODBYE, FLAGS_PURE_HEADER, channel, epoch, 0, &[])
        .await
        .expect("route goodbye");
    assert!(
        tokio::time::timeout(Duration::from_millis(300), client.expect_frame())
            .await
            .is_err(),
        "the closed route's request settled while its blocking work was still held"
    );
    assert!(
        host.handler.route_gones().is_empty(),
        "route-gone ran while the blocking work was still held"
    );
    assert_eq!(host.handler.blocking_charges_released(), 0);
    drop(release);

    let (_, frame) = client
        .frames_until_corr(corr, BUDGET)
        .await
        .expect("terminal for the closed route's work");
    assert_eq!(frame.error_code(), "cancelled");
    await_route_gone_once(&host, channel).await;
    assert_eq!(host.handler.blocking_charges_released(), 1);
    host.shutdown_gracefully().await;
}

/// A handler can answer before its blocking work finishes.
/// Route close still waits for that work.
#[tokio::test]
async fn route_close_waits_for_blocking_work_the_handler_did_not_await() {
    let host = TestHost::start().await;
    let release = ReleaseOnDrop(&host.handler);
    let (mut client, channel, epoch, corr) =
        start_held_blocking_work(&host, "close-detached", "blocking_detached", 1).await;
    let frame = client.frame_within(BUDGET).await.expect("terminal");
    assert_eq!(frame.corr, corr);
    assert_eq!(
        frame.ty, TY_RESPONSE,
        "the handler answered without waiting"
    );
    assert_eq!(host.handler.blocking_charges_released(), 0);

    client
        .send_frame(TY_GOODBYE, FLAGS_PURE_HEADER, channel, epoch, 0, &[])
        .await
        .expect("route goodbye");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        host.handler.route_gones().is_empty(),
        "route-gone ran while the detached blocking work was still held"
    );
    drop(release);

    await_route_gone_once(&host, channel).await;
    assert_eq!(host.handler.blocking_charges_released(), 1);
    host.shutdown_gracefully().await;
}

/// A panic inside the blocking work reports through the request as the handler's internal
/// error and releases the charge the closure held; the process-level redaction is checked by
/// the subprocess test.
#[tokio::test]
async fn a_blocking_work_panic_settles_as_one_internal_error() {
    let host = TestHost::start().await;
    let mut client = host.client().await;
    let (channel, epoch) = client
        .route_open(LINKED_MODULE_ID, ROOT, "opencode", "blocking-panic")
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
            &mode_body(serde_json::json!({"mode": "blocking_panic"})),
        )
        .await
        .expect("send");
    let frame = client.frame_within(BUDGET).await.expect("terminal");
    assert_eq!(frame.corr, corr);
    assert_eq!(frame.error_code(), "internal_error");
    assert!(
        tokio::time::timeout(Duration::from_millis(300), client.expect_frame())
            .await
            .is_err(),
        "one terminal"
    );
    assert_eq!(
        host.handler.blocking_charges_released(),
        1,
        "the unwinding closure released its charge once"
    );
    host.shutdown_gracefully().await;
}

#[tokio::test]
async fn oversized_handler_output_cannot_corrupt_framing() {
    let host = TestHost::start().await;
    let mut client = host.client().await;
    let (channel, epoch) = client
        .route_open(LINKED_MODULE_ID, ROOT, "opencode", "output-limits")
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
            &mode_body(serde_json::json!({"mode": "oversize_response"})),
        )
        .await
        .expect("send");
    let frame = client.frame_within(BUDGET).await.expect("bounded terminal");
    assert_eq!(frame.corr, corr);
    assert_eq!(frame.error_code(), "internal_error");

    let corr = client.next_corr();
    client
        .send_frame(
            TY_REQUEST,
            FLAGS_INTERACTIVE,
            channel,
            epoch,
            corr,
            &mode_body(serde_json::json!({"mode": "oversize_stream"})),
        )
        .await
        .expect("send");
    // An oversized stream item settles the request with an error terminal; a later unary response cannot reach the client.
    // The handler's later unary response loses to the error terminal that settled the correlation.
    // The client cannot observe a truncated-but-successful stream.
    let frame = client.frame_within(BUDGET).await.expect("bounded terminal");
    assert_eq!(frame.corr, corr);
    assert_eq!(frame.error_code(), "internal_error");

    host.shutdown_gracefully().await;
}

#[tokio::test]
async fn concurrent_handler_output_is_reserved_before_allocation() {
    let host = TestHost::start_with(|config| {
        // The cached catalog reduces the resident budget at startup.
        // 64 KiB of headroom keeps the egress budget at its one-frame floor.
        config.limits.max_resident_bytes = host_runtime::config::MIN_RESIDENT_BYTES + 64 * 1024;
    })
    .await;
    let mut client = host.client().await;
    let (channel, epoch) = client
        .route_open(LINKED_MODULE_ID, ROOT, "opencode", "egress")
        .await
        .expect("route");

    let mut corrs = Vec::new();
    for _ in 0..2 {
        let corr = client.next_corr();
        client
            .send_frame(
                TY_REQUEST,
                FLAGS_INTERACTIVE,
                channel,
                epoch,
                corr,
                &mode_body(serde_json::json!({
                    "mode": "reserve_then_await_completion",
                    "bytes": 40 * 1024 * 1024
                })),
            )
            .await
            .expect("send large response request");
        corrs.push(corr);
    }

    let deadline = tokio::time::Instant::now() + BUDGET;
    while host.handler.dispatch_count() < 2 || host.handler.output_reservation_count() < 1 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "handlers did not start"
        );
        tokio::task::yield_now().await;
    }
    assert_eq!(
        host.handler.output_reservation_count(),
        1,
        "the egress budget cannot reserve two 40 MiB handler buffers"
    );

    host.handler.release_completion();
    let first = client
        .frame_within(Duration::from_secs(30))
        .await
        .expect("first large response");
    assert_eq!(first.ty, TY_RESPONSE);
    corrs.retain(|corr| *corr != first.corr);

    let deadline = tokio::time::Instant::now() + BUDGET;
    while host.handler.output_reservation_count() < 2 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "second reservation stayed blocked after the first body flushed"
        );
        tokio::task::yield_now().await;
    }
    host.handler.release_completion();
    let second = client
        .frame_within(Duration::from_secs(30))
        .await
        .expect("second large response");
    assert_eq!(second.ty, TY_RESPONSE);
    corrs.retain(|corr| *corr != second.corr);
    assert!(corrs.is_empty());

    host.shutdown_gracefully().await;
}

#[tokio::test]
async fn egress_budget_deadline_retires_the_generation() {
    let host = TestHost::start_with(|config| {
        config.limits.max_resident_bytes = host_runtime::config::MIN_RESIDENT_BYTES + 64 * 1024;
        config.timing.frame_deadline = Duration::from_millis(100);
    })
    .await;
    let mut client = host.client().await;
    let (channel, epoch) = client
        .route_open(LINKED_MODULE_ID, ROOT, "opencode", "egress-deadline")
        .await
        .expect("route");

    for _ in 0..2 {
        let corr = client.next_corr();
        client
            .send_frame(
                TY_REQUEST,
                FLAGS_INTERACTIVE,
                channel,
                epoch,
                corr,
                &mode_body(serde_json::json!({
                    "mode": "reserve_then_await_completion",
                    "bytes": 40 * 1024 * 1024
                })),
            )
            .await
            .expect("send large response request");
    }

    let deadline = tokio::time::Instant::now() + BUDGET;
    while host.handler.dispatch_count() < 2 || host.handler.output_reservation_count() < 1 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "handlers did not start"
        );
        tokio::task::yield_now().await;
    }
    assert!(
        client.closed_within(BUDGET).await,
        "the blocked second reservation must retire the generation"
    );

    host.shutdown_gracefully().await;
}

/// A semantic channel-0 rejection encodes from the terminal reserve, so it settles while a
/// maximum response holds every ordinary egress byte.
#[tokio::test]
async fn a_control_rejection_settles_while_ordinary_egress_is_exhausted() {
    let host = TestHost::start_with(|config| {
        config.timing.frame_deadline = Duration::from_millis(300);
    })
    .await;
    let mut client = host.client().await;
    let (channel, epoch) = client
        .route_open(LINKED_MODULE_ID, ROOT, "opencode", "egress-hold")
        .await
        .expect("route");
    // `bytes + HEADER_LEN` equals `EGRESS_RESERVED_BYTES`, so ordinary egress is empty.
    let corr = client.next_corr();
    client
        .send_frame(
            TY_REQUEST,
            FLAGS_INTERACTIVE,
            channel,
            epoch,
            corr,
            &mode_body(serde_json::json!({
                "mode": "reserve_then_await_completion",
                "bytes": 64 * 1024 * 1024
            })),
        )
        .await
        .expect("send maximum response request");
    let deadline = tokio::time::Instant::now() + BUDGET;
    while host.handler.output_reservation_count() < 1 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the maximum reservation never landed"
        );
        tokio::task::yield_now().await;
    }

    let rejected = client
        .control(&serde_json::json!({"op": "nope"}))
        .await
        .expect("send unsupported control");
    let (skipped, frame) = client
        .frames_until_corr(rejected, Duration::from_secs(2))
        .await
        .expect("the rejection terminal must not wait for ordinary egress");
    assert!(skipped.is_empty(), "the held response must not settle");
    assert_eq!(frame.ty, TY_ERROR);
    assert_eq!(frame.error_code(), "unsupported_operation");

    host.handler.release_completion();
    host.shutdown_gracefully().await;
}

#[tokio::test]
async fn closing_a_route_settles_its_admitted_work() {
    let host = TestHost::start().await;
    let mut client = host.client().await;
    let (channel, epoch) = client
        .route_open(LINKED_MODULE_ID, ROOT, "opencode", "close")
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
            &mode_body(serde_json::json!({"mode": "hang"})),
        )
        .await
        .expect("send");
    let deadline = tokio::time::Instant::now() + BUDGET;
    while host.handler.dispatch_count() == 0 {
        assert!(tokio::time::Instant::now() < deadline, "handler never ran");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    client
        .send_frame(TY_GOODBYE, FLAGS_PURE_HEADER, channel, epoch, 0, &[])
        .await
        .expect("route goodbye");

    // The in-flight request settles as cancelled while the writer is still live.
    let (_, frame) = client
        .frames_until_corr(corr, BUDGET)
        .await
        .expect("terminal for the closed route's work");
    assert_eq!(frame.error_code(), "cancelled");

    // The route cleanup callback runs exactly once.
    await_route_gone_once(&host, channel).await;

    host.shutdown_gracefully().await;
}

#[tokio::test]
async fn concurrent_requests_never_interleave_frame_bytes() {
    let host = TestHost::start().await;
    let mut client = host.client().await;
    let (channel, epoch) = client
        .route_open(LINKED_MODULE_ID, ROOT, "opencode", "concurrent")
        .await
        .expect("route");

    // Overlapping streams and unary work contend for the single writer.
    let mut expected = HashSet::new();
    for _ in 0..6 {
        let corr = client.next_corr();
        expected.insert(corr);
        client
            .send_frame(
                TY_REQUEST,
                FLAGS_INTERACTIVE,
                channel,
                epoch,
                corr,
                &mode_body(serde_json::json!({"mode": "stream", "items": 4})),
            )
            .await
            .expect("send stream");
        let corr = client.next_corr();
        expected.insert(corr);
        client
            .send_frame(
                TY_REQUEST,
                FLAGS_INTERACTIVE,
                channel,
                epoch,
                corr,
                &support::echo_body("unary"),
            )
            .await
            .expect("send unary");
    }

    // A clean frame decode requires non-interleaved socket writes.
    // No two frames' bytes interleave on the socket.
    let mut settled = HashSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while settled.len() < expected.len() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "settled {settled:?} of {expected:?}"
        );
        let frame: RawFrame = client.frame_within(BUDGET).await.expect("frame");
        assert!(
            frame.ty <= TY_GOODBYE,
            "a torn frame would decode a bogus type"
        );
        assert!(
            expected.contains(&frame.corr),
            "unexpected correlation {}",
            frame.corr
        );
        match frame.ty {
            TY_STREAM_DATA => {}
            TY_RESPONSE | TY_STREAM_END => {
                assert!(settled.insert(frame.corr), "duplicate terminal");
            }
            other => panic!("unexpected frame type {other}"),
        }
    }

    host.shutdown_gracefully().await;
}

fn model_execution_reservation() -> ResourceDeclaration {
    ResourceDeclaration {
        reserved_handler_tasks: 96,
        reserved_pending_requests: 96,
        retained_resident_bytes: 0,
        general_task_hold_bound: 0,
        route_class: RouteClass::Reserved,
    }
}

/// The next reserved-class request is rejected while a general request dispatches.
#[tokio::test]
async fn saturated_model_execution_reserve_cannot_consume_a_general_slot() {
    let (context, local_embeddings, model_execution) = support::stub_trio();
    let model_execution = model_execution.with_resources(model_execution_reservation());
    let composite =
        StaticComposite::new(context.clone(), local_embeddings, model_execution.clone())
            .expect("distinct ids");
    let host = CompositeTestHost::start(composite, |config| {
        // Each pool has one general slot beside the 96-slot reserve.
        config.limits.max_pending_requests = 97;
        config.limits.max_handler_tasks = 97;
    })
    .await;
    let mut client = host.client().await;
    let (br_channel, br_epoch) = client
        .route_open_target(
            "management_surface",
            "model_execution",
            ROOT,
            "opencode",
            "s1",
        )
        .await
        .expect("model_execution binds");
    let (context_channel, context_epoch) = client
        .route_open_target("tool_provider", "context", ROOT, "opencode", "s1")
        .await
        .expect("context binds");

    // Hanging handler tasks hold all 96 reserved pending-request and task permits. One
    // connection may hold at most 63 admitted terminal obligations, so the hangs are split
    // across two connections, each keeping credits free for the requests below.
    let mut second = host.client().await;
    let (second_channel, second_epoch) = second
        .route_open_target(
            "management_surface",
            "model_execution",
            ROOT,
            "opencode",
            "s2",
        )
        .await
        .expect("model_execution binds on the second connection");
    for index in 0..96 {
        let (peer, channel, epoch) = if index % 2 == 0 {
            (&mut client, br_channel, br_epoch)
        } else {
            (&mut second, second_channel, second_epoch)
        };
        let corr = peer.next_corr();
        peer.send_frame(
            TY_REQUEST,
            FLAGS_INTERACTIVE,
            channel,
            epoch,
            corr,
            &mode_body(serde_json::json!({"mode": "hang"})),
        )
        .await
        .expect("send reserved hang");
    }
    let deadline = tokio::time::Instant::now() + BUDGET;
    while model_execution.dispatch_count() < 96 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "reserved handlers never occupied their permits"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    // Request 97 on the reserved class fails fast without dispatch.
    let rejected = client.next_corr();
    client
        .send_frame(
            TY_REQUEST,
            FLAGS_INTERACTIVE,
            br_channel,
            br_epoch,
            rejected,
            &mode_body(serde_json::json!({"mode": "echo"})),
        )
        .await
        .expect("send overflow");
    let (skipped, frame) = client
        .frames_until_corr(rejected, BUDGET)
        .await
        .expect("busy terminal");
    assert!(skipped.is_empty());
    assert_eq!(frame.error_code(), "server_busy");
    assert_eq!(
        model_execution.dispatch_count(),
        96,
        "the rejected reserved request must never dispatch"
    );

    // The general class is untouched: a context request settles.
    let corr = client.next_corr();
    client
        .send_frame(
            TY_REQUEST,
            FLAGS_INTERACTIVE,
            context_channel,
            context_epoch,
            corr,
            &mode_body(serde_json::json!({"mode": "echo"})),
        )
        .await
        .expect("send general request");
    let (skipped, frame) = client
        .frames_until_corr(corr, BUDGET)
        .await
        .expect("general terminal");
    assert!(skipped.is_empty());
    assert_eq!(frame.ty, TY_RESPONSE);
    assert_eq!(frame.json()["served_by"], "context");

    drop(client);
    drop(second);
    host.shutdown().await.expect("graceful shutdown");
}

/// Exhausting general capacity rejects further general requests while a reserved-class request dispatches.
#[tokio::test]
async fn saturated_general_capacity_cannot_consume_the_model_execution_reserve() {
    let (context, local_embeddings, model_execution) = support::stub_trio();
    let model_execution = model_execution.with_resources(model_execution_reservation());
    let composite =
        StaticComposite::new(context.clone(), local_embeddings, model_execution.clone())
            .expect("distinct ids");
    let host = CompositeTestHost::start(composite, |config| {
        config.limits.max_pending_requests = 97;
        config.limits.max_handler_tasks = 97;
    })
    .await;
    let mut client = host.client().await;
    let (context_channel, context_epoch) = client
        .route_open_target("tool_provider", "context", ROOT, "opencode", "s1")
        .await
        .expect("context binds");
    let (br_channel, br_epoch) = client
        .route_open_target(
            "management_surface",
            "model_execution",
            ROOT,
            "opencode",
            "s1",
        )
        .await
        .expect("model_execution binds");

    let holding = client.next_corr();
    client
        .send_frame(
            TY_REQUEST,
            FLAGS_INTERACTIVE,
            context_channel,
            context_epoch,
            holding,
            &mode_body(serde_json::json!({"mode": "hang"})),
        )
        .await
        .expect("send general hang");
    let deadline = tokio::time::Instant::now() + BUDGET;
    while context.dispatch_count() == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the general handler never occupied its permit"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let rejected = client.next_corr();
    client
        .send_frame(
            TY_REQUEST,
            FLAGS_INTERACTIVE,
            context_channel,
            context_epoch,
            rejected,
            &mode_body(serde_json::json!({"mode": "echo"})),
        )
        .await
        .expect("send general overflow");
    let (skipped, frame) = client
        .frames_until_corr(rejected, BUDGET)
        .await
        .expect("busy terminal");
    assert!(skipped.is_empty());
    assert_eq!(frame.error_code(), "server_busy");
    assert_eq!(context.dispatch_count(), 1);

    // The reserved class stays available to ModelExecution.
    let corr = client.next_corr();
    client
        .send_frame(
            TY_REQUEST,
            FLAGS_INTERACTIVE,
            br_channel,
            br_epoch,
            corr,
            &mode_body(serde_json::json!({"mode": "echo"})),
        )
        .await
        .expect("send reserved request");
    let (skipped, frame) = client
        .frames_until_corr(corr, BUDGET)
        .await
        .expect("reserved terminal");
    assert!(skipped.is_empty());
    assert_eq!(frame.ty, TY_RESPONSE);
    assert_eq!(frame.json()["served_by"], "model_execution");

    host.shutdown().await.expect("graceful shutdown");
}

/// Work offered from a task that outlived its handler is either joined by route close or
/// refused: no closure starts after route-gone has run. The test opens and closes the route
/// 500 times to expose the race window.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn work_offered_while_the_route_drains_is_joined_or_refused() {
    let host = TestHost::start().await;
    let mut client = host.client().await;
    for round in 0..500usize {
        let (channel, epoch) = client
            .route_open(
                LINKED_MODULE_ID,
                ROOT,
                "opencode",
                &format!("orphan-{round}"),
            )
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
                &mode_body(serde_json::json!({"mode": "blocking_orphan"})),
            )
            .await
            .expect("send");
        let frame = client.frame_within(BUDGET).await.expect("terminal");
        assert_eq!((frame.corr, frame.ty), (corr, TY_RESPONSE));

        // The varying close delay samples different offer/close interleavings.
        let goodbye_at =
            std::time::Instant::now() + Duration::from_nanos((round % 300) as u64 * 200);
        while std::time::Instant::now() < goodbye_at {
            std::hint::spin_loop();
        }
        client
            .send_frame(TY_GOODBYE, FLAGS_PURE_HEADER, channel, epoch, 0, &[])
            .await
            .expect("route goodbye");
        let deadline = tokio::time::Instant::now() + BUDGET;
        while host.handler.blocking_refused() <= round
            || !host
                .handler
                .route_gones()
                .iter()
                .any(|handle| handle.channel == channel && handle.epoch == epoch)
        {
            assert!(
                tokio::time::Instant::now() < deadline,
                "round {round}: the offer was never refused or route-gone never ran"
            );
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert_eq!(
            host.handler.blocking_started(),
            host.handler.blocking_started_at_route_gone(),
            "round {round}: blocking work started after route-gone ran"
        );
    }
    host.shutdown_gracefully().await;
}

#[tokio::test]
async fn detached_blocking_work_observes_route_cancellation() {
    let host = TestHost::start_with(|config| {
        config.timing.route_close_budget = Duration::from_millis(100);
    })
    .await;
    let release = ReleaseOnDrop(&host.handler);
    let (mut client, channel, epoch, _) =
        start_held_blocking_work(&host, "cooperative", "blocking_cooperative", 1).await;
    assert_eq!(client.frame_within(BUDGET).await.unwrap().ty, TY_RESPONSE);
    client
        .send_frame(TY_GOODBYE, FLAGS_PURE_HEADER, channel, epoch, 0, &[])
        .await
        .unwrap();
    let stopped = tokio::time::timeout(Duration::from_secs(1), async {
        while host.handler.blocking_charges_released() == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok();
    drop(release);
    let shutdown = host.shutdown().await;
    assert!(stopped, "detached work never observed route cancellation");
    assert!(
        shutdown.is_ok(),
        "cooperative close must not trip fatal: {shutdown:?}"
    );
}

/// Longer than both close windows of every `route_close_budget` these tests configure (at most
/// 300 ms), with a margin for a loaded scheduler.
const PAST_BOTH_CLOSE_WINDOWS: Duration = Duration::from_secs(1);

/// Waits for route-gone on `channel` and asserts that it fired exactly once.
async fn await_route_gone_once(host: &TestHost, channel: u16) {
    let deadline = tokio::time::Instant::now() + BUDGET;
    loop {
        let count = host
            .handler
            .route_gones()
            .iter()
            .filter(|handle| handle.channel == channel)
            .count();
        if count > 0 {
            assert_eq!(count, 1, "route-gone must fire once");
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "route-gone missing after the work stopped"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Asserts that route-gone fired exactly once for `channel` over the whole run; called after
/// shutdown has drained every lifecycle task.
fn assert_route_gone_count(handler: &support::TestHandler, channel: u16) {
    assert_eq!(
        handler
            .route_gones()
            .iter()
            .filter(|handle| handle.channel == channel)
            .count(),
        1,
        "route-gone fired once after every lifecycle task drained"
    );
}

/// Asserts that nothing arrives on `client` within `window`, naming whatever did arrive.
async fn assert_silent(client: &mut RawClient, window: Duration, what: &str) {
    if let Ok(read) = tokio::time::timeout(window, client.expect_frame()).await {
        panic!(
            "{what}: a frame or read result arrived while the blocking work was held: {:?}",
            read.map(|frame| (frame.ty, frame.channel, frame.corr))
        );
    }
}

/// The host keeps serving new connections when a closing connection's blocking work
/// outlives twice `route_close_budget`; the held route keeps its slot, and route-gone runs
/// once after the work stops.
#[tokio::test]
async fn a_dropped_connection_with_work_past_the_close_budget_keeps_the_host_serving() {
    let host = TestHost::start_with(|config| {
        config.timing.route_close_budget = Duration::from_millis(50);
        config.limits.max_routes = 2;
    })
    .await;
    let release = ReleaseOnDrop(&host.handler);
    let (client, channel, _epoch, _corr) =
        start_held_blocking_work(&host, "slow-pass", "blocking_hold", 1).await;
    drop(client);
    tokio::time::sleep(PAST_BOTH_CLOSE_WINDOWS).await;
    assert!(
        host.handler.route_gones().is_empty(),
        "route-gone ran while the blocking work was still held"
    );

    let mut successor = host.client().await;
    let (next_channel, next_epoch) = successor
        .route_open(LINKED_MODULE_ID, ROOT, "opencode", "slow-pass")
        .await
        .expect("a new connection opens a route while the old route's work is held");
    assert_eq!(
        successor
            .route_open(LINKED_MODULE_ID, ROOT, "opencode", "slow-pass-third")
            .await,
        Err("target_unavailable".to_owned()),
        "the held route keeps its slot, so a third route exceeds max_routes = 2"
    );
    let corr = successor.next_corr();
    successor
        .send_frame(
            TY_REQUEST,
            FLAGS_INTERACTIVE,
            next_channel,
            next_epoch,
            corr,
            &mode_body(serde_json::json!({"mode": "echo"})),
        )
        .await
        .expect("send");
    let (_, frame) = successor
        .frames_until_corr(corr, BUDGET)
        .await
        .expect("echo terminal");
    assert_eq!(frame.ty, TY_RESPONSE);

    drop(release);
    await_route_gone_once(&host, channel).await;
    assert_eq!(host.handler.blocking_charges_released(), 1);
    successor
        .route_open(LINKED_MODULE_ID, ROOT, "opencode", "slow-pass-third")
        .await
        .expect("route-gone released the held route's slot");
    drop(successor);
    let handler = host.handler.clone();
    let shutdown = host.shutdown().await;
    assert_route_gone_count(&handler, channel);
    assert!(
        shutdown.is_ok(),
        "work that outlived the close budget must not make the host fatal: {shutdown:?}"
    );
}

/// A route `Goodbye` whose blocking work outlives twice `route_close_budget` settles the
/// request as cancelled after the work stops; the connection serves its other routes meanwhile.
#[tokio::test]
async fn a_route_goodbye_with_work_past_the_close_budget_keeps_the_connection_serving() {
    let host = TestHost::start_with(|config| {
        config.timing.route_close_budget = Duration::from_millis(50);
    })
    .await;
    let release = ReleaseOnDrop(&host.handler);
    let (mut client, channel, epoch, corr) =
        start_held_blocking_work(&host, "slow-goodbye", "blocking_hold", 1).await;
    client
        .send_frame(TY_GOODBYE, FLAGS_PURE_HEADER, channel, epoch, 0, &[])
        .await
        .expect("route goodbye");
    assert_silent(&mut client, PAST_BOTH_CLOSE_WINDOWS, "route goodbye").await;

    let (other_channel, other_epoch) = client
        .route_open(LINKED_MODULE_ID, ROOT, "opencode", "slow-goodbye-other")
        .await
        .expect("the connection opens another route while the closed route's work is held");
    let echo = client.next_corr();
    client
        .send_frame(
            TY_REQUEST,
            FLAGS_INTERACTIVE,
            other_channel,
            other_epoch,
            echo,
            &mode_body(serde_json::json!({"mode": "echo"})),
        )
        .await
        .expect("send");
    let (_, frame) = client
        .frames_until_corr(echo, BUDGET)
        .await
        .expect("echo terminal");
    assert_eq!(frame.ty, TY_RESPONSE);

    drop(release);
    let (_, frame) = client
        .frames_until_corr(corr, BUDGET)
        .await
        .expect("terminal for the closed route's work");
    assert_eq!(frame.error_code(), "cancelled");
    await_route_gone_once(&host, channel).await;
    let handler = host.handler.clone();
    host.shutdown_gracefully().await;
    assert_route_gone_count(&handler, channel);
}

/// A request whose route close is deferred keeps counting against `max_pending_requests` until
/// its work stops and the close settles it.
#[tokio::test]
async fn a_deferred_request_keeps_its_pending_permit_until_its_work_stops() {
    let host = TestHost::start_with(|config| {
        config.timing.route_close_budget = Duration::from_millis(50);
        config.limits.max_pending_requests = 1;
    })
    .await;
    let mut other = host.client().await;
    let (other_channel, other_epoch) = other
        .route_open(LINKED_MODULE_ID, ROOT, "opencode", "pending-other")
        .await
        .expect("route");
    let release = ReleaseOnDrop(&host.handler);
    let (mut client, channel, epoch, corr) =
        start_held_blocking_work(&host, "pending-held", "blocking_hold", 1).await;
    client
        .send_frame(TY_GOODBYE, FLAGS_PURE_HEADER, channel, epoch, 0, &[])
        .await
        .expect("route goodbye");
    assert_silent(&mut client, PAST_BOTH_CLOSE_WINDOWS, "route goodbye").await;

    let echo = |client: &mut RawClient| {
        let corr = client.next_corr();
        (corr, mode_body(serde_json::json!({"mode": "echo"})))
    };
    let (busy, body) = echo(&mut other);
    other
        .send_frame(
            TY_REQUEST,
            FLAGS_INTERACTIVE,
            other_channel,
            other_epoch,
            busy,
            &body,
        )
        .await
        .expect("send");
    let (_, frame) = other
        .frames_until_corr(busy, BUDGET)
        .await
        .expect("busy terminal");
    assert_eq!(frame.error_code(), "server_busy");
    assert_eq!(
        frame.json()["message"],
        "pending request capacity exhausted",
        "the deferred request still holds the only pending permit"
    );

    drop(release);
    let (_, frame) = client
        .frames_until_corr(corr, BUDGET)
        .await
        .expect("terminal for the closed route's work");
    assert_eq!(frame.error_code(), "cancelled");
    await_route_gone_once(&host, channel).await;
    let (served, body) = echo(&mut other);
    other
        .send_frame(
            TY_REQUEST,
            FLAGS_INTERACTIVE,
            other_channel,
            other_epoch,
            served,
            &body,
        )
        .await
        .expect("send");
    let (_, frame) = other
        .frames_until_corr(served, BUDGET)
        .await
        .expect("echo terminal");
    assert_eq!(
        frame.ty, TY_RESPONSE,
        "the settled close released the permit"
    );
    let handler = host.handler.clone();
    host.shutdown_gracefully().await;
    assert_route_gone_count(&handler, channel);
}

/// Graceful shutdown that starts while a deferred close waits on held work joins it: once the
/// work stops, the `cancelled` terminal precedes connection `Goodbye`, route-gone runs once
/// before handler shutdown, and shutdown returns `Ok`.
#[tokio::test]
async fn graceful_shutdown_joins_a_deferred_close_before_goodbye() {
    shutdown_joins_a_deferred_close(true).await;
}

/// Graceful shutdown that starts while a route `Goodbye` close is still inside its close windows
/// joins the deferred close that close hands off to.
#[tokio::test]
async fn graceful_shutdown_inside_the_close_windows_joins_the_later_deferred_close() {
    shutdown_joins_a_deferred_close(false).await;
}

async fn shutdown_joins_a_deferred_close(after_windows: bool) {
    let host = TestHost::start_with(|config| {
        config.timing.route_close_budget =
            Duration::from_millis(if after_windows { 50 } else { 300 });
        config.timing.shutdown_deadline = Duration::from_secs(10);
    })
    .await;
    let handler = host.handler.clone();
    let release = ReleaseOnDrop(&handler);
    let (mut client, channel, epoch, corr) =
        start_held_blocking_work(&host, "shutdown-deferred", "blocking_hold", 1).await;
    client
        .send_frame(TY_GOODBYE, FLAGS_PURE_HEADER, channel, epoch, 0, &[])
        .await
        .expect("route goodbye");
    let goodbye_sent = tokio::time::Instant::now();
    if after_windows {
        assert_silent(&mut client, PAST_BOTH_CLOSE_WINDOWS, "route goodbye").await;
    } else {
        // The read loop turns the route Closing as it takes the `Goodbye`, so a request on the
        // route answered `unknown_channel` shows the close has begun, inside its first window.
        let probe = client.next_corr();
        client
            .send_frame(
                TY_REQUEST,
                FLAGS_INTERACTIVE,
                channel,
                epoch,
                probe,
                &mode_body(serde_json::json!({"mode": "echo"})),
            )
            .await
            .expect("send");
        let (_, frame) = client
            .frames_until_corr(probe, BUDGET)
            .await
            .expect("probe terminal");
        assert_eq!(frame.error_code(), "unknown_channel");
    }

    let mut observer = if after_windows {
        None
    } else {
        Some(host.client().await)
    };
    let shutdown = tokio::spawn(host.shutdown());
    if let Some(observer) = observer.as_mut() {
        // `route.open` answers `target_unavailable` once shutdown has begun draining; seeing
        // that inside both 300 ms windows proves shutdown started before the close deferred.
        tokio::time::timeout_at(goodbye_sent + Duration::from_millis(600), async {
            loop {
                match observer
                    .route_open(LINKED_MODULE_ID, ROOT, "opencode", "shutdown-observer")
                    .await
                {
                    Err(code) if code == "target_unavailable" => break,
                    Ok(_) => tokio::time::sleep(Duration::from_millis(5)).await,
                    Err(err) => panic!("shutdown observer failed: {err}"),
                }
            }
        })
        .await
        .expect("shutdown must begin draining inside both 300 ms close windows");
        assert!(
            goodbye_sent.elapsed() < Duration::from_millis(600),
            "shutdown must begin draining inside both 300 ms close windows; it began after {:?}",
            goodbye_sent.elapsed()
        );
    }
    assert_silent(&mut client, PAST_BOTH_CLOSE_WINDOWS, "shutdown").await;
    assert!(
        !shutdown.is_finished(),
        "shutdown waits for the deferred close"
    );
    drop(release);

    let frames = client.drain_until_close(BUDGET).await;
    let terminal = frames
        .iter()
        .position(|frame| frame.corr == corr && frame.channel == channel)
        .expect("the deferred close emits the request's terminal");
    assert_eq!(frames[terminal].error_code(), "cancelled");
    let goodbye = frames
        .iter()
        .position(|frame| frame.ty == TY_GOODBYE && frame.channel == 0)
        .expect("connection goodbye");
    assert!(
        terminal < goodbye,
        "the terminal precedes connection Goodbye"
    );
    let result = shutdown.await.expect("shutdown task");
    assert!(result.is_ok(), "graceful shutdown: {result:?}");
    let events = handler.events();
    let gone = events
        .iter()
        .position(
            |event| matches!(event, support::Event::RouteGone(handle) if handle.channel == channel),
        )
        .expect("route-gone ran");
    let shut = events
        .iter()
        .position(|event| matches!(event, support::Event::Shutdown))
        .expect("handler shutdown ran");
    assert!(gone < shut, "route-gone precedes handler shutdown");
    assert_route_gone_count(&handler, channel);
}

#[tokio::test]
async fn held_blocking_work_retains_handler_and_instance_after_fatal_close() {
    let root = tempfile::tempdir().unwrap();
    let host = TestHost::start_with(|config| {
        config.data_dir = Some(root.path().to_path_buf());
        config.timing.route_close_budget = Duration::from_millis(50);
        config.timing.shutdown_deadline = Duration::from_millis(200);
        config.timing.lifecycle_callback_deadline = Duration::from_millis(50);
    })
    .await;
    let handler = host.handler.clone();
    let release = ReleaseOnDrop(&handler);
    let (_client, _channel, _epoch, _corr) =
        start_held_blocking_work(&host, "fatal-owner", "blocking_hold", 1).await;
    let result = host.shutdown().await;
    let shutdown_ran = handler.events().contains(&support::Event::Shutdown);
    let dropped = handler.handler_dropped();
    let successor = TestHost::try_start_with(support::TestHandler::new(), |config| {
        config.data_dir = Some(root.path().to_path_buf());
    })
    .await;
    let excluded = matches!(successor, Err(host_runtime::HostError::Instance(_)));
    if let Ok(successor) = successor {
        successor.shutdown_gracefully().await;
    }
    drop(release);
    let deadline = tokio::time::Instant::now() + BUDGET;
    while !handler.handler_dropped() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(result.is_err());
    assert!(
        !shutdown_ran && !dropped && excluded,
        "live work lost ownership: shutdown={shutdown_ran}, dropped={dropped}, successor_excluded={excluded}"
    );
    assert!(
        handler.handler_dropped(),
        "released work must finish deferred cleanup"
    );
}

/// The terminal credit ceiling is per connection and independent of the pending-request limit:
/// with 63 requests admitted and unsettled, the 64th is refused before dispatch even though
/// pending slots remain. A settled terminal returns its credit only once the client has
/// consumed the block, after which the next request dispatches again.
#[tokio::test]
async fn terminal_credits_bound_admission_and_return_with_the_settled_block() {
    const CREDITS: usize = host_runtime::config::TERMINAL_CREDITS_PER_CONNECTION;
    let host = TestHost::start_with(|config| {
        config.limits.max_pending_requests = CREDITS + 8;
    })
    .await;
    let mut client = host.client().await;
    let (channel, epoch) = client
        .route_open(LINKED_MODULE_ID, ROOT, "opencode", "credits")
        .await
        .expect("route");

    let mut holding = Vec::with_capacity(CREDITS);
    for _ in 0..CREDITS {
        let corr = client.next_corr();
        client
            .send_frame(
                TY_REQUEST,
                FLAGS_INTERACTIVE,
                channel,
                epoch,
                corr,
                &mode_body(serde_json::json!({"mode": "hang"})),
            )
            .await
            .expect("send holding");
        holding.push(corr);
    }
    let deadline = tokio::time::Instant::now() + BUDGET;
    while host.handler.dispatch_count() < CREDITS {
        assert!(
            tokio::time::Instant::now() < deadline,
            "only {} of {CREDITS} holding requests dispatched",
            host.handler.dispatch_count()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let rejected = client.next_corr();
    client
        .send_frame(
            TY_REQUEST,
            FLAGS_INTERACTIVE,
            channel,
            epoch,
            rejected,
            &support::echo_body("one too many"),
        )
        .await
        .expect("send overflow");
    let (skipped, frame) = client
        .frames_until_corr(rejected, BUDGET)
        .await
        .expect("busy terminal");
    assert!(skipped.is_empty(), "holding requests must not settle");
    assert_eq!(frame.ty, TY_ERROR);
    assert_eq!(frame.error_code(), "server_busy");
    assert_eq!(
        frame.json()["message"],
        "terminal capacity exhausted",
        "pending slots remain, so only the terminal credit ceiling can refuse this request"
    );
    assert_eq!(
        host.handler.dispatch_count(),
        CREDITS,
        "a credit refusal must prove no handler dispatch"
    );

    // Settling one holding request frees its credit once the client consumes the terminal.
    let released = holding[0];
    client
        .send_frame(TY_CANCEL, FLAGS_PURE_HEADER, channel, epoch, released, &[])
        .await
        .expect("cancel");
    let (skipped, frame) = client
        .frames_until_corr(released, BUDGET)
        .await
        .expect("cancelled terminal");
    assert!(skipped.is_empty());
    assert_eq!(frame.error_code(), "cancelled");

    // The credit follows the block, not the settlement, so it returns once the host's ring
    // reclaims the consumed terminal; admission is retried until that reclamation lands.
    let deadline = tokio::time::Instant::now() + BUDGET;
    loop {
        let corr = client.next_corr();
        client
            .send_frame(
                TY_REQUEST,
                FLAGS_INTERACTIVE,
                channel,
                epoch,
                corr,
                &support::echo_body("after release"),
            )
            .await
            .expect("send after release");
        let (skipped, frame) = client
            .frames_until_corr(corr, BUDGET)
            .await
            .expect("terminal after release");
        assert!(skipped.is_empty());
        if frame.ty == TY_RESPONSE {
            break;
        }
        assert_eq!(frame.error_code(), "server_busy");
        assert_eq!(frame.json()["message"], "terminal capacity exhausted");
        assert!(
            tokio::time::Instant::now() < deadline,
            "the released credit never returned"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(host.handler.dispatch_count(), CREDITS + 1);

    host.shutdown_gracefully().await;
}

/// A request body of at most `INLINE_COPY_MAX_BYTES` is copied on its dispatch task and never
/// waits at the blocking-copy gate, while one byte more takes the blocking thread the gate holds.
#[cfg(feature = "test-support")]
#[tokio::test]
async fn a_body_at_the_inline_limit_copies_on_the_dispatch_task_and_one_byte_more_blocks() {
    use host_runtime::inbound_copy_gate::{INLINE_COPY_MAX_BYTES, hold_inbound_copies_of_len};

    fn echo_body_of_len(len: usize) -> Vec<u8> {
        let base = mode_body(serde_json::json!({"mode": "echo", "pad": ""})).len();
        let body = mode_body(serde_json::json!({"mode": "echo", "pad": "p".repeat(len - base)}));
        assert_eq!(body.len(), len);
        body
    }

    let host = TestHost::start().await;
    let mut client = host.client().await;
    let (channel, epoch) = client
        .route_open(LINKED_MODULE_ID, ROOT, "opencode", "inline-limit")
        .await
        .expect("route");

    let inline = echo_body_of_len(INLINE_COPY_MAX_BYTES);
    let hold = hold_inbound_copies_of_len(u32::try_from(inline.len()).unwrap());
    let corr = client.next_corr();
    client
        .send_frame(TY_REQUEST, FLAGS_INTERACTIVE, channel, epoch, corr, &inline)
        .await
        .expect("send");
    let (_, terminal) = client
        .frames_until_corr(corr, BUDGET)
        .await
        .expect("an inline copy settles while copies of its length are held");
    assert_eq!(terminal.ty, TY_RESPONSE);
    assert_eq!(hold.started(), 0, "the inline copy never reaches the gate");
    drop(hold);

    let blocking = echo_body_of_len(INLINE_COPY_MAX_BYTES + 1);
    let hold = hold_inbound_copies_of_len(u32::try_from(blocking.len()).unwrap());
    let corr = client.next_corr();
    client
        .send_frame(
            TY_REQUEST,
            FLAGS_INTERACTIVE,
            channel,
            epoch,
            corr,
            &blocking,
        )
        .await
        .expect("send");
    let deadline = tokio::time::Instant::now() + BUDGET;
    while hold.started() < 1 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the copy one byte over the inline limit never reached the gate"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    hold.release();
    let (_, terminal) = client
        .frames_until_corr(corr, BUDGET)
        .await
        .expect("the released copy settles");
    assert_eq!(terminal.ty, TY_RESPONSE);
    drop(hold);
    host.shutdown_gracefully().await;
}

/// The production inbound copy is request-scoped blocking work: while it is held on its
/// blocking thread, `Cancel`, a route close, and shutdown each wait for it, the transport block
/// it reads stays outstanding, and the block returns once the copy completes.
#[cfg(feature = "test-support")]
#[tokio::test]
async fn cancel_route_close_and_shutdown_wait_for_the_held_production_inbound_copy() {
    use host_runtime::client::Client;
    use host_runtime::inbound_copy_gate::hold_inbound_copies_of_len;

    async fn outstanding(observer: &Client) -> u64 {
        observer
            .host_status()
            .await
            .expect("host.status")
            .shared_memory["returns"]["outstanding"]
            .as_u64()
            .expect("outstanding returns")
    }

    for interruption in ["cancel", "route_close", "shutdown"] {
        let host = TestHost::start().await;
        let mut client = host.client().await;
        let (channel, epoch) = client
            .route_open(LINKED_MODULE_ID, ROOT, "opencode", interruption)
            .await
            .expect("route");
        let pad = format!("{interruption}-{}", "p".repeat(128 * 1024));
        let body = mode_body(serde_json::json!({"mode": "echo", "pad": pad}));
        let hold = hold_inbound_copies_of_len(u32::try_from(body.len()).unwrap());
        let corr = client.next_corr();
        client
            .send_frame(TY_REQUEST, FLAGS_INTERACTIVE, channel, epoch, corr, &body)
            .await
            .expect("send");
        let deadline = tokio::time::Instant::now() + BUDGET;
        while hold.started() < 1 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the copy never started"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let observer = Client::connect(host.publication_path())
            .await
            .expect("observer attaches");
        assert_eq!(
            outstanding(&observer).await,
            1,
            "the held copy keeps its block"
        );

        match interruption {
            "cancel" => client
                .send_frame(TY_CANCEL, FLAGS_PURE_HEADER, channel, epoch, corr, &[])
                .await
                .expect("cancel"),
            "route_close" => client
                .send_frame(TY_GOODBYE, FLAGS_PURE_HEADER, channel, epoch, 0, &[])
                .await
                .expect("route goodbye"),
            _ => {}
        }
        if interruption == "shutdown" {
            observer.close().await.expect("observer closes");
            let shutdown = host.shutdown();
            tokio::pin!(shutdown);
            assert!(
                tokio::time::timeout(Duration::from_millis(300), &mut shutdown)
                    .await
                    .is_err(),
                "host.shutdown_during_held_inbound_copy: shutdown finished under a held copy"
            );
            hold.release();
            shutdown
                .await
                .expect("shutdown completes once the copy joins");
            continue;
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(300), client.expect_frame())
                .await
                .is_err(),
            "host.cancel_during_barrier_held_copy: {interruption} settled under a held copy"
        );
        assert_eq!(
            outstanding(&observer).await,
            1,
            "the block stays held while the copy reads it"
        );
        hold.release();
        let (_, terminal) = client
            .frames_until_corr(corr, BUDGET)
            .await
            .expect("terminal");
        assert_eq!(terminal.error_code(), "cancelled", "{interruption}");
        let deadline = tokio::time::Instant::now() + BUDGET;
        while outstanding(&observer).await != 0 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the copied block never returned"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        observer.close().await.expect("observer closes");
        host.shutdown_gracefully().await;
    }
}
