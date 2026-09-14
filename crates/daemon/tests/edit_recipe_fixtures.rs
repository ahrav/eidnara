//! Shared acceptance and rejection cases for the two-source edit recipe. The TypeScript client
//! reads the same fixture file, so both languages agree on every accepted output and on every
//! rejection without standardizing the error taxonomy.

use std::sync::Arc;

use daemon::edit_recipe::{Recipe, Revision, SourceBase, canonical_len};
use serde::Deserialize;
use serde_json::Value;

const FIXTURE: &str = include_str!("fixtures/transform-edit-recipe-v1.json");

#[derive(Deserialize)]
struct Fixture {
    schema: String,
    input_revision_default: String,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    input_revision: Option<String>,
    input: Vec<Value>,
    previous: Option<PreviousBase>,
    recipe: Value,
    expect: Expectation,
}

#[derive(Deserialize)]
struct PreviousBase {
    revision: String,
    values: Vec<Value>,
}

#[derive(Deserialize)]
struct Expectation {
    ok: bool,
    #[serde(default)]
    output: Vec<Value>,
    canonical_bytes: Option<usize>,
}

struct Shared {
    revision: Revision,
    values: Vec<Arc<Value>>,
    lengths: Vec<usize>,
}

impl Shared {
    fn new(revision: &str, values: &[Value]) -> Self {
        let values: Vec<Arc<Value>> = values.iter().cloned().map(Arc::new).collect();
        Self {
            revision: Revision::parse(revision).expect("held revision"),
            lengths: values
                .iter()
                .map(|value| canonical_len(value).expect("value length"))
                .collect(),
            values,
        }
    }

    fn base(&self) -> SourceBase<'_> {
        SourceBase {
            revision: &self.revision,
            values: &self.values,
            lengths: &self.lengths,
        }
    }
}

fn load() -> Fixture {
    let fixture: Fixture = serde_json::from_str(FIXTURE).expect("fixture parses");
    assert_eq!(fixture.schema, "transform-edit-recipe-fixtures-v1");
    fixture
}

#[test]
fn edit_recipe_fixture_cases_agree_with_the_rust_applier() {
    let fixture = load();
    let mut accepted = 0;
    let mut rejected = 0;
    for case in &fixture.cases {
        let input = Shared::new(
            case.input_revision
                .as_deref()
                .unwrap_or(&fixture.input_revision_default),
            &case.input,
        );
        let previous = case
            .previous
            .as_ref()
            .map(|previous| Shared::new(&previous.revision, &previous.values));
        let result = Recipe::from_json(&case.recipe)
            .and_then(|recipe| recipe.apply(input.base(), previous.as_ref().map(Shared::base)));
        match (case.expect.ok, result) {
            (true, Ok(output)) => {
                let rendered: Vec<Value> = output
                    .values
                    .iter()
                    .map(|value| (**value).clone())
                    .collect();
                assert_eq!(rendered, case.expect.output, "case {:?} output", case.name);
                assert_eq!(
                    Some(output.bytes),
                    case.expect.canonical_bytes,
                    "case {:?} canonical bytes",
                    case.name
                );
                assert_eq!(
                    output.bytes,
                    serde_json::to_vec(&rendered).expect("serializes").len(),
                    "case {:?} measured size matches serialization",
                    case.name
                );
                assert_eq!(
                    output.lengths,
                    rendered
                        .iter()
                        .map(|value| canonical_len(value).expect("value length"))
                        .collect::<Vec<_>>(),
                    "case {:?} per-entry lengths",
                    case.name
                );
                // Every kept entry is a source handle; every other entry is a recipe literal.
                let literals: Vec<&Value> = case.recipe["operations"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|operation| operation["values"].as_array())
                    .flatten()
                    .collect();
                for value in &output.values {
                    let shared = input
                        .values
                        .iter()
                        .chain(previous.iter().flat_map(|base| base.values.iter()))
                        .any(|candidate| Arc::ptr_eq(candidate, value));
                    let literal = literals.iter().any(|literal| *literal == value.as_ref());
                    assert!(
                        shared || literal,
                        "case {:?} produced a copied entry",
                        case.name
                    );
                }
                accepted += 1;
            }
            (false, Err(_)) => rejected += 1,
            (true, Err(error)) => panic!("case {:?} rejected: {error}", case.name),
            (false, Ok(output)) => {
                panic!(
                    "case {:?} accepted {} entries",
                    case.name,
                    output.values.len()
                )
            }
        }
    }
    assert!(accepted >= 15, "accepted cases: {accepted}");
    assert!(rejected >= 30, "rejected cases: {rejected}");
}

#[test]
fn edit_recipe_fixture_previous_keeps_share_their_source_handles() {
    let fixture = load();
    let case = fixture
        .cases
        .iter()
        .find(|case| case.name.starts_with("AE2"))
        .expect("AE2 case");
    let input = Shared::new(&fixture.input_revision_default, &case.input);
    let previous = case.previous.as_ref().expect("AE2 has a previous base");
    let previous = Shared::new(&previous.revision, &previous.values);
    let recipe = Recipe::from_json(&case.recipe).expect("valid");
    let output = recipe
        .apply(input.base(), Some(previous.base()))
        .expect("applies");
    assert!(Arc::ptr_eq(&output.values[0], &previous.values[0]));
    assert!(Arc::ptr_eq(&output.values[1], &previous.values[1]));
    assert!(Arc::ptr_eq(&output.values[2], &input.values[3]));
}
