//! Replays each corpus source's approved producer output through the daemon's real prompt and
//! alias assembly, producer validation, accepted publication, and the m1 and m0 serving paths,
//! and records one owner-attributed observation per case and scenario. Expected tiers are the
//! corpus's approved bodies; the renderer is only ever observed. Setting
//! `EIDNARA_FIDELITY_OBSERVATIONS_DIR` writes each observation once as private JSON; a default
//! run asserts in memory and writes nothing.

use super::compression_fidelity_corpus::*;
use super::*;

use memory_store::{BlockIdentity, MemoryReviewerNonadmissionCode};

const OWNER: &str = "daemon.compression_fidelity.replay";
const OBSERVATIONS_DIR: &str = "EIDNARA_FIDELITY_OBSERVATIONS_DIR";
const ESTIMATOR: &str =
    "tokenizer::estimate_tokens (Claude encoding) through token_cache::cached_estimate_tokens";
const HIGH_PRESSURE_USAGE: u64 = 49_000;
const CONTEXT_LIMIT: u64 = 50_000;

/// Every situation marker this module can fire, each a constant.
mod marker {
    pub const OBLIGATION_TRANSFORMED: &str = "cf_u2_source_obligation_transformed";
    pub const OBLIGATION_TRUNCATED: &str = "cf_u2_source_obligation_truncated";
    pub const OBLIGATION_TOOL_COMPACTED: &str = "cf_u2_source_obligation_tool_compacted";
    pub const REJECTED_PRIMARY_FALLBACK: &str = "cf_u2_rejected_primary_fallback_consumed";
    pub const FINAL_DISCARD_AFTER_COVERAGE: &str = "cf_u2_final_discard_after_valid_coverage";
    pub const INHERITED_P2_P3: &str = "cf_u2_inherited_p2_p3";
    pub const HIGH_IMPORTANCE_PRESSURE: &str = "cf_u2_high_importance_positive_budget_pressure";
}

/// One terminal outcome per observation; aggregates never replace them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum Terminal {
    Published,
    Served,
    ValidationRejected,
    DiscardedCoverage,
    DriftRejected,
    InputTruncated,
}

#[derive(Debug, serde::Serialize)]
struct Observation {
    owner: &'static str,
    corpus_sha256: &'static str,
    case: String,
    /// The corpus scenario this observation witnesses, or the source ID for a source-level stage.
    scenario: String,
    stage: String,
    terminal: Terminal,
    markers: Vec<&'static str>,
    detail: Value,
}

impl Observation {
    fn new(case: &str, scenario: &str, stage: impl Into<String>, terminal: Terminal) -> Self {
        Self {
            owner: OWNER,
            corpus_sha256: CORPUS_SHA256,
            case: case.to_owned(),
            scenario: scenario.to_owned(),
            stage: stage.into(),
            terminal,
            markers: Vec::new(),
            detail: json!({}),
        }
    }

    fn with(mut self, detail: Value) -> Self {
        self.detail = detail;
        self
    }

    fn mark(mut self, marker: &'static str) -> Self {
        self.markers.push(marker);
        self
    }

    /// Writes the observation once, privately, when an operator selected an output directory.
    fn emit(self) -> Self {
        if let Some(dir) = std::env::var_os(OBSERVATIONS_DIR) {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let dir = PathBuf::from(dir);
            std::fs::create_dir_all(&dir).unwrap();
            let name = format!(
                "{OWNER}.{}.{}.{}.json",
                self.case, self.scenario, self.stage
            );
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(dir.join(name))
                .unwrap();
            file.write_all(&serde_json::to_vec_pretty(&self).unwrap())
                .unwrap();
        }
        self
    }
}

/// The corpus scenarios of `source` that serve `tier` on `path`, at `stage` when one is named.
fn scenarios_serving(
    case: &'static Case,
    source: &Source,
    path: ServingPath,
    tier: Tier,
    stage: Option<Stage>,
) -> Vec<&'static Scenario> {
    case.scenarios
        .iter()
        .filter(|s| {
            s.source == source.id
                && s.serving.path == path
                && s.serving.tier == Some(tier)
                && (stage.is_none() || s.serving.stage == stage)
        })
        .collect()
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

/// A test-authored live tail: one unrelated message large enough to fill the protected tail,
/// then the case's follow-up. The fold therefore covers exactly the corpus source.
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

fn decoded(records: &[Arc<Value>]) -> Vec<IngressMessage> {
    crate::codec::opencode::decode_opencode_shared(records).messages
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
    /// Present after whitespace normalization only.
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

fn is_tool_span(source: &Source, span: &Span) -> bool {
    source
        .messages
        .iter()
        .find(|m| m.info.id == span.message_id)
        .and_then(|m| m.parts.get(span.block_index))
        .is_some_and(|part| matches!(part, Part::Tool { .. }))
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
}

/// Folds `prefix ++ source ++ live tail` once, scripting `outputs` as the producer's answers,
/// and waits for the firing to settle.
async fn fold_with(
    config: DaemonConfig,
    prefix: Vec<IngressMessage>,
    source_messages: Vec<IngressMessage>,
    follow_up: &FollowUp,
    outputs: Vec<String>,
) -> Fold {
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
    let (handler, store, dir, _project) = handler_with_store(Arc::clone(&producer), config);
    let mut messages = prefix;
    messages.extend(source_messages);
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

/// The messages a serving response delivers outside its synthetic m0/m1 blocks.
fn live_texts(response: &Value) -> Vec<String> {
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
        .collect()
}

fn m1_text(response: &Value) -> String {
    response["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["meta"]["synthetic"] == json!(true))
        .nth(1)
        .and_then(|message| message["content"][0]["kind"]["text"].as_str())
        .unwrap_or_default()
        .to_string()
}

#[tokio::test(flavor = "current_thread")]
async fn approved_examples_publish_through_real_validation_and_replace_covered_input() {
    let mut fired: BTreeSet<&str> = BTreeSet::new();
    for (case, source) in sources() {
        let follow_up = follow_up_for(case, source);
        let source_messages = decoded(&native_records(&case.id, &source.id, "messages"));
        let fold = fold_with(
            default_test_config(),
            Vec::new(),
            source_messages.clone(),
            follow_up,
            vec![source.approved_example.clone()],
        )
        .await;

        let attempts = fold.attempts();
        let [attempt] = attempts.as_slice() else {
            panic!("{}: one attempt", source.id);
        };
        assert_eq!(
            attempt.system,
            crate::history_summarizer_prompt::HISTORY_SUMMARIZER_SYSTEM_PROMPT,
            "{}",
            source.id
        );
        assert_eq!(attempt.model, "test/model", "{}", source.id);
        assert!(
            attempt
                .session_id
                .starts_with("eidnara-history_summarizer:"),
            "{}: the producer runs in its own summarizer session",
            source.id
        );
        let count = source_messages.len() as u64;
        assert_eq!(
            prompt_ordinal_range(&attempt.prompt),
            Some((1, count)),
            "{}: the chunk is exactly the corpus source",
            source.id
        );

        let mut exposures = Vec::new();
        let mut markers = Vec::new();
        for obligation in case.obligations.iter().filter(|o| o.memory.is_none()) {
            for span in obligation.evidence.iter().filter(|s| s.source == source.id) {
                let seen = exposure(&attempt.prompt, &span.text);
                let tool = is_tool_span(source, span);
                if tool && seen == Exposure::Absent {
                    markers.push(marker::OBLIGATION_TOOL_COMPACTED);
                }
                if seen == Exposure::Transformed {
                    markers.push(marker::OBLIGATION_TRANSFORMED);
                }
                exposures.push(json!({
                    "obligation": obligation.id,
                    "message_id": span.message_id,
                    "block_index": span.block_index,
                    "tool_output": tool,
                    "exposure": seen,
                    "generation_credit": seen != Exposure::Absent,
                }));
            }
        }

        let rows = fold.store.load_history_segments("ses").unwrap();
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
        let live = live_texts(&served).join("\n");
        for message in &source_messages {
            for block in message.ck.content() {
                let text = match block.kind() {
                    BlockKind::Text { text } => text,
                    _ => continue,
                };
                assert!(
                    !live.contains(&serde_json::to_string(text).unwrap()),
                    "{}: covered text {text:?} still reaches live input",
                    source.id
                );
            }
        }
        assert!(
            live.contains(&serde_json::to_string(&follow_up.prompt).unwrap()),
            "{}: the follow-up stays live",
            source.id
        );

        markers.sort_unstable();
        markers.dedup();
        let observation = Observation::new(&case.id, &source.id, "generation", Terminal::Published)
            .with(json!({
                "attempts": 1,
                "model": attempt.model,
                "system_sha256": format!("{:x}", sha2::Sha256::digest(attempt.system.as_bytes())),
                "chunk": [1, count],
                "authored_tiers": {"p1": true, "p2": true, "p3": true, "p4_empty": p4.is_empty()},
                "published_tiers_match_approved": true,
                "exposures": exposures,
                "covered_input_replaced": true,
            }));
        fired.extend(markers.iter().copied());
        markers
            .into_iter()
            .fold(observation, Observation::mark)
            .emit();
        Observation::new(&case.id, &source.id, "m0", Terminal::Served)
            .with(json!({"tier": "p1", "path": "natural", "first_fold_direct_to_m0": true}))
            .emit();
    }
    for situation in [
        marker::OBLIGATION_TRANSFORMED,
        marker::OBLIGATION_TOOL_COMPACTED,
    ] {
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

const BASELINE_MESSAGES: u64 = 3;

fn baseline_messages() -> Vec<IngressMessage> {
    (1..=BASELINE_MESSAGES)
        .map(|ordinal| {
            ck(
                &format!("cf-baseline-{ordinal}"),
                ordinal,
                &format!(
                    "Baseline setup note {ordinal}: {}",
                    "the workspace builds cleanly. ".repeat(40)
                ),
            )
        })
        .collect()
}

fn baseline_output() -> String {
    format!(
        "<output><history_segments><history_segment start=\"1\" end=\"{BASELINE_MESSAGES}\" title=\"Baseline workspace setup\" episode_type=\"infra\" importance=\"40\"><p1>Test-authored baseline: the workspace builds cleanly.</p1><p2>Baseline: workspace builds.</p2><p3>Baseline.</p3><p4 /></history_segment></history_segments><meta><messages_processed>1-{BASELINE_MESSAGES}</messages_processed></meta></output>"
    )
}

#[tokio::test(flavor = "current_thread")]
async fn a_publication_after_a_prior_baseline_serves_p1_in_the_m1_window() {
    for (case, source) in sources() {
        let follow_up = follow_up_for(case, source);
        let source_messages = decoded(&native_records(&case.id, &source.id, "messages"));
        let count = source_messages.len() as u64;
        let producer = Arc::new(ProducerState::default());
        producer.await_results.lock().unwrap().extend([
            Ok(ProducerOutput {
                text: baseline_output(),
                length_capped: false,
            }),
            Ok(ProducerOutput {
                text: translated(&source.approved_example, count, BASELINE_MESSAGES),
                length_capped: false,
            }),
        ]);
        let (handler, store, _dir, _project) =
            handler_with_store(Arc::clone(&producer), default_test_config());
        let call = |messages: Vec<IngressMessage>| {
            let handler = &handler;
            async move {
                call_transform_request(
                    handler,
                    request_with_usage(messages, HIGH_PRESSURE_USAGE, CONTEXT_LIMIT),
                )
                .await
            }
        };
        let mut before = baseline_messages();
        before.extend(live_tail(follow_up, BASELINE_MESSAGES + 1));
        call(before.clone()).await;
        wait_for_count(&producer.starts, 1).await;
        wait_for_idle(&store).await;
        let baseline_served = call(before).await;
        let m0_baseline = m0_text(&baseline_served);
        assert!(
            m0_baseline.contains("Baseline workspace setup"),
            "{m0_baseline}"
        );

        let mut after = baseline_messages();
        after.extend(source_messages.into_iter().map(|mut message| {
            message.ordinal += BASELINE_MESSAGES;
            message
        }));
        after.extend(live_tail(follow_up, BASELINE_MESSAGES + count + 1));
        let fired = call(after.clone()).await;
        assert_eq!(
            fired["history_summarizer"]["fired"], true,
            "{}: {fired}",
            source.id
        );
        wait_for_count(&producer.starts, 2).await;
        wait_for_idle(&store).await;
        let rows = store.load_history_segments("ses").unwrap();
        assert_eq!(rows.len(), 2, "{}", source.id);
        let served = call(after).await;
        let [p1, ..] = approved_tiers(&source.approved_example);
        let m1 = m1_text(&served);
        assert!(m1.contains(&p1), "{}: P1 rides m1: {m1}", source.id);
        assert!(
            !m0_text(&served).contains(&p1),
            "{}: m0 stays frozen at the baseline",
            source.id
        );
        let witnessed = scenarios_serving(
            case,
            source,
            ServingPath::Natural,
            Tier::P1,
            Some(Stage::M1),
        );
        let labels: Vec<String> = if witnessed.is_empty() {
            vec![source.id.clone()]
        } else {
            witnessed.iter().map(|s| s.id.clone()).collect()
        };
        for label in labels {
            Observation::new(&case.id, &label, "m1", Terminal::Served)
                .with(json!({
                    "source": source.id,
                    "tier": "p1",
                    "path": "natural",
                    "baseline": "test-authored, earns no fidelity credit",
                    "ordinal_offset": BASELINE_MESSAGES,
                    "action": served["action"],
                }))
                .emit();
        }
    }
}

/// The tier at which `rendered` serves the case row titled `title`, judged only against the
/// approved bodies: the body it carries, title only for P4, or absent for P5.
fn served_tier(rendered: &str, title: &str, approved: &[String; 4]) -> Tier {
    let Some(at) = rendered.find(title) else {
        return Tier::P5;
    };
    let segment = &rendered[at..];
    let segment = segment[..segment.find("\n\n## ").unwrap_or(segment.len())].trim_end();
    let tiers = [Tier::P1, Tier::P2, Tier::P3];
    let carried: Vec<Tier> = tiers
        .into_iter()
        .zip(approved)
        .filter(|(_, body)| segment.ends_with(body.as_str()))
        .map(|(tier, _)| tier)
        .collect();
    match carried.as_slice() {
        [tier] => *tier,
        [] if segment == segment.lines().next().unwrap_or_default() => Tier::P4,
        other => panic!("{title}: ambiguous serving {other:?}: {segment}"),
    }
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

async fn published_row(case: &Case, source: &Source) -> StoredHistorySegment {
    let fold = fold_with(
        default_test_config(),
        Vec::new(),
        decoded(&native_records(&case.id, &source.id, "messages")),
        follow_up_for(case, source),
        vec![source.approved_example.clone()],
    )
    .await;
    let mut rows = fold.store.load_history_segments("ses").unwrap();
    assert_eq!(rows.len(), 1, "{}", source.id);
    rows.remove(0)
}

#[tokio::test(flavor = "current_thread")]
async fn natural_decay_serves_every_approved_tier_and_then_omits_the_segment() {
    use crate::decay_render::render_stored_history_segments;
    for (case, source) in sources() {
        let row = published_row(case, source).await;
        let approved = approved_tiers(&source.approved_example);
        const TIERS: [Tier; 5] = [Tier::P1, Tier::P2, Tier::P3, Tier::P4, Tier::P5];
        let mut first_age: [Option<i64>; 5] = [None; 5];
        for newer in 0..=crate::decay_render::PRESSURE_WINDOW as i64 + 2 {
            let mut rows = vec![row.clone()];
            rows.extend((1..=newer).map(|k| filler_row(row.sequence + k, row.end_message + k)));
            let rendered = render_stored_history_segments(&rows, 0.0, |text: &str| {
                crate::token_cache::cached_estimate_tokens(text)
            });
            let tier = served_tier(&rendered, &row.title, &approved);
            let rank = TIERS.iter().position(|t| *t == tier).unwrap();
            first_age[rank].get_or_insert(newer);
        }
        let ages: Vec<i64> = first_age
            .iter()
            .zip(TIERS)
            .map(|(age, tier)| {
                age.unwrap_or_else(|| panic!("{}: natural decay never served {tier:?}", source.id))
            })
            .collect();
        assert!(
            ages.windows(2).all(|pair| pair[0] < pair[1]),
            "{}: tiers decay in order: {ages:?}",
            source.id
        );
        for (tier, age) in TIERS.into_iter().zip(ages) {
            let (path, stage) = match tier {
                Tier::P5 => (ServingPath::Omission, Some(Stage::M0)),
                Tier::P1 => (ServingPath::Natural, None),
                _ => (ServingPath::Natural, Some(Stage::M0)),
            };
            let witnessed: Vec<String> = scenarios_serving(case, source, path, tier, stage)
                .into_iter()
                .filter(|s| tier != Tier::P1 || s.serving.stage == Some(Stage::M0))
                .map(|s| s.id.clone())
                .collect();
            let tier_name = format!("{tier:?}").to_lowercase();
            let labels = if witnessed.is_empty() {
                vec![source.id.clone()]
            } else {
                witnessed
            };
            for label in labels {
                Observation::new(
                    &case.id,
                    &label,
                    format!("m0_decay_{tier_name}"),
                    Terminal::Served,
                )
                .with(json!({
                    "source": source.id,
                    "tier": tier_name,
                    "path": if tier == Tier::P5 { "omission" } else { "natural" },
                    "newer_rows": age,
                    "newer_rows_are": "test-authored filler",
                    "importance": row.importance,
                    "history_budget_tokens": 0,
                    "body_matches_approved": tier != Tier::P5,
                    "title_only": tier == Tier::P4,
                }))
                .emit();
            }
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
    for scenario in corpus()
        .cases
        .iter()
        .flat_map(|case| case.scenarios.iter().map(move |s| (case, s)))
        .filter(|(_, s)| s.serving.path == ServingPath::Pressure)
    {
        let (case, scenario) = scenario;
        let source = case.source(&scenario.source).unwrap();
        let target = scenario.serving.tier.unwrap();
        let fold = fold_with(
            default_test_config(),
            Vec::new(),
            decoded(&native_records(&case.id, &source.id, "messages")),
            follow_up_for(case, source),
            vec![source.approved_example.clone()],
        )
        .await;
        let row = fold.store.load_history_segments("ses").unwrap().remove(0);
        let approved = approved_tiers(&source.approved_example);

        let generous = fold.pass(Some(60_000.0), "cfg-generous").await;
        let retained = session_history(&m0_text(&generous));
        assert_eq!(
            served_tier(history_body(&retained), &row.title, &approved),
            Tier::P1,
            "{}",
            scenario.id
        );

        let p1_tokens = estimate(&retained);
        let mut reached = None;
        for (step, budget) in (1..p1_tokens).rev().enumerate() {
            let served = fold.pass(Some(budget as f64), &format!("cfg-{step}")).await;
            let slice = session_history(&m0_text(&served));
            let tier = served_tier(history_body(&slice), &row.title, &approved);
            if tier == target {
                reached = Some((budget, slice));
                break;
            }
        }
        let (budget, slice) = reached
            .unwrap_or_else(|| panic!("{}: no positive budget served {target:?}", scenario.id));
        let body = history_body(&slice).to_owned();
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
        let observation = Observation::new(&case.id, &scenario.id, "m0_pressure", Terminal::Served)
            .with(json!({
                "observed_baseline": {
                    "obligations_left_without_history_text": without_history_text,
                    "accepted_dispositions_need": "visible elsewhere or discoverable; U3 checks recovery",
                },
                "tier": format!("{target:?}").to_lowercase(),
                "path": "pressure",
                "importance": row.importance,
                "requested_history_budget_tokens": budget,
                "estimator": ESTIMATOR,
                "history_body_tokens": estimate(&body),
                "wrapped_session_history_tokens": estimate(&slice),
                "generous_budget_tokens": 60_000,
                "generous_tier": "p1",
            }));
        if row.importance >= 80 {
            high_importance_pressure = true;
        }
        let observation = if row.importance >= 80 {
            observation.mark(marker::HIGH_IMPORTANCE_PRESSURE)
        } else {
            observation
        };
        observation.emit();
    }
    assert!(
        high_importance_pressure,
        "{} never fired",
        marker::HIGH_IMPORTANCE_PRESSURE
    );
}

fn case_source(case: &str, source: &str) -> (&'static Case, &'static Source) {
    let case = corpus().case(case).unwrap();
    (case, case.source(source).unwrap())
}

fn source_ingress(case: &Case, source: &Source) -> Vec<IngressMessage> {
    decoded(&native_records(&case.id, &source.id, "messages"))
}

#[tokio::test(flavor = "current_thread")]
async fn a_rejected_primary_attempt_falls_back_and_publishes_the_fallback_output() {
    let (case, source) = case_source("C1", "C1.V1");
    let mut config = default_test_config();
    config.model_chain = vec!["test/primary".into(), "test/fallback".into()];
    let fold = fold_with(
        config,
        Vec::new(),
        source_ingress(case, source),
        follow_up_for(case, source),
        vec![
            "<output>not a history_segments document</output>".into(),
            source.approved_example.clone(),
        ],
    )
    .await;
    wait_for_count(&fold.producer.starts, 2).await;
    wait_for_idle(&fold.store).await;
    let models: Vec<String> = fold.attempts().into_iter().map(|a| a.model).collect();
    assert_eq!(models, ["test/primary", "test/fallback"]);
    let attempts = fold.attempts();
    assert_eq!(
        attempts[0].prompt, attempts[1].prompt,
        "both attempts see the same input"
    );
    let rows = fold.store.load_history_segments("ses").unwrap();
    let [row] = rows.as_slice() else {
        panic!("one row from the fallback")
    };
    assert_eq!(
        row.p1.as_deref(),
        Some(approved_tiers(&source.approved_example)[0].as_str())
    );
    Observation::new(
        &case.id,
        &source.id,
        "generation_primary",
        Terminal::ValidationRejected,
    )
    .with(json!({"model": "test/primary", "output": "no history_segments document"}))
    .emit();
    Observation::new(
        &case.id,
        &source.id,
        "generation_fallback",
        Terminal::Published,
    )
    .with(json!({
        "attempts": [
            {"model": "test/primary", "outcome": "validation_rejected"},
            {"model": "test/fallback", "outcome": "published"},
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
        Vec::new(),
        source_ingress(case, source),
        follow_up_for(case, source),
        vec![p1_only],
    )
    .await;
    let row = fold.store.load_history_segments("ses").unwrap().remove(0);
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
    Observation::new(
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
        Vec::new(),
        source_ingress(case, source),
        follow_up_for(case, source),
        vec![mangled],
    )
    .await;
    let row = fold.store.load_history_segments("ses").unwrap().remove(0);
    assert_eq!(
        [row.p1.as_deref(), row.p2.as_deref(), row.p3.as_deref()],
        [Some(p1.as_str()), Some(p2.as_str()), Some(p3.as_str())]
    );
    Observation::new(
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
    let messages = source_ingress(case, source);
    let count = messages.len() as u64;
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
        Vec::new(),
        messages.clone(),
        follow_up_for(case, source),
        vec![two_segments],
    )
    .await;
    let rows = fold.store.load_history_segments("ses").unwrap();
    let [row] = rows.as_slice() else {
        panic!("only the earlier segment publishes: {rows:?}")
    };
    assert_eq!((row.start_message, row.end_message), (1, count as i64 - 1));
    let served = fold.pass(None, "cfg0").await;
    let live = live_texts(&served).join("\n");
    let BlockKind::Text { text } = messages[count as usize - 1].ck.content()[0].kind() else {
        panic!("the last C4 message is text")
    };
    assert!(
        live.contains(&serde_json::to_string(text).unwrap()),
        "the discarded range stays live"
    );
    Observation::new(
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
        Vec::new(),
        source_ingress(case, source),
        follow_up_for(case, source),
        vec![with_facts],
    )
    .await;
    assert_eq!(fold.store.load_history_segments("ses").unwrap().len(), 1);
    let latest = fold
        .store
        .load("ses")
        .unwrap()
        .meta
        .history_summarizer
        .memory_reviewer_nonadmission
        .latest
        .expect("the rejected fact set records a nonadmission");
    assert!(
        matches!(
            latest.code,
            MemoryReviewerNonadmissionCode::FactSetRejected { .. }
        ),
        "{latest:?}"
    );
    Observation::new(
        &case.id,
        &source.id,
        "generation_citation_rejected",
        Terminal::Published,
    )
    .with(json!({"facts": "rejected", "history": "published"}))
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
    let producer = Arc::new(ProducerState::default());
    producer
        .await_results
        .lock()
        .unwrap()
        .push_back(Ok(ProducerOutput {
            text: source.approved_example.clone(),
            length_capped: false,
        }));
    let (handler, store, _dir, _project) =
        handler_with_store(Arc::clone(&producer), default_test_config());
    let hook_store = Arc::clone(&store);
    let mid = successor.mid.clone();
    let identities: Vec<BlockIdentity> = drifted;
    *producer.on_await_output.lock().unwrap() = Some(Box::new(move || {
        hook_store.upsert_block_identities_for_test("ses", [(mid, identities)]);
    }));
    let mut messages = source_ingress(case, source);
    let count = messages.len() as u64;
    messages.extend(live_tail(follow_up_for(case, source), count + 1));
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
    Observation::new(
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
    let oversized = vec![ck(
        "cf-oversized-log",
        1,
        &format!("Pasted build log: {}", "warning unused ".repeat(9_000)),
    )];
    let messages: Vec<IngressMessage> = source_ingress(case, source)
        .into_iter()
        .map(|mut message| {
            message.ordinal += 1;
            message
        })
        .collect();
    let fold = fold_with(
        config,
        oversized,
        messages,
        follow_up_for(case, source),
        vec![source.approved_example.clone()],
    )
    .await;
    let attempt = fold.attempts().remove(0);
    assert!(attempt.prompt.contains("tokens truncated by the daemon"));
    let mut exposures = Vec::new();
    for obligation in &case.obligations {
        for span in &obligation.evidence {
            let seen = exposure(&attempt.prompt, &span.text);
            assert_eq!(seen, Exposure::Absent, "{}: {}", obligation.id, span.text);
            exposures.push(
                json!({"obligation": obligation.id, "exposure": seen, "generation_credit": false}),
            );
        }
    }
    assert!(fold.store.load_history_segments("ses").unwrap().is_empty());
    Observation::new(
        &case.id,
        &source.id,
        "generation_truncated",
        Terminal::InputTruncated,
    )
    .with(json!({"exposures": exposures, "lead": "test-authored oversized message"}))
    .mark(marker::OBLIGATION_TRUNCATED)
    .emit();
}

#[tokio::test(flavor = "current_thread")]
async fn legacy_and_tier_sparse_rows_render_from_their_own_fallbacks() {
    use crate::decay_render::render_stored_history_segments;
    let (case, source) = case_source("C1", "C1.V1");
    let row = published_row(case, source).await;
    let [p1, ..] = approved_tiers(&source.approved_example);
    let estimate = |text: &str| crate::token_cache::cached_estimate_tokens(text);

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
    let rendered = render_stored_history_segments(&[legacy, row.clone()], 0.0, estimate);
    assert!(
        rendered.contains("Legacy flat row\nU: a pre-v2 flat summary body"),
        "{rendered}"
    );
    assert!(
        rendered.ends_with(&p1),
        "the tiered row keeps its P1: {rendered}"
    );

    let mut sparse = row.clone();
    sparse.p2 = None;
    sparse.p3 = None;
    let mut fallback_ages = Vec::new();
    for newer in 0..60 {
        let mut rows = vec![sparse.clone()];
        rows.extend((1..=newer).map(|k| filler_row(row.sequence + k, row.end_message + k)));
        let out = render_stored_history_segments(&rows, 0.0, estimate);
        let at = out.find(&row.title).unwrap();
        let segment = &out[at..];
        let segment = &segment[..segment.find("\n\n## ").unwrap_or(segment.len())];
        if segment.lines().count() == 1 {
            break;
        }
        assert!(!segment.is_empty());
        assert!(
            segment.ends_with(&p1),
            "a missing P2 or P3 falls back to P1: {segment}"
        );
        fallback_ages.push(newer);
    }
    Observation::new(
        &case.id,
        &source.id,
        "m0_legacy_and_sparse",
        Terminal::Served,
    )
    .with(json!({
        "legacy_row": "renders flat content, no tier claim",
        "sparse_row_fallback_to_p1_until_newer_rows": fallback_ages.last(),
    }))
    .emit();
}

#[tokio::test(flavor = "current_thread")]
async fn zero_budgets_and_disabled_models_satisfy_no_situation() {
    let (case, source) = case_source("C3", "C3.V1");
    let fold = fold_with(
        default_test_config(),
        Vec::new(),
        source_ingress(case, source),
        follow_up_for(case, source),
        vec![source.approved_example.clone()],
    )
    .await;
    let row = fold.store.load_history_segments("ses").unwrap().remove(0);
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
    let attempt = fold.attempts().remove(0);
    assert!(
        attempt.prompt.contains("<new_messages>") && attempt.prompt.contains("\u{ab}s1\u{bb}"),
        "the producer saw the real assembled prompt, not a placeholder"
    );

    let mut disabled = default_test_config();
    disabled.model_chain.clear();
    let producer = Arc::new(ProducerState::default());
    let (handler, store, _dir, _project) = handler_with_store(Arc::clone(&producer), disabled);
    let mut messages = source_ingress(case, source);
    let count = messages.len() as u64;
    messages.extend(live_tail(follow_up_for(case, source), count + 1));
    let response = call_transform_request(
        &handler,
        request_with_usage(messages, HIGH_PRESSURE_USAGE, CONTEXT_LIMIT),
    )
    .await;
    assert_ne!(response["history_summarizer"]["fired"], true, "{response}");
    assert_eq!(producer.starts.load(Ordering::SeqCst), 0);
    assert!(store.load_history_segments("ses").unwrap().is_empty());
}
