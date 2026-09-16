use std::num::NonZeroUsize;
use std::sync::LazyLock;
use std::time::Instant;

use kernel::applicability::EvalBudget;
use kernel::source_identity::OccurrenceClass;
use kernel::{CommitReadIncarnation, EgressSnapshot, KernelStore, MAX_ELIGIBILITY_CANDIDATES};
use retrieval::batch::{VectorGeneration, dense_eligible, read_checkpoint};
use retrieval::dense::codec::{self, RowLayout};
use retrieval::dense::score::{Ranked, TopK, rank_order, score};
use retrieval::dense::{ExhaustiveQuery, RowAccess, Winner};
use retrieval::eligibility::{Disposition, EligibilityReport, OccurrenceCandidate, judge_tracked};
use rusqlite::params;
use serde_json::{Value, json};
use storage::{GuardedConn, SqliteStore};

static DENSE_CLASSES: LazyLock<String> = LazyLock::new(|| {
    OccurrenceClass::ALL
        .into_iter()
        .filter(|class| dense_eligible(*class))
        .map(|class| format!("'{}'", class.code()))
        .collect::<Vec<_>>()
        .join(",")
});

const FULL_COLUMNS: &str =
    "o.occurrence_id,o.class,o.source_object_id,o.revision,o.source_artifact_digest,v.vector,
                CASE WHEN v.vector IS NULL THEN EXISTS(SELECT 1 FROM embedding_jobs j
    WHERE j.occurrence_id=o.occurrence_id AND j.generation_id=?1 AND j.stop_reason IS NULL
      AND (j.state='pending' OR (j.state='admitted' AND j.host_job_id IS NOT NULL))) ELSE 0 END";

fn page_sql(columns: &str) -> String {
    format!(
        "SELECT {columns}
         FROM occurrences o
         LEFT JOIN occurrence_tombstones t ON t.occurrence_id=o.occurrence_id
         LEFT JOIN occurrence_vectors v ON v.occurrence_id=o.occurrence_id AND v.generation_id=?1
         WHERE t.occurrence_id IS NULL AND +o.class IN ({}) AND o.occurrence_id>?2
         ORDER BY o.occurrence_id
         LIMIT ?3",
        *DENSE_CLASSES
    )
}

static PAGE_SQL: LazyLock<String> = LazyLock::new(|| page_sql(FULL_COLUMNS));
static NARROW_SQL: LazyLock<String> = LazyLock::new(|| page_sql("o.occurrence_id,v.vector"));

static LAYER_PAGE_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT o.occurrence_id,o.class,o.source_object_id,o.revision,o.source_artifact_digest,NULL,
                EXISTS(SELECT 1 FROM embedding_jobs j
    WHERE j.occurrence_id=o.occurrence_id AND j.generation_id=?1 AND j.stop_reason IS NULL
      AND (j.state='pending' OR (j.state='admitted' AND j.host_job_id IS NOT NULL)))
         FROM occurrences o
         LEFT JOIN occurrence_tombstones t ON t.occurrence_id=o.occurrence_id
         WHERE t.occurrence_id IS NULL AND +o.class IN ({}) AND o.occurrence_id>?2
         ORDER BY o.occurrence_id
         LIMIT ?3",
        *DENSE_CLASSES
    )
});

/// The resolved variant is a single static base. Its iterator merges winners with live SQL rows.
pub(super) enum PageSource<'a> {
    Stored,
    Resolved {
        winners: std::slice::Iter<'a, Winner<'a>>,
        rows: &'a dyn RowAccess,
    },
}

impl PageSource<'_> {
    fn page_sql(&self) -> &str {
        match self {
            Self::Stored => &PAGE_SQL,
            Self::Resolved { .. } => &LAYER_PAGE_SQL,
        }
    }

    fn vector(&mut self, id: &str, stored: Option<Vec<u8>>, layout: &RowLayout) -> Vec<f32> {
        match self {
            Self::Stored => codec::decode(&stored.expect("missing vector"), layout)
                .expect("corrupt stored vector"),
            Self::Resolved { winners, rows } => {
                let winner = winners
                    .find(|winner| winner.occurrence_id.as_bytes() >= id.as_bytes())
                    .expect("missing resolved vector");
                assert_eq!(winner.occurrence_id, id, "missing resolved vector");
                assert_eq!(winner.layer, 0, "prototype requires a single base");
                let vector = rows.row(winner.row).expect("unreadable resolved vector");
                codec::validate(&vector, layout).expect("corrupt resolved vector");
                vector
            }
        }
    }
}

pub(super) struct ProbeResult {
    pub ranked: Vec<Ranked>,
    pub measurements: Value,
}

#[derive(Default)]
struct Work {
    passes: usize,
    scanned: usize,
    decoded: usize,
    scored: usize,
    pages: usize,
    page_queries: usize,
    lookahead_rows: usize,
    metadata_lookups: usize,
    judged: usize,
    batches: usize,
    empty_snapshot_batches: usize,
    scan_ns: u128,
    judgment_ns: u128,
    metadata_ns: u128,
    final_revalidation_ns: u128,
    snapshot: Option<EgressSnapshot>,
    incarnation: Option<CommitReadIncarnation>,
}

impl Work {
    fn measurements(&self) -> Value {
        json!({
            "passes": self.passes, "scanned": self.scanned, "decoded": self.decoded,
            "scored": self.scored, "pages": self.pages, "page_queries": self.page_queries,
            "lookahead_rows": self.lookahead_rows, "metadata_lookups": self.metadata_lookups,
            "judged": self.judged, "batches": self.batches,
            "empty_snapshot_batches": self.empty_snapshot_batches,
            "scan_ns": self.scan_ns, "judgment_ns": self.judgment_ns,
            "metadata_ns": self.metadata_ns, "final_revalidation_ns": self.final_revalidation_ns,
        })
    }

    fn judge(
        &mut self,
        kernel: &KernelStore,
        query: &ExhaustiveQuery<'_>,
        rows: &[OccurrenceCandidate],
    ) -> EligibilityReport {
        assert!(rows.len() <= MAX_ELIGIBILITY_CANDIDATES);
        let start = Instant::now();
        let (report, moved) = judge_tracked(
            kernel,
            query.authority,
            rows,
            &EvalBudget::unbounded(),
            &mut self.snapshot,
            &mut self.incarnation,
        )
        .expect("kernel judgment refused");
        self.judgment_ns += start.elapsed().as_nanos();
        self.judged += rows.len();
        self.batches += 1;
        self.empty_snapshot_batches += usize::from(rows.is_empty());
        assert_eq!(moved, None, "kernel snapshot moved");
        assert_eq!(report.occurrences.len(), rows.len());
        report
    }

    fn finish(
        &mut self,
        kernel: &KernelStore,
        query: &ExhaustiveQuery<'_>,
        top: TopK<OccurrenceCandidate>,
    ) -> Vec<Ranked> {
        let start = Instant::now();
        let (ranked, candidates): (Vec<_>, Vec<_>) = top.into_ranked().into_iter().unzip();
        let report = self.judge(kernel, query, &candidates);
        assert!(
            report
                .occurrences
                .iter()
                .all(|row| row.disposition == Disposition::Eligible),
            "finalist eligibility changed"
        );
        self.final_revalidation_ns = start.elapsed().as_nanos();
        ranked
    }
}

pub(super) fn projection_version(conn: &GuardedConn<'_>) -> (i64, u64) {
    (
        conn.query_row("PRAGMA data_version", [], |row| row.get(0))
            .unwrap(),
        conn.total_changes(),
    )
}

fn start(
    conn: &GuardedConn<'_>,
    kernel: &KernelStore,
    query: &ExhaustiveQuery<'_>,
    work: &mut Work,
) -> RowLayout {
    assert!(query.bounds.k.get() <= MAX_ELIGIBILITY_CANDIDATES);
    assert!(query.bounds.page_rows.get() <= MAX_ELIGIBILITY_CANDIDATES);
    let layout = RowLayout {
        dimension: query.generation.vector_dimension,
        metric: query.metric,
        unit_norm_tolerance: query.unit_norm_tolerance,
    };
    assert!(layout.dimension > 0);
    codec::validate(query.query, &layout).expect("invalid query");
    let g = query.generation;
    let valid: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM vector_generations
        WHERE generation_id=?1 AND embedding_model=?2 AND tokenizer_fingerprint=?3
          AND vector_dimension=?4 AND generation_epoch=?5 AND state IN ('building','verified','selected'))",
        params![g.generation_id, g.embedding_model, g.tokenizer_fingerprint, g.vector_dimension,
            i64::try_from(g.generation_epoch).unwrap()], |row| row.get(0)).unwrap();
    assert!(valid, "missing, retired, or mismatched generation");
    let mut other_class = conn
        .prepare_cached("SELECT EXISTS(SELECT 1 FROM occurrences WHERE class=?1)")
        .unwrap();
    for class in OccurrenceClass::ALL
        .into_iter()
        .filter(|class| dense_eligible(*class) && *class != OccurrenceClass::CanonicalClaims)
    {
        let present: bool = other_class
            .query_row([class.code()], |row| row.get(0))
            .unwrap();
        assert!(!present, "prototype requires canonical-only dense fixture");
    }
    work.judge(kernel, query, &[]);
    let database = kernel
        .database_incarnation_id_within_budget(&EvalBudget::unbounded())
        .unwrap();
    let checkpoint = read_checkpoint(conn, &database)
        .unwrap()
        .expect("missing projection checkpoint");
    assert_eq!(
        checkpoint.checkpoint_commit_seq,
        work.snapshot.unwrap().tip,
        "projection is not caught up"
    );
    layout
}

fn result(
    store: &SqliteStore,
    version: (i64, u64),
    ranked: Vec<Ranked>,
    work: Work,
) -> ProbeResult {
    assert_eq!(
        store
            .with_conn(|conn| Ok(projection_version(conn)))
            .unwrap(),
        version,
        "projection changed"
    );
    ProbeResult {
        ranked,
        measurements: work.measurements(),
    }
}

pub(super) fn candidate(row: &rusqlite::Row<'_>) -> rusqlite::Result<OccurrenceCandidate> {
    let class =
        OccurrenceClass::from_code(&row.get::<_, String>(1)?).expect("invalid occurrence class");
    Ok(OccurrenceCandidate::new(
        row.get(0)?,
        class,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
    ))
}

pub(super) fn page_score_first(
    store: &SqliteStore,
    kernel: &KernelStore,
    query: &ExhaustiveQuery<'_>,
) -> ProbeResult {
    page_score_first_from(store, kernel, query, PageSource::Stored)
}

pub(super) fn page_score_first_from(
    store: &SqliteStore,
    kernel: &KernelStore,
    query: &ExhaustiveQuery<'_>,
    mut source: PageSource<'_>,
) -> ProbeResult {
    let mut work = Work::default();
    let (ranked, version) = store
        .with_conn(|conn| {
            let layout = start(conn, kernel, query, &mut work);
            let version = projection_version(conn);
            work.passes = 1;
            let mut top: TopK<OccurrenceCandidate> = TopK::new(query.bounds.k);
            let mut after = String::new();
            loop {
                let scan_start = Instant::now();
                let take = query
                    .bounds
                    .page_rows
                    .get()
                    .min(query.bounds.max_rows.get() - work.scanned);
                let mut stmt = conn.prepare_cached(source.page_sql())?;
                let mut rows = stmt.query(params![
                    query.generation.generation_id,
                    after,
                    (take + 1) as i64
                ])?;
                work.page_queries += 1;
                let mut page = Vec::with_capacity(take);
                let mut more = false;
                while let Some(row) = rows.next()? {
                    if page.len() == take {
                        more = true;
                        work.lookahead_rows += 1;
                        break;
                    }
                    let pending: bool = row.get(6)?;
                    let bytes: Option<Vec<u8>> = row.get(5)?;
                    assert!(
                        !matches!(source, PageSource::Stored) || bytes.is_some(),
                        "missing vector (pending={pending})"
                    );
                    page.push((candidate(row)?, bytes));
                }
                drop(rows);
                drop(stmt);
                let retained = top.into_ranked();
                let threshold = if retained.len() == query.bounds.k.get() {
                    retained.last().map(|(r, _)| r.clone())
                } else {
                    None
                };
                top = TopK::new(query.bounds.k);
                for (ranked, row) in retained {
                    top.offer(ranked, row);
                }
                let mut competitive = Vec::new();
                if !page.is_empty() {
                    work.pages += 1;
                }
                for (row, bytes) in page {
                    after.clone_from(&row.occurrence_id);
                    let vector = source.vector(&row.occurrence_id, bytes, &layout);
                    let value = score(query.metric, query.query, &vector);
                    work.scanned += 1;
                    work.decoded += 1;
                    work.scored += 1;
                    if threshold.as_ref().is_none_or(|t| {
                        rank_order((value, &row.occurrence_id), (t.score, &t.occurrence_id)).is_lt()
                    }) {
                        competitive.push((
                            Ranked {
                                occurrence_id: row.occurrence_id.clone(),
                                class: row.class,
                                score: value,
                            },
                            row,
                        ));
                    }
                }
                work.scan_ns += scan_start.elapsed().as_nanos();
                if !competitive.is_empty() {
                    let (ranked, candidates): (Vec<_>, Vec<_>) = competitive.into_iter().unzip();
                    let report = work.judge(kernel, query, &candidates);
                    for ((ranked, row), verdict) in
                        ranked.into_iter().zip(candidates).zip(report.occurrences)
                    {
                        if verdict.disposition == Disposition::Eligible {
                            top.offer(ranked, row);
                        }
                    }
                }
                if !more {
                    break;
                }
                assert!(
                    work.scanned < query.bounds.max_rows.get(),
                    "row bound exhausted"
                );
            }
            Ok((work.finish(kernel, query, top), version))
        })
        .unwrap();
    result(store, version, ranked, work)
}

fn scan_sql(
    conn: &GuardedConn<'_>,
    query: &ExhaustiveQuery<'_>,
    layout: &RowLayout,
    work: &mut Work,
    mut visit: impl FnMut(&str, &[f32]),
) {
    let mut after = String::new();
    let mut count = 0;
    loop {
        let mut stmt = conn.prepare_cached(&NARROW_SQL).unwrap();
        let mut rows = stmt
            .query(params![
                query.generation.generation_id,
                after,
                query.bounds.page_rows.get() as i64
            ])
            .unwrap();
        work.page_queries += 1;
        let mut page_count = 0;
        while let Some(row) = rows.next().unwrap() {
            count += 1;
            assert!(count <= query.bounds.max_rows.get(), "row bound exhausted");
            after = row.get(0).unwrap();
            let bytes: Option<Vec<u8>> = row.get(1).unwrap();
            let vector = codec::decode(&bytes.expect("missing vector"), layout)
                .expect("corrupt stored vector");
            visit(&after, &vector);
            work.scanned += 1;
            work.decoded += 1;
            page_count += 1;
        }
        work.pages += usize::from(page_count != 0);
        if page_count < query.bounds.page_rows.get() {
            break;
        }
    }
}

fn metadata(
    conn: &GuardedConn<'_>,
    ranked: &[Ranked],
    work: &mut Work,
) -> Vec<OccurrenceCandidate> {
    let start = Instant::now();
    let mut stmt = conn
        .prepare_cached(
            "SELECT o.occurrence_id,o.class,o.source_object_id,o.revision,o.source_artifact_digest
        FROM occurrences o LEFT JOIN occurrence_tombstones t ON t.occurrence_id=o.occurrence_id
        WHERE o.occurrence_id=?1 AND t.occurrence_id IS NULL",
        )
        .unwrap();
    let rows = ranked
        .iter()
        .map(|ranked| {
            let row = stmt
                .query_row([&ranked.occurrence_id], candidate)
                .expect("missing or tombstoned finalist");
            assert_eq!(row.class, ranked.class);
            row
        })
        .collect();
    work.metadata_lookups += ranked.len();
    work.metadata_ns += start.elapsed().as_nanos();
    rows
}

pub(super) struct Resident<'a> {
    store: &'a SqliteStore,
    version: (i64, u64),
    generation: VectorGeneration,
    layout: RowLayout,
    snapshot: EgressSnapshot,
    incarnation: CommitReadIncarnation,
    ids: Vec<String>,
    vectors: Vec<f32>,
    pub column_bytes: usize,
    pub build_measurements: Value,
}

pub(super) fn build_resident<'a>(
    store: &'a SqliteStore,
    kernel: &KernelStore,
    query: &ExhaustiveQuery<'_>,
) -> Resident<'a> {
    let mut work = Work::default();
    let mut resident = store
        .with_conn(|conn| {
            let layout = start(conn, kernel, query, &mut work);
            let version = projection_version(conn);
            let mut ids = Vec::new();
            let mut vectors = Vec::new();
            work.passes = 1;
            let started = Instant::now();
            scan_sql(conn, query, &layout, &mut work, |id, row| {
                ids.push(id.to_owned());
                vectors.extend_from_slice(row);
            });
            work.scan_ns = started.elapsed().as_nanos();
            work.judge(kernel, query, &[]);
            let column_bytes = vectors.capacity() * size_of::<f32>()
                + ids.capacity() * size_of::<String>()
                + ids.iter().map(String::capacity).sum::<usize>();
            Ok(Resident {
                store,
                version,
                generation: query.generation.clone(),
                layout,
                snapshot: work.snapshot.unwrap(),
                incarnation: work.incarnation.unwrap(),
                ids,
                vectors,
                column_bytes,
                build_measurements: Value::Null,
            })
        })
        .unwrap();
    assert_eq!(
        store
            .with_conn(|conn| Ok(projection_version(conn)))
            .unwrap(),
        resident.version,
        "projection changed during build"
    );
    resident.build_measurements = work.measurements();
    resident
}

enum Source<'a, 's> {
    Sql(&'s SqliteStore),
    Resident(&'a Resident<'s>),
}

pub(super) fn shortlist_sql(
    store: &SqliteStore,
    kernel: &KernelStore,
    query: &ExhaustiveQuery<'_>,
) -> ProbeResult {
    shortlist(Source::Sql(store), kernel, query)
}

pub(super) fn resident_shortlist(
    resident: &Resident<'_>,
    kernel: &KernelStore,
    query: &ExhaustiveQuery<'_>,
) -> ProbeResult {
    shortlist(Source::Resident(resident), kernel, query)
}

fn shortlist(
    source: Source<'_, '_>,
    kernel: &KernelStore,
    query: &ExhaustiveQuery<'_>,
) -> ProbeResult {
    let store = match source {
        Source::Sql(store) => store,
        Source::Resident(rows) => rows.store,
    };
    let mut work = Work::default();
    let (ranked, version) = store
        .with_conn(|conn| {
            let layout = start(conn, kernel, query, &mut work);
            let version = projection_version(conn);
            if let Source::Resident(rows) = &source {
                assert_eq!(version, rows.version, "resident projection is stale");
                assert_eq!(*query.generation, rows.generation);
                assert_eq!(layout, rows.layout);
                assert_eq!(
                    work.snapshot,
                    Some(rows.snapshot),
                    "resident kernel snapshot is stale"
                );
                assert_eq!(
                    work.incarnation,
                    Some(rows.incarnation),
                    "resident kernel incarnation is stale"
                );
                assert!(
                    rows.ids.len() <= query.bounds.max_rows.get(),
                    "row bound exhausted"
                );
            }
            let batch = (2 * query.bounds.k.get()).clamp(64, MAX_ELIGIBILITY_CANDIDATES);
            let mut top = TopK::new(query.bounds.k);
            let mut accepted = 0;
            let mut after: Option<Ranked> = None;
            loop {
                work.passes += 1;
                let scan_start = Instant::now();
                let mut shortlist = TopK::new(NonZeroUsize::new(batch).unwrap());
                let mut scored = 0;
                let mut offer = |id: &str, vector: &[f32]| {
                    let value = score(query.metric, query.query, vector);
                    scored += 1;
                    if after.as_ref().is_none_or(|last| {
                        rank_order((value, id), (last.score, &last.occurrence_id)).is_gt()
                    }) {
                        shortlist.offer(
                            Ranked {
                                occurrence_id: id.to_owned(),
                                class: OccurrenceClass::CanonicalClaims,
                                score: value,
                            },
                            (),
                        );
                    }
                };
                match &source {
                    Source::Sql(_) => scan_sql(conn, query, &layout, &mut work, offer),
                    Source::Resident(rows) => {
                        for (id, vector) in rows
                            .ids
                            .iter()
                            .zip(rows.vectors.chunks_exact(layout.dimension as usize))
                        {
                            offer(id, vector);
                        }
                        work.scanned += rows.ids.len();
                    }
                }
                work.scored += scored;
                let ranked: Vec<_> = shortlist
                    .into_ranked()
                    .into_iter()
                    .map(|(r, ())| r)
                    .collect();
                work.scan_ns += scan_start.elapsed().as_nanos();
                if ranked.is_empty() {
                    break;
                }
                let exhausted = ranked.len() < batch;
                after = ranked.last().cloned();
                let candidates = metadata(conn, &ranked, &mut work);
                let report = work.judge(kernel, query, &candidates);
                for ((ranked, candidate), judged) in
                    ranked.into_iter().zip(candidates).zip(report.occurrences)
                {
                    if judged.disposition == Disposition::Eligible {
                        top.offer(ranked, candidate);
                        accepted += 1;
                    }
                }
                if accepted >= query.bounds.k.get() || exhausted {
                    break;
                }
            }
            Ok((work.finish(kernel, query, top), version))
        })
        .unwrap();
    result(store, version, ranked, work)
}
