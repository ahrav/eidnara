//! Replays each corpus source's approved producer output through the daemon's real prompt and
//! alias assembly, producer validation, accepted publication, and the m1 and m0 serving paths,
//! and records owner-attributed observations per case, scenario or source, and stage. Expected
//! tiers are the corpus's approved bodies; the renderer is only observed.

use super::compression_fidelity_corpus::*;
use super::compression_fidelity_observation::{OBSERVATIONS_DIR, Observation, Terminal};
use super::*;

use std::ops::Range;

use memory_store::{ExtractionFailure, MemoryReviewerNonadmissionCode};

const OWNER: &str = "daemon.compression_fidelity.replay";
const ESTIMATOR: &str =
    "tokenizer::estimate_tokens (Claude encoding) through token_cache::cached_estimate_tokens";
const HIGH_PRESSURE_USAGE: u64 = 49_000;
const CONTEXT_LIMIT: u64 = 50_000;

mod marker {
    pub const OBLIGATION_TRANSFORMED: &str = "cf_u2_source_obligation_transformed";
    pub const OBLIGATION_ABSENT_AFTER_TRUNCATION: &str =
        "cf_u2_source_obligation_absent_after_truncation";
    pub const TOOL_OUTPUT_OMITTED: &str = "cf_u2_tool_output_obligation_omitted";
    pub const REJECTED_PRIMARY_FALLBACK: &str = "cf_u2_rejected_primary_fallback_consumed";
    pub const FINAL_DISCARD_AFTER_COVERAGE: &str = "cf_u2_final_discard_after_valid_coverage";
    pub const INHERITED_P2_P3: &str = "cf_u2_inherited_p2_p3";
    pub const HIGH_IMPORTANCE_PRESSURE: &str = "cf_u2_high_importance_positive_budget_pressure";
}

fn observation(
    case: &str,
    source: &str,
    stage: impl Into<String>,
    terminal: Terminal,
) -> Observation {
    Observation::new(OWNER, CORPUS_SHA256, case, source, stage, terminal)
}

fn sha256_hex(text: &str) -> String {
    format!("{:x}", sha2::Sha256::digest(text.as_bytes()))
}

/// The corpus scenarios of `source` that serve `tier` on `path` at `stage`.
fn scenarios_serving(
    case: &'static Case,
    source: &Source,
    path: ServingPath,
    tier: Tier,
    stage: Stage,
) -> Vec<&'static Scenario> {
    case.scenarios
        .iter()
        .filter(|s| {
            s.source == source.id
                && s.serving.path == path
                && s.serving.tier == Some(tier)
                && s.serving.stage == Some(stage)
        })
        .collect()
}

/// Emits `record` once per witnessed scenario, or once at source level when none matches.
fn emit_for(record: impl Fn() -> Observation, witnessed: &[&Scenario]) {
    if witnessed.is_empty() {
        record().emit();
    }
    for scenario in witnessed {
        record().scenario(&scenario.id).emit();
    }
}

/// The corpus's raw native records for one source, as the OpenCode codec reads them.
fn native_records(case: &str, source: &str, field: &str) -> Vec<Arc<Value>> {
    let corpus: Value = serde_json::from_slice(CORPUS_BYTES).unwrap();
    corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == case)
        .and_then(|c| c["sources"].as_array())
        .and_then(|sources| sources.iter().find(|s| s["id"] == source))
        .and_then(|s| s[field].as_array())
        .unwrap()
        .iter()
        .cloned()
        .map(Arc::new)
        .collect()
}

fn decoded(records: &[Arc<Value>]) -> Vec<IngressMessage> {
    crate::codec::opencode::decode_opencode_shared(records).messages
}

fn source_ingress(case: &Case, source: &Source) -> Vec<IngressMessage> {
    decoded(&native_records(&case.id, &source.id, "messages"))
}

/// A test-authored live tail: one unrelated message large enough to fill the protected tail,
/// then the case's follow-up. The fold therefore covers exactly the messages before it.
fn live_tail(follow_up: &FollowUp, first_ordinal: u64) -> Vec<IngressMessage> {
    vec![
        ck(
            "cf-tail-pad",
            first_ordinal,
            &format!("unrelated tail {}", "pad ".repeat(3000)),
        ),
        ck("cf-tail-follow-up", first_ordinal + 1, &follow_up.prompt),
    ]
}

/// The four authored tier bodies of an approved example, read from its tags without the
/// validator, so the published row is compared with what the reviewer approved.
fn approved_tiers(approved_example: &str) -> [String; 4] {
    ["p1", "p2", "p3", "p4"].map(|tag| {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        match approved_example.find(&open) {
            Some(at) => {
                let body = &approved_example[at + open.len()..];
                body[..body.find(&close).unwrap()].to_owned()
            }
            None => {
                assert!(approved_example.contains(&format!("<{tag} />")));
                String::new()
            }
        }
    })
}

fn approved_title(approved_example: &str) -> String {
    let at = approved_example.find("title=\"").unwrap() + "title=\"".len();
    let rest = &approved_example[at..];
    rest[..rest.find('"').unwrap()].to_owned()
}

/// How a material native span reached the producer's complete user input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum Exposure {
    Exact,
    /// Present after the presenter's known rewrites only.
    Transformed,
    Absent,
}

/// The presenter's known rewrites of native text: whitespace runs collapse to one space and
/// marker brackets become a plain double quote.
fn collapse(text: &str) -> String {
    text.replace(['\u{ab}', '\u{bb}'], "\"")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn exposure(prompt: &str, span: &str) -> Exposure {
    if prompt.contains(span) {
        Exposure::Exact
    } else if collapse(prompt).contains(&collapse(span)) {
        Exposure::Transformed
    } else {
        Exposure::Absent
    }
}

fn record_of<'s>(source: &'s Source, span: &Span) -> (usize, &'s NativeMessage) {
    source
        .messages
        .iter()
        .enumerate()
        .find(|(_, m)| m.info.id == span.message_id)
        .unwrap()
}

/// The text blocks the presenter renders for `record`, by block index, as the presenter rewrites
/// them: tool blocks and empty text are left out.
fn rendered_blocks(record: &NativeMessage) -> Vec<(usize, String)> {
    record
        .parts
        .iter()
        .enumerate()
        .filter_map(|(index, part)| match part {
            Part::Text { text, .. } => Some(collapse(text))
                .filter(|text| !text.is_empty())
                .map(|text| (index, text)),
            Part::Tool { .. } => None,
        })
        .collect()
}

const PART_SEPARATOR: &str = " / ";

fn rendered_record(record: &NativeMessage) -> String {
    rendered_blocks(record)
        .into_iter()
        .map(|(_, text)| text)
        .collect::<Vec<_>>()
        .join(PART_SEPARATOR)
}

/// The byte range of block `block_index` inside the record's rendered part; `None` for a block
/// the presenter leaves out.
fn rendered_block_range(record: &NativeMessage, block_index: usize) -> Option<Range<usize>> {
    let mut offset = 0;
    for (index, text) in rendered_blocks(record) {
        if index == block_index {
            return Some(offset..offset + text.len());
        }
        offset += text.len() + PART_SEPARATOR.len();
    }
    None
}

/// The parts inside `<new_messages>` in alias order: the bytes after each `«sN»` marker up to
/// the part separator or the next line.
fn presented_parts(prompt: &str) -> Vec<&str> {
    let start = prompt
        .find("<new_messages>")
        .map_or(0, |at| at + "<new_messages>".len());
    let body = &prompt[start..];
    let body = &body[..body.find("</new_messages>").unwrap_or(body.len())];
    let marker = |n: usize| crate::history_summarizer_chunk::alias_marker(&format!("s{n}"));
    let mut parts = Vec::new();
    let mut cursor = 0;
    for n in 1.. {
        let Some(at) = body[cursor..].find(&marker(n)) else {
            break;
        };
        let text_start = cursor + at + marker(n).len();
        let text_end = body[text_start..]
            .find(&marker(n + 1))
            .map_or(body.len(), |at| text_start + at);
        let line = body[text_start..text_end].split('\n').next().unwrap();
        parts.push(line.strip_suffix(" / ").unwrap_or(line));
        cursor = text_end;
    }
    parts
}

/// Whether the span's message reached the prompt whole, and the span's exposure inside the
/// bytes presenting its annotated block. The message's part is the one at its position among the
/// source's messages holding the message's rendered text.
fn span_exposure(parts: &[&str], source: &Source, span: &Span) -> (bool, Exposure) {
    let (position, record) = record_of(source, span);
    let presented = parts
        .get(position)
        .copied()
        .filter(|part| *part == rendered_record(record));
    let seen = presented
        .zip(rendered_block_range(record, span.block_index))
        .map_or(Exposure::Absent, |(part, range)| {
            exposure(&part[range], &span.text)
        });
    (presented.is_some(), seen)
}

/// One exposure entry per material native span of `source`, in one shape for every stage.
fn exposures(case: &Case, source: &Source, prompt: &str) -> Vec<(Exposure, bool, bool, Value)> {
    let parts = presented_parts(prompt);
    let mut entries = Vec::new();
    for obligation in case.obligations.iter().filter(|o| o.memory.is_none()) {
        for span in obligation.evidence.iter().filter(|s| s.source == source.id) {
            let (_, record) = record_of(source, span);
            let tool = matches!(record.parts[span.block_index], Part::Tool { .. });
            let (text_presented, seen) = span_exposure(&parts, source, span);
            entries.push((
                seen,
                tool,
                text_presented,
                json!({
                    "obligation": obligation.id,
                    "message_id": span.message_id,
                    "block_index": span.block_index,
                    "tool_output": tool,
                    "message_text_presented": text_presented,
                    "exposure": seen,
                    "generation_credit": seen != Exposure::Absent,
                }),
            ));
        }
    }
    entries
}

/// Asserts the attempt carries a real assembled prompt and model, the precondition every
/// generation situation needs.
fn assert_real_generation(attempt: &ProducerAttempt) {
    assert_eq!(
        attempt.system,
        crate::history_summarizer_prompt::HISTORY_SUMMARIZER_SYSTEM_PROMPT
    );
    assert!(!attempt.model.is_empty());
    assert!(
        attempt.prompt.contains("<new_messages>") && attempt.prompt.contains("\u{ab}s1\u{bb}"),
        "a placeholder prompt witnesses nothing"
    );
}

struct Fold {
    handler: Handler,
    store: Arc<MemoryStore>,
    producer: Arc<ProducerState>,
    _dir: tempfile::TempDir,
    messages: Vec<IngressMessage>,
}

impl Fold {
    async fn pass(&self, history_budget_tokens: Option<f64>, render_config: &str) -> Value {
        let mut request =
            request_with_usage(self.messages.clone(), HIGH_PRESSURE_USAGE, CONTEXT_LIMIT);
        request["render_config"] = json!(render_config);
        if let Some(budget) = history_budget_tokens {
            request["history_budget_tokens"] = json!(budget);
        }
        call_transform_request(&self.handler, request).await
    }

    fn attempts(&self) -> Vec<ProducerAttempt> {
        self.producer.attempts.lock().unwrap().clone()
    }

    fn rows(&self) -> Vec<StoredHistorySegment> {
        self.store.load_history_segments("ses").unwrap()
    }
}

fn scripted(outputs: Vec<String>) -> Arc<ProducerState> {
    let producer = Arc::new(ProducerState::default());
    producer
        .await_results
        .lock()
        .unwrap()
        .extend(outputs.into_iter().map(|text| {
            Ok(ProducerOutput {
                text,
                length_capped: false,
            })
        }));
    producer
}

/// Folds `messages ++ live tail` once, scripting `outputs` as the producer's answers, and waits
/// for the firing to settle.
async fn fold_with(
    config: DaemonConfig,
    mut messages: Vec<IngressMessage>,
    follow_up: &FollowUp,
    outputs: Vec<String>,
) -> Fold {
    let producer = scripted(outputs);
    let (handler, store, dir, _project) = handler_with_store(Arc::clone(&producer), config);
    let next = messages.len() as u64 + 1;
    messages.extend(live_tail(follow_up, next));
    let fold = Fold {
        handler,
        store,
        producer,
        _dir: dir,
        messages,
    };
    let first = fold.pass(None, "cfg0").await;
    assert_eq!(first["history_summarizer"]["fired"], true, "{first}");
    wait_for_count(&fold.producer.starts, 1).await;
    wait_for_idle(&fold.store).await;
    fold
}

async fn approved_fold(case: &Case, source: &Source) -> Fold {
    fold_with(
        default_test_config(),
        source_ingress(case, source),
        follow_up_for(case, source),
        vec![source.approved_example.clone()],
    )
    .await
}

fn follow_up_for<'c>(case: &'c Case, source: &Source) -> &'c FollowUp {
    case.follow_ups
        .iter()
        .find(|f| f.source == source.id)
        .unwrap()
}

fn sources() -> impl Iterator<Item = (&'static Case, &'static Source)> {
    corpus()
        .cases
        .iter()
        .flat_map(|case| case.sources.iter().map(move |source| (case, source)))
}

fn case_source(case: &str, source: &str) -> (&'static Case, &'static Source) {
    let case = corpus().case(case).unwrap();
    (case, case.source(source).unwrap())
}

/// The serialized content blocks a serving response delivers outside its synthetic m0/m1
/// blocks.
fn live_texts(response: &Value) -> String {
    response["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["meta"]["synthetic"] != json!(true))
        .flat_map(|message| {
            message["content"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|block| block.to_string())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every native string a covered source contributes: text parts and settled tool outputs.
fn covered_strings(source: &Source) -> Vec<&str> {
    source
        .messages
        .iter()
        .flat_map(|message| message.parts.iter().filter_map(block_text))
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn approved_examples_publish_through_real_validation_and_replace_covered_input() {
    let mut fired: BTreeSet<&str> = BTreeSet::new();
    for (case, source) in sources() {
        let follow_up = follow_up_for(case, source);
        let fold = approved_fold(case, source).await;
        let count = source.messages.len() as u64;

        let attempts = fold.attempts();
        let [attempt] = attempts.as_slice() else {
            panic!("{}: one attempt", source.id);
        };
        assert_real_generation(attempt);
        assert_eq!(attempt.model, "test/model", "{}", source.id);
        assert!(
            attempt
                .session_id
                .starts_with("eidnara-history_summarizer:"),
            "{}: the producer runs in its own summarizer session",
            source.id
        );
        assert_eq!(
            prompt_ordinal_range(&attempt.prompt),
            Some((1, count)),
            "{}: the chunk is exactly the corpus source",
            source.id
        );

        let entries = exposures(case, source, &attempt.prompt);
        let mut markers = Vec::new();
        for (seen, tool, text_presented, entry) in &entries {
            if *tool && *seen == Exposure::Absent && *text_presented {
                markers.push(marker::TOOL_OUTPUT_OMITTED);
            }
            if *seen == Exposure::Transformed {
                markers.push(marker::OBLIGATION_TRANSFORMED);
            }
            assert!(
                *tool || *seen != Exposure::Absent,
                "{}: a text obligation reaches the producer: {entry}",
                source.id
            );
        }

        let rows = fold.rows();
        let [row] = rows.as_slice() else {
            panic!("{}: one published row", source.id);
        };
        let [p1, p2, p3, p4] = approved_tiers(&source.approved_example);
        assert_eq!(
            (row.start_message, row.end_message),
            (1, count as i64),
            "{}",
            source.id
        );
        assert_eq!(row.title, approved_title(&source.approved_example));
        assert_eq!(row.p1.as_deref(), Some(p1.as_str()), "{}", source.id);
        assert_eq!(row.p2.as_deref(), Some(p2.as_str()), "{}", source.id);
        assert_eq!(row.p3.as_deref(), Some(p3.as_str()), "{}", source.id);
        assert_eq!(row.p4.as_deref(), Some(p4.as_str()), "{}", source.id);

        let served = fold.pass(None, "cfg0").await;
        let m0 = m0_text(&served);
        assert!(m0.contains(&row.title), "{}: {m0}", source.id);
        assert!(
            m0.contains(&p1),
            "{}: the newest row serves P1 at m0",
            source.id
        );
        let live = live_texts(&served);
        for text in covered_strings(source) {
            assert!(
                !live.contains(&serde_json::to_string(text).unwrap()),
                "{}: covered bytes {text:?} still reach live input",
                source.id
            );
        }
        assert!(
            live.contains(&serde_json::to_string(&follow_up.prompt).unwrap()),
            "{}: the follow-up stays live",
            source.id
        );

        markers.sort_unstable();
        markers.dedup();
        fired.extend(markers.iter().copied());
        markers
            .into_iter()
            .fold(
                observation(&case.id, &source.id, "generation", Terminal::Published),
                Observation::mark,
            )
            .with(json!({
                "attempts": [{
                    "attempt": 1,
                    "model": attempt.model,
                    "system_sha256": sha256_hex(&attempt.system),
                    "prompt_sha256": sha256_hex(&attempt.prompt),
                    "output_sha256": sha256_hex(&source.approved_example),
                    "output_origin": "scripted approved example",
                }],
                "chunk": [1, count],
                "authored_tiers": {"p1": true, "p2": true, "p3": true, "p4_empty": p4.is_empty()},
                "published_tiers_match_approved": true,
                "exposures": entries.iter().map(|e| e.3.clone()).collect::<Vec<_>>(),
                "covered_input_replaced": true,
            }))
            .emit();
        observation(&case.id, &source.id, "m0", Terminal::Served)
            .with(json!({"tier": "p1", "path": "natural", "first_fold_direct_to_m0": true}))
            .emit();
    }
    for situation in [marker::OBLIGATION_TRANSFORMED, marker::TOOL_OUTPUT_OMITTED] {
        assert!(fired.contains(situation), "{situation} never fired");
    }
}

fn translated(output: &str, offset: u64) -> String {
    use regex::{Captures, Regex};
    let shift = |digits: &str| digits.parse::<u64>().unwrap() + offset;
    let attribute = Regex::new(r#"\b(start|end)="(\d+)""#).unwrap();
    let opener = Regex::new(r"<history_segment\s[^>]*>").unwrap();
    let processed = Regex::new(r"<messages_processed>(\d+)-(\d+)</messages_processed>").unwrap();
    let unprocessed = Regex::new(r"<unprocessed_from>(\d+)</unprocessed_from>").unwrap();
    let output = opener.replace_all(output, |tag: &Captures| {
        attribute
            .replace_all(&tag[0], |pair: &Captures| {
                format!("{}=\"{}\"", &pair[1], shift(&pair[2]))
            })
            .into_owned()
    });
    let output = processed.replace_all(&output, |range: &Captures| {
        format!(
            "<messages_processed>{}-{}</messages_processed>",
            shift(&range[1]),
            shift(&range[2])
        )
    });
    unprocessed
        .replace_all(&output, |from: &Captures| {
            format!("<unprocessed_from>{}</unprocessed_from>", shift(&from[1]))
        })
        .into_owned()
}

#[test]
fn translated_shifts_every_ordinal_the_validator_reads() {
    let output = "<output><history_segments><history_segment start=\"1\" end=\"4\" title=\"T\" importance=\"50\"><p1>x</p1><p2>x</p2><p3>x</p3><p4 /></history_segment></history_segments><meta><messages_processed>1-4</messages_processed><unprocessed_from>5</unprocessed_from></meta></output>";
    assert_eq!(
        translated(output, 3),
        "<output><history_segments><history_segment start=\"4\" end=\"7\" title=\"T\" importance=\"50\"><p1>x</p1><p2>x</p2><p3>x</p3><p4 /></history_segment></history_segments><meta><messages_processed>4-7</messages_processed><unprocessed_from>8</unprocessed_from></meta></output>"
    );
}

const SCAFFOLD_MESSAGES: u64 = 3;

/// Test-authored messages and their scripted segment, at ordinals `first..first + 3`.
fn scaffold(first: u64, title: &str) -> (Vec<IngressMessage>, String) {
    let messages = (0..SCAFFOLD_MESSAGES)
        .map(|k| {
            ck(
                &format!("cf-scaffold-{}", first + k),
                first + k,
                &format!(
                    "Scaffold note {}: {}",
                    first + k,
                    "the workspace builds cleanly. ".repeat(40)
                ),
            )
        })
        .collect();
    let last = first + SCAFFOLD_MESSAGES - 1;
    let output = format!(
        "<output><history_segments><history_segment start=\"{first}\" end=\"{last}\" title=\"{title}\" episode_type=\"infra\" importance=\"40\"><p1>Test-authored {title}: the workspace builds cleanly.</p1><p2>{title}: workspace builds.</p2><p3>{title}.</p3><p4 /></history_segment></history_segments><meta><messages_processed>{first}-{last}</messages_processed></meta></output>"
    );
    (messages, output)
}

/// Folds `before`, then `before ++ added` as a second firing, through one handler.
async fn two_folds(
    before: Vec<IngressMessage>,
    added: Vec<IngressMessage>,
    follow_up: &FollowUp,
    outputs: [String; 2],
) -> Fold {
    let producer = scripted(outputs.into());
    let (handler, store, dir, _project) =
        handler_with_store(Arc::clone(&producer), default_test_config());
    let mut first = before.clone();
    first.extend(live_tail(follow_up, before.len() as u64 + 1));
    let mut fold = Fold {
        handler,
        store,
        producer,
        _dir: dir,
        messages: first,
    };
    fold.pass(None, "cfg0").await;
    wait_for_count(&fold.producer.starts, 1).await;
    wait_for_idle(&fold.store).await;
    let settled = fold.pass(None, "cfg0").await;
    assert!(!m0_text(&settled).is_empty());
    let mut second = before;
    second.extend(added);
    second.extend(live_tail(follow_up, second.len() as u64 + 1));
    fold.messages = second;
    let fired = fold.pass(None, "cfg0").await;
    assert_eq!(fired["history_summarizer"]["fired"], true, "{fired}");
    wait_for_count(&fold.producer.starts, 2).await;
    wait_for_idle(&fold.store).await;
    assert_eq!(fold.rows().len(), 2);
    fold
}

#[tokio::test(flavor = "current_thread")]
async fn a_publication_after_a_prior_baseline_serves_p1_in_the_m1_window() {
    for (case, source) in sources() {
        let follow_up = follow_up_for(case, source);
        let (baseline, baseline_output) = scaffold(1, "Baseline workspace setup");
        let shifted = source_ingress(case, source)
            .into_iter()
            .map(|mut message| {
                message.ordinal += SCAFFOLD_MESSAGES;
                message
            })
            .collect();
        let fold = two_folds(
            baseline,
            shifted,
            follow_up,
            [
                baseline_output,
                translated(&source.approved_example, SCAFFOLD_MESSAGES),
            ],
        )
        .await;
        let served = fold.pass(None, "cfg0").await;
        let [p1, ..] = approved_tiers(&source.approved_example);
        let m1 = synthetic_text(&served, 1);
        assert!(m1.contains(&p1), "{}: P1 rides m1: {m1}", source.id);
        let m0 = m0_text(&served);
        assert!(m0.contains("Baseline workspace setup"), "{m0}");
        assert!(
            !m0.contains(&p1),
            "{}: m0 stays frozen at the baseline",
            source.id
        );
        let witnessed = scenarios_serving(case, source, ServingPath::Natural, Tier::P1, Stage::M1);
        emit_for(
            || {
                observation(&case.id, &source.id, "m1", Terminal::Served).with(json!({
                    "tier": "p1",
                    "path": "natural",
                    "baseline": "test-authored, earns no fidelity credit",
                    "ordinal_offset": SCAFFOLD_MESSAGES,
                    "action": served["action"],
                }))
            },
            &witnessed,
        );
    }
}

/// Heading lookup requires body lines beginning with `## ` to be indented.
fn segment_of<'r>(rendered: &'r str, row: &StoredHistorySegment) -> Option<&'r str> {
    let heading = format!("## {}-{} · ", row.start_message, row.end_message);
    let at = if rendered.starts_with(&heading) {
        0
    } else {
        rendered.find(&format!("\n{heading}"))? + 1
    };
    let segment = &rendered[at..];
    Some(segment[..segment.find("\n\n## ").unwrap_or(segment.len())].trim_end())
}

fn served_body<'r>(rendered: &'r str, row: &StoredHistorySegment) -> Option<&'r str> {
    let segment = segment_of(rendered, row)?;
    Some(segment.split_once('\n').map_or("", |(_, body)| body))
}

/// `bodies` must contain each tier's expected body in renderer output format.
fn served_tier(rendered: &str, row: &StoredHistorySegment, bodies: &[String; 4]) -> Tier {
    let Some(body) = served_body(rendered, row) else {
        return Tier::P5;
    };
    let carried: Vec<Tier> = TIERS
        .into_iter()
        .zip(bodies)
        .filter(|(_, expected)| expected.as_str() == body)
        .map(|(tier, _)| tier)
        .collect();
    match carried.as_slice() {
        [tier] => *tier,
        other => panic!("{}: ambiguous serving {other:?}: {body}", row.title),
    }
}

const TIERS: [Tier; 5] = [Tier::P1, Tier::P2, Tier::P3, Tier::P4, Tier::P5];

fn rank(tier: Tier) -> usize {
    TIERS.iter().position(|t| *t == tier).unwrap()
}

fn filler_row(sequence: i64, ordinal: i64) -> StoredHistorySegment {
    StoredHistorySegment {
        sequence,
        start_message: ordinal,
        end_message: ordinal,
        end_message_id: format!("cf-filler-{ordinal}#0"),
        title: format!("Filler {sequence}"),
        content: format!("Test-authored newer work {sequence}."),
        p1: Some(format!("Test-authored newer work {sequence}.")),
        p2: Some(format!("Newer work {sequence}.")),
        p3: Some(format!("Work {sequence}.")),
        p4: Some(String::new()),
        importance: 50,
        ..Default::default()
    }
}

fn render_natural(rows: &[StoredHistorySegment]) -> String {
    crate::decay_render::render_stored_history_segments(rows, 0.0, |text: &str| {
        crate::token_cache::cached_estimate_tokens(text)
    })
}

/// `row` followed by `newer` test-authored rows, rendered at the natural curve (budget 0).
fn render_aged(row: &StoredHistorySegment, newer: i64) -> String {
    let mut rows = vec![row.clone()];
    rows.extend((1..=newer).map(|k| filler_row(row.sequence + k, row.end_message + k)));
    render_natural(&rows)
}

/// Each tier's first age: the least number of newer rows under which the natural curve serves
/// `row` at that tier. The probe uses distinct tier bodies so `served_tier` can distinguish
/// tiers even when `bodies`, the row's own tiers as rendered, coincide. The search requires the
/// probe's rank to be nondecreasing in the number of newer rows.
fn first_ages(row: &StoredHistorySegment, bodies: &[String; 4], id: &str) -> [i64; 5] {
    const MAX_NEWER: i64 = crate::decay_render::PRESSURE_WINDOW as i64 + 2;
    let probe_bodies = ["P1", "P2", "P3", "P4"].map(|tier| format!("Probe {tier} body."));
    let [p1, p2, p3, p4] = probe_bodies.clone();
    let probe = StoredHistorySegment {
        content: p1.clone(),
        p1: Some(p1),
        p2: Some(p2),
        p3: Some(p3),
        p4: Some(p4),
        ..row.clone()
    };
    let mut rows = vec![probe];
    let mut ranks: Vec<Option<usize>> = Vec::new();
    let mut rank_at = |newer: i64| -> usize {
        let at = usize::try_from(newer).unwrap();
        while rows.len() <= at {
            let k = rows.len() as i64;
            rows.push(filler_row(row.sequence + k, row.end_message + k));
        }
        if ranks.len() <= at {
            ranks.resize(at + 1, None);
        }
        *ranks[at].get_or_insert_with(|| {
            rank(served_tier(
                &render_natural(&rows[..=at]),
                row,
                &probe_bodies,
            ))
        })
    };
    let mut first = [0i64; 5];
    let mut below = 0i64;
    for wanted in 1..TIERS.len() {
        let mut step = 1;
        let mut at_or_past = loop {
            let probe = (below + step).min(MAX_NEWER);
            if rank_at(probe) >= wanted {
                break probe;
            }
            if probe == MAX_NEWER {
                panic!("{id}: natural decay never served {:?}", TIERS[wanted]);
            }
            below = probe;
            step *= 2;
        };
        while at_or_past - below > 1 {
            let middle = below + (at_or_past - below) / 2;
            if rank_at(middle) >= wanted {
                at_or_past = middle;
            } else {
                below = middle;
            }
        }
        assert_eq!(
            rank_at(at_or_past),
            wanted,
            "{id}: natural decay never served {:?}",
            TIERS[wanted]
        );
        first[wanted] = at_or_past;
        below = at_or_past;
    }
    assert_eq!(rank_at(0), 0, "{id}: the fresh row serves P1");
    rows[0] = row.clone();
    let serves = |newer: i64, tier: Tier| {
        let rendered = render_natural(&rows[..=newer as usize]);
        let expected = (tier != Tier::P5).then(|| bodies[rank(tier)].as_str());
        assert_eq!(
            served_body(&rendered, row),
            expected,
            "{id}: {tier:?} at {newer} newer rows"
        );
    };
    for (index, (tier, age)) in TIERS.into_iter().zip(first).enumerate() {
        serves(age, tier);
        if index > 0 {
            serves(age - 1, TIERS[index - 1]);
        }
    }
    first
}

#[test]
fn first_ages_match_an_exhaustive_scan_at_every_decay_rate() {
    let approved = [
        "Alpha ships the first body in full.".to_owned(),
        "Alpha ships the body.".to_owned(),
        "Alpha ships.".to_owned(),
        String::new(),
    ];
    for importance in [1, 25, 50, 75, 100] {
        let row = StoredHistorySegment {
            sequence: 1,
            start_message: 1,
            end_message: 4,
            end_message_id: "cf-alpha-4#0".to_owned(),
            title: "Alpha".to_owned(),
            content: approved[0].clone(),
            p1: Some(approved[0].clone()),
            p2: Some(approved[1].clone()),
            p3: Some(approved[2].clone()),
            p4: Some(String::new()),
            importance,
            ..Default::default()
        };
        let mut scanned: [Option<i64>; 5] = [None; 5];
        let mut rows = vec![row.clone()];
        for newer in 0..=crate::decay_render::PRESSURE_WINDOW as i64 + 2 {
            if newer > 0 {
                rows.push(filler_row(row.sequence + newer, row.end_message + newer));
            }
            let tier = served_tier(&render_natural(&rows), &row, &approved);
            scanned[rank(tier)].get_or_insert(newer);
        }
        let scanned = scanned.map(|age| age.unwrap_or_else(|| panic!("importance {importance}")));
        assert_eq!(
            first_ages(&row, &approved, "alpha"),
            scanned,
            "importance {importance}"
        );
    }
}

#[test]
fn first_ages_hold_for_every_published_tier_shape() {
    let base = StoredHistorySegment {
        sequence: 1,
        start_message: 1,
        end_message: 4,
        end_message_id: "cf-alpha-4#0".to_owned(),
        title: "Alpha".to_owned(),
        content: "Alpha ships the first body in full.".to_owned(),
        p1: Some("Alpha ships the first body in full.".to_owned()),
        p2: Some("Alpha ships the body.".to_owned()),
        p3: Some("Alpha ships.".to_owned()),
        p4: Some(String::new()),
        importance: 60,
        ..Default::default()
    };
    let rendered = |row: &StoredHistorySegment| {
        [&row.p1, &row.p2, &row.p3, &row.p4]
            .map(|body| crate::decay_render::guarded_body(body.as_deref().unwrap_or_default()))
    };
    let expected = first_ages(&base, &rendered(&base), "base");
    let heading = "Alpha ships the first body in full.\n## Details\nAll of it.".to_owned();
    let shapes = [
        (
            "one-sentence P4",
            StoredHistorySegment {
                p4: Some("Alpha shipped.".to_owned()),
                ..base.clone()
            },
        ),
        (
            "P1-only fallback",
            StoredHistorySegment {
                p2: base.p1.clone(),
                p3: base.p1.clone(),
                ..base.clone()
            },
        ),
        (
            "P3 ends P2",
            StoredHistorySegment {
                p2: Some("Alpha ships the body. Alpha ships.".to_owned()),
                ..base.clone()
            },
        ),
        (
            "escaped title",
            StoredHistorySegment {
                title: "Alpha & <beta>".to_owned(),
                ..base.clone()
            },
        ),
        (
            "heading line in P1",
            StoredHistorySegment {
                p1: Some(heading.clone()),
                content: heading,
                ..base.clone()
            },
        ),
    ];
    for (shape, row) in shapes {
        assert_eq!(
            first_ages(&row, &rendered(&row), shape),
            expected,
            "{shape}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn natural_decay_serves_every_approved_tier_and_then_omits_the_segment() {
    for (case, source) in sources() {
        let row = approved_fold(case, source).await.rows().remove(0);
        let approved = approved_tiers(&source.approved_example);
        let ages = first_ages(&row, &approved, &source.id);
        assert!(
            ages.windows(2).all(|pair| pair[0] < pair[1]),
            "{}: tiers decay in order: {ages:?}",
            source.id
        );
        for (tier, age) in TIERS.into_iter().zip(ages) {
            let path = if tier == Tier::P5 {
                ServingPath::Omission
            } else {
                ServingPath::Natural
            };
            let tier_name = format!("{tier:?}").to_lowercase();
            emit_for(
                || {
                    observation(
                        &case.id,
                        &source.id,
                        format!("m0_decay_{tier_name}"),
                        Terminal::Served,
                    )
                    .with(json!({
                        "tier": tier_name,
                        "path": if tier == Tier::P5 { "omission" } else { "natural" },
                        "newer_rows": age,
                        "newer_rows_are": "test-authored filler",
                        "importance": row.importance,
                        "history_budget_tokens": 0,
                        "body_matches_approved": tier != Tier::P5,
                        "title_only": tier == Tier::P4,
                    }))
                },
                &scenarios_serving(case, source, path, tier, Stage::M0),
            );
        }
    }
}

fn session_history(m0: &str) -> String {
    crate::decay_render::extract_m0_block(m0, "session-history").unwrap_or_default()
}

/// The decayed history body inside the wrapped `<session-history>` slice.
fn history_body(slice: &str) -> &str {
    slice
        .strip_prefix("<session-history>\n")
        .and_then(|rest| rest.strip_suffix("\n</session-history>"))
        .unwrap_or("")
}

#[tokio::test(flavor = "current_thread")]
async fn positive_budget_pressure_demotes_the_oldest_row_and_a_generous_budget_retains_it() {
    let estimate = crate::token_cache::cached_estimate_tokens;
    let mut high_importance_pressure = false;
    for (case, scenario) in corpus()
        .cases
        .iter()
        .flat_map(|case| case.scenarios.iter().map(move |s| (case, s)))
        .filter(|(_, s)| s.serving.path == ServingPath::Pressure)
    {
        let source = case.source(&scenario.source).unwrap();
        let target = scenario.serving.tier.unwrap();
        let count = source.messages.len() as u64;
        let newer_title = "Newer scaffold work";
        let (newer, newer_output) = scaffold(count + 1, newer_title);
        let fold = two_folds(
            source_ingress(case, source),
            newer,
            follow_up_for(case, source),
            [source.approved_example.clone(), newer_output.clone()],
        )
        .await;
        let mut rows = fold.rows();
        let newer_row = rows.remove(1);
        let row = rows.remove(0);
        let approved = approved_tiers(&source.approved_example);
        let newer_approved = approved_tiers(&newer_output);

        let generous = fold.pass(Some(60_000.0), "cfg-generous").await;
        let retained = session_history(&m0_text(&generous));
        assert_eq!(
            served_tier(history_body(&retained), &row, &approved),
            Tier::P1,
            "{}",
            scenario.id
        );

        // The search requires the served tier's rank to be nonincreasing as the budget grows.
        let served_at = async |budget: usize, probe: usize| {
            let served = fold
                .pass(Some(budget as f64), &format!("cfg-{probe}"))
                .await;
            let slice = session_history(&m0_text(&served));
            (served_tier(history_body(&slice), &row, &approved), slice)
        };
        let (mut budget, mut high) = (1, estimate(&retained));
        let (mut tier, mut slice) = served_at(budget, 0).await;
        let mut probes = 1;
        while rank(tier) >= rank(target) && high > budget + 1 {
            let middle = budget + (high - budget) / 2;
            let (middle_tier, middle_slice) = served_at(middle, probes).await;
            probes += 1;
            if rank(middle_tier) >= rank(target) {
                (budget, tier, slice) = (middle, middle_tier, middle_slice);
            } else {
                high = middle;
            }
        }
        assert_eq!(
            tier, target,
            "{}: no positive budget served {target:?}",
            scenario.id
        );
        let body = history_body(&slice).to_owned();
        let newer_tier = served_tier(&body, &newer_row, &newer_approved);
        assert!(
            rank(newer_tier) < rank(target),
            "{}: the guard demotes the oldest row first; the newer row serves {newer_tier:?}",
            scenario.id
        );
        assert!(
            estimate(&body) as f64 <= budget as f64,
            "{}: the guarded history body fits its budget",
            scenario.id
        );
        let without_history_text: Vec<&str> = if target == Tier::P5 {
            scenario
                .expectations
                .iter()
                .filter(|e| !e.accepted.contains(&Disposition::Unavailable))
                .map(|e| e.obligation.as_str())
                .collect()
        } else {
            Vec::new()
        };
        let mut record = observation(&case.id, &source.id, "m0_pressure", Terminal::Served)
            .scenario(&scenario.id)
            .with(json!({
                "observed_baseline": {
                    "obligations_left_without_history_text": without_history_text,
                    "accepted_dispositions_need": "visible elsewhere or discoverable; U3 checks recovery",
                },
                "tier": format!("{target:?}").to_lowercase(),
                "path": "pressure",
                "importance": row.importance,
                "newer_row_tier": format!("{newer_tier:?}").to_lowercase(),
                "requested_history_budget_tokens": budget,
                "estimator": ESTIMATOR,
                "history_body_tokens": estimate(&body),
                "wrapped_session_history_tokens": estimate(&slice),
                "wrapped_slice_retries": "not observed by this witness",
                "generous_budget_tokens": 60_000,
                "generous_tier": "p1",
            }));
        if row.importance >= 80 {
            high_importance_pressure = true;
            record = record.mark(marker::HIGH_IMPORTANCE_PRESSURE);
        }
        record.emit();
    }
    assert!(
        high_importance_pressure,
        "{} never fired",
        marker::HIGH_IMPORTANCE_PRESSURE
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_rejected_primary_attempt_falls_back_and_publishes_the_fallback_output() {
    let (case, source) = case_source("C1", "C1.V1");
    let mut config = default_test_config();
    config.model_chain = vec!["test/primary".into(), "test/fallback".into()];
    let invalid = "<output>not a history_segments document</output>".to_string();
    let fold = fold_with(
        config,
        source_ingress(case, source),
        follow_up_for(case, source),
        vec![invalid.clone(), source.approved_example.clone()],
    )
    .await;
    wait_for_count(&fold.producer.starts, 2).await;
    wait_for_idle(&fold.store).await;
    let attempts = fold.attempts();
    let models: Vec<&str> = attempts.iter().map(|a| a.model.as_str()).collect();
    assert_eq!(models, ["test/primary", "test/fallback"]);
    attempts.iter().for_each(assert_real_generation);
    assert_eq!(
        attempts[0].prompt, attempts[1].prompt,
        "both attempts see the same input"
    );
    let rows = fold.rows();
    let [row] = rows.as_slice() else {
        panic!("one row from the fallback")
    };
    assert_eq!(
        row.p1.as_deref(),
        Some(approved_tiers(&source.approved_example)[0].as_str())
    );
    let attempt = |index: usize, output: &str, outcome: &str| {
        json!({
            "attempt": index + 1,
            "model": attempts[index].model,
            "prompt_sha256": sha256_hex(&attempts[index].prompt),
            "output_sha256": sha256_hex(output),
            "outcome": outcome,
        })
    };
    observation(
        &case.id,
        &source.id,
        "generation_primary",
        Terminal::ValidationRejected,
    )
    .with(attempt(0, &invalid, "validation_rejected"))
    .emit();
    observation(
        &case.id,
        &source.id,
        "generation_fallback",
        Terminal::Published,
    )
    .with(json!({
        "attempts": [
            attempt(0, &invalid, "validation_rejected"),
            attempt(1, &source.approved_example, "published"),
        ],
    }))
    .mark(marker::REJECTED_PRIMARY_FALLBACK)
    .emit();
}

#[tokio::test(flavor = "current_thread")]
async fn p1_only_output_inherits_p2_and_p3_and_records_them_as_unauthored() {
    let (case, source) = case_source("C5", "C5.V1");
    let [p1, p2, p3, _] = approved_tiers(&source.approved_example);
    let p1_only = source
        .approved_example
        .replace(&format!("\n<p2>{p2}</p2>"), "")
        .replace(&format!("\n<p3>{p3}</p3>"), "");
    assert!(!p1_only.contains("<p2>") && !p1_only.contains("<p3>"));
    let fold = fold_with(
        default_test_config(),
        source_ingress(case, source),
        follow_up_for(case, source),
        vec![p1_only],
    )
    .await;
    assert_real_generation(&fold.attempts()[0]);
    let row = fold.rows().remove(0);
    assert_eq!(row.p1.as_deref(), Some(p1.as_str()));
    assert_eq!(
        row.p2.as_deref(),
        Some(p1.as_str()),
        "P2 inherits the denser P1"
    );
    assert_eq!(
        row.p3.as_deref(),
        Some(p1.as_str()),
        "P3 inherits the denser P1"
    );
    assert_ne!(row.p2.as_deref(), Some(p2.as_str()));
    observation(
        &case.id,
        &source.id,
        "generation_inherited",
        Terminal::Published,
    )
    .with(json!({
        "authored": {"p1": true, "p2": false, "p3": false},
        "effective_p2_is_p1": true,
        "effective_p3_is_p1": true,
    }))
    .mark(marker::INHERITED_P2_P3)
    .emit();
}

#[tokio::test(flavor = "current_thread")]
async fn a_mismatched_tier_close_heals_into_the_approved_bodies() {
    let (case, source) = case_source("C2", "C2.V1");
    let [p1, p2, p3, _] = approved_tiers(&source.approved_example);
    let mangled = source
        .approved_example
        .replacen(&format!("{p1}</p1>"), &format!("{p1}</p2>"), 1);
    assert_ne!(mangled, source.approved_example);
    let fold = fold_with(
        default_test_config(),
        source_ingress(case, source),
        follow_up_for(case, source),
        vec![mangled],
    )
    .await;
    let row = fold.rows().remove(0);
    assert_eq!(
        [row.p1.as_deref(), row.p2.as_deref(), row.p3.as_deref()],
        [Some(p1.as_str()), Some(p2.as_str()), Some(p3.as_str())]
    );
    observation(
        &case.id,
        &source.id,
        "generation_healed",
        Terminal::Published,
    )
    .with(json!({"authored_close": "</p2> after p1", "healed_tiers_match_approved": true}))
    .emit();
}

#[tokio::test(flavor = "current_thread")]
async fn a_provisional_final_segment_is_discarded_after_valid_earlier_coverage() {
    let (case, source) = case_source("C4", "C4.V1");
    let count = source.messages.len() as u64;
    let last = count;
    let two_segments = source
        .approved_example
        .replacen(
            &format!("start=\"1\" end=\"{count}\""),
            &format!("start=\"1\" end=\"{}\"", count - 1),
            1,
        )
        .replacen(
            "</history_segment>\n</history_segments>",
            &format!(
                "</history_segment>\n<history_segment start=\"{last}\" end=\"{last}\" title=\"Provisional tail\" episode_type=\"design\" importance=\"30\"><p1>Test-authored provisional segment.</p1><p2>Provisional.</p2><p3>Tail.</p3><p4 /></history_segment>\n</history_segments>"
            ),
            1,
        );
    let fold = fold_with(
        default_test_config(),
        source_ingress(case, source),
        follow_up_for(case, source),
        vec![two_segments],
    )
    .await;
    assert_real_generation(&fold.attempts()[0]);
    let rows = fold.rows();
    let [row] = rows.as_slice() else {
        panic!("only the earlier segment publishes: {rows:?}")
    };
    assert_eq!((row.start_message, row.end_message), (1, count as i64 - 1));
    let served = fold.pass(None, "cfg0").await;
    let Some(Part::Text { text, .. }) = source.messages.last().unwrap().parts.first() else {
        panic!("the last C4 record opens with text")
    };
    assert!(
        live_texts(&served).contains(&serde_json::to_string(text).unwrap()),
        "the discarded range stays live"
    );
    observation(
        &case.id,
        &source.id,
        "generation_final_discard",
        Terminal::DiscardedCoverage,
    )
    .with(json!({
        "published": [1, count - 1],
        "discarded": [last, last],
        "discarded_range_live": true,
    }))
    .mark(marker::FINAL_DISCARD_AFTER_COVERAGE)
    .emit();
}

#[tokio::test(flavor = "current_thread")]
async fn a_bad_citation_rejects_the_fact_set_while_history_publishes() {
    let (case, source) = case_source("C3", "C3.V1");
    let with_facts = source.approved_example.replacen(
        "<meta>",
        "<facts>\n<PROJECT_RULES>\n* [s99:0-5] A fact citing a part the model never saw.\n</PROJECT_RULES>\n</facts>\n<meta>",
        1,
    );
    let fold = fold_with(
        default_test_config(),
        source_ingress(case, source),
        follow_up_for(case, source),
        vec![with_facts],
    )
    .await;
    assert_eq!(fold.rows().len(), 1);
    let latest = fold
        .store
        .load("ses")
        .unwrap()
        .meta
        .history_summarizer
        .memory_reviewer_nonadmission
        .latest
        .expect("the rejected fact set records a nonadmission");
    assert_eq!(
        latest.code,
        MemoryReviewerNonadmissionCode::FactSetRejected {
            failure: ExtractionFailure::UnknownAlias
        }
    );
    observation(
        &case.id,
        &source.id,
        "generation_citation_rejected",
        Terminal::Published,
    )
    .with(json!({"facts": "rejected: unknown_alias", "history": "published"}))
    .emit();
}

#[tokio::test(flavor = "current_thread")]
async fn a_same_length_source_drift_during_the_run_rejects_publication() {
    let (case, source) = case_source("C6", "C6.V1");
    let successor = decoded(&native_records(&case.id, &source.id, "successors")).remove(0);
    let drifted = crate::wire::project_messages(&[Arc::new(successor.clone())])
        .unwrap()
        .identity_by_mid
        .remove(&successor.mid)
        .unwrap();
    let producer = scripted(vec![source.approved_example.clone()]);
    let (handler, store, _dir, _project) =
        handler_with_store(Arc::clone(&producer), default_test_config());
    let hook_store = Arc::clone(&store);
    let mid = successor.mid.clone();
    *producer.on_await_output.lock().unwrap() = Some(Box::new(move || {
        hook_store.upsert_block_identities_for_test("ses", [(mid, drifted)]);
    }));
    let mut messages = source_ingress(case, source);
    messages.extend(live_tail(
        follow_up_for(case, source),
        messages.len() as u64 + 1,
    ));
    let fired = call_transform_request(
        &handler,
        request_with_usage(messages, HIGH_PRESSURE_USAGE, CONTEXT_LIMIT),
    )
    .await;
    assert_eq!(fired["history_summarizer"]["fired"], true);
    wait_for_count(&producer.await_outputs, 1).await;
    wait_for_idle(&store).await;
    assert!(
        store.load_history_segments("ses").unwrap().is_empty(),
        "a drifted selection publishes nothing"
    );
    let summarizer = store.load("ses").unwrap().meta.history_summarizer;
    assert_eq!(
        summarizer.last_failure.as_deref(),
        Some(
            "publish rejected: selected history_summarizer message msg_cf_c6_02 changed after firing"
        )
    );
    assert_eq!(summarizer.counters.invalidated, 1);
    observation(
        &case.id,
        &source.id,
        "generation_drift",
        Terminal::DriftRejected,
    )
    .with(json!({
        "drifted_message": successor.mid,
        "drift": "same-length successor revision",
    }))
    .emit();
}

#[tokio::test(flavor = "current_thread")]
async fn an_oversized_lead_truncates_producer_input_and_earns_no_generation_credit() {
    let (case, source) = case_source("C5", "C5.V1");
    let mut config = default_test_config();
    config.history_summarizer_context_limit_tokens = 32_000;
    let mut messages = vec![ck(
        "cf-oversized-log",
        1,
        &format!("Pasted build log: {}", "warning unused ".repeat(9_000)),
    )];
    messages.extend(source_ingress(case, source).into_iter().map(|mut message| {
        message.ordinal += 1;
        message
    }));
    let fold = fold_with(
        config,
        messages,
        follow_up_for(case, source),
        vec![source.approved_example.clone()],
    )
    .await;
    let attempt = fold.attempts().remove(0);
    assert!(attempt.prompt.contains("tokens truncated by the daemon"));
    assert_eq!(
        prompt_ordinal_range(&attempt.prompt),
        Some((1, 1)),
        "the budget-bounded chunk holds only the oversized lead, presented truncated"
    );
    let entries = exposures(case, source, &attempt.prompt);
    for (seen, _, _, entry) in &entries {
        assert_eq!(*seen, Exposure::Absent, "{entry}");
    }
    assert!(fold.rows().is_empty());
    let last_failure = fold
        .store
        .load("ses")
        .unwrap()
        .meta
        .history_summarizer
        .last_failure;
    assert!(
        last_failure
            .as_deref()
            .is_some_and(|failure| failure.starts_with("validate rejected")),
        "{last_failure:?}"
    );
    observation(
        &case.id,
        &source.id,
        "generation_truncated",
        Terminal::InputTruncated,
    )
    .with(json!({
        "lead": "test-authored oversized message",
        "chunk": [1, 1],
        "exposures": entries.into_iter().map(|e| e.3).collect::<Vec<_>>(),
        "validation": last_failure,
    }))
    .mark(marker::OBLIGATION_ABSENT_AFTER_TRUNCATION)
    .emit();
}

#[tokio::test(flavor = "current_thread")]
async fn legacy_and_tier_sparse_rows_render_from_their_own_fallbacks() {
    let (case, source) = case_source("C1", "C1.V1");
    let row = approved_fold(case, source).await.rows().remove(0);
    let approved = approved_tiers(&source.approved_example);
    let [p1, ..] = &approved;

    let legacy = StoredHistorySegment {
        sequence: row.sequence - 1,
        start_message: 0,
        end_message: 0,
        end_message_id: "cf-legacy#0".into(),
        title: "Legacy flat row".into(),
        content: "U: a pre-v2 flat summary body".into(),
        legacy: 1,
        ..Default::default()
    };
    let rendered = crate::decay_render::render_stored_history_segments(
        &[legacy, row.clone()],
        0.0,
        |text: &str| crate::token_cache::cached_estimate_tokens(text),
    );
    assert!(
        rendered.contains("Legacy flat row\nU: a pre-v2 flat summary body"),
        "{rendered}"
    );
    assert!(
        rendered.ends_with(p1.as_str()),
        "the tiered row keeps its P1: {rendered}"
    );

    let ages = first_ages(&row, &approved, &source.id);
    let (p2_age, p4_age) = (ages[rank(Tier::P2)], ages[rank(Tier::P4)]);
    assert!(
        p2_age < p4_age,
        "the authored row serves P2 and P3 before P4"
    );
    let mut sparse = row.clone();
    sparse.p2 = None;
    sparse.p3 = None;
    for newer in p2_age..p4_age {
        let out = render_aged(&sparse, newer);
        let segment = segment_of(&out, &row).unwrap();
        assert!(
            segment.ends_with(p1.as_str()),
            "at {newer} newer rows a missing P2 or P3 falls back to P1: {segment}"
        );
    }
    let at_p4 = render_aged(&sparse, p4_age);
    assert_eq!(
        segment_of(&at_p4, &row).unwrap().lines().count(),
        1,
        "the empty P4 still renders title only"
    );
    observation(
        &case.id,
        &source.id,
        "m0_legacy_and_sparse",
        Terminal::Served,
    )
    .with(json!({
        "legacy_row": "renders flat content, no tier claim",
        "sparse_row_falls_back_to_p1_for_newer_rows": [p2_age, p4_age - 1],
        "sparse_row_title_only_at": p4_age,
    }))
    .emit();
}

#[tokio::test(flavor = "current_thread")]
async fn zero_budgets_and_disabled_models_satisfy_no_situation() {
    let (case, source) = case_source("C3", "C3.V1");
    let fold = approved_fold(case, source).await;
    let row = fold.rows().remove(0);
    let approved = approved_tiers(&source.approved_example);
    for budget in [0.0, -1.0] {
        let served = fold.pass(Some(budget), &format!("cfg-{budget}")).await;
        assert_eq!(
            served_tier(
                history_body(&session_history(&m0_text(&served))),
                &row,
                &approved
            ),
            Tier::P1,
            "a nonpositive budget disables the guard, so it is no pressure witness"
        );
    }

    let mut disabled = default_test_config();
    disabled.model_chain.clear();
    let producer = Arc::new(ProducerState::default());
    let (handler, store, _dir, _project) = handler_with_store(Arc::clone(&producer), disabled);
    let mut messages = source_ingress(case, source);
    messages.extend(live_tail(
        follow_up_for(case, source),
        messages.len() as u64 + 1,
    ));
    let response = call_transform_request(
        &handler,
        request_with_usage(messages, HIGH_PRESSURE_USAGE, CONTEXT_LIMIT),
    )
    .await;
    assert_ne!(response["history_summarizer"]["fired"], true, "{response}");
    assert_eq!(producer.starts.load(Ordering::SeqCst), 0);
    assert!(producer.attempts.lock().unwrap().is_empty());
    assert!(store.load_history_segments("ses").unwrap().is_empty());
}

/// Opt-in real capture: the host connection file of a running host whose model execution can
/// reach the operator's model, the model id the history summarizer runs, and the per-source
/// wait for its firing to settle.
const REAL_CONNECTION_FILE: &str = "EIDNARA_FIDELITY_REAL_CONNECTION_FILE";
const REAL_MODEL: &str = "EIDNARA_FIDELITY_REAL_MODEL";
const REAL_WAIT_SECONDS: &str = "EIDNARA_FIDELITY_REAL_WAIT_SECONDS";
const REAL_OWNER: &str = "daemon.compression_fidelity.real_capture";
/// The harness a capture's session binds under. A real host's ModelExecution route admits only
/// the harnesses it runs and rejects any other with `invalid_identity`.
const CAPTURE_HARNESS: &str = "opencode";

/// One producer start the recorder saw: the complete model input and every output or error
/// the daemon drained for its run.
#[derive(Debug, Clone, Default, serde::Serialize)]
struct RecordedAttempt {
    session_id: String,
    model: String,
    system: String,
    prompt: String,
    max_output_tokens: u32,
    temperature: f64,
    run_id: Option<String>,
    start_error: Option<String>,
    /// Whether a start error proves the host committed no run; `None` without a start error.
    start_effect_proven: Option<bool>,
    outputs: Vec<RecordedOutput>,
    /// The firing's own `cancel` of this run, when it made one.
    cancel: Option<RecordedCancel>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct RecordedCancel {
    confirmed: bool,
    error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct RecordedOutput {
    text: Option<String>,
    length_capped: Option<bool>,
    error: Option<String>,
}

type AttemptLog = Arc<Mutex<Vec<RecordedAttempt>>>;

/// The latest connect's arguments let a run be cancelled through a second connection, and
/// `before_first_start` counts firings from their `connect` until their first start's handle or
/// error is appended, or until a driver that never started is dropped.
struct RecordingFactory {
    inner: Arc<dyn HistorySummarizerProducerFactory>,
    attempts: AttemptLog,
    connected: Mutex<Option<ConnectArgs>>,
    before_first_start: Arc<AtomicUsize>,
    connects: AtomicUsize,
}

#[derive(Clone)]
struct ConnectArgs {
    project_root: PathBuf,
    harness: String,
    credential_fingerprints: BTreeMap<String, String>,
}

#[async_trait]
impl HistorySummarizerProducerFactory for RecordingFactory {
    async fn connect(
        &self,
        project_root: &Path,
        harness: &str,
        credential_fingerprints: &std::collections::BTreeMap<String, String>,
    ) -> Result<Box<dyn HistorySummarizerProducerDriver + Send>, HistorySummarizerProducerError>
    {
        *self.connected.lock().unwrap() = Some(ConnectArgs {
            project_root: project_root.to_path_buf(),
            harness: harness.to_owned(),
            credential_fingerprints: credential_fingerprints.clone(),
        });
        self.connects.fetch_add(1, Ordering::SeqCst);
        self.before_first_start.fetch_add(1, Ordering::SeqCst);
        let inner = match self
            .inner
            .connect(project_root, harness, credential_fingerprints)
            .await
        {
            Ok(inner) => inner,
            Err(error) => {
                self.before_first_start.fetch_sub(1, Ordering::SeqCst);
                return Err(error);
            }
        };
        Ok(Box::new(RecordingDriver {
            inner,
            attempts: Arc::clone(&self.attempts),
            before_first_start: Arc::clone(&self.before_first_start),
            started: false,
        }))
    }
}

struct RecordingDriver {
    inner: Box<dyn HistorySummarizerProducerDriver + Send>,
    attempts: AttemptLog,
    before_first_start: Arc<AtomicUsize>,
    started: bool,
}

impl Drop for RecordingDriver {
    fn drop(&mut self) {
        if !self.started {
            self.before_first_start.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

impl RecordingDriver {
    fn started(
        &self,
        mut attempt: RecordedAttempt,
        started: &Result<RunHandle, HistorySummarizerProducerError>,
    ) {
        match started {
            Ok(handle) => attempt.run_id = Some(handle.run_id.clone()),
            Err(error) => {
                attempt.start_error = Some(format!("{error:?}"));
                attempt.start_effect_proven =
                    Some(crate::history_summarizer::start_effect_proven(error));
            }
        }
        self.attempts.lock().unwrap().push(attempt);
    }

    fn drained(
        &self,
        run_id: &str,
        output: Result<ProducerOutput, HistorySummarizerProducerError>,
    ) -> Result<ProducerOutput, HistorySummarizerProducerError> {
        let recorded = match &output {
            Ok(out) => RecordedOutput {
                text: Some(out.text.clone()),
                length_capped: Some(out.length_capped),
                error: None,
            },
            Err(error) => RecordedOutput {
                text: None,
                length_capped: None,
                error: Some(format!("{error:?}")),
            },
        };
        let mut attempts = self.attempts.lock().unwrap();
        attempts
            .iter_mut()
            .rev()
            .find(|a| a.run_id.as_deref() == Some(run_id))
            .unwrap_or_else(|| panic!("output drained for unrecorded run {run_id}"))
            .outputs
            .push(recorded);
        output
    }
}

#[async_trait]
impl HistorySummarizerProducerDriver for RecordingDriver {
    async fn bind_session(
        &mut self,
        session_id: &str,
    ) -> Result<(), HistorySummarizerProducerError> {
        self.inner.bind_session(session_id).await
    }

    /// Starts with the settings the real producer's `start` applies, so the record names them.
    async fn start(
        &mut self,
        session_id: &str,
        system: &str,
        prompt: &str,
        model: &str,
    ) -> Result<RunHandle, HistorySummarizerProducerError> {
        self.start_with_generation(
            session_id,
            system,
            prompt,
            model,
            crate::history_summarizer_producer::HISTORY_SUMMARIZER_MAX_OUTPUT_TOKENS,
            crate::history_summarizer_producer::HISTORY_SUMMARIZER_TEMPERATURE,
        )
        .await
    }

    async fn start_with_generation(
        &mut self,
        session_id: &str,
        system: &str,
        prompt: &str,
        model: &str,
        max_output_tokens: u32,
        temperature: f64,
    ) -> Result<RunHandle, HistorySummarizerProducerError> {
        let started = self
            .inner
            .start_with_generation(
                session_id,
                system,
                prompt,
                model,
                max_output_tokens,
                temperature,
            )
            .await;
        self.started(
            RecordedAttempt {
                session_id: session_id.into(),
                model: model.into(),
                system: system.into(),
                prompt: prompt.into(),
                max_output_tokens,
                temperature,
                ..RecordedAttempt::default()
            },
            &started,
        );
        if !self.started {
            self.started = true;
            self.before_first_start.fetch_sub(1, Ordering::SeqCst);
        }
        started
    }

    async fn await_output(
        &mut self,
        run_id: &str,
    ) -> Result<ProducerOutput, HistorySummarizerProducerError> {
        let output = self.inner.await_output(run_id).await;
        self.drained(run_id, output)
    }

    async fn await_output_with_timeout(
        &mut self,
        run_id: &str,
        timeout: Duration,
    ) -> Result<ProducerOutput, HistorySummarizerProducerError> {
        let output = self.inner.await_output_with_timeout(run_id, timeout).await;
        self.drained(run_id, output)
    }

    async fn redrain_output(
        &mut self,
        run_id: &str,
    ) -> Result<ProducerOutput, HistorySummarizerProducerError> {
        let output = self.inner.redrain_output(run_id).await;
        self.drained(run_id, output)
    }

    async fn redrain_output_with_timeout(
        &mut self,
        run_id: &str,
        timeout: Duration,
    ) -> Result<ProducerOutput, HistorySummarizerProducerError> {
        let output = self
            .inner
            .redrain_output_with_timeout(run_id, timeout)
            .await;
        self.drained(run_id, output)
    }

    async fn status(&mut self, run_id: &str) -> Result<RunState, HistorySummarizerProducerError> {
        self.inner.status(run_id).await
    }

    async fn cancel(&mut self, run_id: &str) -> Result<(), HistorySummarizerProducerError> {
        let result = self.inner.cancel(run_id).await;
        let recorded = RecordedCancel {
            confirmed: result.is_ok(),
            error: result.as_ref().err().map(|error| format!("{error:?}")),
        };
        if let Some(attempt) = self
            .attempts
            .lock()
            .unwrap()
            .iter_mut()
            .rev()
            .find(|a| a.run_id.as_deref() == Some(run_id))
        {
            attempt.cancel = Some(recorded);
        }
        result
    }

    async fn close_attempt(&mut self) -> Result<(), HistorySummarizerProducerError> {
        self.inner.close_attempt().await
    }

    async fn close(&mut self) -> Result<(), HistorySummarizerProducerError> {
        self.inner.close().await
    }

    async fn purge_session(
        &mut self,
        session_id: &str,
    ) -> Result<(), HistorySummarizerProducerError> {
        self.inner.purge_session(session_id).await
    }
}

/// Folds every corpus source once through `factory` with `model` as the only summarizer model,
/// gives each source `wait` from its transform call to its settled firing, and returns one
/// capture observation per source: every attempt's complete input and drained outputs, the
/// published rows, and `usage: null`, because the host's model-execution protocol reports no
/// token usage.
async fn capture_sources(
    factory: Arc<dyn HistorySummarizerProducerFactory>,
    model: &str,
    output_origin: &str,
    wait: Duration,
) -> Vec<Observation> {
    let mut captured = Vec::new();
    for (case, source) in sources() {
        let attempts: AttemptLog = Arc::default();
        let recording = Arc::new(RecordingFactory {
            inner: Arc::clone(&factory),
            attempts: Arc::clone(&attempts),
            connected: Mutex::new(None),
            before_first_start: Arc::default(),
            connects: AtomicUsize::new(0),
        });
        let config = DaemonConfig {
            model_chain: vec![model.to_owned()],
            ..default_test_config()
        };
        let (handler, store, _dir, _project) = handler_with_factory(
            Arc::clone(&recording) as Arc<dyn HistorySummarizerProducerFactory>,
            config,
            Arc::new(MissingSessionResolver),
            CAPTURE_HARNESS,
        );
        let follow_up = follow_up_for(case, source);
        let mut messages = source_ingress(case, source);
        let next = messages.len() as u64 + 1;
        messages.extend(live_tail(follow_up, next));
        let request = request_with_usage(messages, HIGH_PRESSURE_USAGE, CONTEXT_LIMIT);
        // The transform awaits an emergency firing inline, so one deadline covers the call and
        // the settling that follows it.
        let deadline = tokio::time::Instant::now() + wait;
        let first =
            tokio::time::timeout_at(deadline, call_transform_request(&handler, request)).await;
        let transform_returned = first.is_ok();
        let fired = first.map_or(Value::Null, |first| {
            first["history_summarizer"]["fired"].clone()
        });
        let settled = transform_returned && wait_settled(&store, &attempts, deadline).await;
        // A firing connecting or starting at the deadline appends its run id only when `start`
        // returns, so the snapshot waits for it, bounded by the producer's request timeout.
        let before_first_start = if settled {
            0
        } else {
            wait_for_first_starts(&recording, START_QUIESCE).await
        };
        // Run lifetime is detached from the waiter the timeout dropped; `run.cancel` ends it.
        // A settled firing's own cancel counts only when the host confirmed it.
        let connect = recording.connected.lock().unwrap().clone();
        let (cancelled_runs, purged_sessions) = {
            let attempts = attempts.lock().unwrap().clone();
            cancel_and_purge(&factory, connect, &attempts).await
        };
        // A confirmed cancel ends the firing's wait with a terminal, which the firing records
        // before it settles; the record is built after that settling.
        let settled_after_cancel = if cancelled_runs.iter().any(|o| o["cancelled"] == true) {
            let grace = tokio::time::Instant::now() + SETTLE_AFTER_CANCEL;
            wait_settled(&store, &attempts, grace).await
        } else {
            settled
        };
        let rows = store.load_history_segments("ses").unwrap();
        let attempts = attempts.lock().unwrap().clone();
        // A run the host has not confirmed stopped may still be generating, so no later source
        // starts another one.
        let capture_stopped = before_first_start > 0
            || cancelled_runs
                .iter()
                .any(|outcome| outcome["cancelled"] != true)
            || purged_sessions
                .iter()
                .any(|outcome| outcome["purged"] != true);
        let answered = attempts
            .iter()
            .any(|attempt| attempt.outputs.iter().any(|output| output.text.is_some()));
        let terminal = match (settled && answered, rows.is_empty()) {
            (false, _) => Terminal::Unsettled,
            (true, true) => Terminal::ValidationRejected,
            (true, false) => Terminal::Published,
        };
        captured.push(
            Observation::new(
                REAL_OWNER,
                CORPUS_SHA256,
                &case.id,
                &source.id,
                "capture",
                terminal,
            )
            .with(json!({
                "model": model,
                "harness": CAPTURE_HARNESS,
                "output_origin": output_origin,
                "fired": fired,
                "transform_returned": transform_returned,
                "settled": settled,
                "settled_after_cancel": settled_after_cancel,
                "cancelled_runs": cancelled_runs,
                "purged_sessions": purged_sessions,
                "firings_before_first_start_at_cancel": before_first_start,
                "capture_stopped": capture_stopped,
                "attempt_count": attempts.len(),
                "attempts": attempts,
                "usage": null,
                "usage_reported": false,
                "published_rows": rows.iter().map(|row| json!({
                    "start": row.start_message,
                    "end": row.end_message,
                    "title": row.title,
                    "p1": row.p1, "p2": row.p2, "p3": row.p3, "p4": row.p4,
                    "importance": row.importance,
                })).collect::<Vec<_>>(),
            })),
        );
        if capture_stopped {
            eprintln!(
                "compression fidelity capture: stopped after {} because a run was not confirmed cancelled",
                source.id
            );
            break;
        }
    }
    captured
}

/// The producer bounds one `connect` and one `start` by its 30 s request timeout each; the
/// budget covers both with a margin for a reconnect.
const START_QUIESCE: Duration = Duration::from_secs(75);
/// A spawned firing calls `connect` within this grace or it was never spawned.
const CONNECT_GRACE: Duration = Duration::from_secs(2);
/// A cancelled firing records its terminal and settles within this grace.
const SETTLE_AFTER_CANCEL: Duration = Duration::from_secs(10);

/// Polls until every firing has either passed its first start or been dropped, or `budget`
/// passes, and returns the count still before its first start. A firing spawned just before
/// the deadline shows up only once it calls `connect`, so the poll holds for `CONNECT_GRACE`
/// while no connect has been seen.
async fn wait_for_first_starts(recording: &RecordingFactory, budget: Duration) -> usize {
    let began = tokio::time::Instant::now();
    let deadline = began + budget;
    loop {
        let pending = recording.before_first_start.load(Ordering::SeqCst);
        let now = tokio::time::Instant::now();
        let no_connect_yet = recording.connects.load(Ordering::SeqCst) == 0;
        if pending == 0 && !(no_connect_yet && now < began + CONNECT_GRACE) {
            return 0;
        }
        if now >= deadline {
            return pending;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Through one fresh connection made with the firing's connect arguments: cancels every
/// started run that drained no model output and whose own cancel the host did not confirm,
/// then deletes every producer session the source's attempts used. Every source derives the
/// same producer session id, and the host keeps a session after its connection closes, so the
/// purge is what keeps one source's conversation out of the next source's model call; it also
/// ends any run a start error left unnamed.
async fn cancel_and_purge(
    factory: &Arc<dyn HistorySummarizerProducerFactory>,
    connect: Option<ConnectArgs>,
    attempts: &[RecordedAttempt],
) -> (Vec<Value>, Vec<Value>) {
    let undrained: Vec<(&str, &str)> = attempts
        .iter()
        .filter(|attempt| !attempt.outputs.iter().any(|output| output.text.is_some()))
        .filter(|attempt| {
            !attempt
                .cancel
                .as_ref()
                .is_some_and(|cancel| cancel.confirmed)
        })
        .filter_map(|attempt| Some((attempt.session_id.as_str(), attempt.run_id.as_deref()?)))
        .collect();
    let mut sessions: Vec<&str> = Vec::new();
    for attempt in attempts {
        if !sessions.contains(&attempt.session_id.as_str()) {
            sessions.push(&attempt.session_id);
        }
    }
    if undrained.is_empty() && sessions.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let describe = |result: &Result<(), HistorySummarizerProducerError>| {
        result.as_ref().err().map(|error| format!("{error:?}"))
    };
    let Some(connect) = connect else {
        let detail = "no connect was recorded for a started attempt".to_owned();
        return (
            undrained
                .iter()
                .map(|(_, run_id)| json!({ "run_id": run_id, "cancelled": false, "error": detail }))
                .collect(),
            sessions
                .iter()
                .map(|session_id| json!({ "session_id": session_id, "purged": false, "error": detail }))
                .collect(),
        );
    };
    let mut driver = match factory
        .connect(
            &connect.project_root,
            &connect.harness,
            &connect.credential_fingerprints,
        )
        .await
    {
        Ok(driver) => driver,
        Err(error) => {
            let detail = format!("cancel connect: {error:?}");
            return (
                undrained
                    .iter()
                    .map(|(_, run_id)| json!({ "run_id": run_id, "cancelled": false, "error": detail }))
                    .collect(),
                sessions
                    .iter()
                    .map(|session_id| json!({ "session_id": session_id, "purged": false, "error": detail }))
                    .collect(),
            );
        }
    };
    let mut cancelled = Vec::with_capacity(undrained.len());
    for (session_id, run_id) in undrained {
        // `cancel` travels the session-scoped command route, so the run's session is bound first.
        let result = match driver.bind_session(session_id).await {
            Ok(()) => driver.cancel(run_id).await,
            Err(error) => Err(error),
        };
        cancelled.push(
            json!({ "run_id": run_id, "cancelled": result.is_ok(), "error": describe(&result) }),
        );
    }
    let mut purged = Vec::with_capacity(sessions.len());
    for session_id in sessions {
        let result = driver.purge_session(session_id).await;
        purged.push(json!({ "session_id": session_id, "purged": result.is_ok(), "error": describe(&result) }));
    }
    if let Err(error) = driver.close().await {
        eprintln!("compression fidelity capture: cancel connection close failed: {error:?}");
    }
    (cancelled, purged)
}

/// Whether the run settled by `deadline`: the summarizer is idle after at least one producer
/// start.
async fn wait_settled(
    store: &MemoryStore,
    attempts: &AttemptLog,
    deadline: tokio::time::Instant,
) -> bool {
    loop {
        let idle = store.load("ses").unwrap().meta.history_summarizer.state
            == HistorySummarizerPhase::Idle;
        if idle && !attempts.lock().unwrap().is_empty() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// `dir` as an owner-only directory outside the repository: created with mode `0700` when
/// absent, refused when it names a parent directory, lies inside the repository, or exists
/// with any other mode or owner.
fn private_capture_dir(dir: &Path) -> PathBuf {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    assert!(
        !dir.components()
            .any(|part| part == std::path::Component::ParentDir),
        "{} names a parent directory",
        dir.display()
    );
    let absolute = std::path::absolute(dir).unwrap();
    let existing = absolute.ancestors().find(|a| a.exists()).unwrap();
    let real = existing
        .canonicalize()
        .unwrap()
        .join(absolute.strip_prefix(existing).unwrap());
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    assert!(
        !real.starts_with(&repository),
        "{} is inside the repository",
        real.display()
    );
    // Every existing ancestor is owned by this user or root and is closed to group and other
    // writes or sticky, so no other local user can swap the checked directory for a symlink
    // between the check and the write. The check runs before creation and again after, so a
    // component another user created in between is refused rather than trusted.
    let uid = rustix::process::getuid().as_raw();
    let require_trusted_ancestors = |real: &Path| {
        for ancestor in real.ancestors().filter(|a| a.exists()) {
            let meta = std::fs::metadata(ancestor).unwrap();
            let mode = meta.permissions().mode();
            assert!(
                (meta.uid() == uid || meta.uid() == 0) && (mode & 0o022 == 0 || mode & 0o1000 != 0),
                "{} is writable by others or not owned by this user or root",
                ancestor.display()
            );
        }
    };
    require_trusted_ancestors(&real);
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&real)
        .unwrap();
    require_trusted_ancestors(&real);
    let meta = std::fs::metadata(&real).unwrap();
    assert!(
        meta.permissions().mode() & 0o777 == 0o700
            && meta.uid() == rustix::process::getuid().as_raw(),
        "{} is not an owner-only directory",
        real.display()
    );
    real
}

/// Opt-in real capture of every corpus source through the host's existing producer
/// execution, written as private capture records. Never part of a default run.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "opt-in real producer capture; needs EIDNARA_FIDELITY_REAL_CONNECTION_FILE naming a \
            host that admits sends without a credential claim (direct_host_fixture without \
            --harness-runtime), EIDNARA_FIDELITY_REAL_MODEL, EIDNARA_FIDELITY_REAL_WAIT_SECONDS \
            and EIDNARA_FIDELITY_OBSERVATIONS_DIR"]
async fn real_producer_capture_of_every_corpus_source() {
    let required = |name: &str| {
        std::env::var(name).unwrap_or_else(|_| panic!("{name} is required for real capture"))
    };
    let connection_file = PathBuf::from(required(REAL_CONNECTION_FILE));
    let model = required(REAL_MODEL);
    let wait = Duration::from_secs(required(REAL_WAIT_SECONDS).parse().unwrap());
    let dir = private_capture_dir(Path::new(
        &std::env::var_os(OBSERVATIONS_DIR)
            .unwrap_or_else(|| panic!("{OBSERVATIONS_DIR} must name a private directory")),
    ));
    let factory = Arc::new(RealHistorySummarizerProducerFactory {
        connection_file,
        cancellation: CancellationToken::new(),
    });
    let records = capture_sources(factory, &model, "real producer through the host", wait).await;
    for record in &records {
        record.emit_to(&dir);
    }
    let unsettled: Vec<&str> = records
        .iter()
        .filter(|record| record.terminal == Terminal::Unsettled)
        .map(|record| record.source.as_str())
        .collect();
    assert!(unsettled.is_empty(), "unsettled sources: {unsettled:?}");
}

/// The real capture's harness is one the host's ModelExecution route admits.
#[test]
fn a_capture_binds_a_harness_the_host_admits() {
    assert!(host_runtime::model_execution::backend::Harness::parse(CAPTURE_HARNESS).is_some());
}

/// The capture records a scripted producer's run with the fields a real capture carries.
#[tokio::test(flavor = "current_thread")]
async fn a_capture_records_the_model_attempts_usage_and_complete_input() {
    let producer = Arc::new(ProducerState::default());
    producer
        .await_results
        .lock()
        .unwrap()
        .extend(sources().map(|(_, source)| {
            Ok(ProducerOutput {
                text: source.approved_example.clone(),
                length_capped: false,
            })
        }));
    let factory = Arc::new(TestProducerFactory {
        state: Arc::clone(&producer),
    });
    let records = capture_sources(
        factory,
        "probe/model",
        "scripted approved example",
        Duration::from_secs(10),
    )
    .await;
    assert_eq!(records.len(), sources().count());
    let harnesses = producer.harnesses.lock().unwrap().clone();
    assert!(
        !harnesses.is_empty() && harnesses.iter().all(|harness| harness == "opencode"),
        "the capture connects as a harness host model execution binds: {harnesses:?}"
    );
    let started = producer.attempts.lock().unwrap().clone();
    assert_eq!(started.len(), records.len());
    // Every source derives the same producer session id, so each source's session is deleted
    // once it is recorded and the next source's model call inherits no earlier conversation.
    let events = producer.session_events.lock().unwrap().clone();
    let expected: Vec<String> = started
        .iter()
        .flat_map(|attempt| {
            [
                format!("start:{}", attempt.session_id),
                format!("purge:{}", attempt.session_id),
            ]
        })
        .collect();
    assert_eq!(
        events, expected,
        "each source's session is purged before the next starts"
    );
    for (record, attempt) in records.iter().zip(&started) {
        assert_eq!(
            record.detail["purged_sessions"],
            json!([{ "session_id": attempt.session_id, "purged": true, "error": null }]),
            "{}",
            record.source
        );
    }
    let dir = tempfile::tempdir().unwrap();
    for ((record, (case, source)), start) in records.iter().zip(sources()).zip(&started) {
        let read: Value =
            serde_json::from_slice(&std::fs::read(record.emit_to(dir.path())).unwrap()).unwrap();
        assert_eq!(read["owner"], REAL_OWNER);
        assert_eq!(read["case"], case.id.as_str());
        assert_eq!(read["terminal"], "published", "{}", source.id);
        let detail = &read["detail"];
        assert_eq!(detail["model"], "probe/model");
        assert_eq!(detail["harness"], "opencode");
        assert_eq!(detail["output_origin"], "scripted approved example");
        assert_eq!(detail["settled"], true);
        assert_eq!(detail["attempt_count"], 1);
        assert_eq!(detail["usage"], Value::Null);
        assert_eq!(detail["usage_reported"], false);
        let attempt = &detail["attempts"][0];
        assert_eq!(attempt["session_id"], start.session_id.as_str());
        assert_eq!(attempt["model"], start.model.as_str());
        assert_eq!(attempt["system"], start.system.as_str());
        assert_eq!(attempt["prompt"], start.prompt.as_str(), "{}", source.id);
        assert_eq!(attempt["model"], "probe/model");
        assert_eq!(
            prompt_ordinal_range(attempt["prompt"].as_str().unwrap()),
            Some((1, source.messages.len() as u64)),
            "{}: the complete chunk input",
            source.id
        );
        assert_eq!(
            attempt["max_output_tokens"],
            crate::history_summarizer_producer::HISTORY_SUMMARIZER_MAX_OUTPUT_TOKENS
        );
        assert_eq!(
            attempt["temperature"],
            crate::history_summarizer_producer::HISTORY_SUMMARIZER_TEMPERATURE
        );
        assert_eq!(attempt["outputs"].as_array().unwrap().len(), 1);
        assert_eq!(
            attempt["outputs"][0]["text"],
            source.approved_example.as_str()
        );
        assert_eq!(detail["published_rows"].as_array().unwrap().len(), 1);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_timed_out_capture_cancels_its_started_runs() {
    let producer = Arc::new(ProducerState::default());
    producer.block_output.store(true, Ordering::SeqCst);
    let factory = Arc::new(TestProducerFactory {
        state: Arc::clone(&producer),
    });
    let records = capture_sources(
        factory,
        "probe/model",
        "scripted approved example",
        Duration::from_millis(300),
    )
    .await;
    assert_eq!(records.len(), sources().count());
    let started: Vec<String> = (1..=records.len()).map(|n| format!("run-{n}")).collect();
    let attempts = producer.attempts.lock().unwrap().clone();
    assert_eq!(attempts.len(), records.len());
    // The capture's cancel ends the firing's wait, and the firing then cancels once more on its
    // own error path, so each run is cancelled through its session by both.
    let cancels = producer.cancels.lock().unwrap().clone();
    let expected: Vec<String> = attempts
        .iter()
        .zip(&started)
        .map(|(attempt, run_id)| format!("{}:{run_id}", attempt.session_id))
        .collect();
    let mut distinct = cancels.clone();
    distinct.dedup();
    assert_eq!(
        distinct, expected,
        "every run that outlasted its wait is cancelled through its own session: {cancels:?}"
    );
    let dir = tempfile::tempdir().unwrap();
    for (record, run_id) in records.iter().zip(&started) {
        let read: Value =
            serde_json::from_slice(&std::fs::read(record.emit_to(dir.path())).unwrap()).unwrap();
        assert_eq!(read["terminal"], "unsettled");
        let detail = &read["detail"];
        assert_eq!(detail["settled"], false);
        assert_eq!(
            detail["cancelled_runs"],
            json!([{ "run_id": run_id, "cancelled": true, "error": null }])
        );
    }
    producer.block_output.store(false, Ordering::SeqCst);
    producer.notify.notify_waiters();
}

/// A start still in flight at the deadline is waited for, so its run is cancelled too.
#[tokio::test(flavor = "multi_thread")]
async fn a_start_in_flight_at_the_deadline_is_still_cancelled() {
    let producer = Arc::new(ProducerState::default());
    producer.block_output.store(true, Ordering::SeqCst);
    producer.block_start.store(true, Ordering::SeqCst);
    let factory = Arc::new(TestProducerFactory {
        state: Arc::clone(&producer),
    });
    let release = {
        let producer = Arc::clone(&producer);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(700)).await;
            producer.block_start.store(false, Ordering::SeqCst);
            producer.notify.notify_waiters();
        })
    };
    let records = capture_sources(
        factory,
        "probe/model",
        "scripted approved example",
        Duration::from_millis(300),
    )
    .await;
    release.await.unwrap();
    assert_eq!(records.len(), sources().count());
    let mut cancels = producer.cancels.lock().unwrap().clone();
    cancels.dedup();
    assert_eq!(
        cancels.len(),
        records.len(),
        "the first source's start returned after its deadline and was still cancelled: {cancels:?}"
    );
    assert!(cancels[0].ends_with(":run-1"), "{cancels:?}");
    let first = &records[0].detail;
    assert_eq!(first["attempt_count"], 1);
    assert_eq!(first["cancelled_runs"][0]["cancelled"], true);
    producer.block_output.store(false, Ordering::SeqCst);
    producer.notify.notify_waiters();
}

/// A firing still connecting at the deadline is waited for, so the run it then starts is
/// cancelled too.
#[tokio::test(flavor = "multi_thread")]
async fn a_firing_still_connecting_at_the_deadline_is_still_cancelled() {
    let producer = Arc::new(ProducerState::default());
    producer.block_output.store(true, Ordering::SeqCst);
    producer.block_connect.store(true, Ordering::SeqCst);
    let factory = Arc::new(TestProducerFactory {
        state: Arc::clone(&producer),
    });
    let release = {
        let producer = Arc::clone(&producer);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(700)).await;
            producer.block_connect.store(false, Ordering::SeqCst);
            producer.notify.notify_waiters();
        })
    };
    let records = capture_sources(
        factory,
        "probe/model",
        "scripted approved example",
        Duration::from_millis(300),
    )
    .await;
    release.await.unwrap();
    assert_eq!(records.len(), sources().count());
    let mut cancels = producer.cancels.lock().unwrap().clone();
    cancels.dedup();
    assert_eq!(cancels.len(), records.len(), "{cancels:?}");
    assert!(cancels[0].ends_with(":run-1"), "{cancels:?}");
    assert_eq!(records[0].detail["attempt_count"], 1);
    assert_eq!(records[0].detail["capture_stopped"], false);
    producer.block_output.store(false, Ordering::SeqCst);
    producer.notify.notify_waiters();
}

/// The firing's own cancel after a drained error is recorded; when the host did not confirm
/// it, the capture cancels again through a second connection even though the firing settled.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_in_firing_cancel_is_retried_before_the_next_source() {
    let producer = Arc::new(ProducerState::default());
    producer
        .await_results
        .lock()
        .unwrap()
        .push_back(Err(HistorySummarizerProducerError::Client(
            history_summarizer_producer::HistorySummarizerClientFailure {
                code: "probe_lost".to_owned(),
                message: "probe".to_owned(),
            },
        )));
    producer
        .cancel_errors
        .lock()
        .unwrap()
        .push_back(HistorySummarizerProducerError::TimedOut);
    producer.outputs.lock().unwrap().extend(
        sources()
            .skip(1)
            .map(|(_, source)| source.approved_example.clone()),
    );
    let factory = Arc::new(TestProducerFactory {
        state: Arc::clone(&producer),
    });
    let records =
        capture_sources(factory, "probe/model", "scripted", Duration::from_secs(10)).await;
    let first = &records[0].detail;
    assert_eq!(records.len(), sources().count(), "{first}");
    assert_eq!(first["settled"], true);
    assert_eq!(
        first["attempts"][0]["cancel"],
        json!({ "confirmed": false, "error": "TimedOut" })
    );
    assert_eq!(first["cancelled_runs"][0]["cancelled"], true, "{first}");
    assert_eq!(first["capture_stopped"], false);
    let cancels = producer.cancels.lock().unwrap().clone();
    assert_eq!(
        cancels.len(),
        2,
        "the firing's cancel and the capture's retry: {cancels:?}"
    );
    assert!(cancels.iter().all(|cancel| cancel.ends_with(":run-1")));
}

/// A start error that does not prove the request never reached the host leaves a run the
/// capture cannot name, so the producer session is purged before the next source.
#[tokio::test(flavor = "multi_thread")]
async fn an_unproven_start_failure_purges_the_session_before_the_next_source() {
    let producer = Arc::new(ProducerState::default());
    producer
        .start_errors
        .lock()
        .unwrap()
        .push_back(Err(HistorySummarizerProducerError::TimedOut));
    producer.outputs.lock().unwrap().extend(
        sources()
            .skip(1)
            .map(|(_, source)| source.approved_example.clone()),
    );
    let factory = Arc::new(TestProducerFactory {
        state: Arc::clone(&producer),
    });
    let records =
        capture_sources(factory, "probe/model", "scripted", Duration::from_secs(10)).await;
    let first = &records[0].detail;
    assert_eq!(records.len(), sources().count(), "{first}");
    let attempt = &first["attempts"][0];
    assert_eq!(attempt["run_id"], Value::Null);
    assert_eq!(attempt["start_error"], "TimedOut");
    assert_eq!(attempt["start_effect_proven"], false);
    assert_eq!(
        producer.purges.lock().unwrap().first(),
        Some(&attempt["session_id"].as_str().unwrap().to_owned())
    );
    assert_eq!(first["cancelled_runs"], json!([]));
    assert_eq!(
        first["purged_sessions"],
        json!([{ "session_id": attempt["session_id"], "purged": true, "error": null }])
    );
    assert_eq!(first["capture_stopped"], false);

    let producer = Arc::new(ProducerState::default());
    producer
        .start_errors
        .lock()
        .unwrap()
        .push_back(Err(HistorySummarizerProducerError::TimedOut));
    producer
        .purge_errors
        .lock()
        .unwrap()
        .push_back(HistorySummarizerProducerError::TimedOut);
    let factory = Arc::new(TestProducerFactory {
        state: Arc::clone(&producer),
    });
    let records =
        capture_sources(factory, "probe/model", "scripted", Duration::from_secs(10)).await;
    assert_eq!(records.len(), 1, "a failed purge stops the capture");
    assert_eq!(records[0].detail["capture_stopped"], true);
}

/// The record built after the cancel pass carries the drained terminal and the firing's own
/// cancel that the cancel provoked.
#[tokio::test(flavor = "multi_thread")]
async fn the_record_includes_what_the_cancel_provoked() {
    let producer = Arc::new(ProducerState::default());
    producer.block_output.store(true, Ordering::SeqCst);
    let factory = Arc::new(TestProducerFactory {
        state: Arc::clone(&producer),
    });
    let records = capture_sources(
        factory,
        "probe/model",
        "scripted approved example",
        Duration::from_millis(300),
    )
    .await;
    assert_eq!(records.len(), sources().count());
    for record in &records {
        let attempt = &record.detail["attempts"][0];
        assert_eq!(
            attempt["outputs"][0]["error"]
                .as_str()
                .map(|e| e.contains("run cancelled")),
            Some(true),
            "{}: {attempt}",
            record.source
        );
        assert_eq!(
            attempt["cancel"]["confirmed"], true,
            "{}: {attempt}",
            record.source
        );
        assert_eq!(
            record.detail["settled_after_cancel"], true,
            "{}",
            record.source
        );
    }
    producer.block_output.store(false, Ordering::SeqCst);
    producer.notify.notify_waiters();
}

/// A cancel the host does not confirm ends the capture before another source can start a run.
#[tokio::test(flavor = "multi_thread")]
async fn an_unconfirmed_cancellation_stops_the_capture() {
    let producer = Arc::new(ProducerState::default());
    producer.block_output.store(true, Ordering::SeqCst);
    producer
        .cancel_errors
        .lock()
        .unwrap()
        .push_back(HistorySummarizerProducerError::TimedOut);
    let factory = Arc::new(TestProducerFactory {
        state: Arc::clone(&producer),
    });
    let records = capture_sources(
        factory,
        "probe/model",
        "scripted approved example",
        Duration::from_millis(300),
    )
    .await;
    assert!(sources().count() > 1);
    assert_eq!(
        records.len(),
        1,
        "no later source starts after a failed cancel"
    );
    assert_eq!(producer.starts.load(Ordering::SeqCst), 1);
    assert_eq!(producer.cancels.lock().unwrap().len(), 1);
    let detail = &records[0].detail;
    assert_eq!(records[0].terminal, Terminal::Unsettled);
    assert_eq!(detail["cancelled_runs"][0]["cancelled"], false);
    assert_eq!(detail["cancelled_runs"][0]["error"], "TimedOut");
    assert_eq!(detail["capture_stopped"], true);
    producer.block_output.store(false, Ordering::SeqCst);
    producer.notify.notify_waiters();
}

/// The recorder keeps a start error and every drained output or error of a run, in order.
#[tokio::test(flavor = "current_thread")]
async fn the_recorder_keeps_start_errors_and_every_drained_outcome() {
    let producer = Arc::new(ProducerState::default());
    producer
        .start_errors
        .lock()
        .unwrap()
        .push_back(Err(HistorySummarizerProducerError::Client(
            history_summarizer_producer::HistorySummarizerClientFailure {
                code: "probe_refused".to_owned(),
                message: "probe".to_owned(),
            },
        )));
    producer.await_results.lock().unwrap().extend([
        Err(HistorySummarizerProducerError::Client(
            history_summarizer_producer::HistorySummarizerClientFailure {
                code: "probe_lost".to_owned(),
                message: "probe".to_owned(),
            },
        )),
        Ok(ProducerOutput {
            text: "second".to_owned(),
            length_capped: true,
        }),
    ]);
    let attempts: AttemptLog = Arc::default();
    let factory = RecordingFactory {
        inner: Arc::new(TestProducerFactory {
            state: Arc::clone(&producer),
        }),
        attempts: Arc::clone(&attempts),
        connected: Mutex::new(None),
        before_first_start: Arc::default(),
        connects: AtomicUsize::new(0),
    };
    let mut driver = factory
        .connect(Path::new("/"), "opencode", &Default::default())
        .await
        .unwrap();
    assert!(driver.start("s", "sys", "p1", "m").await.is_err());
    let run = driver.start("s", "sys", "p2", "m").await.unwrap();
    assert!(driver.await_output(&run.run_id).await.is_err());
    assert_eq!(
        driver.redrain_output(&run.run_id).await.unwrap().text,
        "second"
    );
    let recorded = attempts.lock().unwrap().clone();
    assert_eq!(recorded.len(), 2);
    assert!(
        recorded[0]
            .start_error
            .as_deref()
            .unwrap()
            .contains("probe_refused")
    );
    assert!(recorded[0].run_id.is_none() && recorded[0].outputs.is_empty());
    assert_eq!(recorded[1].prompt, "p2");
    assert_eq!(recorded[1].run_id.as_deref(), Some(run.run_id.as_str()));
    let [lost, second] = recorded[1].outputs.as_slice() else {
        panic!("two drained outcomes: {:?}", recorded[1].outputs);
    };
    assert!(lost.error.as_deref().unwrap().contains("probe_lost"));
    assert_eq!(
        (second.text.as_deref(), second.length_capped),
        (Some("second"), Some(true))
    );
}

/// A source whose producer never starts is recorded unsettled, not rejected.
#[tokio::test(flavor = "current_thread")]
async fn a_capture_whose_producer_never_starts_is_recorded_unsettled() {
    let producer = Arc::new(ProducerState::default());
    producer
        .connect_errors
        .lock()
        .unwrap()
        .extend(sources().map(|_| {
            HistorySummarizerProducerError::Client(
                history_summarizer_producer::HistorySummarizerClientFailure {
                    code: "connection_unavailable".to_owned(),
                    message: "probe".to_owned(),
                },
            )
        }));
    let factory = Arc::new(TestProducerFactory {
        state: Arc::clone(&producer),
    });
    let records = capture_sources(
        factory,
        "probe/model",
        "scripted",
        Duration::from_millis(200),
    )
    .await;
    assert_eq!(records.len(), sources().count());
    for record in &records {
        assert_eq!(record.terminal, Terminal::Unsettled, "{}", record.source);
        assert_eq!(record.detail["settled"], false);
        assert_eq!(record.detail["attempt_count"], 0);
    }
}

/// A settled firing whose producer drained only errors is recorded unsettled, not rejected.
#[tokio::test(flavor = "current_thread")]
async fn a_capture_that_drains_only_errors_is_recorded_unsettled() {
    let producer = Arc::new(ProducerState::default());
    producer
        .await_results
        .lock()
        .unwrap()
        .extend((0..4 * sources().count()).map(|_| {
            Err(HistorySummarizerProducerError::Client(
                history_summarizer_producer::HistorySummarizerClientFailure {
                    code: "probe_lost".to_owned(),
                    message: "probe".to_owned(),
                },
            ))
        }));
    let factory = Arc::new(TestProducerFactory {
        state: Arc::clone(&producer),
    });
    let records =
        capture_sources(factory, "probe/model", "scripted", Duration::from_secs(10)).await;
    for record in &records {
        assert_eq!(record.terminal, Terminal::Unsettled, "{}", record.source);
        assert_eq!(record.detail["settled"], true, "{}", record.source);
        assert!(record.detail["attempt_count"].as_u64().unwrap() >= 1);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_capture_whose_producer_outlasts_the_wait_is_recorded_unsettled_when_it_ends() {
    let producer = Arc::new(ProducerState::default());
    producer
        .block_output
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let factory = Arc::new(TestProducerFactory {
        state: Arc::clone(&producer),
    });
    let wait = Duration::from_millis(300);
    let began = std::time::Instant::now();
    let records = tokio::time::timeout(
        Duration::from_secs(30),
        capture_sources(factory, "probe/model", "scripted", wait),
    )
    .await
    .expect("the capture ends within its per-source waits");
    let sources = u32::try_from(sources().count()).unwrap();
    assert!(
        began.elapsed() < wait * sources + Duration::from_secs(5),
        "{:?}",
        began.elapsed()
    );
    for record in &records {
        assert_eq!(record.terminal, Terminal::Unsettled, "{}", record.source);
        assert_eq!(record.detail["settled"], false, "{}", record.source);
    }
}

#[test]
fn the_capture_directory_is_owner_only_and_outside_the_repository() {
    use std::os::unix::fs::PermissionsExt;
    let inside = Path::new(env!("CARGO_MANIFEST_DIR")).join("target-capture-probe");
    let refused = std::panic::catch_unwind(|| private_capture_dir(&inside));
    assert!(refused.is_err());
    assert!(!inside.exists());
    let dir = tempfile::tempdir().unwrap();
    let climbing = dir.path().join("missing/../captures");
    assert!(std::panic::catch_unwind(|| private_capture_dir(&climbing)).is_err());
    let fresh = private_capture_dir(&dir.path().join("captures"));
    assert_eq!(
        std::fs::metadata(&fresh).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let shared = dir.path().join("shared");
    std::fs::create_dir(&shared).unwrap();
    std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(std::panic::catch_unwind(|| private_capture_dir(&shared)).is_err());
    // A writable, unsticky ancestor lets another local user swap the checked directory for a
    // symlink between the check and the write; a sticky one does not.
    std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
    let below = shared.join("private");
    assert!(std::panic::catch_unwind(|| private_capture_dir(&below)).is_err());
    assert!(!below.exists());
    std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o1777)).unwrap();
    assert_eq!(
        std::fs::metadata(private_capture_dir(&shared.join("sticky")))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}

fn new_messages_prompt(lines: &[(u64, &str, &str)]) -> String {
    let body: Vec<String> = lines
        .iter()
        .enumerate()
        .map(|(index, (ordinal, role, text))| {
            format!("[{ordinal}] {role}: \u{ab}s{}\u{bb}{text}", index + 1)
        })
        .collect();
    format!("<new_messages>\n\n{}\n\n</new_messages>", body.join("\n"))
}

#[test]
fn exposure_is_credited_only_within_the_annotated_block() {
    let (case, source) = case_source("C6", "C6.V1");
    let [m1, m2, m3, m4] = source.messages.as_slice() else {
        panic!("C6.V1 holds four messages");
    };
    let [r1, r2, r3, r4] = [m1, m2, m3, m4].map(rendered_record);
    let seen = |prompt: &str| -> Vec<(String, Exposure)> {
        exposures(case, source, prompt)
            .into_iter()
            .map(|(seen, _, _, entry)| (entry["obligation"].as_str().unwrap().to_owned(), seen))
            .collect()
    };

    let whole = new_messages_prompt(&[(1, "U", &r1), (2, "A", &r2), (3, "U", &r3), (4, "A", &r4)]);
    assert_eq!(
        seen(&whole),
        [
            ("C6.O1".to_owned(), Exposure::Exact),
            ("C6.O2".to_owned(), Exposure::Transformed),
        ]
    );

    // msg_cf_c6_03 pastes msg_cf_c6_02's block verbatim. When the prompt omits msg_cf_c6_02, its
    // annotated spans are Absent while msg_cf_c6_03 still carries the copy.
    let annotated_dropped = new_messages_prompt(&[(1, "U", &r1), (3, "U", &r3), (4, "A", &r4)]);
    assert!(r3.contains(&r2), "the control depends on the C6 duplicate");
    assert_eq!(
        seen(&annotated_dropped),
        [
            ("C6.O1".to_owned(), Exposure::Absent),
            ("C6.O2".to_owned(), Exposure::Absent),
        ]
    );

    // msg_cf_c6_02's tool output repeats the summary line its text block quotes. The presenter
    // omits tool output, so a span annotated on the tool block is Absent although the text
    // block beside it carries the same bytes.
    let text_span = &case.obligations[0].evidence[0];
    assert_eq!(text_span.message_id, m2.info.id);
    assert_eq!(text_span.block_index, 1);
    let Part::Tool { state, .. } = &m2.parts[0] else {
        panic!("msg_cf_c6_02 block 0 is the tool call");
    };
    assert!(state.output.as_deref().unwrap().contains(&text_span.text));
    let tool_span = Span {
        source: text_span.source.clone(),
        message_id: text_span.message_id.clone(),
        block_index: 0,
        revision: text_span.revision.clone(),
        start: 0,
        end: text_span.text.len(),
        text: text_span.text.clone(),
    };
    let parts = presented_parts(&whole);
    assert_eq!(
        span_exposure(&parts, source, text_span),
        (true, Exposure::Exact)
    );
    assert_eq!(
        span_exposure(&parts, source, &tool_span),
        (true, Exposure::Absent)
    );
}

#[test]
fn an_observation_is_written_once_privately_and_reads_back() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("observations");
    let record = observation("C1", "C1.V1", "probe", Terminal::Served)
        .scenario("C1.S1")
        .with(json!({"tier": "p1"}));
    let path = record.emit_to(&target);
    assert_eq!(
        std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let read: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(read["owner"], OWNER);
    assert_eq!(read["corpus_sha256"], CORPUS_SHA256);
    assert_eq!(read["scenario"], "C1.S1");
    assert_eq!(read["source"], "C1.V1");
    assert_eq!(read["terminal"], "served");
    assert_eq!(read["schema_version"], 1);
    let leftovers: Vec<_> = std::fs::read_dir(&target)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(
        leftovers.len(),
        1,
        "no temporary file remains: {leftovers:?}"
    );
    let again = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| record.emit_to(&target)));
    assert!(again.is_err(), "a second write of one record is refused");
}

#[test]
fn publication_refuses_to_replace_a_completed_record() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("record.json");
    let temporary = dir.path().join(".record.json.tmp");
    std::fs::write(&path, b"first").unwrap();
    std::fs::write(&temporary, b"second").unwrap();
    assert!(
        super::compression_fidelity_observation::publish(&temporary, &path).is_err(),
        "a second writer whose existence check raced the first is refused"
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"first");
    assert!(
        !temporary.exists(),
        "the refused writer's temporary file is removed"
    );
}

/// Names a directory of published real captures; the opt-in real-serving replay serves each
/// source's captured output in place of its approved example.
const REAL_CAPTURE_DIR: &str = "EIDNARA_FIDELITY_REAL_CAPTURE_DIR";
/// The `output_origin` a capture of a real producer through the host records.
const REAL_ORIGIN: &str = "real producer through the host";

/// A source's captured output, the model that produced it, and the SHA-256 of the capture file
/// that published it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RealCapture {
    output: String,
    model: String,
    capture_sha256: String,
}

fn published_p1(output: &str) -> Option<String> {
    crate::history_summarizer_validate::parse_history_segment_output(output)
        .ok()?
        .history_segments
        .into_iter()
        .next()?
        .p1
}

#[test]
fn published_p1_reads_p1_as_the_validator_does() {
    for (output, p1) in [
        (
            "<output><history_segments><history_segment start=\"1\" end=\"2\" title=\"t\"><p1 >\nfull &amp; narrative\n</p2><p2>condensed</p2><p3>outcome</p3><p4/></history_segment></history_segments><meta><unprocessed_from>3</unprocessed_from></meta></output>",
            "full & narrative",
        ),
        (
            "The tiers use <p1>prose</p1> first.\n<output><history_segments><history_segment start=\"1\" end=\"2\" title=\"t\"><p1>body</p1><p2>b</p2><p3>b</p3><p4/></history_segment></history_segments><meta><unprocessed_from>3</unprocessed_from></meta></output>",
            "body",
        ),
    ] {
        let parsed = crate::history_summarizer_validate::parse_history_segment_output(output)
            .unwrap()
            .history_segments
            .remove(0)
            .p1;
        assert_eq!(parsed.as_deref(), Some(p1), "the validator's P1: {output}");
        assert_eq!(published_p1(output).as_deref(), Some(p1), "{output}");
    }
}

/// Reads every published real capture of `model` in `dir`, keyed by source. A capture names its
/// output by the attempt output whose P1, as the validator publishes it, is its published row's.
fn real_captures(dir: &Path, model: &str) -> BTreeMap<String, RealCapture> {
    let mut captures = BTreeMap::new();
    for entry in std::fs::read_dir(dir).unwrap_or_else(|error| panic!("{}: {error}", dir.display()))
    {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if !(name.starts_with(&format!("{REAL_OWNER}.")) && name.ends_with(".capture.json")) {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        let record: Value = serde_json::from_slice(&bytes).unwrap();
        let detail = &record["detail"];
        if record["owner"] != REAL_OWNER
            || record["corpus_sha256"] != CORPUS_SHA256
            || record["terminal"] != "published"
            || detail["output_origin"] != REAL_ORIGIN
            || detail["model"] != model
        {
            continue;
        }
        let p1 = detail["published_rows"][0]["p1"]
            .as_str()
            .filter(|p1| !p1.is_empty())
            .unwrap_or_else(|| panic!("{name}: the published row has no P1"));
        let output = detail["attempts"]
            .as_array()
            .into_iter()
            .flatten()
            .rev()
            .flat_map(|attempt| attempt["outputs"].as_array().into_iter().flatten().rev())
            .filter_map(|output| output["text"].as_str())
            .find(|text| published_p1(text).as_deref() == Some(p1))
            .unwrap_or_else(|| panic!("{name}: no attempt output carries the published P1"));
        let source = record["source"].as_str().unwrap().to_owned();
        let previous = captures.insert(
            source,
            RealCapture {
                output: output.to_owned(),
                model: model.to_owned(),
                capture_sha256: format!("{:x}", sha2::Sha256::digest(&bytes)),
            },
        );
        assert!(previous.is_none(), "{name}: a second capture of one source");
    }
    captures
}

/// `detail` with the origin of the output it was served from and the capture that published it.
fn bound(detail: Value, capture: &RealCapture) -> Value {
    let mut detail = detail;
    detail["output_origin"] = json!(REAL_ORIGIN);
    detail["generation_capture_sha256"] = json!(capture.capture_sha256);
    detail
}

/// Folds a test-authored baseline, then `source` with `output` translated past it, through one
/// handler, and serves the result: the m1-window scenario of [`serve_capture`].
async fn windowed_fold(case: &'static Case, source: &'static Source, output: &str) -> Value {
    let (baseline, baseline_output) = scaffold(1, "Baseline workspace setup");
    let shifted = source_ingress(case, source)
        .into_iter()
        .map(|mut message| {
            message.ordinal += SCAFFOLD_MESSAGES;
            message
        })
        .collect();
    let outputs = [baseline_output, translated(output, SCAFFOLD_MESSAGES)];
    let windowed = two_folds(baseline, shifted, follow_up_for(case, source), outputs).await;
    windowed.pass(None, "cfg0").await
}

/// Folds `source` once with its captured output under the capture's model, then serves it
/// naturally: P1 at m0 on the first fold, P1 in the m1 window after a test-authored baseline,
/// and each decayed tier at m0 under test-authored newer rows. Every observation names the
/// capture it serves. `windowed` is the source's [`windowed_fold`], which the caller runs
/// alongside the first fold.
async fn serve_capture(
    case: &'static Case,
    source: &'static Source,
    capture: &RealCapture,
    windowed: tokio::task::JoinHandle<Value>,
) -> Vec<Observation> {
    let output = &capture.output;
    let config = || DaemonConfig {
        model_chain: vec![capture.model.clone()],
        ..default_test_config()
    };
    let mut observations = Vec::new();
    let fold = fold_with(
        config(),
        source_ingress(case, source),
        follow_up_for(case, source),
        vec![output.clone()],
    )
    .await;
    let attempt = fold.attempts().remove(0);
    let rows = fold.rows();
    let [row] = rows.as_slice() else {
        panic!("{}: the captured output publishes one row", source.id);
    };
    // The validator trims and unescapes tier bodies, so the published row, not the raw output,
    // names what each tier serves.
    let tier = |body: &Option<String>| body.clone().unwrap_or_default();
    assert_eq!(
        published_p1(output),
        row.p1.clone(),
        "{}: the captured P1 publishes",
        source.id
    );
    // Matching uses the rendered form of served text.
    let tiers = [&row.p1, &row.p2, &row.p3, &row.p4]
        .map(|body| crate::decay_render::guarded_body(&tier(body)));
    let p1 = tiers[0].clone();
    observations.push(
        observation(&case.id, &source.id, "generation", Terminal::Published).with(bound(
            json!({
                "attempts": [{
                    "attempt": 1,
                    "model": attempt.model,
                    "system_sha256": sha256_hex(&attempt.system),
                    "prompt_sha256": sha256_hex(&attempt.prompt),
                    "output_sha256": sha256_hex(output),
                    "output_origin": REAL_ORIGIN,
                }],
                "chunk": [1, source.messages.len()],
            }),
            capture,
        )),
    );
    let served = fold.pass(None, "cfg0").await;
    assert!(m0_text(&served).contains(&p1), "{}: P1 at m0", source.id);
    observations.push(
        observation(&case.id, &source.id, "m0", Terminal::Served).with(bound(
            json!({"tier": "p1", "path": "natural", "first_fold_direct_to_m0": true}),
            capture,
        )),
    );

    let served = windowed
        .await
        .unwrap_or_else(|error| std::panic::resume_unwind(error.into_panic()));
    assert!(
        synthetic_text(&served, 1).contains(&p1),
        "{}: P1 rides m1",
        source.id
    );
    let m0 = m0_text(&served);
    assert!(
        m0.contains("Baseline workspace setup") && !m0.contains(&p1),
        "{}: m0 stays frozen at the baseline",
        source.id
    );
    for scenario in scenarios_serving(case, source, ServingPath::Natural, Tier::P1, Stage::M1) {
        observations.push(
            observation(&case.id, &source.id, "m1", Terminal::Served)
                .scenario(&scenario.id)
                .with(bound(json!({"tier": "p1", "path": "natural"}), capture)),
        );
    }

    let ages = first_ages(row, &tiers, &source.id);
    assert!(
        ages.windows(2).all(|pair| pair[0] < pair[1]),
        "{}: tiers decay in order: {ages:?}",
        source.id
    );
    for (tier, age) in TIERS.into_iter().zip(ages) {
        let path = if tier == Tier::P5 {
            ServingPath::Omission
        } else {
            ServingPath::Natural
        };
        let tier_name = format!("{tier:?}").to_lowercase();
        for scenario in scenarios_serving(case, source, path, tier, Stage::M0) {
            observations.push(
                observation(
                    &case.id,
                    &source.id,
                    format!("m0_decay_{tier_name}"),
                    Terminal::Served,
                )
                .scenario(&scenario.id)
                .with(bound(
                    json!({
                        "tier": tier_name,
                        "path": if tier == Tier::P5 { "omission" } else { "natural" },
                        "newer_rows": age,
                        "newer_rows_are": "test-authored filler",
                        "history_budget_tokens": 0,
                    }),
                    capture,
                )),
            );
        }
    }
    observations
}

/// Serves every corpus source from its published real capture of `EIDNARA_FIDELITY_REAL_MODEL`
/// in `EIDNARA_FIDELITY_REAL_CAPTURE_DIR`, and writes the bound observations under
/// `EIDNARA_FIDELITY_OBSERVATIONS_DIR`. A source with no published real capture fails the run.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "opt-in real-serving replay; needs EIDNARA_FIDELITY_REAL_CAPTURE_DIR naming published \
            real captures, EIDNARA_FIDELITY_REAL_MODEL naming the arm's model, and \
            EIDNARA_FIDELITY_OBSERVATIONS_DIR"]
async fn real_captures_serve_their_natural_tiers() {
    let dir = std::env::var_os(REAL_CAPTURE_DIR)
        .unwrap_or_else(|| panic!("{REAL_CAPTURE_DIR} names no capture directory"));
    let model = std::env::var(REAL_MODEL)
        .unwrap_or_else(|_| panic!("{REAL_MODEL} names no model; the arm binds one model"));
    assert!(
        std::env::var_os(OBSERVATIONS_DIR).is_some(),
        "{OBSERVATIONS_DIR} names no observation directory"
    );
    let captures = real_captures(Path::new(&dir), &model);
    serve_captures(&captures, true).await;
}

/// Serves every corpus source from its capture concurrently and returns the observations in
/// corpus order. A missing capture or a failing serving assertion panics the caller.
async fn serve_captures(
    captures: &BTreeMap<String, RealCapture>,
    emit: bool,
) -> Vec<Vec<Observation>> {
    let captured: Vec<(&'static Case, &'static Source, RealCapture)> = sources()
        .map(|(case, source)| {
            let capture = captures
                .get(&source.id)
                .unwrap_or_else(|| panic!("{}: no published real capture", source.id))
                .clone();
            (case, source, capture)
        })
        .collect();
    // The windowed folds carry the most work, so they start first and the first folds fill in.
    let windowed: Vec<_> = captured
        .iter()
        .map(|&(case, source, ref capture)| {
            let output = capture.output.clone();
            tokio::spawn(async move { windowed_fold(case, source, &output).await })
        })
        .collect();
    let mut tasks = tokio::task::JoinSet::new();
    for (index, ((case, source, capture), windowed)) in
        captured.into_iter().zip(windowed).enumerate()
    {
        tasks.spawn(async move {
            let mut observations = serve_capture(case, source, &capture, windowed).await;
            if emit {
                observations = observations.into_iter().map(Observation::emit).collect();
            }
            (index, observations)
        });
    }
    let mut served: Vec<Option<Vec<Observation>>> = (0..tasks.len()).map(|_| None).collect();
    while let Some(joined) = tasks.join_next().await {
        let (index, observations) = joined.unwrap_or_else(|error| {
            std::panic::resume_unwind(error.into_panic());
        });
        served[index] = Some(observations);
    }
    served.into_iter().map(Option::unwrap).collect()
}

/// A capture file published with the real origin binds what the real-serving replay serves: the
/// reader takes the attempt output that published, as the validator trims and unescapes it,
/// keyed by the capture file's SHA-256; every observation the replay records names that digest,
/// the real origin, and the capture's model. A capture of another model, another corpus, a
/// scripted origin, or an unpublished terminal binds nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_real_capture_binds_every_tier_the_replay_serves_from_it() {
    const MODEL: &str = "probe/model";
    let real_output = |source: &Source| {
        let example = &source.approved_example;
        let count = source.messages.len();
        let [p1, p2, p3, _] = approved_tiers(example);
        let title = approved_title(example);
        let varied = match source.id.as_str() {
            "C1.V1" => example.replacen(
                &format!("<p1>{p1}</p1>"),
                &format!("<p1>\n{} &amp; more\n</p1>", p1.replace('&', "&amp;")),
                1,
            ),
            "C2.V1" => example.replacen(&format!("<p1>{p1}</p1>"), &format!("<p1 >{p1}</p2>"), 1),
            "C2.V2" => example.replacen("<p4 />", "<p4>Deploy d-7781 succeeded.</p4>", 1),
            "C3.V1" => example.replacen(&format!("<p2>{p2}</p2>"), "", 1).replacen(
                &format!("<p3>{p3}</p3>"),
                "",
                1,
            ),
            "C4.V1" => example.replacen(
                &format!("title=\"{title}\""),
                &format!("title=\"{title} &amp; more\""),
                1,
            ),
            "C5.V1" => example.replacen(
                &format!("<p1>{p1}</p1>"),
                &format!("<p1>{p1}\n## Waves\nPayments moved first.</p1>"),
                1,
            ),
            "C6.V1" => {
                example.replacen(&format!("<p2>{p2}</p2>"), &format!("<p2>{p2} {p3}</p2>"), 1)
            }
            other => panic!("{other} has no real shape"),
        };
        assert_ne!(&varied, example, "{}: the variation applies", source.id);
        let processed = format!("<messages_processed>1-{count}</messages_processed>");
        let unprocessed = format!(
            "{processed}<unprocessed_from>{}</unprocessed_from>",
            count + 1
        );
        assert!(varied.contains(&processed), "{}", source.id);
        varied.replacen(&processed, &unprocessed, 1)
    };
    let producer = Arc::new(ProducerState::default());
    producer
        .await_results
        .lock()
        .unwrap()
        .extend(sources().map(|(_, source)| {
            Ok(ProducerOutput {
                text: real_output(source),
                length_capped: false,
            })
        }));
    let records = capture_sources(
        Arc::new(TestProducerFactory {
            state: Arc::clone(&producer),
        }),
        MODEL,
        REAL_ORIGIN,
        Duration::from_secs(10),
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let mut digests = BTreeMap::new();
    for record in &records {
        let path = record.emit_to(dir.path());
        digests.insert(
            record.source.clone(),
            format!("{:x}", sha2::Sha256::digest(std::fs::read(path).unwrap())),
        );
    }
    let captures = real_captures(dir.path(), MODEL);
    assert_eq!(captures.len(), sources().count());
    assert!(real_captures(dir.path(), "other/model").is_empty());
    for (_, source) in sources() {
        assert_eq!(
            captures[&source.id],
            RealCapture {
                output: real_output(source),
                model: MODEL.to_owned(),
                capture_sha256: digests[&source.id].clone(),
            },
            "{}",
            source.id
        );
    }

    let mut witnessed = 0;
    for ((_, source), observations) in sources().zip(serve_captures(&captures, false).await) {
        let capture = &captures[&source.id];
        witnessed += observations
            .iter()
            .filter(|observation| observation.scenario.is_some())
            .count();
        for observation in &observations {
            assert_eq!(observation.detail["output_origin"], REAL_ORIGIN);
            assert_eq!(
                observation.detail["generation_capture_sha256"],
                capture.capture_sha256.as_str(),
                "{}: {}",
                source.id,
                observation.stage
            );
        }
        let attempt = &observations[0].detail["attempts"][0];
        assert_eq!(attempt["output_sha256"], sha256_hex(&capture.output));
        assert_eq!(
            attempt["model"], MODEL,
            "the replay runs the capture's model"
        );
    }
    assert!(witnessed > 0, "the replay witnesses corpus scenarios");

    let twins = tempfile::tempdir().unwrap();
    for (index, change) in [
        |record: &mut Observation| {
            record.detail["output_origin"] = json!("scripted approved example")
        },
        |record: &mut Observation| record.terminal = Terminal::Unsettled,
        |record: &mut Observation| record.corpus_sha256 = "0".repeat(64).leak(),
    ]
    .into_iter()
    .enumerate()
    {
        let mut twin = Observation::new(
            REAL_OWNER,
            CORPUS_SHA256,
            &records[index].case,
            &records[index].source,
            "capture",
            Terminal::Published,
        )
        .with(records[index].detail.clone());
        change(&mut twin);
        twin.emit_to(twins.path());
    }
    assert!(
        real_captures(twins.path(), MODEL).is_empty(),
        "a scripted, unpublished, or foreign-corpus capture binds nothing"
    );
}
