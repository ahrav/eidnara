//! Local-file captures record project text read by a Curator run as evidence and a typed observation.
//!
//! The evidence row carries the bytes' identity and the finite acquisition reference (`retain_until`); the observation carries the typed detail: the trusted project, the relative path, the capture time, the whole-buffer digest, and the captured range. The detail is a versioned JSON document in `ObservationPayload.detail`, not a descriptor class, so the frozen `OccurrenceClass` set and every descriptor reader are untouched. Generic observation writers cannot use this kind or these id prefixes. Expiry retires the observation and then the evidence; the artifact bytes stay for as long as any other live reference names their digest.

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::redaction::identity;
use super::slice::{ObservationPayload, ObservationSpec};
use super::{CommitIntent, Envelope, KernelError, KernelStore, Sensitivity, map_sqlite};

pub const LOCAL_FILE_KIND: &str = "local_file_capture";
pub const LOCAL_FILE_DETAIL_VERSION: u32 = 1;
const OBSERVATION_ID_PREFIX: &str = "localfile:";
const OBJECT_ID_PREFIX: &str = "localfileobj:";
/// Expiries retired per maintenance call.
pub const MAX_EXPIRED_CAPTURES_PER_CALL: usize = 64;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFileCaptureRequest<'a> {
    pub project_digest: &'a str,
    pub relative_path: &'a str,
    pub captured_at: i64,
    pub domain_id: &'a str,
    pub scope_id: Option<&'a str>,
    /// The live evidence row holding the captured bytes.
    pub evidence_id: &'a str,
    pub artifact_digest: &'a str,
    pub byte_length: u64,
    pub sensitivity: Sensitivity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFileCaptureOutcome {
    pub observation_id: String,
    pub object_id: String,
}

pub(crate) fn uses_local_file_namespace(spec: &ObservationSpec) -> bool {
    spec.observation_kind == LOCAL_FILE_KIND
        || spec.source_kind == LOCAL_FILE_KIND
        || spec.observation_id.starts_with(OBSERVATION_ID_PREFIX)
        || spec.object_id.starts_with(OBJECT_ID_PREFIX)
}

impl Envelope<'_> {
    /// Records the typed observation for one capture. The evidence must be live; the observation cites it, so `retire_evidence` conflicts until the observation is retired first. A relative path the redaction scanner would rewrite is refused, because a stored path must equal the path the run named.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::InvalidInput`] for an empty path or evidence id, a detail the scanner rewrites, or a serialization failure, and the observation writer's errors otherwise.
    pub fn record_local_file_capture(
        &mut self,
        request: &LocalFileCaptureRequest<'_>,
    ) -> Result<LocalFileCaptureOutcome, KernelError> {
        if request.relative_path.is_empty() || request.evidence_id.is_empty() {
            return Err(KernelError::InvalidInput);
        }
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
        let observation_id = format!("{OBSERVATION_ID_PREFIX}{}", request.evidence_id);
        let object_id = format!("{OBJECT_ID_PREFIX}{}", request.evidence_id);
        let spec = ObservationSpec {
            observation_id: observation_id.clone(),
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
            sensitivity: request.sensitivity,
        };
        self.insert_observation_inner(spec)?;
        Ok(LocalFileCaptureOutcome {
            observation_id,
            object_id,
        })
    }
}

impl KernelStore {
    /// Retires the capture observations and evidence of Curator captures whose `retain_until` has passed and that no live hold still pins, at most [`MAX_EXPIRED_CAPTURES_PER_CALL`] per call, in one commit. Ownership ends here; the artifact bytes are reclaimed later only if no other live reference names the digest. Returns how many evidence rows were retired.
    ///
    /// # Errors
    ///
    /// Returns the commit's error; nothing is retired when the commit fails.
    pub fn expire_local_file_captures(
        &self,
        intent: CommitIntent,
        now: i64,
    ) -> Result<usize, KernelError> {
        let mut retired = 0;
        self.commit(intent, |envelope| {
            for (evidence_id, evidence_object) in expired_captures(envelope, now)? {
                for observation in live_observations_citing(envelope, &evidence_id)? {
                    envelope.retire_observation(&observation)?;
                }
                envelope.retire_evidence(&evidence_object)?;
                retired += 1;
            }
            Ok(String::new())
        })?;
        Ok(retired)
    }

    /// The typed detail of the live capture observation citing `evidence_id`, or `None` when no such observation is live.
    ///
    /// # Errors
    ///
    /// Returns storage errors, and [`KernelError::CorruptCanonicalRow`] when the stored detail does not decode as a [`LocalFileDetail`].
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

/// Live Curator captures whose acquisition reference has passed and that no live hold pins: `(evidence_id, evidence object id)`, oldest expiry first.
fn expired_captures(
    envelope: &Envelope<'_>,
    now: i64,
) -> Result<Vec<(String, String)>, KernelError> {
    let mut statement = envelope
        .tx
        .prepare_cached(
            "SELECT e.evidence_id,e.object_id FROM evidence_meta e
             WHERE e.retention_class=?1 AND e.invalidated_commit_seq IS NULL
               AND e.retain_until IS NOT NULL AND e.retain_until<=?2
               AND NOT EXISTS(SELECT 1 FROM capture_pin_refs r
                                JOIN capture_pins p ON p.capture_pin_id=r.capture_pin_id
                                WHERE r.evidence_id=e.evidence_id
                                  AND r.released_at IS NULL AND p.released_at IS NULL
                                  AND (p.expires_at IS NULL OR p.expires_at>?2))
             ORDER BY e.retain_until,e.evidence_id
             LIMIT ?3",
        )
        .map_err(map_sqlite)?;
    let rows = statement
        .query_map(
            params![
                super::cas::CURATOR_CAPTURE_RETENTION_CLASS,
                now,
                i64::try_from(MAX_EXPIRED_CAPTURES_PER_CALL).unwrap_or(i64::MAX)
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(map_sqlite)?
        .collect::<rusqlite::Result<_>>()
        .map_err(map_sqlite)?;
    Ok(rows)
}

/// Object ids of the live observations citing `evidence_id`, in creation order.
fn live_observations_citing(
    envelope: &Envelope<'_>,
    evidence_id: &str,
) -> Result<Vec<String>, KernelError> {
    let mut statement = envelope
        .tx
        .prepare_cached(
            "SELECT object_id FROM observations
             WHERE evidence_id=?1 AND invalidated_commit_seq IS NULL
             ORDER BY created_commit_seq,object_id",
        )
        .map_err(map_sqlite)?;
    let rows = statement
        .query_map([evidence_id], |row| row.get(0))
        .map_err(map_sqlite)?
        .collect::<rusqlite::Result<_>>()
        .map_err(map_sqlite)?;
    Ok(rows)
}
