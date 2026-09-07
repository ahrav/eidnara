//! Content-addressed artifact ingestion and capacity accounting.
//!
//! Ingestion validates and redacts input before staging bytes.
//! Publication uses no-replace filesystem operations followed by directory synchronization, then commits the SQLite reference under writer fencing.
//! Failed staging removes its temporary file best-effort: `StagedObject::drop` discards unlink and directory-sync errors, so an orphaned temporary entry can survive.
//! Storage-integrity failures latch CAS ingestion closed.

use std::fs::File;

use context_core::redaction::{Detection, RedactionError, RedactionErrorKind};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use rustix::fs::{self as rfs, AtFlags, OFlags};
use serde::Serialize;
use sha2::{Digest, Sha256};

#[cfg(feature = "test-support")]
use super::ArtifactIngestFault;
use super::{
    ArtifactError, ArtifactErrorKind, ArtifactHandle, ArtifactIngestHook, ArtifactIngestRequest,
    IngestFaults, MAX_PAYLOAD_BYTES, MAX_PAYLOAD_DETECTIONS, MAX_TEXT_FIELD_BYTES, ProviderEgress,
    is_artifact_digest, read_capped,
};
use crate::current_time_ms;
use crate::durable_fs::{
    PublishOutcome, StorageError, classify_errno, classify_io, create_new_file, durable_unlink,
    open_regular_nofollow, publish_noreplace_between_locked, sync_directory,
    sync_publish_directories_with, temp_name, write_and_sync,
};
use crate::envelope::{ObjectRow, PendingChange, check_fence, commit_with_writer};
use crate::object_write::map_write_error;
use crate::redaction::{
    RedactedField, identity, payload_has_secret, record, redact_lossy, redact_payload,
};
use crate::{KernelError, KernelStore, Sensitivity};

const RESERVATION_MS: i64 = 60 * 60 * 1_000;

struct StagedObject<'a> {
    directory: &'a File,
    name: &'a str,
    consumed: bool,
}

impl StagedObject<'_> {
    fn consume(&mut self) {
        self.consumed = true;
    }
}

impl Drop for StagedObject<'_> {
    fn drop(&mut self) {
        if !self.consumed {
            let _ = durable_unlink(self.directory, self.name);
        }
    }
}

struct PreparedArtifact {
    request: ArtifactIngestRequest,
    digest: String,
    bytes: Vec<u8>,
    payload_redaction: RedactedField,
    sensitivity: Sensitivity,
    artifact_reference: String,
    redaction_metadata: Vec<u8>,
}

impl PreparedArtifact {
    fn new(mut request: ArtifactIngestRequest) -> Result<Self, ArtifactError> {
        if request.payload.len() > MAX_PAYLOAD_BYTES {
            return Err(ArtifactError::new(ArtifactErrorKind::PayloadTooLarge));
        }
        if !is_artifact_digest(&request.intent.request_digest)
            || request.intent.refuse_reserved_producer().is_err()
        {
            return Err(ArtifactError::new(ArtifactErrorKind::InvalidInput));
        }
        let (repository_id, revision) =
            request.provenance.as_ref().map_or(("", ""), |provenance| {
                (
                    provenance.repository_id.as_str(),
                    provenance.revision.as_str(),
                )
            });
        if [
            request.evidence_id.as_str(),
            request.object_id.as_str(),
            request.object_kind.as_str(),
            request.domain_id.as_str(),
            request.source_kind.as_str(),
            request.source_id.as_str(),
            request.media_type.as_str(),
            request.retention_class.as_str(),
            request.intent.producer.as_str(),
            request.intent.operation_key.as_str(),
            request.intent.actor.as_str(),
            request.intent.cause.as_str(),
            repository_id,
            revision,
        ]
        .into_iter()
        .any(|field| field.len() > MAX_TEXT_FIELD_BYTES)
        {
            return Err(ArtifactError::new(ArtifactErrorKind::TextFieldTooLong));
        }
        if [
            request.evidence_id.as_str(),
            request.object_id.as_str(),
            request.object_kind.as_str(),
            request.domain_id.as_str(),
            request.source_kind.as_str(),
            request.source_id.as_str(),
            repository_id,
            revision,
        ]
        .into_iter()
        .any(|field| identity(field).is_err())
        {
            return Err(ArtifactError::new(ArtifactErrorKind::InvalidInput));
        }
        if request.evidence_id.trim().is_empty()
            || request.object_id.trim().is_empty()
            || request.object_kind.trim().is_empty()
            || request.domain_id.trim().is_empty()
            || request.source_kind.trim().is_empty()
            || request.source_id.trim().is_empty()
            || request.media_type.trim().is_empty()
            || request.retention_class.trim().is_empty()
            || request.source_revision < 0
            || request.retain_until.is_some_and(|value| value < 0)
        {
            return Err(ArtifactError::new(ArtifactErrorKind::InvalidInput));
        }

        let (payload_redaction, bytes, inspected) = match std::str::from_utf8(&request.payload) {
            Ok(text) => {
                let mut redaction = redact_payload(text, MAX_PAYLOAD_DETECTIONS)
                    .map_err(|error| ArtifactError::new(scan_failure(error)))?;
                // The redacted text becomes the stored bytes without a copy;
                // only its detections are needed afterwards.
                let bytes = std::mem::take(&mut redaction.text).into_bytes();
                (redaction, bytes, true)
            }
            Err(_) => {
                // Lossy decoding expands invalid bytes to three-byte U+FFFD sequences;
                // payload_has_secret scans bounded windows to avoid materializing a lossy-decoded payload.
                match payload_has_secret(&request.payload) {
                    Ok(false) => {}
                    Ok(true) => {
                        return Err(ArtifactError::new(ArtifactErrorKind::UnredactableSecret));
                    }
                    Err(error) => return Err(ArtifactError::new(scan_failure(error))),
                }
                (
                    RedactedField {
                        text: String::new(),
                        detections: Vec::new(),
                    },
                    std::mem::take(&mut request.payload),
                    false,
                )
            }
        };
        let affirmative_provenance = request.provenance.as_ref().is_some_and(|provenance| {
            !provenance.repository_id.trim().is_empty() && !provenance.revision.trim().is_empty()
        });
        // A placeholder can be wider than the secret it replaces, so redaction can push the payload past the cap.
        if bytes.len() > MAX_PAYLOAD_BYTES {
            return Err(ArtifactError::new(ArtifactErrorKind::PayloadTooLarge));
        }
        // A recognized secret anywhere that is stored verbatim-after-redaction must
        // raise the class, not only one in the payload; otherwise a clean payload
        // with a leaking media type stays remotely eligible.
        let metadata_detected = [&request.media_type, &request.retention_class]
            .into_iter()
            .any(|field| !redact_lossy(field).detections.is_empty());
        let sensitivity = if !payload_redaction.detections.is_empty() || metadata_detected {
            Sensitivity::Secret
        } else if !inspected {
            request
                .asserted_sensitivity
                .restrictive(Sensitivity::Sensitive)
        } else if request.asserted_sensitivity == Sensitivity::Normal && !affirmative_provenance {
            Sensitivity::Sensitive
        } else {
            request.asserted_sensitivity
        };
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let artifact_reference = format!("objects/{}/{}", &digest[..2], &digest[2..]);
        let redaction_metadata = detection_metadata(&payload_redaction.detections)?;
        // `clear` would keep the allocation; the payload's bytes now live in
        // `bytes` and the original buffer is released.
        request.payload = Vec::new();

        Ok(Self {
            request,
            digest,
            bytes,
            payload_redaction,
            sensitivity,
            artifact_reference,
            redaction_metadata,
        })
    }
}

/// A payload is refused when its scan cannot vouch for it. Only a match the
/// scanner saw but could not describe counts as a secret; every other failure
/// means the scan did not cover the payload, which is reported as such so an
/// operator is not told a secret was found when the scan merely stopped.
fn scan_failure(error: RedactionError) -> ArtifactErrorKind {
    match error.kind() {
        RedactionErrorKind::DetectionLimit => ArtifactErrorKind::DetectionLimit,
        RedactionErrorKind::MatchLimit => ArtifactErrorKind::UnredactableSecret,
        RedactionErrorKind::Construction
        | RedactionErrorKind::InputLimit
        | RedactionErrorKind::CandidateLimit
        | RedactionErrorKind::WorkLimit
        | RedactionErrorKind::InvalidSpan
        | RedactionErrorKind::UnknownRule
        | RedactionErrorKind::SecretDetected => ArtifactErrorKind::ScanIncomplete,
    }
}

impl KernelStore {
    /// Validates, redacts, durably publishes, and references one artifact.
    ///
    /// The returned digest covers stored bytes after text redaction. Replaying the
    /// same commit intent succeeds only when its committed digest matches. Calls
    /// serialize through the store writer while reserving and committing the
    /// reference.
    ///
    /// # Errors
    ///
    /// Returns [`ArtifactError`] for invalid or oversized fields, rejected secret
    /// material, exhausted capacity, conflicting reclamation, commit failure, or
    /// filesystem failure. Integrity-related storage failures may latch later CAS
    /// ingestion closed.
    pub fn ingest_artifact(
        &self,
        request: ArtifactIngestRequest,
    ) -> Result<ArtifactHandle, ArtifactError> {
        self.ingest_artifact_inner(request, IngestFaults::default(), None, None)
    }

    /// Runs artifact ingestion with one injected failure point.
    ///
    /// # Errors
    ///
    /// Returns the normal ingestion error or the error induced by `fault`.
    #[cfg(feature = "test-support")]
    pub fn ingest_artifact_with_fault_for_test(
        &self,
        request: ArtifactIngestRequest,
        fault: ArtifactIngestFault,
    ) -> Result<ArtifactHandle, ArtifactError> {
        self.ingest_artifact_inner(request, fault.into(), None, None)
    }

    /// Runs `hook` after the staged file is synced and before writer acquisition.
    ///
    /// # Errors
    ///
    /// Returns any error from the normal ingestion path. Hook panics propagate.
    #[cfg(feature = "test-support")]
    pub fn ingest_artifact_with_temp_hook_for_test(
        &self,
        request: ArtifactIngestRequest,
        mut hook: impl FnMut(&str),
    ) -> Result<ArtifactHandle, ArtifactError> {
        self.ingest_artifact_inner(request, IngestFaults::default(), Some(&mut hook), None)
    }

    /// Reports durable protocol boundaries while optionally injecting a failure.
    ///
    /// # Errors
    ///
    /// Returns the normal ingestion error or the error induced by `fault`. Hook
    /// panics propagate.
    #[cfg(feature = "test-support")]
    pub fn ingest_artifact_with_protocol_hook_for_test(
        &self,
        request: ArtifactIngestRequest,
        fault: Option<ArtifactIngestFault>,
        mut hook: impl FnMut(ArtifactIngestHook),
    ) -> Result<ArtifactHandle, ArtifactError> {
        let faults = fault.map(IngestFaults::from).unwrap_or_default();
        self.ingest_artifact_inner(request, faults, None, Some(&mut hook))
    }

    fn ingest_artifact_inner(
        &self,
        request: ArtifactIngestRequest,
        faults: IngestFaults,
        temp_written_hook: Option<&mut dyn FnMut(&str)>,
        mut protocol_hook: Option<&mut dyn FnMut(ArtifactIngestHook)>,
    ) -> Result<ArtifactHandle, ArtifactError> {
        if self.cas_is_failed() {
            return Err(ArtifactError::new(ArtifactErrorKind::IngestionFailClosed));
        }
        let prepared = PreparedArtifact::new(request)?;
        let byte_length = u64::try_from(prepared.bytes.len())
            .map_err(|_| ArtifactError::new(ArtifactErrorKind::InvalidInput))?;

        let tmp = &self.tmp_directory;
        let objects = &self.objects_directory;
        let temp_name = temp_name(&format!("artifact-{}", prepared.digest));
        let mut temp = create_new_file(tmp, &temp_name).map_err(|error| {
            self.map_cas_storage_error(error, ArtifactErrorKind::IngestionFailClosed)
        })?;
        let mut staged = StagedObject {
            directory: tmp,
            name: &temp_name,
            consumed: false,
        };
        let write_result = if faults.write {
            Err(injected_storage_error())
        } else if faults.file_sync {
            std::io::Write::write_all(&mut temp, &prepared.bytes)
                .map_err(classify_io)
                .and(Err(injected_storage_error()))
        } else {
            write_and_sync(&mut temp, &prepared.bytes)
        };
        if let Err(error) = write_result {
            return Err(self.map_cas_storage_error(error, ArtifactErrorKind::IngestionFailClosed));
        }
        drop(temp);
        if let Some(hook) = temp_written_hook {
            hook(&temp_name);
        }

        let mut writer = self
            .lock_writer()
            .map_err(|_| ArtifactError::new(ArtifactErrorKind::ReferenceCommit))?;
        self.check_budget(objects, &prepared.digest, byte_length)?;
        let shard = self
            .shard_directory(&prepared.digest, true)
            .map_err(|error| {
                self.map_cas_storage_error(error, ArtifactErrorKind::IngestionFailClosed)
            })?
            .ok_or_else(|| ArtifactError::new(ArtifactErrorKind::IngestionFailClosed))?;
        let now = current_time_ms();
        let reservation_id = format!(
            "{}-{}",
            prepared.digest,
            crate::durable_fs::next_unique_id()
        );

        let reservation = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| ArtifactError::new(ArtifactErrorKind::ReferenceCommit))?;
        check_fence(&reservation, self.lease_epoch())
            .map_err(|_| ArtifactError::new(ArtifactErrorKind::ReferenceCommit))?;
        // A digest under active reclamation must not be re-admitted.
        if artifact_is_reclaiming(&reservation, &prepared.digest)
            .map_err(|_| ArtifactError::new(ArtifactErrorKind::ReferenceCommit))?
        {
            drop(reservation);
            return Err(ArtifactError::for_digest(
                ArtifactErrorKind::ReclaimInProgress,
                &prepared.digest,
            ));
        }
        if artifact_is_blocked(&reservation, &prepared.digest)
            .map_err(|_| ArtifactError::new(ArtifactErrorKind::ReferenceCommit))?
        {
            drop(reservation);
            return Err(ArtifactError::for_digest(
                ArtifactErrorKind::ReAdmissionBlocked,
                &prepared.digest,
            ));
        }
        reservation
            .execute(
                "INSERT INTO artifact_ingestion_reservations(
                     reservation_id,artifact_digest,artifact_reference,state,writer_epoch,
                     created_at,heartbeat_at,lease_expires_at
                 ) VALUES (?1,?2,?3,'Live',?4,?5,?5,?6)",
                params![
                    reservation_id,
                    prepared.digest,
                    prepared.artifact_reference,
                    i64::try_from(self.lease_epoch())
                        .map_err(|_| ArtifactError::new(ArtifactErrorKind::InvalidInput))?,
                    now,
                    now.saturating_add(RESERVATION_MS),
                ],
            )
            .map_err(|_| ArtifactError::new(ArtifactErrorKind::ReferenceCommit))?;
        if faults.reservation_commit {
            drop(reservation);
            return Err(ArtifactError::new(ArtifactErrorKind::ReferenceCommit));
        }
        reservation
            .commit()
            .map_err(|_| ArtifactError::new(ArtifactErrorKind::ReferenceCommit))?;
        if let Some(hook) = protocol_hook.as_mut() {
            hook(ArtifactIngestHook::AfterReservation);
        }

        let publish = if faults.rename {
            Err(injected_storage_error())
        } else {
            publish_noreplace_between_locked(tmp, &temp_name, &shard, &prepared.digest[2..])
        };
        let published_new = match publish {
            Ok(PublishOutcome::Published) => {
                staged.consume();
                true
            }
            // A retained temp link makes the object's link count two, which
            // `verify_object` rejects.
            Ok(PublishOutcome::PublishedTempRetained) => {
                if let Err(error) = durable_unlink(tmp, &temp_name) {
                    let mapped =
                        self.map_cas_storage_error(error, ArtifactErrorKind::IngestionFailClosed);
                    self.cleanup_failed_reference(
                        &mut writer,
                        &reservation_id,
                        &prepared.digest,
                        true,
                    );
                    return Err(mapped);
                }
                staged.consume();
                true
            }
            Ok(PublishOutcome::AlreadyExists) => {
                if let Err(error) = durable_unlink(tmp, &temp_name) {
                    let mapped =
                        self.map_cas_storage_error(error, ArtifactErrorKind::IngestionFailClosed);
                    self.release_reservation(&mut writer, &reservation_id);
                    return Err(mapped);
                }
                staged.consume();
                false
            }
            Err(error) => {
                let mapped =
                    self.map_cas_storage_error(error, ArtifactErrorKind::IngestionFailClosed);
                self.cleanup_failed_reference(&mut writer, &reservation_id, &prepared.digest, true);
                return Err(mapped);
            }
        };

        if let Err(error) = sync_publish_directories_with(tmp, &shard, sync_directory) {
            let mapped = self.map_cas_storage_error(error, ArtifactErrorKind::IngestionFailClosed);
            self.cleanup_failed_reference(
                &mut writer,
                &reservation_id,
                &prepared.digest,
                published_new,
            );
            return Err(mapped);
        }
        if let Some(hook) = protocol_hook.as_mut() {
            hook(ArtifactIngestHook::AfterPublish);
        }
        if let Err(error) = verify_object(&shard, &prepared.digest[2..], &prepared.digest) {
            self.cleanup_failed_reference(
                &mut writer,
                &reservation_id,
                &prepared.digest,
                published_new,
            );
            return Err(error);
        }
        if faults.after_directory_sync {
            self.cleanup_failed_reference(
                &mut writer,
                &reservation_id,
                &prepared.digest,
                published_new,
            );
            self.latch_cas_failure();
            return Err(ArtifactError::new(ArtifactErrorKind::IngestionFailClosed));
        }
        if self.cas_is_failed() {
            self.cleanup_failed_reference(
                &mut writer,
                &reservation_id,
                &prepared.digest,
                published_new,
            );
            return Err(ArtifactError::new(ArtifactErrorKind::IngestionFailClosed));
        }

        // The receipt lookup runs before the operation, so a `Conflict` raised
        // without entering it is a reused `operation_key`, not a constraint.
        let mut entered = false;
        // Names the reason behind a `Conflict` the operation raises itself,
        // since a storage constraint surfaces as the same error.
        let mut refusal = None;
        let commit_result = commit_with_writer(
            &mut writer,
            self.lease_epoch(),
            prepared.request.intent.clone(),
            |envelope| {
                entered = true;
                insert_reference(envelope, &prepared, &reservation_id, &mut refusal)
            },
            || {
                if faults.after_events {
                    Err(KernelError::Fault)
                } else {
                    Ok(())
                }
            },
        );
        match commit_result {
            Ok(receipt) if receipt.replayed => {
                let committed = committed_digest(&mut writer, &receipt.result);
                match committed {
                    Ok(Some(digest)) if digest == prepared.digest => {
                        if let Err(error) =
                            self.merge_replayed_classification(&mut writer, &prepared)
                        {
                            self.cleanup_failed_reference(
                                &mut writer,
                                &reservation_id,
                                &prepared.digest,
                                published_new,
                            );
                            return Err(error);
                        }
                        self.release_reservation(&mut writer, &reservation_id);
                        Ok(ArtifactHandle {
                            digest: prepared.digest,
                            evidence_id: receipt.result,
                        })
                    }
                    Ok(_) => {
                        self.cleanup_failed_reference(
                            &mut writer,
                            &reservation_id,
                            &prepared.digest,
                            published_new,
                        );
                        Err(ArtifactError::new(ArtifactErrorKind::InvalidInput))
                    }
                    Err(error) => {
                        self.cleanup_failed_reference(
                            &mut writer,
                            &reservation_id,
                            &prepared.digest,
                            published_new,
                        );
                        Err(error)
                    }
                }
            }
            Ok(_) => Ok(ArtifactHandle {
                digest: prepared.digest,
                evidence_id: redact_lossy(&prepared.request.evidence_id).text,
            }),
            Err(error) => {
                self.cleanup_failed_reference(
                    &mut writer,
                    &reservation_id,
                    &prepared.digest,
                    published_new,
                );
                Err(ArtifactError::new(match error {
                    KernelError::InvalidInput => ArtifactErrorKind::InvalidInput,
                    KernelError::Conflict if !entered => ArtifactErrorKind::OperationKeyReused,
                    KernelError::Conflict => {
                        refusal.unwrap_or(ArtifactErrorKind::StorageConstraint)
                    }
                    _ => ArtifactErrorKind::ReferenceCommit,
                }))
            }
        }
    }

    fn check_budget(
        &self,
        objects: &File,
        digest: &str,
        byte_length: u64,
    ) -> Result<(), ArtifactError> {
        let usage = regular_file_bytes(objects, &|| false)
            .map_err(|error| {
                self.map_cas_storage_error(error, ArtifactErrorKind::IngestionFailClosed)
            })?
            .expect("an uncancellable walk completes");
        let already_present = object_is_present(objects, digest);
        let projected = usage.saturating_add(if already_present { 0 } else { byte_length });
        if projected > self.artifact_cap {
            return Err(ArtifactError::capacity(usage, self.artifact_cap));
        }
        Ok(())
    }

    fn merge_replayed_classification(
        &self,
        writer: &mut Connection,
        prepared: &PreparedArtifact,
    ) -> Result<(), ArtifactError> {
        let _change = self.begin_classification_change();
        self.merge_replayed_classification_inner(writer, prepared)
    }

    /// A replay that only repeats the stored classification commits nothing. One
    /// that tightens it is a durable fact about the digest, and a change every
    /// consumer of the tightened evidence has to see, so it commits under the
    /// store's own reserved producer, which no caller intent may name, with a key
    /// derived from the replayed intent and the resulting classes. A replayed
    /// receipt under that key can therefore only be one this path wrote, and is
    /// still required to describe this exact tightening.
    fn merge_replayed_classification_inner(
        &self,
        writer: &mut Connection,
        prepared: &PreparedArtifact,
    ) -> Result<(), ArtifactError> {
        let commit_error = || ArtifactError::new(ArtifactErrorKind::ReferenceCommit);
        let merged = {
            let tx = writer
                .transaction_with_behavior(TransactionBehavior::Deferred)
                .map_err(|_| commit_error())?;
            let merged = load_merged_classification(
                &tx,
                &prepared.digest,
                prepared.sensitivity,
                prepared.request.provider_egress,
            )
            .map_err(|_| commit_error())?;
            tx.commit().map_err(|_| commit_error())?;
            merged
        };
        if !merged.stale {
            return Ok(());
        }
        let outcome = merged.outcome(&prepared.digest);
        let intent = prepared.request.intent.derived(
            "classification",
            &format!(
                "classify:{}:{}",
                merged.sensitivity.as_str(),
                merged.egress.as_str()
            ),
        );
        let receipt = commit_with_writer(
            writer,
            self.lease_epoch(),
            intent,
            |envelope| {
                let merged = load_merged_classification(
                    envelope.tx,
                    &prepared.digest,
                    prepared.sensitivity,
                    prepared.request.provider_egress,
                )?;
                let outcome = merged.outcome(&prepared.digest);
                merged.apply(envelope, &prepared.digest)?;
                Ok(outcome)
            },
            || Ok(()),
        )
        .map_err(|_| commit_error())?;
        if receipt.replayed && receipt.result != outcome {
            return Err(commit_error());
        }
        Ok(())
    }

    fn release_reservation(&self, writer: &mut Connection, reservation_id: &str) {
        let Ok(tx) = writer.transaction_with_behavior(TransactionBehavior::Immediate) else {
            return;
        };
        if check_fence(&tx, self.lease_epoch()).is_err() {
            return;
        }
        if tx
            .execute(
                "DELETE FROM artifact_ingestion_reservations WHERE reservation_id=?1",
                [reservation_id],
            )
            .is_ok()
        {
            let _ = tx.commit();
        }
    }

    fn cleanup_failed_reference(
        &self,
        writer: &mut Connection,
        reservation_id: &str,
        digest: &str,
        published_new: bool,
    ) {
        let Ok(tx) = writer.transaction_with_behavior(TransactionBehavior::Immediate) else {
            return;
        };
        if check_fence(&tx, self.lease_epoch()).is_err() {
            return;
        }
        if tx
            .execute(
                "DELETE FROM artifact_ingestion_reservations WHERE reservation_id=?1",
                [reservation_id],
            )
            .is_err()
        {
            return;
        }
        if !published_new {
            let _ = tx.commit();
            return;
        }
        let protected: i64 = match tx.query_row(
            "SELECT
                 (SELECT COUNT(*) FROM evidence_meta WHERE artifact_digest=?1) +
                 (SELECT COUNT(*) FROM artifact_ingestion_reservations WHERE artifact_digest=?1)",
            [digest],
            |row| row.get(0),
        ) {
            Ok(value) => value,
            Err(_) => return,
        };
        if tx.commit().is_err() || protected != 0 {
            return;
        }
        let Ok(Some(shard)) = self.shard_directory(digest, false) else {
            // No shard means no object to remove; any other failure to reach it
            // is a failed reference cleanup like the unlink below.
            if !matches!(self.shard_directory(digest, false), Ok(None)) {
                self.latch_cas_failure();
            }
            return;
        };
        if durable_unlink(&shard, &digest[2..]).is_err() {
            self.latch_cas_failure();
        }
    }
}

fn insert_reference(
    envelope: &mut crate::Envelope<'_>,
    prepared: &PreparedArtifact,
    reservation_id: &str,
    refusal: &mut Option<ArtifactErrorKind>,
) -> Result<String, KernelError> {
    if artifact_is_blocked(envelope.tx, &prepared.digest)? {
        *refusal = Some(ArtifactErrorKind::ReAdmissionBlocked);
        return Err(KernelError::Conflict);
    }
    let reservation_state: Option<String> = envelope
        .tx
        .query_row(
            "SELECT state FROM artifact_ingestion_reservations WHERE reservation_id=?1 AND artifact_digest=?2",
            params![reservation_id, prepared.digest],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| KernelError::Io)?;
    if reservation_state.as_deref() != Some("Live") {
        *refusal = Some(ArtifactErrorKind::ReferenceCommit);
        return Err(KernelError::Conflict);
    }
    let reclaiming: i64 = envelope
        .tx
        .query_row(
            "SELECT COUNT(*) FROM artifact_ingestion_reservations WHERE artifact_digest=?1 AND state='Reclaiming'",
            [&prepared.digest],
            |row| row.get(0),
        )
        .map_err(|_| KernelError::Io)?;
    if reclaiming != 0 {
        *refusal = Some(ArtifactErrorKind::ReclaimInProgress);
        return Err(KernelError::Conflict);
    }

    let merged = load_merged_classification(
        envelope.tx,
        &prepared.digest,
        prepared.sensitivity,
        prepared.request.provider_egress,
    )?;
    let (sensitivity, egress) = (merged.sensitivity, merged.egress);
    merged.apply(envelope, &prepared.digest)?;

    // Identity columns must survive round-trip, so a detected secret is refused
    // rather than replaced: two ids differing only inside a redacted span would
    // otherwise collapse onto one placeholder-backed identity.
    let evidence_id = identity(&prepared.request.evidence_id)?;
    let object_id = identity(&prepared.request.object_id)?;
    let object_kind = identity(&prepared.request.object_kind)?;
    let domain_id = identity(&prepared.request.domain_id)?;
    let source_kind = identity(&prepared.request.source_kind)?;
    let source_id = identity(&prepared.request.source_id)?;
    let media_type = redact_lossy(&prepared.request.media_type);
    let retention_class = redact_lossy(&prepared.request.retention_class);
    envelope
        .tx
        .execute(
            "INSERT INTO object_registry(
                 object_id,object_kind,domain_id,source_kind,source_id,source_revision,
                 created_commit_seq,sensitivity_class
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                object_id,
                object_kind,
                domain_id,
                source_kind,
                source_id,
                prepared.request.source_revision,
                envelope.commit_seq,
                sensitivity.as_str(),
            ],
        )
        .map_err(map_write_error)?;
    let first_detection = prepared.payload_redaction.detections.first();
    envelope
        .tx
        .execute(
            "INSERT INTO evidence_meta(
                 evidence_id,object_id,artifact_reference,artifact_digest,byte_length,media_type,
                 retention_class,retain_until,detector_kind,detector_version,detector_metadata,
                 detector_id,secret_type,utf8_offset,utf8_length,provider_egress_class,
                 redaction_metadata,created_commit_seq,sensitivity_class
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
            params![
                evidence_id,
                object_id,
                prepared.artifact_reference,
                prepared.digest,
                i64::try_from(prepared.bytes.len()).map_err(|_| KernelError::InvalidInput)?,
                media_type.text,
                retention_class.text,
                prepared.request.retain_until,
                first_detection.map(|_| "secret_redaction"),
                None::<&str>,
                first_detection.map(|_| prepared.redaction_metadata.as_slice()),
                first_detection.map(|detection| detection.detector_id),
                first_detection.map(|detection| detection.secret_type.as_str()),
                first_detection
                    .map(|detection| i64::try_from(detection.offset))
                    .transpose()
                    .map_err(|_| KernelError::InvalidInput)?,
                first_detection
                    .map(|detection| i64::try_from(detection.length))
                    .transpose()
                    .map_err(|_| KernelError::InvalidInput)?,
                egress.as_str(),
                prepared.redaction_metadata,
                envelope.commit_seq,
                sensitivity.as_str(),
            ],
        )
        .map_err(map_write_error)?;
    record(
        envelope.tx,
        "evidence",
        &evidence_id,
        "payload",
        &prepared.payload_redaction,
        Some(envelope.commit_seq),
    )?;
    for (name, field) in [
        ("media_type", &media_type),
        ("retention_class", &retention_class),
    ] {
        record(
            envelope.tx,
            "evidence",
            &evidence_id,
            name,
            field,
            Some(envelope.commit_seq),
        )?;
    }
    if envelope
        .tx
        .execute(
            "DELETE FROM artifact_ingestion_reservations WHERE reservation_id=?1 AND state='Live'",
            [reservation_id],
        )
        .map_err(|_| KernelError::Io)?
        != 1
    {
        return Err(KernelError::Conflict);
    }
    envelope.changes.push(PendingChange {
        object: ObjectRow {
            object_id: object_id.clone(),
            object_kind: object_kind.clone(),
            domain_id: domain_id.clone(),
            source_kind: source_kind.clone(),
            source_id: source_id.clone(),
            source_revision: prepared.request.source_revision,
            created_commit_seq: envelope.commit_seq,
            invalidated_commit_seq: None,
            superseded_by: None,
            sensitivity,
        },
        kind: "insert",
        replaced_object_id: None,
        redactions: Vec::new(),
        audit: None,
    });
    Ok(evidence_id)
}

/// The classification one digest's evidence rows share once a new assertion is
/// folded in, and the rows that assertion tightens.
struct MergedClassification {
    sensitivity: Sensitivity,
    egress: ProviderEgress,
    /// Some stored row, live or invalidated, is looser than the merged class.
    stale: bool,
    /// Live evidence objects whose stored class is looser than the merged one.
    tightened: Vec<ObjectRow>,
}

impl MergedClassification {
    /// The receipt result a classification commit records: the digest and the
    /// classes it left behind, so a replayed receipt can be checked against the
    /// tightening it is taken to stand for.
    fn outcome(&self, digest: &str) -> String {
        format!(
            "classify:{digest}:{}:{}",
            self.sensitivity.as_str(),
            self.egress.as_str()
        )
    }

    /// Writes the merged class onto every row for `digest`, invalidated rows
    /// included so the digest-level class survives when no live reference
    /// remains, and records a `classify` change for each live row it tightened,
    /// so the tightening reaches the change log and outbox the way the original
    /// insert did.
    fn apply(self, envelope: &mut crate::Envelope<'_>, digest: &str) -> Result<(), KernelError> {
        envelope
            .tx
            .execute(
                "UPDATE evidence_meta SET sensitivity_class=?1,provider_egress_class=?2
                 WHERE artifact_digest=?3",
                params![self.sensitivity.as_str(), self.egress.as_str(), digest],
            )
            .map_err(|_| KernelError::Io)?;
        for mut object in self.tightened {
            object.sensitivity = self.sensitivity;
            envelope.changes.push(PendingChange {
                object,
                kind: "classify",
                replaced_object_id: None,
                redactions: Vec::new(),
                audit: Some(serde_json::json!({
                    "artifact_digest": digest,
                    "sensitivity": self.sensitivity.as_str(),
                    "provider_egress": self.egress.as_str(),
                })),
            });
        }
        Ok(())
    }
}

/// Folds `sensitivity` and `egress` with every stored class for `digest`; a class
/// only ever tightens. Invalidated rows contribute their class and receive the
/// merged one, so a label asserted while no reference is live still governs the
/// next reference to the same bytes; only live rows are reported as tightened.
fn load_merged_classification(
    tx: &rusqlite::Transaction<'_>,
    digest: &str,
    mut sensitivity: Sensitivity,
    mut egress: ProviderEgress,
) -> Result<MergedClassification, KernelError> {
    let mut statement = tx
        .prepare(
            "SELECT e.sensitivity_class,e.provider_egress_class,e.invalidated_commit_seq,
                    o.object_id,o.object_kind,o.domain_id,o.source_kind,o.source_id,
                    o.source_revision,o.created_commit_seq,o.superseded_by
             FROM evidence_meta e
             JOIN object_registry o ON o.object_id=e.object_id
             WHERE e.artifact_digest=?1
             ORDER BY o.object_id",
        )
        .map_err(|_| KernelError::Io)?;
    let rows = statement
        .query_map([digest], |row| {
            let stored_sensitivity: String = row.get(0)?;
            let stored_egress: String = row.get(1)?;
            let invalidated: Option<i64> = row.get(2)?;
            let object = ObjectRow {
                object_id: row.get(3)?,
                object_kind: row.get(4)?,
                domain_id: row.get(5)?,
                source_kind: row.get(6)?,
                source_id: row.get(7)?,
                source_revision: row.get(8)?,
                created_commit_seq: row.get(9)?,
                invalidated_commit_seq: invalidated,
                superseded_by: row.get(10)?,
                sensitivity: Sensitivity::from_stored(&stored_sensitivity),
            };
            Ok((ProviderEgress::from_stored(&stored_egress), object))
        })
        .map_err(|_| KernelError::Io)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| KernelError::Io)?;
    for (stored_egress, object) in &rows {
        sensitivity = sensitivity.restrictive(object.sensitivity);
        egress = egress.restrictive(*stored_egress);
    }
    let stale = rows.iter().any(|(stored_egress, object)| {
        object.sensitivity != sensitivity || *stored_egress != egress
    });
    let tightened = rows
        .into_iter()
        .filter(|(stored_egress, object)| {
            object.invalidated_commit_seq.is_none()
                && (object.sensitivity != sensitivity || *stored_egress != egress)
        })
        .map(|(_, object)| object)
        .collect();
    Ok(MergedClassification {
        sensitivity,
        egress,
        stale,
        tightened,
    })
}

fn committed_digest(
    writer: &mut Connection,
    evidence_id: &str,
) -> Result<Option<String>, ArtifactError> {
    writer
        .query_row(
            "SELECT artifact_digest FROM evidence_meta WHERE evidence_id=?1",
            [evidence_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| ArtifactError::new(ArtifactErrorKind::ReferenceCommit))
}

fn artifact_is_blocked(
    connection: &rusqlite::Transaction<'_>,
    digest: &str,
) -> Result<bool, KernelError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM artifact_purge_tombstones WHERE artifact_digest=?1)
                    OR EXISTS(SELECT 1 FROM artifact_pending_unlinks WHERE artifact_digest=?1)",
            [digest],
            |row| row.get(0),
        )
        .map_err(|_| KernelError::Io)
}

fn artifact_is_reclaiming(
    connection: &rusqlite::Transaction<'_>,
    digest: &str,
) -> Result<bool, KernelError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM artifact_ingestion_reservations
                           WHERE artifact_digest=?1 AND state='Reclaiming')",
            [digest],
            |row| row.get(0),
        )
        .map_err(|_| KernelError::Io)
}

fn verify_object(shard: &File, name: &str, digest: &str) -> Result<(), ArtifactError> {
    let object = open_regular_nofollow(shard, name)
        .map_err(|_| ArtifactError::for_digest(ArtifactErrorKind::MissingObject, digest))?;
    let Some(bytes) = read_capped(object)
        .map_err(|_| ArtifactError::for_digest(ArtifactErrorKind::MissingObject, digest))?
    else {
        return Err(ArtifactError::for_digest(
            ArtifactErrorKind::CorruptObject,
            digest,
        ));
    };
    if format!("{:x}", Sha256::digest(bytes)) != digest {
        return Err(ArtifactError::for_digest(
            ArtifactErrorKind::CorruptObject,
            digest,
        ));
    }
    Ok(())
}

fn stat_bytes(stat: &rfs::Stat) -> u64 {
    u64::try_from(stat.st_size).unwrap_or(0)
}

pub(super) fn is_dot_entry(name: &std::ffi::CStr) -> bool {
    matches!(name.to_bytes(), b"." | b"..")
}

pub(super) fn open_shard_nofollow(
    objects: &File,
    name: impl rustix::path::Arg,
) -> Result<File, StorageError> {
    rfs::openat(
        objects,
        name,
        OFlags::DIRECTORY | OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        rfs::Mode::empty(),
    )
    .map(File::from)
    .map_err(classify_errno)
}

/// Sums regular-file sizes directly below `objects` and its shard directories.
///
/// Symlinks and other file types do not contribute. Entries removed during the
/// walk are skipped. The sum saturates at [`u64::MAX`]. `cancelled` is polled
/// before every entry at both levels; a true result returns `None`.
pub(super) fn regular_file_bytes(
    objects: &File,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<u64>, StorageError> {
    let mut bytes = 0_u64;
    for entry in rfs::Dir::read_from(objects).map_err(classify_errno)? {
        if cancelled() {
            return Ok(None);
        }
        let entry = entry.map_err(classify_errno)?;
        let name = entry.file_name();
        if is_dot_entry(name) {
            continue;
        }
        // Reclamation can unlink an entry between the directory read and this stat.
        let stat = match rfs::statat(objects, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => stat,
            Err(rustix::io::Errno::NOENT) => continue,
            Err(error) => return Err(classify_errno(error)),
        };
        let kind = rfs::FileType::from_raw_mode(stat.st_mode);
        if kind.is_file() {
            bytes = bytes.saturating_add(stat_bytes(&stat));
            continue;
        }
        if !kind.is_dir() {
            continue;
        }
        let shard = match open_shard_nofollow(objects, name) {
            Ok(shard) => shard,
            Err(StorageError::Other(source)) if source.kind() == std::io::ErrorKind::NotFound => {
                continue;
            }
            Err(error) => return Err(error),
        };
        for shard_entry in rfs::Dir::read_from(&shard).map_err(classify_errno)? {
            if cancelled() {
                return Ok(None);
            }
            let shard_entry = shard_entry.map_err(classify_errno)?;
            let shard_name = shard_entry.file_name();
            if is_dot_entry(shard_name) {
                continue;
            }
            let shard_stat = match rfs::statat(&shard, shard_name, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(shard_stat) => shard_stat,
                Err(rustix::io::Errno::NOENT) => continue,
                Err(error) => return Err(classify_errno(error)),
            };
            if rfs::FileType::from_raw_mode(shard_stat.st_mode).is_file() {
                bytes = bytes.saturating_add(stat_bytes(&shard_stat));
            }
        }
    }
    Ok(Some(bytes))
}

fn object_is_present(objects: &File, digest: &str) -> bool {
    let Ok(shard) = open_shard_nofollow(objects, &digest[..2]) else {
        return false;
    };
    rfs::statat(&shard, &digest[2..], AtFlags::SYMLINK_NOFOLLOW)
        .is_ok_and(|stat| rfs::FileType::from_raw_mode(stat.st_mode).is_file())
}

fn detection_metadata(detections: &[Detection]) -> Result<Vec<u8>, ArtifactError> {
    #[derive(Serialize)]
    struct Metadata<'a> {
        detector_id: &'a str,
        secret_type: &'a str,
        utf8_offset: usize,
        utf8_length: usize,
    }
    serde_json::to_vec(
        &detections
            .iter()
            .map(|detection| Metadata {
                detector_id: detection.detector_id,
                secret_type: &detection.secret_type,
                utf8_offset: detection.offset,
                utf8_length: detection.length,
            })
            .collect::<Vec<_>>(),
    )
    .map_err(|_| ArtifactError::new(ArtifactErrorKind::InvalidInput))
}

#[cfg(feature = "test-support")]
fn injected_storage_error() -> StorageError {
    classify_io(std::io::Error::from_raw_os_error(
        rustix::io::Errno::IO.raw_os_error(),
    ))
}

#[cfg(not(feature = "test-support"))]
fn injected_storage_error() -> StorageError {
    unreachable!("fault injection requires the test-support feature")
}
