//! Open items in history_summarizer tiers: the status lines a fold carries for questions that wait
//! on the user and for constraints the user set, and the pass that retires the ones the same
//! output settles or that a segment has aged past.
//!
//! A status line is one tier line in one of four forms, each naming its item by a claim key. It
//! may start with one `- ` or `* ` bullet and carries no other emphasis or numbering:
//!
//! - `Waiting on user (key): …`: a question I put to the user that the folded span leaves open.
//! - `Constraint (key): …`: a standing limit the user set for the work.
//! - `Decided (key): …`: the user's answer to an item, from this span or an earlier fold.
//! - `Lifted (key): …`: a user constraint the user later lifted, stated as lifted.
//!
//! Waiting and constraint lines are open; decided and lifted lines settle the item with their key.
//! The summarizer resolves each item against the user turns of the folded span. This pass runs
//! inside validation, on the parsed segments before their tiers are copied and before claims are
//! checked, and enforces the part of that resolution the output's own structure shows:
//!
//! - An open line leaves its tier when a settling line with its key follows it in that tier, or
//!   when a settling line with its key appears in the `p1` or `p2` of a later segment of the same
//!   output. A question the user answered or a constraint the user lifted inside this span keeps
//!   only the outcome, and a question asked again after its answer stays open.
//! - When the segment's `p1` ends a key's status lines with a settling line, an open line with that
//!   key in `p2`, `p3`, or `p4` gives way to that settling line, so every tier carries the outcome;
//!   a tier that already states the outcome just loses the open line.
//! - Every line that begins `Waiting on user (` leaves `p3` and `p4`, whatever its key, or gives way
//!   to `p1`'s settling line for its key. Those tiers render once a segment has aged past its
//!   recent tiers, and they carry outcomes.
//! - A retirement never leaves `p1` empty; a `p1` that would hold nothing else keeps its lines.
//!
//! A key whose open line left `p1` keeps only the claim candidates whose anchor still occurs in
//! some segment's final `p1`, which is the settling line's claim when the model anchored it there.
//! The candidates of the retired open line, and any candidate of that key with no anchor, leave
//! before claim validation, so emission order cannot make the open claim the kept one.
//!
//! The prompt has each status line carry a claim with its key, citing the asking part for a
//! waiting line, the user's message for a constraint, and the user's answer for a settling line.
//! When an earlier fold's waiting or constraint claim and a later fold's settling claim both pass
//! claim validation, the later claim is current and the render-time correction pass marks the
//! earlier line: the correction replaces the line in place when the claim's anchor occurs in the
//! rendered tier, the marker passes the durable scanner, and the hit passes the overlap rule, and it
//! is listed on the segment's `[corrections: …]` line beside the unchanged text otherwise. A waiting
//! line whose claims fail validation stays as written in `p1` and `p2`, and an unsettled constraint
//! line stays in every tier.
//!
//! The pass removes lines and claim candidates, and each replacement puts one of the model's own
//! settling lines where one open line stood, so the stored summary is bounded by what the model
//! emitted.

use std::collections::{BTreeMap, BTreeSet};

use crate::history_summarizer_citations::is_claim_key;
use crate::history_summarizer_validate::{ParsedHistorySegment, ParsedHistorySegmentOutput};

/// The prefix of a status line for a question that waits on the user.
pub(crate) const WAITING_PREFIX: &str = "Waiting on user (";

/// Whether a status line leaves its item open or settles it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    Open,
    Settled,
}

const PREFIXES: [(&str, Status); 4] = [
    (WAITING_PREFIX, Status::Open),
    ("Constraint (", Status::Open),
    ("Decided (", Status::Settled),
    ("Lifted (", Status::Settled),
];

/// `line` without leading whitespace and one leading `- ` or `* ` bullet.
fn unbulleted(line: &str) -> &str {
    let line = line.trim_start();
    line.strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .unwrap_or(line)
        .trim_start()
}

/// The status and key of `line` when it is a status line whose key follows the claim-key grammar.
pub(crate) fn status_line(line: &str) -> Option<(Status, &str)> {
    let line = unbulleted(line);
    let (status, rest) = PREFIXES
        .iter()
        .find_map(|(prefix, status)| line.strip_prefix(prefix).map(|rest| (*status, rest)))?;
    let (key, _) = rest.split_once("):")?;
    is_claim_key(key).then_some((status, key))
}

fn settles(line: &str, key: &str) -> bool {
    status_line(line) == Some((Status::Settled, key))
}

/// What happens to one tier line.
enum Fate {
    Keep,
    Drop,
    Replace(String),
}

/// Retires the open lines the output settles and the waiting lines of `p3` and `p4` per the module
/// rules, keeps each segment's `content` equal to its `p1`, removes the claim candidates of open
/// lines retired from `p1`, and returns the segments.
pub(crate) fn retire_settled_lines(
    output: &mut ParsedHistorySegmentOutput,
) -> &[ParsedHistorySegment] {
    let segments = &mut output.history_segments;
    // Each segment's keys that a settling line in `p1` or `p2` names.
    let settled_in: Vec<BTreeSet<String>> = segments
        .iter()
        .map(|segment| {
            [segment.p1.as_deref(), segment.p2.as_deref()]
                .into_iter()
                .flatten()
                .flat_map(str::lines)
                .filter_map(status_line)
                .filter(|(status, _)| *status == Status::Settled)
                .map(|(_, key)| key.to_owned())
                .collect()
        })
        .collect();
    let mut retired_from_p1: BTreeSet<String> = BTreeSet::new();
    for (index, segment) in segments.iter_mut().enumerate() {
        let Some(p1) = segment.p1.as_deref() else {
            continue;
        };
        let settled_later = |key: &str| {
            settled_in[index + 1..]
                .iter()
                .any(|keys| keys.contains(key))
        };
        // Each key's last status line in `p1`; a settling one is the outcome every tier carries.
        let mut last_in_p1: BTreeMap<&str, (Status, &str)> = BTreeMap::new();
        for line in p1.lines() {
            if let Some((status, key)) = status_line(line) {
                last_in_p1.insert(key, (status, unbulleted(line)));
            }
        }
        let outcome: BTreeMap<String, String> = last_in_p1
            .into_iter()
            .filter(|(_, (status, _))| *status == Status::Settled)
            .map(|(key, (_, line))| (key.to_owned(), line.to_owned()))
            .collect();
        for (tier, text) in [
            (1, &mut segment.p1),
            (2, &mut segment.p2),
            (3, &mut segment.p3),
            (4, &mut segment.p4),
        ] {
            let Some(text) = text.as_mut() else {
                continue;
            };
            // The keys this tier already states an outcome for.
            let stated: BTreeSet<String> = text
                .lines()
                .filter_map(status_line)
                .filter(|(status, _)| *status == Status::Settled)
                .map(|(_, key)| key.to_owned())
                .collect();
            let mut replaced: BTreeSet<String> = BTreeSet::new();
            let mut fate = |line: &str, rest: &[&str]| {
                let parsed = status_line(line);
                let key = parsed.map(|(_, key)| key);
                let aged = tier >= 3 && unbulleted(line).starts_with(WAITING_PREFIX);
                if !aged && !matches!(parsed, Some((Status::Open, _))) {
                    return Fate::Keep;
                }
                if key.is_some_and(|key| rest.iter().any(|line| settles(line, key))) {
                    return Fate::Drop;
                }
                if tier != 1
                    && let Some(key) = key
                    && let Some(settling) = outcome.get(key)
                {
                    if stated.contains(key) || !replaced.insert(key.to_owned()) {
                        return Fate::Drop;
                    }
                    return Fate::Replace(settling.clone());
                }
                if aged || key.is_some_and(&settled_later) {
                    return Fate::Drop;
                }
                Fate::Keep
            };
            let retired = retire_in(text, tier == 1, &mut fate);
            if tier == 1 {
                retired_from_p1.extend(
                    retired
                        .iter()
                        .filter_map(|line| status_line(line).map(|(_, key)| key.to_owned())),
                );
            }
        }
        if let Some(p1) = &segment.p1 {
            segment.content.clone_from(p1);
        }
    }
    let final_p1: Vec<&str> = segments.iter().filter_map(|s| s.p1.as_deref()).collect();
    output.claims.retain(|candidate| {
        !retired_from_p1.contains(&candidate.key)
            || candidate.anchor.as_deref().is_some_and(|anchor| {
                !anchor.is_empty() && final_p1.iter().any(|p1| p1.contains(anchor))
            })
    });
    &output.history_segments
}

/// Applies `fate`, given each line and the lines after it, to the lines of `text` and returns the
/// lines it dropped or replaced. With `keep_nonempty`, a retirement that would leave no other text
/// leaves `text` unchanged and returns nothing. A blank line that would follow a blank line or open
/// the text after a removal goes with it, and is not returned.
fn retire_in(
    text: &mut String,
    keep_nonempty: bool,
    fate: &mut impl FnMut(&str, &[&str]) -> Fate,
) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut kept: Vec<String> = Vec::new();
    let mut retired = Vec::new();
    for (at, line) in lines.iter().enumerate() {
        match fate(line, &lines[at + 1..]) {
            Fate::Keep => {}
            Fate::Drop => {
                retired.push((*line).to_owned());
                continue;
            }
            Fate::Replace(settling) => {
                retired.push((*line).to_owned());
                kept.push(settling);
                continue;
            }
        }
        let blank = line.trim().is_empty();
        if blank && !retired.is_empty() && kept.last().is_none_or(|last| last.trim().is_empty()) {
            continue;
        }
        kept.push((*line).to_owned());
    }
    let rest = kept.join("\n").trim().to_owned();
    if retired.is_empty() || (keep_nonempty && rest.is_empty()) {
        return Vec::new();
    }
    *text = rest;
    retired
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history_summarizer_citations::ClaimCandidate;

    fn segment(p1: &str, p2: &str, p3: &str, p4: &str) -> ParsedHistorySegment {
        ParsedHistorySegment {
            start_message: 1,
            end_message: 4,
            title: "t".into(),
            content: p1.into(),
            p1: Some(p1.into()),
            p2: Some(p2.into()),
            p3: Some(p3.into()),
            p4: Some(p4.into()),
            importance: Some(50),
            episode_type: None,
        }
    }

    fn output(segments: Vec<ParsedHistorySegment>) -> ParsedHistorySegmentOutput {
        ParsedHistorySegmentOutput {
            history_segments: segments,
            facts: Vec::new(),
            facts_block_present: false,
            fact_syntax_failure: None,
            events: Vec::new(),
            unprocessed_from: None,
            user_observations: Vec::new(),
            primer_candidates: Vec::new(),
            claims: Vec::new(),
            claims_block_present: false,
            claims_syntax_failure: None,
        }
    }

    fn retired(segments: Vec<ParsedHistorySegment>) -> Vec<[String; 4]> {
        let mut output = output(segments);
        retire_settled_lines(&mut output);
        output
            .history_segments
            .iter()
            .map(|s| [&s.p1, &s.p2, &s.p3, &s.p4].map(|t| t.clone().unwrap()))
            .collect()
    }

    #[test]
    fn a_status_line_needs_its_prefix_a_claim_key_and_the_closing_colon() {
        for (line, expected) in [
            (
                "Waiting on user (release.order): Ship the CLI first?",
                Status::Open,
            ),
            (
                "- Waiting on user (release.order): Ship the CLI first?",
                Status::Open,
            ),
            ("Constraint (release.order): CLI ships last.", Status::Open),
            ("  Decided (release.order): CLI first.", Status::Settled),
            (
                "* Lifted (release.order): the order rule is lifted.",
                Status::Settled,
            ),
        ] {
            assert_eq!(
                status_line(line),
                Some((expected, "release.order")),
                "{line}"
            );
        }
        for line in [
            "Waiting on user (Release Order): ship?",
            "Waiting on user (release): ship?",
            "Waiting on user (release.order) ship?",
            "I am waiting on user (release.order): ship?",
            "Decided release.order: CLI first.",
            "**Decided (release.order)**: CLI first.",
            "1. Decided (release.order): CLI first.",
        ] {
            assert_eq!(status_line(line), None, "{line}");
        }
    }

    #[test]
    fn the_prompt_examples_parse_as_the_status_lines_the_pass_reads() {
        let prompt = crate::history_summarizer_prompt::HISTORY_SUMMARIZER_SYSTEM_PROMPT;
        let section = prompt
            .split("## Open items: waiting, constraints, decided, lifted")
            .nth(1)
            .and_then(|rest| rest.split("\n## ").next())
            .expect("the prompt documents open items");
        let examples: Vec<&str> = section
            .split("```")
            .nth(1)
            .expect("the section shows the four forms")
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect();
        let statuses: Vec<Status> = examples
            .iter()
            .map(|example| status_line(example).expect(example).0)
            .collect();
        assert_eq!(
            statuses,
            [Status::Open, Status::Open, Status::Settled, Status::Settled]
        );
        for example in &examples {
            assert!(
                example.len() <= crate::history_summarizer_citations::CLAIM_ANCHOR_MAX_BYTES,
                "a status line is its own claim anchor: {example}"
            );
        }
        for rule in [
            "removes a `Waiting on user` or `Constraint` line when a `Decided` or `Lifted` line with its key follows it in the same tier",
            "line that begins `Waiting on user (` leaves `<p3>` and `<p4>`",
            "a `<p1>` that would hold nothing else keeps its lines",
        ] {
            assert!(section.contains(rule), "{rule}");
        }
    }

    #[test]
    fn retirement_is_per_key_and_keeps_the_paragraphs_single_spaced() {
        let p1 = "First paragraph.\n\nWaiting on user (a.b): ask?\n\nWaiting on user (c.d): other?\nDecided (a.b): yes.";
        let kept = "First paragraph.\n\nWaiting on user (c.d): other?\nDecided (a.b): yes.";
        assert_eq!(
            retired(vec![segment(p1, p1, "Done.", "")]),
            [[kept, kept, "Done.", ""].map(String::from)]
        );
    }

    #[test]
    fn a_question_asked_again_after_its_answer_stays_open() {
        let p1 = "Waiting on user (a.b): first ask?\nDecided (a.b): yes.\nWaiting on user (a.b): ask again?";
        let [p1, p2, ..] = retired(vec![segment(
            p1,
            "Waiting on user (a.b): ask again?",
            "Done.",
            "",
        )])
        .remove(0);
        assert_eq!(p1, "Decided (a.b): yes.\nWaiting on user (a.b): ask again?");
        assert_eq!(p2, "Waiting on user (a.b): ask again?");
    }

    #[test]
    fn a_tier_without_the_outcome_takes_p1s_settling_line_in_place_of_its_open_line() {
        let p1 = "Worked.\nWaiting on user (a.b): ask?\nDecided (a.b): yes.\nConstraint (c.d): no fixes.\nLifted (c.d): fixes allowed.";
        let [p1, p2, p3, p4] = retired(vec![segment(
            p1,
            "Short.\nWaiting on user (a.b): ask?\nConstraint (c.d): no fixes.",
            "Old.\n- Waiting on user (a.b): ask?\nDecided (a.b): yes.",
            "Constraint (c.d): no fixes.",
        )])
        .remove(0);
        assert_eq!(
            p1,
            "Worked.\nDecided (a.b): yes.\nLifted (c.d): fixes allowed."
        );
        assert_eq!(
            p2,
            "Short.\nDecided (a.b): yes.\nLifted (c.d): fixes allowed."
        );
        assert_eq!(p3, "Old.\nDecided (a.b): yes.");
        assert_eq!(p4, "Lifted (c.d): fixes allowed.");
    }

    #[test]
    fn a_settling_line_only_in_p2_settles_p2_and_leaves_p1_as_written() {
        let p1 = "Worked.\nWaiting on user (a.b): ask?";
        let [p1_after, p2, ..] = retired(vec![segment(
            p1,
            "Waiting on user (a.b): ask?\nDecided (a.b): yes.",
            "Done.",
            "",
        )])
        .remove(0);
        assert_eq!(p1_after, p1);
        assert_eq!(p2, "Decided (a.b): yes.");
    }

    #[test]
    fn a_settling_line_only_in_p3_retires_nothing_from_the_recent_tiers() {
        let waiting = "Worked.\nWaiting on user (a.b): ask?";
        assert_eq!(
            retired(vec![segment(waiting, waiting, "Decided (a.b): yes.", "")]),
            [[waiting, waiting, "Decided (a.b): yes.", ""].map(String::from)]
        );
    }

    #[test]
    fn a_settlement_in_a_later_segment_retires_earlier_open_lines_only() {
        let open = "Drafted.\nWaiting on user (a.b): ask?\nConstraint (c.d): no fixes.";
        let rows = retired(vec![
            segment(open, open, "Drafted.", ""),
            segment(
                "Decided (a.b): yes.",
                "Lifted (c.d): fixes allowed.",
                "Done.",
                "",
            ),
            segment(open, open, "Drafted.", ""),
        ]);
        assert_eq!(rows[0][..2], ["Drafted.", "Drafted."]);
        // Lines written after the settlement stay open.
        assert_eq!(rows[2][..2], [open, open]);
    }

    #[test]
    fn aged_tiers_lose_every_waiting_line_and_keep_unsettled_constraints() {
        let p3 =
            "Moved the plugin.\n- Waiting on user (bad key): publish?\nConstraint (c.d): no fixes.";
        let [.., p3, p4] = retired(vec![segment(
            "Moved.",
            "Moved.",
            p3,
            "Waiting on user (a.b): publish?",
        )])
        .remove(0);
        assert_eq!(p3, "Moved the plugin.\nConstraint (c.d): no fixes.");
        assert_eq!(p4, "");
    }

    #[test]
    fn p1_keeps_lines_it_would_otherwise_lose_entirely() {
        let only = "Waiting on user (a.b): ask?";
        let rows = retired(vec![
            segment(only, only, only, ""),
            segment("Decided (a.b): yes.", "Decided (a.b): yes.", "Done.", ""),
        ]);
        assert_eq!(rows[0], [only, "", "", ""].map(String::from));
    }

    #[test]
    fn a_retired_keys_claims_survive_only_when_anchored_in_a_final_p1() {
        let candidate = |key: &str, anchor: Option<&str>| ClaimCandidate {
            key: key.into(),
            cite: "[s1:0-1]".into(),
            value: Some("v".into()),
            anchor: anchor.map(Into::into),
        };
        let p1 = "Worked on it now.\nWaiting on user (a.b): ask now?\nDecided (a.b): yes now.";
        let mut output = output(vec![segment(p1, p1, "Done.", "")]);
        output.claims = vec![
            candidate("a.b", Some("Decided (a.b): yes now.")),
            candidate("a.b", Some("Waiting on user (a.b): ask now?")),
            candidate("a.b", None),
            candidate("a.b", Some("now")),
            candidate("c.d", Some("Waiting on user (a.b): ask now?")),
            candidate("c.d", None),
        ];
        retire_settled_lines(&mut output);
        let kept: Vec<_> = output
            .claims
            .iter()
            .map(|c| (c.key.as_str(), c.anchor.as_deref()))
            .collect();
        assert_eq!(
            kept,
            [
                ("a.b", Some("Decided (a.b): yes now.")),
                ("a.b", Some("now")),
                ("c.d", Some("Waiting on user (a.b): ask now?")),
                ("c.d", None),
            ]
        );
    }
}
