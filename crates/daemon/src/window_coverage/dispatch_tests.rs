//! `transform.boundary` through the daemon's request path (WP-P10 daemon side, WP-P24).

use super::*;
use crate::window_coverage::BOUNDARY_PAGE_LIMIT;
use crate::window_coverage::tests::{SESSION, seed_coverage};

fn boundary_handler() -> (Handler, Arc<MemoryStore>, tempfile::TempDir) {
    let (handler, store, dir, project) =
        handler_with_store(Arc::new(ProducerState::default()), default_test_config());
    handler.bind_route(test_route(9), binding(project.to_str().unwrap(), SESSION));
    (handler, store, dir)
}

fn boundary_request(before_sequence: Option<Value>) -> Value {
    let mut request = json!({ "method": "transform.boundary", "v": 3, "session_id": SESSION });
    if let Some(before) = before_sequence {
        request["before_sequence"] = before;
    }
    request
}

async fn page(handler: &Handler, before_sequence: Option<i64>) -> Vec<(String, i64)> {
    let response = call_dispatch_request_on_channel(
        handler,
        9,
        boundary_request(before_sequence.map(Value::from)),
    )
    .await;
    response["anchors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|anchor| {
            (
                anchor["mid"].as_str().unwrap().to_string(),
                anchor["sequence"].as_i64().unwrap(),
            )
        })
        .collect()
}

/// The walk is exhaustive, newest first, strictly below each cursor, never above the rendered
/// boundary, pages at most 4,096 anchors, and ends at an empty page.
#[tokio::test(flavor = "current_thread")]
async fn boundary_walk_is_exhaustive_newest_first_and_bounded_by_the_rendered_row() {
    let (handler, store, _dir) = boundary_handler();
    seed_coverage(&store, 10_000, Some(9_000), None);
    let mut cursor = None;
    let mut walked = Vec::new();
    let mut pages = Vec::new();
    loop {
        let anchors = page(&handler, cursor).await;
        pages.push(anchors.len());
        let Some(last) = anchors.last() else { break };
        assert!(anchors.len() <= BOUNDARY_PAGE_LIMIT);
        assert!(anchors.windows(2).all(|pair| pair[0].1 > pair[1].1));
        assert!(cursor.is_none_or(|before| anchors[0].1 < before));
        cursor = Some(last.1);
        walked.extend(anchors);
    }
    assert_eq!(pages, vec![4_096, 4_096, 808, 0]);
    let expected: Vec<(String, i64)> = (1..=9_000)
        .rev()
        .map(|sequence| (format!("m{}", 2 * sequence), sequence))
        .collect();
    assert_eq!(walked, expected);
    assert_eq!(page(&handler, Some(1)).await, vec![]);
}

/// Without coverage there is no anchor to declare.
#[tokio::test(flavor = "current_thread")]
async fn boundary_page_is_empty_without_a_rendered_boundary() {
    let (handler, store, _dir) = boundary_handler();
    seed_coverage(&store, 5, None, Some(3));
    assert_eq!(page(&handler, None).await, vec![]);
}

/// Unsafe integers and any other malformed body answer `invalid_params`; an unknown method
/// still answers `unrecognized_request_shape`.
#[tokio::test(flavor = "current_thread")]
async fn malformed_boundary_bodies_are_invalid_params() {
    let (handler, store, _dir) = boundary_handler();
    seed_coverage(&store, 5, Some(5), None);
    let safe = (1i64 << 53) - 1;
    assert_eq!(page(&handler, Some(-safe)).await, vec![]);
    assert_eq!(page(&handler, Some(safe)).await.len(), 5);
    let mut bodies = vec![
        boundary_request(Some(json!(safe + 1))),
        boundary_request(Some(json!(-safe - 1))),
        boundary_request(Some(json!(u64::MAX))),
        boundary_request(Some(json!(i64::MIN))),
        boundary_request(Some(json!(i64::MAX))),
        boundary_request(Some(json!(1.5))),
        boundary_request(Some(json!("4"))),
        boundary_request(Some(Value::Null)),
    ];
    let mut version = boundary_request(None);
    version["v"] = json!(2);
    let mut missing_version = boundary_request(None);
    missing_version.as_object_mut().unwrap().remove("v");
    let mut extra = boundary_request(None);
    extra["limit"] = json!(10);
    let mut no_session = boundary_request(None);
    no_session.as_object_mut().unwrap().remove("session_id");
    bodies.extend([version, missing_version, extra, no_session]);
    for body in bodies {
        let outcome = handler.dispatch_value(test_route(9), body.clone()).await;
        assert_eq!(error_code(outcome), "invalid_params", "{body}");
    }
    let unknown = handler
        .dispatch_value(
            test_route(9),
            json!({ "method": "transform.boundaries", "v": 3, "session_id": SESSION }),
        )
        .await;
    assert_eq!(error_code(unknown), "unrecognized_request_shape");
}
