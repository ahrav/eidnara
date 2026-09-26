//! This module reads a session's durable state.
//! The durable state includes history_segments, the user profile, and project docs; the caller supplies the pass's canonical memory rows.
//! This module composes frozen m0 bytes and watermarks that HARD persists.
//!
//! This module produces bytes but does not classify HARD versus SOFT.
//! `apply_once` feeds these bytes into the cache core.
//! This module returns identical bytes for identical store contents, `now_ms`, and `budget`.
//! The caller supplies frozen `now_ms`; this module never reads a live clock.

use retrieval::packing::skip_and_continue;

use memory_store::{MemoryStore, MemoryStoreError};

use crate::canonical_memory::CanonicalMemory;
use crate::decay_render::{
    DecayRenderHistorySegment, PRESSURE_WINDOW, extract_m0_block, fold_horizon,
};
use crate::memory_render::{
    M0Inputs, is_positive_memory_category, render_m0, render_memory_block, render_memory_line,
};
use crate::project_docs::read_project_docs_canonical;

/// Frozen m0 bytes and watermarks persisted atomically by a HARD pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct M0Composition {
    /// `m0_bytes` contains docs, the profile, decayed history_segments, and memories.
    pub m0_bytes: String,
    /// `boundary_id` anchors cache reverts at the last covered raw message.
    /// `boundary_id` is empty when no history_segments are summarized, leaving the live array as the tail.
    pub boundary_id: String,
    /// `coverage_ordinal` marks m0's tail-trim point.
    /// `coverage_ordinal` is `None` when no history_segments exist.
    pub coverage_ordinal: Option<u64>,
    /// `first_covered_ordinal` is the first covered ordinal; the caller rejects live items below it to prevent trimming an uncovered leading gap.
    pub first_covered_ordinal: Option<u64>,
    /// `folded_history_segment_seq` advances only on a HARD.
    pub folded_history_segment_seq: i64,
    /// `docs_hash` records the project-docs version included in m0; it does not trigger HARD.
    /// The next natural HARD re-reads current docs.
    pub docs_hash: String,
    /// The legacy sequences this composition read, to persist in `ModuleMeta`.
    pub legacy_history_segment_seqs: Vec<i64>,
}

/// Frozen inputs for one deterministic m0 composition.
pub struct M0ComposeInputs<'a> {
    pub session_id: &'a str,
    /// `project_path` comes from route binding and cannot be overridden by request content.
    pub project_path: &'a str,
    /// `project_directory` identifies the directory containing `ARCHITECTURE.md` and `STRUCTURE.md`.
    pub project_directory: &'a str,
    pub now_ms: i64,
    /// `history_budget_tokens` limits frozen rendering.
    /// The renderer produces estimator-independent output when all history_segments fit.
    pub history_budget_tokens: f64,
    /// System-role content covered by the current fold.
    pub covered_system_messages: &'a [String],
    /// Disabled memory removes both project memories and the user-profile memory block.
    pub memory_enabled: bool,
    pub user_profile_budget_tokens: f64,
    /// `inject_docs` matches whether the TypeScript materializer includes the project-docs block.
    pub inject_docs: bool,
    /// `temporal_awareness` gates temporal heading dates at render time, including rows persisted by a prior pass.
    pub temporal_awareness: bool,
    /// The persisted legacy sequence list; `None` captures it with one scan.
    pub legacy_history_segment_seqs: Option<&'a [i64]>,
}

/// Keeps the memories whose rendered lines fit `budget_tokens`, in the supplied
/// serving order, charging each category's wrapper once. A row that does not
/// fit is skipped, so a later smaller row can still be admitted. Only positive
/// categories are charged, matching what the renderer emits.
pub(crate) fn trim_memories_to_budget(
    memories: &[CanonicalMemory],
    budget_tokens: f64,
    estimate_tokens: impl Fn(&str) -> usize + Copy,
) -> Vec<CanonicalMemory> {
    let budget = budget_tokens.max(1.0);
    let wrapper = estimate_tokens("<project-memory>\n</project-memory>");
    if wrapper as f64 > budget {
        return Vec::new();
    }
    let positive: Vec<&CanonicalMemory> = memories
        .iter()
        .filter(|memory| is_positive_memory_category(&memory.category))
        .collect();
    let mut open_categories: Vec<&str> = Vec::new();
    let scan = skip_and_continue(
        budget.floor() as u64 - wrapper as u64,
        positive.len(),
        &mut open_categories,
        |open_categories, index| {
            let memory = positive[index];
            let mut cost = estimate_tokens(&(render_memory_line(memory) + "\n"));
            if !open_categories.contains(&memory.category.as_str()) {
                cost +=
                    estimate_tokens(&format!("<{}>\n</{}>\n", memory.category, memory.category));
            }
            cost as u64
        },
        |open_categories, index| {
            let category = positive[index].category.as_str();
            if !open_categories.contains(&category) {
                open_categories.push(category);
            }
        },
    );
    scan.admitted
        .iter()
        .map(|index| positive[*index].clone())
        .collect()
}

pub(crate) fn trim_user_profile_to_budget(
    profile: Vec<String>,
    budget_tokens: f64,
    estimate_tokens: impl Fn(&str) -> usize + Copy,
) -> Vec<String> {
    let mut used = 0usize;
    profile
        .into_iter()
        .filter(|content| {
            let cost = estimate_tokens(&format!("- {content}")) + 4;
            if (used + cost) as f64 > budget_tokens.max(1.0) {
                return false;
            }
            used += cost;
            true
        })
        .collect()
}

/// The history estimate counts only the rendered `<session-history>` slice to match the history budget.
fn history_slice_tokens(m0_text: &str, estimate_tokens: impl Fn(&str) -> usize) -> usize {
    extract_m0_block(m0_text, "session-history").map_or(0, |slice| estimate_tokens(&slice))
}

/// The renderer retries at most three times while history tokens exceed 105% of a positive budget.
/// After three retries, the function returns the last render even if history tokens exceed 105% of the budget.
fn render_m0_with_decay_pressure_retry(
    inputs: &M0Inputs<'_>,
    estimate_tokens: impl Fn(&str) -> usize + Copy,
) -> String {
    let render = |decay_pressure_multiplier| {
        render_m0(
            &M0Inputs {
                project_docs: inputs.project_docs,
                user_profile: inputs.user_profile,
                covered_system_messages: inputs.covered_system_messages,
                history_segments: inputs.history_segments,
                history_budget_tokens: inputs.history_budget_tokens,
                decay_pressure_multiplier,
            },
            estimate_tokens,
        )
    };
    let mut decay_pressure_multiplier = 1.0;
    let mut m0_bytes = render(decay_pressure_multiplier);
    let mut attempts = 0;
    while inputs.history_budget_tokens > 0.0
        && history_slice_tokens(&m0_bytes, estimate_tokens) as f64
            > inputs.history_budget_tokens * 1.05
        && attempts < 3
    {
        decay_pressure_multiplier *= 1.15;
        m0_bytes = render(decay_pressure_multiplier);
        attempts += 1;
    }
    m0_bytes
}

/// Composes m0 from durable state and the pass's pinned canonical memory rows.
///
/// `memories` are already trimmed to the pass's memory budget by the reader
/// that pinned them, so the block renders every row it is given.
pub fn compose_m0(
    store: &MemoryStore,
    inputs: &M0ComposeInputs<'_>,
    memories: &[CanonicalMemory],
    estimate_tokens: impl Fn(&str) -> usize + Copy,
) -> Result<M0Composition, MemoryStoreError> {
    // Read only rows the curve can render: newest non-legacy rows up to the budget's horizon,
    // plus every legacy row. Older rows archive at every retry multiplier, so the bytes match a
    // full read. Appends enforce range order, so coverage comes from the two ends.
    let fold = store.load_history_segment_fold(
        inputs.session_id,
        inputs.legacy_history_segment_seqs,
        PRESSURE_WINDOW,
        |newest| {
            let importances: Vec<i32> = newest.iter().map(|row| row.importance).collect();
            fold_horizon(&importances, inputs.history_budget_tokens)
        },
    )?;
    let history_segments = fold.history_segments;
    let (boundary_id, coverage_ordinal, first_covered_ordinal, folded_history_segment_seq) =
        match (&fold.newest, &fold.oldest) {
            (Some(newest), Some(oldest)) => (
                newest.end_message_id.clone(),
                Some(newest.end_message as u64),
                Some(oldest.start_message as u64),
                newest.sequence,
            ),
            _ => (String::new(), None, None, 0),
        };

    let selected_memories: &[CanonicalMemory] = if inputs.memory_enabled { memories } else { &[] };

    let user_profile = if inputs.memory_enabled {
        store.load_active_user_memories()?
    } else {
        Vec::new()
    };
    let user_profile = trim_user_profile_to_budget(
        user_profile,
        inputs.user_profile_budget_tokens,
        estimate_tokens,
    );
    let docs = if inputs.inject_docs {
        read_project_docs_canonical(inputs.project_directory)
    } else {
        crate::project_docs::ProjectDocs::default()
    };

    let decay_history_segments: Vec<DecayRenderHistorySegment> = history_segments
        .iter()
        .map(|history_segment| {
            let mut rendered = DecayRenderHistorySegment::from(history_segment);
            if !inputs.temporal_awareness {
                rendered.start_date = None;
                rendered.end_date = None;
            }
            rendered
        })
        .collect();
    let mut m0_bytes = render_m0_with_decay_pressure_retry(
        &M0Inputs {
            project_docs: &docs.rendered_block,
            user_profile: &user_profile,
            covered_system_messages: inputs.covered_system_messages,
            history_segments: &decay_history_segments,
            history_budget_tokens: inputs.history_budget_tokens,
            decay_pressure_multiplier: 1.0,
        },
        estimate_tokens,
    );
    let project_memory = render_memory_block(selected_memories, "project-memory");
    if !project_memory.is_empty() {
        m0_bytes.push_str("\n\n");
        m0_bytes.push_str(&project_memory);
    }

    Ok(M0Composition {
        m0_bytes,
        boundary_id,
        coverage_ordinal,
        first_covered_ordinal,
        folded_history_segment_seq,
        docs_hash: docs.canonical_hash,
        legacy_history_segment_seqs: fold.legacy_seqs,
    })
}

#[cfg(test)]
mod trim_memories_tests {
    use proptest::prelude::*;

    use super::*;

    fn memory(category: &str, content: &str) -> CanonicalMemory {
        CanonicalMemory {
            object_id: format!("{category}-{}", content.len()),
            category: category.to_owned(),
            content: content.to_owned(),
        }
    }

    /// The loop the delegating implementation replaced, kept as the reference.
    fn reference(
        memories: &[CanonicalMemory],
        budget_tokens: f64,
        estimate_tokens: impl Fn(&str) -> usize + Copy,
    ) -> Vec<CanonicalMemory> {
        let budget = budget_tokens.max(1.0);
        let mut selected = Vec::new();
        let mut categories = std::collections::HashSet::<&str>::new();
        let mut used = estimate_tokens("<project-memory>\n</project-memory>") as f64;
        for memory in memories
            .iter()
            .filter(|memory| is_positive_memory_category(&memory.category))
        {
            let mut cost = estimate_tokens(&(render_memory_line(memory) + "\n"));
            if !categories.contains(memory.category.as_str()) {
                cost +=
                    estimate_tokens(&format!("<{}>\n</{}>\n", memory.category, memory.category));
            }
            if used + cost as f64 > budget {
                continue;
            }
            used += cost as f64;
            categories.insert(memory.category.as_str());
            selected.push(memory.clone());
        }
        selected
    }

    #[test]
    fn budget_boundaries_match_the_replaced_loop() {
        let bytes = |text: &str| text.len();
        let memories = [
            memory("ARCHITECTURE", "alpha"),
            memory("ARCHITECTURE", "beta beta"),
            memory("NAMING", "gamma"),
            memory("WARNING", "never rendered"),
        ];
        assert_eq!(
            trim_memories_to_budget(&memories, 1e6, bytes).len(),
            3,
            "the positive rows are admitted under a generous budget"
        );
        let wrapper = bytes("<project-memory>\n</project-memory>") as f64;
        for budget in [
            f64::NAN,
            -3.0,
            0.0,
            wrapper - 1.0,
            wrapper,
            wrapper + 10.5,
            wrapper + 40.0,
            wrapper + 40.5,
            wrapper + 41.0,
            1e30,
            f64::INFINITY,
        ] {
            assert_eq!(
                trim_memories_to_budget(&memories, budget, bytes),
                reference(&memories, budget, bytes),
                "budget {budget}"
            );
        }
    }

    proptest! {
        #[test]
        fn skip_and_continue_delegation_matches_the_replaced_loop(
            rows in proptest::collection::vec((0u8..3, 1usize..40), 0..12),
            budget in prop_oneof![Just(f64::NAN), -50.0f64..400.0],
        ) {
            let memories: Vec<CanonicalMemory> = rows
                .iter()
                .map(|(category, len)| {
                    memory(
                        ["ARCHITECTURE", "NAMING", "WARNING"][*category as usize],
                        &"x".repeat(*len),
                    )
                })
                .collect();
            let bytes = |text: &str| text.len();
            prop_assert_eq!(
                trim_memories_to_budget(&memories, budget, bytes),
                reference(&memories, budget, bytes)
            );
        }
    }
}

#[cfg(test)]
mod bounded_read_tests {
    use context_core::decay::{Tier, rendered_tier};
    use memory_store::{ModuleMeta, StoredHistorySegment};

    use super::*;
    use crate::decay_render::PRESSURE_WINDOW;
    use crate::history_segment_coverage::resolve_coverage;
    use crate::m1_compose::compose_m1;
    use crate::memory_render::{M1_PLACEHOLDER, assemble_m1, render_new_history_segments};
    use crate::test_support::synthetic_history::SyntheticHistory;

    const SESSION: &str = "ses";
    const CAPTURE_SCAN: &str = "SELECT sequence, start_message, end_message, legacy FROM";

    fn open(dir: &std::path::Path) -> MemoryStore {
        MemoryStore::open(&crate::test_support::descriptor(dir)).expect("open store")
    }

    fn inputs(budget: f64, legacy: Option<&[i64]>) -> M0ComposeInputs<'_> {
        M0ComposeInputs {
            session_id: SESSION,
            project_path: "git:proj",
            project_directory: "/nonexistent-docs",
            now_ms: 0,
            history_budget_tokens: budget,
            covered_system_messages: &[],
            memory_enabled: false,
            user_profile_budget_tokens: 0.0,
            inject_docs: false,
            temporal_awareness: true,
            legacy_history_segment_seqs: legacy,
        }
    }

    /// m0 over `rows` exactly as `compose_m0` renders it with memory and docs off.
    fn render_rows(rows: &[StoredHistorySegment], budget: f64) -> String {
        let mapped: Vec<DecayRenderHistorySegment> =
            rows.iter().map(DecayRenderHistorySegment::from).collect();
        render_m0_with_decay_pressure_retry(
            &M0Inputs {
                project_docs: "",
                user_profile: &[],
                covered_system_messages: &[],
                history_segments: &mapped,
                history_budget_tokens: budget,
                decay_pressure_multiplier: 1.0,
            },
            tokenizer::estimate_tokens,
        )
    }

    /// The reference: today's read of every row, then the unchanged renderer.
    fn full_read_m0(store: &MemoryStore, budget: f64) -> (String, Option<(String, u64, u64, i64)>) {
        let rows = store
            .load_history_segments(SESSION)
            .expect("load every row");
        let coverage = resolve_coverage(&rows).expect("valid ranges").map(|c| {
            (
                c.boundary_id,
                c.coverage_end_ordinal,
                c.first_covered_ordinal,
                c.max_sequence,
            )
        });
        (render_rows(&rows, budget), coverage)
    }

    fn bounded_m0(store: &MemoryStore, budget: f64, legacy: Option<&[i64]>) -> M0Composition {
        compose_m0(
            store,
            &inputs(budget, legacy),
            &[],
            tokenizer::estimate_tokens,
        )
        .expect("compose m0")
    }

    fn assert_bounded_matches_full(store: &MemoryStore, budget: f64) -> M0Composition {
        let (reference, coverage) = full_read_m0(store, budget);
        let bounded = bounded_m0(store, budget, None);
        assert_eq!(
            bounded.m0_bytes, reference,
            "m0 bytes differ at budget {budget}"
        );
        assert_eq!(
            coverage,
            bounded.coverage_ordinal.map(|end| {
                (
                    bounded.boundary_id.clone(),
                    end,
                    bounded.first_covered_ordinal.expect("start with end"),
                    bounded.folded_history_segment_seq,
                )
            }),
            "coverage differs at budget {budget}"
        );
        let listed = bounded_m0(store, budget, Some(&bounded.legacy_history_segment_seqs));
        assert_eq!(listed, bounded, "a persisted list reads what the scan read");
        bounded
    }

    /// The largest curve index importance 100 keeps renderable under the pressure the newest
    /// window sets, without the window floor `fold_horizon` applies.
    fn renderable_horizon(rows: &[StoredHistorySegment], budget: f64) -> usize {
        let importances: Vec<i32> = rows
            .iter()
            .rev()
            .filter(|row| row.legacy != 1)
            .take(PRESSURE_WINDOW)
            .map(|row| row.importance.clamp(1, 100))
            .collect();
        let pressure = if budget > 0.0 {
            context_core::decay::compute_budget_pressure(&importances, budget)
        } else {
            1.0
        };
        (1..=2_484u32)
            .rev()
            .find(|index| rendered_tier(*index, 100, pressure, 0.0) != Tier::P5)
            .unwrap_or(0) as usize
    }

    /// The newest `count` non-legacy rows, optionally with every legacy row, oldest first.
    fn newest_rows(
        rows: &[StoredHistorySegment],
        count: usize,
        with_legacy: bool,
    ) -> Vec<StoredHistorySegment> {
        let mut newest: Vec<_> = rows
            .iter()
            .rev()
            .filter(|row| row.legacy != 1)
            .take(count)
            .cloned()
            .collect();
        if with_legacy {
            newest.extend(rows.iter().filter(|row| row.legacy == 1).cloned());
        }
        newest.sort_by_key(|row| row.sequence);
        newest
    }

    /// m1 over the rows above `folded` at the default row cap, with memory off.
    fn m1_above(store: &MemoryStore, folded: i64) -> crate::m1_compose::M1Composition {
        let meta = ModuleMeta {
            folded_history_segment_seq: folded,
            coverage_ordinal: Some(0),
            ..ModuleMeta::default()
        };
        compose_m1(
            store,
            "git:proj",
            SESSION,
            &meta,
            0,
            false,
            0.0,
            true,
            crate::m1_compose::DEFAULT_M1_ROW_CAP,
            tokenizer::estimate_tokens,
        )
        .expect("compose m1")
    }

    fn history_segment_work(store: &MemoryStore) -> Vec<storage::StatementWork> {
        store
            .take_statement_work()
            .into_iter()
            .filter(|work| work.sql.contains("history_segments"))
            .collect()
    }

    #[test]
    fn bounded_fold_matches_the_full_read_over_the_store_shape_fixture() {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Raw {
            sequence: i64,
            start_message: i64,
            end_message: i64,
            title: String,
            content: String,
            start_date: Option<String>,
            end_date: Option<String>,
            p1: Option<String>,
            p2: Option<String>,
            p3: Option<String>,
            p4: Option<String>,
            importance: Option<i32>,
            legacy: Option<i32>,
        }
        #[derive(serde::Deserialize)]
        struct Shape {
            history_segments: Vec<Raw>,
        }
        #[derive(serde::Deserialize)]
        struct Case {
            budget: f64,
        }
        #[derive(serde::Deserialize)]
        struct Differential {
            cases: Vec<Case>,
        }
        let shape: Shape =
            serde_json::from_str(include_str!("../testdata/decay-store-shape.json")).unwrap();
        let differential: Differential =
            serde_json::from_str(include_str!("../testdata/decay-store-differential.json"))
                .unwrap();
        let rows: Vec<StoredHistorySegment> = shape
            .history_segments
            .into_iter()
            .map(|raw| StoredHistorySegment {
                sequence: raw.sequence,
                start_message: raw.start_message,
                end_message: raw.end_message,
                end_message_id: format!("m{}#0", raw.end_message),
                title: raw.title,
                content: raw.content,
                start_date: raw.start_date,
                end_date: raw.end_date,
                p1: raw.p1,
                p2: raw.p2,
                p3: raw.p3,
                p4: raw.p4,
                importance: raw.importance.unwrap_or(50),
                legacy: raw.legacy.unwrap_or(0),
                ..Default::default()
            })
            .collect();
        assert_eq!(rows.len(), 388);
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path());
        store.replace_history_segments(SESSION, &rows).unwrap();
        assert_eq!(differential.cases.len(), 4);
        for case in &differential.cases {
            assert_bounded_matches_full(&store, case.budget);
        }
    }

    /// WP-P08 over a 60,000-segment session with mixed importances and legacy rows older
    /// than the largest renderable index, across a geometric budget sweep; the two named
    /// falsifiers must each differ from the reference somewhere in the sweep.
    #[test]
    fn bounded_fold_matches_the_full_read_over_sixty_thousand_segments() {
        let history = SyntheticHistory::mixed(60_000);
        let rows = history.rows();
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path());
        history.seed(&store, SESSION);

        store.start_statement_work_ledger();
        let captured = bounded_m0(&store, 60_000.0, None);
        let scan = history_segment_work(&store)
            .into_iter()
            .find(|work| work.sql.starts_with(CAPTURE_SCAN))
            .expect("the first fold captures the legacy list with one scan");
        println!(
            "legacy-sequence capture scan at H = 60,000: rows = {}, vm_steps = {}",
            scan.rows, scan.vm_steps
        );
        assert_eq!(
            scan.rows as usize,
            rows.len(),
            "the scan verifies every range"
        );
        assert_eq!(
            captured.legacy_history_segment_seqs.len(),
            history.legacy_count()
        );

        let mut budgets = vec![];
        let mut budget = 20.0f64;
        while budget < 10_000_000.0 {
            budgets.push(budget);
            budget *= 3.0;
        }
        budgets.push(10_000_000.0);
        let (mut pressure_falsified, mut legacy_falsified) = (false, false);
        for &budget in &budgets {
            let bounded = assert_bounded_matches_full(&store, budget);
            let k = renderable_horizon(&rows, budget);

            store.start_statement_work_ledger();
            let listed = bounded_m0(&store, budget, Some(&captured.legacy_history_segment_seqs));
            let work = history_segment_work(&store);
            assert_eq!(listed, bounded);
            assert!(
                work.iter().all(|w| !w.sql.starts_with(CAPTURE_SCAN)),
                "a persisted list skips the capture scan"
            );
            let rows_produced: u64 = work.iter().map(|w| w.rows).sum();
            assert!(
                rows_produced as usize <= PRESSURE_WINDOW + k + history.legacy_count() + 2,
                "budget {budget}: {rows_produced} rows for K = {k}"
            );

            // Falsifier (i): pressure from only the newest K rows.
            if k < PRESSURE_WINDOW {
                pressure_falsified |=
                    render_rows(&newest_rows(&rows, k, true), budget) != bounded.m0_bytes;
            }
            // Falsifier (ii): legacy rows skipped.
            legacy_falsified |=
                render_rows(&newest_rows(&rows, PRESSURE_WINDOW.max(k), false), budget)
                    != bounded.m0_bytes;
        }
        assert!(
            pressure_falsified,
            "pressure from the newest K rows alone went undetected"
        );
        assert!(legacy_falsified, "skipping legacy rows went undetected");
    }

    /// The m0 and m1 reads' statement work is equal at two history lengths that share their
    /// newest rows, so it does not grow with H.
    #[test]
    fn bounded_fold_work_is_independent_of_history_length() {
        let cap = crate::m1_compose::DEFAULT_M1_ROW_CAP;
        let work = |segments: usize| {
            let history = SyntheticHistory::mixed(segments);
            let dir = tempfile::tempdir().unwrap();
            let store = open(dir.path());
            history.seed(&store, SESSION);
            let measured = |read: &dyn Fn()| {
                store.start_statement_work_ledger();
                read();
                history_segment_work(&store)
                    .iter()
                    .map(|w| (w.rows, w.vm_steps))
                    .collect::<Vec<_>>()
            };
            let legacy = bounded_m0(&store, 60_000.0, None).legacy_history_segment_seqs;
            let m0 = [20.0, 60_000.0, 10_000_000.0].map(|budget| {
                measured(&|| {
                    bounded_m0(&store, budget, Some(&legacy));
                })
            });
            let m1 = [1, 200, cap + 1].map(|new_rows| {
                measured(&|| {
                    m1_above(&store, (segments - new_rows) as i64);
                })
            });
            (m0, m1)
        };
        assert_eq!(work(4_000), work(60_000));
    }

    #[test]
    fn bounded_m1_matches_the_full_read_and_withholds_an_overflowing_body() {
        let history = SyntheticHistory::mixed(60_000);
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path());
        history.seed(&store, SESSION);
        let cap = crate::m1_compose::DEFAULT_M1_ROW_CAP;
        let m1 = |folded: i64| {
            store.start_statement_work_ledger();
            let m1 = m1_above(&store, folded);
            let rows: u64 = history_segment_work(&store).iter().map(|w| w.rows).sum();
            assert!(rows as usize <= cap + 2, "{rows} rows for cap {cap}");
            m1
        };
        for new_rows in [0, 1, 200, cap] {
            let folded = (60_000 - new_rows) as i64;
            let all = store.load_history_segments(SESSION).unwrap();
            let above: Vec<DecayRenderHistorySegment> = all
                .iter()
                .filter(|row| row.sequence > folded)
                .map(DecayRenderHistorySegment::from)
                .collect();
            let refs: Vec<&DecayRenderHistorySegment> = above.iter().collect();
            let reference = assemble_m1(
                "",
                &render_new_history_segments(&refs),
                "",
                "",
                M1_PLACEHOLDER,
            );
            let last = all.last().unwrap();
            let bounded = m1(folded);
            assert_eq!(bounded.body, Some(reference), "{new_rows} new rows");
            assert_eq!(
                bounded.new_coverage,
                Some((last.end_message_id.clone(), last.end_message as u64))
            );
        }
        assert_eq!(m1((60_000 - cap - 1) as i64).body, None);
    }
}
