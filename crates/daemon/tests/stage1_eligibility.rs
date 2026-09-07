//! One admission policy answers both read lanes with the automatic lane the
//! stricter, `kernel.eligibility.batch` judges object id plus revision, and a
//! verdict cached before a retirement is not served after it. commentlint: allow(JUDGE)

mod support;

use kernel::{
    AdmissionEvent, AdmissionRequest, CommitIntent, DecisionPayload, DecisionSpec, EventKind,
    KernelStore, ObservationPayload, ObservationSpec, Sensitivity, SourceClass, TaintClass,
};
use serde_json::json;
use support::kernel_daemon::{
    DOMAIN, KernelDaemon, candidate, insert_decision, object_ids, retire_decision, verdicts,
};

/// Writes a decision straight into the store, admitted at `CodeObserved` maturity
/// against an observation of its own lineage: the class `auto_inject` serves.
fn commit_verified_decision(store: &KernelStore, scope_id: &str) -> i64 {
    let intent = CommitIntent {
        producer: "stage1".to_string(),
        operation_key: "verified".to_string(),
        request_digest: "b".repeat(64),
        actor: "test".to_string(),
        cause: "stage1".to_string(),
    };
    store
        .commit(intent, |envelope| {
            envelope.insert_observation(ObservationSpec {
                observation_id: "observation-verified".to_string(),
                object_id: "observation-object-verified".to_string(),
                domain_id: DOMAIN.to_string(),
                proposition_id: None,
                scope_id: None,
                anchor_id: None,
                evidence_id: None,
                observation_kind: "code_present".to_string(),
                payload: ObservationPayload {
                    summary: "code present".to_string(),
                    classification: "code_present".to_string(),
                    detail: None,
                },
                observed_at: 1,
                dependencies: Vec::new(),
                source_kind: "repo".to_string(),
                source_id: "verified-lineage".to_string(),
                source_revision: 1,
                sensitivity: Sensitivity::Normal,
            })?;
            envelope.insert_decision(DecisionSpec {
                decision_id: "decision-verified".to_string(),
                object_id: "decision-object-verified".to_string(),
                domain_id: DOMAIN.to_string(),
                proposition_id: None,
                scope_id: Some(scope_id.to_string()),
                anchor_id: None,
                evidence_id: None,
                decision_kind: "architecture".to_string(),
                payload: DecisionPayload {
                    summary: "verified decision".to_string(),
                    rationale: "observed in code".to_string(),
                },
                source_kind: "repo".to_string(),
                source_id: "verified-lineage".to_string(),
                source_revision: 1,
                sensitivity: Sensitivity::Normal,
            })?;
            envelope.record_admission(AdmissionRequest {
                candidate_id: None,
                subject_object_id: Some("decision-object-verified".to_string()),
                source_class: Some(SourceClass::TrustedLocalCode),
                taint_class: Some(TaintClass::CurrentCode),
                event: AdmissionEvent {
                    kind: EventKind::CodeObserved,
                    trigger_object_id: Some("observation-object-verified".to_string()),
                    approval_object_id: None,
                    evidence_id: None,
                    reason: "CodeObserved".to_string(),
                },
            })?;
            Ok(String::new())
        })
        .unwrap()
        .commit_seq
}

#[tokio::test]
async fn one_policy_answers_both_lanes_and_the_automatic_lane_is_stricter() {
    let daemon = KernelDaemon::start().await;
    // A route-asserted decision materializes the project scope the verified one joins.
    let asserted = daemon.commit("create", vec![insert_decision(1)]).await;
    assert_eq!(asserted["state"]["kind"], "available");
    let scope_id = daemon.read("explicit_search", None, None).await["rows"][0]["scope_id"]
        .as_str()
        .unwrap()
        .to_string();
    commit_verified_decision(&daemon.store(), &scope_id);
    let snapshot = daemon.tip();

    let explicit = daemon.read("explicit_search", Some(snapshot), None).await;
    let automatic = daemon.read("auto_inject", Some(snapshot), None).await;
    let explicit_ids = object_ids(&explicit);
    let automatic_ids = object_ids(&automatic);
    assert_eq!(
        explicit_ids,
        ["decision-object-1", "decision-object-verified"]
    );
    assert_eq!(automatic_ids, ["decision-object-verified"]);
    // The same policy labels the asserted decision on the explicit lane and withholds
    // it from injection; the verified one is visible on both.
    let visibility = |read: &serde_json::Value, id: &str| {
        read["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["object"]["object_id"] == id)
            .map(|row| row["visibility"].as_str().unwrap().to_string())
    };
    assert_eq!(
        visibility(&explicit, "decision-object-1").as_deref(),
        Some("labeled")
    );
    assert_eq!(
        visibility(&explicit, "decision-object-verified").as_deref(),
        Some("visible")
    );
    assert_eq!(
        visibility(&automatic, "decision-object-verified").as_deref(),
        Some("visible")
    );
    assert_eq!(visibility(&automatic, "decision-object-1"), None);
    assert_eq!(explicit["known_as_of"], json!(snapshot));
    assert_eq!(automatic["known_as_of"], json!(snapshot));
    daemon.shutdown().await;
}

#[tokio::test]
async fn a_batch_check_judges_each_candidate_by_object_id_and_revision() {
    let daemon = KernelDaemon::start().await;
    daemon
        .commit("create", vec![insert_decision(1), insert_decision(2)])
        .await;
    daemon
        .commit("retire-2", vec![retire_decision("decision-object-2")])
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
    assert_eq!(response["cache_hits"], json!(0));

    // The same batch at the same tip answers from the verdict cache, identically.
    let repeated = daemon
        .eligibility(
            "local",
            vec![
                candidate("decision-object-1", 1),
                candidate("decision-object-1", 7),
            ],
        )
        .await;
    assert_eq!(verdicts(&repeated), verdicts(&response)[..2]);
    assert_eq!(repeated["cache_hits"], json!(2));
    daemon.shutdown().await;
}

#[tokio::test]
async fn a_verdict_cached_at_one_tip_is_rejudged_after_a_retirement() {
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
    assert_eq!(granted["cache_hits"], json!(0));

    daemon
        .commit("retire-1", vec![retire_decision("decision-object-1")])
        .await;

    // The projection a caller holds still lists the object at its own snapshot.
    let stale = daemon
        .read("explicit_search", Some(projected_at), None)
        .await;
    assert_eq!(object_ids(&stale), ["decision-object-1"]);
    assert_eq!(stale["known_as_of"], json!(projected_at));

    // `kernel.eligibility.batch` takes no `as_of`: it judges at the tip, where the
    // cached grant misses and the candidate is denied. commentlint: allow(JUDGE)
    let denied = daemon
        .eligibility("local", vec![candidate("decision-object-1", 1)])
        .await;
    assert_eq!(
        verdicts(&denied),
        [("decision-object-1".to_string(), "retracted".to_string())]
    );
    assert_eq!(denied["known_as_of"], json!(daemon.tip()));
    assert_eq!(denied["cache_hits"], json!(0));
    let current = daemon.read("explicit_search", None, None).await;
    assert!(object_ids(&current).is_empty());
    daemon.shutdown().await;
}
