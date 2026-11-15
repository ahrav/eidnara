//! Open items in rendered history: the status lines a fold writes for questions that wait on the
//! user and for constraints the user set, and the state that decides which of them still render.
//!
//! A status line is one tier line in one of four forms, each naming its item by a claim key. It
//! may start with one `- ` or `* ` bullet and has text after its `):`:
//!
//! - `Waiting on user (key): …`: a question I put to the user that is open at the end of the
//!   segment.
//! - `Constraint (key): …`: a standing limit the user set for the work.
//! - `Decided (key): …`: the user's answer to an item.
//! - `Lifted (key): …`: a user constraint the user later lifted, stated as lifted.
//!
//! Waiting and constraint lines are open; decided and lifted lines settle the item with their key.
//! The state of every key is derived from the stored segments, oldest to newest, each time the
//! renderer builds its rows, so it follows a session across folds with the rows themselves. The
//! stored rows keep the lines as written, and the summarizer's reference window and search read
//! them there. The rendered rows drop an open line under these rules:
//!
//! - **Own segment.** A segment's `p1` is its full statement and decides its keys: a key whose last
//!   status line in `p1` settles it is settled in that segment, and the key's open lines leave every
//!   tier of the segment. In any tier, an open line followed by a settling line with its key leaves.
//! - **A later answer.** An open line leaves when a later segment's `p1` states a settling line with
//!   its key and that segment stores a claim with the key. Claim validation admits a claim only when
//!   its one citation resolves to a message inside the segment that holds it, so the claim is the
//!   answering turn. The earlier line's own claim has no part in this.
//! - **Constraints** stand in every tier until one of the two rules above settles them.
//! - **Waiting lines** speak for the end of their own segment. A waiting line renders while its
//!   segment is the newest tiered row among the rows rendered together. The fold of every later span
//!   read that segment in its reference window and either stated the item again in a newer segment,
//!   settled it, or passed over it; in each case the older line leaves.
//!
//! The pass only removes lines. A `p2` or `p3` it empties renders the next denser tier, as a tier
//! the summarizer left out does; an emptied `p1` or `p4` renders the heading alone. Corrections
//! apply to the text that remains, so a superseded claim whose anchor stood in a removed line lists
//! on the row's `[corrections: …]` line with the key's live value.

use std::collections::{BTreeMap, BTreeSet};

use memory_store::StoredHistorySegment;

use crate::decay_render::DecayRenderHistorySegment;
use crate::history_summarizer_citations::is_claim_key;

/// The form of a status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    Waiting,
    Constraint,
    Decided,
    Lifted,
}

impl Status {
    /// Whether the line leaves its item open.
    pub(crate) fn is_open(self) -> bool {
        matches!(self, Status::Waiting | Status::Constraint)
    }
}

const PREFIXES: [(&str, Status); 4] = [
    ("Waiting on user (", Status::Waiting),
    ("Constraint (", Status::Constraint),
    ("Decided (", Status::Decided),
    ("Lifted (", Status::Lifted),
];

/// The status and key of `line` when it is a status line: one of the four prefixes after leading
/// whitespace and at most one `- ` or `* ` bullet, a key in the claim-key grammar, and text after
/// `):`.
pub(crate) fn status_line(line: &str) -> Option<(Status, &str)> {
    let line = line.trim_start();
    let line = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .unwrap_or(line)
        .trim_start();
    let (status, rest) = PREFIXES
        .iter()
        .find_map(|(prefix, status)| line.strip_prefix(prefix).map(|rest| (*status, rest)))?;
    let (key, text) = rest.split_once("):")?;
    (is_claim_key(key) && !text.trim().is_empty()).then_some((status, key))
}

/// A stored row's `p1` when the row renders from its tiers.
fn tiered_p1(segment: &StoredHistorySegment) -> Option<&str> {
    let p1 = segment.p1.as_deref().filter(|p1| !p1.is_empty())?;
    (segment.legacy != 1 && !segment.is_archive()).then_some(p1)
}

/// What one row drops at render.
#[derive(Debug, Default)]
struct Retired<'s> {
    /// A newer tiered row exists, so every waiting line leaves.
    waiting: bool,
    /// The keys whose open lines leave: settled by the row's own `p1` or by a later row's
    /// settling line and claim.
    settled: BTreeSet<&'s str>,
}

/// The lines each row drops, aligned with `segments`; `None` for a row that renders flat or
/// states no key.
fn retired(segments: &[StoredHistorySegment]) -> Vec<Option<Retired<'_>>> {
    let mut out: Vec<Option<Retired<'_>>> = segments.iter().map(|_| None).collect();
    // The keys a newer row settles with a claim, and whether a newer tiered row exists.
    let mut settled_later: BTreeSet<&str> = BTreeSet::new();
    let mut newer = false;
    for (at, segment) in segments.iter().enumerate().rev() {
        let Some(p1) = tiered_p1(segment) else {
            continue;
        };
        let mut last: BTreeMap<&str, Status> = BTreeMap::new();
        let mut settles: BTreeSet<&str> = BTreeSet::new();
        for (status, key) in p1.lines().filter_map(status_line) {
            last.insert(key, status);
            if !status.is_open() {
                settles.insert(key);
            }
        }
        // A key only a lower tier states still follows the later rows' settlements.
        let mut keys: BTreeSet<&str> = last.keys().copied().collect();
        for tier in [&segment.p2, &segment.p3, &segment.p4].into_iter().flatten() {
            keys.extend(tier.lines().filter_map(status_line).map(|(_, key)| key));
        }
        if !keys.is_empty() {
            out[at] = Some(Retired {
                waiting: newer,
                settled: keys
                    .into_iter()
                    .filter(|key| {
                        settled_later.contains(key)
                            || last.get(key).is_some_and(|status| !status.is_open())
                    })
                    .collect(),
            });
        }
        settled_later.extend(
            settles
                .into_iter()
                .filter(|key| segment.claims.iter().any(|claim| claim.key == *key)),
        );
        newer = true;
    }
    out
}

/// `text` without the open lines `retired` drops and without each open line that a settling line
/// with its key follows in `text`, or `None` when every line stays. A blank line that would open
/// the text, close it, or follow another blank line after a removal goes with the removed line.
fn without_retired(text: &str, retired: &Retired<'_>) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let parsed: Vec<Option<(Status, &str)>> = lines.iter().map(|line| status_line(line)).collect();
    let mut last_settling: BTreeMap<&str, usize> = BTreeMap::new();
    for (at, status) in parsed.iter().enumerate() {
        if let Some((status, key)) = status
            && !status.is_open()
        {
            last_settling.insert(key, at);
        }
    }
    let leaves = |at: usize| {
        let Some((status, key)) = parsed[at] else {
            return false;
        };
        status.is_open()
            && (retired.settled.contains(key)
                || (retired.waiting && status == Status::Waiting)
                || last_settling.get(key).is_some_and(|&last| last > at))
    };
    let mut kept: Vec<&str> = Vec::with_capacity(lines.len());
    // Whether a line left since the last kept line that is not blank.
    let mut removed = false;
    let mut any = false;
    for (at, line) in lines.iter().enumerate() {
        if leaves(at) {
            removed = true;
            any = true;
            continue;
        }
        let blank = line.trim().is_empty();
        if blank && removed && kept.last().is_none_or(|last| last.trim().is_empty()) {
            continue;
        }
        removed &= blank;
        kept.push(line);
    }
    if !any {
        return None;
    }
    if removed {
        while kept.last().is_some_and(|line| line.trim().is_empty()) {
            kept.pop();
        }
    }
    Some(kept.join("\n"))
}

/// Applies `retired` to one render row's tiers and keeps its flat `content` equal to its `p1`.
fn retire_row(row: &mut DecayRenderHistorySegment, retired: &Retired<'_>) {
    if let Some(p1) = row.p1.as_deref().and_then(|p1| without_retired(p1, retired)) {
        row.content.clone_from(&p1);
        row.p1 = Some(p1);
    }
    for tier in [&mut row.p2, &mut row.p3] {
        if let Some(text) = tier.as_deref().and_then(|text| without_retired(text, retired)) {
            *tier = Some(text).filter(|text| !text.trim().is_empty());
        }
    }
    if let Some(p4) = row.p4.as_deref().and_then(|p4| without_retired(p4, retired)) {
        row.p4 = Some(p4);
    }
}

/// The render rows of `segments` in their given order, oldest first, without the open lines the
/// module rules retire.
pub(crate) fn rows(
    segments: &[StoredHistorySegment],
) -> impl Iterator<Item = DecayRenderHistorySegment> + '_ {
    let retired = retired(segments);
    segments.iter().zip(retired).map(|(segment, retired)| {
        let mut row = DecayRenderHistorySegment::from(segment);
        if let Some(retired) = retired {
            retire_row(&mut row, &retired);
        }
        row
    })
}

/// [`rows`] over owned segments.
pub(crate) fn owned_rows(segments: Vec<StoredHistorySegment>) -> Vec<DecayRenderHistorySegment> {
    // The retirement plan borrows keys from the stored tiers, so each row's tiers are rewritten
    // before the rows move.
    let rewritten: Vec<Option<DecayRenderHistorySegment>> = retired(&segments)
        .into_iter()
        .zip(&segments)
        .map(|(retired, segment)| {
            retired.map(|retired| {
                let mut row = DecayRenderHistorySegment::from(segment);
                retire_row(&mut row, &retired);
                row
            })
        })
        .collect();
    segments
        .into_iter()
        .zip(rewritten)
        .map(|(segment, rewritten)| rewritten.unwrap_or_else(|| segment.into()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory_store::Claim;

    fn claim(key: &str, ordinal: i64) -> Claim {
        Claim {
            key: key.into(),
            value: "v".into(),
            ordinal,
            anchor: None,
        }
    }

    /// A stored row whose four tiers hold `tiers` and that stores `claims`.
    fn row(sequence: i64, tiers: [&str; 4], claims: Vec<Claim>) -> StoredHistorySegment {
        let [p1, p2, p3, p4] = tiers.map(str::to_owned);
        StoredHistorySegment {
            sequence,
            start_message: sequence * 10,
            end_message: sequence * 10 + 9,
            start_message_id: String::new(),
            end_message_id: String::new(),
            start_date: None,
            end_date: None,
            title: format!("row {sequence}"),
            content: p1.clone(),
            p1: Some(p1),
            p2: Some(p2),
            p3: Some(p3),
            p4: Some(p4),
            importance: 50,
            episode_type: None,
            legacy: 0,
            created_at: 0,
            claims,
        }
    }

    /// A row whose `p1`, `p2` and `p3` all hold `text`.
    fn same(sequence: i64, text: &str, claims: Vec<Claim>) -> StoredHistorySegment {
        row(sequence, [text, text, text, ""], claims)
    }

    /// The `p1` each row renders.
    fn p1s(segments: &[StoredHistorySegment]) -> Vec<String> {
        rows(segments)
            .map(|row| row.p1.unwrap_or_default())
            .collect()
    }

    const WAITING: &str = "Waiting on user (release.server-steps): Shared checklist or own file?";
    const DECIDED: &str = "Decided (release.server-steps): The user chose a separate file.";
    const CONSTRAINT: &str = "Constraint (policy.measurement-fixes): No bug fixes during the run.";
    const LIFTED: &str = "Lifted (policy.measurement-fixes): The user wants each bug fixed now.";

    #[test]
    fn a_status_line_needs_a_prefix_a_claim_key_and_text() {
        assert_eq!(
            status_line(WAITING),
            Some((Status::Waiting, "release.server-steps"))
        );
        assert_eq!(
            status_line(&format!("  - {CONSTRAINT}")),
            Some((Status::Constraint, "policy.measurement-fixes"))
        );
        assert_eq!(
            status_line(&format!("* {DECIDED}")),
            Some((Status::Decided, "release.server-steps"))
        );
        assert_eq!(
            status_line(LIFTED),
            Some((Status::Lifted, "policy.measurement-fixes"))
        );
        for line in [
            "I am Waiting on user (a.b): not a line start",
            "Waiting on user (nodot): key without a dot",
            "Waiting on user (A.b): uppercase key",
            "Waiting on user (a.b):   ",
            "Waiting on user (a.b) no colon",
            "**Waiting on user (a.b): emphasised**",
            "1. Waiting on user (a.b): numbered",
        ] {
            assert_eq!(status_line(line), None, "{line}");
        }
    }

    #[test]
    fn a_pending_question_closes_when_a_later_fold_holds_the_answer() {
        let first = same(1, &format!("I drafted the steps.\n{WAITING}"), vec![]);
        // Fold 1 alone: the question is open at the end of the newest segment.
        assert_eq!(
            p1s(std::slice::from_ref(&first)),
            [format!("I drafted the steps.\n{WAITING}")]
        );
        // Fold 2 holds the answer and cites it; the earlier waiting line carries no claim.
        let second = same(
            2,
            &format!("{DECIDED}\nI split the file."),
            vec![claim("release.server-steps", 21)],
        );
        assert_eq!(
            p1s(&[first, second]),
            [
                "I drafted the steps.".to_owned(),
                format!("{DECIDED}\nI split the file.")
            ]
        );
    }

    #[test]
    fn an_unanswered_question_ages_out_once_a_later_segment_passes_over_it() {
        let first = same(1, &format!("I drafted the steps.\n{WAITING}"), vec![]);
        let second = same(2, "The user asked for the changelog instead.", vec![]);
        assert_eq!(
            p1s(&[first, second]),
            [
                "I drafted the steps.",
                "The user asked for the changelog instead."
            ]
        );
    }

    #[test]
    fn a_question_stated_again_in_the_newest_segment_stays_open_once() {
        let first = same(1, &format!("I drafted the steps.\n{WAITING}"), vec![]);
        let second = same(2, &format!("I raised the file question again.\n{WAITING}"), vec![]);
        let rendered = p1s(&[first, second]);
        assert_eq!(rendered[0], "I drafted the steps.");
        assert_eq!(
            rendered[1],
            format!("I raised the file question again.\n{WAITING}")
        );
    }

    #[test]
    fn a_lifted_constraint_leaves_and_the_lift_stays() {
        let first = same(1, &format!("{CONSTRAINT}\nBaseline measured."), vec![]);
        let middle = same(2, "Unrelated work.", vec![]);
        let third = same(
            3,
            &format!("{LIFTED}\nI opened two PRs."),
            vec![claim("policy.measurement-fixes", 31)],
        );
        // Before the lift the constraint stands in an old segment, whatever follows it.
        assert_eq!(
            p1s(&[first.clone(), middle.clone()])[0],
            format!("{CONSTRAINT}\nBaseline measured.")
        );
        let rendered: Vec<DecayRenderHistorySegment> = rows(&[first, middle, third]).collect();
        for tier in [&rendered[0].p1, &rendered[0].p2, &rendered[0].p3] {
            assert_eq!(tier.as_deref(), Some("Baseline measured."));
        }
        assert_eq!(
            rendered[2].p3.as_deref(),
            Some(format!("{LIFTED}\nI opened two PRs.").as_str())
        );
    }

    #[test]
    fn a_settling_line_without_a_claim_leaves_an_earlier_constraint_standing() {
        let first = same(1, CONSTRAINT, vec![]);
        let second = same(2, LIFTED, vec![]);
        assert_eq!(p1s(&[first.clone(), second]), [CONSTRAINT, LIFTED]);
        // A claim under another key is evidence for that key only.
        let other = same(2, LIFTED, vec![claim("policy.other", 21)]);
        assert_eq!(p1s(&[first, other]), [CONSTRAINT, LIFTED]);
    }

    #[test]
    fn an_answer_under_another_key_leaves_no_waiting_line_and_keeps_the_constraint() {
        let first = same(1, &format!("{CONSTRAINT}\n{WAITING}"), vec![]);
        let second = same(
            2,
            "Decided (release.checklist-layout): The user chose a separate file.\n\
             Lifted (policy.bug-fixes): The user wants each bug fixed now.",
            vec![
                claim("release.checklist-layout", 21),
                claim("policy.bug-fixes", 21),
            ],
        );
        let rendered = p1s(&[first, second]);
        assert_eq!(rendered[0], CONSTRAINT);
        assert!(rendered[1].contains("Decided (release.checklist-layout)"));
    }

    #[test]
    fn a_segment_settles_its_own_open_lines_in_every_tier() {
        let p1 = format!("{CONSTRAINT}\n{WAITING}\nThe user answered.\n{DECIDED}\n{LIFTED}");
        let p2 = format!("{WAITING}\n{CONSTRAINT}\nOutcome recorded.");
        let p3 = WAITING.to_owned();
        let only = row(1, [&p1, &p2, &p3, WAITING], vec![]);
        let rendered: Vec<DecayRenderHistorySegment> = rows(&[only]).collect();
        let row = &rendered[0];
        assert_eq!(
            row.p1.as_deref(),
            Some(format!("The user answered.\n{DECIDED}\n{LIFTED}").as_str())
        );
        assert_eq!(row.content, row.p1.clone().unwrap());
        assert_eq!(row.p2.as_deref(), Some("Outcome recorded."));
        // An emptied `p3` renders the next denser tier; an emptied `p4` renders the heading alone.
        assert_eq!(row.p3, None);
        assert_eq!(row.p4.as_deref(), Some(""));
    }

    #[test]
    fn a_question_asked_again_after_its_answer_is_open_in_the_newest_segment() {
        let first = same(
            1,
            &format!("{WAITING}\n{DECIDED}"),
            vec![claim("release.server-steps", 11)],
        );
        let second = same(2, &format!("{DECIDED}\nThe file moved.\n{WAITING}"), vec![]);
        let rendered = p1s(&[first, second]);
        assert_eq!(rendered[0], DECIDED);
        assert_eq!(rendered[1], format!("{DECIDED}\nThe file moved.\n{WAITING}"));
    }

    #[test]
    fn a_constraint_set_again_after_its_lift_stands() {
        let first = same(1, CONSTRAINT, vec![]);
        let second = same(2, LIFTED, vec![claim("policy.measurement-fixes", 21)]);
        let third = same(3, CONSTRAINT, vec![]);
        assert_eq!(p1s(&[first, second, third]), ["", LIFTED, CONSTRAINT]);
    }

    #[test]
    fn a_key_only_a_lower_tier_states_follows_a_later_settlement() {
        let first = row(1, ["Baseline measured.", CONSTRAINT, "", ""], vec![]);
        let second = same(2, LIFTED, vec![claim("policy.measurement-fixes", 21)]);
        let rendered: Vec<DecayRenderHistorySegment> = rows(&[first, second]).collect();
        assert_eq!(rendered[0].p2, None);
    }

    #[test]
    fn flat_and_archive_rows_neither_change_nor_count_as_newer() {
        let mut legacy = same(1, WAITING, vec![]);
        legacy.legacy = 1;
        let mut untiered = same(3, WAITING, vec![]);
        untiered.p1 = None;
        let tiered = same(2, WAITING, vec![]);
        let rendered: Vec<DecayRenderHistorySegment> =
            rows(&[legacy, tiered, untiered]).collect();
        assert_eq!(rendered[0].content, WAITING);
        assert_eq!(rendered[1].p1.as_deref(), Some(WAITING));
        assert_eq!(rendered[2].content, WAITING);
    }

    #[test]
    fn removal_keeps_the_other_lines_and_closes_the_gap_it_leaves() {
        let text = format!(
            "{WAITING}\n\nFirst paragraph.\n\n{WAITING}\n\n  Indented line.\n\n{WAITING}\n\n"
        );
        let retired = Retired {
            waiting: true,
            settled: BTreeSet::new(),
        };
        assert_eq!(
            without_retired(&text, &retired).as_deref(),
            Some("First paragraph.\n\n  Indented line.")
        );
        assert_eq!(without_retired("No status line.\n\n\nStill none.", &retired), None);
    }

    #[test]
    fn owned_rows_render_as_borrowed_rows() {
        let segments = vec![
            same(1, &format!("{CONSTRAINT}\n{WAITING}"), vec![]),
            same(2, "No status line.", vec![]),
            same(
                3,
                &format!("{LIFTED}\n{WAITING}"),
                vec![claim("policy.measurement-fixes", 31)],
            ),
        ];
        let borrowed: Vec<_> = rows(&segments)
            .map(|row| (row.content, row.p1, row.p2, row.p3, row.p4))
            .collect();
        let owned: Vec<_> = owned_rows(segments)
            .into_iter()
            .map(|row| (row.content, row.p1, row.p2, row.p3, row.p4))
            .collect();
        assert_eq!(borrowed, owned);
        assert_eq!(owned[0].1.as_deref(), Some(""));
        assert_eq!(owned[2].1.as_deref(), Some(format!("{LIFTED}\n{WAITING}").as_str()));
    }
}
