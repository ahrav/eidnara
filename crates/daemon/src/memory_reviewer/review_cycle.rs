//! What the worker does for a project whose user tier set `memory_reviewer.enabled`: it gives the bound root MODULE memories authority, keeps the project's claim descriptors published, enqueues one review job for each memory that new evidence bears on, and treats each published proposal as `memory_reviewer.apply` says.
//!
//! Selection works on decisions. A decision's `decision_summary` descriptor is the job's subject, so one decision yields one job whatever other forms it has. A memory is reviewed on evidence of change: a later source that overlaps it, the source of a later memory about the same subject, or the source of the memory that contests it. The job carries the decision's cited message blocks and those later sources as starting references, so a run reads the statement, where it came from, and what was said later. The newest later source is the job's causal signal and names it, so newer evidence reviews a memory again and the same evidence reviews it once.

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
    CausalInputs, MAX_IDENTITY_BYTES, MAX_MEMORY_REVIEWER_METADATA_BYTES_PER_HOST,
    MAX_MEMORY_REVIEWER_METADATA_BYTES_PER_PROJECT, MAX_MEMORY_REVIEWER_STARTING_REFERENCES,
    MAX_PENDING_MEMORY_REVIEWER_JOBS_PER_PROJECT, MemoryReviewerJobError, MemoryReviewerJobInput,
    MemoryReviewerJobRefusal, ProducerBinding, ReserveOutcome, ReviewTarget,
};
use memory_store::memory_reviewer_ledger::MAX_RECEIPT_PAGE;
use memory_store::{MemoryStore, MemoryStoreError};
use sha2::{Digest, Sha256};

use super::broker::{QuestionTemplate, originating_decision};
use super::settlement::{
    AcceptRefusal, ReadRefusal, accept_selected_proposal, read_selected_proposal,
    release_settled_review_hold,
};
use super::stage_one::{Writer, takes_evidence};
use crate::attestation_verifier::attest::Identity;
use crate::canonical_memory::MEMORY_DOMAIN_ID;
use crate::claim_sources::{ClaimMaterializer, ClaimProgress, MaterializationEnd};
use crate::config::ReviewApply;
use crate::contradiction_candidates::scorer::matcher_digest;

/// The context store the switch's authority rows are recorded under.
pub const SWITCH_AUTHORITY_STORE: &str = "eidnara-memory-reviewer";
/// The producer every job the switch selects is reserved under.
pub const SELECTION_PRODUCER: &str = "memory-reviewer-switch";
/// The operation key the worker accepts its own proposals under in `apply` mode.
pub const APPLY_OPERATION_KEY: &str = "memory-reviewer-switch";
/// Claim descriptors one walk examines, of every project and representation in the store, so the work of one walk's reads is bounded whatever the inventory's size. A selection runs one walk a pass and covers a larger store over successive passes. The lookup of the memory a contest names runs walk after walk to the inventory's end within one cycle budget.
pub const MAX_WALKED_DESCRIPTORS: usize = 4096;
/// Postings steps the later-statement association of one selection takes at most. A walk of 2,896 statements that all share two subject terms costs this much; a walk whose statements share more ends sooner and the next pass continues it.
pub const MAX_ASSOCIATION_STEPS: usize = 1 << 24;
/// Later overlapping sources a job carries as starting references, newest first. The newest names the job.
pub const MAX_LATER_SOURCES: usize = 4;
/// Cited message blocks a job carries as starting references.
pub const MAX_CITED_SOURCES: usize = 2;
const _: () =
    assert!(MAX_LATER_SOURCES + MAX_CITED_SOURCES <= MAX_MEMORY_REVIEWER_STARTING_REFERENCES);
/// Candidate rows read per decision.
const CANDIDATE_ROWS_PER_DECISION: usize = 16;
/// Decisions per `first_candidates_for_decisions` call.
const CANDIDATE_READ_BATCH: usize = 64;
/// The representation of a decision whose descriptor a job reviews.
const SUBJECT_REPRESENTATION: &str = "decision_summary";
/// The longest one selection or one claim catch-up runs.
const CYCLE_BUDGET: Duration = Duration::from_secs(30);
/// Claim slices one catch-up runs before it leaves the rest to the next pass.
const MAX_CLAIM_SLICES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grant {
    /// The route root resolves to active or draining memories authority.
    Held,
    /// The route root keeps its binding to another identity when the memories lookup returns `None`.
    LeftToOwner,
}

/// Gives `route_root` MODULE memories authority when no authority is bound to it, with the root itself as the authority project, so every key derived from the authority project keeps the value an unmanaged root has. A root that holds an authority, active or draining, is left as it is. The binding write reads the route again in its own transaction, so a binding another identity committed during the grant stands and the grant returns `LeftToOwner`. A grant that stopped after its acknowledgement, or that left the route to such an identity, left its row MODULE and unbound; a later call binds it while no other identity with a preparing, active, or draining authority holds the route.
pub fn grant_authority(store: &MemoryStore, route_root: &str) -> Result<Grant, MemoryStoreError> {
    grant_authority_inner(store, route_root, None)
}

/// [`grant_authority`] with `before_bind` run between the grant's authority steps and its binding write, so a test can commit an owner's binding inside that window.
fn grant_authority_inner(
    store: &MemoryStore,
    route_root: &str,
    before_bind: Option<&dyn Fn()>,
) -> Result<Grant, MemoryStoreError> {
    const DOMAIN: &str = "memories";
    if store
        .authority_project_state_for_route(route_root, DOMAIN)?
        .is_some()
    {
        return Ok(Grant::Held);
    }
    // The notes domain resolves through the same binding row, so rebinding the route would detach the owner's notes authority from it.
    let bound_to_owner = store
        .bound_authority_identity_for_route(route_root)?
        .is_some_and(|(context_store, project)| {
            context_store != SWITCH_AUTHORITY_STORE || project != route_root
        });
    if bound_to_owner {
        return Ok(Grant::LeftToOwner);
    }
    let granted = store
        .authority_status(SWITCH_AUTHORITY_STORE, route_root, DOMAIN)?
        .is_some_and(|row| row.state == "MODULE");
    if !granted {
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
        store.authority_ack_prepare(
            SWITCH_AUTHORITY_STORE,
            route_root,
            DOMAIN,
            begun.generation,
        )?;
    }
    if let Some(before_bind) = before_bind {
        before_bind();
    }
    Ok(
        if store.bind_authority_route_unless_owned(
            SWITCH_AUTHORITY_STORE,
            route_root,
            route_root,
        )? {
            Grant::Held
        } else {
            Grant::LeftToOwner
        },
    )
}

/// Runs claim-source slices from `progress` until the consumer reaches the Kernel's tip, the budget ends, a slice is blocked, or `cancelled` answers true, so every live memory decision has its descriptors before a selection reads them. `cancelled` is read before each slice, so a cancelled pass waits for one slice at most. The host's search lifecycle owner runs the same consumer where it exists; this catch-up serves a deployment without one. Returns whether the consumer reached its target; a catch-up that was cancelled, blocked, failed, or left slices for a later call returns false.
pub fn publish_claims(
    kernel: &KernelStore,
    progress: &mut ClaimProgress,
    now: i64,
    cancelled: &dyn Fn() -> bool,
) -> bool {
    let budget = EvalBudget::new(Some(Instant::now() + CYCLE_BUDGET), Arc::default());
    for _ in 0..MAX_CLAIM_SLICES {
        if cancelled() {
            return false;
        }
        let report = ClaimMaterializer::new(kernel, ProviderEgress::LocalOnly).run_slice(
            progress,
            crate::search_lifecycle_owner::CLAIM_SLICE_BOUNDS,
            &budget,
            now,
        );
        match report.map(|report| report.end) {
            Ok(MaterializationEnd::Continues) => {}
            Ok(MaterializationEnd::ReachedTarget) => return true,
            Ok(MaterializationEnd::Exhausted) => return false,
            Ok(MaterializationEnd::Blocked(blocked)) => {
                eprintln!("daemon: memory_reviewer claim catch-up is blocked: {blocked:?}");
                return false;
            }
            Err(error) => {
                eprintln!("daemon: memory_reviewer claim catch-up failed: {error}");
                return false;
            }
        }
    }
    false
}

/// Decision kinds a recorder writes from a tool event and keeps current itself: the record of a command's failure, which its next run replaces or resolves, and the record of a project file's use.
const RECORDER_KINDS: [&str; 2] = ["TOOL_FAILURE", "named_file_use"];

/// Whether selection may bring `decision` to review. A review weighs a statement against later statements; a recorder's record states none and has its own writer, so selection passes it whatever later text shares its words.
fn reviewable(decision: &DecisionRow) -> bool {
    !RECORDER_KINDS.contains(&decision.decision_kind.as_str())
}

/// What brought a memory to review. A memory is reviewed on evidence that it may have changed, and at its creation it has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Trigger {
    /// An observation contests the statement.
    Contested,
    /// The memory records work in progress and later text bears on it.
    CurrentWork,
    /// A later memory shares the statement's subject terms.
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
    /// Where each project's next selection resumes its walk of the claim descriptors, by project digest. A store with more descriptors than one selection examines is covered over successive passes.
    resume: HashMap<String, String>,
    /// The projects whose review holds were at a bound at their last selection, so the wait is logged once.
    held: BTreeSet<String>,
    /// The projects whose store was found larger than one selection examines, so that is logged once.
    large: BTreeSet<String>,
    /// Each entry identifies a project whose reservation the store's metadata quota refused during an earlier selection.
    quota: BTreeSet<String>,
    /// The bounds of one selection's walk; `None` is [`Walk::PRODUCTION`].
    walk: Option<Walk>,
}

/// The bounds of one selection's walk of the claim descriptors.
#[derive(Debug, Clone, Copy)]
pub struct Walk {
    /// Descriptors one page reads.
    pub page_rows: usize,
    /// Descriptors one selection examines before it leaves the rest to the next pass.
    pub bound: usize,
    /// Postings steps the later-statement association of one selection may take, [`MAX_ASSOCIATION_STEPS`] in production; the selection ends its walk at the last statement inside them.
    pub association_steps: usize,
}

impl Walk {
    pub(crate) const PRODUCTION: Self = Self {
        page_rows: 256,
        bound: MAX_WALKED_DESCRIPTORS,
        association_steps: MAX_ASSOCIATION_STEPS,
    };
}

/// What a selection remembers of one source: who wrote it and the commit that published it. Messages with equal text published in one commit share one evidence row and that commit; a repeat published later is a source of its own. A captured message is retained for a bounded time and a memory outlives it, so a source can be found retired.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceFacts {
    writer: Writer,
    /// The commit that published the source's descriptor. Sources are ordered by it, so a source keeps its place whatever later brings it up as evidence.
    published: i64,
    /// A message's own time in milliseconds, which a message descriptor carries as its revision. `None` for a tool span and a repository document, which revise by other counters.
    written: Option<i64>,
    /// When a read found the descriptor retired; no job reads a retired source. The lookup is repeated after [`RETIRED_RECHECK`], which bounds how long a remembered answer stands in for the store's.
    retired: Option<Instant>,
}

/// Sources remembered at once; the map starts over past this.
const MAX_REMEMBERED_SOURCES: usize = 65_536;
/// How long a source found retired is skipped before its descriptor is read again.
const RETIRED_RECHECK: Duration = Duration::from_secs(60 * 60);

impl SelectionMemory {
    /// Sets the walk's bounds, so a test covers a store larger than one selection examines.
    #[cfg(test)]
    pub fn walk_within(&mut self, walk: Walk) {
        self.walk = Some(walk);
    }

    /// The bounds of one walk of the claim descriptors.
    pub fn walk(&self) -> Walk {
        self.walk.unwrap_or(Walk::PRODUCTION)
    }

    /// The writer and publishing commit of the live source `evidence_id`, read once and remembered. `None` for a source no live descriptor cites: one not yet published, which the next pass looks up again, and one found retired, which is looked up again after [`RETIRED_RECHECK`]. A read the store fails is an error and is remembered as nothing.
    fn source(
        &mut self,
        kernel: &KernelStore,
        evidence_id: &str,
        tip: i64,
        budget: &EvalBudget,
    ) -> Result<Option<SourceFacts>, CycleError> {
        if let Some(facts) = self.sources.get(evidence_id) {
            match facts.retired {
                None => return Ok(Some(facts.clone())),
                Some(found) if found.elapsed() < RETIRED_RECHECK => return Ok(None),
                Some(_) => {}
            }
        }
        let descriptor = match source_descriptor(kernel, evidence_id, tip, budget)? {
            CitingDescriptor::Live(descriptor) => descriptor,
            CitingDescriptor::Uncited => return Ok(None),
            // A retired source is remembered as such, so the passes of the next hour skip its lookup.
            CitingDescriptor::Retired => {
                if self.sources.len() >= MAX_REMEMBERED_SOURCES {
                    self.sources.clear();
                }
                self.sources.insert(
                    evidence_id.to_string(),
                    SourceFacts {
                        writer: Writer::Unrecorded,
                        published: 0,
                        written: None,
                        retired: Some(Instant::now()),
                    },
                );
                return Ok(None);
            }
        };
        let (_, admission) = kernel
            .preview(Instant::now() + Duration::from_secs(5), |preview| {
                preview.subject_admission(&descriptor.object_id)
            })
            .map_err(CycleError::Kernel)?;
        let facts = SourceFacts {
            writer: admission.map_or(Writer::Unrecorded, |(prior, _)| {
                Writer::of_source(prior.source_class)
            }),
            published: descriptor.created_commit_seq,
            written: (descriptor.detail.class == OccurrenceClass::Messages.code())
                .then(|| descriptor.detail.revision.parse().ok())
                .flatten(),
            retired: None,
        };
        if self.sources.len() >= MAX_REMEMBERED_SOURCES {
            self.sources.clear();
        }
        self.sources.insert(evidence_id.to_string(), facts.clone());
        Ok(Some(facts))
    }

    /// When a memory was stated, in milliseconds: the time of the newest live message among `cited`, the sources the memory cites, or else the time the Kernel recorded `stated`, the commit that first recorded the memory. A memory outlives its captured messages, and a note the agent writes cites none, so the commit's time stands in for the source's. `None` when the Kernel holds no time for the commit.
    fn stated_at(
        &mut self,
        kernel: &KernelStore,
        cited: &[String],
        stated: i64,
        tip: i64,
        budget: &EvalBudget,
    ) -> Result<Option<i64>, CycleError> {
        let mut newest = None;
        for evidence_id in cited {
            if let Some(source) = self.source(kernel, evidence_id, tip, budget)? {
                newest = newest.max(source.written);
            }
        }
        if newest.is_some() {
            return Ok(newest);
        }
        Ok(kernel
            .commits_recorded_at(&[stated])
            .map_err(CycleError::Kernel)?
            .get(&stated)
            .copied())
    }

    /// The descriptor a job may read `evidence_id` through now, read from the Kernel at this call, or `None` once the store has retired it. A remembered source found retired is marked, so the passes that follow skip it.
    fn readable(
        &mut self,
        kernel: &KernelStore,
        evidence_id: &str,
        tip: i64,
        budget: &EvalBudget,
    ) -> Result<Option<String>, CycleError> {
        match source_descriptor(kernel, evidence_id, tip, budget)? {
            CitingDescriptor::Live(descriptor) => Ok(Some(descriptor.object_id)),
            CitingDescriptor::Retired | CitingDescriptor::Uncited => {
                if let Some(facts) = self.sources.get_mut(evidence_id) {
                    facts.retired = Some(Instant::now());
                }
                Ok(None)
            }
        }
    }

    /// Records that the memory `target` waits for the change proposal of the job `causal_identity`.
    pub fn note_pending(
        &mut self,
        ledger_project: &str,
        target: &str,
        causal_identity: &str,
        keep_earlier: bool,
    ) {
        let waiting = &mut self
            .pending
            .entry(ledger_project.to_string())
            .or_default()
            .waiting;
        if keep_earlier {
            waiting
                .entry(target.to_string())
                .or_insert_with(|| causal_identity.to_string());
        } else {
            waiting.insert(target.to_string(), causal_identity.to_string());
        }
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
    /// Reads the project's active review holds and the receipts that select them. A read the store fails leaves the map as it was, and the next selection reads again.
    fn rebuild(
        &mut self,
        kernel: &KernelStore,
        ledger: &MemoryStore,
        scope: &SelectionScope<'_>,
        now: i64,
    ) -> Result<(), CycleError> {
        let held: BTreeSet<String> = active_review_holds(kernel, scope.project_digest, now)?
            .into_iter()
            .collect();
        let mut waiting = HashMap::new();
        let mut after: Option<String> = None;
        let mut read = 0;
        while !held.is_empty() && read < MAX_REBUILD_RECEIPTS {
            let page = ledger
                .list_completed_memory_reviewer_receipts(
                    scope.ledger_project,
                    after.as_deref(),
                    MAX_RECEIPT_PAGE,
                )
                .map_err(|error| CycleError::Ledger(error.to_string()))?;
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
                if let Some(job) = ledger
                    .lookup_memory_reviewer_job(scope.ledger_project, &receipt.causal_identity)
                    .map_err(|error| CycleError::Ledger(error.to_string()))?
                    && let ReviewTarget::Memory { object_id, .. } = job.target
                {
                    waiting.insert(object_id, receipt.causal_identity.clone());
                }
            }
            match last {
                // The ledger serves at most one page a call, so a full page is the sign that more follow.
                Some(last) if page.len() == MAX_RECEIPT_PAGE => after = Some(last),
                _ => break,
            }
        }
        self.waiting.extend(waiting);
        self.rebuilt = true;
        Ok(())
    }

    /// Whether `target` waits for a pending change proposal. The proposal is read back: one that was settled, or whose review window ended, no longer holds the memory. While the store cannot be read the memory waits.
    fn waits(
        &mut self,
        kernel: &KernelStore,
        ledger: &MemoryStore,
        scope: &SelectionScope<'_>,
        target: &str,
        now: i64,
    ) -> Result<bool, CycleError> {
        if !self.rebuilt {
            self.rebuild(kernel, ledger, scope, now)?;
        }
        let Some(causal_identity) = self.waiting.get(target) else {
            return Ok(false);
        };
        let pending = match read_selected_proposal(
            kernel,
            ledger,
            scope.ledger_project,
            causal_identity,
            now,
        ) {
            Ok(selected) => matches!(
                selected.proposal.action,
                ProposalAction::Retire | ProposalAction::Revise
            ),
            Err(ReadRefusal::Store(error)) => return Err(CycleError::Ledger(error)),
            Err(_) => false,
        };
        if !pending {
            self.waiting.remove(target);
        }
        Ok(pending)
    }
}

/// The subjects of the project's unreleased, unexpired review holds at `now`.
fn active_review_holds(
    kernel: &KernelStore,
    project_digest: &str,
    now: i64,
) -> Result<Vec<String>, CycleError> {
    Ok(kernel
        .list_active_review_holds(now, kernel::MAX_ACTIVE_MEMORY_REVIEWER_HOLDS_PER_HOST)
        .map_err(|error| CycleError::Ledger(error.to_string()))?
        .into_iter()
        .filter(|hold| hold.binding.project_digest == project_digest)
        .map(|hold| hold.binding.subject)
        .collect())
}

/// One later source that may bear on a memory.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LaterEvidence {
    evidence_id: String,
    trigger: Trigger,
}

struct Candidate<'a> {
    descriptor: &'a LiveDescriptor,
    decision: &'a DecisionRow,
    trigger: Trigger,
    /// The commit that published the newest later source, so the newest evidence is reviewed first.
    recency: i64,
    /// Whether a session receives the memory in its block now. Such a memory is reviewed ahead of one served on explicit search alone.
    delivered: bool,
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
/// Subject terms an older memory shares with a later statement of another category before that statement brings it to review. A shared category is itself a sign of one subject, so a pair without it needs one more shared term. Capture files one rule under different categories from one session to the next ("start variable names with m_" under project rules, its replacement under naming), and this bar is what lets the replacement reach it.
pub const MIN_SHARED_SUBJECT_TERMS_ACROSS_CATEGORIES: usize = 3;
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

/// The subject terms of each decision's statement, in the decisions' order.
fn statement_terms(decisions: &[&DecisionRow]) -> Vec<BTreeSet<String>> {
    decisions
        .iter()
        .map(|decision| subject_terms(&decision.payload.summary))
        .collect()
}

/// How many of the statements, from the first, one selection associates within `steps` postings steps. [`reopened_by_later_statements`] walks, for every statement and each of its terms, the statements that hold the term, so a walk costs the sum over terms of the squared count of its holders, and one more statement adds `2n + 1` for each of its terms that `n` earlier ones hold. Admission ends before the first statement that would pass `steps`, and admits one statement always, so every walk moves its cursor.
fn admitted_within(terms: &[BTreeSet<String>], steps: usize) -> usize {
    let mut holders: HashMap<&str, usize> = HashMap::new();
    let mut cost = 0_usize;
    for (index, statement) in terms.iter().enumerate() {
        let added: usize = statement
            .iter()
            .map(|term| 2 * holders.get(term.as_str()).copied().unwrap_or(0) + 1)
            .sum();
        if index > 0 && cost.saturating_add(added) > steps {
            return index;
        }
        cost = cost.saturating_add(added);
        for term in statement {
            *holders.entry(term.as_str()).or_default() += 1;
        }
    }
    terms.len()
}

/// For each decision, the later decisions that share enough subject terms with it: [`MIN_SHARED_SUBJECT_TERMS`] within one category and [`MIN_SHARED_SUBJECT_TERMS_ACROSS_CATEGORIES`] across two. Each later decision names at most [`REOPENED_PER_STATEMENT`] earlier ones. A statement is ordered by the commit that first recorded it, which a contest's rewrite of its row leaves unchanged. This is the second way a memory comes back to review beside the ledger's candidate rows: a later statement about the same subject reaches the earlier one even when it shares too few words for a candidate row.
fn reopened_by_later_statements(
    decisions: &[&DecisionRow],
    terms: &[BTreeSet<String>],
) -> Vec<Vec<usize>> {
    let stated: Vec<i64> = decisions
        .iter()
        .map(|decision| crate::memory_capture::stated_commit(decision))
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
                if stated[earlier] < stated[later] {
                    *shared.entry(earlier).or_default() += 1;
                }
            }
        }
        let mut ranked: Vec<(usize, usize)> = shared
            .into_iter()
            .filter(|(earlier, count)| {
                let same_category =
                    decisions[*earlier].decision_kind == decisions[later].decision_kind;
                *count
                    >= if same_category {
                        MIN_SHARED_SUBJECT_TERMS
                    } else {
                        MIN_SHARED_SUBJECT_TERMS_ACROSS_CATEGORIES
                    }
            })
            .collect();
        ranked.sort_by(|a, b| {
            (
                std::cmp::Reverse(a.1),
                std::cmp::Reverse(stated[a.0]),
                &decisions[a.0].object_id,
            )
                .cmp(&(
                    std::cmp::Reverse(b.1),
                    std::cmp::Reverse(stated[b.0]),
                    &decisions[b.0].object_id,
                ))
        });
        for (earlier, _) in ranked.into_iter().take(REOPENED_PER_STATEMENT) {
            reopened[earlier].push(later);
        }
    }
    reopened
}

/// Selects the project's memories that new evidence bears on and enqueues at most `room` jobs, each Ready with its starting references: the memories a session receives automatically first, and within each group the newest evidence first, so a pass limit or a store bound that ends the selection has spent its room on the rows a session is handed. Evidence is a later source the memory does not cite, written after the memory was stated ([`written_later`]) by someone who may change it ([`takes_evidence`]): a ledger candidate row, the source of a later memory about the same subject, or the source of the memory that contests it. The newest later source names the job, so a memory is reviewed again when newer evidence arrives and once on the same evidence. A source's place in that order is the commit that published it. A memory whose change proposal is pending waits for it.
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
    let incarnation = kernel
        .database_incarnation_id_within_budget(&budget)
        .map_err(CycleError::Kernel)?;
    let walk = memory.walk();
    let (descriptors, next) = subject_descriptors(
        kernel,
        scope.project,
        tip,
        &budget,
        memory.resume.get(scope.project_digest).map(String::as_str),
        walk,
    )?;
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
            reviewable(decision).then_some((descriptor, decision))
        })
        .collect();
    let mut subjects = subjects;
    let mut next = next;
    let mut rows: Vec<&DecisionRow> = subjects.iter().map(|(_, decision)| *decision).collect();
    let mut terms = statement_terms(&rows);
    // A walk whose later-statement association would pass its step allowance ends at the last statement inside it, and the cursor resumes after that statement's descriptor, so the next pass starts with the statement that did not fit.
    let admitted = admitted_within(&terms, walk.association_steps);
    if admitted < subjects.len() {
        subjects.truncate(admitted);
        rows.truncate(admitted);
        terms.truncate(admitted);
        next = subjects
            .last()
            .map(|(descriptor, _)| descriptor.object_id.clone());
    }
    if next.is_some() && memory.large.insert(scope.project_digest.to_string()) {
        eprintln!(
            "daemon: memory_reviewer selection examines at most {} claim descriptors a pass, fewer where its statements share many terms, and the store holds more; each pass continues where the last one ended",
            walk.bound
        );
    }
    let reopened = reopened_by_later_statements(&rows, &terms);
    let mut candidates = Vec::new();
    let mut candidate_rows = Vec::new().into_iter();
    for (index, (descriptor, decision)) in subjects.iter().enumerate() {
        selected.examined += 1;
        if index % CANDIDATE_READ_BATCH == 0 {
            let batch: Vec<&str> = rows[index..]
                .iter()
                .take(CANDIDATE_READ_BATCH)
                .map(|row| row.object_id.as_str())
                .collect();
            candidate_rows = ledger
                .first_candidates_for_decisions(
                    scope.project_digest,
                    &batch,
                    CANDIDATE_ROWS_PER_DECISION,
                )
                .map_err(|error| CycleError::Ledger(error.to_string()))?
                .into_iter();
        }
        let cited = cited_by(decision);
        let stated = crate::memory_capture::stated_commit(decision);
        let mut evidence: Vec<LaterEvidence> = candidate_rows
            .next()
            .unwrap_or_default()
            .into_iter()
            // A row is evidence while it is one the candidate reader serves: written by this Kernel incarnation under the current matcher, at or before this selection's tip. The source lookup below judges its evidence's liveness.
            .filter(|row| {
                row.publish_tip > stated
                    && row.publish_tip <= tip
                    && row.kernel_incarnation_id == incarnation
                    && row.matcher_digest == matcher_digest()
                    && row.evidence_id.len() <= MAX_IDENTITY_BYTES
            })
            .map(|row| LaterEvidence {
                evidence_id: row.evidence_id,
                trigger: Trigger::LaterMessage,
            })
            .collect();
        for &later in &reopened[index] {
            let statement = rows[later];
            evidence.extend(cited_by(statement).into_iter().take(1).map(|evidence_id| {
                LaterEvidence {
                    evidence_id,
                    trigger: Trigger::LaterStatement,
                }
            }));
        }
        let work_in_progress = crate::memory_capture::states_current_work(decision);
        // The memory that contests a statement names its own source, which is what the review is shown. Two rewrites of a statement's row set a floor under its evidence, the row's commit. A contest without a recording session is one a review wrote: the statement takes as evidence only sources published after it. A statement the user settled after a contest is the user's latest word, which sources older than it leave standing.
        let mut reviewed_through =
            crate::memory_capture::is_settled(decision).then_some(decision.created_commit_seq);
        if crate::memory_capture::is_contested(decision)
            && let Some(contest) = crate::memory_capture::capture_rationale(decision).contested_by
        {
            if contest.session.is_none() {
                reviewed_through = Some(decision.created_commit_seq);
            } else if let Some(contesting) = decisions.get(&contest.memory) {
                evidence.extend(cited_by(contesting).into_iter().take(1).map(|evidence_id| {
                    LaterEvidence {
                        evidence_id,
                        trigger: Trigger::Contested,
                    }
                }));
            }
        }
        let said_by = Writer::of_statement(&decision.payload.summary);
        // When the memory was stated, read once for the first message that needs it.
        let mut stated_at: Option<Option<i64>> = None;
        // Each source the memory does not cite, with the commit that published it. A source keeps that place whichever trigger brings it up and whenever a later memory comes to cite it.
        let mut later = Vec::new();
        for (evidence_id, trigger) in strongest_triggers(evidence) {
            if cited.contains(&evidence_id) {
                continue;
            }
            let Some(source) = memory.source(kernel, &evidence_id, tip, &budget)? else {
                continue;
            };
            if !takes_evidence(said_by, work_in_progress, source.writer)
                || reviewed_through.is_some_and(|through| source.published <= through)
            {
                continue;
            }
            let stated_at = match (source.written, stated_at) {
                (None, _) => None,
                (Some(_), Some(at)) => at,
                (Some(_), None) => {
                    let at = memory.stated_at(kernel, &cited, stated, tip, &budget)?;
                    stated_at = Some(at);
                    at
                }
            };
            if !written_later(&source, stated_at, stated) {
                continue;
            }
            later.push((source.published, evidence_id, trigger));
        }
        // Newest first.
        later.sort_by(|a, b| (std::cmp::Reverse(a.0), &a.1).cmp(&(std::cmp::Reverse(b.0), &b.1)));
        later.truncate(MAX_LATER_SOURCES);
        let Some(&(recency, _, _)) = later.first() else {
            continue;
        };
        let trigger = later
            .iter()
            .map(|(_, _, trigger)| *trigger)
            .min()
            .unwrap_or(Trigger::LaterMessage);
        let signals: Vec<String> = later
            .into_iter()
            .map(|(_, evidence_id, _)| evidence_id)
            .collect();
        candidates.push(Candidate {
            descriptor,
            decision,
            trigger: if work_in_progress {
                trigger.min(Trigger::CurrentWork)
            } else {
                trigger
            },
            recency,
            delivered: false,
            cited,
            signals,
        });
    }
    if !candidates.is_empty() {
        let delivered = automatically_delivered(kernel, scope.project_digest, tip, &budget)?;
        for candidate in &mut candidates {
            candidate.delivered = delivered.contains(&candidate.decision.object_id);
        }
    }
    candidates.sort_by(|a, b| {
        (
            std::cmp::Reverse(a.delivered),
            std::cmp::Reverse(a.recency),
            &a.decision.object_id,
        )
            .cmp(&(
                std::cmp::Reverse(b.delivered),
                std::cmp::Reverse(b.recency),
                &b.decision.object_id,
            ))
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
        // A job is named by the newest later source it is shown. Newer evidence names a new job and reviews the memory again; evidence that ages out of the store leaves the name of the last review standing and starts none.
        let reviewed = |inputs: &mut CausalInputs, newest: &str| {
            inputs.signals = vec![newest.to_string()];
            let Ok(causal_identity) = inputs.causal_identity() else {
                return Ok(true);
            };
            ledger
                .memory_reviewer_jobs_exist(scope.ledger_project, &[causal_identity.as_str()])
                .map(|exists| exists.first().copied().unwrap_or(true))
                .map_err(|error| CycleError::Ledger(error.to_string()))
        };
        let Some(newest) = candidate.signals.first() else {
            continue;
        };
        if reviewed(&mut inputs, newest)? {
            continue;
        }
        if memory
            .pending
            .entry(scope.ledger_project.to_string())
            .or_default()
            .waits(kernel, ledger, scope, &candidate.descriptor.object_id, now)?
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
        // The store can retire the newest source between the two reads; the job is then named by the newest source it still shows.
        let Some(shown) = signals.first() else {
            continue;
        };
        if shown != newest && reviewed(&mut inputs, shown)? {
            continue;
        }
        inputs.signals = vec![shown.clone()];
        let producer = ProducerBinding {
            producer: SELECTION_PRODUCER.to_string(),
            firing_id: format!("selection@{now}"),
            ordinal: u64::try_from(selected.total()).unwrap_or(u64::MAX),
        };
        let reserved = match ledger.reserve_memory_reviewer_job(
            scope.ledger_project,
            &producer,
            &inputs,
            now,
        ) {
            Ok(ReserveOutcome::Reserved(job)) => job,
            Ok(ReserveOutcome::Existing(_)) => continue,
            // Capacity and quota refusals hold for every later candidate of this selection.
            Err(MemoryReviewerJobError::Refused(
                MemoryReviewerJobRefusal::ProjectCapacity | MemoryReviewerJobRefusal::HostCapacity,
            )) => break,
            Err(MemoryReviewerJobError::Refused(MemoryReviewerJobRefusal::MetadataQuota)) => {
                if memory.quota.insert(scope.project_digest.to_string()) {
                    eprintln!(
                        "daemon: memory_reviewer selection queues no new review for a project: the store's review metadata quota refused the reservation ({MAX_MEMORY_REVIEWER_METADATA_BYTES_PER_PROJECT} bytes a project, {MAX_MEMORY_REVIEWER_METADATA_BYTES_PER_HOST} a host)"
                    );
                }
                break;
            }
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
                    "daemon: memory_reviewer enqueued job={} trigger={} later={} source={} delivery={}",
                    &reserved.causal_identity[..16.min(reserved.causal_identity.len())],
                    candidate.trigger.as_str(),
                    signals.len(),
                    cited_sources,
                    if candidate.delivered {
                        "automatic"
                    } else {
                        "search"
                    },
                );
            }
            Err(MemoryReviewerJobError::Refused(refusal)) => eprintln!(
                "daemon: memory_reviewer selection left a reserved job unactivated: {refusal}"
            ),
            Err(error) => return Err(CycleError::Ledger(error.to_string())),
        }
    }
    // The walk moves on once its part of the store was examined to the end, so a selection the store fails covers the same part again.
    match next {
        Some(next) => {
            memory.resume.insert(scope.project_digest.to_string(), next);
        }
        None => {
            memory.resume.remove(scope.project_digest);
        }
    }
    Ok(selected)
}

/// Whether `source` was written after a memory was stated, which is what makes it evidence that the memory changed. A message is ordered by its own time: after `stated_at`, the time of the newest message the memory cites or, for a memory that cites none, of the commit that first recorded it. So the message a memory was taken from, a message of the same moment, and every older one are the memory's origin and never evidence against it, whenever their descriptors were published. A source with no message time, such as a tool span or a repository document, and a message beside a memory whose time the store cannot give, are ordered by commit: published after `stated`, the commit that first recorded the memory.
fn written_later(source: &SourceFacts, stated_at: Option<i64>, stated: i64) -> bool {
    match (source.written, stated_at) {
        (Some(written), Some(stated_at)) => written > stated_at,
        _ => source.published > stated,
    }
}

/// The memories of the project a session receives in its block at `tip`: the decisions of the memory domain the Kernel serves on the automatic-injection surface.
fn automatically_delivered(
    kernel: &KernelStore,
    project_digest: &str,
    tip: i64,
    budget: &EvalBudget,
) -> Result<BTreeSet<String>, CycleError> {
    Ok(kernel
        .visible_domain_decisions_as_of_in_scope_within_budget(
            Surface::AutoInject,
            tip,
            MEMORY_DOMAIN_ID,
            Some(kernel::ScopeTermFilter {
                dimension: kernel::Dimension::Project,
                value: project_digest,
            }),
            budget,
        )
        .map_err(CycleError::Kernel)?
        .rows
        .into_iter()
        .filter(|row| row.visibility == kernel::SurfaceVisibility::Visible)
        .map(|row| row.object.object_id)
        .collect())
}

/// The decision a claim descriptor names.
fn decision_of(descriptor: &LiveDescriptor) -> Option<&str> {
    originating_decision(
        OccurrenceClass::CanonicalClaims,
        &descriptor.detail.identity,
    )
}

fn names_stored_memory(descriptor: &LiveDescriptor) -> bool {
    descriptor.detail.representation == SUBJECT_REPRESENTATION
        && descriptor.domain_id == MEMORY_DOMAIN_ID
        && decision_of(descriptor)
            .is_some_and(|decision| Identity::of_attestation(decision).is_none())
}

/// The walk starts after `after` and examines at most `walk.bound` claim descriptors across all projects and representations. It returns the live `decision_summary` descriptors for this project's memory-domain decisions that the configured model may read. The cursor marks where the next walk resumes; `None` means the walk reached the end.
fn subject_descriptors(
    kernel: &KernelStore,
    project: &ProjectScope,
    tip: i64,
    budget: &EvalBudget,
    after: Option<&str>,
    walk: Walk,
) -> Result<(Vec<LiveDescriptor>, Option<String>), CycleError> {
    let page_rows = NonZeroUsize::new(walk.page_rows).unwrap_or(NonZeroUsize::MIN);
    let mut rows = Vec::new();
    let mut after: Option<String> = after.map(str::to_string);
    let mut examined = 0;
    loop {
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
        rows.extend(page.rows.into_iter().filter(names_stored_memory));
        after = page.next;
        if after.is_none() || examined >= walk.bound {
            break;
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
    Ok((rows, after))
}

/// Each source of `evidence` once, under the strongest of the triggers that found it.
fn strongest_triggers(evidence: Vec<LaterEvidence>) -> BTreeMap<String, Trigger> {
    let mut sources: BTreeMap<String, Trigger> = BTreeMap::new();
    for later in evidence {
        sources
            .entry(later.evidence_id)
            .and_modify(|trigger| *trigger = (*trigger).min(later.trigger))
            .or_insert(later.trigger);
    }
    sources
}

/// The review runs the hold bounds still admit for the project: what is left of the project's bound and of the host's. Every run a model answers ends in a proposal that takes a review hold, so with none left no run can settle: the worker selects and runs nothing for the project until a pending proposal is settled or its review window ends. The wait is logged once.
pub fn holds_left(
    kernel: &KernelStore,
    memory: &mut SelectionMemory,
    project_digest: &str,
    now: i64,
) -> Result<usize, CycleError> {
    let holds = kernel
        .list_active_review_holds(now, kernel::MAX_ACTIVE_MEMORY_REVIEWER_HOLDS_PER_HOST)
        .map_err(|error| CycleError::Ledger(error.to_string()))?;
    let held = holds
        .iter()
        .filter(|hold| hold.binding.project_digest == project_digest)
        .count();
    let left = hold_room(held, holds.len());
    if left > 0 {
        memory.held.remove(project_digest);
    } else if memory.held.insert(project_digest.to_string()) {
        eprintln!(
            "daemon: memory_reviewer reviews of a project wait: {held} of its pending proposals and {} on the host hold every review hold left; reviews resume when one is settled or its review window ends",
            holds.len()
        );
    }
    Ok(left)
}

/// What is left of the project's hold bound and of the host's, given the active review holds of each.
fn hold_room(project_holds: usize, host_holds: usize) -> usize {
    kernel::MAX_ACTIVE_MEMORY_REVIEWER_HOLDS_PER_PROJECT
        .saturating_sub(project_holds)
        .min(kernel::MAX_ACTIVE_MEMORY_REVIEWER_HOLDS_PER_HOST.saturating_sub(host_holds))
}

/// The source descriptor citing `evidence_id`, looked up as a message block first, then a repository document, then a tool span: the first live one, else whether any class holds a retired one.
fn source_descriptor(
    kernel: &KernelStore,
    evidence_id: &str,
    tip: i64,
    budget: &EvalBudget,
) -> Result<CitingDescriptor, CycleError> {
    let mut found = CitingDescriptor::Uncited;
    for class in [
        OccurrenceClass::Messages,
        OccurrenceClass::RepositoryDocuments,
        OccurrenceClass::RawToolSpans,
    ] {
        match kernel.source_descriptor_citing_evidence(class, evidence_id, tip, budget) {
            Ok(live @ CitingDescriptor::Live(_)) => return Ok(live),
            Ok(CitingDescriptor::Retired) => found = CitingDescriptor::Retired,
            Ok(CitingDescriptor::Uncited) => {}
            // A row of another class under the evidence fails this class's row check; the next class reads it.
            Err(KernelError::CorruptCanonicalRow) => {}
            Err(error) => return Err(CycleError::Kernel(error)),
        }
    }
    Ok(found)
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
    /// A `revise` of a user statement: the statement stays delivered and its line now names the later memory `by` as contesting it.
    Contested { by: String },
    /// A `revise` the mode writes nothing for, so the memory stays as it is: of a user statement the store holds no later memory for or whose row a contest cannot rewrite, and in `contest` of any other memory.
    Standing,
    /// `apply`: the Kernel settled the proposal.
    Accepted(Box<kernel::Settlement>),
    /// The proposal could not be read back or the Kernel refused the write; the proposal stays readable for `review.accept`.
    Refused(String),
}

impl Applied {
    /// Whether the proposal still holds its memory after this outcome: it waits for a reader, its target is disputed, or the write was refused and its review hold stands.
    pub fn holds_memory(&self) -> bool {
        matches!(self, Self::Left | Self::Disputed { .. } | Self::Refused(_))
    }

    /// The outcome's name for the worker's log: a closed word, never proposal text.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Dropped => "dropped",
            Self::Disputed { .. } => "disputed",
            Self::Contested { .. } => "contested",
            Self::Standing => "standing",
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
    /// The bound root the project's memories are scoped under.
    pub project_root: &'a std::path::Path,
    pub project_digest: &'a str,
    pub kernel_incarnation: &'a str,
    pub memstore_incarnation: &'a str,
    /// The bounds of one walk of the claim descriptors, as the selection's: [`SelectionMemory::walk`].
    pub walk: Walk,
}

/// The contradictions of a change proposal that pass the action rule: the model states `low` uncertainty, and the contradiction cites one of `later`, the later references that count as evidence in a review stage 1 flagged. `later` is `None` for a review stage 1 did not flag. The memory's own source, another memory and a project file the run read are none of them, so a proposal that cites only those changes nothing.
fn actionable<'a>(
    uncertainty: kernel::Uncertainty,
    contradictions: &'a [kernel::EvidenceReference],
    later: Option<&[String]>,
) -> Vec<&'a kernel::EvidenceReference> {
    let Some(later) = later else {
        return Vec::new();
    };
    if uncertainty != kernel::Uncertainty::Low {
        return Vec::new();
    }
    contradictions
        .iter()
        .filter(|contradiction| later.contains(&contradiction.evidence_id))
        .collect()
}

/// The live decision row of the memory `object_id`; `None` when the store holds none.
fn decision_row(kernel: &KernelStore, object_id: &str) -> Result<Option<DecisionRow>, KernelError> {
    let budget = EvalBudget::new(
        Some(Instant::now() + Duration::from_secs(5)),
        Arc::default(),
    );
    let tip = kernel.tip_within_budget(&budget)?;
    Ok(kernel
        .decisions_for_objects_as_of_within_budget(
            std::slice::from_ref(&object_id.to_string()),
            tip,
            &budget,
        )?
        .into_iter()
        .next())
}

/// The live memory of the project that capture stored for a later message a review cites: one recorded after `statement` that cites one of `evidence`. Among several, the one sharing most subject terms with the statement, then the newest. `None` when capture stored none.
///
/// The lookup reads the claim descriptors to their end at one tip, one walk of `scope.walk` after another from the cursor the last one returned, and keeps the best memory found so far and that cursor between walks. Each Kernel read covers the descriptors of one walk and takes the cycle budget before it acquires a reader, so a read past the deadline ends the lookup with an error, which its caller reports as a refused write.
fn later_memory_citing(
    kernel: &KernelStore,
    scope: &ApplyScope<'_>,
    statement: &DecisionRow,
    evidence: &[&str],
) -> Result<Option<String>, CycleError> {
    let budget = EvalBudget::new(Some(Instant::now() + CYCLE_BUDGET), Arc::default());
    let project = ProjectScope::new(scope.project_digest).map_err(CycleError::Kernel)?;
    let tip = kernel
        .tip_within_budget(&budget)
        .map_err(CycleError::Kernel)?;
    let stated = crate::memory_capture::stated_commit(statement);
    let terms = subject_terms(&statement.payload.summary);
    let mut best: Option<(usize, i64, std::cmp::Reverse<String>)> = None;
    let mut after: Option<String> = None;
    loop {
        let (descriptors, next) =
            subject_descriptors(kernel, &project, tip, &budget, after.as_deref(), scope.walk)?;
        let ids: Vec<&str> = descriptors
            .iter()
            .filter_map(decision_of)
            .filter(|id| *id != statement.object_id)
            .collect();
        let linked = kernel
            .linked_decisions_evidence_within_budget(
                &ids,
                tip,
                super::selection::MAX_LINKED_EVIDENCE,
                &budget,
            )
            .map_err(CycleError::Kernel)?;
        let citing: Vec<String> = ids
            .iter()
            .zip(&linked)
            .filter(|(_, rows)| {
                rows.iter()
                    .any(|row| row.live && evidence.contains(&row.evidence_id.as_str()))
            })
            .map(|(id, _)| (*id).to_string())
            .collect();
        best = kernel
            .decisions_for_objects_as_of_within_budget(&citing, tip, &budget)
            .map_err(CycleError::Kernel)?
            .into_iter()
            .filter(|row| row.created_commit_seq > stated)
            .map(|row| {
                (
                    subject_terms(&row.payload.summary)
                        .intersection(&terms)
                        .count(),
                    row.created_commit_seq,
                    std::cmp::Reverse(row.object_id),
                )
            })
            .chain(best)
            .max();
        match next {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    Ok(best.map(|(_, _, std::cmp::Reverse(object_id))| object_id))
}

/// Treats the proposal the job's completed receipt selects as `mode` says. A `retain` or `no_change` proposal asks for no write, so every mode accepts it at once: the acceptance leaves the memory as it is and ends the proposal's review hold, which keeps the project's hold allowance for proposals that wait on a decision.
///
/// A proposal that would change a memory passes the action rule first (`actionable`): stage 1 flagged the review, the model stated `low` uncertainty, and a contradiction cites one of `later`, the later references the review was shown as evidence. `propose` writes nothing and leaves every change proposal for `review.accept`. In `contest` and `apply` a change proposal outside the rule is dropped: its review hold ends, the memory stays as it is, and new evidence can bring it to review again. A memory whose row the store cannot read now is left with its proposal unapplied.
///
/// A `revise` of the user's own statement inside the rule is treated alike in `contest` and `apply`. The model proposes `revise` when a later statement replaces part of what the statement says, as when one sentence gives two rules and the user later changes one. The reviewer writes no successor text for a user statement, and removing the statement would take the part that still stands out of delivery with it. So the statement stays delivered and is marked contested by the memory capture stored for the later message: its line names that memory and both dates, and a session reads the old sentence beside the newer rule. When capture stored no memory for the cited message, the statement stays as it is.
///
/// For a `retire` inside the rule, `contest` disputes the target, citing the first later reference the proposal names as a contradiction; the statement leaves automatic delivery and stays on explicit search as a labeled row. A `revise` of a memory other than the user's statement says part of the memory still holds; `contest` writes no successor text and a dispute would take the whole row, so the memory stays as it is. `apply` accepts the proposal through the Kernel acceptance `review.accept` runs. A `revise` whose new text opens with another writer than the memory's own is dropped, so the reviewer's text never reads as the user's.
pub fn apply_proposal(
    kernel: &KernelStore,
    ledger: &MemoryStore,
    scope: &ApplyScope<'_>,
    causal_identity: &str,
    mode: ReviewApply,
    later: Option<&[String]>,
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
            &selected.reference,
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
    // A proposal the worker has settled without a reader ends its review hold and leaves `settled` as the outcome. A hold the store keeps past the settlement ends with its review window, and the outcome stands.
    let release = |settled: Applied| {
        let released =
            receipt()
                .map_err(|error| error.to_string())
                .and_then(|receipt| match receipt {
                    Some(receipt) => release_settled_review_hold(
                        kernel,
                        receipt,
                        scope.memstore_incarnation,
                        causal_identity,
                        now,
                    )
                    .map(|_| ()),
                    None => Ok(()),
                });
        if let Err(error) = released {
            eprintln!(
                "daemon: memory_reviewer kept the review hold of a settled proposal: {error}"
            );
        }
        settled
    };
    let cited = actionable(proposal.uncertainty, &proposal.contradictions, later);
    let Some(first) = cited.first() else {
        // The proposal is outside the action rule, so no reader is asked to settle it: its hold ends and the memory stays.
        return release(Applied::Dropped);
    };
    let memory = match decision_row(kernel, &target.object_id) {
        Ok(Some(memory)) => memory,
        // The memory left the store while its review ran; nothing is left to change.
        Ok(None) => return release(Applied::Dropped),
        Err(error) => return Applied::Refused(error.to_string()),
    };
    let writer = Writer::of_statement(&memory.payload.summary);
    if proposal.action == ProposalAction::Revise && writer == Writer::User {
        return match contest_with_later_statement(
            kernel,
            scope,
            causal_identity,
            &memory,
            &cited,
            now,
        ) {
            Applied::Refused(reason) => Applied::Refused(reason),
            settled => release(settled),
        };
    }
    if mode == ReviewApply::Contest {
        // A `revise` says part of the memory still holds, and a dispute would take the whole row. `contest` writes no successor text, so the row stays as it is.
        if proposal.action == ProposalAction::Revise {
            return release(Applied::Standing);
        }
        return dispute(
            kernel,
            scope,
            causal_identity,
            &target.object_id,
            &first.evidence_id,
        );
    }
    // A successor the reviewer writes opens with the memory's own attribution and carries no other writer's label anywhere.
    if proposal.action == ProposalAction::Revise
        && proposal
            .new_text
            .as_deref()
            .is_none_or(|text| Writer::of_statement(text) != writer || writer.claims_another(text))
    {
        return release(Applied::Dropped);
    }
    accept()
}

/// Marks the user statement `statement` contested by the memory capture stored for a later message among `cited`, the later references the proposal names as contradictions, under an operation key the job names, so a second call for the job replays the first commit. [`Applied::Standing`] when no live memory recorded after the statement cites those messages or the statement's row takes no contest.
fn contest_with_later_statement(
    kernel: &KernelStore,
    scope: &ApplyScope<'_>,
    causal_identity: &str,
    statement: &DecisionRow,
    cited: &[&kernel::EvidenceReference],
    now: i64,
) -> Applied {
    let cited: Vec<&str> = cited
        .iter()
        .map(|contradiction| contradiction.evidence_id.as_str())
        .collect();
    let later = match later_memory_citing(kernel, scope, statement, &cited) {
        Ok(Some(later)) => later,
        Ok(None) => return Applied::Standing,
        Err(error) => return Applied::Refused(error.to_string()),
    };
    let request = format!("{}\u{1f}{later}", statement.object_id);
    let intent = CommitIntent {
        producer: "eidnara-daemon/memory-reviewer-later-statement".to_string(),
        operation_key: format!("{}#{causal_identity}", scope.project_digest),
        request_digest: format!("{:x}", Sha256::digest(request.as_bytes())),
        actor: "memory-reviewer".to_string(),
        cause: "a review proposal cites a later statement".to_string(),
    };
    let project = crate::kernel_routes::ProjectBinding::new(scope.project_root);
    // The commit's result is the id of the row that now carries the contest, or empty when the statement kept its row.
    let committed = kernel.commit(intent, |envelope| {
        crate::memory_capture::contest_reviewed_statement(
            envelope, &project, statement, &later, now,
        )
        .map(Option::unwrap_or_default)
    });
    match committed {
        Ok(receipt) if !receipt.result.is_empty() => Applied::Contested { by: later },
        Ok(_) => Applied::Standing,
        Err(error) => Applied::Refused(error.to_string()),
    }
}

/// Records `mark_disputed` on `object_id`, citing `evidence_id`, under an operation key the job names, so a second call for the job replays the first commit.
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

/// The per-trigger counts of one selection, for the worker's log line.
pub fn describe(selected: &Selected) -> BTreeMap<&'static str, usize> {
    Trigger::ALL
        .into_iter()
        .map(Trigger::as_str)
        .zip(selected.enqueued)
        .collect()
}

/// The new jobs one selection may enqueue: what the project's pending capacity, the runs of one pass, and the review holds left ([`holds_left`]) each still admit beside the `pending` memory reviews already queued.
pub fn room(pending: usize, per_pass: usize, holds_left: usize) -> usize {
    MAX_PENDING_MEMORY_REVIEWER_JOBS_PER_PROJECT
        .min(per_pass)
        .min(holds_left)
        .saturating_sub(pending)
}

#[cfg(test)]
mod tests {
    use kernel::{DecisionPayload, Sensitivity};

    use super::*;

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

    /// Moves `identity` to `MODULE` authority for `domain` under the context store `context`.
    fn activate(store: &MemoryStore, identity: &str, domain: &str) {
        let preparing = store
            .authority_begin_prepare("context", identity, domain)
            .unwrap();
        let checksum = store
            .authority_seed_checksum("context", identity, domain)
            .unwrap();
        store
            .authority_verify_prepare(
                "context",
                identity,
                domain,
                preparing.generation,
                &checksum,
                &checksum,
            )
            .unwrap();
        store
            .authority_ack_prepare("context", identity, domain, preparing.generation)
            .unwrap();
    }

    /// A memories grant preserves the root's notes binding. The bound identity controls memories authority: the grant returns `LeftToOwner` until that identity holds `MODULE` authority for memories, then returns `Held`. An unbound root receives `MODULE` authority for memories under its own path.
    #[test]
    fn a_root_bound_to_another_identity_keeps_its_binding() {
        let dir = tempfile::tempdir().unwrap();
        let data_home = dir.path().join("data");
        std::fs::create_dir_all(&data_home).unwrap();
        let store =
            MemoryStore::open(&crate::dev_descriptor_at(data_home.to_str().unwrap())).unwrap();
        let state = |root: &str, domain: &str| {
            store
                .authority_project_state_for_route(root, domain)
                .unwrap()
        };
        let module = |project: &str| Some((project.to_string(), "MODULE".to_string()));

        let owned = "/work/owned";
        activate(&store, "git:identity", "notes");
        store
            .bind_authority_route("context", "git:identity", owned)
            .unwrap();
        assert_eq!(state(owned, "notes"), module("git:identity"));
        assert_eq!(grant_authority(&store, owned).unwrap(), Grant::LeftToOwner);
        assert_eq!(
            state(owned, "notes"),
            module("git:identity"),
            "the notes authority resolves through the route as before"
        );
        assert_eq!(state(owned, "memories"), None);

        activate(&store, "git:identity", "memories");
        assert_eq!(grant_authority(&store, owned).unwrap(), Grant::Held);
        assert_eq!(state(owned, "memories"), module("git:identity"));

        let unmanaged = "/work/unmanaged";
        assert_eq!(grant_authority(&store, unmanaged).unwrap(), Grant::Held);
        assert_eq!(state(unmanaged, "memories"), module(unmanaged));
        assert_eq!(grant_authority(&store, unmanaged).unwrap(), Grant::Held);
    }

    /// An owner's binding committed after the grant read the route and before the grant's own binding write stands: the write leaves the route to the owner's identity, the grant returns `LeftToOwner`, and the owner's notes authority resolves through the route as before.
    #[test]
    fn an_owner_binding_committed_during_the_grant_stands() {
        let dir = tempfile::tempdir().unwrap();
        let data_home = dir.path().join("data");
        std::fs::create_dir_all(&data_home).unwrap();
        let store =
            MemoryStore::open(&crate::dev_descriptor_at(data_home.to_str().unwrap())).unwrap();
        let raced = "/work/raced";
        let owner_binds = || {
            activate(&store, "git:identity", "notes");
            store
                .bind_authority_route("context", "git:identity", raced)
                .unwrap();
        };
        assert_eq!(
            grant_authority_inner(&store, raced, Some(&owner_binds)).unwrap(),
            Grant::LeftToOwner
        );
        assert_eq!(
            store.bound_authority_identity_for_route(raced).unwrap(),
            Some(("context".to_string(), "git:identity".to_string()))
        );
        assert_eq!(
            store
                .authority_project_state_for_route(raced, "notes")
                .unwrap(),
            Some(("git:identity".to_string(), "MODULE".to_string()))
        );
        assert_eq!(
            store
                .authority_project_state_for_route(raced, "memories")
                .unwrap(),
            None
        );
        assert_eq!(grant_authority(&store, raced).unwrap(), Grant::LeftToOwner);
    }

    /// The grant's binding write replaces a binding to an identity that holds no authority, and replays a binding that already names the switch's identity.
    #[test]
    fn the_grant_binds_over_a_binding_with_no_authority_and_replays_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let data_home = dir.path().join("data");
        std::fs::create_dir_all(&data_home).unwrap();
        let store =
            MemoryStore::open(&crate::dev_descriptor_at(data_home.to_str().unwrap())).unwrap();
        let switch_identity =
            |root: &str| Some((SWITCH_AUTHORITY_STORE.to_string(), root.to_string()));

        let ended = "/work/ended";
        store
            .bind_authority_route("context", "git:identity", ended)
            .unwrap();
        assert_eq!(grant_authority(&store, ended).unwrap(), Grant::Held);
        assert_eq!(
            store.bound_authority_identity_for_route(ended).unwrap(),
            switch_identity(ended)
        );

        let twice = "/work/twice";
        let switch_binds = || {
            store
                .bind_authority_route(SWITCH_AUTHORITY_STORE, twice, twice)
                .unwrap();
        };
        assert_eq!(
            grant_authority_inner(&store, twice, Some(&switch_binds)).unwrap(),
            Grant::Held
        );
        assert_eq!(
            store.bound_authority_identity_for_route(twice).unwrap(),
            switch_identity(twice)
        );
    }

    /// A proposal that waits for a reader, a disputed target, and a refused write each hold their memory; every other outcome frees it.
    #[test]
    fn the_outcomes_that_hold_a_memory() {
        assert!(Applied::Left.holds_memory());
        assert!(
            Applied::Disputed {
                disposition: "disputed".to_string()
            }
            .holds_memory()
        );
        assert!(Applied::Refused("the store is busy".to_string()).holds_memory());
        assert!(!Applied::Dropped.holds_memory());
        assert!(!Applied::Standing.holds_memory());
        assert!(
            !Applied::Contested {
                by: "mem_1".to_string()
            }
            .holds_memory()
        );
    }

    /// A refused proposal takes its memory's pending place only when no earlier proposal holds it; a proposal left for a reader replaces the entry.
    #[test]
    fn a_refused_proposal_keeps_an_earlier_pending_one() {
        let mut memory = SelectionMemory::default();
        memory.note_pending("project", "mem_1", "job-a", false);
        memory.note_pending("project", "mem_1", "job-b", true);
        assert_eq!(memory.pending["project"].waiting["mem_1"], "job-a");
        memory.note_pending("project", "mem_2", "job-c", true);
        assert_eq!(memory.pending["project"].waiting["mem_2"], "job-c");
        memory.note_pending("project", "mem_1", "job-d", false);
        assert_eq!(memory.pending["project"].waiting["mem_1"], "job-d");
    }

    /// The association of a walk costs, for each term, the square of the number of statements that hold it. A selection admits the statements, from the first, whose association fits the allowance, and always one.
    #[test]
    fn a_walk_is_admitted_while_its_association_fits_the_allowance() {
        let terms = |words: &[&str]| -> BTreeSet<String> {
            words.iter().map(|word| (*word).to_string()).collect()
        };
        // Statements that share two terms: one costs 2 steps, two cost 8, three 18, four 32.
        let shared = vec![terms(&["release", "branch"]); 4];
        for (steps, admitted) in [(32, 4), (31, 3), (18, 3), (17, 2), (8, 2), (7, 1), (0, 1)] {
            assert_eq!(admitted_within(&shared, steps), admitted, "{steps}");
        }
        // Statements with no term in common cost their own terms: 2 steps each.
        let apart = [
            terms(&["release", "branch"]),
            terms(&["linter", "commit"]),
            terms(&["suffix", "test"]),
        ];
        assert_eq!(admitted_within(&apart, 4), 2);
        assert_eq!(admitted_within(&apart, 6), 3);
        assert_eq!(admitted_within(&[], 0), 0);
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

    /// A later statement of the same category that shares two subject terms brings the earlier one to review. A statement of another category that shares two, one that shares a single term, and an earlier statement leave it where it is, and one later statement names at most the four earlier ones that share most with it.
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
                "User stated: The runner picks up test files by glob.",
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
        let reopened = reopened_by_later_statements(&refs, &statement_terms(&refs));
        assert_eq!(
            reopened[0],
            [3],
            "the later naming rule about test file suffixes; the tooling statement shares two terms across categories, below that bar"
        );
        assert!(reopened[1].is_empty(), "one shared term is below the bar");
        assert!(
            reopened[2].is_empty(),
            "two shared terms across categories are below the bar"
        );
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
        let reopened = reopened_by_later_statements(&refs, &statement_terms(&refs));
        let by_latest = reopened.iter().filter(|later| later.contains(&6)).count();
        assert_eq!(by_latest, REOPENED_PER_STATEMENT);
        assert!(
            reopened[5].contains(&6),
            "the newest earlier statements come first"
        );
        assert!(!reopened[0].contains(&6));
    }

    /// Capture can file one rule under two categories. A later statement of another category brings the earlier one to review when they share three subject terms, here the variable naming rule a project-rules sentence also holds.
    #[test]
    fn a_later_statement_of_another_category_reopens_a_statement_that_shares_three_terms() {
        let rows = [
            decision(
                "rules",
                "PROJECT_RULES",
                "User stated: Annotate every method, and start variable names with m_.",
                1,
            ),
            decision(
                "style",
                "STYLE",
                "User stated: Keep variable declarations at the top of a block.",
                2,
            ),
            decision(
                "naming",
                "NAMING",
                "User stated: From now on start all variable names with y_.",
                3,
            ),
        ];
        let refs: Vec<&DecisionRow> = rows.iter().collect();
        let reopened = reopened_by_later_statements(&refs, &statement_terms(&refs));
        assert_eq!(
            reopened[0],
            [2],
            "start, variable and names are shared across the two categories"
        );
        assert!(
            reopened[1].is_empty(),
            "one shared term across categories: {:?}",
            reopened[1]
        );
    }

    /// A record a recorder keeps current itself is no subject of a review: the record of a failed command and the record of a named file's use are passed, and a statement capture filed under any of its categories is reviewed.
    #[test]
    fn a_recorders_record_is_passed_by_selection() {
        let failure = decision(
            "failure",
            "TOOL_FAILURE",
            "cargo test exited with code 101 in session s1.",
            1,
        );
        let named = decision("named", "named_file_use", "docs/release.md", 2);
        assert!(!reviewable(&failure));
        assert!(!reviewable(&named));
        for category in [
            "PROJECT_RULES",
            "ARCHITECTURE",
            "CONSTRAINTS",
            "CONFIG_VALUES",
            "NAMING",
        ] {
            let statement = decision("rule", category, "User stated: Run cargo test.", 3);
            assert!(reviewable(&statement), "{category}");
        }
    }

    /// One source two triggers found takes one of a job's later slots, under its strongest trigger.
    #[test]
    fn a_source_found_twice_is_one_piece_of_evidence() {
        let later = |evidence_id: &str, trigger| LaterEvidence {
            evidence_id: evidence_id.to_string(),
            trigger,
        };
        let sources = strongest_triggers(vec![
            later("a", Trigger::LaterMessage),
            later("b", Trigger::LaterMessage),
            later("a", Trigger::LaterStatement),
            later("b", Trigger::LaterMessage),
            later("a", Trigger::Contested),
        ]);
        assert_eq!(
            sources.into_iter().collect::<Vec<_>>(),
            [
                ("a".to_string(), Trigger::Contested),
                ("b".to_string(), Trigger::LaterMessage),
            ]
        );
    }

    /// The reviews the hold bounds still admit are what is left of the project's 64 and of the host's 256, whichever is less.
    #[test]
    fn the_hold_bounds_admit_what_the_project_and_the_host_have_left() {
        let project = kernel::MAX_ACTIVE_MEMORY_REVIEWER_HOLDS_PER_PROJECT;
        let host = kernel::MAX_ACTIVE_MEMORY_REVIEWER_HOLDS_PER_HOST;
        assert_eq!(hold_room(0, 0), project);
        assert_eq!(hold_room(project - 2, project - 2), 2);
        assert_eq!(hold_room(project, project), 0);
        assert_eq!(hold_room(project + 1, project + 1), 0);
        assert_eq!(hold_room(3, host - 1), 1, "the host's bound is nearer");
        assert_eq!(hold_room(0, host), 0);
    }

    /// A selection enqueues what the pending capacity, the pass, and the holds left each still admit beside the jobs already queued.
    #[test]
    fn a_selection_enqueues_what_capacity_the_pass_and_the_holds_admit() {
        assert_eq!(room(0, 8, 64), 8);
        assert_eq!(room(3, 8, 64), 5);
        assert_eq!(room(8, 8, 64), 0);
        assert_eq!(room(0, 8, 2), 2, "two review holds are left");
        assert_eq!(room(1, 8, 2), 1);
        assert_eq!(room(2, 8, 2), 0);
        assert_eq!(room(0, 8, 0), 0);
    }

    fn citing(evidence: &[&str]) -> Vec<kernel::EvidenceReference> {
        evidence
            .iter()
            .map(|evidence_id| kernel::EvidenceReference {
                evidence_id: (*evidence_id).to_string(),
                span: None,
            })
            .collect()
    }

    /// The action rule has three conditions and each holds alone: stage 1 flagged the review, the model states `low` uncertainty, and a contradiction cites a later reference of the review. A citation of the memory's own source, of another memory, or of a project file is none.
    #[test]
    fn a_change_acts_only_on_a_later_reference_of_a_flagged_review() {
        use kernel::Uncertainty::{Low, Medium};
        let later = ["later-message".to_string()];
        let cited = |uncertainty, contradictions: &[kernel::EvidenceReference], later| {
            actionable(uncertainty, contradictions, later)
                .into_iter()
                .map(|reference| reference.evidence_id.clone())
                .collect::<Vec<_>>()
        };
        let change = citing(&["own-source", "later-message"]);
        assert_eq!(cited(Low, &change, Some(&later[..])), ["later-message"]);
        assert!(
            cited(Low, &change, None).is_empty(),
            "stage 1 did not flag the review"
        );
        assert!(cited(Medium, &change, Some(&later[..])).is_empty());
        for elsewhere in ["own-source", "another-memory", "project-file"] {
            assert!(
                cited(Low, &citing(&[elsewhere]), Some(&later[..])).is_empty(),
                "{elsewhere}"
            );
        }
        assert!(cited(Low, &change, Some(&[][..])).is_empty());
    }
}
