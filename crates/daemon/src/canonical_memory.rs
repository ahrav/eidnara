//! Daemon-internal reader that composes injectable project memory from canonical kernel rows.
//!
//! The transform and the historian read through this module. A pass reads once
//! and composes every memory surface of that pass from the same pinned rows.
//! The reader trims the rows to the pass's memory budget, so a snapshot holds
//! exactly the rows the `<project-memory>` block renders.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use kernel::{Surface, SurfaceVisibility};
use memory_store::ProjectMemoryComposition;

use crate::kernel_routes::read::{ReadResponse, RowSelection, read_visible};
use crate::kernel_routes::{KernelOpenCoordinator, KernelOutcome, ProjectBinding, serving};
use crate::m0_compose::trim_memories_to_budget;
use crate::memory_render::{is_positive_memory_category, render_memory_line};

pub(crate) const MEMORY_DOMAIN_ID: &str = "memory";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalMemory {
    pub object_id: String,
    /// The decision kind, which memory writers use as the category.
    pub category: String,
    /// The decision summary.
    pub content: String,
}

/// Injectable rows visible at one kernel snapshot, newest first, with the
/// digest of the block they render computed once at construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalMemorySnapshot {
    known_as_of: i64,
    truncated: bool,
    rows: Vec<CanonicalMemory>,
    revision: u64,
}

impl CanonicalMemorySnapshot {
    /// Builds the snapshot and digests the rows the block renders from.
    ///
    /// Rows are digested in object-id order after the renderer's
    /// positive-category filter, so the digest is a function of the rendered
    /// row set and not of the order the read returned it in or of rows the
    /// renderer drops. Each row contributes its category and the exact line
    /// `render_memory_line` emits, so an edit past the renderer's content cap
    /// changes no digested byte. `truncated` is not digested: it does not
    /// change the rendered bytes.
    pub fn new(known_as_of: i64, truncated: bool, rows: Vec<CanonicalMemory>) -> Self {
        let mut rendered: Vec<&CanonicalMemory> = rows
            .iter()
            .filter(|row| is_positive_memory_category(&row.category))
            .collect();
        rendered.sort_by(|left, right| left.object_id.cmp(&right.object_id));
        let mut hasher = DefaultHasher::new();
        "eidnara-project-memory-revision-v1".hash(&mut hasher);
        for row in rendered {
            row.category.hash(&mut hasher);
            render_memory_line(row).hash(&mut hasher);
        }
        Self {
            known_as_of,
            truncated,
            rows,
            revision: hasher.finish(),
        }
    }

    /// The kernel commit sequence the rows were read at.
    pub fn known_as_of(&self) -> i64 {
        self.known_as_of
    }

    /// Whether the visible-row read dropped rows past its caps.
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    pub fn rows(&self) -> &[CanonicalMemory] {
        &self.rows
    }

    /// Digest over the rendered inputs: two snapshots with equal digests
    /// render identical block bytes.
    pub fn revision(&self) -> u64 {
        self.revision
    }
}

/// The outcome of one canonical read for a pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalMemoryRead {
    Available(CanonicalMemorySnapshot),
    /// The read was not served: no rows are injected and the verdict is recorded.
    Withheld(KernelOutcome),
}

impl CanonicalMemoryRead {
    /// The durable record of this read for the meta of the pass that consumed it.
    pub fn composition(&self) -> ProjectMemoryComposition {
        match self {
            Self::Available(snapshot) => ProjectMemoryComposition::Canonical {
                known_as_of: snapshot.known_as_of,
                truncated: snapshot.truncated,
                revision: snapshot.revision,
            },
            Self::Withheld(verdict) => ProjectMemoryComposition::Withheld {
                state: verdict.state_key(),
            },
        }
    }

    /// The row digest the m1 revision signal folds in; `None` when withheld.
    pub fn revision(&self) -> Option<u64> {
        match self {
            Self::Available(snapshot) => Some(snapshot.revision),
            Self::Withheld(_) => None,
        }
    }

    /// The injectable rows; empty when the read was withheld.
    pub fn rows(&self) -> &[CanonicalMemory] {
        match self {
            Self::Available(snapshot) => &snapshot.rows,
            Self::Withheld(_) => &[],
        }
    }
}

/// Reads the project's injectable memory at the kernel tip, trimmed to
/// `memory_budget_tokens`.
///
/// The tip is captured before the lag sample, and the visible-row read is bound to it, so a commit published between them cannot admit rows the freshness verdict did not cover.
///
/// The read admits only memory-domain decisions before the row and byte caps apply, so a project whose other domains or observations are busy cannot crowd its memory rows out of the bounded read. commentlint: allow(JUDGE)
///
/// The store phase, the serving decision for a tip read on the `auto_inject`
/// surface, and the visible-row read each withhold the composition with the
/// `KernelOutcome` the `kernel.read` route would answer with.
pub(crate) fn read_project_memory(
    kernel: &KernelOpenCoordinator,
    project: &ProjectBinding,
    now_ms: i64,
    memory_budget_tokens: f64,
) -> CanonicalMemoryRead {
    let store = match kernel.kernel_store() {
        Ok(store) => store,
        Err(outcome) => return CanonicalMemoryRead::Withheld(outcome),
    };
    let tip = match store.tip() {
        Ok(tip) => tip,
        Err(error) => return CanonicalMemoryRead::Withheld(KernelOutcome::from(error)),
    };
    let lag = match store.outbox_lag(now_ms) {
        Ok(lag) => lag,
        Err(error) => return CanonicalMemoryRead::Withheld(KernelOutcome::from(error)),
    };
    let verdict = serving::project(serving::decide_for_tip_read(&lag), Surface::AutoInject);
    if !verdict.is_available() {
        return CanonicalMemoryRead::Withheld(verdict);
    }
    match read_visible(
        &store,
        project,
        Surface::AutoInject,
        Some(tip),
        RowSelection::DomainDecisions(MEMORY_DOMAIN_ID),
    ) {
        Ok(response) => {
            CanonicalMemoryRead::Available(injectable_snapshot(response, memory_budget_tokens))
        }
        Err(error) => CanonicalMemoryRead::Withheld(KernelOutcome::from(error)),
    }
}

/// The domain filter the injectable memory surface applies to canonical rows:
/// a row is injectable when it is live (the visible-row read already keeps only
/// live rows), resolves visible on the surface, and carries a decision whose
/// kind is a positive memory category. Labeled rows never reach `auto_inject`;
/// the guard keeps a label-bearing row out of a block that has no label
/// renderer. The injectable rows are then trimmed to `memory_budget_tokens` in
/// serving order, so the snapshot holds only rows the block renders.
fn injectable_snapshot(
    response: ReadResponse,
    memory_budget_tokens: f64,
) -> CanonicalMemorySnapshot {
    let ReadResponse {
        known_as_of,
        rows,
        truncated,
        decisions,
        ..
    } = response;
    let injectable: Vec<CanonicalMemory> = rows
        .iter()
        .filter(|row| row.visibility == SurfaceVisibility::Visible)
        .filter_map(|row| decisions.get(&row.object.object_id))
        .filter(|decision| is_positive_memory_category(&decision.decision_kind))
        .map(|decision| CanonicalMemory {
            object_id: decision.object_id.clone(),
            category: decision.decision_kind.clone(),
            content: decision.payload.summary.clone(),
        })
        .collect();
    let rows = trim_memories_to_budget(
        &injectable,
        memory_budget_tokens,
        crate::token_cache::cached_estimate_tokens,
    );
    CanonicalMemorySnapshot::new(known_as_of, truncated, rows)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use kernel::{DecisionPayload, DecisionRow, ObjectRow, Sensitivity, VisibleRow};

    use super::*;
    use crate::kernel_routes::UnavailableReason;

    fn visible_row(
        object_id: &str,
        object_kind: &str,
        visibility: SurfaceVisibility,
    ) -> VisibleRow {
        VisibleRow {
            object: ObjectRow {
                object_id: object_id.to_string(),
                object_kind: object_kind.to_string(),
                domain_id: MEMORY_DOMAIN_ID.to_string(),
                source_kind: "assistant".to_string(),
                source_id: "lineage".to_string(),
                source_revision: 1,
                created_commit_seq: 1,
                invalidated_commit_seq: None,
                superseded_by: None,
                sensitivity: Sensitivity::Normal,
            },
            visibility,
            labeled: visibility == SurfaceVisibility::Labeled,
            scope_id: None,
        }
    }

    fn decision(object_id: &str, kind: &str, summary: &str) -> DecisionRow {
        DecisionRow {
            decision_id: format!("{object_id}-decision"),
            object_id: object_id.to_string(),
            proposition_id: None,
            scope_id: None,
            anchor_id: None,
            evidence_id: None,
            decision_kind: kind.to_string(),
            payload: DecisionPayload {
                summary: summary.to_string(),
                rationale: String::new(),
            },
            created_commit_seq: 1,
            sensitivity: Sensitivity::Normal,
        }
    }

    fn response(rows: Vec<VisibleRow>, decisions: Vec<DecisionRow>) -> ReadResponse {
        ReadResponse {
            known_as_of: 7,
            tip: 7,
            rows,
            truncated: false,
            decisions: decisions
                .into_iter()
                .map(|decision| (decision.object_id.clone(), decision))
                .collect::<HashMap<_, _>>(),
        }
    }

    /// A budget no realistic block reaches, so trimming admits every row.
    const UNBOUNDED: f64 = 1_000_000.0;

    #[test]
    fn injectable_rows_are_visible_decisions_in_a_positive_category() {
        let snapshot = injectable_snapshot(
            response(
                vec![
                    visible_row("rule", "decision", SurfaceVisibility::Visible),
                    visible_row("anti", "decision", SurfaceVisibility::Visible),
                    visible_row("labeled", "decision", SurfaceVisibility::Labeled),
                    visible_row("observation", "observation", SurfaceVisibility::Visible),
                ],
                vec![
                    decision("rule", "PROJECT_RULES", "Keep the public contract."),
                    decision(
                        "anti",
                        "REJECTED_APPROACH",
                        "Do not resurrect the shelved design.",
                    ),
                    decision("labeled", "PROJECT_RULES", "Needs a label."),
                ],
            ),
            UNBOUNDED,
        );
        assert_eq!(snapshot.known_as_of(), 7);
        assert_eq!(
            snapshot.rows(),
            &[CanonicalMemory {
                object_id: "rule".to_string(),
                category: "PROJECT_RULES".to_string(),
                content: "Keep the public contract.".to_string(),
            }]
        );
    }

    #[test]
    fn revision_changes_only_when_the_rendered_inputs_change() {
        let base = injectable_snapshot(
            response(
                vec![visible_row("rule", "decision", SurfaceVisibility::Visible)],
                vec![decision(
                    "rule",
                    "PROJECT_RULES",
                    "Keep the public contract.",
                )],
            ),
            UNBOUNDED,
        );
        let same_rows_later_snapshot =
            CanonicalMemorySnapshot::new(99, base.truncated(), base.rows().to_vec());
        assert_eq!(base.revision(), same_rows_later_snapshot.revision());
        let two = injectable_snapshot(
            response(
                vec![
                    visible_row("rule", "decision", SurfaceVisibility::Visible),
                    visible_row("other", "decision", SurfaceVisibility::Visible),
                ],
                vec![
                    decision("rule", "PROJECT_RULES", "Keep the public contract."),
                    decision("other", "NAMING", "Name things."),
                ],
            ),
            UNBOUNDED,
        );
        let mut reversed_rows = two.rows().to_vec();
        reversed_rows.reverse();
        let reversed =
            CanonicalMemorySnapshot::new(two.known_as_of(), two.truncated(), reversed_rows);
        assert_eq!(two.revision(), reversed.revision());
        assert_ne!(two.revision(), base.revision());
        let edited = injectable_snapshot(
            response(
                vec![visible_row("rule", "decision", SurfaceVisibility::Visible)],
                vec![decision(
                    "rule",
                    "PROJECT_RULES",
                    "Keep the private contract.",
                )],
            ),
            UNBOUNDED,
        );
        assert_ne!(base.revision(), edited.revision());
        let truncated =
            CanonicalMemorySnapshot::new(base.known_as_of(), true, base.rows().to_vec());
        assert_eq!(
            base.revision(),
            truncated.revision(),
            "the read cap does not change the rendered bytes, so it must not move the revision"
        );
        let empty = injectable_snapshot(response(Vec::new(), Vec::new()), UNBOUNDED);
        assert_ne!(base.revision(), empty.revision());
    }

    #[test]
    fn rows_the_renderer_drops_do_not_move_the_revision() {
        let rule = CanonicalMemory {
            object_id: "rule".to_string(),
            category: "PROJECT_RULES".to_string(),
            content: "Keep the public contract.".to_string(),
        };
        let anti = CanonicalMemory {
            object_id: "anti".to_string(),
            category: "REJECTED_APPROACH".to_string(),
            content: "Shelved design.".to_string(),
        };
        let only_rule = CanonicalMemorySnapshot::new(1, false, vec![rule.clone()]);
        let with_dropped = CanonicalMemorySnapshot::new(1, false, vec![rule, anti]);
        assert_eq!(only_rule.revision(), with_dropped.revision());
    }

    #[test]
    fn bytes_past_the_render_cap_do_not_move_the_revision() {
        const CAP: usize = 64 * 1024;
        let row = |content: String| CanonicalMemory {
            object_id: "long".to_string(),
            category: "PROJECT_RULES".to_string(),
            content,
        };
        let base = "a".repeat(CAP);
        let same_prefix = CanonicalMemorySnapshot::new(1, false, vec![row(base.clone())]);
        let edited_past_cap =
            CanonicalMemorySnapshot::new(1, false, vec![row(format!("{base} trailing edit"))]);
        assert_eq!(
            same_prefix.revision(),
            edited_past_cap.revision(),
            "the renderer never emits bytes past the cap, so they must not fold m0"
        );
        let edited_inside_cap =
            CanonicalMemorySnapshot::new(1, false, vec![row(format!("b{}", &base[1..]))]);
        assert_ne!(same_prefix.revision(), edited_inside_cap.revision());
    }

    #[test]
    fn rows_past_the_memory_budget_are_neither_injected_nor_digested() {
        let short = decision("short", "PROJECT_RULES", "Keep the public contract.");
        let long_summary = "x ".repeat(2_000);
        let long = decision("long", "PROJECT_RULES", &long_summary);
        // Serving order is newest first; both rows are candidates, but only the
        // short one fits a budget of a few dozen tokens.
        let rows = || {
            vec![
                visible_row("long", "decision", SurfaceVisibility::Visible),
                visible_row("short", "decision", SurfaceVisibility::Visible),
            ]
        };
        let budget = 60.0;
        let trimmed = injectable_snapshot(response(rows(), vec![long, short.clone()]), budget);
        assert_eq!(
            trimmed
                .rows()
                .iter()
                .map(|row| row.object_id.as_str())
                .collect::<Vec<_>>(),
            vec!["short"],
            "the oversized row is dropped by the budget"
        );
        let only_short = injectable_snapshot(
            response(
                vec![visible_row("short", "decision", SurfaceVisibility::Visible)],
                vec![short.clone()],
            ),
            budget,
        );
        assert_eq!(
            trimmed.revision(),
            only_short.revision(),
            "a row the budget excludes must not be digested"
        );
        let edited_long = decision("long", "PROJECT_RULES", &format!("{long_summary} edited"));
        let edited = injectable_snapshot(response(rows(), vec![edited_long, short]), budget);
        assert_eq!(
            trimmed.revision(),
            edited.revision(),
            "editing a row outside the budget does not change the block, so it must not fold m0"
        );
    }

    #[test]
    fn withheld_reads_record_the_verdict_and_inject_nothing() {
        let withheld = CanonicalMemoryRead::Withheld(KernelOutcome::unavailable(
            UnavailableReason::StoreStarting,
        ));
        assert!(withheld.rows().is_empty());
        assert_eq!(
            withheld.composition(),
            ProjectMemoryComposition::Withheld {
                state: "unavailable:store_starting".to_string()
            }
        );
        let empty = CanonicalMemoryRead::Available(injectable_snapshot(
            response(Vec::new(), Vec::new()),
            UNBOUNDED,
        ));
        assert!(empty.rows().is_empty());
        assert_ne!(withheld.composition(), empty.composition());
    }
}
