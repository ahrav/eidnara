//! Local-file captures record project text read by a MemoryReviewer run as evidence and a typed observation.
//!
//! The evidence row carries the bytes' identity and the finite acquisition reference (`retain_until`); the observation carries the typed detail: the trusted project, the relative path, the capture time, the whole-buffer digest, and the captured range. The detail is a versioned JSON document in `ObservationPayload.detail`, not a descriptor class, so the frozen `OccurrenceClass` set and every descriptor reader are untouched. Generic observation writers cannot use this kind or these id prefixes, or retire an observation under them, and the writer here accepts a detail only when it agrees with the live MemoryReviewer-capture evidence row it cites. Expiry retires the observation and then the evidence; the artifact bytes stay for as long as any other live reference names their digest.

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::Digest;

use super::cas::ProviderEgress;
use super::envelope::commit_with_writer;
use super::memory_reviewer_hold::{
    MemoryReviewerHoldBinding, MemoryReviewerHoldKind, load_valid_hold,
};
use super::redaction::identity;
use super::review_staging::check_digest;
use super::slice::{EVIDENCE_CITED_SQL, ObservationPayload, ObservationSpec};
use super::{CachedSql, CommitIntent, Envelope, KernelError, KernelStore, Sensitivity, map_sqlite};

pub const LOCAL_FILE_KIND: &str = "local_file_capture";
/// `source_kind` of the evidence row a capture ingests; the capture writer accepts a detail only for evidence registered under it.
pub const LOCAL_FILE_SOURCE_KIND: &str = "local_file";
pub const LOCAL_FILE_DETAIL_VERSION: u32 = 1;
const OBSERVATION_ID_PREFIX: &str = "localfile:";
const OBJECT_ID_PREFIX: &str = "localfileobj:";
/// Expiries retired per maintenance call.
pub const MAX_EXPIRED_CAPTURES_PER_CALL: usize = 64;

/// What one expiry sweep did. `retired` captures lost their observation and evidence; `retained` captures lost only their observation because another live row still cites the evidence. A sweep with both at zero found no work in the page.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CaptureExpiry {
    pub retired: usize,
    pub retained: usize,
}
/// The reserved producer expiry receipts are written under; a caller cannot commit under it, so a receipt found under an expiry key was written by this sweep.
const EXPIRY_PRODUCER: &str = "local-file-expiry";
/// The reserved producer under which a run abandons a capture it could not complete.
const ABANDON_PRODUCER: &str = "local-file-abandon";

/// The typed detail stored with a local-file capture observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalFileDetail {
    pub detail_version: u32,
    pub project_digest: String,
    /// Path of the captured file relative to the trusted project root, as the run named it.
    pub relative_path: String,
    pub captured_at: i64,
    /// SHA-256 of the whole captured buffer; equal to the evidence row's artifact digest.
    pub buffer_digest: String,
    /// The half-open byte range of the file the capture holds; a whole-file capture spans `0..byte_length`.
    pub range: (u64, u64),
}

/// A live capture as the store holds it: the typed detail with the bindings and acquisition reference of the rows that carry it, so a run can tell its own capture from one seated under the same identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFileCapture {
    pub detail: LocalFileDetail,
    /// The domain the capture observation and its evidence are registered in.
    pub domain_id: String,
    /// The scope the capture observation was recorded under.
    pub scope_id: Option<String>,
    /// The evidence row's acquisition reference (`retain_until`).
    pub retain_until: i64,
}

#[derive(Debug)]
pub struct LocalFileCaptureRequest<'a> {
    pub project_digest: &'a str,
    pub relative_path: &'a str,
    pub captured_at: i64,
    pub domain_id: &'a str,
    pub scope_id: Option<&'a str>,
    /// The live MemoryReviewer-capture evidence row holding the captured bytes.
    pub evidence_id: &'a str,
    pub artifact_digest: &'a str,
    pub byte_length: u64,
}

/// Whether `object_id` names a local-file capture observation object.
pub(crate) fn is_local_file_object(object_id: &str) -> bool {
    object_id.starts_with(OBJECT_ID_PREFIX)
}

/// The evidence id of the capture a run under `hold` takes of the file at `relative_path` whose whole buffer has `artifact_digest`. The identity is the run's (project, MemoryStore incarnation, subject, generation), the bytes, and the path, so two projects, two MemoryStore incarnations, two runs, or two paths with identical bytes are distinct captures over one stored object. The path is hashed so the id stays within the store's field bound; the Kernel incarnation is not part of it because a hold binding already refuses another Kernel store.
pub fn local_file_capture_id(
    hold: &MemoryReviewerHoldBinding,
    artifact_digest: &str,
    relative_path: &str,
) -> String {
    format!(
        "curcap:{}:{}:{}:{}:{artifact_digest}:{:x}",
        hold.project_digest,
        hold.memstore_incarnation,
        hold.subject,
        hold.generation,
        sha2::Sha256::digest(relative_path.as_bytes())
    )
}

pub(crate) fn uses_local_file_namespace(spec: &ObservationSpec) -> bool {
    spec.observation_kind == LOCAL_FILE_KIND
        || spec.source_kind == LOCAL_FILE_KIND
        || spec.observation_id.starts_with(OBSERVATION_ID_PREFIX)
        || spec.object_id.starts_with(OBJECT_ID_PREFIX)
}

impl Envelope<'_> {
    /// Records the typed observation for one capture. The cited evidence must be a live, local-only MemoryReviewer capture whose digest, length, and finite `retain_until` agree with the request, registered as an evidence object of the request's domain under [`LOCAL_FILE_SOURCE_KIND`] with the relative path as its source id; the observation's sensitivity is the evidence row's, never weaker. The observation cites the evidence, so `retire_evidence` conflicts until the observation is retired first. The detail claims project-confined provenance, so its shape is checked here rather than trusted from the caller: the project digest is a lowercase hex digest and the relative path is ordinary components only. A relative path the redaction scanner would rewrite is refused, because a stored path must equal the path the run named.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::InvalidInput`] for an empty evidence id, a malformed project digest or relative path, a detail the scanner rewrites, or a serialization failure; [`KernelError::NotFound`] when no live, local-only MemoryReviewer-capture row registered that way matches the request; and the observation writer's errors otherwise.
    pub fn record_local_file_capture(
        &mut self,
        request: &LocalFileCaptureRequest<'_>,
    ) -> Result<(), KernelError> {
        if request.evidence_id.is_empty()
            || check_digest(request.project_digest).is_err()
            || !is_relative_path(request.relative_path)
        {
            return Err(KernelError::InvalidInput);
        }
        // The detail describes one registry row: a live evidence object of this domain, registered as a local-file source under the same relative path.
        let sensitivity: String = self
            .tx
            .query_row_cached(
                "SELECT e.sensitivity_class FROM evidence_meta e
                 JOIN object_registry g ON g.object_id=e.object_id
                 WHERE e.evidence_id=?1 AND e.artifact_digest=?2 AND e.byte_length=?3
                   AND e.retention_class=?4 AND e.provider_egress_class=?5
                   AND e.retain_until IS NOT NULL AND e.invalidated_commit_seq IS NULL
                   AND g.object_kind='evidence' AND g.invalidated_commit_seq IS NULL
                   AND g.domain_id=?6 AND g.source_kind=?7 AND g.source_id=?8",
                params![
                    request.evidence_id,
                    request.artifact_digest,
                    i64::try_from(request.byte_length).map_err(|_| KernelError::InvalidInput)?,
                    super::cas::MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS,
                    ProviderEgress::LocalOnly.as_str(),
                    request.domain_id,
                    LOCAL_FILE_SOURCE_KIND,
                    request.relative_path,
                ],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_sqlite)?
            .ok_or(KernelError::NotFound)?;
        let detail = LocalFileDetail {
            detail_version: LOCAL_FILE_DETAIL_VERSION,
            project_digest: request.project_digest.to_string(),
            relative_path: request.relative_path.to_string(),
            captured_at: request.captured_at,
            buffer_digest: request.artifact_digest.to_string(),
            range: (0, request.byte_length),
        };
        let detail_json =
            identity(&serde_json::to_string(&detail).map_err(|_| KernelError::InvalidInput)?)?;
        let object_id = format!("{OBJECT_ID_PREFIX}{}", request.evidence_id);
        let spec = ObservationSpec {
            observation_id: format!("{OBSERVATION_ID_PREFIX}{}", request.evidence_id),
            object_id: object_id.clone(),
            domain_id: request.domain_id.to_string(),
            proposition_id: None,
            scope_id: request.scope_id.map(str::to_string),
            anchor_id: None,
            evidence_id: Some(request.evidence_id.to_string()),
            observation_kind: LOCAL_FILE_KIND.to_string(),
            payload: ObservationPayload {
                summary: "local file capture".to_string(),
                classification: LOCAL_FILE_KIND.to_string(),
                detail: Some(detail_json),
            },
            observed_at: request.captured_at,
            dependencies: Vec::new(),
            source_kind: LOCAL_FILE_KIND.to_string(),
            source_id: request.evidence_id.to_string(),
            source_revision: self.commit_seq,
            sensitivity: Sensitivity::from_stored(&sensitivity).restrictive(Sensitivity::Sensitive),
        };
        self.insert_observation_inner(spec)?;
        self.local_file_objects
            .insert(request.evidence_id.to_string(), object_id);
        Ok(())
    }

    /// Registry inserts use several writers; commit validation admits `localfileobj:` ids only when this envelope's capture writer owns them.
    pub(super) fn check_local_file_ownership(&self) -> Result<(), KernelError> {
        self.check_reserved_ownership(OBJECT_ID_PREFIX, &self.local_file_objects)
    }
}

impl KernelStore {
    /// Retires the capture observation and evidence of every MemoryReviewer capture whose `retain_until` has passed and that no live hold still pins, at most [`MAX_EXPIRED_CAPTURES_PER_CALL`] per call, one commit per capture keyed by `now` and the evidence id. A capture that some other live row still cites keeps its evidence: its own observation is retired, and the capture leaves the sweep until that citation is gone, so a retained capture costs nothing on later calls and never displaces a newer expired one from the page. Ownership ends here; the artifact bytes are reclaimed later only if no other live reference names the digest. Returns how many captures were retired outright and how many were retained for a citation; either is work done, and a call that did neither found nothing to do in the page.
    ///
    /// Candidates are chosen under a reader lock; each commit re-evaluates the expiry and pin predicate under the writer, so a hold acquired or extended over a candidate in between keeps it. A capture whose retirement the store refuses (`NotFound`, `Conflict`, or `InvalidInput` from its own commit) is skipped and the sweep continues, so one such row cannot hold every capture behind it in the page; a row whose registry object is not a live evidence object can never be retired and is not a candidate at all. Receipts are written under a reserved producer, so no caller can seat a receipt under an expiry key and have it replayed in place of the retirement.
    ///
    /// # Errors
    ///
    /// Returns the first storage, lock, or fence error; captures retired before it stay retired.
    pub fn expire_local_file_captures(&self, now: i64) -> Result<CaptureExpiry, KernelError> {
        let expired = {
            let reader = self.lock_reader()?;
            expired_captures(&reader, now)?
        };
        let mut sweep = CaptureExpiry::default();
        for (evidence_id, evidence_object) in expired {
            let mut writer = self.lock_writer()?;
            let outcome = commit_with_writer(
                &mut writer,
                self.lease_epoch(),
                expiry_intent(now, &evidence_id),
                |envelope| retire_expired_capture(envelope, now, &evidence_id, &evidence_object),
                || Ok(()),
            );
            match outcome {
                // A replayed receipt is another sweep's retirement at this cutoff; this call's transaction did not run.
                Ok(receipt) if receipt.replayed => {}
                Ok(receipt) => match receipt.result.as_str() {
                    "retired" => sweep.retired += 1,
                    "retained" => sweep.retained += 1,
                    _ => {}
                },
                Err(KernelError::NotFound | KernelError::Conflict | KernelError::InvalidInput) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(sweep)
    }

    /// Retires a capture its run could not complete: the capture's own observations, then the evidence unless another live row cites it. For a run whose capture was ingested but could not be held or described, so the row does not stay live until its acquisition reference lapses. The caller proves it is that run: `hold_id` must be a live execution hold owned by `binding`, and the capture is named by the inputs [`local_file_capture_id`] derives it from, so a run can abandon only captures of its own identity. The commit runs under the reserved producer, so no caller can seat a receipt in its place. Idempotent for one capture.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::Conflict`] when the hold is not live and owned by `binding`, when a live hold pins the evidence (it belongs to a run and is left alone), or when another live row cites it after its own observations are retired; [`KernelError::NotFound`] when no live MemoryReviewer-capture evidence row has the derived id; and storage, lock, or fence errors otherwise.
    pub fn abandon_local_file_capture(
        &self,
        hold_id: &str,
        binding: &MemoryReviewerHoldBinding,
        artifact_digest: &str,
        relative_path: &str,
    ) -> Result<(), KernelError> {
        let evidence_id = local_file_capture_id(binding, artifact_digest, relative_path);
        let mut writer = self.lock_writer()?;
        commit_with_writer(
            &mut writer,
            self.lease_epoch(),
            CommitIntent {
                producer: format!(
                    "{}{ABANDON_PRODUCER}",
                    CommitIntent::RESERVED_PRODUCER_PREFIX
                ),
                operation_key: evidence_id.clone(),
                request_digest: format!("{:x}", sha2::Sha256::digest(evidence_id.as_bytes())),
                actor: ABANDON_PRODUCER.to_string(),
                cause: "capture abandoned by its run".to_string(),
            },
            |envelope| {
                self.check_incarnation(envelope.tx, binding)
                    .map_err(|_| KernelError::Conflict)?;
                load_valid_hold(
                    envelope.tx,
                    hold_id,
                    MemoryReviewerHoldKind::Execution,
                    binding,
                    super::current_time_ms(),
                )
                .map_err(|_| KernelError::Conflict)?;
                let (evidence_object, pinned): (String, bool) = envelope
                    .tx
                    .query_row_cached(
                        "SELECT e.object_id,EXISTS(SELECT 1 FROM capture_pin_refs r
                                 JOIN capture_pins p ON p.capture_pin_id=r.capture_pin_id
                                 WHERE r.evidence_id=e.evidence_id AND r.released_at IS NULL
                                   AND p.released_at IS NULL AND p.purge_degraded_at IS NULL)
                         FROM evidence_meta e
                         WHERE e.evidence_id=?1 AND e.retention_class=?2
                           AND e.invalidated_commit_seq IS NULL",
                        params![
                            evidence_id,
                            super::cas::MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS
                        ],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()
                    .map_err(map_sqlite)?
                    .ok_or(KernelError::NotFound)?;
                if pinned {
                    return Err(KernelError::Conflict);
                }
                for observation in live_capture_observations(envelope, &evidence_id)? {
                    envelope.retire_capture_observation(&observation)?;
                }
                if cited_elsewhere(envelope, &evidence_id)? {
                    return Err(KernelError::Conflict);
                }
                envelope.retire_evidence(&evidence_object)?;
                Ok("abandoned".to_string())
            },
            || Ok(()),
        )
        .map(|_| ())
    }

    /// The live capture citing `evidence_id`: its typed detail with the domain, scope, and acquisition reference of the rows that carry it, or `None` when no such capture is live.
    ///
    /// # Errors
    ///
    /// Returns storage errors, and [`KernelError::CorruptCanonicalRow`] when the stored detail does not decode as a [`LocalFileDetail`] at the current version.
    pub fn local_file_capture(
        &self,
        evidence_id: &str,
    ) -> Result<Option<LocalFileCapture>, KernelError> {
        let reader = self.lock_reader()?;
        let row: Option<(Vec<u8>, String, Option<String>, i64)> = reader
            .query_row(
                "SELECT o.observation_payload,g.domain_id,o.scope_id,e.retain_until
                 FROM observations o
                 JOIN object_registry g ON g.object_id=o.object_id
                 JOIN evidence_meta e ON e.evidence_id=o.evidence_id
                 WHERE o.evidence_id=?1 AND o.observation_kind=?2
                   AND o.invalidated_commit_seq IS NULL AND e.invalidated_commit_seq IS NULL
                   AND e.retain_until IS NOT NULL",
                params![evidence_id, LOCAL_FILE_KIND],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(map_sqlite)?;
        row.map(|(payload, domain_id, scope_id, retain_until)| {
            serde_json::from_slice::<ObservationPayload>(&payload)
                .ok()
                .and_then(|payload| payload.detail)
                .and_then(|detail| serde_json::from_str::<LocalFileDetail>(&detail).ok())
                .filter(|detail| detail.detail_version == LOCAL_FILE_DETAIL_VERSION)
                .map(|detail| LocalFileCapture {
                    detail,
                    domain_id,
                    scope_id,
                    retain_until,
                })
                .ok_or(KernelError::CorruptCanonicalRow)
        })
        .transpose()
    }
}

/// The intent of one capture's expiry commit, keyed by the cutoff and the evidence id under the reserved producer.
fn expiry_intent(now: i64, evidence_id: &str) -> CommitIntent {
    CommitIntent {
        producer: format!(
            "{}{EXPIRY_PRODUCER}",
            CommitIntent::RESERVED_PRODUCER_PREFIX
        ),
        operation_key: format!("{now}:{evidence_id}"),
        request_digest: format!("{:x}", sha2::Sha256::digest(evidence_id.as_bytes())),
        actor: EXPIRY_PRODUCER.to_string(),
        cause: "acquisition reference expired".to_string(),
    }
}

/// Retires one candidate inside its own transaction: the capture's own observations, then the evidence unless another live row cites it. The candidate list was read outside this transaction, so the expiry and pin predicate is checked again first; a capture a hold has pinned since then is left alone as `Conflict`, which the sweep skips without a commit.
fn retire_expired_capture(
    envelope: &mut Envelope<'_>,
    now: i64,
    evidence_id: &str,
    evidence_object: &str,
) -> Result<String, KernelError> {
    let still_expired: bool = envelope
        .tx
        .query_row_cached(
            &format!(
                "SELECT EXISTS(SELECT 1 FROM evidence_meta e
                               WHERE e.evidence_id=?3 AND {EXPIRED_UNPINNED_SQL})"
            ),
            params![
                super::cas::MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS,
                now,
                evidence_id
            ],
            |row| row.get(0),
        )
        .map_err(map_sqlite)?;
    if !still_expired {
        return Err(KernelError::Conflict);
    }
    for observation in live_capture_observations(envelope, evidence_id)? {
        envelope.retire_capture_observation(&observation)?;
    }
    if cited_elsewhere(envelope, evidence_id)? {
        return Ok("retained".to_string());
    }
    envelope.retire_evidence(evidence_object)?;
    Ok("retired".to_string())
}

/// Whether `path` is a relative path of ordinary components: non-empty, no leading `/`, no NUL, and no empty, `.`, or `..` component.
fn is_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\0')
        && path
            .split('/')
            .all(|component| !matches!(component, "" | "." | ".."))
}

/// SQL predicate over an `evidence_meta` row aliased `e`: a live MemoryReviewer capture (`?1` the retention class) whose acquisition reference has passed at `?2` and that no live hold pins at `?2`. A live hold is unreleased, not purge-degraded, and unexpired, the same three conditions a hold must meet to be used. The candidate query and the per-capture recheck evaluate this same text.
const EXPIRED_UNPINNED_SQL: &str = "e.retention_class=?1 AND e.invalidated_commit_seq IS NULL
           AND e.retain_until IS NOT NULL AND e.retain_until<=?2
           AND NOT EXISTS(SELECT 1 FROM capture_pin_refs r
                            JOIN capture_pins p ON p.capture_pin_id=r.capture_pin_id
                            WHERE r.evidence_id=e.evidence_id
                              AND r.released_at IS NULL AND p.released_at IS NULL
                              AND p.purge_degraded_at IS NULL
                              AND (p.expires_at IS NULL OR p.expires_at>?2))";

/// Live MemoryReviewer captures whose acquisition reference has passed, that no live hold pins, whose registry object is a live evidence object, and that the sweep still has work for: `(evidence_id, evidence object id)`, oldest expiry first. A capture whose observation is already retired and whose evidence another live row cites has nothing left to retire until that citation goes, so it is not a candidate.
fn expired_captures(
    connection: &rusqlite::Connection,
    now: i64,
) -> Result<Vec<(String, String)>, KernelError> {
    let mut statement = connection
        .prepare_cached(&expired_captures_sql())
        .map_err(map_sqlite)?;
    let rows = statement
        .query_map(
            params![
                super::cas::MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS,
                now,
                i64::try_from(MAX_EXPIRED_CAPTURES_PER_CALL).unwrap_or(i64::MAX),
                LOCAL_FILE_KIND
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(map_sqlite)?
        .collect::<rusqlite::Result<_>>()
        .map_err(map_sqlite)?;
    Ok(rows)
}

/// Parameters: `?1` retention class, `?2` now, `?3` page size, `?4` the capture observation kind.
fn expired_captures_sql() -> String {
    format!(
        "SELECT e.evidence_id,e.object_id FROM evidence_meta e
         WHERE {EXPIRED_UNPINNED_SQL}
           AND EXISTS(SELECT 1 FROM object_registry g
                      WHERE g.object_id=e.object_id AND g.object_kind='evidence'
                        AND g.invalidated_commit_seq IS NULL)
           AND (EXISTS(SELECT 1 FROM observations c
                       WHERE c.evidence_id=e.evidence_id AND c.observation_kind=?4
                         AND c.invalidated_commit_seq IS NULL)
                OR NOT {EVIDENCE_CITED_SQL})
         ORDER BY e.retain_until,e.evidence_id
         LIMIT ?3"
    )
}

/// The object ids of the live capture observations citing `evidence_id`. The capture writer creates one per evidence row; every live one is retired so that `retire_evidence` sees only foreign citations.
fn live_capture_observations(
    envelope: &Envelope<'_>,
    evidence_id: &str,
) -> Result<Vec<String>, KernelError> {
    let mut statement = envelope
        .tx
        .prepare_cached(
            "SELECT object_id FROM observations
             WHERE evidence_id=?1 AND observation_kind=?2 AND invalidated_commit_seq IS NULL",
        )
        .map_err(map_sqlite)?;
    let rows = statement
        .query_map(params![evidence_id, LOCAL_FILE_KIND], |row| row.get(0))
        .map_err(map_sqlite)?
        .collect::<rusqlite::Result<_>>()
        .map_err(map_sqlite)?;
    Ok(rows)
}

/// Whether a live row still cites `evidence_id` once the capture's own observations are retired in this transaction; such a row is independent support the expiry must not remove. The predicate is the one `retire_evidence` enforces.
fn cited_elsewhere(envelope: &Envelope<'_>, evidence_id: &str) -> Result<bool, KernelError> {
    envelope
        .tx
        .query_row_cached(
            &format!(
                "SELECT {EVIDENCE_CITED_SQL} FROM evidence_meta e
                 WHERE e.evidence_id=?1"
            ),
            params![evidence_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(map_sqlite)
        .map(|cited| cited.unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use rusqlite::{Connection, StatementStatus, params};
    use sha2::Digest as _;

    use super::{
        CaptureExpiry, LOCAL_FILE_KIND, LocalFileCaptureRequest, MAX_EXPIRED_CAPTURES_PER_CALL,
        commit_with_writer, expired_captures_sql, expiry_intent, retire_expired_capture,
    };
    use crate::cas::MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS;
    use crate::schema::apply_kernel_schema;
    use crate::{
        ArtifactIngestRequest, CommitIntent, DomainSpec, KernelError, KernelStore,
        MemoryReviewerHoldBinding, ProviderEgress, Sensitivity,
    };

    const NOW: i64 = 10_000;

    fn intent(key: &str) -> CommitIntent {
        CommitIntent {
            producer: "test".to_string(),
            operation_key: key.to_string(),
            request_digest: "d".repeat(64),
            actor: "test".to_string(),
            cause: "test".to_string(),
        }
    }

    /// A store holding one capture whose acquisition reference lapses at the returned time, and the hold binding of a run over it. Ingest requires the reference to be live when the capture is created, so the time is the wall clock's.
    fn store_with_lapsing_capture(
        root: &std::path::Path,
    ) -> (KernelStore, MemoryReviewerHoldBinding, i64) {
        let lapses_at = crate::current_time_ms() + 60_000;
        let store = KernelStore::open(root).unwrap();
        store
            .commit(intent("seed"), |envelope| {
                envelope.insert_domain(DomainSpec {
                    domain_id: "domain".to_string(),
                    object_id: "domain-object".to_string(),
                    name: "domain".to_string(),
                    source_kind: "test".to_string(),
                    source_id: "domain".to_string(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                })?;
                Ok(String::new())
            })
            .unwrap();
        let body = b"captured project text";
        let digest = format!("{:x}", sha2::Sha256::digest(body));
        store
            .ingest_exact_artifact(ArtifactIngestRequest {
                intent: intent("ingest"),
                payload: body.to_vec(),
                evidence_id: "cap".to_string(),
                object_id: "cap-object".to_string(),
                object_kind: "evidence".to_string(),
                domain_id: "domain".to_string(),
                source_kind: "local_file".to_string(),
                source_id: "a.txt".to_string(),
                source_revision: 1,
                media_type: "text/plain".to_string(),
                retention_class: MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS.to_string(),
                retain_until: Some(lapses_at),
                asserted_sensitivity: Sensitivity::Sensitive,
                provider_egress: ProviderEgress::LocalOnly,
                provenance: None,
            })
            .unwrap();
        store
            .commit(intent("observe"), |envelope| {
                envelope.record_local_file_capture(&LocalFileCaptureRequest {
                    project_digest: &"a".repeat(64),
                    relative_path: "a.txt",
                    captured_at: 1,
                    domain_id: "domain",
                    scope_id: None,
                    evidence_id: "cap",
                    artifact_digest: &digest,
                    byte_length: u64::try_from(body.len()).unwrap(),
                })?;
                Ok(String::new())
            })
            .unwrap();
        let kernel_incarnation: String = Connection::open_with_flags(
            root.join("kernel.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap()
        .query_row(
            "SELECT database_incarnation_id FROM kernel_format_marker",
            [],
            |row| row.get(0),
        )
        .unwrap();
        let binding = MemoryReviewerHoldBinding {
            project_digest: "a".repeat(64),
            kernel_incarnation,
            memstore_incarnation: "m".repeat(32),
            subject: "job".to_string(),
            generation: 1,
        };
        (store, binding, lapses_at)
    }

    #[test]
    fn a_caller_cannot_seat_a_receipt_the_expiry_would_replay() {
        let root = tempfile::tempdir().unwrap();
        let (store, _, lapses_at) = store_with_lapsing_capture(root.path());
        let now = lapses_at + 1;
        // A caller commits under the very intent the sweep will use for this capture at this cutoff, claiming it was retained.
        assert_eq!(
            store.commit(expiry_intent(now, "cap"), |_| Ok("retained".to_string())),
            Err(KernelError::InvalidInput),
            "the expiry producer is reserved to the store"
        );
        assert_eq!(store.expire_local_file_captures(now).unwrap().retired, 1);
        assert!(store.local_file_capture("cap").unwrap().is_none());
    }

    #[test]
    fn a_replayed_expiry_receipt_is_not_counted_as_a_retirement() {
        let root = tempfile::tempdir().unwrap();
        let (store, _, lapses_at) = store_with_lapsing_capture(root.path());
        let now = lapses_at + 1;
        // Another sweep at the same cutoff committed this capture's receipt first; the closure here stands in for that sweep's retirement.
        let mut writer = store.lock_writer().unwrap();
        commit_with_writer(
            &mut writer,
            store.lease_epoch(),
            expiry_intent(now, "cap"),
            |_| Ok("retired".to_string()),
            || Ok(()),
        )
        .unwrap();
        drop(writer);
        assert_eq!(
            store.expire_local_file_captures(now).unwrap(),
            CaptureExpiry::default(),
            "a replayed receipt is the other sweep's retirement, not this one's"
        );
    }

    #[test]
    fn a_capture_pinned_after_it_was_selected_for_expiry_is_kept() {
        let root = tempfile::tempdir().unwrap();
        let (store, binding, lapses_at) = store_with_lapsing_capture(root.path());
        let now = lapses_at + 1;
        // Between the candidate query and this commit, a run acquires a hold over the lapsed capture.
        store
            .acquire_execution_hold(&binding, &["cap".to_string()], now + 60_000)
            .unwrap();
        let outcome = store.commit(intent("expire"), |envelope| {
            retire_expired_capture(envelope, now, "cap", "cap-object")
        });
        assert_eq!(
            outcome,
            Err(KernelError::Conflict),
            "a pinned capture is not retired under the hold"
        );
        assert!(
            store.local_file_capture("cap").unwrap().is_some(),
            "the capture's detail is still live"
        );
        // Unpinned, the same call retires it.
        let unpinned = tempfile::tempdir().unwrap();
        let (store, _, lapses_at) = store_with_lapsing_capture(unpinned.path());
        let receipt = store
            .commit(intent("expire"), |envelope| {
                retire_expired_capture(envelope, lapses_at + 1, "cap", "cap-object")
            })
            .unwrap();
        assert_eq!(receipt.result, "retired");
        assert!(store.local_file_capture("cap").unwrap().is_none());
    }

    /// One live capture whose acquisition reference lapsed at `NOW`, with its capture observation, plus `history` captures earlier sweeps already retired.
    fn store_with_retired_captures(history: usize) -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        apply_kernel_schema(&mut conn, "00000000000000000000000000000000", 0).unwrap();
        // The registry's domain reference is deferred, so the seed rows share one transaction.
        let tx = conn.transaction().unwrap();
        tx.execute_batch(
            "INSERT INTO commit_log VALUES (1,'seed',1,'test','seed','digest',0,'test','test');
             INSERT INTO commit_log VALUES (2,'retire',1,'test','retire','digest',0,'test','test');
             INSERT INTO object_registry(object_id,object_kind,domain_id,source_kind,source_id,
                 source_revision,created_commit_seq,sensitivity_class)
             VALUES ('domain','domain','domain','domain','domain',1,1,'normal');
             INSERT INTO domains(domain_id,object_id,name,created_commit_seq,sensitivity_class)
             VALUES ('domain','domain','domain',1,'normal');",
        )
        .unwrap();
        insert_capture(&tx, "live", NOW, None);
        for index in 0..history {
            insert_capture(&tx, &format!("retired-{index:06}"), 100, Some(2));
        }
        tx.commit().unwrap();
        conn
    }

    fn insert_capture(conn: &Connection, id: &str, retain_until: i64, invalidated: Option<i64>) {
        conn.execute(
            "INSERT INTO object_registry(object_id,object_kind,domain_id,source_kind,source_id,
                 source_revision,created_commit_seq,invalidated_commit_seq,sensitivity_class)
             VALUES (?1,'evidence','domain','local_file',?1,1,1,?2,'sensitive')",
            params![format!("{id}-object"), invalidated],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO evidence_meta(evidence_id,object_id,artifact_reference,artifact_digest,
                 byte_length,media_type,retention_class,retain_until,provider_egress_class,
                 redaction_metadata,created_commit_seq,invalidated_commit_seq,sensitivity_class)
             VALUES (?1,?2,'object',?1,1,'text/plain',?3,?4,'local_only',x'5b5d',1,?5,'sensitive')",
            params![
                id,
                format!("{id}-object"),
                MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS,
                retain_until,
                invalidated
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO object_registry(object_id,object_kind,domain_id,source_kind,source_id,
                 source_revision,created_commit_seq,invalidated_commit_seq,sensitivity_class)
             VALUES (?1,'observation','domain',?2,?3,1,1,?4,'sensitive')",
            params![
                format!("localfileobj:{id}"),
                LOCAL_FILE_KIND,
                id,
                invalidated
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO observations(observation_id,object_id,evidence_id,observation_kind,
                 observation_payload,observed_at,created_commit_seq,invalidated_commit_seq,
                 sensitivity_class)
             VALUES (?1,?2,?3,?4,x'7b7d',0,1,?5,'sensitive')",
            params![
                format!("localfile:{id}"),
                format!("localfileobj:{id}"),
                id,
                LOCAL_FILE_KIND,
                invalidated
            ],
        )
        .unwrap();
    }

    /// A capture whose only pin was degraded by a purge is a candidate: no run can use that pin, so it holds nothing.
    #[test]
    fn a_purge_degraded_pin_does_not_keep_a_lapsed_capture() {
        let conn = store_with_retired_captures(0);
        conn.execute_batch(
            "INSERT INTO deletion_backfill_barriers(barrier_id,artifact_digest,artifact_reference,
                 delete_commit_seq,created_at,completed_at) VALUES ('barrier','live','object',2,0,NULL);
             INSERT INTO capture_pins(capture_pin_id,pin_kind,owner_id,commit_seq,lease_epoch,writer_epoch,
                 created_at,expires_at,released_at,purge_degraded_at,purge_barrier_id)
             VALUES ('pin','memory_reviewer_execution_hold','owner',1,1,1,0,1000000,NULL,5,'barrier');
             INSERT INTO capture_pin_refs(capture_pin_id,evidence_id,expires_at) VALUES ('pin','live',1000000);",
        )
        .unwrap();
        let mut statement = conn.prepare(&expired_captures_sql()).unwrap();
        let found: Vec<String> = statement
            .query_map(
                params![
                    MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS,
                    NOW,
                    i64::try_from(MAX_EXPIRED_CAPTURES_PER_CALL).unwrap(),
                    LOCAL_FILE_KIND
                ],
                |row| row.get(0),
            )
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            found,
            ["live"],
            "a degraded pin is not a live pin; the capture is a candidate"
        );
    }

    /// SQLite VM steps the candidate query spends finding the one live expired capture.
    fn expired_captures_steps(history: usize) -> i32 {
        let conn = store_with_retired_captures(history);
        let mut statement = conn.prepare(&expired_captures_sql()).unwrap();
        let found: Vec<String> = statement
            .query_map(
                params![
                    MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS,
                    NOW,
                    i64::try_from(MAX_EXPIRED_CAPTURES_PER_CALL).unwrap(),
                    LOCAL_FILE_KIND
                ],
                |row| row.get(0),
            )
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            found,
            ["live"],
            "only the live expired capture is a candidate"
        );
        statement.get_status(StatementStatus::VmStep)
    }

    #[test]
    fn expiry_candidate_cost_does_not_grow_with_retired_history() {
        let baseline = expired_captures_steps(0);
        let with_history = expired_captures_steps(512);
        assert_eq!(
            with_history, baseline,
            "finding one expired capture behind 512 already-retired captures took {with_history} VM steps; \
             the same query with no history took {baseline}"
        );
    }
}
