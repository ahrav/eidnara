//! A source hold pins the exact-retained evidence every live source descriptor
//! cites at one canonical sequence S, for one registered consumer, in one
//! store incarnation, under one source policy, until a finite expiry. The
//! capture runs under the kernel writer, so S and the protection of its bytes
//! are one step and nothing can be reclaimed between them. Admission runs from
//! counts before a reference is written, and an over-bound corpus is refused
//! whole rather than narrowed.

use std::num::{NonZeroU64, NonZeroUsize};
use std::time::Instant;

use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

use super::backup::release_capture_pin_in_tx;
use super::cas::{ArtifactErrorKind, ObjectPresence, is_artifact_digest};
use super::envelope::check_fence;
use super::source_descriptor::SOURCE_DESCRIPTOR_KIND;
use super::{CachedSql, KernelError, KernelStore, current_time_ms, map_sqlite};

const SOURCE_HOLD_KIND: &str = "source_hold";

/// Admission bounds each capture; this limit bounds the evidence that repeated
/// captures by one consumer can pin together.
pub const MAX_ACTIVE_SOURCE_HOLDS_PER_CONSUMER: usize = 32;

/// Maximum duration of a source hold. An unreleased hold blocks artifact
/// reclamation until expiry or explicit release, so a longer deadline is
/// refused rather than pinning the corpus for years.
pub const MAX_SOURCE_HOLD_LIFETIME_MS: u64 = 30 * 24 * 60 * 60 * 1_000;

/// Separates consumer and policy inside `capture_pins.owner_id`. Both parts
/// are restricted to token bytes, so the separator can never appear in either.
const OWNER_SEPARATOR: char = '\u{1f}';

/// Who is capturing and under which contracts. Every later use of the hold
/// must present the same binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceHoldBinding {
    pub consumer_id: String,
    pub lease_epoch: u64,
    /// The frozen source-policy version the descriptors were published under.
    pub source_policy_version: String,
}

/// Admission bounds for one capture, checked from `COUNT` and `SUM` before
/// any reference row exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceHoldBounds {
    pub max_references: NonZeroUsize,
    pub max_encoded_bytes: NonZeroU64,
    /// Finite lifetime of the hold from capture, in the store's millisecond clock.
    pub expiry_ms: NonZeroU64,
}

/// A captured hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceHold {
    pub hold_id: String,
    pub binding: SourceHoldBinding,
    /// The fixed canonical sequence every held descriptor is read at.
    pub snapshot: i64,
    pub captured_at: i64,
    pub expires_at: i64,
    /// Distinct evidence rows the hold protects.
    pub references: usize,
    /// Their stored byte lengths summed.
    pub encoded_bytes: u64,
}

/// Why a hold no longer protects its bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceHoldInvalidity {
    /// No pin row carries this id under this binding.
    Missing,
    Released,
    Expired,
    /// A purge deleted evidence the hold referenced; the pin is degraded.
    PurgeDegraded,
    /// A referenced evidence row is gone, or its object file is absent or corrupt.
    MissingBytes,
}

/// One held descriptor at S, keyed for stable `(class, object_id, revision)` pages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldDescriptor {
    pub class: String,
    pub object_id: String,
    pub revision: i64,
    pub evidence_id: String,
    pub artifact_digest: String,
    pub byte_length: u64,
    /// The commit after S that invalidated this descriptor, when one has;
    /// the hold still protects its bytes.
    pub invalidated_after_snapshot: Option<i64>,
}

/// Keyset cursor into the held inventory: the last key of the previous page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldCursor {
    pub class: String,
    pub object_id: String,
    pub revision: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldPage {
    pub descriptors: Vec<HeldDescriptor>,
    /// `None` when this page ends the inventory.
    pub next: Option<HeldCursor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SourceHoldError {
    #[error("source hold request is malformed")]
    InvalidRequest,
    #[error("source hold names a consumer that is not registered")]
    UnknownConsumer,
    #[error("source hold was captured in another store incarnation")]
    IncarnationMismatch,
    #[error(
        "source hold at S would reference {references} evidence rows totalling {encoded_bytes} bytes, over the admitted bound"
    )]
    Unadmitted {
        references: usize,
        encoded_bytes: u64,
    },
    #[error(
        "source hold consumer already has {MAX_ACTIVE_SOURCE_HOLDS_PER_CONSUMER} unreleased holds"
    )]
    HoldLimitReached,
    #[error("source hold binding does not match the stored hold")]
    BindingMismatch,
    #[error("source hold is not valid: {0:?}")]
    Invalid(SourceHoldInvalidity),
    #[error(transparent)]
    Kernel(#[from] KernelError),
}

fn sqlite(error: rusqlite::Error) -> SourceHoldError {
    SourceHoldError::Kernel(map_sqlite(error))
}

fn corrupt<T>(_: T) -> SourceHoldError {
    KernelError::CorruptCanonicalRow.into()
}

fn owner_prefix(consumer_id: &str) -> String {
    format!("{consumer_id}{OWNER_SEPARATOR}")
}

fn owner_id(binding: &SourceHoldBinding) -> String {
    format!(
        "{}{}",
        owner_prefix(&binding.consumer_id),
        binding.source_policy_version
    )
}

/// Evidence cited by descriptors live at `?1`: the descriptor was created at
/// or before it, and neither the descriptor nor the evidence it cites was
/// invalidated at or before it. A logical delete or a purge invalidates the
/// evidence row, so a descriptor over deleted bytes is not a candidate rather
/// than a held reference that can never be read. Shared by the count, the
/// insert, and the page so they cannot disagree.
fn live_descriptors_sql() -> String {
    // The descriptor-only index and join order let keyset pages stop at LIMIT without sorting.
    format!(
        "FROM object_registry o INDEXED BY idx_objects_source_descriptor_page
         CROSS JOIN observations b ON b.object_id=o.object_id
         CROSS JOIN evidence_meta e ON e.evidence_id=b.evidence_id
         WHERE o.object_id GLOB 'srcdesc:*'
           AND b.observation_kind='{SOURCE_DESCRIPTOR_KIND}'
           AND b.created_commit_seq<=?1
           AND (b.invalidated_commit_seq IS NULL OR b.invalidated_commit_seq>?1)
           AND (e.invalidated_commit_seq IS NULL OR e.invalidated_commit_seq>?1)"
    )
}

fn is_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn held_descriptors_sql() -> String {
    format!(
        "SELECT o.source_kind,o.object_id,o.source_revision,e.evidence_id,
                e.artifact_digest,e.byte_length,b.invalidated_commit_seq
         {}
           AND (o.source_kind,o.object_id,o.source_revision)>(?2,?3,?4)
         ORDER BY o.source_kind,o.object_id,o.source_revision
         LIMIT ?5",
        live_descriptors_sql()
    )
}

impl KernelStore {
    /// Under the writer: check the binding, take S as the current tip, count
    /// the evidence live descriptors cite at S, refuse if over bound, then
    /// insert the pin and its references with one bounded `INSERT ... SELECT`.
    pub fn capture_source_hold(
        &self,
        binding: &SourceHoldBinding,
        bounds: SourceHoldBounds,
    ) -> Result<SourceHold, SourceHoldError> {
        self.capture_source_hold_inner(binding, bounds, None)
    }

    /// [`Self::capture_source_hold`] with `at_admission` run inside the writer
    /// transaction after the admission decision and before any reference is
    /// written, receiving the number of `capture_pin_refs` rows visible there.
    #[cfg(feature = "test-support")]
    pub fn capture_source_hold_with_hook_for_test(
        &self,
        binding: &SourceHoldBinding,
        bounds: SourceHoldBounds,
        mut at_admission: impl FnMut(i64),
    ) -> Result<SourceHold, SourceHoldError> {
        self.capture_source_hold_inner(binding, bounds, Some(&mut at_admission))
    }

    fn capture_source_hold_inner(
        &self,
        binding: &SourceHoldBinding,
        bounds: SourceHoldBounds,
        at_admission: Option<&mut dyn FnMut(i64)>,
    ) -> Result<SourceHold, SourceHoldError> {
        if !is_token(&binding.consumer_id) || !is_token(&binding.source_policy_version) {
            return Err(SourceHoldError::InvalidRequest);
        }
        if bounds.expiry_ms.get() > MAX_SOURCE_HOLD_LIFETIME_MS {
            return Err(SourceHoldError::InvalidRequest);
        }
        let expiry =
            i64::try_from(bounds.expiry_ms.get()).map_err(|_| SourceHoldError::InvalidRequest)?;
        if binding.lease_epoch != self.lease_epoch() {
            return Err(SourceHoldError::IncarnationMismatch);
        }
        let mut writer = self.lock_writer()?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite)?;
        check_fence(&tx, self.lease_epoch())?;
        let registered: bool = tx
            .query_row_cached(
                "SELECT EXISTS(SELECT 1 FROM outbox_consumers WHERE consumer_id=?1)",
                [binding.consumer_id.as_str()],
                |row| row.get(0),
            )
            .map_err(sqlite)?;
        if !registered {
            return Err(SourceHoldError::UnknownConsumer);
        }
        let active: i64 = tx
            .query_row_cached(
                "SELECT COUNT(*) FROM capture_pins
                 WHERE pin_kind=?1 AND substr(owner_id,1,length(?2))=?2
                   AND released_at IS NULL",
                params![SOURCE_HOLD_KIND, owner_prefix(&binding.consumer_id)],
                |row| row.get(0),
            )
            .map_err(sqlite)?;
        if usize::try_from(active).map_err(corrupt)? >= MAX_ACTIVE_SOURCE_HOLDS_PER_CONSUMER {
            return Err(SourceHoldError::HoldLimitReached);
        }
        let snapshot: i64 = tx
            .query_row_cached(
                "SELECT COALESCE(MAX(commit_seq),0) FROM commit_log",
                [],
                |row| row.get(0),
            )
            .map_err(sqlite)?;
        let (references, encoded_bytes): (i64, Option<i64>) = tx
            .query_row_cached(
                &format!(
                    "SELECT COUNT(*),SUM(byte_length) FROM (
                         SELECT e.evidence_id,e.byte_length {}
                         GROUP BY e.evidence_id
                     )",
                    live_descriptors_sql()
                ),
                [snapshot],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(sqlite)?;
        let references = usize::try_from(references).map_err(corrupt)?;
        let encoded_bytes = u64::try_from(encoded_bytes.unwrap_or(0)).map_err(corrupt)?;
        if let Some(hook) = at_admission {
            let refs_in_tx: i64 = tx
                .query_row("SELECT COUNT(*) FROM capture_pin_refs", [], |row| {
                    row.get(0)
                })
                .map_err(sqlite)?;
            hook(refs_in_tx);
        }
        if references > bounds.max_references.get()
            || encoded_bytes > bounds.max_encoded_bytes.get()
        {
            return Err(SourceHoldError::Unadmitted {
                references,
                encoded_bytes,
            });
        }
        let captured_at = current_time_ms();
        let expires_at = captured_at
            .checked_add(expiry)
            .ok_or(SourceHoldError::InvalidRequest)?;
        let hold_id: String = tx
            .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
            .map_err(sqlite)?;
        let epoch = i64::try_from(self.lease_epoch()).map_err(|_| KernelError::InvalidInput)?;
        tx.execute(
            "INSERT INTO capture_pins(
                 capture_pin_id,pin_kind,owner_id,commit_seq,lease_epoch,writer_epoch,
                 created_at,expires_at
             ) VALUES (?1,?2,?3,?4,?5,?5,?6,?7)",
            params![
                hold_id,
                SOURCE_HOLD_KIND,
                owner_id(binding),
                snapshot,
                epoch,
                captured_at,
                expires_at,
            ],
        )
        .map_err(sqlite)?;
        let inserted = tx
            .execute_cached(
                &format!(
                    "INSERT INTO capture_pin_refs(capture_pin_id,evidence_id,expires_at)
                     SELECT ?2,evidence_id,?3 FROM (
                         SELECT e.evidence_id {}
                         GROUP BY e.evidence_id
                     ) LIMIT ?4",
                    live_descriptors_sql()
                ),
                params![
                    snapshot,
                    hold_id,
                    expires_at,
                    i64::try_from(bounds.max_references.get()).unwrap_or(i64::MAX),
                ],
            )
            .map_err(sqlite)?;
        if inserted != references {
            return Err(KernelError::CorruptCanonicalRow.into());
        }
        tx.commit().map_err(sqlite)?;
        Ok(SourceHold {
            hold_id,
            binding: binding.clone(),
            snapshot,
            captured_at,
            expires_at,
            references,
            encoded_bytes,
        })
    }

    /// Elapsed monotonic milliseconds advance `now` so expiry during verification is not missed.
    /// Checked before a candidate built from the hold is published, and never
    /// answered from a cached earlier check. A hold that no longer protects
    /// its bytes is reported as [`SourceHoldError::Invalid`].
    /// Each distinct object is read and hashed, with one object buffer bounded
    /// by [`crate::MAX_PAYLOAD_BYTES`]. Storage failures return [`KernelError::Io`].
    /// The pin is rechecked after hashing; this check does not synchronize a later publication.
    pub fn source_hold_status(
        &self,
        binding: &SourceHoldBinding,
        hold_id: &str,
        now: i64,
    ) -> Result<SourceHold, SourceHoldError> {
        self.source_hold_status_inner(binding, hold_id, now, None)
    }

    /// Runs `after_snapshot` after releasing the reader and before verifying objects.
    #[cfg(feature = "test-support")]
    pub fn source_hold_status_with_hook_for_test(
        &self,
        binding: &SourceHoldBinding,
        hold_id: &str,
        now: i64,
        mut after_snapshot: impl FnMut(),
    ) -> Result<SourceHold, SourceHoldError> {
        self.source_hold_status_inner(binding, hold_id, now, Some(&mut after_snapshot))
    }

    fn source_hold_status_inner(
        &self,
        binding: &SourceHoldBinding,
        hold_id: &str,
        now: i64,
        after_snapshot: Option<&mut dyn FnMut()>,
    ) -> Result<SourceHold, SourceHoldError> {
        let started = Instant::now();
        let mut reader = self.lock_reader()?;
        let tx = reader
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(sqlite)?;
        let pin = self.load_valid_pin(&tx, binding, hold_id, now)?;
        let hold = self.hold_from_pin(&tx, binding, hold_id, &pin)?;
        // Every referenced evidence row must still exist with its object on disk.
        let digests = {
            let mut statement = tx
                .prepare_cached(
                    "SELECT DISTINCT e.artifact_digest FROM capture_pin_refs r
                     LEFT JOIN evidence_meta e ON e.evidence_id=r.evidence_id
                     WHERE r.capture_pin_id=?1",
                )
                .map_err(sqlite)?;
            statement
                .query_map([hold_id], |row| row.get::<_, Option<String>>(0))
                .map_err(sqlite)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(sqlite)?
        };
        // The filesystem probes run with no reader connection held.
        drop(tx);
        drop(reader);
        if let Some(after_snapshot) = after_snapshot {
            after_snapshot();
        }
        for digest in &digests {
            let Some(digest) = digest
                .as_deref()
                .filter(|digest| is_artifact_digest(digest))
            else {
                return Err(SourceHoldError::Invalid(SourceHoldInvalidity::MissingBytes));
            };
            match self.artifact_object_presence(digest) {
                ObjectPresence::Present => {}
                ObjectPresence::Absent => {
                    return Err(SourceHoldError::Invalid(SourceHoldInvalidity::MissingBytes));
                }
                // The probe failed, not the bytes; the hold still protects them.
                ObjectPresence::Unreadable => return Err(KernelError::Io.into()),
            }
            self.read_verified_object(digest).map_err(|error| {
                if error.kind() == ArtifactErrorKind::CorruptObject {
                    SourceHoldError::Invalid(SourceHoldInvalidity::MissingBytes)
                } else {
                    SourceHoldError::Kernel(KernelError::Io)
                }
            })?;
        }
        // A purge can commit degradation while its object is still readable.
        let mut reader = self.lock_reader()?;
        let tx = reader
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(sqlite)?;
        let elapsed_ms = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
        self.load_valid_pin(&tx, binding, hold_id, now.saturating_add(elapsed_ms))?;
        Ok(hold)
    }

    /// One page of the descriptors the hold captured, in `(class, object_id,
    /// revision)` order after `cursor`, read in its own short transaction at
    /// the hold's S. A hold that is released, expired, or purge-degraded at
    /// `now` serves no page. A descriptor invalidated after S is still listed,
    /// with the invalidating commit.
    pub fn held_descriptors(
        &self,
        binding: &SourceHoldBinding,
        hold_id: &str,
        now: i64,
        cursor: Option<&HeldCursor>,
        limit: NonZeroUsize,
    ) -> Result<HeldPage, SourceHoldError> {
        let mut reader = self.lock_reader()?;
        let tx = reader
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(sqlite)?;
        let pin = self.load_valid_pin(&tx, binding, hold_id, now)?;
        let (class, object_id, revision) = match cursor {
            Some(cursor) => (
                cursor.class.as_str(),
                cursor.object_id.as_str(),
                cursor.revision,
            ),
            None => ("", "", -1),
        };
        let fetch = i64::try_from(limit.get().saturating_add(1)).unwrap_or(i64::MAX);
        let mut statement = tx.prepare_cached(&held_descriptors_sql()).map_err(sqlite)?;
        let mut rows = statement
            .query_map(
                params![pin.snapshot, class, object_id, revision, fetch],
                |row| {
                    Ok((
                        HeldDescriptor {
                            class: row.get(0)?,
                            object_id: row.get(1)?,
                            revision: row.get(2)?,
                            evidence_id: row.get(3)?,
                            artifact_digest: row.get(4)?,
                            byte_length: 0,
                            invalidated_after_snapshot: row.get(6)?,
                        },
                        row.get::<_, i64>(5)?,
                    ))
                },
            )
            .map_err(sqlite)?
            .map(|row| {
                let (mut descriptor, byte_length) = row.map_err(sqlite)?;
                descriptor.byte_length = u64::try_from(byte_length).map_err(corrupt)?;
                Ok(descriptor)
            })
            .collect::<Result<Vec<_>, SourceHoldError>>()?;
        let next = if rows.len() > limit.get() {
            rows.truncate(limit.get());
            rows.last().map(|last| HeldCursor {
                class: last.class.clone(),
                object_id: last.object_id.clone(),
                revision: last.revision,
            })
        } else {
            None
        };
        Ok(HeldPage {
            descriptors: rows,
            next,
        })
    }

    /// Releases the hold under the writer, so its bytes become reclaimable
    /// after the grace period. Releasing an already released hold is a no-op;
    /// a hold that does not exist under this binding is an error.
    pub fn release_source_hold(
        &self,
        binding: &SourceHoldBinding,
        hold_id: &str,
        released_at: i64,
    ) -> Result<(), SourceHoldError> {
        if released_at < 0 {
            return Err(SourceHoldError::InvalidRequest);
        }
        let mut writer = self.lock_writer()?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite)?;
        check_fence(&tx, self.lease_epoch())?;
        self.load_pin(&tx, binding, hold_id)?
            .ok_or(SourceHoldError::Invalid(SourceHoldInvalidity::Missing))?;
        release_capture_pin_in_tx(&tx, hold_id, released_at)?;
        tx.commit().map_err(sqlite)?;
        Ok(())
    }

    /// Releases every hold this consumer captured in an earlier incarnation,
    /// in one writer transaction. A new incarnation captures a new S; it never
    /// resumes an old hold's cursor, and the old obligations are closed here so
    /// their bytes can be reclaimed. Returns the released hold ids in id order.
    pub fn reconcile_source_holds(
        &self,
        consumer_id: &str,
        released_at: i64,
    ) -> Result<Vec<String>, SourceHoldError> {
        if !is_token(consumer_id) || released_at < 0 {
            return Err(SourceHoldError::InvalidRequest);
        }
        let epoch = i64::try_from(self.lease_epoch()).map_err(|_| KernelError::InvalidInput)?;
        let mut writer = self.lock_writer()?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite)?;
        check_fence(&tx, self.lease_epoch())?;
        let stale = release_consumer_holds_in_tx(&tx, consumer_id, Some(epoch), released_at)?;
        tx.commit().map_err(sqlite)?;
        Ok(stale)
    }

    /// Returns `None` when no pin exists or its `pin_kind` is not `SOURCE_HOLD_KIND`.
    fn load_pin(
        &self,
        tx: &Transaction<'_>,
        binding: &SourceHoldBinding,
        hold_id: &str,
    ) -> Result<Option<StoredPin>, SourceHoldError> {
        let row = tx
            .query_row_cached(
                "SELECT pin_kind,owner_id,commit_seq,lease_epoch,created_at,expires_at,
                        released_at,purge_degraded_at
                 FROM capture_pins WHERE capture_pin_id=?1",
                [hold_id],
                |row| {
                    Ok(PinRow {
                        kind: row.get(0)?,
                        owner: row.get(1)?,
                        snapshot: row.get(2)?,
                        epoch: row.get(3)?,
                        captured_at: row.get(4)?,
                        expires_at: row.get(5)?,
                        released_at: row.get(6)?,
                        purge_degraded_at: row.get(7)?,
                    })
                },
            )
            .optional()
            .map_err(sqlite)?;
        let Some(pin) = row else {
            return Ok(None);
        };
        if pin.kind != SOURCE_HOLD_KIND {
            return Ok(None);
        }
        let epoch = u64::try_from(pin.epoch).map_err(corrupt)?;
        if pin.owner != owner_id(binding) || epoch != binding.lease_epoch {
            return Err(SourceHoldError::BindingMismatch);
        }
        Ok(Some(StoredPin {
            snapshot: pin.snapshot,
            captured_at: pin.captured_at,
            expires_at: pin.expires_at.ok_or_else(|| corrupt(()))?,
            released: pin.released_at.is_some(),
            purge_degraded: pin.purge_degraded_at.is_some(),
        }))
    }

    fn load_valid_pin(
        &self,
        tx: &Transaction<'_>,
        binding: &SourceHoldBinding,
        hold_id: &str,
        now: i64,
    ) -> Result<StoredPin, SourceHoldError> {
        let pin = self
            .load_pin(tx, binding, hold_id)?
            .ok_or(SourceHoldError::Invalid(SourceHoldInvalidity::Missing))?;
        let invalidity = if pin.released {
            Some(SourceHoldInvalidity::Released)
        } else if pin.purge_degraded {
            Some(SourceHoldInvalidity::PurgeDegraded)
        } else if now >= pin.expires_at {
            Some(SourceHoldInvalidity::Expired)
        } else {
            None
        };
        match invalidity {
            Some(invalidity) => Err(SourceHoldError::Invalid(invalidity)),
            None => Ok(pin),
        }
    }

    fn hold_from_pin(
        &self,
        tx: &Transaction<'_>,
        binding: &SourceHoldBinding,
        hold_id: &str,
        pin: &StoredPin,
    ) -> Result<SourceHold, SourceHoldError> {
        let (references, encoded_bytes): (i64, Option<i64>) = tx
            .query_row_cached(
                "SELECT COUNT(*),SUM(e.byte_length) FROM capture_pin_refs r
                 LEFT JOIN evidence_meta e ON e.evidence_id=r.evidence_id
                 WHERE r.capture_pin_id=?1",
                [hold_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(sqlite)?;
        Ok(SourceHold {
            hold_id: hold_id.to_string(),
            binding: binding.clone(),
            snapshot: pin.snapshot,
            captured_at: pin.captured_at,
            expires_at: pin.expires_at,
            references: usize::try_from(references).map_err(corrupt)?,
            encoded_bytes: u64::try_from(encoded_bytes.unwrap_or(0)).map_err(corrupt)?,
        })
    }
}

/// `keep_epoch` excludes holds whose `lease_epoch` matches it; `None` releases
/// every unreleased hold of the consumer. Returns the released hold ids in
/// `capture_pin_id` order.
pub(crate) fn release_consumer_holds_in_tx(
    tx: &Transaction<'_>,
    consumer_id: &str,
    keep_epoch: Option<i64>,
    released_at: i64,
) -> Result<Vec<String>, KernelError> {
    let held: Vec<String> = {
        let mut statement = tx
            .prepare_cached(
                "SELECT capture_pin_id FROM capture_pins
                 WHERE pin_kind=?1 AND substr(owner_id,1,length(?2))=?2
                   AND (?3 IS NULL OR lease_epoch<>?3) AND released_at IS NULL
                 ORDER BY capture_pin_id",
            )
            .map_err(map_sqlite)?;
        statement
            .query_map(
                params![SOURCE_HOLD_KIND, owner_prefix(consumer_id), keep_epoch],
                |row| row.get(0),
            )
            .map_err(map_sqlite)?
            .collect::<rusqlite::Result<_>>()
            .map_err(map_sqlite)?
    };
    for hold_id in &held {
        if !release_capture_pin_in_tx(tx, hold_id, released_at)? {
            return Err(KernelError::CorruptCanonicalRow);
        }
    }
    Ok(held)
}

struct PinRow {
    kind: String,
    owner: String,
    snapshot: i64,
    epoch: i64,
    captured_at: i64,
    expires_at: Option<i64>,
    released_at: Option<i64>,
    purge_degraded_at: Option<i64>,
}

struct StoredPin {
    snapshot: i64,
    captured_at: i64,
    expires_at: i64,
    released: bool,
    purge_degraded: bool,
}

#[cfg(test)]
mod tests {
    use rusqlite::{Connection, StatementStatus, params};

    use super::held_descriptors_sql;
    use crate::schema::apply_kernel_schema;

    #[test]
    fn held_pages_seek_without_sorting_the_inventory() {
        let mut baseline_steps = None;
        for count in [32, 512] {
            let mut conn = Connection::open_in_memory().unwrap();
            conn.pragma_update(None, "foreign_keys", true).unwrap();
            apply_kernel_schema(&mut conn, "00000000000000000000000000000000", 0).unwrap();
            let tx = conn.transaction().unwrap();
            tx.execute_batch(
                "INSERT INTO commit_log VALUES (1,'query-plan',1,'test','seed','digest',0,'test','test');
                 INSERT INTO object_registry(object_id,object_kind,domain_id,source_kind,source_id,
                     source_revision,created_commit_seq,sensitivity_class)
                 VALUES ('domain','domain','domain','domain','domain',1,1,'normal'),
                        ('evidence','evidence','domain','artifact','evidence',1,1,'normal');
                 INSERT INTO domains(domain_id,object_id,name,created_commit_seq,sensitivity_class)
                 VALUES ('domain','domain','domain',1,'normal');
                 INSERT INTO evidence_meta(evidence_id,object_id,artifact_reference,artifact_digest,
                     byte_length,media_type,retention_class,provider_egress_class,redaction_metadata,
                     created_commit_seq,sensitivity_class)
                 VALUES ('evidence','evidence','object','digest',1,'text/plain','canonical',
                         'local_only',x'5b5d',1,'normal');",
            )
            .unwrap();
            for index in 0..count {
                for (prefix, kind) in [("srcdesc:", "source_descriptor"), ("other:", "other")] {
                    let id = format!("{prefix}{index:06}:1");
                    tx.execute(
                        "INSERT INTO object_registry(object_id,object_kind,domain_id,source_kind,
                             source_id,source_revision,created_commit_seq,sensitivity_class)
                         VALUES (?1,'observation','domain','messages',?1,1,1,'normal')",
                        [&id],
                    )
                    .unwrap();
                    tx.execute(
                        "INSERT INTO observations(observation_id,object_id,evidence_id,
                             observation_kind,observation_payload,observed_at,created_commit_seq,
                             sensitivity_class)
                         VALUES (?1,?1,'evidence',?2,x'7b7d',0,1,'normal')",
                        params![id, kind],
                    )
                    .unwrap();
                }
            }
            tx.commit().unwrap();
            for after in [0, count / 2, count - 10] {
                let mut statement = conn.prepare(&held_descriptors_sql()).unwrap();
                let rows = statement
                    .query_map(
                        params![1, "messages", format!("srcdesc:{after:06}:1"), 1, 9],
                        |row| row.get::<_, String>(1),
                    )
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap();
                let expected: Vec<_> = (after + 1..after + 10)
                    .map(|index| format!("srcdesc:{index:06}:1"))
                    .collect();
                assert_eq!(rows, expected);
                let sorts = statement.get_status(StatementStatus::Sort);
                let steps = statement.get_status(StatementStatus::VmStep);
                eprintln!(
                    "SQLite {}: count={count}, after={after}, sorts={sorts}, steps={steps}",
                    rusqlite::version()
                );
                assert_eq!(sorts, 0, "a held page must not sort the inventory");
                let baseline = *baseline_steps.get_or_insert(steps);
                assert!(
                    steps <= baseline * 2,
                    "page work grew from {baseline} to {steps} steps"
                );
            }
        }
    }
}
