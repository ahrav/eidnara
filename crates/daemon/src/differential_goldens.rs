use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::transform::{TransformRequest, TransformResponse};
use crate::wire::{IngressMessage, WireMessage};

use super::{NativeAttachmentCache, NativeCacheKeyMode, attach_native_messages_incremental};

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
fn dg_goldens_exercise_incremental_native_differential_mode() {
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
            "full_array_fingerprint": format!("fp-{}", case.id),
        }))
        .expect("DG native transform request");
        let cache = Mutex::new(NativeAttachmentCache::new(1024 * 1024));
        let mut first =
            TransformResponse::passthrough(served.clone(), request.full_array_fingerprint.clone());
        attach_native_messages_incremental(
            &mut first,
            &request,
            0,
            &BTreeMap::new(),
            None,
            None,
            false,
            None,
            0,
            &cache,
            NativeCacheKeyMode::Normal,
        );
        let mut replay =
            TransformResponse::passthrough(served.clone(), request.full_array_fingerprint.clone());
        let stats = attach_native_messages_incremental(
            &mut replay,
            &request,
            0,
            &BTreeMap::new(),
            None,
            None,
            false,
            None,
            0,
            &cache,
            NativeCacheKeyMode::Normal,
        );
        assert_eq!(
            serde_json::to_vec(&first.native_messages).unwrap(),
            serde_json::to_vec(&replay.native_messages).unwrap(),
            "native replay drift in {}",
            case.id
        );
        assert_eq!(stats.encoded_messages, 0, "{} missed cache", case.id);
        assert_eq!(stats.reused_messages, served.len(), "{} prefix", case.id);

        let mut appended = request.messages.clone();
        appended.push(IngressMessage {
            mid: format!("dg-{}-tail", case.id),
            ordinal: appended
                .last()
                .map_or(1, |message| message.ordinal.saturating_add(1)),
            ck: WireMessage::synthetic_user_text("differential projection tail"),
        });
        let projection =
            crate::wire::project_messages(&request.messages).expect("DG projection must succeed");
        let incremental = crate::wire::project_messages_incremental(
            &appended,
            &projection,
            request.messages.len(),
        )
        .expect("DG incremental projection must succeed");
        crate::transform::assert_prefix_projection_equivalent(&incremental, &appended)
            .expect("DG full projection must succeed");
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
