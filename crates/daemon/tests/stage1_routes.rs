//! `kernel.commit` retries yield one receipt and no duplicate effect, and
//! `kernel.read` answers a batch of object ids in one call.

mod support;

use serde_json::json;
use support::kernel_daemon::{KernelDaemon, insert_decision, object_ids, state_kind};

#[tokio::test]
async fn a_commit_retried_with_the_same_intent_yields_one_receipt() {
    let daemon = KernelDaemon::start().await;
    let first = daemon.commit("create-1", vec![insert_decision(1)]).await;
    assert_eq!(state_kind(&first), "available");
    assert_eq!(first["receipt"]["replayed"], false);
    let commit_seq = first["receipt"]["commit_seq"].as_i64().unwrap();
    let tip = daemon.tip();

    let retried = daemon.commit("create-1", vec![insert_decision(1)]).await;
    assert_eq!(state_kind(&retried), "available");
    assert_eq!(retried["receipt"]["commit_seq"], commit_seq);
    assert_eq!(retried["receipt"]["replayed"], true);
    assert_eq!(retried["tokens"], first["tokens"]);
    assert_eq!(retried["known_as_of"], first["known_as_of"]);
    assert_eq!(daemon.tip(), tip);
    daemon.shutdown().await;
}

#[tokio::test]
async fn a_commit_whose_response_was_lost_leaves_no_duplicate_when_retried() {
    let daemon = KernelDaemon::start().await;
    // The first response is dropped unread, as a caller whose transport lost it would.
    let _lost = daemon.commit("create-2", vec![insert_decision(2)]).await;
    let tip_after_first = daemon.tip();

    let retried = daemon.commit("create-2", vec![insert_decision(2)]).await;
    assert_eq!(state_kind(&retried), "available");
    assert_eq!(retried["receipt"]["replayed"], true);
    assert_eq!(retried["receipt"]["commit_seq"], tip_after_first);
    assert_eq!(daemon.tip(), tip_after_first);

    let read = daemon.read("explicit_search", None, None).await;
    assert_eq!(object_ids(&read), ["decision-object-2"]);
    assert_eq!(read["rows"].as_array().unwrap().len(), 1);
    daemon.shutdown().await;
}

#[tokio::test]
async fn a_read_answers_a_batch_of_object_ids_in_one_call() {
    let daemon = KernelDaemon::start().await;
    daemon
        .commit(
            "create-many",
            vec![
                insert_decision(1),
                insert_decision(2),
                insert_decision(3),
                insert_decision(4),
            ],
        )
        .await;

    let batch = daemon
        .read(
            "explicit_search",
            None,
            Some(&[
                "decision-object-1",
                "decision-object-3",
                "decision-object-9",
            ]),
        )
        .await;
    assert_eq!(state_kind(&batch), "available");
    assert_eq!(
        object_ids(&batch),
        ["decision-object-1", "decision-object-3"],
        "the batch holds exactly the requested live objects; an unknown id yields no row"
    );
    assert_eq!(batch["truncated"], json!(false));

    let whole = daemon.read("explicit_search", None, None).await;
    assert_eq!(object_ids(&whole).len(), 4);
    daemon.shutdown().await;
}
