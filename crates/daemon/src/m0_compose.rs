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
use crate::decay_render::{DecayRenderHistorySegment, extract_m0_block};
use crate::history_segment_coverage::{CoverageError, resolve_coverage};
use crate::memory_render::{
    M0Inputs, is_positive_memory_category, render_m0, render_memory_block, render_memory_line,
};
use crate::project_docs::read_project_docs_canonical;

#[derive(thiserror::Error, Debug)]
pub enum M0ComposeError {
    #[error("store: {0}")]
    Store(MemoryStoreError),
    /// The stored history_segment ranges overlap or otherwise fail strict ordering.
    #[error("{0}")]
    CoverageGap(CoverageError),
}
impl From<MemoryStoreError> for M0ComposeError {
    fn from(e: MemoryStoreError) -> Self {
        M0ComposeError::Store(e)
    }
}

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
) -> Result<M0Composition, M0ComposeError> {
    let history_segments = store.load_history_segments(inputs.session_id)?;
    let coverage = resolve_coverage(&history_segments).map_err(M0ComposeError::CoverageGap)?;
    let (boundary_id, coverage_ordinal, first_covered_ordinal, folded_history_segment_seq) =
        match &coverage {
            Some(c) => (
                c.boundary_id.clone(),
                Some(c.coverage_end_ordinal),
                Some(c.first_covered_ordinal),
                c.max_sequence,
            ),
            None => (String::new(), None, None, 0),
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
