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

/// A deterministic generator for the operation-sequence test, so a failing seed replays.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[usize::try_from(self.below(items.len() as u64)).unwrap()]
    }
}

/// The table the store should hold after a write sequence, computed from the documented
/// write, eviction, and delete rules without SQL.
#[derive(Default)]
struct Model {
    rows: Vec<StoredCandidate>,
    /// Branches the generated sequences must reach for the comparison to mean anything.
    late_writes_that_wrote: usize,
    late_writes_that_evicted: usize,
    new_rows_evicted_at_once: usize,
    deletes_that_emptied_a_project: usize,
}

fn key_of(row: &StoredCandidate) -> (&str, &str, &str) {
    (
        &row.project_digest,
        &row.evidence_id,
        &row.decision_object_id,
    )
}

impl Model {
    fn write(
        &mut self,
        write: &DescriptorCandidates<'_>,
        caps: CandidateCaps,
    ) -> Option<[usize; 3]> {
        let malformed = write.publish_tip < 0
            || write.created_at_ms < 0
            || write.rows.len() > caps.per_descriptor
            || write.rows.iter().enumerate().any(|(index, row)| {
                row.matched_terms == 0
                    || row.matched_terms > row.decision_terms
                    || write.rows[..index]
                        .iter()
                        .any(|earlier| earlier.decision_object_id == row.decision_object_id)
            });
        if malformed {
            return None;
        }
        let held_tip = self
            .rows
            .iter()
            .filter(|row| {
                row.project_digest == write.project_digest
                    && row.evidence_id == write.evidence_id
                    && row.kernel_incarnation_id == write.kernel_incarnation_id
            })
            .map(|row| row.publish_tip)
            .max();
        let late = held_tip.is_some_and(|held| held > write.publish_tip);
        let held_keys: Vec<String> = self
            .rows
            .iter()
            .filter(|row| {
                row.project_digest == write.project_digest && row.evidence_id == write.evidence_id
            })
            .map(|row| row.decision_object_id.clone())
            .collect();
        let mut written = 0;
        for score in write.rows {
            let key = (
                write.project_digest,
                write.evidence_id,
                score.decision_object_id,
            );
            let existing = self.rows.iter_mut().find(|row| key_of(row) == key);
            let score_bp = score.matched_terms * 10_000 / score.decision_terms;
            match existing {
                Some(row) if late => {
                    if row.kernel_incarnation_id == write.kernel_incarnation_id {
                        row.decision_id = score.decision_id.to_string();
                        row.matched_terms = score.matched_terms;
                        row.decision_terms = score.decision_terms;
                        row.score_bp = score_bp;
                        row.matcher_digest = write.matcher_digest.to_string();
                        row.created_at_ms = write.created_at_ms;
                        written += 1;
                    }
                }
                None if late => {}
                Some(row) => {
                    row.publish_tip = if row.kernel_incarnation_id == write.kernel_incarnation_id {
                        row.publish_tip.max(write.publish_tip)
                    } else {
                        write.publish_tip
                    };
                    row.decision_id = score.decision_id.to_string();
                    row.matched_terms = score.matched_terms;
                    row.decision_terms = score.decision_terms;
                    row.score_bp = score_bp;
                    row.kernel_incarnation_id = write.kernel_incarnation_id.to_string();
                    row.matcher_digest = write.matcher_digest.to_string();
                    row.created_at_ms = write.created_at_ms;
                    written += 1;
                }
                None => {
                    self.rows.push(StoredCandidate {
                        project_digest: write.project_digest.to_string(),
                        evidence_id: write.evidence_id.to_string(),
                        decision_object_id: score.decision_object_id.to_string(),
                        decision_id: score.decision_id.to_string(),
                        matched_terms: score.matched_terms,
                        decision_terms: score.decision_terms,
                        score_bp,
                        publish_tip: write.publish_tip,
                        kernel_incarnation_id: write.kernel_incarnation_id.to_string(),
                        matcher_digest: write.matcher_digest.to_string(),
                        created_at_ms: write.created_at_ms,
                    });
                    written += 1;
                }
            }
        }
        let before = self.rows.len();
        if !late {
            self.rows.retain(|row| {
                row.project_digest != write.project_digest
                    || row.evidence_id != write.evidence_id
                    || write
                        .rows
                        .iter()
                        .any(|score| score.decision_object_id == row.decision_object_id)
            });
        }
        let replaced_away = before - self.rows.len();
        let mut project: Vec<StoredCandidate> = self
            .rows
            .iter()
            .filter(|row| row.project_digest == write.project_digest)
            .cloned()
            .collect();
        let dropped_at_cap = project.len().saturating_sub(caps.per_project);
        project.sort_by(|a, b| {
            (
                a.score_bp,
                a.created_at_ms,
                &a.evidence_id,
                &a.decision_object_id,
            )
                .cmp(&(
                    b.score_bp,
                    b.created_at_ms,
                    &b.evidence_id,
                    &b.decision_object_id,
                ))
        });
        for evicted in &project[..dropped_at_cap] {
            self.rows.retain(|row| key_of(row) != key_of(evicted));
            if evicted.evidence_id == write.evidence_id
                && !held_keys.contains(&evicted.decision_object_id)
            {
                self.new_rows_evicted_at_once += 1;
            }
        }
        if late && written > 0 {
            self.late_writes_that_wrote += 1;
        }
        if late && dropped_at_cap > 0 {
            self.late_writes_that_evicted += 1;
        }
        Some([written, replaced_away, dropped_at_cap])
    }

    fn delete(&mut self, keys: &[CandidateKey<'_>]) -> usize {
        let before = self.rows.len();
        let held: Vec<bool> = [PROJECT, OTHER_PROJECT]
            .iter()
            .map(|project| self.rows.iter().any(|row| row.project_digest == *project))
            .collect();
        self.rows.retain(|row| {
            !keys.iter().any(|key| {
                key_of(row) == (key.project_digest, key.evidence_id, key.decision_object_id)
            })
        });
        for (project, held) in [PROJECT, OTHER_PROJECT].iter().zip(held) {
            if held && !self.rows.iter().any(|row| row.project_digest == *project) {
                self.deletes_that_emptied_a_project += 1;
            }
        }
        before - self.rows.len()
    }

    fn sorted(&self) -> Vec<StoredCandidate> {
        let mut rows = self.rows.clone();
        rows.sort_by(|a, b| key_of(a).cmp(&key_of(b)));
        rows
    }

    /// The decision's rows in reader order strictly after `after`, a
    /// `(score_bp, created_at_ms, evidence_id)` position.
    fn decision_after(
        &self,
        project: &str,
        decision: &str,
        after: Option<&(u32, i64, String)>,
    ) -> Vec<StoredCandidate> {
        let mut rows: Vec<StoredCandidate> = self
            .rows
            .iter()
            .filter(|row| row.project_digest == project && row.decision_object_id == decision)
            .filter(|row| {
                after.is_none_or(|(score, created, evidence)| {
                    row.score_bp < *score
                        || (row.score_bp == *score && row.created_at_ms < *created)
                        || (row.score_bp == *score
                            && row.created_at_ms == *created
                            && row.evidence_id.as_str() > evidence.as_str())
                })
            })
            .cloned()
            .collect();
        reader_order(&mut rows);
        rows
    }

    fn gauge(&self, project: Option<&str>) -> (u64, u64) {
        let rows: Vec<&StoredCandidate> = self
            .rows
            .iter()
            .filter(|row| project.is_none_or(|project| row.project_digest == project))
            .collect();
        let bytes = rows
            .iter()
            .map(|row| {
                row.project_digest.len()
                    + row.evidence_id.len()
                    + row.decision_object_id.len()
                    + row.decision_id.len()
                    + row.kernel_incarnation_id.len()
                    + row.matcher_digest.len()
                    + 40
            })
            .sum::<usize>();
        (rows.len() as u64, bytes as u64)
    }
}

/// Every row with its rowid, read past the store API.
fn raw_rows(conn: &Connection) -> Vec<(i64, StoredCandidate)> {
    conn.prepare(
        "SELECT rowid,project_digest,evidence_id,decision_object_id,decision_id,matched_terms,
                decision_terms,score_bp,publish_tip,kernel_incarnation_id,matcher_digest,
                created_at_ms
           FROM contradiction_candidates ORDER BY rowid",
    )
    .unwrap()
    .query_map([], |row| {
        Ok((
            row.get(0)?,
            StoredCandidate {
                project_digest: row.get(1)?,
                evidence_id: row.get(2)?,
                decision_object_id: row.get(3)?,
                decision_id: row.get(4)?,
                matched_terms: row.get(5)?,
                decision_terms: row.get(6)?,
                score_bp: row.get(7)?,
                publish_tip: row.get(8)?,
                kernel_incarnation_id: row.get(9)?,
                matcher_digest: row.get(10)?,
                created_at_ms: row.get(11)?,
            },
        ))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

const MODEL_EVIDENCE: [&str; 6] = ["ev-0", "ev-1", "ev-2", "ev-3", "ev-é", "ev-10"];
const MODEL_DECISIONS: [&str; 6] = ["d0", "d1", "d2", "d3", "d4", "dé"];
const MODEL_INCARNATIONS: [&str; 2] = [INCARNATION, "fedcba9876543210fedcba9876543210"];
const MODEL_MATCHERS: [&str; 2] = [
    MATCHER,
    "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
];

/// Pages the decision through the store at random limits and checks every page against
/// the model.
fn check_decision_pages(
    store: &MemoryStore,
    model: &Model,
    rng: &mut Lcg,
    project: &str,
    decision: &str,
) {
    let expected = model.decision_after(project, decision, None);
    let mut seen = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let asked = usize::try_from(rng.below(8)).unwrap();
        let page = store
            .candidates_for_decision(project, decision, after.as_deref(), asked)
            .unwrap();
        let limit = asked.clamp(1, MAX_CANDIDATE_PAGE);
        let remaining = expected.len() - seen.len();
        assert_eq!(page.items.len(), limit.min(remaining), "page size");
        assert_eq!(
            page.next.is_some(),
            remaining > limit,
            "next names a further page"
        );
        seen.extend(page.items);
        match page.next {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    assert_eq!(seen, expected, "pages concatenate to the reader order");
}

fn model_write_op<'a>(
    rng: &mut Lcg,
    rows: &'a mut Vec<CandidateScore<'static>>,
) -> (DescriptorCandidates<'a>, CandidateCaps) {
    rows.clear();
    let count = rng.below(6);
    for _ in 0..count {
        let decision_terms = 1 + u32::try_from(rng.below(6)).unwrap();
        let matched_terms = match rng.below(20) {
            0 => 0,
            1 => decision_terms + 1,
            _ => 1 + u32::try_from(rng.below(u64::from(decision_terms))).unwrap(),
        };
        let decision_object_id = rng.pick(&MODEL_DECISIONS);
        if rng.below(12) != 0
            && rows
                .iter()
                .any(|row| row.decision_object_id == decision_object_id)
        {
            continue;
        }
        rows.push(CandidateScore {
            decision_object_id,
            decision_id: rng.pick(&["x", "yyy", "zzzzzz", "é"]),
            matched_terms,
            decision_terms,
        });
    }
    let per_project = [2, 3, 5, 8, 8, 12, 12, 4096][usize::try_from(rng.below(8)).unwrap()];
    let caps = CandidateCaps {
        per_descriptor: if rng.below(10) == 0 { 2 } else { 4 },
        per_project,
    };
    let publish_tip = if rng.below(40) == 0 {
        -1
    } else {
        i64::try_from(rng.below(6)).unwrap()
    };
    let write = DescriptorCandidates {
        project_digest: rng.pick(&[PROJECT, PROJECT, OTHER_PROJECT]),
        evidence_id: rng.pick(&MODEL_EVIDENCE),
        publish_tip,
        kernel_incarnation_id: if rng.below(6) == 0 {
            MODEL_INCARNATIONS[1]
        } else {
            MODEL_INCARNATIONS[0]
        },
        matcher_digest: rng.pick(&MODEL_MATCHERS),
        created_at_ms: i64::try_from(rng.below(4)).unwrap(),
        rows,
    };
    (write, caps)
}

#[test]
fn random_write_and_delete_sequences_match_the_reference_model() {
    let mut coverage = [0_usize; 4];
    for seed in 0..24_u64 {
        let (dir, store) = open();
        let conn = raw(&dir);
        let mut rng = Lcg(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x5851_f42d);
        let mut model = Model::default();
        let mut rows = Vec::new();
        let mut held: Vec<(i64, StoredCandidate)> = Vec::new();
        let mut pending: Option<(String, String, String, usize)> = None;
        for step in 0..220 {
            let context = format!("seed {seed} step {step}");
            if rng.below(8) == 0 {
                let mut keys = Vec::new();
                for _ in 0..rng.below(5) {
                    keys.push(CandidateKey {
                        project_digest: rng.pick(&[PROJECT, OTHER_PROJECT]),
                        evidence_id: rng.pick(&MODEL_EVIDENCE),
                        decision_object_id: rng.pick(&MODEL_DECISIONS),
                    });
                }
                let deleted = store.delete_candidates(&keys).unwrap();
                assert_eq!(deleted, model.delete(&keys), "{context}: delete count");
            } else {
                let (write, caps) = model_write_op(&mut rng, &mut rows);
                let expected = model.write(&write, caps);
                match store.replace_candidates_for_descriptor(&write, caps) {
                    Ok(outcome) => assert_eq!(
                        Some([
                            outcome.written,
                            outcome.replaced_away,
                            outcome.dropped_at_cap
                        ]),
                        expected,
                        "{context}: {write:?} {caps:?}"
                    ),
                    Err(error) => assert_eq!(expected, None, "{context}: {error}"),
                }
            }

            let stored = raw_rows(&conn);
            let mut by_key: Vec<StoredCandidate> =
                stored.iter().map(|(_, row)| row.clone()).collect();
            by_key.sort_by(|a, b| key_of(a).cmp(&key_of(b)));
            assert_eq!(by_key, model.sorted(), "{context}: table contents");
            for (rowid, row) in &stored {
                if let Some((held_rowid, _)) =
                    held.iter().find(|(_, old)| key_of(old) == key_of(row))
                {
                    assert_eq!(rowid, held_rowid, "{context}: a held key keeps its rowid");
                }
            }
            held = stored;
            for project in [Some(PROJECT), Some(OTHER_PROJECT), None] {
                assert_eq!(
                    store.candidate_rows_and_bytes(project).unwrap(),
                    model.gauge(project),
                    "{context}: gauge"
                );
            }
            for project in [PROJECT, OTHER_PROJECT] {
                assert_eq!(
                    store.candidate_rows_and_bytes(Some(project)).unwrap(),
                    gauge_from_rows(&dir, project),
                    "{context}: totals against the rows"
                );
            }

            if let Some((project, decision, cursor, limit)) = pending.take() {
                let page = store
                    .candidates_for_decision(&project, &decision, Some(&cursor), limit)
                    .unwrap();
                let parts: Vec<&str> = cursor.splitn(3, '.').collect();
                let evidence: Vec<u8> = (0..parts[2].len())
                    .step_by(2)
                    .map(|at| u8::from_str_radix(&parts[2][at..at + 2], 16).unwrap())
                    .collect();
                let after = (
                    parts[0].parse().unwrap(),
                    parts[1].parse().unwrap(),
                    String::from_utf8(evidence).unwrap(),
                );
                let expected = model.decision_after(&project, &decision, Some(&after));
                assert_eq!(
                    page.items,
                    expected[..limit.min(expected.len())],
                    "{context}: a cursor continues from its keyset position after a write"
                );
            }
            if step % 7 == 0 {
                let project = rng.pick(&[PROJECT, OTHER_PROJECT]);
                let decision = rng.pick(&MODEL_DECISIONS);
                check_decision_pages(&store, &model, &mut rng, project, decision);
                let first = store
                    .candidates_for_decision(project, decision, None, 1)
                    .unwrap();
                if let Some(next) = first.next {
                    let limit = 1 + usize::try_from(rng.below(4)).unwrap();
                    pending = Some((project.to_string(), decision.to_string(), next, limit));
                }
                let page = usize::try_from(1 + rng.below(5)).unwrap();
                let mut watermark = 0;
                let mut swept = Vec::new();
                loop {
                    let rows = store.candidate_sweep_page(watermark, page).unwrap();
                    assert!(rows.len() <= page, "{context}: sweep page bound");
                    swept.extend(rows.iter().cloned());
                    match rows.last() {
                        Some((rowid, _)) if rows.len() == page => watermark = *rowid,
                        _ => break,
                    }
                }
                assert_eq!(swept, held, "{context}: the sweep walks every row by rowid");
            }
        }
        coverage[0] += model.late_writes_that_wrote;
        coverage[1] += model.late_writes_that_evicted;
        coverage[2] += model.new_rows_evicted_at_once;
        coverage[3] += model.deletes_that_emptied_a_project;
    }
    assert!(
        coverage.iter().all(|reached| *reached > 0),
        "late writes that wrote, late writes that evicted, new rows evicted at once, \
         deletes that emptied a project: {coverage:?}"
    );
}

#[test]
fn the_hex_checks_refuse_uppercase_non_hex_and_misfit_lengths() {
    let (dir, _store) = open();
    let conn = raw(&dir);
    let insert = |project: &str, incarnation: &str, matcher: &str| {
        conn.execute(
            "INSERT INTO contradiction_candidates VALUES (?1,'e','d','d',1,2,5000,0,?2,?3,0)",
            rusqlite::params![project, incarnation, matcher],
        )
        .map_err(|error| {
            assert!(
                error.to_string().contains("CHECK constraint failed"),
                "{error}"
            );
        })
    };
    assert!(insert(PROJECT, INCARNATION, MATCHER).is_ok());
    conn.execute("DELETE FROM contradiction_candidates", [])
        .unwrap();
    let wide = "a".repeat(63);
    for bad in [
        format!("{wide}A"),
        format!("{wide}g"),
        format!("{wide} "),
        format!("{wide}é"),
        wide.clone(),
        "a".repeat(65),
    ] {
        assert!(
            insert(&bad, INCARNATION, MATCHER).is_err(),
            "project {bad:?}"
        );
        assert!(
            insert(PROJECT, INCARNATION, &bad).is_err(),
            "matcher {bad:?}"
        );
    }
    let narrow = "0".repeat(31);
    for bad in [
        format!("{narrow}F"),
        format!("{narrow}x"),
        narrow.clone(),
        "0".repeat(33),
    ] {
        assert!(
            insert(PROJECT, &bad, MATCHER).is_err(),
            "incarnation {bad:?}"
        );
    }
}

/// The gauge computed from the rows themselves, past the store's totals.
fn gauge_from_rows(dir: &tempfile::TempDir, project: &str) -> (u64, u64) {
    let (rows, bytes): (i64, i64) = raw(dir)
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(octet_length(project_digest) + octet_length(evidence_id)
                    + octet_length(decision_object_id) + octet_length(decision_id)
                    + octet_length(kernel_incarnation_id) + octet_length(matcher_digest) + 40), 0)
               FROM contradiction_candidates WHERE project_digest=?1",
            [project],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    (rows.unsigned_abs(), bytes.unsigned_abs())
}

#[test]
fn the_totals_follow_replaced_ids_late_writes_deletes_and_truncation() {
    let (dir, store) = open();
    let ids = ["x", "yyyy", "zzzzzzzzzz", "é", "x"];
    for (step, id) in ids.iter().enumerate() {
        let next = ids[(step + 1) % ids.len()];
        for (evidence, tip) in [("ev-1", 5), ("ev-1", 3), ("ev-2", 5)] {
            let (first, second) = if tip == 3 { (next, *id) } else { (*id, next) };
            let rows = [
                CandidateScore {
                    decision_id: first,
                    ..score("d1", 1, 2)
                },
                CandidateScore {
                    decision_id: second,
                    ..score("d2", 1, 3)
                },
            ];
            let named = if step % 2 == 0 { 2 } else { 1 };
            store
                .replace_candidates_for_descriptor(
                    &write(PROJECT, evidence, tip, 1, &rows[..named]),
                    CAPS,
                )
                .unwrap();
            assert_eq!(
                store.candidate_rows_and_bytes(Some(PROJECT)).unwrap(),
                gauge_from_rows(&dir, PROJECT),
                "step {step}, {evidence} at tip {tip}"
            );
        }
    }
    store
        .delete_candidates(&[CandidateKey {
            project_digest: PROJECT,
            evidence_id: "ev-2",
            decision_object_id: "d1",
        }])
        .unwrap();
    assert_eq!(
        store.candidate_rows_and_bytes(Some(PROJECT)).unwrap(),
        gauge_from_rows(&dir, PROJECT)
    );
    store.truncate_contradiction_candidates_for_test().unwrap();
    assert_eq!(store.candidate_rows_and_bytes(None).unwrap(), (0, 0));
    let totals: i64 = raw(&dir)
        .query_row(
            "SELECT COUNT(*) FROM contradiction_candidate_totals",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(totals, 0, "a project without rows keeps no totals row");
}

#[test]
fn a_new_row_the_cap_evicts_at_once_counts_as_written_and_dropped() {
    let (dir, store) = open();
    let caps = CandidateCaps {
        per_descriptor: 4,
        per_project: 2,
    };
    let high = [score("d1", 4, 4), score("d2", 3, 4)];
    store
        .replace_candidates_for_descriptor(&write(PROJECT, "ev-high", 1, 10, &high), caps)
        .unwrap();
    let held = raw_rows(&raw(&dir));
    let low = [score("d3", 1, 4), score("d4", 1, 5)];
    let outcome = store
        .replace_candidates_for_descriptor(&write(PROJECT, "ev-low", 1, 20, &low), caps)
        .unwrap();
    assert_eq!(
        (outcome.written, outcome.dropped_at_cap),
        (2, 2),
        "both new rows are written and dropped"
    );
    assert_eq!(raw_rows(&raw(&dir)), held, "the held rows and rowids stay");

    let mixed = [score("d5", 4, 4), score("d6", 1, 9)];
    let outcome = store
        .replace_candidates_for_descriptor(&write(PROJECT, "ev-mixed", 1, 30, &mixed), caps)
        .unwrap();
    assert_eq!((outcome.written, outcome.dropped_at_cap), (2, 2));
    let kept: Vec<(String, String)> = all_rows(&store)
        .into_iter()
        .map(|row| (row.evidence_id, row.decision_object_id))
        .collect();
    assert_eq!(
        kept,
        [
            ("ev-high".to_string(), "d1".to_string()),
            ("ev-mixed".to_string(), "d5".to_string())
        ],
        "the new full score displaces the held 3/4 row and the new 1/9 row never lands"
    );
    let highest_held = held.iter().map(|(rowid, _)| *rowid).max().unwrap();
    let landed = raw_rows(&raw(&dir))
        .into_iter()
        .find(|(_, row)| row.evidence_id == "ev-mixed")
        .unwrap();
    assert!(
        landed.0 > highest_held,
        "the new row takes a fresh rowid, not the one its write evicted"
    );
    assert_eq!(
        store.candidate_rows_and_bytes(Some(PROJECT)).unwrap(),
        gauge_from_rows(&dir, PROJECT)
    );
}

#[test]
fn every_candidate_statement_reuses_its_cached_plan() {
    let (_dir, store) = open();
    store.start_statement_reuse_probe();
    let caps = CandidateCaps {
        per_descriptor: 4,
        per_project: 6,
    };
    for index in 0..12_u32 {
        let rows = [
            score("decision", 1 + index % 4, 4),
            score(["d1", "d2", "d3"][index as usize % 3], 1 + index % 3, 3),
        ];
        let evidence = format!("ev-{index}");
        let tip = i64::from(index % 5);
        store
            .replace_candidates_for_descriptor(&write(PROJECT, &evidence, tip, tip, &rows), caps)
            .unwrap();
        let page = store
            .candidates_for_decision(PROJECT, "decision", None, 1 + index as usize % 3)
            .unwrap();
        store
            .candidates_for_decision(PROJECT, "decision", page.next.as_deref(), 2)
            .unwrap();
        store
            .candidate_sweep_page(i64::from(index % 4), 2 + index as usize % 3)
            .unwrap();
        store
            .delete_candidates(&[CandidateKey {
                project_digest: PROJECT,
                evidence_id: &format!("ev-{}", index / 2),
                decision_object_id: "d1",
            }])
            .unwrap();
        store.candidate_rows_and_bytes(Some(PROJECT)).unwrap();
        let rewrite = [score("d1", 1 + index % 2, 2)];
        store
            .replace_candidates_for_descriptor(&write(PROJECT, "ev-0", 9 + tip, 1, &rewrite), caps)
            .unwrap();
        store
            .replace_candidates_for_descriptor(&write(PROJECT, "ev-0", 1, 2, &rewrite), caps)
            .unwrap();
        store
            .replace_candidates_for_descriptor(
                &write(OTHER_PROJECT, "ev-alone", 1, 1, &rewrite),
                caps,
            )
            .unwrap();
        store
            .delete_candidates(&[CandidateKey {
                project_digest: OTHER_PROJECT,
                evidence_id: "ev-alone",
                decision_object_id: "d1",
            }])
            .unwrap();
        store.candidate_rows_and_bytes(None).unwrap();
    }
    let reuse = store.statement_evictions();
    let texts: Vec<&String> = reuse
        .keys()
        .filter(|sql| sql.contains("contradiction_candidate"))
        .collect();
    assert_eq!(texts.len(), 16, "every candidate statement ran: {texts:#?}");
    for sql in texts {
        assert_eq!(reuse[sql], 0, "the statement cache re-created {sql}");
        let reprepared = store
            .with_fenced_conn_for_test(|conn| {
                Ok(conn
                    .prepare_cached(sql)?
                    .get_status(rusqlite::StatementStatus::RePrepare))
            })
            .unwrap();
        assert_eq!(reprepared, 0, "a rebound parameter re-planned {sql}");
    }
}

#[test]
fn only_the_candidate_module_writes_the_candidate_tables() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let writes = [
        "into contradiction_candidate",
        "update contradiction_candidate",
        "delete from contradiction_candidate",
    ];
    let mut files = Vec::new();
    let mut dirs = vec![src.clone()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }
    let mut checked = 0;
    for path in files {
        if path
            .file_name()
            .is_some_and(|name| name == "contradiction_candidates.rs")
        {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .unwrap()
            .to_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        for write in writes {
            assert!(
                !text.contains(write),
                "{} runs `{write}`; the totals would drift",
                path.display()
            );
        }
        checked += 1;
    }
    assert!(checked > 1);
    let (dir, _store) = open();
    for table in ["contradiction_candidates", "contradiction_candidate_totals"] {
        let session_scoped: bool = raw(&dir)
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = 'session_id')",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            !session_scoped,
            "the session purge would delete from {table}"
        );
    }
    let baseline = include_str!("../baseline.sql").to_lowercase();
    for trigger in baseline.split("create trigger").skip(1) {
        let trigger = trigger.split("end;").next().unwrap_or_default();
        let head = trigger.split(" begin").next().unwrap_or_default();
        assert!(
            !trigger.contains("contradiction_candidate"),
            "a trigger touches the candidate tables: {head}"
        );
    }
}

#[test]
fn the_page_sweep_and_eviction_scans_stop_at_their_bound() {
    let (_dir, store) = open();
    let caps = CandidateCaps {
        per_descriptor: 4,
        per_project: 300,
    };
    for index in 0..300_i64 {
        let rows = [score("decision", 3, 4)];
        store
            .replace_candidates_for_descriptor(
                &write(PROJECT, &format!("ev-{index:03}"), 1, index, &rows),
                caps,
            )
            .unwrap();
    }
    store.start_statement_work_ledger();
    store
        .candidates_for_decision(PROJECT, "decision", None, 2)
        .unwrap();
    store.candidate_sweep_page(0, 3).unwrap();
    let low = [score("decision", 1, 4)];
    let outcome = store
        .replace_candidates_for_descriptor(&write(PROJECT, "ev-low", 1, 0, &low), caps)
        .unwrap();
    assert_eq!(outcome.dropped_at_cap, 1);
    let work = store.take_statement_work();
    for needle in [
        "ORDER BY score_bp DESC",
        "ORDER BY rowid",
        "ORDER BY score_bp ASC",
    ] {
        let run = work
            .iter()
            .find(|run| run.sql.contains(needle))
            .unwrap_or_else(|| panic!("no run of {needle}: {work:#?}"));
        assert!(
            run.vm_steps < 500,
            "{needle} took {} VM steps over a 300-row project; the scan must stop at its bound",
            run.vm_steps
        );
    }
}

#[test]
fn a_continuation_page_seeks_to_its_cursor_inside_a_run_of_equal_scores() {
    let (_dir, store) = open();
    let caps = CandidateCaps {
        per_descriptor: 4,
        per_project: 300,
    };
    for (project, created) in [(PROJECT, None), (OTHER_PROJECT, Some(7))] {
        for index in 0..300_i64 {
            let rows = [score("decision", 3, 4)];
            let evidence = format!("ev-{index:03}");
            store
                .replace_candidates_for_descriptor(
                    &write(project, &evidence, 1, created.unwrap_or(index), &rows),
                    caps,
                )
                .unwrap();
        }
        let mut expected: Vec<StoredCandidate> = all_rows(&store)
            .into_iter()
            .filter(|row| row.project_digest == project)
            .collect();
        reader_order(&mut expected);
        let mut cursor = None;
        let mut read = Vec::new();
        for _ in 0..4 {
            let page = store
                .candidates_for_decision(project, "decision", cursor.as_deref(), MAX_CANDIDATE_PAGE)
                .unwrap();
            read.extend(page.items);
            cursor = page.next;
        }
        assert_eq!(read, expected[..256]);
        store.start_statement_work_ledger();
        let page = store
            .candidates_for_decision(project, "decision", cursor.as_deref(), 2)
            .unwrap();
        let work = store.take_statement_work();
        assert_eq!(page.items, expected[256..258]);
        let steps: u64 = work
            .iter()
            .filter(|run| run.sql.contains("contradiction_candidates"))
            .map(|run| run.vm_steps)
            .sum();
        assert!(
            steps < 500,
            "a page 256 rows into a run of equal scores took {steps} VM steps: {work:#?}"
        );
    }
}
