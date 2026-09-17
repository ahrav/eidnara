//! Local-file captures record project text read by a Curator run as evidence and a typed observation.
//!
//! The evidence row carries the bytes' identity and the finite acquisition reference (`retain_until`); the observation carries the typed detail: the trusted project, the relative path, the capture time, the whole-buffer digest, and the captured range. The detail is a versioned JSON document in `ObservationPayload.detail`, not a descriptor class, so the frozen `OccurrenceClass` set and every descriptor reader are untouched. Generic observation writers cannot use this kind or these id prefixes, and the writer here accepts a detail only when it agrees with the live Curator-capture evidence row it cites. Expiry retires the observation and then the evidence; the artifact bytes stay for as long as any other live reference names their digest.

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::Digest;

use super::redaction::identity;
use super::slice::{EVIDENCE_CITED_SQL, ObservationPayload, ObservationSpec};
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
    /// Retires the capture observation and evidence of every Curator capture whose `retain_until` has passed and that no live hold still pins, at most [`MAX_EXPIRED_CAPTURES_PER_CALL`] per call, one commit per capture keyed by `now` and the evidence id. A capture that some other live row still cites keeps its evidence: its own observation is retired, and the capture leaves the sweep until that citation is gone, so a retained capture costs nothing on later calls and never displaces a newer expired one from the page. Ownership ends here; the artifact bytes are reclaimed later only if no other live reference names the digest. Returns how many evidence rows were retired.
    ///
    /// A capture whose retirement the store refuses (`NotFound`, `Conflict`, or `InvalidInput` from its own commit) is skipped and the sweep continues, so one such row cannot hold every capture behind it in the page.
    ///
    /// # Errors
    ///
    /// Returns the first storage, lock, or fence error; captures retired before it stay retired.
    pub fn expire_local_file_captures(&self, now: i64) -> Result<usize, KernelError> {
        let expired = {
            let reader = self.lock_reader()?;
            expired_captures(&reader, now)?
        };
        let mut retired = 0;
        for (evidence_id, evidence_object) in expired {
            let intent = CommitIntent {
                producer: EXPIRY_PRODUCER.to_string(),
                operation_key: format!("{now}:{evidence_id}"),
                request_digest: format!("{:x}", sha2::Sha256::digest(evidence_id.as_bytes())),
                actor: EXPIRY_PRODUCER.to_string(),
                cause: "acquisition reference expired".to_string(),
            };
            let outcome = self.commit(intent, |envelope| {
                for observation in live_capture_observations(envelope, &evidence_id)? {
                    envelope.retire_observation(&observation)?;
                }
                if cited_elsewhere(envelope, &evidence_id)? {
                    return Ok("retained".to_string());
                }
                envelope.retire_evidence(&evidence_object)?;
                Ok("retired".to_string())
            });
            match outcome {
                Ok(receipt) => {
                    if receipt.result == "retired" {
                        retired += 1;
                    }
                }
                Err(KernelError::NotFound | KernelError::Conflict | KernelError::InvalidInput) => {}
                Err(error) => return Err(error),
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

/// Live Curator captures whose acquisition reference has passed, that no live hold pins, and that the sweep still has work for: `(evidence_id, evidence object id)`, oldest expiry first. A capture whose observation is already retired and whose evidence another live row cites has nothing left to retire until that citation goes, so it is not a candidate.
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
                super::cas::CURATOR_CAPTURE_RETENTION_CLASS,
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
         WHERE e.retention_class=?1 AND e.invalidated_commit_seq IS NULL
           AND e.retain_until IS NOT NULL AND e.retain_until<=?2
           AND NOT EXISTS(SELECT 1 FROM capture_pin_refs r
                            JOIN capture_pins p ON p.capture_pin_id=r.capture_pin_id
                            WHERE r.evidence_id=e.evidence_id
                              AND r.released_at IS NULL AND p.released_at IS NULL
                              AND (p.expires_at IS NULL OR p.expires_at>?2))
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

    use super::{LOCAL_FILE_KIND, MAX_EXPIRED_CAPTURES_PER_CALL, expired_captures_sql};
    use crate::cas::CURATOR_CAPTURE_RETENTION_CLASS;
    use crate::schema::apply_kernel_schema;

    const NOW: i64 = 10_000;

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
                CURATOR_CAPTURE_RETENTION_CLASS,
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

    /// SQLite VM steps the candidate query spends finding the one live expired capture.
    fn expired_captures_steps(history: usize) -> i32 {
        let conn = store_with_retired_captures(history);
        let mut statement = conn.prepare(&expired_captures_sql()).unwrap();
        let found: Vec<String> = statement
            .query_map(
                params![
                    CURATOR_CAPTURE_RETENTION_CLASS,
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
