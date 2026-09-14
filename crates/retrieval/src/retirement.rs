//! Replacement-local certificates. Effects and authoritative inventory belong to the caller.

use kernel::ConsumerObligation;
use rusqlite::{OptionalExtension, params};
use storage::GuardedConn;

use crate::ProjectionError;

/// The receipt binds `through` as the commit-sequence bound.
pub struct RetirementReceipt<'a> {
    pub old_consumer: &'a str,
    pub old_generation: &'a str,
    pub old_family: &'a str,
    pub selected_family: &'a str,
    pub kernel_incarnation: &'a str,
    pub through: i64,
}

/// Exact rows, not matching counts, establish completeness against the supplied authority.
pub fn verify_receipt(
    conn: &GuardedConn<'_>,
    receipt: &RetirementReceipt<'_>,
    authority: &[ConsumerObligation],
) -> Result<bool, ProjectionError> {
    let valid = conn
        .query_row(
            "SELECT generation_id=?2 AND old_consumer_id=?3 AND old_family=?1
         AND selected_family=?4 AND kernel_incarnation_id=?5 AND through_commit_seq=?6
         AND obligation_count=?7 AND reason='consumer_retired'
         FROM retirement_receipts WHERE receipt_id=?1",
            params![
                receipt.old_family,
                receipt.old_generation,
                receipt.old_consumer,
                receipt.selected_family,
                receipt.kernel_incarnation,
                receipt.through,
                authority.len() as i64
            ],
            |row| row.get::<_, bool>(0),
        )
        .optional()?;
    let Some(valid) = valid else { return Ok(false) };
    if !valid {
        return Err(ProjectionError::MutationConflict);
    }
    let expected_bytes: usize = authority
        .iter()
        .map(|fact| {
            fact.kind.len() + fact.identity.len() + fact.artifact_digest.len() + "removed".len()
        })
        .sum();
    let (rows, bytes): (i64, i64) = conn.query_row(
        "SELECT count(*),coalesce(sum(size),0) FROM
         (SELECT length(CAST(kind AS BLOB))+length(CAST(identity AS BLOB))+
          length(CAST(artifact_digest AS BLOB))+length(CAST(disposition AS BLOB)) AS size
          FROM retirement_dispositions WHERE receipt_id=?1 LIMIT ?2)",
        params![receipt.old_family, authority.len() as i64 + 1],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if rows != authority.len() as i64 || bytes != expected_bytes as i64 {
        return Err(ProjectionError::CorruptRow);
    }
    let mut statement = conn.prepare(
        "SELECT kind,identity,artifact_digest,commit_seq,invalidated_commit_seq,disposition
         FROM retirement_dispositions WHERE receipt_id=?1 ORDER BY kind,identity",
    )?;
    let mut rows = statement.query([receipt.old_family])?;
    for expected in authority {
        let row = rows.next()?.ok_or(ProjectionError::CorruptRow)?;
        let found = ConsumerObligation {
            kind: row.get(0)?,
            identity: row.get(1)?,
            artifact_digest: row.get(2)?,
            commit_seq: row.get(3)?,
            invalidated_commit_seq: row.get(4)?,
        };
        if &found != expected || row.get::<_, String>(5)? != "removed" {
            return Err(ProjectionError::CorruptRow);
        }
    }
    if rows.next()?.is_some() {
        return Err(ProjectionError::CorruptRow);
    }
    Ok(true)
}

/// Calling this function attests that physical holders have drained and the whole old family is removed.
/// The caller owns the transaction and releases it before taking the kernel writer.
pub fn record_receipt(
    conn: &GuardedConn<'_>,
    receipt: &RetirementReceipt<'_>,
    authority: &[ConsumerObligation],
    now: i64,
) -> Result<(), ProjectionError> {
    if authority
        .windows(2)
        .any(|pair| (&pair[0].kind, &pair[0].identity) >= (&pair[1].kind, &pair[1].identity))
        || authority
            .iter()
            .any(|fact| fact.commit_seq > receipt.through)
    {
        return Err(ProjectionError::MutationConflict);
    }
    if verify_receipt(conn, receipt, authority)? {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO retirement_receipts(receipt_id,generation_id,reason,retired_at,recorded_at,
         old_consumer_id,old_family,selected_family,kernel_incarnation_id,through_commit_seq,obligation_count)
         VALUES (?1,?2,'consumer_retired',?3,?3,?4,?1,?5,?6,?7,?8)",
        params![receipt.old_family, receipt.old_generation, now, receipt.old_consumer,
            receipt.selected_family, receipt.kernel_incarnation, receipt.through, authority.len() as i64],
    )?;
    let mut insert =
        conn.prepare("INSERT INTO retirement_dispositions VALUES (?1,?2,?3,?4,?5,?6,'removed')")?;
    for fact in authority {
        insert.execute(params![
            receipt.old_family,
            fact.kind,
            fact.identity,
            fact.artifact_digest,
            fact.commit_seq,
            fact.invalidated_commit_seq
        ])?;
    }
    Ok(())
}
