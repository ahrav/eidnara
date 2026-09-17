//! Required occurrences complete before optional work, under an integer
//! `ClaudeTokens` budget.

pub mod accounting;
pub mod render;

use std::fmt;
use std::num::NonZeroUsize;

pub use accounting::{AccountingProfile, Authority, Charge, DECLARED_UNCHARGED};
pub use render::{
    AccountingBound, AccountingBounds, AccountingExceeded, BLOCK_CLOSE_FRAGMENT,
    BLOCK_OPEN_FRAGMENT, Charged, Ledger, LedgerEntry, admit_render,
};

use kernel::applicability::EvalBudget;
use kernel::{
    ArtifactDestination, KernelError, KernelStore, MAX_ELIGIBILITY_CANDIDATES, ProjectScope,
};
use retrieval::ProjectionError;
use retrieval::eligibility::{Disposition, judge_occurrences_within_budget};
use retrieval::fusion::OccurrenceId;
use retrieval::packing::{
    BoundExceeded, Group, OptionalBounds, RequiredBound, RequiredBounds, RequiredContextFailure,
    RequiredFact, RequiredRequest, Selected, SelectedOccurrence, TokenCount, Ungrouped,
    admit_fused_candidates, admit_optional_set, admit_required, fetch_payload, group,
    read_selected, reserve_required, skip_and_continue,
};
use storage::{GuardedConn, SqliteStore, StoreError};

/// Provider accounting uses a type distinct from
/// [`host_runtime::local_embeddings::EmbedTokens`].
///
/// ```
/// let claude = daemon::packing::ClaudeTokens::new(1);
/// let embed = host_runtime::local_embeddings::EmbedTokens::new(1);
/// assert_eq!(claude.get(), u64::from(embed.get()));
/// ```
///
/// ```compile_fail,E0308
/// let _: daemon::packing::ClaudeTokens = host_runtime::local_embeddings::EmbedTokens::new(1);
/// ```
///
/// ```compile_fail,E0308
/// let _: host_runtime::local_embeddings::EmbedTokens = daemon::packing::ClaudeTokens::new(1);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClaudeTokens(u64);

impl ClaudeTokens {
    pub const fn new(count: u64) -> Self {
        Self(count)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    /// A fractional, negative, non-finite, over-range, or absent budget is
    /// refused, never rounded, saturated, or clamped. Negative zero is
    /// refused as negative.
    pub fn from_budget(value: Option<f64>) -> Result<Self, BudgetRefusal> {
        const TWO_TO_THE_64: f64 = 18_446_744_073_709_551_616.0;
        let value = value.ok_or(BudgetRefusal::Absent)?;
        if value.is_nan() {
            return Err(BudgetRefusal::NotANumber);
        }
        if value.is_sign_negative() {
            return Err(BudgetRefusal::Negative);
        }
        if !value.is_finite() || value >= TWO_TO_THE_64 {
            return Err(BudgetRefusal::TooLarge);
        }
        if value.fract() != 0.0 {
            return Err(BudgetRefusal::NonInteger);
        }
        Ok(Self(value as u64))
    }
}

impl TokenCount for ClaudeTokens {
    const ZERO: Self = Self(0);
    const MAX: Self = Self(u64::MAX);

    fn checked_add(self, other: Self) -> Option<Self> {
        self.0.checked_add(other.0).map(Self)
    }

    fn checked_sub(self, other: Self) -> Option<Self> {
        self.0.checked_sub(other.0).map(Self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetRefusal {
    Absent,
    NotANumber,
    Negative,
    NonInteger,
    TooLarge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequiredEvent {
    Read,
    Judged,
    Admitted,
    Loaded,
    Reserved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionalEvent {
    Read,
    Judged,
    Bounded,
    Loaded,
    Grouped,
    Scanned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageEvent {
    Required(RequiredEvent, Option<OccurrenceId>),
    Optional(OptionalEvent, Option<OccurrenceId>),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackingTrace {
    events: Vec<StageEvent>,
    retrieval_calls: u64,
    payload_loads: u64,
}

impl PackingTrace {
    pub fn events(&self) -> &[StageEvent] {
        &self.events
    }

    pub fn optional_events(&self) -> usize {
        self.events
            .iter()
            .filter(|event| matches!(event, StageEvent::Optional(..)))
            .count()
    }

    pub fn retrieval_calls(&self) -> u64 {
        self.retrieval_calls
    }

    pub fn payload_loads(&self) -> u64 {
        self.payload_loads
    }

    /// Candidate-retrieving lanes call this; the packer never does.
    pub fn note_retrieval_call(&mut self) {
        self.retrieval_calls += 1;
    }

    fn required(&mut self, event: RequiredEvent, occurrence: Option<OccurrenceId>) {
        self.events.push(StageEvent::Required(event, occurrence));
    }

    fn optional(&mut self, event: OptionalEvent, occurrence: Option<OccurrenceId>) {
        self.events.push(StageEvent::Optional(event, occurrence));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreparationRefusal {
    Required(RequiredContextFailure<ClaudeTokens>),
    OptionalBound(BoundExceeded),
    Accounting(AccountingExceeded),
    /// The optional phase was offered a profile other than the one the
    /// required ledger was charged under.
    ProfileMismatch,
    /// An optional group's summed fragment charges are not representable;
    /// `at` is the group's fused position. Nothing is saturated into an
    /// admissible cost.
    OptionalCostOverflow {
        at: usize,
    },
    Deadline,
    Projection(ProjectionError),
    Storage(String),
    Kernel(KernelError),
}

impl From<RequiredContextFailure<ClaudeTokens>> for PreparationRefusal {
    fn from(failure: RequiredContextFailure<ClaudeTokens>) -> Self {
        Self::Required(failure)
    }
}

impl From<KernelError> for PreparationRefusal {
    fn from(error: KernelError) -> Self {
        match error {
            KernelError::Deadline => Self::Deadline,
            other => Self::Kernel(other),
        }
    }
}

impl From<ProjectionError> for PreparationRefusal {
    fn from(error: ProjectionError) -> Self {
        match error {
            ProjectionError::Interrupted => Self::Deadline,
            other => Self::Projection(other),
        }
    }
}

#[derive(Clone, Copy)]
pub struct RequiredInputs<'a> {
    pub kernel: &'a KernelStore,
    pub project: &'a ProjectScope,
    pub destination: ArtifactDestination,
    pub budget: &'a EvalBudget,
    pub profile: &'a AccountingProfile,
}

#[derive(Clone, PartialEq, Eq)]
pub struct MaterializedRequired {
    pub occurrence: OccurrenceId,
    pub revision: i64,
    pub bytes: Vec<u8>,
    pub cost: ClaudeTokens,
}

/// Payloads are never logged; the byte length stands in for the content.
impl fmt::Debug for MaterializedRequired {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MaterializedRequired")
            .field("occurrence", &self.occurrence)
            .field("revision", &self.revision)
            .field("byte_length", &self.bytes.len())
            .field("cost", &self.cost)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredMaterialization {
    items: Vec<MaterializedRequired>,
    charged: ClaudeTokens,
    remaining: ClaudeTokens,
    ledger: Ledger,
}

impl RequiredMaterialization {
    pub fn items(&self) -> &[MaterializedRequired] {
        &self.items
    }

    /// The block open and every required item's rendered delta, with
    /// headroom.
    pub fn charged(&self) -> ClaudeTokens {
        self.charged
    }

    /// The token limit left for optional work.
    pub fn remaining(&self) -> ClaudeTokens {
        self.remaining
    }

    /// The block open and the required fragments rendered so far.
    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }
}

/// The kernel judges at most `MAX_ELIGIBILITY_CANDIDATES` per batch, so a
/// caller bound above it cannot be honored.
const KERNEL_BATCH_CAP: NonZeroUsize = NonZeroUsize::new(MAX_ELIGIBILITY_CANDIDATES).unwrap();

fn deadline(inputs: &RequiredInputs<'_>) -> Result<(), PreparationRefusal> {
    if inputs.budget.is_exhausted() {
        Err(PreparationRefusal::Deadline)
    } else {
        Ok(())
    }
}

/// Without a deadline, only the polls between stages bound the phase.
fn hold<T>(
    store: &SqliteStore,
    budget: &EvalBudget,
    f: impl FnOnce(&GuardedConn<'_>) -> rusqlite::Result<T>,
) -> Result<T, PreparationRefusal> {
    let outcome = match budget.deadline() {
        Some(deadline) => {
            let budget = budget.clone();
            store.with_conn_interruptible(deadline, move || budget.is_exhausted(), f)
        }
        None => store.with_conn(f),
    };
    outcome.map_err(|error| match error {
        StoreError::Deadline => PreparationRefusal::Deadline,
        other => PreparationRefusal::Storage(other.to_string()),
    })
}

fn until_first_fault<T, E>(results: impl Iterator<Item = Result<T, E>>) -> (Vec<T>, Option<E>) {
    let mut values = Vec::new();
    for result in results {
        match result {
            Ok(value) => values.push(value),
            Err(error) => return (values, Some(error)),
        }
    }
    (values, None)
}

/// An optional row that is missing or corrupt is excluded and the scan
/// continues; any other fault, including an interrupted statement, ends the
/// statements at once as in the required phase.
fn excludable<T>(
    result: Result<T, ProjectionError>,
) -> Result<Result<T, OptionalExclusion>, ProjectionError> {
    match result {
        Ok(value) => Ok(Ok(value)),
        Err(ProjectionError::UnknownOccurrence { .. }) => Ok(Err(OptionalExclusion::Missing)),
        Err(ProjectionError::CorruptRow) => Ok(Err(OptionalExclusion::Corrupt)),
        Err(fault) => Err(fault),
    }
}

pub fn prepare_required(
    store: &SqliteStore,
    inputs: RequiredInputs<'_>,
    requests: &[RequiredRequest],
    bounds: &RequiredBounds<ClaudeTokens>,
    accounting: &AccountingBounds,
    trace: &mut PackingTrace,
) -> Result<RequiredMaterialization, PreparationRefusal> {
    deadline(&inputs)?;
    let load_cap = bounds.max_payload_loads.min(KERNEL_BATCH_CAP);
    let bounds = &RequiredBounds {
        max_payload_loads: load_cap,
        ..*bounds
    };
    if let Some(beyond) = requests.get(load_cap.get()) {
        return Err(RequiredContextFailure::Oversized {
            occurrence: beyond.occurrence,
            bound: RequiredBound::PayloadLoads,
        }
        .into());
    }
    let (selected, fault) = hold(store, inputs.budget, |conn| {
        Ok(until_first_fault(requests.iter().map(|request| {
            read_selected(conn, &[request.occurrence], NonZeroUsize::MIN)
                .map(|mut rows| rows.swap_remove(0))
                .map_err(|error| (request.occurrence, error))
        })))
    })?;
    for row in &selected {
        trace.required(RequiredEvent::Read, Some(row.occurrence));
    }
    if let Some((occurrence, error)) = fault {
        return Err(match error {
            ProjectionError::UnknownOccurrence { .. } => {
                RequiredContextFailure::Missing(occurrence).into()
            }
            ProjectionError::CorruptRow => RequiredContextFailure::Corrupt(occurrence).into(),
            other => other.into(),
        });
    }

    deadline(&inputs)?;
    let candidates: Vec<_> = selected
        .iter()
        .map(SelectedOccurrence::eligibility_candidate)
        .collect();
    let report = judge_occurrences_within_budget(
        inputs.kernel,
        inputs.project,
        inputs.destination,
        &candidates,
        inputs.budget,
    )?;
    trace.required(RequiredEvent::Judged, None);
    let facts: Vec<RequiredFact<'_>> = requests
        .iter()
        .zip(&selected)
        .zip(&report.occurrences)
        .map(|((request, row), judged)| RequiredFact {
            request: *request,
            row,
            disposition: judged.disposition,
        })
        .collect();
    let admitted = admit_required(&facts, bounds)?;
    trace.required(RequiredEvent::Admitted, None);

    deadline(&inputs)?;
    let (bytes, fault) = hold(store, inputs.budget, |conn| {
        Ok(until_first_fault(admitted.iter().map(|item| {
            fetch_payload(conn, &item.row().payload).map_err(|error| (item.row().occurrence, error))
        })))
    })?;
    // Payload verification runs after `hold` releases the connection; a fetch
    // fault is returned only after verifying earlier payloads, preserving
    // request order.
    for (item, payload) in admitted.iter().zip(&bytes) {
        let occurrence = item.row().occurrence;
        item.row()
            .payload
            .verify(payload)
            .map_err(|_| RequiredContextFailure::Corrupt(occurrence))?;
        trace.payload_loads += 1;
        trace.required(RequiredEvent::Loaded, Some(occurrence));
    }
    if let Some((occurrence, error)) = fault {
        return Err(match error {
            ProjectionError::CorruptRow => RequiredContextFailure::Corrupt(occurrence).into(),
            other => other.into(),
        });
    }
    let borrowed: Vec<&[u8]> = bytes.iter().map(Vec::as_slice).collect();
    let mut ledger = Ledger::open(inputs.profile.clone());
    let block_open = ledger.total_with_headroom();
    let Some(items_limit) = bounds.token_limit.checked_sub(block_open) else {
        return Err(RequiredContextFailure::OverBudget {
            limit: bounds.token_limit,
            charged: block_open,
        }
        .into());
    };
    let reservation = reserve_required(&admitted, &borrowed, items_limit, |item, bytes| {
        let occurrence = item.row().occurrence;
        ledger
            .append(
                Charged::Required(occurrence),
                &render::required_fragment(occurrence, bytes),
            )
            .with_headroom()
    })
    .map_err(|failure| match failure {
        RequiredContextFailure::OverBudget { charged, .. } => RequiredContextFailure::OverBudget {
            limit: bounds.token_limit,
            charged: charged.checked_add(block_open).unwrap_or(ClaudeTokens::MAX),
        },
        other => other,
    })?;
    trace.required(RequiredEvent::Reserved, None);
    admit_render(&ledger, accounting).map_err(PreparationRefusal::Accounting)?;
    let charged = ledger.total_with_headroom();

    let items = admitted
        .iter()
        .zip(bytes)
        .zip(reservation.costs)
        .map(|((item, bytes), cost)| MaterializedRequired {
            occurrence: item.row().occurrence,
            revision: item.row().revision,
            bytes,
            cost,
        })
        .collect();
    Ok(RequiredMaterialization {
        items,
        charged,
        remaining: bounds
            .token_limit
            .checked_sub(charged)
            .unwrap_or(ClaudeTokens::ZERO),
        ledger,
    })
}

pub type OptionalRequest = RequiredRequest;

/// Why an optional occurrence left the scan before grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionalExclusion {
    /// A later request for an identity an earlier request already named.
    Duplicate,
    Missing,
    Stale,
    Corrupt,
    Excluded(kernel::EligibilityVerdict),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostedGroup {
    pub group: Group,
    pub cost: ClaudeTokens,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionalAdmission {
    /// The closed render: block, required items, admitted groups.
    pub ledger: Ledger,
    pub admitted: Vec<CostedGroup>,
    /// Groups visited and skipped, in fused order.
    pub skipped: Vec<CostedGroup>,
    /// In the order the exclusions were found: duplicates, then reads, then
    /// judgments, then loads. `Duplicate` names a later request, so an
    /// identity requested twice can appear here as `Duplicate` and again
    /// with the reason its first request earned, or be admitted.
    pub excluded: Vec<(OccurrenceId, OptionalExclusion)>,
    pub ungrouped: Vec<(OccurrenceId, Ungrouped)>,
    pub remaining: ClaudeTokens,
}

/// Runs after [`prepare_required`] over the budget it left; a required
/// materialization is the witness that the required phase completed.
pub fn prepare_optional(
    store: &SqliteStore,
    inputs: RequiredInputs<'_>,
    required: &RequiredMaterialization,
    requests: &[OptionalRequest],
    bounds: &OptionalBounds,
    accounting: &AccountingBounds,
    trace: &mut PackingTrace,
) -> Result<OptionalAdmission, PreparationRefusal> {
    deadline(&inputs)?;
    if inputs.profile != required.ledger.profile() {
        return Err(PreparationRefusal::ProfileMismatch);
    }
    let bounds = &OptionalBounds {
        max_fused_candidates: bounds.max_fused_candidates.min(KERNEL_BATCH_CAP),
        ..*bounds
    };
    admit_fused_candidates(requests.len(), bounds).map_err(PreparationRefusal::OptionalBound)?;
    let mut excluded = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let requests: Vec<OptionalRequest> = requests
        .iter()
        .filter(|request| {
            let first = seen.insert(request.occurrence);
            if !first {
                excluded.push((request.occurrence, OptionalExclusion::Duplicate));
            }
            first
        })
        .copied()
        .collect();
    let (reads, fault) = hold(store, inputs.budget, |conn| {
        Ok(until_first_fault(requests.iter().map(|request| {
            excludable(
                read_selected(conn, &[request.occurrence], NonZeroUsize::MIN)
                    .map(|mut rows| rows.swap_remove(0)),
            )
        })))
    })?;
    let mut live: Vec<(OptionalRequest, SelectedOccurrence)> = Vec::new();
    for (request, read) in requests.iter().zip(reads) {
        match read {
            Ok(row) => live.push((*request, row)),
            Err(exclusion) => excluded.push((request.occurrence, exclusion)),
        }
        trace.optional(OptionalEvent::Read, Some(request.occurrence));
    }
    if let Some(fault) = fault {
        return Err(fault.into());
    }

    deadline(&inputs)?;
    let candidates: Vec<_> = live
        .iter()
        .map(|(_, row)| row.eligibility_candidate())
        .collect();
    let report = judge_occurrences_within_budget(
        inputs.kernel,
        inputs.project,
        inputs.destination,
        &candidates,
        inputs.budget,
    )?;
    trace.optional(OptionalEvent::Judged, None);
    let mut rows: Vec<SelectedOccurrence> = Vec::with_capacity(live.len());
    for ((request, row), judged) in live.into_iter().zip(&report.occurrences) {
        let exclusion = if row.is_stale_for(request.revision) {
            Some(OptionalExclusion::Stale)
        } else {
            match judged.disposition {
                Disposition::Eligible => None,
                Disposition::PolicyExcluded(verdict) => Some(OptionalExclusion::Excluded(verdict)),
            }
        };
        match exclusion {
            Some(exclusion) => excluded.push((request.occurrence, exclusion)),
            None => rows.push(row),
        }
    }
    admit_optional_set(&rows, bounds).map_err(PreparationRefusal::OptionalBound)?;
    trace.optional(OptionalEvent::Bounded, None);

    deadline(&inputs)?;
    let (loaded, fault) = hold(store, inputs.budget, |conn| {
        Ok(until_first_fault(
            rows.iter()
                .map(|row| excludable(fetch_payload(conn, &row.payload))),
        ))
    })?;
    // Payload verification runs after `hold` releases the connection.
    let mut selected: Vec<(&SelectedOccurrence, Vec<u8>)> = Vec::with_capacity(rows.len());
    for (row, load) in rows.iter().zip(loaded) {
        match load.and_then(|payload| {
            row.payload
                .verify(&payload)
                .map(|()| payload)
                .map_err(|_| OptionalExclusion::Corrupt)
        }) {
            Ok(payload) => selected.push((row, payload)),
            Err(exclusion) => {
                excluded.push((row.occurrence, exclusion));
                continue;
            }
        }
        trace.payload_loads += 1;
        trace.optional(OptionalEvent::Loaded, Some(row.occurrence));
    }
    if let Some(fault) = fault {
        return Err(fault.into());
    }
    let selected: Vec<Selected<'_>> = selected
        .iter()
        .map(|(row, bytes)| Selected { row, bytes })
        .collect();
    let partition = group(&selected);
    trace.optional(OptionalEvent::Grouped, None);

    let ledger = required.ledger.clone();
    // The close is part of the render the budget must cover, so its charge is
    // held back from the scan and settled once the closing charge is known. A
    // required render that leaves no room for it is over budget, not closed
    // past the limit.
    let close_reserve = ledger.delta(BLOCK_CLOSE_FRAGMENT).with_headroom();
    let Some(scan_budget) = required.remaining.checked_sub(close_reserve) else {
        return Err(RequiredContextFailure::OverBudget {
            limit: required
                .charged
                .checked_add(required.remaining)
                .unwrap_or(ClaudeTokens::MAX),
            charged: required
                .charged
                .checked_add(close_reserve)
                .unwrap_or(ClaudeTokens::MAX),
        }
        .into());
    };
    let mut costs: Vec<ClaudeTokens> = Vec::with_capacity(partition.groups.len());
    // A group whose priced sum is unrepresentable is never admitted at a
    // saturated cost; the first such group refuses the phase after the scan.
    let mut overflow: Option<usize> = None;
    // Each group is priced once as the entries it would be charged as; the
    // admit callback commits that same pricing, so the deducted cost and the
    // ledger's charges cannot diverge.
    let mut state: (Ledger, Option<render::Staged>) = (ledger, None);
    let scan = skip_and_continue(
        scan_budget,
        partition.groups.len(),
        &mut state,
        |(ledger, staged), index| {
            if inputs.budget.is_exhausted() {
                costs.push(ClaudeTokens::MAX);
                return ClaudeTokens::MAX;
            }
            let priced = ledger.stage(render::group_fragments(index, &partition.groups[index]));
            let Some(cost) = priced.cost() else {
                overflow.get_or_insert(partition.groups[index].first_fused);
                costs.push(ClaudeTokens::MAX);
                return ClaudeTokens::MAX;
            };
            costs.push(cost);
            *staged = Some(priced);
            cost
        },
        |(ledger, staged), _| {
            if let Some(priced) = staged.take() {
                ledger.commit(priced);
            }
        },
    );
    let (mut ledger, _) = state;
    deadline(&inputs)?;
    if let Some(at) = overflow {
        return Err(PreparationRefusal::OptionalCostOverflow { at });
    }
    let close = ledger.close().with_headroom();
    let remaining = scan
        .remaining
        .checked_add(close_reserve)
        .and_then(|budget| budget.checked_sub(close))
        .unwrap_or(ClaudeTokens::ZERO);
    trace.optional(OptionalEvent::Scanned, None);
    admit_render(&ledger, accounting).map_err(PreparationRefusal::Accounting)?;
    let (admitted, skipped): (Vec<_>, Vec<_>) = partition
        .groups
        .into_iter()
        .zip(costs)
        .enumerate()
        .map(|(index, (group, cost))| (index, CostedGroup { group, cost }))
        .partition(|(index, _)| scan.admitted.binary_search(index).is_ok());
    Ok(OptionalAdmission {
        ledger,
        admitted: admitted.into_iter().map(|(_, group)| group).collect(),
        skipped: skipped.into_iter().map(|(_, group)| group).collect(),
        excluded,
        ungrouped: partition.refused,
        remaining,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical_memory::CanonicalMemory;
    use crate::m0_compose::trim_user_profile_to_budget;
    use crate::memory_render::render_memory_line;

    #[test]
    fn materialized_bytes_are_never_printed() {
        let secret = b"the required payload content";
        let item = MaterializedRequired {
            occurrence: retrieval::fusion::OccurrenceId::parse(&"ab".repeat(32)).unwrap(),
            revision: 3,
            bytes: secret.to_vec(),
            cost: ClaudeTokens(7),
        };
        let mut ledger = Ledger::open(AccountingProfile::exact_tokenizer());
        ledger.append(
            Charged::Required(item.occurrence),
            &render::required_fragment(item.occurrence, secret),
        );
        let materialization = RequiredMaterialization {
            items: vec![item.clone()],
            charged: ClaudeTokens(7),
            remaining: ClaudeTokens(0),
            ledger,
        };
        for rendered in [format!("{item:?}"), format!("{materialization:?}")] {
            assert!(
                !rendered.contains("payload content") && !rendered.contains("116, 104, 101"),
                "{rendered}"
            );
            assert!(rendered.contains(&secret.len().to_string()), "{rendered}");
            assert!(rendered.contains("abab"), "{rendered}");
        }
    }

    /// A missing or corrupt row is excluded and the next statement runs; an
    /// interrupted statement is the fault and no later statement runs.
    #[test]
    fn optional_statements_stop_at_the_first_non_excludable_fault() {
        let outcomes = [
            Ok(1),
            Err(ProjectionError::UnknownOccurrence {
                occurrence_id: String::new(),
            }),
            Err(ProjectionError::CorruptRow),
            Err(ProjectionError::Interrupted),
            Ok(2),
        ];
        let mut ran = 0;
        let (results, fault) = until_first_fault(outcomes.into_iter().map(|outcome| {
            ran += 1;
            excludable(outcome)
        }));
        assert_eq!(fault, Some(ProjectionError::Interrupted));
        assert_eq!(
            results,
            [
                Ok(1),
                Err(OptionalExclusion::Missing),
                Err(OptionalExclusion::Corrupt)
            ]
        );
        assert_eq!(ran, 4, "the statement after the interruption never ran");
    }

    #[test]
    fn budgets_are_integers_and_never_clamped() {
        assert_eq!(ClaudeTokens::from_budget(Some(0.0)), Ok(ClaudeTokens(0)));
        assert_eq!(ClaudeTokens::from_budget(Some(10.0)), Ok(ClaudeTokens(10)));
        assert_eq!(
            ClaudeTokens::from_budget(Some(18_446_744_073_709_549_568.0)),
            Ok(ClaudeTokens(18_446_744_073_709_549_568))
        );
        assert_eq!(ClaudeTokens::from_budget(None), Err(BudgetRefusal::Absent));
        assert_eq!(
            ClaudeTokens::from_budget(Some(f64::NAN)),
            Err(BudgetRefusal::NotANumber)
        );
        assert_eq!(
            ClaudeTokens::from_budget(Some(-1.0)),
            Err(BudgetRefusal::Negative)
        );
        assert_eq!(
            ClaudeTokens::from_budget(Some(-0.0)),
            Err(BudgetRefusal::Negative)
        );
        assert_eq!(
            ClaudeTokens::from_budget(Some(2.5)),
            Err(BudgetRefusal::NonInteger)
        );
        assert_eq!(
            ClaudeTokens::from_budget(Some(f64::INFINITY)),
            Err(BudgetRefusal::TooLarge)
        );
        assert_eq!(
            ClaudeTokens::from_budget(Some(18_446_744_073_709_551_616.0)),
            Err(BudgetRefusal::TooLarge),
            "2^64 would saturate `as u64` to u64::MAX"
        );
        assert_eq!(ClaudeTokens(u64::MAX).checked_add(ClaudeTokens(1)), None);
    }

    #[test]
    fn the_retained_malformed_budget_clamp_is_not_inherited() {
        let profile = || vec!["kept".to_owned(), "also kept".to_owned()];
        let generous = trim_user_profile_to_budget(profile(), 100.0, |_| 0);
        assert_eq!(generous, profile());
        for malformed in [f64::NAN, -5.0, f64::NEG_INFINITY] {
            let clamped = trim_user_profile_to_budget(profile(), malformed, |_| 0);
            assert_eq!(clamped, trim_user_profile_to_budget(profile(), 1.0, |_| 0));
            assert_ne!(clamped, generous);
            assert!(ClaudeTokens::from_budget(Some(malformed)).is_err());
        }
    }

    #[test]
    fn the_sixty_four_kib_silent_cut_is_not_inherited() {
        let content = "x".repeat(64 * 1024 + 7);
        let line = render_memory_line(&CanonicalMemory {
            object_id: "o".to_owned(),
            category: "decision".to_owned(),
            content: content.clone(),
        });
        assert_eq!(line.matches('x').count(), 64 * 1024);
        let profile = AccountingProfile::exact_tokenizer();
        let whole = profile.charge(&content).tokens();
        let cut = profile.charge(&content[..64 * 1024]).tokens();
        assert!(cut < whole, "the uncut tail is charged");
        assert_eq!(profile.authority(), Authority::Exact);
    }
}
