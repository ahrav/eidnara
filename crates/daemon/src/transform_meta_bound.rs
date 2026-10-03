#![forbid(unsafe_code)]

use std::sync::Arc;

use memory_store::{HistorySummarizerPhase, MemoryStore, MemoryStoreError};

use crate::history_summarizer_chunk::SELECTED_IDENTITY_BUDGET_BYTES;
use crate::test_support::synthetic_history::{SyntheticHistory, seed_active_summarizer};
use crate::transform::tests::{item, pctx, req, resolved, store};
use crate::transform::{
    TransformError, TransformRequest, TransformResponse, install_transform_attempt_hook,
    transform_with_projection_cached,
};

const SESSION: &str = "meta-bound";
const WINDOW: u64 = 300;

/// Every test here uses [`SESSION`], and the attempt hook is keyed by session, so a test's
/// pass could consume another test's one-shot hook; the tests run one at a time.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A store holding `h` synthetic segments over messages 1..=2h and the continuation base the
/// window continues, so an unanchored first pass numbers the window from `2h - 1`.
fn seeded(h: usize) -> (tempfile::TempDir, Arc<MemoryStore>) {
    let dir = tempfile::tempdir().expect("store dir");
    let store = Arc::new(store(dir.path()));
    SyntheticHistory::mixed(h).seed(&store, SESSION);
    let empty = store.load(SESSION).expect("load empty");
    let mut meta = empty.meta.clone();
    meta.ordinal_continuation_base = Some(2 * h as u64 - 2);
    store
        .commit(SESSION, None, &empty.core, &meta)
        .expect("seed the continuation base");
    (dir, store)
}

/// A `null` window of [`WINDOW`] messages: the newest segment's two messages, then the tail.
fn window(h: usize) -> TransformRequest {
    let first = 2 * h as u64 - 1;
    let messages = (first..first + WINDOW)
        .map(|n| item(&format!("m{n}"), n, &format!("message {n} with some text")))
        .collect();
    req(SESSION, "cfg0", messages)
}

fn pass(
    store: &MemoryStore,
    request: &TransformRequest,
) -> Result<TransformResponse, TransformError> {
    let ctx = pctx("git:meta-bound", "/nonexistent-docs", 1_700_000_000_000);
    transform_with_projection_cached(store, &resolved(store, request), &ctx).map(|out| out.response)
}

/// The stored `meta` text's length, as the durable-text bound measures it.
fn meta_row_bytes(store: &MemoryStore) -> usize {
    store
        .with_fenced_conn_for_test(|tx| {
            tx.query_row(
                "SELECT length(CAST(meta AS BLOB)) FROM cache_state WHERE session_id = ?1",
                [SESSION],
                |row| row.get::<_, i64>(0),
            )
        })
        .expect("meta length") as usize
}

/// The submitted window's mids, cut prefix included (spec D12).
fn window_mids(request: &TransformRequest) -> Vec<String> {
    let mut mids: Vec<String> = request
        .messages
        .iter()
        .map(|message| message.mid.clone())
        .collect();
    mids.sort();
    mids
}

fn identity_mids(store: &MemoryStore) -> Vec<String> {
    store
        .all_block_identities_for_test(SESSION)
        .into_keys()
        .collect()
}

#[test]
fn a_hundred_thousand_message_session_commits_a_three_hundred_message_window() {
    let _serial = serial();
    let (_dir, store) = seeded(50_000);
    let request = window(50_000);
    let expected = window_mids(&request);
    let first = pass(&store, &request).expect("the window commits");
    assert_eq!(first.action, "HARD");
    assert!(first.committed);
    let after = store.load(SESSION).expect("load after");
    assert_eq!(after.meta.coverage_ordinal, Some(100_000));
    assert_eq!(identity_mids(&store), expected);
    let bytes = meta_row_bytes(&store);
    assert!(
        bytes < 128 * 1024,
        "a 100,000-message session commits {bytes} meta bytes"
    );
}

/// A session written before identities were pruned (WP-P25): a `block_identities` row for every
/// covered message, a `meta` row carrying the embedded identity map that preceded the table
/// near the 512 KiB bound, and a history_summarizer firing awaiting its producer. Returns the
/// store, the next window, and the version the legacy row was written at.
fn legacy_session() -> (tempfile::TempDir, Arc<MemoryStore>, TransformRequest, u64) {
    let h = 1_000;
    let (dir, store) = seeded(h);
    let request = window(h);
    pass(&store, &request).expect("first pass");
    seed_active_summarizer(&store, SESSION);
    let loaded = store.load(SESSION).expect("load");
    let vector = store
        .all_block_identities_for_test(SESSION)
        .into_values()
        .next()
        .unwrap();
    let history = memory_store::BlockIdentityDelta {
        upserts: (1..2 * h as u64 - 1)
            .map(|n| (format!("m{n}"), vector.clone()))
            .collect(),
        ..Default::default()
    };
    let version = store
        .commit_with_block_identities_for_test(
            SESSION,
            loaded.row_version,
            &loaded.core,
            &loaded.meta,
            &history,
        )
        .expect("write every covered identity");
    let entry = serde_json::to_string(&vector).unwrap();
    let embedded = (0..)
        .map(|n| format!("\"legacy-{n}\":{entry}"))
        .scan(0, |bytes, pair| {
            *bytes += pair.len() + 1;
            (*bytes < 500 * 1024 - meta_row_bytes(&store)).then_some(pair)
        })
        .collect::<Vec<_>>()
        .join(",");
    store
        .with_fenced_conn_for_test(|tx| {
            tx.execute(
                "UPDATE cache_state SET meta = json_set(meta, '$.block_identity_by_mid', json(?2))
                  WHERE session_id = ?1",
                rusqlite::params![SESSION, format!("{{{embedded}}}")],
            )
        })
        .expect("embed the legacy map");
    let bytes = meta_row_bytes(&store);
    assert!(
        (480 * 1024..512 * 1024).contains(&bytes),
        "the legacy row holds {bytes} meta bytes"
    );
    let mut next = request;
    let ordinal = 2 * h as u64 - 1 + WINDOW;
    next.messages.push(item(
        &format!("m{ordinal}"),
        ordinal,
        "the message after the legacy window",
    ));
    (dir, store, next, version)
}

fn assert_history_kept(store: &MemoryStore) {
    let loaded = store.load(SESSION).expect("load kept");
    assert_eq!(identity_mids(store).len(), 1_998 + WINDOW as usize + 1);
    assert!(meta_row_bytes(store) < 128 * 1024);
    assert_eq!(
        loaded.meta.history_summarizer.state,
        HistorySummarizerPhase::AwaitingProducer
    );
}

#[test]
fn a_legacy_row_is_read_after_a_restart_and_keeps_its_identity_rows_on_its_first_commit() {
    let _serial = serial();
    let (dir, legacy_store, request, _) = legacy_session();
    drop(legacy_store);
    let store = store(dir.path());
    store.load(SESSION).expect("the legacy row reads");
    let answer = pass(&store, &request).expect("the first pass commits");
    assert!(answer.committed);
    assert_eq!(answer.action, "SOFT+");
    assert_history_kept(&store);
}

#[test]
fn a_pass_that_loses_its_cas_reloads_and_keeps_the_identity_history() {
    let _serial = serial();
    let (_dir, store, request, _) = legacy_session();
    let hook_store = Arc::clone(&store);
    install_transform_attempt_hook(SESSION, move || {
        let loaded = hook_store.load(SESSION).unwrap();
        hook_store
            .commit(SESSION, loaded.row_version, &loaded.core, &loaded.meta)
            .unwrap();
    });
    let answer = pass(&store, &request).expect("the retry commits");
    assert!(answer.committed);
    assert_history_kept(&store);
}

#[test]
fn a_writer_that_loses_to_a_pass_reloads_the_row() {
    let _serial = serial();
    let (_dir, store, request, version) = legacy_session();
    let stale = store.load(SESSION).unwrap();
    assert_eq!(stale.row_version, Some(version));
    pass(&store, &request).expect("the pass commits");
    assert!(matches!(
        store.commit(SESSION, Some(version), &stale.core, &stale.meta),
        Err(MemoryStoreError::CasConflict { .. })
    ));
    let reloaded = store.load(SESSION).unwrap();
    store
        .commit(
            SESSION,
            reloaded.row_version,
            &reloaded.core,
            &reloaded.meta,
        )
        .expect("the reloaded writer commits");
    assert_history_kept(&store);
}

fn object(fields: &[(&str, usize)]) -> usize {
    2 + fields
        .iter()
        .map(|(field, bytes)| field.len() + 4 + bytes)
        .sum::<usize>()
}

macro_rules! inventory {
    ($($ty:ident)::+ { $($field:ident => $bound:expr;)* }) => {{
        let _every_field = |value: $($ty)::+| {
            let $($ty)::+ { $($field: _,)* } = value;
        };
        vec![$((stringify!($field), $bound)),*]
    }};
}

macro_rules! longest_variant {
    ($ty:path { $($variant:ident),* $(,)? }) => {{
        type Enum = $ty;
        let _every_variant = |value: Enum| match value {
            $(Enum::$variant => (),)*
        };
        [$(serde_json::to_string(&Enum::$variant).unwrap().len()),*]
            .into_iter()
            .max()
            .unwrap()
    }};
}

const IDENT: usize = 34;
const INT: usize = 20;
const U32: usize = 10;
const BOOLEAN: usize = 5;

fn ascii(bytes: usize) -> usize {
    bytes + 2
}

fn history_summarizer_bound() -> usize {
    use memory_store::summarizer_timeline as timeline;
    let variants = [
        longest_variant!(memory_store::HistorySummarizerPhase {
            Idle,
            Firing,
            AwaitingProducer,
            Validating,
            Publishing,
        }),
        longest_variant!(timeline::FiringSource {
            PressurePath,
            Wrapup,
            Reattach
        }),
        longest_variant!(timeline::FiringTriggerReason {
            ProjectedHeadroom,
            ForceBand,
            CommitClusters,
            TailSize,
        }),
        longest_variant!(timeline::TimelineClock { DaemonWallMs }),
        longest_variant!(memory_store::HistorySummarizerAbandonReason { WindowArchived }),
        longest_variant!(timeline::NoFireReason {
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
        }),
        longest_variant!(timeline::AbandonClass {
            ProducerFailed,
            ValidationRejected,
            Invalidated,
            FingerprintMismatch,
            CallerFenceRejected,
            ProducerMissing,
            Restarted,
            ConnectFailed,
            ReservationSettled,
            HandoffFailed,
        }),
        longest_variant!(memory_store::ExtractionFailure {
            UnknownAlias,
            InvalidSpan,
            OutsideAcceptedSegment,
            TooManyCitations,
            TooManyFacts,
            MalformedCitation,
            MalformedFacts,
            MissingCitation,
            MalformedClaims,
        }),
    ];
    assert!(variants.iter().all(|&bytes| bytes <= IDENT), "{variants:?}");
    let nonadmission_tags = [
        memory_store::MemoryReviewerNonadmissionCode::MEMORY_REVIEWER_UNAVAILABLE_TAG,
        memory_store::MemoryReviewerNonadmissionCode::CAPACITY_FULL_TAG,
        memory_store::MemoryReviewerNonadmissionCode::EVIDENCE_UNAVAILABLE_TAG,
        memory_store::MemoryReviewerNonadmissionCode::FACT_SET_REJECTED_TAG,
        memory_store::MemoryReviewerNonadmissionCode::SUBJECT_REFUSED_TAG,
        memory_store::MemoryReviewerNonadmissionCode::UNRECOGNIZED_TAG,
    ];
    assert!(
        nonadmission_tags
            .iter()
            .all(|tag| ascii(tag.len()) <= IDENT)
    );
    let _every_code = |code: memory_store::MemoryReviewerNonadmissionCode| {
        use memory_store::MemoryReviewerNonadmissionCode::*;
        match code {
            MemoryReviewerUnavailable
            | CapacityFull
            | EvidenceUnavailable
            | FactSetRejected { failure: _ }
            | SubjectRefused
            | Unrecognized => (),
        }
    };
    let _every_outcome = |outcome: timeline::FiringOutcome| match outcome {
        timeline::FiringOutcome::Published { sequence: _ }
        | timeline::FiringOutcome::Abandoned { class: _ }
        | timeline::FiringOutcome::ReattachConnectFailed => (),
    };

    let digest = ascii(64);
    assert_eq!(
        crate::history_summarizer::compute_chunk_fingerprint(&[]).len(),
        64
    );
    assert_eq!(crate::history_summarizer::model_chain_digest(&[]).len(), 64);
    let detail = ascii(crate::history_summarizer::MAX_SUMMARIZER_DETAIL_BYTES);
    let producer_identity = ascii(crate::history_summarizer::MAX_PRODUCER_IDENTITY_BYTES);
    let reservation_identity = ascii(memory_store::MAX_MEMORY_REVIEWER_RESERVATION_ID_BYTES);

    let no_fire = object(&inventory!(timeline::NoFire {
        reason => IDENT;
        detail => ascii(timeline::NO_FIRE_DETAIL_MAX_BYTES);
    }));
    let usage = object(&inventory!(timeline::FiringUsage {
        input_tokens => INT;
        context_limit_tokens => INT;
        usage_percentage => U32;
        execute_threshold_percentage => U32;
    }));
    let outcome = object(&[("kind", IDENT), ("class", IDENT.max(INT))]);
    let recent_firing = object(&inventory!(timeline::RecentFiring {
        firing_seq => INT;
        source => IDENT;
        trigger_reason => IDENT;
        usage => usage;
        clock => IDENT;
        eligible_at_ms => INT;
        last_no_fire => no_fire;
        fired_at_ms => INT;
        producer_started_at_ms => INT;
        output_received_at_ms => INT;
        published_at_ms => INT;
        activated_at_ms => INT;
        activated_by_first_fold => BOOLEAN;
        outcome => outcome;
    }));
    let recent_firings = timeline::RECENT_FIRINGS_CAPACITY * (recent_firing + 1) + 2;
    let counters = object(&inventory!(timeline::FiringCounters {
        firings => INT;
        published => INT;
        superseded_before_activation => INT;
        validation_rejected => INT;
        invalidated => INT;
        connect_failed => INT;
    }));
    let pending_eligibility = object(&inventory!(timeline::PendingEligibility {
        eligible_at_ms => INT;
        no_fire => no_fire;
    }));
    let nonadmission_code = object(&[("code", IDENT), ("failure", IDENT)]);
    let recorded_nonadmission = object(&inventory!(memory_store::RecordedNonadmission {
        firing_seq => INT;
        code => nonadmission_code;
    }));
    let nonadmission = object(&inventory!(memory_store::MemoryReviewerNonadmission {
        count => INT;
        latest => recorded_nonadmission;
    }));
    let reservation = object(&inventory!(memory_store::MemoryReviewerReservation {
        firing_seq => INT;
        causal_identity => reservation_identity;
        candidate_id => reservation_identity;
        payload_digest => reservation_identity;
        kernel_incarnation => reservation_identity;
        queue_deadline_ms => INT;
    }));
    let chunk_range = object(&inventory!(memory_store::HistorySummarizerChunkRange {
        from_ordinal => INT;
        to_ordinal => INT;
    }));
    let generation = object(&inventory!(memory_store::HistorySegmentSetGeneration {
        max_sequence => INT;
        count => INT;
    }));
    let chunk_retry = object(&inventory!(memory_store::HistorySummarizerChunkRetry {
        chunk_start => INT;
        chunk_end => INT;
        failures => U32;
        model_chain_digest => digest;
        token_budget => INT;
    }));
    let _selected_identity = inventory!(memory_store::HistorySummarizerSelectedMessageIdentity {
        mid => SELECTED_IDENTITY_BUDGET_BYTES;
        block_identities => SELECTED_IDENTITY_BUDGET_BYTES;
    });
    let _block_identity = inventory!(memory_store::BlockIdentity {
        kind_tag => SELECTED_IDENTITY_BUDGET_BYTES;
        byte_fingerprint => SELECTED_IDENTITY_BUDGET_BYTES;
    });
    object(&inventory!(memory_store::HistorySummarizerDurableState {
        state => IDENT;
        firing_seq => INT;
        chunk_range => chunk_range;
        chunk_fingerprint => digest;
        selected_range_identities => 0;
        presented_token_budget => INT;
        producer_session_id => producer_identity;
        producer_run_id => producer_identity;
        producer_harness => producer_identity;
        fired_at_ms => INT;
        expected_revert_epoch => INT;
        history_segment_set_generation => generation;
        failure_backoff_at_ms => INT;
        last_failure => detail;
        last_no_fire => detail;
        consecutive_publish_failures => U32;
        chunk_retry => chunk_retry;
        memory_reviewer_nonadmission => nonadmission;
        memory_reviewer_reservation => reservation;
        recent_firings => recent_firings;
        counters => counters;
        pending_eligibility => pending_eligibility;
        withdrawn_selected_mid => crate::wire::MAX_MID_BYTES * 6 + 2;
        last_abandon => object(&inventory!(memory_store::HistorySummarizerAbandon {
            firing_seq => INT;
            reason => IDENT;
            abandoned_at_ms => INT;
        }));
    }))
}

fn worst_case_history_summarizer() -> memory_store::HistorySummarizerDurableState {
    use memory_store::summarizer_timeline::{self as timeline, NoFire};
    let detail = "\"".repeat(crate::history_summarizer::MAX_SUMMARIZER_DETAIL_BYTES / 2);
    let no_fire = NoFire {
        reason: timeline::NoFireReason::ContinuedOrdinalOffsetMissing,
        detail: "\"".repeat(timeline::NO_FIRE_DETAIL_MAX_BYTES / 2),
    };
    let identity = "\\".repeat(crate::history_summarizer::MAX_PRODUCER_IDENTITY_BYTES / 2);
    let reservation_identity =
        "\\".repeat(memory_store::MAX_MEMORY_REVIEWER_RESERVATION_ID_BYTES / 2);
    let firing = timeline::RecentFiring {
        firing_seq: u64::MAX,
        source: timeline::FiringSource::PressurePath,
        trigger_reason: Some(timeline::FiringTriggerReason::ProjectedHeadroom),
        usage: Some(timeline::FiringUsage {
            input_tokens: u64::MAX,
            context_limit_tokens: u64::MAX,
            usage_percentage: u32::MAX,
            execute_threshold_percentage: u32::MAX,
        }),
        clock: timeline::TimelineClock::DaemonWallMs,
        eligible_at_ms: Some(i64::MIN),
        last_no_fire: Some(no_fire.clone()),
        fired_at_ms: Some(i64::MIN),
        producer_started_at_ms: Some(i64::MIN),
        output_received_at_ms: Some(i64::MIN),
        published_at_ms: Some(i64::MIN),
        activated_at_ms: Some(i64::MIN),
        activated_by_first_fold: true,
        outcome: Some(timeline::FiringOutcome::Abandoned {
            class: timeline::AbandonClass::CallerFenceRejected,
        }),
    };
    memory_store::HistorySummarizerDurableState {
        state: memory_store::HistorySummarizerPhase::AwaitingProducer,
        firing_seq: u64::MAX,
        chunk_range: Some(memory_store::HistorySummarizerChunkRange {
            from_ordinal: u64::MAX,
            to_ordinal: u64::MAX,
        }),
        chunk_fingerprint: crate::history_summarizer::compute_chunk_fingerprint(&[]),
        selected_range_identities: Vec::new(),
        presented_token_budget: Some(usize::MAX),
        producer_session_id: Some(identity.clone()),
        producer_run_id: Some(identity.clone()),
        producer_harness: Some(identity),
        fired_at_ms: Some(i64::MIN),
        expected_revert_epoch: u64::MAX,
        history_segment_set_generation: memory_store::HistorySegmentSetGeneration {
            max_sequence: i64::MIN,
            count: i64::MIN,
        },
        failure_backoff_at_ms: Some(i64::MIN),
        last_failure: Some(detail.clone()),
        last_no_fire: Some(detail),
        consecutive_publish_failures: u32::MAX,
        chunk_retry: Some(memory_store::HistorySummarizerChunkRetry {
            chunk_start: u64::MAX,
            chunk_end: u64::MAX,
            failures: u32::MAX,
            model_chain_digest: crate::history_summarizer::model_chain_digest(&[]),
            token_budget: usize::MAX,
        }),
        memory_reviewer_nonadmission: memory_store::MemoryReviewerNonadmission {
            count: u64::MAX,
            latest: Some(memory_store::RecordedNonadmission {
                firing_seq: u64::MAX,
                code: memory_store::MemoryReviewerNonadmissionCode::FactSetRejected {
                    failure: memory_store::ExtractionFailure::OutsideAcceptedSegment,
                },
            }),
        },
        memory_reviewer_reservation: Some(memory_store::MemoryReviewerReservation {
            firing_seq: u64::MAX,
            causal_identity: reservation_identity.clone(),
            candidate_id: reservation_identity.clone(),
            payload_digest: reservation_identity.clone(),
            kernel_incarnation: reservation_identity,
            queue_deadline_ms: i64::MIN,
        }),
        recent_firings: vec![firing; timeline::RECENT_FIRINGS_CAPACITY],
        counters: timeline::FiringCounters {
            firings: u64::MAX,
            published: u64::MAX,
            superseded_before_activation: u64::MAX,
            validation_rejected: u64::MAX,
            invalidated: u64::MAX,
            connect_failed: u64::MAX,
        },
        pending_eligibility: Some(timeline::PendingEligibility {
            eligible_at_ms: i64::MIN,
            no_fire: Some(no_fire),
        }),
        withdrawn_selected_mid: Some("\u{1}".repeat(crate::wire::MAX_MID_BYTES)),
        last_abandon: Some(memory_store::HistorySummarizerAbandon {
            firing_seq: u64::MAX,
            reason: memory_store::HistorySummarizerAbandonReason::WindowArchived,
            abandoned_at_ms: i64::MIN,
        }),
    }
}

#[test]
fn every_history_summarizer_field_has_an_enforced_bound() {
    let bound = history_summarizer_bound();
    let worst = serde_json::to_string(&worst_case_history_summarizer())
        .unwrap()
        .len();
    assert!(worst <= bound, "{worst} > {bound}");
    eprintln!("history_summarizer bound: {bound} bytes, worst case built: {worst} bytes");
}

fn worst_case_synthetic_todo() -> memory_store::FrozenSyntheticTodoPair {
    let content = "\"".repeat(memory_store::MAX_TODO_STATE_BYTES / 2 - 64);
    let state = crate::injection::normalize_todo_state_json(&format!(
        r#"[{{"content":{},"status":"in_progress","priority":"high"}}]"#,
        serde_json::to_string(&content).unwrap()
    ))
    .unwrap();
    assert!(state.len() <= memory_store::MAX_TODO_STATE_BYTES);
    crate::injection::build_synthetic_todo_pair(&state)
        .unwrap()
        .freeze_at(Some("m".repeat(crate::wire::MAX_MID_BYTES)))
}

fn recorded_metadata_bounds() -> Vec<(&'static str, usize)> {
    const ESCAPED: usize = 6;
    let text = |bytes: usize| bytes * ESCAPED + 2;
    let boolean = BOOLEAN;
    let int = INT;
    let hash = text(64);
    let mid_bytes = crate::wire::MAX_MID_BYTES;
    let mid = text(mid_bytes);
    let block_id = text(mid_bytes + 21);
    let request_identity = text(crate::transform::MAX_REQUEST_IDENTITY_BYTES);
    let label = text(32);
    let pending_hint_ids = crate::transform::MAX_PENDING_USER_HINT_BLOCK_IDS * (block_id + 1) + 2;
    let note_nudge_anchors =
        memory_store::MAX_NOTE_NUDGE_ANCHORS * (memory_store::MAX_NOTE_NUDGE_ANCHOR_BYTES + 1) + 2;
    let legacy_seqs = memory_store::MAX_LEGACY_HISTORY_SEGMENTS * 21 + 2;
    let directive = text(memory_store::MAX_STATE_SYNC_DIRECTIVE_BYTES) + 16;
    let synthetic_todo = {
        let built = serde_json::to_vec(&worst_case_synthetic_todo())
            .unwrap()
            .len();
        assert!(
            built <= memory_store::MAX_SYNTHETIC_TODO_PAIR_BYTES,
            "{built}"
        );
        memory_store::MAX_SYNTHETIC_TODO_PAIR_BYTES
    };

    inventory!(memory_store::ModuleMeta {
        initialized => boolean;
        bootstrap_seed_fold_pending => boolean;
        last_render_config => text(5 * crate::transform::MAX_REQUEST_IDENTITY_BYTES) + RENDER_IDENTITY_FRAMING_BYTES;
        last_provider_id => request_identity;
        last_model_key => request_identity;
        last_system_prompt_hash => request_identity;
        last_upgrade_state => request_identity;
        coverage_ordinal => int;
        last_todo_state => memory_store::MAX_TODO_STATE_SERIALIZED_BYTES;
        last_todo_state_owner_message_id => mid;
        last_todo_state_hash => hash;
        soft_refresh_pending => boolean;
        guidance_date => label;
        revert_epoch => int;
        last_recut => ascii(memory_store::MAX_LAST_RECUT_BYTES);
        pending_rewrite => object(&inventory!(memory_store::PendingRewriteState {
            armed_at_ms => INT;
            absent_shape_fingerprint => ascii(64);
            absent_request_count => INT;
            last_present_at_ms => INT;
        }));
        pending_rewrite_trip_count => int;
        pending_rewrite_ambiguous => boolean;
        pending_rewrite_last_failure => ascii(memory_store::MAX_PENDING_REWRITE_DETAIL_BYTES);
        synthetic_todo => synthetic_todo;
        note_nudge_anchors => note_nudge_anchors;
        m1_revision => int;
        m1_history_segment_seq => int;
        memory_disabled => boolean;
        m1_external_revision => int;
        project_memory_epoch => int;
        project_memory_epoch_pending => boolean;
        user_profile_version => int;
        m1_user_profile_version => int;
        m1_pending_since_ms => int;
        folded_history_segment_seq => int;
        archive_fold_seq => int;
        legacy_history_segment_seqs => legacy_seqs;
        history_segments_ordered => boolean;
        coverage_start_ordinal => int;
        coverage_history_segment_seq => int;
        additive_served_history_segment_seq => int;
        project_memory => 256;
        expiry_cutoff_ms => int;
        history_summarizer => history_summarizer_bound();
        publication_floor_ordinal => int;
        block_identity_basis => label;
        tail_identity_re_adopt_count => int;
        newest_live_block_id => block_id;
        last_usage => 256;
        last_serializer_profile => label;
        reasoning_cleared_through_ordinal => int;
        reasoning_cleared_through_tag => int;
        terse_text_compression_age_basis_tag => int;
        cc_u1_active => boolean;
        tagging_surface_active => boolean;
        channel1_last_nudge_undropped => int;
        channel1_last_nudge_level => label;
        tail_hygiene_baseline => 512;
        pending_user_hint_block_ids => pending_hint_ids;
        channel1_reduce_suppressed => boolean;
        last_execute_ordinal => int;
        last_emergency_input_sample => 24;
        has_prior_emergency_drop => boolean;
        deferred_execute_state => directive;
        pending_compaction_marker => text(memory_store::MAX_STATE_SYNC_ID_BYTES) + 96;
        newest_live_ordinal => int;
        descent_completed => boolean;
        lineage_descent_target_key => request_identity;
        lineage_descent_edge_id => int;
        lineage_descent_disposition => label;
        lineage_descent_source_key => request_identity;
        ordinal_continuation_base => int;
        anchor_block_id => block_id;
        anchor_content_hash => hash;
        lineage_descent_materialized => boolean;
        lineage_descent_counters => 512;
        channel2_nudge_state => text(memory_store::MAX_STATE_SYNC_DIRECTIVE_BYTES);
        pending_channel2_directive => 2 * 1024;
        channel2_pressure_latched => boolean;
        channel2_arming_watermark => int;
        emergency_drain_active => boolean;
        emergency_drain_entered_at_ms => int;
        last_committed_pass_at_ms => int;
        shadow_generation => int;
        shadow_seq => int;
        shadow_quarantined => boolean;
        shadow_quarantined_pass_count => int;
        shadow_acked_watermarks => memory_store::MAX_ACKED_WATERMARKS_BYTES;
        eidnara_folds => boolean;
    })
}

#[test]
fn every_metadata_field_has_a_recorded_bound_within_the_headroom() {
    let table = recorded_metadata_bounds();
    let total = object(&table);
    assert!(
        total <= 3 * (512 * 1024) / 4,
        "the recorded bounds sum to {total} bytes, over three quarters of the 512 KiB durable-text guard"
    );
    let with_selection = total + SELECTED_IDENTITY_BUDGET_BYTES;
    assert!(
        with_selection <= 384 * 1024,
        "the recorded bounds and the selected identities sum to {with_selection} bytes"
    );
    eprintln!(
        "recorded metadata bounds: {total} bytes over {} fields, {with_selection} bytes with the selected identities",
        table.len()
    );
}

/// Each U+0001 character serializes as the six-byte JSON escape `\u0001`.
/// The labels, separators, length prefixes, and epoch parts a stored render identity adds to its
/// five request-supplied strings: `additive-only-v2|`, `provider:`, `model:`, `system:`, the
/// `|m0epoch[...]` fold, and every optional epoch part at its generated length.
const RENDER_IDENTITY_FRAMING_BYTES: usize = 512;

fn escaped(bytes: usize) -> String {
    "\u{1}".repeat(bytes)
}

fn redacting(max_bytes: usize) -> String {
    let secret = "password=abc next ";
    let text = secret.repeat(max_bytes / secret.len());
    assert!(text.len() <= max_bytes);
    text
}

fn near_budget_selection() -> Vec<memory_store::HistorySummarizerSelectedMessageIdentity> {
    let mut selected = Vec::new();
    let mut serialized = "[]".len();
    for n in 1.. {
        let entry = memory_store::HistorySummarizerSelectedMessageIdentity {
            mid: format!("m{n}"),
            block_identities: vec![memory_store::BlockIdentity {
                kind_tag: "text".to_string(),
                byte_fingerprint: crate::wire::fingerprint(&format!("message {n} with some text")),
            }],
        };
        let next = serialized
            + usize::from(!selected.is_empty())
            + serde_json::to_vec(&entry).unwrap().len();
        if next > SELECTED_IDENTITY_BUDGET_BYTES {
            break;
        }
        serialized = next;
        selected.push(entry);
    }
    assert_eq!(serde_json::to_vec(&selected).unwrap().len(), serialized);
    assert!(
        SELECTED_IDENTITY_BUDGET_BYTES - serialized < 256,
        "{serialized}"
    );
    selected
}

fn note_nudge_anchor_at_bound(n: usize) -> memory_store::NoteNudgeAnchorSeed {
    let mut anchor = memory_store::NoteNudgeAnchorSeed {
        message_id: format!("m{n}"),
        text: String::new(),
    };
    let empty = serde_json::to_vec(&anchor).unwrap().len();
    anchor.text = "n".repeat(memory_store::MAX_NOTE_NUDGE_ANCHOR_BYTES - empty);
    assert_eq!(
        serde_json::to_vec(&anchor).unwrap().len(),
        memory_store::MAX_NOTE_NUDGE_ANCHOR_BYTES
    );
    anchor
}

fn acked_watermarks_near_bound() -> serde_json::Value {
    let mut watermarks = serde_json::Map::new();
    for n in 0.. {
        let mut next = watermarks.clone();
        next.insert(format!("channel-{n}"), serde_json::Value::from(u64::MAX));
        if serde_json::to_vec(&next).unwrap().len() > memory_store::MAX_ACKED_WATERMARKS_BYTES {
            break;
        }
        watermarks = next;
    }
    serde_json::Value::Object(watermarks)
}

fn worst_case_module_meta() -> memory_store::ModuleMeta {
    let mid = crate::wire::MAX_MID_BYTES;
    let block_id = mid + 21;
    let identity = crate::transform::MAX_REQUEST_IDENTITY_BYTES;
    let mut history_summarizer = worst_case_history_summarizer();
    history_summarizer.selected_range_identities = near_budget_selection();
    memory_store::ModuleMeta {
        initialized: true,
        bootstrap_seed_fold_pending: true,
        last_render_config: format!(
            "{}{}",
            escaped(5 * identity),
            "f".repeat(RENDER_IDENTITY_FRAMING_BYTES)
        ),
        last_provider_id: escaped(identity),
        last_model_key: escaped(identity),
        last_system_prompt_hash: escaped(identity),
        last_upgrade_state: escaped(identity),
        coverage_ordinal: Some(u64::MAX),
        last_todo_state: Some("\"".repeat(memory_store::MAX_TODO_STATE_BYTES)),
        last_todo_state_owner_message_id: Some(escaped(mid)),
        last_todo_state_hash: Some(crate::wire::fingerprint("todo state")),
        soft_refresh_pending: true,
        guidance_date: escaped(32),
        revert_epoch: u64::MAX,
        last_recut: Some(redacting(memory_store::MAX_LAST_RECUT_BYTES)),
        pending_rewrite: Some(memory_store::PendingRewriteState {
            armed_at_ms: i64::MIN,
            absent_shape_fingerprint: crate::wire::fingerprint("absent shape"),
            absent_request_count: u64::MAX,
            last_present_at_ms: Some(i64::MIN),
        }),
        pending_rewrite_trip_count: u32::MAX,
        pending_rewrite_ambiguous: true,
        pending_rewrite_last_failure: Some(redacting(
            memory_store::MAX_PENDING_REWRITE_DETAIL_BYTES,
        )),
        synthetic_todo: Some(worst_case_synthetic_todo()),
        note_nudge_anchors: (0..memory_store::MAX_NOTE_NUDGE_ANCHORS)
            .map(note_nudge_anchor_at_bound)
            .collect(),
        m1_revision: u64::MAX,
        m1_history_segment_seq: Some(i64::MIN),
        memory_disabled: true,
        m1_external_revision: u64::MAX,
        project_memory_epoch: u64::MAX,
        project_memory_epoch_pending: true,
        user_profile_version: u64::MAX,
        m1_user_profile_version: u64::MAX,
        m1_pending_since_ms: Some(i64::MIN),
        folded_history_segment_seq: i64::MIN,
        archive_fold_seq: Some(i64::MIN),
        legacy_history_segment_seqs: Some(vec![
            i64::MIN;
            memory_store::MAX_LEGACY_HISTORY_SEGMENTS
        ]),
        history_segments_ordered: true,
        coverage_start_ordinal: Some(u64::MAX),
        coverage_history_segment_seq: Some(i64::MIN),
        additive_served_history_segment_seq: Some(i64::MIN),
        project_memory: Some(memory_store::ProjectMemoryComposition::Canonical {
            known_as_of: i64::MIN,
            truncated: true,
            revision: u64::MAX,
        }),
        expiry_cutoff_ms: i64::MIN,
        history_summarizer,
        publication_floor_ordinal: Some(u64::MAX),
        block_identity_basis: memory_store::BlockIdentityBasis::Replay,
        tail_identity_re_adopt_count: u64::MAX,
        newest_live_block_id: Some(escaped(block_id)),
        last_usage: Some(memory_store::ModuleUsage {
            current_total_input_tokens: u64::MAX,
            context_limit_tokens: u64::MAX,
            final_wire_input_tokens: u64::MAX,
            final_wire_trusted: true,
        }),
        last_serializer_profile: escaped(32),
        reasoning_cleared_through_ordinal: u64::MAX,
        reasoning_cleared_through_tag: u64::MAX,
        terse_text_compression_age_basis_tag: u64::MAX,
        cc_u1_active: true,
        tagging_surface_active: true,
        channel1_last_nudge_undropped: i64::MIN,
        channel1_last_nudge_level: escaped(32),
        tail_hygiene_baseline: Some(memory_store::TailHygieneBaseline {
            baseline_u: i64::MIN,
            baseline_t: i64::MIN,
            turn_delta_u: i64::MIN,
            turn_delta_t: i64::MIN,
            baseline_generation: u64::MAX,
            computed_at_ms: i64::MIN,
            evaluable: true,
            generation_invalidated: true,
            content_signature: crate::wire::fingerprint("tail")[..32].to_string(),
        }),
        pending_user_hint_block_ids: (0..crate::transform::MAX_PENDING_USER_HINT_BLOCK_IDS)
            .map(|n| format!("{n:02}{}", escaped(block_id - 2)))
            .collect(),
        channel1_reduce_suppressed: true,
        last_execute_ordinal: u64::MAX,
        last_emergency_input_sample: f64::MIN,
        has_prior_emergency_drop: true,
        deferred_execute_state: Some(memory_store::DeferredExecuteState {
            reason: escaped(memory_store::MAX_STATE_SYNC_DIRECTIVE_BYTES),
        }),
        pending_compaction_marker: Some(memory_store::PendingCompactionMarkerState {
            ordinal: u64::MAX,
            end_message_id: escaped(memory_store::MAX_STATE_SYNC_ID_BYTES),
            published_at: i64::MIN,
        }),
        newest_live_ordinal: u64::MAX,
        descent_completed: true,
        lineage_descent_target_key: escaped(identity),
        lineage_descent_edge_id: u64::MAX,
        lineage_descent_disposition: escaped(32),
        lineage_descent_source_key: Some(escaped(identity)),
        ordinal_continuation_base: Some(u64::MAX),
        anchor_block_id: Some(escaped(block_id)),
        anchor_content_hash: Some(crate::wire::fingerprint("anchor")),
        lineage_descent_materialized: true,
        lineage_descent_counters: memory_store::LineageDescentCounters {
            compaction_seen: u64::MAX,
            compaction_answered: u64::MAX,
            fork_arm: u64::MAX,
            descended: u64::MAX,
            unknown_ancestor: u64::MAX,
            already_bootstrapped: u64::MAX,
            not_compaction_shape: u64::MAX,
            observed_flag_missing_shape_present: u64::MAX,
            cycle_detected: u64::MAX,
            pending_build_skew: u64::MAX,
            pending_no_responses: u64::MAX,
        },
        channel2_nudge_state: escaped(memory_store::MAX_STATE_SYNC_DIRECTIVE_BYTES),
        pending_channel2_directive: Some(memory_store::PendingChannel2Directive {
            text: escaped(280),
            directive_id: escaped(32),
            armed_at_ms: i64::MIN,
            arming_watermark: u64::MAX,
        }),
        channel2_pressure_latched: true,
        channel2_arming_watermark: u64::MAX,
        emergency_drain_active: true,
        emergency_drain_entered_at_ms: i64::MIN,
        last_committed_pass_at_ms: i64::MIN,
        shadow_generation: u64::MAX,
        shadow_seq: u64::MAX,
        shadow_quarantined: true,
        shadow_quarantined_pass_count: u64::MAX,
        shadow_acked_watermarks: acked_watermarks_near_bound(),
        eidnara_folds: Some(true),
    }
}

#[test]
fn composition_witness_a_meta_with_every_field_near_its_bound_commits_and_reloads_within_the_total()
{
    let _serial = serial();
    let dir = tempfile::tempdir().expect("store dir");
    let store = store(dir.path());
    let meta = worst_case_module_meta();
    let selection = &meta.history_summarizer.selected_range_identities;
    let identities = memory_store::BlockIdentityDelta {
        upserts: selection
            .iter()
            .map(|selected| (selected.mid.clone(), selected.block_identities.clone()))
            .collect(),
        ..Default::default()
    };
    let empty = store.load(SESSION).expect("load empty");
    store
        .commit_with_block_identities_for_test(SESSION, None, &empty.core, &meta, &identities)
        .expect("the composite meta commits");

    let table = recorded_metadata_bounds();
    let total = object(&table);
    let stored_text = stored_meta_text(&store);
    let serde_json::Value::Object(mut stored) = serde_json::from_str(&stored_text).unwrap() else {
        panic!("the stored meta is an object");
    };
    let selected = stored
        .get_mut("history_summarizer")
        .and_then(|summarizer| summarizer.get_mut("selected_range_identities"))
        .map(serde_json::Value::take)
        .expect("the stored selection");
    let selected_bytes = serde_json::to_vec(&selected).unwrap().len();
    assert!(
        selected_bytes <= SELECTED_IDENTITY_BUDGET_BYTES,
        "the stored selection holds {selected_bytes} bytes"
    );
    stored["history_summarizer"]["selected_range_identities"] = serde_json::json!([]);
    for (field, value) in &stored {
        let bound = table
            .iter()
            .find(|(name, _)| name == field)
            .map(|(_, bound)| *bound)
            .unwrap_or_else(|| panic!("{field} is stored without a recorded bound"));
        let bytes = serde_json::to_vec(value).unwrap().len();
        assert!(
            bytes <= bound,
            "{field} stores {bytes} bytes over its {bound}"
        );
    }
    let stored_bytes = stored_text.len();
    assert!(
        stored_bytes <= total + SELECTED_IDENTITY_BUDGET_BYTES,
        "the composite meta stores {stored_bytes} bytes over {total} + {SELECTED_IDENTITY_BUDGET_BYTES}"
    );
    assert!(stored_bytes < 512 * 1024, "{stored_bytes}");
    assert_eq!(meta_row_bytes(&store), stored_bytes);
    eprintln!(
        "composite meta: {stored_bytes} bytes stored, {selected_bytes} selected, {total} recorded"
    );

    let reloaded = store
        .load(SESSION)
        .expect("the composite meta reloads")
        .meta;
    for (field, shaped, max_bytes) in [
        (
            "last_recut",
            reloaded.last_recut.as_deref(),
            memory_store::MAX_LAST_RECUT_BYTES,
        ),
        (
            "pending_rewrite_last_failure",
            reloaded.pending_rewrite_last_failure.as_deref(),
            memory_store::MAX_PENDING_REWRITE_DETAIL_BYTES,
        ),
    ] {
        let shaped = shaped.unwrap_or_else(|| panic!("{field} reloads"));
        assert!(shaped.contains("<REDACTED"), "{field}: {shaped}");
        assert!(!shaped.contains("password=abc"), "{field}: {shaped}");
        assert!(
            serde_json::to_string(shaped).unwrap().len() <= max_bytes + 2,
            "{field}"
        );
    }
    let mut expected = meta.clone();
    expected.last_recut = reloaded.last_recut.clone();
    expected.pending_rewrite_last_failure = reloaded.pending_rewrite_last_failure.clone();
    assert!(
        reloaded == expected,
        "the reload differs from the committed meta"
    );
    assert_eq!(
        store.all_block_identities_for_test(SESSION).len(),
        selection.len()
    );
}

struct MatrixCell {
    n: u64,
    w: u64,
}

impl MatrixCell {
    fn window(&self, eidnara_folds: bool) -> TransformRequest {
        let first = self.n - self.w + 1;
        let messages = (first..=self.n)
            .map(|n| item(&format!("m{n}"), n, &format!("message {n} with some text")))
            .collect();
        let request = req(SESSION, "cfg0", messages);
        if eidnara_folds {
            request
        } else {
            TransformRequest {
                boundary: None,
                ..request
            }
        }
    }
}

fn stored_meta_text(store: &MemoryStore) -> String {
    store
        .with_fenced_conn_for_test(|tx| {
            tx.query_row(
                "SELECT CAST(meta AS TEXT) FROM cache_state WHERE session_id = ?1",
                [SESSION],
                |row| row.get(0),
            )
        })
        .expect("meta text")
}

fn scalar_width_normalized(value: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::Null | Value::Bool(_) => value.clone(),
        Value::Number(_) => Value::from(0),
        Value::String(text) => {
            let hash = matches!(text.len(), 32 | 64)
                && text
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
            if hash {
                return Value::String("H".to_string());
            }
            let mut normalized = String::with_capacity(text.len());
            for run in text.split_inclusive(|character: char| !character.is_ascii_digit()) {
                let digits = run.bytes().take_while(u8::is_ascii_digit).count();
                if (1..=20).contains(&digits) {
                    normalized.push('0');
                    normalized.push_str(&run[digits..]);
                } else {
                    normalized.push_str(run);
                }
            }
            Value::String(normalized)
        }
        Value::Array(items) => Value::Array(items.iter().map(scalar_width_normalized).collect()),
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, field)| (key.clone(), scalar_width_normalized(field)))
                .collect(),
        ),
    }
}

fn equal_apart_from_digit_width(sizes: &[(String, String)]) -> Result<(), String> {
    let shapes: Vec<(&String, usize)> = sizes
        .iter()
        .map(|(cell, meta)| {
            let normalized =
                scalar_width_normalized(&serde_json::from_str(meta).expect("meta is JSON"));
            (cell, serde_json::to_string(&normalized).unwrap().len())
        })
        .collect();
    let smallest = shapes.iter().map(|(_, len)| *len).min().unwrap();
    let largest = shapes.iter().map(|(_, len)| *len).max().unwrap();
    if (largest - smallest) * 100 > smallest {
        return Err(format!(
            "metadata shapes differ by more than 1%: {shapes:?}"
        ));
    }
    Ok(())
}

fn matrix_meta(cell: &MatrixCell, eidnara_folds: bool, active_firing: bool, grow: bool) -> String {
    let dir = tempfile::tempdir().expect("store dir");
    let store = Arc::new(store(dir.path()));
    if eidnara_folds {
        let h = ((cell.n - cell.w) / 2 + 1) as usize;
        SyntheticHistory::mixed(h).seed(&store, SESSION);
        let empty = store.load(SESSION).expect("load empty");
        let mut meta = empty.meta.clone();
        meta.ordinal_continuation_base = Some(cell.n - cell.w);
        store
            .commit(SESSION, None, &empty.core, &meta)
            .expect("seed the continuation base");
    }
    let mut ctx = pctx("git:meta-bound", "/nonexistent-docs", 1_700_000_000_000);
    ctx.fold_authority.eidnara_folds = eidnara_folds;
    ctx.history_summarizer_active = active_firing;
    let window = cell.window(eidnara_folds);
    let run = |request: &TransformRequest| {
        transform_with_projection_cached(&store, &resolved(&store, request), &ctx)
            .unwrap_or_else(|error| panic!("n={} w={} the pass commits: {error}", cell.n, cell.w))
            .response
    };
    let first = run(&window);
    assert!(
        first.committed,
        "n={} w={} the first pass commits",
        cell.n, cell.w
    );
    assert_eq!(
        store.load(SESSION).unwrap().meta.eidnara_folds,
        Some(eidnara_folds)
    );
    if active_firing {
        seed_active_summarizer(&store, SESSION);
        let loaded = store.load(SESSION).expect("load");
        let mids: Vec<String> = window.messages[..100]
            .iter()
            .map(|message| message.mid.clone())
            .collect();
        let selected = store
            .load_block_identities(
                SESSION,
                &mids.iter().map(String::as_str).collect::<Vec<_>>(),
            )
            .expect("selected identities");
        let mut meta = loaded.meta.clone();
        meta.history_summarizer.selected_range_identities = selected
            .into_iter()
            .map(
                |(mid, block_identities)| memory_store::HistorySummarizerSelectedMessageIdentity {
                    mid,
                    block_identities,
                },
            )
            .collect();
        store
            .commit(SESSION, loaded.row_version, &loaded.core, &meta)
            .expect("the firing's selection commits");
    }
    let mut next = window.clone();
    let tail = cell.n + 1;
    next.messages.push(item(
        &format!("m{tail}"),
        tail,
        &format!("message {tail} with some text"),
    ));
    let before_tail = store.load(SESSION).unwrap().row_version;
    let tail_pass = run(&next);
    assert_eq!(tail_pass.status, crate::transform::TransformStatus::Ok);
    let after = store.load(SESSION).unwrap();
    assert_eq!(after.meta.eidnara_folds, Some(eidnara_folds));
    assert_eq!(
        after.row_version > before_tail,
        tail_pass.committed,
        "the tail pass's commit is durable"
    );
    let selected = after
        .meta
        .history_summarizer
        .selected_range_identities
        .len();
    assert_eq!(
        selected,
        if eidnara_folds && active_firing {
            100
        } else {
            0
        }
    );
    if grow {
        let loaded = store.load(SESSION).expect("load");
        let mut meta = loaded.meta.clone();
        meta.pending_user_hint_block_ids = next
            .messages
            .iter()
            .map(|message| format!("{}#0", message.mid))
            .collect();
        store
            .commit(SESSION, loaded.row_version, &loaded.core, &meta)
            .expect("the restored growing field commits");
    }
    let meta = stored_meta_text(&store);
    assert!(
        meta.len() < 512 * 1024,
        "the durable-text guard is not reached"
    );
    meta
}

#[test]
fn module_meta_size_is_independent_of_message_count_and_window_size() {
    let _serial = serial();
    let cells = [
        MatrixCell { n: 1_000, w: 300 },
        MatrixCell { n: 20_000, w: 300 },
        MatrixCell {
            n: 20_000,
            w: 5_000,
        },
    ];
    let label = |cell: &MatrixCell| format!("n={} w={}", cell.n, cell.w);
    for (eidnara_folds, active_firing) in
        [(true, false), (true, true), (false, false), (false, true)]
    {
        let metas: Vec<(String, String)> = cells
            .iter()
            .map(|cell| {
                (
                    label(cell),
                    matrix_meta(cell, eidnara_folds, active_firing, false),
                )
            })
            .collect();
        let lengths: Vec<_> = metas
            .iter()
            .map(|(cell, meta)| (cell, meta.len()))
            .collect();
        eprintln!("eidnara_folds={eidnara_folds} active_firing={active_firing} {lengths:?}");
        equal_apart_from_digit_width(&metas).unwrap_or_else(|error| {
            panic!("eidnara_folds={eidnara_folds} active_firing={active_firing}: {error}")
        });
    }
    let restored: Vec<(String, String)> = [&cells[1], &cells[2]]
        .iter()
        .map(|cell| (label(cell), matrix_meta(cell, true, false, true)))
        .collect();
    assert!(
        equal_apart_from_digit_width(&restored).is_err(),
        "a field that grows with the window fails the comparison"
    );
    let digits = |count: usize| (String::new(), format!(r#"{{"w":"{}"}}"#, "7".repeat(count)));
    assert!(
        equal_apart_from_digit_width(&[digits(300), digits(5_000)]).is_err(),
        "a digit-only string that grows fails the comparison"
    );
    let zeros = |count: usize| {
        (
            String::new(),
            format!(r#"{{"w":[{}]}}"#, vec!["0"; count].join(",")),
        )
    };
    assert!(
        equal_apart_from_digit_width(&[zeros(300), zeros(5_000)]).is_err(),
        "an array of one-digit numbers that grows fails the comparison"
    );
    let escaped = |text: String| (String::new(), serde_json::json!({ "w": text }).to_string());
    assert!(
        equal_apart_from_digit_width(&[escaped("a".repeat(300)), escaped("\u{1}".repeat(300))])
            .is_err(),
        "a string that grows only in escaping cost fails the comparison"
    );
    assert!(
        equal_apart_from_digit_width(&[escaped("a".repeat(300)), escaped("b".repeat(300))]).is_ok()
    );
    let widths = |value: u64| (String::new(), format!(r#"{{"w":{value},"m":"m{value}"}}"#));
    assert!(equal_apart_from_digit_width(&[widths(7), widths(70_000)]).is_ok());
}

#[test]
fn a_render_identity_from_five_escaped_inputs_at_their_bound_fits_its_allowance() {
    let _serial = serial();
    let dir = tempfile::tempdir().expect("store dir");
    let store = store(dir.path());
    let at_bound = || escaped(crate::transform::MAX_REQUEST_IDENTITY_BYTES);
    let mut request = req(SESSION, &at_bound(), vec![item("m1", 1, "hello")]);
    request.provider_id = Some(at_bound());
    request.model_key = Some(at_bound());
    request.system_prompt_hash = at_bound();
    request.upgrade_state = at_bound();
    pass(&store, &request).expect("the pass commits");
    let stored = store.load(SESSION).unwrap().meta.last_render_config;
    let payload = 5 * crate::transform::MAX_REQUEST_IDENTITY_BYTES;
    assert_eq!(stored.matches('\u{1}').count(), payload, "{stored:?}");
    let framing = stored.len() - payload;
    assert!(
        framing <= RENDER_IDENTITY_FRAMING_BYTES,
        "the render identity adds {framing} framing bytes"
    );
    let recorded = recorded_metadata_bounds()
        .into_iter()
        .find(|(field, _)| *field == "last_render_config")
        .map(|(_, bound)| bound)
        .unwrap();
    let serialized = serde_json::to_string(&stored).unwrap().len();
    assert!(serialized <= recorded, "{serialized} over {recorded}");
}

#[test]
fn request_identity_strings_over_their_bound_are_refused_before_any_read() {
    let _serial = serial();
    let dir = tempfile::tempdir().expect("store dir");
    let store = store(dir.path());
    let at_bound = "\u{e9}".repeat(crate::transform::MAX_REQUEST_IDENTITY_BYTES / 2);
    let over = format!("{at_bound}x");
    let base = req(SESSION, "cfg0", vec![item("m1", 1, "hello")]);
    type SetField = fn(&mut TransformRequest, String);
    let variants: [(&str, SetField); 7] = [
        ("session_id", |request, value| request.session_id = value),
        ("render_config", |request, value| {
            request.render_config = value
        }),
        ("system_prompt_hash", |request, value| {
            request.system_prompt_hash = value
        }),
        ("upgrade_state", |request, value| {
            request.upgrade_state = value
        }),
        ("provider_id", |request, value| {
            request.provider_id = Some(value)
        }),
        ("model_key", |request, value| {
            request.model_key = Some(value)
        }),
        ("prior_conversation_key", |request, value| {
            request.prior_conversation_key = value
        }),
    ];
    for (field, set) in variants {
        let mut accepted = base.clone();
        set(&mut accepted, at_bound.clone());
        assert!(
            crate::transform::resolve_window(&store, &mut accepted).is_ok(),
            "{field} at its bound"
        );
        let mut refused = base.clone();
        set(&mut refused, over.clone());
        let before = refused.clone();
        let error = crate::transform::resolve_window(&store, &mut refused).unwrap_err();
        assert!(
            matches!(&error, TransformError::InvalidWindow(message) if message.starts_with(field)),
            "{field}: {error:?}"
        );
        assert_eq!(refused.messages.len(), before.messages.len(), "{field}");
    }
    assert!(store.load(SESSION).unwrap().row_version.is_none());
}
