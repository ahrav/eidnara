//! Required occurrences complete before optional work, under an integer
//! `ClaudeTokens` budget.

use std::num::NonZeroUsize;

use kernel::applicability::EvalBudget;
use kernel::{ArtifactDestination, KernelError, KernelStore, ProjectScope};
use retrieval::ProjectionError;
use retrieval::eligibility::{Disposition, judge_occurrences_within_budget};
use retrieval::fusion::OccurrenceId;
use retrieval::packing::{
    BoundExceeded, Group, OptionalBounds, RequiredBound, RequiredBounds, RequiredContextFailure,
    RequiredFact, RequiredRequest, Selected, SelectedOccurrence, TokenCount, Ungrouped,
    admit_fused_candidates, admit_optional_set, admit_required, group, load_payload, read_selected,
    reserve_required, skip_and_continue,
};
use storage::SqliteStore;

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

pub trait CostEstimator {
    fn profile(&self) -> &'static str;
    fn cost(&self, bytes: &[u8]) -> ClaudeTokens;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TokenizerEstimator;

impl CostEstimator for TokenizerEstimator {
    fn profile(&self) -> &'static str {
        "tokenizer-estimate"
    }

    fn cost(&self, bytes: &[u8]) -> ClaudeTokens {
        let text = String::from_utf8_lossy(bytes);
        ClaudeTokens(tokenizer::estimate_tokens(&text) as u64)
    }
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
    pub estimator: &'a dyn CostEstimator,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializedRequired {
    pub occurrence: OccurrenceId,
    pub revision: i64,
    pub bytes: Vec<u8>,
    pub cost: ClaudeTokens,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredMaterialization {
    pub profile: &'static str,
    pub items: Vec<MaterializedRequired>,
    pub charged: ClaudeTokens,
    /// The token limit left for optional work.
    pub remaining: ClaudeTokens,
}

fn deadline(inputs: &RequiredInputs<'_>) -> Result<(), PreparationRefusal> {
    if inputs.budget.is_exhausted() {
        Err(PreparationRefusal::Deadline)
    } else {
        Ok(())
    }
}

type Reads = Vec<Result<Vec<SelectedOccurrence>, ProjectionError>>;

fn read_each(
    store: &SqliteStore,
    occurrences: impl Iterator<Item = OccurrenceId>,
) -> Result<Reads, PreparationRefusal> {
    store
        .with_conn(|conn| {
            Ok(occurrences
                .map(|occurrence| read_selected(conn, &[occurrence], NonZeroUsize::MIN))
                .collect())
        })
        .map_err(|error| PreparationRefusal::Storage(error.to_string()))
}

fn load_each<'a>(
    store: &SqliteStore,
    rows: impl Iterator<Item = &'a SelectedOccurrence>,
) -> Result<Vec<Result<Vec<u8>, ProjectionError>>, PreparationRefusal> {
    store
        .with_conn(|conn| Ok(rows.map(|row| load_payload(conn, &row.payload)).collect()))
        .map_err(|error| PreparationRefusal::Storage(error.to_string()))
}

pub fn prepare_required(
    store: &SqliteStore,
    inputs: RequiredInputs<'_>,
    requests: &[RequiredRequest],
    bounds: &RequiredBounds<ClaudeTokens>,
    trace: &mut PackingTrace,
) -> Result<RequiredMaterialization, PreparationRefusal> {
    deadline(&inputs)?;
    if let Some(beyond) = requests.get(bounds.max_payload_loads.get()) {
        return Err(RequiredContextFailure::Oversized {
            occurrence: beyond.occurrence,
            bound: RequiredBound::PayloadLoads,
        }
        .into());
    }
    let reads = read_each(store, requests.iter().map(|request| request.occurrence))?;
    let mut selected = Vec::with_capacity(reads.len());
    for (request, read) in requests.iter().zip(reads) {
        let occurrence = request.occurrence;
        match read {
            Ok(mut read) => selected.push(read.swap_remove(0)),
            Err(ProjectionError::UnknownOccurrence { .. }) => {
                return Err(RequiredContextFailure::Missing(occurrence).into());
            }
            Err(ProjectionError::CorruptRow) => {
                return Err(RequiredContextFailure::Corrupt(occurrence).into());
            }
            Err(other) => return Err(other.into()),
        }
        trace.required(RequiredEvent::Read, Some(occurrence));
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
    let loaded = load_each(store, admitted.iter().map(|item| item.row()))?;
    let mut bytes = Vec::with_capacity(loaded.len());
    for (item, load) in admitted.iter().zip(loaded) {
        let occurrence = item.row().occurrence;
        match load {
            Ok(payload) => bytes.push(payload),
            Err(ProjectionError::CorruptRow) => {
                return Err(RequiredContextFailure::Corrupt(occurrence).into());
            }
            Err(other) => return Err(other.into()),
        }
        trace.payload_loads += 1;
        trace.required(RequiredEvent::Loaded, Some(occurrence));
    }
    let borrowed: Vec<&[u8]> = bytes.iter().map(Vec::as_slice).collect();
    let reservation = reserve_required(&admitted, &borrowed, bounds.token_limit, |bytes| {
        inputs.estimator.cost(bytes)
    })?;
    trace.required(RequiredEvent::Reserved, None);

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
        profile: inputs.estimator.profile(),
        items,
        charged: reservation.charged,
        remaining: bounds
            .token_limit
            .checked_sub(reservation.charged)
            .unwrap_or(ClaudeTokens::ZERO),
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
    pub admitted: Vec<CostedGroup>,
    /// Groups visited and skipped, in fused order.
    pub skipped: Vec<CostedGroup>,
    /// In the order the exclusions were found: duplicates, then reads, then
    /// judgments, then loads.
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
    trace: &mut PackingTrace,
) -> Result<OptionalAdmission, PreparationRefusal> {
    deadline(&inputs)?;
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
    let reads = read_each(store, requests.iter().map(|request| request.occurrence))?;
    let mut live: Vec<(OptionalRequest, SelectedOccurrence)> = Vec::new();
    for (request, read) in requests.iter().zip(reads) {
        match read {
            Ok(mut read) => live.push((*request, read.swap_remove(0))),
            Err(ProjectionError::UnknownOccurrence { .. }) => {
                excluded.push((request.occurrence, OptionalExclusion::Missing));
            }
            Err(ProjectionError::CorruptRow) => {
                excluded.push((request.occurrence, OptionalExclusion::Corrupt));
            }
            Err(other) => return Err(other.into()),
        }
        trace.optional(OptionalEvent::Read, Some(request.occurrence));
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
    let loaded = load_each(store, rows.iter())?;
    let mut selected: Vec<(&SelectedOccurrence, Vec<u8>)> = Vec::with_capacity(rows.len());
    for (row, load) in rows.iter().zip(loaded) {
        match load {
            Ok(payload) => selected.push((row, payload)),
            Err(ProjectionError::CorruptRow) => {
                excluded.push((row.occurrence, OptionalExclusion::Corrupt));
                continue;
            }
            Err(other) => return Err(other.into()),
        }
        trace.payload_loads += 1;
        trace.optional(OptionalEvent::Loaded, Some(row.occurrence));
    }
    let selected: Vec<Selected<'_>> = selected
        .iter()
        .map(|(row, bytes)| Selected { row, bytes })
        .collect();
    let partition = group(&selected);
    trace.optional(OptionalEvent::Grouped, None);

    let costs: Vec<ClaudeTokens> = partition
        .groups
        .iter()
        .map(|group| {
            group.ranges.iter().fold(ClaudeTokens::ZERO, |sum, range| {
                sum.checked_add(inputs.estimator.cost(&range.bytes))
                    .unwrap_or(ClaudeTokens::MAX)
            })
        })
        .collect();
    let scan = skip_and_continue(required.remaining, partition.groups.len(), |_, index| {
        costs[index]
    });
    trace.optional(OptionalEvent::Scanned, None);
    let (admitted, skipped): (Vec<_>, Vec<_>) = partition
        .groups
        .into_iter()
        .zip(costs)
        .enumerate()
        .map(|(index, (group, cost))| (index, CostedGroup { group, cost }))
        .partition(|(index, _)| scan.admitted.binary_search(index).is_ok());
    Ok(OptionalAdmission {
        admitted: admitted.into_iter().map(|(_, group)| group).collect(),
        skipped: skipped.into_iter().map(|(_, group)| group).collect(),
        excluded,
        ungrouped: partition.refused,
        remaining: scan.remaining,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical_memory::CanonicalMemory;
    use crate::m0_compose::trim_user_profile_to_budget;
    use crate::memory_render::render_memory_line;

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
        let estimator = TokenizerEstimator;
        let whole = estimator.cost(content.as_bytes());
        let cut = estimator.cost(&content.as_bytes()[..64 * 1024]);
        assert!(cut < whole, "the uncut tail is charged");
    }
}
