//! Production selection of review targets for the Memory Classifier: a bounded keyset walk over the project's live canonical descriptors, judged through Kernel eligibility, deduplicated against the review jobs the Memory Store already holds, and frozen as at most eight causal inputs per page with the cursor the slot advances to once the page is enqueued.
//!
//! The walk is stable and resumable: `Cursor` names the descriptor class being walked and the last object id passed, so a page that could not be enqueued is examined again from the same place and a slot that enqueued advances past exactly what it froze. When both classes are exhausted the cursor is `None` and the next slot starts over, which is how arrivals whose ids sort before the cursor are reached: every live descriptor is examined once per full pass, and a target already covered by a job at the same causal inputs is passed without a second job. Selection, job admission, execution, and useful completion are separate observations; a frozen page is none of the last three.

use std::num::NonZeroUsize;

use kernel::applicability::EvalBudget;
use kernel::source_identity::OccurrenceClass;
use kernel::{
    ArtifactDestination, EligibilityCandidate, KernelStore, LiveDescriptor, ProjectScope, Surface,
};
use memory_store::MemoryStore;
use memory_store::curator_jobs::{
    CausalInputs, EvidenceAvailability, FrozenSelectionPage, MAX_SELECTION_REFERENCES, ReviewTarget,
};

/// Descriptor classes the production selector walks: the memories the Memory Classifier reviews.
pub const MEMORY_CLASSES: &[OccurrenceClass] = &[
    OccurrenceClass::CanonicalClaims,
    OccurrenceClass::PromotedMemory,
];
/// Live descriptors examined per selection page, so one slot's work is bounded whatever the inventory's size.
pub const MAX_EXAMINED_PER_PAGE: usize = 256;

/// Where a selection resumes: the index into the class list and the last object id passed in it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Cursor {
    class_index: usize,
    after: Option<String>,
}

impl Cursor {
    /// `class_index` and the object id, separated by the unit separator. A token that does not decode as a whole restarts the walk from the beginning rather than refusing the slot or resuming part-way, since the cursor is the slot's own record.
    fn decode(token: Option<&str>, classes: usize) -> Self {
        let start = Self {
            class_index: 0,
            after: None,
        };
        let Some((index, after)) = token.and_then(|token| token.split_once('\u{1f}')) else {
            return start;
        };
        match index.parse::<usize>() {
            Ok(class_index) if class_index < classes => Self {
                class_index,
                after: (!after.is_empty()).then(|| after.to_string()),
            },
            _ => start,
        }
    }

    fn encode(&self) -> String {
        format!(
            "{}\u{1f}{}",
            self.class_index,
            self.after.as_deref().unwrap_or("")
        )
    }
}

/// Why no page could be selected; a store failure keeps the slot due, and nothing else refuses.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SelectionError {
    #[error("kernel {0}")]
    Kernel(String),
    #[error("ledger {0}")]
    Ledger(String),
}

/// What one project's selection is scoped and fingerprinted by.
pub struct SelectionScope<'a> {
    pub project: &'a ProjectScope,
    pub project_digest: &'a str,
    /// Descriptor classes walked, in order; production passes [`MEMORY_CLASSES`].
    pub classes: &'a [OccurrenceClass],
    /// The policies the review depends on, so a policy change permits one new job at an unchanged target.
    pub policy_versions: &'a std::collections::BTreeMap<String, String>,
}

/// Selects at most [`MAX_SELECTION_REFERENCES`] review targets for `scope` from `cursor`. A target is selected when its descriptor is live and eligible for a local reader in the project's scope and no review job exists at its causal inputs; every other live descriptor is passed.
pub fn select_review_targets(
    store: &KernelStore,
    ledger: &MemoryStore,
    scope: &SelectionScope<'_>,
    cursor: Option<&str>,
    budget: &EvalBudget,
) -> Result<FrozenSelectionPage, SelectionError> {
    let SelectionScope {
        project,
        project_digest,
        classes,
        policy_versions,
    } = *scope;
    let kernel = |error: kernel::KernelError| SelectionError::Kernel(error.to_string());
    if classes.is_empty() {
        return Ok(finish(Vec::new(), None));
    }
    let mut cursor = Cursor::decode(cursor, classes.len());
    let tip = store.tip().map_err(kernel)?;
    let mut references = Vec::new();
    let mut examined = 0usize;
    loop {
        let remaining = MAX_EXAMINED_PER_PAGE.saturating_sub(examined);
        let Some(max_rows) = NonZeroUsize::new(remaining.min(MAX_SELECTION_REFERENCES * 4)) else {
            break;
        };
        let page = store
            .live_source_descriptors(
                classes[cursor.class_index],
                tip,
                cursor.after.as_deref(),
                max_rows,
                budget,
            )
            .map_err(kernel)?;
        examined += page.rows.len();
        // The review these targets feed discloses to a remote model, so a target is eligible only if the remote fold admits it; a locally readable but remotely denied memory would spend a job on a policy abstention.
        let candidates: Vec<EligibilityCandidate> = page
            .rows
            .iter()
            .map(|row| EligibilityCandidate {
                object_id: row.object_id.clone(),
                source_revision: row.detail.revision.parse().unwrap_or(-1),
                artifact_digest: Some(row.detail.artifact_digest.clone()),
            })
            .collect();
        let verdicts = if candidates.is_empty() {
            Vec::new()
        } else {
            store
                .judge_surface_eligibility_within_budget(
                    project,
                    ArtifactDestination::Remote,
                    Surface::ExplicitSearch,
                    &candidates,
                    budget,
                )
                .map_err(kernel)?
                .verdicts
        };
        for (row, verdict) in page.rows.iter().zip(verdicts) {
            // The cursor passes a row only after it is selected or skipped; a full page stops before the row that would not fit, so that row is examined first next time.
            if references.len() >= MAX_SELECTION_REFERENCES {
                return Ok(finish(references, Some(cursor)));
            }
            if verdict.permits()
                && let Some(inputs) = causal_inputs(row, policy_versions)
                && ledger
                    .lookup_curator_job(
                        project_digest,
                        &inputs
                            .causal_identity()
                            .map_err(|error| SelectionError::Ledger(error.to_string()))?,
                    )
                    .map_err(|error| SelectionError::Ledger(error.to_string()))?
                    .is_none()
            {
                references.push(inputs);
            }
            cursor.after = Some(row.object_id.clone());
        }
        if page.next.is_none() {
            if cursor.class_index + 1 < classes.len() {
                cursor.class_index += 1;
                cursor.after = None;
            } else {
                return Ok(finish(references, None));
            }
        }
    }
    Ok(finish(references, Some(cursor)))
}

fn finish(references: Vec<CausalInputs>, cursor: Option<Cursor>) -> FrozenSelectionPage {
    FrozenSelectionPage {
        references,
        next_cursor: cursor.map(|cursor| cursor.encode()),
    }
}

/// The causal inputs one live descriptor is reviewed under: the memory at its live revision, the fixed question, no signals until a producer records one, the descriptor's evidence as required and available, and the caller's policy versions.
fn causal_inputs(
    row: &LiveDescriptor,
    policy_versions: &std::collections::BTreeMap<String, String>,
) -> Option<CausalInputs> {
    Some(CausalInputs {
        target: ReviewTarget::Memory {
            object_id: row.object_id.clone(),
            source_revision: row.detail.revision.parse().ok()?,
        },
        question_template: super::broker::QuestionTemplate::ExtractedFacts
            .id()
            .to_string(),
        signals: Vec::new(),
        required_evidence: vec![EvidenceAvailability {
            evidence_id: row.detail.evidence_id.clone(),
            available: true,
        }],
        policy_versions: policy_versions.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::Cursor;

    #[test]
    fn cursors_round_trip_and_malformed_tokens_restart() {
        let cursor = Cursor {
            class_index: 1,
            after: Some("object-7".to_string()),
        };
        assert_eq!(Cursor::decode(Some(&cursor.encode()), 2), cursor);
        let start = Cursor {
            class_index: 0,
            after: None,
        };
        assert_eq!(Cursor::decode(Some("0\u{1f}"), 2), start);
        assert_eq!(Cursor::decode(Some("garbage"), 2), start);
        assert_eq!(
            Cursor::decode(Some("x\u{1f}object-7"), 2),
            start,
            "a partial decode restarts rather than resuming inside class 0"
        );
        assert_eq!(
            Cursor::decode(Some("5\u{1f}object-7"), 2),
            start,
            "a class the list no longer has restarts"
        );
        assert_eq!(Cursor::decode(None, 2), start);
    }
}
