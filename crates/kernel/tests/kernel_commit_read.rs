//! An independent ledger of every commit predicts what a registered consumer reads.

#![cfg(feature = "test-support")]

use std::collections::BTreeMap;
use std::num::{NonZeroU64, NonZeroUsize};
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

use kernel::{
    BackupRequest, CommitIntent, CommitPageBounds, CommitReadError, CommitReadRequest,
    CompleteCommit, DomainSpec, KernelError, KernelStore, PageEnd, Sensitivity,
};
use rusqlite::{Connection, OpenFlags};

const CONSUMER: &str = "search";

fn bounds(commits: usize, rows: usize, bytes: u64) -> CommitPageBounds {
    CommitPageBounds {
        max_commits: NonZeroUsize::new(commits).unwrap(),
        max_rows: NonZeroUsize::new(rows).unwrap(),
        max_payload_bytes: NonZeroU64::new(bytes).unwrap(),
    }
}

fn wide() -> CommitPageBounds {
    bounds(64, 1024, 1 << 20)
}

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "kernel-commit-read-test".to_string(),
        operation_key: key.to_string(),
        request_digest: "a".repeat(64),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

fn domain(index: usize) -> DomainSpec {
    DomainSpec {
        domain_id: format!("domain-{index}"),
        object_id: format!("object-{index}"),
        name: format!("name-{index}"),
        source_kind: "fixture".to_string(),
        source_id: format!("source-{index}"),
        source_revision: i64::try_from(index).unwrap(),
        sensitivity: Sensitivity::Normal,
    }
}

/// The `(object_id, change_kind)` of every change in one commit, in ordinal order.
type Changes = Vec<(String, &'static str)>;
/// One predicted page: its commits, the sequence it completes through, and why it ends.
type PredictedPage = (Vec<(i64, Changes)>, i64, PageEnd);

/// What the test itself wrote, built from the writes and never from a read.
#[derive(Default)]
struct Ledger {
    commits: BTreeMap<i64, Changes>,
    gaps: Vec<i64>,
}

struct Fixture {
    // Fields drop in declaration order, so `store` drops before `root`.
    store: KernelStore,
    root: tempfile::TempDir,
    ledger: Ledger,
}

impl Fixture {
    fn open() -> Self {
        let root = tempfile::tempdir().unwrap();
        let store = KernelStore::open(root.path()).unwrap();
        Self {
            store,
            root,
            ledger: Ledger::default(),
        }
    }

    fn request(&self, after: i64, through: i64) -> CommitReadRequest {
        CommitReadRequest {
            consumer_id: CONSUMER.to_string(),
            incarnation: self.store.capture_commit_read_target().unwrap().incarnation,
            after_commit: after,
            through_commit: through,
        }
    }

    fn register(&mut self, consumer: &str) -> i64 {
        let receipt = self
            .store
            .commit(intent(&format!("register-{consumer}")), |envelope| {
                envelope.register_outbox_consumer(consumer, 1)?;
                Ok(String::new())
            })
            .unwrap();
        self.ledger.commits.insert(
            receipt.commit_seq,
            vec![(consumer.to_string(), "consumer_register")],
        );
        receipt.commit_seq
    }

    fn insert_domains(&mut self, key: &str, indices: &[usize]) -> i64 {
        let receipt = self
            .store
            .commit(intent(key), |envelope| {
                for &index in indices {
                    envelope.insert_domain(domain(index))?;
                }
                Ok(String::new())
            })
            .unwrap();
        self.ledger.commits.insert(
            receipt.commit_seq,
            indices
                .iter()
                .map(|index| (format!("object-{index}"), "insert"))
                .collect(),
        );
        receipt.commit_seq
    }

    fn retire_domain(&mut self, key: &str, index: usize) -> i64 {
        let receipt = self
            .store
            .commit(intent(key), |envelope| {
                envelope.retire_domain(&format!("object-{index}"))?;
                Ok(String::new())
            })
            .unwrap();
        self.ledger.commits.insert(
            receipt.commit_seq,
            vec![(format!("object-{index}"), "retire")],
        );
        receipt.commit_seq
    }

    fn empty(&mut self, key: &str) -> i64 {
        let receipt = self
            .store
            .commit(intent(key), |_| Ok(String::new()))
            .unwrap();
        self.ledger.commits.insert(receipt.commit_seq, Vec::new());
        receipt.commit_seq
    }

    /// A rolled-back commit reverts `sqlite_sequence`, so the only way a gap
    /// appears in `commit_log` is the counter moving without a row. The
    /// failed commit is kept in the recipe to show that it leaves no gap.
    fn gap(&mut self, key: &str, index: usize) {
        let before = self.store.tip().unwrap();
        let error = self
            .store
            .commit_with_fault_after_events_for_test(intent(key), |envelope| {
                envelope.insert_domain(domain(index))?;
                Ok(String::new())
            })
            .unwrap_err();
        assert_eq!(error, KernelError::Fault);
        assert_eq!(self.store.tip().unwrap(), before);
        let mutate = self.mutate();
        mutate
            .execute(
                "UPDATE sqlite_sequence SET seq=seq+1 WHERE name='commit_log'",
                [],
            )
            .unwrap();
        let burned: i64 = mutate
            .query_row(
                "SELECT seq FROM sqlite_sequence WHERE name='commit_log'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(burned > before);
        self.ledger.gaps.push(burned);
    }

    fn tip(&self) -> i64 {
        self.store.tip().unwrap()
    }

    fn inspect(&self) -> Connection {
        Connection::open_with_flags(
            self.root.path().join("kernel.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap()
    }

    fn mutate(&self) -> Connection {
        let connection = Connection::open_with_flags(
            self.root.path().join("kernel.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .unwrap();
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        connection
    }

    fn checkpoint(&self) -> i64 {
        self.inspect()
            .query_row(
                "SELECT checkpoint_commit_seq FROM outbox_consumers WHERE consumer_id=?1",
                [CONSUMER],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn published_rows(&self) -> i64 {
        self.inspect()
            .query_row(
                "SELECT COUNT(*) FROM outbox WHERE published_at IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    /// The pages the ledger predicts for `(after, through]` under `bounds`.
    /// Row counts come from the ledger; payload sizes are read from the
    /// store because the ledger does not encode payloads, and `read_all`
    /// reconciles them with the bytes each returned row carries.
    fn predicted(&self, after: i64, through: i64, bounds: CommitPageBounds) -> Vec<PredictedPage> {
        let sizes: BTreeMap<i64, (usize, u64)> = self
            .inspect()
            .prepare(
                "SELECT commit_seq,COUNT(*),COALESCE(SUM(LENGTH(payload)),0)
                 FROM outbox GROUP BY commit_seq",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    (row.get::<_, i64>(1)? as usize, row.get::<_, i64>(2)? as u64),
                ))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let mut pages = Vec::new();
        let mut page = Vec::new();
        let mut page_through = after;
        let (mut rows, mut bytes) = (0usize, 0u64);
        for (&seq, changes) in self.ledger.commits.range(after + 1..=through) {
            let (commit_rows, commit_bytes) = sizes.get(&seq).copied().unwrap_or((0, 0));
            assert_eq!(
                commit_rows,
                changes.len(),
                "ledger and outbox disagree on {seq}"
            );
            let fits = page.len() < bounds.max_commits.get()
                && rows + commit_rows <= bounds.max_rows.get()
                && bytes + commit_bytes <= bounds.max_payload_bytes.get();
            if !fits {
                if page.is_empty() {
                    pages.push((
                        page,
                        page_through,
                        PageEnd::Oversized {
                            commit_seq: seq,
                            rows: commit_rows,
                            payload_bytes: commit_bytes,
                        },
                    ));
                    return pages;
                }
                pages.push((page, page_through, PageEnd::Deferred { next_commit: seq }));
                page = Vec::new();
                rows = 0;
                bytes = 0;
                let alone = commit_rows <= bounds.max_rows.get()
                    && commit_bytes <= bounds.max_payload_bytes.get();
                if !alone {
                    pages.push((
                        Vec::new(),
                        page_through,
                        PageEnd::Oversized {
                            commit_seq: seq,
                            rows: commit_rows,
                            payload_bytes: commit_bytes,
                        },
                    ));
                    return pages;
                }
            }
            rows += commit_rows;
            bytes += commit_bytes;
            page_through = seq;
            page.push((seq, changes.clone()));
        }
        pages.push((page, page_through, PageEnd::Exhausted));
        pages
    }
}

fn observed(commits: &[CompleteCommit]) -> Vec<(i64, Changes)> {
    commits
        .iter()
        .map(|commit| {
            let ordinals: Vec<i64> = commit.rows.iter().map(|row| row.ordinal).collect();
            assert_eq!(
                ordinals,
                (0..commit.rows.len() as i64).collect::<Vec<_>>(),
                "commit {} ordinals",
                commit.commit_seq
            );
            for row in &commit.rows {
                assert_eq!(row.commit_seq, commit.commit_seq);
                assert_eq!(
                    row.commit_boundary,
                    row.ordinal + 1 == commit.rows.len() as i64
                );
            }
            (
                commit.commit_seq,
                commit
                    .rows
                    .iter()
                    .map(|row| {
                        let payload: serde_json::Value =
                            serde_json::from_slice(&row.payload).unwrap();
                        let kind = payload["change_kind"].as_str().unwrap();
                        let kind: &'static str = match kind {
                            "insert" => "insert",
                            "retire" => "retire",
                            "consumer_register" => "consumer_register",
                            other => panic!("unexpected change kind {other}"),
                        };
                        (row.object_id.clone(), kind)
                    })
                    .collect(),
            )
        })
        .collect()
}

/// Reads every page from `after` through `through`, following `Deferred`
/// ends, and compares each page with the ledger's prediction. Every page is
/// also checked against the bounds from its own returned rows, so the
/// prediction and the implementation cannot share an off-by-one.
fn read_all(
    fixture: &Fixture,
    after: i64,
    through: i64,
    bounds: CommitPageBounds,
) -> Vec<CompleteCommit> {
    let predicted = fixture.predicted(after, through, bounds);
    let mut cursor = after;
    let mut commits = Vec::new();
    for (index, (expected_commits, expected_through, expected_end)) in predicted.iter().enumerate()
    {
        let page = fixture
            .store
            .read_complete_commits(&fixture.request(cursor, through), bounds)
            .unwrap();
        assert_eq!(&observed(&page.commits), expected_commits, "page {index}");
        assert_eq!(page.through, *expected_through, "page {index}");
        assert_eq!(page.end, *expected_end, "page {index}");
        assert!(page.commits.len() <= bounds.max_commits.get());
        let rows: usize = page.commits.iter().map(|commit| commit.rows.len()).sum();
        let bytes: u64 = page
            .commits
            .iter()
            .flat_map(|commit| &commit.rows)
            .map(|row| row.payload.len() as u64)
            .sum();
        assert!(rows <= bounds.max_rows.get(), "page {index} rows");
        assert!(
            bytes <= bounds.max_payload_bytes.get(),
            "page {index} bytes"
        );
        if let PageEnd::Oversized {
            rows,
            payload_bytes,
            ..
        } = page.end
        {
            assert!(rows > bounds.max_rows.get() || payload_bytes > bounds.max_payload_bytes.get());
        }
        commits.extend(page.commits);
        match page.end {
            PageEnd::Deferred { next_commit } => {
                assert!(next_commit > page.through);
                cursor = page.through;
            }
            PageEnd::Exhausted | PageEnd::Oversized { .. } => {
                assert_eq!(index + 1, predicted.len());
            }
        }
    }
    commits
}

fn seeded() -> Fixture {
    let mut fixture = Fixture::open();
    fixture.register(CONSUMER);
    fixture.insert_domains("one", &[1]);
    fixture.insert_domains("many", &[2, 3, 4]);
    fixture.empty("empty-a");
    fixture.gap("gap-a", 90);
    fixture.retire_domain("retire-2", 2);
    fixture.register("other");
    fixture.gap("gap-b", 91);
    fixture.gap("gap-c", 92);
    fixture.empty("empty-b");
    fixture.insert_domains("last", &[5, 6]);
    fixture
}

#[test]
fn every_commit_kind_reads_back_as_the_ledger_predicts_regardless_of_publication() {
    let fixture = seeded();
    let tip = fixture.tip();
    assert!(fixture.ledger.gaps.len() >= 3);
    let distinct: std::collections::BTreeSet<_> = fixture.ledger.gaps.iter().collect();
    assert_eq!(distinct.len(), fixture.ledger.gaps.len());
    for gap in &fixture.ledger.gaps {
        assert!(
            !fixture.ledger.commits.contains_key(gap),
            "gap {gap} is a commit"
        );
        assert!(*gap < tip);
    }
    let before = read_all(&fixture, 0, tip, wide());
    assert_eq!(before.len(), fixture.ledger.commits.len());

    // Publishing every row changes nothing the consumer reads.
    let last_position: i64 = fixture
        .inspect()
        .query_row("SELECT MAX(outbox_position) FROM outbox", [], |row| {
            row.get(0)
        })
        .unwrap();
    fixture
        .store
        .mark_outbox_published_through(last_position, 7)
        .unwrap();
    assert!(fixture.published_rows() > 0);
    assert!(fixture.store.pending_outbox(64).unwrap().is_empty());
    let after = read_all(&fixture, 0, tip, wide());
    assert_eq!(after, before);

    // Sub-ranges and a range that is only gaps and empty commits.
    let seqs: Vec<i64> = fixture.ledger.commits.keys().copied().collect();
    read_all(&fixture, seqs[1], seqs[4], wide());
    let page = fixture
        .store
        .read_complete_commits(&fixture.request(tip, tip), wide())
        .unwrap();
    assert!(page.commits.is_empty());
    assert_eq!(page.through, tip);
    assert_eq!(page.end, PageEnd::Exhausted);

    // A target that names a gap is exhausted at the last real commit below it.
    let gap = *fixture.ledger.gaps.iter().max().unwrap();
    let last_before_gap = *fixture.ledger.commits.range(..gap).next_back().unwrap().0;
    let page = fixture
        .store
        .read_complete_commits(&fixture.request(last_before_gap - 1, gap), wide())
        .unwrap();
    assert_eq!(page.through, last_before_gap);
    assert!(page.through < gap);
    assert_eq!(page.end, PageEnd::Exhausted);
}

#[test]
fn missing_rows_and_malformed_ordinals_fail_and_leave_progress_untouched() {
    let fixture = seeded();
    let tip = fixture.tip();
    let many = *fixture
        .ledger
        .commits
        .iter()
        .find(|(_, changes)| changes.len() == 3)
        .unwrap()
        .0;
    let checkpoint = fixture.checkpoint();
    let published = fixture.published_rows();
    let pending = fixture.store.pending_outbox(64).unwrap();

    // Delete one row of a three-row commit: retained events outnumber rows.
    fixture
        .mutate()
        .execute(
            "DELETE FROM outbox WHERE commit_seq=?1 AND ordinal=1",
            [many],
        )
        .unwrap();
    assert_eq!(
        fixture
            .store
            .read_complete_commits(&fixture.request(0, tip), wide())
            .unwrap_err(),
        CommitReadError::MissingHistory { commit_seq: many }
    );
    // The commits before the damaged one still read; the failure is not an
    // empty stream.
    let page = fixture
        .store
        .read_complete_commits(&fixture.request(0, many - 1), wide())
        .unwrap();
    assert!(!page.commits.is_empty());
    // A page that is already full by commit count ends before the damaged
    // commit is inspected: the healthy commit is delivered and deferred, and the
    // refusal arrives on the page the damaged commit would open.
    let full = fixture
        .store
        .read_complete_commits(&fixture.request(many - 2, tip), bounds(1, 1024, 1 << 20))
        .unwrap();
    assert_eq!(
        observed(&full.commits),
        vec![(many - 1, fixture.ledger.commits[&(many - 1)].clone())]
    );
    assert_eq!(full.through, many - 1);
    assert_eq!(full.end, PageEnd::Deferred { next_commit: many });
    assert_eq!(
        fixture
            .store
            .read_complete_commits(&fixture.request(many - 1, tip), bounds(1, 1024, 1 << 20))
            .unwrap_err(),
        CommitReadError::MissingHistory { commit_seq: many }
    );

    // Restore the row under a wrong ordinal: counts agree, ordinals do not.
    fixture
        .mutate()
        .execute(
            "INSERT INTO outbox(commit_seq,ordinal,object_id,object_kind,source_kind,source_id,
                                source_revision,sensitivity_class,payload,created_at)
             VALUES (?1,7,'object-3','domain','fixture','source-3',3,'normal',X'7B7D',0)",
            [many],
        )
        .unwrap();
    assert_eq!(
        fixture
            .store
            .read_complete_commits(&fixture.request(0, tip), wide())
            .unwrap_err(),
        CommitReadError::MalformedOrdinals { commit_seq: many }
    );
    fixture
        .mutate()
        .execute(
            "UPDATE outbox SET ordinal=1 WHERE commit_seq=?1 AND ordinal=7",
            [many],
        )
        .unwrap();
    // Ordinals {-1,0,2} satisfy `MAX(ordinal)+1 == COUNT(*)`, and neither table
    // bounds `ordinal` below, so the reader must reject the low end itself.
    fixture
        .mutate()
        .execute(
            "UPDATE outbox SET ordinal=-1 WHERE commit_seq=?1 AND ordinal=1",
            [many],
        )
        .unwrap();
    assert_eq!(
        fixture
            .store
            .read_complete_commits(&fixture.request(0, tip), wide())
            .unwrap_err(),
        CommitReadError::MalformedOrdinals { commit_seq: many }
    );
    fixture
        .mutate()
        .execute(
            "UPDATE outbox SET ordinal=1 WHERE commit_seq=?1 AND ordinal=-1",
            [many],
        )
        .unwrap();
    // The highest representable ordinal is a typed refusal, not an overflow.
    fixture
        .mutate()
        .execute(
            "UPDATE outbox SET ordinal=?2 WHERE commit_seq=?1 AND ordinal=2",
            rusqlite::params![many, i64::MAX],
        )
        .unwrap();
    assert_eq!(
        fixture
            .store
            .read_complete_commits(&fixture.request(0, tip), wide())
            .unwrap_err(),
        CommitReadError::MalformedOrdinals { commit_seq: many }
    );
    fixture
        .mutate()
        .execute(
            "UPDATE outbox SET ordinal=2 WHERE commit_seq=?1 AND ordinal=?2",
            rusqlite::params![many, i64::MAX],
        )
        .unwrap();
    // A fourth row on a three-event commit is a surplus, not an ordinal fault.
    fixture
        .mutate()
        .execute(
            "INSERT INTO outbox(commit_seq,ordinal,object_id,object_kind,source_kind,source_id,
                                source_revision,sensitivity_class,payload,created_at)
             VALUES (?1,3,'object-9','domain','fixture','source-9',9,'normal',X'7B7D',0)",
            [many],
        )
        .unwrap();
    assert_eq!(
        fixture
            .store
            .read_complete_commits(&fixture.request(0, tip), wide())
            .unwrap_err(),
        CommitReadError::SurplusRows { commit_seq: many }
    );
    // Every row of the commit gone: it must not read back as an empty commit.
    fixture
        .mutate()
        .execute("DELETE FROM outbox WHERE commit_seq=?1", [many])
        .unwrap();
    assert_eq!(
        fixture
            .store
            .read_complete_commits(&fixture.request(0, tip), wide())
            .unwrap_err(),
        CommitReadError::MissingHistory { commit_seq: many }
    );
    // The same read excludes the empty commit that really is empty.
    let empty = *fixture
        .ledger
        .commits
        .iter()
        .find(|(_, changes)| changes.is_empty())
        .unwrap()
        .0;
    let page = fixture
        .store
        .read_complete_commits(&fixture.request(empty - 1, empty), wide())
        .unwrap();
    assert_eq!(page.commits.len(), 1);
    assert!(page.commits[0].rows.is_empty());

    // The reader itself moved nothing: the checkpoint and publication state
    // are as before, and every surviving row is still unpublished. The row
    // count differs only by the rows this test deleted directly.
    assert_eq!(fixture.checkpoint(), checkpoint);
    assert_eq!(fixture.published_rows(), published);
    let surviving: i64 = fixture
        .inspect()
        .query_row("SELECT COUNT(*) FROM outbox", [], |row| row.get(0))
        .unwrap();
    assert_eq!(surviving as usize, pending.len() - 3);
    assert_eq!(
        fixture.store.pending_outbox(64).unwrap().len(),
        surviving as usize
    );
}

#[test]
fn admission_precedes_materialization_and_refusal_moves_nothing() {
    let fixture = seeded();
    let tip = fixture.tip();
    let many = *fixture
        .ledger
        .commits
        .iter()
        .find(|(_, changes)| changes.len() == 3)
        .unwrap()
        .0;
    let checkpoint = fixture.checkpoint();
    let pending = fixture.store.pending_outbox(64).unwrap().len();

    // Measure the commit's payload bytes from the rows a wide read returns,
    // not from the expression the reader uses for admission.
    let full = fixture
        .store
        .read_complete_commits(&fixture.request(many - 1, many), wide())
        .unwrap();
    let many_bytes: u64 = full.commits[0]
        .rows
        .iter()
        .map(|row| row.payload.len() as u64)
        .sum();
    assert!(many_bytes > 0);

    // The counter is scoped per store, so another store's reads cannot move it.
    let other = seeded();
    let before_other = fixture.store.materialized_outbox_rows_for_test();
    let other_page = other
        .store
        .read_complete_commits(&other.request(0, other.tip()), wide())
        .unwrap();
    assert!(
        other_page
            .commits
            .iter()
            .any(|commit| !commit.rows.is_empty())
    );
    assert_eq!(
        fixture.store.materialized_outbox_rows_for_test(),
        before_other,
        "another store's read moved this store's counter"
    );
    drop(other);

    // Exact fit: a three-row commit under a three-row, exact-byte bound reads whole.
    let exact = bounds(1, 3, many_bytes);
    let page = fixture
        .store
        .read_complete_commits(&fixture.request(many - 1, many), exact)
        .unwrap();
    assert_eq!(page.commits.len(), 1);
    assert_eq!(page.commits[0].rows.len(), 3);
    assert_eq!(page.end, PageEnd::Exhausted);

    // One row or one byte less and the same commit is oversized as the first
    // commit of the page: no payload is selected and progress stays at `after`.
    for tight in [bounds(1, 2, many_bytes), bounds(1, 3, many_bytes - 1)] {
        let materialized = fixture.store.materialized_outbox_rows_for_test();
        let page = fixture
            .store
            .read_complete_commits(&fixture.request(many - 1, tip), tight)
            .unwrap();
        assert!(page.commits.is_empty());
        assert_eq!(page.through, many - 1);
        assert_eq!(
            page.end,
            PageEnd::Oversized {
                commit_seq: many,
                rows: 3,
                payload_bytes: many_bytes,
            }
        );
        assert_eq!(
            fixture.store.materialized_outbox_rows_for_test(),
            materialized,
            "a refused commit selected payload rows"
        );
    }

    // Reached mid-page, the same commit ends the page as deferred; the read
    // from that point is what reports it oversized. Neither read selects it.
    let tight = bounds(64, 2, 1 << 20);
    let materialized = fixture.store.materialized_outbox_rows_for_test();
    let first = fixture
        .store
        .read_complete_commits(&fixture.request(0, tip), tight)
        .unwrap();
    assert!(!first.commits.is_empty());
    assert_eq!(first.end, PageEnd::Deferred { next_commit: many });
    assert_eq!(first.through, many - 1);
    let selected: usize = first.commits.iter().map(|commit| commit.rows.len()).sum();
    assert_eq!(
        fixture.store.materialized_outbox_rows_for_test(),
        materialized + selected
    );
    let blocked = fixture
        .store
        .read_complete_commits(&fixture.request(first.through, tip), tight)
        .unwrap();
    assert!(blocked.commits.is_empty());
    assert!(matches!(blocked.end, PageEnd::Oversized { commit_seq, .. } if commit_seq == many));
    assert_eq!(
        fixture.store.materialized_outbox_rows_for_test(),
        materialized + selected
    );

    // A commit that fits alone but not beside the previous one is deferred
    // whole to the next page; the ledger predicts every page boundary.
    for paging in [
        bounds(1, 1024, 1 << 20),
        bounds(64, 3, 1 << 20),
        bounds(64, 1024, many_bytes),
        bounds(2, 4, 1 << 20),
    ] {
        let pages = fixture.predicted(0, tip, paging);
        assert!(pages.len() > 1, "{paging:?} should need several pages");
        assert!(
            pages
                .iter()
                .all(|(_, _, end)| !matches!(end, PageEnd::Oversized { .. }))
        );
        let commits = read_all(&fixture, 0, tip, paging);
        assert_eq!(commits.len(), fixture.ledger.commits.len());
    }

    assert_eq!(fixture.checkpoint(), checkpoint);
    assert_eq!(fixture.store.pending_outbox(64).unwrap().len(), pending);
    assert_eq!(fixture.published_rows(), 0);
}

#[test]
fn retries_keep_the_captured_target_and_the_wrong_reader_fails_closed() {
    let mut fixture = seeded();
    let capture = fixture.store.capture_commit_read_target().unwrap();
    let target = capture.through_commit;
    assert_eq!(target, fixture.tip());
    let request = CommitReadRequest {
        consumer_id: CONSUMER.to_string(),
        incarnation: capture.incarnation,
        after_commit: 0,
        through_commit: target,
    };
    assert_eq!(request, fixture.request(0, target));
    let first = fixture
        .store
        .read_complete_commits(&request, wide())
        .unwrap();
    assert_eq!(first.through, target);

    // New commits after the target are invisible to a retry of the same request.
    fixture.insert_domains("after-target", &[7, 8]);
    fixture.empty("after-target-empty");
    let retry = fixture
        .store
        .read_complete_commits(&fixture.request(0, target), wide())
        .unwrap();
    assert_eq!(retry, first);
    // An overlapping replay from an earlier cursor returns the same commits.
    let seqs: Vec<i64> = fixture.ledger.commits.keys().copied().collect();
    let overlap = fixture
        .store
        .read_complete_commits(&fixture.request(seqs[2], target), wide())
        .unwrap();
    assert_eq!(
        overlap.commits,
        first
            .commits
            .iter()
            .filter(|commit| commit.commit_seq > seqs[2])
            .cloned()
            .collect::<Vec<_>>()
    );

    let mut wrong_consumer = fixture.request(0, target);
    wrong_consumer.consumer_id = "nobody".to_string();
    assert_eq!(
        fixture
            .store
            .read_complete_commits(&wrong_consumer, wide())
            .unwrap_err(),
        CommitReadError::UnknownConsumer
    );
    let tip = fixture.tip();
    assert_eq!(
        fixture
            .store
            .read_complete_commits(&fixture.request(0, tip + 1), wide())
            .unwrap_err(),
        CommitReadError::TargetBeyondTip
    );
    for (after, through) in [(-1, 1), (5, 4)] {
        assert_eq!(
            fixture
                .store
                .read_complete_commits(&fixture.request(after, through), wide())
                .unwrap_err(),
            CommitReadError::InvalidRequest
        );
    }
    // Reopening the store starts a new incarnation; the captured request is
    // stale and a fresh one reads the same commits.
    let mut stale = fixture.request(0, target);
    let Fixture { root, store, .. } = fixture;
    drop(store);
    let reopened = KernelStore::open(root.path()).unwrap();
    assert_eq!(
        reopened.read_complete_commits(&stale, wide()).unwrap_err(),
        CommitReadError::IncarnationMismatch
    );
    stale.incarnation = reopened.capture_commit_read_target().unwrap().incarnation;
    let again = reopened.read_complete_commits(&stale, wide()).unwrap();
    assert_eq!(again, first);
}

#[test]
fn acknowledgement_and_pruning_bound_what_a_consumer_may_still_read() {
    let fixture = seeded();
    let seqs: Vec<i64> = fixture.ledger.commits.keys().copied().collect();
    let tip = fixture.tip();
    // Acknowledge through the fourth commit and prune: rows at or below it go.
    fixture
        .store
        .acknowledge_outbox(CONSUMER, seqs[3], 5)
        .unwrap();
    fixture.store.acknowledge_outbox("other", tip, 5).unwrap();
    let pruned = fixture.store.prune_outbox().unwrap();
    assert_eq!(pruned.horizon, seqs[3]);
    assert!(pruned.deleted > 0);

    // A range that starts below the checkpoint could name pruned history and
    // is refused before any commit is inspected; the retained range reads.
    assert_eq!(
        fixture
            .store
            .read_complete_commits(&fixture.request(seqs[3] - 1, tip), wide())
            .unwrap_err(),
        CommitReadError::BelowCheckpoint
    );
    let page = fixture
        .store
        .read_complete_commits(&fixture.request(seqs[3], tip), wide())
        .unwrap();
    assert_eq!(
        observed(&page.commits),
        fixture.predicted(seqs[3], tip, wide()).remove(0).0
    );
    assert_eq!(page.end, PageEnd::Exhausted);

    // Publisher regressions: `pending_outbox` still answers in position order
    // with commit boundaries, and the read above published nothing.
    let pending = fixture.store.pending_outbox(64).unwrap();
    assert!(!pending.is_empty());
    assert!(
        pending
            .windows(2)
            .all(|pair| pair[0].outbox_position < pair[1].outbox_position)
    );
    assert!(pending.last().unwrap().commit_boundary);
    assert_eq!(fixture.published_rows(), 0);
}

#[test]
fn a_restore_under_the_same_handle_is_a_new_incarnation() {
    let fixture = seeded();
    let target = *fixture.ledger.commits.keys().nth(2).unwrap();
    let captured = fixture.request(0, target);
    let first = fixture
        .store
        .read_complete_commits(&captured, wide())
        .unwrap();
    assert_eq!(first.through, target);

    let mut other = Fixture::open();
    other.register(CONSUMER);
    other.insert_domains("other-one", &[20]);
    other.insert_domains("other-many", &[21, 22]);
    assert!(other.tip() >= target);
    assert_eq!(
        fixture
            .store
            .read_complete_commits(&other.request(0, target), wide())
            .unwrap_err(),
        CommitReadError::IncarnationMismatch
    );
    let destination = tempfile::tempdir().unwrap();
    std::fs::set_permissions(destination.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let backup = other
        .store
        .backup(BackupRequest {
            destination_directory: destination.path().to_path_buf(),
            deadline: Instant::now() + Duration::from_secs(30),
            capture_pin_expires_at: None,
        })
        .unwrap();
    assert_eq!(
        fixture.store.restore(&backup.destination_path).unwrap(),
        other.tip()
    );

    assert_eq!(
        fixture
            .store
            .read_complete_commits(&captured, wide())
            .unwrap_err(),
        CommitReadError::IncarnationMismatch
    );
    let restored = fixture
        .store
        .read_complete_commits(&fixture.request(0, target), wide())
        .unwrap();
    let expected = other
        .store
        .read_complete_commits(&other.request(0, target), wide())
        .unwrap();
    assert_eq!(restored, expected);
    assert_ne!(restored, first);

    // Both roots now hold the same database identity. Each reopened root acquires
    // a root-local lease epoch and starts a fresh restore generation.
    let Fixture {
        root: root_a,
        store: store_a,
        ..
    } = fixture;
    let Fixture {
        root: root_b,
        store: store_b,
        ..
    } = other;
    drop((store_a, store_b));
    let reopen = |root: tempfile::TempDir| {
        let store = KernelStore::open(root.path()).unwrap();
        Fixture {
            store,
            root,
            ledger: Ledger::default(),
        }
    };
    let mut a = reopen(root_a);
    let mut b = reopen(root_b);
    a.insert_domains("a-diverges", &[30]);
    b.insert_domains("b-diverges", &[31]);
    let tip = a.tip();
    assert_eq!(tip, b.tip());
    let from_a = a.request(0, tip);
    let from_b = b.request(0, tip);
    assert_ne!(
        a.store.read_complete_commits(&from_a, wide()).unwrap(),
        b.store.read_complete_commits(&from_b, wide()).unwrap()
    );
    assert_eq!(
        a.store.read_complete_commits(&from_b, wide()).unwrap_err(),
        CommitReadError::IncarnationMismatch
    );
}
