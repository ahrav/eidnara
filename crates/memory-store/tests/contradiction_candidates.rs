use memory_store::MemoryStore;
use memory_store::contradiction_candidates::{
    CandidateCaps, CandidateKey, CandidateScore, DescriptorCandidates, MAX_CANDIDATE_PAGE,
    StoredCandidate,
};
use rusqlite::Connection;

const PROJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OTHER_PROJECT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const INCARNATION: &str = "0123456789abcdef0123456789abcdef";
const MATCHER: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const CAPS: CandidateCaps = CandidateCaps {
    per_descriptor: 4,
    per_project: 4096,
};

fn open() -> (tempfile::TempDir, MemoryStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open_for_test(dir.path(), "eidnara-candidates-test");
    (dir, store)
}

fn score<'a>(decision: &'a str, matched: u32, terms: u32) -> CandidateScore<'a> {
    CandidateScore {
        decision_object_id: decision,
        decision_id: decision,
        matched_terms: matched,
        decision_terms: terms,
    }
}

fn write<'a>(
    project: &'a str,
    evidence: &'a str,
    tip: i64,
    created: i64,
    rows: &'a [CandidateScore<'a>],
) -> DescriptorCandidates<'a> {
    DescriptorCandidates {
        project_digest: project,
        evidence_id: evidence,
        publish_tip: tip,
        kernel_incarnation_id: INCARNATION,
        matcher_digest: MATCHER,
        created_at_ms: created,
        rows,
    }
}

fn all_rows(store: &MemoryStore) -> Vec<StoredCandidate> {
    store
        .candidate_sweep_page(0, 10_000)
        .unwrap()
        .into_iter()
        .map(|(_, row)| row)
        .collect()
}

fn reader_order(rows: &mut [StoredCandidate]) {
    rows.sort_by(|a, b| {
        b.score_bp
            .cmp(&a.score_bp)
            .then(b.created_at_ms.cmp(&a.created_at_ms))
            .then(a.evidence_id.cmp(&b.evidence_id))
    });
}

fn raw(dir: &tempfile::TempDir) -> Connection {
    Connection::open(dir.path().join("memory.sqlite")).unwrap()
}

#[test]
fn each_key_has_one_row_and_a_rewrite_keeps_the_greater_tip() {
    let (_dir, store) = open();
    let first = [score("d1", 3, 4), score("d2", 4, 4)];
    let outcome = store
        .replace_candidates_for_descriptor(&write(PROJECT, "ev-1", 10, 100, &first), CAPS)
        .unwrap();
    assert_eq!(
        (
            outcome.written,
            outcome.replaced_away,
            outcome.dropped_at_cap
        ),
        (2, 0, 0)
    );
    let replay = [score("d1", 4, 5)];
    let outcome = store
        .replace_candidates_for_descriptor(&write(PROJECT, "ev-1", 7, 200, &replay), CAPS)
        .unwrap();
    assert_eq!(outcome.written, 1);
    assert_eq!(outcome.replaced_away, 0, "d2 is above the replay's tip");
    let rows = all_rows(&store);
    assert_eq!(rows.len(), 2);
    let d1 = rows
        .iter()
        .find(|row| row.decision_object_id == "d1")
        .unwrap();
    assert_eq!(
        (d1.matched_terms, d1.decision_terms, d1.score_bp),
        (4, 5, 8_000)
    );
    assert_eq!((d1.publish_tip, d1.created_at_ms), (10, 200));
    let newer = [score("d1", 4, 5)];
    let outcome = store
        .replace_candidates_for_descriptor(&write(PROJECT, "ev-1", 12, 300, &newer), CAPS)
        .unwrap();
    assert_eq!(outcome.replaced_away, 1, "the newer set no longer names d2");
    let rows = all_rows(&store);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].publish_tip, 12);
}

#[test]
fn a_descriptor_over_its_cap_or_with_a_malformed_row_is_refused_whole() {
    let (_dir, store) = open();
    let five = [
        score("d1", 3, 4),
        score("d2", 3, 4),
        score("d3", 3, 4),
        score("d4", 3, 4),
        score("d5", 3, 4),
    ];
    assert!(
        store
            .replace_candidates_for_descriptor(&write(PROJECT, "ev-1", 1, 1, &five), CAPS)
            .is_err()
    );
    for rows in [
        vec![score("d1", 0, 4)],
        vec![score("d1", 5, 4)],
        vec![score("d1", 3, 4), score("d1", 4, 4)],
        vec![score("", 3, 4)],
        vec![score("d\n1", 3, 4)],
    ] {
        assert!(
            store
                .replace_candidates_for_descriptor(&write(PROJECT, "ev-1", 1, 1, &rows), CAPS)
                .is_err()
        );
    }
    let ok = [score("d1", 3, 4)];
    let upper = "A".repeat(64);
    let not_hex = "z".repeat(64);
    for bad in [
        DescriptorCandidates {
            project_digest: &upper,
            ..write(PROJECT, "ev-1", 1, 1, &ok)
        },
        DescriptorCandidates {
            kernel_incarnation_id: "0123",
            ..write(PROJECT, "ev-1", 1, 1, &ok)
        },
        DescriptorCandidates {
            matcher_digest: &not_hex,
            ..write(PROJECT, "ev-1", 1, 1, &ok)
        },
        DescriptorCandidates {
            publish_tip: -1,
            ..write(PROJECT, "ev-1", 1, 1, &ok)
        },
    ] {
        assert!(store.replace_candidates_for_descriptor(&bad, CAPS).is_err());
    }
    assert!(all_rows(&store).is_empty());
}

#[test]
fn an_id_the_scanner_detects_is_refused_and_nothing_is_written() {
    let (_dir, store) = open();
    let secret = "sk-ant-api03-abcdefghijklmnopqrstuvwxyzABCDEFGH12345678";
    let rows = [score(secret, 3, 4)];
    assert!(
        store
            .replace_candidates_for_descriptor(&write(PROJECT, "ev-1", 1, 1, &rows), CAPS)
            .is_err()
    );
    let ok = [score("d1", 3, 4)];
    assert!(
        store
            .replace_candidates_for_descriptor(&write(PROJECT, secret, 1, 1, &ok), CAPS)
            .is_err()
    );
    assert!(all_rows(&store).is_empty());
}

#[test]
fn a_project_at_its_cap_evicts_the_lowest_score_then_the_oldest() {
    let (_dir, store) = open();
    let caps = CandidateCaps {
        per_descriptor: 4,
        per_project: 3,
    };
    let three_of_four = [score("d1", 3, 4)];
    store
        .replace_candidates_for_descriptor(&write(PROJECT, "ev-old", 1, 10, &three_of_four), caps)
        .unwrap();
    store
        .replace_candidates_for_descriptor(&write(PROJECT, "ev-new", 1, 20, &three_of_four), caps)
        .unwrap();
    let full = [score("d1", 4, 4)];
    store
        .replace_candidates_for_descriptor(&write(PROJECT, "ev-full", 1, 5, &full), caps)
        .unwrap();
    store
        .replace_candidates_for_descriptor(
            &write(OTHER_PROJECT, "ev-x", 1, 1, &three_of_four),
            caps,
        )
        .unwrap();
    let outcome = store
        .replace_candidates_for_descriptor(&write(PROJECT, "ev-next", 1, 30, &full), caps)
        .unwrap();
    assert_eq!(outcome.dropped_at_cap, 1);
    let kept: Vec<(String, String)> = all_rows(&store)
        .into_iter()
        .filter(|row| row.project_digest == PROJECT)
        .map(|row| (row.evidence_id, row.decision_object_id))
        .collect();
    assert_eq!(kept.len(), 3);
    assert!(
        !kept.contains(&("ev-old".to_string(), "d1".to_string())),
        "the older of the two lowest-scored rows is evicted"
    );
    assert_eq!(
        store
            .candidate_rows_and_bytes(Some(OTHER_PROJECT))
            .unwrap()
            .0,
        1,
        "another project's rows are untouched"
    );
}

#[test]
fn a_decision_pages_in_reader_order_with_a_stable_cursor() {
    let (_dir, store) = open();
    for index in 0..70_i64 {
        let matched = 3 + u32::try_from(index % 3).unwrap();
        let rows = [score("decision", matched, 5)];
        let evidence = format!("ev-{index:03}");
        store
            .replace_candidates_for_descriptor(
                &write(PROJECT, &evidence, 1, index / 4, &rows),
                CAPS,
            )
            .unwrap();
    }
    let mut expected = all_rows(&store);
    reader_order(&mut expected);
    let first = store
        .candidates_for_decision(PROJECT, "decision", None, 100)
        .unwrap();
    assert_eq!(first.items.len(), MAX_CANDIDATE_PAGE);
    assert_eq!(first.items, expected[..64]);
    let rows = [score("decision", 3, 5)];
    store
        .replace_candidates_for_descriptor(&write(PROJECT, "ev-late", 1, 0, &rows), CAPS)
        .unwrap();
    let second = store
        .candidates_for_decision(PROJECT, "decision", first.next.as_deref(), 64)
        .unwrap();
    let mut rest = expected[64..].to_vec();
    rest.push(
        all_rows(&store)
            .into_iter()
            .find(|row| row.evidence_id == "ev-late")
            .unwrap(),
    );
    reader_order(&mut rest);
    assert_eq!(
        second.items, rest,
        "a row written between pages appears once, after the cursor"
    );
    assert_eq!(second.next, None);
    assert!(
        store
            .candidates_for_decision(PROJECT, "decision", Some("not-a-cursor"), 1)
            .is_err()
    );
    let one = store
        .candidates_for_decision(PROJECT, "decision", None, 0)
        .unwrap();
    assert_eq!(one.items.len(), 1);

    let (_dir, store) = open();
    for index in 0..70_i64 {
        let rows = [score("decision", 3, 4)];
        store
            .replace_candidates_for_descriptor(
                &write(PROJECT, &format!("ev-{index:03}"), 1, index, &rows),
                CAPS,
            )
            .unwrap();
    }
    let page = store
        .candidates_for_decision(PROJECT, "decision", None, 64)
        .unwrap();
    let next = store
        .candidates_for_decision(PROJECT, "decision", page.next.as_deref(), 64)
        .unwrap();
    assert_eq!(
        (page.items.len(), next.items.len(), next.next),
        (64, 6, None)
    );
}

#[test]
fn the_sweep_pages_by_rowid_and_deletes_remove_exactly_the_named_keys() {
    let (dir, store) = open();
    for index in 0..5 {
        let rows = [score("d1", 3, 4), score("d2", 3, 4)];
        store
            .replace_candidates_for_descriptor(
                &write(PROJECT, &format!("ev-{index}"), 1, 1, &rows),
                CAPS,
            )
            .unwrap();
    }
    let page = store.candidate_sweep_page(0, 4).unwrap();
    assert_eq!(page.len(), 4);
    let watermark = page.last().unwrap().0;
    let rest = store.candidate_sweep_page(watermark, 4).unwrap();
    assert_eq!(rest.len(), 4);
    assert!(rest.iter().all(|(rowid, _)| *rowid > watermark));
    let tail = store
        .candidate_sweep_page(rest.last().unwrap().0, 4)
        .unwrap();
    assert_eq!(tail.len(), 2);

    let deleted = store
        .delete_candidates(&[
            CandidateKey {
                project_digest: PROJECT,
                evidence_id: "ev-0",
                decision_object_id: "d1",
            },
            CandidateKey {
                project_digest: PROJECT,
                evidence_id: "ev-missing",
                decision_object_id: "d1",
            },
        ])
        .unwrap();
    assert_eq!(deleted, 1);
    let remaining = all_rows(&store);
    assert_eq!(remaining.len(), 9);
    assert!(
        !remaining
            .iter()
            .any(|row| row.evidence_id == "ev-0" && row.decision_object_id == "d1")
    );

    let (rows, bytes) = store.candidate_rows_and_bytes(Some(PROJECT)).unwrap();
    let expected: u64 = raw(&dir)
        .prepare(
            "SELECT project_digest,evidence_id,decision_object_id,decision_id,
                    kernel_incarnation_id,matcher_digest FROM contradiction_candidates",
        )
        .unwrap()
        .query_map([], |row| {
            let mut total = 5 * 8;
            for column in 0..6 {
                total += row.get::<_, String>(column)?.len();
            }
            Ok(total as u64)
        })
        .unwrap()
        .map(Result::unwrap)
        .sum();
    assert_eq!((rows, bytes), (9, expected));
    assert_eq!(store.candidate_rows_and_bytes(None).unwrap(), (9, expected));
}

#[test]
fn truncating_and_replaying_the_writes_rebuilds_the_same_rows() {
    let (_dir, store) = open();
    let writes = [
        ("ev-1", 5, vec![score("d1", 3, 4), score("d2", 4, 4)]),
        ("ev-2", 6, vec![score("d1", 5, 6)]),
        ("ev-1", 7, vec![score("d2", 4, 5)]),
    ];
    let replay = |store: &MemoryStore, created: i64| {
        for (evidence, tip, rows) in &writes {
            store
                .replace_candidates_for_descriptor(
                    &write(PROJECT, evidence, *tip, created, rows),
                    CAPS,
                )
                .unwrap();
        }
    };
    let without_time = |rows: Vec<StoredCandidate>| {
        let mut rows: Vec<_> = rows
            .into_iter()
            .map(|row| StoredCandidate {
                created_at_ms: 0,
                ..row
            })
            .collect();
        rows.sort_by(|a, b| {
            (&a.evidence_id, &a.decision_object_id).cmp(&(&b.evidence_id, &b.decision_object_id))
        });
        rows
    };
    replay(&store, 100);
    let built = without_time(all_rows(&store));
    store.truncate_contradiction_candidates_for_test().unwrap();
    assert!(all_rows(&store).is_empty());
    replay(&store, 900);
    assert_eq!(without_time(all_rows(&store)), built);
}

#[test]
fn the_table_holds_ids_integers_and_times_and_checks_its_shapes() {
    let (dir, _store) = open();
    let names: Vec<String> = raw(&dir)
        .prepare("SELECT name FROM pragma_table_info('contradiction_candidates') ORDER BY cid")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        names,
        [
            "project_digest",
            "evidence_id",
            "decision_object_id",
            "decision_id",
            "matched_terms",
            "decision_terms",
            "score_bp",
            "publish_tip",
            "kernel_incarnation_id",
            "matcher_digest",
            "created_at_ms",
        ]
    );
    let insert = |evidence: &str, score_bp: i64, matched: i64| {
        raw(&dir).execute(
            "INSERT INTO contradiction_candidates VALUES (?1,?2,'d','d',?3,4,?4,0,?5,?6,0)",
            rusqlite::params![PROJECT, evidence, matched, score_bp, INCARNATION, MATCHER],
        )
    };
    assert!(insert("ok", 7_500, 3).is_ok());
    assert!(
        insert("bad-score", 7_000, 3).is_err(),
        "score_bp must equal its derivation"
    );
    assert!(
        insert("over", 12_500, 5).is_err(),
        "matched_terms exceeds decision_terms"
    );
}

#[test]
fn a_late_write_replaces_held_keys_and_adds_none() {
    let (_dir, store) = open();
    let held = [score("d1", 3, 4), score("d2", 3, 4)];
    store
        .replace_candidates_for_descriptor(&write(PROJECT, "ev-1", 10, 1, &held), CAPS)
        .unwrap();
    let late = [score("d2", 4, 4), score("d3", 4, 4)];
    let outcome = store
        .replace_candidates_for_descriptor(&write(PROJECT, "ev-1", 7, 2, &late), CAPS)
        .unwrap();
    assert_eq!(
        (outcome.written, outcome.replaced_away),
        (1, 0),
        "only d2 is replaced"
    );
    let rows = all_rows(&store);
    let ids: Vec<&str> = rows
        .iter()
        .map(|row| row.decision_object_id.as_str())
        .collect();
    assert_eq!(ids, ["d1", "d2"]);
    assert!(rows.iter().all(|row| row.publish_tip == 10));
    assert_eq!(rows[1].score_bp, 10_000);
    let equal = [score("d3", 3, 4)];
    let outcome = store
        .replace_candidates_for_descriptor(&write(PROJECT, "ev-1", 10, 3, &equal), CAPS)
        .unwrap();
    assert_eq!(
        outcome.replaced_away, 2,
        "a write at the held tip replaces the set"
    );
}

#[test]
fn a_write_under_another_kernel_incarnation_takes_its_own_tip() {
    let (_dir, store) = open();
    let rows = [score("d1", 3, 4)];
    store
        .replace_candidates_for_descriptor(&write(PROJECT, "ev-1", 50, 1, &rows), CAPS)
        .unwrap();
    let other = DescriptorCandidates {
        kernel_incarnation_id: "fedcba9876543210fedcba9876543210",
        ..write(PROJECT, "ev-1", 3, 2, &rows)
    };
    store
        .replace_candidates_for_descriptor(&other, CAPS)
        .unwrap();
    let stored = all_rows(&store);
    assert_eq!(stored.len(), 1);
    assert_eq!(
        (
            stored[0].kernel_incarnation_id.as_str(),
            stored[0].publish_tip
        ),
        ("fedcba9876543210fedcba9876543210", 3)
    );
}

#[test]
fn the_page_cursor_is_a_keyset_position_and_only_issued_cursors_parse() {
    let (_dir, store) = open();
    for index in 0..10_i64 {
        let rows = [score("decision", 3, 4)];
        store
            .replace_candidates_for_descriptor(
                &write(PROJECT, &format!("ev-{index}"), 1, index, &rows),
                CAPS,
            )
            .unwrap();
    }
    let first = store
        .candidates_for_decision(PROJECT, "decision", None, 4)
        .unwrap();
    let mut expected = all_rows(&store);
    reader_order(&mut expected);
    let before = [score("decision", 4, 4)];
    store
        .replace_candidates_for_descriptor(&write(PROJECT, "ev-top", 1, 99, &before), CAPS)
        .unwrap();
    store
        .delete_candidates(&[CandidateKey {
            project_digest: PROJECT,
            evidence_id: &first.items[0].evidence_id,
            decision_object_id: "decision",
        }])
        .unwrap();
    let second = store
        .candidates_for_decision(PROJECT, "decision", first.next.as_deref(), 4)
        .unwrap();
    assert_eq!(
        second.items,
        expected[4..8],
        "rows added before the cursor or removed from page one move nothing after it"
    );
    for cursor in [
        "01.5.65",
        "7500.5.6A",
        "7500.5.616",
        "+7500.5.65",
        "4294967296.5.65",
        "7500.5.",
    ] {
        assert!(
            store
                .candidates_for_decision(PROJECT, "decision", Some(cursor), 4)
                .is_err(),
            "{cursor}"
        );
    }
}

#[test]
fn the_caps_hold_at_their_boundaries_and_refusals_leave_held_rows() {
    let (_dir, store) = open();
    let four = [
        score("d1", 3, 4),
        score("d2", 3, 4),
        score("d3", 3, 4),
        score("d4", 3, 4),
    ];
    store
        .replace_candidates_for_descriptor(&write(PROJECT, "ev-1", 1, 1, &four), CAPS)
        .unwrap();
    let five = [
        score("e1", 3, 4),
        score("e2", 3, 4),
        score("e3", 3, 4),
        score("e4", 3, 4),
        score("e5", 3, 4),
    ];
    let refused = store
        .replace_candidates_for_descriptor(&write(PROJECT, "ev-1", 2, 2, &five), CAPS)
        .unwrap_err();
    assert!(
        matches!(refused, memory_store::MemoryStoreError::Serde(_)),
        "{refused}"
    );
    let secret = "sk-ant-api03-abcdefghijklmnopqrstuvwxyzABCDEFGH12345678";
    let refused = store
        .replace_candidates_for_descriptor(
            &write(PROJECT, "ev-1", 2, 2, &[score(secret, 3, 4)]),
            CAPS,
        )
        .unwrap_err();
    assert!(
        matches!(refused, memory_store::MemoryStoreError::Redaction(_)),
        "{refused}"
    );
    assert_eq!(all_rows(&store).len(), 4, "the held rows are unchanged");

    let caps = CandidateCaps {
        per_descriptor: 4,
        per_project: 2,
    };
    let outcome = store
        .replace_candidates_for_descriptor(&write(PROJECT, "ev-2", 1, 1, &four[..2]), caps)
        .unwrap();
    assert_eq!(
        outcome.dropped_at_cap, 4,
        "six rows over a cap of two evict four"
    );
    let kept: Vec<(String, String)> = all_rows(&store)
        .into_iter()
        .map(|row| (row.evidence_id, row.decision_object_id))
        .collect();
    assert_eq!(
        kept,
        [
            ("ev-2".to_string(), "d1".to_string()),
            ("ev-2".to_string(), "d2".to_string())
        ],
        "equal score and age evict the smallest key first"
    );
}

#[test]
fn a_keyed_delete_names_its_project_and_the_gauge_counts_bytes() {
    let (_dir, store) = open();
    let rows = [score("décision-ü", 3, 4)];
    store
        .replace_candidates_for_descriptor(&write(PROJECT, "ev-é", 1, 1, &rows), CAPS)
        .unwrap();
    store
        .replace_candidates_for_descriptor(&write(OTHER_PROJECT, "ev-é", 1, 1, &rows), CAPS)
        .unwrap();
    let deleted = store
        .delete_candidates(&[CandidateKey {
            project_digest: PROJECT,
            evidence_id: "ev-é",
            decision_object_id: "décision-ü",
        }])
        .unwrap();
    assert_eq!(deleted, 1);
    let bytes = 64 + "ev-é".len() + 2 * "décision-ü".len() + 32 + 64 + 40;
    assert_eq!(
        store.candidate_rows_and_bytes(Some(OTHER_PROJECT)).unwrap(),
        (1, bytes as u64)
    );
    assert_eq!(
        store.candidate_rows_and_bytes(Some(PROJECT)).unwrap(),
        (0, 0)
    );
}
