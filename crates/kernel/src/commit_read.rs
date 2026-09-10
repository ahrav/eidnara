//! A registered consumer reads canonical commits in `(after_commit, through_commit]` one complete
//! commit at a time; a commit is never split across pages, an oversized commit blocks rather than
//! skips, and missing history or malformed ordinals are refusals rather than an empty stream.

use std::num::{NonZeroU64, NonZeroUsize};

use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

use super::outbox::{OutboxEntry, outbox_entry};
use super::{CachedSql, KernelError, KernelStore, map_sqlite};

/// Identifies the reader and the target it captured once; retries carry the same request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitReadRequest {
    pub consumer_id: String,
    pub incarnation: CommitReadIncarnation,
    /// Commits at or below this sequence are already applied.
    pub after_commit: i64,
    /// The fixed terminal target; nothing above it is read.
    pub through_commit: i64,
}

/// Identifies the history that a captured request's commit sequences refer to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CommitReadIncarnation {
    open_nonce: i64,
    /// A restore keeps `open_nonce` but can let later commits reuse sequences a captured request
    /// already names.
    restore_generation: u64,
}

/// A reader guard captures the target and incarnation together, preventing a restore from
/// separating them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitReadTarget {
    pub through_commit: i64,
    pub incarnation: CommitReadIncarnation,
}

/// Page capacity, checked from `COUNT` and `SUM(LENGTH())` before any payload is selected.
/// `max_payload_bytes` covers payload blobs only, not the other `OutboxEntry` columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitPageBounds {
    pub max_commits: NonZeroUsize,
    pub max_rows: NonZeroUsize,
    pub max_payload_bytes: NonZeroU64,
}

/// The whole of one canonical commit: every retained outbox row in ordinal order, or none for an
/// empty commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompleteCommit {
    pub commit_seq: i64,
    pub rows: Vec<OutboxEntry>,
}

/// Why a page ended where it did. Only this value decides whether the caller is done: `through` can
/// sit below `through_commit` when the target names a gap in `commit_log`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageEnd {
    /// Every commit through the target is in this or an earlier page.
    Exhausted,
    /// The next commit did not fit this page's remaining capacity. The caller applies this page,
    /// then reads again from `through`; the next read decides whether that commit fits a page
    /// alone.
    Deferred { next_commit: i64 },
    /// The first commit of an otherwise empty page exceeds a bound on its own, so nothing was
    /// returned and progress cannot move past `through` without a wider bound.
    Oversized {
        commit_seq: i64,
        rows: usize,
        payload_bytes: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitPage {
    pub commits: Vec<CompleteCommit>,
    /// The highest commit sequence this page completes, or `after_commit` when it completes none.
    pub through: i64,
    pub end: PageEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CommitReadError {
    #[error("commit read request is malformed")]
    InvalidRequest,
    #[error("commit read names a consumer that is not registered")]
    UnknownConsumer,
    #[error("commit read was captured in another store incarnation")]
    IncarnationMismatch,
    #[error("commit read target lies beyond the committed tip")]
    TargetBeyondTip,
    #[error("commit read range lies below the consumer checkpoint")]
    BelowCheckpoint,
    #[error("commit {commit_seq} has retained events whose outbox rows are gone")]
    MissingHistory { commit_seq: i64 },
    #[error("commit {commit_seq} has more outbox rows than retained events")]
    SurplusRows { commit_seq: i64 },
    #[error("commit {commit_seq} has ordinals that are not 0..n")]
    MalformedOrdinals { commit_seq: i64 },
    #[error(transparent)]
    Kernel(#[from] KernelError),
}

fn sqlite(error: rusqlite::Error) -> CommitReadError {
    CommitReadError::Kernel(map_sqlite(error))
}

fn tip(tx: &Transaction<'_>) -> rusqlite::Result<i64> {
    tx.query_row_cached(
        "SELECT COALESCE(MAX(commit_seq),0) FROM commit_log",
        [],
        |row| row.get(0),
    )
}

struct CommitShape {
    rows: usize,
    payload_bytes: u64,
}

fn shape(tx: &Transaction<'_>, commit_seq: i64) -> Result<CommitShape, CommitReadError> {
    let (events, event_ordinals): (i64, (Option<i64>, Option<i64>)) = tx
        .query_row_cached(
            "SELECT COUNT(*),MIN(ordinal),MAX(ordinal) FROM change_event WHERE commit_seq=?1",
            [commit_seq],
            |row| Ok((row.get(0)?, (row.get(1)?, row.get(2)?))),
        )
        .map_err(sqlite)?;
    let (rows, row_ordinals, payload_bytes): (i64, (Option<i64>, Option<i64>), Option<i64>) = tx
        .query_row_cached(
            "SELECT COUNT(*),MIN(ordinal),MAX(ordinal),SUM(LENGTH(payload))
             FROM outbox WHERE commit_seq=?1",
            [commit_seq],
            |row| Ok((row.get(0)?, (row.get(1)?, row.get(2)?), row.get(3)?)),
        )
        .map_err(sqlite)?;
    if rows < events {
        return Err(CommitReadError::MissingHistory { commit_seq });
    }
    if rows > events {
        return Err(CommitReadError::SurplusRows { commit_seq });
    }
    // Both ends are checked because `ordinal` has no CHECK constraint, and `rows-1` is used
    // because `MAX+1` overflows at `i64::MAX`.
    let contiguous = |(min, max): (Option<i64>, Option<i64>)| match (min, max) {
        (None, None) => rows == 0,
        (Some(0), Some(max)) => max == rows - 1,
        _ => false,
    };
    if !contiguous(event_ordinals) || !contiguous(row_ordinals) {
        return Err(CommitReadError::MalformedOrdinals { commit_seq });
    }
    let corrupt = || CommitReadError::Kernel(KernelError::CorruptCanonicalRow);
    Ok(CommitShape {
        rows: usize::try_from(rows).map_err(|_| corrupt())?,
        payload_bytes: u64::try_from(payload_bytes.unwrap_or(0)).map_err(|_| corrupt())?,
    })
}

fn outbox_rows(
    tx: &Transaction<'_>,
    commit_seq: i64,
    shape: &CommitShape,
) -> Result<Vec<OutboxEntry>, CommitReadError> {
    let last = i64::try_from(shape.rows)
        .map_err(|_| CommitReadError::Kernel(KernelError::CorruptCanonicalRow))?
        - 1;
    let mut statement = tx
        .prepare_cached(
            "SELECT outbox_position,commit_seq,ordinal,object_id,object_kind,source_kind,
                    source_id,source_revision,sensitivity_class,payload,created_at,ordinal=?2
             FROM outbox WHERE commit_seq=?1 ORDER BY ordinal",
        )
        .map_err(sqlite)?;
    let rows = statement
        .query_map(params![commit_seq, last], outbox_entry)
        .map_err(sqlite)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(sqlite)?;
    if rows.len() != shape.rows {
        return Err(CommitReadError::Kernel(KernelError::CorruptCanonicalRow));
    }
    Ok(rows)
}

impl KernelStore {
    #[cfg(feature = "test-support")]
    pub fn materialized_outbox_rows_for_test(&self) -> usize {
        self.materialized_outbox_rows
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Captures the committed tip and the incarnation it belongs to from one reader snapshot.
    pub fn capture_commit_read_target(&self) -> Result<CommitReadTarget, KernelError> {
        let mut reader = self.lock_reader()?;
        let tx = reader
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(map_sqlite)?;
        Ok(CommitReadTarget {
            through_commit: tip(&tx).map_err(map_sqlite)?,
            incarnation: self.incarnation(),
        })
    }

    /// Must run under a reader guard because a restore holds every guard while advancing
    /// `restore_generation` and swapping the database.
    fn incarnation(&self) -> CommitReadIncarnation {
        CommitReadIncarnation {
            open_nonce: self.open_nonce,
            restore_generation: self
                .restore_generation
                .load(std::sync::atomic::Ordering::SeqCst),
        }
    }

    /// Reads whole commits in `(after_commit, through_commit]` for a registered consumer until a
    /// bound is reached. Shapes are measured with `COUNT` and `SUM(LENGTH())` before a payload is
    /// selected, so admission precedes materialization. Consumer, incarnation, target, and
    /// checkpoint are checked before any commit is read, and the retained `change_event` inventory
    /// distinguishes an empty commit from pruned outbox history. Everything is read in one snapshot
    /// on one reader connection.
    pub fn read_complete_commits(
        &self,
        request: &CommitReadRequest,
        bounds: CommitPageBounds,
    ) -> Result<CommitPage, CommitReadError> {
        if request.consumer_id.trim().is_empty()
            || request.after_commit < 0
            || request.through_commit < request.after_commit
        {
            return Err(CommitReadError::InvalidRequest);
        }
        let mut reader = self.lock_reader()?;
        if request.incarnation != self.incarnation() {
            return Err(CommitReadError::IncarnationMismatch);
        }
        let tx = reader
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(sqlite)?;
        let checkpoint: i64 = tx
            .query_row_cached(
                "SELECT checkpoint_commit_seq FROM outbox_consumers WHERE consumer_id=?1",
                [request.consumer_id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(sqlite)?
            .ok_or(CommitReadError::UnknownConsumer)?;
        if request.after_commit < checkpoint {
            return Err(CommitReadError::BelowCheckpoint);
        }
        if request.through_commit > tip(&tx).map_err(sqlite)? {
            return Err(CommitReadError::TargetBeyondTip);
        }
        // One more than the page can hold, so the commit that ends the page is seen without enumerating the whole range.
        let limit = i64::try_from(bounds.max_commits.get())
            .unwrap_or(i64::MAX - 1)
            .saturating_add(1);
        let mut sequences = tx
            .prepare_cached(
                "SELECT commit_seq FROM commit_log
                 WHERE commit_seq>?1 AND commit_seq<=?2 ORDER BY commit_seq LIMIT ?3",
            )
            .map_err(sqlite)?;
        let sequences = sequences
            .query_map(
                params![request.after_commit, request.through_commit, limit],
                |row| row.get::<_, i64>(0),
            )
            .map_err(sqlite)?;

        let mut commits = Vec::new();
        let mut through = request.after_commit;
        let mut used_rows = 0usize;
        let mut used_bytes = 0u64;
        let mut end = None;
        for commit_seq in sequences {
            let commit_seq = commit_seq.map_err(sqlite)?;
            // A page full by count ends before inspecting the next commit; defects in that commit
            // belong to the page it opens.
            if commits.len() == bounds.max_commits.get() {
                end = Some(PageEnd::Deferred {
                    next_commit: commit_seq,
                });
                break;
            }
            let shape = shape(&tx, commit_seq)?;
            let fits = used_rows + shape.rows <= bounds.max_rows.get()
                && used_bytes + shape.payload_bytes <= bounds.max_payload_bytes.get();
            if !fits {
                end = Some(if commits.is_empty() {
                    PageEnd::Oversized {
                        commit_seq,
                        rows: shape.rows,
                        payload_bytes: shape.payload_bytes,
                    }
                } else {
                    PageEnd::Deferred {
                        next_commit: commit_seq,
                    }
                });
                break;
            }
            let rows = outbox_rows(&tx, commit_seq, &shape)?;
            #[cfg(feature = "test-support")]
            self.materialized_outbox_rows
                .fetch_add(rows.len(), std::sync::atomic::Ordering::SeqCst);
            used_rows += shape.rows;
            used_bytes += shape.payload_bytes;
            through = commit_seq;
            commits.push(CompleteCommit { commit_seq, rows });
        }
        Ok(CommitPage {
            commits,
            through,
            end: end.unwrap_or(PageEnd::Exhausted),
        })
    }
}
