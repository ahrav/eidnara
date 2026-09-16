//! Measures the stock exhaustive API against a scaled kernel-backed corpus.

#![recursion_limit = "256"]

mod support;
#[path = "support/dense_performance_candidates.rs"]
mod candidates;
#[path = "support/dense_performance_guards.rs"]
mod guards;
#[path = "support/dense_performance_file_layer.rs"]
mod file_layer;

use std::collections::BTreeSet;
use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

use kernel::applicability::EvalBudget;
use kernel::{
    DomainSpec, EligibilityVerdict, KernelStore, MAX_ELIGIBILITY_CANDIDATES, ProjectScope,
};
use retrieval::batch::{
    MutationIdentity, ProjectionBatch, VectorGeneration, apply_batch, register_generation,
};
use retrieval::dense::codec::{self, Metric};
use retrieval::dense::oracle::exhaustive_with_hook_for_test;
use retrieval::dense::{
    Completion, DenseCoverage, ExhaustiveQuery, OracleBounds, Window, exhaustive,
};
use retrieval::{OccurrenceRecord, Payload, ProjectionIdentity, install_identity};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::dense::{
    DIGEST, DOMAIN, Fixture, GENERATION, HOLD, PROJECT_A, Row, SCOPE_A, TOLERANCE, admission,
    batch_bounds, decision, generation, intent, keyed, open_store, reference_over, scope,
};

struct Config {
    n: usize,
    dimension: u32,
    reps: usize,
    k: usize,
    page: usize,
    selectivity: usize,
    query_seed: usize,
}

fn setting(name: &str, default: usize) -> usize {
    match std::env::var(name) {
        Ok(value) => value
            .parse()
            .unwrap_or_else(|_| panic!("{name} must be an unsigned integer")),
        Err(std::env::VarError::NotPresent) => default,
        Err(error) => panic!("{name}: {error}"),
    }
}

impl Config {
    fn read() -> Self {
        let config = Self {
            n: setting("DENSE_N", 20_000),
            dimension: setting("DENSE_DIM", 384)
                .try_into()
                .expect("DENSE_DIM must fit u32"),
            reps: setting("DENSE_REPS", 5),
            k: setting("DENSE_K", 20),
            page: setting("DENSE_PAGE", 1024),
            selectivity: setting("DENSE_SELECTIVITY", 100),
            query_seed: setting("DENSE_QUERY", 1),
        };
        assert!(config.n > 0, "DENSE_N must be positive");
        assert!(config.dimension > 0, "DENSE_DIM must be positive");
        assert!(
            (1..=MAX_ELIGIBILITY_CANDIDATES).contains(&config.k),
            "invalid DENSE_K"
        );
        assert!(
            (1..=MAX_ELIGIBILITY_CANDIDATES).contains(&config.page),
            "invalid DENSE_PAGE"
        );
        assert!(
            config.selectivity <= 100,
            "DENSE_SELECTIVITY must be 0..=100"
        );
        config
    }

    fn admitted(&self) -> usize {
        (self.n as u128 * self.selectivity as u128 / 100)
            .try_into()
            .unwrap()
    }
}

fn seeded_unit(label: &[u8], seed: usize, dimension: u32) -> Vec<f32> {
    let mut raw = Vec::with_capacity(dimension as usize);
    for block in 0..dimension.div_ceil(8) {
        let mut hash = Sha256::new();
        hash.update(label);
        hash.update((seed as u64).to_le_bytes());
        hash.update(block.to_le_bytes());
        for word in hash.finalize().chunks_exact(4) {
            if raw.len() == dimension as usize {
                break;
            }
            raw.push(
                f64::from(u32::from_le_bytes(word.try_into().unwrap())) / f64::from(u32::MAX) * 2.0
                    - 1.0,
            );
        }
    }
    let norm = raw.iter().map(|value| value * value).sum::<f64>().sqrt();
    assert!(norm > 0.0);
    raw.into_iter().map(|value| (value / norm) as f32).collect()
}

fn resources() -> std::collections::BTreeMap<String, u64> {
    let mut values = std::collections::BTreeMap::new();
    let stat = std::fs::read_to_string("/proc/thread-self/stat").unwrap();
    let fields: Vec<_> = stat.rsplit_once(')').unwrap().1.split_whitespace().collect();
    values.insert("cpu_ticks".to_string(), fields[11].parse::<u64>().unwrap() + fields[12].parse::<u64>().unwrap());
    let io = std::fs::read_to_string("/proc/thread-self/io").unwrap();
    for line in io.lines() {
        let (key, value) = line.split_once(':').unwrap();
        values.insert(key.to_string(), value.trim().parse().unwrap());
    }
    values
}

fn resource_delta(before: std::collections::BTreeMap<String, u64>) -> Value {
    json!(resources().into_iter().map(|(key, value)| {
        let delta = value - before[&key];
        (key, delta)
    }).collect::<std::collections::BTreeMap<_, _>>())
}

/// Each bulk kernel commit fits one projection batch, so checkpoints never advertise a partial commit.
fn build_fixture(config: &Config, generation: &VectorGeneration) -> (Fixture, Value) {
    let started = Instant::now();
    let rows = (0..config.n)
        .map(|index| {
            Row::claim(
                &format!("probe-object-{index:010}"),
                seeded_unit(b"dense-probe-row-v1", index / 2, config.dimension),
            )
        })
        .collect();
    let vectors_ns = started.elapsed().as_nanos();
    let started = Instant::now();
    let root = tempfile::tempdir().unwrap();
    let kernel = KernelStore::open(root.path().join("kernel")).unwrap();
    kernel
        .commit(intent("probe-domain"), |envelope| {
            envelope.insert_domain(DomainSpec {
                domain_id: DOMAIN.to_string(),
                object_id: "domain-object".to_string(),
                name: "dense performance probe".to_string(),
                source_kind: "fixture".to_string(),
                source_id: DOMAIN.to_string(),
                source_revision: 1,
                sensitivity: kernel::Sensitivity::Normal,
            })?;
            envelope.insert_scope(scope(SCOPE_A, PROJECT_A))?;
            Ok(String::new())
        })
        .unwrap();
    let incarnation = kernel
        .database_incarnation_id_within_budget(&EvalBudget::unbounded())
        .unwrap();
    let kernel_open_seed_ns = started.elapsed().as_nanos();
    let started = Instant::now();
    let store = open_store(root.path());
    store
        .with_conn_fenced(|conn| {
            install_identity(
                conn,
                &ProjectionIdentity {
                    schema_version: retrieval::SCHEMA_VERSION,
                    kernel_incarnation_id: incarnation.clone(),
                    projection_policy_version: "source-policy.v1".to_string(),
                    identity_contract_version: "search-projection-identity-v3".to_string(),
                    limit_manifest_protocol_version: "limits.v1".to_string(),
                    embedding_model: generation.embedding_model.clone(),
                    tokenizer_fingerprint: generation.tokenizer_fingerprint.clone(),
                    analysis_identity: retrieval::lexical::AnalysisIdentity::current()
                        .as_str()
                        .to_string(),
                    vector_dimension: generation.vector_dimension,
                    generation_epoch: generation.generation_epoch,
                },
                1,
            )
            .unwrap();
            register_generation(conn, generation, 1).unwrap();
            Ok(())
        })
        .unwrap();
    let projection_open_ns = started.elapsed().as_nanos();
    let fixture = Fixture {
        root,
        kernel,
        store,
        incarnation,
        project: ProjectScope::new(PROJECT_A).unwrap(),
        rows,
    };
    let mut kernel_commits = Duration::ZERO;
    let mut projection_batches = Duration::ZERO;
    let chunk_rows = batch_bounds().persist.max_records.get();
    assert!(chunk_rows <= 4096);
    for (chunk_index, chunk) in fixture.rows.chunks(chunk_rows).enumerate() {
        let started = Instant::now();
        fixture
            .kernel
            .commit(intent(&format!("probe-chunk-{chunk_index}")), |envelope| {
                for (offset, row) in chunk.iter().enumerate() {
                    envelope.insert_decision(decision(&row.object))?;
                    if chunk_index * chunk_rows + offset < config.admitted() {
                        envelope.record_admission(admission(&row.object))?;
                    }
                }
                Ok(String::new())
            })
            .unwrap();
        let through = fixture.kernel.tip().unwrap();
        kernel_commits += started.elapsed();
        let started = Instant::now();
        let identities: Vec<_> = chunk.iter().map(Row::identity).collect();
        let records = chunk
            .iter()
            .zip(&identities)
            .map(|(row, identity)| OccurrenceRecord {
                occurrence: row.occurrence(identity),
                payload: Payload::Whole(&row.object),
                domain_id: DOMAIN,
                sensitivity: kernel::Sensitivity::Normal,
                source_object_id: &row.object,
                source_evidence_id: "evidence",
                source_artifact_digest: DIGEST,
                created_commit_seq: through,
            })
            .collect();
        let batch = ProjectionBatch {
            identity: MutationIdentity {
                kernel_incarnation_id: fixture.incarnation.clone(),
                hold_id: HOLD.to_string(),
                snapshot_commit_seq: 0,
                through_commit_seq: through,
            },
            records,
            invalidations: vec![],
            generation_id: Some(GENERATION),
        };
        fixture
            .store
            .with_conn_fenced(|conn| {
                let outcome = apply_batch(conn, &batch, batch_bounds(), through).unwrap();
                assert_eq!(outcome.rows_inserted, chunk.len());
                assert_eq!(outcome.pending_created, chunk.len());
                let mut insert = conn.prepare_cached("INSERT INTO occurrence_vectors(occurrence_id,generation_id,vector,vector_dimension,input_bytes,input_tokens,completed_at) VALUES (?1,?2,?3,?4,1,1,1)")?;
                let mut complete = conn.prepare_cached("UPDATE embedding_jobs SET state='embedded' WHERE occurrence_id=?1 AND generation_id=?2")?;
                for row in chunk {
                    let id = row.occurrence_id();
                    assert_eq!(insert.execute(rusqlite::params![id, GENERATION, codec::encode(row.vector.as_ref().unwrap()), config.dimension])?, 1);
                    assert_eq!(complete.execute(rusqlite::params![id, GENERATION])?, 1);
                }
                Ok(())
            })
            .unwrap();
        projection_batches += started.elapsed();
    }
    let started = Instant::now();
    let mut raw = fixture.raw();
    let through = fixture.kernel.tip().unwrap();
    let tx = raw.transaction().unwrap();
    for row in fixture.rows.iter().skip(19).step_by(20) {
        assert_eq!(tx.execute("INSERT INTO occurrence_tombstones(occurrence_id,invalidated_commit_seq,reason,recorded_at) VALUES (?1,?2,'retired',1)", rusqlite::params![row.occurrence_id(), through]).unwrap(), 1);
    }
    tx.commit().unwrap();
    let tombstones_ns = started.elapsed().as_nanos();
    let shape: (i64, i64, i64, i64) = raw.query_row(
        "SELECT (SELECT count(*) FROM occurrences), (SELECT count(DISTINCT source_object_id) FROM occurrences), (SELECT count(*) FROM occurrence_vectors), (SELECT count(*) FROM occurrence_tombstones)",
        [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).unwrap();
    assert_eq!(shape, (config.n as i64, config.n as i64, config.n as i64, (config.n / 20) as i64));
    let setup = json!({
        "vectors_ns": vectors_ns,
        "kernel_open_seed_ns": kernel_open_seed_ns,
        "projection_open_identity_ns": projection_open_ns,
        "kernel_bulk_commits_ns": kernel_commits.as_nanos(),
        "projection_batches_vectors_ns": projection_batches.as_nanos(),
        "tombstones_sql_ns": tombstones_ns,
        "projection_batches": config.n.div_ceil(chunk_rows),
        "kernel_commits": 1 + config.n.div_ceil(chunk_rows),
        "chunk_rows": chunk_rows,
        "projection_shape_checked": true,
        "journal_mode": raw.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0)).unwrap(),
        "sqlite_page_size": raw.query_row("PRAGMA page_size", [], |row| row.get::<_, i64>(0)).unwrap(),
        "projection_file_bytes": std::fs::metadata(fixture.root.path().join("search/search.sqlite")).unwrap().len(),
        "kernel_file_bytes": std::fs::metadata(fixture.root.path().join("kernel/kernel.sqlite")).unwrap().len(),
    });
    let fixture = if setting("DENSE_KERNEL_CACHE_PAGES", 0) != 0 {
        let Fixture { root, kernel, store, incarnation, project, rows } = fixture;
        drop(kernel);
        let raw = rusqlite::Connection::open(root.path().join("kernel/kernel.sqlite")).unwrap();
        let pages = setting("DENSE_KERNEL_CACHE_PAGES", 0) as i64;
        raw.pragma_update(None, "default_cache_size", pages).unwrap();
        drop(raw);
        let raw = rusqlite::Connection::open(root.path().join("kernel/kernel.sqlite")).unwrap();
        assert_eq!(raw.pragma_query_value(None,"cache_size", |row| row.get::<_,i64>(0)).unwrap(), pages);
        let kernel = KernelStore::open(root.path().join("kernel")).unwrap();
        Fixture { root, kernel, store, incarnation, project, rows }
    } else { fixture };
    if setting("DENSE_PROJECTION_CACHE_KIB", 0) != 0 {
        fixture.store.with_conn_unfenced(|conn| {
            conn.pragma_update(None,"cache_size", -(setting("DENSE_PROJECTION_CACHE_KIB",0) as i64))?;
            conn.pragma_update(None,"temp_store", "MEMORY")
        }).unwrap();
    }
    (fixture, setup)
}

#[test]
#[ignore = "research probe; creates a scaled kernel and projection"]
fn dense_performance_probe() {
    let config = Config::read();
    let setup_started = Instant::now();
    let generation = VectorGeneration {
        vector_dimension: config.dimension,
        ..generation()
    };
    let (fixture, mut setup) = build_fixture(&config, &generation);
    let reference_started = Instant::now();
    let query = seeded_unit(b"dense-probe-query-v1", config.query_seed, config.dimension);
    let query_norm = query
        .iter()
        .map(|value| f64::from(*value).powi(2))
        .sum::<f64>()
        .sqrt();
    assert!((query_norm - 1.0).abs() <= TOLERANCE);
    let all_ids: BTreeSet<_> = fixture.rows.iter().map(Row::occurrence_id).collect();
    assert_eq!(all_ids.len(), config.n);
    // These facts come from the seed, not from the oracle's SQL or kernel verdicts.
    let mut live_ids: Vec<[u8; 64]> = fixture
        .rows
        .iter()
        .enumerate()
        .filter(|(index, _)| index % 20 != 19)
        .map(|(_, row)| row.occurrence_id().as_bytes().try_into().unwrap())
        .collect();
    live_ids.sort();
    let mut expected = reference_over(
        &query,
        fixture
            .rows
            .iter()
            .enumerate()
            .filter(|(index, _)| *index < config.admitted() && index % 20 != 19)
            .map(|(_, row)| (row.occurrence_id(), row.vector.as_deref().unwrap())),
    );
    let eligible = expected.len();
    let excluded = live_ids.len() - eligible;
    let tied_neighbors = expected
        .windows(2)
        .filter(|pair| pair[0].1.to_bits() == pair[1].1.to_bits())
        .count();
    let score_range = expected
        .first()
        .zip(expected.last())
        .map(|(best, worst)| [worst.1, best.1]);
    expected.truncate(config.k);
    setup["query_reference_checks_ns"] = json!(reference_started.elapsed().as_nanos());
    setup["total_ns"] = json!(setup_started.elapsed().as_nanos());
    let git = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    assert!(git.status.success());
    let metadata = json!({
        "probe": "stock-exhaustive-v1", "git_head": String::from_utf8(git.stdout).unwrap().trim(),
        "manifest_dir": env!("CARGO_MANIFEST_DIR"), "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH, "debug_assertions": cfg!(debug_assertions),
        "build_rustflags": option_env!("CARGO_ENCODED_RUSTFLAGS"),
        "pid": std::process::id(), "sqlite_version": rusqlite::version(),
        "n": config.n, "dimension": config.dimension, "k": config.k, "page_rows": config.page,
        "warm_repetitions_per_mode": config.reps, "selectivity_percent": config.selectivity,
        "selectivity_rule": "first floor(N * percent / 100) objects; percent is over all N before tombstones",
        "admitted_objects": config.admitted(), "tombstones": config.n / 20,
        "tombstone_rule": "zero-based row_index % 20 == 19; floor(N / 20) rows; projection only",
        "live_required": live_ids.len(), "eligible": eligible, "hidden": excluded,
        "eligible_tied_neighbors": tied_neighbors, "eligible_score_range": score_range,
        "query_seed": config.query_seed, "query_norm_f64": query_norm,
        "query_sha256": format!("{:x}", Sha256::digest(codec::encode(&query))),
        "vector_seed": "SHA256(label || u64le(row_index/2) || u32le(block)); paired rows",
        "class": "CanonicalClaims", "scope": SCOPE_A, "source_digest": DIGEST,
        "metric": "inner_product", "encoding": "f32le", "unit_norm_tolerance": TOLERANCE,
        "budget": "unbounded", "max_rows": config.n,
        "cache_policy": "freshly created databases; no eviction; first stock then first hooked then alternating warm pair order",
        "wall_boundary": "SqliteStore::with_conn including transaction entry/exit; result drop excluded",
        "call_boundary": "exhaustive entry to return; setup, assertions, JSON and result drop excluded",
        "hook_overhead": "one Instant::now per Window; preallocated Vec push; 64-byte ID copy per Visited; no subtraction",
        "phase_limits": "first read includes validation; SQL includes framework and BLOB materialization; decode excludes last row; judgment includes last decode; score includes exclusion tally and cleanup; final includes top-K extraction, re-judgment and result construction",
        "setup": setup,
    });
    println!("{}", json!({"kind": "setup", "metadata": metadata}));
    let projection_pragmas = || fixture.store.with_conn(|conn| Ok(json!({
        "cache_size":conn.query_row("PRAGMA cache_size",[],|row|row.get::<_,i64>(0))?,
        "mmap_size":conn.query_row("PRAGMA mmap_size",[],|row|row.get::<_,i64>(0))?
    }))).unwrap();
    println!("{}", json!({"kind":"projection_pragmas","point":"before_queries","values":projection_pragmas()}));
    let request = ExhaustiveQuery {
        generation: &generation,
        metric: Metric::InnerProduct,
        unit_norm_tolerance: TOLERANCE,
        query: &query,
        authority: fixture.authority(),
        bounds: OracleBounds {
            k: NonZeroUsize::new(config.k).unwrap(),
            page_rows: NonZeroUsize::new(config.page).unwrap(),
            max_rows: NonZeroUsize::new(config.n).unwrap(),
        },
    };
    let budget = EvalBudget::unbounded();
    let page_count = live_ids.len().div_ceil(config.page);
    for repetition in 0..=config.reps {
        for (position, hooked) in (if repetition % 2 == 0 { [false, true] } else { [true, false] }).into_iter().enumerate() {
            let mut visits: Vec<(Instant, [u8; 64])> =
                Vec::with_capacity(if hooked { live_ids.len() } else { 0 });
            let mut judgments = Vec::with_capacity(if hooked { page_count } else { 0 });
            let mut pages = Vec::with_capacity(if hooked { page_count } else { 0 });
            let mut revalidations = Vec::with_capacity(1);
            let resource_start = resources();
            let wall_start = Instant::now();
            let (result, call_start, call_end) = fixture
                .store
                .with_conn(|conn| {
                    let call_start = Instant::now();
                    let result = if hooked {
                        exhaustive_with_hook_for_test(
                            conn,
                            &fixture.kernel,
                            &request,
                            &budget,
                            |window| {
                                let at = Instant::now();
                                match window {
                                    Window::Visited(id) => {
                                        visits.push((at, id.as_bytes().try_into().unwrap()))
                                    }
                                    Window::AfterJudgment => judgments.push(at),
                                    Window::AfterPage(count) => pages.push((at, count)),
                                    Window::BeforeRevalidation => revalidations.push(at),
                                }
                            },
                        )
                    } else {
                        exhaustive(conn, &fixture.kernel, &request, &budget)
                    };
                    let call_end = Instant::now();
                    Ok((result, call_start, call_end))
                })
                .unwrap();
            let wall_end = Instant::now();
            let resource_usage = resource_delta(resource_start);
            let ranking = result.unwrap();
            assert_eq!(ranking.completion, Completion::Complete);
            assert_eq!(keyed(&ranking), expected);
            for (actual, (_, score)) in ranking.ranked.iter().zip(&expected) {
                assert_eq!(actual.score.to_bits(), score.to_bits());
            }
            assert_eq!(
                ranking.coverage,
                DenseCoverage {
                    required: live_ids.len(),
                    with_vector: live_ids.len(),
                    missing_pending: 0,
                    missing_without_pending: 0,
                }
            );
            assert_eq!(ranking.consumed.pages, page_count);
            assert_eq!(ranking.consumed.judged, live_ids.len() + expected.len());
            assert_eq!(
                ranking.consumed.batches,
                page_count + usize::from(!expected.is_empty())
            );
            let expected_exclusions = if excluded == 0 {
                vec![]
            } else {
                vec![(EligibilityVerdict::Hidden, excluded)]
            };
            assert_eq!(ranking.consumed.excluded, expected_exclusions);
            let phases = if hooked {
                assert_eq!(
                    visits.iter().map(|(_, id)| *id).collect::<Vec<_>>(),
                    live_ids
                );
                assert_eq!(judgments.len(), page_count);
                assert_eq!(pages.len(), page_count);
                assert_eq!(revalidations.len(), 1);
                let mut sql = 0u128;
                let mut decode = 0u128;
                let mut judgment = 0u128;
                let mut score_select = 0u128;
                for (index, chunk) in visits.chunks(config.page).enumerate() {
                    assert_eq!(pages[index].1, index + 1);
                    let first = chunk.first().unwrap().0;
                    let last = chunk.last().unwrap().0;
                    if index != 0 {
                        sql += (first - pages[index - 1].0).as_nanos();
                    }
                    decode += (last - first).as_nanos();
                    judgment += (judgments[index] - last).as_nanos();
                    score_select += (pages[index].0 - judgments[index]).as_nanos();
                }
                let initial = (visits[0].0 - call_start).as_nanos();
                let tail = (revalidations[0] - pages.last().unwrap().0).as_nanos();
                let final_judge = (call_end - revalidations[0]).as_nanos();
                assert_eq!(
                    initial + sql + decode + judgment + score_select + tail + final_judge,
                    (call_end - call_start).as_nanos()
                );
                json!({
                    "initial_validation_first_sql_ns": initial,
                    "between_pages_sql_framework_ns": sql,
                    "decode_except_last_row_ns": decode,
                    "judgment_plus_last_decode_ns": judgment,
                    "score_select_cleanup_ns": score_select,
                    "walk_tail_ns": tail,
                    "final_revalidation_ns": final_judge,
                    "visited_events": visits.len(), "judgment_events": judgments.len(),
                    "page_events": pages.len(), "revalidation_events": revalidations.len(),
                    "window_clock_reads": visits.len() + judgments.len() + pages.len() + revalidations.len(),
                    "sql_intervals": page_count - 1,
                    "decode_rows_in_decode_interval": live_ids.len() - page_count,
                    "decode_rows_in_judgment_interval": page_count,
                    "final_judged_rows": expected.len(),
                    "partition_sum_checked": true,
                })
            } else {
                Value::Null
            };
            let returned: Vec<_> = ranking
                .ranked
                .iter()
                .map(|row| {
                    json!({
                        "occurrence_id": row.occurrence_id, "score": row.score,
                        "score_bits_hex": format!("{:016x}", row.score.to_bits()),
                    })
                })
                .collect();
            println!(
                "{}",
                json!({
                    "kind": "query", "metadata": metadata,
                    "mode": if hooked { "hooked" } else { "stock" },
                    "observation": match (repetition, hooked) {
                        (0, false) => "first_stock_call",
                        (0, true) => "first_hooked_call_after_stock",
                        _ => "warm",
                    },
                    "repetition": repetition, "call_ordinal": repetition * 2 + position,
                    "wall_ns": (wall_end - wall_start).as_nanos(),
                    "call_ns": (call_end - call_start).as_nanos(),
                    "wrapper_entry_ns": (call_start - wall_start).as_nanos(),
                    "wrapper_exit_ns": (wall_end - call_end).as_nanos(),
                    "boundary_clock_reads": 4, "phases": phases,
                    "resources": resource_usage,
                    "completion": "Complete", "exact_ids_and_score_bits_checked": true,
                    "visited_ids_exactly_once_checked": hooked,
                    "coverage": {"required": ranking.coverage.required, "with_vector": ranking.coverage.with_vector, "missing_pending": ranking.coverage.missing_pending, "missing_without_pending": ranking.coverage.missing_without_pending},
                    "consumed": {"pages": ranking.consumed.pages, "judged": ranking.consumed.judged, "batches": ranking.consumed.batches, "hidden": excluded},
                    "ranked": returned,
                })
            );
        }
    }
    if setting("DENSE_CANDIDATES", 0) != 0 {
        let started = Instant::now();
        let resident = candidates::build_resident(&fixture.store, &fixture.kernel, &request);
        println!("{}", json!({"kind":"resident_build", "wall_ns":started.elapsed().as_nanos(),
            "column_bytes":resident.column_bytes,"work":resident.build_measurements}));
        let file = if setting("DENSE_FILE_LAYER",0) != 0 {
            let started = Instant::now();
            let file = file_layer::build(&fixture,&request);
            println!("{}", json!({"kind":"file_build","wall_ns":started.elapsed().as_nanos(),
                "file_bytes":file.file_bytes,"resident_id_bytes":file.resident_id_bytes,"work":file.build_measurements}));
            Some(file)
        } else {None};
        for repetition in 0..=config.reps {
            let mut modes = vec!["stock_control", "page_score_first", "shortlist_sql", "resident_shortlist"];
            if file.is_some() {modes.extend(["file_layer_stock","file_layer_page"]);}
            if repetition % 2 != 0 { modes.reverse(); }
            for (position, mode) in modes.into_iter().enumerate() {
                let resource_start = resources();
                let started = Instant::now();
                let result = match mode {
                    "page_score_first" => candidates::page_score_first(&fixture.store, &fixture.kernel, &request),
                    "shortlist_sql" => candidates::shortlist_sql(&fixture.store, &fixture.kernel, &request),
                    "resident_shortlist" => candidates::resident_shortlist(&resident, &fixture.kernel, &request),
                    "file_layer_stock" => file_layer::baseline(file.as_ref().unwrap(),&fixture,&request),
                    "file_layer_page" => file_layer::optimized(file.as_ref().unwrap(),&fixture,&request),
                    "stock_control" => {
                        let ranking = fixture.store.with_conn(|conn| Ok(exhaustive(conn, &fixture.kernel, &request, &budget))).unwrap().unwrap();
                        assert_eq!(ranking.completion, Completion::Complete);
                        candidates::ProbeResult { ranked: ranking.ranked, measurements:json!({"judged":ranking.consumed.judged,"batches":ranking.consumed.batches}) }
                    }
                    _ => unreachable!(),
                };
                let elapsed = started.elapsed().as_nanos();
                let resource_usage = resource_delta(resource_start);
                let actual: Vec<_> = result.ranked.iter().map(|r| (r.occurrence_id.clone(),r.score)).collect();
                assert_eq!(actual, expected, "{mode}");
                for (actual, (_, score)) in result.ranked.iter().zip(&expected) { assert_eq!(actual.score.to_bits(), score.to_bits(), "{mode}"); }
                println!("{}",json!({"kind":"query","metadata":metadata,"mode":mode,
                    "observation":if repetition==0 {"first_candidate_call"} else {"warm"},
                    "repetition":repetition,"call_ordinal":position,"wall_ns":elapsed,
                    "work":result.measurements,"resources":resource_usage,
                    "exact_ids_and_score_bits_checked":true,"completion":"frozen_fixture_exact_top_k",
                    "ranked":actual}));
            }
        }
        if setting("DENSE_CONCURRENT",0) != 0 {
            for repetition in 0..config.reps {
                for mode in if repetition % 2 == 0 { ["stock","page","resident"] } else { ["resident","page","stock"] } {
                    let barrier = std::sync::Barrier::new(3);
                    let walls = std::thread::scope(|scope| {
                        let handles:Vec<_> = (0..2).map(|_| scope.spawn(|| {
                            barrier.wait();
                            let started=Instant::now();
                            let ranked=match mode {
                                "page"=>candidates::page_score_first(&fixture.store,&fixture.kernel,&request).ranked,
                                "resident"=>candidates::resident_shortlist(&resident,&fixture.kernel,&request).ranked,
                                _=>fixture.store.with_conn(|conn|Ok(exhaustive(conn,&fixture.kernel,&request,&budget))).unwrap().unwrap().ranked,
                            };
                            let elapsed=started.elapsed().as_nanos();
                            assert_eq!(ranked.iter().map(|r|(r.occurrence_id.clone(),r.score)).collect::<Vec<_>>(),expected);
                            elapsed
                        })).collect();
                        barrier.wait();
                        handles.into_iter().map(|h|h.join().unwrap()).collect::<Vec<_>>()
                    });
                    println!("{}",json!({"kind":"concurrent_pair","mode":mode,"repetition":repetition,"wall_ns":walls,
                        "shared_projection_connection":true,"exact_results_checked":true}));
                }
            }
        }
    }
    println!("{}", json!({"kind":"projection_pragmas","point":"after_queries","values":projection_pragmas()}));
}
