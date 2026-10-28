//! A bounded, snapshot-bound read of every canonical fact a claim decision
//! carries: registry row, typed decision row, raw own and lineage admission
//! rows, the served interpretation per surface, the occurrence inventory the
//! kernel encodes for it, and its causal class.
//!
//! Stored values are copied, never re-evaluated. The own and lineage admission
//! rows are selected by the serving view's own SQL, the supporting approval is
//! the one the row names with the authority chain's state at the snapshot, and
//! served visibility comes from the serving query the read routes use. The
//! revision domains stay separate fields: `object.source_revision`, each
//! admission row's `policy_revision`, and the snapshot's `known_as_of`.
//! A required field that fails to decode is an error, not a default.

use std::collections::{HashMap, HashSet};
use std::num::{NonZeroU64, NonZeroUsize};
use std::sync::LazyLock;

use rusqlite::{OptionalExtension, Statement, Transaction, TransactionBehavior, params};

use super::admission::{
    Disposition, EventKind, Maturity, Outcome, SourceClass, Surface, SurfaceVisibility, TaintClass,
    VisibilityRow, served_classes, served_lineage_decision_sql, served_own_decision_sql,
    supporting_approval_valid_sql,
};
use super::applicability::EvalBudget;
use super::claim_causality::{CausalClass, CausalRecord, causal_class_at, registry_row_at};
use super::commit_read::{CommitReadIncarnation, CommitReadTarget, tip};
use super::envelope::{ObjectRow, Sensitivity, object_row_from};
use super::open::AcquireLimit;
use super::source_descriptor::{
    OCCURRENCE_ID_PREFIX, SOURCE_DESCRIPTOR_KIND, descriptor_object_id, reencoded_identity,
    stored_detail,
};
use super::source_hold::Descriptors;
use super::source_identity::{Occurrence, OccurrenceClass, encode_preserving_span};
use super::{KernelError, KernelStore, map_sqlite};

pub const MAX_CLAIM_OBJECT_ID_BYTES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaimFactBounds {
    pub max_claims: NonZeroUsize,
    pub max_causal_payload_bytes: NonZeroU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ClaimFactsError {
    #[error("claim facts request names more claims than the bound")]
    TooManyClaims,
    #[error("claim facts request names one claim twice")]
    DuplicateClaim,
    #[error("claim facts request names an object that is not a decision")]
    NotADecision,
    #[error("claim facts row holds a required field this build cannot interpret")]
    MalformedRequiredField,
    #[error("claim facts target names another kernel incarnation")]
    IncarnationMismatch,
    #[error(transparent)]
    Kernel(#[from] KernelError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportingApproval {
    pub object_id: String,
    pub valid_at_snapshot: bool,
}

/// One stored admission row, copied as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionFacts {
    pub admission_decision_id: String,
    pub commit_seq: i64,
    pub event_kind: EventKind,
    pub historical_maturity: Maturity,
    pub effective_maturity: Maturity,
    pub disposition: Disposition,
    pub visibility: VisibilityRow,
    pub outcome: Outcome,
    pub source_class: SourceClass,
    pub taint_class: TaintClass,
    pub policy_revision: i64,
    pub sensitivity: Sensitivity,
    pub evidence_id: Option<String>,
    pub supporting_approval: Option<SupportingApproval>,
    pub trigger_object_id: Option<String>,
    pub elevated_support: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimDecisionFacts {
    pub decision_id: String,
    pub decision_kind: String,
    pub proposition_id: Option<String>,
    pub scope_id: Option<String>,
    pub anchor_id: Option<String>,
    pub evidence_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServedFacts {
    pub sensitivity: Sensitivity,
    pub auto_inject: SurfaceVisibility,
    pub auto_search: SurfaceVisibility,
    pub explicit_search: SurfaceVisibility,
}

/// Why the serving view has no row for a claim, or the row it has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServedStanding {
    Served(ServedFacts),
    /// The registry row was invalidated at or before the snapshot.
    NotLiveAtSnapshot,
    /// No own admission row has been committed for the object.
    NeverAdmitted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimOccurrence {
    pub class: OccurrenceClass,
    pub representation: &'static str,
    pub descriptor_object_id: String,
    pub occurrence_id: String,
    pub lineage_id: String,
    pub payload_id: String,
    pub artifact_digest: String,
    pub evidence_id: String,
    pub descriptor_commit_seq: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepresentationExclusion {
    /// No descriptor for the representation is live at the snapshot with live
    /// evidence, the same rule the export applies.
    NoDescriptor,
    MalformedIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExcludedRepresentation {
    pub class: OccurrenceClass,
    pub representation: &'static str,
    pub reason: RepresentationExclusion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimFacts {
    pub object: ObjectRow,
    pub decision: ClaimDecisionFacts,
    pub own_admission: Option<AdmissionFacts>,
    pub lineage_admission: Option<AdmissionFacts>,
    pub served: ServedStanding,
    pub occurrences: Vec<ClaimOccurrence>,
    pub excluded_representations: Vec<ExcludedRepresentation>,
    pub causality: CausalClass,
    pub causal_record: Option<CausalRecord>,
}

/// The registry row, decision row, own admission row, and served standing of a
/// claim decision: the [`ClaimFacts`] fields that decide whether it stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimStanding {
    pub object: ObjectRow,
    pub decision: ClaimDecisionFacts,
    pub own_admission: Option<AdmissionFacts>,
    pub served: ServedStanding,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimFactsSnapshot {
    pub known_as_of: i64,
    pub tip: i64,
    pub claims: Vec<ClaimFacts>,
    /// Requested ids with no registry row created by the snapshot.
    pub missing: Vec<String>,
}

impl KernelStore {
    /// Reads every claim in `object_ids` at `requested` from one snapshot.
    /// `claims` follows the request order; `missing` keeps ids with no registry
    /// row by the snapshot.
    ///
    /// # Errors
    ///
    /// `TooManyClaims` and `DuplicateClaim` before any row is read;
    /// `NotADecision` when a named object is registered as another kind;
    /// `MalformedRequiredField` when an admission or decision column holds a
    /// value this build cannot interpret. Kernel errors pass through:
    /// `InvalidInput` for a negative sequence, `FutureSnapshot` past the tip,
    /// `CorruptCanonicalRow` when a decision or descriptor row disagrees with
    /// its registry row, and `Busy`, `Deadline`, or `Io` from the reader.
    pub fn claim_facts_as_of(
        &self,
        object_ids: &[String],
        requested: i64,
        bounds: ClaimFactBounds,
    ) -> Result<ClaimFactsSnapshot, ClaimFactsError> {
        self.claim_facts_inner(
            object_ids,
            requested,
            bounds,
            None,
            &AcquireLimit::default(),
        )
    }

    /// [`Self::claim_facts_as_of`] at `target.through_commit`, refused with
    /// `IncarnationMismatch` under the reader guard when the store is not the
    /// incarnation `target` was captured from, so a restore between capturing
    /// the tip and reading the facts cannot pair one history's tip with
    /// another's rows.
    ///
    /// # Errors
    ///
    /// `IncarnationMismatch` before any claim row is read (`FutureSnapshot`
    /// precedes it when the replacement's tip is below the target), then every
    /// refusal of [`Self::claim_facts_as_of`].
    pub fn claim_facts_at(
        &self,
        object_ids: &[String],
        target: CommitReadTarget,
        bounds: ClaimFactBounds,
    ) -> Result<ClaimFactsSnapshot, ClaimFactsError> {
        self.claim_facts_inner(
            object_ids,
            target.through_commit,
            bounds,
            Some(target.incarnation),
            &AcquireLimit::default(),
        )
    }

    /// [`Self::claim_facts_at`] under `budget`: reader acquisition and every
    /// statement stop at the deadline or on cancellation with
    /// `KernelError::Deadline`.
    ///
    /// # Errors
    ///
    /// `Kernel(Deadline)` from an exhausted budget, then every refusal of
    /// [`Self::claim_facts_at`].
    pub fn claim_facts_at_within_budget(
        &self,
        object_ids: &[String],
        target: CommitReadTarget,
        bounds: ClaimFactBounds,
        budget: &EvalBudget,
    ) -> Result<ClaimFactsSnapshot, ClaimFactsError> {
        let limit = budget.acquire_limit();
        limit.run(|| {
            self.claim_facts_inner(
                object_ids,
                target.through_commit,
                bounds,
                Some(target.incarnation),
                &limit,
            )
        })
    }

    /// The standing of each claim in `object_ids` at `target.through_commit`,
    /// in request order, `None` for an id with no registry row by the snapshot.
    /// Each standing equals the matching fields of
    /// [`Self::claim_facts_at_within_budget`] at the same target; the lineage
    /// admission, occurrence inventory, and causal class are not read.
    ///
    /// # Errors
    ///
    /// The refusals of [`Self::claim_facts_at_within_budget`], with
    /// `max_claims` as the claim bound, except those only the lineage
    /// admission, occurrence, and causal reads raise.
    pub fn claim_standing_at_within_budget(
        &self,
        object_ids: &[String],
        target: CommitReadTarget,
        max_claims: NonZeroUsize,
        budget: &EvalBudget,
    ) -> Result<Vec<Option<ClaimStanding>>, ClaimFactsError> {
        let limit = budget.acquire_limit();
        limit.run(|| {
            check_claim_ids(object_ids, max_claims)?;
            let requested = target.through_commit;
            if requested < 0 {
                return Err(KernelError::InvalidInput.into());
            }
            let mut reader = self.reader_with_limit(&limit)?;
            let tx = reader.transaction(TransactionBehavior::Deferred)?;
            if requested > tip(&tx).map_err(map_sqlite)? {
                return Err(KernelError::FutureSnapshot.into());
            }
            // The reader guard is held, so a restore cannot land between this
            // comparison and the rows read below.
            if target.incarnation != self.incarnation() {
                return Err(ClaimFactsError::IncarnationMismatch);
            }
            let standings = load_standings_in_tx(&tx, requested, object_ids, &limit)?;
            tx.commit()?;
            limit.check()?;
            Ok(standings)
        })
    }

    fn claim_facts_inner(
        &self,
        object_ids: &[String],
        requested: i64,
        bounds: ClaimFactBounds,
        incarnation: Option<CommitReadIncarnation>,
        limit: &AcquireLimit,
    ) -> Result<ClaimFactsSnapshot, ClaimFactsError> {
        check_claim_bounds(object_ids, bounds)?;
        if requested < 0 {
            return Err(KernelError::InvalidInput.into());
        }
        let mut reader = self.reader_with_limit(limit)?;
        let tx = reader.transaction(TransactionBehavior::Deferred)?;
        let tip = tip(&tx).map_err(map_sqlite)?;
        if requested > tip {
            return Err(KernelError::FutureSnapshot.into());
        }
        // The reader guard is held, so a restore cannot land between this
        // comparison and the rows read below.
        if incarnation.is_some_and(|expected| expected != self.incarnation()) {
            return Err(ClaimFactsError::IncarnationMismatch);
        }
        let (claims, missing) = load_claims_in_tx(&tx, requested, object_ids, bounds, limit)?;
        tx.commit()?;
        limit.check()?;
        Ok(ClaimFactsSnapshot {
            known_as_of: requested,
            tip,
            claims,
            missing,
        })
    }
}

pub(crate) fn check_claim_bounds(
    object_ids: &[String],
    bounds: ClaimFactBounds,
) -> Result<(), ClaimFactsError> {
    check_claim_ids(object_ids, bounds.max_claims)
}

fn check_claim_ids(object_ids: &[String], max_claims: NonZeroUsize) -> Result<(), ClaimFactsError> {
    if object_ids.len() > max_claims.get() {
        return Err(ClaimFactsError::TooManyClaims);
    }
    if object_ids
        .iter()
        .any(|id| id.is_empty() || id.len() > MAX_CLAIM_OBJECT_ID_BYTES)
    {
        return Err(KernelError::InvalidInput.into());
    }
    let mut distinct = HashSet::with_capacity(object_ids.len());
    if !object_ids.iter().all(|id| distinct.insert(id.as_str())) {
        return Err(ClaimFactsError::DuplicateClaim);
    }
    Ok(())
}

/// The claims and missing ids of [`KernelStore::claim_facts_as_of`] as `tx`
/// sees them at `requested`, for a caller that holds its own transaction;
/// `limit` is polled before each claim so cancellation lands between claims.
pub(crate) fn load_claims_in_tx(
    tx: &Transaction<'_>,
    requested: i64,
    object_ids: &[String],
    bounds: ClaimFactBounds,
    limit: &AcquireLimit,
) -> Result<(Vec<ClaimFacts>, Vec<String>), ClaimFactsError> {
    load_claims_served_in_tx(
        tx,
        requested,
        object_ids,
        bounds,
        limit,
        &HashMap::new(),
        None,
    )
}

/// Serving facts by object id; `None` records an object with no serving admission.
pub(crate) type KnownServed<'a> = HashMap<&'a str, Option<ServedFacts>>;

/// The occurrences to read for each object, by object id.
pub(crate) type WantedOccurrences<'a> = HashMap<&'a str, Vec<(OccurrenceClass, &'a str)>>;

/// One row per id of `:ids`, in request order: the registry row created by
/// `:governing_as_of`, the decision row with its lifecycle agreement, and the
/// own admission row the serving view selects, with the columns of
/// [`admission_sql`].
fn standing_sql() -> String {
    let selection = served_own_decision_sql("a");
    let approval_valid = supporting_approval_valid_sql("d.approval_object_id");
    format!(
        "SELECT o.object_id,o.object_kind,o.domain_id,o.source_kind,o.source_id,
                o.source_revision,o.created_commit_seq,o.invalidated_commit_seq,
                o.superseded_by,o.sensitivity_class,
                j.value,dec.object_id IS NOT NULL,{agrees},
                dec.decision_id,dec.decision_kind,dec.proposition_id,dec.scope_id,
                dec.anchor_id,dec.evidence_id,
                d.admission_decision_id,d.commit_seq,d.event_kind,d.maturity,
                d.effective_maturity,d.disposition,d.visibility,d.outcome,d.source_class,
                d.taint_class,d.policy_revision,d.sensitivity_class,d.evidence_id,
                d.approval_object_id,{approval_valid},d.trigger_object_id,d.elevated_support
         FROM json_each(:ids) j
         LEFT JOIN object_registry o
           ON o.object_id=j.value AND o.created_commit_seq<=:governing_as_of
         LEFT JOIN decisions dec ON dec.object_id=o.object_id
         LEFT JOIN admission_decisions d
           ON o.object_id IS NOT NULL AND d.admission_decision_id={selection}
         ORDER BY j.key",
        agrees = lifecycle_agrees_sql("dec", "o"),
    )
}

/// The standings of [`KernelStore::claim_standing_at_within_budget`] from one
/// statement, with the refusals [`load_claim`] makes before its lineage read;
/// `limit` is polled before each claim.
fn load_standings_in_tx(
    tx: &Transaction<'_>,
    requested: i64,
    object_ids: &[String],
    limit: &AcquireLimit,
) -> Result<Vec<Option<ClaimStanding>>, ClaimFactsError> {
    let mut served = load_served(tx, requested, object_ids)?;
    let mut standings = Vec::with_capacity(object_ids.len());
    if object_ids.is_empty() {
        return Ok(standings);
    }
    limit.check()?;
    static SQL: LazyLock<String> = LazyLock::new(standing_sql);
    let ids = serde_json::to_string(object_ids).map_err(|_| KernelError::InvalidInput)?;
    let mut statement = tx.prepare_cached(&SQL).map_err(map_sqlite)?;
    let mut rows = statement
        .query(rusqlite::named_params! { ":ids": ids, ":governing_as_of": requested })
        .map_err(map_sqlite)?;
    while let Some(row) = rows.next().map_err(map_sqlite)? {
        limit.check()?;
        let id = row.get_ref(10).and_then(|id| Ok(id.as_str()?));
        if object_ids.get(standings.len()).map(String::as_str) != Some(id.map_err(map_sqlite)?) {
            return Err(KernelError::CorruptCanonicalRow.into());
        }
        standings.push(standing_from(row, requested, &mut served)?);
    }
    if standings.len() != object_ids.len() {
        return Err(KernelError::CorruptCanonicalRow.into());
    }
    Ok(standings)
}

fn column<T: rusqlite::types::FromSql>(
    row: &rusqlite::Row<'_>,
    index: usize,
) -> Result<T, ClaimFactsError> {
    row.get(index).map_err(|error| map_sqlite(error).into())
}

/// One row of [`standing_sql`], judged as [`registry_row_at`], [`load_claim`],
/// [`load_decision`], and [`load_admission`] judge the same rows.
fn standing_from(
    row: &rusqlite::Row<'_>,
    requested: i64,
    served: &mut HashMap<String, ServedFacts>,
) -> Result<Option<ClaimStanding>, ClaimFactsError> {
    if column::<Option<String>>(row, 0)?.is_none() {
        return Ok(None);
    }
    let mut object = object_row_from(row).map_err(map_sqlite)?;
    if object
        .invalidated_commit_seq
        .is_some_and(|seq| seq > requested)
    {
        object.invalidated_commit_seq = None;
        object.superseded_by = None;
    }
    if object.object_kind != "decision" {
        return Err(ClaimFactsError::NotADecision);
    }
    if !column::<bool>(row, 11)? || column::<Option<bool>>(row, 12)? != Some(true) {
        return Err(KernelError::CorruptCanonicalRow.into());
    }
    sensitivity_field(&column::<String>(row, 9)?)?;
    let decision = ClaimDecisionFacts {
        decision_id: column(row, 13)?,
        decision_kind: column(row, 14)?,
        proposition_id: column(row, 15)?,
        scope_id: column(row, 16)?,
        anchor_id: column(row, 17)?,
        evidence_id: column(row, 18)?,
    };
    let own_admission = match column::<Option<String>>(row, 19)? {
        None => None,
        Some(admission_decision_id) => Some(interpret_admission(RawAdmission {
            admission_decision_id,
            commit_seq: column(row, 20)?,
            event_kind: column(row, 21)?,
            maturity: column(row, 22)?,
            effective_maturity: column(row, 23)?,
            disposition: column(row, 24)?,
            visibility: column(row, 25)?,
            outcome: column(row, 26)?,
            source_class: column(row, 27)?,
            taint_class: column(row, 28)?,
            policy_revision: column(row, 29)?,
            sensitivity: column(row, 30)?,
            evidence_id: column(row, 31)?,
            approval_object_id: column(row, 32)?,
            approval_valid: column(row, 33)?,
            trigger_object_id: column(row, 34)?,
            elevated_support: column(row, 35)?,
        })?),
    };
    let served = match served.remove(&object.object_id) {
        Some(facts) => ServedStanding::Served(facts),
        None if object.invalidated_commit_seq.is_some() => ServedStanding::NotLiveAtSnapshot,
        None => ServedStanding::NeverAdmitted,
    };
    Ok(Some(ClaimStanding {
        object,
        decision,
        own_admission,
        served,
    }))
}

/// `known` supplies serving facts from `tx` at `requested`; `None` records an object with no serving admission. `load_claims_served_in_tx` queries the serving view only for objects absent from `known`.
pub(crate) fn load_claims_served_in_tx(
    tx: &Transaction<'_>,
    requested: i64,
    object_ids: &[String],
    bounds: ClaimFactBounds,
    limit: &AcquireLimit,
    known: &KnownServed<'_>,
    wanted: Option<&WantedOccurrences<'_>>,
) -> Result<(Vec<ClaimFacts>, Vec<String>), ClaimFactsError> {
    let mut claims = Vec::with_capacity(object_ids.len());
    let mut missing = Vec::new();
    let unknown: Vec<String> = object_ids
        .iter()
        .filter(|id| !known.contains_key(id.as_str()))
        .cloned()
        .collect();
    let mut served = load_served(tx, requested, &unknown)?;
    if !object_ids.is_empty() {
        limit.check()?;
        static OWN_SQL: LazyLock<String> = LazyLock::new(|| admission_sql(AdmissionScope::Own));
        static LINEAGE_SQL: LazyLock<String> =
            LazyLock::new(|| admission_sql(AdmissionScope::Lineage));
        let mut own = tx.prepare_cached(&OWN_SQL).map_err(map_sqlite)?;
        let mut lineage = tx.prepare_cached(&LINEAGE_SQL).map_err(map_sqlite)?;
        for object_id in object_ids {
            limit.check()?;
            match registry_row_at(tx, requested, object_id)? {
                None => missing.push(object_id.clone()),
                Some(object) => claims.push(load_claim(
                    tx,
                    requested,
                    object,
                    match known.get(object_id.as_str()) {
                        Some(facts) => facts.clone(),
                        None => served.remove(object_id),
                    },
                    wanted.map(|wanted| {
                        wanted
                            .get(object_id.as_str())
                            .map_or(&[][..], Vec::as_slice)
                    }),
                    bounds,
                    (&mut own, &mut lineage),
                )?),
            }
        }
    }
    Ok((claims, missing))
}

fn load_served(
    tx: &Transaction<'_>,
    requested: i64,
    object_ids: &[String],
) -> Result<HashMap<String, ServedFacts>, KernelError> {
    if object_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let ids = serde_json::to_string(object_ids).map_err(|_| KernelError::InvalidInput)?;
    Ok(served_classes(tx, requested, &ids, None)?
        .into_iter()
        .map(|row| {
            let facts = ServedFacts {
                sensitivity: row.object.sensitivity,
                auto_inject: row.visibility(Surface::AutoInject),
                auto_search: row.visibility(Surface::AutoSearch),
                explicit_search: row.visibility(Surface::ExplicitSearch),
            };
            (row.object.object_id, facts)
        })
        .collect())
}

fn load_claim(
    tx: &Transaction<'_>,
    requested: i64,
    object: ObjectRow,
    served: Option<ServedFacts>,
    wanted: Option<&[(OccurrenceClass, &str)]>,
    bounds: ClaimFactBounds,
    (own, lineage): (&mut Statement<'_>, &mut Statement<'_>),
) -> Result<ClaimFacts, ClaimFactsError> {
    if object.object_kind != "decision" {
        return Err(ClaimFactsError::NotADecision);
    }
    let decision = load_decision(tx, &object)?;
    let own_admission = load_admission(own, requested, &object)?;
    let lineage_admission = load_admission(lineage, requested, &object)?;
    let served = match served {
        Some(facts) => ServedStanding::Served(facts),
        None if object.invalidated_commit_seq.is_some() => ServedStanding::NotLiveAtSnapshot,
        None => ServedStanding::NeverAdmitted,
    };
    let (occurrences, excluded_representations) = load_occurrences(tx, requested, &object, wanted)?;
    let (causality, causal_record) =
        causal_class_at(tx, requested, &object, bounds.max_causal_payload_bytes)?;
    Ok(ClaimFacts {
        object,
        decision,
        own_admission,
        lineage_admission,
        served,
        occurrences,
        excluded_representations,
        causality,
        causal_record,
    })
}

fn sensitivity_field(value: &str) -> Result<Sensitivity, ClaimFactsError> {
    Sensitivity::ALL
        .iter()
        .copied()
        .find(|class| class.as_str() == value)
        .ok_or(ClaimFactsError::MalformedRequiredField)
}

/// SQL boolean: every lifecycle column the typed row `typed` copies from its
/// registry row `registry` still agrees with it.
fn lifecycle_agrees_sql(typed: &str, registry: &str) -> String {
    format!(
        "{typed}.created_commit_seq={registry}.created_commit_seq
         AND {typed}.invalidated_commit_seq IS {registry}.invalidated_commit_seq
         AND {typed}.superseded_by IS {registry}.superseded_by
         AND {typed}.sensitivity_class={registry}.sensitivity_class"
    )
}

/// The decision row is written from the same spec as its registry row, so a
/// lifecycle disagreement is corruption, not a fact. `ObjectRow` decodes an
/// unrecognized class as `Secret`; the stored text is decoded here instead.
fn load_decision(
    tx: &Transaction<'_>,
    object: &ObjectRow,
) -> Result<ClaimDecisionFacts, ClaimFactsError> {
    static SQL: LazyLock<String> = LazyLock::new(|| {
        format!(
            "SELECT d.decision_id,d.decision_kind,d.proposition_id,d.scope_id,d.anchor_id,
                    d.evidence_id,{agrees},o.sensitivity_class
             FROM decisions d
             JOIN object_registry o ON o.object_id=d.object_id
             WHERE d.object_id=?1",
            agrees = lifecycle_agrees_sql("d", "o"),
        )
    });
    let mut statement = tx.prepare_cached(&SQL).map_err(map_sqlite)?;
    let row: Option<(ClaimDecisionFacts, bool, String)> = statement
        .query_row([&object.object_id], |row| {
            Ok((
                ClaimDecisionFacts {
                    decision_id: row.get(0)?,
                    decision_kind: row.get(1)?,
                    proposition_id: row.get(2)?,
                    scope_id: row.get(3)?,
                    anchor_id: row.get(4)?,
                    evidence_id: row.get(5)?,
                },
                row.get(6)?,
                row.get(7)?,
            ))
        })
        .optional()
        .map_err(map_sqlite)?;
    let (decision, agrees, registry_sensitivity) = row.ok_or(KernelError::CorruptCanonicalRow)?;
    if !agrees {
        return Err(KernelError::CorruptCanonicalRow.into());
    }
    sensitivity_field(&registry_sensitivity)?;
    Ok(decision)
}

#[derive(Clone, Copy)]
enum AdmissionScope {
    Own,
    Lineage,
}

struct RawAdmission {
    admission_decision_id: String,
    commit_seq: i64,
    event_kind: String,
    maturity: String,
    effective_maturity: String,
    disposition: String,
    visibility: String,
    outcome: String,
    source_class: String,
    taint_class: String,
    policy_revision: i64,
    sensitivity: String,
    evidence_id: Option<String>,
    approval_object_id: Option<String>,
    approval_valid: bool,
    trigger_object_id: Option<String>,
    elevated_support: bool,
}

/// One statement per scope, built once: the row the serving view selects for
/// `o` at `:governing_as_of`, with the supporting approval's validity there.
fn admission_sql(scope: AdmissionScope) -> String {
    let selection = match scope {
        AdmissionScope::Own => served_own_decision_sql("a"),
        AdmissionScope::Lineage => served_lineage_decision_sql("a"),
    };
    let approval_valid = supporting_approval_valid_sql("d.approval_object_id");
    format!(
        "SELECT d.admission_decision_id,d.commit_seq,d.event_kind,d.maturity,
                d.effective_maturity,d.disposition,d.visibility,d.outcome,d.source_class,
                d.taint_class,d.policy_revision,d.sensitivity_class,d.evidence_id,
                d.approval_object_id,{approval_valid},d.trigger_object_id,d.elevated_support
         FROM object_registry o
         JOIN admission_decisions d ON d.admission_decision_id={selection}
         WHERE o.object_id=:object_id"
    )
}

fn load_admission(
    statement: &mut Statement<'_>,
    requested: i64,
    object: &ObjectRow,
) -> Result<Option<AdmissionFacts>, ClaimFactsError> {
    let raw = statement
        .query_row(
            rusqlite::named_params! {
                ":object_id": object.object_id,
                ":governing_as_of": requested,
            },
            |row| {
                Ok(RawAdmission {
                    admission_decision_id: row.get(0)?,
                    commit_seq: row.get(1)?,
                    event_kind: row.get(2)?,
                    maturity: row.get(3)?,
                    effective_maturity: row.get(4)?,
                    disposition: row.get(5)?,
                    visibility: row.get(6)?,
                    outcome: row.get(7)?,
                    source_class: row.get(8)?,
                    taint_class: row.get(9)?,
                    policy_revision: row.get(10)?,
                    sensitivity: row.get(11)?,
                    evidence_id: row.get(12)?,
                    approval_object_id: row.get(13)?,
                    approval_valid: row.get(14)?,
                    trigger_object_id: row.get(15)?,
                    elevated_support: row.get(16)?,
                })
            },
        )
        .optional()
        .map_err(map_sqlite)?;
    raw.map(interpret_admission).transpose()
}

fn interpret_admission(raw: RawAdmission) -> Result<AdmissionFacts, ClaimFactsError> {
    fn field<T: for<'a> TryFrom<&'a str>>(value: &str) -> Result<T, ClaimFactsError> {
        T::try_from(value).map_err(|_| ClaimFactsError::MalformedRequiredField)
    }
    Ok(AdmissionFacts {
        admission_decision_id: raw.admission_decision_id,
        commit_seq: raw.commit_seq,
        event_kind: field(&raw.event_kind)?,
        historical_maturity: field(&raw.maturity)?,
        effective_maturity: field(&raw.effective_maturity)?,
        disposition: field(&raw.disposition)?,
        visibility: field(&raw.visibility)?,
        outcome: field(&raw.outcome)?,
        source_class: field(&raw.source_class)?,
        taint_class: field(&raw.taint_class)?,
        policy_revision: raw.policy_revision,
        sensitivity: sensitivity_field(&raw.sensitivity)?,
        evidence_id: raw.evidence_id,
        supporting_approval: raw.approval_object_id.map(|object_id| SupportingApproval {
            object_id,
            valid_at_snapshot: raw.approval_valid,
        }),
        trigger_object_id: raw.trigger_object_id,
        elevated_support: raw.elevated_support,
    })
}

/// The identity a claim decision publishes under each class it can carry.
fn claim_identity(class: OccurrenceClass, object_id: &str) -> Option<[(&'static str, &str); 1]> {
    match class {
        OccurrenceClass::CanonicalClaims => Some([("object_id", object_id)]),
        OccurrenceClass::PromotedMemory => Some([("decision_object_id", object_id)]),
        _ => None,
    }
}

/// `wanted`, when given, names the class and representation pairs read; every claim representation is read otherwise.
fn load_occurrences(
    tx: &Transaction<'_>,
    requested: i64,
    object: &ObjectRow,
    wanted: Option<&[(OccurrenceClass, &str)]>,
) -> Result<(Vec<ClaimOccurrence>, Vec<ExcludedRepresentation>), KernelError> {
    let revision = object.source_revision.to_string();
    let mut occurrences = Vec::new();
    let mut excluded = Vec::new();
    for class in [
        OccurrenceClass::CanonicalClaims,
        OccurrenceClass::PromotedMemory,
    ] {
        let identity = claim_identity(class, &object.object_id)
            .expect("both claim classes have a one-field identity");
        for representation in class.representations() {
            if wanted.is_some_and(|wanted| !wanted.contains(&(class, *representation))) {
                continue;
            }
            let encoded = encode_preserving_span(&Occurrence {
                class: class.code(),
                identity: &identity,
                revision: &revision,
                representation,
                span: None,
            });
            let Ok(encoded) = encoded else {
                excluded.push(ExcludedRepresentation {
                    class,
                    representation,
                    reason: RepresentationExclusion::MalformedIdentity,
                });
                continue;
            };
            let descriptor_object_id = descriptor_object_id(&encoded.lineage_id, &revision);
            let descriptor = load_descriptor(tx, requested, &descriptor_object_id)?;
            match descriptor {
                None => excluded.push(ExcludedRepresentation {
                    class,
                    representation,
                    reason: RepresentationExclusion::NoDescriptor,
                }),
                Some(raw) => {
                    let detail = stored_detail(&raw.payload)?;
                    let reencoded =
                        reencoded_identity(&detail).ok_or(KernelError::CorruptCanonicalRow)?;
                    if reencoded.tuple != encoded.tuple
                        || !raw.lifecycle_agrees
                        || raw.observation_id
                            != format!("{OCCURRENCE_ID_PREFIX}{}", encoded.occurrence_id)
                        || raw.source_kind != class.code()
                        || raw.source_revision != object.source_revision
                        || detail.lineage_id != raw.source_id
                        || detail.evidence_id != raw.evidence_id
                        || detail.artifact_digest != raw.artifact_digest
                        || detail.payload_id != raw.artifact_digest
                    {
                        return Err(KernelError::CorruptCanonicalRow);
                    }
                    occurrences.push(ClaimOccurrence {
                        class,
                        representation,
                        descriptor_object_id,
                        occurrence_id: detail.occurrence_id,
                        lineage_id: detail.lineage_id,
                        payload_id: detail.payload_id,
                        artifact_digest: detail.artifact_digest,
                        evidence_id: detail.evidence_id,
                        descriptor_commit_seq: raw.created_commit_seq,
                    });
                }
            }
        }
    }
    Ok((occurrences, excluded))
}

struct RawDescriptor {
    payload: Vec<u8>,
    observation_id: String,
    source_kind: String,
    source_revision: i64,
    lifecycle_agrees: bool,
    created_commit_seq: i64,
    source_id: String,
    evidence_id: String,
    artifact_digest: String,
}

/// Returns the descriptor row visible at `requested` under `Descriptors::LiveAtEnd`.
fn load_descriptor(
    tx: &Transaction<'_>,
    requested: i64,
    descriptor_object_id: &str,
) -> Result<Option<RawDescriptor>, KernelError> {
    static SQL: LazyLock<String> = LazyLock::new(|| {
        format!(
            "SELECT b.observation_payload,b.observation_id,o.source_kind,o.source_revision,
                    {agrees},o.created_commit_seq,o.source_id,b.evidence_id,e.artifact_digest
             FROM object_registry o
             JOIN observations b ON b.object_id=o.object_id
             JOIN evidence_meta e ON e.evidence_id=b.evidence_id
             WHERE o.object_id=?1 AND b.observation_kind=?2 AND {live}",
            agrees = lifecycle_agrees_sql("b", "o"),
            live = Descriptors::LiveAtEnd.predicate("?3", "0"),
        )
    });
    let mut statement = tx.prepare_cached(&SQL).map_err(map_sqlite)?;
    statement
        .query_row(
            params![descriptor_object_id, SOURCE_DESCRIPTOR_KIND, requested],
            |row| {
                Ok(RawDescriptor {
                    payload: row.get(0)?,
                    observation_id: row.get(1)?,
                    source_kind: row.get(2)?,
                    source_revision: row.get(3)?,
                    lifecycle_agrees: row.get(4)?,
                    created_commit_seq: row.get(5)?,
                    source_id: row.get(6)?,
                    evidence_id: row.get(7)?,
                    artifact_digest: row.get(8)?,
                })
            },
        )
        .optional()
        .map_err(map_sqlite)
}
