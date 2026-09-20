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
use memory_store::memory_reviewer_jobs::{
    CausalInputs, EvidenceAvailability, FrozenSelectionPage, MAX_SELECTION_REFERENCES, ReviewTarget,
};

/// The descriptor classes the production selector walks: the memories the Memory Classifier reviews, each resolved by the coordinator through its originating decision.
pub const MEMORY_CLASSES: &[OccurrenceClass] = &[
    OccurrenceClass::CanonicalClaims,
    OccurrenceClass::PromotedMemory,
];

/// Gates scheduling of selection over [`MEMORY_CLASSES`]; the scheduler adds no selection task while this is `false`. Opening requires the owner decision and positive witness recorded under "Production selection gate" in `docs/memory-reviewer-operations.md`.
pub const PRODUCTION_SELECTION_OPEN: bool = false;
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

/// A failed selection. `Kernel` retains its error class because retryability depends on it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SelectionError {
    #[error("kernel {0}")]
    Kernel(kernel::KernelError),
    #[error("ledger {0}")]
    Ledger(String),
}

impl SelectionError {
    /// Returns whether retrying the unchanged selection may succeed: a busy reader, an exhausted budget, or a ledger read failure. A corrupt row or a refused request at this cursor cannot clear until the store changes.
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Kernel(error) => error.is_retryable() || *error == kernel::KernelError::Deadline,
            Self::Ledger(_) => true,
        }
    }
}

/// What one project's selection is scoped and fingerprinted by.
pub struct SelectionScope<'a> {
    /// The Kernel scope eligibility is judged in: the only Kernel-scoped member.
    pub project: &'a ProjectScope,
    /// The Memory Store project the review jobs live under: the authority key, not the Kernel digest.
    pub ledger_project: &'a str,
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
        ledger_project,
        classes,
        policy_versions,
    } = *scope;
    let kernel = SelectionError::Kernel;
    if classes.is_empty() {
        return Ok(finish(Vec::new(), None));
    }
    let mut cursor = Cursor::decode(cursor, classes.len());
    let tip = store.tip_within_budget(budget).map_err(kernel)?;
    let mut references = Vec::new();
    let mut examined = 0usize;
    // A page that filled at the end of a batch returns before another batch is read and judged.
    while references.len() < MAX_SELECTION_REFERENCES {
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
                && let Some((inputs, causal_identity)) = causal_inputs(row, policy_versions)
                && ledger
                    .lookup_memory_reviewer_job(ledger_project, &causal_identity)
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

/// The causal inputs one live descriptor is reviewed under, with their identity: the memory at its live revision, the fixed question, no signals until a producer records one, the descriptor's evidence as required and available, and the caller's policy versions. `None` when the ledger cannot bind them (an unparseable revision, an id past the ledger's identity bound): such a descriptor has no job to look up or reserve, so it is passed rather than held as a ledger failure the slot would retry every poll.
fn causal_inputs(
    row: &LiveDescriptor,
    policy_versions: &std::collections::BTreeMap<String, String>,
) -> Option<(CausalInputs, String)> {
    let inputs = CausalInputs {
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
    };
    let causal_identity = inputs.causal_identity().ok()?;
    Some((inputs, causal_identity))
}

#[cfg(test)]
mod tests {
    use super::{Cursor, SelectionError, causal_inputs};

    /// A live descriptor the Kernel admits with an id past the ledger's identity bound has no job to look up or reserve; it is passed like an ineligible row instead of holding the walk on a refusal that never clears.
    #[test]
    fn a_descriptor_the_ledger_cannot_bind_is_passed_not_retried() {
        let row = |evidence_id: String| kernel::LiveDescriptor {
            object_id: "object-1".to_string(),
            domain_id: "domain".to_string(),
            sensitivity: kernel::Sensitivity::Normal,
            created_commit_seq: 1,
            detail: kernel::SourceDescriptorDetail {
                descriptor_version: kernel::SOURCE_DESCRIPTOR_DETAIL_VERSION,
                source_policy: kernel::SourceDescriptorPolicy::Native,
                class: "git_commits".to_string(),
                identity: Vec::new(),
                revision: "1".to_string(),
                representation: "summary".to_string(),
                span: None,
                occurrence_id: "occurrence".to_string(),
                occurrence_tuple: Vec::new(),
                lineage_id: "lineage".to_string(),
                payload_id: "payload".to_string(),
                artifact_digest: "0d".repeat(32),
                evidence_id,
            },
        };
        let versions = std::collections::BTreeMap::new();
        assert!(causal_inputs(&row("evidence-1".to_string()), &versions).is_some());
        // The Kernel accepts text fields up to 1024 bytes; the ledger binds identities up to 256.
        assert!(
            causal_inputs(&row("e".repeat(300)), &versions).is_none(),
            "an evidence id the ledger refuses yields no inputs"
        );
    }

    #[test]
    fn only_a_busy_reader_or_an_exhausted_budget_is_transient() {
        for error in kernel::KernelError::ALL {
            assert_eq!(
                SelectionError::Kernel(*error).is_transient(),
                matches!(
                    error,
                    kernel::KernelError::Busy
                        | kernel::KernelError::Held
                        | kernel::KernelError::Deadline
                ),
                "{error}"
            );
        }
        assert!(SelectionError::Ledger("locked".to_string()).is_transient());
    }

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
