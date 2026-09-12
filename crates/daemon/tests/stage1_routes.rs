//! `kernel.commit` retries yield one receipt and no duplicate effect, and
//! `kernel.read` answers a batch of object ids in one call.

mod support;

use serde_json::json;
use support::kernel_daemon::{
    KernelDaemon, insert_decision, object_ids, retire_decision, state_kind, state_reason,
};

#[tokio::test]
async fn a_commit_retried_with_the_same_intent_replays_one_receipt_from_the_store() {
    let daemon = KernelDaemon::start().await;
    let first = daemon.commit("create-1", vec![insert_decision(1)]).await;
    assert_eq!(state_kind(&first), "available");
    assert_eq!(first["receipt"]["replayed"], false);
    let commit_seq = first["receipt"]["commit_seq"].as_i64().unwrap();
    assert_eq!(
        first["tokens"],
        json!([{"object_id": "decision-object-1", "known_as_of": commit_seq}])
    );
    let tip = daemon.tip();
    assert_eq!(commit_seq, tip);
    let (_, states) = daemon
        .store()
        .object_states(&["decision-object-1".to_string()])
        .unwrap();
    assert_eq!(states[0].as_ref().unwrap().object.created_commit_seq, tip);

    let retried = daemon.commit("create-1", vec![insert_decision(1)]).await;
    assert_eq!(state_kind(&retried), "available");
    assert_eq!(retried["receipt"]["commit_seq"], commit_seq);
    assert_eq!(retried["receipt"]["replayed"], true);
    assert_eq!(retried["tokens"], first["tokens"]);
    assert_eq!(retried["known_as_of"], first["known_as_of"]);
    assert_eq!(retried["known_as_of"], tip);
    assert_eq!(daemon.tip(), tip);
    let read = daemon.read("explicit_search", None, None).await;
    assert_eq!(object_ids(&read), ["decision-object-1"]);

    let reused = daemon
        .commit_with_digest_seed("create-1", "other-bytes", vec![insert_decision(2)])
        .await;
    assert_eq!(state_kind(&reused), "invalid");
    assert_eq!(state_reason(&reused), Some("operation_key_reused"));
    assert_eq!(daemon.tip(), tip);

    // Deduplication is keyed by intent, not by content: the same operations under a new
    // key are a second commit.
    let again = daemon
        .commit("create-1-again", vec![insert_decision(1)])
        .await;
    assert_eq!(state_kind(&again), "invalid");
    assert_eq!(state_reason(&again), Some("already_exists"));
    assert_eq!(daemon.tip(), tip);
    let renamed = daemon.commit("create-3", vec![insert_decision(3)]).await;
    assert_eq!(state_kind(&renamed), "available");
    assert_eq!(renamed["receipt"]["replayed"], false);
    assert_eq!(renamed["receipt"]["commit_seq"], tip + 1);
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
    daemon
        .commit("retire-4", vec![retire_decision("decision-object-4")])
        .await;

    let batch = daemon
        .read(
            "explicit_search",
            None,
            Some(&[
                "decision-object-1",
                "decision-object-3",
                "decision-object-4",
                "decision-object-9",
            ]),
        )
        .await;
    assert_eq!(state_kind(&batch), "available");
    assert_eq!(
        object_ids(&batch),
        ["decision-object-1", "decision-object-3"],
        "the batch holds exactly the requested live objects; a retired or unknown id yields no row"
    );
    let rows = batch["rows"].as_array().unwrap();
    let summaries: Vec<(&str, &str)> = rows
        .iter()
        .map(|row| {
            (
                row["object"]["object_id"].as_str().unwrap(),
                row["decision"]["payload"]["summary"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        summaries,
        [
            ("decision-object-1", "decision 1"),
            ("decision-object-3", "decision 3"),
        ],
        "each row carries its own decision body"
    );
    assert_eq!(batch["truncated"], json!(false));

    let whole = daemon.read("explicit_search", None, None).await;
    assert_eq!(object_ids(&whole).len(), 3);
    daemon.shutdown().await;
}
