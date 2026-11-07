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
//!   tier gives way to that settling line. A tier that already states the key's outcome just
//!   loses the open line, and a tier receives each key's outcome once.
//! - A tier never grows: when the copied outcomes would make a tier longer than the model wrote
//!   it, its open lines leave without a copy, so a tier within the durable byte limit stays within
//!   it.
//! - A question asked again in `p1` after its answer stays open, because `p1` decides the outcome.
//! - Every other line that begins `Waiting on user (` leaves `p3` and `p4`, whatever its key.
//!   Those tiers render once a segment has aged past its recent tiers, and they carry outcomes.
//!
//! Claim candidates follow the segment whose message their one citation names. A candidate of a
//! key whose anchor occurs in an open line of that key that retirement removed from the segment's
//! `p1`, and nowhere in the final `p1`, leaves with the line. In a segment whose `p1` or `p2`
//! settles a key, the key's remaining candidates run in cited-message order, so claim validation,
//! which keeps the last claim per key, keeps the claim on the latest message, the user's answer,
//! whatever order the model emitted them in. A question or constraint whose claim has no anchor,
//! or an anchor elsewhere, stays a candidate; it is the key's stored claim only when every later
//! claim for the key fails validation.
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
//! settling lines where one open line stood only when its tier stays no longer than the model
//! wrote it, so the stored summary is bounded by what the model emitted.

use std::collections::{BTreeMap, BTreeSet};

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

/// The open lines one segment's `p1` lost to retirement, and the keys its `p1` or `p2` settles.
struct SegmentRetirement {
    settled: BTreeSet<String>,
    retired_from_p1: Vec<String>,
}

/// Retires each segment's settled open lines and the waiting lines of its `p3` and `p4` per the
/// module rules, keeps its `content` equal to its `p1`, removes the claim candidates of open lines
/// retired from `p1` and orders the claim candidates of the keys it settles, and returns the
/// segments.
pub(crate) fn retire_settled_lines<'o>(
    output: &'o mut ParsedHistorySegmentOutput,
    aliases: &FrozenAliasTable,
) -> &'o [ParsedHistorySegment] {
    let retired: Vec<SegmentRetirement> = output
        .history_segments
        .iter_mut()
        .map(retire_in_segment)
        .collect();
    order_settled_claims(output, aliases, &retired);
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

/// Retires one segment's settled open lines and aged waiting lines.
fn retire_in_segment(segment: &mut ParsedHistorySegment) -> SegmentRetirement {
    let p2 = segment.p2.as_deref().unwrap_or_default();
    let mut settled: BTreeSet<String> = settling_keys(p2).into_iter().map(str::to_owned).collect();
    let Some(p1) = segment.p1.as_deref() else {
        return SegmentRetirement {
            settled,
            retired_from_p1: Vec::new(),
        };
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
    let mut retired_from_p1 = Vec::new();
    for (tier, text) in [
        (1, &mut segment.p1),
        (2, &mut segment.p2),
        (3, &mut segment.p3),
        (4, &mut segment.p4),
    ] {
        let Some(text) = text.as_mut() else {
            continue;
        };
        // A tier never grows, so a tier within the durable byte limit stays within it: when the
        // copied outcomes would make it longer, its open lines leave without a copy.
        let Some((mut kept, mut retired)) = retire_tier(text, tier, &outcome, true) else {
            continue;
        };
        if kept.len() > text.len() {
            (kept, retired) = retire_tier(text, tier, &outcome, false)
                .expect("a retirement that copies an outcome also removes the open line");
        }
        if tier == 1 {
            retired_from_p1 = retired;
        }
        *text = kept;
    }
    if let Some(p1) = &segment.p1 {
        segment.content.clone_from(p1);
    }
    SegmentRetirement {
        settled,
        retired_from_p1,
    }
}

/// `text` of tier `tier` after retirement, and the lines retirement removed or replaced, or `None`
/// when no line changes. With `copy`, an open line whose key has an outcome gives way to it once
/// per key; without, it leaves. A blank line that would follow a blank line or open the text after
/// a removal goes with it.
fn retire_tier(
    text: &str,
    tier: u8,
    outcome: &BTreeMap<String, String>,
    copy: bool,
) -> Option<(String, Vec<String>)> {
    let lines: Vec<&str> = text.lines().collect();
    // The keys whose outcome this tier states, including outcomes it receives below.
    let mut stated: BTreeSet<&str> = settling_keys(text);
    let mut kept: Vec<&str> = Vec::new();
    let mut retired: Vec<String> = Vec::new();
    for (at, line) in lines.iter().enumerate() {
        let parsed = status_line(line);
        let aged = tier >= 3 && unbulleted(line).starts_with(WAITING_PREFIX);
        let open = matches!(parsed, Some((Status::Open, _)));
        if aged || open {
            let key = parsed.map(|(_, key)| key);
            let settled_below =
                key.is_some_and(|key| lines[at + 1..].iter().any(|line| settles(line, key)));
            let outcome_line = key.and_then(|key| outcome.get_key_value(key));
            if settled_below || outcome_line.is_some() || aged {
                retired.push((*line).to_owned());
                if !settled_below
                    && copy
                    && let Some((key, settling)) = outcome_line
                    && stated.insert(key)
                {
                    kept.push(settling);
                }
                continue;
            }
        }
        let blank = line.trim().is_empty();
        if blank && !retired.is_empty() && kept.last().is_none_or(|last| last.trim().is_empty()) {
            continue;
        }
        kept.push(line);
    }
    (!retired.is_empty()).then(|| (kept.join("\n").trim().to_owned(), retired))
}

/// Removes, in each segment, the claim candidates anchored inside an open line retirement removed
/// from its `p1`, and orders the remaining candidates of each key the segment settles by the
/// message their one citation names; every other candidate stays where it stands.
fn order_settled_claims(
    output: &mut ParsedHistorySegmentOutput,
    aliases: &FrozenAliasTable,
    retired: &[SegmentRetirement],
) {
    let segments = &output.history_segments;
    // The segment and ordinal of a candidate whose one citation names a message in a segment.
    let cited: Vec<Option<(usize, u64)>> = output
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
            Some((segment, ordinal))
        })
        .collect();
    let mut claims = std::mem::take(&mut output.claims);
    let mut keep = vec![true; claims.len()];
    for (at, candidate) in claims.iter().enumerate() {
        let Some((segment, _)) = cited[at] else {
            continue;
        };
        let final_p1 = segments[segment].p1.as_deref().unwrap_or_default();
        keep[at] = !candidate.anchor.as_deref().is_some_and(|anchor| {
            !anchor.trim().is_empty()
                && !final_p1.contains(anchor)
                && retired[segment].retired_from_p1.iter().any(|line| {
                    status_line(line).is_some_and(|(_, key)| key == candidate.key)
                        && line.contains(anchor)
                })
        });
    }
    // The kept positions of each settled key in each segment, which receive that key's kept
    // candidates in cited-message order.
    let mut slots: BTreeMap<(usize, String), Vec<usize>> = BTreeMap::new();
    for (at, candidate) in claims.iter().enumerate() {
        if let Some((segment, _)) = cited[at]
            && keep[at]
            && retired[segment].settled.contains(&candidate.key)
        {
            slots
                .entry((segment, candidate.key.clone()))
                .or_default()
                .push(at);
        }
    }
    let mut arranged: Vec<usize> = (0..claims.len()).collect();
    for positions in slots.values() {
        let mut by_message = positions.clone();
        by_message.sort_by_key(|&at| cited[at].map(|(_, ordinal)| ordinal));
        for (&slot, &from) in positions.iter().zip(&by_message) {
            arranged[slot] = from;
        }
    }
    let mut taken: Vec<Option<_>> = claims.drain(..).map(Some).collect();
    output.claims = arranged
        .into_iter()
        .enumerate()
        .filter(|&(slot, _)| keep[slot])
        .filter_map(|(_, from)| taken[from].take())
        .collect();
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
        let p1 = "Worked.\nWaiting on user (a.b): ask?\nDecided (a.b): yes.\nConstraint (c.d): no fixes.\nLifted (c.d): allowed.";
        let [p1, p2, p3, p4] = retired(vec![segment(
            p1,
            "Short.\nWaiting on user (a.b): ask?\nConstraint (c.d): no fixes.",
            "Old.\n- Waiting on user (a.b): ask?\nDecided (a.b): yes.",
            "Constraint (c.d): no fixes.",
        )])
        .remove(0);
        assert_eq!(p1, "Worked.\nDecided (a.b): yes.\nLifted (c.d): allowed.");
        assert_eq!(p2, "Short.\nDecided (a.b): yes.\nLifted (c.d): allowed.");
        assert_eq!(p3, "Old.\nDecided (a.b): yes.");
        assert_eq!(p4, "Lifted (c.d): allowed.");
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
    fn a_tier_never_grows_so_a_longer_outcome_replaces_nothing() {
        let p1 =
            "Constraint (c.d): no.\nLifted (c.d): the user now wants each bug fixed in its own PR.";
        let [p1_after, p2, _, p4] = retired(vec![segment(
            p1,
            "Short.\nConstraint (c.d): no.",
            "Done.",
            "Constraint (c.d): no.",
        )])
        .remove(0);
        assert_eq!(
            p1_after,
            "Lifted (c.d): the user now wants each bug fixed in its own PR."
        );
        // Each copy would lengthen its tier, so the open line leaves without one.
        assert_eq!(p2, "Short.");
        assert_eq!(p4, "");
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
            segment("Decided (a.b): yes.", "Lifted (c.d): allowed.", "Done.", ""),
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
    fn a_settling_segment_drops_retired_open_claims_and_orders_the_rest_by_message() {
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
            candidate("c.d", "[s3:0-1]", Some("(c.d)")),
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
        // In the second segment, the claim anchored only on the retired `c.d` question leaves; one
        // whose anchor the final `p1` still holds stays, and each settled key's other claims run
        // in message order, whatever their anchors. The first
        // segment settles nothing, so its claim stays, and so do the two-cite claim and the
        // unsettled key.
        assert_eq!(
            kept,
            [
                ("g.h", "[s4:0-1]"),
                ("g.h", "[s5:0-1]"),
                ("g.h", "[s6:0-1]"),
                ("c.d", "[s3:0-1]"),
                ("c.d", "[s4:0-1]"),
                ("c.d", "[s5:0-1]"),
                ("c.d", "[s6:0-1]"),
                ("a.b", "[s5:0-1]"),
                ("a.b", "[s2:0-1]"),
                ("c.d", "[s4:0-1] [s5:0-1]"),
                ("e.f", "[s1:0-1]"),
            ]
        );
    }
}
