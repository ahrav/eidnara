//! Seeded generated operation sequences for the two-source edit recipe. A valid plan is built
//! constructively from independent per-source cursors so its expected output comes from the plan,
//! not from the applier; one mutation of a valid plan must be rejected with the matching variant.

use std::sync::Arc;

use daemon::edit_recipe::{
    Keyed, MAX_CONFIRM_PROBES, Recipe, RecipeError, Revision, Source, SourceBase, build_recipe,
    canonical_len,
};
use proptest::prelude::*;
use proptest::test_runner::TestCaseError;
use serde_json::{Value, json};

#[derive(Debug, Clone)]
enum Step {
    Skip(usize),
    Keep(usize),
    Insert(Vec<Value>),
}

#[derive(Debug, Clone)]
struct Plan {
    input: Vec<Value>,
    previous: Option<Vec<Value>>,
    /// Steps alternate sources by the flag: `true` walks `previous` when it exists.
    steps: Vec<(bool, Step)>,
}

fn message(id: u32) -> Value {
    json!({"info": {"id": format!("m{id}")}, "parts": [{"type": "text", "text": "x".repeat(id as usize % 7)}]})
}

fn plan() -> impl Strategy<Value = Plan> {
    let literal = (0u32..3).prop_map(|id| json!({"info": {"role": "user"}, "text": id}));
    (
        0usize..12,
        proptest::option::of(0usize..12),
        proptest::collection::vec(
            (
                any::<bool>(),
                prop_oneof![
                    (0usize..4).prop_map(Step::Skip),
                    (1usize..4).prop_map(Step::Keep),
                    proptest::collection::vec(literal, 1..3).prop_map(Step::Insert),
                ],
            ),
            0..10,
        ),
    )
        .prop_map(|(input_len, previous_len, steps)| Plan {
            input: (0..input_len as u32).map(message).collect(),
            previous: previous_len.map(|len| (100..100 + len as u32).map(message).collect()),
            steps,
        })
}

struct Built {
    recipe: Value,
    expected: Vec<Value>,
    literal_count: usize,
}

/// Walks the plan with one cursor per source, dropping steps that would run past a source.
fn build(plan: &Plan) -> Built {
    let mut cursors = [0usize; 2];
    let mut operations = Vec::new();
    let mut expected = Vec::new();
    let mut literal_count = 0;
    let mut uses_previous = false;
    for (previous, step) in &plan.steps {
        let (source, values) = match (previous, &plan.previous) {
            (true, Some(previous)) => (Source::Previous, previous),
            _ => (Source::Input, &plan.input),
        };
        let cursor = &mut cursors[source as usize];
        match step {
            Step::Skip(n) => *cursor = (*cursor + n).min(values.len()),
            Step::Keep(n) => {
                let end = (*cursor + n).min(values.len());
                if end == *cursor {
                    continue;
                }
                operations.push(json!({
                    "op": "keep",
                    "source": source.wire_id(),
                    "start": *cursor,
                    "count": end - *cursor,
                }));
                expected.extend(values[*cursor..end].iter().cloned());
                uses_previous |= source == Source::Previous;
                *cursor = end;
            }
            Step::Insert(literals) => {
                operations.push(json!({"op": "insert", "values": literals}));
                expected.extend(literals.iter().cloned());
                literal_count += literals.len();
            }
        }
    }
    let mut recipe = json!({
        "base_revision": "base",
        "output_revision": "out",
        "operations": operations,
    });
    if uses_previous {
        recipe["previous_output_revision"] = json!("prev");
    }
    Built {
        recipe,
        expected,
        literal_count,
    }
}

struct Held {
    revision: Revision,
    values: Vec<Arc<Value>>,
    lengths: Vec<usize>,
}

impl Held {
    fn new(revision: &str, values: &[Value]) -> Self {
        let values: Vec<Arc<Value>> = values.iter().cloned().map(Arc::new).collect();
        Self {
            revision: Revision::parse(revision).expect("revision"),
            lengths: values.iter().map(|value| canonical_len(value)).collect(),
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

#[derive(Debug, Clone, Copy)]
enum Mutation {
    RepeatLastKeep,
    ExtendLastKeepPastEnd,
    ZeroLastCount,
    EmptyFirstInsert,
    DropPreviousRevision,
    AddUnusedPreviousRevision,
    RenameLastOp,
    UnknownSourceOnLastKeep,
    NegativeStart,
    FractionalStart,
    ExtraFieldOnLastOp,
    WrongBaseRevision,
}

const MUTATIONS: [Mutation; 12] = [
    Mutation::RepeatLastKeep,
    Mutation::ExtendLastKeepPastEnd,
    Mutation::ZeroLastCount,
    Mutation::EmptyFirstInsert,
    Mutation::DropPreviousRevision,
    Mutation::AddUnusedPreviousRevision,
    Mutation::RenameLastOp,
    Mutation::UnknownSourceOnLastKeep,
    Mutation::NegativeStart,
    Mutation::FractionalStart,
    Mutation::ExtraFieldOnLastOp,
    Mutation::WrongBaseRevision,
];

fn last_position(operations: &[Value], op: &str) -> Option<usize> {
    operations
        .iter()
        .rposition(|operation| operation["op"] == op)
}

/// Returns the mutated recipe and the variant it must produce, or `None` when the plan lacks the
/// construct the mutation needs.
fn mutate(built: &Built, mutation: Mutation) -> Option<(Value, RecipeError)> {
    let mut recipe = built.recipe.clone();
    let operations = recipe["operations"].as_array_mut()?;
    let keep = last_position(operations, "keep");
    let source_of = |operation: &Value| -> Source {
        if operation["source"] == "previous" {
            Source::Previous
        } else {
            Source::Input
        }
    };
    let expected = match mutation {
        Mutation::RepeatLastKeep => {
            let index = keep?;
            let repeated = operations[index].clone();
            let source = source_of(&repeated);
            operations.push(repeated);
            RecipeError::BackwardRange {
                source,
                index: operations.len() - 1,
            }
        }
        Mutation::ExtendLastKeepPastEnd => {
            let index = keep?;
            let source = source_of(&operations[index]);
            operations[index]["count"] = json!(u64::from(u32::MAX));
            RecipeError::OutOfBounds { source, index }
        }
        Mutation::ZeroLastCount => {
            operations[keep?]["count"] = json!(0);
            RecipeError::ZeroCount
        }
        Mutation::EmptyFirstInsert => {
            let index = operations
                .iter()
                .position(|operation| operation["op"] == "insert")?;
            operations[index]["values"] = json!([]);
            RecipeError::EmptyInsert
        }
        Mutation::DropPreviousRevision => {
            recipe.as_object_mut()?.remove("previous_output_revision")?;
            RecipeError::MissingPreviousBase
        }
        Mutation::AddUnusedPreviousRevision => {
            if recipe.get("previous_output_revision").is_some() {
                return None;
            }
            recipe["previous_output_revision"] = json!("prev");
            RecipeError::UnusedPreviousRevision
        }
        Mutation::RenameLastOp => {
            let index = operations.len().checked_sub(1)?;
            operations[index]["op"] = json!("move");
            RecipeError::UnknownOperation("move".into())
        }
        Mutation::UnknownSourceOnLastKeep => {
            operations[keep?]["source"] = json!("cache");
            RecipeError::UnknownSource("cache".into())
        }
        Mutation::NegativeStart => {
            operations[keep?]["start"] = json!(-1);
            RecipeError::UnsafeInteger("start")
        }
        Mutation::FractionalStart => {
            operations[keep?]["start"] = json!(0.5);
            RecipeError::UnsafeInteger("start")
        }
        Mutation::ExtraFieldOnLastOp => {
            let index = operations.len().checked_sub(1)?;
            operations[index]["path"] = json!("/x");
            RecipeError::UnknownField("path".into())
        }
        Mutation::WrongBaseRevision => {
            recipe["base_revision"] = json!("other");
            RecipeError::WrongBaseRevision
        }
    };
    Some((recipe, expected))
}

fn apply(plan: &Plan, recipe: &Value) -> Result<(Vec<Arc<Value>>, usize), RecipeError> {
    let input = Held::new("base", &plan.input);
    let previous = plan
        .previous
        .as_ref()
        .map(|values| Held::new("prev", values));
    Recipe::from_json(recipe)
        .and_then(|recipe| recipe.apply(input.base(), previous.as_ref().map(Held::base)))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn edit_recipe_generated_valid_plans_reconstruct_the_planned_output(plan in plan()) {
        let built = build(&plan);
        let (output, bytes) = apply(&plan, &built.recipe).expect("a constructively valid plan applies");
        let rendered: Vec<Value> = output.iter().map(|value| (**value).clone()).collect();
        prop_assert_eq!(&rendered, &built.expected);
        prop_assert_eq!(bytes, serde_json::to_vec(&rendered).unwrap().len());
        let previous_len = plan.previous.as_ref().map_or(0, Vec::len);
        prop_assert!(output.len() <= plan.input.len() + previous_len + built.literal_count);
        // Serialize and parse again: the wire form of a recipe is its own round trip.
        let parsed: Recipe = serde_json::from_value(built.recipe.clone()).unwrap();
        prop_assert_eq!(serde_json::to_value(&parsed).unwrap(), built.recipe);
    }

    #[test]
    fn edit_recipe_generated_one_mutation_is_rejected_with_its_variant(
        plan in plan(),
        mutation in proptest::sample::select(MUTATIONS.as_slice()),
    ) {
        let built = build(&plan);
        if let Some((recipe, expected)) = mutate(&built, mutation) {
            prop_assert_eq!(apply(&plan, &recipe).map(|_| ()), Err(expected));
        }
    }
}

/// A final array assembled from source messages and literals in any order; the builder must
/// produce a recipe the applier turns back into exactly this array.
fn assembled_output() -> impl Strategy<Value = (Vec<Value>, Option<Vec<Value>>, Vec<Value>)> {
    (0usize..10, proptest::option::of(0usize..10)).prop_flat_map(|(input_len, previous_len)| {
        let input: Vec<Value> = (0..input_len as u32).map(message).collect();
        let previous: Option<Vec<Value>> =
            previous_len.map(|len| (100..100 + len as u32).map(message).collect());
        let pool: Vec<Value> = input
            .iter()
            .chain(previous.iter().flatten())
            .cloned()
            .chain((200..203).map(message))
            .collect();
        let picks = proptest::collection::vec(0..pool.len().max(1), 0..12);
        (Just(input), Just(previous), Just(pool), picks).prop_map(
            |(input, previous, pool, picks)| {
                let output = if pool.is_empty() {
                    Vec::new()
                } else {
                    picks.into_iter().map(|pick| pool[pick].clone()).collect()
                };
                (input, previous, output)
            },
        )
    })
}

fn keyed(values: &[Arc<Value>], key: fn(&Value) -> String) -> Vec<Keyed<String, Arc<Value>>> {
    values
        .iter()
        .map(|value| Keyed {
            key: key(value),
            value: Arc::clone(value),
        })
        .collect()
}

fn message_id(value: &Value) -> String {
    value["info"]["id"].as_str().unwrap_or("").to_owned()
}

fn shared_key(_: &Value) -> String {
    String::new()
}

fn round_trip(
    input: &[Value],
    previous: Option<&[Value]>,
    output: &[Value],
    key: fn(&Value) -> String,
) -> Result<Value, TestCaseError> {
    let input = Held::new("base", input);
    let previous = previous.map(|values| Held::new("prev", values));
    let output: Vec<Arc<Value>> = output.iter().cloned().map(Arc::new).collect();
    let previous_keyed = previous.as_ref().map(|held| keyed(&held.values, key));
    let previous_source = previous
        .as_ref()
        .zip(previous_keyed.as_ref())
        .map(|(held, entries)| (&held.revision, entries.as_slice()));
    let recipe = build_recipe(
        &keyed(&output, key),
        (&input.revision, &keyed(&input.values, key)),
        previous_source,
        Revision::parse("out").unwrap(),
    );
    let wire = serde_json::to_value(&recipe).unwrap();
    let parsed = Recipe::from_json(&wire).expect("built recipes are valid");
    let (applied, bytes) = parsed
        .apply(input.base(), previous.as_ref().map(Held::base))
        .expect("built recipes apply");
    prop_assert_eq!(applied.len(), output.len());
    for (kept, wanted) in applied.iter().zip(&output) {
        prop_assert_eq!(kept.as_ref(), wanted.as_ref());
    }
    let rendered: Vec<Value> = applied.iter().map(|value| (**value).clone()).collect();
    prop_assert_eq!(bytes, serde_json::to_vec(&rendered).unwrap().len());
    // Adjacent operations are coalesced: no two consecutive inserts, no contiguous same-source keeps.
    let operations = wire["operations"].as_array().unwrap();
    for pair in operations.windows(2) {
        let both_inserts = pair[0]["op"] == "insert" && pair[1]["op"] == "insert";
        prop_assert!(!both_inserts, "adjacent inserts not coalesced: {pair:?}");
        if pair[0]["op"] == "keep"
            && pair[1]["op"] == "keep"
            && pair[0]["source"] == pair[1]["source"]
        {
            let end = pair[0]["start"].as_u64().unwrap() + pair[0]["count"].as_u64().unwrap();
            prop_assert_ne!(
                end,
                pair[1]["start"].as_u64().unwrap(),
                "contiguous keeps not coalesced"
            );
        }
    }
    Ok(wire)
}

fn assembled_output_with_long_sources()
-> impl Strategy<Value = (Vec<Value>, Option<Vec<Value>>, Vec<Value>)> {
    let cap = MAX_CONFIRM_PROBES;
    (0usize..3 * cap, proptest::option::of(0usize..3 * cap)).prop_flat_map(
        move |(input_len, previous_len)| {
            let input: Vec<Value> = (0..input_len as u32).map(message).collect();
            let previous: Option<Vec<Value>> =
                previous_len.map(|len| (100..100 + len as u32).map(message).collect());
            let pool: Vec<Value> = input
                .iter()
                .chain(previous.iter().flatten())
                .cloned()
                .chain((200..203).map(message))
                .collect();
            let picks = proptest::collection::vec(0..pool.len().max(1), 0..3 * cap);
            (Just(input), Just(previous), Just(pool), picks).prop_map(
                |(input, previous, pool, picks)| {
                    let output = if pool.is_empty() {
                        Vec::new()
                    } else {
                        picks.into_iter().map(|pick| pool[pick].clone()).collect()
                    };
                    (input, previous, output)
                },
            )
        },
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn edit_recipe_generated_built_recipes_round_trip_through_the_applier((input, previous, output) in assembled_output()) {
        round_trip(&input, previous.as_deref(), &output, message_id)?;
    }

    #[test]
    fn edit_recipe_generated_shared_key_round_trips_within_the_probe_budget((input, previous, output) in assembled_output_with_long_sources()) {
        let wire = round_trip(&input, previous.as_deref(), &output, shared_key)?;
        // With one shared key every position is a candidate, so `start - cursor` counts probes.
        let mut cursors = [0u64; 2];
        for operation in wire["operations"].as_array().unwrap() {
            if operation["op"] != "keep" {
                continue;
            }
            let source = usize::from(operation["source"] == "previous");
            let start = operation["start"].as_u64().unwrap();
            prop_assert!(
                start - cursors[source] < MAX_CONFIRM_PROBES as u64,
                "keep at {start} probed past the budget from cursor {}",
                cursors[source]
            );
            cursors[source] = start + operation["count"].as_u64().unwrap();
        }
    }
}

/// Every mutation class must fire on some seed; a class whose precondition never holds would
/// make the rejection test vacuous for it.
#[test]
fn edit_recipe_generated_every_mutation_class_is_reachable() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::{Config, TestRunner};
    let mut runner = TestRunner::new(Config::default());
    let mut fired = [false; MUTATIONS.len()];
    for _ in 0..512 {
        let plan = plan().new_tree(&mut runner).expect("tree").current();
        let built = build(&plan);
        for (index, mutation) in MUTATIONS.iter().enumerate() {
            if mutate(&built, *mutation).is_some() {
                fired[index] = true;
            }
        }
    }
    let missing: Vec<&Mutation> = MUTATIONS
        .iter()
        .zip(fired)
        .filter(|(_, fired)| !fired)
        .map(|(mutation, _)| mutation)
        .collect();
    assert!(missing.is_empty(), "mutations never enabled: {missing:?}");
}
