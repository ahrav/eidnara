//! Applies a transform response's edit recipe the way a client does, so integration tests that
//! speak the wire directly can assert on the reconstructed message array.

use std::sync::Arc;

use daemon::edit_recipe::{Recipe, Revision, SourceBase, canonical_len};
use serde_json::Value;

/// Reconstructs the served array from `request.messages[i].ck` and the response's operations.
/// The request carries no applied previous output, so every keep addresses the input.
pub fn applied_messages(request: &Value, response: &Value) -> Vec<Value> {
    let recipe = Recipe::from_json(response).expect("wire response carries a valid recipe");
    let base = Revision::parse(
        request["base_revision"]
            .as_str()
            .expect("the request names its base revision"),
    )
    .expect("test base revisions are well formed");
    let input: Vec<Arc<Value>> = request["messages"]
        .as_array()
        .expect("transform requests carry a messages array")
        .iter()
        .map(|message| Arc::new(message["ck"].clone()))
        .collect();
    let lengths: Vec<usize> = input
        .iter()
        .map(|value| canonical_len(value).expect("test input length"))
        .collect();
    let applied = recipe
        .apply(
            SourceBase {
                revision: &base,
                values: &input,
                lengths: &lengths,
            },
            None,
        )
        .expect("the recipe applies against the request it answers");
    applied
        .values
        .iter()
        .map(|value| (**value).clone())
        .collect()
}
