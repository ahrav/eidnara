//! The bounded per-session record of History Summarizer firings and the session's outcome counters.
//!
//! Each model attempt that advances `firing_seq` leaves one [`RecentFiring`] carrying what the trigger saw and one stamp per transition that actually happened. Every stamp on one entry is read from the clock its [`TimelineClock`] names; a reader treats a missing, inverted, or differently clocked stamp as unknown. Durations, ratios, and counts by reason are derived at read and never stored.

use serde::{Deserialize, Serialize};

use crate::HistorySummarizerDurableState;

/// How many firings the timeline keeps; the ninth evicts the oldest.
pub const RECENT_FIRINGS_CAPACITY: usize = 8;

/// The largest no-fire detail an entry keeps, in UTF-8 bytes, cut at a character boundary.
pub const NO_FIRE_DETAIL_MAX_BYTES: usize = 128;

/// Which daemon entry path started a firing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FiringSource {
    /// A transform pass under context pressure, in the background or inline at the emergency wall.
    #[default]
    PressurePath,
    Wrapup,
    /// A producer run resumed after restart whose firing was recorded before this timeline existed.
    Reattach,
}

/// The boundary trigger tier that fired; wrapup firings have none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FiringTriggerReason {
    ProjectedHeadroom,
    ForceBand,
    CommitClusters,
    TailSize,
}

/// The clock that supplied every stamp on one entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimelineClock {
    /// The daemon's wall clock in Unix milliseconds, sampled at each transition.
    DaemonWallMs,
}

/// Context usage the trigger evaluated when the firing started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FiringUsage {
    pub input_tokens: u64,
    /// The denominator of `usage_percentage`.
    pub context_limit_tokens: u64,
    /// `input_tokens * 100 / context_limit_tokens`, rounded down; above 100 when input exceeds the limit.
    pub usage_percentage: u32,
    /// The execute threshold in force, rounded to a whole percentage.
    pub execute_threshold_percentage: u32,
}

/// What the entry path knew about the firing before it started.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FiringTrigger {
    pub source: FiringSource,
    pub reason: Option<FiringTriggerReason>,
    pub usage: Option<FiringUsage>,
}

/// The closed class of the last reason a pass declined to fire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoFireReason {
    /// A run was live or the durable state was not idle.
    Busy,
    PendingRewrite,
    TriggerFalse,
    NoModels,
    MissingBoundary,
    Backoff,
    AssembleNoFire,
    AssembleFailed,
    ContinuedOrdinalOffsetMissing,
    Other,
}

/// A no-fire reason with its recorded text capped at [`NO_FIRE_DETAIL_MAX_BYTES`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoFire {
    pub reason: NoFireReason,
    pub detail: String,
}

impl NoFire {
    /// Keeps at most [`NO_FIRE_DETAIL_MAX_BYTES`] of `detail`.
    pub fn new(reason: NoFireReason, detail: &str) -> Self {
        let cut = detail.floor_char_boundary(NO_FIRE_DETAIL_MAX_BYTES);
        NoFire {
            reason,
            detail: detail[..cut].to_string(),
        }
    }
}

/// Why a firing ended without publishing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbandonClass {
    ProducerFailed,
    /// The producer answered and validation refused the output.
    ValidationRejected,
    /// A store fence refused the snapshot: the selected input, the segment set, the revert epoch, or the row moved.
    Invalidated,
    /// The recomputed chunk differs from the fired one; a fresh firing observes the chunk it fired, so only a resumed one reaches this.
    FingerprintMismatch,
    /// A retired transform snapshot refused; a pressure-path firing publishes without that fence, so only a resumed or wrapup firing reaches this.
    CallerFenceRejected,
    /// The producer run was gone when a resumed firing asked for it.
    ProducerMissing,
    /// Restart recovery released a firing that had not published.
    Restarted,
    ConnectFailed,
    /// A reserved publication could never commit and its reservation was settled.
    ReservationSettled,
    /// The MemoryReviewer handoff failed before any reservation existed.
    HandoffFailed,
}

/// Where a firing's attempt ended. Only `ReattachConnectFailed` can be replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FiringOutcome {
    /// `sequence` is the highest history_segment sequence the publication appended; `None` when it appended none.
    Published {
        sequence: Option<i64>,
    },
    Abandoned {
        class: AbandonClass,
    },
    /// A resumed firing could not reach the producer; the firing is still awaiting it.
    ReattachConnectFailed,
}

/// One firing's durable record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentFiring {
    pub firing_seq: u64,
    pub source: FiringSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_reason: Option<FiringTriggerReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<FiringUsage>,
    pub clock: TimelineClock,
    /// The first pass that reached the proactive percentage while no run could start, or the fire instant when none did.
    pub eligible_at_ms: Option<i64>,
    /// The last recorded reason a pass declined to fire before this one did.
    pub last_no_fire: Option<NoFire>,
    /// The clock read when the attempt entered `Firing`, not the pass start the durable state's `fired_at_ms` keeps.
    pub fired_at_ms: Option<i64>,
    pub producer_started_at_ms: Option<i64>,
    pub output_received_at_ms: Option<i64>,
    pub published_at_ms: Option<i64>,
    /// The pass clock of the first commit whose served prefix rendered `Published { sequence }`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activated_at_ms: Option<i64>,
    pub outcome: Option<FiringOutcome>,
}

/// Per-session counts. `firings`, `published`, and `superseded_before_activation` commit with the transition they count; `validation_rejected`, `invalidated`, and `connect_failed` are written after the failure and can be lost to a crash in between.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FiringCounters {
    /// Model attempts that advanced `firing_seq`.
    pub firings: u64,
    pub published: u64,
    /// Published segments a revert removed before any pass rendered them.
    pub superseded_before_activation: u64,
    pub validation_rejected: u64,
    pub invalidated: u64,
    /// Producer connections that failed, whether or not a firing had started.
    pub connect_failed: u64,
}

/// The first pass that could have fired but found no run able to start, with the reason it gave.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingEligibility {
    pub eligible_at_ms: i64,
    pub no_fire: Option<NoFire>,
}

impl HistorySummarizerDurableState {
    /// Appends the entry for the firing `firing_seq` names, counting it and consuming the pending eligibility of a pressure-path firing. `prior_no_fire` is the `last_no_fire` the fire cleared; the eligibility's reason stands in when it is absent.
    pub fn record_fire(
        &mut self,
        trigger: FiringTrigger,
        fired_at_ms: i64,
        prior_no_fire: Option<NoFire>,
    ) {
        let pending = match trigger.source {
            FiringSource::PressurePath => self.pending_eligibility.take(),
            FiringSource::Wrapup | FiringSource::Reattach => None,
        };
        let (eligible_at_ms, pending_no_fire) = match pending {
            Some(pending) => (pending.eligible_at_ms, pending.no_fire),
            None => (fired_at_ms, None),
        };
        self.counters.firings = self.counters.firings.saturating_add(1);
        self.push_firing(RecentFiring {
            trigger_reason: trigger.reason,
            usage: trigger.usage,
            eligible_at_ms: Some(eligible_at_ms),
            last_no_fire: prior_no_fire.or(pending_no_fire),
            fired_at_ms: Some(fired_at_ms),
            ..RecentFiring::new(self.firing_seq, trigger.source)
        });
    }

    /// The in-flight firing's entry, created with source `Reattach` for a firing recorded before this timeline.
    pub fn current_firing_mut(&mut self) -> &mut RecentFiring {
        let firing_seq = self.firing_seq;
        match self
            .recent_firings
            .iter()
            .position(|entry| entry.firing_seq == firing_seq)
        {
            Some(index) => &mut self.recent_firings[index],
            None => {
                self.push_firing(RecentFiring::new(firing_seq, FiringSource::Reattach));
                self.recent_firings.last_mut().expect("entry just pushed")
            }
        }
    }

    /// The single writer of an entry's outcome: it records `outcome` on the entry `firing_seq` names unless that entry already ended, and counts what it records.
    pub fn record_outcome(&mut self, outcome: FiringOutcome) {
        let entry = self.current_firing_mut();
        if entry
            .outcome
            .is_some_and(|recorded| recorded != FiringOutcome::ReattachConnectFailed)
        {
            return;
        }
        entry.outcome = Some(outcome);
        let counters = &mut self.counters;
        let counter = match outcome {
            FiringOutcome::Published { .. } => &mut counters.published,
            FiringOutcome::Abandoned {
                class: AbandonClass::ValidationRejected,
            } => &mut counters.validation_rejected,
            FiringOutcome::Abandoned {
                class:
                    AbandonClass::Invalidated
                    | AbandonClass::FingerprintMismatch
                    | AbandonClass::CallerFenceRejected,
            } => &mut counters.invalidated,
            FiringOutcome::Abandoned {
                class: AbandonClass::ConnectFailed,
            }
            | FiringOutcome::ReattachConnectFailed => &mut counters.connect_failed,
            FiringOutcome::Abandoned { .. } => return,
        };
        *counter = counter.saturating_add(1);
    }

    /// Stamps every entry whose published sequence a commit that renders through `rendered` shows for the first time, that is, above `previously_rendered`. A still-pending segment stays unstamped.
    pub fn record_activation(&mut self, previously_rendered: i64, rendered: i64, at_ms: i64) {
        for entry in &mut self.recent_firings {
            if let Some(FiringOutcome::Published {
                sequence: Some(sequence),
            }) = entry.outcome
                && previously_rendered < sequence
                && sequence <= rendered
                && entry.activated_at_ms.is_none()
            {
                entry.activated_at_ms = Some(at_ms);
            }
        }
    }

    fn push_firing(&mut self, entry: RecentFiring) {
        if self.recent_firings.len() >= RECENT_FIRINGS_CAPACITY {
            let excess = self.recent_firings.len() + 1 - RECENT_FIRINGS_CAPACITY;
            self.recent_firings.drain(..excess);
        }
        self.recent_firings.push(entry);
    }
}

impl RecentFiring {
    fn new(firing_seq: u64, source: FiringSource) -> Self {
        RecentFiring {
            firing_seq,
            source,
            trigger_reason: None,
            usage: None,
            clock: TimelineClock::DaemonWallMs,
            eligible_at_ms: None,
            last_no_fire: None,
            fired_at_ms: None,
            producer_started_at_ms: None,
            output_received_at_ms: None,
            published_at_ms: None,
            activated_at_ms: None,
            outcome: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MAX_DURABLE_TEXT_BYTES, ModuleMeta};

    fn fired(state: &mut HistorySummarizerDurableState, at: i64) {
        state.firing_seq += 1;
        state.record_fire(FiringTrigger::default(), at, None);
    }

    #[test]
    fn the_ninth_firing_evicts_only_the_oldest_entry() {
        let mut state = HistorySummarizerDurableState::default();
        for at in 1..=9 {
            fired(&mut state, at);
        }
        let kept: Vec<_> = state.recent_firings.iter().map(|e| e.firing_seq).collect();
        assert_eq!(kept, (2..=9).collect::<Vec<_>>());
        assert_eq!(state.counters.firings, 9);
    }

    #[test]
    fn a_terminal_outcome_is_never_overwritten_and_only_accepted_outcomes_count() {
        let mut state = HistorySummarizerDurableState::default();
        fired(&mut state, 1);
        state.record_outcome(FiringOutcome::ReattachConnectFailed);
        state.record_outcome(FiringOutcome::ReattachConnectFailed);
        assert_eq!(state.counters.connect_failed, 2);
        state.record_outcome(FiringOutcome::Abandoned {
            class: AbandonClass::ValidationRejected,
        });
        state.record_outcome(FiringOutcome::Published { sequence: Some(4) });
        state.record_outcome(FiringOutcome::Abandoned {
            class: AbandonClass::Invalidated,
        });
        assert_eq!(
            state.recent_firings[0].outcome,
            Some(FiringOutcome::Abandoned {
                class: AbandonClass::ValidationRejected
            })
        );
        assert_eq!(
            state.counters,
            FiringCounters {
                firings: 1,
                validation_rejected: 1,
                connect_failed: 2,
                ..FiringCounters::default()
            }
        );
    }

    #[test]
    fn each_abandon_class_lands_in_its_counter() {
        for (class, expected) in [
            (
                AbandonClass::FingerprintMismatch,
                FiringCounters {
                    invalidated: 1,
                    ..Default::default()
                },
            ),
            (
                AbandonClass::CallerFenceRejected,
                FiringCounters {
                    invalidated: 1,
                    ..Default::default()
                },
            ),
            (
                AbandonClass::ConnectFailed,
                FiringCounters {
                    connect_failed: 1,
                    ..Default::default()
                },
            ),
            (AbandonClass::ProducerFailed, FiringCounters::default()),
            (AbandonClass::Restarted, FiringCounters::default()),
        ] {
            let mut state = HistorySummarizerDurableState::default();
            state.record_outcome(FiringOutcome::Abandoned { class });
            assert_eq!(state.counters, expected, "{class:?}");
        }
    }

    #[test]
    fn a_fire_consumes_the_pending_eligibility_and_classifies_the_prior_no_fire() {
        let mut state = HistorySummarizerDurableState {
            pending_eligibility: Some(PendingEligibility {
                eligible_at_ms: 10,
                no_fire: Some(NoFire::new(NoFireReason::Busy, "busy")),
            }),
            firing_seq: 1,
            ..Default::default()
        };
        state.record_fire(
            FiringTrigger {
                source: FiringSource::Wrapup,
                ..Default::default()
            },
            20,
            Some(NoFire::new(NoFireReason::Busy, "busy")),
        );
        assert_eq!(state.recent_firings[0].eligible_at_ms, Some(20));
        assert!(state.pending_eligibility.is_some(), "wrapup leaves it");
        let mut without_prior = state.clone();
        without_prior.firing_seq = 2;
        without_prior.record_fire(FiringTrigger::default(), 30, None);
        assert_eq!(
            without_prior.recent_firings[1].last_no_fire,
            Some(NoFire::new(NoFireReason::Busy, "busy"))
        );
        state.firing_seq = 2;
        state.record_fire(
            FiringTrigger::default(),
            30,
            Some(NoFire::new(NoFireReason::Backoff, "backoff")),
        );
        let entry = &state.recent_firings[1];
        assert_eq!(entry.eligible_at_ms, Some(10));
        assert_eq!(
            entry.last_no_fire.as_ref().unwrap().reason,
            NoFireReason::Backoff
        );
        assert_eq!(state.pending_eligibility, None);
    }

    #[test]
    fn a_no_fire_detail_is_cut_at_a_character_boundary() {
        // 127 ASCII bytes then a two-byte character straddling the cap.
        let text = format!("{}é tail", "a".repeat(NO_FIRE_DETAIL_MAX_BYTES - 1));
        let detail = NoFire::new(NoFireReason::Other, &text).detail;
        assert_eq!(detail, "a".repeat(NO_FIRE_DETAIL_MAX_BYTES - 1));
        let exact = "b".repeat(NO_FIRE_DETAIL_MAX_BYTES + 40);
        assert_eq!(
            NoFire::new(NoFireReason::Other, &exact).detail.len(),
            NO_FIRE_DETAIL_MAX_BYTES
        );
    }

    #[test]
    fn meta_written_before_the_timeline_loads_with_defaults() {
        let mut json = serde_json::to_value(ModuleMeta::default()).unwrap();
        json["history_summarizer"] = serde_json::json!({"state": "idle", "firing_seq": 4});
        let state = serde_json::from_value::<ModuleMeta>(json)
            .unwrap()
            .history_summarizer;
        assert_eq!(state.firing_seq, 4);
        assert!(state.recent_firings.is_empty());
        assert_eq!(state.counters, FiringCounters::default());
        assert_eq!(state.pending_eligibility, None);
    }

    #[test]
    fn a_full_worst_case_timeline_serializes_under_the_durable_text_bound() {
        let entry = RecentFiring {
            firing_seq: u64::MAX,
            source: FiringSource::PressurePath,
            trigger_reason: Some(FiringTriggerReason::ProjectedHeadroom),
            usage: Some(FiringUsage {
                input_tokens: u64::MAX,
                context_limit_tokens: u64::MAX,
                usage_percentage: u32::MAX,
                execute_threshold_percentage: u32::MAX,
            }),
            clock: TimelineClock::DaemonWallMs,
            eligible_at_ms: Some(i64::MIN),
            last_no_fire: Some(NoFire {
                reason: NoFireReason::ContinuedOrdinalOffsetMissing,
                detail: "\u{1}".repeat(NO_FIRE_DETAIL_MAX_BYTES),
            }),
            fired_at_ms: Some(i64::MIN),
            producer_started_at_ms: Some(i64::MIN),
            output_received_at_ms: Some(i64::MIN),
            published_at_ms: Some(i64::MIN),
            activated_at_ms: Some(i64::MIN),
            outcome: Some(FiringOutcome::Abandoned {
                class: AbandonClass::CallerFenceRejected,
            }),
        };
        let meta = ModuleMeta {
            history_summarizer: HistorySummarizerDurableState {
                recent_firings: vec![entry; RECENT_FIRINGS_CAPACITY],
                counters: FiringCounters {
                    firings: u64::MAX,
                    published: u64::MAX,
                    superseded_before_activation: u64::MAX,
                    validation_rejected: u64::MAX,
                    invalidated: u64::MAX,
                    connect_failed: u64::MAX,
                },
                pending_eligibility: Some(PendingEligibility {
                    eligible_at_ms: i64::MIN,
                    no_fire: Some(NoFire {
                        reason: NoFireReason::ContinuedOrdinalOffsetMissing,
                        detail: "\u{1}".repeat(NO_FIRE_DETAIL_MAX_BYTES),
                    }),
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        let bytes = serde_json::to_string(&meta).unwrap().len();
        let empty = serde_json::to_string(&ModuleMeta::default()).unwrap().len();
        assert!(
            bytes - empty <= TIMELINE_WORST_CASE_BYTES,
            "{bytes} - {empty}"
        );
        assert!(bytes < MAX_DURABLE_TEXT_BYTES, "{bytes}");
    }

    /// The timeline's share of the meta column: eight full entries whose details escape every byte, full counters, and a pending eligibility, well inside the 512 KiB durable-text bound the rest of the meta shares.
    const TIMELINE_WORST_CASE_BYTES: usize = 16 * 1024;
}
