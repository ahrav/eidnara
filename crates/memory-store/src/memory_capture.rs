//! Durable automatic-capture inbox. Source text is redacted before storage;
//! identities are rejected rather than rewritten. Prepared model output is
//! frozen before the kernel commit, so recovery repeats identical operations.
//! Completed rows retain their replay identity but release source/output bytes.

use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};

use crate::{DurableWriteFamily, MemoryStore, MemoryStoreError, PreparedWrite, WriteDisposition};

pub const MAX_CAPTURE_SOURCE_BYTES: usize = 64 * 1024;
pub const MAX_CAPTURE_PREPARED_BYTES: usize = 128 * 1024;
const MAX_CAPTURE_BATCH_RETAINED_BYTES: usize = 256 * 1024;
pub const MAX_PENDING_CAPTURE_PER_PROJECT: i64 = 1024;
pub const MAX_PENDING_CAPTURE_TOTAL: i64 = 8192;
pub const MAX_CAPTURE_FAILURES: u32 = 3;

/// One native completed text message, scoped by the host's route.
pub struct CaptureSource<'a> {
    pub project: &'a str,
    pub harness: &'a str,
    pub session_id: &'a str,
    pub message_id: &'a str,
    pub role: &'a str,
    pub text: &'a str,
}

/// `Full` is not an acknowledgment: the caller must keep the source for retry.
#[derive(Debug, PartialEq, Eq)]
pub enum CaptureEnqueue {
    Accepted {
        job_id: String,
        replayed: bool,
    },
    Full,
    /// A resumed or switched conversation cannot copy earlier text into a new project's memory.
    ProjectMismatch,
}

/// Debug excludes source text and prepared model output.
#[derive(Clone)]
pub struct CaptureJob {
    pub job_id: String,
    pub project: String,
    pub harness: String,
    pub session_id: String,
    pub message_id: String,
    pub role: String,
    pub text: String,
    pub prepared: Option<String>,
    pub attempts: u32,
    pub failures: u32,
}

/// Only content-free state is exposed by the status surface.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CaptureQueueStatus {
    pub pending: i64,
    pub prepared: i64,
    pub completed: i64,
    pub failed: i64,
}

fn job_id(source: &CaptureSource<'_>) -> String {
    let mut hash = Sha256::new();
    hash.update(b"eidnara-memory-capture-source-v1");
    for part in [
        source.project,
        source.harness,
        source.session_id,
        source.message_id,
        source.role,
        source.text,
    ] {
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part.as_bytes());
    }
    format!("{:x}", hash.finalize())
}

fn prepared_write(project: &str, job: &str) -> Result<PreparedWrite, MemoryStoreError> {
    let mut write = PreparedWrite::new(DurableWriteFamily::MemoryCapture);
    write.domain_owner("project", project, job);
    write.identity("project", project)?;
    write.identity("job_id", job)?;
    Ok(write)
}

impl MemoryStore {
    /// A new writer may have repaired credentials or runtime configuration.
    /// Keep dispatch counts and retry deadlines, but give pending sources a
    /// fresh failure allowance. Prepared commits never need another model call.
    pub(crate) fn resume_memory_capture_after_open(&self) -> Result<(), MemoryStoreError> {
        PreparedWrite::new(DurableWriteFamily::MemoryCapture).execute(&self.inner, |tx| {
            tx.tx().execute("UPDATE memory_capture_jobs SET failures=0 WHERE commit_seq IS NULL AND prepared_json IS NULL AND failures>0", [])?;
            Ok(WriteDisposition::Applied(()))
        })
    }

    /// Enqueues redacted source text before acknowledging it. Replaying a
    /// completed source does not recreate its payload or consume a queue slot.
    pub fn enqueue_memory_capture(
        &self,
        source: CaptureSource<'_>,
        now_ms: i64,
    ) -> Result<CaptureEnqueue, MemoryStoreError> {
        if !matches!(source.harness, "opencode" | "pi")
            || !matches!(source.role, "user" | "assistant")
            || source.text.trim().is_empty()
            || source.text.len() > MAX_CAPTURE_SOURCE_BYTES
            || [source.session_id, source.message_id]
                .iter()
                .any(|id| id.is_empty() || id.len() > 256 || id.chars().any(char::is_control))
            || source.project.is_empty()
            || source.project.len() > 4096
        {
            return Err(MemoryStoreError::Serde(
                "invalid memory capture source".into(),
            ));
        }
        let job = job_id(&source);
        let mut write = prepared_write(source.project, &job)?;
        write.identity("session_id", source.session_id)?;
        write.identity("message_id", source.message_id)?;
        let text = write.content("text", source.text)?;
        if text.len() > MAX_CAPTURE_SOURCE_BYTES {
            return Err(MemoryStoreError::Serde(
                "redacted capture source requires smaller fragments".into(),
            ));
        }
        write.execute(&self.inner, |tx| {
            let tx = tx.tx();
            let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM memory_capture_jobs WHERE job_id=?1)", [&job], |row| row.get(0))?;
            if exists {
                return Ok(WriteDisposition::Replay(CaptureEnqueue::Accepted { job_id: job.clone(), replayed: true }));
            }
            let mismatched: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM memory_capture_jobs WHERE session_id=?1 AND project<>?2)",
                params![source.session_id, source.project], |row| row.get(0),
            )?;
            if mismatched { return Ok(WriteDisposition::Replay(CaptureEnqueue::ProjectMismatch)); }
            let (project_pending, total): (i64, i64) = tx.query_row(
                "SELECT COALESCE(SUM(project=?1),0),COUNT(*) FROM memory_capture_jobs WHERE commit_seq IS NULL",
                [source.project], |row| Ok((row.get(0)?, row.get(1)?)))?;
            if project_pending >= MAX_PENDING_CAPTURE_PER_PROJECT || total >= MAX_PENDING_CAPTURE_TOTAL {
                return Ok(WriteDisposition::Replay(CaptureEnqueue::Full));
            }
            tx.execute("INSERT INTO memory_capture_jobs (job_id,project,harness,session_id,message_id,role,text,created_at_ms) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![job,source.project,source.harness,source.session_id,source.message_id,source.role,text,now_ms])?;
            Ok(WriteDisposition::Applied(CaptureEnqueue::Accepted { job_id: job.clone(), replayed: false }))
        })
    }

    /// Reads at most 32 jobs and 64 KiB of source text. Prepared jobs respect
    /// commit retry deadlines but do not consume the model-failure allowance.
    pub fn pending_memory_captures(
        &self,
        project: &str,
        harness: &str,
        now_ms: i64,
    ) -> Result<Vec<CaptureJob>, MemoryStoreError> {
        self.inner.with_conn(|conn| {
            let mut statement = conn.prepare_cached("SELECT job_id,project,harness,session_id,message_id,role,text,prepared_json,attempts,failures FROM memory_capture_jobs WHERE project=?1 AND harness=?2 AND commit_seq IS NULL AND retry_at_ms<=?3 AND (prepared_json IS NOT NULL OR failures<?4) ORDER BY created_at_ms,rowid LIMIT 32")?;
            let rows = statement.query_map(params![project,harness,now_ms,MAX_CAPTURE_FAILURES], |row| Ok(CaptureJob {
                job_id: row.get(0)?, project: row.get(1)?, harness: row.get(2)?, session_id: row.get(3)?, message_id: row.get(4)?, role: row.get(5)?, text: row.get(6)?, prepared: row.get(7)?, attempts: row.get(8)?, failures: row.get(9)?,
            }))?;
            let mut jobs = Vec::new();
            let mut bytes = 0;
            let mut retained_bytes = 0;
            for row in rows {
                let job = row?;
                let charge = job.text.len() + job.prepared.as_ref().map_or(0, String::len);
                if bytes + job.text.len() > MAX_CAPTURE_SOURCE_BYTES || retained_bytes + charge > MAX_CAPTURE_BATCH_RETAINED_BYTES { break; }
                bytes += job.text.len();
                retained_bytes += charge;
                jobs.push(job);
            }
            Ok(jobs)
        }).map_err(Into::into)
    }

    /// Next retry of unfinished work. Sources at the model-failure limit remain
    /// visible in status but need a new store owner before further extraction.
    pub fn next_memory_capture_retry(
        &self,
        project: &str,
        harness: &str,
    ) -> Result<Option<i64>, MemoryStoreError> {
        self.inner.with_conn(|conn| conn.query_row(
            "SELECT MIN(retry_at_ms) FROM memory_capture_jobs WHERE project=?1 AND harness=?2 AND commit_seq IS NULL AND (prepared_json IS NOT NULL OR failures<?3)",
            params![project, harness, MAX_CAPTURE_FAILURES], |row| row.get(0),
        )).map_err(Into::into)
    }

    /// Records each dispatch, including interrupted ones, for replay and backoff.
    /// A deleted/completed job cannot acquire an attempt.
    pub fn begin_memory_capture_attempt(
        &self,
        project: &str,
        job: &str,
        retry_at_ms: i64,
    ) -> Result<bool, MemoryStoreError> {
        prepared_write(project, job)?.execute(&self.inner, |tx| {
            let changed = tx.tx().execute("UPDATE memory_capture_jobs SET attempts=attempts+1,retry_at_ms=?3 WHERE project=?1 AND job_id=?2 AND commit_seq IS NULL AND prepared_json IS NULL", params![project,job,retry_at_ms])?;
            Ok(WriteDisposition::Applied(changed == 1))
        })
    }

    /// Freezes accepted output. Replays return the first frozen bytes, never
    /// replacing a possibly committed plan with a stochastic new response.
    pub fn prepare_memory_capture(
        &self,
        project: &str,
        job: &str,
        output: &str,
    ) -> Result<Option<String>, MemoryStoreError> {
        if output.len() > MAX_CAPTURE_PREPARED_BYTES {
            return Err(MemoryStoreError::Serde(
                "memory capture output is too large".into(),
            ));
        }
        let previous: Option<(Option<String>, Option<i64>)> = self.inner.with_conn(|conn| conn.query_row(
            "SELECT prepared_json,commit_seq FROM memory_capture_jobs WHERE project=?1 AND job_id=?2",
            params![project,job], |row| Ok((row.get(0)?,row.get(1)?)),
        ).optional())?;
        match previous {
            Some((Some(frozen), None)) => return Ok(Some(frozen)),
            None | Some((_, Some(_))) => return Ok(None),
            Some((None, None)) => {}
        }
        let mut write = prepared_write(project, job)?;
        // Redaction here would invalidate source quotations. Refuse instead of
        // storing output different from what the daemon validated.
        let output = write.identity("prepared_json", output)?;
        write.execute(&self.inner, |tx| {
            tx.tx().execute("UPDATE memory_capture_jobs SET prepared_json=?3,last_error=NULL,retry_at_ms=0 WHERE project=?1 AND job_id=?2 AND commit_seq IS NULL AND prepared_json IS NULL",params![project,job,output])?;
            let stored = tx.tx().query_row("SELECT prepared_json FROM memory_capture_jobs WHERE project=?1 AND job_id=?2 AND commit_seq IS NULL",params![project,job],|row| row.get(0)).optional()?.flatten();
            Ok(WriteDisposition::Applied(stored))
        })
    }

    /// A rejected kernel transaction made no write. Reconcile again only if
    /// this is still the same frozen plan; a committed source is immutable.
    pub fn retry_memory_capture_reconciliation(
        &self,
        project: &str,
        job: &str,
        frozen: &str,
    ) -> Result<(), MemoryStoreError> {
        prepared_write(project, job)?.execute(&self.inner, |tx| {
            tx.tx().execute("UPDATE memory_capture_jobs SET prepared_json=NULL,last_error='reconciliation_conflict',retry_at_ms=0 WHERE project=?1 AND job_id=?2 AND prepared_json=?3 AND commit_seq IS NULL", params![project, job, frozen])?;
            Ok(WriteDisposition::Applied(()))
        })
    }

    /// Marks a proven kernel commit, releasing payload bytes but retaining the
    /// source identity. A no-memory result still needs a kernel receipt.
    pub fn complete_memory_capture(
        &self,
        project: &str,
        job: &str,
        commit_seq: i64,
    ) -> Result<bool, MemoryStoreError> {
        if commit_seq < 0 {
            return Err(MemoryStoreError::Serde(
                "invalid capture commit sequence".into(),
            ));
        }
        prepared_write(project, job)?.execute(&self.inner, |tx| {
            let changed = tx.tx().execute("UPDATE memory_capture_jobs SET commit_seq=?3,text='',prepared_json=NULL,last_error=NULL WHERE project=?1 AND job_id=?2 AND commit_seq IS NULL AND prepared_json IS NOT NULL",params![project,job,commit_seq])?;
            Ok(WriteDisposition::Applied(changed == 1))
        })
    }

    /// Failures retain the source and any frozen output for later recovery.
    /// The caller supplies a closed error code, never a provider error body.
    pub fn fail_memory_capture(
        &self,
        project: &str,
        job: &str,
        code: &str,
        retry_at_ms: i64,
        model_failure: bool,
    ) -> Result<(), MemoryStoreError> {
        if code.is_empty()
            || code.len() > 64
            || !code.bytes().all(|c| c.is_ascii_lowercase() || c == b'_')
        {
            return Err(MemoryStoreError::Serde("invalid capture error code".into()));
        }
        prepared_write(project, job)?.execute(&self.inner, |tx| {
            tx.tx().execute("UPDATE memory_capture_jobs SET last_error=?3,retry_at_ms=?4,failures=failures+?5 WHERE project=?1 AND job_id=?2 AND commit_seq IS NULL",params![project,job,code,retry_at_ms, i64::from(model_failure)])?;
            Ok(WriteDisposition::Applied(()))
        })
    }

    pub fn memory_capture_status(
        &self,
        project: &str,
    ) -> Result<CaptureQueueStatus, MemoryStoreError> {
        self.inner.with_conn(|conn| conn.query_row("SELECT COALESCE(SUM(commit_seq IS NULL),0),COALESCE(SUM(prepared_json IS NOT NULL),0),COALESCE(SUM(commit_seq IS NOT NULL),0),COALESCE(SUM(commit_seq IS NULL AND last_error IS NOT NULL),0) FROM memory_capture_jobs WHERE project=?1",[project],|row| Ok(CaptureQueueStatus { pending:row.get(0)?,prepared:row.get(1)?,completed:row.get(2)?,failed:row.get(3)? }))).map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(text: &str) -> CaptureSource<'_> {
        CaptureSource {
            project: "/project",
            harness: "pi",
            session_id: "session",
            message_id: "native-1",
            role: "user",
            text,
        }
    }

    fn accepted(result: CaptureEnqueue) -> String {
        match result {
            CaptureEnqueue::Accepted { job_id, .. } => job_id,
            CaptureEnqueue::Full => panic!("queue full"),
            CaptureEnqueue::ProjectMismatch => panic!("project mismatch"),
        }
    }

    #[test]
    fn capture_replays_and_freezes_output_across_restart() {
        let dir = tempfile::tempdir().unwrap();
        let descriptor = MemoryStore::test_descriptor(dir.path(), "capture");
        let store = MemoryStore::open(&descriptor).unwrap();
        let id = accepted(
            store
                .enqueue_memory_capture(source("Use staging port 4321."), 1)
                .unwrap(),
        );
        assert!(
            store
                .begin_memory_capture_attempt("/project", &id, 100)
                .unwrap()
        );
        assert_eq!(
            store.prepare_memory_capture("/project", &id, "[]").unwrap(),
            Some("[]".into())
        );
        assert_eq!(
            store
                .prepare_memory_capture("/project", &id, "[1]")
                .unwrap(),
            Some("[]".into())
        );
        drop(store);
        let store = MemoryStore::open(&descriptor).unwrap();
        let jobs = store.pending_memory_captures("/project", "pi", 2).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].attempts, 1);
        assert_eq!(jobs[0].prepared.as_deref(), Some("[]"));
        assert!(store.complete_memory_capture("/project", &id, 7).unwrap());
        assert_eq!(
            store
                .enqueue_memory_capture(source("Use staging port 4321."), 3)
                .unwrap(),
            CaptureEnqueue::Accepted {
                job_id: id,
                replayed: true
            }
        );
        assert!(
            store
                .pending_memory_captures("/project", "pi", 200)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store.memory_capture_status("/project").unwrap().completed,
            1
        );
    }

    #[test]
    fn capture_redacts_sources_and_refuses_secret_outputs_and_identities() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::open_for_test(dir.path(), "capture");
        let id = accepted(
            store
                .enqueue_memory_capture(source("password=hunter-two. Use staging port 4321."), 1)
                .unwrap(),
        );
        let jobs = store.pending_memory_captures("/project", "pi", 1).unwrap();
        assert!(!jobs[0].text.contains("hunter-two"));
        assert!(jobs[0].text.contains("4321"));
        assert!(
            store
                .prepare_memory_capture("/project", &id, "password=hunter-two")
                .is_err()
        );
        let mut bad = source("Keep this.");
        bad.message_id = "password=hunter-two";
        assert!(store.enqueue_memory_capture(bad, 1).is_err());
        assert!(
            store
                .pending_memory_captures("/other", "pi", 1)
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .pending_memory_captures("/project", "opencode", 1)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn interrupted_dispatches_do_not_exhaust_confirmed_failure_allowance() {
        let dir = tempfile::tempdir().unwrap();
        let descriptor = MemoryStore::test_descriptor(dir.path(), "capture");
        for attempt in 0..5 {
            let store = MemoryStore::open(&descriptor).unwrap();
            let id = accepted(
                store
                    .enqueue_memory_capture(source("Keep this project fact."), 0)
                    .unwrap(),
            );
            let pending = store
                .pending_memory_captures("/project", "pi", 100)
                .unwrap();
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0].attempts, attempt);
            assert_eq!(pending[0].failures, 0);
            assert!(
                store
                    .begin_memory_capture_attempt("/project", &id, 0)
                    .unwrap()
            );
            // No terminal model error was recorded before this simulated crash.
        }
    }

    #[test]
    fn confirmed_failures_pause_work_until_a_new_owner_can_retry() {
        let dir = tempfile::tempdir().unwrap();
        let descriptor = MemoryStore::test_descriptor(dir.path(), "capture");
        let store = MemoryStore::open(&descriptor).unwrap();
        let id = accepted(
            store
                .enqueue_memory_capture(source("Keep this project fact."), 0)
                .unwrap(),
        );
        for _ in 0..MAX_CAPTURE_FAILURES {
            store
                .begin_memory_capture_attempt("/project", &id, 0)
                .unwrap();
            store
                .fail_memory_capture("/project", &id, "extraction_failed", 0, true)
                .unwrap();
        }
        assert!(
            store
                .pending_memory_captures("/project", "pi", 100)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store.memory_capture_status("/project").unwrap().completed,
            0
        );
        drop(store);
        let reopened = MemoryStore::open(&descriptor).unwrap();
        let pending = reopened
            .pending_memory_captures("/project", "pi", 100)
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].attempts, MAX_CAPTURE_FAILURES);
        assert_eq!(pending[0].failures, 0);
    }

    #[test]
    fn capture_never_recaptures_a_conversation_under_another_project() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::open_for_test(dir.path(), "capture");
        store
            .enqueue_memory_capture(source("Project-private fact."), 1)
            .unwrap();
        let mut switched = source("Project-private fact.");
        switched.project = "/different-project";
        assert_eq!(
            store.enqueue_memory_capture(switched, 2).unwrap(),
            CaptureEnqueue::ProjectMismatch
        );
        assert_eq!(
            store.memory_capture_status("/different-project").unwrap(),
            CaptureQueueStatus::default()
        );
    }

    #[test]
    fn capture_failure_and_session_deletion_do_not_report_success() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::open_for_test(dir.path(), "capture");
        let id = accepted(
            store
                .enqueue_memory_capture(source("Keep pending work."), 1)
                .unwrap(),
        );
        store
            .fail_memory_capture("/project", &id, "provider_failed", 100, true)
            .unwrap();
        assert_eq!(
            store.memory_capture_status("/project").unwrap(),
            CaptureQueueStatus {
                pending: 1,
                prepared: 0,
                completed: 0,
                failed: 1
            }
        );
        assert!(
            store
                .pending_memory_captures("/project", "pi", 99)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store
                .pending_memory_captures("/project", "pi", 100)
                .unwrap()
                .len(),
            1
        );
        assert!(!store.complete_memory_capture("/project", &id, 7).unwrap());
        store.delete_session("session", "/project").unwrap();
        assert!(
            !store
                .begin_memory_capture_attempt("/project", &id, 101)
                .unwrap()
        );
        assert_eq!(
            store.prepare_memory_capture("/project", &id, "[]").unwrap(),
            None
        );
    }
}
