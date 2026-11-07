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
//! checked, and enforces the part of that resolution a segment's own tiers show. It reads and
//! changes one segment at a time, because validation stores, discards, and later reverts each
//! segment on its own, so a segment's stored text depends only on its own span:
//!
//! - An open line followed in its own tier by a settling line with its key leaves that tier.
//! - When the segment's `p1` ends a key's status lines with a settling line, or `p1` has no
//!   settling line for the key and `p2` ends it with one, an open line with that key in any other
//!   tier gives way to that settling line. A tier that already states
//!   an outcome for the key just loses the open line, and a tier gets the outcome once, so a
//!   segment grows by at most one copy of a settling line in each of its other three tiers.
//! - A question asked again in `p1` after its answer stays open, because `p1` decides the outcome.
//! - Every other line that begins `Waiting on user (` leaves `p3` and `p4`, whatever its key.
//!   Those tiers render once a segment has aged past its recent tiers, and they carry outcomes.
//!
//! In a segment whose `p1` or `p2` settles a key, the key's claim candidates whose one citation
//! names a message in the segment survive only when their anchor lies inside one of the segment's
//! settling lines for the key, and the survivors run in cited-message order. Claim validation,
//! which keeps the last claim per key, then keeps the settling claim on the latest message, the
//! user's answer, whatever order the model emitted them in; when none survives or passes
//! validation, the key has no claim in that segment, so a retired question or constraint never
//! becomes the key's live value.
//!
//! An item asked in one segment and settled in a later segment or a later fold is settled by its
//! claims. The prompt has each status line carry a claim with its key, citing the asking part for a
//! waiting line, the user's message for a constraint, and the user's answer for a settling line.
//! When the earlier segment's waiting or constraint claim and the later settling claim both pass
//! claim validation, the later claim is current and the render-time correction pass marks the
//! earlier line: the correction replaces the line in place when the claim's anchor occurs in the
//! rendered tier, the marker passes the durable scanner, and the hit passes the overlap rule, and it
//! is listed on the segment's `[corrections: …]` line beside the unchanged text otherwise. A waiting
//! line whose claims fail validation stays as written in `p1` and `p2`, and an unsettled constraint
//! line stays in every tier.
//!
//! The pass removes lines, removes and reorders claim candidates, and puts one of the segment's own
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

/// Retires each segment's settled open lines and the waiting lines of its `p3` and `p4` per the
/// module rules, keeps its `content` equal to its `p1`, orders the claim candidates of the keys it
/// settles, and returns the segments.
pub(crate) fn retire_settled_lines<'o>(
    output: &'o mut ParsedHistorySegmentOutput,
    aliases: &FrozenAliasTable,
) -> &'o [ParsedHistorySegment] {
    let mut settled_in: Vec<BTreeSet<String>> = Vec::new();
    for segment in &mut output.history_segments {
        settled_in.push(retire_in_segment(segment));
    }
    order_settled_claims(output, aliases, &settled_in);
    &output.history_segments
}

/// The keys `text` names in settling status lines.
fn settling_keys(text: &str) -> BTreeSet<&str> {
    text.lines()
        .filter_map(status_line)
        .filter(|(status, _)| *status == Status::Settled)
        .map(|(_, key)| key)
        .collect()
}

/// Each key's last status line in `text`, with its status.
fn last_status_lines(text: &str) -> BTreeMap<&str, (Status, &str)> {
    let mut last = BTreeMap::new();
    for line in text.lines() {
        if let Some((status, key)) = status_line(line) {
            last.insert(key, (status, unbulleted(line)));
        }
    }
    last
}

/// Retires one segment's settled open lines and aged waiting lines, and returns the keys its `p1`
/// or `p2` settles.
fn retire_in_segment(segment: &mut ParsedHistorySegment) -> BTreeSet<String> {
    let p2 = segment.p2.as_deref().unwrap_or_default();
    let mut settled: BTreeSet<String> = settling_keys(p2).into_iter().map(str::to_owned).collect();
    let Some(p1) = segment.p1.as_deref() else {
        return settled;
    };
    let p1_settles = settling_keys(p1);
    settled.extend(p1_settles.iter().map(|key| (*key).to_owned()));
    // Each key's outcome: the settling line that ends its status lines in `p1`, or in `p2` when
    // `p1` has no settling line for the key.
    let mut ends = last_status_lines(p2);
    ends.extend(
        last_status_lines(p1)
            .into_iter()
            .filter(|(key, _)| p1_settles.contains(key)),
    );
    let outcome: BTreeMap<String, String> = ends
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
        // The keys whose outcome this tier states, including outcomes it receives below.
        let mut stated: BTreeSet<String> =
            settling_keys(text).into_iter().map(str::to_owned).collect();
        retire_in(text, &mut |line: &str, rest: &[&str]| {
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
        });
    }
    if let Some(p1) = &segment.p1 {
        segment.content.clone_from(p1);
    }
    settled
}

/// In each segment that settles a key, keeps only the key's claim candidates anchored inside one
/// of the segment's settling lines for it, in cited-message order, and leaves every other
/// candidate where it stands.
fn order_settled_claims(
    output: &mut ParsedHistorySegmentOutput,
    aliases: &FrozenAliasTable,
    settled_in: &[BTreeSet<String>],
) {
    let segments = &output.history_segments;
    // The segment and ordinal of a candidate whose one citation names a message of a segment that
    // settles the candidate's key; claim validation judges every other candidate as emitted.
    let placed: Vec<Option<(usize, u64)>> = output
        .claims
        .iter()
        .map(|candidate| {
            let (citations, rest) = split_citations(&candidate.cite).ok()?;
            let ([citation], true) = (citations.as_slice(), rest.trim().is_empty()) else {
                return None;
            };
            let ordinal = aliases.resolve(&citation.alias)?.ordinal;
            let segment = segments
                .iter()
                .position(|s| (s.start_message..=s.end_message).contains(&ordinal))?;
            settled_in[segment]
                .contains(&candidate.key)
                .then_some((segment, ordinal))
        })
        .collect();
    // A placed candidate survives only when its anchor lies inside a settling line of its key in
    // the segment's final `p1` or `p2`, so the claim of a retired open line, a claim anchored on
    // narrative, and a claim with no anchor leave.
    let anchored_on_outcome = |at: usize| {
        let Some((segment, _)) = placed[at] else {
            return true;
        };
        let candidate = &output.claims[at];
        let Some(anchor) = candidate.anchor.as_deref().filter(|a| !a.trim().is_empty()) else {
            return false;
        };
        let segment = &segments[segment];
        [segment.p1.as_deref(), segment.p2.as_deref()]
            .into_iter()
            .flatten()
            .flat_map(str::lines)
            .any(|line| settles(line, &candidate.key) && line.contains(anchor))
    };
    let keep: Vec<bool> = (0..output.claims.len()).map(anchored_on_outcome).collect();
    let mut queues: BTreeMap<(usize, &str), VecDeque<usize>> = BTreeMap::new();
    for (at, candidate) in output.claims.iter().enumerate() {
        if let Some((segment, _)) = placed[at].filter(|_| keep[at]) {
            queues
                .entry((segment, candidate.key.as_str()))
                .or_default()
                .push_back(at);
        }
    }
    for queue in queues.values_mut() {
        queue
            .make_contiguous()
            .sort_by_key(|&at| placed[at].map(|(_, ordinal)| ordinal));
    }
    // Each kept placed position takes the next candidate of its segment and key in message order;
    // a queue holds one entry per position that pops it, so every pop finds one.
    let order: Vec<usize> = (0..output.claims.len())
        .filter(|&at| keep[at])
        .map(|at| match placed[at] {
            Some((segment, _)) => queues
                .get_mut(&(segment, output.claims[at].key.as_str()))
                .and_then(VecDeque::pop_front)
                .expect("each queued position pops its own queue once"),
            None => at,
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
    fn a_settling_line_only_in_p2_also_settles_p1() {
        let [p1, p2, ..] = retired(vec![segment(
            "Worked.\nWaiting on user (a.b): ask?",
            "Waiting on user (a.b): ask?\nDecided (a.b): yes.",
            "Done.",
            "",
        )])
        .remove(0);
        assert_eq!(p1, "Worked.\nDecided (a.b): yes.");
        assert_eq!(p2, "Decided (a.b): yes.");
    }

    #[test]
    fn a_reask_in_p1_stays_open_when_p2_still_shows_the_answer() {
        let p1 = "Decided (a.b): yes.\nWaiting on user (a.b): ask again?";
        let [p1_after, ..] =
            retired(vec![segment(p1, "Decided (a.b): yes.", "Done.", "")]).remove(0);
        assert_eq!(p1_after, p1);
    }

    #[test]
    fn p1s_outcome_wins_over_p2s_when_both_settle_a_key() {
        let [_, _, p3, _] = retired(vec![segment(
            "Decided (a.b): from p1.",
            "Decided (a.b): from p2.",
            "Constraint (a.b): old rule.",
            "",
        )])
        .remove(0);
        assert_eq!(p3, "Decided (a.b): from p1.");
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
    fn a_later_segments_settlement_leaves_an_earlier_segment_as_written() {
        let open = "Drafted.\nWaiting on user (a.b): ask?\nConstraint (c.d): no fixes.";
        let rows = retired(vec![
            segment(open, open, "Drafted.", ""),
            segment(
                "Decided (a.b): yes.",
                "Lifted (c.d): fixes allowed.",
                "Done.",
                "",
            ),
        ]);
        assert_eq!(rows[0], [open, open, "Drafted.", ""].map(String::from));
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
    fn a_settling_segment_keeps_only_its_keys_claims_anchored_on_an_outcome_in_message_order() {
        let mut aliases = FrozenAliasTable::default();
        for ordinal in 1..=6 {
            aliases.issue(crate::history_summarizer_citations::FrozenAlias {
                message_id: format!("m{ordinal}"),
                ordinal,
                ..Default::default()
            });
        }
        let candidate = |key: &str, cite: &str, anchor: Option<&str>| ClaimCandidate {
            key: key.into(),
            cite: cite.into(),
            value: Some(cite.into()),
            anchor: anchor.map(Into::into),
        };
        let mut first = segment("Waiting on user (a.b): ask?", "Asked.", "Asked.", "");
        first.end_message = 2;
        let mut second = segment(
            "Waiting on user (c.d): other?\nDecided (c.d): no.\nDecided (a.b): yes.\nSome narrative.",
            "Decided (g.h): fine.",
            "Done.",
            "",
        );
        (second.start_message, second.end_message) = (3, 6);
        let mut output = output(vec![first, second]);
        output.claims = vec![
            candidate("c.d", "[s3:0-1]", Some("Waiting on user (c.d): other?")),
            candidate("g.h", "[s6:0-1]", Some("Decided (g.h): fine.")),
            candidate("g.h", "[s4:0-1]", Some("fine.")),
            candidate("g.h", "[s5:0-1]", Some("(g.h)")),
            candidate("c.d", "[s6:0-1]", Some("Decided (c.d): no.")),
            candidate("c.d", "[s5:0-1]", None),
            candidate("c.d", "[s4:0-1]", Some("Some narrative.")),
            candidate("a.b", "[s5:0-1]", Some("yes.")),
            candidate("a.b", "[s2:0-1]", Some("Waiting on user (a.b): ask?")),
            candidate("c.d", "[s4:0-1] [s5:0-1]", None),
            candidate("e.f", "[s1:0-1]", None),
        ];
        retire_settled_lines(&mut output, &aliases);
        let kept: Vec<_> = output
            .claims
            .iter()
            .map(|c| (c.key.as_str(), c.cite.as_str()))
            .collect();
        // In the second segment, the claims on the retired `c.d` question, on narrative, and with
        // no anchor leave, and each settled key's outcome-anchored claims run in message order.
        // The first segment settles nothing, so its claim stays, and so do the two-cite claim and
        // the unsettled key.
        assert_eq!(
            kept,
            [
                ("g.h", "[s4:0-1]"),
                ("g.h", "[s5:0-1]"),
                ("g.h", "[s6:0-1]"),
                ("c.d", "[s6:0-1]"),
                ("a.b", "[s5:0-1]"),
                ("a.b", "[s2:0-1]"),
                ("c.d", "[s4:0-1] [s5:0-1]"),
                ("e.f", "[s1:0-1]"),
            ]
        );
    }
}
