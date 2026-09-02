use std::collections::BTreeMap;

use cache_stability::{Action, CoreState, DurabilityClass, FrozenUnit, PassInput};
use serde::Deserialize;
use serde_json::Value;

const GOLDEN: &str = include_str!("golden/cache-stability-golden-vectors.json");

#[derive(Debug, Deserialize)]
struct GoldenFile {
    schema_version: u32,
    vectors: Vec<Vector>,
}

#[derive(Debug, Deserialize)]
struct Vector {
    name: String,
    #[serde(default)]
    layer: String,
    initial_state: InitialState,
    passes: Vec<Pass>,
}

#[derive(Debug, Deserialize)]
struct InitialState {
    #[serde(default)]
    version: u64,
    boundary_id: String,
    frozen_units: Vec<FrozenUnit>,
    #[serde(default)]
    pending_changes: Vec<FrozenUnit>,
}

#[derive(Debug, Deserialize)]
struct Pass {
    signal: Value,
    boundary_present: String,
    expect_action: Action,
    #[serde(default)]
    new_boundary_id: Option<String>,
    #[serde(default)]
    reconcile_pending: bool,
    #[serde(default)]
    expect_frozen_set_delta: Vec<FrozenUnit>,
    #[serde(default)]
    run_started: bool,
}

fn pass_to_input(pass: &Pass, pending_keys: &[String]) -> PassInput {
    let run_started = pass.run_started
        || pass
            .signal
            .get("kind")
            .and_then(Value::as_str)
            .is_some_and(|k| k == "run-started");

    let mut input = PassInput::new(pass.expect_action, pass.boundary_present.clone());
    input.new_boundary_id = pass.new_boundary_id.clone();
    input.run_started = run_started;

    match pass.expect_action {
        Action::SoftPlus => {
            // A `drop-queued` defer accumulates the unit while the prefix replays frozen bytes.
            // The later HARD expects the queued bytes in its delta.
            if pass
                .signal
                .get("kind")
                .and_then(Value::as_str)
                .is_some_and(|k| k == "drop-queued")
            {
                input.queued = vec![FrozenUnit {
                    key: "drop1".into(),
                    kind: "drop".into(),
                    frozen_payload: "[dropped 1]".into(),
                    durability_class: cache_stability::DurabilityClass::Lineage,
                    reset_rule: "survive + advance-only-merge, never reset".into(),
                }];
            }
        }
        Action::Soft => {
            input.rendered_units = pass.expect_frozen_set_delta.clone();
        }
        Action::Hard => {
            // Withhold queued units so the core must supply them from pending_changes.
            input.rendered_units = pass
                .expect_frozen_set_delta
                .iter()
                .filter(|u| !pending_keys.contains(&u.key))
                .cloned()
                .collect();
        }
    }
    input
}

#[test]
fn golden_fixture_is_schema_v3_with_eleven_vectors() {
    let file: GoldenFile = serde_json::from_str(GOLDEN).expect("golden fixture parses");
    assert_eq!(file.schema_version, 3, "pinned to schema_version 3");
    assert_eq!(
        file.vectors.len(),
        11,
        "11 vectors (8 mechanics + V9 durability + V10 coverage-extending SOFT + V11 never-minted boundary)"
    );
}

#[test]
fn core_state_schema_v3_empty_wire_format_is_stable() {
    let empty = CoreState::empty();
    assert_eq!(
        serde_json::to_value(&empty).expect("empty state serializes"),
        serde_json::json!({
            "version": 0,
            "boundary_id": "",
            "frozen_units": [],
            "pending_changes": [],
            "reconcile_pending": false
        })
    );
    let schema_v3_minimal: CoreState = serde_json::from_value(serde_json::json!({
        "version": 0,
        "boundary_id": "",
        "frozen_units": []
    }))
    .expect("minimal schema-v3 state deserializes");
    assert_eq!(schema_v3_minimal, empty);

    let populated = CoreState {
        version: 7,
        boundary_id: "boundary-7".into(),
        frozen_units: vec![FrozenUnit {
            key: "unit-1".into(),
            kind: "summary".into(),
            frozen_payload: "frozen bytes".into(),
            durability_class: DurabilityClass::Episode,
            reset_rule: "reset on next episode".into(),
        }],
        pending_changes: vec![],
        reconcile_pending: true,
    };
    assert_eq!(
        serde_json::to_value(&populated).expect("populated state serializes"),
        serde_json::json!({
            "version": 7,
            "boundary_id": "boundary-7",
            "frozen_units": [{
                "key": "unit-1",
                "kind": "summary",
                "frozen_payload": "frozen bytes",
                "durability_class": "episode",
                "reset_rule": "reset on next episode"
            }],
            "pending_changes": [],
            "reconcile_pending": true
        })
    );
}

#[test]
fn all_golden_vectors_pass() {
    let file: GoldenFile = serde_json::from_str(GOLDEN).expect("golden fixture parses");

    for vector in &file.vectors {
        run_vector(vector);
    }
}

fn run_vector(vector: &Vector) {
    let mut state = CoreState {
        version: vector.initial_state.version,
        boundary_id: vector.initial_state.boundary_id.clone(),
        frozen_units: vector.initial_state.frozen_units.clone(),
        pending_changes: vector.initial_state.pending_changes.clone(),
        reconcile_pending: false,
    };

    // Per-unit replay map: key -> the frozen_payload that must reproduce verbatim on defer.
    let mut frozen_seen: BTreeMap<String, String> = state
        .frozen_units
        .iter()
        .map(|u| (u.key.clone(), u.frozen_payload.clone()))
        .collect();

    for (i, pass) in vector.passes.iter().enumerate() {
        let pending_keys: Vec<String> = state
            .pending_changes
            .iter()
            .map(|u| u.key.clone())
            .collect();
        let input = pass_to_input(pass, &pending_keys);
        let before_bytes = state.cached_prefix_bytes();
        let boundary_before = state.boundary_id.clone();
        let result = state.step(input).expect("version headroom");

        assert_eq!(
            result.action, pass.expect_action,
            "{}: pass {i} action mismatch",
            vector.name
        );

        let after_bytes = state.cached_prefix_bytes();

        match pass.expect_action {
            Action::SoftPlus => {
                assert_eq!(
                    result.reconcile_pending, pass.reconcile_pending,
                    "{}: pass {i} SOFT+ reconcile_pending must equal boundary-absence",
                    vector.name
                );
                assert_eq!(
                    after_bytes, before_bytes,
                    "{}: pass {i} SOFT+ must not change cached_prefix_bytes",
                    vector.name
                );
                for unit in &state.frozen_units {
                    if let Some(expected) = frozen_seen.get(&unit.key) {
                        assert_eq!(
                            &unit.frozen_payload, expected,
                            "{}: pass {i} frozen unit '{}' re-derived on defer",
                            vector.name, unit.key
                        );
                    }
                }
            }
            Action::Soft | Action::Hard => {
                for delta in &pass.expect_frozen_set_delta {
                    let stored = state
                        .frozen_units
                        .iter()
                        .find(|u| u.key == delta.key)
                        .unwrap_or_else(|| {
                            panic!(
                                "{}: pass {i} bust delta '{}' not in frozen set",
                                vector.name, delta.key
                            )
                        });
                    assert_eq!(
                        stored, delta,
                        "{}: pass {i} bust unit '{}' bytes diverge from fixture",
                        vector.name, delta.key
                    );
                    frozen_seen.insert(delta.key.clone(), delta.frozen_payload.clone());
                }
                if let Some(b) = &pass.new_boundary_id {
                    assert_eq!(
                        &state.boundary_id, b,
                        "{}: pass {i} bust must advance the boundary to the fixture id",
                        vector.name
                    );
                } else {
                    assert_eq!(
                        state.boundary_id, boundary_before,
                        "{}: pass {i} bust without new_boundary_id must leave the anchor unchanged",
                        vector.name
                    );
                }
                assert_eq!(
                    result.reconcile_pending, pass.reconcile_pending,
                    "{}: pass {i} bust reconcile_pending mismatch",
                    vector.name
                );
                if pass.expect_action == Action::Hard {
                    assert!(
                        state.pending_changes.is_empty(),
                        "{}: pass {i} HARD must drain deferred work",
                        vector.name
                    );
                }
            }
        }
    }
}

#[test]
fn cross_episode_lineage_reproduces_byte_identical() {
    let file: GoldenFile = serde_json::from_str(GOLDEN).expect("golden fixture parses");
    let v9 = file
        .vectors
        .iter()
        .find(|v| v.name.starts_with("V9"))
        .expect("V9 present");
    assert_eq!(v9.layer, "durability");

    let mut state = CoreState {
        version: v9.initial_state.version,
        boundary_id: v9.initial_state.boundary_id.clone(),
        frozen_units: v9.initial_state.frozen_units.clone(),
        pending_changes: v9.initial_state.pending_changes.clone(),
        reconcile_pending: false,
    };

    let pre_episode = state.cached_prefix_bytes();
    for pass in &v9.passes {
        let run_started = pass
            .signal
            .get("kind")
            .and_then(Value::as_str)
            .is_some_and(|k| k == "run-started");
        let pending_keys: Vec<String> = state
            .pending_changes
            .iter()
            .map(|u| u.key.clone())
            .collect();
        let before = state.cached_prefix_bytes();
        state
            .step(pass_to_input(pass, &pending_keys))
            .expect("version headroom");
        if run_started {
            assert_eq!(
                state.cached_prefix_bytes(),
                before,
                "RunStarted must not bust the cached prefix (lineage byte-identical)"
            );
        }
    }
    // The whole lineage reproduced byte-identical across the episode boundary.
    assert_eq!(
        state.cached_prefix_bytes(),
        pre_episode,
        "lineage units must reproduce byte-identical across the episode boundary"
    );
}
