//! A source hold pins the exact-retained evidence every live source descriptor
//! cites at one canonical sequence S, for one registered consumer, in one
//! store incarnation, under one source policy, until a finite expiry. The
//! capture runs under the kernel writer, so S and the protection of its bytes
//! are one step and nothing can be reclaimed between them. The same hold is
//! later extended over catch-up windows `(S, through]`, and the consumer's
//! checkpoint moves to `through` only through a hold that already covers the
//! window. Every admission runs from counts over the whole hold before a
//! reference is written, and an over-bound hold is refused whole rather than
//! narrowed. Every use of a hold in another store incarnation is refused.

use std::num::{NonZeroU64, NonZeroUsize};

use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

use super::backup::release_capture_pin_in_tx;
use super::cas::{ObjectPresence, is_artifact_digest};
use super::envelope::check_fence;
use super::outbox::acknowledge_outbox_in_tx;
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

/// What one hold may reference in total, checked from `COUNT` and `SUM`
/// before any reference row exists. A capture and every later extension of
/// the same hold are each admitted against the whole hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceHoldAdmission {
    pub max_references: NonZeroUsize,
    pub max_encoded_bytes: NonZeroU64,
}

/// Admission plus the finite lifetime a capture gives the hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceHoldBounds {
    pub admission: SourceHoldAdmission,
    /// Finite lifetime of the hold from capture, in the store's millisecond
    /// clock. An extension never renews it.
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
    /// A referenced evidence row is gone, or its object file is absent from disk.
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
        "source hold would reference {references} evidence rows totalling {encoded_bytes} bytes in all, over the admitted bound"
    )]
    Unadmitted {
        references: usize,
        encoded_bytes: u64,
    },
    #[error(
        "source hold consumer already has {MAX_ACTIVE_SOURCE_HOLDS_PER_CONSUMER} unreleased holds"
    )]
    HoldLimitReached,
    #[error(
        "source hold does not reference {uncovered} evidence rows cited by descriptors through the commit being acknowledged"
    )]
    ExtensionIncomplete { uncovered: usize },
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

/// Which descriptors in a commit window cite evidence a hold protects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Descriptors {
    /// Descriptors live at the window's end: a capture over `(0, S]` drops
    /// descriptors invalidated at or before S, because the projection at S
    /// never sees them.
    LiveAtEnd,
    /// Every descriptor created in the window, including one invalidated
    /// inside it, because a consumer replaying the window still meets its
    /// creation.
    CreatedInWindow,
}

/// One commit window `(after, through]` of descriptor creations.
#[derive(Clone, Copy)]
struct Window {
    after: i64,
    through: i64,
    descriptors: Descriptors,
}

impl Window {
    fn at_snapshot(snapshot: i64) -> Self {
        Self {
            after: 0,
            through: snapshot,
            descriptors: Descriptors::LiveAtEnd,
        }
    }

    fn catch_up(snapshot: i64, through: i64) -> Self {
        Self {
            after: snapshot,
            through,
            descriptors: Descriptors::CreatedInWindow,
        }
    }

    /// Evidence cited by the window's descriptors whose evidence row is not
    /// invalidated at or before `?1`, the window's end; `?2` is its start. A
    /// logical delete or a purge invalidates the evidence row, so a descriptor
    /// over deleted bytes is not a candidate rather than a held reference that
    /// can never be read. Shared by every count, insert, and page so they
    /// cannot disagree.
    fn cited_evidence_sql(self) -> String {
        let descriptor_liveness = match self.descriptors {
            Descriptors::LiveAtEnd => {
                "AND (b.invalidated_commit_seq IS NULL OR b.invalidated_commit_seq>?1)"
            }
            Descriptors::CreatedInWindow => "",
        };
        format!(
            "FROM observations b
             JOIN object_registry o ON o.object_id=b.object_id
             JOIN evidence_meta e ON e.evidence_id=b.evidence_id
             WHERE b.observation_kind='{SOURCE_DESCRIPTOR_KIND}'
               AND b.created_commit_seq>?2 AND b.created_commit_seq<=?1
               {descriptor_liveness}
               AND (e.invalidated_commit_seq IS NULL OR e.invalidated_commit_seq>?1)"
        )
    }

    /// Distinct cited evidence that hold `?3` does not reference yet, with
    /// its byte length.
    fn unreferenced_evidence_sql(self) -> String {
        format!(
            "SELECT e.evidence_id,e.byte_length {}
               AND NOT EXISTS(SELECT 1 FROM capture_pin_refs r
                              WHERE r.capture_pin_id=?3 AND r.evidence_id=e.evidence_id)
             GROUP BY e.evidence_id",
            self.cited_evidence_sql()
        )
    }
}

fn is_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
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
        let hold_id: String = tx
            .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
            .map_err(sqlite)?;
        let captured_at = current_time_ms();
        let expires_at = captured_at
            .checked_add(expiry)
            .ok_or(SourceHoldError::InvalidRequest)?;
        let epoch = i64::try_from(self.lease_epoch()).map_err(|_| KernelError::InvalidInput)?;
        let admitted = admit_references(
            &tx,
            HoldRow {
                hold_id: &hold_id,
                expires_at,
            },
            Window::at_snapshot(snapshot),
            (0, 0),
            bounds.admission,
            at_admission,
        )?;
        tx.execute_cached(
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
        admitted.materialize(&tx)?;
        let (references, encoded_bytes) = (admitted.references, admitted.encoded_bytes);
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

    /// Extends the hold to the evidence cited by descriptors created in
    /// `(S, through]`, so a consumer can finish the catch-up batch that ends
    /// at `through` with every byte protected. Runs under the writer against
    /// the whole hold's admission; an over-bound extension is refused whole
    /// and writes nothing. Replaying an extension adds no reference twice,
    /// charges nothing twice, and never renews the expiry.
    pub fn extend_source_hold(
        &self,
        binding: &SourceHoldBinding,
        hold_id: &str,
        through: i64,
        admission: SourceHoldAdmission,
    ) -> Result<SourceHold, SourceHoldError> {
        self.extend_source_hold_inner(binding, hold_id, through, admission, None)
    }

    /// [`Self::extend_source_hold`] with `at_admission` run inside the writer
    /// transaction after the admission decision and before any reference is
    /// written, receiving the number of `capture_pin_refs` rows visible there.
    #[cfg(feature = "test-support")]
    pub fn extend_source_hold_with_hook_for_test(
        &self,
        binding: &SourceHoldBinding,
        hold_id: &str,
        through: i64,
        admission: SourceHoldAdmission,
        mut at_admission: impl FnMut(i64),
    ) -> Result<SourceHold, SourceHoldError> {
        self.extend_source_hold_inner(
            binding,
            hold_id,
            through,
            admission,
            Some(&mut at_admission),
        )
    }

    fn extend_source_hold_inner(
        &self,
        binding: &SourceHoldBinding,
        hold_id: &str,
        through: i64,
        admission: SourceHoldAdmission,
        at_admission: Option<&mut dyn FnMut(i64)>,
    ) -> Result<SourceHold, SourceHoldError> {
        let mut writer = self.lock_writer()?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite)?;
        check_fence(&tx, self.lease_epoch())?;
        let pin = self.load_valid_pin(&tx, binding, hold_id, current_time_ms())?;
        let hold = self.hold_from_pin(&tx, binding, hold_id, &pin)?;
        check_window(&tx, hold.snapshot, through)?;
        let admitted = admit_references(
            &tx,
            HoldRow {
                hold_id,
                expires_at: hold.expires_at,
            },
            Window::catch_up(hold.snapshot, through),
            (hold.references, hold.encoded_bytes),
            admission,
            at_admission,
        )?;
        admitted.materialize(&tx)?;
        tx.commit().map_err(sqlite)?;
        Ok(SourceHold {
            references: admitted.references,
            encoded_bytes: admitted.encoded_bytes,
            ..hold
        })
    }

    /// Moves the consumer's checkpoint to `through` in the same writer transaction that checks the hold.
    /// The hold is unreleased, unexpired, not purge-degraded, and references every byte required to replay `(S, through]`.
    /// `Self::source_hold_status` checks object presence before publication; this method does not.
    /// A failed or skipped extension cannot authorize acknowledgement.
    /// The hold keeps the bytes protected after the checkpoint moves until it is released.
    /// Acknowledging through a hold asserts that the consumer's state
    /// reflects the corpus at S plus the window, so a checkpoint below S
    /// moves over `(checkpoint, S]` on the strength of the S baseline.
    pub fn acknowledge_through_source_hold(
        &self,
        binding: &SourceHoldBinding,
        hold_id: &str,
        through: i64,
        updated_at: i64,
    ) -> Result<(), SourceHoldError> {
        if updated_at < 0 {
            return Err(SourceHoldError::InvalidRequest);
        }
        let mut writer = self.lock_writer()?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite)?;
        check_fence(&tx, self.lease_epoch())?;
        let pin = self.load_valid_pin(&tx, binding, hold_id, current_time_ms())?;
        check_window(&tx, pin.snapshot, through)?;
        let uncovered: i64 = tx
            .query_row_cached(
                &format!(
                    "SELECT COUNT(*) FROM ({})",
                    Window::catch_up(pin.snapshot, through).unreferenced_evidence_sql()
                ),
                params![through, pin.snapshot, hold_id],
                |row| row.get(0),
            )
            .map_err(sqlite)?;
        if uncovered != 0 {
            return Err(SourceHoldError::ExtensionIncomplete {
                uncovered: usize::try_from(uncovered).map_err(corrupt)?,
            });
        }
        acknowledge_outbox_in_tx(&tx, &binding.consumer_id, through, updated_at)?;
        tx.commit().map_err(sqlite)?;
        Ok(())
    }

    /// Whether the hold still protects every byte it captured at `now`.
    /// Checked before a candidate built from the hold is published, and never
    /// answered from a cached earlier check. A hold that no longer protects
    /// its bytes is reported as [`SourceHoldError::Invalid`].
    pub fn source_hold_status(
        &self,
        binding: &SourceHoldBinding,
        hold_id: &str,
        now: i64,
    ) -> Result<SourceHold, SourceHoldError> {
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
                    "SELECT e.artifact_digest FROM capture_pin_refs r
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
        }
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
        let window = Window::at_snapshot(pin.snapshot);
        let mut statement = tx
            .prepare_cached(&format!(
                "SELECT o.source_kind,o.object_id,o.source_revision,e.evidence_id,
                        e.artifact_digest,e.byte_length,b.invalidated_commit_seq
                 {}
                   AND (o.source_kind,o.object_id,o.source_revision)>(?3,?4,?5)
                 ORDER BY o.source_kind,o.object_id,o.source_revision
                 LIMIT ?6",
                window.cited_evidence_sql()
            ))
            .map_err(sqlite)?;
        let mut rows = statement
            .query_map(
                params![
                    window.through,
                    window.after,
                    class,
                    object_id,
                    revision,
                    fetch
                ],
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
        if binding.lease_epoch != self.lease_epoch() {
            return Err(SourceHoldError::IncarnationMismatch);
        }
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

/// The pin an admitted set is written to, fixed at admission time so the set
/// cannot be counted against one pin and written to another.
#[derive(Clone, Copy)]
struct HoldRow<'a> {
    hold_id: &'a str,
    expires_at: i64,
}

/// An admitted set of additions, counted but not yet written.
struct Admitted<'a> {
    row: HoldRow<'a>,
    window: Window,
    /// Distinct evidence rows the window adds to the hold.
    added: usize,
    /// Whole-hold totals after the additions.
    references: usize,
    encoded_bytes: u64,
}

impl Admitted<'_> {
    /// Writes the admitted references with one `INSERT ... SELECT` bounded
    /// by the admitted count. `OR IGNORE` makes a replayed extension a no-op
    /// per reference, and the whole-hold count is checked afterwards.
    fn materialize(&self, tx: &Transaction<'_>) -> Result<(), SourceHoldError> {
        tx.execute_cached(
            &format!(
                "INSERT OR IGNORE INTO capture_pin_refs(capture_pin_id,evidence_id,expires_at)
                 SELECT ?3,evidence_id,?4 FROM ({}) LIMIT ?5",
                self.window.unreferenced_evidence_sql()
            ),
            params![
                self.window.through,
                self.window.after,
                self.row.hold_id,
                self.row.expires_at,
                i64::try_from(self.added).map_err(corrupt)?,
            ],
        )
        .map_err(sqlite)?;
        let total: i64 = tx
            .query_row_cached(
                "SELECT COUNT(*) FROM capture_pin_refs WHERE capture_pin_id=?1",
                [self.row.hold_id],
                |row| row.get(0),
            )
            .map_err(sqlite)?;
        if usize::try_from(total).map_err(corrupt)? != self.references {
            return Err(KernelError::CorruptCanonicalRow.into());
        }
        Ok(())
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

/// Counts the distinct unreferenced evidence in `window`, adds it to the
/// hold's current totals, and refuses whole when the result exceeds
/// `admission`. Runs before any reference row is written.
fn admit_references<'a>(
    tx: &Transaction<'_>,
    row: HoldRow<'a>,
    window: Window,
    current: (usize, u64),
    admission: SourceHoldAdmission,
    at_admission: Option<&mut dyn FnMut(i64)>,
) -> Result<Admitted<'a>, SourceHoldError> {
    let (added, added_bytes): (i64, Option<i64>) = tx
        .query_row_cached(
            &format!(
                "SELECT COUNT(*),SUM(byte_length) FROM ({})",
                window.unreferenced_evidence_sql()
            ),
            params![window.through, window.after, row.hold_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(sqlite)?;
    let added = usize::try_from(added).map_err(corrupt)?;
    let references = current.0.checked_add(added).ok_or_else(|| corrupt(()))?;
    let encoded_bytes = current
        .1
        .checked_add(u64::try_from(added_bytes.unwrap_or(0)).map_err(corrupt)?)
        .ok_or_else(|| corrupt(()))?;
    if let Some(hook) = at_admission {
        let refs_in_tx: i64 = tx
            .query_row_cached("SELECT COUNT(*) FROM capture_pin_refs", [], |row| {
                row.get(0)
            })
            .map_err(sqlite)?;
        hook(refs_in_tx);
    }
    if references > admission.max_references.get()
        || encoded_bytes > admission.max_encoded_bytes.get()
    {
        return Err(SourceHoldError::Unadmitted {
            references,
            encoded_bytes,
        });
    }
    Ok(Admitted {
        row,
        window,
        added,
        references,
        encoded_bytes,
    })
}

/// `through` must lie in `[S, tip]`: an extension or acknowledgement can
/// neither move before S nor name a commit that does not exist yet.
fn check_window(tx: &Transaction<'_>, snapshot: i64, through: i64) -> Result<(), SourceHoldError> {
    let tip: i64 = tx
        .query_row_cached(
            "SELECT COALESCE(MAX(commit_seq),0) FROM commit_log",
            [],
            |row| row.get(0),
        )
        .map_err(sqlite)?;
    if through < snapshot || through > tip {
        return Err(SourceHoldError::InvalidRequest);
    }
    Ok(())
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
