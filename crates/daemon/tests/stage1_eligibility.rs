//! One admission policy answers both read lanes with the automatic lane the
//! stricter, `kernel.eligibility.batch` judges object id plus revision, and a
//! stale projection cannot grant what the policy denies.

mod support;

use serde_json::json;
use support::kernel_daemon::{KernelDaemon, candidate, insert_decision, object_ids, verdicts};

#[tokio::test]
async fn one_policy_answers_both_lanes_and_the_automatic_lane_is_stricter() {
    let daemon = KernelDaemon::start().await;
    daemon
        .commit("create", vec![insert_decision(1), insert_decision(2)])
        .await;

    let explicit = daemon.read("explicit_search", None, None).await;
    let automatic = daemon.read("auto_inject", None, None).await;
    let explicit_ids = object_ids(&explicit);
    let automatic_ids = object_ids(&automatic);
    assert_eq!(explicit_ids, ["decision-object-1", "decision-object-2"]);
    assert!(
        automatic_ids.iter().all(|id| explicit_ids.contains(id)),
        "the automatic lane serves a subset of the explicit lane"
    );
    // A freshly asserted decision is labeled on the explicit lane and withheld from injection.
    assert!(
        explicit["rows"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["labeled"] == json!(true))
    );
    assert!(automatic_ids.is_empty());
    assert_eq!(explicit["known_as_of"], automatic["known_as_of"]);
    daemon.shutdown().await;
}

#[tokio::test]
async fn a_batch_check_judges_each_candidate_by_object_id_and_revision() {
    let daemon = KernelDaemon::start().await;
    daemon
        .commit("create", vec![insert_decision(1), insert_decision(2)])
        .await;
    daemon
        .commit(
            "retire-2",
            vec![json!({"op": "retire_decision", "object_id": "decision-object-2"})],
        )
        .await;

    let response = daemon
        .eligibility(
            "local",
            vec![
                candidate("decision-object-1", 1),
                candidate("decision-object-1", 7),
                candidate("decision-object-2", 2),
                candidate("decision-object-9", 1),
            ],
        )
        .await;
    assert_eq!(response["state"]["kind"], "available");
    assert_eq!(
        verdicts(&response),
        [
            ("decision-object-1".to_string(), "ok".to_string()),
            ("decision-object-1".to_string(), "stale".to_string()),
            ("decision-object-2".to_string(), "retracted".to_string()),
            ("decision-object-9".to_string(), "retracted".to_string()),
        ]
    );
    assert_eq!(response["known_as_of"], json!(daemon.tip()));
    daemon.shutdown().await;
}

#[tokio::test]
async fn a_stale_projection_cannot_grant_what_the_policy_denies() {
    let daemon = KernelDaemon::start().await;
    daemon.commit("create", vec![insert_decision(1)]).await;
    let projected_at = daemon.tip();
    let granted = daemon
        .eligibility("local", vec![candidate("decision-object-1", 1)])
        .await;
    assert_eq!(
        verdicts(&granted),
        [("decision-object-1".to_string(), "ok".to_string())]
    );

    daemon
        .commit(
            "retire-1",
            vec![json!({"op": "retire_decision", "object_id": "decision-object-1"})],
        )
        .await;

    // The projection a caller holds still lists the object at its own snapshot.
    let stale = daemon
        .read("explicit_search", Some(projected_at), None)
        .await;
    assert_eq!(object_ids(&stale), ["decision-object-1"]);
    assert_eq!(stale["known_as_of"], json!(projected_at));

    // The policy judges at the tip, so the same candidate is now denied.
    let denied = daemon
        .eligibility("local", vec![candidate("decision-object-1", 1)])
        .await;
    assert_eq!(
        verdicts(&denied),
        [("decision-object-1".to_string(), "retracted".to_string())]
    );
    assert_eq!(denied["known_as_of"], json!(daemon.tip()));
    let current = daemon.read("explicit_search", None, None).await;
    assert!(object_ids(&current).is_empty());
    daemon.shutdown().await;
}
