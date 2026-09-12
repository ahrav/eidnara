//! Transactional writes for decision and observation slices.
//!
//! Inputs are redacted before persistence. Each successful write updates typed rows, object
//! registry state, redaction metadata, and the envelope change list in one transaction.

use rusqlite::{OptionalExtension, Transaction, params};
use serde::Serialize;

use super::{
    DecisionEventOutcome, DecisionEventSpec, DecisionSpec, DecisionWriteOutcome,
    ObservationDependencySpec, ObservationSpec, ObservationWriteOutcome, RetirementOutcome,
};
use crate::CachedSql;
use crate::envelope::{Envelope, ObjectRow, PendingChange};
use crate::object_write::{
    insert_registry, invalidate, map_write_error, record_fields, record_registry_fields,
    set_successor,
};
use crate::redaction::{RedactedField, identity_field, redact};
use crate::{KernelError, Sensitivity, map_sqlite};

/// Every `change_kind` an outbox row of `object_kind` `decision` can carry, in the order the writers below name them: insert, event append, correct, retire.
pub const DECISION_CHANGE_KINDS: [&str; 4] = [
    "decision_insert",
    "decision_event_append",
    "decision_correct",
    "decision_retire",
];

struct RedactedDecision {
    decision_id: RedactedField,
    object_id: RedactedField,
    domain_id: RedactedField,
    proposition_id: Option<RedactedField>,
    scope_id: Option<RedactedField>,
    anchor_id: Option<RedactedField>,
    evidence_id: Option<RedactedField>,
    decision_kind: RedactedField,
    payload: RedactedDecisionPayload,
    source_kind: RedactedField,
    source_id: RedactedField,
    source_revision: i64,
    sensitivity: Sensitivity,
}

struct RedactedDecisionPayload {
    summary: RedactedField,
    rationale: RedactedField,
}

#[derive(Serialize)]
struct StoredDecisionPayload<'a> {
    summary: &'a str,
    rationale: &'a str,
}

struct RedactedObservation {
    observation_id: RedactedField,
    object_id: RedactedField,
    domain_id: RedactedField,
    proposition_id: Option<RedactedField>,
    scope_id: Option<RedactedField>,
    anchor_id: Option<RedactedField>,
    evidence_id: Option<RedactedField>,
    observation_kind: RedactedField,
    payload: RedactedObservationPayload,
    observed_at: i64,
    dependencies: Vec<RedactedDependency>,
    source_kind: RedactedField,
    source_id: RedactedField,
    source_revision: i64,
    sensitivity: Sensitivity,
}

struct RedactedObservationPayload {
    summary: RedactedField,
    classification: RedactedField,
    detail: Option<RedactedField>,
}

#[derive(Serialize)]
struct StoredObservationPayload<'a> {
    summary: &'a str,
    classification: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<&'a str>,
}

struct RedactedDependency {
    object_id: RedactedField,
    kind: RedactedField,
    payload: Option<RedactedField>,
}

struct RedactedEvent {
    kind: RedactedField,
    summary: RedactedField,
    evidence_id: Option<RedactedField>,
    recorded_at: i64,
}

#[derive(Serialize)]
struct StoredEventPayload<'a> {
    summary: &'a str,
}

impl Envelope<'_> {
    /// Inserts a live decision and its registry and redaction records.
    ///
    /// Returns `InvalidInput` for invalid required fields or payload serialization, `NotFound`
    /// for missing live parents, and mapped storage errors for constraint or I/O failures.
    pub fn insert_decision(
        &mut self,
        spec: DecisionSpec,
    ) -> Result<DecisionWriteOutcome, KernelError> {
        self.guarded(|envelope| envelope.insert_decision_inner(spec))
    }

    fn insert_decision_inner(
        &mut self,
        spec: DecisionSpec,
    ) -> Result<DecisionWriteOutcome, KernelError> {
        let mut spec = RedactedDecision::new(spec)?;
        spec.sensitivity =
            fold_cited_evidence_class(self.tx, spec.evidence_id.as_ref(), spec.sensitivity)?;
        insert_decision(self.tx, self.commit_seq, &spec)?;
        let outcome = spec.outcome();
        self.changes.push(PendingChange {
            object: spec.object_row(self.commit_seq),
            kind: DECISION_CHANGE_KINDS[0],
            replaced_object_id: None,
            redactions: spec.text_fields(),
            audit: None,
        });
        Ok(outcome)
    }

    /// Inserts a live observation after validating every declared dependency.
    ///
    /// Alignment dependencies must reference live decisions; other dependencies may reference
    /// any live registered object. Validation finishes before registry insertion.
    pub fn insert_observation(
        &mut self,
        spec: ObservationSpec,
    ) -> Result<ObservationWriteOutcome, KernelError> {
        self.guarded(|envelope| {
            if crate::source_descriptor::uses_descriptor_namespace(&spec) {
                return Err(KernelError::InvalidInput);
            }
            envelope.insert_observation_inner(spec)
        })
    }

    pub(crate) fn insert_observation_inner(
        &mut self,
        spec: ObservationSpec,
    ) -> Result<ObservationWriteOutcome, KernelError> {
        let mut spec = RedactedObservation::new(spec)?;
        spec.sensitivity =
            fold_cited_evidence_class(self.tx, spec.evidence_id.as_ref(), spec.sensitivity)?;
        insert_observation(self.tx, self.commit_seq, &spec)?;
        let outcome = spec.outcome();
        self.changes.push(PendingChange {
            object: spec.object_row(self.commit_seq),
            kind: "observation_insert",
            replaced_object_id: None,
            redactions: spec.text_fields(),
            audit: None,
        });
        Ok(outcome)
    }

    /// Appends to a live decision and allocates its next ordinal inside this envelope.
    ///
    /// Ordinals increase from the current maximum under the envelope transaction. Concurrent
    /// writers are serialized by that transaction. Missing decisions or evidence return
    /// `NotFound`; negative timestamps and empty event kinds return `InvalidInput`.
    pub fn append_decision_event(
        &mut self,
        decision_id: &str,
        spec: DecisionEventSpec,
    ) -> Result<DecisionEventOutcome, KernelError> {
        self.guarded(|envelope| envelope.append_decision_event_inner(decision_id, spec))
    }

    fn append_decision_event_inner(
        &mut self,
        decision_id: &str,
        spec: DecisionEventSpec,
    ) -> Result<DecisionEventOutcome, KernelError> {
        let decision_id = identity_field(decision_id)?;
        let spec = RedactedEvent::new(spec)?;
        require_optional_live(
            self.tx,
            "evidence_meta",
            "evidence_id",
            spec.evidence_id.as_ref(),
        )?;
        let object = load_live_decision_object(self.tx, &decision_id.text)?;
        let ordinal = self
            .tx
            .query_row_cached(
                "SELECT COALESCE(MAX(event_ordinal),0)+1
                 FROM decision_events WHERE decision_id=?1",
                [&decision_id.text],
                |row| row.get::<_, i64>(0),
            )
            .map_err(crate::map_sqlite)?;
        let payload = serde_json::to_vec(&StoredEventPayload {
            summary: &spec.summary.text,
        })
        .map_err(|_| KernelError::InvalidInput)?;
        self.tx
            .execute_cached(
                "INSERT INTO decision_events(
                     decision_id,event_ordinal,commit_seq,event_kind,event_payload,evidence_id,
                     recorded_at
                 ) VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![
                    decision_id.text,
                    ordinal,
                    self.commit_seq,
                    spec.kind.text,
                    payload,
                    spec.evidence_id.as_ref().map(|value| value.text.as_str()),
                    spec.recorded_at,
                ],
            )
            .map_err(map_write_error)?;
        let owner_id = format!("{}:{ordinal}", decision_id.text);
        let mut event_fields = vec![
            ("decision_id".to_string(), decision_id.clone()),
            ("event_kind".to_string(), spec.kind.clone()),
            ("event_payload.summary".to_string(), spec.summary.clone()),
        ];
        push_optional(&mut event_fields, "evidence_id", &spec.evidence_id);
        record_fields(
            self.tx,
            "decision_events",
            &owner_id,
            &event_fields,
            self.commit_seq,
        )?;
        self.changes.push(PendingChange {
            object,
            kind: DECISION_CHANGE_KINDS[1],
            replaced_object_id: None,
            redactions: event_fields,
            audit: Some(serde_json::json!({
                "decision_id": decision_id.text,
                "event_ordinal": ordinal,
                "event_kind": spec.kind.text,
            })),
        });
        Ok(DecisionEventOutcome {
            decision_id: decision_id.text,
            event_ordinal: ordinal,
        })
    }

    /// Replaces a live decision while preserving domain and source identity.
    ///
    /// Source revision must increase. The old object is invalidated before the replacement and
    /// linked to its successor in the same transaction. Lost authority demotes dependents.
    pub fn correct_decision(
        &mut self,
        replaced_object_id: &str,
        replacement: DecisionSpec,
    ) -> Result<DecisionWriteOutcome, KernelError> {
        self.guarded(|envelope| envelope.correct_decision_inner(replaced_object_id, replacement))
    }

    fn correct_decision_inner(
        &mut self,
        replaced_object_id: &str,
        replacement: DecisionSpec,
    ) -> Result<DecisionWriteOutcome, KernelError> {
        let replaced_object_id = identity_field(replaced_object_id)?;
        let old = load_live_typed_object(self.tx, &replaced_object_id.text, "decision")?;
        let granted_before = self.subject_grants_authority(Some(&replaced_object_id.text))?;
        let mut replacement = RedactedDecision::new(replacement)?;
        // Succession carries the predecessor's classification forward: a
        // correction cannot relabel content below the class it was admitted
        // under, so the replacement is at least as classified as what it
        // replaces, and no lower than the evidence it cites.
        replacement.sensitivity = fold_cited_evidence_class(
            self.tx,
            replacement.evidence_id.as_ref(),
            replacement.sensitivity.restrictive(old.sensitivity),
        )?;
        // A replacement naming a decision that is already live folds the
        // predecessor into that survivor: the survivor's stored row, not the
        // spec, is what the predecessor's lineage is checked against, and no
        // row is written for it. A survivor classified below what the
        // correction requires, whether from the predecessor, the asserted
        // class, or the cited evidence, cannot absorb it: its row is not
        // rewritten, so the fold would publish under the weaker class.
        let survivor = load_live_decision_by_object(self.tx, &replacement.object_id.text)?;
        if let Some((survivor, _)) = &survivor {
            if survivor.object_id == old.object_id
                || survivor.sensitivity.restrictive(replacement.sensitivity) != survivor.sensitivity
            {
                return Err(KernelError::InvalidInput);
            }
            validate_successor(
                &old,
                &survivor.domain_id,
                &survivor.source_kind,
                &survivor.source_id,
                survivor.source_revision,
            )?;
        } else {
            validate_successor(
                &old,
                &replacement.domain_id.text,
                &replacement.source_kind.text,
                &replacement.source_id.text,
                replacement.source_revision,
            )?;
        }
        invalidate(
            self.tx,
            self.commit_seq,
            "decisions",
            &replaced_object_id.text,
        )?;
        let (object, outcome, mut redactions) = match survivor {
            Some((survivor, decision_id)) => (
                survivor,
                DecisionWriteOutcome {
                    decision_id,
                    object_id: replacement.object_id.text.clone(),
                },
                Vec::new(),
            ),
            None => {
                insert_decision(self.tx, self.commit_seq, &replacement)?;
                (
                    replacement.object_row(self.commit_seq),
                    replacement.outcome(),
                    replacement.text_fields(),
                )
            }
        };
        set_successor(
            self.tx,
            "decisions",
            &replaced_object_id.text,
            &replacement.object_id.text,
        )?;
        redactions.push(("replaced_object_id".to_string(), replaced_object_id.clone()));
        self.changes.push(PendingChange {
            object,
            kind: DECISION_CHANGE_KINDS[2],
            replaced_object_id: Some(replaced_object_id.text.clone()),
            redactions,
            audit: None,
        });
        if granted_before {
            self.demote_dependents_if_authority_lost(
                Some(&replaced_object_id.text),
                "authority corrected",
            )?;
        }
        Ok(outcome)
    }

    /// Replaces a live observation while preserving domain and source identity.
    ///
    /// Source revision must increase. The old object invalidation, replacement insertion, and
    /// successor link share the envelope transaction.
    pub fn correct_observation(
        &mut self,
        replaced_object_id: &str,
        replacement: ObservationSpec,
    ) -> Result<ObservationWriteOutcome, KernelError> {
        self.guarded(|envelope| {
            if crate::source_descriptor::uses_descriptor_namespace(&replacement) {
                return Err(KernelError::InvalidInput);
            }
            let descriptor: bool = envelope.tx.query_row_cached(
                "SELECT EXISTS(SELECT 1 FROM observations WHERE object_id=?1 AND observation_kind=?2)",
                params![replaced_object_id, crate::SOURCE_DESCRIPTOR_KIND],
                |row| row.get(0),
            ).map_err(map_sqlite)?;
            if descriptor {
                return Err(KernelError::InvalidInput);
            }
            envelope.correct_observation_inner(replaced_object_id, replacement)
        })
    }

    pub(crate) fn correct_observation_inner(
        &mut self,
        replaced_object_id: &str,
        replacement: ObservationSpec,
    ) -> Result<ObservationWriteOutcome, KernelError> {
        let replaced_object_id = identity_field(replaced_object_id)?;
        let old = load_live_typed_object(self.tx, &replaced_object_id.text, "observation")?;
        let mut replacement = RedactedObservation::new(replacement)?;
        // Succession carries the predecessor's classification forward, as for
        // a decision, and the cited evidence's class along with it.
        replacement.sensitivity = fold_cited_evidence_class(
            self.tx,
            replacement.evidence_id.as_ref(),
            replacement.sensitivity.restrictive(old.sensitivity),
        )?;
        validate_successor(
            &old,
            &replacement.domain_id.text,
            &replacement.source_kind.text,
            &replacement.source_id.text,
            replacement.source_revision,
        )?;
        invalidate(
            self.tx,
            self.commit_seq,
            "observations",
            &replaced_object_id.text,
        )?;
        insert_observation(self.tx, self.commit_seq, &replacement)?;
        set_successor(
            self.tx,
            "observations",
            &replaced_object_id.text,
            &replacement.object_id.text,
        )?;
        let outcome = replacement.outcome();
        let mut redactions = replacement.text_fields();
        redactions.push(("replaced_object_id".to_string(), replaced_object_id.clone()));
        self.changes.push(PendingChange {
            object: replacement.object_row(self.commit_seq),
            kind: "observation_correct",
            replaced_object_id: Some(replaced_object_id.text),
            redactions,
            audit: None,
        });
        Ok(outcome)
    }

    /// Invalidates a live decision and demotes dependents if its authority is lost.
    pub fn retire_decision(&mut self, object_id: &str) -> Result<RetirementOutcome, KernelError> {
        self.guarded(|envelope| envelope.retire_slice_object(object_id, "decision", "decisions"))
    }

    /// Invalidates a live observation without deleting its historical row.
    pub fn retire_observation(
        &mut self,
        object_id: &str,
    ) -> Result<RetirementOutcome, KernelError> {
        self.guarded(|envelope| {
            envelope.retire_slice_object(object_id, "observation", "observations")
        })
    }

    /// Invalidates a live evidence object without deleting its historical row. Rejects retirement while a live observation, decision (including its events), or asserted edge cites the evidence.
    /// Evidence metadata can carry a stricter class than the registry, so retirement folds it into the change payload.
    ///
    /// # Errors
    ///
    /// `NotFound` when `object_id` is not a live evidence object; `Conflict` when a live row still cites the evidence.
    pub fn retire_evidence(&mut self, object_id: &str) -> Result<RetirementOutcome, KernelError> {
        self.guarded(|envelope| envelope.retire_evidence_inner(object_id))
    }

    fn retire_evidence_inner(&mut self, object_id: &str) -> Result<RetirementOutcome, KernelError> {
        let object_id = identity_field(object_id)?;
        let mut object = load_live_typed_object(self.tx, &object_id.text, "evidence")?;
        let (sensitivity, cited): (String, bool) = self
            .tx
            .query_row_cached(
                "SELECT e.sensitivity_class,
                        (EXISTS(SELECT 1 FROM observations o
                                   WHERE o.evidence_id=e.evidence_id
                                     AND o.invalidated_commit_seq IS NULL)
                         OR EXISTS(SELECT 1 FROM decisions d
                                   WHERE d.evidence_id=e.evidence_id
                                     AND d.invalidated_commit_seq IS NULL)
                         OR EXISTS(SELECT 1 FROM decision_events de
                                   JOIN decisions d ON d.decision_id=de.decision_id
                                   WHERE de.evidence_id=e.evidence_id
                                     AND d.invalidated_commit_seq IS NULL)
                         OR EXISTS(SELECT 1 FROM asserted_edges a
                                   WHERE a.evidence_id=e.evidence_id
                                     AND a.invalidated_commit_seq IS NULL))
                 FROM evidence_meta e
                 WHERE e.object_id=?1 AND e.invalidated_commit_seq IS NULL",
                [&object_id.text],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(map_sqlite)?
            .ok_or(KernelError::NotFound)?;
        if cited {
            return Err(KernelError::Conflict);
        }
        invalidate(self.tx, self.commit_seq, "evidence_meta", &object_id.text)?;
        object.sensitivity = object
            .sensitivity
            .restrictive(Sensitivity::from_stored(&sensitivity));
        object.invalidated_commit_seq = Some(self.commit_seq);
        self.changes.push(PendingChange {
            object,
            kind: "evidence_retire",
            replaced_object_id: None,
            redactions: vec![("object_id".to_string(), object_id.clone())],
            audit: None,
        });
        Ok(RetirementOutcome {
            object_id: object_id.text,
            object_kind: "evidence".to_string(),
        })
    }

    fn retire_slice_object(
        &mut self,
        object_id: &str,
        object_kind: &'static str,
        table: &'static str,
    ) -> Result<RetirementOutcome, KernelError> {
        let object_id = identity_field(object_id)?;
        let mut object = load_live_typed_object(self.tx, &object_id.text, object_kind)?;
        // Retiring an accepted decision withdraws any authority it granted, so its
        // dependents follow exactly as they do for an explicit revocation. Sampled
        // before the invalidation, which is what removes that authority.
        let granted_before = self.subject_grants_authority(Some(&object_id.text))?;
        invalidate(self.tx, self.commit_seq, table, &object_id.text)?;
        object.invalidated_commit_seq = Some(self.commit_seq);
        self.changes.push(PendingChange {
            object,
            kind: match object_kind {
                "decision" => DECISION_CHANGE_KINDS[3],
                _ => "observation_retire",
            },
            replaced_object_id: None,
            redactions: vec![("object_id".to_string(), object_id.clone())],
            audit: None,
        });
        if granted_before {
            self.demote_dependents_if_authority_lost(Some(&object_id.text), "authority retired")?;
        }
        Ok(RetirementOutcome {
            object_id: object_id.text,
            object_kind: object_kind.to_string(),
        })
    }
}

impl RedactedDecision {
    fn new(spec: DecisionSpec) -> Result<Self, KernelError> {
        require_spec_fields(
            &[
                &spec.decision_id,
                &spec.object_id,
                &spec.domain_id,
                &spec.decision_kind,
                &spec.source_kind,
                &spec.source_id,
            ],
            spec.source_revision,
        )?;
        Ok(Self {
            decision_id: identity_field(&spec.decision_id)?,
            object_id: identity_field(&spec.object_id)?,
            domain_id: identity_field(&spec.domain_id)?,
            proposition_id: spec
                .proposition_id
                .as_deref()
                .map(identity_field)
                .transpose()?,
            scope_id: spec.scope_id.as_deref().map(identity_field).transpose()?,
            anchor_id: spec.anchor_id.as_deref().map(identity_field).transpose()?,
            evidence_id: spec
                .evidence_id
                .as_deref()
                .map(identity_field)
                .transpose()?,
            decision_kind: redact(&spec.decision_kind)?,
            payload: RedactedDecisionPayload {
                summary: redact(&spec.payload.summary)?,
                rationale: redact(&spec.payload.rationale)?,
            },
            source_kind: identity_field(&spec.source_kind)?,
            source_id: identity_field(&spec.source_id)?,
            source_revision: spec.source_revision,
            sensitivity: spec.sensitivity,
        })
    }

    fn object_row(&self, commit_seq: i64) -> ObjectRow {
        ObjectRow {
            object_id: self.object_id.text.clone(),
            object_kind: "decision".to_string(),
            domain_id: self.domain_id.text.clone(),
            source_kind: self.source_kind.text.clone(),
            source_id: self.source_id.text.clone(),
            source_revision: self.source_revision,
            created_commit_seq: commit_seq,
            invalidated_commit_seq: None,
            superseded_by: None,
            sensitivity: self.sensitivity,
        }
    }

    fn outcome(&self) -> DecisionWriteOutcome {
        DecisionWriteOutcome {
            decision_id: self.decision_id.text.clone(),
            object_id: self.object_id.text.clone(),
        }
    }

    fn text_fields(&self) -> Vec<(String, RedactedField)> {
        let mut fields = vec![
            ("decision_id".to_string(), self.decision_id.clone()),
            ("object_id".to_string(), self.object_id.clone()),
            ("domain_id".to_string(), self.domain_id.clone()),
            ("decision_kind".to_string(), self.decision_kind.clone()),
            ("source_kind".to_string(), self.source_kind.clone()),
            ("source_id".to_string(), self.source_id.clone()),
            (
                "decision_payload.summary".to_string(),
                self.payload.summary.clone(),
            ),
            (
                "decision_payload.rationale".to_string(),
                self.payload.rationale.clone(),
            ),
        ];
        push_optional(&mut fields, "proposition_id", &self.proposition_id);
        push_optional(&mut fields, "scope_id", &self.scope_id);
        push_optional(&mut fields, "anchor_id", &self.anchor_id);
        push_optional(&mut fields, "evidence_id", &self.evidence_id);
        fields
    }
}

impl RedactedObservation {
    fn new(spec: ObservationSpec) -> Result<Self, KernelError> {
        require_spec_fields(
            &[
                &spec.observation_id,
                &spec.object_id,
                &spec.domain_id,
                &spec.observation_kind,
                &spec.source_kind,
                &spec.source_id,
            ],
            spec.source_revision,
        )?;
        if spec.observed_at < 0 {
            return Err(KernelError::InvalidInput);
        }
        let dependencies = spec
            .dependencies
            .into_iter()
            .map(RedactedDependency::new)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            observation_id: identity_field(&spec.observation_id)?,
            object_id: identity_field(&spec.object_id)?,
            domain_id: identity_field(&spec.domain_id)?,
            proposition_id: spec
                .proposition_id
                .as_deref()
                .map(identity_field)
                .transpose()?,
            scope_id: spec.scope_id.as_deref().map(identity_field).transpose()?,
            anchor_id: spec.anchor_id.as_deref().map(identity_field).transpose()?,
            evidence_id: spec
                .evidence_id
                .as_deref()
                .map(identity_field)
                .transpose()?,
            observation_kind: redact(&spec.observation_kind)?,
            payload: RedactedObservationPayload {
                summary: redact(&spec.payload.summary)?,
                classification: redact(&spec.payload.classification)?,
                detail: spec.payload.detail.as_deref().map(redact).transpose()?,
            },
            observed_at: spec.observed_at,
            dependencies,
            source_kind: identity_field(&spec.source_kind)?,
            source_id: identity_field(&spec.source_id)?,
            source_revision: spec.source_revision,
            sensitivity: spec.sensitivity,
        })
    }

    fn object_row(&self, commit_seq: i64) -> ObjectRow {
        ObjectRow {
            object_id: self.object_id.text.clone(),
            object_kind: "observation".to_string(),
            domain_id: self.domain_id.text.clone(),
            source_kind: self.source_kind.text.clone(),
            source_id: self.source_id.text.clone(),
            source_revision: self.source_revision,
            created_commit_seq: commit_seq,
            invalidated_commit_seq: None,
            superseded_by: None,
            sensitivity: self.sensitivity,
        }
    }

    fn outcome(&self) -> ObservationWriteOutcome {
        ObservationWriteOutcome {
            observation_id: self.observation_id.text.clone(),
            object_id: self.object_id.text.clone(),
        }
    }

    fn text_fields(&self) -> Vec<(String, RedactedField)> {
        let mut fields = vec![
            ("observation_id".to_string(), self.observation_id.clone()),
            ("object_id".to_string(), self.object_id.clone()),
            ("domain_id".to_string(), self.domain_id.clone()),
            (
                "observation_kind".to_string(),
                self.observation_kind.clone(),
            ),
            ("source_kind".to_string(), self.source_kind.clone()),
            ("source_id".to_string(), self.source_id.clone()),
            (
                "observation_payload.summary".to_string(),
                self.payload.summary.clone(),
            ),
            (
                "observation_payload.classification".to_string(),
                self.payload.classification.clone(),
            ),
        ];
        push_optional(
            &mut fields,
            "observation_payload.detail",
            &self.payload.detail,
        );
        push_optional(&mut fields, "proposition_id", &self.proposition_id);
        push_optional(&mut fields, "scope_id", &self.scope_id);
        push_optional(&mut fields, "anchor_id", &self.anchor_id);
        push_optional(&mut fields, "evidence_id", &self.evidence_id);
        for (index, dependency) in self.dependencies.iter().enumerate() {
            fields.push((
                format!("dependencies.{index}.dependency_object_id"),
                dependency.object_id.clone(),
            ));
            fields.push((
                format!("dependencies.{index}.dependency_kind"),
                dependency.kind.clone(),
            ));
            push_optional(
                &mut fields,
                &format!("dependencies.{index}.dependency_payload"),
                &dependency.payload,
            );
        }
        fields
    }
}

impl RedactedDependency {
    fn new(spec: ObservationDependencySpec) -> Result<Self, KernelError> {
        if spec.dependency_object_id.trim().is_empty() || spec.dependency_kind.trim().is_empty() {
            return Err(KernelError::InvalidInput);
        }
        Ok(Self {
            object_id: identity_field(&spec.dependency_object_id)?,
            kind: redact(&spec.dependency_kind)?,
            payload: spec.dependency_payload.as_deref().map(redact).transpose()?,
        })
    }
}

impl RedactedEvent {
    fn new(spec: DecisionEventSpec) -> Result<Self, KernelError> {
        if spec.event_kind.trim().is_empty() || spec.recorded_at < 0 {
            return Err(KernelError::InvalidInput);
        }
        Ok(Self {
            kind: redact(&spec.event_kind)?,
            summary: redact(&spec.payload.summary)?,
            evidence_id: spec
                .evidence_id
                .as_deref()
                .map(identity_field)
                .transpose()?,
            recorded_at: spec.recorded_at,
        })
    }
}

fn insert_decision(
    tx: &Transaction<'_>,
    commit_seq: i64,
    spec: &RedactedDecision,
) -> Result<(), KernelError> {
    require_parents(
        tx,
        &spec.domain_id,
        spec.proposition_id.as_ref(),
        spec.scope_id.as_ref(),
        spec.anchor_id.as_ref(),
        spec.evidence_id.as_ref(),
    )?;
    insert_registry(tx, commit_seq, &spec.object_row(commit_seq))?;
    record_registry_fields(
        tx,
        &spec.object_id.text,
        &spec.domain_id,
        &spec.object_id,
        &spec.source_kind,
        &spec.source_id,
        commit_seq,
    )?;
    let payload = serde_json::to_vec(&StoredDecisionPayload {
        summary: &spec.payload.summary.text,
        rationale: &spec.payload.rationale.text,
    })
    .map_err(|_| KernelError::InvalidInput)?;
    tx.execute_cached(
        "INSERT INTO decisions(
             decision_id,object_id,proposition_id,scope_id,anchor_id,evidence_id,decision_kind,
             decision_payload,created_commit_seq,sensitivity_class
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![
            spec.decision_id.text,
            spec.object_id.text,
            optional_text(&spec.proposition_id),
            optional_text(&spec.scope_id),
            optional_text(&spec.anchor_id),
            optional_text(&spec.evidence_id),
            spec.decision_kind.text,
            payload,
            commit_seq,
            spec.sensitivity.as_str(),
        ],
    )
    .map_err(map_write_error)?;
    record_fields(
        tx,
        "decisions",
        &spec.decision_id.text,
        &spec.text_fields(),
        commit_seq,
    )
}

/// The class a row citing `evidence_id` must carry: at least the live
/// evidence's own. A decision or observation cannot be classified below the
/// material that supports it, whatever the caller asserted. An absent or
/// invalidated citation is left to `require_parents` to refuse.
fn fold_cited_evidence_class(
    tx: &Transaction<'_>,
    evidence_id: Option<&RedactedField>,
    asserted: Sensitivity,
) -> Result<Sensitivity, KernelError> {
    let Some(evidence_id) = evidence_id else {
        return Ok(asserted);
    };
    let stored: Option<String> = tx
        .query_row_cached(
            "SELECT sensitivity_class FROM evidence_meta
             WHERE evidence_id=?1 AND invalidated_commit_seq IS NULL",
            [evidence_id.text.as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(map_sqlite)?;
    Ok(match stored {
        Some(class) => asserted.restrictive(Sensitivity::from_stored(&class)),
        None => asserted,
    })
}

fn insert_observation(
    tx: &Transaction<'_>,
    commit_seq: i64,
    spec: &RedactedObservation,
) -> Result<(), KernelError> {
    require_parents(
        tx,
        &spec.domain_id,
        spec.proposition_id.as_ref(),
        spec.scope_id.as_ref(),
        spec.anchor_id.as_ref(),
        spec.evidence_id.as_ref(),
    )?;
    for dependency in &spec.dependencies {
        if dependency.kind.text == super::ALIGNMENT_DEPENDENCY_KIND {
            require_live_decision_object(tx, &dependency.object_id.text)?;
        } else {
            require_live(
                tx,
                "object_registry",
                "object_id",
                &dependency.object_id.text,
            )?;
        }
    }
    insert_registry(tx, commit_seq, &spec.object_row(commit_seq))?;
    record_registry_fields(
        tx,
        &spec.object_id.text,
        &spec.domain_id,
        &spec.object_id,
        &spec.source_kind,
        &spec.source_id,
        commit_seq,
    )?;
    let payload = serde_json::to_vec(&StoredObservationPayload {
        summary: &spec.payload.summary.text,
        classification: &spec.payload.classification.text,
        detail: spec
            .payload
            .detail
            .as_ref()
            .map(|value| value.text.as_str()),
    })
    .map_err(|_| KernelError::InvalidInput)?;
    tx.execute_cached(
        "INSERT INTO observations(
             observation_id,object_id,proposition_id,scope_id,anchor_id,evidence_id,
             observation_kind,observation_payload,observed_at,created_commit_seq,sensitivity_class
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![
            spec.observation_id.text,
            spec.object_id.text,
            optional_text(&spec.proposition_id),
            optional_text(&spec.scope_id),
            optional_text(&spec.anchor_id),
            optional_text(&spec.evidence_id),
            spec.observation_kind.text,
            payload,
            spec.observed_at,
            commit_seq,
            spec.sensitivity.as_str(),
        ],
    )
    .map_err(map_write_error)?;
    record_fields(
        tx,
        "observations",
        &spec.observation_id.text,
        &spec.text_fields(),
        commit_seq,
    )?;
    for dependency in &spec.dependencies {
        tx.execute_cached(
            "INSERT INTO observation_dependencies(
                 observation_id,dependency_object_id,dependency_kind,dependency_payload
             ) VALUES (?1,?2,?3,?4)",
            params![
                spec.observation_id.text,
                dependency.object_id.text,
                dependency.kind.text,
                dependency
                    .payload
                    .as_ref()
                    .map(|value| value.text.as_bytes()),
            ],
        )
        .map_err(map_write_error)?;
    }
    Ok(())
}

fn require_parents(
    tx: &Transaction<'_>,
    domain_id: &RedactedField,
    proposition_id: Option<&RedactedField>,
    scope_id: Option<&RedactedField>,
    anchor_id: Option<&RedactedField>,
    evidence_id: Option<&RedactedField>,
) -> Result<(), KernelError> {
    require_live(tx, "domains", "domain_id", &domain_id.text)?;
    require_optional_live(tx, "propositions", "proposition_id", proposition_id)?;
    require_optional_live(tx, "scopes", "scope_id", scope_id)?;
    require_optional_live(tx, "anchors", "anchor_id", anchor_id)?;
    require_optional_live(tx, "evidence_meta", "evidence_id", evidence_id)
}

fn require_optional_live(
    tx: &Transaction<'_>,
    table: &str,
    column: &str,
    value: Option<&RedactedField>,
) -> Result<(), KernelError> {
    match value {
        Some(value) => require_live(tx, table, column, &value.text),
        None => Ok(()),
    }
}

fn require_live_decision_object(tx: &Transaction<'_>, object_id: &str) -> Result<(), KernelError> {
    tx.query_row_cached(
        "SELECT 1
         FROM decisions d
         JOIN object_registry r ON r.object_id=d.object_id
         WHERE r.object_id=?1 AND r.object_kind='decision'
           AND r.invalidated_commit_seq IS NULL
           AND d.invalidated_commit_seq IS NULL",
        [object_id],
        |_| Ok(()),
    )
    .optional()
    .map_err(crate::map_sqlite)?
    .ok_or(KernelError::NotFound)
}

fn require_live(
    tx: &Transaction<'_>,
    table: &str,
    column: &str,
    value: &str,
) -> Result<(), KernelError> {
    let sql = format!("SELECT 1 FROM {table} WHERE {column}=?1 AND invalidated_commit_seq IS NULL");
    tx.query_row_cached(&sql, [value], |_| Ok(()))
        .optional()
        .map_err(crate::map_sqlite)?
        .ok_or(KernelError::NotFound)
}

fn load_live_decision_object(
    tx: &Transaction<'_>,
    decision_id: &str,
) -> Result<ObjectRow, KernelError> {
    tx.query_row_cached(
        "SELECT r.object_id,r.object_kind,r.domain_id,r.source_kind,r.source_id,
                r.source_revision,r.created_commit_seq,r.sensitivity_class
         FROM decisions d JOIN object_registry r ON r.object_id=d.object_id
         WHERE d.decision_id=?1 AND d.invalidated_commit_seq IS NULL
           AND r.invalidated_commit_seq IS NULL",
        [decision_id],
        row_to_object,
    )
    .optional()
    .map_err(crate::map_sqlite)?
    .ok_or(KernelError::NotFound)
}

/// The live decision whose object id is `object_id`, with its `decision_id`.
fn load_live_decision_by_object(
    tx: &Transaction<'_>,
    object_id: &str,
) -> Result<Option<(ObjectRow, String)>, KernelError> {
    tx.query_row_cached(
        "SELECT r.object_id,r.object_kind,r.domain_id,r.source_kind,r.source_id,
                r.source_revision,r.created_commit_seq,r.sensitivity_class,d.decision_id
         FROM decisions d JOIN object_registry r ON r.object_id=d.object_id
         WHERE d.object_id=?1 AND d.invalidated_commit_seq IS NULL
           AND r.invalidated_commit_seq IS NULL",
        [object_id],
        |row| Ok((row_to_object(row)?, row.get::<_, String>(8)?)),
    )
    .optional()
    .map_err(crate::map_sqlite)
}

fn load_live_typed_object(
    tx: &Transaction<'_>,
    object_id: &str,
    object_kind: &str,
) -> Result<ObjectRow, KernelError> {
    tx.query_row_cached(
        "SELECT object_id,object_kind,domain_id,source_kind,source_id,source_revision,
                created_commit_seq,sensitivity_class
         FROM object_registry
         WHERE object_id=?1 AND object_kind=?2 AND invalidated_commit_seq IS NULL",
        params![object_id, object_kind],
        row_to_object,
    )
    .optional()
    .map_err(crate::map_sqlite)?
    .ok_or(KernelError::NotFound)
}

/// Every caller's SELECT must project the same eight columns in this order.
/// `load_live_decision_by_object` appends `d.decision_id` at index 8.
///
/// `invalidated_commit_seq` is `None` because every caller filters `invalidated_commit_seq IS NULL`.
/// `superseded_by` is `None` because the projections omit it, so a live but superseded row decodes with `superseded_by: None`.
fn row_to_object(row: &rusqlite::Row<'_>) -> rusqlite::Result<ObjectRow> {
    let sensitivity: String = row.get(7)?;
    Ok(ObjectRow {
        object_id: row.get(0)?,
        object_kind: row.get(1)?,
        domain_id: row.get(2)?,
        source_kind: row.get(3)?,
        source_id: row.get(4)?,
        source_revision: row.get(5)?,
        created_commit_seq: row.get(6)?,
        invalidated_commit_seq: None,
        superseded_by: None,
        sensitivity: Sensitivity::from_stored(&sensitivity),
    })
}

fn validate_successor(
    old: &ObjectRow,
    domain_id: &str,
    source_kind: &str,
    source_id: &str,
    source_revision: i64,
) -> Result<(), KernelError> {
    if old.domain_id != domain_id || old.source_kind != source_kind || old.source_id != source_id {
        return Err(KernelError::InvalidInput);
    }
    if source_revision <= old.source_revision {
        return Err(KernelError::Conflict);
    }
    Ok(())
}

fn require_spec_fields(fields: &[&str], source_revision: i64) -> Result<(), KernelError> {
    if source_revision < 0 || fields.iter().any(|field| field.trim().is_empty()) {
        return Err(KernelError::InvalidInput);
    }
    Ok(())
}

fn optional_text(field: &Option<RedactedField>) -> Option<&str> {
    field.as_ref().map(|value| value.text.as_str())
}

fn push_optional(
    fields: &mut Vec<(String, RedactedField)>,
    name: &str,
    field: &Option<RedactedField>,
) {
    if let Some(field) = field {
        fields.push((name.to_string(), field.clone()));
    }
}
