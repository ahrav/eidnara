use std::ops::Add;

use memory_store::{
    HistoryArchiveRequest, HistorySummarizerDurableState, HistorySummarizerPhase,
    HistorySummarizerPublishError, MemoryStore,
};

use crate::boundary::completed_tool_arc_crosses_boundary;
use crate::history_summarizer::{
    HistorySummarizerStateError, completion_wait_budget, settle_reservation,
};
use crate::history_summarizer_chunk::tool_arcs;
use crate::wire::{FlatBlock, FlatProjection};

pub const WINDOW_CAP_BLOCKS: usize = 800;
pub const WINDOW_CAP_BYTES: usize = 32 * 1024 * 1024;
pub const HALF_CAP_BLOCKS: usize = WINDOW_CAP_BLOCKS / 2;
pub const HALF_CAP_BYTES: usize = WINDOW_CAP_BYTES / 2;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WindowSize {
    pub blocks: usize,
    pub bytes: usize,
}

impl Add for WindowSize {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            blocks: self.blocks + other.blocks,
            bytes: self.bytes + other.bytes,
        }
    }
}

impl WindowSize {
    pub fn of(projection: &FlatProjection) -> Self {
        projection
            .blocks
            .iter()
            .filter(|block| !block.synthetic())
            .fold(Self::default(), |size, block| size + Self::block(block))
    }

    fn block(block: &FlatBlock) -> Self {
        Self {
            blocks: 1,
            bytes: block.bytes().len(),
        }
    }

    pub fn at_cap(self) -> bool {
        self.blocks >= WINDOW_CAP_BLOCKS || self.bytes >= WINDOW_CAP_BYTES
    }

    fn within_half_cap(self) -> bool {
        self.blocks <= HALF_CAP_BLOCKS && self.bytes <= HALF_CAP_BYTES
    }
}

struct WindowMessage<'a> {
    ordinal: u64,
    size: WindowSize,
    mid: &'a str,
    last_block_id: &'a str,
}

fn window_messages(projection: &FlatProjection) -> Vec<WindowMessage<'_>> {
    let mut messages: Vec<WindowMessage<'_>> = Vec::new();
    for block in projection.blocks.iter().filter(|block| !block.synthetic()) {
        match messages.last_mut() {
            Some(message) if message.ordinal == block.ordinal() => {
                message.size = message.size + WindowSize::block(block);
                message.last_block_id = block.id();
            }
            _ => messages.push(WindowMessage {
                ordinal: block.ordinal(),
                size: WindowSize::block(block),
                mid: &block.mid,
                last_block_id: block.id(),
            }),
        }
    }
    messages
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveCut {
    pub start: u64,
    pub end: u64,
    pub start_message_id: String,
    pub end_message_id: String,
}

/// The archive over the messages after `covered_end`. It ends on the message before the newest
/// suffix that fits within half the cap, since the HARD that folds it serves only the messages
/// after its end. An unpersisted message or a tool arc it would split moves the end later, or
/// earlier when no later edge exists, so a live tool call keeps its arc served whole. A tool
/// call at or after the half-cap edge with no result yet is live; an older one is abandoned.
pub fn archive_cut(
    projection: &FlatProjection,
    covered_end: Option<u64>,
    persisted: impl Fn(&str) -> bool,
) -> Option<ArchiveCut> {
    let messages = window_messages(projection);
    let newest = messages.len().checked_sub(1)?;
    let first = messages
        .iter()
        .position(|message| covered_end.is_none_or(|end| message.ordinal > end))?;
    let mut kept = messages[newest].size;
    let mut kept_from = newest;
    while kept_from > 0 && (kept + messages[kept_from - 1].size).within_half_cap() {
        kept_from -= 1;
        kept = kept + messages[kept_from].size;
    }
    if kept_from <= first {
        return None;
    }
    let blocks: Vec<FlatBlock> = projection
        .blocks
        .iter()
        .filter(|block| !block.synthetic())
        .cloned()
        .collect();
    let arcs = tool_arcs(&blocks);
    let live_open_from = messages[kept_from - 1].ordinal;
    let valid = |index: usize| {
        let ordinal = messages[index].ordinal;
        persisted(messages[index].mid)
            && !arcs
                .completed
                .iter()
                .any(|arc| completed_tool_arc_crosses_boundary(arc.start, arc.end, ordinal + 1))
            && !arcs
                .open_invocations
                .iter()
                .any(|invocation| *invocation >= live_open_from && *invocation <= ordinal)
    };
    let cut = (kept_from - 1..newest)
        .find(|index| valid(*index))
        .or_else(|| (first..kept_from - 1).rev().find(|index| valid(*index)))?;
    Some(ArchiveCut {
        start: messages[first].ordinal,
        end: messages[cut].ordinal,
        start_message_id: messages[first].last_block_id.to_string(),
        end_message_id: messages[cut].last_block_id.to_string(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveCause {
    InFlightFiring { firing_seq: u64 },
    IdleBackoff,
}

pub fn archive_cause(
    summarizer: &HistorySummarizerDurableState,
    now_ms: i64,
) -> Option<ArchiveCause> {
    let in_backoff = summarizer
        .failure_backoff_at_ms
        .is_some_and(|backoff_at_ms| now_ms < backoff_at_ms);
    if summarizer.state == HistorySummarizerPhase::Idle {
        return in_backoff.then_some(ArchiveCause::IdleBackoff);
    }
    let budget_ms = i64::try_from(completion_wait_budget().as_millis()).unwrap_or(i64::MAX);
    let past_deadline = summarizer
        .fired_at_ms
        .is_some_and(|fired_at_ms| now_ms >= fired_at_ms.saturating_add(budget_ms));
    (past_deadline || in_backoff).then_some(ArchiveCause::InFlightFiring {
        firing_seq: summarizer.firing_seq,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Archived {
    pub sequence: i64,
    pub cut: ArchiveCut,
    pub cause: ArchiveCause,
}

pub fn archive_window(
    store: &MemoryStore,
    session_id: &str,
    project_path: &str,
    projection: &FlatProjection,
    now_ms: i64,
) -> Result<Option<Archived>, HistorySummarizerStateError> {
    let loaded = store.load(session_id)?;
    let Some(cause) = archive_cause(&loaded.meta.history_summarizer, now_ms) else {
        return Ok(None);
    };
    let snapshot = store.load_history_summarizer_assembly_snapshot(session_id, 1)?;
    let covered_end = snapshot
        .max_end_message
        .and_then(|end| u64::try_from(end).ok())
        .filter(|end| *end > 0);
    let durable = &loaded.meta.block_identity_by_mid;
    let Some(cut) = archive_cut(projection, covered_end, |mid| durable.contains_key(mid)) else {
        return Ok(None);
    };
    let published = store.publish_history_archive(HistoryArchiveRequest {
        session_id,
        expected_row_version: loaded.row_version,
        expected_revert_epoch: snapshot.revert_epoch,
        history_segment_set_generation: snapshot.history_segment_set_generation,
        abandons_firing_seq: match cause {
            ArchiveCause::InFlightFiring { firing_seq } => Some(firing_seq),
            ArchiveCause::IdleBackoff => None,
        },
        start_message: cut.start,
        end_message: cut.end,
        start_message_id: &cut.start_message_id,
        end_message_id: &cut.end_message_id,
        now_ms,
    });
    let published = match published {
        Ok(published) => published,
        Err(
            HistorySummarizerPublishError::CasConflict { .. }
            | HistorySummarizerPublishError::FenceRejected { .. }
            | HistorySummarizerPublishError::InvalidState { .. }
            | HistorySummarizerPublishError::HistorySegmentOverlap { .. },
        ) => return Ok(None),
        Err(error) => return Err(HistorySummarizerStateError::Publish(error)),
    };
    if let Some(reservation) = &published.abandoned_reservation
        && let Err(error) = settle_reservation(store, session_id, project_path, reservation, now_ms)
    {
        eprintln!(
            "daemon: history_summarizer archive left reservation {} unsettled session={session_id}: {error}",
            reservation.causal_identity
        );
    }
    Ok(Some(Archived {
        sequence: published.sequence,
        cut,
        cause,
    }))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use memory_store::memory_reviewer_jobs::{
        CausalInputs, MemoryReviewerJobOutcome, MemoryReviewerJobState, ProducerBinding,
        ReserveOutcome, ReviewTarget,
    };
    use memory_store::{
        HistorySummarizerAbandonReason, HistorySummarizerChunkRange, HistorySummarizerDurableState,
        HistorySummarizerPhase, MemoryReviewerReservation, ModuleMeta,
    };

    use super::*;
    use crate::history_summarizer::persist_history_summarizer_state;
    use crate::transform::tests::{assistant_tool_call, item, tool_result, wire_item};
    use crate::wire::{IngressMessage, project_messages};

    const NOW: i64 = 10_000_000;

    fn text(ordinal: u64) -> IngressMessage {
        item(
            &format!("m{ordinal}"),
            ordinal,
            &format!("message {ordinal}"),
        )
    }

    fn projection(messages: Vec<IngressMessage>) -> FlatProjection {
        let messages: Vec<Arc<IngressMessage>> = messages.into_iter().map(Arc::new).collect();
        project_messages(&messages).unwrap()
    }

    fn window_with(last: u64, special: impl Fn(u64) -> Option<IngressMessage>) -> FlatProjection {
        projection(
            (1..=last)
                .map(|ordinal| special(ordinal).unwrap_or_else(|| text(ordinal)))
                .collect(),
        )
    }

    fn cut_of(window: &FlatProjection, covered_end: Option<u64>) -> Option<(u64, u64)> {
        archive_cut(window, covered_end, |mid| {
            window.identity_by_mid.contains_key(mid)
        })
        .map(|cut| (cut.start, cut.end))
    }

    /// The messages the next HARD serves: those after the archive's end message, which the
    /// boundary render replaces.
    fn served_after(window: &FlatProjection, end: u64) -> Vec<&str> {
        let mut mids: Vec<&str> = window
            .blocks
            .iter()
            .filter(|block| !block.synthetic() && block.ordinal() > end)
            .map(|block| block.mid.as_str())
            .collect();
        mids.dedup();
        mids
    }

    /// The window the next HARD serves: every message after the archive's end.
    fn window_after(window: &FlatProjection, end: u64) -> WindowSize {
        window
            .blocks
            .iter()
            .filter(|block| !block.synthetic() && block.ordinal() > end)
            .fold(WindowSize::default(), |size, block| {
                size + WindowSize::block(block)
            })
    }

    const LAST: u64 = WINDOW_CAP_BLOCKS as u64 + 100;
    const LAST_ARCHIVED: u64 = LAST - HALF_CAP_BLOCKS as u64;

    #[test]
    fn the_cut_leaves_half_the_cap_after_its_end() {
        let window = window_with(LAST, |_| None);
        let cut = archive_cut(&window, None, |_| true).unwrap();
        assert_eq!((cut.start, cut.end), (1, LAST_ARCHIVED));
        assert_eq!(cut.start_message_id, "m1#0");
        assert_eq!(cut.end_message_id, format!("m{LAST_ARCHIVED}#0"));
        assert_eq!(window_after(&window, cut.end).blocks, HALF_CAP_BLOCKS);
        assert_eq!(cut_of(&window, Some(100)), Some((101, LAST_ARCHIVED)));
        assert_eq!(
            cut_of(&window, Some(LAST_ARCHIVED - 1)),
            Some((LAST_ARCHIVED, LAST_ARCHIVED))
        );
        assert_eq!(cut_of(&window, Some(LAST_ARCHIVED)), None);
    }

    #[test]
    fn the_cut_ends_outside_every_tool_arc_and_on_a_persisted_message() {
        let call = LAST_ARCHIVED;
        let arc = window_with(LAST, |ordinal| match ordinal {
            o if o == call => Some(assistant_tool_call(&format!("m{o}"), o, "call")),
            o if o == call + 1 => Some(tool_result(&format!("m{o}"), o, "call", "ok")),
            _ => None,
        });
        assert_eq!(cut_of(&arc, None), Some((1, call + 1)));
        assert!(window_after(&arc, call + 1).within_half_cap());

        let live_open = window_with(LAST, |ordinal| {
            (ordinal == LAST_ARCHIVED)
                .then(|| assistant_tool_call(&format!("m{ordinal}"), ordinal, "open"))
        });
        assert_eq!(cut_of(&live_open, None), Some((1, LAST_ARCHIVED - 1)));
        assert_eq!(
            window_after(&live_open, LAST_ARCHIVED - 1).blocks,
            HALF_CAP_BLOCKS + 1
        );

        let stale_open = window_with(LAST, |ordinal| {
            (ordinal == 300).then(|| assistant_tool_call("m300", 300, "abandoned"))
        });
        assert_eq!(cut_of(&stale_open, None), Some((1, LAST_ARCHIVED)));

        let mut unpersisted = window_with(LAST, |_| None);
        unpersisted
            .identity_by_mid
            .remove(&format!("m{LAST_ARCHIVED}"));
        assert_eq!(cut_of(&unpersisted, None), Some((1, LAST_ARCHIVED + 1)));
    }

    #[test]
    fn a_newest_message_larger_than_half_the_cap_is_served_alone() {
        let texts: Vec<String> = (0..=HALF_CAP_BLOCKS)
            .map(|block| format!("part {block}"))
            .collect();
        let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
        let mut messages: Vec<IngressMessage> = (1..=9).map(text).collect();
        messages.push(wire_item("user", "m10", 10, &texts));
        let window = projection(messages);
        assert_eq!(cut_of(&window, None), Some((1, 9)));
        assert_eq!(served_after(&window, 9), ["m10"]);

        let pair_then_oversize = projection(vec![
            assistant_tool_call("m1", 1, "call"),
            tool_result("m2", 2, "call", "ok"),
            wire_item("user", "m3", 3, &texts),
        ]);
        assert_eq!(cut_of(&pair_then_oversize, None), Some((1, 2)));
        assert_eq!(served_after(&pair_then_oversize, 2), ["m3"]);
    }

    #[test]
    fn the_cap_and_the_cut_count_block_bytes() {
        let at = |blocks, bytes| WindowSize { blocks, bytes }.at_cap();
        assert!(!at(WINDOW_CAP_BLOCKS - 1, WINDOW_CAP_BYTES - 1));
        assert!(at(WINDOW_CAP_BLOCKS, 0));
        assert!(at(0, WINDOW_CAP_BYTES));

        let large = 6 * 1024 * 1024;
        let window = projection(
            (1..=6)
                .map(|ordinal| item(&format!("m{ordinal}"), ordinal, &"x".repeat(large)))
                .collect(),
        );
        let size = WindowSize::of(&window);
        assert_eq!(size.blocks, 6);
        assert!(size.bytes >= 6 * large && size.at_cap(), "{size:?}");
        let (start, end) = cut_of(&window, None).unwrap();
        assert_eq!((start, end), (1, 4));
        assert!(window_after(&window, end).within_half_cap());
        assert!(!window_after(&window, end - 1).within_half_cap());
    }

    #[test]
    fn the_summarizer_cannot_publish_past_its_deadline_or_in_backoff() {
        let budget = i64::try_from(completion_wait_budget().as_millis()).unwrap();
        let in_flight = HistorySummarizerDurableState {
            state: HistorySummarizerPhase::AwaitingProducer,
            firing_seq: 7,
            fired_at_ms: Some(1_000),
            ..HistorySummarizerDurableState::default()
        };
        let firing = Some(ArchiveCause::InFlightFiring { firing_seq: 7 });
        assert_eq!(archive_cause(&in_flight, 1_000 + budget - 1), None);
        assert_eq!(archive_cause(&in_flight, 1_000 + budget), firing);
        let retained_in_backoff = HistorySummarizerDurableState {
            state: HistorySummarizerPhase::Publishing,
            failure_backoff_at_ms: Some(5_000),
            ..in_flight.clone()
        };
        assert_eq!(archive_cause(&retained_in_backoff, 4_999), firing);
        let idle = HistorySummarizerDurableState {
            failure_backoff_at_ms: Some(5_000),
            ..HistorySummarizerDurableState::default()
        };
        assert_eq!(archive_cause(&idle, 4_999), Some(ArchiveCause::IdleBackoff));
        assert_eq!(archive_cause(&idle, 5_000), None);
        assert_eq!(
            archive_cause(&HistorySummarizerDurableState::default(), 0),
            None
        );
    }

    fn store_with(
        window: &FlatProjection,
        summarizer: HistorySummarizerDurableState,
    ) -> (tempfile::TempDir, MemoryStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(&MemoryStore::test_descriptor(
            dir.path(),
            "eidnara-archive-test",
        ))
        .unwrap();
        let meta = ModuleMeta {
            block_identity_by_mid: window.identity_by_mid.clone(),
            history_summarizer: summarizer,
            ..ModuleMeta::default()
        };
        store
            .commit("ses", None, &cache_stability::CoreState::empty(), &meta)
            .unwrap();
        (dir, store)
    }

    #[test]
    fn an_idle_summarizer_in_backoff_archives_the_window() {
        let window = window_with(LAST, |_| None);
        let summarizer = HistorySummarizerDurableState {
            failure_backoff_at_ms: Some(NOW + 1),
            ..HistorySummarizerDurableState::default()
        };
        let (_dir, store) = store_with(&window, summarizer.clone());
        let archived = archive_window(&store, "ses", "git:proj", &window, NOW)
            .unwrap()
            .expect("an idle summarizer in backoff cannot publish");
        assert_eq!(archived.cause, ArchiveCause::IdleBackoff);
        assert_eq!((archived.cut.start, archived.cut.end), (1, FIRST_KEPT));
        let loaded = store.load("ses").unwrap();
        assert_eq!(
            loaded.meta.history_summarizer.state,
            HistorySummarizerPhase::Idle
        );
        assert_eq!(
            loaded.meta.history_summarizer.failure_backoff_at_ms,
            Some(NOW + 1)
        );
        assert_eq!(loaded.meta.history_summarizer.last_abandon, None);
        assert_eq!(loaded.meta.archive_fold_seq, Some(archived.sequence));

        let (_dir, expired) = store_with(&window, summarizer);
        assert_eq!(
            archive_window(&expired, "ses", "git:proj", &window, NOW + 1).unwrap(),
            None
        );
        assert!(expired.load_history_segments("ses").unwrap().is_empty());
    }

    /// A firing past its deadline that holds a reserved review job is ended by the archive,
    /// its job is closed through the settle path, and its later transitions are refused.
    #[test]
    fn an_archive_settles_the_reservation_of_the_firing_it_ends() {
        let window = window_with(LAST, |_| None);
        let (_dir, store) = store_with(&window, HistorySummarizerDurableState::default());
        let producer = ProducerBinding {
            producer: "history_summarizer".to_string(),
            firing_id: "archived#5".to_string(),
            ordinal: 1,
        };
        let inputs = CausalInputs {
            target: ReviewTarget::Memory {
                object_id: "memory-1".to_string(),
                source_revision: 1,
            },
            question_template: "extracted_facts".to_string(),
            signals: Vec::new(),
            required_evidence: Vec::new(),
            policy_versions: BTreeMap::new(),
        };
        let ReserveOutcome::Reserved(job) = store
            .reserve_memory_reviewer_job("git:proj", &producer, &inputs, NOW)
            .unwrap()
        else {
            panic!("a fresh job is reserved");
        };
        let budget = i64::try_from(completion_wait_budget().as_millis()).unwrap();
        let stalled = HistorySummarizerDurableState {
            state: HistorySummarizerPhase::AwaitingProducer,
            firing_seq: 5,
            chunk_range: Some(HistorySummarizerChunkRange {
                from_ordinal: 1,
                to_ordinal: 40,
            }),
            producer_run_id: Some("run-5".to_string()),
            fired_at_ms: Some(NOW - budget),
            memory_reviewer_reservation: Some(MemoryReviewerReservation {
                firing_seq: 5,
                causal_identity: job.causal_identity.clone(),
                candidate_id: "candidate".to_string(),
                payload_digest: "digest".to_string(),
                kernel_incarnation: "kernel".to_string(),
                queue_deadline_ms: job.queue_deadline_ms,
            }),
            ..HistorySummarizerDurableState::default()
        };
        let loaded = store.load("ses").unwrap();
        let mut meta = loaded.meta.clone();
        meta.history_summarizer = stalled.clone();
        store
            .commit("ses", loaded.row_version, &loaded.core, &meta)
            .unwrap();

        let archived = archive_window(&store, "ses", "git:proj", &window, NOW)
            .unwrap()
            .expect("a firing past its deadline cannot publish");
        assert_eq!(
            archived.cause,
            ArchiveCause::InFlightFiring { firing_seq: 5 }
        );
        let after = store.load("ses").unwrap();
        let summarizer = &after.meta.history_summarizer;
        assert_eq!(summarizer.state, HistorySummarizerPhase::Idle);
        assert_eq!(summarizer.memory_reviewer_reservation, None);
        assert_eq!(
            summarizer
                .last_abandon
                .as_ref()
                .map(|abandon| (abandon.firing_seq, abandon.reason)),
            Some((5, HistorySummarizerAbandonReason::WindowArchived))
        );
        let settled = store
            .lookup_memory_reviewer_job("git:proj", &job.causal_identity)
            .unwrap()
            .unwrap();
        assert_eq!(
            settled.state,
            MemoryReviewerJobState::Terminal(MemoryReviewerJobOutcome::Nonadmitted)
        );
        assert_eq!(store.load_pending_publication("ses").unwrap(), None);

        let resumed = HistorySummarizerDurableState {
            state: HistorySummarizerPhase::Validating,
            ..stalled
        };
        assert!(matches!(
            persist_history_summarizer_state(&store, "ses", resumed),
            Err(HistorySummarizerStateError::InvalidTransition { .. })
        ));
        assert_eq!(store.load("ses").unwrap().row_version, after.row_version);
    }
}
