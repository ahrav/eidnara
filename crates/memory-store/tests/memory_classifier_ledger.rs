//! The MemoryClassifier receipt ledger: `ABSENT` to `IN_PROGRESS` to `COMPLETE`,
//! generation-fenced takeover, row-predicate-guarded transitions, and a
//! request digest that does not depend on map order.

use memory_store::MemoryStore;
use memory_store::memory_classifier_ledger::{
    MemoryClassifierAttemptSpec, MemoryClassifierBeginOutcome, MemoryClassifierReceiptBinding,
    MemoryClassifierReceiptKey, MemoryClassifierReceiptState, MemoryClassifierTerminalKind,
    MemoryClassifierTransition, memory_classifier_request_digest,
};
use serde_json::json;
use storage::StorageDescriptor;

const PROJECT: &str = "git:project";
const PRODUCER: &str = "memory_classifier.run_task";
const OPERATION_KEY: &str = "command-1";
const INCARNATION: &str = "context-store-0123456789abcdef";
const SYSTEM_PROMPT_HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn descriptor(dir: &std::path::Path) -> StorageDescriptor {
    MemoryStore::test_descriptor(dir, "eidnara-memory_classifier-ledger-test")
}

fn key() -> MemoryClassifierReceiptKey<'static> {
    MemoryClassifierReceiptKey {
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

fn binding(prompt: &str) -> MemoryClassifierReceiptBinding {
    MemoryClassifierReceiptBinding {
        database_incarnation_id: INCARNATION.to_string(),
        authority_generation: 3,
        request_digest: memory_classifier_request_digest(&request(prompt)).unwrap(),
        ledger_session: "ses-1".to_string(),
        command_id: "cmd-1".to_string(),
    }
}

fn attempt(index: u32, model: &str) -> MemoryClassifierAttemptSpec<'_> {
    MemoryClassifierAttemptSpec {
        attempt_index: index,
        model,
        prompt_template_version: 1,
        system_prompt_hash: SYSTEM_PROMPT_HASH,
        schema_version: 1,
        child_session: "eidnara-memory_classifier:classify:0123456789abcdef",
        project_root: "/repo",
        harness: "pi",
    }
}

#[test]
fn a_receipt_moves_from_absent_through_in_progress_to_complete_and_replays() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
    assert_eq!(store.lookup_memory_classifier_receipt(key()).unwrap(), None);

    let begun = store
        .begin_memory_classifier_receipt(key(), &binding("prompt"), 10)
        .unwrap();
    assert_eq!(begun, MemoryClassifierBeginOutcome::Begun { generation: 1 });
    // A second begin sees the in-progress receipt and never a terminal answer.
    assert_eq!(
        store
            .begin_memory_classifier_receipt(key(), &binding("prompt"), 11)
            .unwrap(),
        MemoryClassifierBeginOutcome::InProgress { generation: 1 }
    );
    assert_eq!(
        store
            .lookup_memory_classifier_receipt(key())
            .unwrap()
            .unwrap()
            .state,
        MemoryClassifierReceiptState::InProgress { generation: 1 }
    );

    assert_eq!(
        store
            .begin_memory_classifier_attempt(key(), 1, &attempt(0, "prov/model-a"), 12)
            .unwrap(),
        MemoryClassifierTransition::Applied
    );
    // Repeating the same attempt beginning is fenced, not a primary-key error.
    assert_eq!(
        store
            .begin_memory_classifier_attempt(key(), 1, &attempt(0, "prov/model-a"), 12)
            .unwrap(),
        MemoryClassifierTransition::Fenced
    );
    assert_eq!(
        store.list_memory_classifier_attempts(key()).unwrap().len(),
        1
    );
    assert_eq!(
        store
            .record_memory_classifier_run_handle(key(), 1, 0, "run-1")
            .unwrap(),
        MemoryClassifierTransition::Applied
    );
    assert_eq!(
        store
            .record_memory_classifier_run_handle(key(), 1, 0, "replacement-run")
            .unwrap(),
        MemoryClassifierTransition::Fenced
    );
    assert_eq!(
        store
            .release_memory_classifier_attempt_session(key(), 1, 0, 12)
            .unwrap(),
        MemoryClassifierTransition::Fenced
    );
    assert_eq!(
        store.list_memory_classifier_attempts(key()).unwrap()[0].session_released_at_ms,
        None
    );
    assert_eq!(
        store
            .finish_memory_classifier_attempt(
                key(),
                1,
                0,
                MemoryClassifierTerminalKind::Complete,
                13
            )
            .unwrap(),
        MemoryClassifierTransition::Applied
    );
    // An attempt ends once.
    assert_eq!(
        store
            .finish_memory_classifier_attempt(key(), 1, 0, MemoryClassifierTerminalKind::Failed, 14)
            .unwrap(),
        MemoryClassifierTransition::Fenced
    );

    let result = json!({"ok": true, "manifest_text": "<manifest/>"}).to_string();
    assert_eq!(
        store
            .complete_memory_classifier_receipt(
                key(),
                1,
                MemoryClassifierTerminalKind::Complete,
                &result,
                15
            )
            .unwrap(),
        MemoryClassifierTransition::Applied
    );
    // A completed receipt never completes again, and a later begin replays it.
    assert_eq!(
        store
            .complete_memory_classifier_receipt(
                key(),
                1,
                MemoryClassifierTerminalKind::Failed,
                "{}",
                16
            )
            .unwrap(),
        MemoryClassifierTransition::Fenced
    );
    assert_eq!(
        store
            .begin_memory_classifier_receipt(key(), &binding("prompt"), 17)
            .unwrap(),
        MemoryClassifierBeginOutcome::Complete {
            generation: 1,
            terminal_kind: MemoryClassifierTerminalKind::Complete,
            result_json: result.clone(),
        }
    );
    // The same identity under a different digest is a conflict that writes nothing.
    let stored_digest = binding("prompt").request_digest;
    assert_eq!(
        store
            .begin_memory_classifier_receipt(key(), &binding("other prompt"), 18)
            .unwrap(),
        MemoryClassifierBeginOutcome::DigestConflict { stored_digest }
    );
    let receipt = store
        .lookup_memory_classifier_receipt(key())
        .unwrap()
        .unwrap();
    assert_eq!(receipt.updated_at_ms, 15);
    assert_eq!(receipt.binding.ledger_session, "ses-1");
    assert_eq!(receipt.binding.command_id, "cmd-1");
    assert_eq!(
        store
            .release_memory_classifier_attempt_session(key(), 1, 0, 19)
            .unwrap(),
        MemoryClassifierTransition::Applied
    );
    assert_eq!(
        store
            .release_memory_classifier_attempt_session(key(), 1, 0, 20)
            .unwrap(),
        MemoryClassifierTransition::Fenced
    );

    let attempts = store.list_memory_classifier_attempts(key()).unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].model, "prov/model-a");
    assert_eq!(attempts[0].run_handle.as_deref(), Some("run-1"));
    assert_eq!(
        attempts[0].terminal_kind,
        Some(MemoryClassifierTerminalKind::Complete)
    );
    assert_eq!(attempts[0].terminal_at_ms, Some(13));
    assert_eq!(attempts[0].session_released_at_ms, Some(19));
    assert_eq!(
        store.count_memory_classifier_attempts(PROJECT, 0).unwrap(),
        1
    );
    assert_eq!(
        store.count_memory_classifier_attempts(PROJECT, 13).unwrap(),
        0
    );
    assert_eq!(
        store
            .count_memory_classifier_attempts("git:other", 0)
            .unwrap(),
        0
    );
}

#[test]
fn an_attempt_that_was_never_sent_is_recorded_but_not_counted_as_a_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
    store
        .begin_memory_classifier_receipt(key(), &binding("prompt"), 1)
        .unwrap();
    store
        .begin_memory_classifier_attempt(key(), 1, &attempt(0, "prov/model-a"), 2)
        .unwrap();
    assert_eq!(
        store.count_memory_classifier_attempts(PROJECT, 0).unwrap(),
        1
    );
    assert_eq!(
        store
            .finish_memory_classifier_attempt(key(), 1, 0, MemoryClassifierTerminalKind::NotSent, 3)
            .unwrap(),
        MemoryClassifierTransition::Applied
    );
    // NotSent attempts remain in audit history but are excluded from dispatch counts.
    let attempts = store.list_memory_classifier_attempts(key()).unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(
        attempts[0].terminal_kind,
        Some(MemoryClassifierTerminalKind::NotSent)
    );
    assert_eq!(
        store.count_memory_classifier_attempts(PROJECT, 0).unwrap(),
        0
    );

    // A later attempt whose send outcome is unknown still counts.
    store
        .begin_memory_classifier_attempt(key(), 1, &attempt(1, "prov/model-b"), 4)
        .unwrap();
    assert_eq!(
        store
            .finish_memory_classifier_attempt(key(), 1, 1, MemoryClassifierTerminalKind::Unknown, 5)
            .unwrap(),
        MemoryClassifierTransition::Applied
    );
    assert_eq!(
        store.count_memory_classifier_attempts(PROJECT, 0).unwrap(),
        1
    );
}

#[test]
fn a_flagged_command_id_is_refused_only_when_no_receipt_exists_for_it() {
    use context_core::redaction::RedactionErrorKind;
    use memory_store::MemoryStoreError;
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
    let mut flagged = binding("prompt");
    flagged.command_id = ["password=", "memory_classifier-fixture"].concat();

    assert!(matches!(
        store.begin_memory_classifier_receipt(key(), &flagged, 1),
        Err(MemoryStoreError::Redaction(
            RedactionErrorKind::SecretDetected
        ))
    ));
    assert_eq!(store.lookup_memory_classifier_receipt(key()).unwrap(), None);

    // A retained receipt whose stored id a later detector flags still replays.
    store
        .execute_tag_sql_for_test(&format!(
            "INSERT INTO memory_classifier_receipts (
                 project, producer, operation_key, database_incarnation_id,
                 authority_generation, request_encoding_version, request_digest,
                 ledger_session, command_id, state, generation, terminal_kind,
                 result_json, created_at_ms, updated_at_ms
             ) VALUES (
                 '{PROJECT}', '{PRODUCER}', '{OPERATION_KEY}', '{INCARNATION}',
                 3, 1, '{}', 'ses-1', '{}', 'complete', 1, 'complete', '{{}}', 2, 3
             )",
            flagged.request_digest, flagged.command_id
        ))
        .unwrap();
    assert_eq!(
        store
            .begin_memory_classifier_receipt(key(), &flagged, 4)
            .unwrap(),
        MemoryClassifierBeginOutcome::Complete {
            generation: 1,
            terminal_kind: MemoryClassifierTerminalKind::Complete,
            result_json: "{}".to_string(),
        }
    );
}

#[test]
fn an_undispatched_receipt_can_be_taken_over_only_without_a_possible_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
    store
        .begin_memory_classifier_receipt(key(), &binding("prompt"), 1)
        .unwrap();
    assert_eq!(
        store
            .take_over_undispatched_memory_classifier_receipt(key(), 1, 2)
            .unwrap(),
        MemoryClassifierTransition::Applied
    );
    assert_eq!(
        store
            .lookup_memory_classifier_receipt(key())
            .unwrap()
            .unwrap()
            .state,
        MemoryClassifierReceiptState::InProgress { generation: 2 }
    );
    // Only the current generation can create an attempt.
    assert_eq!(
        store
            .begin_memory_classifier_attempt(key(), 1, &attempt(0, "prov/model-a"), 3)
            .unwrap(),
        MemoryClassifierTransition::Fenced
    );
    assert_eq!(
        store
            .begin_memory_classifier_attempt(key(), 2, &attempt(0, "prov/model-a"), 4)
            .unwrap(),
        MemoryClassifierTransition::Applied
    );
    assert_eq!(
        store
            .take_over_undispatched_memory_classifier_receipt(key(), 2, 5)
            .unwrap(),
        MemoryClassifierTransition::Fenced
    );
    store
        .finish_memory_classifier_attempt(key(), 2, 0, MemoryClassifierTerminalKind::NotSent, 6)
        .unwrap();
    assert_eq!(
        store
            .take_over_undispatched_memory_classifier_receipt(key(), 2, 7)
            .unwrap(),
        MemoryClassifierTransition::Applied
    );
    assert_eq!(
        store
            .lookup_memory_classifier_receipt(key())
            .unwrap()
            .unwrap()
            .state,
        MemoryClassifierReceiptState::InProgress { generation: 3 }
    );
    assert_eq!(
        store
            .take_over_undispatched_memory_classifier_receipt(key(), 3, 8)
            .unwrap(),
        MemoryClassifierTransition::Applied
    );
    store
        .begin_memory_classifier_attempt(key(), 4, &attempt(0, "prov/model-a"), 9)
        .unwrap();
    store
        .finish_memory_classifier_attempt(key(), 4, 0, MemoryClassifierTerminalKind::Failed, 10)
        .unwrap();
    assert_eq!(
        store
            .take_over_undispatched_memory_classifier_receipt(key(), 4, 11)
            .unwrap(),
        MemoryClassifierTransition::Fenced
    );
}

#[test]
fn a_different_incarnation_or_authority_generation_is_a_binding_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
    store
        .begin_memory_classifier_receipt(key(), &binding("prompt"), 1)
        .unwrap();
    let mut other_generation = binding("prompt");
    other_generation.authority_generation = 4;
    assert_eq!(
        store
            .begin_memory_classifier_receipt(key(), &other_generation, 2)
            .unwrap(),
        MemoryClassifierBeginOutcome::BindingMismatch {
            field: "authority_generation",
            expected: "3".to_string(),
            found: "4".to_string(),
        }
    );
    let mut other_incarnation = binding("prompt");
    other_incarnation.database_incarnation_id = "context-store-other".to_string();
    assert!(matches!(
        store
            .begin_memory_classifier_receipt(key(), &other_incarnation, 3)
            .unwrap(),
        MemoryClassifierBeginOutcome::BindingMismatch {
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
            .begin_memory_classifier_receipt(key(), &binding("prompt"), 1)
            .unwrap();
        store
            .begin_memory_classifier_attempt(key(), 1, &attempt(0, "prov/model-a"), 2)
            .unwrap();
        store
            .record_memory_classifier_run_handle(key(), 1, 0, "run-1")
            .unwrap();
    }
    let reopened = MemoryStore::open(&descriptor(dir.path())).unwrap();
    assert_eq!(
        reopened
            .begin_memory_classifier_receipt(key(), &binding("prompt"), 3)
            .unwrap(),
        MemoryClassifierBeginOutcome::InProgress { generation: 1 }
    );
    let attempts = reopened.list_memory_classifier_attempts(key()).unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].dispatched_at_ms, 2);
    assert_eq!(attempts[0].run_handle.as_deref(), Some("run-1"));
    assert_eq!(attempts[0].terminal_kind, None);
    assert_eq!(
        reopened
            .begin_memory_classifier_attempt(key(), 1, &attempt(1, "prov/model-b"), 4)
            .unwrap(),
        MemoryClassifierTransition::Applied
    );
    assert_eq!(
        reopened
            .complete_memory_classifier_receipt(
                key(),
                1,
                MemoryClassifierTerminalKind::Unknown,
                "{}",
                5
            )
            .unwrap(),
        MemoryClassifierTransition::Applied
    );
    let before = reopened.list_memory_classifier_attempts(key()).unwrap();
    assert_eq!(
        reopened
            .record_memory_classifier_run_handle(key(), 1, 1, "late-run")
            .unwrap(),
        MemoryClassifierTransition::Fenced
    );
    assert_eq!(
        reopened
            .finish_memory_classifier_attempt(
                key(),
                1,
                1,
                MemoryClassifierTerminalKind::Complete,
                6
            )
            .unwrap(),
        MemoryClassifierTransition::Fenced
    );
    assert_eq!(
        reopened.list_memory_classifier_attempts(key()).unwrap(),
        before
    );
}

#[test]
fn a_higher_generation_takes_over_and_the_predecessor_is_fenced_on_every_transition() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
    store
        .begin_memory_classifier_receipt(key(), &binding("prompt"), 1)
        .unwrap();
    store
        .begin_memory_classifier_attempt(key(), 1, &attempt(0, "prov/model-a"), 2)
        .unwrap();
    assert_eq!(
        store
            .begin_memory_classifier_attempt(key(), 1, &attempt(1, "prov/model-b"), 2)
            .unwrap(),
        MemoryClassifierTransition::Applied
    );
    assert_eq!(
        store
            .finish_memory_classifier_attempt(
                key(),
                1,
                1,
                MemoryClassifierTerminalKind::Complete,
                2
            )
            .unwrap(),
        MemoryClassifierTransition::Applied
    );

    assert_eq!(
        store
            .take_over_memory_classifier_receipt(key(), 1, 3)
            .unwrap(),
        MemoryClassifierTransition::Applied
    );
    // A stale takeover naming the old generation is fenced.
    assert_eq!(
        store
            .take_over_memory_classifier_receipt(key(), 1, 4)
            .unwrap(),
        MemoryClassifierTransition::Fenced
    );
    assert_eq!(
        store
            .lookup_memory_classifier_receipt(key())
            .unwrap()
            .unwrap()
            .state,
        MemoryClassifierReceiptState::InProgress { generation: 2 }
    );

    // The predecessor's completion, failure record, attempt, and session
    // release each match no row and write nothing.
    assert_eq!(
        store
            .complete_memory_classifier_receipt(
                key(),
                1,
                MemoryClassifierTerminalKind::Complete,
                "{}",
                5
            )
            .unwrap(),
        MemoryClassifierTransition::Fenced
    );
    assert_eq!(
        store
            .complete_memory_classifier_receipt(
                key(),
                1,
                MemoryClassifierTerminalKind::Failed,
                "{}",
                6
            )
            .unwrap(),
        MemoryClassifierTransition::Fenced
    );
    assert_eq!(
        store
            .begin_memory_classifier_attempt(key(), 1, &attempt(1, "prov/model-b"), 7)
            .unwrap(),
        MemoryClassifierTransition::Fenced
    );
    assert_eq!(
        store
            .record_memory_classifier_run_handle(key(), 1, 0, "stale-run")
            .unwrap(),
        MemoryClassifierTransition::Fenced
    );
    assert_eq!(
        store
            .finish_memory_classifier_attempt(
                key(),
                1,
                0,
                MemoryClassifierTerminalKind::Complete,
                8
            )
            .unwrap(),
        MemoryClassifierTransition::Fenced
    );
    assert_eq!(
        store
            .release_memory_classifier_attempt_session(key(), 1, 1, 8)
            .unwrap(),
        MemoryClassifierTransition::Fenced
    );
    let receipt = store
        .lookup_memory_classifier_receipt(key())
        .unwrap()
        .unwrap();
    assert_eq!(
        receipt.state,
        MemoryClassifierReceiptState::InProgress { generation: 2 }
    );
    assert_eq!(receipt.updated_at_ms, 3);
    let attempts = store.list_memory_classifier_attempts(key()).unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].run_handle, None);
    assert_eq!(attempts[0].terminal_kind, None);
    assert_eq!(attempts[0].terminal_at_ms, None);
    assert_eq!(attempts[0].session_released_at_ms, None);
    assert_eq!(attempts[1].session_released_at_ms, None);

    // The successor proceeds: its attempt row, terminal, release, and completion land.
    assert_eq!(
        store
            .begin_memory_classifier_attempt(key(), 2, &attempt(0, "prov/model-a"), 9)
            .unwrap(),
        MemoryClassifierTransition::Applied
    );
    assert_eq!(
        store
            .finish_memory_classifier_attempt(
                key(),
                2,
                0,
                MemoryClassifierTerminalKind::Complete,
                10
            )
            .unwrap(),
        MemoryClassifierTransition::Applied
    );
    assert_eq!(
        store
            .release_memory_classifier_attempt_session(key(), 2, 0, 11)
            .unwrap(),
        MemoryClassifierTransition::Applied
    );
    assert_eq!(
        store
            .release_memory_classifier_attempt_session(key(), 2, 0, 12)
            .unwrap(),
        MemoryClassifierTransition::Fenced
    );
    assert_eq!(
        store
            .complete_memory_classifier_receipt(
                key(),
                2,
                MemoryClassifierTerminalKind::Complete,
                "{\"ok\":true}",
                13
            )
            .unwrap(),
        MemoryClassifierTransition::Applied
    );
    // Once complete, no generation can take the receipt over.
    assert_eq!(
        store
            .take_over_memory_classifier_receipt(key(), 2, 14)
            .unwrap(),
        MemoryClassifierTransition::Fenced
    );
    assert_eq!(
        store.count_memory_classifier_attempts(PROJECT, 0).unwrap(),
        3
    );
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
        memory_classifier_request_digest(&ordered).unwrap(),
        memory_classifier_request_digest(&permuted).unwrap()
    );
    // Array order affects the digest.
    let reordered_items = json!({
        "task": "classify",
        "prompt_body": "p",
        "items": ["b", "a"],
        "model_chain": ["prov/x"],
        "nested": {"alpha": 1, "beta": [1, 2, {"gamma": true, "delta": null}]},
    });
    assert_ne!(
        memory_classifier_request_digest(&ordered).unwrap(),
        memory_classifier_request_digest(&reordered_items).unwrap()
    );
    let canonical = context_core::canonical_json::canonical_json_encode(&ordered).unwrap();
    let expected = {
        use sha2::Digest as _;
        let mut hasher = sha2::Sha256::new();
        hasher.update(b"eidnara-memory_classifier-request-v1\n");
        hasher.update(canonical.as_bytes());
        format!("{:x}", hasher.finalize())
    };
    assert_eq!(
        memory_classifier_request_digest(&ordered).unwrap(),
        expected
    );
}
