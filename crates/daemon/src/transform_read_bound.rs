//! The per-pass read inventory (spec D15, property WP-P07): every statement a transform pass
//! or a history_summarizer prepare and publish runs is recorded by the store's statement-work
//! ledger, classified into one inventory row by its SQL text, and checked against that row's
//! bound while the stored history, the overlays, and the active user memories grow, together
//! and apart. The guard against a new full read is the H-independence check: its rows and VM
//! steps would grow with the seeded tables. Classification only names the row; a statement no
//! needle names fails the test, and history_segments reads are named by their exact shapes.

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
use crate::transform::tests::{active_cc_req, assistant_tool_call, item, pctx, store, tool_result};
use crate::transform::{
    TransformRequest, install_transform_attempt_hook, transform_with_projection_cached,
};

const SESSION: &str = "read-bound";
/// Live messages after the covered pair the request repeats.
const TAIL: u64 = 8;
/// Largest non-archived index under any pressure (WP-P08).
const MAX_K: u64 = crate::decay_render::MAX_RENDERABLE_INDEX as u64;
/// The request's tool call id; one legacy tag row is keyed by it.
const CALL: &str = "call-read-bound";
/// Active user memories seeded when the axis holds them fixed.
const MEMORIES: usize = 4;

/// The D15 inventory rows, in report order.
const ROWS: &[&str] = &[
    // Holds the session state reads too: the pass's cache_state row and the lineage and
    // assembly meta reads (see CLASSES).
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

/// Rows no statement of these passes touches, each with the reason.
const ABSENT_BY_DESIGN: &[(&str, &str)] = &[(
    "kernel project memory",
    "the handler reads it before the pass and hands it in through ProducerContext",
)];

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
        "history_segments WHERE session_id = ?1 ORDER BY sequence DESC LIMIT ?2",
        "summarizer assembly",
    ),
    (
        "SELECT MAX(end_message) FROM history_segments",
        "summarizer assembly",
    ),
    (
        "COALESCE(MAX(sequence), 0) FROM history_segments",
        "publication set fence",
    ),
    (
        "SELECT COALESCE(MAX(end_message), 0) FROM history_segments WHERE session_id = ?1",
        "coverage snapshot",
    ),
    // Session state: the pass's cache_state row, and the lineage and assembly meta reads.
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

/// The history_segments edge lookups and the uncovered-ordinal probe, matched as whole
/// statements so a full read sharing their column list stays unclassified.
const EDGE_STATEMENTS: &[&str] = &[
    "SELECT sequence, start_message, end_message, start_message_id, end_message_id \
     FROM history_segments WHERE session_id = ?1 ORDER BY sequence DESC LIMIT 1",
    "SELECT sequence, start_message, end_message, start_message_id, end_message_id \
     FROM history_segments WHERE session_id = ?1 ORDER BY sequence ASC LIMIT 1",
    "SELECT sequence, start_message, end_message, start_message_id, end_message_id \
     FROM history_segments WHERE session_id = ?1 AND end_message = ?2 LIMIT 1",
    "SELECT sequence, start_message, end_message, start_message_id, end_message_id \
     FROM history_segments WHERE session_id = ?1 AND sequence = ?2",
    "SELECT sequence, start_message, end_message, start_message_id, end_message_id \
     FROM history_segments WHERE session_id = ?1 AND sequence < ?2 ORDER BY sequence DESC LIMIT 1",
    "SELECT sequence, start_message, end_message, start_message_id, end_message_id \
     FROM history_segments WHERE session_id = ?1 AND sequence > ?2 ORDER BY sequence ASC LIMIT 1",
    "SELECT j.value FROM json_each(?2) AS j WHERE COALESCE( (SELECT h.start_message \
     FROM history_segments AS h WHERE h.session_id = ?1 AND h.end_message >= j.value \
     ORDER BY h.end_message LIMIT 1), j.value + 1) > j.value",
];

/// m1's capped read above the folded sequence, matched as a whole statement like the edges.
const M1_STATEMENT: &str = "SELECT sequence, start_message, end_message, start_message_id, \
     end_message_id, start_date, end_date, title, content, p1, p2, p3, p4, importance, \
     episode_type, legacy, created_at FROM history_segments \
     WHERE session_id = ?1 AND sequence > ?2 ORDER BY sequence DESC LIMIT ?3";

/// The SQL text with its whitespace collapsed and padded by one space on each side.
fn normalized(sql: &str) -> String {
    format!(" {} ", sql.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// The inventory row of one statement, by its SQL text.
fn classify(sql: &str) -> Option<&'static str> {
    let sql = normalized(sql);
    if EDGE_STATEMENTS
        .iter()
        .any(|statement| sql.trim() == normalized(statement).trim())
    {
        return Some("coverage snapshot");
    }
    if sql.trim() == normalized(M1_STATEMENT).trim() {
        return Some("m1 segments");
    }
    CLASSES
        .iter()
        .find(|(needle, _)| sql.contains(needle))
        .map(|(_, row)| *row)
}

/// A table read, count, update, or delete without a WHERE clause. SQLite can do O(table)
/// work in one opcode (`COUNT(*)`, a whole-table clear), which `VM_STEP` does not see, and
/// page counts grow with B-tree depth, so the shape itself is what is ruled out.
/// The connection's temp schema is the one table read whole: it holds the storage layer's
/// own fixed set of temp objects, not session data. The check is textual: any WHERE in the
/// statement, even one inside a subquery, hides a whole-table read in the outer query.
fn whole_table(sql: &str) -> bool {
    let sql = normalized(sql);
    (sql.contains(" FROM ") || sql.starts_with(" UPDATE "))
        && !sql.contains(" WHERE ")
        && !sql.contains(" FROM temp.sqlite_schema ")
}

/// Rows and VM steps per inventory row, summed over the statements of one phase.
type Totals = BTreeMap<&'static str, (u64, u64)>;

/// The totals of each phase of one measurement, in phase order.
type Measured = Vec<(&'static str, Totals)>;

fn totals(phase: &str, h: usize, work: &[StatementWork]) -> Totals {
    let mut out = Totals::new();
    for statement in work {
        assert!(
            !whole_table(&statement.sql),
            "{phase} at H={h} ran a whole-table statement: {}",
            statement.sql
        );
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

/// The covered pair, then `TAIL` window messages ending in a tool call and its result.
fn request(h: usize, render_config: &str) -> TransformRequest {
    let end = 2 * h as u64;
    let mut messages = vec![
        item(&format!("m{}", end - 1), end - 1, "covered question"),
        item(&format!("m{end}"), end, "covered answer"),
    ];
    messages.extend((1..=TAIL - 2).map(|k| {
        item(
            &format!("m{}", end + k),
            end + k,
            &format!("window message {k} with some text"),
        )
    }));
    let (call, result) = (end + TAIL - 1, end + TAIL);
    messages.push(assistant_tool_call(&format!("m{call}"), call, CALL));
    messages.push(tool_result(
        &format!("m{result}"),
        result,
        CALL,
        "tool output",
    ));
    active_cc_req(SESSION, render_config, messages)
}

fn pass(store: &MemoryStore, request: &TransformRequest) -> String {
    let dir = "/nonexistent-docs";
    let ctx = pctx("git:read-bound", dir, 1_700_000_000_000);
    transform_with_projection_cached(store, request, &ctx, &Mutex::default())
        .expect("pass")
        .response
        .action
}

/// The legacy tag read's rows in `work`: the statement keyed by the window's tool call ids.
fn legacy_tag_rows(work: &[StatementWork]) -> u64 {
    work.iter()
        .filter(|statement| {
            normalized(&statement.sql)
                .contains(" FROM tags WHERE session_id = ?1 AND block_id IN (SELECT value FROM json_each(?2))")
        })
        .map(|statement| statement.rows)
        .sum()
}

/// The rows the pass's window-keyed read of overlay `table` returned in `work`.
fn window_overlay_rows(work: &[StatementWork], table: &str) -> u64 {
    work.iter()
        .filter(|statement| {
            let sql = normalized(&statement.sql);
            sql.contains(&format!(" FROM {table} "))
                && sql.contains(" AND block_id IN (SELECT value FROM json_each(?2)) ")
        })
        .map(|statement| statement.rows)
        .sum()
}

/// Seeds a session of `h` segments, `overlays` overlay rows per table off the window plus
/// one per table on a window block, `h` tag rows outside the window plus one legacy row keyed
/// by the request's tool call id, and `memories` active user memories, then measures each
/// phase. The foreign tag ids sort below the window (`history-*`), between a window id and
/// its `#` range (`m<end>!*`), and above the window (`zz-*`), so a range loose at either end
/// reads rows that grow with `h`.
fn measure(h: usize, overlays: usize, memories: usize) -> Measured {
    let dir = tempfile::tempdir().expect("store dir");
    let store = Arc::new(store(dir.path()));
    let history = SyntheticHistory::mixed(h);
    history.seed(&store, SESSION);
    seed_overlays(&store, SESSION, overlays);
    let end = 2 * h as u64;
    store
        .with_fenced_conn_for_test(|tx| {
            // The temporal mark sits on the tool call, a block the pass never marks, so its
            // row is read only because the window keys reach it.
            for (table, text, ordinal) in [
                ("temporal_marks", "marker_text, created_at", end + TAIL - 1),
                ("user_hints", "hint_text, created_at", end + 1),
                ("channel1_appends", "reminder_text, fired_at_ms", end + 1),
            ] {
                tx.execute(
                    &format!(
                        "INSERT INTO {table} (session_id, block_id, {text}) VALUES (?1, ?2, ?3, 1)"
                    ),
                    rusqlite::params![SESSION, format!("m{ordinal}#0"), format!("{table} window")],
                )?;
            }
            Ok(())
        })
        .expect("seed window overlays");
    store
        .execute_tag_sql_for_test(&format!(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {h})
             INSERT INTO tags
                 (session_id, tag_number, block_id, kind, token_count, created_at_ms, source_bytes)
             SELECT '{SESSION}', i,
                    CASE i % 4
                        WHEN 1 THEN 'm{end}!' || i
                        WHEN 3 THEN 'zz-' || i || '#0'
                        ELSE 'history-' || i || '#0'
                    END,
                    'message', 1, 1, X'61'
               FROM n;
             INSERT INTO tags
                 (session_id, tag_number, block_id, kind, token_count, created_at_ms, source_bytes)
             VALUES ('{SESSION}', {h} + 1, '{CALL}', 'tool_call', 1, 1, X'61');"
        ))
        .expect("seed tags");
    store
        .with_fenced_conn_for_test(|tx| {
            for id in 1..=memories as i64 {
                tx.execute(
                    "INSERT INTO user_memories (id, content, status, promoted_at)
                     VALUES (?1, ?2, 'active', ?1)",
                    rusqlite::params![id, format!("profile line {id}")],
                )?;
            }
            Ok(())
        })
        .expect("seed user memories");

    // The first fold captures the legacy list with its one declared scan; the second pass
    // mints the window's tags once the tag surface is durable.
    let warm = request(h, "cfg0");
    assert_eq!(pass(&store, &warm), "HARD");
    pass(&store, &warm);
    // A daemon restart: the measured passes run on a store with no process-local memo, so a
    // read the store remembers only in memory shows up as a cold full read.
    drop(store);
    let store = Arc::new(crate::transform::tests::store(dir.path()));
    seed_active_summarizer(&store, SESSION);

    let mut phases = Vec::new();
    store.arm_soft_refresh(SESSION).expect("arm soft");
    store.start_statement_work_ledger();
    assert_eq!(pass(&store, &warm), "SOFT", "H={h}");
    phases.push(("SOFT", totals("SOFT", h, &store.take_statement_work())));

    store.start_statement_work_ledger();
    assert_eq!(pass(&store, &request(h, "cfg1")), "HARD", "H={h}");
    let work = store.take_statement_work();
    assert_eq!(
        legacy_tag_rows(&work),
        1,
        "H={h}: the legacy tag row is read"
    );
    // Temporal marks: the pass's own marks on the covered pair and the six window user
    // messages, plus the seeded row on the tool call.
    for (table, rows) in [
        ("user_hints", 1),
        ("channel1_appends", 1),
        ("temporal_marks", TAIL + 1),
    ] {
        assert_eq!(
            window_overlay_rows(&work, table),
            rows,
            "H={h}: {table} rows on the window"
        );
    }
    phases.push(("HARD", totals("HARD", h, &work)));

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

    // The host drops the covered pair: the boundary is absent while the durable lineage and
    // the seeded history remain, so the pass decides the absent shape from the oldest row.
    let mut absent = request(h, "cfg2");
    absent.messages.drain(..2);
    store.start_statement_work_ledger();
    assert_eq!(pass(&store, &absent), "PASSTHROUGH", "H={h}");
    let work = store.take_statement_work();
    let oldest_edge = normalized(EDGE_STATEMENTS[1]);
    assert!(
        work.iter()
            .any(|statement| normalized(&statement.sql) == oldest_edge),
        "H={h}: the absent shape reads the oldest row"
    );
    phases.push(("absent boundary", totals("absent boundary", h, &work)));
    for (phase, totals) in &phases {
        // The bound of this row is the host profile's line count, not H: state sync replaces
        // the active memories wholesale and a HARD pass reads every one.
        let memory_rows = totals.get("active user memories").map(|(rows, _)| *rows);
        if phase.starts_with("HARD") {
            assert_eq!(memory_rows, Some(memories as u64), "{phase} at H={h}");
        } else if let Some(rows) = memory_rows {
            assert_eq!(rows, memories as u64, "{phase} at H={h}");
        }
    }
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

fn report(axis: &str, label: &str, phases: &Measured) {
    for (phase, totals) in phases {
        for row in ROWS {
            if let Some((rows, steps)) = totals.get(row) {
                eprintln!("read-bound {axis} | {label} | {phase} | {row} | {rows} | {steps}");
            }
        }
    }
}

/// Asserts every row outside `skip` is equal between two measurements of the same request.
fn assert_same(axis: &str, small: &Measured, large: &Measured, skip: impl Fn(&str) -> bool) {
    for ((phase, small), (_, large)) in small.iter().zip(large) {
        let rows = small
            .keys()
            .chain(large.keys())
            .collect::<std::collections::BTreeSet<_>>();
        for row in rows.into_iter().filter(|row| !skip(row)) {
            if *row == "scan ledger" {
                // Scan ids are random, so a statement's B-tree walk varies by a few steps
                // from run to run at any H; the rows it touches do not.
                let ((small_rows, small_steps), (large_rows, large_steps)) =
                    (small[row], large[row]);
                assert_eq!(small_rows, large_rows, "{axis} {phase} {row}: rows");
                assert!(
                    small_steps.abs_diff(large_steps) <= 16 + small_steps / 100,
                    "{axis} {phase} {row}: {small_steps} steps vs {large_steps}"
                );
            } else {
                assert_eq!(small.get(row), large.get(row), "{axis} {phase} {row}");
            }
        }
    }
}

#[test]
fn every_pass_read_is_bounded_independent_of_history_size() {
    let sizes = [100usize, 5_000, 50_000];
    let together = sizes.map(|h| measure(h, h, MEMORIES));
    let history_only = measure(50_000, 100, MEMORIES);
    let overlays_only = measure(100, 5_000, 10 * MEMORIES);
    eprintln!("read-bound axis | size | phase | row | rows | vm_steps");
    for (h, phases) in sizes.iter().zip(&together) {
        report("together", &format!("H=overlays={h}"), phases);
    }
    report("H only", "H=50000 overlays=100", &history_only);
    report(
        "overlays and memories only",
        "H=100 overlays=5000 memories=40",
        &overlays_only,
    );

    let all = together.iter().chain([&history_only, &overlays_only]);
    let mut observed = std::collections::BTreeSet::new();
    for phases in all {
        for (phase, totals) in phases {
            assert!(
                !totals.contains_key(UNCLASSIFIED),
                "{phase} ran a statement no inventory row names"
            );
            observed.extend(totals.keys().copied());
        }
    }
    for row in ROWS {
        let absent = ABSENT_BY_DESIGN.iter().find(|(name, _)| name == row);
        assert_eq!(
            observed.contains(row),
            absent.is_none(),
            "{row}: observed {}, declared absent {absent:?}",
            observed.contains(row)
        );
    }

    let legacy = SyntheticHistory::mixed(50_000).legacy_count() as u64;
    for ((phase, middle), (_, large)) in together[1].iter().zip(&together[2]) {
        for row in large.keys().filter(|row| fold_row(row)) {
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
        }
    }
    let memories = |row: &str| row == "active user memories";
    assert_same("together", &together[0], &together[2], fold_row);
    assert_same("H only", &together[0], &history_only, fold_row);
    assert_same(
        "overlays and memories only",
        &together[0],
        &overlays_only,
        memories,
    );
}
