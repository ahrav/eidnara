use std::sync::LazyLock;

use serde_json::{Value, json};

use super::compression_fidelity_corpus::*;
use crate::harness_sources::{Harness, Representation, SessionIdentity, opencode_units};
use crate::history_summarizer_validate::parse_history_segment_output;

fn corpus_value() -> Value {
    static VALUE: LazyLock<Value> = LazyLock::new(|| serde_json::from_slice(CORPUS_BYTES).unwrap());
    VALUE.clone()
}

fn validate_value(value: Value) -> Vec<Violation> {
    validate(&serde_json::from_value(value).expect("mutated corpus still decodes"))
}

fn case_mut(value: &mut Value, case: usize) -> &mut Value {
    &mut value["cases"][case]
}

fn text_mut<'v>(value: &'v mut Value, path: &[&str]) -> &'v mut Value {
    path.iter()
        .fold(value, |at, key| match key.parse::<usize>() {
            Ok(index) => &mut at[index],
            Err(_) => &mut at[*key],
        })
}

fn with_appended(path: &[&str], suffix: &str) -> Vec<Violation> {
    let mut value = corpus_value();
    let text = text_mut(&mut value, path);
    *text = format!("{}{suffix}", text.as_str().unwrap()).into();
    validate_value(value)
}

fn leaked(field: &str, label: &str) -> Violation {
    Violation::AnswerKeyInProviderInput {
        field: field.into(),
        label: label.into(),
    }
}

fn adapter_disagreements(value: &Value) -> Vec<String> {
    let corpus: Corpus = serde_json::from_value(value.clone()).expect("the corpus decodes");
    let mut out = Vec::new();
    for (case, raw_case) in corpus.cases.iter().zip(value["cases"].as_array().unwrap()) {
        for (source, raw) in case
            .sources
            .iter()
            .zip(raw_case["sources"].as_array().unwrap())
        {
            let session = SessionIdentity {
                project_id: corpus.project_id.clone(),
                harness: Harness::OpenCode,
                session_id: source.session_id.clone(),
            };
            let natives = raw["messages"]
                .as_array()
                .unwrap()
                .iter()
                .chain(raw["successors"].as_array().into_iter().flatten());
            let records = source
                .messages
                .iter()
                .chain(&source.successors)
                .zip(natives);
            for (message, native) in records {
                let at = format!("{}/{}", source.id, message.info.id);
                let units = match opencode_units(&session, native) {
                    Ok(units) => units,
                    Err(refusal) => {
                        out.push(format!("{at}: {refusal}"));
                        continue;
                    }
                };
                if units.len() != message.parts.len() {
                    out.push(format!(
                        "{at}: {} units for {} parts",
                        units.len(),
                        message.parts.len()
                    ));
                }
                for (index, part) in message.parts.iter().enumerate() {
                    let (text_part, field, key) = match part {
                        Part::Text { .. } => (true, "block_index", index.to_string()),
                        Part::Tool { call_id, .. } => (false, "tool_call_id", call_id.clone()),
                    };
                    let published = units.iter().find(|unit| {
                        (unit.representation == Representation::Text) == text_part
                            && unit
                                .identity
                                .iter()
                                .any(|(name, value)| *name == field && *value == key)
                    });
                    let agrees = published.is_some_and(|unit| {
                        unit.revision == block_revision(message, part).to_string()
                            && Some(unit.text.as_str()) == block_text(part)
                    });
                    if !agrees {
                        out.push(format!("{at}#{index}"));
                    }
                }
            }
        }
    }
    out
}

#[test]
fn the_committed_corpus_matches_its_pin_and_validates() {
    let corpus = corpus();
    assert_eq!(
        corpus
            .cases
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        ["C1", "C2", "C3", "C4", "C5", "C6"]
    );
    assert!(
        corpus
            .cases
            .iter()
            .all(|c| matches!(c.provenance, Provenance::Synthetic)),
        "no case cites a documented incident, so every case is synthetic"
    );
}

fn authored_tier<'e>(example: &'e str, tier: &str) -> Option<&'e str> {
    if [format!("<{tier} />"), format!("<{tier}/>")]
        .iter()
        .any(|tag| example.contains(tag.as_str()))
    {
        return Some("");
    }
    let (_, rest) = example.split_once(&format!("<{tier}>"))?;
    rest.split_once(&format!("</{tier}>"))
        .map(|(body, _)| body.trim())
}

fn approved_example_problem(example: &str, message_count: u64) -> Result<(), String> {
    for tier in ["p1", "p2", "p3"] {
        if !authored_tier(example, tier).is_some_and(|body| !body.is_empty()) {
            return Err(format!("<{tier}> is authored"));
        }
    }
    if authored_tier(example, "p4").is_none() {
        return Err("<p4> is authored".into());
    }
    let parsed = parse_history_segment_output(example).map_err(|error| format!("{error:?}"))?;
    let [segment] = parsed.history_segments.as_slice() else {
        return Err("one segment".into());
    };
    if (segment.start_message, segment.end_message) != (1, message_count) {
        return Err("the segment covers the whole source".into());
    }
    if segment.title.trim().is_empty() {
        return Err("the segment has a title".into());
    }
    for tier in [&segment.p1, &segment.p2, &segment.p3] {
        if !tier.as_deref().is_some_and(|body| !body.trim().is_empty()) {
            return Err("authored P1 to P3".into());
        }
    }
    if segment.p4.as_deref() != Some("") {
        return Err("P4 is the title-only capsule".into());
    }
    if !example.contains(&format!(
        "<messages_processed>1-{message_count}</messages_processed>"
    )) {
        return Err("the processed range covers the whole source".into());
    }
    Ok(())
}

#[test]
fn every_approved_example_is_one_segment_over_its_whole_source() {
    for case in &corpus().cases {
        for source in &case.sources {
            assert_eq!(
                approved_example_problem(&source.approved_example, source.messages.len() as u64),
                Ok(()),
                "{}",
                source.id
            );
        }
    }
}

/// Approved examples author P2, P3, and P4 explicitly, even where the history parser accepts
/// their omission.
#[test]
fn an_approved_example_without_an_authored_tier_is_rejected() {
    let source = &corpus().cases[0].sources[0];
    let count = source.messages.len() as u64;
    let example = source.approved_example.as_str();
    let without = |open: &str, close: &str| {
        let start = example.find(open).unwrap();
        let end = example.find(close).unwrap() + close.len();
        format!("{}{}", &example[..start], &example[end..])
    };
    for (tier, edited) in [
        ("p2", without("<p2>", "</p2>")),
        ("p3", without("<p3>", "</p3>")),
        ("p4", without("<p4 />", "<p4 />")),
    ] {
        assert_ne!(edited, example, "{tier}");
        assert!(
            parse_history_segment_output(&edited).is_ok(),
            "{tier}: the edit still parses"
        );
        assert_eq!(
            approved_example_problem(&edited, count),
            Err(format!("<{tier}> is authored")),
            "{tier}"
        );
    }
}

#[test]
fn the_digest_pin_accepts_only_the_exact_committed_bytes() {
    assert_eq!(verify_corpus_identity(CORPUS_BYTES), Ok(()));
    let text = std::str::from_utf8(CORPUS_BYTES).unwrap();
    let mut older = corpus_value();
    older["cases"].as_array_mut().unwrap().pop();
    let stale = serde_json::to_string_pretty(&older).unwrap();
    let whitespace = text.replacen("\n  ", "\n   ", 1);
    let same_length = text.replacen("quick win", "quick wit", 1);
    let scenario = text.replacen(
        r#""abstention": "forbidden""#,
        r#""abstention": "permitted""#,
        1,
    );
    assert_eq!(same_length.len(), text.len());
    for (label, edited) in [
        ("missing", ""),
        ("stale prebuilt copy", stale.as_str()),
        ("whitespace only", whitespace.as_str()),
        ("same length", same_length.as_str()),
        ("scenario only", scenario.as_str()),
    ] {
        assert_ne!(edited.as_bytes(), CORPUS_BYTES, "{label} edits the bytes");
        assert!(
            verify_corpus_identity(edited.as_bytes()).is_err(),
            "{label} must be rejected"
        );
        assert!(
            corpus_from(edited.as_bytes())
                .unwrap_err()
                .starts_with("corpus digest"),
            "{label} is rejected by its digest"
        );
    }
    for edited in [&whitespace, &same_length] {
        assert!(
            validate(&serde_json::from_str(edited).unwrap()).is_empty(),
            "only the pin rejects a content-preserving edit"
        );
    }
}

#[test]
fn duplicate_and_malformed_ids_are_rejected() {
    let mut value = corpus_value();
    case_mut(&mut value, 1)["id"] = "C1".into();
    assert!(validate_value(value).contains(&Violation::DuplicateId("C1".into())));

    let mut value = corpus_value();
    let duplicate = case_mut(&mut value, 0)["scenarios"][0]["id"].clone();
    case_mut(&mut value, 0)["scenarios"][1]["id"] = duplicate;
    assert!(validate_value(value).contains(&Violation::DuplicateId("C1.S1".into())));

    for malformed in ["C1-S2", "C1.S0", "C2.S2", "C1.O2"] {
        let mut value = corpus_value();
        case_mut(&mut value, 0)["scenarios"][1]["id"] = malformed.into();
        assert!(
            validate_value(value).contains(&Violation::MalformedId(malformed.into())),
            "{malformed}"
        );
    }
    let mut value = corpus_value();
    case_mut(&mut value, 0)["id"] = "C01".into();
    assert!(validate_value(value).contains(&Violation::MalformedId("C01".into())));
}

#[test]
fn spans_that_do_not_resolve_into_their_native_block_are_rejected() {
    fn mutated(mutate: impl FnOnce(&mut Value)) -> (Vec<Violation>, Value) {
        let mut value = corpus_value();
        let span = &mut case_mut(&mut value, 0)["obligations"][0]["evidence"][0];
        mutate(span);
        let span = span.clone();
        (validate_value(value), span)
    }
    let unresolved = |problem| {
        vec![Violation::SpanUnresolved {
            owner: "C1.O1".into(),
            problem,
        }]
    };
    let (violations, _) = mutated(|span| span["text"] = "Let's do pooling first.".into());
    assert_eq!(
        violations,
        unresolved("the span bytes differ from its quoted text")
    );
    let (violations, _) = mutated(|span| {
        span["start"] = (span["start"].as_u64().unwrap() + 1).into();
        span["end"] = (span["end"].as_u64().unwrap() + 1).into();
    });
    assert_eq!(
        violations,
        unresolved("the span bytes differ from its quoted text")
    );
    let (violations, _) = mutated(|span| span["end"] = 100_000.into());
    assert_eq!(violations, unresolved("the span ends past the block"));
    let (violations, _) = mutated(|span| span["end"] = span["start"].clone());
    assert_eq!(violations, unresolved("the span is empty or reversed"));
    let (violations, _) = mutated(|span| span["message_id"] = "msg_cf_c1_99".into());
    assert_eq!(violations, unresolved("the message is not in the source"));
    let (violations, _) = mutated(|span| span["block_index"] = 7.into());
    assert_eq!(
        violations,
        unresolved("the block index is not in the message")
    );
    let canonical = corpus().cases[0].obligations[0].evidence[0]
        .revision
        .clone();
    for revision in ["0".to_owned(), format!("0{canonical}")] {
        let (violations, span) = mutated(|span| span["revision"] = revision.as_str().into());
        assert_eq!(
            violations,
            [Violation::RevisionMismatch {
                owner: "C1.O1".into(),
                message_id: "msg_cf_c1_03".into(),
                revision: span["revision"].as_str().unwrap().to_owned(),
            }],
            "{revision}"
        );
    }

    let mut value = corpus_value();
    let span = &mut case_mut(&mut value, 5)["obligations"][1]["evidence"][0];
    let text = span["text"].as_str().unwrap().to_owned();
    span["end"] = (text.find('ï').unwrap() + 1).into();
    span["text"] = "x".into();
    assert_eq!(
        validate_value(value),
        [Violation::SpanUnresolved {
            owner: "C6.O2".into(),
            problem: "the span splits a UTF-8 character",
        }]
    );
}

#[test]
fn a_span_resolves_the_bytes_of_the_revision_it_names() {
    let source = corpus().case("C6").unwrap().source("C6.V1").unwrap();
    let (original, successor) = (&source.messages[1], &source.successors[0]);
    let original_revision = block_revision(original, &original.parts[1]);
    let successor_revision = block_revision(successor, &successor.parts[1]);
    assert!(successor_revision > original_revision);
    let original_text = block_text(&original.parts[1]).unwrap();
    let successor_text = block_text(&successor.parts[1]).unwrap();
    assert_ne!(original_text, successor_text);
    assert_eq!(original_text.len(), successor_text.len());

    let resolve = |revision: u64, text: &str| {
        let mut value = corpus_value();
        let span = &mut case_mut(&mut value, 5)["obligations"][1]["evidence"][0];
        span["revision"] = revision.to_string().into();
        span["text"] = text.into();
        validate_value(value)
    };
    let mismatch = vec![Violation::SpanUnresolved {
        owner: "C6.O2".into(),
        problem: "the span bytes differ from its quoted text",
    }];
    assert_eq!(resolve(successor_revision, successor_text), []);
    assert_eq!(resolve(successor_revision, original_text), mismatch);
    assert_eq!(resolve(original_revision, successor_text), mismatch);
}

#[test]
fn successor_revisions_keep_identity_and_byte_length() {
    let successor_problem = |problem| Violation::Successor {
        source: "C6.V1".into(),
        message_id: "msg_cf_c6_02".into(),
        problem,
    };
    let successor = ["cases", "5", "sources", "0", "successors", "0"];
    assert_eq!(
        with_appended(&[&successor[..], &["parts", "1", "text"]].concat(), " "),
        [successor_problem("a successor text keeps its byte length")]
    );

    let mut value = corpus_value();
    let original = case_mut(&mut value, 5)["sources"][0]["messages"][1]["parts"][1]["text"].clone();
    case_mut(&mut value, 5)["sources"][0]["successors"][0]["parts"][1]["text"] = original;
    assert_eq!(
        validate_value(value),
        [successor_problem("a successor changes some text")]
    );

    let mut value = corpus_value();
    let completed =
        case_mut(&mut value, 5)["sources"][0]["messages"][1]["info"]["time"]["completed"].clone();
    case_mut(&mut value, 5)["sources"][0]["successors"][0]["info"]["time"]["completed"] = completed;
    assert_eq!(
        validate_value(value),
        [successor_problem(
            "a successor's revision is later than the original's"
        )]
    );

    let mut value = corpus_value();
    case_mut(&mut value, 5)["sources"][0]["successors"][0]["parts"][0]["state"]["output"] =
        "changed".into();
    assert_eq!(
        validate_value(value),
        [successor_problem(
            "a successor keeps each part's identity and every tool part"
        )]
    );

    let mut value = corpus_value();
    let successors = case_mut(&mut value, 5)["sources"][0]["successors"]
        .as_array_mut()
        .unwrap();
    let mut conflicting = successors[0].clone();
    let text = conflicting["parts"][1]["text"].as_str().unwrap();
    conflicting["parts"][1]["text"] = text
        .replacen("4095 tests run: 4095", "4094 tests run: 4094", 1)
        .into();
    assert_ne!(conflicting, successors[0]);
    successors.push(conflicting);
    assert_eq!(
        validate_value(value),
        [successor_problem(
            "a block has one byte string at each revision"
        )]
    );
}

#[test]
fn native_records_keep_their_session_revision_and_settled_tools() {
    let message_problem = |message_id: &str, problem| Violation::Message {
        source: "C1.V1".into(),
        message_id: message_id.into(),
        problem,
    };
    let mut value = corpus_value();
    case_mut(&mut value, 0)["sources"][0]["messages"][0]["info"]["sessionID"] = "ses_other".into();
    assert_eq!(
        validate_value(value),
        [message_problem(
            "msg_cf_c1_01",
            "the record names another session"
        )]
    );

    let mut value = corpus_value();
    let time = &mut case_mut(&mut value, 0)["sources"][0]["messages"][4]["info"]["time"];
    time.as_object_mut().unwrap().remove("completed");
    assert!(validate_value(value).contains(&message_problem(
        "msg_cf_c1_05",
        "an assistant record is completed, so its text has a revision"
    )));

    let mut value = corpus_value();
    let state = &mut case_mut(&mut value, 0)["sources"][0]["messages"][3]["parts"][0]["state"];
    state["status"] = "error".into();
    assert!(validate_value(value).contains(&message_problem(
        "msg_cf_c1_04",
        "a tool part is settled with one output or error string"
    )));
}

#[test]
fn every_case_requires_p1_to_p4_and_a_pressure_or_omission_scenario() {
    use Tier::{P1, P2, P3, P4};
    for (tier, index) in [(P1, 0), (P2, 1), (P3, 2), (P4, 3)] {
        let mut value = corpus_value();
        case_mut(&mut value, 0)["scenarios"]
            .as_array_mut()
            .unwrap()
            .remove(index);
        assert_eq!(
            validate_value(value),
            [Violation::MissingTier {
                case: "C1".into(),
                tier
            }],
            "{tier:?}"
        );
    }
    let mut value = corpus_value();
    case_mut(&mut value, 0)["scenarios"][0]["serving"]["stage"] = "m0".into();
    assert_eq!(
        validate_value(value),
        [Violation::MissingTier {
            case: "C1".into(),
            tier: P1
        }],
        "P1 counts only in the m1 window"
    );
    for (tier, index) in [(P2, 1), (P3, 2), (P4, 3)] {
        let mut value = corpus_value();
        case_mut(&mut value, 0)["scenarios"][index]["serving"]["stage"] = "m1".into();
        assert_eq!(
            validate_value(value),
            [Violation::MissingTier {
                case: "C1".into(),
                tier
            }],
            "{tier:?} counts only in the m0 window"
        );
    }
    let mut value = corpus_value();
    case_mut(&mut value, 4)["scenarios"]
        .as_array_mut()
        .unwrap()
        .remove(4);
    assert_eq!(
        validate_value(value),
        [Violation::MissingPressureOrOmission("C5".into())]
    );
}

#[test]
fn serving_paths_name_a_consistent_tier_and_stage() {
    let serving_problem = |id: &str, problem| {
        vec![Violation::Scenario {
            id: id.into(),
            problem,
        }]
    };
    let mutated = |case: usize, scenario: usize, serving: Value| {
        let mut value = corpus_value();
        case_mut(&mut value, case)["scenarios"][scenario]["serving"] = serving;
        validate_value(value)
    };
    assert_eq!(
        mutated(
            4,
            4,
            json!({"path": "omission", "tier": "p4", "stage": "m0"})
        ),
        serving_problem("C5.S5", "an omission scenario serves the segment at P5")
    );
    assert_eq!(
        mutated(5, 5, json!({"path": "exact_read", "tier": "p1"})),
        serving_problem(
            "C6.S6",
            "an exact read is internal and names no served tier or stage"
        )
    );
    assert_eq!(
        mutated(4, 4, json!({"path": "omission", "tier": "p5"})),
        serving_problem("C5.S5", "a served scenario names its tier and stage")
    );
    let mut value = corpus_value();
    case_mut(&mut value, 1)["scenarios"][5]["serving"]["tier"] = "p5".into();
    assert_eq!(
        validate_value(value),
        serving_problem(
            "C2.S6",
            "natural serving shows segment text, so its tier is P1 to P4"
        )
    );
}

#[test]
fn answer_keys_and_evaluator_labels_stay_out_of_provider_input() {
    let statement = corpus().cases[0].obligations[0].statement.clone();
    assert_eq!(
        with_appended(
            &["cases", "0", "follow_ups", "0", "prompt"],
            &format!(" {statement}")
        ),
        [leaked("C1.F1", &statement)]
    );
    let prompt = ["cases", "0", "follow_ups", "0", "prompt"];
    let title = corpus().cases[0].title.clone();
    assert_eq!(
        with_appended(&prompt, &format!(" {title}")),
        [leaked("C1.F1", &title)]
    );
    let note = corpus().note.clone();
    let violations = with_appended(&prompt, &format!(" {note}"));
    assert!(
        violations.contains(&leaked("C1.F1", &note)),
        "{violations:?}"
    );
    let mut value = corpus_value();
    let reference = "INC-4242 store outage";
    case_mut(&mut value, 0)["provenance"] = json!({"kind": "incident", "reference": reference});
    let text = text_mut(&mut value, &prompt);
    *text = format!("{} {reference}", text.as_str().unwrap()).into();
    assert_eq!(validate_value(value), [leaked("C1.F1", reference)]);
    assert_eq!(
        with_appended(
            &["cases", "0", "follow_ups", "0", "prompt"],
            " nonmateriality"
        ),
        [
            leaked("C1.F1", "materiality"),
            leaked("C1.F1", "nonmaterial")
        ],
        "overlapping labels each leak, in label order"
    );
    let c1_part = ["cases", "0", "sources", "0", "messages", "2", "parts", "0"];
    assert_eq!(
        with_appended(&[&c1_part[..], &["text"]].concat(), " (see C1.O1)"),
        [
            leaked("msg_cf_c1_03#0", "C1"),
            leaked("msg_cf_c1_03#0", "C1.O1")
        ],
        "a case id inside a longer label leaks on its own"
    );
    assert_eq!(
        with_appended(&prompt, " C1"),
        [leaked("C1.F1", "C1")],
        "a case id is an evaluator label"
    );
    let c1_call = [
        "cases", "0", "sources", "0", "messages", "3", "parts", "0", "callID",
    ];
    assert_eq!(
        with_appended(&c1_call, "_C1.O1"),
        [
            leaked("msg_cf_c1_04#0", "C1"),
            leaked("msg_cf_c1_04#0", "C1.O1")
        ],
        "a tool call id is replayed as the provider-visible tool_use id"
    );
    let c1_tool = [
        "cases", "0", "sources", "0", "messages", "3", "parts", "0", "state",
    ];
    assert_eq!(
        with_appended(
            &[&c1_tool[..], &["output"]].concat(),
            "materiality: material"
        ),
        [leaked("msg_cf_c1_04#0", "materiality")]
    );
    assert_eq!(
        with_appended(&[&c1_tool[..], &["input", "filePath"]].concat(), "#C1.S2"),
        [
            leaked("msg_cf_c1_04#0", "C1"),
            leaked("msg_cf_c1_04#0", "C1.S2")
        ]
    );
    let mut value = corpus_value();
    text_mut(&mut value, &[&c1_tool[..], &["input"]].concat())["disposition"] = "visible".into();
    assert_eq!(
        validate_value(value),
        [leaked("msg_cf_c1_04#0", "disposition")]
    );
    assert_eq!(
        with_appended(
            &["cases", "2", "memory_examples", "0", "text"],
            " Answer key: 64 MiB."
        ),
        [leaked("C3.M1", "answer key")]
    );
    let mut value = corpus_value();
    let output = &mut case_mut(&mut value, 1)["sources"][0]["approved_example"];
    *output = output
        .as_str()
        .unwrap()
        .replacen("<p3>", "<p3>Disposition: visible. ", 1)
        .into();
    assert_eq!(
        validate_value(value),
        [leaked("C2.V1.approved_example", "disposition")]
    );

    let mut value = corpus_value();
    text_mut(&mut value, &c1_part)["expected"] = "pooling-first rejected".into();
    let error = serde_json::from_value::<Corpus>(value).unwrap_err();
    assert!(error.to_string().contains("unknown field"), "{error}");
    let mut value = corpus_value();
    case_mut(&mut value, 0)["follow_ups"][0]["answer"] = "no".into();
    assert!(serde_json::from_value::<Corpus>(value).is_err());
}

#[test]
fn scenario_expectations_and_entries_are_consistent() {
    let mut value = corpus_value();
    case_mut(&mut value, 0)["scenarios"][0]["expectations"][0]["accepted"] = json!(["unavailable"]);
    assert_eq!(
        validate_value(value),
        [Violation::Scenario {
            id: "C1.S1".into(),
            problem: "unavailable evidence is acceptable only where abstention is permitted",
        }]
    );

    let mut value = corpus_value();
    for scenario in case_mut(&mut value, 0)["scenarios"].as_array_mut().unwrap() {
        scenario["expectations"]
            .as_array_mut()
            .unwrap()
            .retain(|e| e["obligation"] != "C1.O3");
    }
    assert_eq!(
        validate_value(value),
        [Violation::UnexercisedObligation("C1.O3".into())]
    );

    let mut value = corpus_value();
    case_mut(&mut value, 1)["scenarios"][5]["follow_up"] = "C2.F1".into();
    assert_eq!(
        validate_value(value),
        [Violation::Scenario {
            id: "C2.S6".into(),
            problem: "the follow-up belongs to another source",
        }]
    );

    let mut value = corpus_value();
    for forbidden in case_mut(&mut value, 3)["forbidden_conclusions"]
        .as_array_mut()
        .unwrap()
    {
        forbidden["class"] = "recall".into();
    }
    assert_eq!(
        validate_value(value),
        [Violation::MissingFalseTransition("C4".into())]
    );

    let mut value = corpus_value();
    case_mut(&mut value, 2)["memory_examples"][0]["category"] = "SESSION_NOTES".into();
    assert_eq!(
        validate_value(value),
        [Violation::Entry {
            id: "C3.M1".into(),
            problem: "a memory example is nonempty project memory in a promotable category",
        }]
    );

    let mut value = corpus_value();
    for case in value["cases"].as_array_mut().unwrap() {
        for obligation in case["obligations"].as_array_mut().unwrap() {
            if obligation["kind"] == "constraint" {
                obligation["kind"] = "decision".into();
            }
        }
    }
    assert_eq!(
        validate_value(value),
        [Violation::MissingObligationKind(ObligationKind::Constraint)]
    );

    let mut value = corpus_value();
    case_mut(&mut value, 0)["obligations"][0]["materiality"] = "nonmaterial".into();
    assert_eq!(
        validate_value(value),
        [Violation::Entry {
            id: "C1.O1".into(),
            problem: "an obligation is material; nonmaterial detail is an allowed loss",
        }]
    );
}

#[test]
fn references_resolve_within_their_case() {
    let unknown = |from: &str, to: &str| {
        vec![Violation::UnknownReference {
            from: from.into(),
            to: to.into(),
        }]
    };
    let mut value = corpus_value();
    case_mut(&mut value, 0)["scenarios"][0]["forbidden"][0] = "C1.X99".into();
    assert_eq!(validate_value(value), unknown("C1.S1", "C1.X99"));

    let mut value = corpus_value();
    case_mut(&mut value, 0)["scenarios"][0]["expectations"][0]["obligation"] = "C1.O99".into();
    let violations = validate_value(value);
    assert!(
        violations.contains(&unknown("C1.S1", "C1.O99")[0]),
        "{violations:?}"
    );

    let mut value = corpus_value();
    case_mut(&mut value, 0)["obligations"][0]["evidence"][0]["source"] = "C1.V9".into();
    assert_eq!(validate_value(value), unknown("C1.O1", "C1.V9"));

    let mut value = corpus_value();
    case_mut(&mut value, 0)["scenarios"][0]["source"] = "C1.V9".into();
    let violations = validate_value(value);
    assert!(
        violations.contains(&unknown("C1.S1", "C1.V9")[0]),
        "{violations:?}"
    );

    let mut value = corpus_value();
    case_mut(&mut value, 0)["follow_ups"][0]["source"] = "C1.V9".into();
    let violations = validate_value(value);
    assert!(
        violations.contains(&unknown("C1.F1", "C1.V9")[0]),
        "{violations:?}"
    );

    let mut value = corpus_value();
    case_mut(&mut value, 2)["obligations"][3]["memory"] = "C3.M9".into();
    assert_eq!(validate_value(value), unknown("C3.O4", "C3.M9"));
}

#[test]
fn allowed_losses_and_memory_obligations_keep_their_entry_rules() {
    let entry_problem = |id: &str, problem| {
        vec![Violation::Entry {
            id: id.into(),
            problem,
        }]
    };
    let mut value = corpus_value();
    case_mut(&mut value, 0)["allowed_losses"][0]["materiality"] = "material".into();
    assert_eq!(
        validate_value(value),
        entry_problem("C1.L1", "an allowed loss is marked nonmaterial")
    );
    let mut value = corpus_value();
    case_mut(&mut value, 0)["allowed_losses"][0]["evidence"] = json!([]);
    assert_eq!(
        validate_value(value),
        entry_problem(
            "C1.L1",
            "an allowed loss names the native detail it permits losing"
        )
    );
    let mut value = corpus_value();
    case_mut(&mut value, 0)["allowed_losses"][0]["evidence"][0]["text"] = "about two years".into();
    assert_eq!(
        validate_value(value),
        [Violation::SpanUnresolved {
            owner: "C1.L1".into(),
            problem: "the span bytes differ from its quoted text",
        }]
    );

    let mut value = corpus_value();
    let span = case_mut(&mut value, 2)["obligations"][0]["evidence"][0].clone();
    case_mut(&mut value, 2)["obligations"][3]["evidence"] = json!([span]);
    assert_eq!(
        validate_value(value),
        entry_problem(
            "C3.O4",
            "an obligation cites native evidence or a memory example, not both"
        )
    );
    let mut value = corpus_value();
    case_mut(&mut value, 2)["obligations"][3]
        .as_object_mut()
        .unwrap()
        .remove("memory");
    assert_eq!(
        validate_value(value),
        entry_problem(
            "C3.O4",
            "an obligation needs native evidence or a memory example"
        )
    );
}

#[test]
fn native_message_and_part_ids_are_unique_within_a_source() {
    let mut value = corpus_value();
    let messages = &mut case_mut(&mut value, 0)["sources"][0]["messages"];
    messages[1]["info"]["id"] = messages[0]["info"]["id"].clone();
    let violations = validate_value(value);
    assert!(
        violations.contains(&Violation::DuplicateId("msg_cf_c1_01".into())),
        "{violations:?}"
    );

    let mut value = corpus_value();
    let messages = &mut case_mut(&mut value, 0)["sources"][0]["messages"];
    messages[1]["parts"][0]["id"] = messages[0]["parts"][0]["id"].clone();
    assert_eq!(
        validate_value(value),
        [Violation::DuplicateId("prt_cf_c1_01_0".into())]
    );
}

type RuleMutation = (&'static str, fn(&mut Value), Violation);

#[test]
fn every_remaining_rule_rejects_its_own_mutation() {
    let cases: [RuleMutation; 8] = [
        (
            "schema",
            |v| v["schema"] = "eidnara.compression-fidelity-corpus/v0".into(),
            Violation::Header("schema"),
        ),
        (
            "project id",
            |v| v["project_id"] = "not-hex".into(),
            Violation::Header("project_id"),
        ),
        (
            "case title",
            |v| v["cases"][0]["title"] = " ".into(),
            Violation::Case {
                case: "C1".into(),
                problem: "a case has a title",
            },
        ),
        (
            "empty expectations",
            |v| v["cases"][0]["scenarios"][4]["expectations"] = json!([]),
            Violation::Scenario {
                id: "C1.S5".into(),
                problem: "a scenario names the obligations it checks",
            },
        ),
        (
            "duplicate disposition",
            |v| {
                v["cases"][0]["scenarios"][0]["expectations"][0]["accepted"] =
                    json!(["visible", "visible"])
            },
            Violation::Scenario {
                id: "C1.S1".into(),
                problem: "accepted dispositions are nonempty and distinct",
            },
        ),
        (
            "successor without an original",
            |v| v["cases"][5]["sources"][0]["successors"][0]["info"]["id"] = "msg_cf_c6_09".into(),
            Violation::Successor {
                source: "C6.V1".into(),
                message_id: "msg_cf_c6_09".into(),
                problem: "a successor revises a record of its source",
            },
        ),
        (
            "tool settle order",
            |v| {
                v["cases"][0]["sources"][0]["messages"][3]["parts"][0]["state"]["time"]["start"] =
                    u64::MAX.into()
            },
            Violation::Message {
                source: "C1.V1".into(),
                message_id: "msg_cf_c1_04".into(),
                problem: "a tool part is settled with one output or error string",
            },
        ),
        (
            "tool name label",
            |v| {
                v["cases"][0]["sources"][0]["messages"][3]["parts"][0]["tool"] = "abstention".into()
            },
            leaked("msg_cf_c1_04#0", "abstention"),
        ),
    ];
    for (label, mutate, expected) in cases {
        let mut value = corpus_value();
        mutate(&mut value);
        assert_eq!(validate_value(value), [expected], "{label}");
    }
}

#[test]
fn the_opencode_adapter_publishes_every_block_the_corpus_resolves() {
    assert_eq!(adapter_disagreements(&corpus_value()), Vec::<String>::new());
}

#[test]
fn a_revision_the_validator_accepts_but_the_adapter_refuses_is_a_disagreement() {
    let mut value = corpus_value();
    case_mut(&mut value, 0)["sources"][0]["messages"][3]["parts"][0]["state"]["time"]["end"] =
        (i64::MAX as u64 + 1).into();
    assert_eq!(validate_value(value.clone()), []);
    assert_eq!(
        adapter_disagreements(&value),
        ["C1.V1/msg_cf_c1_04: the native state.time.end is not a canonical revision"]
    );
}
