use std::sync::Arc;

use memory_store::{CoreState, ModuleMeta};

use super::*;
use crate::edit_recipe::{Keyed, Recipe, Revision, SourceBase, build_operations, canonical_len};
use crate::test_support::descriptor;
use crate::test_support::synthetic_history::SyntheticHistory;

pub(crate) const SESSION: &str = "window-coverage";

/// Seeds `segments` synthetic rows (row `i` ends at message `m{2i}`, ordinal `2i`) and, when
/// `rendered` or `continuation_base` is set, a session row whose coverage is row `rendered`.
pub(crate) fn seed_coverage(
    store: &MemoryStore,
    segments: usize,
    rendered: Option<usize>,
    continuation_base: Option<u64>,
) {
    if segments > 0 {
        SyntheticHistory::mixed(segments).seed(store, SESSION);
    }
    if rendered.is_none() && continuation_base.is_none() {
        return;
    }
    let mut core = CoreState::empty();
    let mut meta = ModuleMeta::default();
    if let Some(rendered) = rendered {
        core.boundary_id = format!("m{}#0", 2 * rendered);
        meta.coverage_ordinal = Some(2 * rendered as u64);
    }
    meta.ordinal_continuation_base = continuation_base;
    store
        .with_fenced_conn_for_test(|conn| {
            conn.execute(
                "INSERT INTO cache_state (session_id, row_version, core_state, meta, last_activity_at)
                 VALUES (?1, 1, ?2, ?3, 0)",
                rusqlite::params![
                    SESSION,
                    serde_json::to_string(&core).unwrap(),
                    serde_json::to_string(&meta).unwrap()
                ],
            )
        })
        .unwrap();
}

fn open_store() -> (tempfile::TempDir, MemoryStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&descriptor(dir.path())).unwrap();
    (dir, store)
}

/// Messages `m{from}..=m{to}`, none synthetic.
fn mids(from: usize, to: usize) -> Vec<String> {
    (from..=to).map(|n| format!("m{n}")).collect()
}

fn window(mids: &[String]) -> Vec<WindowMessage<'_>> {
    mids.iter()
        .map(|mid| WindowMessage {
            mid,
            synthetic: mid.starts_with('s'),
        })
        .collect()
}

fn anchor(mid: &str, sequence: i64) -> Option<DeclaredAnchor<'_>> {
    Some(DeclaredAnchor { mid, sequence })
}

fn resolve_in(
    store: &MemoryStore,
    declared: Option<DeclaredAnchor<'_>>,
    window: &[WindowMessage<'_>],
) -> Result<Resolved, String> {
    let snapshot = read_snapshot(store, SESSION, declared, window).unwrap();
    resolve(&snapshot, declared, window)
}

/// A keep of every processed message, then one previous-output keep and one insert.
fn translated_keeps(processed: usize, cut: usize) -> Vec<Operation<u8>> {
    let mut operations = vec![
        Operation::Keep {
            source: Source::Input,
            start: 0,
            count: processed as u64,
        },
        Operation::Keep {
            source: Source::Previous,
            start: 0,
            count: 1,
        },
        Operation::Insert { values: vec![7] },
    ];
    translate_input_keeps(&mut operations, cut);
    operations
}

struct Case {
    name: &'static str,
    segments: usize,
    rendered: Option<usize>,
    continuation_base: Option<u64>,
    declared: Option<(&'static str, i64)>,
    window: Vec<String>,
    resolution: Resolution,
    anchor_sequence: Option<i64>,
    ordinals: Vec<u64>,
    /// Where the processed window's first message sits in the submitted window.
    keep_start: u64,
}

/// WP-P02: one witness per transition row, with its cut, its ordinals, and its translated keeps.
#[test]
fn each_resolution_outcome_has_its_cut_ordinals_and_keeps() {
    let cases = [
        Case {
            name: "the declared row is the rendered boundary",
            segments: 5,
            rendered: Some(5),
            continuation_base: None,
            declared: Some(("m10", 5)),
            window: mids(10, 12),
            resolution: Resolution::Normal,
            anchor_sequence: Some(5),
            ordinals: vec![10, 11, 12],
            keep_start: 0,
        },
        Case {
            name: "coverage advanced past the declared row, whose successor is still in the window",
            segments: 5,
            rendered: Some(5),
            continuation_base: None,
            declared: Some(("m6", 3)),
            window: mids(6, 12),
            resolution: Resolution::StaleSlice { cut: 4 },
            anchor_sequence: Some(5),
            ordinals: vec![10, 11, 12],
            keep_start: 4,
        },
        Case {
            name: "the rendered boundary's message is gone from the window",
            segments: 5,
            rendered: Some(5),
            continuation_base: None,
            declared: Some(("m6", 3)),
            window: mids(6, 8),
            resolution: Resolution::Revert {
                keep_through_seq: Some(3),
            },
            anchor_sequence: Some(3),
            ordinals: vec![6, 7, 8],
            keep_start: 0,
        },
        Case {
            name: "the declared sequence names no row",
            segments: 5,
            rendered: Some(5),
            continuation_base: None,
            declared: Some(("m6", 99)),
            window: mids(6, 8),
            resolution: Resolution::Unknown,
            anchor_sequence: None,
            ordinals: vec![],
            keep_start: 0,
        },
        Case {
            name: "no boundary, coverage held, and a surviving segment end in the window",
            segments: 5,
            rendered: Some(5),
            continuation_base: None,
            declared: None,
            window: mids(1, 8),
            resolution: Resolution::StaleSlice { cut: 7 },
            anchor_sequence: Some(4),
            ordinals: vec![8],
            keep_start: 7,
        },
        Case {
            name: "no boundary, coverage held, and no surviving segment end",
            segments: 5,
            rendered: Some(5),
            continuation_base: Some(30),
            declared: None,
            window: vec!["x1".into(), "x2".into()],
            resolution: Resolution::Revert {
                keep_through_seq: None,
            },
            anchor_sequence: None,
            ordinals: vec![1, 2],
            keep_start: 0,
        },
        Case {
            name: "no boundary and no coverage",
            segments: 0,
            rendered: None,
            continuation_base: Some(40),
            declared: None,
            window: vec!["x1".into(), "x2".into()],
            resolution: Resolution::FirstPass,
            anchor_sequence: None,
            ordinals: vec![41, 42],
            keep_start: 0,
        },
    ];
    for case in cases {
        let (_dir, store) = open_store();
        seed_coverage(&store, case.segments, case.rendered, case.continuation_base);
        let window = window(&case.window);
        let declared = case
            .declared
            .map(|(mid, sequence)| DeclaredAnchor { mid, sequence });
        let resolved = resolve_in(&store, declared, &window).unwrap();
        assert_eq!(resolved.resolution, case.resolution, "{}", case.name);
        assert_eq!(
            resolved.anchor.as_ref().map(|row| row.sequence),
            case.anchor_sequence,
            "{}",
            case.name
        );
        assert_eq!(resolved.ordinals, case.ordinals, "{}", case.name);
        let cut = resolved.cut();
        if resolved.resolution != Resolution::Unknown {
            assert_eq!(resolved.ordinals.len(), window.len() - cut, "{}", case.name);
        }
        assert_eq!(
            translated_keeps(window.len() - cut, cut)[..2],
            [
                Operation::Keep {
                    source: Source::Input,
                    start: case.keep_start,
                    count: (window.len() - cut) as u64,
                },
                Operation::Keep {
                    source: Source::Previous,
                    start: 0,
                    count: 1,
                },
            ],
            "{}",
            case.name
        );
    }
}

/// WP-P02: `Revert(None)` is never `Unknown`, and inputs a correct plugin cannot send are
/// `invalid_params`.
#[test]
fn impossible_declarations_are_invalid_params() {
    let (_dir, store) = open_store();
    seed_coverage(&store, 5, Some(3), None);
    let newer = mids(8, 10);
    let error = resolve_in(&store, anchor("m8", 4), &window(&newer)).unwrap_err();
    assert!(
        error.contains("newer than the rendered boundary"),
        "{error}"
    );
    let mismatched = mids(7, 10);
    let error = resolve_in(&store, anchor("m7", 3), &window(&mismatched)).unwrap_err();
    assert!(error.contains("not at the declared boundary"), "{error}");
    let off_head = mids(7, 10);
    let error = resolve_in(&store, anchor("m6", 3), &window(&off_head)).unwrap_err();
    assert!(error.contains("does not start at"), "{error}");
}

/// A declared row that survives a revert truncation which left `core.boundary_id` naming a
/// removed row is `Unknown`, a decline that makes the plugin rediscover, not `invalid_params`.
#[test]
fn a_declared_row_without_a_rendered_boundary_is_unknown() {
    let (_dir, store) = open_store();
    seed_coverage(&store, 5, Some(5), None);
    store
        .truncate_history_segments_for_revert(SESSION, 3, Some(1))
        .unwrap();
    let submitted = mids(6, 8);
    let resolved = resolve_in(&store, anchor("m6", 3), &window(&submitted)).unwrap();
    assert_eq!(resolved.resolution, Resolution::Unknown);
    let resolved = resolve_in(&store, None, &window(&submitted)).unwrap();
    assert_eq!(resolved.resolution, Resolution::FirstPass);
}

/// WP-P02 snapshot clause: a publish that commits between the session-row read and the
/// declared-row read is invisible to the resolution that read them. Reading the declared
/// row outside the snapshot would miss it and answer `Unknown`.
#[test]
fn a_publish_between_the_core_read_and_the_declared_read_is_invisible() {
    let (dir, store) = open_store();
    seed_coverage(&store, 5, Some(5), None);
    let raw_path = dir.path().join("store.db");
    store.set_coverage_snapshot_hook(Box::new(move || {
        let raw = rusqlite::Connection::open(raw_path).unwrap();
        raw.execute(
            "DELETE FROM history_segments WHERE session_id = ?1",
            [SESSION],
        )
        .unwrap();
        raw.execute(
            "UPDATE cache_state SET row_version = 2, core_state = json_set(core_state, '$.boundary_id', '')
              WHERE session_id = ?1",
            [SESSION],
        )
        .unwrap();
    }));
    let submitted = mids(10, 11);
    let held = resolve_in(&store, anchor("m10", 5), &window(&submitted)).unwrap();
    assert_eq!(held.resolution, Resolution::Normal);
    assert_eq!(held.ordinals, vec![10, 11]);
    let after = resolve_in(&store, anchor("m10", 5), &window(&submitted)).unwrap();
    assert_eq!(after.resolution, Resolution::Unknown);
}

/// The snapshot clause on the null path: segments deleted between the session-row read and
/// the intersection are still seen, so the answer is the snapshot's `StaleSlice`, not
/// `Revert { keep_through_seq: None }`.
#[test]
fn a_removal_between_the_core_read_and_the_intersection_is_invisible() {
    let (dir, store) = open_store();
    seed_coverage(&store, 5, Some(5), None);
    let raw_path = dir.path().join("store.db");
    store.set_coverage_snapshot_hook(Box::new(move || {
        rusqlite::Connection::open(raw_path)
            .unwrap()
            .execute(
                "DELETE FROM history_segments WHERE session_id = ?1",
                [SESSION],
            )
            .unwrap();
    }));
    let submitted = mids(1, 8);
    let held = resolve_in(&store, None, &window(&submitted)).unwrap();
    assert_eq!(held.resolution, Resolution::StaleSlice { cut: 7 });
    // Afterwards no rendered row remains.
    let after = resolve_in(&store, None, &window(&submitted)).unwrap();
    assert_eq!(after.resolution, Resolution::FirstPass);
}

/// The null-anchor intersection matches a window mid only at its positional ordinal (see the
/// module docs): a synthetic message before the hit does not shift it, a segment end at
/// another ordinal does not match, and an interior removal before the hit is the documented
/// miss.
#[test]
fn the_null_anchor_intersection_matches_mids_at_their_positional_ordinals() {
    let (_dir, store) = open_store();
    // Rows end at m2, m4, .., m10 (ordinals 2, 4, .., 10); the base puts m5 at ordinal 5.
    seed_coverage(&store, 5, Some(5), Some(4));
    // m10 sits at ordinal 9 here, not its stored 10, so the newest row does not match.
    let names: Vec<String> = ["m5", "s1", "m6", "m7", "m8", "m10"]
        .map(String::from)
        .into();
    let resolved = resolve_in(&store, None, &window(&names)).unwrap();
    assert_eq!(resolved.resolution, Resolution::StaleSlice { cut: 4 });
    assert_eq!(resolved.anchor.as_ref().map(|row| row.sequence), Some(4));
    assert_eq!(resolved.ordinals, vec![8, 9]);
    // m5 was removed: m6 and m8 sit one ordinal below their stored ends and are missed.
    let removed = mids(6, 8);
    let resolved = resolve_in(&store, None, &window(&removed)).unwrap();
    assert_eq!(
        resolved.resolution,
        Resolution::Revert {
            keep_through_seq: None
        }
    );
}

/// The plugin's `annotateOrdinals` loops, transcribed: non-synthetic messages are its memoized
/// (resolved) messages, numbered from the head; synthetic messages are unresolved. With no
/// resolved message at all, the plugin's provisional base is the ordinal before the head.
fn plugin_ordinal_model(synthetic: &[bool], anchor: Option<u64>, base: Option<u64>) -> Vec<u64> {
    let first = anchor.unwrap_or(base.unwrap_or(0) + 1);
    let mut next = first;
    let mut resolved: Vec<Option<u64>> = synthetic
        .iter()
        .map(|synthetic| {
            (!synthetic).then(|| {
                next += 1;
                next - 1
            })
        })
        .collect();
    for index in 0..resolved.len() {
        if resolved[index].is_some() || !synthetic[index] {
            continue;
        }
        if !resolved[index + 1..].iter().any(Option::is_some) {
            continue;
        }
        let prior = resolved[..index].iter().rev().find_map(|ordinal| *ordinal);
        resolved[index] = Some(prior.unwrap_or(anchor.unwrap_or(0)));
    }
    let suffix_start = resolved
        .iter()
        .rposition(Option::is_some)
        .map_or(0, |last| last + 1);
    let suffix_base = match suffix_start {
        0 => first - 1,
        start => resolved[start - 1].unwrap(),
    };
    for (offset, ordinal) in resolved[suffix_start..].iter_mut().enumerate() {
        *ordinal = Some(suffix_base + offset as u64 + 1);
    }
    resolved.into_iter().map(Option::unwrap).collect()
}

/// WP-P03: the synthetic rule matches the plugin's for every synthetic pattern up to eight
/// messages, anchored, unanchored, and under a lineage continuation base. The domain is D11's:
/// every non-synthetic message is persisted (resolved in the plugin's memo) and every synthetic
/// one is unpersisted. `annotateOrdinals` also numbers an unpersisted suffix densely, synthetic
/// messages included, which D11 does not adopt: after a resolved `a` at 1, the unpersisted
/// suffix `[s, b]` is `[2, 3]` in the plugin and `[1, 2]` here.
#[test]
fn synthetic_borrowing_matches_the_plugin_rule_case_for_case() {
    for len in 0..=8usize {
        for pattern in 0..(1u32 << len) {
            let synthetic: Vec<bool> = (0..len).map(|bit| pattern & (1 << bit) != 0).collect();
            let names: Vec<String> = synthetic
                .iter()
                .enumerate()
                .map(|(index, synthetic)| format!("{}{index}", if *synthetic { 's' } else { 'm' }))
                .collect();
            let window = window(&names);
            for (anchor, base) in [(None, None), (None, Some(40)), (Some(10), None)] {
                assert_eq!(
                    assign_ordinals(&window, anchor, base),
                    plugin_ordinal_model(&synthetic, anchor, base),
                    "pattern {synthetic:?} anchor {anchor:?} base {base:?}"
                );
            }
        }
    }
}

/// WP-P03: resolved ordinals equal the model with a multi-block anchor, synthetic head,
/// middle, and tail, a lineage continuation, and an interior covered removal.
#[test]
fn resolved_ordinals_equal_the_independent_model() {
    let (_dir, store) = open_store();
    seed_coverage(&store, 2, None, None);
    // Row 3 ends at block 2 of message m5: a multi-block anchor, rendered.
    store
        .with_fenced_conn_for_test(|conn| {
            conn.execute(
                "INSERT INTO history_segments (session_id, sequence, start_message, end_message,
                        start_message_id, end_message_id, title, content)
                 VALUES (?1, 3, 5, 5, 'm5#0', 'm5#2', 't', 'c')",
                [SESSION],
            )?;
            let mut core = CoreState::empty();
            core.boundary_id = "m5#2".to_string();
            let meta = ModuleMeta {
                coverage_ordinal: Some(5),
                ..ModuleMeta::default()
            };
            conn.execute(
                "INSERT INTO cache_state (session_id, row_version, core_state, meta, last_activity_at)
                 VALUES (?1, 1, ?2, ?3, 0)",
                rusqlite::params![
                    SESSION,
                    serde_json::to_string(&core).unwrap(),
                    serde_json::to_string(&meta).unwrap()
                ],
            )
        })
        .unwrap();
    let check = |declared: Option<DeclaredAnchor<'_>>, names: &[&str], cut: usize| {
        let names: Vec<String> = names.iter().map(|name| name.to_string()).collect();
        let window = window(&names);
        let resolved = resolve_in(&store, declared, &window).unwrap();
        assert_eq!(resolved.cut(), cut, "{names:?}");
        let synthetic: Vec<bool> = window[cut..].iter().map(|m| m.synthetic).collect();
        let anchor = resolved.anchor.as_ref().map(|row| row.end_message as u64);
        assert_eq!(
            resolved.ordinals,
            plugin_ordinal_model(&synthetic, anchor, None),
            "{names:?}"
        );
        resolved.ordinals
    };
    let head_middle_tail = ["m5", "s1", "m6", "s2", "m7", "s3", "s4"];
    assert_eq!(
        check(anchor("m5", 3), &head_middle_tail, 0),
        vec![5, 5, 6, 6, 7, 8, 9]
    );
    // A stale declared row with and without the covered interior message m3.
    let whole = check(anchor("m2", 1), &["m2", "m3", "m4", "m5", "s1", "m6"], 3);
    let removed = check(anchor("m2", 1), &["m2", "m4", "m5", "s1", "m6"], 2);
    assert_eq!(whole, vec![5, 5, 6]);
    assert_eq!(removed, whole);
    // No anchor under a lineage continuation base: synthetic head, middle, and tail.
    let names = ["s0", "a", "s1", "b", "s2"].map(String::from);
    let window = window(&names);
    let synthetic: Vec<bool> = window.iter().map(|m| m.synthetic).collect();
    assert_eq!(
        assign_ordinals(&window, None, Some(40)),
        plugin_ordinal_model(&synthetic, None, Some(40))
    );
    assert_eq!(
        assign_ordinals(&window, None, Some(40)),
        vec![0, 41, 41, 42, 43]
    );
}

fn message(mid: &str) -> Keyed<String, Arc<Value>> {
    Keyed {
        key: mid.to_string(),
        value: Arc::new(json!({ "info": { "id": mid }, "parts": [{ "text": mid }] })),
    }
}

/// WP-P05 and WP-P19: at a stale cut of four, input keeps built against the processed
/// window, translated back by the cut, reconstruct the served array from the unsliced input.
#[test]
fn a_stale_cut_keep_reconstructs_the_served_array_from_the_unsliced_input() {
    let (_dir, store) = open_store();
    seed_coverage(&store, 5, Some(5), None);
    let names = mids(6, 13);
    let submitted_window = window(&names);
    let resolved = resolve_in(&store, anchor("m6", 3), &submitted_window).unwrap();
    assert_eq!(resolved.resolution, Resolution::StaleSlice { cut: 4 });
    let cut = resolved.cut();

    let submitted: Vec<_> = names.iter().map(|mid| message(mid)).collect();
    let previous = vec![message("summary")];
    let served: Vec<_> = [message("summary"), message("inserted")]
        .into_iter()
        .chain(submitted[cut..].iter().cloned())
        .collect();
    let mut built = build_operations(&served, &submitted[cut..], Some(&previous), |a, b| a == b);
    assert!(built.operations.iter().any(|op| matches!(
        op,
        Operation::Keep {
            source: Source::Input,
            ..
        }
    )));
    translate_input_keeps(&mut built.operations, cut);
    assert!(built.operations.contains(&Operation::Keep {
        source: Source::Input,
        start: cut as u64,
        count: (names.len() - cut) as u64,
    }));

    let values = |entries: &[Keyed<String, Arc<Value>>]| -> Vec<Arc<Value>> {
        entries
            .iter()
            .map(|entry| Arc::clone(&entry.value))
            .collect()
    };
    let lengths = |values: &[Arc<Value>]| -> Vec<usize> {
        values
            .iter()
            .map(|value| canonical_len(value).unwrap())
            .collect()
    };
    let (input_revision, previous_revision) = (
        Revision::parse("r-input").unwrap(),
        Revision::parse("r-prev").unwrap(),
    );
    let recipe = Recipe {
        base_revision: input_revision.clone(),
        output_revision: Revision::parse("r-out").unwrap(),
        previous_output_revision: Some(previous_revision.clone()),
        operations: built.operations,
    };
    let (input_values, previous_values) = (values(&submitted), values(&previous));
    let (input_lengths, previous_lengths) = (lengths(&input_values), lengths(&previous_values));
    let applied = recipe
        .apply(
            SourceBase {
                revision: &input_revision,
                values: &input_values,
                lengths: &input_lengths,
            },
            Some(SourceBase {
                revision: &previous_revision,
                values: &previous_values,
                lengths: &previous_lengths,
            }),
        )
        .unwrap();
    assert_eq!(applied.values, values(&served));
}

/// WP-P07: the null-anchor intersection and the anchor page visit the same rows at fixed W
/// whatever the stored history's length H.
#[test]
fn intersection_and_page_work_is_independent_of_history_length() {
    let measure = |segments: usize| {
        let (_dir, store) = open_store();
        seed_coverage(&store, segments, Some(segments), None);
        let names = mids(1, 32);
        let window = window(&names);
        store.start_statement_work_ledger();
        let resolved = resolve_in(&store, None, &window).unwrap();
        let page = boundary_page(&store, SESSION, None).unwrap();
        let work = store.take_statement_work();
        assert_eq!(resolved.resolution, Resolution::StaleSlice { cut: 31 });
        assert_eq!(
            page["anchors"].as_array().unwrap().len(),
            BOUNDARY_PAGE_LIMIT
        );
        let of = |needle: &str| -> (u64, u64) {
            let runs: Vec<_> = work.iter().filter(|run| run.sql.contains(needle)).collect();
            assert_eq!(runs.len(), 1, "{needle}: {work:?}");
            (runs[0].rows, runs[0].vm_steps)
        };
        (of("CROSS JOIN history_segments"), of("LIMIT ?3"))
    };
    let (short, long) = (measure(4_500), measure(9_000));
    eprintln!("intersection and page (rows, vm_steps): H=4500 {short:?}, H=9000 {long:?}");
    assert_eq!(short, long);
}

/// A run of rows without a message id longer than a page, below the rendered row and above
/// real anchors, is skipped without ending the walk early.
#[test]
fn a_long_run_of_unlisted_rows_does_not_end_the_walk() {
    let (_dir, store) = open_store();
    seed_coverage(&store, 5_000, Some(5_000), None);
    store
        .with_fenced_conn_for_test(|conn| {
            conn.execute(
                "UPDATE history_segments SET end_message_id = 'unlisted'
                  WHERE session_id = ?1 AND sequence BETWEEN 2 AND 4500",
                [SESSION],
            )
        })
        .unwrap();
    let mut cursor = None;
    let mut walked = Vec::new();
    loop {
        let page = boundary_page(&store, SESSION, cursor).unwrap();
        let anchors: Vec<i64> = page["anchors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|anchor| anchor["sequence"].as_i64().unwrap())
            .collect();
        let Some(last) = anchors.last() else { break };
        cursor = Some(*last);
        walked.extend(anchors);
    }
    let expected: Vec<i64> = (4_501..=5_000).rev().chain([1]).collect();
    assert_eq!(walked, expected);
}

/// Both anchor queries accept `<mid>#[0-9]+` with no other `#`, which agrees with
/// `split_block_id` except for the pinned cases: a signed index, which `usize` parsing accepts,
/// and an index past `usize`, which it refuses.
#[test]
fn the_sql_anchor_grammar_matches_split_block_id() {
    let (_dir, store) = open_store();
    // Row 1 ends at ordinal 2; row 3 is the rendered boundary.
    seed_coverage(&store, 3, Some(3), None);
    let overflow = format!("m#{}0", usize::MAX);
    let cases = [
        ("m1#0", true),
        ("a#b#1", true),
        ("m1#2x", true),
        ("#1", true),
        ("m#", true),
        ("m1#", true),
        ("m#+1", false),
        (overflow.as_str(), false),
    ];
    for (id, agrees) in cases {
        store
            .with_fenced_conn_for_test(|conn| {
                conn.execute(
                    "UPDATE history_segments SET end_message_id = ?2
                      WHERE session_id = ?1 AND sequence = 1",
                    rusqlite::params![SESSION, id],
                )
            })
            .unwrap();
        let split = split_block_id(id).is_some();
        let paged = store
            .coverage_anchor_page(SESSION, i64::MAX, 10)
            .unwrap()
            .iter()
            .any(|(sequence, _)| *sequence == 1);
        let mid = id.rsplit_once('#').map_or(id, |(mid, _)| mid);
        let intersected = store
            .coverage_snapshot(SESSION, None, &["m1", mid])
            .unwrap()
            .newest_window_end
            .is_some();
        assert_eq!(paged, intersected, "{id}: the two queries disagree");
        assert_eq!(
            paged == split,
            agrees,
            "{id}: SQL {paged}, split_block_id {split}"
        );
    }
}
