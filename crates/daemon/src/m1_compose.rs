use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Instant;

use memory_store::{MemoryStore, MemoryStoreError, ModuleMeta, NoteDelivery, StoredNote};

use crate::compartment_coverage::{CoverageError, partition_by_folded_seq, resolve_coverage};
use crate::decay_render::DecayRenderCompartment;
use crate::m0_compose::trim_user_profile_to_budget;
use crate::memory_render::{
    M1_PLACEHOLDER, assemble_m1, render_new_compartments, render_user_profile_block,
};

/// Failure to read composition state, or a compartment range that overlaps or fails to advance.
#[derive(thiserror::Error, Debug)]
pub enum M1ComposeError {
    #[error("store: {0}")]
    Store(MemoryStoreError),
    #[error("{0}")]
    CoverageGap(CoverageError),
}

impl From<MemoryStoreError> for M1ComposeError {
    fn from(error: MemoryStoreError) -> Self {
        Self::Store(error)
    }
}

/// `revision` is a digest over ALL byte-affecting m1 render inputs such that the
/// rendered bytes are a pure function of what the digest covers: if the bytes
/// would differ, `revision` differs. NEVER a max-id counter (a same-id update
/// changes bytes without raising a max id).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct M1RevisionSignal {
    pub revision: u64,
    pub external_revision: u64,
    pub max_compartment_seq: i64,
    pub note_status_version: i64,
    pub user_profile_version: u64,
}

/// Wall-clock read timings in milliseconds.
///
/// `notes_ms` remains available to callers but this module currently records
/// revision snapshot reads in `memories_ms` only.
#[derive(Debug, Clone, Copy, Default)]
pub struct M1RevisionReadTimings {
    pub memories_ms: f64,
    pub notes_ms: f64,
}

/// Computes nonzero in-session and external revision hashes.
///
/// The in-session hash covers the project-memory revision when memory is
/// enabled, maximum compartment sequence, note status version, and user profile
/// version. The external hash covers only the optional project-memory revision.
/// Store failures return [`MemoryStoreError`]. Elapsed read time, when
/// requested, is accumulated in milliseconds.
pub fn m1_revision_signal_timed(
    store: &MemoryStore,
    note_project_path: &str,
    session_id: &str,
    user_profile_version: u64,
    memory_enabled: bool,
    project_memory_revision: Option<u64>,
    timings: Option<&mut M1RevisionReadTimings>,
) -> Result<M1RevisionSignal, MemoryStoreError> {
    let snapshot_started_at = Instant::now();
    let snapshot = store.load_m1_revision_snapshot(note_project_path, session_id)?;
    if let Some(timings) = timings {
        timings.memories_ms += snapshot_started_at.elapsed().as_secs_f64() * 1_000.0;
    }
    let memory_revision = memory_enabled.then_some(project_memory_revision).flatten();
    let mut in_session = DefaultHasher::new();
    "eidnara-m1-claim-in-session-v1".hash(&mut in_session);
    memory_revision.hash(&mut in_session);
    snapshot.max_compartment_seq.hash(&mut in_session);
    snapshot.note_status_version.hash(&mut in_session);
    user_profile_version.hash(&mut in_session);
    let mut external = DefaultHasher::new();
    "eidnara-m1-claim-external-v1".hash(&mut external);
    memory_revision.hash(&mut external);
    Ok(M1RevisionSignal {
        revision: in_session.finish() | 1,
        external_revision: external.finish() | 1,
        max_compartment_seq: snapshot.max_compartment_seq,
        note_status_version: snapshot.note_status_version,
        user_profile_version,
    })
}

/// Rendered M1 body plus state that the caller must publish after delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct M1Composition {
    pub body: String,
    pub memory_update_count: usize,
    pub new_coverage: Option<(String, u64)>,
    pub note_deliveries: Vec<NoteDelivery>,
    pub profile_rendered: bool,
    pub notes_block: String,
}

/// Atomically claims eligible notes and renders them in delivery order.
///
/// Returns store errors from note claiming. An empty claim renders as an empty
/// string and produces no delivery records.
pub fn claim_and_render_notes(
    store: &MemoryStore,
    project_path: &str,
    session_id: &str,
    delivered_pass_fingerprint: &str,
    transform_pass_id: &str,
    now_ms: i64,
) -> Result<(String, Vec<NoteDelivery>), MemoryStoreError> {
    let deliveries = store.claim_note_delivery(
        project_path,
        session_id,
        delivered_pass_fingerprint,
        transform_pass_id,
        now_ms,
    )?;
    let notes = deliveries
        .iter()
        .map(|(note, _)| note.clone())
        .collect::<Vec<_>>();
    Ok((
        render_note_delta(&notes),
        deliveries
            .into_iter()
            .map(|(_, delivery)| delivery)
            .collect(),
    ))
}

fn render_note_delta(notes: &[StoredNote]) -> String {
    if notes.is_empty() {
        return String::new();
    }
    let mut lines = vec!["<new-notes>".to_string()];
    for note in notes {
        let condition = note
            .ready_reason
            .as_deref()
            .or(note.surface_condition.as_deref())
            .unwrap_or("Condition satisfied");
        lines.push(format!(
            "- #{}: {}\n  Condition: {}",
            note.id, note.content, condition
        ));
    }
    lines.push("</new-notes>".to_string());
    lines.join("\n")
}

/// Composes new compartments, a changed user profile, and newly claimed notes.
///
/// Compartment order follows the store result. A compartment whose `start_message` does not advance past the previous `end_message` returns [`M1ComposeError::CoverageGap`], while sparse ordinal gaps are permitted; store reads and note claims return [`M1ComposeError::Store`].
/// User profile budget units are tokens. Profile trimming receives 25 percent of that budget, clamped to at least one token.
#[allow(clippy::too_many_arguments)]
pub fn compose_m1(
    store: &MemoryStore,
    note_project_path: &str,
    session_id: &str,
    meta: &ModuleMeta,
    now_ms: i64,
    memory_enabled: bool,
    user_profile_budget_tokens: f64,
    temporal_awareness: bool,
    estimate_tokens: impl Fn(&str) -> usize + Copy,
) -> Result<M1Composition, M1ComposeError> {
    let compartments = store.load_compartments(session_id)?;
    let coverage = resolve_coverage(&compartments).map_err(M1ComposeError::CoverageGap)?;
    let (_, new_compartments) = partition_by_folded_seq(&compartments, meta.folded_compartment_seq);
    let rendered_compartments = new_compartments
        .iter()
        .map(|compartment| {
            let mut rendered = DecayRenderCompartment::from(*compartment);
            if !temporal_awareness {
                rendered.start_date = None;
                rendered.end_date = None;
            }
            rendered
        })
        .collect::<Vec<_>>();
    let compartment_refs = rendered_compartments.iter().collect::<Vec<_>>();
    let new_compartments_block = render_new_compartments(&compartment_refs);
    let new_coverage = match coverage {
        Some(coverage) if Some(coverage.coverage_end_ordinal) > meta.coverage_ordinal => {
            Some((coverage.boundary_id, coverage.coverage_end_ordinal))
        }
        _ => None,
    };

    let (new_user_profile_block, profile_rendered) =
        if memory_enabled && meta.user_profile_version != meta.m1_user_profile_version {
            let profile = trim_user_profile_to_budget(
                store.load_active_user_memories()?,
                (user_profile_budget_tokens.max(1.0) * 0.25)
                    .floor()
                    .max(1.0),
                estimate_tokens,
            );
            let block = render_user_profile_block(&profile, "new-user-profile");
            let rendered = !block.is_empty();
            (block, rendered)
        } else {
            (String::new(), false)
        };

    let (notes_block, note_deliveries) = claim_and_render_notes(
        store,
        note_project_path,
        session_id,
        &format!("m1:{}:{now_ms}", meta.m1_revision),
        &format!("m1:{}:{now_ms}", meta.m1_revision),
        now_ms,
    )?;
    let profile_and_notes = [new_user_profile_block.as_str(), notes_block.as_str()]
        .into_iter()
        .filter(|block| !block.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    Ok(M1Composition {
        body: assemble_m1(
            "",
            &new_compartments_block,
            "",
            &profile_and_notes,
            M1_PLACEHOLDER,
        ),
        memory_update_count: 0,
        new_coverage,
        note_deliveries,
        profile_rendered,
        notes_block,
    })
}
