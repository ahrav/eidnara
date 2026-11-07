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
//! - An open line followed in its own tier by a settling line with its key leaves that tier.
//! - Otherwise an open line gives way to its key's outcome when one exists: the settling line that
//!   ends the key's status lines in the segment's own `p1`, or else the settling line of the last
//!   later segment of the output that settles the key in its `p1` or `p2`. A tier that already
//!   states an outcome for the key just loses the open line, and a tier gets the outcome once.
//!   An earlier segment that keeps the outcome keeps the referent of a short answer, such as a
//!   bare "yes", even when the segment holding the answer is later discarded and refolded.
//! - A question asked again after its answer, with no settling line after it, stays open.
//! - Every other line that begins `Waiting on user (` leaves `p3` and `p4`, whatever its key.
//!   Those tiers render once a segment has aged past its recent tiers, and they carry outcomes.
//!
//! Claims follow the same outcome. For a key that some segment settles, the claim candidates whose
//! cited message lies in an earlier segment leave, and in the last settling segment the candidates
//! are ordered by cited message, so claim validation keeps the claim on the latest message, which
//! is the user's answer, whatever order the model emitted them in.
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

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::history_summarizer_citations::{FrozenAliasTable, is_claim_key, split_citations};
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
/// rules, keeps each segment's `content` equal to its `p1`, orders and removes the claim candidates
/// of settled keys, and returns the segments.
pub(crate) fn retire_settled_lines<'o>(
    output: &'o mut ParsedHistorySegmentOutput,
    aliases: &FrozenAliasTable,
) -> &'o [ParsedHistorySegment] {
    // Each settled key's last settling segment and that segment's settling line.
    let mut settled: BTreeMap<String, (usize, String)> = BTreeMap::new();
    for (index, segment) in output.history_segments.iter().enumerate() {
        for tier in [segment.p2.as_deref(), segment.p1.as_deref()]
            .into_iter()
            .flatten()
        {
            for line in tier.lines() {
                if let Some((Status::Settled, key)) = status_line(line) {
                    settled.insert(key.to_owned(), (index, unbulleted(line).to_owned()));
                }
            }
        }
    }
    for (index, segment) in output.history_segments.iter_mut().enumerate() {
        let Some(p1) = segment.p1.as_deref() else {
            continue;
        };
        // Each key's outcome: the settling line that ends its status lines in this `p1`, or else
        // the settling line of the last later segment that settles it.
        let mut last_in_p1: BTreeMap<&str, (Status, &str)> = BTreeMap::new();
        for line in p1.lines() {
            if let Some((status, key)) = status_line(line) {
                last_in_p1.insert(key, (status, unbulleted(line)));
            }
        }
        let mut outcome: BTreeMap<String, String> = settled
            .iter()
            .filter(|(_, (at, _))| *at > index)
            .map(|(key, (_, line))| (key.clone(), line.clone()))
            .collect();
        for (key, (status, line)) in last_in_p1 {
            if status == Status::Settled {
                outcome.insert(key.to_owned(), line.to_owned());
            }
        }
        for (tier, text) in [
            (1, &mut segment.p1),
            (2, &mut segment.p2),
            (3, &mut segment.p3),
            (4, &mut segment.p4),
        ] {
            let Some(text) = text.as_mut() else {
                continue;
            };
            // The keys whose outcome this tier states, including outcomes it receives below.
            let mut stated: BTreeSet<String> = text
                .lines()
                .filter_map(status_line)
                .filter(|(status, _)| *status == Status::Settled)
                .map(|(_, key)| key.to_owned())
                .collect();
            let mut fate = |line: &str, rest: &[&str]| {
                let parsed = status_line(line);
                let aged = tier >= 3 && unbulleted(line).starts_with(WAITING_PREFIX);
                if !aged && !matches!(parsed, Some((Status::Open, _))) {
                    return Fate::Keep;
                }
                let key = parsed.map(|(_, key)| key);
                if key.is_some_and(|key| rest.iter().any(|line| settles(line, key))) {
                    return Fate::Drop;
                }
                if let Some(key) = key
                    && let Some(settling) = outcome.get(key)
                {
                    return if stated.insert(key.to_owned()) {
                        Fate::Replace(settling.clone())
                    } else {
                        Fate::Drop
                    };
                }
                if aged { Fate::Drop } else { Fate::Keep }
            };
            retire_in(text, &mut fate);
        }
        if let Some(p1) = &segment.p1 {
            segment.content.clone_from(p1);
        }
    }
    order_settled_claims(output, aliases, &settled);
    &output.history_segments
}

/// For each settled key, removes the claim candidates that cite a message in a segment before its
/// last settling segment and orders the candidates that cite a message in that segment by cited
/// message, keeping every other candidate where it stands.
fn order_settled_claims(
    output: &mut ParsedHistorySegmentOutput,
    aliases: &FrozenAliasTable,
    settled: &BTreeMap<String, (usize, String)>,
) {
    let segments = &output.history_segments;
    let cited = |cite: &str| -> Option<(usize, u64)> {
        let (citations, _) = split_citations(cite).ok()?;
        let ordinal = aliases.resolve(&citations.first()?.alias)?.ordinal;
        let segment = segments
            .iter()
            .position(|s| (s.start_message..=s.end_message).contains(&ordinal))?;
        Some((segment, ordinal))
    };
    let placed: Vec<Option<(usize, u64)>> = output.claims.iter().map(|c| cited(&c.cite)).collect();
    // Per settled key, the positions of its candidates in the last settling segment, by message.
    let mut queues: BTreeMap<String, VecDeque<usize>> = BTreeMap::new();
    let mut keep = vec![true; output.claims.len()];
    for (at, candidate) in output.claims.iter().enumerate() {
        let (Some((last, _)), Some((segment, _))) = (settled.get(&candidate.key), placed[at])
        else {
            continue;
        };
        if segment < *last {
            keep[at] = false;
        } else if segment == *last {
            queues
                .entry(candidate.key.clone())
                .or_default()
                .push_back(at);
        }
    }
    for queue in queues.values_mut() {
        queue
            .make_contiguous()
            .sort_by_key(|&at| placed[at].map(|(_, ordinal)| ordinal));
    }
    // Each queued position takes the next candidate of its key in message order.
    let order: Vec<usize> = (0..output.claims.len())
        .filter(|&at| keep[at])
        .map(|at| {
            let queued = placed[at].is_some_and(|(segment, _)| {
                settled
                    .get(&output.claims[at].key)
                    .is_some_and(|(last, _)| segment == *last)
            });
            if queued {
                queues
                    .get_mut(&output.claims[at].key)
                    .and_then(VecDeque::pop_front)
                    .unwrap_or(at)
            } else {
                at
            }
        })
        .collect();
    let mut claims: Vec<_> = std::mem::take(&mut output.claims)
        .into_iter()
        .map(Some)
        .collect();
    output.claims = order
        .into_iter()
        .filter_map(|at| claims[at].take())
        .collect();
}

/// Applies `fate`, given each line and the lines after it, to the lines of `text`. A blank line
/// that would follow a blank line or open the text after a removal goes with it.
fn retire_in(text: &mut String, fate: &mut impl FnMut(&str, &[&str]) -> Fate) {
    let lines: Vec<&str> = text.lines().collect();
    let mut kept: Vec<String> = Vec::new();
    let mut retired = false;
    for (at, line) in lines.iter().enumerate() {
        match fate(line, &lines[at + 1..]) {
            Fate::Keep => {}
            Fate::Drop => {
                retired = true;
                continue;
            }
            Fate::Replace(settling) => {
                retired = true;
                kept.push(settling);
                continue;
            }
        }
        let blank = line.trim().is_empty();
        if blank && retired && kept.last().is_none_or(|last| last.trim().is_empty()) {
            continue;
        }
        kept.push((*line).to_owned());
    }
    if retired {
        *text = kept.join("\n").trim().to_owned();
    }
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
        retire_settled_lines(&mut output, &FrozenAliasTable::default());
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
    fn a_settlement_in_a_later_segment_gives_earlier_open_lines_its_outcome() {
        let open = "Drafted.\nWaiting on user (a.b): ask?\nConstraint (c.d): no fixes.";
        let rows = retired(vec![
            segment(open, open, "Drafted.", "Waiting on user (a.b): ask?"),
            segment(
                "Decided (a.b): yes.",
                "Lifted (c.d): fixes allowed.",
                "Done.",
                "",
            ),
            segment(open, open, "Drafted.", ""),
        ]);
        let settled = "Drafted.\nDecided (a.b): yes.\nLifted (c.d): fixes allowed.";
        assert_eq!(
            rows[0],
            [settled, settled, "Drafted.", "Decided (a.b): yes."].map(String::from)
        );
        // Lines written after the settlement stay open.
        assert_eq!(rows[2][..2], [open, open]);
    }

    #[test]
    fn a_tier_gets_an_outcome_once_and_keeps_one_it_already_states() {
        let p1 = "Waiting on user (a.b): ask?\nDecided (a.b): yes.";
        let [_, p2, p3, _] = retired(vec![segment(
            p1,
            "Waiting on user (a.b): ask?\nMore.\nWaiting on user (a.b): ask again?",
            "Decided (a.b): yes.\nWaiting on user (a.b): ask?",
            "",
        )])
        .remove(0);
        assert_eq!(p2, "Decided (a.b): yes.\nMore.");
        assert_eq!(p3, "Decided (a.b): yes.");
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
    fn a_settled_keys_claims_come_from_its_last_settling_segment_in_message_order() {
        let mut aliases = FrozenAliasTable::default();
        for ordinal in 1..=6 {
            aliases.issue(crate::history_summarizer_citations::FrozenAlias {
                message_id: format!("m{ordinal}"),
                ordinal,
                ..Default::default()
            });
        }
        let candidate = |key: &str, alias: &str| ClaimCandidate {
            key: key.into(),
            cite: format!("[{alias}:0-1]"),
            value: Some(alias.into()),
            anchor: None,
        };
        let mut first = segment("Waiting on user (a.b): ask?", "Asked.", "Asked.", "");
        first.end_message = 2;
        let mut second = segment(
            "Waiting on user (c.d): other?\nDecided (c.d): no.\nDecided (a.b): yes.",
            "Done.",
            "Done.",
            "",
        );
        (second.start_message, second.end_message) = (3, 6);
        let mut output = output(vec![first, second]);
        output.claims = vec![
            candidate("a.b", "s5"),
            candidate("c.d", "s6"),
            candidate("a.b", "s2"),
            candidate("c.d", "s3"),
            candidate("e.f", "s1"),
            candidate("a.b", "s9"),
        ];
        retire_settled_lines(&mut output, &aliases);
        let kept: Vec<_> = output
            .claims
            .iter()
            .map(|c| (c.key.as_str(), c.value.as_deref().unwrap()))
            .collect();
        // The first segment's claim on the settled key leaves; the second segment's claims on
        // each settled key run in message order; an unsettled key and an unresolved cite stay.
        assert_eq!(
            kept,
            [
                ("a.b", "s5"),
                ("c.d", "s3"),
                ("c.d", "s6"),
                ("e.f", "s1"),
                ("a.b", "s9")
            ]
        );
    }
}
