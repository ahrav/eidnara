//! Local-file captures record project text read by a Curator run as evidence and a typed observation.
//!
//! The evidence row carries the bytes' identity and the finite acquisition reference (`retain_until`); the observation carries the typed detail: the trusted project, the relative path, the capture time, the whole-buffer digest, and the captured range. The detail is a versioned JSON document in `ObservationPayload.detail`, not a descriptor class, so the frozen `OccurrenceClass` set and every descriptor reader are untouched. Generic observation writers cannot use this kind or these id prefixes, and the writer here accepts a detail only when it agrees with the live Curator-capture evidence row it cites. Expiry retires the observation and then the evidence; the artifact bytes stay for as long as any other live reference names their digest.

use rusqlite::{OptionalExtension, named_params, params};
use serde::{Deserialize, Serialize};
use sha2::Digest;

use super::redaction::identity;
use super::slice::{ObservationPayload, ObservationSpec};
use super::{CachedSql, CommitIntent, Envelope, KernelError, KernelStore, Sensitivity, map_sqlite};

pub const LOCAL_FILE_KIND: &str = "local_file_capture";
pub const LOCAL_FILE_DETAIL_VERSION: u32 = 1;
const OBSERVATION_ID_PREFIX: &str = "localfile:";
const OBJECT_ID_PREFIX: &str = "localfileobj:";
/// Expiries retired per maintenance call.
pub const MAX_EXPIRED_CAPTURES_PER_CALL: usize = 64;
const EXPIRY_PRODUCER: &str = "kernel-local-file-expiry";

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

#[derive(Debug)]
pub struct LocalFileCaptureRequest<'a> {
    pub project_digest: &'a str,
    pub relative_path: &'a str,
    pub captured_at: i64,
    pub domain_id: &'a str,
    pub scope_id: Option<&'a str>,
    /// The live Curator-capture evidence row holding the captured bytes.
    pub evidence_id: &'a str,
    pub artifact_digest: &'a str,
    pub byte_length: u64,
}

pub(crate) fn uses_local_file_namespace(spec: &ObservationSpec) -> bool {
    spec.observation_kind == LOCAL_FILE_KIND
        || spec.source_kind == LOCAL_FILE_KIND
        || spec.observation_id.starts_with(OBSERVATION_ID_PREFIX)
        || spec.object_id.starts_with(OBJECT_ID_PREFIX)
}

impl Envelope<'_> {
    /// Records the typed observation for one capture. The cited evidence must be a live Curator capture whose digest, length, and finite `retain_until` agree with the request; the observation's sensitivity is the evidence row's, never weaker. The observation cites the evidence, so `retire_evidence` conflicts until the observation is retired first. A relative path the redaction scanner would rewrite is refused, because a stored path must equal the path the run named.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::InvalidInput`] for an empty path or evidence id, a detail the scanner rewrites, or a serialization failure; [`KernelError::NotFound`] when no live Curator-capture row matches the request; and the observation writer's errors otherwise.
    pub fn record_local_file_capture(
        &mut self,
        request: &LocalFileCaptureRequest<'_>,
    ) -> Result<(), KernelError> {
        if request.relative_path.is_empty() || request.evidence_id.is_empty() {
            return Err(KernelError::InvalidInput);
        }
        let sensitivity: String = self
            .tx
            .query_row_cached(
                "SELECT sensitivity_class FROM evidence_meta
                 WHERE evidence_id=?1 AND artifact_digest=?2 AND byte_length=?3
                   AND retention_class=?4 AND retain_until IS NOT NULL
                   AND invalidated_commit_seq IS NULL",
                params![
                    request.evidence_id,
                    request.artifact_digest,
                    i64::try_from(request.byte_length).map_err(|_| KernelError::InvalidInput)?,
                    super::cas::CURATOR_CAPTURE_RETENTION_CLASS,
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
        let spec = ObservationSpec {
            observation_id: format!("{OBSERVATION_ID_PREFIX}{}", request.evidence_id),
            object_id: format!("{OBJECT_ID_PREFIX}{}", request.evidence_id),
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
        Ok(())
    }
}

impl KernelStore {
    /// Retires the capture observation and evidence of every Curator capture whose `retain_until` has passed and that no live hold still pins, at most [`MAX_EXPIRED_CAPTURES_PER_CALL`] per call, one commit per capture keyed by `now` and the evidence id. A capture that some other live row still cites is left alone: only its own observation is retired, and the evidence stays until that citation is gone. Once its observation is retired, such a capture is not selected again while the citation lives, so a full page of retained captures costs no commits and never hides a later unreferenced one. Ownership ends here; the artifact bytes are reclaimed later only if no other live reference names the digest. Returns how many evidence rows were retired.
    ///
    /// # Errors
    ///
    /// Returns the first storage or commit error; captures retired before it stay retired.
    pub fn expire_local_file_captures(&self, now: i64) -> Result<usize, KernelError> {
        self.expire_local_file_captures_inner(now, || {})
    }

    /// [`Self::expire_local_file_captures`] with `between` run after the candidates are chosen and before the first retiring transaction, so a test can act in that window.
    #[cfg(feature = "test-support")]
    pub fn expire_local_file_captures_with_hook_for_test(
        &self,
        now: i64,
        between: impl FnOnce(),
    ) -> Result<usize, KernelError> {
        self.expire_local_file_captures_inner(now, between)
    }

    fn expire_local_file_captures_inner(
        &self,
        now: i64,
        between: impl FnOnce(),
    ) -> Result<usize, KernelError> {
        let expired = {
            let reader = self.lock_reader()?;
            expired_captures(&reader, now)?
        };
        between();
        let mut retired = 0;
        for (evidence_id, evidence_object) in expired {
            let intent = CommitIntent {
                producer: EXPIRY_PRODUCER.to_string(),
                operation_key: format!("{now}:{evidence_id}"),
                request_digest: format!("{:x}", sha2::Sha256::digest(evidence_id.as_bytes())),
                actor: EXPIRY_PRODUCER.to_string(),
                cause: "acquisition reference expired".to_string(),
            };
            let receipt = self.commit(intent, |envelope| {
                // A hold acquired since the candidates were chosen pins the capture again; the check the selection made outside this transaction is repeated inside it.
                if pinned(envelope, &evidence_id, now)? {
                    return Ok("pinned".to_string());
                }
                if let Some(observation) = live_capture_observation(envelope, &evidence_id)? {
                    envelope.retire_observation(&observation)?;
                }
                if cited_elsewhere(envelope, &evidence_id)? {
                    return Ok("retained".to_string());
                }
                envelope.retire_evidence(&evidence_object)?;
                Ok("retired".to_string())
            })?;
            if receipt.result == "retired" {
                retired += 1;
            }
        }
        Ok(retired)
    }

    /// The typed detail of the live capture observation citing `evidence_id`, or `None` when no such observation is live.
    ///
    /// # Errors
    ///
    /// Returns storage errors, and [`KernelError::CorruptCanonicalRow`] when the stored detail does not decode as a [`LocalFileDetail`] at the current version.
    pub fn local_file_capture(
        &self,
        evidence_id: &str,
    ) -> Result<Option<LocalFileDetail>, KernelError> {
        let reader = self.lock_reader()?;
        let payload: Option<Vec<u8>> = reader
            .query_row(
                "SELECT observation_payload FROM observations
                 WHERE evidence_id=?1 AND observation_kind=?2 AND invalidated_commit_seq IS NULL",
                params![evidence_id, LOCAL_FILE_KIND],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_sqlite)?;
        payload
            .map(|payload| {
                serde_json::from_slice::<ObservationPayload>(&payload)
                    .ok()
                    .and_then(|payload| payload.detail)
                    .and_then(|detail| serde_json::from_str::<LocalFileDetail>(&detail).ok())
                    .filter(|detail| detail.detail_version == LOCAL_FILE_DETAIL_VERSION)
                    .ok_or(KernelError::CorruptCanonicalRow)
            })
            .transpose()
    }
}

/// Whether a live, unexpired hold pins the evidence row `e` at `:now`. Shared by the sweep's selection and its in-transaction recheck so both agree on what counts as a pin.
const PINNED_SQL: &str = "EXISTS(SELECT 1 FROM capture_pin_refs r
                 JOIN capture_pins p ON p.capture_pin_id=r.capture_pin_id
                 WHERE r.evidence_id=e.evidence_id
                   AND r.released_at IS NULL AND p.released_at IS NULL
                   AND (p.expires_at IS NULL OR p.expires_at>:now))";

/// Whether a live row other than the capture observation cites the evidence row `e`; `:kind` is [`LOCAL_FILE_KIND`]. Shared by the sweep's selection and its in-transaction check so both agree on what counts as a citation.
const CITED_ELSEWHERE_SQL: &str = "EXISTS(SELECT 1 FROM observations o
                    WHERE o.evidence_id=e.evidence_id AND o.invalidated_commit_seq IS NULL
                      AND o.observation_kind<>:kind)
          OR EXISTS(SELECT 1 FROM decisions d
                    WHERE d.evidence_id=e.evidence_id AND d.invalidated_commit_seq IS NULL)
          OR EXISTS(SELECT 1 FROM decision_events de
                    JOIN decisions d ON d.decision_id=de.decision_id
                    WHERE de.evidence_id=e.evidence_id AND d.invalidated_commit_seq IS NULL)
          OR EXISTS(SELECT 1 FROM asserted_edges a
                    WHERE a.evidence_id=e.evidence_id AND a.invalidated_commit_seq IS NULL)";

/// Live Curator captures whose acquisition reference has passed, that no live hold pins, and that still have work: a live capture observation to retire, or evidence nothing else cites. `(evidence_id, evidence object id)`, oldest expiry first.
fn expired_captures(
    connection: &rusqlite::Connection,
    now: i64,
) -> Result<Vec<(String, String)>, KernelError> {
    let mut statement = connection
        .prepare_cached(&format!(
            "SELECT e.evidence_id,e.object_id FROM evidence_meta e
             WHERE e.retention_class=:class AND e.invalidated_commit_seq IS NULL
               AND e.retain_until IS NOT NULL AND e.retain_until<=:now
               AND NOT {PINNED_SQL}
               AND (EXISTS(SELECT 1 FROM observations o
                           WHERE o.evidence_id=e.evidence_id AND o.observation_kind=:kind
                             AND o.invalidated_commit_seq IS NULL)
                    OR NOT ({CITED_ELSEWHERE_SQL}))
             ORDER BY e.retain_until,e.evidence_id
             LIMIT :limit"
        ))
        .map_err(map_sqlite)?;
    let rows = statement
        .query_map(
            named_params! {
                ":class": super::cas::CURATOR_CAPTURE_RETENTION_CLASS,
                ":kind": LOCAL_FILE_KIND,
                ":now": now,
                ":limit": i64::try_from(MAX_EXPIRED_CAPTURES_PER_CALL).unwrap_or(i64::MAX),
            },
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(map_sqlite)?
        .collect::<rusqlite::Result<_>>()
        .map_err(map_sqlite)?;
    Ok(rows)
}

/// The object id of the live capture observation citing `evidence_id`, when there is one.
fn live_capture_observation(
    envelope: &Envelope<'_>,
    evidence_id: &str,
) -> Result<Option<String>, KernelError> {
    envelope
        .tx
        .query_row_cached(
            "SELECT object_id FROM observations
             WHERE evidence_id=?1 AND observation_kind=?2 AND invalidated_commit_seq IS NULL",
            params![evidence_id, LOCAL_FILE_KIND],
            |row| row.get(0),
        )
        .optional()
        .map_err(map_sqlite)
}

/// Whether a live, unexpired hold pins `evidence_id` at `now`, read inside the retiring transaction.
fn pinned(envelope: &Envelope<'_>, evidence_id: &str, now: i64) -> Result<bool, KernelError> {
    envelope
        .tx
        .query_row_cached(
            &format!("SELECT {PINNED_SQL} FROM evidence_meta e WHERE e.evidence_id=:evidence"),
            named_params! { ":evidence": evidence_id, ":now": now },
            |row| row.get(0),
        )
        .map_err(map_sqlite)
}

/// Whether a live row other than the capture observation cites `evidence_id`; such a row is independent support the expiry must not remove.
fn cited_elsewhere(envelope: &Envelope<'_>, evidence_id: &str) -> Result<bool, KernelError> {
    envelope
        .tx
        .query_row_cached(
            &format!(
                "SELECT {CITED_ELSEWHERE_SQL} FROM evidence_meta e WHERE e.evidence_id=:evidence"
            ),
            named_params! { ":evidence": evidence_id, ":kind": LOCAL_FILE_KIND },
            |row| row.get(0),
        )
        .map_err(map_sqlite)
}
