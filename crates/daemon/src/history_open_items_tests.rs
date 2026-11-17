//! Fold-level tests for open items: a question pending after one fold leaves the rendered history
//! once a later fold holds the user's answer or passes over it, and a constraint stands until a
//! later fold stores the claim of its lift.
//!
//! The fixtures follow the shape of real sessions in which a compaction summary kept a "waiting
//! on the user" line after the user had answered: a scope confirmation, a wait on merges the user
//! then reported done, a choice between two values, a go-ahead on a numbered list, and a constraint
//! the user later lifted. The deterministic tests script the summarizer's answers through the
//! production validator and renderer. The live test sends such a session to a real summarizer
//! model.

use super::*;

use crate::decay_render::{render_history_segment_at_tier, render_rows};
use crate::history_open_items::{Status, status_line};
use crate::history_summarizer_chunk::build_history_summarizer_chunk;
use crate::history_summarizer_validate::{
    HistorySummarizerChunk, ValidateOptions, ValidatedHistorySegment,
    validate_history_summarizer_output,
};
use crate::wire::project_messages;

const HIGH_PRESSURE_USAGE: u64 = 49_000;
const CONTEXT_LIMIT: u64 = 50_000;

fn user(ordinal: u64, text: &str) -> IngressMessage {
    wire_with_role(&format!("open-items-{ordinal}"), ordinal, "user", text)
}

fn assistant(ordinal: u64, text: &str) -> IngressMessage {
    wire_with_role(&format!("open-items-{ordinal}"), ordinal, "assistant", text)
}

/// The work report supplies enough text to meet the fold trigger's minimum span.
fn work_log(subject: &str) -> String {
    ["build", "test", "changelog", "version bump", "tag", "publish", "announce"]
        .iter()
        .enumerate()
        .map(|(step, action)| {
            format!(
                "Step {}: I checked the {action} step for {subject} against each package's scripts, \
                 noted which commands differ between packages, and recorded the order they run in.\n",
                step + 1
            )
        })
        .collect()
}

/// The chunk the production builder makes of the messages `first..end_exclusive`.
fn chunk_of(all: &[IngressMessage], first: u64, end_exclusive: u64) -> HistorySummarizerChunk {
    let messages: Vec<Arc<IngressMessage>> = all.iter().cloned().map(Arc::new).collect();
    let projection = project_messages(&messages).unwrap();
    build_history_summarizer_chunk(&messages, &projection.blocks, first, 100_000, end_exclusive)
        .chunk
}

/// A `[sN:start-end]` citation of `words` inside the presented part of message `ordinal`.
fn cite(chunk: &HistorySummarizerChunk, ordinal: u64, words: &str) -> String {
    let frozen = chunk
        .aliases
        .aliases
        .iter()
        .find(|frozen| frozen.ordinal == ordinal && frozen.presented.contains(words))
        .unwrap_or_else(|| panic!("message {ordinal} presents {words:?}"));
    let start = frozen.presented.find(words).unwrap();
    format!("[{}:{}-{}]", frozen.alias, start, start + words.len())
}

fn claim(key: &str, cite: &str, value: &str, anchor: &str) -> String {
    format!(
        "<claim><key>{key}</key><cite>{cite}</cite><value>{value}</value><anchor>{anchor}</anchor></claim>"
    )
}

/// One `<history_segment>` element over `start..=end`.
fn segment_xml(start: u64, end: u64, tiers: [&str; 4]) -> String {
    let [p1, p2, p3, p4] = tiers;
    format!(
        r#"<history_segment start="{start}" end="{end}" title="Work {start}-{end}" episode_type="feature" importance="50"><p1>{p1}</p1><p2>{p2}</p2><p3>{p3}</p3><p4>{p4}</p4></history_segment>"#
    )
}

/// One summarizer answer holding `segments` and `claims`; `next` is the first unprocessed message.
fn output(segments: &[String], claims: &[String], next: u64) -> String {
    format!(
        "<output><history_segments>{}</history_segments><claims>{}</claims><meta><unprocessed_from>{next}</unprocessed_from></meta></output>",
        segments.concat(),
        claims.concat(),
    )
}

/// The rows the production validator makes of `text` for `chunk`, as the store holds them, after
/// the rows `stored` already holds.
fn fold(
    stored: &mut Vec<StoredHistorySegment>,
    text: &str,
    chunk: &HistorySummarizerChunk,
) -> Vec<ValidatedHistorySegment> {
    let options = ValidateOptions {
        sequence_offset: stored.len() as u64 + 1,
        force_keep_last_history_segment: true,
        ..ValidateOptions::default()
    };
    let validated = validate_history_summarizer_output(text, chunk, &[], options).unwrap();
    stored.extend(validated.history_segments.iter().map(|segment| {
        crate::history_summarizer::to_stored_history_segment(segment, 0, &BTreeMap::new())
    }));
    validated.history_segments
}

/// Every row of `stored` rendered at each of the tiers one to three, as the history shows a row
/// while it is recent, consolidated, or old.
fn rendered_tiers(stored: &[StoredHistorySegment]) -> Vec<[String; 3]> {
    render_rows(stored, false)
        .iter()
        .map(|row| [1, 2, 3].map(|tier| render_history_segment_at_tier(row, tier)))
        .collect()
}

/// The open status lines in `text`.
fn open_lines(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|line| status_line(line).is_some_and(|(status, _)| status.is_open()))
        .collect()
}

/// One real-session shape: the assistant's closing question, the user's reply, and the status
/// lines a summarizer writes for them.
struct Answered {
    key: &'static str,
    request: &'static str,
    question: &'static str,
    answer: &'static str,
    decision: &'static str,
}

const ANSWERED: [Answered; 4] = [
    // The user confirms the scope the assistant asked about.
    Answered {
        key: "migration.remaining-scope",
        request: "Carry the storage migration through its remaining steps.",
        question: "Should I continue with the remaining migration steps in that order?",
        answer: "confirm, continue with the goal",
        decision: "The user confirmed the scope: continue with the remaining steps in order.",
    },
    // The assistant waits on an event the user then reports.
    Answered {
        key: "stack.rebase-after-merge",
        request: "Get the stacked changes ready to land.",
        question: "Tell me when the two open pull requests have merged and I will rebase the rest.",
        answer: "All pull requests are merged now",
        decision: "The user reported both pull requests merged; the rebase can go ahead.",
    },
    // The user picks one of two values.
    Answered {
        key: "poller.interval-seconds",
        request: "Tune the status poller.",
        question: "Should the poller interval stay at 60 seconds or drop to 30?",
        answer: "keep the poller on 60 then",
        decision: "The user kept the poller interval at 60 seconds.",
    },
    // The user gives the go-ahead on a numbered list.
    Answered {
        key: "review.recommended-fixes",
        request: "Review the parser module and list what to fix.",
        question: "Do you want me to apply the five recommended fixes?",
        answer: "apply items 1 to 5",
        decision: "The user asked for items 1 to 5 to be applied.",
    },
];

impl Answered {
    fn waiting_line(&self) -> String {
        format!("Waiting on user ({}): {}", self.key, self.question)
    }

    fn decided_line(&self) -> String {
        format!("Decided ({}): {}", self.key, self.decision)
    }

    /// The span that ends on the assistant's question.
    fn asking(&self) -> Vec<IngressMessage> {
        vec![
            user(1, self.request),
            assistant(2, &format!("{}{}", work_log("this request"), self.question)),
        ]
    }

    /// The asking span, then the user's reply and the assistant's follow-up.
    fn answered(&self) -> Vec<IngressMessage> {
        let mut messages = self.asking();
        messages.push(user(3, &format!("{}.", self.answer)));
        messages.push(assistant(4, "Done as you said; the change is in place."));
        messages
    }

    /// The first fold's answer: the question is open at the end of its span.
    fn first_output(&self) -> String {
        let p1 = format!("I worked through the request.\n{}", self.waiting_line());
        output(
            &[segment_xml(1, 2, [&p1, &p1, "Worked on it.", ""])],
            &[],
            3,
        )
    }
}

#[test]
fn a_question_pending_after_one_fold_leaves_once_the_next_fold_holds_the_answer() {
    for case in ANSWERED {
        let messages = case.answered();
        let mut stored = Vec::new();
        fold(
            &mut stored,
            &case.first_output(),
            &chunk_of(&messages, 1, 3),
        );

        // The question is open after the first fold, in the tiers a recent row renders.
        let after_first = rendered_tiers(&stored);
        assert_eq!(open_lines(&after_first[0][0]), [case.waiting_line()]);
        assert_eq!(open_lines(&after_first[0][1]), [case.waiting_line()]);

        // The second fold records the answer and cites the user's turn.
        let second_chunk = chunk_of(&messages, 3, 5);
        let p1 = format!("{}\nI made the change.", case.decided_line());
        let second = output(
            &[segment_xml(3, 4, [&p1, &p1, &p1, ""])],
            &[claim(
                case.key,
                &cite(&second_chunk, 3, case.answer),
                case.answer,
                &case.decided_line(),
            )],
            5,
        );
        let rows = fold(&mut stored, &second, &second_chunk);
        assert_eq!(rows[0].claims.len(), 1, "{}", case.key);
        assert_eq!(rows[0].claims[0].ordinal, 3);

        let history = rendered_tiers(&stored);
        for (row, tiers) in history.iter().enumerate() {
            for tier in tiers {
                assert_eq!(
                    open_lines(tier),
                    Vec::<&str>::new(),
                    "{} row {row}",
                    case.key
                );
            }
        }
        for tier in &history[1] {
            assert!(tier.contains(&case.decided_line()), "{tier}");
        }
        // The stored rows keep what the summarizer wrote.
        assert!(
            stored[0]
                .p1
                .as_deref()
                .unwrap()
                .contains(&case.waiting_line())
        );
    }
}

#[test]
fn a_summary_that_records_the_answer_and_keeps_the_waiting_line_renders_the_answer_alone() {
    for case in ANSWERED {
        let messages = case.answered();
        let chunk = chunk_of(&messages, 1, 5);
        // The summarizer writes the decision and repeats the waiting line beside it, in a
        // different order in each tier.
        let p1 = format!(
            "I worked through the request.\n{}\n{}\nI made the change.",
            case.waiting_line(),
            case.decided_line()
        );
        let p2 = format!("{}\n{}", case.decided_line(), case.waiting_line());
        let p3 = case.waiting_line();
        let text = output(
            &[segment_xml(1, 4, [&p1, &p2, &p3, ""])],
            &[claim(
                case.key,
                &cite(&chunk, 3, case.answer),
                case.answer,
                &case.decided_line(),
            )],
            5,
        );
        let mut stored = Vec::new();
        fold(&mut stored, &text, &chunk);

        let [recent, consolidated, old] = &rendered_tiers(&stored)[0];
        assert!(recent.ends_with(&format!(
            "I worked through the request.\n{}\nI made the change.",
            case.decided_line()
        )));
        assert!(
            consolidated.ends_with(&format!("\n{}", case.decided_line())),
            "{consolidated}"
        );
        // The third tier held the waiting line alone, so the row renders its heading there.
        assert_eq!(old, "## 1-4 \u{b7} Work 1-4");
    }
}

#[test]
fn an_unanswered_question_leaves_once_a_later_segment_passes_over_it() {
    let case = &ANSWERED[0];
    let mut messages = case.asking();
    messages.push(user(
        3,
        "Separate task: add a changelog entry for the last release.",
    ));
    messages.push(assistant(
        4,
        "The changelog has the entry for the last release.",
    ));
    let mut stored = Vec::new();
    fold(
        &mut stored,
        &case.first_output(),
        &chunk_of(&messages, 1, 3),
    );
    assert_eq!(
        open_lines(&rendered_tiers(&stored)[0][0]),
        [case.waiting_line()]
    );

    // The user turned to other work, and the fold of that span states nothing for the key.
    let p1 = "The user asked for a changelog entry; I added it.";
    let second = output(&[segment_xml(3, 4, [p1, p1, p1, ""])], &[], 5);
    fold(&mut stored, &second, &chunk_of(&messages, 3, 5));

    for tiers in rendered_tiers(&stored) {
        for tier in tiers {
            assert_eq!(open_lines(&tier), Vec::<&str>::new());
        }
    }
}

#[test]
fn a_question_raised_again_in_a_later_segment_renders_once_there() {
    let case = &ANSWERED[2];
    let mut messages = case.asking();
    messages.push(user(3, "First show me the current poller logs."));
    messages.push(assistant(
        4,
        &format!(
            "Here are the logs. I still need your call: {}",
            case.question
        ),
    ));
    let mut stored = Vec::new();
    fold(
        &mut stored,
        &case.first_output(),
        &chunk_of(&messages, 1, 3),
    );
    let p1 = format!("I showed the poller logs.\n{}", case.waiting_line());
    let second = output(
        &[segment_xml(3, 4, [&p1, &p1, "Showed the logs.", ""])],
        &[],
        5,
    );
    fold(&mut stored, &second, &chunk_of(&messages, 3, 5));

    let history = rendered_tiers(&stored);
    assert_eq!(open_lines(&history[0][0]), Vec::<&str>::new());
    assert_eq!(open_lines(&history[1][0]), [case.waiting_line()]);
}

#[test]
fn a_question_raised_again_after_its_key_was_settled_stays_open_until_its_next_answer() {
    let case = &ANSWERED[2];
    let mut messages = case.asking();
    messages.extend([
        user(3, "Before I pick, show me the current poller logs."),
        assistant(4, "Here are the poller logs for the last hour."),
        user(5, "Also add a debug flag to the poller."),
        assistant(
            6,
            &format!("The flag is in. I still need your call: {}", case.question),
        ),
        user(7, "Run the poller tests."),
        assistant(
            8,
            &format!("The tests pass. Still open on your side: {}", case.question),
        ),
        user(9, &format!("{}.", case.answer)),
        assistant(10, "Done as you said; the change is in place."),
    ]);
    let waiting = case.waiting_line();
    let put_off = format!(
        "Decided ({}): The user put the choice off until the poller logs are shown.",
        case.key
    );
    let status_of_each_exchange = [&waiting, &put_off, &waiting, &waiting, &case.decided_line()];
    let mut stored = Vec::new();
    for (at, status) in status_of_each_exchange.into_iter().enumerate() {
        let start = 2 * at as u64 + 1;
        let p1 = format!("I worked on the poller.\n{status}");
        fold(
            &mut stored,
            &output(
                &[segment_xml(start, start + 1, [&p1, &p1, &p1, ""])],
                &[],
                start + 2,
            ),
            &chunk_of(&messages, start, start + 2),
        );
        // The newest segment determines the key's open status, even after an earlier settlement.
        let history = rendered_tiers(&stored);
        assert_eq!(history.len(), at + 1, "fold {at}");
        for (row, tiers) in history.iter().enumerate() {
            let open = if row == at && *status == waiting {
                vec![waiting.as_str()]
            } else {
                vec![]
            };
            let stated = status_of_each_exchange[row];
            for tier in tiers {
                assert_eq!(open_lines(tier), open, "fold {at} row {row}");
                // A settling line stays in its segment while a newer segment has the key open.
                assert!(
                    *stated == waiting || tier.contains(stated.as_str()),
                    "fold {at} row {row}: {tier}"
                );
            }
        }
    }
}

const CONSTRAINT: &str = "Do not fix product bugs during the baseline measurement";
const LIFT: &str = "fix each of these bugs in its own pull request";
const CONSTRAINT_LINE: &str =
    "Constraint (policy.baseline-fixes): No product bug fixes during the baseline measurement.";
const LIFTED_LINE: &str = "Lifted (policy.baseline-fixes): The no-fixes rule is lifted; the user wants each bug fixed in its own pull request.";

/// The user sets a constraint, unrelated work follows, and the user lifts the constraint.
fn constraint_session() -> Vec<IngressMessage> {
    vec![
        user(1, &format!("Measure the baseline first. {CONSTRAINT}.")),
        assistant(
            2,
            &format!(
                "{}Baseline measured. I found two parser bugs and left both unfixed.",
                work_log("the baseline run")
            ),
        ),
        user(3, "Now write up the measurement method."),
        assistant(4, "The method write-up is in the notes file."),
        user(5, &format!("The baseline is done, so {LIFT}.")),
        assistant(6, "I opened two pull requests, one per parser fix."),
    ]
}

/// The first two folds of [`constraint_session`]: the constraint, then unrelated work.
fn constraint_then_other_work(messages: &[IngressMessage]) -> Vec<StoredHistorySegment> {
    let first_chunk = chunk_of(messages, 1, 3);
    let p1 =
        format!("{CONSTRAINT_LINE}\nI measured the baseline and left two parser bugs unfixed.");
    let first = output(&[segment_xml(1, 2, [&p1, &p1, &p1, ""])], &[], 3);
    let mut stored = Vec::new();
    fold(&mut stored, &first, &first_chunk);
    let p1 = "I wrote up the measurement method.";
    let second = output(&[segment_xml(3, 4, [p1, p1, p1, ""])], &[], 5);
    fold(&mut stored, &second, &chunk_of(messages, 3, 5));
    stored
}

#[test]
fn a_constraint_stands_in_every_tier_until_a_later_fold_stores_its_lift_claim() {
    let messages = constraint_session();
    let mut stored = constraint_then_other_work(&messages);
    for tier in &rendered_tiers(&stored)[0] {
        assert_eq!(open_lines(tier), [CONSTRAINT_LINE]);
    }

    let third_chunk = chunk_of(&messages, 5, 7);
    let p1 = format!("{LIFTED_LINE}\nI opened two pull requests.");
    let third = output(
        &[segment_xml(5, 6, [&p1, &p1, &p1, ""])],
        &[claim(
            "policy.baseline-fixes",
            &cite(&third_chunk, 5, LIFT),
            LIFT,
            LIFTED_LINE,
        )],
        7,
    );
    fold(&mut stored, &third, &third_chunk);

    let history = rendered_tiers(&stored);
    for tier in &history[0] {
        assert_eq!(
            tier,
            "## 1-2 \u{b7} Work 1-2\nI measured the baseline and left two parser bugs unfixed."
        );
    }
    for tier in &history[2] {
        assert!(tier.contains(LIFTED_LINE), "{tier}");
    }
}

#[test]
fn a_lift_whose_claim_fails_validation_leaves_the_constraint_standing() {
    let messages = constraint_session();
    let mut stored = constraint_then_other_work(&messages);
    let third_chunk = chunk_of(&messages, 5, 7);
    // The lift's claim cites a byte range past its part, so claim validation drops it.
    let past_the_part = cite(&third_chunk, 5, LIFT)
        .rsplit_once('-')
        .map(|(start, _)| format!("{start}-99999]"))
        .unwrap();
    let p1 = format!("{LIFTED_LINE}\nI opened two pull requests.");
    let third = output(
        &[segment_xml(5, 6, [&p1, &p1, &p1, ""])],
        &[claim(
            "policy.baseline-fixes",
            &past_the_part,
            LIFT,
            LIFTED_LINE,
        )],
        7,
    );
    let rows = fold(&mut stored, &third, &third_chunk);
    assert_eq!(rows[0].claims, Vec::new());

    let history = rendered_tiers(&stored);
    assert_eq!(open_lines(&history[0][0]), [CONSTRAINT_LINE]);
    assert!(history[2][0].contains(LIFTED_LINE));
}

/// Claim validation keeps a lift's anchor only as a whole `Lifted` line of `p1`, with or without
/// its bullet, on a claim that cites a user message; every other lift claim stores no anchor and
/// leaves the constraint standing.
#[test]
fn a_lift_takes_effect_only_as_a_whole_line_whose_claim_cites_a_user_message() {
    let mut messages = constraint_session();
    // The assistant repeats the user's words, so a claim can cite either message.
    messages[5] = assistant(
        6,
        &format!("Understood: {LIFT}. I opened two pull requests."),
    );
    let lifted_with = |p1: &str, cited: u64, value: &str, anchor: &str| {
        let mut stored = constraint_then_other_work(&messages);
        let chunk = chunk_of(&messages, 5, 7);
        let text = output(
            &[segment_xml(5, 6, [p1, p1, p1, ""])],
            &[claim(
                "policy.baseline-fixes",
                &cite(&chunk, cited, LIFT),
                value,
                anchor,
            )],
            7,
        );
        let rows = fold(&mut stored, &text, &chunk);
        (
            rows[0].claims[0].anchor.clone(),
            open_lines(&rendered_tiers(&stored)[0][0]).len(),
        )
    };
    let lifted = |p1: &str, cited: u64, anchor: &str| lifted_with(p1, cited, LIFT, anchor);
    let line = format!("{LIFTED_LINE}\nI opened two pull requests.");
    // The user's message, the whole line, with or without its bullet: the constraint leaves.
    assert_eq!(lifted(&line, 5, LIFTED_LINE), (Some(LIFTED_LINE.into()), 0));
    assert_eq!(
        lifted(&format!("- {line}"), 5, LIFTED_LINE),
        (Some(LIFTED_LINE.into()), 0)
    );
    // The assistant's message.
    assert_eq!(lifted(&line, 6, LIFTED_LINE), (None, 1));
    // A claim that retracts the key's value and so holds none of the user's words.
    assert_eq!(lifted_with(&line, 5, "", LIFTED_LINE), (None, 1));
    // The lifted form inside a sentence, with the form alone as the anchor.
    let inside = format!("The user did not say {LIFTED_LINE}");
    assert_eq!(lifted(&inside, 5, LIFTED_LINE), (None, 1));
    // Part of the line.
    let part = &LIFTED_LINE[..LIFTED_LINE.len() - 20];
    assert_eq!(lifted(&line, 5, part), (None, 1));
    // The anchor with the line's bullet, and a line that ends in spaces.
    let bulleted = format!("- {LIFTED_LINE}");
    assert_eq!(
        lifted(&format!("- {line}"), 5, &bulleted),
        (Some(bulleted.clone()), 0)
    );
    let spaced = format!("{LIFTED_LINE}   \nI opened two pull requests.");
    assert_eq!(
        lifted(&spaced, 5, LIFTED_LINE),
        (Some(LIFTED_LINE.into()), 0)
    );
    assert_eq!(
        lifted(&format!("- {spaced}"), 5, LIFTED_LINE),
        (Some(LIFTED_LINE.into()), 0)
    );
    // A line with two bullets is text, and its tail is no lift.
    assert_eq!(lifted(&format!("- - {line}"), 5, &bulleted), (None, 1));
    // The rule reads `Lifted` lines alone: a decision's anchor stays on the assistant's
    // message and as part of its line.
    let decided = "Decided (policy.baseline-fixes): The assistant restated the plan.";
    let narrative = format!("{decided}\nI opened two pull requests.");
    assert_eq!(lifted(&narrative, 6, decided), (Some(decided.into()), 1));
    assert_eq!(
        lifted(&narrative, 6, "The assistant restated the plan."),
        (Some("The assistant restated the plan.".into()), 1)
    );
}

#[test]
fn a_lift_claim_that_cites_words_no_user_wrote_leaves_the_constraint_standing() {
    let said = |text: String| BlockKind::Text { text };
    let attachment = BlockKind::Media(memory_store::MediaBlock {
        kind: memory_store::MediaKind::File,
        media_type: "text/plain".into(),
        filename: Some(format!("{LIFT}.txt")),
        source: json!({"type": "data_base64", "data": "stable-bytes"}),
    });
    let nested_reminder =
        format!("<system-reminder><system-reminder>x</system-reminder>{LIFT}</system-reminder>");
    let cases = [
        ("open-items-5", said(format!("So {LIFT}.")), true),
        ("open-items-5", attachment, false),
        ("open-items-5", said(nested_reminder), false),
        ("eidnara:custom:5", said(LIFT.into()), false),
    ];
    for (mid, block, lifts) in cases {
        let mut messages = constraint_session();
        messages[4] = IngressMessage {
            mid: mid.into(),
            ordinal: 5,
            ck: WireMessage::from_parts(
                "user",
                vec![WireBlock::bare(block)],
                None,
                ProviderExtras::new(),
                HarnessMeta {
                    harness_id: Some(mid.into()),
                    ..Default::default()
                },
            ),
        };
        let mut stored = constraint_then_other_work(&messages);
        let chunk = chunk_of(&messages, 5, 7);
        let p1 = format!("{LIFTED_LINE}\nI opened two pull requests.");
        let text = output(
            &[segment_xml(5, 6, [&p1, &p1, &p1, ""])],
            &[claim(
                "policy.baseline-fixes",
                &cite(&chunk, 5, LIFT),
                LIFT,
                LIFTED_LINE,
            )],
            7,
        );
        let rows = fold(&mut stored, &text, &chunk);
        let presented = &chunk.aliases.aliases[0].presented;
        assert_eq!(rows[0].claims[0].value, LIFT, "{presented}");
        assert_eq!(
            rows[0].claims[0].anchor.is_some(),
            lifts,
            "{mid} {presented}"
        );
        let standing = if lifts { vec![] } else { vec![CONSTRAINT_LINE] };
        assert_eq!(
            open_lines(&rendered_tiers(&stored)[0][0]),
            standing,
            "{mid} {presented}"
        );
    }
}

#[test]
fn an_answer_and_a_lift_under_other_keys_leave_the_constraint_standing() {
    let case = &ANSWERED[0];
    let messages = vec![
        user(1, &format!("{} {CONSTRAINT}.", case.request)),
        assistant(
            2,
            &format!("{}{}", work_log("the migration"), case.question),
        ),
        user(3, &format!("{}. Also, {LIFT}.", case.answer)),
        assistant(4, "Continuing with the remaining steps."),
    ];
    let first_chunk = chunk_of(&messages, 1, 3);
    let p1 = format!(
        "{CONSTRAINT_LINE}\nI started the migration.\n{}",
        case.waiting_line()
    );
    let first = output(&[segment_xml(1, 2, [&p1, &p1, &p1, ""])], &[], 3);
    let mut stored = Vec::new();
    fold(&mut stored, &first, &first_chunk);

    // The second fold names both outcomes with keys of its own.
    let second_chunk = chunk_of(&messages, 3, 5);
    let decided = format!("Decided (migration.step-order): {}", case.decision);
    let lifted =
        "Lifted (policy.bug-fixes): The user wants each bug fixed in its own pull request.";
    let p1 = format!("{decided}\n{lifted}");
    let second = output(
        &[segment_xml(3, 4, [&p1, &p1, &p1, ""])],
        &[
            claim(
                "migration.step-order",
                &cite(&second_chunk, 3, case.answer),
                case.answer,
                &decided,
            ),
            claim(
                "policy.bug-fixes",
                &cite(&second_chunk, 3, LIFT),
                LIFT,
                lifted,
            ),
        ],
        5,
    );
    let rows = fold(&mut stored, &second, &second_chunk);
    assert_eq!(
        rows[0]
            .claims
            .iter()
            .map(|claim| claim.key.as_str())
            .collect::<Vec<_>>(),
        ["migration.step-order", "policy.bug-fixes"]
    );

    let history = rendered_tiers(&stored);
    for tier in &history[0] {
        assert_eq!(open_lines(tier), [CONSTRAINT_LINE]);
    }
    assert!(history[1][0].contains(&decided) && history[1][0].contains(lifted));
}

#[test]
fn a_later_segment_of_the_same_fold_lifts_an_earlier_segments_constraint() {
    let messages = constraint_session();
    let chunk = chunk_of(&messages, 1, 7);
    let set = format!("{CONSTRAINT_LINE}\nI measured the baseline.");
    let lifted = format!("{LIFTED_LINE}\nI opened two pull requests.");
    let text = output(
        &[
            segment_xml(1, 4, [&set, &set, &set, ""]),
            segment_xml(5, 6, [&lifted, &lifted, &lifted, ""]),
        ],
        &[claim(
            "policy.baseline-fixes",
            &cite(&chunk, 5, LIFT),
            LIFT,
            LIFTED_LINE,
        )],
        7,
    );
    let mut stored = Vec::new();
    assert_eq!(fold(&mut stored, &text, &chunk).len(), 2);

    let history = rendered_tiers(&stored);
    for tier in &history[0] {
        assert_eq!(open_lines(tier), Vec::<&str>::new());
        assert!(tier.ends_with("I measured the baseline."), "{tier}");
    }
    assert!(history[1][0].contains(LIFTED_LINE));
}

/// The baseline `compose_m0` writes for session `ses` of `store` under a 60,000-token history
/// budget, with memory and project documents left out.
fn composed_baseline(store: &MemoryStore) -> String {
    crate::m0_compose::compose_m0(
        store,
        &crate::m0_compose::M0ComposeInputs {
            session_id: "ses",
            project_path: "git:proj",
            project_directory: "/nonexistent-docs",
            now_ms: 0,
            history_budget_tokens: 60_000.0,
            covered_system_messages: &[],
            inject_memory: false,
            user_profile_budget_tokens: 0.0,
            inject_docs: false,
            project_doc_files: &[],
            document_reads: None,
            temporal_awareness: false,
            legacy_history_segment_seqs: None,
        },
        &[],
        tokenizer::estimate_tokens,
    )
    .unwrap()
    .m0_bytes
}

/// A long session whose constraint sits in an important old row and whose lift sits in a newer
/// row the decay curve archives. The bounded read loads the archived row as its claims alone,
/// and the baseline still retires the constraint, as a read of every row does.
#[test]
fn a_bounded_read_lifts_a_constraint_through_a_row_it_loads_without_text() {
    use crate::test_support::synthetic_history::SyntheticHistory;

    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&crate::test_support::descriptor(dir.path())).unwrap();
    SyntheticHistory::mixed(400).seed(&store, "ses");
    let set = format!("{CONSTRAINT_LINE}\nBaseline measured in the important old row.");
    let lifted = format!("{LIFTED_LINE}\nTwo pull requests opened.");
    let lift_claim = serde_json::to_string(&[memory_store::Claim {
        key: "policy.baseline-fixes".into(),
        value: LIFT.into(),
        ordinal: 705,
        anchor: Some(LIFTED_LINE.into()),
    }])
    .unwrap();
    let rewrite = |sequence: i64, text: &str, importance: i64, claims: &str| {
        store
            .with_fenced_conn_for_test(|tx| {
                let changed = tx.execute(
                    "UPDATE history_segments
                        SET content = ?3, p1 = ?3, p2 = ?3, p3 = ?3, importance = ?4, claims = ?5,
                            legacy = 0
                      WHERE session_id = ?1 AND sequence = ?2",
                    rusqlite::params!["ses", sequence, text, importance, claims],
                )?;
                assert_eq!(changed, 1);
                Ok(())
            })
            .unwrap();
    };
    rewrite(40, &set, 100, "[]");
    rewrite(70, &lifted, 1, "[]");

    // With no lift claim stored, the constraint renders, and the newer row is archived: the
    // baseline holds the old row's text and none of the lifting row's.
    let standing = composed_baseline(&store);
    assert_eq!(open_lines(&standing), [CONSTRAINT_LINE], "{standing}");
    assert!(
        !standing.contains("Two pull requests opened."),
        "{standing}"
    );

    rewrite(70, &lifted, 1, &lift_claim);
    let retired = composed_baseline(&store);
    assert_eq!(open_lines(&retired), Vec::<&str>::new(), "{retired}");
    assert!(retired.contains("Baseline measured in the important old row."));
    assert!(!retired.contains("Two pull requests opened."), "{retired}");
}

/// A session in two spans: the user sets a constraint and the assistant ends on a question, then
/// the user answers the question and lifts the constraint.
fn question_under_a_constraint(case: &Answered) -> (Vec<IngressMessage>, Vec<IngressMessage>) {
    let log = work_log("the migration");
    let asking = vec![
        user(1, &format!("{} {CONSTRAINT}.", case.request)),
        assistant(
            2,
            &format!("{log}I finished the first step. {}", case.question),
        ),
    ];
    let answering = vec![
        user(3, &format!("{}. Also, {LIFT}.", case.answer)),
        assistant(
            4,
            &format!("{log}The remaining steps are done, and two pull requests hold the fixes."),
        ),
    ];
    (asking, answering)
}

/// `messages` followed by a padded tail, which keeps them outside the live window so a fold
/// covers all of them.
fn with_tail(mut messages: Vec<IngressMessage>) -> Vec<IngressMessage> {
    let next = messages.last().unwrap().ordinal + 1;
    messages.push(user(
        next,
        &format!("unrelated tail {}", "pad ".repeat(3000)),
    ));
    messages.push(user(next + 1, "What is left to do?"));
    messages
}

/// The first fold ends waiting on the user and records a user constraint. The second fold holds
/// the user's answer and the lift, then a second question and its answer, in two segments. Both
/// folds run through the production handler. The next request carries the outcomes in the rows
/// above its frozen baseline with the second question retired there, and the baseline the next
/// rebuild writes holds the outcomes alone.
#[tokio::test(flavor = "current_thread")]
async fn a_fold_that_settles_open_items_reaches_the_next_request_and_the_next_baseline() {
    let (case, follow_up) = (&ANSWERED[0], &ANSWERED[2]);
    let (asking, mut answering) = question_under_a_constraint(case);
    answering.extend([
        assistant(5, &format!("One more point. {}", follow_up.question)),
        user(6, &format!("{}.", follow_up.answer)),
        assistant(7, "The poller keeps its interval."),
        user(8, "Now check the changelog entry for this release."),
        assistant(9, "The changelog entry is in place."),
    ]);
    let first_messages = with_tail(asking.clone());
    let first_p1 = format!(
        "{CONSTRAINT_LINE}\nI finished the first migration step.\n{}",
        case.waiting_line()
    );
    let first_output = output(
        &[segment_xml(
            1,
            2,
            [
                &first_p1,
                &first_p1,
                "Finished the first migration step.",
                "",
            ],
        )],
        &[],
        3,
    );
    let second_messages = with_tail([asking, answering].concat());
    let second_chunk = chunk_of(&second_messages, 3, 10);
    let settled = format!(
        "{}\n{LIFTED_LINE}\nI finished the remaining steps.\n{}",
        case.decided_line(),
        follow_up.waiting_line()
    );
    let answered = format!("{}\nThe poller is unchanged.", follow_up.decided_line());
    // Validation withholds the answer's last segment for the next fold to derive again, so a
    // third segment follows the two this test reads.
    let checked = "I checked the changelog entry.";
    let second_output = output(
        &[
            segment_xml(3, 5, [&settled, &settled, &settled, ""]),
            segment_xml(6, 7, [&answered, &answered, &answered, ""]),
            segment_xml(8, 9, [checked, checked, checked, ""]),
        ],
        &[claim(
            "policy.baseline-fixes",
            &cite(&second_chunk, 3, LIFT),
            LIFT,
            LIFTED_LINE,
        )],
        10,
    );
    let producer = Arc::new(ProducerState::default());
    producer
        .await_results
        .lock()
        .unwrap()
        .extend([first_output, second_output].map(|text| {
            Ok(ProducerOutput {
                text,
                length_capped: false,
            })
        }));
    let (handler, store, _dir, _project) =
        handler_with_store(Arc::clone(&producer), default_test_config());
    let pass = |messages: Vec<IngressMessage>| {
        call_transform_request(
            &handler,
            request_with_usage(messages, HIGH_PRESSURE_USAGE, CONTEXT_LIMIT),
        )
    };
    let open = [CONSTRAINT_LINE.to_owned(), case.waiting_line()];
    let fired = pass(first_messages.clone()).await;
    assert_eq!(fired["history_summarizer"]["fired"], true, "{fired}");
    wait_for_count(&producer.starts, 1).await;
    wait_for_idle(&store).await;
    // The request after the first fold carries the open question and the constraint.
    let served = pass(first_messages).await;
    let history = synthetic_text(&served, 0) + &synthetic_text(&served, 1);
    assert_eq!(open_lines(&history), open, "{history}");
    let fired = pass(second_messages.clone()).await;
    assert_eq!(fired["history_summarizer"]["fired"], true, "{fired}");
    wait_for_count(&producer.starts, 2).await;
    wait_for_idle(&store).await;

    // The stored rows keep both folds as written.
    let rows = store.load_history_segments("ses").unwrap();
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert_eq!(open_lines(rows[0].p1.as_deref().unwrap()), open);
    assert_eq!(
        open_lines(rows[1].p1.as_deref().unwrap()),
        [follow_up.waiting_line()]
    );

    // The frozen baseline keeps its bytes until its next rebuild. The rows above it state the
    // outcomes, the second question leaves the older of them, and the update block names the
    // lift under the constraint's key.
    let served = pass(second_messages).await;
    let (baseline, since) = (synthetic_text(&served, 0), synthetic_text(&served, 1));
    assert_eq!(open_lines(&baseline), open, "{baseline}");
    assert_eq!(open_lines(&since), Vec::<&str>::new(), "{since}");
    for outcome in [
        case.decided_line().as_str(),
        LIFTED_LINE,
        follow_up.decided_line().as_str(),
        &format!("policy.baseline-fixes = {LIFT} @3"),
    ] {
        assert!(since.contains(outcome), "{since}");
    }

    // The baseline the next rebuild writes from the stored rows holds the outcomes alone.
    let rebuilt = composed_baseline(&store);
    assert_eq!(open_lines(&rebuilt), Vec::<&str>::new(), "{rebuilt}");
    for outcome in [
        case.decided_line().as_str(),
        LIFTED_LINE,
        follow_up.decided_line().as_str(),
    ] {
        assert!(rebuilt.contains(outcome), "{rebuilt}");
    }
}

const LIVE_CONNECTION_FILE: &str = "EIDNARA_FIDELITY_REAL_CONNECTION_FILE";
const LIVE_MODEL: &str = "EIDNARA_FIDELITY_REAL_MODEL";
const LIVE_WAIT_SECONDS: &str = "EIDNARA_FIDELITY_REAL_WAIT_SECONDS";

/// A session whose folds run through a real summarizer model.
struct LiveSession {
    handler: Handler,
    store: Arc<MemoryStore>,
    project: PathBuf,
    factory: Arc<RealHistorySummarizerProducerFactory>,
    wait: Duration,
    _dir: tempfile::TempDir,
}

impl LiveSession {
    /// Folds `messages`, with a padded tail, as firing `firing` of session `ses`, and returns
    /// every stored row.
    async fn fold(&self, firing: u64, messages: Vec<IngressMessage>) -> Vec<StoredHistorySegment> {
        let request = request_with_usage(with_tail(messages), HIGH_PRESSURE_USAGE, CONTEXT_LIMIT);
        let fired = tokio::time::timeout(self.wait, call_transform_request(&self.handler, request))
            .await
            .expect("the transform returns before the deadline");
        assert_eq!(fired["history_summarizer"]["fired"], true, "{fired}");
        tokio::time::timeout(self.wait, async {
            loop {
                let state = self.store.load("ses").unwrap().meta.history_summarizer;
                if state.firing_seq >= firing && state.state == HistorySummarizerPhase::Idle {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        })
        .await
        .expect("the firing settles before the deadline");
        // The host keeps a producer session after its run ends, so the session is deleted
        // before the next fold's model call.
        let mut driver = self
            .factory
            .connect(&self.project, "opencode", &BTreeMap::new())
            .await
            .expect("the host accepts a cleanup connection");
        driver
            .purge_session(
                &crate::history_summarizer::history_summarizer_producer_session_id(
                    &project_slug(&self.project),
                    "ses",
                    firing,
                ),
            )
            .await
            .expect("the host deletes the fold's producer session");
        let rows = self.store.load_history_segments("ses").unwrap();
        for row in &rows {
            eprintln!(
                "live fold {firing} row {}-{}:\np1: {:?}\np2: {:?}\np3: {:?}\nclaims: {:?}",
                row.start_message, row.end_message, row.p1, row.p2, row.p3, row.claims
            );
        }
        rows
    }
}

/// The status lines of `status` that `tiers` render.
fn rendered_lines(tiers: &[[String; 3]], status: Status) -> Vec<&str> {
    tiers
        .iter()
        .flatten()
        .flat_map(|tier| tier.lines())
        .filter(|line| status_line(line).is_some_and(|(found, _)| found == status))
        .collect()
}

/// Two folds through a real summarizer model: the first ends on the assistant's question under a
/// user constraint, the second holds the user's answer and the lift. After the first fold the
/// history renders the question as waiting and the constraint as standing; after the second the
/// stored rows hold the decision and the lift's claim, and the history renders both items settled.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "opt-in live summarizer run; needs EIDNARA_FIDELITY_REAL_CONNECTION_FILE naming a \
            host that admits sends without a credential claim (direct_host_fixture without \
            --harness-runtime), EIDNARA_FIDELITY_REAL_MODEL and EIDNARA_FIDELITY_REAL_WAIT_SECONDS"]
async fn a_live_summarizer_settles_open_items_across_two_folds() {
    let required = |name: &str| {
        std::env::var(name).unwrap_or_else(|_| panic!("{name} is required for the live run"))
    };
    let factory = Arc::new(RealHistorySummarizerProducerFactory {
        connection_file: PathBuf::from(required(LIVE_CONNECTION_FILE)),
        cancellation: CancellationToken::new(),
    });
    let wait = Duration::from_secs(required(LIVE_WAIT_SECONDS).parse().unwrap());
    let config = DaemonConfig {
        model_chain: vec![required(LIVE_MODEL)],
        ..default_test_config()
    };
    let (handler, store, dir, project) = handler_with_factory(
        Arc::clone(&factory) as Arc<dyn HistorySummarizerProducerFactory>,
        config,
        Arc::new(MissingSessionResolver),
        "opencode",
    );
    let session = LiveSession {
        handler,
        store,
        project,
        factory,
        wait,
        _dir: dir,
    };
    let (asking, answering) = question_under_a_constraint(&ANSWERED[0]);

    let rows = session.fold(1, asking.clone()).await;
    let history = rendered_tiers(&rows);
    assert!(
        !rendered_lines(&history, Status::Waiting).is_empty(),
        "{history:#?}"
    );
    assert!(
        !rendered_lines(&history, Status::Constraint).is_empty(),
        "{history:#?}"
    );

    let rows = session.fold(2, [asking, answering].concat()).await;
    // The second fold's rows state the decision and store the claim that lifts the constraint.
    let settling: Vec<Status> = rows
        .iter()
        .filter(|row| row.end_message >= 3)
        .filter_map(|row| row.p1.as_deref())
        .flat_map(str::lines)
        .filter_map(status_line)
        .map(|(status, _)| status)
        .collect();
    assert!(settling.contains(&Status::Decided), "{rows:#?}");
    assert!(
        rows.iter().flat_map(|row| &row.claims).any(|claim| {
            claim.ordinal == 3
                && claim.anchor.as_deref().and_then(status_line)
                    == Some((Status::Lifted, claim.key.as_str()))
        }),
        "{rows:#?}"
    );
    let history = rendered_tiers(&rows);
    assert_eq!(
        rendered_lines(&history, Status::Waiting),
        Vec::<&str>::new()
    );
    assert_eq!(
        rendered_lines(&history, Status::Constraint),
        Vec::<&str>::new()
    );
    assert!(
        !rendered_lines(&history, Status::Lifted).is_empty(),
        "{history:#?}"
    );
}
