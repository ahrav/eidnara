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

/// The approved example with its segment range moved `offset` ordinals later. Tier bodies,
/// title, and importance are untouched; only the coordinate frame changes.
fn translated(approved_example: &str, count: u64, offset: u64) -> String {
    approved_example
        .replacen(
            &format!("start=\"1\" end=\"{count}\""),
            &format!("start=\"{}\" end=\"{}\"", 1 + offset, count + offset),
            1,
        )
        .replacen(
            &format!("<messages_processed>1-{count}</messages_processed>"),
            &format!(
                "<messages_processed>{}-{}</messages_processed>",
                1 + offset,
                count + offset
            ),
            1,
        )
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
        let count = source.messages.len() as u64;
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
                translated(&source.approved_example, count, SCAFFOLD_MESSAGES),
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

/// The served segment of the row titled `title`: from its heading to the next heading.
fn segment_of<'r>(rendered: &'r str, title: &str) -> Option<&'r str> {
    let at = rendered.find(title)?;
    let segment = &rendered[at..];
    Some(segment[..segment.find("\n\n## ").unwrap_or(segment.len())].trim_end())
}

/// The tier at which `rendered` serves the case row titled `title`, judged only against the
/// approved bodies: the body it carries, title only for P4, or absent for P5.
fn served_tier(rendered: &str, title: &str, approved: &[String; 4]) -> Tier {
    let Some(segment) = segment_of(rendered, title) else {
        return Tier::P5;
    };
    let carried: Vec<Tier> = [Tier::P1, Tier::P2, Tier::P3]
        .into_iter()
        .zip(approved)
        .filter(|(_, body)| segment.ends_with(body.as_str()))
        .map(|(tier, _)| tier)
        .collect();
    match carried.as_slice() {
        [tier] => *tier,
        [] if segment.lines().count() == 1 => Tier::P4,
        other => panic!("{title}: ambiguous serving {other:?}: {segment}"),
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

/// `row` followed by `newer` test-authored rows, rendered at the natural curve (budget 0).
fn render_aged(row: &StoredHistorySegment, newer: i64) -> String {
    let mut rows = vec![row.clone()];
    rows.extend((1..=newer).map(|k| filler_row(row.sequence + k, row.end_message + k)));
    crate::decay_render::render_stored_history_segments(&rows, 0.0, |text: &str| {
        crate::token_cache::cached_estimate_tokens(text)
    })
}

/// The first number of newer rows at which `row` serves each tier.
fn first_ages(row: &StoredHistorySegment, approved: &[String; 4], id: &str) -> [i64; 5] {
    let mut first: [Option<i64>; 5] = [None; 5];
    for newer in 0..=crate::decay_render::PRESSURE_WINDOW as i64 + 2 {
        let tier = served_tier(&render_aged(row, newer), &row.title, approved);
        first[rank(tier)].get_or_insert(newer);
        if first.iter().all(Option::is_some) {
            break;
        }
    }
    TIERS.map(|tier| {
        first[rank(tier)].unwrap_or_else(|| panic!("{id}: natural decay never served {tier:?}"))
    })
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
        let row = fold.rows().remove(0);
        let approved = approved_tiers(&source.approved_example);
        let newer_approved = approved_tiers(&newer_output);

        let generous = fold.pass(Some(60_000.0), "cfg-generous").await;
        let retained = session_history(&m0_text(&generous));
        assert_eq!(
            served_tier(history_body(&retained), &row.title, &approved),
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
            (
                served_tier(history_body(&slice), &row.title, &approved),
                slice,
            )
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
        let newer_tier = served_tier(&body, newer_title, &newer_approved);
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
        let segment = segment_of(&out, &row.title).unwrap();
        assert!(
            segment.ends_with(p1.as_str()),
            "at {newer} newer rows a missing P2 or P3 falls back to P1: {segment}"
        );
    }
    let at_p4 = render_aged(&sparse, p4_age);
    assert_eq!(
        segment_of(&at_p4, &row.title).unwrap().lines().count(),
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
                &row.title,
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
    outputs: Vec<RecordedOutput>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct RecordedOutput {
    text: Option<String>,
    length_capped: Option<bool>,
    error: Option<String>,
}

type AttemptLog = Arc<Mutex<Vec<RecordedAttempt>>>;

/// Connects every producer through `inner` and records each start and output.
struct RecordingFactory {
    inner: Arc<dyn HistorySummarizerProducerFactory>,
    attempts: AttemptLog,
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
        let inner = self
            .inner
            .connect(project_root, harness, credential_fingerprints)
            .await?;
        Ok(Box::new(RecordingDriver {
            inner,
            attempts: Arc::clone(&self.attempts),
        }))
    }
}

struct RecordingDriver {
    inner: Box<dyn HistorySummarizerProducerDriver + Send>,
    attempts: AttemptLog,
}

impl RecordingDriver {
    fn started(
        &self,
        mut attempt: RecordedAttempt,
        started: &Result<RunHandle, HistorySummarizerProducerError>,
    ) {
        match started {
            Ok(handle) => attempt.run_id = Some(handle.run_id.clone()),
            Err(error) => attempt.start_error = Some(format!("{error:?}")),
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
        self.inner.cancel(run_id).await
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
/// waits up to `wait` for each firing to settle, and returns one capture observation per
/// source: every attempt's complete input and drained outputs, the published rows, and
/// `usage: null`, because the host's model-execution protocol reports no token usage.
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
        });
        let config = DaemonConfig {
            model_chain: vec![model.to_owned()],
            ..default_test_config()
        };
        let (handler, store, _dir, _project) = handler_with_factory_for_harness(
            recording,
            config,
            Arc::new(MissingSessionResolver),
            CAPTURE_HARNESS,
        );
        let follow_up = follow_up_for(case, source);
        let mut messages = source_ingress(case, source);
        let next = messages.len() as u64 + 1;
        messages.extend(live_tail(follow_up, next));
        let request = request_with_usage(messages, HIGH_PRESSURE_USAGE, CONTEXT_LIMIT);
        let first = call_transform_request(&handler, request).await;
        let settled = wait_settled(&store, &attempts, wait).await;
        let rows = store.load_history_segments("ses").unwrap();
        let attempts = attempts.lock().unwrap().clone();
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
                "output_origin": output_origin,
                "fired": first["history_summarizer"]["fired"],
                "settled": settled,
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
    }
    captured
}

/// Whether the run settled: the summarizer is idle after at least one producer start.
async fn wait_settled(store: &MemoryStore, attempts: &AttemptLog, wait: Duration) -> bool {
    let deadline = std::time::Instant::now() + wait;
    loop {
        let idle = store.load("ses").unwrap().meta.history_summarizer.state
            == HistorySummarizerPhase::Idle;
        if idle && !attempts.lock().unwrap().is_empty() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
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
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&real)
        .unwrap();
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
#[ignore = "opt-in real producer capture; needs EIDNARA_FIDELITY_REAL_CONNECTION_FILE, \
            EIDNARA_FIDELITY_REAL_MODEL, EIDNARA_FIDELITY_REAL_WAIT_SECONDS and \
            EIDNARA_FIDELITY_OBSERVATIONS_DIR"]
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
    let started = producer.attempts.lock().unwrap().clone();
    assert_eq!(started.len(), records.len());
    let dir = tempfile::tempdir().unwrap();
    for ((record, (case, source)), start) in records.iter().zip(sources()).zip(&started) {
        let read: Value =
            serde_json::from_slice(&std::fs::read(record.emit_to(dir.path())).unwrap()).unwrap();
        assert_eq!(read["owner"], REAL_OWNER);
        assert_eq!(read["case"], case.id.as_str());
        assert_eq!(read["terminal"], "published", "{}", source.id);
        let detail = &read["detail"];
        assert_eq!(detail["model"], "probe/model");
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
