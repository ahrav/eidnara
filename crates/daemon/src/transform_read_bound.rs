//! The per-pass read inventory (spec D15, property WP-P07): every statement a transform pass
//! or a history_summarizer prepare and publish runs is recorded by the store's statement-work
//! ledger, classified into one inventory row by its SQL text, and checked against that row's
//! bound while the stored history, overlays, and tags grow. An unclassified statement fails
//! the test, so a new full read cannot land unnoticed.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use memory_store::{
    HistorySegmentSetGeneration, HistorySummarizerChunkRange, HistorySummarizerDurableState,
    HistorySummarizerPhase, HistorySummarizerPublishPredicate, HistorySummarizerPublishRequest,
    HistorySummarizerSelectedMessageIdentity, MemoryStore, StoredHistorySegment,
};
use storage::StatementWork;

use crate::test_support::synthetic_history::{
    SyntheticHistory, seed_active_summarizer, seed_overlays,
};
use crate::transform::tests::{active_cc_req, item, pctx, store};
use crate::transform::{
    TransformRequest, install_transform_attempt_hook, transform_with_projection_cached,
};

const SESSION: &str = "read-bound";
/// Live messages after the covered pair the request repeats.
const TAIL: u64 = 8;
/// Largest non-archived index under any pressure (WP-P08).
const MAX_K: u64 = 2_484;

/// The D15 inventory rows, in report order.
const ROWS: &[&str] = &[
    "coverage snapshot",
    "append range validation",
    "m0 segments",
    "m1 segments",
    "summarizer assembly",
    "publication set fence",
    "temporal marks, hints, appends",
    "tags",
    "active user memories",
    "notes status version",
    "kernel project memory",
    "pass trace and ledgers",
    "scan ledger",
];

/// Where a statement no inventory row names is counted; any such statement fails the test.
const UNCLASSIFIED: &str = "unclassified";

/// Inventory rows by SQL text fragment; the first matching fragment wins, so the specific
/// history_segments shapes come before the generic ones.
const CLASSES: &[(&str, &str)] = &[
    ("INSERT INTO history_segments", "append range validation"),
    ("COALESCE(MAX(sequence), 0) + 1", "append range validation"),
    ("chunk_transcripts", "append range validation"),
    ("legacy <> 1", "m0 segments"),
    ("legacy = 1", "m0 segments"),
    (
        "history_segments WHERE session_id = ?1 AND sequence > ?2",
        "m1 segments",
    ),
    (
        "history_segments WHERE session_id = ?1 ORDER BY sequence DESC LIMIT ?2",
        "summarizer assembly",
    ),
    (
        "SELECT MAX(end_message) FROM history_segments",
        "summarizer assembly",
    ),
    ("SELECT meta FROM cache_state", "summarizer assembly"),
    (
        "COALESCE(MAX(sequence), 0) FROM history_segments",
        "publication set fence",
    ),
    ("history_segments", "coverage snapshot"),
    ("cache_state", "coverage snapshot"),
    ("block_identities", "coverage snapshot"),
    (" tags ", "tags"),
    ("temporal_marks", "temporal marks, hints, appends"),
    ("user_hints", "temporal marks, hints, appends"),
    ("channel1_appends", "temporal marks, hints, appends"),
    ("overlay_frontiers", "temporal marks, hints, appends"),
    ("user_memories", "active user memories"),
    (" notes ", "notes status version"),
    ("project_memory", "kernel project memory"),
    // The secret-scan ledger is part of the spec's "pass trace and ledgers" row, measured
    // apart because its keys are random.
    ("scan_", "scan ledger"),
    ("field_scans", "scan ledger"),
    // The trace ring, the side channels and pending drops, the session's project root, and
    // the connection's authority fence and transactions.
    ("pass_trace", "pass trace and ledgers"),
    ("history_summarizer_", "pass trace and ledgers"),
    ("pending_agent_drops", "pass trace and ledgers"),
    ("transform_session_roots", "pass trace and ledgers"),
    ("FROM fence", "pass trace and ledgers"),
    ("PRAGMA ", "pass trace and ledgers"),
    ("temp.sqlite_schema", "pass trace and ledgers"),
    ("BEGIN ", "pass trace and ledgers"),
    ("COMMIT", "pass trace and ledgers"),
    ("ROLLBACK", "pass trace and ledgers"),
];

/// The inventory row of one statement, by its SQL text.
fn classify(sql: &str) -> Option<&'static str> {
    let sql = format!(" {} ", sql.split_whitespace().collect::<Vec<_>>().join(" "));
    CLASSES
        .iter()
        .find(|(needle, _)| sql.contains(needle))
        .map(|(_, row)| *row)
}

/// Rows and VM steps per inventory row, summed over the statements of one phase.
type Totals = BTreeMap<&'static str, (u64, u64)>;

fn totals(phase: &str, h: usize, work: &[StatementWork]) -> Totals {
    let mut out = Totals::new();
    for statement in work {
        let row = classify(&statement.sql).unwrap_or_else(|| {
            eprintln!("read-bound unclassified {phase} at H={h}: {statement:?}");
            UNCLASSIFIED
        });
        let entry = out.entry(row).or_default();
        entry.0 += statement.rows;
        entry.1 += statement.vm_steps;
    }
    out
}

fn request(h: usize, render_config: &str) -> TransformRequest {
    let end = 2 * h as u64;
    let mut messages = vec![
        item(&format!("m{}", end - 1), end - 1, "covered question"),
        item(&format!("m{end}"), end, "covered answer"),
    ];
    messages.extend((1..=TAIL).map(|k| {
        item(
            &format!("m{}", end + k),
            end + k,
            &format!("window message {k} with some text"),
        )
    }));
    active_cc_req(SESSION, render_config, messages)
}

fn pass(store: &MemoryStore, request: &TransformRequest) -> String {
    let dir = "/nonexistent-docs";
    let ctx = pctx("git:read-bound", dir, 1_700_000_000_000);
    transform_with_projection_cached(store, request, &ctx, &Mutex::default(), None)
        .expect("pass")
        .response
        .action
}

/// Seeds a session of `h` segments with `h` overlay rows per table and `h` tag rows outside
/// the window, then measures each phase.
fn measure(h: usize) -> Vec<(&'static str, Totals)> {
    let dir = tempfile::tempdir().expect("store dir");
    let store = Arc::new(store(dir.path()));
    let history = SyntheticHistory::mixed(h);
    history.seed(&store, SESSION);
    seed_overlays(&store, SESSION, h);
    store
        .execute_tag_sql_for_test(&format!(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {h})
             INSERT INTO tags
                 (session_id, tag_number, block_id, kind, token_count, created_at_ms, source_bytes)
             SELECT '{SESSION}', i, 'history-' || i || '#0', 'message', 1, 1, X'61' FROM n"
        ))
        .expect("seed tags");

    // The first fold captures the legacy list with its one declared scan; the second pass
    // mints the window's tags once the tag surface is durable.
    let warm = request(h, "cfg0");
    assert_eq!(pass(&store, &warm), "HARD");
    pass(&store, &warm);
    seed_active_summarizer(&store, SESSION);

    let mut phases = Vec::new();
    store.arm_soft_refresh(SESSION).expect("arm soft");
    store.start_statement_work_ledger();
    assert_eq!(pass(&store, &warm), "SOFT", "H={h}");
    phases.push(("SOFT", totals("SOFT", h, &store.take_statement_work())));

    store.start_statement_work_ledger();
    assert_eq!(pass(&store, &request(h, "cfg1")), "HARD", "H={h}");
    phases.push(("HARD", totals("HARD", h, &store.take_statement_work())));

    // A write between the pass's reads and its commit fails the CAS; the retry reruns the
    // pass. The conflicting write itself is kept out of the ledger.
    let first_attempt = Arc::new(Mutex::new(Vec::new()));
    let hook_store = Arc::clone(&store);
    let hook_work = Arc::clone(&first_attempt);
    install_transform_attempt_hook(SESSION, move || {
        *hook_work.lock().unwrap() = hook_store.take_statement_work();
        let loaded = hook_store.load(SESSION).unwrap();
        hook_store
            .commit(SESSION, loaded.row_version, &loaded.core, &loaded.meta)
            .unwrap();
        hook_store.start_statement_work_ledger();
    });
    store.start_statement_work_ledger();
    assert_eq!(pass(&store, &request(h, "cfg2")), "HARD", "H={h}");
    let retry = store.take_statement_work();
    let first = std::mem::take(&mut *first_attempt.lock().unwrap());
    assert!(!first.is_empty(), "H={h}: the CAS conflict fired");
    phases.push(("HARD first attempt", totals("HARD first", h, &first)));
    phases.push(("HARD CAS retry", totals("HARD retry", h, &retry)));

    phases.push(("summarizer", summarizer_round(&store, h)));
    phases
}

/// Prepares a history_summarizer chunk over the first two window messages and publishes it.
fn summarizer_round(store: &MemoryStore, h: usize) -> Totals {
    let end = 2 * h as i64;
    let loaded = store.load(SESSION).expect("load");
    let generation = HistorySegmentSetGeneration {
        max_sequence: h as i64,
        ..Default::default()
    };
    let selected_range_identities = [end + 1, end + 2]
        .iter()
        .map(|ordinal| {
            let mid = format!("m{ordinal}");
            HistorySummarizerSelectedMessageIdentity {
                block_identities: loaded.meta.block_identity_by_mid[&mid].clone(),
                mid,
            }
        })
        .collect::<Vec<_>>();
    let predicate = HistorySummarizerPublishPredicate {
        firing_seq: 2,
        producer_run_id: "read-bound-run".to_string(),
        chunk_fingerprint: "read-bound-fingerprint".to_string(),
        selected_range_identities: selected_range_identities.clone(),
        history_segment_set_generation: generation,
    };
    let mut meta = loaded.meta.clone();
    meta.history_summarizer = HistorySummarizerDurableState {
        state: HistorySummarizerPhase::Publishing,
        firing_seq: predicate.firing_seq,
        chunk_range: Some(HistorySummarizerChunkRange {
            from_ordinal: end as u64 + 1,
            to_ordinal: end as u64 + 2,
        }),
        chunk_fingerprint: predicate.chunk_fingerprint.clone(),
        selected_range_identities,
        producer_session_id: Some("read-bound-producer".to_string()),
        producer_run_id: Some(predicate.producer_run_id.clone()),
        fired_at_ms: Some(1),
        expected_revert_epoch: loaded.meta.revert_epoch,
        history_segment_set_generation: generation,
        ..Default::default()
    };
    let row_version = store
        .commit(SESSION, loaded.row_version, &loaded.core, &meta)
        .expect("enter publishing");
    let published = StoredHistorySegment {
        sequence: h as i64 + 1,
        start_message: end + 1,
        end_message: end + 2,
        start_message_id: format!("m{}#0", end + 1),
        end_message_id: format!("m{}#0", end + 2),
        title: "published".to_string(),
        content: "published summary".to_string(),
        p1: Some("published summary".to_string()),
        importance: 50,
        created_at: 1,
        ..Default::default()
    };

    store.start_statement_work_ledger();
    let snapshot = store
        .load_history_summarizer_assembly_snapshot(SESSION, 6)
        .expect("prepare");
    assert_eq!(snapshot.newest_history_segments.len(), 6);
    store
        .publish_history_summarizer_chunk(HistorySummarizerPublishRequest {
            session_id: SESSION,
            expected_row_version: Some(row_version),
            expected_revert_epoch: loaded.meta.revert_epoch,
            predicate: &predicate,
            project_path: "git:read-bound",
            history_segments: std::slice::from_ref(&published),
            events: &[],
            primer_candidates: &[],
            user_memory_candidates: &[],
            publication_floor_ordinal: end as u64 + 3,
            chunk_transcript: None,
            memory_reviewer_nonadmission: None,
            memory_reviewer_activation: None,
            published_at_ms: 1,
        })
        .expect("publish");
    totals("summarizer", h, &store.take_statement_work())
}

/// Rows whose work depends on the stored history through the fold's decay bound.
fn fold_row(row: &str) -> bool {
    matches!(row, "m0 segments" | "m1 segments")
}

#[test]
fn every_pass_read_is_bounded_independent_of_history_size() {
    let sizes = [100usize, 5_000, 50_000];
    let measured = sizes.map(|h| (h, measure(h)));
    eprintln!("read-bound phase | row | H | rows | vm_steps");
    for (h, phases) in &measured {
        for (phase, totals) in phases {
            for row in ROWS {
                if let Some((rows, steps)) = totals.get(row) {
                    eprintln!("read-bound {phase} | {row} | {h} | {rows} | {steps}");
                }
            }
        }
    }

    for (h, phases) in &measured {
        for (phase, totals) in phases {
            assert!(
                !totals.contains_key(UNCLASSIFIED),
                "{phase} at H={h} ran a statement no inventory row names"
            );
        }
    }

    let legacy = SyntheticHistory::mixed(50_000).legacy_count() as u64;
    let phases = |index: usize| &measured[index].1;
    for (phase_index, (phase, small)) in phases(0).iter().enumerate() {
        let (_, middle) = &phases(1)[phase_index];
        let (_, large) = &phases(2)[phase_index];
        let rows = small
            .keys()
            .chain(large.keys())
            .collect::<std::collections::BTreeSet<_>>();
        for row in rows {
            if fold_row(row) {
                let (rows_read, _) = large[row];
                let cap = if *row == "m0 segments" {
                    249 + MAX_K + legacy
                } else {
                    crate::m1_compose::DEFAULT_M1_ROW_CAP as u64 + 1
                };
                assert!(
                    rows_read <= cap,
                    "{phase} {row}: {rows_read} rows over {cap}"
                );
                assert_eq!(
                    middle.get(row),
                    large.get(row),
                    "{phase} {row}: H=5,000 vs 50,000"
                );
            } else if *row == "scan ledger" {
                // Scan ids are random, so a statement's B-tree walk varies by a few steps
                // from run to run at any H; the rows it touches do not.
                let ((small_rows, small_steps), (large_rows, large_steps)) =
                    (small[row], large[row]);
                assert_eq!(
                    small_rows, large_rows,
                    "{phase} {row}: rows at H=100 vs 50,000"
                );
                assert!(
                    small_steps.abs_diff(large_steps) <= 16 + small_steps / 100,
                    "{phase} {row}: {small_steps} steps at H=100 vs {large_steps} at 50,000"
                );
            } else {
                assert_eq!(
                    small.get(row),
                    large.get(row),
                    "{phase} {row}: H=100 vs 50,000"
                );
            }
        }
    }
}
