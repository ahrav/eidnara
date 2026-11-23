//! What the worker does for a project whose user tier set `memory_reviewer.enabled`: it gives the bound root MODULE memories authority, keeps the project's claim descriptors published, enqueues one review job for each memory that new evidence bears on, and treats each published proposal as `memory_reviewer.apply` says.
//!
//! Selection works on decisions. A decision's `decision_summary` descriptor is the job's subject, so one decision yields one job whatever other forms it has. A memory is reviewed on evidence of change: a later source that overlaps it, the source of a later memory about the same subject, or the source of the memory that contests it. The job carries the decision's cited message blocks and those later sources as starting references, so a run reads the statement, where it came from, and what was said later. The later sources are the job's causal signal, so new evidence reviews a memory again and the same evidence reviews it once.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};

use kernel::applicability::EvalBudget;
use kernel::source_identity::OccurrenceClass;
use kernel::{
    AdmissionEvent, AdmissionRequest, ArtifactDestination, CitingDescriptor, CommitIntent,
    DecisionRow, DisputeReporter, EligibilityCandidate, EventKind, KernelError, KernelStore,
    LiveDescriptor, MAX_ELIGIBILITY_CANDIDATES, ProjectScope, ProposalAction, ProposalTarget,
    ProviderEgress, SourceClass, Surface, TaintClass,
};
use memory_store::memory_reviewer_jobs::{
    CausalInputs, MAX_IDENTITY_BYTES, MAX_MEMORY_REVIEWER_STARTING_REFERENCES,
    MAX_PENDING_MEMORY_REVIEWER_JOBS_PER_PROJECT, MemoryReviewerJobError, MemoryReviewerJobInput,
    MemoryReviewerJobRefusal, ProducerBinding, ReserveOutcome, ReviewTarget,
};
use memory_store::{MemoryStore, MemoryStoreError};
use sha2::{Digest, Sha256};

use super::broker::{QuestionTemplate, originating_decision};
use super::classifier::{Stage1, Stage1Record, Writer};
use super::settlement::{
    AcceptRefusal, ReadRefusal, accept_selected_proposal, read_selected_proposal,
    release_settled_review_hold,
};
use crate::claim_sources::{ClaimMaterializer, ClaimProgress, MaterializationEnd};
use crate::config::ReviewApply;

/// The context store the switch's authority rows are recorded under.
pub const SWITCH_AUTHORITY_STORE: &str = "eidnara-memory-reviewer";
/// The producer every job the switch selects is reserved under.
pub const SELECTION_PRODUCER: &str = "memory-reviewer-switch";
/// The operation key the worker accepts its own proposals under in `apply` mode.
pub const APPLY_OPERATION_KEY: &str = "memory-reviewer-switch";
/// Claim descriptors one selection examines, so a selection's work is bounded whatever the inventory's size.
pub const MAX_WALKED_DESCRIPTORS: usize = 4096;
/// Later overlapping sources a job carries as starting references and causal signals, newest first.
pub const MAX_LATER_SOURCES: usize = 4;
/// Cited message blocks a job carries as starting references.
pub const MAX_CITED_SOURCES: usize = 2;
const _: () =
    assert!(MAX_LATER_SOURCES + MAX_CITED_SOURCES <= MAX_MEMORY_REVIEWER_STARTING_REFERENCES);
/// Candidate rows read per decision.
const CANDIDATE_ROWS_PER_DECISION: usize = 16;
/// The representation of a decision whose descriptor a job reviews.
const SUBJECT_REPRESENTATION: &str = "decision_summary";
/// The longest one selection or one claim catch-up runs.
const CYCLE_BUDGET: Duration = Duration::from_secs(30);
/// Claim slices one catch-up runs before it leaves the rest to the next pass.
const MAX_CLAIM_SLICES: usize = 256;

/// Gives `route_root` MODULE memories authority when it has none, with the root itself as the authority project, so every key derived from the authority project keeps the value an unmanaged root has. A root that already holds the authority is left as it is.
pub fn grant_authority(store: &MemoryStore, route_root: &str) -> Result<(), MemoryStoreError> {
    const DOMAIN: &str = "memories";
    if store
        .module_authority_for_route(route_root, DOMAIN)?
        .is_some()
    {
        return Ok(());
    }
    let begun = store.authority_begin_prepare(SWITCH_AUTHORITY_STORE, route_root, DOMAIN)?;
    // The seed is the store's own rows for this identity, so the expected checksum is the one it computes.
    let checksum = store.authority_seed_checksum(SWITCH_AUTHORITY_STORE, route_root, DOMAIN)?;
    store.authority_verify_prepare(
        SWITCH_AUTHORITY_STORE,
        route_root,
        DOMAIN,
        begun.generation,
        &checksum,
        &checksum,
    )?;
    store.authority_ack_prepare(SWITCH_AUTHORITY_STORE, route_root, DOMAIN, begun.generation)?;
    store.bind_authority_route(SWITCH_AUTHORITY_STORE, route_root, route_root)
}

/// Runs claim-source slices from `progress` until the consumer reaches the Kernel's tip, the budget ends, or a slice is blocked, so every live memory decision has its descriptors before a selection reads them. The host's search lifecycle owner runs the same consumer where it exists; this catch-up serves a deployment without one.
pub fn publish_claims(kernel: &KernelStore, progress: &mut ClaimProgress, now: i64) {
    let budget = EvalBudget::new(Some(Instant::now() + CYCLE_BUDGET), Arc::default());
    for _ in 0..MAX_CLAIM_SLICES {
        let report = ClaimMaterializer::new(kernel, ProviderEgress::LocalOnly).run_slice(
            progress,
            crate::search_lifecycle_owner::CLAIM_SLICE_BOUNDS,
            &budget,
            now,
        );
        match report.map(|report| report.end) {
            Ok(MaterializationEnd::Continues) => {}
            Ok(MaterializationEnd::ReachedTarget | MaterializationEnd::Exhausted) => return,
            Ok(MaterializationEnd::Blocked(blocked)) => {
                eprintln!("daemon: memory_reviewer claim catch-up is blocked: {blocked:?}");
                return;
            }
            Err(error) => {
                eprintln!("daemon: memory_reviewer claim catch-up failed: {error}");
                return;
            }
        }
    }
}

/// What brought a memory to review. A memory is reviewed on evidence that it may have changed, and at its creation it has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Trigger {
    /// An observation contests the statement.
    Contested,
    /// The memory records work in progress and later text bears on it.
    CurrentWork,
    /// A later memory of the same category shares the statement's subject terms.
    LaterStatement,
    /// Source text published after the memory overlaps it.
    LaterMessage,
}

impl Trigger {
    pub const ALL: [Self; 4] = [
        Self::Contested,
        Self::CurrentWork,
        Self::LaterStatement,
        Self::LaterMessage,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Contested => "contested",
            Self::CurrentWork => "current_work",
            Self::LaterStatement => "later_statement",
            Self::LaterMessage => "later_message",
        }
    }
}

/// One project's selection scope: the Kernel scope its decisions are judged in and the Memory Store project its jobs live under.
pub struct SelectionScope<'a> {
    pub project: &'a ProjectScope,
    pub project_digest: &'a str,
    pub ledger_project: &'a str,
}

/// What one selection did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Selected {
    /// Decisions examined.
    pub examined: usize,
    /// Jobs enqueued, by trigger in [`Trigger::ALL`] order.
    pub enqueued: [usize; 4],
    /// Memories with new evidence that wait for a proposal a reader has yet to settle.
    pub waiting: usize,
}

impl Selected {
    pub fn total(&self) -> usize {
        self.enqueued.iter().sum()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CycleError {
    #[error("kernel {0}")]
    Kernel(KernelError),
    #[error("ledger {0}")]
    Ledger(String),
}

/// What the selection keeps between passes for one worker: who wrote each source it has looked up, and which memories wait for a pending proposal.
#[derive(Default)]
pub struct SelectionMemory {
    sources: HashMap<String, SourceFacts>,
    pending: HashMap<String, PendingProposals>,
}

/// The descriptor a job reads a source through and who wrote the source. The writer is fixed for an evidence row. The descriptor is live until the store retires it: a captured message is retained for a bounded time, and a memory outlives it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceFacts {
    object_id: String,
    writer: Writer,
    /// Set once a later read finds the descriptor retired. A retired source still counts as evidence a memory was reviewed on, and no job reads it.
    retired: bool,
}

/// Sources remembered at once; the map starts over past this.
const MAX_REMEMBERED_SOURCES: usize = 65_536;

impl SelectionMemory {
    /// The descriptor and writer of `evidence_id`, read once and remembered, with whether a later read found the descriptor retired. `None` while no live descriptor has cited the evidence, which is looked up again on the next pass.
    fn source(
        &mut self,
        kernel: &KernelStore,
        evidence_id: &str,
        tip: i64,
        budget: &EvalBudget,
    ) -> Result<Option<SourceFacts>, CycleError> {
        if let Some(facts) = self.sources.get(evidence_id) {
            return Ok(Some(facts.clone()));
        }
        let Some(object_id) = source_descriptor(kernel, evidence_id, tip, budget)? else {
            return Ok(None);
        };
        let writer = kernel
            .preview(Instant::now() + Duration::from_secs(5), |preview| {
                preview.subject_admission(&object_id)
            })
            .ok()
            .and_then(|(_, admission)| admission)
            .map_or(Writer::Unrecorded, |(prior, _)| match prior.source_class {
                SourceClass::ExplicitUser => Writer::User,
                SourceClass::TrustedToolResult => Writer::Tool,
                SourceClass::UntrustedRepoText => Writer::RepositoryFile,
                _ => Writer::Assistant,
            });
        if self.sources.len() >= MAX_REMEMBERED_SOURCES {
            self.sources.clear();
        }
        let facts = SourceFacts {
            object_id,
            writer,
            retired: false,
        };
        self.sources.insert(evidence_id.to_string(), facts.clone());
        Ok(Some(facts))
    }

    /// The descriptor a job may read `evidence_id` through now, read from the Kernel at this call, or `None` once the store has retired it. A remembered source found retired is marked, so later passes skip the read.
    fn readable(
        &mut self,
        kernel: &KernelStore,
        evidence_id: &str,
        tip: i64,
        budget: &EvalBudget,
    ) -> Result<Option<String>, CycleError> {
        if self
            .sources
            .get(evidence_id)
            .is_some_and(|facts| facts.retired)
        {
            return Ok(None);
        }
        let live = source_descriptor(kernel, evidence_id, tip, budget)?;
        if live.is_none()
            && let Some(facts) = self.sources.get_mut(evidence_id)
        {
            facts.retired = true;
        }
        Ok(live)
    }

    /// Records that the memory `target` waits for the change proposal of the job `causal_identity`.
    pub fn note_pending(&mut self, ledger_project: &str, target: &str, causal_identity: &str) {
        self.pending
            .entry(ledger_project.to_string())
            .or_default()
            .waiting
            .insert(target.to_string(), causal_identity.to_string());
    }
}

/// The memories of one project that wait for a change proposal: each maps the reviewed descriptor to the job whose proposal is pending. The map is rebuilt once per process from the project's active review holds, so a restart keeps a waiting memory waiting.
#[derive(Default)]
struct PendingProposals {
    rebuilt: bool,
    waiting: HashMap<String, String>,
}

/// Completed receipts one rebuild reads to find the proposals behind the project's active review holds.
const MAX_REBUILD_RECEIPTS: usize = 16_384;

impl PendingProposals {
    fn rebuild(
        &mut self,
        kernel: &KernelStore,
        ledger: &MemoryStore,
        scope: &SelectionScope<'_>,
        now: i64,
    ) {
        self.rebuilt = true;
        let Ok(holds) =
            kernel.list_active_review_holds(now, kernel::MAX_ACTIVE_MEMORY_REVIEWER_HOLDS_PER_HOST)
        else {
            return;
        };
        let held: BTreeSet<String> = holds
            .into_iter()
            .filter(|hold| hold.binding.project_digest == scope.project_digest)
            .map(|hold| hold.binding.subject)
            .collect();
        if held.is_empty() {
            return;
        }
        let mut after: Option<String> = None;
        let mut read = 0;
        while read < MAX_REBUILD_RECEIPTS {
            let Ok(page) = ledger.list_completed_memory_reviewer_receipts(
                scope.ledger_project,
                after.as_deref(),
                256,
            ) else {
                return;
            };
            read += page.len();
            let last = page.last().map(|receipt| receipt.causal_identity.clone());
            for receipt in &page {
                let selects_held = receipt
                    .selected
                    .as_ref()
                    .is_some_and(|(_, selection)| held.contains(&selection.candidate_id));
                if !selects_held {
                    continue;
                }
                if let Ok(Some(job)) = ledger
                    .lookup_memory_reviewer_job(scope.ledger_project, &receipt.causal_identity)
                    && let ReviewTarget::Memory { object_id, .. } = job.target
                {
                    self.waiting
                        .insert(object_id, receipt.causal_identity.clone());
                }
            }
            match last {
                Some(last) if page.len() == 256 => after = Some(last),
                _ => break,
            }
        }
    }

    /// Whether `target` waits for a pending change proposal. The proposal is read back: one that was settled, or whose review window ended, no longer holds the memory.
    fn waits(
        &mut self,
        kernel: &KernelStore,
        ledger: &MemoryStore,
        scope: &SelectionScope<'_>,
        target: &str,
        now: i64,
    ) -> bool {
        if !self.rebuilt {
            self.rebuild(kernel, ledger, scope, now);
        }
        let Some(causal_identity) = self.waiting.get(target) else {
            return false;
        };
        let pending =
            read_selected_proposal(kernel, ledger, scope.ledger_project, causal_identity, now)
                .is_ok_and(|selected| {
                    matches!(
                        selected.proposal.action,
                        ProposalAction::Retire | ProposalAction::Revise
                    )
                });
        if !pending {
            self.waiting.remove(target);
        }
        pending
    }
}

/// One later source that may bear on a memory.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LaterEvidence {
    /// The publish tip of a ledger candidate row, or the commit of the later memory that cites the source.
    recency: i64,
    evidence_id: String,
    trigger: Trigger,
}

struct Candidate<'a> {
    descriptor: &'a LiveDescriptor,
    decision: &'a DecisionRow,
    trigger: Trigger,
    /// The newest later source's recency, so the newest evidence is reviewed first.
    recency: i64,
    cited: Vec<String>,
    /// The later sources' evidence ids, newest first.
    signals: Vec<String>,
}

/// Words a statement uses whatever it is about; they are left out of its subject terms.
const GENERIC_TERMS: [&str; 36] = [
    "all",
    "also",
    "always",
    "any",
    "can",
    "each",
    "ensure",
    "every",
    "forward",
    "going",
    "has",
    "its",
    "like",
    "make",
    "must",
    "need",
    "never",
    "new",
    "now",
    "our",
    "please",
    "remember",
    "should",
    "stated",
    "sure",
    "their",
    "them",
    "these",
    "those",
    "user",
    "want",
    "when",
    "will",
    "would",
    "assistant",
    "reported",
];
/// Subject terms an older memory shares with a later statement of its category before that statement brings it to review.
pub const MIN_SHARED_SUBJECT_TERMS: usize = 2;
/// Older memories one later statement brings to review, the ones sharing most subject terms first.
pub const REOPENED_PER_STATEMENT: usize = 4;

/// The subject terms of a statement: its analyzed terms without the generic words, each cut to its stem.
fn subject_terms(statement: &str) -> BTreeSet<String> {
    crate::contradiction_candidates::scorer::term_set(
        statement,
        crate::contradiction_candidates::scorer::SUMMARY_BOUNDS,
    )
    .unwrap_or_default()
    .into_iter()
    .filter(|term| !GENERIC_TERMS.contains(&term.as_str()))
    .map(|term| stem(&term).to_string())
    .collect()
}

/// The stem two inflections of one word share: the term without one of the endings `ing`, `ed`, `es`, `s`, and then without a final `e`, while at least three bytes remain. `naming`, `names` and `name` meet at `nam`, and `ending` and `ends` at `end`. The stem is a key for matching statements about one subject and is never shown.
fn stem(term: &str) -> &str {
    fn cut<'a>(term: &'a str, ending: &str) -> Option<&'a str> {
        term.strip_suffix(ending).filter(|stem| stem.len() >= 3)
    }
    // A word that ends in a double `s` is whole: `class` keeps its `s`.
    let stem = ["ing", "ed", "es", "s"]
        .into_iter()
        .filter(|ending| !(ending.len() <= 2 && term.ends_with("ss")))
        .find_map(|ending| cut(term, ending))
        .unwrap_or(term);
    cut(stem, "e").unwrap_or(stem)
}

/// For each decision, the later decisions of its category that share at least [`MIN_SHARED_SUBJECT_TERMS`] subject terms with it, where each later decision names at most [`REOPENED_PER_STATEMENT`] earlier ones. This is the second way a memory comes back to review beside the ledger's candidate rows: a later statement about the same subject reaches the earlier one even when it shares too few words for a candidate row.
fn reopened_by_later_statements(decisions: &[&DecisionRow]) -> Vec<Vec<usize>> {
    let terms: Vec<BTreeSet<String>> = decisions
        .iter()
        .map(|decision| subject_terms(&decision.payload.summary))
        .collect();
    let mut postings: HashMap<&str, Vec<usize>> = HashMap::new();
    for (index, terms) in terms.iter().enumerate() {
        for term in terms {
            postings.entry(term.as_str()).or_default().push(index);
        }
    }
    let mut reopened = vec![Vec::new(); decisions.len()];
    for (later, later_terms) in terms.iter().enumerate() {
        let mut shared: HashMap<usize, usize> = HashMap::new();
        for term in later_terms {
            for &earlier in postings.get(term.as_str()).into_iter().flatten() {
                if decisions[earlier].created_commit_seq < decisions[later].created_commit_seq
                    && decisions[earlier].decision_kind == decisions[later].decision_kind
                {
                    *shared.entry(earlier).or_default() += 1;
                }
            }
        }
        let mut ranked: Vec<(usize, usize)> = shared
            .into_iter()
            .filter(|(_, count)| *count >= MIN_SHARED_SUBJECT_TERMS)
            .collect();
        ranked.sort_by(|a, b| {
            (
                std::cmp::Reverse(a.1),
                std::cmp::Reverse(decisions[a.0].created_commit_seq),
                &decisions[a.0].object_id,
            )
                .cmp(&(
                    std::cmp::Reverse(b.1),
                    std::cmp::Reverse(decisions[b.0].created_commit_seq),
                    &decisions[b.0].object_id,
                ))
        });
        for (earlier, _) in ranked.into_iter().take(REOPENED_PER_STATEMENT) {
            reopened[earlier].push(later);
        }
    }
    reopened
}

/// Who a memory's statement says said it.
fn said_by(statement: &str) -> Writer {
    if statement.starts_with("User stated") {
        Writer::User
    } else if statement.starts_with("Assistant reported") {
        Writer::Assistant
    } else {
        Writer::Unrecorded
    }
}

/// Selects the project's memories that new evidence bears on and enqueues at most `room` jobs, newest evidence first, each Ready with its starting references. Evidence is a later source the memory does not cite, written by someone who may change it: a ledger candidate row, the source of a later memory of the same category and subject, or the source of the memory that contests it. A user's statement takes evidence only from the user; a fact the assistant reported, and work in progress, take it from anyone. The evidence is the job's causal signal, so a memory is reviewed once per set of evidence and again only when the set grows. A memory whose change proposal is pending waits for it.
pub fn select_and_enqueue(
    kernel: &KernelStore,
    ledger: &MemoryStore,
    memory: &mut SelectionMemory,
    scope: &SelectionScope<'_>,
    room: usize,
    now: i64,
) -> Result<Selected, CycleError> {
    let mut selected = Selected::default();
    if room == 0 {
        return Ok(selected);
    }
    let budget = EvalBudget::new(Some(Instant::now() + CYCLE_BUDGET), Arc::default());
    let tip = kernel
        .tip_within_budget(&budget)
        .map_err(CycleError::Kernel)?;
    let descriptors = subject_descriptors(kernel, scope.project, tip, &budget)?;
    let ids: Vec<String> = descriptors
        .iter()
        .filter_map(|row| decision_of(row).map(str::to_string))
        .collect();
    let decisions: HashMap<String, DecisionRow> = kernel
        .decisions_for_objects_as_of_within_budget(&ids, tip, &budget)
        .map_err(CycleError::Kernel)?
        .into_iter()
        .map(|row| (row.object_id.clone(), row))
        .collect();
    let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let linked = kernel
        .linked_decisions_evidence_within_budget(
            &id_refs,
            tip,
            super::selection::MAX_LINKED_EVIDENCE,
            &budget,
        )
        .map_err(CycleError::Kernel)?;
    let linked: HashMap<&str, &Vec<kernel::LinkedEvidence>> =
        id_refs.iter().copied().zip(&linked).collect();
    let cited_by = |decision: &DecisionRow| -> Vec<String> {
        linked
            .get(decision.object_id.as_str())
            .into_iter()
            .flat_map(|rows| rows.iter())
            .filter(|row| row.live && row.evidence_id.len() <= MAX_IDENTITY_BYTES)
            .map(|row| row.evidence_id.clone())
            .collect()
    };
    // A memory is reviewed under the memory template, whose revision is part of every job's causal inputs, so an edited template reviews each memory with evidence once more.
    let mut policy_versions = super::handoff::review_policy_versions();
    policy_versions.insert(
        "question_template".to_string(),
        QuestionTemplate::MemoryStatement.revision(),
    );
    let subjects: Vec<(&LiveDescriptor, &DecisionRow)> = descriptors
        .iter()
        .filter_map(|descriptor| {
            let decision = decision_of(descriptor).and_then(|id| decisions.get(id))?;
            Some((descriptor, decision))
        })
        .collect();
    let rows: Vec<&DecisionRow> = subjects.iter().map(|(_, decision)| *decision).collect();
    let reopened = reopened_by_later_statements(&rows);
    let mut candidates = Vec::new();
    for (index, (descriptor, decision)) in subjects.iter().enumerate() {
        selected.examined += 1;
        let cited = cited_by(decision);
        let mut evidence: Vec<LaterEvidence> = ledger
            .candidates_for_decision(
                scope.project_digest,
                &decision.object_id,
                None,
                CANDIDATE_ROWS_PER_DECISION,
            )
            .map_err(|error| CycleError::Ledger(error.to_string()))?
            .items
            .into_iter()
            .filter(|row| {
                row.publish_tip > decision.created_commit_seq
                    && row.evidence_id.len() <= MAX_IDENTITY_BYTES
            })
            .map(|row| LaterEvidence {
                recency: row.publish_tip,
                evidence_id: row.evidence_id,
                trigger: Trigger::LaterMessage,
            })
            .collect();
        for &later in &reopened[index] {
            let statement = rows[later];
            evidence.extend(cited_by(statement).into_iter().take(1).map(|evidence_id| {
                LaterEvidence {
                    recency: statement.created_commit_seq,
                    evidence_id,
                    trigger: Trigger::LaterStatement,
                }
            }));
        }
        let work_in_progress = crate::memory_capture::states_current_work(decision);
        // The memory that contests a statement names its own source, which is what the review is shown.
        if crate::memory_capture::is_contested(decision)
            && let Some(contest) = crate::memory_capture::capture_rationale(decision).contested_by
            && let Some(contesting) = decisions.get(&contest.memory)
        {
            evidence.extend(cited_by(contesting).into_iter().take(1).map(|evidence_id| {
                LaterEvidence {
                    recency: contesting.created_commit_seq,
                    evidence_id,
                    trigger: Trigger::Contested,
                }
            }));
        }
        evidence.retain(|later| !cited.contains(&later.evidence_id));
        // Newest first; one source keeps its strongest trigger.
        evidence.sort_by(|a, b| {
            (std::cmp::Reverse(a.recency), &a.evidence_id, a.trigger).cmp(&(
                std::cmp::Reverse(b.recency),
                &b.evidence_id,
                b.trigger,
            ))
        });
        evidence.dedup_by(|a, b| a.evidence_id == b.evidence_id);
        let own_words = said_by(&decision.payload.summary) == Writer::User && !work_in_progress;
        let mut signals = Vec::new();
        let mut trigger = None;
        let mut recency = decision.created_commit_seq;
        for item in evidence {
            if signals.len() == MAX_LATER_SOURCES {
                break;
            }
            let Some(source) = memory.source(kernel, &item.evidence_id, tip, &budget)? else {
                continue;
            };
            if source.retired || (own_words && source.writer != Writer::User) {
                continue;
            }
            if signals.is_empty() {
                recency = item.recency;
            }
            trigger = Some(trigger.map_or(item.trigger, |seen: Trigger| seen.min(item.trigger)));
            signals.push(item.evidence_id);
        }
        let Some(trigger) = trigger else {
            continue;
        };
        candidates.push(Candidate {
            descriptor,
            decision,
            trigger: if work_in_progress {
                trigger.min(Trigger::CurrentWork)
            } else {
                trigger
            },
            recency,
            cited,
            signals,
        });
    }
    candidates.sort_by(|a, b| {
        (std::cmp::Reverse(a.recency), &a.decision.object_id)
            .cmp(&(std::cmp::Reverse(b.recency), &b.decision.object_id))
    });
    for candidate in candidates {
        if selected.total() >= room {
            break;
        }
        let linked_rows = linked
            .get(candidate.decision.object_id.as_str())
            .map_or(&[][..], |rows| rows.as_slice());
        let Some((mut inputs, _)) =
            super::selection::causal_inputs(candidate.descriptor, linked_rows, &policy_versions)
        else {
            continue;
        };
        inputs.question_template = QuestionTemplate::MemoryStatement.id().to_string();
        let reviewed = |inputs: &mut CausalInputs, signals: &[String]| {
            let mut signals = signals.to_vec();
            signals.sort();
            inputs.signals = signals;
            let Ok(causal_identity) = inputs.causal_identity() else {
                return Ok(true);
            };
            ledger
                .memory_reviewer_jobs_exist(scope.ledger_project, &[causal_identity.as_str()])
                .map(|exists| exists.first().copied().unwrap_or(true))
                .map_err(|error| CycleError::Ledger(error.to_string()))
        };
        if reviewed(&mut inputs, &candidate.signals)? {
            continue;
        }
        if memory
            .pending
            .entry(scope.ledger_project.to_string())
            .or_default()
            .waits(kernel, ledger, scope, &candidate.descriptor.object_id, now)
        {
            selected.waiting += 1;
            continue;
        }
        // A job reads what the store still retains, checked now: the source comes first, so every review that can read it carries it, and the later sources follow, newest first. A memory whose later sources have all been retired has nothing to be shown and is passed.
        let mut starting = Vec::new();
        for evidence in candidate.cited.iter().take(MAX_CITED_SOURCES) {
            if let Some(object_id) = memory.readable(kernel, evidence, tip, &budget)?
                && !starting.contains(&object_id)
            {
                starting.push(object_id);
            }
        }
        let cited_sources = starting.len();
        let mut signals = Vec::new();
        for evidence in &candidate.signals {
            if let Some(object_id) = memory.readable(kernel, evidence, tip, &budget)? {
                signals.push(evidence.clone());
                if !starting.contains(&object_id) {
                    starting.push(object_id);
                }
            }
        }
        // The job is named by the evidence it is shown, so a memory reviewed on these sources before is passed.
        if signals.is_empty()
            || (signals.len() < candidate.signals.len() && reviewed(&mut inputs, &signals)?)
        {
            continue;
        }
        let producer = ProducerBinding {
            producer: SELECTION_PRODUCER.to_string(),
            firing_id: format!("selection@{now}"),
            ordinal: u64::try_from(selected.total()).unwrap_or(u64::MAX),
        };
        let reserved =
            match ledger.reserve_memory_reviewer_job(scope.ledger_project, &producer, &inputs, now)
            {
                Ok(ReserveOutcome::Reserved(job)) => job,
                Ok(ReserveOutcome::Existing(_)) => continue,
                // Capacity and quota refusals hold for every later candidate of this selection.
                Err(MemoryReviewerJobError::Refused(
                    MemoryReviewerJobRefusal::ProjectCapacity
                    | MemoryReviewerJobRefusal::HostCapacity
                    | MemoryReviewerJobRefusal::MetadataQuota,
                )) => break,
                Err(MemoryReviewerJobError::Refused(_)) => continue,
                Err(error) => return Err(CycleError::Ledger(error.to_string())),
            };
        let input = MemoryReviewerJobInput {
            subject: inputs.target.clone(),
            starting_references: starting,
            question_template: QuestionTemplate::MemoryStatement.id().to_string(),
        };
        match ledger.activate_memory_reviewer_job(
            scope.ledger_project,
            &reserved.causal_identity,
            &producer,
            &input,
            now,
        ) {
            Ok(_) => {
                selected.enqueued[candidate.trigger as usize] += 1;
                eprintln!(
                    "daemon: memory_reviewer enqueued job={} trigger={} later={} source={}",
                    &reserved.causal_identity[..16.min(reserved.causal_identity.len())],
                    candidate.trigger.as_str(),
                    signals.len(),
                    cited_sources,
                );
            }
            Err(MemoryReviewerJobError::Refused(refusal)) => eprintln!(
                "daemon: memory_reviewer selection left a reserved job unactivated: {refusal}"
            ),
            Err(error) => return Err(CycleError::Ledger(error.to_string())),
        }
    }
    Ok(selected)
}

/// The decision a claim descriptor names.
fn decision_of(descriptor: &LiveDescriptor) -> Option<&str> {
    originating_decision(
        OccurrenceClass::CanonicalClaims,
        &descriptor.detail.identity,
    )
}

/// The live `decision_summary` descriptors of the project's decisions that the configured model may read, at most [`MAX_WALKED_DESCRIPTORS`] examined.
fn subject_descriptors(
    kernel: &KernelStore,
    project: &ProjectScope,
    tip: i64,
    budget: &EvalBudget,
) -> Result<Vec<LiveDescriptor>, CycleError> {
    let page_rows = NonZeroUsize::new(256).expect("nonzero");
    let mut rows = Vec::new();
    let mut after: Option<String> = None;
    let mut examined = 0;
    while examined < MAX_WALKED_DESCRIPTORS {
        let page = kernel
            .live_source_descriptors(
                OccurrenceClass::CanonicalClaims,
                tip,
                after.as_deref(),
                page_rows,
                budget,
            )
            .map_err(CycleError::Kernel)?;
        examined += page.rows.len();
        rows.extend(
            page.rows
                .into_iter()
                .filter(|row| row.detail.representation == SUBJECT_REPRESENTATION),
        );
        match page.next {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    let mut permitted = Vec::with_capacity(rows.len());
    for chunk in rows.chunks(MAX_ELIGIBILITY_CANDIDATES) {
        let candidates: Vec<EligibilityCandidate> = chunk
            .iter()
            .map(|row| EligibilityCandidate {
                object_id: row.object_id.clone(),
                source_revision: row.detail.revision.parse().unwrap_or(-1),
                artifact_digest: Some(row.detail.artifact_digest.clone()),
            })
            .collect();
        let verdicts = kernel
            .judge_surface_eligibility_within_budget(
                project,
                ArtifactDestination::ConfiguredModel,
                Surface::ExplicitSearch,
                &candidates,
                budget,
            )
            .map_err(CycleError::Kernel)?
            .verdicts;
        permitted.extend(verdicts.iter().map(|verdict| verdict.permits()));
    }
    let mut permitted = permitted.into_iter();
    rows.retain(|_| permitted.next().unwrap_or(false));
    Ok(rows)
}

/// The object id of a live source descriptor citing `evidence_id`: a message block first, then a repository document, then a tool span.
fn source_descriptor(
    kernel: &KernelStore,
    evidence_id: &str,
    tip: i64,
    budget: &EvalBudget,
) -> Result<Option<String>, CycleError> {
    for class in [
        OccurrenceClass::Messages,
        OccurrenceClass::RepositoryDocuments,
        OccurrenceClass::RawToolSpans,
    ] {
        match kernel.source_descriptor_citing_evidence(class, evidence_id, tip, budget) {
            Ok(CitingDescriptor::Live(descriptor)) => return Ok(Some(descriptor.object_id)),
            Ok(CitingDescriptor::Retired | CitingDescriptor::Uncited) => {}
            // A row of another class under the evidence fails this class's row check; the next class reads it.
            Err(KernelError::CorruptCanonicalRow) => {}
            Err(error) => return Err(CycleError::Kernel(error)),
        }
    }
    Ok(None)
}

/// What the worker did with one published proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// The mode writes nothing for this proposal, which waits for `review.accept`.
    Left,
    /// A change proposal outside the action rule: its review hold ended and the memory stays as it is.
    Dropped,
    /// `contest`: the target took a `mark_disputed` disposition and now reads `disposition`.
    Disputed { disposition: String },
    /// `apply`: the Kernel settled the proposal.
    Accepted(Box<kernel::Settlement>),
    /// The proposal could not be read back or the Kernel refused the write; the proposal stays readable for `review.accept`.
    Refused(String),
}

impl Applied {
    /// The outcome's name for the worker's log: a closed word, never proposal text.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Dropped => "dropped",
            Self::Disputed { .. } => "disputed",
            Self::Accepted(settlement) => match settlement.effect {
                kernel::SettlementEffect::Created { .. } => "created",
                kernel::SettlementEffect::Revised { .. } => "revised",
                kernel::SettlementEffect::Retired { .. } => "retired",
                kernel::SettlementEffect::Unchanged => "unchanged",
            },
            Self::Refused(_) => "refused",
        }
    }
}

/// The live store pair and project a proposal is applied under.
pub struct ApplyScope<'a> {
    pub ledger_project: &'a str,
    pub project_digest: &'a str,
    pub kernel_incarnation: &'a str,
    pub memstore_incarnation: &'a str,
}

/// Who decided that a review may change its memory.
#[derive(Debug, Clone, PartialEq)]
pub enum ActionGate {
    /// No classifier decides for the project: the chat model's proposal stands on the action rule alone.
    ChatDecides,
    /// The run went through stage 1; a change needs its record to say the classifier flagged the review.
    Classifier(Option<Stage1Record>),
}

impl ActionGate {
    fn admits_change(&self) -> bool {
        match self {
            Self::ChatDecides => true,
            Self::Classifier(record) => record
                .as_ref()
                .is_some_and(|record| record.decision == Stage1::Flagged),
        }
    }
}

/// Whether a change proposal passes the action rule: the gate admits a change, the model states `low` uncertainty, and a contradiction cites a source other than the ones the memory itself cites, which is a later reference.
fn actionable(
    kernel: &KernelStore,
    proposal: &kernel::ReviewProposal,
    target: &kernel::CanonicalTarget,
    gate: &ActionGate,
) -> bool {
    if !gate.admits_change() || proposal.uncertainty != kernel::Uncertainty::Low {
        return false;
    }
    let budget = EvalBudget::new(
        Some(Instant::now() + Duration::from_secs(5)),
        Arc::default(),
    );
    let Ok(cited) = kernel.tip_within_budget(&budget).and_then(|tip| {
        kernel.linked_decision_evidence_within_budget(
            &target.object_id,
            tip,
            super::selection::MAX_LINKED_EVIDENCE,
            &budget,
        )
    }) else {
        return false;
    };
    proposal.contradictions.iter().any(|contradiction| {
        !cited
            .iter()
            .any(|linked| linked.evidence_id == contradiction.evidence_id)
    })
}

/// Whether the memory `object_id` records the user's own statement.
fn is_user_statement(kernel: &KernelStore, object_id: &str) -> bool {
    let budget = EvalBudget::new(
        Some(Instant::now() + Duration::from_secs(5)),
        Arc::default(),
    );
    kernel
        .tip_within_budget(&budget)
        .and_then(|tip| {
            kernel.decisions_for_objects_as_of_within_budget(
                std::slice::from_ref(&object_id.to_string()),
                tip,
                &budget,
            )
        })
        .ok()
        .and_then(|rows| rows.into_iter().next())
        .is_some_and(|decision| said_by(&decision.payload.summary) == Writer::User)
}

/// Treats the proposal the job's completed receipt selects as `mode` says. A `retain` or `no_change` proposal asks for no write, so every mode accepts it at once: the acceptance leaves the memory as it is and ends the proposal's review hold, which keeps the project's hold allowance for proposals that wait on a decision.
///
/// A proposal that would change a memory passes the action rule first ([`actionable`]): the review was flagged by the classifier where one decides, the model stated `low` uncertainty, and it cites a later reference. `propose` writes nothing and leaves every change proposal for `review.accept`. In `contest` and `apply` a change proposal outside the rule is dropped: its review hold ends, the memory stays as it is, and new evidence can bring it to review again. `contest` disputes the target of a `retire` or `revise` inside the rule, citing the first contradicting evidence row; the statement leaves automatic delivery and stays on explicit search as a labeled row. `apply` accepts a `retire` through the Kernel acceptance `review.accept` runs. A `revise` of the user's own statement is disputed in `apply` as in `contest`: the user's later statement, which capture records in the user's words, is what replaces it, and the reviewer adds no model-written successor to a user statement. A `revise` of any other memory is accepted.
pub fn apply_proposal(
    kernel: &KernelStore,
    ledger: &MemoryStore,
    scope: &ApplyScope<'_>,
    causal_identity: &str,
    mode: ReviewApply,
    gate: &ActionGate,
    now: i64,
) -> Applied {
    let selected =
        match read_selected_proposal(kernel, ledger, scope.ledger_project, causal_identity, now) {
            Ok(selected) => selected,
            Err(refusal) => return Applied::Refused(refusal.to_string()),
        };
    let proposal = &selected.proposal;
    let receipt = || ledger.lookup_memory_reviewer_receipt(scope.ledger_project, causal_identity);
    let accept = || {
        let receipt = match receipt() {
            Ok(receipt) => receipt,
            Err(error) => return Applied::Refused(error.to_string()),
        };
        match accept_selected_proposal(
            kernel,
            receipt.clone(),
            (scope.kernel_incarnation, scope.memstore_incarnation),
            causal_identity,
            APPLY_OPERATION_KEY,
            &proposal.target,
        ) {
            Ok(outcome) => {
                // The settlement is the decision the review hold waited for, so the hold ends with it.
                if let Some(receipt) = receipt
                    && let Err(error) = release_settled_review_hold(
                        kernel,
                        receipt,
                        scope.memstore_incarnation,
                        causal_identity,
                        now,
                    )
                {
                    eprintln!(
                        "daemon: memory_reviewer kept the review hold of a settled proposal: {error}"
                    );
                }
                Applied::Accepted(Box::new(outcome.settlement))
            }
            Err(AcceptRefusal::Receipt(refusal)) => Applied::Refused(refusal.to_string()),
            Err(AcceptRefusal::Kernel(refusal)) => Applied::Refused(format!("{refusal:?}")),
        }
    };
    if matches!(
        proposal.action,
        ProposalAction::Retain | ProposalAction::NoChange
    ) {
        return accept();
    }
    if mode == ReviewApply::Propose {
        return Applied::Left;
    }
    let (ProposalTarget::Memory(target), true) = (
        &proposal.target,
        matches!(
            proposal.action,
            ProposalAction::Retire | ProposalAction::Revise
        ),
    ) else {
        return Applied::Left;
    };
    if !actionable(kernel, proposal, target, gate) {
        // The proposal is outside the action rule, so no reader is asked to settle it: its hold ends and the memory stays.
        return match receipt() {
            Ok(Some(receipt)) => match release_settled_review_hold(
                kernel,
                receipt,
                scope.memstore_incarnation,
                causal_identity,
                now,
            ) {
                Ok(_) => Applied::Dropped,
                Err(error) => Applied::Refused(error),
            },
            Ok(None) => Applied::Dropped,
            Err(error) => Applied::Refused(error.to_string()),
        };
    }
    let disputes = mode == ReviewApply::Contest
        || (proposal.action == ProposalAction::Revise
            && is_user_statement(kernel, &target.object_id));
    if disputes {
        let Some(contradiction) = proposal.contradictions.first() else {
            return Applied::Dropped;
        };
        return dispute(
            kernel,
            scope,
            causal_identity,
            &target.object_id,
            &contradiction.evidence_id,
        );
    }
    accept()
}

/// Records `mark_disputed` on `object_id`, citing `evidence_id`, under an operation key the job names, so a retried pass replays the first commit.
fn dispute(
    kernel: &KernelStore,
    scope: &ApplyScope<'_>,
    causal_identity: &str,
    object_id: &str,
    evidence_id: &str,
) -> Applied {
    let request = format!("{object_id}\u{1f}{evidence_id}");
    let intent = CommitIntent {
        producer: "eidnara-daemon/memory-reviewer-contest".to_string(),
        operation_key: format!("{}#{causal_identity}", scope.project_digest),
        request_digest: format!("{:x}", Sha256::digest(request.as_bytes())),
        actor: "memory-reviewer".to_string(),
        cause: "a review proposal cites a contradiction".to_string(),
    };
    let reporter = DisputeReporter {
        source_class: SourceClass::ModelInference,
        taint_class: TaintClass::MemoryClassifierInference,
        event_payload: None,
        project_digest: scope.project_digest.to_string(),
        harness: "memory_reviewer".to_string(),
        session_id: causal_identity.to_string(),
    };
    let committed = kernel.commit(intent, |envelope| {
        let (prior, _) = envelope
            .subject_admission(object_id)?
            .ok_or(KernelError::AdmissionPolicy)?;
        let decision = envelope.record_dispute_admission(
            AdmissionRequest {
                candidate_id: None,
                subject_object_id: Some(object_id.to_string()),
                source_class: Some(prior.source_class),
                taint_class: Some(prior.taint_class),
                event: AdmissionEvent {
                    kind: EventKind::MarkDisputed,
                    trigger_object_id: None,
                    approval_object_id: None,
                    evidence_id: Some(evidence_id.to_string()),
                    reason: "a memory review cited contradicting evidence".to_string(),
                },
            },
            &reporter,
        )?;
        Ok(decision.disposition.as_str().to_string())
    });
    match committed {
        Ok(receipt) => Applied::Disputed {
            disposition: receipt.result,
        },
        Err(error) => Applied::Refused(error.to_string()),
    }
}

/// Reads a refusal as text for the worker's log.
impl From<ReadRefusal> for Applied {
    fn from(refusal: ReadRefusal) -> Self {
        Self::Refused(refusal.to_string())
    }
}

/// The per-trigger counts of one selection, for the worker's log line.
pub fn describe(selected: &Selected) -> BTreeMap<&'static str, usize> {
    Trigger::ALL
        .into_iter()
        .map(Trigger::as_str)
        .zip(selected.enqueued)
        .collect()
}

/// Pending capacity the project has left for new jobs.
pub fn room(pending: usize, per_pass: usize) -> usize {
    MAX_PENDING_MEMORY_REVIEWER_JOBS_PER_PROJECT
        .saturating_sub(pending)
        .min(per_pass.saturating_sub(pending))
}

#[cfg(test)]
mod tests {
    use kernel::{DecisionPayload, Sensitivity};

    use super::*;
    use crate::memory_reviewer::classifier::Answers;

    fn decision(object_id: &str, category: &str, summary: &str, seq: i64) -> DecisionRow {
        DecisionRow {
            decision_id: format!("decision-{object_id}"),
            object_id: object_id.to_string(),
            proposition_id: None,
            scope_id: None,
            anchor_id: None,
            evidence_id: None,
            decision_kind: category.to_string(),
            payload: DecisionPayload {
                summary: summary.to_string(),
                rationale: String::new(),
            },
            created_commit_seq: seq,
            sensitivity: Sensitivity::Normal,
        }
    }

    /// The subject terms of a statement leave out who said it and the words every rule uses, and cut each term to its stem, so two statements about one subject share terms whatever their wording around it.
    #[test]
    fn subject_terms_keep_what_a_statement_is_about() {
        let terms = subject_terms("User stated: Always name test files with the suffix _spec.");
        for kept in ["nam", "test", "fil", "suffix"] {
            assert!(terms.contains(kept), "{kept} in {terms:?}");
        }
        for dropped in ["user", "stated", "always", "files", "name"] {
            assert!(!terms.contains(dropped), "{dropped} in {terms:?}");
        }
        // Inflections of one word meet at one stem, so "ending variable names" and "end variable names" share three terms.
        for (a, b) in [
            ("naming", "names"),
            ("ending", "ends"),
            ("variables", "variable"),
            ("named", "name"),
        ] {
            assert_eq!(stem(a), stem(b), "{a} and {b}");
        }
        assert_eq!(stem("class"), "class");
        assert_eq!(stem("use"), "use");
    }

    /// A later statement of the same category that shares two subject terms brings the earlier one to review. A statement of another category, one that shares a single term, and an earlier statement leave it where it is, and one later statement names at most the four earlier ones that share most with it.
    #[test]
    fn a_later_statement_about_the_same_subject_reopens_the_earlier_one() {
        let rows = [
            decision(
                "a",
                "NAMING",
                "User stated: Name test files with the suffix _spec.",
                1,
            ),
            decision("b", "NAMING", "User stated: Keep module names short.", 2),
            decision(
                "c",
                "TOOLS",
                "User stated: Test files run under the suffix runner.",
                3,
            ),
            decision(
                "d",
                "NAMING",
                "User stated: From now on test files take the suffix _test.",
                4,
            ),
        ];
        let refs: Vec<&DecisionRow> = rows.iter().collect();
        let reopened = reopened_by_later_statements(&refs);
        assert_eq!(
            reopened[0],
            [3],
            "the later naming rule about test file suffixes"
        );
        assert!(reopened[1].is_empty(), "one shared term is below the bar");
        assert!(reopened[2].is_empty(), "another category");
        assert!(
            reopened[3].is_empty(),
            "nothing later than the newest statement"
        );

        let mut many: Vec<DecisionRow> = (0..6)
            .map(|index| {
                decision(
                    &format!("m{index}"),
                    "NAMING",
                    "User stated: Name test files with the suffix _spec.",
                    index,
                )
            })
            .collect();
        many.push(decision(
            "latest",
            "NAMING",
            "User stated: Name test files with the suffix _test.",
            10,
        ));
        let refs: Vec<&DecisionRow> = many.iter().collect();
        let reopened = reopened_by_later_statements(&refs);
        let by_latest = reopened.iter().filter(|later| later.contains(&6)).count();
        assert_eq!(by_latest, REOPENED_PER_STATEMENT);
        assert!(
            reopened[5].contains(&6),
            "the newest earlier statements come first"
        );
        assert!(!reopened[0].contains(&6));
    }

    /// Where a classifier decides, a change needs its flag; a run with no record, or with a keep, admits none. Where the chat model decides, the gate is open and the rest of the action rule stands alone.
    #[test]
    fn the_gate_admits_a_change_only_on_the_classifiers_flag() {
        let record = |decision| Stage1Record {
            decision,
            answers: Answers {
                changes: 0.9,
                relation: "replaces".to_string(),
                work_ended: 0.0,
                strongest: None,
            },
        };
        assert!(ActionGate::ChatDecides.admits_change());
        assert!(ActionGate::Classifier(Some(record(Stage1::Flagged))).admits_change());
        assert!(!ActionGate::Classifier(Some(record(Stage1::Keep))).admits_change());
        assert!(!ActionGate::Classifier(None).admits_change());
    }

    #[test]
    fn statements_name_who_said_them() {
        assert_eq!(said_by("User stated: Deploy from main."), Writer::User);
        assert_eq!(
            said_by("Assistant reported: the limit is 2."),
            Writer::Assistant
        );
        assert_eq!(said_by("Deploy from main."), Writer::Unrecorded);
    }
}
