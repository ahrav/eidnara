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
//! Waiting and constraint lines are open; decided and lifted lines settle. The state of every key
//! is derived from the stored segments, newest to oldest, each time the renderer builds its rows,
//! so it follows a session across folds with the rows themselves. The stored rows keep the lines
//! as written, and the summarizer's reference window and search read them there. A rendered row
//! retires an open line under these rules:
//!
//! - **Waiting lines** speak for the end of their own segment. A waiting line renders while its
//!   segment is the newest summarized segment among the rows rendered together; a legacy row and
//!   an archive row, which covers a span no fold summarized, leave it in place. The frozen
//!   baseline keeps its bytes between rebuilds, so a waiting line in its newest segment stays
//!   there until the next rebuild, with the newer segments rendered after it. The fold of the
//!   next span reads that segment in its
//!   reference window and states the item again in a newer segment, settles it, or passes over
//!   it; in each case the older line leaves. In the newest segment a waiting line leaves when the
//!   segment itself answers it: the key's last status line in `p1`, the segment's full statement,
//!   settles it, or a settling line with its key follows it in its own tier.
//! - **Constraints** stand in every tier until a lift with evidence. The evidence is a stored claim
//!   with the constraint's key whose anchor reads as the `Lifted` line of that key. Claim
//!   validation stores a claim only when its one citation resolves to a message inside the
//!   segment that stores it, and keeps its anchor only when the anchor is text of that segment's
//!   `p1`; the summarizer prompt has the claim cite the user's lifting message and anchor on the
//!   whole line. Such a claim in a later segment retires the constraint line; in the constraint's
//!   own segment it retires the line when `Lifted` is the key's last status line in `p1`. A
//!   `Decided` line, and any other claim, leaves the constraint line standing: a correction whose
//!   anchor lies in an open line of the row lists on the row's `[corrections: …]` line, so only
//!   retirement takes an open line out of a tier.
//!
//! The evidence lives in the claims of the summarized segments, which every read of such a row
//! carries, so the state is the same whether a row the decay curve archives was loaded with its
//! text or with its claims alone. The summarizer sees
//! the newest [`crate::history_summarizer_prompt::SESSION_REF_WINDOW`] segments when it folds. A
//! lift it writes under another key than the constraint's leaves the constraint line standing
//! beside the lifted line.
//!
//! Retirement only removes lines from a row's tiers. A tier it empties renders the row's heading
//! alone, at that tier and at a sparser tier the row stores no text for. Corrections apply to
//! the text that remains, so a superseded claim whose anchor stood only in a retired line lists
//! on the row's `[corrections: …]` line with the key's live value.

use std::collections::{BTreeMap, BTreeSet};

use memory_store::{Claim, StoredHistorySegment};

use crate::decay_render::{Correction, DecayRenderHistorySegment};
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

/// Whether `claim` is the evidence of a lift: its anchor is one line that reads as the `Lifted`
/// line of its own key.
fn lifts(claim: &Claim) -> bool {
    let one_line = claim
        .anchor
        .as_deref()
        .filter(|anchor| !anchor.contains('\n'));
    one_line.and_then(status_line) == Some((Status::Lifted, claim.key.as_str()))
}

/// The open lines one row retires at render.
#[derive(Debug)]
struct Retired {
    /// A newer summarized segment exists, so every waiting line leaves.
    aged: bool,
    /// The keys whose waiting lines leave: the row's own `p1` ends them with a settling line.
    answered: BTreeSet<String>,
    /// The keys whose constraint lines leave: a later row lifts them, or this row does and its
    /// `p1` ends them with the `Lifted` line.
    lifted: BTreeSet<String>,
}

/// What each row retires, aligned with `segments`; `None` for a row that renders flat or states
/// no key.
fn retirements(segments: &[StoredHistorySegment]) -> Vec<Option<Retired>> {
    let mut out: Vec<Option<Retired>> = segments.iter().map(|_| None).collect();
    // The keys a newer summarized segment lifts, and whether one exists.
    let mut lifted_later: BTreeSet<&str> = BTreeSet::new();
    let mut newer = false;
    for (at, segment) in segments.iter().enumerate().rev() {
        let lifted_here = || {
            segment
                .claims
                .iter()
                .filter(|claim| lifts(claim))
                .map(|claim| claim.key.as_str())
        };
        // A legacy row renders flat and an archive row renders empty over a span no fold
        // summarized. Every other row is a summarized segment, with or without its text loaded.
        let session_segment = segment.legacy != 1 && !segment.is_archive();
        if session_segment && let Some(p1) = segment.p1.as_deref().filter(|p1| !p1.is_empty()) {
            // Each key's last status line in `p1`.
            let last: BTreeMap<&str, Status> = p1
                .lines()
                .filter_map(status_line)
                .map(|(status, key)| (key, status))
                .collect();
            // A key that only a lower tier states still follows the later rows' lifts.
            let mut keys: BTreeSet<&str> = last.keys().copied().collect();
            for tier in [&segment.p2, &segment.p3, &segment.p4]
                .into_iter()
                .flatten()
            {
                keys.extend(tier.lines().filter_map(status_line).map(|(_, key)| key));
            }
            if !keys.is_empty() {
                let lifted_last: BTreeSet<&str> = lifted_here()
                    .filter(|key| last.get(key) == Some(&Status::Lifted))
                    .collect();
                out[at] = Some(Retired {
                    aged: newer,
                    answered: last
                        .iter()
                        .filter(|(_, status)| !status.is_open())
                        .map(|(key, _)| (*key).to_owned())
                        .collect(),
                    lifted: keys
                        .into_iter()
                        .filter(|key| lifted_later.contains(key) || lifted_last.contains(key))
                        .map(str::to_owned)
                        .collect(),
                });
            }
        }
        if session_segment {
            newer = true;
            lifted_later.extend(lifted_here());
        }
    }
    out
}

/// `text` after `retired` takes its open lines and each waiting line that a settling line with
/// its key follows in `text`, or `None` when every line stays. A blank line that a removal
/// leaves at the start or the end of the text, or next to another blank line, goes with the
/// removed line.
fn retire_lines(text: &str, retired: &Retired) -> Option<String> {
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
    let leaves = |at: usize| match parsed[at] {
        Some((Status::Waiting, key)) => {
            retired.aged
                || retired.answered.contains(key)
                || last_settling.get(key).is_some_and(|&last| last > at)
        }
        Some((Status::Constraint, key)) => retired.lifted.contains(key),
        _ => false,
    };
    let mut kept: Vec<&str> = Vec::with_capacity(lines.len());
    // Whether the line before the current one left.
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
        removed = false;
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

/// `row` with the tiers retirement leaves. A tier that retirement empties becomes one space, which
/// the renderer trims: the tier still counts as text, so the row renders from its tiers with its
/// heading and corrections, and a sparser tier the row stores no text for renders this one. The
/// flat `content` follows `p1`.
fn retire(
    mut row: DecayRenderHistorySegment,
    retired: Option<Retired>,
) -> DecayRenderHistorySegment {
    let Some(retired) = retired else {
        return row;
    };
    let mut p1_changed = false;
    for (tier, is_p1) in [
        (&mut row.p1, true),
        (&mut row.p2, false),
        (&mut row.p3, false),
        (&mut row.p4, false),
    ] {
        if let Some(text) = tier
            .as_deref()
            .and_then(|text| retire_lines(text, &retired))
        {
            *tier = Some(if text.is_empty() {
                " ".to_owned()
            } else {
                text
            });
            p1_changed |= is_p1;
        }
    }
    if p1_changed && let Some(p1) = &row.p1 {
        row.content.clone_from(p1);
    }
    row
}

/// The render rows of `segments` in their given order, oldest first, each with the tiers that
/// retirement leaves.
pub(crate) fn rows(
    segments: &[StoredHistorySegment],
) -> impl Iterator<Item = DecayRenderHistorySegment> + '_ {
    segments
        .iter()
        .map(DecayRenderHistorySegment::from)
        .zip(retirements(segments))
        .map(|(row, retired)| retire(row, retired))
}

/// [`rows`] over owned segments.
pub(crate) fn owned_rows(
    segments: Vec<StoredHistorySegment>,
) -> impl Iterator<Item = DecayRenderHistorySegment> {
    let retired = retirements(&segments);
    segments
        .into_iter()
        .map(DecayRenderHistorySegment::from)
        .zip(retired)
        .map(|(row, retired)| retire(row, retired))
}

/// `corrections` for `row`, with each one whose anchor lies in an open line of one of the row's
/// tiers listed on the footer: the line stays as written beside the key's live value, and
/// leaves only through retirement.
pub(crate) fn beside_open_lines(
    row: &DecayRenderHistorySegment,
    mut corrections: Vec<Correction>,
) -> Vec<Correction> {
    // Whether one of the lines that the first occurrence of `anchor` in the trimmed tier touches,
    // the occurrence a splice replaces, is an open status line.
    let in_open_line = |text: &str, anchor: &str| {
        let text = text.trim();
        text.find(anchor).is_some_and(|start| {
            let end = start + anchor.len();
            let from = text[..start].rfind('\n').map_or(0, |at| at + 1);
            let to = text[end..].find('\n').map_or(text.len(), |at| end + at);
            text[from..to]
                .lines()
                .any(|line| status_line(line).is_some_and(|(status, _)| status.is_open()))
        })
    };
    for correction in &mut corrections {
        let anchored_on_open_line = correction.anchor.as_deref().is_some_and(|anchor| {
            [&row.p1, &row.p2, &row.p3, &row.p4]
                .into_iter()
                .flatten()
                .any(|text| in_open_line(text, anchor))
        });
        if anchored_on_open_line {
            correction.anchor = None;
        }
    }
    corrections
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decay_render::{render_history_segment_at_tier, render_rows};
    use proptest::prelude::*;

    const WAITING: &str = "Waiting on user (release.server-steps): Shared checklist or own file?";
    const DECIDED: &str = "Decided (release.server-steps): The user chose a separate file.";
    const CONSTRAINT: &str = "Constraint (policy.measurement-fixes): No bug fixes during the run.";
    const LIFTED: &str = "Lifted (policy.measurement-fixes): The user wants each bug fixed now.";

    /// A claim for `key` anchored on `anchor`.
    fn claim(key: &str, anchor: Option<&str>) -> Claim {
        Claim {
            key: key.into(),
            value: "v".into(),
            ordinal: 1,
            anchor: anchor.map(str::to_owned),
        }
    }

    /// The claim that is the evidence of [`LIFTED`].
    fn lift() -> Claim {
        claim("policy.measurement-fixes", Some(LIFTED))
    }

    /// A stored row whose four tiers hold `tiers` and that stores `claims`.
    fn row(sequence: i64, tiers: [&str; 4], claims: Vec<Claim>) -> StoredHistorySegment {
        let [p1, p2, p3, p4] = tiers.map(str::to_owned);
        StoredHistorySegment {
            sequence,
            start_message: sequence * 10,
            end_message: sequence * 10 + 9,
            title: format!("row {sequence}"),
            content: p1.clone(),
            p1: Some(p1),
            p2: Some(p2),
            p3: Some(p3),
            p4: Some(p4),
            importance: 50,
            claims,
            ..StoredHistorySegment::default()
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
    fn a_waiting_line_renders_only_in_the_newest_tiered_row() {
        let asked = same(1, &format!("I drafted the steps.\n{WAITING}"), vec![]);
        // Alone, the question is open at the end of the newest segment.
        assert_eq!(
            p1s(std::slice::from_ref(&asked)),
            [format!("I drafted the steps.\n{WAITING}")]
        );
        // A later segment holds the answer, passes over the question, or raises it again; the
        // older line leaves in each case and the newer segment speaks for the item.
        for later in [
            format!("{DECIDED}\nI split the file."),
            "The user asked for the changelog instead.".to_owned(),
            format!("I raised the file question again.\n{WAITING}"),
        ] {
            assert_eq!(
                p1s(&[asked.clone(), same(2, &later, vec![])]),
                ["I drafted the steps.".to_owned(), later]
            );
        }
    }

    #[test]
    fn the_newest_segment_answers_its_own_waiting_line() {
        // `p1` ends the key with the decision, so the waiting line leaves every tier, wherever a
        // tier puts it.
        let p1 = format!("{WAITING}\nThe user answered.\n{DECIDED}");
        let p2 = format!("{DECIDED}\n{WAITING}");
        let answered = row(1, [&p1, &p2, WAITING, WAITING], vec![]);
        let rendered: Vec<DecayRenderHistorySegment> = rows(&[answered]).collect();
        assert_eq!(
            rendered[0].p1.as_deref(),
            Some(format!("The user answered.\n{DECIDED}").as_str())
        );
        assert_eq!(rendered[0].content, rendered[0].p1.clone().unwrap());
        assert_eq!(rendered[0].p2.as_deref(), Some(DECIDED));
        // A tier that held the waiting line alone keeps one space and renders the heading alone.
        assert_eq!(rendered[0].p3.as_deref(), Some(" "));
        assert_eq!(rendered[0].p4.as_deref(), Some(" "));
        assert_eq!(
            render_history_segment_at_tier(&rendered[0], 3),
            "## 10-19 · row 1"
        );

        // Asked, answered, and asked again: the last line of `p1` leaves the key open, and the
        // line the decision follows leaves its tier.
        let p1 = format!("{WAITING}\n{DECIDED}\nThe file moved.\n{WAITING}");
        let asked_again = row(1, [&p1, "", "", ""], vec![]);
        assert_eq!(
            p1s(&[asked_again]),
            [format!("{DECIDED}\nThe file moved.\n{WAITING}")]
        );

        // A lower tier settles a key that `p1` leaves unstated; the last settling line of the
        // tier decides for every waiting line before it.
        let lower = row(
            1,
            [
                "Narrative.",
                &format!("{DECIDED}\n{WAITING}\n{DECIDED}"),
                "",
                "",
            ],
            vec![],
        );
        let rendered: Vec<DecayRenderHistorySegment> = rows(&[lower]).collect();
        assert_eq!(
            rendered[0].p2.as_deref(),
            Some(format!("{DECIDED}\n{DECIDED}").as_str())
        );
    }

    #[test]
    fn a_constraint_stands_until_a_later_row_stores_the_lift_claim() {
        let set = same(1, &format!("{CONSTRAINT}\nBaseline measured."), vec![]);
        let other = same(2, "Unrelated work.", vec![]);
        let lifted = same(3, &format!("{LIFTED}\nI opened two PRs."), vec![lift()]);
        // The constraint stands in an old segment, in every tier, whatever follows it.
        for tier in rows(&[set.clone(), other.clone()])
            .next()
            .map(|row| [row.p1, row.p2, row.p3])
            .unwrap()
        {
            assert_eq!(
                tier.as_deref(),
                Some(format!("{CONSTRAINT}\nBaseline measured.").as_str())
            );
        }
        let rendered: Vec<DecayRenderHistorySegment> = rows(&[set, other, lifted]).collect();
        for tier in [&rendered[0].p1, &rendered[0].p2, &rendered[0].p3] {
            assert_eq!(tier.as_deref(), Some("Baseline measured."));
        }
        assert_eq!(
            rendered[2].p3.as_deref(),
            Some(format!("{LIFTED}\nI opened two PRs.").as_str())
        );
    }

    #[test]
    fn only_a_claim_anchored_on_the_lifted_line_of_the_key_lifts_a_constraint() {
        let set = same(1, CONSTRAINT, vec![]);
        let decided = "Decided (policy.measurement-fixes): The user confirmed the rule stays.";
        for (later, claims) in [
            // A lifted line whose claim validation dropped.
            (LIFTED, vec![]),
            // A claim under another key.
            (LIFTED, vec![claim("policy.other", Some(LIFTED))]),
            // A claim with the key whose anchor is other text of the segment, or absent.
            (
                LIFTED,
                vec![claim("policy.measurement-fixes", Some("each bug fixed"))],
            ),
            (LIFTED, vec![claim("policy.measurement-fixes", None)]),
            // A claim with the key anchored on another key's lifted line, or on running text
            // that names the lifted form.
            (
                LIFTED,
                vec![claim(
                    "policy.measurement-fixes",
                    Some("Lifted (policy.other): Another rule is lifted."),
                )],
            ),
            (
                LIFTED,
                vec![claim(
                    "policy.measurement-fixes",
                    Some("Note that Lifted (policy.measurement-fixes): is not the case."),
                )],
            ),
            // A claim with the key whose anchor holds the lifted line beside another line.
            (
                LIFTED,
                vec![claim(
                    "policy.measurement-fixes",
                    Some(&format!("Measured.\n{LIFTED}")),
                )],
            ),
            (
                LIFTED,
                vec![claim(
                    "policy.measurement-fixes",
                    Some(&format!("{LIFTED}\nI opened two PRs.")),
                )],
            ),
            // The constraint's own claim, restated in a later segment.
            (
                CONSTRAINT,
                vec![claim("policy.measurement-fixes", Some(CONSTRAINT))],
            ),
            // A decision under the constraint's key.
            (
                decided,
                vec![claim("policy.measurement-fixes", Some(decided))],
            ),
        ] {
            let rendered = p1s(&[set.clone(), same(2, later, claims.clone())]);
            assert_eq!(rendered, [CONSTRAINT, later], "{claims:?}");
        }
        // The claim lifts from the row that stores it: a lifted line in one row and the claim's
        // key on unrelated text in another leave the constraint standing.
        let unclaimed = same(2, LIFTED, vec![]);
        let elsewhere = same(
            3,
            "Later work.",
            vec![claim("policy.measurement-fixes", Some("Later work."))],
        );
        assert_eq!(p1s(&[set, unclaimed, elsewhere])[0], CONSTRAINT);
    }

    #[test]
    fn a_segment_lifts_its_own_constraint_when_its_p1_ends_the_key_lifted() {
        let set_then_lifted = format!("{CONSTRAINT}\nMeasured.\n{LIFTED}");
        let rendered: Vec<DecayRenderHistorySegment> = rows(&[row(
            1,
            [&set_then_lifted, CONSTRAINT, CONSTRAINT, ""],
            vec![lift()],
        )])
        .collect();
        assert_eq!(
            rendered[0].p1.as_deref(),
            Some(format!("Measured.\n{LIFTED}").as_str())
        );
        assert_eq!(rendered[0].p2.as_deref(), Some(" "));
        assert_eq!(rendered[0].p3.as_deref(), Some(" "));
        // The same lines with no stored lift claim keep the constraint.
        assert_eq!(
            p1s(&[same(1, &set_then_lifted, vec![])])[0],
            set_then_lifted
        );
        // Lifted and then set again in one segment: the later constraint stands there, and the
        // lift still retires the constraint of an older segment.
        let lifted_then_set = format!("{LIFTED}\n{CONSTRAINT}");
        assert_eq!(
            p1s(&[
                same(1, &format!("Older.\n{CONSTRAINT}"), vec![]),
                same(2, &lifted_then_set, vec![lift()])
            ]),
            ["Older.".to_owned(), lifted_then_set]
        );
    }

    #[test]
    fn a_constraint_set_again_after_its_lift_stands() {
        let rendered = p1s(&[
            same(1, &format!("First.\n{CONSTRAINT}"), vec![]),
            same(2, LIFTED, vec![lift()]),
            same(3, CONSTRAINT, vec![]),
        ]);
        assert_eq!(rendered, ["First.", LIFTED, CONSTRAINT]);
    }

    #[test]
    fn a_key_that_only_a_lower_tier_states_follows_a_later_lift() {
        for tiers in [
            ["Measured.", CONSTRAINT, "", ""],
            ["Measured.", "", CONSTRAINT, ""],
            ["Measured.", "", "", CONSTRAINT],
        ] {
            let rendered: Vec<DecayRenderHistorySegment> =
                rows(&[row(1, tiers, vec![]), same(2, LIFTED, vec![lift()])]).collect();
            let row = &rendered[0];
            assert_eq!(
                [&row.p2, &row.p3, &row.p4].map(|tier| tier.as_deref().map(str::trim)),
                [Some(""); 3],
                "{tiers:?}"
            );
        }
    }

    #[test]
    fn a_row_loaded_with_its_claims_alone_lifts_an_older_constraint() {
        let set = same(1, &format!("Measured.\n{CONSTRAINT}"), vec![]);
        // The bounded read loads an archived row past its window as sequence, importance and
        // claims; the full read loads its text too. Both lift the constraint.
        let textless = StoredHistorySegment {
            sequence: 2,
            importance: 20,
            claims: vec![lift()],
            ..StoredHistorySegment::default()
        };
        let newest = same(3, "Later work.", vec![]);
        for lifting in [textless, same(2, LIFTED, vec![lift()])] {
            assert_eq!(p1s(&[set.clone(), lifting, newest.clone()])[0], "Measured.");
        }
    }

    #[test]
    fn legacy_and_archive_rows_keep_their_text_and_leave_a_waiting_line_in_place() {
        let asked = same(1, &format!("I drafted the steps.\n{WAITING}"), vec![]);
        let mut legacy = same(2, WAITING, vec![]);
        legacy.legacy = 1;
        let mut archive = same(3, WAITING, vec![]);
        archive.episode_type = Some(memory_store::ARCHIVE_EPISODE_TYPE.to_owned());
        archive.content = String::new();
        assert!(archive.is_archive());
        let rendered: Vec<DecayRenderHistorySegment> =
            rows(&[asked.clone(), legacy.clone(), archive.clone()]).collect();
        assert_eq!(
            rendered[0].p1.as_deref(),
            Some(format!("I drafted the steps.\n{WAITING}").as_str())
        );
        // A summarized segment after them ages the waiting line, and the legacy and archive rows
        // keep their own text beside it.
        let later = same(4, "Later work.", vec![]);
        let rendered: Vec<DecayRenderHistorySegment> =
            rows(&[asked, legacy, archive, later]).collect();
        assert_eq!(rendered[0].p1.as_deref(), Some("I drafted the steps."));
        assert_eq!(rendered[1].content, WAITING);
        assert_eq!(rendered[1].p1.as_deref(), Some(WAITING));
        assert_eq!(rendered[2].p1.as_deref(), Some(WAITING));
    }

    #[test]
    fn a_lift_claim_counts_on_a_summarized_segment_only() {
        let set = same(1, &format!("Measured.\n{CONSTRAINT}"), vec![]);
        let mut legacy = same(2, LIFTED, vec![lift()]);
        legacy.legacy = 1;
        let mut archive = same(2, LIFTED, vec![lift()]);
        archive.episode_type = Some(memory_store::ARCHIVE_EPISODE_TYPE.to_owned());
        archive.content = String::new();
        for other in [legacy, archive] {
            assert_eq!(
                p1s(&[set.clone(), other])[0],
                format!("Measured.\n{CONSTRAINT}")
            );
        }
    }

    #[test]
    fn a_newer_segment_loaded_without_its_tiers_ages_a_waiting_line_and_keeps_its_own_text() {
        let asked = same(1, &format!("I drafted the steps.\n{WAITING}"), vec![]);
        // A row the bounded read loads as its claims alone, and a row whose `p1` is blank.
        let textless = StoredHistorySegment {
            sequence: 2,
            ..StoredHistorySegment::default()
        };
        let mut blank = same(2, WAITING, vec![]);
        blank.p1 = Some(String::new());
        for newer in [textless, blank] {
            let flat_tier = newer.p2.clone();
            let rendered: Vec<DecayRenderHistorySegment> = rows(&[asked.clone(), newer]).collect();
            assert_eq!(rendered[0].p1.as_deref(), Some("I drafted the steps."));
            assert_eq!(rendered[1].p2, flat_tier);
        }
    }

    #[test]
    fn a_row_whose_p1_empties_still_renders_its_tiers_and_its_corrections() {
        let narrative = format!("{CONSTRAINT}\nBaseline measured.");
        let set = row(
            1,
            [CONSTRAINT, &narrative, &narrative, ""],
            vec![claim("policy.measurement-fixes", Some(CONSTRAINT))],
        );
        let lifting = Claim {
            value: "fix them now".into(),
            ordinal: 21,
            ..lift()
        };
        let rendered = render_rows(&[set, same(2, LIFTED, vec![lifting])], false);
        let footer = "[corrections: policy.measurement-fixes = fix them now @21]";
        assert_eq!(
            render_history_segment_at_tier(&rendered[0], 1),
            format!("## 10-19 · row 1\n{footer}")
        );
        for tier in [2, 3] {
            assert_eq!(
                render_history_segment_at_tier(&rendered[0], tier),
                format!("## 10-19 · row 1\nBaseline measured.\n{footer}")
            );
        }
    }

    #[test]
    fn a_tier_the_row_stores_no_text_for_renders_the_denser_tier_retirement_leaves() {
        let heading = "## 10-19 \u{b7} row 1";
        let lifted = same(2, LIFTED, vec![lift()]);
        let missing = |p2: &str, p3: Option<&str>| {
            let mut set = row(1, ["Baseline measured in detail.", p2, "", ""], vec![]);
            set.p3 = p3.map(str::to_owned);
            set.p4 = None;
            render_rows(&[set, lifted.clone()], false).remove(0)
        };
        // The tier the missing ones fall back to held the constraint alone: all render the
        // heading, and the missing tiers stay missing.
        for p3 in [None, Some("")] {
            let rendered = missing(CONSTRAINT, p3);
            assert_eq!(rendered.p4, None);
            for tier in [2, 3, 4] {
                assert_eq!(render_history_segment_at_tier(&rendered, tier), heading);
            }
        }
        // It keeps its other lines, and the missing tiers render them.
        let rendered = missing(&format!("{CONSTRAINT}\nBaseline measured."), None);
        for tier in [2, 3, 4] {
            assert_eq!(
                render_history_segment_at_tier(&rendered, tier),
                format!("{heading}\nBaseline measured.")
            );
        }
    }

    #[test]
    fn a_claim_that_would_splice_into_a_standing_open_line_lists_beside_it() {
        let decided = "Decided (policy.measurement-fixes): The user confirmed the rule stays.";
        let later = Claim {
            value: "keep that rule".into(),
            ordinal: 21,
            ..claim("policy.measurement-fixes", Some(decided))
        };
        let narrative = "Baseline measured on port 5432.";
        // Anchors on the whole constraint line, on part of it, and across the line before it.
        for anchor in [
            CONSTRAINT.to_owned(),
            "No bug fixes".to_owned(),
            format!("port 5432.\n{CONSTRAINT}"),
        ] {
            let own = claim("policy.measurement-fixes", Some(&anchor));
            let set = same(1, &format!("{narrative}\n{CONSTRAINT}"), vec![own]);
            let rendered = render_rows(&[set, same(2, decided, vec![later.clone()])], false);
            for tier in [1, 2, 3] {
                assert_eq!(
                    render_history_segment_at_tier(&rendered[0], tier),
                    format!(
                        "## 10-19 \u{b7} row 1\n{narrative}\n{CONSTRAINT}\n\
                         [corrections: policy.measurement-fixes = keep that rule @21]"
                    ),
                    "{anchor}"
                );
            }
        }

        // A claim under another key whose words stand in the narrative still splices there, and
        // one whose words stand in the waiting line of the newest segment lists beside it.
        let port = |sequence, anchor: &str| Claim {
            ordinal: sequence,
            ..claim("db.port", Some(anchor))
        };
        let asked = "Waiting on user (db.port-move): Move the database off port 5432?";
        let rendered = render_rows(
            &[
                same(1, narrative, vec![port(1, "port 5432")]),
                same(2, asked, vec![port(2, "port 5432")]),
                same(3, asked, vec![port(3, "port 5432"), port(4, "port 5432")]),
            ],
            false,
        );
        assert_eq!(
            render_history_segment_at_tier(&rendered[0], 1),
            "## 10-19 \u{b7} row 1\nBaseline measured on [corrected @4: db.port = v]."
        );
        assert_eq!(
            render_history_segment_at_tier(&rendered[2], 1),
            format!("## 30-39 \u{b7} row 3\n{asked}\n[corrections: db.port = v @4]")
        );
    }

    #[test]
    fn a_correction_splices_where_its_first_occurrence_touches_no_open_line() {
        let heading = "## 10-19 \u{b7} row 1";
        let freeze = "Constraint (policy.freeze): Freeze the schema!";
        let p1 = format!(
            "Line one of the narrative.\nSecond narrative line.\nThird narrative line.\n\
             The server listens on port 5432\n{freeze}\nport 9 starts here.\n{DECIDED}\n\
             Freeze and port 5432 again."
        );
        // Row 1 stores a claim anchored on `anchor`, and row 2 supersedes it.
        let corrected = |tiers: [&str; 4], anchor: &str| {
            let port = |ordinal| Claim {
                ordinal,
                ..claim("db.port", Some(anchor))
            };
            let first = row(1, tiers, vec![port(1)]);
            render_rows(&[first, same(2, "Port moved.", vec![port(21)])], false).remove(0)
        };
        let marker = "[corrected @21: db.port = v]";
        let footer = "[corrections: db.port = v @21]";

        // Occurrences in the narrative before the open line, after it, and in a settling line.
        for anchor in ["port 5432", "port 9", "separate file"] {
            assert_eq!(
                render_history_segment_at_tier(&corrected([&p1, "", "", ""], anchor), 1),
                format!("{heading}\n{}", p1.replacen(anchor, marker, 1)),
                "{anchor}"
            );
        }
        // Occurrences at the start of the open line, at its end, inside it, and across the
        // line break before it.
        for anchor in ["Con", "!", "Freeze", "5432\nC"] {
            assert_eq!(
                render_history_segment_at_tier(&corrected([&p1, "", "", ""], anchor), 1),
                format!("{heading}\n{p1}\n{footer}"),
                "{anchor}"
            );
        }
        // The open line holds the words in one tier only: the correction lists on the footer
        // at that tier.
        let narrative = "The server listens on port 5432";
        let closed = "Constraint (policy.port): Keep port 5432 closed.";
        for (tiers, tier) in [
            ([narrative, closed, "", ""], 2),
            ([narrative, "", closed, ""], 3),
            ([narrative, "", "", closed], 4),
        ] {
            assert_eq!(
                render_history_segment_at_tier(&corrected(tiers, "port 5432"), tier),
                format!("{heading}\n{closed}\n{footer}"),
                "tier {tier}"
            );
        }
        // The start of an open line that ends its tier, and the line break after an open line.
        assert_eq!(
            render_history_segment_at_tier(&corrected([narrative, closed, "", ""], "Con"), 2),
            format!("{heading}\n{closed}\n{footer}")
        );
        let after = format!("{closed}\nport 9 starts here.");
        assert_eq!(
            render_history_segment_at_tier(&corrected([&after, "", "", ""], ".\nport 9"), 1),
            format!("{heading}\n{after}\n{footer}")
        );
        // A tier that opens with whitespace is searched as the renderer trims it.
        let indented = format!("  port 5432 is the port.\n{freeze} Keep  port 5432");
        assert_eq!(
            render_history_segment_at_tier(&corrected([&indented, "", "", ""], "  port 5432"), 1),
            format!("{heading}\n{}\n{footer}", indented.trim())
        );
    }

    #[test]
    fn the_prompts_example_lines_are_status_lines() {
        let prompt = crate::history_summarizer_prompt::HISTORY_SUMMARIZER_SYSTEM_PROMPT;
        let section = &prompt[prompt
            .find("## Open items")
            .expect("the prompt has an open-items section")..];
        let examples: Vec<(Status, &str)> = section
            .lines()
            .skip_while(|line| *line != "```")
            .skip(1)
            .take_while(|line| *line != "```")
            .map(|line| status_line(line).unwrap_or_else(|| panic!("{line}")))
            .collect();
        assert_eq!(
            examples,
            [
                (Status::Waiting, "release.server-steps"),
                (Status::Constraint, "policy.measurement-fixes"),
                (Status::Decided, "release.server-steps"),
                (Status::Lifted, "policy.measurement-fixes"),
            ]
        );
    }

    #[test]
    fn removal_keeps_the_other_lines_and_closes_the_gap_it_leaves() {
        let aged = Retired {
            aged: true,
            answered: BTreeSet::new(),
            lifted: BTreeSet::new(),
        };
        let text = format!(
            "{WAITING}\n\nFirst paragraph.\n\n{WAITING}\n\n  Indented line.\n\n{WAITING}\n\n"
        );
        assert_eq!(
            retire_lines(&text, &aged).as_deref(),
            Some("First paragraph.\n\n  Indented line.")
        );
        // A blank run the summarizer wrote after the removed line stays as written.
        assert_eq!(
            retire_lines(&format!("First.\n{WAITING}\n\n\nSecond."), &aged).as_deref(),
            Some("First.\n\n\nSecond.")
        );
        // A blank line at the end stays when the removed line stood elsewhere.
        assert_eq!(
            retire_lines(&format!("{WAITING}\nFirst.\n\n"), &aged).as_deref(),
            Some("First.\n")
        );
        assert_eq!(
            retire_lines("No status line.\n\n\nStill none.", &aged),
            None
        );
    }

    #[test]
    fn rows_that_retire_no_line_keep_their_tiers_byte_for_byte() {
        // Narrative that mentions the forms in running text, blank runs, and open lines that
        // stay: a constraint with no lift and a waiting line in the newest row.
        let narrative =
            "  I said I was Waiting on user (a.b): mid-sentence.\n\n\nU: Decided later.\n";
        let segments = vec![
            row(1, [narrative, narrative, "", ""], vec![claim("a.b", None)]),
            row(2, [CONSTRAINT, "", CONSTRAINT, CONSTRAINT], vec![]),
            row(3, [WAITING, WAITING, "\n", WAITING], vec![]),
        ];
        let tiers = |row: DecayRenderHistorySegment| (row.content, row.p1, row.p2, row.p3, row.p4);
        let projected: Vec<_> = segments
            .iter()
            .map(DecayRenderHistorySegment::from)
            .map(tiers)
            .collect();
        assert_eq!(rows(&segments).map(tiers).collect::<Vec<_>>(), projected);
        assert_eq!(
            owned_rows(segments).map(tiers).collect::<Vec<_>>(),
            projected
        );
    }

    /// One generated tier line: a status line over one of two keys, narrative, or a blank line.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Line {
        Status(Status, usize),
        Narrative(usize),
        Blank,
    }

    const KEYS: [&str; 2] = ["release.server-steps", "policy.measurement-fixes"];

    impl Line {
        fn text(self) -> String {
            match self {
                Line::Status(status, key) => {
                    let prefix = PREFIXES.iter().find(|(_, s)| *s == status).unwrap().0;
                    format!("{prefix}{}): status text", KEYS[key])
                }
                Line::Narrative(n) => format!("Narrative sentence {n}."),
                Line::Blank => String::new(),
            }
        }
    }

    fn line() -> impl Strategy<Value = Line> {
        prop_oneof![
            4 => (0usize..4, 0usize..2).prop_map(|(status, key)| {
                Line::Status(PREFIXES[status].1, key)
            }),
            2 => (0usize..3).prop_map(Line::Narrative),
            1 => Just(Line::Blank),
        ]
    }

    /// The tiers of one generated row, and for each key whether the row stores its lift claim.
    type Generated = ([Vec<Line>; 3], [bool; 2]);

    /// A stored `p1` has text, so each generated `p1` opens with a narrative line.
    fn generated_row() -> impl Strategy<Value = Generated> {
        (
            [
                prop::collection::vec(line(), 0..5),
                prop::collection::vec(line(), 0..5),
                prop::collection::vec(line(), 0..4),
            ],
            any::<[bool; 2]>(),
        )
            .prop_map(|(mut tiers, lift_claims)| {
                tiers[0].insert(0, Line::Narrative(9));
                (tiers, lift_claims)
            })
    }

    fn stored(sequence: usize, (tiers, lift_claims): &Generated) -> StoredHistorySegment {
        let text = |lines: &Vec<Line>| {
            lines
                .iter()
                .map(|line| line.text())
                .collect::<Vec<_>>()
                .join("\n")
        };
        let [p1, p2, p3] = tiers.each_ref().map(text);
        let claims = (0..2)
            .filter(|key| lift_claims[*key])
            .map(|key| {
                let anchor = Line::Status(Status::Lifted, key).text();
                claim(KEYS[key], Some(&anchor))
            })
            .collect();
        row(sequence as i64 + 1, [&p1, &p2, &p3, ""], claims)
    }

    /// The text lines of `tier` that a line-by-line reading of the module rules keeps.
    fn expected(generated: &[Generated], at: usize, tier: usize) -> Vec<String> {
        let (tiers, lift_claims) = &generated[at];
        let lines = &tiers[tier];
        let last_in_p1 = |key: usize| {
            tiers[0].iter().rev().find_map(|line| match line {
                Line::Status(status, k) if *k == key => Some(*status),
                _ => None,
            })
        };
        let lifted_later = |key: usize| generated[at + 1..].iter().any(|(_, lifts)| lifts[key]);
        lines
            .iter()
            .enumerate()
            .filter(|(index, line)| match line {
                Line::Status(Status::Waiting, key) => {
                    at + 1 == generated.len()
                        && last_in_p1(*key).is_none_or(Status::is_open)
                        && !lines[index + 1..].iter().any(
                            |later| matches!(later, Line::Status(s, k) if k == key && !s.is_open()),
                        )
                }
                Line::Status(Status::Constraint, key) => {
                    !lifted_later(*key)
                        && !(lift_claims[*key] && last_in_p1(*key) == Some(Status::Lifted))
                }
                Line::Blank => false,
                _ => true,
            })
            .map(|(_, line)| line.text())
            .collect()
    }

    proptest! {
        /// Every tier renders the lines a line-by-line reading of the rules keeps, in order,
        /// and the owned rows equal the borrowed rows.
        #[test]
        fn rendered_tiers_keep_exactly_the_lines_the_rules_keep(
            generated in prop::collection::vec(generated_row(), 1..5),
        ) {
            let segments: Vec<StoredHistorySegment> = generated
                .iter()
                .enumerate()
                .map(|(sequence, row)| stored(sequence, row))
                .collect();
            let rendered: Vec<DecayRenderHistorySegment> = rows(&segments).collect();
            for (at, row) in rendered.iter().enumerate() {
                for (tier, text) in [&row.p1, &row.p2, &row.p3].into_iter().enumerate() {
                    let kept: Vec<String> = text
                        .as_deref()
                        .unwrap_or_default()
                        .lines()
                        .filter(|line| !line.trim().is_empty())
                        .map(str::to_owned)
                        .collect();
                    prop_assert_eq!(kept, expected(&generated, at, tier), "row {} tier {}", at, tier);
                }
            }
            let tiers = |row: DecayRenderHistorySegment| (row.content, row.p1, row.p2, row.p3, row.p4);
            prop_assert_eq!(
                owned_rows(segments).map(tiers).collect::<Vec<_>>(),
                rendered.into_iter().map(tiers).collect::<Vec<_>>()
            );
        }
    }
}
