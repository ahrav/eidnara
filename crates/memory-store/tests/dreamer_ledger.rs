//! The Dreamer receipt ledger: `ABSENT` to `IN_PROGRESS` to `COMPLETE`,
//! generation-fenced takeover, row-predicate-guarded transitions, and a
//! request digest that does not depend on map order.

use memory_store::MemoryStore;
use memory_store::dreamer_ledger::{
    DreamerAttemptSpec, DreamerBeginOutcome, DreamerReceiptBinding, DreamerReceiptKey,
    DreamerReceiptState, DreamerTerminalKind, DreamerTransition, dreamer_request_digest,
};
use serde_json::json;
use storage::StorageDescriptor;

const PROJECT: &str = "git:project";
const PRODUCER: &str = "dreamer.run_task";
const OPERATION_KEY: &str = "command-1";
const INCARNATION: &str = "context-store-0123456789abcdef";
const SYSTEM_PROMPT_HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn descriptor(dir: &std::path::Path) -> StorageDescriptor {
    MemoryStore::test_descriptor(dir, "eidnara-dreamer-ledger-test")
}

fn key() -> DreamerReceiptKey<'static> {
    DreamerReceiptKey {
        project: PROJECT,
        producer: PRODUCER,
        operation_key: OPERATION_KEY,
    }
}

fn request(prompt: &str) -> serde_json::Value {
    json!({
        "task": "classify",
        "prompt_body": prompt,
        "items": ["mcm_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],
        "model_chain": ["prov/model-a", "prov/model-b"],
    })
}

fn binding(prompt: &str) -> DreamerReceiptBinding {
    DreamerReceiptBinding {
        database_incarnation_id: INCARNATION.to_string(),
        authority_generation: 3,
        request_digest: dreamer_request_digest(&request(prompt)).unwrap(),
        ledger_session: "ses-1".to_string(),
        command_id: "cmd-1".to_string(),
    }
}

fn attempt(index: u32, model: &str) -> DreamerAttemptSpec<'_> {
    DreamerAttemptSpec {
        attempt_index: index,
        model,
        prompt_template_version: 1,
        system_prompt_hash: SYSTEM_PROMPT_HASH,
        schema_version: 1,
        child_session: "eidnara-dreamer:classify:0123456789abcdef",
    }
}

#[test]
fn a_receipt_moves_from_absent_through_in_progress_to_complete_and_replays() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
    assert_eq!(store.lookup_dreamer_receipt(key()).unwrap(), None);

    let begun = store
        .begin_dreamer_receipt(key(), &binding("prompt"), 10)
        .unwrap();
    assert_eq!(begun, DreamerBeginOutcome::Begun { generation: 1 });
    // A second begin sees the in-progress receipt and never a terminal answer.
    assert_eq!(
        store
            .begin_dreamer_receipt(key(), &binding("prompt"), 11)
            .unwrap(),
        DreamerBeginOutcome::InProgress { generation: 1 }
    );
    assert_eq!(
        store.lookup_dreamer_receipt(key()).unwrap().unwrap().state,
        DreamerReceiptState::InProgress { generation: 1 }
    );

    assert_eq!(
        store
            .begin_dreamer_attempt(key(), 1, &attempt(0, "prov/model-a"), 12)
            .unwrap(),
        DreamerTransition::Applied
    );
    assert_eq!(
        store
            .record_dreamer_run_handle(key(), 1, 0, "run-1")
            .unwrap(),
        DreamerTransition::Applied
    );
    assert_eq!(
        store
            .finish_dreamer_attempt(key(), 1, 0, DreamerTerminalKind::Complete, 13)
            .unwrap(),
        DreamerTransition::Applied
    );
    // An attempt ends once.
    assert_eq!(
        store
            .finish_dreamer_attempt(key(), 1, 0, DreamerTerminalKind::Failed, 14)
            .unwrap(),
        DreamerTransition::Fenced
    );

    let result = json!({"ok": true, "manifest_text": "<manifest/>"}).to_string();
    assert_eq!(
        store
            .complete_dreamer_receipt(key(), 1, DreamerTerminalKind::Complete, &result, 15)
            .unwrap(),
        DreamerTransition::Applied
    );
    // A completed receipt never completes again, and a later begin replays it.
    assert_eq!(
        store
            .complete_dreamer_receipt(key(), 1, DreamerTerminalKind::Failed, "{}", 16)
            .unwrap(),
        DreamerTransition::Fenced
    );
    assert_eq!(
        store
            .begin_dreamer_receipt(key(), &binding("prompt"), 17)
            .unwrap(),
        DreamerBeginOutcome::Complete {
            generation: 1,
            terminal_kind: DreamerTerminalKind::Complete,
            result_json: result.clone(),
        }
    );
    // The same identity under a different digest is a conflict that writes nothing.
    let stored_digest = binding("prompt").request_digest;
    assert_eq!(
        store
            .begin_dreamer_receipt(key(), &binding("other prompt"), 18)
            .unwrap(),
        DreamerBeginOutcome::DigestConflict { stored_digest }
    );
    let receipt = store.lookup_dreamer_receipt(key()).unwrap().unwrap();
    assert_eq!(receipt.updated_at_ms, 15);
    assert_eq!(receipt.binding.ledger_session, "ses-1");
    assert_eq!(receipt.binding.command_id, "cmd-1");

    let attempts = store.list_dreamer_attempts(key()).unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].model, "prov/model-a");
    assert_eq!(attempts[0].run_handle.as_deref(), Some("run-1"));
    assert_eq!(
        attempts[0].terminal_kind,
        Some(DreamerTerminalKind::Complete)
    );
    assert_eq!(attempts[0].terminal_at_ms, Some(13));
    assert_eq!(store.count_dreamer_attempts(PROJECT, 0).unwrap(), 1);
    assert_eq!(store.count_dreamer_attempts(PROJECT, 13).unwrap(), 0);
    assert_eq!(store.count_dreamer_attempts("git:other", 0).unwrap(), 0);
}

#[test]
fn a_different_incarnation_or_authority_generation_is_a_binding_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
    store
        .begin_dreamer_receipt(key(), &binding("prompt"), 1)
        .unwrap();
    let mut other_generation = binding("prompt");
    other_generation.authority_generation = 4;
    assert_eq!(
        store
            .begin_dreamer_receipt(key(), &other_generation, 2)
            .unwrap(),
        DreamerBeginOutcome::BindingMismatch {
            field: "authority_generation",
            expected: "3".to_string(),
            found: "4".to_string(),
        }
    );
    let mut other_incarnation = binding("prompt");
    other_incarnation.database_incarnation_id = "context-store-other".to_string();
    assert!(matches!(
        store
            .begin_dreamer_receipt(key(), &other_incarnation, 3)
            .unwrap(),
        DreamerBeginOutcome::BindingMismatch {
            field: "database_incarnation_id",
            ..
        }
    ));
}

#[test]
fn reopening_the_store_preserves_an_in_progress_receipt_and_its_dispatch_marker() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
        store
            .begin_dreamer_receipt(key(), &binding("prompt"), 1)
            .unwrap();
        store
            .begin_dreamer_attempt(key(), 1, &attempt(0, "prov/model-a"), 2)
            .unwrap();
        store
            .record_dreamer_run_handle(key(), 1, 0, "run-1")
            .unwrap();
    }
    let reopened = MemoryStore::open(&descriptor(dir.path())).unwrap();
    assert_eq!(
        reopened
            .begin_dreamer_receipt(key(), &binding("prompt"), 3)
            .unwrap(),
        DreamerBeginOutcome::InProgress { generation: 1 }
    );
    let attempts = reopened.list_dreamer_attempts(key()).unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].dispatched_at_ms, 2);
    assert_eq!(attempts[0].run_handle.as_deref(), Some("run-1"));
    assert_eq!(attempts[0].terminal_kind, None);
}

#[test]
fn a_higher_generation_takes_over_and_the_predecessor_is_fenced_on_every_transition() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
    store
        .begin_dreamer_receipt(key(), &binding("prompt"), 1)
        .unwrap();
    store
        .begin_dreamer_attempt(key(), 1, &attempt(0, "prov/model-a"), 2)
        .unwrap();

    assert_eq!(
        store.take_over_dreamer_receipt(key(), 1, 3).unwrap(),
        DreamerTransition::Applied
    );
    // A stale takeover naming the old generation is fenced.
    assert_eq!(
        store.take_over_dreamer_receipt(key(), 1, 4).unwrap(),
        DreamerTransition::Fenced
    );
    assert_eq!(
        store.lookup_dreamer_receipt(key()).unwrap().unwrap().state,
        DreamerReceiptState::InProgress { generation: 2 }
    );

    // The predecessor's completion, failure record, attempt, and session
    // release each match no row and write nothing.
    assert_eq!(
        store
            .complete_dreamer_receipt(key(), 1, DreamerTerminalKind::Complete, "{}", 5)
            .unwrap(),
        DreamerTransition::Fenced
    );
    assert_eq!(
        store
            .complete_dreamer_receipt(key(), 1, DreamerTerminalKind::Failed, "{}", 6)
            .unwrap(),
        DreamerTransition::Fenced
    );
    assert_eq!(
        store
            .begin_dreamer_attempt(key(), 1, &attempt(1, "prov/model-b"), 7)
            .unwrap(),
        DreamerTransition::Fenced
    );
    assert_eq!(
        store
            .release_dreamer_attempt_session(key(), 1, 0, 8)
            .unwrap(),
        DreamerTransition::Fenced
    );
    let receipt = store.lookup_dreamer_receipt(key()).unwrap().unwrap();
    assert_eq!(
        receipt.state,
        DreamerReceiptState::InProgress { generation: 2 }
    );
    assert_eq!(receipt.updated_at_ms, 3);
    let attempts = store.list_dreamer_attempts(key()).unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].session_released_at_ms, None);

    // The successor proceeds: its attempt row, terminal, release, and completion land.
    assert_eq!(
        store
            .begin_dreamer_attempt(key(), 2, &attempt(0, "prov/model-a"), 9)
            .unwrap(),
        DreamerTransition::Applied
    );
    assert_eq!(
        store
            .finish_dreamer_attempt(key(), 2, 0, DreamerTerminalKind::Complete, 10)
            .unwrap(),
        DreamerTransition::Applied
    );
    assert_eq!(
        store
            .release_dreamer_attempt_session(key(), 2, 0, 11)
            .unwrap(),
        DreamerTransition::Applied
    );
    assert_eq!(
        store
            .release_dreamer_attempt_session(key(), 2, 0, 12)
            .unwrap(),
        DreamerTransition::Fenced
    );
    assert_eq!(
        store
            .complete_dreamer_receipt(key(), 2, DreamerTerminalKind::Complete, "{\"ok\":true}", 13)
            .unwrap(),
        DreamerTransition::Applied
    );
    // Once complete, no generation can take the receipt over.
    assert_eq!(
        store.take_over_dreamer_receipt(key(), 2, 14).unwrap(),
        DreamerTransition::Fenced
    );
    assert_eq!(store.count_dreamer_attempts(PROJECT, 0).unwrap(), 2);
}

#[test]
fn the_request_digest_ignores_map_insertion_order_and_pins_the_protocol() {
    let ordered = json!({
        "task": "classify",
        "prompt_body": "p",
        "items": ["a", "b"],
        "model_chain": ["prov/x"],
        "nested": {"alpha": 1, "beta": [1, 2, {"gamma": true, "delta": null}]},
    });
    let permuted = json!({
        "nested": {"beta": [1, 2, {"delta": null, "gamma": true}], "alpha": 1},
        "model_chain": ["prov/x"],
        "items": ["a", "b"],
        "prompt_body": "p",
        "task": "classify",
    });
    assert_eq!(
        dreamer_request_digest(&ordered).unwrap(),
        dreamer_request_digest(&permuted).unwrap()
    );
    // Array order is part of the request; the model chain is ordered.
    let reordered_chain = json!({
        "task": "classify",
        "prompt_body": "p",
        "items": ["b", "a"],
        "model_chain": ["prov/x"],
        "nested": {"alpha": 1, "beta": [1, 2, {"gamma": true, "delta": null}]},
    });
    assert_ne!(
        dreamer_request_digest(&ordered).unwrap(),
        dreamer_request_digest(&reordered_chain).unwrap()
    );
    // The protocol prefix keeps a Dreamer digest distinct from a claim digest
    // over the same bytes.
    assert_ne!(
        dreamer_request_digest(&ordered).unwrap(),
        context_core::claim_operation::compute_claim_operation_request_digest(&ordered).unwrap()
    );
    assert_eq!(dreamer_request_digest(&ordered).unwrap().len(), 64);
}
