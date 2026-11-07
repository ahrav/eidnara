//! Open items in history_summarizer tiers: the status lines a fold carries for questions and
//! decisions that wait on the user, and the pass that retires the ones the fold no longer supports.
//!
//! A status line is one tier line in one of three forms, each naming its item by a claim key:
//!
//! - `Waiting on user (key): …`: a question I put to the user that the folded span leaves open.
//! - `Decided (key): …`: the user's answer to an item, from this span or an earlier fold.
//! - `Lifted (key): …`: a user constraint the user later lifted, stated as lifted.
//!
//! The summarizer resolves each item against the user turns of the folded span. This pass
//! enforces the part of that resolution the fold's own structure proves, so these rules hold for
//! every status line the model writes:
//!
//! - A waiting line is retired when a `Decided` or `Lifted` line with its key appears in the same
//!   segment or a later segment of the same output. The user answered the item or lifted it inside
//!   this span, and the resolving line carries the outcome.
//! - A waiting line is retired from `p3` and `p4`. Those tiers render once a segment has aged
//!   past its recent tiers, and they carry outcomes and decisions.
//!
//! Each status line carries a claim with its key, citing the asking part for a waiting line and
//! the user's message for a resolving line. A later fold that settles an item emits its `Decided`
//! or `Lifted` claim with the same key, and the render-time correction pass supersedes the earlier
//! segment's waiting claim, so an answer that lands in the next span reaches the earlier waiting
//! line as well. That path depends on both claims passing claim validation; a waiting line whose
//! claim was dropped stays as written until its segment ages past `p2`.
//!
//! Every retired line leaves a tier that still holds other text; a tier that would hold nothing
//! else keeps its waiting lines. A claim anchored inside a retired line leaves with it, and a
//! claim whose anchor the retirement removed from `p1` keeps its value without an anchor. The
//! pass removes lines and claims and never adds any, so the stored summary is bounded by what the
//! model emitted.

use std::collections::BTreeMap;

use crate::history_summarizer_citations::is_claim_key;
use crate::history_summarizer_validate::ValidatedHistorySegment;

/// The prefix of a status line for a question that waits on the user.
pub(crate) const WAITING_PREFIX: &str = "Waiting on user (";
/// The prefixes of status lines that settle an item.
pub(crate) const RESOLVING_PREFIXES: [&str; 2] = ["Decided (", "Lifted ("];

/// What one status line says about its item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Waiting,
    Resolved,
}

/// The status and key of `line` when it is a status line whose key follows the claim-key grammar.
fn status_line(line: &str) -> Option<(Status, &str)> {
    let line = line.trim_start();
    let (status, rest) = match line.strip_prefix(WAITING_PREFIX) {
        Some(rest) => (Status::Waiting, rest),
        None => (
            Status::Resolved,
            RESOLVING_PREFIXES
                .iter()
                .find_map(|prefix| line.strip_prefix(prefix))?,
        ),
    };
    let (key, _) = rest.split_once("):")?;
    is_claim_key(key).then_some((status, key))
}

/// The key of `line` when it is a waiting status line.
fn waiting_key(line: &str) -> Option<&str> {
    status_line(line).and_then(|(status, key)| (status == Status::Waiting).then_some(key))
}

/// What [`retire_waiting_lines`] removed from the claims, so the claims outcome counts stay true.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Retired {
    /// Claims removed because their anchor sat inside a retired waiting line.
    pub claims: usize,
    /// Claims kept without an anchor because the retirement removed their anchor from `p1`.
    pub anchors: usize,
}

/// Retires the waiting lines the output no longer supports, per the module rules, in place.
pub(crate) fn retire_waiting_lines(segments: &mut [ValidatedHistorySegment]) -> Retired {
    // Each key's last segment holding a resolving line; ascending order leaves the last index.
    let mut resolved_through: BTreeMap<String, usize> = BTreeMap::new();
    for (index, segment) in segments.iter().enumerate() {
        for tier in tiers(segment) {
            for (status, key) in tier.lines().filter_map(status_line) {
                if status == Status::Resolved {
                    resolved_through.insert(key.to_owned(), index);
                }
            }
        }
    }
    let mut retired = Retired::default();
    for (index, segment) in segments.iter_mut().enumerate() {
        let settled = |key: &str| resolved_through.get(key).is_some_and(|&last| last >= index);
        let dropped_from_p1 = match segment.p1.as_mut() {
            Some(p1) => retire_in(p1, settled),
            None => retire_in(&mut segment.content, settled),
        };
        if let Some(p2) = segment.p2.as_mut() {
            retire_in(p2, settled);
        }
        for tier in [&mut segment.p3, &mut segment.p4].into_iter().flatten() {
            retire_in(tier, |_| true);
        }
        if dropped_from_p1.is_empty() {
            continue;
        }
        let Some(p1) = segment.p1.as_deref() else {
            continue;
        };
        segment.content = p1.to_owned();
        segment.claims.retain_mut(|claim| {
            let Some(anchor) = claim.anchor.as_deref() else {
                return true;
            };
            if p1.contains(anchor) {
                return true;
            }
            if dropped_from_p1.iter().any(|line| line.contains(anchor)) {
                retired.claims += 1;
                return false;
            }
            retired.anchors += 1;
            claim.anchor = None;
            true
        });
    }
    retired
}

fn tiers(segment: &ValidatedHistorySegment) -> impl Iterator<Item = &str> {
    [
        segment.p1.as_deref(),
        segment.p2.as_deref(),
        segment.p3.as_deref(),
        segment.p4.as_deref(),
    ]
    .into_iter()
    .flatten()
    .chain(segment.p1.is_none().then_some(segment.content.as_str()))
}

/// Removes from `text` the waiting lines whose key `retire` accepts and returns them. A removal
/// that would leave no other text leaves `text` unchanged and returns nothing. A blank line that
/// would follow a blank line or open the text after a removal goes with it.
fn retire_in(text: &mut String, retire: impl Fn(&str) -> bool) -> Vec<String> {
    let mut kept: Vec<&str> = Vec::new();
    let mut dropped = Vec::new();
    for line in text.lines() {
        if waiting_key(line).is_some_and(&retire) {
            dropped.push(line.to_owned());
            continue;
        }
        let blank = line.trim().is_empty();
        if blank && !dropped.is_empty() && kept.last().is_none_or(|last| last.trim().is_empty()) {
            continue;
        }
        kept.push(line);
    }
    let rest = kept.join("\n").trim().to_owned();
    if dropped.is_empty() || rest.is_empty() {
        return Vec::new();
    }
    *text = rest;
    dropped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(
        p1: &str,
        p2: &str,
        p3: &str,
        p4: &str,
        claim_keys: &[&str],
    ) -> ValidatedHistorySegment {
        ValidatedHistorySegment {
            sequence: 0,
            start_message: 1,
            end_message: 4,
            start_message_id: "m1#0".into(),
            end_message_id: "m4#0".into(),
            title: "t".into(),
            content: p1.into(),
            p1: Some(p1.into()),
            p2: Some(p2.into()),
            p3: Some(p3.into()),
            p4: Some(p4.into()),
            importance: Some(50),
            episode_type: None,
            claims: claim_keys
                .iter()
                .map(|key| memory_store::Claim {
                    key: (*key).into(),
                    value: "v".into(),
                    ordinal: 2,
                    anchor: None,
                })
                .collect(),
        }
    }

    #[test]
    fn a_status_line_needs_its_prefix_a_claim_key_and_the_closing_colon() {
        assert_eq!(
            status_line("Waiting on user (release.order): Ship the CLI first?"),
            Some((Status::Waiting, "release.order"))
        );
        assert_eq!(
            status_line("  Decided (release.order): CLI first."),
            Some((Status::Resolved, "release.order"))
        );
        assert_eq!(
            status_line("Lifted (policy.bug-fixes): no fixes during measurement, lifted."),
            Some((Status::Resolved, "policy.bug-fixes"))
        );
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
    fn retiring_a_line_leaves_the_surrounding_paragraphs_single_spaced() {
        let original = "First paragraph.\n\nWaiting on user (a.b): ask?\n\nSecond paragraph.";
        let mut text = original.to_owned();
        assert_eq!(retire_in(&mut text, |_| false), Vec::<String>::new());
        assert_eq!(text, original);
        assert_eq!(
            retire_in(&mut text, |_| true),
            ["Waiting on user (a.b): ask?"]
        );
        assert_eq!(text, "First paragraph.\n\nSecond paragraph.");
    }

    #[test]
    fn a_tier_made_only_of_a_waiting_line_keeps_it() {
        let mut segments = [segment(
            "Waiting on user (a.b): ask?",
            "Waiting on user (a.b): ask?",
            "Waiting on user (a.b): ask?",
            "",
            &[],
        )];
        assert_eq!(retire_waiting_lines(&mut segments), Retired::default());
        assert_eq!(
            segments[0].p1.as_deref(),
            Some("Waiting on user (a.b): ask?")
        );
        assert_eq!(
            segments[0].p3.as_deref(),
            Some("Waiting on user (a.b): ask?")
        );
    }

    #[test]
    fn a_resolution_in_a_later_segment_retires_the_earlier_waiting_line() {
        let waiting = "I drafted the steps.\nWaiting on user (a.b): ask?";
        let mut segments = [
            segment(waiting, waiting, "Drafted.", "", &["a.b"]),
            segment(
                "Decided (a.b): yes.\nDone.",
                "Decided (a.b): yes.",
                "Done.",
                "",
                &["a.b"],
            ),
            segment(waiting, waiting, "Drafted.", "", &["a.b"]),
        ];
        retire_waiting_lines(&mut segments);
        assert_eq!(segments[0].p1.as_deref(), Some("I drafted the steps."));
        assert_eq!(segments[0].content, "I drafted the steps.");
        assert_eq!(segments[0].p2.as_deref(), Some("I drafted the steps."));
        // A question asked again after the resolution stays open.
        assert_eq!(segments[2].p1.as_deref(), Some(waiting));
    }

    #[test]
    fn a_legacy_row_retires_its_settled_waiting_line_from_content() {
        let mut segments = [segment("", "", "", "", &[])];
        segments[0].p1 = None;
        segments[0].p2 = None;
        segments[0].p3 = None;
        segments[0].p4 = None;
        segments[0].content = "Worked.\nWaiting on user (a.b): ask?\nDecided (a.b): yes.".into();
        retire_waiting_lines(&mut segments);
        assert_eq!(segments[0].content, "Worked.\nDecided (a.b): yes.");
        assert_eq!(segments[0].p1, None);
    }

    #[test]
    fn claims_anchored_in_a_retired_line_leave_and_others_lose_only_a_lost_anchor() {
        let p1 = "Worked on a.b.\nWaiting on user (a.b): ask now?\nDecided (a.b): yes.";
        let mut segments = [segment(p1, p1, "Done.", "", &["a.b", "c.d", "e.f"])];
        segments[0].claims[0].anchor = Some("Waiting on user (a.b): ask now?".into());
        segments[0].claims[1].anchor = Some("ask now?\nDecided".into());
        segments[0].claims[2].anchor = Some("Decided (a.b): yes.".into());
        assert_eq!(
            retire_waiting_lines(&mut segments),
            Retired {
                claims: 1,
                anchors: 1
            }
        );
        let claims: Vec<_> = segments[0]
            .claims
            .iter()
            .map(|c| (c.key.as_str(), c.anchor.as_deref()))
            .collect();
        assert_eq!(
            claims,
            [("c.d", None), ("e.f", Some("Decided (a.b): yes."))]
        );
    }
}
