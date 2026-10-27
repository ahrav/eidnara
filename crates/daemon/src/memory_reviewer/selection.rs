//! Production selection of review targets for the Memory Classifier: a bounded keyset walk over the project's live canonical descriptors, judged through Kernel eligibility, deduplicated against the review jobs the Memory Store already holds, and frozen as at most eight causal inputs per page with the cursor the slot advances to once the page is enqueued.
//!
//! The walk is stable and resumable: `Cursor` names the descriptor class being walked and the last object id passed, so a page that could not be enqueued is examined again from the same place and a slot that enqueued advances past exactly what it froze. When both classes are exhausted the cursor is `None` and the next slot starts over, which is how arrivals whose ids sort before the cursor are reached: every live descriptor is examined once per full pass, and a target already covered by a job at the same causal inputs is passed without a second job. Selection, job admission, execution, and useful completion are separate observations; a frozen page is none of the last three.

use std::num::NonZeroUsize;

use kernel::applicability::EvalBudget;
use kernel::source_identity::OccurrenceClass;
use kernel::{
    ArtifactDestination, EligibilityCandidate, KernelStore, LinkedEvidence, LiveDescriptor,
    ProjectScope, Surface,
};
use memory_store::MemoryStore;
use memory_store::memory_reviewer_jobs::{
    CausalInputs, EvidenceAvailability, FrozenSelectionPage, MAX_IDENTITY_BYTES,
    MAX_SELECTION_REFERENCES, ReviewTarget,
};

/// The descriptor classes the production selector walks: the memories the Memory Classifier reviews, each resolved by the coordinator through its originating decision.
pub const MEMORY_CLASSES: &[OccurrenceClass] = &[
    OccurrenceClass::CanonicalClaims,
    OccurrenceClass::PromotedMemory,
];

// Every walked class must be decision-derived: native resolution targets its own descriptor and bypasses its owner's retraction. This assertion detects classes omitted from `decision_derived`.
const _: () = {
    let mut index = 0;
    while index < MEMORY_CLASSES.len() {
        assert!(super::broker::decision_derived(MEMORY_CLASSES[index]));
        index += 1;
    }
};

/// Gates scheduling of selection over [`MEMORY_CLASSES`]; the scheduler adds no selection task while this is `false`. Opening requires the owner decision and positive witness recorded under "Production selection gate" in `docs/memory-reviewer-operations.md`.
pub const PRODUCTION_SELECTION_OPEN: bool = false;
/// Live descriptors examined per selection page, so one slot's work is bounded whatever the inventory's size.
pub const MAX_EXAMINED_PER_PAGE: usize = 256;
/// Evidence rows an originating decision may contribute to one target's causal inputs: the most recently cited first, so the newest citation always enters the fingerprint, and a liveness change reconsiders a target only for evidence among these rows. With the descriptor's own evidence this stays under the ledger's `MAX_REQUIRED_EVIDENCE`.
pub const MAX_LINKED_EVIDENCE: NonZeroUsize = NonZeroUsize::new(8).unwrap();
const _: () =
    assert!(MAX_LINKED_EVIDENCE.get() < memory_store::memory_reviewer_jobs::MAX_REQUIRED_EVIDENCE);

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

/// Selects at most [`MAX_SELECTION_REFERENCES`] review targets for `scope` from `cursor`. A target is selected when its descriptor is live, the Kernel's Remote fold permits it in the project's scope, and no review job exists at its causal inputs; every other live descriptor is passed, including a locally readable one whose artifact the Remote destination denies.
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
    // The first batch holds as many rows as the page has references, and each later batch doubles up to the cap, so a page that fills from its first rows reads and judges few rows past them while a long walk still reads in large batches.
    let mut batch = MAX_SELECTION_REFERENCES;
    let mut ledger_first = false;
    // A page that filled at the end of a batch returns before another batch is read and judged.
    while references.len() < MAX_SELECTION_REFERENCES {
        let remaining = MAX_EXAMINED_PER_PAGE.saturating_sub(examined);
        let Some(max_rows) = NonZeroUsize::new(remaining.min(batch)) else {
            break;
        };
        batch = (batch * 2).min(MAX_SELECTION_REFERENCES * 8);
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
        let judge = |rows: &[&LiveDescriptor]| -> Result<Vec<bool>, SelectionError> {
            if rows.is_empty() {
                return Ok(Vec::new());
            }
            let candidates: Vec<EligibilityCandidate> = rows
                .iter()
                .map(|row| EligibilityCandidate {
                    object_id: row.object_id.clone(),
                    source_revision: row.detail.revision.parse().unwrap_or(-1),
                    artifact_digest: Some(row.detail.artifact_digest.clone()),
                })
                .collect();
            Ok(store
                .judge_surface_eligibility_within_budget(
                    project,
                    ArtifactDestination::Remote,
                    Surface::ExplicitSearch,
                    &candidates,
                    budget,
                )
                .map_err(kernel)?
                .verdicts
                .iter()
                .map(|verdict| verdict.permits())
                .collect())
        };
        let rows: Vec<&LiveDescriptor> = page.rows.iter().collect();
        // Eligibility-first judges the whole batch up front; ledger-first judges only the rows the ledger shows without a job.
        let permits = if ledger_first {
            None
        } else {
            Some(judge(&rows)?)
        };
        let may_select = |index: usize| permits.as_ref().is_none_or(|permits| permits[index]);
        let class = classes[cursor.class_index];
        let (mut checked, mut with_job) = (0usize, 0usize);
        let mut next_row = 0;
        // Each chunk holds exactly as many rows that may still be selected as the page has room for, so a chunk never reads past the row that fills the page, and its dedup is one ledger read.
        while next_row < rows.len() {
            // The cursor passes a row only after it is selected or skipped; a full page stops before the row that would not fit, so that row is examined first next time.
            if references.len() >= MAX_SELECTION_REFERENCES {
                return Ok(finish(references, Some(cursor)));
            }
            let room = MAX_SELECTION_REFERENCES - references.len();
            let start = next_row;
            let mut taken = 0;
            while next_row < rows.len() && taken < room {
                taken += usize::from(may_select(next_row));
                next_row += 1;
            }
            let judged: Vec<&LiveDescriptor> = (start..next_row)
                .filter(|index| may_select(*index))
                .map(|index| rows[index])
                .collect();
            let linked = linked_evidence(store, class, &judged, tip, budget).map_err(kernel)?;
            let mut bound = Vec::with_capacity(judged.len());
            let mut identities = Vec::with_capacity(judged.len());
            for (row, linked) in judged.into_iter().zip(linked) {
                if let Some((inputs, causal_identity)) =
                    causal_inputs(row, &linked, policy_versions)
                {
                    bound.push((row, inputs));
                    identities.push(causal_identity);
                }
            }
            if !identities.is_empty() {
                let identities: Vec<&str> = identities.iter().map(String::as_str).collect();
                let existing = ledger
                    .memory_reviewer_jobs_exist(ledger_project, &identities)
                    .map_err(|error| SelectionError::Ledger(error.to_string()))?;
                checked += existing.len();
                with_job += existing.iter().filter(|exists| **exists).count();
                let fresh: Vec<_> = bound
                    .into_iter()
                    .zip(existing)
                    .filter_map(|(bound, exists)| (!exists).then_some(bound))
                    .collect();
                if permits.is_some() {
                    references.extend(fresh.into_iter().map(|(_, inputs)| inputs));
                } else {
                    let fresh_rows: Vec<&LiveDescriptor> =
                        fresh.iter().map(|(row, _)| *row).collect();
                    let fresh_permits = judge(&fresh_rows)?;
                    references.extend(
                        fresh
                            .into_iter()
                            .zip(fresh_permits)
                            .filter_map(|((_, inputs), permitted)| permitted.then_some(inputs)),
                    );
                }
            }
            cursor.after = Some(rows[next_row - 1].object_id.clone());
        }
        // A target needs both a permitting verdict and no job, so either order selects the same rows; the order decides which check the rows failing the other one skip. A batch whose ledger-checked rows mostly had a job is followed by a ledger-first batch.
        ledger_first = with_job * 2 > checked;
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

/// The evidence each of `rows`' originating decisions cites at `tip`, itself or through its events, bounded by [`MAX_LINKED_EVIDENCE`], in `rows` order and read in one Kernel transaction; a native descriptor links none. This is the bounded enumeration that lets newly linked or newly retired evidence reconsider a target at an unchanged revision; unrelated writes, clocks, and selection attempts never enter it.
fn linked_evidence(
    store: &KernelStore,
    class: OccurrenceClass,
    rows: &[&LiveDescriptor],
    tip: i64,
    budget: &EvalBudget,
) -> Result<Vec<Vec<LinkedEvidence>>, kernel::KernelError> {
    let decisions: Vec<Option<&str>> = rows
        .iter()
        .map(|row| super::broker::originating_decision(class, &row.detail.identity))
        .collect();
    let read: Vec<&str> = decisions.iter().flatten().copied().collect();
    let mut linked = if read.is_empty() {
        Vec::new()
    } else {
        store.linked_decisions_evidence_within_budget(&read, tip, MAX_LINKED_EVIDENCE, budget)?
    }
    .into_iter();
    Ok(decisions
        .iter()
        .map(|decision| decision.and_then(|_| linked.next()).unwrap_or_default())
        .collect())
}

/// The causal inputs one live descriptor is reviewed under, with their identity: the memory at its live revision, the fixed question, no signals until a producer records one, the descriptor's evidence as required and available together with each `linked` row and its liveness, and the caller's policy versions. The descriptor's own evidence is live while the descriptor is, so it keeps that availability when its decision also cites it. A linked id longer than the ledger's identity bound is left out, since no job could carry it, so one such citation cannot hide a reviewable target. `None` when the ledger cannot bind them (an unparseable revision, an id past the ledger's identity bound): such a descriptor has no job to look up or reserve, so it is passed rather than held as a ledger failure the slot would retry every poll.
fn causal_inputs(
    row: &LiveDescriptor,
    linked: &[LinkedEvidence],
    policy_versions: &std::collections::BTreeMap<String, String>,
) -> Option<(CausalInputs, String)> {
    let mut evidence = std::collections::BTreeMap::from([(row.detail.evidence_id.clone(), true)]);
    for link in linked
        .iter()
        .filter(|link| link.evidence_id.len() <= MAX_IDENTITY_BYTES)
    {
        evidence
            .entry(link.evidence_id.clone())
            .or_insert(link.live);
    }
    let inputs = CausalInputs {
        target: ReviewTarget::Memory {
            object_id: row.object_id.clone(),
            source_revision: row.detail.revision.parse().ok()?,
        },
        question_template: super::broker::QuestionTemplate::ExtractedFacts
            .id()
            .to_string(),
        signals: Vec::new(),
        required_evidence: evidence
            .into_iter()
            .map(|(evidence_id, available)| EvidenceAvailability {
                evidence_id,
                available,
            })
            .collect(),
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
            byte_length: 0,
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
                outcome: None,
            },
        };
        let versions = std::collections::BTreeMap::new();
        assert!(causal_inputs(&row("evidence-1".to_string()), &[], &versions).is_some());
        // The Kernel accepts text fields up to 1024 bytes; the ledger binds identities up to 256.
        assert!(
            causal_inputs(&row("e".repeat(300)), &[], &versions).is_none(),
            "an evidence id the ledger refuses yields no inputs"
        );
        // A linked id past the ledger's bound is left out; the rest still bind.
        let linked = [
            kernel::LinkedEvidence {
                evidence_id: "l".repeat(300),
                live: true,
            },
            kernel::LinkedEvidence {
                evidence_id: "linked-1".to_string(),
                live: false,
            },
        ];
        let (inputs, _) = causal_inputs(&row("evidence-1".to_string()), &linked, &versions)
            .expect("an overlong linked id does not hide the target");
        assert_eq!(
            inputs
                .required_evidence
                .iter()
                .map(|e| (e.evidence_id.as_str(), e.available))
                .collect::<Vec<_>>(),
            vec![("evidence-1", true), ("linked-1", false)]
        );
        // The descriptor's own evidence keeps its availability when its decision cites it too.
        let own = [kernel::LinkedEvidence {
            evidence_id: "evidence-1".to_string(),
            live: false,
        }];
        let (inputs, _) = causal_inputs(&row("evidence-1".to_string()), &own, &versions).unwrap();
        assert_eq!(inputs.required_evidence.len(), 1);
        assert!(inputs.required_evidence[0].available);
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
