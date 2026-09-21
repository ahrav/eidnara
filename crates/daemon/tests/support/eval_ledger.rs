//! This module maps query-route and packer returns to `eval_core` ledger
//! observations. Production functions return each observed value; the shell
//! adds its stage label and incarnation token.

use std::collections::BTreeSet;
use std::path::Path;

use daemon::packing::{
    Charged, ClaudeTokens, OptionalAdmission, PackingTrace, PreparationRefusal, RequiredInputs,
    prepare_optional, prepare_required,
};
use daemon::query_route::{
    Admitted, ExactAdmission, ExactReport, LaneStatus, QueryFailure, QueryOutcome,
};
use eval_core::{ChainStage, Ledger, Observation};
use kernel::applicability::EvalBudget;
use kernel::{ArtifactDestination, CommitReadIncarnation, KernelStore, ProjectScope};
use retrieval::eligibility::{Disposition, EligibilityReport};
use retrieval::fusion::{Lane, OccurrenceId};
use retrieval::packing::RequiredRequest;
use storage::{Isolation, SqliteStore, StorageBackend, StorageDescriptor, open_sqlite};

use super::packing::{accounting_bounds, bounds, byte_profile, wide};

pub type ChainLedger = Ledger<ChainStage>;

/// `CommitReadIncarnation` compares but does not serialize; the shell numbers
/// each distinct value in first-seen order and hands the ledger the number.
#[derive(Default)]
pub struct Incarnations(Vec<CommitReadIncarnation>);

impl Incarnations {
    pub fn token(&mut self, incarnation: CommitReadIncarnation) -> u64 {
        let index = match self.0.iter().position(|seen| *seen == incarnation) {
            Some(index) => index,
            None => {
                self.0.push(incarnation);
                self.0.len() - 1
            }
        };
        u64::try_from(index).unwrap()
    }

    pub fn distinct(&self) -> usize {
        self.0.len()
    }
}

/// `LaneView` copies admission results before `select` consumes `Admitted`.
#[derive(Debug, Clone)]
pub struct LaneView {
    pub statuses: [LaneStatus; Lane::ORDER.len()],
    pub exact: ExactReport,
    /// Per lane in `Lane::ORDER`: the occurrences its ranking carries.
    pub rankings: [Vec<String>; Lane::ORDER.len()],
}

fn slot(lane: Lane) -> usize {
    Lane::ORDER
        .iter()
        .position(|candidate| *candidate == lane)
        .unwrap()
}

impl LaneView {
    pub fn of(admitted: &Admitted<'_>) -> Self {
        let rankings = Lane::ORDER.map(|lane| {
            admitted
                .lanes()
                .lane(lane)
                .map(|ranking| {
                    ranking
                        .entries()
                        .iter()
                        .map(|entry| entry.occurrence().to_string())
                        .collect()
                })
                .unwrap_or_default()
        });
        Self {
            statuses: admitted.statuses.clone(),
            exact: admitted.exact.clone(),
            rankings,
        }
    }

    pub fn status(&self, lane: Lane) -> &LaneStatus {
        &self.statuses[slot(lane)]
    }

    pub fn ranking(&self, lane: Lane) -> &[String] {
        &self.rankings[slot(lane)]
    }

    /// The report the exact lane's admission judged under, reusable or not.
    pub fn exact_report(&self) -> Option<&EligibilityReport> {
        match &self.exact.admission {
            ExactAdmission::Judged(report) | ExactAdmission::Moved(report) => Some(report),
            ExactAdmission::NotJudged | ExactAdmission::KernelError => None,
        }
    }

    fn unavailable(&self, lane: Lane) -> bool {
        matches!(self.status(lane), LaneStatus::Unavailable(_))
    }

    /// Admission verdicts join only when every declared lane finished them
    /// under a reusable window: a moved or errored exact judgement and an
    /// unavailable lexical or dense lane both leave unknown verdicts behind.
    fn eligibility_unjoinable(&self) -> bool {
        matches!(
            self.exact.admission,
            ExactAdmission::Moved(_) | ExactAdmission::KernelError
        ) || [Lane::Lexical, Lane::Dense]
            .into_iter()
            .any(|lane| self.unavailable(lane))
    }
}

fn stage_of(lane: Lane) -> ChainStage {
    match lane {
        Lane::Exact => ChainStage::Exact,
        Lane::Lexical => ChainStage::Lexical,
        Lane::Dense => ChainStage::Dense,
    }
}

fn candidates(
    stage: ChainStage,
    incarnation: Option<u64>,
    ids: impl IntoIterator<Item = String>,
) -> Observation<ChainStage> {
    Observation::new(stage, 0, incarnation, ids.into_iter().collect())
        .unwrap_or_else(|_| Observation::unjoinable(stage, 0, incarnation))
}

/// Records the three lane stages and the eligibility stage. The exact stage is
/// the rows the lane read; an exact lane that ended before reading, or a
/// lexical or dense lane that ended unavailable, produced no joinable output.
pub fn observe_lanes(ledger: &mut ChainLedger, view: &LaneView, incarnations: &mut Incarnations) {
    for lane in Lane::ORDER {
        if *view.status(lane) == LaneStatus::Undeclared {
            continue;
        }
        let stage = stage_of(lane);
        let observation = match lane {
            Lane::Exact if view.exact.rows.is_empty() && view.unavailable(lane) => {
                Observation::unjoinable(stage, 0, None)
            }
            Lane::Exact => candidates(
                stage,
                None,
                view.exact.rows.iter().map(|row| row.occurrence_id.clone()),
            ),
            Lane::Lexical | Lane::Dense if view.unavailable(lane) => {
                Observation::unjoinable(stage, 0, None)
            }
            Lane::Lexical | Lane::Dense => {
                candidates(stage, None, view.ranking(lane).iter().cloned())
            }
        };
        ledger.observe(observation);
    }
    let incarnation = view
        .exact_report()
        .map(|report| incarnations.token(report.incarnation));
    if view.eligibility_unjoinable() {
        ledger.observe(Observation::unjoinable(
            ChainStage::Eligibility,
            0,
            incarnation,
        ));
        return;
    }
    let mut eligible: BTreeSet<String> = view
        .exact_report()
        .into_iter()
        .flat_map(|report| report.occurrences.iter())
        .filter(|judged| judged.disposition == Disposition::Eligible)
        .map(|judged| judged.occurrence_id.clone())
        .collect();
    eligible.extend(view.ranking(Lane::Lexical).iter().cloned());
    eligible.extend(view.ranking(Lane::Dense).iter().cloned());
    ledger.observe(candidates(ChainStage::Eligibility, incarnation, eligible));
}

/// Records fusion (every entry revalidation judged) and selection (the
/// entries the response carries).
pub fn observe_outcome(
    ledger: &mut ChainLedger,
    outcome: &QueryOutcome,
    incarnations: &mut Incarnations,
) {
    let incarnation = outcome
        .revalidation
        .as_ref()
        .map(|report| incarnations.token(report.incarnation));
    let fused: BTreeSet<String> = outcome
        .revalidation
        .iter()
        .flat_map(|report| report.occurrences.iter())
        .map(|judged| judged.occurrence_id.clone())
        .collect();
    ledger.observe(candidates(ChainStage::Fusion, incarnation, fused));
    let selected: BTreeSet<String> = outcome.body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["occurrence_id"].as_str().unwrap().to_string())
        .collect();
    ledger.observe(candidates(ChainStage::Selection, incarnation, selected));
}

/// A refusal after admission. The union bound is fusion ending the request
/// with nothing; the response bounds are selection ending it with nothing;
/// every other refusal (a moved or failed revalidation, a deadline, a
/// cancellation) discards what fusion produced, so fusion is unjoinable.
pub fn observe_refusal(ledger: &mut ChainLedger, failure: &QueryFailure) {
    let observation = match failure {
        QueryFailure::Unavailable("fused_union") => candidates(ChainStage::Fusion, None, []),
        QueryFailure::Unavailable("response_bytes" | "response_measure") => {
            candidates(ChainStage::Selection, None, [])
        }
        QueryFailure::Unavailable(_)
        | QueryFailure::Terminal(_)
        | QueryFailure::InvalidQuery(_) => Observation::unjoinable(ChainStage::Fusion, 0, None),
    };
    ledger.observe(observation);
}

/// The occurrences the closed render carries: required items plus every member
/// of every charged range, joined through the admitted group the range labels.
pub fn survivors(admission: &OptionalAdmission) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for entry in admission.ledger().entries() {
        match entry.item {
            Charged::Required(occurrence) => {
                out.insert(occurrence.to_string());
            }
            Charged::Range(group, position) => {
                let group = admission
                    .admitted()
                    .iter()
                    .find(|costed| costed.index == group)
                    .expect("a charged range names an admitted group");
                out.extend(
                    group.group.ranges[position]
                        .members
                        .iter()
                        .map(ToString::to_string),
                );
            }
            Charged::BlockOpen
            | Charged::GroupOpen(_)
            | Charged::GroupClose(_)
            | Charged::BlockClose => {}
        }
    }
    out
}

/// A closed render is the packing output. A bound the render hit ends the
/// request with nothing packed, so it is an empty output; a deadline, a
/// storage, projection, or kernel fault, or a caller mismatch leaves no
/// closed result, so packing is unjoinable.
pub fn observe_packing(
    ledger: &mut ChainLedger,
    admission: Result<&OptionalAdmission, &PreparationRefusal>,
) {
    let observation = match admission {
        Ok(admission) => candidates(ChainStage::Packing, None, survivors(admission)),
        Err(
            PreparationRefusal::Required(_)
            | PreparationRefusal::OptionalBound(_)
            | PreparationRefusal::Accounting(_)
            | PreparationRefusal::CloseOverBudget { .. },
        ) => candidates(ChainStage::Packing, None, []),
        Err(
            PreparationRefusal::ProfileMismatch
            | PreparationRefusal::OptionalCostOverflow { .. }
            | PreparationRefusal::Deadline
            | PreparationRefusal::Projection(_)
            | PreparationRefusal::Storage(_)
            | PreparationRefusal::Kernel(_),
        ) => Observation::unjoinable(ChainStage::Packing, 0, None),
    };
    ledger.observe(observation);
}

/// The projection file the route read, reopened as the store the packer takes.
/// The descriptor is the daemon's own, so the storage lease and fence match.
pub fn packing_store(projection_path: &Path) -> SqliteStore {
    open_sqlite(
        &StorageDescriptor {
            module_id: "eidnara".to_string(),
            storage_namespace: "search-projection".to_string(),
            isolation: Isolation::Module,
            backend: StorageBackend::Sqlite {
                path: projection_path.to_str().unwrap().to_string(),
            },
        },
        retrieval::BASELINE,
    )
    .unwrap()
}

pub struct Packed {
    pub trace: PackingTrace,
    pub admission: Result<OptionalAdmission, PreparationRefusal>,
}

/// Runs both packer phases over `requests` as optional candidates under the
/// byte profile, with `token_limit` as the whole render budget.
pub fn pack(
    store: &SqliteStore,
    kernel: &KernelStore,
    project: &ProjectScope,
    requests: &[RequiredRequest],
    token_limit: ClaudeTokens,
) -> Packed {
    let profile = byte_profile();
    let budget = EvalBudget::unbounded();
    let inputs = RequiredInputs {
        kernel,
        project,
        destination: ArtifactDestination::Local,
        budget: &budget,
        profile: &profile,
    };
    let mut trace = PackingTrace::default();
    trace.note_retrieval_call();
    let required = prepare_required(
        store,
        inputs,
        &[],
        &bounds(token_limit.get()),
        &accounting_bounds(),
        &mut trace,
    )
    .unwrap();
    let admission = prepare_optional(
        store,
        inputs,
        &required,
        requests,
        &wide(),
        &accounting_bounds(),
        &mut trace,
    );
    Packed { trace, admission }
}

pub fn request(occurrence: &str, revision: i64) -> RequiredRequest {
    RequiredRequest {
        occurrence: OccurrenceId::parse(occurrence).unwrap(),
        revision,
    }
}
