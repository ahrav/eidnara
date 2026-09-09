//! This module reads a session's durable state.
//! The durable state includes compartments, the user profile, and project docs; the caller supplies the pass's canonical memory rows.
//! This module composes frozen m0 bytes and watermarks that HARD persists.
//!
//! This module produces bytes but does not classify HARD versus SOFT.
//! `apply_once` feeds these bytes into the cache core.
//! This module returns identical bytes for identical store contents, `now_ms`, and `budget`.
//! The caller supplies frozen `now_ms`; this module never reads a live clock.

use std::collections::HashSet;

use memory_store::{MemoryStore, MemoryStoreError};
use sha2::{Digest, Sha256};

use crate::canonical_memory::CanonicalMemory;
use crate::compartment_coverage::{CoverageError, resolve_coverage};
use crate::decay_render::{DecayRenderCompartment, extract_m0_block};
use crate::memory_render::{
    M0Inputs, is_positive_memory_category, render_m0, render_memory_block, render_memory_line,
};
use crate::project_docs::read_project_docs_canonical;

pub(crate) const MEMORY_MURAL_BLOCK: &str =
    "<memory-mural>\nThe project memory mural image follows.\n</memory-mural>";

#[derive(thiserror::Error, Debug)]
pub enum M0ComposeError {
    #[error("store: {0}")]
    Store(MemoryStoreError),
    /// The stored compartment ranges overlap or otherwise fail strict ordering.
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
    /// `m0_bytes` contains docs, the profile, decayed compartments, and memories.
    pub m0_bytes: String,
    /// `mural` follows the m0 text block on the OpenCode wire.
    pub mural: Option<M0MuralBlock>,
    /// `boundary_id` anchors cache reverts at the last covered raw message.
    /// `boundary_id` is empty when no compartments are summarized, leaving the live array as the tail.
    pub boundary_id: String,
    /// `coverage_ordinal` marks m0's tail-trim point.
    /// `coverage_ordinal` is `None` when no compartments exist.
    pub coverage_ordinal: Option<u64>,
    /// `first_covered_ordinal` is the first covered ordinal; the caller rejects live items below it to prevent trimming an uncovered leading gap.
    pub first_covered_ordinal: Option<u64>,
    /// `folded_compartment_seq` advances only on a HARD.
    pub folded_compartment_seq: i64,
    /// `docs_hash` records the project-docs version included in m0; it does not trigger HARD.
    /// The next natural HARD re-reads current docs.
    pub docs_hash: String,
}

/// Capability-gated mural input supplied by the host.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct M0MuralInput {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub supports_vision: bool,
    #[serde(default)]
    pub data_url: Option<String>,
    #[serde(default, alias = "content_epoch")]
    pub content_hash: Option<String>,
}

/// Mural payload appended to the composed OpenCode context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct M0MuralBlock {
    pub data_url: String,
    pub content_hash: String,
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
    /// The renderer produces estimator-independent output when all compartments fit.
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
    /// The host resolves and capability-gates OpenCode-only image bytes before passing them here.
    pub mural: Option<&'a M0MuralInput>,
}

pub(crate) fn resolved_mural(input: Option<&M0MuralInput>) -> Option<M0MuralBlock> {
    let input = input?;
    if !input.enabled || !input.supports_vision {
        return None;
    }
    let data_url = input
        .data_url
        .as_deref()
        .filter(|value| !value.is_empty())?
        .to_string();
    let content_hash = input
        .content_hash
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("{:x}", Sha256::digest(data_url.as_bytes())));
    Some(M0MuralBlock {
        data_url,
        content_hash,
    })
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
    let mut selected = Vec::new();
    let mut categories = HashSet::<&str>::new();
    let mut used = estimate_tokens("<project-memory>\n</project-memory>") as f64;
    for memory in memories
        .iter()
        .filter(|memory| is_positive_memory_category(&memory.category))
    {
        let mut cost = estimate_tokens(&(render_memory_line(memory) + "\n"));
        if !categories.contains(memory.category.as_str()) {
            cost += estimate_tokens(&format!("<{}>\n</{}>\n", memory.category, memory.category));
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
                compartments: inputs.compartments,
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
    let compartments = store.load_compartments(inputs.session_id)?;
    let coverage = resolve_coverage(&compartments).map_err(M0ComposeError::CoverageGap)?;
    let (boundary_id, coverage_ordinal, first_covered_ordinal, folded_compartment_seq) =
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

    let decay_compartments: Vec<DecayRenderCompartment> = compartments
        .iter()
        .map(|compartment| {
            let mut rendered = DecayRenderCompartment::from(compartment);
            if !inputs.temporal_awareness {
                rendered.start_date = None;
                rendered.end_date = None;
            }
            rendered
        })
        .collect();
    let mural = resolved_mural(inputs.mural);
    let mut m0_bytes = render_m0_with_decay_pressure_retry(
        &M0Inputs {
            project_docs: &docs.rendered_block,
            user_profile: &user_profile,
            covered_system_messages: inputs.covered_system_messages,
            compartments: &decay_compartments,
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
    if mural.is_some() {
        m0_bytes.push_str("\n\n");
        m0_bytes.push_str(MEMORY_MURAL_BLOCK);
    }

    Ok(M0Composition {
        m0_bytes,
        mural,
        boundary_id,
        coverage_ordinal,
        first_covered_ordinal,
        folded_compartment_seq,
        docs_hash: docs.canonical_hash,
    })
}
