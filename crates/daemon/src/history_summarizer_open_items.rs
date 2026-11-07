//! Open items in history_summarizer tiers: the status lines a fold carries for questions that wait
//! on the user and for constraints the user set, and the pass that retires the ones a later line
//! of the same output settles.
//!
//! A status line is one tier line, optionally bulleted with `- ` or `* `, in one of four forms, each
//! naming its item by a claim key:
//!
//! - `Waiting on user (key): …`: a question I put to the user that the folded span leaves open.
//! - `Constraint (key): …`: a standing limit the user set for the work.
//! - `Decided (key): …`: the user's answer to an item, from this span or an earlier fold.
//! - `Lifted (key): …`: a user constraint the user later lifted, stated as lifted.
//!
//! Waiting and constraint lines are open; decided and lifted lines settle the item with their key.
//! The summarizer resolves each item against the user turns of the folded span. This pass
//! enforces the part of that resolution the output's own structure shows, for every status line
//! the model writes:
//!
//! - An open line is retired when a settling line with its key follows it in the same tier, when
//!   the segment's `p1` ends that key's status lines with a settling line, or when a settling line
//!   with its key appears in the `p1` or `p2` of a later segment of the same output. A question
//!   the user answered or a constraint the user lifted inside this span keeps only the outcome,
//!   and a question asked again after its answer stays open.
//! - Every line that begins `Waiting on user (` is retired from `p3` and `p4`, whatever its key.
//!   Those tiers render once a segment has aged past its recent tiers, and they carry outcomes.
//!
//! The pass runs on parsed segments, before claim validation. A claim candidate whose key matches
//! a line retired from `p1` and whose anchor lies inside that line leaves with the line, so claim
//! validation keeps the item's settling claim. Claim validation then checks every remaining anchor
//! against the final `p1`.
//!
//! The prompt has each status line carry a claim with its key, citing the asking part for a
//! waiting line, the user's message for a constraint, and the user's answer for a settling line.
//! When an earlier fold's waiting or constraint claim and a later fold's settling claim both pass
//! claim validation, the later claim is current and the render-time correction pass marks the
//! earlier one: it splices the correction over the earlier line when that claim's anchor occurs in
//! the rendered tier, and lists it on the segment's `[corrections: …]` line otherwise. An open line
//! whose claims fail validation stays as written until its segment ages past `p2`.
//!
//! A retirement never leaves `p1` empty; a `p1` that would hold nothing else keeps its lines. The
//! pass removes lines and claim candidates and never adds any, so the stored summary is bounded by
//! what the model emitted.

use std::collections::BTreeSet;

use super::ParsedHistorySegment;
use crate::history_summarizer_citations::{ClaimCandidate, is_claim_key};

/// The prefix of a status line for a question that waits on the user.
pub(crate) const WAITING_PREFIX: &str = "Waiting on user (";
/// The prefix of a status line for a standing user constraint.
pub(crate) const CONSTRAINT_PREFIX: &str = "Constraint (";
/// The prefixes of status lines that settle an item.
pub(crate) const SETTLING_PREFIXES: [&str; 2] = ["Decided (", "Lifted ("];

/// Whether a status line leaves its item open or settles it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Open,
    Settled,
}

/// `line` without leading whitespace and one leading `- ` or `* ` bullet.
fn unbulleted(line: &str) -> &str {
    let line = line.trim_start();
    line.strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .unwrap_or(line)
        .trim_start()
}

/// The status and key of `line` when it is a status line whose key follows the claim-key grammar.
fn status_line(line: &str) -> Option<(Status, &str)> {
    let line = unbulleted(line);
    let open = [WAITING_PREFIX, CONSTRAINT_PREFIX]
        .iter()
        .find_map(|prefix| line.strip_prefix(prefix))
        .map(|rest| (Status::Open, rest));
    let (status, rest) = open.or_else(|| {
        SETTLING_PREFIXES
            .iter()
            .find_map(|prefix| line.strip_prefix(prefix))
            .map(|rest| (Status::Settled, rest))
    })?;
    let (key, _) = rest.split_once("):")?;
    is_claim_key(key).then_some((status, key))
}

/// One line the pass removed, with the key its status line named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RetiredLine {
    pub key: Option<String>,
    pub line: String,
    /// The tier the line left, 1 through 4.
    pub tier: u8,
}

/// Whether `candidate` is the claim of a line retired from `p1`: its key is that line's key and
/// its anchor lies inside that line.
pub(crate) fn claims_retired_line(candidate: &ClaimCandidate, retired: &[RetiredLine]) -> bool {
    let Some(anchor) = candidate.anchor.as_deref().filter(|a| !a.is_empty()) else {
        return false;
    };
    retired.iter().any(|retired| {
        retired.tier == 1
            && retired.key.as_deref() == Some(candidate.key.as_str())
            && retired.line.contains(anchor)
    })
}

/// Retires the open lines the output settles, per the module rules, in place, and returns them.
pub(crate) fn retire_settled_lines(segments: &mut [ParsedHistorySegment]) -> Vec<RetiredLine> {
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
    let mut retired = Vec::new();
    for (index, segment) in segments.iter_mut().enumerate() {
        let Some(p1) = segment.p1.as_deref() else {
            continue;
        };
        let settled_later = |key: &str| {
            settled_in[index + 1..]
                .iter()
                .any(|keys| keys.contains(key))
        };
        // The keys whose last status line in `p1` settles them.
        let mut last_in_p1: Vec<(&str, Status)> = Vec::new();
        for (status, key) in p1.lines().filter_map(status_line) {
            last_in_p1.retain(|(k, _)| *k != key);
            last_in_p1.push((key, status));
        }
        let settled_by_p1: BTreeSet<String> = last_in_p1
            .into_iter()
            .filter(|(_, status)| *status == Status::Settled)
            .map(|(key, _)| key.to_owned())
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
            let aged = tier >= 3;
            let lines = retire_in(text, tier == 1, |line, rest| {
                if aged && unbulleted(line).starts_with(WAITING_PREFIX) {
                    return true;
                }
                match status_line(line) {
                    Some((Status::Open, key)) => {
                        settled_later(key)
                            || (tier != 1 && settled_by_p1.contains(key))
                            || rest
                                .iter()
                                .filter_map(|line| status_line(line))
                                .any(|(status, k)| status == Status::Settled && k == key)
                    }
                    _ => false,
                }
            });
            retired.extend(lines.into_iter().map(|line| RetiredLine {
                key: status_line(&line).map(|(_, key)| key.to_owned()),
                line,
                tier,
            }));
        }
        if let Some(p1) = &segment.p1 {
            segment.content.clone_from(p1);
        }
    }
    retired
}

/// Removes from `text` each line `retire` accepts, given the line and the lines after it, and
/// returns the removed lines. With `keep_nonempty`, a removal that would leave no other text
/// leaves `text` unchanged and returns nothing. A blank line that would follow a blank line or
/// open the text after a removal goes with it.
fn retire_in(
    text: &mut String,
    keep_nonempty: bool,
    retire: impl Fn(&str, &[&str]) -> bool,
) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut kept: Vec<&str> = Vec::new();
    let mut dropped = Vec::new();
    for (at, line) in lines.iter().enumerate() {
        if retire(line, &lines[at + 1..]) {
            dropped.push((*line).to_owned());
            continue;
        }
        let blank = line.trim().is_empty();
        if blank && !dropped.is_empty() && kept.last().is_none_or(|last| last.trim().is_empty()) {
            continue;
        }
        kept.push(line);
    }
    let rest = kept.join("\n").trim().to_owned();
    if dropped.is_empty() || (keep_nonempty && rest.is_empty()) {
        return Vec::new();
    }
    *text = rest;
    dropped
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn tiers(segment: &ParsedHistorySegment) -> [&str; 4] {
        [&segment.p1, &segment.p2, &segment.p3, &segment.p4].map(|t| t.as_deref().unwrap())
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
            "removes a `Waiting on user` or `Constraint` line when a `Decided` or `Lifted` line with its key follows it",
            "every line that begins `Waiting on user (` from `<p3>` and `<p4>`",
        ] {
            assert!(section.contains(rule), "{rule}");
        }
    }

    #[test]
    fn retirement_is_per_key_and_keeps_the_paragraphs_single_spaced() {
        let p1 = "First paragraph.\n\nWaiting on user (a.b): ask?\n\nWaiting on user (c.d): other?\nDecided (a.b): yes.";
        let mut segments = [segment(p1, p1, "Done.", "")];
        let retired = retire_settled_lines(&mut segments);
        let kept = "First paragraph.\n\nWaiting on user (c.d): other?\nDecided (a.b): yes.";
        assert_eq!(tiers(&segments[0]), [kept, kept, "Done.", ""]);
        assert_eq!(segments[0].content, kept);
        assert_eq!(
            retired
                .iter()
                .map(|r| (r.key.as_deref(), r.tier))
                .collect::<Vec<_>>(),
            [(Some("a.b"), 1), (Some("a.b"), 2)]
        );
    }

    #[test]
    fn a_question_asked_again_after_its_answer_stays_open() {
        let p1 = "Waiting on user (a.b): first ask?\nDecided (a.b): yes.\nWaiting on user (a.b): ask again?";
        let mut segments = [segment(
            p1,
            "Waiting on user (a.b): ask again?",
            "Done.",
            "",
        )];
        retire_settled_lines(&mut segments);
        assert_eq!(
            tiers(&segments[0])[..2],
            [
                "Decided (a.b): yes.\nWaiting on user (a.b): ask again?",
                "Waiting on user (a.b): ask again?"
            ]
        );
    }

    #[test]
    fn a_p1_that_settles_a_key_retires_its_open_line_from_p2() {
        let p1 = "Worked.\nWaiting on user (a.b): ask?\nDecided (a.b): yes.";
        let mut segments = [segment(
            p1,
            "Short.\nWaiting on user (a.b): ask?",
            "Done.",
            "",
        )];
        retire_settled_lines(&mut segments);
        assert_eq!(
            tiers(&segments[0])[..2],
            ["Worked.\nDecided (a.b): yes.", "Short."]
        );
    }

    #[test]
    fn a_settling_line_only_in_p3_retires_nothing_from_the_recent_tiers() {
        let waiting = "Worked.\nWaiting on user (a.b): ask?";
        let mut segments = [segment(waiting, waiting, "Decided (a.b): yes.", "")];
        retire_settled_lines(&mut segments);
        assert_eq!(
            tiers(&segments[0]),
            [waiting, waiting, "Decided (a.b): yes.", ""]
        );
    }

    #[test]
    fn a_settlement_in_a_later_segment_retires_earlier_open_lines_only() {
        let waiting = "Drafted.\nWaiting on user (a.b): ask?\nConstraint (c.d): no fixes.";
        let mut segments = [
            segment(waiting, waiting, "Drafted.", ""),
            segment(
                "Decided (a.b): yes.\nLifted (c.d): fixes allowed.",
                "Done.",
                "Done.",
                "",
            ),
            segment(waiting, waiting, "Drafted.", ""),
        ];
        retire_settled_lines(&mut segments);
        assert_eq!(tiers(&segments[0])[..2], ["Drafted.", "Drafted."]);
        // Lines written after the settlement stay open.
        assert_eq!(tiers(&segments[2])[..2], [waiting, waiting]);
    }

    #[test]
    fn aged_tiers_lose_every_waiting_line_and_keep_unsettled_constraints() {
        let p3 =
            "Moved the plugin.\n- Waiting on user (bad key): publish?\nConstraint (c.d): no fixes.";
        let mut segments = [segment(
            "Moved.",
            "Moved.",
            p3,
            "Waiting on user (a.b): publish?",
        )];
        retire_settled_lines(&mut segments);
        assert_eq!(
            tiers(&segments[0])[2..],
            ["Moved the plugin.\nConstraint (c.d): no fixes.", ""]
        );
    }

    #[test]
    fn p1_keeps_lines_it_would_otherwise_lose_entirely() {
        let only = "Waiting on user (a.b): ask?";
        let mut segments = [
            segment(only, only, only, ""),
            segment("Decided (a.b): yes.", "Decided (a.b): yes.", "Done.", ""),
        ];
        let retired = retire_settled_lines(&mut segments);
        assert_eq!(tiers(&segments[0]), [only, "", "", ""]);
        assert!(retired.iter().all(|r| r.tier != 1));
    }

    #[test]
    fn only_the_retired_lines_own_claim_leaves_with_it() {
        let claim = |key: &str, anchor: &str| ClaimCandidate {
            key: key.into(),
            cite: "[s1:0-1]".into(),
            value: Some("v".into()),
            anchor: Some(anchor.into()),
        };
        let retired = [RetiredLine {
            key: Some("a.b".into()),
            line: "Waiting on user (a.b): ask now?".into(),
            tier: 1,
        }];
        assert!(claims_retired_line(&claim("a.b", "ask now?"), &retired));
        assert!(!claims_retired_line(&claim("c.d", "ask now?"), &retired));
        assert!(!claims_retired_line(
            &claim("a.b", "Decided (a.b): yes."),
            &retired
        ));
        let from_p2 = [RetiredLine {
            tier: 2,
            ..retired[0].clone()
        }];
        assert!(!claims_retired_line(&claim("a.b", "ask now?"), &from_p2));
    }
}
