//! Open items in rendered history: the status lines a fold writes for questions that wait on the
//! user and for constraints the user set, and the state that decides which of them still render.
//!
//! A status line is one tier line in one of four forms. Each form holds a key between
//! parentheses. The line may start with one `- ` or `* ` bullet and has text after its `):`:
//!
//! - `Waiting on user (key): …`: a question I put to the user that is open at the end of the
//!   segment.
//! - `Constraint (key): …`: a standing limit the user set for the work.
//! - `Decided (key): …`: the user's answer to an item.
//! - `Lifted (key): …`: a user constraint the user later lifted, stated as lifted.
//!
//! A line's form makes it a status line, whatever its key holds. The prompt asks for a key in
//! the claim-key grammar, and the fold stores the line as the summarizer wrote it, so a stored
//! key may hold capitals, spaces, another alphabet, more bytes than a claim key, or nothing. The
//! agent reads every line of the form as a status of the work, so every such line follows the
//! rules below. The key is the bytes up to the line's first `):`. Two lines name one item when
//! their keys hold a character other than whitespace and are equal byte for byte. The state is
//! derived at render, so a row stored with any key renders under these rules. A constraint whose
//! key lies outside the claim-key grammar stands while its segment renders: a lift needs a stored
//! claim with the constraint's key, and a stored claim's key is in that grammar.
//!
//! Waiting and constraint lines are open; decided and lifted lines settle. The state of every key
//! is derived from the stored segments, newest to oldest, each time the renderer builds its rows,
//! so it follows a session across folds with the rows themselves. The stored rows keep the lines
//! as written, and the summarizer's reference window and search read them there. A rendered row
//! retires an open line under these rules:
//!
//! - **Waiting lines** speak for the end of their own segment. A waiting line renders while its
//!   segment is the newest summarized segment among the rows rendered together; a legacy row and
//!   an archive row, which covers a span no fold summarized, leave it in place. The fold of the
//!   next span reads that segment in its reference window and states the item again in a newer
//!   segment, settles it, or passes over it; in each case the older line leaves. In the newest
//!   segment a waiting line leaves when the segment itself answers it: the key's last status line
//!   in `p1`, the segment's full statement, settles it, or a settling line with its key follows
//!   it in its own tier. A waiting line in a newer segment opens its key again after an older
//!   segment settled it.
//! - **Constraints** stand in every tier until a lift with evidence. The evidence is a stored claim
//!   with the constraint's key whose anchor reads as the `Lifted` line of that key. Claim
//!   validation stores a claim only when its one citation resolves to a message inside the
//!   segment that stores it, and keeps an anchor that reads as a `Lifted` line only when the
//!   claim cites the text of a user message and the anchor is a whole `Lifted` line of that
//!   segment's `p1`. The lift holds while that claim is the key's live claim, the newest claim
//!   with the key among the summarized segments rendered together. Such a claim in a later
//!   segment retires the constraint line; in the constraint's own segment it retires the line
//!   when `Lifted` is the key's last status line in `p1`. A `Decided` line, and any other claim,
//!   leaves the constraint line standing: a correction whose anchor touches an open line of the
//!   row lists on the row's `[corrections: …]` line, so only retirement takes an open line out
//!   of a tier.
//!
//! The rules read the rows rendered together. The frozen baseline keeps its bytes between
//! rebuilds and the rows above it render on their own, so an open line in the baseline stays
//! until the next rebuild, with the segment that settles it rendered after it.
//!
//! The evidence lives in the claims of the summarized segments, which every read of such a row
//! carries, so the state is the same whether a row the decay curve archives was loaded with its
//! text or with its claims alone. The summarizer sees the newest
//! [`crate::history_summarizer_prompt::SESSION_REF_WINDOW`] segments when it folds. A lift it
//! writes under another key than the constraint's leaves the constraint line standing beside the
//! lifted line.
//!
//! Retirement only removes lines from a row's tiers. A tier it empties renders the row's heading
//! alone, at that tier and at a sparser tier the row stores no text for. Corrections apply to
//! the text that remains, so a superseded claim whose anchor stood only in a retired line lists
//! on the row's `[corrections: …]` line with the key's live value.

use std::collections::{BTreeMap, BTreeSet};

use memory_store::{Claim, StoredHistorySegment};

use crate::decay_render::{Correction, DecayRenderHistorySegment};

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

/// `line` after its leading whitespace and at most one `- ` or `* ` bullet.
fn unbulleted(line: &str) -> &str {
    let line = line.trim_start();
    line.strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .unwrap_or(line)
        .trim_start()
}

/// The status and key of `line` when it is a status line: one of the four prefixes after leading
/// whitespace and at most one `- ` or `* ` bullet, a key that ends at the line's first `):`, and
/// text after it. `status_line` preserves the key byte for byte.
pub(crate) fn status_line(line: &str) -> Option<(Status, &str)> {
    let line = unbulleted(line);
    let (status, rest) = PREFIXES
        .iter()
        .find_map(|(prefix, status)| line.strip_prefix(prefix).map(|rest| (*status, rest)))?;
    let (key, text) = rest.split_once("):")?;
    (!key.contains('\n') && !text.trim().is_empty()).then_some((status, key))
}

/// A settling line settles the waiting lines that carry its key, so two lines with blank keys
/// stay separate items.
fn names_item(key: &str) -> bool {
    !key.trim().is_empty()
}

/// `memchr_lines` preserves [`str::lines`] semantics.
fn memchr_lines(text: &str) -> impl Iterator<Item = &str> {
    let mut start = 0;
    memchr::memchr_iter(b'\n', text.as_bytes())
        .map(Some)
        .chain(std::iter::once(None))
        .filter_map(move |end| match end {
            Some(end) => {
                let line = &text[start..end];
                start = end + 1;
                Some(line.strip_suffix('\r').unwrap_or(line))
            }
            None => (start < text.len()).then(|| &text[start..]),
        })
}

/// Whether `text` reads as a `Lifted` status line.
pub(crate) fn is_lifted_line(text: &str) -> bool {
    matches!(status_line(text), Some((Status::Lifted, _)))
}

/// Whether `anchor` is a whole `Lifted` line of `p1`, with or without the line's bullet.
pub(crate) fn is_lifted_line_of(p1: &str, anchor: &str) -> bool {
    p1.lines().any(|line| {
        is_lifted_line(line) && (line.trim() == anchor || unbulleted(line).trim_end() == anchor)
    })
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
    /// The keys whose last status line in the row's own `p1` settles them.
    answered: BTreeSet<String>,
    /// The keys whose constraint lines leave: a later row lifts them, or this row does and its
    /// `p1` ends them with the `Lifted` line.
    lifted: BTreeSet<String>,
}

/// What each row retires, aligned with `segments`; `None` for a row that renders flat or states
/// no key.
fn retirements(segments: &[StoredHistorySegment]) -> Vec<Option<Retired>> {
    let mut out: Vec<Option<Retired>> = segments.iter().map(|_| None).collect();
    // Over the newer summarized segments: the keys whose live claim, the newest one, is a lift;
    // the keys that have a claim at all; and whether such a segment exists.
    let mut lifted_later: BTreeSet<&str> = BTreeSet::new();
    let mut claimed_later: BTreeSet<&str> = BTreeSet::new();
    let mut newer = false;
    for (at, segment) in segments.iter().enumerate().rev() {
        // A legacy row renders flat and an archive row renders empty over a span no fold
        // summarized. Every other row is a summarized segment, with or without its text loaded.
        let session_segment = segment.legacy != 1 && !segment.is_archive();
        let lifted_here: BTreeSet<&str> = segment
            .claims
            .iter()
            .filter(|claim| lifts(claim) && !claimed_later.contains(claim.key.as_str()))
            .map(|claim| claim.key.as_str())
            .collect();
        if session_segment && let Some(p1) = segment.p1.as_deref().filter(|p1| !p1.is_empty()) {
            // Each key's last status line in `p1`.
            let last: BTreeMap<&str, Status> = memchr_lines(p1)
                .filter_map(status_line)
                .map(|(status, key)| (key, status))
                .collect();
            // A key that only a lower tier states still follows the later rows' lifts.
            let mut keys: BTreeSet<&str> = last.keys().copied().collect();
            for tier in [&segment.p2, &segment.p3, &segment.p4]
                .into_iter()
                .flatten()
            {
                keys.extend(
                    memchr_lines(tier)
                        .filter_map(status_line)
                        .map(|(_, key)| key),
                );
            }
            if !keys.is_empty() {
                out[at] = Some(Retired {
                    aged: newer,
                    answered: last
                        .iter()
                        .filter(|(_, status)| !status.is_open())
                        .map(|(key, _)| (*key).to_owned())
                        .collect(),
                    lifted: keys
                        .into_iter()
                        .filter(|key| {
                            lifted_later.contains(key)
                                || (lifted_here.contains(key)
                                    && last.get(key) == Some(&Status::Lifted))
                        })
                        .map(str::to_owned)
                        .collect(),
                });
            }
        }
        if session_segment {
            newer = true;
            lifted_later.extend(lifted_here);
            claimed_later.extend(segment.claims.iter().map(|claim| claim.key.as_str()));
        }
    }
    out
}

/// `text` after `retired` takes its open lines and each waiting line that a settling line with
/// its key follows in `text`, or `None` when every line stays. A blank line that a removal
/// leaves at the start or the end of the text, or next to another blank line, goes with the
/// removed line.
fn retire_lines(text: &str, retired: &Retired) -> Option<String> {
    let lines: Vec<&str> = memchr_lines(text).collect();
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
                || (names_item(key)
                    && (retired.answered.contains(key)
                        || last_settling.get(key).is_some_and(|&last| last > at)))
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

/// [`rows`] over owned segments, with `corrections`, aligned with them, passed through
/// [`beside_open_lines`] before the segments move.
pub(crate) fn owned_rows(
    segments: Vec<StoredHistorySegment>,
    corrections: &mut Vec<Vec<Correction>>,
) -> impl Iterator<Item = DecayRenderHistorySegment> + use<> {
    *corrections = beside_open_lines(&segments, std::mem::take(corrections));
    let retired = retirements(&segments);
    segments
        .into_iter()
        .map(DecayRenderHistorySegment::from)
        .zip(retired)
        .map(|(row, retired)| retire(row, retired))
}

/// `beside_open_lines` clears the anchor of each correction, aligned with `segments`, whose
/// anchor touches an open line of its row.
pub(crate) fn beside_open_lines(
    segments: &[StoredHistorySegment],
    mut corrections: Vec<Vec<Correction>>,
) -> Vec<Vec<Correction>> {
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
    let mut retired: Option<Vec<Option<Retired>>> = None;
    for (at, (segment, corrections)) in segments.iter().zip(&mut corrections).enumerate() {
        let tiers = [&segment.p1, &segment.p2, &segment.p3, &segment.p4];
        for correction in corrections {
            let Some(anchor) = correction.anchor.as_deref() else {
                continue;
            };
            let stored = tiers
                .into_iter()
                .flatten()
                .any(|text| in_open_line(text, anchor));
            let can_move = anchor.contains('\n') || anchor.trim() != anchor;
            let beside = stored
                || (can_move
                    && retired.get_or_insert_with(|| retirements(segments))[at]
                        .as_ref()
                        .is_some_and(|retired| {
                            tiers.into_iter().flatten().any(|text| {
                                retire_lines(text, retired)
                                    .is_some_and(|left| in_open_line(&left, anchor))
                            })
                        }));
            if beside {
                correction.anchor = None;
            }
        }
    }
    corrections
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decay_render::{
        render_history_segment_at_tier, render_owned_rows, render_rows as render_borrowed_rows,
    };
    use proptest::prelude::*;

    const WAITING: &str = "Waiting on user (release.server-steps): Shared checklist or own file?";
    const DECIDED: &str = "Decided (release.server-steps): The user chose a separate file.";
    const CONSTRAINT: &str = "Constraint (policy.measurement-fixes): No bug fixes during the run.";
    const LIFTED: &str = "Lifted (policy.measurement-fixes): The user wants each bug fixed now.";

    /// The rows of `segments` as the rows above the baseline render them, after checking that
    /// the baseline's owned rows render the same text at every tier.
    fn render_rows(
        segments: &[StoredHistorySegment],
        temporal_awareness: bool,
    ) -> Vec<DecayRenderHistorySegment> {
        let borrowed = render_borrowed_rows(segments, temporal_awareness);
        let owned = render_owned_rows(segments.to_vec(), temporal_awareness);
        for tier in 1..=4 {
            let at_tier = |rows: &[DecayRenderHistorySegment]| -> Vec<String> {
                rows.iter()
                    .map(|row| render_history_segment_at_tier(row, tier))
                    .collect()
            };
            assert_eq!(at_tier(&borrowed), at_tier(&owned), "tier {tier}");
        }
        borrowed
    }

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
    fn a_status_line_needs_a_prefix_a_key_in_parentheses_and_text() {
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
        // One bullet, with any space after it.
        assert_eq!(
            status_line(&format!("-   {DECIDED}")),
            Some((Status::Decided, "release.server-steps"))
        );
        let long = "release.".repeat(9);
        for key in [
            "A.b",
            "nodot",
            "release server steps",
            "выпуск.шаги",
            "fn(x)",
            " ",
            "",
            long.as_str(),
        ] {
            for (prefix, status) in PREFIXES {
                assert_eq!(
                    status_line(&format!("{prefix}{key}): text")),
                    Some((status, key)),
                    "{prefix}{key}"
                );
            }
        }
        assert_eq!(
            status_line("Decided (a.b): the call f(x): returns"),
            Some((Status::Decided, "a.b"))
        );
        for line in [
            "- - Waiting on user (a.b): two bullets",
            "I am Waiting on user (a.b): not a line start",
            "Waiting on user (a.b):   ",
            "Waiting on user (a.b) no colon",
            "Waiting on user: no parentheses",
            "Lifted (see the note\nDecided (a.b): a key across two lines",
            "**Waiting on user (a.b): emphasised**",
            "1. Waiting on user (a.b): numbered",
        ] {
            assert_eq!(status_line(line), None, "{line}");
        }
    }

    /// `status_line` accepts these keys; `is_claim_key` rejects them.
    fn free_keys() -> [String; 6] {
        [
            "Release.ServerSteps".to_owned(),
            "release server steps".to_owned(),
            format!("release.{}", "server-steps-".repeat(6)),
            "выпуск.шаги".to_owned(),
            "steps".to_owned(),
            String::new(),
        ]
    }

    #[test]
    fn a_waiting_line_ages_by_position_whatever_its_key_holds() {
        for key in free_keys() {
            assert!(
                !crate::history_summarizer_citations::is_claim_key(&key),
                "{key}"
            );
            let waiting = format!("Waiting on user ({key}): Shared checklist or own file?");
            let asked = row(
                1,
                [
                    &format!("I drafted the steps.\n{waiting}"),
                    &format!("- {waiting}\nSteps drafted."),
                    &waiting,
                    "",
                ],
                vec![],
            );
            let newest = render_rows(std::slice::from_ref(&asked), false);
            for tier in [1, 2, 3] {
                assert!(
                    render_history_segment_at_tier(&newest[0], tier).contains(&waiting),
                    "{key} tier {tier}"
                );
            }
            let followed = render_rows(&[asked, same(2, "Later work.", vec![])], false);
            assert_eq!(
                [1, 2, 3].map(|tier| render_history_segment_at_tier(&followed[0], tier)),
                [
                    "## 10-19 \u{b7} row 1\nI drafted the steps.",
                    "## 10-19 \u{b7} row 1\nSteps drafted.",
                    "## 10-19 \u{b7} row 1",
                ],
                "{key}"
            );
        }
    }

    #[test]
    fn a_settling_line_settles_the_waiting_lines_that_carry_its_exact_key() {
        let waiting = |key: &str| format!("Waiting on user ({key}): Shared checklist or own file?");
        let decided = |key: &str| format!("Decided ({key}): The user chose a separate file.");
        let newest = |tiers: [&str; 4]| rows(&[row(1, tiers, vec![])]).next().unwrap();
        for key in free_keys().into_iter().filter(|key| !key.is_empty()) {
            let p1 = format!("{}\nThe user answered.\n{}", waiting(&key), decided(&key));
            let answered = newest([&p1, &waiting(&key), "", ""]);
            assert_eq!(
                answered.p1.as_deref(),
                Some(format!("The user answered.\n{}", decided(&key)).as_str()),
                "{key}"
            );
            assert_eq!(answered.p2.as_deref(), Some(" "), "{key}");
            let lower = format!("{}\n{}", waiting(&key), decided(&key));
            assert_eq!(
                newest(["Narrative.", &lower, "", ""]).p2.as_deref(),
                Some(decided(&key).as_str()),
                "{key}"
            );
        }
        for (asked, settled) in [
            ("Release.ServerSteps", "release.serversteps"),
            ("release.server-steps", "Release.Server-Steps"),
            ("release server steps", "release  server steps"),
            ("release.server-steps", "release.server-steps-2"),
            ("release.server-steps", " release.server-steps"),
            ("release.server-steps ", "release.server-steps"),
            ("", ""),
            (" ", " "),
        ] {
            let text = format!("{}\n{}", waiting(asked), decided(settled));
            let rendered = newest([&text, &text, "", ""]);
            assert_eq!(
                [rendered.p1.as_deref(), rendered.p2.as_deref()],
                [Some(text.as_str()); 2],
                "{asked:?} {settled:?}"
            );
        }
        let open = waiting("Release Notes");
        let mixed = format!("{WAITING}\n{open}\n{DECIDED}");
        assert_eq!(
            p1s(&[same(1, &mixed, vec![])]),
            [format!("{open}\n{DECIDED}")]
        );
        assert_eq!(
            p1s(&[same(1, &mixed, vec![]), same(2, "Later work.", vec![])])[0],
            DECIDED
        );
    }

    #[test]
    fn a_constraint_whose_key_lies_outside_the_claim_grammar_stands() {
        let set = "Constraint (Policy.MeasurementFixes): No bug fixes during the run.";
        let lifted = "Lifted (Policy.MeasurementFixes): The user wants each bug fixed now.";
        let claims = vec![claim("policy.measurementfixes", Some(lifted))];
        let rendered: Vec<DecayRenderHistorySegment> =
            rows(&[same(1, set, vec![]), same(2, lifted, claims)]).collect();
        for tier in [&rendered[0].p1, &rendered[0].p2, &rendered[0].p3] {
            assert_eq!(tier.as_deref(), Some(set));
        }
        let lifted_in_grammar = "Lifted (policy.measurementfixes): The user wants each bug fixed.";
        let claims = vec![claim("policy.measurementfixes", Some(lifted_in_grammar))];
        assert_eq!(
            p1s(&[same(1, set, vec![]), same(2, lifted_in_grammar, claims)])[0],
            set
        );
        let lifted_in_capitals =
            "Lifted (Policy.Measurement-Fixes): The user wants each bug fixed.";
        let claims = vec![claim("policy.measurement-fixes", Some(lifted_in_capitals))];
        assert_eq!(
            p1s(&[
                same(1, CONSTRAINT, vec![]),
                same(2, lifted_in_capitals, claims)
            ])[0],
            CONSTRAINT
        );
        let spaced = "Constraint ( policy.measurement-fixes): No bug fixes during the run.";
        assert_eq!(
            p1s(&[same(1, spaced, vec![]), same(2, LIFTED, vec![lift()])])[0],
            spaced
        );
        let lifted_spaced = "Lifted ( policy.measurement-fixes): The user wants each bug fixed.";
        let claims = vec![claim("policy.measurement-fixes", Some(lifted_spaced))];
        assert_eq!(
            p1s(&[same(1, CONSTRAINT, vec![]), same(2, lifted_spaced, claims)])[0],
            CONSTRAINT
        );
        let port = |ordinal| Claim {
            ordinal,
            ..claim("db.port", Some("port 5432"))
        };
        let closed = "Constraint (DB Port): Keep port 5432 closed.";
        let rendered = render_rows(
            &[
                same(1, closed, vec![port(1)]),
                same(2, "Port moved.", vec![port(21)]),
            ],
            false,
        );
        assert_eq!(
            render_history_segment_at_tier(&rendered[0], 1),
            format!("## 10-19 \u{b7} row 1\n{closed}\n[corrections: db.port = v @21]")
        );
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
    fn a_lift_holds_while_its_claim_is_the_newest_claim_for_the_key() {
        let set = same(1, &format!("Measured.\n{CONSTRAINT}"), vec![]);
        let lifted = same(2, LIFTED, vec![lift()]);
        let later = |claims| same(3, "The user took the lift back.", claims);
        assert_eq!(
            p1s(&[set.clone(), lifted.clone(), later(vec![])])[0],
            "Measured."
        );
        // A newer claim under the key supersedes the lift, so the constraint renders again.
        let superseding = claim(
            "policy.measurement-fixes",
            Some("The user took the lift back."),
        );
        assert_eq!(
            p1s(&[
                set.clone(),
                lifted.clone(),
                later(vec![superseding.clone()])
            ])[0],
            format!("Measured.\n{CONSTRAINT}")
        );
        // A newer claim with `anchor: None` supersedes the lift too. Validation stores a
        // `Lifted` line's claim that cites an assistant message in this form.
        let restated = later(vec![claim("policy.measurement-fixes", None)]);
        assert_eq!(
            p1s(&[set, lifted, restated])[0],
            format!("Measured.\n{CONSTRAINT}")
        );
        // The same holds for a segment that lifts its own constraint.
        let own = same(
            1,
            &format!("{CONSTRAINT}\nMeasured.\n{LIFTED}"),
            vec![lift()],
        );
        assert_eq!(
            p1s(&[own.clone(), later(vec![])])[0],
            format!("Measured.\n{LIFTED}")
        );
        assert_eq!(
            p1s(&[own, later(vec![superseding])])[0],
            format!("{CONSTRAINT}\nMeasured.\n{LIFTED}")
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
        for lifting in [textless.clone(), same(2, LIFTED, vec![lift()])] {
            assert_eq!(p1s(&[set.clone(), lifting, newest.clone()])[0], "Measured.");
        }
        // A newer row loaded the same way supersedes the lift with another claim under the key.
        let superseding = StoredHistorySegment {
            sequence: 3,
            claims: vec![claim("policy.measurement-fixes", Some("Later work."))],
            ..StoredHistorySegment::default()
        };
        assert_eq!(
            p1s(&[set, textless, superseding, same(4, "Later work.", vec![])])[0],
            format!("Measured.\n{CONSTRAINT}")
        );
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
    fn a_correction_anchored_on_a_retired_line_lists_on_the_footer() {
        // The claim's words stand in the waiting line and again in the narrative after it.
        let asked = "Waiting on user (db.port-move): Move the database off port 5432?";
        let port = |ordinal| Claim {
            ordinal,
            ..claim("db.port", Some("port 5432"))
        };
        let first = same(
            1,
            &format!("{asked}\nThe tests use port 5432."),
            vec![port(1)],
        );
        let rendered = render_rows(&[first, same(2, "Port moved.", vec![port(21)])], false);
        assert_eq!(
            render_history_segment_at_tier(&rendered[0], 1),
            "## 10-19 \u{b7} row 1\nThe tests use port 5432.\n[corrections: db.port = v @21]"
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
    fn a_correction_whose_first_occurrence_moves_into_an_open_line_after_retirement_lists_on_the_footer()
     {
        let heading = "## 10-19 \u{b7} row 1";
        let footer = "[corrections: db.port = v @21]";
        let asked = "Waiting on user (release.step): Proceed?";
        let schema = "Constraint (policy.schema): No edits.";
        // Row 1 stores a claim anchored on `anchor`, and row 2 supersedes it and ages the
        // waiting line of row 1.
        let corrected = |p1: &str, anchor: &str| {
            let port = |ordinal| Claim {
                ordinal,
                ..claim("db.port", Some(anchor))
            };
            let first = row(1, [p1, "", "", ""], vec![port(1)]);
            render_rows(&[first, same(2, "Port moved.", vec![port(21)])], false).remove(0)
        };

        // The stored tier first holds the anchor in its narrative. Once the waiting line
        // leaves, the lines around it meet and hold the anchor across the start of the
        // constraint line.
        let p1 = format!(
            "Setup on 5432.\n{asked}\n{schema}\nSetup on 5432.\nConstraint notes recorded."
        );
        assert_eq!(
            render_history_segment_at_tier(&corrected(&p1, "Setup on 5432.\nConstraint"), 1),
            format!(
                "{heading}\nSetup on 5432.\n{schema}\nSetup on 5432.\nConstraint notes recorded.\n\
                 {footer}"
            )
        );

        // The stored tier first holds the anchor in an indented narrative line. Once the
        // waiting line above it leaves, the renderer trims that indent, and the first
        // occurrence is the one inside the constraint line.
        let p1 = format!("{asked}\n  port 5432 ok.\nConstraint (policy.db): Keep  port 5432");
        assert_eq!(
            render_history_segment_at_tier(&corrected(&p1, "  port 5432"), 1),
            format!("{heading}\nport 5432 ok.\nConstraint (policy.db): Keep  port 5432\n{footer}")
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
        // A line of spaces is blank: the one after the removed line goes with it.
        assert_eq!(
            retire_lines(&format!("First.\n  \n{WAITING}\n  \nSecond."), &aged).as_deref(),
            Some("First.\n  \nSecond.")
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
            owned_rows(segments, &mut Vec::new())
                .map(tiers)
                .collect::<Vec<_>>(),
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

    /// The claim one generated row stores for a key.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Stored {
        None,
        Lift,
        Other,
    }

    /// The tiers of one generated row, and the claim it stores for each key.
    type Generated = ([Vec<Line>; 3], [Stored; 2]);

    /// A stored `p1` has text, so each generated `p1` opens with a narrative line.
    fn generated_row() -> impl Strategy<Value = Generated> {
        (
            [
                prop::collection::vec(line(), 0..5),
                prop::collection::vec(line(), 0..5),
                prop::collection::vec(line(), 0..4),
            ],
            [0usize..3, 0usize..3],
        )
            .prop_map(|(mut tiers, claims)| {
                tiers[0].insert(0, Line::Narrative(9));
                (
                    tiers,
                    claims.map(|claim| [Stored::None, Stored::Lift, Stored::Other][claim]),
                )
            })
    }

    fn stored(sequence: usize, (tiers, stored): &Generated) -> StoredHistorySegment {
        let text = |lines: &Vec<Line>| {
            lines
                .iter()
                .map(|line| line.text())
                .collect::<Vec<_>>()
                .join("\n")
        };
        let [p1, p2, p3] = tiers.each_ref().map(text);
        let claims = (0..2)
            .filter_map(|key| {
                match stored[key] {
                    Stored::None => None,
                    Stored::Lift => Some(Line::Status(Status::Lifted, key).text()),
                    Stored::Other => Some(Line::Narrative(9).text()),
                }
                .map(|anchor| claim(KEYS[key], Some(&anchor)))
            })
            .collect();
        row(sequence as i64 + 1, [&p1, &p2, &p3, ""], claims)
    }

    /// The text lines of `tier` that a line-by-line reading of the module rules keeps.
    fn expected(generated: &[Generated], at: usize, tier: usize) -> Vec<String> {
        let (tiers, stored) = &generated[at];
        let lines = &tiers[tier];
        let last_in_p1 = |key: usize| {
            tiers[0].iter().rev().find_map(|line| match line {
                Line::Status(status, k) if *k == key => Some(*status),
                _ => None,
            })
        };
        // The newest claim a later row stores for the key, the key's live claim, when it is
        // a lift.
        let live_later = |key: usize| {
            generated[at + 1..]
                .iter()
                .rev()
                .map(|(_, stored)| stored[key])
                .find(|stored| *stored != Stored::None)
        };
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
                Line::Status(Status::Constraint, key) => match live_later(*key) {
                    Some(live) => live != Stored::Lift,
                    None => {
                        !(stored[*key] == Stored::Lift && last_in_p1(*key) == Some(Status::Lifted))
                    }
                },
                Line::Blank => false,
                _ => true,
            })
            .map(|(_, line)| line.text())
            .collect()
    }

    #[test]
    fn memchr_lines_splits_each_line_ending_as_str_lines_does() {
        for text in [
            "",
            "\n",
            "\r",
            "a",
            "a\n",
            "a\r",
            "a\r\n",
            "a\r\r\n",
            "a\n\nb",
            "\r\n\r\n",
            "\u{e9}\r\nb\r",
        ] {
            assert_eq!(
                memchr_lines(text).collect::<Vec<_>>(),
                text.lines().collect::<Vec<_>>(),
                "{text:?}"
            );
        }
    }

    proptest! {
        #[test]
        fn memchr_lines_yields_what_str_lines_yields(text in "[a \u{e9}\r\n]{0,24}") {
            prop_assert_eq!(
                memchr_lines(&text).collect::<Vec<_>>(),
                text.lines().collect::<Vec<_>>()
            );
        }

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
                owned_rows(segments, &mut Vec::new()).map(tiers).collect::<Vec<_>>(),
                rendered.into_iter().map(tiers).collect::<Vec<_>>()
            );
        }
    }
}
