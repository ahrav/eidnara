#[allow(dead_code)]
#[path = "support/alloc_recorder.rs"]
mod alloc_recorder;

use std::sync::Arc;

use alloc_recorder::record_window;
use daemon::edit_recipe::{Operation, Recipe, RecipeError, Revision, Source, SourceBase};
use serde_json::{Value, json};

#[global_allocator]
static GLOBAL: alloc_recorder::RecordingAlloc = alloc_recorder::RecordingAlloc;

#[test]
fn rejected_many_insert_operations_validate_before_materializing_literals() {
    let mut operations = (0..20_000)
        .map(|_| json!({ "op": "insert", "values": [null] }))
        .collect::<Vec<_>>();
    operations.push(json!({}));
    let value = json!({
        "base_revision": "b",
        "output_revision": "o",
        "operations": operations,
    });

    let (result, ledger) = record_window(|| Recipe::from_json(&value));
    assert_eq!(result, Err(RecipeError::MissingField("op")));
    assert!(!ledger.overflow, "allocation ledger overflowed");
    assert!(
        ledger.allocation_events < 100,
        "validation made {} allocations",
        ledger.allocation_events
    );
    assert!(
        ledger.peak_live_bytes < 512 * 1024,
        "validation peaked at {} bytes",
        ledger.peak_live_bytes
    );
}

#[test]
fn rejected_many_insert_operations_keep_validation_allocations_flat() {
    let literal = Arc::new(Value::Null);
    let mut operations = (0..20_000)
        .map(|_| Operation::Insert {
            values: vec![Arc::clone(&literal)],
        })
        .collect::<Vec<_>>();
    operations.push(Operation::Keep {
        source: Source::Input,
        start: 0,
        count: 1,
    });
    let revision = Revision::parse("b").unwrap();
    let recipe = Recipe {
        base_revision: revision.clone(),
        output_revision: Revision::parse("o").unwrap(),
        previous_output_revision: None,
        operations,
    };
    let input = SourceBase {
        revision: &revision,
        values: &[],
        lengths: &[],
    };

    let (result, ledger) = record_window(|| recipe.apply(input, None));
    assert!(matches!(
        result,
        Err(RecipeError::OutOfBounds {
            source: Source::Input,
            index: 20_000
        })
    ));
    assert!(!ledger.overflow, "allocation ledger overflowed");
    assert!(
        ledger.allocation_events < 100,
        "validation made {} allocations",
        ledger.allocation_events
    );
    assert!(
        ledger.peak_live_bytes < 512 * 1024,
        "validation peaked at {} bytes",
        ledger.peak_live_bytes
    );
}
