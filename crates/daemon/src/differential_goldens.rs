use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

use crate::transform::{TransformRequest, TransformResponse};
use crate::wire::{IngressMessage, WireMessage};

use super::{NativeOutput, attach_native_messages_with_tags};

#[derive(Debug, Deserialize)]
struct Golden {
    schema: u32,
    provenance: Provenance,
    cases: Vec<GoldenCase>,
}

#[derive(Debug, Deserialize)]
struct Provenance {
    generator_version: String,
    input_sha256: String,
}

#[derive(Debug, Deserialize)]
struct GoldenCase {
    id: String,
    family: String,
    input: Value,
    expected: Expected,
}

#[derive(Debug, Deserialize)]
struct Expected {
    status: String,
    action: String,
    decision: String,
    wire: Vec<Value>,
}

#[test]
fn dg_goldens_match_ts_wire_surface_and_gate_labels() {
    let golden: Golden = serde_json::from_str(include_str!("../testdata/differential-golden.json"))
        .expect("parse differential golden");
    assert_eq!(golden.schema, 1);
    assert_eq!(golden.provenance.generator_version, "dg-reference-v1");
    assert_eq!(golden.provenance.input_sha256.len(), 64);
    assert_eq!(golden.cases.len(), 3);

    for case in &golden.cases {
        let input_wire = case.input["messages"]
            .as_array()
            .expect("every DG input has messages");
        let parsed: Vec<WireMessage> = serde_json::from_value(Value::Array(input_wire.clone()))
            .expect("DG input must be canonical CK wire");
        let rust_wire = parsed
            .iter()
            .map(|message| serde_json::to_value(message).expect("serialize CK wire"))
            .collect::<Vec<_>>();
        assert_eq!(rust_wire, case.expected.wire, "wire drift in {}", case.id);
        assert!(!case.family.is_empty());
        assert_eq!(
            case.expected.status, "ok",
            "unexpected status in {}",
            case.id
        );
        assert!(!case.expected.action.is_empty());
        assert!(!case.expected.decision.is_empty());
    }
}

#[test]
fn dg_golden_vacuity_guard_rejects_one_byte_fixture_perturbation_per_family() {
    let golden: Golden = serde_json::from_str(include_str!("../testdata/differential-golden.json"))
        .expect("parse differential golden");
    let mut observed = 0;
    for case in &golden.cases {
        let mut perturbed = case.input["messages"].clone();
        let mut mutated_text = None;
        if let Some(message) = perturbed
            .as_array_mut()
            .and_then(|messages| messages.first_mut())
            .and_then(|message| message.get_mut("content"))
            .and_then(Value::as_array_mut)
            .and_then(|parts| parts.first_mut())
            .and_then(|part| part.get_mut("kind"))
            .and_then(|kind| kind.get_mut("text"))
            && let Some(text) = message.as_str()
        {
            mutated_text = Some(format!("{text}x"));
            *message = Value::String(mutated_text.clone().expect("mutation text"));
        }
        if mutated_text.is_none() {
            let bytes = serde_json::to_vec(&perturbed).expect("serialize fixture");
            perturbed = Value::String(String::from_utf8_lossy(&bytes).to_string() + "x");
        }
        assert_ne!(
            perturbed,
            Value::Array(case.expected.wire.clone()),
            "{} accepted a one-byte mutation",
            case.id
        );
        observed += 1;
    }
    assert_eq!(observed, 3, "every DG family needs a vacuity mutation");
}

#[test]
fn dg_goldens_encode_native_output_deterministically() {
    let golden: Golden = serde_json::from_str(include_str!("../testdata/differential-golden.json"))
        .expect("parse differential golden");
    for case in &golden.cases {
        let wire = case
            .input
            .get("messages")
            .and_then(Value::as_array)
            .expect("every DG input has messages");
        let served: Vec<WireMessage> =
            serde_json::from_value(Value::Array(wire.clone())).expect("canonical DG CK wire");
        let ingress = served
            .iter()
            .enumerate()
            .map(|(index, message)| IngressMessage {
                mid: message
                    .meta
                    .harness_id
                    .clone()
                    .unwrap_or_else(|| format!("dg-{index}")),
                ordinal: index as u64 + 1,
                ck: message.clone(),
            })
            .collect::<Vec<_>>();
        let request: TransformRequest = serde_json::from_value(json!({
            "kind": "transform",
            "v": 2,
            "serializer_profile": "opencode-aisdk",
            "session_id": format!("dg-native-{}", case.id),
            "render_config": "dg",
            "serve_native": true,
            "messages": ingress,
        }))
        .expect("DG native transform request");
        let encode = || {
            let mut response = TransformResponse::passthrough(served.clone());
            attach_native_messages_with_tags(
                &mut response,
                &request,
                0,
                &BTreeMap::new(),
                None,
                None,
                false,
            );
            NativeOutput::measure(response.native_messages.expect("native output"))
        };
        let (first, replay) = (encode(), encode());
        assert!(!first.values.is_empty(), "{}", case.id);
        assert_eq!(
            serde_json::to_vec(&first.values).unwrap(),
            serde_json::to_vec(&replay.values).unwrap(),
            "native replay drift in {}",
            case.id
        );
        assert_eq!(
            first.wire_lens,
            first
                .values
                .iter()
                .map(|value| serde_json::to_vec(value).unwrap().len())
                .collect::<Vec<_>>(),
            "native wire lengths in {}",
            case.id
        );
    }
}

#[cfg(test)]
mod fixture_builder_tests {
    use super::super::test_support::FixtureBuilder;

    #[test]
    fn builders_cover_all_in_process_facade_shapes() {
        for fixture in [
            FixtureBuilder::session_with_boundary(),
            FixtureBuilder::tagged_session(),
            FixtureBuilder::frozen_reductions(),
            FixtureBuilder::synthetic_todo_armed(),
        ] {
            assert_eq!(fixture.handle_transform()["kind"], "transform");
            assert_eq!(fixture.call_transform()["session_id"], fixture.session_id);
        }
    }
}
