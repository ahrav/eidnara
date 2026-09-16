//! Research measurements are descriptive, not performance gates or capacity estimates.
//! SQL-only samples use a standalone connection; end-to-end samples use `Fixture::retrieve`.

use super::*;
use retrieval::lexical::{probe_engine, verify_rows};
use retrieval::{QueryRow, Tombstone, TombstoneReason, tombstone_occurrence};
use rusqlite::{Statement, StatementStatus, params};
use serde_json::{Value, json};
use std::sync::{Barrier, mpsc};

const LATE_JOIN: &str =
    "SELECT l.occurrence_id,l.rank,o.class,o.source_object_id,o.revision,o.source_artifact_digest
    FROM (SELECT occurrence_id,rank FROM lexical WHERE lexical MATCH ?1
          ORDER BY rank,occurrence_id LIMIT ?2) l
    JOIN occurrences o ON o.occurrence_id=l.occurrence_id
    WHERE NOT EXISTS(SELECT 1 FROM occurrence_tombstones t WHERE t.occurrence_id=o.occurrence_id)
    ORDER BY l.rank,l.occurrence_id";
const STREAM_RANK: &str =
    "SELECT l.occurrence_id,l.rank,o.class,o.source_object_id,o.revision,o.source_artifact_digest
    FROM (SELECT occurrence_id,rank FROM lexical WHERE lexical MATCH ?1
          ORDER BY rank LIMIT -1 OFFSET 0) l
    JOIN occurrences o ON o.occurrence_id=l.occurrence_id
    WHERE NOT EXISTS(SELECT 1 FROM occurrence_tombstones t WHERE t.occurrence_id=o.occurrence_id)
    ORDER BY l.rank,l.occurrence_id LIMIT ?2";
const FTS_RANK_ID: &str = "SELECT occurrence_id,rank FROM lexical WHERE lexical MATCH ?1
    ORDER BY rank,occurrence_id LIMIT ?2";
const FTS_RANK: &str = "SELECT occurrence_id,rank FROM lexical WHERE lexical MATCH ?1
    ORDER BY rank LIMIT ?2";
const SQL_CASES: [(&str, &str); 6] = [
    ("common", "parse"),
    ("medium", "medium"),
    ("unique", "uniqueneedle"),
    ("phrase", "x\u{0305}y"),
    ("missing", "missingneedle"),
    ("ties", "tiegroup"),
];
const LIMITS: [i64; 3] = [2, 65, 1025];
const DF_SKIP_REPETITIONS: usize = 8;
const DF_CASES: [(&str, &[&str]); 2] = [("parse", &["parse"]), ("parse io", &["parse", "io"])];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Corpus {
    Diverse,
    Tied,
}

impl Corpus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Diverse => "diverse",
            Self::Tied => "tied",
        }
    }

    fn text(self, n: usize) -> String {
        match self {
            Self::Tied => "parse tiegroup".to_string(),
            Self::Diverse => {
                let mut text = if n.is_multiple_of(4) {
                    "parse tiegroup".to_string()
                } else {
                    format!("parse {}", "padding ".repeat((n % 480) * 17 % 480))
                };
                if n.is_multiple_of(100) {
                    text.push_str(" medium");
                }
                if n == 0 {
                    text.push_str(" uniqueneedle");
                }
                if n == 1 {
                    text.push_str(" x\u{0305}y");
                }
                text
            }
        }
    }
}

fn production_sql() -> &'static str {
    let source = include_str!("../../src/lexical/retrieve.rs");
    let marker = "const PROBE_SQL: &str =";
    assert_eq!(source.matches(marker).count(), 1, "ambiguous PROBE_SQL");
    let (_, rest) = source.split_once(marker).unwrap();
    let rest = rest
        .trim_start()
        .strip_prefix('"')
        .expect("quoted SQL constant");
    let (sql, suffix) = rest.split_once('"').expect("quoted SQL constant");
    assert!(
        suffix.starts_with(';'),
        "expected an ordinary string literal"
    );
    assert!(
        !sql.contains('\\'),
        "SQL extraction does not decode Rust escapes"
    );
    sql
}

type FullRow = (String, u64, String, String, i64, String);
type IdRank = (String, u64);

#[derive(Debug, PartialEq, Eq)]
enum SqlRows {
    Full(Vec<FullRow>),
    IdRank(Vec<IdRank>),
}

impl SqlRows {
    fn pairs(&self) -> Vec<IdRank> {
        match self {
            Self::Full(rows) => rows.iter().map(|row| (row.0.clone(), row.1)).collect(),
            Self::IdRank(rows) => rows.clone(),
        }
    }

    fn json(&self) -> Value {
        match self {
            Self::Full(rows) => json!(rows),
            Self::IdRank(rows) => json!(rows),
        }
    }
}

fn decode_rows(statement: &mut Statement<'_>, probe: &Probe, limit: i64) -> SqlRows {
    match statement.column_count() {
        6 => SqlRows::Full(
            statement
                .query_map(params![probe, limit], |row| {
                    Ok((
                        row.get(0)?,
                        row.get::<_, f64>(1)?.to_bits(),
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                })
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap(),
        ),
        2 => SqlRows::IdRank(
            statement
                .query_map(params![probe, limit], |row| {
                    Ok((row.get(0)?, row.get::<_, f64>(1)?.to_bits()))
                })
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap(),
        ),
        columns => panic!("unexpected probe column count: {columns}"),
    }
}

fn sql_rows(conn: &Connection, sql: &str, probe: &Probe, limit: i64) -> SqlRows {
    decode_rows(&mut conn.prepare_cached(sql).unwrap(), probe, limit)
}

fn explain_bytecode(conn: &Connection, sql: &str, probe: &Probe, limit: i64) -> Vec<Value> {
    conn.prepare(&format!("EXPLAIN {sql}"))
        .unwrap()
        .query_map(params![probe, limit], |row| {
            Ok(json!({
                "address": row.get::<_, i64>(0)?,
                "opcode": row.get::<_, String>(1)?,
                "p1": row.get::<_, i64>(2)?,
                "p2": row.get::<_, i64>(3)?,
                "p3": row.get::<_, i64>(4)?,
                "p4": row.get::<_, Option<String>>(5)?,
                "p5": row.get::<_, i64>(6)?,
                "comment": row.get::<_, Option<String>>(7)?,
            }))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn emit(mut record: Value) {
    record["schema"] = json!("lexical_probe_research.v1");
    record["label"] = json!(std::env::var("LEXICAL_RESEARCH_LABEL").unwrap_or_default());
    record["production_variant"] = json!(std::env::var("LEXICAL_RESEARCH_VARIANT").ok());
    record["pid"] = json!(std::process::id());
    println!("RESEARCH_JSON {record}");
}

fn project_research_rows(fixture: &Fixture, count: usize, corpus: Corpus) {
    for first in (0..count).step_by(256) {
        let rows: Vec<Row> = (first..count.min(first.saturating_add(256)))
            .map(|n| {
                let text = corpus.text(n);
                assert!(text.len() <= 4096);
                Row::claim(&format!("research-{n}"), &text)
            })
            .collect();
        fixture.project(&rows);
    }
}

fn assert_equivalent(fixture: &Fixture, requests: &[&str]) {
    let raw = fixture.raw();
    verify_rows(&raw).unwrap();
    for request in requests {
        for probe in probes(request) {
            let mut reference = fixture
                .store
                .with_conn(|conn| Ok(ranks(conn, &probe)))
                .unwrap();
            reference.sort_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
            for limit in LIMITS {
                let baseline = sql_rows(&raw, production_sql(), &probe, limit);
                let late = sql_rows(&raw, LATE_JOIN, &probe, limit);
                assert_eq!(baseline, late, "{request:?}, limit {limit}");
                assert_eq!(
                    baseline,
                    sql_rows(&raw, STREAM_RANK, &probe, limit),
                    "rank_stream: {request:?}, limit {limit}"
                );
                let expected: Vec<_> = reference
                    .iter()
                    .take(usize::try_from(limit).unwrap())
                    .map(|(id, rank)| (id.clone(), rank.to_bits()))
                    .collect();
                assert_eq!(baseline.pairs(), expected);
                emit(json!({
                    "kind": "correctness", "query": request, "probe": format!("{probe:?}"),
                    "limit": limit, "six_columns_bitwise_equal": true,
                    "variants": ["baseline", "late_join", "rank_stream"],
                    "rows": baseline.json(),
                }));
            }
        }
    }
}

#[test]
fn sql_variants_agree_on_original_fixture() {
    let fixture = Fixture::all_admitted();
    assert_eq!(fixture.rows.len(), 6);
    let raw_metadata = sqlite_metadata(&fixture.raw());
    let guarded_metadata = fixture
        .store
        .with_conn(|conn| Ok(sqlite_metadata(conn)))
        .unwrap();
    assert_eq!(raw_metadata["source_id"], guarded_metadata["source_id"]);
    assert!(!explain_bytecode(&fixture.raw(), STREAM_RANK, &probes("parse")[0], 65).is_empty());
    assert_eq!(
        LIMITS[1],
        i64::try_from(bounds().scan_rows.get() + 1).unwrap()
    );
    assert_equivalent(
        &fixture,
        &[
            "parse",
            "io",
            "parse fetch io",
            "HTTPServer",
            "medium",
            "uniqueneedle",
            "x\u{0305}y",
            "missingneedle",
        ],
    );
}

#[test]
fn sql_variants_agree_on_both_projected_corpora() {
    for (corpus, medium, unique, phrase, ties) in
        [(Corpus::Diverse, 3, 1, 1, 65), (Corpus::Tied, 0, 0, 0, 257)]
    {
        let fixture = Fixture::all_admitted();
        project_research_rows(&fixture, 257, corpus);
        let raw = fixture.raw();
        for (query, matches) in [
            ("parse", 261),
            ("medium", medium),
            ("uniqueneedle", unique),
            ("x\u{0305}y", phrase),
            ("tiegroup", ties),
            ("missingneedle", 0),
        ] {
            let compiled = probes(query);
            assert_eq!(compiled.len(), 1);
            assert_eq!(
                sql_rows(&raw, production_sql(), &compiled[0], 1025)
                    .pairs()
                    .len(),
                matches
            );
        }
        assert_equivalent(&fixture, &SQL_CASES.map(|(_, query)| query));
        let footprint = database_footprint(&fixture, &raw);
        assert!(footprint["logical_db_bytes"].as_i64().unwrap() > 0);
        assert!(
            footprint["lexical_shadow_dbstat"]["bytes"]
                .as_i64()
                .unwrap()
                > 0
        );
    }
}

#[test]
fn raw_tombstone_breaks_late_join_equivalence_at_limit() {
    let fixture = Fixture::all_admitted();
    let raw = fixture.raw();
    verify_rows(&raw).unwrap();
    let probe = &probes("parse")[0];
    let leader = sql_rows(&raw, production_sql(), probe, 2).pairs()[0]
        .0
        .clone();
    raw.execute(
        "INSERT INTO occurrence_tombstones(occurrence_id,invalidated_commit_seq,reason,recorded_at)
         VALUES (?1,99,'retired',0)",
        [&leader],
    )
    .unwrap();
    assert_eq!(
        verify_rows(&raw),
        Err(retrieval::ProjectionError::CorruptRow)
    );
    let baseline = sql_rows(&raw, production_sql(), probe, 2);
    let late = sql_rows(&raw, LATE_JOIN, probe, 2);
    let stream = sql_rows(&raw, STREAM_RANK, probe, 2);
    assert_eq!(baseline.pairs().len(), 2);
    assert_eq!(late.pairs().len(), 1);
    assert_ne!(baseline, late);
    assert_eq!(baseline, stream);
    assert!(baseline.pairs().iter().all(|(id, _)| *id != leader));
    emit(json!({
        "kind": "raw_tombstone_counterexample", "limit": 2,
        "verify_rows": "CorruptRow", "baseline": baseline.json(), "late_join": late.json(),
        "rank_stream": stream.json(),
    }));
}

#[test]
fn production_tombstone_preserves_late_join_equivalence() {
    let fixture = Fixture::all_admitted();
    let raw = fixture.raw();
    verify_rows(&raw).unwrap();
    let probe = &probes("parse")[0];
    let leader = sql_rows(&raw, production_sql(), probe, 2).pairs()[0]
        .0
        .clone();
    fixture
        .store
        .with_conn_fenced(|conn| {
            let result = tombstone_occurrence(
                conn,
                &leader,
                Tombstone {
                    invalidated_commit_seq: 99,
                    reason: TombstoneReason::Retired,
                },
                99,
            )
            .unwrap();
            assert!(result.recorded);
            assert_eq!(result.lexical_rows_deleted, 1);
            verify_rows(conn).unwrap();
            Ok(())
        })
        .unwrap();
    assert_eq!(sql_rows(&raw, production_sql(), probe, 2).pairs().len(), 2);
    assert_equivalent(
        &fixture,
        &[
            "parse",
            "io",
            "parse fetch io",
            "HTTPServer",
            "missingneedle",
        ],
    );
}

#[test]
fn rank_only_changes_membership_at_a_valid_tied_cutoff() {
    let fixture = Fixture::all_admitted();
    let tied: Vec<_> = (0..64)
        .map(|n| Row::claim(&format!("tied-{n}"), "parse tiedneedle"))
        .collect();
    fixture.project(&tied);
    let raw = fixture.raw();
    verify_rows(&raw).unwrap();
    let probe = &probes("tiedneedle")[0];
    let ordered = sql_rows(&raw, FTS_RANK_ID, probe, 65).pairs();
    let rank_only = sql_rows(&raw, FTS_RANK, probe, 65).pairs();
    assert_eq!(ordered.len(), 64);
    assert!(
        ordered
            .windows(2)
            .all(|pair| pair[0].1 == pair[1].1 && pair[0].0 < pair[1].0)
    );
    let mut same_set = rank_only.clone();
    same_set.sort();
    assert_eq!(same_set, ordered);
    assert_ne!(
        rank_only, ordered,
        "FTS rowid order is not occurrence-id order"
    );
    let (cutoff, ordered_cutoff, rank_only_cutoff) = (1..64)
        .find_map(|limit| {
            let ordered = sql_rows(&raw, FTS_RANK_ID, probe, limit).pairs();
            let mut rank_only = sql_rows(&raw, FTS_RANK, probe, limit).pairs();
            rank_only.sort();
            (rank_only != ordered).then_some((limit, ordered, rank_only))
        })
        .expect("a tied cutoff changes membership, not only order");
    assert_eq!(ordered_cutoff.len(), usize::try_from(cutoff).unwrap());
    assert_eq!(rank_only_cutoff.len(), ordered_cutoff.len());
    assert_ne!(rank_only_cutoff, ordered_cutoff);
    assert_eq!(
        sql_rows(&raw, production_sql(), probe, 65),
        sql_rows(&raw, LATE_JOIN, probe, 65)
    );
    assert_eq!(
        sql_rows(&raw, production_sql(), probe, 65),
        sql_rows(&raw, STREAM_RANK, probe, 65)
    );
    emit(json!({
        "kind": "rank_only_counterexample", "verify_rows": "ok",
        "ordered": ordered, "rank_only": rank_only, "equivalent": false,
        "cutoff": cutoff, "ordered_cutoff": ordered_cutoff,
        "rank_only_cutoff_sorted": rank_only_cutoff, "cutoff_membership_equal": false,
    }));
}

fn sqlite_metadata(conn: &impl QueryRow) -> Value {
    let engine = probe_engine(conn).unwrap();
    let options: Vec<String> = conn
        .prepare("PRAGMA compile_options")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let mut pragmas = serde_json::Map::new();
    for name in [
        "journal_mode",
        "synchronous",
        "page_size",
        "cache_size",
        "temp_store",
        "mmap_size",
        "foreign_keys",
        "query_only",
        "automatic_index",
        "busy_timeout",
    ] {
        let value = conn
            .query_row(&format!("PRAGMA {name}"), [], |row| {
                row.get::<_, rusqlite::types::Value>(0)
            })
            .unwrap();
        pragmas.insert(
            name.to_string(),
            match value {
                rusqlite::types::Value::Integer(value) => json!(value),
                rusqlite::types::Value::Text(value) => json!(value),
                value => panic!("unexpected pragma {name}: {value:?}"),
            },
        );
    }
    json!({"version": engine.sqlite_version, "source_id": engine.sqlite_source_id, "compile_options": options, "pragmas": pragmas})
}

fn process_cpu_ticks() -> Option<(u64, u64)> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let (_, fields) = stat.rsplit_once(") ")?;
    let mut fields = fields.split_whitespace().skip(11);
    Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
}

fn process_io() -> Option<Value> {
    let text = std::fs::read_to_string("/proc/self/io").ok()?;
    let mut counters = serde_json::Map::new();
    for line in text.lines() {
        let (name, value) = line.split_once(':')?;
        if ["read_bytes", "syscr", "rchar"].contains(&name) {
            counters.insert(name.to_string(), json!(value.trim().parse::<u64>().ok()?));
        }
    }
    (counters.len() == 3).then_some(Value::Object(counters))
}

fn database_footprint(fixture: &Fixture, conn: &Connection) -> Value {
    let page_count: i64 = conn
        .query_row("PRAGMA page_count", [], |row| row.get(0))
        .unwrap();
    let page_size: i64 = conn
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .unwrap();
    let lexical = conn.query_row(
        "SELECT count(*), coalesce(sum(pgsize),0) FROM dbstat
         WHERE name IN ('lexical_content','lexical_data','lexical_idx','lexical_docsize','lexical_config')",
        [],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    );
    let lexical = match lexical {
        Ok((pages, bytes)) => json!({"pages": pages, "bytes": bytes}),
        Err(error) => json!({"unavailable": error.to_string()}),
    };
    let directory = fixture.root.path().join("search");
    json!({
        "page_count": page_count, "page_size": page_size,
        "logical_db_bytes": page_count.checked_mul(page_size).unwrap(),
        "main_file_bytes": std::fs::metadata(directory.join("search.sqlite")).ok().map(|file| file.len()),
        "wal_file_bytes": std::fs::metadata(directory.join("search.sqlite-wal")).ok().map(|file| file.len()),
        "lexical_shadow_dbstat": lexical,
    })
}

fn normalized_retrieval(result: &Retrieval) -> Value {
    json!({
        "contributions": result.contributions.iter().map(|row| json!({
            "occurrence_id": row.occurrence_id, "rank_bits": row.rank.to_bits(),
            "class": row.class.code(), "ordinal": row.ordinal,
        })).collect::<Vec<_>>(),
        "completion": format!("{:?}", result.completion),
        "consumed": {
            "probes": result.consumed.probes, "scanned_rows": result.consumed.scanned_rows,
            "judged": result.consumed.judged, "batches": result.consumed.batches,
            "excluded": result.consumed.excluded.iter().map(|(verdict, count)| (format!("{verdict:?}"), count)).collect::<Vec<_>>(),
        },
        "accepted": result.contributions.len(),
        "excluded": result.consumed.excluded.iter().map(|(_, count)| count).sum::<usize>(),
    })
}

type NamedProbes = Vec<(&'static str, Probe)>;

fn df_requests() -> [(&'static str, NamedProbes); 2] {
    DF_CASES.map(|(query, terms)| {
        let compiled = probes(query);
        assert_eq!(compiled.len(), terms.len());
        (query, terms.iter().copied().zip(compiled).collect())
    })
}

struct DfSkipPolicy<'a> {
    fixture: &'a Fixture,
    vocab: Connection,
    total_rows: i64,
    bounds: RetrievalBounds,
}

impl<'a> DfSkipPolicy<'a> {
    fn new(fixture: &'a Fixture) -> Self {
        let vocab = fixture.raw();
        verify_rows(&vocab).unwrap();
        vocab.execute_batch(
            "CREATE VIRTUAL TABLE temp.lexical_research_df USING fts5vocab('main','lexical','row')",
        ).unwrap();
        let total_rows = vocab
            .query_row("SELECT count(*) FROM lexical", [], |row| row.get(0))
            .unwrap();
        Self {
            fixture,
            vocab,
            total_rows,
            bounds: RetrievalBounds {
                batch_rows: NonZeroUsize::new(64).unwrap(),
                ..bounds()
            },
        }
    }

    fn retrieve(&self, request: &NamedProbes) -> (Retrieval, Vec<(&'static str, i64)>) {
        let mut selected = Vec::new();
        let mut frequencies = Vec::new();
        for (term, probe) in request {
            assert!(matches!(*term, "parse" | "io"));
            let df: i64 = self
                .vocab
                .prepare_cached(
                    "SELECT coalesce((SELECT doc FROM temp.lexical_research_df WHERE term=?1),0)",
                )
                .unwrap()
                .query_row([term], |row| row.get(0))
                .unwrap();
            frequencies.push((*term, df));
            if df <= self.total_rows / 10 {
                selected.push(probe.clone());
            }
        }
        let result = self
            .fixture
            .retrieve(&selected, self.bounds, &EvalBudget::unbounded())
            .unwrap();
        (result, frequencies)
    }
}

fn retained_reference_contributions(reference: &Retrieval, actual: &Retrieval) -> usize {
    reference
        .contributions
        .iter()
        .filter(|expected| {
            actual.contributions.iter().any(|row| {
                row.occurrence_id == expected.occurrence_id
                    && row.rank.to_bits() == expected.rank.to_bits()
            })
        })
        .count()
}

#[test]
fn df_skip_loses_common_hits_and_keeps_a_term_at_the_threshold() {
    for (corpus, count) in [
        (Corpus::Diverse, 0),
        (Corpus::Diverse, 4),
        (Corpus::Diverse, 257),
        (Corpus::Tied, 257),
    ] {
        let fixture = Fixture::all_admitted();
        project_research_rows(&fixture, count, corpus);
        let policy = DfSkipPolicy::new(&fixture);
        for (query, request) in df_requests() {
            let compiled: Vec<_> = request.iter().map(|(_, probe)| probe.clone()).collect();
            let reference = fixture
                .retrieve(&compiled, policy.bounds, &EvalBudget::unbounded())
                .unwrap();
            let (actual, frequencies) = policy.retrieve(&request);
            assert_eq!(frequencies[0], ("parse", i64::try_from(count + 4).unwrap()));
            assert!(!reference.contributions.is_empty());
            let retained = retained_reference_contributions(&reference, &actual);
            if query == "parse io" && policy.total_rows / 10 >= 1 {
                assert_eq!(frequencies[1], ("io", 1));
                assert_eq!(ids_of(&actual), vec![fixture.id("delta")]);
                assert_eq!(actual.completion, Completion::Complete);
                assert_eq!(actual.consumed.probes, 1);
                assert_eq!(retained, 1);
                let mut changed_rank = actual.clone();
                let rank = &mut changed_rank.contributions[0].rank;
                *rank = f64::from_bits(rank.to_bits() ^ 1);
                assert_eq!(
                    retained_reference_contributions(&reference, &changed_rank),
                    0
                );
            } else {
                assert!(actual.contributions.is_empty());
                assert_eq!(actual.completion, Completion::Empty);
                assert_eq!(actual.consumed.probes, 0);
                assert_eq!(retained, 0);
            }
            assert!(retained < reference.contributions.len());
            emit(json!({
                "kind": "df_skip_correctness", "query": query, "corpus_mode": corpus.as_str(),
                "synthetic_rows": count, "total_rows": policy.total_rows,
                "document_frequencies": frequencies, "threshold": policy.total_rows / 10,
                "retained_reference_contributions": retained,
                "total_reference_contributions": reference.contributions.len(),
                "reference": normalized_retrieval(&reference), "result": normalized_retrieval(&actual),
            }));
        }
        verify_rows(&fixture.raw()).unwrap();
    }
}

fn run_df_policy(fixture: &Fixture, fixture_name: &str) {
    let policy = DfSkipPolicy::new(fixture);
    for (query, request) in df_requests() {
        let compiled: Vec<_> = request.iter().map(|(_, probe)| probe.clone()).collect();
        let reference = fixture
            .retrieve(&compiled, policy.bounds, &EvalBudget::unbounded())
            .unwrap();
        std::hint::black_box(policy.retrieve(&request));
        for round in 0..DF_SKIP_REPETITIONS {
            let start = Instant::now();
            let (actual, frequencies) = policy.retrieve(&request);
            let elapsed = start.elapsed();
            let retained = retained_reference_contributions(&reference, &actual);
            if query == "parse" {
                assert!(frequencies[0].1 > policy.total_rows / 10);
                assert!(!reference.contributions.is_empty());
                assert!(actual.contributions.is_empty());
                assert_eq!(actual.completion, Completion::Empty);
                assert_eq!(retained, 0);
            }
            let skipped: Vec<_> = frequencies
                .iter()
                .filter_map(|(term, df)| (*df > policy.total_rows / 10).then_some(*term))
                .collect();
            emit(json!({
                "kind": "df_skip_sample", "fixture": fixture_name, "query": query,
                "round": round, "total_rows": policy.total_rows, "threshold": policy.total_rows / 10,
                "document_frequencies": frequencies, "skipped_terms": skipped,
                "scan_rows": 64, "max_accepted": 64, "batch_rows": 64, "cache_state": "warm",
                "elapsed_ns": u64::try_from(elapsed.as_nanos()).unwrap(),
                "retained_reference_contributions": retained,
                "total_reference_contributions": reference.contributions.len(),
                "reference": normalized_retrieval(&reference), "result": normalized_retrieval(&actual),
                "quality_scope": "retention relative to exact bounded retrieval by occurrence ID and rank bits; no qrels or relevance claim",
            }));
        }
    }
}

fn run_e2e(fixture: &Fixture, fixture_name: &str, repetitions: usize) {
    let total_rows: i64 = fixture
        .raw()
        .query_row("SELECT count(*) FROM lexical", [], |row| row.get(0))
        .unwrap();
    let limits = RetrievalBounds {
        batch_rows: NonZeroUsize::new(64).unwrap(),
        ..bounds()
    };
    for query in ["parse", "io", "parse fetch io", "HTTPServer"] {
        let request = probes(query);
        let budget = EvalBudget::unbounded();
        let expected = normalized_retrieval(&fixture.retrieve(&request, limits, &budget).unwrap());
        for round in 0..repetitions {
            let cpu_before = process_cpu_ticks();
            let start = Instant::now();
            let result = fixture.retrieve(&request, limits, &budget);
            let elapsed = start.elapsed();
            let cpu_after = process_cpu_ticks();
            let result = normalized_retrieval(&result.unwrap());
            assert_eq!(result, expected);
            emit(json!({
                "kind": "e2e_sample", "fixture": fixture_name, "total_rows": total_rows,
                "query": query, "round": round, "readers": 1, "cache_state": "warm",
                "scan_rows": 64, "max_accepted": 64, "batch_rows": 64,
                "elapsed_ns": u64::try_from(elapsed.as_nanos()).unwrap(),
                "cpu_ticks_before": cpu_before, "cpu_ticks_after": cpu_after, "result": result,
            }));

            let ready = Barrier::new(5);
            let start_gate = Barrier::new(5);
            let (samples, elapsed, cpu_before, cpu_after) = std::thread::scope(|scope| {
                let ready = &ready;
                let start_gate = &start_gate;
                let (send, receive) = mpsc::channel();
                let request = &request;
                for reader in 0..4 {
                    let send = send.clone();
                    scope.spawn(move || {
                        let budget = EvalBudget::unbounded();
                        ready.wait();
                        start_gate.wait();
                        let start = Instant::now();
                        let result = fixture.retrieve(request, limits, &budget);
                        let elapsed = start.elapsed();
                        send.send((reader, result, elapsed)).unwrap();
                    });
                }
                drop(send);
                ready.wait();
                let cpu_before = process_cpu_ticks();
                let start = Instant::now();
                start_gate.wait();
                let samples: Vec<_> = receive.into_iter().collect();
                let elapsed = start.elapsed();
                let cpu_after = process_cpu_ticks();
                (samples, elapsed, cpu_before, cpu_after)
            });
            assert_eq!(samples.len(), 4);
            let mut samples: Vec<_> = samples.into_iter().map(|(reader, result, elapsed)| {
                let result = normalized_retrieval(&result.unwrap());
                assert_eq!(result, expected);
                (reader, json!({"reader": reader, "elapsed_ns": u64::try_from(elapsed.as_nanos()).unwrap(), "result": result}))
            }).collect();
            samples.sort_by_key(|(reader, _)| *reader);
            emit(json!({
                "kind": "e2e_sample", "fixture": fixture_name, "total_rows": total_rows,
                "query": query, "round": round, "readers": 4, "cache_state": "warm",
                "scan_rows": 64, "max_accepted": 64, "batch_rows": 64,
                "elapsed_ns": u64::try_from(elapsed.as_nanos()).unwrap(),
                "cpu_ticks_before": cpu_before, "cpu_ticks_after": cpu_after,
                "requests": samples.into_iter().map(|(_, sample)| sample).collect::<Vec<_>>(),
            }));
        }
    }
}

#[test]
#[ignore = "research timing; run only after recording the experiment protocol"]
fn probe_cost_experiment() {
    if let Ok(expected) = std::env::var("LEXICAL_RESEARCH_EXPECTED_SQL_SHA256") {
        assert_eq!(
            format!("{:x}", Sha256::digest(production_sql().as_bytes())),
            expected,
            "compiled SQL differs from the intended treatment"
        );
    }
    let corpus = match std::env::var("LEXICAL_RESEARCH_CORPUS")
        .as_deref()
        .unwrap_or("diverse")
    {
        "diverse" => Corpus::Diverse,
        "tied" => Corpus::Tied,
        value => panic!("LEXICAL_RESEARCH_CORPUS must be diverse or tied, got {value:?}"),
    };
    let count = std::env::var("LEXICAL_RESEARCH_ROWS").map_or(20_000, |value| {
        value
            .parse::<usize>()
            .expect("LEXICAL_RESEARCH_ROWS must be an integer")
    });
    let repetitions = std::env::var("LEXICAL_RESEARCH_REPS").map_or(8, |value| {
        value
            .parse::<usize>()
            .expect("LEXICAL_RESEARCH_REPS must be an integer")
    });
    assert!(count >= 2, "LEXICAL_RESEARCH_ROWS must be at least 2");
    assert!(
        repetitions >= 2 && repetitions.is_multiple_of(2),
        "LEXICAL_RESEARCH_REPS must be positive and even"
    );
    let variants = [
        ("baseline", production_sql()),
        ("late_join", LATE_JOIN),
        ("rank_stream", STREAM_RANK),
        ("fts_rank_id", FTS_RANK_ID),
        ("fts_rank", FTS_RANK),
    ];
    let small = Fixture::all_admitted();
    let large = Fixture::all_admitted();
    project_research_rows(&large, count, corpus);
    let raw = large.raw();
    verify_rows(&small.raw()).unwrap();
    verify_rows(&raw).unwrap();
    let total_rows: i64 = raw
        .query_row("SELECT count(*) FROM lexical", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        usize::try_from(total_rows).unwrap(),
        count.checked_add(6).unwrap()
    );
    let guarded_metadata = large
        .store
        .with_conn(|conn| Ok(sqlite_metadata(conn)))
        .unwrap();
    emit(json!({
        "kind": "protocol", "synthetic_rows": count, "total_rows": total_rows, "repetitions": repetitions,
        "corpus_mode": corpus.as_str(),
        "database_footprint": {"original": database_footprint(&small, &small.raw()), "large": database_footprint(&large, &raw)},
        "footprint_scope": "logical SQLite page bytes and file lengths, not resident memory or physical allocation accounting; dbstat aggregates lexical shadow tables",
        "variants": variants.map(|(name, _)| name),
        "sqlite_raw": sqlite_metadata(&raw), "sqlite_guarded": guarded_metadata,
        "production_sql_sha256": format!("{:x}", Sha256::digest(production_sql().as_bytes())),
        "sql_timing": "prepare_cached + bind + iterate + decode; excludes connection setup, statement drop, status reads, verification, compilation, EQP/bytecode and output",
        "cache_reset_diagnostic": "cache-reset-requested: flush_prepared_statement_cache + PRAGMA shrink_memory; OS-warm, not OS-cold; does not prove all SQLite pages are evicted",
        "e2e_timing": "Fixture::retrieve including with_conn mutex/transaction, merge, kernel eligibility and revalidation; excludes compile/setup/output",
        "df_skip_policy": "literal parse/io terms only; skip when fts5vocab row doc > total_rows/10; not a generalized query compiler rewrite",
        "df_skip_repetitions": DF_SKIP_REPETITIONS,
        "df_skip_timing": "DF lookups + selecting compiled probes + Fixture::retrieve; excludes vocab setup, query compilation, reference retrieval, quality comparison and output; one unmeasured warmup",
        "concurrent_timing": "four closed readers, one request each, shared Fixture/store; total includes start barrier, completion channel and scheduling, excludes spawn and join",
        "admission": "only original six objects admitted; synthetic objects have no kernel decision or admission",
        "cpu_ticks": "process-wide /proc/self/stat utime,stime clock ticks; sampled outside wall-clock interval; null when unavailable",
        "sql_io": "process-wide /proc/self/io snapshots outside timing; read_bytes is storage-layer read accounting, rchar/syscr include cached reads and observer reads; not exact per-query physical I/O or allocation accounting; null when unavailable",
        "order": "one reset-cache diagnostic per variant, one unmeasured warm run, then alternating forward/reverse variant order each round per case/limit",
        "claim": "raw descriptive observations only; no significance, production speedup or open-loop capacity claim",
    }));
    for (case, query) in SQL_CASES {
        let compiled = probes(query);
        assert_eq!(compiled.len(), 1);
        let probe = &compiled[0];
        let matches: i64 = raw
            .query_row(
                "SELECT count(*) FROM lexical WHERE lexical MATCH ?1",
                [probe],
                |row| row.get(0),
            )
            .unwrap();
        for limit in LIMITS {
            let baseline = sql_rows(&raw, production_sql(), probe, limit);
            assert_eq!(baseline, sql_rows(&raw, LATE_JOIN, probe, limit));
            assert_eq!(baseline, sql_rows(&raw, STREAM_RANK, probe, limit));
            assert_eq!(
                baseline.pairs(),
                sql_rows(&raw, FTS_RANK_ID, probe, limit).pairs()
            );
            for (variant, sql) in variants {
                let plan: Vec<(i64, i64, i64, String)> = raw
                    .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                    .unwrap()
                    .query_map(params![probe, limit], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                    })
                    .unwrap()
                    .collect::<rusqlite::Result<_>>()
                    .unwrap();
                emit(json!({
                    "kind": "sql_plan", "fixture": corpus.as_str(), "case": case, "query": query,
                    "variant": variant, "limit": limit, "total_rows": total_rows, "matching_rows": matches,
                    "sql": sql, "sql_sha256": format!("{:x}", Sha256::digest(sql.as_bytes())), "eqp": plan,
                }));
                if case == "common" && limit == 65 {
                    emit(json!({
                        "kind": "sql_bytecode", "fixture": corpus.as_str(), "case": case,
                        "query": query, "variant": variant, "limit": limit,
                        "total_rows": total_rows, "matching_rows": matches,
                        "sql_sha256": format!("{:x}", Sha256::digest(sql.as_bytes())),
                        "bytecode": explain_bytecode(&raw, sql, probe, limit),
                    }));
                }
            }
            // A reset-cache sample is a separate diagnostic, not a warm repetition.
            for round in 0..=repetitions {
                let mut order = variants;
                if round > 0 && round.is_multiple_of(2) {
                    order.reverse();
                }
                for (position, (variant, sql)) in order.into_iter().enumerate() {
                    if round == 0 {
                        raw.flush_prepared_statement_cache();
                        raw.execute_batch("PRAGMA shrink_memory").unwrap();
                    }
                    let cpu_before = process_cpu_ticks();
                    let io_before = process_io();
                    let start = Instant::now();
                    let mut statement = raw.prepare_cached(sql).unwrap();
                    let rows = decode_rows(&mut statement, probe, limit);
                    let elapsed = start.elapsed();
                    let io_after = process_io();
                    let cpu_after = process_cpu_ticks();
                    let steps = statement.reset_status(StatementStatus::VmStep);
                    let sorts = statement.reset_status(StatementStatus::Sort);
                    drop(statement);
                    let pairs = rows.pairs();
                    let ordered_equal = pairs == baseline.pairs();
                    match variant {
                        "baseline" | "late_join" | "rank_stream" => assert_eq!(rows, baseline),
                        "fts_rank_id" => assert!(ordered_equal),
                        "fts_rank" => assert!(pairs.windows(2).all(|pair| f64::from_bits(
                            pair[0].1
                        ) <= f64::from_bits(
                            pair[1].1
                        ))),
                        _ => unreachable!(),
                    }
                    emit(json!({
                        "kind": "sql_sample", "fixture": corpus.as_str(), "case": case, "query": query,
                        "variant": variant, "limit": limit, "synthetic_rows": count, "total_rows": total_rows,
                        "matching_rows": matches, "returned_rows": pairs.len(), "position": position,
                        "round": round.checked_sub(1), "cache_state": if round == 0 { "cache-reset-requested" } else { "warm" },
                        "elapsed_ns": u64::try_from(elapsed.as_nanos()).unwrap(), "vm_steps": steps, "sorts": sorts,
                        "cpu_ticks_before": cpu_before, "cpu_ticks_after": cpu_after,
                        "io_before": io_before, "io_after": io_after,
                        "result_sha256": format!("{:x}", Sha256::digest(serde_json::to_vec(&rows.json()).unwrap())),
                        "ordered_id_rank_equal_to_baseline": ordered_equal,
                        "comparison": match variant { "fts_rank" => "non_equivalent_rank_only", "fts_rank_id" => "id_rank_projection_only", _ => "six_columns_bitwise" },
                    }));
                }
                if round == 0 {
                    for (_, sql) in variants {
                        let mut statement = raw.prepare_cached(sql).unwrap();
                        std::hint::black_box(decode_rows(&mut statement, probe, limit));
                        statement.reset_status(StatementStatus::VmStep);
                        statement.reset_status(StatementStatus::Sort);
                    }
                }
            }
        }
    }
    run_e2e(&small, "original", repetitions);
    run_e2e(&large, corpus.as_str(), repetitions);
    run_df_policy(&small, "original");
    run_df_policy(&large, corpus.as_str());
    verify_rows(&raw).unwrap();
    emit(json!({"kind": "complete", "verify_rows": "ok"}));
}
