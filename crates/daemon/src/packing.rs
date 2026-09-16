//! Required occurrences complete before optional work, under an integer
//! `ClaudeTokens` budget.

use std::fmt;
use std::num::NonZeroUsize;

use kernel::applicability::EvalBudget;
use kernel::{
    ArtifactDestination, KernelError, KernelStore, MAX_ELIGIBILITY_CANDIDATES, ProjectScope,
};
use retrieval::ProjectionError;
use retrieval::eligibility::judge_occurrences_within_budget;
use retrieval::fusion::OccurrenceId;
use retrieval::packing::{
    RequiredBound, RequiredBounds, RequiredContextFailure, RequiredFact, RequiredRequest,
    SelectedOccurrence, TokenCount, admit_required, fetch_payload, read_selected, reserve_required,
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
    /// refused as negative. Values above 2^53 are over-range: an integer
    /// there may already have rounded on its way into the `f64`.
    pub fn from_budget(value: Option<f64>) -> Result<Self, BudgetRefusal> {
        const MAX_EXACT: f64 = 9_007_199_254_740_992.0;
        let value = value.ok_or(BudgetRefusal::Absent)?;
        if value.is_nan() {
            return Err(BudgetRefusal::NotANumber);
        }
        if value.is_sign_negative() {
            return Err(BudgetRefusal::Negative);
        }
        if value > MAX_EXACT {
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
pub enum StageEvent {
    Required(RequiredEvent, Option<OccurrenceId>),
    Optional,
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
            .filter(|event| matches!(event, StageEvent::Optional))
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreparationRefusal {
    Required(RequiredContextFailure<ClaudeTokens>),
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
    pub profile: &'static str,
    pub items: Vec<MaterializedRequired>,
    pub charged: ClaudeTokens,
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

pub fn prepare_required(
    store: &SqliteStore,
    inputs: RequiredInputs<'_>,
    requests: &[RequiredRequest],
    bounds: &RequiredBounds<ClaudeTokens>,
    trace: &mut PackingTrace,
) -> Result<RequiredMaterialization, PreparationRefusal> {
    let deadline = || {
        if inputs.budget.is_exhausted() {
            Err(PreparationRefusal::Deadline)
        } else {
            Ok(())
        }
    };

    deadline()?;
    // The kernel judges at most `MAX_ELIGIBILITY_CANDIDATES` per batch, so a
    // caller bound above it cannot be honored.
    const KERNEL_BATCH_CAP: NonZeroUsize = NonZeroUsize::new(MAX_ELIGIBILITY_CANDIDATES).unwrap();
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

    deadline()?;
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

    deadline()?;
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
        trace.payload_loads += 1;
        trace.required(RequiredEvent::Loaded, Some(occurrence));
        item.row()
            .payload
            .verify(payload)
            .map_err(|_| RequiredContextFailure::Corrupt(occurrence))?;
    }
    if let Some((occurrence, error)) = fault {
        return Err(match error {
            ProjectionError::CorruptRow => RequiredContextFailure::Corrupt(occurrence).into(),
            other => other.into(),
        });
    }
    let borrowed: Vec<&[u8]> = bytes.iter().map(Vec::as_slice).collect();
    deadline()?;
    let reservation = reserve_required(&admitted, &borrowed, bounds.token_limit, |bytes| {
        inputs.estimator.cost(bytes)
    })?;
    deadline()?;
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
        let materialization = RequiredMaterialization {
            profile: "test",
            items: vec![item.clone()],
            charged: ClaudeTokens(7),
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

    #[test]
    fn budgets_are_integers_and_never_clamped() {
        assert_eq!(ClaudeTokens::from_budget(Some(0.0)), Ok(ClaudeTokens(0)));
        assert_eq!(ClaudeTokens::from_budget(Some(10.0)), Ok(ClaudeTokens(10)));
        assert_eq!(
            ClaudeTokens::from_budget(Some(9_007_199_254_740_992.0)),
            Ok(ClaudeTokens(1 << 53))
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
            ClaudeTokens::from_budget(Some(9_007_199_254_740_994.0)),
            Err(BudgetRefusal::TooLarge),
            "above 2^53 an integer may already have rounded before it arrived"
        );
        assert_eq!(
            ClaudeTokens::from_budget(Some(18_446_744_073_709_551_616.0)),
            Err(BudgetRefusal::TooLarge)
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
