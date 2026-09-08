//! Daemon-internal reader that composes injectable project memory from canonical kernel rows.
//!
//! The transform and the historian read through this module. A pass reads once
//! and composes every memory surface of that pass from the same pinned rows.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use kernel::{Surface, SurfaceVisibility};
use memory_store::ProjectMemoryComposition;

use crate::kernel_routes::read::{ReadResponse, read_visible};
use crate::kernel_routes::{KernelOpenCoordinator, KernelOutcome, ProjectBinding, serving};
use crate::memory_render::is_positive_memory_category;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalMemory {
    pub object_id: String,
    /// The decision kind, which memory writers use as the category.
    pub category: String,
    /// The decision summary.
    pub content: String,
}

/// Injectable rows visible at one kernel snapshot, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalMemorySnapshot {
    pub known_as_of: i64,
    /// Whether the visible-row read dropped rows past its caps.
    pub truncated: bool,
    pub rows: Vec<CanonicalMemory>,
}

impl CanonicalMemorySnapshot {
    /// Digest over every input the block renders from, so two snapshots with
    /// equal digests render identical bytes. Rows are digested in object-id
    /// order, so the digest is a function of the row set and not of the order
    /// the read returned it in.
    pub fn revision(&self) -> u64 {
        let mut rows: Vec<&CanonicalMemory> = self.rows.iter().collect();
        rows.sort_by(|left, right| left.object_id.cmp(&right.object_id));
        let mut hasher = DefaultHasher::new();
        "eidnara-project-memory-revision-v1".hash(&mut hasher);
        self.truncated.hash(&mut hasher);
        for row in rows {
            row.object_id.hash(&mut hasher);
            row.category.hash(&mut hasher);
            row.content.hash(&mut hasher);
        }
        hasher.finish()
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
                revision: snapshot.revision(),
            },
            Self::Withheld(verdict) => ProjectMemoryComposition::Withheld {
                state: verdict.state_key(),
            },
        }
    }

    /// The row digest the m1 revision signal folds in; `None` when withheld.
    pub fn revision(&self) -> Option<u64> {
        match self {
            Self::Available(snapshot) => Some(snapshot.revision()),
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

/// Reads the project's injectable memory at the kernel tip.
///
/// The store phase, the serving decision for a tip read on the `auto_inject`
/// surface, and the visible-row read each withhold the composition with the
/// `KernelOutcome` the `kernel.read` route would answer with.
pub(crate) fn read_project_memory(
    kernel: &KernelOpenCoordinator,
    project: &ProjectBinding,
    now_ms: i64,
) -> CanonicalMemoryRead {
    let store = match kernel.kernel_store() {
        Ok(store) => store,
        Err(outcome) => return CanonicalMemoryRead::Withheld(outcome),
    };
    let lag = match store.outbox_lag(now_ms) {
        Ok(lag) => lag,
        Err(error) => return CanonicalMemoryRead::Withheld(KernelOutcome::from(error)),
    };
    let verdict = serving::project(serving::decide_for_tip_read(&lag), Surface::AutoInject);
    if !verdict.is_available() {
        return CanonicalMemoryRead::Withheld(verdict);
    }
    match read_visible(&store, project, Surface::AutoInject, None, None) {
        Ok(response) => CanonicalMemoryRead::Available(injectable_snapshot(response)),
        Err(error) => CanonicalMemoryRead::Withheld(KernelOutcome::from(error)),
    }
}

/// Restates the domain filter of the retired claim mirror on canonical rows:
/// a row is injectable when it is live (the visible-row read already keeps only
/// live rows), resolves visible on the surface, and carries a decision whose
/// kind is a positive memory category. Labeled rows never reach `auto_inject`;
/// the guard keeps a label-bearing row out of a block that has no label
/// renderer.
fn injectable_snapshot(response: ReadResponse) -> CanonicalMemorySnapshot {
    let ReadResponse {
        known_as_of,
        rows,
        truncated,
        decisions,
        ..
    } = response;
    let rows = rows
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
    CanonicalMemorySnapshot {
        known_as_of,
        truncated,
        rows,
    }
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
                domain_id: "memory".to_string(),
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

    #[test]
    fn injectable_rows_are_visible_decisions_in_a_positive_category() {
        let snapshot = injectable_snapshot(response(
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
        ));
        assert_eq!(snapshot.known_as_of, 7);
        assert_eq!(
            snapshot.rows,
            vec![CanonicalMemory {
                object_id: "rule".to_string(),
                category: "PROJECT_RULES".to_string(),
                content: "Keep the public contract.".to_string(),
            }]
        );
    }

    #[test]
    fn revision_changes_only_when_the_rendered_inputs_change() {
        let base = injectable_snapshot(response(
            vec![visible_row("rule", "decision", SurfaceVisibility::Visible)],
            vec![decision(
                "rule",
                "PROJECT_RULES",
                "Keep the public contract.",
            )],
        ));
        let same_rows_later_snapshot = CanonicalMemorySnapshot {
            known_as_of: 99,
            ..base.clone()
        };
        assert_eq!(base.revision(), same_rows_later_snapshot.revision());
        let two = injectable_snapshot(response(
            vec![
                visible_row("rule", "decision", SurfaceVisibility::Visible),
                visible_row("other", "decision", SurfaceVisibility::Visible),
            ],
            vec![
                decision("rule", "PROJECT_RULES", "Keep the public contract."),
                decision("other", "NAMING", "Name things."),
            ],
        ));
        let mut reversed = two.clone();
        reversed.rows.reverse();
        assert_eq!(two.revision(), reversed.revision());
        assert_ne!(two.revision(), base.revision());
        let edited = injectable_snapshot(response(
            vec![visible_row("rule", "decision", SurfaceVisibility::Visible)],
            vec![decision(
                "rule",
                "PROJECT_RULES",
                "Keep the private contract.",
            )],
        ));
        assert_ne!(base.revision(), edited.revision());
        let truncated = CanonicalMemorySnapshot {
            truncated: true,
            ..base.clone()
        };
        assert_ne!(base.revision(), truncated.revision());
        let empty = injectable_snapshot(response(Vec::new(), Vec::new()));
        assert_ne!(base.revision(), empty.revision());
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
        let empty =
            CanonicalMemoryRead::Available(injectable_snapshot(response(Vec::new(), Vec::new())));
        assert!(empty.rows().is_empty());
        assert_ne!(withheld.composition(), empty.composition());
    }
}
