//! Daemon-internal reader that composes injectable project memory from canonical kernel rows.
//!
//! The transform and the history_summarizer read through this module. A pass reads once
//! and composes every memory surface of that pass from the same pinned rows.
//! The reader trims the rows to the pass's memory budget, so a snapshot holds
//! exactly the rows the `<project-memory>` block renders.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use kernel::{Surface, SurfaceVisibility};
use memory_store::ProjectMemoryComposition;

use crate::kernel_routes::read::{ReadResponse, RowSelection, read_visible};
use crate::kernel_routes::{KernelOpenCoordinator, KernelOutcome, ProjectBinding};
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
    /// The served-set digest of [`Self::served_ids`]: the identity a use-outcome record of this composition carries.
    served_set_digest: String,
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
        let rendered = rendered_rows(&rows);
        let mut hasher = DefaultHasher::new();
        "eidnara-project-memory-revision-v1".hash(&mut hasher);
        for row in &rendered {
            row.category.hash(&mut hasher);
            render_memory_line(row).hash(&mut hasher);
        }
        let served_set_digest =
            kernel::served_set_digest(rendered.iter().map(|row| row.object_id.as_str()));
        Self {
            known_as_of,
            truncated,
            rows,
            revision: hasher.finish(),
            served_set_digest,
        }
    }

    /// The decision object ids the block renders, ascending and distinct.
    pub fn served_ids(&self) -> Vec<&str> {
        let mut ids: Vec<&str> = rendered_rows(&self.rows)
            .into_iter()
            .map(|row| row.object_id.as_str())
            .collect();
        ids.dedup();
        ids
    }

    /// The domain-separated SHA-256 over [`Self::served_ids`]: equal digests name one served id set whatever bytes rendered it.
    pub fn served_set_digest(&self) -> &str {
        &self.served_set_digest
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

    /// What a use-outcome serve records of this snapshot, owned, so a write can outlive the pass without its rows.
    pub fn served_set(&self) -> ServedSet {
        ServedSet {
            ids: self.served_ids().into_iter().map(str::to_string).collect(),
            digest: self.served_set_digest.clone(),
            revision: self.revision,
            known_as_of: self.known_as_of,
        }
    }
}

/// The identity a serve record carries: the rendered decision ids, ascending and distinct, their digest, the
/// revision, and the commit sequence the rows were read at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServedSet {
    ids: Vec<String>,
    digest: String,
    revision: u64,
    known_as_of: i64,
}

impl ServedSet {
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.ids.iter().map(String::as_str)
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn known_as_of(&self) -> i64 {
        self.known_as_of
    }
}

/// The rows the block renders, in object-id order; the revision and the served-set digest derive from them.
fn rendered_rows(rows: &[CanonicalMemory]) -> Vec<&CanonicalMemory> {
    let mut rendered: Vec<&CanonicalMemory> = rows
        .iter()
        .filter(|row| is_positive_memory_category(&row.category))
        .collect();
    rendered.sort_by(|left, right| left.object_id.cmp(&right.object_id));
    rendered
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
/// The visible-row read is bound to the tip captured first, so the rows a served read pins are the canonical rows at that commit.
///
/// The read admits only memory-domain decisions before the row and byte caps apply, so a project whose other domains or observations are busy cannot crowd its memory rows out of the bounded read.
///
/// The rows come from the canonical tables at the pinned tip, and outbox consumers maintain derived state, so the read is served at any consumer lag. The report marks a served read with the lag facts of the kernel health block the coordinator last published, while that block is fresh, and logs a read served while a consumer trails past the serving thresholds. Store-access, tip-read, and visible-row-read failures withhold the composition with the corresponding `KernelOutcome`. Each change of verdict logs one line.
pub(crate) fn read_project_memory(
    kernel: &KernelOpenCoordinator,
    project: &ProjectBinding,
    memory_budget_tokens: f64,
) -> CanonicalMemoryRead {
    let read = read_at_tip(kernel, project, memory_budget_tokens);
    let published = kernel.health_block();
    let facts = (!published.is_stale())
        .then_some(published.block.facts.as_ref())
        .flatten();
    if let Some(line) = kernel.memory_read_report().observe(observed(&read, facts)) {
        eprintln!("daemon: project memory {line}");
    }
    read
}

fn read_at_tip(
    kernel: &KernelOpenCoordinator,
    project: &ProjectBinding,
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

/// The verdict the report compares for `read`, marked with `facts`, the lag facts of a fresh health block; a served read without them is unsampled.
fn observed(
    read: &CanonicalMemoryRead,
    facts: Option<&crate::kernel_routes::health::KernelFactsBlock>,
) -> Observed {
    match (read, facts) {
        (CanonicalMemoryRead::Withheld(verdict), _) => Observed::Withheld(verdict.state_key()),
        (CanonicalMemoryRead::Available(_), Some(facts)) if facts.lag_threshold_tripped => {
            Observed::ServedWhileDerivedLag {
                consumers: facts.required_consumer_count,
                lag_positions: facts.outbox_position_lag.unwrap_or(0),
                oldest_unconsumed_age_ms: facts.oldest_unconsumed_age_ms.unwrap_or(0),
            }
        }
        (CanonicalMemoryRead::Available(_), Some(_)) => Observed::Served,
        (CanonicalMemoryRead::Available(_), None) => Observed::ServedUnsampled,
    }
}

/// One read's verdict as the report compares it with the last one recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Observed {
    Served,
    /// Served with no fresh lag facts, before the first health sample, after a failed one, or after the published block went stale.
    ServedUnsampled,
    /// Served while a registered outbox consumer trails past a serving threshold; the fields are the lag sample.
    ServedWhileDerivedLag {
        consumers: u64,
        lag_positions: i64,
        oldest_unconsumed_age_ms: i64,
    },
    /// Withheld under the verdict's state key.
    Withheld(String),
}

/// The last verdict the canonical memory reader recorded, so a log reader sees each change of verdict once: a withheld read with its state, a read served while derived consumers lag, and the first plainly served read after either. Repeats of a verdict log nothing, and a lag sample that changes within the lagging verdict logs nothing either. An unsampled served read says nothing about the consumers: it keeps a served or lagging verdict as recorded and counts as plainly served after a withheld read or before any verdict. The report also counts the verdict changes it records, for the life of the coordinator, and each line carries that count as `verdict_seq`.
#[derive(Debug, Default)]
pub(crate) struct ReadReport(std::sync::Mutex<(Option<Observed>, u64)>);

impl ReadReport {
    /// Records `observed` and returns the line to log when its verdict differs from the last one recorded. The verdict is recorded before the caller logs, so a failed log write repeats nothing, and the caller logs outside the report's lock, so other reads record their verdicts while a log write is blocked, though reads that emit a line share stderr's own lock; lines from concurrent reads can print in either order, and each line carries `verdict_seq`, the count of verdict changes recorded through it, so a log reader orders them.
    pub(crate) fn observe(&self, observed: Observed) -> Option<String> {
        let mut guard = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (last, changes) = &mut *guard;
        let observed = match (&*last, observed) {
            (
                Some(Observed::Served | Observed::ServedWhileDerivedLag { .. }),
                Observed::ServedUnsampled,
            ) => return None,
            (_, Observed::ServedUnsampled) => Observed::Served,
            (_, observed) => observed,
        };
        let line = match (&*last, &observed) {
            (
                Some(Observed::ServedWhileDerivedLag { .. }),
                Observed::ServedWhileDerivedLag { .. },
            ) => None,
            (previous, _) if previous.as_ref() == Some(&observed) => None,
            (None, Observed::Served) => None,
            (_, Observed::Served | Observed::ServedUnsampled) => Some("served".to_owned()),
            (
                _,
                Observed::ServedWhileDerivedLag {
                    consumers,
                    lag_positions,
                    oldest_unconsumed_age_ms,
                },
            ) => Some(format!(
                "served from canonical rows while derived consumers lag: consumers={consumers} lag_positions={lag_positions} oldest_unconsumed_age_ms={oldest_unconsumed_age_ms}"
            )),
            (_, Observed::Withheld(state)) => Some(format!("withheld state={state}")),
        };
        *last = Some(observed);
        line.map(|line| {
            *changes += 1;
            format!("{line} verdict_seq={changes}")
        })
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

    fn lagging(lag_positions: i64) -> Observed {
        Observed::ServedWhileDerivedLag {
            consumers: 2,
            lag_positions,
            oldest_unconsumed_age_ms: 61_000,
        }
    }

    /// A withheld read is observed by its state key whatever the lag facts; a served read is lagging with fresh facts whose threshold tripped, plainly served with fresh facts below it, and unsampled without fresh facts.
    #[test]
    fn a_read_is_observed_by_its_verdict_and_the_fresh_lag_facts() {
        use crate::kernel_routes::health::KernelFactsBlock;
        let facts = |tripped| KernelFactsBlock {
            core_file_bytes: 0,
            core_file_warn: false,
            artifact_usage_bytes: 0,
            artifact_cap_bytes: 0,
            artifact_warn: false,
            outbox_position_lag: Some(12_000),
            oldest_unconsumed_age_ms: Some(61_000),
            retained_outbox_rows: 0,
            required_consumer_count: 2,
            lag_threshold_tripped: tripped,
        };
        let served =
            CanonicalMemoryRead::Available(CanonicalMemorySnapshot::new(4, false, Vec::new()));
        let withheld = CanonicalMemoryRead::Withheld(KernelOutcome::unavailable(
            UnavailableReason::StoreStarting,
        ));
        assert_eq!(
            observed(&withheld, Some(&facts(true))),
            Observed::Withheld("unavailable:store_starting".to_owned())
        );
        assert_eq!(observed(&served, None), Observed::ServedUnsampled);
        assert_eq!(observed(&served, Some(&facts(false))), Observed::Served);
        assert_eq!(
            observed(&served, Some(&facts(true))),
            Observed::ServedWhileDerivedLag {
                consumers: 2,
                lag_positions: 12_000,
                oldest_unconsumed_age_ms: 61_000,
            }
        );
    }

    /// Each change of verdict yields one line, a repeat yields none, a lag sample that moves within the lagging verdict yields none, and the first plainly served read after a withheld or lagging one yields `served`.
    #[test]
    fn the_read_report_logs_each_change_of_verdict_once() {
        let report = ReadReport::default();
        assert_eq!(report.observe(Observed::Served), None);
        assert_eq!(report.observe(Observed::Served), None);
        assert_eq!(
            report.observe(lagging(10_000)).as_deref(),
            Some(
                "served from canonical rows while derived consumers lag: consumers=2 lag_positions=10000 oldest_unconsumed_age_ms=61000 verdict_seq=1"
            )
        );
        assert_eq!(report.observe(lagging(12_000)), None);
        assert_eq!(
            report.observe(Observed::ServedUnsampled),
            None,
            "an unsampled read keeps the lagging verdict"
        );
        assert_eq!(
            report.observe(lagging(12_000)),
            None,
            "the lagging verdict stands"
        );
        assert_eq!(
            report.observe(Observed::Served).as_deref(),
            Some("served verdict_seq=2")
        );
        assert_eq!(
            report
                .observe(Observed::Withheld("unavailable:store_starting".to_owned()))
                .as_deref(),
            Some("withheld state=unavailable:store_starting verdict_seq=3")
        );
        assert_eq!(
            report.observe(Observed::Withheld("unavailable:store_starting".to_owned())),
            None
        );
        assert_eq!(
            report
                .observe(Observed::Withheld("unavailable:io".to_owned()))
                .as_deref(),
            Some("withheld state=unavailable:io verdict_seq=4")
        );
        assert_eq!(
            report.observe(Observed::ServedUnsampled).as_deref(),
            Some("served verdict_seq=5"),
            "an unsampled read after a withheld one is served"
        );
        assert!(report.observe(lagging(10_000)).is_some());
    }

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

    /// The served-set digest names the rendered rows' ids: it ignores order and rendered bytes, drops rows the renderer drops, and changes with the id set.
    #[test]
    fn the_served_set_digest_names_the_rendered_ids_and_not_their_bytes() {
        let row = |object_id: &str, category: &str, content: &str| CanonicalMemory {
            object_id: object_id.to_string(),
            category: category.to_string(),
            content: content.to_string(),
        };
        let first = CanonicalMemorySnapshot::new(
            7,
            false,
            vec![
                row("b", "PROJECT_RULES", "Rule b."),
                row("a", "NAMING", "Name a."),
                row("a", "PROJECT_RULES", "A second row of one object."),
                row("dropped", "not-a-memory-category", "Dropped."),
            ],
        );
        assert_eq!(first.served_ids(), ["a", "b"]);
        assert_eq!(
            first.served_set_digest(),
            kernel::served_set_digest(["a", "b"])
        );
        let reworded = CanonicalMemorySnapshot::new(
            8,
            false,
            vec![
                row("a", "NAMING", "Name a, reworded."),
                row("b", "PROJECT_RULES", "Rule b."),
            ],
        );
        assert_ne!(first.revision(), reworded.revision());
        assert_eq!(first.served_set_digest(), reworded.served_set_digest());
        let smaller = CanonicalMemorySnapshot::new(8, false, vec![row("a", "NAMING", "Name a.")]);
        assert_ne!(first.served_set_digest(), smaller.served_set_digest());
        let empty = CanonicalMemorySnapshot::new(8, false, Vec::new());
        assert_eq!(empty.served_set_digest(), kernel::served_set_digest([]));
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
        assert_eq!(
            trimmed.served_ids(),
            ["short"],
            "a row the budget excludes is not served"
        );
        assert_eq!(trimmed.served_set_digest(), only_short.served_set_digest());
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
