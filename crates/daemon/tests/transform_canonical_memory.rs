//! The transform composes its project-memory block from canonical kernel rows
//! read through the daemon's own store, and pins the snapshot it composed from.

mod support;

use daemon::kernel_route_fixtures::{admission, intent};
use kernel::{
    DecisionPayload, DecisionSpec, Envelope, EventKind, KernelError, KernelStore,
    ObservationPayload, ObservationSpec, ScopeSpec, ScopeTermSpec, Sensitivity, SourceClass,
    TaintClass,
};
use serde_json::{Value, json};
use support::kernel_daemon::{DOMAIN, KernelDaemon, SESSION, insert_decision, state_kind};

fn decision_spec(object_id: &str, scope_id: &str, kind: &str, summary: &str) -> DecisionSpec {
    DecisionSpec {
        decision_id: format!("{object_id}-decision"),
        object_id: object_id.to_string(),
        domain_id: DOMAIN.to_string(),
        proposition_id: None,
        scope_id: Some(scope_id.to_string()),
        anchor_id: None,
        evidence_id: None,
        decision_kind: kind.to_string(),
        payload: DecisionPayload {
            summary: summary.to_string(),
            rationale: String::new(),
        },
        source_kind: "repo".to_string(),
        source_id: format!("{object_id}-lineage"),
        source_revision: 1,
        sensitivity: Sensitivity::Normal,
    }
}

/// Verifies `object_id` against an observation of its own lineage: the trusted
/// code observation lifts the decision to the maturity the automatic surface
/// serves.
fn verify(envelope: &mut Envelope<'_>, object_id: &str) -> Result<(), KernelError> {
    let observation = format!("{object_id}-observed");
    envelope.insert_observation(ObservationSpec {
        observation_id: observation.clone(),
        object_id: observation.clone(),
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
        source_id: format!("{object_id}-lineage"),
        source_revision: 1,
        sensitivity: Sensitivity::Normal,
    })?;
    envelope.record_admission(admission(
        object_id,
        EventKind::CodeObserved,
        Some(&observation),
        (SourceClass::TrustedLocalCode, TaintClass::CurrentCode),
    ))?;
    Ok(())
}

fn commit_verified_memory(
    store: &KernelStore,
    key: &str,
    object_id: &str,
    scope_id: &str,
    kind: &str,
    summary: &str,
) {
    store
        .commit(intent(key), |envelope| {
            envelope.insert_decision(decision_spec(object_id, scope_id, kind, summary))?;
            verify(envelope, object_id)?;
            Ok(String::new())
        })
        .unwrap();
}

fn transform_request(fingerprint: &str) -> Value {
    json!({
        "method": "transform",
        "kind": "transform",
        "v": 2,
        "serializer_profile": "owned-llmrunner",
        "session_id": SESSION,
        "render_config": "canonical-memory",
        "full_array_fingerprint": fingerprint,
        "messages": [{
            "mid": "m1",
            "ordinal": 1,
            "ck": {
                "role": "user",
                "content": [{"kind": {"type": "text", "text": "live prompt"}}],
                "meta": {"harness_id": "m1"}
            }
        }]
    })
}

fn served_text(response: &Value) -> String {
    serde_json::to_string(&response["messages"]).unwrap()
}

#[tokio::test]
async fn transform_composes_project_memory_from_canonical_rows_and_observes_absence() {
    let daemon = KernelDaemon::start().await;
    // The route-asserted decision materializes the bound project's scope and
    // stays a candidate, which the automatic surface withholds.
    let asserted = daemon.commit("asserted", vec![insert_decision(1)]).await;
    assert_eq!(state_kind(&asserted), "available");
    let scope_id = daemon.read("explicit_search", None, None).await["rows"][0]["scope_id"]
        .as_str()
        .unwrap()
        .to_string();
    let store = daemon.store();
    for (key, object_id, kind, summary) in [
        (
            "rule",
            "mem-rule",
            "PROJECT_RULES",
            "Keep the public contract.",
        ),
        (
            "soon-quarantined",
            "mem-quarantine",
            "CONSTRAINTS",
            "Soon quarantined.",
        ),
        ("soon-retired", "mem-retired", "NAMING", "Soon retired."),
        ("old", "mem-old", "ARCHITECTURE", "Old architecture note."),
        ("anti", "mem-anti", "REJECTED_APPROACH", "Shelved design."),
        ("plain-kind", "mem-kind", "memory", "Plain decision kind."),
    ] {
        commit_verified_memory(&store, key, object_id, &scope_id, kind, summary);
    }
    store
        .commit(intent("other-project"), |envelope| {
            envelope.insert_scope(ScopeSpec {
                scope_id: "scope-other".to_string(),
                object_id: "scope-other".to_string(),
                domain_id: DOMAIN.to_string(),
                source_kind: "repo".to_string(),
                source_id: "scope-other".to_string(),
                source_revision: 1,
                sensitivity: Sensitivity::Normal,
                terms: vec![ScopeTermSpec {
                    dimension: "project".to_string(),
                    operator: "exact".to_string(),
                    exact_value: Some("f".repeat(64)),
                    ..ScopeTermSpec::default()
                }],
            })?;
            envelope.insert_decision(decision_spec(
                "mem-other",
                "scope-other",
                "PROJECT_RULES",
                "Other project rule.",
            ))?;
            verify(envelope, "mem-other")?;
            Ok(String::new())
        })
        .unwrap();
    store
        .commit(intent("shape"), |envelope| {
            envelope.retire_decision("mem-retired")?;
            let mut replacement = decision_spec(
                "mem-new",
                &scope_id,
                "ARCHITECTURE",
                "New architecture note.",
            );
            replacement.source_id = "mem-old-lineage".to_string();
            replacement.source_revision = 2;
            envelope.supersede_decision("mem-old", replacement)?;
            Ok(String::new())
        })
        .unwrap();
    let tip = daemon.tip();

    let first = daemon.call(transform_request("first")).await;
    assert_eq!(first["status"], "ok", "{first}");
    assert_eq!(first["action"], "HARD", "{first}");
    let text = served_text(&first);
    assert!(text.contains("<project-memory>"), "{text}");
    assert!(
        text.contains("mem-rule: Keep the public contract."),
        "{text}"
    );
    assert!(text.contains("mem-quarantine: Soon quarantined."), "{text}");
    for absent in [
        "decision 1",
        "Soon retired.",
        "Old architecture note.",
        "Shelved design.",
        "Plain decision kind.",
        "Other project rule.",
    ] {
        assert!(!text.contains(absent), "{absent} leaked into {text}");
    }
    assert_eq!(
        first["project_memory"],
        json!({
            "kind": "canonical",
            "known_as_of": tip,
            "truncated": false,
            "revision": first["project_memory"]["revision"],
        }),
        "{first}"
    );
    assert!(first["project_memory"]["revision"].is_u64());

    // A quarantine between two passes hides the row on the next pinned read.
    store
        .commit(intent("quarantine"), |envelope| {
            envelope.record_admission(admission(
                "mem-quarantine",
                EventKind::Quarantine,
                None,
                (SourceClass::TrustedLocalCode, TaintClass::CurrentCode),
            ))?;
            Ok(String::new())
        })
        .unwrap();
    let quarantined_tip = daemon.tip();
    assert!(quarantined_tip > tip);

    let second = daemon.call(transform_request("second")).await;
    assert_eq!(second["status"], "ok", "{second}");
    assert_eq!(second["action"], "HARD", "{second}");
    assert_eq!(
        second["materialize_reason"], "project_memory_epoch",
        "{second}"
    );
    let text = served_text(&second);
    assert!(
        text.contains("mem-rule: Keep the public contract."),
        "{text}"
    );
    assert!(!text.contains("Soon quarantined."), "{text}");
    assert_eq!(second["project_memory"]["kind"], "canonical");
    assert_eq!(second["project_memory"]["known_as_of"], quarantined_tip);
    assert_eq!(second["project_memory"]["truncated"], false);
    assert_ne!(
        second["project_memory"]["revision"],
        first["project_memory"]["revision"]
    );

    // A pass with unchanged rows keeps the frozen m0.
    let third = daemon.call(transform_request("third")).await;
    assert_eq!(third["status"], "ok", "{third}");
    assert_ne!(third["action"], "HARD", "{third}");
    assert_eq!(third["project_memory"], second["project_memory"]);
    daemon.shutdown().await;
}

/// Emits `count` outbox rows in one commit, so a registered consumer that never
/// acknowledges falls behind by that many positions once they are published.
fn emit_outbox_rows(store: &KernelStore, first: i64, count: i64) {
    store
        .commit(intent(&format!("domains-{first}-{count}")), |envelope| {
            for index in first..first + count {
                envelope.insert_domain(kernel::DomainSpec {
                    domain_id: format!("lag-domain-{index}"),
                    object_id: format!("lag-object-{index}"),
                    name: format!("lag-{index}"),
                    source_kind: "repo".to_string(),
                    source_id: format!("lag-source-{index}"),
                    source_revision: index,
                    sensitivity: Sensitivity::Normal,
                })?;
            }
            Ok(String::new())
        })
        .unwrap();
}

#[tokio::test]
async fn a_lagging_consumer_withholds_the_block_and_acknowledging_restores_it() {
    let daemon = KernelDaemon::start().await;
    let asserted = daemon.commit("asserted", vec![insert_decision(1)]).await;
    assert_eq!(state_kind(&asserted), "available");
    let scope_id = daemon.read("explicit_search", None, None).await["rows"][0]["scope_id"]
        .as_str()
        .unwrap()
        .to_string();
    let store = daemon.store();
    commit_verified_memory(
        &store,
        "rule",
        "mem-rule",
        &scope_id,
        "PROJECT_RULES",
        "Keep the public contract.",
    );

    // A registered consumer that trails the published outbox by the position
    // threshold makes the tip read abstain, exactly as `kernel.read gated` does.
    let registered = store
        .commit(intent("register-indexer"), |envelope| {
            envelope.register_outbox_consumer("indexer", 1)?;
            Ok(String::new())
        })
        .unwrap()
        .commit_seq;
    store.acknowledge_outbox("indexer", registered, 1).unwrap();
    for batch in 0..4 {
        emit_outbox_rows(&store, batch * 2_500, 2_500);
    }
    let newest_boundary = store
        .pending_outbox(usize::MAX)
        .unwrap()
        .into_iter()
        .filter(|entry| entry.commit_boundary)
        .map(|entry| entry.outbox_position)
        .max()
        .unwrap();
    store
        .mark_outbox_published_through(newest_boundary, 1)
        .unwrap();

    let withheld = daemon.call(transform_request("withheld")).await;
    assert_eq!(withheld["status"], "ok", "{withheld}");
    assert_eq!(withheld["action"], "HARD", "{withheld}");
    assert!(
        !served_text(&withheld).contains("<project-memory>"),
        "{withheld}"
    );
    assert_eq!(
        withheld["project_memory"],
        json!({"kind": "withheld", "state": "abstained"}),
        "{withheld}"
    );

    // Once the consumer catches up the same pass serves the block.
    store
        .acknowledge_outbox("indexer", daemon.tip(), 1)
        .unwrap();
    let served = daemon.call(transform_request("served")).await;
    assert_eq!(served["status"], "ok", "{served}");
    assert_eq!(served["action"], "HARD", "{served}");
    assert_eq!(
        served["materialize_reason"], "project_memory_epoch",
        "{served}"
    );
    assert!(
        served_text(&served).contains("mem-rule: Keep the public contract."),
        "{served}"
    );
    assert_eq!(served["project_memory"]["kind"], "canonical");
    daemon.shutdown().await;
}

/// The transform wire tolerates unknown top-level keys (hosts send `method`), so
/// a retired `claim_lane` field is ignored rather than rejected, and changes
/// nothing about the composition.
#[tokio::test]
async fn a_retired_claim_lane_field_is_ignored() {
    let daemon = KernelDaemon::start().await;
    let plain = daemon.call(transform_request("plain")).await;
    assert_eq!(plain["status"], "ok", "{plain}");
    let mut with_lane = transform_request("plain");
    with_lane["claim_lane"] = json!({"enabled": true, "snapshot_vector": null});
    let ignored = daemon.call(with_lane).await;
    assert_eq!(ignored["status"], "ok", "{ignored}");
    assert_eq!(ignored["project_memory"], plain["project_memory"]);
    assert_eq!(served_text(&ignored), served_text(&plain));
    daemon.shutdown().await;
}
