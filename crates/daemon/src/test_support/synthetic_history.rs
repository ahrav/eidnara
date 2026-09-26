//! Synthetic session histories seeded straight into a real SQLite `MemoryStore`.
//!
//! A row's content, importance, and legacy flag are functions of its distance from the
//! newest row, so two sessions of different lengths share an identical newest region and a
//! fold read over either visits the same rows. Ranges and sequences still grow with the
//! absolute position, as the store requires.

use memory_store::{HistorySummarizerPhase, MemoryStore, StoredHistorySegment};

use super::transform_corpus::Rng;

/// Shape of one synthetic session.
#[derive(Debug, Clone)]
pub struct SyntheticHistory {
    /// Stored history_segment count, `H`.
    pub segments: usize,
    /// Distances from the newest row (1 = newest) that hold legacy rows.
    pub legacy_from_newest: Vec<usize>,
    pub seed: u64,
}

impl SyntheticHistory {
    /// `segments` rows with legacy rows inside the newest pressure window, just past it,
    /// and older than the largest renderable index (2,484), where the session is long enough.
    pub fn mixed(segments: usize) -> Self {
        Self {
            segments,
            legacy_from_newest: [3, 120, 400, 2_600, 2_900]
                .into_iter()
                .filter(|distance| *distance <= segments)
                .collect(),
            seed: 0x5EED_0826,
        }
    }

    /// Legacy row count, `L`.
    pub fn legacy_count(&self) -> usize {
        self.legacy_from_newest.len()
    }

    /// The row at 1-based `sequence`, oldest first.
    pub fn segment(&self, sequence: usize) -> StoredHistorySegment {
        assert!((1..=self.segments).contains(&sequence));
        let distance = self.segments + 1 - sequence;
        let mut rng = Rng::new(self.seed ^ (distance as u64).wrapping_mul(0x9E37_79B9));
        let end = 2 * sequence as i64;
        let words = |rng: &mut Rng, n: u64| {
            let count = 3 + rng.next() % n;
            (0..count)
                .map(|_| *rng.pick(&["fold", "cache", "segment", "decay", "tier", "budget"]))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let title = format!("segment {distance} {}", words(&mut rng, 4));
        let base = StoredHistorySegment {
            sequence: sequence as i64,
            start_message: end - 1,
            end_message: end,
            start_message_id: format!("m{}#0", end - 1),
            end_message_id: format!("m{end}#0"),
            title,
            created_at: sequence as i64,
            ..Default::default()
        };
        if self.legacy_from_newest.contains(&distance) {
            let body = words(&mut rng, 40);
            return StoredHistorySegment {
                content: if distance.is_multiple_of(2) {
                    format!("U: {body}\nA: {body}")
                } else {
                    body
                },
                legacy: 1,
                ..base
            };
        }
        // The extremes sit inside the newest pressure window so pressure depends on them.
        let importance = match distance {
            5 | 180 => 100,
            7 | 240 => 1,
            _ => *rng.pick(&[1, 10, 30, 50, 50, 50, 75, 90, 100]),
        };
        let p1 = words(&mut rng, 60);
        StoredHistorySegment {
            content: p1.clone(),
            p1: Some(p1),
            p2: Some(words(&mut rng, 25)),
            p3: Some(words(&mut rng, 8)),
            p4: (!distance.is_multiple_of(5)).then(|| words(&mut rng, 3)),
            importance,
            ..base
        }
    }

    /// Every row, oldest first.
    pub fn rows(&self) -> Vec<StoredHistorySegment> {
        (1..=self.segments)
            .map(|sequence| self.segment(sequence))
            .collect()
    }

    /// Inserts every row into `session_id` in one fenced transaction.
    pub fn seed(&self, store: &MemoryStore, session_id: &str) {
        store
            .with_fenced_conn_for_test(|tx| {
                let mut insert = tx.prepare_cached(
                    "INSERT INTO history_segments
                       (session_id, sequence, start_message, end_message, start_message_id,
                        end_message_id, start_date, end_date, title, content, p1, p2, p3, p4,
                        importance, episode_type, legacy, created_at)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)",
                )?;
                for sequence in 1..=self.segments {
                    let c = self.segment(sequence);
                    insert.execute(rusqlite::params![
                        session_id,
                        c.sequence,
                        c.start_message,
                        c.end_message,
                        c.start_message_id,
                        c.end_message_id,
                        c.start_date,
                        c.end_date,
                        c.title,
                        c.content,
                        c.p1,
                        c.p2,
                        c.p3,
                        c.p4,
                        c.importance,
                        c.episode_type,
                        c.legacy,
                        c.created_at,
                    ])?;
                }
                Ok(())
            })
            .expect("seed synthetic history");
    }
}

/// Writes `count` rows into each of `temporal_marks`, `user_hints`, and `channel1_appends`
/// under block ids `overlay-{n}#0`, none of which a synthetic window carries.
pub fn seed_overlays(store: &MemoryStore, session_id: &str, count: usize) {
    let ids: Vec<String> = (0..count).map(|n| format!("overlay-{n}#0")).collect();
    seed_block_overlays(store, session_id, &ids);
}

/// Writes one row per block id into each of `temporal_marks`, `user_hints`, and
/// `channel1_appends`, replacing any row the block already has.
pub fn seed_block_overlays(store: &MemoryStore, session_id: &str, block_ids: &[String]) {
    store
        .with_fenced_conn_for_test(|tx| {
            for (table, text) in [
                ("temporal_marks", "marker_text, created_at"),
                ("user_hints", "hint_text, created_at"),
                ("channel1_appends", "reminder_text, fired_at_ms"),
            ] {
                let mut insert = tx.prepare(&format!(
                    "INSERT OR REPLACE INTO {table} (session_id, block_id, {text})
                     VALUES (?1, ?2, ?3, ?4)"
                ))?;
                for (n, block_id) in block_ids.iter().enumerate() {
                    insert.execute(rusqlite::params![
                        session_id,
                        block_id,
                        format!("{table} {n}"),
                        n as i64,
                    ])?;
                }
            }
            Ok(())
        })
        .expect("seed overlays");
}

/// Commits session metadata with a history_summarizer firing awaiting its producer.
pub fn seed_active_summarizer(store: &MemoryStore, session_id: &str) {
    let loaded = store.load(session_id).expect("load");
    let mut meta = loaded.meta.clone();
    meta.initialized = true;
    meta.history_summarizer.state = HistorySummarizerPhase::AwaitingProducer;
    meta.history_summarizer.firing_seq = 1;
    meta.history_summarizer.producer_run_id = Some("synthetic-run".to_string());
    meta.history_summarizer
        .history_segment_set_generation
        .max_sequence = store
        .max_history_segment_seq(session_id)
        .expect("max sequence");
    store
        .commit(session_id, loaded.row_version, &loaded.core, &meta)
        .expect("commit active summarizer");
}
